use super::{
    AnyElement, FontWeight, IconName, IntoElement, RemCmdApp, RightSidebarView,
    ServerPerformanceSnapshot, SessionId, SessionState, SharedString, div, format_remote_size, px,
};
use gpui::prelude::*;
use std::time::{Duration, Instant};

impl RemCmdApp {
    pub(super) fn sync_performance_monitoring(&mut self) {
        let target_session = (self.right_sidebar_open
            && self.right_sidebar_view == RightSidebarView::Performance)
            .then_some(self.active_session_id)
            .flatten();

        for session in &mut self.sessions {
            let should_monitor = target_session == Some(session.id)
                && session.connection_state == SessionState::Connected
                && session.connection_handle.is_some();
            if session.performance.monitoring == should_monitor {
                continue;
            }

            let result = session
                .connection_handle
                .as_ref()
                .map(|handle| handle.set_performance_monitoring(should_monitor));
            match result {
                Some(Ok(())) => {
                    session.performance.monitoring = should_monitor;
                    session.performance.loading =
                        should_monitor && session.performance.snapshot.is_none();
                    if should_monitor {
                        session.performance.error = None;
                    }
                }
                Some(Err(error)) => {
                    session.performance.monitoring = false;
                    session.performance.loading = false;
                    session.performance.error = Some(error.to_string());
                }
                None => {
                    session.performance.monitoring = false;
                    session.performance.loading = false;
                }
            }
        }
    }

    pub(super) fn render_server_performance(&self, session_id: SessionId) -> AnyElement {
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        if session.connection_state != SessionState::Connected {
            return div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.render_sidebar_icon(IconName::Performance, 20.0))
                .child(self.tr("performance-connect-hint"))
                .into_any_element();
        }

        let performance = &session.performance;
        let Some(snapshot) = performance.snapshot.as_ref() else {
            let message = performance
                .error
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| self.tr("performance-collecting"));
            return div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .px_3()
                .text_center()
                .text_sm()
                .text_color(if performance.error.is_some() {
                    self.theme.error_text
                } else {
                    self.theme.text_muted
                })
                .child(self.render_sidebar_icon(IconName::Performance, 20.0))
                .child(message)
                .into_any_element();
        };

        let cpu_usage = performance.cpu_usage.unwrap_or(0.0);
        let memory_used = snapshot
            .memory_total_bytes
            .saturating_sub(snapshot.memory_available_bytes);
        let memory_usage = percent(memory_used, snapshot.memory_total_bytes);
        let swap_used = snapshot
            .swap_total_bytes
            .saturating_sub(snapshot.swap_free_bytes);
        let swap_usage = percent(swap_used, snapshot.swap_total_bytes);
        let disk = snapshot
            .disk_total_bytes
            .zip(snapshot.disk_available_bytes)
            .filter(|(total, available)| *total > 0 && available <= total);
        let status_color = if performance.error.is_some() {
            self.theme.status_warn
        } else {
            self.theme.status_ok
        };

        let mut content = div()
            .id("server_performance")
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .pt_2()
                    .pb_3()
                    .child(div().size(px(7.0)).rounded_full().bg(status_color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(snapshot.hostname.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(self.theme.text_muted)
                            .child(if performance.error.is_some() {
                                self.tr("performance-retrying")
                            } else {
                                self.tr("performance-live")
                            }),
                    ),
            )
            .child(self.render_performance_meter(
                self.tr("performance-cpu").into(),
                cpu_usage,
                if performance.cpu_usage.is_some() {
                    format!("{cpu_usage:.0}%")
                } else {
                    self.tr("performance-collecting")
                },
                self.theme.accent,
            ))
            .child(self.render_logical_cpu_usage(snapshot, performance))
            .child(self.render_performance_meter(
                self.tr("performance-memory").into(),
                memory_usage,
                format!(
                    "{} / {}",
                    format_remote_size(memory_used),
                    format_remote_size(snapshot.memory_total_bytes)
                ),
                if memory_usage >= 85.0 {
                    self.theme.status_warn
                } else {
                    self.theme.accent
                },
            ))
            .child(self.render_performance_meter(
                self.tr("performance-swap").into(),
                swap_usage,
                if snapshot.swap_total_bytes == 0 {
                    self.tr("performance-not-configured")
                } else {
                    format!(
                        "{} / {}",
                        format_remote_size(swap_used),
                        format_remote_size(snapshot.swap_total_bytes)
                    )
                },
                if swap_usage >= 85.0 {
                    self.theme.status_warn
                } else {
                    self.theme.accent
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_sm()
                            .child(self.tr("performance-load-average"))
                            .child(div().text_color(self.theme.text_muted).child(format!(
                                "{:.2}  {:.2}  {:.2}",
                                snapshot.load_one_milli as f32 / 1000.0,
                                snapshot.load_five_milli as f32 / 1000.0,
                                snapshot.load_fifteen_milli as f32 / 1000.0,
                            ))),
                    )
                    .child(div().text_xs().text_color(self.theme.text_faint).child({
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("count", snapshot.cpu_count);
                        self.tr_with("performance-load-periods", &args)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("performance-network")),
                    )
                    .child(
                        self.render_performance_value_row(
                            self.tr("performance-download").into(),
                            performance
                                .network_rx_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| self.tr("performance-collecting")),
                        ),
                    )
                    .child(
                        self.render_performance_value_row(
                            self.tr("performance-upload").into(),
                            performance
                                .network_tx_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| self.tr("performance-collecting")),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("performance-disk-io")),
                    )
                    .child(
                        self.render_performance_value_row(
                            self.tr("performance-read").into(),
                            performance
                                .disk_read_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| {
                                    if snapshot.disk_read_bytes.is_some() {
                                        self.tr("performance-collecting")
                                    } else {
                                        self.tr("performance-unavailable")
                                    }
                                }),
                        ),
                    )
                    .child(
                        self.render_performance_value_row(
                            self.tr("performance-write").into(),
                            performance
                                .disk_write_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| {
                                    if snapshot.disk_write_bytes.is_some() {
                                        self.tr("performance-collecting")
                                    } else {
                                        self.tr("performance-unavailable")
                                    }
                                }),
                        ),
                    ),
            );

        if let Some((disk_total, disk_available)) = disk {
            let disk_used = disk_total.saturating_sub(disk_available);
            let disk_usage = percent(disk_used, disk_total);
            content = content.child(self.render_performance_meter(
                self.tr("performance-root-disk").into(),
                disk_usage,
                format!(
                    "{} / {}",
                    format_remote_size(disk_used),
                    format_remote_size(disk_total)
                ),
                if disk_usage >= 90.0 {
                    self.theme.danger
                } else {
                    self.theme.accent
                },
            ));
        }

        content
            .child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("performance-system")),
                    )
                    .child(self.render_performance_value_row(
                        self.tr("performance-uptime").into(),
                        format_uptime(snapshot.uptime_seconds),
                    ))
                    .child(self.render_performance_value_row(
                        self.tr("performance-processes").into(),
                        {
                            let mut args = fluent_bundle::FluentArgs::new();
                            args.set("running", snapshot.processes_running);
                            args.set("total", snapshot.processes_total);
                            self.tr_with("performance-process-count", &args)
                        },
                    ))
                    .child(self.render_performance_value_row(
                        self.tr("performance-ssh-response").into(),
                        format_response_time(snapshot.ssh_response_time),
                    )),
            )
            .when_some(performance.error.as_ref(), |this, error| {
                this.child(
                    div()
                        .flex_none()
                        .pb_2()
                        .text_xs()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            })
            .into_any_element()
    }

    fn render_logical_cpu_usage(
        &self,
        snapshot: &ServerPerformanceSnapshot,
        performance: &ServerPerformanceState,
    ) -> gpui::Div {
        let logical_cpus = snapshot.logical_cpus.iter().map(|cpu| {
            let usage = performance
                .logical_cpu_usage
                .iter()
                .find_map(|(id, usage)| (*id == cpu.id).then_some(*usage));

            div()
                .flex()
                .min_w(px(0.0))
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .text_xs()
                        .child(format!("CPU {}", cpu.id))
                        .child(div().text_color(self.theme.text_muted).child(
                            usage.map_or_else(|| "...".into(), |usage| format!("{usage:.0}%")),
                        )),
                )
                .child(
                    div()
                        .h(px(3.0))
                        .w_full()
                        .overflow_hidden()
                        .rounded_full()
                        .bg(self.theme.control_bg)
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(
                                    usage.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0,
                                ))
                                .rounded_full()
                                .bg(self.theme.accent),
                        ),
                )
        });

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .py_3()
            .border_b_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(self.tr("performance-logical-cpus"))
                    .child(div().text_color(self.theme.text_muted).child({
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("count", snapshot.logical_cpus.len());
                        self.tr_with("performance-thread-count", &args)
                    })),
            )
            .child(
                self.render_performance_value_row(
                    self.tr("performance-io-wait").into(),
                    performance
                        .cpu_iowait_usage
                        .map(|usage| format!("{usage:.1}%"))
                        .unwrap_or_else(|| self.tr("performance-collecting")),
                ),
            )
            .child(
                div()
                    .grid()
                    .grid_cols(2)
                    .gap_x_3()
                    .gap_y_2()
                    .children(logical_cpus),
            )
    }

    fn render_performance_meter(
        &self,
        label: SharedString,
        value: f32,
        detail: String,
        color: gpui::Hsla,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .py_3()
            .border_b_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(label)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_color(self.theme.text_muted)
                            .child(detail),
                    ),
            )
            .child(
                div()
                    .h(px(4.0))
                    .w_full()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(self.theme.control_bg)
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(value.clamp(0.0, 100.0) / 100.0))
                            .rounded_full()
                            .bg(color),
                    ),
            )
    }

    fn render_performance_value_row(&self, label: SharedString, value: String) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_sm()
            .child(div().text_color(self.theme.text_muted).child(label))
            .child(div().min_w(px(0.0)).truncate().child(value))
    }
}

#[derive(Debug)]
struct PerformanceCounters {
    captured_at: Instant,
    cpu_total: u64,
    cpu_idle: u64,
    cpu_iowait: u64,
    logical_cpus: Vec<(u32, u64, u64)>,
    network_rx_bytes: u64,
    network_tx_bytes: u64,
    disk_read_bytes: Option<u64>,
    disk_write_bytes: Option<u64>,
}

#[derive(Default)]
pub(super) struct ServerPerformanceState {
    pub(super) snapshot: Option<ServerPerformanceSnapshot>,
    previous: Option<PerformanceCounters>,
    pub(super) cpu_usage: Option<f32>,
    pub(super) cpu_iowait_usage: Option<f32>,
    pub(super) logical_cpu_usage: Vec<(u32, f32)>,
    pub(super) network_rx_per_second: Option<f64>,
    pub(super) network_tx_per_second: Option<f64>,
    pub(super) disk_read_per_second: Option<f64>,
    pub(super) disk_write_per_second: Option<f64>,
    pub(super) monitoring: bool,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

impl ServerPerformanceState {
    pub(super) fn update(&mut self, snapshot: ServerPerformanceSnapshot, captured_at: Instant) {
        if let Some(previous) = self.previous.as_ref() {
            let total_delta = snapshot.cpu_total.saturating_sub(previous.cpu_total);
            let idle_delta = snapshot.cpu_idle.saturating_sub(previous.cpu_idle);
            if total_delta > 0 && idle_delta <= total_delta {
                self.cpu_usage =
                    Some((total_delta - idle_delta) as f32 / total_delta as f32 * 100.0);
                let iowait_delta = snapshot.cpu_iowait.saturating_sub(previous.cpu_iowait);
                self.cpu_iowait_usage = (iowait_delta <= total_delta)
                    .then_some(iowait_delta as f32 / total_delta as f32 * 100.0);
            }

            let elapsed = captured_at
                .saturating_duration_since(previous.captured_at)
                .as_secs_f64();
            if elapsed > 0.0 {
                self.network_rx_per_second = Some(
                    snapshot
                        .network_rx_bytes
                        .saturating_sub(previous.network_rx_bytes) as f64
                        / elapsed,
                );
                self.network_tx_per_second = Some(
                    snapshot
                        .network_tx_bytes
                        .saturating_sub(previous.network_tx_bytes) as f64
                        / elapsed,
                );
                self.disk_read_per_second = snapshot
                    .disk_read_bytes
                    .zip(previous.disk_read_bytes)
                    .map(|(current, previous)| current.saturating_sub(previous) as f64 / elapsed);
                self.disk_write_per_second = snapshot
                    .disk_write_bytes
                    .zip(previous.disk_write_bytes)
                    .map(|(current, previous)| current.saturating_sub(previous) as f64 / elapsed);
            }

            self.logical_cpu_usage = snapshot
                .logical_cpus
                .iter()
                .filter_map(|cpu| {
                    let (_, previous_total, previous_idle) = previous
                        .logical_cpus
                        .iter()
                        .find(|(id, _, _)| *id == cpu.id)?;
                    let total_delta = cpu.total.saturating_sub(*previous_total);
                    let idle_delta = cpu.idle.saturating_sub(*previous_idle);
                    (total_delta > 0 && idle_delta <= total_delta).then(|| {
                        (
                            cpu.id,
                            (total_delta - idle_delta) as f32 / total_delta as f32 * 100.0,
                        )
                    })
                })
                .collect();
        }

        self.previous = Some(PerformanceCounters {
            captured_at,
            cpu_total: snapshot.cpu_total,
            cpu_idle: snapshot.cpu_idle,
            cpu_iowait: snapshot.cpu_iowait,
            logical_cpus: snapshot
                .logical_cpus
                .iter()
                .map(|cpu| (cpu.id, cpu.total, cpu.idle))
                .collect(),
            network_rx_bytes: snapshot.network_rx_bytes,
            network_tx_bytes: snapshot.network_tx_bytes,
            disk_read_bytes: snapshot.disk_read_bytes,
            disk_write_bytes: snapshot.disk_write_bytes,
        });
        self.snapshot = Some(snapshot);
        self.loading = false;
        self.error = None;
    }

    pub(super) fn clear_connection(&mut self) {
        self.snapshot = None;
        self.previous = None;
        self.cpu_usage = None;
        self.cpu_iowait_usage = None;
        self.logical_cpu_usage.clear();
        self.network_rx_per_second = None;
        self.network_tx_per_second = None;
        self.disk_read_per_second = None;
        self.disk_write_per_second = None;
        self.monitoring = false;
        self.loading = false;
        self.error = None;
    }
}

fn format_byte_rate(bytes_per_second: f64) -> String {
    let bytes = bytes_per_second.max(0.0) as u64;
    format!("{}/s", format_remote_size(bytes))
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = seconds % 86_400 / 3_600;
    let minutes = seconds % 3_600 / 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn format_response_time(duration: Duration) -> String {
    let milliseconds = duration.as_secs_f64() * 1_000.0;
    if milliseconds < 1.0 {
        "<1 ms".into()
    } else if milliseconds < 1_000.0 {
        format!("{milliseconds:.0} ms")
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used.min(total) as f32 / total as f32 * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remcmd_ssh::LogicalCpuSnapshot;

    #[test]
    fn performance_state_calculates_counter_deltas() {
        let started = Instant::now();
        let mut performance = ServerPerformanceState::default();
        performance.update(performance_snapshot(1_000, 700, 1_000, 2_000), started);
        performance.update(
            performance_snapshot(1_200, 750, 5_000, 8_000),
            started + Duration::from_secs(2),
        );

        assert_eq!(performance.cpu_usage, Some(75.0));
        assert_eq!(performance.cpu_iowait_usage, Some(5.0));
        assert_eq!(performance.logical_cpu_usage, vec![(0, 75.0), (1, 75.0)]);
        assert_eq!(performance.network_rx_per_second, Some(2_000.0));
        assert_eq!(performance.network_tx_per_second, Some(3_000.0));
        assert_eq!(performance.disk_read_per_second, Some(4_000.0));
        assert_eq!(performance.disk_write_per_second, Some(6_000.0));
        assert!(!performance.loading);
        assert!(performance.error.is_none());
    }

    #[test]
    fn performance_formatting_uses_compact_units() {
        assert_eq!(format_byte_rate(1536.0), "1.5 KB/s");
        assert_eq!(format_uptime(61), "1m");
        assert_eq!(format_uptime(90_061), "1d 1h 1m");
        assert_eq!(format_response_time(Duration::from_micros(900)), "<1 ms");
        assert_eq!(format_response_time(Duration::from_millis(42)), "42 ms");
        assert_eq!(format_response_time(Duration::from_millis(1_250)), "1.25 s");
        assert_eq!(percent(3, 4), 75.0);
        assert_eq!(percent(1, 0), 0.0);
    }

    fn performance_snapshot(
        cpu_total: u64,
        cpu_idle: u64,
        network_rx_bytes: u64,
        network_tx_bytes: u64,
    ) -> ServerPerformanceSnapshot {
        ServerPerformanceSnapshot {
            hostname: "demo".into(),
            cpu_total,
            cpu_idle,
            cpu_iowait: cpu_total / 20,
            cpu_count: 4,
            logical_cpus: vec![
                LogicalCpuSnapshot {
                    id: 0,
                    total: cpu_total / 2,
                    idle: cpu_idle / 2,
                },
                LogicalCpuSnapshot {
                    id: 1,
                    total: cpu_total / 2,
                    idle: cpu_idle / 2,
                },
            ],
            memory_total_bytes: 8 * 1024 * 1024 * 1024,
            memory_available_bytes: 4 * 1024 * 1024 * 1024,
            swap_total_bytes: 2 * 1024 * 1024 * 1024,
            swap_free_bytes: 1024 * 1024 * 1024,
            load_one_milli: 100,
            load_five_milli: 200,
            load_fifteen_milli: 300,
            processes_running: 2,
            processes_total: 100,
            network_rx_bytes,
            network_tx_bytes,
            disk_read_bytes: Some(network_rx_bytes * 2),
            disk_write_bytes: Some(network_tx_bytes * 2),
            disk_total_bytes: Some(100 * 1024 * 1024 * 1024),
            disk_available_bytes: Some(50 * 1024 * 1024 * 1024),
            uptime_seconds: 3_600,
            ssh_response_time: Duration::from_millis(42),
        }
    }
}
