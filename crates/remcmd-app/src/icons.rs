use std::borrow::Cow;

use gpui::{AnyElement, AssetSource, Hsla, IntoElement, Result, SharedString, prelude::*, px, svg};

use crate::theme::{IconTone, Theme};

// RemCmd-owned icon paths use a shared 24px view box with rounded caps and joins.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IconName {
    Add,
    ArrowLeft,
    ArrowUp,
    Cancel,
    ClosePane,
    Collapse,
    Connect,
    Delete,
    Disconnect,
    Expand,
    File,
    Folder,
    ForgetCredential,
    NewConnection,
    Reconnect,
    Search,
    Server,
    Settings,
    SidebarLeft,
    SidebarRight,
    SplitDown,
    SplitRight,
    Terminal,
}

impl IconName {
    const fn asset_path(self) -> &'static str {
        match self {
            Self::Add => "icons/add.svg",
            Self::ArrowLeft => "icons/arrow-left.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::Cancel => "icons/cancel.svg",
            Self::ClosePane => "icons/close-pane.svg",
            Self::Collapse => "icons/collapse.svg",
            Self::Connect => "icons/connect.svg",
            Self::Delete => "icons/delete.svg",
            Self::Disconnect => "icons/disconnect.svg",
            Self::Expand => "icons/expand.svg",
            Self::File => "icons/file.svg",
            Self::Folder => "icons/folder.svg",
            Self::ForgetCredential => "icons/forget-credential.svg",
            Self::NewConnection => "icons/new-connection.svg",
            Self::Reconnect => "icons/reconnect.svg",
            Self::Search => "icons/search.svg",
            Self::Server => "icons/server.svg",
            Self::Settings => "icons/settings.svg",
            Self::SidebarLeft => "icons/sidebar-left.svg",
            Self::SidebarRight => "icons/sidebar-right.svg",
            Self::SplitDown => "icons/split-down.svg",
            Self::SplitRight => "icons/split-right.svg",
            Self::Terminal => "icons/terminal.svg",
        }
    }

    fn from_asset_path(path: &str) -> Option<Self> {
        Some(match path {
            "icons/add.svg" => Self::Add,
            "icons/arrow-left.svg" => Self::ArrowLeft,
            "icons/arrow-up.svg" => Self::ArrowUp,
            "icons/cancel.svg" => Self::Cancel,
            "icons/close-pane.svg" => Self::ClosePane,
            "icons/collapse.svg" => Self::Collapse,
            "icons/connect.svg" => Self::Connect,
            "icons/delete.svg" => Self::Delete,
            "icons/disconnect.svg" => Self::Disconnect,
            "icons/expand.svg" => Self::Expand,
            "icons/file.svg" => Self::File,
            "icons/folder.svg" => Self::Folder,
            "icons/forget-credential.svg" => Self::ForgetCredential,
            "icons/new-connection.svg" => Self::NewConnection,
            "icons/reconnect.svg" => Self::Reconnect,
            "icons/search.svg" => Self::Search,
            "icons/server.svg" => Self::Server,
            "icons/settings.svg" => Self::Settings,
            "icons/sidebar-left.svg" => Self::SidebarLeft,
            "icons/sidebar-right.svg" => Self::SidebarRight,
            "icons/split-down.svg" => Self::SplitDown,
            "icons/split-right.svg" => Self::SplitRight,
            "icons/terminal.svg" => Self::Terminal,
            _ => return None,
        })
    }

    const fn body(self) -> &'static str {
        match self {
            Self::Add => {
                r#"<path d="M4 12H20" stroke-width="1.4"/><path d="M12 4V20" stroke-width="1.4"/>"#
            }
            Self::ArrowLeft => r#"<path d="M15.25 3.5 7.75 12l7.5 8.5"/>"#,
            Self::ArrowUp => r#"<path d="m3.5 15.25 8.5-7.5 8.5 7.5"/>"#,
            Self::Cancel => r#"<path d="m5.5 5.5 13 13"/><path d="m18.5 5.5-13 13"/>"#,
            Self::ClosePane => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3"/><path d="M15 4v16"/><path d="m17.2 9.7 2.6 2.6m0-2.6-2.6 2.6" stroke-width="0.9"/>"#
            }
            Self::Collapse => r#"<path d="m6.5 9.25 5.5 5.5 5.5-5.5"/>"#,
            Self::Connect => {
                r#"<path d="M3 12h13.5"/><path d="m12.5 8 4 4-4 4"/><rect x="16.5" y="4" width="4.5" height="16" rx="2.25"/>"#
            }
            Self::Delete => {
                r#"<path d="M3.5 7h17"/>
                <path d="m5.75 7 .5 11.5c.05 1.15 1 2 2.15 2h7.2c1.15 0 2.1-.85 2.15-2l.5-11.5"/>
                <path d="M9.25 7V5.5c0-1.105.895-2 2-2h1.5c1.105 0 2 .895 2 2V7"/>
                <path d="M9 9.75v8.5M12 9.75v8.5M15 9.75v8.5" stroke-width="0.8"/>"#
            }
            Self::Disconnect => {
                r#"<rect x="3" y="4" width="4.5" height="16" rx="2.25"/><path d="M7.5 12H21"/><path d="m17 8 4 4-4 4"/>"#
            }
            Self::Expand => r#"<path d="m9.25 6.5 5.5 5.5-5.5 5.5"/>"#,
            Self::File => {
                r#"<path d="M6.5 2.5H14L19.5 8v12a1.5 1.5 0 0 1-1.5 1.5H6.5a2 2 0 0 1-2-2v-15a2 2 0 0 1 2-2Z"/><path d="M14 2.5V8h5.5"/><path d="M8 12h8m-8 3h8m-8 3h5.5" stroke-width="0.7"/>"#
            }
            Self::Folder => {
                r#"<path d="M3 6.5h5.75L11 8.75h9.5a1.5 1.5 0 0 1 1.5 1.5V19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8.5a2 2 0 0 1 2-2Z"/><path d="M2 11.25h20" stroke-width="0.7"/>"#
            }
            Self::ForgetCredential => {
                r#"<circle cx="7.5" cy="14" r="4"/><path d="M11.5 14h9m-3 0v3m-3-3v2"/><circle cx="18" cy="5.5" r="3"/><path d="M16.5 5.5h3" stroke-width="1.1"/>"#
            }
            Self::NewConnection => {
                r#"<path d="M14.75 5H5.5A2.5 2.5 0 0 0 3 7.5v10A2.5 2.5 0 0 0 5.5 20H16a2.5 2.5 0 0 0 2.5-2.5V9"/><path d="M3 12.5h15.5"/><circle cx="6.5" cy="8.75" r="0.55" fill="currentColor" stroke="none"/><circle cx="6.5" cy="16.25" r="0.55" fill="currentColor" stroke="none"/><circle cx="18.5" cy="5.5" r="3.25"/><path d="M18.5 3.75v3.5M16.75 5.5h3.5" stroke-width="1.1"/>"#
            }
            Self::Reconnect => {
                r#"<path d="M19.5 8.25A8.25 8.25 0 1 0 19.75 15.25"/><path d="M19.5 3.75v4.5H15"/>"#
            }
            Self::Search => {
                r#"<circle cx="10.25" cy="10.25" r="7.25"/><path d="m15.65 15.65 5.35 5.35"/>"#
            }
            Self::Server => {
                r#"<rect x="2.5" y="3.5" width="19" height="7" rx="2.25"/><rect x="2.5" y="13.5" width="19" height="7" rx="2.25"/><circle cx="6" cy="7" r="0.65" fill="currentColor" stroke="none"/><circle cx="6" cy="17" r="0.65" fill="currentColor" stroke="none"/><path d="M9.5 7h9m-9 10h9" stroke-width="0.7"/>"#
            }
            Self::Settings => {
                r#"<path d="M8.977 4.701 10.089 4.335 10.264 2.152h3.472l.175 2.183 1.112.366 1.046.527 1.667-1.42 2.456 2.456-1.42 1.667.527 1.046.366 1.112 2.183.175v3.472l-2.183.175-.366 1.112-.527 1.046 1.42 1.667-2.456 2.456-1.667-1.42-1.046.527-1.112.366-.175 2.183h-3.472l-.175-2.183-1.112-.366-1.046-.527-1.667 1.42-2.456-2.456 1.42-1.667-.527-1.046-.366-1.112-2.183-.175v-3.472l2.183-.175.366-1.112.527-1.046-1.42-1.667 2.456-2.456 1.667 1.42Z"/><circle cx="12" cy="12" r="3.25"/>"#
            }
            Self::SidebarLeft => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3" stroke-width="1.333" stroke-linejoin="round"/>
                <path d="M9.36842 4V20" stroke-width="1.333"/>
                <path d="M4.10526 7.73333H7.26316" stroke-width="0.7" stroke-linecap="round"/>
                <path d="M4.10526 10.4H7.26316" stroke-width="0.7" stroke-linecap="round"/>
                <path d="M4.10526 13.0667H7.26316" stroke-width="0.7" stroke-linecap="round"/>
"#
            }
            Self::SidebarRight => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3" stroke-width="1.333" stroke-linejoin="round"/>
                <path d="M14.63 4V20" stroke-width="1.333"/>
                <path d="M16.73 7.73333H19.8879" stroke-width="0.7" stroke-linecap="round"/>
                <path d="M16.73 10.4H19.8879" stroke-width="0.7" stroke-linecap="round"/>
                <path d="M16.73 13.0667H19.8879" stroke-width="0.7" stroke-linecap="round"/>"#
            }
            Self::SplitDown => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3"/><path d="M2 12h20"/><path d="M5 8h4m-4 8h4" stroke-width="0.7"/>"#
            }
            Self::SplitRight => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3"/><path d="M12 4v16"/><path d="M5 8h3m7 0h3" stroke-width="0.7"/>"#
            }
            Self::Terminal => {
                r#"<rect x="2" y="4" width="20" height="16" rx="3"/><path d="m6.5 8.75 3.25 3.25-3.25 3.25"/><path d="M12.5 15.25h5"/>"#
            }
        }
    }
}

pub(crate) struct RemCmdAssets;

impl AssetSource for RemCmdAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
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

        assert!(assets.load("icons/add.svg").unwrap().is_some());
        assert!(assets.load("icons/unknown.svg").unwrap().is_none());
    }
}
