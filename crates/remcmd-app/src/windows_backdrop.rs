use std::{ffi::c_void, mem::size_of};

use gpui::{Window, WindowBackgroundAppearance};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{
        DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    },
    UI::Controls::MARGINS,
};

pub(crate) fn apply_mica(window: &Window, dark: bool) {
    window.set_background_appearance(WindowBackgroundAppearance::Opaque);

    let Some(hwnd) = window_hwnd(window) else {
        window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        return;
    };
    let backdrop = DWMSBT_MAINWINDOW;
    let backdrop_result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&backdrop as *const i32).cast::<c_void>(),
            size_of::<i32>() as u32,
        )
    };
    if backdrop_result < 0 {
        window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        return;
    }

    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    let frame_result = unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) };
    if frame_result < 0 {
        let none = DWMSBT_NONE;
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&none as *const i32).cast::<c_void>(),
                size_of::<i32>() as u32,
            );
        }
        window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        return;
    }

    let dark = i32::from(dark);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&dark as *const i32).cast::<c_void>(),
            size_of::<i32>() as u32,
        );
    }
}

fn window_hwnd(window: &Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(handle.hwnd.get() as HWND)
}
