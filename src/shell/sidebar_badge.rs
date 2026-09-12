use std::time::Instant;

use crate::motion::{self, ResponsiveModeMotion, ResponsiveModeVisual};

pub(super) const DOWNLOAD_BADGE_COMPACT_RIGHT_PX: f32 = 4.;
pub(super) const DOWNLOAD_BADGE_EXPANDED_RIGHT_PX: f32 = 9.;
pub(super) const DOWNLOAD_BADGE_COMPACT_TOP_PX: f32 = 6.;
pub(super) const DOWNLOAD_BADGE_EXPANDED_TOP_PX: f32 = 12.5;
pub(super) const DOWNLOAD_BADGE_NUMERAL_OFFSET_PX: f32 = 1.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarDownloadBadgePresenceVisual {
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) hidden: bool,
    pub(super) count: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SidebarDownloadBadgePresenceMotion {
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
    initialized: bool,
    count: usize,
}

impl SidebarDownloadBadgePresenceMotion {
    fn displayed_at(&self, now: Instant) -> f32 {
        if !self.initialized {
            return self.target;
        }
        let Some(started_at) = self.started_at else {
            return self.target;
        };
        let duration = motion::CONTENT_DURATION;
        if duration.is_zero() {
            return self.target;
        }
        let progress = motion::clamp_unit(
            now.saturating_duration_since(started_at).as_secs_f32() / duration.as_secs_f32(),
        );
        motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
    }

    fn prepare(
        &mut self,
        count: usize,
        now: Instant,
        reduced_motion: bool,
    ) -> SidebarDownloadBadgePresenceVisual {
        let target = (count > 0) as u8 as f32;
        if !self.initialized {
            self.from = target;
            self.target = target;
            self.started_at = None;
            self.initialized = true;
            self.count = count;
        } else {
            let displayed = self.displayed_at(now);
            if self.target != target {
                self.from = displayed;
                self.target = target;
                self.epoch = self.epoch.wrapping_add(1);
                self.started_at = (!reduced_motion).then_some(now);
            } else if self.started_at.is_some_and(|started_at| {
                now.saturating_duration_since(started_at) >= motion::CONTENT_DURATION
            }) {
                self.from = self.target;
                self.started_at = None;
            }
            if target > 0. {
                self.count = count;
            }
        }

        if reduced_motion || motion::CONTENT_DURATION.is_zero() {
            self.from = self.target;
            self.started_at = None;
        }

        SidebarDownloadBadgePresenceVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            hidden: self.target == 0. && self.from == 0. && self.started_at.is_none(),
            count: self.count,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarDownloadBadgeVisual {
    pub(super) responsive: ResponsiveModeVisual,
    pub(super) presence: SidebarDownloadBadgePresenceVisual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct SidebarDownloadBadgeMotion {
    responsive: ResponsiveModeMotion,
    presence: SidebarDownloadBadgePresenceMotion,
}

impl SidebarDownloadBadgeMotion {
    pub(super) fn prepare(
        &mut self,
        compact: bool,
        count: usize,
        now: Instant,
        reduced_motion: bool,
    ) -> SidebarDownloadBadgeVisual {
        SidebarDownloadBadgeVisual {
            responsive: self.responsive.prepare(compact, now, reduced_motion),
            presence: self.presence.prepare(count, now, reduced_motion),
        }
    }
}

pub(super) fn download_badge_position_endpoints(
    responsive: ResponsiveModeVisual,
) -> ((f32, f32), (f32, f32)) {
    (
        responsive.endpoints(
            DOWNLOAD_BADGE_EXPANDED_TOP_PX,
            DOWNLOAD_BADGE_COMPACT_TOP_PX,
        ),
        responsive.endpoints(
            DOWNLOAD_BADGE_EXPANDED_RIGHT_PX,
            DOWNLOAD_BADGE_COMPACT_RIGHT_PX,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DOWNLOAD_BADGE_COMPACT_RIGHT_PX, DOWNLOAD_BADGE_COMPACT_TOP_PX,
        DOWNLOAD_BADGE_EXPANDED_RIGHT_PX, DOWNLOAD_BADGE_EXPANDED_TOP_PX,
        DOWNLOAD_BADGE_NUMERAL_OFFSET_PX, SidebarDownloadBadgeMotion,
        download_badge_position_endpoints,
    };
    use crate::motion::{self, ResponsiveModeVisual};
    use std::time::{Duration, Instant};

    #[test]
    fn badge_geometry_matches_mode_specific_nav_insets() {
        assert_eq!(DOWNLOAD_BADGE_COMPACT_RIGHT_PX, 4.);
        assert_eq!(DOWNLOAD_BADGE_EXPANDED_RIGHT_PX, 9.);
        assert_eq!(DOWNLOAD_BADGE_COMPACT_TOP_PX, 6.);
        assert_eq!(DOWNLOAD_BADGE_EXPANDED_TOP_PX, 12.5);
        assert_eq!(DOWNLOAD_BADGE_NUMERAL_OFFSET_PX, 1.);

        let expanded = ResponsiveModeVisual {
            from: 0.,
            target: 0.,
            target_compact: false,
            epoch: 0,
        };
        let compact = ResponsiveModeVisual {
            from: 1.,
            target: 1.,
            target_compact: true,
            epoch: 0,
        };
        assert_eq!(
            download_badge_position_endpoints(expanded),
            ((12.5, 12.5), (9., 9.))
        );
        assert_eq!(
            download_badge_position_endpoints(compact),
            ((6., 6.), (4., 4.))
        );
    }

    #[test]
    fn responsive_badge_position_retargets_and_reverses_from_the_live_value() {
        let now = Instant::now();
        let mut motion = SidebarDownloadBadgeMotion::default();
        motion.prepare(false, 0, now, false);
        let compact = motion.prepare(true, 0, now, false);
        assert_eq!(compact.responsive.epoch, 1);
        assert_eq!(
            download_badge_position_endpoints(compact.responsive),
            ((12.5, 6.), (9., 4.))
        );

        let midpoint = now + motion::CONTENT_DURATION / 2;
        let reversed = {
            let mut motion = motion;
            motion.prepare(false, 0, midpoint, false)
        };
        let expected = motion::lerp(0., 1., 0.5);
        assert!((reversed.responsive.from - expected).abs() < 0.0001);
        assert_eq!(reversed.responsive.target, 0.);
        assert_eq!(reversed.responsive.epoch, 2);
        assert_eq!(
            download_badge_position_endpoints(reversed.responsive),
            ((9.25, 12.5), (6.5, 9.))
        );
    }

    #[test]
    fn badge_presence_fades_in_and_retains_count_during_fade_out() {
        let now = Instant::now();
        let mut motion = SidebarDownloadBadgeMotion::default();
        let hidden = motion.prepare(false, 0, now, false);
        assert!(hidden.presence.hidden);
        assert_eq!(hidden.presence.count, 0);

        let entered = motion.prepare(false, 3, now, false);
        assert_eq!((entered.presence.from, entered.presence.target), (0., 1.));
        assert!(!entered.presence.hidden);
        assert_eq!(entered.presence.count, 3);

        let cleared = motion.prepare(false, 0, now + Duration::from_millis(40), false);
        assert!(cleared.presence.from > 0.);
        assert_eq!(cleared.presence.target, 0.);
        assert!(!cleared.presence.hidden);
        assert_eq!(cleared.presence.count, 3);

        let settled = motion.prepare(
            false,
            0,
            now + Duration::from_millis(40) + motion::CONTENT_DURATION,
            false,
        );
        assert!(settled.presence.hidden);
        assert_eq!(settled.presence.count, 3);
    }

    #[test]
    fn badge_presence_reversal_starts_from_the_current_opacity() {
        let now = Instant::now();
        let mut motion = SidebarDownloadBadgeMotion::default();
        motion.prepare(false, 1, now, false);
        let cleared = motion.prepare(false, 0, now, false);
        assert_eq!(cleared.presence.from, 1.);
        let reversed = motion.prepare(false, 2, now + motion::CONTENT_DURATION / 2, false);
        assert!(reversed.presence.from > 0. && reversed.presence.from < 1.);
        assert_eq!(reversed.presence.target, 1.);
        assert_eq!(reversed.presence.count, 2);
    }

    #[test]
    fn reduced_motion_snaps_badge_position_and_presence() {
        let now = Instant::now();
        let mut motion = SidebarDownloadBadgeMotion::default();
        motion.prepare(false, 0, now, true);
        let visual = motion.prepare(true, 4, now, true);
        assert_eq!(visual.responsive.from, 1.);
        assert_eq!(visual.responsive.target, 1.);
        assert_eq!(visual.presence.from, 1.);
        assert_eq!(visual.presence.target, 1.);
        assert!(!visual.presence.hidden);
    }
}
