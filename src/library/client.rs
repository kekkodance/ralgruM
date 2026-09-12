use super::{
    favorite_state::FavoriteKind,
    model::{
        Card, Category, FLOW_TRACK_DESCRIPTION, Page, Route, Section, SectionLayout, Service,
        Track, root_copy, value_string,
    },
    normalize,
};
use crate::search::DeezerArl;
use crate::search::{
    Card as SearchCard, DetailPage, DetailRoute, ResultType, SearchClient, Track as SearchTrack,
};
use crate::smart_mix_title::{CANONICAL_SMART_MIX_TITLE, specific_smart_mix_title};
use reqwest::{Client, Response, Url, header};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};

const GATEWAY: &str = "https://www.deezer.com/ajax/gw-light.php";
const DEEZER_USER_DATA_URL: &str = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const FLOW_CARD_SUBTITLE: &str = "Personalized mix";
const FLOW_EMPTY_DESCRIPTION: &str = "No Flow mixes were returned for this account.";
const TRACKS_PREFIX_SIZE: usize = 256;
const TRACKS_PAGE_SIZE: usize = 10_000;
const TRACKS_OVERLAP: usize = 8;

#[derive(Clone)]
pub(crate) struct LibraryClient {
    pub(super) client: Client,
}

#[derive(Clone)]
pub(super) struct DeezerSession {
    pub(super) token: String,
    pub(super) cookie: header::HeaderValue,
}

pub(super) struct DeezerTracksLoad {
    pub(super) page: Page,
    pub(super) continuation: Option<DeezerTracksContinuation>,
    pub(super) user_id: String,
}

pub(super) struct DeezerTracksCompletion {
    pub(super) page: Page,
    pub(super) hydration: Option<DeezerTracksHydration>,
}

#[derive(Clone)]
pub(super) struct DeezerTracksContinuation {
    client: LibraryClient,
    session: DeezerSession,
    user_id: String,
    suffix: Vec<Value>,
    total: usize,
}

#[derive(Clone)]
pub(super) struct DeezerTracksHydration {
    client: LibraryClient,
    session: DeezerSession,
    source: Vec<Value>,
    total: usize,
}

impl DeezerTracksHydration {
    pub(super) async fn hydrate(self) -> Result<Page, String> {
        let ids = track_ids(&self.source);
        let hydrated = self.client.hydrate_track_values(&self.session, ids).await?;
        let items = merge_hydrated_tracks(self.source, hydrated);
        Ok(normalized_root_page(Category::Tracks, self.total, &items))
    }
}

impl DeezerTracksContinuation {
    pub(super) async fn complete(self) -> Result<DeezerTracksCompletion, String> {
        let (items, total) = if self.total == self.suffix.len() {
            (self.suffix.clone(), self.total)
        } else {
            match self.fetch_all(&self.suffix, self.total).await {
                Ok(items) => (items, self.total),
                Err(TracksAssemblyError::Inconsistent(_)) => {
                    let (restarted_suffix, restarted_total) = self
                        .client
                        .fetch_tracks_preview(&self.session, &self.user_id)
                        .await?;
                    if restarted_total == restarted_suffix.len() {
                        (restarted_suffix, restarted_total)
                    } else {
                        let items = self
                            .fetch_all(&restarted_suffix, restarted_total)
                            .await
                            .map_err(TracksAssemblyError::into_message)?;
                        (items, restarted_total)
                    }
                }
                Err(error) => return Err(error.into_message()),
            }
        };
        let page = normalized_root_page(Category::Tracks, total, &items);
        let ids = track_ids(&items);
        let hydration = (!ids.is_empty()).then(|| DeezerTracksHydration {
            client: self.client,
            session: self.session,
            source: items,
            total,
        });
        Ok(DeezerTracksCompletion { page, hydration })
    }

    async fn fetch_all(
        &self,
        suffix: &[Value],
        total: usize,
    ) -> Result<Vec<Value>, TracksAssemblyError> {
        if suffix.is_empty() || suffix.len() > total {
            return Err(TracksAssemblyError::Inconsistent(
                "Deezer Tracks suffix was incomplete".into(),
            ));
        }
        let first = self
            .client
            .fetch_tracks_chunk(&self.session, &self.user_id, 0, total.min(TRACKS_PAGE_SIZE))
            .await
            .map_err(TracksAssemblyError::Request)?;
        let expected_first_len = total.min(TRACKS_PAGE_SIZE);
        let mut items = assemble_first_tracks_page(total, expected_first_len, first)?;
        while items.len() < total {
            let old_len = items.len();
            let next_start = old_len.saturating_sub(TRACKS_OVERLAP);
            let request_size = total.saturating_sub(next_start).min(TRACKS_PAGE_SIZE);
            if request_size <= TRACKS_OVERLAP {
                return Err(TracksAssemblyError::Inconsistent(
                    "Deezer Tracks pagination was incomplete".into(),
                ));
            }
            let chunk = self
                .client
                .fetch_tracks_chunk(&self.session, &self.user_id, next_start, request_size)
                .await
                .map_err(TracksAssemblyError::Request)?;
            append_overlapping_tracks(&mut items, total, request_size, chunk)?;
            if items.len() <= old_len {
                return Err(TracksAssemblyError::Inconsistent(
                    "Deezer Tracks pagination did not advance".into(),
                ));
            }
        }
        validate_tracks_suffix(&items, suffix)?;
        Ok(items)
    }
}

struct TracksChunk {
    items: Vec<Value>,
    total: Option<usize>,
}

#[derive(Debug)]
enum TracksAssemblyError {
    Request(String),
    Inconsistent(String),
}

impl TracksAssemblyError {
    fn into_message(self) -> String {
        match self {
            Self::Request(message) | Self::Inconsistent(message) => message,
        }
    }
}

impl LibraryClient {
    pub(crate) async fn load_favorite_catalogs(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<Vec<(FavoriteKind, Result<Vec<String>, String>)>, String> {
        let (session, user) = self.bootstrap(arl, saved_user_id).await?;
        let tracks = self.load_favorite_track_ids(&session, &user);
        let albums = self.load_root_favorite_ids(Category::Albums, &session, &user);
        let artists = self.load_root_favorite_ids(Category::Artists, &session, &user);
        let playlists = self.load_root_favorite_ids(Category::Playlists, &session, &user);
        let (tracks, albums, artists, playlists) = tokio::join!(tracks, albums, artists, playlists);
        Ok(vec![
            (FavoriteKind::Track, tracks),
            (FavoriteKind::Album, albums),
            (FavoriteKind::Artist, artists),
            (FavoriteKind::Playlist, playlists),
        ])
    }

    async fn load_favorite_track_ids(
        &self,
        session: &DeezerSession,
        user: &str,
    ) -> Result<Vec<String>, String> {
        let first = self
            .fetch_tracks_chunk(session, user, 0, TRACKS_PAGE_SIZE)
            .await?;
        let total = required_tracks_total(&first)?;
        let mut ids = track_ids(&first.items);
        let mut start = first.items.len();
        while start < total {
            let page_size = (total - start).min(TRACKS_PAGE_SIZE);
            let chunk = self
                .fetch_tracks_chunk(session, user, start, page_size)
                .await?;
            if chunk.items.is_empty() {
                return Err("Deezer favorite track preload returned an incomplete page".into());
            }
            ids.extend(track_ids(&chunk.items));
            start += chunk.items.len();
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    async fn load_root_favorite_ids(
        &self,
        category: Category,
        session: &DeezerSession,
        user: &str,
    ) -> Result<Vec<String>, String> {
        let (items, total) = self.fetch_root_collection(category, session, user).await?;
        if items.len() < total {
            return Err(format!(
                "Deezer {} favorite preload returned an incomplete list",
                category.action()
            ));
        }
        let id_field = favorite_catalog_id_field(category)
            .ok_or_else(|| "Unsupported Deezer favorite catalog".to_string())?;
        let mut ids = items
            .iter()
            .map(|item| value_string(item.get(id_field)))
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    pub(crate) fn new() -> Result<Self, String> {
        Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .build()
            .map(|client| Self { client })
            .map_err(|_| "Library client could not be created".into())
    }
    pub(crate) async fn load(
        &self,
        category: Category,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<Page, String> {
        let (session, user) = self.bootstrap(arl, saved_user_id).await?;
        if category == Category::Flow {
            return self.load_flow(session).await;
        }
        if category == Category::History {
            return self.load_history(session, &user).await;
        }
        let (mut items, total) = self
            .fetch_root_collection(category, &session, &user)
            .await?;
        if category == Category::Tracks {
            let ids = track_ids(&items);
            if !ids.is_empty() {
                let hydrated = self.hydrate_track_values(&session, ids).await?;
                items = merge_hydrated_tracks(items, hydrated);
            }
        }
        Ok(normalized_root_page(category, total, &items))
    }

    pub(super) async fn load_tracks_progressive(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<DeezerTracksLoad, String> {
        let (session, user) = self.bootstrap(arl, saved_user_id).await?;
        let (suffix, total) = self.fetch_tracks_preview(&session, &user).await?;
        let page = normalized_root_page(Category::Tracks, total, &suffix);
        let continuation = (!suffix.is_empty()).then(|| DeezerTracksContinuation {
            client: self.clone(),
            session,
            user_id: user.clone(),
            suffix,
            total,
        });
        Ok(DeezerTracksLoad {
            page,
            continuation,
            user_id: user,
        })
    }

    async fn fetch_tracks_preview(
        &self,
        session: &DeezerSession,
        user: &str,
    ) -> Result<(Vec<Value>, usize), String> {
        let mut probe = self
            .fetch_tracks_chunk(session, user, 0, TRACKS_PREFIX_SIZE)
            .await?;
        let mut total = required_tracks_total(&probe)?;
        if total > 0 && probe.items.is_empty() {
            probe = self
                .fetch_tracks_chunk(session, user, 0, TRACKS_PREFIX_SIZE)
                .await?;
            total = required_tracks_total(&probe)?;
            if total > 0 && probe.items.is_empty() {
                return Err(
                    "Deezer favorite_song.getList returned an empty page for a nonempty library"
                        .into(),
                );
            }
        }

        let preview_len = total.min(TRACKS_PREFIX_SIZE);
        if total > probe.items.len() {
            let suffix_start = total - preview_len;
            let suffix = self
                .fetch_tracks_chunk(session, user, suffix_start, preview_len)
                .await?;
            return validate_tracks_preview(suffix, total, preview_len);
        }
        validate_tracks_preview(probe, total, total)
    }

    async fn fetch_tracks_chunk(
        &self,
        session: &DeezerSession,
        user: &str,
        start: usize,
        nb: usize,
    ) -> Result<TracksChunk, String> {
        let mut last_error = None;
        for attempt in 0..=1 {
            match self.fetch_tracks_chunk_once(session, user, start, nb).await {
                Ok(chunk) => return Ok(chunk),
                Err((error, retryable)) if retryable && attempt == 0 => last_error = Some(error),
                Err((error, _)) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| "Deezer favorite_song.getList request failed".into()))
    }

    async fn fetch_tracks_chunk_once(
        &self,
        session: &DeezerSession,
        user: &str,
        start: usize,
        nb: usize,
    ) -> Result<TracksChunk, (String, bool)> {
        let mut url = Url::parse(GATEWAY)
            .map_err(|_| ("Invalid Deezer library endpoint".to_string(), false))?;
        url.query_pairs_mut()
            .append_pair("method", "favorite_song.getList")
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", &session.token);
        let response = self
            .client
            .post(url)
            .header(header::COOKIE, session.cookie.clone())
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&tracks_request(user, start, nb))
            .send()
            .await
            .map_err(|_| ("Deezer favorite_song.getList request failed".into(), true))?;
        let status = response.status();
        if !status.is_success() {
            return Err((
                format!("Deezer returned {status}"),
                status.is_server_error(),
            ));
        }
        let envelope = response
            .json()
            .await
            .map_err(|_| ("Deezer returned an invalid response".into(), false))?;
        let results =
            envelope_results(envelope, "favorite_song.getList").map_err(|error| (error, false))?;
        Ok(parse_tracks_chunk(results))
    }

    async fn fetch_root_collection(
        &self,
        category: Category,
        session: &DeezerSession,
        user: &str,
    ) -> Result<(Vec<Value>, usize), String> {
        let (operation, body) = root_request(category, user);
        let mut url =
            Url::parse(GATEWAY).map_err(|_| "Invalid Deezer library endpoint".to_string())?;
        url.query_pairs_mut()
            .append_pair("method", operation)
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", &session.token);
        let response = self
            .client
            .post(url)
            .header(header::COOKIE, session.cookie.clone())
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&body)
            .send()
            .await
            .map_err(|_| format!("Deezer {operation} request failed"))?;
        let results = envelope_results(decode(response).await?, operation)?;
        if matches!(
            category,
            Category::Albums | Category::Artists | Category::Playlists
        ) {
            let key = category.action();
            let mut items = results
                .pointer(&format!("/TAB/{key}/data"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if category == Category::Playlists {
                let owner = results
                    .pointer("/DATA/USER/BLOG_NAME")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        results
                            .pointer("/DATA/USER/DISPLAY_NAME")
                            .and_then(Value::as_str)
                    })
                    .or_else(|| results.pointer("/DATA/USER/NAME").and_then(Value::as_str))
                    .filter(|name| !name.trim().is_empty());
                if let Some(owner) = owner {
                    for item in &mut items {
                        if item
                            .get("PARENT_USERNAME")
                            .and_then(Value::as_str)
                            .map_or(true, |s| s.trim().is_empty())
                        {
                            item["PARENT_USERNAME"] = Value::String(owner.to_owned());
                        }
                    }
                }
            }
            Ok((
                items,
                results
                    .pointer(&format!("/TAB/{key}/total"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize,
            ))
        } else {
            Ok((
                results
                    .get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                results.get("total").and_then(Value::as_u64).unwrap_or(0) as usize,
            ))
        }
    }

    async fn hydrate_track_values(
        &self,
        session: &DeezerSession,
        ids: Vec<String>,
    ) -> Result<Vec<Value>, String> {
        let hydrated = self
            .gateway_call(
                "song.getListData",
                json!({ "sng_ids": ids }),
                &session.token,
                session.cookie.clone(),
            )
            .await?;
        Ok(hydrated
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    async fn load_flow(&self, session: DeezerSession) -> Result<Page, String> {
        let results = self
            .gateway_call(
                "page.get",
                json!({
                    "PAGE": "home",
                    "VERSION": "2.5",
                    "SUPPORT": { "filterable-grid": ["flow"] },
                    "LANG": "en",
                    "OPTIONS": []
                }),
                &session.token,
                session.cookie,
            )
            .await?;
        let section = results
            .get("sections")
            .and_then(Value::as_array)
            .and_then(|sections| {
                sections.iter().find(|section| {
                    section.get("layout").and_then(Value::as_str) == Some("filterable-grid")
                        && section
                            .get("items")
                            .and_then(Value::as_array)
                            .is_some_and(|items| {
                                items.iter().any(|item| {
                                    item.get("type").and_then(Value::as_str) == Some("flow")
                                })
                            })
                })
            })
            .ok_or_else(|| "Flow response has no Flow section".to_string())?;
        let cards = section
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("flow"))
            .filter_map(|item| {
                let id = item.pointer("/data/id").and_then(Value::as_str)?.trim();
                (!id.is_empty()).then(|| Card {
                    kind: Category::Flow,
                    id: id.into(),
                    title: first_string(
                        &[
                            value_string(item.get("title")),
                            value_string(item.pointer("/data/title")),
                        ],
                        "Flow",
                    ),
                    subtitle: first_string(
                        &[value_string(item.get("subtitle"))],
                        FLOW_CARD_SUBTITLE,
                    ),
                    artwork: flow_artwork(item),
                    source: crate::search::Provider::Deezer,
                    ..Card::default()
                })
            })
            .collect::<Vec<_>>();
        let flow_catalog = super::deezer_radio::parse_flow_catalog(section);
        Ok(Page {
            title: value_string(section.get("title")),
            description: root_copy(Service::Deezer, Category::Flow).1.into(),
            total: cards.len(),
            cards,
            flow_catalog,
            count_noun: "mix".into(),
            show_count: false,
            empty_title: "Flow is unavailable".into(),
            empty_description: FLOW_EMPTY_DESCRIPTION.into(),
            ..Page::default()
        })
    }

    async fn load_flow_tracks(&self, route: Route, arl: DeezerArl) -> Result<Page, String> {
        let (session, user) = self.bootstrap(arl, None).await?;
        let page = self.load_flow(session.clone()).await?;
        let flow_title = page
            .cards
            .iter()
            .find(|card| card.id == route.id)
            .map(|card| card.title.clone())
            .ok_or_else(|| "Flow mix is no longer available".to_string())?;
        let batch = self
            .load_flow_radio_with_session(
                &route.id,
                super::deezer_radio::FlowTuner::initial(super::deezer_radio::FlowMode::Default),
                &user,
                session,
            )
            .await?;
        Ok(Page {
            title: if route.title.is_empty() {
                flow_title
            } else {
                route.title
            },
            description: FLOW_TRACK_DESCRIPTION.into(),
            count_noun: "track".into(),
            total: batch.total,
            tracks: batch.tracks,
            next_flow_tuner: batch.next_flow_tuner,
            clear_remaining_tracks: batch.clear_remaining_tracks,
            platform: Some(crate::library::model::Service::Deezer),
            ..Page::default()
        })
    }

    /// Load a Flow route using an explicitly selected mode.  The initial
    /// tuner is deliberately created here from the mode and the request is
    /// sent through `load_flow_radio`, so the opaque continuation returned by
    /// Deezer stays attached to the resulting page.
    pub(crate) async fn load_flow_radio_page(
        &self,
        route: Route,
        mode: super::deezer_radio::FlowMode,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        smart_mix: bool,
    ) -> Result<Page, String> {
        if smart_mix {
            let smart = self
                .load_smart_tracklist(&route.id, arl, saved_user_id)
                .await?;
            return Ok(smart_tracklist_page(route, smart));
        }
        let batch = self
            .load_flow_radio(
                &route.id,
                super::deezer_radio::FlowTuner::initial(mode),
                arl,
                saved_user_id,
            )
            .await?;
        Ok(Page {
            title: route.title,
            description: FLOW_TRACK_DESCRIPTION.into(),
            count_noun: "track".into(),
            total: batch.total,
            tracks: batch.tracks,
            next_flow_tuner: batch.next_flow_tuner,
            clear_remaining_tracks: batch.clear_remaining_tracks,
            platform: Some(crate::library::model::Service::Deezer),
            ..Page::default()
        })
    }

    pub(crate) async fn set_favorite(
        &self,
        key: super::favorite_state::FavoriteKey,
        favorite: bool,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<(), String> {
        if key.provider != crate::search::Provider::Deezer {
            return Err("Unsupported favorite provider".into());
        }
        let id = valid_deezer_id(&key.id)?;
        let session = self.bootstrap(arl, saved_user_id).await?.0;
        let (operation, body) = favorite_request(key.kind, &id, favorite);
        let result = self
            .gateway_call(operation, body, &session.token, session.cookie)
            .await?;
        confirm_true(result, operation)
    }

    pub(crate) async fn load_route(
        &self,
        route: Route,
        arl: Option<DeezerArl>,
    ) -> Result<Page, String> {
        if route.source != crate::search::Provider::Deezer {
            return Err("Unsupported Deezer library route".into());
        }
        if route.action == "flowTracks" {
            let arl = arl.ok_or_else(|| "Deezer account required".to_string())?;
            return self.load_flow_tracks(route, arl).await;
        }
        let kind = match route.action.as_str() {
            "albumTracks" => ResultType::Albums,
            "playlistTracks" => ResultType::Playlists,
            "artist" => ResultType::Artists,
            _ => return Err("Unsupported Deezer library route".into()),
        };
        let client = SearchClient::new().map_err(|error| error.message)?;
        let detail = client
            .detail(detail_route(route, kind), arl, None)
            .await
            .map_err(|error| error.message)?;
        Ok(detail_page(detail))
    }

    pub(super) async fn gateway_call(
        &self,
        operation: &str,
        body: Value,
        token: &str,
        cookie: header::HeaderValue,
    ) -> Result<Value, String> {
        let mut url =
            Url::parse(GATEWAY).map_err(|_| "Invalid Deezer library endpoint".to_string())?;
        url.query_pairs_mut()
            .append_pair("method", operation)
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", token);
        let response = self
            .client
            .post(url)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&body)
            .send()
            .await
            .map_err(|_| format!("Deezer {operation} request failed"))?;
        envelope_results(decode(response).await?, operation)
    }

    pub(super) async fn bootstrap(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<(DeezerSession, String), String> {
        let cookie = arl.cookie_header().map_err(|error| error.message)?;
        let bootstrap = library_session_request(&self.client, cookie)
            .send()
            .await
            .map_err(|_| "Deezer login session could not be verified".to_string())?;
        let cookies = response_cookies(&bootstrap);
        let value = decode(bootstrap).await?;
        let results = envelope_results(value, "session bootstrap")?;
        let token = results
            .pointer("/checkForm")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "Deezer login required".to_string())?
            .to_owned();
        let user = value_string(results.pointer("/USER/USER_ID"));
        let user = valid_user_id(&user)
            .or_else(|| saved_user_id.as_deref().and_then(valid_user_id))
            .ok_or_else(|| "Deezer login required".to_string())?;
        let cookie = session_cookie(&arl, &cookies)?;
        Ok((DeezerSession { token, cookie }, user))
    }
}

fn smart_tracklist_page(route: Route, smart: super::deezer_radio::DeezerSmartTracklist) -> Page {
    Page {
        title: smart_tracklist_heading_title(&route.title, &smart.title),
        subtitle: if smart.subtitle.trim().is_empty() {
            route.subtitle
        } else {
            smart.subtitle
        },
        description: if smart.description.trim().is_empty() {
            FLOW_TRACK_DESCRIPTION.into()
        } else {
            smart.description
        },
        artwork: if smart.artwork.trim().is_empty() {
            route.artwork
        } else {
            smart.artwork
        },
        count_noun: "track".into(),
        total: smart.total,
        tracks: smart.tracks,
        next_flow_tuner: None,
        resolved_smart_mix_title: smart.resolved_smart_mix_title,
        clear_remaining_tracks: true,
        platform: Some(crate::library::model::Service::Deezer),
        ..Page::default()
    }
}

fn smart_tracklist_heading_title(route_title: &str, server_title: &str) -> String {
    specific_smart_mix_title(server_title)
        .or_else(|| specific_smart_mix_title(route_title))
        .unwrap_or(CANONICAL_SMART_MIX_TITLE)
        .to_owned()
}

pub(super) fn root_page(category: Category, total: usize) -> Page {
    let (title, description) = root_copy(Service::Deezer, category);
    Page {
        title: title.into(),
        description: description.into(),
        count_noun: if matches!(category, Category::Tracks | Category::History) {
            "track"
        } else {
            "item"
        }
        .into(),
        show_count: !matches!(category, Category::Flow),
        total,
        ..Page::default()
    }
}

fn first_string(values: &[String], fallback: &str) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.into())
}

fn track_id(track: &Value) -> Option<String> {
    let id = value_string(track.get("SNG_ID"));
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

fn track_ids(tracks: &[Value]) -> Vec<String> {
    tracks.iter().filter_map(track_id).collect()
}

fn tracks_request(user: &str, start: usize, nb: usize) -> Value {
    json!({"user_id":user,"tab":"loved","nb":nb,"start":start})
}

fn parse_tracks_chunk(results: Value) -> TracksChunk {
    TracksChunk {
        items: results
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        total: results
            .get("total")
            .and_then(Value::as_u64)
            .and_then(|total| usize::try_from(total).ok()),
    }
}

fn required_tracks_total(chunk: &TracksChunk) -> Result<usize, String> {
    let total = chunk
        .total
        .ok_or_else(|| "Deezer favorite_song.getList response is missing total".to_string())?;
    if total < chunk.items.len() {
        return Err("Deezer favorite_song.getList returned an inconsistent total".into());
    }
    Ok(total)
}

fn assemble_first_tracks_page(
    total: usize,
    expected_len: usize,
    first: TracksChunk,
) -> Result<Vec<Value>, TracksAssemblyError> {
    if first.total != Some(total) {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks changed while it was loading".into(),
        ));
    }
    if first.items.len() != expected_len {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks response was incomplete".into(),
        ));
    }
    Ok(first.items)
}

fn append_overlapping_tracks(
    items: &mut Vec<Value>,
    total: usize,
    expected_len: usize,
    chunk: TracksChunk,
) -> Result<(), TracksAssemblyError> {
    if chunk.total != Some(total) {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks changed while it was loading".into(),
        ));
    }
    if chunk.items.len() != expected_len || chunk.items.len() <= TRACKS_OVERLAP {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks pagination was incomplete".into(),
        ));
    }
    let overlap_start = items.len().checked_sub(TRACKS_OVERLAP).ok_or_else(|| {
        TracksAssemblyError::Inconsistent("Deezer Tracks pagination was incomplete".into())
    })?;
    if !same_track_id_sequence(&items[overlap_start..], &chunk.items[..TRACKS_OVERLAP]) {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks changed while it was loading".into(),
        ));
    }
    let next_len = items.len() + chunk.items.len() - TRACKS_OVERLAP;
    if next_len > total {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks response exceeded its reported total".into(),
        ));
    }
    items.extend(chunk.items.into_iter().skip(TRACKS_OVERLAP));
    Ok(())
}

fn validate_tracks_preview(
    chunk: TracksChunk,
    total: usize,
    expected_len: usize,
) -> Result<(Vec<Value>, usize), String> {
    if chunk.total != Some(total) {
        return Err("Deezer Tracks changed while it was loading".into());
    }
    if chunk.items.len() != expected_len {
        return Err("Deezer Tracks preview was incomplete".into());
    }
    Ok((chunk.items, total))
}

fn validate_tracks_suffix(items: &[Value], suffix: &[Value]) -> Result<(), TracksAssemblyError> {
    if suffix.len() > items.len()
        || !same_track_id_sequence(&items[items.len() - suffix.len()..], suffix)
    {
        return Err(TracksAssemblyError::Inconsistent(
            "Deezer Tracks latest suffix changed while it was loading".into(),
        ));
    }
    Ok(())
}

fn same_track_id_sequence(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            track_id(left)
                .zip(track_id(right))
                .is_some_and(|(left, right)| left == right)
        })
}

fn normalized_root_page(category: Category, total: usize, items: &[Value]) -> Page {
    let mut page = root_page(category, if total == 0 { items.len() } else { total });
    if category == Category::Tracks {
        page.tracks = items.iter().rev().map(normalize::track).collect();
    } else if category == Category::History {
        page.tracks = items.iter().map(normalize::track).collect();
    } else {
        page.cards = items
            .iter()
            .map(|item| normalize::card(category, item))
            .collect();
    }
    page.raw_loaded_count = items.len();
    page.normalized_count = if matches!(category, Category::Tracks | Category::History) {
        page.tracks.len()
    } else {
        page.cards.len()
    };
    page.authoritative_total = Some(page.total);
    page
}

/// Merge Deezer's detailed song response into the original collection while
/// retaining the collection's order and any collection-only fields.
///
/// The source list can contain thousands of tracks. Indexing hydrated rows by
/// ID keeps this merge linear instead of scanning the complete hydrated list
/// for every source row.
fn merge_hydrated_tracks(source: Vec<Value>, hydrated: Vec<Value>) -> Vec<Value> {
    let hydrated_by_id: HashMap<String, Value> = hydrated
        .into_iter()
        .filter_map(|track| track_id(&track).map(|id| (id, track)))
        .collect();

    source
        .into_iter()
        .map(|track| {
            let Some(id) = track_id(&track) else {
                return track;
            };
            let Some(Value::Object(hydrated)) = hydrated_by_id.get(&id) else {
                return track;
            };
            let mut merged = match track {
                Value::Object(object) => object,
                other => return other,
            };
            for (key, value) in hydrated {
                merged.insert(key.clone(), value.clone());
            }
            Value::Object(merged)
        })
        .collect()
}

fn flow_artwork(item: &Value) -> String {
    let picture = item
        .get("pictures")
        .and_then(Value::as_array)
        .and_then(|v| v.first());
    let hash = value_string(picture.and_then(|v| v.get("md5")));
    let kind = first_string(
        &[value_string(picture.and_then(|v| v.get("type")))],
        "cover",
    );
    if !hash.is_empty() {
        format!("https://e-cdns-images.dzcdn.net/images/{kind}/{hash}/500x500.jpg")
    } else {
        String::new()
    }
}

fn detail_route(route: Route, kind: ResultType) -> DetailRoute {
    DetailRoute {
        provider: route.source,
        kind,
        id: route.id,
        title: route.title,
        subtitle: route.subtitle,
        artwork: route.artwork,
        release_date: route.release_date,
    }
}

fn library_session_request(client: &Client, arl: header::HeaderValue) -> reqwest::RequestBuilder {
    client.get(DEEZER_USER_DATA_URL).header(header::COOKIE, arl)
}

pub(super) fn detail_page(detail: DetailPage) -> Page {
    let route = detail.route;
    let album_info = detail.album_info.clone();
    let description = crate::search::detail_metadata(&route);
    let is_playlist = route.kind == ResultType::Playlists;
    let mut page = Page {
        title: route.title.clone(),
        subtitle: route.subtitle.clone(),
        description,
        album_info,
        artwork: route.artwork.clone(),
        platform: Some(match route.provider {
            crate::search::Provider::Deezer => crate::library::model::Service::Deezer,
            crate::search::Provider::SoundCloud => crate::library::model::Service::SoundCloud,
        }),
        count_noun: "track".into(),
        meta_text: String::new(),
        show_count: true,
        total: detail.total.unwrap_or(detail.tracks.len()),
        raw_loaded_count: detail.raw_loaded_count,
        normalized_count: detail.normalized_count,
        authoritative_total: detail.authoritative_total,
        tracks: detail.tracks.into_iter().map(search_track).collect(),
        empty_title: if is_playlist {
            "This playlist is empty".into()
        } else {
            String::new()
        },
        empty_description: if is_playlist {
            "Add tracks to this playlist to get started.".into()
        } else {
            String::new()
        },
        ..Page::default()
    };
    let Some(artist) = detail.artist else {
        return page;
    };
    if !artist.profile.title.is_empty() {
        page.title = artist.profile.title;
    }
    if !artist.profile.subtitle.is_empty() {
        page.subtitle = artist.profile.subtitle;
    }
    page.artwork = if artist.profile.artwork.is_empty() {
        page.artwork
    } else {
        artist.profile.artwork
    };
    page.meta_text = artist
        .fans
        .filter(|fans| *fans > 0)
        .map(|fans| format!("{} fans", crate::search::format_number(fans)))
        .unwrap_or_default();
    page.description.clear();
    if artist.fans.is_some() {
        page.subtitle.clear();
    }
    page.show_count = false;
    page.total = 0;
    page.sections = vec![
        Section {
            title: "Tracks".into(),
            description: String::new(),
            total: artist.popular_total,
            show_count: true,
            preview_limit: Some(5),
            layout: SectionLayout::Tracks,
            tracks: artist
                .popular_tracks
                .into_iter()
                .map(search_track)
                .collect(),
            ..Section::default()
        },
        card_section(
            "Similar Artists",
            artist.similar_total,
            artist.similar_artists,
        ),
        card_section("Albums", artist.albums_total, artist.albums),
        card_section("Featured in", artist.featured_total, artist.featured),
        card_section("Playlists", artist.playlists_total, artist.playlists),
    ];
    page
}

fn card_section(title: &str, total: usize, cards: Vec<SearchCard>) -> Section {
    Section {
        title: title.into(),
        description: String::new(),
        total,
        show_count: true,
        layout: SectionLayout::Cards,
        preview_limit: Some(12),
        card_row: true,
        cards: cards.into_iter().map(search_card).collect(),
        ..Section::default()
    }
}

fn search_track(track: SearchTrack) -> Track {
    Track {
        id: track.id,
        title: track.title,
        artist: track.artist,
        artists: track.artists,
        album: track.album,
        album_id: track.album_id,
        release_date: track.release_date,
        duration: track.duration,
        artwork: track.artwork,
        explicit: track.explicit,
        service_url: track.service_url,
        ..Track::default()
    }
}

fn search_card(card: SearchCard) -> Card {
    Card {
        kind: match card.kind {
            ResultType::Albums => Category::Albums,
            ResultType::Artists => Category::Artists,
            ResultType::Playlists => Category::Playlists,
            _ => Category::Tracks,
        },
        id: card.id,
        title: card.title,
        subtitle: card.subtitle,
        artwork: card.artwork,
        release_date: card.release_date,
        badge: card.badge,
        source: card.source,
        service_url: card.service_url,
        is_private: None,
        library_service: None,
    }
}

fn session_cookie(arl: &DeezerArl, cookies: &str) -> Result<header::HeaderValue, String> {
    let arl = arl.cookie_header().map_err(|error| error.message)?;
    let mut cookie = header::HeaderValue::from_str(&format!(
        "{}; {cookies}",
        arl.to_str()
            .map_err(|_| "The saved Deezer session is invalid")?
    ))
    .map_err(|_| "Deezer returned an invalid session".to_string())?;
    cookie.set_sensitive(true);
    Ok(cookie)
}

fn root_request(category: Category, user: &str) -> (&'static str, Value) {
    match category {
        Category::Tracks => ("favorite_song.getList", tracks_request(user, 0, 10_000)),
        Category::History => unreachable!("History uses the paged history endpoint"),
        Category::Albums => (
            "deezer.pageProfile",
            json!({"user_id":user,"tab":"albums","nb":2000}),
        ),
        Category::Artists => (
            "deezer.pageProfile",
            json!({"user_id":user,"tab":"artists","nb":2000}),
        ),
        Category::Playlists => (
            "deezer.pageProfile",
            json!({"user_id":user,"tab":"playlists","nb":2000}),
        ),
        Category::Flow => unreachable!("Flow uses the authenticated Flow endpoint"),
        Category::MyTracks | Category::Station => unreachable!("SoundCloud-only category"),
    }
}

fn favorite_catalog_id_field(category: Category) -> Option<&'static str> {
    match category {
        Category::Tracks => Some("SNG_ID"),
        Category::Albums => Some("ALB_ID"),
        Category::Artists => Some("ART_ID"),
        Category::Playlists => Some("PLAYLIST_ID"),
        Category::History | Category::Flow | Category::MyTracks | Category::Station => None,
    }
}

fn favorite_request(kind: FavoriteKind, id: &str, favorite: bool) -> (&'static str, Value) {
    match (kind, favorite) {
        (FavoriteKind::Track, true) => ("favorite_song.add", json!({ "SNG_ID": id })),
        (FavoriteKind::Track, false) => ("favorite_song.remove", json!({ "SNG_ID": id })),
        (FavoriteKind::Album, true) => ("album.addFavorite", json!({ "ALB_ID": id })),
        (FavoriteKind::Album, false) => ("album.deleteFavorite", json!({ "ALB_ID": id })),
        (FavoriteKind::Artist, true) => ("artist.addFavorite", json!({ "ART_ID": id })),
        (FavoriteKind::Artist, false) => ("artist.deleteFavorite", json!({ "ART_ID": id })),
        (FavoriteKind::Playlist, true) => {
            ("playlist.addFavorite", json!({ "parent_playlist_id": id }))
        }
        (FavoriteKind::Playlist, false) => {
            ("playlist.deleteFavorite", json!({ "playlist_id": id }))
        }
    }
}

pub(super) fn confirm_true(result: Value, operation: &str) -> Result<(), String> {
    if result == Value::Bool(true) {
        Ok(())
    } else {
        Err(format!("Deezer {operation} did not confirm the change"))
    }
}

pub(super) fn valid_deezer_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 32 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Deezer ids must contain only decimal digits".into());
    }
    Ok(value.to_owned())
}
async fn decode(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!("Deezer returned {}", response.status()));
    }
    response
        .json()
        .await
        .map_err(|_| "Deezer returned an invalid response".into())
}

fn envelope_error_detail(error: &Value) -> Option<String> {
    match error {
        Value::Null | Value::Bool(false) => None,
        Value::Object(value) if value.is_empty() => None,
        Value::Array(value) if value.is_empty() => None,
        Value::String(value) if value.is_empty() => None,
        Value::Number(value) if value.as_i64() == Some(0) => None,
        Value::String(value) => Some(value.clone()),
        Value::Object(_) | Value::Array(_) => Some(error.to_string()),
        _ => Some(error.to_string()),
    }
}

fn envelope_results(envelope: Value, operation: &str) -> Result<Value, String> {
    if let Some(error) = envelope.get("error").and_then(envelope_error_detail) {
        return Err(format!("Deezer {operation} failed: {error}"));
    }
    envelope
        .get("results")
        .cloned()
        .ok_or_else(|| format!("Deezer {operation} response is missing results"))
}

fn valid_user_id(value: &str) -> Option<String> {
    (!value.is_empty() && value != "0" && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.to_owned())
}
fn response_cookies(response: &Response) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
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
        let section = json!({
            "layout": "filterable-grid",
            "items": [item]
        });
        let cards = section
            .get("items")
            .and_then(Value::as_array)
            .unwrap()
            .iter()
            .map(|item| Card {
                kind: Category::Flow,
                id: value_string(item.pointer("/data/id")),
                title: first_string(&[value_string(item.get("title"))], "Flow"),
                subtitle: value_string(item.get("subtitle")),
                artwork: flow_artwork(item),
                source: crate::search::Provider::Deezer,
                ..Card::default()
            })
            .collect::<Vec<_>>();
        assert_eq!(cards[0].id, "genre-danceedm");
        assert_eq!(
            cards[0].artwork,
            "https://e-cdns-images.dzcdn.net/images/cover/flow-art/500x500.jpg"
        );
        assert_eq!(cards[0].source, crate::search::Provider::Deezer);
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
    fn saved_user_id_is_used_only_when_bootstrap_id_is_missing_or_zero() {
        assert_eq!(valid_user_id("42"), Some("42".into()));
        assert_eq!(valid_user_id("0"), None);
        assert_eq!(valid_user_id(""), None);
        assert_eq!(valid_user_id("abc"), None);
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
}
