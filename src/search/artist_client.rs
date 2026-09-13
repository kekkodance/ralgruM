use std::collections::HashSet;

use reqwest::{Url, header};
use serde_json::{Value, json};

use super::{
    client::{SOUNDCLOUD_CLIENT_ID, SearchClient},
    credential::{DeezerArl, SoundCloudToken},
    detail::{DetailPage, DetailRoute, parse_release_date},
    models::{ArtistPage, Provider, ProviderError, ResultType},
    normalize::{normalize_card, normalize_tracks},
};

impl SearchClient {
    pub(super) async fn artist_detail(
        &self,
        mut route: DetailRoute,
        id: &str,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
    ) -> Result<DetailPage, ProviderError> {
        match route.provider {
            Provider::Deezer => {
                let page = self.deezer_artist(id, deezer_arl).await?;
                route.title = page.profile.title.clone();
                route.artwork = page.profile.artwork.clone();
                route.subtitle = page
                    .fans
                    .map(|fans| format!("{} fans", super::normalize::format_number(fans)))
                    .unwrap_or_default();
                Ok(DetailPage {
                    route,
                    tracks: Vec::new(),
                    total: None,
                    raw_loaded_count: 0,
                    normalized_count: 0,
                    authoritative_total: None,
                    artist: Some(page),
                    description: String::new(),
                    album_info: None,
                })
            }
            Provider::SoundCloud => {
                let token = soundcloud_token
                    .ok_or_else(|| ProviderError::new("SoundCloud account required"))?;
                let profile = self.soundcloud_artist_profile(id, &token);
                let tracks =
                    self.soundcloud_artist_collection(id, SoundCloudArtistResource::Tracks, &token);
                let albums =
                    self.soundcloud_artist_collection(id, SoundCloudArtistResource::Albums, &token);
                let playlists = self.soundcloud_artist_collection(
                    id,
                    SoundCloudArtistResource::Playlists,
                    &token,
                );
                // Profile and tracks are required. Album and playlist errors
                // become empty sections so the page still shows tracks.
                let required = async { futures::try_join!(profile, tracks) };
                let (required, albums, playlists) = futures::join!(required, albums, playlists);
                let (profile, tracks) = required?;
                let albums = albums.unwrap_or_default();
                let playlists = playlists.unwrap_or_default();
                let page = normalize_soundcloud_artist(&profile, tracks, albums, playlists);
                apply_soundcloud_artist_profile(&mut route, &profile);
                Ok(DetailPage {
                    total: Some(page.popular_total),
                    raw_loaded_count: page.popular_tracks.len(),
                    normalized_count: page.popular_tracks.len(),
                    authoritative_total: Some(page.popular_total),
                    tracks: Vec::new(),
                    route,
                    artist: Some(page),
                    description: String::new(),
                    album_info: None,
                })
            }
        }
    }

    async fn soundcloud_artist_profile(
        &self,
        id: &str,
        token: &SoundCloudToken,
    ) -> Result<Value, ProviderError> {
        let url = soundcloud_artist_profile_url(id)?;
        let response = self
            .http()
            .get(url)
            .header(header::AUTHORIZATION, token.authorization_header()?)
            .send()
            .await
            .map_err(profile_request_error)?;
        if !response.status().is_success() {
            return Err(ProviderError::new(format!(
                "SoundCloud returned {}",
                response.status()
            )));
        }
        response.json().await.map_err(|_| {
            ProviderError::new("SoundCloud returned an invalid artist profile response")
        })
    }

    async fn deezer_artist(
        &self,
        id: &str,
        arl: Option<DeezerArl>,
    ) -> Result<ArtistPage, ProviderError> {
        let session = self.deezer_session(arl).await?;
        let popular = self.deezer_gateway(
            &session,
            "artist.getTopTrack",
            json!({"art_id": id, "nb": 2000}),
        );
        let albums = self.deezer_gateway(
            &session,
            "album.getDiscography",
            deezer_discography_body(id, 0),
        );
        let featured = self.deezer_gateway(
            &session,
            "album.getDiscography",
            deezer_discography_body(id, 5),
        );
        let playlists = self.deezer_gateway(
            &session,
            "artist.getSelectedAndRelatedPlaylist",
            json!({"id": id, "nb": 2000}),
        );
        let profile = self.deezer_gateway(&session, "artist.getData", json!({"art_id": id}));
        let favorite = self.deezer_artist_favorite(&session, id);
        let main = async { futures::try_join!(popular, albums, featured, playlists, profile) };
        let (main, favorite) = futures::join!(main, favorite);
        let (popular, albums, featured, playlists, profile) = main?;
        let favorite = favorite.unwrap_or(None);

        let (popular, popular_total) = response_data(&popular, "artist.getTopTrack")?;
        let (album_items, albums_total) = response_data(&albums, "album.getDiscography")?;
        let (featured_items, featured_total) = response_data(&featured, "album.getDiscography")?;
        let (playlist_items, playlists_total) =
            response_data(&playlists, "artist.getSelectedAndRelatedPlaylist")?;
        let profile = profile
            .pointer("/results/DATA")
            .or_else(|| profile.pointer("/results"))
            .ok_or_else(|| {
                ProviderError::new("Deezer returned an invalid artist.getData response")
            })?;
        let ids = popular
            .iter()
            .filter_map(|track| value_string(track.get("SNG_ID")))
            .collect::<Vec<_>>();
        let hydrated = if ids.is_empty() {
            Vec::new()
        } else {
            let response = self
                .deezer_gateway(&session, "song.getListData", json!({"sng_ids": ids}))
                .await?;
            response_data(&response, "song.getListData")?.0
        };

        Ok(normalize_deezer_artist(
            profile,
            (popular, popular_total),
            hydrated,
            (album_items, albums_total),
            (featured_items, featured_total),
            (playlist_items, playlists_total),
            favorite,
        ))
    }

    async fn deezer_artist_favorite(
        &self,
        session: &super::client::DeezerSession,
        artist_id: &str,
    ) -> Result<Option<bool>, ProviderError> {
        let Some(user_id) = session.user_id.as_deref() else {
            return Ok(None);
        };
        let response = self
            .deezer_gateway(
                session,
                "deezer.pageProfile",
                json!({"user_id": user_id, "tab": "artists", "nb": 2000}),
            )
            .await?;
        Ok(parse_deezer_artist_favorite(&response, artist_id))
    }

    async fn soundcloud_artist_collection(
        &self,
        id: &str,
        resource: SoundCloudArtistResource,
        token: &SoundCloudToken,
    ) -> Result<Vec<Value>, ProviderError> {
        let context = resource.path();
        let expected_path = format!("/users/{id}/{context}");
        let mut offset = "0".to_owned();
        let mut seen_offsets = HashSet::new();
        let mut items = Vec::new();
        loop {
            if !record_offset(&mut seen_offsets, &offset) {
                return Err(ProviderError::new(format!(
                    "SoundCloud repeated the artist {context} continuation"
                )));
            }
            let url = soundcloud_artist_url(id, resource, &offset)?;
            let response = self
                .http()
                .get(url)
                .header(header::AUTHORIZATION, token.authorization_header()?)
                .send()
                .await
                .map_err(|error| collection_request_error(error, context))?;
            if !response.status().is_success() {
                return Err(ProviderError::new(format!(
                    "SoundCloud returned {}",
                    response.status()
                )));
            }
            let page: Value = response.json().await.map_err(|_| {
                ProviderError::new(format!(
                    "SoundCloud returned an invalid artist {context} response"
                ))
            })?;
            let next = soundcloud_next_offset(&page, &expected_path, context)?;
            items.extend(
                page.get("collection")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        ProviderError::new(format!(
                            "SoundCloud returned an invalid artist {context} response"
                        ))
                    })?
                    .iter()
                    .cloned(),
            );
            let Some(next) = next else { break };
            offset = next;
        }
        Ok(items)
    }
}

fn deezer_discography_body(id: &str, role: u64) -> Value {
    json!({
        "art_id": id,
        "nb": 2000,
        "nb_songs": 0,
        "filter_role_id": [role],
        "discography_mode": "all",
    })
}

fn record_offset(seen: &mut HashSet<String>, offset: &str) -> bool {
    seen.insert(offset.to_owned())
}

fn response_data(value: &Value, operation: &str) -> Result<(Vec<Value>, usize), ProviderError> {
    let items = value
        .pointer("/results/data")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| {
            ProviderError::new(format!("Deezer returned an invalid {operation} response"))
        })?;
    let total = value
        .pointer("/results/total")
        .and_then(|value| value_u64(Some(value)))
        .map_or(items.len(), |total| total as usize);
    Ok((items, total))
}

fn normalize_deezer_artist(
    profile: &Value,
    popular: (Vec<Value>, usize),
    hydrated: Vec<Value>,
    albums: (Vec<Value>, usize),
    featured: (Vec<Value>, usize),
    playlists: (Vec<Value>, usize),
    favorite: Option<bool>,
) -> ArtistPage {
    let (popular, popular_total) = popular;
    let (albums, albums_total) = albums;
    let (featured, featured_total) = featured;
    let (playlists, playlists_total) = playlists;
    let mut albums = normalize_cards(Provider::Deezer, ResultType::Albums, albums);
    let mut featured = normalize_cards(Provider::Deezer, ResultType::Albums, featured);
    sort_cards_by_release_date_desc(&mut albums);
    sort_cards_by_release_date_desc(&mut featured);
    ArtistPage {
        profile: normalize_card(Provider::Deezer, ResultType::Artists, profile),
        favorite,
        fans: value_u64(profile.get("NB_FAN")),
        popular_tracks: normalize_tracks(
            Provider::Deezer,
            &super::detail_client::merge_deezer_tracks(popular, hydrated),
        ),
        popular_total,
        similar_artists: Vec::new(),
        similar_total: 0,
        albums,
        albums_total,
        featured,
        featured_total,
        playlists: normalize_cards(Provider::Deezer, ResultType::Playlists, playlists),
        playlists_total,
    }
}

fn normalize_soundcloud_artist(
    profile: &Value,
    tracks: Vec<Value>,
    albums: Vec<Value>,
    playlists: Vec<Value>,
) -> ArtistPage {
    ArtistPage {
        profile: normalize_card(Provider::SoundCloud, ResultType::Artists, profile),
        favorite: None,
        fans: None,
        popular_total: tracks.len(),
        popular_tracks: normalize_tracks(Provider::SoundCloud, &tracks),
        similar_artists: Vec::new(),
        similar_total: 0,
        albums_total: albums.len(),
        albums: normalize_cards(Provider::SoundCloud, ResultType::Albums, albums),
        featured: Vec::new(),
        featured_total: 0,
        playlists_total: playlists.len(),
        playlists: normalize_cards(Provider::SoundCloud, ResultType::Playlists, playlists),
    }
}

fn parse_deezer_artist_favorite(value: &Value, artist_id: &str) -> Option<bool> {
    let artists = value
        .pointer("/results/TAB/artists/data")
        .and_then(Value::as_array)?;
    let artist_id = artist_id.trim();
    Some(
        artists.iter().any(|artist| {
            value_string(artist.get("ART_ID")).is_some_and(|id| id.trim() == artist_id)
        }),
    )
}

fn sort_cards_by_release_date_desc(cards: &mut [super::models::Card]) {
    cards.sort_by(|left, right| {
        match (
            parse_release_date(&left.release_date),
            parse_release_date(&right.release_date),
        ) {
            (Some(left), Some(right)) => right.cmp(&left),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

fn normalize_cards(
    provider: Provider,
    kind: ResultType,
    items: Vec<Value>,
) -> Vec<super::models::Card> {
    items
        .iter()
        .map(|item| normalize_card(provider, kind, item))
        .collect()
}

fn apply_soundcloud_artist_profile(route: &mut DetailRoute, value: &Value) {
    let profile = normalize_card(Provider::SoundCloud, ResultType::Artists, value);
    route.title = profile.title;
    route.subtitle = profile.subtitle;
    route.artwork = profile.artwork;
    // The permalink is the canonical link the card menus copy; detail menus
    // read it from the route once the profile has supplied it.
    if !profile.service_url.is_empty() {
        route.service_url = profile.service_url;
    }
}

fn value_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::to_owned)
        .or_else(|| value?.as_u64().map(|id| id.to_string()))
}

fn value_u64(value: Option<&Value>) -> Option<u64> {
    value?.as_u64().or_else(|| value?.as_str()?.parse().ok())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SoundCloudArtistResource {
    Tracks,
    Albums,
    Playlists,
}

impl SoundCloudArtistResource {
    fn path(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Playlists => "playlists",
        }
    }
}

fn soundcloud_artist_url(
    id: &str,
    resource: SoundCloudArtistResource,
    offset: &str,
) -> Result<Url, ProviderError> {
    let context = resource.path();
    let mut url = Url::parse(&format!(
        "https://api-v2.soundcloud.com/users/{id}/{context}"
    ))
    .map_err(|_| ProviderError::new(format!("Invalid SoundCloud artist {context} endpoint")))?;
    url.query_pairs_mut()
        .append_pair("client_id", SOUNDCLOUD_CLIENT_ID)
        .append_pair("limit", "200")
        .append_pair("offset", offset);
    Ok(url)
}

fn soundcloud_artist_profile_url(id: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(&format!("https://api-v2.soundcloud.com/users/{id}"))
        .map_err(|_| ProviderError::new("Invalid SoundCloud artist profile endpoint"))?;
    url.query_pairs_mut()
        .append_pair("client_id", SOUNDCLOUD_CLIENT_ID);
    Ok(url)
}

fn soundcloud_next_offset(
    response: &Value,
    expected_path: &str,
    context: &str,
) -> Result<Option<String>, ProviderError> {
    let Some(next_href) = response.get("next_href") else {
        return Ok(None);
    };
    let Some(next_href) = next_href.as_str() else {
        return if next_href.is_null() {
            Ok(None)
        } else {
            Err(ProviderError::new(format!(
                "SoundCloud returned an invalid artist {context} continuation"
            )))
        };
    };
    if next_href.is_empty() {
        return Ok(None);
    }
    let next = Url::parse(next_href).map_err(|_| {
        ProviderError::new(format!(
            "SoundCloud returned an invalid artist {context} continuation"
        ))
    })?;
    if next.scheme() != "https"
        || next.host_str() != Some("api-v2.soundcloud.com")
        || next.path() != expected_path
    {
        return Err(ProviderError::new(format!(
            "SoundCloud returned an unexpected artist {context} continuation"
        )));
    }
    next.query_pairs()
        .find_map(|(key, value)| (key == "offset" && !value.is_empty()).then(|| value.into_owned()))
        .map(Some)
        .ok_or_else(|| {
            ProviderError::new(format!(
                "SoundCloud artist {context} continuation did not include an offset"
            ))
        })
}

fn collection_request_error(error: reqwest::Error, context: &str) -> ProviderError {
    ProviderError::new(if error.is_timeout() {
        format!("SoundCloud artist {context} timed out")
    } else if error.is_connect() {
        "SoundCloud could not be reached".to_owned()
    } else {
        format!("SoundCloud artist {context} failed")
    })
}

fn profile_request_error(error: reqwest::Error) -> ProviderError {
    ProviderError::new(if error.is_timeout() {
        "SoundCloud artist profile timed out"
    } else if error.is_connect() {
        "SoundCloud could not be reached"
    } else {
        "SoundCloud artist profile failed"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deezer_artist_payloads_match_the_gateway_contract() {
        assert_eq!(
            deezer_discography_body("42", 0),
            json!({"art_id":"42","nb":2000,"nb_songs":0,"filter_role_id":[0],"discography_mode":"all"})
        );
        assert_eq!(
            deezer_discography_body("42", 5)["filter_role_id"],
            json!([5])
        );
    }

    #[test]
    fn artist_section_total_uses_gateway_total_with_item_fallback() {
        let with_total = response_data(
            &json!({"results":{"data":[{"id":1}],"total":"12"}}),
            "artist.getTopTrack",
        )
        .unwrap();
        assert_eq!(with_total.1, 12);

        let without_total = response_data(
            &json!({"results":{"data":[{"id":1},{"id":2}]}}),
            "artist.getTopTrack",
        )
        .unwrap();
        assert_eq!(without_total.1, 2);
    }

    #[test]
    fn deezer_artist_normalization_preserves_sections_and_fans() {
        let page = normalize_deezer_artist(
            &json!({"ART_ID":"42","ART_NAME":"Artist","NB_FAN":"99"}),
            (vec![json!({"SNG_ID":"1","SNG_TITLE":"Track"})], 8),
            vec![json!({"SNG_ID":"1","DURATION":"61"})],
            (vec![json!({"ALB_ID":"2","ALB_TITLE":"Album"})], 13),
            (vec![json!({"ALB_ID":"3","ALB_TITLE":"Feature"})], 14),
            (vec![json!({"PLAYLIST_ID":"4","TITLE":"Playlist"})], 15),
            None,
        );
        assert_eq!(page.fans, Some(99));
        assert_eq!(page.popular_total, 8);
        assert_eq!(page.albums_total, 13);
        assert_eq!(page.featured_total, 14);
        assert_eq!(page.playlists_total, 15);
        assert_eq!(page.popular_tracks[0].duration, 61);
        assert_eq!(page.albums[0].title, "Album");
        assert_eq!(page.featured[0].title, "Feature");
        assert_eq!(page.playlists[0].title, "Playlist");
    }

    #[test]
    fn deezer_artist_favorite_parser_distinguishes_membership_and_malformed_data() {
        let response = json!({
            "results": {
                "TAB": {
                    "artists": {
                        "data": [{"ART_ID":"42"}, {"ART_ID":84}]
                    }
                }
            }
        });

        assert_eq!(parse_deezer_artist_favorite(&response, "42"), Some(true));
        assert_eq!(parse_deezer_artist_favorite(&response, "7"), Some(false));
        assert_eq!(
            parse_deezer_artist_favorite(&json!({"results":{}}), "42"),
            None
        );
    }

    #[test]
    fn deezer_artist_albums_and_features_are_stably_sorted_by_release_date() {
        let album = |id: &str, title: &str, release_date: Option<&str>| {
            let mut value = json!({"ALB_ID":id,"ALB_TITLE":title});
            if let Some(release_date) = release_date {
                value["PHYSICAL_RELEASE_DATE"] = json!(release_date);
            }
            value
        };
        let playlist = |id: &str, title: &str, release_date: &str| {
            json!({
                "PLAYLIST_ID":id,
                "TITLE":title,
                "PHYSICAL_RELEASE_DATE":release_date
            })
        };
        let albums = vec![
            album("1", "Invalid first", Some("not-a-date")),
            album("2", "Tie A", Some("2024-06-01")),
            album("3", "Newest", Some("2025-02-03T12:34:56Z")),
            album("4", "Missing", None),
            album("5", "Tie B", Some("2024-06-01T00:00:00Z")),
            album("6", "Oldest", Some("2020-01-01")),
        ];
        let featured = vec![
            album("7", "Feature missing", None),
            album("8", "Feature tie A", Some("2023-04-05")),
            album("9", "Feature newest", Some("2026-01-02T00:00:00Z")),
            album("10", "Feature invalid", Some("2023-02-29")),
            album("11", "Feature tie B", Some("2023-04-05T09:00:00Z")),
        ];
        let playlists = vec![
            playlist("12", "Playlist older", "2020-01-01"),
            playlist("13", "Playlist newer", "2026-01-01"),
        ];

        let page = normalize_deezer_artist(
            &json!({"ART_ID":"42","ART_NAME":"Artist"}),
            (Vec::new(), 0),
            Vec::new(),
            (albums, 6),
            (featured, 5),
            (playlists, 2),
            None,
        );

        assert_eq!(
            page.albums
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Newest",
                "Tie A",
                "Tie B",
                "Oldest",
                "Invalid first",
                "Missing",
            ]
        );
        assert_eq!(
            page.featured
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            [
                "Feature newest",
                "Feature tie A",
                "Feature tie B",
                "Feature missing",
                "Feature invalid",
            ]
        );
        assert_eq!(
            page.playlists
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            ["Playlist older", "Playlist newer"]
        );
    }

    #[test]
    fn soundcloud_artist_url_builders_for_tracks_albums_and_playlists_are_exact() {
        for (resource, path) in [
            (SoundCloudArtistResource::Tracks, "/users/42/tracks"),
            (SoundCloudArtistResource::Albums, "/users/42/albums"),
            (SoundCloudArtistResource::Playlists, "/users/42/playlists"),
        ] {
            let url = soundcloud_artist_url("42", resource, "cursor:one").unwrap();
            let pairs = url.query_pairs().collect::<Vec<_>>();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("api-v2.soundcloud.com"));
            assert_eq!(url.path(), path);
            assert!(pairs.contains(&("client_id".into(), SOUNDCLOUD_CLIENT_ID.into())));
            assert!(pairs.contains(&("limit".into(), "200".into())));
            assert!(pairs.contains(&("offset".into(), "cursor:one".into())));
        }
    }

    #[test]
    fn soundcloud_artist_continuation_rejects_wrong_hosts_and_paths() {
        for (resource, path) in [
            (SoundCloudArtistResource::Tracks, "/users/42/tracks"),
            (SoundCloudArtistResource::Albums, "/users/42/albums"),
            (SoundCloudArtistResource::Playlists, "/users/42/playlists"),
        ] {
            let context = resource.path();
            let valid = json!({
                "next_href": format!(
                    "https://api-v2.soundcloud.com{path}?offset=next%3Aone"
                )
            });
            assert_eq!(
                soundcloud_next_offset(&valid, path, context).unwrap(),
                Some("next:one".into())
            );
            for href in [
                format!("http://api-v2.soundcloud.com{path}?offset=x"),
                format!("https://evil.invalid{path}?offset=x"),
                format!("https://api-v2.soundcloud.com/users/43/{context}?offset=x"),
            ] {
                assert!(
                    soundcloud_next_offset(&json!({"next_href": href}), path, context).is_err()
                );
            }
        }
        assert!(
            soundcloud_next_offset(
                &json!({
                    "next_href": "https://api-v2.soundcloud.com/users/42/albums?offset=x"
                }),
                "/users/42/tracks",
                "tracks",
            )
            .is_err()
        );
    }

    #[test]
    fn soundcloud_artist_page_maps_collections_and_leaves_fans_unset() {
        let page = normalize_soundcloud_artist(
            &json!({
                "id": "55",
                "username": "TRVCY",
                "followers_count": 12345,
                "avatar_url": "https://example.com/trvcy-large.jpg"
            }),
            vec![json!({
                "id": "9",
                "title": "Song",
                "user": {"id": "55", "username": "TRVCY"}
            })],
            vec![json!({
                "id": "2",
                "title": "Album",
                "is_album": true,
                "user": {"username": "TRVCY"}
            })],
            vec![json!({
                "id": "3",
                "title": "Playlist",
                "is_album": false,
                "track_count": 4,
                "user": {"username": "TRVCY"}
            })],
        );

        assert_eq!(page.fans, None);
        assert_eq!(page.favorite, None);
        assert_eq!(page.profile.title, "TRVCY");
        assert_eq!(page.profile.subtitle, "12,345 followers");
        assert_eq!(page.popular_tracks.len(), 1);
        assert_eq!(page.popular_tracks[0].title, "Song");
        assert_eq!(page.popular_total, 1);
        assert_eq!(page.albums.len(), 1);
        assert_eq!(page.albums[0].kind, ResultType::Albums);
        assert_eq!(page.albums[0].title, "Album");
        assert_eq!(page.albums_total, 1);
        assert_eq!(page.playlists.len(), 1);
        assert_eq!(page.playlists[0].kind, ResultType::Playlists);
        assert_eq!(page.playlists[0].title, "Playlist");
        assert_eq!(page.playlists_total, 1);
        assert!(page.similar_artists.is_empty());
        assert_eq!(page.similar_total, 0);
        assert!(page.featured.is_empty());
        assert_eq!(page.featured_total, 0);
    }

    #[test]
    fn soundcloud_artist_profile_request_is_exact() {
        let url = soundcloud_artist_profile_url("55").unwrap();
        let pairs = url.query_pairs().collect::<Vec<_>>();

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("api-v2.soundcloud.com"));
        assert_eq!(url.path(), "/users/55");
        assert_eq!(
            pairs,
            vec![("client_id".into(), SOUNDCLOUD_CLIENT_ID.into())]
        );
    }

    #[test]
    fn soundcloud_profile_refreshes_stale_artist_route_metadata() {
        let mut route = DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Artists,
            id: "55".into(),
            title: "Skrillex".into(),
            subtitle: "Old subtitle".into(),
            artwork: "https://example.com/old.jpg".into(),
            release_date: String::new(),
            service_url: String::new(),
        };

        apply_soundcloud_artist_profile(
            &mut route,
            &json!({
                "id": "55",
                "username": "TRVCY",
                "track_count": 17,
                "followers_count": 12345,
                "avatar_url": "https://example.com/trvcy-large.jpg"
            }),
        );

        assert_eq!(route.title, "TRVCY");
        assert_eq!(route.subtitle, "12,345 followers");
        assert_eq!(route.artwork, "https://example.com/trvcy-t500x500.jpg");
    }

    #[test]
    fn page_collections_preserve_soundcloud_order() {
        let mut output = Vec::new();
        output.extend([json!({"id":3}), json!({"id":1})]);
        output.extend([json!({"id":2})]);
        assert_eq!(
            output
                .iter()
                .map(|item| item["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }

    #[test]
    fn repeated_soundcloud_artist_offsets_are_rejected() {
        let mut seen = HashSet::new();
        assert!(record_offset(&mut seen, "0"));
        assert!(record_offset(&mut seen, "cursor"));
        assert!(!record_offset(&mut seen, "cursor"));
    }
}
