use gpui::{
    AnimationExt, Context, Entity, FontWeight, IntoElement, KeyDownEvent, Render, Window, div,
    prelude::*, px, relative, rgb, rgba,
};
use gpui_component::slider::SliderState;

use super::Cs2SettingsDialog;
use super::slider::{cs2_action_slider, cs2_slider, slider_fraction};
use crate::{
    app_button::secondary_dialog_button_with_disabled,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    dialog_layout::request_dialog_close,
    plugins::counter_strike_2::{service, settings::PlaybackAction, state::MatchState},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED},
};

const SUCCESS: u32 = 0x22c55e;

// Em-height matching the player bar volume icons so the action indicator has
// the same visual weight; widths follow each icon's viewbox aspect.
const ACTION_ICON_EM_HEIGHT_PX: f32 = 14.5;
const ACTION_ICON_VIEWBOX_HEIGHT: f32 = 512.;

/// Vertical space the dialog keeps for its header and footer when clamping
/// the scrollable body against the viewport.
const DIALOG_CHROME_HEIGHT: f32 = 160.;

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
        let body_max_height = body_max_height(f32::from(window.viewport_size().height));
        let body = div()
            .id("counter-strike-2-settings-body")
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(18.0))
            .max_h(px(body_max_height))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(status_card(runtime.connection, runtime.match_state))
            .child(action_row(
                "Active round",
                "Applied after freeze time while you are alive.",
                self.settings.active_round,
                &self.active_round,
                cx,
            ))
            .child(action_row(
                "Player dead",
                "Overrides the active-round behavior after you die.",
                self.settings.player_dead,
                &self.player_dead,
                cx,
            ))
            .child(action_row(
                "Between rounds",
                "Applied from round conclusion through the next freeze time.",
                self.settings.between_rounds,
                &self.between_rounds,
                cx,
            ))
            .child(fade_card(
                self.settings.fade_out_ms,
                self.settings.fade_in_ms,
                &self.fade_out,
                &self.fade_in,
                cx,
            ))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_size(px(11.5))
                        .line_height(relative(1.45))
                        .text_color(rgb(DANGER))
                        .child(error),
                )
            })
            .into_any_element();
        let scrolled = browser_scroll_surface(
            "counter-strike-2-settings-scroll",
            body,
            BrowserScrollTarget::Handle(self.scroll.clone()),
            self.browser_scroll.clone(),
        );
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
            .child(scrolled)
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

/// Height budget for the scrollable body so the dialog chrome stays on screen
/// in short viewports.
fn body_max_height(viewport_height: f32) -> f32 {
    ((viewport_height - 32.).max(0.) - DIALOG_CHROME_HEIGHT).max(0.)
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
    cx: &Context<Cs2SettingsDialog>,
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
                .child(action_icon(action)),
        )
        .child(
            div()
                .w_full()
                .px(px(3.0))
                .child(cs2_action_slider(slider, slider_fraction(slider, cx))),
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
    cx: &Context<Cs2SettingsDialog>,
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
        .child(fade_row("Fade out", fade_out_ms, fade_out, cx))
        .child(fade_row("Fade in", fade_in_ms, fade_in, cx))
}

fn fade_row(
    label: &'static str,
    milliseconds: u32,
    slider: &Entity<SliderState>,
    cx: &Context<Cs2SettingsDialog>,
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
                .child(cs2_slider(slider, slider_fraction(slider, cx))),
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

/// Indicator icon for the current action, sized with the same em-height
/// scaling as the player bar volume icons.
fn action_icon(action: PlaybackAction) -> impl IntoElement {
    let icon = match action {
        PlaybackAction::Pause => LocalIcon::Pause,
        PlaybackAction::Mute => LocalIcon::VolumeMuted,
        PlaybackAction::Volume(percent) if percent <= 33 => LocalIcon::VolumeOff,
        PlaybackAction::Volume(percent) if percent <= 66 => LocalIcon::VolumeLow,
        PlaybackAction::Volume(_) => LocalIcon::VolumeHigh,
    };
    let viewbox_width = match icon {
        LocalIcon::Pause => 384.,
        LocalIcon::VolumeMuted => 576.,
        LocalIcon::VolumeOff => 320.,
        LocalIcon::VolumeLow => 448.,
        _ => 640.,
    };
    let width = ACTION_ICON_EM_HEIGHT_PX * viewbox_width / ACTION_ICON_VIEWBOX_HEIGHT;
    div().flex_none().flex().items_center().child(
        gpui::svg()
            .path(icon.path())
            .text_color(rgb(FOREGROUND))
            .w(px(width))
            .h(px(ACTION_ICON_EM_HEIGHT_PX)),
    )
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
