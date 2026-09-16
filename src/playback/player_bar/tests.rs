use super::{
    CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX, CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX,
    CLOSE_PLAYER_RIGHT_PX, FAVORITE_PINK, FadeMotion, PLAYER_ACTION_BUTTON_RADIUS_PX,
    PLAYER_ACTION_DISABLED_OPACITY, PLAYER_BAR_DESKTOP_HEIGHT_PX, PLAYER_CLOSE_GLYPH_PX,
    PlayerBarLayout, PlayerBarLayoutMotion, PlayerBarMotion, QUALITY_BADGE_HEIGHT_PX,
    QUALITY_BADGE_MIN_WIDTH_PX, QUALITY_BADGE_TEXT_OFFSET_PX, REPEAT_CONTROL_SIZE_PX,
    REPEAT_ONE_BADGE_BOTTOM_PX, REPEAT_ONE_BADGE_RIGHT_PX, RectMotion, RightControlGeometry,
    ScalarMotion, SeekFillMotion, SeekPointerState, VOLUME_DRAG_THRESHOLD_PX, VOLUME_GAP_PX,
    VOLUME_ICON_EM_HEIGHT_PX, VOLUME_ICON_FRAME_PX, VOLUME_MUTE_BUTTON_PX,
    VOLUME_SLIDER_CONTROL_HEIGHT_PX, VOLUME_SLIDER_WIDTH_PX, VOLUME_THUMB_DIAMETER_PX,
    VOLUME_TRACK_HEIGHT_PX, VolumeIconLevel, VolumeMotion, VolumeMotionMode, VolumePointerPhase,
    VolumePointerRelease, VolumePointerState, WIDE_VOLUME_INSET_PX, accessibility_seek_fraction,
    artist_routes_for_track, artwork_resource, bare_action_visual, close_player_tooltip_gap,
    current_block_width, current_favorite_key, current_subtitle, current_text_available_width,
    current_text_width_for_layout, current_track_title, desktop_player_geometry,
    download_available, fade_motion_geometry, fade_motion_opacity, favorite_feedback_opacity,
    favorite_left_for_text_width, favorite_top, finish_volume_pointer_interaction,
    pointer_seek_fraction, quality_badge_animation_key, quality_badge_opacity_endpoints,
    quality_text_animation_key, quality_text_opacity_endpoints, rendered_artist_text,
    rendered_current_artist_text, resolve_artwork, search_provider_for_playback,
    seekbar_display_progress, update_last_quality_label, update_quality_label_for_generation,
    volume_container_offsets, volume_icon_dimensions, volume_icon_speaker_offset,
    volume_logical_bounds, volume_motion_mode_for_render, wide_volume_inset,
};
use crate::{
    entity_navigation::{MenuRoute, MenuRouteKind},
    library::{FavoriteKey, FavoriteKind},
    playback::{PlaybackProvider, PlaybackStatus, PlaybackTrack},
    search::{Provider, TrackArtistRef},
    theme::{FOREGROUND, MUTED},
};
use gpui::{Bounds, ImageCacheError, RenderImage, Resource, point, px, size};
use image::{Frame, Rgba, RgbaImage};
use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn selected_player_favorite_uses_pink_without_panel_background() {
    assert_eq!(
        bare_action_visual(true, Some(FAVORITE_PINK)),
        (FAVORITE_PINK, FAVORITE_PINK, false)
    );
    assert_eq!(
        bare_action_visual(false, Some(FAVORITE_PINK)),
        (MUTED, FOREGROUND, false)
    );
    assert_eq!(
        bare_action_visual(true, None),
        (FOREGROUND, FOREGROUND, true)
    );
}

#[test]
fn pending_player_favorite_dims_once_like_tracklist_favorites() {
    let (from, to) = favorite_feedback_opacity(true);
    assert_eq!((from, to), (1., 1.));
    assert_eq!(to * PLAYER_ACTION_DISABLED_OPACITY, 0.4);
}

#[test]
fn current_track_actions_match_provider_availability() {
    let track = |provider| PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider,
        id: "42".into(),
        title: String::new(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::ZERO,
        explicit: false,
        service_url: String::new(),
    };
    let deezer = track(PlaybackProvider::Deezer);
    let soundcloud = track(PlaybackProvider::SoundCloud);

    assert!(!download_available(None));
    assert!(download_available(Some(&deezer)));
    assert_eq!(
        current_favorite_key(Some(&deezer)).unwrap(),
        FavoriteKey::deezer(FavoriteKind::Track, "42".into())
    );
    assert_eq!(
        current_favorite_key(Some(&soundcloud)).unwrap(),
        FavoriteKey::soundcloud(FavoriteKind::Track, "42".into())
    );
    assert!(current_favorite_key(None).is_none());
}

#[test]
fn ready_artwork_paints_and_becomes_the_remembered_cover() {
    let cover = test_cover();
    let mut remembered = None;
    match resolve_artwork(Some(Ok(cover.clone())), &mut remembered) {
        Some(Ok(painted)) => assert!(Arc::ptr_eq(&painted, &cover)),
        _ => panic!("a ready artwork must paint"),
    }
    assert!(
        remembered
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &cover))
    );
}

#[test]
fn loading_artwork_keeps_the_previous_cover_on_screen() {
    let previous = test_cover();
    let mut remembered = Some(previous.clone());
    match resolve_artwork(None, &mut remembered) {
        Some(Ok(painted)) => assert!(Arc::ptr_eq(&painted, &previous)),
        _ => panic!("a loading artwork must keep the previous cover"),
    }
    assert!(
        remembered
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &previous))
    );
}

#[test]
fn loading_artwork_without_history_shows_the_placeholder() {
    let mut remembered = None;
    assert!(resolve_artwork(None, &mut remembered).is_none());
    assert!(remembered.is_none());
}

#[test]
fn failed_artwork_shows_the_placeholder_and_keeps_the_previous_cover() {
    let previous = test_cover();
    let mut remembered = Some(previous.clone());
    match resolve_artwork(
        Some(Err(ImageCacheError::Asset("cover unavailable".into()))),
        &mut remembered,
    ) {
        Some(Err(_)) => {}
        _ => panic!("a failed artwork must pass its error through"),
    }
    assert!(
        remembered
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &previous))
    );
}

#[test]
fn newer_ready_artwork_replaces_the_remembered_cover() {
    let older = test_cover();
    let newer = test_cover();
    let mut remembered = Some(older);
    match resolve_artwork(Some(Ok(newer.clone())), &mut remembered) {
        Some(Ok(painted)) => assert!(Arc::ptr_eq(&painted, &newer)),
        _ => panic!("a ready artwork must paint"),
    }
    assert!(
        remembered
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(image, &newer))
    );
}

#[test]
fn rapid_switches_keep_the_last_ready_cover_until_one_loads() {
    let first = test_cover();
    let mut remembered = None;
    resolve_artwork(Some(Ok(first.clone())), &mut remembered);
    // Two skips in a row before any new artwork finishes loading.
    assert!(matches!(
        resolve_artwork(None, &mut remembered),
        Some(Ok(painted)) if Arc::ptr_eq(&painted, &first)
    ));
    assert!(matches!(
        resolve_artwork(None, &mut remembered),
        Some(Ok(painted)) if Arc::ptr_eq(&painted, &first)
    ));
}

#[test]
fn artwork_urls_classify_like_plain_img_sources() {
    assert!(matches!(
        artwork_resource("https://cdn.example.test/cover.jpg".to_owned()),
        Resource::Uri(_)
    ));
    assert!(matches!(
        artwork_resource("icons/cover".to_owned()),
        Resource::Embedded(_)
    ));
}

fn test_cover() -> Arc<RenderImage> {
    Arc::new(RenderImage::new([Frame::new(RgbaImage::from_pixel(
        2,
        2,
        Rgba([24, 48, 96, 255]),
    ))]))
}

#[test]
fn seekbar_preview_wins_over_playback_progress_until_release() {
    assert_eq!(seekbar_display_progress(0.2, Some(0.8)), 0.8);
    assert_eq!(seekbar_display_progress(0.2, None), 0.2);
    assert_eq!(seekbar_display_progress(1.4, None), 1.0);
    assert_eq!(seekbar_display_progress(-0.4, Some(1.4)), 1.0);
}

#[test]
fn seek_fill_retarget_starts_at_the_eased_displayed_midpoint() {
    let started_at = Instant::now();
    let halfway = started_at + crate::motion::INTERACTION_DURATION / 2;
    let mut motion = SeekFillMotion::default();

    let first = motion.prepare(1, 0., 1., started_at, false);
    assert!(first.active);

    let expected = gpui::ease_in_out(0.5);
    let second = motion.prepare(2, 1., 0., halfway, false);

    assert!(second.active);
    assert!((second.from - expected).abs() < 0.0001);
    assert!((second.from - 1.).abs() > 0.0001);
}

#[test]
fn volume_motion_initializes_at_the_model_value() {
    let now = Instant::now();
    let mut motion = VolumeMotion::default();

    let visual = motion.prepare(0.4, now, false, VolumeMotionMode::Animated);

    assert_eq!(visual.from, 0.4);
    assert_eq!(visual.target, 0.4);
    assert!(!visual.active);
}

#[test]
fn volume_motion_keeps_dragging_direct_and_handles_a_full_mute_jump() {
    let now = Instant::now();
    let mut motion = VolumeMotion::default();
    motion.prepare(1.0, now, false, VolumeMotionMode::Animated);

    let dragged = motion.prepare(
        0.35,
        now + Duration::from_millis(20),
        false,
        VolumeMotionMode::Direct,
    );
    assert_eq!(dragged.from, 0.35);
    assert_eq!(dragged.target, 0.35);
    assert!(!dragged.active);

    let clicked = motion.prepare(
        0.0,
        now + Duration::from_millis(40),
        false,
        VolumeMotionMode::Animated,
    );
    assert_eq!(clicked.from, 0.35);
    assert_eq!(clicked.target, 0.0);
    assert!(clicked.active);
}

#[test]
fn pending_volume_click_holds_the_visible_value_until_release() {
    let started_at = Instant::now();
    let mut motion = VolumeMotion::default();
    motion.prepare(0.8, started_at, false, VolumeMotionMode::Animated);
    let held_value = motion.displayed_at(started_at);
    let mut pointer = VolumePointerState::pending(12., 18., held_value);

    let held = motion.prepare(
        0.15,
        started_at + Duration::from_millis(10),
        false,
        pointer.motion_mode(),
    );
    assert_eq!(held.from, held_value);
    assert_eq!(held.target, held_value);
    assert!(!held.active);

    assert_eq!(pointer.finish(), Some(VolumePointerRelease::PendingClick));
    let released = motion.prepare(
        0.15,
        started_at + Duration::from_millis(10),
        false,
        pointer.motion_mode(),
    );
    assert_eq!(released.from, held_value);
    assert_eq!(released.target, 0.15);
    assert!(released.active);
}

#[test]
fn volume_pointer_crosses_threshold_once_and_enters_dragging() {
    let mut pointer = VolumePointerState::pending(20., 20., 0.5);

    assert!(!pointer.update_move(22., 21.));
    assert!(matches!(
        pointer.phase,
        VolumePointerPhase::PendingClick { .. }
    ));
    assert!(pointer.update_move(24., 20.));
    assert_eq!(pointer.phase, VolumePointerPhase::Dragging);
    assert!(!pointer.update_move(60., 20.));
    assert_eq!(pointer.phase, VolumePointerPhase::Dragging);
}

#[test]
fn every_volume_drag_update_is_direct_and_release_is_direct_too() {
    let now = Instant::now();
    let mut motion = VolumeMotion::default();
    motion.prepare(0.9, now, false, VolumeMotionMode::Animated);
    let mut pointer = VolumePointerState::pending(0., 0., 0.9);
    assert!(pointer.update_move(VOLUME_DRAG_THRESHOLD_PX, 0.));

    for (offset, target) in [(2_u64, 0.65), (4, 0.35), (6, 0.05)] {
        let visual = motion.prepare(
            target,
            now + Duration::from_millis(offset),
            false,
            pointer.motion_mode(),
        );
        assert_eq!(visual.from, target);
        assert_eq!(visual.target, target);
        assert!(!visual.active);
    }

    assert_eq!(pointer.finish(), Some(VolumePointerRelease::Dragging));
    assert_eq!(pointer.phase, VolumePointerPhase::DirectRelease);
    let settled = motion.prepare(
        0.2,
        now + Duration::from_millis(8),
        false,
        pointer.motion_mode(),
    );
    assert_eq!(settled.from, 0.2);
    assert_eq!(settled.target, 0.2);
    assert!(!settled.active);
}

#[test]
fn volume_pointer_release_does_not_depend_on_pointer_being_inside_bounds() {
    let mut pointer = VolumePointerState::pending(10., 10., 0.4);
    assert!(pointer.update_move(10. + VOLUME_DRAG_THRESHOLD_PX, 10.));
    assert_eq!(pointer.finish(), Some(VolumePointerRelease::Dragging));
    assert_eq!(pointer.phase, VolumePointerPhase::DirectRelease);
}

#[test]
fn volume_pointer_release_persists_in_cell_until_render_consumes_it() {
    let stored = Cell::new(VolumePointerState::pending(4., 6., 0.7));
    assert_eq!(
        finish_volume_pointer_interaction(&stored),
        Some(VolumePointerRelease::PendingClick)
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::Idle);

    let mut dragging = VolumePointerState::pending(4., 6., 0.7);
    assert!(dragging.update_move(4. + VOLUME_DRAG_THRESHOLD_PX, 6.));
    stored.set(dragging);
    assert_eq!(
        finish_volume_pointer_interaction(&stored),
        Some(VolumePointerRelease::Dragging)
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::DirectRelease);
    assert_eq!(finish_volume_pointer_interaction(&stored), None);

    assert_eq!(
        volume_motion_mode_for_render(&stored),
        VolumeMotionMode::Direct
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
}

#[test]
fn pending_volume_lost_release_returns_to_animated_idle() {
    let stored = Cell::new(VolumePointerState::pending(8., 10., 0.6));

    assert_eq!(
        finish_volume_pointer_interaction(&stored),
        Some(VolumePointerRelease::PendingClick)
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
    assert_eq!(
        volume_motion_mode_for_render(&stored),
        VolumeMotionMode::Animated
    );
}

#[test]
fn dragging_volume_lost_release_gets_one_final_direct_settle() {
    let mut dragging = VolumePointerState::pending(8., 10., 0.6);
    assert!(dragging.update_move(8. + VOLUME_DRAG_THRESHOLD_PX, 10.));
    let stored = Cell::new(dragging);

    assert_eq!(
        finish_volume_pointer_interaction(&stored),
        Some(VolumePointerRelease::Dragging)
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::DirectRelease);
    assert_eq!(
        volume_motion_mode_for_render(&stored),
        VolumeMotionMode::Direct
    );
    assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
}

#[test]
fn volume_motion_animates_large_jumps_between_discrete_positions() {
    let started_at = Instant::now();
    let mut motion = VolumeMotion::default();
    motion.prepare(0.1, started_at, false, VolumeMotionMode::Animated);

    let visual = motion.prepare(0.9, started_at, false, VolumeMotionMode::Animated);

    assert_eq!(visual.from, 0.1);
    assert_eq!(visual.target, 0.9);
    assert!(visual.active);
    let sampled = motion.displayed_at(started_at + crate::motion::INTERACTION_DURATION / 2);
    let expected = crate::motion::lerp(0.1, 0.9, gpui::ease_in_out(0.5));
    assert!((sampled - expected).abs() < 0.0001);
}

#[test]
fn reduced_motion_snaps_volume_to_each_target() {
    let now = Instant::now();
    let mut motion = VolumeMotion::default();

    let first = motion.prepare(1.0, now, true, VolumeMotionMode::Animated);
    let second = motion.prepare(
        0.0,
        now + Duration::from_millis(20),
        true,
        VolumeMotionMode::Animated,
    );

    assert_eq!(first.from, 1.0);
    assert_eq!(second.from, 0.0);
    assert_eq!(second.target, 0.0);
    assert!(!second.active);
}

#[test]
fn volume_motion_retargets_rapid_changes_from_the_displayed_value() {
    let started_at = Instant::now();
    let first_retarget_at = started_at + Duration::from_millis(30);
    let second_retarget_at = first_retarget_at + Duration::from_millis(25);
    let mut motion = VolumeMotion::default();

    motion.prepare(0.0, started_at, false, VolumeMotionMode::Animated);
    motion.prepare(1.0, first_retarget_at, false, VolumeMotionMode::Animated);
    let first_displayed = motion.displayed_at(first_retarget_at);
    let second = motion.prepare(0.2, first_retarget_at, false, VolumeMotionMode::Animated);
    assert!((second.from - first_displayed).abs() < 0.0001);

    let second_displayed = motion.displayed_at(second_retarget_at);
    let third = motion.prepare(0.8, second_retarget_at, false, VolumeMotionMode::Animated);
    assert!((third.from - second_displayed).abs() < 0.0001);
}

#[test]
fn player_artist_routes_use_the_playback_provider_mapping() {
    assert_eq!(
        search_provider_for_playback(PlaybackProvider::Deezer),
        Provider::Deezer
    );
    assert_eq!(
        search_provider_for_playback(PlaybackProvider::SoundCloud),
        Provider::SoundCloud
    );

    let artists = vec![
        TrackArtistRef {
            id: "11".into(),
            name: "Primary".into(),
        },
        TrackArtistRef {
            id: "legacy/12".into(),
            name: "Legacy".into(),
        },
        TrackArtistRef {
            id: "13".into(),
            name: "Collaborator".into(),
        },
    ];
    let routes = artist_routes_for_track(
        search_provider_for_playback(PlaybackProvider::Deezer),
        &artists,
    );

    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].id, "11");
    assert_eq!(routes[1].id, "13");
}

#[test]
fn rapid_seek_commits_continue_from_the_currently_displayed_value() {
    let started_at = Instant::now();
    let first_retarget_at = started_at + Duration::from_millis(40);
    let second_retarget_at = first_retarget_at + Duration::from_millis(30);
    let mut motion = SeekFillMotion::default();

    motion.prepare(1, 0., 1., started_at, false);
    let first_displayed = motion.displayed_at(first_retarget_at);
    let second = motion.prepare(2, 1., 0.1, first_retarget_at, false);

    assert!((second.from - first_displayed).abs() < 0.0001);
    let second_displayed = motion.displayed_at(second_retarget_at);
    let third = motion.prepare(3, 0.1, 0.8, second_retarget_at, false);

    assert!((third.from - second_displayed).abs() < 0.0001);
    assert!((third.from - 0.1).abs() > 0.0001);
}

#[test]
fn direct_seek_preview_cancels_old_motion_before_release() {
    let started_at = Instant::now();
    let preview = 0.42;
    let release_at = started_at + Duration::from_millis(35);
    let mut motion = SeekFillMotion::default();

    motion.prepare(1, 0.2, 0.8, started_at, false);
    motion.set_displayed(preview);

    assert!(motion.started_at.is_none());
    assert_eq!(motion.displayed_at(release_at), preview);

    let release = motion.prepare(2, preview, 0.9, release_at, false);

    assert_eq!(release.from, preview);
    assert_eq!(release.target, 0.9);
    assert!(release.active);
}

#[test]
fn reduced_motion_snaps_seek_fill_to_each_commit_target() {
    let now = Instant::now();
    let mut motion = SeekFillMotion::default();

    let first = motion.prepare(1, 0., 1., now, true);
    assert_eq!(first.from, 1.);
    assert_eq!(first.target, 1.);
    assert!(!first.active);

    let second = motion.prepare(2, 1., 0., now + Duration::from_millis(50), true);
    assert_eq!(second.from, 0.);
    assert_eq!(second.target, 0.);
    assert!(!second.active);
}

#[test]
fn accessibility_seek_uses_slider_step_and_clamps() {
    assert!(
        (accessibility_seek_fraction(0.2, super::SEEK_SLIDER_STEP, true).unwrap() - 0.21).abs()
            < f32::EPSILON
    );
    assert_eq!(
        accessibility_seek_fraction(0.995, super::SEEK_SLIDER_STEP, true),
        Some(1.0)
    );
    assert_eq!(
        accessibility_seek_fraction(0.005, -super::SEEK_SLIDER_STEP, true),
        Some(0.0)
    );
}

#[test]
fn disabled_accessibility_seek_is_ignored() {
    assert_eq!(
        accessibility_seek_fraction(0.5, super::SEEK_SLIDER_STEP, false),
        None
    );
}

#[test]
fn pointer_seek_fraction_uses_bounds_origin() {
    let bounds = Bounds {
        origin: point(px(10.), px(0.)),
        size: size(px(100.), px(24.)),
    };

    assert!((pointer_seek_fraction(60.25, bounds) - 0.5025).abs() < 0.000001);
}

#[test]
fn pointer_seek_fraction_clamps_outside_bounds() {
    let bounds = Bounds {
        origin: point(px(10.), px(0.)),
        size: size(px(100.), px(24.)),
    };

    assert_eq!(pointer_seek_fraction(9., bounds), 0.);
    assert_eq!(pointer_seek_fraction(111., bounds), 1.);
}

#[test]
fn pointer_seek_fraction_returns_zero_for_zero_width() {
    let bounds = Bounds {
        origin: point(px(10.), px(0.)),
        size: size(px(0.), px(24.)),
    };

    assert_eq!(pointer_seek_fraction(60.25, bounds), 0.);
}

#[test]
fn expanded_volume_hitbox_keeps_fraction_mapping_on_the_logical_track() {
    let expanded = Bounds {
        origin: point(px(93.5), px(0.)),
        size: size(px(93.), px(24.)),
    };
    let logical = volume_logical_bounds(expanded);

    assert_eq!(logical.origin.x, px(100.));
    assert_eq!(logical.size.width, px(VOLUME_SLIDER_WIDTH_PX));
    assert_eq!(pointer_seek_fraction(93.5, logical), 0.);
    assert_eq!(pointer_seek_fraction(100., logical), 0.);
    assert_eq!(pointer_seek_fraction(180., logical), 1.);
    assert_eq!(pointer_seek_fraction(186.5, logical), 1.);
}

#[test]
fn seek_pointer_state_survives_a_simulated_rerender() {
    let state = Rc::new(Cell::new(SeekPointerState::default()));
    let first_render_state = state.clone();
    first_render_state.set(SeekPointerState {
        active: true,
        moved: true,
    });

    let rerender_state = state.clone();

    assert_eq!(
        rerender_state.get(),
        SeekPointerState {
            active: true,
            moved: true,
        }
    );
}

#[test]
fn opening_player_bar_animates_height_without_fading_content() {
    let now = Instant::now();
    let mut motion = PlayerBarMotion::default();

    let visual = motion.prepare(true, false, now, false);

    assert_eq!(visual.from_height, 0.);
    assert_eq!(visual.target_height, PLAYER_BAR_DESKTOP_HEIGHT_PX);
    assert_eq!(visual.from_opacity, 1.);
    assert_eq!(visual.target_opacity, 1.);
    assert!(visual.active);
}

#[test]
fn closing_player_bar_still_fades_and_collapses() {
    let now = Instant::now();
    let mut motion = PlayerBarMotion::default();
    motion.prepare(true, false, now, true);

    let visual = motion.prepare(false, false, now, false);

    assert_eq!(visual.from_height, PLAYER_BAR_DESKTOP_HEIGHT_PX);
    assert_eq!(visual.target_height, 0.);
    assert_eq!(visual.from_opacity, 1.);
    assert_eq!(visual.target_opacity, 0.);
    assert!(visual.active);
}

#[test]
fn compact_wide_layout_transition_interpolates_and_settles() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let now = Instant::now();
    let mut motion = PlayerBarLayoutMotion::default();

    let initial = motion.prepare(true, compact, now, false);
    assert_eq!(initial.from, compact);
    assert_eq!(initial.target, compact);
    assert!(!initial.active);

    let transition = motion.prepare(false, wide, now, false);
    assert_eq!(transition.from, compact);
    assert_eq!(transition.target, wide);
    assert!(transition.active);

    let midpoint = transition.at(0.5);
    assert!(
        (midpoint.center_width - (compact.center_width + wide.center_width) * 0.5).abs() < 0.001
    );
    assert!((midpoint.padding - (compact.padding + wide.padding) * 0.5).abs() < 0.001);

    let settled = motion.prepare(false, wide, now + crate::motion::PANEL_DURATION, false);
    assert_eq!(settled.from, wide);
    assert_eq!(settled.target, wide);
    assert!(!settled.active);
}

#[test]
fn relocating_control_switches_geometry_only_while_fully_faded_out() {
    let from = RightControlGeometry {
        left: 20.,
        top: 17.,
        width: 42.,
        height: 24.,
    };
    let target = RightControlGeometry {
        left: 240.,
        top: 30.,
        width: 120.,
        height: 34.,
    };

    assert_eq!(fade_motion_geometry(from, target, 0.499), from);
    assert_eq!(fade_motion_geometry(from, target, 0.5), target);
    assert_eq!(fade_motion_opacity(1., 1., 0.5), 0.);
}

#[test]
fn relocating_control_same_mode_resize_keeps_fade_epoch_and_start_time() {
    let compact = RightControlGeometry {
        left: 20.,
        top: 17.,
        width: 42.,
        height: 24.,
    };
    let wide = RightControlGeometry {
        left: 240.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let resized_wide = RightControlGeometry {
        left: 260.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let started_at = Instant::now();
    let resize_at = started_at + crate::motion::PANEL_DURATION / 4;
    let mut motion = FadeMotion::default();

    motion.prepare(true, compact, started_at, false, false);
    let transition = motion.prepare(false, wide, started_at, true, false);
    let retargeted = motion.prepare(false, resized_wide, resize_at, true, false);

    assert!(transition.active);
    assert!(retargeted.active);
    assert_eq!(retargeted.epoch, transition.epoch);
    assert_eq!(retargeted.started_at, transition.started_at);
    assert_eq!(retargeted.from, compact);
    assert_eq!(retargeted.target, resized_wide);
}

#[test]
fn relocating_control_reversal_starts_from_current_displayed_state() {
    let compact = RightControlGeometry {
        left: 20.,
        top: 17.,
        width: 42.,
        height: 24.,
    };
    let wide = RightControlGeometry {
        left: 240.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let started_at = Instant::now();
    let reverse_at = started_at + crate::motion::PANEL_DURATION / 4;
    let mut motion = FadeMotion::default();

    motion.prepare(true, compact, started_at, false, false);
    motion.prepare(false, wide, started_at, true, false);
    let (displayed_geometry, displayed_opacity) = motion.displayed_at(reverse_at);
    let reversed = motion.prepare(true, compact, reverse_at, true, false);

    assert!(reversed.active);
    assert_eq!(reversed.from, displayed_geometry);
    assert!((reversed.from_opacity - displayed_opacity).abs() < 0.000001);
}

#[test]
fn wide_current_track_uses_the_entire_side_column() {
    let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1600., false));

    assert!(layout.side_width > 380.);
    assert_eq!(current_block_width(layout), layout.side_width);
}

#[test]
fn wide_favorite_stays_after_the_intrinsic_text_block() {
    let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));

    assert_eq!(favorite_left_for_text_width(layout, false, 120.), 204.);
    assert_eq!(favorite_left_for_text_width(layout, false, 0.), 84.);
}

#[test]
fn compact_favorite_uses_the_center_column_anchor() {
    let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));

    assert_eq!(favorite_left_for_text_width(layout, true, 0.), 483.);
    assert_eq!(favorite_top(true), 2.);
    assert_eq!(favorite_top(false), 13.);
}

#[test]
fn text_and_favorite_keep_a_gap_through_the_mode_transition() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let intrinsic_text_width = 1000.;
    let compact_text_width =
        current_text_width_for_layout(compact, true, true, intrinsic_text_width);
    let wide_text_width = current_text_width_for_layout(wide, false, true, intrinsic_text_width);
    let compact_favorite_left = favorite_left_for_text_width(compact, true, 0.);
    let wide_favorite_left = favorite_left_for_text_width(wide, false, wide_text_width);

    for step in 0..=10 {
        let delta = step as f32 / 10.;
        let text_width = crate::motion::lerp(compact_text_width, wide_text_width, delta);
        let favorite_left = crate::motion::lerp(compact_favorite_left, wide_favorite_left, delta);
        let text_right = 60. + 12. + text_width;
        assert!(
            text_right + 11. <= favorite_left + 0.001,
            "text and favorite overlap at transition delta {delta}"
        );
    }

    assert_eq!(
        current_text_available_width(compact, true, true),
        compact_text_width
    );
    assert_eq!(
        current_text_available_width(wide, false, true),
        wide_text_width
    );
}

#[test]
fn favorite_measurement_uses_the_artist_line_text() {
    let routes = vec![
        MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "1".into(),
            title: "Primary".into(),
        },
        MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "2".into(),
            title: "Collaborator".into(),
        },
    ];

    assert_eq!(
        rendered_artist_text("Primary, Collaborator", Provider::Deezer, &routes),
        "Primary, Collaborator"
    );
    assert_eq!(
        rendered_artist_text("Primary, Guest", Provider::Deezer, &routes),
        "Primary, Collaborator"
    );
    assert_eq!(
        rendered_artist_text("Primary, Guest", Provider::SoundCloud, &routes[..1]),
        "Primary, Guest"
    );
}

#[test]
fn live_resize_keeps_the_active_mode_transition_clock_stable() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let resized = PlayerBarLayout::from_geometry(desktop_player_geometry(1500., false));
    let started_at = Instant::now();
    let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = PlayerBarLayoutMotion::default();

    motion.prepare(true, compact, started_at, false);
    let transition = motion.prepare(false, wide, started_at, false);
    let transition_started_at = motion.started_at;
    let progress_before_resize = motion.animation_progress(resize_at);
    let retargeted = motion.prepare(false, resized, resize_at, false);

    assert!(retargeted.active);
    assert!(!retargeted.mode_changed);
    assert_eq!(retargeted.from, transition.from);
    assert_eq!(retargeted.target, resized);
    assert_eq!(retargeted.epoch, transition.epoch);
    assert_eq!(motion.started_at, transition_started_at);
    assert_eq!(motion.animation_progress(resize_at), progress_before_resize);
}

#[test]
fn repeated_same_mode_resize_does_not_restart_the_layout_transition() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let resized_once = PlayerBarLayout::from_geometry(desktop_player_geometry(1500., false));
    let resized_twice = PlayerBarLayout::from_geometry(desktop_player_geometry(1600., false));
    let started_at = Instant::now();
    let resize_once_at = started_at + crate::motion::PANEL_DURATION / 3;
    let resize_twice_at = resize_once_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = PlayerBarLayoutMotion::default();

    motion.prepare(true, compact, started_at, false);
    let transition = motion.prepare(false, wide, started_at, false);
    let transition_started_at = motion.started_at;
    let first = motion.prepare(false, resized_once, resize_once_at, false);
    let second = motion.prepare(false, resized_twice, resize_twice_at, false);

    assert!(first.active);
    assert!(second.active);
    assert_eq!(first.from, transition.from);
    assert_eq!(second.from, transition.from);
    assert_eq!(first.epoch, transition.epoch);
    assert_eq!(second.epoch, transition.epoch);
    assert_eq!(second.target, resized_twice);
    assert_eq!(motion.started_at, transition_started_at);
}

#[test]
fn mode_reversal_still_rebases_from_the_current_layout() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let reversed_compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1439., true));
    let started_at = Instant::now();
    let reverse_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = PlayerBarLayoutMotion::default();

    motion.prepare(true, compact, started_at, false);
    let outward = motion.prepare(false, wide, started_at, false);
    let displayed = motion.displayed_at(reverse_at);
    let reversed = motion.prepare(true, reversed_compact, reverse_at, false);

    assert!(reversed.active);
    assert!(reversed.mode_changed);
    assert_eq!(reversed.from, displayed);
    assert_eq!(reversed.target, reversed_compact);
    assert_eq!(reversed.epoch, outward.epoch.wrapping_add(1));
    assert_eq!(motion.started_at, Some(reverse_at));
}

#[test]
fn breakpoint_wiggle_restarts_only_when_the_mode_actually_changes() {
    let started_at = Instant::now();
    let mut motion = PlayerBarLayoutMotion::default();
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    motion.prepare(true, compact, started_at, false);

    let wide_at = started_at + Duration::from_millis(20);
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let outward = motion.prepare(false, wide, wide_at, false);
    assert!(outward.mode_changed);
    assert_eq!(motion.started_at, Some(wide_at));

    let wide_resize_at = wide_at + Duration::from_millis(12);
    let wider = PlayerBarLayout::from_geometry(desktop_player_geometry(1460., false));
    let same_wide = motion.prepare(false, wider, wide_resize_at, false);
    assert!(!same_wide.mode_changed);
    assert_eq!(same_wide.epoch, outward.epoch);
    assert_eq!(motion.started_at, Some(wide_at));

    let compact_at = wide_resize_at + Duration::from_millis(12);
    let compact_again = PlayerBarLayout::from_geometry(desktop_player_geometry(1439., true));
    let reversed = motion.prepare(true, compact_again, compact_at, false);
    assert!(reversed.mode_changed);
    assert_eq!(reversed.epoch, outward.epoch.wrapping_add(1));
    assert_eq!(motion.started_at, Some(compact_at));

    let compact_resize_at = compact_at + Duration::from_millis(12);
    let narrower = PlayerBarLayout::from_geometry(desktop_player_geometry(1420., true));
    let same_compact = motion.prepare(true, narrower, compact_resize_at, false);
    assert!(!same_compact.mode_changed);
    assert_eq!(same_compact.epoch, reversed.epoch);
    assert_eq!(motion.started_at, Some(compact_at));
}

#[test]
fn scalar_motion_retargets_text_width_from_the_displayed_value() {
    let started_at = Instant::now();
    let retarget_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = ScalarMotion::default();

    motion.prepare(0., started_at, true, false, false);
    let first = motion.prepare(240., started_at, true, true, false);
    assert!(first.active);
    let displayed = motion.displayed_at(retarget_at);
    let retargeted = motion.prepare(96., retarget_at, true, true, false);

    assert!(retargeted.active);
    assert_eq!(retargeted.from, displayed);
    assert_eq!(retargeted.target, 96.);
}

#[test]
fn scalar_motion_updates_resize_targets_without_restarting() {
    let started_at = Instant::now();
    let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = ScalarMotion::default();

    motion.prepare(0., started_at, true, false, false);
    let transition = motion.prepare(240., started_at, true, true, false);
    let transition_started_at = motion.started_at;
    let resized = motion.prepare(180., resize_at, true, false, false);

    assert!(resized.active);
    assert_eq!(resized.from, transition.from);
    assert_eq!(resized.target, 180.);
    assert_eq!(resized.epoch, transition.epoch);
    assert_eq!(motion.started_at, transition_started_at);
}

#[test]
fn rect_motion_retargets_right_control_from_the_displayed_geometry() {
    let started_at = Instant::now();
    let retarget_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = RectMotion::default();
    let from = RightControlGeometry {
        left: 20.,
        top: 17.,
        width: 34.,
        height: 24.,
    };
    let target = RightControlGeometry {
        left: 240.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let resized = RightControlGeometry {
        left: 180.,
        top: 28.,
        width: 100.,
        height: 34.,
    };

    motion.prepare(from, started_at, true, false, false);
    motion.prepare(target, started_at, true, true, false);
    let displayed = motion.displayed_at(retarget_at);
    let retargeted = motion.prepare(resized, retarget_at, true, true, false);

    assert!(retargeted.active);
    assert_eq!(retargeted.from, displayed);
    assert_eq!(retargeted.target, resized);
}

#[test]
fn rect_motion_updates_resize_targets_without_restarting() {
    let started_at = Instant::now();
    let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
    let mut motion = RectMotion::default();
    let from = RightControlGeometry {
        left: 20.,
        top: 17.,
        width: 34.,
        height: 24.,
    };
    let target = RightControlGeometry {
        left: 240.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let resized = RightControlGeometry {
        left: 210.,
        top: 30.,
        width: 110.,
        height: 34.,
    };

    motion.prepare(from, started_at, true, false, false);
    let transition = motion.prepare(target, started_at, true, true, false);
    let transition_started_at = motion.started_at;
    let resized_visual = motion.prepare(resized, resize_at, true, false, false);

    assert!(resized_visual.active);
    assert_eq!(resized_visual.from, transition.from);
    assert_eq!(resized_visual.target, resized);
    assert_eq!(resized_visual.epoch, transition.epoch);
    assert_eq!(motion.started_at, transition_started_at);
}

#[test]
fn scalar_and_rect_motion_snap_when_reduced_motion_is_enabled() {
    let now = Instant::now();
    let mut scalar = ScalarMotion::default();
    scalar.prepare(12., now, true, false, true);
    let scalar_visual = scalar.prepare(80., now, true, true, true);
    assert_eq!(scalar_visual.from, 80.);
    assert_eq!(scalar_visual.target, 80.);
    assert!(!scalar_visual.active);

    let mut rect = RectMotion::default();
    rect.prepare(
        RightControlGeometry {
            left: 0.,
            top: 0.,
            width: 34.,
            height: 34.,
        },
        now,
        true,
        false,
        true,
    );
    let target = RightControlGeometry {
        left: 80.,
        top: 30.,
        width: 120.,
        height: 34.,
    };
    let rect_visual = rect.prepare(target, now, true, true, true);
    assert_eq!(rect_visual.from, target);
    assert_eq!(rect_visual.target, target);
    assert!(!rect_visual.active);
}

#[test]
fn compact_wide_layout_reduced_motion_snaps_without_repaint_state() {
    let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
    let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
    let now = Instant::now();
    let mut motion = PlayerBarLayoutMotion::default();

    motion.prepare(true, compact, now, false);
    let snapped = motion.prepare(false, wide, now, true);

    assert_eq!(snapped.from, wide);
    assert_eq!(snapped.target, wide);
    assert!(!snapped.active);
    assert_eq!(motion.started_at, None);
}

#[test]
fn compact_geometry_matches_the_900px_reference() {
    assert_eq!(desktop_player_geometry(900., true), (16., 12., 360., 242.),);
}

#[test]
fn compact_geometry_reaches_440px_center_at_1440px() {
    assert_eq!(desktop_player_geometry(1440., true), (16., 12., 440., 472.),);
}

#[test]
fn compact_right_keeps_action_track_before_second_column() {
    for width in 769..=1440 {
        let width = width as f32;
        let (content_width, action_width, action_gap, volume_shift, second_column_width) =
            super::compact_player_right_geometry(
                super::desktop_player_geometry(width, true).3,
                width,
            );
        let action_content_width = 68. + action_gap;
        let action_right = (action_width + action_content_width) * 0.5;
        let second_column_left = action_width - volume_shift;
        assert!(
            action_right <= second_column_left + 0.001,
            "compact right controls overlap at {width}px"
        );
        assert!(
            action_width + second_column_width <= content_width + 0.001,
            "compact right controls overflow at {width}px"
        );
    }
}

#[test]
fn compact_volume_shifts_two_pixels_left_without_moving_quality() {
    for width in 769..=1440 {
        let width = width as f32;
        let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(width, true));
        let (_, action_width, _, _, _) =
            super::compact_player_right_geometry(layout.side_width, width);
        let quality = super::right_control_geometry(
            super::RightControlKind::Quality,
            layout,
            width,
            true,
            QUALITY_BADGE_MIN_WIDTH_PX,
        );
        let volume = super::right_control_geometry(
            super::RightControlKind::Volume,
            layout,
            width,
            true,
            QUALITY_BADGE_MIN_WIDTH_PX,
        );
        assert_eq!(
            quality.left, action_width,
            "compact quality left moved at {width}px"
        );
        assert_eq!(
            volume.left,
            action_width - 2.,
            "compact volume left is not 2px left at {width}px"
        );
    }
}

#[test]
fn wide_geometry_switches_to_the_520px_center_at_1441px() {
    assert_eq!(
        desktop_player_geometry(1441., false),
        (16., 20., 520., 424.5),
    );
}

#[test]
fn quality_badge_keeps_its_box_and_offsets_only_the_text() {
    assert_eq!(QUALITY_BADGE_HEIGHT_PX, 20.);
    assert_eq!(QUALITY_BADGE_MIN_WIDTH_PX, 42.0);
    assert_eq!(QUALITY_BADGE_TEXT_OFFSET_PX, -1.);
    assert_eq!(PLAYER_CLOSE_GLYPH_PX, 9.);
}

#[test]
fn quality_text_animation_identity_is_stable_across_generations() {
    let previous_generation_label = quality_text_animation_key("FLAC");
    let next_generation_label = quality_text_animation_key("FLAC");

    assert_eq!(previous_generation_label, next_generation_label);
}

#[test]
fn quality_text_animation_identity_changes_for_a_different_label() {
    assert_ne!(
        quality_text_animation_key("FLAC"),
        quality_text_animation_key("MP3")
    );
}

#[test]
fn quality_badge_animation_identity_changes_across_generations_for_same_label() {
    assert_ne!(
        quality_badge_animation_key(1, "FLAC"),
        quality_badge_animation_key(2, "FLAC")
    );
}

#[test]
fn quality_badge_animation_identity_is_stable_for_same_generation_and_label() {
    assert_eq!(
        quality_badge_animation_key(1, "FLAC"),
        quality_badge_animation_key(1, "FLAC")
    );
}

#[test]
fn quality_label_clears_stale_label_on_generation_change_before_resolution() {
    let mut label = String::from("FLAC");
    let mut generation = Some(1);

    update_quality_label_for_generation(&mut label, &mut generation, 2, None);

    assert_eq!(label, "");
    assert_eq!(generation, Some(2));
}

#[test]
fn quality_label_retains_known_quality_when_unavailable_in_same_generation() {
    let mut label = String::from("FLAC");
    let mut generation = Some(1);

    update_quality_label_for_generation(&mut label, &mut generation, 1, None);

    assert_eq!(label, "FLAC");
    assert_eq!(generation, Some(1));
}

#[test]
fn quality_label_stores_immediate_quality_on_generation_change() {
    let mut label = String::from("FLAC");
    let mut generation = Some(1);

    update_quality_label_for_generation(&mut label, &mut generation, 2, Some("MP3"));

    assert_eq!(label, "MP3");
    assert_eq!(generation, Some(2));
}

#[test]
fn quality_badge_opacity_fades_only_when_the_shell_appears() {
    assert_eq!(quality_badge_opacity_endpoints("", ""), (0., 0.));
    assert_eq!(quality_badge_opacity_endpoints("FLAC", ""), (0., 0.));
    assert_eq!(quality_badge_opacity_endpoints("", "FLAC"), (0., 1.));
    assert_eq!(quality_badge_opacity_endpoints("FLAC", "MP3"), (1., 1.));
    assert_eq!(quality_badge_opacity_endpoints("FLAC", "FLAC"), (1., 1.));
}

#[test]
fn quality_text_opacity_fades_first_and_different_labels_only() {
    assert_eq!(quality_text_opacity_endpoints("", ""), (0., 0.));
    assert_eq!(quality_text_opacity_endpoints("", "FLAC"), (0., 1.));
    assert_eq!(quality_text_opacity_endpoints("FLAC", "MP3"), (0., 1.));
    assert_eq!(quality_text_opacity_endpoints("FLAC", "FLAC"), (1., 1.));
}

#[test]
fn quality_text_opacity_keeps_the_retained_label_visible() {
    let mut label = String::from("FLAC");

    update_last_quality_label(&mut label, None);

    assert_eq!(label, "FLAC");
    assert_eq!(quality_text_opacity_endpoints("FLAC", &label), (1., 1.));
}

#[test]
fn quality_badge_retains_the_previous_label_while_unavailable() {
    let mut label = String::from("FLAC");

    update_last_quality_label(&mut label, None);
    assert_eq!(label, "FLAC");

    update_last_quality_label(&mut label, Some("MP3"));
    assert_eq!(label, "MP3");

    update_last_quality_label(&mut label, None);
    assert_eq!(label, "MP3");

    update_last_quality_label(&mut label, Some(""));
    assert_eq!(label, "MP3");
}

#[test]
fn close_player_uses_the_five_pixel_right_inset() {
    assert_eq!(CLOSE_PLAYER_RIGHT_PX, 5.);
}

#[test]
fn close_player_tooltip_gap_matches_layout_density() {
    assert_eq!(CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX, 5.);
    assert_eq!(CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX, 3.);
    assert_eq!(close_player_tooltip_gap(true), 5.);
    assert_eq!(close_player_tooltip_gap(false), 3.);
}

#[test]
fn player_action_buttons_use_the_original_radius() {
    assert_eq!(PLAYER_ACTION_BUTTON_RADIUS_PX, 6.);
}

#[test]
fn repeat_one_badge_uses_the_original_inner_control_geometry() {
    assert_eq!(REPEAT_CONTROL_SIZE_PX, 14.);
    assert_eq!(REPEAT_ONE_BADGE_RIGHT_PX, 2.);
    assert_eq!(REPEAT_ONE_BADGE_BOTTOM_PX, 4.);
}

#[test]
fn volume_control_geometry_matches_the_original() {
    assert_eq!(VOLUME_GAP_PX, 7.);
    assert_eq!(VOLUME_MUTE_BUTTON_PX, 34.);
    assert_eq!(VOLUME_SLIDER_WIDTH_PX, 80.);
    assert_eq!(VOLUME_SLIDER_CONTROL_HEIGHT_PX, 24.);
    assert_eq!(VOLUME_TRACK_HEIGHT_PX, 4.);
    assert_eq!(VOLUME_THUMB_DIAMETER_PX, 13.);
    assert_eq!(VOLUME_ICON_FRAME_PX, 16.);
    assert_eq!(VOLUME_ICON_EM_HEIGHT_PX, 14.5);
}

#[test]
fn wide_volume_inset_moves_only_the_volume_control() {
    assert_eq!(WIDE_VOLUME_INSET_PX, 12.);
    assert_eq!(wide_volume_inset(true), 12.);
    assert_eq!(wide_volume_inset(false), 0.);
}

#[test]
fn compact_volume_container_offsets_nudge_two_pixels_left() {
    assert_eq!(volume_container_offsets(false), (-2., 0.));
    assert_eq!(volume_container_offsets(true), (14., WIDE_VOLUME_INSET_PX));
}

#[test]
fn volume_icons_preserve_fontawesome_viewbox_widths() {
    assert_eq!(
        volume_icon_dimensions(VolumeIconLevel::High),
        (18.125, 14.5)
    );
    assert_eq!(
        volume_icon_dimensions(VolumeIconLevel::Low),
        (12.6875, 14.5)
    );
    assert_eq!(volume_icon_dimensions(VolumeIconLevel::Off), (9.0625, 14.5));
    assert_eq!(
        volume_icon_dimensions(VolumeIconLevel::Muted),
        (16.3125, 14.5)
    );
}

#[test]
fn volume_speaker_bodies_share_a_fixed_left_anchor() {
    assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::High), -0.90625);
    assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Low), 0.);
    assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Off), 0.);
    assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Muted), 0.);
}

#[test]
fn ended_queue_keeps_the_last_track_artist_visible() {
    let track = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "7".into(),
        title: "Last Song".into(),
        artist: "Last Artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        service_url: String::new(),
    };
    assert_eq!(
        current_subtitle(Some(&track), PlaybackStatus::Ended, false),
        "Last Artist"
    );
    assert_eq!(
        current_subtitle(Some(&track), PlaybackStatus::Playing, false),
        "Last Artist"
    );
    assert_eq!(
        rendered_current_artist_text(Some(&track), PlaybackStatus::Ended, "Last Artist", None),
        "Last Artist"
    );
    assert_eq!(
        current_subtitle(Some(&track), PlaybackStatus::Failed, false),
        "Playback failed"
    );
    assert_eq!(
        rendered_current_artist_text(
            Some(&track),
            PlaybackStatus::Failed,
            "Playback failed",
            None
        ),
        "Playback failed"
    );
    assert_ne!(
        current_subtitle(Some(&track), PlaybackStatus::Playing, false),
        "engine exploded"
    );
}

#[test]
fn cached_loading_keeps_the_artist_visible() {
    let track = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "7".into(),
        title: "Cached Song".into(),
        artist: "Cached Artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        service_url: String::new(),
    };

    assert_eq!(
        current_subtitle(Some(&track), PlaybackStatus::Loading, true),
        "Cached Artist"
    );
    assert_eq!(
        current_subtitle(Some(&track), PlaybackStatus::Loading, false),
        "Loading audio..."
    );
}

#[test]
fn pending_load_titles_the_bar_instead_of_claiming_nothing_plays() {
    let track = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "7".into(),
        title: "Pending Song".into(),
        artist: "Pending Artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        service_url: String::new(),
    };

    assert_eq!(
        current_track_title(Some(&track), PlaybackStatus::Loading),
        "Pending Song"
    );
    assert_eq!(
        current_track_title(None, PlaybackStatus::Loading),
        "Loading..."
    );
    assert_eq!(
        current_track_title(None, PlaybackStatus::Empty),
        "Nothing playing"
    );
}
