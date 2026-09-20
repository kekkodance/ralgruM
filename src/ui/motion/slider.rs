use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SliderVisual {
    pub(crate) from: f32,
    pub(crate) target: f32,
    pub(crate) epoch: u64,
    pub(crate) active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SliderMotionMode {
    Animated,
    Hold(f32),
    Direct,
}

#[derive(Debug)]
pub(crate) struct SliderMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for SliderMotion {
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

impl SliderMotion {
    pub(crate) fn prepare(
        &mut self,
        target: f32,
        now: Instant,
        reduced_motion: bool,
        mode: SliderMotionMode,
    ) -> SliderVisual {
        let target = target.clamp(0., 1.);

        if !self.initialized {
            self.initialized = true;
            let initial = match mode {
                SliderMotionMode::Hold(held) => held.clamp(0., 1.),
                SliderMotionMode::Animated | SliderMotionMode::Direct => target,
            };
            self.from = initial;
            self.target = initial;
            self.started_at = None;
        } else if let SliderMotionMode::Hold(held) = mode {
            let held = held.clamp(0., 1.);
            self.from = held;
            self.target = held;
            self.started_at = None;
        } else if mode == SliderMotionMode::Direct {
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

        SliderVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(crate) fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            super::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if super::INTERACTION_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / super::INTERACTION_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}
