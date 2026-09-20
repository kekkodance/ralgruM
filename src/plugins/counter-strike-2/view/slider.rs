use std::{cell::Cell, rc::Rc};

use gpui::{
    AnimationExt as _, AnyElement, Bounds, Entity, EntityId, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window, div, prelude::*, px, relative,
    rgb,
};
use gpui_component::slider::{Slider, SliderEvent, SliderState, SliderValue};

use crate::{
    motion::{SliderMotionMode, SliderVisual},
    theme::{PRIMARY, SCROLLBAR_THUMB},
    ui::slider_pointer::{SliderPointerPaint, slider_pointer_surface},
};

// Geometry mirrors the player bar volume slider (see
// src/playback/player_bar.rs). Those constants are private to the player bar
// module, so the same values live here.
const SLIDER_CONTROL_HEIGHT_PX: f32 = 24.;
const SLIDER_TRACK_HEIGHT_PX: f32 = 4.;
const SLIDER_THUMB_DIAMETER_PX: f32 = 13.;
const SLIDER_DETENT_DIAMETER_PX: f32 = 5.;

/// Marker color: light enough to read on the unfilled track, bright enough to
/// stay visible where the primary fill passes through it.
pub(super) const SLIDER_DETENT_COLOR: u32 = 0xd4d4d8;

/// Shared 0..120 action slider scale (see controller::action_position). Both
/// the controller math and the paint path use this, so the scale has a single
/// source of truth.
pub(super) const ACTION_SLIDER_MAX: f32 = 120.;

/// Snap position of the Mute action on the shared scale. The slider detent,
/// the controller's action math, and the marker below the track all use it.
pub(super) const ACTION_MUTE_POSITION: f32 = 10.;

/// Slider positions that get a visible detent dot: the Pause and Mute snap
/// points only. The volume zone is continuous, so it gets no landmarks.
const ACTION_SLIDER_DETENTS: [f32; 2] = [0., ACTION_MUTE_POSITION];
const SLIDER_DRAG_THRESHOLD_PX: f32 = 3.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum SliderPointerPhase {
    #[default]
    Idle,
    Pending {
        press_x: f32,
        press_y: f32,
        held_fraction: f32,
    },
    Dragging,
    DirectRelease,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SliderPointerInteraction {
    owner: Option<EntityId>,
    phase: SliderPointerPhase,
}

impl SliderPointerInteraction {
    fn pending(owner: EntityId, press_x: f32, press_y: f32, held_fraction: f32) -> Self {
        Self {
            owner: Some(owner),
            phase: SliderPointerPhase::Pending {
                press_x,
                press_y,
                held_fraction: held_fraction.clamp(0., 1.),
            },
        }
    }

    fn update_move(&mut self, pointer_x: f32, pointer_y: f32) -> bool {
        let SliderPointerPhase::Pending {
            press_x, press_y, ..
        } = self.phase
        else {
            return matches!(self.phase, SliderPointerPhase::Dragging);
        };
        let dx = pointer_x - press_x;
        let dy = pointer_y - press_y;
        if dx * dx + dy * dy < SLIDER_DRAG_THRESHOLD_PX * SLIDER_DRAG_THRESHOLD_PX {
            return false;
        }
        self.phase = SliderPointerPhase::Dragging;
        true
    }

    fn finish(&mut self) -> bool {
        let was_dragging = matches!(self.phase, SliderPointerPhase::Dragging);
        self.phase = if was_dragging {
            SliderPointerPhase::DirectRelease
        } else {
            SliderPointerPhase::Idle
        };
        if !was_dragging {
            self.owner = None;
        }
        was_dragging
    }
}

#[derive(Clone, Default)]
pub(super) struct SliderPointerState {
    interaction: Rc<Cell<SliderPointerInteraction>>,
}

impl SliderPointerState {
    pub(super) fn motion_mode_for(&self, slider: EntityId) -> SliderMotionMode {
        let interaction = self.interaction.get();
        if interaction.owner != Some(slider) {
            return SliderMotionMode::Animated;
        }
        match interaction.phase {
            SliderPointerPhase::Pending { held_fraction, .. } => {
                SliderMotionMode::Hold(held_fraction)
            }
            SliderPointerPhase::Dragging => SliderMotionMode::Direct,
            SliderPointerPhase::DirectRelease => {
                self.interaction.set(SliderPointerInteraction::default());
                SliderMotionMode::Direct
            }
            SliderPointerPhase::Idle => SliderMotionMode::Animated,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SliderPresentation {
    pub(super) visual: SliderVisual,
    pub(super) displayed_fraction: f32,
}

/// Read the current fill fraction (0..1) of a single-value slider.
pub(super) fn slider_fraction(slider: &Entity<SliderState>, cx: &gpui::App) -> f32 {
    let state = slider.read(cx);
    let SliderValue::Single(value) = state.value() else {
        return 0.;
    };
    let min = state.min_value();
    let max = state.max_value();
    if max <= min {
        return 0.;
    }
    ((value - min) / (max - min)).clamp(0., 1.)
}

/// Build the custom-styled slider used by the fade rows: a hand-painted track
/// (base, fill, thumb) with the invisible gpui-component Slider overlaid for
/// input. The pattern comes from the player bar volume slider.
pub(super) fn cs2_slider(
    slider: &Entity<SliderState>,
    presentation: SliderPresentation,
    pointer: &SliderPointerState,
) -> impl IntoElement {
    cs2_slider_with_detents(slider, presentation, None, pointer)
}

/// Same as [`cs2_slider`], with detent dots marking the snap points of the
/// action sliders.
pub(super) fn cs2_action_slider(
    slider: &Entity<SliderState>,
    presentation: SliderPresentation,
    pointer: &SliderPointerState,
) -> impl IntoElement {
    cs2_slider_with_detents(slider, presentation, Some(&ACTION_SLIDER_DETENTS), pointer)
}

fn cs2_slider_with_detents(
    slider: &Entity<SliderState>,
    presentation: SliderPresentation,
    detents: Option<&[f32]>,
    pointer: &SliderPointerState,
) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(SLIDER_CONTROL_HEIGHT_PX))
        .cursor_pointer()
        .child(cs2_slider_track(
            slider.entity_id(),
            presentation.visual,
            detents,
        ))
        .child(
            div()
                .absolute()
                .inset_0()
                .overflow_hidden()
                .child(Slider::new(slider).horizontal().opacity(0.)),
        )
        .child(slider_pointer_layer(
            slider,
            presentation.displayed_fraction,
            pointer,
        ))
}

fn slider_pointer_layer(
    slider: &Entity<SliderState>,
    displayed_fraction: f32,
    pointer: &SliderPointerState,
) -> impl IntoElement {
    let slider = slider.clone();
    let slider_for_paint = slider.clone();
    let pointer = pointer.clone();
    let pointer_for_paint = pointer.clone();
    slider_pointer_surface(SLIDER_THUMB_DIAMETER_PX, move |paint, window, _| {
        register_slider_pointer_handlers(
            paint,
            slider_for_paint.clone(),
            displayed_fraction,
            pointer_for_paint.clone(),
            window,
        );
    })
}

fn register_slider_pointer_handlers(
    paint: SliderPointerPaint,
    slider: Entity<SliderState>,
    displayed_fraction: f32,
    pointer: SliderPointerState,
    window: &mut Window,
) {
    let slider_id = slider.entity_id();
    let down_hitbox = paint.hitbox.clone();
    let down_pointer = pointer.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.capture() || event.button != MouseButton::Left || !down_hitbox.is_hovered(window)
        {
            return;
        }
        down_pointer
            .interaction
            .set(SliderPointerInteraction::pending(
                slider_id,
                f32::from(event.position.x),
                f32::from(event.position.y),
                displayed_fraction,
            ));
        window.capture_pointer(down_hitbox.id);
        window.prevent_default();
        cx.stop_propagation();
        window.refresh();
    });

    let move_bounds = paint.logical_bounds;
    let move_slider = slider.clone();
    let move_pointer = pointer.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        let mut interaction = move_pointer.interaction.get();
        if interaction.owner != Some(slider_id) {
            return;
        }
        if event.pressed_button == Some(MouseButton::Left) {
            let dragging =
                interaction.update_move(f32::from(event.position.x), f32::from(event.position.y));
            move_pointer.interaction.set(interaction);
            if dragging {
                update_slider_from_pointer(
                    &move_slider,
                    event.position.x,
                    move_bounds,
                    false,
                    window,
                    cx,
                );
            }
        } else {
            interaction.finish();
            move_pointer.interaction.set(interaction);
            update_slider_from_pointer(
                &move_slider,
                event.position.x,
                move_bounds,
                true,
                window,
                cx,
            );
            window.release_pointer();
        }
        window.prevent_default();
        cx.stop_propagation();
        window.refresh();
    });

    let up_bounds = paint.logical_bounds;
    let up_pointer = pointer;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if !phase.capture() || event.button != MouseButton::Left {
            return;
        }
        let mut interaction = up_pointer.interaction.get();
        if interaction.owner != Some(slider_id) {
            return;
        }
        if matches!(interaction.phase, SliderPointerPhase::DirectRelease) {
            up_pointer
                .interaction
                .set(SliderPointerInteraction::default());
        } else {
            interaction.finish();
            up_pointer.interaction.set(interaction);
            update_slider_from_pointer(&slider, event.position.x, up_bounds, true, window, cx);
        }
        window.release_pointer();
        window.prevent_default();
        cx.stop_propagation();
        window.refresh();
    });
}

fn update_slider_from_pointer(
    slider: &Entity<SliderState>,
    pointer_x: Pixels,
    bounds: Bounds<Pixels>,
    release: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let fraction = if bounds.size.width <= px(0.) {
        0.0
    } else {
        (f32::from(pointer_x - bounds.left()) / f32::from(bounds.size.width)).clamp(0.0, 1.0)
    };
    slider.update(cx, |state, cx| {
        let min = state.min_value();
        let max = state.max_value();
        let step = state.step_value();
        let value = slider_value_at_fraction(fraction, min, max, step);
        state.set_value(value, window, cx);
        if release {
            cx.emit(SliderEvent::Release(SliderValue::Single(value)));
        } else {
            cx.emit(SliderEvent::Change(SliderValue::Single(value)));
        }
    });
}

fn slider_value_at_fraction(fraction: f32, min: f32, max: f32, step: f32) -> f32 {
    let raw = min + (max - min) * fraction.clamp(0.0, 1.0);
    if step > 0.0 {
        ((raw / step).round() * step).clamp(min, max)
    } else {
        raw.clamp(min, max)
    }
}

fn cs2_slider_track(
    slider_id: EntityId,
    visual: SliderVisual,
    detents: Option<&[f32]>,
) -> impl IntoElement {
    let fill = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(relative(visual.from))
        .rounded_full()
        .bg(rgb(PRIMARY));
    let thumb = div()
        .absolute()
        .left(relative(visual.from))
        .top(px((SLIDER_TRACK_HEIGHT_PX - SLIDER_THUMB_DIAMETER_PX) * 0.5))
        .ml(px(-SLIDER_THUMB_DIAMETER_PX * 0.5))
        .size(px(SLIDER_THUMB_DIAMETER_PX))
        .rounded_full()
        .bg(rgb(PRIMARY));
    let fill: AnyElement = if visual.active {
        fill.with_animation(
            format!("cs2-slider-fill-{slider_id}-{}", visual.epoch),
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
        fill.into_any_element()
    };
    let thumb: AnyElement = if visual.active {
        thumb
            .with_animation(
                format!("cs2-slider-thumb-{slider_id}-{}", visual.epoch),
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
        thumb.into_any_element()
    };
    let track = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((SLIDER_CONTROL_HEIGHT_PX - SLIDER_TRACK_HEIGHT_PX) * 0.5))
        .h(px(SLIDER_TRACK_HEIGHT_PX))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .child(fill);
    // Detents sit above base and fill so they stay visible on both.
    let track = match detents {
        Some(positions) => track.children(
            positions
                .iter()
                .map(|position| detent_dot(position / ACTION_SLIDER_MAX)),
        ),
        None => track,
    };
    track.child(thumb)
}

fn detent_dot(fraction: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(relative(fraction))
        .top(px(
            (SLIDER_TRACK_HEIGHT_PX - SLIDER_DETENT_DIAMETER_PX) * 0.5
        ))
        .ml(px(-SLIDER_DETENT_DIAMETER_PX * 0.5))
        .size(px(SLIDER_DETENT_DIAMETER_PX))
        .rounded_full()
        .bg(rgb(SLIDER_DETENT_COLOR))
}

#[cfg(test)]
mod tests {
    use gpui::EntityId;

    use super::{
        SliderPointerInteraction, SliderPointerPhase, SliderPointerState, slider_value_at_fraction,
    };
    use crate::motion::SliderMotionMode;

    #[test]
    fn pointer_values_clamp_and_follow_the_slider_step() {
        assert_eq!(slider_value_at_fraction(-1.0, 0.0, 120.0, 1.0), 0.0);
        assert_eq!(slider_value_at_fraction(0.505, 0.0, 120.0, 1.0), 61.0);
        assert_eq!(slider_value_at_fraction(2.0, 0.0, 120.0, 1.0), 120.0);
        assert!((slider_value_at_fraction(0.52, 0.0, 5.0, 0.1) - 2.6).abs() < 0.000_001);
    }

    #[test]
    fn pending_click_holds_the_visible_fraction_then_animates() {
        let slider_id = EntityId::from(1);
        let pointer = SliderPointerState::default();
        pointer
            .interaction
            .set(SliderPointerInteraction::pending(slider_id, 10., 20., 0.65));

        assert_eq!(
            pointer.motion_mode_for(slider_id),
            SliderMotionMode::Hold(0.65)
        );
        let mut interaction = pointer.interaction.get();
        assert!(!interaction.finish());
        pointer.interaction.set(interaction);
        assert_eq!(
            pointer.motion_mode_for(slider_id),
            SliderMotionMode::Animated
        );
    }

    #[test]
    fn drag_updates_directly_and_release_settles_once() {
        let slider_id = EntityId::from(2);
        let pointer = SliderPointerState::default();
        let mut interaction = SliderPointerInteraction::pending(slider_id, 10., 20., 0.4);

        assert!(!interaction.update_move(11., 21.));
        assert!(interaction.update_move(13., 20.));
        assert_eq!(interaction.phase, SliderPointerPhase::Dragging);
        pointer.interaction.set(interaction);
        assert_eq!(pointer.motion_mode_for(slider_id), SliderMotionMode::Direct);

        let mut interaction = pointer.interaction.get();
        assert!(interaction.finish());
        pointer.interaction.set(interaction);
        assert_eq!(pointer.motion_mode_for(slider_id), SliderMotionMode::Direct);
        assert_eq!(
            pointer.motion_mode_for(slider_id),
            SliderMotionMode::Animated
        );
    }
}
