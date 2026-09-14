use super::*;

#[test]
fn stale_completion_does_not_replace_current_page() {
    let mut state = LibraryState::default();
    let old = state.select(Service::Deezer, Category::Tracks).0;
    let new = state.select(Service::Deezer, Category::History).0;
    assert!(!state.complete(old, Ok(Page::default())));
    assert_eq!(state.category, Category::History);
    assert!(state.complete(new, Ok(Page::default())));
}

#[test]
fn cache_is_used_after_success() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(
        generation,
        Ok(Page {
            total: 1,
            ..Page::default()
        }),
    );
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
}

#[test]
fn forced_root_reload_bypasses_cache_without_discarding_it() {
    let mut state = LibraryState::default();
    let cached_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(
        cached_generation,
        Ok(Page {
            title: "cached".into(),
            total: 1,
            ..Page::default()
        }),
    );

    let reload_generation = state.reload(Service::Deezer, Category::Tracks);
    assert!(state.page.is_none());
    assert_eq!(state.status, Status::Loading);
    assert!(!state.complete(cached_generation, Ok(Page::default())));
    assert!(state.complete(reload_generation, Err("failed".into())));
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
}

#[test]
fn background_root_reload_keeps_the_visible_page() {
    let mut state = LibraryState::default();
    let loaded = state.select(Service::Deezer, Category::History).0;
    state.complete(
        loaded,
        Ok(Page {
            title: "Visible history".into(),
            total: 1,
            tracks: vec![Track::default()],
            ..Page::default()
        }),
    );

    let refresh = state.reload_preserving_page(Service::Deezer, Category::History);

    assert_eq!(state.status, Status::Results);
    assert_eq!(
        state.page.as_ref().map(|page| page.title.as_str()),
        Some("Visible history")
    );
    assert!(state.complete_preserving_page(refresh, Err("offline".into())));
    assert_eq!(state.status, Status::Results);
    assert_eq!(
        state.page.as_ref().map(|page| page.title.as_str()),
        Some("Visible history")
    );
}

#[test]
fn cache_is_scoped_by_service() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(generation, Ok(Page::default()));
    assert!(
        state
            .select(Service::SoundCloud, Category::Tracks)
            .1
            .is_none()
    );
}

#[test]
fn soundcloud_history_invalidation_removes_only_history_cache() {
    let mut state = LibraryState::default();
    let history = state.select(Service::SoundCloud, Category::History).0;
    state.complete(
        history,
        Ok(Page {
            total: 1,
            ..Page::default()
        }),
    );
    let tracks = state.select(Service::SoundCloud, Category::Tracks).0;
    state.complete(
        tracks,
        Ok(Page {
            total: 1,
            ..Page::default()
        }),
    );
    state.invalidate_soundcloud_history();
    assert!(!state.has_cached(Service::SoundCloud, Category::History));
    assert!(state.has_cached(Service::SoundCloud, Category::Tracks));
}

#[test]
fn deezer_history_invalidation_removes_only_deezer_history_cache() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    for (service, category) in [
        (Service::Deezer, Category::History),
        (Service::Deezer, Category::Tracks),
        (Service::SoundCloud, Category::History),
    ] {
        let generation = state.select(service, category).0;
        state.complete(
            generation,
            Ok(Page {
                total: 1,
                ..Page::default()
            }),
        );
    }

    state.invalidate_deezer(Category::History);

    assert!(!state.has_cached(Service::Deezer, Category::History));
    assert!(state.has_cached(Service::Deezer, Category::Tracks));
    assert!(state.has_cached(Service::SoundCloud, Category::History));
}

#[test]
fn changing_account_scope_hides_visible_page_and_preserves_scoped_caches() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(
        generation,
        Ok(Page {
            title: "first account".into(),
            total: 1,
            ..Page::default()
        }),
    );
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());

    assert!(state.set_account_scope("two".into()));
    assert!(state.page.is_none());
    assert_eq!(state.status, Status::Initial);
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_none());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(
        generation,
        Ok(Page {
            title: "second account".into(),
            total: 1,
            ..Page::default()
        }),
    );

    assert!(state.set_account_scope("one".into()));
    assert_eq!(
        state
            .select(Service::Deezer, Category::Tracks)
            .1
            .unwrap()
            .title,
        "first account"
    );
}

#[test]
fn account_scope_change_invalidates_pending_completion_but_keeps_old_cache() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let old_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(old_generation, Ok(Page::default()));

    let pending_generation = state.select(Service::Deezer, Category::History).0;
    assert!(state.set_account_scope("two".into()));
    assert!(!state.complete(pending_generation, Ok(Page::default())));

    assert!(state.set_account_scope("one".into()));
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
}

#[test]
fn missing_account_and_empty_page_have_distinct_states() {
    let mut state = LibraryState::default();
    state.account_required();
    assert_eq!(state.status, Status::AccountRequired);
    let generation = state.select(Service::Deezer, Category::History).0;
    state.complete(generation, Ok(Page::default()));
    assert_eq!(state.status, Status::Empty);
    assert!(state.page.is_some());
}

#[test]
fn nested_routes_push_and_back_to_the_exact_root() {
    let mut state = LibraryState::default();
    state.select(Service::Deezer, Category::Albums);
    let route = Route {
        source: crate::search::Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "42".into(),
        title: "Album".into(),
        subtitle: "Artist".into(),
        artwork: String::new(),
        release_date: String::new(),
    };
    state.push(route.clone());
    assert_eq!(
        state.routes,
        vec![Route::root(Service::Deezer, Category::Albums), route]
    );
    let (_, root, _) = state.back().unwrap();
    assert_eq!(root, Route::root(Service::Deezer, Category::Albums));
    assert!(state.back().is_none());
}

#[test]
fn back_rejects_a_stale_nested_completion() {
    let mut state = LibraryState::default();
    let (generation, _) = state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Playlists,
        action: "playlistTracks".into(),
        id: "7".into(),
        title: "Playlist".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    state.back();
    assert!(!state.complete(generation, Ok(Page::default())));
    assert_eq!(state.routes.len(), 1);
}

#[test]
fn nested_page_does_not_replace_the_root_cache() {
    let mut state = LibraryState::default();
    let root_generation = state.select(Service::SoundCloud, Category::Artists).0;
    let root = Page {
        title: "Followed Artists".into(),
        ..Page::default()
    };
    state.complete(root_generation, Ok(root.clone()));

    let (nested_generation, _) = state.push(Route {
        source: crate::search::Provider::SoundCloud,
        category: Category::Artists,
        action: "artistTracks".into(),
        id: "42".into(),
        title: "Artist".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    state.complete_nested(
        nested_generation,
        Ok(Page {
            title: "Artist".into(),
            ..Page::default()
        }),
    );

    assert_eq!(
        state
            .select(Service::SoundCloud, Category::Artists)
            .1
            .unwrap()
            .title,
        root.title
    );
}

#[test]
fn track_mutation_invalidates_only_the_active_accounts_deezer_tracks_cache() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(generation, Ok(Page::default()));
    state.invalidate_deezer(Category::Tracks);
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_none());

    state.set_account_scope("two".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(generation, Ok(Page::default()));
    state.set_account_scope("one".into());
    state.invalidate_deezer(Category::Tracks);
    state.set_account_scope("two".into());
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
}

#[test]
fn provider_invalidation_keeps_the_other_services_collection_cache() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());

    let generation = state.select(Service::Deezer, Category::Albums).0;
    state.complete(generation, Ok(Page::default()));
    let generation = state.select(Service::SoundCloud, Category::Albums).0;
    state.complete(generation, Ok(Page::default()));

    state.invalidate_provider(crate::search::Provider::SoundCloud, Category::Albums);

    assert!(state.has_cached(Service::Deezer, Category::Albums));
    assert!(!state.has_cached(Service::SoundCloud, Category::Albums));
}

#[test]
fn playlist_delete_pops_exact_detail_and_invalidates_only_playlist_cache() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Playlists).0;
    state.complete(generation, Ok(Page::default()));
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete(generation, Ok(Page::default()));
    state.select(Service::Deezer, Category::Playlists);
    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Playlists,
        action: "playlistTracks".into(),
        id: "42".into(),
        title: "Title".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    state.invalidate_deezer(Category::Playlists);
    state.back();
    assert_eq!(state.routes.len(), 1);
    assert!(!state.has_cached(Service::Deezer, Category::Playlists));
    assert!(state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn playlist_update_patches_visible_and_cached_cards_before_refresh() {
    use crate::library::{
        model::{Card, Section},
        playlist_client::{OwnedPlaylist, PlaylistOwner},
    };

    fn playlist_card(artwork: &str) -> Card {
        Card {
            kind: Category::Playlists,
            id: "42".into(),
            title: "Old title".into(),
            subtitle: "Old owner".into(),
            artwork: artwork.into(),
            source: crate::search::Provider::Deezer,
            ..Card::default()
        }
    }

    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Playlists).0;
    state.complete(
        generation,
        Ok(Page {
            cards: vec![playlist_card("old-root")],
            sections: vec![Section {
                cards: vec![playlist_card("old-section")],
                ..Section::default()
            }],
            ..Page::default()
        }),
    );
    let playlist = OwnedPlaylist {
        id: "42".into(),
        title: "New title".into(),
        description: String::new(),
        is_private: false,
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: "7".into(),
            name: "New owner".into(),
        },
        artwork: "new-art".into(),
        track_count: None,
    };

    state.patch_deezer_playlist(&playlist);

    let page = state.page.as_ref().unwrap();
    for card in [&page.cards[0], &page.sections[0].cards[0]] {
        assert_eq!(card.title, "New title");
        assert_eq!(card.subtitle, "New owner");
        assert_eq!(card.artwork, "new-art");
    }
    let cached = state
        .cache
        .get(&(state.route().clone(), "one".into()))
        .unwrap();
    assert_eq!(cached.cards[0].artwork, "new-art");
}

#[test]
fn deezer_tracks_root_is_the_only_favorite_refresh_route() {
    let mut state = LibraryState::default();
    assert!(state.deezer_root_active(Category::Tracks));

    state.select(Service::Deezer, Category::History);
    assert!(!state.deezer_root_active(Category::Tracks));

    state.select(Service::SoundCloud, Category::Tracks);
    assert!(!state.deezer_root_active(Category::Tracks));

    state.select(Service::Deezer, Category::Tracks);
    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "42".into(),
        title: "Album".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    assert!(!state.deezer_root_active(Category::Tracks));
}

#[test]
fn collection_refresh_eligibility_requires_the_exact_active_root() {
    let mut state = LibraryState::default();
    for category in [Category::Albums, Category::Artists, Category::Playlists] {
        state.select(Service::Deezer, category);
        assert!(state.deezer_root_active(category));
        assert!(!state.deezer_root_active(Category::Tracks));
    }
    state.select(Service::Deezer, Category::Albums);
    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "42".into(),
        title: "Album".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    assert!(!state.deezer_root_active(Category::Albums));
}

#[test]
fn deezer_history_refresh_eligibility_requires_the_exact_active_root() {
    let mut state = LibraryState::default();
    state.select(Service::Deezer, Category::History);
    assert!(state.deezer_root_active(Category::History));

    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::History,
        action: "historyTrack".into(),
        id: "42".into(),
        title: "Track".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    assert!(!state.deezer_root_active(Category::History));

    state.select(Service::SoundCloud, Category::History);
    assert!(!state.deezer_root_active(Category::History));
    state.select(Service::Deezer, Category::Tracks);
    assert!(!state.deezer_root_active(Category::History));
}

#[test]
fn selected_root_eligibility_includes_soundcloud_collections() {
    let mut state = LibraryState::default();
    state.select(Service::SoundCloud, Category::Artists);
    assert!(state.selected_root_active(Category::Artists));

    state.push(Route {
        source: crate::search::Provider::SoundCloud,
        category: Category::Artists,
        action: "artistTracks".into(),
        id: "42".into(),
        title: "Artist".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });
    assert!(!state.selected_root_active(Category::Artists));
}

#[test]
fn account_scope_changes_preserve_the_local_page() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Local, Category::Tracks).0;
    state.complete_nested_without_cache(generation, Ok(tracks_page("Local Tracks")));
    let page = state.page.clone();
    let route = state.route().clone();

    assert!(!state.set_account_scope("another account".into()));
    assert_eq!(state.route(), &route);
    assert_eq!(state.status, Status::Results);
    assert_eq!(
        state.page.as_ref().map(|page| format!("{page:?}")),
        page.as_ref().map(|page| format!("{page:?}"))
    );
}

fn tracks_page(title: &str) -> Page {
    Page {
        title: title.into(),
        tracks: vec![super::super::model::Track {
            id: "42".into(),
            ..super::super::model::Track::default()
        }],
        ..Page::default()
    }
}

fn soundcloud_station_route() -> Route {
    Route {
        source: crate::search::Provider::SoundCloud,
        category: Category::Station,
        action: "stationTracks".into(),
        id: "station".into(),
        title: "Station".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    }
}

fn soundcloud_track(id: &str) -> Track {
    Track {
        id: id.into(),
        ..Track::default()
    }
}

fn active_soundcloud_station_state(ids: &[&str]) -> LibraryState {
    let mut state = LibraryState::default();
    state.set_account_scope("soundcloud-account".into());
    state.select(Service::SoundCloud, Category::Station);
    let generation = state.push(soundcloud_station_route()).0;
    state.complete_nested(
        generation,
        Ok(Page {
            show_count: false,
            total: ids.len(),
            raw_loaded_count: 17,
            normalized_count: 13,
            authoritative_total: Some(99),
            tracks: ids.iter().map(|id| soundcloud_track(id)).collect(),
            ..Page::default()
        }),
    );
    state
}

fn smart_mix_route() -> Route {
    Route {
        source: crate::search::Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "inspired-by-3".into(),
        title: "Riddim Dubstep".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    }
}

fn active_deezer_smart_mix_state(ids: &[&str]) -> LibraryState {
    let mut state = LibraryState::default();
    state.set_account_scope("deezer-account".into());
    state.select(Service::Deezer, Category::Flow);
    let generation = state.push(smart_mix_route()).0;
    state.complete_nested(
        generation,
        Ok(Page {
            total: ids.len(),
            tracks: ids.iter().map(|id| soundcloud_track(id)).collect(),
            ..Page::default()
        }),
    );
    state
}

fn assert_state_unchanged(
    state: &LibraryState,
    route: &Route,
    page: &Option<Page>,
    cache: &HashMap<(Route, String), Page>,
) {
    assert_eq!(state.route(), route);
    assert_eq!(state.status, Status::Results);
    assert_eq!(
        state.page.as_ref().map(|page| format!("{page:?}")),
        page.as_ref().map(|page| format!("{page:?}"))
    );
    assert_eq!(format!("{:?}", state.cache), format!("{:?}", cache));
}

#[test]
fn soundcloud_station_extension_appends_fresh_tracks_and_updates_cache() {
    let mut state = active_soundcloud_station_state(&["10", "123"]);
    let route = state.route().clone();
    let additions = [
        soundcloud_track(" 123 "),
        soundcloud_track("200"),
        soundcloud_track("200"),
        soundcloud_track("300"),
    ];

    let appended = state.append_active_soundcloud_station_tracks(" 123 ", &additions);

    assert_eq!(appended, 2);
    let page = state.page.as_ref().unwrap();
    assert_eq!(
        page.tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["10", "123", "200", "300"]
    );
    assert_eq!(page.total, page.tracks.len());
    assert!(!page.show_count);
    assert_eq!(page.raw_loaded_count, 17);
    assert_eq!(page.normalized_count, 13);
    assert_eq!(page.authoritative_total, Some(99));

    state.back();
    let (_, cached) = state.push(route);
    let cached = cached.unwrap();
    assert_eq!(
        cached
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["10", "123", "200", "300"]
    );
    assert_eq!(state.page.as_ref().unwrap().tracks, cached.tracks);
}

#[test]
fn soundcloud_station_extension_rejects_invalid_guards_without_mutation() {
    let mut mismatched_seed = active_soundcloud_station_state(&["10", "123"]);
    let route = mismatched_seed.route().clone();
    let page = mismatched_seed.page.clone();
    let cache = mismatched_seed.cache.clone();
    assert_eq!(
        mismatched_seed.append_active_soundcloud_station_tracks("999", &[soundcloud_track("200")]),
        0
    );
    assert_state_unchanged(&mismatched_seed, &route, &page, &cache);

    for (source, action) in [
        (crate::search::Provider::Deezer, "stationTracks"),
        (crate::search::Provider::SoundCloud, "station"),
    ] {
        let mut wrong_route = active_soundcloud_station_state(&["10", "123"]);
        wrong_route.routes.last_mut().unwrap().source = source;
        wrong_route.routes.last_mut().unwrap().action = action.into();
        let route = wrong_route.route().clone();
        let page = wrong_route.page.clone();
        let cache = wrong_route.cache.clone();
        assert_eq!(
            wrong_route.append_active_soundcloud_station_tracks("123", &[soundcloud_track("200")]),
            0
        );
        assert_state_unchanged(&wrong_route, &route, &page, &cache);
    }

    for invalid_last_id in ["", "not-numeric"] {
        let mut invalid_page = active_soundcloud_station_state(&["10", "123"]);
        invalid_page
            .page
            .as_mut()
            .unwrap()
            .tracks
            .last_mut()
            .unwrap()
            .id = invalid_last_id.into();
        let route = invalid_page.route().clone();
        let page = invalid_page.page.clone();
        let cache = invalid_page.cache.clone();
        assert_eq!(
            invalid_page.append_active_soundcloud_station_tracks("123", &[soundcloud_track("200")]),
            0
        );
        assert_state_unchanged(&invalid_page, &route, &page, &cache);
    }
}

#[test]
fn soundcloud_station_extension_returns_zero_for_duplicate_only_batches() {
    let mut state = active_soundcloud_station_state(&["10", "123"]);
    let route = state.route().clone();
    let page = state.page.clone();
    let cache = state.cache.clone();

    assert_eq!(
        state.append_active_soundcloud_station_tracks(
            "123",
            &[
                soundcloud_track(" 123 "),
                soundcloud_track("10"),
                soundcloud_track("10"),
            ]
        ),
        0
    );
    assert_state_unchanged(&state, &route, &page, &cache);
}

#[test]
fn smart_mix_extension_appends_fresh_tracks_and_updates_cache() {
    let mut state = active_deezer_smart_mix_state(&["1", "2"]);
    let route = state.route().clone();

    let appended = state.sync_active_deezer_smart_mix_tracks(
        "inspired-by-3",
        &[
            soundcloud_track("2"),
            soundcloud_track("3"),
            soundcloud_track("3"),
        ],
    );

    assert_eq!(appended, 1);
    assert_eq!(
        state
            .page
            .as_ref()
            .unwrap()
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert_eq!(state.page.as_ref().unwrap().total, 3);
    assert_eq!(
        state
            .cache
            .get(&cache_key(&route, "deezer-account"))
            .unwrap()
            .tracks
            .len(),
        3
    );
}

#[test]
fn smart_mix_extension_rejects_stale_or_mismatched_pages_without_mutation() {
    let mut state = active_deezer_smart_mix_state(&["1", "2"]);
    let route = state.route().clone();
    let page = state.page.clone();
    let cache = state.cache.clone();

    assert_eq!(
        state.sync_active_deezer_smart_mix_tracks("other-mix", &[soundcloud_track("3")],),
        0
    );
    assert_state_unchanged(&state, &route, &page, &cache);

    state.status = Status::Loading;
    assert_eq!(
        state.sync_active_deezer_smart_mix_tracks("inspired-by-3", &[soundcloud_track("3")],),
        0
    );
    assert_eq!(
        state.page.as_ref().map(|page| format!("{page:?}")),
        page.as_ref().map(|page| format!("{page:?}"))
    );
    assert_eq!(format!("{:?}", state.cache), format!("{:?}", cache));
}

#[test]
fn smart_mix_duplicate_only_batch_returns_zero_without_mutation() {
    let mut state = active_deezer_smart_mix_state(&["1", "2", "3"]);
    let route = state.route().clone();
    let page = state.page.clone();
    let cache = state.cache.clone();

    assert_eq!(
        state.sync_active_deezer_smart_mix_tracks(
            "inspired-by-3",
            &[soundcloud_track(" 2 "), soundcloud_track("1")],
        ),
        0
    );
    assert_state_unchanged(&state, &route, &page, &cache);
}

#[test]
fn smart_mix_title_handoff_updates_the_active_route_and_page() {
    let mut state = active_deezer_smart_mix_state(&["1", "2"]);
    state.routes.last_mut().unwrap().title = "daily".into();
    state.page.as_mut().unwrap().title = "daily".into();

    assert!(state.update_active_deezer_smart_mix_title("inspired-by-3", "Electro Dance"));
    assert_eq!(state.route().title, "Electro Dance");
    assert_eq!(state.page.as_ref().unwrap().title, "Electro Dance");
    assert!(!state.update_active_deezer_smart_mix_title("other-mix", "Stale title"));
    assert_eq!(state.route().title, "Electro Dance");
    assert_eq!(
        state
            .cache
            .get(&cache_key(state.route(), "deezer-account"))
            .map(|page| page.title.as_str()),
        Some("Electro Dance")
    );
}

#[test]
fn smart_mix_title_handoff_rejects_generic_placeholders() {
    let mut state = active_deezer_smart_mix_state(&["1", "2"]);
    state.routes.last_mut().unwrap().title = "Electro Dance".into();
    state.page.as_mut().unwrap().title = "Electro Dance".into();

    assert!(!state.update_active_deezer_smart_mix_title("inspired-by-3", "daily"));
    assert_eq!(state.route().title, "Electro Dance");
    assert_eq!(state.page.as_ref().unwrap().title, "Electro Dance");
}

#[test]
fn smart_mix_cache_identity_ignores_presentation_fields() {
    let mut state = active_deezer_smart_mix_state(&["1"]);
    state.back();

    let mut reopened = smart_mix_route();
    reopened.id = " inspired-by-3 ".into();
    reopened.title = "A different card title".into();
    reopened.subtitle = "A different subtitle".into();
    reopened.artwork = "new-artwork".into();
    reopened.release_date = "2026-09-10".into();

    let (_, cached) = state.push(reopened);
    assert_eq!(cached.as_ref().map(|page| page.tracks.len()), Some(1));
}

#[test]
fn smart_mix_cache_isolated_by_account_scope() {
    let mut state = active_deezer_smart_mix_state(&["1"]);
    state.back();
    assert!(state.set_account_scope("another-deezer-account".into()));

    let (_, cached) = state.push(smart_mix_route());
    assert!(cached.is_none());
}

#[test]
fn preview_completion_is_visible_but_not_cached() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;

    assert!(state.complete_tracks_preview(generation, tracks_page("raw")));
    assert_eq!(state.status, Status::Results);
    assert_eq!(state.page.as_ref().unwrap().title, "raw");
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn accepted_tail_preserves_the_stable_load_id_and_is_reused_on_return() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));
    assert!(state.complete_tracks_tail(generation, tracks_page("full")));
    assert_eq!(state.active_tracks_load_id(), Some(generation));
    assert!(state.has_cached(Service::Deezer, Category::Tracks));

    state.select(Service::Deezer, Category::Albums);
    let (_, cached) = state.select(Service::Deezer, Category::Tracks);
    assert_eq!(
        cached.as_ref().map(|page| page.title.as_str()),
        Some("full")
    );
}

#[test]
fn stale_tail_cannot_publish_or_expose_append_data() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("prefix"));
    state.select(Service::Deezer, Category::Albums);
    assert!(!state.complete_tracks_tail(generation, tracks_page("stale")));
    assert_eq!(state.active_tracks_load_id(), None);
}

#[test]
fn a_new_deezer_tracks_root_blocks_the_previous_playback_tail() {
    let mut state = LibraryState::default();
    let old_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(old_generation, tracks_page("old"));
    state.select(Service::Deezer, Category::History);

    let new_generation = state.select(Service::Deezer, Category::Tracks).0;

    assert!(!state.tracks_playback_append_allowed(old_generation));
    assert!(state.tracks_playback_append_allowed(new_generation));
}

#[test]
fn enrichment_completion_replaces_and_caches_the_preview() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));

    assert!(state.complete_tracks_enrichment(generation, Ok(tracks_page("rich"))));
    assert_eq!(state.page.as_ref().unwrap().title, "rich");
    assert!(state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn stale_enrichment_cannot_replace_preview() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));
    state.reload(Service::Deezer, Category::Tracks);

    assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
    assert!(state.page.is_none());
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn enrichment_failure_keeps_preview() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));

    assert!(!state.complete_tracks_enrichment(generation, Err("failed".into())));
    assert_eq!(state.status, Status::Results);
    assert_eq!(state.page.as_ref().unwrap().title, "raw");
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn refresh_failure_keeps_a_disk_snapshot_visible() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("disk snapshot"));

    assert!(!state.complete_tracks_refresh_failure(generation, "offline".into()));
    assert_eq!(state.status, Status::Results);
    assert_eq!(state.page.as_ref().unwrap().title, "disk snapshot");
}

#[test]
fn refresh_failure_without_a_snapshot_keeps_first_load_error_behavior() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;

    assert!(state.complete_tracks_refresh_failure(generation, "offline".into()));
    assert_eq!(state.status, Status::Failed("offline".into()));
    assert!(state.page.is_none());
}

#[test]
fn late_refresh_failure_cannot_touch_another_account() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let old_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(old_generation, tracks_page("old snapshot"));
    state.set_account_scope("two".into());
    let current_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(current_generation, tracks_page("new snapshot"));

    assert!(!state.complete_tracks_refresh_failure(old_generation, "late".into()));
    assert_eq!(state.status, Status::Results);
    assert_eq!(state.page.as_ref().unwrap().title, "new snapshot");
}

#[test]
fn expired_session_replaces_visible_tracks_snapshot() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_cached_pipeline(token, tracks_page("disk snapshot")));

    assert_eq!(
        state.fail_tracks_pipeline_load(token, generation, DEEZER_SESSION_EXPIRED.into()),
        TracksPipelineLoadOutcome::Failed
    );
    assert_eq!(state.status, Status::Failed(DEEZER_SESSION_EXPIRED.into()));
    assert!(state.page.is_none());
    assert_eq!(state.tracks_pipeline_token(), None);
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));

    let (_, cached) = state.select(Service::Deezer, Category::Tracks);
    assert!(cached.is_none());
    assert!(state.page.is_none());
}

#[test]
fn non_expired_failure_keeps_a_visible_snapshot_retryable() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_cached_pipeline(token, tracks_page("disk snapshot")));

    assert_eq!(
        state.fail_tracks_pipeline_load(token, generation, "offline".into()),
        TracksPipelineLoadOutcome::Ignored
    );
    assert_eq!(state.status, Status::Results);
    assert_eq!(state.page.as_ref().unwrap().title, "disk snapshot");
    assert!(state.tracks_pipeline_retryable());
}

#[test]
fn late_cached_page_cannot_publish_after_an_expired_failure() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_cached_pipeline(token, tracks_page("disk snapshot")));
    assert_eq!(
        state.fail_tracks_pipeline_load(token, generation, DEEZER_SESSION_EXPIRED.into()),
        TracksPipelineLoadOutcome::Failed
    );

    assert!(!state.accept_tracks_cached_pipeline(token, tracks_page("late")));
    assert!(state.page.is_none());

    state.select(Service::Deezer, Category::Tracks);
    let token = state.begin_tracks_pipeline();
    assert!(!state.accept_tracks_cached_pipeline(token, tracks_page("late")));
    assert!(state.page.is_none());
    assert_eq!(state.status, Status::Loading);
}

#[test]
fn live_success_lifts_the_expired_snapshot_guard() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    let token = state.begin_tracks_pipeline();
    assert_eq!(
        state.fail_tracks_pipeline_load(token, generation, DEEZER_SESSION_EXPIRED.into()),
        TracksPipelineLoadOutcome::Failed
    );

    state.select(Service::Deezer, Category::Tracks);
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_preview_pipeline(token, tracks_page("live")));

    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_cached_pipeline(token, tracks_page("disk snapshot")));
}

#[test]
fn refresh_failure_with_expired_session_replaces_the_snapshot() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    assert!(state.complete_tracks_tail(generation, tracks_page("disk snapshot")));

    assert!(state.complete_tracks_refresh_failure(generation, DEEZER_SESSION_EXPIRED.into()));
    assert_eq!(state.status, Status::Failed(DEEZER_SESSION_EXPIRED.into()));
    assert!(state.page.is_none());
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));
    assert!(state.select(Service::Deezer, Category::Tracks).1.is_none());
}

#[test]
fn failed_enrichment_accepts_only_the_current_raw_preview() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));

    assert!(state.accept_tracks_enrichment_failure(generation));
    assert_eq!(state.page.as_ref().unwrap().title, "raw");
    let stale = generation;
    state.reload(Service::Deezer, Category::Tracks);
    assert!(!state.accept_tracks_enrichment_failure(stale));
}

#[test]
fn favorite_invalidation_rejects_pending_enrichment() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));
    state.invalidate_deezer(Category::Tracks);

    assert!(!state.tracks_playback_append_allowed(generation));
    assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
    assert_eq!(state.page.as_ref().unwrap().title, "raw");
    assert!(!state.has_cached(Service::Deezer, Category::Tracks));
}

#[test]
fn favorite_invalidation_rejects_a_pending_initial_load() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;

    state.invalidate_deezer(Category::Tracks);

    assert!(!state.complete_tracks_preview(generation, tracks_page("stale")));
    assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
    assert!(!state.tracks_playback_append_allowed(generation));
    assert!(state.page.is_none());
    assert_eq!(state.status, Status::Loading);
}

#[test]
fn favorite_invalidation_rejects_hydration_after_the_tail() {
    let mut state = LibraryState::default();
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(generation, tracks_page("raw"));
    state.complete_tracks_tail(generation, tracks_page("full"));

    state.invalidate_deezer(Category::Tracks);

    assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
    assert!(!state.tracks_playback_append_allowed(generation));
}

#[test]
fn navigation_rejects_enrichment_from_the_previous_tracks_root() {
    let mut state = LibraryState::default();
    let tracks_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(tracks_generation, tracks_page("raw"));
    let history_generation = state.select(Service::Deezer, Category::History).0;
    state.complete(history_generation, Ok(tracks_page("history")));

    assert!(state.tracks_playback_append_allowed(tracks_generation));
    assert!(!state.complete_tracks_enrichment(tracks_generation, Ok(tracks_page("stale"))));
    assert_eq!(state.page.as_ref().unwrap().title, "history");
}

#[test]
fn track_mutation_after_navigation_blocks_the_old_playback_tail() {
    let mut state = LibraryState::default();
    let tracks_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(tracks_generation, tracks_page("prefix"));
    state.select(Service::Deezer, Category::History);
    assert!(state.tracks_playback_append_allowed(tracks_generation));

    state.invalidate_deezer(Category::Tracks);

    assert!(!state.tracks_playback_append_allowed(tracks_generation));
    assert_eq!(state.category, Category::History);
}

#[test]
fn account_change_rejects_enrichment_from_the_previous_scope() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let old_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(old_generation, tracks_page("old raw"));

    state.set_account_scope("two".into());
    let new_generation = state.select(Service::Deezer, Category::Tracks).0;
    state.complete_tracks_preview(new_generation, tracks_page("new raw"));

    assert!(!state.tracks_playback_append_allowed(old_generation));
    assert!(state.tracks_playback_append_allowed(new_generation));
    assert!(!state.complete_tracks_enrichment(old_generation, Ok(tracks_page("old rich"))));
    assert_eq!(state.page.as_ref().unwrap().title, "new raw");
}

#[test]
fn progressive_preview_is_reused_after_leaving_and_returning() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let generation = state.select(Service::Deezer, Category::Tracks).0;
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_preview_pipeline(token, tracks_page("preview")));

    state.select(Service::Deezer, Category::History);
    let (_, cached) = state.select(Service::Deezer, Category::Tracks);

    assert_eq!(
        cached.as_ref().map(|page| page.title.as_str()),
        Some("preview")
    );
    assert!(state.tracks_pipeline_active());
    assert_ne!(generation, state.active_generation());
}

#[test]
fn stale_tracks_load_failure_restarts_once_for_the_returned_route() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    let old_generation = state.select(Service::Deezer, Category::Tracks).0;
    let old_token = state.begin_tracks_pipeline();

    state.select(Service::Deezer, Category::History);
    let returned_generation = state.select(Service::Deezer, Category::Tracks).0;
    let outcome = state.fail_tracks_pipeline_load(old_token, old_generation, "offline".into());

    let TracksPipelineLoadOutcome::Restart { generation, token } = outcome else {
        panic!("a returned Tracks route should restart the failed base request");
    };
    assert_eq!(generation, returned_generation);
    assert_ne!(token, old_token);
    assert!(state.tracks_pipeline_active());

    assert_eq!(
        state.fail_tracks_pipeline_load(token, generation, "offline".into()),
        TracksPipelineLoadOutcome::Failed
    );
    assert!(matches!(state.status, Status::Failed(_)));
}

#[test]
fn raw_tail_survives_navigation_and_enrichment_can_resume() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    state.select(Service::Deezer, Category::Tracks);
    let token = state.begin_tracks_pipeline();
    state.accept_tracks_preview_pipeline(token, tracks_page("preview"));
    state.select(Service::Deezer, Category::History);

    assert!(!state.accept_tracks_tail_pipeline(token, tracks_page("raw")));
    assert!(state.has_cached(Service::Deezer, Category::Tracks));
    assert!(!state.fail_tracks_pipeline_enrichment(token));

    let (_, cached) = state.select(Service::Deezer, Category::Tracks);
    assert_eq!(cached.as_ref().map(|page| page.title.as_str()), Some("raw"));
    assert!(state.tracks_pipeline_retryable());
}

#[test]
fn continuation_failure_is_retryable_after_return_without_refetching_preview() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    state.select(Service::Deezer, Category::Tracks);
    let token = state.begin_tracks_pipeline();
    assert!(state.accept_tracks_preview_pipeline(token, tracks_page("preview")));

    state.select(Service::Deezer, Category::History);
    state.select(Service::Deezer, Category::Tracks);
    assert!(state.fail_tracks_pipeline_continuation(token));
    assert!(state.tracks_pipeline_retryable());
    assert!(state.begin_tracks_pipeline_continuation_retry(token));
    assert!(!state.tracks_pipeline_retryable());
    assert_eq!(
        state.page.as_ref().map(|page| page.title.as_str()),
        Some("preview")
    );
}

#[test]
fn progressive_pipeline_rejects_a_previous_account() {
    let mut state = LibraryState::default();
    state.set_account_scope("one".into());
    state.select(Service::Deezer, Category::Tracks);
    let token = state.begin_tracks_pipeline();
    state.set_account_scope("two".into());
    state.select(Service::Deezer, Category::Tracks);

    assert!(!state.accept_tracks_preview_pipeline(token, tracks_page("old")));
    assert!(state.page.is_none());
    assert_eq!(state.tracks_pipeline_token(), None);
}
