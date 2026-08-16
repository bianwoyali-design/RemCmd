use super::{
    ActivePanel, ActiveTerminal, Context, FocusHandle, MOTION_STANDARD_DURATION, PaneId,
    PaneLayout, RemCmdApp, RightSidebarView, SftpBrowserPlacement, SplitAxis,
    TERMINAL_REDRAW_INTERVAL, TabLayout, TerminalModes, TerminalSession, Timer, Window,
    encode_focus, point, px,
};

impl RemCmdApp {
    pub(super) fn session(&self, session_id: SessionId) -> Option<&TerminalSession> {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
    }

    pub(super) fn session_mut(&mut self, session_id: SessionId) -> Option<&mut TerminalSession> {
        self.sessions
            .iter_mut()
            .find(|session| session.id == session_id)
    }

    pub(super) fn active_session(&self) -> Option<&TerminalSession> {
        self.active_session_id
            .and_then(|session_id| self.session(session_id))
    }

    pub(super) fn active_session_mut(&mut self) -> Option<&mut TerminalSession> {
        let session_id = self.active_session_id?;
        self.session_mut(session_id)
    }

    pub(super) fn terminal_input_session_id(&self) -> Option<SessionId> {
        self.focused_terminal_session_id
            .filter(|session_id| self.session(*session_id).is_some())
            .or(self.active_session_id)
    }

    pub(super) fn terminal_input_session(&self) -> Option<&TerminalSession> {
        self.terminal_input_session_id()
            .and_then(|session_id| self.session(session_id))
    }

    pub(super) fn terminal_input_session_mut(&mut self) -> Option<&mut TerminalSession> {
        let session_id = self.terminal_input_session_id()?;
        self.session_mut(session_id)
    }

    pub(super) fn session_for_profile_mut(
        &mut self,
        profile_id: &str,
    ) -> Option<&mut TerminalSession> {
        self.sessions
            .iter_mut()
            .rev()
            .find(|session| session.profile_id == profile_id)
    }

    pub(super) fn session_for_profile(&self, profile_id: &str) -> Option<&TerminalSession> {
        self.sessions
            .iter()
            .rev()
            .find(|session| session.profile_id == profile_id)
    }

    pub(super) fn selected_session(&self) -> Option<&TerminalSession> {
        let profile_id = self.selected_profile_id.as_deref()?;
        self.active_session()
            .filter(|session| session.profile_id == profile_id)
    }

    pub(super) fn create_session_for_profile(&mut self, profile_id: &str) -> SessionId {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        self.sessions
            .push(TerminalSession::new(session_id, profile_id.to_owned()));
        session_id
    }

    pub(super) fn create_local_session(&mut self) -> SessionId {
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;
        let session = TerminalSession::new_local(session_id, self.tr("sftp-ssh-only"));
        self.sessions.push(session);
        session_id
    }

    pub(super) fn tab(&self, tab_id: TabId) -> Option<&TerminalTab> {
        self.tabs.iter().find(|tab| tab.id == tab_id)
    }

    pub(super) fn tab_mut(&mut self, tab_id: TabId) -> Option<&mut TerminalTab> {
        self.tabs.iter_mut().find(|tab| tab.id == tab_id)
    }

    pub(super) fn active_tab(&self) -> Option<&TerminalTab> {
        self.active_tab_id.and_then(|tab_id| self.tab(tab_id))
    }

    pub(super) fn active_tab_view(&self) -> TerminalTabView {
        self.active_tab().map(|tab| tab.view).unwrap_or_default()
    }

    pub(super) fn create_tab_for_session(
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

    pub(super) fn animate_titlebar_tabs_to_end(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn pane(&self, pane_id: PaneId) -> Option<&TerminalPane> {
        self.panes.iter().find(|pane| pane.id == pane_id)
    }

    pub(super) fn pane_mut(&mut self, pane_id: PaneId) -> Option<&mut TerminalPane> {
        self.panes.iter_mut().find(|pane| pane.id == pane_id)
    }

    pub(super) fn pane_for_session(&self, session_id: SessionId) -> Option<&TerminalPane> {
        self.panes.iter().find(|pane| pane.session_id == session_id)
    }

    pub(super) fn active_pane(&self) -> Option<&TerminalPane> {
        self.active_pane_id.and_then(|pane_id| self.pane(pane_id))
    }

    pub(super) fn create_terminal_pane(
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

    pub(super) fn set_active_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn handle_pane_focus(
        &mut self,
        pane_id: PaneId,
        focused: bool,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn activate_session_in_window(
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

    pub(super) fn activate_session(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane_id) = self.pane_for_session(session_id).map(|pane| pane.id) else {
            return false;
        };

        self.dismiss_credential_prompt(cx);
        self.set_active_pane(pane_id, cx)
    }

    pub(super) fn activate_tab_in_window(
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

    pub(super) fn remove_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn remove_tab_record(&mut self, tab_id: TabId, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn remove_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn split_active_pane(
        &mut self,
        axis: SplitAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn close_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn has_terminal_workspace(&self, profile_id: &str) -> bool {
        self.active_tab()
            .is_some_and(|tab| tab.profile_id == profile_id)
            && self.active_pane_id.is_some()
    }

    pub(super) fn terminal_has_ended(&self, profile_id: &str) -> bool {
        self.active_session()
            .filter(|session| session.profile_id == profile_id)
            .is_some_and(TerminalSession::terminal_has_ended)
    }

    pub(super) fn close_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
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

    pub(super) fn close_tab(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
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

    pub(super) fn block_close_for_unsaved_file(
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

    pub(super) fn reconnect_session(
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
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct SessionId(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct TabId(pub(super) u64);

pub(super) struct TerminalTab {
    pub(super) id: TabId,
    pub(super) profile_id: String,
    pub(super) layout: PaneLayout,
    pub(super) active_pane_id: PaneId,
    pub(super) view: TerminalTabView,
}

pub(super) struct TerminalPane {
    pub(super) id: PaneId,
    pub(super) tab_id: TabId,
    pub(super) session_id: SessionId,
    pub(super) focus_handle: FocusHandle,
    pub(super) focused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum TerminalTabView {
    #[default]
    Terminal,
    Files,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shell::workspace_tab_title;
    use crate::i18n::Localizer;
    use remcmd_core::LanguageMode;
    use remcmd_ssh::PtySize;

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
}
