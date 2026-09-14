use super::{
    RectSelectorMotion, RectSelectorTransition, RightSidebarTransition,
    RightSidebarTransitionAction, SidebarBottomMotion, SidebarWidthMotion,
    playback_sidebar_exits_settings, right_sidebar_layout_width, update_account_scope,
    update_download_scope,
};
use crate::{motion, playback::RightSidebar};
use std::time::{Duration, Instant};

#[test]
fn account_observer_only_transitions_when_scope_changes() {
    let mut last_scope = "deezer\n42\nsoundcloud".to_owned();

    assert!(!update_account_scope(
        &mut last_scope,
        "deezer\n42\nsoundcloud"
    ));
    assert!(update_account_scope(
        &mut last_scope,
        "other-deezer\n7\nother-soundcloud"
    ));
    assert_eq!(last_scope, "other-deezer\n7\nother-soundcloud");
}

#[test]
fn download_observer_tracks_credential_generation_separately() {
    let mut last_scope = 7;
    assert!(!update_download_scope(&mut last_scope, 7));
    assert!(update_download_scope(&mut last_scope, 8));
    assert_eq!(last_scope, 8);
}

#[test]
fn sidebar_transition_retains_panel_until_close_duration() {
    let started_at = Instant::now();
    let mut transition = RightSidebarTransition::new(RightSidebar::Queue);

    assert_eq!(
        transition.request(RightSidebar::Closed, false, started_at),
        RightSidebarTransitionAction::ScheduleClose {
            epoch: 1,
            started_at,
        }
    );
    assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);
    assert!(transition.closing);
    assert!(!transition.finish_close(1, started_at));
    assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);

    assert!(transition.finish_close(1, started_at + motion::PANEL_DURATION));
    assert_eq!(transition.displayed_sidebar, RightSidebar::Closed);
    assert!(!transition.closing);
}

#[test]
fn sidebar_transition_reopen_invalidates_stale_close_completion() {
    let started_at = Instant::now();
    let mut transition = RightSidebarTransition::new(RightSidebar::Lyrics);
    let close_epoch = match transition.request(RightSidebar::Closed, false, started_at) {
        RightSidebarTransitionAction::ScheduleClose { epoch, .. } => epoch,
        action => panic!("unexpected transition action: {action:?}"),
    };

    assert_eq!(
        transition.request(RightSidebar::Lyrics, false, started_at),
        RightSidebarTransitionAction::CancelClose
    );
    assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
    assert_eq!(transition.desired_sidebar, RightSidebar::Lyrics);
    assert!(!transition.closing);
    assert!(!transition.finish_close(close_epoch, started_at + motion::PANEL_DURATION));
}

#[test]
fn sidebar_transition_between_open_panels_marks_content_switch() {
    let now = Instant::now();
    let mut transition = RightSidebarTransition::new(RightSidebar::Queue);
    assert!(!transition.content_switch);

    assert_eq!(
        transition.request(RightSidebar::Lyrics, false, now),
        RightSidebarTransitionAction::CancelClose
    );
    assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
    assert!(transition.content_switch);

    assert_eq!(
        transition.request(RightSidebar::Queue, false, now),
        RightSidebarTransitionAction::CancelClose
    );
    assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);
    assert!(transition.content_switch);

    assert_eq!(
        transition.request(RightSidebar::Closed, false, now),
        RightSidebarTransitionAction::ScheduleClose {
            epoch: transition.epoch,
            started_at: now,
        }
    );
    assert!(!transition.content_switch);
}

#[test]
fn sidebar_transition_skips_enter_when_settings_remounts_open_panel() {
    let now = Instant::now();
    let mut transition = RightSidebarTransition::new(RightSidebar::Queue);
    transition.skip_enter_if_already_open();
    assert!(transition.skip_enter);
    assert_eq!(
        transition.request(RightSidebar::Queue, false, now),
        RightSidebarTransitionAction::None
    );
    assert!(transition.skip_enter);

    assert_eq!(
        transition.request(RightSidebar::Closed, false, now),
        RightSidebarTransitionAction::ScheduleClose {
            epoch: 1,
            started_at: now,
        }
    );
    assert!(!transition.skip_enter);
    assert!(transition.closing);
}

#[test]
fn sidebar_transition_does_not_skip_enter_when_closed_or_closing() {
    let now = Instant::now();
    let mut closed = RightSidebarTransition::new(RightSidebar::Closed);
    closed.skip_enter_if_already_open();
    assert!(!closed.skip_enter);

    let mut closing = RightSidebarTransition::new(RightSidebar::Lyrics);
    assert_eq!(
        closing.request(RightSidebar::Closed, false, now),
        RightSidebarTransitionAction::ScheduleClose {
            epoch: 1,
            started_at: now,
        }
    );
    closing.skip_enter_if_already_open();
    assert!(!closing.skip_enter);
}

#[test]
fn sidebar_transition_real_open_keeps_enter_animation() {
    let now = Instant::now();
    let mut transition = RightSidebarTransition::new(RightSidebar::Closed);
    transition.skip_enter_if_already_open();
    assert_eq!(
        transition.request(RightSidebar::Lyrics, false, now),
        RightSidebarTransitionAction::CancelClose
    );
    assert!(!transition.skip_enter);
    assert!(!transition.content_switch);
    assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
}

#[test]
fn playback_sidebar_transition_matrix_exits_settings_only_on_panel_change() {
    let sidebars = [
        RightSidebar::Closed,
        RightSidebar::Queue,
        RightSidebar::Lyrics,
    ];
    for previous in sidebars {
        for current in sidebars {
            let expected = previous != current
                && matches!(current, RightSidebar::Queue | RightSidebar::Lyrics);
            assert_eq!(
                playback_sidebar_exits_settings(previous, current),
                expected,
                "unexpected transition: {previous:?} -> {current:?}"
            );
        }
    }
}

#[test]
fn sidebar_width_motion_initializes_at_first_rendered_width() {
    let started_at = Instant::now();
    let mut motion = SidebarWidthMotion::default();

    motion.retarget(240.0, started_at, false);

    assert!(motion.initialized);
    assert_eq!(motion.from, 240.0);
    assert_eq!(motion.target, 240.0);
    assert_eq!(motion.epoch, 0);
    assert_eq!(motion.displayed_width(started_at), 240.0);
}

#[test]
fn sidebar_width_motion_retargets_from_current_displayed_width() {
    let started_at = Instant::now();
    let mut motion = SidebarWidthMotion::default();
    motion.retarget(240.0, started_at, false);
    motion.retarget(68.0, started_at, false);

    let retarget_at = started_at + Duration::from_millis(40);
    let displayed = motion.displayed_width(retarget_at);
    motion.retarget(240.0, retarget_at, false);

    assert_eq!(motion.from, displayed);
    assert_eq!(motion.target, 240.0);
    assert_eq!(motion.epoch, 2);
    assert_eq!(motion.displayed_width(retarget_at), displayed);
    assert_eq!(
        motion.displayed_width(retarget_at + motion::PANEL_DURATION),
        240.0
    );
}

#[test]
fn sidebar_width_motion_reduced_motion_snaps_to_target() {
    let started_at = Instant::now();
    let mut motion = SidebarWidthMotion::default();
    motion.retarget(240.0, started_at, false);
    motion.retarget(68.0, started_at, true);

    assert_eq!(motion.from, 68.0);
    assert_eq!(motion.target, 68.0);
    assert_eq!(motion.displayed_width(started_at), 68.0);
}

#[test]
fn sidebar_bottom_motion_initializes_both_fractions_at_the_first_endpoint() {
    let started_at = Instant::now();
    let mut motion = SidebarBottomMotion::default();

    let visual = motion.prepare(false, true, started_at, false);

    assert!(motion.initialized);
    assert_eq!(motion.compact_from, 0.0);
    assert_eq!(motion.compact_target, 0.0);
    assert_eq!(motion.settings_from, 1.0);
    assert_eq!(motion.settings_target, 1.0);
    assert_eq!(visual.fractions_at(0.0), (0.0, 1.0));
    assert_eq!(visual.epoch, 0);
}

#[test]
fn sidebar_bottom_motion_retargets_from_current_fractions_without_jumping() {
    let started_at = Instant::now();
    let mut motion = SidebarBottomMotion::default();
    motion.prepare(false, false, started_at, false);
    motion.prepare(true, true, started_at, false);

    let reversal_at = started_at + Duration::from_millis(40);
    let displayed = motion.displayed_fractions(reversal_at);
    let visual = motion.prepare(false, false, reversal_at, false);

    assert_eq!(motion.compact_from, displayed.0);
    assert_eq!(motion.settings_from, displayed.1);
    assert_eq!(visual.fractions_at(0.0), displayed);
    assert_eq!(visual.compact_target, 0.0);
    assert_eq!(visual.settings_target, 0.0);
    assert_eq!(visual.epoch, 2);
    assert_eq!(motion.displayed_fractions(reversal_at), displayed);
}

#[test]
fn sidebar_bottom_motion_reduced_motion_snaps_both_fractions() {
    let started_at = Instant::now();
    let mut motion = SidebarBottomMotion::default();
    motion.prepare(false, false, started_at, false);

    let visual = motion.prepare(true, true, started_at, true);

    assert_eq!(motion.compact_from, 1.0);
    assert_eq!(motion.compact_target, 1.0);
    assert_eq!(motion.settings_from, 1.0);
    assert_eq!(motion.settings_target, 1.0);
    assert_eq!(visual.fractions_at(0.0), (1.0, 1.0));
}

#[test]
fn right_sidebar_layout_width_has_open_and_close_endpoints() {
    assert_eq!(right_sidebar_layout_width(360.0, false, 0.0), 0.0);
    assert_eq!(right_sidebar_layout_width(360.0, false, 1.0), 360.0);
    assert_eq!(right_sidebar_layout_width(360.0, true, 0.0), 360.0);
    assert_eq!(right_sidebar_layout_width(360.0, true, 1.0), 0.0);
}

#[test]
fn selector_motion_settles_first_render_and_animates_retargets() {
    let start = Instant::now();
    let mut motion = RectSelectorMotion::default();

    motion.retarget(3.0, 70.0, start, false, false);
    assert_eq!(motion.visible_geometry(start), (3.0, 70.0));
    assert_eq!(motion.epoch, 0);

    motion.retarget(75.0, 90.0, start, false, false);
    assert_eq!(motion.epoch, 1);
    assert_eq!(motion.transition, RectSelectorTransition::Interaction);
    assert_eq!(motion.transition.duration(), motion::INTERACTION_DURATION);
    assert_eq!(motion.visible_geometry(start), (3.0, 70.0));
    assert_ne!(
        motion.visible_geometry(start + Duration::from_millis(50)),
        (3.0, 70.0)
    );
    assert_eq!(
        motion.visible_geometry(start + motion::INTERACTION_DURATION),
        (75.0, 90.0)
    );
}

#[test]
fn selector_motion_uses_content_transition_for_responsive_retargets() {
    let start = Instant::now();
    let mut motion = RectSelectorMotion::default();
    motion.retarget(3.0, 70.0, start, false, true);
    motion.retarget(167.0, 122.0, start, false, true);

    assert_eq!(motion.transition, RectSelectorTransition::Responsive);
    assert_eq!(motion.transition.duration(), motion::CONTENT_DURATION);
    let expected = gpui::ease_in_out(0.5);
    let (x, width) = motion.visible_geometry(start + motion::CONTENT_DURATION / 2);
    assert!((x - motion::lerp(3.0, 167.0, expected)).abs() < 0.0001);
    assert!((width - motion::lerp(70.0, 122.0, expected)).abs() < 0.0001);
}

#[test]
fn selector_motion_retargets_from_current_eased_geometry() {
    let start = Instant::now();
    let mut motion = RectSelectorMotion::default();
    motion.retarget(3.0, 70.0, start, false, true);
    motion.retarget(167.0, 122.0, start, false, true);

    let retarget_at = start + Duration::from_millis(40);
    let visible = motion.visible_geometry(retarget_at);
    motion.retarget(3.0, 70.0, retarget_at, false, false);

    assert_eq!(motion.from_x, visible.0);
    assert_eq!(motion.from_width, visible.1);
    assert_eq!(motion.target_x, 3.0);
    assert_eq!(motion.target_width, 70.0);
    assert_eq!(motion.epoch, 2);
    assert_eq!(motion.transition, RectSelectorTransition::Interaction);
    assert_eq!(motion.visible_geometry(retarget_at), visible);
}

#[test]
fn selector_motion_force_rebases_live_geometry_with_an_unchanged_target() {
    let start = Instant::now();
    let mut motion = RectSelectorMotion::default();
    motion.retarget(3.0, 70.0, start, false, false);
    motion.retarget(167.0, 122.0, start, false, false);

    let rebase_at = start + motion::INTERACTION_DURATION / 2;
    let visible = motion.visible_geometry(rebase_at);
    let epoch = motion.epoch;
    motion.retarget(167.0, 122.0, rebase_at, false, true);

    assert_eq!(motion.from_x, visible.0);
    assert_eq!(motion.from_width, visible.1);
    assert_eq!(motion.target_x, 167.0);
    assert_eq!(motion.target_width, 122.0);
    assert_eq!(motion.epoch, epoch + 1);
    assert_eq!(motion.transition, RectSelectorTransition::Responsive);
    assert_eq!(motion.visible_geometry(rebase_at), visible);
}

#[test]
fn selector_motion_reduced_motion_snaps_to_target() {
    let start = Instant::now();
    let mut motion = RectSelectorMotion::default();
    motion.retarget(3.0, 70.0, start, true, true);
    motion.retarget(75.0, 34.0, start, true, true);

    assert_eq!(motion.from_x, 75.0);
    assert_eq!(motion.from_width, 34.0);
    assert_eq!(motion.visible_geometry(start), (75.0, 34.0));
}
