use super::{
    ActivePanel, AuthConfig, ConnectionProfile, Context, DiagnosticLevel, DiagnosticsGlobal,
    FontWeight, IconName, IconTone, OpenSshImportPreview, OpenSshImportStatus, PathPromptOptions,
    RemCmdApp, SharedString, SshRuntime, TextButtonTone, UI_MONOSPACE_FONT_FAMILY, Window,
    apply_openssh_import, default_openssh_config_path, div, icon, preview_openssh_import,
    profile_auth_label, px, text_button,
};
use gpui::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

impl RemCmdApp {
    pub(super) fn show_openssh_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::OpenSshImport;
        self.settings_focus_handle.focus(window);
        if self.openssh_import_preview.is_none()
            && !self.openssh_import_loading
            && let Ok(path) = default_openssh_config_path()
        {
            self.load_openssh_preview(path, cx);
        }
        cx.notify();
    }

    pub(super) fn choose_openssh_config(&mut self, cx: &mut Context<Self>) {
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("common-select").into()),
        });
        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |this, cx| this.load_openssh_preview(path, cx));
                }
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.openssh_import_error =
                        Some(format!("{}: {error}", this.tr("app-file-picker-failed")));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn load_openssh_preview(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.openssh_import_loading {
            return;
        }
        self.openssh_import_loading = true;
        self.openssh_import_error = None;
        let profiles = self.profiles.clone();
        let runtime = cx.global::<SshRuntime>().handle();
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        store.redactor().register_text(&path.to_string_lossy());
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn_blocking(move || preview_openssh_import(&path, &profiles))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.openssh_import_loading = false;
                match result {
                    Ok(Ok(preview)) => {
                        let mut selected = preview
                            .candidates
                            .iter()
                            .filter(|candidate| {
                                matches!(
                                    candidate.status,
                                    OpenSshImportStatus::New | OpenSshImportStatus::Update
                                )
                            })
                            .map(|candidate| candidate.alias.clone())
                            .collect();
                        include_openssh_dependencies(&preview, &mut selected);
                        this.openssh_selected_aliases = selected;
                        this.openssh_overwrite_conflicts.clear();
                        store.record(
                            DiagnosticLevel::Info,
                            "openssh.import",
                            "OpenSSH import preview completed",
                            [(
                                "candidate_count".into(),
                                preview.candidates.len().to_string(),
                            )],
                        );
                        this.openssh_import_preview = Some(preview);
                    }
                    Ok(Err(error)) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-preview-failed")));
                        store.record(
                            DiagnosticLevel::Warn,
                            "openssh.import",
                            "OpenSSH import preview failed",
                            [("error_kind".into(), format!("{:?}", error.kind()))],
                        );
                    }
                    Err(error) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-preview-failed")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn toggle_openssh_candidate(&mut self, alias: String, cx: &mut Context<Self>) {
        if !self.openssh_selected_aliases.remove(&alias) {
            self.openssh_selected_aliases.insert(alias);
            if let Some(preview) = self.openssh_import_preview.as_ref() {
                include_openssh_dependencies(preview, &mut self.openssh_selected_aliases);
            }
        }
        cx.notify();
    }

    pub(super) fn toggle_openssh_conflict_policy(&mut self, alias: String, cx: &mut Context<Self>) {
        if !self.openssh_overwrite_conflicts.remove(&alias) {
            self.openssh_overwrite_conflicts.insert(alias);
        }
        cx.notify();
    }

    pub(super) fn cycle_openssh_authentication(&mut self, alias: String, cx: &mut Context<Self>) {
        let Some(candidate) = self.openssh_import_preview.as_mut().and_then(|preview| {
            preview
                .candidates
                .iter_mut()
                .find(|candidate| candidate.alias == alias)
        }) else {
            return;
        };
        let identity_file = candidate.identity_file().map(Path::to_path_buf);
        let Some(profile) = candidate.profile.as_mut() else {
            return;
        };
        profile.auth = match &profile.auth {
            AuthConfig::None => AuthConfig::Password,
            AuthConfig::Password => identity_file
                .map(|path| AuthConfig::PrivateKey { path })
                .unwrap_or(AuthConfig::Agent),
            AuthConfig::Agent => AuthConfig::None,
            AuthConfig::PrivateKey { .. } => AuthConfig::Agent,
        };
        cx.notify();
    }

    pub(super) fn apply_openssh_preview(&mut self, cx: &mut Context<Self>) {
        if self.openssh_import_loading || self.openssh_selected_aliases.is_empty() {
            return;
        }
        let Some(preview) = self.openssh_import_preview.clone() else {
            return;
        };
        let root_path = preview.root_path.clone();
        self.openssh_import_loading = true;
        self.openssh_import_error = None;
        let existing = self.profiles.clone();
        let profiles_path = self.profiles_path.clone();
        let selected = self.openssh_selected_aliases.clone();
        let overwrite = self.openssh_overwrite_conflicts.clone();
        let runtime = cx.global::<SshRuntime>().handle();
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn_blocking(move || {
                    apply_openssh_import(
                        &profiles_path,
                        &existing,
                        &preview.candidates,
                        &selected,
                        &overwrite,
                    )
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.openssh_import_loading = false;
                match result {
                    Ok(Ok(profiles)) => {
                        let imported = profiles.len().saturating_sub(this.profiles.len());
                        this.profiles = profiles;
                        this.selected_profile_id = this
                            .selected_profile_id
                            .clone()
                            .filter(|id| this.profiles.iter().any(|profile| &profile.id == id))
                            .or_else(|| this.profiles.first().map(|profile| profile.id.clone()));
                        this.openssh_import_error = None;
                        store.record(
                            DiagnosticLevel::Info,
                            "openssh.import",
                            "OpenSSH import applied",
                            [("new_profile_count".into(), imported.to_string())],
                        );
                        this.openssh_selected_aliases.clear();
                        this.load_openssh_preview(root_path, cx);
                    }
                    Ok(Err(error)) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-apply-failed")));
                        store.record(
                            DiagnosticLevel::Error,
                            "openssh.import",
                            "OpenSSH import apply failed",
                            [("error_kind".into(), "apply".into())],
                        );
                    }
                    Err(error) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-apply-failed")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_openssh_import(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut candidates = div().flex().flex_col().gap_2().w_full();
        if let Some(preview) = self.openssh_import_preview.as_ref() {
            if !preview.warnings.is_empty() {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", preview.warnings.len());
                let warning_text = preview
                    .warnings
                    .iter()
                    .map(|warning| {
                        format!(
                            "{}:{}: {}",
                            warning.path.display(),
                            warning.line,
                            warning.message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                candidates = candidates.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(self.theme.control_bg)
                        .text_sm()
                        .text_color(self.theme.status_warn)
                        .child(self.tr_with("import-warning-count", &args))
                        .child(
                            div()
                                .font_family(UI_MONOSPACE_FONT_FAMILY)
                                .text_xs()
                                .child(warning_text),
                        ),
                );
            }
            for candidate in &preview.candidates {
                let alias = candidate.alias.clone();
                let toggle_alias = alias.clone();
                let policy_alias = alias.clone();
                let auth_alias = alias.clone();
                let selected = self.openssh_selected_aliases.contains(&alias);
                let invalid = candidate.status == OpenSshImportStatus::Invalid;
                let overwrite = self.openssh_overwrite_conflicts.contains(&alias);
                let status_key = openssh_status_key(candidate.status);
                let auth_label = candidate
                    .profile
                    .as_ref()
                    .map(|profile| profile_auth_label(&profile.auth, &self.localizer))
                    .unwrap_or_else(|| self.tr("common-none"));
                let endpoint = candidate
                    .profile
                    .as_ref()
                    .map(ConnectionProfile::address)
                    .unwrap_or_default();
                let warning_text = candidate
                    .warnings
                    .iter()
                    .map(|warning| {
                        format!(
                            "{}:{}: {}",
                            warning.path.display(),
                            warning.line,
                            warning.message
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                candidates = candidates.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .py_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(self.theme.border)
                        .bg(self.theme.settings_group_bg)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id(SharedString::from(format!("import-select-{alias}")))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(20.0))
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(self.theme.border_strong)
                                        .bg(if selected {
                                            self.theme.accent
                                        } else {
                                            self.theme.control_bg
                                        })
                                        .cursor_pointer()
                                        .when(selected, |this| {
                                            this.child(icon(
                                                IconName::Check,
                                                self.theme,
                                                IconTone::Default,
                                                13.0,
                                            ))
                                        })
                                        .when(!invalid, |this| {
                                            this.on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle_openssh_candidate(
                                                    toggle_alias.clone(),
                                                    cx,
                                                );
                                            }))
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(alias.clone()),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .text_color(self.theme.text_muted)
                                                .child(endpoint),
                                        ),
                                )
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .bg(self.theme.control_bg)
                                        .child(self.tr(status_key)),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("import-auth-{alias}")))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .bg(self.theme.control_bg)
                                        .cursor_pointer()
                                        .child(auth_label)
                                        .when(!invalid, |this| {
                                            this.on_click(cx.listener(move |this, _, _, cx| {
                                                this.cycle_openssh_authentication(
                                                    auth_alias.clone(),
                                                    cx,
                                                );
                                            }))
                                        }),
                                ),
                        )
                        .when(candidate.status == OpenSshImportStatus::Conflict, |this| {
                            this.child(
                                div()
                                    .id(SharedString::from(format!("import-policy-{alias}")))
                                    .text_sm()
                                    .text_color(self.theme.status_warn)
                                    .cursor_pointer()
                                    .child(if overwrite {
                                        self.tr("import-overwrite-local")
                                    } else {
                                        self.tr("import-keep-local")
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_openssh_conflict_policy(
                                            policy_alias.clone(),
                                            cx,
                                        );
                                    })),
                            )
                        })
                        .when(!warning_text.is_empty(), |this| {
                            this.child(
                                div()
                                    .font_family(UI_MONOSPACE_FONT_FAMILY)
                                    .text_xs()
                                    .text_color(self.theme.status_warn)
                                    .child(warning_text),
                            )
                        }),
                );
            }
            if preview.candidates.is_empty() {
                candidates = candidates.child(
                    div()
                        .py_8()
                        .text_center()
                        .text_color(self.theme.text_muted)
                        .child(self.tr("import-no-candidates")),
                );
            }
        }

        let browse = text_button(
            "openssh-browse",
            self.tr("common-browse"),
            TextButtonTone::Secondary,
            !self.openssh_import_loading,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.choose_openssh_config(cx)));
        let apply = text_button(
            "openssh-apply",
            if self.openssh_import_loading {
                self.tr("common-loading")
            } else {
                self.tr("import-apply")
            },
            TextButtonTone::Primary,
            !self.openssh_import_loading && !self.openssh_selected_aliases.is_empty(),
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.apply_openssh_preview(cx)));
        let source = self
            .openssh_import_preview
            .as_ref()
            .map(|preview| preview.root_path.display().to_string())
            .unwrap_or_else(|| {
                default_openssh_config_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            });
        let back = self
            .render_icon_button(
                "openssh-back-to-settings",
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
                                    .child(self.tr("import-title")),
                            ),
                        )
                        .child(div().flex().gap_2().child(browse).child(apply)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .text_sm()
                        .child(
                            div()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("import-source")),
                        )
                        .child(
                            div()
                                .font_family(UI_MONOSPACE_FONT_FAMILY)
                                .truncate()
                                .child(source),
                        ),
                )
                .when_some(self.openssh_import_error.as_ref(), |this, error| {
                    this.child(
                        div()
                            .text_sm()
                            .text_color(self.theme.error_text)
                            .child(error.clone()),
                    )
                })
                .child(
                    div()
                        .id("openssh-import-candidates")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_y_scroll()
                        .child(candidates),
                ),
        )
    }
}

pub(super) const fn openssh_status_key(status: OpenSshImportStatus) -> &'static str {
    match status {
        OpenSshImportStatus::New => "import-status-new",
        OpenSshImportStatus::Update => "import-status-update",
        OpenSshImportStatus::Unchanged => "import-status-unchanged",
        OpenSshImportStatus::Conflict => "import-status-conflict",
        OpenSshImportStatus::Invalid => "import-status-invalid",
    }
}

pub(super) fn include_openssh_dependencies(
    preview: &OpenSshImportPreview,
    selected_aliases: &mut HashSet<String>,
) {
    let alias_by_id = preview
        .candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .profile
                .as_ref()
                .map(|profile| (profile.id.as_str(), candidate.alias.as_str()))
        })
        .collect::<HashMap<_, _>>();
    loop {
        let mut added = false;
        for candidate in &preview.candidates {
            if !selected_aliases.contains(&candidate.alias) {
                continue;
            }
            let Some(profile) = candidate.profile.as_ref() else {
                continue;
            };
            for jump_id in &profile.route.jump_host_ids {
                if let Some(alias) = alias_by_id.get(jump_id.as_str()) {
                    added |= selected_aliases.insert((*alias).to_owned());
                }
            }
        }
        if !added {
            break;
        }
    }
}
