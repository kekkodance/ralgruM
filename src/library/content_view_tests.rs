use super::{
    HEADING_CONTROL_OPTICAL_OFFSET_PX, artist_section_action_visible,
    artist_section_action_visible_for_mode, card_row_for_artist_section, category_empty_copy,
    detail_download_action_eligible, detail_download_page_is_complete, detail_header_page,
    detail_heading_has_context, flow_detail_header_copy, flow_detail_header_page,
    flow_header_subtitle_skeleton_visible, page_count_text, page_empty_copy,
    page_header_secondary_copy, page_header_shows_platform, page_uses_flattened_sections,
    page_uses_virtualized_scroll, playback_context_for_route, playback_context_for_route_with_kind,
    provider_detail_route, root_header_page, soundcloud_tracks_only_page, station_seed_from_page,
    title_uses_intrinsic_width,
};
use crate::collection_detail::{
    DETAIL_CONTEXT_OPTICAL_OFFSET_PX as ARTIST_DETAIL_CONTEXT_OPTICAL_OFFSET_PX,
    DETAIL_HEADER_ARTWORK_RADIUS_PX as ARTIST_DETAIL_HEADER_ARTWORK_RADIUS_PX,
    DETAIL_HEADER_ARTWORK_SIZE_PX as ARTIST_DETAIL_HEADER_ARTWORK_SIZE_PX,
    DETAIL_HEADER_GAP_PX as ARTIST_DETAIL_HEADER_LEFT_GAP_PX,
    DETAIL_SECTION_CONTENT_GAP_PX as ARTIST_DETAIL_SECTION_CONTENT_GAP_PX,
    DETAIL_SECTION_GAP_PX as ARTIST_DETAIL_SECTION_GAP_PX,
    DETAIL_SECTION_TITLE_SIZE_PX as ARTIST_DETAIL_SECTION_TITLE_SIZE_PX,
};
use crate::library::deezer_radio::FlowMode;
use crate::library::model::{
    Card, Category, FLOW_TRACK_DESCRIPTION, Page, Route, Section, SectionLayout, Service, Track,
    is_deezer_flow_detail, is_detail_route,
};
use crate::library::state::Status;
use crate::search::Provider;

#[test]
fn artist_fallback_copy_uses_the_active_service() {
    assert_eq!(
        category_empty_copy(Service::SoundCloud, Category::Artists),
        (
            "No followed artists",
            "Artists you follow on SoundCloud will appear here."
        )
    );
    assert_eq!(
        category_empty_copy(Service::Deezer, Category::Artists),
        (
            "No followed artists",
            "Artists you follow on Deezer will appear here."
        )
    );
}

#[test]
fn local_playlist_empty_copy_is_account_independent() {
    assert_eq!(
        category_empty_copy(Service::Local, Category::Playlists),
        (
            "No local playlists yet",
            "Create a local playlist to organize track references. Local playlists do not download audio files."
        )
    );
}

#[test]
fn local_tracks_empty_copy_matches_the_reference_text() {
    assert_eq!(
        category_empty_copy(Service::Local, Category::Tracks),
        (
            "No local tracks yet",
            "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file."
        )
    );
}

#[test]
fn artist_section_actions_follow_search_preview_rules() {
    let section = Section {
        total: 13,
        preview_limit: Some(12),
        cards: vec![Card::default(); 13],
        ..Section::default()
    };
    assert!(artist_section_action_visible(&section, false));
    assert!(artist_section_action_visible(&section, true));
    let underreported = Section {
        total: 12,
        preview_limit: Some(12),
        cards: vec![Card::default(); 13],
        ..Section::default()
    };
    assert!(artist_section_action_visible(&underreported, false));
    assert!(!artist_section_action_visible_for_mode(
        &section, false, true
    ));
    let short = Section {
        total: 12,
        preview_limit: Some(12),
        cards: vec![Card::default(); 12],
        ..Section::default()
    };
    assert!(!artist_section_action_visible(&short, false));
}

#[test]
fn library_artist_layout_uses_search_geometry() {
    assert_eq!(ARTIST_DETAIL_HEADER_ARTWORK_SIZE_PX, 76.);
    assert_eq!(ARTIST_DETAIL_HEADER_ARTWORK_RADIUS_PX, 6.);
    assert_eq!(ARTIST_DETAIL_HEADER_LEFT_GAP_PX, 16.);
    assert_eq!(ARTIST_DETAIL_CONTEXT_OPTICAL_OFFSET_PX, 2.);
    assert_eq!(ARTIST_DETAIL_SECTION_GAP_PX, 18.);
    assert_eq!(ARTIST_DETAIL_SECTION_CONTENT_GAP_PX, 9.);
    assert_eq!(ARTIST_DETAIL_SECTION_TITLE_SIZE_PX, 15.);
}

#[test]
fn soundcloud_tracks_only_artist_pages_match_search_behavior() {
    let page = Page {
        sections: vec![
            Section {
                title: "Tracks".into(),
                preview_limit: Some(5),
                tracks: vec![Track::default(); 6],
                ..Section::default()
            },
            Section {
                title: "Albums".into(),
                ..Section::default()
            },
        ],
        ..Page::default()
    };
    assert!(soundcloud_tracks_only_page(
        Provider::SoundCloud,
        "artistTracks",
        &page
    ));
    assert!(!soundcloud_tracks_only_page(
        Provider::Deezer,
        "artistTracks",
        &page
    ));

    let mut with_cards = page.clone();
    with_cards.sections[1].cards.push(Card::default());
    assert!(!soundcloud_tracks_only_page(
        Provider::SoundCloud,
        "artistTracks",
        &with_cards
    ));
}

#[test]
fn root_card_pages_use_virtualized_scrolling() {
    for category in [Category::Albums, Category::Artists, Category::Playlists] {
        let page = Page {
            cards: vec![Card {
                kind: category,
                id: "42".into(),
                ..Card::default()
            }],
            ..Page::default()
        };
        assert!(page_uses_virtualized_scroll(&page));
    }
}

#[test]
fn unbounded_card_sections_are_flattened_but_bounded_previews_are_not() {
    let mut page = Page {
        sections: vec![Section {
            layout: SectionLayout::Cards,
            cards: vec![Card {
                kind: Category::Artists,
                id: "artist-1".into(),
                ..Card::default()
            }],
            ..Section::default()
        }],
        ..Page::default()
    };

    assert!(page_uses_flattened_sections(&page));
    page.sections[0].preview_limit = Some(12);
    assert!(!page_uses_flattened_sections(&page));
}

#[test]
fn mixed_unbounded_track_and_card_sections_share_the_flattened_page_path() {
    let page = Page {
        sections: vec![
            Section {
                layout: SectionLayout::Tracks,
                tracks: vec![Track {
                    id: "track-1".into(),
                    ..Track::default()
                }],
                ..Section::default()
            },
            Section {
                layout: SectionLayout::Cards,
                cards: vec![Card {
                    kind: Category::Albums,
                    id: "album-1".into(),
                    ..Card::default()
                }],
                ..Section::default()
            },
        ],
        ..Page::default()
    };

    assert!(page_uses_flattened_sections(&page));
}

#[test]
fn all_empty_detail_pages_use_the_frozen_page_level_fallback() {
    assert_eq!(
        page_empty_copy(&Page::default()),
        (
            "Nothing here yet",
            "This part of your library is currently empty."
        )
    );

    let page = Page {
        empty_title: "No followed artists".into(),
        empty_description: "Artists you follow on SoundCloud will appear here.".into(),
        ..Page::default()
    };
    assert_eq!(
        page_empty_copy(&page),
        (
            "No followed artists",
            "Artists you follow on SoundCloud will appear here."
        )
    );
}

#[test]
fn provider_names_are_reserved_for_opened_detail_headings() {
    assert!(!detail_heading_has_context(1));
    assert!(detail_heading_has_context(2));
}

#[test]
fn detail_route_depth_is_shared_by_root_and_opened_library_chrome() {
    assert!(!is_detail_route(1));
    assert!(is_detail_route(2));
}

#[test]
fn flow_detail_promotion_requires_a_nested_deezer_flow_route() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "dance".into(),
        title: "Dance".into(),
        ..Route::root(Service::Deezer, Category::Flow)
    };

    assert!(is_deezer_flow_detail(Service::Deezer, &route, 2));
    assert!(!is_deezer_flow_detail(Service::Deezer, &route, 1));
    assert!(!is_deezer_flow_detail(Service::SoundCloud, &route, 2));
}

#[test]
fn flow_header_copy_prefers_loaded_page_and_falls_back_to_route() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "dance".into(),
        title: "daily".into(),
        ..Route::root(Service::Deezer, Category::Flow)
    };
    let loaded = Page {
        title: "Electro Dance".into(),
        description: "Loaded description".into(),
        show_count: true,
        total: 42,
        count_noun: "track".into(),
        ..Page::default()
    };

    assert_eq!(
        flow_detail_header_copy(&route, Some(&loaded), false),
        ("Electro Dance", "Loaded description")
    );
    assert_eq!(
        flow_detail_header_copy(&route, None, false),
        ("daily", FLOW_TRACK_DESCRIPTION)
    );
    let projected = flow_detail_header_page(&route, Some(&loaded), false);
    assert!(projected.show_count);
    assert_eq!(projected.total, 42);
    assert_eq!(projected.count_noun, "track");
    assert!(projected.tracks.is_empty());
}

#[test]
fn eponymous_release_headers_still_show_the_artist_subtitle() {
    // An album named after its artist ("Justice" by Justice) must not
    // lose its artist line just because the two strings match.
    assert_eq!(
        page_header_secondary_copy(false, "Justice", "Justice", ""),
        "Justice"
    );
    assert_eq!(
        page_header_secondary_copy(true, "Justice", "Justice", ""),
        "Justice"
    );
    assert_eq!(
        page_header_secondary_copy(false, "Justice", "Justice", "Description"),
        "Description"
    );
    assert_eq!(
        page_header_secondary_copy(false, "Justice", "", "Description"),
        "Description"
    );
    assert_eq!(page_header_secondary_copy(false, "Justice", "", ""), "");
}

#[test]
fn smart_mix_header_uses_known_route_subtitle_then_prefers_authoritative_copy() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "smart-mix".into(),
        title: "Electro Dance".into(),
        subtitle: "Route subtitle".into(),
        ..Route::root(Service::Deezer, Category::Flow)
    };
    let loaded = Page {
        title: "Electro Dance".into(),
        subtitle: "Authoritative subtitle".into(),
        description: "Description fallback".into(),
        ..Page::default()
    };

    assert_eq!(
        flow_detail_header_copy(&route, None, true),
        ("Electro Dance", "")
    );
    let loading = flow_detail_header_page(&route, None, true);
    assert_eq!(loading.subtitle, "Route subtitle");
    let loaded_without_subtitle = Page {
        title: "Electro Dance".into(),
        ..Page::default()
    };
    assert_eq!(
        flow_detail_header_page(&route, Some(&loaded_without_subtitle), true).subtitle,
        "Route subtitle"
    );
    let projected = flow_detail_header_page(&route, Some(&loaded), true);
    assert_eq!(projected.subtitle, "Authoritative subtitle");
    assert_eq!(projected.description, "Description fallback");
    assert_eq!(
        page_header_secondary_copy(
            true,
            &projected.title,
            &projected.subtitle,
            &projected.description,
        ),
        "Authoritative subtitle"
    );

    let cached_generic = Page {
        title: "daily".into(),
        ..Page::default()
    };
    assert_eq!(
        flow_detail_header_copy(&route, Some(&cached_generic), true).0,
        "Electro Dance"
    );
    let generic_route = Route {
        title: "Mix".into(),
        ..route.clone()
    };
    assert_eq!(
        flow_detail_header_copy(&generic_route, Some(&cached_generic), true).0,
        "Mix"
    );
    let endpoint = Page {
        title: "Nuove Uscite".into(),
        ..Page::default()
    };
    let uppercase_route = Route {
        title: "NUOVE USCITE".into(),
        ..route
    };
    assert_eq!(
        flow_detail_header_copy(&uppercase_route, Some(&endpoint), true).0,
        "Nuove Uscite"
    );
}

#[test]
fn smart_mix_header_skeleton_only_fills_genuinely_unknown_loading_copy() {
    assert!(!flow_header_subtitle_skeleton_visible(
        true,
        &Status::Loading,
        false,
        "Known route subtitle"
    ));
    assert!(flow_header_subtitle_skeleton_visible(
        true,
        &Status::Loading,
        false,
        ""
    ));
    assert!(!flow_header_subtitle_skeleton_visible(
        true,
        &Status::Results,
        false,
        ""
    ));
    assert!(!flow_header_subtitle_skeleton_visible(
        true,
        &Status::Loading,
        true,
        ""
    ));
    assert!(!flow_header_subtitle_skeleton_visible(
        false,
        &Status::Loading,
        false,
        ""
    ));
}

#[test]
fn root_header_projection_preserves_copy_and_count_without_body_collections() {
    let loaded = Page {
        title: "Loaded collection".into(),
        description: "Saved tracks".into(),
        show_count: true,
        total: 42,
        count_noun: "track".into(),
        tracks: vec![Track::default()],
        ..Page::default()
    };

    let projected = root_header_page(
        Category::Tracks,
        Some(&loaded),
        "Fallback",
        "Fallback description",
    );

    assert_eq!(projected.title, "Loaded collection");
    assert_eq!(projected.description, "Saved tracks");
    assert!(projected.show_count);
    assert_eq!(projected.total, 42);
    assert_eq!(projected.count_noun, "track");
    assert!(projected.tracks.is_empty());
    assert!(projected.cards.is_empty());
    assert!(projected.sections.is_empty());

    let sectioned = Page {
        title: "Sectioned collection".into(),
        description: "Saved sections".into(),
        show_count: true,
        total: 12,
        count_noun: "item".into(),
        sections: vec![Section::default()],
        ..Page::default()
    };
    let sectioned_projected = root_header_page(
        Category::Tracks,
        Some(&sectioned),
        "Fallback",
        "Fallback description",
    );
    assert!(!sectioned_projected.show_count);
    assert_eq!(sectioned_projected.total, 12);
    assert_eq!(sectioned_projected.count_noun, "item");
}

#[test]
fn root_flow_header_keeps_the_flow_title_after_loading() {
    let loaded = Page {
        title: "Flow: play how you feel".into(),
        description: "Flow: play how you feel".into(),
        ..Page::default()
    };

    let projected = root_header_page(
        Category::Flow,
        Some(&loaded),
        "Flow",
        "Personalized moods and genres for you.",
    );

    assert_eq!(projected.title, "Flow");
    assert_eq!(
        projected.description,
        "Personalized moods and genres for you."
    );
}

#[test]
fn detail_header_projection_covers_artist_album_playlist_and_flow_routes() {
    let routes = [
        (Category::Artists, "artist", "Artist"),
        (Category::Albums, "albumTracks", "Album"),
        (Category::Playlists, "playlistTracks", "Playlist"),
        (Category::Flow, "flowTracks", "Flow mix"),
    ];

    for (category, action, title) in routes {
        let root = Route::root(Service::Deezer, category);
        assert!(!is_detail_route(1));
        assert_eq!(root.action, category.action());

        let route = Route {
            source: Provider::Deezer,
            category,
            action: action.into(),
            id: "detail".into(),
            title: title.into(),
            subtitle: "Context".into(),
            ..Route::root(Service::Deezer, category)
        };
        let projected = detail_header_page(&route, None);

        assert!(is_detail_route(2));
        assert_eq!(projected.title, title);
        assert_eq!(projected.subtitle, "Context");
        if category == Category::Flow {
            assert_eq!(projected.description, FLOW_TRACK_DESCRIPTION);
        } else {
            assert!(projected.description.is_empty());
        }
    }
}

#[test]
fn detail_header_projection_prefers_loaded_metadata_without_copying_body_rows() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "detail".into(),
        title: "Route title".into(),
        subtitle: "Route context".into(),
        artwork: "https://example.com/route.jpg".into(),
        ..Route::root(Service::Deezer, Category::Albums)
    };
    let loaded = Page {
        title: "Loaded album".into(),
        subtitle: "Loaded artist".into(),
        description: "Loaded metadata".into(),
        artwork: "https://example.com/loaded.jpg".into(),
        platform: Some(Service::Deezer),
        meta_text: "Loaded meta".into(),
        show_count: true,
        total: 7,
        count_noun: "track".into(),
        tracks: vec![Track::default()],
        ..Page::default()
    };

    let projected = detail_header_page(&route, Some(&loaded));
    assert_eq!(projected.title, "Loaded album");
    assert_eq!(projected.subtitle, "Loaded artist");
    assert_eq!(projected.description, "Loaded metadata");
    assert_eq!(projected.artwork, "https://example.com/loaded.jpg");
    assert_eq!(projected.meta_text, "Loaded meta");
    assert_eq!(projected.total, 7);
    assert_eq!(projected.count_noun, "track");
    assert!(projected.tracks.is_empty());
    assert!(projected.sections.is_empty());
}

#[test]
fn provider_detail_metadata_uses_loaded_owner_and_canonical_album_release_date() {
    let route = Route {
        source: Provider::SoundCloud,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "album".into(),
        title: "Route album".into(),
        subtitle: "Route artist".into(),
        release_date: "2024-02-03".into(),
        ..Route::root(Service::SoundCloud, Category::Albums)
    };
    let loaded = Page {
        subtitle: "Loaded artist".into(),
        ..Page::default()
    };
    let detail = provider_detail_route(&route, &loaded, crate::search::ResultType::Albums);
    assert_eq!(detail.subtitle, "Loaded artist");
    assert_eq!(detail.release_date, "2024-02-03");
    assert_eq!(
        crate::search::detail_metadata(&detail),
        "Loaded artist • Feb 3, 2024"
    );

    let playlist_route = Route {
        category: Category::Playlists,
        action: "playlistTracks".into(),
        id: "playlist".into(),
        subtitle: "Route owner".into(),
        ..Route::root(Service::SoundCloud, Category::Playlists)
    };
    let playlist_detail = provider_detail_route(
        &playlist_route,
        &Page {
            subtitle: "Loaded owner".into(),
            ..Page::default()
        },
        crate::search::ResultType::Playlists,
    );
    assert_eq!(playlist_detail.subtitle, "Loaded owner");
    assert_eq!(
        crate::search::detail_metadata(&playlist_detail),
        "Loaded owner"
    );
}

#[test]
fn provider_detail_metadata_prefers_loaded_deezer_album_release_date() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "album".into(),
        title: "Route album".into(),
        subtitle: "Route artist".into(),
        release_date: "2011".into(),
        ..Route::root(Service::Deezer, Category::Albums)
    };
    let loaded = Page {
        subtitle: "Loaded artist".into(),
        album_info: Some(crate::search::AlbumInfo {
            release_date: "2011-06-07".into(),
            ..Default::default()
        }),
        ..Page::default()
    };

    let detail = provider_detail_route(&route, &loaded, crate::search::ResultType::Albums);

    assert_eq!(detail.release_date, "2011-06-07");
    assert_eq!(
        crate::search::detail_metadata(&detail),
        "Loaded artist • Jun 7, 2011"
    );
}

#[test]
fn expanded_artist_cards_use_the_full_grid_path() {
    assert!(!card_row_for_artist_section(true, true, true));
    assert!(card_row_for_artist_section(true, true, false));
    assert!(card_row_for_artist_section(true, false, true));
    assert!(!card_row_for_artist_section(false, true, true));
}

#[test]
fn flow_headers_suppress_provider_context_at_every_depth() {
    assert!(!page_header_shows_platform(
        Category::Flow,
        Some(Service::Deezer)
    ));
    assert!(!page_header_shows_platform(
        Category::Flow,
        Some(Service::SoundCloud)
    ));
    assert!(page_header_shows_platform(
        Category::Albums,
        Some(Service::Deezer)
    ));
    assert!(!page_header_shows_platform(Category::Albums, None));
}

#[test]
fn flow_empty_copy_is_provider_neutral() {
    let (_, description) = category_empty_copy(Service::Deezer, Category::Flow);
    assert_eq!(description, "No Flow mixes were returned for this account.");
    assert!(!description.contains("Deezer"));
    assert!(!description.contains("SoundCloud"));
}

#[test]
fn opened_detail_context_uses_a_two_pixel_optical_offset() {
    assert_eq!(HEADING_CONTROL_OPTICAL_OFFSET_PX, 2.);
}

#[test]
fn detail_download_requires_nonempty_complete_track_pages() {
    let empty = Page {
        authoritative_total: Some(0),
        ..Page::default()
    };
    assert!(!detail_download_page_is_complete(&empty));

    let loading = Page {
        raw_loaded_count: 1,
        normalized_count: 1,
        tracks: vec![Track {
            id: "track-1".into(),
            ..Track::default()
        }],
        ..Page::default()
    };
    assert!(!detail_download_page_is_complete(&loading));

    let complete = Page {
        raw_loaded_count: 1,
        normalized_count: 1,
        authoritative_total: Some(1),
        tracks: vec![Track {
            id: "track-1".into(),
            ..Track::default()
        }],
        ..Page::default()
    };
    assert!(detail_download_page_is_complete(&complete));
    assert!(detail_download_action_eligible("albumTracks", 2, &complete));
    assert!(!detail_download_action_eligible(
        "playlistTracks",
        2,
        &complete
    ));
}

#[test]
fn progressive_root_track_count_uses_the_normal_page_total() {
    let page = Page {
        count_noun: "track".into(),
        total: 2_592,
        raw_loaded_count: 256,
        normalized_count: 256,
        authoritative_total: Some(2_592),
        ..Page::default()
    };
    assert_eq!(page_count_text(&page), "2592 tracks");
}

#[test]
fn complete_root_track_count_keeps_the_existing_count_format() {
    let page = Page {
        count_noun: "track".into(),
        total: 2_592,
        raw_loaded_count: 2_592,
        normalized_count: 2_592,
        authoritative_total: Some(2_592),
        ..Page::default()
    };
    assert_eq!(page_count_text(&page), "2592 tracks");
}

#[test]
fn deezer_tracks_root_uses_its_finite_playback_context() {
    let route = Route::root(Service::Deezer, Category::Tracks);
    assert_eq!(route.action, "tracks");
    assert_eq!(
        playback_context_for_route(&route, FlowMode::Default, None, Some(7), None),
        crate::playback::PlaybackContext::DeezerLibraryTracks { load_id: 7 }
    );
    assert_eq!(
        playback_context_for_route(&route, FlowMode::Default, None, None, None),
        crate::playback::PlaybackContext::None
    );
}

#[test]
fn smart_mix_flow_routes_preserve_the_smart_mix_playback_kind() {
    let route = Route {
        source: Provider::Deezer,
        category: Category::Flow,
        action: "flowTracks".into(),
        id: "monthly-top".into(),
        ..Route::root(Service::Deezer, Category::Flow)
    };
    assert_eq!(
        playback_context_for_route_with_kind(
            &route,
            FlowMode::Default,
            None,
            None,
            None,
            crate::playback::DeezerFlowKind::SmartMix,
        ),
        crate::playback::PlaybackContext::DeezerFlow {
            config_id: "monthly-top".into(),
            mode: FlowMode::Default,
            tuner: None,
            kind: crate::playback::DeezerFlowKind::SmartMix,
        }
    );
}

#[test]
fn soundcloud_collection_routes_keep_the_provider_history_context() {
    for (action, expected) in [
        ("albumTracks", "soundcloud:playlists:2189715572"),
        ("playlistTracks", "soundcloud:playlists:2189715572"),
        ("artistTracks", "soundcloud:users:2189715572"),
    ] {
        let route = Route {
            source: Provider::SoundCloud,
            action: action.into(),
            id: "2189715572".into(),
            ..Route::root(Service::SoundCloud, Category::Tracks)
        };
        assert_eq!(
            playback_context_for_route(&route, FlowMode::Default, None, None, None),
            crate::playback::PlaybackContext::SoundCloudCollection {
                context_urn: expected.into(),
            }
        );
    }
}

#[test]
fn soundcloud_station_context_uses_the_last_loaded_track_as_the_next_seed() {
    let route = Route {
        source: Provider::SoundCloud,
        category: Category::Station,
        action: "stationTracks".into(),
        id: "1687721097".into(),
        ..Route::root(Service::SoundCloud, Category::Station)
    };
    let page = Page {
        tracks: vec![
            Track {
                id: "1687721097".into(),
                ..Track::default()
            },
            Track {
                id: "1957877095".into(),
                ..Track::default()
            },
        ],
        ..Page::default()
    };

    assert_eq!(
        playback_context_for_route(
            &route,
            FlowMode::Default,
            None,
            None,
            station_seed_from_page(Some(&page)),
        ),
        crate::playback::PlaybackContext::SoundCloudStation {
            seed_track_id: "1957877095".into(),
        }
    );
}

#[test]
fn station_context_falls_back_to_route_seed_without_a_valid_loaded_track() {
    let route = Route {
        source: Provider::SoundCloud,
        category: Category::Station,
        action: "stationTracks".into(),
        id: "1687721097".into(),
        ..Route::root(Service::SoundCloud, Category::Station)
    };
    let page = Page {
        tracks: vec![Track::default()],
        ..Page::default()
    };

    assert_eq!(station_seed_from_page(Some(&page)), None);
    assert_eq!(
        playback_context_for_route(&route, FlowMode::Default, None, None, None),
        crate::playback::PlaybackContext::SoundCloudStation {
            seed_track_id: "1687721097".into(),
        }
    );
}

#[test]
fn inline_heading_controls_make_the_title_use_only_its_intrinsic_width() {
    assert!(title_uses_intrinsic_width(false, true, false));
    assert!(title_uses_intrinsic_width(true, false, false));
    assert!(title_uses_intrinsic_width(false, false, true));
    assert!(!title_uses_intrinsic_width(false, false, false));
}
