use gpui::{
    App, Bounds, Hitbox, HitboxBehavior, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, ScrollHandle, ScrollWheelEvent, Window, canvas, fill, point, prelude::*, px, size,
};
use gpui_component::ActiveTheme;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
    time::{Duration, Instant},
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
/// Idle fade timing for the horizontal scrollbar, matching the vertical
/// scrollbar (FADE_OUT_DELAY and FADE_OUT_DURATION in gpui-component's
/// Scrollbar): the thumb holds for the delay after the last activity, then
/// fades out over the final second.
const CARD_SCROLLBAR_FADE_OUT_DELAY: f32 = 2.;
const CARD_SCROLLBAR_FADE_OUT_DURATION: f32 = 3.;
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

/// Persistent visual state for one carousel scrollbar: hover plus the idle
/// fade timing that mirrors the vertical scrollbar.
struct CardScrollbarShowState {
    owner: Weak<Cell<Option<CarouselDrag>>>,
    hovered: Cell<bool>,
    last_scroll_offset: Cell<f32>,
    last_scroll_time: Cell<Option<Instant>>,
    fade_wakeup_scheduled: Cell<bool>,
}

thread_local! {
    static CARD_SCROLLBAR_SHOW_STATES: RefCell<HashMap<usize, Rc<CardScrollbarShowState>>> =
        RefCell::new(HashMap::new());
}

fn card_scrollbar_show_state(drag: &Rc<Cell<Option<CarouselDrag>>>) -> Rc<CardScrollbarShowState> {
    let key = Rc::as_ptr(drag) as usize;
    CARD_SCROLLBAR_SHOW_STATES.with(|states| {
        let mut states = states.borrow_mut();
        states.retain(|_, state| state.owner.upgrade().is_some());
        states
            .entry(key)
            .or_insert_with(|| {
                Rc::new(CardScrollbarShowState {
                    owner: Rc::downgrade(drag),
                    hovered: Cell::new(false),
                    last_scroll_offset: Cell::new(0.),
                    last_scroll_time: Cell::new(None),
                    fade_wakeup_scheduled: Cell::new(false),
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

/// Refresh the scrollbar's show window from live interaction, matching the
/// vertical scrollbar: any offset change, hover, or drag counts as activity
/// and holds the thumb fully visible.
fn refresh_scrollbar_activity(
    show_state: &CardScrollbarShowState,
    offset: f32,
    hovered: bool,
    dragging: bool,
    now: Instant,
) {
    if hovered || dragging || offset != show_state.last_scroll_offset.get() {
        show_state.last_scroll_offset.set(offset);
        show_state.last_scroll_time.set(Some(now));
    }
}

fn scrollbar_is_available(max_extent: f32) -> bool {
    super::card_row_has_more(0., max_extent)
}

/// Thumb opacity `elapsed` seconds after the scrollbar's last activity.
/// Mirrors the vertical scrollbar's idle fade: fully visible through the
/// delay window, then a one-second ease to hidden. Reduced motion skips the
/// animated tail and hides as soon as the delay elapses.
fn scrollbar_fade_opacity(elapsed: Option<f32>, reduced_motion: bool) -> f32 {
    let Some(elapsed) = elapsed else {
        return 0.;
    };
    if elapsed < CARD_SCROLLBAR_FADE_OUT_DELAY {
        1.
    } else if !reduced_motion && elapsed < CARD_SCROLLBAR_FADE_OUT_DURATION {
        1. - (elapsed - CARD_SCROLLBAR_FADE_OUT_DELAY).powi(10)
    } else {
        0.
    }
}

/// Wake the window once the scrollbar's hold window ends so the idle fade can
/// start without further input, mirroring the vertical scrollbar's idle
/// timer.
fn schedule_scrollbar_fade_wakeup(
    show_state: &Rc<CardScrollbarShowState>,
    delay: f32,
    window: &mut Window,
    cx: &App,
) {
    if show_state.fade_wakeup_scheduled.get() {
        return;
    }
    show_state.fade_wakeup_scheduled.set(true);
    let show_state = show_state.clone();
    window
        .spawn(cx, async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f32(delay.max(0.)))
                .await;
            show_state.fade_wakeup_scheduled.set(false);
            cx.update(|window, _| window.refresh()).ok();
        })
        .detach();
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
    // this state by CardCarouselState's persistent drag Rc keeps the hover and
    // fade timing across those rebuilds without adding a field outside this
    // module.
    let show_state = card_scrollbar_show_state(&event_state.drag);
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
                show_state.hovered.set(false);
                // A row that stops overflowing restarts its show window hidden.
                show_state
                    .last_scroll_offset
                    .set(f32::from(event_state.scroll_handle.offset().x));
                show_state.last_scroll_time.set(None);
                return;
            }
            let now = Instant::now();
            let dragging = event_state.drag.get().is_some();
            let hovered = show_state.hovered.get();
            refresh_scrollbar_activity(
                &show_state,
                f32::from(event_state.scroll_handle.offset().x),
                hovered,
                dragging,
                now,
            );
            let elapsed = show_state
                .last_scroll_time
                .get()
                .map(|last| now.saturating_duration_since(last).as_secs_f32());
            let opacity = scrollbar_fade_opacity(elapsed, cx.reduce_motion());
            if opacity > 0. {
                paint_scrollbar(
                    bounds,
                    paint_data.thumb_bounds,
                    hovered,
                    dragging,
                    opacity,
                    window,
                    cx,
                );
            }
            // Drive the idle fade without hover or drag activity: wake once
            // when the hold window ends, then animate every frame until the
            // thumb is hidden.
            if let Some(elapsed) = elapsed.filter(|_| !hovered && !dragging) {
                if elapsed < CARD_SCROLLBAR_FADE_OUT_DELAY {
                    schedule_scrollbar_fade_wakeup(
                        &show_state,
                        CARD_SCROLLBAR_FADE_OUT_DELAY - elapsed,
                        window,
                        cx,
                    );
                } else if opacity > 0. {
                    window.request_animation_frame();
                }
            }
            register_handlers(
                paint_data,
                event_state.drag.clone(),
                event_state.scroll_handle.clone(),
                event_state.snap_epoch.clone(),
                event_state.pending_target.clone(),
                card_pitch,
                cursor_owner,
                show_state.clone(),
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
    opacity: f32,
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
    let thumb_color = scrollbar_thumb_color(cx, thumb_state).opacity(opacity);
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
    show_state: Rc<CardScrollbarShowState>,
    window: &mut Window,
) {
    let window_active = window.is_window_active();
    if !window_active && drag.take().is_some() {
        window.release_pointer();
        set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
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
    let move_hover = show_state.clone();
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
    let up_hover = show_state.clone();
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
    use std::{
        cell::Cell,
        rc::Weak,
        time::{Duration, Instant},
    };

    use gpui::{TestAppContext, prelude::*};
    use gpui_component::ActiveTheme;

    use super::{
        CARD_SCROLLBAR_FADE_OUT_DELAY, CARD_SCROLLBAR_FADE_OUT_DURATION, CardScrollbarShowState,
        ScrollbarThumbState, active_carousel_cursor_state, card_scrollbar_show_state,
        refresh_scrollbar_activity, scrollbar_fade_opacity, scrollbar_is_available,
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
    #[test]
    fn scrollbar_fade_holds_then_eases_to_hidden_like_the_vertical_scrollbar() {
        assert_eq!(CARD_SCROLLBAR_FADE_OUT_DELAY, 2.);
        assert_eq!(CARD_SCROLLBAR_FADE_OUT_DURATION, 3.);

        // Never shown: hidden.
        assert_eq!(scrollbar_fade_opacity(None, false), 0.);
        // Activity holds the thumb fully visible through the delay window.
        assert_eq!(scrollbar_fade_opacity(Some(0.), false), 1.);
        assert_eq!(scrollbar_fade_opacity(Some(1.999), false), 1.);
        // The final second eases toward hidden.
        let fading = scrollbar_fade_opacity(Some(2.5), false);
        assert!(fading > 0.9 && fading < 1.);
        assert_eq!(scrollbar_fade_opacity(Some(3.), false), 0.);
        assert_eq!(scrollbar_fade_opacity(Some(4.), false), 0.);
    }

    #[test]
    fn scrollbar_fade_skips_the_animated_tail_under_reduced_motion() {
        assert_eq!(scrollbar_fade_opacity(Some(1.999), true), 1.);
        assert_eq!(scrollbar_fade_opacity(Some(2.), true), 0.);
        assert_eq!(scrollbar_fade_opacity(Some(2.5), true), 0.);
    }

    #[test]
    fn scrollbar_activity_refreshes_on_offset_hover_and_drag() {
        let show_state = CardScrollbarShowState {
            owner: Weak::new(),
            hovered: Cell::new(false),
            last_scroll_offset: Cell::new(0.),
            last_scroll_time: Cell::new(None),
            fade_wakeup_scheduled: Cell::new(false),
        };
        let t0 = Instant::now();

        // A still, unhovered row is not activity.
        refresh_scrollbar_activity(&show_state, 0., false, false, t0);
        assert_eq!(show_state.last_scroll_time.get(), None);

        // Any offset change refreshes the show window.
        refresh_scrollbar_activity(&show_state, -40., false, false, t0);
        assert_eq!(show_state.last_scroll_time.get(), Some(t0));
        assert_eq!(show_state.last_scroll_offset.get(), -40.);

        // Hover and drags hold the thumb visible without offset changes.
        let t1 = t0 + Duration::from_secs(5);
        refresh_scrollbar_activity(&show_state, -40., true, false, t1);
        assert_eq!(show_state.last_scroll_time.get(), Some(t1));
        let t2 = t1 + Duration::from_secs(5);
        refresh_scrollbar_activity(&show_state, -40., false, true, t2);
        assert_eq!(show_state.last_scroll_time.get(), Some(t2));

        // Going idle again leaves the show window to age out.
        let t3 = t2 + Duration::from_secs(5);
        refresh_scrollbar_activity(&show_state, -40., false, false, t3);
        assert_eq!(show_state.last_scroll_time.get(), Some(t2));
    }

    #[gpui::test]
    fn scrollbar_show_window_follows_the_rendered_scroll_activity(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let cx = cx.add_empty_window();

        struct CarouselHost(super::CardCarouselState);
        impl gpui::Render for CarouselHost {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                crate::music_ui::card_carousel(
                    "scrollbar-show-wiring",
                    self.0.clone(),
                    150.,
                    12.,
                    true,
                    gpui::div()
                        .flex()
                        .flex_none()
                        .children((0..8).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))))
                        .into_any_element(),
                )
            }
        }

        let state = super::CardCarouselState::new();
        let show_state = card_scrollbar_show_state(&state.drag);
        let view = cx.update(|_, cx| cx.new(|_| CarouselHost(state.clone())));
        let draw = |cx: &mut gpui::VisualTestContext, view: &gpui::Entity<CarouselHost>| {
            cx.draw(
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::size(gpui::px(400.), gpui::px(300.)),
                {
                    let view = view.clone();
                    move |_, _| view.into_any_element()
                },
            );
        };

        // A fresh row has not scrolled, so the thumb stays hidden.
        draw(cx, &view);
        assert_eq!(show_state.last_scroll_time.get(), None);

        // Scrolling counts as activity and holds the thumb fully visible.
        let extent = f32::from(state.scroll_handle.max_offset().x);
        assert!(extent > 0.);
        state
            .scroll_handle
            .set_offset(gpui::point(gpui::px(-(extent - 20.)), gpui::px(0.)));
        draw(cx, &view);
        let elapsed = show_state
            .last_scroll_time
            .get()
            .map(|last| last.elapsed().as_secs_f32());
        assert!(elapsed.is_some());
        assert_eq!(scrollbar_fade_opacity(elapsed, false), 1.);
    }
}
