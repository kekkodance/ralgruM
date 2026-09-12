use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, CursorStyle, ElementId, Hitbox, HitboxBehavior, ListOffset, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, Size, Window, canvas, div, point, prelude::*, px, size,
};
use gpui_component::scroll::ScrollbarHandle;

use crate::browser_scroll_cursor::{
    BrowserScrollCursor, restore_browser_scroll_cursor, set_browser_scroll_cursor,
    take_browser_scroll_cursor_cancellation,
};

/// The vertical scroll target owned by a browser-like surface.
///
/// `ScrollHandle` is used by ordinary overflow containers, while `ListState`
/// is used by virtualized lists. Keeping both behind one target lets input
/// behavior remain identical across the two GPUI scrolling implementations.
#[derive(Clone)]
pub(crate) enum BrowserScrollTarget {
    Handle(ScrollHandle),
    List(ListState),
    FixedList(FixedListScrollHandle),
}

/// Stable pixel geometry for a GPUI list whose rows have a known fixed height.
/// GPUI discards list size hints after a width change, so using the raw
/// `ListState` for scrollbar math can temporarily make unmeasured rows count as
/// zero-height. This adapter keeps scrollbar and wheel bounds exact while the
/// list remains lazily rendered.
#[derive(Clone, Debug)]
pub(crate) struct FixedListScrollHandle {
    state: ListState,
    item_count: usize,
    item_height: Pixels,
}

impl FixedListScrollHandle {
    pub(crate) fn new(state: ListState, item_count: usize, item_height: Pixels) -> Self {
        Self {
            state,
            item_count,
            item_height: px(f32::from(item_height).max(1.)),
        }
    }

    fn viewport_size(&self) -> Size<Pixels> {
        self.state.viewport_bounds().size
    }

    fn content_height(&self) -> Pixels {
        px(self.item_count as f32 * f32::from(self.item_height))
    }

    fn position(&self) -> f32 {
        let offset = self.state.logical_scroll_top();
        (offset.item_ix.min(self.item_count) as f32 * f32::from(self.item_height)
            + f32::from(offset.offset_in_item).max(0.))
        .clamp(0., self.maximum())
    }

    fn maximum(&self) -> f32 {
        (f32::from(self.content_height()) - f32::from(self.viewport_size().height)).max(0.)
    }

    fn set_position(&self, position: f32) {
        let position = position.clamp(0., self.maximum());
        let item_height = f32::from(self.item_height);
        let item_ix = ((position / item_height).floor() as usize).min(self.item_count);
        let offset_in_item = if item_ix < self.item_count {
            px(position - item_ix as f32 * item_height)
        } else {
            px(0.)
        };
        self.state.scroll_to(ListOffset {
            item_ix,
            offset_in_item,
        });
    }

    pub(crate) fn scroll_offset(&self) -> Point<Pixels> {
        point(px(0.), px(-self.position()))
    }

    pub(crate) fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.set_position(-f32::from(offset.y));
    }
}

impl ScrollbarHandle for FixedListScrollHandle {
    fn offset(&self) -> Point<Pixels> {
        self.scroll_offset()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set_scroll_offset(offset);
    }

    fn content_size(&self) -> Size<Pixels> {
        size(self.viewport_size().width, self.content_height())
    }

    fn start_drag(&self) {
        self.state.scrollbar_drag_started();
    }

    fn end_drag(&self) {
        self.state.scrollbar_drag_ended();
    }
}

/// State that survives element-tree rebuilds for one scroll surface.
#[derive(Clone, Default)]
pub(crate) struct BrowserScrollState {
    inner: Rc<RefCell<BrowserScrollStateInner>>,
}

#[derive(Default)]
struct BrowserScrollStateInner {
    wheel_target: Option<f32>,
    wheel_velocity_px_per_second: f32,
    wheel_last_frame: Option<Instant>,
    wheel_last_input: Option<Instant>,
    user_input_generation: u64,
    frame_scheduled: bool,
    autoscroll: Option<AutoscrollState>,
    generation: u64,
    scrolls_while_menu_open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct AutoscrollState {
    anchor_y: f32,
    pointer_y: f32,
}

/// The distance around the middle-click anchor where autoscroll remains still.
pub(crate) const AUTOSCROLL_DEAD_ZONE_PX: f32 = 15.;
/// Safety cap for the converted per-frame autoscroll velocity.
///
/// Chromium's velocity is unbounded in theory. A cap prevents a malformed or
/// very distant pointer position from producing an oversized one-frame jump,
/// while remaining above the velocity reached across a typical window.
pub(crate) const AUTOSCROLL_MAX_SPEED_PX_PER_FRAME: f32 = 640.;
pub(crate) const WHEEL_LINE_HEIGHT_PX: f32 = 40.0;
const AUTOSCROLL_EXPONENT: f32 = 2.2;
const AUTOSCROLL_SPEED_PER_MILLISECOND: f32 = 0.000008;
const AUTOSCROLL_FRAME_DURATION_MILLISECONDS: f32 = 16.6667;
const SMOOTH_SCROLL_RESPONSE_SECONDS: f32 = 0.045;
const SMOOTH_SCROLL_MAX_FRAME_SECONDS: f32 = 0.05;
const SMOOTH_SCROLL_POSITION_EPSILON_PX: f32 = 0.35;
const SMOOTH_SCROLL_VELOCITY_EPSILON_PX_PER_SECOND: f32 = 2.0;
const MAX_POST_INPUT_TAIL: Duration = Duration::from_millis(120);

fn wheel_pixel_delta(delta: ScrollDelta) -> Point<Pixels> {
    match delta {
        ScrollDelta::Pixels(delta) => delta,
        ScrollDelta::Lines(delta) => point(
            px(delta.x * WHEEL_LINE_HEIGHT_PX),
            px(delta.y * WHEEL_LINE_HEIGHT_PX),
        ),
    }
}

/// Return the autoscroll displacement for one animation frame.
///
/// The dead zone prevents small hand movement from producing jitter. Once the
/// pointer leaves it, this follows Chromium's power-curve velocity and scales
/// its millisecond-based value to one 60 Hz frame. The dead-zone radius is not
/// subtracted from the distance, matching Chromium's behavior.
pub(crate) fn autoscroll_speed(pointer_y: f32, anchor_y: f32) -> f32 {
    let distance = pointer_y - anchor_y;
    if !distance.is_finite() || distance.abs() <= AUTOSCROLL_DEAD_ZONE_PX {
        return 0.;
    }
    let speed = distance.abs().powf(AUTOSCROLL_EXPONENT)
        * AUTOSCROLL_SPEED_PER_MILLISECOND
        * AUTOSCROLL_FRAME_DURATION_MILLISECONDS;
    distance.signum() * speed.min(AUTOSCROLL_MAX_SPEED_PX_PER_FRAME)
}

/// The direction represented by the browser-style middle-click autoscroll
/// cursor.
///
/// GPUI exposes directional resize cursors rather than Chromium's dedicated
/// panning cursors. Keeping this direction separate from the GPUI mapping lets
/// the native cursor bridge use the same pure state calculation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutoscrollDirection {
    Up,
    Neutral,
    Down,
}

/// Choose the autoscroll cursor direction for the pointer's position relative
/// to its middle-click anchor.
pub(crate) fn autoscroll_direction(pointer_y: f32, anchor_y: f32) -> AutoscrollDirection {
    let distance = pointer_y - anchor_y;
    if distance < -AUTOSCROLL_DEAD_ZONE_PX {
        AutoscrollDirection::Up
    } else if distance > AUTOSCROLL_DEAD_ZONE_PX {
        AutoscrollDirection::Down
    } else {
        AutoscrollDirection::Neutral
    }
}

/// Map an autoscroll direction to the closest built-in GPUI cursor.
pub(crate) const fn autoscroll_cursor_style(direction: AutoscrollDirection) -> CursorStyle {
    match direction {
        AutoscrollDirection::Up => CursorStyle::ResizeUp,
        AutoscrollDirection::Neutral => CursorStyle::ResizeUpDown,
        AutoscrollDirection::Down => CursorStyle::ResizeDown,
    }
}

/// Map the pointer position directly to the closest built-in GPUI cursor.
pub(crate) fn autoscroll_cursor(pointer_y: f32, anchor_y: f32) -> CursorStyle {
    autoscroll_cursor_style(autoscroll_direction(pointer_y, anchor_y))
}

fn smooth_scroll_substep(
    current: f32,
    target: f32,
    velocity: f32,
    delta_seconds: f32,
) -> (f32, f32) {
    if current == target {
        return (target, 0.);
    }

    let omega = 2. / SMOOTH_SCROLL_RESPONSE_SECONDS;
    let x = omega * delta_seconds;
    let decay = 1. / (1. + x + 0.48 * x * x + 0.235 * x * x * x);
    let change = current - target;
    let temp = (velocity + omega * change) * delta_seconds;
    let next_velocity = (velocity - omega * temp) * decay;
    let next = target + (change + temp) * decay;

    if !next.is_finite() || !next_velocity.is_finite() {
        return (target, 0.);
    }

    let crossed_target = if target > current {
        next > target
    } else {
        next < target
    };
    if crossed_target || next == target {
        (target, 0.)
    } else {
        (next, next_velocity)
    }
}

/// Advance a queued scroll position with a stable critically damped spring.
pub(crate) fn smooth_scroll_step(
    current: f32,
    target: f32,
    velocity: f32,
    delta_seconds: f32,
) -> (f32, f32) {
    if !current.is_finite()
        || !target.is_finite()
        || !velocity.is_finite()
        || !delta_seconds.is_finite()
    {
        let safe_target = if target.is_finite() {
            target
        } else if current.is_finite() {
            current
        } else {
            0.
        };
        return (safe_target, 0.);
    }

    let mut remaining_seconds = delta_seconds.max(0.);
    let mut next_position = current;
    let mut next_velocity = velocity;
    while remaining_seconds > 0. {
        let substep_seconds = remaining_seconds.min(SMOOTH_SCROLL_MAX_FRAME_SECONDS);
        (next_position, next_velocity) =
            smooth_scroll_substep(next_position, target, next_velocity, substep_seconds);
        if next_position == target && next_velocity == 0. {
            break;
        }
        let next_remaining_seconds = remaining_seconds - substep_seconds;
        if next_remaining_seconds == remaining_seconds {
            return (target, 0.);
        }
        remaining_seconds = next_remaining_seconds;
    }
    (next_position, next_velocity)
}

fn wheel_target_after_delta(
    current: f32,
    pending_target: Option<f32>,
    delta_y: f32,
    maximum: f32,
) -> f32 {
    let maximum = if maximum.is_finite() {
        maximum.max(0.)
    } else {
        0.
    };
    let delta_y = if delta_y.is_finite() { delta_y } else { 0. };
    let base = match pending_target {
        Some(pending)
            if pending.is_finite()
                && delta_y != 0.
                && pending != current
                && delta_y.signum() != (pending - current).signum() =>
        {
            current
        }
        Some(pending) if pending.is_finite() => pending,
        _ => current,
    };
    (base + delta_y).clamp(0., maximum)
}

fn queue_wheel_state(
    inner: &mut BrowserScrollStateInner,
    current: f32,
    maximum: f32,
    delta_y: f32,
    now: Instant,
) {
    let was_idle = inner.wheel_target.is_none();
    let is_reversal = inner.wheel_target.is_some_and(|pending_target| {
        delta_y != 0.
            && pending_target.is_finite()
            && pending_target != current
            && delta_y.signum() != (pending_target - current).signum()
    });
    inner.wheel_target = Some(wheel_target_after_delta(
        current,
        inner.wheel_target,
        delta_y,
        maximum,
    ));
    inner.wheel_last_input = Some(now);
    if was_idle || is_reversal {
        inner.wheel_velocity_px_per_second = 0.;
        inner.wheel_last_frame = Some(now);
    }
}

fn clamp_and_settle_wheel_step(
    next_position: f32,
    next_velocity: f32,
    target: f32,
    maximum: f32,
) -> (f32, f32, bool) {
    let maximum = if maximum.is_finite() {
        maximum.max(0.)
    } else {
        0.
    };
    let target = if target.is_finite() {
        target.clamp(0., maximum)
    } else {
        0.
    };
    let next_position = if next_position.is_finite() {
        next_position.clamp(0., maximum)
    } else {
        target
    };
    let next_velocity = if next_velocity.is_finite() {
        next_velocity
    } else {
        0.
    };
    let at_target = (target - next_position).abs() <= SMOOTH_SCROLL_POSITION_EPSILON_PX
        && next_velocity.abs() <= SMOOTH_SCROLL_VELOCITY_EPSILON_PX_PER_SECOND;
    let at_target_boundary =
        (target == 0. && next_position == 0.) || (target == maximum && next_position == maximum);
    if at_target || at_target_boundary {
        (target, 0., true)
    } else {
        (next_position, next_velocity, false)
    }
}

fn advance_wheel_motion(
    inner: &mut BrowserScrollStateInner,
    current: f32,
    maximum: f32,
    now: Instant,
) -> (Option<f32>, bool) {
    if !current.is_finite() || !maximum.is_finite() {
        inner.clear_wheel_motion();
        return (None, false);
    }

    let maximum = maximum.max(0.);
    let Some(target_position) = inner.wheel_target else {
        inner.clear_wheel_motion();
        return (None, false);
    };
    if !target_position.is_finite() || !inner.wheel_velocity_px_per_second.is_finite() {
        inner.clear_wheel_motion();
        return (None, false);
    }

    let target_position = target_position.clamp(0., maximum);
    if inner
        .wheel_last_input
        .is_some_and(|last_input| now.saturating_duration_since(last_input) >= MAX_POST_INPUT_TAIL)
    {
        inner.clear_wheel_motion();
        return (Some(target_position), false);
    }
    let delta_seconds = inner
        .wheel_last_frame
        .map(|last| now.saturating_duration_since(last).as_secs_f32())
        .unwrap_or_default();
    inner.wheel_last_frame = Some(now);
    let (next_position, next_velocity) = smooth_scroll_step(
        current,
        target_position,
        inner.wheel_velocity_px_per_second,
        delta_seconds,
    );
    let (next_position, next_velocity, settled) =
        clamp_and_settle_wheel_step(next_position, next_velocity, target_position, maximum);
    if settled {
        inner.clear_wheel_motion();
        ((next_position != current).then_some(next_position), false)
    } else {
        inner.wheel_velocity_px_per_second = next_velocity;
        ((next_position != current).then_some(next_position), true)
    }
}

/// Whether a wheel event should be handled by a vertical surface.
///
/// When horizontal movement dominates, the event is left to a nested
/// carousel. This is important for diagonal trackpad gestures over card rows.
pub(crate) fn is_vertical_scroll(delta: gpui::Point<gpui::Pixels>) -> bool {
    let x = f32::from(delta.x);
    let y = f32::from(delta.y);
    y != 0. && (x == 0. || y.abs() >= x.abs())
}

impl BrowserScrollState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn for_context_menu() -> Self {
        let state = Self::new();
        state.inner.borrow_mut().scrolls_while_menu_open = true;
        state
    }

    fn scrolls_while_menu_open(&self) -> bool {
        self.inner.borrow().scrolls_while_menu_open
    }

    #[cfg(test)]
    fn is_autoscrolling(&self) -> bool {
        self.inner.borrow().autoscroll.is_some()
    }

    fn autoscroll_cursor(&self) -> Option<CursorStyle> {
        self.inner
            .borrow()
            .autoscroll
            .map(|state| autoscroll_cursor(state.pointer_y, state.anchor_y))
    }

    fn cursor_owner(&self) -> usize {
        Rc::as_ptr(&self.inner) as usize
    }

    pub(crate) fn reset(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.clear_wheel_motion();
        inner.autoscroll = None;
        inner.frame_scheduled = false;
        inner.generation = inner.generation.wrapping_add(1);
    }

    pub(crate) fn user_input_generation(&self) -> u64 {
        self.inner.borrow().user_input_generation
    }

    fn begin_autoscroll(&self, anchor_y: f32) -> bool {
        let mut inner = self.inner.borrow_mut();
        let newly_active = inner.autoscroll.is_none();
        inner.autoscroll = Some(AutoscrollState {
            anchor_y,
            pointer_y: anchor_y,
        });
        inner.clear_wheel_motion();
        if newly_active {
            inner.user_input_generation = inner.user_input_generation.wrapping_add(1);
        }
        newly_active
    }

    fn cancel_autoscroll(&self) -> bool {
        let mut inner = self.inner.borrow_mut();
        let was_active = inner.autoscroll.is_some();
        inner.autoscroll = None;
        was_active
    }

    fn update_pointer(&self, pointer_y: f32, pressed_button: Option<MouseButton>) -> bool {
        let mut inner = self.inner.borrow_mut();
        if pressed_button != Some(MouseButton::Middle) {
            return inner.autoscroll.take().is_some();
        }
        let Some(autoscroll) = inner.autoscroll.as_mut() else {
            return false;
        };
        autoscroll.pointer_y = pointer_y;
        true
    }

    fn queue_wheel(&self, target: &BrowserScrollTarget, delta_y: f32, now: Instant) -> bool {
        let current = target.position();
        let maximum = target.maximum();
        let mut inner = self.inner.borrow_mut();
        queue_wheel_state(&mut inner, current, maximum, delta_y, now);
        inner.user_input_generation = inner.user_input_generation.wrapping_add(1);
        !inner.frame_scheduled
    }

    fn apply_precise_wheel(&self, target: &BrowserScrollTarget, delta_y: f32) {
        let mut inner = self.inner.borrow_mut();
        inner.clear_wheel_motion();
        inner.user_input_generation = inner.user_input_generation.wrapping_add(1);
        drop(inner);
        target.set_position(target.position() + delta_y);
    }

    fn schedule_if_needed(&self, target: BrowserScrollTarget, window: &Window) {
        let should_schedule = {
            let mut inner = self.inner.borrow_mut();
            if inner.frame_scheduled {
                false
            } else {
                inner.frame_scheduled = true;
                true
            }
        };
        if !should_schedule {
            return;
        }
        let state = self.clone();
        let generation = self.inner.borrow().generation;
        window.on_next_frame(move |window, _cx| {
            if state.inner.borrow().generation != generation {
                return;
            }
            let (next_position, keep_running) = state.tick(&target, Instant::now());
            if let Some(next_position) = next_position {
                target.set_position(next_position);
                window.refresh();
            }
            {
                let mut inner = state.inner.borrow_mut();
                inner.frame_scheduled = false;
            }
            if keep_running {
                state.schedule_if_needed(target, window);
            }
        });
    }

    fn tick(&self, target: &BrowserScrollTarget, now: Instant) -> (Option<f32>, bool) {
        let current = target.position();
        let maximum = target.maximum();
        let mut inner = self.inner.borrow_mut();

        if let Some(autoscroll) = inner.autoscroll {
            inner.wheel_target = None;
            let next = (current + autoscroll_speed(autoscroll.pointer_y, autoscroll.anchor_y))
                .clamp(0., maximum);
            return ((next != current).then_some(next), true);
        }

        advance_wheel_motion(&mut inner, current, maximum, now)
    }
}

impl BrowserScrollStateInner {
    fn clear_wheel_motion(&mut self) {
        self.wheel_target = None;
        self.wheel_velocity_px_per_second = 0.;
        self.wheel_last_frame = None;
        self.wheel_last_input = None;
    }
}

impl BrowserScrollTarget {
    fn position(&self) -> f32 {
        match self {
            Self::Handle(handle) => -f32::from(handle.offset().y),
            Self::List(list) => -f32::from(list.scroll_px_offset_for_scrollbar().y),
            Self::FixedList(list) => list.position(),
        }
        .max(0.)
    }

    fn maximum(&self) -> f32 {
        match self {
            Self::Handle(handle) => f32::from(handle.max_offset().y).abs(),
            Self::List(list) => f32::from(list.max_offset_for_scrollbar().y).abs(),
            Self::FixedList(list) => list.maximum(),
        }
    }

    fn set_position(&self, position: f32) {
        let position = position.clamp(0., self.maximum());
        match self {
            Self::Handle(handle) => {
                let offset = handle.offset();
                handle.set_offset(point(offset.x, px(-position)));
            }
            Self::List(list) => {
                let current = self.position();
                let delta = position - current;
                if delta != 0. {
                    list.scroll_by(px(delta));
                }
            }
            Self::FixedList(list) => list.set_position(position),
        }
    }
}

fn browser_scroll_event_layer(
    target: BrowserScrollTarget,
    state: BrowserScrollState,
) -> AnyElement {
    let event_target = target.clone();
    let event_state = state.clone();
    canvas(
        move |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_bounds, hitbox, window, _cx| {
            register_handlers(hitbox, event_target, event_state, window);
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn browser_scroll_cursor_layer(state: BrowserScrollState) -> AnyElement {
    let cursor_state = state.clone();
    canvas(
        move |bounds, window, _cx| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_bounds, _hitbox: Hitbox, window, _cx| {
            if let Some(cursor) = cursor_state.autoscroll_cursor() {
                set_browser_scroll_cursor(
                    window,
                    BrowserScrollCursor::from_gpui_style(cursor),
                    cursor_state.cursor_owner(),
                );
            } else {
                set_browser_scroll_cursor(
                    window,
                    BrowserScrollCursor::Reset,
                    cursor_state.cursor_owner(),
                );
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// Overlay hitboxes that apply wheel smoothing without wrapping the scrollport
/// in another flex container. Context menus use this so max_h still creates
/// a real overflow range.
pub(crate) fn browser_scroll_overlays(
    target: BrowserScrollTarget,
    state: BrowserScrollState,
) -> [AnyElement; 2] {
    [
        browser_scroll_event_layer(target, state.clone()),
        browser_scroll_cursor_layer(state),
    ]
}

/// Wrap a scrollable element with reusable wheel smoothing and middle-click
/// autoscroll handlers. The event layer is intentionally painted first so
/// nested bubble-phase handlers, such as carousel middle drags, run first.
pub(crate) fn browser_scroll_surface(
    id: impl Into<ElementId>,
    content: AnyElement,
    target: BrowserScrollTarget,
    state: BrowserScrollState,
) -> AnyElement {
    let [event_layer, cursor_layer] = browser_scroll_overlays(target, state);
    div()
        .id(id)
        .relative()
        .w_full()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(event_layer)
        .child(content)
        .child(cursor_layer)
        .into_any_element()
}

fn register_handlers(
    hitbox: Hitbox,
    target: BrowserScrollTarget,
    state: BrowserScrollState,
    window: &mut Window,
) {
    if take_browser_scroll_cursor_cancellation(state.cursor_owner()) {
        let was_active = state.cancel_autoscroll();
        window.release_pointer();
        restore_browser_scroll_cursor(window);
        if was_active {
            window.refresh();
        }
    }
    if !window.is_window_active() && cancel_autoscroll_and_release(&state, window) {
        window.refresh();
    }

    let wheel_hitbox = hitbox.clone();
    let wheel_target = target.clone();
    let wheel_state = state.clone();
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        if crate::context_menu::context_menu_is_open() && !wheel_state.scrolls_while_menu_open() {
            cancel_autoscroll_and_release(&wheel_state, window);
            if wheel_hitbox.should_handle_scroll(window) {
                cx.stop_propagation();
            }
            return;
        }
        if !wheel_hitbox.should_handle_scroll(window) {
            return;
        }
        let delta = wheel_pixel_delta(event.delta);
        if !is_vertical_scroll(delta) || wheel_target.maximum() <= 0. {
            return;
        }

        cancel_autoscroll_and_release(&wheel_state, window);
        let delta_y = -f32::from(delta.y);
        match event.delta {
            ScrollDelta::Lines(_) if cx.reduce_motion() => {
                wheel_state.apply_precise_wheel(&wheel_target, delta_y);
            }
            ScrollDelta::Lines(_) => {
                if wheel_state.queue_wheel(&wheel_target, delta_y, Instant::now()) {
                    wheel_state.schedule_if_needed(wheel_target.clone(), window);
                }
            }
            ScrollDelta::Pixels(_) => wheel_state.apply_precise_wheel(&wheel_target, delta_y),
        }
        window.refresh();
        cx.stop_propagation();
    });

    let down_hitbox = hitbox.clone();
    let down_state = state.clone();
    let down_target = target.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        if event.button == MouseButton::Middle
            && down_hitbox.is_hovered(window)
            && down_target.maximum() > 0.
        {
            let activated = down_state.begin_autoscroll(f32::from(event.position.y));
            if activated {
                window.capture_pointer(down_hitbox.id);
                down_state.schedule_if_needed(down_target.clone(), window);
            }
            window.prevent_default();
            window.refresh();
            cx.stop_propagation();
        } else if event.button != MouseButton::Middle
            && cancel_autoscroll_and_release(&down_state, window)
        {
            window.refresh();
            cx.stop_propagation();
        }
    });

    let move_state = state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
        if !phase.bubble() {
            return;
        }
        if !window.is_window_active() {
            if cancel_autoscroll_and_release(&move_state, window) {
                window.refresh();
            }
            return;
        }
        if move_state.update_pointer(f32::from(event.position.y), event.pressed_button) {
            if event.pressed_button != Some(MouseButton::Middle) {
                window.release_pointer();
            }
            window.refresh();
        }
    });

    let up_state = state.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if phase.bubble()
            && event.button == MouseButton::Middle
            && cancel_autoscroll_and_release(&up_state, window)
        {
            window.prevent_default();
            window.refresh();
            cx.stop_propagation();
        }
    });
}

fn cancel_autoscroll_and_release(state: &BrowserScrollState, window: &mut Window) -> bool {
    let was_active = state.cancel_autoscroll();
    if was_active {
        window.release_pointer();
        set_browser_scroll_cursor(window, BrowserScrollCursor::Reset, state.cursor_owner());
    }
    was_active
}

#[cfg(test)]
mod tests {
    use super::{
        AUTOSCROLL_DEAD_ZONE_PX, AUTOSCROLL_MAX_SPEED_PX_PER_FRAME, AutoscrollDirection,
        BrowserScrollState, BrowserScrollStateInner, FixedListScrollHandle, advance_wheel_motion,
        autoscroll_cursor, autoscroll_direction, autoscroll_speed, clamp_and_settle_wheel_step,
        is_vertical_scroll, queue_wheel_state, smooth_scroll_step, wheel_pixel_delta,
    };
    use gpui::{CursorStyle, ListAlignment, ListState, MouseButton, ScrollDelta, point, px};
    use std::time::{Duration, Instant};

    #[test]
    fn fixed_list_extent_survives_discarded_gpui_height_hints() {
        let state =
            ListState::new(100, ListAlignment::Top, px(0.)).with_uniform_item_height(px(58.));
        let fixed = FixedListScrollHandle::new(state.clone(), 100, px(58.));

        state.reset(100);

        assert_eq!(f32::from(state.max_offset_for_scrollbar().y), 0.);
        assert_eq!(fixed.maximum(), 5800.);
        fixed.set_position(5750.);
        assert_eq!(state.logical_scroll_top().item_ix, 99);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 8.);
        assert_eq!(fixed.position(), 5750.);
    }

    #[test]
    fn autoscroll_cursor_direction_tracks_the_dead_zone() {
        let anchor = 100.;
        assert_eq!(
            autoscroll_direction(anchor - AUTOSCROLL_DEAD_ZONE_PX, anchor),
            AutoscrollDirection::Neutral
        );
        assert_eq!(
            autoscroll_direction(anchor + AUTOSCROLL_DEAD_ZONE_PX, anchor),
            AutoscrollDirection::Neutral
        );
        assert_eq!(
            autoscroll_direction(anchor - AUTOSCROLL_DEAD_ZONE_PX - 1., anchor),
            AutoscrollDirection::Up
        );
        assert_eq!(
            autoscroll_direction(anchor + AUTOSCROLL_DEAD_ZONE_PX + 1., anchor),
            AutoscrollDirection::Down
        );
    }

    #[test]
    fn autoscroll_cursor_uses_directional_gpui_cursors() {
        let anchor = 100.;
        assert_eq!(
            autoscroll_cursor(anchor - AUTOSCROLL_DEAD_ZONE_PX - 1., anchor),
            CursorStyle::ResizeUp
        );
        assert_eq!(autoscroll_cursor(anchor, anchor), CursorStyle::ResizeUpDown);
        assert_eq!(
            autoscroll_cursor(anchor + AUTOSCROLL_DEAD_ZONE_PX + 1., anchor),
            CursorStyle::ResizeDown
        );
    }

    #[test]
    fn middle_autoscroll_is_hold_only_and_lost_buttons_cancel_it() {
        let state = BrowserScrollState::new();
        assert!(state.begin_autoscroll(100.));
        assert!(state.is_autoscrolling());
        assert!(!state.begin_autoscroll(100.));

        assert!(state.update_pointer(140., Some(MouseButton::Middle)));
        assert!(state.is_autoscrolling());
        assert!(state.update_pointer(140., None));
        assert!(!state.is_autoscrolling());
        assert!(!state.update_pointer(140., None));
    }

    #[test]
    fn reset_cancels_middle_autoscroll() {
        let state = BrowserScrollState::new();
        assert!(state.begin_autoscroll(100.));
        state.reset();
        assert!(!state.is_autoscrolling());
    }

    #[test]
    fn manual_input_generation_changes_for_wheel_and_middle_scroll() {
        let state = BrowserScrollState::new();
        let target = super::BrowserScrollTarget::Handle(gpui::ScrollHandle::new());
        let initial = state.user_input_generation();

        state.apply_precise_wheel(&target, 20.);
        assert_eq!(state.user_input_generation(), initial.wrapping_add(1));

        assert!(state.begin_autoscroll(100.));
        assert_eq!(state.user_input_generation(), initial.wrapping_add(2));
    }

    #[test]
    fn autoscroll_has_a_dead_zone_and_signed_speed() {
        assert_eq!(autoscroll_speed(100., 100.), 0.);
        assert_eq!(autoscroll_speed(100. + AUTOSCROLL_DEAD_ZONE_PX, 100.), 0.);
        assert!(autoscroll_speed(140., 100.) > 0.);
        assert!(autoscroll_speed(60., 100.) < 0.);

        let expected = 100_f32.powf(2.2) * 0.000008 * 16.6667;
        assert!((autoscroll_speed(200., 100.) - expected).abs() < 0.0001);
        assert!((autoscroll_speed(0., 100.) + expected).abs() < 0.0001);
    }

    #[test]
    fn autoscroll_speed_is_capped() {
        assert_eq!(
            autoscroll_speed(f32::MAX, 0.),
            AUTOSCROLL_MAX_SPEED_PX_PER_FRAME
        );
        assert_eq!(
            autoscroll_speed(f32::MIN, 0.),
            -AUTOSCROLL_MAX_SPEED_PX_PER_FRAME
        );
    }

    #[test]
    fn isolated_wheel_tick_advances_smoothly_and_settles_without_a_tail() {
        let mut current = 0.;
        let mut velocity = 0.;
        let mut settled = false;
        let mut frames = 0;
        for _ in 0..120 {
            let before = current;
            let (next, next_velocity) = smooth_scroll_step(current, 120., velocity, 1. / 60.);
            let (next, next_velocity, did_settle) =
                clamp_and_settle_wheel_step(next, next_velocity, 120., 240.);
            if !did_settle {
                assert!(next > before);
            }
            current = next;
            velocity = next_velocity;
            frames += 1;
            if did_settle {
                settled = true;
                break;
            }
        }

        assert!(settled);
        assert!(frames < 120);
        assert_eq!(current, 120.);
        assert_eq!(velocity, 0.);
    }

    #[test]
    fn equal_elapsed_time_is_nearly_equal_at_60_and_120_hz() {
        fn simulate(delta_seconds: f32, frames: usize) -> (f32, f32) {
            let mut position = 0.;
            let mut velocity = 0.;
            for _ in 0..frames {
                (position, velocity) = smooth_scroll_step(position, 600., velocity, delta_seconds);
            }
            (position, velocity)
        }

        let at_60 = simulate(1. / 60., 30);
        let at_120 = simulate(1. / 120., 60);
        assert!((at_60.0 - at_120.0).abs() < 1.);
        assert!((at_60.1 - at_120.1).abs() < 12.);
    }

    #[test]
    fn one_long_step_matches_equivalent_bounded_substeps() {
        let one_call = smooth_scroll_step(0., 600., 0., 0.20);
        let mut four_calls = (0., 0.);
        for _ in 0..4 {
            four_calls = smooth_scroll_step(four_calls.0, 600., four_calls.1, 0.05);
        }

        assert!((one_call.0 - four_calls.0).abs() < 0.0001);
        assert!((one_call.1 - four_calls.1).abs() < 0.001);
    }

    #[test]
    fn idle_queue_initializes_target_timestamp_and_zero_velocity() {
        let now = Instant::now();
        let mut inner = BrowserScrollStateInner {
            wheel_velocity_px_per_second: 240.,
            ..Default::default()
        };

        queue_wheel_state(&mut inner, 100., 400., 40., now);

        assert_eq!(inner.wheel_target, Some(140.));
        assert_eq!(inner.wheel_velocity_px_per_second, 0.);
        assert_eq!(inner.wheel_last_frame, Some(now));
        assert_eq!(inner.wheel_last_input, Some(now));
    }

    #[test]
    fn same_direction_queue_extends_target_without_resetting_motion_state() {
        let first_frame = Instant::now();
        let later = first_frame + Duration::from_millis(10);
        let mut inner = BrowserScrollStateInner::default();
        queue_wheel_state(&mut inner, 100., 400., 40., first_frame);
        inner.wheel_velocity_px_per_second = 240.;

        queue_wheel_state(&mut inner, 100., 400., 40., later);

        assert_eq!(inner.wheel_target, Some(180.));
        assert_eq!(inner.wheel_velocity_px_per_second, 240.);
        assert_eq!(inner.wheel_last_frame, Some(first_frame));
        assert_eq!(inner.wheel_last_input, Some(later));
    }

    #[test]
    fn opposite_direction_queue_rebases_and_resets_motion_state() {
        let first_frame = Instant::now();
        let later = first_frame + Duration::from_millis(10);
        let mut inner = BrowserScrollStateInner::default();
        queue_wheel_state(&mut inner, 150., 400., 80., first_frame);
        inner.wheel_velocity_px_per_second = 300.;

        queue_wheel_state(&mut inner, 150., 400., -40., later);

        assert_eq!(inner.wheel_target, Some(110.));
        assert_eq!(inner.wheel_velocity_px_per_second, 0.);
        assert_eq!(inner.wheel_last_frame, Some(later));
        assert_eq!(inner.wheel_last_input, Some(later));
    }

    #[test]
    fn direction_reversal_never_crosses_the_new_target() {
        let (next, velocity) = smooth_scroll_step(100., 40., -5_000., 0.05);

        assert_eq!(next, 40.);
        assert_eq!(velocity, 0.);
    }

    #[test]
    fn endpoint_settling_clamps_and_clears_velocity() {
        assert_eq!(
            clamp_and_settle_wheel_step(-4., -12., 0., 240.),
            (0., 0., true)
        );
        assert_eq!(
            clamp_and_settle_wheel_step(244., 12., 240., 240.),
            (240., 0., true)
        );
    }

    #[test]
    fn wheel_integration_waits_for_elapsed_time_then_advances() {
        let start = Instant::now();
        let mut inner = BrowserScrollStateInner::default();
        queue_wheel_state(&mut inner, 0., 400., 120., start);
        inner.frame_scheduled = true;

        let at_start = advance_wheel_motion(&mut inner, 0., 400., start);
        assert_eq!(at_start, (None, true));
        assert_eq!(inner.wheel_target, Some(120.));
        assert_eq!(inner.wheel_last_frame, Some(start));
        assert!(inner.frame_scheduled);

        let one_frame_later = start + Duration::from_secs_f32(1. / 60.);
        let after_frame = advance_wheel_motion(&mut inner, 0., 400., one_frame_later);
        assert!(after_frame.0.is_some_and(|position| position > 0.));
        assert!(after_frame.1);
        assert!(inner.wheel_velocity_px_per_second > 0.);
        assert!(inner.frame_scheduled);
    }

    #[test]
    fn wheel_endpoint_settling_clears_all_motion_state() {
        let start = Instant::now();
        let mut inner = BrowserScrollStateInner::default();
        queue_wheel_state(&mut inner, 200., 240., 80., start);
        inner.wheel_velocity_px_per_second = 120.;
        inner.frame_scheduled = true;

        let result = advance_wheel_motion(
            &mut inner,
            240.,
            240.,
            start + Duration::from_secs_f32(1. / 60.),
        );

        assert_eq!(result, (None, false));
        assert_eq!(inner.wheel_target, None);
        assert_eq!(inner.wheel_velocity_px_per_second, 0.);
        assert_eq!(inner.wheel_last_frame, None);
        assert_eq!(inner.wheel_last_input, None);
        assert!(inner.frame_scheduled);
    }

    #[test]
    fn wheel_tail_continues_before_120_milliseconds_and_snaps_at_120_milliseconds() {
        let start = Instant::now();
        let mut inner = BrowserScrollStateInner::default();
        queue_wheel_state(&mut inner, 0., 400., 120., start);

        let before_tail =
            advance_wheel_motion(&mut inner, 0., 400., start + Duration::from_millis(119));
        assert!(before_tail.1);
        assert_eq!(inner.wheel_target, Some(120.));

        let at_tail = advance_wheel_motion(
            &mut inner,
            before_tail.0.unwrap_or(0.),
            400.,
            start + Duration::from_millis(120),
        );
        assert_eq!(at_tail, (Some(120.), false));
        assert_eq!(inner.wheel_target, None);
        assert_eq!(inner.wheel_velocity_px_per_second, 0.);
        assert_eq!(inner.wheel_last_frame, None);
        assert_eq!(inner.wheel_last_input, None);
    }

    #[test]
    fn non_finite_wheel_inputs_fail_safe_to_the_target() {
        assert_eq!(smooth_scroll_step(f32::NAN, 120., 0., 1. / 60.), (120., 0.));
        assert_eq!(
            smooth_scroll_step(0., 120., f32::INFINITY, 1. / 60.),
            (120., 0.)
        );
        assert_eq!(smooth_scroll_step(0., 120., 0., f32::NAN), (120., 0.));
    }

    #[test]
    fn precise_wheel_interruption_clears_target_velocity_and_timestamp() {
        let state = BrowserScrollState::new();
        let now = Instant::now();
        {
            let mut inner = state.inner.borrow_mut();
            inner.wheel_target = Some(120.);
            inner.wheel_velocity_px_per_second = 240.;
            inner.wheel_last_frame = Some(now);
            inner.wheel_last_input = Some(now);
            inner.frame_scheduled = true;
            inner.clear_wheel_motion();
            assert_eq!(inner.wheel_target, None);
            assert_eq!(inner.wheel_velocity_px_per_second, 0.);
            assert_eq!(inner.wheel_last_frame, None);
            assert_eq!(inner.wheel_last_input, None);
            assert!(inner.frame_scheduled);
        }
    }

    #[test]
    fn gpui_lines_already_include_os_preference_and_use_fixed_pixel_scale() {
        assert_eq!(
            wheel_pixel_delta(ScrollDelta::Lines(point(3., -2.))),
            point(px(120.), px(-80.))
        );
        let pixels = point(px(12.5), px(-4.25));
        assert_eq!(wheel_pixel_delta(ScrollDelta::Pixels(pixels)), pixels);
    }

    #[test]
    fn vertical_axis_wins_only_when_it_is_not_dominated_by_horizontal_motion() {
        assert!(is_vertical_scroll(point(px(0.), px(-1.))));
        assert!(is_vertical_scroll(point(px(1.), px(-1.))));
        assert!(!is_vertical_scroll(point(px(2.), px(-1.))));
        assert!(!is_vertical_scroll(point(px(0.), px(0.))));
    }

    #[test]
    fn open_context_menu_does_not_steal_wheel_from_the_menu() {
        let source = include_str!("browser_scroll.rs");
        let handler = source
            .split("if crate::context_menu::context_menu_is_open()")
            .nth(1)
            .expect("menu-open wheel branch");
        let handler = handler
            .split("if !wheel_hitbox.should_handle_scroll")
            .next()
            .unwrap();
        assert!(handler.contains("scrolls_while_menu_open()"));
        assert!(handler.contains("cancel_autoscroll_and_release"));
        assert!(handler.contains("should_handle_scroll(window)"));
        assert!(
            handler.contains("cx.stop_propagation()"),
            "page surfaces still swallow wheel when they own the pointer"
        );
    }
}
