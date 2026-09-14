use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, IntoElement, div, ease_in_out, ease_out_quint, prelude::*,
    pulsating_between, px, relative, rgba,
};

mod segmented_selector;

pub(crate) use segmented_selector::{SegmentedSelectorMotion, SegmentedSelectorVisual};

pub(crate) const INTERACTION_DURATION: Duration = Duration::from_millis(100);
pub(crate) const QUICK_CONTENT_DURATION: Duration = Duration::from_millis(140);
pub(crate) const CONTENT_DURATION: Duration = Duration::from_millis(180);
pub(crate) const PANEL_DURATION: Duration = Duration::from_millis(220);
pub(crate) const DIALOG_DURATION: Duration = Duration::from_millis(250);
pub(crate) const ANIMATION_FRAME_DURATION: Duration = Duration::from_millis(17);
pub(crate) const PANEL_SETTLING_DURATION: Duration =
    PANEL_DURATION.saturating_add(ANIMATION_FRAME_DURATION);
pub(crate) const SKELETON_LOOP_DURATION: Duration = Duration::from_millis(1400);
/// How long a freshly loaded artwork image takes to fade in. The placeholder
/// beneath it stays visible for the whole window.
pub(crate) const ARTWORK_REVEAL_DURATION: Duration = Duration::from_millis(180);
/// How long the carousel's horizontal scrollbar and both arrows take to
/// fade in or out together once the row starts or stops overflowing.
pub(crate) const CAROUSEL_CONTROLS_FADE_DURATION: Duration = Duration::from_millis(180);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResponsiveModeVisual {
    pub(crate) from: f32,
    pub(crate) target: f32,
    pub(crate) target_compact: bool,
    pub(crate) epoch: u64,
}

impl ResponsiveModeVisual {
    pub(crate) fn endpoints(self, full: f32, compact: f32) -> (f32, f32) {
        (
            lerp(full, compact, self.from),
            lerp(full, compact, self.target),
        )
    }

    #[cfg(test)]
    pub(crate) fn value_at(self, full: f32, compact: f32, delta: f32) -> f32 {
        let (from, target) = self.endpoints(full, compact);
        lerp(from, target, delta)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ResponsiveModeMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl ResponsiveModeMotion {
    fn displayed_at(&self, now: Instant) -> f32 {
        if !self.initialized {
            return self.target;
        }

        let Some(started_at) = self.started_at else {
            return self.target;
        };

        let elapsed = now.saturating_duration_since(started_at);
        if elapsed >= CONTENT_DURATION {
            return self.target;
        }

        let duration = CONTENT_DURATION.as_secs_f32();
        let progress = if duration == 0.0 {
            1.0
        } else {
            (elapsed.as_secs_f32() / duration).clamp(0.0, 1.0)
        };
        lerp(self.from, self.target, ease_in_out(progress))
    }

    pub(crate) fn prepare(
        &mut self,
        target_compact: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> ResponsiveModeVisual {
        let target = if target_compact { 1.0 } else { 0.0 };

        if !self.initialized {
            self.initialized = true;
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if self.target != target {
            self.from = self.displayed_at(now);
            self.target = target;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion).then_some(now);
        }

        if reduced_motion {
            self.from = self.target;
            self.started_at = None;
        } else if let Some(started_at) = self.started_at
            && now.saturating_duration_since(started_at) >= CONTENT_DURATION
        {
            self.from = self.target;
            self.started_at = None;
        }

        ResponsiveModeVisual {
            from: self.from,
            target: self.target,
            target_compact,
            epoch: self.epoch,
        }
    }
}

fn one_shot(duration: Duration, easing: impl Fn(f32) -> f32 + 'static) -> Animation {
    Animation::new(duration).with_easing(easing)
}

pub(crate) fn interaction() -> Animation {
    one_shot(INTERACTION_DURATION, ease_out_quint())
}

pub(crate) fn quick_content() -> Animation {
    one_shot(QUICK_CONTENT_DURATION, ease_out_quint())
}

pub(crate) fn dialog_close() -> Animation {
    let opening = gpui_component::animation::cubic_bezier(0.32, 0.72, 0., 1.);
    one_shot(DIALOG_DURATION, move |delta| 1. - opening(1. - delta))
}

pub(crate) fn content() -> Animation {
    one_shot(CONTENT_DURATION, ease_in_out)
}

pub(crate) fn panel() -> Animation {
    one_shot(PANEL_DURATION, ease_out_quint())
}

pub(crate) fn relocation_fade() -> Animation {
    one_shot(PANEL_DURATION, |delta| delta)
}

pub(crate) fn skeleton_loop() -> Animation {
    Animation::new(SKELETON_LOOP_DURATION)
        .repeat_synced()
        .with_easing(pulsating_between(0.0, 1.0))
        .with_max_fps(30.0)
}

pub(crate) fn segmented_selector_indicator(visual: SegmentedSelectorVisual) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left(relative(visual.left_fraction()))
        .w(relative(visual.width_fraction()))
        .h(px(30.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x818cf8d9))
        .bg(rgba(0x6366f13d))
        .with_animation(
            ("segmented-selector-indicator", visual.epoch),
            content(),
            move |this, delta| this.left(relative(visual.animated_left_fraction(delta))),
        )
}

pub(crate) fn clamp_unit(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

pub(crate) fn lerp(start: f32, end: f32, amount: f32) -> f32 {
    start + (end - start) * clamp_unit(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_unit_limits_values_to_the_unit_interval() {
        assert_eq!(clamp_unit(-0.5), 0.0);
        assert_eq!(clamp_unit(0.25), 0.25);
        assert_eq!(clamp_unit(1.5), 1.0);
    }

    #[test]
    fn lerp_interpolates_and_clamps_progress() {
        assert_eq!(lerp(10.0, 30.0, 0.25), 15.0);
        assert_eq!(lerp(10.0, 30.0, -1.0), 10.0);
        assert_eq!(lerp(10.0, 30.0, 2.0), 30.0);
    }

    #[test]
    fn responsive_mode_first_render_is_settled() {
        let now = Instant::now();
        let mut motion = ResponsiveModeMotion::default();
        let visual = motion.prepare(true, now, false);

        assert_eq!(visual.from, 1.0);
        assert_eq!(visual.target, 1.0);
        assert!(visual.target_compact);
        assert_eq!(visual.epoch, 0);
    }

    #[test]
    fn responsive_mode_change_persists_until_the_next_change() {
        let now = Instant::now();
        let mut motion = ResponsiveModeMotion::default();
        motion.prepare(false, now, false);
        let changed = motion.prepare(true, now, false);
        let repeated = motion.prepare(true, now + Duration::from_millis(50), false);

        assert_eq!(changed.from, 0.0);
        assert_eq!(changed.target, 1.0);
        assert!(changed.target_compact);
        assert_eq!(changed.epoch, 1);
        assert_eq!(repeated, changed);
    }

    #[test]
    fn responsive_mode_reverse_starts_from_the_live_displayed_value() {
        let now = Instant::now();
        let midpoint = now + Duration::from_millis(90);
        let mut motion = ResponsiveModeMotion::default();
        motion.prepare(false, now, false);
        motion.prepare(true, now, false);
        let displayed = motion.displayed_at(midpoint);
        let reversed = motion.prepare(false, midpoint, false);

        assert!((reversed.from - displayed).abs() < f32::EPSILON);
        assert_eq!(reversed.target, 0.0);
        assert!(!reversed.target_compact);
        assert_eq!(reversed.epoch, 2);
    }

    #[test]
    fn responsive_mode_completion_settles_the_transition() {
        let now = Instant::now();
        let mut motion = ResponsiveModeMotion::default();
        motion.prepare(false, now, false);
        motion.prepare(true, now, false);
        let visual = motion.prepare(true, now + CONTENT_DURATION, false);

        assert_eq!(visual.from, 1.0);
        assert_eq!(visual.target, 1.0);
        assert_eq!(visual.epoch, 1);
        assert_eq!(motion.displayed_at(now + CONTENT_DURATION), 1.0);
    }

    #[test]
    fn responsive_mode_reduced_motion_settles_the_endpoints() {
        let now = Instant::now();
        let mut motion = ResponsiveModeMotion::default();
        motion.prepare(false, now, false);
        let visual = motion.prepare(true, now + Duration::from_millis(1), true);

        assert_eq!(visual.from, 1.0);
        assert_eq!(visual.target, 1.0);
        assert_eq!(visual.epoch, 1);
        assert_eq!(motion.started_at, None);
    }

    #[test]
    fn responsive_mode_visual_interpolates_compactness_then_animation_delta() {
        let visual = ResponsiveModeVisual {
            from: 0.25,
            target: 0.75,
            target_compact: true,
            epoch: 4,
        };

        assert_eq!(visual.endpoints(100.0, 40.0), (85.0, 55.0));
        assert_eq!(visual.value_at(100.0, 40.0, 0.5), 70.0);
    }
}
