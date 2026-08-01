#[cfg(target_os = "windows")]
use gpui::Window;
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::ReleaseCapture,
        WindowsAndMessaging::{HTCAPTION, PostMessageW, WM_NCLBUTTONDOWN},
    },
};

#[cfg(target_os = "windows")]
pub(crate) fn begin_drag(window: &Window) {
    let Some(hwnd) = window_hwnd(window) else {
        return;
    };

    unsafe {
        let _ = ReleaseCapture();
        let _ = PostMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
    }
}

#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn begin_drag(_: &gpui::Window) {}
