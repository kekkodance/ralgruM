use std::time::Duration;

use gpui::{App, Entity, Global, WeakEntity};

use super::PlaybackModel;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AutomationDirective {
    Release,
    Gain(f32),
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AutomationTransition {
    pub directive: AutomationDirective,
    pub fade_out: Duration,
    pub fade_in: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AutomationState {
    pub(super) directive: AutomationDirective,
    pub(super) target_gain: f32,
    pub(super) paused_by_automation: bool,
    pub(super) epoch: u64,
}

impl Default for AutomationState {
    fn default() -> Self {
        Self {
            directive: AutomationDirective::Release,
            target_gain: 1.0,
            paused_by_automation: false,
            epoch: 0,
        }
    }
}

struct PlaybackGlobal(WeakEntity<PlaybackModel>);

impl Global for PlaybackGlobal {}

pub(crate) fn set_global(cx: &mut App, playback: &Entity<PlaybackModel>) {
    cx.set_global(PlaybackGlobal(playback.downgrade()));
}

pub(crate) fn apply_global(transition: AutomationTransition, cx: &mut App) {
    let Some(playback) = cx
        .try_global::<PlaybackGlobal>()
        .and_then(|global| global.0.upgrade())
    else {
        return;
    };
    playback.update(cx, |playback, cx| {
        playback.apply_automation_transition(transition, cx)
    });
}

pub(crate) fn release_global(cx: &mut App) {
    apply_global(
        AutomationTransition {
            directive: AutomationDirective::Release,
            fade_out: Duration::ZERO,
            fade_in: Duration::from_millis(300),
        },
        cx,
    );
}
