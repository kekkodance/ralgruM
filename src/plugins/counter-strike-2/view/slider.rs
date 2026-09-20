use gpui::{Entity, IntoElement, div, prelude::*, px, relative, rgb};
use gpui_component::slider::{Slider, SliderState, SliderValue};

use crate::theme::{PRIMARY, SCROLLBAR_THUMB};

// Geometry mirrors the player bar volume slider (see
// src/playback/player_bar.rs). Those constants are private to the player bar
// module, so the same values live here.
const SLIDER_CONTROL_HEIGHT_PX: f32 = 24.;
const SLIDER_TRACK_HEIGHT_PX: f32 = 4.;
const SLIDER_THUMB_DIAMETER_PX: f32 = 13.;
const SLIDER_DETENT_DIAMETER_PX: f32 = 3.;

/// Marker color: light enough to read on the unfilled track, bright enough to
/// stay visible where the primary fill passes through it.
const SLIDER_DETENT_COLOR: u32 = 0xd4d4d8;

/// End of the shared 0..120 action slider scale.
const ACTION_SLIDER_MAX: f32 = 120.;

/// Slider positions that get a visible detent dot: Pause, Mute, and the
/// 25/50/75/100 percent volume landmarks on the same 0..120 scale the
/// controller uses (see action_position).
const ACTION_SLIDER_DETENTS: [f32; 6] = [0., 10., 41.21, 67.52, 93.74, 120.];

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
pub(super) fn cs2_slider(slider: &Entity<SliderState>, fraction: f32) -> impl IntoElement {
    cs2_slider_with_detents(slider, fraction, None)
}

/// Same as [`cs2_slider`], with detent dots marking the snap points of the
/// action sliders.
pub(super) fn cs2_action_slider(slider: &Entity<SliderState>, fraction: f32) -> impl IntoElement {
    cs2_slider_with_detents(slider, fraction, Some(&ACTION_SLIDER_DETENTS))
}

fn cs2_slider_with_detents(
    slider: &Entity<SliderState>,
    fraction: f32,
    detents: Option<&[f32]>,
) -> impl IntoElement {
    div()
        .relative()
        .w_full()
        .h(px(SLIDER_CONTROL_HEIGHT_PX))
        .child(cs2_slider_track(fraction, detents))
        .child(
            div()
                .absolute()
                .inset_0()
                .overflow_hidden()
                .child(Slider::new(slider).horizontal().opacity(0.)),
        )
}

fn cs2_slider_track(fraction: f32, detents: Option<&[f32]>) -> impl IntoElement {
    let track = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((SLIDER_CONTROL_HEIGHT_PX - SLIDER_TRACK_HEIGHT_PX) * 0.5))
        .h(px(SLIDER_TRACK_HEIGHT_PX))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(relative(fraction))
                .rounded_full()
                .bg(rgb(PRIMARY)),
        );
    // Detents sit above base and fill so they stay visible on both.
    let track = match detents {
        Some(positions) => track.children(
            positions
                .iter()
                .map(|position| detent_dot(position / ACTION_SLIDER_MAX)),
        ),
        None => track,
    };
    track.child(
        div()
            .absolute()
            .left(relative(fraction))
            .top(px((SLIDER_TRACK_HEIGHT_PX - SLIDER_THUMB_DIAMETER_PX) * 0.5))
            .ml(px(-SLIDER_THUMB_DIAMETER_PX * 0.5))
            .size(px(SLIDER_THUMB_DIAMETER_PX))
            .rounded_full()
            .bg(rgb(PRIMARY)),
    )
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
