//! Native AppKit menus for GPUI-rendered views.

use std::cell::Cell;

use gpui::{Context, Pixels, Point, SharedString, Window};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSEvent, NSEventType, NSMenu, NSMenuItem, NSView,
};
use objc2_foundation::{NSPoint, NSProcessInfo, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::RemCmdApp;

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RemCmdContextMenuTarget"]
    #[ivars = Cell<Option<isize>>]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(selectItem:))]
        fn select_item(&self, sender: &NSMenuItem) {
            self.ivars().set(Some(sender.tag()));
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Cell::new(None));
        // SAFETY: NSObject's designated initializer takes no arguments.
        unsafe { msg_send![super(this), init] }
    }
}

pub(super) enum NativeMenuEntry {
    Item {
        label: SharedString,
        enabled: bool,
        checked: bool,
    },
    Separator,
}

/// Where a native menu appears, in GPUI window coordinates.
pub(super) struct NativeMenuPlacement {
    /// The top-left corner of `positioning_item`, or of the menu when there is none.
    pub(super) location: Point<Pixels>,
    /// The entry placed at `location`, as NSPopUpButton places its selected item.
    pub(super) positioning_item: Option<usize>,
    pub(super) minimum_width: Option<Pixels>,
    /// Whether the menu opened on a right mouse press whose release it will consume.
    pub(super) opened_on_right_mouse_down: bool,
}

pub(super) type NativeMenuSelection =
    Box<dyn FnOnce(&mut RemCmdApp, usize, &mut Window, &mut Context<RemCmdApp>)>;

/// Shows `entries` as an NSMenu, returning false when AppKit is unavailable.
///
/// `on_select` receives the index of the chosen entry after the menu closes.
pub(super) fn pop_up_menu(
    entries: Vec<NativeMenuEntry>,
    placement: NativeMenuPlacement,
    on_select: NativeMenuSelection,
    window: &mut Window,
    cx: &mut Context<RemCmdApp>,
) -> bool {
    let Some(view) = renderer_view(window) else {
        return false;
    };
    // AppKit tracks the menu in a nested run loop. Start it from a task so no GPUI
    // borrow is held while it runs and window redraws can continue.
    cx.spawn_in(window, async move |this, cx| {
        let selected = track_menu(&view, &entries, &placement);
        if placement.opened_on_right_mouse_down {
            // Menu tracking consumes the button release; replay it so pressed rows reset.
            post_right_mouse_up(&view);
        }
        let Some(index) = selected else {
            return;
        };
        let _ = this.update_in(cx, |this, window, cx| {
            on_select(this, index, window, cx);
            cx.notify();
        });
    })
    .detach();
    true
}

fn renderer_view(window: &Window) -> Option<Retained<NSView>> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return None;
    };
    // SAFETY: GPUI owns this NSView for the window's lifetime; retaining keeps it valid
    // until the menu closes.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    Some(view.retain())
}

fn track_menu(
    view: &NSView,
    entries: &[NativeMenuEntry],
    placement: &NativeMenuPlacement,
) -> Option<usize> {
    let mtm = MainThreadMarker::new()?;
    let target = MenuTarget::new(mtm);
    let target_object: &AnyObject = target.as_ref();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    if let Some(width) = placement.minimum_width {
        menu.setMinimumWidth(f64::from(f32::from(width)));
    }
    let no_key_equivalent = NSString::new();
    let mut positioning_item = None;

    for (index, entry) in entries.iter().enumerate() {
        let item = match entry {
            NativeMenuEntry::Item {
                label,
                enabled,
                checked,
            } => {
                let title = NSString::from_str(label);
                // SAFETY: `selectItem:` is implemented by MenuTarget with the NSMenuItem sender.
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &title,
                        Some(sel!(selectItem:)),
                        &no_key_equivalent,
                    )
                };
                // SAFETY: The target is retained until tracking below has finished.
                unsafe { item.setTarget(Some(target_object)) };
                item.setTag(index as isize);
                item.setEnabled(*enabled);
                item.setState(if *checked {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                item
            }
            NativeMenuEntry::Separator => NSMenuItem::separatorItem(mtm),
        };
        if placement.positioning_item == Some(index) {
            positioning_item = Some(item.clone());
        }
        menu.addItem(&item);
    }

    // GPUI reports top-left window coordinates; the renderer view is not flipped.
    let location = NSPoint::new(
        f64::from(f32::from(placement.location.x)),
        view.bounds().size.height - f64::from(f32::from(placement.location.y)),
    );
    menu.popUpMenuPositioningItem_atLocation_inView(
        positioning_item.as_deref(),
        location,
        Some(view),
    );
    target
        .ivars()
        .get()
        .and_then(|tag| usize::try_from(tag).ok())
}

fn post_right_mouse_up(view: &NSView) {
    let Some(window) = view.window() else {
        return;
    };
    let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        NSEventType::RightMouseUp,
        window.mouseLocationOutsideOfEventStream(),
        NSEvent::modifierFlags_class(),
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        0,
        1,
        0.0,
    ) else {
        return;
    };
    window.postEvent_atStart(&event, false);
}
