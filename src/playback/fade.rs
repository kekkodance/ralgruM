use std::time::Duration;

pub(super) const USER_FADE_DURATION: Duration = Duration::from_millis(150);
pub(super) const USER_FADE_FRAME: Duration = Duration::from_millis(10);
pub(super) const USER_FADE_BOUNDARY_MARGIN: Duration = Duration::from_millis(250);
pub(super) const USER_FADE_SETTLE_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UserToggleFadeDecision {
    Fade,
    Immediate,
    Boundary,
}

pub(super) fn user_toggle_fade_decision(
    standby_armed: bool,
    queued_sources: usize,
    duration: Duration,
    position: Duration,
) -> UserToggleFadeDecision {
    if !standby_armed {
        return UserToggleFadeDecision::Fade;
    }
    match queued_sources {
        0 | 1 => UserToggleFadeDecision::Boundary,
        2 if duration.saturating_sub(position) > USER_FADE_DURATION + USER_FADE_BOUNDARY_MARGIN => {
            UserToggleFadeDecision::Fade
        }
        _ => UserToggleFadeDecision::Immediate,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct UserFadePlan {
    epoch: u64,
}

#[derive(Debug, Default)]
pub(super) struct UserFadeSupervisor {
    epoch: u64,
    active: bool,
}

impl UserFadeSupervisor {
    pub(super) fn begin(&mut self) -> UserFadePlan {
        self.epoch = self.epoch.wrapping_add(1);
        self.active = true;
        UserFadePlan { epoch: self.epoch }
    }

    pub(super) fn is_current(&self, plan: UserFadePlan) -> bool {
        self.active && plan.epoch == self.epoch
    }

    pub(super) fn complete(&mut self, plan: UserFadePlan) -> bool {
        if !self.is_current(plan) {
            return false;
        }
        self.active = false;
        true
    }

    pub(super) fn cancel(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_plan_invalidates_the_previous_plan() {
        let mut supervisor = UserFadeSupervisor::default();
        let previous = supervisor.begin();
        let current = supervisor.begin();

        assert!(!supervisor.is_current(previous));
        assert!(supervisor.is_current(current));
    }

    #[test]
    fn completion_marks_only_the_current_plan_inactive() {
        let mut supervisor = UserFadeSupervisor::default();
        let plan = supervisor.begin();

        assert!(supervisor.is_current(plan));
        assert!(supervisor.complete(plan));
        assert!(!supervisor.is_current(plan));
        assert!(!supervisor.complete(plan));
    }

    #[test]
    fn cancellation_rejects_stale_completion() {
        let mut supervisor = UserFadeSupervisor::default();
        let plan = supervisor.begin();

        supervisor.cancel();

        assert!(!supervisor.is_current(plan));
        assert!(!supervisor.complete(plan));
    }

    #[test]
    fn armed_standby_uses_immediate_transport_inside_the_boundary_guard() {
        let threshold = USER_FADE_DURATION + USER_FADE_BOUNDARY_MARGIN;
        let duration = Duration::from_secs(30);

        assert_eq!(
            user_toggle_fade_decision(true, 2, duration, duration - threshold),
            UserToggleFadeDecision::Immediate
        );
        assert_eq!(
            user_toggle_fade_decision(
                true,
                2,
                duration,
                duration - threshold + Duration::from_millis(1)
            ),
            UserToggleFadeDecision::Immediate
        );
        assert_eq!(
            user_toggle_fade_decision(
                true,
                2,
                duration,
                duration - threshold - Duration::from_millis(1)
            ),
            UserToggleFadeDecision::Fade
        );
    }

    #[test]
    fn armed_standby_commits_after_the_source_count_changes() {
        for queued_sources in [0, 1] {
            assert_eq!(
                user_toggle_fade_decision(
                    true,
                    queued_sources,
                    Duration::from_secs(30),
                    Duration::ZERO
                ),
                UserToggleFadeDecision::Boundary
            );
        }
        assert_eq!(
            user_toggle_fade_decision(true, 3, Duration::from_secs(30), Duration::ZERO),
            UserToggleFadeDecision::Immediate
        );
    }

    #[test]
    fn unarmed_playback_can_fade_regardless_of_source_count_or_position() {
        assert_eq!(
            user_toggle_fade_decision(false, 0, Duration::from_secs(30), Duration::from_secs(30)),
            UserToggleFadeDecision::Fade
        );
    }
}
