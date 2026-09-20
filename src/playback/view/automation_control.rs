use std::time::{Duration, Instant};

use gpui::Context;

use super::PlaybackModel;
use crate::playback::{
    PlaybackStatus,
    automation::{AutomationDirective, AutomationTransition},
    engine::AudioEngine,
};

const AUTOMATION_SETTLE_FRAME: Duration = Duration::from_millis(10);
const AUTOMATION_SETTLE_MARGIN: Duration = Duration::from_millis(250);

impl PlaybackModel {
    pub(crate) fn apply_automation_transition(
        &mut self,
        transition: AutomationTransition,
        cx: &mut Context<Self>,
    ) {
        self.automation_task.take();
        self.automation.epoch = self.automation.epoch.wrapping_add(1);
        self.automation.directive = transition.directive;

        match transition.directive {
            AutomationDirective::Release => {
                self.apply_automation_gain(1.0, transition.fade_in, cx);
            }
            AutomationDirective::Gain(gain) => {
                let gain = gain.clamp(0.0, 1.0);
                let duration = if gain < self.automation.target_gain {
                    transition.fade_out
                } else {
                    transition.fade_in
                };
                self.apply_automation_gain(gain, duration, cx);
            }
            AutomationDirective::Pause => self.apply_automation_pause(transition.fade_out, cx),
        }
    }

    pub(super) fn release_automation_for_manual_control(&mut self, reset_gain: bool) {
        self.automation_task.take();
        self.automation.epoch = self.automation.epoch.wrapping_add(1);
        self.automation.directive = AutomationDirective::Release;
        self.automation.target_gain = 1.0;
        self.automation.paused_by_automation = false;
        if reset_gain && let Ok(engine) = &self.engine {
            engine.reset_automation_gain(1.0);
        }
    }

    pub(super) fn reapply_automation_after_load(&mut self, cx: &mut Context<Self>) {
        let transition = match self.automation.directive {
            AutomationDirective::Release => return,
            AutomationDirective::Gain(gain) => AutomationTransition {
                directive: AutomationDirective::Gain(gain),
                fade_out: Duration::ZERO,
                fade_in: Duration::ZERO,
            },
            AutomationDirective::Pause => AutomationTransition {
                directive: AutomationDirective::Pause,
                fade_out: Duration::ZERO,
                fade_in: Duration::ZERO,
            },
        };
        self.apply_automation_transition(transition, cx);
    }

    fn apply_automation_gain(&mut self, target: f32, duration: Duration, cx: &mut Context<Self>) {
        let was_paused_by_automation = self.automation.paused_by_automation;
        self.automation.paused_by_automation = false;
        self.automation.target_gain = target;

        if was_paused_by_automation && self.state.status == PlaybackStatus::Paused {
            let _ = self.state.toggle();
            if let Some(session) = self.deezer_listen.as_mut() {
                session.set_playing(true);
            }
            if let Ok(engine) = &self.engine {
                engine.set_volume(self.state.volume);
                engine.play();
            }
            self.sync_discord();
        }
        if let Ok(engine) = &self.engine {
            engine.set_automation_gain_target(target, duration);
        }
        cx.notify();
    }

    fn apply_automation_pause(&mut self, duration: Duration, cx: &mut Context<Self>) {
        self.automation.target_gain = 0.0;
        if let Ok(engine) = &self.engine {
            engine.set_automation_gain_target(0.0, duration);
        }

        if self.state.status != PlaybackStatus::Playing {
            if self.state.status != PlaybackStatus::Paused {
                self.automation.paused_by_automation = false;
            }
            return;
        }

        let epoch = self.automation.epoch;
        let generation = self.state.generation;
        let executor = cx.background_executor().clone();
        let started = Instant::now();
        let timeout = duration.saturating_add(AUTOMATION_SETTLE_MARGIN);
        self.automation_task = Some(cx.spawn(async move |this, cx| {
            if !duration.is_zero() {
                executor.timer(duration).await;
            }
            loop {
                let finished = this
                    .update(cx, |this, cx| {
                        if this.automation.epoch != epoch
                            || this.state.generation != generation
                            || this.automation.directive != AutomationDirective::Pause
                        {
                            return true;
                        }
                        let settled = this
                            .engine
                            .as_ref()
                            .is_ok_and(|engine| engine.automation_gain_settled(0.0));
                        if !settled && started.elapsed() < timeout {
                            return false;
                        }
                        if this.state.status == PlaybackStatus::Playing {
                            if !settled && let Ok(engine) = &this.engine {
                                engine.reset_automation_gain(0.0);
                            }
                            let _ = this.state.toggle();
                            if let Some(session) = this.deezer_listen.as_mut() {
                                session.set_playing(false);
                            }
                            if let Ok(engine) = &this.engine {
                                engine.pause();
                            }
                            this.automation.paused_by_automation = true;
                            this.sync_discord();
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
                executor.timer(AUTOMATION_SETTLE_FRAME).await;
            }
        }));
    }
}
