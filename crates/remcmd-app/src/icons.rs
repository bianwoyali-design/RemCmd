use std::borrow::Cow;

use gpui::{AnyElement, AssetSource, IntoElement, Result, SharedString, prelude::*, px, svg};

use crate::theme::{IconTone, Theme};

// Path data is from Lucide, rendered with its standard 24px view box and
// rounded caps and joins. The stroke is tuned slightly lighter for GPUI.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IconName {
    Add,
    Cancel,
    ClosePane,
    Collapse,
    Connect,
    Delete,
    Disconnect,
    Expand,
    Folder,
    ForgetCredential,
    NewConnection,
    Reconnect,
    Search,
    Server,
    Settings,
    SplitDown,
    SplitRight,
    Terminal,
}

impl IconName {
    const fn asset_path(self) -> &'static str {
        match self {
            Self::Add => "icons/add.svg",
            Self::Cancel => "icons/cancel.svg",
            Self::ClosePane => "icons/close-pane.svg",
            Self::Collapse => "icons/collapse.svg",
            Self::Connect => "icons/connect.svg",
            Self::Delete => "icons/delete.svg",
            Self::Disconnect => "icons/disconnect.svg",
            Self::Expand => "icons/expand.svg",
            Self::Folder => "icons/folder.svg",
            Self::ForgetCredential => "icons/forget-credential.svg",
            Self::NewConnection => "icons/new-connection.svg",
            Self::Reconnect => "icons/reconnect.svg",
            Self::Search => "icons/search.svg",
            Self::Server => "icons/server.svg",
            Self::Settings => "icons/settings.svg",
            Self::SplitDown => "icons/split-down.svg",
            Self::SplitRight => "icons/split-right.svg",
            Self::Terminal => "icons/terminal.svg",
        }
    }

    fn from_asset_path(path: &str) -> Option<Self> {
        Some(match path {
            "icons/add.svg" => Self::Add,
            "icons/cancel.svg" => Self::Cancel,
            "icons/close-pane.svg" => Self::ClosePane,
            "icons/collapse.svg" => Self::Collapse,
            "icons/connect.svg" => Self::Connect,
            "icons/delete.svg" => Self::Delete,
            "icons/disconnect.svg" => Self::Disconnect,
            "icons/expand.svg" => Self::Expand,
            "icons/folder.svg" => Self::Folder,
            "icons/forget-credential.svg" => Self::ForgetCredential,
            "icons/new-connection.svg" => Self::NewConnection,
            "icons/reconnect.svg" => Self::Reconnect,
            "icons/search.svg" => Self::Search,
            "icons/server.svg" => Self::Server,
            "icons/settings.svg" => Self::Settings,
            "icons/split-down.svg" => Self::SplitDown,
            "icons/split-right.svg" => Self::SplitRight,
            "icons/terminal.svg" => Self::Terminal,
            _ => return None,
        })
    }

    const fn body(self) -> &'static str {
        match self {
            Self::Add => r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#,
            Self::Cancel => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
            Self::ClosePane => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M15 3v18"/><path d="m8 9 3 3-3 3"/>"#
            }
            Self::Collapse => r#"<path d="m6 9 6 6 6-6"/>"#,
            Self::Connect => {
                r#"<path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z"/><path d="m2 22 3-3"/><path d="M7.5 13.5 10 11"/><path d="M10.5 16.5 13 14"/><path d="m18 3-4 4h6l-4 4"/>"#
            }
            Self::Delete => {
                r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/><line x1="10" x2="10" y1="11" y2="17"/><line x1="14" x2="14" y1="11" y2="17"/>"#
            }
            Self::Disconnect => {
                r#"<path d="m19 5 3-3"/><path d="m2 22 3-3"/><path d="M6.3 20.3a2.4 2.4 0 0 0 3.4 0L12 18l-6-6-2.3 2.3a2.4 2.4 0 0 0 0 3.4Z"/><path d="M7.5 13.5 10 11"/><path d="M10.5 16.5 13 14"/><path d="m12 6 6 6 2.3-2.3a2.4 2.4 0 0 0 0-3.4l-2.6-2.6a2.4 2.4 0 0 0-3.4 0Z"/>"#
            }
            Self::Expand => r#"<path d="m9 18 6-6-6-6"/>"#,
            Self::Folder => {
                r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
            }
            Self::ForgetCredential => {
                r#"<path d="M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z"/><circle cx="16.5" cy="7.5" r=".5" fill="currentColor"/>"#
            }
            Self::NewConnection => {
                r#"<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.375 2.625a1 1 0 0 1 3 3l-9.013 9.014a2 2 0 0 1-.853.505l-2.873.84a.5.5 0 0 1-.62-.62l.84-2.873a2 2 0 0 1 .506-.852z"/>"#
            }
            Self::Reconnect => {
                r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#
            }
            Self::Search => r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#,
            Self::Server => {
                r#"<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/>"#
            }
            Self::Settings => {
                r#"<path d="M20 7h-9"/><path d="M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>"#
            }
            Self::SplitDown => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 12h18"/>"#
            }
            Self::SplitRight => {
                r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M12 3v18"/>"#
            }
            Self::Terminal => {
                r#"<path d="m7 11 2-2-2-2"/><path d="M11 13h4"/><rect width="18" height="18" x="3" y="3" rx="2" ry="2"/>"#
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

    svg()
        .path(name.asset_path())
        .size(px(size))
        .text_color(color)
        .into_any_element()
}

fn build_svg(name: IconName) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" color="#000" stroke="#000" stroke-width="1.333333" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
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
        assert!(svg.contains(r##"stroke="#000""##));
    }

    #[test]
    fn asset_source_serves_known_icons_only() {
        let assets = RemCmdAssets;

        assert!(assets.load("icons/add.svg").unwrap().is_some());
        assert!(assets.load("icons/unknown.svg").unwrap().is_none());
    }
}
