use gpui::{
    AnimationExt, Context, Entity, FontWeight, IntoElement, KeyDownEvent, Render, SharedString,
    Window, div, prelude::*, px, relative, rgb, rgba,
};
use gpui_component::slider::SliderState;

use super::Cs2SettingsDialog;
use super::controller::action_fraction;
use super::slider::{
    ACTION_MUTE_POSITION, ACTION_SLIDER_MAX, SLIDER_DETENT_COLOR, cs2_action_slider, cs2_slider,
    slider_fraction,
};
use crate::{
    app_button::secondary_dialog_button_with_disabled,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    dialog_layout::request_dialog_close,
    plugins::counter_strike_2::{service, settings::PlaybackAction, state::MatchState},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED, PRIMARY},
};

const SUCCESS: u32 = 0x22c55e;

/// Height of the row of snap markers below the action sliders.
const ACTION_LABEL_ROW_HEIGHT_PX: f32 = 18.0;

/// Icon height for the snap markers below the action sliders, matching the
/// text size of the old labels. Widths follow each icon's viewbox aspect.
const SNAP_ICON_HEIGHT_PX: f32 = 10.5;
const SNAP_ICON_VIEWBOX_HEIGHT: f32 = 512.;
const SNAP_ICON_PAUSE_WIDTH_PX: f32 = SNAP_ICON_HEIGHT_PX * 384. / SNAP_ICON_VIEWBOX_HEIGHT;
const SNAP_ICON_MUTE_WIDTH_PX: f32 = SNAP_ICON_HEIGHT_PX * 576. / SNAP_ICON_VIEWBOX_HEIGHT;
const SNAP_ICON_MAX_WIDTH_PX: f32 = SNAP_ICON_HEIGHT_PX * 640. / SNAP_ICON_VIEWBOX_HEIGHT;

/// Vertical space the dialog keeps for its header and footer when clamping
/// the scrollable body against the viewport.
const DIALOG_CHROME_HEIGHT: f32 = 160.;

impl Render for Cs2SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                .pl(px(3.0))
                .pr(px(8.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(cs2_action_slider(slider, action_fraction(action)))
                .child(action_marker_row()),
        )
}

/// Row of snap markers below an action slider. It lives inside the same
/// wrapper as the slider, so its edges line up with the track and the
/// marker centers land on the exact x positions of the detent dots and the
/// thumb rest positions. Taffy positions absolute children against the
/// padding box, so the row itself must not add padding.
fn action_marker_row() -> impl IntoElement {
    // Fractions of the track where each marker sits: Pause at 0, Mute at its
    // snap point, and the max-volume icon at the right end where the thumb
    // rests at 100%.
    let pause_fraction = 0.0;
    let mute_fraction = ACTION_MUTE_POSITION / ACTION_SLIDER_MAX;
    let max_fraction = 1.0;
    div()
        .relative()
        .h(px(ACTION_LABEL_ROW_HEIGHT_PX))
        // The negative half-width margins center each icon on its detent.
        // Each icon sits below a 1px tick that marks the detent x.
        .child(snap_marker(
            pause_fraction,
            LocalIcon::Pause,
            SNAP_ICON_PAUSE_WIDTH_PX,
        ))
        .child(snap_marker(
            mute_fraction,
            LocalIcon::VolumeMuted,
            SNAP_ICON_MUTE_WIDTH_PX,
        ))
        .child(snap_marker(
            max_fraction,
            LocalIcon::VolumeHigh,
            SNAP_ICON_MAX_WIDTH_PX,
        ))
}

/// One snap marker: a 1px tick at the top of the row plus the icon centered
/// below it. Both hang off a zero-width anchor placed at the marker fraction,
/// so the tick and the icon center share the exact same x.
fn snap_marker(fraction: f32, icon: LocalIcon, icon_width: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(relative(fraction))
        .top_0()
        .child(
            div()
                .absolute()
                .left(px(-0.5))
                .top_0()
                .w(px(1.0))
                .h(px(4.0))
                .bg(rgb(SLIDER_DETENT_COLOR)),
        )
        .child(
            div()
                .absolute()
                .left(px(-icon_width * 0.5))
                .top(px(6.0))
                .child(snap_icon(icon, icon_width)),
        )
}

fn snap_icon(icon: LocalIcon, width: f32) -> impl IntoElement {
    gpui::svg()
        .path(icon.path())
        .text_color(rgb(MUTED))
        .w(px(width))
        .h(px(SNAP_ICON_HEIGHT_PX))
}

fn value_badge(value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .justify_center()
        .min_w(px(50.0))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(5.0))
        .bg(rgba(0x6366f126))
        .text_size(px(11.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(PRIMARY))
        .child(value.into())
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
