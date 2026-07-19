mod text_field;
use text_field::{TextField, bind_text_field_keys};

mod ssh_runtime;
use ssh_runtime::SshRuntime;

mod theme;
use theme::{ButtonVariant, Theme, button, set_global_theme};

mod terminal_input;
use terminal_input::{
    encode_alternate_scroll, encode_focus, encode_key, encode_paste,
    should_translate_alternate_scroll,
};

mod terminal_canvas;
use terminal_canvas::{TerminalCanvasFrame, TerminalCellMetrics};

mod terminal_view;
use terminal_view::{TerminalPalette, TerminalViewModel, palette_color};

#[cfg(target_os = "macos")]
mod private_key_picker;

use std::{ops::Range, path::PathBuf, time::Duration};

use gpui::{
    App, Application, Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight, IntoElement, KeyBinding,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Render, ScrollWheelEvent, SharedString, Subscription, Task, Timer, TitlebarOptions,
    UTF16Selection, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, canvas, div,
    point, prelude::*, px, rgb, size,
};

#[cfg(not(target_os = "macos"))]
use gpui::PathPromptOptions;

use remcmd_core::{AuthConfig, ConnectionProfile, ThemeMode};
use remcmd_ssh::{
    AuthMethod, ConnectionEvent, ConnectionHandle, HostKeyInfo, PtySize, SessionState, ShellEvent,
    SshConnection, SshErrorKind,
};
use remcmd_storage::{
    AppSettings, default_profiles_path, default_settings_path, ensure_profiles_file, load_profiles,
    load_settings, save_profiles, save_settings,
};
use remcmd_terminal::{
    Clipboard as TerminalClipboard, Scroll as TerminalScroll, TerminalEngine, TerminalEvent,
    TerminalModes, TerminalPoint, TerminalSelection, TerminalSnapshot, TextAreaSize,
};

const TERMINAL_COLUMNS: u32 = 80;
const TERMINAL_ROWS: u32 = 24;
const TERMINAL_CELL_WIDTH: u16 = 8;
const TERMINAL_CELL_HEIGHT: u16 = 19;
const TERMINAL_RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

#[cfg(target_os = "macos")]
const TERMINAL_FONT_FAMILY: &str = "SF Mono";
#[cfg(target_os = "windows")]
const TERMINAL_FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TERMINAL_FONT_FAMILY: &str = "DejaVu Sans Mono";

gpui::actions!(credential_prompt, [SubmitCredential, CancelCredential]);
gpui::actions!(host_key_prompt, [CancelHostKeyVerification]);

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
    terminal_end_reason: Option<String>,
    credential_prompt: Option<CredentialPrompt>,
    host_key_prompt: Option<HostKeyInfo>,
    terminal: Option<ActiveTerminal>,
    terminal_focus_handle: FocusHandle,
    terminal_marked_text: String,
    terminal_selection: Option<TerminalSelection>,
    terminal_selecting: bool,
    terminal_scroll_accumulator: f32,
    terminal_resize_task: Option<Task<()>>,
    active_panel: ActivePanel,
    theme_mode: ThemeMode,
    theme: Theme,
    settings_path: PathBuf,
    settings_error: Option<String>,
    _appearance_subscription: Subscription,
}

struct ActiveTerminal {
    profile_id: String,
    engine: TerminalEngine,
    title: Option<String>,
    pty_size: PtySize,
    pending_pty_size: Option<PtySize>,
    cell_width: f32,
    cell_height: f32,
    viewport_bounds: Option<Bounds<Pixels>>,
    was_connected: bool,
}

impl ActiveTerminal {
    fn new(profile_id: String, size: PtySize) -> Self {
        let columns = usize::try_from(size.columns).expect("PTY columns fit usize");
        let rows = usize::try_from(size.rows).expect("PTY rows fit usize");
        let engine = TerminalEngine::new(columns, rows).expect("valid initial terminal size");

        Self {
            profile_id,
            engine,
            title: None,
            pty_size: size,
            pending_pty_size: None,
            cell_width: f32::from(TERMINAL_CELL_WIDTH),
            cell_height: f32::from(TERMINAL_CELL_HEIGHT),
            viewport_bounds: None,
            was_connected: false,
        }
    }

    fn process(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        self.engine.process(bytes);
        self.engine.drain_events()
    }

    fn snapshot(&self) -> TerminalSnapshot {
        self.engine.snapshot()
    }

    fn text_area_size(&self) -> TextAreaSize {
        let size = self.engine.size();

        TextAreaSize {
            rows: u16::try_from(size.rows()).unwrap_or(u16::MAX),
            columns: u16::try_from(size.columns()).unwrap_or(u16::MAX),
            cell_width: pixel_cell_dimension(self.cell_width),
            cell_height: pixel_cell_dimension(self.cell_height),
        }
    }

    fn modes(&self) -> TerminalModes {
        self.engine.modes()
    }

    fn stage_resize(&mut self, size: PtySize) -> bool {
        let current_target = self.pending_pty_size.unwrap_or(self.pty_size);
        if current_target == size {
            return false;
        }

        self.pending_pty_size = Some(size);
        true
    }

    fn acknowledge_resize(&mut self, size: PtySize) -> bool {
        let dimensions_changed =
            self.pty_size.columns != size.columns || self.pty_size.rows != size.rows;
        if dimensions_changed {
            self.engine
                .resize(
                    usize::try_from(size.columns).expect("PTY columns fit usize"),
                    usize::try_from(size.rows).expect("PTY rows fit usize"),
                )
                .expect("measured terminal size is valid");
        }

        self.pty_size = size;
        if self.pending_pty_size == Some(size) {
            self.pending_pty_size = None;
        }
        dimensions_changed
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalLayout {
    pty_size: PtySize,
    cell_width: f32,
    cell_height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ActivePanel {
    #[default]
    Connection,
    Settings,
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
    fn load(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profiles_path = default_profiles_path().expect("failed to resolve profiles path");
        let settings_path = default_settings_path().expect("failed to resolve settings path");

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

        let (settings, settings_error) = match load_settings(&settings_path) {
            Ok(settings) => (settings, None),
            Err(error) => (
                AppSettings::default(),
                Some(format!("Failed to load settings: {error}")),
            ),
        };
        let theme_mode = settings.theme_mode;
        let theme = Theme::resolve(theme_mode, window);
        set_global_theme(theme, cx);

        let appearance_subscription = cx.observe_window_appearance(window, |this, window, cx| {
            this.refresh_system_theme(window, cx);
        });

        let terminal_focus_handle = cx.focus_handle();
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
            terminal_end_reason: None,
            credential_prompt: None,
            host_key_prompt: None,
            terminal: None,
            terminal_focus_handle: terminal_focus_handle.clone(),
            terminal_marked_text: String::new(),
            terminal_selection: None,
            terminal_selecting: false,
            terminal_scroll_accumulator: 0.0,
            terminal_resize_task: None,
            active_panel: ActivePanel::Connection,
            theme_mode,
            theme,
            settings_path,
            settings_error,
            _appearance_subscription: appearance_subscription,
        };

        cx.on_focus(&terminal_focus_handle, window, |this, _, cx| {
            this.report_terminal_focus(true, cx);
        })
        .detach();
        cx.on_blur(&terminal_focus_handle, window, |this, _, cx| {
            this.report_terminal_focus(false, cx);
        })
        .detach();

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

    fn refresh_system_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.theme_mode != ThemeMode::System {
            return;
        }

        self.theme = Theme::resolve(self.theme_mode, window);
        set_global_theme(self.theme, cx);
        cx.notify();
    }

    fn set_theme_mode(&mut self, theme_mode: ThemeMode, window: &Window, cx: &mut Context<Self>) {
        self.theme_mode = theme_mode;
        self.theme = Theme::resolve(theme_mode, window);
        set_global_theme(self.theme, cx);

        let settings = AppSettings { theme_mode };
        self.settings_error = save_settings(&self.settings_path, &settings)
            .err()
            .map(|error| format!("Failed to save settings: {error}"));
        cx.notify();
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
        self.active_panel = ActivePanel::Connection;
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

        self.active_panel = ActivePanel::Connection;
        self.selected_profile_id = Some(profile.id.clone());
        self.profiles.push(profile);
        self.next_profile_number += 1;

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        cx.notify();
    }

    fn show_settings(&mut self, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::Settings;
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

        if self
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal.profile_id == selected_id)
        {
            self.terminal = None;
            self.terminal_selection = None;
            self.terminal_selecting = false;
            self.terminal_resize_task = None;
            self.connection_error = None;
            self.connection_message = None;
            self.terminal_end_reason = None;
        }

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
        self.host_key_prompt = None;

        let runtime = cx.global::<SshRuntime>().handle();
        let pty_size = PtySize::new(TERMINAL_COLUMNS, TERMINAL_ROWS);
        let connection = SshConnection::spawn(&runtime, profile.clone(), auth, pty_size);
        let (handle, mut events) = connection.split();

        self.connection_state = SessionState::Connecting;
        self.connection_handle = Some(handle);
        self.connection_profile_id = Some(profile.id.clone());
        self.connection_error = None;
        self.connection_message = None;
        self.terminal_end_reason = None;
        self.terminal = Some(ActiveTerminal::new(profile.id, pty_size));
        self.terminal_marked_text.clear();
        self.terminal_selection = None;
        self.terminal_selecting = false;
        self.terminal_scroll_accumulator = 0.0;
        self.terminal_resize_task = None;

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

    fn trust_pending_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.host_key_prompt.take() else {
            return;
        };

        let Some(handle) = self.connection_handle.as_ref() else {
            self.connection_error = Some("SSH connection handle is missing".into());
            cx.notify();
            return;
        };

        match handle.trust_host_key() {
            Ok(()) => {
                self.connection_message = Some(format!("Trusting {}", info.address()));
            }
            Err(error) => {
                self.connection_error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    fn reject_pending_host_key(&mut self, cx: &mut Context<Self>) {
        if self.host_key_prompt.take().is_none() {
            return;
        }

        if let Some(handle) = self.connection_handle.as_ref()
            && let Err(error) = handle.reject_host_key()
        {
            self.connection_error = Some(error.to_string());
        }
        self.connection_message = None;
        cx.notify();
    }

    fn on_cancel_host_key_verification(
        &mut self,
        _: &CancelHostKeyVerification,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reject_pending_host_key(cx);
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

        self.terminal_resize_task = None;

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
            self.terminal_end_reason = Some("Session disconnected".into());
        }

        cx.notify();
    }

    fn terminal_modes(&self) -> TerminalModes {
        self.terminal
            .as_ref()
            .map(ActiveTerminal::modes)
            .unwrap_or(TerminalModes::NONE)
    }

    fn terminal_palette(&self) -> TerminalPalette {
        if self.theme.is_light() {
            TerminalPalette::light()
        } else {
            TerminalPalette::dark()
        }
    }

    fn terminal_point_for_position(&self, position: gpui::Point<Pixels>) -> Option<TerminalPoint> {
        let terminal = self.terminal.as_ref()?;
        let bounds = terminal.viewport_bounds?;
        let local = bounds.localize(&position)?;
        let size = terminal.engine.size();
        let cell_width = terminal.cell_width.max(1.0);
        let cell_height = terminal.cell_height.max(1.0);

        Some(terminal_point_for_pixels(
            f32::from(local.x),
            f32::from(local.y),
            size.columns(),
            size.rows(),
            cell_width,
            cell_height,
        ))
    }

    fn on_terminal_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_focus_handle.focus(window);

        let Some(point) = self.terminal_point_for_position(event.position) else {
            return;
        };

        if event.modifiers.shift
            && let Some(selection) = self.terminal_selection.as_mut()
        {
            selection.head = point;
        } else {
            self.terminal_selection = Some(TerminalSelection::new(point, point));
        }

        self.terminal_selecting = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn on_terminal_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal_selecting || !event.dragging() {
            return;
        }

        let Some(point) = self.terminal_point_for_position(event.position) else {
            return;
        };
        let Some(selection) = self.terminal_selection.as_mut() else {
            return;
        };

        if selection.head != point {
            selection.head = point;
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_terminal_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal_selecting = false;
        if self
            .terminal_selection
            .is_some_and(TerminalSelection::is_empty)
        {
            self.terminal_selection = None;
            cx.notify();
        }
    }

    fn copy_terminal_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(selection) = self
            .terminal_selection
            .filter(|selection| !selection.is_empty())
        else {
            return false;
        };
        let Some(terminal) = self.terminal.as_ref() else {
            return false;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(
            terminal.snapshot().selected_text(selection),
        ));
        true
    }

    fn clear_terminal_selection(&mut self) -> bool {
        let had_selection = self.terminal_selection.take().is_some();
        let was_selecting = std::mem::take(&mut self.terminal_selecting);
        had_selection || was_selecting
    }

    fn on_terminal_key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_terminal_copy_shortcut(&event.keystroke) && self.copy_terminal_selection(cx) {
            cx.stop_propagation();
            return;
        }

        if is_terminal_paste_shortcut(&event.keystroke) {
            self.paste_into_terminal(cx);
            cx.stop_propagation();
            return;
        }

        if let Some(bytes) = encode_key(&event.keystroke, self.terminal_modes()) {
            self.send_terminal_user_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn paste_into_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };

        let bytes = encode_paste(&text, self.terminal_modes());
        self.send_terminal_user_input(bytes, cx);
    }

    fn report_terminal_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if let Some(bytes) = encode_focus(focused, self.terminal_modes()) {
            self.send_terminal_input(bytes, cx);
        }
    }

    fn send_terminal_input(&mut self, data: Vec<u8>, cx: &mut Context<Self>) {
        if data.is_empty() || self.connection_state != SessionState::Connected {
            return;
        }

        let Some(handle) = self.connection_handle.as_ref() else {
            return;
        };

        if let Err(error) = handle.send_input(data) {
            self.connection_error = Some(error.to_string());
            cx.notify();
        }
    }

    fn send_terminal_user_input(&mut self, data: Vec<u8>, cx: &mut Context<Self>) {
        if data.is_empty() {
            return;
        }

        let selection_cleared = self.clear_terminal_selection();

        if let Some(terminal) = self.terminal.as_mut()
            && terminal.engine.display_offset() != 0
        {
            terminal.engine.scroll(TerminalScroll::Bottom);
            cx.notify();
        }

        if selection_cleared {
            cx.notify();
        }

        self.send_terminal_input(data, cx);
    }

    fn apply_terminal_layout(
        &mut self,
        profile_id: &str,
        bounds: Bounds<Pixels>,
        layout: TerminalLayout,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self
            .terminal
            .as_mut()
            .filter(|terminal| terminal.profile_id == profile_id)
        else {
            return;
        };

        terminal.viewport_bounds = Some(bounds);
        let cell_size_changed =
            terminal.cell_width != layout.cell_width || terminal.cell_height != layout.cell_height;
        terminal.cell_width = layout.cell_width;
        terminal.cell_height = layout.cell_height;

        if !terminal.stage_resize(layout.pty_size) {
            if cell_size_changed {
                cx.notify();
            }
            return;
        }

        self.schedule_terminal_resize(profile_id.to_owned(), layout.pty_size, cx);
        cx.notify();
    }

    fn schedule_terminal_resize(
        &mut self,
        profile_id: String,
        size: PtySize,
        cx: &mut Context<Self>,
    ) {
        // Keep local reflow and the remote PTY on the same final size after live resizing settles.
        self.terminal_resize_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(TERMINAL_RESIZE_DEBOUNCE).await;

            let _ = this.update(cx, |this, cx| {
                let is_current_size = this.terminal.as_ref().is_some_and(|terminal| {
                    terminal.profile_id == profile_id && terminal.pending_pty_size == Some(size)
                });
                if !is_current_size
                    || this.connection_profile_id.as_deref() != Some(profile_id.as_str())
                {
                    return;
                }

                let Some(handle) = this.connection_handle.as_ref() else {
                    return;
                };

                if let Err(error) = handle.resize(size) {
                    this.connection_error = Some(error.to_string());
                    cx.notify();
                }
            });
        }));
    }

    fn on_terminal_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.cell_height)
            .unwrap_or(f32::from(TERMINAL_CELL_HEIGHT));
        let delta = f32::from(event.delta.pixel_delta(px(line_height)).y);
        if delta == 0.0 {
            return;
        }
        cx.stop_propagation();

        if self.terminal_scroll_accumulator.signum() != delta.signum() {
            self.terminal_scroll_accumulator = 0.0;
        }
        self.terminal_scroll_accumulator += delta;

        let lines = (self.terminal_scroll_accumulator / line_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        self.terminal_scroll_accumulator -= lines as f32 * line_height;

        let modes = self.terminal_modes();
        let display_offset = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.engine.display_offset())
            .unwrap_or_default();
        let alternate_scroll = should_translate_alternate_scroll(modes, display_offset);

        if alternate_scroll {
            self.clear_terminal_selection();
            self.send_terminal_input(encode_alternate_scroll(lines, modes), cx);
        } else if let Some(terminal) = self.terminal.as_mut() {
            self.terminal_selection = None;
            self.terminal_selecting = false;
            terminal.engine.scroll(TerminalScroll::Lines(lines));
            cx.notify();
        }
    }

    fn process_terminal_output(&mut self, data: &[u8], cx: &mut Context<Self>) {
        if !data.is_empty()
            && self
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.engine.display_offset() == 0)
        {
            self.terminal_selection = None;
            self.terminal_selecting = false;
        }

        let events = self
            .terminal
            .as_mut()
            .map(|terminal| terminal.process(data))
            .unwrap_or_default();

        for event in events {
            self.handle_terminal_event(event, cx);
        }
    }

    fn handle_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::TitleChanged(title) => {
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.title = title;
                }
            }
            TerminalEvent::ClipboardStore {
                clipboard,
                contents,
            } => self.write_terminal_clipboard(clipboard, contents, cx),
            TerminalEvent::ClipboardLoad(request) => {
                let contents = self
                    .read_terminal_clipboard(request.clipboard, cx)
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.send_terminal_response(request.response(&contents));
            }
            TerminalEvent::ColorRequest(request) => {
                let palette = self.terminal_palette();
                if let Some(terminal) = self.terminal.as_ref() {
                    let snapshot = terminal.snapshot();
                    let color = palette_color(&snapshot, request.index, palette).into();
                    self.send_terminal_response(request.response(color));
                }
            }
            TerminalEvent::WriteToPty(data) => self.send_terminal_response(data),
            TerminalEvent::TextAreaSizeRequest(request) => {
                if let Some(terminal) = self.terminal.as_ref() {
                    self.send_terminal_response(request.response(terminal.text_area_size()));
                }
            }
            TerminalEvent::Bell => {
                self.connection_message = Some("Remote terminal bell".into());
            }
            TerminalEvent::ExitRequested => {
                self.connection_message = Some("Remote terminal requested exit".into());
                self.terminal_end_reason = Some("Remote terminal requested exit".into());
            }
            TerminalEvent::ChildExited(status) => {
                self.terminal_end_reason = status
                    .map(|status| format!("Remote terminal exited with status {status}"))
                    .or_else(|| Some("Remote terminal exited".into()));
                self.connection_message
                    .clone_from(&self.terminal_end_reason);
            }
            TerminalEvent::MouseCursorDirty
            | TerminalEvent::CursorBlinkingChanged
            | TerminalEvent::Wakeup => {}
        }
    }

    fn send_terminal_response(&mut self, data: Vec<u8>) {
        let Some(handle) = self.connection_handle.as_ref() else {
            return;
        };

        if let Err(error) = handle.send_input(data) {
            self.connection_error = Some(error.to_string());
        }
    }

    fn write_terminal_clipboard(
        &self,
        clipboard: TerminalClipboard,
        contents: String,
        cx: &mut Context<Self>,
    ) {
        let item = ClipboardItem::new_string(contents);

        match clipboard {
            TerminalClipboard::Clipboard => cx.write_to_clipboard(item),
            TerminalClipboard::Selection => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                cx.write_to_primary(item);
                #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                cx.write_to_clipboard(item);
            }
        }
    }

    fn read_terminal_clipboard(
        &self,
        clipboard: TerminalClipboard,
        cx: &mut Context<Self>,
    ) -> Option<ClipboardItem> {
        match clipboard {
            TerminalClipboard::Clipboard => cx.read_from_clipboard(),
            TerminalClipboard::Selection => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                {
                    cx.read_from_primary()
                }
                #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
                {
                    cx.read_from_clipboard()
                }
            }
        }
    }

    fn handle_connection_event(&mut self, event: ConnectionEvent, cx: &mut Context<Self>) {
        let should_notify = match event {
            ConnectionEvent::StateChanged(state) => {
                let previous_state = self.connection_state;
                self.connection_state = state;

                if matches!(
                    state,
                    SessionState::Authenticating | SessionState::Connected
                ) {
                    self.host_key_prompt = None;
                    self.connection_message = None;
                }

                if state == SessionState::Connected
                    && let Some(terminal) = self.terminal.as_mut()
                {
                    terminal.was_connected = true;
                }

                if state == SessionState::Disconnected {
                    self.host_key_prompt = None;
                    self.terminal_resize_task = None;
                    self.connection_handle = None;
                    self.connection_profile_id = None;
                    if previous_state == SessionState::Disconnecting
                        && self.terminal_end_reason.is_none()
                    {
                        self.terminal_end_reason = Some("Session disconnected".into());
                    }
                }

                true
            }
            ConnectionEvent::HostKeyVerificationRequired(info) => {
                self.connection_message = Some(format!("Verify host key for {}", info.address()));
                self.host_key_prompt = Some(info);
                true
            }
            ConnectionEvent::Failed(error) => {
                let failed_profile_id = self.connection_profile_id.take();
                self.connection_state = SessionState::Failed;
                self.terminal_resize_task = None;
                self.connection_handle = None;
                self.host_key_prompt = None;

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
            ConnectionEvent::Resized(size) => {
                let dimensions_changed = self
                    .terminal
                    .as_mut()
                    .filter(|terminal| {
                        self.connection_profile_id.as_deref() == Some(terminal.profile_id.as_str())
                    })
                    .is_some_and(|terminal| terminal.acknowledge_resize(size));
                if dimensions_changed {
                    self.terminal_selection = None;
                    self.terminal_selecting = false;
                }
                true
            }
            ConnectionEvent::Shell(
                ShellEvent::Output(data) | ShellEvent::ExtendedOutput { data, .. },
            ) => {
                self.process_terminal_output(&data, cx);
                true
            }
            ConnectionEvent::Shell(ShellEvent::ExitStatus(status)) => {
                let message = format!("Remote shell exited with status {status}");
                self.connection_message = Some(message.clone());
                self.terminal_end_reason = Some(message);
                true
            }
            ConnectionEvent::Shell(ShellEvent::ExitSignal {
                signal,
                core_dumped,
                message,
            }) => {
                let core_dump = if core_dumped { " (core dumped)" } else { "" };
                let message =
                    format!("Remote shell exited on signal {signal}{core_dump}: {message}");
                self.connection_message = Some(message.clone());
                self.terminal_end_reason = Some(message);
                true
            }
            ConnectionEvent::Shell(ShellEvent::Eof) => {
                self.connection_message = Some("Remote shell reached EOF".into());
                if self.terminal_end_reason.is_none() {
                    self.terminal_end_reason = Some("Remote shell reached EOF".into());
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Closed) => {
                self.connection_message = Some("Remote shell closed".into());
                if self.terminal_end_reason.is_none() {
                    self.terminal_end_reason = Some("Remote shell closed".into());
                }
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
        let should_focus_terminal = self.active_panel == ActivePanel::Connection
            && selected_profile
                .as_ref()
                .is_some_and(|profile| self.is_terminal_visible(&profile.id));

        let mut root = div()
            .relative()
            .flex()
            .size_full()
            .text_color(self.theme.text_primary)
            .child(self.render_sidebar(cx))
            .child(self.render_detail_panel(selected_profile, cx));

        if self.host_key_prompt.is_some() {
            root = root.child(self.render_host_key_prompt(cx));
        } else if let Some(prompt) = self.credential_prompt.as_ref() {
            let focus_handle = prompt.input.focus_handle(cx);
            if !focus_handle.is_focused(window) {
                window.focus(&focus_handle);
            }

            root = root.child(self.render_credential_prompt(cx));
        } else if should_focus_terminal && !self.terminal_focus_handle.is_focused(window) {
            window.focus(&self.terminal_focus_handle);
        }

        root
    }
}

impl RemCmdApp {
    fn is_terminal_visible(&self, profile_id: &str) -> bool {
        let active_connection = self.connection_profile_id.as_deref() == Some(profile_id)
            && !self.connection_state.can_connect();

        self.terminal.as_ref().is_some_and(|terminal| {
            terminal.profile_id == profile_id && (active_connection || terminal.was_connected)
        })
    }

    fn terminal_has_ended(&self, profile_id: &str) -> bool {
        self.connection_state.can_connect()
            && self.connection_profile_id.as_deref() != Some(profile_id)
            && self
                .terminal
                .as_ref()
                .is_some_and(|terminal| terminal.profile_id == profile_id && terminal.was_connected)
    }

    fn close_terminal(&mut self, cx: &mut Context<Self>) {
        if self.connection_state.can_disconnect() {
            return;
        }

        self.terminal = None;
        self.terminal_marked_text.clear();
        self.terminal_selection = None;
        self.terminal_selecting = false;
        self.terminal_scroll_accumulator = 0.0;
        self.terminal_resize_task = None;
        cx.notify();
    }

    fn render_terminal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.terminal_palette();
        let cell_height = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.cell_height)
            .unwrap_or(f32::from(TERMINAL_CELL_HEIGHT));
        let model = self.terminal.as_ref().map(|terminal| {
            TerminalViewModel::from_snapshot_with_selection(
                &terminal.snapshot(),
                self.terminal_selection,
                palette,
            )
        });

        let profile_id = self
            .terminal
            .as_ref()
            .map(|terminal| terminal.profile_id.clone())
            .unwrap_or_default();
        let input_entity = cx.entity();
        let layout_entity = input_entity.clone();
        let input_focus_handle = self.terminal_focus_handle.clone();
        let input_layer = canvas(
            move |bounds, window, _| {
                let metrics = TerminalCellMetrics::measure(window);
                let layout = terminal_layout_for_pixels(
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                    metrics.width,
                    metrics.height,
                );
                let frame = model.map(|model| TerminalCanvasFrame::prepare(model, metrics, window));

                (layout, frame)
            },
            move |bounds, (layout, frame), window, cx| {
                window.handle_input(
                    &input_focus_handle,
                    ElementInputHandler::new(bounds, input_entity),
                    cx,
                );

                if let Some(frame) = frame {
                    frame.paint(bounds, window, cx);
                }

                cx.defer(move |cx| {
                    layout_entity.update(cx, |this, cx| {
                        this.apply_terminal_layout(&profile_id, bounds, layout, cx);
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        div()
            .id("terminal_view")
            .key_context("Terminal")
            .track_focus(&self.terminal_focus_handle)
            .flex_1()
            .w_full()
            .mt_4()
            .p_3()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(rgb(palette.background.hex()))
            .font_family(TERMINAL_FONT_FAMILY)
            .text_size(px(14.0))
            .line_height(px(cell_height))
            .cursor(CursorStyle::IBeam)
            .focus(|style| style.border_color(self.theme.border_strong))
            .on_key_down(cx.listener(Self::on_terminal_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_terminal_mouse_down))
            .on_mouse_move(cx.listener(Self::on_terminal_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_terminal_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_terminal_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_terminal_scroll))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(input_layer),
            )
    }

    fn render_terminal_lifecycle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (message, color) = if let Some(error) = self.connection_error.as_ref() {
            (error.clone(), self.theme.error_text)
        } else if let Some(message) = self.terminal_end_reason.as_ref() {
            (message.clone(), self.theme.text_muted)
        } else {
            ("Session ended".into(), self.theme.text_muted)
        };

        div()
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .mt_2()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.control_bg)
            .child(
                div()
                    .flex_1()
                    .min_w(px(120.0))
                    .truncate()
                    .text_sm()
                    .text_color(color)
                    .child(message),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(
                        button(
                            "terminal_reconnect",
                            "Reconnect",
                            ButtonVariant::Primary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.connect_selected_profile(window, cx);
                        })),
                    )
                    .child(
                        button(
                            "terminal_close",
                            "Close terminal",
                            ButtonVariant::Secondary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_terminal(cx);
                        })),
                    ),
            )
    }

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
            .w_full()
            .max_w(px(420.0))
            .mx_4()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.modal_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(24.0),
                spread_radius: px(-8.0),
            }])
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
            .child(
                div()
                    .mt_2()
                    .rounded_md()
                    .border_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.surface_bg)
                    .child(prompt.input.clone()),
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
                    button(
                        "credential_cancel",
                        "Cancel",
                        ButtonVariant::Secondary,
                        true,
                        &self.theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dismiss_credential_prompt(cx);
                        cx.notify();
                    })),
                )
                .child(
                    button(
                        "credential_submit",
                        "Connect",
                        ButtonVariant::Primary,
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

    fn render_host_key_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self
            .host_key_prompt
            .as_ref()
            .expect("host-key prompt should exist before rendering");

        let modal = div()
            .w_full()
            .max_w(px(500.0))
            .mx_4()
            .p_4()
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.modal_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(24.0),
                spread_radius: px(-8.0),
            }])
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Verify host key"),
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
                    .child("This server is not recorded in known_hosts. Verify its fingerprint before connecting."),
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
                            .child("Algorithm"),
                    )
                    .child(div().child(info.algorithm().to_owned()))
            )
            .child(
                div()
                    .mt_3()
                    .text_sm()
                    .text_color(self.theme.text_faint)
                    .child("SHA-256 fingerprint"),
            )
            .child(
                div()
                    .mt_1()
                    .w_full()
                    .font_family(TERMINAL_FONT_FAMILY)
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
                        button(
                            "host_key_cancel",
                            "Cancel",
                            ButtonVariant::Secondary,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.reject_pending_host_key(cx);
                        })),
                    )
                    .child(
                        button(
                            "host_key_trust",
                            "Trust and connect",
                            ButtonVariant::Primary,
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

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut connection_list = div()
            .id("connection_list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .mt_2();

        for profile in &self.profiles {
            let profile_id = profile.id.clone();
            let is_selected = self.active_panel == ActivePanel::Connection
                && self.selected_profile_id.as_ref() == Some(&profile.id);
            let item_background = if is_selected {
                self.theme.list_selected_bg
            } else {
                self.theme.transparent
            };
            let hover_background = if is_selected {
                self.theme.list_selected_hover_bg
            } else {
                self.theme.list_hover_bg
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
                .hover(move |this| this.bg(hover_background))
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
                        .text_color(self.theme.text_muted)
                        .child(profile.address()),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_profile(profile_id.clone(), cx);
                }));

            connection_list = connection_list.child(profile_item);
        }

        let settings_selected = self.active_panel == ActivePanel::Settings;
        let settings_background = if settings_selected {
            self.theme.list_selected_bg
        } else {
            self.theme.transparent
        };
        let settings_hover = if settings_selected {
            self.theme.list_selected_hover_bg
        } else {
            self.theme.list_hover_bg
        };
        let settings_footer = div()
            .flex_none()
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .id("show_settings")
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(settings_background)
                    .cursor_pointer()
                    .hover(move |this| this.bg(settings_hover))
                    .child(
                        div()
                            .w(px(18.0))
                            .text_center()
                            .text_size(px(16.0))
                            .child("\u{2699}"),
                    )
                    .child("Settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_settings(cx);
                    })),
            );

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(300.0))
            .h_full()
            .bg(self.theme.sidebar_bg)
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
                        button(
                            "add_connection",
                            "Add",
                            ButtonVariant::Ghost,
                            true,
                            &self.theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.add_profile(cx);
                        })),
                    ),
            )
            .child(connection_list)
            .child(settings_footer)
    }

    fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self.active_panel == ActivePanel::Settings {
            return self.render_settings(cx);
        }

        let mut panel = self.detail_panel_shell();

        match selected_profile {
            Some(profile) => {
                let Some(editor) = self.editor.as_ref() else {
                    return panel.child("No editor loaded");
                };

                panel = panel.child(
                    div()
                        .flex()
                        .flex_none()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(120.0))
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(profile.name.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .flex_wrap()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(self.render_connection_controls(cx))
                                .child(
                                    button(
                                        "delete_connection",
                                        "Delete",
                                        ButtonVariant::Danger,
                                        true,
                                        &self.theme,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.delete_selected_profile(cx);
                                        },
                                    )),
                                ),
                        ),
                );

                if self.is_terminal_visible(&profile.id) {
                    panel = panel.child(self.render_terminal(cx));
                    if self.terminal_has_ended(&profile.id) {
                        panel = panel.child(self.render_terminal_lifecycle(cx));
                    }
                } else {
                    let form = div()
                        .id("connection_form")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(0.0))
                        .overflow_x_hidden()
                        .overflow_y_scroll()
                        .pr_1()
                        .child(self.render_form_row("Name", editor.name.clone()))
                        .child(self.render_form_row("Host", editor.host.clone()))
                        .child(self.render_form_row("Port", editor.port.clone()))
                        .child(self.render_form_row("Username", editor.username.clone()))
                        .child(self.render_auth_method_row(editor.auth_kind, cx))
                        .when(editor.auth_kind == ProfileAuthKind::PrivateKey, |this| {
                            this.child(
                                self.render_private_key_row(editor.private_key_path.clone(), cx),
                            )
                        })
                        .when_some(self.form_error.as_ref(), |this, error| {
                            this.child(
                                div()
                                    .mt_3()
                                    .text_color(self.theme.error_text)
                                    .child(error.clone()),
                            )
                        })
                        .when_some(self.connection_error.as_ref(), |this, error| {
                            this.child(
                                div()
                                    .mt_3()
                                    .text_color(self.theme.error_text)
                                    .child(error.clone()),
                            )
                        })
                        .when_some(self.connection_message.as_ref(), |this, message| {
                            this.child(
                                div()
                                    .mt_3()
                                    .text_color(self.theme.text_muted)
                                    .child(message.clone()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .flex_wrap()
                                .mt_6()
                                .gap_2()
                                .pb_2()
                                .child(
                                    button(
                                        "save_profile",
                                        "Save",
                                        ButtonVariant::Primary,
                                        true,
                                        &self.theme,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.save_editor(cx);
                                        },
                                    )),
                                )
                                .child(
                                    button(
                                        "cancel_profile",
                                        "Cancel",
                                        ButtonVariant::Secondary,
                                        true,
                                        &self.theme,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.cancel_editor(cx);
                                        },
                                    )),
                                ),
                        );
                    panel = panel.child(form);
                }
            }
            None => {
                panel = panel.child(
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_color(self.theme.text_muted)
                        .child("No connection selected"),
                );
            }
        }

        panel
    }

    fn detail_panel_shell(&self) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .px_4()
            .pb_4()
            .pt(px(52.0))
            .bg(self.theme.panel_bg)
            .border_l_1()
            .border_color(self.theme.border_strong)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(-1.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }])
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let appearance_control = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .p(px(2.0))
            .gap(px(2.0))
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.control_bg)
            .child(self.render_theme_mode_option("theme_system", "System", ThemeMode::System, cx))
            .child(self.render_theme_mode_option("theme_light", "Light", ThemeMode::Light, cx))
            .child(self.render_theme_mode_option("theme_dark", "Dark", ThemeMode::Dark, cx));

        let content = div()
            .id("settings_content")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .pr_1()
            .child(
                div()
                    .mt_6()
                    .mb_3()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.theme.text_muted)
                    .child("Appearance"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_none().w(px(112.0)).truncate().child("Theme"))
                    .child(appearance_control),
            )
            .when_some(self.settings_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .mt_3()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            });

        self.detail_panel_shell()
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Settings"),
            )
            .child(content)
    }

    fn render_theme_mode_option(
        &self,
        id: &'static str,
        label: &'static str,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = self.theme_mode == mode;
        let background = if is_selected {
            self.theme.list_selected_bg
        } else {
            self.theme.transparent
        };
        let border = if is_selected {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };
        let hover_background = if is_selected {
            self.theme.list_selected_hover_bg
        } else {
            self.theme.control_hover_bg
        };

        div()
            .id(id)
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .items_center()
            .justify_center()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .text_sm()
            .cursor_pointer()
            .hover(move |this| this.bg(hover_background))
            .when(is_selected, |this| {
                this.shadow(vec![BoxShadow {
                    color: self.theme.shadow,
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(3.0),
                    spread_radius: px(-1.0),
                }])
            })
            .child(div().truncate().child(label))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.set_theme_mode(mode, window, cx);
            }))
    }

    fn render_connection_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_connect = self.connection_state.can_connect();
        let can_disconnect = self.connection_state.can_disconnect();

        let label = match self.connection_state {
            SessionState::Failed => "Retry",
            SessionState::Disconnecting => "Disconnecting",
            _ if can_disconnect => "Disconnect",
            _ if self
                .selected_profile_id
                .as_deref()
                .is_some_and(|profile_id| self.terminal_has_ended(profile_id)) =>
            {
                "Reconnect"
            }
            _ => "Connect",
        };

        let button_variant = if can_disconnect {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Primary
        };

        let status_color = match self.connection_state {
            SessionState::Connected => self.theme.status_ok,
            SessionState::Failed => self.theme.error_text,
            SessionState::Connecting
            | SessionState::Authenticating
            | SessionState::Disconnecting => self.theme.status_warn,
            SessionState::Disconnected => self.theme.text_muted,
        };

        let mut action = button(
            "connection_action",
            label,
            button_variant,
            can_connect || can_disconnect,
            &self.theme,
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
            .justify_end()
            .gap_2()
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
            .child(
                div()
                    .flex_none()
                    .w(px(112.0))
                    .truncate()
                    .child("Authentication"),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .p(px(2.0))
                    .gap(px(2.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.control_bg)
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
            self.theme.list_selected_bg
        } else {
            self.theme.transparent
        };
        let border = if is_selected {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };
        let hover_background = if is_selected {
            self.theme.list_selected_hover_bg
        } else {
            self.theme.control_hover_bg
        };

        div()
            .id(id)
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .items_center()
            .justify_center()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .text_sm()
            .cursor_pointer()
            .hover(move |this| this.bg(hover_background))
            .when(is_selected, |this| {
                this.shadow(vec![BoxShadow {
                    color: self.theme.shadow,
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(3.0),
                    spread_radius: px(-1.0),
                }])
            })
            .child(div().truncate().child(label))
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
            .child(div().flex_none().w(px(112.0)).truncate().child("Key file"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .rounded_lg()
                            .border_1()
                            .border_color(self.theme.border)
                            .bg(self.theme.surface_bg)
                            .child(field),
                    )
                    .child(
                        button(
                            "browse_private_key",
                            "Browse",
                            ButtonVariant::Secondary,
                            true,
                            &self.theme,
                        )
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
            .child(div().flex_none().w(px(112.0)).truncate().child(label))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .rounded_lg()
                    .border_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.surface_bg)
                    .child(field),
            )
    }
}

impl EntityInputHandler for RemCmdApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16_len = self.terminal_marked_text.encode_utf16().count();
        let start_utf16 = range_utf16.start.min(utf16_len);
        let end_utf16 = range_utf16.end.clamp(start_utf16, utf16_len);
        let start = utf16_offset_to_utf8(&self.terminal_marked_text, start_utf16);
        let end = utf16_offset_to_utf8(&self.terminal_marked_text, end_utf16);
        let adjusted_start = self.terminal_marked_text[..start].encode_utf16().count();
        let adjusted_end = self.terminal_marked_text[..end].encode_utf16().count();

        adjusted_range.replace(adjusted_start..adjusted_end);
        Some(self.terminal_marked_text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.terminal_marked_text.encode_utf16().count();
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let len = self.terminal_marked_text.encode_utf16().count();
        (len != 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.terminal_marked_text);
        self.send_terminal_user_input(text.into_bytes(), cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_marked_text.clear();
        self.send_terminal_user_input(new_text.as_bytes().to_vec(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        new_text.clone_into(&mut self.terminal_marked_text);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let terminal = self.terminal.as_ref()?;
        let cursor = terminal.snapshot().cursor;
        let (row, column) = cursor
            .map(|cursor| (cursor.row, cursor.column))
            .unwrap_or_default();

        Some(Bounds::new(
            point(
                element_bounds.left() + px(column as f32 * terminal.cell_width),
                element_bounds.top() + px(row as f32 * terminal.cell_height),
            ),
            size(px(terminal.cell_width), px(terminal.cell_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.terminal_marked_text.encode_utf16().count())
    }
}

fn terminal_layout_for_pixels(
    viewport_width: f32,
    viewport_height: f32,
    measured_cell_width: f32,
    measured_cell_height: f32,
) -> TerminalLayout {
    let cell_width = valid_dimension(measured_cell_width, f32::from(TERMINAL_CELL_WIDTH));
    let cell_height = valid_dimension(measured_cell_height, f32::from(TERMINAL_CELL_HEIGHT));
    let columns = cell_count(viewport_width, cell_width);
    let rows = cell_count(viewport_height, cell_height);

    TerminalLayout {
        pty_size: PtySize::new(columns, rows).with_pixels(
            pixel_dimension(viewport_width),
            pixel_dimension(viewport_height),
        ),
        cell_width,
        cell_height,
    }
}

fn terminal_point_for_pixels(
    x: f32,
    y: f32,
    columns: usize,
    rows: usize,
    cell_width: f32,
    cell_height: f32,
) -> TerminalPoint {
    let cell_width = valid_dimension(cell_width, f32::from(TERMINAL_CELL_WIDTH));
    let cell_height = valid_dimension(cell_height, f32::from(TERMINAL_CELL_HEIGHT));
    let column = (x.max(0.0) / cell_width).round() as usize;
    let row = (y.max(0.0) / cell_height).floor() as usize;

    TerminalPoint::new(row.min(rows.saturating_sub(1)), column.min(columns))
}

fn valid_dimension(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn cell_count(viewport: f32, cell: f32) -> u32 {
    (valid_dimension(viewport, cell) / cell)
        .floor()
        .clamp(1.0, u32::MAX as f32) as u32
}

fn pixel_dimension(value: f32) -> u32 {
    valid_dimension(value, 1.0)
        .floor()
        .clamp(1.0, u32::MAX as f32) as u32
}

fn pixel_cell_dimension(value: f32) -> u16 {
    value.round().clamp(1.0, f32::from(u16::MAX)) as u16
}

fn utf16_offset_to_utf8(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;

    for (utf8_offset, character) in text.char_indices() {
        if utf16_offset >= offset || utf16_offset + character.len_utf16() > offset {
            return utf8_offset;
        }
        utf16_offset += character.len_utf16();
    }

    text.len()
}

fn is_terminal_paste_shortcut(keystroke: &Keystroke) -> bool {
    if keystroke.key == "insert" && keystroke.modifiers.shift {
        return true;
    }

    #[cfg(target_os = "macos")]
    {
        keystroke.key == "v" && keystroke.modifiers.platform
    }

    #[cfg(target_os = "windows")]
    {
        keystroke.key == "v" && keystroke.modifiers.control
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        keystroke.key == "v" && keystroke.modifiers.control && keystroke.modifiers.shift
    }
}

fn is_terminal_copy_shortcut(keystroke: &Keystroke) -> bool {
    if keystroke.key != "c" {
        return false;
    }

    #[cfg(target_os = "macos")]
    {
        keystroke.modifiers.platform
    }

    #[cfg(target_os = "windows")]
    {
        keystroke.modifiers.control
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        keystroke.modifiers.control && keystroke.modifiers.shift
    }
}

// Application startup functions stay outside main so startup remains testable and readable.
fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.0), px(480.0))),
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

    cx.open_window(options, |window, cx| {
        cx.new(|cx| RemCmdApp::load(window, cx))
    })
    .expect("failed to open main window");
}

fn bind_credential_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitCredential, Some("CredentialPrompt")),
        KeyBinding::new("escape", CancelCredential, Some("CredentialPrompt")),
    ]);
}

fn bind_host_key_prompt_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelHostKeyVerification,
        Some("HostKeyPrompt"),
    )]);
}

fn launch(cx: &mut App) {
    cx.set_global(SshRuntime::new().expect("failed to create SSH runtime"));

    bind_text_field_keys(cx);
    bind_credential_prompt_keys(cx);
    bind_host_key_prompt_keys(cx);
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

    #[test]
    fn terminal_layout_uses_measured_cells_and_viewport_pixels() {
        let layout = terminal_layout_for_pixels(803.0, 479.0, 8.0, 19.0);

        assert_eq!(layout.pty_size, PtySize::new(100, 25).with_pixels(803, 479));
        assert_eq!(layout.cell_width, 8.0);
        assert_eq!(layout.cell_height, 19.0);
    }

    #[test]
    fn terminal_layout_never_reports_an_empty_pty() {
        let layout = terminal_layout_for_pixels(0.0, 0.0, 0.0, f32::NAN);

        assert_eq!(layout.pty_size.columns, 1);
        assert_eq!(layout.pty_size.rows, 1);
        assert_eq!(layout.pty_size.pixel_width, 1);
        assert_eq!(layout.pty_size.pixel_height, 1);
        assert_eq!(layout.cell_width, f32::from(TERMINAL_CELL_WIDTH));
        assert_eq!(layout.cell_height, f32::from(TERMINAL_CELL_HEIGHT));
    }

    #[test]
    fn terminal_resize_ignores_intermediate_live_sizes() {
        let initial_size = PtySize::new(80, 24);
        let final_size = initial_size.with_pixels(640, 456);
        let mut terminal = ActiveTerminal::new("profile-1".into(), initial_size);
        terminal.process(b"first prompt\r\nsecond prompt");
        let initial_snapshot = terminal.snapshot();

        assert!(terminal.stage_resize(PtySize::new(48, 18).with_pixels(384, 342)));
        assert!(terminal.stage_resize(final_size));
        assert_eq!(terminal.pty_size, initial_size);
        assert_eq!(terminal.snapshot(), initial_snapshot);

        assert!(!terminal.acknowledge_resize(final_size));
        assert_eq!(terminal.pty_size, final_size);
        assert_eq!(terminal.snapshot(), initial_snapshot);
    }

    #[test]
    fn terminal_resize_tracks_stale_acknowledgements_without_losing_final_target() {
        let initial_size = PtySize::new(80, 24);
        let narrow_size = PtySize::new(48, 18).with_pixels(384, 342);
        let final_size = PtySize::new(100, 30).with_pixels(800, 570);
        let mut terminal = ActiveTerminal::new("profile-1".into(), initial_size);

        assert!(terminal.stage_resize(narrow_size));
        assert!(terminal.stage_resize(final_size));
        assert!(terminal.acknowledge_resize(narrow_size));
        assert_eq!(terminal.pty_size, narrow_size);
        assert_eq!(terminal.pending_pty_size, Some(final_size));

        assert!(terminal.acknowledge_resize(final_size));
        assert_eq!(terminal.pty_size, final_size);
        assert_eq!(terminal.pending_pty_size, None);
    }

    #[test]
    fn terminal_resize_reflows_only_when_the_final_grid_changes() {
        let mut terminal = ActiveTerminal::new("profile-1".into(), PtySize::new(80, 24));
        let final_size = PtySize::new(48, 18).with_pixels(384, 342);

        assert!(terminal.stage_resize(final_size));
        assert!(terminal.acknowledge_resize(final_size));
        assert_eq!(terminal.engine.size().columns(), 48);
        assert_eq!(terminal.engine.size().rows(), 18);
        assert_eq!(terminal.pending_pty_size, None);
    }

    #[test]
    fn terminal_mouse_positions_snap_to_character_boundaries() {
        assert_eq!(
            terminal_point_for_pixels(3.9, 18.9, 80, 24, 8.0, 19.0),
            TerminalPoint::new(0, 0)
        );
        assert_eq!(
            terminal_point_for_pixels(4.1, 19.0, 80, 24, 8.0, 19.0),
            TerminalPoint::new(1, 1)
        );
        assert_eq!(
            terminal_point_for_pixels(10_000.0, 10_000.0, 80, 24, 8.0, 19.0),
            TerminalPoint::new(23, 80)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn command_c_is_the_terminal_copy_shortcut_on_macos() {
        assert!(is_terminal_copy_shortcut(
            &Keystroke::parse("cmd-c").unwrap()
        ));
        assert!(!is_terminal_copy_shortcut(
            &Keystroke::parse("ctrl-c").unwrap()
        ));
    }

    #[test]
    fn utf16_offsets_snap_to_valid_utf8_boundaries() {
        let text = "a\u{1f642}b";

        assert_eq!(utf16_offset_to_utf8(text, 0), 0);
        assert_eq!(utf16_offset_to_utf8(text, 1), 1);
        assert_eq!(utf16_offset_to_utf8(text, 2), 1);
        assert_eq!(utf16_offset_to_utf8(text, 3), 5);
        assert_eq!(utf16_offset_to_utf8(text, 4), 6);
    }
}
