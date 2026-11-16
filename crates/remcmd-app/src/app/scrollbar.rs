//! Overlay scroll indicator for custom-drawn scroll areas.
//!
//! GPUI scroll containers draw no scroller of their own. This one follows the macOS
//! overlay style: it appears while the pointer is over the scroll area, can be dragged,
//! and jumps when its track is clicked.

use gpui::{
    App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, ScrollHandle, Style, Window, div, fill, point, prelude::*, px, relative,
    size,
};
use std::{cell::RefCell, rc::Rc};

const TRACK_WIDTH: f32 = 12.0;
const THUMB_WIDTH: f32 = 6.0;
const THUMB_MIN_HEIGHT: f32 = 28.0;
const TRACK_INSET: f32 = 3.0;

/// A vertical scroller pinned to the right edge of its nearest positioned ancestor.
pub(super) fn overlay_scrollbar(
    id: impl Into<ElementId>,
    handle: ScrollHandle,
    color: Hsla,
) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .w(px(TRACK_WIDTH))
        .child(OverlayScrollbar {
            id: id.into(),
            handle,
            color,
        })
}

struct OverlayScrollbar {
    id: ElementId,
    handle: ScrollHandle,
    color: Hsla,
}

#[derive(Default)]
struct ScrollbarState {
    /// Distance from the thumb's top edge to the pointer while dragging.
    grab: Option<Pixels>,
    pointer_inside: bool,
}

#[derive(Clone, Copy)]
struct ThumbGeometry {
    track_top: Pixels,
    travel: Pixels,
    thumb: Bounds<Pixels>,
    max_offset: Pixels,
}

impl ThumbGeometry {
    fn new(track: Bounds<Pixels>, handle: &ScrollHandle) -> Option<Self> {
        let max_offset = handle.max_offset().height;
        let viewport_height = handle.bounds().size.height;
        let track_height = track.size.height - px(TRACK_INSET * 2.0);
        if max_offset <= px(0.5) || viewport_height <= px(0.0) || track_height <= px(0.0) {
            return None;
        }
        let thumb_height = thumb_height(
            f32::from(track_height),
            f32::from(viewport_height),
            f32::from(max_offset),
        );
        let travel = track_height - px(thumb_height);
        let progress = (-handle.offset().y / max_offset).clamp(0.0, 1.0);
        let track_top = track.top() + px(TRACK_INSET);
        Some(Self {
            track_top,
            travel,
            thumb: Bounds::new(
                point(
                    track.right() - px(THUMB_WIDTH + TRACK_INSET),
                    track_top + travel * progress,
                ),
                size(px(THUMB_WIDTH), px(thumb_height)),
            ),
            max_offset,
        })
    }

    fn scroll_to(&self, thumb_top: Pixels, handle: &ScrollHandle) {
        if self.travel <= px(0.0) {
            return;
        }
        let progress = ((thumb_top - self.track_top) / self.travel).clamp(0.0, 1.0);
        handle.set_offset(point(handle.offset().x, -self.max_offset * progress));
    }
}

/// The thumb covers the same share of the track that the viewport covers of the content.
fn thumb_height(track_height: f32, viewport_height: f32, max_offset: f32) -> f32 {
    (track_height * viewport_height / (viewport_height + max_offset))
        .max(THUMB_MIN_HEIGHT)
        .min(track_height)
}

impl IntoElement for OverlayScrollbar {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for OverlayScrollbar {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        ThumbGeometry::new(bounds, &self.handle)
            .map(|_| window.insert_hitbox(bounds, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        let (Some(hitbox), Some(global_id)) = (hitbox.clone(), global_id) else {
            return;
        };
        let Some(geometry) = ThumbGeometry::new(bounds, &self.handle) else {
            return;
        };
        let state = window.with_element_state(
            global_id,
            |state: Option<Rc<RefCell<ScrollbarState>>>, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            },
        );
        let viewport = self.handle.bounds();
        let dragging = state.borrow().grab.is_some();
        let pointer_inside = viewport.contains(&window.mouse_position());
        state.borrow_mut().pointer_inside = pointer_inside;

        if dragging || pointer_inside {
            let emphasized = dragging || hitbox.is_hovered(window);
            window.paint_quad(
                fill(
                    geometry.thumb,
                    self.color.opacity(if emphasized { 0.55 } else { 0.32 }),
                )
                .corner_radii(px(THUMB_WIDTH / 2.0)),
            );
        }

        window.on_mouse_event({
            let state = state.clone();
            let handle = self.handle.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                let grab = if geometry.thumb.contains(&event.position) {
                    event.position.y - geometry.thumb.top()
                } else {
                    // Clicking the track centers the thumb on the pointer.
                    let grab = geometry.thumb.size.height / 2.0;
                    geometry.scroll_to(event.position.y - grab, &handle);
                    grab
                };
                state.borrow_mut().grab = Some(grab);
                cx.stop_propagation();
                window.refresh();
            }
        });
        window.on_mouse_event({
            let state = state.clone();
            let handle = self.handle.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                let mut state = state.borrow_mut();
                if let Some(grab) = state.grab {
                    if event.pressed_button == Some(MouseButton::Left) {
                        geometry.scroll_to(event.position.y - grab, &handle);
                        cx.stop_propagation();
                    } else {
                        state.grab = None;
                    }
                    window.refresh();
                } else if viewport.contains(&event.position) != state.pointer_inside {
                    window.refresh();
                }
            }
        });
        window.on_mouse_event(move |_: &MouseUpEvent, phase, window, _| {
            if phase == DispatchPhase::Bubble && state.borrow_mut().grab.take().is_some() {
                window.refresh();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_height_tracks_the_visible_share_of_the_content() {
        // Half the content is visible, so the thumb fills half the track.
        assert_eq!(thumb_height(200.0, 300.0, 300.0), 100.0);
        // Long lists keep a thumb that is still easy to grab.
        assert_eq!(thumb_height(200.0, 300.0, 30_000.0), THUMB_MIN_HEIGHT);
        assert_eq!(thumb_height(20.0, 300.0, 30_000.0), 20.0);
    }
}
