use super::*;

#[test]
fn smart_tracklist_page_keeps_server_metadata_and_is_finite() {
    let page = smart_tracklist_page(
        Route {
            source: crate::search::Provider::Deezer,
            category: Category::Flow,
            action: "flowTracks".into(),
            id: "inspired-by-3".into(),
            title: "Card title".into(),
            subtitle: "Card subtitle".into(),
            artwork: "card-artwork".into(),
            release_date: String::new(),
        },
        super::super::deezer_radio::DeezerSmartTracklist {
            tracks: vec![Track {
                id: "42".into(),
                ..Track::default()
            }],
            total: 50,
            title: "Server title".into(),
            resolved_smart_mix_title: Some("Server title".into()),
            subtitle: "Server subtitle".into(),
            description: "Server description".into(),
            artwork: "server-artwork".into(),
        },
    );

    assert_eq!(page.title, "Server title");
    assert_eq!(
        page.resolved_smart_mix_title.as_deref(),
        Some("Server title")
    );
    assert_eq!(page.subtitle, "Server subtitle");
    assert_eq!(page.description, "Server description");
    assert_eq!(page.artwork, "server-artwork");
    assert_eq!(page.total, 50);
    assert!(page.next_flow_tuner.is_none());
    assert!(page.clear_remaining_tracks);
    assert_eq!(page.platform, Some(Service::Deezer));
}

#[test]
fn smart_tracklist_heading_rejects_generic_server_titles() {
    assert_eq!(
        smart_tracklist_heading_title("Electro Dance", "daily"),
        "Electro Dance"
    );
    assert_eq!(
        smart_tracklist_heading_title("Electro Dance", "daily 2/3"),
        "Electro Dance"
    );
    assert_eq!(
        smart_tracklist_heading_title("Electro Dance", "Daily 1 - 08/09/26"),
        "Electro Dance"
    );
    assert_eq!(
        smart_tracklist_heading_title("Riddim Dubstep", "Mix"),
        "Riddim Dubstep"
    );
    assert_eq!(
        smart_tracklist_heading_title("Riddim Dubstep", "Flow"),
        "Riddim Dubstep"
    );
    assert_eq!(
        smart_tracklist_heading_title("Riddim Dubstep", "Daily Mix"),
        "Riddim Dubstep"
    );
    assert_eq!(
        smart_tracklist_heading_title("daily", "Electro Dance"),
        "Electro Dance"
    );
    assert_eq!(smart_tracklist_heading_title("daily", "Daily 1"), "Mix");
    assert_eq!(
        smart_tracklist_heading_title("IL MEGLIO DEL MIO AGOSTO", "Nuove Uscite"),
        "Nuove Uscite"
    );
}

#[test]
fn smart_tracklist_route_fallback_does_not_claim_endpoint_provenance() {
    let page = smart_tracklist_page(
        Route {
            source: crate::search::Provider::Deezer,
            category: Category::Flow,
            action: "flowTracks".into(),
            id: "inspired-by-3".into(),
            title: "Electro Dance".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        },
        super::super::deezer_radio::DeezerSmartTracklist::default(),
    );
    assert_eq!(page.title, "Electro Dance");
    assert!(page.resolved_smart_mix_title.is_none());
}

#[test]
fn root_actions_match_reference() {
    assert_eq!(
        Category::DEEZER.map(Category::action),
        [
            "tracks",
            "albums",
            "artists",
            "playlists",
            "history",
            "flow"
        ]
    );
}

#[test]
fn search_track_conversion_preserves_album_and_collaborating_artists() {
    let artists = vec![
        crate::search::TrackArtistRef {
            id: "1".into(),
            name: "Primary Artist".into(),
        },
        crate::search::TrackArtistRef {
            id: "2".into(),
            name: "Featured Artist".into(),
        },
    ];
    let track = search_track(SearchTrack {
        id: "42".into(),
        album_id: "album-7".into(),
        album: "Album".into(),
        artists: artists.clone(),
        release_date: "2022-08-05".into(),
        ..SearchTrack::default()
    });
    assert_eq!(track.album_id, "album-7");
    assert_eq!(track.artists, artists);
    assert_eq!(track.release_date, "2022-08-05");
}

#[test]
fn flow_card_metadata_matches_reference_contract() {
    let item = json!({
        "type": "flow",
        "title": "Dance",
        "subtitle": "Energetic",
        "data": { "id": "genre-danceedm" },
        "pictures": [{ "md5": "flow-art", "type": "cover" }]
    });
    let card = flow_card(&item).unwrap();
    assert_eq!(card.id, "genre-danceedm");
    assert_eq!(card.title, "Dance");
    assert_eq!(card.subtitle, "Energetic");
    assert_eq!(
        card.artwork,
        "https://e-cdns-images.dzcdn.net/images/cover/flow-art/500x500.jpg"
    );
    assert_eq!(card.source, crate::search::Provider::Deezer);
}

#[test]
fn flow_cards_drop_a_subtitle_that_mirrors_the_title() {
    let mirrored = json!({
        "type": "flow",
        "title": "Dance",
        "subtitle": "Dance",
        "data": { "id": "genre-danceedm" }
    });
    assert_eq!(flow_card(&mirrored).unwrap().subtitle, "");

    let mirrored_case_insensitive = json!({
        "type": "flow",
        "title": "Dance",
        "subtitle": "dance ",
        "data": { "id": "genre-danceedm" }
    });
    assert_eq!(flow_card(&mirrored_case_insensitive).unwrap().subtitle, "");

    let absent = json!({
        "type": "flow",
        "title": "Dance",
        "data": { "id": "genre-danceedm" }
    });
    assert_eq!(flow_card(&absent).unwrap().subtitle, FLOW_CARD_SUBTITLE);
}

#[test]
fn artist_detail_sections_match_reference_metadata() {
    let page = detail_page(DetailPage {
        route: DetailRoute {
            provider: crate::search::Provider::Deezer,
            kind: ResultType::Artists,
            id: "42".into(),
            title: "Artist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        },
        total: None,
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: None,
        tracks: Vec::new(),
        artist: Some(crate::search::ArtistPage {
            profile: SearchCard::default(),
            favorite: None,
            fans: None,
            popular_tracks: Vec::new(),
            popular_total: 5,
            similar_artists: Vec::new(),
            similar_total: 0,
            albums: Vec::new(),
            albums_total: 12,
            featured: Vec::new(),
            featured_total: 12,
            playlists: Vec::new(),
            playlists_total: 12,
        }),
        description: String::new(),
        album_info: None,
    });
    assert_eq!(page.sections[0].preview_limit, Some(5));
    assert_eq!(page.sections[0].title, "Tracks");
    assert_eq!(page.title, "Artist");
    assert_eq!(page.platform, Some(crate::library::model::Service::Deezer));
    assert_eq!(page.count_noun, "track");
    assert_eq!(page.sections[0].layout, SectionLayout::Tracks);
    assert_eq!(page.sections[1].title, "Similar Artists");
    assert_eq!(page.sections[2].title, "Albums");
    for section in &page.sections[1..] {
        assert_eq!(section.preview_limit, Some(12));
        assert_eq!(section.layout, SectionLayout::Cards);
        assert!(section.card_row);
    }
}

#[test]
fn root_pages_omit_provider_metadata_while_nested_pages_include_it() {
    assert_eq!(root_page(Category::Albums, 1).platform, None);
    assert!(!root_page(Category::Flow, 15).owns_top_level_count());
    let nested = detail_page(DetailPage {
        route: DetailRoute {
            provider: crate::search::Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        },
        total: None,
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: None,
        tracks: Vec::new(),
        artist: None,
        description: String::new(),
        album_info: None,
    });
    assert_eq!(
        nested.platform,
        Some(crate::library::model::Service::Deezer)
    );

    let soundcloud = detail_page(DetailPage {
        route: DetailRoute {
            provider: crate::search::Provider::SoundCloud,
            kind: ResultType::Artists,
            id: "42".into(),
            title: "Artist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        },
        total: None,
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: None,
        tracks: Vec::new(),
        artist: None,
        description: String::new(),
        album_info: None,
    });
    assert_eq!(
        soundcloud.platform,
        Some(crate::library::model::Service::SoundCloud)
    );
}

#[test]
fn flow_copy_is_provider_neutral_for_root_and_opened_pages() {
    for copy in [
        FLOW_CARD_SUBTITLE,
        root_copy(Service::Deezer, Category::Flow).1,
        FLOW_TRACK_DESCRIPTION,
        FLOW_EMPTY_DESCRIPTION,
    ] {
        assert!(!copy.contains("Deezer"));
        assert!(!copy.contains("SoundCloud"));
    }
    assert_eq!(
        root_page(Category::Flow, 0).description,
        root_copy(Service::Deezer, Category::Flow).1
    );
}

#[test]
fn album_route_preserves_card_badge_as_detail_release_date() {
    let route = Route {
        source: crate::search::Provider::Deezer,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "42".into(),
        title: "Album".into(),
        subtitle: "Artist".into(),
        artwork: String::new(),
        release_date: "2024".into(),
    };
    let detail = detail_route(route, ResultType::Albums);

    assert_eq!(detail.release_date, "2024");
    assert_eq!(crate::search::detail_metadata(&detail), "Artist • 2024");
}

#[test]
fn collection_detail_heading_preserves_route_metadata() {
    let page = detail_page(DetailPage {
        route: DetailRoute {
            provider: crate::search::Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: "https://example.com/album.jpg".into(),
            release_date: "2024".into(),
            service_url: String::new(),
        },
        total: Some(9),
        raw_loaded_count: 0,
        normalized_count: 0,
        authoritative_total: Some(9),
        tracks: Vec::new(),
        artist: None,
        description: String::new(),
        album_info: None,
    });

    assert_eq!(page.title, "Album");
    assert_eq!(page.description, "Artist • 2024");
    assert_eq!(page.artwork, "https://example.com/album.jpg");
    assert_eq!(page.total, 9);
    assert_eq!(page.count_noun, "track");
}

#[test]
fn root_request_payloads_match_reference() {
    let (operation, body) = root_request(Category::Tracks, "42");
    assert_eq!(operation, "favorite_song.getList");
    assert_eq!(body["user_id"], "42");
    assert_eq!(body["nb"], 10000);
    let (operation, body) = root_request(Category::Albums, "42");
    assert_eq!(operation, "deezer.pageProfile");
    assert_eq!(body["tab"], "albums");
}

fn track_value(id: &str) -> Value {
    json!({
        "SNG_ID": id,
        "SNG_TITLE": format!("Track {id}"),
        "ART_NAME": "Artist",
        "DURATION": "180"
    })
}

#[test]
fn progressive_track_requests_probe_newest_suffix_then_use_bounded_pages() {
    let first = tracks_request("42", 0, TRACKS_PREFIX_SIZE);
    assert_eq!(first["start"], 0);
    assert_eq!(first["nb"], 256);
    let continuation = tracks_request("42", 25_000 - TRACKS_PREFIX_SIZE, TRACKS_PREFIX_SIZE);
    assert_eq!(continuation["start"], 24_744);
    assert_eq!(continuation["nb"], 256);
    let continuation = tracks_request("42", 0, TRACKS_PAGE_SIZE);
    assert_eq!(continuation["start"], 0);
    assert_eq!(continuation["nb"], 10_000);
    let next = tracks_request("42", TRACKS_PAGE_SIZE - TRACKS_OVERLAP, TRACKS_PAGE_SIZE);
    assert_eq!(next["start"], 9_992);
    assert_eq!(next["nb"], 10_000);
}

#[test]
fn overlapping_track_pages_preserve_order_and_real_duplicates() {
    let first_items = vec![
        track_value("1"),
        track_value("2"),
        track_value("2"),
        track_value("3"),
        track_value("4"),
        track_value("4"),
        track_value("5"),
        track_value("6"),
    ];
    let first = TracksChunk {
        items: [first_items.clone(), vec![track_value("7")]].concat(),
        total: Some(9),
    };
    let combined = assemble_first_tracks_page(9, 9, first).unwrap();
    assert_eq!(
        track_ids(&combined),
        ["1", "2", "2", "3", "4", "4", "5", "6", "7"]
    );

    let mut items = (1..=10)
        .map(|id| track_value(&id.to_string()))
        .collect::<Vec<_>>();
    let tail = TracksChunk {
        items: (3..=12).map(|id| track_value(&id.to_string())).collect(),
        total: Some(12),
    };
    append_overlapping_tracks(&mut items, 12, 10, tail).unwrap();
    assert_eq!(
        track_ids(&items),
        (1..=12).map(|id| id.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn newest_suffix_preview_requires_exact_total_and_length() {
    let valid = TracksChunk {
        items: (5..=6).map(|id| track_value(&id.to_string())).collect(),
        total: Some(6),
    };
    assert_eq!(validate_tracks_preview(valid, 6, 2).unwrap().0.len(), 2);
    let short = TracksChunk {
        items: vec![track_value("6")],
        total: Some(6),
    };
    assert!(validate_tracks_preview(short, 6, 2).is_err());
    let changed_total = TracksChunk {
        items: (5..=6).map(|id| track_value(&id.to_string())).collect(),
        total: Some(7),
    };
    assert!(validate_tracks_preview(changed_total, 6, 2).is_err());
}

#[test]
fn overlapping_track_pages_reject_a_changed_total_or_boundary() {
    let first_items = (1..=9)
        .map(|id| track_value(&id.to_string()))
        .collect::<Vec<_>>();
    let changed_total = TracksChunk {
        items: (1..=9).map(|id| track_value(&id.to_string())).collect(),
        total: Some(10),
    };
    assert!(matches!(
        assemble_first_tracks_page(9, 9, changed_total),
        Err(TracksAssemblyError::Inconsistent(_))
    ));

    let mut items = (1..=10)
        .map(|id| track_value(&id.to_string()))
        .collect::<Vec<_>>();
    let changed_overlap = TracksChunk {
        items: (4..=13).map(|id| track_value(&id.to_string())).collect(),
        total: Some(13),
    };
    assert!(matches!(
        append_overlapping_tracks(&mut items, 13, 10, changed_overlap),
        Err(TracksAssemblyError::Inconsistent(_))
    ));
    assert_eq!(
        track_ids(&first_items),
        (1..=9).map(|id| id.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn overlapping_track_pages_reject_an_incomplete_tail() {
    let prefix = (1..=8)
        .map(|id| track_value(&id.to_string()))
        .collect::<Vec<_>>();
    let short_tail = TracksChunk {
        items: prefix.clone(),
        total: Some(9),
    };
    assert!(matches!(
        append_overlapping_tracks(&mut prefix.clone(), 9, 9, short_tail),
        Err(TracksAssemblyError::Inconsistent(_))
    ));
}

#[test]
fn progressive_pages_report_raw_normalized_and_authoritative_counts() {
    let items = vec![track_value("1"), track_value("2")];
    let page = normalized_root_page(Category::Tracks, 2_592, &items);
    assert_eq!(page.total, 2_592);
    assert_eq!(page.raw_loaded_count, 2);
    assert_eq!(page.normalized_count, 2);
    assert_eq!(page.authoritative_total, Some(2_592));
    assert!(!page.playlist_removal_proven());
}

#[test]
fn only_tracks_are_normalized_in_reverse_provider_order() {
    let items = vec![track_value("1"), track_value("2")];
    let tracks = normalized_root_page(Category::Tracks, 2, &items);
    let history = normalized_root_page(Category::History, 2, &items);

    assert_eq!(
        tracks
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["2", "1"]
    );
    assert_eq!(
        history
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2"]
    );
}

#[test]
fn newest_suffix_and_full_page_share_the_published_prefix() {
    let raw = (1..=6)
        .map(|id| track_value(&id.to_string()))
        .collect::<Vec<_>>();
    let suffix = raw[3..].to_vec();
    let preview = normalized_root_page(Category::Tracks, raw.len(), &suffix);
    let full = normalized_root_page(Category::Tracks, raw.len(), &raw);

    assert_eq!(
        preview
            .tracks
            .iter()
            .map(|track| track.id.as_str())
            .collect::<Vec<_>>(),
        ["6", "5", "4"]
    );
    assert_eq!(full.tracks[..preview.tracks.len()], preview.tracks[..]);
}

#[test]
fn full_page_suffix_validation_preserves_duplicates() {
    let items = [
        track_value("1"),
        track_value("2"),
        track_value("2"),
        track_value("3"),
    ];
    assert!(validate_tracks_suffix(&items, &items[1..]).is_ok());
    assert!(validate_tracks_suffix(&items, &[track_value("2"), track_value("3")]).is_ok());
    assert!(validate_tracks_suffix(&items, &[track_value("3"), track_value("2")]).is_err());
}

#[test]
fn progressive_tracks_require_an_authoritative_total() {
    let missing = TracksChunk {
        items: vec![track_value("1")],
        total: None,
    };
    assert!(required_tracks_total(&missing).is_err());
    let impossible = TracksChunk {
        items: vec![track_value("1"), track_value("2")],
        total: Some(1),
    };
    assert!(required_tracks_total(&impossible).is_err());
}

#[test]
fn nonempty_deezer_error_envelopes_are_rejected() {
    assert!(envelope_results(json!({"error": "expired", "results": {}}), "root").is_err());
    assert!(envelope_results(json!({"error": {"code": 1}, "results": {}}), "root").is_err());
    assert!(envelope_results(json!({"error": false, "results": {}}), "root").is_ok());
}

#[test]
fn anonymous_bootstrap_is_rejected_even_with_a_saved_user_id() {
    assert_eq!(valid_user_id("42"), Some("42".into()));
    assert_eq!(valid_user_id("0"), None);
    assert_eq!(valid_user_id(""), None);
    assert_eq!(valid_user_id("abc"), None);
    assert_eq!(
        bootstrap_user_id(Some("42".into()), Some("42")),
        Ok("42".into())
    );
    assert_eq!(
        bootstrap_user_id(Some("42".into()), Some("7")),
        Err("Deezer account changed while the library was loading".into())
    );
    assert_eq!(bootstrap_user_id(Some("42".into()), None), Ok("42".into()));
    // An anonymous bootstrap reaches here with USER_ID "0" filtered to
    // None, and the saved id must not stand in for it.
    assert_eq!(
        bootstrap_user_id(None, Some("42")),
        Err(DEEZER_SESSION_EXPIRED.into())
    );
    assert_eq!(
        bootstrap_user_id(None, None),
        Err(DEEZER_SESSION_EXPIRED.into())
    );
}

#[test]
fn deezer_library_bootstrap_matches_get_contract() {
    let arl = DeezerArl::from_saved("sentinel").unwrap();
    let request = library_session_request(&Client::new(), arl.cookie_header().unwrap())
        .build()
        .unwrap();
    assert_eq!(request.method(), reqwest::Method::GET);
    assert_eq!(request.url().as_str(), DEEZER_USER_DATA_URL);
    assert_eq!(request.headers()[header::COOKIE], "arl=sentinel");
    assert!(!request.headers().contains_key(header::CONTENT_LENGTH));
    assert!(request.body().is_none());
}

#[test]
fn bootstrap_cache_keys_are_isolated_by_credential_and_saved_user() {
    let first = BootstrapCacheKey {
        arl: "first".into(),
        saved_user_id: Some("1".into()),
    };
    let same = BootstrapCacheKey {
        arl: "first".into(),
        saved_user_id: Some("1".into()),
    };
    let other_user = BootstrapCacheKey {
        arl: "first".into(),
        saved_user_id: Some("2".into()),
    };
    let other_credential = BootstrapCacheKey {
        arl: "second".into(),
        saved_user_id: Some("1".into()),
    };
    assert!(first == same);
    assert!(first != other_user);
    assert!(first != other_credential);
}

#[test]
fn track_favorite_request_matches_captured_contract() {
    let (operation, body) = favorite_request(
        super::super::favorite_state::FavoriteKind::Track,
        "3196513721",
        true,
    );
    assert_eq!(operation, "favorite_song.add");
    assert_eq!(body, json!({ "SNG_ID": "3196513721" }));

    let (operation, body) = favorite_request(
        super::super::favorite_state::FavoriteKind::Track,
        "3196513721",
        false,
    );
    assert_eq!(operation, "favorite_song.remove");
    assert_eq!(body, json!({ "SNG_ID": "3196513721" }));
}

#[test]
fn hydration_merge_preserves_order_context_and_duplicate_rows() {
    let source = vec![
        json!({ "SNG_ID": "2", "SNG_TITLE": "old", "TS": 10 }),
        json!({ "SNG_ID": "1", "DATE_ADD": "saved" }),
        json!({ "SNG_ID": "2", "TS": 20 }),
        json!({ "SNG_TITLE": "without id" }),
    ];
    let hydrated = vec![
        json!({ "SNG_ID": "1", "SNG_TITLE": "one", "TRACK_TOKEN": "a" }),
        json!({ "SNG_ID": "2", "SNG_TITLE": "two", "TRACK_TOKEN": "b" }),
    ];

    let merged = merge_hydrated_tracks(source, hydrated);

    assert_eq!(merged.len(), 4);
    assert_eq!(merged[0]["SNG_ID"], "2");
    assert_eq!(merged[1]["SNG_ID"], "1");
    assert_eq!(merged[2]["SNG_ID"], "2");
    assert_eq!(merged[0]["SNG_TITLE"], "two");
    assert_eq!(merged[0]["TRACK_TOKEN"], "b");
    assert_eq!(merged[0]["TS"], 10);
    assert_eq!(merged[1]["DATE_ADD"], "saved");
    assert_eq!(merged[2]["TS"], 20);
    assert_eq!(merged[3]["SNG_TITLE"], "without id");
}

#[test]
fn hydration_merge_matches_trimmed_string_ids() {
    let merged = merge_hydrated_tracks(
        vec![json!({ "SNG_ID": " 42 ", "TS": 10 })],
        vec![json!({ "SNG_ID": "42", "TRACK_TOKEN": "token" })],
    );

    assert_eq!(merged[0]["SNG_ID"], "42");
    assert_eq!(merged[0]["TS"], 10);
    assert_eq!(merged[0]["TRACK_TOKEN"], "token");
}

#[test]
fn track_ids_trim_strings_preserve_numbers_and_ignore_whitespace() {
    assert_eq!(
        track_ids(&[
            json!({ "SNG_ID": " 42 " }),
            json!({ "SNG_ID": 7 }),
            json!({ "SNG_ID": "   " }),
            json!({ "SNG_TITLE": "missing" }),
        ]),
        vec!["42", "7"]
    );
}

#[test]
fn raw_tracks_form_a_complete_page_without_enrichment() {
    let page = normalized_root_page(
        Category::Tracks,
        0,
        &[
            json!({ "SNG_ID": "1", "SNG_TITLE": "One", "ART_NAME": "Artist" }),
            json!({ "SNG_ID": "2", "SNG_TITLE": "Two", "ART_NAME": "Artist" }),
        ],
    );

    assert_eq!(page.title, "Liked Tracks");
    assert_eq!(page.total, 2);
    assert_eq!(page.tracks.len(), 2);
    assert!(page.cards.is_empty());
    assert_eq!(page.tracks[0].id, "2");
    assert_eq!(page.tracks[1].title, "One");
}

#[test]
fn hydration_merge_handles_large_library_without_quadratic_lookup() {
    let source = (0..2500)
        .map(|id| json!({ "SNG_ID": id.to_string(), "position": id }))
        .collect::<Vec<_>>();
    let hydrated = (0..2500)
        .rev()
        .map(|id| json!({ "SNG_ID": id.to_string(), "TRACK_TOKEN": format!("token-{id}") }))
        .collect::<Vec<_>>();

    let merged = merge_hydrated_tracks(source, hydrated);

    assert_eq!(merged.len(), 2500);
    assert_eq!(merged[0]["SNG_ID"], "0");
    assert_eq!(merged[0]["position"], 0);
    assert_eq!(merged[0]["TRACK_TOKEN"], "token-0");
    assert_eq!(merged[2499]["SNG_ID"], "2499");
    assert_eq!(merged[2499]["position"], 2499);
    assert_eq!(merged[2499]["TRACK_TOKEN"], "token-2499");
}

#[test]
fn collection_favorite_requests_match_captured_contracts() {
    use super::super::favorite_state::FavoriteKind;

    assert_eq!(
        favorite_request(FavoriteKind::Album, "42", true),
        ("album.addFavorite", json!({ "ALB_ID": "42" }))
    );
    assert_eq!(
        favorite_request(FavoriteKind::Album, "42", false),
        ("album.deleteFavorite", json!({ "ALB_ID": "42" }))
    );
    assert_eq!(
        favorite_request(FavoriteKind::Artist, "42", true),
        ("artist.addFavorite", json!({ "ART_ID": "42" }))
    );
    assert_eq!(
        favorite_request(FavoriteKind::Artist, "42", false),
        ("artist.deleteFavorite", json!({ "ART_ID": "42" }))
    );
    assert_eq!(
        favorite_request(FavoriteKind::Playlist, "42", true),
        (
            "playlist.addFavorite",
            json!({ "parent_playlist_id": "42" })
        )
    );
    assert_eq!(
        favorite_request(FavoriteKind::Playlist, "42", false),
        ("playlist.deleteFavorite", json!({ "playlist_id": "42" }))
    );
}

#[test]
fn favorite_mutation_requires_boolean_true_confirmation() {
    assert!(confirm_true(Value::Bool(true), "favorite_song.add").is_ok());
    assert!(confirm_true(Value::Bool(false), "favorite_song.add").is_err());
    assert!(confirm_true(json!({}), "favorite_song.add").is_err());
}
