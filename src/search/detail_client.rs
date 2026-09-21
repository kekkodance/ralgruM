use std::collections::{HashMap, HashSet};

use futures::{StreamExt, TryStreamExt, stream};
use reqwest::{Url, header};
use serde_json::{Value, json};

use super::{
    client::{SOUNDCLOUD_CLIENT_ID, SearchClient, deezer_json},
    credential::{DeezerArl, SoundCloudToken},
    detail::{DetailPage, DetailRoute, validate_id},
    models::{Provider, ProviderError, ResultType, Track},
    normalize::{normalize_tracks, soundcloud_artwork},
};

const SOUNDCLOUD_TRACK_HYDRATION_LIMIT: usize = 500;
const SOUNDCLOUD_TRACK_HYDRATION_CONCURRENCY: usize = 4;
const DEEZER_COLLECTION_PAGE_SIZE: usize = 500;
const DEEZER_TRACK_HYDRATION_CONCURRENCY: usize = 4;

impl SearchClient {
    pub(crate) async fn detail(
        &self,
        mut route: DetailRoute,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
    ) -> Result<DetailPage, ProviderError> {
        let id = validate_id(&route.id)?;
        if route.kind == ResultType::Artists {
            return self
                .artist_detail(route, &id, deezer_arl, soundcloud_token)
                .await;
        }
        let mut ai_generated = None;
        let (items, total, raw_loaded_count, authoritative_total, description, album_info) =
            match route.provider {
                Provider::Deezer => {
                    let ai_arl = deezer_arl.clone();
                    let session = self.deezer_session(deezer_arl).await?;
                    let (detail, album_info, album_ai_generated) = tokio::join!(
                        self.deezer_detail(&route, &id, &session),
                        async {
                            match route.kind {
                                ResultType::Albums => {
                                    self.deezer_album_info(&id, &session).await.ok()
                                }
                                ResultType::Playlists => {
                                    self.deezer_playlist_info(&id, &session).await.ok()
                                }
                                _ => None,
                            }
                        },
                        async {
                            if route.kind != ResultType::Albums {
                                return None;
                            }
                            let arl = ai_arl?;
                            tokio::time::timeout(
                                std::time::Duration::from_secs(4),
                                self.deezer_ai_content(vec![id.clone()], arl),
                            )
                            .await
                            .ok()?
                            .ok()?
                            .get(&id)
                            .copied()
                        }
                    );
                    let (items, total) = detail?;
                    let raw_loaded_count = items.len();
                    if let Some(info) = album_info.as_ref() {
                        apply_deezer_metadata(&mut route, info);
                    }
                    if album_ai_generated.is_some() {
                        ai_generated = album_ai_generated;
                    }
                    (
                        items,
                        total,
                        raw_loaded_count,
                        total,
                        String::new(),
                        album_info,
                    )
                }
                Provider::SoundCloud => {
                    let token = soundcloud_token
                        .ok_or_else(|| ProviderError::new("SoundCloud account required"))?;
                    let detail = self.soundcloud_detail(&id, token).await?;
                    apply_soundcloud_metadata(&mut route, &detail.playlist);
                    let description = rich_text_to_plain(
                        detail
                            .playlist
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                    let album_info =
                        if matches!(route.kind, ResultType::Albums | ResultType::Playlists) {
                            Some(super::album_info::soundcloud_album_info(
                                &detail.playlist,
                                &description,
                            ))
                        } else {
                            None
                        };
                    let total = detail.tracks.len();
                    let raw_loaded_count = detail.tracks.len();
                    (
                        detail.tracks,
                        Some(total),
                        raw_loaded_count,
                        Some(total),
                        description,
                        album_info,
                    )
                }
            };
        let mut tracks = normalize_tracks(route.provider, &items);
        if ai_generated == Some(true) {
            for track in &mut tracks {
                track.ai_generated = true;
            }
        }
        apply_track_metadata_fallback(&mut route, &tracks);
        let tracks = soundcloud_album_tracks(tracks, &route);
        let normalized_count = tracks.len();
        Ok(DetailPage {
            tracks,
            route,
            raw_loaded_count,
            normalized_count,
            authoritative_total,
            total,
            artist: None,
            description,
            album_info,
        })
    }

    async fn deezer_detail(
        &self,
        route: &DetailRoute,
        id: &str,
        session: &super::client::DeezerSession,
    ) -> Result<(Vec<Value>, Option<usize>), ProviderError> {
        let (mut tracks, total) = self
            .deezer_collection_pages(route.kind, id, session)
            .await?;
        if route.kind != ResultType::Albums || tracks.is_empty() {
            return Ok((tracks, total));
        }
        let ids = tracks
            .iter()
            .filter_map(|track| track.get("SNG_ID"))
            .filter_map(value_string)
            .collect::<Vec<_>>();
        let hydrated = self.deezer_hydrate_tracks(session, ids).await?;
        tracks = merge_deezer_tracks(tracks, hydrated);
        Ok((tracks, total))
    }

    async fn deezer_collection_pages(
        &self,
        kind: ResultType,
        id: &str,
        session: &super::client::DeezerSession,
    ) -> Result<(Vec<Value>, Option<usize>), ProviderError> {
        let mut start = 0usize;
        let mut tracks = Vec::new();
        let mut total = None;
        loop {
            let (operation, body) = deezer_collection_request(kind, id, start)?;
            let response = self.deezer_gateway(session, operation, body).await?;
            let page = response
                .pointer("/results/data")
                .and_then(Value::as_array)
                .cloned()
                .ok_or_else(|| {
                    ProviderError::new("Deezer returned an invalid collection response")
                })?;
            let page_total = response
                .pointer("/results/total")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            total = total.or(page_total);
            let page_len = page.len();
            tracks.extend(page);

            let expected_total = total.unwrap_or(tracks.len());
            if tracks.len() >= expected_total || page_len < DEEZER_COLLECTION_PAGE_SIZE {
                return Ok((tracks, total));
            }
            if page_len == 0 {
                return Err(ProviderError::new(
                    "Deezer returned an incomplete collection response",
                ));
            }
            start = tracks.len();
        }
    }

    async fn deezer_hydrate_tracks(
        &self,
        session: &super::client::DeezerSession,
        ids: Vec<String>,
    ) -> Result<Vec<Value>, ProviderError> {
        let chunks = ids
            .chunks(DEEZER_COLLECTION_PAGE_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        stream::iter(chunks.into_iter().map(|ids| {
            let client = self.clone();
            let session = session.clone();
            async move {
                let response = client
                    .deezer_gateway(&session, "song.getListData", json!({"sng_ids": ids}))
                    .await?;
                response
                    .pointer("/results/data")
                    .and_then(Value::as_array)
                    .cloned()
                    .ok_or_else(|| ProviderError::new("Deezer returned an invalid track response"))
            }
        }))
        .buffered(DEEZER_TRACK_HYDRATION_CONCURRENCY)
        .try_collect::<Vec<Vec<Value>>>()
        .await
        .map(|pages| pages.into_iter().flatten().collect())
    }

    /// Fetches album metadata for the "About this album" popover. Failures are
    /// tolerated by the caller so the track list still renders.
    async fn deezer_album_info(
        &self,
        id: &str,
        session: &super::client::DeezerSession,
    ) -> Result<super::album_info::AlbumInfo, ProviderError> {
        let album = self
            .deezer_gateway(session, "album.getData", json!({"alb_id": id}))
            .await?;
        let album = album
            .get("results")
            .cloned()
            .ok_or_else(|| ProviderError::new("Deezer returned an invalid album response"))?;
        Ok(super::album_info::parse_deezer_album_info(&album))
    }

    async fn deezer_playlist_info(
        &self,
        id: &str,
        session: &super::client::DeezerSession,
    ) -> Result<super::album_info::AlbumInfo, ProviderError> {
        let playlist = self
            .deezer_gateway(
                session,
                "deezer.pagePlaylist",
                json!({"playlist_id": id, "lang": "en", "nb": 20}),
            )
            .await?;
        let playlist = playlist
            .get("results")
            .cloned()
            .ok_or_else(|| ProviderError::new("Deezer returned an invalid playlist response"))?;
        Ok(super::album_info::parse_deezer_playlist_info(&playlist))
    }

    pub(super) async fn deezer_gateway(
        &self,
        session: &super::client::DeezerSession,
        operation: &str,
        body: Value,
    ) -> Result<Value, ProviderError> {
        let cookie = session.request_cookie()?;
        let mut url = Url::parse("https://www.deezer.com/ajax/gw-light.php")
            .map_err(|_| ProviderError::new("Invalid Deezer collection endpoint"))?;
        url.query_pairs_mut()
            .append_pair("method", operation)
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", &session.check_form);
        // The arl segment keeps the merged cookie non-empty, but a defensive
        // check keeps an empty merge from sending a malformed COOKIE header.
        let mut request = self
            .http()
            .post(url)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8");
        if !cookie.as_bytes().is_empty() {
            request = request.header(header::COOKIE, cookie);
        }
        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|error| request_error(error, "Deezer"))?;
        deezer_json(response).await
    }

    async fn soundcloud_detail(
        &self,
        id: &str,
        token: SoundCloudToken,
    ) -> Result<SoundCloudDetail, ProviderError> {
        let mut url = Url::parse(&format!("https://api-v2.soundcloud.com/playlists/{id}"))
            .map_err(|_| ProviderError::new("Invalid SoundCloud collection endpoint"))?;
        url.query_pairs_mut()
            .append_pair("client_id", SOUNDCLOUD_CLIENT_ID)
            .append_pair("representation", "owner");
        let response = self
            .http()
            .get(url)
            .header(header::AUTHORIZATION, token.authorization_header()?)
            .send()
            .await
            .map_err(|error| request_error(error, "SoundCloud"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::new(format!("SoundCloud returned {status}")));
        }
        let playlist: Value = crate::provider_response::json(response)
            .await
            .map_err(|_| ProviderError::new("SoundCloud returned an invalid response"))?;
        let embedded = playlist.get("tracks").and_then(Value::as_array);
        let embedded_complete = embedded.is_some_and(|tracks| {
            playlist
                .get("track_count")
                .and_then(value_usize)
                .is_none_or(|count| count <= tracks.len())
        });
        let source_tracks = if embedded_complete {
            embedded.cloned().unwrap_or_default()
        } else {
            self.soundcloud_playlist_tracks(id, &token).await?
        };
        let ids = source_tracks
            .iter()
            .map(soundcloud_track_id)
            .collect::<Result<Vec<_>, _>>()?;
        let hydrated = self.hydrate_soundcloud_track_values(&ids, &token).await?;
        Ok(SoundCloudDetail {
            tracks: ordered_soundcloud_tracks(&ids, hydrated),
            playlist,
        })
    }

    pub(crate) async fn hydrate_soundcloud_track_values(
        &self,
        track_ids: &[String],
        token: &SoundCloudToken,
    ) -> Result<Vec<Value>, ProviderError> {
        self.hydrate_soundcloud_track_values_inner(track_ids, token)
            .await
    }

    pub(crate) async fn hydrate_soundcloud_discover_track_values(
        &self,
        track_ids: &[String],
        token: &SoundCloudToken,
    ) -> Result<Vec<Value>, ProviderError> {
        if track_ids.len() > SOUNDCLOUD_TRACK_HYDRATION_LIMIT {
            return Err(ProviderError::new(
                "SoundCloud selection contains too many tracks",
            ));
        }
        self.hydrate_soundcloud_track_values_inner(track_ids, token)
            .await
    }

    async fn hydrate_soundcloud_track_values_inner(
        &self,
        track_ids: &[String],
        token: &SoundCloudToken,
    ) -> Result<Vec<Value>, ProviderError> {
        let ids = track_ids
            .iter()
            .map(|id| validate_id(id))
            .collect::<Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let authorization = token.authorization_header()?;
        let chunks = ids
            .chunks(50)
            .map(|chunk| chunk.to_owned())
            .collect::<Vec<_>>();
        let hydrated = stream::iter(chunks.into_iter().map(|chunk| {
            let authorization = authorization.clone();
            async move {
                let mut url = Url::parse("https://api-v2.soundcloud.com/tracks")
                    .map_err(|_| ProviderError::new("Invalid SoundCloud track endpoint"))?;
                url.query_pairs_mut()
                    .append_pair("client_id", SOUNDCLOUD_CLIENT_ID)
                    .append_pair("ids", &chunk.join(","));
                let response = self
                    .http()
                    .get(url)
                    .header(header::AUTHORIZATION, authorization.clone())
                    .send()
                    .await
                    .map_err(|error| request_error(error, "SoundCloud"))?;
                if !response.status().is_success() {
                    return Err(ProviderError::new(format!(
                        "SoundCloud returned {}",
                        response.status()
                    )));
                }
                crate::provider_response::json::<Vec<Value>>(response)
                    .await
                    .map_err(|_| {
                        ProviderError::new("SoundCloud returned an invalid track response")
                    })
            }
        }))
        .buffered(SOUNDCLOUD_TRACK_HYDRATION_CONCURRENCY)
        .try_collect::<Vec<Vec<Value>>>()
        .await?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        Ok(ordered_soundcloud_tracks(&ids, hydrated))
    }

    async fn soundcloud_playlist_tracks(
        &self,
        id: &str,
        token: &SoundCloudToken,
    ) -> Result<Vec<Value>, ProviderError> {
        let endpoint = format!("https://api-v2.soundcloud.com/playlists/{id}/tracks");
        let expected_path = format!("/playlists/{id}/tracks");
        let mut offset = "0".to_owned();
        let mut seen_offsets = HashSet::new();
        let mut tracks = Vec::new();
        loop {
            if !seen_offsets.insert(offset.clone()) {
                return Err(ProviderError::new(
                    "SoundCloud repeated the playlist tracks continuation",
                ));
            }
            let mut url = Url::parse(&endpoint)
                .map_err(|_| ProviderError::new("Invalid SoundCloud playlist tracks endpoint"))?;
            url.query_pairs_mut()
                .append_pair("client_id", SOUNDCLOUD_CLIENT_ID)
                .append_pair("limit", "200")
                .append_pair("offset", &offset);
            let response = self
                .http()
                .get(url)
                .header(header::AUTHORIZATION, token.authorization_header()?)
                .send()
                .await
                .map_err(|error| request_error(error, "SoundCloud"))?;
            if !response.status().is_success() {
                return Err(ProviderError::new(format!(
                    "SoundCloud returned {}",
                    response.status()
                )));
            }
            let page: Value = crate::provider_response::json(response)
                .await
                .map_err(|_| {
                    ProviderError::new("SoundCloud returned an invalid playlist tracks response")
                })?;
            let next = soundcloud_next_offset(&page, &expected_path)?;
            let collection = page
                .get("collection")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ProviderError::new("SoundCloud returned an invalid playlist tracks response")
                })?;
            tracks.extend(collection.iter().cloned());
            let Some(next) = next else { break };
            offset = next;
        }
        Ok(tracks)
    }
}

fn deezer_collection_request(
    kind: ResultType,
    id: &str,
    start: usize,
) -> Result<(&'static str, Value), ProviderError> {
    match kind {
        ResultType::Albums => Ok((
            "song.getListByAlbum",
            json!({"alb_id": id, "start": start, "nb": DEEZER_COLLECTION_PAGE_SIZE}),
        )),
        ResultType::Playlists => Ok((
            "playlist.getSongs",
            json!({"playlist_id": id, "start": start, "nb": DEEZER_COLLECTION_PAGE_SIZE}),
        )),
        _ => Err(ProviderError::new("Unsupported Deezer collection")),
    }
}

fn apply_deezer_metadata(route: &mut DetailRoute, info: &super::album_info::AlbumInfo) {
    if let Some(title) = nonempty_copy(&info.title) {
        route.title = title;
    }
    let subtitle = match route.kind {
        ResultType::Albums => {
            nonempty_copy(&info.album_artist).or_else(|| nonempty_copy(&info.artists))
        }
        ResultType::Playlists => nonempty_copy(&info.artists),
        _ => None,
    };
    if let Some(subtitle) = subtitle {
        route.subtitle = subtitle;
    }
    if let Some(release_date) = nonempty_copy(&info.release_date) {
        route.release_date = release_date;
    }
}

fn nonempty_copy(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn apply_track_metadata_fallback(route: &mut DetailRoute, tracks: &[super::models::Track]) {
    if route.artwork.trim().is_empty()
        && let Some(artwork) = tracks
            .iter()
            .map(|track| track.artwork.trim())
            .find(|artwork| !artwork.is_empty())
    {
        route.artwork = artwork.to_owned();
    }

    if route.kind != ResultType::Albums {
        return;
    }
    if route.title.trim().is_empty()
        && let Some(title) = tracks
            .iter()
            .map(|track| track.album.trim())
            .find(|title| !title.is_empty())
    {
        route.title = title.to_owned();
    }
    if route.subtitle.trim().is_empty()
        && let Some(artist) = tracks
            .iter()
            .map(|track| track.artist.trim())
            .find(|artist| !artist.is_empty() && *artist != "Unknown Artist")
    {
        route.subtitle = artist.to_owned();
    }
    if route.release_date.trim().is_empty()
        && let Some(release_date) = tracks
            .iter()
            .map(|track| track.release_date.trim())
            .find(|release_date| !release_date.is_empty())
    {
        route.release_date = release_date.to_owned();
    }
}

struct SoundCloudDetail {
    tracks: Vec<Value>,
    playlist: Value,
}

fn value_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn value_usize(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn soundcloud_track_id(track: &Value) -> Result<String, ProviderError> {
    track
        .get("id")
        .and_then(value_string)
        .or_else(|| {
            track
                .get("urn")
                .and_then(Value::as_str)
                .and_then(|urn| urn.strip_prefix("soundcloud:tracks:"))
                .map(str::to_owned)
        })
        .and_then(|id| validate_id(&id).ok())
        .ok_or_else(|| {
            ProviderError::new("SoundCloud returned a playlist track without a valid ID")
        })
}

fn ordered_soundcloud_tracks(ids: &[String], tracks: Vec<Value>) -> Vec<Value> {
    let by_id = tracks
        .into_iter()
        .filter_map(|track| soundcloud_track_id(&track).ok().map(|id| (id, track)))
        .collect::<HashMap<_, _>>();
    ids.iter().filter_map(|id| by_id.get(id).cloned()).collect()
}

fn soundcloud_next_offset(
    response: &Value,
    expected_path: &str,
) -> Result<Option<String>, ProviderError> {
    let Some(next_href) = response.get("next_href") else {
        return Ok(None);
    };
    let Some(next_href) = next_href.as_str() else {
        return if next_href.is_null() {
            Ok(None)
        } else {
            Err(ProviderError::new(
                "SoundCloud returned an invalid playlist tracks continuation",
            ))
        };
    };
    if next_href.is_empty() {
        return Ok(None);
    }
    let next = Url::parse(next_href).map_err(|_| {
        ProviderError::new("SoundCloud returned an invalid playlist tracks continuation")
    })?;
    if next.scheme() != "https"
        || next.host_str() != Some("api-v2.soundcloud.com")
        || next.path() != expected_path
    {
        return Err(ProviderError::new(
            "SoundCloud returned an unexpected playlist tracks continuation",
        ));
    }
    next.query_pairs()
        .find_map(|(key, value)| (key == "offset" && !value.is_empty()).then(|| value.into_owned()))
        .map(Some)
        .ok_or_else(|| {
            ProviderError::new("SoundCloud playlist tracks continuation did not include an offset")
        })
}

fn apply_soundcloud_metadata(route: &mut DetailRoute, playlist: &Value) {
    route.title = nonempty_string(playlist.get("title")).unwrap_or_else(|| route.title.clone());
    route.subtitle = nonempty_string(
        playlist
            .pointer("/user/username")
            .or_else(|| playlist.pointer("/user/full_name")),
    )
    .unwrap_or_else(|| route.subtitle.clone());
    route.release_date = nonempty_string(
        playlist
            .get("release_date")
            .or_else(|| playlist.get("display_date"))
            .or_else(|| playlist.get("created_at")),
    )
    .unwrap_or_else(|| route.release_date.clone());
    let artwork = soundcloud_artwork(playlist);
    if !artwork.is_empty() {
        route.artwork = artwork;
    }
    // The permalink is the canonical link the card menus copy; detail menus
    // read it from the route once the payload has supplied it.
    let service_url = crate::search::soundcloud_service_url(playlist);
    if !service_url.is_empty() {
        route.service_url = service_url;
    }
}

/// Album identity a SoundCloud album detail stamps onto its tracks. Track
/// payloads carry no album id, so the album page is the only source; giving
/// each track the id and title lets menus away from the page (the player
/// bar, the queue) resolve the album, matching Deezer tracks that always
/// carry ALB_ID. Playlist details leave track album data untouched.
fn soundcloud_album_tracks(tracks: Vec<Track>, route: &DetailRoute) -> Vec<Track> {
    if route.provider != Provider::SoundCloud || route.kind != ResultType::Albums {
        return tracks;
    }
    let album_id = route.id.trim();
    if album_id.is_empty() || !album_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return tracks;
    }
    let album_title = route.title.trim();
    tracks
        .into_iter()
        .map(|mut track| {
            if track.album_id.trim().is_empty() {
                track.album_id = album_id.to_owned();
            }
            if track.album.trim().is_empty() && !album_title.is_empty() {
                track.album = album_title.to_owned();
            }
            track
        })
        .collect()
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn merge_deezer_tracks(source: Vec<Value>, hydrated: Vec<Value>) -> Vec<Value> {
    let hydrated = hydrated
        .into_iter()
        .filter_map(|track| {
            track
                .get("SNG_ID")
                .and_then(value_string)
                .map(|id| (id, track))
        })
        .collect::<HashMap<_, _>>();
    source
        .into_iter()
        .map(|mut track| {
            let Some(id) = track.get("SNG_ID").and_then(value_string) else {
                return track;
            };
            let Some(Value::Object(extra)) = hydrated.get(&id) else {
                return track;
            };
            if let Value::Object(base) = &mut track {
                base.extend(extra.clone());
            }
            track
        })
        .collect()
}

fn request_error(error: reqwest::Error, provider: &str) -> ProviderError {
    ProviderError::new(if error.is_timeout() {
        format!("{provider} collection timed out")
    } else if error.is_connect() {
        format!("{provider} could not be reached")
    } else {
        format!("{provider} request failed")
    })
}

/// Converts SoundCloud description HTML to plain text, preserving line
/// structure from block tags and dropping disallowed content entirely.
/// Port of the original rich-text.js allowlist semantics.
pub(crate) fn rich_text_to_plain(html: &str) -> String {
    let trimmed = html.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if !trimmed.contains('<') {
        return trimmed.to_owned();
    }
    // Remove script/style/iframe/object/embed nodes together with their content,
    // exactly like the original's querySelectorAll removal pass.
    let drop_tags = ["script", "style", "iframe", "object", "embed"];
    let mut working = trimmed.to_owned();
    for tag in drop_tags {
        while let Some(open_start) = working.to_ascii_lowercase().find(&format!("<{tag}")) {
            let Some(open_end_rel) = working[open_start..].find('>') else {
                break;
            };
            let open_end = open_start + open_end_rel + 1;
            let close_tag = format!("</{tag}>");
            let end = working[open_end..]
                .to_ascii_lowercase()
                .find(&close_tag)
                .map(|rel| open_end + rel + close_tag.len())
                .unwrap_or(working.len());
            working.replace_range(open_start..end, "");
        }
    }
    let block_tags: [&str; 7] = ["p", "div", "ul", "ol", "li", "pre", "blockquote"];
    let mut output = String::with_capacity(working.len());
    let mut rest = working.as_str();
    while let Some(angle) = rest.find('<') {
        output.push_str(&rest[..angle]);
        let after_angle = &rest[angle + 1..];
        let Some(close) = after_angle.find('>') else {
            output.push('<');
            output.push_str(after_angle);
            rest = "";
            break;
        };
        let tag_inner = &after_angle[..close];
        let tag_name = tag_inner
            .trim()
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if tag_name == "br" || (block_tags.contains(&tag_name.as_str()) && !output.ends_with('\n'))
        {
            output.push('\n');
        }
        rest = &after_angle[close + 1..];
    }
    output.push_str(rest);
    let mut collapsed = String::with_capacity(output.len());
    let mut previous_blank = true;
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            if previous_blank {
                continue;
            }
            previous_blank = true;
            continue;
        }
        previous_blank = false;
        collapsed.push_str(line);
        collapsed.push('\n');
    }
    collapsed.trim().to_owned()
}

#[cfg(test)]
mod rich_text_tests {
    use super::rich_text_to_plain;

    #[test]
    fn plain_text_without_tags_is_preserved_after_trimming() {
        assert_eq!(rich_text_to_plain(""), "");
        assert_eq!(rich_text_to_plain("  just words  "), "just words");
    }

    #[test]
    fn disallowed_elements_lose_content_and_allowed_blocks_become_lines() {
        assert_eq!(rich_text_to_plain("<p>one</p><p>two</p>"), "one\ntwo");
        assert_eq!(
            rich_text_to_plain("<script>alert(1)</script><p>safe</p>"),
            "safe"
        );
        assert_eq!(
            rich_text_to_plain("line one<br>line two<p>para</p>"),
            "line one\nline two\npara"
        );
        assert_eq!(rich_text_to_plain("<ul><li>a</li><li>b</li></ul>"), "a\nb");
        assert_eq!(
            rich_text_to_plain("<a href=\"https://x.test\">link text</a>"),
            "link text"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_tracks_preserve_duplicates_and_skip_missing_hydration() {
        let ids = vec!["3".into(), "1".into(), "3".into(), "9".into()];
        let tracks = vec![json!({"id": 1}), json!({"id": 3})];
        let ordered = ordered_soundcloud_tracks(&ids, tracks);
        assert_eq!(ordered[0]["id"], 3);
        assert_eq!(ordered[1]["id"], 1);
        assert_eq!(ordered[2]["id"], 3);
        assert_eq!(ordered.len(), 3);
    }

    #[test]
    fn deezer_collection_pages_are_bounded_and_offset_based() {
        let (operation, first) = deezer_collection_request(ResultType::Albums, "42", 0).unwrap();
        assert_eq!(operation, "song.getListByAlbum");
        assert_eq!(first["alb_id"], "42");
        assert_eq!(first["start"], 0);
        assert_eq!(first["nb"], DEEZER_COLLECTION_PAGE_SIZE);

        let (operation, next) =
            deezer_collection_request(ResultType::Playlists, "7", DEEZER_COLLECTION_PAGE_SIZE)
                .unwrap();
        assert_eq!(operation, "playlist.getSongs");
        assert_eq!(next["playlist_id"], "7");
        assert_eq!(next["start"], DEEZER_COLLECTION_PAGE_SIZE);
        assert!(deezer_collection_request(ResultType::Tracks, "7", 0).is_err());
    }

    #[test]
    fn playlist_continuations_are_scoped_to_exact_https_endpoint() {
        let path = "/playlists/42/tracks";
        let valid = json!({
            "next_href": "https://api-v2.soundcloud.com/playlists/42/tracks?client_id=hidden&offset=cursor%3Aone"
        });
        assert_eq!(
            soundcloud_next_offset(&valid, path).unwrap(),
            Some("cursor:one".into())
        );
        for next_href in [
            "http://api-v2.soundcloud.com/playlists/42/tracks?offset=x",
            "https://evil.invalid/playlists/42/tracks?offset=x",
            "https://api-v2.soundcloud.com/playlists/43/tracks?offset=x",
            "https://api-v2.soundcloud.com/playlists/42/tracks?offset=",
        ] {
            assert!(soundcloud_next_offset(&json!({"next_href": next_href}), path).is_err());
        }
        assert_eq!(
            soundcloud_next_offset(&json!({"next_href": null}), path).unwrap(),
            None
        );
    }

    #[test]
    fn soundcloud_metadata_is_authoritative_and_artwork_stays_https() {
        let mut route = DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Search title".into(),
            subtitle: "Search owner".into(),
            artwork: "https://example.com/search.jpg".into(),
            release_date: "2020".into(),
            service_url: String::new(),
        };
        apply_soundcloud_metadata(
            &mut route,
            &json!({
                "title": "Detail title",
                "artwork_url": "http://127.0.0.1/private.jpg",
                "release_date": "2024-02-03",
                "permalink_url": "https://soundcloud.com/owner/sets/detail-title",
                "user": {"username": "Detail owner"}
            }),
        );
        assert_eq!(route.title, "Detail title");
        assert_eq!(route.subtitle, "Detail owner");
        assert_eq!(route.release_date, "2024-02-03");
        assert_eq!(route.artwork, "https://example.com/search.jpg");
        assert_eq!(
            route.service_url,
            "https://soundcloud.com/owner/sets/detail-title"
        );
    }

    #[test]
    fn blank_deezer_album_route_uses_loaded_metadata() {
        let mut route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        apply_deezer_metadata(
            &mut route,
            &super::super::album_info::AlbumInfo {
                title: "Discovery".into(),
                album_artist: "Daft Punk".into(),
                release_date: "2001-03-07".into(),
                ..Default::default()
            },
        );
        assert_eq!(route.title, "Discovery");
        assert_eq!(route.subtitle, "Daft Punk");
        assert_eq!(route.release_date, "2001-03-07");
    }

    #[test]
    fn empty_deezer_metadata_does_not_erase_route_values() {
        let mut route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Existing title".into(),
            subtitle: "Existing artist".into(),
            artwork: "https://example.com/cover.jpg".into(),
            release_date: "2001".into(),
            service_url: String::new(),
        };
        apply_deezer_metadata(&mut route, &Default::default());
        assert_eq!(route.title, "Existing title");
        assert_eq!(route.subtitle, "Existing artist");
        assert_eq!(route.artwork, "https://example.com/cover.jpg");
        assert_eq!(route.release_date, "2001");
    }

    #[test]
    fn blank_album_route_uses_loaded_track_metadata_as_a_fallback() {
        let mut route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Albums,
            id: "42".into(),
            title: "Album".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        let tracks = vec![
            super::super::models::Track::default(),
            super::super::models::Track {
                album: "Discovery".into(),
                artist: "Daft Punk".into(),
                release_date: "2001-03-07".into(),
                artwork: "https://example.com/track.jpg".into(),
                ..Default::default()
            },
        ];
        route.title.clear();
        apply_track_metadata_fallback(&mut route, &tracks);
        assert_eq!(route.title, "Discovery");
        assert_eq!(route.subtitle, "Daft Punk");
        assert_eq!(route.release_date, "2001-03-07");
        assert_eq!(route.artwork, "https://example.com/track.jpg");
    }
    #[test]
    fn soundcloud_album_tracks_inherit_the_album_identity() {
        let album_route = DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Albums,
            id: "2189715572".into(),
            title: "Album".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        let named = super::super::models::Track {
            id: "7".into(),
            album: "Own album name".into(),
            ..Default::default()
        };
        let stamped = soundcloud_album_tracks(
            vec![super::super::models::Track::default(), named],
            &album_route,
        );
        assert_eq!(stamped[0].album_id, "2189715572");
        assert_eq!(stamped[0].album, "Album");
        // The track's own album name wins over the page title, matching the
        // album tracklist menu which also prefers the track album data.
        assert_eq!(stamped[1].album_id, "2189715572");
        assert_eq!(stamped[1].album, "Own album name");

        // Playlists and Deezer albums keep their track album data as loaded.
        let playlist_route = DetailRoute {
            kind: ResultType::Playlists,
            ..album_route.clone()
        };
        assert_eq!(
            soundcloud_album_tracks(
                vec![super::super::models::Track::default()],
                &playlist_route
            )[0]
            .album_id,
            ""
        );
        let deezer_route = DetailRoute {
            provider: Provider::Deezer,
            ..album_route
        };
        assert_eq!(
            soundcloud_album_tracks(vec![super::super::models::Track::default()], &deezer_route)[0]
                .album_id,
            ""
        );
    }

    #[test]
    fn provider_errors_redact_secret_shaped_messages() {
        assert_eq!(
            ProviderError::new("Authorization OAuth sentinel").message,
            "Provider request failed"
        );
    }
}
