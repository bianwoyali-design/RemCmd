mod text_field;
use text_field::{TextField, bind_text_field_keys};

mod file_editor;
use file_editor::{FileEditor, FileEditorEvent, bind_file_editor_keys};

mod pane_layout;
use pane_layout::{PaneId, PaneLayout, SplitAxis};

mod icons;
use icons::{IconName, RemCmdAssets, icon, icon_with_color};

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
use terminal_canvas::{TerminalCanvasFrame, TerminalCellMetrics};

mod terminal_view;
use terminal_view::{TerminalPalette, TerminalViewModel, palette_color};

#[cfg(target_os = "macos")]
mod private_key_picker;

#[cfg(target_os = "macos")]
mod macos_symbols;

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    Animation, AnimationExt, AnyElement, AnyView, App, Application, Bounds, BoxShadow,
    ClipboardItem, Context, CursorStyle, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, FontWeight, IntoElement, KeyBinding, KeyDownEvent, Keystroke,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels,
    PromptButton, PromptLevel, Render, ScrollHandle, ScrollWheelEvent, SharedString, Subscription,
    Task, Timer, TitlebarOptions, UTF16Selection, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowOptions, canvas, deferred,
    div, ease_in_out, ease_out_quint, img, point, prelude::*, px, rgb, size, uniform_list,
};
use secrecy::SecretString;

use remcmd_core::{AuthConfig, ConnectionProfile, TabLayout, ThemeMode, TransferSettings};
use remcmd_ssh::{
    AuthMethod, ConnectionEvent, ConnectionHandle, HostKeyInfo, MAX_REMOTE_FILE_BYTES, PtySize,
    RemoteDirectory, RemoteDirectoryTree, RemoteFile, RemoteFileEntry, RemoteFileKind,
    ServerPerformanceSnapshot, SessionState, SftpOperation, SftpTransferDirection, ShellEvent,
    SshConnection, SshErrorKind, TransferRateLimiter,
};
use remcmd_storage::{
    AppSettings, CredentialKind, default_profiles_path, default_settings_path, delete_credential,
    delete_profile_credentials, ensure_profiles_file, load_credential, load_profiles,
    load_settings, save_credential, save_profiles, save_settings,
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
const SIDEBAR_DEFAULT_WIDTH: f32 = 300.0;
const SIDEBAR_MIN_WIDTH: f32 = 220.0;
const SIDEBAR_MAX_WIDTH: f32 = 480.0;
const SIDEBAR_SFTP_REQUEST_ID_START: u64 = 1 << 63;
const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;
const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 340.0;
const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 260.0;
const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 520.0;
const MIN_DETAIL_PANEL_WIDTH: f32 = 180.0;
const COLLAPSED_TITLEBAR_LEADING_WIDTH: f32 = 140.0;
const TITLEBAR_HEIGHT: f32 = 52.0;
const TITLEBAR_TAB_HEIGHT: f32 = 30.0;
const TITLEBAR_TAB_GROUP_HEIGHT: f32 = 36.0;
const TITLEBAR_ACTION_GROUP_WIDTH: f32 = 74.0;
const TITLEBAR_CONTROL_HOVER_SIZE: f32 = 28.0;
const TITLEBAR_ADD_ICON_SIZE: f32 = 16.0;
const TITLEBAR_SIDEBAR_ICON_SIZE: f32 = 20.0;
const TITLEBAR_LEFT_CONTROL_EDGE_GAP: f32 = 10.0;
const TITLEBAR_TAB_ICON_ONLY_WIDTH: f32 = 44.0;
const TITLEBAR_TAB_ELLIPSIS_MIN_WIDTH: f32 = 56.0;
const TITLEBAR_ACTIVE_TAB_GROWTH: f32 = 36.0;
const TITLEBAR_CLOSE_SYMBOL_SIZE: f32 = 12.0;
const TRAFFIC_LIGHT_INSET_X: f32 = 20.0;
const TRAFFIC_LIGHT_INSET_Y: f32 = 18.0;
#[cfg(target_os = "macos")]
const TERMINAL_FONT_FAMILY: &str = "SF Mono";
#[cfg(target_os = "windows")]
const TERMINAL_FONT_FAMILY: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const TERMINAL_FONT_FAMILY: &str = "DejaVu Sans Mono";

gpui::actions!(credential_prompt, [SubmitCredential, CancelCredential]);
gpui::actions!(host_key_prompt, [CancelHostKeyVerification]);
gpui::actions!(settings_selector, [CancelSettingsSelector]);
gpui::actions!(sftp_create_prompt, [SubmitSftpCreate, CancelSftpCreate]);

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
    titlebar_tabs_scroll_task: Option<Task<()>>,
    next_tab_id: u64,
    panes: Vec<TerminalPane>,
    active_pane_id: Option<PaneId>,
    next_pane_id: u64,
    sidebar_search: Entity<TextField>,
    sidebar_search_visible: bool,
    connections_expanded: bool,
    sidebar_width: f32,
    left_sidebar_open: bool,
    left_sidebar_progress: f32,
    left_sidebar_animation_task: Option<Task<()>>,
    sidebar_resize: Option<SidebarResize>,
    right_sidebar_open: bool,
    right_sidebar_width: f32,
    right_sidebar_resize: Option<SidebarResize>,
    right_sidebar_transition_id: u64,
    right_sidebar_view: RightSidebarView,
    credential_lookup_task: Option<Task<()>>,
    credential_lookup_session_id: Option<SessionId>,
    credential_mutations_in_progress: HashMap<String, usize>,
    active_panel: ActivePanel,
    theme_mode: ThemeMode,
    tab_layout: TabLayout,
    transfer_settings: TransferSettings,
    transfer_rate_limiter: Arc<TransferRateLimiter>,
    next_transfer_session_cursor: usize,
    sftp_context_menu: Option<SftpContextMenu>,
    sftp_create_prompt: Option<SftpCreatePrompt>,
    open_settings_selector: Option<SettingsSelector>,
    settings_focus_handle: FocusHandle,
    theme: Theme,
    settings_path: PathBuf,
    settings_error: Option<String>,
    _appearance_subscription: Subscription,
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
    close_when_disconnected: bool,
    connection_state: SessionState,
    connection_handle: Option<ConnectionHandle>,
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
    connection_credential: Option<ConnectionCredential>,
    sftp: SftpBrowserState,
    sidebar_sftp: SftpBrowserState,
    transfers: SftpTransferQueue,
    performance: ServerPerformanceState,
}

impl TerminalSession {
    fn new(id: SessionId, profile_id: String) -> Self {
        Self {
            id,
            profile_id,
            close_when_disconnected: false,
            connection_state: SessionState::Disconnected,
            connection_handle: None,
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
            connection_credential: None,
            sftp: SftpBrowserState::default(),
            sidebar_sftp: SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START),
            transfers: SftpTransferQueue::default(),
            performance: ServerPerformanceState::default(),
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
}

struct CommandTooltip {
    label: SharedString,
    theme: Theme,
}

impl Render for CommandTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py(px(5.0))
            .rounded_md()
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.modal_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(3.0)),
                blur_radius: px(10.0),
                spread_radius: px(-3.0),
            }])
            .text_sm()
            .text_color(self.theme.text_primary)
            .child(self.label.clone())
    }
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
            remote_cwd: None,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsSelector {
    Theme,
    TabLayout,
    TransferRate,
    ParallelTransfers,
}

impl SettingsSelector {
    const fn element_id(self) -> &'static str {
        match self {
            Self::Theme => "settings-theme-selector",
            Self::TabLayout => "settings-tab-layout-selector",
            Self::TransferRate => "settings-transfer-rate-selector",
            Self::ParallelTransfers => "settings-parallel-transfers-selector",
        }
    }

    const fn options(self) -> &'static [SettingsOption] {
        match self {
            Self::Theme => &THEME_SETTING_OPTIONS,
            Self::TabLayout => &TAB_LAYOUT_SETTING_OPTIONS,
            Self::TransferRate => &TRANSFER_RATE_SETTING_OPTIONS,
            Self::ParallelTransfers => &PARALLEL_TRANSFER_SETTING_OPTIONS,
        }
    }

    const fn control_width(self) -> f32 {
        match self {
            Self::Theme => 92.0,
            Self::TabLayout => 104.0,
            Self::TransferRate => 104.0,
            Self::ParallelTransfers => 56.0,
        }
    }

    const fn menu_width(self) -> f32 {
        match self {
            Self::Theme => 104.0,
            Self::TabLayout => 120.0,
            Self::TransferRate => 120.0,
            Self::ParallelTransfers => 72.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsValue {
    Theme(ThemeMode),
    TabLayout(TabLayout),
    TransferRate(u32),
    ParallelTransfers(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsOption {
    label: &'static str,
    value: SettingsValue,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RightSidebarView {
    #[default]
    Sftp,
    Performance,
}

#[derive(Clone, Copy, Debug)]
struct PerformanceCounters {
    captured_at: Instant,
    cpu_total: u64,
    cpu_idle: u64,
    network_rx_bytes: u64,
    network_tx_bytes: u64,
}

#[derive(Default)]
struct ServerPerformanceState {
    snapshot: Option<ServerPerformanceSnapshot>,
    previous: Option<PerformanceCounters>,
    cpu_usage: Option<f32>,
    network_rx_per_second: Option<f64>,
    network_tx_per_second: Option<f64>,
    monitoring: bool,
    loading: bool,
    error: Option<String>,
}

impl ServerPerformanceState {
    fn update(&mut self, snapshot: ServerPerformanceSnapshot, captured_at: Instant) {
        if let Some(previous) = self.previous {
            let total_delta = snapshot.cpu_total.saturating_sub(previous.cpu_total);
            let idle_delta = snapshot.cpu_idle.saturating_sub(previous.cpu_idle);
            if total_delta > 0 && idle_delta <= total_delta {
                self.cpu_usage =
                    Some((total_delta - idle_delta) as f32 / total_delta as f32 * 100.0);
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
            }
        }

        self.previous = Some(PerformanceCounters {
            captured_at,
            cpu_total: snapshot.cpu_total,
            cpu_idle: snapshot.cpu_idle,
            network_rx_bytes: snapshot.network_rx_bytes,
            network_tx_bytes: snapshot.network_tx_bytes,
        });
        self.snapshot = Some(snapshot);
        self.loading = false;
        self.error = None;
    }

    fn clear_connection(&mut self) {
        self.snapshot = None;
        self.previous = None;
        self.cpu_usage = None;
        self.network_rx_per_second = None;
        self.network_tx_per_second = None;
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
    pending_download_trees: HashMap<u64, PathBuf>,
    selected_paths: Vec<String>,
    selection_anchor: Option<String>,
    scroll_handle: UniformListScrollHandle,
    breadcrumb_scroll_handle: ScrollHandle,
}

#[derive(Clone)]
struct SftpTreeRow {
    entry: RemoteFileEntry,
    depth: usize,
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
        self.error = None;
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
            self.error = None;
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
        self.error = None;
        self.active_request_id = None;
        self.resolved_source_path = self.active_request_path.take();
        self.breadcrumb_scroll_handle
            .scroll_to_item(breadcrumb_count.saturating_mul(2).saturating_sub(2));
        true
    }

    fn fail_request(&mut self, request_id: u64, error: String) -> bool {
        if let Some(path) = self.tree_requests.remove(&request_id) {
            self.expanded_paths.remove(&path);
            self.error = Some(error);
            return true;
        }
        if self.pending_download_trees.remove(&request_id).is_some() {
            self.error = Some(error);
            return true;
        }
        if self.active_request_id != Some(request_id) {
            return false;
        }

        self.loading = false;
        self.error = Some(error);
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
        self.error = None;
        request_id
    }

    fn begin_download_tree(&mut self, destination: PathBuf) -> u64 {
        let request_id = self.next_request_id();
        self.pending_download_trees.insert(request_id, destination);
        request_id
    }

    fn take_download_tree_destination(&mut self, request_id: u64) -> Option<PathBuf> {
        self.pending_download_trees.remove(&request_id)
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

struct SftpContextMenu {
    session_id: SessionId,
    placement: SftpBrowserPlacement,
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
    direction: SftpTransferDirection,
    local_path: PathBuf,
    remote_path: String,
    overwrite: bool,
    state: SftpTransferState,
    transferred: u64,
    total: Option<u64>,
    error: Option<String>,
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

    fn status_text(&self) -> String {
        match self.state {
            SftpTransferState::Queued => "Queued".into(),
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
            SftpTransferState::Cancelling => "Cancelling...".into(),
            SftpTransferState::Conflict => "Destination already exists".into(),
            SftpTransferState::Completed => {
                format!("Completed · {}", format_remote_size(self.transferred))
            }
            SftpTransferState::Failed => self.error.clone().unwrap_or_else(|| "Failed".into()),
            SftpTransferState::Cancelled => "Cancelled".into(),
        }
    }
}

#[derive(Default)]
struct SftpTransferQueue {
    next_id: u64,
    tasks: Vec<SftpTransferTask>,
}

impl SftpTransferQueue {
    fn enqueue(
        &mut self,
        direction: SftpTransferDirection,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
    ) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(SftpTransferTask {
            id,
            direction,
            local_path,
            remote_path,
            overwrite,
            state: SftpTransferState::Queued,
            transferred: 0,
            total: None,
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
        task.total = total;
        true
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
    session_id: SessionId,
    profile_id: String,
    kind: CredentialPromptKind,
    input: Entity<TextField>,
    remember: bool,
    error: Option<String>,
}

#[derive(Clone)]
enum CredentialPromptKind {
    Password,
    PrivateKeyPassphrase { path: PathBuf },
}

impl CredentialPromptKind {
    fn credential_kind(&self) -> CredentialKind {
        match self {
            Self::Password => CredentialKind::Password,
            Self::PrivateKeyPassphrase { .. } => CredentialKind::PrivateKeyPassphrase,
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
        let tab_layout = settings.tab_layout;
        let transfer_settings = settings.transfers.normalized();
        let transfer_rate_limiter = Arc::new(TransferRateLimiter::new(
            transfer_settings.bytes_per_second(),
        ));
        let theme = Theme::resolve(theme_mode, window);
        set_global_theme(theme, cx);

        let appearance_subscription = cx.observe_window_appearance(window, |this, window, cx| {
            this.refresh_system_theme(window, cx);
        });
        let sidebar_search = cx.new(|cx| TextField::new(cx, "", "Search connections"));
        cx.observe(&sidebar_search, |_, _, cx| cx.notify()).detach();
        let settings_focus_handle = cx.focus_handle();

        let mut app = Self {
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
            titlebar_tabs_scroll_task: None,
            next_tab_id: 1,
            panes: Vec::new(),
            active_pane_id: None,
            next_pane_id: 1,
            sidebar_search,
            sidebar_search_visible: false,
            connections_expanded: true,
            sidebar_width: SIDEBAR_DEFAULT_WIDTH,
            left_sidebar_open: true,
            left_sidebar_progress: 1.0,
            left_sidebar_animation_task: None,
            sidebar_resize: None,
            right_sidebar_open: false,
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            right_sidebar_resize: None,
            right_sidebar_transition_id: 0,
            right_sidebar_view: RightSidebarView::Sftp,
            credential_lookup_task: None,
            credential_lookup_session_id: None,
            credential_mutations_in_progress: HashMap::new(),
            active_panel: ActivePanel::Connection,
            theme_mode,
            tab_layout,
            transfer_settings,
            transfer_rate_limiter,
            next_transfer_session_cursor: 0,
            sftp_context_menu: None,
            sftp_create_prompt: None,
            open_settings_selector: None,
            settings_focus_handle,
            theme,
            settings_path,
            settings_error,
            _appearance_subscription: appearance_subscription,
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

    fn session_for_profile_mut(&mut self, profile_id: &str) -> Option<&mut TerminalSession> {
        self.sessions
            .iter_mut()
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
        let left_sidebar_width =
            clamp_sidebar_width(self.sidebar_width, viewport_width) * self.left_sidebar_progress;
        clamp_right_sidebar_width(self.right_sidebar_width, viewport_width, left_sidebar_width)
    }

    fn titlebar_leading_width(&self, window: &Window) -> f32 {
        COLLAPSED_TITLEBAR_LEADING_WIDTH
            + (self.effective_sidebar_width(window) - COLLAPSED_TITLEBAR_LEADING_WIDTH)
                * self.left_sidebar_progress
    }

    fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        self.left_sidebar_open = !self.left_sidebar_open;
        self.sidebar_resize = None;
        let start = self.left_sidebar_progress;
        let end = if self.left_sidebar_open { 1.0 } else { 0.0 };
        self.left_sidebar_animation_task = Some(cx.spawn(async move |this, cx| {
            for frame in 1..=12 {
                Timer::after(Duration::from_millis(15)).await;
                let progress = ease_in_out(frame as f32 / 12.0);
                let value = start + (end - start) * progress;
                let _ = this.update(cx, |this, cx| {
                    this.left_sidebar_progress = value;
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    fn begin_sidebar_resize(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.left_sidebar_progress < 1.0 {
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
            self.effective_sidebar_width(window) * self.left_sidebar_progress,
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

    fn toggle_right_sidebar(&mut self, cx: &mut Context<Self>) {
        self.right_sidebar_open = !self.right_sidebar_open;
        self.right_sidebar_transition_id += 1;
        self.right_sidebar_resize = None;
        if self.right_sidebar_open
            && self.right_sidebar_view == RightSidebarView::Sftp
            && let Some(session_id) = self.active_session_id
        {
            self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
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
        let Some((path, needs_load)) = self.session(session_id).map(|session| {
            let path = session
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.remote_cwd.clone())
                .unwrap_or_else(|| ".".into());
            let browser = session.sftp_browser(placement);
            let editor_closed =
                placement == SftpBrowserPlacement::Sidebar || session.sftp.file.is_none();
            let needs_load = editor_closed && browser.needs_request(&path);
            (path, needs_load)
        }) else {
            return;
        };
        if needs_load {
            self.request_sftp_directory(session_id, placement, path, cx);
        }
    }

    fn request_sftp_directory(
        &mut self,
        session_id: SessionId,
        placement: SftpBrowserPlacement,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let handle = self.session(session_id).and_then(|session| {
            (session.connection_state == SessionState::Connected)
                .then(|| session.connection_handle.clone())
                .flatten()
        });
        let Some(handle) = handle else {
            if let Some(session) = self.session_mut(session_id) {
                let browser = session.sftp_browser_mut(placement);
                browser.error = Some("Connect this terminal to browse remote files".into());
                browser.loading = false;
            }
            cx.notify();
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

        if let Err(error) = handle.read_directory(request_id, request_path)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp_browser_mut(placement)
                .fail_request(request_id, error.to_string());
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
        if let Err(error) = handle.read_directory(request_id, path)
            && let Some(session) = self.session_mut(session_id)
        {
            session
                .sftp_browser_mut(placement)
                .fail_request(request_id, error.to_string());
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
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some(format!(
                    "File exceeds the {} MB editor limit",
                    MAX_REMOTE_FILE_BYTES / 1024 / 1024
                ));
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
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error = Some("Save or revert your changes before closing this file".into());
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
            prompt: Some("Upload".into()),
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
                        if let Some(session) = this.session_mut(session_id) {
                            session.sftp_browser_mut(placement).error =
                                Some(format!("Failed to prepare upload: {message}"));
                        }
                        cx.notify();
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
                            if let Some(session) = this.session_mut(session_id) {
                                session.sftp_browser_mut(placement).error =
                                    Some("Failed to queue remote directory creation".into());
                            }
                            cx.notify();
                            return;
                        }
                    }

                    for (local_path, remote_path) in plan.files {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferDirection::Upload,
                            local_path,
                            remote_path,
                            false,
                            cx,
                        );
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    if let Some(session) = this.session_mut(session_id) {
                        session.sftp_browser_mut(placement).error =
                            Some(format!("Failed to open upload picker: {error}"));
                    }
                    cx.notify();
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
            prompt: Some("Download".into()),
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

                    for entry in entries {
                        let local_path = remote_relative_path(&browser_root, &entry.path)
                            .map(|relative| join_remote_relative(&destination, relative))
                            .unwrap_or_else(|| destination.join(&entry.name));
                        if entry.kind == RemoteFileKind::Directory {
                            let request_id = this
                                .session_mut(session_id)
                                .expect("SFTP session should still exist")
                                .sftp_browser_mut(placement)
                                .begin_download_tree(local_path);
                            if let Err(error) =
                                handle.read_directory_tree(request_id, entry.path.clone())
                                && let Some(session) = this.session_mut(session_id)
                            {
                                session
                                    .sftp_browser_mut(placement)
                                    .fail_request(request_id, error.to_string());
                            }
                        } else if entry.kind == RemoteFileKind::File {
                            this.enqueue_sftp_transfer(
                                session_id,
                                SftpTransferDirection::Download,
                                local_path,
                                entry.path,
                                false,
                                cx,
                            );
                        }
                    }
                });
            }
            Ok(Ok(None)) | Err(_) => {}
            Ok(Err(error)) => {
                let _ = this.update(cx, |this, cx| {
                    if let Some(session) = this.session_mut(session_id) {
                        session.sftp_browser_mut(placement).error =
                            Some(format!("Failed to open download destination: {error}"));
                    }
                    cx.notify();
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
        let destination = self.session_mut(session_id).and_then(|session| {
            session
                .sftp_browser_mut(placement)
                .take_download_tree_destination(request_id)
        });
        let Some(destination) = destination else {
            return;
        };
        let runtime = cx.global::<SshRuntime>().handle();

        cx.spawn(async move |this, cx| {
            let plan = runtime
                .spawn_blocking(move || build_remote_download_plan(tree, destination))
                .await;
            let _ = this.update(cx, |this, cx| match plan {
                Ok(Ok(plan)) => {
                    for (local_path, remote_path) in plan {
                        this.enqueue_sftp_transfer(
                            session_id,
                            SftpTransferDirection::Download,
                            local_path,
                            remote_path,
                            false,
                            cx,
                        );
                    }
                }
                Ok(Err(error)) => {
                    if let Some(session) = this.session_mut(session_id) {
                        session.sftp_browser_mut(placement).error =
                            Some(format!("Failed to prepare recursive download: {error}"));
                    }
                    cx.notify();
                }
                Err(error) => {
                    if let Some(session) = this.session_mut(session_id) {
                        session.sftp_browser_mut(placement).error =
                            Some(format!("Recursive download task failed: {error}"));
                    }
                    cx.notify();
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
            format!("Delete {}?", entries[0].name)
        } else {
            format!("Delete {} selected items?", paths.len())
        };
        self.sftp_context_menu = None;
        let answer = window.prompt(
            PromptLevel::Critical,
            &message,
            Some("Directories and their contents will be deleted recursively."),
            &[PromptButton::new("Delete"), PromptButton::cancel("Cancel")],
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
                if let Err(error) = handle.delete_paths(request_id, paths)
                    && let Some(session) = this.session_mut(session_id)
                {
                    session.sftp_browser_mut(placement).error = Some(error.to_string());
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
        let input = cx.new(|cx| {
            TextField::new(
                cx,
                "",
                match kind {
                    SftpCreateKind::File => "File name",
                    SftpCreateKind::Directory => "Folder name",
                },
            )
        });
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
            if let Some(prompt) = self.sftp_create_prompt.as_mut() {
                prompt.error = Some("Enter a valid name without path separators".into());
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
        direction: SftpTransferDirection,
        local_path: PathBuf,
        remote_path: String,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session
            .transfers
            .enqueue(direction, local_path, remote_path, overwrite);
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
                    "Connect this terminal before transferring files",
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
            let result = handle
                .ok_or_else(|| {
                    remcmd_ssh::SshError::new(
                        SshErrorKind::InvalidState,
                        "SSH connection task is not running",
                    )
                })
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

        let scroll_handle = self.titlebar_tabs_scroll_handle.clone();
        self.titlebar_tabs_scroll_task = Some(cx.spawn(async move |this, cx| {
            // Let GPUI lay out the new tab before reading the track's final overflow.
            Timer::after(Duration::from_millis(16)).await;

            let start = scroll_handle.offset();
            let easing = ease_out_quint();
            for frame in 1..=10 {
                let progress = easing(frame as f32 / 10.0);
                let target_x = -scroll_handle.max_offset().width;
                scroll_handle.set_offset(point(start.x + (target_x - start.x) * progress, start.y));
                let _ = this.update(cx, |_, cx| cx.notify());
                Timer::after(Duration::from_millis(16)).await;
            }
        }));
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

        let profile_changed = self.selected_profile_id.as_deref() != Some(profile_id.as_str());
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
        if profile_changed {
            self.load_editor_for_selected_profile(cx);
        }
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

        self.sync_performance_monitoring();
        true
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

        self.persist_settings();
        cx.notify();
    }

    fn set_tab_layout(&mut self, tab_layout: TabLayout, cx: &mut Context<Self>) {
        self.tab_layout = tab_layout;
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
            SettingsSelector::Theme => SettingsValue::Theme(self.theme_mode),
            SettingsSelector::TabLayout => SettingsValue::TabLayout(self.tab_layout),
            SettingsSelector::TransferRate => {
                SettingsValue::TransferRate(self.transfer_settings.rate_limit_mib_per_second)
            }
            SettingsSelector::ParallelTransfers => {
                SettingsValue::ParallelTransfers(self.transfer_settings.max_parallel_transfers)
            }
        }
    }

    fn settings_value_label(&self, selector: SettingsSelector) -> SharedString {
        let value = self.settings_value(selector);
        if let Some(option) = selector
            .options()
            .iter()
            .find(|option| option.value == value)
        {
            return option.label.into();
        }

        match value {
            SettingsValue::TransferRate(rate) => format!("{rate} MiB/s").into(),
            SettingsValue::ParallelTransfers(count) => count.to_string().into(),
            SettingsValue::Theme(_) | SettingsValue::TabLayout(_) => {
                unreachable!("all theme and tab-layout values have labels")
            }
        }
    }

    fn toggle_settings_selector(
        &mut self,
        selector: SettingsSelector,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_selector = if self.open_settings_selector == Some(selector) {
            None
        } else {
            Some(selector)
        };
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
            SettingsValue::Theme(mode) => self.set_theme_mode(mode, window, cx),
            SettingsValue::TabLayout(layout) => self.set_tab_layout(layout, cx),
            SettingsValue::TransferRate(rate) => self.set_transfer_rate_limit(rate, cx),
            SettingsValue::ParallelTransfers(count) => {
                self.set_max_parallel_transfers(count, cx);
            }
        }
    }

    fn persist_settings(&mut self) {
        let settings = AppSettings {
            theme_mode: self.theme_mode,
            tab_layout: self.tab_layout,
            transfers: self.transfer_settings,
        };
        self.settings_error = save_settings(&self.settings_path, &settings)
            .err()
            .map(|error| format!("Failed to save settings: {error}"));
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
        session_id: SessionId,
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
        success_message: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        let runtime = cx.global::<SshRuntime>().handle();
        *self
            .credential_mutations_in_progress
            .entry(profile_id.clone())
            .or_default() += 1;
        if let Some(session) = self.session_for_profile_mut(&profile_id) {
            session.connection_message = Some("Updating the system keychain".into());
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
                            session.connection_message = Some(message.into());
                        }
                    }
                    Ok(Err(error)) => {
                        this.form_error = Some(error.to_string());
                    }
                    Err(error) => {
                        this.form_error = Some(format!(
                            "Failed to access the system keychain task: {error}"
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
            ProfileAuthKind::Agent => return,
        };

        self.form_error = None;
        self.delete_stored_credentials(
            editor.profile_id.clone(),
            Some(kind),
            Some("Saved credential removed from the system keychain"),
            cx,
        );
    }

    fn select_profile(&mut self, profile_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::Connection;
        self.open_settings_selector = None;
        let tab_id = self
            .active_tab()
            .filter(|tab| tab.profile_id == profile_id)
            .or_else(|| {
                self.tabs
                    .iter()
                    .rev()
                    .find(|tab| tab.profile_id == profile_id)
            })
            .map(|tab| tab.id);
        self.active_session_id = None;
        self.selected_profile_id = Some(profile_id);
        self.load_editor_for_selected_profile(cx);
        if let Some(tab_id) = tab_id {
            self.activate_tab_in_window(tab_id, window, cx);
        }
        self.sync_performance_monitoring();
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
        self.open_settings_selector = None;
        self.active_session_id = None;
        self.selected_profile_id = Some(profile.id.clone());
        self.profiles.push(profile);
        self.next_profile_number += 1;

        self.load_editor_for_selected_profile(cx);
        self.persist_profiles();

        self.sync_performance_monitoring();
        cx.notify();
    }

    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.active_panel = ActivePanel::Settings;
        self.open_settings_selector = None;
        self.settings_focus_handle.focus(window);
        cx.notify();
    }

    fn toggle_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn delete_selected_profile(&mut self, cx: &mut Context<Self>) {
        let Some(selected_id) = self.selected_profile_id.clone() else {
            return;
        };

        if self.sessions.iter().any(|session| {
            session.profile_id == selected_id && session.connection_state.can_disconnect()
        }) {
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

        self.selected_profile_id = if self.profiles.is_empty() {
            None
        } else if selected_index == 0 {
            Some(self.profiles[0].id.clone())
        } else {
            Some(self.profiles[selected_index - 1].id.clone())
        };

        self.load_editor_for_selected_profile(cx);
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

        let credentials_changed = self
            .profiles
            .iter()
            .find(|profile| profile.id == editor.profile_id)
            .is_some_and(|profile| {
                credentials_invalidated_by_edit(profile, &host, port, &username, &auth)
            });

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
        if credentials_changed {
            self.delete_stored_credentials(
                editor.profile_id,
                None,
                Some("Saved credentials cleared because connection details changed"),
                cx,
            );
        }

        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.load_editor_for_selected_profile(cx);
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
        let Some(profile) = self
            .active_session()
            .and_then(|session| {
                self.profiles
                    .iter()
                    .find(|profile| profile.id == session.profile_id)
            })
            .cloned()
        else {
            return;
        };
        if self.credential_lookup_task.is_some()
            || self
                .credential_mutations_in_progress
                .contains_key(&profile.id)
            || !self
                .tab(tab_id)
                .is_some_and(|tab| tab.layout.contains(active_pane_id))
        {
            return;
        }

        let session_id = self.create_session_for_profile(&profile.id);
        let pane_id = self.create_terminal_pane(tab_id, session_id, window, cx);
        let split = self
            .tab_mut(tab_id)
            .expect("validated tab should remain present")
            .layout
            .split(active_pane_id, pane_id, axis);
        debug_assert!(split, "validated active pane should be splittable");

        self.activate_session(session_id, cx);
        self.connect_profile_in_session(session_id, profile, window, cx);
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

        match &profile.auth {
            AuthConfig::Password => {
                self.lookup_credential_and_connect(
                    session_id,
                    profile,
                    CredentialPromptKind::Password,
                    window,
                    cx,
                );
            }
            AuthConfig::PrivateKey { path } => {
                let prompt_kind = CredentialPromptKind::PrivateKeyPassphrase { path: path.clone() };
                self.lookup_credential_and_connect(session_id, profile, prompt_kind, window, cx);
            }
            AuthConfig::Agent => {
                if self.activate_session_in_window(session_id, window, cx) {
                    self.start_connection(session_id, profile, AuthMethod::Agent, None, cx);
                }
            }
        }
    }

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
        if let Some(session) = self.session_mut(session_id) {
            session.connection_error = None;
            session.connection_message = Some("Checking the system keychain".into());
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
                    },
                }
            });
        }));

        cx.notify();
    }

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
        session.connection_credential = credential;
        session.connection_error = None;
        session.connection_message = None;
        session.terminal_end_reason = None;
        session.terminal = Some(ActiveTerminal::new(profile.id, pty_size));
        session.terminal_marked_text.clear();
        session.terminal_selection = None;
        session.terminal_selecting = false;
        session.terminal_scroll_accumulator = 0.0;
        session.terminal_resize_task = None;
        session.sftp = SftpBrowserState::default();
        session.sidebar_sftp =
            SftpBrowserState::with_request_id_start(SIDEBAR_SFTP_REQUEST_ID_START);

        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next_event().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_connection_event(session_id, event, cx);
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
        let session_id = prompt.session_id;
        let kind = prompt.kind.clone();
        let remember = prompt.remember;
        let input = prompt.input.clone();

        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
        else {
            self.dismiss_credential_prompt(cx);
            if let Some(session) = self.session_mut(session_id) {
                session.connection_error = Some("Connection profile no longer exists".into());
            }
            cx.notify();
            return;
        };

        let secret = SecretString::new(
            input
                .update(cx, |input, cx| input.take_text(cx))
                .into_boxed_str(),
        );
        self.credential_prompt = None;

        let credential_kind = kind.credential_kind();
        let save_on_success = remember.then(|| secret.clone());
        let auth = auth_method_with_secret(kind, secret);
        let credential =
            ConnectionCredential::from_prompt(profile_id, credential_kind, save_on_success);

        if self.activate_session_in_window(session_id, window, cx) {
            self.start_connection(session_id, profile, auth, Some(credential), cx);
        }
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
        let Some(session) = self.active_session_mut() else {
            return;
        };
        let Some(info) = session.host_key_prompt.take() else {
            return;
        };

        let Some(handle) = session.connection_handle.as_ref() else {
            session.connection_error = Some("SSH connection handle is missing".into());
            cx.notify();
            return;
        };

        match handle.trust_host_key() {
            Ok(()) => {
                session.connection_message = Some(format!("Trusting {}", info.address()));
            }
            Err(error) => {
                session.connection_error = Some(error.to_string());
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
                AuthConfig::Password | AuthConfig::Agent => None,
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
        let uses_password = self.profiles.iter().any(|profile| {
            profile.id == profile_id && matches!(profile.auth, AuthConfig::Password)
        });
        if !uses_password {
            return false;
        }

        self.activate_session(session_id, cx);
        self.open_credential_prompt(
            session_id,
            profile_id,
            CredentialPromptKind::Password,
            Some(error),
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
        if let Some(session) = self.session_mut(session_id) {
            session.connection_message = Some("Removing the rejected saved credential".into());
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
                        "{authentication_error}\nFailed to access the system keychain task: {error}"
                    ),
                };

                match kind {
                    CredentialKind::Password => {
                        this.prompt_for_password(session_id, profile_id, error, cx);
                    }
                    CredentialKind::PrivateKeyPassphrase => {
                        this.prompt_for_private_key_passphrase(session_id, profile_id, error, cx);
                    }
                }
                cx.notify();
            });
        }));
    }

    fn save_successful_credential(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(credential) = self
            .session_mut(session_id)
            .and_then(|session| session.connection_credential.take())
        else {
            return;
        };
        let Some(secret) = credential.save_on_success else {
            return;
        };

        let profile_id = credential.profile_id;
        let kind = credential.kind;
        let runtime = cx.global::<SshRuntime>().handle();
        cx.spawn(async move |this, cx| {
            let result = runtime
                .spawn_blocking(move || save_credential(&profile_id, kind, &secret))
                .await;

            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this.session_mut(session_id) {
                    session.connection_message = Some(match result {
                        Ok(Ok(())) => "Credential saved in the system keychain".into(),
                        Ok(Err(error)) => {
                            format!("Connected, but the credential could not be saved: {error}")
                        }
                        Err(error) => {
                            format!("Connected, but the system keychain task failed: {error}")
                        }
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect_active_connection(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session_id else {
            return;
        };
        self.disconnect_session(session_id, cx);
    }

    fn disconnect_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let should_remove = {
            let Some(session) = self.session_mut(session_id) else {
                return;
            };
            if !session.connection_state.can_disconnect() {
                return;
            }

            session.terminal_resize_task = None;

            if let Some(handle) = session.connection_handle.as_ref() {
                if let Err(error) = handle.disconnect() {
                    session.connection_state = SessionState::Failed;
                    session.connection_handle = None;
                    session.connection_error = Some(error.to_string());
                    session.close_when_disconnected
                } else {
                    // Disable repeated clicks before the worker publishes its event.
                    session.connection_state = SessionState::Disconnecting;
                    session.terminal_end_reason = Some("Session disconnected".into());
                    false
                }
            } else {
                session.connection_state = SessionState::Failed;
                session.connection_error = Some("SSH connection handle is missing".into());
                session.close_when_disconnected
            }
        };

        if should_remove {
            self.remove_session(session_id, cx);
        }

        cx.notify();
    }

    fn terminal_modes(&self) -> TerminalModes {
        self.active_session()
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

    fn terminal_point_for_position(&self, position: gpui::Point<Pixels>) -> Option<TerminalPoint> {
        let terminal = self.active_session()?.terminal.as_ref()?;
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
        focus_handle.focus(window);

        let Some(point) = self.terminal_point_for_position(event.position) else {
            return;
        };

        let Some(session) = self.active_session_mut() else {
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
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .active_session()
            .is_some_and(|session| session.terminal_selecting)
            || !event.dragging()
        {
            return;
        }

        let Some(point) = self.terminal_point_for_position(event.position) else {
            return;
        };
        let Some(selection) = self
            .active_session_mut()
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

    fn on_terminal_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_mut() else {
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

    fn copy_terminal_selection(&self, cx: &mut Context<Self>) -> bool {
        let Some(session) = self.active_session() else {
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

    fn clear_terminal_selection(&mut self) -> bool {
        let Some(session) = self.active_session_mut() else {
            return false;
        };
        let had_selection = session.terminal_selection.take().is_some();
        let was_selecting = std::mem::take(&mut session.terminal_selecting);
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

    fn send_terminal_input(&mut self, data: Vec<u8>, cx: &mut Context<Self>) {
        let Some(session) = self.active_session_mut() else {
            return;
        };
        if data.is_empty() || session.connection_state != SessionState::Connected {
            return;
        }

        let Some(handle) = session.connection_handle.as_ref() else {
            return;
        };

        if let Err(error) = handle.send_input(data) {
            session.connection_error = Some(error.to_string());
            cx.notify();
        }
    }

    fn send_terminal_user_input(&mut self, data: Vec<u8>, cx: &mut Context<Self>) {
        if data.is_empty() {
            return;
        }

        let selection_cleared = self.clear_terminal_selection();

        if let Some(terminal) = self
            .active_session_mut()
            .and_then(|session| session.terminal.as_mut())
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

                let Some(handle) = session.connection_handle.as_ref() else {
                    return;
                };

                if let Err(error) = handle.resize(size) {
                    session.connection_error = Some(error.to_string());
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

        let line_height = self
            .active_session()
            .and_then(|session| session.terminal.as_ref())
            .map(|terminal| terminal.cell_height)
            .unwrap_or(f32::from(TERMINAL_CELL_HEIGHT));
        let delta = f32::from(event.delta.pixel_delta(px(line_height)).y);
        if delta == 0.0 {
            return;
        }
        cx.stop_propagation();

        let Some(session) = self.active_session_mut() else {
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

        let modes = self.terminal_modes();
        let display_offset = self
            .active_session()
            .and_then(|session| session.terminal.as_ref())
            .map(|terminal| terminal.engine.display_offset())
            .unwrap_or_default();
        let alternate_scroll = should_translate_alternate_scroll(modes, display_offset);

        if alternate_scroll {
            self.clear_terminal_selection();
            self.send_terminal_input(encode_alternate_scroll(lines, modes), cx);
        } else if let Some(session) = self.active_session_mut() {
            session.terminal_selection = None;
            session.terminal_selecting = false;
            let Some(terminal) = session.terminal.as_mut() else {
                return;
            };
            terminal.engine.scroll(TerminalScroll::Lines(lines));
            cx.notify();
        }
    }

    fn process_terminal_output(
        &mut self,
        session_id: SessionId,
        data: &[u8],
        cx: &mut Context<Self>,
    ) {
        let events = {
            let Some(session) = self.session_mut(session_id) else {
                return;
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

        for event in events {
            self.handle_terminal_event(session_id, event, cx);
        }
    }

    fn handle_terminal_event(
        &mut self,
        session_id: SessionId,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            TerminalEvent::TitleChanged(title) => {
                if let Some(terminal) = self
                    .session_mut(session_id)
                    .and_then(|session| session.terminal.as_mut())
                {
                    terminal.title = title;
                }
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
                self.send_terminal_response(session_id, request.response(&contents));
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
            }
            TerminalEvent::WriteToPty(data) => self.send_terminal_response(session_id, data),
            TerminalEvent::TextAreaSizeRequest(request) => {
                let size = self
                    .session(session_id)
                    .and_then(|session| session.terminal.as_ref())
                    .map(ActiveTerminal::text_area_size);
                if let Some(size) = size {
                    self.send_terminal_response(session_id, request.response(size));
                }
            }
            TerminalEvent::Bell => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some("Remote terminal bell".into());
                }
            }
            TerminalEvent::ExitRequested => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some("Remote terminal requested exit".into());
                    session.terminal_end_reason = Some("Remote terminal requested exit".into());
                }
            }
            TerminalEvent::ChildExited(status) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.terminal_end_reason = status
                        .map(|status| format!("Remote terminal exited with status {status}"))
                        .or_else(|| Some("Remote terminal exited".into()));
                    session
                        .connection_message
                        .clone_from(&session.terminal_end_reason);
                }
            }
            TerminalEvent::MouseCursorDirty
            | TerminalEvent::CursorBlinkingChanged
            | TerminalEvent::Wakeup => {}
        }
    }

    fn send_terminal_response(&mut self, session_id: SessionId, data: Vec<u8>) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        let Some(handle) = session.connection_handle.as_ref() else {
            return;
        };

        if let Err(error) = handle.send_input(data) {
            session.connection_error = Some(error.to_string());
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
                        session.connection_credential = None;
                        session.sftp.stop_loading();
                        session.sidebar_sftp.stop_loading();
                        session.transfers.fail_pending("SSH connection closed");
                        session.performance.clear_connection();
                        if previous_state == SessionState::Disconnecting
                            && session.terminal_end_reason.is_none()
                        {
                            session.terminal_end_reason = Some("Session disconnected".into());
                        }
                    }

                    state == SessionState::Disconnected && session.close_when_disconnected
                };

                if state == SessionState::Connected {
                    self.save_successful_credential(session_id, cx);
                } else if close_when_disconnected {
                    self.remove_session(session_id, cx);
                }
                self.sync_performance_monitoring();

                true
            }
            ConnectionEvent::HostKeyVerificationRequired(info) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message =
                        Some(format!("Verify host key for {}", info.address()));
                    session.host_key_prompt = Some(info);
                }
                self.activate_session(session_id, cx);
                true
            }
            ConnectionEvent::Failed(error) => {
                let (failed_profile_id, failed_credential, close_when_disconnected) = {
                    let session = self
                        .session_mut(session_id)
                        .expect("checked session should still exist");
                    let profile_id = session.profile_id.clone();
                    let credential = session.connection_credential.take();
                    session.connection_state = SessionState::Failed;
                    session.terminal_resize_task = None;
                    session.connection_handle = None;
                    session.host_key_prompt = None;
                    session.sftp.stop_loading();
                    session.sidebar_sftp.stop_loading();
                    session.transfers.fail_pending("SSH connection failed");
                    session.performance.clear_connection();
                    (profile_id, credential, session.close_when_disconnected)
                };

                if close_when_disconnected {
                    self.remove_session(session_id, cx);
                    return;
                }
                self.sync_performance_monitoring();

                let authentication_error = error.to_string();
                let prompted_for_credential =
                    match (failed_profile_id, failed_credential, error.kind()) {
                        (
                            profile_id,
                            Some(credential),
                            SshErrorKind::Authentication | SshErrorKind::PrivateKeyPassphrase,
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
                        _ => false,
                    };

                if !prompted_for_credential && let Some(session) = self.session_mut(session_id) {
                    session.connection_error = Some(error.to_string());
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
            ConnectionEvent::SftpFailed {
                request_id,
                path: _,
                operation,
                error,
            } => {
                let transfer_failed = if let Some(session) = self.session_mut(session_id) {
                    match operation {
                        SftpOperation::ReadDirectory => {
                            let placement = sftp_browser_placement_for_request(request_id);
                            session
                                .sftp_browser_mut(placement)
                                .fail_request(request_id, error.to_string());
                            false
                        }
                        SftpOperation::ReadDirectoryTree => {
                            let placement = sftp_browser_placement_for_request(request_id);
                            session
                                .sftp_browser_mut(placement)
                                .fail_request(request_id, error.to_string());
                            false
                        }
                        SftpOperation::ReadFile | SftpOperation::WriteFile => {
                            session.sftp.fail_file_request(
                                request_id,
                                operation,
                                error.to_string(),
                            );
                            false
                        }
                        SftpOperation::CreateFile
                        | SftpOperation::CreateDirectory
                        | SftpOperation::DeletePaths => {
                            let placement = sftp_browser_placement_for_request(request_id);
                            session.sftp_browser_mut(placement).error = Some(error.to_string());
                            false
                        }
                        SftpOperation::UploadFile
                        | SftpOperation::DownloadFile
                        | SftpOperation::CancelTransfer => {
                            session.transfers.mark_failed(request_id, error.to_string());
                            true
                        }
                    }
                } else {
                    false
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
                if let Some(session) = self.session_mut(session_id) {
                    session.performance.loading = false;
                    session.performance.error = Some(error.to_string());
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
                self.process_terminal_output(session_id, &data, cx);
                true
            }
            ConnectionEvent::Shell(ShellEvent::ExitStatus(status)) => {
                let message = format!("Remote shell exited with status {status}");
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
                let core_dump = if core_dumped { " (core dumped)" } else { "" };
                let message =
                    format!("Remote shell exited on signal {signal}{core_dump}: {message}");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Eof) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some("Remote shell reached EOF".into());
                    if session.terminal_end_reason.is_none() {
                        session.terminal_end_reason = Some("Remote shell reached EOF".into());
                    }
                }
                true
            }
            ConnectionEvent::Shell(ShellEvent::Closed) => {
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some("Remote shell closed".into());
                    if session.terminal_end_reason.is_none() {
                        session.terminal_end_reason = Some("Remote shell closed".into());
                    }
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
        let right_sidebar_width = self.effective_right_sidebar_width(window);
        let sidebar_width = self.effective_sidebar_width(window);
        let should_focus_terminal = self.active_panel == ActivePanel::Connection
            && self.active_tab_view() == TerminalTabView::Terminal
            && !self.right_sidebar_open
            && selected_profile
                .as_ref()
                .is_some_and(|profile| self.is_terminal_visible(&profile.id));

        let mut root = div()
            .id("remcmd_root")
            .relative()
            .flex()
            .size_full()
            .text_color(self.theme.text_primary)
            .on_mouse_move(cx.listener(Self::resize_sidebar))
            .on_mouse_move(cx.listener(Self::resize_right_sidebar))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_sidebar_resize))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(Self::finish_right_sidebar_resize),
            );

        let rendered_left_sidebar_width = sidebar_width * self.left_sidebar_progress;
        root = root.child(
            div()
                .flex()
                .flex_none()
                .w(px(rendered_left_sidebar_width))
                .h_full()
                .overflow_hidden()
                .child(self.render_sidebar(sidebar_width, cx)),
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
        if right_sidebar_open {
            right_sidebar = right_sidebar.child(self.render_right_sidebar(right_sidebar_width, cx));
        }
        root = root.child(
            right_sidebar.with_animation(
                SharedString::from(format!(
                    "right-sidebar-layout-{right_transition_id}-{right_sidebar_open}"
                )),
                Animation::new(if right_transition_id == 0 {
                    Duration::from_millis(1)
                } else {
                    Duration::from_millis(180)
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
        if self.right_sidebar_open {
            root = root.child(self.render_right_sidebar_titlebar(right_sidebar_width, cx));
        }
        if self.left_sidebar_progress > 0.0 {
            root = root.child(self.render_sidebar_resize_handle(rendered_left_sidebar_width, cx));
        }
        if self.right_sidebar_open {
            root = root.child(self.render_right_sidebar_resize_handle(right_sidebar_width, cx));
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

        if self
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
        tooltip: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
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
                label: tooltip.into(),
                theme,
            })
            .into()
        })
    }

    fn render_titlebar_action_group(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let can_create_terminal = self.active_panel == ActivePanel::Connection
            && self.selected_profile_id.is_some()
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
                label: "New terminal".into(),
                theme,
            })
            .into()
        });
        if can_create_terminal {
            new_terminal = new_terminal.on_click(cx.listener(|this, _, window, cx| {
                this.connect_selected_profile_in_new_session(window, cx);
            }));
        }

        let right_sidebar = self
            .render_titlebar_sidebar_button("toggle_right_sidebar", false, "Toggle right sidebar")
            .on_click(cx.listener(|this, _, _, cx| this.toggle_right_sidebar(cx)));

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
            (RightSidebarView::Sftp, "SFTP", IconName::Folder),
            (
                RightSidebarView::Performance,
                "Performance",
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
            .top(px(-1.0))
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
                    Duration::from_millis(1)
                } else {
                    Duration::from_millis(180)
                })
                .with_easing(ease_in_out),
                move |this, delta| this.w(px(width * delta)).opacity(delta),
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
        let sftp_path = self
            .pane(tab.active_pane_id)
            .and_then(|pane| self.session(pane.session_id))
            .filter(|session| session.sftp.loaded)
            .map(|session| session.sftp.display_path());
        let remote_cwd = self
            .pane(tab.active_pane_id)
            .and_then(|pane| self.session(pane.session_id))
            .and_then(|session| session.terminal.as_ref())
            .and_then(|terminal| terminal.remote_cwd.as_deref());

        workspace_tab_title(tab.view, terminal_number, sftp_path, remote_cwd)
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

        titlebar.with_animation(
            SharedString::from(format!("titlebar-right-edge-{transition_id}-{open}")),
            Animation::new(if transition_id == 0 {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(180)
            })
            .with_easing(ease_in_out),
            move |this, delta| this.right(px(start_width + (end_width - start_width) * delta)),
        )
    }

    fn render_titlebar_tabs(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let drag_area = || {
            div()
                .flex_none()
                .h_full()
                .window_control_area(WindowControlArea::Drag)
        };
        let leading_width = self.titlebar_leading_width(window);
        let expanded_right_sidebar_width = self.effective_right_sidebar_width(window);
        let titlebar_right_inset = if self.right_sidebar_open {
            expanded_right_sidebar_width
        } else {
            0.0
        };
        let left_sidebar_button = self
            .render_titlebar_sidebar_button("toggle_left_sidebar", true, "Toggle sidebar")
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
        let leading = div()
            .flex()
            .flex_none()
            .items_center()
            .w(px(leading_width))
            .h_full()
            .child(drag_area().flex_1())
            .child(left_sidebar_group)
            .child(drag_area().w(px(TITLEBAR_LEFT_CONTROL_EDGE_GAP)));
        let titlebar = div()
            .id("window_titlebar")
            .absolute()
            .top(px(-1.0))
            .left_0()
            .right_0()
            .h(px(TITLEBAR_HEIGHT))
            .flex()
            .items_center()
            .child(leading);

        if self.tab_layout == TabLayout::Vertical {
            let titlebar = titlebar
                .child(drag_area().flex_1())
                .child(self.render_titlebar_action_group(cx))
                .child(drag_area().w(px(12.0)));
            return self.animate_titlebar_right_edge(titlebar, expanded_right_sidebar_width);
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

        for (tab_index, (tab, label)) in self.tabs.iter().zip(tab_labels).enumerate() {
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
                    Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()),
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
                            .child(label),
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
                Animation::new(Duration::from_millis(240)).with_easing(ease_in_out),
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
                        label: "Close terminal".into(),
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
                Animation::new(Duration::from_millis(120)).with_easing(ease_out_quint()),
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
                                Animation::new(Duration::from_millis(280)).with_easing(ease_in_out),
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
                Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
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
                    Animation::new(Duration::from_millis(300)).with_easing(ease_in_out),
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
            .px(px(12.0));
        if !self.tabs.is_empty() {
            controls = controls.child(tabs);
        } else {
            controls = controls.child(drag_area().flex_1());
        }

        let titlebar = titlebar.child(controls.child(self.render_titlebar_action_group(cx)));
        self.animate_titlebar_right_edge(titlebar, expanded_right_sidebar_width)
    }

    fn is_terminal_visible(&self, profile_id: &str) -> bool {
        self.active_session()
            .filter(|session| session.profile_id == profile_id)
            .is_some_and(TerminalSession::is_terminal_visible)
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
            if let Some(file) = self
                .session_mut(session_id)
                .and_then(|session| session.sftp.file.as_mut())
            {
                file.error =
                    Some("Save or revert your changes before closing this terminal".into());
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
        let palette = self.terminal_palette();
        let session_id = pane.session_id;
        let session = self.session(session_id);
        let cell_height = self
            .session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .map(|terminal| terminal.cell_height)
            .unwrap_or(f32::from(TERMINAL_CELL_HEIGHT));
        let model = session.and_then(|session| {
            session.terminal.as_ref().map(|terminal| {
                TerminalViewModel::from_snapshot_with_selection(
                    &terminal.snapshot(),
                    session.terminal_selection,
                    palette,
                )
            })
        });
        let input_entity = cx.entity();
        let layout_entity = input_entity.clone();
        let input_focus_handle = pane.focus_handle.clone();
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
                        this.apply_terminal_layout(session_id, bounds, layout, cx);
                    });
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let border = if self.active_pane_id == Some(pane_id) {
            self.theme.border_strong
        } else {
            self.theme.border
        };
        let terminal_view = div()
            .id(SharedString::from(format!("terminal-view-{}", pane_id.0)))
            .key_context("Terminal")
            .track_focus(&pane.focus_handle)
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .w_full()
            .p_3()
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .bg(rgb(palette.background.hex()))
            .font_family(TERMINAL_FONT_FAMILY)
            .text_size(px(14.0))
            .line_height(px(cell_height))
            .cursor(CursorStyle::IBeam)
            .focus(|style| style.border_color(self.theme.border_strong))
            .on_key_down(cx.listener(Self::on_terminal_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event, window, cx| {
                    this.on_terminal_mouse_down(pane_id, event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(Self::on_terminal_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_terminal_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_terminal_mouse_up))
            .on_scroll_wheel(cx.listener(move |this, event, window, cx| {
                this.on_terminal_scroll(pane_id, event, window, cx);
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
                        self.render_icon_button(
                            SharedString::from(format!("terminal-reconnect-{}", session_id.0)),
                            IconName::Reconnect,
                            "Reconnect",
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
                            "Close terminal",
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
            )
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
                    .child("Remember in system keychain")
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
                        "Cancel",
                        TextButtonTone::Secondary,
                        true,
                        &self.theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dismiss_credential_prompt(cx);
                        cx.notify();
                    })),
                )
                .child(
                    text_button(
                        "credential_submit",
                        "Connect",
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

    fn render_host_key_prompt(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self
            .active_session()
            .expect("host-key prompt requires an active session")
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
                        text_button(
                            "host_key_cancel",
                            "Cancel",
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
                            "Trust and Connect",
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

    fn render_sidebar(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.sidebar_search.read(cx).text().trim().to_lowercase();
        let list_hover_background = self.theme.list_hover_bg;
        let pressed_background = self.theme.control_pressed_bg;
        let mut connection_tree = div()
            .id("connection_list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .gap(px(2.0))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .mt_3();

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
                .px_2()
                .rounded_md()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(self.theme.text_muted)
                .cursor_pointer()
                .hover(move |this| this.bg(list_hover_background))
                .active(move |this| this.bg(pressed_background))
                .child(self.render_sidebar_icon(section_icon, 15.0))
                .child("Connections")
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
                let can_create_terminal = self.credential_lookup_task.is_none()
                    && !self
                        .credential_mutations_in_progress
                        .contains_key(&profile.id);
                let mut new_terminal_button = self
                    .render_icon_button(
                        SharedString::from(format!("new-terminal-{}", profile.id)),
                        IconName::Add,
                        "New terminal",
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
                let is_selected = self.active_panel == ActivePanel::Connection
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
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_profile(select_profile_id.clone(), window, cx);
                        })),
                );

                if self.tab_layout == TabLayout::Vertical {
                    for tab in self.tabs.iter().filter(|tab| tab.profile_id == profile.id) {
                        let tab_id = tab.id;
                        let terminal_title = self.terminal_tab_title(tab);
                        let is_active = self.active_panel == ActivePanel::Connection
                            && self.active_tab_id == Some(tab_id);
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
                        connection_tree = connection_tree.child(
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
                                        SharedString::from(format!(
                                            "close-sidebar-tab-{}",
                                            tab_id.0
                                        )),
                                        IconName::Cancel,
                                        "Close terminal",
                                        IconTone::Default,
                                        true,
                                    )
                                    .size(px(24.0))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.close_tab(tab_id, cx);
                                        },
                                    )),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this.activate_tab_in_window(tab_id, window, cx) {
                                        cx.notify();
                                    }
                                })),
                        );
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
                    .child("No matching connections"),
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
            .flex_none()
            .mt_3()
            .pt_3()
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .id("show_settings")
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .w_full()
                    .h(px(34.0))
                    .px_2()
                    .rounded_md()
                    .bg(settings_background)
                    .cursor_pointer()
                    .hover(move |this| this.bg(settings_hover))
                    .active(move |this| this.bg(pressed_background))
                    .child(self.render_sidebar_icon(IconName::Settings, 18.0))
                    .child("Settings")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_settings(window, cx);
                    })),
            );

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .h_full()
            .bg(self.theme.sidebar_bg)
            .px_3()
            .pb_4()
            .pt(px(TITLEBAR_HEIGHT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_none()
                    .h(px(34.0))
                    .child(
                        div()
                            .ml_2()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .child("RemCmd"),
                    )
                    .child(
                        self.render_icon_button(
                            "toggle_sidebar_search",
                            IconName::Search,
                            "Search connections",
                            IconTone::Default,
                            true,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sidebar_search(window, cx);
                        })),
                    ),
            )
            .when(self.sidebar_search_visible, |this| {
                this.child(
                    div()
                        .flex_none()
                        .mt_2()
                        .rounded_md()
                        .border_1()
                        .border_color(self.theme.border)
                        .bg(self.theme.surface_bg)
                        .child(self.sidebar_search.clone()),
                )
            })
            .child(
                div()
                    .id("add_connection")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(36.0))
                    .mt_3()
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |this| this.bg(list_hover_background))
                    .active(move |this| this.bg(pressed_background))
                    .child(self.render_sidebar_icon(IconName::NewConnection, 17.0))
                    .child("New Connection")
                    .on_click(cx.listener(|this, _, _, cx| this.add_profile(cx))),
            )
            .child(connection_tree)
            .child(settings_footer)
    }

    fn render_sidebar_resize_handle(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = self.theme.border_strong;
        let resting = if self.sidebar_resize.is_some() {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };

        div()
            .id("sidebar_resize_handle")
            .absolute()
            .top_0()
            .bottom_0()
            .left(px(width - SIDEBAR_RESIZE_HANDLE_WIDTH / 2.0))
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .bg(self.theme.transparent)
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(move |this| this.bg(hover))
            .child(div().w(px(1.0)).h_full().bg(resting))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_sidebar_resize))
    }

    fn render_right_sidebar_resize_handle(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hover = self.theme.border_strong;
        let transition_id = self.right_sidebar_transition_id;
        let start_width = if transition_id == 0 { width } else { 0.0 };
        let resting = if self.right_sidebar_resize.is_some() {
            self.theme.border_strong
        } else {
            self.theme.transparent
        };

        div()
            .id("right_sidebar_resize_handle")
            .absolute()
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .w(px(SIDEBAR_RESIZE_HANDLE_WIDTH))
            .bg(self.theme.transparent)
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(move |this| this.bg(hover))
            .child(div().w(px(1.0)).h_full().bg(resting))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::begin_right_sidebar_resize),
            )
            .with_animation(
                SharedString::from(format!("right-sidebar-resize-handle-{transition_id}")),
                Animation::new(if transition_id == 0 {
                    Duration::from_millis(1)
                } else {
                    Duration::from_millis(180)
                })
                .with_easing(ease_in_out),
                move |this, delta| {
                    let animated_width = start_width + (width - start_width) * delta;
                    this.right(px(animated_width - SIDEBAR_RESIZE_HANDLE_WIDTH / 2.0))
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
                .child("Connect to monitor this server")
                .into_any_element();
        }

        let performance = &session.performance;
        let Some(snapshot) = performance.snapshot.as_ref() else {
            let message = performance
                .error
                .as_deref()
                .unwrap_or("Collecting server metrics...");
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
                                "Retrying"
                            } else {
                                "Live, 2s"
                            }),
                    ),
            )
            .child(self.render_performance_meter(
                "CPU",
                cpu_usage,
                if performance.cpu_usage.is_some() {
                    format!("{cpu_usage:.0}%")
                } else {
                    "Collecting".into()
                },
                self.theme.accent,
            ))
            .child(self.render_performance_meter(
                "Memory",
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
                            .child("Load average")
                            .child(div().text_color(self.theme.text_muted).child(format!(
                                "{:.2}  {:.2}  {:.2}",
                                snapshot.load_one_milli as f32 / 1000.0,
                                snapshot.load_five_milli as f32 / 1000.0,
                                snapshot.load_fifteen_milli as f32 / 1000.0,
                            ))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(self.theme.text_faint)
                            .child(format!(
                                "1m / 5m / 15m, {} logical CPUs",
                                snapshot.cpu_count
                            )),
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
                            .child("Network"),
                    )
                    .child(
                        self.render_performance_value_row(
                            "Download",
                            performance
                                .network_rx_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| "Collecting".into()),
                        ),
                    )
                    .child(
                        self.render_performance_value_row(
                            "Upload",
                            performance
                                .network_tx_per_second
                                .map(format_byte_rate)
                                .unwrap_or_else(|| "Collecting".into()),
                        ),
                    ),
            );

        if let Some((disk_total, disk_available)) = disk {
            let disk_used = disk_total.saturating_sub(disk_available);
            let disk_usage = percent(disk_used, disk_total);
            content = content.child(self.render_performance_meter(
                "Root disk",
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
                            .child("System"),
                    )
                    .child(self.render_performance_value_row(
                        "Uptime",
                        format_uptime(snapshot.uptime_seconds),
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

    fn render_performance_meter(
        &self,
        label: &'static str,
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

    fn render_performance_value_row(&self, label: &'static str, value: String) -> gpui::Div {
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
                .child("No active terminal")
                .into_any_element()
        };

        div()
            .id("right_sidebar")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_w(px(0.0))
            .h_full()
            .pt(px(TITLEBAR_HEIGHT))
            .px_3()
            .pb_3()
            .border_l_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.sidebar_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(-1.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }])
            .child(content)
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
                                .child(self.render_workspace_controls(cx))
                                .child(self.render_pane_controls(cx))
                                .child(self.render_connection_controls(cx))
                                .child(
                                    self.render_icon_button(
                                        "delete_connection",
                                        IconName::Delete,
                                        "Delete",
                                        IconTone::Danger,
                                        true,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.delete_selected_profile(cx);
                                        },
                                    )),
                                ),
                        ),
                );

                if self.has_terminal_workspace(&profile.id) {
                    match self.active_tab_view() {
                        TerminalTabView::Terminal => {
                            if let Some(layout) = self.active_tab().map(|tab| &tab.layout) {
                                panel = panel.child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .min_h(px(0.0))
                                        .mt_4()
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
                        .when(editor.auth_kind != ProfileAuthKind::Agent, |this| {
                            this.child(self.render_saved_credential_row(cx))
                        })
                        .when_some(self.form_error.as_ref(), |this, error| {
                            this.child(
                                div()
                                    .mt_3()
                                    .text_color(self.theme.error_text)
                                    .child(error.clone()),
                            )
                        })
                        .when_some(
                            self.selected_session()
                                .and_then(|session| session.connection_error.as_ref()),
                            |this, error| {
                                this.child(
                                    div()
                                        .mt_3()
                                        .text_color(self.theme.error_text)
                                        .child(error.clone()),
                                )
                            },
                        )
                        .when_some(
                            self.selected_session()
                                .and_then(|session| session.connection_message.as_ref()),
                            |this, message| {
                                this.child(
                                    div()
                                        .mt_3()
                                        .text_color(self.theme.text_muted)
                                        .child(message.clone()),
                                )
                            },
                        )
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .flex_wrap()
                                .mt_6()
                                .gap_2()
                                .pb_2()
                                .child(
                                    text_button(
                                        "save_profile",
                                        "Save",
                                        TextButtonTone::Primary,
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
                                    text_button(
                                        "cancel_profile",
                                        "Cancel",
                                        TextButtonTone::Secondary,
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
            .pt(px(TITLEBAR_HEIGHT))
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
        let appearance_group = div()
            .flex()
            .flex_col()
            .w_full()
            .rounded_lg()
            .bg(self.theme.settings_group_bg)
            .child(self.render_settings_row(
                "settings-theme-row",
                "Theme",
                SettingsSelector::Theme,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-tab-layout-row",
                "Tab layout",
                SettingsSelector::TabLayout,
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
                "Speed limit",
                SettingsSelector::TransferRate,
                true,
                cx,
            ))
            .child(self.render_settings_row(
                "settings-parallel-files-row",
                "Parallel files",
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
                    .child("Appearance"),
            )
            .child(appearance_group)
            .child(
                div()
                    .w_full()
                    .mt_6()
                    .mb_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Transfers"),
            )
            .child(transfer_group)
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

    fn render_settings_row(
        &self,
        id: &'static str,
        label: &'static str,
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
        let is_open = self.open_settings_selector == Some(selector);
        let control_width = selector.control_width();
        let menu_width = selector.menu_width();
        let control_group: SharedString = format!("{}-control", selector.element_id()).into();
        let mut menu = div()
            .id(SharedString::from(format!(
                "{}-menu",
                selector.element_id()
            )))
            .absolute()
            .top(px(26.0))
            .right_0()
            .w(px(menu_width))
            .flex()
            .flex_col()
            .p(px(3.0))
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.modal_bg)
            .text_sm()
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(3.0)),
                blur_radius: px(12.0),
                spread_radius: px(-3.0),
            }])
            .occlude();
        let selected_value = self.settings_value(selector);
        let pressed_background = self.theme.control_pressed_bg;
        for (index, option) in selector.options().iter().copied().enumerate() {
            let is_selected = option.value == selected_value;
            let mut check = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.0));
            if is_selected {
                check = check.child(icon_with_color(IconName::Check, self.theme.on_accent, 15.0));
            }
            let option_hover = if is_selected {
                self.theme.accent_hover
            } else {
                self.theme.control_hover_bg
            };
            menu = menu.child(
                div()
                    .id(SharedString::from(format!(
                        "{}-option-{index}",
                        selector.element_id()
                    )))
                    .flex()
                    .items_center()
                    .h(px(24.0))
                    .px_1()
                    .rounded_lg()
                    .when(is_selected, |this| {
                        this.bg(self.theme.accent).text_color(self.theme.on_accent)
                    })
                    .cursor_pointer()
                    .hover(move |this| this.bg(option_hover))
                    .active(move |this| this.bg(pressed_background))
                    .child(check)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_center()
                            .child(option.label),
                    )
                    .child(div().flex_none().size(px(16.0)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.apply_settings_value(option.value, window, cx);
                    })),
            );
        }

        let button_hover = self.theme.control_hover_bg;
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
            .w(px(control_width))
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
            .active(move |this| this.bg(pressed_background))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
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
            .w(px(control_width))
            .child(button)
            .when(is_open, |this| this.child(deferred(menu).with_priority(10)))
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
                    "Split right",
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
                    "Split down",
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
                    "Close pane",
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
                "Terminal",
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
                "Remote files",
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

        let mut new_file = self.render_sftp_context_menu_item(
            "sftp-context-new-file",
            IconName::File,
            "New File",
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
        let mut new_folder = self.render_sftp_context_menu_item(
            "sftp-context-new-folder",
            IconName::Folder,
            "New Folder",
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
        let mut copy_path = self.render_sftp_context_menu_item(
            "sftp-context-copy-path",
            IconName::Copy,
            "Copy Path",
            IconTone::Default,
            !entries.is_empty(),
        );
        if !entries.is_empty() {
            copy_path = copy_path.on_click(cx.listener(move |this, _, _, cx| {
                this.copy_selected_sftp_paths(session_id, placement, cx);
            }));
        }
        let mut view = self.render_sftp_context_menu_item(
            "sftp-context-view",
            IconName::View,
            "View",
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            view = view.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, false, cx);
            }));
        }
        let mut edit = self.render_sftp_context_menu_item(
            "sftp-context-edit",
            IconName::Edit,
            "Edit",
            IconTone::Default,
            single_file.is_some(),
        );
        if single_file.is_some() {
            edit = edit.on_click(cx.listener(move |this, _, _, cx| {
                this.open_selected_sftp_file(session_id, placement, true, cx);
            }));
        }
        let mut download = self.render_sftp_context_menu_item(
            "sftp-context-download",
            IconName::Download,
            "Download",
            IconTone::Default,
            can_download,
        );
        if can_download {
            download = download.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let mut delete = self.render_sftp_context_menu_item(
            "sftp-context-delete",
            IconName::Delete,
            "Delete",
            IconTone::Danger,
            connected && !entries.is_empty(),
        );
        if connected && !entries.is_empty() {
            delete = delete.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        div()
            .id("sftp_context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(188.0))
            .flex()
            .flex_col()
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(self.theme.border_strong)
            .bg(self.theme.context_menu_bg)
            .shadow(vec![BoxShadow {
                color: self.theme.shadow,
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(4.0),
                spread_radius: px(-2.0),
            }])
            .occlude()
            .child(new_file)
            .child(new_folder)
            .child(self.render_sftp_context_menu_separator())
            .child(copy_path)
            .child(view)
            .child(edit)
            .child(download)
            .child(self.render_sftp_context_menu_separator())
            .child(delete)
            .into_any_element()
    }

    fn render_sftp_context_menu_item(
        &self,
        id: &'static str,
        icon_name: IconName,
        label: &'static str,
        tone: IconTone,
        enabled: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let (hover, pressed) = match tone {
            IconTone::Danger => (self.theme.danger, self.theme.danger_hover),
            IconTone::Accent | IconTone::Default => (self.theme.accent, self.theme.accent_hover),
        };
        let on_accent = self.theme.on_accent;
        let base_color = match tone {
            IconTone::Danger => self.theme.danger,
            IconTone::Accent => self.theme.accent,
            IconTone::Default => self.theme.text_primary,
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
                            .child(label),
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

    fn render_sftp_context_menu_separator(&self) -> gpui::Div {
        div().h(px(1.0)).mx_2().my_1().bg(self.theme.border)
    }

    fn render_sftp_create_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(prompt) = self.sftp_create_prompt.as_ref() else {
            return div().into_any_element();
        };
        let title = match prompt.kind {
            SftpCreateKind::File => "New File",
            SftpCreateKind::Directory => "New Folder",
        };
        let input = prompt.input.clone();
        let create = text_button(
            "submit_sftp_create",
            "Create",
            TextButtonTone::Primary,
            true,
            &self.theme,
        )
        .on_click(cx.listener(|this, _, _, cx| this.submit_sftp_create(cx)));
        let cancel = text_button(
            "cancel_sftp_create",
            "Cancel",
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
                div()
                    .w(px(360.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(self.theme.border_strong)
                    .bg(self.theme.modal_bg)
                    .shadow(vec![BoxShadow {
                        color: self.theme.shadow,
                        offset: point(px(0.0), px(6.0)),
                        blur_radius: px(24.0),
                        spread_radius: px(-5.0),
                    }])
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
                .child("Loading remote files...")
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
                .child("This directory is empty")
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
            "Parent directory",
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
            "Refresh",
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
            "Upload files",
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
            "Download selected",
            IconTone::Default,
            can_download,
        );
        if can_download {
            download_button = download_button.on_click(cx.listener(move |this, _, _, cx| {
                this.download_selected_sftp_entries(session_id, placement, cx);
            }));
        }
        let can_delete = connected && !loading && !selected_entries.is_empty();
        let mut delete_button = self.render_icon_button(
            SharedString::from(format!("sftp_delete_selected_{element_suffix}")),
            IconName::Delete,
            "Delete selected",
            IconTone::Danger,
            can_delete,
        );
        if can_delete {
            delete_button = delete_button.on_click(cx.listener(move |this, _, window, cx| {
                this.delete_selected_sftp_entries(session_id, placement, window, cx);
            }));
        }

        let mut browser =
            div()
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
                .bg(self.theme.panel_bg)
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
                        .child(
                            div().flex_1().min_w(px(0.0)).ml_2().child(
                                self.render_sftp_breadcrumbs(session_id, placement, &path, cx),
                            ),
                        )
                        .when(loading && loaded, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_sm()
                                    .text_color(self.theme.text_muted)
                                    .child("Loading..."),
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
            .font_family(TERMINAL_FONT_FAMILY)
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
        let tasks = self
            .session(session_id)
            .map(|session| session.transfers.tasks.clone())
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
            "Clear finished transfers",
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
                            .child("Transfers"),
                    )
                    .child(clear_button),
            );

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
                                "Replace",
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
                                "Cancel",
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
                            "Cancel transfer",
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
                                .child(task.status_text()),
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
            "Back to directory",
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
            "Revert",
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
            if saving { "Saving" } else { "Save" },
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
                .child("Loading remote file...");
        } else if binary {
            content = content
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(self.theme.text_muted)
                .child(self.render_sidebar_icon(IconName::File, 20.0))
                .child("Binary or non-UTF-8 files cannot be edited")
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
                            .font_family(TERMINAL_FONT_FAMILY)
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
                                .child("Modified"),
                        )
                    })
                    .when(!editable, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .mr_2()
                                .text_sm()
                                .text_color(self.theme.text_muted)
                                .child("Read Only"),
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
            _ if checking_keychain => "Checking",
            _ if updating_keychain => "Updating",
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
        let session = self.selected_session();
        if session.is_some_and(|session| {
            self.credential_lookup_task.is_some()
                && self.credential_lookup_session_id == Some(session.id)
        }) {
            return "Checking system keychain".into();
        }
        if self
            .selected_profile_id
            .as_deref()
            .is_some_and(|profile_id| {
                self.credential_mutations_in_progress
                    .contains_key(profile_id)
            })
        {
            return "Updating system keychain".into();
        }

        let state = match session
            .map(|session| session.connection_state)
            .unwrap_or(SessionState::Disconnected)
        {
            SessionState::Disconnected => "Disconnected",
            SessionState::Connecting => "Connecting",
            SessionState::Authenticating => "Authenticating",
            SessionState::Connected => "Connected",
            SessionState::Disconnecting => "Disconnecting",
            SessionState::Failed => "Failed",
        };

        let Some(profile_id) = session.map(|session| session.profile_id.as_str()) else {
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
                        self.render_icon_button(
                            "browse_private_key",
                            IconName::Folder,
                            "Browse",
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
            .mt_3()
            .child(
                div()
                    .flex_none()
                    .w(px(112.0))
                    .truncate()
                    .child("Credential"),
            )
            .child(
                self.render_icon_button(
                    "forget_saved_credential",
                    IconName::ForgetCredential,
                    "Forget",
                    IconTone::Danger,
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.forget_selected_credential(cx);
                })),
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
        let marked_text = &self.active_session()?.terminal_marked_text;
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
            .active_session()
            .map(|session| session.terminal_marked_text.encode_utf16().count())
            .unwrap_or_default();
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let len = self
            .active_session()
            .map(|session| session.terminal_marked_text.encode_utf16().count())
            .unwrap_or_default();
        (len != 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .active_session_mut()
            .map(|session| std::mem::take(&mut session.terminal_marked_text))
            .unwrap_or_default();
        self.send_terminal_user_input(text.into_bytes(), cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.active_session_mut() {
            session.terminal_marked_text.clear();
        }
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
        if let Some(session) = self.active_session_mut() {
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
        let terminal = self.active_session()?.terminal.as_ref()?;
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
            self.active_session()
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

fn clamp_sidebar_width(requested: f32, viewport_width: f32) -> f32 {
    let available_width = (viewport_width - MIN_DETAIL_PANEL_WIDTH - SIDEBAR_RESIZE_HANDLE_WIDTH)
        .clamp(0.0, SIDEBAR_MAX_WIDTH);

    if available_width < SIDEBAR_MIN_WIDTH {
        available_width
    } else {
        requested.clamp(SIDEBAR_MIN_WIDTH, available_width)
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

fn sftp_browser_placement_for_request(request_id: u64) -> SftpBrowserPlacement {
    if request_id >= SIDEBAR_SFTP_REQUEST_ID_START {
        SftpBrowserPlacement::Sidebar
    } else {
        SftpBrowserPlacement::Center
    }
}

fn estimated_titlebar_label_width(label: &str) -> f32 {
    label
        .chars()
        .map(|character| if character.is_ascii() { 8.5 } else { 14.5 })
        .sum::<f32>()
        .max(20.0)
}

fn workspace_tab_title(
    view: TerminalTabView,
    terminal_number: usize,
    sftp_path: Option<&str>,
    remote_cwd: Option<&str>,
) -> String {
    match view {
        TerminalTabView::Terminal => remote_cwd
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Terminal {terminal_number}")),
        TerminalTabView::Files => sftp_path.or(remote_cwd).unwrap_or("Files").to_owned(),
    }
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
) -> std::io::Result<Vec<(PathBuf, String)>> {
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
            Ok((local_path, file.path))
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
            traffic_light_position: Some(point(
                px(TRAFFIC_LIGHT_INSET_X),
                px(TRAFFIC_LIGHT_INSET_Y),
            )),
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

fn launch(cx: &mut App) {
    cx.set_global(SshRuntime::new().expect("failed to create SSH runtime"));

    bind_text_field_keys(cx);
    bind_file_editor_keys(cx);
    bind_credential_prompt_keys(cx);
    bind_host_key_prompt_keys(cx);
    bind_settings_selector_keys(cx);
    bind_sftp_create_prompt_keys(cx);
    open_main_window(cx);
    cx.activate(true);
}

fn main() {
    Application::new().with_assets(RemCmdAssets).run(launch);
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(SettingsSelector::Theme.options(), &THEME_SETTING_OPTIONS);
        assert_eq!(
            SettingsSelector::TabLayout.options(),
            &TAB_LAYOUT_SETTING_OPTIONS
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
    fn performance_state_calculates_cpu_and_network_deltas() {
        let started = Instant::now();
        let mut performance = ServerPerformanceState::default();
        performance.update(performance_snapshot(1_000, 700, 1_000, 2_000), started);
        performance.update(
            performance_snapshot(1_200, 750, 5_000, 8_000),
            started + Duration::from_secs(2),
        );

        assert_eq!(performance.cpu_usage, Some(75.0));
        assert_eq!(performance.network_rx_per_second, Some(2_000.0));
        assert_eq!(performance.network_tx_per_second, Some(3_000.0));
        assert!(!performance.loading);
        assert!(performance.error.is_none());
    }

    #[test]
    fn performance_formatting_uses_compact_units() {
        assert_eq!(format_byte_rate(1536.0), "1.5 KB/s");
        assert_eq!(format_uptime(61), "1m");
        assert_eq!(format_uptime(90_061), "1d 1h 1m");
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
                TerminalTabView::Files,
                1,
                Some("/home/test"),
                Some("/ignored")
            ),
            "/home/test"
        );
        assert_eq!(
            workspace_tab_title(TerminalTabView::Files, 1, None, Some("/var/log")),
            "/var/log"
        );
        assert_eq!(
            workspace_tab_title(TerminalTabView::Terminal, 2, Some("/ignored"), None),
            "Terminal 2"
        );
        assert_eq!(
            workspace_tab_title(TerminalTabView::Terminal, 2, None, Some("/srv/app")),
            "/srv/app"
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
            cpu_count: 4,
            memory_total_bytes: 8 * 1024 * 1024 * 1024,
            memory_available_bytes: 4 * 1024 * 1024 * 1024,
            load_one_milli: 100,
            load_five_milli: 200,
            load_fifteen_milli: 300,
            network_rx_bytes,
            network_tx_bytes,
            disk_total_bytes: Some(100 * 1024 * 1024 * 1024),
            disk_available_bytes: Some(50 * 1024 * 1024 * 1024),
            uptime_seconds: 3_600,
        }
    }

    #[test]
    fn sftp_transfer_queue_tracks_multiple_active_tasks_and_conflicts_release_slots() {
        let mut queue = SftpTransferQueue::default();
        let upload = queue.enqueue(
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
        );
        let download = queue.enqueue(
            SftpTransferDirection::Download,
            PathBuf::from("/tmp/download.txt"),
            "/remote/download.txt".into(),
            false,
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
        let transfer = queue.enqueue(
            SftpTransferDirection::Upload,
            PathBuf::from("/tmp/upload.txt"),
            "/remote/upload.txt".into(),
            false,
        );

        assert_eq!(queue.begin_cancel(transfer), Some(false));
        assert_eq!(
            queue.task_mut(transfer).map(|task| task.state),
            Some(SftpTransferState::Cancelled)
        );
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
