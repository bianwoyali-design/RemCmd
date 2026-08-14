#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod text_field;
use text_field::{TextField, bind_text_field_keys};

mod file_editor;
use file_editor::{FileEditor, FileEditorEvent, bind_file_editor_keys};

mod pane_layout;
use pane_layout::{PaneId, PaneLayout, SplitAxis};

mod icons;
use icons::{IconName, RemCmdAssets, app_icon, icon, icon_with_color, wordmark};

mod i18n;
use i18n::Localizer;

mod ssh_runtime;
use ssh_runtime::SshRuntime;

mod theme;
use theme::{IconTone, TextButtonTone, Theme, icon_button, set_global_theme, text_button};

mod terminal_input;
use terminal_input::{
    encode_alternate_scroll, encode_focus, encode_key, encode_paste,
    should_translate_alternate_scroll,
};

mod terminal_canvas;
use terminal_canvas::{
    TerminalCanvasCache, TerminalCanvasFrame, TerminalCanvasInput, TerminalCellMetrics,
};

mod terminal_view;
use terminal_view::{TerminalPalette, palette_color};

mod windows_chrome;

#[cfg(target_os = "macos")]
mod private_key_picker;

#[cfg(target_os = "macos")]
mod macos_symbols;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use gpui::img;
use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Application, Bounds, BoxShadow,
    ClipboardItem, Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, FontWeight, Global, Hsla, IntoElement, KeyBinding, KeyDownEvent,
    Keystroke, Menu, MenuItem, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PathPromptOptions, Pixels, PromptButton, PromptLevel, Render, ScrollHandle, ScrollWheelEvent,
    SharedString, Subscription, Task, Timer, TitlebarOptions, UTF16Selection,
    UniformListScrollHandle, Window, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    WindowHandle, WindowOptions, canvas, deferred, div, ease_in_out, ease_out_quint, point,
    prelude::*, px, rgb, size, uniform_list,
};
use secrecy::{ExposeSecret, SecretString};

use remcmd_core::{
    AuthConfig, ConnectionProfile, ConnectionRoute, LanguageMode, ProxyConfig, TabLayout,
    TerminalSettings, ThemeMode, TransferSettings,
};
use remcmd_local::{LocalPtySize, LocalTerminal, LocalTerminalEvent, LocalTerminalHandle};
#[cfg(test)]
use remcmd_ssh::LogicalCpuSnapshot;
use remcmd_ssh::{
    AuthMethod, ConnectionEvent, ConnectionHandle, ConnectionPlan, ConnectionStage, ConnectionStep,
    HostKeyInfo, MAX_REMOTE_FILE_BYTES, PtySize, RemoteDirectory, RemoteDirectoryTree, RemoteFile,
    RemoteFileEntry, RemoteFileKind, RuntimeProxy, ServerPerformanceSnapshot, SessionState,
    SftpOperation, SftpTransferDirection, ShellEvent, SshConnection, SshError, SshErrorKind,
    TransferRateLimiter, proxy_command_content_digest,
};
use remcmd_storage::{
    AppSettings, CredentialKind, OpenSshImportPreview, OpenSshImportStatus, apply_openssh_import,
    default_openssh_config_path, default_profiles_path, default_settings_path, delete_credential,
    delete_profile_auth_credentials, delete_profile_credentials, ensure_profiles_file,
    load_credential, load_profiles, load_settings, preview_openssh_import, save_credential,
    save_profiles, save_profiles_with_route_secrets, save_settings,
};
use remcmd_terminal::{
    Clipboard as TerminalClipboard, Scroll as TerminalScroll, TerminalDamage, TerminalEngine,
    TerminalEvent, TerminalModes, TerminalPoint, TerminalSelection, TerminalSnapshot, TextAreaSize,
};

const TERMINAL_COLUMNS: u32 = 80;
const TERMINAL_ROWS: u32 = 24;
const LOCAL_PROFILE_ID: &str = "__remcmd_local_terminal__";
const TERMINAL_CELL_WIDTH: u16 = 8;
const TERMINAL_CELL_HEIGHT: u16 = 19;
const TERMINAL_RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
const TERMINAL_REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const TERMINAL_EVENT_BATCH_LIMIT: usize = 64;
const MOTION_INSTANT_DURATION: Duration = Duration::from_millis(1);
const MOTION_FAST_DURATION: Duration = Duration::from_millis(120);
const MOTION_STANDARD_DURATION: Duration = Duration::from_millis(180);
const MOTION_EMPHASIZED_DURATION: Duration = Duration::from_millis(240);
const SELECT_MENU_ROW_HEIGHT: f32 = 28.0;
const SELECT_MENU_MAX_VISIBLE_ROWS: usize = 9;
const SFTP_ERROR_HINT_DURATION: Duration = Duration::from_secs(3);
const SIDEBAR_DEFAULT_WIDTH: f32 = 300.0;
const SIDEBAR_MIN_WIDTH: f32 = 220.0;
const SIDEBAR_MAX_WIDTH: f32 = 480.0;
const SIDEBAR_SFTP_REQUEST_ID_START: u64 = 1 << 63;
const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;
const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 340.0;
const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 260.0;
const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 520.0;
const MIN_DETAIL_PANEL_WIDTH: f32 = 180.0;
const BOTTOM_PANEL_DEFAULT_HEIGHT: f32 = 240.0;
const BOTTOM_PANEL_MIN_HEIGHT: f32 = 140.0;
const BOTTOM_PANEL_MAX_HEIGHT: f32 = 520.0;
const BOTTOM_PANEL_HEADER_HEIGHT: f32 = 34.0;
const PROFILE_FORM_LABEL_WIDTH: f32 = 128.0;
const COLLAPSED_TITLEBAR_LEADING_WIDTH: f32 = 140.0;
const TITLEBAR_HEIGHT: f32 = 52.0;
const TITLEBAR_TAB_HEIGHT: f32 = 30.0;
const TITLEBAR_TAB_GROUP_HEIGHT: f32 = 36.0;
const TITLEBAR_ACTION_GROUP_WIDTH: f32 = 112.0;
const TITLEBAR_CONTROL_HOVER_SIZE: f32 = 28.0;
const TITLEBAR_ADD_ICON_SIZE: f32 = 16.0;
const TITLEBAR_SIDEBAR_ICON_SIZE: f32 = 20.0;
const TITLEBAR_LEFT_CONTROL_EDGE_GAP: f32 = 10.0;
const TITLEBAR_EDGE_INSET: f32 = 12.0;
const WINDOWS_CHROME_HEIGHT: f32 = 34.0;
const WINDOWS_BRAND_WIDTH: f32 = 112.0;
const WINDOWS_TITLEBAR_BUTTON_WIDTH: f32 = 46.0;
const WINDOWS_TITLEBAR_CONTROLS_WIDTH: f32 = WINDOWS_TITLEBAR_BUTTON_WIDTH * 3.0;
const WINDOWS_MENU_MIN_WIDTH: f32 = 180.0;
const TITLEBAR_TAB_ICON_ONLY_WIDTH: f32 = 44.0;
const TITLEBAR_TAB_ELLIPSIS_MIN_WIDTH: f32 = 56.0;
const TITLEBAR_ACTIVE_TAB_GROWTH: f32 = 36.0;
const TITLEBAR_CLOSE_SYMBOL_SIZE: f32 = 12.0;
const TRAFFIC_LIGHT_INSET_X: f32 = 20.0;
const TRAFFIC_LIGHT_INSET_Y: f32 = 18.0;
#[cfg(target_os = "macos")]
const UI_MONOSPACE_FONT_FAMILY: &str = "Menlo";
#[cfg(target_os = "windows")]
const UI_MONOSPACE_FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const UI_MONOSPACE_FONT_FAMILY: &str = "DejaVu Sans Mono";
const TERMINAL_FONT_LINE_HEIGHT_FACTOR: f32 = 19.0 / 14.0;

const fn platform_chrome_height() -> f32 {
    if cfg!(target_os = "windows") {
        WINDOWS_CHROME_HEIGHT
    } else {
        0.0
    }
}

const fn content_top_inset() -> f32 {
    TITLEBAR_HEIGHT + platform_chrome_height()
}

gpui::actions!(credential_prompt, [SubmitCredential, CancelCredential]);
gpui::actions!(host_key_prompt, [CancelHostKeyVerification]);
gpui::actions!(settings_selector, [CancelSettingsSelector]);
gpui::actions!(sftp_create_prompt, [SubmitSftpCreate, CancelSftpCreate]);
gpui::actions!(quick_command, [SubmitQuickCommand, CancelQuickCommand]);
gpui::actions!(
    app_menu,
    [
        ShowHome,
        ShowAbout,
        ShowSettings,
        NewConnection,
        NewLocalTerminal,
        NewRemoteTerminal,
        ConnectSelectedProfile,
        DisconnectActiveSession,
        SplitHorizontal,
        SplitVertical,
        CloseActivePane,
        CloseActiveTab,
        ResetActiveTerminal,
        ShowTerminalView,
        ShowFilesView,
        ToggleLeftSidebar,
        ToggleConnectionSearch,
        ShowSftpSidebar,
        ShowPerformanceSidebar,
        ToggleBottomPanel,
        SaveProfileEditor,
        CancelProfileEditor,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullscreen,
        CloseWindow,
        Quit,
    ]
);

struct RemCmdApp {
    profiles: Vec<ConnectionProfile>,
    selected_profile_id: Option<String>,
    next_profile_number: usize,
    editor: Option<ProfileEditor>,
    form_error: Option<String>,
    profiles_path: PathBuf,
    credential_prompt: Option<CredentialPrompt>,
    sessions: Vec<TerminalSession>,
    active_session_id: Option<SessionId>,
    next_session_id: u64,
    tabs: Vec<TerminalTab>,
    active_tab_id: Option<TabId>,
    previous_active_tab_id: Option<TabId>,
    titlebar_tab_transition_id: u64,
    hovered_titlebar_tab_id: Option<TabId>,
    hovered_titlebar_close_id: Option<TabId>,
    titlebar_tabs_scroll_handle: ScrollHandle,
    titlebar_tabs_scroll_transition_id: u64,
    titlebar_tabs_scroll_start: gpui::Point<Pixels>,
    titlebar_tabs_scroll_active: bool,
    titlebar_tabs_scroll_cleanup_task: Option<Task<()>>,
    terminal_redraw_task: Option<Task<()>>,
    next_tab_id: u64,
    panes: Vec<TerminalPane>,
    active_pane_id: Option<PaneId>,
    next_pane_id: u64,
    sidebar_search: Entity<TextField>,
    sidebar_search_visible: bool,
    connections_expanded: bool,
    sidebar_width: f32,
    left_sidebar_open: bool,
    left_sidebar_transition_id: u64,
    sidebar_resize: Option<SidebarResize>,
    right_sidebar_open: bool,
    right_sidebar_width: f32,
    right_sidebar_resize: Option<SidebarResize>,
    right_sidebar_transition_id: u64,
    right_sidebar_rendered: bool,
    right_sidebar_animation_task: Option<Task<()>>,
    right_sidebar_view: RightSidebarView,
    credential_lookup_task: Option<Task<()>>,
    credential_lookup_session_id: Option<SessionId>,
    credential_mutations_in_progress: HashMap<String, usize>,
    pending_connection: Option<PendingConnectionPreparation>,
    pending_proxy_approval: HashSet<SessionId>,
    proxy_command_approval_prompt: Option<ProxyCommandApprovalPrompt>,
    active_panel: ActivePanel,
    language_mode: LanguageMode,
    localizer: Localizer,
    theme_mode: ThemeMode,
    tab_layout: TabLayout,
    terminal_font_family: SharedString,
    terminal_font_families: Vec<SharedString>,
    terminal_font_size: u16,
    transfer_settings: TransferSettings,
    transfer_rate_limiter: Arc<TransferRateLimiter>,
    next_transfer_session_cursor: usize,
    profile_context_menu: Option<ProfileContextMenu>,
    sftp_context_menu: Option<SftpContextMenu>,
    terminal_context_menu: Option<TerminalContextMenu>,
    windows_menu_open: Option<WindowsMenu>,
    sftp_create_prompt: Option<SftpCreatePrompt>,
    quick_command_prompt: Option<QuickCommandPrompt>,
    bottom_panel_open: bool,
    bottom_panel_height: f32,
    bottom_panel_resize: Option<BottomPanelResize>,
    quick_terminal_session_id: Option<SessionId>,
    quick_terminal_focus_handle: FocusHandle,
    focused_terminal_session_id: Option<SessionId>,
    profile_auth_selector_open: bool,
    open_settings_selector: Option<SettingsSelector>,
    settings_selector_scroll_handle: ScrollHandle,
    settings_virtual_selector_scroll_handle: UniformListScrollHandle,
    settings_focus_handle: FocusHandle,
    theme: Theme,
    settings_path: PathBuf,
    settings_error: Option<String>,
    openssh_import_preview: Option<OpenSshImportPreview>,
    openssh_selected_aliases: HashSet<String>,
    openssh_overwrite_conflicts: HashSet<String>,
    openssh_import_loading: bool,
    openssh_import_error: Option<String>,
    about_window: Option<WindowHandle<AboutWindow>>,
    _appearance_subscription: Subscription,
}

/// Keeps menu-bar actions independent from the currently focused GPUI element.
struct RemCmdMainWindow(WindowHandle<RemCmdApp>);

impl Global for RemCmdMainWindow {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsMenu {
    File,
    Edit,
    Terminal,
    View,
    Window,
    Help,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsMenuCommand {
    NewConnection,
    NewLocalTerminal,
    NewRemoteTerminal,
    ConnectSelectedProfile,
    DisconnectActiveSession,
    Edit(EditCommand),
    SplitHorizontal,
    SplitVertical,
    ShowTerminalView,
    ShowFilesView,
    ResetActiveTerminal,
    CloseActivePane,
    CloseActiveTab,
    ShowHome,
    ToggleLeftSidebar,
    ToggleConnectionSearch,
    ShowSftpSidebar,
    ShowPerformanceSidebar,
    ToggleBottomPanel,
    MinimizeWindow,
    ZoomWindow,
    ToggleFullscreen,
    CloseWindow,
    ShowSettings,
    ShowAbout,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowsMenuEntry {
    Item {
        label: &'static str,
        shortcut: &'static str,
        command: WindowsMenuCommand,
    },
    Separator,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SessionId(u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TabId(u64);

struct TerminalTab {
    id: TabId,
    profile_id: String,
    layout: PaneLayout,
    active_pane_id: PaneId,
    view: TerminalTabView,
}

struct TerminalPane {
    id: PaneId,
    tab_id: TabId,
    session_id: SessionId,
    focus_handle: FocusHandle,
    focused: bool,
}

struct TerminalSession {
    id: SessionId,
    profile_id: String,
    kind: TerminalSessionKind,
    close_when_disconnected: bool,
    connection_state: SessionState,
    connection_handle: Option<ConnectionHandle>,
    local_terminal_handle: Option<LocalTerminalHandle>,
    connection_error: Option<String>,
    connection_message: Option<String>,
    terminal_end_reason: Option<String>,
    host_key_prompt: Option<HostKeyInfo>,
    terminal: Option<ActiveTerminal>,
    terminal_marked_text: String,
    terminal_selection: Option<TerminalSelection>,
    terminal_selecting: bool,
    terminal_scroll_accumulator: f32,
    terminal_resize_task: Option<Task<()>>,
    connection_credentials: Vec<ConnectionCredential>,
    sftp_availability: SftpAvailability,
    sftp: SftpBrowserState,
    sidebar_sftp: SftpBrowserState,
    transfers: SftpTransferQueue,
    performance: ServerPerformanceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalSessionKind {
    Ssh,
    Local,
}

impl TerminalSession {
    fn new(id: SessionId, profile_id: String) -> Self {
        Self {
            id,
            profile_id,
            kind: TerminalSessionKind::Ssh,
            close_when_disconnected: false,
            connection_state: SessionState::Disconnected,
            connection_handle: None,
            local_terminal_handle: None,
            connection_error: None,
            connection_message: None,
            terminal_end_reason: None,
            host_key_prompt: None,
            terminal: None,
            terminal_marked_text: String::new(),
            terminal_selection: None,
            terminal_selecting: false,
            terminal_scroll_accumulator: 0.0,
            terminal_resize_task: None,
            connection_credentials: Vec::new(),
            sftp_availability: SftpAvailability::Checking,
            sftp: SftpBrowserState::default(),
            sidebar_sftp: SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START),
            transfers: SftpTransferQueue::default(),
            performance: ServerPerformanceState::default(),
        }
    }

    fn new_local(id: SessionId, sftp_unavailable: String) -> Self {
        let mut session = Self::new(id, LOCAL_PROFILE_ID.into());
        session.kind = TerminalSessionKind::Local;
        session.sftp_availability = SftpAvailability::Unavailable(sftp_unavailable);
        session
    }

    fn is_local(&self) -> bool {
        self.kind == TerminalSessionKind::Local
    }

    fn write_terminal_input(&self, data: Vec<u8>) -> Result<(), String> {
        match self.kind {
            TerminalSessionKind::Ssh => self
                .connection_handle
                .as_ref()
                .ok_or_else(|| "SSH connection handle is missing".to_owned())?
                .send_input(data)
                .map_err(|error| error.to_string()),
            TerminalSessionKind::Local => self
                .local_terminal_handle
                .as_ref()
                .ok_or_else(|| "local terminal handle is missing".to_owned())?
                .send_input(data)
                .map_err(|error| error.to_string()),
        }
    }

    fn resize_terminal(&self, size: PtySize) -> Result<(), String> {
        match self.kind {
            TerminalSessionKind::Ssh => self
                .connection_handle
                .as_ref()
                .ok_or_else(|| "SSH connection handle is missing".to_owned())?
                .resize(size)
                .map_err(|error| error.to_string()),
            TerminalSessionKind::Local => self
                .local_terminal_handle
                .as_ref()
                .ok_or_else(|| "local terminal handle is missing".to_owned())?
                .resize(local_pty_size(size))
                .map_err(|error| error.to_string()),
        }
    }

    fn disconnect_terminal(&self) -> Result<(), String> {
        match self.kind {
            TerminalSessionKind::Ssh => self
                .connection_handle
                .as_ref()
                .ok_or_else(|| "SSH connection handle is missing".to_owned())?
                .disconnect()
                .map_err(|error| error.to_string()),
            TerminalSessionKind::Local => self
                .local_terminal_handle
                .as_ref()
                .ok_or_else(|| "local terminal handle is missing".to_owned())?
                .disconnect()
                .map_err(|error| error.to_string()),
        }
    }

    fn sftp_browser(&self, placement: SftpBrowserPlacement) -> &SftpBrowserState {
        match placement {
            SftpBrowserPlacement::Center => &self.sftp,
            SftpBrowserPlacement::Sidebar => &self.sidebar_sftp,
        }
    }

    fn sftp_browser_mut(&mut self, placement: SftpBrowserPlacement) -> &mut SftpBrowserState {
        match placement {
            SftpBrowserPlacement::Center => &mut self.sftp,
            SftpBrowserPlacement::Sidebar => &mut self.sidebar_sftp,
        }
    }

    fn is_terminal_visible(&self) -> bool {
        let active_connection = !self.connection_state.can_connect();

        self.terminal.as_ref().is_some_and(|terminal| {
            terminal.profile_id == self.profile_id && (active_connection || terminal.was_connected)
        })
    }

    fn terminal_has_ended(&self) -> bool {
        self.connection_state.can_connect()
            && self.terminal.as_ref().is_some_and(|terminal| {
                terminal.profile_id == self.profile_id && terminal.was_connected
            })
    }
}

struct ActiveTerminal {
    profile_id: String,
    engine: TerminalEngine,
    title: Option<String>,
    remote_cwd: Option<String>,
    pty_size: PtySize,
    pending_pty_size: Option<PtySize>,
    cell_width: f32,
    cell_height: f32,
    viewport_bounds: Option<Bounds<Pixels>>,
    was_connected: bool,
    render_damage: RefCell<TerminalDamage>,
    render_snapshot: RefCell<Option<Rc<TerminalSnapshot>>>,
    canvas_cache: Rc<RefCell<TerminalCanvasCache>>,
}

struct CommandTooltip {
    label: SharedString,
    theme: Theme,
}

struct AboutWindow {
    language_mode: LanguageMode,
}

impl Render for CommandTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py(px(5.0))
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.floating_glass_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }])
            .text_sm()
            .text_color(self.theme.text_primary)
            .child(self.label.clone())
    }
}

impl Render for AboutWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let localizer = Localizer::new(self.language_mode);
        let mut version_args = fluent_bundle::FluentArgs::new();
        version_args.set("version", env!("CARGO_PKG_VERSION"));

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .bg(theme.panel_bg)
            .text_color(theme.text_primary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .size(px(96.0))
                            .rounded_lg()
                            .shadow(vec![BoxShadow {
                                color: theme.shadow,
                                offset: point(px(0.0), px(5.0)),
                                blur_radius: px(18.0),
                                spread_radius: px(-6.0),
                            }])
                            .child(app_icon(96.0)),
                    )
                    .child(div().mt_5().child(wordmark(theme, 174.0, 38.0)))
                    .child(
                        div()
                            .mt_3()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(localizer.text_with("about-version", Some(&version_args))),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(localizer.text("about-tagline")),
                    )
                    .child(
                        div()
                            .mt_5()
                            .text_xs()
                            .text_color(theme.text_faint)
                            .child(localizer.text("about-license")),
                    ),
            )
    }
}

impl ActiveTerminal {
    fn new(profile_id: String, size: PtySize) -> Self {
        let columns = usize::try_from(size.columns).expect("PTY columns fit usize");
        let rows = usize::try_from(size.rows).expect("PTY rows fit usize");
        let mut engine = TerminalEngine::new(columns, rows).expect("valid initial terminal size");
        let _ = engine.take_damage();

        Self {
            profile_id,
            engine,
            title: None,
            remote_cwd: None,
            pty_size: size,
            pending_pty_size: None,
            cell_width: f32::from(TERMINAL_CELL_WIDTH),
            cell_height: f32::from(TERMINAL_CELL_HEIGHT),
            viewport_bounds: None,
            was_connected: false,
            render_damage: RefCell::new(TerminalDamage::Full),
            render_snapshot: RefCell::new(None),
            canvas_cache: Rc::new(RefCell::new(TerminalCanvasCache::default())),
        }
    }

    fn process(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        self.engine.process(bytes);
        self.capture_damage();
        self.engine.drain_events()
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.title = None;
        *self.render_damage.borrow_mut() = TerminalDamage::Full;
        self.render_snapshot.borrow_mut().take();
    }

    fn snapshot(&self) -> TerminalSnapshot {
        self.engine.snapshot()
    }

    fn snapshot_for_render(&self) -> (Rc<TerminalSnapshot>, TerminalDamage) {
        let damage = self
            .render_damage
            .replace(TerminalDamage::Partial(Vec::new()));
        let can_reuse = matches!(&damage, TerminalDamage::Partial(ranges) if ranges.is_empty());
        let snapshot = if can_reuse {
            self.render_snapshot.borrow().clone()
        } else {
            None
        }
        .unwrap_or_else(|| {
            let snapshot = Rc::new(self.engine.snapshot());
            *self.render_snapshot.borrow_mut() = Some(snapshot.clone());
            snapshot
        });

        (snapshot, damage)
    }

    fn scroll(&mut self, scroll: TerminalScroll) {
        self.engine.scroll(scroll);
        self.capture_damage();
    }

    fn capture_damage(&mut self) {
        let damage = self.engine.take_damage();
        merge_terminal_damage(&mut self.render_damage.borrow_mut(), damage);
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
            self.capture_damage();
        }

        self.pty_size = size;
        if self.pending_pty_size == Some(size) {
            self.pending_pty_size = None;
        }
        dimensions_changed
    }
}

fn merge_terminal_damage(current: &mut TerminalDamage, incoming: TerminalDamage) {
    match (&mut *current, incoming) {
        (TerminalDamage::Full, _) | (_, TerminalDamage::Full) => {
            *current = TerminalDamage::Full;
        }
        (TerminalDamage::Partial(current), TerminalDamage::Partial(incoming)) => {
            for range in incoming {
                if let Some(existing) = current
                    .iter_mut()
                    .find(|existing| existing.row == range.row)
                {
                    existing.left = existing.left.min(range.left);
                    existing.right = existing.right.max(range.right);
                } else {
                    current.push(range);
                }
            }
        }
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
    Home,
    Server,
    Connection,
    Settings,
    OpenSshImport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSelector {
    Language,
    Theme,
    TabLayout,
    TerminalFont,
    TerminalFontSize,
    TransferRate,
    ParallelTransfers,
}

impl SettingsSelector {
    const fn element_id(self) -> &'static str {
        match self {
            Self::Language => "settings-language-selector",
            Self::Theme => "settings-theme-selector",
            Self::TabLayout => "settings-tab-layout-selector",
            Self::TerminalFont => "settings-terminal-font-selector",
            Self::TerminalFontSize => "settings-terminal-font-size-selector",
            Self::TransferRate => "settings-transfer-rate-selector",
            Self::ParallelTransfers => "settings-parallel-transfers-selector",
        }
    }

    const fn options(self) -> &'static [SettingsOption] {
        match self {
            Self::Language => &LANGUAGE_SETTING_OPTIONS,
            Self::Theme => &THEME_SETTING_OPTIONS,
            Self::TabLayout => &TAB_LAYOUT_SETTING_OPTIONS,
            Self::TerminalFont => &[],
            Self::TerminalFontSize => &TERMINAL_FONT_SIZE_SETTING_OPTIONS,
            Self::TransferRate => &TRANSFER_RATE_SETTING_OPTIONS,
            Self::ParallelTransfers => &PARALLEL_TRANSFER_SETTING_OPTIONS,
        }
    }

    const fn control_width(self) -> f32 {
        match self {
            Self::Language => 132.0,
            Self::Theme => 92.0,
            Self::TabLayout => 104.0,
            Self::TerminalFont => 180.0,
            Self::TerminalFontSize => 72.0,
            Self::TransferRate => 104.0,
            Self::ParallelTransfers => 56.0,
        }
    }

    const fn menu_width(self) -> f32 {
        match self {
            Self::Language => 148.0,
            Self::Theme => 104.0,
            Self::TabLayout => 120.0,
            Self::TerminalFont => 220.0,
            Self::TerminalFontSize => 88.0,
            Self::TransferRate => 120.0,
            Self::ParallelTransfers => 72.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsValue {
    Language(LanguageMode),
    Theme(ThemeMode),
    TabLayout(TabLayout),
    TerminalFontSize(u16),
    TransferRate(u32),
    ParallelTransfers(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsOption {
    label: &'static str,
    value: SettingsValue,
}

const LANGUAGE_SETTING_OPTIONS: [SettingsOption; 3] = [
    SettingsOption {
        label: "Follow System",
        value: SettingsValue::Language(LanguageMode::System),
    },
    SettingsOption {
        label: "English",
        value: SettingsValue::Language(LanguageMode::EnUs),
    },
    SettingsOption {
        label: "简体中文",
        value: SettingsValue::Language(LanguageMode::ZhCn),
    },
];

const THEME_SETTING_OPTIONS: [SettingsOption; 3] = [
    SettingsOption {
        label: "System",
        value: SettingsValue::Theme(ThemeMode::System),
    },
    SettingsOption {
        label: "Light",
        value: SettingsValue::Theme(ThemeMode::Light),
    },
    SettingsOption {
        label: "Dark",
        value: SettingsValue::Theme(ThemeMode::Dark),
    },
];
const TAB_LAYOUT_SETTING_OPTIONS: [SettingsOption; 2] = [
    SettingsOption {
        label: "Horizontal",
        value: SettingsValue::TabLayout(TabLayout::Horizontal),
    },
    SettingsOption {
        label: "Vertical",
        value: SettingsValue::TabLayout(TabLayout::Vertical),
    },
];
const TERMINAL_FONT_SIZE_SETTING_OPTIONS: [SettingsOption; 17] = [
    SettingsOption {
        label: "8 pt",
        value: SettingsValue::TerminalFontSize(8),
    },
    SettingsOption {
        label: "9 pt",
        value: SettingsValue::TerminalFontSize(9),
    },
    SettingsOption {
        label: "10 pt",
        value: SettingsValue::TerminalFontSize(10),
    },
    SettingsOption {
        label: "11 pt",
        value: SettingsValue::TerminalFontSize(11),
    },
    SettingsOption {
        label: "12 pt",
        value: SettingsValue::TerminalFontSize(12),
    },
    SettingsOption {
        label: "13 pt",
        value: SettingsValue::TerminalFontSize(13),
    },
    SettingsOption {
        label: "14 pt",
        value: SettingsValue::TerminalFontSize(14),
    },
    SettingsOption {
        label: "15 pt",
        value: SettingsValue::TerminalFontSize(15),
    },
    SettingsOption {
        label: "16 pt",
        value: SettingsValue::TerminalFontSize(16),
    },
    SettingsOption {
        label: "17 pt",
        value: SettingsValue::TerminalFontSize(17),
    },
    SettingsOption {
        label: "18 pt",
        value: SettingsValue::TerminalFontSize(18),
    },
    SettingsOption {
        label: "20 pt",
        value: SettingsValue::TerminalFontSize(20),
    },
    SettingsOption {
        label: "22 pt",
        value: SettingsValue::TerminalFontSize(22),
    },
    SettingsOption {
        label: "24 pt",
        value: SettingsValue::TerminalFontSize(24),
    },
    SettingsOption {
        label: "26 pt",
        value: SettingsValue::TerminalFontSize(26),
    },
    SettingsOption {
        label: "28 pt",
        value: SettingsValue::TerminalFontSize(28),
    },
    SettingsOption {
        label: "32 pt",
        value: SettingsValue::TerminalFontSize(32),
    },
];
const TRANSFER_RATE_SETTING_OPTIONS: [SettingsOption; 4] = [
    SettingsOption {
        label: "Unlimited",
        value: SettingsValue::TransferRate(0),
    },
    SettingsOption {
        label: "5 MiB/s",
        value: SettingsValue::TransferRate(5),
    },
    SettingsOption {
        label: "20 MiB/s",
        value: SettingsValue::TransferRate(20),
    },
    SettingsOption {
        label: "100 MiB/s",
        value: SettingsValue::TransferRate(100),
    },
];
const PARALLEL_TRANSFER_SETTING_OPTIONS: [SettingsOption; 4] = [
    SettingsOption {
        label: "1",
        value: SettingsValue::ParallelTransfers(1),
    },
    SettingsOption {
        label: "2",
        value: SettingsValue::ParallelTransfers(2),
    },
    SettingsOption {
        label: "4",
        value: SettingsValue::ParallelTransfers(4),
    },
    SettingsOption {
        label: "8",
        value: SettingsValue::ParallelTransfers(8),
    },
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TerminalTabView {
    #[default]
    Terminal,
    Files,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SftpBrowserPlacement {
    Center,
    Sidebar,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum SftpAvailability {
    #[default]
    Checking,
    Available,
    Unavailable(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RightSidebarView {
    #[default]
    Sftp,
    Performance,
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
struct ServerPerformanceState {
    snapshot: Option<ServerPerformanceSnapshot>,
    previous: Option<PerformanceCounters>,
    cpu_usage: Option<f32>,
    cpu_iowait_usage: Option<f32>,
    logical_cpu_usage: Vec<(u32, f32)>,
    network_rx_per_second: Option<f64>,
    network_tx_per_second: Option<f64>,
    disk_read_per_second: Option<f64>,
    disk_write_per_second: Option<f64>,
    monitoring: bool,
    loading: bool,
    error: Option<String>,
}

impl ServerPerformanceState {
    fn update(&mut self, snapshot: ServerPerformanceSnapshot, captured_at: Instant) {
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

    fn clear_connection(&mut self) {
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

impl SftpBrowserPlacement {
    fn element_suffix(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Sidebar => "sidebar",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarResize {
    start_x: Pixels,
    start_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BottomPanelResize {
    start_y: Pixels,
    start_height: f32,
}

struct SftpBrowserState {
    path: String,
    entries: Vec<RemoteFileEntry>,
    file: Option<SftpFileState>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    next_request_id: u64,
    active_request_id: Option<u64>,
    active_request_path: Option<String>,
    resolved_source_path: Option<String>,
    tree_entries: HashMap<String, Vec<RemoteFileEntry>>,
    expanded_paths: HashSet<String>,
    tree_requests: HashMap<u64, String>,
    pending_download_trees: HashMap<u64, PendingSftpDownloadTree>,
    selected_paths: Vec<String>,
    selection_anchor: Option<String>,
    scroll_handle: UniformListScrollHandle,
    breadcrumb_scroll_handle: ScrollHandle,
    error_generation: u64,
}

#[derive(Clone)]
struct SftpTreeRow {
    entry: RemoteFileEntry,
    depth: usize,
}

struct PendingSftpDownloadTree {
    destination: PathBuf,
    batch_id: u64,
}

impl Default for SftpBrowserState {
    fn default() -> Self {
        Self {
            path: ".".into(),
            entries: Vec::new(),
            file: None,
            loading: false,
            loaded: false,
            error: None,
            next_request_id: 1,
            active_request_id: None,
            active_request_path: None,
            resolved_source_path: None,
            tree_entries: HashMap::new(),
            expanded_paths: HashSet::new(),
            tree_requests: HashMap::new(),
            pending_download_trees: HashMap::new(),
            selected_paths: Vec::new(),
            selection_anchor: None,
            scroll_handle: UniformListScrollHandle::new(),
            breadcrumb_scroll_handle: ScrollHandle::new(),
            error_generation: 0,
        }
    }
}

impl SftpBrowserState {
    fn with_request_id_start(next_request_id: u64) -> Self {
        Self {
            next_request_id,
            ..Self::default()
        }
    }

    fn needs_request(&self, path: &str) -> bool {
        self.active_request_path.as_deref() != Some(path)
            && self.resolved_source_path.as_deref() != Some(path)
    }

    fn begin_request(&mut self, path: String) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        if !self.loaded || self.path != path {
            self.scroll_handle = UniformListScrollHandle::new();
        }
        self.active_request_id = Some(request_id);
        self.active_request_path = Some(path);
        self.loading = true;
        self.clear_error();
        self.file = None;
        self.tree_entries.clear();
        self.expanded_paths.clear();
        self.tree_requests.clear();
        self.selected_paths.clear();
        self.selection_anchor = None;
        request_id
    }

    fn complete_request(&mut self, request_id: u64, directory: RemoteDirectory) -> bool {
        if let Some(requested_path) = self.tree_requests.remove(&request_id) {
            self.expanded_paths.remove(&requested_path);
            self.expanded_paths.insert(directory.path.clone());
            self.tree_entries.insert(directory.path, directory.entries);
            self.clear_error();
            return true;
        }
        if self.active_request_id != Some(request_id) {
            return false;
        }

        let breadcrumb_count = remote_breadcrumbs(&directory.path).len();
        self.path = directory.path;
        self.entries = directory.entries;
        self.loading = false;
        self.loaded = true;
        self.clear_error();
        self.active_request_id = None;
        self.resolved_source_path = self.active_request_path.take();
        self.breadcrumb_scroll_handle
            .scroll_to_item(breadcrumb_count.saturating_mul(2).saturating_sub(2));
        true
    }

    fn fail_request(&mut self, request_id: u64, error: String) -> bool {
        if let Some(path) = self.tree_requests.remove(&request_id) {
            self.expanded_paths.remove(&path);
            self.set_error(error);
            return true;
        }
        if self.pending_download_trees.remove(&request_id).is_some() {
            self.set_error(error);
            return true;
        }
        if self.active_request_id != Some(request_id) {
            return false;
        }

        self.loading = false;
        self.set_error(error);
        self.active_request_id = None;
        self.active_request_path = None;
        self.tree_requests.clear();
        self.pending_download_trees.clear();
        true
    }

    fn stop_loading(&mut self) {
        self.loading = false;
        self.active_request_id = None;
        self.active_request_path = None;
        if let Some(file) = self.file.as_mut() {
            file.loading = false;
            file.saving = false;
            file.read_request_id = None;
            file.write_request_id = None;
        }
    }

    fn next_request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        request_id
    }

    fn begin_tree_request(&mut self, path: String) -> u64 {
        let request_id = self.next_request_id();
        self.tree_requests.insert(request_id, path.clone());
        self.expanded_paths.insert(path);
        self.clear_error();
        request_id
    }

    fn begin_download_tree(&mut self, destination: PathBuf, batch_id: u64) -> u64 {
        let request_id = self.next_request_id();
        self.pending_download_trees.insert(
            request_id,
            PendingSftpDownloadTree {
                destination,
                batch_id,
            },
        );
        request_id
    }

    fn take_download_tree(&mut self, request_id: u64) -> Option<PendingSftpDownloadTree> {
        self.pending_download_trees.remove(&request_id)
    }

    fn set_error(&mut self, error: String) -> u64 {
        self.error_generation = self.error_generation.wrapping_add(1);
        self.error = Some(error);
        self.error_generation
    }

    fn clear_error(&mut self) {
        self.error_generation = self.error_generation.wrapping_add(1);
        self.error = None;
    }

    fn clear_error_if_current(&mut self, generation: u64) -> bool {
        if self.error_generation != generation || self.error.is_none() {
            return false;
        }
        self.clear_error();
        true
    }

    fn visible_rows(&self, tree: bool) -> Vec<SftpTreeRow> {
        if !tree {
            return self
                .entries
                .iter()
                .cloned()
                .map(|entry| SftpTreeRow { entry, depth: 0 })
                .collect();
        }

        let mut rows = Vec::new();
        let mut pending = self
            .entries
            .iter()
            .rev()
            .cloned()
            .map(|entry| SftpTreeRow { entry, depth: 0 })
            .collect::<Vec<_>>();
        while let Some(row) = pending.pop() {
            let path = row.entry.path.clone();
            let depth = row.depth;
            rows.push(row);
            if self.expanded_paths.contains(&path)
                && let Some(children) = self.tree_entries.get(&path)
            {
                pending.extend(children.iter().rev().cloned().map(|entry| SftpTreeRow {
                    entry,
                    depth: depth + 1,
                }));
            }
        }
        rows
    }

    fn selected_entries(&self) -> Vec<RemoteFileEntry> {
        self.selected_paths
            .iter()
            .filter_map(|path| self.entry(path).cloned())
            .collect()
    }

    fn entry(&self, path: &str) -> Option<&RemoteFileEntry> {
        self.entries
            .iter()
            .chain(self.tree_entries.values().flatten())
            .find(|entry| entry.path == path)
    }

    fn select_path(&mut self, path: &str, modifiers: gpui::Modifiers, tree: bool) {
        let visible_paths = self
            .visible_rows(tree)
            .into_iter()
            .map(|row| row.entry.path)
            .collect::<Vec<_>>();
        let Some(clicked_index) = visible_paths.iter().position(|candidate| candidate == path)
        else {
            return;
        };

        if modifiers.shift {
            let anchor_index = self
                .selection_anchor
                .as_ref()
                .and_then(|anchor| {
                    visible_paths
                        .iter()
                        .position(|candidate| candidate == anchor)
                })
                .unwrap_or(clicked_index);
            let range = anchor_index.min(clicked_index)..=anchor_index.max(clicked_index);
            if !modifiers.secondary() {
                self.selected_paths.clear();
            }
            for index in range {
                let path = &visible_paths[index];
                if !self.selected_paths.contains(path) {
                    self.selected_paths.push(path.clone());
                }
            }
        } else if modifiers.secondary() {
            if let Some(index) = self
                .selected_paths
                .iter()
                .position(|candidate| candidate == path)
            {
                self.selected_paths.remove(index);
            } else {
                self.selected_paths.push(path.to_owned());
            }
            self.selection_anchor = Some(path.to_owned());
        } else {
            self.selected_paths.clear();
            self.selected_paths.push(path.to_owned());
            self.selection_anchor = Some(path.to_owned());
        }
    }

    fn select_for_context_menu(&mut self, path: &str) {
        if !self.selected_paths.iter().any(|selected| selected == path) {
            self.selected_paths.clear();
            self.selected_paths.push(path.to_owned());
            self.selection_anchor = Some(path.to_owned());
        }
    }

    fn remove_paths(&mut self, paths: &[String]) {
        self.selected_paths
            .retain(|selected| !paths.iter().any(|path| selected == path));
        self.tree_entries.retain(|path, _| {
            !paths
                .iter()
                .any(|deleted| path == deleted || remote_path_is_descendant(deleted, path))
        });
        for entries in self.tree_entries.values_mut() {
            entries.retain(|entry| {
                !paths
                    .iter()
                    .any(|path| entry.path == *path || remote_path_is_descendant(path, &entry.path))
            });
        }
        self.entries.retain(|entry| {
            !paths
                .iter()
                .any(|path| entry.path == *path || remote_path_is_descendant(path, &entry.path))
        });
    }

    fn begin_file_request(&mut self, path: String, editable: bool) -> u64 {
        let request_id = self.next_request_id();
        self.file = Some(SftpFileState {
            path,
            original_contents: Vec::new(),
            editor: None,
            text_format: None,
            loading: true,
            saving: false,
            error: None,
            editable,
            read_request_id: Some(request_id),
            write_request_id: None,
        });
        request_id
    }

    fn begin_file_save(&mut self) -> Option<u64> {
        let request_id = self.next_request_id();
        let file = self.file.as_mut()?;
        file.saving = true;
        file.error = None;
        file.write_request_id = Some(request_id);
        Some(request_id)
    }

    fn fail_file_request(&mut self, request_id: u64, operation: SftpOperation, error: String) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        match operation {
            SftpOperation::ReadFile if file.read_request_id == Some(request_id) => {
                file.loading = false;
                file.read_request_id = None;
                file.error = Some(error);
            }
            SftpOperation::WriteFile if file.write_request_id == Some(request_id) => {
                file.saving = false;
                file.write_request_id = None;
                file.error = Some(error);
            }
            SftpOperation::ReadDirectory
            | SftpOperation::ReadDirectoryTree
            | SftpOperation::ReadFile
            | SftpOperation::WriteFile
            | SftpOperation::CreateFile
            | SftpOperation::CreateDirectory
            | SftpOperation::DeletePaths
            | SftpOperation::UploadFile
            | SftpOperation::DownloadFile
            | SftpOperation::CancelTransfer => {}
        }
    }

    fn display_path(&self) -> &str {
        self.file
            .as_ref()
            .map(|file| file.path.as_str())
            .unwrap_or(&self.path)
    }
}

struct SftpFileState {
    path: String,
    original_contents: Vec<u8>,
    editor: Option<Entity<FileEditor>>,
    text_format: Option<RemoteTextFormat>,
    loading: bool,
    saving: bool,
    error: Option<String>,
    editable: bool,
    read_request_id: Option<u64>,
    write_request_id: Option<u64>,
}

struct ProfileContextMenu {
    profile_id: String,
    position: gpui::Point<Pixels>,
}

struct SftpContextMenu {
    session_id: SessionId,
    placement: SftpBrowserPlacement,
    position: gpui::Point<Pixels>,
}

struct TerminalContextMenu {
    session_id: SessionId,
    position: gpui::Point<Pixels>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SftpCreateKind {
    File,
    Directory,
}

struct SftpCreatePrompt {
    session_id: SessionId,
    placement: SftpBrowserPlacement,
    kind: SftpCreateKind,
    input: Entity<TextField>,
    error: Option<String>,
}

struct QuickCommandPrompt {
    input: Entity<TextField>,
    selected_profile_ids: HashSet<String>,
    selection_touched: bool,
    target_menu_open: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SftpTransferState {
    Queued,
    Running,
    Cancelling,
    Conflict,
    Completed,
    Failed,
    Cancelled,
}

impl SftpTransferState {
    const fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }

    const fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug)]
struct SftpTransferTask {
    id: u64,
    batch_id: u64,
    direction: SftpTransferDirection,
    local_path: PathBuf,
    remote_path: String,
    overwrite: bool,
    state: SftpTransferState,
    transferred: u64,
    total: Option<u64>,
    error: Option<String>,
}

struct SftpTransferSpec {
    batch_id: u64,
    direction: SftpTransferDirection,
    local_path: PathBuf,
    remote_path: String,
    overwrite: bool,
    expected_total: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SftpTransferBatchProgress {
    task_count: usize,
    settled_count: usize,
    failed_count: usize,
    transferred: u64,
    total: Option<u64>,
    fraction: f32,
}

impl SftpTransferTask {
    fn display_name(&self) -> String {
        match self.direction {
            SftpTransferDirection::Upload => self
                .local_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.local_path.display().to_string()),
            SftpTransferDirection::Download => remote_file_name(&self.remote_path).to_owned(),
        }
    }

    fn status_text(&self, localizer: &Localizer) -> String {
        match self.state {
            SftpTransferState::Queued => localizer.text("sftp-queued"),
            SftpTransferState::Running => self.total.map_or_else(
                || format_remote_size(self.transferred),
                |total| {
                    format!(
                        "{} / {}",
                        format_remote_size(self.transferred),
                        format_remote_size(total)
                    )
                },
            ),
            SftpTransferState::Cancelling => localizer.text("sftp-cancelling"),
            SftpTransferState::Conflict => localizer.text("sftp-conflict"),
            SftpTransferState::Completed => {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("size", format_remote_size(self.transferred));
                localizer.text_with("sftp-completed", Some(&args))
            }
            SftpTransferState::Failed => self
                .error
                .clone()
                .unwrap_or_else(|| localizer.text("sftp-failed")),
            SftpTransferState::Cancelled => localizer.text("sftp-cancelled"),
        }
    }
}

#[derive(Default)]
struct SftpTransferQueue {
    next_id: u64,
    next_batch_id: u64,
    tasks: Vec<SftpTransferTask>,
}

impl SftpTransferQueue {
    fn begin_batch(&mut self) -> u64 {
        self.next_batch_id = self.next_batch_id.max(1);
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;
        batch_id
    }

    fn enqueue_in_batch(
        &mut self,
        batch_id: u64,
        direction: SftpTransferDirection,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
        expected_total: Option<u64>,
    ) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(SftpTransferTask {
            id,
            batch_id,
            direction,
            local_path,
            remote_path,
            overwrite,
            state: SftpTransferState::Queued,
            transferred: 0,
            total: expected_total,
            error: None,
        });
        id
    }

    fn start_next(&mut self) -> Option<SftpTransferTask> {
        let task = self
            .tasks
            .iter_mut()
            .find(|task| task.state == SftpTransferState::Queued)?;
        task.state = SftpTransferState::Running;
        task.error = None;
        Some(task.clone())
    }

    fn active_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.state.is_active())
            .count()
    }

    fn task_mut(&mut self, id: u64) -> Option<&mut SftpTransferTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    fn mark_progress(&mut self, id: u64, transferred: u64, total: Option<u64>) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if !matches!(
            task.state,
            SftpTransferState::Running | SftpTransferState::Cancelling
        ) {
            return false;
        }
        task.transferred = task.transferred.max(transferred);
        if total.is_some() {
            task.total = total;
        }
        true
    }

    fn latest_batch_progress(
        &self,
        direction: SftpTransferDirection,
    ) -> Option<SftpTransferBatchProgress> {
        let batch_id = self
            .tasks
            .iter()
            .filter(|task| task.direction == direction)
            .map(|task| task.batch_id)
            .max()?;
        let tasks = self
            .tasks
            .iter()
            .filter(|task| task.direction == direction && task.batch_id == batch_id)
            .collect::<Vec<_>>();
        if tasks.len() < 2 {
            return None;
        }

        let task_count = tasks.len();
        let settled_count = tasks.iter().filter(|task| task.state.is_finished()).count();
        let failed_count = tasks
            .iter()
            .filter(|task| {
                matches!(
                    task.state,
                    SftpTransferState::Failed | SftpTransferState::Cancelled
                )
            })
            .count();
        let all_totals_known = tasks.iter().all(|task| task.total.is_some());
        let total = all_totals_known.then(|| {
            tasks
                .iter()
                .filter_map(|task| task.total)
                .fold(0_u64, u64::saturating_add)
        });
        let transferred = tasks.iter().fold(0_u64, |sum, task| {
            sum.saturating_add(
                task.total
                    .map(|total| task.transferred.min(total))
                    .unwrap_or(task.transferred),
            )
        });
        let fraction = total
            .filter(|total| *total > 0)
            .map(|total| transferred as f32 / total as f32)
            .unwrap_or_else(|| {
                tasks
                    .iter()
                    .map(|task| {
                        if task.state == SftpTransferState::Completed {
                            1.0
                        } else {
                            task.total
                                .filter(|total| *total > 0)
                                .map(|total| task.transferred as f32 / total as f32)
                                .unwrap_or(0.0)
                        }
                    })
                    .sum::<f32>()
                    / task_count as f32
            })
            .clamp(0.0, 1.0);

        Some(SftpTransferBatchProgress {
            task_count,
            settled_count,
            failed_count,
            transferred,
            total,
            fraction,
        })
    }

    fn mark_conflict(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state != SftpTransferState::Running {
            return false;
        }
        task.state = SftpTransferState::Conflict;
        true
    }

    fn mark_completed(&mut self, id: u64, bytes: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if !matches!(
            task.state,
            SftpTransferState::Running | SftpTransferState::Cancelling
        ) {
            return false;
        }
        task.state = SftpTransferState::Completed;
        task.transferred = bytes;
        task.total = Some(bytes);
        task.error = None;
        true
    }

    fn mark_failed(&mut self, id: u64, error: String) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state.is_finished() {
            return false;
        }
        task.state = SftpTransferState::Failed;
        task.error = Some(error);
        true
    }

    fn mark_cancelled(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state.is_finished() {
            return false;
        }
        task.state = SftpTransferState::Cancelled;
        task.error = None;
        true
    }

    fn retry_with_overwrite(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        if task.state != SftpTransferState::Conflict {
            return false;
        }
        task.overwrite = true;
        task.state = SftpTransferState::Queued;
        task.transferred = 0;
        task.total = None;
        task.error = None;
        true
    }

    fn begin_cancel(&mut self, id: u64) -> Option<bool> {
        let task = self.task_mut(id)?;
        match task.state {
            SftpTransferState::Running => {
                task.state = SftpTransferState::Cancelling;
                Some(true)
            }
            SftpTransferState::Queued | SftpTransferState::Conflict => {
                task.state = SftpTransferState::Cancelled;
                Some(false)
            }
            SftpTransferState::Cancelling
            | SftpTransferState::Completed
            | SftpTransferState::Failed
            | SftpTransferState::Cancelled => None,
        }
    }

    fn clear_finished(&mut self) {
        self.tasks.retain(|task| !task.state.is_finished());
    }

    fn fail_pending(&mut self, error: &str) {
        for task in &mut self.tasks {
            if !task.state.is_finished() {
                task.state = SftpTransferState::Failed;
                task.error = Some(error.into());
            }
        }
    }
}

impl SftpFileState {
    fn is_dirty(&self, cx: &App) -> bool {
        if !self.editable {
            return false;
        }
        self.editor
            .as_ref()
            .zip(self.text_format)
            .is_some_and(|(editor, format)| {
                format.encode(editor.read(cx).text()).as_slice() != self.original_contents
            })
    }

    fn edited_contents(&self, cx: &App) -> Option<Vec<u8>> {
        if !self.editable {
            return None;
        }
        self.editor
            .as_ref()
            .zip(self.text_format)
            .map(|(editor, format)| format.encode(editor.read(cx).text()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RemoteTextFormat {
    utf8_bom: bool,
    line_ending: RemoteLineEnding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RemoteLineEnding {
    Lf,
    CrLf,
}

impl RemoteTextFormat {
    fn decode(contents: &[u8]) -> Option<(Self, String)> {
        if contents.contains(&0) {
            return None;
        }
        let (utf8_bom, text_bytes) = contents
            .strip_prefix(&[0xef, 0xbb, 0xbf])
            .map_or((false, contents), |contents| (true, contents));
        let text = std::str::from_utf8(text_bytes).ok()?;
        let line_ending = if text.contains("\r\n") {
            RemoteLineEnding::CrLf
        } else {
            RemoteLineEnding::Lf
        };
        let text = match line_ending {
            RemoteLineEnding::Lf => text.to_owned(),
            RemoteLineEnding::CrLf => text.replace("\r\n", "\n"),
        };
        Some((
            Self {
                utf8_bom,
                line_ending,
            },
            text,
        ))
    }

    fn encode(self, text: &str) -> Vec<u8> {
        let text = match self.line_ending {
            RemoteLineEnding::Lf => text.to_owned(),
            RemoteLineEnding::CrLf => text.replace('\n', "\r\n"),
        };
        let mut contents = Vec::with_capacity(text.len() + usize::from(self.utf8_bom) * 3);
        if self.utf8_bom {
            contents.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        }
        contents.extend_from_slice(text.as_bytes());
        contents
    }
}

#[derive(Clone)]
struct ProfileEditor {
    mode: ProfileEditorMode,
    profile_id: String,
    name: Entity<TextField>,
    host: Entity<TextField>,
    port: Entity<TextField>,
    username: Entity<TextField>,
    auth_kind: ProfileAuthKind,
    private_key_path: Entity<TextField>,
    proxy_kind: ProfileProxyKind,
    proxy_host: Entity<TextField>,
    proxy_port: Entity<TextField>,
    proxy_username: Entity<TextField>,
    proxy_password: Entity<TextField>,
    proxy_command: Entity<TextField>,
    jump_search: Entity<TextField>,
    jump_host_ids: Vec<String>,
    proxy_secret_loaded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileEditorMode {
    Create,
    Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileAuthKind {
    None,
    Password,
    PrivateKey,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProfileProxyKind {
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

    fn from_config(config: Option<&ProxyConfig>) -> Self {
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
    const OPTIONS: [(Self, &'static str); 4] = [
        (Self::None, "No Password"),
        (Self::Password, "Password"),
        (Self::PrivateKey, "Private Key"),
        (Self::Agent, "SSH Agent"),
    ];

    fn from_config(config: &AuthConfig) -> Self {
        match config {
            AuthConfig::None => Self::None,
            AuthConfig::Password => Self::Password,
            AuthConfig::PrivateKey { .. } => Self::PrivateKey,
            AuthConfig::Agent => Self::Agent,
        }
    }

    fn into_config(self, private_key_path: &str) -> Result<AuthConfig, &'static str> {
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

struct CredentialPrompt {
    session_id: SessionId,
    profile_id: String,
    kind: CredentialPromptKind,
    input: Entity<TextField>,
    remember: bool,
    error: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum CredentialPromptKind {
    Password,
    PrivateKeyPassphrase { path: PathBuf },
    ProxyPassword,
}

impl CredentialPromptKind {
    fn credential_kind(&self) -> CredentialKind {
        match self {
            Self::Password => CredentialKind::Password,
            Self::PrivateKeyPassphrase { .. } => CredentialKind::PrivateKeyPassphrase,
            Self::ProxyPassword => CredentialKind::ProxyPassword,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialSource {
    SystemKeychain,
    Prompt,
}

struct ConnectionCredential {
    profile_id: String,
    kind: CredentialKind,
    source: CredentialSource,
    save_on_success: Option<secrecy::SecretString>,
}

struct PendingConnectionPreparation {
    session_id: SessionId,
    target_profile: ConnectionProfile,
    steps: Vec<ConnectionProfile>,
    next_step: usize,
    prepared_steps: Vec<ConnectionStep>,
    credentials: Vec<ConnectionCredential>,
    runtime_proxy: Option<RuntimeProxy>,
    proxy_prepared: bool,
    force_prompt: Option<(String, CredentialKind, Option<String>)>,
}

struct ProxyCommandApprovalPrompt {
    session_id: SessionId,
    target_profile: ConnectionProfile,
    plan: ConnectionPlan,
    credentials: Vec<ConnectionCredential>,
    expanded_command: SecretString,
    approval_digest: String,
}

impl ConnectionCredential {
    fn from_keychain(profile_id: String, kind: CredentialKind) -> Self {
        Self {
            profile_id,
            kind,
            source: CredentialSource::SystemKeychain,
            save_on_success: None,
        }
    }

    fn from_prompt(
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

// Application construction and shared data helpers.
impl RemCmdApp {
    fn load(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profiles_path = default_profiles_path().expect("failed to resolve profiles path");
        let settings_path = default_settings_path().expect("failed to resolve settings path");

        let (profiles, profile_load_error) = match ensure_profiles_file(&profiles_path)
            .and_then(|_| load_profiles(&profiles_path))
        {
            Ok(profiles) => (profiles, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };

        let selected_profile_id = profiles.first().map(|profile| profile.id.clone());
        let next_profile_number = profiles
            .iter()
            .filter_map(|profile| profile.id.strip_prefix("demo-")?.parse::<usize>().ok())
            .max()
            .unwrap_or(0)
            + 1;

        let (settings, settings_load_error) = match load_settings(&settings_path) {
            Ok(settings) => (settings, None),
            Err(error) => (AppSettings::default(), Some(error.to_string())),
        };
        let language_mode = settings.language_mode;
        let localizer = Localizer::new(language_mode);
        let form_error = profile_load_error
            .map(|error| format!("{}: {error}", localizer.text("app-load-profiles-failed")));
        let settings_error = settings_load_error
            .map(|error| format!("{}: {error}", localizer.text("app-load-settings-failed")));
        let theme_mode = settings.theme_mode;
        let tab_layout = settings.tab_layout;
        let terminal_settings = settings.terminal.normalized();
        let terminal_font_families =
            normalize_terminal_font_families(cx.text_system().all_font_names());
        let terminal_font_family = resolve_terminal_font_family(
            terminal_settings.font_family.as_deref(),
            &terminal_font_families,
        );
        let terminal_font_size = terminal_settings.font_size;
        let transfer_settings = settings.transfers.normalized();
        let transfer_rate_limiter = Arc::new(TransferRateLimiter::new(
            transfer_settings.bytes_per_second(),
        ));
        let theme = Theme::resolve(theme_mode, window);
        set_global_theme(theme, cx);

        let appearance_subscription = cx.observe_window_appearance(window, |this, window, cx| {
            this.refresh_system_theme(window, cx);
        });
        let sidebar_search =
            cx.new(|cx| TextField::new(cx, "", localizer.text("sidebar-search-placeholder")));
        cx.observe(&sidebar_search, |_, _, cx| cx.notify()).detach();
        let settings_focus_handle = cx.focus_handle();
        let quick_terminal_focus_handle = cx.focus_handle();

        let app = Self {
            profiles,
            profiles_path,
            selected_profile_id,
            next_profile_number,
            editor: None,
            form_error,
            credential_prompt: None,
            sessions: Vec::new(),
            active_session_id: None,
            next_session_id: 1,
            tabs: Vec::new(),
            active_tab_id: None,
            previous_active_tab_id: None,
            titlebar_tab_transition_id: 0,
            hovered_titlebar_tab_id: None,
            hovered_titlebar_close_id: None,
            titlebar_tabs_scroll_handle: ScrollHandle::new(),
            titlebar_tabs_scroll_transition_id: 0,
            titlebar_tabs_scroll_start: point(px(0.0), px(0.0)),
            titlebar_tabs_scroll_active: false,
            titlebar_tabs_scroll_cleanup_task: None,
            terminal_redraw_task: None,
            next_tab_id: 1,
            panes: Vec::new(),
            active_pane_id: None,
            next_pane_id: 1,
            sidebar_search,
            sidebar_search_visible: false,
            connections_expanded: true,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            left_sidebar_open: true,
            left_sidebar_transition_id: 0,
            sidebar_resize: None,
            right_sidebar_open: false,
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            right_sidebar_resize: None,
            right_sidebar_transition_id: 0,
            right_sidebar_rendered: false,
            right_sidebar_animation_task: None,
            right_sidebar_view: RightSidebarView::Sftp,
            credential_lookup_task: None,
            credential_lookup_session_id: None,
            credential_mutations_in_progress: HashMap::new(),
            pending_connection: None,
            pending_proxy_approval: HashSet::new(),
            proxy_command_approval_prompt: None,
            active_panel: ActivePanel::Home,
            language_mode,
            localizer,
            theme_mode,
            tab_layout,
            terminal_font_family,
            terminal_font_families,
            terminal_font_size,
            transfer_settings,
            transfer_rate_limiter,
            next_transfer_session_cursor: 0,
            profile_context_menu: None,
            sftp_context_menu: None,
            terminal_context_menu: None,
            windows_menu_open: None,
            sftp_create_prompt: None,
            quick_command_prompt: None,
            bottom_panel_open: false,
            bottom_panel_height: BOTTOM_PANEL_DEFAULT_HEIGHT,
            bottom_panel_resize: None,
            quick_terminal_session_id: None,
            quick_terminal_focus_handle: quick_terminal_focus_handle.clone(),
            focused_terminal_session_id: None,
            profile_auth_selector_open: false,
            open_settings_selector: None,
            settings_selector_scroll_handle: ScrollHandle::new(),
            settings_virtual_selector_scroll_handle: UniformListScrollHandle::new(),
            settings_focus_handle,
            theme,
            settings_path,
            settings_error,
            openssh_import_preview: None,
            openssh_selected_aliases: HashSet::new(),
            openssh_overwrite_conflicts: HashSet::new(),
            openssh_import_loading: false,
            openssh_import_error: None,
            about_window: None,
            _appearance_subscription: appearance_subscription,
        };

        cx.on_focus(&quick_terminal_focus_handle, window, |this, _, cx| {
            if let Some(session_id) = this.quick_terminal_session_id {
                this.focused_terminal_session_id = Some(session_id);
                let modes = this
                    .session(session_id)
                    .and_then(|session| session.terminal.as_ref())
                    .map(ActiveTerminal::modes)
                    .unwrap_or(TerminalModes::NONE);
                if let Some(bytes) = encode_focus(true, modes) {
                    this.send_terminal_response(session_id, bytes);
                }
                cx.notify();
            }
        })
        .detach();
        cx.on_blur(&quick_terminal_focus_handle, window, |this, _, cx| {
            let Some(session_id) = this.quick_terminal_session_id else {
                return;
            };
            if this.focused_terminal_session_id == Some(session_id) {
                this.focused_terminal_session_id = None;
            }
            let modes = this
                .session(session_id)
                .and_then(|session| session.terminal.as_ref())
                .map(ActiveTerminal::modes)
                .unwrap_or(TerminalModes::NONE);
            if let Some(bytes) = encode_focus(false, modes) {
                this.send_terminal_response(session_id, bytes);
            }
            cx.notify();
        })
        .detach();

        app
    }

    fn selected_profile(&self) -> Option<&ConnectionProfile> {
        let selected_id = self.selected_profile_id.as_ref()?;

        self.profiles
            .iter()
            .find(|profile| &profile.id == selected_id)
    }

    fn session(&self, session_id: SessionId) -> Option<&TerminalSession> {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
    }

    fn session_mut(&mut self, session_id: SessionId) -> Option<&mut TerminalSession> {
        self.sessions
            .iter_mut()
            .find(|session| session.id == session_id)
    }

    fn active_session(&self) -> Option<&TerminalSession> {
        self.active_session_id
            .and_then(|session_id| self.session(session_id))
    }

    fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        let session_id = self.active_session_id?;
        self.session_mut(session_id)
    }

    fn terminal_input_session_id(&self) -> Option<SessionId> {
        self.focused_terminal_session_id
            .filter(|session_id| self.session(*session_id).is_some())
            .or(self.active_session_id)
    }

    fn terminal_input_session(&self) -> Option<&TerminalSession> {
        self.terminal_input_session_id()
            .and_then(|session_id| self.session(session_id))
    }

    fn terminal_input_session_mut(&mut self) -> Option<&mut TerminalSession> {
        let session_id = self.terminal_input_session_id()?;
        self.session_mut(session_id)
    }

    fn session_for_profile_mut(&mut self, profile_id: &str) -> Option<&mut TerminalSession> {
        self.sessions
            .iter_mut()
            .rev()
            .find(|session| session.profile_id == profile_id)
    }

    fn session_for_profile(&self, profile_id: &str) -> Option<&TerminalSession> {
        self.sessions
            .iter()
            .rev()
            .find(|session| session.profile_id == profile_id)
    }

    fn selected_session(&self) -> Option<&TerminalSession> {
        let profile_id = self.selected_profile_id.as_deref()?;
        self.active_session()
            .filter(|session| session.profile_id == profile_id)
    }

    fn create_session_for_profile(&mut self, profile_id: &str) -> SessionId {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        self.sessions
            .push(TerminalSession::new(session_id, profile_id.to_owned()));
        session_id
    }

    fn create_local_session(&mut self) -> SessionId {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        let session = TerminalSession::new_local(session_id, self.tr("sftp-ssh-only"));
        self.sessions.push(session);
        session_id
    }

    fn tab(&self, tab_id: TabId) -> Option<&TerminalTab> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
    }

    fn tab_mut(&mut self, tab_id: TabId) -> Option<&mut TerminalTab> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    fn active_tab(&self) -> Option<&TerminalTab> {
        self.active_tab_id.and_then(|tab_id| self.tab(tab_id))
    }

    fn active_tab_view(&self) -> TerminalTabView {
        self.active_tab().map(|tab| tab.view).unwrap_or_default()
    }

    fn effective_sidebar_width(&self, window: &Window) -> f32 {
        clamp_sidebar_width(self.sidebar_width, f32::from(window.viewport_size().width))
    }

    fn effective_right_sidebar_width(&self, window: &Window) -> f32 {
        let viewport_width = f32::from(window.viewport_size().width);
        let left_sidebar_width = if self.left_sidebar_open {
            clamp_sidebar_width(self.sidebar_width, viewport_width)
        } else {
            0.0
        };
        clamp_right_sidebar_width(self.right_sidebar_width, viewport_width, left_sidebar_width)
    }

    fn titlebar_leading_width(&self, window: &Window) -> f32 {
        if self.left_sidebar_open {
            self.effective_sidebar_width(window)
        } else {
            COLLAPSED_TITLEBAR_LEADING_WIDTH
        }
    }

    fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        self.left_sidebar_open = !self.left_sidebar_open;
        self.left_sidebar_transition_id += 1;
        self.sidebar_resize = None;
        cx.notify();
    }

    fn begin_sidebar_resize(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.left_sidebar_open {
            cx.stop_propagation();
            return;
        }
        self.sidebar_resize = Some(SidebarResize {
            start_x: event.position.x,
            start_width: self.effective_sidebar_width(window),
        });
        cx.stop_propagation();
    }

    fn resize_sidebar(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.sidebar_resize else {
            return;
        };
        if !event.dragging() {
            self.sidebar_resize = None;
            return;
        }

        let requested_width = resize.start_width + f32::from(event.position.x - resize.start_x);
        let width = clamp_sidebar_width(requested_width, f32::from(window.viewport_size().width));
        if self.sidebar_width != width {
            self.sidebar_width = width;
            cx.notify();
        }
    }

    fn finish_sidebar_resize(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_right_sidebar_resize(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_sidebar_resize = Some(SidebarResize {
            start_x: event.position.x,
            start_width: self.effective_right_sidebar_width(window),
        });
        cx.stop_propagation();
    }

    fn resize_right_sidebar(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.right_sidebar_resize else {
            return;
        };
        if !event.dragging() {
            self.right_sidebar_resize = None;
            return;
        }

        let requested_width = resize.start_width + f32::from(resize.start_x - event.position.x);
        let width = clamp_right_sidebar_width(
            requested_width,
            f32::from(window.viewport_size().width),
            if self.left_sidebar_open {
                self.effective_sidebar_width(window)
            } else {
                0.0
            },
        );
        if self.right_sidebar_width != width {
            self.right_sidebar_width = width;
            cx.notify();
        }
    }

    fn finish_right_sidebar_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_sidebar_resize.take().is_some() {
            cx.notify();
        }
    }

    fn begin_bottom_panel_resize(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.bottom_panel_open {
            return;
        }
        self.bottom_panel_resize = Some(BottomPanelResize {
            start_y: event.position.y,
            start_height: clamp_bottom_panel_height(
                self.bottom_panel_height,
                f32::from(window.viewport_size().height),
            ),
        });
        cx.stop_propagation();
    }

    fn resize_bottom_panel(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(resize) = self.bottom_panel_resize else {
            return;
        };
        if !event.dragging() {
            self.bottom_panel_resize = None;
            return;
        }

        let requested_height = resize.start_height + f32::from(resize.start_y - event.position.y);
        let height =
            clamp_bottom_panel_height(requested_height, f32::from(window.viewport_size().height));
        if self.bottom_panel_height != height {
            self.bottom_panel_height = height;
            cx.notify();
        }
    }

    fn finish_bottom_panel_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bottom_panel_resize.take().is_some() {
            cx.notify();
        }
    }

    fn toggle_right_sidebar(&mut self, cx: &mut Context<Self>) {
        self.right_sidebar_open = !self.right_sidebar_open;
        self.right_sidebar_transition_id += 1;
        let transition_id = self.right_sidebar_transition_id;
        self.right_sidebar_resize = None;
        self.right_sidebar_animation_task = None;
        if self.right_sidebar_open {
            self.right_sidebar_rendered = true;
            if self.right_sidebar_view == RightSidebarView::Sftp
                && let Some(session_id) = self.active_session_id
            {
                self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
            }
        } else {
            self.right_sidebar_animation_task = Some(cx.spawn(async move |this, cx| {
                Timer::after(MOTION_STANDARD_DURATION).await;
                let _ = this.update(cx, |this, cx| {
                    if this.right_sidebar_transition_id == transition_id && !this.right_sidebar_open
                    {
                        this.right_sidebar_rendered = false;
                        this.right_sidebar_animation_task = None;
                        cx.notify();
                    }
                });
            }));
        }
        self.sync_performance_monitoring();
        cx.notify();
    }

    fn set_right_sidebar_view(&mut self, view: RightSidebarView, cx: &mut Context<Self>) {
        self.right_sidebar_view = view;
        if view == RightSidebarView::Sftp
            && let Some(session_id) = self.active_session_id
        {
            self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
        }
        self.sync_performance_monitoring();
        cx.notify();
    }

    fn sync_performance_monitoring(&mut self) {
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

    fn set_active_tab_view(
        &mut self,
        view: TerminalTabView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.active_tab_id else {
            return;
        };
        let Some(session_id) = self.active_session_id else {
            return;
        };
        if let Some(tab) = self.tab_mut(tab_id) {
            tab.view = view;
        }

        match view {
            TerminalTabView::Terminal => {
                if let Some(focus_handle) = self.active_pane().map(|pane| pane.focus_handle.clone())
                {
                    focus_handle.focus(window);
                }
            }
            TerminalTabView::Files => {
                self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Center, cx)
            }
        }
        cx.notify();
    }

    fn ensure_sftp_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some((path, needs_load)) = self.session(session_id).and_then(|session| {
            if session.connection_state == SessionState::Connected
                && session.sftp_availability != SftpAvailability::Available
            {
                return None;
            }
            let path = session
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.remote_cwd.clone())
                .unwrap_or_else(|| ".".into());
            let browser = session.sftp_browser(placement);
            let editor_closed =
                placement == SftpBrowserPlacement::Sidebar || session.sftp.file.is_none();
            let needs_load = editor_closed && browser.needs_request(&path);
            Some((path, needs_load))
        }) else {
            return;
        };
        if needs_load {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    fn show_sftp_error(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let generation = self
            .session_mut(session_id)
            .map(|session| session.sftp_browser_mut(placement).set_error(error));
        if let Some(generation) = generation {
            self.schedule_sftp_error_clear(session_id, placement, generation, cx);
        }
        cx.notify();
    }

    fn fail_sftp_request(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        request_id: u64,
        error: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = self.session_mut(session_id).map(|session| {
            let browser = session.sftp_browser_mut(placement);
            let failed = browser.fail_request(request_id, error);
            (failed, browser.error_generation)
        });
        let Some((failed, generation)) = result else {
            return false;
        };
        if failed {
            self.schedule_sftp_error_clear(session_id, placement, generation, cx);
            cx.notify();
        }
        failed
    }

    fn schedule_sftp_error_clear(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            Timer::after(SFTP_ERROR_HINT_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                let cleared = this.session_mut(session_id).is_some_and(|session| {
                    session
                        .sftp_browser_mut(placement)
                        .clear_error_if_current(generation)
                });
                if cleared {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn request_sftp_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let capability_blocks_request = self.session(session_id).is_some_and(|session| {
            session.connection_state == SessionState::Connected
                && session.sftp_availability != SftpAvailability::Available
        });
        if capability_blocks_request {
            if let Some(session) = self.session_mut(session_id) {
                session.sftp_browser_mut(placement).loading = false;
            }
            cx.notify();
            return;
        }

        let handle = self.session(session_id).and_then(|session| {
            (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten()
        });
        let Some(handle) = handle else {
            if let Some(session) = self.session_mut(session_id) {
                session.sftp_browser_mut(placement).loading = false;
            }
            self.show_sftp_error(session_id, placement, self.tr("sftp-connect-browse"), cx);
            return;
        };

        let (request_id, request_path) = {
            let session = self
                .session_mut(session_id)
                .expect("SFTP session should still exist");
            let request_id = session
                .sftp_browser_mut(placement)
                .begin_request(path.clone());
            (request_id, path)
        };

        if let Err(error) = handle.read_directory(request_id, request_path) {
            self.fail_sftp_request(session_id, placement, request_id, error.to_string(), cx);
        }
        cx.notify();
    }

    fn refresh_active_sftp_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let path = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        self.request_sftp_directory(session_id, placement, path, cx);
    }

    fn refresh_sftp_directory_for_session(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let path = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone());
        if let Some(path) = path {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    fn toggle_remote_tree_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let expanded = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .expanded_paths
                .contains(&path)
        });
        if expanded {
            if let Some(session) = self.session_mut(session_id) {
                session
                    .sftp_browser_mut(placement)
                    .expanded_paths
                    .remove(&path);
            }
            cx.notify();
            return;
        }
        self.expand_remote_tree_directory(session_id, placement, path, cx);
    }

    fn expand_remote_tree_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let cached = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .tree_entries
                .contains_key(&path)
        });
        let expanded = self.session(session_id).is_some_and(|session| {
            session
                .sftp_browser(placement)
                .expanded_paths
                .contains(&path)
        });
        if expanded {
            return;
        }
        if cached {
            if let Some(session) = self.session_mut(session_id) {
                session
                    .sftp_browser_mut(placement)
                    .expanded_paths
                    .insert(path);
            }
            cx.notify();
            return;
        }

        let handle = self
            .session(session_id)
            .and_then(|session| session.connection_handle.clone());
        let Some(handle) = handle else {
            return;
        };
        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp_browser_mut(placement)
            .begin_tree_request(path.clone());
        if let Err(error) = handle.read_directory(request_id, path) {
            self.fail_sftp_request(session_id, placement, request_id, error.to_string(), cx);
        }
        cx.notify();
    }

    fn open_remote_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.active_session_id {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    fn open_parent_remote_directory(
        &mut self,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some((session_id, parent)) = self.active_session_id.and_then(|session_id| {
            self.session(session_id)
                .and_then(|session| remote_parent_path(&session.sftp_browser(placement).path))
                .map(|parent| (session_id, parent))
        }) else {
            return;
        };
        self.request_sftp_directory(session_id, placement, parent, cx);
    }

    fn open_remote_file(&mut self, path: String, editable: bool, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let handle = self.session(session_id).and_then(|session| {
            (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten()
        });
        let Some(handle) = handle else {
            return;
        };

        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp
            .begin_file_request(path.clone(), editable);
        if let Some(tab_id) = self.active_tab_id
            && let Some(tab) = self.tab_mut(tab_id)
        {
            tab.view = TerminalTabView::Files;
        }
        if let Err(error) = handle.read_file(request_id, path)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp
                .fail_file_request(request_id, SftpOperation::ReadFile, error.to_string());
        }
        cx.notify();
    }

    fn complete_remote_file_read(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        file: RemoteFile,
        cx: &mut Context<Self>,
    ) {
        let editable = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .filter(|state| state.read_request_id == Some(request_id))
            .map(|state| state.editable);
        let Some(editable) = editable else {
            return;
        };

        let decoded = RemoteTextFormat::decode(&file.contents);
        let editor = decoded.as_ref().map(|(_, text)| {
            let editor = cx.new(|cx| {
                if editable {
                    FileEditor::new(cx, text.clone())
                } else {
                    FileEditor::new_read_only(cx, text.clone())
                }
            });
            cx.observe(&editor, |_, _, cx| cx.notify()).detach();
            cx.subscribe(&editor, move |this, _, event, cx| match event {
                FileEditorEvent::SaveRequested => this.save_remote_file(session_id, cx),
            })
            .detach();
            editor
        });

        if let Some(state) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
            .filter(|state| state.read_request_id == Some(request_id))
        {
            state.path = file.path;
            state.original_contents = file.contents;
            state.text_format = decoded.map(|(format, _)| format);
            state.editor = editor;
            state.loading = false;
            state.error = None;
            state.read_request_id = None;
        }
    }

    fn save_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some((handle, path, expected_contents, contents)) =
            self.session(session_id).and_then(|session| {
                let handle = (session.connection_state == SessionState::Connected)
                    .then(|| session.connection_handle.clone())
                    .flatten()?;
                let file = session.sftp.file.as_ref()?;
                let contents = file.edited_contents(cx)?;
                (!file.loading && !file.saving && contents != file.original_contents).then(|| {
                    (
                        handle,
                        file.path.clone(),
                        file.original_contents.clone(),
                        contents,
                    )
                })
            })
        else {
            return;
        };

        if contents.len() > MAX_REMOTE_FILE_BYTES {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("size", MAX_REMOTE_FILE_BYTES / 1024 / 1024);
            let message = self.tr_with("sftp-file-limit", &args);
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(message);
            }
            cx.notify();
            return;
        }

        let Some(request_id) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.begin_file_save())
        else {
            return;
        };
        if let Err(error) = handle.write_file(request_id, path, expected_contents, contents)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp
                .fail_file_request(request_id, SftpOperation::WriteFile, error.to_string());
        }
        cx.notify();
    }

    fn complete_remote_file_write(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        file: RemoteFile,
    ) {
        let Some(state) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
            .filter(|state| state.write_request_id == Some(request_id))
        else {
            return;
        };
        state.path = file.path;
        state.original_contents = file.contents;
        state.saving = false;
        state.error = None;
        state.write_request_id = None;
    }

    fn revert_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let replacement = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .and_then(|file| {
                Some((
                    file.editor.clone()?,
                    RemoteTextFormat::decode(&file.original_contents)?.1,
                ))
            });
        if let Some((editor, text)) = replacement {
            editor.update(cx, |editor, cx| editor.replace_all(text, cx));
        }
        if let Some(file) = self
            .session_mut(session_id)
            .and_then(|session| session.sftp.file.as_mut())
        {
            file.error = None;
        }
        cx.notify();
    }

    fn close_remote_file(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let dirty = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .is_some_and(|file| file.is_dirty(cx));
        if dirty {
            let message = self.tr("sftp-save-before-close");
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(message);
            }
        } else if let Some(session) = self.session_mut(session_id) {
            session.sftp.file = None;
        }
        cx.notify();
    }

    fn choose_sftp_uploads(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let Some(remote_directory) = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
        else {
            return;
        };
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.tr("sftp-upload").into()),
        });
        let runtime = cx.global::<SshRuntime>().handle();

        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                let plan = runtime
                    .spawn_blocking(move || build_local_upload_plan(&paths, &remote_directory))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    let Ok(Ok(plan)) = plan else {
                        let message = match plan {
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error.to_string(),
                            Ok(Ok(_)) => unreachable!(),
                        };
                        this.show_sftp_error(
                            session_id,
                            placement,
                            format!("{}: {message}", this.tr("sftp-prepare-upload-failed")),
                            cx,
                        );
                        return;
                    };

                    if !plan.directories.is_empty() {
                        let request_id = this
                            .session_mut(session_id)
                            .map(|session| session.sftp_browser_mut(placement).next_request_id());
                        let create_result = request_id
                            .zip(
                                this.session(session_id)
                                    .and_then(|session| session.connection_handle.clone()),
                            )
                            .map(|(request_id, handle)| {
                                handle.create_directories(request_id, plan.directories)
                            });
                        if !matches!(create_result, Some(Ok(()))) {
                            this.show_sftp_error(
                                session_id,
                                placement,
                                this.tr("sftp-queue-directory-failed"),
                                cx,
                            );
                            return;
                        }
                    }

                    let Some(batch_id) = this
                        .session_mut(session_id)
                        .map(|session| session.transfers.begin_batch())
                    else {
                        return;
                    };
                    for (local_path, remote_path) in plan.files {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferSpec {
                                batch_id,
                                direction: SftpTransferDirection::Upload,
                                local_path,
                                remote_path,
                                overwrite: false,
                                expected_total: None,
                            },
                            cx,
                        );
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-open-upload-failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    fn choose_sftp_downloads(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        entries: Vec<RemoteFileEntry>,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return;
        }
        let entries = collapse_nested_remote_entries(entries);
        let browser_root = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        let selected_paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.tr("sftp-download").into()),
        });

        cx.spawn(async move |this, cx| match selected_paths.await {
            Ok(Ok(Some(paths))) => {
                let Some(destination) = paths.into_iter().next() else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    let handle = this
                        .session(session_id)
                        .and_then(|session| session.connection_handle.clone());
                    let Some(handle) = handle else {
                        return;
                    };
                    let Some(batch_id) = this
                        .session_mut(session_id)
                        .map(|session| session.transfers.begin_batch())
                    else {
                        return;
                    };

                    for entry in entries {
                        let local_path = remote_relative_path(&browser_root, &entry.path)
                            .map(|relative| join_remote_relative(&destination, relative))
                            .unwrap_or_else(|| destination.join(&entry.name));
                        if entry.kind == RemoteFileKind::Directory {
                            let request_id = this
                                .session_mut(session_id)
                                .expect("SFTP session should still exist")
                                .sftp_browser_mut(placement)
                                .begin_download_tree(local_path, batch_id);
                            if let Err(error) =
                                handle.read_directory_tree(request_id, entry.path.clone())
                            {
                                this.fail_sftp_request(
                                    session_id,
                                    placement,
                                    request_id,
                                    error.to_string(),
                                    cx,
                                );
                            }
                        } else if entry.kind == RemoteFileKind::File {
                            this.enqueue_sftp_transfer(
                                session_id,
                                SftpTransferSpec {
                                    batch_id,
                                    direction: SftpTransferDirection::Download,
                                    local_path,
                                    remote_path: entry.path,
                                    overwrite: false,
                                    expected_total: entry.size,
                                },
                                cx,
                            );
                        }
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-open-download-failed")),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    fn complete_directory_tree_download(
        &mut self,
        session_id: SessionId,
        request_id: u64,
        tree: RemoteDirectoryTree,
        cx: &mut Context<Self>,
    ) {
        let placement = sftp_browser_placement_for_request(request_id);
        let pending = self.session_mut(session_id).and_then(|session| {
            session
                .sftp_browser_mut(placement)
                .take_download_tree(request_id)
        });
        let Some(pending) = pending else {
            return;
        };
        let PendingSftpDownloadTree {
            destination,
            batch_id,
        } = pending;
        let runtime = cx.global::<SshRuntime>().handle();

        cx.spawn(async move |this, cx| {
            let plan = runtime
                .spawn_blocking(move || build_remote_download_plan(tree, destination))
                .await;
            let _ = this.update(cx, |this, cx| match plan {
                Ok(Ok(plan)) => {
                    for (local_path, remote_path, size) in plan {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferSpec {
                                batch_id,
                                direction: SftpTransferDirection::Download,
                                local_path,
                                remote_path,
                                overwrite: false,
                                expected_total: size,
                            },
                            cx,
                        );
                    }
                }
                Ok(Err(error)) => {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-prepare-download-failed")),
                        cx,
                    );
                }
                Err(error) => {
                    this.show_sftp_error(
                        session_id,
                        placement,
                        format!("{}: {error}", this.tr("sftp-download-task-failed")),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn selected_sftp_entries(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
    ) -> Vec<RemoteFileEntry> {
        self.session(session_id)
            .map(|session| session.sftp_browser(placement).selected_entries())
            .unwrap_or_default()
    }

    fn download_selected_sftp_entries(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let entries = self.selected_sftp_entries(session_id, placement);
        self.sftp_context_menu = None;
        self.choose_sftp_downloads(session_id, placement, entries, cx);
    }

    fn open_selected_sftp_file(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        editable: bool,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .selected_sftp_entries(session_id, placement)
            .into_iter()
            .next()
            .filter(|entry| entry.kind == RemoteFileKind::File);
        self.sftp_context_menu = None;
        if let Some(entry) = entry {
            self.open_remote_file(entry.path, editable, cx);
        }
    }

    fn copy_selected_sftp_paths(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) {
        let paths = self
            .selected_sftp_entries(session_id, placement)
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(paths.join("\n")));
        }
        self.sftp_context_menu = None;
        cx.notify();
    }

    fn delete_selected_sftp_entries(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries =
            collapse_nested_remote_entries(self.selected_sftp_entries(session_id, placement));
        if entries.is_empty() {
            return;
        }
        let paths = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let message = if paths.len() == 1 {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("name", entries[0].name.clone());
            self.tr_with("sftp-delete-one", &args)
        } else {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("count", paths.len());
            self.tr_with("sftp-delete-many", &args)
        };
        self.sftp_context_menu = None;
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some(&self.tr("sftp-delete-recursive")),
            &[
                PromptButton::new(self.tr("common-delete")),
                PromptButton::cancel(self.tr("common-cancel")),
            ],
            cx,
        );

        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let handle = this
                    .session(session_id)
                    .and_then(|session| session.connection_handle.clone());
                let Some(handle) = handle else {
                    return;
                };
                let request_id = this
                    .session_mut(session_id)
                    .expect("SFTP session should still exist")
                    .sftp_browser_mut(placement)
                    .next_request_id();
                if let Err(error) = handle.delete_paths(request_id, paths) {
                    this.show_sftp_error(session_id, placement, error.to_string(), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_sftp_create_prompt(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        kind: SftpCreateKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = match kind {
            SftpCreateKind::File => self.tr("sftp-file-name"),
            SftpCreateKind::Directory => self.tr("sftp-folder-name"),
        };
        let input = cx.new(|cx| TextField::new(cx, "", placeholder));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        input.focus_handle(cx).focus(window);
        self.sftp_context_menu = None;
        self.sftp_create_prompt = Some(SftpCreatePrompt {
            session_id,
            placement,
            kind,
            input,
            error: None,
        });
        cx.notify();
    }

    fn submit_sftp_create(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.sftp_create_prompt.as_ref() else {
            return;
        };
        let name = prompt.input.read(cx).text().trim().to_owned();
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            let message = self.tr("sftp-valid-name");
            if let Some(prompt) = self.sftp_create_prompt.as_mut() {
                prompt.error = Some(message);
            }
            cx.notify();
            return;
        }

        let session_id = prompt.session_id;
        let placement = prompt.placement;
        let kind = prompt.kind;
        let directory = self
            .session(session_id)
            .map(|session| session.sftp_browser(placement).path.clone())
            .unwrap_or_else(|| ".".into());
        let path = remote_join_path(&directory, &name);
        let handle = self
            .session(session_id)
            .and_then(|session| session.connection_handle.clone());
        let Some(handle) = handle else {
            return;
        };
        let request_id = self
            .session_mut(session_id)
            .expect("SFTP session should still exist")
            .sftp_browser_mut(placement)
            .next_request_id();
        let result = match kind {
            SftpCreateKind::File => handle.create_file(request_id, path),
            SftpCreateKind::Directory => handle.create_directories(request_id, vec![path]),
        };
        match result {
            Ok(()) => {
                self.sftp_create_prompt = None;
            }
            Err(error) => {
                if let Some(prompt) = self.sftp_create_prompt.as_mut() {
                    prompt.error = Some(error.to_string());
                }
            }
        }
        cx.notify();
    }

    fn enqueue_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer: SftpTransferSpec,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.transfers.enqueue_in_batch(
            transfer.batch_id,
            transfer.direction,
            transfer.local_path,
            transfer.remote_path,
            transfer.overwrite,
            transfer.expected_total,
        );
        self.start_queued_sftp_transfers(cx);
        cx.notify();
    }

    fn active_sftp_transfer_count(&self) -> usize {
        self.sessions
            .iter()
            .map(|session| session.transfers.active_count())
            .sum()
    }

    fn take_next_queued_sftp_transfer(
        &mut self,
    ) -> Option<(SessionId, Option<ConnectionHandle>, SftpTransferTask)> {
        if self.sessions.is_empty() {
            self.next_transfer_session_cursor = 0;
            return None;
        }

        let session_count = self.sessions.len();
        for offset in 0..session_count {
            let index = (self.next_transfer_session_cursor + offset) % session_count;
            let session = &mut self.sessions[index];
            let Some(task) = session.transfers.start_next() else {
                continue;
            };
            let handle = (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten();
            self.next_transfer_session_cursor = (index + 1) % session_count;
            return Some((session.id, handle, task));
        }
        None
    }

    fn start_queued_sftp_transfers(&mut self, cx: &mut Context<Self>) {
        let connect_before_transfer = self.tr("sftp-connect-transfer");
        loop {
            if self.active_sftp_transfer_count()
                >= usize::from(self.transfer_settings.max_parallel_transfers)
            {
                break;
            }
            let Some((session_id, handle, task)) = self.take_next_queued_sftp_transfer() else {
                break;
            };
            let transfer_id = task.id;

            let result = match handle {
                Some(handle) => match task.direction {
                    SftpTransferDirection::Upload => handle.upload_file(
                        task.id,
                        task.local_path,
                        task.remote_path,
                        task.overwrite,
                    ),
                    SftpTransferDirection::Download => handle.download_file(
                        task.id,
                        task.remote_path,
                        task.local_path,
                        task.overwrite,
                    ),
                },
                None => Err(remcmd_ssh::SshError::new(
                    SshErrorKind::InvalidState,
                    connect_before_transfer.clone(),
                )),
            };

            match result {
                Ok(()) => {}
                Err(error) => {
                    if let Some(session) = self.session_mut(session_id) {
                        session
                            .transfers
                            .mark_failed(transfer_id, error.to_string());
                    }
                }
            }
        }
        cx.notify();
    }

    fn cancel_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some((signal_worker, handle)) = self.session_mut(session_id).and_then(|session| {
            let signal_worker = session.transfers.begin_cancel(transfer_id)?;
            Some((signal_worker, session.connection_handle.clone()))
        }) else {
            return;
        };

        if signal_worker {
            let task_missing = self.tr("sftp-task-missing");
            let result = handle
                .ok_or_else(|| remcmd_ssh::SshError::new(SshErrorKind::InvalidState, task_missing))
                .and_then(|handle| handle.cancel_transfer(transfer_id));
            if let Err(error) = result
                && let Some(session) = self.session_mut(session_id)
            {
                session
                    .transfers
                    .mark_failed(transfer_id, error.to_string());
                self.start_queued_sftp_transfers(cx);
            }
        } else {
            self.start_queued_sftp_transfers(cx);
        }
        cx.notify();
    }

    fn replace_sftp_transfer_destination(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        cx: &mut Context<Self>,
    ) {
        if self
            .session_mut(session_id)
            .is_some_and(|session| session.transfers.retry_with_overwrite(transfer_id))
        {
            self.start_queued_sftp_transfers(cx);
        }
        cx.notify();
    }

    fn clear_finished_sftp_transfers(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.session_mut(session_id) {
            session.transfers.clear_finished();
        }
        cx.notify();
    }

    fn complete_sftp_transfer(
        &mut self,
        session_id: SessionId,
        transfer_id: u64,
        direction: SftpTransferDirection,
        remote_path: String,
        bytes: u64,
        cx: &mut Context<Self>,
    ) {
        let completed = self
            .session_mut(session_id)
            .is_some_and(|session| session.transfers.mark_completed(transfer_id, bytes));
        if !completed {
            return;
        }

        if direction == SftpTransferDirection::Upload {
            let parent = remote_parent_path(&remote_path).unwrap_or_else(|| ".".into());
            let placements = [SftpBrowserPlacement::Center, SftpBrowserPlacement::Sidebar]
                .into_iter()
                .filter(|placement| {
                    self.session(session_id).is_some_and(|session| {
                        let browser = session.sftp_browser(*placement);
                        browser.loaded && browser.path == parent
                    })
                })
                .collect::<Vec<_>>();
            for placement in placements {
                self.request_sftp_directory(session_id, placement, parent.clone(), cx);
            }
        }

        self.start_queued_sftp_transfers(cx);
    }

    fn create_tab_for_session(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TabId {
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let pane_id = self.create_terminal_pane(tab_id, session_id, window, cx);
        self.tabs.push(TerminalTab {
            id: tab_id,
            profile_id,
            layout: PaneLayout::Pane(pane_id),
            active_pane_id: pane_id,
            view: TerminalTabView::Terminal,
        });
        self.animate_titlebar_tabs_to_end(cx);
        tab_id
    }

    fn animate_titlebar_tabs_to_end(&mut self, cx: &mut Context<Self>) {
        if self.tab_layout != TabLayout::Horizontal {
            return;
        }

        self.titlebar_tabs_scroll_start = self.titlebar_tabs_scroll_handle.offset();
        self.titlebar_tabs_scroll_transition_id += 1;
        self.titlebar_tabs_scroll_active = true;
        let transition_id = self.titlebar_tabs_scroll_transition_id;
        let scroll_handle = self.titlebar_tabs_scroll_handle.clone();
        self.titlebar_tabs_scroll_cleanup_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(MOTION_STANDARD_DURATION + TERMINAL_REDRAW_INTERVAL).await;
            let _ = this.update(cx, |this, cx| {
                if this.titlebar_tabs_scroll_transition_id == transition_id {
                    scroll_handle.set_offset(point(-scroll_handle.max_offset().width, px(0.0)));
                    this.titlebar_tabs_scroll_active = false;
                    this.titlebar_tabs_scroll_cleanup_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn pane(&self, pane_id: PaneId) -> Option<&TerminalPane> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    fn pane_mut(&mut self, pane_id: PaneId) -> Option<&mut TerminalPane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    fn pane_for_session(&self, session_id: SessionId) -> Option<&TerminalPane> {
        self.panes.iter().find(|pane| pane.session_id == session_id)
    }

    fn active_pane(&self) -> Option<&TerminalPane> {
        self.active_pane_id.and_then(|pane_id| self.pane(pane_id))
    }

    fn create_terminal_pane(
        &mut self,
        tab_id: TabId,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PaneId {
        let pane_id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        let focus_handle = cx.focus_handle();

        cx.on_focus(&focus_handle, window, move |this, _, cx| {
            this.handle_pane_focus(pane_id, true, cx);
        })
        .detach();
        cx.on_blur(&focus_handle, window, move |this, _, cx| {
            this.handle_pane_focus(pane_id, false, cx);
        })
        .detach();

        self.panes.push(TerminalPane {
            id: pane_id,
            tab_id,
            session_id,
            focus_handle,
            focused: false,
        });
        pane_id
    }

    fn set_active_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> bool {
        let Some((tab_id, session_id, profile_id)) = self.pane(pane_id).and_then(|pane| {
            self.session(pane.session_id)
                .map(|session| (pane.tab_id, session.id, session.profile_id.clone()))
        }) else {
            return false;
        };

        if let Some(tab) = self.tab_mut(tab_id) {
            tab.active_pane_id = pane_id;
        }
        if self.active_tab_id != Some(tab_id) {
            self.previous_active_tab_id = self.active_tab_id;
            self.titlebar_tab_transition_id += 1;
        }
        self.active_tab_id = Some(tab_id);
        self.active_pane_id = Some(pane_id);
        self.active_session_id = Some(session_id);
        self.active_panel = ActivePanel::Connection;
        self.open_settings_selector = None;
        self.selected_profile_id = Some(profile_id);
        if self.right_sidebar_open && self.right_sidebar_view == RightSidebarView::Sftp {
            self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
        }
        if self
            .tab(tab_id)
            .is_some_and(|tab| tab.view == TerminalTabView::Files)
        {
            self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Center, cx);
        }
        self.sync_performance_monitoring();
        true
    }

    fn handle_pane_focus(&mut self, pane_id: PaneId, focused: bool, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        pane.focused = focused;
        let session_id = pane.session_id;
        if focused {
            self.set_active_pane(pane_id, cx);
            self.focused_terminal_session_id = Some(session_id);
        } else if self.focused_terminal_session_id == Some(session_id) {
            self.focused_terminal_session_id = None;
        }

        let modes = self
            .session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .map(ActiveTerminal::modes)
            .unwrap_or(TerminalModes::NONE);
        if let Some(bytes) = encode_focus(focused, modes) {
            self.send_terminal_response(session_id, bytes);
        }
        cx.notify();
    }

    fn activate_session_in_window(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pane_for_session(session_id).is_none() {
            let Some(profile_id) = self
                .session(session_id)
                .map(|session| session.profile_id.clone())
            else {
                return false;
            };
            self.create_tab_for_session(session_id, profile_id, window, cx);
        }
        if !self.activate_session(session_id, cx) {
            return false;
        }

        if let Some(focus_handle) = self.active_pane().map(|pane| pane.focus_handle.clone()) {
            focus_handle.focus(window);
        }
        true
    }

    fn activate_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
        let Some(pane_id) = self.pane_for_session(session_id).map(|pane| pane.id) else {
            return false;
        };

        self.dismiss_credential_prompt(cx);
        self.set_active_pane(pane_id, cx)
    }

    fn activate_tab_in_window(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.tab(tab_id).map(|tab| tab.active_pane_id) else {
            return false;
        };
        if !self.set_active_pane(pane_id, cx) {
            return false;
        }
        if let Some(focus_handle) = self.active_pane().map(|pane| pane.focus_handle.clone()) {
            focus_handle.focus(window);
        }
        true
    }

    fn remove_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.panes.iter().position(|pane| pane.id == pane_id) else {
            return false;
        };
        let tab_id = self.panes[index].tab_id;
        let Some(layout) = self.tab(tab_id).map(|tab| tab.layout.clone()) else {
            return false;
        };
        let (next_layout, removed) = layout.without(pane_id);
        if !removed {
            return false;
        }

        let (session_id, focused) = {
            let pane = &self.panes[index];
            (pane.session_id, pane.focused)
        };
        if focused {
            let modes = self
                .session(session_id)
                .and_then(|session| session.terminal.as_ref())
                .map(ActiveTerminal::modes)
                .unwrap_or(TerminalModes::NONE);
            if let Some(bytes) = encode_focus(false, modes) {
                self.send_terminal_response(session_id, bytes);
            }
        }
        self.panes.remove(index);
        if let Some(next_layout) = next_layout {
            let replacement_pane = next_layout.first_pane();
            if let Some(tab) = self.tab_mut(tab_id) {
                tab.layout = next_layout;
                if tab.active_pane_id == pane_id {
                    tab.active_pane_id = replacement_pane;
                }
            }
            if self.active_tab_id == Some(tab_id) && self.active_pane_id == Some(pane_id) {
                self.set_active_pane(replacement_pane, cx);
            }
        } else {
            self.remove_tab_record(tab_id, cx);
        }
        true
    }

    fn remove_tab_record(&mut self, tab_id: TabId, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return false;
        };
        let was_active = self.active_tab_id == Some(tab_id);
        let replacement = index
            .checked_sub(1)
            .and_then(|index| self.tabs.get(index))
            .or_else(|| self.tabs.get(index + 1))
            .map(|tab| tab.id);
        self.tabs.remove(index);

        if was_active {
            self.active_tab_id = None;
            self.active_pane_id = None;
            self.active_session_id = None;
            if let Some(replacement) = replacement {
                let pane_id = self
                    .tab(replacement)
                    .expect("replacement tab should remain present")
                    .active_pane_id;
                self.set_active_pane(pane_id, cx);
            }
        }
        self.sync_performance_monitoring();
        true
    }

    fn remove_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| session.id == session_id)
        else {
            return false;
        };
        if let Some(pane_id) = self.pane_for_session(session_id).map(|pane| pane.id) {
            self.remove_pane(pane_id, cx);
        }
        self.sessions.remove(index);

        if self.credential_lookup_session_id == Some(session_id) {
            self.credential_lookup_task = None;
            self.credential_lookup_session_id = None;
        }
        if self
            .pending_connection
            .as_ref()
            .is_some_and(|pending| pending.session_id == session_id)
        {
            self.pending_connection = None;
        }
        if self
            .proxy_command_approval_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.session_id == session_id)
        {
            self.proxy_command_approval_prompt = None;
            self.pending_proxy_approval.remove(&session_id);
        }
        if self
            .credential_prompt
            .as_ref()
            .is_some_and(|prompt| prompt.session_id == session_id)
        {
            self.dismiss_credential_prompt(cx);
        }

        if self.active_session_id == Some(session_id) {
            self.active_session_id = self
                .active_pane()
                .map(|pane| pane.session_id)
                .filter(|replacement| *replacement != session_id);
        }
        if self.quick_terminal_session_id == Some(session_id) {
            self.quick_terminal_session_id = None;
        }
        if self.focused_terminal_session_id == Some(session_id) {
            self.focused_terminal_session_id = None;
        }
        if self
            .terminal_context_menu
            .as_ref()
            .is_some_and(|menu| menu.session_id == session_id)
        {
            self.terminal_context_menu = None;
        }

        self.sync_performance_monitoring();
        true
    }

    fn persist_profiles(&mut self) {
        if let Err(error) = save_profiles(&self.profiles_path, &self.profiles) {
            self.form_error = Some(format!("{}:\n{error}", self.tr("app-save-profiles-failed")));
        }
    }

    fn tr(&self, key: &str) -> String {
        self.localizer.text(key)
    }

    fn tr_with(&self, key: &str, args: &fluent_bundle::FluentArgs<'_>) -> String {
        self.localizer.text_with(key, Some(args))
    }

    fn set_language_mode(&mut self, language_mode: LanguageMode, cx: &mut Context<Self>) {
        self.language_mode = language_mode;
        self.localizer = Localizer::new(language_mode);

        let sidebar_placeholder = self.tr("sidebar-search-placeholder");
        self.sidebar_search.update(cx, |field, cx| {
            field.set_placeholder(sidebar_placeholder, cx);
        });
        if let Some(editor) = self.editor.as_ref() {
            for (field, key) in [
                (editor.name.clone(), "field-name"),
                (editor.host.clone(), "field-host"),
                (editor.port.clone(), "field-port"),
                (editor.username.clone(), "field-username"),
                (editor.private_key_path.clone(), "field-private-key"),
                (editor.proxy_host.clone(), "field-proxy-host"),
                (editor.proxy_port.clone(), "field-proxy-port"),
                (editor.proxy_username.clone(), "field-proxy-username"),
                (editor.proxy_password.clone(), "field-proxy-password"),
                (editor.proxy_command.clone(), "field-proxy-command"),
                (editor.jump_search.clone(), "sidebar-search-placeholder"),
            ] {
                let placeholder = self.tr(key);
                field.update(cx, |field, cx| field.set_placeholder(placeholder, cx));
            }
        }
        if let Some(prompt) = self.credential_prompt.as_ref() {
            let key = match prompt.kind {
                CredentialPromptKind::Password => "credential-password",
                CredentialPromptKind::PrivateKeyPassphrase { .. } => "credential-passphrase",
                CredentialPromptKind::ProxyPassword => "field-proxy-password",
            };
            let placeholder = self.tr(key);
            prompt
                .input
                .update(cx, |field, cx| field.set_placeholder(placeholder, cx));
        }
        if let Some(prompt) = self.sftp_create_prompt.as_ref() {
            let placeholder = self.tr(match prompt.kind {
                SftpCreateKind::File => "sftp-file-name",
                SftpCreateKind::Directory => "sftp-folder-name",
            });
            prompt
                .input
                .update(cx, |field, cx| field.set_placeholder(placeholder, cx));
        }
        if let Some(prompt) = self.quick_command_prompt.as_ref() {
            let placeholder = self.tr("field-command");
            prompt
                .input
                .update(cx, |field, cx| field.set_placeholder(placeholder, cx));
        }
        let sftp_ssh_only = self.tr("sftp-ssh-only");
        for session in &mut self.sessions {
            if session.is_local() {
                session.sftp_availability = SftpAvailability::Unavailable(sftp_ssh_only.clone());
            }
        }
        if let Some(about_window) = self.about_window {
            let title = self.tr("about-title");
            let _ = about_window.update(cx, move |about, window, cx| {
                about.language_mode = language_mode;
                window.set_window_title(&title);
                cx.notify();
            });
        }

        cx.set_menus(application_menus(&self.localizer));
        self.persist_settings();
        cx.notify();
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

        self.persist_settings();
        cx.notify();
    }

    fn set_tab_layout(&mut self, tab_layout: TabLayout, cx: &mut Context<Self>) {
        self.tab_layout = tab_layout;
        self.persist_settings();
        cx.notify();
    }

    fn set_terminal_font_family(
        &mut self,
        terminal_font_family: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_selector = None;
        self.terminal_font_family = terminal_font_family;
        self.persist_settings();
        cx.notify();
    }

    fn set_terminal_font_size(&mut self, terminal_font_size: u16, cx: &mut Context<Self>) {
        self.terminal_font_size = TerminalSettings {
            font_family: None,
            font_size: terminal_font_size,
        }
        .normalized()
        .font_size;
        self.persist_settings();
        cx.notify();
    }

    fn set_transfer_rate_limit(&mut self, rate_limit_mib_per_second: u32, cx: &mut Context<Self>) {
        self.transfer_settings.rate_limit_mib_per_second = rate_limit_mib_per_second;
        self.transfer_settings = self.transfer_settings.normalized();
        self.transfer_rate_limiter
            .set_bytes_per_second(self.transfer_settings.bytes_per_second());
        self.persist_settings();
        cx.notify();
    }

    fn set_max_parallel_transfers(&mut self, max_parallel_transfers: u8, cx: &mut Context<Self>) {
        self.transfer_settings.max_parallel_transfers = max_parallel_transfers;
        self.transfer_settings = self.transfer_settings.normalized();
        self.persist_settings();
        self.start_queued_sftp_transfers(cx);
        cx.notify();
    }

    fn settings_value(&self, selector: SettingsSelector) -> SettingsValue {
        match selector {
            SettingsSelector::Language => SettingsValue::Language(self.language_mode),
            SettingsSelector::Theme => SettingsValue::Theme(self.theme_mode),
            SettingsSelector::TabLayout => SettingsValue::TabLayout(self.tab_layout),
            SettingsSelector::TerminalFont => {
                unreachable!("terminal font choices are generated from installed fonts")
            }
            SettingsSelector::TerminalFontSize => {
                SettingsValue::TerminalFontSize(self.terminal_font_size)
            }
            SettingsSelector::TransferRate => {
                SettingsValue::TransferRate(self.transfer_settings.rate_limit_mib_per_second)
            }
            SettingsSelector::ParallelTransfers => {
                SettingsValue::ParallelTransfers(self.transfer_settings.max_parallel_transfers)
            }
        }
    }

    fn settings_value_label(&self, selector: SettingsSelector) -> SharedString {
        if selector == SettingsSelector::TerminalFont {
            return self.terminal_font_family.clone();
        }

        let value = self.settings_value(selector);
        if let Some(option) = selector
            .options()
            .iter()
            .find(|option| option.value == value)
        {
            return self.settings_option_label(option).into();
        }

        match value {
            SettingsValue::TerminalFontSize(size) => format!("{size} pt").into(),
            SettingsValue::TransferRate(rate) => format!("{rate} MiB/s").into(),
            SettingsValue::ParallelTransfers(count) => count.to_string().into(),
            SettingsValue::Language(_) | SettingsValue::Theme(_) | SettingsValue::TabLayout(_) => {
                unreachable!("all enumerated settings values have labels")
            }
        }
    }

    fn settings_option_label(&self, option: &SettingsOption) -> String {
        let key = match option.value {
            SettingsValue::Language(LanguageMode::System) => Some("settings-language-system"),
            SettingsValue::Language(LanguageMode::EnUs) => Some("settings-language-english"),
            SettingsValue::Language(LanguageMode::ZhCn) => Some("settings-language-chinese"),
            SettingsValue::Theme(ThemeMode::System) => Some("common-system"),
            SettingsValue::Theme(ThemeMode::Light) => Some("common-light"),
            SettingsValue::Theme(ThemeMode::Dark) => Some("common-dark"),
            SettingsValue::TabLayout(TabLayout::Horizontal) => Some("common-horizontal"),
            SettingsValue::TabLayout(TabLayout::Vertical) => Some("common-vertical"),
            SettingsValue::TransferRate(0) => Some("common-unlimited"),
            _ => None,
        };
        key.map_or_else(|| option.label.to_owned(), |key| self.tr(key))
    }

    fn toggle_settings_selector(
        &mut self,
        selector: SettingsSelector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_settings_selector == Some(selector) {
            self.open_settings_selector = None;
        } else {
            self.settings_selector_scroll_handle = ScrollHandle::new();
            self.settings_virtual_selector_scroll_handle = UniformListScrollHandle::new();
            let (selected_index, option_count) = if selector == SettingsSelector::TerminalFont {
                (
                    self.terminal_font_families
                        .iter()
                        .position(|family| *family == self.terminal_font_family),
                    self.terminal_font_families.len(),
                )
            } else {
                let selected_value = self.settings_value(selector);
                (
                    selector
                        .options()
                        .iter()
                        .position(|option| option.value == selected_value),
                    selector.options().len(),
                )
            };
            if let Some(selected_index) = selected_index {
                if option_count > SELECT_MENU_MAX_VISIBLE_ROWS {
                    self.settings_virtual_selector_scroll_handle
                        .scroll_to_item_strict(selected_index, gpui::ScrollStrategy::Center);
                } else {
                    self.settings_selector_scroll_handle.set_offset(point(
                        px(0.0),
                        px(-select_menu_scroll_offset(selected_index, option_count)),
                    ));
                }
            }
            self.open_settings_selector = Some(selector);
        }
        self.settings_focus_handle.focus(window);
        cx.notify();
    }

    fn dismiss_settings_selector(&mut self, cx: &mut Context<Self>) {
        if self.open_settings_selector.take().is_some() {
            cx.notify();
        }
    }

    fn apply_settings_value(
        &mut self,
        value: SettingsValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_selector = None;
        match value {
            SettingsValue::Language(mode) => self.set_language_mode(mode, cx),
            SettingsValue::Theme(mode) => self.set_theme_mode(mode, window, cx),
            SettingsValue::TabLayout(layout) => self.set_tab_layout(layout, cx),
            SettingsValue::TerminalFontSize(size) => self.set_terminal_font_size(size, cx),
            SettingsValue::TransferRate(rate) => self.set_transfer_rate_limit(rate, cx),
            SettingsValue::ParallelTransfers(count) => {
                self.set_max_parallel_transfers(count, cx);
            }
        }
    }

    fn persist_settings(&mut self) {
        let settings = AppSettings {
            language_mode: self.language_mode,
            theme_mode: self.theme_mode,
            tab_layout: self.tab_layout,
            transfers: self.transfer_settings,
            terminal: TerminalSettings {
                font_family: Some(self.terminal_font_family.to_string()),
                font_size: self.terminal_font_size,
            },
        };
        self.settings_error = save_settings(&self.settings_path, &settings)
            .err()
            .map(|error| format!("{}: {error}", self.tr("app-save-settings-failed")));
    }

    fn load_editor_for_selected_profile(&mut self, cx: &mut Context<Self>) {
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

    fn open_new_profile_editor(&mut self, cx: &mut Context<Self>) {
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
}

// User interaction handlers.
impl RemCmdApp {
    fn open_credential_prompt(
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

    fn dismiss_credential_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.credential_prompt.take() {
            prompt.input.update(cx, |input, cx| input.clear(cx));
            if self.pane_for_session(prompt.session_id).is_none() {
                self.remove_session(prompt.session_id, cx);
            }
        }
    }

    fn delete_stored_credentials(
        &mut self,
        profile_id: String,
        kind: Option<CredentialKind>,
        success_message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let runtime = cx.global::<SshRuntime>().handle();
        *self
            .credential_mutations_in_progress
            .entry(profile_id.clone())
            .or_default() += 1;
        let updating_message = self.tr("credential-updating");
        if let Some(session) = self.session_for_profile_mut(&profile_id) {
            session.connection_message = Some(updating_message);
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
                        if let Some(message) = success_message
                            && remove_counter
                            && this.selected_profile_id.as_deref()
                                == Some(deleted_profile_id.as_str())
                            && let Some(session) = this.session_for_profile_mut(&deleted_profile_id)
                        {
                            session.connection_message = Some(message);
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

    fn forget_selected_credential(&mut self, cx: &mut Context<Self>) {
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
            Some(self.tr("credential-removed")),
            cx,
        );
    }

    fn select_profile(&mut self, profile_id: String, window: &mut Window, cx: &mut Context<Self>) {
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

    fn add_profile(&mut self, cx: &mut Context<Self>) {
        self.open_new_profile_editor(cx);
    }

    fn edit_profile(&mut self, profile_id: String, cx: &mut Context<Self>) {
        self.selected_profile_id = Some(profile_id);
        self.active_panel = ActivePanel::Server;
        self.active_session_id = None;
        self.profile_context_menu = None;
        self.load_editor_for_selected_profile(cx);
        cx.notify();
    }

    fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.bottom_panel_open = false;
        self.bottom_panel_resize = None;
        self.active_panel = ActivePanel::Home;
        self.active_session_id = None;
        self.focused_terminal_session_id = None;
        self.open_settings_selector = None;
        self.settings_focus_handle.focus(window);
        self.sync_performance_monitoring();
        cx.notify();
    }

    fn open_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = self.create_local_session();
        let tab_id = self.create_tab_for_session(session_id, LOCAL_PROFILE_ID.into(), window, cx);
        self.activate_tab_in_window(tab_id, window, cx);
        self.start_local_terminal(session_id, cx);
        cx.notify();
    }

    fn open_terminal_for_current_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_session().is_some_and(TerminalSession::is_local) {
            self.open_local_terminal(window, cx);
        } else {
            self.connect_selected_profile_in_new_session(window, cx);
        }
    }

    fn start_local_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let size = PtySize::new(TERMINAL_COLUMNS, TERMINAL_ROWS);
        let terminal = LocalTerminal::spawn(local_pty_size(size));
        let (handle, mut events) = terminal.split();
        let starting_message = self.tr("terminal-starting-local");
        let sftp_unavailable = self.tr("sftp-ssh-only");

        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.close_when_disconnected = false;
        session.connection_state = SessionState::Connecting;
        session.connection_handle = None;
        session.local_terminal_handle = Some(handle);
        session.connection_error = None;
        session.connection_message = Some(starting_message);
        session.terminal_end_reason = None;
        session.terminal = Some(ActiveTerminal::new(LOCAL_PROFILE_ID.into(), size));
        session.terminal_marked_text.clear();
        session.terminal_selection = None;
        session.terminal_selecting = false;
        session.terminal_scroll_accumulator = 0.0;
        session.terminal_resize_task = None;
        session.sftp_availability = SftpAvailability::Unavailable(sftp_unavailable);

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
                            this.handle_local_terminal_event(session_id, event, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.bottom_panel_open = false;
        self.bottom_panel_resize = None;
        self.terminal_context_menu = None;
        self.active_panel = ActivePanel::Settings;
        self.open_settings_selector = None;
        self.settings_focus_handle.focus(window);
        cx.notify();
    }

    fn show_openssh_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn choose_openssh_config(&mut self, cx: &mut Context<Self>) {
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

    fn load_openssh_preview(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.openssh_import_loading {
            return;
        }
        self.openssh_import_loading = true;
        self.openssh_import_error = None;
        let profiles = self.profiles.clone();
        let runtime = cx.global::<SshRuntime>().handle();
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
                        this.openssh_import_preview = Some(preview);
                    }
                    Ok(Err(error)) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-preview-failed")));
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

    fn toggle_openssh_candidate(&mut self, alias: String, cx: &mut Context<Self>) {
        if !self.openssh_selected_aliases.remove(&alias) {
            self.openssh_selected_aliases.insert(alias);
            if let Some(preview) = self.openssh_import_preview.as_ref() {
                include_openssh_dependencies(preview, &mut self.openssh_selected_aliases);
            }
        }
        cx.notify();
    }

    fn toggle_openssh_conflict_policy(&mut self, alias: String, cx: &mut Context<Self>) {
        if !self.openssh_overwrite_conflicts.remove(&alias) {
            self.openssh_overwrite_conflicts.insert(alias);
        }
        cx.notify();
    }

    fn cycle_openssh_authentication(&mut self, alias: String, cx: &mut Context<Self>) {
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

    fn apply_openssh_preview(&mut self, cx: &mut Context<Self>) {
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
                        this.profiles = profiles;
                        this.selected_profile_id = this
                            .selected_profile_id
                            .clone()
                            .filter(|id| this.profiles.iter().any(|profile| &profile.id == id))
                            .or_else(|| this.profiles.first().map(|profile| profile.id.clone()));
                        this.openssh_import_error = None;
                        this.openssh_selected_aliases.clear();
                        this.load_openssh_preview(root_path, cx);
                    }
                    Ok(Err(error)) => {
                        this.openssh_import_error =
                            Some(format!("{}: {error}", this.tr("import-apply-failed")));
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

    fn show_about(&mut self, cx: &mut Context<Self>) {
        if let Some(window_handle) = self.about_window
            && window_handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }

        let options = about_window_options(cx, &self.localizer);
        let language_mode = self.language_mode;
        match cx.open_window(options, |_, cx| cx.new(|_| AboutWindow { language_mode })) {
            Ok(window_handle) => {
                self.about_window = Some(window_handle);
            }
            Err(error) => {
                self.settings_error =
                    Some(format!("{}: {error}", self.tr("app-open-about-failed")));
                cx.notify();
            }
        }
    }

    fn connected_ssh_profile_ids(&self) -> Vec<String> {
        self.profiles
            .iter()
            .filter(|profile| {
                self.sessions.iter().any(|session| {
                    session.kind == TerminalSessionKind::Ssh
                        && session.profile_id == profile.id
                        && session.connection_state == SessionState::Connected
                        && session.connection_handle.is_some()
                })
            })
            .map(|profile| profile.id.clone())
            .collect()
    }

    fn ensure_quick_command_prompt(&mut self, cx: &mut Context<Self>) {
        if self.quick_command_prompt.is_some() {
            return;
        }
        let input = cx.new(|cx| TextField::new(cx, "", self.tr("field-command")));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        self.quick_command_prompt = Some(QuickCommandPrompt {
            input,
            selected_profile_ids: self.connected_ssh_profile_ids().into_iter().collect(),
            selection_touched: false,
            target_menu_open: false,
            error: None,
        });
    }

    fn sync_default_quick_command_targets(&mut self) {
        let connected_profile_ids = self.connected_ssh_profile_ids();
        if let Some(prompt) = self.quick_command_prompt.as_mut()
            && !prompt.selection_touched
        {
            prompt.selected_profile_ids = connected_profile_ids.into_iter().collect();
        }
    }

    fn close_quick_command(&mut self, cx: &mut Context<Self>) {
        if self.bottom_panel_open {
            self.bottom_panel_open = false;
            self.bottom_panel_resize = None;
            cx.notify();
        }
    }

    fn toggle_bottom_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.bottom_panel_open {
            self.bottom_panel_open = false;
            self.bottom_panel_resize = None;
            cx.notify();
            return;
        }

        self.ensure_quick_command_prompt(cx);
        self.sync_default_quick_command_targets();
        self.bottom_panel_open = true;
        if self.quick_terminal_session_id.is_none() {
            let session_id = self.create_local_session();
            self.quick_terminal_session_id = Some(session_id);
            self.start_local_terminal(session_id, cx);
        }
        self.quick_terminal_focus_handle.focus(window);
        cx.notify();
    }

    fn restart_quick_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = if let Some(session_id) = self.quick_terminal_session_id {
            session_id
        } else {
            let session_id = self.create_local_session();
            self.quick_terminal_session_id = Some(session_id);
            session_id
        };
        if self
            .session(session_id)
            .is_some_and(|session| session.connection_state.can_connect())
        {
            self.start_local_terminal(session_id, cx);
        }
        self.quick_terminal_focus_handle.focus(window);
        cx.notify();
    }

    fn dispose_quick_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.quick_terminal_session_id else {
            return;
        };
        let can_disconnect = self
            .session(session_id)
            .is_some_and(|session| session.connection_state.can_disconnect());
        if can_disconnect {
            if let Some(session) = self.session_mut(session_id) {
                session.close_when_disconnected = true;
            }
            self.disconnect_session(session_id, cx);
        } else {
            self.remove_session(session_id, cx);
            cx.notify();
        }
    }

    fn toggle_quick_command_targets(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.quick_command_prompt.as_mut() {
            prompt.target_menu_open = !prompt.target_menu_open;
            cx.notify();
        }
    }

    fn toggle_quick_command_target(&mut self, profile_id: String, cx: &mut Context<Self>) {
        let connected = self
            .connected_ssh_profile_ids()
            .iter()
            .any(|candidate| candidate == &profile_id);
        if !connected {
            return;
        }
        let Some(prompt) = self.quick_command_prompt.as_mut() else {
            return;
        };
        if !prompt.selected_profile_ids.remove(&profile_id) {
            prompt.selected_profile_ids.insert(profile_id);
        }
        prompt.selection_touched = true;
        prompt.error = None;
        cx.notify();
    }

    fn submit_quick_command(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.quick_command_prompt.as_ref() else {
            return;
        };
        let command = prompt.input.read(cx).text().trim().to_owned();
        let selected_profile_ids = prompt.selected_profile_ids.clone();
        if command.is_empty() {
            let message = self.tr("quick-enter-command");
            if let Some(prompt) = self.quick_command_prompt.as_mut() {
                prompt.error = Some(message);
            }
            cx.notify();
            return;
        }
        if selected_profile_ids.is_empty() {
            let message = self.tr("quick-select-server");
            if let Some(prompt) = self.quick_command_prompt.as_mut() {
                prompt.error = Some(message);
            }
            cx.notify();
            return;
        }

        let profile_ids = self
            .profiles
            .iter()
            .map(|profile| profile.id.clone())
            .collect::<Vec<_>>();
        let session_candidates = self
            .sessions
            .iter()
            .map(|session| {
                (
                    session.id,
                    session.profile_id.as_str(),
                    session.kind == TerminalSessionKind::Ssh
                        && session.connection_state == SessionState::Connected
                        && session.connection_handle.is_some(),
                )
            })
            .collect::<Vec<_>>();
        let target_session_ids = quick_command_target_sessions(
            &profile_ids,
            &selected_profile_ids,
            &session_candidates,
            self.active_session_id,
        );
        let targets = target_session_ids
            .into_iter()
            .filter_map(|(profile_id, session_id)| {
                self.session(session_id)
                    .and_then(|session| session.connection_handle.clone())
                    .map(|handle| (profile_id, handle))
            })
            .collect::<Vec<_>>();

        if targets.is_empty() {
            let message = self.tr("quick-servers-disconnected");
            if let Some(prompt) = self.quick_command_prompt.as_mut() {
                prompt.error = Some(message);
            }
            cx.notify();
            return;
        }

        let data = format!("{command}\r").into_bytes();
        let mut failures = Vec::new();
        for (profile_id, handle) in targets {
            if let Err(error) = handle.send_input(data.clone()) {
                let label = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == profile_id)
                    .map(|profile| profile.name.as_str())
                    .unwrap_or(profile_id.as_str());
                failures.push(format!("{label}: {error}"));
            }
        }

        if failures.is_empty() {
            if let Some(prompt) = self.quick_command_prompt.as_mut() {
                prompt.error = None;
                prompt.input.update(cx, |input, cx| input.clear(cx));
            }
        } else if let Some(prompt) = self.quick_command_prompt.as_mut() {
            prompt.error = Some(failures.join("\n"));
        }
        cx.notify();
    }

    fn toggle_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn toggle_connections(&mut self, cx: &mut Context<Self>) {
        self.connections_expanded = !self.connections_expanded;
        cx.notify();
    }

    fn delete_profile(&mut self, selected_id: String, cx: &mut Context<Self>) {
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
        self.profile_auth_selector_open = false;
        self.form_error = None;

        if auth_kind == ProfileAuthKind::PrivateKey {
            window.focus(&private_key_path.focus_handle(cx));
        }

        cx.notify();
    }

    fn select_proxy_method(&mut self, proxy_kind: ProfileProxyKind, cx: &mut Context<Self>) {
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

    fn toggle_jump_host(&mut self, jump_id: String, cx: &mut Context<Self>) {
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

    fn move_jump_host(&mut self, jump_id: String, direction: isize, cx: &mut Context<Self>) {
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

    fn toggle_profile_auth_selector(&mut self, cx: &mut Context<Self>) {
        self.profile_auth_selector_open = !self.profile_auth_selector_open;
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
                    this.form_error =
                        Some(format!("{}: {error}", this.tr("app-file-picker-failed")));
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

    fn set_private_key_path(&mut self, profile_id: &str, path: PathBuf, cx: &mut Context<Self>) {
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

    fn save_editor(&mut self, cx: &mut Context<Self>) {
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

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.profile_auth_selector_open = false;
        self.form_error = None;
        cx.notify();
    }

    fn connect_selected_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };
        let session_id = self
            .selected_session()
            .map(|session| session.id)
            .unwrap_or_else(|| self.create_session_for_profile(&profile.id));
        self.connect_profile_in_session(session_id, profile, window, cx);
    }

    fn connect_selected_profile_in_new_session(
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

    fn split_active_pane(&mut self, axis: SplitAxis, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_pane_id) = self.active_pane_id else {
            return;
        };
        let Some(tab_id) = self.active_tab_id else {
            return;
        };
        let Some(active_session) = self.active_session() else {
            return;
        };
        let is_local = active_session.is_local();
        let profile = (!is_local)
            .then(|| {
                self.profiles
                    .iter()
                    .find(|profile| profile.id == active_session.profile_id)
                    .cloned()
            })
            .flatten();
        if (!is_local && (profile.is_none() || self.credential_lookup_task.is_some()))
            || profile.as_ref().is_some_and(|profile| {
                self.credential_mutations_in_progress
                    .contains_key(&profile.id)
            })
            || !self
                .tab(tab_id)
                .is_some_and(|tab| tab.layout.contains(active_pane_id))
        {
            return;
        }

        let session_id = if is_local {
            self.create_local_session()
        } else {
            self.create_session_for_profile(
                &profile
                    .as_ref()
                    .expect("remote split should retain its profile")
                    .id,
            )
        };
        let pane_id = self.create_terminal_pane(tab_id, session_id, window, cx);
        let split = self
            .tab_mut(tab_id)
            .expect("validated tab should remain present")
            .layout
            .split(active_pane_id, pane_id, axis);
        debug_assert!(split, "validated active pane should be splittable");

        self.activate_session(session_id, cx);
        if let Some(profile) = profile {
            self.connect_profile_in_session(session_id, profile, window, cx);
        } else {
            self.start_local_terminal(session_id, cx);
        }
        if let Some(focus_handle) = self.pane(pane_id).map(|pane| pane.focus_handle.clone()) {
            focus_handle.focus(window);
        }
        cx.notify();
    }

    fn close_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_tab_id else {
            return;
        };
        if self
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab_id)
            .count()
            <= 1
        {
            return;
        }
        let Some(pane_id) = self.active_pane_id else {
            return;
        };
        let Some(session_id) = self.pane(pane_id).map(|pane| pane.session_id) else {
            return;
        };
        if self.block_close_for_unsaved_file(session_id, cx) {
            if let Some(tab) = self.tab_mut(tab_id) {
                tab.view = TerminalTabView::Files;
            }
            return;
        }

        if self.remove_pane(pane_id, cx) {
            self.close_session(session_id, cx);
            if let Some(focus_handle) = self.active_pane().map(|pane| pane.focus_handle.clone()) {
                focus_handle.focus(window);
            }
        }
        cx.notify();
    }

    fn connect_profile_in_session(
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

    #[allow(dead_code)]
    fn lookup_credential_and_connect(
        &mut self,
        session_id: SessionId,
        profile: ConnectionProfile,
        prompt_kind: CredentialPromptKind,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let profile_id = profile.id.clone();
        let credential_kind = prompt_kind.credential_kind();
        let runtime = cx.global::<SshRuntime>().handle();
        let checking_message = self.tr("credential-checking");
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(checking_message);
        }
        self.credential_lookup_session_id = Some(session_id);

        self.credential_lookup_task = Some(cx.spawn_in(window, async move |this, cx| {
            let lookup_profile_id = profile_id.clone();
            let result = runtime
                .spawn_blocking(move || load_credential(&lookup_profile_id, credential_kind))
                .await;

            let _ = this.update_in(cx, |this, window, cx| {
                this.credential_lookup_task = None;
                this.credential_lookup_session_id = None;
                if !this
                    .session(session_id)
                    .is_some_and(|session| session.connection_state.can_connect())
                    || !this.profiles.iter().any(|candidate| candidate == &profile)
                    || this
                        .credential_mutations_in_progress
                        .contains_key(&profile_id)
                {
                    if let Some(session) = this.session_mut(session_id) {
                        session.connection_message = None;
                        cx.notify();
                    }
                    return;
                }

                let loaded = match result {
                    Ok(result) => result.map_err(|error| error.to_string()),
                    Err(error) => Err(format!(
                        "Failed to access the system keychain task: {error}"
                    )),
                };

                match loaded {
                    Ok(Some(secret)) => {
                        let auth = auth_method_with_secret(prompt_kind, secret);
                        let credential =
                            ConnectionCredential::from_keychain(profile_id, credential_kind);
                        if this.activate_session_in_window(session_id, window, cx) {
                            this.start_connection(session_id, profile, auth, Some(credential), cx);
                        }
                    }
                    Ok(None) => match prompt_kind {
                        CredentialPromptKind::Password => {
                            this.open_credential_prompt(
                                session_id,
                                profile_id,
                                CredentialPromptKind::Password,
                                None,
                                cx,
                            );
                        }
                        CredentialPromptKind::PrivateKeyPassphrase { path } => {
                            let auth = AuthMethod::PrivateKey {
                                path,
                                passphrase: None,
                            };
                            if this.activate_session_in_window(session_id, window, cx) {
                                this.start_connection(session_id, profile, auth, None, cx);
                            }
                        }
                        CredentialPromptKind::ProxyPassword => {
                            this.open_credential_prompt(
                                session_id,
                                profile_id,
                                CredentialPromptKind::ProxyPassword,
                                None,
                                cx,
                            );
                        }
                    },
                    Err(error) => match prompt_kind {
                        CredentialPromptKind::Password => {
                            this.open_credential_prompt(
                                session_id,
                                profile_id,
                                CredentialPromptKind::Password,
                                Some(error),
                                cx,
                            );
                        }
                        CredentialPromptKind::PrivateKeyPassphrase { path } => {
                            let auth = AuthMethod::PrivateKey {
                                path,
                                passphrase: None,
                            };
                            if this.activate_session_in_window(session_id, window, cx) {
                                this.start_connection(session_id, profile, auth, None, cx);
                                if let Some(session) = this.session_mut(session_id) {
                                    session.connection_message = Some(error);
                                }
                            }
                        }
                        CredentialPromptKind::ProxyPassword => {
                            this.open_credential_prompt(
                                session_id,
                                profile_id,
                                CredentialPromptKind::ProxyPassword,
                                Some(error),
                                cx,
                            );
                        }
                    },
                }
            });
        }));

        cx.notify();
    }

    #[allow(dead_code)]
    fn start_connection(
        &mut self,
        session_id: SessionId,
        profile: ConnectionProfile,
        auth: AuthMethod,
        credential: Option<ConnectionCredential>,
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
        let connection = SshConnection::spawn_with_transfer_rate_limiter(
            &runtime,
            profile.clone(),
            auth,
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
        session.connection_credentials = credential.into_iter().collect();
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

    fn begin_connection_preparation(
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
                let message = format!("{}: {jump_id}", self.tr("connection-missing-jump"));
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(message);
                }
                cx.notify();
                return;
            };
            // Referenced profiles contribute only their endpoint and authentication.
            jump.route = ConnectionRoute::default();
            steps.push(jump);
        }
        steps.push(target_profile.clone());
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
        let checking_message = self.tr("credential-checking");
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(checking_message);
        }
        self.continue_connection_preparation(cx);
    }

    fn continue_connection_preparation(&mut self, cx: &mut Context<Self>) {
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
                let message = localized_connection_error(&error, &self.localizer);
                if let Some(session) = self.session_mut(pending.session_id) {
                    session.connection_error = Some(message);
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

    fn lookup_preparation_credential(
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
        let checking_message = self.tr("credential-checking");
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some(checking_message);
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

    fn handle_missing_preparation_secret(
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
                let message = error.unwrap_or_else(|| self.tr("proxy-command-keychain-missing"));
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

    fn accept_preparation_secret(
        &mut self,
        profile_id: String,
        credential_kind: CredentialKind,
        prompt_kind: Option<CredentialPromptKind>,
        secret: SecretString,
        credential: ConnectionCredential,
        cx: &mut Context<Self>,
    ) {
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

    fn accept_preparation_auth(
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

    fn finish_connection_preparation(
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
                let approval_message = self.tr("proxy-command-approval-title");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(approval_message);
                }
                cx.notify();
            }
            Ok(_) => self.start_connection_plan(session_id, target_profile, plan, credentials, cx),
            Err(error) => {
                let message = localized_connection_error(&error, &self.localizer);
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(message);
                    session.connection_message = None;
                }
                cx.notify();
            }
        }
    }

    fn approve_proxy_command(&mut self, cx: &mut Context<Self>) {
        let Some(mut prompt) = self.proxy_command_approval_prompt.take() else {
            return;
        };
        self.pending_proxy_approval.remove(&prompt.session_id);
        if let Err(error) = prompt
            .plan
            .approve_proxy_command(prompt.approval_digest.clone())
        {
            let message = localized_connection_error(&error, &self.localizer);
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(message);
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
            let message = self.tr("profile-validation-proxy-command");
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(message);
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
            let message = format!("{}: {error}", self.tr("app-save-profiles-failed"));
            if let Some(session) = self.session_mut(prompt.session_id) {
                session.connection_error = Some(message);
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

    fn cancel_proxy_command_approval(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.proxy_command_approval_prompt.take() else {
            return;
        };
        self.pending_proxy_approval.remove(&prompt.session_id);
        let cancelled_message = self.tr("proxy-command-approval-cancelled");
        if let Some(session) = self.session_mut(prompt.session_id) {
            session.connection_error = Some(cancelled_message);
            session.connection_message = None;
        }
        cx.notify();
    }

    fn start_connection_plan(
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

    fn submit_credential_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let message = self.tr("connection-preparation-expired");
            if let Some(session) = self.session_mut(session_id) {
                session.connection_error = Some(message);
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
        self.pending_connection = None;
        self.dismiss_credential_prompt(cx);
        cx.notify();
    }

    fn trust_pending_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        let handle_missing = self.tr("connection-handle-missing");
        let Some((info, handle)) = self.session_mut(session_id).and_then(|session| {
            let info = session.host_key_prompt.take()?;
            match session.connection_handle.clone() {
                Some(handle) => Some((info, handle)),
                None => {
                    session.connection_error = Some(handle_missing);
                    None
                }
            }
        }) else {
            cx.notify();
            return;
        };

        match handle.trust_host_key() {
            Ok(()) => {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("address", info.address());
                let message = self.tr_with("connection-trusting-host", &args);
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message);
                }
            }
            Err(error) => {
                let message = localized_connection_error(&error, &self.localizer);
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(message);
                }
            }
        }
        cx.notify();
    }

    fn reject_pending_host_key(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_mut() else {
            return;
        };
        if session.host_key_prompt.take().is_none() {
            return;
        }

        if let Some(handle) = session.connection_handle.as_ref()
            && let Err(error) = handle.reject_host_key()
        {
            session.connection_error = Some(error.to_string());
        }
        session.connection_message = None;
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

    fn on_cancel_settings_selector(
        &mut self,
        _: &CancelSettingsSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_settings_selector(cx);
    }

    fn prompt_for_private_key_passphrase(
        &mut self,
        session_id: SessionId,
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
                AuthConfig::None | AuthConfig::Password | AuthConfig::Agent => None,
            })
        else {
            return false;
        };

        self.activate_session(session_id, cx);
        self.open_credential_prompt(
            session_id,
            profile_id,
            CredentialPromptKind::PrivateKeyPassphrase { path },
            Some(error),
            cx,
        );
        true
    }

    fn prompt_for_password(
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

    fn retry_connection_with_prompt(
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
            session.connection_message = Some(error.clone());
        }
        self.begin_connection_preparation(
            session_id,
            target_profile,
            Some((profile_id, kind, Some(error))),
            cx,
        );
        true
    }

    fn remove_rejected_credential_then_prompt(
        &mut self,
        session_id: SessionId,
        profile_id: String,
        kind: CredentialKind,
        authentication_error: String,
        cx: &mut Context<Self>,
    ) {
        let runtime = cx.global::<SshRuntime>().handle();
        let removing_message = self.tr("credential-removing-rejected");
        if let Some(session) = self.session_mut(session_id) {
            session.connection_message = Some(removing_message);
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
                    session.connection_error = Some(error);
                }
                cx.notify();
            });
        }));
    }

    fn save_successful_credentials(
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
                    let saved_message = this.tr("credential-saved");
                    if let Some(session) = this.session_mut(session_id) {
                        session.connection_message = Some(match result {
                            Ok(Ok(())) => saved_message,
                            Ok(Err(error)) => error.to_string(),
                            Err(error) => error.to_string(),
                        });
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn disconnect_active_connection(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        self.disconnect_session(session_id, cx);
    }

    fn disconnect_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let disconnected_message = self.tr("terminal-session-disconnected");
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
                session.connection_error = Some(error);
                session.close_when_disconnected
            } else {
                // Disable repeated clicks before the worker publishes its event.
                session.connection_state = SessionState::Disconnecting;
                session.terminal_end_reason = Some(disconnected_message);
                false
            }
        };

        if should_remove {
            self.remove_session(session_id, cx);
        }

        cx.notify();
    }

    fn terminal_modes(&self, session_id: SessionId) -> TerminalModes {
        self.session(session_id)
            .and_then(|session| session.terminal.as_ref())
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

    fn terminal_point_for_position(
        &self,
        session_id: SessionId,
        position: gpui::Point<Pixels>,
    ) -> Option<TerminalPoint> {
        let terminal = self.session(session_id)?.terminal.as_ref()?;
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
        pane_id: PaneId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.set_active_pane(pane_id, cx) {
            return;
        }
        let Some(focus_handle) = self.pane(pane_id).map(|pane| pane.focus_handle.clone()) else {
            return;
        };
        let Some(session_id) = self.pane(pane_id).map(|pane| pane.session_id) else {
            return;
        };
        self.focused_terminal_session_id = Some(session_id);
        focus_handle.focus(window);

        self.begin_terminal_selection(session_id, event, cx);
    }

    fn on_quick_terminal_mouse_down(
        &mut self,
        session_id: SessionId,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_terminal_session_id = Some(session_id);
        self.quick_terminal_focus_handle.focus(window);
        self.begin_terminal_selection(session_id, event, cx);
    }

    fn open_profile_context_menu(
        &mut self,
        profile_id: String,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.sftp_context_menu = None;
        self.terminal_context_menu = None;
        self.profile_context_menu = Some(ProfileContextMenu {
            profile_id,
            position: event.position,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn open_terminal_context_menu(
        &mut self,
        session_id: SessionId,
        pane_id: Option<PaneId>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane_id) = pane_id {
            if !self.set_active_pane(pane_id, cx) {
                return;
            }
            let Some(focus_handle) = self.pane(pane_id).map(|pane| pane.focus_handle.clone())
            else {
                return;
            };
            focus_handle.focus(window);
        } else {
            self.focused_terminal_session_id = Some(session_id);
            self.quick_terminal_focus_handle.focus(window);
        }

        self.sftp_context_menu = None;
        self.terminal_context_menu = Some(TerminalContextMenu {
            session_id,
            position: event.position,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn begin_terminal_selection(
        &mut self,
        session_id: SessionId,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(point) = self.terminal_point_for_position(session_id, event.position) else {
            return;
        };

        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if event.modifiers.shift
            && let Some(selection) = session.terminal_selection.as_mut()
        {
            selection.head = point;
        } else {
            session.terminal_selection = Some(TerminalSelection::new(point, point));
        }

        session.terminal_selecting = true;
        cx.stop_propagation();
        cx.notify();
    }

    fn on_terminal_mouse_move(
        &mut self,
        session_id: SessionId,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .session(session_id)
            .is_some_and(|session| session.terminal_selecting)
            || !event.dragging()
        {
            return;
        }

        let Some(point) = self.terminal_point_for_position(session_id, event.position) else {
            return;
        };
        let Some(selection) = self
            .session_mut(session_id)
            .and_then(|session| session.terminal_selection.as_mut())
        else {
            return;
        };

        if selection.head != point {
            selection.head = point;
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_terminal_mouse_up(
        &mut self,
        session_id: SessionId,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.terminal_selecting = false;
        if session
            .terminal_selection
            .is_some_and(TerminalSelection::is_empty)
        {
            session.terminal_selection = None;
            cx.notify();
        }
    }

    fn copy_terminal_selection(&self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session(session_id) else {
            return false;
        };
        let Some(selection) = session
            .terminal_selection
            .filter(|selection| !selection.is_empty())
        else {
            return false;
        };
        let Some(terminal) = session.terminal.as_ref() else {
            return false;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(
            terminal.snapshot().selected_text(selection),
        ));
        true
    }

    fn clear_terminal_selection(&mut self, session_id: SessionId) -> bool {
        let Some(session) = self.session_mut(session_id) else {
            return false;
        };
        let had_selection = session.terminal_selection.take().is_some();
        let was_selecting = std::mem::take(&mut session.terminal_selecting);
        had_selection || was_selecting
    }

    fn select_all_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.session_mut(session_id) else {
            return false;
        };
        let Some(terminal) = session.terminal.as_ref() else {
            return false;
        };
        let size = terminal.engine.size();
        let Some(selection) = full_terminal_selection(size.rows(), size.columns()) else {
            return false;
        };

        session.terminal_selection = Some(selection);
        session.terminal_selecting = false;
        cx.notify();
        true
    }

    fn reset_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let Some(terminal) = session.terminal.as_mut() else {
            return;
        };

        terminal.reset();
        session.terminal_marked_text.clear();
        session.terminal_selection = None;
        session.terminal_selecting = false;
        session.terminal_scroll_accumulator = 0.0;
        self.terminal_context_menu = None;
        cx.notify();
    }

    fn on_terminal_key_down(
        &mut self,
        session_id: SessionId,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_terminal_session_id = Some(session_id);
        if is_terminal_copy_shortcut(&event.keystroke)
            && self.copy_terminal_selection(session_id, cx)
        {
            cx.stop_propagation();
            return;
        }

        if is_terminal_paste_shortcut(&event.keystroke) {
            self.paste_into_terminal(session_id, cx);
            cx.stop_propagation();
            return;
        }

        if let Some(bytes) = encode_key(&event.keystroke, self.terminal_modes(session_id)) {
            self.send_terminal_user_input(session_id, bytes, cx);
            cx.stop_propagation();
        }
    }

    fn paste_into_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };

        let bytes = encode_paste(&text, self.terminal_modes(session_id));
        self.send_terminal_user_input(session_id, bytes, cx);
    }

    fn send_terminal_input(
        &mut self,
        session_id: SessionId,
        data: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if data.is_empty() || session.connection_state != SessionState::Connected {
            return;
        }

        if let Err(error) = session.write_terminal_input(data) {
            session.connection_error = Some(error);
            cx.notify();
        }
    }

    fn send_terminal_user_input(
        &mut self,
        session_id: SessionId,
        data: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        if data.is_empty() {
            return;
        }

        let selection_cleared = self.clear_terminal_selection(session_id);

        if let Some(terminal) = self
            .session_mut(session_id)
            .and_then(|session| session.terminal.as_mut())
            && terminal.engine.display_offset() != 0
        {
            terminal.scroll(TerminalScroll::Bottom);
            cx.notify();
        }

        if selection_cleared {
            cx.notify();
        }

        self.send_terminal_input(session_id, data, cx);
    }

    fn apply_terminal_layout(
        &mut self,
        session_id: SessionId,
        bounds: Bounds<Pixels>,
        layout: TerminalLayout,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self
            .session_mut(session_id)
            .and_then(|session| session.terminal.as_mut())
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

        self.schedule_terminal_resize(session_id, layout.pty_size, cx);
        cx.notify();
    }

    fn schedule_terminal_resize(
        &mut self,
        session_id: SessionId,
        size: PtySize,
        cx: &mut Context<Self>,
    ) {
        // Keep local reflow and the remote PTY on the same final size after live resizing settles.
        let task = cx.spawn(async move |this, cx| {
            Timer::after(TERMINAL_RESIZE_DEBOUNCE).await;

            let _ = this.update(cx, |this, cx| {
                let Some(session) = this.session_mut(session_id) else {
                    return;
                };
                let is_current_size = session
                    .terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.pending_pty_size == Some(size));
                if !is_current_size {
                    return;
                }

                if let Err(error) = session.resize_terminal(size) {
                    session.connection_error = Some(error);
                    cx.notify();
                }
            });
        });
        if let Some(session) = self.session_mut(session_id) {
            session.terminal_resize_task = Some(task);
        }
    }

    fn on_terminal_scroll(
        &mut self,
        pane_id: PaneId,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.set_active_pane(pane_id, cx) {
            return;
        }
        if let Some(focus_handle) = self.pane(pane_id).map(|pane| pane.focus_handle.clone()) {
            focus_handle.focus(window);
        }
        let Some(session_id) = self.pane(pane_id).map(|pane| pane.session_id) else {
            return;
        };
        self.focused_terminal_session_id = Some(session_id);
        self.scroll_terminal_session(session_id, event, cx);
    }

    fn on_quick_terminal_scroll(
        &mut self,
        session_id: SessionId,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focused_terminal_session_id = Some(session_id);
        self.quick_terminal_focus_handle.focus(window);
        self.scroll_terminal_session(session_id, event, cx);
    }

    fn scroll_terminal_session(
        &mut self,
        session_id: SessionId,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let line_height = self
            .session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .map(|terminal| terminal.cell_height)
            .unwrap_or(f32::from(TERMINAL_CELL_HEIGHT));
        let delta = f32::from(event.delta.pixel_delta(px(line_height)).y);
        if delta == 0.0 {
            return;
        }
        cx.stop_propagation();

        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if session.terminal_scroll_accumulator.signum() != delta.signum() {
            session.terminal_scroll_accumulator = 0.0;
        }
        session.terminal_scroll_accumulator += delta;

        let lines = (session.terminal_scroll_accumulator / line_height).trunc() as i32;
        if lines == 0 {
            return;
        }
        session.terminal_scroll_accumulator -= lines as f32 * line_height;

        let modes = self.terminal_modes(session_id);
        let display_offset = self
            .session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .map(|terminal| terminal.engine.display_offset())
            .unwrap_or_default();
        let alternate_scroll = should_translate_alternate_scroll(modes, display_offset);

        if alternate_scroll {
            self.clear_terminal_selection(session_id);
            self.send_terminal_input(session_id, encode_alternate_scroll(lines, modes), cx);
        } else if let Some(session) = self.session_mut(session_id) {
            session.terminal_selection = None;
            session.terminal_selecting = false;
            let Some(terminal) = session.terminal.as_mut() else {
                return;
            };
            terminal.scroll(TerminalScroll::Lines(lines));
            cx.notify();
        }
    }

    fn process_terminal_output(
        &mut self,
        session_id: SessionId,
        data: &[u8],
        cx: &mut Context<Self>,
    ) -> bool {
        let events = {
            let Some(session) = self.session_mut(session_id) else {
                return false;
            };
            if !data.is_empty()
                && session
                    .terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.engine.display_offset() == 0)
            {
                session.terminal_selection = None;
                session.terminal_selecting = false;
            }

            session
                .terminal
                .as_mut()
                .map(|terminal| terminal.process(data))
                .unwrap_or_default()
        };

        let mut ui_state_changed = false;
        for event in events {
            ui_state_changed |= self.handle_terminal_event(session_id, event, cx);
        }
        ui_state_changed
    }

    fn terminal_session_is_rendered(&self, session_id: SessionId) -> bool {
        if self.bottom_panel_open && self.quick_terminal_session_id == Some(session_id) {
            return true;
        }
        if self.active_panel != ActivePanel::Connection
            || self.active_tab_view() != TerminalTabView::Terminal
        {
            return false;
        }
        let Some(active_tab_id) = self.active_tab_id else {
            return false;
        };

        self.panes
            .iter()
            .any(|pane| pane.tab_id == active_tab_id && pane.session_id == session_id)
    }

    fn schedule_terminal_redraw(&mut self, cx: &mut Context<Self>) {
        if self.terminal_redraw_task.is_some() {
            return;
        }

        self.terminal_redraw_task = Some(cx.spawn(async move |this, cx| {
            Timer::after(TERMINAL_REDRAW_INTERVAL).await;
            let _ = this.update(cx, |this, cx| {
                this.terminal_redraw_task = None;
                cx.notify();
            });
        }));
    }

    fn handle_terminal_event(
        &mut self,
        session_id: SessionId,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            TerminalEvent::TitleChanged(title) => {
                if let Some(terminal) = self
                    .session_mut(session_id)
                    .and_then(|session| session.terminal.as_mut())
                {
                    terminal.title = title;
                }
                true
            }
            TerminalEvent::WorkingDirectoryChanged(path) => {
                if let Some(terminal) = self
                    .session_mut(session_id)
                    .and_then(|session| session.terminal.as_mut())
                {
                    terminal.remote_cwd = Some(path);
                }
                if self.active_session_id == Some(session_id) {
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
            TerminalEvent::ClipboardStore {
                clipboard,
                contents,
            } => {
                self.write_terminal_clipboard(clipboard, contents, cx);
                false
            }
            TerminalEvent::ClipboardLoad(request) => {
                let contents = self
                    .read_terminal_clipboard(request.clipboard, cx)
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.send_terminal_response(session_id, request.response(&contents));
                false
            }
            TerminalEvent::ColorRequest(request) => {
                let palette = self.terminal_palette();
                let color = self
                    .session(session_id)
                    .and_then(|session| session.terminal.as_ref())
                    .map(|terminal| {
                        palette_color(&terminal.snapshot(), request.index, palette).into()
                    });
                if let Some(color) = color {
                    self.send_terminal_response(session_id, request.response(color));
                }
                false
            }
            TerminalEvent::WriteToPty(data) => {
                self.send_terminal_response(session_id, data);
                false
            }
            TerminalEvent::TextAreaSizeRequest(request) => {
                let size = self
                    .session(session_id)
                    .and_then(|session| session.terminal.as_ref())
                    .map(ActiveTerminal::text_area_size);
                if let Some(size) = size {
                    self.send_terminal_response(session_id, request.response(size));
                }
                false
            }
            TerminalEvent::Bell => {
                let message = self.tr("terminal-remote-bell");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message);
                }
                true
            }
            TerminalEvent::ExitRequested => {
                let message = self.tr("terminal-remote-exit-requested");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            TerminalEvent::ChildExited(status) => {
                let message = status.map_or_else(
                    || self.tr("terminal-remote-exited"),
                    |status| {
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("status", status);
                        self.tr_with("terminal-remote-exited-status", &args)
                    },
                );
                if let Some(session) = self.session_mut(session_id) {
                    session.terminal_end_reason = Some(message);
                    session
                        .connection_message
                        .clone_from(&session.terminal_end_reason);
                }
                true
            }
            TerminalEvent::MouseCursorDirty
            | TerminalEvent::CursorBlinkingChanged
            | TerminalEvent::Wakeup => false,
        }
    }

    fn send_terminal_response(&mut self, session_id: SessionId, data: Vec<u8>) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if let Err(error) = session.write_terminal_input(data) {
            session.connection_error = Some(error);
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

    fn handle_connection_event(
        &mut self,
        session_id: SessionId,
        event: ConnectionEvent,
        cx: &mut Context<Self>,
    ) {
        if self.session(session_id).is_none() {
            return;
        }

        let should_notify = match event {
            ConnectionEvent::StateChanged(state) => {
                let connection_closed = self.tr("sftp-connection-closed");
                let disconnected = self.tr("terminal-session-disconnected");
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
                            session.terminal_end_reason = Some(disconnected);
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
                    ConnectionStage::Proxy => self.tr("connection-connecting-proxy"),
                    ConnectionStage::Jump { index, total, .. } => {
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("index", index);
                        args.set("total", total);
                        self.tr_with("connection-connecting-jump", &args)
                    }
                    ConnectionStage::Target { .. } => self.tr("connection-connecting-target"),
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
                let stage_label = connection_stage_label(&stage, &self.localizer);
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(format!("{stage_label}: {}", info.address()));
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

                if !prompted_for_credential {
                    let message = localized_connection_error(&error, &self.localizer);
                    if let Some(session) = self.session_mut(session_id) {
                        session.connection_error = Some(message);
                    }
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
            ConnectionEvent::SftpAvailabilityChanged { available, message } => {
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
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("status", status);
                let message = self.tr_with("terminal-remote-shell-status", &args);
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
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("signal", signal);
                args.set(
                    "core",
                    if core_dumped {
                        self.tr("terminal-core-dumped")
                    } else {
                        String::new()
                    },
                );
                args.set("message", message);
                let message = self.tr_with("terminal-remote-shell-signal", &args);
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Eof) => {
                let message = self.tr("terminal-remote-shell-eof");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    if session.terminal_end_reason.is_none() {
                        session.terminal_end_reason = Some(message);
                    }
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Closed) => {
                let message = self.tr("terminal-remote-shell-closed");
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

    fn handle_local_terminal_event(
        &mut self,
        session_id: SessionId,
        event: LocalTerminalEvent,
        cx: &mut Context<Self>,
    ) {
        if self.session(session_id).is_none() {
            return;
        }

        let should_notify = match event {
            LocalTerminalEvent::Started => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_state = SessionState::Connected;
                    session.connection_message = None;
                    if let Some(terminal) = session.terminal.as_mut() {
                        terminal.was_connected = true;
                    }
                }
                true
            }
            LocalTerminalEvent::Output(data) => {
                let ui_state_changed = self.process_terminal_output(session_id, &data, cx);
                if ui_state_changed || self.terminal_session_is_rendered(session_id) {
                    self.schedule_terminal_redraw(cx);
                }
                false
            }
            LocalTerminalEvent::Resized(size) => {
                let size = ssh_pty_size(size);
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
            LocalTerminalEvent::Exited { exit_code, signal } => {
                let message = signal.map_or_else(
                    || {
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("status", exit_code);
                        self.tr_with("terminal-local-shell-status", &args)
                    },
                    |signal| {
                        let mut args = fluent_bundle::FluentArgs::new();
                        args.set("signal", signal);
                        self.tr_with("terminal-local-shell-signal", &args)
                    },
                );
                let should_remove = {
                    let session = self
                        .session_mut(session_id)
                        .expect("checked local session should still exist");
                    session.connection_state = SessionState::Disconnected;
                    session.local_terminal_handle = None;
                    session.terminal_resize_task = None;
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                    session.close_when_disconnected
                };
                if should_remove {
                    self.remove_session(session_id, cx);
                }
                true
            }
            LocalTerminalEvent::Failed(error) => {
                let should_remove = {
                    let session = self
                        .session_mut(session_id)
                        .expect("checked local session should still exist");
                    session.connection_state = SessionState::Failed;
                    session.local_terminal_handle = None;
                    session.terminal_resize_task = None;
                    session.connection_error = Some(error.to_string());
                    session.close_when_disconnected
                };
                if should_remove {
                    self.remove_session(session_id, cx);
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
        self.ensure_quick_command_prompt(cx);
        self.sync_default_quick_command_targets();
        let selected_profile = self.selected_profile().cloned();
        let right_sidebar_width = self.effective_right_sidebar_width(window);
        let sidebar_width = self.effective_sidebar_width(window);
        let should_focus_terminal = self.active_panel == ActivePanel::Connection
            && self.active_tab_view() == TerminalTabView::Terminal
            && !self.right_sidebar_open
            && !self.bottom_panel_open
            && self
                .active_session()
                .is_some_and(TerminalSession::is_terminal_visible);

        let mut root = div()
            .id("remcmd_root")
            .relative()
            .flex()
            .size_full()
            .text_color(self.theme.text_primary)
            .on_mouse_move(cx.listener(Self::resize_sidebar))
            .on_mouse_move(cx.listener(Self::resize_right_sidebar))
            .on_mouse_move(cx.listener(Self::resize_bottom_panel))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_sidebar_resize))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(Self::finish_right_sidebar_resize),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(Self::finish_bottom_panel_resize),
            );

        let left_sidebar_open = self.left_sidebar_open;
        let left_transition_id = self.left_sidebar_transition_id;
        let left_start_width = if left_transition_id == 0 || left_sidebar_open {
            0.0
        } else {
            sidebar_width
        };
        let left_end_width = if left_sidebar_open {
            sidebar_width
        } else {
            0.0
        };
        root = root.child(
            div()
                .flex()
                .flex_none()
                .h_full()
                .overflow_hidden()
                .child(self.render_sidebar(sidebar_width, cx))
                .with_animation(
                    SharedString::from(format!(
                        "left-sidebar-layout-{left_transition_id}-{left_sidebar_open}"
                    )),
                    Animation::new(if left_transition_id == 0 {
                        MOTION_INSTANT_DURATION
                    } else {
                        MOTION_STANDARD_DURATION
                    })
                    .with_easing(ease_in_out),
                    move |this, delta| {
                        this.w(px(
                            left_start_width + (left_end_width - left_start_width) * delta
                        ))
                    },
                ),
        );
        root = root.child(self.render_detail_panel(selected_profile, cx));
        let right_sidebar_open = self.right_sidebar_open;
        let right_transition_id = self.right_sidebar_transition_id;
        let right_start_width = if right_transition_id == 0 || right_sidebar_open {
            0.0
        } else {
            right_sidebar_width
        };
        let right_end_width = if right_sidebar_open {
            right_sidebar_width
        } else {
            0.0
        };
        let mut right_sidebar = div().flex().flex_none().h_full().overflow_hidden();
        if self.right_sidebar_rendered {
            right_sidebar = right_sidebar.child(self.render_right_sidebar(right_sidebar_width, cx));
        }
        root = root.child(
            right_sidebar.with_animation(
                SharedString::from(format!(
                    "right-sidebar-layout-{right_transition_id}-{right_sidebar_open}"
                )),
                Animation::new(if right_transition_id == 0 {
                    MOTION_INSTANT_DURATION
                } else {
                    MOTION_STANDARD_DURATION
                })
                .with_easing(ease_in_out),
                move |this, delta| {
                    this.w(px(
                        right_start_width + (right_end_width - right_start_width) * delta
                    ))
                },
            ),
        );
        root = root.child(self.render_titlebar_tabs(window, cx));
        if self.right_sidebar_rendered {
            root = root.child(self.render_right_sidebar_titlebar(right_sidebar_width, cx));
        }
        if cfg!(target_os = "windows") {
            root = root.child(self.render_windows_chrome(window, cx));
        }
        root = root.child(self.render_sidebar_resize_handle(sidebar_width, cx));
        if self.right_sidebar_rendered {
            root = root.child(self.render_right_sidebar_resize_handle(right_sidebar_width, cx));
        }
        if cfg!(target_os = "windows") && self.windows_menu_open.is_some() {
            root = root
                .child(
                    div()
                        .id("windows_menu_dismiss_layer")
                        .absolute()
                        .top(px(platform_chrome_height()))
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(self.theme.transparent)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.windows_menu_open = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.windows_menu_open = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(deferred(self.render_windows_menu_popup(cx)).with_priority(30));
        }
        if self.active_panel == ActivePanel::Settings && self.open_settings_selector.is_some() {
            root = root.child(
                div()
                    .id("settings_selector_dismiss_layer")
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .bg(self.theme.transparent)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.dismiss_settings_selector(cx);
                        }),
                    ),
            );
        }
        if self.profile_context_menu.is_some() {
            root = root
                .child(
                    div()
                        .id("profile_context_menu_dismiss_layer")
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(self.theme.transparent)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.profile_context_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.profile_context_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(deferred(self.render_profile_context_menu(window, cx)).with_priority(20));
        }
        if self.sftp_context_menu.is_some() {
            root = root
                .child(
                    div()
                        .id("sftp_context_menu_dismiss_layer")
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(self.theme.transparent)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.sftp_context_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.sftp_context_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(deferred(self.render_sftp_context_menu(window, cx)).with_priority(20));
        }
        if self.terminal_context_menu.is_some() {
            root = root
                .child(
                    div()
                        .id("terminal_context_menu_dismiss_layer")
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(self.theme.transparent)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.terminal_context_menu = None;
                                cx.notify();
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _, _, cx| {
                                this.terminal_context_menu = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(deferred(self.render_terminal_context_menu(window, cx)).with_priority(20));
        }

        if self.proxy_command_approval_prompt.is_some() {
            root = root.child(self.render_proxy_command_approval_prompt(cx));
        } else if self
            .active_session()
            .is_some_and(|session| session.host_key_prompt.is_some())
        {
            root = root.child(self.render_host_key_prompt(cx));
        } else if let Some(prompt) = self.credential_prompt.as_ref() {
            let focus_handle = prompt.input.focus_handle(cx);
            if !focus_handle.is_focused(window) {
                window.focus(&focus_handle);
            }

            root = root.child(self.render_credential_prompt(cx));
        } else if let Some(prompt) = self.sftp_create_prompt.as_ref() {
            let focus_handle = prompt.input.focus_handle(cx);
            if !focus_handle.is_focused(window) {
                window.focus(&focus_handle);
            }
            root = root.child(self.render_sftp_create_prompt(cx));
        } else if self.editor.is_some() {
            root = root.child(self.render_profile_editor_overlay(cx));
            if self.profile_auth_selector_open {
                root = root.child(
                    div()
                        .id("profile_auth_selector_dismiss_layer")
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .left_0()
                        .bg(self.theme.transparent)
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.profile_auth_selector_open = false;
                                cx.notify();
                            }),
                        ),
                );
            }
        } else if should_focus_terminal
            && let Some(focus_handle) = self.active_pane().map(|pane| pane.focus_handle.clone())
            && !focus_handle.is_focused(window)
        {
            window.focus(&focus_handle);
        }

        root
    }
}

impl RemCmdApp {
    fn render_titlebar_drag_area(&self) -> gpui::Div {
        div().h_full().window_control_area(WindowControlArea::Drag)
    }

    fn render_windows_chrome(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_maximized = window.is_maximized();
        let drag_area = self
            .render_titlebar_drag_area()
            .id("windows_chrome_drag_area")
            .flex_1();

        div()
            .id("windows_chrome")
            .absolute()
            .top(px(-1.0))
            .left(px(-1.0))
            .right(px(-1.0))
            .h(px(WINDOWS_CHROME_HEIGHT + 1.0))
            .flex()
            .items_center()
            .overflow_hidden()
            .rounded_tl(px(10.0))
            .rounded_tr(px(10.0))
            .bg(self.theme.sidebar_bg)
            .occlude()
            .child(
                self.render_titlebar_drag_area()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_start()
                    .w(px(WINDOWS_BRAND_WIDTH))
                    .h_full()
                    .pl(px(12.0))
                    .child(wordmark(self.theme, 90.0, 20.0)),
            )
            .child(self.render_windows_menu_button(
                WindowsMenu::File,
                self.tr("menu-file").into(),
                cx,
            ))
            .child(self.render_windows_menu_button(
                WindowsMenu::Edit,
                self.tr("menu-edit").into(),
                cx,
            ))
            .child(self.render_windows_menu_button(
                WindowsMenu::Terminal,
                self.tr("menu-terminal").into(),
                cx,
            ))
            .child(self.render_windows_menu_button(
                WindowsMenu::View,
                self.tr("menu-view").into(),
                cx,
            ))
            .child(self.render_windows_menu_button(
                WindowsMenu::Window,
                self.tr("menu-window").into(),
                cx,
            ))
            .child(self.render_windows_menu_button(
                WindowsMenu::Help,
                self.tr("menu-help").into(),
                cx,
            ))
            .child(drag_area)
            .child(self.render_windows_titlebar_controls(is_maximized, cx))
    }

    fn render_windows_menu_button(
        &self,
        menu: WindowsMenu,
        label: SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.windows_menu_open == Some(menu);
        let hover = self.theme.control_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let width = windows_menu_button_width(menu);

        div()
            .id(SharedString::from(format!("windows-menu-{menu:?}")))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(width))
            .h(px(24.0))
            .rounded_md()
            .text_size(px(12.0))
            .text_color(self.theme.text_primary)
            .bg(if selected {
                self.theme.control_pressed_bg
            } else {
                self.theme.transparent
            })
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .active(move |this| this.bg(pressed))
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if *hovered
                    && this.windows_menu_open.is_some()
                    && this.windows_menu_open != Some(menu)
                {
                    this.windows_menu_open = Some(menu);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.windows_menu_open = if this.windows_menu_open == Some(menu) {
                    None
                } else {
                    Some(menu)
                };
                cx.notify();
            }))
            .child(label)
    }

    fn render_windows_menu_popup(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu) = self.windows_menu_open else {
            return div().into_any_element();
        };
        let entries = windows_menu_entries(menu);
        let left = windows_menu_left(menu);
        let width = windows_menu_popup_width(&entries, &self.localizer);
        let mut popup = self
            .glass_floating_surface()
            .id("windows_menu_popup")
            .absolute()
            .left(px(left))
            .top(px(platform_chrome_height() - 1.0))
            .w(px(width))
            .flex()
            .flex_col()
            .p_1()
            .occlude();

        for (index, entry) in entries.into_iter().enumerate() {
            popup = match entry {
                WindowsMenuEntry::Item {
                    label,
                    shortcut,
                    command,
                } => popup.child(self.render_windows_menu_item(
                    SharedString::from(format!("windows-menu-entry-{menu:?}-{index}")),
                    windows_menu_label(&self.localizer, label).into(),
                    shortcut,
                    command,
                    cx,
                )),
                WindowsMenuEntry::Separator => popup.child(self.render_context_menu_separator()),
            };
        }

        popup.into_any_element()
    }

    fn render_windows_menu_item(
        &self,
        id: SharedString,
        label: SharedString,
        shortcut: &'static str,
        command: WindowsMenuCommand,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let enabled = self.windows_menu_command_enabled(command);
        let hover_group = SharedString::from(format!("{id}-hover"));
        let foreground = self.theme.text_primary;
        let muted = self.theme.text_muted;
        let on_accent = self.theme.on_accent;
        let hover = self.theme.accent;
        let pressed = self.theme.accent_hover;
        let row = div()
            .id(id)
            .relative()
            .flex()
            .items_center()
            .h(px(28.0))
            .px_2()
            .rounded_md()
            .text_size(px(12.0))
            .when(enabled, |this| {
                this.group(hover_group.clone())
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .active(move |this| this.bg(pressed))
            })
            .when(!enabled, |this| this.opacity(0.42))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .px_2()
                    .when(enabled, |this| {
                        this.group_hover(hover_group.clone(), |style| style.opacity(0.0))
                    })
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(foreground)
                            .child(label.clone()),
                    )
                    .when(!shortcut.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .whitespace_nowrap()
                                .text_color(muted)
                                .child(shortcut),
                        )
                    }),
            )
            .when(enabled, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .px_2()
                        .opacity(0.0)
                        .text_color(on_accent)
                        .group_hover(hover_group, |style| style.opacity(1.0))
                        .justify_between()
                        .gap_4()
                        .child(div().flex_none().whitespace_nowrap().child(label))
                        .when(!shortcut.is_empty(), |this| {
                            this.child(div().flex_none().whitespace_nowrap().child(shortcut))
                        }),
                )
            });

        if enabled {
            row.on_click(cx.listener(move |this, _, window, cx| {
                this.execute_windows_menu_command(command, window, cx);
            }))
        } else {
            row
        }
    }

    fn windows_menu_command_enabled(&self, command: WindowsMenuCommand) -> bool {
        match command {
            WindowsMenuCommand::NewRemoteTerminal | WindowsMenuCommand::ConnectSelectedProfile => {
                self.selected_profile().is_some()
            }
            WindowsMenuCommand::DisconnectActiveSession => self
                .active_session()
                .is_some_and(|session| session.connection_state.can_disconnect()),
            WindowsMenuCommand::SplitHorizontal
            | WindowsMenuCommand::SplitVertical
            | WindowsMenuCommand::ShowTerminalView
            | WindowsMenuCommand::ShowFilesView
            | WindowsMenuCommand::ResetActiveTerminal
            | WindowsMenuCommand::CloseActivePane
            | WindowsMenuCommand::CloseActiveTab => self.active_session().is_some(),
            _ => true,
        }
    }

    fn execute_windows_menu_command(
        &mut self,
        command: WindowsMenuCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.windows_menu_open = None;
        match command {
            WindowsMenuCommand::NewConnection => self.open_new_profile_editor(cx),
            WindowsMenuCommand::NewLocalTerminal => self.open_local_terminal(window, cx),
            WindowsMenuCommand::NewRemoteTerminal => {
                self.connect_selected_profile_in_new_session(window, cx);
            }
            WindowsMenuCommand::ConnectSelectedProfile => {
                self.connect_selected_profile(window, cx);
            }
            WindowsMenuCommand::DisconnectActiveSession => {
                self.disconnect_active_connection(cx);
            }
            WindowsMenuCommand::Edit(command) => {
                self.dispatch_edit_command(command, window, cx);
            }
            WindowsMenuCommand::SplitHorizontal => {
                self.split_active_pane(SplitAxis::Horizontal, window, cx);
            }
            WindowsMenuCommand::SplitVertical => {
                self.split_active_pane(SplitAxis::Vertical, window, cx);
            }
            WindowsMenuCommand::ShowTerminalView => {
                self.set_active_tab_view(TerminalTabView::Terminal, window, cx);
            }
            WindowsMenuCommand::ShowFilesView => {
                self.set_active_tab_view(TerminalTabView::Files, window, cx);
            }
            WindowsMenuCommand::ResetActiveTerminal => {
                if let Some(session_id) = self.active_session_id {
                    self.reset_terminal(session_id, cx);
                }
            }
            WindowsMenuCommand::CloseActivePane => self.close_active_pane(window, cx),
            WindowsMenuCommand::CloseActiveTab => {
                if let Some(tab_id) = self.active_tab_id {
                    self.close_tab(tab_id, cx);
                }
            }
            WindowsMenuCommand::ShowHome => self.show_home(window, cx),
            WindowsMenuCommand::ToggleLeftSidebar => self.toggle_left_sidebar(cx),
            WindowsMenuCommand::ToggleConnectionSearch => {
                self.toggle_sidebar_search(window, cx);
            }
            WindowsMenuCommand::ShowSftpSidebar => {
                self.set_right_sidebar_view(RightSidebarView::Sftp, cx);
                if !self.right_sidebar_open {
                    self.toggle_right_sidebar(cx);
                }
            }
            WindowsMenuCommand::ShowPerformanceSidebar => {
                self.set_right_sidebar_view(RightSidebarView::Performance, cx);
                if !self.right_sidebar_open {
                    self.toggle_right_sidebar(cx);
                }
            }
            WindowsMenuCommand::ToggleBottomPanel => self.toggle_bottom_panel(window, cx),
            WindowsMenuCommand::MinimizeWindow => window.minimize_window(),
            WindowsMenuCommand::ZoomWindow => window.zoom_window(),
            WindowsMenuCommand::ToggleFullscreen => window.toggle_fullscreen(),
            WindowsMenuCommand::CloseWindow => window.remove_window(),
            WindowsMenuCommand::ShowSettings => self.show_settings(window, cx),
            WindowsMenuCommand::ShowAbout => self.show_about(cx),
            WindowsMenuCommand::Quit => cx.quit(),
        }
        cx.notify();
    }

    fn dispatch_edit_command(
        &mut self,
        command: EditCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session_id) = self.focused_terminal_session_id {
            match command {
                EditCommand::Copy => {
                    self.copy_terminal_selection(session_id, cx);
                    return;
                }
                EditCommand::Paste => {
                    self.paste_into_terminal(session_id, cx);
                    return;
                }
                EditCommand::SelectAll => {
                    self.select_all_terminal(session_id, cx);
                    return;
                }
                EditCommand::Undo | EditCommand::Redo | EditCommand::Cut => return,
            }
        }

        match command {
            EditCommand::Undo => {
                window.dispatch_action(Box::new(file_editor::Undo), cx);
            }
            EditCommand::Redo => {
                window.dispatch_action(Box::new(file_editor::Redo), cx);
            }
            EditCommand::Cut => {
                window.dispatch_action(Box::new(text_field::Cut), cx);
                window.dispatch_action(Box::new(file_editor::Cut), cx);
            }
            EditCommand::Copy => {
                window.dispatch_action(Box::new(text_field::Copy), cx);
                window.dispatch_action(Box::new(file_editor::Copy), cx);
            }
            EditCommand::Paste => {
                window.dispatch_action(Box::new(text_field::Paste), cx);
                window.dispatch_action(Box::new(file_editor::Paste), cx);
            }
            EditCommand::SelectAll => {
                window.dispatch_action(Box::new(text_field::SelectAll), cx);
                window.dispatch_action(Box::new(file_editor::SelectAll), cx);
            }
        }
    }

    fn render_icon_button(
        &self,
        id: impl Into<gpui::ElementId>,
        icon_name: IconName,
        tooltip: impl Into<SharedString>,
        tone: IconTone,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let tooltip = tooltip.into();
        icon_button(
            id,
            icon(icon_name, theme, tone, 18.0),
            tone,
            enabled,
            &theme,
        )
        .tooltip(move |_, cx| -> AnyView {
            cx.new(|_| CommandTooltip {
                label: tooltip.clone(),
                theme,
            })
            .into()
        })
    }

    fn render_sidebar_icon(&self, icon_name: IconName, size: f32) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .child(icon(icon_name, self.theme, IconTone::Default, size))
    }

    fn render_titlebar_close_symbol(&self) -> AnyElement {
        #[cfg(target_os = "macos")]
        if let Some(symbol) = macos_symbols::close_circle(self.theme.panel_bg.l < 0.5) {
            return img(symbol)
                .size(px(TITLEBAR_CLOSE_SYMBOL_SIZE))
                .into_any_element();
        }

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(TITLEBAR_CLOSE_SYMBOL_SIZE))
            .rounded_full()
            .bg(self.theme.text_primary)
            .child(icon_with_color(IconName::Cancel, self.theme.panel_bg, 7.0))
            .into_any_element()
    }

    fn render_titlebar_sidebar_symbol(&self, left: bool) -> AnyElement {
        icon(
            if left {
                IconName::SidebarLeft
            } else {
                IconName::SidebarRight
            },
            self.theme,
            IconTone::Default,
            TITLEBAR_SIDEBAR_ICON_SIZE,
        )
    }

    fn render_titlebar_sidebar_button(
        &self,
        id: &'static str,
        left: bool,
        tooltip: impl Into<SharedString>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let tooltip = tooltip.into();
        icon_button(
            id,
            self.render_titlebar_sidebar_symbol(left),
            IconTone::Default,
            true,
            &theme,
        )
        .size(px(TITLEBAR_CONTROL_HOVER_SIZE))
        .rounded_full()
        .tooltip(move |_, cx| -> AnyView {
            cx.new(|_| CommandTooltip {
                label: tooltip.clone(),
                theme,
            })
            .into()
        })
    }

    fn render_windows_titlebar_controls(
        &self,
        is_maximized: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let hover = self.theme.control_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let glyph = self.theme.text_muted;

        let minimize = div()
            .id("minimize_window")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(WINDOWS_TITLEBAR_BUTTON_WIDTH))
            .h_full()
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .active(move |this| this.bg(pressed))
            .child(div().w(px(10.0)).h(px(1.0)).bg(glyph))
            .on_click(cx.listener(|_, _, window, _| window.minimize_window()));
        let maximize_symbol = if is_maximized {
            div()
                .relative()
                .size(px(12.0))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .size(px(8.0))
                        .border_1()
                        .border_color(glyph)
                        .rounded(px(1.0)),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .size(px(8.0))
                        .border_1()
                        .border_color(glyph)
                        .rounded(px(1.0)),
                )
        } else {
            div()
                .size(px(10.0))
                .border_1()
                .border_color(glyph)
                .rounded(px(1.0))
        };
        let maximize_tooltip = if is_maximized {
            self.tr("common-restore")
        } else {
            self.tr("common-maximize")
        };
        let tooltip_theme = self.theme;
        let maximize = div()
            .id("maximize_window")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(WINDOWS_TITLEBAR_BUTTON_WIDTH))
            .h_full()
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .active(move |this| this.bg(pressed))
            .child(maximize_symbol)
            .tooltip(move |_, cx| -> AnyView {
                cx.new(|_| CommandTooltip {
                    label: maximize_tooltip.clone().into(),
                    theme: tooltip_theme,
                })
                .into()
            })
            .on_click(cx.listener(|_, _, window, _| windows_chrome::toggle_maximize(window)));
        let close_hover = self.theme.danger;
        let close_pressed = self.theme.danger_hover;
        let close = div()
            .id("close_window")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(WINDOWS_TITLEBAR_BUTTON_WIDTH))
            .h_full()
            .cursor_pointer()
            .hover(move |this| this.bg(close_hover))
            .active(move |this| this.bg(close_pressed))
            .child(icon_with_color(IconName::Cancel, glyph, 11.0))
            .on_click(cx.listener(|_, _, window, _| window.remove_window()));

        div()
            .flex()
            .flex_none()
            .items_center()
            .w(px(WINDOWS_TITLEBAR_CONTROLS_WIDTH))
            .h_full()
            .child(minimize)
            .child(maximize)
            .child(close)
    }

    fn render_titlebar_action_group(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let local_active = self.active_session().is_some_and(TerminalSession::is_local);
        let can_create_terminal = self.active_panel == ActivePanel::Connection
            && (local_active || self.selected_profile().is_some())
            && self.credential_lookup_task.is_none()
            && self
                .selected_profile_id
                .as_deref()
                .is_none_or(|profile_id| {
                    !self
                        .credential_mutations_in_progress
                        .contains_key(profile_id)
                });
        let theme = self.theme;
        let new_terminal_tooltip = self.tr("sidebar-new-terminal");
        let mut new_terminal = icon_button(
            "new-titlebar-terminal",
            icon(
                IconName::Add,
                theme,
                IconTone::Default,
                TITLEBAR_ADD_ICON_SIZE,
            ),
            IconTone::Default,
            can_create_terminal,
            &theme,
        )
        .size(px(TITLEBAR_CONTROL_HOVER_SIZE))
        .rounded_full()
        .tooltip(move |_, cx| -> AnyView {
            cx.new(|_| CommandTooltip {
                label: new_terminal_tooltip.clone().into(),
                theme,
            })
            .into()
        });
        if can_create_terminal {
            new_terminal = new_terminal.on_click(cx.listener(|this, _, window, cx| {
                this.open_terminal_for_current_target(window, cx);
            }));
        }

        let right_sidebar = self
            .render_titlebar_sidebar_button(
                "toggle_right_sidebar",
                false,
                self.tr("terminal-toggle-right-sidebar"),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_right_sidebar(cx)));
        let bottom_panel_enabled = self.active_panel == ActivePanel::Connection;
        let bottom_panel_tooltip = self.tr("terminal-toggle-bottom-panel");
        let mut bottom_panel = icon_button(
            "toggle_bottom_panel",
            icon(
                IconName::PanelBottom,
                theme,
                IconTone::Default,
                TITLEBAR_SIDEBAR_ICON_SIZE,
            ),
            IconTone::Default,
            bottom_panel_enabled,
            &theme,
        )
        .size(px(TITLEBAR_CONTROL_HOVER_SIZE))
        .rounded_full()
        .when(self.bottom_panel_open, |this| {
            this.bg(self.theme.control_pressed_bg)
        })
        .tooltip(move |_, cx| -> AnyView {
            cx.new(|_| CommandTooltip {
                label: bottom_panel_tooltip.clone().into(),
                theme,
            })
            .into()
        });
        if bottom_panel_enabled {
            bottom_panel = bottom_panel.on_click(cx.listener(|this, _, window, cx| {
                this.toggle_bottom_panel(window, cx);
            }));
        }

        div()
            .id("titlebar_action_group")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.0))
            .h(px(TITLEBAR_TAB_GROUP_HEIGHT))
            .rounded_full()
            .border_1()
            .border_color(self.theme.titlebar_add_border)
            .bg(self.theme.titlebar_tab_selected_bg)
            .shadow(self.titlebar_control_shadow())
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(TITLEBAR_TAB_GROUP_HEIGHT))
                    .child(new_terminal),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(TITLEBAR_TAB_GROUP_HEIGHT))
                    .child(bottom_panel),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(TITLEBAR_TAB_GROUP_HEIGHT))
                    .child(right_sidebar),
            )
    }

    fn titlebar_control_shadow(&self) -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: self.theme.titlebar_add_shadow,
                offset: point(px(0.0), px(0.5)),
                blur_radius: px(1.0),
                spread_radius: px(-0.25),
            },
            BoxShadow {
                color: self.theme.titlebar_add_shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(3.0),
                spread_radius: px(-1.5),
            },
        ]
    }

    fn render_right_sidebar_titlebar(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let titlebar_width = width;
        let open = self.right_sidebar_open;
        let start_width = if open { 0.0 } else { titlebar_width };
        let end_width = if open { titlebar_width } else { 0.0 };
        let mut tabs = div()
            .id("right_sidebar_titlebar_tabs")
            .relative()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .max_w(px(280.0))
            .items_center()
            .h(px(TITLEBAR_TAB_GROUP_HEIGHT))
            .p(px(3.0))
            .rounded_full()
            .border_1()
            .border_color(self.theme.titlebar_tab_group_border)
            .bg(self.theme.transparent)
            .shadow(vec![BoxShadow {
                color: self.theme.titlebar_tab_group_shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(8.0),
                spread_radius: px(-4.0),
            }]);

        for (view, label, icon_name) in [
            (
                RightSidebarView::Sftp,
                self.tr("sftp-title"),
                IconName::Folder,
            ),
            (
                RightSidebarView::Performance,
                self.tr("performance-title"),
                IconName::Performance,
            ),
        ] {
            let selected = self.right_sidebar_view == view;
            let hover = if selected {
                self.theme.titlebar_tab_selected_hover_bg
            } else {
                self.theme.titlebar_tab_hover_bg
            };
            let pressed = self.theme.titlebar_tab_pressed_bg;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("right-sidebar-tab-{label}")))
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .h(px(TITLEBAR_TAB_HEIGHT))
                    .px_2()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .active(move |this| this.bg(pressed))
                    .when(selected, |this| {
                        this.border_1()
                            .border_color(self.theme.titlebar_tab_border)
                            .bg(self.theme.titlebar_tab_selected_bg)
                            .shadow(vec![BoxShadow {
                                color: self.theme.titlebar_tab_shadow,
                                offset: point(px(0.0), px(1.0)),
                                blur_radius: px(3.0),
                                spread_radius: px(-1.0),
                            }])
                    })
                    .child(icon(icon_name, self.theme, IconTone::Default, 14.0))
                    .child(div().min_w(px(0.0)).truncate().text_sm().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_right_sidebar_view(view, cx);
                    })),
            );
        }

        div()
            .id("right_sidebar_titlebar")
            .absolute()
            .top(px(platform_chrome_height() - 1.0))
            .right_0()
            .flex()
            .items_center()
            .justify_center()
            .h(px(TITLEBAR_HEIGHT))
            .px_3()
            .overflow_hidden()
            .child(tabs)
            .with_animation(
                SharedString::from(format!(
                    "right-sidebar-titlebar-{}",
                    self.right_sidebar_transition_id
                )),
                Animation::new(if self.right_sidebar_transition_id == 0 {
                    MOTION_INSTANT_DURATION
                } else {
                    MOTION_STANDARD_DURATION
                })
                .with_easing(ease_in_out),
                move |this, delta| {
                    let progress = if open { delta } else { 1.0 - delta };
                    this.w(px(start_width + (end_width - start_width) * delta))
                        .opacity(progress)
                },
            )
    }

    fn terminal_tab_title(&self, tab: &TerminalTab) -> String {
        let terminal_number = self
            .tabs
            .iter()
            .take_while(|candidate| candidate.id != tab.id)
            .filter(|candidate| candidate.profile_id == tab.profile_id)
            .count()
            + 1;
        let active_session = self
            .pane(tab.active_pane_id)
            .and_then(|pane| self.session(pane.session_id));
        let profile_id = active_session
            .map(|session| session.profile_id.as_str())
            .unwrap_or(&tab.profile_id);
        let server_name = if profile_id == LOCAL_PROFILE_ID {
            self.tr("terminal-local")
        } else {
            self.profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .map(|profile| profile.name.clone())
                .unwrap_or_else(|| self.tr("terminal-server"))
        };
        let sftp_path = active_session
            .filter(|session| session.sftp.loaded)
            .map(|session| session.sftp.display_path());
        let remote_cwd = active_session
            .and_then(|session| session.terminal.as_ref())
            .and_then(|terminal| terminal.remote_cwd.as_deref());

        workspace_tab_title(
            &server_name,
            tab.view,
            terminal_number,
            sftp_path,
            remote_cwd,
            &self.localizer,
        )
    }

    fn animate_titlebar_right_edge(
        &self,
        titlebar: gpui::Stateful<gpui::Div>,
        expanded_width: f32,
    ) -> impl IntoElement {
        let transition_id = self.right_sidebar_transition_id;
        let open = self.right_sidebar_open;
        let start_width = if transition_id == 0 || open {
            0.0
        } else {
            expanded_width
        };
        let end_width = if open { expanded_width } else { 0.0 };

        titlebar.child(
            div().flex_none().h_full().with_animation(
                SharedString::from(format!("titlebar-right-spacer-{transition_id}-{open}")),
                Animation::new(if transition_id == 0 {
                    MOTION_INSTANT_DURATION
                } else {
                    MOTION_STANDARD_DURATION
                })
                .with_easing(ease_in_out),
                move |this, delta| this.w(px(start_width + (end_width - start_width) * delta)),
            ),
        )
    }

    fn render_titlebar_tabs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let leading_width = self.titlebar_leading_width(window);
        let expanded_right_sidebar_width = self.effective_right_sidebar_width(window);
        let expanded_right_titlebar_width = expanded_right_sidebar_width;
        let titlebar_right_inset = if self.right_sidebar_open {
            expanded_right_titlebar_width
        } else {
            0.0
        };
        let left_sidebar_button = self
            .render_titlebar_sidebar_button(
                "toggle_left_sidebar",
                true,
                self.tr("terminal-toggle-left-sidebar"),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_left_sidebar(cx)));
        let left_sidebar_group = div()
            .id("titlebar_left_sidebar_group")
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(TITLEBAR_TAB_GROUP_HEIGHT))
            .rounded_full()
            .border_1()
            .border_color(self.theme.titlebar_add_border)
            .bg(self.theme.titlebar_tab_selected_bg)
            .shadow(self.titlebar_control_shadow())
            .overflow_hidden()
            .child(left_sidebar_button);
        let leading_transition_id = self.left_sidebar_transition_id;
        let leading_open = self.left_sidebar_open;
        let expanded_sidebar_width = self.effective_sidebar_width(window);
        let open_button_offset =
            (expanded_sidebar_width - TITLEBAR_TAB_GROUP_HEIGHT - TITLEBAR_LEFT_CONTROL_EDGE_GAP)
                .max(TITLEBAR_EDGE_INSET);
        let closed_button_offset = TITLEBAR_EDGE_INSET;
        let button_start_offset = if leading_transition_id == 0 {
            if leading_open {
                open_button_offset
            } else {
                closed_button_offset
            }
        } else if leading_open {
            closed_button_offset
        } else {
            open_button_offset
        };
        let button_end_offset = if leading_open {
            open_button_offset
        } else {
            closed_button_offset
        };
        let leading_start_width = if leading_transition_id == 0 || leading_open {
            COLLAPSED_TITLEBAR_LEADING_WIDTH
        } else {
            expanded_sidebar_width
        };
        let leading_end_width = leading_width;
        let leading = div()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .when(cfg!(target_os = "windows") && !leading_open, |this| {
                this.bg(self.theme.panel_bg)
            })
            .child(
                div().flex_none().h_full().with_animation(
                    SharedString::from(format!(
                        "titlebar-left-button-offset-{leading_transition_id}-{leading_open}"
                    )),
                    Animation::new(if leading_transition_id == 0 {
                        MOTION_INSTANT_DURATION
                    } else {
                        MOTION_STANDARD_DURATION
                    })
                    .with_easing(ease_in_out),
                    move |this, delta| {
                        this.w(px(
                            button_start_offset + (button_end_offset - button_start_offset) * delta
                        ))
                    },
                ),
            )
            .child(left_sidebar_group)
            .child(self.render_titlebar_drag_area().flex_1())
            .with_animation(
                SharedString::from(format!(
                    "titlebar-left-edge-{leading_transition_id}-{leading_open}"
                )),
                Animation::new(if leading_transition_id == 0 {
                    MOTION_INSTANT_DURATION
                } else {
                    MOTION_STANDARD_DURATION
                })
                .with_easing(ease_in_out),
                move |this, delta| {
                    this.w(px(
                        leading_start_width + (leading_end_width - leading_start_width) * delta
                    ))
                },
            );
        let titlebar = div()
            .id("window_titlebar")
            .absolute()
            .top(px(platform_chrome_height() - 1.0))
            .left_0()
            .right_0()
            .h(px(TITLEBAR_HEIGHT))
            .flex()
            .items_center()
            .when(cfg!(target_os = "windows"), |this| {
                this.bg(self.theme.sidebar_bg)
            })
            .child(leading);

        if self.tab_layout == TabLayout::Vertical {
            let center = div()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .items_center()
                .when(cfg!(target_os = "windows"), |this| {
                    this.bg(self.theme.panel_bg)
                        .when(self.left_sidebar_open, |this| this.rounded_tl(px(10.0)))
                        .when(self.right_sidebar_rendered, |this| {
                            this.rounded_tr(px(10.0))
                        })
                })
                .child(self.render_titlebar_drag_area().flex_1())
                .child(self.render_titlebar_action_group(cx))
                .child(self.render_titlebar_drag_area().w(px(TITLEBAR_EDGE_INSET)));
            let titlebar = titlebar.child(center);
            return self.animate_titlebar_right_edge(titlebar, expanded_right_titlebar_width);
        }

        let tab_labels = self
            .tabs
            .iter()
            .map(|tab| self.terminal_tab_title(tab))
            .collect::<Vec<_>>();
        let selected_tab_min_width = tab_labels
            .iter()
            .map(|label| estimated_titlebar_label_width(label) + 68.0)
            .fold(TITLEBAR_TAB_ICON_ONLY_WIDTH, f32::max);
        let track_width = (f32::from(window.viewport_size().width)
            - leading_width
            - titlebar_right_inset
            - 24.0
            - 8.0
            - TITLEBAR_ACTION_GROUP_WIDTH)
            .max(0.0);
        let tab_count = self.tabs.len();
        let inactive_count = self.tabs.len().saturating_sub(1);
        let separator_width = self.tabs.len().saturating_sub(1) as f32;
        let inactive_width = if inactive_count == 0 {
            track_width
        } else {
            ((track_width - 6.0 - separator_width - selected_tab_min_width).max(0.0)
                / inactive_count as f32)
                .max(TITLEBAR_TAB_ICON_ONLY_WIDTH)
        };
        let hide_inactive_labels = inactive_width < TITLEBAR_TAB_ELLIPSIS_MIN_WIDTH;
        let selected_tab_basis =
            titlebar_active_tab_basis(track_width, tab_count, selected_tab_min_width);
        let mut tabs = div()
            .id("titlebar_terminal_tabs")
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .h(px(TITLEBAR_TAB_GROUP_HEIGHT))
            .items_center()
            .p(px(3.0))
            .rounded_full()
            .border_1()
            .border_color(self.theme.titlebar_tab_group_border)
            .bg(self.theme.transparent)
            .shadow(vec![BoxShadow {
                color: self.theme.titlebar_tab_group_shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(8.0),
                spread_radius: px(-4.0),
            }])
            .overflow_x_scroll()
            .track_scroll(&self.titlebar_tabs_scroll_handle);
        let close_terminal_tooltip = self.tr("common-close-terminal");

        for (tab_index, (tab, label)) in self.tabs.iter().zip(tab_labels).enumerate() {
            let close_terminal_tooltip = close_terminal_tooltip.clone();
            let tab_id = tab.id;
            let is_active =
                self.active_panel == ActivePanel::Connection && self.active_tab_id == Some(tab_id);
            let is_deactivating = self.active_panel == ActivePanel::Connection
                && self.previous_active_tab_id == Some(tab_id)
                && self.active_tab_id != Some(tab_id);
            let is_hovered = self.hovered_titlebar_tab_id == Some(tab_id);
            let is_close_hovered = self.hovered_titlebar_close_id == Some(tab_id);
            let show_close = is_active || is_hovered;
            let icon_only = !is_active && hide_inactive_labels;
            let hover_background = if is_active {
                self.theme.titlebar_tab_selected_hover_bg
            } else {
                self.theme.titlebar_tab_hover_bg
            };
            let status = self
                .pane(tab.active_pane_id)
                .and_then(|pane| self.session(pane.session_id))
                .map(|session| session.connection_state)
                .unwrap_or(SessionState::Disconnected);
            let status_color = match status {
                SessionState::Connected => self.theme.status_ok,
                SessionState::Failed => self.theme.error_text,
                SessionState::Connecting
                | SessionState::Authenticating
                | SessionState::Disconnecting => self.theme.status_warn,
                SessionState::Disconnected => self.theme.text_faint,
            };
            let pressed_background = self.theme.titlebar_tab_pressed_bg;
            let selected_background = self.theme.titlebar_tab_selected_bg;
            let tab_border = self.theme.titlebar_tab_border;
            let tab_shadow = self.theme.titlebar_tab_shadow;
            let (start_tab_basis, end_tab_basis, start_tab_min_width, end_tab_min_width) =
                if is_active {
                    (
                        0.0,
                        selected_tab_basis,
                        TITLEBAR_TAB_ICON_ONLY_WIDTH,
                        selected_tab_min_width,
                    )
                } else if is_deactivating {
                    (
                        selected_tab_basis,
                        0.0,
                        selected_tab_min_width,
                        TITLEBAR_TAB_ICON_ONLY_WIDTH,
                    )
                } else {
                    (
                        0.0,
                        0.0,
                        TITLEBAR_TAB_ICON_ONLY_WIDTH,
                        TITLEBAR_TAB_ICON_ONLY_WIDTH,
                    )
                };

            if tab_index > 0 {
                let previous_is_active = self.active_panel == ActivePanel::Connection
                    && self.active_tab_id == Some(self.tabs[tab_index - 1].id);
                let separator = if is_active || previous_is_active {
                    self.theme.transparent
                } else {
                    self.theme.titlebar_tab_separator
                };
                tabs = tabs.child(div().flex_none().w(px(1.0)).h(px(18.0)).bg(separator));
            }

            let terminal_icon = div()
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(20.0))
                .child(icon(
                    IconName::Terminal,
                    self.theme,
                    IconTone::Default,
                    15.0,
                ))
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .size(px(5.0))
                        .rounded_full()
                        .bg(status_color),
                )
                .with_animation(
                    SharedString::from(format!("titlebar-tab-terminal-{}-{show_close}", tab_id.0)),
                    Animation::new(MOTION_FAST_DURATION).with_easing(ease_out_quint()),
                    move |this, delta| this.opacity(if show_close { 1.0 - delta } else { delta }),
                );
            let tab_content = if is_active {
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .px(px(34.0))
                    .text_center()
                    .text_sm()
                    .whitespace_nowrap()
                    .child(label)
            } else if icon_only {
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .child(terminal_icon)
            } else {
                div()
                    .flex()
                    .w_full()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .child(terminal_icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_sm()
                            .child(label.clone()),
                    )
            };
            let content_start_opacity = if is_active {
                0.5
            } else if is_deactivating {
                0.68
            } else {
                1.0
            };
            let content_animation_id = if is_active || is_deactivating {
                format!(
                    "titlebar-tab-content-{}-{}",
                    tab_id.0, self.titlebar_tab_transition_id
                )
            } else {
                format!("titlebar-tab-content-{}-stable", tab_id.0)
            };
            let tab_content = tab_content.with_animation(
                SharedString::from(content_animation_id),
                Animation::new(MOTION_EMPHASIZED_DURATION).with_easing(ease_in_out),
                move |this, delta| {
                    this.opacity(content_start_opacity + (1.0 - content_start_opacity) * delta)
                },
            );

            let tooltip_theme = self.theme;
            let close_hover_background = self.theme.titlebar_tab_hover_bg;
            let close_pressed_background = self.theme.titlebar_tab_pressed_bg;
            let mut close_control = div()
                .id(SharedString::from(format!(
                    "close-titlebar-tab-{}",
                    tab_id.0
                )))
                .absolute()
                .top(px(6.0))
                .left(px(9.0))
                .flex()
                .items_center()
                .justify_center()
                .size(px(18.0))
                .rounded_full()
                .bg(if is_close_hovered {
                    close_hover_background
                } else {
                    self.theme.transparent
                })
                .cursor_pointer()
                .hover(move |this| this.bg(close_hover_background))
                .active(move |this| this.bg(close_pressed_background))
                .child(self.render_titlebar_close_symbol())
                .tooltip(move |_, cx| -> AnyView {
                    cx.new(|_| CommandTooltip {
                        label: close_terminal_tooltip.clone().into(),
                        theme: tooltip_theme,
                    })
                    .into()
                })
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    let hovered_close_id = if *hovered { Some(tab_id) } else { None };
                    if this.hovered_titlebar_close_id != hovered_close_id {
                        this.hovered_titlebar_close_id = hovered_close_id;
                        cx.notify();
                    }
                }));
            if show_close {
                close_control = close_control.on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_tab(tab_id, cx);
                }));
            }
            let close_control = close_control.with_animation(
                SharedString::from(format!("titlebar-tab-close-{}-{show_close}", tab_id.0)),
                Animation::new(MOTION_FAST_DURATION).with_easing(ease_out_quint()),
                move |this, delta| this.opacity(if show_close { delta } else { 1.0 - delta }),
            );

            let tab_element = div()
                .id(SharedString::from(format!("titlebar-tab-{}", tab_id.0)))
                .relative()
                .flex()
                .w_full()
                .min_w(px(0.0))
                .items_center()
                .justify_center()
                .h(px(TITLEBAR_TAB_HEIGHT))
                .rounded_full()
                .bg(self.theme.transparent)
                .cursor_pointer()
                .hover(move |this| this.bg(hover_background))
                .active(move |this| this.bg(pressed_background))
                .when(is_active, move |this| {
                    this.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .rounded_full()
                            .border_1()
                            .border_color(tab_border)
                            .bg(selected_background)
                            .shadow(vec![BoxShadow {
                                color: tab_shadow,
                                offset: point(px(0.0), px(1.0)),
                                blur_radius: px(3.0),
                                spread_radius: px(-1.0),
                            }])
                            .with_animation(
                                SharedString::from(format!("titlebar-tab-selection-{}", tab_id.0)),
                                Animation::new(MOTION_EMPHASIZED_DURATION).with_easing(ease_in_out),
                                |this, delta| this.opacity(0.72 + 0.28 * delta),
                            ),
                    )
                })
                .child(close_control)
                .child(tab_content)
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    let hovered_tab_id = if *hovered { Some(tab_id) } else { None };
                    if this.hovered_titlebar_tab_id != hovered_tab_id {
                        this.hovered_titlebar_tab_id = hovered_tab_id;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.activate_tab_in_window(tab_id, window, cx) {
                        cx.notify();
                    }
                }));

            let tab_element = tab_element.with_animation(
                SharedString::from(format!("titlebar-tab-entry-{}", tab_id.0)),
                Animation::new(MOTION_STANDARD_DURATION).with_easing(ease_out_quint()),
                |this, delta| {
                    this.left(px((1.0 - delta) * 10.0))
                        .opacity(0.72 + 0.28 * delta)
                },
            );
            let layout_animation_id = if is_active || is_deactivating {
                format!(
                    "titlebar-tab-layout-{}-{}",
                    tab_id.0, self.titlebar_tab_transition_id
                )
            } else {
                format!("titlebar-tab-layout-{}-stable", tab_id.0)
            };
            let tab_slot = div()
                .flex()
                .flex_1()
                .min_w(px(TITLEBAR_TAB_ICON_ONLY_WIDTH))
                .h(px(TITLEBAR_TAB_HEIGHT))
                .child(tab_element)
                .with_animation(
                    SharedString::from(layout_animation_id),
                    Animation::new(MOTION_EMPHASIZED_DURATION).with_easing(ease_in_out),
                    move |this, delta| {
                        let basis = start_tab_basis + (end_tab_basis - start_tab_basis) * delta;
                        let min_width =
                            start_tab_min_width + (end_tab_min_width - start_tab_min_width) * delta;
                        this.flex_basis(px(basis)).min_w(px(min_width))
                    },
                );
            tabs = tabs.child(tab_slot);
        }

        let mut controls = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .items_center()
            .gap(px(8.0))
            .px(px(TITLEBAR_EDGE_INSET));
        if !self.tabs.is_empty() {
            if self.titlebar_tabs_scroll_active {
                let scroll_handle = self.titlebar_tabs_scroll_handle.clone();
                let scroll_start = self.titlebar_tabs_scroll_start;
                let transition_id = self.titlebar_tabs_scroll_transition_id;
                controls = controls.child(tabs.with_animation(
                    SharedString::from(format!("titlebar-tabs-scroll-{transition_id}")),
                    Animation::new(MOTION_STANDARD_DURATION).with_easing(ease_out_quint()),
                    move |this, delta| {
                        let target_x = -scroll_handle.max_offset().width;
                        scroll_handle.set_offset(point(
                            scroll_start.x + (target_x - scroll_start.x) * delta,
                            scroll_start.y,
                        ));
                        this
                    },
                ));
            } else {
                controls = controls.child(tabs);
            }
        } else {
            controls = controls.child(self.render_titlebar_drag_area().flex_1());
        }

        let center = div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .items_center()
            .when(cfg!(target_os = "windows"), |this| {
                this.bg(self.theme.panel_bg)
                    .when(self.left_sidebar_open, |this| this.rounded_tl(px(10.0)))
                    .when(self.right_sidebar_rendered, |this| {
                        this.rounded_tr(px(10.0))
                    })
            })
            .child(controls.child(self.render_titlebar_action_group(cx)));
        let titlebar = titlebar.child(center);
        self.animate_titlebar_right_edge(titlebar, expanded_right_titlebar_width)
    }

    fn has_terminal_workspace(&self, profile_id: &str) -> bool {
        self.active_tab()
            .is_some_and(|tab| tab.profile_id == profile_id)
            && self.active_pane_id.is_some()
    }

    fn terminal_has_ended(&self, profile_id: &str) -> bool {
        self.active_session()
            .filter(|session| session.profile_id == profile_id)
            .is_some_and(TerminalSession::terminal_has_ended)
    }

    fn close_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(can_disconnect) = self
            .session(session_id)
            .map(|session| session.connection_state.can_disconnect())
        else {
            return;
        };

        if can_disconnect {
            if let Some(session) = self.session_mut(session_id) {
                session.close_when_disconnected = true;
            }
            self.disconnect_session(session_id, cx);
        } else {
            self.remove_session(session_id, cx);
        }
        cx.notify();
    }

    fn close_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        let unsaved = self
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab_id)
            .find_map(|pane| {
                self.session(pane.session_id)
                    .and_then(|session| session.sftp.file.as_ref())
                    .filter(|file| file.is_dirty(cx))
                    .map(|_| (pane.id, pane.session_id))
            });
        if let Some((pane_id, session_id)) = unsaved {
            self.set_active_pane(pane_id, cx);
            if let Some(tab) = self.tab_mut(tab_id) {
                tab.view = TerminalTabView::Files;
            }
            self.block_close_for_unsaved_file(session_id, cx);
            return;
        }

        if self.hovered_titlebar_tab_id == Some(tab_id) {
            self.hovered_titlebar_tab_id = None;
        }
        if self.hovered_titlebar_close_id == Some(tab_id) {
            self.hovered_titlebar_close_id = None;
        }
        let pane_ids = self
            .panes
            .iter()
            .filter(|pane| pane.tab_id == tab_id)
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        let session_ids = pane_ids
            .iter()
            .filter_map(|pane_id| self.pane(*pane_id).map(|pane| pane.session_id))
            .collect::<Vec<_>>();

        for pane_id in pane_ids {
            self.remove_pane(pane_id, cx);
        }
        for session_id in session_ids {
            self.close_session(session_id, cx);
        }
        cx.notify();
    }

    fn block_close_for_unsaved_file(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
        let dirty = self
            .session(session_id)
            .and_then(|session| session.sftp.file.as_ref())
            .is_some_and(|file| file.is_dirty(cx));
        if dirty {
            let message = self.tr("terminal-save-before-close");
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(message);
            }
            cx.notify();
        }
        dirty
    }

    fn reconnect_session(
        &mut self,
        session_id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .session(session_id)
            .is_some_and(TerminalSession::is_local)
        {
            if self.activate_session_in_window(session_id, window, cx) {
                self.start_local_terminal(session_id, cx);
            }
            return;
        }

        let Some(profile) = self
            .session(session_id)
            .and_then(|session| {
                self.profiles
                    .iter()
                    .find(|profile| profile.id == session.profile_id)
            })
            .cloned()
        else {
            return;
        };

        if self.activate_session_in_window(session_id, window, cx) {
            self.connect_profile_in_session(session_id, profile, window, cx);
        }
    }

    fn render_pane_layout(&self, layout: &PaneLayout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            PaneLayout::Pane(pane_id) => self.render_terminal_pane(*pane_id, cx),
            PaneLayout::Split {
                axis,
                first,
                second,
            } => {
                let first = self.render_pane_layout(first, cx);
                let second = self.render_pane_layout(second, cx);
                match axis {
                    SplitAxis::Horizontal => div()
                        .flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .child(first)
                        .child(
                            div()
                                .flex_none()
                                .w(px(1.0))
                                .h_full()
                                .bg(self.theme.border_strong),
                        )
                        .child(second)
                        .into_any_element(),
                    SplitAxis::Vertical => div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .overflow_hidden()
                        .child(first)
                        .child(
                            div()
                                .flex_none()
                                .h(px(1.0))
                                .w_full()
                                .bg(self.theme.border_strong),
                        )
                        .child(second)
                        .into_any_element(),
                }
            }
        }
    }

    fn render_terminal_pane(&self, pane_id: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let Some(pane) = self.pane(pane_id) else {
            return div().into_any_element();
        };
        let session_id = pane.session_id;
        let terminal_view = self.render_terminal_session_view(
            session_id,
            pane.focus_handle.clone(),
            Some(pane_id),
            format!("terminal-view-{}", pane_id.0),
            cx,
        );
        let session = self.session(session_id);
        let mut pane_view = div()
            .id(SharedString::from(format!("terminal-pane-{}", pane_id.0)))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(120.0))
            .min_h(px(100.0))
            .overflow_hidden()
            .child(terminal_view);
        if session.is_some_and(TerminalSession::terminal_has_ended) {
            pane_view = pane_view.child(self.render_terminal_lifecycle(session_id, cx));
        }

        pane_view.into_any_element()
    }

    fn render_terminal_session_view(
        &self,
        session_id: SessionId,
        focus_handle: FocusHandle,
        pane_id: Option<PaneId>,
        element_id: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let palette = self.terminal_palette();
        let session = self.session(session_id);
        let terminal_font_size = f32::from(self.terminal_font_size);
        let terminal_line_height = terminal_font_size * TERMINAL_FONT_LINE_HEIGHT_FACTOR;
        let render_state = session.and_then(|session| {
            session.terminal.as_ref().map(|terminal| {
                let (snapshot, damage) = terminal.snapshot_for_render();
                (
                    snapshot,
                    damage,
                    session.terminal_selection,
                    terminal.canvas_cache.clone(),
                )
            })
        });
        let terminal_font_family = self.terminal_font_family.clone();
        let input_entity = cx.entity();
        let layout_entity = input_entity.clone();
        let input_focus_handle = focus_handle.clone();
        let input_layer = canvas(
            move |bounds, window, _| {
                let metrics = TerminalCellMetrics::measure(window);
                let layout = terminal_layout_for_pixels(
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                    metrics.width,
                    metrics.height,
                );
                let frame = render_state.map(|(snapshot, damage, selection, cache)| {
                    TerminalCanvasFrame::prepare(
                        TerminalCanvasInput {
                            cache: &cache,
                            snapshot: &snapshot,
                            damage,
                            selection,
                            palette,
                            font_family: terminal_font_family,
                            font_size: terminal_font_size,
                            metrics,
                        },
                        window,
                    )
                });

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
                        this.apply_terminal_layout(session_id, bounds, layout, cx);
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let terminal_view = div()
            .id(SharedString::from(element_id))
            .key_context("Terminal")
            .track_focus(&focus_handle)
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .w_full()
            .p_3()
            .overflow_hidden()
            .bg(rgb(palette.background.hex()))
            .font_family(self.terminal_font_family.clone())
            .text_size(px(terminal_font_size))
            .line_height(px(terminal_line_height))
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(move |this, event, window, cx| {
                this.on_terminal_key_down(session_id, event, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    if let Some(pane_id) = pane_id {
                        this.on_terminal_mouse_down(pane_id, event, window, cx);
                    } else {
                        this.on_quick_terminal_mouse_down(session_id, event, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event, window, cx| {
                    this.open_terminal_context_menu(session_id, pane_id, event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(move |this, event, window, cx| {
                this.on_terminal_mouse_move(session_id, event, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_terminal_mouse_up(session_id, event, window, cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_terminal_mouse_up(session_id, event, window, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(move |this, event, window, cx| {
                if let Some(pane_id) = pane_id {
                    this.on_terminal_scroll(pane_id, event, window, cx);
                } else {
                    this.on_quick_terminal_scroll(session_id, event, window, cx);
                }
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .size_full()
                    .overflow_hidden()
                    .child(input_layer),
            );

        terminal_view.into_any_element()
    }

    fn render_terminal_lifecycle(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.session(session_id);
        let (message, color) =
            if let Some(error) = session.and_then(|session| session.connection_error.as_ref()) {
                (error.clone(), self.theme.error_text)
            } else if let Some(message) =
                session.and_then(|session| session.terminal_end_reason.as_ref())
            {
                (message.clone(), self.theme.text_muted)
            } else {
                (self.tr("terminal-session-ended"), self.theme.text_muted)
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
                        self.render_icon_button(
                            SharedString::from(format!("terminal-reconnect-{}", session_id.0)),
                            IconName::Reconnect,
                            self.tr("common-reconnect"),
                            IconTone::Accent,
                            true,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.reconnect_session(session_id, window, cx);
                            },
                        )),
                    )
                    .child(
                        self.render_icon_button(
                            SharedString::from(format!("terminal-close-{}", session_id.0)),
                            IconName::Cancel,
                            self.tr("common-close-terminal"),
                            IconTone::Default,
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_session(session_id, cx);
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

    fn render_proxy_command_approval_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_host_key_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_sidebar_wordmark(&self) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(24.0))
            .ml_2()
            .child(wordmark(self.theme, 108.0, 24.0))
    }

    fn render_sidebar(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.sidebar_search.read(cx).text().trim().to_lowercase();
        let list_hover_background = self.theme.list_hover_bg;
        let pressed_background = self.theme.control_pressed_bg;
        let mut connection_tree = div()
            .id("sidebar_navigation")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(2.0))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .mt_3();

        let home_selected = self.active_panel == ActivePanel::Home;
        let home_background = if home_selected {
            self.theme.list_selected_bg
        } else {
            self.theme.transparent
        };
        let home_hover = if home_selected {
            self.theme.list_selected_hover_bg
        } else {
            self.theme.list_hover_bg
        };
        connection_tree = connection_tree
            .child(
                div()
                    .id("show_home")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .px_2()
                    .rounded_md()
                    .bg(home_background)
                    .cursor_pointer()
                    .hover(move |this| this.bg(home_hover))
                    .active(move |this| this.bg(pressed_background))
                    .child(self.render_sidebar_icon(IconName::Home, 17.0))
                    .child(self.tr("sidebar-home"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_home(window, cx);
                    })),
            )
            .child(
                div()
                    .id("open_local_terminal")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(list_hover_background))
                    .active(move |this| this.bg(pressed_background))
                    .child(self.render_sidebar_icon(IconName::Terminal, 17.0))
                    .child(self.tr("sidebar-local-terminal"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_local_terminal(window, cx);
                    })),
            )
            .when(self.tab_layout == TabLayout::Vertical, |mut this| {
                for tab in self
                    .tabs
                    .iter()
                    .filter(|tab| tab.profile_id == LOCAL_PROFILE_ID)
                {
                    this =
                        this.child(self.render_sidebar_terminal_tab(tab, pressed_background, cx));
                }
                this
            })
            .child(
                div()
                    .id("add_connection")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(list_hover_background))
                    .active(move |this| this.bg(pressed_background))
                    .child(self.render_sidebar_icon(IconName::NewConnection, 17.0))
                    .child(self.tr("sidebar-new-connection"))
                    .on_click(cx.listener(|this, _, _, cx| this.add_profile(cx))),
            );

        let section_icon = if self.connections_expanded {
            IconName::Collapse
        } else {
            IconName::Expand
        };
        connection_tree = connection_tree.child(
            div()
                .id("toggle_connections")
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.0))
                .h(px(32.0))
                .mt_2()
                .px_2()
                .rounded_md()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(self.theme.text_muted)
                .cursor_pointer()
                .hover(move |this| this.bg(list_hover_background))
                .active(move |this| this.bg(pressed_background))
                .child(self.render_sidebar_icon(section_icon, 15.0))
                .child(self.tr("sidebar-connections"))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_connections(cx))),
        );

        let mut visible_profiles = 0usize;
        if self.connections_expanded {
            for profile in &self.profiles {
                if !query.is_empty()
                    && !profile.name.to_lowercase().contains(&query)
                    && !profile.host.to_lowercase().contains(&query)
                    && !profile.address().to_lowercase().contains(&query)
                {
                    continue;
                }
                visible_profiles += 1;

                let select_profile_id = profile.id.clone();
                let new_terminal_profile_id = profile.id.clone();
                let context_profile_id = profile.id.clone();
                let can_create_terminal = self.credential_lookup_task.is_none()
                    && !self
                        .credential_mutations_in_progress
                        .contains_key(&profile.id);
                let mut new_terminal_button = self
                    .render_icon_button(
                        SharedString::from(format!("new-terminal-{}", profile.id)),
                        IconName::Add,
                        self.tr("sidebar-new-terminal"),
                        IconTone::Default,
                        can_create_terminal,
                    )
                    .size(px(24.0));
                if can_create_terminal {
                    new_terminal_button =
                        new_terminal_button.on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.select_profile(new_terminal_profile_id.clone(), window, cx);
                            this.connect_selected_profile_in_new_session(window, cx);
                        }));
                }
                let is_selected = self.active_panel == ActivePanel::Server
                    && self.selected_profile_id.as_ref() == Some(&profile.id);
                let background = if is_selected {
                    self.theme.list_selected_bg
                } else {
                    self.theme.transparent
                };
                let hover = if is_selected {
                    self.theme.list_selected_hover_bg
                } else {
                    self.theme.list_hover_bg
                };
                connection_tree = connection_tree.child(
                    div()
                        .id(SharedString::from(format!("profile-{}", profile.id)))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(10.0))
                        .h(px(34.0))
                        .pl_2()
                        .pr_1()
                        .rounded_md()
                        .bg(background)
                        .cursor_pointer()
                        .hover(move |this| this.bg(hover))
                        .active(move |this| this.bg(pressed_background))
                        .child(self.render_sidebar_icon(IconName::Server, 18.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_sm()
                                .child(profile.name.clone()),
                        )
                        .child(new_terminal_button)
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event, _, cx| {
                                this.open_profile_context_menu(
                                    context_profile_id.clone(),
                                    event,
                                    cx,
                                );
                            }),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_profile(select_profile_id.clone(), window, cx);
                        })),
                );

                if self.tab_layout == TabLayout::Vertical {
                    for tab in self.tabs.iter().filter(|tab| tab.profile_id == profile.id) {
                        connection_tree = connection_tree.child(self.render_sidebar_terminal_tab(
                            tab,
                            pressed_background,
                            cx,
                        ));
                    }
                }
            }
        }

        if self.connections_expanded && visible_profiles == 0 {
            connection_tree = connection_tree.child(
                div()
                    .ml(px(32.0))
                    .mt_2()
                    .text_sm()
                    .text_color(self.theme.text_faint)
                    .child(self.tr("sidebar-no-match")),
            );
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
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .ml(px(-12.0))
            .mt_4()
            .pt(px(10.0))
            .pb(px(10.0))
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap_1()
                    .h(px(30.0))
                    .pl_2()
                    .pr_3()
                    .child(
                        div()
                            .id("show_settings")
                            .flex()
                            .flex_1()
                            .min_w(px(0.0))
                            .items_center()
                            .justify_start()
                            .gap_2()
                            .h_full()
                            .pl_1()
                            .pr_2()
                            .rounded_md()
                            .bg(settings_background)
                            .cursor_pointer()
                            .hover(move |this| this.bg(settings_hover))
                            .active(move |this| this.bg(pressed_background))
                            .child(self.render_sidebar_icon(IconName::Settings, 17.0))
                            .child(self.tr("sidebar-settings"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_settings(window, cx);
                            })),
                    )
                    .child(
                        self.render_icon_button(
                            "show_about",
                            IconName::About,
                            self.tr("about-title"),
                            IconTone::Default,
                            true,
                        )
                        .size(px(30.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_about(cx);
                        })),
                    ),
            );

        self.glass_sidebar_surface()
            .w(px(width))
            .px_3()
            .pt(px(content_top_inset()))
            .when(!cfg!(target_os = "windows"), |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .flex_none()
                        .h(px(34.0))
                        .child(self.render_sidebar_wordmark())
                        .child(
                            self.render_icon_button(
                                "toggle_sidebar_search",
                                IconName::Search,
                                self.tr("menu-search-connections"),
                                IconTone::Default,
                                true,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.toggle_sidebar_search(window, cx);
                                },
                            )),
                        ),
                )
            })
            .when(
                cfg!(target_os = "windows") || self.sidebar_search_visible,
                |this| {
                    this.child(
                        div()
                            .flex_none()
                            .mt(px(if cfg!(target_os = "windows") {
                                6.0
                            } else {
                                8.0
                            }))
                            .child(self.sidebar_search.clone()),
                    )
                },
            )
            .child(connection_tree)
            .child(settings_footer)
    }

    fn render_sidebar_terminal_tab(
        &self,
        tab: &TerminalTab,
        pressed_background: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let tab_id = tab.id;
        let terminal_title = self.terminal_tab_title(tab);
        let is_active =
            self.active_panel == ActivePanel::Connection && self.active_tab_id == Some(tab_id);
        let background = if is_active {
            self.theme.list_selected_bg
        } else {
            self.theme.transparent
        };
        let hover = if is_active {
            self.theme.list_selected_hover_bg
        } else {
            self.theme.list_hover_bg
        };

        div()
            .id(SharedString::from(format!("sidebar-tab-{}", tab_id.0)))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(32.0))
            .ml(px(20.0))
            .pl_2()
            .pr_1()
            .rounded_md()
            .bg(background)
            .cursor_pointer()
            .hover(move |this| this.bg(hover))
            .active(move |this| this.bg(pressed_background))
            .child(self.render_sidebar_icon(IconName::Terminal, 16.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(terminal_title),
            )
            .child(
                self.render_icon_button(
                    SharedString::from(format!("close-sidebar-tab-{}", tab_id.0)),
                    IconName::Cancel,
                    self.tr("common-close-terminal"),
                    IconTone::Default,
                    true,
                )
                .size(px(24.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_tab(tab_id, cx);
                })),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.activate_tab_in_window(tab_id, window, cx) {
                    cx.notify();
                }
            }))
    }

    fn glass_sidebar_surface(&self) -> gpui::Div {
        let surface = div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .min_w(px(0.0))
            .h_full();

        if cfg!(target_os = "windows") {
            surface.child(
                div()
                    .absolute()
                    .top(px(content_top_inset() - 1.0))
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .bg(self.theme.sidebar_bg),
            )
        } else {
            surface.bg(self.theme.sidebar_bg)
        }
    }

    fn glass_floating_surface(&self) -> gpui::Div {
        div()
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.floating_glass_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }])
    }

    fn render_sidebar_resize_handle(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.theme.border_strong;
        let transition_id = self.left_sidebar_transition_id;
        let open = self.left_sidebar_open;
        let start_width = if transition_id == 0 || open {
            0.0
        } else {
            width
        };
        let end_width = if open { width } else { 0.0 };
        let resting = if self.sidebar_resize.is_some() {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };

        let handle = div()
            .id("sidebar_resize_handle")
            .absolute()
            .top(px(platform_chrome_height()))
            .bottom_0()
            .left(px(width - SIDEBAR_RESIZE_HANDLE_WIDTH / 2.0))
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .bg(self.theme.transparent)
            .child(div().w(px(1.0)).h_full().bg(resting));
        let handle = if open {
            handle
                .cursor(CursorStyle::ResizeLeftRight)
                .hover(move |this| this.bg(hover))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_sidebar_resize))
        } else {
            handle
        };

        handle.with_animation(
            SharedString::from(format!("left-sidebar-resize-handle-{transition_id}")),
            Animation::new(if transition_id == 0 {
                MOTION_INSTANT_DURATION
            } else {
                MOTION_STANDARD_DURATION
            })
            .with_easing(ease_in_out),
            move |this, delta| {
                let animated_width = start_width + (end_width - start_width) * delta;
                let progress = if open { delta } else { 1.0 - delta };
                this.left(px(animated_width - SIDEBAR_RESIZE_HANDLE_WIDTH / 2.0))
                    .opacity(progress)
            },
        )
    }

    fn render_right_sidebar_resize_handle(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = self.theme.border_strong;
        let transition_id = self.right_sidebar_transition_id;
        let open = self.right_sidebar_open;
        let start_width = if transition_id == 0 || open {
            0.0
        } else {
            width
        };
        let end_width = if open { width } else { 0.0 };
        let resting = if self.right_sidebar_resize.is_some() {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };

        let handle = div()
            .id("right_sidebar_resize_handle")
            .absolute()
            .top(px(platform_chrome_height()))
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .bg(self.theme.transparent)
            .child(div().w(px(1.0)).h_full().bg(resting));
        let handle = if open {
            handle
                .cursor(CursorStyle::ResizeLeftRight)
                .hover(move |this| this.bg(hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(Self::begin_right_sidebar_resize),
                )
        } else {
            handle
        };

        handle.with_animation(
            SharedString::from(format!("right-sidebar-resize-handle-{transition_id}")),
            Animation::new(if transition_id == 0 {
                MOTION_INSTANT_DURATION
            } else {
                MOTION_STANDARD_DURATION
            })
            .with_easing(ease_in_out),
            move |this, delta| {
                let animated_width = start_width + (end_width - start_width) * delta;
                let progress = if open { delta } else { 1.0 - delta };
                this.right(px(animated_width - SIDEBAR_RESIZE_HANDLE_WIDTH / 2.0))
                    .opacity(progress)
            },
        )
    }

    fn render_server_performance(&self, session_id: SessionId) -> AnyElement {
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
                .child(message.to_owned())
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

    fn render_right_sidebar(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(session_id) = self.active_session_id {
            match self.right_sidebar_view {
                RightSidebarView::Sftp => {
                    self.render_sftp_browser(session_id, SftpBrowserPlacement::Sidebar, cx)
                }
                RightSidebarView::Performance => self
                    .render_server_performance(session_id)
                    .into_any_element(),
            }
        } else {
            div()
                .flex()
                .flex_1()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.render_sidebar_icon(
                    match self.right_sidebar_view {
                        RightSidebarView::Sftp => IconName::Folder,
                        RightSidebarView::Performance => IconName::Performance,
                    },
                    20.0,
                ))
                .child(self.tr("terminal-no-active"))
                .into_any_element()
        };

        self.glass_sidebar_surface()
            .id("right_sidebar")
            .w(px(width))
            .pt(px(content_top_inset()))
            .px_3()
            .pb_3()
            .child(content)
    }

    fn render_bottom_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut panel = div()
            .id("bottom_panel")
            .relative()
            .flex()
            .flex_none()
            .flex_col()
            .h(px(0.0))
            .overflow_hidden();
        if !self.bottom_panel_open {
            return panel;
        }

        let mut actions = div().flex().flex_none().items_center().gap_1();
        if self.quick_terminal_session_id.is_some() {
            actions = actions.child(
                self.render_icon_button(
                    "dispose_quick_terminal",
                    IconName::Delete,
                    self.tr("terminal-close-quick"),
                    IconTone::Default,
                    true,
                )
                .size(px(28.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.dispose_quick_terminal(cx);
                })),
            );
        }
        actions = actions.child(
            self.render_icon_button(
                "collapse_bottom_panel",
                IconName::Collapse,
                self.tr("terminal-collapse-panel"),
                IconTone::Default,
                true,
            )
            .size(px(28.0))
            .on_click(cx.listener(|this, _, _, cx| {
                this.bottom_panel_open = false;
                this.bottom_panel_resize = None;
                cx.notify();
            })),
        );

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(BOTTOM_PANEL_HEADER_HEIGHT))
            .px_2()
            .border_b_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.render_sidebar_icon(IconName::Terminal, 15.0))
                    .child(self.tr("terminal-label")),
            )
            .child(actions);

        panel = panel
            .h(px(self.bottom_panel_height))
            .min_h(px(BOTTOM_PANEL_HEADER_HEIGHT))
            .mx(px(-16.0))
            .mb(px(-16.0))
            .mt_2()
            .border_t_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.panel_bg)
            .child(
                div()
                    .id("bottom_panel_resize_handle")
                    .absolute()
                    .top(px(-3.0))
                    .left_0()
                    .right_0()
                    .h(px(6.0))
                    .cursor(CursorStyle::ResizeUpDown)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::begin_bottom_panel_resize),
                    ),
            )
            .child(self.render_quick_command_prompt(cx))
            .child(header)
            .child(self.render_quick_terminal_panel(cx));

        panel
    }

    fn render_quick_terminal_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(session_id) = self.quick_terminal_session_id else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    text_button(
                        "start_quick_terminal",
                        self.tr("terminal-start-local"),
                        TextButtonTone::Primary,
                        true,
                        &self.theme,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.restart_quick_terminal(window, cx);
                    })),
                )
                .into_any_element();
        };
        let ended = self
            .session(session_id)
            .is_some_and(TerminalSession::terminal_has_ended);
        let message = self.session(session_id).and_then(|session| {
            session
                .connection_error
                .as_ref()
                .or(session.terminal_end_reason.as_ref())
                .cloned()
        });
        let terminal = self.render_terminal_session_view(
            session_id,
            self.quick_terminal_focus_handle.clone(),
            None,
            "quick-terminal-view".into(),
            cx,
        );

        div()
            .flex()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .p_2()
            .pt_0()
            .child(terminal)
            .when(ended, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pt_2()
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(message.unwrap_or_else(|| self.tr("terminal-local-ended")))
                        .child(
                            text_button(
                                "restart_quick_terminal",
                                self.tr("terminal-restart"),
                                TextButtonTone::Secondary,
                                true,
                                &self.theme,
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.restart_quick_terminal(window, cx);
                                },
                            )),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_home(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut connections = div()
            .flex()
            .flex_col()
            .w_full()
            .overflow_hidden()
            .rounded_lg()
            .bg(self.theme.settings_group_bg);
        if self.profiles.is_empty() {
            connections = connections.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(72.0))
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("home-no-connections")),
            );
        } else {
            let visible_profile_count = self.profiles.len().min(6);
            for (index, profile) in self.profiles.iter().take(visible_profile_count).enumerate() {
                let profile_id = profile.id.clone();
                let hover = self.theme.control_hover_bg;
                connections = connections.child(
                    div()
                        .id(SharedString::from(format!("home-profile-{}", profile.id)))
                        .relative()
                        .flex()
                        .items_center()
                        .gap_3()
                        .h(px(52.0))
                        .px_3()
                        .when(index == 0, |this| this.rounded_t_lg())
                        .when(index + 1 == visible_profile_count, |this| {
                            this.rounded_b_lg()
                        })
                        .cursor_pointer()
                        .hover(move |this| this.bg(hover))
                        .child(self.render_sidebar_icon(IconName::Server, 19.0))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex_col()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(profile.name.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(self.theme.text_muted)
                                        .child(profile.address()),
                                ),
                        )
                        .child(self.render_sidebar_icon(IconName::Expand, 14.0))
                        .when(index + 1 < visible_profile_count, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .bottom_0()
                                    .left(px(44.0))
                                    .right(px(12.0))
                                    .h(px(1.0))
                                    .bg(self.theme.settings_separator),
                            )
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_profile(profile_id.clone(), window, cx);
                        })),
                );
            }
        }

        let new_connection = text_button(
            "home-new-connection",
            self.tr("sidebar-new-connection"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.add_profile(cx)));
        let local_terminal = text_button(
            "home-local-terminal",
            self.tr("sidebar-local-terminal"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.open_local_terminal(window, cx);
        }));

        self.detail_panel_shell().child(
            div()
                .id("home_content")
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .w_full()
                        .max_w(px(720.0))
                        .flex_col()
                        .py(px(64.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .size(px(42.0))
                                        .rounded_lg()
                                        .bg(self.theme.control_bg)
                                        .child(icon(
                                            IconName::Home,
                                            self.theme,
                                            IconTone::Default,
                                            23.0,
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_size(px(24.0))
                                                .font_weight(FontWeight::BOLD)
                                                .child(self.tr("app-name")),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(self.theme.text_muted)
                                                .child(self.tr("home-product-tagline")),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .mt_6()
                                .child(new_connection)
                                .child(local_terminal),
                        )
                        .child(
                            div()
                                .mt_8()
                                .mb_2()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.tr("sidebar-connections")),
                        )
                        .child(connections),
                ),
        )
    }

    fn render_server_overview(
        &self,
        profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Some(profile) = profile else {
            return self.render_home(cx);
        };
        let session = self.session_for_profile(&profile.id);
        let state = session
            .map(|session| session.connection_state)
            .unwrap_or(SessionState::Disconnected);
        let status = self.tr(session_state_key(state));
        let status_color = match state {
            SessionState::Connected => self.theme.status_ok,
            SessionState::Failed => self.theme.error_text,
            SessionState::Connecting
            | SessionState::Authenticating
            | SessionState::Disconnecting => self.theme.status_warn,
            SessionState::Disconnected => self.theme.text_muted,
        };
        let terminal_count = self
            .tabs
            .iter()
            .filter(|tab| tab.profile_id == profile.id)
            .count();
        let state_allows_connect = matches!(
            state,
            SessionState::Disconnected | SessionState::Connected | SessionState::Failed
        );
        let can_connect = state_allows_connect
            && self.credential_lookup_task.is_none()
            && !self
                .credential_mutations_in_progress
                .contains_key(&profile.id);
        let action_label = if state == SessionState::Connected {
            self.tr("connection-new-terminal")
        } else if state == SessionState::Failed {
            self.tr("connection-retry")
        } else if state == SessionState::Connecting || state == SessionState::Authenticating {
            self.tr("connection-status-connecting")
        } else if state == SessionState::Disconnecting {
            self.tr("connection-status-disconnecting")
        } else {
            self.tr("common-connect")
        };
        let action_hover = self.theme.accent_hover;
        let action_pressed = self.theme.button_primary_pressed_bg;
        let mut connect = div()
            .id("server-overview-connect")
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h(px(40.0))
            .px_5()
            .rounded_lg()
            .bg(self.theme.accent)
            .text_color(self.theme.on_accent)
            .font_weight(FontWeight::MEDIUM)
            .when(can_connect, |this| {
                this.cursor_pointer()
                    .hover(move |this| this.bg(action_hover))
                    .active(move |this| this.bg(action_pressed))
            })
            .when(!can_connect, |this| this.opacity(0.5))
            .child(icon_with_color(
                if state == SessionState::Failed {
                    IconName::Reconnect
                } else {
                    IconName::Connect
                },
                self.theme.on_accent,
                18.0,
            ))
            .child(action_label);
        if can_connect {
            connect = connect.on_click(cx.listener(|this, _, window, cx| {
                this.connect_selected_profile_in_new_session(window, cx);
            }));
        }

        let info = div()
            .flex()
            .w_full()
            .max_w(px(520.0))
            .flex_col()
            .overflow_hidden()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(self.render_server_info_row(
                self.tr("field-host").into(),
                profile.host.clone(),
                true,
            ))
            .child(self.render_server_info_row(
                self.tr("field-port").into(),
                profile.port.to_string(),
                true,
            ))
            .child(self.render_server_info_row(
                self.tr("field-username").into(),
                profile.username.clone(),
                true,
            ))
            .child(self.render_server_info_row(
                self.tr("profile-authentication").into(),
                profile_auth_label(&profile.auth, &self.localizer),
                true,
            ))
            .child(self.render_server_info_row(
                self.tr("connection-open-terminals").into(),
                terminal_count.to_string(),
                false,
            ));

        self.detail_panel_shell().child(
            div()
                .id("server_overview_content")
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .items_center()
                .child(
                    div()
                        .flex()
                        .w_full()
                        .flex_col()
                        .items_center()
                        .py(px(64.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(px(58.0))
                                .rounded_lg()
                                .bg(self.theme.control_bg)
                                .child(icon(IconName::Server, self.theme, IconTone::Default, 30.0)),
                        )
                        .child(
                            div()
                                .mt_4()
                                .text_size(px(28.0))
                                .font_weight(FontWeight::BOLD)
                                .child(profile.name.clone()),
                        )
                        .child(
                            div()
                                .mt_1()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(profile.address()),
                        )
                        .child(
                            div()
                                .mt_2()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(status_color)
                                .child(status),
                        )
                        .child(div().mt_6().child(connect))
                        .child(div().mt_8().w_full().max_w(px(520.0)).child(info))
                        .when_some(
                            session.and_then(|session| session.connection_error.as_ref()),
                            |this, error| {
                                this.child(
                                    div()
                                        .mt_3()
                                        .max_w(px(520.0))
                                        .text_sm()
                                        .text_color(self.theme.error_text)
                                        .child(error.clone()),
                                )
                            },
                        ),
                ),
        )
    }

    fn render_server_info_row(
        &self,
        label: SharedString,
        value: String,
        divided: bool,
    ) -> gpui::Div {
        div()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .min_h(px(40.0))
            .px_3()
            .text_sm()
            .child(div().text_color(self.theme.text_muted).child(label))
            .child(div().min_w(px(0.0)).truncate().child(value))
            .when(divided, |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left(px(12.0))
                        .right(px(12.0))
                        .h(px(1.0))
                        .bg(self.theme.settings_separator),
                )
            })
    }

    fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        match self.active_panel {
            ActivePanel::Home => return self.render_home(cx),
            ActivePanel::Server => return self.render_server_overview(selected_profile, cx),
            ActivePanel::Settings => return self.render_settings(cx),
            ActivePanel::OpenSshImport => return self.render_openssh_import(cx),
            ActivePanel::Connection => {}
        }
        if self.active_session().is_some_and(TerminalSession::is_local)
            && self
                .active_tab()
                .is_some_and(|tab| tab.profile_id == LOCAL_PROFILE_ID)
        {
            return self.render_local_terminal_panel(cx);
        }

        let mut panel = self.detail_panel_shell();

        if let Some(profile) = selected_profile
            && self.has_terminal_workspace(&profile.id)
        {
            panel = panel.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_wrap()
                    .items_center()
                    .justify_start()
                    .gap_1()
                    .min_h(px(36.0))
                    .mx(px(-16.0))
                    .px_4()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(self.render_workspace_controls(cx))
                    .child(self.render_pane_controls(cx))
                    .child(self.render_connection_controls(cx)),
            );

            match self.active_tab_view() {
                TerminalTabView::Terminal => {
                    if let Some(layout) = self.active_tab().map(|tab| &tab.layout) {
                        panel = panel.child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w(px(0.0))
                                .min_h(px(0.0))
                                .overflow_hidden()
                                .child(self.render_pane_layout(layout, cx)),
                        );
                    }
                }
                TerminalTabView::Files => {
                    if let Some(session_id) = self.active_session_id {
                        panel = panel.child(self.render_sftp_browser(
                            session_id,
                            SftpBrowserPlacement::Center,
                            cx,
                        ));
                    }
                }
            }
        } else {
            panel = panel.child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_color(self.theme.text_muted)
                    .child(self.tr("terminal-no-selected")),
            );
        }

        panel.child(self.render_bottom_panel(cx))
    }

    fn render_local_terminal_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut panel = self.detail_panel_shell().child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_start()
                .min_h(px(36.0))
                .mx(px(-16.0))
                .px_4()
                .border_b_1()
                .border_color(self.theme.border)
                .child(self.render_pane_controls(cx)),
        );

        if let Some(layout) = self.active_tab().map(|tab| &tab.layout) {
            panel = panel.child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(self.render_pane_layout(layout, cx)),
            );
        }

        panel.child(self.render_bottom_panel(cx))
    }

    fn detail_panel_shell(&self) -> gpui::Div {
        let mut panel = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .px_4()
            .pb_4()
            .pt(px(content_top_inset()));

        if cfg!(target_os = "windows") {
            panel = panel.child(
                div()
                    .absolute()
                    .top(px(content_top_inset() - 1.0))
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .bg(self.theme.panel_bg),
            );
        } else {
            let mut shadows = vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(-1.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }];
            panel = panel
                .bg(self.theme.panel_bg)
                .border_l_1()
                .border_color(self.theme.border_strong);
            if self.right_sidebar_open {
                panel = panel.border_r_1();
                shadows.push(BoxShadow {
                    color: self.theme.shadow,
                    offset: point(px(1.0), px(0.0)),
                    blur_radius: px(4.0),
                    spread_radius: px(-2.0),
                });
            }
            panel = panel.shadow(shadows);
        }

        panel
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
        let appearance_group = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(self.render_settings_row(
                "settings-language-row",
                self.tr("settings-language").into(),
                SettingsSelector::Language,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-theme-row",
                self.tr("settings-theme").into(),
                SettingsSelector::Theme,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-tab-layout-row",
                self.tr("settings-tab-layout").into(),
                SettingsSelector::TabLayout,
                false,
                cx,
            ));
        let terminal_group = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(self.render_settings_row(
                "settings-terminal-font-row",
                self.tr("settings-font").into(),
                SettingsSelector::TerminalFont,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-terminal-font-size-row",
                self.tr("settings-font-size").into(),
                SettingsSelector::TerminalFontSize,
                false,
                cx,
            ));
        let transfer_group = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(self.render_settings_row(
                "settings-transfer-rate-row",
                self.tr("settings-speed-limit").into(),
                SettingsSelector::TransferRate,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-parallel-files-row",
                self.tr("settings-parallel-files").into(),
                SettingsSelector::ParallelTransfers,
                false,
                cx,
            ));

        let content = div()
            .id("settings_content")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .px(px(100.0))
            .child(
                div()
                    .w_full()
                    .mt_6()
                    .mb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("settings-appearance")),
            )
            .child(appearance_group)
            .child(
                div()
                    .w_full()
                    .mt_6()
                    .mb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("settings-terminal")),
            )
            .child(terminal_group)
            .child(
                div()
                    .w_full()
                    .mt_6()
                    .mb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("settings-transfers")),
            )
            .child(transfer_group)
            .child(
                div()
                    .w_full()
                    .mt_6()
                    .mb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("import-title")),
            )
            .child(
                div()
                    .id("open-openssh-import")
                    .flex()
                    .items_center()
                    .justify_between()
                    .min_h(px(38.0))
                    .px(px(10.0))
                    .rounded_lg()
                    .bg(self.theme.settings_group_bg)
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|this| this.bg(self.theme.control_hover_bg))
                    .child(self.tr("import-openssh"))
                    .child(icon(IconName::Expand, self.theme, IconTone::Default, 15.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_openssh_import(window, cx);
                    })),
            )
            .when_some(self.settings_error.as_ref(), |this, error| {
                this.child(
                    div()
                        .w_full()
                        .mt_3()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            });

        self.detail_panel_shell()
            .px(px(0.0))
            .key_context("Settings")
            .track_focus(&self.settings_focus_handle)
            .on_action(cx.listener(Self::on_cancel_settings_selector))
            .child(content)
    }

    fn render_openssh_import(&self, cx: &mut Context<Self>) -> gpui::Div {
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

    fn render_settings_row(
        &self,
        id: &'static str,
        label: SharedString,
        selector: SettingsSelector,
        divided: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .min_h(px(38.0))
            .px(px(10.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
            )
            .child(self.render_settings_selector(selector, cx))
            .when(divided, |this| {
                this.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left(px(10.0))
                        .right(px(10.0))
                        .h(px(1.0))
                        .bg(self.theme.settings_separator),
                )
            })
    }

    fn render_settings_selector(
        &self,
        selector: SettingsSelector,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if selector == SettingsSelector::TerminalFont {
            return self.render_terminal_font_selector(cx);
        }

        let is_open = self.open_settings_selector == Some(selector);
        let max_control_width = selector.control_width();
        let menu_width = selector.menu_width();
        let control_group: SharedString = format!("{}-control", selector.element_id()).into();
        let option_count = selector.options().len();
        let menu_height = select_menu_height(option_count);
        let menu = self
            .glass_floating_surface()
            .id(SharedString::from(format!(
                "{}-menu",
                selector.element_id()
            )))
            .absolute()
            .top(px(26.0))
            .right_0()
            .w(px(menu_width))
            .h(px(menu_height))
            .flex()
            .flex_col()
            .p_1()
            .text_sm()
            .occlude();
        let menu = if option_count > SELECT_MENU_MAX_VISIBLE_ROWS {
            menu.overflow_hidden().child(
                uniform_list(
                    SharedString::from(format!("{}-virtual-options", selector.element_id())),
                    option_count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.render_virtual_settings_selector_rows(selector, range, cx)
                    }),
                )
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .track_scroll(self.settings_virtual_selector_scroll_handle.clone()),
            )
        } else {
            let mut menu = menu
                .overflow_y_scroll()
                .track_scroll(&self.settings_selector_scroll_handle);
            for (index, option) in selector.options().iter().copied().enumerate() {
                menu = menu
                    .child(self.render_settings_selector_row(selector, index, option, false, cx));
            }
            menu
        };

        let button_hover = self.theme.control_hover_bg;
        let button_pressed = self.theme.control_pressed_bg;
        let current_label = self.settings_value_label(selector);
        let picker = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.0))
            .rounded_full()
            .bg(if is_open {
                self.theme.control_pressed_bg
            } else {
                self.theme.settings_picker_bg
            })
            .when(!is_open, |this| {
                this.group_hover(control_group.clone(), |style| {
                    style.bg(self.theme.transparent)
                })
            })
            .child(icon(IconName::Picker, self.theme, IconTone::Default, 15.0));
        let button = div()
            .id(selector.element_id())
            .group(control_group)
            .flex()
            .flex_none()
            .max_w(px(max_control_width))
            .items_center()
            .h(px(24.0))
            .pl(px(6.0))
            .pr(px(1.0))
            .rounded_lg()
            .bg(if is_open {
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
                    .min_w(px(0.0))
                    .max_w(px(max_control_width - 33.0))
                    .truncate()
                    .pr(px(4.0))
                    .text_right()
                    .child(current_label),
            )
            .child(picker)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_settings_selector(selector, window, cx);
            }));

        div()
            .relative()
            .flex()
            .flex_none()
            .max_w(px(max_control_width))
            .child(button)
            .when(is_open, |this| this.child(deferred(menu).with_priority(10)))
    }

    fn render_settings_selector_row(
        &self,
        selector: SettingsSelector,
        index: usize,
        option: SettingsOption,
        virtualized: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected = option.value == self.settings_value(selector);
        let hover_group: SharedString =
            format!("{}-option-{index}-hover", selector.element_id()).into();
        let check = self.render_select_menu_check(is_selected, hover_group.clone());
        let label = if virtualized {
            self.render_virtual_select_menu_label(
                option.label.into(),
                hover_group.clone(),
                selector.menu_width() - 40.0,
            )
        } else {
            self.render_select_menu_label(option.label.into(), hover_group.clone())
        };
        let option_hover = self.theme.accent;
        let option_pressed = self.theme.accent_hover;
        let on_accent = self.theme.on_accent;

        let row = div()
            .id(SharedString::from(format!(
                "{}-option-{index}",
                selector.element_id()
            )))
            .group(hover_group)
            .flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .h(px(SELECT_MENU_ROW_HEIGHT))
            .px_2()
            .rounded_md()
            .text_color(self.theme.text_primary)
            .cursor_pointer()
            .hover(move |this| this.bg(option_hover).text_color(on_accent))
            .active(move |this| this.bg(option_pressed).text_color(on_accent))
            .child(check)
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.apply_settings_value(option.value, window, cx);
            }));

        row.into_any_element()
    }

    fn render_virtual_settings_selector_rows(
        &self,
        selector: SettingsSelector,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|index| {
                selector.options().get(index).copied().map(|option| {
                    self.render_settings_selector_row(selector, index, option, true, cx)
                })
            })
            .collect()
    }

    fn render_terminal_font_selector(&self, cx: &mut Context<Self>) -> gpui::Div {
        let selector = SettingsSelector::TerminalFont;
        let is_open = self.open_settings_selector == Some(selector);
        let max_control_width = selector.control_width();
        let control_group: SharedString = format!("{}-control", selector.element_id()).into();
        let option_count = self.terminal_font_families.len();
        let menu_height = select_menu_height(option_count);
        let menu = self
            .glass_floating_surface()
            .id(SharedString::from(format!(
                "{}-menu",
                selector.element_id()
            )))
            .absolute()
            .top(px(26.0))
            .right_0()
            .w(px(selector.menu_width()))
            .h(px(menu_height))
            .flex()
            .flex_col()
            .p_1()
            .text_sm()
            .occlude();
        let menu = if option_count > SELECT_MENU_MAX_VISIBLE_ROWS {
            menu.overflow_hidden().child(
                uniform_list(
                    "settings-terminal-font-virtual-options",
                    option_count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.render_virtual_terminal_font_selector_rows(range, cx)
                    }),
                )
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .track_scroll(self.settings_virtual_selector_scroll_handle.clone()),
            )
        } else {
            let mut menu = menu
                .overflow_y_scroll()
                .track_scroll(&self.settings_selector_scroll_handle);
            for (index, family) in self.terminal_font_families.iter().cloned().enumerate() {
                menu = menu.child(self.render_terminal_font_selector_row(index, family, false, cx));
            }
            menu
        };

        let button_hover = self.theme.control_hover_bg;
        let button_pressed = self.theme.control_pressed_bg;
        let picker = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.0))
            .rounded_full()
            .bg(if is_open {
                self.theme.control_pressed_bg
            } else {
                self.theme.settings_picker_bg
            })
            .when(!is_open, |this| {
                this.group_hover(control_group.clone(), |style| {
                    style.bg(self.theme.transparent)
                })
            })
            .child(icon(IconName::Picker, self.theme, IconTone::Default, 15.0));
        let button = div()
            .id(selector.element_id())
            .group(control_group)
            .flex()
            .flex_none()
            .max_w(px(max_control_width))
            .items_center()
            .h(px(24.0))
            .pl(px(6.0))
            .pr(px(1.0))
            .rounded_lg()
            .bg(if is_open {
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
                    .min_w(px(0.0))
                    .max_w(px(max_control_width - 33.0))
                    .truncate()
                    .pr(px(4.0))
                    .text_right()
                    .child(self.terminal_font_family.clone()),
            )
            .child(picker)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_settings_selector(selector, window, cx);
            }));

        div()
            .relative()
            .flex()
            .flex_none()
            .max_w(px(max_control_width))
            .child(button)
            .when(is_open, |this| this.child(deferred(menu).with_priority(10)))
    }

    fn render_terminal_font_selector_row(
        &self,
        index: usize,
        family: SharedString,
        virtualized: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = SettingsSelector::TerminalFont;
        let is_selected = family == self.terminal_font_family;
        let hover_group: SharedString =
            format!("{}-option-{index}-hover", selector.element_id()).into();
        let check = self.render_select_menu_check(is_selected, hover_group.clone());
        let label = if virtualized {
            self.render_virtual_select_menu_label(
                family.clone(),
                hover_group.clone(),
                selector.menu_width() - 40.0,
            )
        } else {
            self.render_select_menu_label(family.clone(), hover_group.clone())
        };
        let selected_family = family;
        let option_hover = self.theme.accent;
        let option_pressed = self.theme.accent_hover;
        let on_accent = self.theme.on_accent;

        let row = div()
            .id(SharedString::from(format!(
                "{}-option-{index}",
                selector.element_id()
            )))
            .group(hover_group)
            .flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap_2()
            .h(px(SELECT_MENU_ROW_HEIGHT))
            .px_2()
            .rounded_md()
            .text_color(self.theme.text_primary)
            .cursor_pointer()
            .hover(move |this| this.bg(option_hover).text_color(on_accent))
            .active(move |this| this.bg(option_pressed).text_color(on_accent))
            .child(check)
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.set_terminal_font_family(selected_family.clone(), cx);
            }));

        row.into_any_element()
    }

    fn render_virtual_terminal_font_selector_rows(
        &self,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|index| {
                self.terminal_font_families
                    .get(index)
                    .cloned()
                    .map(|family| self.render_terminal_font_selector_row(index, family, true, cx))
            })
            .collect()
    }

    fn render_select_menu_label(
        &self,
        label: SharedString,
        hover_group: SharedString,
    ) -> gpui::Div {
        let hover_label = label.clone();

        div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .child(
                div()
                    .absolute()
                    .flex()
                    .items_center()
                    .size_full()
                    .min_w(px(0.0))
                    .truncate()
                    .text_color(self.theme.text_primary)
                    .group_hover(hover_group.clone(), |style| style.opacity(0.0))
                    .child(label),
            )
            .child(
                div()
                    .absolute()
                    .flex()
                    .items_center()
                    .size_full()
                    .min_w(px(0.0))
                    .truncate()
                    .opacity(0.0)
                    .text_color(self.theme.on_accent)
                    .group_hover(hover_group, |style| style.opacity(1.0))
                    .child(hover_label),
            )
    }

    fn render_virtual_select_menu_label(
        &self,
        label: SharedString,
        hover_group: SharedString,
        max_width: f32,
    ) -> gpui::Div {
        let hover_label = label.clone();

        div()
            .relative()
            .flex()
            .flex_none()
            .min_w(px(0.0))
            .max_w(px(max_width))
            .h_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w(px(0.0))
                    .max_w(px(max_width))
                    .h_full()
                    .truncate()
                    .text_color(self.theme.text_primary)
                    .group_hover(hover_group.clone(), |style| style.opacity(0.0))
                    .child(label),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .min_w(px(0.0))
                    .max_w(px(max_width))
                    .truncate()
                    .opacity(0.0)
                    .text_color(self.theme.on_accent)
                    .group_hover(hover_group, |style| style.opacity(1.0))
                    .child(hover_label),
            )
    }

    fn render_select_menu_check(&self, selected: bool, hover_group: SharedString) -> gpui::Div {
        let check = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(16.0));
        if !selected {
            return check;
        }

        check
            .child(
                div()
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .group_hover(hover_group.clone(), |style| style.opacity(0.0))
                    .child(icon_with_color(IconName::Check, self.theme.accent, 15.0)),
            )
            .child(
                div()
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size_full()
                    .opacity(0.0)
                    .group_hover(hover_group, |style| style.opacity(1.0))
                    .child(icon_with_color(IconName::Check, self.theme.on_accent, 15.0)),
            )
    }

    fn render_pane_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_workspace = self
            .selected_profile_id
            .as_deref()
            .is_some_and(|profile_id| self.has_terminal_workspace(profile_id));
        if !has_workspace || self.active_tab_view() != TerminalTabView::Terminal {
            return div().id("pane_controls_empty");
        }

        let can_split = self.credential_lookup_task.is_none()
            && self
                .selected_profile_id
                .as_deref()
                .is_none_or(|profile_id| {
                    !self
                        .credential_mutations_in_progress
                        .contains_key(profile_id)
                });

        div()
            .id("pane_controls")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                self.render_icon_button(
                    "split_pane_right",
                    IconName::SplitRight,
                    self.tr("terminal-split-right"),
                    IconTone::Default,
                    can_split,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.split_active_pane(SplitAxis::Horizontal, window, cx);
                })),
            )
            .child(
                self.render_icon_button(
                    "split_pane_down",
                    IconName::SplitDown,
                    self.tr("terminal-split-down"),
                    IconTone::Default,
                    can_split,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.split_active_pane(SplitAxis::Vertical, window, cx);
                })),
            )
            .child(
                self.render_icon_button(
                    "close_active_pane",
                    IconName::ClosePane,
                    self.tr("terminal-close-pane"),
                    IconTone::Default,
                    self.active_tab_id.is_some_and(|tab_id| {
                        self.panes
                            .iter()
                            .filter(|pane| pane.tab_id == tab_id)
                            .count()
                            > 1
                    }),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close_active_pane(window, cx);
                })),
            )
    }

    fn render_workspace_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_workspace = self
            .selected_profile_id
            .as_deref()
            .is_some_and(|profile_id| self.has_terminal_workspace(profile_id));
        if !has_workspace {
            return div().id("workspace_controls_empty");
        }

        let view = self.active_tab_view();
        let can_browse_files = self
            .active_session()
            .is_some_and(|session| session.connection_state == SessionState::Connected);
        let selected_background = self.theme.control_bg;
        let terminal_button = self
            .render_icon_button(
                "show_terminal",
                IconName::Terminal,
                self.tr("terminal-view"),
                IconTone::Default,
                true,
            )
            .bg(if view == TerminalTabView::Terminal {
                selected_background
            } else {
                self.theme.transparent
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.set_active_tab_view(TerminalTabView::Terminal, window, cx);
            }));
        let mut files_button = self
            .render_icon_button(
                "show_remote_files",
                IconName::Folder,
                self.tr("terminal-remote-files"),
                IconTone::Default,
                can_browse_files,
            )
            .bg(if view == TerminalTabView::Files {
                selected_background
            } else {
                self.theme.transparent
            });
        if can_browse_files {
            files_button = files_button.on_click(cx.listener(|this, _, window, cx| {
                this.set_active_tab_view(TerminalTabView::Files, window, cx);
            }));
        }

        div()
            .id("workspace_controls")
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(terminal_button)
            .child(files_button)
    }

    fn render_profile_context_menu(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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

    fn render_terminal_context_menu(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu_state) = self.terminal_context_menu.as_ref() else {
            return div().into_any_element();
        };
        let session_id = menu_state.session_id;
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        let can_copy = session
            .terminal_selection
            .is_some_and(|selection| !selection.is_empty());
        let can_paste =
            session.connection_state == SessionState::Connected && session.terminal.is_some();
        let can_select_all = session.terminal.as_ref().is_some_and(|terminal| {
            let size = terminal.engine.size();
            size.rows() > 0 && size.columns() > 0
        });
        let can_reset = session.terminal.is_some();
        let viewport = window.viewport_size();
        let left = f32::from(menu_state.position.x)
            .min((f32::from(viewport.width) - 196.0).max(8.0))
            .max(8.0);
        let top = f32::from(menu_state.position.y)
            .min((f32::from(viewport.height) - 166.0).max(8.0))
            .max(8.0);

        let mut copy = self.render_context_menu_item(
            "terminal-context-copy",
            IconName::Copy,
            self.tr("common-copy"),
            IconTone::Default,
            can_copy,
        );
        if can_copy {
            copy = copy.on_click(cx.listener(move |this, _, _, cx| {
                this.copy_terminal_selection(session_id, cx);
                this.terminal_context_menu = None;
                cx.notify();
            }));
        }
        let mut paste = self.render_context_menu_item(
            "terminal-context-paste",
            IconName::Paste,
            self.tr("common-paste"),
            IconTone::Default,
            can_paste,
        );
        if can_paste {
            paste = paste.on_click(cx.listener(move |this, _, _, cx| {
                this.paste_into_terminal(session_id, cx);
                this.terminal_context_menu = None;
                cx.notify();
            }));
        }
        let mut select_all = self.render_context_menu_item(
            "terminal-context-select-all",
            IconName::SelectAll,
            self.tr("common-select-all"),
            IconTone::Default,
            can_select_all,
        );
        if can_select_all {
            select_all = select_all.on_click(cx.listener(move |this, _, _, cx| {
                this.select_all_terminal(session_id, cx);
                this.terminal_context_menu = None;
                cx.notify();
            }));
        }
        let mut reset = self.render_context_menu_item(
            "terminal-context-reset",
            IconName::Reconnect,
            self.tr("terminal-reset"),
            IconTone::Default,
            can_reset,
        );
        if can_reset {
            reset = reset.on_click(cx.listener(move |this, _, _, cx| {
                this.reset_terminal(session_id, cx);
            }));
        }

        self.glass_floating_surface()
            .id("terminal_context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(188.0))
            .flex()
            .flex_col()
            .p_1()
            .occlude()
            .child(copy)
            .child(paste)
            .child(self.render_context_menu_separator())
            .child(select_all)
            .child(self.render_context_menu_separator())
            .child(reset)
            .into_any_element()
    }

    fn render_sftp_context_menu(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(menu_state) = self.sftp_context_menu.as_ref() else {
            return div().into_any_element();
        };
        let session_id = menu_state.session_id;
        let placement = menu_state.placement;
        let entries = self.selected_sftp_entries(session_id, placement);
        let connected = self
            .session(session_id)
            .is_some_and(|session| session.connection_state == SessionState::Connected);
        let single_file = (entries.len() == 1 && entries[0].kind == RemoteFileKind::File)
            .then(|| entries[0].clone());
        let can_download = connected
            && entries.iter().any(|entry| {
                matches!(entry.kind, RemoteFileKind::File | RemoteFileKind::Directory)
            });
        let viewport = window.viewport_size();
        let left = f32::from(menu_state.position.x)
            .min((f32::from(viewport.width) - 196.0).max(8.0))
            .max(8.0);
        let top = f32::from(menu_state.position.y)
            .min((f32::from(viewport.height) - 286.0).max(8.0))
            .max(8.0);

        let mut new_file = self.render_context_menu_item(
            "sftp-context-new-file",
            IconName::File,
            self.tr("sftp-new-file"),
            IconTone::Default,
            connected,
        );
        if connected {
            new_file = new_file.on_click(cx.listener(move |this, _, window, cx| {
                this.open_sftp_create_prompt(
                    session_id,
                    placement,
                    SftpCreateKind::File,
                    window,
                    cx,
                );
            }));
        }
        let mut new_folder = self.render_context_menu_item(
            "sftp-context-new-folder",
            IconName::Folder,
            self.tr("sftp-new-folder"),
            IconTone::Default,
            connected,
        );
        if connected {
            new_folder = new_folder.on_click(cx.listener(move |this, _, window, cx| {
                this.open_sftp_create_prompt(
                    session_id,
                    placement,
                    SftpCreateKind::Directory,
                    window,
                    cx,
                );
            }));
        }
        let mut copy_path = self.render_context_menu_item(
            "sftp-context-copy-path",
            IconName::Copy,
            self.tr("sftp-copy-path"),
            IconTone::Default,
            !entries.is_empty(),
        );
        if !entries.is_empty() {
            copy_path = copy_path.on_click(cx.listener(move |this, _, _, cx| {
                this.copy_selected_sftp_paths(session_id, placement, cx);
            }));
        }
        let mut view = self.render_context_menu_item(
            "sftp-context-view",
            IconName::View,
            self.tr("sftp-view"),
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            view = view.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, false, cx);
            }));
        }
        let mut edit = self.render_context_menu_item(
            "sftp-context-edit",
            IconName::Edit,
            self.tr("sftp-edit"),
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            edit = edit.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, true, cx);
            }));
        }
        let mut download = self.render_context_menu_item(
            "sftp-context-download",
            IconName::Download,
            self.tr("sftp-download"),
            IconTone::Default,
            can_download,
        );
        if can_download {
            download = download.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let mut delete = self.render_context_menu_item(
            "sftp-context-delete",
            IconName::Delete,
            self.tr("common-delete"),
            IconTone::Danger,
            connected && !entries.is_empty(),
        );
        if connected && !entries.is_empty() {
            delete = delete.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        self.glass_floating_surface()
            .id("sftp_context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(188.0))
            .flex()
            .flex_col()
            .p_1()
            .occlude()
            .child(new_file)
            .child(new_folder)
            .child(self.render_context_menu_separator())
            .child(copy_path)
            .child(view)
            .child(edit)
            .child(download)
            .child(self.render_context_menu_separator())
            .child(delete)
            .into_any_element()
    }

    fn render_context_menu_item(
        &self,
        id: &'static str,
        icon_name: IconName,
        label: impl Into<SharedString>,
        tone: IconTone,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let label = label.into();
        let (hover, pressed) = match tone {
            IconTone::Danger => (self.theme.danger, self.theme.danger_hover),
            IconTone::Accent | IconTone::Default => (self.theme.accent, self.theme.accent_hover),
        };
        let on_accent = self.theme.on_accent;
        let base_color = match tone {
            IconTone::Accent => self.theme.accent,
            IconTone::Danger | IconTone::Default => self.theme.text_primary,
        };
        let hover_group = SharedString::from(format!("{id}-hover"));
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.0))
            .px_2()
            .rounded_md()
            .text_sm()
            .when(enabled, |this| {
                this.group(hover_group.clone())
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .active(move |this| this.bg(pressed))
            })
            .when(!enabled, |this| this.opacity(0.45))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .relative()
                    .items_center()
                    .justify_center()
                    .size(px(16.0))
                    .child(
                        div()
                            .absolute()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size_full()
                            .when(enabled, |this| {
                                this.group_hover(hover_group.clone(), |style| style.opacity(0.0))
                            })
                            .child(icon_with_color(icon_name, base_color, 14.0)),
                    )
                    .when(enabled, |this| {
                        this.child(
                            div()
                                .absolute()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size_full()
                                .opacity(0.0)
                                .group_hover(hover_group.clone(), |style| style.opacity(1.0))
                                .child(icon_with_color(icon_name, on_accent, 14.0)),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h_full()
                    .child(
                        div()
                            .absolute()
                            .flex()
                            .items_center()
                            .size_full()
                            .text_color(base_color)
                            .when(enabled, |this| {
                                this.group_hover(hover_group.clone(), |style| style.opacity(0.0))
                            })
                            .child(label.clone()),
                    )
                    .when(enabled, |this| {
                        this.child(
                            div()
                                .absolute()
                                .flex()
                                .items_center()
                                .size_full()
                                .opacity(0.0)
                                .text_color(on_accent)
                                .group_hover(hover_group, |style| style.opacity(1.0))
                                .child(label),
                        )
                    }),
            )
    }

    fn render_context_menu_separator(&self) -> gpui::Div {
        div().h(px(1.0)).mx_2().my_1().bg(self.theme.border)
    }

    fn render_sftp_create_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.sftp_create_prompt.as_ref() else {
            return div().into_any_element();
        };
        let title = match prompt.kind {
            SftpCreateKind::File => self.tr("sftp-new-file"),
            SftpCreateKind::Directory => self.tr("sftp-new-folder"),
        };
        let input = prompt.input.clone();
        let create = text_button(
            "submit_sftp_create",
            self.tr("common-create"),
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.submit_sftp_create(cx)));
        let cancel = text_button(
            "cancel_sftp_create",
            self.tr("common-cancel"),
            TextButtonTone::Secondary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.sftp_create_prompt = None;
            cx.notify();
        }));

        div()
            .id("sftp_create_prompt_overlay")
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
            .key_context("SftpCreatePrompt")
            .on_action(cx.listener(|this, _: &SubmitSftpCreate, _, cx| this.submit_sftp_create(cx)))
            .on_action(cx.listener(|this, _: &CancelSftpCreate, _, cx| {
                this.sftp_create_prompt = None;
                cx.notify();
            }))
            .child(
                self.glass_floating_surface()
                    .w(px(360.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(input)
                    .when_some(prompt.error.as_ref(), |this, error| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(self.theme.error_text)
                                .child(error.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(cancel)
                            .child(create),
                    ),
            )
            .into_any_element()
    }

    fn render_quick_command_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.quick_command_prompt.as_ref() else {
            return div().into_any_element();
        };
        let connected_profile_ids = self.connected_ssh_profile_ids();
        let selected_count = connected_profile_ids
            .iter()
            .filter(|profile_id| prompt.selected_profile_ids.contains(*profile_id))
            .count();
        let target_label: SharedString =
            if selected_count == connected_profile_ids.len() && !connected_profile_ids.is_empty() {
                self.tr("quick-all-servers").into()
            } else if selected_count == 0 {
                self.tr("quick-no-servers").into()
            } else if selected_count == 1 {
                let selected_profile_id = connected_profile_ids
                    .iter()
                    .find(|profile_id| prompt.selected_profile_ids.contains(*profile_id));
                selected_profile_id
                    .and_then(|profile_id| {
                        self.profiles
                            .iter()
                            .find(|profile| profile.id == *profile_id)
                    })
                    .map(|profile| profile.name.clone().into())
                    .unwrap_or_else(|| self.tr("quick-one-server").into())
            } else {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", selected_count);
                self.tr_with("quick-server-count", &args).into()
            };

        let selector_group: SharedString = "quick-command-target-control".into();
        let selector_hover = self.theme.control_hover_bg;
        let pressed_background = self.theme.control_pressed_bg;
        let picker = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(22.0))
            .rounded_full()
            .bg(if prompt.target_menu_open {
                self.theme.control_pressed_bg
            } else {
                self.theme.settings_picker_bg
            })
            .when(!prompt.target_menu_open, |this| {
                this.group_hover(selector_group.clone(), |style| {
                    style.bg(self.theme.transparent)
                })
            })
            .child(icon(IconName::Picker, self.theme, IconTone::Default, 15.0));
        let selector_button = div()
            .id("quick_command_targets")
            .group(selector_group)
            .flex()
            .flex_none()
            .items_center()
            .max_w(px(180.0))
            .h(px(24.0))
            .pl(px(6.0))
            .pr(px(1.0))
            .rounded_lg()
            .bg(if prompt.target_menu_open {
                self.theme.control_hover_bg
            } else {
                self.theme.transparent
            })
            .text_sm()
            .cursor_pointer()
            .hover(move |this| this.bg(selector_hover))
            .active(move |this| this.bg(pressed_background))
            .child(
                div()
                    .flex_none()
                    .min_w(px(0.0))
                    .max_w(px(147.0))
                    .truncate()
                    .pr(px(4.0))
                    .text_right()
                    .child(target_label),
            )
            .child(picker)
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_quick_command_targets(cx);
            }));

        let mut selector = div()
            .relative()
            .flex()
            .flex_none()
            .max_w(px(180.0))
            .child(selector_button);
        if prompt.target_menu_open {
            let mut menu = self
                .glass_floating_surface()
                .id("quick_command_target_menu")
                .absolute()
                .top(px(26.0))
                .right_0()
                .w(px(240.0))
                .max_h(px(220.0))
                .flex()
                .flex_col()
                .overflow_y_scroll()
                .p_1()
                .text_sm()
                .occlude();
            let option_hover = self.theme.accent;
            let option_pressed = self.theme.accent_hover;
            let on_accent = self.theme.on_accent;
            for (index, profile_id) in connected_profile_ids.iter().enumerate() {
                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == *profile_id)
                else {
                    continue;
                };
                let is_selected = prompt.selected_profile_ids.contains(profile_id);
                let hover_group: SharedString =
                    format!("quick-command-target-option-{index}-hover").into();
                let check = self.render_select_menu_check(is_selected, hover_group.clone());
                let target_profile_id = profile_id.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!(
                            "quick-command-target-option-{index}"
                        )))
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
                        .child(
                            self.render_select_menu_label(profile.name.clone().into(), hover_group),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_quick_command_target(target_profile_id.clone(), cx);
                        })),
                );
            }
            selector = selector.child(deferred(menu).with_priority(30));
        }

        let run_enabled = !prompt.input.read(cx).text().trim().is_empty()
            && selected_count > 0
            && !connected_profile_ids.is_empty();
        let input = prompt.input.clone();
        let run = text_button(
            "submit_quick_command",
            self.tr("common-run"),
            TextButtonTone::Primary,
            run_enabled,
            &self.theme,
        );
        let run = if run_enabled {
            run.on_click(cx.listener(|this, _, _, cx| this.submit_quick_command(cx)))
        } else {
            run
        };

        div()
            .id("quick_command_panel")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(px(42.0))
            .px_3()
            .border_b_1()
            .border_color(self.theme.border)
            .key_context("QuickCommandPrompt")
            .on_action(
                cx.listener(|this, _: &SubmitQuickCommand, _, cx| this.submit_quick_command(cx)),
            )
            .on_action(
                cx.listener(|this, _: &CancelQuickCommand, _, cx| this.close_quick_command(cx)),
            )
            .child(self.render_sidebar_icon(IconName::QuickCommand, 15.0))
            .child(div().flex().flex_1().min_w(px(100.0)).child(input))
            .when_some(prompt.error.as_ref(), |this, error| {
                this.child(
                    div()
                        .max_w(px(140.0))
                        .truncate()
                        .text_sm()
                        .text_color(self.theme.error_text)
                        .child(error.clone()),
                )
            })
            .child(selector)
            .child(run)
            .into_any_element()
    }

    fn render_sftp_browser(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        if placement == SftpBrowserPlacement::Center && session.sftp.file.is_some() {
            return self.render_sftp_file(session_id, cx);
        }
        if session.connection_state == SessionState::Connected {
            match &session.sftp_availability {
                SftpAvailability::Checking => {
                    return self.render_sftp_availability_hint(
                        self.tr("sftp-checking"),
                        self.tr("sftp-checking-detail"),
                    );
                }
                SftpAvailability::Unavailable(message) => {
                    return self.render_sftp_availability_hint(
                        self.tr("sftp-unavailable"),
                        message.clone(),
                    );
                }
                SftpAvailability::Available => {}
            }
        }
        let browser_state = session.sftp_browser(placement);
        let path = browser_state.path.clone();
        let tree = placement == SftpBrowserPlacement::Sidebar;
        let entry_count = browser_state.visible_rows(tree).len();
        let selected_entries = browser_state.selected_entries();
        let scroll_handle = browser_state.scroll_handle.clone();
        let loading = browser_state.loading;
        let loaded = browser_state.loaded;
        let error = browser_state.error.clone();
        let connected = session.connection_state == SessionState::Connected;
        let can_go_up = connected && remote_parent_path(&path).is_some() && !loading;
        let element_suffix = placement.element_suffix();
        let list_id = SharedString::from(format!("sftp_directory_entries_{element_suffix}"));

        let list = if !loaded && loading {
            div()
                .id(list_id)
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-loading-files"))
                .into_any_element()
        } else if loaded && entry_count == 0 {
            div()
                .id(list_id)
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-empty-directory"))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .overflow_hidden()
                .child(
                    uniform_list(
                        list_id,
                        entry_count,
                        cx.processor(move |this, range: Range<usize>, _, cx| {
                            this.render_sftp_entry_rows(session_id, placement, range, cx)
                        }),
                    )
                    .size_full()
                    .track_scroll(scroll_handle),
                )
                .into_any_element()
        };

        let mut parent_button = self.render_icon_button(
            SharedString::from(format!("sftp_parent_directory_{element_suffix}")),
            IconName::ArrowUp,
            self.tr("sftp-parent-directory"),
            IconTone::Default,
            can_go_up,
        );
        if can_go_up {
            parent_button = parent_button.on_click(cx.listener(move |this, _, _, cx| {
                this.open_parent_remote_directory(placement, cx);
            }));
        }
        let can_refresh = connected && !loading;
        let mut refresh_button = self.render_icon_button(
            SharedString::from(format!("sftp_refresh_directory_{element_suffix}")),
            IconName::Reconnect,
            self.tr("common-refresh"),
            IconTone::Default,
            can_refresh,
        );
        if can_refresh {
            refresh_button = refresh_button.on_click(cx.listener(move |this, _, _, cx| {
                this.refresh_active_sftp_directory(placement, cx);
            }));
        }
        let can_upload = connected && !loading;
        let mut upload_button = self.render_icon_button(
            SharedString::from(format!("sftp_upload_{element_suffix}")),
            IconName::Upload,
            self.tr("sftp-upload-files"),
            IconTone::Default,
            can_upload,
        );
        if can_upload {
            upload_button = upload_button.on_click(cx.listener(move |this, _, _, cx| {
                this.choose_sftp_uploads(session_id, placement, cx);
            }));
        }
        let can_download = connected
            && !loading
            && selected_entries.iter().any(|entry| {
                matches!(entry.kind, RemoteFileKind::File | RemoteFileKind::Directory)
            });
        let mut download_button = self.render_icon_button(
            SharedString::from(format!("sftp_download_selected_{element_suffix}")),
            IconName::Download,
            self.tr("sftp-download-selected"),
            IconTone::Default,
            can_download,
        );
        if can_download {
            download_button = download_button.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let can_delete = connected && !loading && !selected_entries.is_empty();
        let browser_background = if placement == SftpBrowserPlacement::Sidebar {
            self.theme.transparent
        } else {
            self.theme.panel_bg
        };
        let mut delete_button = self.render_icon_button(
            SharedString::from(format!("sftp_delete_selected_{element_suffix}")),
            IconName::Delete,
            self.tr("sftp-delete-selected"),
            IconTone::Danger,
            can_delete,
        );
        if can_delete {
            delete_button = delete_button.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        let mut browser = div()
            .id(SharedString::from(format!("sftp_browser_{element_suffix}")))
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .mt_4()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(browser_background)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(session) = this.session_mut(session_id) {
                        let browser = session.sftp_browser_mut(placement);
                        browser.selected_paths.clear();
                        browser.selection_anchor = None;
                    }
                    this.sftp_context_menu = Some(SftpContextMenu {
                        session_id,
                        placement,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .h(px(40.0))
                    .px_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(parent_button)
                    .child(refresh_button)
                    .child(upload_button)
                    .child(download_button)
                    .child(delete_button)
                    .child(div().flex_1())
                    .when(loading && loaded, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("common-loading")),
                        )
                    }),
            );

        if let Some(error) = error {
            browser = browser.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.control_bg)
                    .text_sm()
                    .text_color(self.theme.error_text)
                    .child(error),
            );
        }

        browser
            .child(list)
            .child(self.render_sftp_transfer_queue(session_id, placement, cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(32.0))
                    .min_w(px(0.0))
                    .px_2()
                    .overflow_hidden()
                    .border_t_1()
                    .border_color(self.theme.border)
                    .bg(self.theme.surface_bg)
                    .child(self.render_sftp_breadcrumbs(session_id, placement, &path, cx)),
            )
            .into_any_element()
    }

    fn render_sftp_availability_hint(
        &self,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_4()
            .text_center()
            .child(self.render_sidebar_icon(IconName::Folder, 22.0))
            .child(div().font_weight(FontWeight::MEDIUM).child(title.into()))
            .child(
                div()
                    .max_w(px(420.0))
                    .text_sm()
                    .text_color(self.theme.text_muted)
                    .child(message.into()),
            )
            .into_any_element()
    }

    fn render_sftp_breadcrumbs(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let hover = self.theme.control_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let scroll_handle = self
            .session(session_id)
            .map(|session| {
                session
                    .sftp_browser(placement)
                    .breadcrumb_scroll_handle
                    .clone()
            })
            .unwrap_or_default();
        let mut breadcrumbs = div()
            .id(SharedString::from(format!(
                "sftp-breadcrumbs-{}",
                placement.element_suffix()
            )))
            .flex()
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .items_center()
            .overflow_x_scroll()
            .track_scroll(&scroll_handle)
            .font_family(UI_MONOSPACE_FONT_FAMILY)
            .text_sm();
        for (index, (label, target)) in remote_breadcrumbs(path).into_iter().enumerate() {
            if index > 0 {
                breadcrumbs = breadcrumbs.child(
                    div()
                        .flex_none()
                        .px_1()
                        .text_color(self.theme.text_faint)
                        .child("/"),
                );
            }
            breadcrumbs = breadcrumbs.child(
                div()
                    .id(SharedString::from(format!(
                        "sftp-breadcrumb-{}-{index}",
                        placement.element_suffix()
                    )))
                    .flex_none()
                    .max_w(px(120.0))
                    .truncate()
                    .px_1()
                    .py(px(2.0))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .active(move |this| this.bg(pressed))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_sftp_directory(session_id, placement, target.clone(), cx);
                    })),
            );
        }
        breadcrumbs
    }

    fn render_sftp_entry_rows(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tree = placement == SftpBrowserPlacement::Sidebar;
        let Some((tree_rows, selected_paths, connected, loading)) =
            self.session(session_id).map(|session| {
                let browser = session.sftp_browser(placement);
                let tree_rows = browser
                    .visible_rows(tree)
                    .get(range)
                    .map_or_else(Vec::new, <[SftpTreeRow]>::to_vec);
                (
                    tree_rows,
                    browser.selected_paths.clone(),
                    session.connection_state == SessionState::Connected,
                    browser.loading,
                )
            })
        else {
            return Vec::new();
        };
        let list_hover = self.theme.list_hover_bg;
        let pressed = self.theme.control_pressed_bg;
        let selected_background = self.theme.list_selected_bg;
        let element_suffix = placement.element_suffix();
        let mut rows = Vec::with_capacity(tree_rows.len());

        for tree_row in tree_rows {
            let entry = tree_row.entry;
            let is_directory = entry.kind == RemoteFileKind::Directory;
            let is_file = entry.kind == RemoteFileKind::File;
            let entry_path = entry.path.clone();
            let context_entry_path = entry_path.clone();
            let is_selected = selected_paths.contains(&entry_path);
            let is_expanded = self.session(session_id).is_some_and(|session| {
                session
                    .sftp_browser(placement)
                    .expanded_paths
                    .contains(&entry_path)
            });
            let icon_name = if is_directory {
                IconName::Folder
            } else {
                IconName::File
            };
            let size = if is_directory {
                "-".into()
            } else {
                entry
                    .size
                    .map(format_remote_size)
                    .unwrap_or_else(|| "-".into())
            };
            let disclosure = if tree && is_directory {
                let disclosure_path = entry_path.clone();
                div()
                    .id(SharedString::from(format!(
                        "sftp-disclosure-{element_suffix}-{}",
                        entry.path
                    )))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(20.0))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(list_hover))
                    .child(icon(
                        if is_expanded {
                            IconName::Collapse
                        } else {
                            IconName::Expand
                        },
                        self.theme,
                        IconTone::Default,
                        13.0,
                    ))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if !event.standard_click() || event.click_count() != 1 {
                            return;
                        }
                        cx.stop_propagation();
                        this.toggle_remote_tree_directory(
                            session_id,
                            placement,
                            disclosure_path.clone(),
                            cx,
                        );
                    }))
                    .into_any_element()
            } else if tree {
                div().flex_none().size(px(20.0)).into_any_element()
            } else {
                div().flex_none().w(px(0.0)).into_any_element()
            };
            let row_hover = if is_selected {
                selected_background
            } else {
                list_hover
            };
            let mut row = div()
                .id(SharedString::from(format!(
                    "sftp-entry-{element_suffix}-{}",
                    entry.path
                )))
                .flex()
                .flex_none()
                .w_full()
                .min_w(px(0.0))
                .items_center()
                .gap_2()
                .h(px(36.0))
                .pl(px(8.0 + tree_row.depth as f32 * 16.0))
                .pr_3()
                .border_b_1()
                .border_color(self.theme.border)
                .bg(if is_selected {
                    selected_background
                } else {
                    self.theme.transparent
                })
                .child(disclosure)
                .child(self.render_sidebar_icon(icon_name, 16.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_sm()
                        .child(entry.name),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(88.0))
                        .text_right()
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(size),
                );
            if is_directory || is_file {
                row = row
                    .cursor_pointer()
                    .hover(move |this| this.bg(row_hover))
                    .active(move |this| this.bg(pressed))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(session) = this.session_mut(session_id) {
                                session
                                    .sftp_browser_mut(placement)
                                    .select_for_context_menu(&context_entry_path);
                            }
                            this.sftp_context_menu = Some(SftpContextMenu {
                                session_id,
                                placement,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        if !event.standard_click() {
                            return;
                        }
                        if let Some(session) = this.session_mut(session_id) {
                            session.sftp_browser_mut(placement).select_path(
                                &entry_path,
                                event.modifiers(),
                                tree,
                            );
                        }
                        if event.click_count() >= 2 && connected && !loading {
                            if is_directory {
                                if tree {
                                    this.request_sftp_directory(
                                        session_id,
                                        placement,
                                        entry_path.clone(),
                                        cx,
                                    );
                                } else {
                                    this.open_remote_directory(placement, entry_path.clone(), cx);
                                }
                            } else {
                                this.open_remote_file(entry_path.clone(), true, cx);
                            }
                        } else {
                            cx.notify();
                        }
                    }));
            }
            rows.push(row.into_any_element());
        }

        rows
    }

    fn render_sftp_transfer_queue(
        &self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (tasks, batch_progress) = self
            .session(session_id)
            .map(|session| {
                (
                    session.transfers.tasks.clone(),
                    session
                        .transfers
                        .latest_batch_progress(SftpTransferDirection::Download),
                )
            })
            .unwrap_or_default();
        if tasks.is_empty() {
            return div().into_any_element();
        }
        let has_finished = tasks.iter().any(|task| task.state.is_finished());
        let element_suffix = placement.element_suffix();

        let mut clear_button = self.render_icon_button(
            SharedString::from(format!(
                "clear-sftp-transfers-{element_suffix}-{}",
                session_id.0
            )),
            IconName::Delete,
            self.tr("sftp-clear-finished"),
            IconTone::Default,
            has_finished,
        );
        if has_finished {
            clear_button = clear_button.on_click(cx.listener(move |this, _, _, cx| {
                this.clear_finished_sftp_transfers(session_id, cx);
            }));
        }

        let mut queue = div()
            .flex()
            .flex_none()
            .flex_col()
            .max_h(px(220.0))
            .border_t_1()
            .border_color(self.theme.border)
            .bg(self.theme.surface_bg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(px(36.0))
                    .px_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.tr("sftp-transfers")),
                    )
                    .child(clear_button),
            );

        if let Some(progress) = batch_progress {
            let percentage = (progress.fraction * 100.0).round() as u32;
            let title = if progress.settled_count < progress.task_count {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", progress.task_count);
                self.tr_with("sftp-downloading-files", &args)
            } else if progress.failed_count == 0 {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", progress.task_count);
                self.tr_with("sftp-downloaded-files", &args)
            } else {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set("count", progress.failed_count);
                self.tr_with("sftp-downloaded-errors", &args)
            };
            let status = progress.total.map_or_else(
                || {
                    let mut args = fluent_bundle::FluentArgs::new();
                    args.set("settled", progress.settled_count);
                    args.set("total", progress.task_count);
                    args.set("percent", percentage);
                    self.tr_with("sftp-file-progress", &args)
                },
                |total| {
                    let mut args = fluent_bundle::FluentArgs::new();
                    args.set("transferred", format_remote_size(progress.transferred));
                    args.set("total", format_remote_size(total));
                    args.set("percent", percentage);
                    self.tr_with("sftp-byte-progress", &args)
                },
            );
            queue = queue.child(
                div()
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(self.theme.text_muted)
                                    .child(status),
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
                                    .w(gpui::relative(progress.fraction))
                                    .rounded_full()
                                    .bg(self.theme.accent),
                            ),
                    ),
            );
        }

        let mut rows = div()
            .id(SharedString::from(format!(
                "sftp-transfer-rows-{element_suffix}-{}",
                session_id.0
            )))
            .flex()
            .flex_col()
            .overflow_y_scroll();
        for task in tasks {
            let transfer_id = task.id;
            let direction = task.direction;
            let state = task.state;
            let progress = task
                .total
                .filter(|total| *total > 0)
                .map(|total| (task.transferred as f32 / total as f32).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            let status_color = match state {
                SftpTransferState::Completed => self.theme.status_ok,
                SftpTransferState::Failed => self.theme.error_text,
                SftpTransferState::Conflict => self.theme.status_warn,
                SftpTransferState::Queued
                | SftpTransferState::Running
                | SftpTransferState::Cancelling
                | SftpTransferState::Cancelled => self.theme.text_muted,
            };
            let mut controls = div().flex().flex_none().items_center().gap_1();
            match state {
                SftpTransferState::Conflict => {
                    controls = controls
                        .child(
                            text_button(
                                SharedString::from(format!(
                                    "replace-sftp-transfer-{element_suffix}-{transfer_id}"
                                )),
                                self.tr("common-replace"),
                                TextButtonTone::Primary,
                                true,
                                &self.theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.replace_sftp_transfer_destination(
                                        session_id,
                                        transfer_id,
                                        cx,
                                    );
                                },
                            )),
                        )
                        .child(
                            text_button(
                                SharedString::from(format!(
                                    "cancel-conflicted-sftp-transfer-{element_suffix}-{transfer_id}"
                                )),
                                self.tr("common-cancel"),
                                TextButtonTone::Secondary,
                                true,
                                &self.theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.cancel_sftp_transfer(session_id, transfer_id, cx);
                                },
                            )),
                        );
                }
                SftpTransferState::Queued | SftpTransferState::Running => {
                    controls = controls.child(
                        self.render_icon_button(
                            SharedString::from(format!(
                                "cancel-active-sftp-transfer-{element_suffix}-{transfer_id}"
                            )),
                            IconName::Cancel,
                            self.tr("sftp-cancel-transfer"),
                            IconTone::Default,
                            true,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.cancel_sftp_transfer(session_id, transfer_id, cx);
                        })),
                    );
                }
                SftpTransferState::Cancelling
                | SftpTransferState::Completed
                | SftpTransferState::Failed
                | SftpTransferState::Cancelled => {}
            }

            let row = div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(self.theme.border)
                .child(self.render_sidebar_icon(
                    match direction {
                        SftpTransferDirection::Upload => IconName::Upload,
                        SftpTransferDirection::Download => IconName::Download,
                    },
                    16.0,
                ))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(task.display_name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(status_color)
                                .child(task.status_text(&self.localizer)),
                        )
                        .when(
                            matches!(
                                state,
                                SftpTransferState::Running | SftpTransferState::Cancelling
                            ),
                            |this| {
                                this.child(
                                    div()
                                        .h(px(3.0))
                                        .w_full()
                                        .overflow_hidden()
                                        .rounded_full()
                                        .bg(self.theme.control_bg)
                                        .child(
                                            div()
                                                .h_full()
                                                .w(gpui::relative(progress))
                                                .rounded_full()
                                                .bg(self.theme.accent),
                                        ),
                                )
                            },
                        ),
                )
                .child(controls);
            rows = rows.child(row);
        }

        queue = queue.child(rows);
        queue.into_any_element()
    }

    fn render_sftp_file(&self, session_id: SessionId, cx: &mut Context<Self>) -> AnyElement {
        let Some(session) = self.session(session_id) else {
            return div().into_any_element();
        };
        let Some(file) = session.sftp.file.as_ref() else {
            return div().into_any_element();
        };
        let path = file.path.clone();
        let editor = file.editor.clone();
        let loading = file.loading;
        let saving = file.saving;
        let editable = file.editable;
        let error = file.error.clone();
        let binary = !loading && error.is_none() && file.text_format.is_none();
        let dirty = file.is_dirty(cx);
        let connected = session.connection_state == SessionState::Connected;
        let size = file.original_contents.len() as u64;

        let mut back_button = self.render_icon_button(
            "sftp_close_file",
            IconName::ArrowLeft,
            self.tr("sftp-back-directory"),
            IconTone::Default,
            !saving,
        );
        if !saving {
            back_button = back_button.on_click(cx.listener(move |this, _, _, cx| {
                this.close_remote_file(session_id, cx);
            }));
        }

        let mut revert_button = text_button(
            "sftp_revert_file",
            self.tr("common-revert"),
            TextButtonTone::Secondary,
            dirty && !saving,
            &self.theme,
        );
        if dirty && !saving {
            revert_button = revert_button.on_click(cx.listener(move |this, _, _, cx| {
                this.revert_remote_file(session_id, cx);
            }));
        }

        let can_save = editable && dirty && !saving && connected;
        let mut save_button = text_button(
            "sftp_save_file",
            if saving {
                self.tr("common-saving")
            } else {
                self.tr("common-save")
            },
            TextButtonTone::Primary,
            can_save,
            &self.theme,
        );
        if can_save {
            save_button = save_button.on_click(cx.listener(move |this, _, _, cx| {
                this.save_remote_file(session_id, cx);
            }));
        }

        let mut content = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0));
        if loading {
            content = content
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.tr("sftp-loading-file"));
        } else if binary {
            content = content
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.render_sidebar_icon(IconName::File, 20.0))
                .child(self.tr("sftp-binary-read-only"))
                .child(format_remote_size(size));
        } else if let Some(editor) = editor {
            content = content.child(editor);
        }

        div()
            .id("sftp_file")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .mt_4()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.panel_bg)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .h(px(40.0))
                    .px_2()
                    .border_b_1()
                    .border_color(self.theme.border)
                    .child(back_button)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .ml_2()
                            .truncate()
                            .font_family(UI_MONOSPACE_FONT_FAMILY)
                            .text_sm()
                            .child(path),
                    )
                    .when(dirty, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .mr_2()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("sftp-modified")),
                        )
                    })
                    .when(!editable, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .mr_2()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child(self.tr("sftp-read-only")),
                        )
                    })
                    .when(editable, |this| {
                        this.child(revert_button).child(save_button)
                    }),
            )
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(self.theme.border)
                        .bg(self.theme.control_bg)
                        .text_sm()
                        .text_color(self.theme.error_text)
                        .child(error),
                )
            })
            .child(content)
            .child(self.render_sftp_transfer_queue(session_id, SftpBrowserPlacement::Center, cx))
            .into_any_element()
    }

    fn render_profile_editor_overlay(&self, cx: &mut Context<Self>) -> AnyElement {
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

    fn render_connection_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn connection_status_text(&self) -> String {
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

    fn render_auth_method_row(
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

    fn render_private_key_row(
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

    fn render_saved_credential_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    fn render_form_row(&self, label: SharedString, field: Entity<TextField>) -> impl IntoElement {
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

    fn render_route_editor(
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

    fn render_jump_host_editor(
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

impl EntityInputHandler for RemCmdApp {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked_text = &self.terminal_input_session()?.terminal_marked_text;
        let utf16_len = marked_text.encode_utf16().count();
        let start_utf16 = range_utf16.start.min(utf16_len);
        let end_utf16 = range_utf16.end.clamp(start_utf16, utf16_len);
        let start = utf16_offset_to_utf8(marked_text, start_utf16);
        let end = utf16_offset_to_utf8(marked_text, end_utf16);
        let adjusted_start = marked_text[..start].encode_utf16().count();
        let adjusted_end = marked_text[..end].encode_utf16().count();

        adjusted_range.replace(adjusted_start..adjusted_end);
        Some(marked_text[start..end].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self
            .terminal_input_session()
            .map(|session| session.terminal_marked_text.encode_utf16().count())
            .unwrap_or_default();
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let len = self
            .terminal_input_session()
            .map(|session| session.terminal_marked_text.encode_utf16().count())
            .unwrap_or_default();
        (len != 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session_id) = self.terminal_input_session_id() else {
            return;
        };
        let text = self
            .session_mut(session_id)
            .map(|session| std::mem::take(&mut session.terminal_marked_text))
            .unwrap_or_default();
        self.send_terminal_user_input(session_id, text.into_bytes(), cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.terminal_input_session_id() else {
            return;
        };
        if let Some(session) = self.session_mut(session_id) {
            session.terminal_marked_text.clear();
        }
        self.send_terminal_user_input(session_id, new_text.as_bytes().to_vec(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.terminal_input_session_mut() {
            new_text.clone_into(&mut session.terminal_marked_text);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let terminal = self.terminal_input_session()?.terminal.as_ref()?;
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
        Some(
            self.terminal_input_session()
                .map(|session| session.terminal_marked_text.encode_utf16().count())
                .unwrap_or_default(),
        )
    }
}

fn auth_method_with_secret(prompt_kind: CredentialPromptKind, secret: SecretString) -> AuthMethod {
    match prompt_kind {
        CredentialPromptKind::Password => AuthMethod::Password { password: secret },
        CredentialPromptKind::PrivateKeyPassphrase { path } => AuthMethod::PrivateKey {
            path,
            passphrase: Some(secret),
        },
        CredentialPromptKind::ProxyPassword => {
            unreachable!("proxy passwords are applied to RuntimeProxy")
        }
    }
}

fn runtime_proxy_with_password(
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

const fn openssh_status_key(status: OpenSshImportStatus) -> &'static str {
    match status {
        OpenSshImportStatus::New => "import-status-new",
        OpenSshImportStatus::Update => "import-status-update",
        OpenSshImportStatus::Unchanged => "import-status-unchanged",
        OpenSshImportStatus::Conflict => "import-status-conflict",
        OpenSshImportStatus::Invalid => "import-status-invalid",
    }
}

fn include_openssh_dependencies(
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

fn profile_auth_label(auth: &AuthConfig, localizer: &Localizer) -> String {
    localizer.text(match auth {
        AuthConfig::None => "common-none",
        AuthConfig::Password => "credential-password",
        AuthConfig::PrivateKey { .. } => "profile-auth-private-key",
        AuthConfig::Agent => "profile-auth-agent",
    })
}

const fn profile_auth_kind_key(kind: ProfileAuthKind) -> &'static str {
    match kind {
        ProfileAuthKind::None => "profile-auth-none",
        ProfileAuthKind::Password => "profile-auth-password",
        ProfileAuthKind::PrivateKey => "profile-auth-private-key",
        ProfileAuthKind::Agent => "profile-auth-agent",
    }
}

fn connection_stage_label(stage: &ConnectionStage, localizer: &Localizer) -> String {
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

fn localized_connection_error(error: &SshError, localizer: &Localizer) -> String {
    let (summary_key, suggestion_key) = match error.kind() {
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
    let headline = error.stage().map_or_else(
        || summary.clone(),
        |stage| format!("{}: {summary}", connection_stage_label(stage, localizer)),
    );
    format!(
        "{headline}\n{}\n{}: {}",
        localizer.text(suggestion_key),
        localizer.text("connection-technical-details"),
        error.message()
    )
}

const fn session_state_key(state: SessionState) -> &'static str {
    match state {
        SessionState::Disconnected => "connection-status-disconnected",
        SessionState::Connecting => "connection-status-connecting",
        SessionState::Authenticating => "connection-status-authenticating",
        SessionState::Connected => "connection-status-connected",
        SessionState::Disconnecting => "connection-status-disconnecting",
        SessionState::Failed => "connection-status-failed",
    }
}

fn credentials_invalidated_by_edit(
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

fn rejected_credential_kind(
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

fn clamp_sidebar_width(requested: f32, viewport_width: f32) -> f32 {
    let available_width = (viewport_width - MIN_DETAIL_PANEL_WIDTH - SIDEBAR_RESIZE_HANDLE_WIDTH)
        .clamp(0.0, SIDEBAR_MAX_WIDTH);

    if available_width < SIDEBAR_MIN_WIDTH {
        available_width
    } else {
        requested.clamp(SIDEBAR_MIN_WIDTH, available_width)
    }
}

fn select_menu_height(option_count: usize) -> f32 {
    option_count.clamp(1, SELECT_MENU_MAX_VISIBLE_ROWS) as f32 * SELECT_MENU_ROW_HEIGHT + 8.0
}

fn select_menu_scroll_offset(selected_index: usize, option_count: usize) -> f32 {
    let visible_rows = option_count.min(SELECT_MENU_MAX_VISIBLE_ROWS);
    let first_visible = selected_index
        .saturating_sub(visible_rows / 2)
        .min(option_count.saturating_sub(visible_rows));
    first_visible as f32 * SELECT_MENU_ROW_HEIGHT
}

const fn windows_menu_button_width(menu: WindowsMenu) -> f32 {
    match menu {
        WindowsMenu::File | WindowsMenu::Edit | WindowsMenu::Help => 43.0,
        WindowsMenu::Terminal => 66.0,
        WindowsMenu::View => 45.0,
        WindowsMenu::Window => 64.0,
    }
}

fn windows_menu_left(menu: WindowsMenu) -> f32 {
    let menus = [
        WindowsMenu::File,
        WindowsMenu::Edit,
        WindowsMenu::Terminal,
        WindowsMenu::View,
        WindowsMenu::Window,
        WindowsMenu::Help,
    ];
    WINDOWS_BRAND_WIDTH
        + menus
            .into_iter()
            .take_while(|candidate| *candidate != menu)
            .map(windows_menu_button_width)
            .sum::<f32>()
}

fn windows_menu_popup_width(entries: &[WindowsMenuEntry], localizer: &Localizer) -> f32 {
    entries
        .iter()
        .filter_map(|entry| match entry {
            WindowsMenuEntry::Item {
                label, shortcut, ..
            } => Some(
                28.0 + estimated_windows_menu_text_width(&windows_menu_label(localizer, label))
                    + if shortcut.is_empty() {
                        0.0
                    } else {
                        20.0 + estimated_windows_menu_text_width(shortcut)
                    },
            ),
            WindowsMenuEntry::Separator => None,
        })
        .fold(WINDOWS_MENU_MIN_WIDTH, f32::max)
        .ceil()
}

fn windows_menu_label(localizer: &Localizer, label: &str) -> String {
    let key = match label {
        "New Connection" => "menu-new-connection",
        "New Local Terminal" => "menu-new-local-terminal",
        "New SSH Terminal" => "menu-new-remote-terminal",
        "Connect Selected Server" => "menu-connect",
        "Disconnect Active Session" => "menu-disconnect",
        "Settings" => "menu-settings",
        "Exit" => "menu-exit",
        "Undo" => "menu-undo",
        "Redo" => "menu-redo",
        "Cut" => "menu-cut",
        "Copy" => "menu-copy",
        "Paste" => "menu-paste",
        "Select All" => "menu-select-all",
        "Split Horizontally" => "menu-split-horizontal",
        "Split Vertically" => "menu-split-vertical",
        "Show Terminal" => "menu-show-terminal",
        "Show Remote Files" => "menu-show-remote-files",
        "Reset Terminal" => "menu-reset-terminal",
        "Close Active Split" => "menu-close-active-split",
        "Close Active Tab" => "menu-close-active-tab",
        "Home" => "menu-home",
        "Toggle Connections Sidebar" => "menu-toggle-connections-sidebar",
        "Search Connections" => "menu-search-connections",
        "Show Remote Files Sidebar" => "menu-show-remote-files-sidebar",
        "Show Server Performance" => "menu-show-server-performance",
        "Toggle Bottom Terminal" => "menu-toggle-bottom-terminal",
        "Minimize" => "menu-minimize",
        "Maximize or Restore" => "menu-maximize-restore",
        "Toggle Full Screen" => "menu-fullscreen",
        "Close Window" => "menu-close-window",
        "About RemCmd" => "about-title",
        _ => return label.to_owned(),
    };
    localizer.text(key)
}

fn estimated_windows_menu_text_width(text: &str) -> f32 {
    text.chars()
        .map(|character| if character.is_ascii() { 7.25 } else { 12.0 })
        .sum()
}

fn windows_menu_entries(menu: WindowsMenu) -> Vec<WindowsMenuEntry> {
    use WindowsMenuCommand as Command;
    use WindowsMenuEntry::{Item, Separator};

    match menu {
        WindowsMenu::File => vec![
            Item {
                label: "New Connection",
                shortcut: "Ctrl+N",
                command: Command::NewConnection,
            },
            Item {
                label: "New Local Terminal",
                shortcut: "Ctrl+T",
                command: Command::NewLocalTerminal,
            },
            Item {
                label: "New SSH Terminal",
                shortcut: "Ctrl+Shift+T",
                command: Command::NewRemoteTerminal,
            },
            Separator,
            Item {
                label: "Connect Selected Server",
                shortcut: "Ctrl+Enter",
                command: Command::ConnectSelectedProfile,
            },
            Item {
                label: "Disconnect Active Session",
                shortcut: "Ctrl+Shift+X",
                command: Command::DisconnectActiveSession,
            },
            Separator,
            Item {
                label: "Settings",
                shortcut: "Ctrl+,",
                command: Command::ShowSettings,
            },
            Item {
                label: "Exit",
                shortcut: "Ctrl+Q",
                command: Command::Quit,
            },
        ],
        WindowsMenu::Edit => vec![
            Item {
                label: "Undo",
                shortcut: "Ctrl+Z",
                command: Command::Edit(EditCommand::Undo),
            },
            Item {
                label: "Redo",
                shortcut: "Ctrl+Y",
                command: Command::Edit(EditCommand::Redo),
            },
            Separator,
            Item {
                label: "Cut",
                shortcut: "Ctrl+X",
                command: Command::Edit(EditCommand::Cut),
            },
            Item {
                label: "Copy",
                shortcut: "Ctrl+C",
                command: Command::Edit(EditCommand::Copy),
            },
            Item {
                label: "Paste",
                shortcut: "Ctrl+V",
                command: Command::Edit(EditCommand::Paste),
            },
            Item {
                label: "Select All",
                shortcut: "Ctrl+A",
                command: Command::Edit(EditCommand::SelectAll),
            },
        ],
        WindowsMenu::Terminal => vec![
            Item {
                label: "Split Horizontally",
                shortcut: "Ctrl+D",
                command: Command::SplitHorizontal,
            },
            Item {
                label: "Split Vertically",
                shortcut: "Ctrl+Shift+D",
                command: Command::SplitVertical,
            },
            Separator,
            Item {
                label: "Show Terminal",
                shortcut: "Ctrl+1",
                command: Command::ShowTerminalView,
            },
            Item {
                label: "Show Remote Files",
                shortcut: "Ctrl+2",
                command: Command::ShowFilesView,
            },
            Separator,
            Item {
                label: "Reset Terminal",
                shortcut: "Ctrl+R",
                command: Command::ResetActiveTerminal,
            },
            Item {
                label: "Close Active Split",
                shortcut: "Ctrl+Alt+W",
                command: Command::CloseActivePane,
            },
            Item {
                label: "Close Active Tab",
                shortcut: "Ctrl+Shift+W",
                command: Command::CloseActiveTab,
            },
        ],
        WindowsMenu::View => vec![
            Item {
                label: "Home",
                shortcut: "Ctrl+Shift+H",
                command: Command::ShowHome,
            },
            Separator,
            Item {
                label: "Toggle Connections Sidebar",
                shortcut: "Ctrl+Shift+S",
                command: Command::ToggleLeftSidebar,
            },
            Item {
                label: "Search Connections",
                shortcut: "Ctrl+F",
                command: Command::ToggleConnectionSearch,
            },
            Separator,
            Item {
                label: "Show Remote Files Sidebar",
                shortcut: "Ctrl+Shift+F",
                command: Command::ShowSftpSidebar,
            },
            Item {
                label: "Show Server Performance",
                shortcut: "Ctrl+Shift+P",
                command: Command::ShowPerformanceSidebar,
            },
            Item {
                label: "Toggle Bottom Terminal",
                shortcut: "Ctrl+J",
                command: Command::ToggleBottomPanel,
            },
        ],
        WindowsMenu::Window => vec![
            Item {
                label: "Minimize",
                shortcut: "",
                command: Command::MinimizeWindow,
            },
            Item {
                label: "Maximize or Restore",
                shortcut: "",
                command: Command::ZoomWindow,
            },
            Item {
                label: "Toggle Full Screen",
                shortcut: "Ctrl+Alt+F",
                command: Command::ToggleFullscreen,
            },
            Separator,
            Item {
                label: "Close Window",
                shortcut: "Ctrl+W",
                command: Command::CloseWindow,
            },
        ],
        WindowsMenu::Help => vec![Item {
            label: "About RemCmd",
            shortcut: "",
            command: Command::ShowAbout,
        }],
    }
}

fn clamp_right_sidebar_width(requested: f32, viewport_width: f32, left_sidebar_width: f32) -> f32 {
    let available_width = (viewport_width
        - left_sidebar_width
        - MIN_DETAIL_PANEL_WIDTH
        - SIDEBAR_RESIZE_HANDLE_WIDTH)
        .clamp(0.0, RIGHT_SIDEBAR_MAX_WIDTH);

    if available_width < RIGHT_SIDEBAR_MIN_WIDTH {
        available_width
    } else {
        requested.clamp(RIGHT_SIDEBAR_MIN_WIDTH, available_width)
    }
}

fn clamp_bottom_panel_height(requested: f32, viewport_height: f32) -> f32 {
    let available_height = (viewport_height - content_top_inset() - 100.0)
        .clamp(BOTTOM_PANEL_MIN_HEIGHT, BOTTOM_PANEL_MAX_HEIGHT);
    requested.clamp(BOTTOM_PANEL_MIN_HEIGHT, available_height)
}

fn sftp_browser_placement_for_request(request_id: u64) -> SftpBrowserPlacement {
    if request_id >= SIDEBAR_SFTP_REQUEST_ID_START {
        SftpBrowserPlacement::Sidebar
    } else {
        SftpBrowserPlacement::Center
    }
}

fn quick_command_target_sessions(
    profile_ids: &[String],
    selected_profile_ids: &HashSet<String>,
    sessions: &[(SessionId, &str, bool)],
    active_session_id: Option<SessionId>,
) -> Vec<(String, SessionId)> {
    let mut seen_profile_ids = HashSet::new();
    profile_ids
        .iter()
        .filter(|profile_id| {
            selected_profile_ids.contains(*profile_id)
                && seen_profile_ids.insert((*profile_id).clone())
        })
        .filter_map(|profile_id| {
            let active = active_session_id.and_then(|active_session_id| {
                sessions
                    .iter()
                    .find(|(session_id, session_profile_id, available)| {
                        *session_id == active_session_id
                            && *session_profile_id == profile_id
                            && *available
                    })
            });
            active
                .or_else(|| {
                    sessions
                        .iter()
                        .rev()
                        .find(|(_, session_profile_id, available)| {
                            *session_profile_id == profile_id && *available
                        })
                })
                .map(|(session_id, _, _)| (profile_id.clone(), *session_id))
        })
        .collect()
}

fn estimated_titlebar_label_width(label: &str) -> f32 {
    label
        .chars()
        .map(|character| if character.is_ascii() { 8.5 } else { 14.5 })
        .sum::<f32>()
        .max(20.0)
}

fn workspace_tab_title(
    server_name: &str,
    view: TerminalTabView,
    terminal_number: usize,
    sftp_path: Option<&str>,
    remote_cwd: Option<&str>,
    localizer: &Localizer,
) -> String {
    let path = match view {
        TerminalTabView::Terminal => remote_cwd.map(str::to_owned).unwrap_or_else(|| {
            let mut args = fluent_bundle::FluentArgs::new();
            args.set("number", terminal_number);
            localizer.text_with("terminal-number", Some(&args))
        }),
        TerminalTabView::Files => sftp_path
            .or(remote_cwd)
            .map(str::to_owned)
            .unwrap_or_else(|| localizer.text("terminal-files")),
    };
    format!("{server_name} - {path}")
}

fn remote_parent_path(path: &str) -> Option<String> {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        return None;
    }

    match path.rfind('/') {
        Some(0) => Some("/".into()).filter(|_| path != "/"),
        Some(separator) => Some(path[..separator].into()),
        None => Some(".".into()),
    }
}

fn remote_file_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("download")
}

fn remote_join_path(directory: &str, name: &str) -> String {
    if directory == "/" {
        format!("/{name}")
    } else if directory == "." {
        name.to_owned()
    } else {
        format!("{}/{}", directory.trim_end_matches('/'), name)
    }
}

struct LocalUploadPlan {
    directories: Vec<String>,
    files: Vec<(PathBuf, String)>,
}

fn build_local_upload_plan(
    selected_paths: &[PathBuf],
    remote_directory: &str,
) -> std::io::Result<LocalUploadPlan> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    let mut pending = Vec::new();

    for path in selected_paths {
        let Some(name) = path.file_name() else {
            continue;
        };
        let remote_path = remote_join_path(remote_directory, name.to_string_lossy().as_ref());
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            directories.push(remote_path.clone());
            pending.push((path.clone(), remote_path));
        } else if metadata.is_file() {
            files.push((path.clone(), remote_path));
        }
    }

    while let Some((local_directory, remote_directory)) = pending.pop() {
        let mut entries = std::fs::read_dir(&local_directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let local_path = entry.path();
            let metadata = std::fs::symlink_metadata(&local_path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let remote_path = remote_join_path(
                &remote_directory,
                entry.file_name().to_string_lossy().as_ref(),
            );
            if metadata.is_dir() {
                directories.push(remote_path.clone());
                pending.push((local_path, remote_path));
            } else if metadata.is_file() {
                files.push((local_path, remote_path));
            }
        }
    }

    directories.sort_by(|left, right| {
        remote_path_depth(left)
            .cmp(&remote_path_depth(right))
            .then_with(|| left.cmp(right))
    });
    directories.dedup();
    files.sort_by(|left, right| left.1.cmp(&right.1));
    files.dedup_by(|left, right| left.1 == right.1);
    Ok(LocalUploadPlan { directories, files })
}

fn build_remote_download_plan(
    tree: RemoteDirectoryTree,
    destination: PathBuf,
) -> std::io::Result<Vec<(PathBuf, String, Option<u64>)>> {
    std::fs::create_dir_all(&destination)?;
    for directory in tree.directories {
        let relative = remote_relative_path(&tree.root, &directory).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "remote directory escaped its requested root",
            )
        })?;
        std::fs::create_dir_all(join_remote_relative(&destination, relative))?;
    }

    tree.files
        .into_iter()
        .map(|file| {
            let relative = remote_relative_path(&tree.root, &file.path).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "remote file escaped its requested root",
                )
            })?;
            let local_path = join_remote_relative(&destination, relative);
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok((local_path, file.path, file.size))
        })
        .collect()
}

fn remote_relative_path<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    if path == root {
        return Some("");
    }
    path.strip_prefix(root.trim_end_matches('/'))?
        .strip_prefix('/')
}

fn join_remote_relative(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|component| !component.is_empty() && *component != "." && *component != "..")
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

fn collapse_nested_remote_entries(mut entries: Vec<RemoteFileEntry>) -> Vec<RemoteFileEntry> {
    entries.sort_by(|left, right| {
        remote_path_depth(&left.path)
            .cmp(&remote_path_depth(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    entries.dedup_by(|left, right| left.path == right.path);
    let selected_directories = entries
        .iter()
        .filter(|entry| entry.kind == RemoteFileKind::Directory)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    entries.retain(|entry| {
        !selected_directories.iter().any(|directory| {
            directory != &entry.path && remote_path_is_descendant(directory, &entry.path)
        })
    });
    entries
}

fn remote_path_is_descendant(parent: &str, candidate: &str) -> bool {
    if parent == candidate {
        return false;
    }
    if parent == "/" {
        return candidate.starts_with('/') && candidate.len() > 1;
    }
    candidate
        .strip_prefix(parent.trim_end_matches('/'))
        .is_some_and(|suffix| suffix.starts_with('/'))
}

fn remote_path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|component| !component.is_empty())
        .count()
}

fn remote_breadcrumbs(path: &str) -> Vec<(String, String)> {
    if !path.starts_with('/') {
        return vec![(path.to_owned(), path.to_owned())];
    }
    let mut breadcrumbs = vec![("/".into(), "/".into())];
    let mut target = String::new();
    for component in path.split('/').filter(|component| !component.is_empty()) {
        target.push('/');
        target.push_str(component);
        breadcrumbs.push((component.to_owned(), target.clone()));
    }
    breadcrumbs
}

fn format_remote_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let bytes = bytes as f64;

    if bytes >= GIB {
        format!("{:.1} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
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

fn titlebar_active_tab_basis(track_width: f32, tab_count: usize, expanded_width: f32) -> f32 {
    if tab_count <= 1 {
        return 0.0;
    }

    let separator_width = tab_count.saturating_sub(1) as f32;
    let available_width = (track_width - 6.0 - separator_width).max(0.0);
    let equal_width = available_width / tab_count as f32;
    let required_growth = (expanded_width - equal_width).max(0.0) * tab_count as f32
        / tab_count.saturating_sub(1) as f32;
    required_growth.max(TITLEBAR_ACTIVE_TAB_GROWTH)
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

fn local_pty_size(size: PtySize) -> LocalPtySize {
    LocalPtySize::new(size.columns, size.rows).with_pixels(size.pixel_width, size.pixel_height)
}

fn ssh_pty_size(size: LocalPtySize) -> PtySize {
    PtySize::new(size.columns, size.rows).with_pixels(size.pixel_width, size.pixel_height)
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

fn full_terminal_selection(rows: usize, columns: usize) -> Option<TerminalSelection> {
    (rows > 0 && columns > 0).then(|| {
        TerminalSelection::new(
            TerminalPoint::new(0, 0),
            TerminalPoint::new(rows - 1, columns),
        )
    })
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

fn normalize_terminal_font_families(mut families: Vec<String>) -> Vec<SharedString> {
    families.retain(|family| {
        let family = family.trim();
        !family.is_empty() && !family.starts_with('.')
    });
    families.sort_by_cached_key(|family| family.to_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    families.into_iter().map(SharedString::from).collect()
}

fn resolve_terminal_font_family(
    preferred: Option<&str>,
    available: &[SharedString],
) -> SharedString {
    let find_available = |candidate: &str| {
        available
            .iter()
            .find(|family| family.as_ref().eq_ignore_ascii_case(candidate))
            .cloned()
    };

    if let Some(preferred) = preferred
        && let Some(family) = find_available(preferred)
    {
        return family;
    }

    for fallback in ["SF Mono", "Menlo", UI_MONOSPACE_FONT_FAMILY] {
        if let Some(family) = find_available(fallback) {
            return family;
        }
    }

    available
        .first()
        .cloned()
        .unwrap_or_else(|| SharedString::from("Menlo"))
}

#[cfg(target_os = "macos")]
fn register_macos_sf_mono(cx: &mut App) {
    const FONT_DIRECTORIES: [&str; 2] = [
        "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts",
        "/Applications/Utilities/Terminal.app/Contents/Resources/Fonts",
    ];
    const FONT_FILES: [&str; 12] = [
        "SF-Mono-Regular.otf",
        "SF-Mono-RegularItalic.otf",
        "SF-Mono-Medium.otf",
        "SF-Mono-MediumItalic.otf",
        "SF-Mono-Semibold.otf",
        "SF-Mono-SemiboldItalic.otf",
        "SF-Mono-Bold.otf",
        "SF-Mono-BoldItalic.otf",
        "SF-Mono-Light.otf",
        "SF-Mono-LightItalic.otf",
        "SF-Mono-Heavy.otf",
        "SF-Mono-HeavyItalic.otf",
    ];

    let Some(directory) = FONT_DIRECTORIES
        .iter()
        .map(Path::new)
        .find(|directory| directory.join(FONT_FILES[0]).is_file())
    else {
        return;
    };
    let fonts = FONT_FILES
        .iter()
        .filter_map(|file| std::fs::read(directory.join(file)).ok())
        .map(std::borrow::Cow::Owned)
        .collect::<Vec<_>>();
    if !fonts.is_empty() {
        let _ = cx.text_system().add_fonts(fonts);
    }
}

#[cfg(not(target_os = "macos"))]
fn register_macos_sf_mono(_: &mut App) {}

fn main_window_titlebar() -> TitlebarOptions {
    #[cfg(target_os = "macos")]
    {
        TitlebarOptions {
            appears_transparent: true,
            traffic_light_position: Some(point(
                px(TRAFFIC_LIGHT_INSET_X),
                px(TRAFFIC_LIGHT_INSET_Y),
            )),
            ..Default::default()
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        TitlebarOptions {
            title: Some("RemCmd".into()),
            appears_transparent: cfg!(target_os = "windows"),
            ..Default::default()
        }
    }
}

fn about_window_options(cx: &App, _localizer: &Localizer) -> WindowOptions {
    let window_size = size(px(440.0), px(380.0));
    let titlebar = {
        #[cfg(target_os = "macos")]
        {
            TitlebarOptions {
                appears_transparent: true,
                traffic_light_position: Some(point(px(18.0), px(18.0))),
                ..Default::default()
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            TitlebarOptions {
                title: Some(_localizer.text("about-title").into()),
                ..Default::default()
            }
        }
    };

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        window_min_size: Some(window_size),
        window_background: WindowBackgroundAppearance::Blurred,
        titlebar: Some(titlebar),
        ..Default::default()
    }
}

// Application startup functions stay outside main so startup remains testable and readable.
fn main_window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(720.0), px(480.0))),
        window_background: WindowBackgroundAppearance::Blurred,
        titlebar: Some(main_window_titlebar()),
        ..Default::default()
    }
}

fn open_main_window(cx: &mut App) -> WindowHandle<RemCmdApp> {
    let options = main_window_options(cx);

    cx.open_window(options, |window, cx| {
        cx.new(|cx| RemCmdApp::load(window, cx))
    })
    .expect("failed to open main window")
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

fn bind_settings_selector_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "escape",
        CancelSettingsSelector,
        Some("Settings"),
    )]);
}

fn bind_sftp_create_prompt_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitSftpCreate, Some("SftpCreatePrompt")),
        KeyBinding::new("escape", CancelSftpCreate, Some("SftpCreatePrompt")),
    ]);
}

fn bind_quick_command_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", SubmitQuickCommand, Some("QuickCommandPrompt")),
        KeyBinding::new("escape", CancelQuickCommand, Some("QuickCommandPrompt")),
    ]);
}

fn bind_profile_editor_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-s", SaveProfileEditor, Some("ProfileEditor")),
        KeyBinding::new("escape", CancelProfileEditor, Some("ProfileEditor")),
    ]);
    #[cfg(target_os = "windows")]
    cx.bind_keys([KeyBinding::new(
        "ctrl-s",
        SaveProfileEditor,
        Some("ProfileEditor"),
    )]);
}

fn application_menus(localizer: &Localizer) -> Vec<Menu> {
    let mut application_items = vec![
        MenuItem::action(localizer.text("about-title"), ShowAbout),
        MenuItem::separator(),
        MenuItem::action(localizer.text("menu-settings"), ShowSettings),
    ];
    #[cfg(target_os = "macos")]
    application_items.push(MenuItem::os_submenu(
        localizer.text("menu-services"),
        gpui::SystemMenuType::Services,
    ));
    application_items.extend([
        MenuItem::separator(),
        MenuItem::action(localizer.text("menu-quit"), Quit),
    ]);

    vec![
        Menu {
            name: "RemCmd".into(),
            items: application_items,
        },
        Menu {
            name: localizer.text("menu-file").into(),
            items: vec![
                MenuItem::action(localizer.text("menu-new-connection"), NewConnection),
                MenuItem::action(localizer.text("menu-new-local-terminal"), NewLocalTerminal),
                MenuItem::action(
                    localizer.text("menu-new-remote-terminal"),
                    NewRemoteTerminal,
                ),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-connect"), ConnectSelectedProfile),
                MenuItem::action(localizer.text("menu-disconnect"), DisconnectActiveSession),
            ],
        },
        Menu {
            name: localizer.text("menu-terminal").into(),
            items: vec![
                MenuItem::action(localizer.text("menu-split-horizontal"), SplitHorizontal),
                MenuItem::action(localizer.text("menu-split-vertical"), SplitVertical),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-terminal-view"), ShowTerminalView),
                MenuItem::action(localizer.text("menu-files-view"), ShowFilesView),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-reset-terminal"), ResetActiveTerminal),
                MenuItem::action(localizer.text("menu-close-pane"), CloseActivePane),
                MenuItem::action(localizer.text("menu-close-terminal"), CloseActiveTab),
            ],
        },
        Menu {
            name: localizer.text("menu-view").into(),
            items: vec![
                MenuItem::action(localizer.text("menu-home"), ShowHome),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-toggle-sidebar"), ToggleLeftSidebar),
                MenuItem::action(
                    localizer.text("menu-search-connections"),
                    ToggleConnectionSearch,
                ),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-sftp-sidebar"), ShowSftpSidebar),
                MenuItem::action(
                    localizer.text("menu-performance-sidebar"),
                    ShowPerformanceSidebar,
                ),
                MenuItem::action(localizer.text("menu-bottom-panel"), ToggleBottomPanel),
            ],
        },
        Menu {
            name: localizer.text("menu-window").into(),
            items: vec![
                MenuItem::action(localizer.text("menu-minimize"), MinimizeWindow),
                MenuItem::action(localizer.text("menu-zoom"), ZoomWindow),
                MenuItem::action(localizer.text("menu-fullscreen"), ToggleFullscreen),
                MenuItem::separator(),
                MenuItem::action(localizer.text("menu-close-window"), CloseWindow),
            ],
        },
    ]
}

fn dispatch_main_window_action(
    cx: &mut App,
    action: impl FnOnce(&mut RemCmdApp, &mut Window, &mut Context<RemCmdApp>) + 'static,
) {
    let window = cx.global::<RemCmdMainWindow>().0;
    cx.defer(move |cx| {
        let _ = window.update(cx, action);
    });
    cx.stop_propagation();
}

fn configure_application_menu(cx: &mut App, localizer: &Localizer) {
    cx.bind_keys([
        KeyBinding::new("cmd-,", ShowSettings, None),
        KeyBinding::new("cmd-shift-h", ShowHome, None),
        KeyBinding::new("cmd-n", NewConnection, None),
        KeyBinding::new("cmd-t", NewLocalTerminal, None),
        KeyBinding::new("cmd-shift-t", NewRemoteTerminal, None),
        KeyBinding::new("cmd-enter", ConnectSelectedProfile, None),
        KeyBinding::new("cmd-shift-x", DisconnectActiveSession, None),
        KeyBinding::new("cmd-d", SplitHorizontal, None),
        KeyBinding::new("cmd-shift-d", SplitVertical, None),
        KeyBinding::new("alt-cmd-w", CloseActivePane, None),
        KeyBinding::new("cmd-shift-w", CloseActiveTab, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-r", ResetActiveTerminal, None),
        KeyBinding::new("cmd-1", ShowTerminalView, None),
        KeyBinding::new("cmd-2", ShowFilesView, None),
        KeyBinding::new("cmd-shift-s", ToggleLeftSidebar, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
        KeyBinding::new("cmd-f", ToggleConnectionSearch, None),
        KeyBinding::new("cmd-shift-f", ShowSftpSidebar, None),
        KeyBinding::new("cmd-shift-p", ShowPerformanceSidebar, None),
        KeyBinding::new("cmd-j", ToggleBottomPanel, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    #[cfg(target_os = "windows")]
    cx.bind_keys([
        KeyBinding::new("ctrl-,", ShowSettings, None),
        KeyBinding::new("ctrl-shift-h", ShowHome, None),
        KeyBinding::new("ctrl-n", NewConnection, None),
        KeyBinding::new("ctrl-t", NewLocalTerminal, None),
        KeyBinding::new("ctrl-shift-t", NewRemoteTerminal, None),
        KeyBinding::new("ctrl-enter", ConnectSelectedProfile, None),
        KeyBinding::new("ctrl-shift-x", DisconnectActiveSession, None),
        KeyBinding::new("ctrl-d", SplitHorizontal, None),
        KeyBinding::new("ctrl-shift-d", SplitVertical, None),
        KeyBinding::new("ctrl-alt-w", CloseActivePane, None),
        KeyBinding::new("ctrl-shift-w", CloseActiveTab, None),
        KeyBinding::new("ctrl-w", CloseWindow, None),
        KeyBinding::new("ctrl-r", ResetActiveTerminal, None),
        KeyBinding::new("ctrl-1", ShowTerminalView, None),
        KeyBinding::new("ctrl-2", ShowFilesView, None),
        KeyBinding::new("ctrl-shift-s", ToggleLeftSidebar, None),
        KeyBinding::new("ctrl-m", MinimizeWindow, None),
        KeyBinding::new("ctrl-alt-f", ToggleFullscreen, None),
        KeyBinding::new("ctrl-f", ToggleConnectionSearch, None),
        KeyBinding::new("ctrl-shift-f", ShowSftpSidebar, None),
        KeyBinding::new("ctrl-shift-p", ShowPerformanceSidebar, None),
        KeyBinding::new("ctrl-j", ToggleBottomPanel, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
    cx.on_action(|_: &ShowSettings, cx| {
        dispatch_main_window_action(cx, |this, window, cx| this.show_settings(window, cx));
    });
    cx.on_action(|_: &ShowAbout, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.show_about(cx));
    });
    cx.on_action(|_: &ShowHome, cx| {
        dispatch_main_window_action(cx, |this, window, cx| this.show_home(window, cx));
    });
    cx.on_action(|_: &NewConnection, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.open_new_profile_editor(cx));
    });
    cx.on_action(|_: &NewLocalTerminal, cx| {
        dispatch_main_window_action(cx, |this, window, cx| this.open_local_terminal(window, cx));
    });
    cx.on_action(|_: &NewRemoteTerminal, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.connect_selected_profile_in_new_session(window, cx);
        });
    });
    cx.on_action(|_: &ConnectSelectedProfile, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.connect_selected_profile(window, cx);
        });
    });
    cx.on_action(|_: &DisconnectActiveSession, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.disconnect_active_connection(cx));
    });
    cx.on_action(|_: &SplitHorizontal, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.split_active_pane(SplitAxis::Horizontal, window, cx);
        });
    });
    cx.on_action(|_: &SplitVertical, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.split_active_pane(SplitAxis::Vertical, window, cx);
        });
    });
    cx.on_action(|_: &CloseActivePane, cx| {
        dispatch_main_window_action(cx, |this, window, cx| this.close_active_pane(window, cx));
    });
    cx.on_action(|_: &CloseActiveTab, cx| {
        dispatch_main_window_action(cx, |this, _, cx| {
            if let Some(tab_id) = this.active_tab_id {
                this.close_tab(tab_id, cx);
            }
        });
    });
    cx.on_action(|_: &ResetActiveTerminal, cx| {
        dispatch_main_window_action(cx, |this, _, cx| {
            if let Some(session_id) = this.active_session_id {
                this.reset_terminal(session_id, cx);
            }
        });
    });
    cx.on_action(|_: &ShowTerminalView, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.set_active_tab_view(TerminalTabView::Terminal, window, cx);
        });
    });
    cx.on_action(|_: &ShowFilesView, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.set_active_tab_view(TerminalTabView::Files, window, cx);
        });
    });
    cx.on_action(|_: &ToggleLeftSidebar, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.toggle_left_sidebar(cx));
    });
    cx.on_action(|_: &ToggleConnectionSearch, cx| {
        dispatch_main_window_action(cx, |this, window, cx| {
            this.toggle_sidebar_search(window, cx);
        });
    });
    cx.on_action(|_: &ShowSftpSidebar, cx| {
        dispatch_main_window_action(cx, |this, _, cx| {
            this.set_right_sidebar_view(RightSidebarView::Sftp, cx);
            if !this.right_sidebar_open {
                this.toggle_right_sidebar(cx);
            }
        });
    });
    cx.on_action(|_: &ShowPerformanceSidebar, cx| {
        dispatch_main_window_action(cx, |this, _, cx| {
            this.set_right_sidebar_view(RightSidebarView::Performance, cx);
            if !this.right_sidebar_open {
                this.toggle_right_sidebar(cx);
            }
        });
    });
    cx.on_action(|_: &ToggleBottomPanel, cx| {
        dispatch_main_window_action(cx, |this, window, cx| this.toggle_bottom_panel(window, cx));
    });
    cx.on_action(|_: &SaveProfileEditor, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.save_editor(cx));
    });
    cx.on_action(|_: &CancelProfileEditor, cx| {
        dispatch_main_window_action(cx, |this, _, cx| this.cancel_editor(cx));
    });
    cx.on_action(|_: &MinimizeWindow, cx| {
        dispatch_main_window_action(cx, |_, window, _| window.minimize_window());
    });
    cx.on_action(|_: &ZoomWindow, cx| {
        dispatch_main_window_action(cx, |_, window, _| window.zoom_window());
    });
    cx.on_action(|_: &ToggleFullscreen, cx| {
        dispatch_main_window_action(cx, |_, window, _| window.toggle_fullscreen());
    });
    cx.on_action(|_: &CloseWindow, cx| {
        dispatch_main_window_action(cx, |_, window, _| window.remove_window());
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(application_menus(localizer));
}

fn launch(cx: &mut App) {
    cx.set_global(SshRuntime::new().expect("failed to create SSH runtime"));
    register_macos_sf_mono(cx);

    bind_text_field_keys(cx);
    bind_file_editor_keys(cx);
    bind_credential_prompt_keys(cx);
    bind_host_key_prompt_keys(cx);
    bind_settings_selector_keys(cx);
    bind_sftp_create_prompt_keys(cx);
    bind_quick_command_keys(cx);
    bind_profile_editor_keys(cx);
    let main_window = open_main_window(cx);
    let language_mode = main_window
        .update(cx, |this, _, _| this.language_mode)
        .unwrap_or(LanguageMode::System);
    cx.set_global(RemCmdMainWindow(main_window));
    configure_application_menu(cx, &Localizer::new(language_mode));
    cx.activate(true);
}

fn reopen_main_window(cx: &mut App) {
    let main_window = open_main_window(cx);
    cx.set_global(RemCmdMainWindow(main_window));
    cx.activate(true);
}

fn main() {
    let application = Application::new().with_assets(RemCmdAssets);
    application.on_reopen(reopen_main_window);
    application.run(launch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use remcmd_terminal::DamageRange;

    #[test]
    fn terminal_damage_merge_coalesces_rows_and_preserves_full_repaints() {
        let mut damage = TerminalDamage::Partial(vec![DamageRange {
            row: 3,
            left: 5,
            right: 8,
        }]);

        merge_terminal_damage(
            &mut damage,
            TerminalDamage::Partial(vec![
                DamageRange {
                    row: 3,
                    left: 2,
                    right: 6,
                },
                DamageRange {
                    row: 7,
                    left: 1,
                    right: 4,
                },
            ]),
        );

        assert_eq!(
            damage,
            TerminalDamage::Partial(vec![
                DamageRange {
                    row: 3,
                    left: 2,
                    right: 8,
                },
                DamageRange {
                    row: 7,
                    left: 1,
                    right: 4,
                },
            ])
        );

        merge_terminal_damage(&mut damage, TerminalDamage::Full);
        assert_eq!(damage, TerminalDamage::Full);
    }

    #[test]
    fn terminal_render_snapshot_is_reused_until_screen_damage_arrives() {
        let mut terminal = ActiveTerminal::new("profile".into(), PtySize::default());

        let (first, first_damage) = terminal.snapshot_for_render();
        assert_eq!(first_damage, TerminalDamage::Full);

        let (second, second_damage) = terminal.snapshot_for_render();
        assert!(Rc::ptr_eq(&first, &second));
        assert_eq!(second_damage, TerminalDamage::Partial(Vec::new()));

        terminal.process(b"changed");
        let (third, third_damage) = terminal.snapshot_for_render();
        assert!(!Rc::ptr_eq(&second, &third));
        assert!(matches!(third_damage, TerminalDamage::Partial(ranges) if !ranges.is_empty()));
    }

    #[test]
    fn select_menu_scroll_offset_centers_and_clamps_the_selected_option() {
        assert_eq!(select_menu_scroll_offset(0, 17), 0.0);
        assert_eq!(select_menu_scroll_offset(8, 17), 4.0 * 28.0);
        assert_eq!(select_menu_scroll_offset(16, 17), 8.0 * 28.0);
        assert_eq!(select_menu_scroll_offset(2, 3), 0.0);
    }

    #[test]
    fn application_menu_exposes_workspace_operations() {
        let menus = application_menus(&Localizer::new(LanguageMode::EnUs));
        let menu_names = menus
            .iter()
            .map(|menu| menu.name.to_string())
            .collect::<Vec<_>>();

        assert_eq!(menu_names, ["RemCmd", "File", "Terminal", "View", "Window"]);
        assert!(matches!(
            &menus[0].items[0],
            MenuItem::Action { name, .. } if name.as_ref() == "About RemCmd"
        ));
    }

    #[test]
    fn application_menus_and_widths_rebuild_for_chinese() {
        let localizer = Localizer::new(LanguageMode::ZhCn);
        let menus = application_menus(&localizer);
        assert_eq!(
            menus
                .iter()
                .map(|menu| menu.name.to_string())
                .collect::<Vec<_>>(),
            ["RemCmd", "文件", "终端", "视图", "窗口"]
        );
        assert!(matches!(
            &menus[0].items[0],
            MenuItem::Action { name, .. } if name.as_ref() == "关于 RemCmd"
        ));

        for menu in [
            WindowsMenu::File,
            WindowsMenu::Edit,
            WindowsMenu::Terminal,
            WindowsMenu::View,
            WindowsMenu::Window,
            WindowsMenu::Help,
        ] {
            let entries = windows_menu_entries(menu);
            let width = windows_menu_popup_width(&entries, &localizer);
            for entry in entries {
                let WindowsMenuEntry::Item {
                    label, shortcut, ..
                } = entry
                else {
                    continue;
                };
                let required = 28.0
                    + estimated_windows_menu_text_width(&windows_menu_label(&localizer, label))
                    + if shortcut.is_empty() {
                        0.0
                    } else {
                        20.0 + estimated_windows_menu_text_width(shortcut)
                    };
                assert!(width >= required.ceil());
            }
        }
    }

    #[test]
    fn windows_titlebar_menus_expose_expected_command_groups() {
        let file_entries = windows_menu_entries(WindowsMenu::File);
        let edit_entries = windows_menu_entries(WindowsMenu::Edit);
        let help_entries = windows_menu_entries(WindowsMenu::Help);

        assert!(file_entries.iter().any(|entry| matches!(
            entry,
            WindowsMenuEntry::Item {
                command: WindowsMenuCommand::NewConnection,
                ..
            }
        )));
        assert!(edit_entries.iter().any(|entry| matches!(
            entry,
            WindowsMenuEntry::Item {
                command: WindowsMenuCommand::Edit(EditCommand::Paste),
                ..
            }
        )));
        assert_eq!(
            help_entries,
            vec![WindowsMenuEntry::Item {
                label: "About RemCmd",
                shortcut: "",
                command: WindowsMenuCommand::ShowAbout,
            }]
        );
    }

    #[test]
    fn windows_titlebar_menus_do_not_start_or_end_with_separators() {
        for menu in [
            WindowsMenu::File,
            WindowsMenu::Edit,
            WindowsMenu::Terminal,
            WindowsMenu::View,
            WindowsMenu::Window,
            WindowsMenu::Help,
        ] {
            let entries = windows_menu_entries(menu);
            assert!(!matches!(
                entries.first(),
                Some(WindowsMenuEntry::Separator)
            ));
            assert!(!matches!(entries.last(), Some(WindowsMenuEntry::Separator)));
        }
    }

    #[test]
    fn windows_titlebar_menu_geometry_fits_labels_and_tracks_buttons() {
        let menus = [
            WindowsMenu::File,
            WindowsMenu::Edit,
            WindowsMenu::Terminal,
            WindowsMenu::View,
            WindowsMenu::Window,
            WindowsMenu::Help,
        ];
        let mut expected_left = WINDOWS_BRAND_WIDTH;

        for menu in menus {
            assert_eq!(windows_menu_left(menu), expected_left);
            expected_left += windows_menu_button_width(menu);

            let entries = windows_menu_entries(menu);
            let width = windows_menu_popup_width(&entries, &Localizer::new(LanguageMode::EnUs));
            assert!(width >= WINDOWS_MENU_MIN_WIDTH);
            for entry in entries {
                let WindowsMenuEntry::Item {
                    label, shortcut, ..
                } = entry
                else {
                    continue;
                };
                let required =
                    28.0 + estimated_windows_menu_text_width(&windows_menu_label(
                        &Localizer::new(LanguageMode::EnUs),
                        label,
                    )) + if shortcut.is_empty() {
                        0.0
                    } else {
                        20.0 + estimated_windows_menu_text_width(shortcut)
                    };
                assert!(width >= required.ceil());
            }
        }
    }

    #[test]
    fn titlebar_width_estimate_reserves_more_space_for_wide_characters() {
        assert!(
            estimated_titlebar_label_width("标题测试") > estimated_titlebar_label_width("test")
        );
        assert_eq!(estimated_titlebar_label_width(""), 20.0);
    }

    #[test]
    fn sidebar_width_stays_within_layout_limits() {
        assert_eq!(clamp_sidebar_width(120.0, 1200.0), 220.0);
        assert_eq!(clamp_sidebar_width(600.0, 1200.0), 480.0);
        assert_eq!(clamp_sidebar_width(300.0, 720.0), 300.0);
    }

    #[test]
    fn settings_selectors_cover_every_persisted_choice() {
        assert_eq!(
            SettingsSelector::Language.options(),
            &LANGUAGE_SETTING_OPTIONS
        );
        assert_eq!(SettingsSelector::Theme.options(), &THEME_SETTING_OPTIONS);
        assert_eq!(
            SettingsSelector::TabLayout.options(),
            &TAB_LAYOUT_SETTING_OPTIONS
        );
        assert!(SettingsSelector::TerminalFont.options().is_empty());
        assert_eq!(
            SettingsSelector::TerminalFontSize.options(),
            &TERMINAL_FONT_SIZE_SETTING_OPTIONS
        );
        assert_eq!(
            SettingsSelector::TransferRate.options(),
            &TRANSFER_RATE_SETTING_OPTIONS
        );
        assert_eq!(
            SettingsSelector::ParallelTransfers.options(),
            &PARALLEL_TRANSFER_SETTING_OPTIONS
        );
        assert!(THEME_SETTING_OPTIONS.contains(&SettingsOption {
            label: "System",
            value: SettingsValue::Theme(ThemeMode::System),
        }));
        assert!(TAB_LAYOUT_SETTING_OPTIONS.contains(&SettingsOption {
            label: "Horizontal",
            value: SettingsValue::TabLayout(TabLayout::Horizontal),
        }));
        assert!(
            TERMINAL_FONT_SIZE_SETTING_OPTIONS.contains(&SettingsOption {
                label: "14 pt",
                value: SettingsValue::TerminalFontSize(14),
            })
        );
        assert!(TRANSFER_RATE_SETTING_OPTIONS.contains(&SettingsOption {
            label: "Unlimited",
            value: SettingsValue::TransferRate(0),
        }));
        assert!(PARALLEL_TRANSFER_SETTING_OPTIONS.contains(&SettingsOption {
            label: "4",
            value: SettingsValue::ParallelTransfers(4),
        }));
    }

    #[test]
    fn terminal_font_selection_prefers_saved_font_then_sf_mono_then_menlo() {
        let available = normalize_terminal_font_families(vec![
            "Menlo".into(),
            "SF Mono".into(),
            "Custom Mono".into(),
            ".SystemUIFont".into(),
        ]);

        assert_eq!(
            resolve_terminal_font_family(Some("custom mono"), &available),
            "Custom Mono"
        );
        assert_eq!(
            resolve_terminal_font_family(Some("Missing Mono"), &available),
            "SF Mono"
        );

        let without_sf = normalize_terminal_font_families(vec!["Menlo".into(), "Arial".into()]);
        assert_eq!(resolve_terminal_font_family(None, &without_sf), "Menlo");
        assert!(
            without_sf
                .iter()
                .all(|family| !family.as_ref().starts_with('.'))
        );
    }

    #[test]
    fn quick_commands_target_each_selected_server_once() {
        let profile_ids = vec!["server-a".to_owned(), "server-b".to_owned()];
        let selected_profile_ids = HashSet::from(["server-a".to_owned(), "server-b".to_owned()]);
        let sessions = [
            (SessionId(1), "server-a", true),
            (SessionId(2), "server-a", true),
            (SessionId(3), "server-b", true),
        ];

        assert_eq!(
            quick_command_target_sessions(
                &profile_ids,
                &selected_profile_ids,
                &sessions,
                Some(SessionId(1)),
            ),
            vec![
                ("server-a".to_owned(), SessionId(1)),
                ("server-b".to_owned(), SessionId(3)),
            ]
        );
    }

    #[test]
    fn quick_commands_fall_back_to_the_latest_connected_session() {
        let profile_ids = vec!["server-a".to_owned(), "server-a".to_owned()];
        let selected_profile_ids = HashSet::from(["server-a".to_owned()]);
        let sessions = [
            (SessionId(1), "server-a", false),
            (SessionId(2), "server-a", true),
        ];

        assert_eq!(
            quick_command_target_sessions(
                &profile_ids,
                &selected_profile_ids,
                &sessions,
                Some(SessionId(1)),
            ),
            vec![("server-a".to_owned(), SessionId(2))]
        );
    }

    #[test]
    fn opening_right_sidebar_does_not_move_the_left_sidebar() {
        let left_width = clamp_sidebar_width(300.0, 720.0);
        let right_width = clamp_right_sidebar_width(340.0, 720.0, left_width);

        assert_eq!(left_width, 300.0);
        assert_eq!(right_width, 234.0);
        assert!(
            left_width + right_width + MIN_DETAIL_PANEL_WIDTH + SIDEBAR_RESIZE_HANDLE_WIDTH
                <= 720.0
        );
    }

    #[test]
    fn right_sidebar_width_stays_within_layout_limits() {
        assert_eq!(clamp_right_sidebar_width(100.0, 1200.0, 300.0), 260.0);
        assert_eq!(clamp_right_sidebar_width(700.0, 1200.0, 300.0), 520.0);
        assert_eq!(clamp_right_sidebar_width(340.0, 720.0, 0.0), 340.0);
    }

    #[test]
    fn bottom_panel_height_preserves_main_content() {
        assert_eq!(clamp_bottom_panel_height(80.0, 720.0), 140.0);
        assert_eq!(clamp_bottom_panel_height(600.0, 720.0), 520.0);
        assert_eq!(clamp_bottom_panel_height(400.0, 480.0), 328.0);
    }

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

    #[test]
    fn center_and_sidebar_sftp_requests_are_isolated() {
        let mut center = SftpBrowserState::default();
        let mut sidebar = SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START);
        let center_request = center.begin_request("/center".into());
        let sidebar_request = sidebar.begin_request("/sidebar".into());

        assert_eq!(
            sftp_browser_placement_for_request(center_request),
            SftpBrowserPlacement::Center
        );
        assert_eq!(
            sftp_browser_placement_for_request(sidebar_request),
            SftpBrowserPlacement::Sidebar
        );
        assert!(!center.fail_request(sidebar_request, "wrong browser".into()));
        assert!(center.loading);
        assert!(sidebar.fail_request(sidebar_request, "expected".into()));
        assert!(!sidebar.loading);
    }

    #[test]
    fn remote_parent_path_handles_root_and_nested_directories() {
        assert_eq!(remote_parent_path("/"), None);
        assert_eq!(remote_parent_path("/home"), Some("/".into()));
        assert_eq!(remote_parent_path("/home/test/"), Some("/home".into()));
        assert_eq!(remote_parent_path("relative"), Some(".".into()));
    }

    #[test]
    fn tab_title_prefers_the_path_for_its_active_view() {
        assert_eq!(
            workspace_tab_title(
                "Demo Server",
                TerminalTabView::Files,
                1,
                Some("/home/test"),
                Some("/ignored"),
                &Localizer::new(LanguageMode::EnUs),
            ),
            "Demo Server - /home/test"
        );
        assert_eq!(
            workspace_tab_title(
                "Demo Server",
                TerminalTabView::Files,
                1,
                None,
                Some("/var/log"),
                &Localizer::new(LanguageMode::EnUs),
            ),
            "Demo Server - /var/log"
        );
        assert_eq!(
            workspace_tab_title(
                "Demo Server",
                TerminalTabView::Terminal,
                2,
                Some("/ignored"),
                None,
                &Localizer::new(LanguageMode::EnUs),
            ),
            "Demo Server - Terminal 2"
        );
        assert_eq!(
            workspace_tab_title(
                "Demo Server",
                TerminalTabView::Terminal,
                2,
                None,
                Some("/srv/app"),
                &Localizer::new(LanguageMode::EnUs),
            ),
            "Demo Server - /srv/app"
        );
    }

    #[test]
    fn remote_file_sizes_use_compact_binary_units() {
        assert_eq!(format_remote_size(42), "42 B");
        assert_eq!(format_remote_size(1536), "1.5 KB");
        assert_eq!(format_remote_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn remote_transfer_paths_join_root_relative_and_nested_directories() {
        assert_eq!(remote_join_path("/", "notes.txt"), "/notes.txt");
        assert_eq!(remote_join_path(".", "notes.txt"), "notes.txt");
        assert_eq!(
            remote_join_path("/home/test/", "notes.txt"),
            "/home/test/notes.txt"
        );
        assert_eq!(remote_file_name("/home/test/notes.txt"), "notes.txt");
    }

    #[test]
    fn sftp_tree_flattens_only_expanded_directories() {
        let mut browser = SftpBrowserState {
            entries: vec![
                remote_entry("/home/test/projects", RemoteFileKind::Directory),
                remote_entry("/home/test/notes.txt", RemoteFileKind::File),
            ],
            ..SftpBrowserState::default()
        };
        browser.tree_entries.insert(
            "/home/test/projects".into(),
            vec![
                remote_entry("/home/test/projects/src", RemoteFileKind::Directory),
                remote_entry("/home/test/projects/todo.txt", RemoteFileKind::File),
            ],
        );
        browser.expanded_paths.insert("/home/test/projects".into());

        let rows = browser.visible_rows(true);
        assert_eq!(
            rows.iter()
                .map(|row| (row.entry.path.as_str(), row.depth))
                .collect::<Vec<_>>(),
            vec![
                ("/home/test/projects", 0),
                ("/home/test/projects/src", 1),
                ("/home/test/projects/todo.txt", 1),
                ("/home/test/notes.txt", 0),
            ]
        );
    }

    #[test]
    fn sftp_tree_selection_supports_ranges_and_secondary_toggle() {
        let mut browser = SftpBrowserState {
            entries: vec![
                remote_entry("/home/test/first", RemoteFileKind::File),
                remote_entry("/home/test/second", RemoteFileKind::File),
                remote_entry("/home/test/third", RemoteFileKind::File),
            ],
            ..SftpBrowserState::default()
        };

        browser.select_path("/home/test/first", gpui::Modifiers::default(), true);
        browser.select_path(
            "/home/test/third",
            gpui::Modifiers {
                shift: true,
                ..gpui::Modifiers::default()
            },
            true,
        );
        assert_eq!(
            browser.selected_paths,
            vec!["/home/test/first", "/home/test/second", "/home/test/third"]
        );

        browser.select_path("/home/test/second", secondary_modifiers_for_test(), true);
        assert_eq!(
            browser.selected_paths,
            vec!["/home/test/first", "/home/test/third"]
        );
    }

    #[test]
    fn recursive_operations_drop_children_of_selected_directories() {
        let entries = collapse_nested_remote_entries(vec![
            remote_entry("/home/test/projects/src/main.rs", RemoteFileKind::File),
            remote_entry("/home/test/notes.txt", RemoteFileKind::File),
            remote_entry("/home/test/projects", RemoteFileKind::Directory),
            remote_entry("/home/test/projects/src", RemoteFileKind::Directory),
        ]);

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/home/test/notes.txt", "/home/test/projects"]
        );
    }

    #[test]
    fn remote_breadcrumbs_link_every_ancestor() {
        assert_eq!(
            remote_breadcrumbs("/home/test/projects"),
            vec![
                ("/".into(), "/".into()),
                ("home".into(), "/home".into()),
                ("test".into(), "/home/test".into()),
                ("projects".into(), "/home/test/projects".into()),
            ]
        );
    }

    #[test]
    fn recursive_upload_plan_preserves_empty_directories_and_files() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        std::fs::create_dir_all(project.join("empty")).unwrap();
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();

        let plan = build_local_upload_plan(std::slice::from_ref(&project), "/home/test").unwrap();

        assert_eq!(
            plan.directories,
            vec![
                "/home/test/project",
                "/home/test/project/empty",
                "/home/test/project/src",
            ]
        );
        assert_eq!(
            plan.files,
            vec![(
                project.join("src/main.rs"),
                "/home/test/project/src/main.rs".into(),
            )]
        );
    }

    #[test]
    fn recursive_download_plan_creates_empty_directories_and_file_targets() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("project");
        let plan = build_remote_download_plan(
            RemoteDirectoryTree {
                root: "/home/test/project".into(),
                directories: vec![
                    "/home/test/project".into(),
                    "/home/test/project/empty".into(),
                    "/home/test/project/src".into(),
                ],
                files: vec![remote_entry(
                    "/home/test/project/src/main.rs",
                    RemoteFileKind::File,
                )],
            },
            destination.clone(),
        )
        .unwrap();

        assert!(destination.join("empty").is_dir());
        assert!(destination.join("src").is_dir());
        assert_eq!(
            plan,
            vec![(
                destination.join("src/main.rs"),
                "/home/test/project/src/main.rs".into(),
                Some(12),
            )]
        );
    }

    fn remote_entry(path: &str, kind: RemoteFileKind) -> RemoteFileEntry {
        RemoteFileEntry {
            name: remote_file_name(path).into(),
            path: path.into(),
            kind,
            size: (kind == RemoteFileKind::File).then_some(12),
            modified: None,
        }
    }

    fn secondary_modifiers_for_test() -> gpui::Modifiers {
        #[cfg(target_os = "macos")]
        {
            gpui::Modifiers {
                platform: true,
                ..gpui::Modifiers::default()
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            gpui::Modifiers {
                control: true,
                ..gpui::Modifiers::default()
            }
        }
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

    #[test]
    fn sftp_transfer_queue_tracks_multiple_active_tasks_and_conflicts_release_slots() {
        let mut queue = SftpTransferQueue::default();
        let upload_batch = queue.begin_batch();
        let upload = queue.enqueue_in_batch(
            upload_batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
            None,
        );
        let download_batch = queue.begin_batch();
        let download = queue.enqueue_in_batch(
            download_batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/download.txt"),
            "/remote/download.txt".into(),
            false,
            None,
        );

        assert_eq!(queue.start_next().unwrap().id, upload);
        assert_eq!(queue.start_next().unwrap().id, download);
        assert_eq!(queue.active_count(), 2);
        assert!(queue.mark_progress(upload, 9, Some(12)));
        assert!(queue.mark_progress(upload, 3, Some(12)));
        assert_eq!(queue.task_mut(upload).unwrap().transferred, 9);
        assert!(queue.mark_conflict(download));
        assert_eq!(queue.active_count(), 1);
        assert!(queue.mark_completed(upload, 12));
        assert_eq!(queue.active_count(), 0);
        assert!(queue.retry_with_overwrite(download));
        let retried = queue.start_next().unwrap();
        assert_eq!(retried.id, download);
        assert!(retried.overwrite);
    }

    #[test]
    fn queued_sftp_transfer_can_be_cancelled_without_signalling_the_worker() {
        let mut queue = SftpTransferQueue::default();
        let batch = queue.begin_batch();
        let transfer = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
            None,
        );

        assert_eq!(queue.begin_cancel(transfer), Some(false));
        assert_eq!(
            queue.task_mut(transfer).map(|task| task.state),
            Some(SftpTransferState::Cancelled)
        );
    }

    #[test]
    fn multi_file_download_reports_batch_byte_progress() {
        let mut queue = SftpTransferQueue::default();
        let batch = queue.begin_batch();
        let first = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/first.bin"),
            "/remote/first.bin".into(),
            false,
            Some(100),
        );
        let second = queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/second.bin"),
            "/remote/second.bin".into(),
            false,
            Some(100),
        );
        queue.enqueue_in_batch(
            batch,
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/third.bin"),
            "/remote/third.bin".into(),
            false,
            Some(50),
        );

        assert_eq!(queue.start_next().map(|task| task.id), Some(first));
        assert_eq!(queue.start_next().map(|task| task.id), Some(second));
        assert!(queue.mark_completed(first, 100));
        assert!(queue.mark_progress(second, 50, Some(100)));

        assert_eq!(
            queue.latest_batch_progress(SftpTransferDirection::Download),
            Some(SftpTransferBatchProgress {
                task_count: 3,
                settled_count: 1,
                failed_count: 0,
                transferred: 150,
                total: Some(250),
                fraction: 0.6,
            })
        );
    }

    #[test]
    fn stale_sftp_error_timer_cannot_clear_a_newer_hint() {
        let mut browser = SftpBrowserState::default();
        let stale_generation = browser.set_error("first".into());
        let current_generation = browser.set_error("second".into());

        assert!(!browser.clear_error_if_current(stale_generation));
        assert_eq!(browser.error.as_deref(), Some("second"));
        assert!(browser.clear_error_if_current(current_generation));
        assert!(browser.error.is_none());
    }

    #[test]
    fn stale_sftp_response_does_not_replace_the_latest_directory() {
        let mut browser = SftpBrowserState::default();
        let stale_request = browser.begin_request("/stale".into());
        let current_request = browser.begin_request("/current".into());

        assert!(!browser.complete_request(
            stale_request,
            RemoteDirectory {
                path: "/stale".into(),
                entries: Vec::new(),
            },
        ));
        assert!(browser.complete_request(
            current_request,
            RemoteDirectory {
                path: "/current".into(),
                entries: Vec::new(),
            },
        ));
        assert_eq!(browser.path, "/current");
        assert_eq!(browser.resolved_source_path.as_deref(), Some("/current"));
        assert!(!browser.loading);
    }

    #[test]
    fn canonical_sftp_result_remains_linked_to_its_shell_cwd_request() {
        let mut browser = SftpBrowserState::default();
        assert!(browser.needs_request("."));

        let request = browser.begin_request(".".into());
        assert!(!browser.needs_request("."));
        assert!(browser.complete_request(
            request,
            RemoteDirectory {
                path: "/home/test".into(),
                entries: Vec::new(),
            }
        ));

        assert!(!browser.needs_request("."));
        assert!(browser.needs_request("/var/log"));
    }

    #[test]
    fn remote_text_format_preserves_utf8_bom_and_crlf() {
        let contents = b"\xef\xbb\xbffirst\r\nsecond\r\n";
        let (format, text) = RemoteTextFormat::decode(contents).expect("UTF-8 text");

        assert_eq!(text, "first\nsecond\n");
        assert_eq!(format.line_ending, RemoteLineEnding::CrLf);
        assert!(format.utf8_bom);
        assert_eq!(format.encode(&text), contents);
    }

    #[test]
    fn remote_text_format_rejects_binary_and_invalid_utf8() {
        assert!(RemoteTextFormat::decode(b"text\0data").is_none());
        assert!(RemoteTextFormat::decode(&[0xff, 0xfe]).is_none());
    }

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
    fn app_navigation_defaults_to_home() {
        assert_eq!(ActivePanel::default(), ActivePanel::Home);
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

    #[test]
    fn terminal_session_keeps_ended_output_available_for_its_tab() {
        let profile_id = "server-1";
        let mut session = TerminalSession::new(SessionId(1), profile_id.into());
        let mut terminal = ActiveTerminal::new(profile_id.into(), PtySize::new(80, 24));
        terminal.was_connected = true;
        session.terminal = Some(terminal);
        session.terminal_marked_text = "composition".into();
        session.terminal_selection = Some(TerminalSelection::new(
            TerminalPoint::new(0, 0),
            TerminalPoint::new(0, 1),
        ));
        session.terminal_selecting = true;
        session.terminal_scroll_accumulator = 12.0;

        assert!(session.is_terminal_visible());
        assert!(session.terminal_has_ended());
        assert_eq!(session.profile_id, profile_id);
        assert_eq!(session.id, SessionId(1));
    }

    #[test]
    fn same_profile_terminal_sessions_keep_independent_screen_state() {
        let mut first = TerminalSession::new(SessionId(1), "server-1".into());
        let mut second = TerminalSession::new(SessionId(2), "server-1".into());
        first.terminal = Some(ActiveTerminal::new(
            first.profile_id.clone(),
            PtySize::new(80, 24),
        ));
        second.terminal = Some(ActiveTerminal::new(
            second.profile_id.clone(),
            PtySize::new(80, 24),
        ));

        first.terminal.as_mut().unwrap().process(b"first session");
        second.terminal.as_mut().unwrap().process(b"second session");

        let first_snapshot = first.terminal.as_ref().unwrap().snapshot();
        let second_snapshot = second.terminal.as_ref().unwrap().snapshot();
        assert_ne!(first_snapshot, second_snapshot);
        assert_eq!(first.id, SessionId(1));
        assert_eq!(second.id, SessionId(2));
    }

    #[test]
    fn terminal_tab_keeps_split_panes_in_one_layout() {
        let first_pane = PaneId(1);
        let second_pane = PaneId(2);
        let mut tab = TerminalTab {
            id: TabId(1),
            profile_id: "server-1".into(),
            layout: PaneLayout::Pane(first_pane),
            active_pane_id: first_pane,
            view: TerminalTabView::Terminal,
        };

        assert!(
            tab.layout
                .split(first_pane, second_pane, SplitAxis::Horizontal)
        );
        tab.active_pane_id = second_pane;

        assert!(tab.layout.contains(first_pane));
        assert!(tab.layout.contains(second_pane));
        assert_eq!(tab.active_pane_id, second_pane);
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

    #[test]
    fn terminal_select_all_covers_the_complete_visible_grid() {
        assert_eq!(full_terminal_selection(0, 80), None);
        assert_eq!(full_terminal_selection(24, 0), None);
        assert_eq!(
            full_terminal_selection(24, 80),
            Some(TerminalSelection::new(
                TerminalPoint::new(0, 0),
                TerminalPoint::new(23, 80),
            ))
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
