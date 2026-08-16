use super::{
    AnyElement, CancelQuickCommand, Context, CursorStyle, Entity, FontWeight, IconName, IconTone,
    IntoElement, MouseButton, RemCmdApp, SessionId, SessionState, SharedString, SubmitQuickCommand,
    TerminalSession, TerminalSessionKind, TextButtonTone, TextField, Window, content_top_inset,
    deferred, div, icon, px, text_button,
};
use gpui::prelude::*;
use std::collections::HashSet;

impl RemCmdApp {
    pub(super) fn connected_ssh_profile_ids(&self) -> Vec<String> {
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

    pub(super) fn ensure_quick_command_prompt(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn sync_default_quick_command_targets(&mut self) {
        let connected_profile_ids = self.connected_ssh_profile_ids();
        if let Some(prompt) = self.quick_command_prompt.as_mut()
            && !prompt.selection_touched
        {
            prompt.selected_profile_ids = connected_profile_ids.into_iter().collect();
        }
    }

    pub(super) fn close_quick_command(&mut self, cx: &mut Context<Self>) {
        if self.bottom_panel_open {
            self.bottom_panel_open = false;
            self.bottom_panel_resize = None;
            cx.notify();
        }
    }

    pub(super) fn toggle_bottom_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn restart_quick_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn dispose_quick_terminal(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_quick_command_targets(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.quick_command_prompt.as_mut() {
            prompt.target_menu_open = !prompt.target_menu_open;
            cx.notify();
        }
    }

    pub(super) fn toggle_quick_command_target(
        &mut self,
        profile_id: String,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn submit_quick_command(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn render_bottom_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn render_quick_terminal_panel(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .map(|message| message.render(&self.localizer))
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

    pub(super) fn render_quick_command_prompt(&self, cx: &mut Context<Self>) -> AnyElement {
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
}

pub(super) const BOTTOM_PANEL_DEFAULT_HEIGHT: f32 = 240.0;

pub(super) const BOTTOM_PANEL_MIN_HEIGHT: f32 = 140.0;

pub(super) const BOTTOM_PANEL_MAX_HEIGHT: f32 = 520.0;

pub(super) const BOTTOM_PANEL_HEADER_HEIGHT: f32 = 34.0;

pub(super) struct QuickCommandPrompt {
    pub(super) input: Entity<TextField>,
    pub(super) selected_profile_ids: HashSet<String>,
    pub(super) selection_touched: bool,
    pub(super) target_menu_open: bool,
    pub(super) error: Option<String>,
}

pub(super) fn clamp_bottom_panel_height(requested: f32, viewport_height: f32) -> f32 {
    let available_height = (viewport_height - content_top_inset() - 100.0)
        .clamp(BOTTOM_PANEL_MIN_HEIGHT, BOTTOM_PANEL_MAX_HEIGHT);
    requested.clamp(BOTTOM_PANEL_MIN_HEIGHT, available_height)
}

pub(super) fn quick_command_target_sessions(
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn bottom_panel_height_preserves_main_content() {
        assert_eq!(clamp_bottom_panel_height(80.0, 720.0), 140.0);
        assert_eq!(clamp_bottom_panel_height(600.0, 720.0), 520.0);
        let panel_height = clamp_bottom_panel_height(400.0, 480.0);
        assert_eq!(480.0 - content_top_inset() - panel_height, 100.0);
    }
}
