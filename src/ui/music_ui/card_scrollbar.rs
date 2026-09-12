use gpui::{
    App, Bounds, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollHandle, ScrollWheelEvent, Window, canvas, fill, point, prelude::*, px, size,
};
use gpui_component::ActiveTheme;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

use super::{
    CAROUSEL_CONTROL_HEIGHT, CAROUSEL_THUMB_HEIGHT, CAROUSEL_TRACK_INSET, CardCarouselState,
    CarouselDrag, CarouselDragMode, carousel_drag_offset, carousel_motion, carousel_snap_offset,
    should_consume_horizontal_scroll,
};
use crate::drag_cursor::{
    DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned,
};

const MIN_THUMB_WIDTH: f32 = 18.;
#[derive(Clone, Copy)]
enum ScrollbarThumbState {
    Idle,
    Hovered,
    Dragging,
}

fn scrollbar_thumb_color(cx: &App, state: ScrollbarThumbState) -> gpui::Background {
    match state {
        ScrollbarThumbState::Idle => cx.theme().tokens.scrollbar_thumb.into(),
        ScrollbarThumbState::Hovered | ScrollbarThumbState::Dragging => {
            cx.theme().tokens.scrollbar_thumb_hover.into()
        }
    }
}

struct CardScrollbarHoverState {
    owner: Weak<Cell<Option<CarouselDrag>>>,
    hovered: Cell<bool>,
}

thread_local! {
    static CARD_SCROLLBAR_HOVER_STATES: RefCell<HashMap<usize, Rc<CardScrollbarHoverState>>> =
        RefCell::new(HashMap::new());
}

fn card_scrollbar_hover_state(
    drag: &Rc<Cell<Option<CarouselDrag>>>,
) -> Rc<CardScrollbarHoverState> {
    let key = Rc::as_ptr(drag) as usize;
    CARD_SCROLLBAR_HOVER_STATES.with(|states| {
        let mut states = states.borrow_mut();
        states.retain(|_, state| state.owner.upgrade().is_some());
        states
            .entry(key)
            .or_insert_with(|| {
                Rc::new(CardScrollbarHoverState {
                    owner: Rc::downgrade(drag),
                    hovered: Cell::new(false),
                })
            })
            .clone()
    })
}

fn update_scrollbar_hover(state: &Cell<bool>, track_hovered: bool, dragging: bool) -> bool {
    let hovered = track_hovered || dragging;
    let changed = state.get() != hovered;
    state.set(hovered);
    changed
}

fn scrollbar_is_available(max_extent: f32) -> bool {
    super::card_row_has_more(0., max_extent)
}

#[derive(Clone)]
struct CardScrollbarPaintState {
    viewport_hitbox: Hitbox,
    track_hitbox: Hitbox,
    thumb_bounds: Bounds<Pixels>,
    max_extent: f32,
    thumb_travel: f32,
}

pub(crate) fn card_scrollbar(state: CardCarouselState, card_pitch: f32) -> impl IntoElement {
    let prepaint_state = state.clone();
    let event_state = state.clone();
    let cursor_owner = DragCursorOwner::carousel(std::rc::Rc::as_ptr(&event_state.drag) as usize);
    // The carousel is rebuilt when Window::refresh bypasses view caching. Keying
    // this state by CardCarouselState's persistent drag Rc keeps hover across
    // those rebuilds without adding a field outside this module.
    let hover_state = card_scrollbar_hover_state(&event_state.drag);
    canvas(
        move |bounds, window, _cx| {
            let viewport_bounds = viewport_bounds(&prepaint_state.scroll_handle);
            let track_bounds = track_bounds(bounds);
            let geometry = scrollbar_geometry(
                track_bounds,
                viewport_bounds.size.width,
                &prepaint_state.scroll_handle,
            );
            CardScrollbarPaintState {
                viewport_hitbox: window.insert_hitbox(viewport_bounds, HitboxBehavior::Normal),
                track_hitbox: window.insert_hitbox(track_bounds, HitboxBehavior::Normal),
                thumb_bounds: geometry.thumb_bounds,
                max_extent: geometry.max_extent,
                thumb_travel: geometry.thumb_travel,
            }
        },
        move |bounds, paint_data, window, cx| {
            let available = scrollbar_is_available(paint_data.max_extent);
            if !available {
                if event_state.drag.take().is_some() {
                    window.release_pointer();
                    set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
                }
                hover_state.hovered.set(false);
                return;
            }
            let dragging = event_state.drag.get().is_some();
            paint_scrollbar(
                bounds,
                paint_data.thumb_bounds,
                hover_state.hovered.get(),
                dragging,
                window,
                cx,
            );
            register_handlers(
                paint_data,
                event_state.drag.clone(),
                event_state.scroll_handle.clone(),
                event_state.snap_epoch.clone(),
                event_state.pending_target.clone(),
                card_pitch,
                cursor_owner,
                hover_state.clone(),
                window,
            );
        },
    )
    .absolute()
    .inset_0()
}

#[derive(Clone, Copy)]
struct ScrollbarGeometry {
    thumb_bounds: Bounds<Pixels>,
    max_extent: f32,
    thumb_travel: f32,
}

fn viewport_bounds(scroll_handle: &ScrollHandle) -> Bounds<Pixels> {
    let mut bounds = scroll_handle.bounds();
    bounds.size.height = (bounds.size.height - px(CAROUSEL_CONTROL_HEIGHT)).max(px(0.));
    bounds
}

fn track_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let track_width = (bounds.size.width - px(CAROUSEL_TRACK_INSET * 2.)).max(px(0.));
    Bounds {
        origin: point(
            bounds.origin.x + px(CAROUSEL_TRACK_INSET),
            bounds.origin.y + bounds.size.height - px(CAROUSEL_CONTROL_HEIGHT),
        ),
        size: size(track_width, px(CAROUSEL_CONTROL_HEIGHT)),
    }
}

fn scrollbar_geometry(
    track_bounds: Bounds<Pixels>,
    viewport_width: Pixels,
    scroll_handle: &ScrollHandle,
) -> ScrollbarGeometry {
    let track_width = f32::from(track_bounds.size.width).max(0.);
    let max_extent = f32::from(scroll_handle.max_offset().x).abs();
    let viewport_width = f32::from(viewport_width).max(0.);
    let content_width = viewport_width + max_extent;
    let thumb_width = if content_width > 0. {
        (track_width * viewport_width / content_width)
            .max(MIN_THUMB_WIDTH.min(track_width))
            .min(track_width)
    } else {
        track_width
    };
    let thumb_travel = (track_width - thumb_width).max(0.);
    let offset = f32::from(scroll_handle.offset().x).clamp(-max_extent, 0.);
    let progress = if max_extent > 0. {
        (-offset / max_extent).clamp(0., 1.)
    } else {
        0.
    };
    let thumb_x = track_bounds.origin.x + px(thumb_travel * progress);
    let thumb_y =
        track_bounds.origin.y + px((CAROUSEL_CONTROL_HEIGHT - CAROUSEL_THUMB_HEIGHT) * 0.5);
    ScrollbarGeometry {
        thumb_bounds: Bounds {
            origin: point(thumb_x, thumb_y),
            size: size(px(thumb_width), px(CAROUSEL_THUMB_HEIGHT)),
        },
        max_extent,
        thumb_travel,
    }
}

fn paint_scrollbar(
    bounds: Bounds<Pixels>,
    thumb_bounds: Bounds<Pixels>,
    hovered: bool,
    dragging: bool,
    window: &mut Window,
    cx: &App,
) {
    let track = track_bounds(bounds);
    if track.size.width <= px(0.) || bounds.size.height < px(CAROUSEL_CONTROL_HEIGHT) {
        return;
    }
    let thumb_state = if dragging {
        ScrollbarThumbState::Dragging
    } else if hovered {
        ScrollbarThumbState::Hovered
    } else {
        ScrollbarThumbState::Idle
    };
    let thumb_color = scrollbar_thumb_color(cx, thumb_state);
    window
        .paint_quad(fill(thumb_bounds, thumb_color).corner_radii(px(CAROUSEL_THUMB_HEIGHT * 0.5)));
}

fn active_carousel_cursor_state(mode: Option<CarouselDragMode>) -> Option<DragCursorState> {
    (mode == Some(CarouselDragMode::Viewport)).then_some(DragCursorState::Grabbing)
}

fn register_handlers(
    paint_state: CardScrollbarPaintState,
    drag: std::rc::Rc<std::cell::Cell<Option<CarouselDrag>>>,
    scroll_handle: ScrollHandle,
    snap_epoch: Rc<Cell<u64>>,
    pending_target: Rc<Cell<Option<f32>>>,
    card_pitch: f32,
    cursor_owner: DragCursorOwner,
    hover_state: Rc<CardScrollbarHoverState>,
    window: &mut Window,
) {
    let window_active = window.is_window_active();
    if !window_active {
        if drag.take().is_some() {
            window.release_pointer();
            set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
        }
    }
    if let Some(DragCursorState::Grabbing) =
        active_carousel_cursor_state(drag.get().map(|drag| drag.mode))
    {
        window.set_window_cursor_style(grabbing_cursor());
        set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
    }

    let track_hitbox = paint_state.track_hitbox.clone();
    let viewport_hitbox = paint_state.viewport_hitbox.clone();
    let down_drag = drag.clone();
    let down_scroll = scroll_handle.clone();
    let down_snap_epoch = snap_epoch.clone();
    let down_pending_target = pending_target.clone();
    let down_paint = paint_state;
    let down_track = track_hitbox.clone();
    let down_viewport = viewport_hitbox.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        match event.button {
            MouseButton::Left if down_track.is_hovered(window) => {
                carousel_motion::cancel(&down_snap_epoch, &down_pending_target);
                if down_paint.max_extent <= 0. || down_paint.thumb_travel <= 0. {
                    return;
                }
                let thumb = down_paint.thumb_bounds;
                let pointer_x = f32::from(event.position.x);
                let thumb_left = if pointer_x < f32::from(thumb.origin.x)
                    || pointer_x > f32::from(thumb.right())
                {
                    (pointer_x - f32::from(thumb.size.width) * 0.5).clamp(
                        f32::from(down_paint.track_hitbox.bounds.origin.x),
                        f32::from(down_paint.track_hitbox.bounds.right() - thumb.size.width),
                    )
                } else {
                    f32::from(thumb.origin.x)
                };
                let progress = ((thumb_left - f32::from(down_paint.track_hitbox.bounds.origin.x))
                    / down_paint.thumb_travel)
                    .clamp(0., 1.);
                let start_offset = -down_paint.max_extent * progress;
                down_scroll.set_offset(point(px(start_offset), down_scroll.offset().y));
                down_drag.set(Some(CarouselDrag {
                    pointer_x,
                    start_offset,
                    scale: down_paint.max_extent / down_paint.thumb_travel,
                    mode: CarouselDragMode::Scrollbar,
                }));
                window.capture_pointer(down_track.id);
                window.prevent_default();
                window.refresh();
                cx.stop_propagation();
            }
            MouseButton::Middle if down_viewport.is_hovered(window) => {
                carousel_motion::cancel(&down_snap_epoch, &down_pending_target);
                down_drag.set(Some(CarouselDrag {
                    pointer_x: f32::from(event.position.x),
                    start_offset: f32::from(down_scroll.offset().x),
                    scale: 1.,
                    mode: CarouselDragMode::Viewport,
                }));
                window.capture_pointer(down_viewport.id);
                set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
                window.prevent_default();
                window.refresh();
                cx.stop_propagation();
            }
            _ => {}
        }
    });

    let move_drag = drag.clone();
    let move_scroll = scroll_handle.clone();
    let move_snap_epoch = snap_epoch.clone();
    let move_pending_target = pending_target.clone();
    let move_owner = cursor_owner;
    let move_track = track_hitbox.clone();
    let move_hover = hover_state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        if update_scrollbar_hover(
            &move_hover.hovered,
            move_track.is_hovered(window),
            move_drag.get().is_some(),
        ) {
            window.refresh();
        }
        let Some(drag_state) = move_drag.get() else {
            return;
        };
        let expected_button = match drag_state.mode {
            CarouselDragMode::Viewport => MouseButton::Middle,
            CarouselDragMode::Scrollbar => MouseButton::Left,
        };
        if event.pressed_button != Some(expected_button) {
            finish_drag(
                &move_drag,
                &move_scroll,
                &move_snap_epoch,
                &move_pending_target,
                card_pitch,
                move_owner,
                window,
                cx,
                true,
            );
            return;
        }
        let pointer_delta = f32::from(event.position.x) - drag_state.pointer_x;
        let offset = match drag_state.mode {
            CarouselDragMode::Viewport => carousel_drag_offset(
                drag_state.start_offset,
                pointer_delta,
                f32::from(move_scroll.max_offset().x),
            ),
            CarouselDragMode::Scrollbar => {
                let extent = f32::from(move_scroll.max_offset().x).abs();
                (drag_state.start_offset - pointer_delta * drag_state.scale).clamp(-extent, 0.)
            }
        };
        move_scroll.set_offset(point(px(offset), move_scroll.offset().y));
        if drag_state.mode == CarouselDragMode::Viewport {
            set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
        }
        window.refresh();
        cx.stop_propagation();
    });

    let up_drag = drag.clone();
    let up_scroll = scroll_handle.clone();
    let up_snap_epoch = snap_epoch.clone();
    let up_pending_target = pending_target.clone();
    let up_owner = cursor_owner;
    let up_track = track_hitbox.clone();
    let up_hover = hover_state.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase.bubble() {
            let Some(drag_state) = up_drag.get() else {
                return;
            };
            let expected_button = match drag_state.mode {
                CarouselDragMode::Viewport => MouseButton::Middle,
                CarouselDragMode::Scrollbar => MouseButton::Left,
            };
            if event.button == expected_button {
                finish_drag(
                    &up_drag,
                    &up_scroll,
                    &up_snap_epoch,
                    &up_pending_target,
                    card_pitch,
                    up_owner,
                    window,
                    cx,
                    true,
                );
                if update_scrollbar_hover(&up_hover.hovered, up_track.is_hovered(window), false) {
                    window.refresh();
                }
            }
        }
    });

    let wheel_viewport = viewport_hitbox.clone();
    let wheel_snap_epoch = snap_epoch.clone();
    let wheel_pending_target = pending_target.clone();
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if phase.capture() && wheel_viewport.should_handle_scroll(window) {
            let delta = event.delta.pixel_delta(window.line_height());
            if should_consume_horizontal_scroll(f32::from(delta.x), f32::from(delta.y)) {
                carousel_motion::cancel(&wheel_snap_epoch, &wheel_pending_target);
                cx.stop_propagation();
            }
        }
    });
}

fn finish_drag(
    drag: &std::rc::Rc<std::cell::Cell<Option<CarouselDrag>>>,
    scroll_handle: &ScrollHandle,
    snap_epoch: &Rc<Cell<u64>>,
    pending_target: &Rc<Cell<Option<f32>>>,
    card_pitch: f32,
    cursor_owner: DragCursorOwner,
    window: &mut Window,
    cx: &mut App,
    stop_propagation: bool,
) {
    let Some(drag_state) = drag.take() else {
        return;
    };
    if drag_state.mode == CarouselDragMode::Viewport {
        let current = f32::from(scroll_handle.offset().x);
        let target =
            carousel_snap_offset(current, f32::from(scroll_handle.max_offset().x), card_pitch);
        pending_target.set(Some(target));
        carousel_motion::start(
            scroll_handle.clone(),
            snap_epoch.clone(),
            pending_target.clone(),
            target,
            cx.reduce_motion(),
            window,
        );
    }
    window.release_pointer();
    window.refresh();
    set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
    if stop_propagation {
        cx.stop_propagation();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::TestAppContext;
    use gpui_component::ActiveTheme;

    use super::{
        ScrollbarThumbState, active_carousel_cursor_state, scrollbar_is_available,
        scrollbar_thumb_color, update_scrollbar_hover,
    };

    #[test]
    fn only_middle_button_viewport_drags_own_the_closed_hand() {
        assert_eq!(active_carousel_cursor_state(None), None);
        assert_eq!(
            active_carousel_cursor_state(Some(super::CarouselDragMode::Scrollbar)),
            None
        );
        assert_eq!(
            active_carousel_cursor_state(Some(super::CarouselDragMode::Viewport)),
            Some(super::DragCursorState::Grabbing)
        );
    }

    #[gpui::test]
    fn scrollbar_thumb_color_matches_vertical_scrollbar_states(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);

            let idle = gpui::Background::from(cx.theme().tokens.scrollbar_thumb);
            let hover = gpui::Background::from(cx.theme().tokens.scrollbar_thumb_hover);

            assert_eq!(scrollbar_thumb_color(cx, ScrollbarThumbState::Idle), idle);
            assert_eq!(
                scrollbar_thumb_color(cx, ScrollbarThumbState::Hovered),
                hover
            );
            assert_eq!(
                scrollbar_thumb_color(cx, ScrollbarThumbState::Dragging),
                hover
            );
        });
    }

    #[test]
    fn scrollbar_hover_refreshes_only_when_hover_changes() {
        let hovered = Cell::new(false);

        assert!(update_scrollbar_hover(&hovered, true, false));
        assert!(!update_scrollbar_hover(&hovered, true, false));
        assert!(!update_scrollbar_hover(&hovered, false, true));
        assert!(update_scrollbar_hover(&hovered, false, false));
        assert!(!update_scrollbar_hover(&hovered, false, false));
    }

    #[test]
    fn scrollbar_is_hidden_until_the_carousel_has_real_overflow() {
        assert!(!scrollbar_is_available(0.));
        assert!(!scrollbar_is_available(2.));
        assert!(scrollbar_is_available(2.1));
        assert!(scrollbar_is_available(-20.));
    }
}
