//! Context and popup menus described once and presented natively on macOS or as GPUI
//! popovers elsewhere.

use std::rc::Rc;
#[cfg(target_os = "macos")]
use std::{cell::RefCell, collections::HashMap};

#[cfg(target_os = "macos")]
use gpui::Bounds;
use gpui::{
    AnyElement, Context, MouseButton, Pixels, Point, SharedString, Window, deferred, div,
    prelude::*, px,
};

#[cfg(target_os = "macos")]
use super::native_menu::{NativeMenuEntry, NativeMenuPlacement, pop_up_menu};

use super::{IconName, IconTone, RemCmdApp, SessionId};

pub(super) type ContextMenuAction =
    Rc<dyn Fn(&mut RemCmdApp, &mut Window, &mut Context<RemCmdApp>)>;

#[derive(Clone)]
pub(super) enum ContextMenuEntry {
    Item {
        id: &'static str,
        icon: IconName,
        label: SharedString,
        tone: IconTone,
        enabled: bool,
        action: ContextMenuAction,
    },
    Separator,
}

impl ContextMenuEntry {
    pub(super) fn item(
        id: &'static str,
        icon: IconName,
        label: impl Into<SharedString>,
        enabled: bool,
        action: impl Fn(&mut RemCmdApp, &mut Window, &mut Context<RemCmdApp>) + 'static,
    ) -> Self {
        Self::Item {
            id,
            icon,
            label: label.into(),
            tone: IconTone::Default,
            enabled,
            action: Rc::new(action),
        }
    }

    pub(super) fn danger(self) -> Self {
        match self {
            Self::Item {
                id,
                icon,
                label,
                enabled,
                action,
                ..
            } => Self::Item {
                id,
                icon,
                label,
                tone: IconTone::Danger,
                enabled,
                action,
            },
            Self::Separator => Self::Separator,
        }
    }
}

/// An option in a popup selector shown as a native menu.
#[cfg(target_os = "macos")]
pub(super) struct SelectMenuOption {
    pub(super) label: SharedString,
    pub(super) checked: bool,
    pub(super) action: ContextMenuAction,
}

#[cfg(target_os = "macos")]
impl SelectMenuOption {
    pub(super) fn new(
        label: impl Into<SharedString>,
        checked: bool,
        action: impl Fn(&mut RemCmdApp, &mut Window, &mut Context<RemCmdApp>) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            checked,
            action: Rc::new(action),
        }
    }
}

/// Control bounds that native popup menus open over, keyed by control id.
#[cfg(target_os = "macos")]
pub(super) type SelectMenuAnchors = Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>;

/// A GPUI-rendered menu that is open in the main window.
pub(super) struct ContextMenu {
    /// The terminal or file session the menu acts on, if any.
    pub(super) session_id: Option<SessionId>,
    position: Point<Pixels>,
    entries: Vec<ContextMenuEntry>,
}

const CONTEXT_MENU_WIDTH: f32 = 188.0;
const CONTEXT_MENU_ITEM_HEIGHT: f32 = 28.0;
const CONTEXT_MENU_SEPARATOR_HEIGHT: f32 = 9.0;
const CONTEXT_MENU_CHROME_HEIGHT: f32 = 10.0;
const CONTEXT_MENU_EDGE_INSET: f32 = 8.0;

impl RemCmdApp {
    /// Opens a context menu at `position` in window coordinates.
    pub(super) fn open_context_menu(
        &mut self,
        session_id: Option<SessionId>,
        entries: Vec<ContextMenuEntry>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        cx.stop_propagation();
        cx.notify();
        #[cfg(target_os = "macos")]
        {
            let native_entries = entries
                .iter()
                .map(|entry| match entry {
                    ContextMenuEntry::Item { label, enabled, .. } => NativeMenuEntry::Item {
                        label: label.clone(),
                        enabled: *enabled,
                        checked: false,
                    },
                    ContextMenuEntry::Separator => NativeMenuEntry::Separator,
                })
                .collect();
            let placement = NativeMenuPlacement {
                location: position,
                positioning_item: None,
                minimum_width: None,
                opened_on_right_mouse_down: true,
            };
            let actions = entries.clone();
            let on_select = Box::new(
                move |this: &mut Self,
                      index: usize,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
                    if let Some(ContextMenuEntry::Item {
                        enabled: true,
                        action,
                        ..
                    }) = actions.get(index)
                    {
                        this.run_context_menu_action(action, window, cx);
                    }
                },
            );
            if pop_up_menu(native_entries, placement, on_select, window, cx) {
                return;
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        self.context_menu = Some(ContextMenu {
            session_id,
            position,
            entries,
        });
    }

    /// Shows a popup selector's options as a native menu over the control `anchor_id`.
    ///
    /// Single-choice selectors place the checked option over the control, as
    /// NSPopUpButton does; multiple-choice selectors drop the menu below it.
    /// Returns false when the native menu is unavailable.
    #[cfg(target_os = "macos")]
    pub(super) fn pop_up_select_menu(
        &mut self,
        anchor_id: &'static str,
        options: Vec<SelectMenuOption>,
        multiple: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let anchor = self
            .select_menu_anchors
            .borrow()
            .get(anchor_id)
            .copied()
            .unwrap_or_else(|| Bounds::new(window.mouse_position(), Default::default()));
        let positioning_item = (!multiple)
            .then(|| options.iter().position(|option| option.checked))
            .flatten();
        let location = if positioning_item.is_some() {
            anchor.origin
        } else {
            gpui::point(anchor.left(), anchor.bottom() + px(4.0))
        };
        let entries = options
            .iter()
            .map(|option| NativeMenuEntry::Item {
                label: option.label.clone(),
                enabled: true,
                checked: option.checked,
            })
            .collect();
        let placement = NativeMenuPlacement {
            location,
            positioning_item,
            minimum_width: Some(anchor.size.width),
            opened_on_right_mouse_down: false,
        };
        let on_select = Box::new(
            move |this: &mut Self, index: usize, window: &mut Window, cx: &mut Context<Self>| {
                if let Some(option) = options.get(index) {
                    (option.action)(this, window, cx);
                }
            },
        );
        pop_up_menu(entries, placement, on_select, window, cx)
    }

    /// Records the bounds of `container`'s first child as the anchor for `anchor_id`'s menu.
    pub(super) fn track_select_menu_anchor(
        &self,
        anchor_id: &'static str,
        container: gpui::Div,
    ) -> gpui::Div {
        #[cfg(target_os = "macos")]
        {
            let anchors = self.select_menu_anchors.clone();
            container.on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    anchors.borrow_mut().insert(anchor_id, *bounds);
                }
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = anchor_id;
            container
        }
    }

    pub(super) fn run_context_menu_action(
        &mut self,
        action: &ContextMenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = None;
        action(self, window, cx);
        cx.notify();
    }

    pub(super) fn dismiss_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            cx.notify();
        }
    }

    /// Returns the dismiss layer and popover for an open GPUI context menu.
    pub(super) fn render_context_menu_layer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<[AnyElement; 2]> {
        let menu = self.context_menu.as_ref()?;
        let dismiss_layer = div()
            .id("context_menu_dismiss_layer")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .bg(self.theme.transparent)
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.dismiss_context_menu(cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.dismiss_context_menu(cx)),
            )
            .into_any_element();

        let height = CONTEXT_MENU_CHROME_HEIGHT
            + menu
                .entries
                .iter()
                .map(|entry| match entry {
                    ContextMenuEntry::Item { .. } => CONTEXT_MENU_ITEM_HEIGHT,
                    ContextMenuEntry::Separator => CONTEXT_MENU_SEPARATOR_HEIGHT,
                })
                .sum::<f32>();
        let viewport = window.viewport_size();
        let left = f32::from(menu.position.x)
            .min(f32::from(viewport.width) - CONTEXT_MENU_WIDTH - CONTEXT_MENU_EDGE_INSET)
            .max(CONTEXT_MENU_EDGE_INSET);
        let top = f32::from(menu.position.y)
            .min(f32::from(viewport.height) - height - CONTEXT_MENU_EDGE_INSET)
            .max(CONTEXT_MENU_EDGE_INSET);

        let mut popover = self
            .glass_floating_surface()
            .id("context_menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(CONTEXT_MENU_WIDTH))
            .flex()
            .flex_col()
            .p_1()
            .occlude();
        for entry in &menu.entries {
            popover = match entry {
                ContextMenuEntry::Item {
                    id,
                    icon,
                    label,
                    tone,
                    enabled,
                    action,
                } => {
                    let item =
                        self.render_context_menu_item(id, *icon, label.clone(), *tone, *enabled);
                    if *enabled {
                        let action = action.clone();
                        popover.child(item.on_click(cx.listener(move |this, _, window, cx| {
                            this.run_context_menu_action(&action, window, cx);
                        })))
                    } else {
                        popover.child(item)
                    }
                }
                ContextMenuEntry::Separator => popover.child(self.render_context_menu_separator()),
            };
        }

        Some([
            dismiss_layer,
            deferred(popover).with_priority(20).into_any_element(),
        ])
    }
}
