use super::*;
use serde_json::json;

#[test]
fn continuation_validation_matches_api_contract() {
    let valid = json!({"next_href":"https://api-v2.soundcloud.com/users/42/track_likes?offset=cursor%3Aone"});
    assert_eq!(
        next_offset(&valid, "/users/42/track_likes").unwrap(),
        Some("cursor:one".into())
    );
    for value in [
        json!({"next_href":"https://example.invalid/users/42/track_likes?offset=x"}),
        json!({"next_href":"https://api-v2.soundcloud.com/users/43/track_likes?offset=x"}),
        json!({"next_href":42}),
    ] {
        assert!(next_offset(&value, "/users/42/track_likes").is_err());
    }
    assert_eq!(next_offset(&json!({"next_href":null}), "/x").unwrap(), None);
    let captured = json!({
        "next_href": "https://api-v2.soundcloud.com/me/library/all?offset=2026-08-31T13%3A10%3A07.558Z%2Cuser-playlist-likes%2C000-7&limit=200"
    });
    assert_eq!(
        next_offset(&captured, "/me/library/all").unwrap(),
        Some("2026-08-31T13:10:07.558Z,user-playlist-likes,000-7".into())
    );
}

#[test]
fn captured_library_items_split_albums_from_playlists() {
    let raw = vec![
        json!({
            "type": "playlist-like",
            "playlist": {
                "id": 11,
                "title": "Album",
                "is_album": true,
                "release_date": "2020-09-25T00:00:00Z",
                "track_count": 7,
                "permalink_url": "https://soundcloud.com/artist/sets/album",
                "artwork_url": "https://i1.sndcdn.com/album-large.jpg",
                "user": {"username": "Artist"}
            }
        }),
        json!({
            "type": "playlist-like",
            "playlist": {
                "id": 22,
                "title": "Playlist",
                "is_album": false,
                "track_count": 23,
                "permalink_url": "https://soundcloud.com/owner/sets/playlist",
                "user": {
                    "username": "Owner",
                    "avatar_url": "https://i1.sndcdn.com/owner-large.jpg"
                }
            }
        }),
        json!({"type": "track-like", "track": {"id": 33}}),
    ];

    let albums = library_playlists(raw.clone(), Category::Albums).unwrap();
    let playlists = library_playlists(raw, Category::Playlists).unwrap();
    assert_eq!(albums.len(), 1);
    assert_eq!(playlists.len(), 1);

    let album = playlist_card(Category::Albums, &albums[0]);
    assert_eq!(
        (
            album.id.as_str(),
            album.title.as_str(),
            album.subtitle.as_str()
        ),
        ("11", "Album", "Artist")
    );
    assert_eq!(album.badge, "2020");
    assert_eq!(album.release_date, "2020-09-25T00:00:00Z");
    assert_eq!(album.artwork, "https://i1.sndcdn.com/album-t500x500.jpg");
    assert_eq!(
        album.service_url,
        "https://soundcloud.com/artist/sets/album"
    );

    let playlist = playlist_card(Category::Playlists, &playlists[0]);
    assert_eq!(playlist.id, "22");
    assert_eq!(playlist.badge, "23");
    assert_eq!(playlist.artwork, "https://i1.sndcdn.com/owner-t500x500.jpg");
    assert_eq!(
        playlist.service_url,
        "https://soundcloud.com/owner/sets/playlist"
    );
    assert_eq!(album.is_private, None);
    assert_eq!(playlist.is_private, None);
}

#[test]
fn soundcloud_album_card_to_route_keeps_full_date_for_shared_metadata() {
    let card = playlist_card(
        Category::Albums,
        &json!({
            "id": 11,
            "title": "Album",
            "release_date": "2020-09-25T00:00:00Z",
            "user": {"username": "Artist"}
        }),
    );
    assert_eq!(card.badge, "2020");
    assert_eq!(card.release_date, "2020-09-25T00:00:00Z");
    let route = super::super::view::card_route(card).expect("album route");
    let detail = DetailRoute {
        provider: route.source,
        kind: ResultType::Albums,
        id: route.id,
        title: route.title,
        subtitle: route.subtitle,
        artwork: route.artwork,
        release_date: route.release_date,
        service_url: String::new(),
    };
    assert_eq!(
        crate::search::detail_metadata(&detail),
        "Artist • Sep 25, 2020"
    );
}

#[test]
fn soundcloud_page_conversion_retains_description_for_albums_and_playlists() {
    let album_route = super::super::model::Route {
        source: Provider::SoundCloud,
        category: Category::Albums,
        action: "albumTracks".into(),
        id: "11".into(),
        title: "Album".into(),
        subtitle: "Artist".into(),
        artwork: String::new(),
        release_date: "2020".into(),
    };
    let playlist = json!({
        "title": "Album",
        "description": "Description",
        "display_date": "2020-09-25T00:00:00Z",
        "permalink_url": "https://soundcloud.com/artist/sets/album",
        "user": {"username": "Artist"}
    });
    let tracks = [json!({
        "id": 7,
        "title": "Track",
        "duration": 1000,
        "user": {"username": "Artist"}
    })];
    let page = playlist_page(&album_route, &playlist, &tracks, 1);
    // Album tracks inherit the album identity so menus away from the
    // album page (the player bar, the queue) resolve the album.
    assert_eq!(page.tracks[0].album_id, "11");
    assert_eq!(page.tracks[0].album, "Album");
    let release_date = page
        .album_info
        .as_ref()
        .map(|info| info.release_date.as_str());
    assert_eq!(release_date, Some("2020-09-25T00:00:00Z"));
    let detail = DetailRoute {
        provider: album_route.source,
        kind: ResultType::Albums,
        id: album_route.id.clone(),
        title: page.title.clone(),
        subtitle: page.subtitle.clone(),
        artwork: page.artwork.clone(),
        release_date: page
            .album_info
            .as_ref()
            .map(|info| info.release_date.clone())
            .unwrap_or(album_route.release_date.clone()),
        service_url: page.service_url.clone(),
    };
    assert_eq!(
        crate::search::detail_metadata(&detail),
        "Artist • Sep 25, 2020"
    );

    let playlist_route = super::super::model::Route {
        category: Category::Playlists,
        action: "playlistTracks".into(),
        ..album_route
    };
    let playlist_detail = playlist_page(&playlist_route, &playlist, &tracks, 1);
    // Playlist pages never stamp a collection identity onto tracks.
    assert_eq!(playlist_detail.tracks[0].album_id, "");
    // The page keeps the canonical permalink for the detail more menu.
    assert_eq!(page.service_url, "https://soundcloud.com/artist/sets/album");
    assert_eq!(
        playlist_detail.service_url,
        "https://soundcloud.com/artist/sets/album"
    );
    assert_eq!(playlist_detail.description, "Description");
    assert_eq!(
        playlist_detail
            .album_info
            .as_ref()
            .map(|info| info.description.as_str()),
        Some("Description")
    );
}

#[test]
fn soundcloud_playlist_cards_report_sharing_privacy_metadata() {
    let private = playlist_card(
        Category::Playlists,
        &json!({"id": 7, "title": "Private", "sharing": "private"}),
    );
    assert_eq!(private.is_private, Some(true));

    let public = playlist_card(
        Category::Playlists,
        &json!({"id": 8, "title": "Public", "public": true}),
    );
    assert_eq!(public.is_private, Some(false));

    let album = playlist_card(
        Category::Albums,
        &json!({"id": 9, "title": "Album", "public": false}),
    );
    assert_eq!(album.is_private, None);
}

#[test]
fn soundcloud_playlists_merge_created_before_saved_and_deduplicate_valid_ids() {
    let merged = merge_soundcloud_playlists(
        vec![
            json!({"id": 7, "title": "Created"}),
            json!({"urn": "soundcloud:playlists:8", "title": "Created 2"}),
            json!({"id": "invalid", "title": "Invalid"}),
        ],
        vec![
            json!({"id": 8, "title": "Saved duplicate"}),
            json!({"id": 9, "title": "Saved"}),
            json!({"id": 10, "is_album": true, "title": "Album"}),
            json!({"title": "Missing ID"}),
        ],
    );
    assert_eq!(
        merged
            .iter()
            .map(|playlist| soundcloud_playlist_id(playlist).unwrap())
            .collect::<Vec<_>>(),
        vec!["7", "8", "9"]
    );
    assert_eq!(
        merged[0].get("title").and_then(Value::as_str),
        Some("Created")
    );
    assert_eq!(
        merged[1].get("title").and_then(Value::as_str),
        Some("Created 2")
    );
}

#[test]
fn captured_artist_follow_and_unfollow_requests_match_mobile_api() {
    let key = super::super::favorite_state::FavoriteKey::soundcloud(
        super::super::favorite_state::FavoriteKind::Artist,
        "619944075".into(),
    );
    let follow = soundcloud_favorite_request(&key, true).unwrap();
    assert_eq!(follow.method, Method::POST);
    assert_eq!(
        follow.url,
        "https://api-mobile.soundcloud.com/follows/users/soundcloud:users:619944075"
    );
    assert_eq!(follow.body, None);
    assert_eq!(
        follow.content_type,
        "application/x-www-form-urlencoded; charset=UTF-8"
    );

    let unfollow = soundcloud_favorite_request(&key, false).unwrap();
    assert_eq!(unfollow.method, Method::DELETE);
    assert_eq!(unfollow.url, follow.url);
    assert_eq!(unfollow.body, None);
}

#[test]
fn captured_album_and_playlist_like_requests_share_playlist_urn_contract() {
    for kind in [
        super::super::favorite_state::FavoriteKind::Album,
        super::super::favorite_state::FavoriteKind::Playlist,
    ] {
        let key = super::super::favorite_state::FavoriteKey::soundcloud(kind, "951165127".into());
        let favorite = soundcloud_favorite_request(&key, true).unwrap();
        assert_eq!(favorite.method, Method::POST);
        assert_eq!(
            favorite.url,
            "https://api-mobile.soundcloud.com/likes/playlists/create"
        );
        assert_eq!(favorite.content_type, "application/json; charset=UTF-8");
        assert_eq!(
            favorite.body,
            Some(json!({
                "likes": [{"target_urn": "soundcloud:playlists:951165127"}]
            }))
        );

        let unfavorite = soundcloud_favorite_request(&key, false).unwrap();
        assert_eq!(
            unfavorite.url,
            "https://api-mobile.soundcloud.com/likes/playlists/delete"
        );
        assert_eq!(unfavorite.body, favorite.body);
    }
}

#[test]
fn soundcloud_track_favorite_requests_use_the_mobile_bulk_like_contract() {
    let key = super::super::favorite_state::FavoriteKey::soundcloud(
        super::super::favorite_state::FavoriteKind::Track,
        "2332245164".into(),
    );
    let favorite = soundcloud_favorite_request(&key, true).unwrap();
    assert_eq!(favorite.method, Method::POST);
    assert_eq!(
        favorite.url,
        "https://api-mobile.soundcloud.com/likes/tracks/create"
    );
    assert_eq!(favorite.content_type, "application/json; charset=UTF-8");
    assert_eq!(
        favorite.body,
        Some(json!({
            "likes": [{"target_urn": "soundcloud:tracks:2332245164"}]
        }))
    );

    let unfavorite = soundcloud_favorite_request(&key, false).unwrap();
    assert_eq!(unfavorite.method, Method::POST);
    assert_eq!(
        unfavorite.url,
        "https://api-mobile.soundcloud.com/likes/tracks/delete"
    );
    assert_eq!(unfavorite.body, favorite.body);
}

#[test]
fn soundcloud_mobile_favorite_client_matches_captured_identity_headers() {
    let headers = soundcloud_mobile_headers();
    assert_eq!(headers.get(header::USER_AGENT).unwrap(), MOBILE_USER_AGENT);
    assert_eq!(headers.get(header::ACCEPT).unwrap(), "*/*");
    assert_eq!(
        headers.get(header::ACCEPT_ENCODING).unwrap(),
        MOBILE_ACCEPT_ENCODING
    );
}

#[test]
fn captured_soundcloud_cookies_seed_mobile_jar_without_overwriting_refreshes() {
    use reqwest::cookie::CookieStore as _;

    let client = SoundCloudLibraryClient::new().unwrap();
    let source = "oauth_token=desktop-token; datadome=initial-guard";
    client.seed_mobile_cookies(source).unwrap();
    let url = Url::parse(MOBILE_API).unwrap();
    let seeded = client.mobile_cookies.cookies(&url).unwrap();
    let seeded = seeded.to_str().unwrap();
    assert!(seeded.contains("oauth_token=desktop-token"));
    assert!(seeded.contains("datadome=initial-guard"));

    client.mobile_cookies.add_cookie_str(
        "datadome=refreshed-guard; Domain=.soundcloud.com; Path=/; Secure",
        &url,
    );
    client.seed_mobile_cookies(source).unwrap();
    let refreshed = client.mobile_cookies.cookies(&url).unwrap();
    let refreshed = refreshed.to_str().unwrap();
    assert!(refreshed.contains("datadome=refreshed-guard"));
    assert!(!refreshed.contains("datadome=initial-guard"));
}

#[test]
fn malformed_captured_playlist_like_items_fail_closed() {
    assert!(library_playlists(vec![json!({"type": "playlist-like"})], Category::Albums).is_err());
    assert!(
        library_playlists(
            vec![json!({"type": "playlist-like", "playlist": {"id": 7}})],
            Category::Playlists
        )
        .is_err()
    );
}

#[test]
fn normalization_matches_soundcloud_search_contract() {
    let value = json!({"urn":"soundcloud:tracks:9","title":"Uploader - Actual title","duration":61000,"artwork_url":"https://example.com/image-large.jpg","user":{"username":"Other"}});
    let normalized = track(&value);
    assert_eq!(
        (
            normalized.id.as_str(),
            normalized.title.as_str(),
            normalized.artist.as_str()
        ),
        ("9", "Actual title", "Uploader")
    );
    assert_eq!(normalized.duration, 61);
    assert_eq!(normalized.artwork, "https://example.com/image-t500x500.jpg");
}

#[test]
fn track_keeps_credited_artist_and_canonical_uploader_ref() {
    let normalized = track(&json!({
        "id": "7004",
        "title": "Synthetic Artist - Synthetic Song",
        "publisher_metadata": {"artist": "Synthetic Artist"},
        "user": {
            "id": "8002",
            "username": "synthetic-uploader",
            "full_name": "Synthetic Uploader"
        }
    }));

    assert_eq!(normalized.artist, "Synthetic Artist");
    assert_eq!(
        normalized.artists,
        vec![crate::search::TrackArtistRef {
            id: "8002".into(),
            name: "synthetic-uploader".into()
        }]
    );
}

#[test]
fn library_normalization_populates_soundcloud_service_urls() {
    let normalized = track(&json!({
        "id": 7,
        "title": "Artist - Song",
        "permalink_url": "https://soundcloud.com/artist/song"
    }));
    assert_eq!(normalized.service_url, "https://soundcloud.com/artist/song");

    let subdomain = track(&json!({
        "id": 8,
        "title": "Track",
        "permalink_url": "https://m.soundcloud.com/artist/track"
    }));
    assert_eq!(
        subdomain.service_url,
        "https://m.soundcloud.com/artist/track"
    );

    let foreign = track(&json!({
        "id": 9,
        "title": "Track",
        "permalink_url": "https://evil.test/soundcloud.com"
    }));
    assert_eq!(foreign.service_url, "");

    let missing = track(&json!({"id": 10, "title": "Track"}));
    assert_eq!(missing.service_url, "");
}

#[test]
fn track_normalization_preserves_explicit_metadata() {
    assert!(
        track(&json!({
            "id": 7,
            "title": "Artist - Song",
            "publisher_metadata": { "explicit": true }
        }))
        .explicit
    );
}

#[test]
fn soundcloud_root_copy_matches_the_original_routes() {
    assert_eq!(
        root_copy(Service::SoundCloud, Category::MyTracks),
        ("My Tracks", "Tracks uploaded on your SoundCloud account.")
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::Tracks),
        ("Liked Tracks", "Tracks you liked on SoundCloud.")
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::Artists),
        ("Followed Artists", "Artists you followed on SoundCloud.")
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::History),
        (
            "Track History",
            "Your most recently played SoundCloud tracks."
        )
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::Station),
        (
            "Station",
            "Start a continuous mix from one of your liked tracks."
        )
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::Albums),
        ("Albums", "Albums you liked on SoundCloud.")
    );
    assert_eq!(
        root_copy(Service::SoundCloud, Category::Playlists),
        (
            "Playlists",
            "Your playlist you saved or created on SoundCloud."
        )
    );
}

#[test]
fn artist_page_uses_canonical_profile_metadata_and_preserves_track_credit() {
    let profile = json!({
        "id": "8002",
        "username": "synthetic-uploader",
        "full_name": "Synthetic Uploader",
        "description": "Canonical profile",
        "track_count": 17,
        "followers_count": 12345,
        "avatar_url": "https://example.com/synthetic-uploader-large.jpg"
    });
    let raw = vec![json!({
        "id": "7004",
        "title": "Synthetic Artist - Synthetic Song",
        "publisher_metadata": {"artist": "Synthetic Artist"},
        "user": {
            "id": "8002",
            "username": "synthetic-uploader",
            "full_name": "Synthetic Uploader"
        }
    })];

    let page = artist_page(&profile, &raw);

    assert_eq!(page.title, "synthetic-uploader");
    assert_eq!(page.subtitle, "12,345 followers");
    assert_eq!(page.description, "Canonical profile");
    assert_eq!(
        page.artwork,
        "https://example.com/synthetic-uploader-t500x500.jpg"
    );
    assert!(page.show_count);
    assert_eq!(page.count_noun, "track");
    assert_eq!(page.total, 1);
    assert_eq!(page.platform, Some(Service::SoundCloud));
    assert_eq!(page.tracks[0].artist, "Synthetic Artist");
    assert_eq!(
        page.tracks[0].artists,
        vec![crate::search::TrackArtistRef {
            id: "8002".into(),
            name: "synthetic-uploader".into()
        }]
    );
}

#[test]
fn nested_soundcloud_detail_pages_show_the_provider_heading_marker() {
    assert_eq!(detail_platform("albumTracks"), Some(Service::SoundCloud));
    assert_eq!(detail_platform("playlistTracks"), Some(Service::SoundCloud));
    assert_eq!(detail_platform("artistTracks"), Some(Service::SoundCloud));
    assert_eq!(detail_platform("stationTracks"), Some(Service::SoundCloud));
    assert_eq!(detail_platform("artists"), None);
    assert_eq!(detail_platform("station"), None);
}

#[test]
fn station_route_uses_the_captured_numeric_endpoint() {
    assert_eq!(
        station_url("42").unwrap(),
        "https://api-v2.soundcloud.com/stations/soundcloud:track-stations:42/tracks"
    );
    for invalid in ["", "42/other", "soundcloud:tracks:42"] {
        assert!(station_url(invalid).is_err());
    }
}

#[test]
fn artist_station_route_uses_the_captured_numeric_endpoint() {
    assert_eq!(
        artist_station_url("604200750").unwrap(),
        "https://api-v2.soundcloud.com/stations/soundcloud:artist-stations:604200750/tracks"
    );
    for invalid in ["", "0", "artist", "42/next"] {
        assert!(artist_station_url(invalid).is_err());
    }
}

#[test]
fn station_reads_use_the_cookie_free_mobile_contract() {
    let client = SoundCloudLibraryClient::new().unwrap();
    let authorization = SoundCloudToken::from_saved("mobile-sentinel")
        .unwrap()
        .authorization_header()
        .unwrap();
    let request = client
        .station_request(station_url("42").unwrap(), authorization)
        .build()
        .unwrap();
    assert_eq!(request.headers()[header::USER_AGENT], MOBILE_USER_AGENT);
    assert_eq!(request.headers()[header::ACCEPT], "*/*");
    assert_eq!(
        request.headers()[header::ACCEPT_ENCODING],
        MOBILE_ACCEPT_ENCODING
    );
    assert_eq!(
        request.headers()[header::AUTHORIZATION],
        "OAuth mobile-sentinel"
    );
    assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
    assert!(!request.headers().contains_key(header::COOKIE));
    assert_eq!(
        request
            .url()
            .query_pairs()
            .find(|(key, _)| key == "client_id")
            .map(|(_, value)| value.into_owned()),
        Some(SOUNDCLOUD_CLIENT_ID.into())
    );
}

#[test]
fn station_page_uses_human_context_without_a_finite_count() {
    let raw = (0..50)
        .map(|id| json!({"id": id, "title": format!("Track {id}")}))
        .collect::<Vec<_>>();

    let page = station_page(
        "Seed station".into(),
        "Start from Artist".into(),
        "https://example.com/station.jpg".into(),
        &raw,
    );

    assert_eq!(page.title, "Seed station");
    assert_eq!(page.subtitle, "Start from Artist");
    assert!(page.description.is_empty());
    assert_eq!(page.artwork, "https://example.com/station.jpg");
    assert_eq!(page.platform, Some(Service::SoundCloud));
    assert!(!page.show_count);
    assert_eq!(page.total, 50);
    assert_eq!(page.tracks.len(), 50);
}

#[test]
fn artist_route_uses_canonical_profile_and_tracks_endpoints() {
    assert_eq!(
        artist_endpoints("42").unwrap(),
        (
            "https://api-v2.soundcloud.com/users/42".into(),
            "https://api-v2.soundcloud.com/users/42/tracks".into()
        )
    );
    for invalid in ["", "42/other", "soundcloud:users:42"] {
        assert!(artist_endpoints(invalid).is_err());
    }
}

#[test]
fn playlist_routes_validate_ids_and_preserve_stub_order() {
    assert_eq!(
        numeric_id("soundcloud:playlists:42", "soundcloud:playlists:"),
        Some("42".into())
    );
    for invalid in ["", "42/other", "soundcloud:users:42"] {
        assert!(numeric_id(invalid, "soundcloud:playlists:").is_none());
    }
    assert_eq!(
        playlist_track_ids(&json!({
            "tracks": [{"id": 42}, {"urn": "soundcloud:tracks:7"}]
        }))
        .unwrap(),
        vec!["42", "7"]
    );
    assert_eq!(
        playlist_ids_for_update(&json!({
            "tracks": [7001u64, {"id": 7002}]
        }))
        .unwrap(),
        vec!["7001", "7002"]
    );
}

#[test]
fn truncated_playlist_track_metadata_requires_full_track_collection() {
    assert_eq!(playlist_track_count(&json!({"track_count": 3})), Some(3));
    assert_eq!(playlist_track_count(&json!({"track_count": "3"})), Some(3));
    assert_eq!(playlist_track_count(&json!({"track_count": -1})), None);
    assert_eq!(playlist_track_count(&json!({})), None);
    assert!(playlist_tracks_are_truncated(&json!({
        "track_count": 3,
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert!(!playlist_tracks_are_truncated(&json!({
        "track_count": 2,
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert!(!playlist_tracks_are_truncated(&json!({
        "track_count": 1,
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert!(playlist_tracks_are_truncated(&json!({
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert!(playlist_tracks_are_truncated(&json!({
        "track_count": "unknown",
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert!(playlist_tracks_are_truncated(&json!({
        "track_count": -1,
        "tracks": [{"id": 1}, {"id": 2}]
    })));
    assert_eq!(
        playlist_ids_from_collection(&[
            json!({"track": {"id": 1}}),
            json!({"track": {"urn": "soundcloud:tracks:2"}}),
            json!({"id": 3}),
        ])
        .unwrap(),
        vec!["1", "2", "3"]
    );
}

#[test]
fn captured_create_public_playlist_request_matches_api_contract() {
    let write = create_playlist_write(
        "synthetic public playlist",
        "synthetic public description",
        false,
        &["7001".into()],
    )
    .unwrap();
    let url = Url::parse(&write.url).unwrap();
    assert_eq!(write.method, Method::POST);
    assert_eq!(url.path(), "/playlists");
    assert_eq!(
        write.body,
        json!({
            "playlist": {
                "title": "synthetic public playlist",
                "description": "synthetic public description",
                "sharing": "public",
                "tracks": [7001u64]
            }
        })
    );
    assert!(
        write
            .body
            .pointer("/playlist/tracks/0")
            .unwrap()
            .is_number()
    );
    assert!(write.body.get("_resource_id").is_none());
    assert!(write.body.get("_resource_type").is_none());
    assert!(write.body.pointer("/playlist/_resource_id").is_none());
    assert_eq!(
        created_playlist_id(&json!({
            "id": 9001u64,
            "kind": "playlist",
            "sharing": "public",
            "public": true,
            "secret_token": null,
            "title": "synthetic public playlist",
            "permalink_url": "https://example.test/soundcloud/sets/synthetic-public-playlist"
        }))
        .unwrap(),
        "9001"
    );
}

#[test]
fn captured_create_private_playlist_request_matches_api_contract() {
    let write = create_playlist_write(
        "synthetic private playlist",
        "synthetic private description",
        true,
        &["7003".into()],
    )
    .unwrap();
    let url = Url::parse(&write.url).unwrap();
    assert_eq!(write.method, Method::POST);
    assert_eq!(url.path(), "/playlists");
    assert_eq!(
        write
            .body
            .pointer("/playlist/sharing")
            .and_then(Value::as_str),
        Some("private")
    );
    assert_eq!(
        write
            .body
            .pointer("/playlist/description")
            .and_then(Value::as_str),
        Some("synthetic private description")
    );
    assert_eq!(
        write.body.pointer("/playlist/tracks"),
        Some(&json!([7003u64]))
    );
    assert!(write.body.get("_resource_id").is_none());
    assert_eq!(
        created_playlist_id(&json!({
            "id": "9002",
            "kind": "playlist",
            "sharing": "private",
            "public": false,
            "secret_token": "synthetic-private-secret-token",
            "title": "synthetic private playlist",
            "permalink_url": "https://example.test/soundcloud/sets/synthetic-private-playlist"
        }))
        .unwrap(),
        "9002"
    );
}

#[test]
fn soundcloud_create_and_update_enforce_unicode_text_boundaries() {
    let title = "é".repeat(SOUNDCLOUD_TITLE_MAX_CHARS);
    let description = "🎵".repeat(SOUNDCLOUD_DESCRIPTION_MAX_CHARS);
    assert!(create_playlist_write(&title, &description, false, &[]).is_ok());
    assert!(update_playlist_write("9004", &title, &description, false).is_ok());
    assert!(create_playlist_write(&format!("{title}é"), "", false, &[],).is_err());
    assert!(update_playlist_write("9004", "title", &format!("{description}🎵"), false,).is_err());
    assert!(create_playlist_write("title\u{0000}", "", false, &[]).is_err());
    assert!(update_playlist_write("9004", "title", "line\u{0000}", false).is_err());
}

#[test]
fn captured_update_playlist_request_is_minimal_and_maps_privacy_both_ways() {
    for (private, sharing, public) in [(false, "public", true), (true, "private", false)] {
        let write = update_playlist_write("9004", "updated", "description", private).unwrap();
        let url = Url::parse(&write.url).unwrap();
        assert_eq!(write.method, Method::PUT);
        assert_eq!(url.path(), "/playlists/9004");
        assert_eq!(
            write.body,
            json!({
                "playlist": {
                    "title": "updated",
                    "description": "description",
                    "sharing": sharing,
                    "public": public,
                }
            })
        );
        let fields = write
            .body
            .pointer("/playlist")
            .unwrap()
            .as_object()
            .unwrap();
        assert_eq!(fields.len(), 4);
        for forbidden in [
            "secret_token",
            "user",
            "tracks",
            "_resource_id",
            "_resource_type",
        ] {
            assert!(!fields.contains_key(forbidden));
        }
    }
}

#[test]
fn captured_artwork_request_uses_the_urn_path_and_raw_base64_body() {
    let write = artwork_write("9004", "synthetic-base64").unwrap();
    let url = Url::parse(&write.url).unwrap();
    assert_eq!(write.method, Method::PUT);
    assert_eq!(url.path(), "/playlists/soundcloud:playlists:9004/artwork");
    assert!(url.query().is_none());
    assert_eq!(write.body, json!({ "image_data": "synthetic-base64" }));
    assert_eq!(write.body.as_object().unwrap().len(), 1);
}

#[test]
fn updated_playlist_response_preserves_authoritative_nonmutable_fields() {
    let existing = OwnedPlaylist {
        id: "9004".into(),
        title: "old".into(),
        description: "old description".into(),
        is_private: false,
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: "8004".into(),
            name: "owner".into(),
        },
        artwork: "https://i1.sndcdn.com/old.jpg".into(),
        track_count: Some(3),
    };
    let updated = updated_playlist_from_response(
        &json!({
            "kind": "playlist",
            "id": 9004,
            "title": "new",
            "description": "new description",
            "public": false,
            "sharing": "private",
        }),
        &existing,
        "new",
        "new description",
        true,
    )
    .unwrap();
    assert_eq!(updated.id, existing.id);
    assert_eq!(updated.title, "new");
    assert_eq!(updated.description, "new description");
    assert!(updated.is_private);
    assert_eq!(updated.owner, existing.owner);
    assert_eq!(updated.track_count, existing.track_count);
    assert_eq!(updated.artwork, existing.artwork);
}

#[test]
fn updated_playlist_response_uses_submitted_mutable_fields_when_response_is_incomplete() {
    let existing = OwnedPlaylist {
        id: "9004".into(),
        title: "old".into(),
        description: "old description".into(),
        is_private: false,
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: "8004".into(),
            name: "owner".into(),
        },
        artwork: "https://i1.sndcdn.com/old.jpg".into(),
        track_count: Some(3),
    };
    let responses = [
        json!({"kind": "playlist", "id": 9004}),
        json!({
            "kind": "playlist",
            "id": 9004,
            "title": null,
            "description": null,
            "sharing": null,
            "public": null,
        }),
        json!({
            "kind": "playlist",
            "id": 9004,
            "title": "stale title",
            "description": "stale description",
            "sharing": "public",
            "public": true,
        }),
        json!({
            "kind": "playlist",
            "id": 9004,
            "title": "",
            "description": "",
            "sharing": "",
            "public": false,
        }),
    ];
    for response in responses {
        let updated = updated_playlist_from_response(
            &response,
            &existing,
            "submitted title",
            "submitted description",
            true,
        )
        .unwrap();
        assert_eq!(updated.title, "submitted title");
        assert_eq!(updated.description, "submitted description");
        assert!(updated.is_private);
        assert_eq!(updated.owner, existing.owner);
        assert_eq!(updated.artwork, existing.artwork);
    }
}

#[test]
fn updated_playlist_response_still_rejects_wrong_identity_or_malformed_json() {
    let existing = OwnedPlaylist {
        id: "9004".into(),
        title: "old".into(),
        description: "old description".into(),
        is_private: false,
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: "8004".into(),
            name: "owner".into(),
        },
        artwork: String::new(),
        track_count: None,
    };
    for response in [
        json!({"kind": "playlist", "id": 9005}),
        json!({"kind": "track", "id": 9004}),
        json!({"kind": "playlist"}),
        Value::Null,
    ] {
        assert!(
            updated_playlist_from_response(
                &response,
                &existing,
                "submitted title",
                "submitted description",
                false,
            )
            .is_err()
        );
    }
}

#[test]
fn updated_playlist_response_keeps_existing_artwork_for_null_or_empty_urls() {
    let existing = OwnedPlaylist {
        id: "9004".into(),
        title: "old".into(),
        description: "old description".into(),
        is_private: false,
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: "8004".into(),
            name: "owner".into(),
        },
        artwork: "https://i1.sndcdn.com/old.jpg".into(),
        track_count: Some(3),
    };
    for artwork_url in [Value::Null, Value::String(String::new())] {
        let updated = updated_playlist_from_response(
            &json!({
                "kind": "playlist",
                "id": 9004,
                "title": "new",
                "description": "new description",
                "public": true,
                "artwork_url": artwork_url,
            }),
            &existing,
            "new",
            "new description",
            false,
        )
        .unwrap();
        assert_eq!(updated.artwork, existing.artwork);
    }
}

#[test]
fn artwork_response_requires_a_safe_soundcloud_https_url() {
    assert_eq!(
        artwork_url_from_response(&json!({
            "artwork_url": "https://i1.sndcdn.com/new.jpg"
        }))
        .unwrap(),
        "https://i1.sndcdn.com/new.jpg"
    );
    for url in [
        "http://i1.sndcdn.com/new.jpg",
        "https://example.invalid/new.jpg",
        "https://user:i1@sndcdn.com/new.jpg",
    ] {
        assert!(artwork_url_from_response(&json!({ "artwork_url": url })).is_err());
    }
}

#[test]
fn initial_playlist_artwork_omits_non_soundcloud_hosts() {
    assert_eq!(
        playlist_artwork(&json!({
            "artwork_url": "https://i1.sndcdn.com/valid.jpg"
        })),
        "https://i1.sndcdn.com/valid.jpg"
    );
    assert!(
        playlist_artwork(&json!({
            "artwork_url": "https://example.com/unsafe.jpg"
        }))
        .is_empty()
    );
    assert!(
        playlist_artwork(&json!({
            "artwork_url": "http://i1.sndcdn.com/unsafe.jpg"
        }))
        .is_empty()
    );
    assert_eq!(
        playlist_artwork(&json!({
            "artwork": "https://example.com/unsafe.jpg",
            "artwork_url": "https://i1.sndcdn.com/fallback.jpg"
        })),
        "https://i1.sndcdn.com/fallback.jpg"
    );
}

#[test]
fn artwork_base64_validation_requires_a_decodable_jpeg_within_limit() {
    let image = image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    let encoded = STANDARD.encode(bytes);
    assert!(validate_artwork_base64(&encoded).is_ok());
    assert!(validate_artwork_base64("not-base64").is_err());
    assert!(validate_artwork_base64("data:image/jpeg;base64,abc").is_err());
}

#[test]
fn artwork_base64_limit_is_checked_before_decoding() {
    assert!(artwork_base64_size_allowed(MAX_ARTWORK_BASE64_BYTES));
    assert!(!artwork_base64_size_allowed(MAX_ARTWORK_BASE64_BYTES + 1));
}

#[test]
fn artwork_base64_dimensions_are_checked_before_full_decode() {
    let image = image::RgbImage::from_pixel(MAX_IMAGE_DIMENSION + 1, 1, image::Rgb([1, 2, 3]));
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    let encoded = STANDARD.encode(bytes);
    assert!(
        validate_artwork_base64(&encoded)
            .unwrap_err()
            .contains("dimensions")
    );
}

#[test]
fn delete_contract_uses_a_bodyless_numeric_endpoint() {
    let id = positive_numeric_id("soundcloud:playlists:9004", "soundcloud:playlists:").unwrap();
    let url = Url::parse(&format!("{API}/playlists/{id}")).unwrap();
    assert_eq!(url.path(), "/playlists/9004");
    assert!(url.query().is_none());
    assert!(positive_numeric_id("0", "").is_none());
    assert!(positive_numeric_id("not-a-playlist", "").is_none());
    assert_eq!(
        delete_response_status(reqwest::StatusCode::NO_CONTENT),
        Ok(true)
    );
    assert_eq!(delete_response_status(reqwest::StatusCode::OK), Ok(true));
    assert!(delete_response_status(reqwest::StatusCode::BAD_REQUEST).is_err());
}

#[test]
fn empty_create_sends_an_empty_tracks_array() {
    let write = create_playlist_write("plus button", "", false, &[]).unwrap();
    assert_eq!(
        write.body,
        json!({
            "playlist": {
                "title": "plus button",
                "description": "",
                "sharing": "public",
                "tracks": []
            }
        })
    );
}

#[test]
fn playlist_create_accepts_the_five_hundred_track_boundary() {
    let track_ids = (1..=MAX_PLAYLIST_TRACKS)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    assert!(create_playlist_write("synthetic playlist", "", false, &track_ids).is_ok());
}

#[test]
fn playlist_create_rejects_more_than_five_hundred_tracks() {
    let track_ids = (1..=MAX_PLAYLIST_TRACKS + 1)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let error = create_playlist_write("synthetic playlist", "", false, &track_ids).unwrap_err();
    assert!(error.contains("at most 500 tracks"));
}

#[test]
fn captured_add_to_playlist_put_uses_full_numeric_track_list() {
    let plan = plan_add_tracks(&["7001".into()], &["7002".into()]).unwrap();
    let AddTracksPlan::Update { track_ids, result } = plan else {
        panic!("new tracks should PUT the merged list");
    };
    assert_eq!(track_ids, vec![7001, 7002]);
    assert_eq!(result, AddTracksResult::Added { count: 1 });
    let write = add_tracks_write("9001", &track_ids).unwrap();
    let url = Url::parse(&write.url).unwrap();
    assert_eq!(write.method, Method::PUT);
    assert_eq!(url.path(), "/playlists/9001");
    assert_eq!(write.body, json!({"playlist":{"tracks":[7001u64, 7002]}}));
    assert!(
        write
            .body
            .pointer("/playlist/tracks/0")
            .unwrap()
            .is_number()
    );
    assert!(
        write
            .body
            .pointer("/playlist/tracks/1")
            .unwrap()
            .is_number()
    );
    assert!(write.body.get("_resource_id").is_none());

    let partial = plan_add_tracks(&["7001".into()], &["7001".into(), "7002".into()]).unwrap();
    assert_eq!(
        partial,
        AddTracksPlan::Update {
            track_ids: vec![7001, 7002],
            result: AddTracksResult::Partial {
                added: 1,
                duplicated: 1
            }
        }
    );
}

#[test]
fn captured_reorder_playlist_put_uses_the_exact_numeric_track_order() {
    let write = reorder_playlist_write("2294000010", &[110878517, 2264206496]).unwrap();
    let url = Url::parse(&write.url).unwrap();
    assert_eq!(write.method, Method::PUT);
    assert_eq!(url.path(), "/playlists/2294000010");
    assert!(url.query().is_none());
    assert_eq!(
        write.body,
        json!({"playlist": {"tracks": [110878517u64, 2264206496u64]}})
    );
}

#[test]
fn reorder_validation_requires_a_complete_unique_positive_playlist_order() {
    assert_eq!(
        validate_reorder_ids(&["1".into(), "2".into()]).unwrap(),
        vec![1, 2]
    );
    for invalid in [
        vec!["1".into()],
        vec!["1".into(), "1".into()],
        vec!["0".into(), "2".into()],
        vec!["nope".into(), "2".into()],
    ] {
        assert!(validate_reorder_ids(&invalid).is_err());
    }
    let too_large = (1..=MAX_PLAYLIST_TRACKS + 1)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    assert!(validate_reorder_ids(&too_large).is_err());
    assert!(positive_numeric_id("0", "").is_none());
    assert_eq!(
        positive_numeric_id("soundcloud:playlists:42", "soundcloud:playlists:"),
        Some("42".into())
    );
}

#[test]
fn reordered_playlist_response_requires_the_same_playlist_kind_and_id() {
    let expected = "2294000010";
    assert!(
        validate_reordered_playlist_response(
            &json!({"id": 2294000010u64, "kind": "playlist"}),
            expected
        )
        .is_ok()
    );
    assert!(
        validate_reordered_playlist_response(
            &json!({"playlist": {"id": 2294000010u64, "kind": "playlist"}}),
            expected
        )
        .is_ok()
    );
    for response in [
        json!({"id": 7, "kind": "playlist"}),
        json!({"id": 2294000010u64, "kind": "track"}),
        json!({"id": 2294000010u64}),
        json!({"kind": "playlist"}),
    ] {
        assert!(validate_reordered_playlist_response(&response, expected).is_err());
    }
}

#[test]
fn already_present_tracks_skip_the_playlist_put() {
    let plan = plan_add_tracks(&["7001".into()], &["7001".into()]).unwrap();
    assert_eq!(plan, AddTracksPlan::AlreadyPresent { count: 1 });
    let duplicates = plan_add_tracks(
        &["10".into(), "20".into(), "30".into()],
        &["20".into(), "10".into()],
    )
    .unwrap();
    assert_eq!(duplicates, AddTracksPlan::AlreadyPresent { count: 2 });
    assert!(plan_add_tracks(&["7001".into()], &[]).is_err());
}

#[test]
fn playlist_put_rejects_more_than_five_hundred_full_track_ids() {
    let existing = (1..=500).map(|id| id.to_string()).collect::<Vec<_>>();
    assert!(plan_add_tracks(&existing, &["501".into()]).is_err());
    let requested = (1..=501).map(|id| id.to_string()).collect::<Vec<_>>();
    assert!(plan_add_tracks(&[], &requested).is_err());
}

#[test]
fn owned_playlist_mapping_distinguishes_public_from_private() {
    let raw = vec![
        json!({
            "id": 9001u64,
            "kind": "playlist",
            "title": "synthetic public playlist",
            "description": null,
            "sharing": "public",
            "public": true,
            "secret_token": null,
            "permalink_url": "https://example.test/soundcloud/sets/synthetic-public-playlist",
            "is_album": false,
            "track_count": 1,
            "artwork_url": "https://i1.sndcdn.com/public-large.jpg",
            "user": {"id": 8001, "username": "synthetic-owner"}
        }),
        json!({
            "id": 9002u64,
            "kind": "playlist",
            "title": "synthetic private playlist",
            "description": "kept",
            "sharing": "private",
            "public": false,
            "secret_token": "synthetic-private-secret-token",
            "permalink_url": "https://example.test/soundcloud/sets/synthetic-private-playlist",
            "is_album": false,
            "track_count": 1,
            "user": {"id": "8001", "username": "synthetic-owner"}
        }),
        json!({
            "id": 9003,
            "title": "Album",
            "is_album": true,
            "sharing": "public",
            "public": true,
            "user": {"id": 8001, "username": "synthetic-owner"}
        }),
    ];
    let playlists = owned_playlists_from(&raw).unwrap();
    assert_eq!(playlists.len(), 2);
    assert_eq!(playlists[0].id, "9001");
    assert_eq!(playlists[0].title, "synthetic public playlist");
    assert!(playlists[0].description.is_empty());
    assert!(!playlists[0].is_private);
    assert!(!playlists[0].is_from_favorite_tracks);
    assert!(!playlists[0].is_collaborative);
    assert_eq!(playlists[0].owner.id, "8001");
    assert_eq!(playlists[0].owner.name, "synthetic-owner");
    assert_eq!(
        playlists[0].artwork,
        "https://i1.sndcdn.com/public-t500x500.jpg"
    );
    assert_eq!(playlists[0].track_count, Some(1));
    assert_eq!(playlists[1].id, "9002");
    assert!(playlists[1].is_private);
    assert_eq!(playlists[1].description, "kept");
}

#[test]
fn invalid_playlist_and_track_ids_fail_closed() {
    assert!(create_playlist_write("   ", "", false, &[]).is_err());
    assert!(create_playlist_write("title", "", false, &["abc".into()]).is_err());
    assert!(create_playlist_write("title", "", false, &["0".into()]).is_err());
    assert!(create_playlist_write("title", "", false, &["12.5".into()]).is_err());
    assert!(add_tracks_write("not-a-number", &[7002]).is_err());
    assert!(plan_add_tracks(&["7001".into()], &["nope".into()]).is_err());
    assert!(created_playlist_id(&json!({"id": 1, "kind": "track"})).is_err());
    assert!(
        owned_playlist(&json!({
            "id": "bad",
            "title": "Broken",
            "user": {"id": 1, "username": "x"}
        }))
        .is_err()
    );
    assert!(
        owned_playlist(&json!({
            "id": 1,
            "title": "Broken",
            "user": {"id": "bad", "username": "x"}
        }))
        .is_err()
    );
}

#[cfg(feature = "live-soundcloud-tests")]
#[tokio::test]
#[ignore = "requires explicit live SoundCloud credentials"]
async fn live_mobile_playlist_transport_creates_adds_and_cleans_up() {
    let saved = std::env::var("RALGRUM_LIVE_SOUNDCLOUD_MOBILE_TOKEN")
        .expect("live SoundCloud mobile credential");
    let track_id =
        std::env::var("RALGRUM_LIVE_SOUNDCLOUD_TRACK_ID").expect("live SoundCloud track ID");
    let token = SoundCloudToken::from_saved(&saved).expect("valid mobile credential");
    let client = SoundCloudLibraryClient::new().unwrap();
    let title = format!(
        "ralgruM Rust transport verification {}",
        uuid::Uuid::new_v4()
    );
    let playlist_id = client
        .create_playlist(token.clone(), &title, "", true, &[])
        .await
        .expect("mobile playlist create");
    let add_result = client
        .add_tracks_to_playlist(token.clone(), &playlist_id, std::slice::from_ref(&track_id))
        .await;
    let authorization = token.authorization_header().unwrap();
    let verification = client
        .mobile_get(
            format!("{API}/playlists/{playlist_id}"),
            &authorization,
            &[("representation", "full")],
        )
        .await;
    let cleanup = client
        .mobile_request(
            Method::DELETE,
            format!("{API}/playlists/{playlist_id}"),
            authorization,
        )
        .send()
        .await
        .expect("mobile playlist cleanup request");
    assert!(cleanup.status().is_success(), "temporary playlist cleanup");
    assert_eq!(add_result.unwrap(), AddTracksResult::Added { count: 1 });
    let playlist = verification.unwrap();
    assert!(
        playlist_ids_for_update(&playlist)
            .unwrap()
            .iter()
            .any(|id| id == &track_id),
        "added track should be present"
    );
}
