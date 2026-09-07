//! Accessibility for GPUI's custom-drawn controls. Native queries read a snapshot;
//! actions are queued back to GPUI, never re-entering a borrowed application.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use gpui::{prelude::*, *};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Button,
    CheckBox,
    TextField,
    TextArea,
    StaticText,
}
#[derive(Clone)]
pub(crate) enum Action {
    Press,
    Focus,
    SetValue(String),
    SetChecked(bool),
}
pub(crate) type Handler = Rc<dyn Fn(Action, &mut Window, &mut App)>;

pub(crate) struct Node {
    pub role: Role,
    pub label: SharedString,
    pub value: Option<String>,
    pub enabled: bool,
    pub selected: bool,
    pub secure: bool,
    pub focused: bool,
    pub focus_handle: Option<FocusHandle>,
    pub focus_marker: Option<Rc<std::cell::Cell<bool>>>,
    pub handler: Option<Handler>,
    pub menu: bool,
}
impl Node {
    pub fn button(label: impl Into<SharedString>, enabled: bool) -> Self {
        Self {
            role: Role::Button,
            label: label.into(),
            value: None,
            enabled,
            selected: false,
            secure: false,
            focused: false,
            focus_marker: None,
            focus_handle: None,
            handler: None,
            menu: false,
        }
    }
    pub fn text(label: impl Into<SharedString>, value: String) -> Self {
        Self {
            role: Role::StaticText,
            value: Some(value),
            ..Self::button(label, true)
        }
    }
}

/// An invisible child with exactly the parent's bounds. The global GPUI ID keeps
/// repeated controls (tabs, rows, panes) distinct without exposing their content.
pub(crate) fn node(id: impl Into<ElementId>, spec: Node) -> impl IntoElement {
    let id = id.into();
    canvas(
        move |bounds, window, _| {
            #[cfg(target_os = "macos")]
            window.with_global_id(id, |id, window| {
                native::collect(id.to_vec(), spec, bounds, window)
            });
            #[cfg(not(target_os = "macos"))]
            let _ = (id, spec, bounds, window);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(crate) fn install(cx: &App) {
    #[cfg(target_os = "macos")]
    cx.on_window_closed(native::remove_closed).detach();
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

pub(crate) fn root(child: impl IntoElement) -> impl IntoElement {
    Layer {
        inner: child.into_any_element(),
        modal: false,
        root: true,
    }
}
pub(crate) fn modal(child: impl IntoElement) -> impl IntoElement {
    Layer {
        inner: child.into_any_element(),
        modal: true,
        root: false,
    }
}
struct Layer {
    inner: AnyElement,
    modal: bool,
    root: bool,
}
impl IntoElement for Layer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for Layer {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.inner.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(target_os = "macos")]
        if self.root {
            native::begin(window, cx);
        }
        #[cfg(target_os = "macos")]
        let old = native::enter(self.modal);
        self.inner.prepaint(window, cx);
        #[cfg(target_os = "macos")]
        native::leave(old);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
        // All regular and deferred prepaint passes finish before painting starts.
        #[cfg(target_os = "macos")]
        if self.root {
            native::publish(window, cx);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (self.modal, self.root);
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2::{
        DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject,
    };
    use objc2_app_kit::*;
    use objc2_foundation::{
        MainThreadMarker, NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
    };

    struct Record {
        id: Vec<ElementId>,
        spec: Node,
        bounds: Bounds<Pixels>,
        layer: u8,
    }
    #[derive(Default)]
    struct Tree {
        pending: Vec<Record>,
        nodes: HashMap<Vec<ElementId>, Retained<Accessible>>,
        order: Vec<Vec<ElementId>>,
    }
    thread_local! {
        static TREES: RefCell<HashMap<WindowId, Tree>> = RefCell::new(HashMap::new());
        static LAYER: Cell<u8> = const { Cell::new(0) };
    }
    type Dispatch = Rc<dyn Fn(Action)>;
    #[derive(Default)]
    pub struct Ivars {
        value: RefCell<Option<String>>,
        checked: Cell<Option<bool>>,
        focused: Cell<bool>,
        selected: Cell<bool>,
        action: RefCell<Option<Dispatch>>,
        editable: Cell<bool>,
        focusable: Cell<bool>,
        bounds: Cell<Bounds<Pixels>>,
    }
    define_class!(
        // SAFETY: NSAccessibilityElement supports subclassing custom controls.
        #[unsafe(super = NSAccessibilityElement)]
        #[thread_kind = MainThreadOnly]
        #[ivars = Ivars]
        struct Accessible;
        unsafe impl NSObjectProtocol for Accessible {}
        impl Accessible {
            #[unsafe(method_id(accessibilityValue))]
            fn value(&self) -> Option<Retained<AnyObject>> {
                if let Some(checked) = self.ivars().checked.get() {
                    Some(objc2_foundation::NSNumber::new_bool(checked).into_super().into_super().into())
                } else {
                    self.ivars().value.borrow().as_ref().map(|value| NSString::from_str(value).into_super().into())
                }
            }
            #[unsafe(method(isAccessibilityFocused))]
            fn focused(&self) -> bool { self.ivars().focused.get() }
            #[unsafe(method(isAccessibilitySelected))]
            fn selected(&self) -> bool { self.ivars().selected.get() }
            #[unsafe(method(accessibilityPerformPress))]
            fn press(&self) -> bool { self.dispatch(Action::Press) }
            #[unsafe(method(setAccessibilityFocused:))]
            fn focus(&self, focused: bool) { if focused { self.dispatch(Action::Focus); } }
            #[unsafe(method(setAccessibilityValue:))]
            fn set_value(&self, value: Option<&AnyObject>) {
                if self.ivars().editable.get() && let Some(value) = value.and_then(|v| v.downcast_ref::<NSString>()) {
                    self.dispatch(Action::SetValue(value.to_string()));
                } else if let Some(checked) = self.ivars().checked.get()
                    && let Some(value) = value.and_then(|v| v.downcast_ref::<objc2_foundation::NSNumber>())
                    && checked != value.boolValue() {
                    self.dispatch(Action::SetChecked(value.boolValue()));
                }
            }
            #[unsafe(method(accessibilityIsAttributeSettable:))]
            fn attribute_settable(&self, attribute: &NSString) -> bool {
                match attribute.to_string().as_str() {
                    "AXValue" => self.ivars().editable.get() || (self.ivars().checked.get().is_some() && self.ivars().action.borrow().is_some()),
                    "AXFocused" => self.ivars().focusable.get(),
                    _ => false,
                }
            }
            #[unsafe(method(isAccessibilitySelectorAllowed:))]
            fn selector_allowed(&self, selector: objc2::runtime::Sel) -> bool {
                if selector == objc2::sel!(setAccessibilityValue:) { self.ivars().editable.get() || (self.ivars().checked.get().is_some() && self.ivars().action.borrow().is_some()) }
                else if selector == objc2::sel!(setAccessibilityFocused:) { self.ivars().focusable.get() }
                else if selector == objc2::sel!(accessibilityPerformPress) { self.ivars().action.borrow().is_some() }
                // SAFETY: Delegate other documented accessibility selectors to AppKit.
                else { unsafe { msg_send![super(self), isAccessibilitySelectorAllowed: selector] } }
            }
        }
    );
    impl Accessible {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(Ivars::default());
            // SAFETY: Initializes the NSAccessibilityElement superclass and installed ivars.
            unsafe { msg_send![super(this), init] }
        }
        fn dispatch(&self, action: Action) -> bool {
            let callback = self.ivars().action.borrow().clone();
            if let Some(callback) = callback {
                callback(action);
                true
            } else {
                false
            }
        }
        fn expire(&self) {
            self.ivars().action.borrow_mut().take();
            self.ivars().value.borrow_mut().take();
            self.ivars().editable.set(false);
            self.ivars().focusable.set(false);
            self.ivars().focused.set(false);
            self.setAccessibilityEnabled(false);
        }
    }
    extern "C-unwind" fn focused_element(
        window: &NSWindow,
        _: objc2::runtime::Sel,
    ) -> *mut AnyObject {
        if let Some(view) = window.contentView() {
            if let Some(children) = view.accessibilityChildren() {
                for child in children.iter() {
                    if let Some(node) = child.downcast_ref::<Accessible>()
                        && node.ivars().focused.get()
                    {
                        return Retained::autorelease_return(child.clone());
                    }
                }
            }
            let object: Retained<AnyObject> = view.into_super().into_super().into();
            return Retained::autorelease_return(object);
        }
        std::ptr::null_mut()
    }
    fn install_focus_forwarder(view: &NSView) {
        let Some(window) = view.window() else {
            return;
        };
        // SAFETY: Add no ivars, retain GPUI's entire NSWindow implementation, and
        // expose only the documented object-returning accessibility focus getter.
        unsafe {
            if window.class().name() == c"RemCmdAccessibleWindow" {
                return;
            }
            let class = if let Some(mut builder) =
                objc2::runtime::ClassBuilder::new(c"RemCmdAccessibleWindow", window.class())
            {
                builder.add_method(
                    objc2::sel!(accessibilityFocusedUIElement),
                    focused_element as extern "C-unwind" fn(_, _) -> _,
                );
                builder.register()
            } else {
                objc2::runtime::AnyClass::get(c"RemCmdAccessibleWindow").unwrap()
            };
            AnyObject::set_class(&window, class);
        }
    }
    fn activate(view: &NSView) {
        if let Some(window) = view.window() {
            window.makeKeyAndOrderFront(None);
            // macOS 13 compatibility; the replacement API requires macOS 14.
            #[allow(deprecated)]
            NSApplication::sharedApplication(view.mtm()).activateIgnoringOtherApps(true);
        }
    }
    fn press(view: &NSView, bounds: Bounds<Pixels>) {
        let Some(window) = view.window() else {
            return;
        };
        let center = bounds.center();
        let y = if view.isFlipped() {
            f64::from(center.y)
        } else {
            view.bounds().size.height - f64::from(center.y)
        };
        let point = view.convertPoint_toView(NSPoint::new(f64::from(center.x), y), None);
        for kind in [
            NSEventType::MouseMoved,
            NSEventType::LeftMouseDown,
            NSEventType::LeftMouseUp,
        ] {
            if let Some(event) = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind, point, NSEventModifierFlags::empty(), 0., window.windowNumber(), None, 0, 1, 0.
            ) {
                // Deliver through the same responder callbacks as physical input. This
                // runs after GPUI's update borrow ends and does not move the OS pointer.
                match kind {
                    NSEventType::MouseMoved => view.mouseMoved(&event),
                    NSEventType::LeftMouseDown => view.mouseDown(&event),
                    _ => view.mouseUp(&event),
                }
            }
        }
    }
    pub(super) fn enter(modal: bool) -> u8 {
        let old = LAYER.get();
        if modal {
            LAYER.set(1);
        }
        old
    }
    pub(super) fn leave(old: u8) {
        LAYER.set(old);
    }
    pub(super) fn remove_closed(cx: &mut App) {
        TREES.with(|trees| {
            trees.borrow_mut().retain(|id, tree| {
                let live = cx.windows().iter().any(|window| window.window_id() == *id);
                if !live {
                    for node in tree.nodes.values() {
                        node.expire();
                    }
                }
                live
            })
        });
    }
    pub(super) fn begin(window: &Window, _: &App) {
        TREES.with(|trees| {
            trees
                .borrow_mut()
                .entry(window.window_handle().window_id())
                .or_default()
                .pending
                .clear()
        });
    }
    pub(super) fn collect(
        id: Vec<ElementId>,
        mut spec: Node,
        bounds: Bounds<Pixels>,
        window: &Window,
    ) {
        let bounds = bounds.intersect(&window.content_mask().bounds);
        if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
            return;
        }
        spec.focused |= spec
            .focus_handle
            .as_ref()
            .is_some_and(|focus| focus.is_focused(window));
        spec.focused |= spec
            .focus_marker
            .as_ref()
            .is_some_and(|marker| marker.get());
        let layer = if spec.menu { 2 } else { LAYER.get() };
        TREES.with(|trees| {
            trees
                .borrow_mut()
                .entry(window.window_handle().window_id())
                .or_default()
                .pending
                .push(Record {
                    id,
                    spec,
                    bounds,
                    layer,
                })
        });
    }
    pub(super) fn publish(window: &Window, cx: &App) {
        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: GPUI supplies a live NSView on the main thread, valid for this call.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
        install_focus_forwarder(view);
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let mut notifications = Vec::new();
        let mut layout_changed = false;
        TREES.with(|trees| {
            let mut trees = trees.borrow_mut();
            let tree = trees.entry(window.window_handle().window_id()).or_default();
            let layer = tree
                .pending
                .iter()
                .map(|record| record.layer)
                .max()
                .unwrap_or(0);
            let pending = std::mem::take(&mut tree.pending);
            let mut order = Vec::new();
            for Record {
                id,
                spec,
                bounds,
                layer: node_layer,
            } in pending
            {
                if node_layer != layer {
                    continue;
                }
                order.push(id.clone());
                let node = tree
                    .nodes
                    .entry(id.clone())
                    .or_insert_with(|| Accessible::new(mtm));
                // SAFETY: These are immutable AppKit role constants.
                let role = unsafe {
                    match spec.role {
                        Role::Button => NSAccessibilityButtonRole,
                        Role::CheckBox => NSAccessibilityCheckBoxRole,
                        Role::TextField => NSAccessibilityTextFieldRole,
                        Role::TextArea => NSAccessibilityTextAreaRole,
                        Role::StaticText => NSAccessibilityStaticTextRole,
                    }
                };
                node.setAccessibilityRole(Some(role));
                node.setAccessibilitySubrole(
                    spec.secure
                        .then_some(unsafe { NSAccessibilitySecureTextFieldSubrole }),
                );
                node.setAccessibilityLabel(Some(&NSString::from_str(&spec.label)));
                node.setAccessibilityEnabled(spec.enabled);
                node.setAccessibilityElement(true);
                // SAFETY: NSView is the real parent; AppKit stores a weak reference.
                unsafe {
                    node.setAccessibilityParent(Some(view));
                }
                let y = if view.isFlipped() {
                    f64::from(bounds.origin.y)
                } else {
                    view.bounds().size.height - f64::from(bounds.bottom())
                };
                node.setAccessibilityFrameInParentSpace(NSRect::new(
                    NSPoint::new(f64::from(bounds.origin.x), y),
                    NSSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)),
                ));
                let checked = (spec.role == Role::CheckBox).then_some(spec.selected);
                let checked_changed = node.ivars().checked.replace(checked) != checked;
                if checked_changed || *node.ivars().value.borrow() != spec.value {
                    *node.ivars().value.borrow_mut() = spec.value;
                    notifications.push((node.clone(), unsafe {
                        NSAccessibilityValueChangedNotification
                    }));
                }
                if node.ivars().focused.replace(spec.focused) != spec.focused && spec.focused {
                    notifications.push((node.clone(), unsafe {
                        NSAccessibilityFocusedUIElementChangedNotification
                    }));
                }
                node.ivars().selected.set(spec.selected);
                node.ivars()
                    .editable
                    .set(spec.role == Role::TextField && spec.enabled);
                node.ivars().focusable.set(
                    spec.enabled
                        && (spec.focus_handle.is_some()
                            || (matches!(spec.role, Role::TextField | Role::TextArea)
                                && spec.handler.is_some())),
                );
                node.ivars().bounds.set(bounds);
                let actionable = spec.enabled
                    && (matches!(spec.role, Role::Button | Role::CheckBox)
                        || spec.handler.is_some());
                *node.ivars().action.borrow_mut() = actionable.then(|| {
                    let context = cx.to_async();
                    let handle = window.window_handle();
                    let handler = spec.handler;
                    let weak_view = objc2::rc::Weak::new(view);
                    let weak_node = objc2::rc::Weak::from_retained(node);
                    Rc::new(move |action: Action| {
                        let handler = handler.clone();
                        let weak_view = weak_view.clone();
                        let weak_node = weak_node.clone();
                        context
                            .spawn(async move |cx| {
                                let Some(node) = weak_node.load() else {
                                    return;
                                };
                                if node.ivars().action.borrow().is_none() {
                                    return;
                                }
                                let Some(view) = weak_view.load() else {
                                    return;
                                };
                                activate(&view);
                                if let Some(handler) = handler {
                                    let _ = handle
                                        .update(cx, |_, window, cx| handler(action, window, cx));
                                } else if matches!(action, Action::Press) {
                                    press(&view, node.ivars().bounds.get());
                                }
                            })
                            .detach();
                    }) as Dispatch
                });
            }
            tree.nodes.retain(|id, node| {
                let live = order.contains(id);
                if !live {
                    node.expire();
                }
                live
            });
            layout_changed = tree.order != order;
            tree.order = order;
            if layout_changed {
                let children: Vec<&AnyObject> = tree
                    .order
                    .iter()
                    .filter_map(|id| tree.nodes.get(id))
                    .map(|node| &**node as &AnyObject)
                    .collect();
                // SAFETY: Every child is an initialized NSAccessibilityElement retained by the tree.
                unsafe {
                    view.setAccessibilityChildren(Some(&NSArray::from_slice(&children)));
                }
            }
        });
        // Posting after releasing the registry allows native clients to query synchronously.
        for (node, notification) in notifications {
            unsafe {
                NSAccessibilityPostNotification(&node, notification);
            }
        }
        if layout_changed {
            unsafe {
                NSAccessibilityPostNotification(view, NSAccessibilityLayoutChangedNotification);
            }
        }
    }
}
