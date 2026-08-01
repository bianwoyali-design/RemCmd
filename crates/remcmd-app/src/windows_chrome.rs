#[cfg(target_os = "windows")]
use gpui::Window;
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{PostMessageW, SC_MAXIMIZE, SC_RESTORE, WM_SYSCOMMAND},
};

#[cfg(target_os = "windows")]
pub(crate) fn toggle_maximize(window: &Window) {
    let Some(hwnd) = window_hwnd(window) else {
        window.zoom_window();
        return;
    };
    let command = if window.is_maximized() {
        SC_RESTORE
    } else {
        SC_MAXIMIZE
    };

    unsafe {
        let _ = PostMessageW(hwnd, WM_SYSCOMMAND, command as usize, 0);
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
pub(crate) fn toggle_maximize(window: &gpui::Window) {
    window.zoom_window();
}
