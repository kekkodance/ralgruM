use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, Response, header};
use serde_json::{Value, json};

use crate::search::DeezerArl;

use super::client::DEEZER_SESSION_EXPIRED;
use super::playlist_limits::{DEEZER_DESCRIPTION_MAX_CHARS, DEEZER_TITLE_MAX_CHARS};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const USER_DATA_URL: &str = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
const JWT_URL: &str = "https://auth.deezer.com/login/arl?jo=p&rto=c&i=p";
const GRAPHQL_URL: &str = "https://pipe.deezer.com/api";
const PLAYLIST_FRAGMENT: &str = r#"fragment PlaylistInfo on Playlist {
 id title description isPrivate isFromFavoriteTracks isCollaborative estimatedTracksCount owner { id name __typename }
  picture { id small: urls(pictureRequest: {height: 100, width: 100}) medium: urls(pictureRequest: {width: 264, height: 264}) large: urls(pictureRequest: {width: 500, height: 500}) __typename }
  __typename
}"#;
const PLAYLIST_GATEWAY_URL: &str = "https://www.deezer.com/ajax/gw-light.php";
const PLAYLIST_PICTURE_URL: &str = "https://upload.deezer.com/v2/playlist/picture";
const MAX_PICTURE_BYTES: usize = 10 * 1024 * 1024;
static CORRELATION_COUNTER: AtomicU64 = AtomicU64::new(0);
const SIDEBAR_QUERY: &str = r#"query SidebarPlaylistsInfo($first: Int!) {
 me { id playlists(first: $first, sort: {by: LAST_MODIFICATION_DATE, order: DESC}) { edges { node { ...PlaylistInfo } } } userFavorites { playlists(first: $first) { edges { node { ...PlaylistInfo } } } } }
}"#;

const UPDATE_MUTATION: &str = r#"mutation UpdatePlaylist($input: PlaylistUpdateMutationInput!) {
  updatePlaylist(input: $input) { playlist { ...PlaylistInfo } }
}
"#;

const ADD_TRACKS_MUTATION: &str = r#"mutation AddTracksToPlaylist($input: PlaylistAddTracksMutationInput!) {
  addTracksToPlaylist(input: $input) {
    ... on PlaylistAddTracksOutput {
      addedTrackIds
      duplicatedTrackIds
      __typename
    }
    __typename
  }
}"#;
const CREATE_MUTATION: &str = r#"mutation CreatePlaylist($input: PlaylistCreateMutationInput!) {
 createPlaylist(input: $input) { playlist { id } }
}"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlaylistOwner {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnedPlaylist {
    pub id: String,
    pub title: String,
    pub description: String,
    pub is_private: bool,
    pub is_from_favorite_tracks: bool,
    pub is_collaborative: bool,
    pub owner: PlaylistOwner,
    pub artwork: String,
    pub track_count: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AddTrackResult {
    Added,
    AlreadyPresent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AddTracksResult {
    Added { count: usize },
    AlreadyPresent { count: usize },
    Partial { added: usize, duplicated: usize },
}

impl AddTracksResult {
    pub(crate) fn added_count(self) -> usize {
        match self {
            Self::Added { count } => count,
            Self::AlreadyPresent { .. } => 0,
            Self::Partial { added, .. } => added,
        }
    }

    pub(crate) fn status_message(self) -> String {
        match self {
            Self::Added { count: 1 } => "Track added to playlist.".into(),
            Self::Added { count } => format!("{count} tracks added to playlist."),
            Self::AlreadyPresent { count: 1 } => "Track was already in that playlist.".into(),
            Self::AlreadyPresent { .. } => "Those tracks were already in that playlist.".into(),
            Self::Partial { added, duplicated } => {
                format!("Added {added} tracks; {duplicated} were already in the playlist.")
            }
        }
    }

    fn as_single(self) -> Result<AddTrackResult, String> {
        match self {
            Self::Added { count: 1 } => Ok(AddTrackResult::Added),
            Self::AlreadyPresent { count: 1 } => Ok(AddTrackResult::AlreadyPresent),
            _ => Err("Deezer AddTracksToPlaylist returned an unaccounted track result".into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CreatedPlaylist {
    pub id: String,
}

impl OwnedPlaylist {
    pub(crate) fn editable(&self) -> bool {
        !self.is_from_favorite_tracks && !self.is_collaborative
    }
}

#[derive(Clone)]
pub(crate) struct PlaylistClient {
    pub(super) client: Client,
}

pub(super) struct Session {
    pub(super) cookie: header::HeaderValue,
    pub(super) jwt: String,
    pub(super) api_token: String,
    pub(super) user_id: String,
}

impl PlaylistClient {
    pub(crate) fn new() -> Result<Self, String> {
        Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .user_agent(USER_AGENT)
            .build()
            .map(|client| Self { client })
            .map_err(|_| "Deezer playlist client could not be created".into())
    }

    pub(crate) async fn catalog(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<Vec<OwnedPlaylist>, String> {
        let session = self.session(arl, saved_user_id).await?;
        let data = self
            .graphql(
                &session,
                "SidebarPlaylistsInfo",
                json!({ "first": 50 }),
                format!("{SIDEBAR_QUERY}{PLAYLIST_FRAGMENT}"),
            )
            .await?;
        parse_catalog(&data)
    }

    pub(crate) async fn create(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        title: &str,
        description: &str,
        is_private: bool,
        picture_base64: &str,
    ) -> Result<CreatedPlaylist, String> {
        let title = valid_text(title, false, DEEZER_TITLE_MAX_CHARS)?;
        let description = valid_text(description, true, DEEZER_DESCRIPTION_MAX_CHARS)?;
        let session = self.session(arl, saved_user_id).await?;
        let token = self.upload_picture(&session, picture_base64).await?;
        let data = self
            .graphql(
                &session,
                "CreatePlaylist",
                json!({"input": {
                    "title": title, "description": description, "isPrivate": is_private,
                    "isCollaborative": false, "picture": token
                }}),
                CREATE_MUTATION.into(),
            )
            .await?;
        parse_created_playlist(&data)
    }

    async fn upload_picture(
        &self,
        session: &Session,
        picture_base64: &str,
    ) -> Result<String, String> {
        let picture = STANDARD
            .decode(picture_base64.trim())
            .map_err(|_| "Playlist cover data is invalid".to_string())?;
        if picture.is_empty() || picture.len() > MAX_PICTURE_BYTES {
            return Err("Playlist cover must be between 1 byte and 10 MB".into());
        }
        let boundary = format!("----ralgrum{}", next_correlation_id());
        let mut body = Vec::with_capacity(picture.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"template.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").as_bytes());
        body.extend_from_slice(&picture);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {}", session.jwt))
            .map_err(|_| "The Deezer JWT login returned an invalid token")?;
        authorization.set_sensitive(true);
        let response = self
            .client
            .post(PLAYLIST_PICTURE_URL)
            .header(header::AUTHORIZATION, authorization)
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header(header::ORIGIN, "https://www.deezer.com")
            .header(header::REFERER, "https://www.deezer.com/")
            .body(body)
            .send()
            .await
            .map_err(|_| "Deezer playlist picture upload failed".to_string())?;
        let envelope = decode(response).await?;
        envelope
            .get("results")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "Deezer playlist picture upload returned no picture token".to_string())
    }

    pub(crate) async fn update(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
        title: &str,
        description: &str,
        is_private: bool,
        picture_base64: Option<&str>,
    ) -> Result<OwnedPlaylist, String> {
        let playlist_id = valid_id(playlist_id)?;
        let title = valid_text(title, false, DEEZER_TITLE_MAX_CHARS)?;
        let description = valid_text(description, true, DEEZER_DESCRIPTION_MAX_CHARS)?;
        let session = self.session(arl, saved_user_id).await?;
        let catalog = self
            .graphql(
                &session,
                "SidebarPlaylistsInfo",
                json!({ "first": 50 }),
                format!("{SIDEBAR_QUERY}{PLAYLIST_FRAGMENT}"),
            )
            .await?;
        let owned = parse_catalog(&catalog)?
            .into_iter()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Deezer did not identify this as one of your playlists".to_string())?;
        if !owned.editable() {
            return Err("This Deezer playlist cannot be edited".into());
        }
        let picture = match picture_base64 {
            Some(picture) => Some(self.upload_picture(&session, picture).await?),
            None => None,
        };
        let data = self
            .graphql(
                &session,
                "UpdatePlaylist",
                update_variables(
                    &playlist_id,
                    &title,
                    &description,
                    is_private,
                    picture.as_deref(),
                ),
                format!("{UPDATE_MUTATION}{PLAYLIST_FRAGMENT}"),
            )
            .await?;
        let playlist = data
            .pointer("/updatePlaylist/playlist")
            .ok_or_else(|| "Deezer UpdatePlaylist response is missing the playlist".to_string())?;
        let playlist = parse_playlist(playlist)?;
        if playlist.id != playlist_id {
            return Err("Deezer UpdatePlaylist returned a different playlist".into());
        }
        Ok(playlist)
    }

    #[allow(dead_code)]
    pub(crate) async fn add_track(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
        track_id: &str,
    ) -> Result<AddTrackResult, String> {
        let track_ids = [track_id.to_owned()];
        self.add_tracks(arl, saved_user_id, playlist_id, &track_ids)
            .await
            .and_then(AddTracksResult::as_single)
    }

    pub(crate) async fn add_tracks(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
        track_ids: &[String],
    ) -> Result<AddTracksResult, String> {
        if track_ids.is_empty() {
            return Err("No tracks were provided to add.".into());
        }
        let playlist_id = valid_id(playlist_id)?;
        let mut valid_ids = Vec::with_capacity(track_ids.len());
        let mut seen = HashSet::new();
        for id in track_ids {
            let Ok(id) = valid_id(id) else {
                continue;
            };
            if seen.insert(id.clone()) {
                valid_ids.push(id);
            }
        }
        if valid_ids.is_empty() {
            return Err("Deezer ids must contain only decimal digits".into());
        }
        let session = self.session(arl, saved_user_id).await?;
        let catalog = self
            .graphql(
                &session,
                "SidebarPlaylistsInfo",
                json!({ "first": 50 }),
                format!("{SIDEBAR_QUERY}{PLAYLIST_FRAGMENT}"),
            )
            .await?;
        let playlist = parse_catalog(&catalog)?
            .into_iter()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Deezer did not identify this as one of your playlists".to_string())?;
        if !playlist.editable() {
            return Err("This Deezer playlist cannot be edited".into());
        }
        let data = self
            .graphql(
                &session,
                "AddTracksToPlaylist",
                add_tracks_variables(&playlist_id, &valid_ids),
                ADD_TRACKS_MUTATION.into(),
            )
            .await?;
        parse_add_tracks_result(&data, &valid_ids)
    }

    pub(crate) async fn reorder(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
        order: &[String],
    ) -> Result<bool, String> {
        let playlist_id = valid_id(playlist_id)?;
        validate_order(order)?;
        let session = self.session(arl, saved_user_id).await?;
        let catalog = self
            .graphql(
                &session,
                "SidebarPlaylistsInfo",
                json!({ "first": 50 }),
                format!("{SIDEBAR_QUERY}{PLAYLIST_FRAGMENT}"),
            )
            .await?;
        let playlist = parse_catalog(&catalog)?
            .into_iter()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Deezer did not identify this as one of your playlists".to_string())?;
        if !playlist.editable() {
            return Err("This Deezer playlist cannot be edited".into());
        }
        let songs = self
            .gateway(
                &session,
                "playlist.getSongs",
                json!({ "playlist_id": playlist_id, "nb": 2000 }),
            )
            .await?;
        let tracks = songs
            .pointer("/data")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "Deezer playlist.getSongs response is missing its item list".to_string()
            })?;
        let total =
            songs.get("total").and_then(Value::as_u64).ok_or_else(|| {
                "Deezer playlist.getSongs response is missing its total".to_string()
            })? as usize;
        let loaded = tracks
            .iter()
            .map(|track| {
                value_string(track.get("SNG_ID")).ok_or_else(|| {
                    "Deezer playlist.getSongs returned a track without an id".to_string()
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if total != tracks.len() || total != order.len() {
            return Err(
                "The complete Deezer playlist must be loaded before its order can be changed"
                    .into(),
            );
        }
        let loaded_set: HashSet<&str> = loaded.iter().map(String::as_str).collect();
        if loaded_set.len() != loaded.len() {
            return Err("Reordering playlists with duplicate track ids was not captured".into());
        }
        let submitted_set: HashSet<&str> = order.iter().map(String::as_str).collect();
        if loaded_set != submitted_set {
            return Err("The Deezer playlist changed before its new order was saved".into());
        }
        let result = self
            .gateway(
                &session,
                "playlist.updateOrder",
                reorder_body(&playlist_id, order),
            )
            .await?;
        if result.as_bool() != Some(true) {
            return Err("Deezer playlist.updateOrder did not confirm success".into());
        }
        Ok(true)
    }

    /* removal transport lives in playlist_remove_client */
    pub(crate) async fn remove_track(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
        track_id: &str,
    ) -> Result<bool, String> {
        super::playlist_remove_client::remove_track(self, arl, saved_user_id, playlist_id, track_id)
            .await
    }

    pub(super) async fn session(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
    ) -> Result<Session, String> {
        let arl_cookie = arl.cookie_header().map_err(|error| error.message)?;
        let response = session_bootstrap_request(&self.client, arl_cookie.clone())
            .send()
            .await
            .map_err(|_| "Deezer login session could not be verified".to_string())?;
        let cookies = response_cookies(&response);
        let value = decode(response).await?;
        let results = envelope_results(&value)?;
        // getUserData answering an anonymous session (USER_ID "0") means the
        // saved ARL is no longer authenticated, so the saved user id must
        // not be substituted for the missing bootstrap id.
        let user_id = value_string(results.pointer("/USER/USER_ID"))
            .filter(|id| id != "0")
            .ok_or_else(|| DEEZER_SESSION_EXPIRED.to_string())?;
        if let Some(saved) = saved_user_id.as_deref().and_then(|id| valid_id(id).ok())
            && saved != user_id
        {
            return Err("Deezer account changed while the library was loading".into());
        }
        let cookie = session_cookie(arl_cookie, &cookies)?;
        let request = jwt_request(&self.client, &arl, &user_id);
        let response = request
            .send()
            .await
            .map_err(|_| "The Deezer JWT login request could not be completed".to_string())?;
        let value = decode(response).await?;
        let jwt = value
            .get("jwt")
            .and_then(Value::as_str)
            .filter(|jwt| valid_jwt(jwt))
            .ok_or_else(|| "Deezer JWT login returned an invalid token".to_string())?
            .to_owned();
        let api_token = value_string(results.get("checkForm"))
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "Deezer session bootstrap is missing its API token".to_string())?;
        Ok(Session {
            cookie,
            jwt,
            api_token,
            user_id,
        })
    }

    pub(super) async fn graphql(
        &self,
        session: &Session,
        operation: &str,
        variables: Value,
        query: String,
    ) -> Result<Value, String> {
        let response = graphql_request(&self.client, session, operation, variables, query)?
            .send()
            .await
            .map_err(|_| format!("Deezer {operation} request failed"))?;
        let value = decode(response).await?;
        if let Some(message) = value
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|errors| errors.first())
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            return Err(Self::graphql_error_message(operation, message));
        }
        value
            .get("data")
            .cloned()
            .ok_or_else(|| format!("Deezer {operation} response is missing data"))
    }

    fn graphql_error_message(operation: &str, provider_message: &str) -> String {
        if operation == "CreatePlaylist" {
            let message = provider_message.to_ascii_lowercase();
            if message.contains("description")
                || message.contains("invalid input")
                || message.contains("validation")
                || message.contains("playlist creation failed")
            {
                return "Deezer rejected the playlist description. Shorten it and try again."
                    .into();
            }
        }
        format!("Deezer {operation} failed ({provider_message})")
    }

    async fn gateway(
        &self,
        session: &Session,
        operation: &str,
        body: Value,
    ) -> Result<Value, String> {
        let mut cookie = session.cookie.clone();
        cookie.set_sensitive(true);
        let mut url = reqwest::Url::parse(PLAYLIST_GATEWAY_URL)
            .map_err(|_| "Invalid Deezer gateway URL".to_string())?;
        url.query_pairs_mut()
            .append_pair("method", operation)
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", &session.api_token)
            .append_pair("cid", &next_correlation_id());
        let referer = format!(
            "https://www.deezer.com/en/playlist/{}",
            body.get("playlist_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
        let response = self
            .client
            .post(url)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
            .header(header::ORIGIN, "https://www.deezer.com")
            .header(header::REFERER, referer)
            .header("x-deezer-user", &session.user_id)
            .body(body.to_string())
            .send()
            .await
            .map_err(|_| format!("Deezer {operation} request failed"))?;
        let value = decode(response).await?;
        envelope_results(&value).map(|value| value.to_owned())
    }
}

fn next_correlation_id() -> String {
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or_default();
    let sequence = CORRELATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    (100_000_000 + (clock.wrapping_add(sequence) % 900_000_000)).to_string()
}

fn reorder_body(playlist_id: &str, order: &[String]) -> Value {
    json!({ "playlist_id": playlist_id, "position": "0", "order": order })
}

fn validate_order(order: &[String]) -> Result<(), String> {
    if order.is_empty() || order.len() > 10000 {
        return Err("Playlist order must contain between 1 and 10000 tracks".into());
    }
    let mut ids = HashSet::new();
    for id in order {
        let id = valid_id(id)?;
        if !ids.insert(id) {
            return Err("Reordering playlists with duplicate track ids was not captured".into());
        }
    }
    Ok(())
}

fn graphql_request(
    client: &Client,
    session: &Session,
    operation: &str,
    variables: Value,
    query: String,
) -> Result<reqwest::RequestBuilder, String> {
    let mut authorization = header::HeaderValue::from_str(&format!("Bearer {}", session.jwt))
        .map_err(|_| "The Deezer JWT login returned an invalid token")?;
    authorization.set_sensitive(true);
    Ok(client
        .post(GRAPHQL_URL)
        .header(header::COOKIE, session.cookie.clone())
        .header(header::AUTHORIZATION, authorization)
        .header(header::ORIGIN, "https://www.deezer.com")
        .header(header::REFERER, "https://www.deezer.com/")
        .json(&json!({ "operationName": operation, "variables": variables, "query": query })))
}

fn jwt_request(client: &Client, arl: &DeezerArl, user_id: &str) -> reqwest::RequestBuilder {
    client
        .post(JWT_URL)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "https://www.deezer.com")
        .header(header::REFERER, "https://www.deezer.com/")
        .json(&json!({ "arl": arl.expose(), "account_id": user_id }))
}

fn session_bootstrap_request(
    client: &Client,
    arl_cookie: header::HeaderValue,
) -> reqwest::RequestBuilder {
    client
        .post(USER_DATA_URL)
        .header(header::COOKIE, arl_cookie)
        .header(header::CONTENT_LENGTH, "0")
        .body("")
}

fn update_variables(
    id: &str,
    title: &str,
    description: &str,
    is_private: bool,
    picture: Option<&str>,
) -> Value {
    let mut input = json!({
        "playlistId": id,
        "title": title.trim(),
        "description": description.trim(),
        "isPrivate": is_private,
        "isCollaborative": false
    });
    if let Some(picture) = picture {
        input["picture"] = Value::String(picture.to_owned());
    }
    json!({ "input": input })
}

fn add_tracks_variables(playlist_id: &str, track_ids: &[String]) -> Value {
    json!({ "input": { "playlistId": playlist_id, "trackIds": track_ids } })
}

pub(super) fn parse_catalog(data: &Value) -> Result<Vec<OwnedPlaylist>, String> {
    let edges = data
        .pointer("/me/playlists/edges")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Deezer SidebarPlaylistsInfo response is missing owned playlists".to_string()
        })?;
    edges
        .iter()
        .map(|edge| {
            edge.get("node")
                .ok_or_else(|| "Deezer owned playlist entry is missing metadata".to_string())
                .and_then(parse_playlist)
        })
        .collect()
}

fn parse_add_tracks_result(
    data: &Value,
    requested_ids: &[String],
) -> Result<AddTracksResult, String> {
    if requested_ids.is_empty() {
        return Err("No tracks were provided to add.".into());
    }
    let result = data
        .pointer("/addTracksToPlaylist")
        .filter(|value| !value.is_null())
        .ok_or_else(|| "Deezer AddTracksToPlaylist response is missing its result".to_string())?;
    if result.get("__typename").and_then(Value::as_str) != Some("PlaylistAddTracksOutput") {
        return Err("Deezer AddTracksToPlaylist did not return the captured success result".into());
    }
    let added = result
        .get("addedTrackIds")
        .and_then(Value::as_array)
        .ok_or_else(|| "Deezer AddTracksToPlaylist did not return added track ids".to_string())?;
    let duplicated = result
        .get("duplicatedTrackIds")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            "Deezer AddTracksToPlaylist did not return duplicated track ids".to_string()
        })?;
    let added_ids: HashSet<String> = added
        .iter()
        .filter_map(|item| value_string(Some(item)))
        .collect();
    let duplicated_ids: HashSet<String> = duplicated
        .iter()
        .filter_map(|item| value_string(Some(item)))
        .collect();
    let mut added_count = 0usize;
    let mut duplicated_count = 0usize;
    for id in requested_ids {
        match (added_ids.contains(id), duplicated_ids.contains(id)) {
            (true, false) => added_count += 1,
            (false, true) => duplicated_count += 1,
            _ => {
                return Err(
                    "Deezer AddTracksToPlaylist returned an unaccounted track result".into(),
                );
            }
        }
    }
    match (added_count, duplicated_count) {
        (count, 0) => Ok(AddTracksResult::Added { count }),
        (0, count) => Ok(AddTracksResult::AlreadyPresent { count }),
        (added, duplicated) => Ok(AddTracksResult::Partial { added, duplicated }),
    }
}

pub(super) fn parse_created_playlist(data: &Value) -> Result<CreatedPlaylist, String> {
    let id = value_string(data.pointer("/createPlaylist/playlist/id"))
        .ok_or_else(|| "Deezer CreatePlaylist response is missing its playlist ID".to_string())?;
    Ok(CreatedPlaylist { id: valid_id(&id)? })
}

fn parse_playlist(value: &Value) -> Result<OwnedPlaylist, String> {
    let boolean = |name| {
        value
            .get(name)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("Deezer did not describe playlist {name}"))
    };
    let owner = value
        .get("owner")
        .ok_or_else(|| "Deezer did not describe the playlist owner".to_string())?;
    let id = valid_id(&value_string(value.get("id")).unwrap_or_default())?;
    Ok(OwnedPlaylist {
        artwork: crate::integrations::deezer::playlist_image_url(&id),
        id,
        title: value_string(value.get("title")).unwrap_or_default(),
        description: value_string(value.get("description")).unwrap_or_default(),
        is_private: boolean("isPrivate")?,
        is_from_favorite_tracks: boolean("isFromFavoriteTracks")?,
        is_collaborative: boolean("isCollaborative")?,
        owner: PlaylistOwner {
            id: valid_id(&value_string(owner.get("id")).unwrap_or_default())?,
            name: value_string(owner.get("name")).unwrap_or_default(),
        },
        track_count: parse_playlist_track_count(value),
    })
}

pub(super) fn valid_id(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 32 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Deezer ids must contain only decimal digits".into());
    }
    Ok(value.to_owned())
}

fn parse_playlist_track_count(value: &Value) -> Option<u64> {
    for key in [
        "estimatedTracksCount",
        "trackCount",
        "nbTracks",
        "NB_SONG",
        "nb_song",
        "track_count",
        "totalTracks",
        "nb_tracks",
    ] {
        if let Some(count) = value.get(key).and_then(Value::as_u64) {
            return Some(count);
        }
        if let Some(text) = value.get(key).and_then(Value::as_str)
            && let Ok(count) = text.trim().parse::<u64>()
        {
            return Some(count);
        }
        if let Some(count) = value
            .get(key)
            .and_then(Value::as_i64)
            .and_then(|v| u64::try_from(v).ok())
        {
            return Some(count);
        }
    }
    None
}

fn valid_text(value: &str, empty: bool, limit: usize) -> Result<String, String> {
    let value = value.trim();
    if (!empty && value.is_empty())
        || value.chars().count() > limit
        || value.chars().any(|character| {
            character.is_control() && (!empty || !matches!(character, '\n' | '\r' | '\t'))
        })
    {
        return Err(if empty {
            "Playlist description is invalid"
        } else {
            "Enter a playlist title."
        }
        .into());
    }
    Ok(value.to_owned())
}

fn valid_jwt(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.split('.').count() == 3
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn value_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn envelope_results(value: &Value) -> Result<&Value, String> {
    if value.get("error").is_some_and(deezer_envelope_has_error) {
        return Err("Deezer session bootstrap failed".into());
    }
    value
        .get("results")
        .ok_or_else(|| "Deezer session bootstrap is missing results".into())
}

fn deezer_envelope_has_error(error: &Value) -> bool {
    match error {
        Value::Null | Value::Bool(false) => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
        Value::String(message) => !message.is_empty(),
        Value::Number(number) => number.as_i64() != Some(0),
        _ => true,
    }
}

async fn decode(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Deezer returned HTTP status {}",
            response.status().as_u16()
        ));
    }
    response
        .json()
        .await
        .map_err(|_| "Deezer returned an invalid response".into())
}

fn response_cookies(response: &Response) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

fn session_cookie(arl: header::HeaderValue, cookies: &str) -> Result<header::HeaderValue, String> {
    let mut value = arl
        .to_str()
        .map_err(|_| "The saved Deezer session is invalid")?
        .to_owned();
    if !cookies.is_empty() {
        value.push_str("; ");
        value.push_str(cookies);
    }
    let mut value =
        header::HeaderValue::from_str(&value).map_err(|_| "Deezer returned an invalid session")?;
    value.set_sensitive(true);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_bootstrap_matches_the_captured_empty_post_contract() {
        let mut cookie = header::HeaderValue::from_static("arl=session-secret");
        cookie.set_sensitive(true);
        let request = session_bootstrap_request(&Client::new(), cookie)
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), USER_DATA_URL);
        assert_eq!(request.headers()[header::CONTENT_LENGTH], "0");
        assert!(request.headers()[header::COOKIE].is_sensitive());
        assert!(!request.headers().contains_key(header::CONTENT_TYPE));
        assert_eq!(
            request.body().and_then(|body| body.as_bytes()),
            Some(&b""[..])
        );
        assert!(!format!("{request:?}").contains("session-secret"));
    }

    #[test]
    fn playlist_creation_validation_error_has_actionable_copy() {
        assert_eq!(
            PlaylistClient::graphql_error_message("CreatePlaylist", "Invalid input"),
            "Deezer rejected the playlist description. Shorten it and try again."
        );
        assert_eq!(
            PlaylistClient::graphql_error_message(
                "CreatePlaylist",
                "Description validation failed"
            ),
            "Deezer rejected the playlist description. Shorten it and try again."
        );
        assert_eq!(
            PlaylistClient::graphql_error_message("CreatePlaylist", "Playlist creation failed"),
            "Deezer rejected the playlist description. Shorten it and try again."
        );
        assert_eq!(
            PlaylistClient::graphql_error_message("UpdatePlaylist", "Invalid input"),
            "Deezer UpdatePlaylist failed (Invalid input)"
        );
    }

    #[test]
    fn session_bootstrap_rejects_provider_errors_without_echoing_them() {
        let response = json!({"error":{"message":"credential-shaped-provider-detail"}});
        let error = envelope_results(&response).unwrap_err();
        assert_eq!(error, "Deezer session bootstrap failed");
        assert!(!error.contains("credential-shaped-provider-detail"));
    }

    #[test]
    fn session_bootstrap_accepts_the_captured_empty_error_array() {
        let response = json!({"error":[],"results":{"checkForm":"token"}});
        let results = envelope_results(&response).unwrap();
        assert_eq!(results["checkForm"], "token");
        assert!(!deezer_envelope_has_error(&Value::Null));
        assert!(!deezer_envelope_has_error(&json!({})));
        assert!(!deezer_envelope_has_error(&json!(false)));
        assert!(deezer_envelope_has_error(&json!([{"code": 1}])));
    }

    #[test]
    fn jwt_request_matches_contract_without_exposing_secret_in_debug() {
        let arl = DeezerArl::from_saved("sentinel-secret").unwrap();
        let request = jwt_request(&Client::new(), &arl, "42").build().unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), JWT_URL);
        assert_eq!(request.headers()[header::ORIGIN], "https://www.deezer.com");
        assert_eq!(
            request.headers()[header::REFERER],
            "https://www.deezer.com/"
        );
        assert!(!format!("{request:?}").contains("sentinel-secret"));
    }

    #[test]
    fn graphql_authorization_and_cookie_headers_are_sensitive() {
        let mut cookie = header::HeaderValue::from_static("arl=cookie-secret");
        cookie.set_sensitive(true);
        let session = Session {
            cookie,
            jwt: "header.token-secret.signature".into(),
            api_token: "api-token-secret".into(),
            user_id: "42".into(),
        };
        let request = graphql_request(
            &Client::new(),
            &session,
            "TestOperation",
            json!({}),
            "query TestOperation { __typename }".into(),
        )
        .unwrap()
        .build()
        .unwrap();
        assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
        assert!(request.headers()[header::COOKIE].is_sensitive());
        let debug = format!("{request:?}");
        assert!(!debug.contains("token-secret"));
        assert!(!debug.contains("cookie-secret"));
    }

    #[test]
    fn ownership_parsing_and_editability_fail_closed() {
        let data = json!({"me":{"playlists":{"edges":[{"node": playlist_json(false, false)}]}}});
        let playlists = parse_catalog(&data).unwrap();
        assert!(playlists[0].editable());
        let mut malformed = playlist_json(false, false);
        malformed.as_object_mut().unwrap().remove("isCollaborative");
        assert!(parse_playlist(&malformed).is_err());
        assert!(
            !parse_playlist(&playlist_json(true, false))
                .unwrap()
                .editable()
        );
        assert!(
            !parse_playlist(&playlist_json(false, true))
                .unwrap()
                .editable()
        );
    }

    #[test]
    fn update_payload_validation_and_authoritative_metadata_are_exact() {
        assert_eq!(
            update_variables("42", " title ", " body ", true, None),
            json!({"input":{"playlistId":"42","title":"title","description":"body","isPrivate":true,"isCollaborative":false}})
        );
        assert_eq!(
            update_variables("42", "title", "body", false, Some("picture-token")),
            json!({"input":{"playlistId":"42","title":"title","description":"body","isPrivate":false,"isCollaborative":false,"picture":"picture-token"}})
        );
        assert!(valid_id("42x").is_err());
        assert!(valid_text("  ", false, 500).is_err());
        assert!(valid_text(&"b".repeat(2000), true, 2000).is_ok());
        assert!(valid_text(&"b".repeat(2001), true, 2000).is_err());
        assert!(valid_text("First line\nSecond line", true, 2000).is_ok());
        assert_eq!(
            parse_playlist(&playlist_json(false, false)).unwrap().id,
            "42"
        );
    }

    #[test]
    fn add_track_result_requires_the_requested_id() {
        assert_eq!(
            add_tracks_variables("7", &["42".into()]),
            json!({"input":{"playlistId":"7","trackIds":["42"]}})
        );
        assert!(ADD_TRACKS_MUTATION.contains("... on PlaylistAddTracksOutput"));
        assert!(!ADD_TRACKS_MUTATION.contains("context"));
        assert_eq!(valid_id("42"), Ok("42".into()));
        assert!(valid_id("42x").is_err());
        let added = json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":["42"],"duplicatedTrackIds":[]}});
        let duplicate = json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":[],"duplicatedTrackIds":[42]}});
        assert_eq!(
            parse_add_tracks_result(&added, &["42".into()]).and_then(AddTracksResult::as_single),
            Ok(AddTrackResult::Added)
        );
        assert_eq!(
            parse_add_tracks_result(&duplicate, &["42".into()])
                .and_then(AddTracksResult::as_single),
            Ok(AddTrackResult::AlreadyPresent)
        );
        assert!(parse_add_tracks_result(&json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":["7"],"duplicatedTrackIds":[]}}), &["42".into()]).is_err());
    }

    #[test]
    fn add_tracks_variables_include_every_requested_id() {
        assert_eq!(
            add_tracks_variables("7", &["42".into(), "9".into()]),
            json!({"input":{"playlistId":"7","trackIds":["42","9"]}})
        );
    }

    #[test]
    fn add_tracks_result_classifies_mixed_added_and_duplicated_ids() {
        let mixed = json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":["42", 7],"duplicatedTrackIds":[9]}});
        assert_eq!(
            parse_add_tracks_result(&mixed, &["42".into(), "7".into(), "9".into()]),
            Ok(AddTracksResult::Partial {
                added: 2,
                duplicated: 1
            })
        );
        let added = json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":["42","7"],"duplicatedTrackIds":[]}});
        assert_eq!(
            parse_add_tracks_result(&added, &["42".into(), "7".into()]),
            Ok(AddTracksResult::Added { count: 2 })
        );
        let duplicated = json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":[],"duplicatedTrackIds":["42", 7]}});
        assert_eq!(
            parse_add_tracks_result(&duplicated, &["42".into(), "7".into()]),
            Ok(AddTracksResult::AlreadyPresent { count: 2 })
        );
        assert!(
            parse_add_tracks_result(
                &json!({"addTracksToPlaylist":{"__typename":"PlaylistAddTracksOutput","addedTrackIds":["42"],"duplicatedTrackIds":[]}}),
                &["42".into(), "7".into()]
            )
            .is_err()
        );
        assert_eq!(AddTracksResult::Added { count: 3 }.added_count(), 3);
        assert_eq!(
            AddTracksResult::Partial {
                added: 2,
                duplicated: 1
            }
            .added_count(),
            2
        );
        assert_eq!(
            AddTracksResult::AlreadyPresent { count: 4 }.added_count(),
            0
        );
    }

    #[test]
    fn reorder_validation_and_payload_match_the_captured_contract() {
        assert_eq!(
            reorder_body("42", &["7".into(), "8".into()]),
            json!({"playlist_id":"42","position":"0","order":["7","8"]})
        );
        assert!(validate_order(&[]).is_err());
        assert!(validate_order(&vec!["1".into(); 10001]).is_err());
        assert!(validate_order(&["7".into(), "x".into()]).is_err());
        assert!(validate_order(&["7".into(), "7".into()]).is_err());
        assert!(validate_order(&["7".into(), "8".into()]).is_ok());
    }

    fn playlist_json(special: bool, collaborative: bool) -> Value {
        json!({"id":"42","title":"Title","description":"Body","isPrivate":true,"isFromFavoriteTracks":special,"isCollaborative":collaborative,"owner":{"id":"7","name":"Owner"},"picture":{"large":["https://example.com/a.jpg"]}})
    }

    #[test]
    fn create_response_parser_requires_only_numeric_playlist_id() {
        assert_eq!(
            parse_created_playlist(&json!({"createPlaylist":{"playlist":{"id":42}}}))
                .unwrap()
                .id,
            "42"
        );
        assert!(
            parse_created_playlist(&json!({"createPlaylist":{"playlist":{"id":"x"}}})).is_err()
        );
        assert!(
            parse_created_playlist(
                &json!({"createPlaylist":{"playlist":{"id":"42","owner":null}}})
            )
            .is_ok()
        );
    }

    #[test]
    fn playlist_track_count_parses_known_keys_and_stays_optional() {
        let base = playlist_json(false, false);
        assert_eq!(parse_playlist(&base).unwrap().track_count, None);
        for payload in [
            json!({"estimatedTracksCount": 34}),
            json!({"trackCount": 34}),
            json!({"NB_SONG": 34}),
            json!({"nb_song": "34"}),
            json!({"track_count": 34}),
        ] {
            let mut value = base.clone();
            for (key, count) in payload.as_object().unwrap() {
                value
                    .as_object_mut()
                    .unwrap()
                    .insert(key.clone(), count.clone());
            }
            assert_eq!(
                parse_playlist(&value).unwrap().track_count,
                Some(34),
                "payload {payload}"
            );
        }
    }
}
