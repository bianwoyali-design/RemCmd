use std::borrow::Cow;

use gpui::{
    AnyElement, AssetSource, Hsla, IntoElement, Result, SharedString, div, img, prelude::*, px, svg,
};

use crate::theme::{IconTone, Theme};

const APP_ICON_PATH: &str = "brand/remcmd-app-icon.png";
const WORDMARK_REM_PATH: &str = "brand/remcmd-wordmark-rem.svg";
const WORDMARK_COMMAND_PATH: &str = "brand/remcmd-wordmark-command.svg";

// RemCmd-owned icon paths use a shared 24px view box with rounded caps and joins.
//
// Icons are drawn in three levels, like hierarchical system symbols: the outline at
// full strength, secondary detail at half opacity, and a soft fill on the part the
// icon is about. Rendering keeps only alpha, so every level follows the tint.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IconName {
    Add,
    About,
    Alias,
    ArrowLeft,
    ArrowUp,
    Cancel,
    Check,
    ClosePane,
    Collapse,
    Connect,
    Copy,
    Delete,
    Download,
    Disconnect,
    Edit,
    Expand,
    File,
    Folder,
    ForgetCredential,
    Home,
    NewConnection,
    PanelBottom,
    Paste,
    Performance,
    Picker,
    QuickCommand,
    Reconnect,
    Search,
    SelectAll,
    Server,
    Settings,
    SidebarLeft,
    SidebarRight,
    SplitDown,
    SplitRight,
    Terminal,
    Upload,
    View,
}

impl IconName {
    const fn asset_path(self) -> &'static str {
        match self {
            Self::Add => "icons/add.svg",
            Self::About => "icons/about.svg",
            Self::Alias => "icons/alias.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Cancel => "icons/cancel.svg",
            Self::Check => "icons/check.svg",
            Self::ClosePane => "icons/close-pane.svg",
            Self::Collapse => "icons/collapse.svg",
            Self::Connect => "icons/connect.svg",
            Self::Copy => "icons/copy.svg",
            Self::Delete => "icons/delete.svg",
            Self::Download => "icons/download.svg",
            Self::Disconnect => "icons/disconnect.svg",
            Self::Edit => "icons/edit.svg",
            Self::Expand => "icons/expand.svg",
            Self::File => "icons/file.svg",
            Self::Folder => "icons/folder.svg",
            Self::ForgetCredential => "icons/forget-credential.svg",
            Self::Home => "icons/home.svg",
            Self::NewConnection => "icons/new-connection.svg",
            Self::PanelBottom => "icons/panel-bottom.svg",
            Self::Paste => "icons/paste.svg",
            Self::Performance => "icons/performance.svg",
            Self::Picker => "icons/picker.svg",
            Self::QuickCommand => "icons/quick-command.svg",
            Self::Reconnect => "icons/reconnect.svg",
            Self::Search => "icons/search.svg",
            Self::SelectAll => "icons/select-all.svg",
            Self::Server => "icons/server.svg",
            Self::Settings => "icons/settings.svg",
            Self::SidebarLeft => "icons/sidebar-left.svg",
            Self::SidebarRight => "icons/sidebar-right.svg",
            Self::SplitDown => "icons/split-down.svg",
            Self::SplitRight => "icons/split-right.svg",
            Self::Terminal => "icons/terminal.svg",
            Self::Upload => "icons/upload.svg",
            Self::View => "icons/view.svg",
        }
    }

    fn from_asset_path(path: &str) -> Option<Self> {
        Some(match path {
            "icons/add.svg" => Self::Add,
            "icons/about.svg" => Self::About,
            "icons/alias.svg" => Self::Alias,
            "icons/arrow-left.svg" => Self::ArrowLeft,
            "icons/arrow-up.svg" => Self::ArrowUp,
            "icons/cancel.svg" => Self::Cancel,
            "icons/check.svg" => Self::Check,
            "icons/close-pane.svg" => Self::ClosePane,
            "icons/collapse.svg" => Self::Collapse,
            "icons/connect.svg" => Self::Connect,
            "icons/copy.svg" => Self::Copy,
            "icons/delete.svg" => Self::Delete,
            "icons/download.svg" => Self::Download,
            "icons/disconnect.svg" => Self::Disconnect,
            "icons/edit.svg" => Self::Edit,
            "icons/expand.svg" => Self::Expand,
            "icons/file.svg" => Self::File,
            "icons/folder.svg" => Self::Folder,
            "icons/forget-credential.svg" => Self::ForgetCredential,
            "icons/home.svg" => Self::Home,
            "icons/new-connection.svg" => Self::NewConnection,
            "icons/panel-bottom.svg" => Self::PanelBottom,
            "icons/paste.svg" => Self::Paste,
            "icons/performance.svg" => Self::Performance,
            "icons/picker.svg" => Self::Picker,
            "icons/quick-command.svg" => Self::QuickCommand,
            "icons/reconnect.svg" => Self::Reconnect,
            "icons/search.svg" => Self::Search,
            "icons/select-all.svg" => Self::SelectAll,
            "icons/server.svg" => Self::Server,
            "icons/settings.svg" => Self::Settings,
            "icons/sidebar-left.svg" => Self::SidebarLeft,
            "icons/sidebar-right.svg" => Self::SidebarRight,
            "icons/split-down.svg" => Self::SplitDown,
            "icons/split-right.svg" => Self::SplitRight,
            "icons/terminal.svg" => Self::Terminal,
            "icons/upload.svg" => Self::Upload,
            "icons/view.svg" => Self::View,
            _ => return None,
        })
    }

    const fn body(self) -> &'static str {
        match self {
            Self::Add => r#"<path d="M12 5v14M5 12h14"/>"#,
            Self::About => {
                r#"<circle cx="12" cy="12" r="8.75"/><circle cx="12" cy="8" r="0.9" fill="currentColor" stroke="none"/><path d="M12 11.25v5.25"/>"#
            }
            Self::Alias => {
                r#"<path d="M7 17 16.5 7.5" stroke-width="2.4"/><path d="M9 7.5h7.5V15" stroke-width="2.4"/>"#
            }
            Self::ArrowLeft => r#"<path d="M14.75 5.5 8.25 12l6.5 6.5"/>"#,
            Self::ArrowUp => r#"<path d="m5.5 14.75 6.5-6.5 6.5 6.5"/>"#,
            Self::Cancel => r#"<path d="m6.25 6.25 11.5 11.5m0-11.5-11.5 11.5"/>"#,
            Self::Check => r#"<path d="m5 12.5 4.5 4.5 9.5-10" stroke-width="1.8"/>"#,
            Self::ClosePane => {
                r#"<path d="M14.5 4.5H18A3.5 3.5 0 0 1 21.5 8v8a3.5 3.5 0 0 1-3.5 3.5h-3.5Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M14.5 4.5v15"/><path d="m16.75 10.75 2.5 2.5m0-2.5-2.5 2.5" stroke-width="1.1"/>"#
            }
            Self::Collapse => r#"<path d="m6.5 9.5 5.5 5.5 5.5-5.5"/>"#,
            Self::Connect => {
                r#"<rect x="16.5" y="4.5" width="4.5" height="15" rx="2.25" fill="currentColor" fill-opacity="0.16"/><path d="M3 12h13.5"/><path d="m12.75 8.25 3.75 3.75-3.75 3.75"/>"#
            }
            Self::Copy => {
                r#"<rect x="8" y="8" width="12.5" height="12.5" rx="3" fill="currentColor" fill-opacity="0.14"/><path d="M16 8V6.5a3 3 0 0 0-3-3H6.5a3 3 0 0 0-3 3V13a3 3 0 0 0 3 3H8"/>"#
            }
            Self::Delete => {
                r#"<path d="M6 7h12l-.72 11.13A2 2 0 0 1 15.28 20H8.72a2 2 0 0 1-2-1.87Z" fill="currentColor" fill-opacity="0.12" stroke="none"/><path d="M4 7h16"/><path d="m6 7 .72 11.13A2 2 0 0 0 8.72 20h6.56a2 2 0 0 0 2-1.87L18 7"/><path d="M9.5 7V5.5A1.5 1.5 0 0 1 11 4h2a1.5 1.5 0 0 1 1.5 1.5V7"/><path d="M10 10.75v5.5M14 10.75v5.5" opacity="0.5"/>"#
            }
            Self::Download => {
                r#"<path d="M12 3.5v11.25"/><path d="m7.75 10.5 4.25 4.25 4.25-4.25"/><path d="M4 16v2.5A2.5 2.5 0 0 0 6.5 21h11a2.5 2.5 0 0 0 2.5-2.5V16"/>"#
            }
            Self::Disconnect => {
                r#"<rect x="3" y="4.5" width="4.5" height="15" rx="2.25" fill="currentColor" fill-opacity="0.16"/><path d="M7.5 12H21"/><path d="m17.25 8.25 3.75 3.75-3.75 3.75"/>"#
            }
            Self::Edit => {
                r#"<path d="m4.5 19.5 1.05-4.3 3.25 3.25Z" fill="currentColor" fill-opacity="0.3" stroke="none"/><path d="m4.5 19.5 1.05-4.3L15.8 4.95a2 2 0 0 1 2.83 0l.42.42a2 2 0 0 1 0 2.83L8.8 18.45l-4.3 1.05Z"/><path d="m13.75 7 3.25 3.25"/>"#
            }
            Self::Expand => r#"<path d="m9.5 6.5 5.5 5.5-5.5 5.5"/>"#,
            Self::File => {
                r#"<path d="M13.5 3 19 8.5h-3.5a2 2 0 0 1-2-2Z" fill="currentColor" fill-opacity="0.2" stroke="none"/><path d="M7 3h6.5L19 8.5V19a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z"/><path d="M13.5 3v3.5a2 2 0 0 0 2 2H19"/><path d="M8.5 13h7M8.5 16.5h4.25" opacity="0.5"/>"#
            }
            Self::Folder => {
                r#"<path d="M3 10.5h18V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><path d="M3 7a2 2 0 0 1 2-2h3.67a2 2 0 0 1 1.5.68L11.33 7H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>"#
            }
            Self::ForgetCredential => {
                r#"<circle cx="7.5" cy="14.5" r="3.75" fill="currentColor" fill-opacity="0.16"/><path d="M11.25 14.5H20.5m-3 0v2.75m-3-2.75v1.75"/><circle cx="18" cy="6" r="3.25"/><path d="M16.5 6h3" stroke-width="1.2"/>"#
            }
            Self::Home => {
                r#"<path d="M9.75 20v-4.75A1.25 1.25 0 0 1 11 14h2a1.25 1.25 0 0 1 1.25 1.25V20Z" fill="currentColor" fill-opacity="0.2" stroke="none"/><path d="m3.5 11 8.5-7 8.5 7"/><path d="M5.5 9.5V18a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V9.5"/><path d="M9.75 20v-4.75A1.25 1.25 0 0 1 11 14h2a1.25 1.25 0 0 1 1.25 1.25V20"/>"#
            }
            Self::NewConnection => {
                r#"<path d="M14 5.5H6A2.5 2.5 0 0 0 3.5 8v9.5A2.5 2.5 0 0 0 6 20h10a2.5 2.5 0 0 0 2.5-2.5v-7"/><path d="M3.5 12.75h15"/><circle cx="6.75" cy="9.1" r="0.85" fill="currentColor" stroke="none"/><circle cx="6.75" cy="16.4" r="0.85" fill="currentColor" stroke="none"/><circle cx="18.5" cy="5.5" r="3.25" fill="currentColor" fill-opacity="0.16"/><path d="M18.5 4v3M17 5.5h3" stroke-width="1.2"/>"#
            }
            Self::PanelBottom => {
                r#"<path d="M2.5 13.5h19V16a3.5 3.5 0 0 1-3.5 3.5H6A3.5 3.5 0 0 1 2.5 16Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M2.5 13.5h19"/>"#
            }
            Self::Paste => {
                r#"<rect x="8.75" y="3" width="6.5" height="3.75" rx="1.5" fill="currentColor" fill-opacity="0.2"/><path d="M8.75 4.75H7a2 2 0 0 0-2 2V19a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V6.75a2 2 0 0 0-2-2h-1.75"/><path d="M8.75 12h6.5M8.75 15.5h4" opacity="0.5"/>"#
            }
            Self::Performance => r#"<path d="M2.75 12.5h3.6l2.4-6.5 4.5 12 2.4-5.5h5.6"/>"#,
            Self::Picker => {
                r#"<path d="m8.25 9.75 3.75-3.5 3.75 3.5" stroke-width="1.9"/><path d="m8.25 14.25 3.75 3.5 3.75-3.5" stroke-width="1.9"/>"#
            }
            Self::QuickCommand => {
                r#"<rect x="2.5" y="4.5" width="19" height="15" rx="3.5" fill="currentColor" fill-opacity="0.1"/><path d="m6.25 9.25 3 2.75-3 2.75"/><path d="m16 8.5-2.75 3.75h3.5L14 16"/>"#
            }
            Self::Reconnect => {
                r#"<path d="M20 12a8 8 0 1 1-2.35-5.65L20 8.75"/><path d="M20 4.25v4.5h-4.5"/>"#
            }
            Self::Search => {
                r#"<circle cx="10.5" cy="10.5" r="6.75"/><path d="m15.4 15.4 5.1 5.1"/>"#
            }
            Self::SelectAll => {
                r#"<path d="M4 9V6a2 2 0 0 1 2-2h3M15 4h3a2 2 0 0 1 2 2v3M20 15v3a2 2 0 0 1-2 2h-3M9 20H6a2 2 0 0 1-2-2v-3"/><rect x="8.5" y="8.5" width="7" height="7" rx="1.75" fill="currentColor" fill-opacity="0.18"/>"#
            }
            Self::Server => {
                r#"<rect x="3" y="4" width="18" height="7" rx="2.5" fill="currentColor" fill-opacity="0.1"/><rect x="3" y="13" width="18" height="7" rx="2.5" fill="currentColor" fill-opacity="0.1"/><circle cx="6.75" cy="7.5" r="0.9" fill="currentColor" stroke="none"/><circle cx="6.75" cy="16.5" r="0.9" fill="currentColor" stroke="none"/><path d="M10.5 7.5h6.75M10.5 16.5h6.75" opacity="0.5"/>"#
            }
            Self::Settings => {
                r#"<path d="M10.67 3.1A9 9 0 0 1 13.33 3.1L13.81 5.24A7 7 0 0 1 15.5 5.94L17.35 4.77A9 9 0 0 1 19.23 6.65L18.06 8.5A7 7 0 0 1 18.76 10.19L20.9 10.67A9 9 0 0 1 20.9 13.33L18.76 13.81A7 7 0 0 1 18.06 15.5L19.23 17.35A9 9 0 0 1 17.35 19.23L15.5 18.06A7 7 0 0 1 13.81 18.76L13.33 20.9A9 9 0 0 1 10.67 20.9L10.19 18.76A7 7 0 0 1 8.5 18.06L6.65 19.23A9 9 0 0 1 4.77 17.35L5.94 15.5A7 7 0 0 1 5.24 13.81L3.1 13.33A9 9 0 0 1 3.1 10.67L5.24 10.19A7 7 0 0 1 5.94 8.5L4.77 6.65A9 9 0 0 1 6.65 4.77L8.5 5.94A7 7 0 0 1 10.19 5.24Z"/><circle cx="12" cy="12" r="3" fill="currentColor" fill-opacity="0.16"/>"#
            }
            Self::SidebarLeft => {
                r#"<path d="M9 4.5H6A3.5 3.5 0 0 0 2.5 8v8A3.5 3.5 0 0 0 6 19.5h3Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M9 4.5v15"/>"#
            }
            Self::SidebarRight => {
                r#"<path d="M15 4.5h3A3.5 3.5 0 0 1 21.5 8v8a3.5 3.5 0 0 1-3.5 3.5h-3Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M15 4.5v15"/>"#
            }
            Self::SplitDown => {
                r#"<path d="M2.5 12h19v4a3.5 3.5 0 0 1-3.5 3.5H6A3.5 3.5 0 0 1 2.5 16Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M2.5 12h19"/>"#
            }
            Self::SplitRight => {
                r#"<path d="M12 4.5h6A3.5 3.5 0 0 1 21.5 8v8a3.5 3.5 0 0 1-3.5 3.5h-6Z" fill="currentColor" fill-opacity="0.16" stroke="none"/><rect x="2.5" y="4.5" width="19" height="15" rx="3.5"/><path d="M12 4.5v15"/>"#
            }
            Self::Terminal => {
                r#"<rect x="2.5" y="4.5" width="19" height="15" rx="3.5" fill="currentColor" fill-opacity="0.1"/><path d="m6.75 9.25 3 2.75-3 2.75"/><path d="M12.25 14.75H17"/>"#
            }
            Self::Upload => {
                r#"<path d="M12 14.75V3.5"/><path d="m7.75 7.75 4.25-4.25 4.25 4.25"/><path d="M4 16v2.5A2.5 2.5 0 0 0 6.5 21h11a2.5 2.5 0 0 0 2.5-2.5V16"/>"#
            }
            Self::View => {
                r#"<path d="M2.5 12S5.75 6 12 6s9.5 6 9.5 6-3.25 6-9.5 6-9.5-6-9.5-6Z"/><circle cx="12" cy="12" r="2.75" fill="currentColor" fill-opacity="0.2"/>"#
            }
        }
    }
}

pub(crate) struct RemCmdAssets;

impl AssetSource for RemCmdAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == APP_ICON_PATH {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icons/remcmd-256.png"
            ))));
        }
        if path == WORDMARK_REM_PATH {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icons/remcmd-wordmark-rem.svg"
            ))));
        }
        if path == WORDMARK_COMMAND_PATH {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../../../assets/icons/remcmd-wordmark-command.svg"
            ))));
        }

        Ok(IconName::from_asset_path(path)
            .map(build_svg)
            .map(String::into_bytes)
            .map(Cow::Owned))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

pub(crate) fn icon(name: IconName, theme: Theme, tone: IconTone, size: f32) -> AnyElement {
    let color = match tone {
        IconTone::Default => theme.text_primary,
        IconTone::Accent => theme.accent,
        IconTone::Danger => theme.danger,
    };

    icon_with_color(name, color, size)
}

pub(crate) fn icon_with_color(name: IconName, color: Hsla, size: f32) -> AnyElement {
    svg()
        .path(name.asset_path())
        .size(px(size))
        .text_color(color)
        .into_any_element()
}

pub(crate) fn app_icon(size: f32) -> AnyElement {
    img(APP_ICON_PATH).size(px(size)).into_any_element()
}

pub(crate) fn wordmark(theme: Theme, width: f32, height: f32) -> AnyElement {
    div()
        .relative()
        .flex_none()
        .w(px(width))
        .h(px(height))
        .child(
            svg()
                .absolute()
                .inset_0()
                .path(WORDMARK_REM_PATH)
                .size_full()
                .text_color(theme.text_primary),
        )
        .child(
            svg()
                .absolute()
                .inset_0()
                .path(WORDMARK_COMMAND_PATH)
                .size_full()
                .text_color(theme.accent),
        )
        .into_any_element()
}

fn build_svg(name: IconName) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.333333" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
        name.body()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_use_native_size_thin_stroke_svg() {
        let svg = build_svg(IconName::Search);

        assert!(svg.contains(r#"width="24" height="24""#));
        assert!(svg.contains(r#"viewBox="0 0 24 24""#));
        assert!(svg.contains(r#"stroke-width="1.333333""#));
        assert!(svg.contains(r#"stroke="currentColor""#));
    }

    #[test]
    fn asset_source_serves_known_icons_only() {
        let assets = RemCmdAssets;

        assert!(assets.load(APP_ICON_PATH).unwrap().is_some());
        assert!(assets.load("icons/add.svg").unwrap().is_some());
        assert!(assets.load("icons/about.svg").unwrap().is_some());
        assert!(assets.load("icons/check.svg").unwrap().is_some());
        assert!(assets.load("icons/copy.svg").unwrap().is_some());
        assert!(assets.load("icons/edit.svg").unwrap().is_some());
        assert!(assets.load("icons/home.svg").unwrap().is_some());
        assert!(assets.load("icons/performance.svg").unwrap().is_some());
        assert!(assets.load("icons/picker.svg").unwrap().is_some());
        assert!(assets.load("icons/view.svg").unwrap().is_some());
        assert!(assets.load(WORDMARK_REM_PATH).unwrap().is_some());
        assert!(assets.load(WORDMARK_COMMAND_PATH).unwrap().is_some());
        assert!(assets.load("icons/unknown.svg").unwrap().is_none());
    }

    #[test]
    fn wordmark_uses_vector_paths_without_font_dependencies() {
        let rem = std::str::from_utf8(include_bytes!(
            "../../../assets/icons/remcmd-wordmark-rem.svg"
        ))
        .expect("Rem wordmark SVG is UTF-8");
        let command = std::str::from_utf8(include_bytes!(
            "../../../assets/icons/remcmd-wordmark-command.svg"
        ))
        .expect("Cmd wordmark SVG is UTF-8");

        assert!(rem.contains(r#"viewBox="0 0 128 28""#));
        assert!(command.contains(r#"stroke-width="3.5""#));
        assert!(!rem.contains("<text"));
        assert!(!command.contains("<text"));
    }
}
