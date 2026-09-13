use super::model::{Card, Category, Track, value_string};
use crate::search::{Provider, TrackArtistCollector};
use serde_json::Value;

fn number(value: Option<&Value>) -> u64 {
    value
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(0)
}
fn first(values: &[String], fallback: &str) -> String {
    values
        .iter()
        .find(|v| !v.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.into())
}
fn artwork(hash: &str, kind: &str) -> String {
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
        let size = if kind == "artist" {
            "250x250"
        } else {
            "500x500"
        };
        format!("https://e-cdns-images.dzcdn.net/images/{kind}/{hash}/{size}.jpg")
    }
}

fn release_year(value: &Value) -> String {
    let date = crate::search::release_date(value);
    date.get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_default()
        .to_owned()
}

fn release_date(value: &Value, kind: Category) -> String {
    if kind == Category::Albums {
        crate::search::release_date(value)
    } else {
        String::new()
    }
}

pub(crate) fn track(value: &Value) -> Track {
    let base = first(
        &[
            value_string(value.get("SNG_TITLE")),
            value_string(value.get("title")),
        ],
        "Unknown Track",
    );
    let version = value_string(value.get("VERSION"));
    let title = if version.is_empty() {
        base
    } else if version.starts_with('(') {
        format!("{base} {version}")
    } else {
        format!("{base} ({version})")
    };
    let mut collector = TrackArtistCollector::default();
    for source in [
        value.get("ARTISTS").and_then(Value::as_array),
        value.get("contributors").and_then(Value::as_array),
    ] {
        for artist in source.into_iter().flatten() {
            let id = first(
                &[
                    value_string(artist.get("ART_ID")),
                    value_string(artist.get("id")),
                ],
                "",
            );
            let name = first(
                &[
                    value_string(artist.get("ART_NAME")),
                    value_string(artist.get("name")),
                ],
                "",
            );
            collector.push(&id, &name);
        }
    }
    if let Some(artist) = value.get("artist") {
        let id = first(
            &[
                value_string(artist.get("ART_ID")),
                value_string(artist.get("id")),
            ],
            "",
        );
        let name = first(
            &[
                value_string(artist.get("ART_NAME")),
                value_string(artist.get("name")),
            ],
            "",
        );
        collector.push(&id, &name);
    }
    if !value_string(value.get("ART_NAME")).trim().is_empty() {
        collector.push(
            &value_string(value.get("ART_ID")),
            &value_string(value.get("ART_NAME")),
        );
    }
    let artists = collector.finish();
    Track {
        origin: None,
        id: first(
            &[
                value_string(value.get("SNG_ID")),
                value_string(value.get("ID")),
                value_string(value.get("id")),
            ],
            "",
        ),
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
        artists,
        album: first(
            &[
                value_string(value.get("ALB_TITLE")),
                value_string(value.pointer("/album/title")),
                value_string(value.pointer("/album/name")),
            ],
            "",
        ),
        album_id: first(
            &[
                value_string(value.get("ALB_ID")),
                value_string(value.pointer("/album/id")),
            ],
            "",
        ),
        release_date: crate::search::release_date(value),
        duration: number(value.get("DURATION")).max(number(value.get("duration"))),
        artwork: artwork(&value_string(value.get("ALB_PICTURE")), "cover"),
        explicit: value_string(value.get("EXPLICIT_LYRICS")) == "1"
            || value_string(value.get("EXPLICIT_TRACK_CONTENT")) == "1"
            || value.get("explicit_lyrics").and_then(Value::as_bool) == Some(true)
            || title.to_ascii_lowercase().contains("explicit"),
        // Deezer track links are derived from the numeric id.
        service_url: String::new(),
    }
}

pub(crate) fn card(kind: Category, value: &Value) -> Card {
    let (id, title, picture, subtitle, badge, fallback, image_kind) = match kind {
        Category::Albums => (
            "ALB_ID",
            "ALB_TITLE",
            "ALB_PICTURE",
            value_string(value.get("ART_NAME")),
            release_year(value),
            "Untitled album",
            "cover",
        ),
        Category::Artists => (
            "ART_ID",
            "ART_NAME",
            "ART_PICTURE",
            format!(
                "{} fans",
                crate::search::format_number(number(value.get("NB_FAN")))
            ),
            String::new(),
            "Unknown artist",
            "artist",
        ),
        Category::Playlists => (
            "PLAYLIST_ID",
            "TITLE",
            "PLAYLIST_PICTURE",
            first(
                &[
                    value_string(value.get("PARENT_USERNAME")),
                    value_string(value.get("USER_NAME")),
                    value_string(value.get("CREATOR_NAME")),
                    value_string(value.get("AUTHOR")),
                    value_string(value.pointer("/USER/BLOG_NAME")),
                    value_string(value.pointer("/USER/DISPLAY_NAME")),
                    value_string(value.pointer("/CREATOR/BLOG_NAME")),
                    value_string(value.pointer("/CREATOR/DISPLAY_NAME")),
                    value_string(value.pointer("/OWNER/name")),
                    value_string(value.pointer("/OWNER/NAME")),
                ],
                "Deezer playlist",
            ),
            number(value.get("NB_SONG")).to_string(),
            "Untitled playlist",
            "playlist",
        ),
        Category::MyTracks | Category::Station | Category::Flow => (
            "",
            "",
            "",
            String::new(),
            String::new(),
            "Untitled",
            "cover",
        ),
        _ => (
            "",
            "",
            "",
            String::new(),
            String::new(),
            "Untitled",
            "cover",
        ),
    };
    let id_value = value_string(value.get(id));
    let artwork = if kind == Category::Playlists {
        crate::integrations::deezer::playlist_image_url(&id_value)
    } else {
        artwork(&value_string(value.get(picture)), image_kind)
    };
    Card {
        kind,
        id: id_value,
        title: first(&[value_string(value.get(title))], fallback),
        subtitle,
        artwork,
        release_date: release_date(value, kind),
        badge,
        source: Provider::Deezer,
        service_url: String::new(),
        is_private: if kind == Category::Playlists {
            playlist_is_private(value)
        } else {
            None
        },
        library_service: None,
    }
}

/// Deezer GW `STATUS` is `0` for public playlists and non-zero otherwise
/// (typically `1` private, `2` collaborative). GraphQL `isPrivate` wins when
/// present so library cards stay aligned with the playlist client.
fn playlist_is_private(value: &Value) -> Option<bool> {
    json_flag(value.get("isPrivate"))
        .or_else(|| json_flag(value.get("IS_PRIVATE")))
        .or_else(|| status_is_private(value.get("STATUS")))
        .or_else(|| status_is_private(value.get("status")))
        .or_else(|| privacy_label(value.get("TYPE")))
        .or_else(|| privacy_label(value.get("type")))
}

fn json_flag(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_i64().and_then(|n| u64::try_from(n).ok()))
            .map(|n| n != 0),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn status_is_private(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
            .map(|status| status != 0),
        Value::String(text) => privacy_label_text(text)
            .or_else(|| text.trim().parse::<i64>().ok().map(|status| status != 0)),
        _ => None,
    }
}

fn privacy_label(value: Option<&Value>) -> Option<bool> {
    privacy_label_text(value?.as_str()?)
}

fn privacy_label_text(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "public" => Some(false),
        "private" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reference_mappings_normalize_track_and_cards() {
        let t = track(
            &json!({"SNG_ID":"7","SNG_TITLE":"Song","VERSION":"Live","DURATION":"61","ALB_PICTURE":"h",
                "ALB_ID":"302","ALB_TITLE":"Album",
                "ARTISTS":[{"ART_ID":"11","ART_NAME":"A"},{"ART_ID":"11","ART_NAME":"A"}]}),
        );
        assert_eq!(t.title, "Song (Live)");
        assert_eq!(t.artist, "A");
        assert_eq!(
            t.artists,
            vec![crate::search::TrackArtistRef {
                id: "11".into(),
                name: "A".into()
            }]
        );
        assert_eq!(t.album_id, "302");
        assert_eq!(t.album, "Album");
        assert_eq!(t.duration, 61);
        assert!(!t.explicit);
        assert!(
            track(&json!({
                "SNG_TITLE": "Explicit Song",
                "EXPLICIT_TRACK_CONTENT": "1"
            }))
            .explicit
        );
        let c = card(
            Category::Albums,
            &json!({"ALB_ID":"2","ALB_TITLE":"Album","ART_NAME":"A","DIGITAL_RELEASE_DATE":"2024-01-01"}),
        );
        assert_eq!(c.badge, "2024");
        assert_eq!(c.release_date, "2024-01-01");

        let physical_date_album = card(
            Category::Albums,
            &json!({
                "ALB_ID":"3",
                "ALB_TITLE":"Physical",
                "PHYSICAL_RELEASE_DATE":"1984-05-18"
            }),
        );
        assert_eq!(physical_date_album.badge, "1984");
        assert_eq!(physical_date_album.release_date, "1984-05-18");

        let original_date_album = card(
            Category::Albums,
            &json!({
                "ALB_ID":"4",
                "ALB_TITLE":"Original",
                "ORIGINAL_RELEASE_DATE":"1999-07-01"
            }),
        );
        assert_eq!(original_date_album.badge, "1999");
        assert_eq!(original_date_album.release_date, "1999-07-01");

        let malformed_date_album = card(
            Category::Albums,
            &json!({
                "ALB_ID":"5",
                "ALB_TITLE":"Malformed",
                "PHYSICAL_RELEASE_DATE":"20xx-01-01"
            }),
        );
        assert_eq!(malformed_date_album.badge, "");

        let artist = card(
            Category::Artists,
            &json!({"ART_ID":"3","ART_NAME":"Artist","ART_PICTURE":"artist-hash"}),
        );
        assert_eq!(
            artist.artwork,
            "https://e-cdns-images.dzcdn.net/images/artist/artist-hash/250x250.jpg"
        );

        let playlist = card(
            Category::Playlists,
            &json!({"PLAYLIST_ID":"4","TITLE":"Rock Playlist","PARENT_USERNAME":"Deezer Editor","NB_SONG":25}),
        );
        assert_eq!(playlist.subtitle, "Deezer Editor");
        assert_eq!(playlist.badge, "25");
        assert_eq!(playlist.is_private, None);

        let fallback_playlist = card(
            Category::Playlists,
            &json!({"PLAYLIST_ID":"5","TITLE":"Custom Playlist","USER_NAME":"edoardof03","NB_SONG":0}),
        );
        assert_eq!(fallback_playlist.subtitle, "edoardof03");
    }

    #[test]
    fn deezer_album_card_to_route_keeps_full_date_for_shared_metadata() {
        let card = card(
            Category::Albums,
            &json!({
                "ALB_ID": "902",
                "ALB_TITLE": "Canonical album",
                "ART_NAME": "Artist",
                "PHYSICAL_RELEASE_DATE": "2011-06-07",
                "DIGITAL_RELEASE_DATE": "2011-06-08"
            }),
        );
        assert_eq!(card.badge, "2011");
        assert_eq!(card.release_date, "2011-06-07");
        let route = super::super::view::card_route(card).expect("album route");
        let detail = crate::search::DetailRoute {
            provider: route.source,
            kind: crate::search::ResultType::Albums,
            id: route.id,
            title: route.title,
            subtitle: route.subtitle,
            artwork: route.artwork,
            release_date: route.release_date,
            service_url: String::new(),
        };
        assert_eq!(
            crate::search::detail_metadata(&detail),
            "Artist • Jun 7, 2011"
        );
    }

    #[test]
    fn deezer_playlist_cover_uses_the_canonical_id_endpoint() {
        let hash = "db4a108e2e0578228716e7ffebdafac2-953fdae6e89776300999dab5ad10dd63-19824b123a2257af052888116467e4f5-1fd07470637104beaeada95c3514a7aa";
        let playlist = card(
            Category::Playlists,
            &json!({"PLAYLIST_ID":"13743145521","TITLE":"Generated","PLAYLIST_PICTURE":hash}),
        );

        assert_eq!(
            playlist.artwork,
            "https://api.deezer.com/playlist/13743145521/image"
        );
    }

    #[test]
    fn playlist_artwork_rejects_unsafe_hashes() {
        let playlist = card(
            Category::Playlists,
            &json!({"PLAYLIST_ID":"5","TITLE":"Unsafe","PLAYLIST_PICTURE":"../secret"}),
        );

        assert_eq!(playlist.artwork, "https://api.deezer.com/playlist/5/image");
    }

    #[test]
    fn deezer_playlist_privacy_uses_status_zero_as_public() {
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Public","STATUS":0}),
            )
            .is_private,
            Some(false)
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Public string","STATUS":"0"}),
            )
            .is_private,
            Some(false)
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Private","STATUS":1}),
            )
            .is_private,
            Some(true)
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Collaborative","STATUS":2}),
            )
            .is_private,
            Some(true)
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"GraphQL private","isPrivate":true,"STATUS":0}),
            )
            .is_private,
            Some(true)
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"GraphQL public","isPrivate":false,"STATUS":1}),
            )
            .is_private,
            Some(false)
        );
        assert_eq!(
            card(
                Category::Albums,
                &json!({"ALB_ID":"2","ALB_TITLE":"Album","STATUS":1}),
            )
            .is_private,
            None
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Numeric type is not privacy","TYPE":0}),
            )
            .is_private,
            None
        );
        assert_eq!(
            card(
                Category::Playlists,
                &json!({"PLAYLIST_ID":"4","TITLE":"Type label","TYPE":"private"}),
            )
            .is_private,
            Some(true)
        );
    }

    #[test]
    fn track_keeps_artist_object_refs_when_arrays_are_missing() {
        let value = json!({
            "SNG_ID": "7",
            "SNG_TITLE": "Song",
            "artist": {"id": "11", "name": "Primary"},
            "contributors": [{"id": "12", "name": "Collaborator"}]
        });
        let normalized = track(&value);
        assert_eq!(
            normalized.artists,
            vec![
                crate::search::TrackArtistRef {
                    id: "12".into(),
                    name: "Collaborator".into(),
                },
                crate::search::TrackArtistRef {
                    id: "11".into(),
                    name: "Primary".into(),
                },
            ]
        );
    }
}
