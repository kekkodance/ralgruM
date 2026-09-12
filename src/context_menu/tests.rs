use super::card_menu::{
    album_add_to_playlist_enabled, album_info_enabled, playlist_add_to_playlist_enabled,
    playlist_info_enabled,
};
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
fn track_station_menu_dispatches_to_the_selected_provider() {
    let source = include_str!("items.rs");
    assert!(source.contains("pub(super) fn station_item"));
    assert!(source.contains("host.start_deezer_track_mix(track.id.clone(), cx)"));
    assert!(source.contains("host.start_soundcloud_track_station(track.clone(), cx)"));
}

#[test]
fn soundcloud_track_menu_omits_deezer_feedback_entirely() {
    let source = include_str!("items.rs");
    let feedback = source
        .split("pub(super) fn deezer_feedback_items")
        .nth(1)
        .and_then(|source| source.split("pub(super) fn artist_menu_items").next())
        .expect("Deezer feedback menu builder");
    assert!(feedback.contains("if entity.provider != Provider::Deezer"));
    assert!(feedback.contains("return menu;"));
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
        service_url: "https://soundcloud.com/artist/song".into(),
    });
    assert_eq!(entity.kind, EntityKind::Track);
    assert_eq!(entity.album_id, "album-1");
    assert_eq!(entity.artists.len(), 1);
    assert!(entity.service_link().is_some());
}

#[test]
fn card_menu_exposes_delete_playlist_action_for_owned_playlists() {
    let source = include_str!("card_menu.rs");
    assert!(source.contains("Delete playlist"));
    assert!(source.contains("danger_action_item"));
    assert!(source.contains("open_playlist_delete"));
}

#[test]
fn local_playlist_menu_matches_collection_actions_and_has_a_left_click_variant() {
    let source = include_str!("card_menu.rs");
    let local = source
        .split("fn local_playlist_card_menu_with_trigger")
        .nth(1)
        .and_then(|body| body.split("pub(crate) fn card_menu").next())
        .expect("local playlist menu should be present");
    for action in [
        "Play next in queue",
        "Play last in queue",
        "Download",
        "Edit",
        "Copy title",
        "Delete playlist",
    ] {
        assert!(local.contains(action), "missing Local action: {action}");
    }
    assert!(local.contains("items::playlist_info_item"));
    assert!(source.contains("pub(crate) fn local_playlist_card_menu_button"));
    assert!(source.contains("MouseButton::Left"));
}

#[test]
fn playlist_card_menus_share_info_action_and_skip_add_to_playlist() {
    let source = include_str!("card_menu.rs");
    let items_source = include_str!("items.rs");
    assert!(source.contains("playlist_info_item"));
    assert!(items_source.contains("\"Info\""));
    assert!(items_source.contains("Some(LocalIcon::CircleInfo)"));
    assert!(source.contains("search.open_card_info(info_card.clone(), window, cx)"));
    assert!(source.contains("library.open_card_info(info_card.clone(), window, cx)"));
    assert!(!source.contains("search.open_card(info_card.clone(), cx)"));
    assert!(!source.contains("library.open_card(info_card.clone(), cx)"));
    assert!(playlist_info_enabled(&EntityKind::Playlist));
    assert!(!playlist_info_enabled(&EntityKind::Album));
    assert!(!playlist_add_to_playlist_enabled(&EntityKind::Playlist));
    assert!(playlist_add_to_playlist_enabled(&EntityKind::Album));
    assert!(source.contains("library.add_collection_to_playlist(add_card.clone(), cx)"));
    assert!(source.contains("search.add_collection_to_playlist(add_card.clone(), cx)"));

    let library_menu = source
        .split("fn library_card_menu_with_trigger")
        .nth(1)
        .and_then(|body| body.split("pub(crate) fn card_menu").next())
        .expect("library card menu should be present");
    assert!(
        library_menu
            .find("items::favorite_item")
            .expect("library playlist menu should include Favorite")
            < library_menu
                .find("items::playlist_info_item")
                .expect("library playlist menu should include Info")
    );

    let search_menu = source
        .split("fn card_menu_with_trigger")
        .nth(1)
        .or_else(|| source.split("pub(crate) fn card_menu").nth(1))
        .expect("search card menu should be present");
    assert!(
        search_menu
            .find("items::favorite_item")
            .expect("search playlist menu should include Favorite")
            < search_menu
                .find("items::playlist_info_item")
                .expect("search playlist menu should include Info")
    );
}

#[test]
fn card_menu_enables_add_to_playlist_only_for_deezer_albums() {
    assert!(album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "42",
        true,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Playlist,
        Provider::Deezer,
        "42",
        true,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::SoundCloud,
        "42",
        true,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "not-numeric",
        true,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "42",
        false,
        true
    ));
    assert!(!album_add_to_playlist_enabled(
        &EntityKind::Album,
        Provider::Deezer,
        "42",
        true,
        false
    ));
}

#[test]
fn album_card_menus_expose_info_action_with_circle_info_icon() {
    assert!(album_info_enabled(&EntityKind::Album));
    assert!(!album_info_enabled(&EntityKind::Playlist));
    assert!(!album_info_enabled(&EntityKind::Artist));
    assert!(!album_info_enabled(&EntityKind::Track));

    let source = include_str!("card_menu.rs");
    let items_source = include_str!("items.rs");
    assert!(source.contains("album_info_enabled(&entity.kind)"));
    assert!(items_source.contains("\"Info\""));
    assert!(items_source.contains("Some(LocalIcon::CircleInfo)"));
    assert!(source.contains("search.open_card_info(info_card.clone(), window, cx)"));
    assert!(source.contains("library.open_card_info(info_card.clone(), window, cx)"));

    let library_menu = source
        .split("fn library_card_menu_with_trigger")
        .nth(1)
        .and_then(|body| body.split("pub(crate) fn card_menu").next())
        .expect("library card menu should be present");
    let library_info = library_menu
        .find("items::playlist_info_item")
        .expect("library album menu should include Info");
    assert!(
        library_menu
            .find("items::favorite_item")
            .expect("library album menu should include Favorite")
            < library_info
    );
    assert!(
        library_info
            < library_menu
                .find("items::copy_items")
                .expect("library album Info should precede copy actions")
    );

    let search_menu = source
        .split("fn card_menu_with_trigger")
        .nth(1)
        .or_else(|| source.split("pub(crate) fn card_menu").nth(1))
        .expect("search card menu should be present");
    let search_info = search_menu
        .find("items::playlist_info_item")
        .expect("search album menu should include Info");
    assert!(
        search_menu
            .find("items::favorite_item")
            .expect("search album menu should include Favorite")
            < search_info
    );
    assert!(
        search_info
            < search_menu
                .find("items::copy_items")
                .expect("search album Info should precede copy actions")
    );
}

#[test]
fn unfavorite_action_uses_the_favorite_pink_heart() {
    let source = include_str!("items.rs");
    assert!(source.contains("const FAVORITE_PINK: u32 = 0xec4899"));
    assert!(source.contains("(known == Some(true)).then_some(FAVORITE_PINK)"));
    assert!(
        source
            .matches("(known == Some(true)).then_some(FAVORITE_PINK)")
            .count()
            >= 3,
        "track, artist, and collection Unfavorite rows should all use the pink heart"
    );
}

#[test]
fn player_download_menu_opens_above_with_chevron() {
    let track_menu = include_str!("track_menu.rs");
    assert!(track_menu.contains("pub(crate) fn track_download_button_above"));
    assert!(!track_menu.contains("pub(crate) fn track_download_button("));
    assert!(track_menu.contains(".place_above()"));
    assert!(track_menu.contains("download_format_menu_above"));
    let download_menu = include_str!("download_menu.rs");
    assert!(download_menu.contains("pub(super) fn download_format_menu_above"));
    assert!(!download_menu.contains("pub(super) fn download_format_menu("));
    assert!(download_menu.contains(".with_arrow()"));
    let context_ext = include_str!("context_ext.rs");
    assert!(context_ext.contains("Anchor::BottomCenter"));
    assert!(context_ext.contains("CONTEXT_MENU_ABOVE_GAP_PX"));
    assert!(context_ext.contains("above_position"));
    assert!(context_ext.contains("set_arrow_anchor"));
    let popup = include_str!("popup.rs");
    assert!(popup.contains("show_arrow"));
    assert!(popup.contains("with_arrow"));
    assert!(popup.contains("arrow_anchor_x"));
    assert!(popup.contains("arrow_center_offset"));
    assert!(popup.contains("POPUP_ARROW_EDGE_INSET_PX"));
    let render = include_str!("popup_render.rs");
    assert!(render.contains("app-popup-menu-arrow"));
    assert!(render.contains("arrow_center_offset"));
    let search_rows = include_str!("../search/rows_view.rs");
    assert!(search_rows.contains("track_download_button_above("));
    let library_rows = include_str!("../library/track_view.rs");
    assert!(library_rows.contains("track_download_button_above("));
}

#[test]
fn above_arrow_geometry_matches_tooltip_diamond() {
    let render = include_str!("popup_render.rs");
    assert!(render.contains("MENU_ARROW_SIDE_PX: f32 = 7."));
    assert!(render.contains("MENU_ARROW_CANVAS_PX: f32 = 12."));
    assert!(render.contains("MENU_ARROW_OVERHANG_PX: f32 = 6."));
    let half = 7. / std::f32::consts::SQRT_2;
    assert!((half - 4.9497474).abs() < 0.0001);
}

#[test]
fn play_last_in_queue_action_is_disabled_when_queue_empty() {
    let source = include_str!("items.rs");
    assert!(source.contains("pub(super) fn queue_position_items("));
    assert!(source.contains("next_enabled: bool"));
    assert!(source.contains("last_enabled: bool"));
    assert!(source.contains("!last_enabled"));
}

#[test]
fn soundcloud_artist_menu_starts_station_between_favorite_and_copy() {
    let source = include_str!("items.rs");
    let artist_fn = source
        .split("pub(super) fn artist_menu_items")
        .nth(1)
        .expect("artist menu should be present");
    let soundcloud_check = artist_fn
        .find("Provider::SoundCloud")
        .expect("Station action should be SoundCloud only");
    let station = artist_fn
        .find("\"Station\"")
        .expect("Station action should be present");
    assert!(
        soundcloud_check < station,
        "SoundCloud provider check should guard the Station item"
    );
    let station_window = &artist_fn[station..std::cmp::min(station + 400, artist_fn.len())];
    assert!(
        station_window.contains("Radio"),
        "Station action should use the radio icon"
    );
    assert!(
        station_window.contains("start_soundcloud_artist_station"),
        "Station action should use the existing station queue entry point"
    );
    let favorite = artist_fn
        .find("\"Favorite\"")
        .expect("artist menu should include Favorite");
    let copy_name = artist_fn
        .find("\"Copy name\"")
        .expect("artist menu should include Copy name");
    assert!(favorite < station, "Favorite should precede Station");
    assert!(station < copy_name, "Station should precede Copy items");
    let separator_after_station = artist_fn[station..]
        .find(".separator()")
        .expect("Station should be followed by a separator before Copy items");
    let copy_offset = station + separator_after_station;
    assert!(copy_offset < artist_fn.find("\"Copy name\"").unwrap_or(usize::MAX) + 500);
    let favorite_to_station = &artist_fn[favorite..station];
    assert!(
        favorite_to_station.contains(".separator()"),
        "Station should be preceded by a separator after Favorite"
    );
}

#[test]
fn track_menus_preload_the_playlist_catalog_on_open() {
    let host = include_str!("../app/entity_navigation.rs");
    assert!(host.contains("fn preload_playlist_catalog("));
    assert!(host.contains("Implementations must reuse the guarded catalog load."));

    let track_menu = include_str!("track_menu.rs");
    assert_eq!(
        track_menu
            .matches("host.preload_playlist_catalog(entity.provider, cx)")
            .count(),
        2,
        "both track menu builders should warm the catalog on open"
    );
    assert!(track_menu.contains("if available.add_to_playlist"));

    let queue_menu = include_str!("queue_menu.rs");
    assert!(queue_menu.contains("host.preload_playlist_catalog(entity.provider, cx)"));
    assert!(queue_menu.contains("if entity_actions.add_to_playlist"));

    let library = include_str!("../library/view.rs");
    assert!(library.contains("fn preload_playlist_catalog("));
    assert!(library.contains("self.ensure_playlist_catalog(cx);"));

    let search = include_str!("../search/view.rs");
    assert!(search.contains("fn preload_playlist_catalog("));
    assert!(search.contains("library.ensure_playlist_catalog(cx)"));

    let queue = include_str!("../playback/queue.rs");
    assert!(queue.contains("fn preload_playlist_catalog("));
    assert!(queue.contains("library.ensure_playlist_catalog(cx)"));

    let controller = include_str!("../library/playlist_controller.rs");
    assert!(controller.contains("self.playlists.begin_load(&scope, force)"));
}

#[test]
fn track_menus_share_an_origin_aware_playlist_submenu() {
    let items = include_str!("items.rs");
    let track_menu = include_str!("track_menu.rs");
    let queue_menu = include_str!("queue_menu.rs");

    assert!(items.contains("Save to Local"));
    assert!(items.contains("Remove from Local"));
    assert!(items.contains("pub(super) fn add_to_playlist_submenu"));
    assert!(items.contains("open_local_playlist_picker(local_track"));
    assert!(items.contains("open_playlist_picker("));
    assert!(
        items.contains("provider_track_id,\n                                    origin_provider,")
    );
    assert!(items.matches("window.defer(cx").count() >= 2);
    assert!(items.contains("\"Local\""));
    assert!(items.contains("provider.label()"));
    assert!(!items.contains("Add to Local playlist"));
    assert_eq!(
        track_menu.matches("items::add_to_playlist_submenu").count(),
        2
    );
    assert!(queue_menu.contains("items::add_to_playlist_submenu"));
    assert!(track_menu.contains("track.clone(),\n                available.add_to_playlist"));
    assert!(queue_menu.contains("track.clone(),\n                entity_actions.add_to_playlist"));
}

#[test]
fn library_detail_more_uses_left_click_without_changing_card_right_click() {
    let card_menu = include_str!("card_menu.rs");
    let detail = include_str!("../library/content_view.rs");
    let cards = include_str!("../library/cards_view.rs");
    let track_menu = include_str!("track_menu.rs");

    for function in [
        "fn library_card_menu_with_trigger",
        "fn local_playlist_card_menu_with_trigger",
        "fn card_menu_with_trigger",
    ] {
        let shared = card_menu
            .split_once(&format!("\n{function}"))
            .map(|(_, rest)| rest)
            .map(|rest| {
                rest.split_once("\npub(crate) fn ")
                    .map_or(rest, |(body, _)| body)
            })
            .expect("card menu trigger implementation should be present");
        assert!(shared.contains(".open_on(trigger_button)"));
        assert!(shared.contains("if matches!(trigger_button, MouseButton::Left)"));
        assert!(shared.contains("menu.place_below()"));
    }
    assert!(card_menu.contains("pub(crate) fn library_card_menu_button"));
    assert!(card_menu.contains("pub(crate) fn local_playlist_card_menu_button"));
    assert!(card_menu.contains("pub(crate) fn card_menu_button"));
    for (function, end, button) in [
        (
            "pub(crate) fn library_card_menu<",
            "\npub(crate) fn library_card_menu_button",
            "MouseButton::Right",
        ),
        (
            "pub(crate) fn library_card_menu_button<",
            "\nfn library_card_menu_with_trigger",
            "MouseButton::Left",
        ),
        (
            "pub(crate) fn local_playlist_card_menu<",
            "\npub(crate) fn local_playlist_card_menu_button",
            "MouseButton::Right",
        ),
        (
            "pub(crate) fn local_playlist_card_menu_button<",
            "\nfn local_playlist_card_menu_with_trigger",
            "MouseButton::Left",
        ),
        (
            "pub(crate) fn card_menu<",
            "\npub(crate) fn card_menu_button",
            "MouseButton::Right",
        ),
        (
            "pub(crate) fn card_menu_button<",
            "\nfn card_menu_with_trigger",
            "MouseButton::Left",
        ),
    ] {
        let body = card_menu
            .split_once(function)
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once(end))
            .map(|(body, _)| body)
            .expect("card menu wrapper should have a bounded trigger body");
        assert!(body.contains(button), "{function} should use {button}");
    }
    assert!(track_menu.contains("fn track_menu_with_trigger"));
    let track_shared = track_menu
        .split_once("\nfn track_menu_with_trigger")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\npub(crate) fn current_menu"))
        .map(|(body, _)| body)
        .expect("track menu trigger implementation should be present");
    assert!(track_shared.contains(".open_on(trigger_button)"));
    assert!(track_shared.contains("if matches!(trigger_button, MouseButton::Left)"));
    assert!(track_shared.contains("menu.place_below()"));
    for (function, end, button) in [
        (
            "pub(crate) fn track_menu<E, N>",
            "\npub(crate) fn track_menu_button",
            "MouseButton::Right",
        ),
        (
            "pub(crate) fn track_menu_button<E, N>",
            "\n/// Tracklist and player bar variant",
            "MouseButton::Left",
        ),
    ] {
        let body = track_menu
            .split_once(function)
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split_once(end))
            .map(|(body, _)| body)
            .expect("track menu wrapper should have a bounded trigger body");
        assert!(body.contains(button), "{function} should use {button}");
    }
    assert!(detail.contains("crate::context_menu::library_card_menu_button("));
    assert!(cards.contains("context_menu::library_card_menu("));
    assert!(!cards.contains("context_menu::library_card_menu_button("));
}

#[test]
fn service_switch_resets_category_motion_before_selection() {
    let source = include_str!("../library/view.rs");
    let loader = source
        .split_once("fn load_service_inner(")
        .map(|(_, rest)| rest)
        .expect("service loader should exist");
    let reset = loader
        .find("if self.state.service != service")
        .expect("service changes should reset category motion");
    let reset_assignment = loader[reset..]
        .find("self.category_motion = SegmentedSelectorMotion::default();")
        .map(|offset| reset + offset)
        .expect("service reset should clear the category motion state");
    let selection = loader
        .find("self.state.select(service, category)")
        .expect("service loader should select the requested category");
    assert!(reset < reset_assignment);
    assert!(reset_assignment < selection);
    assert!(loader[reset..reset_assignment].contains("self.state.service != service"));
}

#[test]
fn context_menu_lease_releases_when_the_owner_is_torn_down() {
    let baseline = open_context_menu_count();
    let lease = ContextMenuLease::acquire();
    assert_eq!(open_context_menu_count(), baseline + 1);
    drop(lease);
    assert_eq!(open_context_menu_count(), baseline);
}

#[test]
fn context_menu_dismissal_does_not_capture_shared_state_strongly() {
    let source = include_str!("context_ext.rs");
    assert!(source.contains("let shared_state = Rc::downgrade(&shared_state);"));
    assert!(source.contains("let Some(shared_state) = shared_state.upgrade() else"));
}
