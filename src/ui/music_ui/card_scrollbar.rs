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
    CAROUSEL_CARD_ROW_BOTTOM_GAP, CAROUSEL_CONTENT_BOTTOM_PADDING, CAROUSEL_CONTROL_HEIGHT,
    CAROUSEL_THUMB_HEIGHT, CAROUSEL_TRACK_INSET, CardCarouselState, CarouselDrag, CarouselDragMode,
    carousel_drag_offset, carousel_motion, carousel_snap_offset, should_consume_horizontal_scroll,
};
use crate::drag_cursor::{
    DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned,
};
use crate::motion::CAROUSEL_CONTROLS_FADE_DURATION;

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

/// Persistent interaction state for one carousel scrollbar: hover plus the
/// offset its show window was last refreshed from. The idle fade timing
/// itself lives in the shared controls fade so the arrows can follow it.
struct CardScrollbarShowState {
    owner: Weak<Cell<Option<CarouselDrag>>>,
    hovered: Cell<bool>,
    last_scroll_offset: Cell<f32>,
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

/// Whether this frame counts as activity for the shared controls fade:
/// any offset change, hover, or drag refreshes the show window, matching
/// the vertical scrollbar.
fn scrollbar_activity(
    show_state: &CardScrollbarShowState,
    offset: f32,
    hovered: bool,
    dragging: bool,
) -> bool {
    hovered || dragging || offset != show_state.last_scroll_offset.get()
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

/// Shared visibility for the carousel's horizontal scrollbar and both
/// arrows, which appear and disappear as one unit. The row's live
/// overflow gates the whole unit: when the row starts overflowing the
/// controls fade in together, and when it stops they fade out together
/// no matter how active the row still is. While the row keeps
/// overflowing, the unit follows the scrollbar's activity window: any
/// scroll, hover, or drag shows it, the hold window keeps it visible,
/// and going idle fades it back out.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CarouselControlsFade {
    overflow: bool,
    last_activity: Option<Instant>,
    initialized: bool,
    gate_from: f32,
    gate_to: f32,
    gate_started_at: Option<Instant>,
    reduced_motion: bool,
    last_opacity: f32,
    needs_render_sync: bool,
}

impl CarouselControlsFade {
    /// Advance the state with this frame's inputs. `overflow` is the
    /// row's live measured overflow and `activity` whether the user
    /// interacted this frame. The first update settles instead of
    /// animating, so freshly mounted rows and skeleton carousels that
    /// rebuild their state never flicker or drive endless frames.
    pub(crate) fn update(
        &mut self,
        overflow: bool,
        activity: bool,
        now: Instant,
        reduced_motion: bool,
    ) {
        self.reduced_motion = reduced_motion;
        let was_initialized = self.initialized;
        self.initialized = true;
        if !was_initialized {
            self.overflow = overflow;
            self.gate_to = if overflow { 1. } else { 0. };
            self.gate_from = self.gate_to;
            self.gate_started_at = None;
            // A row that is already scrollable when first measured shows
            // its controls right away.
            self.last_activity = overflow.then_some(now);
            self.needs_render_sync = false;
            self.last_opacity = self.opacity_at(now);
            return;
        }
        if activity || (overflow && !self.overflow) {
            self.last_activity = Some(now);
        }
        let gate_was_running = self.gate_started_at.is_some();
        if overflow != self.overflow {
            self.overflow = overflow;
            self.gate_from = self.gate_at(now);
            self.gate_to = if overflow { 1. } else { 0. };
            self.gate_started_at =
                (!reduced_motion && self.gate_from != self.gate_to).then_some(now);
            if reduced_motion || self.gate_from == self.gate_to {
                self.gate_from = self.gate_to;
                self.gate_started_at = None;
            }
        } else if gate_was_running
            && let Some(started_at) = self.gate_started_at
            && now.saturating_duration_since(started_at) >= CAROUSEL_CONTROLS_FADE_DURATION
        {
            self.gate_from = self.gate_to;
            self.gate_started_at = None;
        }
        let opacity = self.opacity_at(now);
        // One more frame whenever the shown-or-hidden answer changes, or
        // a running fade settles, so render-time consumers catch up
        // exactly once instead of sticking a frame behind.
        self.needs_render_sync = (self.last_opacity > 0.) != (opacity > 0.)
            || (gate_was_running && self.gate_started_at.is_none());
        self.last_opacity = opacity;
    }

    /// The overflow gate's animated zero-to-one value at `now`.
    fn gate_at(&self, now: Instant) -> f32 {
        let Some(started_at) = self.gate_started_at else {
            return self.gate_to;
        };
        let elapsed = now.saturating_duration_since(started_at);
        if elapsed >= CAROUSEL_CONTROLS_FADE_DURATION {
            return self.gate_to;
        }
        let duration = CAROUSEL_CONTROLS_FADE_DURATION.as_secs_f32();
        let progress = if duration == 0. {
            1.
        } else {
            (elapsed.as_secs_f32() / duration).clamp(0., 1.)
        };
        crate::motion::lerp(self.gate_from, self.gate_to, gpui::ease_in_out(progress))
    }

    /// The shared opacity for the scrollbar and both arrows at `now`:
    /// the overflow gate times the scrollbar's activity fade.
    pub(crate) fn opacity_at(&self, now: Instant) -> f32 {
        let elapsed = self
            .last_activity
            .map(|last| now.saturating_duration_since(last).as_secs_f32());
        self.gate_at(now) * scrollbar_fade_opacity(elapsed, self.reduced_motion)
    }

    /// Opacity for render-time consumers such as the arrows, which are
    /// built before the scrollbar paint has measured this frame's
    /// overflow. An unmeasured state defers to the caller's layout
    /// estimate; once the paint starts driving the state, the shared
    /// fade takes over.
    pub(crate) fn render_opacity(&self, now: Instant, overflow_estimate: bool) -> f32 {
        if !self.initialized {
            if overflow_estimate { 1. } else { 0. }
        } else {
            self.opacity_at(now)
        }
    }

    /// Whether the shared fade still owes the window frames.
    pub(crate) fn is_animating(&self, now: Instant) -> bool {
        self.gate_started_at.is_some_and(|started_at| {
            now.saturating_duration_since(started_at) < CAROUSEL_CONTROLS_FADE_DURATION
        })
    }

    /// Whether this update changed the shown-or-hidden answer or settled
    /// a running fade, so consumers that only re-render can catch up.
    pub(crate) fn needs_render_sync(&self) -> bool {
        self.needs_render_sync
    }

    /// Whether the paint has started driving this state yet.
    pub(crate) fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Seconds since the controls' last activity, if the show window ever
    /// opened.
    pub(crate) fn idle_for(&self, now: Instant) -> Option<f32> {
        self.last_activity
            .map(|last| now.saturating_duration_since(last).as_secs_f32())
    }
}

#[derive(Clone)]
struct CardScrollbarPaintState {
    viewport_hitbox: Hitbox,
    track_hitbox: Option<Hitbox>,
    thumb_bounds: Bounds<Pixels>,
    max_extent: f32,
    thumb_travel: f32,
}

pub(crate) fn card_scrollbar(
    state: CardCarouselState,
    card_pitch: f32,
    strip_reserved: bool,
) -> impl IntoElement {
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
            let track_bounds = track_bounds(bounds, strip_reserved);
            let geometry = scrollbar_geometry(
                track_bounds,
                viewport_bounds.size.width,
                &prepaint_state.scroll_handle,
            );
            // The track hitbox exists whenever the row can scroll, even
            // while the shared controls are faded to zero opacity:
            // gating it on visibility would stop the faded strip from
            // ever catching the hover that brings the controls back. A
            // row with nothing to scroll keeps no hitbox, so its strip
            // stays non-interactive. The viewport hitbox stays for the
            // whole overflowing row: middle-button drags and wheel
            // consumption belong to the row itself, not to the visible
            // controls.
            let track_hitbox = scrollbar_is_available(geometry.max_extent)
                .then(|| window.insert_hitbox(track_bounds, HitboxBehavior::Normal));
            CardScrollbarPaintState {
                viewport_hitbox: window.insert_hitbox(viewport_bounds, HitboxBehavior::Normal),
                track_hitbox,
                thumb_bounds: geometry.thumb_bounds,
                max_extent: geometry.max_extent,
                thumb_travel: geometry.thumb_travel,
            }
        },
        move |bounds, paint_data, window, cx| {
            let offset = f32::from(event_state.scroll_handle.offset().x);
            let available = scrollbar_is_available(paint_data.max_extent);
            if !available {
                if event_state.drag.take().is_some() {
                    window.release_pointer();
                    set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
                }
                show_state.hovered.set(false);
            }
            let was_initialized = event_state.controls_fade.get().is_initialized();
            let now = Instant::now();
            let dragging = event_state.drag.get().is_some();
            let hovered = show_state.hovered.get();
            let activity = available && scrollbar_activity(&show_state, offset, hovered, dragging);
            show_state.last_scroll_offset.set(offset);
            let mut fade = event_state.controls_fade.get();
            fade.update(available, activity, now, cx.reduce_motion());
            let opacity = fade.opacity_at(now);
            let animating = fade.is_animating(now);
            let needs_render_sync = fade.needs_render_sync();
            event_state.controls_fade.set(fade);
            if opacity > 0. {
                paint_scrollbar(
                    bounds,
                    strip_reserved,
                    paint_data.thumb_bounds,
                    hovered,
                    dragging,
                    opacity,
                    window,
                    cx,
                );
            }
            if available {
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
            }
            // Drive the shared fade without hover or drag activity: wake
            // once when the hold window ends, then animate every frame
            // until the unit hides. States the paint has not driven yet
            // skip this: skeleton carousels rebuild their state every
            // render, and a settle must not arm an endless wakeup cycle.
            if was_initialized
                && !hovered
                && !dragging
                && let Some(elapsed) = fade.idle_for(now)
            {
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
            if animating || needs_render_sync {
                window.request_animation_frame();
            }
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

/// The track lane for the carousel scrollbar. The lane belongs to the card
/// row, `CAROUSEL_CARD_ROW_BOTTOM_GAP` below its bottom edge, no matter
/// where the caller reserved the strip's space: inside the scroll viewport
/// when it overflows, or below the carousel when it does not. Anchoring to
/// the card row keeps the thumb at one fixed position while the shared
/// controls fade, so the fade only ever changes opacity, never geometry.
fn track_bounds(bounds: Bounds<Pixels>, strip_reserved: bool) -> Bounds<Pixels> {
    let track_width = (bounds.size.width - px(CAROUSEL_TRACK_INSET * 2.)).max(px(0.));
    let content_bottom = bounds.origin.y + bounds.size.height
        - px(if strip_reserved {
            CAROUSEL_CONTENT_BOTTOM_PADDING
        } else {
            0.
        });
    Bounds {
        origin: point(
            bounds.origin.x + px(CAROUSEL_TRACK_INSET),
            content_bottom + px(CAROUSEL_CARD_ROW_BOTTOM_GAP),
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
    strip_reserved: bool,
    thumb_bounds: Bounds<Pixels>,
    hovered: bool,
    dragging: bool,
    opacity: f32,
    window: &mut Window,
    cx: &App,
) {
    let track = track_bounds(bounds, strip_reserved);
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
            MouseButton::Left => {
                // The track exists whenever the row can scroll, so a click
                // lands even on a faded scrollbar; the drag it starts is
                // activity, which revives the whole unit.
                let Some(track) = down_track.as_ref().filter(|track| track.is_hovered(window))
                else {
                    return;
                };
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
                        f32::from(track.bounds.origin.x),
                        f32::from(track.bounds.right() - thumb.size.width),
                    )
                } else {
                    f32::from(thumb.origin.x)
                };
                let progress = ((thumb_left - f32::from(track.bounds.origin.x))
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
                window.capture_pointer(track.id);
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
            move_track
                .as_ref()
                .is_some_and(|track| track.is_hovered(window)),
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
                if update_scrollbar_hover(
                    &up_hover.hovered,
                    up_track
                        .as_ref()
                        .is_some_and(|track| track.is_hovered(window)),
                    false,
                ) {
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

    use super::{
        CARD_SCROLLBAR_FADE_OUT_DELAY, CARD_SCROLLBAR_FADE_OUT_DURATION, CardScrollbarShowState,
        CarouselControlsFade, ScrollbarThumbState, active_carousel_cursor_state,
        scrollbar_activity, scrollbar_fade_opacity, scrollbar_geometry, scrollbar_is_available,
        scrollbar_thumb_color, track_bounds, update_scrollbar_hover,
    };
    use crate::motion::CAROUSEL_CONTROLS_FADE_DURATION;
    use gpui::{TestAppContext, prelude::*};
    use gpui_component::ActiveTheme;

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
    fn scrollbar_activity_comes_from_offset_hover_and_drag() {
        let show_state = CardScrollbarShowState {
            owner: Weak::new(),
            hovered: Cell::new(false),
            last_scroll_offset: Cell::new(0.),
            fade_wakeup_scheduled: Cell::new(false),
        };

        // A still, unhovered row is not activity.
        assert!(!scrollbar_activity(&show_state, 0., false, false));
        // Any offset change refreshes the show window.
        assert!(scrollbar_activity(&show_state, -40., false, false));
        show_state.last_scroll_offset.set(-40.);
        assert!(!scrollbar_activity(&show_state, -40., false, false));
        // Hover and drags hold the controls visible without offset changes.
        assert!(scrollbar_activity(&show_state, -40., true, false));
        assert!(scrollbar_activity(&show_state, -40., false, true));
    }

    #[test]
    fn controls_fade_settles_on_the_first_update() {
        let now = Instant::now();

        // A freshly measured overflowing row shows its controls right away,
        // without starting a fade: skeleton carousels rebuild their state
        // every render and must neither flicker nor drive frames.
        let mut shown = CarouselControlsFade::default();
        assert_eq!(shown.render_opacity(now, true), 1.);
        assert_eq!(shown.render_opacity(now, false), 0.);
        shown.update(true, false, now, false);
        assert_eq!(shown.opacity_at(now), 1.);
        assert!(!shown.is_animating(now));
        assert!(!shown.needs_render_sync());

        // A freshly measured row that fits stays hidden.
        let mut hidden = CarouselControlsFade::default();
        hidden.update(false, false, now, false);
        assert_eq!(hidden.opacity_at(now), 0.);
        assert!(!hidden.is_animating(now));
        assert!(!hidden.needs_render_sync());

        // Once the paint drives the state, render consumers follow it
        // instead of the caller's estimate.
        assert_eq!(shown.render_opacity(now, false), 1.);
        assert_eq!(hidden.render_opacity(now, true), 0.);
    }

    #[test]
    fn controls_fade_animates_the_overflow_flip_together() {
        let now = Instant::now();
        let mut fade = CarouselControlsFade::default();
        fade.update(false, false, now, false);
        assert_eq!(fade.opacity_at(now), 0.);

        // Becoming scrollable fades the whole unit in from hidden.
        fade.update(true, false, now, false);
        assert!(fade.is_animating(now));
        assert_eq!(fade.opacity_at(now), 0.);
        let midpoint = now + CAROUSEL_CONTROLS_FADE_DURATION / 2;
        assert!((fade.opacity_at(midpoint) - 0.5).abs() < 1e-3);
        let done = now + CAROUSEL_CONTROLS_FADE_DURATION;
        assert_eq!(fade.opacity_at(done), 1.);
        assert!(!fade.is_animating(done));
        // The settling frame still owes one render sync.
        fade.update(true, false, done, false);
        assert_eq!(fade.opacity_at(done), 1.);
        assert!(!fade.is_animating(done));
        assert!(fade.needs_render_sync());
        fade.update(true, false, done, false);
        assert!(!fade.needs_render_sync());

        // Stopping the overflow fades the unit out from shown, even while
        // the row is still being interacted with.
        fade.update(false, true, done, false);
        assert!(fade.is_animating(done));
        assert_eq!(fade.opacity_at(done), 1.);
        let out_mid = done + CAROUSEL_CONTROLS_FADE_DURATION / 2;
        assert!((fade.opacity_at(out_mid) - 0.5).abs() < 1e-3);
        let out_done = done + CAROUSEL_CONTROLS_FADE_DURATION;
        assert_eq!(fade.opacity_at(out_done), 0.);
    }

    #[test]
    fn controls_fade_snaps_under_reduced_motion() {
        let now = Instant::now();
        let mut fade = CarouselControlsFade::default();
        fade.update(false, false, now, false);

        fade.update(true, false, now, true);
        assert_eq!(fade.opacity_at(now), 1.);
        assert!(!fade.is_animating(now));
        assert!(fade.needs_render_sync());

        fade.update(false, false, now, true);
        assert_eq!(fade.opacity_at(now), 0.);
        assert!(!fade.is_animating(now));
    }

    #[test]
    fn controls_fade_composes_with_the_activity_window() {
        let now = Instant::now();
        let mut fade = CarouselControlsFade::default();
        fade.update(true, false, now, false);
        assert_eq!(fade.opacity_at(now), 1.);

        // Going idle past the hold window fades the unit out while the
        // row keeps overflowing.
        let idle = now + Duration::from_secs_f32(CARD_SCROLLBAR_FADE_OUT_DELAY + 0.5);
        fade.update(true, false, idle, false);
        let tail = fade.opacity_at(idle);
        assert!(tail > 0. && tail < 1.);

        // Scroll activity brings the whole unit back.
        fade.update(true, true, idle, false);
        assert_eq!(fade.opacity_at(idle), 1.);

        // The overflow gate wins over activity: losing the overflow fades
        // the unit out regardless.
        fade.update(false, true, idle, false);
        assert!(fade.is_animating(idle));
        let gone = idle + CAROUSEL_CONTROLS_FADE_DURATION;
        assert_eq!(fade.opacity_at(gone), 0.);
        // Fully hidden means the controls owe one render sync so the
        // arrows can drop to their inert shells.
        fade.update(false, false, gone, false);
        assert!(fade.needs_render_sync());
        assert_eq!(fade.opacity_at(gone), 0.);
    }

    #[test]
    fn controls_fade_duration_stays_in_the_polish_range() {
        assert!(CAROUSEL_CONTROLS_FADE_DURATION >= Duration::from_millis(150));
        assert!(CAROUSEL_CONTROLS_FADE_DURATION <= Duration::from_millis(250));
    }

    #[test]
    fn controls_fade_revives_from_fully_hidden_on_activity() {
        let now = Instant::now();
        let mut fade = CarouselControlsFade::default();
        fade.update(true, false, now, false);
        assert_eq!(fade.opacity_at(now), 1.);

        // Idling past the hold window and the whole fade tail hides the
        // unit while the row keeps overflowing.
        let hidden = now + Duration::from_secs_f32(CARD_SCROLLBAR_FADE_OUT_DURATION + 0.5);
        fade.update(true, false, hidden, false);
        assert_eq!(fade.opacity_at(hidden), 0.);

        // A hover or scroll on the still-overflowing row refreshes the
        // activity window and brings the whole unit back.
        fade.update(true, true, hidden, false);
        assert_eq!(fade.opacity_at(hidden), 1.);
        assert!(!fade.is_animating(hidden));

        // A row with nothing to scroll stays hidden no matter how active
        // the pointer is over its strip.
        let mut inert = CarouselControlsFade::default();
        inert.update(false, false, now, false);
        inert.update(false, true, now, false);
        assert_eq!(inert.opacity_at(now), 0.);
        assert!(!inert.is_animating(now));
    }

    #[test]
    fn scrollbar_track_stays_under_the_cards_across_reserve_locations() {
        // The strip's reserve can sit inside the scroll viewport or below
        // the carousel, but the track belongs to the card row: it must
        // land at the same place either way, so the fading thumb never
        // moves when the reserve flips.
        let reserved_canvas = gpui::Bounds {
            origin: gpui::point(gpui::px(24.), gpui::px(96.)),
            size: gpui::size(gpui::px(400.), gpui::px(214.)),
        };
        let unreserved_canvas = gpui::Bounds {
            origin: gpui::point(gpui::px(24.), gpui::px(96.)),
            size: gpui::size(gpui::px(400.), gpui::px(196.)),
        };
        let reserved = track_bounds(reserved_canvas, true);
        let unreserved = track_bounds(unreserved_canvas, false);
        assert_eq!(reserved.origin.y, unreserved.origin.y);
        assert_eq!(reserved.size, unreserved.size);
        // With the reserve inside the viewport, the track is the bottom
        // control lane of the canvas, matching the pinned layout.
        assert_eq!(
            f32::from(reserved.origin.y),
            96. + 214. - super::CAROUSEL_CONTROL_HEIGHT
        );
        assert_eq!(
            f32::from(reserved.size.height),
            super::CAROUSEL_CONTROL_HEIGHT
        );

        let handle = gpui::ScrollHandle::new();
        let geometry = scrollbar_geometry(reserved, gpui::px(376.), &handle);
        let thumb_centering = (super::CAROUSEL_CONTROL_HEIGHT - super::CAROUSEL_THUMB_HEIGHT) * 0.5;
        assert_eq!(
            f32::from(geometry.thumb_bounds.origin.y),
            96. + 214. - super::CAROUSEL_CONTROL_HEIGHT + thumb_centering
        );
        assert_eq!(
            f32::from(geometry.thumb_bounds.size.height),
            super::CAROUSEL_THUMB_HEIGHT
        );
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

        // A fresh overflowing row settles its controls shown right away.
        draw(cx, &view);
        let fade = state.controls_fade.get();
        assert!(fade.is_initialized());
        assert_eq!(fade.opacity_at(Instant::now()), 1.);

        // Scrolling counts as activity and refreshes the show window.
        let extent = f32::from(state.scroll_handle.max_offset().x);
        assert!(extent > 0.);
        state
            .scroll_handle
            .set_offset(gpui::point(gpui::px(-(extent - 20.)), gpui::px(0.)));
        draw(cx, &view);
        let fade = state.controls_fade.get();
        let elapsed = fade.idle_for(Instant::now()).expect("activity recorded");
        assert!(elapsed < CARD_SCROLLBAR_FADE_OUT_DELAY);
        assert_eq!(fade.opacity_at(Instant::now()), 1.);
    }

    #[gpui::test]
    fn controls_fade_follows_the_rendered_overflow_flip(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        // The host derives the caller's overflow estimate from the live
        // viewport width exactly like the real callers, so resizing the
        // window drives the whole flip.
        struct CarouselHost(super::CardCarouselState);
        impl gpui::Render for CarouselHost {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let width = f32::from(window.viewport_size().width);
                crate::music_ui::card_carousel(
                    "controls-fade-wiring",
                    self.0.clone(),
                    150.,
                    12.,
                    crate::music_ui::card_carousel_has_overflow(8, 150., 12., width, 0.),
                    gpui::div()
                        .flex()
                        .flex_none()
                        .children((0..8).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))))
                        .into_any_element(),
                )
            }
        }

        // Eight cards need 1284 pixels, so the row only overflows below
        // that width.
        let state = super::CardCarouselState::new();
        let window = cx.add_window({
            let state = state.clone();
            move |_, _| CarouselHost(state)
        });
        let cx = &mut gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        let resize = |cx: &mut gpui::VisualTestContext, width: f32| {
            cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(300.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        };
        let fade_opacity =
            |state: &super::CardCarouselState| state.controls_fade.get().opacity_at(Instant::now());
        let click_next_arrow = |cx: &mut gpui::VisualTestContext, width: f32| {
            cx.simulate_click(
                gpui::point(gpui::px(width - 5.), gpui::px(295.)),
                gpui::Modifiers::default(),
            );
        };

        // Wide enough to fit: whatever the opening size measured, resizing
        // wide settles the controls hidden.
        resize(cx, 1600.);
        std::thread::sleep(CAROUSEL_CONTROLS_FADE_DURATION + Duration::from_millis(50));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(fade_opacity(&state), 0.);
        assert!(!state.controls_fade.get().is_animating(Instant::now()));
        click_next_arrow(cx, 1600.);
        assert_eq!(state.pending_target.get(), None);

        // Resizing across the overflow boundary fades the controls in
        // instead of snapping them visible.
        resize(cx, 400.);
        assert!(state.controls_fade.get().is_animating(Instant::now()));
        assert!(fade_opacity(&state) < 0.5);

        // The fade progresses frame by frame and settles shown.
        std::thread::sleep(CAROUSEL_CONTROLS_FADE_DURATION + Duration::from_millis(50));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(fade_opacity(&state), 1.);
        assert!(!state.controls_fade.get().is_animating(Instant::now()));

        // While shown, the arrow is a real button: clicking it scrolls.
        state.pending_target.set(None);
        click_next_arrow(cx, 400.);
        assert_eq!(state.pending_target.get(), Some(-162.));

        // Resizing back across the boundary fades the controls out
        // instead of snapping them hidden.
        resize(cx, 1600.);
        assert!(state.controls_fade.get().is_animating(Instant::now()));
        assert!(fade_opacity(&state) > 0.5);

        // Once fully hidden, the arrow lane intercepts nothing again.
        std::thread::sleep(CAROUSEL_CONTROLS_FADE_DURATION + Duration::from_millis(50));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(fade_opacity(&state), 0.);
        state.pending_target.set(None);
        click_next_arrow(cx, 1600.);
        assert_eq!(state.pending_target.get(), None);
    }

    #[gpui::test]
    fn hovering_the_faded_strip_revives_the_carousel_controls(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        struct CarouselHost(super::CardCarouselState);
        impl gpui::Render for CarouselHost {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let width = f32::from(window.viewport_size().width);
                crate::music_ui::card_carousel(
                    "scrollbar-hover-revival",
                    self.0.clone(),
                    150.,
                    12.,
                    crate::music_ui::card_carousel_has_overflow(8, 150., 12., width, 0.),
                    gpui::div()
                        .flex()
                        .flex_none()
                        .children((0..8).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))))
                        .into_any_element(),
                )
            }
        }

        let state = super::CardCarouselState::new();
        let window = cx.add_window({
            let state = state.clone();
            move |_, _| CarouselHost(state)
        });
        let cx = &mut gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        };
        let fade_opacity =
            |state: &super::CardCarouselState| state.controls_fade.get().opacity_at(Instant::now());

        cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(300.)));
        cx.run_until_parked();
        std::thread::sleep(CAROUSEL_CONTROLS_FADE_DURATION + Duration::from_millis(50));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        assert_eq!(fade_opacity(&state), 1.);

        // Waiting out the hold window and the whole fade tail hides the
        // controls while the row keeps overflowing.
        std::thread::sleep(Duration::from_secs_f32(
            CARD_SCROLLBAR_FADE_OUT_DURATION + 0.5,
        ));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        assert_eq!(fade_opacity(&state), 0.);

        // Hovering the faded scrollbar strip must bring the whole unit
        // back: the strip still hit-tests while invisible.
        cx.simulate_mouse_move(
            gpui::point(
                gpui::px(200.),
                gpui::px(300. - super::CAROUSEL_CONTROL_HEIGHT * 0.5),
            ),
            None,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(fade_opacity(&state), 1.);
    }

    #[gpui::test]
    fn hovering_the_strip_cannot_revive_controls_without_overflow(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        struct CarouselHost(super::CardCarouselState);
        impl gpui::Render for CarouselHost {
            fn render(
                &mut self,
                window: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let width = f32::from(window.viewport_size().width);
                crate::music_ui::card_carousel(
                    "scrollbar-no-overflow-hover",
                    self.0.clone(),
                    150.,
                    12.,
                    crate::music_ui::card_carousel_has_overflow(2, 150., 12., width, 0.),
                    gpui::div()
                        .flex()
                        .flex_none()
                        .children((0..2).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))))
                        .into_any_element(),
                )
            }
        }

        let state = super::CardCarouselState::new();
        let window = cx.add_window({
            let state = state.clone();
            move |_, _| CarouselHost(state)
        });
        let cx = &mut gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        };
        let fade_opacity =
            |state: &super::CardCarouselState| state.controls_fade.get().opacity_at(Instant::now());

        // Two cards fit: the controls stay hidden no matter what hovers
        // over the strip lane.
        cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(300.)));
        cx.run_until_parked();
        draw(cx);
        assert_eq!(fade_opacity(&state), 0.);
        assert!(state.controls_fade.get().is_initialized());

        cx.simulate_mouse_move(
            gpui::point(
                gpui::px(200.),
                gpui::px(300. - super::CAROUSEL_CONTROL_HEIGHT * 0.5),
            ),
            None,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(fade_opacity(&state), 0.);
    }

    #[gpui::test]
    fn the_control_strip_stays_below_the_cards_when_the_reserve_sits_outside(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        // The wrapper shrink-wraps the carousel so its height is the card
        // row plus the reserve, exactly like the stacked sections that
        // host carousels in the app. The caller's estimate says the strip
        // is not reserved inside the viewport, so the strip must anchor to
        // the card row and land in the space below the carousel rather
        // than sliding up into the cards.
        struct CarouselHost(super::CardCarouselState);
        impl gpui::Render for CarouselHost {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div().flex().flex_col().child(
                    gpui::div()
                        .flex_none()
                        .child(crate::music_ui::card_carousel(
                            "anchored-strip",
                            self.0.clone(),
                            150.,
                            12.,
                            false,
                            gpui::div()
                                .flex()
                                .flex_none()
                                .children(
                                    (0..8).map(|_| gpui::div().w(gpui::px(150.)).h(gpui::px(196.))),
                                )
                                .into_any_element(),
                        )),
                )
            }
        }

        let state = super::CardCarouselState::new();
        let window = cx.add_window({
            let state = state.clone();
            move |_, _| CarouselHost(state)
        });
        let cx = &mut gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                _ = window.draw(cx);
            });
        };
        let fade_opacity =
            |state: &super::CardCarouselState| state.controls_fade.get().opacity_at(Instant::now());
        let card_row_bottom = 196.;
        let old_lane_y = card_row_bottom - super::CAROUSEL_CONTROL_HEIGHT;
        let anchored_lane_y = card_row_bottom + super::CAROUSEL_CARD_ROW_BOTTOM_GAP;

        cx.simulate_resize(gpui::size(gpui::px(400.), gpui::px(300.)));
        cx.run_until_parked();
        std::thread::sleep(CAROUSEL_CONTROLS_FADE_DURATION + Duration::from_millis(50));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        assert_eq!(fade_opacity(&state), 1.);

        // Hovering where a bottom-anchored track would sit, up inside the
        // cards, must not hold the controls: the strip no longer lives
        // there once the reserve is not inside the viewport.
        cx.simulate_mouse_move(
            gpui::point(gpui::px(200.), gpui::px(old_lane_y + 5.)),
            None,
            gpui::Modifiers::default(),
        );
        std::thread::sleep(Duration::from_secs_f32(
            CARD_SCROLLBAR_FADE_OUT_DURATION + 0.5,
        ));
        cx.update(|window, cx| window.simulate_next_frame(cx));
        draw(cx);
        assert_eq!(
            fade_opacity(&state),
            0.,
            "the strip must not sit inside the cards"
        );

        // Hovering the lane below the card row brings the controls back,
        // proving the strip anchors to the card row across reserve
        // locations.
        cx.simulate_mouse_move(
            gpui::point(gpui::px(200.), gpui::px(anchored_lane_y + 5.)),
            None,
            gpui::Modifiers::default(),
        );
        draw(cx);
        assert_eq!(
            fade_opacity(&state),
            1.,
            "the strip must sit below the card row"
        );
    }
}
