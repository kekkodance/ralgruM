use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use gpui::{
    AnimationExt as _, Context, FontWeight, IntoElement, KeyDownEvent, Render, ScrollHandle,
    Window, div, prelude::*, px, rgb,
};
use gpui_component::{
    WindowExt,
    scroll::{Scrollbar, ScrollbarShow},
};
use reqwest::header;
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::{
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    music_ui::ghost_close_button_with_icon_size,
    search::{DeezerArl, Provider, SOUNDCLOUD_CLIENT_ID, SoundCloudToken, format_release_date},
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED},
};

const INFO_DIALOG_WIDTH: f32 = 440.;
const INFO_DIALOG_BODY_PADDING: f32 = 18.;
const INFO_DIALOG_LABEL_WIDTH: f32 = 96.;
const FETCH_TIMEOUT_SECS: u64 = 10;
const TRACK_INFO_CACHE_LIMIT: usize = 32;
/// Outer dialog chrome: viewport margin on each side, matching the
/// playlist dialogs.
const INFO_DIALOG_VIEWPORT_MARGIN: f32 = 32.;
/// Close button is 28px with 14px vertical padding and a 1px bottom border.
const INFO_DIALOG_HEADER_HEIGHT: f32 = 57.;
/// 1px border on the top and bottom of the inner dialog frame.
const INFO_DIALOG_BORDER: f32 = 2.;
/// Approximate tag row height (18px line plus the 7px row gap), used to
/// size the frame. The tag list uses a definite height so extra wrap
/// still scrolls instead of growing the dialog off screen.
const INFO_DIALOG_ROW_HEIGHT: f32 = 26.;
const INFO_DIALOG_ROW_LINE_HEIGHT: f32 = 18.;

fn track_info_chrome_height() -> f32 {
    INFO_DIALOG_HEADER_HEIGHT + INFO_DIALOG_BODY_PADDING * 2. + INFO_DIALOG_BORDER
}

pub(crate) fn track_info_dialog_max_height(viewport_height: f32) -> f32 {
    (viewport_height - INFO_DIALOG_VIEWPORT_MARGIN).max(0.)
}

/// Content-sized height, capped so short windows keep the chrome on screen.
pub(crate) fn track_info_dialog_height(viewport_height: f32, row_count: usize) -> f32 {
    let content_height =
        track_info_chrome_height() + row_count.max(1) as f32 * INFO_DIALOG_ROW_HEIGHT;
    content_height.min(track_info_dialog_max_height(viewport_height))
}

/// Centers the dialog on its current content. Right-click preload means
/// the dialog usually opens fully populated and never moves; the seeded
/// fallback settles once when the fetch lands, together with the rows
/// entrance animation.
pub(crate) fn track_info_margin_top(viewport_height: f32, row_count: usize) -> f32 {
    ((viewport_height - track_info_dialog_height(viewport_height, row_count)) / 2.).max(0.)
}

/// Definite tag-list viewport for the current dialog frame. GPUI only
/// scrolls reliably when this is an explicit height, not a max.
pub(crate) fn track_info_scroll_height(viewport_height: f32, row_count: usize) -> f32 {
    (track_info_dialog_height(viewport_height, row_count) - track_info_chrome_height()).max(0.)
}

/// Preloaded tag sets keyed by track. The authenticated flag splits entries
/// because only a logged-in fetch resolves the pageTrack rows (composer,
/// copyright).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TrackInfoCacheKey {
    provider: Provider,
    track_id: String,
    authenticated: bool,
}

static TRACK_INFO_CACHE: LazyLock<Mutex<HashMap<TrackInfoCacheKey, TrackInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TRACK_INFO_IN_FLIGHT: LazyLock<Mutex<HashSet<TrackInfoCacheKey>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn track_info_cache_key(
    provider: Provider,
    track_id: &str,
    deezer_arl: &Option<DeezerArl>,
    soundcloud_token: &Option<SoundCloudToken>,
) -> TrackInfoCacheKey {
    TrackInfoCacheKey {
        provider,
        track_id: track_id.to_owned(),
        authenticated: match provider {
            Provider::Deezer => deezer_arl.is_some(),
            Provider::SoundCloud => soundcloud_token.is_some(),
        },
    }
}

fn cached_track_info(key: &TrackInfoCacheKey) -> Option<TrackInfo> {
    TRACK_INFO_CACHE.lock().ok()?.get(key).cloned()
}

fn store_track_info(key: TrackInfoCacheKey, info: TrackInfo) {
    if let Ok(mut cache) = TRACK_INFO_CACHE.lock() {
        if cache.len() >= TRACK_INFO_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key.clone(), info);
    }
    if let Ok(mut in_flight) = TRACK_INFO_IN_FLIGHT.lock() {
        in_flight.remove(&key);
    }
}

/// Warms the tag cache on menu open so the dialog renders populated. The
/// guarded spawn is a no-op when a fetch is already in flight or cached.
pub(crate) fn preload_track_info(
    provider: Provider,
    track_id: String,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    runtime: &Arc<Runtime>,
) {
    let key = track_info_cache_key(provider, &track_id, &deezer_arl, &soundcloud_token);
    if cached_track_info(&key).is_some() {
        return;
    }
    if let Ok(mut in_flight) = TRACK_INFO_IN_FLIGHT.lock()
        && !in_flight.insert(key.clone())
    {
        return;
    }
    runtime.spawn(async move {
        fetch_and_store_track_info(provider, &track_id, deezer_arl, soundcloud_token).await;
    });
}

/// Full tag set shown by the track Info dialog. Field order matches the
/// original tags dialog: file-level rows first, then album rows, then audio
/// rows. Sources are the public Deezer REST track/album payloads plus the
/// gw-light pageTrack response (composer-style contributors and copyright).
/// Rows without a value stay hidden, like the card info dialog.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TrackInfo {
    pub(crate) artists: String,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) album: String,
    pub(crate) release_date: String,
    pub(crate) album_artist: String,
    pub(crate) album_type: String,
    pub(crate) genre: String,
    pub(crate) explicit: Option<bool>,
    pub(crate) composer: String,
    pub(crate) author: String,
    pub(crate) writer: String,
    pub(crate) record_label: String,
    pub(crate) copyright: String,
    pub(crate) isrc: String,
    pub(crate) barcode: String,
    pub(crate) track_number: Option<u64>,
    pub(crate) total_tracks: Option<u64>,
    pub(crate) disc_number: Option<u64>,
    pub(crate) total_discs: Option<u64>,
    pub(crate) duration_secs: Option<u64>,
    pub(crate) bpm: Option<i64>,
    pub(crate) gain_db: Option<f64>,
    pub(crate) peak: Option<f64>,
    pub(crate) description: String,
}

impl TrackInfo {
    /// Seeds the dialog with what the menu already knows so it opens
    /// populated. The fetch below fills the remaining rows in place.
    pub(crate) fn seed(
        title: String,
        artist: String,
        album: String,
        release_date: String,
        duration_secs: u64,
    ) -> Self {
        Self {
            artists: artist,
            title,
            album,
            release_date,
            duration_secs: (duration_secs > 0).then_some(duration_secs),
            ..Self::default()
        }
    }

    pub(crate) fn soundcloud_seed(title: String, artist: String, duration_secs: u64) -> Self {
        Self {
            artists: artist,
            title,
            duration_secs: (duration_secs > 0).then_some(duration_secs),
            ..Self::default()
        }
    }

    fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = Vec::new();
        push_text(&mut rows, "Artists", &self.artists);
        push_text(&mut rows, "Title", &self.title);
        push_text(&mut rows, "Subtitle", &self.subtitle);
        push_text(&mut rows, "Album", &self.album);
        push_text(
            &mut rows,
            "Release Date",
            &format_release_date(&self.release_date),
        );
        push_text(&mut rows, "Album artist", &self.album_artist);
        push_text(&mut rows, "Album type", &self.album_type);
        push_text(&mut rows, "Genre", &self.genre);
        if let Some(explicit) = self.explicit {
            rows.push(("Explicit", if explicit { "Yes" } else { "No" }.to_owned()));
        }
        push_text(&mut rows, "Composer", &self.composer);
        push_text(&mut rows, "Author", &self.author);
        push_text(&mut rows, "Writer", &self.writer);
        push_text(&mut rows, "Record Label", &self.record_label);
        push_text(&mut rows, "Copyright", &self.copyright);
        push_text(&mut rows, "ISRC", &self.isrc);
        push_text(&mut rows, "Barcode", &self.barcode);
        if let Some(number) = self.track_number.filter(|number| *number > 0) {
            rows.push(("Track number", number.to_string()));
        }
        if let Some(total) = self.total_tracks.filter(|total| *total > 0) {
            rows.push(("Total tracks", total.to_string()));
        }
        if let Some(number) = self.disc_number.filter(|number| *number > 0) {
            rows.push(("Disc number", number.to_string()));
        }
        if let Some(total) = self.total_discs.filter(|total| *total > 0) {
            rows.push(("Total discs", total.to_string()));
        }
        if let Some(duration) = self.duration_secs.filter(|duration| *duration > 0) {
            rows.push(("Duration", format_track_duration(duration)));
        }
        if let Some(bpm) = self.bpm.filter(|bpm| *bpm > 0) {
            rows.push(("BPM", bpm.to_string()));
        }
        if let Some(gain) = self.gain_db {
            rows.push(("Gain", format!("{gain} dB")));
        }
        if let Some(peak) = self.peak {
            rows.push(("Peak", peak.to_string()));
        }
        push_text(&mut rows, "Description", &self.description);
        rows
    }
}

pub(crate) fn format_track_duration(total_seconds: u64) -> String {
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn push_text(rows: &mut Vec<(&'static str, String)>, label: &'static str, value: &str) {
    if !value.trim().is_empty() {
        rows.push((label, value.to_owned()));
    }
}

fn text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn id_string(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| {
        value
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| value.as_str().map(str::to_owned))
    })
}

fn names(values: Option<&Value>) -> String {
    values
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = text(item.get("name"));
                    (!name.is_empty()).then_some(name)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Merges the REST track/album payloads, the pageTrack data, and the album
/// track list into one tag set. Each source is optional so a partial fetch
/// still renders the rows it resolved.
#[allow(clippy::field_reassign_with_default)]
pub(crate) fn parse_deezer_track_info(
    track: &Value,
    album: Option<&Value>,
    page: Option<&Value>,
    album_tracks: Option<&Value>,
) -> TrackInfo {
    let mut info = TrackInfo::default();
    info.artists = names(track.get("contributors"));
    if info.artists.is_empty() {
        info.artists = text(track.pointer("/artist/name"));
    }
    info.title = text(track.get("title"));
    info.subtitle = text(track.get("title_version"));
    info.album = text(track.pointer("/album/title"));
    info.release_date = text(track.get("release_date"));
    info.explicit = track.get("explicit_lyrics").and_then(Value::as_bool);
    info.isrc = text(track.get("isrc"));
    info.track_number = track.get("track_position").and_then(Value::as_u64);
    info.disc_number = track.get("disk_number").and_then(Value::as_u64);
    info.duration_secs = track.get("duration").and_then(Value::as_u64);
    info.bpm = track.get("bpm").and_then(Value::as_i64);
    info.gain_db = track.get("gain").and_then(Value::as_f64);
    if let Some(album) = album {
        info.album_artist = text(album.pointer("/artist/name"));
        info.album_type = capitalize(&text(album.get("record_type")));
        info.genre = album
            .pointer("/genres/data")
            .map(|data| names(Some(data)))
            .unwrap_or_default();
        info.record_label = text(album.get("label"));
        info.barcode = text(album.get("upc"));
        info.total_tracks = album.get("nb_tracks").and_then(Value::as_u64);
        if info.release_date.is_empty() {
            info.release_date = text(album.get("release_date"));
        }
    }
    if let Some(page) = page {
        let contributors = |role: &str| {
            page.pointer(&format!("/SNG_CONTRIBUTORS/{role}"))
                .and_then(Value::as_array)
                .map(|people| {
                    people
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
        };
        info.composer = contributors("composer");
        info.author = contributors("author");
        info.writer = contributors("writer");
        info.copyright = text(page.get("COPYRIGHT"));
    }
    info.total_discs = album_tracks
        .and_then(|tracks| tracks.get("data"))
        .and_then(Value::as_array)
        .and_then(|tracks| {
            tracks
                .iter()
                .filter_map(|track| track.get("disk_number").and_then(Value::as_u64))
                .max()
        });
    info
}

#[allow(clippy::field_reassign_with_default)]
pub(crate) fn parse_soundcloud_track_info(track: &Value) -> TrackInfo {
    let mut info = TrackInfo::default();
    info.artists = [
        text(track.pointer("/publisher_metadata/artist")),
        text(track.pointer("/user/username")),
        text(track.pointer("/user/full_name")),
    ]
    .into_iter()
    .find(|value| !value.is_empty())
    .unwrap_or_default();
    info.title = text(track.get("title"));
    info.genre = text(track.get("genre"));
    info.explicit = track.get("explicit").and_then(Value::as_bool).or_else(|| {
        track
            .pointer("/publisher_metadata/explicit")
            .and_then(Value::as_bool)
    });
    info.duration_secs = track
        .get("duration")
        .and_then(json_u64)
        .filter(|duration_ms| *duration_ms > 0)
        .or_else(|| {
            track
                .get("full_duration")
                .and_then(json_u64)
                .filter(|duration_ms| *duration_ms > 0)
        })
        .map(|duration_ms| duration_ms / 1000)
        .filter(|duration_secs| *duration_secs > 0);
    info.description = text(track.get("description"));
    info
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())
}

const REST_BASE: &str = "https://api.deezer.com";
const GATEWAY_BASE: &str = "https://www.deezer.com/ajax/gw-light.php";
const SOUNDCLOUD_API: &str = "https://api-v2.soundcloud.com";
const SOUNDCLOUD_USER_AGENT: &str = "ktor-client";
const SOUNDCLOUD_ACCEPT_ENCODING: &str = "gzip,deflate,identity";

/// Fetches then caches the tag set, logging the failing step (never the
/// error text: gateway failures can carry the session token in the URL).
async fn fetch_and_store_track_info(
    provider: Provider,
    track_id: &str,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
) -> Option<TrackInfo> {
    let key = track_info_cache_key(provider, track_id, &deezer_arl, &soundcloud_token);
    let result = match provider {
        Provider::Deezer => fetch_deezer_track_info(track_id, deezer_arl).await,
        Provider::SoundCloud => fetch_soundcloud_track_info(track_id, soundcloud_token).await,
    };
    match result {
        Ok(info) => {
            store_track_info(key, info.clone());
            Some(info)
        }
        Err(step) => {
            crate::diagnostics::event(
                "WARN",
                format!("track-info fetch track_id={track_id} step={step}"),
            );
            if let Ok(mut in_flight) = TRACK_INFO_IN_FLIGHT.lock() {
                in_flight.remove(&key);
            }
            None
        }
    }
}

async fn fetch_soundcloud_track_info(
    track_id: &str,
    token: Option<SoundCloudToken>,
) -> Result<TrackInfo, &'static str> {
    let token = token.ok_or("soundcloud-account")?;
    let authorization = token
        .authorization_header()
        .map_err(|_| "soundcloud-credential")?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .user_agent(SOUNDCLOUD_USER_AGENT)
        .build()
        .map_err(|_| "http-client")?;
    let response = http
        .get(format!("{SOUNDCLOUD_API}/tracks/{track_id}"))
        .header(header::AUTHORIZATION, authorization)
        .header(header::ACCEPT, "*/*")
        .header(header::ACCEPT_ENCODING, SOUNDCLOUD_ACCEPT_ENCODING)
        .query(&[("client_id", SOUNDCLOUD_CLIENT_ID)])
        .send()
        .await
        .map_err(|_| "soundcloud-track")?;
    if !response.status().is_success() {
        return Err("soundcloud-track-status");
    }
    let track: Value = response.json().await.map_err(|_| "soundcloud-track-body")?;
    Ok(parse_soundcloud_track_info(&track))
}

async fn fetch_deezer_track_info(
    track_id: &str,
    deezer_arl: Option<DeezerArl>,
) -> Result<TrackInfo, &'static str> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|_| "http-client")?;
    let track: Value = http
        .get(format!("{REST_BASE}/track/{track_id}"))
        .send()
        .await
        .map_err(|_| "rest-track")?
        .json()
        .await
        .map_err(|_| "rest-track-body")?;
    if track.get("error").is_some() {
        return Err("rest-track-error");
    }
    let album_id = id_string(track.pointer("/album/id")).ok_or("missing-album-id")?;
    let album: Option<Value> = http
        .get(format!("{REST_BASE}/album/{album_id}"))
        .send()
        .await
        .map_err(|_| "rest-album")?
        .json()
        .await
        .ok()
        .filter(|album: &Value| album.get("error").is_none());
    let album_tracks: Option<Value> = http
        .get(format!("{REST_BASE}/album/{album_id}/tracks?limit=2000"))
        .send()
        .await
        .map_err(|_| "rest-album-tracks")?
        .json()
        .await
        .ok();
    let page = match deezer_arl {
        Some(arl) => page_track(&http, track_id, &arl).await.ok(),
        None => None,
    };
    Ok(parse_deezer_track_info(
        &track,
        album.as_ref(),
        page.as_ref(),
        album_tracks.as_ref(),
    ))
}

/// Fetches the gw-light pageTrack response, which carries the contributor
/// roles and copyright the public REST payloads omit. Mirrors the search
/// client session bootstrap: getUserData yields the checkForm token, then
/// pageTrack runs authenticated with it.
async fn page_track(
    http: &reqwest::Client,
    track_id: &str,
    arl: &DeezerArl,
) -> Result<Value, String> {
    let arl_cookie = arl.cookie_header().map_err(|error| error.message.clone())?;
    let session_response = http
        .post(format!(
            "{GATEWAY_BASE}?method=deezer.getUserData&input=3&api_version=1.0&api_token="
        ))
        .header(header::COOKIE, arl_cookie)
        .header(header::CONTENT_LENGTH, "0")
        .body("")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let session_cookies = session_response
        .cookies()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");
    let session: Value = session_response
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let check_form = session
        .pointer("/results/checkForm")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| "Deezer login required".to_owned())?;
    let mut cookie = arl.cookie_header().map_err(|error| error.message.clone())?;
    if !session_cookies.is_empty() {
        let arl = cookie
            .to_str()
            .map_err(|_| "The saved Deezer session is invalid".to_owned())?;
        cookie = header::HeaderValue::from_str(&format!("{arl}; {session_cookies}"))
            .map_err(|_| "Deezer returned an invalid session".to_owned())?;
        cookie.set_sensitive(true);
    }
    let page: Value = http
        .post(format!(
            "{GATEWAY_BASE}?method=deezer.pageTrack&input=3&api_version=1.0&api_token={check_form}"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
        .json(&json!({"sng_id": track_id}))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    page.pointer("/results/DATA")
        .cloned()
        .ok_or_else(|| "Deezer returned an invalid track response".to_owned())
}

/// Opens the Info dialog, using the preloaded tag set when the menu open
/// already warmed it so the dialog renders fully populated. Falls back to
/// the seeded rows then fetch flow otherwise. Fresh fetches update the
/// cache so later dialogs and preloads avoid duplicate requests.
pub(crate) fn open_track_info_dialog<T: 'static>(
    provider: Provider,
    track_id: String,
    seed: TrackInfo,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    runtime: Arc<Runtime>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    let key = track_info_cache_key(provider, &track_id, &deezer_arl, &soundcloud_token);
    if let Some(cached) = cached_track_info(&key) {
        let dialog = cx.new(|_| TrackInfoDialog {
            info: cached,
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            loaded_epoch: 1,
            close_motion: DialogCloseMotion::default(),
        });
        open_track_info_dialog_entity(dialog, window, cx);
        return;
    }
    let dialog = cx.new(|_| TrackInfoDialog {
        info: seed,
        scroll: ScrollHandle::new(),
        browser_scroll: BrowserScrollState::new(),
        loaded_epoch: 0,
        close_motion: DialogCloseMotion::default(),
    });
    open_track_info_dialog_entity(dialog.clone(), window, cx);
    // reqwest needs a Tokio reactor, which the GPUI executor does not
    // provide, so the fetch runs on the app runtime while the dialog swap
    // stays on the UI thread.
    let task = runtime.spawn(async move {
        fetch_and_store_track_info(provider, &track_id, deezer_arl, soundcloud_token).await
    });
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Some(info)) = task.await else {
            return;
        };
        let _ = this.update_in(cx, |_view, window, cx| {
            if window.has_active_dialog(cx) {
                dialog.update(cx, |dialog, cx| {
                    dialog.info = info;
                    dialog.loaded_epoch = dialog.loaded_epoch.wrapping_add(1);
                    cx.notify();
                });
            }
        });
    })
    .detach();
}

fn open_track_info_dialog_entity<T: 'static>(
    dialog_content: gpui::Entity<TrackInfoDialog>,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    window.open_dialog(cx, move |dialog_view, dialog_window, cx| {
        let viewport_height = f32::from(dialog_window.viewport_size().height);
        let row_count = dialog_content.read(cx).info.rows().len();
        let max_h = track_info_dialog_max_height(viewport_height);
        let dialog_height = track_info_dialog_height(viewport_height, row_count);
        let centered_top = px(track_info_margin_top(viewport_height, row_count));
        let closing = dialog_content.read(cx).close_motion.closing();
        let close_epoch = dialog_content.read(cx).close_motion.epoch();
        let dialog_handle = dialog_content.clone();
        dialog_view
            .w(px(INFO_DIALOG_WIDTH))
            .max_w(px(560.))
            .h(px(dialog_height))
            .min_h(px(dialog_height))
            .max_h(px(max_h))
            .margin_top(centered_top)
            .p_0()
            .gap_0()
            .bg(gpui::rgba(0x00000000))
            .border_0()
            .rounded(px(8.))
            .close_button(false)
            .overlay(true)
            .overlay_closable(true)
            .keyboard(false)
            .on_cancel(move |_, window, cx| {
                dialog_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                false
            })
            .closing(closing, close_epoch)
            .child(dialog_content.clone())
    });
}

struct TrackInfoDialog {
    info: TrackInfo,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
    /// Bumped on every fetch swap so the rows entrance replays once.
    loaded_epoch: usize,
    close_motion: DialogCloseMotion,
}

impl DialogCloseTarget for TrackInfoDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for TrackInfoDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.info.rows();
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog_entity = cx.entity();
        let viewport_height = f32::from(window.viewport_size().height);
        let dialog_max = track_info_dialog_max_height(viewport_height);
        let dialog_height = track_info_dialog_height(viewport_height, rows.len());
        let scroll_h = track_info_scroll_height(viewport_height, rows.len());

        let rows = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(7.))
            .pr(px(10.))
            .children(rows.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .gap(px(12.))
                    .child(
                        div()
                            .w(px(INFO_DIALOG_LABEL_WIDTH))
                            .flex_none()
                            .text_size(px(12.))
                            .line_height(px(INFO_DIALOG_ROW_LINE_HEIGHT))
                            .text_color(rgb(MUTED))
                            .child(label),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.5))
                            .line_height(px(INFO_DIALOG_ROW_LINE_HEIGHT))
                            .child(value),
                    )
            }));
        let rows = if self.loaded_epoch == 0 {
            rows.into_any_element()
        } else {
            rows.with_animation(
                ("track-info-rows", self.loaded_epoch),
                crate::motion::quick_content(),
                |this, delta| {
                    this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                        .top(px(crate::motion::lerp(4.0, 0.0, delta)))
                },
            )
            .into_any_element()
        };
        let list_content = div()
            .id("track-info-rows")
            .flex()
            .flex_col()
            .h(px(scroll_h))
            .w_full()
            .min_w_0()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .child(rows)
            .into_any_element();
        let scrolled = browser_scroll_surface(
            "track-info-scroll",
            list_content,
            BrowserScrollTarget::Handle(self.scroll.clone()),
            self.browser_scroll.clone(),
        );
        let body = div()
            .id("track-info-body")
            .relative()
            .w_full()
            .p(px(INFO_DIALOG_BODY_PADDING))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(scroll_h))
                    .child(scrolled)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .right(px(-INFO_DIALOG_BODY_PADDING))
                            .child(
                                Scrollbar::vertical(&self.scroll)
                                    .scrollbar_show(ScrollbarShow::Hover),
                            ),
                    ),
            );

        let dialog = div()
            .id("track-info-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                request_dialog_close(this, window, cx);
                cx.stop_propagation();
            }))
            .flex()
            .flex_col()
            .overflow_hidden()
            .w_full()
            .h(px(dialog_height))
            .min_h(px(dialog_height))
            .max_h(px(dialog_max))
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .pl(px(18.))
                    .pr(px(14.))
                    .py(px(14.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Track info"))
                    .child(ghost_close_button_with_icon_size(
                        "track-info-close",
                        10.,
                        move |_, window, cx| {
                            dialog_entity
                                .update(cx, |this, cx| request_dialog_close(this, window, cx));
                        },
                    )),
            )
            .child(body);

        if closing {
            dialog
                .with_animation(
                    ("track-info-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn track_durations_render_as_minutes_and_seconds() {
        assert_eq!(format_track_duration(161), "2:41");
        assert_eq!(format_track_duration(59), "0:59");
        assert_eq!(format_track_duration(3600), "60:00");
    }

    #[test]
    fn seed_keeps_only_the_menu_known_rows() {
        let seed = TrackInfo::seed(
            "Title".into(),
            "Artist".into(),
            "Album".into(),
            "2021-08-09".into(),
            161,
        );
        let rows = seed.rows();
        assert_eq!(
            rows,
            vec![
                ("Artists", "Artist".to_owned()),
                ("Title", "Title".to_owned()),
                ("Album", "Album".to_owned()),
                ("Release Date", "Aug 9, 2021".to_owned()),
                ("Duration", "2:41".to_owned()),
            ]
        );
    }

    #[test]
    fn rest_and_page_responses_map_every_dialog_row() {
        let info = parse_deezer_track_info(
            &json!({
                "title": "TRASH CLAN",
                "title_version": "Extended",
                "contributors": [{"name": "Metaroom"}],
                "release_date": "2021-08-09",
                "explicit_lyrics": false,
                "isrc": "QZK6H2164212",
                "track_position": 1,
                "disk_number": 2,
                "duration": 161,
                "bpm": 128,
                "gain": -7.7,
                "album": {"id": 852192912, "title": "TRASH CLAN"},
            }),
            Some(&json!({
                "artist": {"name": "Metaroom"},
                "record_type": "single",
                "genres": {"data": [{"name": "Dance"}]},
                "label": "Nothing World 2021",
                "upc": "196323953496",
                "nb_tracks": 1,
            })),
            Some(&json!({
                "SNG_CONTRIBUTORS": {"composer": ["Federico Lassalle"]},
                "COPYRIGHT": "2021 Federico Lassalle",
            })),
            Some(&json!({"data": [
                {"disk_number": 1},
                {"disk_number": 2},
            ]})),
        );
        let rows = info.rows();
        let value = |label: &str| {
            rows.iter()
                .find(|(row, _)| *row == label)
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| panic!("missing row {label}"))
        };
        assert_eq!(value("Artists"), "Metaroom");
        assert_eq!(value("Title"), "TRASH CLAN");
        assert_eq!(value("Subtitle"), "Extended");
        assert_eq!(value("Album"), "TRASH CLAN");
        assert_eq!(value("Release Date"), "Aug 9, 2021");
        assert_eq!(value("Album artist"), "Metaroom");
        assert_eq!(value("Album type"), "Single");
        assert_eq!(value("Genre"), "Dance");
        assert_eq!(value("Explicit"), "No");
        assert_eq!(value("Composer"), "Federico Lassalle");
        assert_eq!(value("Record Label"), "Nothing World 2021");
        assert_eq!(value("Copyright"), "2021 Federico Lassalle");
        assert_eq!(value("ISRC"), "QZK6H2164212");
        assert_eq!(value("Barcode"), "196323953496");
        assert_eq!(value("Track number"), "1");
        assert_eq!(value("Total tracks"), "1");
        assert_eq!(value("Disc number"), "2");
        assert_eq!(value("Total discs"), "2");
        assert_eq!(value("Duration"), "2:41");
        assert_eq!(value("BPM"), "128");
        assert_eq!(value("Gain"), "-7.7 dB");
        assert!(!rows.iter().any(|(row, _)| *row == "Author"));
        assert!(!rows.iter().any(|(row, _)| *row == "Peak"));
    }

    #[test]
    fn preloaded_tags_are_reused_and_split_by_login_state() {
        let authenticated = track_info_cache_key(Provider::Deezer, "100", &None, &None);
        assert!(!authenticated.authenticated);
        let other = TrackInfoCacheKey {
            provider: Provider::Deezer,
            track_id: "100".into(),
            authenticated: true,
        };
        assert_ne!(authenticated, other);

        let info = TrackInfo {
            title: "Cached".into(),
            ..TrackInfo::default()
        };
        store_track_info(authenticated.clone(), info.clone());
        assert_eq!(cached_track_info(&authenticated), Some(info));
        assert_eq!(cached_track_info(&other), None);
        if let Ok(mut in_flight) = TRACK_INFO_IN_FLIGHT.lock() {
            in_flight.remove(&authenticated);
        }
        if let Ok(mut cache) = TRACK_INFO_CACHE.lock() {
            cache.remove(&authenticated);
        }
    }

    #[test]
    fn soundcloud_response_maps_the_existing_dialog_fields() {
        let info = parse_soundcloud_track_info(&json!({
            "id": 2297202227u64,
            "kind": "track",
            "title": "Captured title",
            "description": "Captured description",
            "genre": "Dance & EDM",
            "duration": 130_332,
            "full_duration": 130_324,
            "publisher_metadata": {
                "artist": "Published Artist",
                "explicit": false
            },
            "user": {"username": "Uploader"}
        }));
        assert_eq!(
            info.rows(),
            vec![
                ("Artists", "Published Artist".into()),
                ("Title", "Captured title".into()),
                ("Genre", "Dance & EDM".into()),
                ("Explicit", "No".into()),
                ("Duration", "2:10".into()),
                ("Description", "Captured description".into()),
            ]
        );
    }

    #[test]
    fn soundcloud_metadata_preserves_missing_values_and_duration_fallback() {
        let info = parse_soundcloud_track_info(&json!({
            "title": "Sparse track",
            "caption": "Unsupported subtitle",
            "album_type": "Unsupported album type",
            "bpm": 128,
            "replay_gain": -7.75,
            "peak": 0.98,
            "duration": 0,
            "full_duration": 61_999,
            "publisher_metadata": {"release_title": "Not a subtitle"},
            "user": {"username": "Uploader"},
            "kind": "track"
        }));
        assert_eq!(
            info.rows(),
            vec![
                ("Artists", "Uploader".into()),
                ("Title", "Sparse track".into()),
                ("Duration", "1:01".into()),
            ]
        );
        assert_eq!(info.explicit, None);
    }

    #[test]
    fn soundcloud_cache_entries_do_not_collide_with_deezer() {
        let deezer = track_info_cache_key(Provider::Deezer, "100", &None, &None);
        let soundcloud = track_info_cache_key(Provider::SoundCloud, "100", &None, &None);
        assert_ne!(deezer, soundcloud);
    }

    #[test]
    fn dialog_body_scrolls_inside_short_windows() {
        const CHROME: f32 = 57. + 36. + 2.;
        assert_eq!(track_info_dialog_max_height(900.), 868.);
        assert_eq!(track_info_dialog_max_height(20.), 0.);
        assert_eq!(track_info_scroll_height(900., 100), 868. - CHROME);
        assert_eq!(track_info_scroll_height(100., 24), 0.);
        // Hug content on a tall window; lock to the viewport cap on a short one.
        assert_eq!(track_info_dialog_height(900., 24), CHROME + 24. * 26.);
        assert_eq!(track_info_dialog_height(400., 24), 368.);
        assert_eq!(track_info_dialog_height(300., 24), 268.);
        assert_eq!(track_info_scroll_height(900., 24), 24. * 26.);
        assert_eq!(track_info_scroll_height(400., 24), 368. - CHROME);
        // The margin follows the live content so seeded and full tag sets
        // both center. Preload usually opens fully populated, so the
        // fetch swap rarely recenters after open.
        assert_eq!(
            track_info_margin_top(900., 24),
            (900. - (CHROME + 24. * 26.)) / 2.
        );
        assert_eq!(
            track_info_margin_top(900., 5),
            (900. - (CHROME + 5. * 26.)) / 2.
        );
        assert_eq!(track_info_margin_top(400., 24), 16.);
        assert_eq!(track_info_margin_top(300., 24), 16.);
    }

    #[test]
    fn dialog_stays_on_screen_at_short_window_heights() {
        for viewport in [200., 400., 658., 720., 900.] {
            for rows in [1, 5, 12, 20, 24] {
                let height = track_info_dialog_height(viewport, rows);
                let top = track_info_margin_top(viewport, rows);
                assert!(
                    top + height <= viewport,
                    "viewport={viewport} rows={rows} top={top} height={height}"
                );
                assert!(height <= track_info_dialog_max_height(viewport));
            }
        }
        // The reported clip height: a full tag list must lock to the
        // viewport cap so the bottom edge stays on screen.
        assert_eq!(track_info_dialog_height(658., 24), 626.);
        assert_eq!(track_info_margin_top(658., 24), 16.);
    }

    #[test]
    fn dialog_row_order_matches_the_original_tags_dialog() {
        let info = TrackInfo {
            artists: "a".into(),
            title: "b".into(),
            subtitle: "c".into(),
            album: "d".into(),
            release_date: "2021-08-09".into(),
            album_artist: "e".into(),
            album_type: "Single".into(),
            genre: "f".into(),
            explicit: Some(true),
            composer: "g".into(),
            author: "h".into(),
            writer: "i".into(),
            record_label: "j".into(),
            copyright: "k".into(),
            isrc: "l".into(),
            barcode: "m".into(),
            track_number: Some(1),
            total_tracks: Some(2),
            disc_number: Some(3),
            total_discs: Some(4),
            duration_secs: Some(161),
            bpm: Some(128),
            gain_db: Some(-7.7),
            peak: Some(0.98),
            ..TrackInfo::default()
        };
        let labels = info
            .rows()
            .into_iter()
            .map(|(label, _)| label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "Artists",
                "Title",
                "Subtitle",
                "Album",
                "Release Date",
                "Album artist",
                "Album type",
                "Genre",
                "Explicit",
                "Composer",
                "Author",
                "Writer",
                "Record Label",
                "Copyright",
                "ISRC",
                "Barcode",
                "Track number",
                "Total tracks",
                "Disc number",
                "Total discs",
                "Duration",
                "BPM",
                "Gain",
                "Peak",
            ]
        );
    }
}
