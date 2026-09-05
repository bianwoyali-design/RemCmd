//! Route Dock and system termination requests through the same asynchronous
//! save guard as the application's Quit action. GPUI's shutdown hook is too late
//! to cancel termination or await remote writes.
use gpui::{App, AsyncApp};
use objc2::runtime::{AnyObject, ClassBuilder, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::NSApplication;
use std::cell::{Cell, RefCell};

thread_local! {
    static CONTEXT: RefCell<Option<AsyncApp>> = const { RefCell::new(None) };
    static APPROVED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn approve_termination() {
    APPROVED.set(true);
}

extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    if APPROVED.get() {
        return 1; // NSTerminateNow
    }
    CONTEXT.with(|context| {
        if let Some(cx) = context.borrow().as_ref() {
            cx.spawn(async |cx| {
                let _ = cx.update(crate::app::request_application_exit);
            })
            .detach();
        }
    });
    0 // NSTerminateCancel; the guarded Quit action retries once approved.
}

pub(crate) fn install(cx: &App) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    CONTEXT.with(|context| *context.borrow_mut() = Some(cx.to_async()));
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: NSApplication owns a live delegate. The subclass adds no ivars,
    // inherits GPUI's delegate implementation, and only overrides the documented
    // termination decision selector with its native NSUInteger return type.
    unsafe {
        let delegate: *mut AnyObject = msg_send![&app, delegate];
        let Some(delegate) = delegate.as_ref() else {
            return;
        };
        if let Some(mut builder) = ClassBuilder::new(c"RemCmdApplicationDelegate", delegate.class())
        {
            builder.add_method(
                sel!(applicationShouldTerminate:),
                should_terminate as extern "C-unwind" fn(_, _, _) -> _,
            );
            AnyObject::set_class(delegate, builder.register());
        }
    }
}
