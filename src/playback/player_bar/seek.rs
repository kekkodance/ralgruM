use super::*;

pub(super) fn seekbar_display_progress(playback_progress: f32, preview: Option<f32>) -> f32 {
    preview.unwrap_or(playback_progress).clamp(0.0, 1.0)
}

pub(super) fn accessibility_seek_fraction(current: f32, delta: f32, enabled: bool) -> Option<f32> {
    enabled.then_some((current + delta).clamp(0.0, 1.0))
}

pub(super) fn seek_accessibly(
    model: Entity<PlaybackModel>,
    delta: f32,
    enabled: bool,
    cx: &mut App,
) {
    let current = {
        let state = &model.read(cx).state;
        if !matches!(
            state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) || state.duration.is_zero()
        {
            return;
        }
        state.position.as_secs_f32() / state.duration.as_secs_f32()
    };
    let Some(next) = accessibility_seek_fraction(current, delta, enabled) else {
        return;
    };
    model.update(cx, |model, cx| model.seek_fraction(next, cx));
}

pub(super) struct SeekPointerPaintState {
    pub(super) hitbox: Hitbox,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct SeekPointerState {
    pub(super) active: bool,
    pub(super) moved: bool,
}

pub(super) fn pointer_seek_fraction(pointer_x: f32, bounds: Bounds<Pixels>) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0. {
        return 0.;
    }
    ((pointer_x - f32::from(bounds.origin.x)) / width).clamp(0., 1.)
}

pub(super) fn register_seek_pointer_handlers(
    paint: SeekPointerPaintState,
    model: Entity<PlaybackModel>,
    state: Rc<Cell<SeekPointerState>>,
    window: &mut Window,
) {
    let down_hitbox = paint.hitbox.clone();
    let down_model = model.clone();
    let down_state = state.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        if event.button == MouseButton::Left && down_hitbox.is_hovered(window) {
            down_state.set(SeekPointerState {
                active: true,
                moved: false,
            });
            down_model.update(cx, |model, _| model.begin_new_seek_pointer_interaction());
            window.capture_pointer(down_hitbox.id);
            window.prevent_default();
            cx.stop_propagation();
        }
    });

    let move_hitbox = paint.hitbox.clone();
    let move_model = model.clone();
    let move_state = state.clone();
    let move_bounds = move_hitbox.bounds;
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        let mut pointer_state = move_state.get();
        if pointer_state.active && event.pressed_button == Some(MouseButton::Left) {
            pointer_state.moved = true;
            move_state.set(pointer_state);
            let fraction = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
            move_model.update(cx, |model, cx| model.preview_seek_fraction(fraction, cx));
            cx.stop_propagation();
        } else if pointer_state.active && event.pressed_button.is_none() {
            move_state.set(SeekPointerState::default());
            let fraction = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
            move_model.update(cx, |model, cx| model.commit_seek_fraction(fraction, cx));
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
        }
    });

    let up_hitbox = paint.hitbox.clone();
    let up_model = model.clone();
    let up_state = state;
    let up_bounds = up_hitbox.bounds;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        let pointer_state = up_state.get();
        if event.button == MouseButton::Left && pointer_state.active {
            up_state.set(SeekPointerState::default());
            let fraction = pointer_seek_fraction(f32::from(event.position.x), up_bounds);
            up_model.update(cx, |model, cx| model.commit_seek_fraction(fraction, cx));
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
        }
    });
}

pub(super) fn progress_control(
    model: Entity<PlaybackModel>,
    state: Rc<Cell<SeekPointerState>>,
    buffered: BufferedVisual,
    seek_fill: SeekFillVisual,
    enabled: bool,
    cx: &mut Context<PlaybackView>,
) -> AnyElement {
    // Keep the component slider for pointer interaction and paint the
    // four-pixel track separately so no thumb is visible.
    div()
        .id("playback-progress")
        .flex_1()
        .h(px(24.))
        .relative()
        .when(enabled, {
            let model = model.clone();
            move |this| {
                this.role(Role::Slider)
                    .aria_numeric_value(seek_fill.target as f64)
                    .aria_min_numeric_value(0.)
                    .aria_max_numeric_value(1.)
                    .aria_numeric_value_step(SEEK_SLIDER_STEP as f64)
                    .focusable()
                    .tab_stop(true)
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .focus_visible(|style| style.border_color(rgb(PRIMARY)))
                    .cursor_pointer()
                    .on_a11y_action(AccessibleAction::Increment, {
                        let model = model.clone();
                        move |_, _, cx| seek_accessibly(model.clone(), SEEK_SLIDER_STEP, true, cx)
                    })
                    .on_a11y_action(AccessibleAction::Decrement, move |_, _, cx| {
                        seek_accessibly(model.clone(), -SEEK_SLIDER_STEP, true, cx)
                    })
            }
        })
        .child(
            div()
                .id("playback-track")
                .absolute()
                .top(px(10.))
                .left_0()
                .right_0()
                .h(px(4.))
                .rounded_full()
                .bg(rgb(BORDER))
                .when(buffered.target > 0.0 || buffered.active, move |this| {
                    let buffered_bar = div()
                        .id("playback-buffered")
                        .absolute()
                        .top_0()
                        .left_0()
                        .bottom_0()
                        .w(relative(buffered.from))
                        .rounded_full()
                        .bg(rgba(0x94a3b261))
                        .with_animation(
                            format!(
                                "playback-buffered-{}-{}",
                                buffered.generation, buffered.epoch
                            ),
                            crate::motion::content(),
                            move |this, delta| {
                                this.w(relative(crate::motion::lerp(
                                    buffered.from,
                                    buffered.target,
                                    delta,
                                )))
                            },
                        );
                    this.child(buffered_bar)
                })
                .when(seek_fill.target > 0.0 || seek_fill.active, move |this| {
                    let playback_fill = if seek_fill.active {
                        div()
                            .id("playback-fill")
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .w(relative(seek_fill.from))
                            .rounded_full()
                            .bg(rgb(FOREGROUND))
                            .with_animation(
                                format!("playback-seek-fill-{}", seek_fill.epoch),
                                crate::motion::interaction(),
                                move |this, delta| {
                                    this.w(relative(crate::motion::lerp(
                                        seek_fill.from,
                                        seek_fill.target,
                                        delta,
                                    )))
                                },
                            )
                            .into_any_element()
                    } else {
                        div()
                            .id("playback-fill")
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .w(relative(seek_fill.from))
                            .rounded_full()
                            .bg(rgb(FOREGROUND))
                            .into_any_element()
                    };
                    this.child(playback_fill)
                }),
        )
        .when(enabled, {
            let model = model.clone();
            move |this| {
                this.child(
                    canvas(
                        move |bounds, window, _cx| SeekPointerPaintState {
                            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        },
                        move |_bounds, paint, window, _cx| {
                            register_seek_pointer_handlers(
                                paint,
                                model.clone(),
                                state.clone(),
                                window,
                            );
                        },
                    )
                    .absolute()
                    .inset_0()
                    .cursor_pointer(),
                )
            }
        })
        .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
            let delta = match event.keystroke.key.as_str() {
                "left" => -SEEK_SLIDER_STEP,
                "right" => SEEK_SLIDER_STEP,
                _ => return,
            };
            let current = {
                let state = &model.read(cx).state;
                if !matches!(
                    state.status,
                    PlaybackStatus::Playing | PlaybackStatus::Paused
                ) || state.duration.is_zero()
                {
                    return;
                }
                state.position.as_secs_f32() / state.duration.as_secs_f32()
            };
            model.update(cx, |model, cx| model.seek_fraction(current + delta, cx));
        }))
        .into_any_element()
}
