use gpui::Hsla;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlatformPreferences {
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub accent: Option<Hsla>,
}

impl PlatformPreferences {
    pub fn read() -> Self {
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSColor, NSColorSpace, NSWorkspace};
            if MainThreadMarker::new().is_none() {
                return Self::default();
            }
            let workspace = NSWorkspace::sharedWorkspace();
            let accent = NSColor::controlAccentColor()
                .colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())
                .map(|color| {
                    gpui::Rgba {
                        r: color.redComponent() as f32,
                        g: color.greenComponent() as f32,
                        b: color.blueComponent() as f32,
                        a: 1.0,
                    }
                    .into()
                });
            Self {
                reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
                reduce_transparency: workspace.accessibilityDisplayShouldReduceTransparency(),
                increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
                accent,
            }
        }
        #[cfg(not(target_os = "macos"))]
        Self::default()
    }
}
