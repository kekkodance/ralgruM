use gpui::{
    AnimationExt, Context, Entity, FontWeight, IntoElement, KeyDownEvent, Render, SharedString,
    Window, div, prelude::*, px, relative, rgb, rgba,
};
use gpui_component::{
    scroll::ScrollableElement as _,
    slider::{Slider, SliderState},
};

use super::Cs2SettingsDialog;
use crate::{
    app_button::secondary_dialog_button_with_disabled,
    assets::{LocalIcon, local_icon},
    dialog_layout::request_dialog_close,
    plugins::counter_strike_2::{service, settings::PlaybackAction, state::MatchState},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED, PRIMARY},
};

const SUCCESS: u32 = 0x22c55e;

impl Render for Cs2SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((setting, position)) = self.pending_snap.take() {
            let slider = self.action_slider_for(setting);
            window.defer(cx, move |window, cx| {
                slider.update(cx, |slider, cx| slider.set_value(position, window, cx));
            });
        }

        let runtime = service::snapshot();
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog = div()
            .id("counter-strike-2-settings-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() == "escape" {
                    window.prevent_default();
                    request_dialog_close(this, window, cx);
                    cx.stop_propagation();
                }
            }))
            .overflow_hidden()
            .rounded(px(8.0))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .child(dialog_header())
            .child(
                div()
                    .max_h(px(560.0))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .p(px(18.0))
                    .child(status_card(runtime.connection, runtime.match_state))
                    .child(action_row(
                        "Active round",
                        "Applied after freeze time while you are alive.",
                        self.settings.active_round,
                        &self.active_round,
                    ))
                    .child(action_row(
                        "Player dead",
                        "Overrides the active-round behavior after you die.",
                        self.settings.player_dead,
                        &self.player_dead,
                    ))
                    .child(action_row(
                        "Between rounds",
                        "Applied from round conclusion through the next freeze time.",
                        self.settings.between_rounds,
                        &self.between_rounds,
                    ))
                    .child(fade_card(
                        self.settings.fade_out_ms,
                        self.settings.fade_in_ms,
                        &self.fade_out,
                        &self.fade_in,
                    ))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_size(px(11.5))
                                .line_height(relative(1.45))
                                .text_color(rgb(DANGER))
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .px(px(18.0))
                    .py(px(14.0))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(secondary_dialog_button_with_disabled(
                        "repair-counter-strike-2-integration",
                        Some(LocalIcon::RotateRight),
                        if self.repairing {
                            "Installing..."
                        } else {
                            "Repair integration"
                        },
                        self.repairing,
                        cx.listener(|this, _, _, cx| this.repair(cx)),
                    ))
                    .child(secondary_dialog_button_with_disabled(
                        "close-counter-strike-2-settings",
                        None,
                        "Close",
                        false,
                        cx.listener(|this, _, window, cx| request_dialog_close(this, window, cx)),
                    )),
            );

        if closing {
            dialog
                .with_animation(
                    ("counter-strike-2-settings-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

fn dialog_header() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(18.0))
        .py(px(16.0))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .relative()
                .size(px(16.0))
                .top(px(1.0))
                .child(local_icon(LocalIcon::Crosshairs, FOREGROUND).size(px(14.0))),
        )
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Counter-Strike 2 Integration"),
        )
}

fn status_card(connection: service::ConnectionStatus, match_state: MatchState) -> impl IntoElement {
    let (status, color) = match connection {
        service::ConnectionStatus::Disabled => ("Disabled".to_owned(), MUTED),
        service::ConnectionStatus::Waiting => ("Waiting for Counter-Strike 2".to_owned(), MUTED),
        service::ConnectionStatus::Connected => ("Connected".to_owned(), SUCCESS),
        service::ConnectionStatus::Error(error) => (error, DANGER),
    };
    let state = match match_state {
        MatchState::Inactive => "No active match",
        MatchState::ActiveRound => "Active round",
        MatchState::PlayerDead => "Player dead",
        MatchState::BetweenRounds => "Between rounds",
    };
    card()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(div().size(px(7.0)).rounded_full().bg(rgb(color)))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .child(status),
                ),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(MUTED))
                .child(state),
        )
}

fn action_row(
    title: &'static str,
    description: &'static str,
    action: PlaybackAction,
    slider: &Entity<SliderState>,
) -> impl IntoElement {
    card()
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .line_height(relative(1.45))
                                .text_color(rgb(MUTED))
                                .child(description),
                        ),
                )
                .child(value_badge(action.label())),
        )
        .child(
            div()
                .w_full()
                .px(px(3.0))
                .child(Slider::new(slider).horizontal()),
        )
        .child(
            div()
                .relative()
                .h(px(15.0))
                .text_size(px(10.5))
                .text_color(rgb(MUTED))
                .child(div().absolute().left_0().child("Pause"))
                .child(div().absolute().left(relative(0.083)).child("Mute"))
                .child(div().absolute().right_0().child("100%")),
        )
}

fn fade_card(
    fade_out_ms: u32,
    fade_in_ms: u32,
    fade_out: &Entity<SliderState>,
    fade_in: &Entity<SliderState>,
) -> impl IntoElement {
    card()
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .child("Transitions"),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(relative(1.45))
                .text_color(rgb(MUTED))
                .child("Smooth volume changes when gameplay state changes."),
        )
        .child(fade_row("Fade out", fade_out_ms, fade_out))
        .child(fade_row("Fade in", fade_in_ms, fade_in))
}

fn fade_row(
    label: &'static str,
    milliseconds: u32,
    slider: &Entity<SliderState>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(11.5))
                .child(label)
                .child(
                    div()
                        .text_color(rgb(MUTED))
                        .child(format_duration(milliseconds)),
                ),
        )
        .child(
            div()
                .w_full()
                .px(px(3.0))
                .child(Slider::new(slider).horizontal()),
        )
}

fn card() -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .p(px(14.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgba(0x18181bb8))
}

fn value_badge(value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_none()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(5.0))
        .bg(rgba(0x6366f126))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(PRIMARY))
        .child(value.into())
}

fn format_duration(milliseconds: u32) -> String {
    if milliseconds == 0 {
        "Off".into()
    } else if milliseconds.is_multiple_of(1_000) {
        format!("{} s", milliseconds / 1_000)
    } else {
        format!("{:.1} s", milliseconds as f32 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_copy_is_compact() {
        assert_eq!(format_duration(0), "Off");
        assert_eq!(format_duration(500), "0.5 s");
        assert_eq!(format_duration(2_000), "2 s");
    }
}
