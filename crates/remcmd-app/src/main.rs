mod text_field;
use text_field::{TextField, bind_text_field_keys};

mod ssh_runtime;
use ssh_runtime::SshRuntime;

#[cfg(target_os = "macos")]
mod private_key_picker;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, BoxShadow, Context, Entity, Focusable, FontWeight, IntoElement,
    KeyBinding, Render, SharedString, TitlebarOptions, Window, WindowBackgroundAppearance,
    WindowBounds, WindowOptions, div, point, prelude::*, px, rgb, rgba, size,
};

#[cfg(not(target_os = "macos"))]
use gpui::PathPromptOptions;

use remcmd_core::{AuthConfig, ConnectionProfile};
use remcmd_ssh::{
    AuthMethod, ConnectionEvent, ConnectionHandle, PtySize, SessionState, ShellEvent,
    SshConnection, SshErrorKind,
};
use remcmd_storage::{default_profiles_path, ensure_profiles_file, load_profiles, save_profiles};

gpui::actions!(credential_prompt, [SubmitCredential, CancelCredential]);

struct RemCmdApp {
    profiles: Vec<ConnectionProfile>,
    selected_profile_id: Option<String>,
    next_profile_number: usize,
    editor: Option<ProfileEditor>,
    form_error: Option<String>,
    profiles_path: PathBuf,
    connection_state: SessionState,
    connection_handle: Option<ConnectionHandle>,
    connection_profile_id: Option<String>,
    connection_error: Option<String>,
    connection_message: Option<String>,
    credential_prompt: Option<CredentialPrompt>,
}

#[derive(Clone)]
struct ProfileEditor {
    profile_id: String,
    name: Entity<TextField>,
    host: Entity<TextField>,
    port: Entity<TextField>,
    username: Entity<TextField>,
    auth_kind: ProfileAuthKind,
    private_key_path: Entity<TextField>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileAuthKind {
    Password,
    PrivateKey,
    Agent,
}

impl ProfileAuthKind {
    fn from_config(config: &AuthConfig) -> Self {
        match config {
            AuthConfig::Password => Self::Password,
            AuthConfig::PrivateKey { .. } => Self::PrivateKey,
            AuthConfig::Agent => Self::Agent,
        }
    }

    fn into_config(self, private_key_path: &str) -> Result<AuthConfig, &'static str> {
        match self {
            Self::Password => Ok(AuthConfig::Password),
            Self::PrivateKey => {
                let path = private_key_path.trim();
                if path.is_empty() {
                    return Err("Private key path is required");
                }

                Ok(AuthConfig::PrivateKey {
                    path: PathBuf::from(path),
                })
            }
            Self::Agent => Ok(AuthConfig::Agent),
        }
    }
}

struct CredentialPrompt {
    profile_id: String,
    kind: CredentialPromptKind,
    input: Entity<TextField>,
    error: Option<String>,
}

#[derive(Clone)]
enum CredentialPromptKind {
    Password,
    PrivateKeyPassphrase { path: PathBuf },
}

// Application construction and shared data helpers.
impl RemCmdApp {
    fn load(cx: &mut Context<Self>) -> Self {
        let profiles_path = default_profiles_path().expect("failed to resolve profiles path");

        let (profiles, form_error) = match ensure_profiles_file(&profiles_path)
            .and_then(|_| load_profiles(&profiles_path))
        {
            Ok(profiles) => (profiles, None),
            Err(error) => (
                Vec::new(),
                Some(format!("Failed to load profiles: {error}")),
            ),
        };

        let selected_profile_id = profiles.first().map(|profile| profile.id.clone());
        let next_profile_number = profiles
            .iter()
            .filter_map(|profile| profile.id.strip_prefix("demo-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;

        let mut app = Self {
            profiles,
            profiles_path,
            selected_profile_id,
            next_profile_number,
            editor: None,
            form_error,
            connection_state: SessionState::Disconnected,
            connection_handle: None,
            connection_profile_id: None,
            connection_error: None,
            connection_message: None,
            credential_prompt: None,
        };

        app.load_editor_for_selected_profile(cx);
        app
    }

    fn selected_profile(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_profile_id.as_ref()?;

        self.profiles
            .iter()
            .find(|profile| &profile.id == selected_id)
    }

    fn persist_profiles(&mut self) {
        if let Err(error) = save_profiles(&self.profiles_path, &self.profiles) {
            self.form_error = Some(format!("Failed to save profiles:\n{error}"));
        }
    }

    fn load_editor_for_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.editor = None;
            return;
        };

        let auth_kind = ProfileAuthKind::from_config(&profile.auth);
        let private_key_path = match &profile.auth {
            AuthConfig::PrivateKey { path } => path.to_string_lossy().into_owned(),
            AuthConfig::Password | AuthConfig::Agent => String::new(),
        };

        self.editor = Some(ProfileEditor {
            profile_id: profile.id.clone(),
            name: cx.new(|cx| TextField::new(cx, profile.name, "Name")),
            host: cx.new(|cx| TextField::new(cx, profile.host, "Host")),
            port: cx.new(|cx| TextField::new(cx, profile.port.to_string(), "Port")),
            username: cx.new(|cx| TextField::new(cx, profile.username, "Username")),
            auth_kind,
            private_key_path: cx.new(|cx| TextField::new(cx, private_key_path, "Private key path")),
        });

        self.form_error = None;
    }
}

// User interaction handlers.
impl RemCmdApp {
    fn open_credential_prompt(
        &mut self,
        profile_id: String,
        kind: CredentialPromptKind,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        self.dismiss_credential_prompt(cx);

        let placeholder = match kind {
            CredentialPromptKind::Password => "Password",
            CredentialPromptKind::PrivateKeyPassphrase { .. } => "Passphrase",
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
            profile_id,
            kind,
            input: input.clone(),
            error,
        });
        self.connection_error = None;
        cx.notify();

        input
    }

    fn dismiss_credential_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.credential_prompt.take() {
            prompt.input.update(cx, |input, cx| input.clear(cx));
        }
    }

    fn select_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.selected_profile_id = Some(profile_id);
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }

    fn add_profile(&mut self, cx: &mut Context<Self>) {
        let number = self.next_profile_number;

        let profile = ConnectionProfile::new(
            format!("demo-{number}"),
            format!("Demo Server {number}"),
            format!("demo-{number}.example.com"),
            22,
            "ubuntu",
        );

        self.selected_profile_id = Some(profile.id.clone());
        self.profiles.push(profile);
        self.next_profile_number += 1;

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        cx.notify();
    }

    fn delete_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(selected_id) = self.selected_profile_id.as_deref() else {
            return;
        };

        if self.connection_profile_id.as_deref() == Some(selected_id) {
            self.form_error = Some("Disconnect this profile before deleting it".into());
            cx.notify();
            return;
        }

        let Some(selected_index) = self
            .profiles
            .iter()
            .position(|profile| profile.id == selected_id)
        else {
            self.selected_profile_id = None;
            cx.notify();
            return;
        };

        self.profiles.remove(selected_index);

        self.selected_profile_id = if self.profiles.is_empty() {
            None
        } else if selected_index == 0 {
            Some(self.profiles[0].id.clone())
        } else {
            Some(self.profiles[selected_index - 1].id.clone())
        };

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        cx.notify();
    }

    fn select_auth_method(
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
        self.form_error = None;

        if auth_kind == ProfileAuthKind::PrivateKey {
            window.focus(&private_key_path.focus_handle(cx));
        }

        cx.notify();
    }

    #[cfg(target_os = "macos")]
    fn browse_private_key(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };

        let profile_id = editor.profile_id.clone();
        let current_path = editor.private_key_path.read(cx).text();
        let current_path =
            (!current_path.trim().is_empty()).then(|| PathBuf::from(current_path.trim()));

        cx.spawn(async move |this, cx| {
            let result = private_key_picker::pick_private_key(current_path.as_deref());

            let _ = this.update(cx, |this, cx| match result {
                Ok(Some(path)) => this.set_private_key_path(&profile_id, path, cx),
                Ok(None) => {}
                Err(error) => {
                    this.form_error = Some(format!("Failed to open file picker: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    #[cfg(not(target_os = "macos"))]
    fn browse_private_key(&mut self, cx: &mut Context<Self>) {
        let Some(profile_id) = self.editor.as_ref().map(|editor| editor.profile_id.clone()) else {
            return;
        };

        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select".into()),
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
                        this.form_error = Some(format!("Failed to open file picker: {error}"));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn set_private_key_path(&mut self, profile_id: &str, path: PathBuf, cx: &mut Context<Self>) {
        let Some(editor) = self
            .editor
            .as_mut()
            .filter(|editor| editor.profile_id == profile_id)
        else {
            return;
        };

        let path = path.to_string_lossy().into_owned();
        editor.private_key_path = cx.new(|cx| TextField::new(cx, path, "Private key path"));
        self.form_error = None;
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.clone() else {
            return;
        };

        let name = editor.name.read(cx).text();
        let host = editor.host.read(cx).text();
        let port_text = editor.port.read(cx).text();
        let username = editor.username.read(cx).text();
        let private_key_path = editor.private_key_path.read(cx).text();

        let Ok(port) = port_text.trim().parse::<u16>() else {
            self.form_error = Some("Port must be a number from 1 to 65535".into());
            cx.notify();
            return;
        };

        if port == 0 {
            self.form_error = Some("Port must be a number from 1 to 65535".into());
            cx.notify();
            return;
        };

        let auth = match editor.auth_kind.into_config(&private_key_path) {
            Ok(auth) => auth,
            Err(error) => {
                self.form_error = Some(error.into());
                cx.notify();
                return;
            }
        };

        if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id == editor.profile_id)
        {
            profile.name = name;
            profile.host = host;
            profile.port = port;
            profile.username = username;
            profile.auth = auth;
        }

        self.form_error = None;
        self.persist_profiles();

        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }

    fn connect_selected_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.connection_state.can_connect() {
            return;
        }

        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };

        let auth = match &profile.auth {
            AuthConfig::Password => {
                let input = self.open_credential_prompt(
                    profile.id.clone(),
                    CredentialPromptKind::Password,
                    None,
                    cx,
                );
                window.focus(&input.focus_handle(cx));
                return;
            }
            AuthConfig::PrivateKey { path } => AuthMethod::private_key(path.clone(), None),
            AuthConfig::Agent => AuthMethod::Agent,
        };

        self.start_connection(profile, auth, cx);
    }

    fn start_connection(
        &mut self,
        profile: ConnectionProfile,
        auth: AuthMethod,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_credential_prompt(cx);

        let runtime = cx.global::<SshRuntime>().handle();
        let connection = SshConnection::spawn(&runtime, profile.clone(), auth, PtySize::default());
        let (handle, mut events) = connection.split();

        self.connection_state = SessionState::Connecting;
        self.connection_handle = Some(handle);
        self.connection_profile_id = Some(profile.id);
        self.connection_error = None;
        self.connection_message = None;

        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next_event().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_connection_event(event, cx);
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

    fn submit_credential_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.credential_prompt.as_mut() else {
            return;
        };

        if prompt.input.read(cx).is_empty() {
            let label = match prompt.kind {
                CredentialPromptKind::Password => "Password",
                CredentialPromptKind::PrivateKeyPassphrase { .. } => "Passphrase",
            };
            prompt.error = Some(format!("{label} is required"));
            window.focus(&prompt.input.focus_handle(cx));
            cx.notify();
            return;
        }

        let profile_id = prompt.profile_id.clone();
        let kind = prompt.kind.clone();
        let input = prompt.input.clone();

        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
        else {
            self.dismiss_credential_prompt(cx);
            self.connection_error = Some("Connection profile no longer exists".into());
            cx.notify();
            return;
        };

        let secret = input.update(cx, |input, cx| input.take_text(cx));
        self.credential_prompt = None;

        let auth = match kind {
            CredentialPromptKind::Password => AuthMethod::password(secret),
            CredentialPromptKind::PrivateKeyPassphrase { path } => {
                AuthMethod::private_key(path, Some(secret))
            }
        };

        self.start_connection(profile, auth, cx);
    }

    fn on_submit_credential(
        &mut self,
        _: &SubmitCredential,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_credential_prompt(window, cx);
    }

    fn on_cancel_credential(
        &mut self,
        _: &CancelCredential,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_credential_prompt(cx);
        cx.notify();
    }

    fn prompt_for_private_key_passphrase(
        &mut self,
        profile_id: String,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .and_then(|profile| match &profile.auth {
                AuthConfig::PrivateKey { path } => Some(path.clone()),
                AuthConfig::Password | AuthConfig::Agent => None,
            })
        else {
            return false;
        };

        self.open_credential_prompt(
            profile_id,
            CredentialPromptKind::PrivateKeyPassphrase { path },
            Some(error),
            cx,
        );
        true
    }

    fn disconnect_active_connection(&mut self, cx: &mut Context<Self>) {
        if !self.connection_state.can_disconnect() {
            return;
        }

        let Some(handle) = self.connection_handle.as_ref() else {
            self.connection_state = SessionState::Failed;
            self.connection_error = Some("SSH connection handle is missing".into());
            self.connection_profile_id = None;
            cx.notify();
            return;
        };

        if let Err(error) = handle.disconnect() {
            self.connection_state = SessionState::Failed;
            self.connection_handle = None;
            self.connection_profile_id = None;
            self.connection_error = Some(error.to_string());
        } else {
            // Disable repeated clicks before the worker publishes its event.
            self.connection_state = SessionState::Disconnecting;
        }

        cx.notify();
    }

    fn handle_connection_event(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        let should_notify = match event {
            ConnectionEvent::StateChanged(state) => {
                self.connection_state = state;

                if state == SessionState::Disconnected {
                    self.connection_handle = None;
                    self.connection_profile_id = None;
                }

                true
            }
            ConnectionEvent::Failed(error) => {
                let failed_profile_id = self.connection_profile_id.take();
                self.connection_state = SessionState::Failed;
                self.connection_handle = None;

                let prompted_for_passphrase = if error.kind() == SshErrorKind::PrivateKeyPassphrase
                {
                    failed_profile_id
                        .map(|profile_id| {
                            self.prompt_for_private_key_passphrase(
                                profile_id,
                                error.to_string(),
                                cx,
                            )
                        })
                        .unwrap_or(false)
                } else {
                    false
                };

                if !prompted_for_passphrase {
                    self.connection_error = Some(error.to_string());
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Output(_) | ShellEvent::ExtendedOutput { .. }) => {
                // Terminal output will be consumed by remcmd-terminal later.
                false
            }
            ConnectionEvent::Shell(ShellEvent::ExitStatus(status)) => {
                self.connection_message = Some(format!("Remote shell exited with status {status}"));
                true
            }
            ConnectionEvent::Shell(ShellEvent::ExitSignal {
                signal,
                core_dumped,
                message,
            }) => {
                let core_dump = if core_dumped { " (core dumped)" } else { "" };
                self.connection_message = Some(format!(
                    "Remote shell exited on signal {signal}{core_dump}: {message}"
                ));
                true
            }
            ConnectionEvent::Shell(ShellEvent::Eof) => {
                self.connection_message = Some("Remote shell reached EOF".into());
                true
            }
            ConnectionEvent::Shell(ShellEvent::Closed) => {
                self.connection_message = Some("Remote shell closed".into());
                true
            }
        };

        if should_notify {
            cx.notify();
        }
    }
}

// Root rendering entry point and drawing helpers.
impl Render for RemCmdApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_profile = self.selected_profile().cloned();

        let mut root = div()
            .relative()
            .flex()
            .size_full()
            .text_color(rgb(0xf4f4f5))
            .child(self.render_sidebar(cx))
            .child(self.render_detail_panel(selected_profile, cx));

        if let Some(prompt) = self.credential_prompt.as_ref() {
            let focus_handle = prompt.input.focus_handle(cx);
            if !focus_handle.is_focused(window) {
                window.focus(&focus_handle);
            }

            root = root.child(self.render_credential_prompt(cx));
        }

        root
    }
}

impl RemCmdApp {
    fn render_credential_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            CredentialPromptKind::Password => ("Password", "Password", None),
            CredentialPromptKind::PrivateKeyPassphrase { path } => (
                "Private key passphrase",
                "Passphrase",
                Some(path.display().to_string()),
            ),
        };

        let mut modal = div()
            .w(px(420.0))
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(rgba(0xffffff24))
            .bg(rgb(0x242424))
            .shadow(vec![BoxShadow {
                color: rgba(0x00000066).into(),
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(24.0),
                spread_radius: px(-8.0),
            }])
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .text_color(rgb(0xa1a1aa))
                    .child(profile_label),
            );

        if let Some(path) = key_path {
            modal = modal.child(
                div()
                    .mt_1()
                    .w_full()
                    .truncate()
                    .text_sm()
                    .text_color(rgb(0x71717a))
                    .child(path),
            );
        }

        modal = modal
            .child(div().mt_4().text_sm().child(field_label))
            .child(
                div()
                    .mt_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgba(0xffffff2e))
                    .bg(rgb(0x181818))
                    .child(prompt.input.clone()),
            );

        if let Some(error) = prompt.error.as_ref() {
            modal = modal.child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(rgb(0xfca5a5))
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
                    div()
                        .id("credential_cancel")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgba(0xffffff12))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgba(0xffffff1f)))
                        .child("Cancel")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dismiss_credential_prompt(cx);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("credential_submit")
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x2563eb))
                        .cursor_pointer()
                        .hover(|this| this.bg(rgb(0x3b82f6)))
                        .child("Connect")
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
            .bg(rgba(0x0000008f))
            .occlude()
            .child(modal)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut connection_list = div()
            .id("connection_list")
            .flex_1()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .mt_2();

        for profile in &self.profiles {
            let profile_id = profile.id.clone();
            let is_selected = self.selected_profile_id.as_ref() == Some(&profile.id);
            let item_background = if is_selected {
                rgb(0x4f4d50)
            } else {
                rgba(0xffffff00)
            };

            let profile_item = div()
                .id(SharedString::from(format!("profile-{}", profile.id)))
                .mb_1()
                .w_full()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(item_background)
                .cursor_pointer()
                .hover(move |this| {
                    let background = if is_selected {
                        rgb(0x59575b)
                    } else {
                        rgb(0x454347)
                    };

                    this.bg(background)
                })
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .child(profile.name.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .w_full()
                        .truncate()
                        .text_sm()
                        .text_color(rgb(0xa1a1aa))
                        .child(profile.address()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_profile(profile_id.clone(), cx);
                }));

            connection_list = connection_list.child(profile_item);
        }

        div()
            .flex()
            .flex_col()
            .w(px(300.0))
            .h_full()
            .bg(rgba(0x212121e8))
            .px_4()
            .pb_4()
            .pt(px(52.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_none()
                    .child("Connections")
                    .child(
                        div()
                            .id("add_connection")
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(rgb(0x3b82f6))
                            .cursor_pointer()
                            .child("Add")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_profile(cx);
                            })),
                    ),
            )
            .child(connection_list)
    }

    fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut panel = div()
            .flex_1()
            .h_full()
            .px_4()
            .pb_4()
            .pt(px(52.0))
            .bg(rgb(0x181818))
            .border_l_1()
            .border_color(rgba(0xffffff40))
            .shadow(vec![BoxShadow {
                color: rgba(0x0000001f).into(),
                offset: point(px(-1.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }]);

        match selected_profile {
            Some(profile) => {
                let Some(editor) = self.editor.as_ref() else {
                    return panel.child("No editor loaded");
                };

                panel = panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(profile.name.clone())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(self.render_connection_controls(cx))
                                    .child(
                                        div()
                                            .id("delete_connection")
                                            .px_2()
                                            .py_1()
                                            .rounded_lg()
                                            .bg(rgb(0xdc2626))
                                            .cursor_pointer()
                                            .child("Delete")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.delete_selected_profile(cx);
                                            })),
                                    ),
                            ),
                    )
                    .child(self.render_form_row("Name", editor.name.clone()))
                    .child(self.render_form_row("Host", editor.host.clone()))
                    .child(self.render_form_row("Port", editor.port.clone()))
                    .child(self.render_form_row("Username", editor.username.clone()))
                    .child(self.render_auth_method_row(editor.auth_kind, cx))
                    .when(editor.auth_kind == ProfileAuthKind::PrivateKey, |this| {
                        this.child(self.render_private_key_row(editor.private_key_path.clone(), cx))
                    })
                    .when_some(self.form_error.as_ref(), |this, error| {
                        this.child(div().mt_3().text_color(rgb(0xfca5a5)).child(error.clone()))
                    })
                    .when_some(self.connection_error.as_ref(), |this, error| {
                        this.child(div().mt_3().text_color(rgb(0xfca5a5)).child(error.clone()))
                    })
                    .when_some(self.connection_message.as_ref(), |this, message| {
                        this.child(
                            div()
                                .mt_3()
                                .text_color(rgb(0xa1a1aa))
                                .child(message.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .mt_6()
                            .gap_2()
                            .child(
                                div()
                                    .id("save_profile")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgb(0x2563eb))
                                    .cursor_pointer()
                                    .child("Save")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save_editor(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .id("cancel_profile")
                                    .px_3()
                                    .py_2()
                                    .rounded_lg()
                                    .bg(rgba(0xffffff18))
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_editor(cx);
                                    })),
                            ),
                    );
            }
            None => {
                panel = panel.child("No connection selected");
            }
        }

        panel
    }

    fn render_connection_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_connect = self.connection_state.can_connect();
        let can_disconnect = self.connection_state.can_disconnect();

        let label = match self.connection_state {
            SessionState::Failed => "Retry",
            SessionState::Disconnecting => "Disconnecting",
            _ if can_disconnect => "Disconnect",
            _ => "Connect",
        };

        let button_color = if can_disconnect {
            rgb(0xdc2626)
        } else {
            rgb(0x2563eb)
        };

        let status_color = match self.connection_state {
            SessionState::Connected => rgb(0x86efac),
            SessionState::Failed => rgb(0xfca5a5),
            SessionState::Connecting
            | SessionState::Authenticating
            | SessionState::Disconnecting => rgb(0xfde68a),
            SessionState::Disconnected => rgb(0xa1a1aa),
        };

        let mut button = div()
            .id("connection_action")
            .px_3()
            .py_1()
            .rounded_lg()
            .bg(button_color)
            .child(label);

        if can_connect {
            button = button
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    this.connect_selected_profile(window, cx);
                }));
        } else if can_disconnect {
            button = button
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.disconnect_active_connection(cx);
                }));
        } else {
            button = button.opacity(0.55);
        }

        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(status_color)
                    .child(self.connection_status_text()),
            )
            .child(button)
    }

    fn connection_status_text(&self) -> String {
        let state = match self.connection_state {
            SessionState::Disconnected => "Disconnected",
            SessionState::Connecting => "Connecting",
            SessionState::Authenticating => "Authenticating",
            SessionState::Connected => "Connected",
            SessionState::Disconnecting => "Disconnecting",
            SessionState::Failed => "Failed",
        };

        let Some(profile_id) = self.connection_profile_id.as_deref() else {
            return state.into();
        };

        let profile_name = self
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .map(|profile| profile.name.as_str())
            .unwrap_or(profile_id);

        format!("{state} - {profile_name}")
    }

    fn render_auth_method_row(
        &self,
        selected: ProfileAuthKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .mt_3()
            .child(div().w(px(112.0)).child("Authentication"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .h(px(38.0))
                    .p(px(2.0))
                    .gap(px(2.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffff26))
                    .bg(rgba(0xffffff0d))
                    .child(self.render_auth_method_option(
                        "auth_password",
                        "Password",
                        ProfileAuthKind::Password,
                        selected,
                        cx,
                    ))
                    .child(self.render_auth_method_option(
                        "auth_private_key",
                        "Private Key",
                        ProfileAuthKind::PrivateKey,
                        selected,
                        cx,
                    ))
                    .child(self.render_auth_method_option(
                        "auth_agent",
                        "SSH Agent",
                        ProfileAuthKind::Agent,
                        selected,
                        cx,
                    )),
            )
    }

    fn render_auth_method_option(
        &self,
        id: &'static str,
        label: &'static str,
        auth_kind: ProfileAuthKind,
        selected: ProfileAuthKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = auth_kind == selected;
        let background = if is_selected {
            rgb(0x4f4d50)
        } else {
            rgba(0xffffff00)
        };
        let border = if is_selected {
            rgba(0xffffff40)
        } else {
            rgba(0xffffff00)
        };

        div()
            .id(id)
            .flex()
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .text_sm()
            .cursor_pointer()
            .hover(move |this| {
                if is_selected {
                    this.bg(rgb(0x59575b))
                } else {
                    this.bg(rgba(0xffffff18))
                }
            })
            .when(is_selected, |this| {
                this.shadow(vec![BoxShadow {
                    color: rgba(0x0000002e).into(),
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(3.0),
                    spread_radius: px(-1.0),
                }])
            })
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_auth_method(auth_kind, window, cx);
            }))
    }

    fn render_private_key_row(
        &self,
        field: Entity<TextField>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .mt_3()
            .child(div().w(px(112.0)).child("Key file"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgba(0xffffff26))
                            .bg(rgba(0xffffff12))
                            .child(field),
                    )
                    .child(
                        div()
                            .id("browse_private_key")
                            .flex()
                            .flex_none()
                            .h(px(34.0))
                            .items_center()
                            .px_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgba(0xffffff26))
                            .bg(rgba(0xffffff12))
                            .cursor_pointer()
                            .hover(|this| this.bg(rgba(0xffffff1f)))
                            .child("Browse")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.browse_private_key(cx);
                            })),
                    ),
            )
    }

    fn render_form_row(&self, label: &'static str, field: Entity<TextField>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .mt_3()
            .child(div().w(px(112.0)).child(label))
            .child(
                div()
                    .flex_1()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xffffff26))
                    .bg(rgba(0xffffff12))
                    .child(field),
            )
    }
}

// Application startup functions stay outside main so startup remains testable and readable.
fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: WindowBackgroundAppearance::Blurred,
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn open_main_window(cx: &mut App) {
    let options = main_window_options(cx);

    cx.open_window(options, |_, cx| cx.new(RemCmdApp::load))
        .expect("failed to open main window");
}

fn bind_credential_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitCredential, Some("CredentialPrompt")),
        KeyBinding::new("escape", CancelCredential, Some("CredentialPrompt")),
    ]);
}

fn launch(cx: &mut App) {
    cx.set_global(SshRuntime::new().expect("failed to create SSH runtime"));

    bind_text_field_keys(cx);
    bind_credential_prompt_keys(cx);
    open_main_window(cx);
    cx.activate(true);
}

fn main() {
    Application::new().run(launch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_auth_kind_reflects_saved_configuration() {
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
    fn private_key_authentication_requires_a_path() {
        assert_eq!(
            ProfileAuthKind::PrivateKey.into_config("   "),
            Err("Private key path is required")
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
            ProfileAuthKind::Password.into_config("ignored"),
            Ok(AuthConfig::Password)
        );
        assert_eq!(
            ProfileAuthKind::Agent.into_config("ignored"),
            Ok(AuthConfig::Agent)
        );
    }
}
