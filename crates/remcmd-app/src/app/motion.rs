//! Interruptible motion for chrome that changes while it is still moving.
//!
//! GPUI's keyed animations restart from their first frame when the target changes, which
//! shows as a jump when tabs are switched, opened, or closed in quick succession. A spring
//! keeps its position and velocity when retargeted, so every change continues from where
//! the last one left off.

use super::workspace::TabId;
use gpui::{Hsla, Pixels, SharedString, px};
use std::time::Instant;

/// Angular frequency for chrome springs; settles in roughly a third of a second.
const SPRING_FREQUENCY: f32 = 20.0;
const SPRING_REST_DISTANCE: f32 = 0.001;
const SPRING_REST_VELOCITY: f32 = 0.01;

/// A critically damped spring: the fastest approach to a target that never overshoots.
#[derive(Clone, Copy, Debug)]
pub(super) struct Spring {
    target: f32,
    /// Displacement from the target when the current motion began.
    offset: f32,
    velocity: f32,
    started: Instant,
    resting: bool,
}

impl Spring {
    pub(super) fn settled(value: f32) -> Self {
        Self {
            target: value,
            offset: 0.0,
            velocity: 0.0,
            started: Instant::now(),
            resting: true,
        }
    }

    pub(super) fn target(&self) -> f32 {
        self.target
    }

    fn state(&self, now: Instant) -> (f32, f32) {
        if self.resting {
            return (self.target, 0.0);
        }
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        let decay = (-SPRING_FREQUENCY * elapsed).exp();
        let slope = self.velocity + SPRING_FREQUENCY * self.offset;
        (
            self.target + (self.offset + slope * elapsed) * decay,
            (self.velocity - SPRING_FREQUENCY * slope * elapsed) * decay,
        )
    }

    pub(super) fn value(&self, now: Instant) -> f32 {
        self.state(now).0
    }

    /// Heads for `target` from the current position without losing momentum.
    pub(super) fn retarget(&mut self, target: f32, now: Instant) {
        if target == self.target {
            return;
        }
        let (value, velocity) = self.state(now);
        *self = Self {
            target,
            offset: value - target,
            velocity,
            started: now,
            resting: false,
        };
    }

    pub(super) fn snap(&mut self, target: f32) {
        *self = Self::settled(target);
    }

    /// Advances the rest check; returns whether the spring is still moving.
    pub(super) fn tick(&mut self, now: Instant) -> bool {
        if self.resting {
            return false;
        }
        let (value, velocity) = self.state(now);
        if (value - self.target).abs() < SPRING_REST_DISTANCE
            && velocity.abs() < SPRING_REST_VELOCITY
        {
            self.resting = true;
        }
        !self.resting
    }
}

/// What the strip needs to know about a live tab each frame.
pub(super) struct TabStripInput {
    pub(super) id: TabId,
    pub(super) label: SharedString,
    pub(super) status_color: Hsla,
    pub(super) active: bool,
}

/// One capsule in the strip. A closed tab stays as a non-interactive item until it has
/// finished collapsing, so its neighbours can glide into the space.
pub(super) struct TabStripItem {
    pub(super) id: TabId,
    pub(super) label: SharedString,
    pub(super) status_color: Hsla,
    pub(super) closing: bool,
    presence: Spring,
    active: Spring,
}

impl TabStripItem {
    /// 0 while absent, 1 once fully open.
    pub(super) fn presence(&self, now: Instant) -> f32 {
        self.presence.value(now).clamp(0.0, 1.0)
    }

    /// 0 for a background tab, 1 for the selected one.
    pub(super) fn activeness(&self, now: Instant) -> f32 {
        self.active.value(now).clamp(0.0, 1.0)
    }
}

/// Which part of the strip an in-flight scroll is bringing into view.
#[derive(Clone, Copy, PartialEq)]
enum Reveal {
    End,
    Tab(TabId),
}

/// Motion state for the titlebar tab strip.
#[derive(Default)]
pub(super) struct TabStripMotion {
    items: Vec<TabStripItem>,
    /// The scroll spring starts from the strip's offset on the first frame it is driven.
    reveal: Option<(Reveal, Option<Spring>)>,
    active: Option<TabId>,
    tabs_animating: bool,
}

impl TabStripMotion {
    pub(super) fn items(&self) -> &[TabStripItem] {
        &self.items
    }

    /// Whether another frame is needed to finish the current motion.
    pub(super) fn is_animating(&self) -> bool {
        self.tabs_animating || self.reveal.is_some()
    }

    /// Scrolls the strip to its trailing edge as the newest tab opens.
    pub(super) fn reveal_end(&mut self, scroll_x: Pixels) {
        self.reveal = Some((Reveal::End, Some(Spring::settled(f32::from(scroll_x)))));
    }

    /// Reconciles the strip with the live tabs and advances every spring.
    pub(super) fn sync(&mut self, tabs: Vec<TabStripInput>, now: Instant, reduce_motion: bool) {
        let previous = std::mem::take(&mut self.items);
        let mut remaining = previous.into_iter();
        let mut known = remaining
            .as_slice()
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        let active = tabs.iter().find(|tab| tab.active).map(|tab| tab.id);
        let live = tabs.iter().map(|tab| tab.id).collect::<Vec<_>>();

        for tab in tabs {
            if known.contains(&tab.id) {
                // Closed tabs ahead of this one keep their place while they collapse.
                for mut item in remaining.by_ref() {
                    known.retain(|id| *id != item.id);
                    let found = item.id == tab.id;
                    if found {
                        item.label = tab.label.clone();
                        item.status_color = tab.status_color;
                    }
                    self.items.push(item);
                    if found {
                        break;
                    }
                }
            } else {
                let mut presence = Spring::settled(0.0);
                presence.retarget(1.0, now);
                self.items.push(TabStripItem {
                    id: tab.id,
                    label: tab.label,
                    status_color: tab.status_color,
                    closing: false,
                    presence,
                    active: Spring::settled(0.0),
                });
            }
        }
        self.items.extend(remaining);

        let mut animating = false;
        for item in &mut self.items {
            if !live.contains(&item.id) {
                item.closing = true;
                item.presence.retarget(0.0, now);
            } else {
                // The selection of a closing tab fades with it rather than snapping off.
                item.active
                    .retarget(if active == Some(item.id) { 1.0 } else { 0.0 }, now);
            }
            if reduce_motion {
                item.presence.snap(item.presence.target());
                item.active.snap(item.active.target());
            }
            animating |= item.presence.tick(now);
            animating |= item.active.tick(now);
        }
        self.items
            .retain(|item| !(item.closing && item.presence.resting));

        if active != self.active {
            self.active = active;
            // A newly selected tab may sit outside the visible part of a crowded strip.
            if let Some(active) = active
                && !matches!(self.reveal, Some((Reveal::End, _)))
            {
                let scroll = self.reveal.and_then(|(_, scroll)| scroll);
                self.reveal = Some((Reveal::Tab(active), scroll));
            }
        }
        self.tabs_animating = animating;
    }

    /// Steers the strip's scroll offset toward whatever is being revealed.
    ///
    /// `geometry` reports last frame's layout: the current offset, the maximum offset, and
    /// how far the item at an index overflows the viewport (negative to the left, positive
    /// to the right, zero when fully visible).
    pub(super) fn scroll_offset(
        &mut self,
        now: Instant,
        reduce_motion: bool,
        current: Pixels,
        max: Pixels,
        overflow_for_item: impl Fn(usize) -> Option<Pixels>,
    ) -> Option<Pixels> {
        let (reveal, scroll) = self.reveal?;
        let current = f32::from(current);
        let mut scroll = scroll.unwrap_or_else(|| Spring::settled(current));
        let limit = -f32::from(max);
        let target = match reveal {
            Reveal::End => limit,
            Reveal::Tab(tab_id) => {
                let overflow = self
                    .items
                    .iter()
                    .position(|item| item.id == tab_id)
                    .and_then(&overflow_for_item)
                    .map_or(0.0, f32::from);
                (current - overflow).clamp(limit.min(0.0), 0.0)
            }
        };
        scroll.retarget(target, now);
        if reduce_motion {
            scroll.snap(target);
        }
        let moving = scroll.tick(now);
        let tabs_moving = self
            .items
            .iter()
            .any(|item| !item.presence.resting || !item.active.resting);
        let value = scroll.value(now);
        // Keep following until the tabs stop resizing; their widths move the target.
        self.reveal = (moving || tabs_moving).then_some((reveal, Some(scroll)));
        Some(px(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn input(id: u64, active: bool) -> TabStripInput {
        TabStripInput {
            id: TabId(id),
            label: SharedString::from(format!("tab {id}")),
            status_color: Hsla::default(),
            active,
        }
    }

    fn settle(strip: &mut TabStripMotion, tabs: &[(u64, bool)], mut now: Instant) -> Instant {
        for _ in 0..240 {
            now += Duration::from_millis(16);
            strip.sync(
                tabs.iter()
                    .map(|(id, active)| input(*id, *active))
                    .collect(),
                now,
                false,
            );
            strip.scroll_offset(now, false, px(0.0), px(0.0), |_| Some(px(0.0)));
            if !strip.is_animating() {
                break;
            }
        }
        now
    }

    #[test]
    fn spring_reaches_its_target_without_overshoot() {
        let start = Instant::now();
        let mut spring = Spring::settled(0.0);
        spring.retarget(1.0, start);

        let mut previous = 0.0;
        for frame in 1..=60 {
            let now = start + Duration::from_millis(frame * 16);
            let value = spring.value(now);
            assert!(
                value >= previous && value <= 1.0,
                "{value} after {previous}"
            );
            previous = value;
            spring.tick(now);
        }
        assert_eq!(spring.value(start + Duration::from_secs(1)), 1.0);
    }

    #[test]
    fn retargeting_continues_from_the_current_position() {
        let start = Instant::now();
        let mut spring = Spring::settled(0.0);
        spring.retarget(1.0, start);
        let interrupted = start + Duration::from_millis(80);
        let before = spring.value(interrupted);

        spring.retarget(0.0, interrupted);

        assert!((spring.value(interrupted) - before).abs() < 1e-5);
        assert!(before > 0.2 && before < 1.0);
    }

    #[test]
    fn closed_tabs_collapse_in_place_before_leaving_the_strip() {
        let mut strip = TabStripMotion::default();
        let now = settle(
            &mut strip,
            &[(1, false), (2, true), (3, false)],
            Instant::now(),
        );
        assert_eq!(strip.items().len(), 3);

        let closed_at = now + Duration::from_millis(16);
        strip.sync(vec![input(1, false), input(3, true)], closed_at, false);

        let ids = strip
            .items()
            .iter()
            .map(|item| (item.id.0, item.closing))
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![(1, false), (2, true), (3, false)]);
        assert!(strip.is_animating());

        settle(&mut strip, &[(1, false), (3, true)], closed_at);
        let ids = strip
            .items()
            .iter()
            .map(|item| item.id.0)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn new_tabs_open_from_nothing_and_reduced_motion_skips_the_animation() {
        let now = Instant::now();
        let mut strip = TabStripMotion::default();
        strip.sync(vec![input(1, true)], now, false);
        assert_eq!(strip.items()[0].presence(now), 0.0);
        assert!(strip.is_animating());

        let mut reduced = TabStripMotion::default();
        reduced.sync(vec![input(1, true)], now, true);
        assert_eq!(reduced.items()[0].presence(now), 1.0);
        assert_eq!(reduced.items()[0].activeness(now), 1.0);
        // Selecting a tab asks the strip to reveal it; that resolves on the same frame.
        let offset = reduced.scroll_offset(now, true, px(-40.0), px(120.0), |_| Some(px(-25.0)));
        assert_eq!(offset, Some(px(-15.0)));
        assert!(!reduced.is_animating());
    }
}
