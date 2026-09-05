use super::{Context, RemCmdApp, Window};
use gpui::{App, FocusHandle, Focusable, KeyBinding};

gpui::actions!(keyboard_navigation, [FocusNext, FocusPrevious]);

pub(super) fn bind_navigation_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, Some("RemCmd && !Terminal && !FileEditor")),
        KeyBinding::new(
            "shift-tab",
            FocusPrevious,
            Some("RemCmd && !Terminal && !FileEditor"),
        ),
    ]);
}

fn cycle_focus(scope: Option<&FocusHandle>, backwards: bool, window: &mut Window, cx: &App) {
    let original = window.focused(cx);
    let mut first = None;
    loop {
        if backwards {
            window.focus_prev();
        } else {
            window.focus_next();
        }
        let focused = window.focused(cx);
        if scope.is_none_or(|scope| scope.contains_focused(window, cx)) || focused.is_none() {
            break;
        }
        if focused == original || focused == first {
            break;
        }
        if first.is_none() {
            first = focused;
        }
    }
}

impl RemCmdApp {
    pub(super) fn focus_new_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = if self.proxy_command_approval_prompt.is_some()
            || self
                .active_session()
                .is_some_and(|session| session.host_key_prompt.is_some())
        {
            Some(self.modal_focus_handle.clone())
        } else if let Some(prompt) = self.credential_prompt.as_ref() {
            Some(prompt.input.focus_handle(cx))
        } else if let Some(prompt) = self.sftp_create_prompt.as_ref() {
            Some(prompt.input.focus_handle(cx))
        } else {
            self.editor
                .as_ref()
                .map(|editor| editor.name.focus_handle(cx))
        };
        if focus != self.modal_initial_focus {
            if let Some(focus) = focus.as_ref() {
                focus.focus(window);
            }
            self.modal_initial_focus = focus;
        }
    }

    fn move_keyboard_focus(
        &mut self,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_selector = None;
        self.profile_auth_selector_open = false;
        let modal = self.editor.is_some()
            || self.credential_prompt.is_some()
            || self.sftp_create_prompt.is_some()
            || self.proxy_command_approval_prompt.is_some()
            || self
                .active_session()
                .is_some_and(|session| session.host_key_prompt.is_some());
        cycle_focus(
            modal.then_some(&self.modal_focus_handle),
            backwards,
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn focus_next(
        &mut self,
        _: &FocusNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_keyboard_focus(false, window, cx);
    }

    pub(super) fn focus_previous(
        &mut self,
        _: &FocusPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_keyboard_focus(true, window, cx);
    }
}

pub(super) fn menu_index(current: usize, count: usize, key: &str) -> usize {
    let last = count.saturating_sub(1);
    match key {
        "up" => current.saturating_sub(1).min(last),
        "down" => current.saturating_add(1).min(last),
        "home" => 0,
        "end" => last,
        _ => current.min(last),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_navigation_handles_boundaries_empty_and_filtered_lists() {
        assert_eq!(menu_index(0, 4, "up"), 0);
        assert_eq!(menu_index(3, 4, "down"), 3);
        assert_eq!(menu_index(2, 4, "up"), 1);
        assert_eq!(menu_index(2, 4, "home"), 0);
        assert_eq!(menu_index(0, 4, "end"), 3);
        assert_eq!(menu_index(20, 0, "down"), 0);
        assert_eq!(menu_index(20, 2, "up"), 1);
    }
}
