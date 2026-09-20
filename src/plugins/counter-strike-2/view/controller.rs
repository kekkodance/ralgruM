use gpui::{AppContext as _, Context, Entity};
use gpui_component::slider::{SliderEvent, SliderState, SliderValue};

use super::slider::{ACTION_MUTE_POSITION, ACTION_SLIDER_MAX};
use super::{ActionSetting, Cs2SettingsDialog, FadeSetting};
use crate::dialog_layout::{DialogCloseMotion, DialogCloseTarget};
use crate::plugins::counter_strike_2::{
    config, service,
    settings::{PlaybackAction, Settings},
};

const ACTION_PAUSE_END: f32 = 5.0;
const ACTION_MUTE_END: f32 = 15.0;
const ACTION_VOLUME_START: f32 = 16.0;

impl Cs2SettingsDialog {
    pub(super) fn new(settings: Settings, cx: &mut Context<Self>) -> Self {
        let active_round = action_slider(settings.active_round, cx);
        let player_dead = action_slider(settings.player_dead, cx);
        let between_rounds = action_slider(settings.between_rounds, cx);
        let fade_out = fade_slider(settings.fade_out_ms, cx);
        let fade_in = fade_slider(settings.fade_in_ms, cx);

        subscribe_action_slider(&active_round, ActionSetting::ActiveRound, cx);
        subscribe_action_slider(&player_dead, ActionSetting::PlayerDead, cx);
        subscribe_action_slider(&between_rounds, ActionSetting::BetweenRounds, cx);
        subscribe_fade_slider(&fade_out, FadeSetting::Out, cx);
        subscribe_fade_slider(&fade_in, FadeSetting::In, cx);

        let executor = cx.background_executor().clone();
        let status_poll = cx.spawn(async move |this, cx| {
            loop {
                executor.timer(std::time::Duration::from_millis(500)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });

        Self {
            settings,
            active_round,
            player_dead,
            between_rounds,
            fade_out,
            fade_in,
            error: None,
            repairing: false,
            scroll: gpui::ScrollHandle::new(),
            browser_scroll: crate::browser_scroll::BrowserScrollState::new(),
            close_motion: DialogCloseMotion::default(),
            _status_poll: status_poll,
        }
    }

    fn update_action(
        &mut self,
        setting: ActionSetting,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        let value = match event {
            SliderEvent::Change(SliderValue::Single(value))
            | SliderEvent::Release(SliderValue::Single(value)) => *value,
            SliderEvent::Change(SliderValue::Range(_, _))
            | SliderEvent::Release(SliderValue::Range(_, _)) => return,
        };
        let action = action_from_position(value);
        match setting {
            ActionSetting::ActiveRound => self.settings.active_round = action,
            ActionSetting::PlayerDead => self.settings.player_dead = action,
            ActionSetting::BetweenRounds => self.settings.between_rounds = action,
        }
        if matches!(event, SliderEvent::Release(_)) {
            self.persist(cx);
        }
        cx.notify();
    }

    fn update_fade(&mut self, setting: FadeSetting, event: &SliderEvent, cx: &mut Context<Self>) {
        let value = match event {
            SliderEvent::Change(SliderValue::Single(value))
            | SliderEvent::Release(SliderValue::Single(value)) => *value,
            SliderEvent::Change(SliderValue::Range(_, _))
            | SliderEvent::Release(SliderValue::Range(_, _)) => return,
        };
        let milliseconds = (value.clamp(0.0, 5.0) * 1_000.0).round() as u32;
        match setting {
            FadeSetting::Out => self.settings.fade_out_ms = milliseconds,
            FadeSetting::In => self.settings.fade_in_ms = milliseconds,
        }
        if matches!(event, SliderEvent::Release(_)) {
            self.persist(cx);
        }
        cx.notify();
    }

    fn persist(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        let revision = settings.begin_background_persist();
        self.error = None;
        service::reapply_current(cx);
        let task = cx
            .background_executor()
            .spawn(async move { settings.persist_revision(revision) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if !Settings::revision_is_current(revision) {
                    return;
                }
                if let Err(error) = result {
                    this.error = Some(error.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn repair(&mut self, cx: &mut Context<Self>) {
        if self.repairing {
            return;
        }
        self.repairing = true;
        self.error = None;
        let settings = self.settings.clone();
        let task = cx
            .background_executor()
            .spawn(async move { config::install(&settings) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.repairing = false;
                match result {
                    Ok(_) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Counter-Strike 2 integration installed",
                        Some("Restart Counter-Strike 2 if it is currently running.".into()),
                    ),
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl DialogCloseTarget for Cs2SettingsDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

fn subscribe_action_slider(
    slider: &Entity<SliderState>,
    setting: ActionSetting,
    cx: &mut Context<Cs2SettingsDialog>,
) {
    cx.subscribe(slider, move |this, _, event: &SliderEvent, cx| {
        this.update_action(setting, event, cx)
    })
    .detach();
}

fn subscribe_fade_slider(
    slider: &Entity<SliderState>,
    setting: FadeSetting,
    cx: &mut Context<Cs2SettingsDialog>,
) {
    cx.subscribe(slider, move |this, _, event: &SliderEvent, cx| {
        this.update_fade(setting, event, cx)
    })
    .detach();
}

fn action_slider(
    action: PlaybackAction,
    cx: &mut Context<Cs2SettingsDialog>,
) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .min(0.0)
            .max(ACTION_SLIDER_MAX)
            .step(1.0)
            .default_value(action_position(action))
    })
}

fn fade_slider(milliseconds: u32, cx: &mut Context<Cs2SettingsDialog>) -> Entity<SliderState> {
    cx.new(|_| {
        SliderState::new()
            .min(0.0)
            .max(5.0)
            .step(0.1)
            .default_value(milliseconds as f32 / 1_000.0)
    })
}

fn action_from_position(value: f32) -> PlaybackAction {
    if value <= ACTION_PAUSE_END {
        PlaybackAction::Pause
    } else if value <= ACTION_MUTE_END {
        PlaybackAction::Mute
    } else {
        let fraction = ((value - ACTION_VOLUME_START) / (ACTION_SLIDER_MAX - ACTION_VOLUME_START))
            .clamp(0.0, 1.0);
        PlaybackAction::Volume((1.0 + fraction * 99.0).round() as u8)
    }
}

fn action_position(action: PlaybackAction) -> f32 {
    match action {
        PlaybackAction::Pause => 0.0,
        PlaybackAction::Mute => ACTION_MUTE_POSITION,
        PlaybackAction::Volume(percent) => {
            ACTION_VOLUME_START
                + (f32::from(percent.clamp(1, 100)) - 1.0) / 99.0
                    * (ACTION_SLIDER_MAX - ACTION_VOLUME_START)
        }
    }
}

/// Paint fraction for the action sliders, derived from the dialog's current
/// action rather than the raw slider entity so a drag cannot fight a snap.
pub(super) fn action_fraction(action: PlaybackAction) -> f32 {
    action_position(action) / ACTION_SLIDER_MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_slider_has_two_stable_detents_and_full_volume_range() {
        for action in [
            PlaybackAction::Pause,
            PlaybackAction::Mute,
            PlaybackAction::Volume(1),
            PlaybackAction::Volume(50),
            PlaybackAction::Volume(100),
        ] {
            assert_eq!(action_from_position(action_position(action)), action);
        }
        assert_eq!(action_from_position(3.0), PlaybackAction::Pause);
        assert_eq!(action_from_position(12.0), PlaybackAction::Mute);
    }
}
