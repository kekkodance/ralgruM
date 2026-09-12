use serde_json::Value;

use super::models::{
    Card, Groups, Provider, ResultType, Track, TrackArtistCollector, TrackArtistRef,
};
use super::soundcloud_metadata::artist_subtitle as soundcloud_artist_subtitle;

pub(crate) fn append(
    groups: &mut Groups,
    provider: Provider,
    category: ResultType,
    items: Vec<Value>,
) {
    match category {
        ResultType::Tracks => groups
            .tracks
            .extend(items.iter().map(|item| track(provider, item))),
        ResultType::Albums => groups
            .albums
            .extend(items.iter().map(|item| card(provider, category, item))),
        ResultType::Artists => groups
            .artists
            .extend(items.iter().map(|item| card(provider, category, item))),
        ResultType::Playlists => groups
            .playlists
            .extend(items.iter().map(|item| card(provider, category, item))),
        ResultType::All => {}
    }
}

pub(crate) fn normalize_tracks(provider: Provider, items: &[Value]) -> Vec<Track> {
    items.iter().map(|item| track(provider, item)).collect()
}

pub(crate) fn normalize_card(provider: Provider, kind: ResultType, value: &Value) -> Card {
    card(provider, kind, value)
}

fn string(value: Option<&Value>) -> String {
    value
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn number(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0)
}

pub(crate) fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let first = digits.len() % 3;
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    if first != 0 {
        output.push_str(&digits[..first]);
    }
    for (index, chunk) in digits.as_bytes()[first..].chunks(3).enumerate() {
        if first != 0 || index > 0 {
            output.push(',');
        }
        output.push_str(std::str::from_utf8(chunk).unwrap_or_default());
    }
    output
}

fn boolean(value: Option<&Value>) -> Option<bool> {
    value.and_then(|value| match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) => match value.as_str() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

fn deezer_artwork(hash: &str, kind: &str) -> String {
    let hash = hash.trim();
    let valid_hash = !hash.is_empty()
        && hash.split('-').all(|segment| {
            !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    if !valid_hash {
        String::new()
    } else {
        let kind = if kind == "playlist" && hash.contains('-') {
            "cover"
        } else {
            kind
        };
        format!("https://e-cdns-images.dzcdn.net/images/{kind}/{hash}/500x500.jpg")
    }
}

fn deezer_card_artwork(value: &Value, picture_key: &str, picture_kind: &str) -> String {
    let direct = [
        value.get("picture_xl"),
        value.get("picture_big"),
        value.get("picture_medium"),
        value.get("picture"),
        value.get("PICTURE_XL"),
        value.get("PICTURE_BIG"),
        value.get("PICTURE_MEDIUM"),
        value.get("PICTURE"),
    ]
    .into_iter()
    .filter_map(|value| value.and_then(Value::as_str))
    .map(safe_artwork)
    .find(|value| !value.is_empty());
    direct.unwrap_or_else(|| {
        safe_artwork(&deezer_artwork(
            &string(value.get(picture_key)),
            picture_kind,
        ))
    })
}

pub(crate) fn soundcloud_artwork(value: &Value) -> String {
    let source = [
        value.get("artwork"),
        value.get("artwork_url"),
        value.get("artworkUrl"),
        value.get("artwork_url_template"),
        value.get("avatar_url"),
        value.pointer("/user/avatar_url"),
    ]
    .into_iter()
    .find_map(|value| value.and_then(Value::as_str))
    .unwrap_or_default();
    safe_artwork(
        &source
            .replace("-large", "-t500x500")
            .replace("{size}", "t500x500"),
    )
}

pub(crate) fn safe_artwork(value: &str) -> String {
    match reqwest::Url::parse(value) {
        Ok(url) if url.scheme() == "https" => url.to_string(),
        _ => String::new(),
    }
}

/// Mirrors normalizedMusicServiceUrl in public/js/core/service-urls.js for
/// SoundCloud: only https URLs on soundcloud.com (or a subdomain) survive.
pub(crate) fn soundcloud_service_url(value: &Value) -> String {
    let raw = [value.get("permalink_url"), value.get("permalinkUrl")]
        .into_iter()
        .find_map(|value| value.and_then(Value::as_str))
        .unwrap_or_default();
    match reqwest::Url::parse(raw) {
        Ok(url)
            if url.scheme() == "https"
                && url.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case("soundcloud.com")
                        || host.to_ascii_lowercase().ends_with(".soundcloud.com")
                }) =>
        {
            url.to_string()
        }
        _ => String::new(),
    }
}

fn track(provider: Provider, value: &Value) -> Track {
    match provider {
        Provider::Deezer => deezer_track(value),
        Provider::SoundCloud => soundcloud_track(value),
    }
}

fn deezer_track(value: &Value) -> Track {
    let base_values = [string(value.get("SNG_TITLE")), string(value.get("title"))];
    let base = nonempty(&base_values).unwrap_or("Unknown Track");
    let version_values = [
        string(value.get("VERSION")),
        string(value.get("title_version")),
    ];
    let version = nonempty(&version_values).unwrap_or_default();
    let title = if version.is_empty() {
        base.to_owned()
    } else if version.starts_with('(') {
        format!("{base} {version}")
    } else {
        format!("{base} ({version})")
    };
    let mut collector = TrackArtistCollector::default();
    let entries = value
        .get("ARTISTS")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            value
                .get("contributors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .chain(value.get("artist"));
    for artist in entries {
        let id_values = [string(artist.get("ART_ID")), string(artist.get("id"))];
        let name_values = [string(artist.get("ART_NAME")), string(artist.get("name"))];
        collector.push(
            nonempty(&id_values).unwrap_or_default(),
            nonempty(&name_values).unwrap_or_default(),
        );
    }
    collector.push(
        &string(value.get("ART_ID")),
        nonempty(&[string(value.get("ART_NAME"))]).unwrap_or_default(),
    );
    let artists = collector.finish();
    let artwork = deezer_artwork(&string(value.get("ALB_PICTURE")), "cover");
    let artwork = nonempty(&[
        artwork,
        string(value.pointer("/album/cover_xl")),
        string(value.pointer("/album/cover_medium")),
    ])
    .unwrap_or_default()
    .to_owned();
    let artwork = safe_artwork(&artwork);
    Track {
        id: nonempty(&[
            string(value.get("SNG_ID")),
            string(value.get("ID")),
            string(value.get("id")),
        ])
        .unwrap_or_default()
        .to_owned(),
        title: title.clone(),
        artist: if artists.is_empty() {
            "Unknown Artist".into()
        } else {
            artists
                .iter()
                .map(|artist| artist.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        },
        artists: artists.clone(),
        album: nonempty(&[
            string(value.get("ALB_TITLE")),
            string(value.pointer("/album/title")),
            string(value.pointer("/album/name")),
        ])
        .unwrap_or_default()
        .to_owned(),
        album_id: nonempty(&[
            string(value.get("ALB_ID")),
            string(value.pointer("/album/id")),
        ])
        .unwrap_or_default()
        .to_owned(),
        release_date: release_date(value),
        duration: number(value.get("DURATION")).max(number(value.get("duration"))),
        downloadable: false,
        progressive: false,
        artwork,
        source: Provider::Deezer,
        explicit: string(value.get("EXPLICIT_LYRICS")) == "1"
            || string(value.get("EXPLICIT_TRACK_CONTENT")) == "1"
            || value.get("explicit_lyrics").and_then(Value::as_bool) == Some(true)
            || title.to_ascii_lowercase().contains("explicit"),
        favorite: boolean(
            value
                .get("IS_FAVORITE")
                .or_else(|| value.get("is_favorite"))
                .or_else(|| value.get("isFavorite")),
        ),
        service_url: String::new(),
    }
}

fn soundcloud_track(value: &Value) -> Track {
    let title_values = [string(value.get("title"))];
    let raw_title = nonempty(&title_values).unwrap_or("Unknown Track");
    let split = raw_title.split_once(" - ");
    let publisher_values = [
        string(value.pointer("/publisher_metadata/artist")),
        string(value.pointer("/publisherMetadata/artist")),
    ];
    let publisher = nonempty(&publisher_values);
    let username = string(value.pointer("/user/username"));
    let full_name = string(value.pointer("/user/full_name"));
    let title_artist = split
        .map(|(artist, _)| artist.trim())
        .filter(|v| !v.is_empty());
    let title = split
        .map(|(_, title)| title.trim())
        .filter(|title| title_artist.is_some() && !title.is_empty())
        .unwrap_or(raw_title);
    let artist = publisher
        .or(title_artist)
        .or((!username.trim().is_empty()).then_some(username.as_str()))
        .or((!full_name.trim().is_empty()).then_some(full_name.as_str()))
        .unwrap_or("Unknown Artist");
    let duration_ms = number(value.get("duration"));
    let duration_ms = if duration_ms > 0 {
        duration_ms
    } else {
        number(value.get("full_duration"))
    };
    let user_id = string(value.pointer("/user/id"));
    let uploader_name = if !username.trim().is_empty() {
        Some(username.as_str())
    } else if !full_name.trim().is_empty() {
        Some(full_name.as_str())
    } else {
        None
    };
    let artists = if user_id.trim().is_empty() || uploader_name.is_none() {
        Vec::new()
    } else {
        vec![TrackArtistRef {
            id: user_id,
            name: uploader_name.unwrap_or_default().to_owned(),
        }]
    };
    Track {
        id: nonempty(&[string(value.get("id")), urn_id(value)])
            .unwrap_or_default()
            .to_owned(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        artists,
        album: nonempty(&[
            string(value.pointer("/publisher_metadata/album_name")),
            string(value.pointer("/publisherMetadata/albumName")),
            string(value.get("album_name")),
            string(value.get("album")),
        ])
        .unwrap_or_default()
        .to_owned(),
        album_id: String::new(),
        release_date: release_date(value),
        duration: duration_ms / 1000,
        downloadable: value
            .get("downloadable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        progressive: value
            .pointer("/media/transcodings")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.pointer("/format/protocol")
                        .and_then(Value::as_str)
                        .is_some_and(|protocol| protocol.eq_ignore_ascii_case("progressive"))
                        && item.get("url").and_then(Value::as_str).is_some()
                })
            }),
        artwork: soundcloud_artwork(value),
        source: Provider::SoundCloud,
        explicit: value.get("explicit").and_then(Value::as_bool) == Some(true)
            || value
                .pointer("/publisher_metadata/explicit")
                .and_then(Value::as_bool)
                == Some(true)
            || title.to_ascii_lowercase().contains("explicit"),
        favorite: boolean(
            value
                .get("user_favorite")
                .or_else(|| value.get("userFavorite"))
                .or_else(|| value.get("is_favorite")),
        ),
        service_url: soundcloud_service_url(value),
    }
}

fn card(provider: Provider, kind: ResultType, value: &Value) -> Card {
    match provider {
        Provider::Deezer => deezer_card(kind, value),
        Provider::SoundCloud => soundcloud_card(kind, value),
    }
}

fn deezer_card(kind: ResultType, value: &Value) -> Card {
    let (id_key, title_key, picture_key, picture_kind) = match kind {
        ResultType::Albums => ("ALB_ID", "ALB_TITLE", "ALB_PICTURE", "cover"),
        ResultType::Artists => ("ART_ID", "ART_NAME", "ART_PICTURE", "artist"),
        ResultType::Playlists => ("PLAYLIST_ID", "TITLE", "PLAYLIST_PICTURE", "playlist"),
        _ => ("", "", "", "cover"),
    };
    let title_fallback = match kind {
        ResultType::Albums => "Untitled album",
        ResultType::Artists => "Unknown artist",
        ResultType::Playlists => "Untitled playlist",
        _ => "Untitled",
    };
    let subtitle = match kind {
        ResultType::Albums => nonempty(&[string(value.get("ART_NAME"))])
            .unwrap_or("Unknown artist")
            .to_owned(),
        ResultType::Artists => {
            let fans = number(value.get("NB_FAN"));
            if fans > 0 {
                format!("{} fans", format_number(fans))
            } else {
                "Followed artist".into()
            }
        }
        ResultType::Playlists => nonempty(&[string(value.get("PARENT_USERNAME"))])
            .unwrap_or("Deezer playlist")
            .to_owned(),
        _ => String::new(),
    };
    let badge = match kind {
        ResultType::Albums => release_year(value),
        ResultType::Playlists => number(value.get("NB_SONG")).to_string(),
        _ => String::new(),
    };
    let id = string(value.get(id_key));
    let artwork = if kind == ResultType::Playlists {
        crate::integrations::deezer::playlist_image_url(&id)
    } else {
        deezer_card_artwork(value, picture_key, picture_kind)
    };
    Card {
        kind,
        id,
        title: nonempty(&[string(value.get(title_key))])
            .unwrap_or(title_fallback)
            .to_owned(),
        subtitle,
        artwork,
        source: Provider::Deezer,
        badge,
        release_date: release_date(value),
        service_url: String::new(),
    }
}

fn soundcloud_card(kind: ResultType, value: &Value) -> Card {
    if kind == ResultType::Artists {
        return Card {
            kind,
            id: nonempty(&[string(value.get("id")), urn_id(value)])
                .unwrap_or_default()
                .to_owned(),
            title: nonempty(&[
                string(value.get("username")),
                string(value.get("full_name")),
            ])
            .unwrap_or("Unknown artist")
            .to_owned(),
            subtitle: soundcloud_artist_subtitle(value),
            artwork: soundcloud_artwork(value),
            source: Provider::SoundCloud,
            badge: String::new(),
            release_date: release_date(value),
            service_url: soundcloud_service_url(value),
        };
    }
    let tracks = value.get("tracks").and_then(Value::as_array);
    let embedded_count = tracks.map_or(0, |items| items.len() as u64);
    let count = if embedded_count > 0 {
        embedded_count
    } else {
        number(value.get("track_count"))
    };
    let title_fallback = if kind == ResultType::Albums {
        "Untitled album"
    } else {
        "Untitled playlist"
    };
    Card {
        kind,
        id: nonempty(&[string(value.get("id")), urn_id(value)])
            .unwrap_or_default()
            .to_owned(),
        title: nonempty(&[string(value.get("title"))])
            .unwrap_or(title_fallback)
            .to_owned(),
        subtitle: nonempty(&[
            string(value.pointer("/user/username")),
            string(value.pointer("/user/full_name")),
        ])
        .unwrap_or("SoundCloud")
        .to_owned(),
        artwork: {
            let artwork = soundcloud_artwork(value);
            if artwork.is_empty() {
                tracks
                    .into_iter()
                    .flatten()
                    .map(soundcloud_artwork)
                    .find(|artwork| !artwork.is_empty())
                    .unwrap_or_default()
            } else {
                artwork
            }
        },
        source: Provider::SoundCloud,
        badge: if kind == ResultType::Albums {
            release_year(value)
        } else {
            count.to_string()
        },
        release_date: release_date(value),
        service_url: soundcloud_service_url(value),
    }
}

fn release_year(value: &Value) -> String {
    let date = release_date(value);
    date.get(0..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_default()
        .to_owned()
}

pub(crate) fn release_date(value: &Value) -> String {
    nonempty(&[
        string(value.get("PHYSICAL_RELEASE_DATE")),
        string(value.get("physical_release_date")),
        string(value.get("DIGITAL_RELEASE_DATE")),
        string(value.get("digital_release_date")),
        string(value.get("ORIGINAL_RELEASE_DATE")),
        string(value.get("original_release_date")),
        string(value.get("originalReleaseDate")),
        string(value.get("release_date")),
        string(value.get("display_date")),
    ])
    .unwrap_or_default()
    .to_owned()
}

fn urn_id(value: &Value) -> String {
    string(value.get("urn"))
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn nonempty(values: &[String]) -> Option<&str> {
    values
        .iter()
        .map(String::as_str)
        .find(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn track_album_metadata_is_normalized() {
        let track = normalize_tracks(
            Provider::SoundCloud,
            &[
                json!({"id":"1","title":"Artist - Song","publisher_metadata":{"album_name":"Album"}}),
            ],
        );
        assert_eq!(track[0].album, "Album");
    }

    #[test]
    fn format_number_groups_large_fan_counts() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(3_875_324), "3,875,324");
    }

    #[test]
    fn normalizes_deezer_title_version_and_deduplicates_artists() {
        let item = deezer_track(&json!({
            "SNG_ID": "7", "SNG_TITLE": "Song", "VERSION": "Live",
            "DURATION": "123", "ALB_PICTURE": "hash", "EXPLICIT_LYRICS": "1",
            "ALB_ID": "302", "ALB_TITLE": "Album",
            "ARTISTS": [{"ART_ID": "11", "ART_NAME": "One"}, {"ART_NAME": "one"}],
            "contributors": [{"id": "12", "name": "Two"}]
        }));
        assert_eq!(item.title, "Song (Live)");
        assert_eq!(item.artist, "One, Two");
        assert_eq!(
            item.artists,
            vec![
                TrackArtistRef {
                    id: "11".into(),
                    name: "One".into()
                },
                TrackArtistRef {
                    id: "12".into(),
                    name: "Two".into()
                },
            ]
        );
        assert_eq!(item.album_id, "302");
        assert_eq!(item.album, "Album");
        assert_eq!(item.duration, 123);
        assert!(item.explicit);
    }

    #[test]
    fn normalizes_soundcloud_track_fallbacks() {
        let item = soundcloud_track(&json!({
            "urn": "soundcloud:tracks:9", "title": "Uploader - Actual title",
            "duration": 61000, "artwork_url": "https://example.com/image-large.jpg",
            "user": {"id": "55", "username": "Other"}
        }));
        assert_eq!(item.id, "9");
        assert_eq!(item.title, "Actual title");
        assert_eq!(item.artist, "Uploader");
        assert_eq!(item.artwork, "https://example.com/image-t500x500.jpg");
        assert_eq!(
            item.artists,
            vec![TrackArtistRef {
                id: "55".into(),
                name: "Other".into()
            }]
        );
        assert_eq!(item.album_id, "");
        assert_eq!(item.service_url, "");

        let anonymous = soundcloud_track(&json!({"id": "10", "title": "Track"}));
        assert!(anonymous.artists.is_empty());
    }

    #[test]
    fn soundcloud_track_keeps_credited_artist_and_canonical_uploader_ref() {
        let item = soundcloud_track(&json!({
            "id": "9",
            "title": "Skrillex - Song",
            "publisher_metadata": {"artist": "Skrillex"},
            "user": {"id": "55", "username": "TRVCY", "full_name": "Trevor"}
        }));

        assert_eq!(item.artist, "Skrillex");
        assert_eq!(
            item.artists,
            vec![TrackArtistRef {
                id: "55".into(),
                name: "TRVCY".into()
            }]
        );
    }

    #[test]
    fn soundcloud_artist_card_uses_top_level_avatar_url() {
        let artist = soundcloud_card(
            ResultType::Artists,
            &json!({
                "id": "55",
                "username": "TRVCY",
                "avatar_url": "https://example.com/avatar-large.jpg"
            }),
        );

        assert_eq!(artist.artwork, "https://example.com/avatar-t500x500.jpg");
    }

    #[test]
    fn soundcloud_service_urls_accept_only_https_soundcloud_hosts() {
        assert_eq!(
            soundcloud_service_url(&json!({"permalink_url": "https://soundcloud.com/artist/song"})),
            "https://soundcloud.com/artist/song"
        );
        assert_eq!(
            soundcloud_service_url(&json!({"permalink_url": "https://m.soundcloud.com/sets/list"})),
            "https://m.soundcloud.com/sets/list"
        );
        assert_eq!(
            soundcloud_service_url(&json!({"permalink_url": "http://soundcloud.com/artist/song"})),
            ""
        );
        assert_eq!(
            soundcloud_service_url(&json!({"permalink_url": "https://evil.test/soundcloud.com"})),
            ""
        );
        assert_eq!(soundcloud_service_url(&json!({})), "");

        let track = soundcloud_track(&json!({
            "id": "11",
            "title": "Track",
            "permalink_url": "https://soundcloud.com/artist/track"
        }));
        assert_eq!(track.service_url, "https://soundcloud.com/artist/track");

        let playlist = soundcloud_card(
            ResultType::Playlists,
            &json!({"id": 3, "title": "List", "permalink_url": "https://soundcloud.com/art/sets/list"}),
        );
        assert_eq!(playlist.service_url, "https://soundcloud.com/art/sets/list");
    }

    #[test]
    fn soundcloud_duration_prefers_the_playable_duration() {
        let item = soundcloud_track(&json!({
            "id": 9, "title": "Track", "duration": 30000, "full_duration": 90000
        }));
        assert_eq!(item.duration, 30);

        let fallback = soundcloud_track(&json!({
            "id": 10, "title": "Track", "duration": 0, "full_duration": 90000
        }));
        assert_eq!(fallback.duration, 90);
    }

    #[test]
    fn exposed_detail_normalizer_uses_the_provider_contract() {
        let tracks = normalize_tracks(
            Provider::Deezer,
            &[json!({
                "SNG_ID": "42", "SNG_TITLE": "Fixture", "DURATION": "61",
                "ARTISTS": [{"ART_NAME": "Artist"}]
            })],
        );
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "42");
        assert_eq!(tracks[0].title, "Fixture");
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[0].duration, 61);
        assert_eq!(tracks[0].favorite, None);
    }

    #[test]
    fn deezer_favorite_metadata_is_optional_and_normalized() {
        let known = deezer_track(&json!({"SNG_ID": "42", "IS_FAVORITE": "1"}));
        let unknown = deezer_track(&json!({"SNG_ID": "43"}));
        assert_eq!(known.favorite, Some(true));
        assert_eq!(unknown.favorite, None);
    }

    #[test]
    fn soundcloud_favorite_metadata_is_optional_and_normalized() {
        let known = soundcloud_track(&json!({
            "id": "42",
            "title": "Known",
            "user_favorite": true
        }));
        let unknown = soundcloud_track(&json!({"id": "43", "title": "Unknown"}));
        assert_eq!(known.favorite, Some(true));
        assert_eq!(unknown.favorite, None);
    }

    #[test]
    fn normalizes_provider_cards_without_inventing_fields() {
        let album = deezer_card(
            ResultType::Albums,
            &json!({
                "ALB_ID": "2", "ALB_TITLE": "Album", "ART_NAME": "Artist",
                "DIGITAL_RELEASE_DATE": "2024-01-01"
            }),
        );
        assert_eq!(album.subtitle, "Artist");
        assert_eq!(album.badge, "2024");
        let playlist = soundcloud_card(
            ResultType::Playlists,
            &json!({
                "id": 3, "title": "List", "track_count": 8, "user": {"full_name": "Owner"}
            }),
        );
        assert_eq!(playlist.subtitle, "Owner");
        assert_eq!(playlist.badge, "8");
    }

    #[test]
    fn normalizes_missing_search_card_badges_at_the_provider_boundary() {
        let album = deezer_card(ResultType::Albums, &json!({"ALB_ID": "2"}));
        assert_eq!(album.badge, "");

        let deezer_playlist = deezer_card(ResultType::Playlists, &json!({"PLAYLIST_ID": "3"}));
        assert_eq!(deezer_playlist.badge, "0");

        let soundcloud_playlist = soundcloud_card(ResultType::Playlists, &json!({"id": 4}));
        assert_eq!(soundcloud_playlist.badge, "0");
    }

    #[test]
    fn soundcloud_collection_uses_count_and_artwork_fallbacks() {
        let card = soundcloud_card(
            ResultType::Playlists,
            &json!({
                "id": 3,
                "title": "List",
                "track_count": 8,
                "tracks": [{"artwork_url": "https://example.com/track-large.jpg"}]
            }),
        );
        assert_eq!(card.badge, "1");
        assert_eq!(card.artwork, "https://example.com/track-t500x500.jpg");

        let empty = soundcloud_card(
            ResultType::Playlists,
            &json!({"id": 4, "title": "List", "track_count": 8, "tracks": []}),
        );
        assert_eq!(empty.badge, "8");
    }

    #[test]
    fn artwork_accepts_only_https_urls() {
        assert_eq!(
            safe_artwork("https://example.com/image.jpg"),
            "https://example.com/image.jpg"
        );
        assert!(safe_artwork("http://127.0.0.1/image.jpg").is_empty());
        assert!(safe_artwork("file:///C:/secret.txt").is_empty());
        assert!(safe_artwork("not a URL").is_empty());
    }

    #[test]
    fn deezer_card_artwork_is_normalized_before_rendering() {
        let card = deezer_card(
            ResultType::Albums,
            &json!({"ALB_ID": "2", "ALB_PICTURE": "not/a/provider/hash"}),
        );

        assert!(card.artwork.is_empty());
    }

    #[test]
    fn deezer_playlist_cover_uses_the_canonical_id_endpoint() {
        let hash = "db4a108e2e0578228716e7ffebdafac2-953fdae6e89776300999dab5ad10dd63-19824b123a2257af052888116467e4f5-1fd07470637104beaeada95c3514a7aa";
        let card = deezer_card(
            ResultType::Playlists,
            &json!({"PLAYLIST_ID":"13743145521","TITLE":"Generated","PLAYLIST_PICTURE":hash}),
        );

        assert_eq!(
            card.artwork,
            "https://api.deezer.com/playlist/13743145521/image"
        );
    }

    #[test]
    fn deezer_playlist_card_prefers_the_canonical_id_endpoint() {
        let card = deezer_card(
            ResultType::Playlists,
            &json!({
                "PLAYLIST_ID":"7",
                "TITLE":"Direct",
                "PLAYLIST_PICTURE":"legacyhash",
                "picture_xl":"https://cdn.example.com/playlist.jpg"
            }),
        );

        assert_eq!(card.artwork, "https://api.deezer.com/playlist/7/image");
    }
}
