use super::{
    ActivePanel, Context, FontWeight, IconName, IconTone, RemCmdApp, SshRuntime, Task,
    TextButtonTone, Timer, Window, div, icon, px, text_button,
};
use gpui::prelude::*;
use remcmd_core::UpdateSettings;
use remcmd_update::{DownloadProgress, PackageKind, Release, UpdateError};
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Status {
    #[default]
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Ready,
    Verifying,
    Cancelled,
    Failed,
}
impl Status {
    fn busy(self) -> bool {
        matches!(self, Self::Checking | Self::Downloading | Self::Verifying)
    }
}

pub(super) struct UpdateState {
    pub(super) settings: UpdateSettings,
    status: Status,
    release: Option<Release>,
    downloaded: Option<PathBuf>,
    progress: DownloadProgress,
    error: Option<String>,
    task: Option<Task<()>>,
    auto_task: Option<Task<()>>,
    cancel: Option<watch::Sender<bool>>,
}

impl UpdateState {
    pub(super) fn has_release(&self) -> bool {
        self.release.is_some()
    }

    pub(super) fn new(settings: UpdateSettings) -> Self {
        Self {
            settings,
            status: Status::Idle,
            release: None,
            downloaded: None,
            progress: DownloadProgress::default(),
            error: None,
            task: None,
            auto_task: None,
            cancel: None,
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn package_kind() -> Option<PackageKind> {
    PackageKind::for_platform(
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::var_os("APPIMAGE").is_some(),
        std::path::Path::new("/etc/debian_version").exists(),
    )
}

impl RemCmdApp {
    fn set_automatic_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.updates.settings.automatic == enabled {
            return;
        }
        self.updates.settings.automatic = enabled;
        if enabled {
            self.schedule_update_check(cx);
        } else {
            self.updates.auto_task = None;
        }
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn schedule_update_check(&mut self, cx: &mut Context<Self>) {
        if !self.updates.settings.automatic || self.updates.auto_task.is_some() {
            return;
        }
        self.updates.auto_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(3)).await;
            loop {
                if this
                    .update(cx, |this, cx| this.check_for_updates(false, cx))
                    .is_err()
                {
                    return;
                }
                Timer::after(Duration::from_secs(60 * 60)).await;
            }
        }));
    }

    pub(super) fn show_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_settings(window, cx);
        self.active_panel = ActivePanel::Updates;
        window.activate_window();
        cx.notify();
    }

    pub(super) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.updates.status.busy()
            || (!manual && !self.updates.settings.automatic_check_due(now()))
        {
            return;
        }
        self.updates.status = Status::Checking;
        self.updates.error = None;
        self.updates.settings.last_check_unix = now();
        self.persist_settings();
        let worker = cx
            .global::<SshRuntime>()
            .handle()
            .spawn(remcmd_update::check(
                env!("CARGO_PKG_VERSION"),
                package_kind(),
            ));
        self.updates.task = Some(cx.spawn(async move |this, cx| {
            let result = worker.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.task = None;
                match result {
                    Ok(Ok(release)) => {
                        this.updates.status = if release.is_some() {
                            Status::Available
                        } else {
                            Status::Current
                        };
                        this.updates.release = release;
                        this.updates.downloaded = None;
                    }
                    Ok(Err(error)) => this.update_error(error),
                    Err(_) => {
                        this.updates.status = Status::Failed;
                        this.updates.error = Some(this.tr("updates-worker-error"));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn update_error(&mut self, error: UpdateError) {
        self.updates.status = if matches!(error, UpdateError::Cancelled) {
            Status::Cancelled
        } else {
            Status::Failed
        };
        let key = match error {
            UpdateError::Integrity => "updates-integrity-error",
            UpdateError::MissingDigest => "updates-digest-error",
            UpdateError::UnsafeUrl | UpdateError::InvalidRelease | UpdateError::TooLarge => {
                "updates-metadata-error"
            }
            UpdateError::Http(403 | 429) => "updates-rate-limit",
            UpdateError::Cancelled => "updates-cancelled",
            UpdateError::Io(_) => "updates-file-error",
            _ => "updates-network-error",
        };
        self.updates.error = Some(self.tr(key));
    }

    fn download_update(&mut self, cx: &mut Context<Self>) {
        if self.updates.status.busy() {
            return;
        }
        let Some(package) = self
            .updates
            .release
            .as_ref()
            .and_then(|release| release.package.clone())
        else {
            return;
        };
        let Some(parent) = self.settings_path.parent() else {
            return;
        };
        let directory = parent.join("updates");
        let (cancel, receiver) = watch::channel(false);
        let (progress_tx, mut progress_rx) = watch::channel(DownloadProgress {
            received: 0,
            total: package.size,
        });
        self.updates.status = Status::Downloading;
        self.updates.error = None;
        self.updates.downloaded = None;
        self.updates.progress = *progress_rx.borrow();
        self.updates.cancel = Some(cancel);
        let worker = cx.global::<SshRuntime>().handle().spawn(async move {
            remcmd_update::download(&package, &directory, receiver, progress_tx).await
        });
        self.updates.task = Some(cx.spawn(async move |this, cx| {
            while progress_rx.changed().await.is_ok() {
                let progress = *progress_rx.borrow_and_update();
                if this
                    .update(cx, |this, cx| {
                        this.updates.progress = progress;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                // Coalesce fast downloads without building an unbounded UI event queue.
                Timer::after(Duration::from_millis(100)).await;
            }
            let result = worker.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.task = None;
                this.updates.cancel = None;
                match result {
                    Ok(Ok(path)) => {
                        this.updates.downloaded = Some(path);
                        this.updates.status = Status::Ready;
                    }
                    Ok(Err(error)) => this.update_error(error),
                    Err(_) => {
                        this.updates.status = Status::Failed;
                        this.updates.error = Some(this.tr("updates-worker-error"));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn open_update(&mut self, cx: &mut Context<Self>) {
        if self.updates.status.busy() {
            return;
        }
        let Some((package, path)) = self
            .updates
            .release
            .as_ref()
            .and_then(|release| release.package.clone())
            .zip(self.updates.downloaded.clone())
        else {
            return;
        };
        self.updates.status = Status::Verifying;
        self.updates.error = None;
        let checked_path = path.clone();
        let worker = cx
            .global::<SshRuntime>()
            .handle()
            .spawn(async move { remcmd_update::verify(&package, &checked_path).await });
        self.updates.task = Some(cx.spawn(async move |this, cx| {
            let result = worker.await;
            let _ = this.update(cx, |this, cx| {
                this.updates.task = None;
                match result {
                    Ok(Ok(())) => {
                        this.updates.status = Status::Ready;
                        if path
                            .extension()
                            .is_some_and(|extension| extension == "AppImage")
                        {
                            cx.reveal_path(&path);
                        } else {
                            cx.open_with_system(&path);
                        }
                    }
                    Ok(Err(error)) => {
                        this.updates.downloaded = None;
                        this.update_error(error);
                    }
                    Err(_) => {
                        this.updates.status = Status::Failed;
                        this.updates.error = Some(this.tr("updates-worker-error"));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_update_entry(&self, cx: &mut Context<Self>) -> gpui::Div {
        div().mt_6().child(
            div()
                .id("open-updates")
                .relative()
                .child(crate::accessibility::node(
                    "open-updates",
                    crate::accessibility::Node::button(self.tr("updates-title"), true),
                ))
                .tab_index(0)
                .flex()
                .items_center()
                .justify_between()
                .min_h(px(38.0))
                .px(px(10.0))
                .rounded_lg()
                .bg(self.theme.settings_group_bg)
                .text_sm()
                .cursor_pointer()
                .hover(|style| style.bg(self.theme.control_hover_bg))
                .focus(|style| style.bg(self.theme.list_selected_bg))
                .child(self.tr(if self.updates.release.is_some() {
                    "updates-available-entry"
                } else {
                    "updates-title"
                }))
                .child(icon(IconName::Expand, self.theme, IconTone::Default, 15.0))
                .on_click(cx.listener(|this, _, window, cx| this.show_updates(window, cx))),
        )
    }

    pub(super) fn render_updates_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut args = fluent_bundle::FluentArgs::new();
        args.set("version", env!("CARGO_PKG_VERSION"));
        let current = self.tr_with("updates-current-version", &args);
        let status = match self.updates.status {
            Status::Idle => self.tr("updates-idle"),
            Status::Checking => self.tr("updates-checking"),
            Status::Current => self.tr("updates-current"),
            Status::Available => {
                args.set(
                    "version",
                    self.updates
                        .release
                        .as_ref()
                        .map(|release| release.version.as_str())
                        .unwrap_or_default(),
                );
                self.tr_with("updates-available", &args)
            }
            Status::Downloading => {
                args.set(
                    "received",
                    super::format_remote_size(self.updates.progress.received),
                );
                args.set(
                    "total",
                    super::format_remote_size(self.updates.progress.total),
                );
                self.tr_with("updates-progress", &args)
            }
            Status::Ready => self.tr("updates-ready"),
            Status::Verifying => self.tr("updates-verifying"),
            Status::Cancelled => self.tr("updates-cancelled"),
            Status::Failed => self.tr("updates-failed"),
        };
        let app = cx.entity().downgrade();
        let automatic_action =
            std::rc::Rc::new(move |action, _: &mut Window, cx: &mut gpui::App| {
                let _ = app.update(cx, |app, cx| {
                    let enabled = match action {
                        crate::accessibility::Action::SetChecked(enabled) => enabled,
                        crate::accessibility::Action::Press => !app.updates.settings.automatic,
                        _ => return,
                    };
                    app.set_automatic_updates(enabled, cx);
                });
            });
        let busy = self.updates.status.busy();
        let mut actions = div().flex().flex_wrap().gap_2().mt_4();
        let check = text_button(
            "check-updates",
            self.tr("updates-check"),
            TextButtonTone::Secondary,
            !busy,
            &self.theme,
        )
        .when(!busy, |button| {
            button.on_click(cx.listener(|this, _, _, cx| this.check_for_updates(true, cx)))
        });
        actions = actions.child(check);
        if self
            .updates
            .release
            .as_ref()
            .is_some_and(|release| release.package.is_some())
            && !busy
        {
            let ready = self.updates.downloaded.is_some();
            actions = actions.child(
                text_button(
                    "download-update",
                    self.tr(if ready {
                        "updates-open"
                    } else {
                        "updates-download"
                    }),
                    TextButtonTone::Primary,
                    true,
                    &self.theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if ready {
                        this.open_update(cx);
                    } else {
                        this.download_update(cx);
                    }
                })),
            );
        }
        if self.updates.status == Status::Downloading {
            actions = actions.child(
                text_button(
                    "cancel-update",
                    self.tr("common-cancel"),
                    TextButtonTone::Secondary,
                    true,
                    &self.theme,
                )
                .on_click(cx.listener(|this, _, _, _| {
                    if let Some(cancel) = &this.updates.cancel {
                        cancel.send_replace(true);
                    }
                })),
            );
        }
        let url = self
            .updates
            .release
            .as_ref()
            .map(|release| release.url.clone())
            .unwrap_or_else(|| remcmd_update::RELEASES_URL.into());
        actions = actions.child(
            text_button(
                "update-release-page",
                self.tr("updates-release-page"),
                TextButtonTone::Secondary,
                true,
                &self.theme,
            )
            .on_click(move |_, _, cx| cx.open_url(&url)),
        );
        let accessibility_status = match self.updates.error.as_ref() {
            Some(error) => format!("{current}\n{status}\n{error}"),
            None => format!("{current}\n{status}"),
        };
        let mut content = div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(640.0))
            .py_6()
            .child(
                div()
                    .text_size(px(22.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("updates-title")),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(current),
            )
            .child(
                div()
                    .mt_6()
                    .p_4()
                    .rounded_lg()
                    .bg(self.theme.settings_group_bg)
                    .relative()
                    .child(crate::accessibility::node(
                        "update-status",
                        crate::accessibility::Node::text(
                            self.tr("updates-title"),
                            accessibility_status,
                        ),
                    ))
                    .child(div().text_sm().child(status))
                    .when_some(self.updates.error.as_ref(), |this, error| {
                        this.child(
                            div()
                                .mt_2()
                                .text_sm()
                                .text_color(self.theme.error_text)
                                .child(error.clone()),
                        )
                    })
                    .when(self.updates.status == Status::Downloading, |this| {
                        this.child(
                            div()
                                .mt_3()
                                .h(px(4.0))
                                .w_full()
                                .rounded_full()
                                .overflow_hidden()
                                .bg(self.theme.control_bg)
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(if self.updates.progress.total > 0 {
                                            (self.updates.progress.received as f32
                                                / self.updates.progress.total as f32)
                                                .min(1.0)
                                        } else {
                                            0.0
                                        }))
                                        .bg(self.theme.accent),
                                ),
                        )
                    })
                    .child(actions),
            )
            .child(
                div()
                    .id("automatic-updates")
                    .relative()
                    .child(crate::accessibility::node(
                        "automatic-updates",
                        crate::accessibility::Node {
                            role: crate::accessibility::Role::CheckBox,
                            selected: self.updates.settings.automatic,
                            handler: Some(automatic_action),
                            ..crate::accessibility::Node::button(self.tr("updates-automatic"), true)
                        },
                    ))
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_4()
                    .p_2()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .focus(|style| style.bg(self.theme.list_selected_bg))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(16.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(self.theme.border_strong)
                            .bg(if self.updates.settings.automatic {
                                self.theme.accent
                            } else {
                                self.theme.transparent
                            })
                            .when(self.updates.settings.automatic, |this| {
                                this.child(super::icon_with_color(
                                    IconName::Check,
                                    self.theme.on_accent,
                                    12.0,
                                ))
                            }),
                    )
                    .child(self.tr("updates-automatic"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_automatic_updates(!this.updates.settings.automatic, cx);
                    })),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("updates-install-help")),
            );
        if self
            .updates
            .release
            .as_ref()
            .is_some_and(|release| release.package.is_none())
        {
            content = content.child(
                div()
                    .mt_3()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("updates-platform-unavailable")),
            );
        }
        if package_kind() == Some(PackageKind::AppImage) {
            content = content.child(
                div()
                    .mt_3()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("updates-appimage-help")),
            );
        }
        if let Some(release) = &self.updates.release
            && !release.notes.is_empty()
        {
            content = content
                .child(
                    div()
                        .mt_6()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.tr("updates-notes")),
                )
                .child(
                    div()
                        .mt_2()
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(release.notes.clone()),
                );
        }
        self.detail_panel_shell().child(
            div()
                .id("updates-content")
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .justify_center()
                .child(content),
        )
    }
}
