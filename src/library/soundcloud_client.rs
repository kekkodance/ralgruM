use std::{
    collections::HashSet,
    io::Cursor,
    sync::{Arc, Mutex},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{StreamExt, TryStreamExt, stream};
use image::{ImageFormat, ImageReader};
use reqwest::{Client, Method, Response, Url, cookie::Jar, header};
use serde_json::Value;

use super::model::{Card, Category, Page, Service, Track, root_copy, value_string};
use super::playlist_client::{AddTracksResult, OwnedPlaylist, PlaylistOwner};
use super::{
    playlist_image::{MAX_IMAGE_DIMENSION, MAX_IMAGE_PIXELS},
    playlist_limits::{SOUNDCLOUD_DESCRIPTION_MAX_CHARS, SOUNDCLOUD_TITLE_MAX_CHARS},
};
use crate::search::{
    DetailRoute, Provider, ResultType, SOUNDCLOUD_CLIENT_ID, SearchClient, SoundCloudToken,
};

const API: &str = "https://api-v2.soundcloud.com";
const MOBILE_API: &str = "https://api-mobile.soundcloud.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const MOBILE_USER_AGENT: &str = "ktor-client";
const MOBILE_ACCEPT_ENCODING: &str = "gzip,deflate,identity";
const PLAYLIST_ACCEPT_ENCODING: &str = "gzip, deflate, identity, br";
pub(super) const MAX_PLAYLIST_TRACKS: usize = 500;
const MAX_ARTWORK_BYTES: usize = 10 * 1024 * 1024;
const MAX_ARTWORK_BASE64_BYTES: usize = MAX_ARTWORK_BYTES.div_ceil(3) * 4;
const TRACK_HYDRATION_CONCURRENCY: usize = 4;
const MAX_COLLECTION_PAGES: usize = 50;

#[derive(Clone)]
pub(crate) struct SoundCloudLibraryClient {
    client: Client,
    search_client: Result<SearchClient, String>,
    playlist_client: Client,
    station_client: Client,
    mobile_client: Client,
    mobile_cookies: Arc<Jar>,
    mobile_cookie_source: Arc<Mutex<Option<String>>>,
}

impl SoundCloudLibraryClient {
    pub(crate) fn new() -> Result<Self, String> {
        let client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| "Library client could not be created".to_string())?;
        let mobile_cookies = Arc::new(Jar::default());
        let mut playlist_headers = soundcloud_mobile_headers();
        playlist_headers.insert(
            header::ACCEPT_ENCODING,
            header::HeaderValue::from_static(PLAYLIST_ACCEPT_ENCODING),
        );
        let playlist_client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .default_headers(playlist_headers)
            .build()
            .map_err(|_| "Playlist client could not be created".to_string())?;
        let station_client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .default_headers(soundcloud_mobile_headers())
            .build()
            .map_err(|_| "Station client could not be created".to_string())?;
        let mobile_client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .default_headers(soundcloud_mobile_headers())
            .cookie_provider(mobile_cookies.clone())
            .build()
            .map_err(|_| "Favorite client could not be created".to_string())?;
        Ok(Self {
            client,
            search_client: SearchClient::new().map_err(|error| error.message),
            playlist_client,
            station_client,
            mobile_client,
            mobile_cookies,
            mobile_cookie_source: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) async fn load(
        &self,
        category: Category,
        token: SoundCloudToken,
    ) -> Result<Page, String> {
        if !Service::SoundCloud.categories().contains(&category) {
            return Err("Unsupported SoundCloud library category".into());
        }
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let identity = self.get(format!("{API}/me"), &authorization, &[]).await?;
        let user_id = id(identity.get("id"), "")
            .ok_or_else(|| "SoundCloud did not return an authenticated user ID".to_string())?;
        let (title, description, raw) = match category {
            Category::MyTracks => (
                root_copy(Service::SoundCloud, category).0,
                root_copy(Service::SoundCloud, category).1,
                self.collection(
                    &format!("{API}/users/{user_id}/tracks"),
                    "posted tracks",
                    &authorization,
                )
                .await?,
            ),
            Category::Tracks | Category::Station => {
                let liked = self
                    .collection(
                        &format!("{API}/users/{user_id}/track_likes"),
                        "liked tracks",
                        &authorization,
                    )
                    .await?;
                let tracks = liked
                    .into_iter()
                    .filter_map(|mut item| item.get_mut("track").map(Value::take))
                    .collect();
                (
                    root_copy(Service::SoundCloud, category).0,
                    root_copy(Service::SoundCloud, category).1,
                    tracks,
                )
            }
            Category::Albums => {
                let library = self
                    .collection(
                        &format!("{API}/me/library/all"),
                        "library collections",
                        &authorization,
                    )
                    .await?;
                let playlists = library_playlists(library, category)?;
                (
                    root_copy(Service::SoundCloud, category).0,
                    root_copy(Service::SoundCloud, category).1,
                    playlists,
                )
            }
            Category::Playlists => {
                let created_endpoint = format!("{API}/users/{user_id}/playlists_without_albums");
                let saved_endpoint = format!("{API}/me/library/all");
                let created =
                    self.collection(&created_endpoint, "created playlists", &authorization);
                let saved = self.collection(&saved_endpoint, "library collections", &authorization);
                let (created, saved) = futures::try_join!(created, saved)?;
                let saved = library_playlists(saved, Category::Playlists)?;
                let playlists = merge_soundcloud_playlists(created, saved);
                (
                    root_copy(Service::SoundCloud, category).0,
                    root_copy(Service::SoundCloud, category).1,
                    playlists,
                )
            }
            Category::Artists => (
                root_copy(Service::SoundCloud, category).0,
                root_copy(Service::SoundCloud, category).1,
                collection_value(
                    self.get(
                        format!("{API}/users/{user_id}/followings"),
                        &authorization,
                        &[("limit", "200"), ("offset", "0")],
                    )
                    .await?,
                    "followed artists",
                )?,
            ),
            Category::History => {
                let history = self
                    .collection(
                        &format!("{API}/me/play-history/tracks"),
                        "track history",
                        &authorization,
                    )
                    .await?;
                (
                    root_copy(Service::SoundCloud, category).0,
                    root_copy(Service::SoundCloud, category).1,
                    history
                        .into_iter()
                        .filter_map(|mut item| item.get_mut("track").map(Value::take))
                        .collect(),
                )
            }
            _ => unreachable!("unsupported category"),
        };
        let total = raw.len();
        let mut page = Page {
            title: title.into(),
            description: description.into(),
            total,
            ..Page::default()
        };
        match category {
            Category::Albums | Category::Playlists => {
                page.cards = raw
                    .iter()
                    .map(|playlist| playlist_card(category, playlist))
                    .collect();
                page.empty_title = match category {
                    Category::Albums => "No saved albums",
                    Category::Playlists => "No playlists",
                    _ => unreachable!(),
                }
                .into();
                page.empty_description = match category {
                    Category::Albums => "Albums you save on SoundCloud will appear here.",
                    Category::Playlists => {
                        "Playlists you create or save on SoundCloud will appear here."
                    }
                    _ => unreachable!(),
                }
                .into();
            }
            Category::Artists => {
                page.cards = raw.iter().map(artist_card).collect();
                page.empty_title = "No followed artists".into();
                page.empty_description =
                    "Artists you follow on SoundCloud will appear here.".into();
            }
            Category::Station => {
                page.cards = raw.iter().map(station_card).collect();
                page.empty_title = "No station seeds".into();
                page.empty_description =
                    "Like a SoundCloud track first, then use it to start a station.".into();
            }
            _ => page.tracks = raw.iter().map(track).collect(),
        }
        Ok(page)
    }

    pub(crate) async fn set_favorite(
        &self,
        key: super::favorite_state::FavoriteKey,
        favorite: bool,
        token: SoundCloudToken,
        session_cookies: String,
    ) -> Result<(), String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let request = soundcloud_favorite_request(&key, favorite)?;
        self.seed_mobile_cookies(&session_cookies)?;
        let builder = self
            .mobile_client
            .request(request.method, request.url)
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, request.content_type);
        let builder = if let Some(body) = request.body {
            builder.json(&body)
        } else {
            builder.body("")
        };
        let response = builder
            .send()
            .await
            .map_err(|_| "SoundCloud favorite request failed".to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "SoundCloud favorite request was rejected ({})",
                response.status()
            ))
        }
    }

    fn seed_mobile_cookies(&self, source: &str) -> Result<(), String> {
        let source = source.trim();
        if source.is_empty() {
            return Err("SoundCloud sign-in session is incomplete".into());
        }
        let mut current = self
            .mobile_cookie_source
            .lock()
            .map_err(|_| "SoundCloud sign-in session could not be prepared".to_string())?;
        if current.as_deref() == Some(source) {
            return Ok(());
        }
        let url = Url::parse(MOBILE_API)
            .map_err(|_| "SoundCloud sign-in session could not be prepared".to_string())?;
        if let Some(previous) = current.as_deref() {
            for (name, _) in soundcloud_cookie_pairs(previous)? {
                self.mobile_cookies.add_cookie_str(
                    &format!(
                        "{name}=; Max-Age=0; Domain=.soundcloud.com; Path=/; Secure; SameSite=Lax"
                    ),
                    &url,
                );
            }
        }
        for (name, value) in soundcloud_cookie_pairs(source)? {
            self.mobile_cookies.add_cookie_str(
                &format!("{name}={value}; Domain=.soundcloud.com; Path=/; Secure; SameSite=Lax"),
                &url,
            );
        }
        *current = Some(source.to_owned());
        Ok(())
    }

    pub(crate) async fn load_station(
        &self,
        track_id: String,
        title: String,
        subtitle: String,
        artwork: String,
        token: SoundCloudToken,
    ) -> Result<Page, String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let response = self
            .station_get(station_url(&track_id)?, &authorization)
            .await?;
        let raw = collection_value(response, "track station")?;
        Ok(station_page(title, subtitle, artwork, &raw))
    }

    pub(crate) async fn load_artist_station(
        &self,
        artist_id: String,
        token: SoundCloudToken,
    ) -> Result<Page, String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let response = self
            .station_get(artist_station_url(&artist_id)?, &authorization)
            .await?;
        let raw = collection_value(response, "artist station")?;
        Ok(station_page(
            String::new(),
            String::new(),
            String::new(),
            &raw,
        ))
    }

    pub(crate) async fn load_route(
        &self,
        route: super::model::Route,
        token: Option<SoundCloudToken>,
    ) -> Result<Page, String> {
        if route.source != Provider::SoundCloud {
            return Err("Unsupported SoundCloud library route".into());
        }
        let token = token.ok_or_else(|| "SoundCloud account required".to_string())?;
        match route.action.as_str() {
            "albumTracks" | "playlistTracks" => self.load_playlist(route, token).await,
            "artistTracks" => self.load_artist(route, token).await,
            "stationTracks" => {
                self.load_station(route.id, route.title, route.subtitle, route.artwork, token)
                    .await
            }
            _ => Err("Unsupported SoundCloud library route".into()),
        }
    }

    pub(crate) async fn owned_playlists(
        &self,
        token: SoundCloudToken,
    ) -> Result<Vec<OwnedPlaylist>, String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let identity = self.get(format!("{API}/me"), &authorization, &[]).await?;
        let user_id = id(identity.get("id"), "")
            .ok_or_else(|| "SoundCloud did not return an authenticated user ID".to_string())?;
        let raw = self
            .collection(
                &format!("{API}/users/{user_id}/playlists_without_albums"),
                "owned playlists",
                &authorization,
            )
            .await?;
        owned_playlists_from(&raw)
    }

    pub(crate) async fn create_playlist(
        &self,
        token: SoundCloudToken,
        title: &str,
        description: &str,
        private: bool,
        track_ids: &[String],
    ) -> Result<String, String> {
        let write = create_playlist_write(title, description, private, track_ids)?;
        let created = self
            .send_mobile_json(write.method, write.url, &token, &write.body)
            .await?;
        created_playlist_id(&created)
    }

    pub(crate) async fn update_playlist(
        &self,
        token: SoundCloudToken,
        owned: OwnedPlaylist,
        title: &str,
        description: &str,
        private: bool,
        artwork_base64: Option<&str>,
    ) -> Result<OwnedPlaylist, String> {
        let playlist_id = positive_numeric_id(&owned.id, "")
            .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
        if let Some(image_data) = artwork_base64 {
            validate_artwork_base64(image_data)?;
        }
        let title = valid_playlist_text(title, false, SOUNDCLOUD_TITLE_MAX_CHARS)?;
        let description = valid_playlist_text(description, true, SOUNDCLOUD_DESCRIPTION_MAX_CHARS)?;
        let write = update_playlist_write(&playlist_id, &title, &description, private)?;
        let response = self
            .send_mobile_json(write.method, write.url, &token, &write.body)
            .await?;
        let mut updated =
            updated_playlist_from_response(&response, &owned, &title, &description, private)?;
        if let Some(image_data) = artwork_base64 {
            updated.artwork = self
                .upload_playlist_artwork(&token, &playlist_id, image_data)
                .await?;
        }
        Ok(updated)
    }

    pub(crate) async fn upload_playlist_artwork(
        &self,
        token: &SoundCloudToken,
        playlist_id: &str,
        image_data: &str,
    ) -> Result<String, String> {
        validate_artwork_base64(image_data)?;
        let artwork = artwork_write(playlist_id, image_data)?;
        let response = self
            .send_mobile_json(artwork.method, artwork.url, token, &artwork.body)
            .await?;
        artwork_url_from_response(&response)
    }

    pub(crate) async fn delete_playlist(
        &self,
        token: SoundCloudToken,
        playlist_id: &str,
    ) -> Result<bool, String> {
        let playlist_id = positive_numeric_id(playlist_id, "")
            .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let response = self
            .mobile_request(
                Method::DELETE,
                format!("{API}/playlists/{playlist_id}"),
                authorization,
            )
            .send()
            .await
            .map_err(|_| "SoundCloud playlist delete request failed".to_string())?;
        delete_response_status(response.status())
    }

    pub(crate) async fn add_tracks_to_playlist(
        &self,
        token: SoundCloudToken,
        playlist_id: &str,
        track_ids: &[String],
    ) -> Result<AddTracksResult, String> {
        let playlist_id = numeric_id(playlist_id, "")
            .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
        if track_ids.is_empty() {
            return Err("No tracks were provided to add.".into());
        }
        let requested = track_ids
            .iter()
            .map(|id| {
                numeric_id(id, "").ok_or_else(|| "Valid SoundCloud track ID required".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let playlist = self
            .mobile_get(
                format!("{API}/playlists/{playlist_id}"),
                &authorization,
                &[("representation", "full")],
            )
            .await?;
        let existing = if playlist_tracks_are_truncated(&playlist) {
            let tracks = self
                .mobile_collection(
                    &format!("{API}/playlists/{playlist_id}/tracks"),
                    "playlist tracks",
                    &authorization,
                )
                .await?;
            playlist_ids_from_collection(&tracks)?
        } else {
            playlist_ids_for_update(&playlist)?
        };
        match plan_add_tracks(&existing, &requested)? {
            AddTracksPlan::AlreadyPresent { count } => {
                Ok(AddTracksResult::AlreadyPresent { count })
            }
            AddTracksPlan::Update { track_ids, result } => {
                let write = add_tracks_write(&playlist_id, &track_ids)?;
                self.send_mobile_json(write.method, write.url, &token, &write.body)
                    .await?;
                Ok(result)
            }
        }
    }

    pub(crate) async fn reorder_playlist(
        &self,
        token: SoundCloudToken,
        playlist_id: &str,
        submitted_order: &[String],
    ) -> Result<bool, String> {
        let playlist_id = positive_numeric_id(playlist_id, "")
            .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
        let submitted = validate_reorder_ids(submitted_order)?;
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let playlist = self
            .mobile_get(
                format!("{API}/playlists/{playlist_id}"),
                &authorization,
                &[("representation", "full")],
            )
            .await?;
        if playlist_id_from_value(&playlist).as_deref() != Some(&playlist_id) {
            return Err("SoundCloud returned a different playlist than requested".into());
        }
        let expected_count = playlist_track_count(&playlist).ok_or_else(|| {
            "SoundCloud returned an incomplete playlist track collection".to_string()
        })?;
        let current = if playlist_tracks_are_truncated(&playlist) {
            let tracks = self
                .mobile_collection(
                    &format!("{API}/playlists/{playlist_id}/tracks"),
                    "playlist tracks",
                    &authorization,
                )
                .await?;
            playlist_ids_from_collection(&tracks)?
        } else {
            playlist_ids_for_update(&playlist)?
        };
        if current.len() != expected_count {
            return Err("SoundCloud returned an incomplete playlist track collection".into());
        }
        let current = validate_reorder_ids(&current)?;
        if current.len() != submitted.len()
            || current.iter().copied().collect::<HashSet<_>>()
                != submitted.iter().copied().collect::<HashSet<_>>()
        {
            return Err("SoundCloud playlist order does not match the current playlist".into());
        }
        let write = reorder_playlist_write(&playlist_id, &submitted)?;
        let response = self
            .send_mobile_json(write.method, write.url, &token, &write.body)
            .await?;
        validate_reordered_playlist_response(&response, &playlist_id)?;
        Ok(true)
    }

    async fn load_playlist(
        &self,
        route: super::model::Route,
        token: SoundCloudToken,
    ) -> Result<Page, String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message)?;
        let playlist_id = numeric_id(&route.id, "soundcloud:playlists:")
            .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
        let playlist = self
            .get(
                format!("{API}/playlists/{playlist_id}"),
                &authorization,
                &[("representation", "owner")],
            )
            .await?;
        let track_ids = if playlist
            .get("tracks")
            .and_then(Value::as_array)
            .is_some_and(|tracks| {
                playlist
                    .get("track_count")
                    .and_then(Value::as_u64)
                    .is_none_or(|count| count <= tracks.len() as u64)
            }) {
            playlist_track_ids(&playlist)?
        } else {
            self.collection(
                &format!("{API}/playlists/{playlist_id}/tracks"),
                "playlist tracks",
                &authorization,
            )
            .await?
            .iter()
            .map(|item| soundcloud_track_id(item.get("track").unwrap_or(item)))
            .map(|id| {
                id.ok_or_else(|| {
                    "SoundCloud returned a playlist track without a valid ID".to_string()
                })
            })
            .collect::<Result<Vec<_>, _>>()?
        };
        let tracks = self.hydrate_tracks(&track_ids, &authorization).await?;
        if tracks.len() != track_ids.len() {
            return Err("SoundCloud returned an incomplete playlist track collection".into());
        }
        Ok(playlist_page(&route, &playlist, &tracks, track_ids.len()))
    }

    async fn hydrate_tracks(
        &self,
        track_ids: &[String],
        authorization: &header::HeaderValue,
    ) -> Result<Vec<Value>, String> {
        let chunks = track_ids
            .chunks(50)
            .map(|chunk| chunk.to_owned())
            .collect::<Vec<_>>();
        let hydrated = stream::iter(chunks.into_iter().map(|chunk| async move {
            let ids = chunk.join(",");
            let response = self
                .get(
                    format!("{API}/tracks"),
                    authorization,
                    &[("ids", ids.as_str())],
                )
                .await?;
            response
                .as_array()
                .ok_or_else(|| {
                    "SoundCloud returned an invalid track hydration response".to_string()
                })
                .map(|items| items.to_vec())
        }))
        .buffered(TRACK_HYDRATION_CONCURRENCY)
        .try_collect::<Vec<Vec<Value>>>()
        .await?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let by_id = hydrated
            .into_iter()
            .filter_map(|value| id(value.get("id"), "").map(|id| (id, value)))
            .collect::<std::collections::HashMap<_, _>>();
        Ok(track_ids
            .iter()
            .filter_map(|id| by_id.get(id).cloned())
            .collect())
    }

    async fn load_artist(
        &self,
        route: super::model::Route,
        token: SoundCloudToken,
    ) -> Result<Page, String> {
        let client = self
            .search_client
            .clone()
            .map_err(|error| error.to_owned())?;
        let detail = client
            .detail(
                DetailRoute {
                    provider: Provider::SoundCloud,
                    kind: ResultType::Artists,
                    id: route.id,
                    title: route.title,
                    subtitle: route.subtitle,
                    artwork: route.artwork,
                    release_date: route.release_date,
                    service_url: String::new(),
                },
                None,
                Some(token),
            )
            .await
            .map_err(|error| error.message)?;
        Ok(super::client::detail_page(detail))
    }

    async fn collection(
        &self,
        endpoint: &str,
        context: &str,
        authorization: &header::HeaderValue,
    ) -> Result<Vec<Value>, String> {
        let path = Url::parse(endpoint)
            .map_err(|_| format!("Invalid SoundCloud {context} endpoint"))?
            .path()
            .to_owned();
        let mut offset = "0".to_owned();
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for _ in 0..MAX_COLLECTION_PAGES {
            if !seen.insert(offset.clone()) {
                // SoundCloud re-serves a continuation cursor when a page boundary
                // lands on equal-timestamp play-history entries or at its history
                // depth cap. There is no further page, so end the walk with the
                // items collected so far instead of failing the whole load.
                return Ok(items);
            }
            let response = self
                .get(
                    endpoint.to_owned(),
                    authorization,
                    &[("limit", "200"), ("offset", offset.as_str())],
                )
                .await?;
            let next = next_offset(&response, &path)?;
            let page = collection_value(response, context)?;
            if page.is_empty() {
                return Ok(items);
            }
            items.extend(page);
            let Some(next) = next else {
                return Ok(items);
            };
            offset = next;
        }
        Ok(items)
    }

    async fn get(
        &self,
        url: String,
        authorization: &header::HeaderValue,
        query: &[(&str, &str)],
    ) -> Result<Value, String> {
        let response = self
            .client
            .get(url)
            .header(header::AUTHORIZATION, authorization.clone())
            .query(&[("client_id", SOUNDCLOUD_CLIENT_ID)])
            .query(query)
            .send()
            .await
            .map_err(|_| "SoundCloud library request failed".to_string())?;
        decode(response).await
    }

    async fn station_get(
        &self,
        url: String,
        authorization: &header::HeaderValue,
    ) -> Result<Value, String> {
        let response = self
            .station_request(url, authorization.clone())
            .send()
            .await
            .map_err(|_| "SoundCloud station request failed".to_string())?;
        decode(response).await
    }

    fn station_request(
        &self,
        url: String,
        authorization: header::HeaderValue,
    ) -> reqwest::RequestBuilder {
        self.station_client
            .get(url)
            .header(header::USER_AGENT, MOBILE_USER_AGENT)
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_ENCODING, MOBILE_ACCEPT_ENCODING)
            .header(header::AUTHORIZATION, authorization)
            .query(&[("client_id", SOUNDCLOUD_CLIENT_ID)])
    }

    async fn mobile_collection(
        &self,
        endpoint: &str,
        context: &str,
        authorization: &header::HeaderValue,
    ) -> Result<Vec<Value>, String> {
        let path = Url::parse(endpoint)
            .map_err(|_| format!("Invalid SoundCloud {context} endpoint"))?
            .path()
            .to_owned();
        let mut offset = "0".to_owned();
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for _ in 0..MAX_COLLECTION_PAGES {
            if !seen.insert(offset.clone()) {
                // SoundCloud re-serves a continuation cursor when a page boundary
                // lands on equal-timestamp play-history entries or at its history
                // depth cap. There is no further page, so end the walk with the
                // items collected so far instead of failing the whole load.
                return Ok(items);
            }
            let response = self
                .mobile_get(
                    endpoint.to_owned(),
                    authorization,
                    &[("limit", "200"), ("offset", offset.as_str())],
                )
                .await?;
            let next = next_offset(&response, &path)?;
            let page = collection_value(response, context)?;
            if page.is_empty() {
                return Ok(items);
            }
            items.extend(page);
            let Some(next) = next else {
                return Ok(items);
            };
            offset = next;
        }
        Ok(items)
    }

    async fn mobile_get(
        &self,
        url: String,
        authorization: &header::HeaderValue,
        query: &[(&str, &str)],
    ) -> Result<Value, String> {
        let response = self
            .mobile_request(Method::GET, url, authorization.clone())
            .query(query)
            .send()
            .await
            .map_err(|_| "SoundCloud playlist request failed".to_string())?;
        decode(response).await
    }

    fn mobile_request(
        &self,
        method: Method,
        url: String,
        authorization: header::HeaderValue,
    ) -> reqwest::RequestBuilder {
        self.playlist_client
            .request(method, url)
            .header(header::USER_AGENT, MOBILE_USER_AGENT)
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_ENCODING, PLAYLIST_ACCEPT_ENCODING)
            .header(header::AUTHORIZATION, authorization)
    }

    async fn send_mobile_json(
        &self,
        method: Method,
        url: String,
        token: &SoundCloudToken,
        body: &Value,
    ) -> Result<Value, String> {
        let authorization = token
            .authorization_header()
            .map_err(|error| error.message.clone())?;
        let response = self
            .mobile_request(method, url, authorization)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(body)
            .send()
            .await
            .map_err(|_| "SoundCloud library request failed".to_string())?;
        decode(response).await
    }
}

/// Album identity a SoundCloud album page stamps onto its tracks. Track
/// payloads carry no album id, so the album page is the only source; giving
/// each track the id and title lets menus away from the page (the player
/// bar, the queue) resolve the album, matching Deezer tracks that always
/// carry ALB_ID. Playlist pages leave track album data untouched.
fn album_member(mut track: Track, route: &super::model::Route, playlist_title: &str) -> Track {
    if route.action != "albumTracks" {
        return track;
    }
    let album_id = route.id.trim();
    if track.album_id.trim().is_empty()
        && !album_id.is_empty()
        && album_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        track.album_id = album_id.to_owned();
    }
    let album_title = [playlist_title, route.title.as_str()]
        .into_iter()
        .map(str::trim)
        .find(|title| !title.is_empty())
        .unwrap_or_default();
    if track.album.trim().is_empty() && !album_title.is_empty() {
        track.album = album_title.to_owned();
    }
    track
}

fn playlist_page(
    route: &super::model::Route,
    playlist: &Value,
    tracks: &[Value],
    authoritative_total: usize,
) -> Page {
    let description = value_string(playlist.get("description"));
    let title = value_string(playlist.get("title"));
    Page {
        title: title.clone(),
        subtitle: value_string(playlist.pointer("/user/username")),
        album_info: matches!(route.category, Category::Albums | Category::Playlists)
            .then(|| crate::search::soundcloud_album_info(playlist, &description)),
        description,
        artwork: playlist_artwork(playlist),
        show_count: true,
        count_noun: "track".into(),
        total: tracks.len(),
        authoritative_total: Some(authoritative_total),
        raw_loaded_count: authoritative_total,
        normalized_count: tracks.len(),
        platform: detail_platform(&route.action),
        service_url: crate::search::soundcloud_service_url(playlist),
        tracks: tracks
            .iter()
            .map(|value| album_member(track(value), route, &title))
            .collect(),
        ..Page::default()
    }
}

fn soundcloud_mobile_headers() -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(MOBILE_USER_AGENT),
    );
    headers.insert(header::ACCEPT, header::HeaderValue::from_static("*/*"));
    headers.insert(
        header::ACCEPT_ENCODING,
        header::HeaderValue::from_static(MOBILE_ACCEPT_ENCODING),
    );
    headers
}

#[derive(Debug, PartialEq)]
struct SoundCloudFavoriteRequest {
    method: Method,
    url: String,
    content_type: &'static str,
    body: Option<Value>,
}

fn soundcloud_favorite_request(
    key: &super::favorite_state::FavoriteKey,
    favorite: bool,
) -> Result<SoundCloudFavoriteRequest, String> {
    if key.provider != Provider::SoundCloud {
        return Err("Unsupported SoundCloud favorite provider".into());
    }
    let id = numeric_id(&key.id, "")
        .ok_or_else(|| "Valid SoundCloud favorite ID required".to_string())?;
    match key.kind {
        super::favorite_state::FavoriteKind::Artist => Ok(SoundCloudFavoriteRequest {
            method: if favorite {
                Method::POST
            } else {
                Method::DELETE
            },
            url: format!("{MOBILE_API}/follows/users/soundcloud:users:{id}"),
            content_type: "application/x-www-form-urlencoded; charset=UTF-8",
            body: None,
        }),
        super::favorite_state::FavoriteKind::Album
        | super::favorite_state::FavoriteKind::Playlist => {
            let operation = if favorite { "create" } else { "delete" };
            Ok(SoundCloudFavoriteRequest {
                method: Method::POST,
                url: format!("{MOBILE_API}/likes/playlists/{operation}"),
                content_type: "application/json; charset=UTF-8",
                body: Some(serde_json::json!({
                    "likes": [{"target_urn": format!("soundcloud:playlists:{id}")}]
                })),
            })
        }
        super::favorite_state::FavoriteKind::Track => {
            let operation = if favorite { "create" } else { "delete" };
            Ok(SoundCloudFavoriteRequest {
                method: Method::POST,
                url: format!("{MOBILE_API}/likes/tracks/{operation}"),
                content_type: "application/json; charset=UTF-8",
                body: Some(serde_json::json!({
                    "likes": [{"target_urn": format!("soundcloud:tracks:{id}")}]
                })),
            })
        }
    }
}

fn station_url(track_id: &str) -> Result<String, String> {
    let track_id = id(Some(&Value::String(track_id.trim().to_owned())), "")
        .ok_or_else(|| "Valid SoundCloud track ID required".to_string())?;
    Ok(format!(
        "{API}/stations/soundcloud:track-stations:{track_id}/tracks"
    ))
}

fn artist_station_url(artist_id: &str) -> Result<String, String> {
    let artist_id = numeric_id(artist_id, "")
        .ok_or_else(|| "Valid SoundCloud artist ID required".to_string())?;
    if artist_id == "0" {
        return Err("Valid SoundCloud artist ID required".into());
    }
    Ok(format!(
        "{API}/stations/soundcloud:artist-stations:{artist_id}/tracks"
    ))
}

#[cfg(test)]
fn artist_endpoints(artist_id: &str) -> Result<(String, String), String> {
    let artist_id = id(Some(&Value::String(artist_id.trim().to_owned())), "")
        .ok_or_else(|| "Valid SoundCloud artist ID required".to_string())?;
    Ok((
        format!("{API}/users/{artist_id}"),
        format!("{API}/users/{artist_id}/tracks"),
    ))
}

fn numeric_id(value: &str, prefix: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix(prefix).unwrap_or(value);
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| value.into())
}

fn positive_numeric_id(value: &str, prefix: &str) -> Option<String> {
    let value = numeric_id(value, prefix)?;
    let number = value.parse::<u64>().ok()?;
    (number > 0).then(|| number.to_string())
}

fn soundcloud_cookie_pairs(source: &str) -> Result<Vec<(&str, &str)>, String> {
    let pairs = source
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            (!part.is_empty()).then_some(part)
        })
        .map(|part| {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| "SoundCloud sign-in session is invalid".to_string())?;
            let name = name.trim();
            let value = value.trim();
            let valid_name = !name.is_empty()
                && name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(
                            byte,
                            b'!' | b'#'
                                | b'$'
                                | b'%'
                                | b'&'
                                | b'\''
                                | b'*'
                                | b'+'
                                | b'-'
                                | b'.'
                                | b'^'
                                | b'_'
                                | b'`'
                                | b'|'
                                | b'~'
                        )
                });
            let valid_value = !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte >= 0x21 && byte != 0x7f && byte != b';');
            if valid_name && valid_value {
                Ok((name, value))
            } else {
                Err("SoundCloud sign-in session is invalid".to_string())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if pairs.is_empty() {
        Err("SoundCloud sign-in session is incomplete".into())
    } else {
        Ok(pairs)
    }
}

fn playlist_track_ids(value: &Value) -> Result<Vec<String>, String> {
    value
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or_else(|| "SoundCloud returned an invalid playlist response".to_string())?
        .iter()
        .map(soundcloud_track_id)
        .map(|id| {
            id.ok_or_else(|| "SoundCloud returned a playlist track without a valid ID".to_string())
        })
        .collect()
}

fn playlist_ids_for_update(value: &Value) -> Result<Vec<String>, String> {
    playlist_track_ids(value)
}

fn playlist_ids_from_collection(raw: &[Value]) -> Result<Vec<String>, String> {
    raw.iter()
        .map(|item| soundcloud_track_id(item.get("track").unwrap_or(item)))
        .map(|id| {
            id.ok_or_else(|| "SoundCloud returned a playlist track without a valid ID".to_string())
        })
        .collect()
}

fn playlist_id_from_value(value: &Value) -> Option<String> {
    id(value.get("id"), "")
        .or_else(|| id(value.get("urn"), "soundcloud:playlists:"))
        .and_then(|id| positive_numeric_id(&id, ""))
}

fn validate_reorder_ids(ids: &[String]) -> Result<Vec<u64>, String> {
    if ids.len() < 2 {
        return Err("SoundCloud playlist order requires at least two tracks".into());
    }
    if ids.len() > MAX_PLAYLIST_TRACKS {
        return Err(format!(
            "SoundCloud playlist updates support at most {MAX_PLAYLIST_TRACKS} tracks."
        ));
    }
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter()
        .map(|id| positive_track_id(id))
        .map(|id| {
            let id = id?;
            if !seen.insert(id) {
                return Err("SoundCloud playlist order contains duplicate track IDs".into());
            }
            Ok(id)
        })
        .collect()
}

fn soundcloud_track_id(value: &Value) -> Option<String> {
    id(value.get("id"), "")
        .or_else(|| id(value.get("urn"), "soundcloud:tracks:"))
        .or_else(|| id(Some(value), "soundcloud:tracks:"))
}

fn playlist_tracks_are_truncated(value: &Value) -> bool {
    let Some(track_count) = playlist_track_count(value) else {
        return true;
    };
    let Some(embedded) = value.get("tracks").and_then(Value::as_array) else {
        return true;
    };
    track_count > embedded.len()
}

fn playlist_track_count(value: &Value) -> Option<usize> {
    value
        .get("track_count")
        .and_then(|count| {
            count
                .as_u64()
                .or_else(|| count.as_str()?.parse::<u64>().ok())
        })
        .and_then(|count| usize::try_from(count).ok())
}

#[derive(Debug, PartialEq)]
struct SoundCloudPlaylistWrite {
    method: Method,
    url: String,
    body: Value,
}

#[derive(Debug, PartialEq)]
enum AddTracksPlan {
    AlreadyPresent {
        count: usize,
    },
    Update {
        track_ids: Vec<u64>,
        result: AddTracksResult,
    },
}

fn create_playlist_write(
    title: &str,
    description: &str,
    private: bool,
    track_ids: &[String],
) -> Result<SoundCloudPlaylistWrite, String> {
    let title = valid_playlist_text(title, false, SOUNDCLOUD_TITLE_MAX_CHARS)?;
    let description = valid_playlist_text(description, true, SOUNDCLOUD_DESCRIPTION_MAX_CHARS)?;
    let mut playlist = serde_json::Map::new();
    playlist.insert("title".into(), Value::String(title));
    playlist.insert("description".into(), Value::String(description));
    playlist.insert(
        "sharing".into(),
        Value::String((if private { "private" } else { "public" }).into()),
    );
    let tracks = track_ids
        .iter()
        .map(|id| positive_track_id(id).map(Value::from))
        .collect::<Result<Vec<_>, _>>()?;
    if tracks.len() > MAX_PLAYLIST_TRACKS {
        return Err(format!(
            "SoundCloud playlist creation supports at most {MAX_PLAYLIST_TRACKS} tracks."
        ));
    }
    playlist.insert("tracks".into(), Value::Array(tracks));
    Ok(SoundCloudPlaylistWrite {
        method: Method::POST,
        url: format!("{API}/playlists"),
        body: serde_json::json!({ "playlist": playlist }),
    })
}

fn update_playlist_write(
    playlist_id: &str,
    title: &str,
    description: &str,
    private: bool,
) -> Result<SoundCloudPlaylistWrite, String> {
    let playlist_id = positive_numeric_id(playlist_id, "")
        .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
    let title = valid_playlist_text(title, false, SOUNDCLOUD_TITLE_MAX_CHARS)?;
    let description = valid_playlist_text(description, true, SOUNDCLOUD_DESCRIPTION_MAX_CHARS)?;
    Ok(SoundCloudPlaylistWrite {
        method: Method::PUT,
        url: format!("{API}/playlists/{playlist_id}"),
        body: serde_json::json!({
            "playlist": {
                "title": title,
                "description": description,
                "sharing": if private { "private" } else { "public" },
                "public": !private,
            }
        }),
    })
}

fn artwork_write(playlist_id: &str, image_data: &str) -> Result<SoundCloudPlaylistWrite, String> {
    let playlist_id = positive_numeric_id(playlist_id, "")
        .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
    Ok(SoundCloudPlaylistWrite {
        method: Method::PUT,
        url: format!("{API}/playlists/soundcloud:playlists:{playlist_id}/artwork"),
        body: serde_json::json!({ "image_data": image_data }),
    })
}

fn valid_playlist_text(value: &str, multiline: bool, max_chars: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() && !multiline {
        return Err("SoundCloud playlist title required".into());
    }
    if value.chars().count() > max_chars
        || value.chars().any(|character| {
            character.is_control() && (!multiline || !matches!(character, '\n' | '\r' | '\t'))
        })
    {
        return Err("SoundCloud playlist text is invalid".into());
    }
    Ok(value.to_owned())
}

pub(super) fn validate_artwork_base64(image_data: &str) -> Result<(), String> {
    if image_data.trim().is_empty() || image_data != image_data.trim() {
        return Err("SoundCloud artwork must be raw Base64 JPEG data".into());
    }
    if !artwork_base64_size_allowed(image_data.len()) {
        return Err("SoundCloud artwork exceeds the 10 MB upload limit".into());
    }
    let bytes = STANDARD
        .decode(image_data)
        .map_err(|_| "SoundCloud artwork must be valid Base64".to_string())?;
    if bytes.is_empty() || bytes.len() > MAX_ARTWORK_BYTES {
        return Err("SoundCloud artwork must be between 1 byte and 10 MB".into());
    }
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| "SoundCloud artwork must be a valid JPEG".to_string())?;
    if reader.format() != Some(ImageFormat::Jpeg) {
        return Err("SoundCloud artwork must be a valid JPEG".into());
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| "SoundCloud artwork dimensions could not be read".to_string())?;
    if width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err("SoundCloud artwork dimensions exceed the supported limit".into());
    }
    image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
        .map_err(|_| "SoundCloud artwork must be a valid JPEG".to_string())?;
    Ok(())
}

fn artwork_base64_size_allowed(encoded_len: usize) -> bool {
    encoded_len <= MAX_ARTWORK_BASE64_BYTES
}

fn updated_playlist_from_response(
    value: &Value,
    existing: &OwnedPlaylist,
    submitted_title: &str,
    submitted_description: &str,
    submitted_private: bool,
) -> Result<OwnedPlaylist, String> {
    let playlist = value.get("playlist").unwrap_or(value);
    if playlist.get("kind").and_then(Value::as_str) != Some("playlist")
        || playlist_id_from_value(playlist).as_deref() != Some(existing.id.as_str())
    {
        return Err("SoundCloud returned an invalid updated playlist response".into());
    }
    let artwork = match playlist.get("artwork_url") {
        None => None,
        Some(Value::Null) => None,
        Some(Value::String(value)) if value.trim().is_empty() => None,
        Some(Value::String(value)) => Some(safe_soundcloud_artwork_url(value)?),
        Some(_) => return Err("SoundCloud returned an invalid artwork URL".into()),
    };
    let mut updated = existing.clone();
    updated.title = submitted_title.to_owned();
    updated.description = submitted_description.to_owned();
    updated.is_private = submitted_private;
    if let Some(artwork) = artwork {
        updated.artwork = artwork;
    }
    Ok(updated)
}

fn artwork_url_from_response(value: &Value) -> Result<String, String> {
    let artwork = value
        .get("artwork_url")
        .and_then(Value::as_str)
        .ok_or_else(|| "SoundCloud artwork response is missing its URL".to_string())?;
    safe_soundcloud_artwork_url(artwork)
}

fn delete_response_status(status: reqwest::StatusCode) -> Result<bool, String> {
    status
        .is_success()
        .then_some(true)
        .ok_or_else(|| format!("SoundCloud returned {status}"))
}

fn safe_soundcloud_artwork_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value.trim())
        .map_err(|_| "SoundCloud returned an invalid artwork URL".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "SoundCloud returned an artwork URL without a host".to_string())?;
    let soundcloud_host = host == "soundcloud.com"
        || host.ends_with(".soundcloud.com")
        || host == "sndcdn.com"
        || host.ends_with(".sndcdn.com");
    if url.scheme() != "https"
        || !soundcloud_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return Err("SoundCloud returned an unsafe artwork URL".into());
    }
    Ok(url.to_string())
}

fn add_tracks_write(
    playlist_id: &str,
    track_ids: &[u64],
) -> Result<SoundCloudPlaylistWrite, String> {
    let playlist_id = numeric_id(playlist_id, "")
        .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
    Ok(SoundCloudPlaylistWrite {
        method: Method::PUT,
        url: format!("{API}/playlists/{playlist_id}"),
        body: serde_json::json!({ "playlist": { "tracks": track_ids } }),
    })
}

fn reorder_playlist_write(
    playlist_id: &str,
    track_ids: &[u64],
) -> Result<SoundCloudPlaylistWrite, String> {
    let playlist_id = positive_numeric_id(playlist_id, "")
        .ok_or_else(|| "Valid SoundCloud playlist ID required".to_string())?;
    let track_ids = track_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let validated = validate_reorder_ids(&track_ids)?;
    Ok(SoundCloudPlaylistWrite {
        method: Method::PUT,
        url: format!("{API}/playlists/{playlist_id}"),
        body: serde_json::json!({ "playlist": { "tracks": validated } }),
    })
}

fn plan_add_tracks(existing: &[String], requested: &[String]) -> Result<AddTracksPlan, String> {
    if requested.is_empty() {
        return Err("No tracks were provided to add.".into());
    }
    let mut present = HashSet::new();
    let mut existing_ids = Vec::with_capacity(existing.len());
    for id in existing {
        let number = track_id_number(id)?;
        present.insert(number);
        existing_ids.push(number);
    }
    let mut added = Vec::new();
    let mut added_count = 0usize;
    let mut duplicated = 0usize;
    for id in requested {
        let number = track_id_number(id)?;
        if present.contains(&number) {
            duplicated += 1;
        } else {
            present.insert(number);
            added.push(number);
            added_count += 1;
        }
    }
    if added.is_empty() {
        return Ok(AddTracksPlan::AlreadyPresent {
            count: requested.len(),
        });
    }
    if existing_ids.len().saturating_add(added.len()) > MAX_PLAYLIST_TRACKS {
        return Err(format!(
            "SoundCloud playlist updates support at most {MAX_PLAYLIST_TRACKS} tracks."
        ));
    }
    existing_ids.extend(added);
    let result = if duplicated == 0 {
        AddTracksResult::Added { count: added_count }
    } else {
        AddTracksResult::Partial {
            added: added_count,
            duplicated,
        }
    };
    Ok(AddTracksPlan::Update {
        track_ids: existing_ids,
        result,
    })
}

fn created_playlist_id(value: &Value) -> Result<String, String> {
    if value.get("kind").and_then(Value::as_str) != Some("playlist") {
        return Err("SoundCloud returned an invalid playlist response".into());
    }
    id(value.get("id"), "")
        .ok_or_else(|| "SoundCloud returned a playlist without a valid ID".to_string())
}

fn validate_reordered_playlist_response(value: &Value, expected_id: &str) -> Result<(), String> {
    let playlist = value.get("playlist").unwrap_or(value);
    if playlist.get("kind").and_then(Value::as_str) != Some("playlist")
        || playlist_id_from_value(playlist).as_deref() != Some(expected_id)
    {
        return Err("SoundCloud returned an invalid reordered playlist response".into());
    }
    Ok(())
}

fn owned_playlists_from(raw: &[Value]) -> Result<Vec<OwnedPlaylist>, String> {
    raw.iter()
        .filter(|playlist| playlist.get("is_album").and_then(Value::as_bool) != Some(true))
        .map(owned_playlist)
        .collect()
}

fn owned_playlist(value: &Value) -> Result<OwnedPlaylist, String> {
    let playlist_id = id(value.get("id"), "")
        .or_else(|| id(value.get("urn"), "soundcloud:playlists:"))
        .ok_or_else(|| "SoundCloud returned a playlist without a valid ID".to_string())?;
    let owner = value
        .get("user")
        .ok_or_else(|| "SoundCloud returned a playlist without owner metadata".to_string())?;
    let owner_id = id(owner.get("id"), "")
        .or_else(|| id(owner.get("urn"), "soundcloud:users:"))
        .ok_or_else(|| "SoundCloud returned a playlist without a valid owner ID".to_string())?;
    let sharing = value.get("sharing").and_then(Value::as_str);
    let public = value.get("public").and_then(Value::as_bool);
    Ok(OwnedPlaylist {
        id: playlist_id,
        title: value_string(value.get("title")),
        description: value_string(value.get("description")),
        is_private: sharing == Some("private") || public == Some(false),
        is_from_favorite_tracks: false,
        is_collaborative: false,
        owner: PlaylistOwner {
            id: owner_id,
            name: value_string(owner.get("username")),
        },
        artwork: playlist_artwork(value),
        track_count: value
            .get("track_count")
            .and_then(|count| count.as_u64().or_else(|| count.as_str()?.parse().ok())),
    })
}

fn track_id_number(value: &str) -> Result<u64, String> {
    numeric_id(value, "")
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| "Valid SoundCloud track ID required".to_string())
}

fn positive_track_id(value: &str) -> Result<u64, String> {
    let id = track_id_number(value)?;
    (id > 0)
        .then_some(id)
        .ok_or_else(|| "Valid SoundCloud track ID required".to_string())
}

fn detail_platform(action: &str) -> Option<Service> {
    matches!(
        action,
        "albumTracks" | "playlistTracks" | "artistTracks" | "stationTracks"
    )
    .then_some(Service::SoundCloud)
}

fn library_playlists(raw: Vec<Value>, category: Category) -> Result<Vec<Value>, String> {
    let want_album = match category {
        Category::Albums => true,
        Category::Playlists => false,
        _ => return Err("Invalid SoundCloud collection category".into()),
    };
    let mut playlists = Vec::new();
    for mut item in raw {
        if item.get("type").and_then(Value::as_str) != Some("playlist-like") {
            continue;
        }
        let playlist = item.get_mut("playlist").map(Value::take).ok_or_else(|| {
            "SoundCloud returned a library playlist without playlist metadata".to_string()
        })?;
        let is_album = playlist
            .get("is_album")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                "SoundCloud returned a library playlist without its album type".to_string()
            })?;
        if is_album == want_album {
            playlists.push(playlist);
        }
    }
    Ok(playlists)
}

fn merge_soundcloud_playlists(created: Vec<Value>, saved: Vec<Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    created
        .into_iter()
        .chain(saved)
        .filter(|playlist| playlist.get("is_album").and_then(Value::as_bool) != Some(true))
        .filter_map(|playlist| {
            let id = soundcloud_playlist_id(&playlist)?;
            seen.insert(id).then_some(playlist)
        })
        .collect()
}

fn soundcloud_playlist_id(value: &Value) -> Option<String> {
    id(value.get("id"), "").or_else(|| id(value.get("urn"), "soundcloud:playlists:"))
}

fn collection_value(mut value: Value, context: &str) -> Result<Vec<Value>, String> {
    value
        .get_mut("collection")
        .and_then(Value::as_array_mut)
        .map(std::mem::take)
        .ok_or_else(|| format!("SoundCloud returned an invalid {context} response"))
}

fn next_offset(value: &Value, expected_path: &str) -> Result<Option<String>, String> {
    let Some(next) = value.get("next_href") else {
        return Ok(None);
    };
    let Some(next) = next.as_str() else {
        return if next.is_null() {
            Ok(None)
        } else {
            Err("SoundCloud returned an invalid library continuation".into())
        };
    };
    if next.is_empty() {
        return Ok(None);
    }
    let next = Url::parse(next)
        .map_err(|_| "SoundCloud returned an invalid library continuation".to_string())?;
    if next.scheme() != "https"
        || next.host_str() != Some("api-v2.soundcloud.com")
        || next.path() != expected_path
    {
        return Err("SoundCloud returned an unexpected library continuation".into());
    }
    next.query_pairs()
        .find_map(|(key, value)| (key == "offset" && !value.is_empty()).then(|| value.into_owned()))
        .map(Some)
        .ok_or_else(|| "SoundCloud library continuation did not include an offset".into())
}

fn id(value: Option<&Value>, prefix: &str) -> Option<String> {
    let value = value?
        .as_str()
        .map(str::to_owned)
        .or_else(|| value?.as_u64().map(|value| value.to_string()))?;
    let value = value.strip_prefix(prefix).unwrap_or(&value);
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())).then(|| value.to_owned())
}

fn number(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0)
}

fn artwork(value: &Value) -> String {
    let source = [
        value.get("artwork"),
        value.get("artwork_url"),
        value.get("artworkUrl"),
        value.get("artwork_url_template"),
        value.pointer("/user/avatar_url"),
        value.get("avatar_url"),
    ]
    .into_iter()
    .find_map(|value| value.and_then(Value::as_str))
    .unwrap_or_default()
    .replace("-large", "-t500x500")
    .replace("{size}", "t500x500");
    Url::parse(&source)
        .ok()
        .filter(|url| url.scheme() == "https")
        .map(|url| url.to_string())
        .unwrap_or_default()
}

fn playlist_artwork(value: &Value) -> String {
    [
        value.get("artwork"),
        value.get("artwork_url"),
        value.get("artworkUrl"),
        value.get("artwork_url_template"),
        value.pointer("/user/avatar_url"),
        value.get("avatar_url"),
    ]
    .into_iter()
    .filter_map(|value| value.and_then(Value::as_str))
    .map(|source| {
        source
            .replace("-large", "-t500x500")
            .replace("{size}", "t500x500")
    })
    .find_map(|source| safe_soundcloud_artwork_url(&source).ok())
    .unwrap_or_default()
}

fn track(value: &Value) -> Track {
    let raw = value_string(value.get("title"));
    let raw = if raw.trim().is_empty() {
        "Unknown Track"
    } else {
        &raw
    };
    let split = raw.split_once(" - ");
    let artist = value_string(value.pointer("/publisher_metadata/artist"));
    let artist = if !artist.trim().is_empty() {
        artist
    } else {
        split
            .map(|(artist, _)| artist.trim().to_owned())
            .unwrap_or_else(|| value_string(value.pointer("/user/username")))
    };
    let title = split
        .map(|(_, title)| title.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or(raw);
    let duration = number(value.get("duration")).max(number(value.get("full_duration")));
    let artist = if artist.is_empty() {
        "Unknown Artist".to_owned()
    } else {
        artist
    };
    let user_id = value_string(value.pointer("/user/id"));
    let username = value_string(value.pointer("/user/username"));
    let full_name = value_string(value.pointer("/user/full_name"));
    let uploader_name = if !username.trim().is_empty() {
        Some(username)
    } else if !full_name.trim().is_empty() {
        Some(full_name)
    } else {
        None
    };
    let artists = if user_id.trim().is_empty() || uploader_name.is_none() {
        Vec::new()
    } else {
        vec![crate::search::TrackArtistRef {
            id: user_id,
            name: uploader_name.unwrap_or_default(),
        }]
    };
    Track {
        origin: None,
        id: id(value.get("id"), "")
            .or_else(|| id(value.get("urn"), "soundcloud:tracks:"))
            .unwrap_or_default(),
        title: title.to_owned(),
        artist,
        artists,
        album: [
            value_string(value.pointer("/publisher_metadata/album_name")),
            value_string(value.pointer("/publisherMetadata/albumName")),
            value_string(value.get("album_name")),
            value_string(value.get("album")),
        ]
        .into_iter()
        .find(|album| !album.trim().is_empty())
        .unwrap_or_default(),
        album_id: String::new(),
        release_date: crate::search::release_date(value),
        duration: duration / 1000,
        artwork: artwork(value),
        explicit: value.get("explicit").and_then(Value::as_bool) == Some(true)
            || value
                .pointer("/publisher_metadata/explicit")
                .and_then(Value::as_bool)
                == Some(true)
            || title.to_ascii_lowercase().contains("explicit"),
        service_url: crate::search::soundcloud_service_url(value),
    }
}

fn artist_card(value: &Value) -> Card {
    Card {
        kind: Category::Artists,
        id: id(value.get("id"), "")
            .or_else(|| id(value.get("urn"), "soundcloud:users:"))
            .unwrap_or_default(),
        title: {
            let title = value_string(value.get("username"));
            if title.is_empty() {
                value_string(value.get("full_name"))
            } else {
                title
            }
        },
        subtitle: crate::search::soundcloud_artist_subtitle(value),
        artwork: artwork(value),
        release_date: String::new(),
        badge: String::new(),
        source: Provider::SoundCloud,
        service_url: crate::search::soundcloud_service_url(value),
        is_private: None,
        library_service: None,
    }
}

fn playlist_card(kind: Category, value: &Value) -> Card {
    let title = value_string(value.get("title"));
    let subtitle = [
        value_string(value.pointer("/user/username")),
        value_string(value.pointer("/user/full_name")),
    ]
    .into_iter()
    .find(|value| !value.trim().is_empty())
    .unwrap_or_else(|| "SoundCloud".into());
    let badge = if kind == Category::Albums {
        crate::search::release_date(value)
            .get(..4)
            .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
            .unwrap_or_default()
            .to_owned()
    } else {
        number(value.get("track_count")).to_string()
    };
    Card {
        kind,
        id: id(value.get("id"), "")
            .or_else(|| id(value.get("urn"), "soundcloud:playlists:"))
            .unwrap_or_default(),
        title: if title.trim().is_empty() {
            match kind {
                Category::Albums => "Untitled album",
                Category::Playlists => "Untitled playlist",
                _ => "Untitled collection",
            }
            .into()
        } else {
            title
        },
        subtitle,
        artwork: playlist_artwork(value),
        release_date: if kind == Category::Albums {
            crate::search::release_date(value)
        } else {
            String::new()
        },
        badge,
        source: Provider::SoundCloud,
        service_url: crate::search::soundcloud_service_url(value),
        is_private: (kind == Category::Playlists)
            .then(|| playlist_privacy(value))
            .flatten(),
        library_service: None,
    }
}

fn playlist_privacy(value: &Value) -> Option<bool> {
    if let Some(sharing) = value.get("sharing").and_then(Value::as_str) {
        if sharing.eq_ignore_ascii_case("private") {
            return Some(true);
        }
        if sharing.eq_ignore_ascii_case("public") {
            return Some(false);
        }
    }
    value
        .get("public")
        .and_then(Value::as_bool)
        .map(|public| !public)
}

#[cfg(test)]
fn artist_page(profile: &Value, raw: &[Value]) -> Page {
    let username = value_string(profile.get("username"));
    let full_name = value_string(profile.get("full_name"));
    let title = if !username.trim().is_empty() {
        username.clone()
    } else if !full_name.trim().is_empty() {
        full_name.clone()
    } else {
        "Artist Tracks".into()
    };
    let mut page = Page {
        title,
        subtitle: crate::search::soundcloud_artist_subtitle(profile),
        description: value_string(profile.get("description")),
        artwork: artwork(profile),
        show_count: true,
        count_noun: "track".into(),
        total: raw.len(),
        platform: detail_platform("artistTracks"),
        ..Page::default()
    };
    page.tracks = raw.iter().map(track).collect();
    page
}

fn station_page(title: String, subtitle: String, artwork: String, raw: &[Value]) -> Page {
    Page {
        title: if title.trim().is_empty() {
            "Station".into()
        } else {
            title
        },
        subtitle,
        description: String::new(),
        artwork,
        show_count: false,
        total: raw.len(),
        platform: detail_platform("stationTracks"),
        tracks: raw.iter().map(track).collect(),
        ..Page::default()
    }
}

fn station_card(value: &Value) -> Card {
    let track = track(value);
    Card {
        kind: Category::Station,
        id: track.id,
        title: track.title,
        subtitle: format!("Start from {}", track.artist),
        artwork: track.artwork,
        release_date: String::new(),
        badge: String::new(),
        source: Provider::SoundCloud,
        service_url: track.service_url,
        is_private: None,
        library_service: None,
    }
}

async fn decode(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!("SoundCloud returned {}", response.status()));
    }
    response
        .json()
        .await
        .map_err(|_| "SoundCloud returned an invalid response".into())
}

#[cfg(test)]
mod tests {
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
    fn soundcloud_owned_playlist_catalog_uses_without_albums_endpoint() {
        let source = include_str!("soundcloud_client.rs");
        let production = source.split_once("#[cfg(test)]\nmod tests").map_or_else(
            || panic!("SoundCloud client tests should follow production"),
            |(code, _)| code,
        );
        assert!(production.contains("/users/{user_id}/playlists_without_albums"));
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
            let key =
                super::super::favorite_state::FavoriteKey::soundcloud(kind, "951165127".into());
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
        assert!(
            library_playlists(vec![json!({"type": "playlist-like"})], Category::Albums).is_err()
        );
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
    fn library_artist_routes_use_the_rich_search_artist_loader() {
        let source = include_str!("soundcloud_client.rs");
        let loader = source
            .split_once("async fn load_artist(")
            .and_then(|(_, rest)| rest.split_once("async fn collection("))
            .map_or_else(
                || panic!("artist loader should be present"),
                |(body, _)| body,
            );
        assert!(loader.contains("search_client"));
        assert!(loader.contains("ResultType::Artists"));
        assert!(loader.contains("super::client::detail_page(detail)"));
        assert!(!loader.contains("artist_page(&profile, &raw)"));
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
    fn playlist_writes_use_the_cookie_free_mobile_contract() {
        let source = include_str!("soundcloud_client.rs");
        let production = source.split_once("#[cfg(test)]\nmod tests").map_or_else(
            || panic!("SoundCloud client tests should follow production"),
            |(code, _)| code,
        );
        assert!(production.contains("send_mobile_json("));
        let client = SoundCloudLibraryClient::new().unwrap();
        let authorization = SoundCloudToken::from_saved("mobile-sentinel")
            .unwrap()
            .authorization_header()
            .unwrap();
        let request = client
            .mobile_request(Method::POST, format!("{API}/playlists"), authorization)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&json!({"playlist":{"title":"test","tracks":[]}}))
            .build()
            .unwrap();
        assert_eq!(request.headers()[header::USER_AGENT], MOBILE_USER_AGENT);
        assert_eq!(request.headers()[header::ACCEPT], "*/*");
        assert_eq!(
            request.headers()[header::ACCEPT_ENCODING],
            PLAYLIST_ACCEPT_ENCODING
        );
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "OAuth mobile-sentinel"
        );
        assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
        assert!(!request.headers().contains_key(header::COOKIE));
        assert!(request.url().query().is_none());
        assert!(!production.contains("seed_mobile_cookies(session_cookies)"));
        let create = production
            .split("pub(crate) async fn create_playlist(")
            .nth(1)
            .and_then(|rest| {
                rest.split("pub(crate) async fn add_tracks_to_playlist(")
                    .next()
            })
            .expect("create_playlist body");
        assert!(create.contains("send_mobile_json("));
        assert!(!create.contains("CLIENT_ID"));
        assert!(!create.contains(".query(&[(\"client_id\""));
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
        assert!(
            update_playlist_write("9004", "title", &format!("{description}🎵"), false,).is_err()
        );
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
}
