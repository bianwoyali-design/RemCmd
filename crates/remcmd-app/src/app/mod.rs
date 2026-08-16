use crate::file_editor::{self, FileEditor, FileEditorEvent, bind_file_editor_keys};
use crate::i18n::Localizer;
use crate::icons::{IconName, RemCmdAssets, app_icon, icon, icon_with_color, wordmark};
use crate::pane_layout::{PaneId, PaneLayout, SplitAxis};
use crate::ssh_runtime::SshRuntime;
use crate::terminal_canvas::{
    TerminalCanvasCache, TerminalCanvasFrame, TerminalCanvasInput, TerminalCellMetrics,
};
use crate::terminal_input::{
    encode_alternate_scroll, encode_focus, encode_key, encode_paste,
    should_translate_alternate_scroll,
};
use crate::terminal_view::{TerminalPalette, palette_color};
use crate::text_field::{self, TextField, bind_text_field_keys};
use crate::theme::{IconTone, TextButtonTone, Theme, icon_button, set_global_theme, text_button};

mod bootstrap;
mod connection_flow;
mod diagnostics;
mod menus;
mod openssh_import;
mod profiles;
mod quick_commands;
mod settings;
#[path = "sftp/operations.rs"]
mod sftp_operations;
#[path = "sftp/state.rs"]
mod sftp_state;
#[path = "sftp/view.rs"]
mod sftp_view;
mod shell;
mod terminal_session;
mod workspace;

pub use bootstrap::run;
use connection_flow::{
    ConnectionCredential, CredentialPrompt, CredentialPromptKind, PendingConnectionPreparation,
    ProxyCommandApprovalPrompt, connection_stage_label, localized_connection_error_parts,
};
use menus::{WINDOWS_CHROME_HEIGHT, WindowsMenu, application_menus, configure_application_menu};
use profiles::{ProfileAuthKind, ProfileContextMenu, ProfileEditor, profile_auth_label};
use quick_commands::{BOTTOM_PANEL_DEFAULT_HEIGHT, QuickCommandPrompt, clamp_bottom_panel_height};
use settings::{
    SettingsSelector, UI_MONOSPACE_FONT_FAMILY, normalize_terminal_font_families,
    resolve_terminal_font_family,
};
use sftp_state::{
    SIDEBAR_SFTP_REQUEST_ID_START, SftpAvailability, SftpBrowserPlacement, SftpBrowserState,
    SftpContextMenu, SftpCreateKind, SftpCreatePrompt, SftpTransferQueue, format_remote_size,
    sftp_browser_placement_for_request,
};
use shell::{
    AboutWindow, ActivePanel, BottomPanelResize, CommandTooltip, MOTION_INSTANT_DURATION,
    MOTION_STANDARD_DURATION, RIGHT_SIDEBAR_DEFAULT_WIDTH, RightSidebarView, SIDEBAR_DEFAULT_WIDTH,
    ServerPerformanceState, SidebarResize, TRAFFIC_LIGHT_INSET_X, TRAFFIC_LIGHT_INSET_Y,
    content_top_inset, platform_chrome_height, session_state_key,
};
use terminal_session::{
    ActiveTerminal, LOCAL_PROFILE_ID, SessionMessage, SessionMessageArg, TERMINAL_COLUMNS,
    TERMINAL_EVENT_BATCH_LIMIT, TERMINAL_REDRAW_INTERVAL, TERMINAL_ROWS, TerminalContextMenu,
    TerminalSession, TerminalSessionKind,
};
use workspace::{SessionId, TabId, TerminalPane, TerminalTab, TerminalTabView};

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use directories::BaseDirs;
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
use secrecy::SecretString;

use remcmd_core::{
    AuthConfig, ConnectionProfile, ConnectionRoute, LanguageMode, ProxyConfig, TabLayout,
    TerminalSettings, ThemeMode, TransferSettings,
};
use remcmd_diagnostics::{
    DiagnosticFilter, DiagnosticLevel, DiagnosticStore, Diagnostics, SupportBundleContext,
    default_log_directory, fallback_log_directory,
};
use remcmd_local::{LocalPtySize, LocalTerminal, LocalTerminalEvent, LocalTerminalHandle};
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
    diagnostic_level: Option<DiagnosticLevel>,
    diagnostic_module_filter: Entity<TextField>,
    diagnostic_text_filter: Entity<TextField>,
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

struct DiagnosticsGlobal(DiagnosticStore);

impl Global for DiagnosticsGlobal {}

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
        let diagnostic_module_filter =
            cx.new(|cx| TextField::new(cx, "", localizer.text("diagnostics-filter-module")));
        cx.observe(&diagnostic_module_filter, |_, _, cx| cx.notify())
            .detach();
        let diagnostic_text_filter =
            cx.new(|cx| TextField::new(cx, "", localizer.text("diagnostics-filter-text")));
        cx.observe(&diagnostic_text_filter, |_, _, cx| cx.notify())
            .detach();
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
            diagnostic_level: None,
            diagnostic_module_filter,
            diagnostic_text_filter,
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

    fn tr(&self, key: &str) -> String {
        self.localizer.text(key)
    }

    fn tr_with(&self, key: &str, args: &fluent_bundle::FluentArgs<'_>) -> String {
        self.localizer.text_with(key, Some(args))
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
