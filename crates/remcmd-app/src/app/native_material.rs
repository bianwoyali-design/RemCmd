//! Native macOS material behind GPUI's transparent sidebar pixels.

use gpui::Window;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use remcmd_core::ThemeMode;

pub(super) struct SidebarBackdrop {
    view: Retained<NSVisualEffectView>,
}

impl SidebarBackdrop {
    pub(super) fn install(
        window: &Window,
        theme_mode: ThemeMode,
        reduce_transparency: bool,
    ) -> Option<Self> {
        if reduce_transparency {
            return None;
        }
        let mtm = MainThreadMarker::new()?;
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // GPUI owns this NSView. The borrowed handle is live for this main-thread call.
        let renderer_view = unsafe { &*handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
        let content_view = renderer_view.window()?.contentView()?;
        let view = NSVisualEffectView::new(mtm);
        view.setFrame(content_view.bounds());
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setMaterial(NSVisualEffectMaterial::Sidebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        Self::set_appearance(&view, theme_mode);
        content_view.addSubview_positioned_relativeTo(
            &view,
            NSWindowOrderingMode::Below,
            Some(renderer_view),
        );
        Some(Self { view })
    }

    pub(super) fn sync(
        backdrop: &mut Option<Self>,
        window: &Window,
        theme_mode: ThemeMode,
        reduce_transparency: bool,
    ) {
        if reduce_transparency {
            if let Some(backdrop) = backdrop.take() {
                backdrop.view.removeFromSuperview();
            }
        } else if let Some(backdrop) = backdrop {
            Self::set_appearance(&backdrop.view, theme_mode);
        } else {
            *backdrop = Self::install(window, theme_mode, false);
        }
    }

    fn set_appearance(view: &NSVisualEffectView, theme_mode: ThemeMode) {
        let name = match theme_mode {
            ThemeMode::System => {
                view.setAppearance(None);
                return;
            }
            ThemeMode::Light => unsafe { NSAppearanceNameAqua },
            ThemeMode::Dark => unsafe { NSAppearanceNameDarkAqua },
        };
        view.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }
}
