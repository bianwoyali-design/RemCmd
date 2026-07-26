use std::{sync::Arc, time::Duration};

use tokio::{
    sync::{mpsc, oneshot},
    time::{MissedTickBehavior, interval},
};

use crate::{ConnectionEvent, SshError, SshErrorKind, SshTransport};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

const PERFORMANCE_COMMAND: &str = r#"if [ ! -r /proc/stat ] || [ ! -r /proc/meminfo ]; then
    printf '%s\n' 'Performance monitoring requires a Linux server with /proc' >&2
    exit 69
fi
printf 'hostname=%s\n' "$(uname -n 2>/dev/null || printf 'Remote host')"
awk '/^cpu / { total=0; for (i=2; i<=9 && i<=NF; i++) total += $i; idle=$5+$6; printf "cpu_total=%.0f\ncpu_idle=%.0f\n", total, idle; exit }' /proc/stat
awk '/^MemTotal:/ { total=$2 } /^MemAvailable:/ { available=$2; found=1 } /^MemFree:/ { free=$2 } /^Buffers:/ { buffers=$2 } /^Cached:/ { cached=$2 } END { if (!found) available=free+buffers+cached; printf "memory_total_bytes=%.0f\nmemory_available_bytes=%.0f\n", total*1024, available*1024 }' /proc/meminfo
awk '{ printf "load_one_milli=%.0f\nload_five_milli=%.0f\nload_fifteen_milli=%.0f\n", $1*1000, $2*1000, $3*1000 }' /proc/loadavg
awk '{ printf "uptime_seconds=%.0f\n", $1 }' /proc/uptime
awk 'NR > 2 { rx += $2; tx += $10 } END { printf "network_rx_bytes=%.0f\nnetwork_tx_bytes=%.0f\n", rx, tx }' /proc/net/dev
df -Pk / 2>/dev/null | awk 'NR == 2 { printf "disk_total_bytes=%.0f\ndisk_available_bytes=%.0f\n", $2*1024, $4*1024 }'
printf 'cpu_count=%s\n' "$(getconf _NPROCESSORS_ONLN 2>/dev/null || printf '1')"
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerPerformanceSnapshot {
    pub hostname: String,
    pub cpu_total: u64,
    pub cpu_idle: u64,
    pub cpu_count: u32,
    pub memory_total_bytes: u64,
    pub memory_available_bytes: u64,
    pub load_one_milli: u32,
    pub load_five_milli: u32,
    pub load_fifteen_milli: u32,
    pub network_rx_bytes: u64,
    pub network_tx_bytes: u64,
    pub disk_total_bytes: Option<u64>,
    pub disk_available_bytes: Option<u64>,
    pub uptime_seconds: u64,
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
    let output = transport.execute(PERFORMANCE_COMMAND).await?;
    parse_snapshot(&output)
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

    let cpu_total = parse_u64("cpu_total")?;
    let cpu_idle = parse_u64("cpu_idle")?;
    let memory_total_bytes = parse_u64("memory_total_bytes")?;
    let memory_available_bytes = parse_u64("memory_available_bytes")?;
    if cpu_idle > cpu_total || memory_available_bytes > memory_total_bytes {
        return Err(SshError::new(
            SshErrorKind::Protocol,
            "server performance response contained invalid counters",
        ));
    }

    Ok(ServerPerformanceSnapshot {
        hostname: value("hostname").unwrap_or("Remote host").to_owned(),
        cpu_total,
        cpu_idle,
        cpu_count: parse_u32("cpu_count")?.max(1),
        memory_total_bytes,
        memory_available_bytes,
        load_one_milli: parse_u32("load_one_milli")?,
        load_five_milli: parse_u32("load_five_milli")?,
        load_fifteen_milli: parse_u32("load_fifteen_milli")?,
        network_rx_bytes: parse_u64("network_rx_bytes")?,
        network_tx_bytes: parse_u64("network_tx_bytes")?,
        disk_total_bytes: optional_u64("disk_total_bytes")?,
        disk_available_bytes: optional_u64("disk_available_bytes")?,
        uptime_seconds: parse_u64("uptime_seconds")?,
    })
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
memory_total_bytes=8589934592\n\
memory_available_bytes=5368709120\n\
load_one_milli=125\n\
load_five_milli=250\n\
load_fifteen_milli=500\n\
uptime_seconds=86461\n\
network_rx_bytes=1048576\n\
network_tx_bytes=524288\n\
disk_total_bytes=107374182400\n\
disk_available_bytes=64424509440\n\
cpu_count=8\n";

    #[test]
    fn parses_linux_performance_snapshot() {
        let snapshot = parse_snapshot(SAMPLE).expect("valid performance snapshot");

        assert_eq!(snapshot.hostname, "demo");
        assert_eq!(snapshot.cpu_total, 1000);
        assert_eq!(snapshot.cpu_idle, 750);
        assert_eq!(snapshot.cpu_count, 8);
        assert_eq!(snapshot.memory_total_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(snapshot.load_fifteen_milli, 500);
        assert_eq!(snapshot.disk_available_bytes, Some(60 * 1024 * 1024 * 1024));
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

    #[test]
    fn rejects_inconsistent_counters() {
        let input = String::from_utf8(SAMPLE.to_vec())
            .unwrap()
            .replace("cpu_idle=750", "cpu_idle=1001");

        let error = parse_snapshot(input.as_bytes()).expect_err("invalid counters must fail");

        assert_eq!(error.kind(), SshErrorKind::Protocol);
    }
}
