use super::*;
use crate::library::FlowTuner;

fn active_root_after_scope_change(
    state: &mut LibraryState,
    scope: String,
) -> Option<(Service, Category)> {
    scope_reload(state, scope)
}

#[test]
fn account_scope_change_returns_the_active_root_for_reload() {
    let mut state = LibraryState::default();
    state.select(Service::SoundCloud, Category::History);
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
    state.set_account_scope("old account".into());

    assert_eq!(
        active_root_after_scope_change(&mut state, String::new()),
        Some((Service::SoundCloud, Category::History))
    );
    assert_eq!(
        state.routes,
        vec![Route::root(Service::SoundCloud, Category::History)]
    );
}

#[test]
fn every_service_category_has_a_focus_slot() {
    let focus_slots = category_focus_slot_count();

    for service in [Service::Local, Service::Deezer, Service::SoundCloud] {
        for index in 0..service.categories().len() {
            assert!(
                index < focus_slots,
                "missing focus slot for {service:?} category {index}"
            );
        }
    }
}

#[test]
fn only_discover_origin_flow_details_return_to_discover() {
    let origin = Some((Service::SoundCloud, Category::Playlists));
    let flow = Route {
        source: crate::search::Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "flow-id".into(),
        title: "Flow".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    };

    assert!(discover_flow_return_active(
        origin,
        Service::Deezer,
        &flow,
        2
    ));
    assert!(!discover_flow_return_active(
        None,
        Service::Deezer,
        &flow,
        2
    ));
    assert!(!discover_flow_return_active(
        origin,
        Service::Deezer,
        &flow,
        1
    ));
}

#[test]
fn matching_flow_route_refresh_preserves_the_playback_kind() {
    let context = PlaybackContext::DeezerFlow {
        config_id: "inspired-by-1".into(),
        mode: FlowMode::Default,
        tuner: Some(FlowTuner::initial(FlowMode::Default)),
        kind: DeezerFlowKind::SmartMix,
    };

    assert_eq!(
        active_deezer_flow_kind(&context, "inspired-by-1"),
        Some(DeezerFlowKind::SmartMix)
    );
    assert_eq!(active_deezer_flow_kind(&context, "default"), None);
}

#[test]
fn smart_mix_reuses_a_cached_page_while_ordinary_flow_reloads() {
    let page = Page::default();
    assert!(reuse_cached_flow_page(
        &DeezerFlowKind::SmartMix,
        Some(&page)
    ));
    assert!(!reuse_cached_flow_page(&DeezerFlowKind::Flow, Some(&page)));
    assert!(!reuse_cached_flow_page(&DeezerFlowKind::SmartMix, None));
}

#[test]
fn smart_mix_title_event_uses_trimmed_stable_values() {
    assert_eq!(
        smart_mix_title_event(" inspired-by-3 ", " Nuove Uscite "),
        Some(LibraryEvent::SmartMixTitleResolved {
            config_id: "inspired-by-3".into(),
            title: "Nuove Uscite".into(),
        })
    );
    assert!(smart_mix_title_event(" ", "Title").is_none());
    assert!(smart_mix_title_event("config", " ").is_none());
    assert!(smart_mix_title_event("config", "daily").is_none());
    assert!(smart_mix_title_event("config", "Mix").is_none());
}

#[test]
fn local_page_keeps_provider_origins_and_user_facing_empty_copy() {
    let tracks = [crate::library::local_store::LocalTrack {
        provider: crate::search::Provider::SoundCloud,
        id: "42".into(),
        title: "Track".into(),
        artist: "Artist".into(),
        artists: Vec::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        duration: 180,
        artwork: String::new(),
        explicit: false,
        service_url: "https://soundcloud.com/a/t".into(),
    }];
    let page = local_tracks_page(&tracks);

    assert_eq!(page.platform, Some(Service::Local));
    assert_eq!(
        page.tracks[0].origin,
        Some(crate::search::Provider::SoundCloud)
    );
    assert!(page.empty_title.contains("local tracks"));
    assert_eq!(
        page.empty_description,
        "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file."
    );
    assert_eq!(
        page.description,
        "Tracks saved to your local library from Deezer and SoundCloud."
    );
}

#[test]
fn local_playlist_cards_and_details_keep_local_navigation_and_origins() {
    let playlist = crate::library::local_playlist_store::LocalPlaylist {
        id: "local-1".into(),
        title: "Mixed playlist".into(),
        description: "Saved locally".into(),
        artwork: String::new(),
        tracks: vec![crate::library::local_store::LocalTrack {
            provider: crate::search::Provider::Deezer,
            id: "1".into(),
            title: "Track".into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            duration: 180,
            artwork: String::new(),
            explicit: false,
            service_url: String::new(),
        }],
    };
    let root = local_playlists_page(std::slice::from_ref(&playlist));
    let card = root.cards.first().cloned().expect("local playlist card");
    assert_eq!(card.library_service, Some(Service::Local));
    assert!(card.search_card().is_none());
    let route = card_route(card).expect("local playlist route");
    assert_eq!(route.action, "localPlaylistTracks");
    assert!(route.is_local_playlist_detail());
    let detail = local_playlist_page(&playlist);
    assert_eq!(detail.platform, Some(Service::Local));
    assert_eq!(
        detail.tracks[0].origin,
        Some(crate::search::Provider::Deezer)
    );
}

#[test]
fn flow_request_failure_copy_is_provider_neutral() {
    assert_eq!(super::FLOW_REQUEST_FAILURE, "Flow request failed");
    assert!(!super::FLOW_REQUEST_FAILURE.contains("Deezer"));
    assert!(!super::FLOW_REQUEST_FAILURE.contains("SoundCloud"));
}

#[test]
fn both_provider_settings_entry_points_use_the_shared_category() {
    assert_eq!(
        settings_category_for_service(Service::Deezer),
        crate::settings::Category::Providers
    );
    assert_eq!(
        settings_category_for_service(Service::SoundCloud),
        crate::settings::Category::Providers
    );
}

#[test]
fn album_cards_propagate_canonical_release_date_with_badge_fallback() {
    let album = Card {
        kind: Category::Albums,
        id: "42".into(),
        badge: "2024".into(),
        release_date: "2024-05-18".into(),
        source: crate::search::Provider::Deezer,
        ..Card::default()
    };
    let artist = Card {
        kind: Category::Artists,
        id: "7".into(),
        badge: "12".into(),
        source: crate::search::Provider::Deezer,
        ..Card::default()
    };

    assert_eq!(card_route(album).unwrap().release_date, "2024-05-18");
    assert!(card_route(artist).unwrap().release_date.is_empty());

    let fallback = Card {
        kind: Category::Albums,
        id: "43".into(),
        badge: "2024".into(),
        source: crate::search::Provider::Deezer,
        ..Card::default()
    };
    assert_eq!(card_route(fallback).unwrap().release_date, "2024");
}

#[test]
fn provider_playlist_cards_route_to_hydrated_tracks() {
    for source in [
        crate::search::Provider::Deezer,
        crate::search::Provider::SoundCloud,
    ] {
        let route = card_route(Card {
            kind: Category::Playlists,
            id: "42".into(),
            title: "Playlist".into(),
            subtitle: "Uploader".into(),
            artwork: String::new(),
            badge: "8".into(),
            source,
            service_url: "https://soundcloud.com/u/sets/playlist".into(),
            ..Card::default()
        })
        .unwrap();
        assert_eq!(route.action, "playlistTracks");
        assert_eq!(route.source, source);
        assert_eq!(route.id, "42");
    }
}

#[test]
fn soundcloud_album_cards_route_to_hydrated_tracks() {
    let route = card_route(Card {
        kind: Category::Albums,
        id: "84".into(),
        title: "Album".into(),
        subtitle: "Uploader".into(),
        artwork: String::new(),
        badge: "2026".into(),
        release_date: "2026-01-02".into(),
        source: crate::search::Provider::SoundCloud,
        service_url: "https://soundcloud.com/u/sets/album".into(),
        ..Card::default()
    })
    .unwrap();
    assert_eq!(route.action, "albumTracks");
    assert_eq!(route.source, crate::search::Provider::SoundCloud);
    assert_eq!(route.id, "84");
    assert_eq!(route.release_date, "2026-01-02");
}

#[test]
fn provider_collection_card_activation_keeps_library_provider_routes() {
    for source in [
        crate::search::Provider::Deezer,
        crate::search::Provider::SoundCloud,
    ] {
        for (kind, deezer_action, soundcloud_action) in [
            (Category::Albums, "albumTracks", "albumTracks"),
            (Category::Artists, "artist", "artistTracks"),
            (Category::Playlists, "playlistTracks", "playlistTracks"),
        ] {
            let route = card_route(Card {
                kind,
                id: "42".into(),
                source,
                ..Card::default()
            })
            .expect("provider collection card route");
            assert_eq!(route.source, source);
            assert_eq!(
                route.action,
                if source == crate::search::Provider::Deezer {
                    deezer_action
                } else {
                    soundcloud_action
                }
            );
        }
    }
}

#[test]
fn deezer_track_lists_expose_favorite_actions() {
    let deezer_history = Route {
        source: crate::search::Provider::Deezer,
        category: Category::History,
        action: "history".into(),
        ..Route::root(Service::Deezer, Category::History)
    };
    let deezer_artist = Route {
        source: crate::search::Provider::Deezer,
        category: Category::Artists,
        action: "artistTracks".into(),
        ..Route::root(Service::Deezer, Category::Artists)
    };
    let deezer_flow = Route {
        source: crate::search::Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        ..Route::root(Service::Deezer, Category::Flow)
    };

    assert!(track_favorites_available_for_route(&deezer_history));
    assert!(track_favorites_available_for_route(&deezer_artist));
    assert!(track_favorites_available_for_route(&deezer_flow));
}

#[test]
fn soundcloud_history_exposes_soundcloud_favorite_actions() {
    let soundcloud_history = Route::root(Service::SoundCloud, Category::History);

    assert!(track_favorites_available_for_route(&soundcloud_history));
}

#[test]
fn non_tracks_routes_do_not_assume_tracks_are_favorited() {
    let mut state = LibraryState::default();
    state.select(Service::Deezer, Category::History);
    assert!(!state.deezer_root_active(Category::Tracks));

    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Artists,
        action: "artistTracks".into(),
        ..Route::root(Service::Deezer, Category::Artists)
    });
    assert!(!state.deezer_root_active(Category::Tracks));
}

#[test]
fn root_favorite_helpers_cover_supported_roots_and_reject_non_roots() {
    let page = Page {
        tracks: vec![Track {
            id: "track-1".into(),
            ..Track::default()
        }],
        cards: vec![
            Card {
                id: "album-1".into(),
                ..Card::default()
            },
            Card {
                id: "artist-1".into(),
                ..Card::default()
            },
            Card {
                id: "playlist-1".into(),
                ..Card::default()
            },
        ],
        ..Page::default()
    };

    assert_eq!(
        favorite_kind_for_root(Category::Tracks),
        Some(FavoriteKind::Track)
    );
    assert_eq!(
        favorite_kind_for_root(Category::Albums),
        Some(FavoriteKind::Album)
    );
    assert_eq!(
        favorite_kind_for_root(Category::Artists),
        Some(FavoriteKind::Artist)
    );
    assert_eq!(
        favorite_kind_for_root(Category::Playlists),
        Some(FavoriteKind::Playlist)
    );

    assert_eq!(root_favorite_ids(Category::Tracks, &page), vec!["track-1"]);
    assert_eq!(
        root_favorite_ids(Category::Albums, &page),
        vec!["album-1", "artist-1", "playlist-1"]
    );
    assert_eq!(
        root_favorite_ids(Category::Artists, &page),
        vec!["album-1", "artist-1", "playlist-1"]
    );
    assert_eq!(
        root_favorite_ids(Category::Playlists, &page),
        vec!["album-1", "artist-1", "playlist-1"]
    );

    let keys = root_favorite_keys(Service::SoundCloud, Category::Artists, &page);
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|key| {
        key.provider == crate::search::Provider::SoundCloud && key.kind == FavoriteKind::Artist
    }));

    for category in [
        Category::History,
        Category::Flow,
        Category::MyTracks,
        Category::Station,
    ] {
        assert_eq!(favorite_kind_for_root(category), None);
        assert!(root_favorite_ids(category, &page).is_empty());
        assert!(root_favorite_keys(Service::Deezer, category, &page).is_empty());
    }
}

#[test]
fn detail_scroll_reset_moves_cached_lists_to_the_top() {
    let state = ListState::new(8, ListAlignment::Top, px(8.));
    state.scroll_to(ListOffset {
        item_ix: 4,
        offset_in_item: px(3.),
    });

    reset_list_state_to_top(&state);

    let offset = state.logical_scroll_top();
    assert_eq!(offset.item_ix, 0);
    assert_eq!(offset.offset_in_item, px(0.));
}

struct ScrollOwnerProbe {
    scroll: ScrollHandle,
    contained: bool,
}

impl Render for ScrollOwnerProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .children((0..20).map(|_| div().h(px(50.)).flex_none()))
            .into_any_element();
        div().size_full().flex().child(library_content_scroll(
            content,
            self.contained,
            &self.scroll,
            crate::browser_scroll::BrowserScrollState::new(),
        ))
    }
}

#[gpui::test]
fn library_content_scroll_handle_owns_wheel_and_reset(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::configure_component_theme(cx);
    });
    let scroll = ScrollHandle::new();
    let window = cx.add_window({
        let scroll = scroll.clone();
        move |_, _| ScrollOwnerProbe {
            scroll,
            contained: false,
        }
    });
    let mut cx = gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });

    cx.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(px(20.), px(20.)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-120.))),
        ..Default::default()
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });
    assert!(scroll.offset().y < px(0.));

    scroll.set_offset(gpui::point(px(0.), px(0.)));
    assert_eq!(scroll.offset(), gpui::point(px(0.), px(0.)));
}

#[gpui::test]
fn contained_library_content_does_not_scroll(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::theme::configure_component_theme(cx);
    });
    let scroll = ScrollHandle::new();
    let window = cx.add_window({
        let scroll = scroll.clone();
        move |_, _| ScrollOwnerProbe {
            scroll,
            contained: true,
        }
    });
    let mut cx = gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });

    cx.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(px(20.), px(20.)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-120.))),
        ..Default::default()
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        _ = window.draw(cx);
    });

    assert_eq!(scroll.offset(), gpui::point(px(0.), px(0.)));
}

#[test]
fn root_card_categories_share_content_transition_identity() {
    let mut state = LibraryState::default();
    let mut card_identities = Vec::new();

    for category in [Category::Albums, Category::Artists, Category::Playlists] {
        state.select(Service::Deezer, category);
        card_identities.push(library_content_transition_identity(&state));
    }

    assert_eq!(card_identities[0], card_identities[1]);
    assert_eq!(card_identities[1], card_identities[2]);

    state.select(Service::Deezer, Category::Tracks);
    assert_ne!(
        library_content_transition_identity(&state),
        card_identities[0]
    );
}

#[test]
fn logged_out_categories_share_account_required_content_transition_identity() {
    let mut state = LibraryState::default();
    let mut provider_identities = Vec::new();

    for service in [Service::Deezer, Service::SoundCloud] {
        let identities = service
            .categories()
            .iter()
            .copied()
            .map(|category| {
                state.select(service, category);
                state.account_required();
                library_content_transition_identity(&state)
            })
            .collect::<Vec<_>>();

        assert!(identities.windows(2).all(|pair| pair[0] == pair[1]));
        provider_identities.push(identities[0].clone());
    }

    assert_ne!(provider_identities[0], provider_identities[1]);
}

#[test]
fn detail_routes_keep_route_specific_content_transition_identity() {
    let mut state = LibraryState::default();
    state.select(Service::Deezer, Category::Playlists);
    let root_identity = library_content_transition_identity(&state);

    state.push(Route {
        source: crate::search::Provider::Deezer,
        category: Category::Playlists,
        action: "playlistTracks".into(),
        id: "42".into(),
        title: "Playlist".into(),
        subtitle: String::new(),
        artwork: String::new(),
        release_date: String::new(),
    });

    assert_ne!(library_content_transition_identity(&state), root_identity);
}
