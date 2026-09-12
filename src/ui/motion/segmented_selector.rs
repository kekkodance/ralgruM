use std::time::Instant;

#[cfg(test)]
use std::time::Duration;

use gpui::ease_in_out;

use super::{CONTENT_DURATION, clamp_unit, lerp};

/// The paint-ready state of an equal-width segmented selector indicator.
///
/// `position`, `from`, and `target` are expressed in segment-index units. A
/// consumer can place the indicator with `position / segment_count` and give
/// it a width of `1 / segment_count`. The endpoints stay stable for the
/// lifetime of an animation, so they can be captured by a GPUI animation
/// closure while `position` remains useful for retargeting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SegmentedSelectorVisual {
    pub(crate) position: f32,
    pub(crate) from: f32,
    pub(crate) target: f32,
    pub(crate) segment_count: usize,
    pub(crate) active: bool,
    pub(crate) epoch: u64,
}

impl SegmentedSelectorVisual {
    /// Returns the indicator's left edge as a fraction of the selector width.
    pub(crate) fn left_fraction(self) -> f32 {
        if self.segment_count == 0 {
            0.0
        } else {
            clamp_unit(self.position / self.segment_count as f32)
        }
    }

    /// Returns the width of one equal-width segment as a selector fraction.
    pub(crate) fn width_fraction(self) -> f32 {
        if self.segment_count == 0 {
            0.0
        } else {
            1.0 / self.segment_count as f32
        }
    }

    /// Returns an animation-ready left edge for the supplied animation delta.
    pub(crate) fn animated_left_fraction(self, delta: f32) -> f32 {
        if self.segment_count == 0 {
            0.0
        } else {
            clamp_unit(self.animated_position(delta) / self.segment_count as f32)
        }
    }

    /// Returns the eased index position for a selector animation frame.
    pub(crate) fn animated_position(self, delta: f32) -> f32 {
        lerp(self.from, self.target, ease_in_out(delta))
    }
}

/// Retargetable motion state for one equal-width segmented selector.
///
/// The state stores index units rather than pixel geometry. This means a
/// resize or relayout at the same selection only updates the segment count;
/// it cannot restart an in-flight selection animation. GPUI owns frame
/// scheduling when a consumer uses the returned `epoch` as its animation key.
#[derive(Clone, Debug)]
pub(crate) struct SegmentedSelectorMotion {
    previous_index: Option<usize>,
    current_index: usize,
    from: f32,
    target: f32,
    start: Option<Instant>,
    epoch: u64,
    active: bool,
    segment_count: usize,
}

impl Default for SegmentedSelectorMotion {
    fn default() -> Self {
        Self {
            previous_index: None,
            current_index: 0,
            from: 0.0,
            target: 0.0,
            start: None,
            epoch: 0,
            active: false,
            segment_count: 0,
        }
    }
}

impl SegmentedSelectorMotion {
    /// Updates the selected segment and returns the current indicator visual.
    ///
    /// The first selection settles immediately. Later selection changes start
    /// at the currently visible eased position, so rapid changes never jump
    /// back to the previous target. `segment_count` only affects clamping and
    /// rendering geometry, not whether a same-index animation restarts.
    pub(crate) fn prepare(
        &mut self,
        index: usize,
        segment_count: usize,
        now: Instant,
        reduced_motion: bool,
    ) -> SegmentedSelectorVisual {
        let selected_index = clamped_index(index, segment_count);

        if self.previous_index.is_none() {
            self.previous_index = Some(selected_index);
            self.current_index = selected_index;
            self.segment_count = segment_count;
            self.from = selected_index as f32;
            self.target = self.from;
            self.start = None;
            self.active = false;
            return self.visual(now);
        }

        self.segment_count = segment_count;

        if selected_index == self.current_index {
            if reduced_motion && self.active {
                self.settle();
            }
            return self.visual(now);
        }

        let visible_position = self.position_at(now);
        self.previous_index = Some(self.current_index);
        self.current_index = selected_index;
        self.from = visible_position;
        self.target = selected_index as f32;
        self.epoch = self.epoch.wrapping_add(1);

        if reduced_motion || self.from == self.target || CONTENT_DURATION.is_zero() {
            self.settle();
        } else {
            self.start = Some(now);
            self.active = true;
        }

        self.visual(now)
    }

    /// Returns the visual state at `now`, settling completed animations.
    pub(crate) fn visual(&mut self, now: Instant) -> SegmentedSelectorVisual {
        let position = if self.active {
            if self.progress(now) >= 1.0 {
                self.settle();
                self.target
            } else {
                self.position_at(now)
            }
        } else {
            self.target
        };

        SegmentedSelectorVisual {
            position,
            from: self.from,
            target: self.target,
            segment_count: self.segment_count,
            active: self.active,
            epoch: self.epoch,
        }
    }

    #[cfg(test)]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    fn position_at(&self, now: Instant) -> f32 {
        if !self.active {
            return self.target;
        }

        lerp(self.from, self.target, ease_in_out(self.progress(now)))
    }

    fn progress(&self, now: Instant) -> f32 {
        let Some(start) = self.start else {
            return 1.0;
        };

        let duration = CONTENT_DURATION;
        if duration.is_zero() {
            return 1.0;
        }

        let elapsed = now.saturating_duration_since(start);
        clamp_unit(elapsed.as_secs_f32() / duration.as_secs_f32())
    }

    fn settle(&mut self) {
        self.from = self.target;
        self.start = None;
        self.active = false;
    }
}

fn clamped_index(index: usize, segment_count: usize) -> usize {
    index.min(segment_count.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.0001;

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn first_selection_settles_immediately() {
        let now = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();

        let visual = motion.prepare(2, 4, now, false);

        assert_eq!(visual.position, 2.0);
        assert_eq!(visual.from, 2.0);
        assert_eq!(visual.target, 2.0);
        assert!(!visual.active);
        assert_eq!(visual.epoch, 0);
        assert_eq!(visual.left_fraction(), 0.5);
        assert_eq!(visual.width_fraction(), 0.25);
    }

    #[test]
    fn selection_change_animates_to_the_new_index() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();
        motion.prepare(0, 4, start, false);

        let visual = motion.prepare(3, 4, start + Duration::from_millis(1), false);

        assert_eq!(visual.from, 0.0);
        assert_eq!(visual.target, 3.0);
        assert!(visual.active);
        assert_eq!(visual.epoch, 1);
        assert_eq!(motion.epoch(), 1);

        let settled = motion.visual(start + Duration::from_millis(1) + CONTENT_DURATION);
        assert_eq!(settled.position, 3.0);
        assert!(!settled.active);
    }

    #[test]
    fn mid_animation_retargets_from_the_visible_position() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();
        motion.prepare(0, 4, start, false);
        motion.prepare(3, 4, start, false);

        let midpoint = start + Duration::from_millis(90);
        let before_retarget = motion.visual(midpoint);
        let retargeted = motion.prepare(1, 4, midpoint, false);

        assert_close(retargeted.from, before_retarget.position);
        assert_eq!(retargeted.target, 1.0);
        assert_eq!(retargeted.epoch, 2);
        assert!(retargeted.active);
    }

    #[test]
    fn rapid_retarget_does_not_snap_back_to_the_old_target() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();
        motion.prepare(0, 5, start, false);
        motion.prepare(4, 5, start, false);

        let first_change = start + Duration::from_millis(40);
        let first_visual = motion.visual(first_change);
        let second_visual = motion.prepare(1, 5, first_change, false);
        assert_close(second_visual.from, first_visual.position);

        let second_change = first_change + Duration::from_millis(40);
        let second_position = motion.visual(second_change).position;
        let third_visual = motion.prepare(3, 5, second_change, false);

        assert_close(third_visual.from, second_position);
        assert_eq!(third_visual.target, 3.0);
        assert_eq!(third_visual.epoch, 3);
    }

    #[test]
    fn reduced_motion_settles_initial_and_follow_up_selection() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();
        let first = motion.prepare(0, 3, start, true);
        let second = motion.prepare(2, 3, start, true);

        assert!(!first.active);
        assert!(!second.active);
        assert_eq!(second.position, 2.0);
        assert_eq!(second.from, 2.0);
        assert_eq!(second.target, 2.0);
        assert_eq!(second.epoch, 1);
    }

    #[test]
    fn same_selection_layout_change_does_not_restart_animation() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();
        motion.prepare(0, 4, start, false);
        let animating = motion.prepare(3, 4, start, false);
        let epoch = animating.epoch;

        let resized = motion.prepare(3, 6, start + Duration::from_millis(30), false);

        assert_eq!(resized.epoch, epoch);
        assert!(resized.active);
        assert_eq!(resized.segment_count, 6);
    }

    #[test]
    fn zero_and_one_segment_controls_are_safe() {
        let start = Instant::now();
        let mut motion = SegmentedSelectorMotion::default();

        let empty = motion.prepare(99, 0, start, false);
        assert_eq!(empty.position, 0.0);
        assert_eq!(empty.left_fraction(), 0.0);
        assert_eq!(empty.width_fraction(), 0.0);
        assert!(!empty.active);

        let one = motion.prepare(99, 1, start, false);
        assert_eq!(one.position, 0.0);
        assert_eq!(one.left_fraction(), 0.0);
        assert_eq!(one.width_fraction(), 1.0);
        assert_eq!(one.epoch, empty.epoch);

        let mut clamped = SegmentedSelectorMotion::default();
        let visual = clamped.prepare(99, 3, start, false);
        assert_eq!(visual.position, 2.0);
        assert_eq!(visual.left_fraction(), 2.0 / 3.0);
    }
}
