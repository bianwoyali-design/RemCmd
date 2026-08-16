use super::{
    ActivePanel, AnyElement, Bounds, ClipboardItem, ConnectionCredential, ConnectionHandle,
    ConnectionStage, Context, CursorStyle, ElementInputHandler, EntityInputHandler, FocusHandle,
    HostKeyInfo, IconName, IconTone, IntoElement, KeyDownEvent, Keystroke, LocalPtySize,
    LocalTerminal, LocalTerminalEvent, LocalTerminalHandle, Localizer, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaneId, PaneLayout, Pixels, PtySize, Range, Rc, RefCell,
    RemCmdApp, RightSidebarView, SIDEBAR_SFTP_REQUEST_ID_START, ScrollWheelEvent,
    ServerPerformanceState, SessionId, SessionState, SftpAvailability, SftpBrowserPlacement,
    SftpBrowserState, SftpTransferQueue, SharedString, SplitAxis, SshError, SshErrorKind, Task,
    TerminalCanvasCache, TerminalCanvasFrame, TerminalCanvasInput, TerminalCellMetrics,
    TerminalClipboard, TerminalDamage, TerminalEngine, TerminalEvent, TerminalModes,
    TerminalPalette, TerminalPoint, TerminalScroll, TerminalSelection, TerminalSnapshot,
    TerminalTabView, TextAreaSize, Timer, UTF16Selection, Window, canvas, connection_stage_label,
    div, encode_alternate_scroll, encode_key, encode_paste, localized_connection_error_parts,
    palette_color, point, px, rgb, should_translate_alternate_scroll, size,
};
use gpui::prelude::*;
use std::time::Duration;

impl RemCmdApp {
    pub(super) fn open_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session_id = self.create_local_session();
        let tab_id = self.create_tab_for_session(session_id, LOCAL_PROFILE_ID.into(), window, cx);
        self.activate_tab_in_window(tab_id, window, cx);
        self.start_local_terminal(session_id, cx);
        cx.notify();
    }

    pub(super) fn start_local_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let size = PtySize::new(TERMINAL_COLUMNS, TERMINAL_ROWS);
        let terminal = LocalTerminal::spawn(local_pty_size(size));
        let (handle, mut events) = terminal.split();
        let sftp_unavailable = self.tr("sftp-ssh-only");

        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        session.close_when_disconnected = false;
        session.connection_state = SessionState::Connecting;
        session.connection_handle = None;
        session.local_terminal_handle = Some(handle);
        session.connection_error = None;
        session.connection_message = Some(SessionMessage::localized("terminal-starting-local"));
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

    pub(super) fn terminal_modes(&self, session_id: SessionId) -> TerminalModes {
        self.session(session_id)
            .and_then(|session| session.terminal.as_ref())
            .map(ActiveTerminal::modes)
            .unwrap_or(TerminalModes::NONE)
    }

    pub(super) fn terminal_palette(&self) -> TerminalPalette {
        if self.theme.is_light() {
            TerminalPalette::light()
        } else {
            TerminalPalette::dark()
        }
    }

    pub(super) fn terminal_point_for_position(
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

    pub(super) fn on_terminal_mouse_down(
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

    pub(super) fn on_quick_terminal_mouse_down(
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

    pub(super) fn open_terminal_context_menu(
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

    pub(super) fn begin_terminal_selection(
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

    pub(super) fn on_terminal_mouse_move(
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

    pub(super) fn on_terminal_mouse_up(
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

    pub(super) fn copy_terminal_selection(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
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

    pub(super) fn clear_terminal_selection(&mut self, session_id: SessionId) -> bool {
        let Some(session) = self.session_mut(session_id) else {
            return false;
        };
        let had_selection = session.terminal_selection.take().is_some();
        let was_selecting = std::mem::take(&mut session.terminal_selecting);
        had_selection || was_selecting
    }

    pub(super) fn select_all_terminal(
        &mut self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
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

    pub(super) fn reset_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
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

    pub(super) fn on_terminal_key_down(
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

    pub(super) fn paste_into_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };

        let bytes = encode_paste(&text, self.terminal_modes(session_id));
        self.send_terminal_user_input(session_id, bytes, cx);
    }

    pub(super) fn send_terminal_input(
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
            session.connection_error = Some(error.into());
            cx.notify();
        }
    }

    pub(super) fn send_terminal_user_input(
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

    pub(super) fn apply_terminal_layout(
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

    pub(super) fn schedule_terminal_resize(
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
                    session.connection_error = Some(error.into());
                    cx.notify();
                }
            });
        });
        if let Some(session) = self.session_mut(session_id) {
            session.terminal_resize_task = Some(task);
        }
    }

    pub(super) fn on_terminal_scroll(
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

    pub(super) fn on_quick_terminal_scroll(
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

    pub(super) fn scroll_terminal_session(
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

    pub(super) fn process_terminal_output(
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

    pub(super) fn terminal_session_is_rendered(&self, session_id: SessionId) -> bool {
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

    pub(super) fn schedule_terminal_redraw(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn handle_terminal_event(
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
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message =
                        Some(SessionMessage::localized("terminal-remote-bell"));
                }
                true
            }
            TerminalEvent::ExitRequested => {
                let message = SessionMessage::localized("terminal-remote-exit-requested");
                if let Some(session) = self.session_mut(session_id) {
                    session.connection_message = Some(message.clone());
                    session.terminal_end_reason = Some(message);
                }
                true
            }
            TerminalEvent::ChildExited(status) => {
                let message = status.map_or_else(
                    || SessionMessage::localized("terminal-remote-exited"),
                    |status| {
                        SessionMessage::localized_with(
                            "terminal-remote-exited-status",
                            [("status", status.to_string())],
                        )
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

    pub(super) fn send_terminal_response(&mut self, session_id: SessionId, data: Vec<u8>) {
        let Some(session) = self.session_mut(session_id) else {
            return;
        };
        if let Err(error) = session.write_terminal_input(data) {
            session.connection_error = Some(error.into());
        }
    }

    pub(super) fn write_terminal_clipboard(
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

    pub(super) fn read_terminal_clipboard(
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

    pub(super) fn handle_local_terminal_event(
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
                        SessionMessage::localized_with(
                            "terminal-local-shell-status",
                            [("status", exit_code.to_string())],
                        )
                    },
                    |signal| {
                        SessionMessage::localized_with(
                            "terminal-local-shell-signal",
                            [("signal", signal)],
                        )
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
                    session.connection_error = Some(error.to_string().into());
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

    pub(super) fn render_pane_layout(
        &self,
        layout: &PaneLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    pub(super) fn render_terminal_pane(
        &self,
        pane_id: PaneId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    pub(super) fn render_terminal_session_view(
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

    pub(super) fn render_terminal_lifecycle(
        &self,
        session_id: SessionId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.session(session_id);
        let (message, color) =
            if let Some(error) = session.and_then(|session| session.connection_error.as_ref()) {
                (error.render(&self.localizer), self.theme.error_text)
            } else if let Some(message) =
                session.and_then(|session| session.terminal_end_reason.as_ref())
            {
                (message.render(&self.localizer), self.theme.text_muted)
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

    pub(super) fn render_terminal_context_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
}

pub(super) const TERMINAL_COLUMNS: u32 = 80;

pub(super) const TERMINAL_ROWS: u32 = 24;

pub(super) const LOCAL_PROFILE_ID: &str = "__remcmd_local_terminal__";

pub(super) const TERMINAL_CELL_WIDTH: u16 = 8;

pub(super) const TERMINAL_CELL_HEIGHT: u16 = 19;

pub(super) const TERMINAL_RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);

pub(super) const TERMINAL_REDRAW_INTERVAL: Duration = Duration::from_millis(16);

pub(super) const TERMINAL_EVENT_BATCH_LIMIT: usize = 64;

pub(super) const TERMINAL_FONT_LINE_HEIGHT_FACTOR: f32 = 19.0 / 14.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionMessageArg {
    Text(String),
    Localized(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SessionMessage {
    Plain(String),
    Localized {
        key: &'static str,
        args: Vec<(&'static str, SessionMessageArg)>,
    },
    LocalizedWithDetail {
        key: &'static str,
        detail: String,
    },
    ConnectionError {
        kind: SshErrorKind,
        details: String,
        stage: Option<ConnectionStage>,
    },
    HostKeyVerification {
        stage: ConnectionStage,
        address: String,
    },
}

impl SessionMessage {
    pub(super) fn localized(key: &'static str) -> Self {
        Self::Localized {
            key,
            args: Vec::new(),
        }
    }

    pub(super) fn localized_with(
        key: &'static str,
        args: impl IntoIterator<Item = (&'static str, String)>,
    ) -> Self {
        Self::Localized {
            key,
            args: args
                .into_iter()
                .map(|(name, value)| (name, SessionMessageArg::Text(value)))
                .collect(),
        }
    }

    pub(super) fn localized_with_args(
        key: &'static str,
        args: impl IntoIterator<Item = (&'static str, SessionMessageArg)>,
    ) -> Self {
        Self::Localized {
            key,
            args: args.into_iter().collect(),
        }
    }

    pub(super) fn connection_error(error: &SshError) -> Self {
        Self::ConnectionError {
            kind: error.kind(),
            details: error.message().to_owned(),
            stage: error.stage().cloned(),
        }
    }

    pub(super) fn host_key_verification(stage: ConnectionStage, address: String) -> Self {
        Self::HostKeyVerification { stage, address }
    }

    pub(super) fn localized_with_detail(key: &'static str, detail: impl Into<String>) -> Self {
        Self::LocalizedWithDetail {
            key,
            detail: detail.into(),
        }
    }

    pub(super) fn render(&self, localizer: &Localizer) -> String {
        match self {
            Self::Plain(message) => message.clone(),
            Self::Localized { key, args } => {
                let mut fluent_args = fluent_bundle::FluentArgs::new();
                for (name, value) in args {
                    fluent_args.set(
                        *name,
                        match value {
                            SessionMessageArg::Text(value) => value.clone(),
                            SessionMessageArg::Localized(key) => localizer.text(key),
                        },
                    );
                }
                localizer.text_with(key, Some(&fluent_args))
            }
            Self::LocalizedWithDetail { key, detail } => {
                format!("{}: {detail}", localizer.text(key))
            }
            Self::ConnectionError {
                kind,
                details,
                stage,
            } => localized_connection_error_parts(*kind, details, stage.as_ref(), localizer),
            Self::HostKeyVerification { stage, address } => {
                format!("{}: {address}", connection_stage_label(stage, localizer))
            }
        }
    }
}

impl From<String> for SessionMessage {
    fn from(message: String) -> Self {
        Self::Plain(message)
    }
}

pub(super) struct TerminalSession {
    pub(super) id: SessionId,
    pub(super) profile_id: String,
    pub(super) kind: TerminalSessionKind,
    pub(super) close_when_disconnected: bool,
    pub(super) connection_state: SessionState,
    pub(super) connection_handle: Option<ConnectionHandle>,
    pub(super) local_terminal_handle: Option<LocalTerminalHandle>,
    pub(super) connection_error: Option<SessionMessage>,
    pub(super) connection_message: Option<SessionMessage>,
    pub(super) terminal_end_reason: Option<SessionMessage>,
    pub(super) host_key_prompt: Option<HostKeyInfo>,
    pub(super) terminal: Option<ActiveTerminal>,
    pub(super) terminal_marked_text: String,
    pub(super) terminal_selection: Option<TerminalSelection>,
    pub(super) terminal_selecting: bool,
    pub(super) terminal_scroll_accumulator: f32,
    pub(super) terminal_resize_task: Option<Task<()>>,
    pub(super) connection_credentials: Vec<ConnectionCredential>,
    pub(super) sftp_availability: SftpAvailability,
    pub(super) sftp: SftpBrowserState,
    pub(super) sidebar_sftp: SftpBrowserState,
    pub(super) transfers: SftpTransferQueue,
    pub(super) performance: ServerPerformanceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerminalSessionKind {
    Ssh,
    Local,
}

impl TerminalSession {
    pub(super) fn new(id: SessionId, profile_id: String) -> Self {
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

    pub(super) fn new_local(id: SessionId, sftp_unavailable: String) -> Self {
        let mut session = Self::new(id, LOCAL_PROFILE_ID.into());
        session.kind = TerminalSessionKind::Local;
        session.sftp_availability = SftpAvailability::Unavailable(sftp_unavailable);
        session
    }

    pub(super) fn is_local(&self) -> bool {
        self.kind == TerminalSessionKind::Local
    }

    pub(super) fn write_terminal_input(&self, data: Vec<u8>) -> Result<(), String> {
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

    pub(super) fn resize_terminal(&self, size: PtySize) -> Result<(), String> {
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

    pub(super) fn disconnect_terminal(&self) -> Result<(), String> {
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

    pub(super) fn sftp_browser(&self, placement: SftpBrowserPlacement) -> &SftpBrowserState {
        match placement {
            SftpBrowserPlacement::Center => &self.sftp,
            SftpBrowserPlacement::Sidebar => &self.sidebar_sftp,
        }
    }

    pub(super) fn sftp_browser_mut(
        &mut self,
        placement: SftpBrowserPlacement,
    ) -> &mut SftpBrowserState {
        match placement {
            SftpBrowserPlacement::Center => &mut self.sftp,
            SftpBrowserPlacement::Sidebar => &mut self.sidebar_sftp,
        }
    }

    pub(super) fn is_terminal_visible(&self) -> bool {
        let active_connection = !self.connection_state.can_connect();

        self.terminal.as_ref().is_some_and(|terminal| {
            terminal.profile_id == self.profile_id && (active_connection || terminal.was_connected)
        })
    }

    pub(super) fn terminal_has_ended(&self) -> bool {
        self.connection_state.can_connect()
            && self.terminal.as_ref().is_some_and(|terminal| {
                terminal.profile_id == self.profile_id && terminal.was_connected
            })
    }
}

pub(super) struct ActiveTerminal {
    pub(super) profile_id: String,
    pub(super) engine: TerminalEngine,
    pub(super) title: Option<String>,
    pub(super) remote_cwd: Option<String>,
    pub(super) pty_size: PtySize,
    pub(super) pending_pty_size: Option<PtySize>,
    pub(super) cell_width: f32,
    pub(super) cell_height: f32,
    pub(super) viewport_bounds: Option<Bounds<Pixels>>,
    pub(super) was_connected: bool,
    pub(super) render_damage: RefCell<TerminalDamage>,
    pub(super) render_snapshot: RefCell<Option<Rc<TerminalSnapshot>>>,
    pub(super) canvas_cache: Rc<RefCell<TerminalCanvasCache>>,
}

impl ActiveTerminal {
    pub(super) fn new(profile_id: String, size: PtySize) -> Self {
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

    pub(super) fn process(&mut self, bytes: &[u8]) -> Vec<TerminalEvent> {
        self.engine.process(bytes);
        self.capture_damage();
        self.engine.drain_events()
    }

    pub(super) fn reset(&mut self) {
        self.engine.reset();
        self.title = None;
        *self.render_damage.borrow_mut() = TerminalDamage::Full;
        self.render_snapshot.borrow_mut().take();
    }

    pub(super) fn snapshot(&self) -> TerminalSnapshot {
        self.engine.snapshot()
    }

    pub(super) fn snapshot_for_render(&self) -> (Rc<TerminalSnapshot>, TerminalDamage) {
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

    pub(super) fn scroll(&mut self, scroll: TerminalScroll) {
        self.engine.scroll(scroll);
        self.capture_damage();
    }

    pub(super) fn capture_damage(&mut self) {
        let damage = self.engine.take_damage();
        merge_terminal_damage(&mut self.render_damage.borrow_mut(), damage);
    }

    pub(super) fn text_area_size(&self) -> TextAreaSize {
        let size = self.engine.size();

        TextAreaSize {
            rows: u16::try_from(size.rows()).unwrap_or(u16::MAX),
            columns: u16::try_from(size.columns()).unwrap_or(u16::MAX),
            cell_width: pixel_cell_dimension(self.cell_width),
            cell_height: pixel_cell_dimension(self.cell_height),
        }
    }

    pub(super) fn modes(&self) -> TerminalModes {
        self.engine.modes()
    }

    pub(super) fn stage_resize(&mut self, size: PtySize) -> bool {
        let current_target = self.pending_pty_size.unwrap_or(self.pty_size);
        if current_target == size {
            return false;
        }

        self.pending_pty_size = Some(size);
        true
    }

    pub(super) fn acknowledge_resize(&mut self, size: PtySize) -> bool {
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

pub(super) fn merge_terminal_damage(current: &mut TerminalDamage, incoming: TerminalDamage) {
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
pub(super) struct TerminalLayout {
    pub(super) pty_size: PtySize,
    pub(super) cell_width: f32,
    pub(super) cell_height: f32,
}

pub(super) struct TerminalContextMenu {
    pub(super) session_id: SessionId,
    pub(super) position: gpui::Point<Pixels>,
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

pub(super) fn terminal_layout_for_pixels(
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

pub(super) fn local_pty_size(size: PtySize) -> LocalPtySize {
    LocalPtySize::new(size.columns, size.rows).with_pixels(size.pixel_width, size.pixel_height)
}

pub(super) fn ssh_pty_size(size: LocalPtySize) -> PtySize {
    PtySize::new(size.columns, size.rows).with_pixels(size.pixel_width, size.pixel_height)
}

pub(super) fn terminal_point_for_pixels(
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

pub(super) fn full_terminal_selection(rows: usize, columns: usize) -> Option<TerminalSelection> {
    (rows > 0 && columns > 0).then(|| {
        TerminalSelection::new(
            TerminalPoint::new(0, 0),
            TerminalPoint::new(rows - 1, columns),
        )
    })
}

pub(super) fn valid_dimension(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

pub(super) fn cell_count(viewport: f32, cell: f32) -> u32 {
    (valid_dimension(viewport, cell) / cell)
        .floor()
        .clamp(1.0, u32::MAX as f32) as u32
}

pub(super) fn pixel_dimension(value: f32) -> u32 {
    valid_dimension(value, 1.0)
        .floor()
        .clamp(1.0, u32::MAX as f32) as u32
}

pub(super) fn pixel_cell_dimension(value: f32) -> u16 {
    value.round().clamp(1.0, f32::from(u16::MAX)) as u16
}

pub(super) fn utf16_offset_to_utf8(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;

    for (utf8_offset, character) in text.char_indices() {
        if utf16_offset >= offset || utf16_offset + character.len_utf16() > offset {
            return utf8_offset;
        }
        utf16_offset += character.len_utf16();
    }

    text.len()
}

pub(super) fn is_terminal_paste_shortcut(keystroke: &Keystroke) -> bool {
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

pub(super) fn is_terminal_copy_shortcut(keystroke: &Keystroke) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use remcmd_core::LanguageMode;
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
    fn session_messages_localize_at_render_time() {
        let message = SessionMessage::localized("terminal-session-disconnected");
        let english = Localizer::new(LanguageMode::EnUs);
        let chinese = Localizer::new(LanguageMode::ZhCn);

        assert_eq!(
            message.render(&english),
            english.text("terminal-session-disconnected")
        );
        assert_eq!(
            message.render(&chinese),
            chinese.text("terminal-session-disconnected")
        );
        assert_ne!(message.render(&english), message.render(&chinese));
    }

    #[test]
    fn connection_errors_relocalize_without_changing_technical_details() {
        let error = SshError::new(SshErrorKind::Network, "connection-detail-canary");
        let message = SessionMessage::connection_error(&error);
        let english = message.render(&Localizer::new(LanguageMode::EnUs));
        let chinese = message.render(&Localizer::new(LanguageMode::ZhCn));

        assert_ne!(english, chinese);
        assert!(english.contains("connection-detail-canary"));
        assert!(chinese.contains("connection-detail-canary"));
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
