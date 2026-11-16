use super::{
    AnyElement, FontWeight, IconName, IntoElement, RemCmdApp, RightSidebarView,
    ServerPerformanceSnapshot, SessionId, SessionState, SharedString, div, format_remote_size, px,
};
use gpui::{Bounds, PathBuilder, canvas, fill, point, prelude::*, size};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

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
        let collecting = || self.tr("performance-collecting");

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_1()
            .pt_1()
            .pb_1()
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
            );

        let cpu_card = self
            .render_performance_card(
                self.tr("performance-cpu").into(),
                performance
                    .cpu_usage
                    .map_or_else(collecting, |usage| format!("{usage:.0}%")),
            )
            .child(sparkline(
                vec![SparklineSeries {
                    values: performance.cpu_history.iter().copied().collect(),
                    color: self.theme.accent,
                    filled: true,
                }],
                100.0,
                SPARKLINE_HEIGHT,
                self.theme.control_bg,
            ))
            .when(!snapshot.logical_cpus.is_empty(), |card| {
                card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_xs()
                        .text_color(self.theme.text_muted)
                        .child(self.tr("performance-logical-cpus"))
                        .child({
                            let mut args = fluent_bundle::FluentArgs::new();
                            args.set("count", snapshot.logical_cpus.len());
                            self.tr_with("performance-thread-count", &args)
                        }),
                )
                .child(core_bars(
                    snapshot
                        .logical_cpus
                        .iter()
                        .map(|cpu| performance.logical_usage(cpu.id))
                        .collect(),
                    self.theme.control_bg,
                    self.theme.accent,
                    self.theme.status_warn,
                ))
            })
            .child(
                self.render_performance_value_row(
                    self.tr("performance-io-wait").into(),
                    performance
                        .cpu_iowait_usage
                        .map_or_else(collecting, |usage| format!("{usage:.1}%")),
                ),
            )
            .child(self.render_performance_value_row(
                self.tr("performance-load-average").into(),
                format!(
                    "{:.2}  {:.2}  {:.2}",
                    snapshot.load_one_milli as f32 / 1000.0,
                    snapshot.load_five_milli as f32 / 1000.0,
                    snapshot.load_fifteen_milli as f32 / 1000.0,
                ),
            ))
            .child(div().text_xs().text_color(self.theme.text_faint).child({
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", snapshot.cpu_count);
                self.tr_with("performance-load-periods", &args)
            }));

        let memory_color = usage_color(
            memory_usage,
            85.0,
            self.theme.accent,
            self.theme.status_warn,
        );
        let memory_card = self
            .render_performance_card(
                self.tr("performance-memory").into(),
                format!("{memory_usage:.0}%"),
            )
            .child(sparkline(
                vec![SparklineSeries {
                    values: performance.memory_history.iter().copied().collect(),
                    color: memory_color,
                    filled: true,
                }],
                100.0,
                SPARKLINE_HEIGHT,
                self.theme.control_bg,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.text_muted)
                    .child(format!(
                        "{} / {}",
                        format_remote_size(memory_used),
                        format_remote_size(snapshot.memory_total_bytes)
                    )),
            )
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
                usage_color(swap_usage, 85.0, self.theme.accent, self.theme.status_warn),
            ));

        let network_peak = performance
            .network_rx_history
            .iter()
            .chain(&performance.network_tx_history)
            .fold(MIN_NETWORK_SCALE, |peak, value| peak.max(*value));
        let network_card = self
            .render_performance_card(self.tr("performance-network").into(), String::new())
            .child(sparkline(
                vec![
                    SparklineSeries {
                        values: performance.network_rx_history.iter().copied().collect(),
                        color: self.theme.accent,
                        filled: true,
                    },
                    SparklineSeries {
                        values: performance.network_tx_history.iter().copied().collect(),
                        color: self.theme.status_ok,
                        filled: false,
                    },
                ],
                network_peak,
                SPARKLINE_HEIGHT,
                self.theme.control_bg,
            ))
            .child(
                self.render_performance_legend_row(
                    self.tr("performance-download").into(),
                    performance
                        .network_rx_per_second
                        .map_or_else(collecting, format_byte_rate),
                    self.theme.accent,
                ),
            )
            .child(
                self.render_performance_legend_row(
                    self.tr("performance-upload").into(),
                    performance
                        .network_tx_per_second
                        .map_or_else(collecting, format_byte_rate),
                    self.theme.status_ok,
                ),
            );

        let disk_rate = |rate: Option<f64>, counter: Option<u64>| {
            rate.map_or_else(
                || {
                    if counter.is_some() {
                        self.tr("performance-collecting")
                    } else {
                        self.tr("performance-unavailable")
                    }
                },
                format_byte_rate,
            )
        };
        let disk_card = self
            .render_performance_card(self.tr("performance-disk-io").into(), String::new())
            .child(self.render_performance_value_row(
                self.tr("performance-read").into(),
                disk_rate(performance.disk_read_per_second, snapshot.disk_read_bytes),
            ))
            .child(self.render_performance_value_row(
                self.tr("performance-write").into(),
                disk_rate(performance.disk_write_per_second, snapshot.disk_write_bytes),
            ))
            .when_some(disk, |card, (disk_total, disk_available)| {
                let disk_used = disk_total.saturating_sub(disk_available);
                let disk_usage = percent(disk_used, disk_total);
                card.child(self.render_performance_meter(
                    self.tr("performance-root-disk").into(),
                    disk_usage,
                    format!(
                        "{} / {}",
                        format_remote_size(disk_used),
                        format_remote_size(disk_total)
                    ),
                    usage_color(disk_usage, 90.0, self.theme.accent, self.theme.danger),
                ))
            });

        let system_card = self
            .render_performance_card(self.tr("performance-system").into(), String::new())
            .child(self.render_performance_value_row(
                self.tr("performance-uptime").into(),
                format_uptime(snapshot.uptime_seconds),
            ))
            .child(
                self.render_performance_value_row(self.tr("performance-processes").into(), {
                    let mut args = fluent_bundle::FluentArgs::new();
                    args.set("running", snapshot.processes_running);
                    args.set("total", snapshot.processes_total);
                    self.tr_with("performance-process-count", &args)
                }),
            )
            .child(self.render_performance_value_row(
                self.tr("performance-ssh-response").into(),
                format_response_time(snapshot.ssh_response_time),
            ));

        div()
            .id("server_performance")
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .gap_2()
            .pb_2()
            .overflow_y_scroll()
            .child(header)
            .child(cpu_card)
            .child(memory_card)
            .child(network_card)
            .child(disk_card)
            .child(system_card)
            .when_some(performance.error.as_ref(), |this, error| {
                this.child(
                    div()
                        .flex_none()
                        .px_1()
                        .text_xs()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            })
            .into_any_element()
    }

    fn render_performance_card(&self, title: SharedString, value: String) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(self.theme.text_muted)
                            .child(value),
                    ),
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
            .flex_col()
            .gap_1()
            .child(self.render_performance_value_row(label, detail))
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

    fn render_performance_legend_row(
        &self,
        label: SharedString,
        value: String,
        color: gpui::Hsla,
    ) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .child(div().flex_none().size(px(6.0)).rounded_full().bg(color))
            .child(
                div()
                    .flex_1()
                    .text_color(self.theme.text_muted)
                    .child(label),
            )
            .child(div().min_w(px(0.0)).truncate().child(value))
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

    pub(super) fn performance_is_rendered(&self, session_id: SessionId) -> bool {
        self.right_sidebar_open
            && self.right_sidebar_view == RightSidebarView::Performance
            && self.active_session_id == Some(session_id)
    }
}

const HISTORY_SAMPLES: usize = 60;
const SPARKLINE_HEIGHT: f32 = 36.0;
const CORE_COLUMNS: usize = 16;
const CORE_ROW_HEIGHT: f32 = 14.0;
const CORE_GAP: f32 = 2.0;
/// Keeps idle links from magnifying background chatter into a full-height chart.
const MIN_NETWORK_SCALE: f32 = 16.0 * 1024.0;

struct SparklineSeries {
    values: Vec<f32>,
    color: gpui::Hsla,
    filled: bool,
}

/// History chart painted in one canvas; the newest sample sits on the right edge.
fn sparkline(
    series: Vec<SparklineSeries>,
    max: f32,
    height: f32,
    background: gpui::Hsla,
) -> gpui::Div {
    div()
        .h(px(height))
        .w_full()
        .overflow_hidden()
        .rounded_md()
        .bg(background)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| {
                    let step = bounds.size.width / (HISTORY_SAMPLES - 1) as f32;
                    let inset = px(2.0);
                    let plot_height = bounds.size.height - inset;
                    for series in series {
                        if series.values.len() < 2 {
                            continue;
                        }
                        let start = bounds.right() - step * (series.values.len() - 1) as f32;
                        let points = series
                            .values
                            .iter()
                            .enumerate()
                            .map(|(index, value)| {
                                point(
                                    start + step * index as f32,
                                    bounds.bottom() - plot_height * (value / max).clamp(0.0, 1.0),
                                )
                            })
                            .collect::<Vec<_>>();
                        if series.filled {
                            let mut area = PathBuilder::fill();
                            area.move_to(point(start, bounds.bottom()));
                            for point in &points {
                                area.line_to(*point);
                            }
                            area.line_to(point(bounds.right(), bounds.bottom()));
                            area.close();
                            if let Ok(path) = area.build() {
                                window.paint_path(path, series.color.opacity(0.18));
                            }
                        }
                        let mut line = PathBuilder::stroke(px(1.25));
                        line.move_to(points[0]);
                        for point in &points[1..] {
                            line.line_to(*point);
                        }
                        if let Ok(path) = line.build() {
                            window.paint_path(path, series.color);
                        }
                    }
                },
            )
            .size_full(),
        )
}

/// Per-core usage as compact vertical gauges, scaling to many-core hosts
/// without laying out a text element per core.
fn core_bars(
    usages: Vec<Option<f32>>,
    track: gpui::Hsla,
    color: gpui::Hsla,
    busy_color: gpui::Hsla,
) -> impl IntoElement {
    let columns = usages.len().clamp(1, CORE_COLUMNS);
    let rows = usages.len().div_ceil(columns);
    let height = rows as f32 * (CORE_ROW_HEIGHT + CORE_GAP) - CORE_GAP;
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            let gap = px(CORE_GAP);
            let row_height = px(CORE_ROW_HEIGHT);
            let width = (bounds.size.width - gap * (columns - 1) as f32) / columns as f32;
            for (index, usage) in usages.iter().enumerate() {
                let origin = point(
                    bounds.left() + (width + gap) * (index % columns) as f32,
                    bounds.top() + (row_height + gap) * (index / columns) as f32,
                );
                window.paint_quad(
                    fill(Bounds::new(origin, size(width, row_height)), track).corner_radii(px(2.0)),
                );
                let Some(usage) = usage else {
                    continue;
                };
                let filled = row_height * (usage / 100.0).clamp(0.0, 1.0);
                window.paint_quad(
                    fill(
                        Bounds::new(
                            point(origin.x, origin.y + row_height - filled),
                            size(width, filled),
                        ),
                        usage_color(*usage, 85.0, color, busy_color),
                    )
                    .corner_radii(px(2.0)),
                );
            }
        },
    )
    .w_full()
    .h(px(height))
}

fn usage_color(usage: f32, threshold: f32, normal: gpui::Hsla, high: gpui::Hsla) -> gpui::Hsla {
    if usage >= threshold { high } else { normal }
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
    pub(super) cpu_history: VecDeque<f32>,
    pub(super) memory_history: VecDeque<f32>,
    pub(super) network_rx_history: VecDeque<f32>,
    pub(super) network_tx_history: VecDeque<f32>,
    pub(super) monitoring: bool,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

impl ServerPerformanceState {
    pub(super) fn update(&mut self, snapshot: ServerPerformanceSnapshot, captured_at: Instant) {
        push_history(
            &mut self.memory_history,
            percent(
                snapshot
                    .memory_total_bytes
                    .saturating_sub(snapshot.memory_available_bytes),
                snapshot.memory_total_bytes,
            ),
        );
        if let Some(previous) = self.previous.as_ref() {
            let total_delta = snapshot.cpu_total.saturating_sub(previous.cpu_total);
            let idle_delta = snapshot.cpu_idle.saturating_sub(previous.cpu_idle);
            if total_delta > 0 && idle_delta <= total_delta {
                let usage = (total_delta - idle_delta) as f32 / total_delta as f32 * 100.0;
                self.cpu_usage = Some(usage);
                push_history(&mut self.cpu_history, usage);
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
                push_history(
                    &mut self.network_rx_history,
                    self.network_rx_per_second.unwrap_or_default() as f32,
                );
                push_history(
                    &mut self.network_tx_history,
                    self.network_tx_per_second.unwrap_or_default() as f32,
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

    /// `logical_cpu_usage` follows the snapshot's id order.
    fn logical_usage(&self, id: u32) -> Option<f32> {
        self.logical_cpu_usage
            .binary_search_by_key(&id, |(cpu, _)| *cpu)
            .ok()
            .map(|index| self.logical_cpu_usage[index].1)
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
        self.cpu_history.clear();
        self.memory_history.clear();
        self.network_rx_history.clear();
        self.network_tx_history.clear();
        self.monitoring = false;
        self.loading = false;
        self.error = None;
    }
}

fn push_history(history: &mut VecDeque<f32>, value: f32) {
    if history.len() == HISTORY_SAMPLES {
        history.pop_front();
    }
    history.push_back(value);
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
        assert_eq!(performance.cpu_history, [75.0]);
        assert_eq!(performance.memory_history, [50.0, 50.0]);
        assert_eq!(performance.network_rx_history, [2_000.0]);
        assert_eq!(performance.logical_usage(1), Some(75.0));
        assert_eq!(performance.logical_usage(7), None);
    }

    #[test]
    fn performance_history_keeps_a_bounded_window() {
        let mut history = VecDeque::new();
        for value in 0..HISTORY_SAMPLES + 5 {
            push_history(&mut history, value as f32);
        }

        assert_eq!(history.len(), HISTORY_SAMPLES);
        assert_eq!(history.front(), Some(&5.0));
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
