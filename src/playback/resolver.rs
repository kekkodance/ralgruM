use std::{
    cmp::Reverse,
    io::Read as StdRead,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use blowfish::{
    Blowfish,
    cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray},
};
use chrono::NaiveDate;
use futures::{StreamExt, future::join_all};
use md5::{Digest, Md5};
use reqwest::{
    Client, Response, StatusCode,
    header::{self, HeaderMap},
};
use serde_json::{Value, json};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::Semaphore,
};
use tokio_util::sync::CancellationToken;

use crate::{
    murglar_backend::{MediaBackend, MediaCredentials},
    search::{
        DEEZER_USER_AGENT, DeezerArl, SOUNDCLOUD_CLIENT_ID, SoundCloudToken, merge_cookie_parts,
    },
};

use super::cache::{AudioCache, BLOCK_SIZE, CacheTrackToken};
use super::listen_history::SoundCloudListenReport;
use super::media_source::{BackendProvider, BackendSource, MediaRequest, MediaResolveOutcome};
use super::progressive::{
    DownloadPauseGate, ProgressiveFile, ProgressiveReader, ProgressiveWriter, TimelineSeekSession,
    startup_bytes,
};
use super::resolve_limiter::ResolveLimiter;
use super::resolve_source_cache::{ResolvedSourceCache, SourceCacheValue, SourceResolveFlights};
use super::retry::{RequestClass, RetryBudget, send_with_retry};
use super::soundcloud_hls::{self, HlsDescriptor};
use super::{
    DownloadChoice, DownloadVariant, PlaybackProvider, PlaybackTrack,
    deezer_collection_download_choices, selection_order,
};

// Single deezer.com browser profile shared by every direct client; kept as a
// local alias because the media submodules reach it through `super::*`.
const BROWSER_USER_AGENT: &str = DEEZER_USER_AGENT;
const SOUNDCLOUD_MOBILE_USER_AGENT: &str = "ktor-client";
const SOUNDCLOUD_MOBILE_ACCEPT_ENCODING: &str = "gzip,deflate,identity";
const DEEZER_SECRET: &[u8; 16] = b"g4el58wc0zvf9na1";
const STRIPE_SIZE: usize = 2048;
const RANGE_CHUNK: u64 = 1024 * 1024;
const MAX_AUDIO_SIZE: u64 = 512 * 1024 * 1024;
const DEEZER_STREAM_WRITE_BUFFER_SIZE: usize = 64 * 1024;
const PROGRESSIVE_WRITE_CHUNK_SIZE: usize = 64 * 1024;
const MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES: usize = 8;
const MAX_SOUNDCLOUD_OWNER_SLUG_LENGTH: usize = 128;
const SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN: &str =
    "SoundCloud original format could not be identified";
const SOUNDCLOUD_ORIGINAL_INSPECTION_BYTES: usize = 4096;
const DEEZER_UNAVAILABLE: &str = "This track is unavailable in Deezer for this account or region.";

// Murglar has its own process-wide limiter. Direct Deezer capability requests
// do not, so keep those exact-format probes from repeating session/song work
// concurrently while allowing Murglar candidates to use the official limiter.
static DIRECT_DEEZER_CAPABILITY_GATE: Semaphore = Semaphore::const_new(1);

#[derive(Clone)]
pub(crate) struct StreamResolver {
    client: Client,
    media_backend: MediaBackend,
    cache: Option<AudioCache>,
    limiter: ResolveLimiter,
    resolved_source_cache: ResolvedSourceCache<ResolvedSource>,
    source_resolve_flights: SourceResolveFlights<ResolvedSource>,
    #[cfg(test)]
    backend_resolve_override: Option<backend_tests::BackendResolveOverride>,
}

pub(crate) struct ResolvedAudio {
    pub(crate) path: PathBuf,
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) duration: Option<Duration>,
    pub(crate) format: AudioFormat,
    pub(crate) declared_bitrate: Option<u32>,
}

pub(crate) struct ResolvedProgressiveAudio {
    pub(crate) reader: ProgressiveReader,
    pub(crate) file: tempfile::NamedTempFile,
    pub(crate) duration: Option<Duration>,
    pub(crate) format: AudioFormat,
    pub(crate) total: Option<u64>,
    pub(crate) timeline_size_unknown: bool,
    pub(crate) declared_bitrate: Option<u32>,
    pub(crate) initial_downloaded: u64,
    pub(crate) initial_buffered_fraction: Option<f32>,
    pub(crate) fully_cached: bool,
    pub(crate) timeline_seek_session: Option<Arc<dyn TimelineSeekSession>>,
    pub(crate) worker: Option<ProgressiveDownload>,
}

pub(crate) struct ProgressiveDownload {
    cancellation: Option<CancellationToken>,
    task: Option<tokio::task::JoinHandle<(ProgressiveWriter, Result<(), String>)>>,
}

impl ProgressiveDownload {
    pub(crate) async fn cancel_and_join(mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    pub(crate) fn detach(mut self) {
        let _ = self.cancellation.take();
        let _ = self.task.take();
    }
}

impl Drop for ProgressiveDownload {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }
}

impl ResolvedProgressiveAudio {
    pub(crate) fn take_progressive_worker(&mut self) -> ProgressiveDownload {
        self.worker
            .take()
            .expect("progressive audio must retain its download worker")
    }
}

pub(crate) use super::media_source::{
    AudioFormat, DownloadOutput, PlaybackDownloadError, ProgressCallback, ProgressUpdate,
};

#[derive(Clone)]
pub(crate) struct ResolvedSource {
    data: SourceData,
    size: u64,
    deezer_track_id: Option<String>,
    is_soundcloud: bool,
    cache_identity: Option<String>,
    pub(crate) format: AudioFormat,
    pub(crate) format_name: String,
    pub(crate) declared_bitrate: Option<u32>,
}

impl ResolvedSource {
    /// Total byte size of the resolved audio, when the provider reported one.
    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    pub(crate) fn timeline_size_unknown(&self) -> bool {
        self.size == 0
            && (matches!(&self.data, SourceData::Hls(_))
                || matches!(&self.data, SourceData::Backend(source) if source.metadata().timeline))
    }

    fn uses_backend(&self) -> bool {
        matches!(&self.data, SourceData::Backend(_))
    }
}

impl SourceCacheValue for ResolvedSource {
    fn is_cacheable(&self) -> bool {
        matches!(self.data, SourceData::Remote(_) | SourceData::Hls(_))
            || matches!(&self.data, SourceData::Backend(source) if source.is_cacheable())
    }
}

struct RemoteAudio {
    data: SourceData,
    size: u64,
    deezer_track_id: Option<String>,
    format: AudioFormat,
    format_name: String,
    declared_bitrate: Option<u32>,
    is_soundcloud: bool,
    cache_identity: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct DeezerPlaybackIdentity {
    source_track_id: String,
    track_token: String,
    used_fallback: bool,
}

struct DeezerPlaybackContext {
    headers: HeaderMap,
    license_token: String,
    identity: DeezerPlaybackIdentity,
}

#[derive(Default)]
struct SoundCloudOriginalInspection {
    sniffed_format: Option<AudioFormat>,
    header_format: Option<AudioFormat>,
    size: Option<u64>,
}

fn deezer_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => (!value.trim().is_empty()).then(|| value.trim().to_owned()),
        Value::Number(value) => value.as_u64().map(|value| value.to_string()),
        _ => None,
    }
}

fn deezer_token(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn deezer_playback_identity(
    response: &Value,
    requested_track_id: &str,
) -> Result<DeezerPlaybackIdentity, String> {
    let song = response
        .pointer("/results/DATA")
        .or_else(|| response.get("results"))
        .unwrap_or(&Value::Null);

    if let Some(fallback) = song.get("FALLBACK")
        && let (Some(source_track_id), Some(track_token)) = (
            deezer_id(fallback.get("SNG_ID")),
            deezer_token(fallback.get("TRACK_TOKEN")),
        )
    {
        return Ok(DeezerPlaybackIdentity {
            source_track_id,
            track_token,
            used_fallback: true,
        });
    }

    let track_token = deezer_token(song.get("TRACK_TOKEN"))
        .ok_or_else(|| "Deezer did not return a track token".to_string())?;
    let source_track_id =
        deezer_id(song.get("SNG_ID")).unwrap_or_else(|| requested_track_id.to_owned());
    Ok(DeezerPlaybackIdentity {
        source_track_id,
        track_token,
        used_fallback: false,
    })
}

fn deezer_fallback_track_id<'a>(
    identity: &'a DeezerPlaybackIdentity,
    requested_track_id: &str,
) -> Option<&'a str> {
    identity
        .used_fallback
        .then_some(identity.source_track_id.as_str())
        .filter(|track_id| *track_id != requested_track_id)
}

fn deezer_track_is_unavailable(response: &Value) -> bool {
    let song = response
        .pointer("/results/DATA")
        .or_else(|| response.get("results"))
        .unwrap_or(&Value::Null);
    if song.get("FALLBACK").is_some_and(|fallback| {
        deezer_id(fallback.get("SNG_ID")).is_some()
            && deezer_token(fallback.get("TRACK_TOKEN")).is_some()
    }) {
        return false;
    }
    let readable = song
        .get("READABLE")
        .or_else(|| song.get("readable"))
        .and_then(Value::as_bool);
    if readable == Some(false) {
        return true;
    }
    song.get("AVAILABLE_COUNTRIES")
        .or_else(|| song.get("available_countries"))
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
}

impl ResolvedSource {
    fn from_remote(source: RemoteAudio) -> Self {
        Self {
            data: source.data,
            size: source.size,
            deezer_track_id: source.deezer_track_id,
            is_soundcloud: source.is_soundcloud,
            cache_identity: source.cache_identity,
            format: source.format,
            format_name: source.format_name,
            declared_bitrate: source.declared_bitrate,
        }
    }

    fn from_backend(source: BackendSource) -> Self {
        let metadata = source.metadata();
        Self {
            data: SourceData::Backend(source),
            size: metadata.size,
            deezer_track_id: metadata.deezer_track_id,
            is_soundcloud: metadata.provenance
                == super::media_source::BackendProvenance::SoundCloud,
            cache_identity: Some(metadata.cache_identity.as_str().to_owned()),
            format: metadata.format,
            format_name: metadata.format_name,
            declared_bitrate: metadata.declared_bitrate,
        }
    }
}

fn cache_key(source: &ResolvedSource) -> Option<String> {
    source.cache_identity.clone().or_else(|| {
        let SourceData::Remote(url) = &source.data else {
            return None;
        };
        Some(match source.deezer_track_id.as_deref() {
            Some(track_id) => format!("deezer:{track_id}:{}", source.format_name),
            None => format!("stream:{url}"),
        })
    })
}

#[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceVariant {
    Deezer,
    #[cfg(test)]
    BackendDeezer,
    SoundCloudOriginal,
    BackendSoundCloud,
    SoundCloudStandard,
}

impl SourceVariant {
    const fn label(self) -> &'static str {
        match self {
            Self::Deezer => "deezer",
            #[cfg(test)]
            Self::BackendDeezer => "murglar",
            Self::SoundCloudOriginal => "original",
            Self::BackendSoundCloud => "murglar",
            Self::SoundCloudStandard => "standard",
        }
    }
}

fn stable_cache_identity(
    provider: PlaybackProvider,
    track_id: &str,
    variant: SourceVariant,
    format: AudioFormat,
    format_name: &str,
) -> String {
    let provider = match provider {
        PlaybackProvider::Deezer => "deezer",
        PlaybackProvider::SoundCloud => "soundcloud",
    };
    let format_name = format_name.trim();
    let format_name = if format_name.is_empty() {
        format.label()
    } else {
        format_name
    };
    format!(
        "{provider}:{track_id}:{}:{}:{format_name}",
        variant.label(),
        format.extension()
    )
}

#[derive(Clone)]
enum SourceData {
    Remote(String),
    Hls(Box<HlsDescriptor>),
    Backend(BackendSource),
    Inline(Vec<u8>),
}

mod crypto;
mod format;
mod playback;
mod providers;
mod range;
mod range_seek;
mod source;

use crypto::{DeezerStripeStream, decrypt_stripes};
use format::{
    append_soundcloud_transcoding_query, audio_format, choose_soundcloud_original_format,
    declared_bitrate, infer_soundcloud_original_format, is_soundcloud_hls_transcoding,
    normalize_release_date, percent_decode_filename, read_bounded_response,
    sniff_soundcloud_original_format, soundcloud_format_from_media_headers, soundcloud_format_name,
    soundcloud_original_bitrate, soundcloud_original_head_can_fallback,
    soundcloud_playback_transcodings, soundcloud_track_authorization,
    soundcloud_transcoding_bitrate, soundcloud_transcodings, transcoding_format,
    validate_audio_output, validate_media_response_url, validate_progressive_prefix,
    validate_soundcloud_stream_url, validate_soundcloud_transcoding_url,
};
use range::{
    aligned_range, cacheable_size, content_range_total, inline_range, prefetch_range, ranged_body,
    read_response_range, trim_range, validate_download_range_response,
};

impl StreamResolver {
    pub(crate) fn new() -> Result<Self, String> {
        Self::new_with_limiter(ResolveLimiter::new())
    }

    pub(crate) fn new_with_limiter(limiter: ResolveLimiter) -> Result<Self, String> {
        Client::builder()
            .connect_timeout(Duration::from_secs(12))
            .timeout(Duration::from_secs(30))
            .https_only(true)
            .build()
            .map(|client| Self {
                media_backend: MediaBackend::new(client.clone()),
                client,
                cache: None,
                limiter,
                resolved_source_cache: ResolvedSourceCache::new(),
                source_resolve_flights: SourceResolveFlights::new(),
                #[cfg(test)]
                backend_resolve_override: None,
            })
            .map_err(|_| "The playback network client could not be initialized".into())
    }

    pub(crate) fn with_cache(mut self, cache: AudioCache) -> Self {
        self.cache = Some(cache);
        self
    }

    pub(crate) fn clear_resolved_source_cache(&self) {
        self.resolved_source_cache.clear();
        self.source_resolve_flights.clear();
    }

    fn insert_resolved_source_if_current(
        &self,
        expected_epoch: u64,
        cancellation: &CancellationToken,
        key: String,
        source: ResolvedSource,
    ) -> bool {
        if cancellation.is_cancelled() {
            return false;
        }
        self.resolved_source_cache
            .insert_if_epoch(expected_epoch, key, source)
    }

    fn resolved_source_cache_key(
        track: &PlaybackTrack,
        soundcloud_token_available: bool,
        backend_eligible: bool,
    ) -> Option<String> {
        if track.id.is_empty() {
            return None;
        }
        Some(match track.provider {
            PlaybackProvider::Deezer => format!("deezer:{}:murglar={backend_eligible}", track.id),
            PlaybackProvider::SoundCloud => format!(
                "soundcloud:{}:token={soundcloud_token_available}:downloadable={}:murglar={backend_eligible}",
                track.id, track.downloadable
            ),
        })
    }

    fn resolved_source_cache_key_for_source(
        track: &PlaybackTrack,
        soundcloud_token_available: bool,
        source: &ResolvedSource,
    ) -> Option<String> {
        Self::resolved_source_cache_key(track, soundcloud_token_available, source.uses_backend())
    }
}

fn deezer_headers(
    arl: Option<&DeezerArl>,
    cookies: Option<&str>,
) -> Result<header::HeaderMap, String> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static(BROWSER_USER_AGENT),
    );
    let mut cookie = match arl {
        Some(arl) => arl
            .cookie_header()
            .map_err(|error| error.message)?
            .to_str()
            .map_err(|_| "The saved Deezer session is invalid".to_string())?
            .to_owned(),
        None => String::new(),
    };
    if let Some(extra) = cookies.filter(|cookies| !cookies.is_empty()) {
        // The saved arl value cannot contain ';', so the first segment is
        // always the arl cookie and any remainder is the attached jar
        // snapshot. Repeated cookie names replace instead of duplicating,
        // and any arl set by the server is dropped, so the saved arl wins.
        let (arl_part, rest) = match cookie.split_once("; ") {
            Some((arl_part, rest)) => (arl_part, rest),
            None => (cookie.as_str(), ""),
        };
        let parts = merge_cookie_parts(rest, extra);
        cookie = if arl_part.is_empty() {
            parts
        } else if parts.is_empty() {
            arl_part.to_owned()
        } else {
            format!("{arl_part}; {parts}")
        };
    }
    if !cookie.is_empty() {
        let mut value = header::HeaderValue::from_str(&cookie)
            .map_err(|_| "The saved Deezer session is invalid".to_string())?;
        value.set_sensitive(true);
        headers.insert(header::COOKIE, value);
    }
    Ok(headers)
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

async fn response_json(response: Response, stage: &'static str) -> Result<Value, String> {
    let status = response.status();
    if !status.is_success() {
        crate::diagnostics::event(
            "WARN",
            format!("playback provider stage={stage} status={status} retry=false class=final"),
        );
        return Err(provider_rejection(stage, status));
    }
    response
        .json()
        .await
        .map_err(|_| format!("The {stage} provider returned an invalid playback response"))
}

fn provider_rejection(stage: &str, status: StatusCode) -> String {
    let reason = status.canonical_reason().unwrap_or("unknown status");
    let provider = if stage.starts_with("deezer") {
        "Deezer"
    } else if stage.starts_with("soundcloud") {
        "SoundCloud"
    } else if stage.starts_with("murglar") {
        "Murglar"
    } else {
        "The provider"
    };
    let request_kind = if stage.ends_with("session") {
        "session"
    } else if stage.ends_with("media") {
        "media API"
    } else if stage.ends_with("track") {
        "track"
    } else if stage.ends_with("original") || stage.ends_with("transcoding") {
        "source"
    } else {
        "playback"
    };
    format!(
        "{provider} {request_kind} request was rejected (HTTP {} {reason})",
        status.as_u16()
    )
}

fn request_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "The playback request timed out"
    } else {
        "The audio provider could not be reached"
    }
    .into()
}

fn remote_size_from_response(
    status: StatusCode,
    headers: &header::HeaderMap,
    content_length: Option<u64>,
) -> Option<u64> {
    content_range_total(headers).or_else(|| {
        (status == StatusCode::OK)
            .then_some(content_length)
            .flatten()
            .filter(|size| *size > 0)
    })
}

#[cfg(test)]
async fn cache_bytes(
    cache: &AudioCache,
    key: &str,
    total: u64,
    bytes: &[u8],
    generation: u64,
    cache_track_token: Option<&CacheTrackToken>,
) {
    let mut start = 0;
    while start < total {
        if !cache.is_current(generation) {
            return;
        }
        let end = start.saturating_add(BLOCK_SIZE - 1).min(total - 1);
        let start_index = start as usize;
        let end_index = end as usize + 1;
        let Some(block) = bytes.get(start_index..end_index) else {
            return;
        };
        let path = cache.block_path(key, total, start, end);
        cache
            .write_with_token(&path, block, generation, cache_track_token)
            .await;
        start = end + 1;
    }
}

async fn cache_writer_blocks(
    cache: &AudioCache,
    key: &str,
    total: u64,
    writer: &ProgressiveWriter,
    generation: u64,
    cache_track_token: Option<&CacheTrackToken>,
) -> bool {
    let mut start = 0;
    while start < total {
        if !cache.is_current(generation) || !cache.is_track_current(cache_track_token).await {
            return false;
        }
        let block_len = usize::try_from((total - start).min(BLOCK_SIZE)).unwrap_or(0);
        if block_len == 0 {
            return false;
        }
        let Ok(block) = writer.read_range(start, block_len).await else {
            return false;
        };
        if block.len() != block_len {
            return false;
        }
        let end = start + block_len as u64 - 1;
        let path = cache.block_path(key, total, start, end);
        cache
            .write_with_token(&path, &block, generation, cache_track_token)
            .await;
        start = end + 1;
    }
    cache.is_current(generation) && cache.is_track_current(cache_track_token).await
}

fn log_response_diagnostics(label: &str, status: StatusCode, content_length: Option<u64>) {
    crate::diagnostics::event(
        "INFO",
        response_diagnostics_message(label, status, content_length),
    );
}

fn response_diagnostics_message(
    label: &str,
    status: StatusCode,
    content_length: Option<u64>,
) -> String {
    format!("{label} status={status} content_length={content_length:?}")
}

fn is_audio_validation_error(error: &str) -> bool {
    error.starts_with("The resolved audio output") || error.starts_with("The resolved FLAC output")
}

fn deezer_format_preferences(strict_flac: bool) -> &'static [&'static str] {
    const LOSSLESS: &[&str] = &["FLAC"];
    const STANDARD: &[&str] = &["FLAC", "MP3_320", "MP3_128"];
    if strict_flac { LOSSLESS } else { STANDARD }
}

fn enforce_deezer_format(format: AudioFormat, strict_flac: bool) -> Result<AudioFormat, String> {
    if strict_flac && format != AudioFormat::Flac {
        crate::diagnostics::event(
            "ERROR",
            format!(
                "deezer strict recovery rejected format={} expected=FLAC",
                format.label()
            ),
        );
        return Err("Deezer did not return a FLAC source for strict lossless playback".into());
    }
    Ok(format)
}

fn playback_source_label(source: &ResolvedSource) -> &'static str {
    if source.is_soundcloud {
        "soundcloud"
    } else if source.deezer_track_id.is_some() {
        "deezer"
    } else {
        "remote"
    }
}

fn download_choice_text(variant: DownloadVariant, source: &ResolvedSource) -> (String, String) {
    let format = source.format_name.trim();
    let bitrate = source.declared_bitrate;
    let display_format = match (source.format, bitrate) {
        (AudioFormat::Flac, _) => "FLAC".to_owned(),
        (AudioFormat::Mp3, Some(bitrate)) => format!("MP3 {bitrate} kbps"),
        _ if !format.is_empty() => format.to_owned(),
        _ => source.format.label().to_owned(),
    };
    let detail = if source.is_soundcloud {
        match variant {
            DownloadVariant::Original => "Original quality",
            DownloadVariant::Murglar if source.format == AudioFormat::Flac => "Lossless quality",
            DownloadVariant::Murglar => "High quality",
            DownloadVariant::Standard => "Standard quality",
            _ => variant.download_detail(),
        }
    } else {
        variant.download_detail()
    };
    (display_format, detail.to_owned())
}

fn is_expected_capability_absence(variant: DownloadVariant, error: &str) -> bool {
    match variant {
        DownloadVariant::Original => matches!(
            error,
            "SoundCloud did not return an original download URL"
                | "SoundCloud source request was rejected (HTTP 403 Forbidden)"
                | "SoundCloud source request was rejected (HTTP 404 Not Found)"
                | SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN
        ),
        DownloadVariant::Standard => matches!(
            error,
            "SoundCloud did not return audio transcodings"
                | "No active stream link found for this SoundCloud track"
        ),
        DownloadVariant::Murglar => error == "Murglar returned no HQ source",
        DownloadVariant::DeezerFlac
        | DownloadVariant::DeezerMp3_320
        | DownloadVariant::DeezerMp3_128 => matches!(
            error,
            DEEZER_UNAVAILABLE
                | "No Deezer direct source available without an ARL"
                | "Deezer login is required for playback"
        ),
        DownloadVariant::Best => false,
    }
}

fn is_soundcloud_candidate_absence(error: &str) -> bool {
    matches!(
        error,
        "SoundCloud source request was rejected (HTTP 403 Forbidden)"
            | "SoundCloud source request was rejected (HTTP 404 Not Found)"
    )
}

fn has_unexpected_capability_failure(failures: &[(DownloadVariant, String)]) -> bool {
    failures
        .iter()
        .any(|(variant, error)| !is_expected_capability_absence(*variant, error))
}

fn should_skip_direct_deezer_capability(
    include_remote_size: bool,
    deezer_arl: Option<&DeezerArl>,
) -> bool {
    !include_remote_size && deezer_arl.is_none()
}

fn should_fallback_to_direct_deezer(has_direct_fallback: bool, error: &str) -> bool {
    has_direct_fallback && is_audio_validation_error(error)
}

fn should_recover_timeline_startup_error(
    timeline_startup: bool,
    murglar_fallback: bool,
    cancelled: bool,
) -> bool {
    timeline_startup && murglar_fallback && !cancelled
}

#[cfg(test)]
mod backend_tests;

#[cfg(test)]
mod tests;
