use super::*;

fn tracks() -> Vec<PlaybackTrack> {
    (0..3)
        .map(|id| PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: id.to_string(),
            title: format!("Track {id}"),
            artist: "Artist".into(),
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
        .collect()
}

#[test]
fn output_source_reload_keeps_queue_navigation_and_lyrics_context() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.history.push(2);
    state.play_next.push(1);
    state.open_lyrics_context(&state.queue[0].clone());
    state.status = PlaybackStatus::Paused;
    state.position = Duration::from_secs(4);
    let history = state.history.clone();
    let play_next = state.play_next.clone();
    let lyrics = state.lyrics_context.clone();
    let queue_epoch = state.queue_epoch();
    let generation = state.generation;

    assert_eq!(state.reload_current_source(), Some(generation + 1));
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert_eq!(state.position, Duration::ZERO);
    assert_eq!(state.history, history);
    assert_eq!(state.play_next, play_next);
    assert_eq!(state.lyrics_context, lyrics);
    assert_eq!(state.queue_epoch(), queue_epoch);
}

fn fixed_shuffle_queue() -> Vec<PlaybackTrack> {
    let templates = tracks();
    (0..8)
        .map(|id| {
            let mut track = templates[id % templates.len()].clone();
            track.id = id.to_string();
            track.title = format!("Track {id}");
            track
        })
        .collect()
}

#[test]
fn shuffle_seed_normalization_is_nonzero_and_preserves_nonzero_values() {
    assert_eq!(normalize_shuffle_seed(0), SHUFFLE_SEED_FALLBACK);
    assert_eq!(normalize_shuffle_seed(1), 1);
    assert_eq!(normalize_shuffle_seed(u64::MAX), u64::MAX);
}

#[test]
fn shuffle_entropy_folding_uses_both_uuid_halves() {
    let zero = [0u8; 16];
    let mut low = zero;
    low[0] = 1;
    let mut high = zero;
    high[8] = 1;

    assert_eq!(fold_shuffle_entropy(&zero), SHUFFLE_SEED_FALLBACK);
    assert_ne!(fold_shuffle_entropy(&low), fold_shuffle_entropy(&high));
    assert_ne!(fold_shuffle_entropy(&low), 0);
    assert_ne!(fold_shuffle_entropy(&high), 0);
}

#[test]
fn fixed_shuffle_seeds_produce_distinct_complete_orders_with_current_pinned() {
    let queue = fixed_shuffle_queue();

    let mut first = PlaybackState {
        shuffle_seed: 0x0123_4567_89AB_CDEF,
        ..PlaybackState::default()
    };
    let _ = first.replace(queue.clone(), 3);
    first.set_shuffle_enabled(true);
    let first_order = first.upcoming_indices();
    assert_eq!(first_order, vec![5, 0, 2, 6, 4, 1, 7]);
    assert!(!first_order.contains(&3));
    assert_eq!(
        first_order.iter().copied().collect::<HashSet<_>>(),
        (0..8).filter(|index| *index != 3).collect::<HashSet<_>>()
    );

    let mut second = PlaybackState {
        shuffle_seed: 0xFEDC_BA98_7654_3211,
        ..PlaybackState::default()
    };
    let _ = second.replace(queue, 3);
    second.set_shuffle_enabled(true);
    let second_order = second.upcoming_indices();
    assert_eq!(second_order, vec![6, 7, 2, 1, 0, 4, 5]);
    assert!(!second_order.contains(&3));
    assert_ne!(first_order, second_order);
}

#[test]
fn source_conversions_preserve_all_artist_refs_and_ids() {
    let artists = vec![
        crate::search::TrackArtistRef {
            id: "11".into(),
            name: "Primary".into(),
        },
        crate::search::TrackArtistRef {
            id: "22".into(),
            name: "Collaborator".into(),
        },
    ];
    let search_track = crate::search::Track {
        artists: artists.clone(),
        artist: "Primary, Collaborator".into(),
        ai_generated: true,
        ..crate::search::Track::default()
    };
    let library_track = crate::library::Track {
        artists: artists.clone(),
        artist: "Primary, Collaborator".into(),
        ..crate::library::Track::default()
    };

    assert_eq!(PlaybackTrack::from_search(&search_track).artists, artists);
    assert!(PlaybackTrack::from_search(&search_track).ai_generated);
    assert_eq!(
        PlaybackTrack::from_library(&library_track, crate::search::Provider::Deezer).artists,
        artists
    );
}

#[test]
fn deezer_ai_content_updates_matching_queue_albums_only() {
    let mut state = PlaybackState {
        queue: tracks(),
        ..PlaybackState::default()
    };
    state.queue[0].provider = PlaybackProvider::Deezer;
    state.queue[0].album_id = "10".into();
    state.queue[1].provider = PlaybackProvider::SoundCloud;
    state.queue[1].album_id = "10".into();

    assert!(state.apply_deezer_ai_content(&std::collections::HashMap::from([("10".into(), true)])));
    assert!(state.queue[0].ai_generated);
    assert!(!state.queue[1].ai_generated);
}

fn explicit_tracks(ids: &[usize]) -> Vec<PlaybackTrack> {
    tracks()
        .into_iter()
        .enumerate()
        .map(|(index, mut track)| {
            track.explicit = ids.contains(&index);
            track
        })
        .collect()
}

#[test]
fn playback_preferences_restore_volume_mute_repeat_and_shuffle() {
    let mut state = PlaybackState::default();

    state.restore_volume(0.35, true);
    state.set_repeat_mode(RepeatMode::One);
    state.set_shuffle_enabled(true);

    assert_eq!((state.volume, state.muted), (0.0, true));
    assert_eq!(state.persisted_volume(), 0.35);
    assert_eq!(state.repeat_mode, RepeatMode::One);
    assert!(state.shuffle_enabled);
    assert_eq!(state.toggle_mute(), 0.35);
}

#[test]
fn volume_icons_match_original_thresholds_and_mute_restores_volume() {
    let mut state = PlaybackState::default();
    state.set_volume(0.0);
    assert_eq!(state.volume_icon_level(), VolumeIconLevel::Muted);
    state.set_volume(0.33);
    assert_eq!(state.volume_icon_level(), VolumeIconLevel::Off);
    state.set_volume(0.66);
    assert_eq!(state.volume_icon_level(), VolumeIconLevel::Low);
    state.set_volume(0.67);
    assert_eq!(state.volume_icon_level(), VolumeIconLevel::High);

    assert_eq!(state.toggle_mute(), 0.0);
    assert_eq!(state.toggle_mute(), 0.67);
}

#[test]
fn sidebar_restores_preference_and_persists_toggle_transitions() {
    let mut state = PlaybackState::default();
    state.restore_sidebar(true, RightSidebar::Lyrics);
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Lyrics));

    state.replace(tracks(), 0);
    assert_eq!(state.right_sidebar, RightSidebar::Lyrics);
    state.toggle_sidebar(RightSidebar::Lyrics);
    assert_eq!(state.sidebar_preferences(), (false, RightSidebar::Lyrics));
    state.toggle_sidebar(RightSidebar::Queue);
    assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
    state.clear();
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
}

#[test]
fn closing_the_player_keeps_the_standing_sidebar_preference() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.toggle_sidebar(RightSidebar::Queue);
    assert_eq!(state.right_sidebar, RightSidebar::Queue);

    // closePlayer hides the sidebar without persisting the close; the
    // preference stands and the next track reopens the panel.
    state.clear();
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
    state.replace(tracks(), 0);
    assert_eq!(state.right_sidebar, RightSidebar::Queue);

    // An explicit toggle-close is a real preference change and stays closed.
    state.toggle_sidebar(RightSidebar::Queue);
    state.clear();
    state.replace(tracks(), 0);
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(state.sidebar_preferences(), (false, RightSidebar::Queue));
}

#[test]
fn replacement_loading_and_stale_completion_are_ordered() {
    let mut state = PlaybackState::default();
    let first = state.replace(tracks(), 1).unwrap();
    let second = state.select(2).unwrap();
    assert!(!state.loaded_progressive(first, None));
    assert!(!state.loaded(first, None));
    assert!(state.loaded(second, Some(Duration::from_secs(12))));
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.duration, Duration::from_secs(12));
}

#[test]
fn pending_load_opens_the_bar_blank_and_closes_without_a_track() {
    let mut state = PlaybackState::default();
    assert_eq!(state.status, PlaybackStatus::Empty);
    assert!(!state.player_bar_open());

    assert!(state.begin_pending_load());
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert_eq!(state.current_index, None);
    assert!(state.player_bar_open());
    let first_epoch = state.queue_epoch();

    // Queue loading owns visibility independently of the transport. A
    // transport lifecycle update cannot hide the pending player.
    state.status = PlaybackStatus::Empty;
    assert!(state.player_bar_open());

    // A newer pending source owns a distinct epoch, so an older request
    // cannot close or populate its player bar.
    assert!(state.begin_pending_load());
    assert_ne!(state.queue_epoch(), first_epoch);

    let pending_epoch = state.queue_epoch();
    assert!(state.abandon_pending_load());
    assert_eq!(state.status, PlaybackStatus::Empty);
    assert_ne!(state.queue_epoch(), pending_epoch);

    // Once a real load selected a track, abandon no longer applies.
    state.begin_pending_load();
    state.replace(tracks(), 0);
    assert_eq!(state.current_index, Some(0));
    assert!(!state.abandon_pending_load());
    assert_eq!(state.status, PlaybackStatus::Loading);
}

#[test]
fn explicit_play_restarts_a_mix_that_is_already_the_active_context() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 1);
    state.replace_context(PlaybackContext::DeezerFlow {
        config_id: "mix-1".into(),
        mode: library::FlowMode::Default,
        tuner: None,
        kind: DeezerFlowKind::SmartMix,
    });

    // Navigating into the already-playing mix keeps the current track.
    assert!(state.should_preserve_flow_current("mix-1", true, true));
    // An explicit play command restarts from the first track.
    assert!(!state.should_preserve_flow_current("mix-1", true, false));
    // A different mix, or a flow that appends, never preserves.
    assert!(!state.should_preserve_flow_current("mix-2", true, true));
    assert!(!state.should_preserve_flow_current("mix-1", false, true));

    // The restart path replaces the queue and selects the first track.
    state.replace(tracks(), 0);
    assert_eq!(state.current_index, Some(0));
}

#[test]
fn pending_load_clears_a_playing_queue_so_the_bar_goes_blank() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 1);
    state.loaded(state.generation, None);
    assert_eq!(state.status, PlaybackStatus::Playing);
    assert_eq!(state.current_index, Some(1));

    assert!(state.begin_pending_load());
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert_eq!(state.current_index, None);
    assert!(state.queue.is_empty());
    assert!(matches!(state.context, PlaybackContext::None));

    // The real load that follows supersedes the pending state cleanly.
    state.replace(tracks(), 0);
    assert_eq!(state.current_index, Some(0));
}

#[test]
fn pending_load_keeps_the_sidebar_closed_until_tracks_exist() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.toggle_sidebar(RightSidebar::Queue);
    assert_eq!(state.right_sidebar, RightSidebar::Queue);

    state.begin_pending_load();
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(state.current(), None);
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Lyrics),
        RightSidebar::Closed
    );
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Queue),
        RightSidebar::Closed
    );

    state.replace(tracks(), 0);
    assert_eq!(state.right_sidebar, RightSidebar::Queue);
}

#[test]
fn previous_restarts_after_three_seconds_then_moves_back() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 1);
    state.loaded(state.generation, None);
    state.seek(Duration::from_secs(4));
    assert!(matches!(state.previous(), PreviousAction::Restart));
    assert_eq!(state.current_index, Some(1));
    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(0));
}

#[test]
fn repeated_previous_walks_back_through_history_without_ping_ponging() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    assert!(state.next().is_some());
    assert!(state.next().is_some());
    assert_eq!(state.current_index, Some(2));
    assert_eq!(state.history, vec![0, 1]);

    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(1));
    assert_eq!(state.history, vec![0]);

    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(0));
    assert!(state.history.is_empty());

    assert!(matches!(state.previous(), PreviousAction::Restart));
    assert_eq!(state.current_index, Some(0));
}

#[test]
fn next_and_end_never_wrap_a_finite_queue() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 1);
    assert!(state.next().is_some());
    assert!(state.next().is_none());
    assert_eq!(state.status, PlaybackStatus::Ended);
    assert_eq!(state.current_index, Some(2));
}

#[test]
fn duplicate_queue_rows_keep_sequential_navigation_occurrence_safe() {
    let mut duplicate = tracks()[0].clone();
    duplicate.title = "Second occurrence".into();
    let queue = vec![tracks()[0].clone(), duplicate, tracks()[1].clone()];
    let mut state = PlaybackState::default();
    state.replace(queue, 0);

    assert_eq!(state.upcoming_indices(), vec![1, 2]);
    state.next();
    assert_eq!(state.current_index, Some(1));
    assert_eq!(state.current().unwrap().title, "Second occurrence");
    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(0));

    state.next();
    assert_eq!(state.current_index, Some(1));
    state.next();
    assert_eq!(state.current_index, Some(2));
    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(1));
}

#[test]
fn duplicate_queue_rows_remain_distinct_in_shuffle_and_direct_selection() {
    let mut duplicate = tracks()[0].clone();
    duplicate.title = "Second occurrence".into();
    let mut state = PlaybackState::default();
    state.replace(vec![tracks()[0].clone(), duplicate, tracks()[1].clone()], 2);
    state.set_shuffle_enabled(true);

    let upcoming = state.upcoming_indices();
    assert_eq!(upcoming.len(), 2);
    assert!(upcoming.contains(&0));
    assert!(upcoming.contains(&1));
    assert!(state.select(1).is_some());
    assert_eq!(state.current_index, Some(1));
    assert_eq!(state.current().unwrap().title, "Second occurrence");
}

#[test]
fn content_blocked_covers_explicit_and_ai_independently() {
    let mut state = PlaybackState::default();
    let queue = explicit_tracks(&[1]);
    let mut ai_track = tracks()[0].clone();
    ai_track.ai_generated = true;

    assert!(!state.content_blocked(&queue[1]));
    assert!(!state.content_blocked(&ai_track));

    state.skip_explicit = true;
    assert!(state.content_blocked(&queue[1]));
    assert!(!state.content_blocked(&ai_track));

    state.block_ai = true;
    assert!(state.content_blocked(&queue[1]));
    assert!(state.content_blocked(&ai_track));
    assert!(!state.content_blocked(&tracks()[0]));

    state.skip_explicit = false;
    assert!(!state.content_blocked(&queue[1]));
    assert!(state.content_blocked(&ai_track));
}

fn set_block_ai_reports_blocked_current_and_defaults_off() {
    let mut state = PlaybackState::default();
    let mut ai_track = tracks()[0].clone();
    ai_track.ai_generated = true;
    state.replace(vec![ai_track], 0);

    assert!(!state.block_ai);
    assert!(!state.set_block_ai(false));
    assert!(state.set_block_ai(true));
    assert!(state.block_ai);
    assert!(state.set_block_ai(true));
    assert!(!state.set_block_ai(false));
}

#[test]
fn select_refuses_blocked_tracks_without_changing_generation() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1]), 0);
    state.set_skip_explicit(true);
    let generation = state.generation;
    assert!(state.select(1).is_none());
    assert_eq!(state.current_index, Some(0));
    assert_eq!(state.generation, generation);
    assert!(state.select(2).is_some());
}

#[test]
fn next_skips_blocked_tracks_sequentially_and_ends_like_exhaustion() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1, 2]), 0);
    state.set_skip_explicit(true);
    assert_eq!(state.current_id(), Some("0"));
    assert!(state.next().is_none());
    assert_eq!(state.status, PlaybackStatus::Ended);
    assert_eq!(state.current_index, Some(0));

    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1]), 0);
    state.set_skip_explicit(true);
    assert_eq!(state.next().map(|_| ()), Some(()));
    assert_eq!(state.current_id(), Some("2"));
    assert!(state.next().is_none());
    assert_eq!(state.status, PlaybackStatus::Ended);
}

#[test]
fn repeat_all_next_wraps_to_the_first_non_blocked_track() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[0, 1]), 2);
    state.set_skip_explicit(true);
    state.set_repeat_mode(RepeatMode::All);
    assert!(state.next().is_some());
    assert_eq!(state.current_id(), Some("2"));
}

#[test]
fn shuffle_next_skips_blocked_tracks_and_repeat_all_redeals() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1, 2]), 0);
    state.set_skip_explicit(true);
    state.set_shuffle_enabled(true);
    assert!(state.next().is_none());
    assert_eq!(state.status, PlaybackStatus::Ended);

    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1]), 0);
    state.set_skip_explicit(true);
    state.set_shuffle_enabled(true);
    state.set_repeat_mode(RepeatMode::All);
    for _ in 0..5 {
        assert!(state.next().is_some());
    }
}

#[test]
fn replace_picks_the_first_playable_track_in_queue_order() {
    let mut state = PlaybackState::default();
    state.set_skip_explicit(true);
    let generation = state.replace(explicit_tracks(&[1]), 1).unwrap();
    assert_eq!(state.current_index, Some(0));
    assert!(state.loaded(generation, None));
}

#[test]
fn replace_clears_when_every_track_is_blocked() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.set_skip_explicit(true);
    assert!(state.replace(explicit_tracks(&[0, 1, 2]), 1).is_none());
    assert_eq!(state.status, PlaybackStatus::Empty);
    assert!(state.queue.is_empty());
    assert_eq!(state.current_index, None);
}

#[test]
fn toggling_skip_explicit_reports_a_blocked_current_track() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1]), 0);
    assert!(!state.set_skip_explicit(true));
    assert!(state.select(1).is_none());

    assert!(!state.set_skip_explicit(false));
    state.replace(explicit_tracks(&[1, 2]), 1);
    assert!(state.set_skip_explicit(true));
}

#[test]
fn repeat_one_moves_past_an_explicit_current_track_when_skipping_is_enabled() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[0]), 0);
    state.set_repeat_mode(RepeatMode::One);
    assert!(state.set_skip_explicit(true));

    let generation = state.generation;
    assert!(state.next().is_some());
    assert_ne!(state.generation, generation);
    assert_eq!(state.current_id(), Some("1"));
    assert!(!state.current().is_some_and(|track| track.explicit));
}

#[test]
fn previous_skips_blocked_history_entries_and_walks_past_blocked_tracks() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[1]), 2);
    state.set_skip_explicit(true);
    state.loaded(state.generation, None);
    state.seek(Duration::from_secs(4));
    assert!(matches!(state.previous(), PreviousAction::Restart));
    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(0));
}

#[test]
fn previous_with_repeat_all_wraps_once_past_blocked_tracks() {
    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[0, 2]), 1);
    state.set_skip_explicit(true);
    state.set_repeat_mode(RepeatMode::All);
    assert!(matches!(state.previous(), PreviousAction::Load(_)));
    assert_eq!(state.current_index, Some(1));

    let mut state = PlaybackState::default();
    state.replace(explicit_tracks(&[0, 1, 2]), 2);
    state.set_skip_explicit(true);
    state.set_repeat_mode(RepeatMode::All);
    assert!(matches!(state.previous(), PreviousAction::Restart));
    assert_eq!(state.current_index, Some(2));
}

#[test]
fn removal_preserves_current_track_and_adjusts_its_index() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 1);
    assert!(!state.remove(1));
    assert!(state.remove(0));
    assert_eq!(state.current_index, Some(0));
    assert_eq!(state.current().unwrap().id, "1");
}

#[test]
fn repeated_play_next_is_last_action_first_without_duplicates() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);

    assert!(state.enqueue_next(2));
    assert!(state.enqueue_next(1));
    assert_eq!(state.upcoming_indices(), vec![1, 2]);
    assert!(state.enqueue_next(2));
    assert_eq!(state.upcoming_indices(), vec![2, 1]);

    state.next();
    assert_eq!(state.current_id(), Some("2"));
    state.next();
    assert_eq!(state.current_id(), Some("1"));
}

#[test]
fn play_last_moves_an_existing_track_to_the_end() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);

    assert!(state.enqueue_last(1));
    assert_eq!(state.upcoming_indices(), vec![2, 1]);
    assert_eq!(
        state.queue.iter().filter(|track| track.id == "1").count(),
        1
    );
    assert!(!state.enqueue_last(0));
}

#[test]
fn shuffled_play_next_registers_new_duplicate_occurrences_in_source_order() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.set_shuffle_enabled(true);
    let previous_upcoming = state.upcoming_indices();
    let previous_epoch = state.queue_epoch();

    let mut current_duplicate = state.queue[0].clone();
    current_duplicate.title = "Current duplicate".into();
    let mut second_duplicate = state.queue[1].clone();
    second_duplicate.title = "Second duplicate".into();
    assert_eq!(
        state.insert_queue_additions(vec![current_duplicate, second_duplicate], false),
        2
    );

    let upcoming = state.upcoming_indices();
    assert_eq!(&upcoming[..2], &[3, 4]);
    assert_eq!(&upcoming[2..], previous_upcoming);
    assert_eq!(state.current_index, Some(0));
    assert_ne!(state.queue_epoch(), previous_epoch);

    assert!(state.next().is_some());
    assert_eq!(state.current_index, Some(3));
    assert_eq!(state.current().unwrap().title, "Current duplicate");
    assert!(state.next().is_some());
    assert_eq!(state.current_index, Some(4));
    assert_eq!(state.current().unwrap().title, "Second duplicate");
    for expected in previous_upcoming {
        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(expected));
    }
    assert!(state.next().is_none());
}

#[test]
fn shuffled_play_last_runs_after_the_existing_deck_and_survives_repeat_all() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.set_shuffle_enabled(true);
    state.set_repeat_mode(RepeatMode::All);
    let previous_upcoming = state.upcoming_indices();

    let mut duplicate = state.queue[1].clone();
    duplicate.title = "Duplicate occurrence".into();
    let mut another = state.queue[2].clone();
    another.title = "Another occurrence".into();
    assert_eq!(
        state.insert_queue_additions(vec![duplicate, another], true),
        2
    );

    let upcoming = state.upcoming_indices();
    assert_eq!(&upcoming[..previous_upcoming.len()], previous_upcoming);
    let added_order = upcoming[previous_upcoming.len()..].to_vec();
    assert_eq!(
        added_order.iter().copied().collect::<HashSet<_>>(),
        HashSet::from([3, 4])
    );

    for expected in previous_upcoming {
        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(expected));
    }
    for expected in added_order {
        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(expected));
    }
    assert!(state.next().is_some());
}

#[test]
fn shuffled_existing_queue_actions_move_physical_occurrences_only() {
    let mut duplicate = tracks()[1].clone();
    duplicate.title = "Duplicate occurrence".into();
    let mut state = PlaybackState::default();
    state.replace(
        vec![
            tracks()[0].clone(),
            tracks()[1].clone(),
            duplicate,
            tracks()[2].clone(),
        ],
        0,
    );
    state.set_shuffle_enabled(true);

    assert!(state.enqueue_next(2));
    assert_eq!(state.upcoming_indices().first(), Some(&2));
    assert_eq!(state.current_index, Some(0));
    assert!(state.enqueue_last(2));
    assert_eq!(state.upcoming_indices().last(), Some(&2));
    assert_eq!(state.queue[1].title, "Track 1");
    assert_eq!(state.queue[2].title, "Duplicate occurrence");

    assert!(state.remove(1));
    assert_eq!(state.upcoming_indices().last(), Some(&1));
    assert_eq!(state.queue[1].title, "Duplicate occurrence");
}

#[test]
fn reorder_preserves_current_and_controls_the_normal_future_order() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);

    assert!(state.reorder(1, 0));
    assert_eq!(state.current_id(), Some("0"));
    assert_eq!(state.upcoming_indices(), vec![2, 1]);
    assert!(!state.reorder(0, 0));
    state.next();
    assert_eq!(state.current_id(), Some("2"));
    state.next();
    assert_eq!(state.current_id(), Some("1"));
}

#[test]
fn reorder_controls_shuffle_and_repeat_all_future_order() {
    let mut shuffled = PlaybackState::default();
    shuffled.replace(tracks(), 0);
    shuffled.set_shuffle_enabled(true);
    assert!(shuffled.reorder(1, 0));
    let expected = shuffled.upcoming_indices();
    shuffled.next();
    assert_eq!(shuffled.current_index, Some(expected[0]));

    let mut repeated = PlaybackState::default();
    repeated.replace(tracks(), 1);
    repeated.set_repeat_mode(RepeatMode::All);
    assert_eq!(repeated.upcoming_indices(), vec![2, 0]);
    assert!(repeated.reorder(1, 0));
    assert_eq!(repeated.current_id(), Some("1"));
    assert_eq!(repeated.upcoming_indices(), vec![0, 2]);
    repeated.next();
    assert_eq!(repeated.current_id(), Some("0"));
}

#[test]
fn buffered_fraction_clamps_and_guards_zero_duration() {
    let mut state = PlaybackState::default();
    state.set_buffered_fraction(0.75);
    assert_eq!(state.buffered, Duration::ZERO);
    let generation = state.replace(tracks(), 0).unwrap();
    assert!(state.loaded(generation, Some(Duration::from_secs(10))));
    state.set_buffered_fraction(0.5);
    assert_eq!(state.buffered, Duration::from_secs(5));
    state.set_buffered_fraction(2.0);
    assert_eq!(state.buffered, Duration::from_secs(10));
    state.set_buffered_fraction(-1.0);
    assert_eq!(state.buffered, Duration::ZERO);
}

#[test]
fn progressive_load_preserves_partial_buffer_and_full_load_fills_it() {
    let mut state = PlaybackState::default();
    let generation = state.replace(tracks(), 0).unwrap();
    state.set_buffered_fraction(0.25);
    assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
    assert_eq!(state.buffered, Duration::from_secs(5));

    let mut full = PlaybackState::default();
    let generation = full.replace(tracks(), 0).unwrap();
    assert!(full.loaded_fully_buffered(generation, Some(Duration::from_secs(20))));
    assert_eq!(full.buffered, Duration::from_secs(20));
}

#[test]
fn stopping_a_loading_track_leaves_a_stable_retryable_state() {
    let mut state = PlaybackState::default();
    let generation = state.replace(tracks(), 0).unwrap();
    assert_eq!(state.status, PlaybackStatus::Loading);
    assert!(state.stop_loading());
    assert_eq!(state.status, PlaybackStatus::Ended);
    assert_eq!(state.position, Duration::ZERO);
    assert_eq!(state.buffered, Duration::ZERO);
    assert!(state.error.is_none());
    assert!(!state.stop_loading());
    assert_ne!(state.generation, generation);
}

#[test]
fn seek_volume_and_mute_are_clamped_and_restored() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    assert_eq!(state.seek(Duration::from_secs(20)), Duration::from_secs(10));
    assert_eq!(state.set_volume(2.0), 1.0);
    assert_eq!(state.toggle_mute(), 0.0);
    assert_eq!(state.toggle_mute(), 1.0);
    assert_eq!(state.set_volume(f32::NAN), 0.8);
}

#[test]
fn clear_invalidates_pending_selection() {
    let mut state = PlaybackState::default();
    let generation = state.replace(tracks(), 0).unwrap();
    state.clear();
    assert!(!state.loaded(generation, None));
    assert_eq!(state.status, PlaybackStatus::Empty);
}

#[test]
fn lyrics_are_closed_by_default_and_toggle_only_with_a_track() {
    let mut state = PlaybackState::default();
    assert_eq!(state.right_sidebar, RightSidebar::Closed);
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Lyrics),
        RightSidebar::Closed
    );
    state.replace(tracks(), 0);
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Lyrics),
        RightSidebar::Lyrics
    );
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Queue),
        RightSidebar::Queue
    );
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Queue),
        RightSidebar::Closed
    );
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Lyrics),
        RightSidebar::Lyrics
    );
    assert_eq!(
        state.toggle_sidebar(RightSidebar::Lyrics),
        RightSidebar::Closed
    );
}

#[test]
fn context_lyrics_display_a_non_playing_track_without_touching_playback() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    let context = PlaybackTrack {
        id: "99".into(),
        ..tracks().into_iter().next().unwrap()
    };

    state.open_lyrics_context(&context);
    assert_eq!(state.right_sidebar, RightSidebar::Lyrics);
    assert_eq!(state.lyrics_display_track(), Some(&context));
    assert!(!state.lyrics_follows_playback());
    assert_eq!(state.current_id(), Some("0"));

    // A selection change hands the panel back to the playing track.
    state.select(1);
    assert_eq!(
        state.lyrics_display_track().map(|t| t.id.as_str()),
        Some("1")
    );
    assert!(state.lyrics_follows_playback());

    // Requesting lyrics for the playing track keeps following playback.
    state.open_lyrics_context(&tracks()[1]);
    assert_eq!(
        state.lyrics_display_track().map(|t| t.id.as_str()),
        Some("1")
    );
    assert!(state.lyrics_follows_playback());

    // Closing or switching the sidebar drops the context track.
    state.open_lyrics_context(&context);
    state.toggle_sidebar(RightSidebar::Queue);
    assert!(state.lyrics_follows_playback());
}

#[test]
fn first_upcoming_index_matches_first_element_of_upcoming_indices() {
    let mut state = PlaybackState::default();
    assert_eq!(state.first_upcoming_index(), None);

    state.replace(tracks(), 0);
    assert_eq!(
        state.first_upcoming_index(),
        state.upcoming_indices().first().copied()
    );
    assert_eq!(state.first_upcoming_index(), Some(1));

    state.enqueue_next(2);
    assert_eq!(
        state.first_upcoming_index(),
        state.upcoming_indices().first().copied()
    );
    assert_eq!(state.first_upcoming_index(), Some(2));

    state.set_shuffle_enabled(true);
    assert_eq!(
        state.first_upcoming_index(),
        state.upcoming_indices().first().copied()
    );

    let mut repeat_state = PlaybackState::default();
    repeat_state.replace(tracks(), 2);
    assert_eq!(repeat_state.first_upcoming_index(), None);
    repeat_state.set_repeat_mode(RepeatMode::All);
    assert_eq!(repeat_state.first_upcoming_index(), Some(0));
    assert_eq!(
        repeat_state.first_upcoming_index(),
        repeat_state.upcoming_indices().first().copied()
    );
}

#[test]
fn contexts_are_infinite_but_flow_requires_a_returned_tuner() {
    let flow_without_tuner = PlaybackContext::DeezerFlow {
        config_id: "flow".into(),
        mode: library::FlowMode::Discovery,
        tuner: None,
        kind: DeezerFlowKind::Flow,
    };
    assert!(flow_without_tuner.is_infinite());
    assert!(!flow_without_tuner.can_extend());
    let smart_mix = PlaybackContext::DeezerFlow {
        config_id: "inspired-by-3".into(),
        mode: library::FlowMode::Default,
        tuner: Some(library::FlowTuner::initial(library::FlowMode::Default)),
        kind: DeezerFlowKind::SmartMix,
    };
    assert!(!smart_mix.is_infinite());
    assert!(!smart_mix.can_extend());
    assert!(
        PlaybackContext::DeezerTrackMix {
            seed_track_id: "1".into()
        }
        .can_extend()
    );
    assert!(
        PlaybackContext::DeezerArtistMix {
            seed_artist_id: "2".into()
        }
        .is_infinite()
    );
    let library = PlaybackContext::DeezerLibraryTracks { load_id: 7 };
    assert!(!library.is_infinite());
    assert!(!library.can_extend());
}

#[test]
fn exact_library_tail_preserves_duplicates_order_and_current_audio_state() {
    let mut state = PlaybackState::default();
    state.replace(vec![tracks()[0].clone(), tracks()[1].clone()], 1);
    state.loaded(state.generation, Some(Duration::from_secs(20)));
    state.position = Duration::from_secs(7);
    state.buffered = Duration::from_secs(13);
    state.history.push(0);
    state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 41 });
    let ticket = state.deezer_library_append_ticket(41).unwrap();
    let generation = state.generation;
    let status = state.status;
    let index = state.current_index;
    let position = state.position;
    let duration = state.duration;
    let buffered = state.buffered;
    let history = state.history.clone();
    let mut enriched_prefix = tracks()[0].clone();
    enriched_prefix.title = "metadata changed".into();
    let duplicate = tracks()[1].clone();
    let tail = tracks()[2].clone();

    assert_eq!(
        state.append_deezer_library_exact_tail(
            &ticket,
            vec![enriched_prefix, tracks()[1].clone(), duplicate, tail],
        ),
        ExactQueueAppend::Applied {
            added: 2,
            resume_index: None,
        }
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["0", "1", "1", "2"]
    );
    assert_eq!(state.queue[0].title, tracks()[0].title);
    assert_eq!(state.generation, generation);
    assert_eq!(state.status, status);
    assert_eq!(state.current_index, index);
    assert_eq!(state.position, position);
    assert_eq!(state.duration, duration);
    assert_eq!(state.buffered, buffered);
    assert_eq!(state.history, history);
}

#[test]
fn exact_library_tail_rejects_stale_context_epoch_length_and_prefix() {
    let partial = vec![tracks()[0].clone(), tracks()[1].clone()];
    let full = tracks();

    let mut replaced = PlaybackState::default();
    replaced.replace(partial.clone(), 0);
    replaced.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 1 });
    assert!(replaced.deezer_library_append_ticket(99).is_none());
    let replacement_ticket = replaced.deezer_library_append_ticket(1).unwrap();
    replaced.replace(vec![tracks()[2].clone()], 0);
    assert_eq!(
        replaced.append_deezer_library_exact_tail(&replacement_ticket, full.clone()),
        ExactQueueAppend::Stale
    );

    let mut reordered = PlaybackState::default();
    let mut reordered_full = full.clone();
    reordered_full.push(PlaybackTrack {
        id: "3".into(),
        ..tracks()[2].clone()
    });
    reordered.replace(full.clone(), 0);
    reordered.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 2 });
    let reorder_ticket = reordered.deezer_library_append_ticket(2).unwrap();
    assert!(reordered.reorder(0, 1));
    assert_eq!(
        reordered.append_deezer_library_exact_tail(&reorder_ticket, reordered_full),
        ExactQueueAppend::Stale
    );

    let mut wrong_context = PlaybackState::default();
    wrong_context.replace(partial.clone(), 0);
    wrong_context.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 3 });
    let context_ticket = wrong_context.deezer_library_append_ticket(3).unwrap();
    wrong_context.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 4 });
    assert_eq!(
        wrong_context.append_deezer_library_exact_tail(&context_ticket, full.clone()),
        ExactQueueAppend::Stale
    );

    let mut changed_length = PlaybackState::default();
    changed_length.replace(partial.clone(), 0);
    changed_length.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 5 });
    let length_ticket = changed_length.deezer_library_append_ticket(5).unwrap();
    changed_length.queue.push(tracks()[2].clone());
    assert_eq!(
        changed_length.append_deezer_library_exact_tail(&length_ticket, full.clone()),
        ExactQueueAppend::Stale
    );

    let mut wrong_prefix = PlaybackState::default();
    wrong_prefix.replace(partial.clone(), 0);
    wrong_prefix.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 6 });
    let prefix_ticket = wrong_prefix.deezer_library_append_ticket(6).unwrap();
    assert_eq!(
        wrong_prefix.append_deezer_library_exact_tail(
            &prefix_ticket,
            vec![
                tracks()[1].clone(),
                tracks()[0].clone(),
                tracks()[2].clone()
            ],
        ),
        ExactQueueAppend::Stale
    );

    let mut replayed = PlaybackState::default();
    replayed.replace(partial, 0);
    replayed.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 7 });
    let replay_ticket = replayed.deezer_library_append_ticket(7).unwrap();
    assert_eq!(
        replayed.append_deezer_library_exact_tail(&replay_ticket, full.clone()),
        ExactQueueAppend::Applied {
            added: 1,
            resume_index: None,
        }
    );
    assert_eq!(
        replayed.append_deezer_library_exact_tail(&replay_ticket, full),
        ExactQueueAppend::Stale
    );
}

#[test]
fn exact_library_tail_reports_when_an_ended_tail_can_resume() {
    let mut state = PlaybackState::default();
    state.replace(vec![tracks()[0].clone()], 0);
    state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 8 });
    assert!(state.next().is_none());
    let ticket = state.deezer_library_append_ticket(8).unwrap();
    let generation = state.generation;
    assert_eq!(
        state.append_deezer_library_exact_tail(
            &ticket,
            vec![
                tracks()[0].clone(),
                tracks()[0].clone(),
                tracks()[2].clone()
            ],
        ),
        ExactQueueAppend::Applied {
            added: 2,
            resume_index: Some(1),
        }
    );
    assert_eq!(state.generation, generation);
    assert_eq!(state.status, PlaybackStatus::Ended);
    assert!(state.select(1).is_some());
    assert_eq!(state.current_index, Some(1));
}

#[test]
fn exact_library_tail_joins_the_remaining_shuffle_deck() {
    let make_track = |index: usize| PlaybackTrack {
        id: index.to_string(),
        title: format!("Track {index}"),
        ..tracks()[0].clone()
    };
    let full: Vec<_> = (0..24).map(make_track).collect();
    let mut state = PlaybackState::default();
    state.replace(full[..3].to_vec(), 0);
    state.shuffle_seed = SHUFFLE_SEED_FALLBACK;
    state.set_shuffle_enabled(true);
    state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 9 });
    state.play_next.push(1);
    let ticket = state.deezer_library_append_ticket(9).unwrap();
    let existing_upcoming = state.upcoming_indices();
    assert_eq!(existing_upcoming, vec![1, 2]);

    assert_eq!(
        state.append_deezer_library_exact_tail(&ticket, full.clone()),
        ExactQueueAppend::Applied {
            added: 21,
            resume_index: None,
        }
    );
    let upcoming = state.upcoming_indices();
    let expected_tail = vec![
        8, 7, 17, 20, 9, 12, 11, 21, 4, 23, 10, 13, 18, 22, 15, 5, 14, 3, 6, 16, 19,
    ];
    assert_eq!(
        &upcoming[..existing_upcoming.len()],
        existing_upcoming.as_slice()
    );
    assert_eq!(
        &upcoming[existing_upcoming.len()..],
        expected_tail.as_slice()
    );
    assert!(!upcoming.contains(&state.current_index.unwrap()));
    assert_eq!(upcoming.len(), full.len() - 1);
    assert_eq!(
        upcoming.iter().copied().collect::<HashSet<_>>(),
        (1..full.len()).collect::<HashSet<_>>()
    );
}

#[test]
fn stale_extension_ticket_cannot_touch_a_replaced_queue() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.replace_context(PlaybackContext::DeezerTrackMix {
        seed_track_id: "0".into(),
    });
    let ticket = state.extension_ticket().unwrap();
    state.replace(vec![tracks()[2].clone()], 0);
    assert_eq!(
        state.apply_extension(&ticket, vec![tracks()[1].clone()], None, None),
        ExtensionApply::Stale
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["2"]
    );
}

#[test]
fn replace_remaining_tracks_preserves_history_and_current_without_audio_reset() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 2);
    state.select(0);
    state.loaded(state.generation, Some(Duration::from_secs(10)));
    state.position = Duration::from_secs(3);
    state.buffered = Duration::from_secs(5);
    state.context = PlaybackContext::DeezerFlow {
        config_id: "flow".into(),
        mode: library::FlowMode::Default,
        tuner: Some(library::FlowTuner::initial(library::FlowMode::Default)),
        kind: DeezerFlowKind::Flow,
    };
    let generation = state.generation;
    let status = state.status;
    let position = state.position;
    let duration = state.duration;
    let buffered = state.buffered;
    assert_eq!(
        state.replace_remaining_tracks(vec![
            tracks()[1].clone(),
            PlaybackTrack {
                id: "new".into(),
                ..tracks()[1].clone()
            }
        ]),
        2
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["2", "0", "1", "new"]
    );
    assert_eq!(state.current_index, Some(1));
    assert_eq!(state.history, vec![0]);
    assert_eq!(state.generation, generation);
    assert_eq!(state.status, status);
    assert_eq!(state.position, position);
    assert_eq!(state.duration, duration);
    assert_eq!(state.buffered, buffered);
}

#[test]
fn extension_appends_new_tracks_once_in_response_order() {
    let mut state = PlaybackState::default();
    state.replace(vec![tracks()[0].clone(), tracks()[1].clone()], 0);
    state.replace_context(PlaybackContext::DeezerTrackMix {
        seed_track_id: "0".into(),
    });
    let ticket = state.extension_ticket().unwrap();
    let mut duplicate = tracks()[1].clone();
    duplicate.title = "duplicate".into();
    let mut fresh = tracks()[2].clone();
    fresh.title = "fresh".into();
    assert_eq!(
        state.apply_extension(
            &ticket,
            vec![duplicate.clone(), fresh.clone(), duplicate],
            None,
            None,
        ),
        ExtensionApply::Applied { added: 1 }
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["0", "1", "2"]
    );
    assert_eq!(state.queue[2].title, "fresh");
}

#[test]
fn queue_epoch_advances_when_extension_changes_the_queue() {
    let mut state = PlaybackState::default();
    state.replace(vec![tracks()[0].clone()], 0);
    state.replace_context(PlaybackContext::DeezerTrackMix {
        seed_track_id: "0".into(),
    });
    let ticket = state.extension_ticket().unwrap();
    let epoch = state.queue_epoch();

    assert_eq!(
        state.apply_extension(&ticket, vec![tracks()[1].clone()], None, None),
        ExtensionApply::Applied { added: 1 }
    );
    assert_ne!(state.queue_epoch(), epoch);
    assert!(!state.extension_ticket_is_current(&ticket));
}

#[test]
fn flow_extension_keeps_listed_upcoming_tracks_and_appends_the_batch() {
    let mut state = PlaybackState::default();
    state.replace(tracks(), 0);
    state.replace_context(PlaybackContext::DeezerFlow {
        config_id: "flow".into(),
        mode: library::FlowMode::Default,
        tuner: Some(library::FlowTuner::initial(library::FlowMode::Default)),
        kind: DeezerFlowKind::Flow,
    });
    let ticket = state.extension_ticket().unwrap();
    let previous_epoch = state.queue_epoch();

    // Flow responses ask Deezer to clear the remaining queue, but the
    // extension must keep the listed upcoming tracks and append below
    // them like a SoundCloud station.
    let mut duplicate = tracks()[1].clone();
    duplicate.title = "duplicate".into();
    let mut fresh = tracks()[1].clone();
    fresh.id = "fresh".into();
    fresh.title = "fresh flow track".into();

    assert_eq!(
        state.apply_extension(
            &ticket,
            vec![duplicate, fresh],
            Some(library::FlowTuner::initial(library::FlowMode::Discovery)),
            None,
        ),
        ExtensionApply::Applied { added: 1 }
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["0", "1", "2", "fresh"]
    );
    assert_eq!(state.current_index, Some(0));
    assert!(matches!(
        &state.context,
        PlaybackContext::DeezerFlow { tuner, .. }
            if tuner.as_ref().unwrap().as_str()
                == library::FlowTuner::initial(library::FlowMode::Discovery).as_str()
    ));
    assert_ne!(state.queue_epoch(), previous_epoch);
    assert!(!state.extension_ticket_is_current(&ticket));
}

#[test]
fn soundcloud_station_extension_advances_to_the_raw_response_seed() {
    let mut seed = tracks()[0].clone();
    seed.provider = PlaybackProvider::SoundCloud;
    seed.id = "1957877095".into();
    let mut prior_track = tracks()[1].clone();
    prior_track.provider = PlaybackProvider::SoundCloud;
    prior_track.id = "986413711".into();
    let mut state = PlaybackState::default();
    state.replace(vec![prior_track.clone(), seed.clone()], 0);
    state.replace_context(PlaybackContext::SoundCloudStation {
        seed_track_id: "1957877095".into(),
    });
    let ticket = state.extension_ticket().unwrap();
    let previous_epoch = state.queue_epoch();

    let mut duplicate = seed;
    duplicate.title = "duplicate seed".into();
    let mut final_track = duplicate.clone();
    final_track.id = "2132666208".into();
    final_track.title = "final station track".into();
    let raw_response_seed = prior_track.id.clone();

    assert_eq!(
        state.apply_extension(
            &ticket,
            vec![duplicate, final_track, prior_track],
            None,
            Some(raw_response_seed.clone()),
        ),
        ExtensionApply::Applied { added: 1 }
    );
    assert_eq!(
        state
            .queue
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["986413711", "1957877095", "2132666208"]
    );
    assert_eq!(
        state.context,
        PlaybackContext::SoundCloudStation {
            seed_track_id: raw_response_seed.clone(),
        }
    );
    assert_ne!(state.queue_epoch(), previous_epoch);
    assert!(!state.extension_ticket_is_current(&ticket));
    assert_eq!(
        state.extension_ticket().unwrap().context,
        PlaybackContext::SoundCloudStation {
            seed_track_id: raw_response_seed,
        }
    );
}

#[test]
fn can_next_reports_false_on_last_track_without_repeat() {
    let mut state = PlaybackState::default();
    let track_list = tracks();
    state.replace(track_list.clone(), 0);
    assert!(state.can_next());

    // Move to the last track
    state.select(track_list.len() - 1);
    assert!(!state.can_next());

    // With repeat all, can_next is true
    state.repeat_mode = RepeatMode::All;
    assert!(state.can_next());

    // With repeat one, can_next is true
    state.repeat_mode = RepeatMode::One;
    assert!(state.can_next());

    // With repeat off but an index in play_next
    state.repeat_mode = RepeatMode::Off;
    assert!(!state.can_next());
    state.play_next.push(0);
    assert!(state.can_next());
}
