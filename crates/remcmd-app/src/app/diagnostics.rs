use super::{
    ActivePanel, ConnectionEvent, Context, DiagnosticFilter, DiagnosticLevel, DiagnosticsGlobal,
    FontWeight, IconName, IconTone, PromptButton, PromptLevel, RemCmdApp, SharedString, SshRuntime,
    SupportBundleContext, TextButtonTone, UI_MONOSPACE_FONT_FAMILY, Window, connection_stage_label,
    div, px, text_button,
};
use gpui::prelude::*;
use std::path::Path;

impl RemCmdApp {
    pub(super) fn show_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::Diagnostics;
        self.open_settings_selector = None;
        self.settings_focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn set_diagnostic_level(
        &mut self,
        level: Option<DiagnosticLevel>,
        cx: &mut Context<Self>,
    ) {
        self.diagnostic_level = level;
        cx.notify();
    }

    pub(super) fn toggle_detailed_diagnostics(&mut self, cx: &mut Context<Self>) {
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        store.set_debug_enabled(!store.debug_enabled());
        store.record(
            DiagnosticLevel::Info,
            "diagnostics.settings",
            if store.debug_enabled() {
                "Detailed diagnostics enabled for this run"
            } else {
                "Detailed diagnostics disabled"
            },
            [],
        );
        cx.notify();
    }

    pub(super) fn open_diagnostic_log_directory(&self, cx: &mut Context<Self>) {
        let path = cx.global::<DiagnosticsGlobal>().0.log_directory();
        cx.open_with_system(path);
    }

    pub(super) fn clear_diagnostic_logs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let answer = window.prompt(
            PromptLevel::Critical,
            &self.tr("diagnostics-clear-confirm"),
            None,
            &[
                PromptButton::new(self.tr("common-clear")),
                PromptButton::cancel(self.tr("common-cancel")),
            ],
            cx,
        );
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let result = store.clear();
            let _ = this.update_in(cx, |this, _, cx| {
                this.settings_error = result.err().map(|error| error.to_string());
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn export_support_bundle(&mut self, cx: &mut Context<Self>) {
        let directory = self
            .profiles_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let destination = cx.prompt_for_new_path(directory, Some("remcmd-support.zip"));
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        let runtime = cx.global::<SshRuntime>().handle();
        let profiles = self.profiles.clone();
        let context = SupportBundleContext {
            app_version: env!("CARGO_PKG_VERSION").into(),
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            language: self.language_mode,
            theme: self.theme_mode,
            tab_layout: self.tab_layout,
            terminal_font_size: self.terminal_font_size,
            transfer_rate_limit_mib_per_second: self.transfer_settings.rate_limit_mib_per_second,
            max_parallel_transfers: self.transfer_settings.max_parallel_transfers,
        };
        cx.spawn(async move |this, cx| match destination.await {
            Ok(Ok(Some(path))) => {
                let result = runtime
                    .spawn_blocking(move || store.export_support_bundle(&path, context, &profiles))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.settings_error = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(error.to_string()),
                    };
                    cx.notify();
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.settings_error = Some(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn record_connection_diagnostic(&self, event: &ConnectionEvent, cx: &Context<Self>) {
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        match event {
            ConnectionEvent::StateChanged(state) => store.record(
                DiagnosticLevel::Info,
                "ssh.lifecycle",
                "SSH state changed",
                [("state".into(), format!("{state:?}"))],
            ),
            ConnectionEvent::ConnectionStageChanged(stage) => store.record(
                DiagnosticLevel::Info,
                "ssh.route",
                "SSH connection stage started",
                [(
                    "stage".into(),
                    connection_stage_label(stage, &self.localizer),
                )],
            ),
            ConnectionEvent::AuthenticationSucceeded { stage, method } => store.record(
                DiagnosticLevel::Info,
                "ssh.authentication",
                "SSH authentication succeeded",
                [
                    (
                        "stage".into(),
                        connection_stage_label(stage, &self.localizer),
                    ),
                    ("method".into(), format!("{method:?}")),
                ],
            ),
            ConnectionEvent::HostKeyVerificationRequired { stage, .. } => store.record(
                DiagnosticLevel::Info,
                "ssh.host_key",
                "Host-key verification requires user confirmation",
                [(
                    "stage".into(),
                    connection_stage_label(stage, &self.localizer),
                )],
            ),
            ConnectionEvent::DirectoryRead { .. } => store.record(
                DiagnosticLevel::Debug,
                "sftp.operation",
                "Remote directory read completed",
                [],
            ),
            ConnectionEvent::DirectoryTreeRead { .. } => store.record(
                DiagnosticLevel::Debug,
                "sftp.operation",
                "Remote directory tree read completed",
                [],
            ),
            ConnectionEvent::FileRead { .. } => store.record(
                DiagnosticLevel::Debug,
                "sftp.operation",
                "Remote file read completed",
                [],
            ),
            ConnectionEvent::FileWritten { .. } => store.record(
                DiagnosticLevel::Info,
                "sftp.operation",
                "Remote file write completed",
                [],
            ),
            ConnectionEvent::PathCreated { kind, .. } => store.record(
                DiagnosticLevel::Info,
                "sftp.operation",
                "Remote path created",
                [("kind".into(), format!("{kind:?}"))],
            ),
            ConnectionEvent::DirectoriesCreated { paths, .. } => store.record(
                DiagnosticLevel::Info,
                "sftp.operation",
                "Remote directories created",
                [("count".into(), paths.len().to_string())],
            ),
            ConnectionEvent::PathsDeleted { paths, .. } => store.record(
                DiagnosticLevel::Info,
                "sftp.operation",
                "Remote paths deleted",
                [("count".into(), paths.len().to_string())],
            ),
            ConnectionEvent::TransferCompleted {
                direction, bytes, ..
            } => store.record(
                DiagnosticLevel::Info,
                "sftp.transfer",
                "SFTP transfer completed",
                [
                    ("direction".into(), format!("{direction:?}")),
                    ("bytes".into(), bytes.to_string()),
                ],
            ),
            ConnectionEvent::TransferCancelled { .. } => store.record(
                DiagnosticLevel::Info,
                "sftp.transfer",
                "SFTP transfer cancelled",
                [],
            ),
            ConnectionEvent::SftpFailed {
                operation, error, ..
            } => store.record(
                DiagnosticLevel::Warn,
                "sftp.operation",
                "SFTP operation failed",
                [
                    ("operation".into(), format!("{operation:?}")),
                    ("error_kind".into(), format!("{:?}", error.kind())),
                ],
            ),
            ConnectionEvent::PerformanceFailed(error) => store.record(
                DiagnosticLevel::Warn,
                "ssh.performance",
                "Performance sampling failed",
                [("error_kind".into(), format!("{:?}", error.kind()))],
            ),
            ConnectionEvent::Failed(error) => store.record(
                DiagnosticLevel::Error,
                "ssh.connection",
                "SSH connection failed",
                [
                    (
                        "stage".into(),
                        error
                            .stage()
                            .map(|stage| connection_stage_label(stage, &self.localizer))
                            .unwrap_or_default(),
                    ),
                    ("error".into(), error.to_string()),
                ],
            ),
            ConnectionEvent::Resized(_)
            | ConnectionEvent::Shell(_)
            | ConnectionEvent::TransferProgress { .. }
            | ConnectionEvent::TransferConflict { .. }
            | ConnectionEvent::SftpAvailabilityChanged { .. }
            | ConnectionEvent::PerformanceSnapshot(_) => {}
        }
    }

    pub(super) fn render_diagnostics(&self, cx: &mut Context<Self>) -> gpui::Div {
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        let filter = DiagnosticFilter {
            level: self.diagnostic_level,
            module: self.diagnostic_module_filter.read(cx).text(),
            text: self.diagnostic_text_filter.read(cx).text(),
        };
        let events = store.recent(&filter);
        let events_empty = events.is_empty();
        let mut event_list = div().flex().flex_col().gap_1().w_full();
        for event in events.into_iter().rev().take(500) {
            let level_color = match event.level {
                DiagnosticLevel::Error => self.theme.error_text,
                DiagnosticLevel::Warn => self.theme.status_warn,
                DiagnosticLevel::Info => self.theme.text_primary,
                DiagnosticLevel::Debug | DiagnosticLevel::Trace => self.theme.text_muted,
            };
            let fields = event
                .fields
                .into_iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("  ");
            event_list = event_list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(self.theme.settings_group_bg)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_xs()
                            .text_color(self.theme.text_muted)
                            .child(event.timestamp)
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(level_color)
                                    .child(diagnostic_level_label(event.level)),
                            )
                            .child(event.module),
                    )
                    .child(div().text_sm().child(event.message))
                    .when(!fields.is_empty(), |this| {
                        this.child(
                            div()
                                .font_family(UI_MONOSPACE_FONT_FAMILY)
                                .text_xs()
                                .text_color(self.theme.text_muted)
                                .child(fields),
                        )
                    }),
            );
        }
        if events_empty {
            event_list = event_list.child(
                div()
                    .py_8()
                    .text_center()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("diagnostics-no-events")),
            );
        }

        let mut level_filters = div().flex().items_center().gap_1();
        for level in [
            None,
            Some(DiagnosticLevel::Error),
            Some(DiagnosticLevel::Warn),
            Some(DiagnosticLevel::Info),
            Some(DiagnosticLevel::Debug),
        ] {
            let selected = self.diagnostic_level == level;
            let label = level.map_or_else(|| self.tr("common-all"), diagnostic_level_label);
            level_filters = level_filters.child(
                div()
                    .id(SharedString::from(format!("diagnostic-level-{level:?}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .bg(if selected {
                        self.theme.accent
                    } else {
                        self.theme.control_bg
                    })
                    .text_color(if selected {
                        self.theme.on_accent
                    } else {
                        self.theme.text_primary
                    })
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_diagnostic_level(level, cx);
                    })),
            );
        }

        let open = text_button(
            "diagnostics-open-folder",
            self.tr("diagnostics-open-folder"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.open_diagnostic_log_directory(cx)));
        let clear = text_button(
            "diagnostics-clear",
            self.tr("diagnostics-clear"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.clear_diagnostic_logs(window, cx);
        }));
        let export = text_button(
            "diagnostics-export",
            self.tr("diagnostics-export"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.export_support_bundle(cx)));
        let back = self
            .render_icon_button(
                "diagnostics-back-to-settings",
                IconName::ArrowLeft,
                self.tr("settings-back"),
                IconTone::Default,
                true,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.show_settings(window, cx);
            }));

        self.detail_panel_shell().child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .gap_3()
                .pt_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div().flex().items_center().gap_2().child(back).child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.tr("diagnostics-title")),
                            ),
                        )
                        .child(div().flex().gap_2().child(open).child(clear).child(export)),
                )
                .when_some(store.initialization_error(), |this, error| {
                    this.child(
                        div()
                            .rounded_md()
                            .px_3()
                            .py_2()
                            .bg(self.theme.control_bg)
                            .text_sm()
                            .text_color(self.theme.error_text)
                            .child(self.tr("diagnostics-memory-fallback"))
                            .child(format!(" {error}")),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(level_filters)
                        .child(
                            div()
                                .w(px(180.0))
                                .child(self.diagnostic_module_filter.clone()),
                        )
                        .child(div().flex_1().child(self.diagnostic_text_filter.clone()))
                        .child(
                            div()
                                .id("diagnostics-debug-toggle")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(if store.debug_enabled() {
                                    self.theme.accent
                                } else {
                                    self.theme.control_bg
                                })
                                .text_sm()
                                .cursor_pointer()
                                .child(self.tr("diagnostics-debug"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_detailed_diagnostics(cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .id("diagnostic-events")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(event_list),
                ),
        )
    }
}

pub(super) fn diagnostic_level_label(level: DiagnosticLevel) -> String {
    match level {
        DiagnosticLevel::Error => "ERROR",
        DiagnosticLevel::Warn => "WARN",
        DiagnosticLevel::Info => "INFO",
        DiagnosticLevel::Debug => "DEBUG",
        DiagnosticLevel::Trace => "TRACE",
    }
    .into()
}
