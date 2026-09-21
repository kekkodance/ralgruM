use super::card_menu::album_add_to_playlist_enabled;
use super::links::{canonical_collection_link, canonical_link, playback_link};
use super::*;

#[test]
fn canonical_links_require_numeric_deezer_ids() {
    assert_eq!(
        canonical_link(Provider::Deezer, "42"),
        Some("https://www.deezer.com/track/42".into())
    );
    assert_eq!(canonical_link(Provider::Deezer, ""), None);
    assert_eq!(canonical_link(Provider::SoundCloud, "42"), None);
}

#[test]
fn collection_paths_match_entity_kind() {
    assert_eq!(
        canonical_collection_link(Provider::Deezer, &EntityKind::Album, "7"),
        Some("https://www.deezer.com/album/7".into())
    );
    assert_eq!(
        canonical_collection_link(Provider::Deezer, &EntityKind::Playlist, "7"),
        Some("https://www.deezer.com/playlist/7".into())
    );
    assert_eq!(
        canonical_collection_link(Provider::Deezer, &EntityKind::Artist, "7"),
        Some("https://www.deezer.com/artist/7".into())
    );
}

fn entity(kind: EntityKind, provider: Provider, id: &str, has_tracks: bool) -> MenuEntity {
    MenuEntity {
        kind,
        provider,
        id: id.into(),
        title: "Title".into(),
        track: None,
        has_tracks,
        album_id: String::new(),
        artists: Vec::new(),
        service_url: String::new(),
    }
}

#[test]
fn track_actions_are_exact_by_provider_id() {
    let deezer = action_availability(
        &entity(EntityKind::Track, Provider::Deezer, "42", true),
        false,
    );
    assert_eq!(
        deezer,
        ActionAvailability {
            play_next: true,
            play_last: true,
            download: true,
            add_to_playlist: true,
            favorite: true,
            station: true,
            negative_feedback: true,
            lyrics: true,
            copy_title: true,
            copy_link: true,
        }
    );
    let soundcloud = action_availability(
        &entity(EntityKind::Track, Provider::SoundCloud, "42", true),
        false,
    );
    assert!(soundcloud.download);
    // Lyrics now open for any track, not only the playing one.
    assert!(soundcloud.lyrics);
    assert!(soundcloud.add_to_playlist);
    assert!(soundcloud.favorite);
    assert!(soundcloud.station);
    assert!(!soundcloud.negative_feedback);
    // SoundCloud has no id-derived link, so the item stays disabled.
    assert!(!soundcloud.copy_link);
    let invalid = action_availability(
        &entity(EntityKind::Track, Provider::Deezer, "42/a", true),
        false,
    );
    assert!(!invalid.add_to_playlist);
    let invalid_soundcloud = action_availability(
        &entity(EntityKind::Track, Provider::SoundCloud, "42/a", true),
        false,
    );
    assert!(!invalid_soundcloud.add_to_playlist);
    assert!(!invalid.favorite);
    assert!(!invalid.station);
    assert!(!invalid.negative_feedback);
    assert!(!invalid.copy_link);
}

#[test]
fn soundcloud_service_urls_enable_the_copy_link_item() {
    let mut soundcloud = entity(EntityKind::Track, Provider::SoundCloud, "9", true);
    soundcloud.service_url = "https://soundcloud.com/artist/song".into();
    assert!(action_availability(&soundcloud, false).copy_link);
    assert_eq!(
        soundcloud.service_link().as_deref(),
        Some("https://soundcloud.com/artist/song")
    );

    let mut playlist = entity(EntityKind::Playlist, Provider::SoundCloud, "9", false);
    playlist.service_url = "https://soundcloud.com/art/sets/list".into();
    assert!(action_availability(&playlist, false).copy_link);
}

#[test]
fn library_soundcloud_entities_carry_the_service_url_link() {
    let track = crate::library::Track {
        id: "9".into(),
        title: "Song".into(),
        service_url: "https://soundcloud.com/artist/song".into(),
        ..crate::library::Track::default()
    };
    let entity = library_track_entity(&track, Provider::SoundCloud);
    assert_eq!(
        entity.service_link().as_deref(),
        Some("https://soundcloud.com/artist/song")
    );
    assert!(action_availability(&entity, false).copy_link);
    let embedded = entity.track.expect("library entity embeds a track");
    assert_eq!(
        playback_link(&embedded).as_deref(),
        Some("https://soundcloud.com/artist/song")
    );

    let bare = library_track_entity(&crate::library::Track::default(), Provider::SoundCloud);
    assert!(bare.service_link().is_none());
    assert!(!action_availability(&bare, false).copy_link);

    // Deezer library rows keep deriving the link from the numeric id.
    let deezer = library_track_entity(
        &crate::library::Track {
            id: "42".into(),
            ..crate::library::Track::default()
        },
        Provider::Deezer,
    );
    assert_eq!(
        deezer.service_link().as_deref(),
        Some("https://www.deezer.com/track/42")
    );
}

#[test]
fn library_soundcloud_collection_entities_keep_their_public_links() {
    for (kind, url) in [
        (
            crate::library::Category::Albums,
            "https://soundcloud.com/artist/sets/album",
        ),
        (
            crate::library::Category::Artists,
            "https://soundcloud.com/artist",
        ),
        (
            crate::library::Category::Playlists,
            "https://soundcloud.com/artist/sets/playlist",
        ),
    ] {
        let card = crate::library::Card {
            kind,
            id: "9".into(),
            source: Provider::SoundCloud,
            service_url: url.into(),
            ..crate::library::Card::default()
        };
        let entity = library_card_entity(&card, true).expect("supported library card");
        assert_eq!(entity.service_link().as_deref(), Some(url));
        assert!(action_availability(&entity, false).copy_link);
    }
}

#[test]
fn explicit_blocking_disables_play_actions_but_keeps_the_rest() {
    let blocked = action_availability(
        &entity(EntityKind::Track, Provider::Deezer, "42", true),
        true,
    );
    assert!(!blocked.play_next && !blocked.play_last);
    assert!(blocked.download);
    assert!(blocked.add_to_playlist);
    assert!(blocked.copy_title);

    let collection = action_availability(
        &entity(EntityKind::Album, Provider::Deezer, "7", true),
        true,
    );
    assert!(!collection.play_next && !collection.play_last);
}

#[test]
fn collection_and_artist_actions_require_real_capabilities() {
    let album = action_availability(
        &entity(EntityKind::Album, Provider::Deezer, "7", true),
        false,
    );
    assert!(album.play_next && album.play_last && album.favorite);
    assert!(!album.download && !album.lyrics);
    let unloaded = action_availability(
        &entity(EntityKind::Playlist, Provider::Deezer, "7", false),
        false,
    );
    assert!(!unloaded.play_next && !unloaded.play_last && !unloaded.download);
    let artist = action_availability(
        &entity(EntityKind::Artist, Provider::Deezer, "7", false),
        false,
    );
    assert!(artist.favorite && artist.copy_title && artist.copy_link);
    assert!(!artist.play_next && !artist.play_last && !artist.download);

    let soundcloud_album = action_availability(
        &entity(EntityKind::Album, Provider::SoundCloud, "7", true),
        false,
    );
    assert!(soundcloud_album.favorite);
    assert!(!soundcloud_album.add_to_playlist);
    let soundcloud_artist = action_availability(
        &entity(EntityKind::Artist, Provider::SoundCloud, "7", false),
        false,
    );
    assert!(soundcloud_artist.favorite);
}

#[test]
fn library_card_entities_only_expose_context_menu_supported_categories() {
    let album = crate::library::Card {
        kind: crate::library::Category::Albums,
        id: "7".into(),
        title: "Album".into(),
        source: Provider::Deezer,
        ..crate::library::Card::default()
    };
    assert_eq!(
        library_card_entity(&album, true).map(|entity| entity.kind),
        Some(EntityKind::Album)
    );
    for kind in [
        crate::library::Category::Flow,
        crate::library::Category::Station,
    ] {
        let card = crate::library::Card {
            kind,
            id: "7".into(),
            source: Provider::Deezer,
            ..crate::library::Card::default()
        };
        assert!(library_card_entity(&card, true).is_none());
    }
}

#[test]
fn queue_actions_only_enable_valid_reordering_and_provider_links() {
    let track = PlaybackTrack {
        provider: crate::playback::PlaybackProvider::Deezer,
        id: "42".into(),
        title: "Title".into(),
        artist: String::new(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: std::time::Duration::ZERO,
        downloadable: false,
        progressive: false,
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    assert_eq!(
        queue_action_availability(&track, 0, 2, false),
        QueueActionAvailability {
            play_now: true,
            play_next: true,
            play_last: true,
            move_up: false,
            move_down: true,
            remove: true,
            copy_link: true,
        }
    );
    assert!(!queue_action_availability(&track, 0, 2, true).play_now);
    assert!(queue_action_availability(&track, 0, 2, true).remove);
    assert!(
        !queue_action_availability(
            &PlaybackTrack {
                id: "bad/id".into(),
                ..track.clone()
            },
            1,
            2,
            false
        )
        .copy_link
    );
    let mut soundcloud = PlaybackTrack {
        provider: crate::playback::PlaybackProvider::SoundCloud,
        id: "9".into(),
        ..track
    };
    assert!(!queue_action_availability(&soundcloud, 1, 2, false).copy_link);
    soundcloud.service_url = "https://soundcloud.com/artist/song".into();
    assert!(queue_action_availability(&soundcloud, 1, 2, false).copy_link);
}

#[test]
fn queue_track_entities_carry_navigation_and_link_data() {
    let entity = queue_track_entity(&PlaybackTrack {
        provider: crate::playback::PlaybackProvider::SoundCloud,
        id: "9".into(),
        title: "Song".into(),
        artist: "Artist".into(),
        album: "Album".into(),
        album_id: "album-1".into(),
        release_date: String::new(),
        artists: vec![TrackArtistRef {
            id: "55".into(),
            name: "Artist".into(),
        }],
        artwork: String::new(),
        duration: std::time::Duration::ZERO,
        downloadable: false,
        progressive: false,
        explicit: false,
        ai_generated: false,
        service_url: "https://soundcloud.com/artist/song".into(),
    });
    assert_eq!(entity.kind, EntityKind::Track);
    assert_eq!(entity.album_id, "album-1");
    assert_eq!(entity.artists.len(), 1);
    assert!(entity.service_link().is_some());
}

#[test]
fn card_menu_allows_local_deezer_albums_without_account() {
    assert!(album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "42",
        true,
        false
    ));
    assert!(album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::SoundCloud,
        "42",
        true,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Playlist,
        Provider::Deezer,
        "42",
        true,
        false
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "not-numeric",
        true,
        false
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "42",
        false,
        false
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::SoundCloud,
        "42",
        true,
        false
    ));
}

#[test]
fn context_menu_lease_releases_when_the_owner_is_torn_down() {
    let baseline = open_context_menu_count();
    let lease = ContextMenuLease::acquire();
    assert_eq!(open_context_menu_count(), baseline + 1);
    drop(lease);
    assert_eq!(open_context_menu_count(), baseline);
}
