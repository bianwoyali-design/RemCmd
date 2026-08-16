#[cfg(not(target_os = "macos"))]
use super::PathPromptOptions;
use super::{
    ActivePanel, AnyElement, AuthConfig, ConnectionProfile, ConnectionRoute, Context,
    CredentialKind, DiagnosticLevel, DiagnosticsGlobal, Entity, FontWeight, IconName, IconTone,
    IntoElement, Localizer, Pixels, ProxyConfig, RemCmdApp, SecretString, SessionState,
    SharedString, SshRuntime, TextButtonTone, TextField, Window, deferred,
    delete_profile_auth_credentials, div, icon, load_credential, proxy_command_content_digest, px,
    save_profiles, save_profiles_with_route_secrets, session_state_key, text_button,
};
use gpui::Focusable;
use gpui::prelude::*;
use secrecy::ExposeSecret;
use std::{collections::HashSet, path::PathBuf};

impl RemCmdApp {
    pub(super) fn selected_profile(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_profile_id.as_ref()?;

        self.profiles
            .iter()
            .find(|profile| &profile.id == selected_id)
    }

    pub(super) fn persist_profiles(&mut self) {
        if let Err(error) = save_profiles(&self.profiles_path, &self.profiles) {
            self.form_error = Some(format!("{}:\n{error}", self.tr("app-save-profiles-failed")));
        }
    }

    pub(super) fn load_editor_for_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.editor = None;
            return;
        };

        let auth_kind = ProfileAuthKind::from_config(&profile.auth);
        let private_key_path = match &profile.auth {
            AuthConfig::PrivateKey { path } => path.to_string_lossy().into_owned(),
            AuthConfig::None | AuthConfig::Password | AuthConfig::Agent => String::new(),
        };
        let proxy_kind = ProfileProxyKind::from_config(profile.route.upstream_proxy.as_ref());
        let (proxy_host, proxy_port, proxy_username) = match &profile.route.upstream_proxy {
            Some(ProxyConfig::HttpConnect {
                host,
                port,
                username,
            })
            | Some(ProxyConfig::Socks5 {
                host,
                port,
                username,
            }) => (
                host.clone(),
                port.to_string(),
                username.clone().unwrap_or_default(),
            ),
            Some(ProxyConfig::ProxyCommand { .. }) | None => {
                (String::new(), String::new(), String::new())
            }
        };
        let proxy_secret_kind = match proxy_kind {
            ProfileProxyKind::HttpConnect | ProfileProxyKind::Socks5 => {
                Some(CredentialKind::ProxyPassword)
            }
            ProfileProxyKind::ProxyCommand => Some(CredentialKind::ProxyCommand),
            ProfileProxyKind::Direct => None,
        };
        let jump_search =
            cx.new(|cx| TextField::new(cx, "", self.tr("sidebar-search-placeholder")));
        cx.observe(&jump_search, |_, _, cx| cx.notify()).detach();

        self.editor = Some(ProfileEditor {
            mode: ProfileEditorMode::Edit,
            profile_id: profile.id.clone(),
            name: cx.new(|cx| TextField::new(cx, profile.name, self.tr("field-name"))),
            host: cx.new(|cx| TextField::new(cx, profile.host, self.tr("field-host"))),
            port: cx.new(|cx| TextField::new(cx, profile.port.to_string(), self.tr("field-port"))),
            username: cx.new(|cx| TextField::new(cx, profile.username, self.tr("field-username"))),
            auth_kind,
            private_key_path: cx
                .new(|cx| TextField::new(cx, private_key_path, self.tr("field-private-key"))),
            proxy_kind,
            proxy_host: cx.new(|cx| TextField::new(cx, proxy_host, self.tr("field-proxy-host"))),
            proxy_port: cx.new(|cx| TextField::new(cx, proxy_port, self.tr("field-proxy-port"))),
            proxy_username: cx
                .new(|cx| TextField::new(cx, proxy_username, self.tr("field-proxy-username"))),
            proxy_password: cx.new(|cx| TextField::new_secure(cx, self.tr("field-proxy-password"))),
            proxy_command: cx.new(|cx| TextField::new(cx, "", self.tr("field-proxy-command"))),
            jump_search,
            jump_host_ids: profile.route.jump_host_ids.clone(),
            proxy_secret_loaded: proxy_secret_kind.is_none(),
        });

        if let Some(kind) = proxy_secret_kind {
            let profile_id = profile.id.clone();
            let runtime = cx.global::<SshRuntime>().handle();
            cx.spawn(async move |this, cx| {
                let lookup_id = profile_id.clone();
                let result = runtime
                    .spawn_blocking(move || load_credential(&lookup_id, kind))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let Some(editor) = this
                        .editor
                        .as_mut()
                        .filter(|editor| editor.profile_id == profile_id)
                    else {
                        return;
                    };
                    match result {
                        Ok(Ok(secret)) => {
                            if let Some(secret) = secret {
                                let value = secret.expose_secret().to_owned();
                                match kind {
                                    CredentialKind::ProxyPassword => editor
                                        .proxy_password
                                        .update(cx, |field, cx| field.replace_all(value, cx)),
                                    CredentialKind::ProxyCommand => editor
                                        .proxy_command
                                        .update(cx, |field, cx| field.replace_all(value, cx)),
                                    CredentialKind::Password
                                    | CredentialKind::PrivateKeyPassphrase => unreachable!(),
                                }
                            }
                            editor.proxy_secret_loaded = true;
                        }
                        Ok(Err(error)) => {
                            this.form_error = Some(error.to_string());
                        }
                        Err(error) => this.form_error = Some(error.to_string()),
                    }
                    cx.notify();
                });
            })
            .detach();
        }

        self.profile_auth_selector_open = false;
        self.form_error = None;
    }

    pub(super) fn open_new_profile_editor(&mut self, cx: &mut Context<Self>) {
        let number = self.next_profile_number;
        let jump_search =
            cx.new(|cx| TextField::new(cx, "", self.tr("sidebar-search-placeholder")));
        cx.observe(&jump_search, |_, _, cx| cx.notify()).detach();
        self.editor = Some(ProfileEditor {
            mode: ProfileEditorMode::Create,
            profile_id: format!("demo-{number}"),
            name: cx.new(|cx| TextField::new(cx, "", self.tr("field-name"))),
            host: cx.new(|cx| TextField::new(cx, "", self.tr("field-host"))),
            port: cx.new(|cx| TextField::new(cx, "22", self.tr("field-port"))),
            username: cx.new(|cx| TextField::new(cx, "", self.tr("field-username"))),
            auth_kind: ProfileAuthKind::Password,
            private_key_path: cx.new(|cx| TextField::new(cx, "", self.tr("field-private-key"))),
            proxy_kind: ProfileProxyKind::Direct,
            proxy_host: cx.new(|cx| TextField::new(cx, "", self.tr("field-proxy-host"))),
            proxy_port: cx.new(|cx| TextField::new(cx, "", self.tr("field-proxy-port"))),
            proxy_username: cx.new(|cx| TextField::new(cx, "", self.tr("field-proxy-username"))),
            proxy_password: cx.new(|cx| TextField::new_secure(cx, self.tr("field-proxy-password"))),
            proxy_command: cx.new(|cx| TextField::new(cx, "", self.tr("field-proxy-command"))),
            jump_search,
            jump_host_ids: Vec::new(),
            proxy_secret_loaded: true,
        });
        self.profile_auth_selector_open = false;
        self.form_error = None;
        cx.notify();
    }

    pub(super) fn select_profile(
        &mut self,
        profile_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::Server;
        self.profile_context_menu = None;
        self.open_settings_selector = None;
        self.bottom_panel_open = false;
        self.bottom_panel_resize = None;
        self.active_session_id = None;
        self.selected_profile_id = Some(profile_id);
        self.focused_terminal_session_id = None;
        self.settings_focus_handle.focus(window);
        self.sync_performance_monitoring();
        cx.notify();
    }

    pub(super) fn add_profile(&mut self, cx: &mut Context<Self>) {
        self.open_new_profile_editor(cx);
    }

    pub(super) fn edit_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        self.selected_profile_id = Some(profile_id);
        self.active_panel = ActivePanel::Server;
        self.active_session_id = None;
        self.profile_context_menu = None;
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }

    pub(super) fn toggle_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cfg!(target_os = "windows") {
            if !self.left_sidebar_open {
                self.toggle_left_sidebar(cx);
            }
            self.sidebar_search.focus_handle(cx).focus(window);
            return;
        }

        self.sidebar_search_visible = !self.sidebar_search_visible;
        if self.sidebar_search_visible {
            self.sidebar_search.focus_handle(cx).focus(window);
        } else {
            self.sidebar_search
                .update(cx, |search, cx| search.clear(cx));
        }
        cx.notify();
    }

    pub(super) fn toggle_connections(&mut self, cx: &mut Context<Self>) {
        self.connections_expanded = !self.connections_expanded;
        cx.notify();
    }

    pub(super) fn delete_profile(&mut self, selected_id: String, cx: &mut Context<Self>) {
        if let Some(referrer) = self.profiles.iter().find(|profile| {
            profile.id != selected_id
                && profile
                    .route
                    .jump_host_ids
                    .iter()
                    .any(|jump_id| jump_id == &selected_id)
        }) {
            self.form_error = Some(format!(
                "{}: {}",
                self.tr("profile-delete-in-use"),
                referrer.name
            ));
            cx.notify();
            return;
        }
        if self.sessions.iter().any(|session| {
            session.profile_id == selected_id && session.connection_state.can_disconnect()
        }) {
            self.form_error = Some(self.tr("profile-disconnect-before-delete"));
            cx.notify();
            return;
        }

        let Some(selected_index) = self
            .profiles
            .iter()
            .position(|profile| profile.id == selected_id)
        else {
            cx.notify();
            return;
        };

        let session_ids = self
            .sessions
            .iter()
            .filter(|session| session.profile_id == selected_id)
            .map(|session| session.id)
            .collect::<Vec<_>>();
        for session_id in session_ids {
            self.remove_session(session_id, cx);
        }

        self.profiles.remove(selected_index);

        if self.selected_profile_id.as_deref() == Some(selected_id.as_str()) {
            self.selected_profile_id = None;
            self.active_panel = ActivePanel::Home;
            self.active_session_id = None;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.profile_id == selected_id)
        {
            self.editor = None;
            self.profile_auth_selector_open = false;
        }
        self.profile_context_menu = None;
        self.persist_profiles();
        self.delete_stored_credentials(selected_id, None, None, cx);

        cx.notify();
    }

    pub(super) fn select_auth_method(
        &mut self,
        auth_kind: ProfileAuthKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };

        editor.auth_kind = auth_kind;
        let private_key_path = editor.private_key_path.clone();
        self.profile_auth_selector_open = false;
        self.form_error = None;

        if auth_kind == ProfileAuthKind::PrivateKey {
            window.focus(&private_key_path.focus_handle(cx));
        }

        cx.notify();
    }

    pub(super) fn select_proxy_method(
        &mut self,
        proxy_kind: ProfileProxyKind,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.proxy_kind = proxy_kind;
        editor.proxy_secret_loaded = true;
        if proxy_kind == ProfileProxyKind::ProxyCommand {
            editor.jump_host_ids.clear();
        }
        self.form_error = None;
        cx.notify();
    }

    pub(super) fn toggle_jump_host(&mut self, jump_id: String, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if jump_id == editor.profile_id {
            return;
        }
        if let Some(index) = editor
            .jump_host_ids
            .iter()
            .position(|candidate| candidate == &jump_id)
        {
            editor.jump_host_ids.remove(index);
        } else {
            if editor.proxy_kind == ProfileProxyKind::ProxyCommand {
                editor.proxy_kind = ProfileProxyKind::Direct;
            }
            editor.jump_host_ids.push(jump_id);
        }
        self.form_error = None;
        cx.notify();
    }

    pub(super) fn move_jump_host(
        &mut self,
        jump_id: String,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(index) = editor
            .jump_host_ids
            .iter()
            .position(|candidate| candidate == &jump_id)
        else {
            return;
        };
        let next = index.saturating_add_signed(direction);
        if next < editor.jump_host_ids.len() {
            editor.jump_host_ids.swap(index, next);
            cx.notify();
        }
    }

    pub(super) fn toggle_profile_auth_selector(&mut self, cx: &mut Context<Self>) {
        self.profile_auth_selector_open = !self.profile_auth_selector_open;
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    pub(super) fn browse_private_key(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };

        let profile_id = editor.profile_id.clone();
        let current_path = editor.private_key_path.read(cx).text();
        let current_path =
            (!current_path.trim().is_empty()).then(|| PathBuf::from(current_path.trim()));

        cx.spawn(async move |this, cx| {
            let result = crate::private_key_picker::pick_private_key(current_path.as_deref());

            let _ = this.update(cx, |this, cx| match result {
                Ok(Some(path)) => this.set_private_key_path(&profile_id, path, cx),
                Ok(None) => {}
                Err(error) => {
                    this.form_error =
                        Some(format!("{}: {error}", this.tr("app-file-picker-failed")));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn browse_private_key(&mut self, cx: &mut Context<Self>) {
        let Some(profile_id) = self.editor.as_ref().map(|editor| editor.profile_id.clone()) else {
            return;
        };

        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("common-select").into()),
        });

        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                let Some(path) = paths.into_iter().next() else {
                    return;
                };

                let _ = this.update(cx, |this, cx| {
                    this.set_private_key_path(&profile_id, path, cx);
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    if this
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.profile_id == profile_id)
                    {
                        this.form_error =
                            Some(format!("{}: {error}", this.tr("app-file-picker-failed")));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn set_private_key_path(
        &mut self,
        profile_id: &str,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let placeholder = self.tr("field-private-key");
        let Some(editor) = self
            .editor
            .as_mut()
            .filter(|editor| editor.profile_id == profile_id)
        else {
            return;
        };

        let path = path.to_string_lossy().into_owned();
        editor.private_key_path = cx.new(|cx| TextField::new(cx, path, placeholder));
        self.form_error = None;
        cx.notify();
    }

    pub(super) fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };

        if !editor.proxy_secret_loaded {
            self.form_error = Some(self.tr("credential-checking"));
            cx.notify();
            return;
        }

        let name = editor.name.read(cx).text().trim().to_owned();
        let host = editor.host.read(cx).text().trim().to_owned();
        let port_text = editor.port.read(cx).text();
        let username = editor.username.read(cx).text().trim().to_owned();
        let private_key_path = editor.private_key_path.read(cx).text();

        if name.is_empty() || host.is_empty() || username.is_empty() {
            self.form_error = Some(self.tr("profile-validation-required"));
            cx.notify();
            return;
        }

        let Ok(port) = port_text.trim().parse::<u16>() else {
            self.form_error = Some(self.tr("profile-validation-port"));
            cx.notify();
            return;
        };

        if port == 0 {
            self.form_error = Some(self.tr("profile-validation-port"));
            cx.notify();
            return;
        };

        let auth = match editor.auth_kind.into_config(&private_key_path) {
            Ok(auth) => auth,
            Err(error) => {
                self.form_error = Some(self.tr(error));
                cx.notify();
                return;
            }
        };

        let credentials_changed = editor.mode == ProfileEditorMode::Edit
            && self
                .profiles
                .iter()
                .find(|profile| profile.id == editor.profile_id)
                .is_some_and(|profile| {
                    credentials_invalidated_by_edit(profile, &host, port, &username, &auth)
                });

        let proxy_host = editor.proxy_host.read(cx).text().trim().to_owned();
        let proxy_port_text = editor.proxy_port.read(cx).text();
        let proxy_username = editor.proxy_username.read(cx).text().trim().to_owned();
        let proxy_password_text = editor.proxy_password.read(cx).text();
        let proxy_command_text = editor.proxy_command.read(cx).text();
        let existing = self
            .profiles
            .iter()
            .find(|profile| profile.id == editor.profile_id)
            .cloned();
        let mut proxy_password = None;
        let mut proxy_command = None;
        let upstream_proxy = match editor.proxy_kind {
            ProfileProxyKind::Direct => None,
            ProfileProxyKind::HttpConnect | ProfileProxyKind::Socks5 => {
                let Ok(proxy_port) = proxy_port_text.trim().parse::<u16>() else {
                    self.form_error = Some(self.tr("profile-validation-port"));
                    cx.notify();
                    return;
                };
                if proxy_host.is_empty() || proxy_port == 0 {
                    self.form_error = Some(self.tr("profile-validation-proxy"));
                    cx.notify();
                    return;
                }
                if proxy_username.is_empty() && !proxy_password_text.is_empty() {
                    self.form_error = Some(self.tr("profile-validation-proxy-auth"));
                    cx.notify();
                    return;
                }
                if !proxy_password_text.is_empty() {
                    proxy_password = Some(SecretString::new(proxy_password_text.into_boxed_str()));
                }
                let username = (!proxy_username.is_empty()).then_some(proxy_username);
                Some(match editor.proxy_kind {
                    ProfileProxyKind::HttpConnect => ProxyConfig::HttpConnect {
                        host: proxy_host,
                        port: proxy_port,
                        username,
                    },
                    ProfileProxyKind::Socks5 => ProxyConfig::Socks5 {
                        host: proxy_host,
                        port: proxy_port,
                        username,
                    },
                    ProfileProxyKind::Direct | ProfileProxyKind::ProxyCommand => unreachable!(),
                })
            }
            ProfileProxyKind::ProxyCommand => {
                let command = proxy_command_text.trim();
                if command.is_empty() {
                    self.form_error = Some(self.tr("profile-validation-proxy-command"));
                    cx.notify();
                    return;
                }
                let command_digest = proxy_command_content_digest(command);
                let approved_digest = existing.as_ref().and_then(|profile| {
                    let unchanged_endpoint = profile.name == name
                        && profile.host == host
                        && profile.port == port
                        && profile.username == username;
                    match profile.route.upstream_proxy.as_ref() {
                        Some(ProxyConfig::ProxyCommand {
                            command_digest: existing_digest,
                            approved_digest,
                        }) if unchanged_endpoint && existing_digest == &command_digest => {
                            approved_digest.clone()
                        }
                        _ => None,
                    }
                });
                proxy_command = Some(SecretString::new(command.to_owned().into_boxed_str()));
                Some(ProxyConfig::ProxyCommand {
                    command_digest,
                    approved_digest,
                })
            }
        };

        let mut seen_jumps = HashSet::new();
        if editor.jump_host_ids.iter().any(|jump_id| {
            jump_id == &editor.profile_id
                || !seen_jumps.insert(jump_id)
                || !self.profiles.iter().any(|profile| &profile.id == jump_id)
        }) {
            self.form_error = Some(self.tr("profile-validation-jumps"));
            cx.notify();
            return;
        }
        if matches!(upstream_proxy, Some(ProxyConfig::ProxyCommand { .. }))
            && !editor.jump_host_ids.is_empty()
        {
            self.form_error = Some(self.tr("profile-validation-proxy-jump"));
            cx.notify();
            return;
        }

        let mut profile = existing
            .clone()
            .unwrap_or_else(|| ConnectionProfile::new(&editor.profile_id, "", "", 22, ""));
        profile.name = name;
        profile.host = host;
        profile.port = port;
        profile.username = username;
        profile.auth = auth;
        profile.route = ConnectionRoute {
            upstream_proxy,
            jump_host_ids: editor.jump_host_ids,
        };

        let mut next_profiles = self.profiles.clone();
        if let Some(index) = next_profiles
            .iter()
            .position(|candidate| candidate.id == profile.id)
        {
            next_profiles[index] = profile.clone();
        } else {
            next_profiles.push(profile.clone());
        }
        let had_proxy = existing
            .as_ref()
            .is_some_and(|profile| profile.route.upstream_proxy.is_some());
        let route_changed = existing
            .as_ref()
            .is_some_and(|existing| existing.route.upstream_proxy != profile.route.upstream_proxy);
        let route_secrets_changed =
            proxy_password.is_some() || proxy_command.is_some() || (had_proxy && route_changed);
        let profile_id = profile.id.clone();
        let profiles_path = self.profiles_path.clone();
        let runtime = cx.global::<SshRuntime>().handle();
        let store = cx.global::<DiagnosticsGlobal>().0.clone();
        if let Some(secret) = proxy_password.as_ref() {
            store.register_secret(secret);
        }
        if let Some(secret) = proxy_command.as_ref() {
            store.register_secret(secret);
        }
        *self
            .credential_mutations_in_progress
            .entry(profile_id.clone())
            .or_default() += 1;
        self.form_error = None;
        cx.spawn(async move |this, cx| {
            let task_profile_id = profile_id.clone();
            let result = runtime
                .spawn_blocking(move || {
                    if route_secrets_changed {
                        save_profiles_with_route_secrets(
                            &profiles_path,
                            &next_profiles,
                            &task_profile_id,
                            proxy_password.as_ref(),
                            proxy_command.as_ref(),
                        )?;
                    } else {
                        save_profiles(&profiles_path, &next_profiles)?;
                    }
                    let auth_cleanup_error = credentials_changed
                        .then(|| delete_profile_auth_credentials(&task_profile_id))
                        .and_then(Result::err)
                        .map(|error| error.to_string());
                    Ok::<_, std::io::Error>((next_profiles, auth_cleanup_error))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.credential_mutations_in_progress.remove(&profile_id);
                match result {
                    Ok(Ok((profiles, warning))) => {
                        this.profiles = profiles;
                        this.selected_profile_id = Some(profile_id.clone());
                        this.active_panel = ActivePanel::Server;
                        if editor.mode == ProfileEditorMode::Create {
                            this.next_profile_number += 1;
                        }
                        this.editor = None;
                        this.profile_auth_selector_open = false;
                        this.form_error = warning;
                        store.record(
                            DiagnosticLevel::Info,
                            "profiles.save",
                            "Connection profile saved",
                            [("has_route".into(), (!profile.route.is_direct()).to_string())],
                        );
                    }
                    Ok(Err(error)) => this.form_error = Some(error.to_string()),
                    Err(error) => this.form_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.profile_auth_selector_open = false;
        self.form_error = None;
        cx.notify();
    }

    pub(super) fn render_profile_context_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(menu_state) = self.profile_context_menu.as_ref() else {
            return div().into_any_element();
        };
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == menu_state.profile_id)
        else {
            return div().into_any_element();
        };
        let profile_id = profile.id.clone();
        let has_live_session = self.sessions.iter().any(|session| {
            session.profile_id == profile_id && session.connection_state.can_disconnect()
        });
        let can_open_terminal = self.credential_lookup_task.is_none()
            && !self
                .credential_mutations_in_progress
                .contains_key(&profile_id);
        let viewport = window.viewport_size();
        let left = f32::from(menu_state.position.x)
            .min((f32::from(viewport.width) - 196.0).max(8.0))
            .max(8.0);
        let top = f32::from(menu_state.position.y)
            .min((f32::from(viewport.height) - 142.0).max(8.0))
            .max(8.0);

        let terminal_profile_id = profile_id.clone();
        let mut new_terminal = self.render_context_menu_item(
            "profile-context-new-terminal",
            IconName::Connect,
            self.tr("profile-context-new-terminal"),
            IconTone::Default,
            can_open_terminal,
        );
        if can_open_terminal {
            new_terminal = new_terminal.on_click(cx.listener(move |this, _, window, cx| {
                this.profile_context_menu = None;
                this.selected_profile_id = Some(terminal_profile_id.clone());
                this.active_panel = ActivePanel::Server;
                this.connect_selected_profile_in_new_session(window, cx);
            }));
        }

        let edit_profile_id = profile_id.clone();
        let mut edit = self.render_context_menu_item(
            "profile-context-edit",
            IconName::Edit,
            self.tr("profile-context-edit"),
            IconTone::Default,
            true,
        );
        edit = edit.on_click(cx.listener(move |this, _, _, cx| {
            this.edit_profile(edit_profile_id.clone(), cx);
        }));

        let delete_profile_id = profile_id;
        let mut delete = self.render_context_menu_item(
            "profile-context-delete",
            IconName::Delete,
            self.tr("profile-context-delete"),
            IconTone::Danger,
            !has_live_session,
        );
        if !has_live_session {
            delete = delete.on_click(cx.listener(move |this, _, _, cx| {
                this.delete_profile(delete_profile_id.clone(), cx);
            }));
        }

        self.glass_floating_surface()
            .id("profile_context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(188.0))
            .flex()
            .flex_col()
            .p_1()
            .occlude()
            .child(new_terminal)
            .child(edit)
            .child(self.render_context_menu_separator())
            .child(delete)
            .into_any_element()
    }

    pub(super) fn render_profile_editor_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };
        let title = match editor.mode {
            ProfileEditorMode::Create => self.tr("profile-new-title"),
            ProfileEditorMode::Edit => self.tr("profile-edit-title"),
        };
        let save = text_button(
            "save_profile",
            self.tr("common-save"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.save_editor(cx)));
        let cancel = text_button(
            "cancel_profile",
            self.tr("common-cancel"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.cancel_editor(cx)));

        let form = div()
            .id("connection_form")
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .child(self.render_form_row(self.tr("field-name").into(), editor.name.clone()))
            .child(self.render_form_row(self.tr("field-host").into(), editor.host.clone()))
            .child(self.render_form_row(self.tr("field-port").into(), editor.port.clone()))
            .child(self.render_form_row(self.tr("field-username").into(), editor.username.clone()))
            .child(self.render_auth_method_row(editor.auth_kind, cx))
            .when(editor.auth_kind == ProfileAuthKind::PrivateKey, |this| {
                this.child(self.render_private_key_row(editor.private_key_path.clone(), cx))
            })
            .when(
                editor.mode == ProfileEditorMode::Edit
                    && matches!(
                        editor.auth_kind,
                        ProfileAuthKind::Password | ProfileAuthKind::PrivateKey
                    ),
                |this| this.child(self.render_saved_credential_row(cx)),
            )
            .child(self.render_route_editor(editor, cx))
            .when_some(self.form_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .mt_3()
                        .text_sm()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            });

        div()
            .id("profile_editor_overlay")
            .key_context("ProfileEditor")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(self.theme.overlay_bg)
            .occlude()
            .child(
                self.glass_floating_surface()
                    .flex()
                    .w_full()
                    .max_w(px(620.0))
                    .max_h(px(640.0))
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .h(px(46.0))
                            .px_4()
                            .border_b_1()
                            .border_color(self.theme.border)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(form)
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_between()
                            .h(px(54.0))
                            .px_4()
                            .border_t_1()
                            .border_color(self.theme.border)
                            .child(cancel)
                            .child(save),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_connection_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.selected_session();
        let state = session
            .map(|session| session.connection_state)
            .unwrap_or(SessionState::Disconnected);
        let checking_keychain = session.is_some_and(|session| {
            self.credential_lookup_task.is_some()
                && self.credential_lookup_session_id == Some(session.id)
        });
        let updating_keychain = self
            .selected_profile_id
            .as_deref()
            .is_some_and(|profile_id| {
                self.credential_mutations_in_progress
                    .contains_key(profile_id)
            });
        let can_connect = state.can_connect() && !checking_keychain && !updating_keychain;
        let can_disconnect = state.can_disconnect();

        let label = match state {
            _ if checking_keychain => self.tr("credential-checking"),
            _ if updating_keychain => self.tr("credential-updating"),
            SessionState::Failed => self.tr("common-retry"),
            SessionState::Disconnecting => self.tr("connection-status-disconnecting"),
            _ if can_disconnect => self.tr("common-disconnect"),
            _ if self
                .selected_profile_id
                .as_deref()
                .is_some_and(|profile_id| self.terminal_has_ended(profile_id)) =>
            {
                self.tr("common-reconnect")
            }
            _ => self.tr("common-connect"),
        };

        let tone = if can_disconnect {
            IconTone::Danger
        } else {
            IconTone::Accent
        };
        let icon = if can_disconnect {
            IconName::Disconnect
        } else if state == SessionState::Failed
            || self
                .selected_profile_id
                .as_deref()
                .is_some_and(|profile_id| self.terminal_has_ended(profile_id))
        {
            IconName::Reconnect
        } else {
            IconName::Connect
        };

        let status_color = match state {
            _ if checking_keychain || updating_keychain => self.theme.status_warn,
            SessionState::Connected => self.theme.status_ok,
            SessionState::Failed => self.theme.error_text,
            SessionState::Connecting
            | SessionState::Authenticating
            | SessionState::Disconnecting => self.theme.status_warn,
            SessionState::Disconnected => self.theme.text_muted,
        };

        let mut action = self.render_icon_button(
            "connection_action",
            icon,
            label,
            tone,
            can_connect || can_disconnect,
        );

        if can_connect {
            action = action.on_click(cx.listener(|this, _, window, cx| {
                this.connect_selected_profile(window, cx);
            }));
        } else if can_disconnect {
            action = action.on_click(cx.listener(|this, _, _, cx| {
                this.disconnect_active_connection(cx);
            }));
        }

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .justify_start()
            .gap_1()
            .child(
                div()
                    .min_w(px(0.0))
                    .max_w(px(220.0))
                    .truncate()
                    .text_sm()
                    .text_color(status_color)
                    .child(self.connection_status_text()),
            )
            .child(action)
    }

    pub(super) fn connection_status_text(&self) -> String {
        let session = self.selected_session();
        if session.is_some_and(|session| {
            self.credential_lookup_task.is_some()
                && self.credential_lookup_session_id == Some(session.id)
        }) {
            return self.tr("credential-checking");
        }
        if self
            .selected_profile_id
            .as_deref()
            .is_some_and(|profile_id| {
                self.credential_mutations_in_progress
                    .contains_key(profile_id)
            })
        {
            return self.tr("credential-updating");
        }

        let state = session
            .map(|session| session.connection_state)
            .unwrap_or(SessionState::Disconnected);
        self.tr(session_state_key(state))
    }

    pub(super) fn render_auth_method_row(
        &self,
        selected: ProfileAuthKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let control_group: SharedString = "profile-auth-selector-control".into();
        let button_hover = self.theme.control_hover_bg;
        let button_pressed = self.theme.control_pressed_bg;
        let picker = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.0))
            .rounded_full()
            .bg(if self.profile_auth_selector_open {
                self.theme.control_pressed_bg
            } else {
                self.theme.settings_picker_bg
            })
            .when(!self.profile_auth_selector_open, |this| {
                this.group_hover(control_group.clone(), |style| {
                    style.bg(self.theme.transparent)
                })
            })
            .child(icon(IconName::Picker, self.theme, IconTone::Default, 15.0));
        let button = div()
            .id("profile-auth-selector")
            .group(control_group)
            .flex()
            .flex_none()
            .max_w(px(180.0))
            .items_center()
            .h(px(24.0))
            .pl(px(6.0))
            .pr(px(1.0))
            .rounded_lg()
            .bg(if self.profile_auth_selector_open {
                self.theme.control_hover_bg
            } else {
                self.theme.transparent
            })
            .text_sm()
            .cursor_pointer()
            .hover(move |this| this.bg(button_hover))
            .active(move |this| this.bg(button_pressed))
            .child(
                div()
                    .flex_none()
                    .max_w(px(147.0))
                    .truncate()
                    .pr(px(4.0))
                    .text_right()
                    .child(self.tr(profile_auth_kind_key(selected))),
            )
            .child(picker)
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_profile_auth_selector(cx);
            }));

        let mut selector = div().relative().flex().flex_none().child(button);
        if self.profile_auth_selector_open {
            let mut menu = self
                .glass_floating_surface()
                .id("profile-auth-selector-menu")
                .absolute()
                .top(px(26.0))
                .right_0()
                .w(px(180.0))
                .flex()
                .flex_col()
                .p_1()
                .text_sm()
                .occlude();
            let option_hover = self.theme.accent;
            let option_pressed = self.theme.accent_hover;
            let on_accent = self.theme.on_accent;
            for (index, (auth_kind, _)) in ProfileAuthKind::OPTIONS.iter().copied().enumerate() {
                let is_selected = auth_kind == selected;
                let hover_group: SharedString = format!("profile-auth-option-{index}-hover").into();
                let check = self.render_select_menu_check(is_selected, hover_group.clone());
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("profile-auth-option-{index}")))
                        .group(hover_group.clone())
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .h(px(28.0))
                        .px_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |this| this.bg(option_hover).text_color(on_accent))
                        .active(move |this| this.bg(option_pressed).text_color(on_accent))
                        .child(check)
                        .child(self.render_select_menu_label(
                            self.tr(profile_auth_kind_key(auth_kind)).into(),
                            hover_group,
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.select_auth_method(auth_kind, window, cx);
                        })),
                );
            }
            selector = selector.child(deferred(menu).with_priority(30));
        }

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .mt_3()
            .child(
                div()
                    .flex_none()
                    .w(px(PROFILE_FORM_LABEL_WIDTH))
                    .truncate()
                    .child(self.tr("profile-authentication")),
            )
            .child(selector)
    }

    pub(super) fn render_private_key_row(
        &self,
        field: Entity<TextField>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .mt_3()
            .child(
                div()
                    .flex_none()
                    .w(px(PROFILE_FORM_LABEL_WIDTH))
                    .truncate()
                    .child(self.tr("profile-key-file")),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w(px(0.0)).child(field))
                    .child(
                        self.render_icon_button(
                            "browse_private_key",
                            IconName::Folder,
                            self.tr("common-browse"),
                            IconTone::Default,
                            true,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.browse_private_key(cx);
                        })),
                    ),
            )
    }

    pub(super) fn render_saved_credential_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .mt_3()
            .child(
                div()
                    .flex_none()
                    .w(px(PROFILE_FORM_LABEL_WIDTH))
                    .truncate()
                    .child(self.tr("profile-credential")),
            )
            .child(
                self.render_icon_button(
                    "forget_saved_credential",
                    IconName::ForgetCredential,
                    self.tr("profile-forget-credential"),
                    IconTone::Danger,
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.forget_selected_credential(cx);
                })),
            )
    }

    pub(super) fn render_form_row(
        &self,
        label: SharedString,
        field: Entity<TextField>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .mt_3()
            .child(
                div()
                    .flex_none()
                    .w(px(PROFILE_FORM_LABEL_WIDTH))
                    .truncate()
                    .child(label),
            )
            .child(div().flex_1().min_w(px(0.0)).child(field))
    }

    pub(super) fn render_route_editor(
        &self,
        editor: &ProfileEditor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut proxy_options = div().flex().flex_wrap().gap_1();
        for proxy_kind in ProfileProxyKind::OPTIONS {
            let selected = editor.proxy_kind == proxy_kind;
            proxy_options = proxy_options.child(
                div()
                    .id(SharedString::from(format!("profile-proxy-{proxy_kind:?}")))
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
                    .child(self.tr(proxy_kind.label_key()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_proxy_method(proxy_kind, cx);
                    })),
            );
        }

        let mut route = div()
            .flex()
            .flex_col()
            .mt_5()
            .pt_4()
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .mb_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("profile-route")),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .w(px(PROFILE_FORM_LABEL_WIDTH))
                            .pt_1()
                            .child(self.tr("profile-proxy")),
                    )
                    .child(proxy_options),
            );
        match editor.proxy_kind {
            ProfileProxyKind::Direct => {}
            ProfileProxyKind::HttpConnect | ProfileProxyKind::Socks5 => {
                route = route
                    .child(self.render_form_row(
                        self.tr("field-proxy-host").into(),
                        editor.proxy_host.clone(),
                    ))
                    .child(self.render_form_row(
                        self.tr("field-proxy-port").into(),
                        editor.proxy_port.clone(),
                    ))
                    .child(self.render_form_row(
                        self.tr("field-proxy-username").into(),
                        editor.proxy_username.clone(),
                    ))
                    .child(self.render_form_row(
                        self.tr("field-proxy-password").into(),
                        editor.proxy_password.clone(),
                    ));
            }
            ProfileProxyKind::ProxyCommand => {
                route = route.child(self.render_form_row(
                    self.tr("field-proxy-command").into(),
                    editor.proxy_command.clone(),
                ));
            }
        }
        if editor.proxy_kind != ProfileProxyKind::ProxyCommand {
            route = route.child(self.render_jump_host_editor(editor, cx));
        }
        route
    }

    pub(super) fn render_jump_host_editor(
        &self,
        editor: &ProfileEditor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = editor.jump_search.read(cx).text().trim().to_lowercase();
        let mut profiles = self
            .profiles
            .iter()
            .filter(|profile| profile.id != editor.profile_id)
            .filter(|profile| {
                query.is_empty()
                    || profile.name.to_lowercase().contains(&query)
                    || profile.host.to_lowercase().contains(&query)
            })
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| {
            let left_order = editor.jump_host_ids.iter().position(|id| id == &left.id);
            let right_order = editor.jump_host_ids.iter().position(|id| id == &right.id);
            left_order
                .cmp(&right_order)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        let mut rows = div()
            .id("profile-jump-host-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(180.0))
            .overflow_y_scroll();
        for profile in profiles {
            let id = profile.id.clone();
            let toggle_id = id.clone();
            let up_id = id.clone();
            let down_id = id.clone();
            let order = editor
                .jump_host_ids
                .iter()
                .position(|candidate| candidate == &id);
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(30.0))
                    .px_2()
                    .rounded_md()
                    .bg(self.theme.control_bg)
                    .child(
                        div()
                            .id(SharedString::from(format!("jump-toggle-{id}")))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(self.theme.border_strong)
                            .bg(if order.is_some() {
                                self.theme.accent
                            } else {
                                self.theme.transparent
                            })
                            .cursor_pointer()
                            .when(order.is_some(), |this| {
                                this.child(icon(
                                    IconName::Check,
                                    self.theme,
                                    IconTone::Default,
                                    12.0,
                                ))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_jump_host(toggle_id.clone(), cx);
                            })),
                    )
                    .child(div().flex_1().min_w(px(0.0)).truncate().child(match order {
                        Some(index) => format!("{}. {}", index + 1, profile.name),
                        None => profile.name.clone(),
                    }))
                    .when_some(order, |this, index| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("jump-up-{id}")))
                                .px_1()
                                .cursor_pointer()
                                .text_color(if index > 0 {
                                    self.theme.text_primary
                                } else {
                                    self.theme.text_faint
                                })
                                .child("↑")
                                .when(index > 0, |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_jump_host(up_id.clone(), -1, cx);
                                    }))
                                }),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("jump-down-{id}")))
                                .px_1()
                                .cursor_pointer()
                                .text_color(if index + 1 < editor.jump_host_ids.len() {
                                    self.theme.text_primary
                                } else {
                                    self.theme.text_faint
                                })
                                .child("↓")
                                .when(index + 1 < editor.jump_host_ids.len(), |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.move_jump_host(down_id.clone(), 1, cx);
                                    }))
                                }),
                        )
                    }),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .mt_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .w(px(PROFILE_FORM_LABEL_WIDTH))
                            .child(self.tr("profile-jump-hosts")),
                    )
                    .child(div().flex_1().child(editor.jump_search.clone())),
            )
            .child(rows)
    }
}

pub(super) const PROFILE_FORM_LABEL_WIDTH: f32 = 128.0;

pub(super) struct ProfileContextMenu {
    pub(super) profile_id: String,
    pub(super) position: gpui::Point<Pixels>,
}

#[derive(Clone)]
pub(super) struct ProfileEditor {
    pub(super) mode: ProfileEditorMode,
    pub(super) profile_id: String,
    pub(super) name: Entity<TextField>,
    pub(super) host: Entity<TextField>,
    pub(super) port: Entity<TextField>,
    pub(super) username: Entity<TextField>,
    pub(super) auth_kind: ProfileAuthKind,
    pub(super) private_key_path: Entity<TextField>,
    pub(super) proxy_kind: ProfileProxyKind,
    pub(super) proxy_host: Entity<TextField>,
    pub(super) proxy_port: Entity<TextField>,
    pub(super) proxy_username: Entity<TextField>,
    pub(super) proxy_password: Entity<TextField>,
    pub(super) proxy_command: Entity<TextField>,
    pub(super) jump_search: Entity<TextField>,
    pub(super) jump_host_ids: Vec<String>,
    pub(super) proxy_secret_loaded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileEditorMode {
    Create,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileAuthKind {
    None,
    Password,
    PrivateKey,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfileProxyKind {
    Direct,
    HttpConnect,
    Socks5,
    ProxyCommand,
}

impl ProfileProxyKind {
    const OPTIONS: [Self; 4] = [
        Self::Direct,
        Self::HttpConnect,
        Self::Socks5,
        Self::ProxyCommand,
    ];

    pub(super) fn from_config(config: Option<&ProxyConfig>) -> Self {
        match config {
            None => Self::Direct,
            Some(ProxyConfig::HttpConnect { .. }) => Self::HttpConnect,
            Some(ProxyConfig::Socks5 { .. }) => Self::Socks5,
            Some(ProxyConfig::ProxyCommand { .. }) => Self::ProxyCommand,
        }
    }

    const fn label_key(self) -> &'static str {
        match self {
            Self::Direct => "profile-proxy-direct",
            Self::HttpConnect => "profile-proxy-http",
            Self::Socks5 => "profile-proxy-socks5",
            Self::ProxyCommand => "profile-proxy-command",
        }
    }
}

impl ProfileAuthKind {
    pub(super) const OPTIONS: [(Self, &'static str); 4] = [
        (Self::None, "No Password"),
        (Self::Password, "Password"),
        (Self::PrivateKey, "Private Key"),
        (Self::Agent, "SSH Agent"),
    ];

    pub(super) fn from_config(config: &AuthConfig) -> Self {
        match config {
            AuthConfig::None => Self::None,
            AuthConfig::Password => Self::Password,
            AuthConfig::PrivateKey { .. } => Self::PrivateKey,
            AuthConfig::Agent => Self::Agent,
        }
    }

    pub(super) fn into_config(self, private_key_path: &str) -> Result<AuthConfig, &'static str> {
        match self {
            Self::None => Ok(AuthConfig::None),
            Self::Password => Ok(AuthConfig::Password),
            Self::PrivateKey => {
                let path = private_key_path.trim();
                if path.is_empty() {
                    return Err("profile-validation-private-key");
                }

                Ok(AuthConfig::PrivateKey {
                    path: PathBuf::from(path),
                })
            }
            Self::Agent => Ok(AuthConfig::Agent),
        }
    }
}

pub(super) fn profile_auth_label(auth: &AuthConfig, localizer: &Localizer) -> String {
    localizer.text(match auth {
        AuthConfig::None => "common-none",
        AuthConfig::Password => "credential-password",
        AuthConfig::PrivateKey { .. } => "profile-auth-private-key",
        AuthConfig::Agent => "profile-auth-agent",
    })
}

pub(super) const fn profile_auth_kind_key(kind: ProfileAuthKind) -> &'static str {
    match kind {
        ProfileAuthKind::None => "profile-auth-none",
        ProfileAuthKind::Password => "profile-auth-password",
        ProfileAuthKind::PrivateKey => "profile-auth-private-key",
        ProfileAuthKind::Agent => "profile-auth-agent",
    }
}

pub(super) fn credentials_invalidated_by_edit(
    profile: &ConnectionProfile,
    host: &str,
    port: u16,
    username: &str,
    auth: &AuthConfig,
) -> bool {
    profile.host != host
        || profile.port != port
        || profile.username != username
        || profile.auth != *auth
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_auth_kind_reflects_saved_configuration() {
        assert_eq!(
            ProfileAuthKind::from_config(&AuthConfig::None),
            ProfileAuthKind::None
        );
        assert_eq!(
            ProfileAuthKind::from_config(&AuthConfig::Password),
            ProfileAuthKind::Password
        );
        assert_eq!(
            ProfileAuthKind::from_config(&AuthConfig::PrivateKey {
                path: PathBuf::from("/tmp/id_ed25519"),
            }),
            ProfileAuthKind::PrivateKey
        );
        assert_eq!(
            ProfileAuthKind::from_config(&AuthConfig::Agent),
            ProfileAuthKind::Agent
        );
    }

    #[test]
    fn profile_auth_selector_exposes_every_supported_method() {
        assert_eq!(
            ProfileAuthKind::OPTIONS,
            [
                (ProfileAuthKind::None, "No Password"),
                (ProfileAuthKind::Password, "Password"),
                (ProfileAuthKind::PrivateKey, "Private Key"),
                (ProfileAuthKind::Agent, "SSH Agent"),
            ]
        );
        assert!(
            ProfileAuthKind::OPTIONS
                .iter()
                .all(|(kind, _)| profile_auth_kind_key(*kind).starts_with("profile-auth-"))
        );
    }

    #[test]
    fn private_key_authentication_requires_a_path() {
        assert_eq!(
            ProfileAuthKind::PrivateKey.into_config("   "),
            Err("profile-validation-private-key")
        );
    }

    #[test]
    fn private_key_authentication_trims_the_path() {
        assert_eq!(
            ProfileAuthKind::PrivateKey.into_config("  /Users/test/.ssh/id_ed25519  "),
            Ok(AuthConfig::PrivateKey {
                path: PathBuf::from("/Users/test/.ssh/id_ed25519"),
            })
        );
    }

    #[test]
    fn password_and_agent_authentication_do_not_use_the_key_path() {
        assert_eq!(
            ProfileAuthKind::None.into_config("ignored"),
            Ok(AuthConfig::None)
        );
        assert_eq!(
            ProfileAuthKind::Password.into_config("ignored"),
            Ok(AuthConfig::Password)
        );
        assert_eq!(
            ProfileAuthKind::Agent.into_config("ignored"),
            Ok(AuthConfig::Agent)
        );
    }

    #[test]
    fn profile_name_changes_keep_saved_credentials() {
        let profile = ConnectionProfile::new("server-1", "Old name", "host", 22, "user");

        assert!(!credentials_invalidated_by_edit(
            &profile,
            "host",
            22,
            "user",
            &AuthConfig::Password,
        ));
    }

    #[test]
    fn connection_identity_or_auth_changes_invalidate_saved_credentials() {
        let profile = ConnectionProfile::new("server-1", "Server", "old-host", 22, "user");

        assert!(credentials_invalidated_by_edit(
            &profile,
            "new-host",
            22,
            "user",
            &AuthConfig::Password,
        ));
        assert!(credentials_invalidated_by_edit(
            &profile,
            "old-host",
            22,
            "user",
            &AuthConfig::Agent,
        ));
    }
}
