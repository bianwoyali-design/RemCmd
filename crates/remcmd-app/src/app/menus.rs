use super::{
    AnyElement, AnyView, App, CancelProfileEditor, CloseActivePane, CloseActiveTab, CloseWindow,
    CommandTooltip, ConnectSelectedProfile, Context, DisconnectActiveSession, IconName,
    IntoElement, KeyBinding, Localizer, Menu, MenuItem, MinimizeWindow, NewConnection,
    NewLocalTerminal, NewRemoteTerminal, Quit, RemCmdApp, RemCmdMainWindow, ResetActiveTerminal,
    RightSidebarView, SaveProfileEditor, SharedString, ShowAbout, ShowFilesView, ShowHome,
    ShowPerformanceSidebar, ShowSettings, ShowSftpSidebar, ShowTerminalView, SplitAxis,
    SplitHorizontal, SplitVertical, TerminalTabView, ToggleBottomPanel, ToggleConnectionSearch,
    ToggleFullscreen, ToggleLeftSidebar, Window, WindowControlArea, ZoomWindow, div, file_editor,
    icon_with_color, platform_chrome_height, px, text_field, wordmark,
};
use gpui::prelude::*;

impl RemCmdApp {
    pub(super) fn render_titlebar_drag_area(&self) -> gpui::Div {
        div().h_full().window_control_area(WindowControlArea::Drag)
    }

    pub(super) fn render_windows_chrome(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(super) fn render_windows_menu_button(
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

    pub(super) fn render_windows_menu_popup(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    label_key,
                    shortcut,
                    command,
                } => popup.child(self.render_windows_menu_item(
                    SharedString::from(format!("windows-menu-entry-{menu:?}-{index}")),
                    self.localizer.text(label_key).into(),
                    shortcut,
                    command,
                    cx,
                )),
                WindowsMenuEntry::Separator => popup.child(self.render_context_menu_separator()),
            };
        }

        popup.into_any_element()
    }

    pub(super) fn render_windows_menu_item(
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

    pub(super) fn windows_menu_command_enabled(&self, command: WindowsMenuCommand) -> bool {
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

    pub(super) fn execute_windows_menu_command(
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

    pub(super) fn dispatch_edit_command(
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

    pub(super) fn render_windows_titlebar_controls(
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
            .on_click(
                cx.listener(|_, _, window, _| crate::windows_chrome::toggle_maximize(window)),
            );
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
}

pub(super) const WINDOWS_CHROME_HEIGHT: f32 = 34.0;

pub(super) const WINDOWS_BRAND_WIDTH: f32 = 112.0;

pub(super) const WINDOWS_TITLEBAR_BUTTON_WIDTH: f32 = 46.0;

pub(super) const WINDOWS_TITLEBAR_CONTROLS_WIDTH: f32 = WINDOWS_TITLEBAR_BUTTON_WIDTH * 3.0;

pub(super) const WINDOWS_MENU_MIN_WIDTH: f32 = 180.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowsMenu {
    File,
    Edit,
    Terminal,
    View,
    Window,
    Help,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowsMenuCommand {
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
pub(super) enum WindowsMenuEntry {
    Item {
        label_key: &'static str,
        shortcut: &'static str,
        command: WindowsMenuCommand,
    },
    Separator,
}

pub(super) const fn windows_menu_button_width(menu: WindowsMenu) -> f32 {
    match menu {
        WindowsMenu::File | WindowsMenu::Edit | WindowsMenu::Help => 43.0,
        WindowsMenu::Terminal => 66.0,
        WindowsMenu::View => 45.0,
        WindowsMenu::Window => 64.0,
    }
}

pub(super) fn windows_menu_left(menu: WindowsMenu) -> f32 {
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

pub(super) fn windows_menu_popup_width(entries: &[WindowsMenuEntry], localizer: &Localizer) -> f32 {
    entries
        .iter()
        .filter_map(|entry| match entry {
            WindowsMenuEntry::Item {
                label_key,
                shortcut,
                ..
            } => Some(
                28.0 + estimated_windows_menu_text_width(&localizer.text(label_key))
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

pub(super) fn estimated_windows_menu_text_width(text: &str) -> f32 {
    text.chars()
        .map(|character| if character.is_ascii() { 7.25 } else { 12.0 })
        .sum()
}

pub(super) fn windows_menu_entries(menu: WindowsMenu) -> Vec<WindowsMenuEntry> {
    use WindowsMenuCommand as Command;
    use WindowsMenuEntry::{Item, Separator};

    match menu {
        WindowsMenu::File => vec![
            Item {
                label_key: "menu-new-connection",
                shortcut: "Ctrl+N",
                command: Command::NewConnection,
            },
            Item {
                label_key: "menu-new-local-terminal",
                shortcut: "Ctrl+T",
                command: Command::NewLocalTerminal,
            },
            Item {
                label_key: "menu-new-remote-terminal",
                shortcut: "Ctrl+Shift+T",
                command: Command::NewRemoteTerminal,
            },
            Separator,
            Item {
                label_key: "menu-connect",
                shortcut: "Ctrl+Enter",
                command: Command::ConnectSelectedProfile,
            },
            Item {
                label_key: "menu-disconnect",
                shortcut: "Ctrl+Shift+X",
                command: Command::DisconnectActiveSession,
            },
            Separator,
            Item {
                label_key: "menu-settings",
                shortcut: "Ctrl+,",
                command: Command::ShowSettings,
            },
            Item {
                label_key: "menu-exit",
                shortcut: "Ctrl+Q",
                command: Command::Quit,
            },
        ],
        WindowsMenu::Edit => vec![
            Item {
                label_key: "menu-undo",
                shortcut: "Ctrl+Z",
                command: Command::Edit(EditCommand::Undo),
            },
            Item {
                label_key: "menu-redo",
                shortcut: "Ctrl+Y",
                command: Command::Edit(EditCommand::Redo),
            },
            Separator,
            Item {
                label_key: "menu-cut",
                shortcut: "Ctrl+X",
                command: Command::Edit(EditCommand::Cut),
            },
            Item {
                label_key: "menu-copy",
                shortcut: "Ctrl+C",
                command: Command::Edit(EditCommand::Copy),
            },
            Item {
                label_key: "menu-paste",
                shortcut: "Ctrl+V",
                command: Command::Edit(EditCommand::Paste),
            },
            Item {
                label_key: "menu-select-all",
                shortcut: "Ctrl+A",
                command: Command::Edit(EditCommand::SelectAll),
            },
        ],
        WindowsMenu::Terminal => vec![
            Item {
                label_key: "menu-split-horizontal",
                shortcut: "Ctrl+D",
                command: Command::SplitHorizontal,
            },
            Item {
                label_key: "menu-split-vertical",
                shortcut: "Ctrl+Shift+D",
                command: Command::SplitVertical,
            },
            Separator,
            Item {
                label_key: "menu-show-terminal",
                shortcut: "Ctrl+1",
                command: Command::ShowTerminalView,
            },
            Item {
                label_key: "menu-show-remote-files",
                shortcut: "Ctrl+2",
                command: Command::ShowFilesView,
            },
            Separator,
            Item {
                label_key: "menu-reset-terminal",
                shortcut: "Ctrl+R",
                command: Command::ResetActiveTerminal,
            },
            Item {
                label_key: "menu-close-active-split",
                shortcut: "Ctrl+Alt+W",
                command: Command::CloseActivePane,
            },
            Item {
                label_key: "menu-close-active-tab",
                shortcut: "Ctrl+Shift+W",
                command: Command::CloseActiveTab,
            },
        ],
        WindowsMenu::View => vec![
            Item {
                label_key: "menu-home",
                shortcut: "Ctrl+Shift+H",
                command: Command::ShowHome,
            },
            Separator,
            Item {
                label_key: "menu-toggle-connections-sidebar",
                shortcut: "Ctrl+Shift+S",
                command: Command::ToggleLeftSidebar,
            },
            Item {
                label_key: "menu-search-connections",
                shortcut: "Ctrl+F",
                command: Command::ToggleConnectionSearch,
            },
            Separator,
            Item {
                label_key: "menu-show-remote-files-sidebar",
                shortcut: "Ctrl+Shift+F",
                command: Command::ShowSftpSidebar,
            },
            Item {
                label_key: "menu-show-server-performance",
                shortcut: "Ctrl+Shift+P",
                command: Command::ShowPerformanceSidebar,
            },
            Item {
                label_key: "menu-toggle-bottom-terminal",
                shortcut: "Ctrl+J",
                command: Command::ToggleBottomPanel,
            },
        ],
        WindowsMenu::Window => vec![
            Item {
                label_key: "menu-minimize",
                shortcut: "",
                command: Command::MinimizeWindow,
            },
            Item {
                label_key: "menu-maximize-restore",
                shortcut: "",
                command: Command::ZoomWindow,
            },
            Item {
                label_key: "menu-fullscreen",
                shortcut: "Ctrl+Alt+F",
                command: Command::ToggleFullscreen,
            },
            Separator,
            Item {
                label_key: "menu-close-window",
                shortcut: "Ctrl+W",
                command: Command::CloseWindow,
            },
        ],
        WindowsMenu::Help => vec![Item {
            label_key: "about-title",
            shortcut: "",
            command: Command::ShowAbout,
        }],
    }
}

pub(super) fn application_menus(localizer: &Localizer) -> Vec<Menu> {
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

pub(super) fn dispatch_main_window_action(
    cx: &mut App,
    action: impl FnOnce(&mut RemCmdApp, &mut Window, &mut Context<RemCmdApp>) + 'static,
) {
    let window = cx.global::<RemCmdMainWindow>().0;
    cx.defer(move |cx| {
        let _ = window.update(cx, action);
    });
    cx.stop_propagation();
}

pub(super) fn configure_application_menu(cx: &mut App, localizer: &Localizer) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shell::estimated_titlebar_label_width;
    use remcmd_core::LanguageMode;

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
                    label_key,
                    shortcut,
                    ..
                } = entry
                else {
                    continue;
                };
                let required = 28.0
                    + estimated_windows_menu_text_width(&localizer.text(label_key))
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
                label_key: "about-title",
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
                    label_key,
                    shortcut,
                    ..
                } = entry
                else {
                    continue;
                };
                let required =
                    28.0 + estimated_windows_menu_text_width(
                        &Localizer::new(LanguageMode::EnUs).text(label_key),
                    ) + if shortcut.is_empty() {
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
}
