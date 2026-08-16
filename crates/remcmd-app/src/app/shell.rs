#[cfg(target_os = "macos")]
use super::img;
use super::{
    Animation, AnyElement, AnyView, App, Bounds, BoxShadow, ConnectionProfile, Context,
    CursorStyle, FontWeight, Hsla, IconName, IconTone, IntoElement, LOCAL_PROFILE_ID, Localizer,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ProfileContextMenu,
    RemCmdApp, Render, ServerPerformanceSnapshot, SessionId, SessionState, SftpBrowserPlacement,
    SharedString, SplitAxis, TabLayout, TerminalSession, TerminalTab, TerminalTabView,
    TextButtonTone, Theme, Timer, TitlebarOptions, WINDOWS_CHROME_HEIGHT, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, app_icon, clamp_bottom_panel_height,
    div, ease_in_out, ease_out_quint, format_remote_size, icon, icon_button, icon_with_color,
    point, profile_auth_label, px, size, text_button, wordmark,
};
use gpui::AnimationExt;
use gpui::prelude::*;
use std::time::{Duration, Instant};

const TITLEBAR_TAB_ICON_ONLY_WIDTH: f32 = 44.0;
const TITLEBAR_TAB_ELLIPSIS_MIN_WIDTH: f32 = 56.0;

impl RemCmdApp {
    pub(super) fn effective_sidebar_width(&self, window: &Window) -> f32 {
        clamp_sidebar_width(self.sidebar_width, f32::from(window.viewport_size().width))
    }

    pub(super) fn effective_right_sidebar_width(&self, window: &Window) -> f32 {
        let viewport_width = f32::from(window.viewport_size().width);
        let left_sidebar_width = if self.left_sidebar_open {
            clamp_sidebar_width(self.sidebar_width, viewport_width)
        } else {
            0.0
        };
        clamp_right_sidebar_width(self.right_sidebar_width, viewport_width, left_sidebar_width)
    }

    pub(super) fn titlebar_leading_width(&self, window: &Window) -> f32 {
        if self.left_sidebar_open {
            self.effective_sidebar_width(window)
        } else {
            COLLAPSED_TITLEBAR_LEADING_WIDTH
        }
    }

    pub(super) fn toggle_left_sidebar(&mut self, cx: &mut Context<Self>) {
        self.left_sidebar_open = !self.left_sidebar_open;
        self.left_sidebar_transition_id += 1;
        self.sidebar_resize = None;
        cx.notify();
    }

    pub(super) fn begin_sidebar_resize(
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

    pub(super) fn resize_sidebar(
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

    pub(super) fn finish_sidebar_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_resize.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn begin_right_sidebar_resize(
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

    pub(super) fn resize_right_sidebar(
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

    pub(super) fn finish_right_sidebar_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.right_sidebar_resize.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn begin_bottom_panel_resize(
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

    pub(super) fn resize_bottom_panel(
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

    pub(super) fn finish_bottom_panel_resize(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.bottom_panel_resize.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn toggle_right_sidebar(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn set_right_sidebar_view(
        &mut self,
        view: RightSidebarView,
        cx: &mut Context<Self>,
    ) {
        self.right_sidebar_view = view;
        if view == RightSidebarView::Sftp
            && let Some(session_id) = self.active_session_id
        {
            self.ensure_sftp_directory(session_id, SftpBrowserPlacement::Sidebar, cx);
        }
        self.sync_performance_monitoring();
        cx.notify();
    }

    pub(super) fn sync_performance_monitoring(&mut self) {
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

    pub(super) fn set_active_tab_view(
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

    pub(super) fn show_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn show_about(&mut self, cx: &mut Context<Self>) {
        if let Some(window_handle) = self.about_window
            && window_handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }

        let options = about_window_options(cx, &self.localizer);
        let language_mode = self.language_mode;
        match cx.open_window(options, |_, cx| {
            cx.new(|_| AboutWindow {
                localizer: Localizer::new(language_mode),
            })
        }) {
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

    pub(super) fn open_profile_context_menu(
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

    pub(super) fn render_icon_button(
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

    pub(super) fn render_sidebar_icon(&self, icon_name: IconName, size: f32) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(20.0))
            .child(icon(icon_name, self.theme, IconTone::Default, size))
    }

    pub(super) fn render_titlebar_close_symbol(&self) -> AnyElement {
        #[cfg(target_os = "macos")]
        if let Some(symbol) = crate::macos_symbols::close_circle(self.theme.panel_bg.l < 0.5) {
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

    pub(super) fn render_titlebar_sidebar_symbol(&self, left: bool) -> AnyElement {
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

    pub(super) fn render_titlebar_sidebar_button(
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

    pub(super) fn render_titlebar_action_group(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn titlebar_control_shadow(&self) -> Vec<BoxShadow> {
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

    pub(super) fn render_right_sidebar_titlebar(
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

    pub(super) fn terminal_tab_title(&self, tab: &TerminalTab) -> String {
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

    pub(super) fn animate_titlebar_right_edge(
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

    pub(super) fn render_titlebar_tabs(
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
                Animation::new(MOTION_EMPHASIZED_DURATION).with_easing(ease_in_out),
                move |this, delta| {
                    this.opacity(content_start_opacity + (1.0 - content_start_opacity) * delta)
                },
            );

            let tooltip_theme = self.theme;
            let close_tooltip = close_terminal_tooltip.clone();
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
                        label: close_tooltip.clone().into(),
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

    pub(super) fn render_sidebar_wordmark(&self) -> gpui::Div {
        div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(24.0))
            .ml_2()
            .child(wordmark(self.theme, 108.0, 24.0))
    }

    pub(super) fn render_sidebar(&self, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn render_sidebar_terminal_tab(
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

    pub(super) fn glass_sidebar_surface(&self) -> gpui::Div {
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

    pub(super) fn glass_floating_surface(&self) -> gpui::Div {
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

    pub(super) fn render_sidebar_resize_handle(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(super) fn render_right_sidebar_resize_handle(
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

    pub(super) fn render_server_performance(&self, session_id: SessionId) -> AnyElement {
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
                .child(message)
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

    pub(super) fn render_logical_cpu_usage(
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

    pub(super) fn render_performance_meter(
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

    pub(super) fn render_performance_value_row(
        &self,
        label: SharedString,
        value: String,
    ) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .text_sm()
            .child(div().text_color(self.theme.text_muted).child(label))
            .child(div().min_w(px(0.0)).truncate().child(value))
    }

    pub(super) fn render_right_sidebar(
        &self,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(super) fn render_home(&self, cx: &mut Context<Self>) -> gpui::Div {
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

    pub(super) fn render_server_overview(
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
                                        .child(error.render(&self.localizer)),
                                )
                            },
                        ),
                ),
        )
    }

    pub(super) fn render_server_info_row(
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

    pub(super) fn render_detail_panel(
        &self,
        selected_profile: Option<ConnectionProfile>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        match self.active_panel {
            ActivePanel::Home => return self.render_home(cx),
            ActivePanel::Server => return self.render_server_overview(selected_profile, cx),
            ActivePanel::Settings => return self.render_settings(cx),
            ActivePanel::Diagnostics => return self.render_diagnostics(cx),
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

    pub(super) fn render_local_terminal_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
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

    pub(super) fn detail_panel_shell(&self) -> gpui::Div {
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

    pub(super) fn render_pane_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn render_workspace_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn render_context_menu_item(
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

    pub(super) fn render_context_menu_separator(&self) -> gpui::Div {
        div().h(px(1.0)).mx_2().my_1().bg(self.theme.border)
    }
}

pub(super) const MOTION_INSTANT_DURATION: Duration = Duration::from_millis(1);

pub(super) const MOTION_FAST_DURATION: Duration = Duration::from_millis(120);

pub(super) const MOTION_STANDARD_DURATION: Duration = Duration::from_millis(180);

pub(super) const MOTION_EMPHASIZED_DURATION: Duration = Duration::from_millis(240);

pub(super) const SIDEBAR_DEFAULT_WIDTH: f32 = 300.0;

pub(super) const SIDEBAR_MIN_WIDTH: f32 = 220.0;

pub(super) const SIDEBAR_MAX_WIDTH: f32 = 480.0;

pub(super) const SIDEBAR_RESIZE_HANDLE_WIDTH: f32 = 6.0;

pub(super) const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 340.0;

pub(super) const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 260.0;

pub(super) const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 520.0;

pub(super) const MIN_DETAIL_PANEL_WIDTH: f32 = 180.0;

pub(super) const COLLAPSED_TITLEBAR_LEADING_WIDTH: f32 = 140.0;

pub(super) const TITLEBAR_HEIGHT: f32 = 52.0;

pub(super) const TITLEBAR_TAB_HEIGHT: f32 = 30.0;

pub(super) const TITLEBAR_TAB_GROUP_HEIGHT: f32 = 36.0;

pub(super) const TITLEBAR_ACTION_GROUP_WIDTH: f32 = 112.0;

pub(super) const TITLEBAR_CONTROL_HOVER_SIZE: f32 = 28.0;

pub(super) const TITLEBAR_ADD_ICON_SIZE: f32 = 16.0;

pub(super) const TITLEBAR_SIDEBAR_ICON_SIZE: f32 = 20.0;

pub(super) const TITLEBAR_LEFT_CONTROL_EDGE_GAP: f32 = 10.0;

pub(super) const TITLEBAR_EDGE_INSET: f32 = 12.0;

pub(super) const TITLEBAR_ACTIVE_TAB_GROWTH: f32 = 36.0;

pub(super) const TITLEBAR_CLOSE_SYMBOL_SIZE: f32 = 12.0;

#[cfg(target_os = "macos")]
pub(super) const TRAFFIC_LIGHT_INSET_X: f32 = 20.0;

#[cfg(target_os = "macos")]
pub(super) const TRAFFIC_LIGHT_INSET_Y: f32 = 18.0;

pub(super) const fn platform_chrome_height() -> f32 {
    if cfg!(target_os = "windows") {
        WINDOWS_CHROME_HEIGHT
    } else {
        0.0
    }
}

pub(super) const fn content_top_inset() -> f32 {
    TITLEBAR_HEIGHT + platform_chrome_height()
}

pub(super) struct CommandTooltip {
    pub(super) label: SharedString,
    pub(super) theme: Theme,
}

pub(super) struct AboutWindow {
    pub(super) localizer: Localizer,
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
                            .child(
                                self.localizer
                                    .text_with("about-version", Some(&version_args)),
                            ),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(self.localizer.text("about-tagline")),
                    )
                    .child(
                        div()
                            .mt_5()
                            .text_xs()
                            .text_color(theme.text_faint)
                            .child(self.localizer.text("about-license")),
                    ),
            )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ActivePanel {
    #[default]
    Home,
    Server,
    Connection,
    Settings,
    Diagnostics,
    OpenSshImport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RightSidebarView {
    #[default]
    Sftp,
    Performance,
}

#[derive(Debug)]
pub(super) struct PerformanceCounters {
    pub(super) captured_at: Instant,
    pub(super) cpu_total: u64,
    pub(super) cpu_idle: u64,
    pub(super) cpu_iowait: u64,
    pub(super) logical_cpus: Vec<(u32, u64, u64)>,
    pub(super) network_rx_bytes: u64,
    pub(super) network_tx_bytes: u64,
    pub(super) disk_read_bytes: Option<u64>,
    pub(super) disk_write_bytes: Option<u64>,
}

#[derive(Default)]
pub(super) struct ServerPerformanceState {
    pub(super) snapshot: Option<ServerPerformanceSnapshot>,
    pub(super) previous: Option<PerformanceCounters>,
    pub(super) cpu_usage: Option<f32>,
    pub(super) cpu_iowait_usage: Option<f32>,
    pub(super) logical_cpu_usage: Vec<(u32, f32)>,
    pub(super) network_rx_per_second: Option<f64>,
    pub(super) network_tx_per_second: Option<f64>,
    pub(super) disk_read_per_second: Option<f64>,
    pub(super) disk_write_per_second: Option<f64>,
    pub(super) monitoring: bool,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
}

impl ServerPerformanceState {
    pub(super) fn update(&mut self, snapshot: ServerPerformanceSnapshot, captured_at: Instant) {
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

    pub(super) fn clear_connection(&mut self) {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarResize {
    pub(super) start_x: Pixels,
    pub(super) start_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BottomPanelResize {
    pub(super) start_y: Pixels,
    pub(super) start_height: f32,
}

pub(super) const fn session_state_key(state: SessionState) -> &'static str {
    match state {
        SessionState::Disconnected => "connection-status-disconnected",
        SessionState::Connecting => "connection-status-connecting",
        SessionState::Authenticating => "connection-status-authenticating",
        SessionState::Connected => "connection-status-connected",
        SessionState::Disconnecting => "connection-status-disconnecting",
        SessionState::Failed => "connection-status-failed",
    }
}

pub(super) fn clamp_sidebar_width(requested: f32, viewport_width: f32) -> f32 {
    let available_width = (viewport_width - MIN_DETAIL_PANEL_WIDTH - SIDEBAR_RESIZE_HANDLE_WIDTH)
        .clamp(0.0, SIDEBAR_MAX_WIDTH);

    if available_width < SIDEBAR_MIN_WIDTH {
        available_width
    } else {
        requested.clamp(SIDEBAR_MIN_WIDTH, available_width)
    }
}

pub(super) fn clamp_right_sidebar_width(
    requested: f32,
    viewport_width: f32,
    left_sidebar_width: f32,
) -> f32 {
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

pub(super) fn estimated_titlebar_label_width(label: &str) -> f32 {
    label
        .chars()
        .map(|character| if character.is_ascii() { 8.5 } else { 14.5 })
        .sum::<f32>()
        .max(20.0)
}

pub(super) fn workspace_tab_title(
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

pub(super) fn format_byte_rate(bytes_per_second: f64) -> String {
    let bytes = bytes_per_second.max(0.0) as u64;
    format!("{}/s", format_remote_size(bytes))
}

pub(super) fn format_uptime(seconds: u64) -> String {
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

pub(super) fn format_response_time(duration: Duration) -> String {
    let milliseconds = duration.as_secs_f64() * 1_000.0;
    if milliseconds < 1.0 {
        "<1 ms".into()
    } else if milliseconds < 1_000.0 {
        format!("{milliseconds:.0} ms")
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

pub(super) fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used.min(total) as f32 / total as f32 * 100.0
    }
}

pub(super) fn titlebar_active_tab_basis(
    track_width: f32,
    tab_count: usize,
    expanded_width: f32,
) -> f32 {
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

pub(super) fn about_window_options(cx: &App, _localizer: &Localizer) -> WindowOptions {
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

#[cfg(test)]
mod tests {
    use super::*;
    use remcmd_ssh::LogicalCpuSnapshot;

    #[test]
    fn sidebar_width_stays_within_layout_limits() {
        assert_eq!(clamp_sidebar_width(120.0, 1200.0), 220.0);
        assert_eq!(clamp_sidebar_width(600.0, 1200.0), 480.0);
        assert_eq!(clamp_sidebar_width(300.0, 720.0), 300.0);
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
    fn app_navigation_defaults_to_home() {
        assert_eq!(ActivePanel::default(), ActivePanel::Home);
    }
}
