use super::{
    ActiveTerminal, AuthConfig, AuthMethod, BaseDirs, CancelCredential, CancelHostKeyVerification,
    ConnectionEvent, ConnectionPlan, ConnectionProfile, ConnectionRoute, ConnectionStage,
    ConnectionStep, Context, CredentialKind, DiagnosticLevel, DiagnosticsGlobal, Entity,
    FontWeight, IntoElement, Localizer, ProfileAuthKind, ProxyConfig, PtySize, RemCmdApp,
    RemoteFileKind, RightSidebarView, RuntimeProxy, SIDEBAR_SFTP_REQUEST_ID_START, SecretString,
    SessionId, SessionMessage, SessionMessageArg, SessionState, SftpAvailability,
    SftpBrowserPlacement, SftpBrowserState, SftpOperation, ShellEvent, SshConnection, SshError,
    SshErrorKind, SshRuntime, SubmitCredential, TERMINAL_COLUMNS, TERMINAL_EVENT_BATCH_LIMIT,
    TERMINAL_ROWS, TerminalSession, TerminalTabView, TextButtonTone, TextField,
    UI_MONOSPACE_FONT_FAMILY, Window, delete_credential, delete_profile_credentials, div,
    load_credential, px, save_credential, save_profiles, sftp_browser_placement_for_request,
    text_button,
};
use gpui::Focusable;
use gpui::prelude::*;
use secrecy::ExposeSecret;
use std::{path::PathBuf, time::Instant};

impl RemCmdApp {
    pub(super) fn open_credential_prompt(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        kind: CredentialPromptKind,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        self.dismiss_credential_prompt(cx);

        let placeholder = match kind {
            CredentialPromptKind::Password => self.tr("credential-password"),
            CredentialPromptKind::PrivateKeyPassphrase { .. } => self.tr("credential-passphrase"),
            CredentialPromptKind::ProxyPassword => self.tr("field-proxy-password"),
        };
        let input = cx.new(|cx| TextField::new_secure(cx, placeholder));
        cx.observe(&input, |this, input, cx| {
            if let Some(prompt) = this.credential_prompt.as_mut()
                && prompt.input == input
                && prompt.error.take().is_some()
            {
                cx.notify();
            }
        })
        .detach();

        self.credential_prompt = Some(CredentialPrompt {
            session_id,
            profile_id,
            kind,
            input: input.clone(),
            remember: false,
            error,
        });
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = None;
        }
        cx.notify();

        input
    }

    pub(super) fn dismiss_credential_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.credential_prompt.take() {
            prompt.input.update(cx, |input, cx| input.clear(cx));
            if self.pane_for_session(prompt.session_id).is_none() {
                self.remove_session(prompt.session_id, cx);
            }
        }
    }

    pub(super) fn delete_stored_credentials(
        &mut self,
        profile_id: String,
        kind: Option<CredentialKind>,
        success_message_key: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        let runtime = cx.global::<SshRuntime>().handle();
        *self
            .credential_mutations_in_progress
            .entry(profile_id.clone())
            .or_default() += 1;
        if let Some(session) = self.session_for_profile_mut(&profile_id) {
            session.connection_message = Some(SessionMessage::localized("credential-updating"));
        }

        cx.spawn(async move |this, cx| {
            let deleted_profile_id = profile_id.clone();
            let result = runtime
                .spawn_blocking(move || match kind {
                    Some(kind) => delete_credential(&profile_id, kind),
                    None => delete_profile_credentials(&profile_id),
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                let remove_counter = this
                    .credential_mutations_in_progress
                    .get_mut(&deleted_profile_id)
                    .is_some_and(|count| {
                        *count -= 1;
                        *count == 0
                    });
                if remove_counter {
                    this.credential_mutations_in_progress
                        .remove(&deleted_profile_id);
                    if let Some(session) = this.session_for_profile_mut(&deleted_profile_id) {
                        session.connection_message = None;
                    }
                }

                match result {
                    Ok(Ok(())) => {
                        if let Some(message_key) = success_message_key
                            && remove_counter
                            && this.selected_profile_id.as_deref()
                                == Some(deleted_profile_id.as_str())
                            && let Some(session) = this.session_for_profile_mut(&deleted_profile_id)
                        {
                            session.connection_message =
                                Some(SessionMessage::localized(message_key));
                        }
                    }
                    Ok(Err(error)) => {
                        this.form_error = Some(error.to_string());
                    }
                    Err(error) => {
                        this.form_error = Some(format!(
                            "{}: {error}",
                            this.tr("credential-keychain-task-failed")
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn forget_selected_credential(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let kind = match editor.auth_kind {
            ProfileAuthKind::Password => CredentialKind::Password,
            ProfileAuthKind::PrivateKey => CredentialKind::PrivateKeyPassphrase,
            ProfileAuthKind::None | ProfileAuthKind::Agent => return,
        };

        self.form_error = None;
        self.delete_stored_credentials(
            editor.profile_id.clone(),
            Some(kind),
            Some("credential-removed"),
            cx,
        );
    }

    pub(super) fn open_terminal_for_current_target(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session().is_some_and(TerminalSession::is_local) {
            self.open_local_terminal(window, cx);
        } else {
            self.connect_selected_profile_in_new_session(window, cx);
        }
    }

    pub(super) fn connect_selected_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .unwrap_or_else(|| self.create_session_for_profile(&profile.id));
        self.connect_profile_in_session(session_id, profile, window, cx);
    }

    pub(super) fn connect_selected_profile_in_new_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };
        if self.credential_lookup_task.is_some()
            || self
                .credential_mutations_in_progress
                .contains_key(&profile.id)
        {
            return;
        }

        let session_id = self.create_session_for_profile(&profile.id);
        self.connect_profile_in_session(session_id, profile, window, cx);
    }

    pub(super) fn connect_profile_in_session(
        &mut self,
        session_id: SessionId,
        profile: ConnectionProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .session(session_id)
            .is_some_and(|session| session.connection_state.can_connect())
            || self.credential_lookup_task.is_some()
            || self.pending_connection.is_some()
            || self.pending_proxy_approval.contains(&session_id)
        {
            cx.notify();
            return;
        }
        if self
            .credential_mutations_in_progress
            .contains_key(&profile.id)
        {
            return;
        }

        if self.activate_session_in_window(session_id, window, cx) {
            self.begin_connection_preparation(session_id, profile, None, cx);
        }
    }

    pub(super) fn begin_connection_preparation(
        &mut self,
        session_id: SessionId,
        target_profile: ConnectionProfile,
        force_prompt: Option<(String, CredentialKind, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_credential_prompt(cx);
        let mut steps = Vec::with_capacity(target_profile.route.jump_host_ids.len() + 1);
        for jump_id in &target_profile.route.jump_host_ids {
            let Some(mut jump) = self
                .profiles
                .iter()
                .find(|profile| &profile.id == jump_id)
                .cloned()
            else {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(SessionMessage::localized_with_detail(
                        "connection-missing-jump",
                        jump_id,
                    ));
                }
                cx.notify();
                return;
            };
            jump.route = ConnectionRoute::default();
            steps.push(jump);
        }
        steps.push(target_profile.clone());
        let redactor = cx.global::<DiagnosticsGlobal>().0.redactor();
        for step in &steps {
            if let AuthConfig::PrivateKey { path } = &step.auth {
                redactor.register_text(&path.to_string_lossy());
                if let Ok(relative) = path.strip_prefix("~")
                    && let Some(base_dirs) = BaseDirs::new()
                {
                    redactor.register_text(&base_dirs.home_dir().join(relative).to_string_lossy());
                }
            }
        }
        self.pending_connection = Some(PendingConnectionPreparation {
            session_id,
            target_profile,
            steps,
            next_step: 0,
            prepared_steps: Vec::new(),
            credentials: Vec::new(),
            runtime_proxy: None,
            proxy_prepared: false,
            force_prompt,
        });
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(SessionMessage::localized("credential-checking"));
        }
        self.continue_connection_preparation(cx);
    }

    pub(super) fn continue_connection_preparation(&mut self, cx: &mut Context<Self>) {
        loop {
            let Some(mut pending) = self.pending_connection.take() else {
                return;
            };
            if self.session(pending.session_id).is_none() {
                return;
            }

            if !pending.proxy_prepared {
                let target_id = pending.target_profile.id.clone();
                match pending.target_profile.route.upstream_proxy.as_ref() {
                    None => {
                        pending.proxy_prepared = true;
                        self.pending_connection = Some(pending);
                        continue;
                    }
                    Some(ProxyConfig::HttpConnect {
                        host,
                        port,
                        username,
                    }) if username.is_none() => {
                        pending.runtime_proxy =
                            Some(RuntimeProxy::http_connect(host.clone(), *port, None, None));
                        pending.proxy_prepared = true;
                        self.pending_connection = Some(pending);
                        continue;
                    }
                    Some(ProxyConfig::Socks5 {
                        host,
                        port,
                        username,
                    }) if username.is_none() => {
                        pending.runtime_proxy =
                            Some(RuntimeProxy::socks5(host.clone(), *port, None, None));
                        pending.proxy_prepared = true;
                        self.pending_connection = Some(pending);
                        continue;
                    }
                    Some(ProxyConfig::HttpConnect { .. }) | Some(ProxyConfig::Socks5 { .. }) => {
                        let forced =
                            pending
                                .force_prompt
                                .as_ref()
                                .is_some_and(|(profile_id, kind, _)| {
                                    profile_id == &target_id
                                        && *kind == CredentialKind::ProxyPassword
                                });
                        let prompt_error = forced
                            .then(|| pending.force_prompt.take().and_then(|(_, _, error)| error))
                            .flatten();
                        let prompt_session_id = pending.session_id;
                        self.pending_connection = Some(pending);
                        if forced {
                            self.open_credential_prompt(
                                prompt_session_id,
                                target_id,
                                CredentialPromptKind::ProxyPassword,
                                prompt_error,
                                cx,
                            );
                        } else {
                            self.lookup_preparation_credential(
                                target_id,
                                CredentialKind::ProxyPassword,
                                Some(CredentialPromptKind::ProxyPassword),
                                cx,
                            );
                        }
                        return;
                    }
                    Some(ProxyConfig::ProxyCommand { .. }) => {
                        self.pending_connection = Some(pending);
                        self.lookup_preparation_credential(
                            target_id,
                            CredentialKind::ProxyCommand,
                            None,
                            cx,
                        );
                        return;
                    }
                }
            }

            if pending.next_step < pending.steps.len() {
                let step = pending.steps[pending.next_step].clone();
                let profile_id = step.id.clone();
                match &step.auth {
                    AuthConfig::None => {
                        pending
                            .prepared_steps
                            .push(ConnectionStep::new(step, AuthMethod::None));
                        pending.next_step += 1;
                        self.pending_connection = Some(pending);
                        continue;
                    }
                    AuthConfig::Agent => {
                        pending
                            .prepared_steps
                            .push(ConnectionStep::new(step, AuthMethod::Agent));
                        pending.next_step += 1;
                        self.pending_connection = Some(pending);
                        continue;
                    }
                    AuthConfig::Password => {
                        let kind = CredentialKind::Password;
                        let forced = pending.force_prompt.as_ref().is_some_and(
                            |(forced_profile_id, forced_kind, _)| {
                                forced_profile_id == &profile_id && *forced_kind == kind
                            },
                        );
                        let prompt_error = forced
                            .then(|| pending.force_prompt.take().and_then(|(_, _, error)| error))
                            .flatten();
                        let prompt_session_id = pending.session_id;
                        self.pending_connection = Some(pending);
                        if forced {
                            self.open_credential_prompt(
                                prompt_session_id,
                                profile_id,
                                CredentialPromptKind::Password,
                                prompt_error,
                                cx,
                            );
                        } else {
                            self.lookup_preparation_credential(
                                profile_id,
                                kind,
                                Some(CredentialPromptKind::Password),
                                cx,
                            );
                        }
                        return;
                    }
                    AuthConfig::PrivateKey { path } => {
                        let kind = CredentialKind::PrivateKeyPassphrase;
                        let prompt =
                            CredentialPromptKind::PrivateKeyPassphrase { path: path.clone() };
                        let forced = pending.force_prompt.as_ref().is_some_and(
                            |(forced_profile_id, forced_kind, _)| {
                                forced_profile_id == &profile_id && *forced_kind == kind
                            },
                        );
                        let prompt_error = forced
                            .then(|| pending.force_prompt.take().and_then(|(_, _, error)| error))
                            .flatten();
                        let prompt_session_id = pending.session_id;
                        self.pending_connection = Some(pending);
                        if forced {
                            self.open_credential_prompt(
                                prompt_session_id,
                                profile_id,
                                prompt,
                                prompt_error,
                                cx,
                            );
                        } else {
                            self.lookup_preparation_credential(profile_id, kind, Some(prompt), cx);
                        }
                        return;
                    }
                }
            }

            let target_step = pending
                .prepared_steps
                .pop()
                .expect("connection preparation includes the target step");
            let mut plan = ConnectionPlan::new(target_step);
            for jump in pending.prepared_steps {
                plan.push_jump(jump);
            }
            if let Some(proxy) = pending.runtime_proxy {
                plan.set_proxy(proxy);
            }
            if let Err(error) = plan.validate() {
                if let Some(session) = self.session_mut(pending.session_id) {
                    session.connection_error = Some(SessionMessage::connection_error(&error));
                    session.connection_message = None;
                }
                cx.notify();
                return;
            }
            self.finish_connection_preparation(
                pending.session_id,
                pending.target_profile,
                plan,
                pending.credentials,
                cx,
            );
            return;
        }
    }

    pub(super) fn lookup_preparation_credential(
        &mut self,
        profile_id: String,
        credential_kind: CredentialKind,
        prompt_kind: Option<CredentialPromptKind>,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self
            .pending_connection
            .as_ref()
            .map(|pending| pending.session_id)
        else {
            return;
        };
        let runtime = cx.global::<SshRuntime>().handle();
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(SessionMessage::localized("credential-checking"));
        }
        self.credential_lookup_session_id = Some(session_id);
        self.credential_lookup_task = Some(cx.spawn(async move |this, cx| {
            let lookup_profile_id = profile_id.clone();
            let result = runtime
                .spawn_blocking(move || load_credential(&lookup_profile_id, credential_kind))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.credential_lookup_task = None;
                this.credential_lookup_session_id = None;
                if this
                    .pending_connection
                    .as_ref()
                    .is_none_or(|pending| pending.session_id != session_id)
                {
                    return;
                }
                let loaded = match result {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                match loaded {
                    Ok(Some(secret)) => {
                        let credential = ConnectionCredential::from_keychain(
                            profile_id.clone(),
                            credential_kind,
                        );
                        this.accept_preparation_secret(
                            profile_id,
                            credential_kind,
                            prompt_kind,
                            secret,
                            credential,
                            cx,
                        );
                    }
                    Ok(None) => this.handle_missing_preparation_secret(
                        profile_id,
                        credential_kind,
                        prompt_kind,
                        None,
                        cx,
                    ),
                    Err(error) => this.handle_missing_preparation_secret(
                        profile_id,
                        credential_kind,
                        prompt_kind,
                        Some(error),
                        cx,
                    ),
                }
            });
        }));
        cx.notify();
    }

    pub(super) fn handle_missing_preparation_secret(
        &mut self,
        profile_id: String,
        credential_kind: CredentialKind,
        prompt_kind: Option<CredentialPromptKind>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match (credential_kind, prompt_kind) {
            (
                CredentialKind::PrivateKeyPassphrase,
                Some(CredentialPromptKind::PrivateKeyPassphrase { path }),
            ) if error.is_none() => {
                self.accept_preparation_auth(
                    profile_id,
                    AuthMethod::PrivateKey {
                        path,
                        passphrase: None,
                    },
                    None,
                    cx,
                );
            }
            (CredentialKind::ProxyCommand, None) => {
                let message = error.map_or_else(
                    || SessionMessage::localized("proxy-command-keychain-missing"),
                    SessionMessage::from,
                );
                let session_id = self
                    .pending_connection
                    .take()
                    .map(|pending| pending.session_id);
                if let Some(session_id) = session_id
                    && let Some(session) = self.session_mut(session_id)
                {
                    session.connection_error = Some(message);
                    session.connection_message = None;
                }
                cx.notify();
            }
            (_, Some(prompt_kind)) => {
                if let Some(session_id) = self
                    .pending_connection
                    .as_ref()
                    .map(|pending| pending.session_id)
                {
                    self.open_credential_prompt(session_id, profile_id, prompt_kind, error, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn accept_preparation_secret(
        &mut self,
        profile_id: String,
        credential_kind: CredentialKind,
        prompt_kind: Option<CredentialPromptKind>,
        secret: SecretString,
        credential: ConnectionCredential,
        cx: &mut Context<Self>,
    ) {
        cx.global::<DiagnosticsGlobal>().0.register_secret(&secret);
        if credential_kind == CredentialKind::ProxyCommand {
            let Some(pending) = self.pending_connection.as_mut() else {
                return;
            };
            let Some(ProxyConfig::ProxyCommand {
                approved_digest, ..
            }) = pending.target_profile.route.upstream_proxy.as_ref()
            else {
                return;
            };
            pending.runtime_proxy =
                Some(RuntimeProxy::proxy_command(secret, approved_digest.clone()));
            pending.proxy_prepared = true;
            self.continue_connection_preparation(cx);
            return;
        }
        let Some(prompt_kind) = prompt_kind else {
            return;
        };
        if prompt_kind == CredentialPromptKind::ProxyPassword {
            let Some(pending) = self.pending_connection.as_mut() else {
                return;
            };
            pending.runtime_proxy = runtime_proxy_with_password(&pending.target_profile, secret);
            pending.proxy_prepared = true;
            pending.credentials.push(credential);
            self.continue_connection_preparation(cx);
            return;
        }
        self.accept_preparation_auth(
            profile_id,
            auth_method_with_secret(prompt_kind, secret),
            Some(credential),
            cx,
        );
    }

    pub(super) fn accept_preparation_auth(
        &mut self,
        profile_id: String,
        auth: AuthMethod,
        credential: Option<ConnectionCredential>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_connection.as_mut() else {
            return;
        };
        let Some(step) = pending.steps.get(pending.next_step).cloned() else {
            return;
        };
        if step.id != profile_id {
            return;
        }
        pending.prepared_steps.push(ConnectionStep::new(step, auth));
        pending.next_step += 1;
        if let Some(credential) = credential {
            pending.credentials.push(credential);
        }
        self.continue_connection_preparation(cx);
    }

    pub(super) fn finish_connection_preparation(
        &mut self,
        session_id: SessionId,
        target_profile: ConnectionProfile,
        plan: ConnectionPlan,
        credentials: Vec<ConnectionCredential>,
        cx: &mut Context<Self>,
    ) {
        match plan.proxy_command_preview() {
            Ok(Some(preview)) if !preview.is_approved() => {
                self.pending_proxy_approval.insert(session_id);
                self.proxy_command_approval_prompt = Some(ProxyCommandApprovalPrompt {
                    session_id,
                    target_profile,
                    plan,
                    credentials,
                    expanded_command: preview.expanded_command().clone(),
                    approval_digest: preview.approval_digest().to_owned(),
                });
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message =
                        Some(SessionMessage::localized("proxy-command-approval-title"));
                }
                cx.notify();
            }
            Ok(_) => self.start_connection_plan(session_id, target_profile, plan, credentials, cx),
            Err(error) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(SessionMessage::connection_error(&error));
                    session.connection_message = None;
                }
                cx.notify();
            }
        }
    }

    pub(super) fn approve_proxy_command(&mut self, cx: &mut Context<Self>) {
        let Some(mut prompt) = self.proxy_command_approval_prompt.take() else {
            return;
        };
        self.pending_proxy_approval.remove(&prompt.session_id);
        if let Err(error) = prompt
            .plan
            .approve_proxy_command(prompt.approval_digest.clone())
        {
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(SessionMessage::connection_error(&error));
            }
            cx.notify();
            return;
        }
        let profile_index = self
            .profiles
            .iter()
            .position(|profile| profile.id == prompt.target_profile.id);
        let approval_update = profile_index.and_then(|index| {
            let profile = &mut self.profiles[index];
            if let Some(ProxyConfig::ProxyCommand {
                approved_digest, ..
            }) = profile.route.upstream_proxy.as_mut()
            {
                let previous = approved_digest.clone();
                *approved_digest = Some(prompt.approval_digest.clone());
                Some((index, previous))
            } else {
                None
            }
        });
        let Some((profile_index, previous_approval)) = approval_update else {
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(SessionMessage::localized(
                    "profile-validation-proxy-command",
                ));
            }
            cx.notify();
            return;
        };
        if let Err(error) = save_profiles(&self.profiles_path, &self.profiles) {
            if let Some(ProxyConfig::ProxyCommand {
                approved_digest, ..
            }) = self.profiles[profile_index].route.upstream_proxy.as_mut()
            {
                *approved_digest = previous_approval;
            }
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(SessionMessage::localized_with_detail(
                    "app-save-profiles-failed",
                    error.to_string(),
                ));
            }
            cx.notify();
            return;
        }
        self.start_connection_plan(
            prompt.session_id,
            prompt.target_profile,
            prompt.plan,
            prompt.credentials,
            cx,
        );
    }

    pub(super) fn cancel_proxy_command_approval(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.proxy_command_approval_prompt.take() else {
            return;
        };
        self.pending_proxy_approval.remove(&prompt.session_id);
        if let Some(session) = self.session_mut(prompt.session_id) {
            session.connection_error = Some(SessionMessage::localized(
                "proxy-command-approval-cancelled",
            ));
            session.connection_message = None;
        }
        cx.notify();
    }

    pub(super) fn start_connection_plan(
        &mut self,
        session_id: SessionId,
        profile: ConnectionProfile,
        plan: ConnectionPlan,
        credentials: Vec<ConnectionCredential>,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_credential_prompt(cx);
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.host_key_prompt = None;
        self.credential_lookup_task = None;
        self.credential_lookup_session_id = None;

        let runtime = cx.global::<SshRuntime>().handle();
        let pty_size = PtySize::new(TERMINAL_COLUMNS, TERMINAL_ROWS);
        let connection = SshConnection::spawn_plan_with_transfer_rate_limiter(
            &runtime,
            plan,
            pty_size,
            self.transfer_rate_limiter.clone(),
        );
        let (handle, mut events) = connection.split();

        let session = self
            .session_mut(session_id)
            .expect("session should exist while starting a connection");
        session.close_when_disconnected = false;
        session.connection_state = SessionState::Connecting;
        session.connection_handle = Some(handle);
        session.local_terminal_handle = None;
        session.connection_credentials = credentials;
        session.connection_error = None;
        session.connection_message = None;
        session.terminal_end_reason = None;
        session.terminal = Some(ActiveTerminal::new(profile.id.clone(), pty_size));
        session.terminal_marked_text.clear();
        session.terminal_selection = None;
        session.terminal_selecting = false;
        session.terminal_scroll_accumulator = 0.0;
        session.terminal_resize_task = None;
        session.sftp = SftpBrowserState::default();
        session.sidebar_sftp =
            SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START);
        session.sftp_availability = SftpAvailability::Checking;
        cx.global::<DiagnosticsGlobal>().0.record(
            DiagnosticLevel::Info,
            "ssh.connection",
            "SSH connection started",
            [
                (
                    "jump_count".into(),
                    profile.route.jump_host_ids.len().to_string(),
                ),
                (
                    "proxy".into(),
                    profile.route.upstream_proxy.is_some().to_string(),
                ),
            ],
        );

        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next_event().await {
                let mut batch = Vec::with_capacity(TERMINAL_EVENT_BATCH_LIMIT);
                batch.push(event);
                while batch.len() < TERMINAL_EVENT_BATCH_LIMIT {
                    let Some(event) = events.try_next_event() else {
                        break;
                    };
                    batch.push(event);
                }
                if this
                    .update(cx, move |this, cx| {
                        for event in batch {
                            this.handle_connection_event(session_id, event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        cx.notify();
    }

    pub(super) fn submit_credential_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.credential_prompt.as_ref() else {
            return;
        };

        if prompt.input.read(cx).is_empty() {
            let input = prompt.input.clone();
            let label = match prompt.kind {
                CredentialPromptKind::Password => self.tr("credential-password"),
                CredentialPromptKind::PrivateKeyPassphrase { .. } => {
                    self.tr("credential-passphrase")
                }
                CredentialPromptKind::ProxyPassword => self.tr("field-proxy-password"),
            };
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("label", label);
            let required = self.tr_with("credential-required", &args);
            if let Some(prompt) = self.credential_prompt.as_mut() {
                prompt.error = Some(required);
            }
            window.focus(&input.focus_handle(cx));
            cx.notify();
            return;
        }

        let profile_id = prompt.profile_id.clone();
        let session_id = prompt.session_id;
        let kind = prompt.kind.clone();
        let remember = prompt.remember;
        let input = prompt.input.clone();

        if self
            .pending_connection
            .as_ref()
            .is_none_or(|pending| pending.session_id != session_id)
        {
            self.dismiss_credential_prompt(cx);
            if let Some(session) = self.session_mut(session_id) {
                session.connection_error =
                    Some(SessionMessage::localized("connection-preparation-expired"));
            }
            cx.notify();
            return;
        }

        let secret = SecretString::new(
            input
                .update(cx, |input, cx| input.take_text(cx))
                .into_boxed_str(),
        );
        self.credential_prompt = None;

        let credential_kind = kind.credential_kind();
        let save_on_success = remember.then(|| secret.clone());
        let credential =
            ConnectionCredential::from_prompt(profile_id.clone(), credential_kind, save_on_success);
        self.accept_preparation_secret(
            profile_id,
            credential_kind,
            Some(kind),
            secret,
            credential,
            cx,
        );
    }

    pub(super) fn on_submit_credential(
        &mut self,
        _: &SubmitCredential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_credential_prompt(window, cx);
    }

    pub(super) fn on_cancel_credential(
        &mut self,
        _: &CancelCredential,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_connection = None;
        self.dismiss_credential_prompt(cx);
        cx.notify();
    }

    pub(super) fn trust_pending_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let Some((info, handle)) = self.session_mut(session_id).and_then(|session| {
            let info = session.host_key_prompt.take()?;
            match session.connection_handle.clone() {
                Some(handle) => Some((info, handle)),
                None => {
                    session.connection_error =
                        Some(SessionMessage::localized("connection-handle-missing"));
                    None
                }
            }
        }) else {
            cx.notify();
            return;
        };

        match handle.trust_host_key() {
            Ok(()) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(SessionMessage::localized_with(
                        "connection-trusting-host",
                        [("address", info.address())],
                    ));
                }
            }
            Err(error) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(SessionMessage::connection_error(&error));
                }
            }
        }
        cx.notify();
    }

    pub(super) fn reject_pending_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_mut() else {
            return;
        };
        if session.host_key_prompt.take().is_none() {
            return;
        }

        if let Some(handle) = session.connection_handle.as_ref()
            && let Err(error) = handle.reject_host_key()
        {
            session.connection_error = Some(error.to_string().into());
        }
        session.connection_message = None;
        cx.notify();
    }

    pub(super) fn on_cancel_host_key_verification(
        &mut self,
        _: &CancelHostKeyVerification,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reject_pending_host_key(cx);
    }

    pub(super) fn prompt_for_private_key_passphrase(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_connection_with_prompt(
            session_id,
            profile_id,
            CredentialKind::PrivateKeyPassphrase,
            error,
            cx,
        )
    }

    pub(super) fn prompt_for_password(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_connection_with_prompt(
            session_id,
            profile_id,
            CredentialKind::Password,
            error,
            cx,
        )
    }

    pub(super) fn retry_connection_with_prompt(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        kind: CredentialKind,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target_profile) = self
            .session(session_id)
            .and_then(|session| {
                self.profiles
                    .iter()
                    .find(|profile| profile.id == session.profile_id)
            })
            .cloned()
        else {
            return false;
        };
        let valid_step = if kind == CredentialKind::ProxyPassword {
            matches!(
                target_profile.route.upstream_proxy,
                Some(ProxyConfig::HttpConnect {
                    username: Some(_),
                    ..
                }) | Some(ProxyConfig::Socks5 {
                    username: Some(_),
                    ..
                })
            ) && profile_id == target_profile.id
        } else {
            self.profiles.iter().any(|profile| {
                profile.id == profile_id
                    && matches!(
                        (&profile.auth, kind),
                        (AuthConfig::Password, CredentialKind::Password)
                            | (
                                AuthConfig::PrivateKey { .. },
                                CredentialKind::PrivateKeyPassphrase
                            )
                    )
            })
        };
        if !valid_step {
            return false;
        }
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(error.clone().into());
        }
        self.begin_connection_preparation(
            session_id,
            target_profile,
            Some((profile_id, kind, Some(error))),
            cx,
        );
        true
    }

    pub(super) fn remove_rejected_credential_then_prompt(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        kind: CredentialKind,
        authentication_error: String,
        cx: &mut Context<Self>,
    ) {
        let runtime = cx.global::<SshRuntime>().handle();
        if let Some(session) = self.session_mut(session_id) {
            session.connection_message =
                Some(SessionMessage::localized("credential-removing-rejected"));
        }

        self.credential_lookup_session_id = Some(session_id);
        self.credential_lookup_task = Some(cx.spawn(async move |this, cx| {
            let delete_profile_id = profile_id.clone();
            let result = runtime
                .spawn_blocking(move || delete_credential(&delete_profile_id, kind))
                .await;

            let _ = this.update(cx, |this, cx| {
                this.credential_lookup_task = None;
                this.credential_lookup_session_id = None;
                if this.session(session_id).is_none() {
                    return;
                }

                let error = match result {
                    Ok(Ok(())) => authentication_error,
                    Ok(Err(error)) => format!("{authentication_error}\n{error}"),
                    Err(error) => format!(
                        "{authentication_error}\n{}: {error}",
                        this.tr("credential-keychain-task-failed")
                    ),
                };

                if kind != CredentialKind::ProxyCommand
                    && !this.retry_connection_with_prompt(
                        session_id,
                        profile_id,
                        kind,
                        error.clone(),
                        cx,
                    )
                    && let Some(session) = this.session_mut(session_id)
                {
                    session.connection_error = Some(error.into());
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn save_successful_credentials(
        &mut self,
        session_id: SessionId,
        profile_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let credentials = std::mem::take(&mut session.connection_credentials);
        let (successful, remaining): (Vec<_>, Vec<_>) =
            credentials.into_iter().partition(|credential| {
                credential.profile_id == profile_id
                    || credential.kind == CredentialKind::ProxyPassword
            });
        session.connection_credentials = remaining;
        let runtime = cx.global::<SshRuntime>().handle();
        for credential in successful {
            let Some(secret) = credential.save_on_success else {
                continue;
            };
            let profile_id = credential.profile_id;
            let kind = credential.kind;
            let runtime = runtime.clone();
            cx.spawn(async move |this, cx| {
                let result = runtime
                    .spawn_blocking(move || save_credential(&profile_id, kind, &secret))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(session) = this.session_mut(session_id) {
                        session.connection_message = Some(match result {
                            Ok(Ok(())) => SessionMessage::localized("credential-saved"),
                            Ok(Err(error)) => SessionMessage::localized_with_detail(
                                "credential-save-failed",
                                error.to_string(),
                            ),
                            Err(error) => SessionMessage::localized_with_detail(
                                "credential-save-task-failed",
                                error.to_string(),
                            ),
                        });
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(super) fn disconnect_active_connection(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        self.disconnect_session(session_id, cx);
    }

    pub(super) fn disconnect_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let should_remove = {
            let Some(session) = self.session_mut(session_id) else {
                return;
            };
            if !session.connection_state.can_disconnect() {
                return;
            }

            session.terminal_resize_task = None;

            if let Err(error) = session.disconnect_terminal() {
                session.connection_state = SessionState::Failed;
                session.connection_handle = None;
                session.local_terminal_handle = None;
                session.connection_error = Some(error.into());
                session.close_when_disconnected
            } else {
                // Disable repeated clicks before the worker publishes its event.
                session.connection_state = SessionState::Disconnecting;
                session.terminal_end_reason =
                    Some(SessionMessage::localized("terminal-session-disconnected"));
                false
            }
        };

        if should_remove {
            self.remove_session(session_id, cx);
        }

        cx.notify();
    }

    pub(super) fn handle_connection_event(
        &mut self,
        session_id: SessionId,
        event: ConnectionEvent,
        cx: &mut Context<Self>,
    ) {
        if self.session(session_id).is_none() {
            return;
        }
        self.record_connection_diagnostic(&event, cx);

        let should_notify = match event {
            ConnectionEvent::StateChanged(state) => {
                let connection_closed = self.tr("sftp-connection-closed");
                let close_when_disconnected = {
                    let session = self
                        .session_mut(session_id)
                        .expect("checked session should still exist");
                    let previous_state = session.connection_state;
                    session.connection_state = state;

                    if matches!(
                        state,
                        SessionState::Authenticating | SessionState::Connected
                    ) {
                        session.host_key_prompt = None;
                        session.connection_message = None;
                    }

                    if state == SessionState::Connected
                        && let Some(terminal) = session.terminal.as_mut()
                    {
                        terminal.was_connected = true;
                    }

                    if state == SessionState::Disconnected {
                        session.host_key_prompt = None;
                        session.terminal_resize_task = None;
                        session.connection_handle = None;
                        session.connection_credentials.clear();
                        session.sftp_availability = SftpAvailability::Checking;
                        session.sftp.stop_loading();
                        session.sidebar_sftp.stop_loading();
                        session.transfers.fail_pending(&connection_closed);
                        session.performance.clear_connection();
                        if previous_state == SessionState::Disconnecting
                            && session.terminal_end_reason.is_none()
                        {
                            session.terminal_end_reason =
                                Some(SessionMessage::localized("terminal-session-disconnected"));
                        }
                    }

                    state == SessionState::Disconnected && session.close_when_disconnected
                };

                if close_when_disconnected {
                    self.remove_session(session_id, cx);
                }
                self.sync_performance_monitoring();

                true
            }
            ConnectionEvent::ConnectionStageChanged(stage) => {
                let message = match stage {
                    ConnectionStage::Proxy => {
                        SessionMessage::localized("connection-connecting-proxy")
                    }
                    ConnectionStage::Jump { index, total, .. } => SessionMessage::localized_with(
                        "connection-connecting-jump",
                        [("index", index.to_string()), ("total", total.to_string())],
                    ),
                    ConnectionStage::Target { .. } => {
                        SessionMessage::localized("connection-connecting-target")
                    }
                };
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message);
                }
                true
            }
            ConnectionEvent::AuthenticationSucceeded { stage, .. } => {
                if let Some(profile_id) = stage.profile_id() {
                    self.save_successful_credentials(session_id, profile_id, cx);
                }
                true
            }
            ConnectionEvent::HostKeyVerificationRequired { stage, info } => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message =
                        Some(SessionMessage::host_key_verification(stage, info.address()));
                    session.host_key_prompt = Some(info);
                }
                self.activate_session(session_id, cx);
                true
            }
            ConnectionEvent::Failed(error) => {
                let connection_failed = self.tr("sftp-connection-failed");
                let stage_profile_id = error
                    .stage()
                    .and_then(ConnectionStage::profile_id)
                    .map(str::to_owned);
                let rejected_kind = rejected_credential_kind(
                    error.kind(),
                    stage_profile_id.as_deref().and_then(|profile_id| {
                        self.profiles
                            .iter()
                            .find(|profile| profile.id == profile_id)
                            .map(|profile| &profile.auth)
                    }),
                );
                let (failed_profile_id, failed_credential, close_when_disconnected) = {
                    let session = self
                        .session_mut(session_id)
                        .expect("checked session should still exist");
                    let profile_id = stage_profile_id
                        .clone()
                        .unwrap_or_else(|| session.profile_id.clone());
                    let credential_index =
                        session
                            .connection_credentials
                            .iter()
                            .position(|credential| {
                                rejected_kind == Some(credential.kind)
                                    && credential.profile_id == profile_id
                            });
                    let credential =
                        credential_index.map(|index| session.connection_credentials.remove(index));
                    // No credential can still succeed after the connection attempt has failed.
                    // Drop every remaining runtime secret before presenting a retry action.
                    session.connection_credentials.clear();
                    session.connection_state = SessionState::Failed;
                    session.terminal_resize_task = None;
                    session.connection_handle = None;
                    session.host_key_prompt = None;
                    session.sftp.stop_loading();
                    session.sidebar_sftp.stop_loading();
                    session.transfers.fail_pending(&connection_failed);
                    session.performance.clear_connection();
                    (profile_id, credential, session.close_when_disconnected)
                };

                if close_when_disconnected {
                    self.remove_session(session_id, cx);
                    return;
                }
                self.sync_performance_monitoring();

                let authentication_error = localized_connection_error(&error, &self.localizer);
                let prompted_for_credential =
                    match (failed_profile_id, failed_credential, error.kind()) {
                        (
                            profile_id,
                            Some(credential),
                            SshErrorKind::Authentication
                            | SshErrorKind::PrivateKeyPassphrase
                            | SshErrorKind::ProxyAuthentication,
                        ) if credential.profile_id == profile_id => {
                            if credential.source == CredentialSource::SystemKeychain {
                                self.remove_rejected_credential_then_prompt(
                                    session_id,
                                    profile_id,
                                    credential.kind,
                                    authentication_error,
                                    cx,
                                );
                                true
                            } else {
                                match credential.kind {
                                    CredentialKind::Password => self.prompt_for_password(
                                        session_id,
                                        profile_id,
                                        authentication_error,
                                        cx,
                                    ),
                                    CredentialKind::PrivateKeyPassphrase => self
                                        .prompt_for_private_key_passphrase(
                                            session_id,
                                            profile_id,
                                            authentication_error,
                                            cx,
                                        ),
                                    CredentialKind::ProxyPassword => self
                                        .retry_connection_with_prompt(
                                            session_id,
                                            profile_id,
                                            CredentialKind::ProxyPassword,
                                            authentication_error,
                                            cx,
                                        ),
                                    CredentialKind::ProxyCommand => false,
                                }
                            }
                        }
                        (profile_id, None, SshErrorKind::PrivateKeyPassphrase) => self
                            .prompt_for_private_key_passphrase(
                                session_id,
                                profile_id,
                                authentication_error,
                                cx,
                            ),
                        (profile_id, None, SshErrorKind::ProxyAuthentication) => self
                            .retry_connection_with_prompt(
                                session_id,
                                profile_id,
                                CredentialKind::ProxyPassword,
                                authentication_error,
                                cx,
                            ),
                        _ => false,
                    };

                if !prompted_for_credential && let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(SessionMessage::connection_error(&error));
                }
                true
            }
            ConnectionEvent::DirectoryRead {
                request_id,
                directory,
            } => {
                if let Some(session) = self.session_mut(session_id) {
                    let placement = sftp_browser_placement_for_request(request_id);
                    session
                        .sftp_browser_mut(placement)
                        .complete_request(request_id, directory);
                }
                true
            }
            ConnectionEvent::DirectoryTreeRead { request_id, tree } => {
                self.complete_directory_tree_download(session_id, request_id, tree, cx);
                true
            }
            ConnectionEvent::FileRead { request_id, file } => {
                self.complete_remote_file_read(session_id, request_id, file, cx);
                true
            }
            ConnectionEvent::FileWritten { request_id, file } => {
                self.complete_remote_file_write(session_id, request_id, file);
                true
            }
            ConnectionEvent::PathCreated {
                request_id,
                path,
                kind,
            } => {
                let placement = sftp_browser_placement_for_request(request_id);
                self.refresh_sftp_directory_for_session(session_id, placement, cx);
                if kind == RemoteFileKind::File && self.active_session_id == Some(session_id) {
                    self.open_remote_file(path, true, cx);
                }
                true
            }
            ConnectionEvent::DirectoriesCreated {
                request_id,
                paths: _,
            } => {
                let placement = sftp_browser_placement_for_request(request_id);
                self.refresh_sftp_directory_for_session(session_id, placement, cx);
                true
            }
            ConnectionEvent::PathsDeleted { request_id, paths } => {
                let placement = sftp_browser_placement_for_request(request_id);
                if let Some(session) = self.session_mut(session_id) {
                    session.sftp.remove_paths(&paths);
                    session.sidebar_sftp.remove_paths(&paths);
                }
                self.refresh_sftp_directory_for_session(session_id, placement, cx);
                true
            }
            ConnectionEvent::TransferProgress {
                transfer_id,
                transferred,
                total,
            } => {
                if let Some(session) = self.session_mut(session_id) {
                    session
                        .transfers
                        .mark_progress(transfer_id, transferred, total);
                }
                true
            }
            ConnectionEvent::TransferConflict {
                transfer_id,
                direction: _,
                path: _,
            } => {
                if let Some(session) = self.session_mut(session_id) {
                    session.transfers.mark_conflict(transfer_id);
                }
                self.start_queued_sftp_transfers(cx);
                true
            }
            ConnectionEvent::TransferCompleted {
                transfer_id,
                direction,
                path,
                bytes,
            } => {
                self.complete_sftp_transfer(session_id, transfer_id, direction, path, bytes, cx);
                true
            }
            ConnectionEvent::TransferCancelled { transfer_id } => {
                if let Some(session) = self.session_mut(session_id) {
                    session.transfers.mark_cancelled(transfer_id);
                }
                self.start_queued_sftp_transfers(cx);
                true
            }
            ConnectionEvent::SftpAvailabilityChanged {
                available,
                scp_available,
                message,
            } => {
                let unavailable_message = message.map_or_else(
                    || self.tr("sftp-server-unavailable"),
                    |details| {
                        format!(
                            "{}\n{}: {details}",
                            self.tr("sftp-server-unavailable"),
                            self.tr("connection-technical-details")
                        )
                    },
                );
                if let Some(session) = self.session_mut(session_id) {
                    session.sftp_availability = if available {
                        SftpAvailability::Available
                    } else if scp_available {
                        SftpAvailability::ScpOnly(unavailable_message)
                    } else {
                        SftpAvailability::Unavailable(unavailable_message)
                    };
                    if !available {
                        session.sftp.stop_loading();
                        session.sidebar_sftp.stop_loading();
                    }
                }

                if available && self.active_session_id == Some(session_id) {
                    if self.right_sidebar_open && self.right_sidebar_view == RightSidebarView::Sftp
                    {
                        self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
                    }
                    if self.active_tab_view() == TerminalTabView::Files {
                        self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Center, cx);
                    }
                }
                true
            }
            ConnectionEvent::SftpFailed {
                request_id,
                path: _,
                operation,
                error,
            } => {
                let error_message = format!(
                    "{}\n{}: {}",
                    self.tr("sftp-failed"),
                    self.tr("connection-technical-details"),
                    error.message()
                );
                let transfer_failed = match operation {
                    SftpOperation::ReadDirectory | SftpOperation::ReadDirectoryTree => {
                        let placement = sftp_browser_placement_for_request(request_id);
                        self.fail_sftp_request(
                            session_id,
                            placement,
                            request_id,
                            error_message,
                            cx,
                        );
                        false
                    }
                    SftpOperation::ReadFile | SftpOperation::WriteFile => {
                        if let Some(session) = self.session_mut(session_id) {
                            session
                                .sftp
                                .fail_file_request(request_id, operation, error_message);
                        }
                        false
                    }
                    SftpOperation::CreateFile
                    | SftpOperation::CreateDirectory
                    | SftpOperation::DeletePaths => {
                        let placement = sftp_browser_placement_for_request(request_id);
                        self.show_sftp_error(session_id, placement, error_message, cx);
                        false
                    }
                    SftpOperation::UploadFile
                    | SftpOperation::DownloadFile
                    | SftpOperation::CancelTransfer => {
                        if let Some(session) = self.session_mut(session_id) {
                            session.transfers.mark_failed(request_id, error_message);
                        }
                        true
                    }
                };
                if transfer_failed {
                    self.start_queued_sftp_transfers(cx);
                }
                true
            }
            ConnectionEvent::PerformanceSnapshot(snapshot) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.performance.update(snapshot, Instant::now());
                }
                true
            }
            ConnectionEvent::PerformanceFailed(error) => {
                let message = format!(
                    "{}\n{}: {}",
                    self.tr("performance-unavailable"),
                    self.tr("connection-technical-details"),
                    error.message()
                );
                if let Some(session) = self.session_mut(session_id) {
                    session.performance.loading = false;
                    session.performance.error = Some(message);
                }
                true
            }
            ConnectionEvent::Resized(size) => {
                let dimensions_changed = self
                    .session_mut(session_id)
                    .and_then(|session| session.terminal.as_mut())
                    .is_some_and(|terminal| terminal.acknowledge_resize(size));
                if dimensions_changed && let Some(session) = self.session_mut(session_id) {
                    session.terminal_selection = None;
                    session.terminal_selecting = false;
                }
                true
            }
            ConnectionEvent::Shell(
                ShellEvent::Output(data) | ShellEvent::ExtendedOutput { data, .. },
            ) => {
                let ui_state_changed = self.process_terminal_output(session_id, &data, cx);
                if ui_state_changed || self.terminal_session_is_rendered(session_id) {
                    self.schedule_terminal_redraw(cx);
                }
                false
            }
            ConnectionEvent::Shell(ShellEvent::ExitStatus(status)) => {
                let message = SessionMessage::localized_with(
                    "terminal-remote-shell-status",
                    [("status", status.to_string())],
                );
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::ExitSignal {
                signal,
                core_dumped,
                message,
            }) => {
                let message = SessionMessage::localized_with_args(
                    "terminal-remote-shell-signal",
                    [
                        ("signal", SessionMessageArg::Text(signal)),
                        (
                            "core",
                            if core_dumped {
                                SessionMessageArg::Localized("terminal-core-dumped")
                            } else {
                                SessionMessageArg::Text(String::new())
                            },
                        ),
                        ("message", SessionMessageArg::Text(message)),
                    ],
                );
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Eof) => {
                let message = SessionMessage::localized("terminal-remote-shell-eof");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    if session.terminal_end_reason.is_none() {
                        session.terminal_end_reason = Some(message);
                    }
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Closed) => {
                let message = SessionMessage::localized("terminal-remote-shell-closed");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    if session.terminal_end_reason.is_none() {
                        session.terminal_end_reason = Some(message);
                    }
                }
                true
            }
        };

        if should_notify {
            cx.notify();
        }
    }

    pub(super) fn render_credential_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let prompt = self
            .credential_prompt
            .as_ref()
            .expect("credential prompt should exist before rendering");
        let profile_label = self
            .profiles
            .iter()
            .find(|profile| profile.id == prompt.profile_id)
            .map(ConnectionProfile::address)
            .unwrap_or_else(|| prompt.profile_id.clone());
        let (title, field_label, key_path) = match &prompt.kind {
            CredentialPromptKind::Password => (
                self.tr("credential-password"),
                self.tr("credential-password"),
                None,
            ),
            CredentialPromptKind::PrivateKeyPassphrase { path } => (
                self.tr("credential-private-key-passphrase"),
                self.tr("credential-passphrase"),
                Some(path.display().to_string()),
            ),
            CredentialPromptKind::ProxyPassword => (
                self.tr("field-proxy-password"),
                self.tr("field-proxy-password"),
                None,
            ),
        };

        let mut modal = self
            .glass_floating_surface()
            .w_full()
            .max_w(px(420.0))
            .mx_4()
            .p_4()
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(profile_label),
            );

        if let Some(path) = key_path {
            modal = modal.child(
                div()
                    .mt_1()
                    .w_full()
                    .truncate()
                    .text_sm()
                    .text_color(self.theme.text_faint)
                    .child(path),
            );
        }

        modal = modal
            .child(div().mt_4().text_sm().child(field_label))
            .child(div().mt_2().child(prompt.input.clone()))
            .child(
                div()
                    .id("credential_remember")
                    .flex()
                    .items_center()
                    .gap_2()
                    .mt_3()
                    .text_sm()
                    .cursor_pointer()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(16.0))
                            .rounded_sm()
                            .border_1()
                            .border_color(if prompt.remember {
                                self.theme.accent
                            } else {
                                self.theme.border_strong
                            })
                            .bg(if prompt.remember {
                                self.theme.accent
                            } else {
                                self.theme.surface_bg
                            })
                            .text_color(self.theme.on_accent)
                            .when(prompt.remember, |this| this.child("✓")),
                    )
                    .child(self.tr("credential-remember"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(prompt) = this.credential_prompt.as_mut() {
                            prompt.remember = !prompt.remember;
                            cx.notify();
                        }
                    })),
            );

        if let Some(error) = prompt.error.as_ref() {
            modal = modal.child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(self.theme.error_text)
                    .child(error.clone()),
            );
        }

        modal = modal.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .mt_4()
                .child(
                    text_button(
                        "credential_cancel",
                        self.tr("common-cancel"),
                        TextButtonTone::Secondary,
                        true,
                        &self.theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_connection = None;
                        this.dismiss_credential_prompt(cx);
                        cx.notify();
                    })),
                )
                .child(
                    text_button(
                        "credential_submit",
                        self.tr("common-connect"),
                        TextButtonTone::Primary,
                        true,
                        &self.theme,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.submit_credential_prompt(window, cx);
                    })),
                ),
        );

        div()
            .id("credential_prompt")
            .key_context("CredentialPrompt")
            .on_action(cx.listener(Self::on_submit_credential))
            .on_action(cx.listener(Self::on_cancel_credential))
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.theme.overlay_bg)
            .occlude()
            .child(modal)
    }

    pub(super) fn render_proxy_command_approval_prompt(
        &self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let prompt = self
            .proxy_command_approval_prompt
            .as_ref()
            .expect("ProxyCommand approval prompt should exist before rendering");
        let command = prompt.expanded_command.expose_secret().to_owned();
        let modal = self
            .glass_floating_surface()
            .w_full()
            .max_w(px(620.0))
            .mx_4()
            .p_4()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("proxy-command-approval-title")),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(self.theme.status_warn)
                    .child(self.tr("proxy-command-approval-risk")),
            )
            .child(
                div()
                    .id("proxy-command-preview")
                    .mt_3()
                    .max_h(px(220.0))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.control_bg)
                    .p_3()
                    .font_family(UI_MONOSPACE_FONT_FAMILY)
                    .text_sm()
                    .child(command),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .mt_4()
                    .child(
                        text_button(
                            "proxy-command-cancel",
                            self.tr("common-cancel"),
                            TextButtonTone::Secondary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cancel_proxy_command_approval(cx);
                        })),
                    )
                    .child(
                        text_button(
                            "proxy-command-approve",
                            self.tr("common-trust-connect"),
                            TextButtonTone::Primary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.approve_proxy_command(cx);
                        })),
                    ),
            );
        div()
            .id("proxy-command-approval-prompt")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.theme.overlay_bg)
            .occlude()
            .child(modal)
    }

    pub(super) fn render_host_key_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self
            .active_session()
            .expect("host-key prompt requires an active session")
            .host_key_prompt
            .as_ref()
            .expect("host-key prompt should exist before rendering");

        let modal = self
            .glass_floating_surface()
            .w_full()
            .max_w(px(500.0))
            .mx_4()
            .p_4()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.tr("host-key-title")),
            )
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(info.address()),
            )
            .child(
                div()
                    .mt_4()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("host-key-description")),
            )
            .child(
                div()
                    .mt_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_sm()
                    .child(
                        div()
                            .w(px(80.0))
                            .flex_none()
                            .text_color(self.theme.text_faint)
                            .child(self.tr("host-key-algorithm")),
                    )
                    .child(div().child(info.algorithm().to_owned())),
            )
            .child(
                div()
                    .mt_3()
                    .text_sm()
                    .text_color(self.theme.text_faint)
                    .child(self.tr("host-key-fingerprint")),
            )
            .child(
                div()
                    .mt_1()
                    .w_full()
                    .font_family(UI_MONOSPACE_FONT_FAMILY)
                    .text_xs()
                    .child(info.fingerprint().to_owned()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .mt_4()
                    .child(
                        text_button(
                            "host_key_cancel",
                            self.tr("common-cancel"),
                            TextButtonTone::Secondary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reject_pending_host_key(cx);
                        })),
                    )
                    .child(
                        text_button(
                            "host_key_trust",
                            self.tr("common-trust-connect"),
                            TextButtonTone::Primary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.trust_pending_host_key(cx);
                        })),
                    ),
            );

        div()
            .id("host_key_prompt")
            .key_context("HostKeyPrompt")
            .on_action(cx.listener(Self::on_cancel_host_key_verification))
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(self.theme.overlay_bg)
            .occlude()
            .child(modal)
    }
}

pub(super) struct CredentialPrompt {
    pub(super) session_id: SessionId,
    pub(super) profile_id: String,
    pub(super) kind: CredentialPromptKind,
    pub(super) input: Entity<TextField>,
    pub(super) remember: bool,
    pub(super) error: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum CredentialPromptKind {
    Password,
    PrivateKeyPassphrase { path: PathBuf },
    ProxyPassword,
}

impl CredentialPromptKind {
    pub(super) fn credential_kind(&self) -> CredentialKind {
        match self {
            Self::Password => CredentialKind::Password,
            Self::PrivateKeyPassphrase { .. } => CredentialKind::PrivateKeyPassphrase,
            Self::ProxyPassword => CredentialKind::ProxyPassword,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CredentialSource {
    SystemKeychain,
    Prompt,
}

pub(super) struct ConnectionCredential {
    pub(super) profile_id: String,
    pub(super) kind: CredentialKind,
    pub(super) source: CredentialSource,
    pub(super) save_on_success: Option<secrecy::SecretString>,
}

pub(super) struct PendingConnectionPreparation {
    pub(super) session_id: SessionId,
    pub(super) target_profile: ConnectionProfile,
    pub(super) steps: Vec<ConnectionProfile>,
    pub(super) next_step: usize,
    pub(super) prepared_steps: Vec<ConnectionStep>,
    pub(super) credentials: Vec<ConnectionCredential>,
    pub(super) runtime_proxy: Option<RuntimeProxy>,
    pub(super) proxy_prepared: bool,
    pub(super) force_prompt: Option<(String, CredentialKind, Option<String>)>,
}

pub(super) struct ProxyCommandApprovalPrompt {
    pub(super) session_id: SessionId,
    pub(super) target_profile: ConnectionProfile,
    pub(super) plan: ConnectionPlan,
    pub(super) credentials: Vec<ConnectionCredential>,
    pub(super) expanded_command: SecretString,
    pub(super) approval_digest: String,
}

impl ConnectionCredential {
    pub(super) fn from_keychain(profile_id: String, kind: CredentialKind) -> Self {
        Self {
            profile_id,
            kind,
            source: CredentialSource::SystemKeychain,
            save_on_success: None,
        }
    }

    pub(super) fn from_prompt(
        profile_id: String,
        kind: CredentialKind,
        save_on_success: Option<secrecy::SecretString>,
    ) -> Self {
        Self {
            profile_id,
            kind,
            source: CredentialSource::Prompt,
            save_on_success,
        }
    }
}

pub(super) fn auth_method_with_secret(
    prompt_kind: CredentialPromptKind,
    secret: SecretString,
) -> AuthMethod {
    match prompt_kind {
        CredentialPromptKind::Password => AuthMethod::Password { password: secret },
        CredentialPromptKind::PrivateKeyPassphrase { path } => AuthMethod::PrivateKey {
            path,
            passphrase: Some(secret),
        },
        CredentialPromptKind::ProxyPassword => {
            unreachable!("proxy passwords are used by RuntimeProxy, not AuthMethod")
        }
    }
}

pub(super) fn runtime_proxy_with_password(
    profile: &ConnectionProfile,
    password: SecretString,
) -> Option<RuntimeProxy> {
    match profile.route.upstream_proxy.as_ref()? {
        ProxyConfig::HttpConnect {
            host,
            port,
            username,
        } => Some(RuntimeProxy::http_connect(
            host.clone(),
            *port,
            username.clone(),
            Some(password),
        )),
        ProxyConfig::Socks5 {
            host,
            port,
            username,
        } => Some(RuntimeProxy::socks5(
            host.clone(),
            *port,
            username.clone(),
            Some(password),
        )),
        ProxyConfig::ProxyCommand { .. } => None,
    }
}

pub(super) fn connection_stage_label(stage: &ConnectionStage, localizer: &Localizer) -> String {
    match stage {
        ConnectionStage::Proxy => localizer.text("connection-stage-proxy"),
        ConnectionStage::Jump { index, total, .. } => {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("index", *index);
            args.set("total", *total);
            localizer.text_with("connection-stage-jump", Some(&args))
        }
        ConnectionStage::Target { .. } => localizer.text("connection-stage-target"),
    }
}

pub(super) fn localized_connection_error(error: &SshError, localizer: &Localizer) -> String {
    localized_connection_error_parts(error.kind(), error.message(), error.stage(), localizer)
}

pub(super) fn localized_connection_error_parts(
    kind: SshErrorKind,
    details: &str,
    stage: Option<&ConnectionStage>,
    localizer: &Localizer,
) -> String {
    let (summary_key, suggestion_key) = match kind {
        SshErrorKind::InvalidState | SshErrorKind::Configuration => (
            "connection-error-summary-configuration",
            "connection-error-suggestion-configuration",
        ),
        SshErrorKind::Network => (
            "connection-error-summary-network",
            "connection-error-suggestion-network",
        ),
        SshErrorKind::Proxy => (
            "connection-error-summary-proxy",
            "connection-error-suggestion-proxy",
        ),
        SshErrorKind::ProxyAuthentication => (
            "connection-error-summary-proxy-auth",
            "connection-error-suggestion-proxy-auth",
        ),
        SshErrorKind::ProxyCommandApproval => (
            "connection-error-summary-proxy-command",
            "connection-error-suggestion-proxy-command",
        ),
        SshErrorKind::HostKeyUntrusted
        | SshErrorKind::HostKeyChanged
        | SshErrorKind::HostKeyPersistence
        | SshErrorKind::HostKeyVerification => (
            "connection-error-summary-host-key",
            "connection-error-suggestion-host-key",
        ),
        SshErrorKind::Authentication | SshErrorKind::PrivateKeyPassphrase => (
            "connection-error-summary-auth",
            "connection-error-suggestion-auth",
        ),
        SshErrorKind::Timeout => (
            "connection-error-summary-timeout",
            "connection-error-suggestion-timeout",
        ),
        SshErrorKind::Protocol => (
            "connection-error-summary-protocol",
            "connection-error-suggestion-protocol",
        ),
        SshErrorKind::Sftp => (
            "connection-error-summary-sftp",
            "connection-error-suggestion-sftp",
        ),
    };
    let summary = localizer.text(summary_key);
    let headline = stage.map_or_else(
        || summary.clone(),
        |stage| format!("{}: {summary}", connection_stage_label(stage, localizer)),
    );
    format!(
        "{headline}\n{}\n{}: {}",
        localizer.text(suggestion_key),
        localizer.text("connection-technical-details"),
        details
    )
}

pub(super) fn rejected_credential_kind(
    error_kind: SshErrorKind,
    authentication: Option<&AuthConfig>,
) -> Option<CredentialKind> {
    match error_kind {
        SshErrorKind::ProxyAuthentication => Some(CredentialKind::ProxyPassword),
        SshErrorKind::PrivateKeyPassphrase => Some(CredentialKind::PrivateKeyPassphrase),
        SshErrorKind::Authentication if matches!(authentication, Some(AuthConfig::Password)) => {
            Some(CredentialKind::Password)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remcmd_core::LanguageMode;

    #[test]
    fn authentication_failures_select_only_the_matching_credential_kind() {
        assert_eq!(
            rejected_credential_kind(SshErrorKind::Authentication, Some(&AuthConfig::Password)),
            Some(CredentialKind::Password)
        );
        assert_eq!(
            rejected_credential_kind(
                SshErrorKind::Authentication,
                Some(&AuthConfig::PrivateKey {
                    path: PathBuf::from("id_ed25519")
                })
            ),
            None
        );
        assert_eq!(
            rejected_credential_kind(SshErrorKind::ProxyAuthentication, None),
            Some(CredentialKind::ProxyPassword)
        );
        assert_eq!(
            rejected_credential_kind(
                SshErrorKind::PrivateKeyPassphrase,
                Some(&AuthConfig::PrivateKey {
                    path: PathBuf::from("id_ed25519")
                })
            ),
            Some(CredentialKind::PrivateKeyPassphrase)
        );
    }

    #[test]
    fn connection_errors_localize_summary_and_preserve_technical_details() {
        let error = SshError::new(SshErrorKind::Network, "connection refused by 10.0.0.1")
            .at_stage(ConnectionStage::Jump {
                index: 1,
                total: 2,
                profile_id: "jump".into(),
            });

        let english = localized_connection_error(&error, &Localizer::new(LanguageMode::EnUs));
        let chinese = localized_connection_error(&error, &Localizer::new(LanguageMode::ZhCn));

        assert!(english.contains("Jump 1/2: Network connection failed"));
        assert!(chinese.contains("跳板 1/2: 网络连接失败"));
        assert!(english.contains(error.message()));
        assert!(chinese.contains(error.message()));
    }
}
