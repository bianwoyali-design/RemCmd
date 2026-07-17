use gpui::{App, Global, Hsla, Window, WindowAppearance, div, hsla, prelude::*, px, rgb, rgba};

use remcmd_core::ThemeMode;

/// Semantic color tokens for the whole application. Every render helper reads
/// colors from a `Theme` so light and dark appearances stay consistent.
///
/// `Theme` is stored as a GPUI global (see `set_global_theme`) so widgets that
/// do not receive it explicitly (like the text field) can read the active
/// palette from any `&App`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub text_primary: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,

    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub on_accent: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub error_text: Hsla,
    pub status_ok: Hsla,
    pub status_warn: Hsla,

    pub sidebar_bg: Hsla,
    pub panel_bg: Hsla,
    pub surface_bg: Hsla,
    pub modal_bg: Hsla,
    pub overlay_bg: Hsla,
    pub transparent: Hsla,
    pub shadow: Hsla,

    pub border: Hsla,
    pub border_strong: Hsla,

    pub list_selected_bg: Hsla,
    pub list_hover_bg: Hsla,
    pub list_selected_hover_bg: Hsla,
    pub control_bg: Hsla,
    pub control_hover_bg: Hsla,

    pub input_cursor: Hsla,
    pub input_placeholder: Hsla,
    pub selection_bg: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Self {
            text_primary: opaque(0xf4f4f5),
            text_muted: opaque(0xa1a1aa),
            text_faint: opaque(0x71717a),

            accent: opaque(0x2563eb),
            accent_hover: opaque(0x3b82f6),
            on_accent: opaque(0xffffff),
            danger: opaque(0xdc2626),
            danger_hover: opaque(0xef4444),
            error_text: opaque(0xfca5a5),
            status_ok: opaque(0x86efac),
            status_warn: opaque(0xfde68a),

            sidebar_bg: alpha(0x212121e8),
            panel_bg: opaque(0x181818),
            surface_bg: alpha(0xffffff12),
            modal_bg: opaque(0x242424),
            overlay_bg: alpha(0x0000008f),
            transparent: alpha(0x00000000),
            shadow: alpha(0x00000042),

            border: alpha(0xffffff26),
            border_strong: alpha(0xffffff40),

            list_selected_bg: opaque(0x4f4d50),
            list_hover_bg: opaque(0x454347),
            list_selected_hover_bg: opaque(0x59575b),
            control_bg: alpha(0xffffff0d),
            control_hover_bg: alpha(0xffffff1f),

            input_cursor: hsla(0.0, 0.0, 1.0, 0.9),
            input_placeholder: hsla(0.0, 0.0, 1.0, 0.45),
            selection_bg: alpha(0x60a5fa55),
        }
    }

    pub fn light() -> Self {
        Self {
            text_primary: opaque(0x1a1a1a),
            text_muted: opaque(0x5f5f66),
            text_faint: opaque(0x8b8b92),

            accent: opaque(0x2563eb),
            accent_hover: opaque(0x1d4ed8),
            on_accent: opaque(0xffffff),
            danger: opaque(0xdc2626),
            danger_hover: opaque(0xb91c1c),
            error_text: opaque(0xb91c1c),
            status_ok: opaque(0x15803d),
            status_warn: opaque(0xa16207),

            sidebar_bg: alpha(0xf1f1f3e8),
            panel_bg: opaque(0xffffff),
            surface_bg: alpha(0x00000008),
            modal_bg: opaque(0xffffff),
            overlay_bg: alpha(0x00000059),
            transparent: alpha(0x00000000),
            shadow: alpha(0x00000024),

            border: alpha(0x0000001a),
            border_strong: alpha(0x00000038),

            list_selected_bg: alpha(0x00000012),
            list_hover_bg: alpha(0x00000008),
            list_selected_hover_bg: alpha(0x00000018),
            control_bg: alpha(0x00000005),
            control_hover_bg: alpha(0x0000000f),

            input_cursor: hsla(0.0, 0.0, 0.0, 0.8),
            input_placeholder: hsla(0.0, 0.0, 0.0, 0.4),
            selection_bg: alpha(0x2563eb33),
        }
    }

    /// Resolves the palette for a mode against the window's current appearance.
    pub fn resolve(mode: ThemeMode, window: &Window) -> Self {
        match mode {
            ThemeMode::Light => Self::light(),
            ThemeMode::Dark => Self::dark(),
            ThemeMode::System => Self::for_appearance(window.appearance()),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
        }
    }
}

fn opaque(value: u32) -> Hsla {
    rgb(value).into()
}

fn alpha(value: u32) -> Hsla {
    rgba(value).into()
}

/// Installs the theme as a GPUI global so any widget can read it via
/// `cx.global::<Theme>()`.
pub fn set_global_theme(theme: Theme, cx: &mut App) {
    cx.set_global(theme);
}

/// The visual treatment of an interactive button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

/// Builds a consistently styled, Codex-like button. Returns the configured
/// element before its `on_click` handler is attached so callers stay in charge
/// of behavior while the look stays centralized.
pub fn button(
    id: &'static str,
    label: &'static str,
    variant: ButtonVariant,
    enabled: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let (base_bg, hover_bg, text) = match variant {
        ButtonVariant::Primary => (theme.accent, theme.accent_hover, theme.on_accent),
        ButtonVariant::Danger => (theme.danger, theme.danger_hover, theme.on_accent),
        ButtonVariant::Secondary => (theme.surface_bg, theme.control_hover_bg, theme.text_primary),
        ButtonVariant::Ghost => (
            theme.transparent,
            theme.control_hover_bg,
            theme.text_primary,
        ),
    };

    let mut el = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .px_3()
        .py(px(6.0))
        .rounded_md()
        .bg(base_bg)
        .text_color(text)
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .whitespace_nowrap()
        .child(label);

    if enabled {
        el = el.cursor_pointer().hover(move |this| this.bg(hover_bg));
    } else {
        el = el.opacity(0.5);
    }

    if matches!(variant, ButtonVariant::Secondary) {
        el = el.border_1().border_color(theme.border);
    }

    el
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_resolves_to_the_matching_palette() {
        assert_eq!(
            Theme::for_appearance(WindowAppearance::Light),
            Theme::light()
        );
        assert_eq!(
            Theme::for_appearance(WindowAppearance::VibrantLight),
            Theme::light()
        );
        assert_eq!(Theme::for_appearance(WindowAppearance::Dark), Theme::dark());
        assert_eq!(
            Theme::for_appearance(WindowAppearance::VibrantDark),
            Theme::dark()
        );
    }
}
