use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BufferedVisual {
    pub(super) generation: u64,
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

#[derive(Debug)]
pub(super) struct BufferedMotion {
    pub(super) generation: Option<u64>,
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl Default for BufferedMotion {
    fn default() -> Self {
        Self {
            generation: None,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl BufferedMotion {
    pub(super) fn prepare(
        &mut self,
        generation: u64,
        target: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> BufferedVisual {
        let target = target.clamp(0., 1.);
        let changed = self.generation != Some(generation) || self.target != target;

        if changed {
            let displayed = self.displayed_at(now);
            self.generation = Some(generation);
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

        BufferedVisual {
            generation,
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::CONTENT_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::CONTENT_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SeekFillVisual {
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

impl SeekFillVisual {
    pub(super) fn direct(progress: f32) -> Self {
        let progress = progress.clamp(0., 1.);
        Self {
            from: progress,
            target: progress,
            epoch: 0,
            active: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct SeekFillMotion {
    pub(super) last_epoch: u64,
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl Default for SeekFillMotion {
    fn default() -> Self {
        Self {
            last_epoch: 0,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl SeekFillMotion {
    pub(super) fn reset(&mut self, last_epoch: u64) {
        *self = Self {
            last_epoch,
            ..Self::default()
        };
    }

    pub(super) fn set_displayed(&mut self, progress: f32) {
        let progress = progress.clamp(0., 1.);
        self.from = progress;
        self.target = progress;
        self.started_at = None;
    }

    pub(super) fn prepare(
        &mut self,
        commit_epoch: u64,
        from: f32,
        target: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> SeekFillVisual {
        let from = from.clamp(0., 1.);
        let target = target.clamp(0., 1.);

        if self.last_epoch != commit_epoch {
            let displayed = if self.started_at.is_some() {
                self.displayed_at(now)
            } else {
                from
            };
            self.last_epoch = commit_epoch;
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

        SeekFillVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::INTERACTION_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::INTERACTION_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerBarVisual {
    pub(super) from_height: f32,
    pub(super) target_height: f32,
    pub(super) from_opacity: f32,
    pub(super) target_opacity: f32,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

#[derive(Debug)]
pub(super) struct PlayerBarMotion {
    pub(super) open: Option<bool>,
    pub(super) narrow: Option<bool>,
    pub(super) from_height: f32,
    pub(super) target_height: f32,
    pub(super) from_opacity: f32,
    pub(super) target_opacity: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl Default for PlayerBarMotion {
    fn default() -> Self {
        Self {
            open: None,
            narrow: None,
            from_height: 0.,
            target_height: 0.,
            from_opacity: 0.,
            target_opacity: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl PlayerBarMotion {
    pub(super) fn prepare(
        &mut self,
        open: bool,
        narrow: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> PlayerBarVisual {
        let target_height = if open && !narrow {
            PLAYER_BAR_DESKTOP_HEIGHT_PX
        } else {
            0.
        };
        let target_opacity = if open { 1. } else { 0. };
        let changed = self.open != Some(open)
            || self.narrow != Some(narrow)
            || self.target_height != target_height
            || self.target_opacity != target_opacity;

        if changed {
            let (displayed_height, displayed_opacity) = self.displayed_at(now);
            self.open = Some(open);
            self.narrow = Some(narrow);
            self.from_height = if narrow {
                target_height
            } else {
                displayed_height
            };
            self.target_height = target_height;
            self.from_opacity = if open {
                target_opacity
            } else {
                displayed_opacity
            };
            self.target_opacity = target_opacity;
            self.epoch = self.epoch.wrapping_add(1);
            let needs_animation = (!narrow && self.from_height != target_height)
                || self.from_opacity != target_opacity;
            self.started_at = (!reduced_motion && needs_animation).then_some(now);
            if reduced_motion || !needs_animation {
                self.from_height = target_height;
                self.from_opacity = target_opacity;
                self.started_at = None;
            }
        } else if reduced_motion
            || (self.started_at.is_some() && self.animation_progress(now) >= 1.)
        {
            self.from_height = self.target_height;
            self.from_opacity = self.target_opacity;
            self.started_at = None;
        }

        PlayerBarVisual {
            from_height: self.from_height,
            target_height: self.target_height,
            from_opacity: self.from_opacity,
            target_opacity: self.target_opacity,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> (f32, f32) {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            (self.target_height, self.target_opacity)
        } else {
            let progress = panel_motion_ease(progress);
            (
                crate::motion::lerp(self.from_height, self.target_height, progress),
                crate::motion::lerp(self.from_opacity, self.target_opacity, progress),
            )
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::PANEL_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::PANEL_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

pub(super) fn panel_motion_ease(progress: f32) -> f32 {
    1. - (1. - progress.clamp(0., 1.)).powi(5)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerBarLayout {
    pub(super) padding: f32,
    pub(super) column_gap: f32,
    pub(super) center_width: f32,
    pub(super) side_width: f32,
}

impl PlayerBarLayout {
    pub(super) fn from_geometry(geometry: (f32, f32, f32, f32)) -> Self {
        let (padding, column_gap, center_width, side_width) = geometry;
        Self {
            padding,
            column_gap,
            center_width,
            side_width,
        }
    }

    pub(super) fn geometry(self) -> (f32, f32, f32, f32) {
        (
            self.padding,
            self.column_gap,
            self.center_width,
            self.side_width,
        )
    }

    pub(super) fn at(self, target: Self, delta: f32) -> Self {
        Self {
            padding: crate::motion::lerp(self.padding, target.padding, delta),
            column_gap: crate::motion::lerp(self.column_gap, target.column_gap, delta),
            center_width: crate::motion::lerp(self.center_width, target.center_width, delta),
            side_width: crate::motion::lerp(self.side_width, target.side_width, delta),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PlayerBarLayoutVisual {
    pub(super) from: PlayerBarLayout,
    pub(super) target: PlayerBarLayout,
    pub(super) epoch: u64,
    pub(super) active: bool,
    pub(super) mode_changed: bool,
}

impl PlayerBarLayoutVisual {
    pub(super) fn at(self, delta: f32) -> PlayerBarLayout {
        self.from.at(self.target, delta)
    }
}

#[derive(Debug)]
pub(super) struct PlayerBarLayoutMotion {
    pub(super) compact: Option<bool>,
    pub(super) from: PlayerBarLayout,
    pub(super) target: PlayerBarLayout,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl Default for PlayerBarLayoutMotion {
    fn default() -> Self {
        let layout = PlayerBarLayout::from_geometry((0., 0., 0., 0.));
        Self {
            compact: None,
            from: layout,
            target: layout,
            epoch: 0,
            started_at: None,
        }
    }
}

impl PlayerBarLayoutMotion {
    pub(super) fn prepare(
        &mut self,
        compact: bool,
        target: PlayerBarLayout,
        now: Instant,
        reduced_motion: bool,
    ) -> PlayerBarLayoutVisual {
        let mode_changed = self.compact.is_some() && self.compact != Some(compact);
        let geometry_changed = self.target != target;
        if self.compact.is_none() {
            self.compact = Some(compact);
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if mode_changed {
            self.from = self.displayed_at(now);
            self.target = target;
            self.compact = Some(compact);
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && self.from != self.target).then_some(now);
        } else if geometry_changed {
            if self.started_at.is_some() {
                // Live WM_SIZE events are part of the same mode transition.
                // Update only the destination so the original animation clock,
                // easing phase, and element identity keep advancing instead of
                // restarting on every resize frame.
                self.target = target;
            } else {
                // A resize within an idle mode should track the viewport directly.
                self.from = target;
                self.target = target;
                self.started_at = None;
            }
        }

        if reduced_motion || (self.started_at.is_some() && self.animation_progress(now) >= 1.) {
            self.from = self.target;
            self.started_at = None;
        }

        PlayerBarLayoutVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
            mode_changed,
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> PlayerBarLayout {
        if self.started_at.is_none() || self.animation_progress(now) >= 1. {
            self.target
        } else {
            self.from
                .at(self.target, panel_motion_ease(self.animation_progress(now)))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::PANEL_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::PANEL_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}
