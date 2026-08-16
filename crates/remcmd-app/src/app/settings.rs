use super::{
    ActivePanel, AnyElement, AppSettings, CancelSettingsSelector, Context, CredentialPromptKind,
    FontWeight, IconName, IconTone, LanguageMode, Localizer, Range, RemCmdApp, ScrollHandle,
    SftpAvailability, SftpCreateKind, SharedString, TabLayout, TerminalSettings, Theme, ThemeMode,
    UniformListScrollHandle, Window, application_menus, deferred, div, icon, icon_with_color,
    point, px, save_settings, set_global_theme, uniform_list,
};
use gpui::prelude::*;

impl RemCmdApp {
    pub(super) fn set_language_mode(
        &mut self,
        language_mode: LanguageMode,
        cx: &mut Context<Self>,
    ) {
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
        let module_placeholder = self.tr("diagnostics-filter-module");
        self.diagnostic_module_filter.update(cx, |field, cx| {
            field.set_placeholder(module_placeholder, cx);
        });
        let text_placeholder = self.tr("diagnostics-filter-text");
        self.diagnostic_text_filter.update(cx, |field, cx| {
            field.set_placeholder(text_placeholder, cx);
        });
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
                about.localizer = Localizer::new(language_mode);
                window.set_window_title(&title);
                cx.notify();
            });
        }

        cx.set_menus(application_menus(&self.localizer));
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn refresh_system_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.theme_mode != ThemeMode::System {
            return;
        }

        self.theme = Theme::resolve(self.theme_mode, window);
        set_global_theme(self.theme, cx);
        cx.notify();
    }

    pub(super) fn set_theme_mode(
        &mut self,
        theme_mode: ThemeMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.theme_mode = theme_mode;
        self.theme = Theme::resolve(theme_mode, window);
        set_global_theme(self.theme, cx);

        self.persist_settings();
        cx.notify();
    }

    pub(super) fn set_tab_layout(&mut self, tab_layout: TabLayout, cx: &mut Context<Self>) {
        self.tab_layout = tab_layout;
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn set_terminal_font_family(
        &mut self,
        terminal_font_family: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_selector = None;
        self.terminal_font_family = terminal_font_family;
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn set_terminal_font_size(
        &mut self,
        terminal_font_size: u16,
        cx: &mut Context<Self>,
    ) {
        self.terminal_font_size = TerminalSettings {
            font_family: None,
            font_size: terminal_font_size,
        }
        .normalized()
        .font_size;
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn set_transfer_rate_limit(
        &mut self,
        rate_limit_mib_per_second: u32,
        cx: &mut Context<Self>,
    ) {
        self.transfer_settings.rate_limit_mib_per_second = rate_limit_mib_per_second;
        self.transfer_settings = self.transfer_settings.normalized();
        self.transfer_rate_limiter
            .set_bytes_per_second(self.transfer_settings.bytes_per_second());
        self.persist_settings();
        cx.notify();
    }

    pub(super) fn set_max_parallel_transfers(
        &mut self,
        max_parallel_transfers: u8,
        cx: &mut Context<Self>,
    ) {
        self.transfer_settings.max_parallel_transfers = max_parallel_transfers;
        self.transfer_settings = self.transfer_settings.normalized();
        self.persist_settings();
        self.start_queued_sftp_transfers(cx);
        cx.notify();
    }

    pub(super) fn settings_value(&self, selector: SettingsSelector) -> SettingsValue {
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

    pub(super) fn settings_value_label(&self, selector: SettingsSelector) -> SharedString {
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

    pub(super) fn settings_option_label(&self, option: &SettingsOption) -> String {
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

    pub(super) fn toggle_settings_selector(
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

    pub(super) fn dismiss_settings_selector(&mut self, cx: &mut Context<Self>) {
        if self.open_settings_selector.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn apply_settings_value(
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

    pub(super) fn persist_settings(&mut self) {
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

    pub(super) fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_credential_prompt(cx);
        self.bottom_panel_open = false;
        self.bottom_panel_resize = None;
        self.terminal_context_menu = None;
        self.active_panel = ActivePanel::Settings;
        self.open_settings_selector = None;
        self.settings_focus_handle.focus(window);
        cx.notify();
    }

    pub(super) fn on_cancel_settings_selector(
        &mut self,
        _: &CancelSettingsSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_settings_selector(cx);
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> gpui::Div {
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
                    .child(self.tr("settings-diagnostics")),
            )
            .child(
                div()
                    .id("open-diagnostics")
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
                    .child(self.tr("diagnostics-title"))
                    .child(icon(IconName::Expand, self.theme, IconTone::Default, 15.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_diagnostics(window, cx);
                    })),
            )
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

    pub(super) fn render_settings_row(
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

    pub(super) fn render_settings_selector(
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

    pub(super) fn render_settings_selector_row(
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
                self.settings_option_label(&option).into(),
                hover_group.clone(),
                selector.menu_width() - 40.0,
            )
        } else {
            self.render_select_menu_label(
                self.settings_option_label(&option).into(),
                hover_group.clone(),
            )
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

    pub(super) fn render_virtual_settings_selector_rows(
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

    pub(super) fn render_terminal_font_selector(&self, cx: &mut Context<Self>) -> gpui::Div {
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

    pub(super) fn render_terminal_font_selector_row(
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

    pub(super) fn render_virtual_terminal_font_selector_rows(
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

    pub(super) fn render_select_menu_label(
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

    pub(super) fn render_virtual_select_menu_label(
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

    pub(super) fn render_select_menu_check(
        &self,
        selected: bool,
        hover_group: SharedString,
    ) -> gpui::Div {
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
}

pub(super) const SELECT_MENU_ROW_HEIGHT: f32 = 28.0;

pub(super) const SELECT_MENU_MAX_VISIBLE_ROWS: usize = 9;

#[cfg(target_os = "macos")]
pub(super) const UI_MONOSPACE_FONT_FAMILY: &str = "Menlo";

#[cfg(target_os = "windows")]
pub(super) const UI_MONOSPACE_FONT_FAMILY: &str = "Consolas";

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) const UI_MONOSPACE_FONT_FAMILY: &str = "DejaVu Sans Mono";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsSelector {
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

    pub(super) const fn options(self) -> &'static [SettingsOption] {
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
pub(super) enum SettingsValue {
    Language(LanguageMode),
    Theme(ThemeMode),
    TabLayout(TabLayout),
    TerminalFontSize(u16),
    TransferRate(u32),
    ParallelTransfers(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SettingsOption {
    pub(super) label: &'static str,
    pub(super) value: SettingsValue,
}

pub(super) const LANGUAGE_SETTING_OPTIONS: [SettingsOption; 3] = [
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

pub(super) const THEME_SETTING_OPTIONS: [SettingsOption; 3] = [
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

pub(super) const TAB_LAYOUT_SETTING_OPTIONS: [SettingsOption; 2] = [
    SettingsOption {
        label: "Horizontal",
        value: SettingsValue::TabLayout(TabLayout::Horizontal),
    },
    SettingsOption {
        label: "Vertical",
        value: SettingsValue::TabLayout(TabLayout::Vertical),
    },
];

pub(super) const TERMINAL_FONT_SIZE_SETTING_OPTIONS: [SettingsOption; 17] = [
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

pub(super) const TRANSFER_RATE_SETTING_OPTIONS: [SettingsOption; 4] = [
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

pub(super) const PARALLEL_TRANSFER_SETTING_OPTIONS: [SettingsOption; 4] = [
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

pub(super) fn select_menu_height(option_count: usize) -> f32 {
    option_count.clamp(1, SELECT_MENU_MAX_VISIBLE_ROWS) as f32 * SELECT_MENU_ROW_HEIGHT + 8.0
}

pub(super) fn select_menu_scroll_offset(selected_index: usize, option_count: usize) -> f32 {
    let visible_rows = option_count.min(SELECT_MENU_MAX_VISIBLE_ROWS);
    let first_visible = selected_index
        .saturating_sub(visible_rows / 2)
        .min(option_count.saturating_sub(visible_rows));
    first_visible as f32 * SELECT_MENU_ROW_HEIGHT
}

pub(super) fn normalize_terminal_font_families(mut families: Vec<String>) -> Vec<SharedString> {
    families.retain(|family| {
        let family = family.trim();
        !family.is_empty() && !family.starts_with('.')
    });
    families.sort_by_cached_key(|family| family.to_lowercase());
    families.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    families.into_iter().map(SharedString::from).collect()
}

pub(super) fn resolve_terminal_font_family(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_menu_scroll_offset_centers_and_clamps_the_selected_option() {
        assert_eq!(select_menu_scroll_offset(0, 17), 0.0);
        assert_eq!(select_menu_scroll_offset(8, 17), 4.0 * 28.0);
        assert_eq!(select_menu_scroll_offset(16, 17), 8.0 * 28.0);
        assert_eq!(select_menu_scroll_offset(2, 3), 0.0);
    }

    #[test]
    fn settings_selectors_cover_every_persisted_choice() {
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
}
