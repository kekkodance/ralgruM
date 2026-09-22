use std::cell::RefCell;
use std::time::Duration;

use super::{
    DownloadProgress, SEEK_SLIDER_STEP, SeekSliderAction, SeekSliderInteraction, adjacent_track,
    apply_buffered_progress, apply_download_progress, apply_timeline_suffix_progress,
    completed_download_size, effective_sink_gain, listen_history_changed_on_completion,
    listen_history_scope_matches, media_play_should_toggle, pause_silently, player_format_label,
    quality_label, seek_slider_accessibility_commit, seek_slider_action,
};
use crate::playback::fade::{USER_FADE_DURATION, USER_FADE_SETTLE_TIMEOUT};
use crate::playback::progressive::TimelineSuffixState;
use crate::playback::{PlaybackProvider, PlaybackState, PlaybackStatus, PlaybackTrack};

use gpui_component::slider::{SliderEvent, SliderValue};

#[test]
fn paused_playback_keeps_the_effective_sink_gain_silent() {
    assert_eq!(effective_sink_gain(PlaybackStatus::Paused, 0.8), 0.0);
    assert_eq!(effective_sink_gain(PlaybackStatus::Paused, 0.2), 0.0);
    assert_eq!(effective_sink_gain(PlaybackStatus::Playing, 0.8), 0.8);
}

#[test]
fn silent_pause_mutes_before_pausing() {
    let steps = RefCell::new(Vec::new());
    pause_silently(
        || {
            steps.borrow_mut().push("mute");
        },
        || steps.borrow_mut().push("pause"),
    );
    assert_eq!(*steps.borrow(), ["mute", "pause"]);
}

#[test]
fn user_fade_settle_timeout_has_room_for_the_sample_ramp() {
    assert!(USER_FADE_SETTLE_TIMEOUT > USER_FADE_DURATION);
    assert_eq!(
        USER_FADE_SETTLE_TIMEOUT,
        USER_FADE_DURATION + Duration::from_millis(250)
    );
}

#[test]
fn quality_label_combines_format_and_computed_bitrate() {
    assert_eq!(
        quality_label("FLAC", None, Some(1_000_000), Some(Duration::from_secs(10))),
        Some("FLAC 800kbps".into())
    );
    assert_eq!(
        quality_label("MP3", None, Some(161_250), Some(Duration::from_secs(10))),
        Some("MP3 128kbps".into())
    );
    assert_eq!(
        quality_label("MP3", None, None, Some(Duration::from_secs(10))),
        Some("MP3".into())
    );
    assert_eq!(
        quality_label("MP3", None, Some(100), Some(Duration::from_secs(0))),
        Some("MP3".into())
    );
    assert_eq!(
        quality_label("MP3", None, Some(0), Some(Duration::from_secs(5))),
        Some("MP3".into())
    );
    assert_eq!(quality_label("MP3", None, Some(100), None), None);
    assert_eq!(
        quality_label(
            player_format_label(super::super::resolver::AudioFormat::M4a),
            Some(160),
            None,
            None
        ),
        Some("AAC 160kbps".into())
    );
    assert_eq!(
        quality_label(
            player_format_label(super::super::resolver::AudioFormat::M4a),
            Some(160),
            Some(100),
            Some(Duration::from_secs(1))
        ),
        Some("AAC 160kbps".into())
    );
}

#[test]
fn deezer_history_changes_only_after_a_successful_completed_listen() {
    assert!(listen_history_changed_on_completion(true, true));
    assert!(!listen_history_changed_on_completion(true, false));
    assert!(!listen_history_changed_on_completion(false, true));
    assert!(!listen_history_changed_on_completion(false, false));
}

#[test]
fn listen_history_completion_requires_the_same_account_scope() {
    assert!(listen_history_scope_matches("account-a", "account-a"));
    assert!(!listen_history_scope_matches("account-a", "account-b"));
}

#[test]
fn background_prefetch_is_preference_gated_and_uses_the_next_track() {
    let mut state = PlaybackState::default();
    let tracks = ["current", "next"].map(|id| PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: id.into(),
        title: id.into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::ZERO,
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    });
    state.replace(tracks.into(), 0);

    assert!(adjacent_track(false, &state).is_none());
    assert_eq!(
        adjacent_track(true, &state).map(|track| track.id.as_str()),
        Some("next")
    );
    state.select(1);
    assert!(adjacent_track(true, &state).is_none());
}

#[test]
fn media_play_restarts_ended_playback() {
    assert!(!media_play_should_toggle(PlaybackStatus::Empty));
    assert!(media_play_should_toggle(PlaybackStatus::Loading));
    assert!(media_play_should_toggle(PlaybackStatus::Paused));
    assert!(media_play_should_toggle(PlaybackStatus::Ended));
    assert!(!media_play_should_toggle(PlaybackStatus::Playing));
    assert!(!media_play_should_toggle(PlaybackStatus::Failed));
}

#[test]
fn seek_slider_previews_changes_and_commits_once_on_release() {
    assert_eq!(SEEK_SLIDER_STEP, 0.01);
    assert_eq!(
        seek_slider_action(&SliderEvent::Change(SliderValue::Single(0.42))),
        SeekSliderAction::Preview(0.42)
    );
    assert_eq!(
        seek_slider_action(&SliderEvent::Release(SliderValue::Single(0.42))),
        SeekSliderAction::Commit(0.42)
    );
}

#[test]
fn seek_slider_click_is_a_preview_followed_by_one_commit() {
    let click = [
        seek_slider_action(&SliderEvent::Change(SliderValue::Single(0.75))),
        seek_slider_action(&SliderEvent::Release(SliderValue::Single(0.75))),
    ];
    assert_eq!(
        click,
        [
            SeekSliderAction::Preview(0.75),
            SeekSliderAction::Commit(0.75)
        ]
    );
}

#[test]
fn seek_pointer_release_commits_once() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.5025));
    assert_eq!(interaction.commit(7, 0.5025), Some(0.5025));
    assert_eq!(interaction.commit(7, 0.5025), None);
}

#[test]
fn seek_slider_ignores_range_events() {
    assert_eq!(
        seek_slider_action(&SliderEvent::Change(SliderValue::Range(0.2, 0.8))),
        SeekSliderAction::Ignore
    );
    assert_eq!(
        seek_slider_action(&SliderEvent::Release(SliderValue::Range(0.2, 0.8))),
        SeekSliderAction::Ignore
    );
}

#[test]
fn seek_slider_actions_clamp_to_the_playback_range() {
    assert_eq!(
        seek_slider_action(&SliderEvent::Change(SliderValue::Single(-0.2))),
        SeekSliderAction::Preview(0.0)
    );
    assert_eq!(
        seek_slider_action(&SliderEvent::Release(SliderValue::Single(1.2))),
        SeekSliderAction::Commit(1.0)
    );
}

#[test]
fn stale_release_after_track_transition_is_ignored() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));

    interaction.cancel();

    assert_eq!(interaction.commit(8, 0.75), None);
}

#[test]
fn held_drag_cannot_rearm_until_release() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));
    interaction.cancel();

    assert!(!interaction.preview(8, 0.5));
    assert_eq!(interaction.commit(8, 0.5), None);
    assert!(interaction.preview(8, 0.75));
    assert_eq!(interaction.commit(8, 0.75), Some(0.75));
}

#[test]
fn slider_accessibility_value_commits_only_when_enabled_and_not_previewing() {
    assert_eq!(
        seek_slider_accessibility_commit(SliderValue::Single(0.31), Some(0.2), None),
        Some(0.31)
    );
    assert_eq!(
        seek_slider_accessibility_commit(SliderValue::Single(0.31), Some(0.2), Some(0.31)),
        None
    );
    assert_eq!(
        seek_slider_accessibility_commit(SliderValue::Single(0.31), None, None),
        None
    );
}

#[test]
fn cancelled_pointer_change_is_suppressed_before_accessibility_inference() {
    let mut interaction = SeekSliderInteraction::default();
    let value = SliderValue::Single(0.6);
    interaction.record_pointer_change(value);
    interaction.preview(7, 0.6);
    interaction.cancel();

    let should_seek = !interaction.suppress_pointer_change(value)
        && seek_slider_accessibility_commit(value, Some(0.2), interaction.preview_fraction)
            .is_some();
    assert!(!should_seek);
}

#[test]
fn seamless_cancel_stays_blocked_until_release() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));
    interaction.cancel();

    assert!(!interaction.preview(8, 0.5));
    assert_eq!(interaction.commit(8, 0.5), None);
    assert!(interaction.preview(8, 0.75));
    assert_eq!(interaction.commit(8, 0.75), Some(0.75));
}

#[test]
fn disabled_cycle_allows_the_first_new_click() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));
    interaction.set_control_enabled(false);
    assert_eq!(interaction.preview_fraction, None);
    assert!(!interaction.preview(7, 0.5));

    interaction.set_control_enabled(true);
    interaction.begin_new_pointer_interaction();
    assert!(interaction.preview(8, 0.75));
    assert_eq!(interaction.commit(8, 0.75), Some(0.75));
}

#[test]
fn held_drag_across_disabled_cycle_stays_rejected_without_new_pointer_down() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));
    interaction.set_control_enabled(false);
    interaction.set_control_enabled(true);

    assert!(!interaction.preview(8, 0.5));
    assert_eq!(interaction.commit(8, 0.5), None);
}

#[test]
fn fresh_pointer_down_after_disabled_cycle_rearms_the_first_click() {
    let mut interaction = SeekSliderInteraction::default();
    assert!(interaction.preview(7, 0.25));
    interaction.set_control_enabled(false);
    interaction.set_control_enabled(true);
    interaction.begin_new_pointer_interaction();

    assert!(interaction.preview(8, 0.75));
    assert_eq!(interaction.commit(8, 0.75), Some(0.75));
}

#[test]
fn progressive_download_progress_updates_buffered_time_after_load() {
    let mut state = PlaybackState::default();
    let track = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "progressive".into(),
        title: "Progressive".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::ZERO,
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let generation = state.replace(vec![track], 0).unwrap();
    assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
    assert_eq!(state.buffered, Duration::ZERO);

    assert!(apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 25,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(5));
    assert!(apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 75,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(15));
    assert!(!apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 25,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(15));
    assert!(!apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 75,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));

    let mut full = PlaybackState::default();
    let generation = full
        .replace(vec![state.current().unwrap().clone()], 0)
        .unwrap();
    assert!(full.loaded_fully_buffered(generation, Some(Duration::from_secs(20))));
    assert_eq!(full.buffered, Duration::from_secs(20));
}

#[test]
fn cached_initial_progress_marks_the_buffer_full_without_regressing() {
    let mut progress = DownloadProgress {
        generation: 7,
        downloaded: 24,
        total: Some(100),
        buffered_fraction: None,
        fully_buffered: false,
    };
    progress.adopt_initial(DownloadProgress {
        generation: 7,
        downloaded: 100,
        total: Some(100),
        buffered_fraction: None,
        fully_buffered: true,
    });
    assert!(progress.fully_buffered);
    assert_eq!(progress.fraction(), Some(1.0));

    progress.adopt_initial(DownloadProgress {
        generation: 7,
        downloaded: 25,
        total: Some(100),
        buffered_fraction: None,
        fully_buffered: false,
    });
    assert_eq!(progress.downloaded, 100);
    assert!(progress.fully_buffered);
}

#[test]
fn explicit_buffered_fraction_updates_without_a_byte_total() {
    let mut state = PlaybackState::default();
    let track = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::SoundCloud,
        id: "hls".into(),
        title: "HLS".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(20),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let generation = state.replace(vec![track], 0).unwrap();
    assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
    assert!(apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 100,
            total: None,
            buffered_fraction: Some(0.25),
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(5));
    assert!(!apply_download_progress(
        &mut state,
        DownloadProgress {
            generation,
            downloaded: 80,
            total: None,
            buffered_fraction: Some(0.1),
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(5));
}

/// Front and suffix ranges must stay separate when a seek leaves a gap.
#[test]
fn pending_timeline_seek_keeps_front_and_suffix_ranges_separate() {
    let mut state = PlaybackState::default();
    let track = PlaybackTrack {
        downloadable: false,
        progressive: true,
        provider: PlaybackProvider::Deezer,
        id: "fresh".into(),
        title: "Fresh".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let generation = state.replace(vec![track], 0).unwrap();
    state.loaded_progressive(generation, Some(Duration::from_secs(10)));

    let pending = TimelineSuffixState {
        pending: true,
        base: Duration::from_secs(9),
        written: 50,
        total: 100,
    };
    assert!(apply_buffered_progress(
        &mut state,
        Some(DownloadProgress {
            generation,
            downloaded: 30,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        }),
        Some(pending),
    ));
    assert_eq!(
        state.buffered,
        Duration::from_secs(3),
        "the front indicator must stop at the actual front frontier"
    );
    assert_eq!(
        state.suffix_buffered,
        Some((Duration::from_secs(9), Duration::from_millis(9_500)))
    );

    assert!(apply_timeline_suffix_progress(
        &mut state,
        &TimelineSuffixState {
            pending: true,
            base: Duration::from_secs(9),
            written: 100,
            total: 100,
        },
    ));
    assert_eq!(
        state.suffix_buffered,
        Some((Duration::from_secs(9), Duration::from_secs(10))),
        "the completed suffix is a distinct downloaded range"
    );
}

#[test]
fn pending_backward_seek_preserves_the_downloaded_frontier() {
    let mut state = PlaybackState::default();
    state.status = PlaybackStatus::Playing;
    state.duration = Duration::from_secs(10);
    state.buffered = Duration::from_secs(10);

    assert!(!apply_timeline_suffix_progress(
        &mut state,
        &TimelineSuffixState {
            pending: true,
            base: Duration::from_secs(2),
            written: 0,
            total: 100,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(10));
    assert_eq!(state.suffix_buffered, None);
}

/// The suffix may advance independently without filling the gap in front.
#[test]
fn landed_suffix_and_front_progress_keep_the_indicator_honest() {
    let mut state = PlaybackState::default();
    let track = PlaybackTrack {
        downloadable: false,
        progressive: true,
        provider: PlaybackProvider::Deezer,
        id: "fresh".into(),
        title: "Fresh".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let generation = state.replace(vec![track], 0).unwrap();
    state.loaded_progressive(generation, Some(Duration::from_secs(10)));

    let landed = TimelineSuffixState {
        pending: false,
        base: Duration::from_secs(9),
        written: 50,
        total: 100,
    };
    assert!(apply_buffered_progress(
        &mut state,
        Some(DownloadProgress {
            generation,
            downloaded: 80,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        }),
        Some(landed),
    ));
    assert_eq!(
        state.buffered,
        Duration::from_secs(8),
        "the front indicator must not span the missing middle section"
    );
    assert_eq!(
        state.suffix_buffered,
        Some((Duration::from_secs(9), Duration::from_millis(9_500)))
    );

    assert!(apply_buffered_progress(
        &mut state,
        Some(DownloadProgress {
            generation,
            downloaded: 100,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        }),
        Some(landed),
    ));
    assert_eq!(
        state.buffered,
        Duration::from_secs(10),
        "a completed front download buffers the whole track"
    );

    assert!(apply_timeline_suffix_progress(
        &mut state,
        &TimelineSuffixState {
            pending: false,
            base: Duration::from_secs(9),
            written: 0,
            total: 100,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(10));
    assert_eq!(state.suffix_buffered, None);
}

#[test]
fn completed_download_size_requires_an_exact_completed_total() {
    assert_eq!(
        completed_download_size(DownloadProgress {
            generation: 1,
            downloaded: 42,
            total: Some(42),
            buffered_fraction: Some(1.0),
            fully_buffered: true,
        }),
        Some(42)
    );
    assert_eq!(
        completed_download_size(DownloadProgress {
            generation: 1,
            downloaded: 42,
            total: Some(43),
            buffered_fraction: Some(1.0),
            fully_buffered: true,
        }),
        None
    );
    assert_eq!(
        completed_download_size(DownloadProgress {
            generation: 1,
            downloaded: 42,
            total: Some(42),
            buffered_fraction: Some(1.0),
            fully_buffered: false,
        }),
        None
    );
}

#[test]
fn partial_initial_progress_does_not_overwrite_newer_callback_progress() {
    let mut progress = DownloadProgress {
        generation: 7,
        downloaded: 75,
        total: Some(100),
        buffered_fraction: None,
        fully_buffered: false,
    };
    progress.adopt_initial(DownloadProgress {
        generation: 7,
        downloaded: 25,
        total: Some(100),
        buffered_fraction: None,
        fully_buffered: false,
    });
    assert_eq!(progress.downloaded, 75);
    assert_eq!(progress.total, Some(100));
}

#[test]
fn stale_download_progress_cannot_update_a_new_generation() {
    let mut state = PlaybackState::default();
    let track = |id: &str| PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: id.into(),
        title: id.into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let first = state.replace(vec![track("first")], 0).unwrap();
    assert!(state.loaded_progressive(first, Some(Duration::from_secs(10))));
    assert!(apply_download_progress(
        &mut state,
        DownloadProgress {
            generation: first,
            downloaded: 50,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_secs(5));

    let second = state.replace(vec![track("second")], 0).unwrap();
    assert_ne!(first, second);
    assert_eq!(state.buffered, Duration::ZERO);
    assert!(!apply_download_progress(
        &mut state,
        DownloadProgress {
            generation: first,
            downloaded: 100,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::ZERO);
    assert!(apply_download_progress(
        &mut state,
        DownloadProgress {
            generation: second,
            downloaded: 25,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        },
    ));
    assert_eq!(state.buffered, Duration::from_millis(2500));
}

#[test]
fn consecutive_auto_skip_failure_limit_is_five() {
    assert_eq!(super::MAX_CONSECUTIVE_AUTO_SKIPS, 5);
}

#[test]
fn auto_skip_logic_bounds_consecutive_failures() {
    let mut state = PlaybackState::default();
    let tracks: Vec<PlaybackTrack> = (0..10)
        .map(|i| PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: format!("track-{i}"),
            title: format!("Track {i}"),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            ai_generated: false,
            service_url: String::new(),
        })
        .collect();
    let _gen = state.replace(tracks, 0).unwrap();
    assert_eq!(state.current_id(), Some("track-0"));

    let mut consecutive_failures = 0;
    let mut skipped_count = 0;

    for _ in 0..10 {
        if state.first_upcoming_index().is_some()
            && consecutive_failures < super::MAX_CONSECUTIVE_AUTO_SKIPS
        {
            consecutive_failures += 1;
            if state.next().is_some() {
                skipped_count += 1;
            }
        }
    }

    assert_eq!(consecutive_failures, 5);
    assert_eq!(skipped_count, 5);
    assert_eq!(state.current_id(), Some("track-5"));
}

#[test]
fn enqueue_actions_auto_play_when_playback_status_is_ended() {
    let mut state = PlaybackState::default();
    let track0 = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "track-0".into(),
        title: "Track 0".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let _gen = state.replace(vec![track0], 0).unwrap();
    state.status = PlaybackStatus::Ended;

    let track1 = PlaybackTrack {
        downloadable: false,
        progressive: false,
        provider: PlaybackProvider::Deezer,
        id: "track-1".into(),
        title: "Track 1".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };

    state.insert_queue_additions(vec![track1], true);
    assert_eq!(state.status, PlaybackStatus::Ended);
    assert_eq!(state.queue.len(), 2);
    let _next_gen = state.next().unwrap();
    assert_eq!(state.current_id(), Some("track-1"));
}
