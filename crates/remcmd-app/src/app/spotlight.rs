//! Spotlight-style search over open terminals, connections, and common actions.

use gpui::{
    Animation, AnimationExt, BoxShadow, Context, Entity, FocusHandle, Focusable, FontWeight,
    KeyDownEvent, MouseButton, ScrollHandle, SharedString, Window, div, ease_out_quint, point,
    prelude::*, px,
};

use super::{
    CloseSpotlight, ConfirmSpotlight, IconName, LOCAL_PROFILE_ID, RemCmdApp, RevealSpotlight,
    TabId, TextField, TextFieldEvent, icon_with_color,
};

const SPOTLIGHT_WIDTH: f32 = 640.0;
const SPOTLIGHT_ROW_HEIGHT: f32 = 38.0;
const SPOTLIGHT_SECTION_HEIGHT: f32 = 26.0;
const SPOTLIGHT_MAX_LIST_HEIGHT: f32 = 380.0;
const SPOTLIGHT_HEADER_HEIGHT: f32 = 56.0;
/// Header, divider, the full result list, and the list's bottom padding.
const SPOTLIGHT_MAX_HEIGHT: f32 = SPOTLIGHT_HEADER_HEIGHT + 1.0 + SPOTLIGHT_MAX_LIST_HEIGHT + 8.0;

pub(super) struct Spotlight {
    input: Entity<TextField>,
    query: String,
    cursor: usize,
    scroll_handle: ScrollHandle,
    /// The focus to restore when the panel closes without opening anything.
    previous_focus: Option<FocusHandle>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SpotlightSection {
    Terminals,
    Servers,
    Actions,
}

impl SpotlightSection {
    const fn label_key(self) -> &'static str {
        match self {
            Self::Terminals => "spotlight-section-terminals",
            Self::Servers => "spotlight-section-servers",
            Self::Actions => "spotlight-section-actions",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum SpotlightTarget {
    Tab(TabId),
    Profile(String),
    LocalTerminal,
    NewConnection,
    Home,
    Settings,
}

struct SpotlightResult {
    section: SpotlightSection,
    target: SpotlightTarget,
    icon: IconName,
    title: SharedString,
    subtitle: Option<SharedString>,
}

/// Ranks how well `query` matches any of `fields`: prefix, word prefix, then substring.
fn match_rank(query: &str, fields: &[&str]) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    fields
        .iter()
        .filter_map(|field| {
            let field = field.to_lowercase();
            if field.starts_with(query) {
                Some(0)
            } else if field
                .split(|c: char| !c.is_alphanumeric())
                .any(|word| word.starts_with(query))
            {
                Some(1)
            } else if field.contains(query) {
                Some(2)
            } else {
                None
            }
        })
        .min()
}

impl RemCmdApp {
    pub(super) fn toggle_spotlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spotlight.is_some() {
            self.close_spotlight(window, cx);
            return;
        }
        let input = cx.new(|cx| TextField::new_spotlight(cx, self.tr("spotlight-placeholder")));
        cx.subscribe(&input, |this, input, _: &TextFieldEvent, cx| {
            let query = input.read(cx).text();
            if let Some(spotlight) = this.spotlight.as_mut()
                && spotlight.query != query
            {
                spotlight.query = query;
                spotlight.cursor = 0;
                spotlight.scroll_handle.set_offset(point(px(0.0), px(0.0)));
                cx.notify();
            }
        })
        .detach();
        let previous_focus = window.focused(cx);
        input.read(cx).focus_handle(cx).focus(window);
        self.context_menu = None;
        self.open_settings_selector = None;
        self.spotlight = Some(Spotlight {
            input,
            query: String::new(),
            cursor: 0,
            scroll_handle: ScrollHandle::new(),
            previous_focus,
        });
        cx.notify();
    }

    pub(super) fn close_spotlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(spotlight) = self.spotlight.take() {
            if let Some(focus) = spotlight.previous_focus {
                focus.focus(window);
            }
            cx.notify();
        }
    }

    fn spotlight_results(&self) -> Vec<SpotlightResult> {
        let Some(spotlight) = self.spotlight.as_ref() else {
            return Vec::new();
        };
        let query = spotlight.query.trim().to_lowercase();
        let mut results = Vec::new();

        for tab in &self.tabs {
            let title = self.terminal_tab_title(tab);
            let profile = self
                .profiles
                .iter()
                .find(|profile| profile.id == tab.profile_id);
            let host = profile.map(|profile| profile.host.as_str()).unwrap_or("");
            let Some(rank) = match_rank(&query, &[&title, host]) else {
                continue;
            };
            let subtitle = if tab.profile_id == LOCAL_PROFILE_ID {
                None
            } else {
                profile.map(|profile| SharedString::from(profile.address()))
            };
            results.push((
                rank,
                SpotlightResult {
                    section: SpotlightSection::Terminals,
                    target: SpotlightTarget::Tab(tab.id),
                    icon: IconName::Terminal,
                    title: title.into(),
                    subtitle,
                },
            ));
        }

        for profile in &self.profiles {
            let address = profile.address();
            let Some(rank) = match_rank(
                &query,
                &[&profile.name, &profile.host, &profile.username, &address],
            ) else {
                continue;
            };
            results.push((
                rank,
                SpotlightResult {
                    section: SpotlightSection::Servers,
                    target: SpotlightTarget::Profile(profile.id.clone()),
                    icon: IconName::Server,
                    title: profile.name.clone().into(),
                    subtitle: Some(address.into()),
                },
            ));
        }

        for (target, icon, key) in [
            (
                SpotlightTarget::LocalTerminal,
                IconName::Terminal,
                "menu-new-local-terminal",
            ),
            (
                SpotlightTarget::NewConnection,
                IconName::NewConnection,
                "sidebar-new-connection",
            ),
            (SpotlightTarget::Home, IconName::Home, "sidebar-home"),
            (
                SpotlightTarget::Settings,
                IconName::Settings,
                "sidebar-settings",
            ),
        ] {
            let title = self.tr(key);
            let Some(rank) = match_rank(&query, &[&title]) else {
                continue;
            };
            results.push((
                rank,
                SpotlightResult {
                    section: SpotlightSection::Actions,
                    target,
                    icon,
                    title: title.into(),
                    subtitle: None,
                },
            ));
        }

        // Group by section, then put the closest matches first within each group.
        results.sort_by_key(|(rank, result)| (result.section, *rank));
        results.into_iter().map(|(_, result)| result).collect()
    }

    fn navigate_spotlight(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.modified() || !matches!(key, "up" | "down") {
            return;
        }
        cx.stop_propagation();
        let results = self.spotlight_results();
        let Some(spotlight) = self.spotlight.as_mut() else {
            return;
        };
        if results.is_empty() {
            return;
        }
        spotlight.cursor = super::keyboard::menu_index(spotlight.cursor, results.len(), key);
        // Section headings share the list with rows, so scroll to the row's child index.
        let headings = results[..=spotlight.cursor]
            .iter()
            .enumerate()
            .filter(|(index, result)| *index == 0 || results[index - 1].section != result.section)
            .count();
        spotlight
            .scroll_handle
            .scroll_to_item(spotlight.cursor + headings);
        cx.notify();
    }

    fn confirm_spotlight(&mut self, reveal: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cursor) = self.spotlight.as_ref().map(|spotlight| spotlight.cursor) else {
            return;
        };
        let Some(target) = self
            .spotlight_results()
            .into_iter()
            .nth(cursor)
            .map(|result| result.target)
        else {
            return;
        };
        self.open_spotlight_target(target, reveal, window, cx);
    }

    fn open_spotlight_target(
        &mut self,
        target: SpotlightTarget,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Opening a result moves focus itself, so the previous focus is not restored.
        self.spotlight = None;
        match target {
            SpotlightTarget::Tab(tab_id) => {
                self.activate_tab_in_window(tab_id, window, cx);
            }
            SpotlightTarget::Profile(profile_id) => {
                self.select_profile(profile_id, window, cx);
                if !reveal {
                    self.connect_selected_profile_in_new_session(window, cx);
                }
            }
            SpotlightTarget::LocalTerminal => self.open_local_terminal(window, cx),
            SpotlightTarget::NewConnection => self.open_new_profile_editor(cx),
            SpotlightTarget::Home => self.show_home(window, cx),
            SpotlightTarget::Settings => self.show_settings(window, cx),
        }
        cx.notify();
    }

    /// Returns the dismiss layer and panel for an open Spotlight search.
    pub(super) fn render_spotlight_layer(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let spotlight = self.spotlight.as_ref()?;
        let results = self.spotlight_results();
        let has_query = !spotlight.query.trim().is_empty();
        let viewport = window.viewport_size();
        let width = SPOTLIGHT_WIDTH
            .min(f32::from(viewport.width) - 48.0)
            .max(240.0);
        // Center the panel at its full height so the field stays put as results narrow.
        let top = ((f32::from(viewport.height) - SPOTLIGHT_MAX_HEIGHT) / 2.0)
            .max(super::content_top_inset() + 12.0);
        let cursor = spotlight.cursor.min(results.len().saturating_sub(1));

        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(SPOTLIGHT_HEADER_HEIGHT))
            .px_4()
            .child(icon_with_color(
                IconName::Search,
                self.theme.text_muted,
                22.0,
            ))
            .child(div().flex_1().min_w(px(0.0)).child(spotlight.input.clone()));

        let mut panel = div()
            .id("spotlight_panel")
            .key_context("Spotlight")
            .on_action(cx.listener(|this, _: &CloseSpotlight, window, cx| {
                this.close_spotlight(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ConfirmSpotlight, window, cx| {
                this.confirm_spotlight(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &RevealSpotlight, window, cx| {
                this.confirm_spotlight(true, window, cx);
            }))
            .on_key_down(cx.listener(Self::navigate_spotlight))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .absolute()
            .top(px(top))
            .left(px((f32::from(viewport.width) - width) / 2.0))
            .w(px(width))
            .flex()
            .flex_col()
            .rounded(px(18.0))
            .border_1()
            .border_color(self.theme.border_strong)
            // GPUI cannot blur what lies behind the panel, so keep it opaque enough to read.
            .bg(self.theme.floating_glass_bg.alpha(1.0))
            .shadow(vec![
                BoxShadow {
                    color: self.theme.shadow,
                    offset: point(px(0.0), px(18.0)),
                    blur_radius: px(48.0),
                    spread_radius: px(-8.0),
                },
                BoxShadow {
                    color: self.theme.shadow,
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(3.0),
                    spread_radius: px(0.0),
                },
            ])
            .overflow_hidden()
            .child(header);

        if has_query || !results.is_empty() {
            let mut list = div()
                .id("spotlight_results")
                .flex()
                .flex_col()
                .max_h(px(SPOTLIGHT_MAX_LIST_HEIGHT))
                .overflow_y_scroll()
                .track_scroll(&spotlight.scroll_handle)
                .px_2()
                .pb_2();
            if results.is_empty() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(SPOTLIGHT_ROW_HEIGHT * 2.0))
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(self.tr("spotlight-no-results")),
                );
            }
            for (index, result) in results.iter().enumerate() {
                if index == 0 || results[index - 1].section != result.section {
                    list = list.child(
                        div()
                            .flex()
                            .flex_none()
                            .items_end()
                            .h(px(SPOTLIGHT_SECTION_HEIGHT))
                            .px_2()
                            .pb(px(4.0))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_muted)
                            .child(self.tr(result.section.label_key())),
                    );
                }
                list = list.child(self.render_spotlight_row(index, result, index == cursor, cx));
            }
            panel = panel
                .child(
                    div()
                        .flex_none()
                        .h(px(1.0))
                        .mx_4()
                        .bg(self.theme.settings_separator),
                )
                .child(list);
        }

        let panel = panel.with_animation(
            "spotlight-open",
            Animation::new(
                self.theme
                    .motion_duration(std::time::Duration::from_millis(160)),
            )
            .with_easing(ease_out_quint()),
            move |panel, progress| {
                panel
                    .opacity(progress)
                    .top(px(top - 8.0 * (1.0 - progress)))
            },
        );

        Some(
            div()
                .id("spotlight_dismiss_layer")
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0()
                .bg(self.theme.transparent)
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_spotlight(window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.close_spotlight(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_spotlight_row(
        &self,
        index: usize,
        result: &SpotlightResult,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (title_color, detail_color) = if selected {
            (self.theme.on_accent, self.theme.on_accent.opacity(0.75))
        } else {
            (self.theme.text_primary, self.theme.text_muted)
        };
        let hint = match result.target {
            SpotlightTarget::Profile(_) => Some(self.tr("common-connect")),
            _ => None,
        };
        let target = result.target.clone();
        div()
            .id(SharedString::from(format!("spotlight-result-{index}")))
            .relative()
            .child(crate::accessibility::node(
                "spotlight-result",
                crate::accessibility::Node {
                    selected,
                    menu: true,
                    ..crate::accessibility::Node::button(result.title.clone(), true)
                },
            ))
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(SPOTLIGHT_ROW_HEIGHT))
            .px_2()
            .rounded(px(8.0))
            .when(selected, |this| this.bg(self.theme.accent_fill))
            .cursor_pointer()
            .child(icon_with_color(result.icon, title_color, 18.0))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(280.0))
                            .truncate()
                            .text_sm()
                            .text_color(title_color)
                            .child(result.title.clone()),
                    )
                    .when_some(result.subtitle.clone(), |this, subtitle| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .truncate()
                                .text_xs()
                                .text_color(detail_color)
                                .child(subtitle),
                        )
                    }),
            )
            .when(selected, |this| {
                this.when_some(hint, |this, hint| {
                    this.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(detail_color)
                            .child(format!("{hint} ↩")),
                    )
                })
            })
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if let Some(spotlight) = this.spotlight.as_mut()
                    && spotlight.cursor != index
                {
                    spotlight.cursor = index;
                    cx.notify();
                }
            }))
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    let reveal = event.modifiers().secondary();
                    this.open_spotlight_target(target.clone(), reveal, window, cx);
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::match_rank;

    #[test]
    fn match_rank_prefers_prefixes_over_substrings() {
        assert_eq!(match_rank("", &["anything"]), Some(0));
        assert_eq!(match_rank("pro", &["Production"]), Some(0));
        assert_eq!(match_rank("db", &["main-db.example.com"]), Some(1));
        assert_eq!(match_rank("ain", &["main"]), Some(2));
        assert_eq!(match_rank("zz", &["main", "host"]), None);
        assert_eq!(match_rank("ho", &["main", "host"]), Some(0));
    }
}
