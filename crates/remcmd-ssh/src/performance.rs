use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{
    sync::{mpsc, oneshot},
    time::{MissedTickBehavior, interval},
};

use crate::{ConnectionEvent, SshError, SshErrorKind, SshTransport};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

const PROC_CHECK: &str = r#"if [ ! -r /proc/stat ] || [ ! -r /proc/meminfo ]; then
    printf '%s\n' 'Performance monitoring requires a Linux server with /proc' >&2
    exit 69
fi
"#;

// One awk process reads every counter so each sample forks only `df` and
// `awk`. Loopback traffic and loop/RAM block devices are excluded because they
// do not represent host network or disk activity.
const SAMPLE_PIPELINE: &str = r#"df -Pk / 2>/dev/null | awk -v proc=/proc -v sys=/sys '
NR == 2 { disk_total = $2; disk_available = $4 }
END {
    if ((getline host < (proc "/sys/kernel/hostname")) <= 0) host = "Remote host"
    printf "hostname=%s\n", host
    cpus = 0
    while ((getline line < (proc "/stat")) > 0) {
        n = split(line, f, " ")
        if (f[1] != "cpu" && f[1] !~ /^cpu[0-9]+$/) continue
        total = 0
        for (i = 2; i <= 9 && i <= n; i++) total += f[i]
        if (f[1] == "cpu") {
            printf "cpu_total=%.0f\ncpu_idle=%.0f\ncpu_iowait=%.0f\n", total, f[5] + f[6], f[6]
        } else {
            printf "cpu_core=%s,%.0f,%.0f\n", substr(f[1], 4), total, f[5] + f[6]
            cpus++
        }
    }
    while ((getline line < (proc "/meminfo")) > 0) {
        split(line, f, " ")
        memory[f[1]] = f[2]
    }
    available = ("MemAvailable:" in memory) ? memory["MemAvailable:"] : memory["MemFree:"] + memory["Buffers:"] + memory["Cached:"]
    printf "memory_total_bytes=%.0f\nmemory_available_bytes=%.0f\nswap_total_bytes=%.0f\nswap_free_bytes=%.0f\n", memory["MemTotal:"] * 1024, available * 1024, memory["SwapTotal:"] * 1024, memory["SwapFree:"] * 1024
    if ((getline line < (proc "/loadavg")) > 0) {
        split(line, f, " ")
        split(f[4], processes, "/")
        printf "load_one_milli=%.0f\nload_five_milli=%.0f\nload_fifteen_milli=%.0f\nprocesses_running=%s\nprocesses_total=%s\n", f[1] * 1000, f[2] * 1000, f[3] * 1000, processes[1], processes[2]
    }
    if ((getline line < (proc "/uptime")) > 0) {
        split(line, f, " ")
        printf "uptime_seconds=%.0f\n", f[1]
    }
    rx = 0
    tx = 0
    while ((getline line < (proc "/net/dev")) > 0) {
        separator = index(line, ":")
        if (!separator) continue
        name = substr(line, 1, separator - 1)
        gsub(/[ \t]/, "", name)
        if (name == "lo") continue
        split(substr(line, separator + 1), f, " ")
        rx += f[1]
        tx += f[9]
    }
    printf "network_rx_bytes=%.0f\nnetwork_tx_bytes=%.0f\n", rx, tx
    disk = 0
    while ((getline line < (proc "/diskstats")) > 0) {
        split(line, f, " ")
        if (f[3] ~ /^(loop|ram|zram)/) continue
        device = sys "/dev/block/" f[1] ":" f[2]
        readable = (getline value < (device "/dev")) > 0
        close(device "/dev")
        if (!readable) continue
        partition = (getline value < (device "/partition")) > 0
        close(device "/partition")
        if (partition) continue
        disk = 1
        sectors_read += f[6]
        sectors_written += f[10]
    }
    if (disk) printf "disk_read_bytes=%.0f\ndisk_write_bytes=%.0f\n", sectors_read * 512, sectors_written * 512
    if (disk_total != "") printf "disk_total_bytes=%.0f\ndisk_available_bytes=%.0f\n", disk_total * 1024, disk_available * 1024
    printf "cpu_count=%d\n", (cpus > 0 ? cpus : 1)
}'
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LogicalCpuSnapshot {
    pub id: u32,
    pub total: u64,
    pub idle: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerPerformanceSnapshot {
    pub hostname: String,
    pub cpu_total: u64,
    pub cpu_idle: u64,
    pub cpu_iowait: u64,
    pub cpu_count: u32,
    pub logical_cpus: Vec<LogicalCpuSnapshot>,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
    pub load_one_milli: u32,
    pub load_five_milli: u32,
    pub load_fifteen_milli: u32,
    pub processes_running: u32,
    pub processes_total: u32,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub disk_read_bytes: Option<u64>,
    pub disk_write_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub disk_available_bytes: Option<u64>,
    pub uptime_seconds: u64,
    pub ssh_response_time: Duration,
}

pub(crate) struct PerformanceMonitorHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl PerformanceMonitorHandle {
    pub(crate) fn spawn(
        transport: Arc<SshTransport>,
        events: mpsc::Sender<ConnectionEvent>,
    ) -> Self {
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            let mut timer = interval(SAMPLE_INTERVAL);
            timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = timer.tick() => {
                        let event = match collect_snapshot(&transport).await {
                            Ok(snapshot) => ConnectionEvent::PerformanceSnapshot(snapshot),
                            Err(error) => ConnectionEvent::PerformanceFailed(error),
                        };
                        if events.send(event).await.is_err() {
                            break;
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        Self {
            shutdown_tx: Some(shutdown_tx),
        }
    }
}

impl Drop for PerformanceMonitorHandle {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

async fn collect_snapshot(transport: &SshTransport) -> Result<ServerPerformanceSnapshot, SshError> {
    let started_at = Instant::now();
    let output = transport
        .execute(&format!("{PROC_CHECK}{SAMPLE_PIPELINE}"))
        .await?;
    let mut snapshot = parse_snapshot(&output)?;
    snapshot.ssh_response_time = started_at.elapsed();
    Ok(snapshot)
}

fn parse_snapshot(output: &[u8]) -> Result<ServerPerformanceSnapshot, SshError> {
    let output = std::str::from_utf8(output).map_err(|_| {
        SshError::new(
            SshErrorKind::Protocol,
            "server performance response was not UTF-8",
        )
    })?;

    let value = |key: &str| {
        output.lines().find_map(|line| {
            let (candidate, value) = line.split_once('=')?;
            (candidate == key).then_some(value.trim())
        })
    };
    let parse_u64 = |key: &str| -> Result<u64, SshError> {
        value(key)
            .ok_or_else(|| performance_field_error(key))?
            .parse()
            .map_err(|_| performance_field_error(key))
    };
    let parse_u32 = |key: &str| -> Result<u32, SshError> {
        value(key)
            .ok_or_else(|| performance_field_error(key))?
            .parse()
            .map_err(|_| performance_field_error(key))
    };
    let optional_u64 = |key: &str| -> Result<Option<u64>, SshError> {
        value(key)
            .map(|value| value.parse().map_err(|_| performance_field_error(key)))
            .transpose()
    };
    let mut logical_cpus = output
        .lines()
        .filter_map(|line| line.strip_prefix("cpu_core="))
        .map(parse_logical_cpu)
        .collect::<Result<Vec<_>, _>>()?;
    logical_cpus.sort_unstable_by_key(|cpu| cpu.id);
    if logical_cpus.windows(2).any(|cpus| cpus[0].id == cpus[1].id) {
        return Err(SshError::new(
            SshErrorKind::Protocol,
            "server performance response contained duplicate logical CPUs",
        ));
    }

    let cpu_total = parse_u64("cpu_total")?;
    let cpu_idle = parse_u64("cpu_idle")?;
    let cpu_iowait = parse_u64("cpu_iowait")?;
    let memory_total_bytes = parse_u64("memory_total_bytes")?;
    let memory_available_bytes = parse_u64("memory_available_bytes")?;
    let swap_total_bytes = parse_u64("swap_total_bytes")?;
    let swap_free_bytes = parse_u64("swap_free_bytes")?;
    let processes_running = parse_u32("processes_running")?;
    let processes_total = parse_u32("processes_total")?;
    if cpu_idle > cpu_total
        || cpu_iowait > cpu_total
        || logical_cpus.iter().any(|cpu| cpu.idle > cpu.total)
        || memory_available_bytes > memory_total_bytes
        || swap_free_bytes > swap_total_bytes
        || processes_running > processes_total
    {
        return Err(SshError::new(
            SshErrorKind::Protocol,
            "server performance response contained invalid counters",
        ));
    }

    Ok(ServerPerformanceSnapshot {
        hostname: value("hostname").unwrap_or("Remote host").to_owned(),
        cpu_total,
        cpu_idle,
        cpu_iowait,
        cpu_count: parse_u32("cpu_count")?.max(1),
        logical_cpus,
        memory_total_bytes,
        memory_available_bytes,
        swap_total_bytes,
        swap_free_bytes,
        load_one_milli: parse_u32("load_one_milli")?,
        load_five_milli: parse_u32("load_five_milli")?,
        load_fifteen_milli: parse_u32("load_fifteen_milli")?,
        processes_running,
        processes_total,
        network_rx_bytes: parse_u64("network_rx_bytes")?,
        network_tx_bytes: parse_u64("network_tx_bytes")?,
        disk_read_bytes: optional_u64("disk_read_bytes")?,
        disk_write_bytes: optional_u64("disk_write_bytes")?,
        disk_total_bytes: optional_u64("disk_total_bytes")?,
        disk_available_bytes: optional_u64("disk_available_bytes")?,
        uptime_seconds: parse_u64("uptime_seconds")?,
        ssh_response_time: Duration::ZERO,
    })
}

fn parse_logical_cpu(value: &str) -> Result<LogicalCpuSnapshot, SshError> {
    let mut fields = value.split(',');
    let id = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| performance_field_error("cpu_core"))?;
    let total = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| performance_field_error("cpu_core"))?;
    let idle = fields
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| performance_field_error("cpu_core"))?;
    if fields.next().is_some() {
        return Err(performance_field_error("cpu_core"));
    }
    Ok(LogicalCpuSnapshot { id, total, idle })
}

fn performance_field_error(field: &str) -> SshError {
    SshError::new(
        SshErrorKind::Protocol,
        format!("server performance response omitted or invalid {field}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"hostname=demo\n\
cpu_total=1000\n\
cpu_idle=750\n\
cpu_iowait=50\n\
cpu_core=0,510,390\n\
cpu_core=1,490,360\n\
memory_total_bytes=8589934592\n\
memory_available_bytes=5368709120\n\
swap_total_bytes=2147483648\n\
swap_free_bytes=1610612736\n\
load_one_milli=125\n\
load_five_milli=250\n\
load_fifteen_milli=500\n\
processes_running=2\n\
processes_total=184\n\
uptime_seconds=86461\n\
network_rx_bytes=1048576\n\
network_tx_bytes=524288\n\
disk_read_bytes=2097152\n\
disk_write_bytes=1048576\n\
disk_total_bytes=107374182400\n\
disk_available_bytes=64424509440\n\
cpu_count=8\n";

    #[test]
    fn parses_linux_performance_snapshot() {
        let snapshot = parse_snapshot(SAMPLE).expect("valid performance snapshot");

        assert_eq!(snapshot.hostname, "demo");
        assert_eq!(snapshot.cpu_total, 1000);
        assert_eq!(snapshot.cpu_idle, 750);
        assert_eq!(snapshot.cpu_iowait, 50);
        assert_eq!(snapshot.cpu_count, 8);
        assert_eq!(
            snapshot.logical_cpus,
            vec![
                LogicalCpuSnapshot {
                    id: 0,
                    total: 510,
                    idle: 390,
                },
                LogicalCpuSnapshot {
                    id: 1,
                    total: 490,
                    idle: 360,
                },
            ]
        );
        assert_eq!(snapshot.memory_total_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(snapshot.swap_total_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(snapshot.processes_total, 184);
        assert_eq!(snapshot.load_fifteen_milli, 500);
        assert_eq!(snapshot.disk_read_bytes, Some(2 * 1024 * 1024));
        assert_eq!(snapshot.disk_available_bytes, Some(60 * 1024 * 1024 * 1024));
        assert_eq!(snapshot.ssh_response_time, Duration::ZERO);
    }

    #[test]
    fn permits_missing_optional_disk_metrics() {
        let input = SAMPLE
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.starts_with(b"disk_"))
            .flat_map(|line| line.iter().copied().chain([b'\n']))
            .collect::<Vec<_>>();

        let snapshot = parse_snapshot(&input).expect("disk metrics are optional");

        assert_eq!(snapshot.disk_total_bytes, None);
        assert_eq!(snapshot.disk_available_bytes, None);
    }

    #[cfg(unix)]
    #[test]
    fn sample_pipeline_reads_linux_counters_in_one_awk_process() {
        let root = tempfile::tempdir().unwrap();
        let write = |path: &str, contents: &str| {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        };
        write("proc/sys/kernel/hostname", "demo\n");
        write(
            "proc/stat",
            "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 1 2 3\n",
        );
        write(
            "proc/meminfo",
            "MemTotal:       8000 kB\nMemFree:        1000 kB\nMemAvailable:   5000 kB\nSwapTotal:      2000 kB\nSwapFree:       1500 kB\n",
        );
        write("proc/loadavg", "0.12 0.25 0.50 2/184 999\n");
        write("proc/uptime", "86461.42 1000.00\n");
        write(
            "proc/net/dev",
            "Inter-|   Receive |  Transmit\n face |bytes packets|bytes\n    lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n  eth0:1000 1 0 0 0 0 0 0 500 1 0 0 0 0 0 0\n  eth1: 24 1 0 0 0 0 0 0 12 1 0 0 0 0 0 0\n",
        );
        write(
            "proc/diskstats",
            "   8       0 sda 1 0 10 0 1 0 20 0 0 0 0\n   8       1 sda1 1 0 7 0 1 0 9 0 0 0 0\n   7       0 loop0 1 0 99 0 1 0 99 0 0 0 0\n",
        );
        write("sys/dev/block/8:0/dev", "8:0\n");
        write("sys/dev/block/8:1/dev", "8:1\n");
        write("sys/dev/block/8:1/partition", "1\n");
        write("sys/dev/block/7:0/dev", "7:0\n");

        let script = SAMPLE_PIPELINE.replacen(
            "-v proc=/proc -v sys=/sys",
            &format!("-v proc={0}/proc -v sys={0}/sys", root.path().display()),
            1,
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .expect("POSIX shell should run the sample pipeline");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let snapshot = parse_snapshot(&output.stdout).expect("pipeline output should parse");
        assert_eq!(snapshot.hostname, "demo");
        assert_eq!(snapshot.cpu_total, 1000);
        assert_eq!(snapshot.cpu_idle, 850);
        assert_eq!(snapshot.cpu_iowait, 50);
        assert_eq!(snapshot.cpu_count, 2);
        assert_eq!(snapshot.logical_cpus.len(), 2);
        assert_eq!(snapshot.memory_available_bytes, 5000 * 1024);
        assert_eq!(snapshot.swap_free_bytes, 1500 * 1024);
        assert_eq!(snapshot.load_five_milli, 250);
        assert_eq!(snapshot.processes_total, 184);
        assert_eq!(snapshot.uptime_seconds, 86461);
        assert_eq!(snapshot.network_rx_bytes, 1024);
        assert_eq!(snapshot.network_tx_bytes, 512);
        assert_eq!(snapshot.disk_read_bytes, Some(10 * 512));
        assert_eq!(snapshot.disk_write_bytes, Some(20 * 512));
        assert!(snapshot.disk_total_bytes.is_some());
    }

    #[test]
    fn rejects_inconsistent_counters() {
        let input = String::from_utf8(SAMPLE.to_vec())
            .unwrap()
            .replace("cpu_idle=750", "cpu_idle=1001");

        let error = parse_snapshot(input.as_bytes()).expect_err("invalid counters must fail");

        assert_eq!(error.kind(), SshErrorKind::Protocol);
    }

    #[test]
    fn rejects_duplicate_logical_cpus() {
        let input = String::from_utf8(SAMPLE.to_vec())
            .unwrap()
            .replace("cpu_core=1,490,360", "cpu_core=0,490,360");

        let error = parse_snapshot(input.as_bytes()).expect_err("CPU ids must be unique");

        assert_eq!(error.kind(), SshErrorKind::Protocol);
    }
}
