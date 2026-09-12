use reqwest::{Url, header};
use serde_json::{Value, json};

use super::model::{DiscoverAction, DiscoverItem, DiscoverSection};
use crate::search::credential::SoundCloudToken;
use crate::search::models::{Card, Provider, ResultType};
use crate::search::{SearchClient, normalize, release_date, soundcloud_service_url};

const API: &str = "https://api-v2.soundcloud.com/mixed-selections";
const LIMIT: &str = "10";
const OFFSET: &str = "0";
const LINKED_PARTITIONING: &str = "1";
const APP_VERSION: &str = "1788853345";
const APP_LOCALE: &str = "en";
const SYSTEM_PLAYLIST_TRACK_LIMIT: usize = 500;

pub(crate) async fn load(
    client: &SearchClient,
    token: Option<SoundCloudToken>,
) -> Result<Vec<DiscoverSection>, String> {
    let url = request_url().map_err(str::to_owned)?;
    let mut request = client.http().get(url);
    if let Some(token) = token.as_ref() {
        request = request.header(
            header::AUTHORIZATION,
            token
                .authorization_header()
                .map_err(|error| error.message)?,
        );
    }
    let response = request
        .send()
        .await
        .map_err(|_| "SoundCloud Discover request failed".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("SoundCloud returned {}", response.status()));
    }
    let value = response
        .json::<Value>()
        .await
        .map_err(|_| "SoundCloud returned an invalid Discover response".to_owned())?;
    parse_home(&value)
}

pub(crate) fn request_url() -> Result<Url, &'static str> {
    let mut url = Url::parse(API).map_err(|_| "Invalid SoundCloud Discover endpoint")?;
    url.query_pairs_mut()
        .append_pair("client_id", crate::search::client::SOUNDCLOUD_CLIENT_ID)
        .append_pair("limit", LIMIT)
        .append_pair("offset", OFFSET)
        .append_pair("linked_partitioning", LINKED_PARTITIONING)
        .append_pair("app_version", APP_VERSION)
        .append_pair("app_locale", APP_LOCALE);
    Ok(url)
}

pub(crate) fn parse_home(value: &Value) -> Result<Vec<DiscoverSection>, String> {
    let sections = value
        .get("collection")
        .and_then(Value::as_array)
        .ok_or_else(|| "SoundCloud returned an invalid Discover response".to_owned())?;
    let mut parsed = Vec::new();
    for (index, section) in sections.iter().enumerate() {
        let title = string(section.get("title"));
        if excluded_section(&title) {
            continue;
        }
        let Some(items) = section
            .pointer("/items/collection")
            .and_then(Value::as_array)
        else {
            continue;
        };
        let items = items.iter().filter_map(parse_item).collect::<Vec<_>>();
        if title.trim().is_empty() || items.is_empty() {
            continue;
        }
        parsed.push(DiscoverSection {
            id: format!("soundcloud:{index}:{}", stable_part(&title)),
            provider: Provider::SoundCloud,
            title,
            subtitle: String::new(),
            items,
        });
    }
    Ok(parsed)
}

pub(crate) fn excluded_section(title: &str) -> bool {
    title.trim().eq_ignore_ascii_case("recently played")
}

fn parse_item(item: &Value) -> Option<DiscoverItem> {
    match item.get("kind").and_then(Value::as_str) {
        Some("playlist") => parse_regular_playlist(item),
        Some("system-playlist") => parse_system_playlist(item),
        _ => None,
    }
}

fn parse_regular_playlist(item: &Value) -> Option<DiscoverItem> {
    let id = string(item.get("id"));
    if !valid_id(&id) {
        return None;
    }
    let kind = if item.get("is_album").and_then(Value::as_bool) == Some(true)
        || string(item.get("set_type")).eq_ignore_ascii_case("album")
    {
        ResultType::Albums
    } else {
        ResultType::Playlists
    };
    let mut card = normalize::normalize_card(Provider::SoundCloud, kind, item);
    card.id = id;
    card.title = first_string(&[string(item.get("title"))]).unwrap_or_else(|| {
        if kind == ResultType::Albums {
            "Untitled album".to_owned()
        } else {
            "Untitled playlist".to_owned()
        }
    });
    card.subtitle = user_name(item).unwrap_or_else(|| "SoundCloud".to_owned());
    if let Some(artwork) = artwork_from_item(item) {
        card.artwork = artwork;
    }
    card.release_date = release_date(item);
    card.badge = if kind == ResultType::Albums {
        year(&card.release_date)
    } else {
        let count = item
            .get("track_count")
            .and_then(number)
            .or_else(|| {
                item.pointer("/tracks")
                    .and_then(Value::as_array)
                    .map(|items| items.len() as u64)
            })
            .unwrap_or_default();
        count.to_string()
    };
    card.service_url = soundcloud_service_url(item);
    (!card.service_url.is_empty()).then_some(())?;
    Some(DiscoverItem {
        card,
        action: DiscoverAction::OpenDetail,
    })
}

fn parse_system_playlist(item: &Value) -> Option<DiscoverItem> {
    let id = first_string(&[
        string(item.get("id")),
        string(item.get("urn")),
        string(item.get("query_urn")),
    ])?;
    let title = first_string(&[string(item.get("title")), string(item.get("short_title"))])
        .unwrap_or_else(|| "SoundCloud selection".to_owned());
    let subtitle = user_name(item)
        .or_else(|| first_string(&[string(item.get("short_description"))]))
        .unwrap_or_else(|| "SoundCloud".to_owned());
    let card = Card {
        kind: ResultType::Playlists,
        id,
        title,
        subtitle,
        artwork: artwork_from_item(item).unwrap_or_default(),
        source: Provider::SoundCloud,
        badge: item
            .pointer("/tracks")
            .and_then(Value::as_array)
            .map(|tracks| tracks.len().to_string())
            .unwrap_or_default(),
        ..Card::default()
    };
    let track_ids = system_playlist_track_ids(item);
    Some(DiscoverItem {
        card,
        action: if !track_ids.is_empty() {
            DiscoverAction::OpenSoundCloudSelection(track_ids)
        } else {
            DiscoverAction::None
        },
    })
}

fn system_playlist_track_ids(item: &Value) -> Vec<String> {
    let Some(tracks) = item.get("tracks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for id in tracks
        .iter()
        .map(|track| string(track.get("id")))
        .filter(|id| !id.is_empty())
    {
        if !valid_id(&id) || ids.iter().any(|existing| existing == &id) {
            continue;
        }
        ids.push(id);
        if ids.len() == SYSTEM_PLAYLIST_TRACK_LIMIT {
            break;
        }
    }
    ids
}

fn artwork_from_item(item: &Value) -> Option<String> {
    [
        item.get("artwork_url"),
        item.get("calculated_artwork_url"),
        item.pointer("/tracks/0/artwork_url"),
    ]
    .into_iter()
    .find_map(artwork_from_value)
}

fn artwork_from_value(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Array(values) => values
            .iter()
            .find_map(|value| artwork_from_value(Some(value))),
        Value::String(value) => soundcloud_artwork(value),
        _ => None,
    }
}

fn soundcloud_artwork(value: &str) -> Option<String> {
    let normalized = normalize::soundcloud_artwork(&json!({ "artwork_url": value }));
    if normalized.is_empty() {
        return None;
    }
    let url = Url::parse(&normalized).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    (host == "sndcdn.com" || host.ends_with(".sndcdn.com")).then_some(normalized)
}

fn user_name(item: &Value) -> Option<String> {
    first_string(&[
        string(item.pointer("/user/username")),
        string(item.pointer("/user/full_name")),
    ])
}

fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

fn valid_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn year(value: &str) -> String {
    value
        .get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_default()
        .to_owned()
}

fn stable_part(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
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

fn first_string(values: &[String]) -> Option<String> {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_matches_captured_mixed_selection_contract() {
        let url = request_url().unwrap();
        assert_eq!(url.path(), "/mixed-selections");
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                (
                    "client_id".into(),
                    crate::search::client::SOUNDCLOUD_CLIENT_ID.into()
                ),
                ("limit".into(), "10".into()),
                ("offset".into(), "0".into()),
                ("linked_partitioning".into(), "1".into()),
                ("app_version".into(), "1788853345".into()),
                ("app_locale".into(), "en".into()),
            ]
        );
    }

    #[test]
    fn parser_excludes_recent_and_keeps_regular_and_system_items() {
        let value = json!({
            "collection": [
                { "title": "Recently Played", "items": { "collection": [{"kind":"playlist","id":1}] } },
                { "title": "Useful", "items": { "collection": [
                    { "kind":"playlist", "id":42, "title":"Album", "set_type":"album", "is_album":true, "track_count":9, "release_date":"2024-05-06", "permalink_url":"https://soundcloud.com/user/album", "artwork_url":["https://i1.sndcdn.com/artworks-large.jpg"], "user":{"username":"user"} },
                    { "kind":"system-playlist", "urn":"soundcloud:system:one", "title":"Station", "short_description":"For you", "calculated_artwork_url":"https://i1.sndcdn.com/system-large.jpg", "tracks":[{"id":7,"artwork_url":"https://i1.sndcdn.com/track-large.jpg"},{"id":"7"},{"id":8},{"id":"bad"}] }
                ] } }
            ]
        });
        let sections = parse_home(&value).unwrap();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].items.len(), 2);
        assert_eq!(sections[0].items[0].action, DiscoverAction::OpenDetail);
        assert_eq!(sections[0].items[0].card.kind, ResultType::Albums);
        assert_eq!(sections[0].items[0].card.badge, "2024");
        assert_eq!(sections[0].items[0].card.subtitle, "user");
        assert_eq!(
            sections[0].items[1].action,
            DiscoverAction::OpenSoundCloudSelection(vec!["7".into(), "8".into()])
        );
        assert_eq!(sections[0].items[1].card.title, "Station");
    }

    #[test]
    fn system_playlist_without_tracks_remains_non_actionable() {
        let item = json!({
            "kind": "system-playlist",
            "urn": "soundcloud:system:one",
            "title": "Station"
        });
        let parsed = parse_system_playlist(&item).unwrap();
        assert_eq!(parsed.action, DiscoverAction::None);
    }

    #[test]
    fn system_playlist_track_ids_are_validated_deduplicated_and_bounded() {
        let mut tracks = (1..=SYSTEM_PLAYLIST_TRACK_LIMIT + 1)
            .map(|id| json!({ "id": id }))
            .collect::<Vec<_>>();
        tracks.insert(0, json!({ "id": "123456789012345678901234567890123" }));
        tracks.insert(1, json!({ "id": "1" }));
        let item = json!({ "tracks": tracks });

        let ids = system_playlist_track_ids(&item);

        assert_eq!(ids.len(), SYSTEM_PLAYLIST_TRACK_LIMIT);
        assert_eq!(ids.first().map(String::as_str), Some("1"));
        assert_eq!(
            ids.last().cloned(),
            Some(SYSTEM_PLAYLIST_TRACK_LIMIT.to_string())
        );
    }

    #[test]
    fn system_playlists_without_a_stable_identifier_are_rejected() {
        let value = json!({
            "collection": [{
                "title": "Useful",
                "items": {"collection": [{
                    "kind": "system-playlist",
                    "title": "Missing id"
                }]}
            }]
        });
        assert!(parse_home(&value).unwrap().is_empty());
    }

    #[test]
    fn artwork_accepts_sndcdn_strings_and_arrays_but_rejects_foreign_hosts() {
        assert_eq!(
            artwork_from_value(Some(&json!("https://i1.sndcdn.com/artworks-large.jpg"))),
            Some("https://i1.sndcdn.com/artworks-t500x500.jpg".into())
        );
        assert_eq!(
            artwork_from_value(Some(&json!([
                "https://example.com/nope.jpg",
                "https://i1.sndcdn.com/artworks-large.jpg"
            ]))),
            Some("https://i1.sndcdn.com/artworks-t500x500.jpg".into())
        );
        assert!(artwork_from_value(Some(&json!("https://example.com/nope.jpg"))).is_none());
    }
}
