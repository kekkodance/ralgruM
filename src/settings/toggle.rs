use std::time::Duration;

use gpui::{App, CursorStyle, ElementId, Window, prelude::*};
use gpui_component::switch::Switch;

/// Keep this in sync with the thumb movement used by gpui-component's Switch.
pub(crate) const SETTINGS_SWITCH_TRANSITION: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MotionTransition {
    Unchanged,
    EnableImmediately,
    DisableAfter(Duration),
}

pub(crate) fn motion_transition(previous_reduced: bool, current_reduced: bool) -> MotionTransition {
    match (previous_reduced, current_reduced) {
        (false, true) => MotionTransition::DisableAfter(SETTINGS_SWITCH_TRANSITION),
        (true, false) => MotionTransition::EnableImmediately,
        _ => MotionTransition::Unchanged,
    }
}

/// A delayed reduction is valid only if the persisted preference still matches
/// the preference that scheduled it.
pub(crate) const fn motion_preference_still_matches(
    expected_reduced: bool,
    saved_reduced: bool,
) -> bool {
    expected_reduced == saved_reduced
}

pub(crate) const fn should_apply_deferred_motion_reduction(
    expected_generation: u64,
    current_generation: u64,
    saved_reduced: bool,
) -> bool {
    expected_generation == current_generation
        && motion_preference_still_matches(true, saved_reduced)
}

pub(crate) fn settings_switch<F>(id: impl Into<ElementId>, checked: bool, on_click: F) -> Switch
where
    F: Fn(&bool, &mut Window, &mut App) + 'static,
{
    Switch::new(id)
        .checked(checked)
        .cursor(CursorStyle::PointingHand)
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::{
        MotionTransition, SETTINGS_SWITCH_TRANSITION, motion_preference_still_matches,
        motion_transition, should_apply_deferred_motion_reduction,
    };
    use std::time::Duration;

    #[test]
    fn settings_switch_transition_matches_gpui_component_thumb_motion() {
        assert_eq!(SETTINGS_SWITCH_TRANSITION, Duration::from_millis(150));
    }

    #[test]
    fn motion_transitions_defer_only_when_reducing_motion() {
        assert_eq!(
            motion_transition(false, true),
            MotionTransition::DisableAfter(SETTINGS_SWITCH_TRANSITION)
        );
        assert_eq!(
            motion_transition(true, false),
            MotionTransition::EnableImmediately
        );
        assert_eq!(motion_transition(false, false), MotionTransition::Unchanged);
        assert_eq!(motion_transition(true, true), MotionTransition::Unchanged);
    }

    #[test]
    fn deferred_motion_reduction_rejects_a_reversed_saved_preference() {
        assert!(motion_preference_still_matches(true, true));
        assert!(!motion_preference_still_matches(true, false));
        assert!(!should_apply_deferred_motion_reduction(1, 3, true));
        assert!(should_apply_deferred_motion_reduction(3, 3, true));
    }
}
