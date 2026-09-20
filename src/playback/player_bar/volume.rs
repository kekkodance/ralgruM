use super::*;

use crate::ui::slider_pointer::{SliderPointerPaint, slider_pointer_surface};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VolumeVisual {
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum VolumeMotionMode {
    Animated,
    Hold(f32),
    Direct,
}

#[derive(Debug)]
pub(super) struct VolumeMotion {
    pub(super) initialized: bool,
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl Default for VolumeMotion {
    fn default() -> Self {
        Self {
            initialized: false,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl VolumeMotion {
    pub(super) fn prepare(
        &mut self,
        target: f32,
        now: Instant,
        reduced_motion: bool,
        mode: VolumeMotionMode,
    ) -> VolumeVisual {
        let target = target.clamp(0., 1.);

        if !self.initialized {
            self.initialized = true;
            let initial = match mode {
                VolumeMotionMode::Hold(held) => held.clamp(0., 1.),
                VolumeMotionMode::Animated | VolumeMotionMode::Direct => target,
            };
            self.from = initial;
            self.target = initial;
            self.started_at = None;
        } else if let VolumeMotionMode::Hold(held) = mode {
            let held = held.clamp(0., 1.);
            self.from = held;
            self.target = held;
            self.started_at = None;
        } else if mode == VolumeMotionMode::Direct {
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if self.target != target {
            let displayed = self.displayed_at(now);
            self.from = displayed;
            self.target = target;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && displayed != target).then_some(now);
            if reduced_motion || displayed == target {
                self.from = target;
                self.started_at = None;
            }
        } else if reduced_motion
            || (self.started_at.is_some() && self.animation_progress(now) >= 1.)
        {
            self.from = self.target;
            self.started_at = None;
        }

        VolumeVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::INTERACTION_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::INTERACTION_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

pub(super) fn register_volume_pointer_handlers(
    paint: SliderPointerPaint,
    displayed_value: f32,
    model: Entity<PlaybackModel>,
    state: Rc<Cell<VolumePointerState>>,
    window: &mut Window,
) {
    let down_hitbox = paint.hitbox.clone();
    let down_state = state.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        if event.button == MouseButton::Left && down_hitbox.is_hovered(window) {
            down_state.set(VolumePointerState::pending(
                f32::from(event.position.x),
                f32::from(event.position.y),
                displayed_value,
            ));
            window.capture_pointer(down_hitbox.id);
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        }
    });

    let move_model = model.clone();
    let move_state = state.clone();
    let move_bounds = paint.logical_bounds;
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        let mut pointer_state = move_state.get();
        if event.pressed_button == Some(MouseButton::Left) && pointer_state.is_active() {
            let dragging = pointer_state
                .update_move(f32::from(event.position.x), f32::from(event.position.y))
                || matches!(pointer_state.phase, VolumePointerPhase::Dragging);
            move_state.set(pointer_state);
            if dragging {
                let volume = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
                move_model.update(cx, |model, cx| model.set_volume(volume, cx));
            }
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        } else if event.pressed_button.is_none() && pointer_state.is_active() {
            if finish_volume_pointer_interaction(&move_state).is_some() {
                let volume = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
                move_model.update(cx, |model, cx| model.set_volume(volume, cx));
                window.release_pointer();
                window.prevent_default();
                cx.stop_propagation();
                window.refresh();
            } else if matches!(pointer_state.phase, VolumePointerPhase::DirectRelease) {
                // The final direct update was already consumed. Release the
                // capture without writing the same value a second time.
                window.release_pointer();
                window.prevent_default();
                cx.stop_propagation();
            }
        }
    });

    let up_model = model;
    let up_state = state;
    let up_bounds = paint.logical_bounds;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        if event.button == MouseButton::Left {
            let released = finish_volume_pointer_interaction(&up_state);
            if released.is_some() {
                let volume = pointer_seek_fraction(f32::from(event.position.x), up_bounds);
                up_model.update(cx, |model, cx| model.set_volume(volume, cx));
            } else if !matches!(up_state.get().phase, VolumePointerPhase::DirectRelease) {
                return;
            }
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        }
    });
}

pub(super) fn volume_controls(
    model: Entity<PlaybackModel>,
    slider: Entity<SliderState>,
    pointer_state: Rc<Cell<VolumePointerState>>,
    level: VolumeIconLevel,
    visual: VolumeVisual,
    displayed_value: f32,
    offset_visual: RectMotionVisual,
) -> AnyElement {
    let pointer_state_for_canvas = pointer_state.clone();
    let pointer_active = matches!(
        pointer_state.get().phase,
        VolumePointerPhase::PendingClick { .. } | VolumePointerPhase::Dragging
    );
    let container = div()
        .id("volume-container")
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(VOLUME_GAP_PX))
        .child(volume_button(model.clone(), level))
        .child(
            div()
                .id("volume-slider")
                .w(px(VOLUME_SLIDER_WIDTH_PX))
                .h(px(VOLUME_SLIDER_CONTROL_HEIGHT_PX))
                .flex_none()
                .relative()
                .cursor_pointer()
                .child(volume_track(visual))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .overflow_hidden()
                        .cursor_pointer()
                        .child(Slider::new(&slider).horizontal().opacity(0.)),
                )
                .child(slider_pointer_surface(
                    VOLUME_THUMB_DIAMETER_PX,
                    pointer_active,
                    move |paint, window, _| {
                        register_volume_pointer_handlers(
                            paint,
                            displayed_value,
                            model.clone(),
                            pointer_state_for_canvas.clone(),
                            window,
                        );
                    },
                )),
        );
    let target_offsets = (offset_visual.target.left, offset_visual.target.top);
    if offset_visual.active {
        container
            .ml(px(offset_visual.from.left))
            .relative()
            .right(px(offset_visual.from.top))
            .with_animation(
                ("player-volume-container-layout", offset_visual.epoch),
                crate::motion::panel(),
                move |this, delta| {
                    this.ml(px(crate::motion::lerp(
                        offset_visual.from.left,
                        target_offsets.0,
                        delta,
                    )))
                    .right(px(crate::motion::lerp(
                        offset_visual.from.top,
                        target_offsets.1,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        container
            .ml(px(target_offsets.0))
            .relative()
            .right(px(target_offsets.1))
            .into_any_element()
    }
}

pub(super) fn volume_track(visual: VolumeVisual) -> AnyElement {
    let volume_fill = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(relative(visual.from))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    let volume_thumb = div()
        .absolute()
        .left(relative(visual.from))
        .top(px((VOLUME_TRACK_HEIGHT_PX - VOLUME_THUMB_DIAMETER_PX) * 0.5))
        .ml(px(-VOLUME_THUMB_DIAMETER_PX * 0.5))
        .size(px(VOLUME_THUMB_DIAMETER_PX))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    let volume_fill = if visual.active {
        volume_fill
            .with_animation(
                format!("player-volume-fill-{}", visual.epoch),
                crate::motion::interaction(),
                move |this, delta| {
                    this.w(relative(crate::motion::lerp(
                        visual.from,
                        visual.target,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        volume_fill.into_any_element()
    };
    let volume_thumb = if visual.active {
        volume_thumb
            .with_animation(
                format!("player-volume-thumb-{}", visual.epoch),
                crate::motion::interaction(),
                move |this, delta| {
                    this.left(relative(crate::motion::lerp(
                        visual.from,
                        visual.target,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        volume_thumb.into_any_element()
    };

    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((VOLUME_SLIDER_CONTROL_HEIGHT_PX
            - VOLUME_TRACK_HEIGHT_PX)
            * 0.5))
        .h(px(VOLUME_TRACK_HEIGHT_PX))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .cursor_pointer()
        .child(volume_fill)
        .child(volume_thumb)
        .into_any_element()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum VolumePointerPhase {
    #[default]
    Idle,
    PendingClick {
        press_x: f32,
        press_y: f32,
        held_value: f32,
    },
    Dragging,
    DirectRelease,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct VolumePointerState {
    pub(super) phase: VolumePointerPhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum VolumePointerRelease {
    PendingClick,
    Dragging,
}

pub(super) const VOLUME_DRAG_THRESHOLD_PX: f32 = 3.;

impl VolumePointerState {
    pub(super) fn pending(press_x: f32, press_y: f32, held_value: f32) -> Self {
        Self {
            phase: VolumePointerPhase::PendingClick {
                press_x,
                press_y,
                held_value: held_value.clamp(0., 1.),
            },
        }
    }

    pub(super) fn is_active(self) -> bool {
        !matches!(self.phase, VolumePointerPhase::Idle)
    }

    pub(super) fn update_move(&mut self, pointer_x: f32, pointer_y: f32) -> bool {
        let VolumePointerPhase::PendingClick {
            press_x, press_y, ..
        } = self.phase
        else {
            return false;
        };
        let dx = pointer_x - press_x;
        let dy = pointer_y - press_y;
        if dx * dx + dy * dy < VOLUME_DRAG_THRESHOLD_PX * VOLUME_DRAG_THRESHOLD_PX {
            return false;
        }
        self.phase = VolumePointerPhase::Dragging;
        true
    }

    pub(super) fn finish(&mut self) -> Option<VolumePointerRelease> {
        let release = match self.phase {
            VolumePointerPhase::PendingClick { .. } => VolumePointerRelease::PendingClick,
            VolumePointerPhase::Dragging => VolumePointerRelease::Dragging,
            VolumePointerPhase::DirectRelease | VolumePointerPhase::Idle => return None,
        };
        self.phase = if release == VolumePointerRelease::Dragging {
            VolumePointerPhase::DirectRelease
        } else {
            VolumePointerPhase::Idle
        };
        Some(release)
    }

    pub(super) fn motion_mode(self) -> VolumeMotionMode {
        match self.phase {
            VolumePointerPhase::PendingClick { held_value, .. } => {
                VolumeMotionMode::Hold(held_value)
            }
            VolumePointerPhase::Dragging | VolumePointerPhase::DirectRelease => {
                VolumeMotionMode::Direct
            }
            VolumePointerPhase::Idle => VolumeMotionMode::Animated,
        }
    }
}

pub(super) fn finish_volume_pointer_interaction(
    state: &Cell<VolumePointerState>,
) -> Option<VolumePointerRelease> {
    let mut pointer_state = state.get();
    if !pointer_state.is_active() {
        return None;
    }
    let release = pointer_state.finish();
    state.set(pointer_state);
    release
}

pub(super) fn volume_motion_mode_for_render(state: &Cell<VolumePointerState>) -> VolumeMotionMode {
    let pointer_state = state.get();
    let mode = pointer_state.motion_mode();
    if pointer_state.phase == VolumePointerPhase::DirectRelease {
        state.set(VolumePointerState::default());
    }
    mode
}
