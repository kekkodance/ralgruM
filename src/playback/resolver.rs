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
    search::{DeezerArl, SOUNDCLOUD_CLIENT_ID, SoundCloudToken},
};

use super::cache::{AudioCache, BLOCK_SIZE, CacheTrackToken};
use super::listen_history::SoundCloudListenReport;
use super::media_source::{BackendProvider, BackendSource, MediaRequest, MediaResolveOutcome};
use super::progressive::{
    ProgressiveFile, ProgressiveReader, ProgressiveWriter, TimelineSeekSession, startup_bytes,
};
use super::resolve_limiter::ResolveLimiter;
use super::resolve_source_cache::{ResolvedSourceCache, SourceCacheValue};
use super::retry::{RequestClass, RetryBudget, RetryClass, classify_status, send_with_retry};
use super::soundcloud_hls::{self, HlsDescriptor};
use super::{
    DownloadChoice, DownloadVariant, PlaybackProvider, PlaybackTrack,
    deezer_collection_download_choices, selection_order,
};

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
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
    pub(crate) seekable_after_completion: bool,
    pub(crate) timeline_seek_session: Option<Arc<dyn TimelineSeekSession>>,
    worker: Option<ProgressiveDownload>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceVariant {
    Deezer,
    BackendDeezer,
    SoundCloudOriginal,
    BackendSoundCloud,
    SoundCloudStandard,
}

impl SourceVariant {
    const fn label(self) -> &'static str {
        match self {
            Self::Deezer => "deezer",
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
    Hls(HlsDescriptor),
    Backend(BackendSource),
    Inline(Vec<u8>),
}

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

    /// Resolve a source for playback, reusing the short-lived remote URL
    /// cache for playback and seamless prefetches. Downloads and metadata
    /// probes deliberately call `resolve_source` directly so they never
    /// populate this cache.
    async fn resolve_playback_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        backend: Option<MediaCredentials>,
        cancellation: CancellationToken,
        refresh: bool,
        expected_cache_epoch: u64,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() || self.resolved_source_cache.epoch() != expected_cache_epoch
        {
            return Err("Playback request cancelled".into());
        }
        let soundcloud_token_available = soundcloud_token.is_some();
        let key =
            Self::resolved_source_cache_key(track, soundcloud_token_available, backend.is_some());
        if refresh {
            if let Some(key) = key.as_deref() {
                self.resolved_source_cache
                    .invalidate_if_epoch(expected_cache_epoch, key);
            }
        } else if let Some(source) = key
            .as_deref()
            .and_then(|key| self.resolved_source_cache.get(key))
        {
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            if backend.is_some() && !source.uses_backend() {
                if let Some(key) = key.as_deref() {
                    self.resolved_source_cache
                        .invalidate_if_epoch(expected_cache_epoch, key);
                }
            } else {
                return Ok(source);
            }
        }
        if !refresh && backend.is_some() {
            let direct_key =
                Self::resolved_source_cache_key(track, soundcloud_token_available, false);
            if let Some(source) = direct_key
                .as_deref()
                .and_then(|key| self.resolved_source_cache.get(key))
            {
                if !source.uses_backend() {
                    if cancellation.is_cancelled()
                        || self.resolved_source_cache.epoch() != expected_cache_epoch
                    {
                        return Err("Playback request cancelled".into());
                    }
                    return Ok(source);
                }
                if let Some(direct_key) = direct_key.as_deref() {
                    self.resolved_source_cache
                        .invalidate_if_epoch(expected_cache_epoch, direct_key);
                }
            }
        }

        // Mirror the original app's shared resolve reservation and provider
        // spacing only when a fresh provider resolve is needed. A valid cache
        // hit must not spend resolve budget or touch the provider.
        self.limiter.reserve(&cancellation).await?;
        let source = self
            .resolve_source(
                track,
                deezer_arl,
                soundcloud_token,
                backend,
                cancellation.clone(),
                false,
            )
            .await?;
        if let Some(key) =
            Self::resolved_source_cache_key_for_source(track, soundcloud_token_available, &source)
        {
            self.insert_resolved_source_if_current(
                expected_cache_epoch,
                &cancellation,
                key,
                source.clone(),
            );
        }
        Ok(source)
    }

    pub(crate) async fn resolve(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<ResolvedAudio, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let murglar_fallback =
            track.provider == PlaybackProvider::Deezer && murglar.is_some() && deezer_arl.is_some();
        let source_generation = self.cache.as_ref().map(AudioCache::generation);
        let cache_track_token = match self.cache.as_ref() {
            Some(cache) => Some(cache.track_token(track.provider, &track.id).await),
            None => None,
        };
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                cancellation.clone(),
                false,
                source_cache_epoch,
            )
            .await?;
        let mut format = source.format;
        let mut declared_bitrate = source.declared_bitrate;
        let file = tempfile::Builder::new()
            .prefix("ralgrum-playback-")
            .suffix(&format!(".{}", format.extension()))
            .tempfile()
            .map_err(|_| "A temporary playback file could not be created".to_string())?;
        let path = file.path().to_owned();
        let mut source_budget = RetryBudget::default();
        loop {
            if cancellation.is_cancelled()
                || source_generation.is_some_and(|generation| {
                    self.cache
                        .as_ref()
                        .is_some_and(|cache| !cache.is_current(generation))
                })
            {
                return Err("Playback request cancelled".into());
            }
            let mut output = File::from_std(
                file.reopen()
                    .map_err(|_| "The playback buffer could not be opened".to_string())?,
            );
            output
                .set_len(0)
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            output
                .seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let result = self
                .download_playback(
                    source,
                    &mut output,
                    &cancellation,
                    progress.clone(),
                    Some(track),
                    cache_track_token.as_ref(),
                )
                .await;
            match result {
                Ok(()) => break,
                Err(error) if error.refresh_source && source_budget.take_source_refresh() => {
                    if cancellation.is_cancelled() {
                        return Err("Playback request cancelled".into());
                    }
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback source refresh track_id={} reason=expired_media budget=1",
                            track.id
                        ),
                    );
                    source = self
                        .resolve_playback_source(
                            track,
                            deezer_arl.clone(),
                            soundcloud_token.clone(),
                            murglar.clone(),
                            cancellation.clone(),
                            true,
                            source_cache_epoch,
                        )
                        .await?;
                    format = source.format;
                    declared_bitrate = source.declared_bitrate;
                }
                Err(error) => return Err(error.message),
            }
        }
        if let Err(error) = validate_audio_output(&path, format).await {
            if !should_fallback_to_direct_deezer(murglar_fallback, &error) {
                return Err(error);
            }
            crate::diagnostics::event(
                "WARN",
                format!("deezer recovery mode=playback source=direct reason={error}"),
            );
            if let Some(key) = Self::resolved_source_cache_key(
                track,
                soundcloud_token.is_some(),
                murglar.is_some(),
            ) {
                self.resolved_source_cache
                    .invalidate_if_epoch(source_cache_epoch, &key);
            }
            let direct = self
                .resolve_deezer(&track.id, deezer_arl.as_ref(), false, &cancellation, false)
                .await?;
            let mut retry_output = File::from_std(
                file.reopen()
                    .map_err(|_| "The playback buffer could not be reopened".to_string())?,
            );
            retry_output
                .set_len(0)
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            retry_output
                .seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let retry_format = direct.format;
            let retry_declared_bitrate = direct.declared_bitrate;
            let direct = ResolvedSource {
                data: direct.data,
                size: direct.size,
                deezer_track_id: direct.deezer_track_id,
                is_soundcloud: direct.is_soundcloud,
                cache_identity: direct.cache_identity,
                format: direct.format,
                format_name: direct.format_name,
                declared_bitrate: direct.declared_bitrate,
            };
            if let Some(key) = Self::resolved_source_cache_key_for_source(
                track,
                soundcloud_token.is_some(),
                &direct,
            ) {
                self.insert_resolved_source_if_current(
                    source_cache_epoch,
                    &cancellation,
                    key,
                    direct.clone(),
                );
            }
            self.download_playback(
                direct,
                &mut retry_output,
                &cancellation,
                progress,
                Some(track),
                cache_track_token.as_ref(),
            )
            .await
            .map_err(|error| error.message)?;
            validate_audio_output(&path, retry_format).await?;
            format = retry_format;
            declared_bitrate = retry_declared_bitrate;
        }
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        Ok(ResolvedAudio {
            path,
            file,
            duration: (!track.duration.is_zero()).then_some(track.duration),
            format,
            declared_bitrate,
        })
    }

    pub(crate) async fn resolve_progressive(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<ResolvedProgressiveAudio, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let cache_track_token = match self.cache.as_ref() {
            Some(cache) => Some(cache.track_token(track.provider, &track.id).await),
            None => None,
        };
        let murglar_fallback =
            track.provider == PlaybackProvider::Deezer && murglar.is_some() && deezer_arl.is_some();
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                cancellation.clone(),
                false,
                source_cache_epoch,
            )
            .await?;
        if source.size == 0
            && let (Some(cache), Some(key)) = (self.cache.as_ref(), cache_key(&source))
            && let Some(total) = cache.known_total(&key).await
        {
            source.size = total;
        }
        let initially_fully_cached = match (self.cache.as_ref(), cache_key(&source)) {
            (Some(cache), Some(key)) if source.size > 0 && source.size <= cache.max_bytes() => {
                cache.is_fully_cached(&key, source.size).await
            }
            _ => false,
        };
        if initially_fully_cached
            && let (Some(cache), Some(key)) = (self.cache.as_ref(), cache_key(&source))
        {
            cache
                .remember_track_with_token(
                    track,
                    &key,
                    (source.size > 0).then_some(source.size),
                    cache_track_token.as_ref(),
                )
                .await;
        }
        let mut fully_cached = initially_fully_cached;
        let buffer = ProgressiveFile::new(source.format, (source.size > 0).then_some(source.size))
            .map_err(|_| "A temporary playback file could not be created".to_string())?;
        let mut writer = buffer
            .writer()
            .map_err(|_| "The playback buffer could not be opened".to_string())?;
        let mut reader = buffer
            .reader()
            .map_err(|_| "The playback buffer could not be opened".to_string())?;
        let playback_exposed = Arc::new(tokio::sync::Mutex::new(false));
        let mut worker_cancellation = cancellation.child_token();
        let mut task = self.spawn_progressive_download(
            track.clone(),
            deezer_arl.clone(),
            soundcloud_token.clone(),
            murglar.clone(),
            source.clone(),
            worker_cancellation.clone(),
            progress.clone(),
            writer,
            playback_exposed.clone(),
            cache_track_token.clone(),
            source_cache_epoch,
        );
        let timeline_startup = matches!(&source.data, SourceData::Hls(_))
            || matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
        let minimum = startup_bytes(source.format, (source.size > 0).then_some(source.size));
        let wait = tokio::task::spawn_blocking(move || {
            let result = if timeline_startup {
                reader.wait_until_startup_ready()
            } else {
                reader.wait_until_ready(minimum)
            };
            (reader, result)
        })
        .await;
        let (returned_reader, ready) = match wait {
            Ok(result) => result,
            Err(_) => {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err("The playback worker stopped unexpectedly".into());
            }
        };
        reader = returned_reader;
        let startup_error = match ready {
            Ok(()) => None,
            Err(error)
                if should_recover_timeline_startup_error(
                    timeline_startup,
                    murglar_fallback,
                    cancellation.is_cancelled() || worker_cancellation.is_cancelled(),
                ) =>
            {
                Some(error)
            }
            Err(error) => {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        };

        let (mut format, mut declared_bitrate) = buffer
            .metadata()
            .unwrap_or((source.format, source.declared_bitrate));
        let metadata_minimum = startup_bytes(format, reader.total());
        if !timeline_startup && metadata_minimum > minimum {
            let wait = tokio::task::spawn_blocking(move || {
                let result = reader.wait_until_ready(metadata_minimum);
                (reader, result)
            })
            .await;
            let (returned_reader, ready) = match wait {
                Ok(result) => result,
                Err(_) => {
                    worker_cancellation.cancel();
                    let _ = task.await;
                    return Err("The playback worker stopped unexpectedly".into());
                }
            };
            reader = returned_reader;
            if let Err(error) = ready {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        }
        let prefix_error =
            startup_error.or_else(|| validate_progressive_prefix(buffer.path(), format).err());
        if let Some(error) = prefix_error {
            if !murglar_fallback {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
            worker_cancellation.cancel();
            let (returned_writer, _) = task
                .await
                .map_err(|_| "The playback worker stopped unexpectedly".to_string())?;
            writer = returned_writer;
            if let Some(key) = Self::resolved_source_cache_key(
                track,
                soundcloud_token.is_some(),
                murglar.is_some(),
            ) {
                self.resolved_source_cache
                    .invalidate_if_epoch(source_cache_epoch, &key);
            }
            let direct = self
                .resolve_deezer(&track.id, deezer_arl.as_ref(), false, &cancellation, false)
                .await?;
            source = ResolvedSource {
                data: direct.data,
                size: direct.size,
                deezer_track_id: direct.deezer_track_id,
                is_soundcloud: direct.is_soundcloud,
                cache_identity: direct.cache_identity,
                format: direct.format,
                format_name: direct.format_name,
                declared_bitrate: direct.declared_bitrate,
            };
            if let Some(key) = Self::resolved_source_cache_key_for_source(
                track,
                soundcloud_token.is_some(),
                &source,
            ) {
                self.insert_resolved_source_if_current(
                    source_cache_epoch,
                    &cancellation,
                    key,
                    source.clone(),
                );
            }
            fully_cached = false;
            format = source.format;
            declared_bitrate = source.declared_bitrate;
            writer
                .reset((source.size > 0).then_some(source.size))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let minimum = startup_bytes(source.format, (source.size > 0).then_some(source.size));
            task = self.spawn_progressive_download(
                track.clone(),
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                source.clone(),
                {
                    worker_cancellation = cancellation.child_token();
                    worker_cancellation.clone()
                },
                progress.clone(),
                writer,
                playback_exposed.clone(),
                cache_track_token.clone(),
                source_cache_epoch,
            );
            let wait = tokio::task::spawn_blocking(move || {
                let result = reader.wait_until_ready(minimum);
                (reader, result)
            })
            .await;
            let (returned_reader, ready) = match wait {
                Ok(result) => result,
                Err(_) => {
                    worker_cancellation.cancel();
                    let _ = task.await;
                    return Err("The playback worker stopped unexpectedly".into());
                }
            };
            reader = returned_reader;
            if let Err(error) = ready {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
            if let Err(error) = validate_progressive_prefix(buffer.path(), format) {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        }

        let total = reader
            .total()
            .or_else(|| (source.size > 0).then_some(source.size));
        if let Some((metadata_format, metadata_bitrate)) = buffer.metadata() {
            format = metadata_format;
            declared_bitrate = metadata_bitrate;
        }
        fully_cached = if fully_cached && total == Some(source.size) {
            match (self.cache.as_ref(), cache_key(&source)) {
                (Some(cache), Some(key)) => cache.is_fully_cached(&key, source.size).await,
                _ => false,
            }
        } else {
            false
        };
        let initial_downloaded = if fully_cached {
            total.unwrap_or_default()
        } else {
            reader.written()
        };
        let initial_buffered_fraction = match &source.data {
            SourceData::Hls(descriptor) => descriptor.buffered_fraction(1),
            SourceData::Backend(source) => source.metadata().initial_buffered_fraction,
            SourceData::Remote(_) | SourceData::Inline(_) => None,
        };
        let timeline_seek_session: Option<Arc<dyn TimelineSeekSession>> = match &source.data {
            SourceData::Hls(descriptor) => Some(Arc::new(soundcloud_hls::HlsSeekSession::new(
                self.client.clone(),
                descriptor.clone(),
                tokio::runtime::Handle::current(),
                cancellation.clone(),
                MAX_AUDIO_SIZE,
                BROWSER_USER_AGENT,
            ))),
            SourceData::Backend(source) => source.timeline_seek_session(cancellation.clone()),
            SourceData::Remote(_) | SourceData::Inline(_) => None,
        };
        if fully_cached && let (Some(progress), Some(total)) = (&progress, total) {
            progress(ProgressUpdate::bytes(total, Some(total)));
        }
        *playback_exposed.lock().await = true;
        let worker = ProgressiveDownload {
            cancellation: Some(worker_cancellation),
            task: Some(task),
        };
        let duration = (!track.duration.is_zero())
            .then_some(track.duration)
            .or_else(|| match &source.data {
                SourceData::Hls(descriptor) => descriptor.duration(),
                SourceData::Backend(source) => source.metadata().duration,
                SourceData::Remote(_) | SourceData::Inline(_) => None,
            });
        Ok(ResolvedProgressiveAudio {
            reader,
            file: buffer.into_file(),
            duration,
            format,
            total,
            timeline_size_unknown: source.timeline_size_unknown(),
            declared_bitrate,
            initial_downloaded,
            initial_buffered_fraction,
            fully_cached,
            seekable_after_completion: timeline_startup,
            timeline_seek_session,
            worker: Some(worker),
        })
    }

    fn spawn_progressive_download(
        &self,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        source: ResolvedSource,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
        writer: ProgressiveWriter,
        playback_exposed: Arc<tokio::sync::Mutex<bool>>,
        cache_track_token: Option<CacheTrackToken>,
        source_cache_epoch: u64,
    ) -> tokio::task::JoinHandle<(ProgressiveWriter, Result<(), String>)> {
        let resolver = self.clone();
        tokio::spawn(async move {
            resolver
                .download_progressive(
                    track,
                    deezer_arl,
                    soundcloud_token,
                    murglar,
                    source,
                    cancellation,
                    progress,
                    writer,
                    playback_exposed,
                    cache_track_token,
                    source_cache_epoch,
                )
                .await
        })
    }

    async fn download_progressive(
        &self,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        mut source: ResolvedSource,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
        mut writer: ProgressiveWriter,
        playback_exposed: Arc<tokio::sync::Mutex<bool>>,
        cache_track_token: Option<CacheTrackToken>,
        source_cache_epoch: u64,
    ) -> (ProgressiveWriter, Result<(), String>) {
        let source_generation = self.cache.as_ref().map(AudioCache::generation);
        let mut source_budget = RetryBudget::default();
        let mut dash_fallback_attempted = false;
        loop {
            if cancellation.is_cancelled()
                || source_generation.is_some_and(|generation| {
                    self.cache
                        .as_ref()
                        .is_some_and(|cache| !cache.is_current(generation))
                })
            {
                writer.cancel();
                return (writer, Err("Playback request cancelled".into()));
            }
            let exposure_guard = playback_exposed.lock().await;
            if *exposure_guard {
                let message = "The playback source expired after playback started".to_string();
                writer.fail(message.clone());
                return (writer, Err(message));
            }
            writer.set_metadata(source.format, source.declared_bitrate);
            if let Err(error) = writer.reset((source.size > 0).then_some(source.size)).await {
                let message = format!("The playback buffer could not be reset: {error}");
                writer.fail(message.clone());
                return (writer, Err(message));
            }
            drop(exposure_guard);
            let source_is_timeline = matches!(&source.data, SourceData::Hls(_))
                || matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
            let source_is_backend_timeline =
                matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
            let unknown_source = source.size == 0 || source_is_timeline;
            let cache_source = unknown_source.then(|| source.clone());
            match self
                .download_playback(
                    source,
                    &mut writer,
                    &cancellation,
                    progress.clone(),
                    Some(&track),
                    cache_track_token.as_ref(),
                )
                .await
            {
                Ok(()) => {
                    if let Err(error) = writer.finish().await {
                        let message =
                            format!("The playback buffer could not be finalized: {error}");
                        writer.fail(message.clone());
                        return (writer, Err(message));
                    }
                    if writer.is_complete()
                        && let Some(progress) = &progress
                        && let Some(total) = writer.total()
                    {
                        progress(ProgressUpdate::complete(total));
                    }
                    if let Some(cache_source) = cache_source.as_ref() {
                        self.persist_unknown_progressive_cache(
                            &track,
                            &writer,
                            cache_source,
                            source_generation,
                            cache_track_token.as_ref(),
                        )
                        .await;
                    }
                    return (writer, Ok(()));
                }
                Err(error) if error.refresh_source && source_budget.take_source_refresh() => {
                    if cancellation.is_cancelled() {
                        writer.cancel();
                        return (writer, Err("Playback request cancelled".into()));
                    }
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback source refresh track_id={} reason=expired_media budget=1",
                            track.id
                        ),
                    );
                    match self
                        .resolve_playback_source(
                            &track,
                            deezer_arl.clone(),
                            soundcloud_token.clone(),
                            murglar.clone(),
                            cancellation.clone(),
                            true,
                            source_cache_epoch,
                        )
                        .await
                    {
                        Ok(next) => source = next,
                        Err(message) => {
                            if message == "Playback request cancelled" {
                                writer.cancel();
                            } else {
                                writer.fail(message.clone());
                            }
                            return (writer, Err(message));
                        }
                    }
                }
                Err(error) => {
                    let message = error.message;
                    if message == "Playback request cancelled" {
                        writer.cancel();
                        return (writer, Err(message));
                    }
                    let can_fallback_to_direct = !dash_fallback_attempted
                        && source_is_backend_timeline
                        && track.provider == PlaybackProvider::Deezer
                        && deezer_arl.is_some()
                        && murglar.is_some()
                        && !*playback_exposed.lock().await;
                    if can_fallback_to_direct {
                        dash_fallback_attempted = true;
                        if let Some(key) = Self::resolved_source_cache_key(
                            &track,
                            soundcloud_token.is_some(),
                            true,
                        ) {
                            self.resolved_source_cache
                                .invalidate_if_epoch(source_cache_epoch, &key);
                        }
                        match self
                            .resolve_deezer(
                                &track.id,
                                deezer_arl.as_ref(),
                                false,
                                &cancellation,
                                false,
                            )
                            .await
                        {
                            Ok(next) => {
                                source = ResolvedSource::from_remote(next);
                                continue;
                            }
                            Err(fallback_error) => {
                                writer.fail(fallback_error.clone());
                                return (writer, Err(fallback_error));
                            }
                        }
                    }
                    writer.fail(message.clone());
                    return (writer, Err(message));
                }
            }
        }
    }

    async fn persist_unknown_progressive_cache(
        &self,
        track: &PlaybackTrack,
        writer: &ProgressiveWriter,
        source: &ResolvedSource,
        source_generation: Option<u64>,
        cache_track_token: Option<&CacheTrackToken>,
    ) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let Some(key) = cache_key(source) else {
            return;
        };
        let Some(total) = writer
            .total()
            .filter(|total| cacheable_size(*total, cache.max_bytes()).is_some())
        else {
            return;
        };
        let generation = source_generation.unwrap_or_else(|| cache.generation());
        if !cache.is_current(generation) || !cache.is_track_current(cache_track_token).await {
            return;
        }
        if !cache_writer_blocks(cache, &key, total, writer, generation, cache_track_token).await {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "playback cache source={} skipped_unknown_size total={}",
                    playback_source_label(source),
                    total
                ),
            );
            return;
        }
        cache
            .remember_total_with_token(&key, total, generation, cache_track_token)
            .await;
        if cache.is_fully_cached(&key, total).await {
            cache
                .remember_track_with_token(track, &key, Some(total), cache_track_token)
                .await;
        }
        let _ = cache.prune().await;
    }

    async fn fetch_cached_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
        total: u64,
        deezer_track_id: Option<&str>,
        is_soundcloud: bool,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, PlaybackDownloadError> {
        let (request_start, request_end) = aligned_range(start, end, total);
        let response = send_with_retry(
            "media.cache_range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(
                        header::RANGE,
                        format!("bytes={request_start}-{request_end}"),
                    )
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await?;
        validate_media_response_url(response.url(), is_soundcloud)
            .map_err(PlaybackDownloadError::message)?;
        if !response.status().is_success() {
            return Err(PlaybackDownloadError::media_status(
                "audio provider",
                response.status(),
            ));
        }
        let (status, content_length, body) =
            read_response_range(response, request_start, request_end).await?;
        log_response_diagnostics("playback cache range", status, content_length);
        let mut bytes = body;
        if let Some(track_id) = deezer_track_id {
            decrypt_stripes(&mut bytes, track_id, request_start / STRIPE_SIZE as u64)?;
        }
        Ok(trim_range(bytes, start, end, request_start)?)
    }

    async fn download_playback<W>(
        &self,
        source: ResolvedSource,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
        catalog_track: Option<&PlaybackTrack>,
        cache_track_token: Option<&CacheTrackToken>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        let source_label = playback_source_label(&source);
        let decryption = if source.deezer_track_id.is_some() {
            "deezer_stripes"
        } else {
            "none"
        };
        let Some(cache) = self.cache.as_ref() else {
            return self
                .download_source_inner(source, output, cancellation, progress)
                .await;
        };
        let cache_generation = cache.generation();
        let Some(size) = cacheable_size(source.size, cache.max_bytes()) else {
            return self
                .download_source_inner(source, output, cancellation, progress)
                .await;
        };
        let Some(key) = cache_key(&source) else {
            return self
                .download_source_inner(source, output, cancellation, progress)
                .await;
        };
        if matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline)
            && !cache.is_fully_cached(&key, size).await
        {
            return self
                .download_source_inner(source, output, cancellation, progress)
                .await;
        }
        cache.promote_prefetch(&key, size).await;
        let remote_url = match &source.data {
            SourceData::Remote(url) => Some(url.as_str()),
            SourceData::Backend(_) | SourceData::Hls(_) | SourceData::Inline(_) => None,
        };
        crate::diagnostics::event(
            "INFO",
            format!(
                "playback cache source={source_label} generation={cache_generation} format={} decryption={decryption}",
                source.format.label()
            ),
        );
        output.set_total_hint(Some(size));
        let mut downloaded = 0_u64;
        let mut start = 0;
        while start < size {
            if cancellation.is_cancelled() || !cache.is_current(cache_generation) {
                return Err(PlaybackDownloadError::message("Playback request cancelled"));
            }
            let end = start.saturating_add(BLOCK_SIZE - 1).min(size - 1);
            let path = cache.block_path(&key, size, start, end);
            let expected = (end - start + 1) as usize;
            let bytes = if let Some(bytes) = cache.read(&path, expected).await {
                bytes
            } else {
                let _guard = cache.lock_for(&path).await;
                if let Some(bytes) = cache.read(&path, expected).await {
                    bytes
                } else if let SourceData::Inline(bytes) = &source.data {
                    let bytes = inline_range(bytes, start, end)?;
                    if bytes.len() != expected {
                        return Err(
                            "The inline source did not contain a complete cache block".into()
                        );
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                } else if start == 0
                    && let SourceData::Backend(backend) = &source.data
                    && let Some(prefix_len) = output
                        .progressive_startup_bytes(source.format, Some(size))
                        .filter(|prefix_len| *prefix_len > 0 && *prefix_len < expected as u64)
                {
                    let prefix_end = prefix_len - 1;
                    let prefix = backend.read_range(0, prefix_end, cancellation).await?;
                    if prefix.len() != prefix_len as usize {
                        return Err(
                            "The backend source returned an incomplete startup prefix".into()
                        );
                    }
                    self.write_progressive_chunks(
                        output,
                        &prefix,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;

                    let rest_start = prefix_len;
                    let rest = backend.read_range(rest_start, end, cancellation).await?;
                    let expected_rest = expected - prefix.len();
                    if rest.len() != expected_rest {
                        return Err("The backend source returned an incomplete cache block".into());
                    }
                    let mut block = prefix;
                    block.extend_from_slice(&rest);
                    cache
                        .write_with_token(&path, &block, cache_generation, cache_track_token)
                        .await;

                    self.write_progressive_chunks(
                        output,
                        &rest,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;
                    start = end + 1;
                    continue;
                } else if let SourceData::Backend(backend) = &source.data {
                    let bytes = backend.read_range(start, end, cancellation).await?;
                    if bytes.len() != expected {
                        return Err("The backend source returned an incomplete cache block".into());
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                } else if start == 0
                    && let Some(url) = remote_url
                    && let Some(prefix_len) = output
                        .progressive_startup_bytes(source.format, Some(size))
                        .filter(|prefix_len| *prefix_len > 0 && *prefix_len < expected as u64)
                {
                    let prefix_end = prefix_len - 1;
                    let prefix = self
                        .fetch_cached_range(
                            url,
                            0,
                            prefix_end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    if prefix.len() != prefix_len as usize {
                        return Err(
                            "The audio provider returned an incomplete startup prefix".into()
                        );
                    }
                    self.write_progressive_chunks(
                        output,
                        &prefix,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;

                    let rest_start = prefix_len;
                    let rest = self
                        .fetch_cached_range(
                            url,
                            rest_start,
                            end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    let expected_rest = expected - prefix.len();
                    if rest.len() != expected_rest {
                        return Err("The audio provider returned an incomplete cache block".into());
                    }
                    let mut block = prefix;
                    block.extend_from_slice(&rest);
                    cache
                        .write_with_token(&path, &block, cache_generation, cache_track_token)
                        .await;

                    self.write_progressive_chunks(
                        output,
                        &rest,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;
                    start = end + 1;
                    continue;
                } else {
                    let Some(url) = remote_url else {
                        return Err("The remote source URL was unavailable".into());
                    };
                    let bytes = self
                        .fetch_cached_range(
                            url,
                            start,
                            end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    if bytes.len() != expected {
                        return Err("The audio provider returned an incomplete cache block".into());
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                }
            };
            self.write_progressive_chunks(
                output,
                &bytes,
                &mut downloaded,
                Some(size),
                cancellation,
                progress.as_ref(),
            )
            .await?;
            start = end + 1;
        }
        if let Some(track) = catalog_track
            && cache.is_fully_cached(&key, size).await
        {
            cache
                .remember_track_with_token(track, &key, Some(size), cache_track_token)
                .await;
        }
        if !cancellation.is_cancelled() && cache.is_current(cache_generation) {
            let _ = cache.prune().await;
        }
        if cancellation.is_cancelled() || !cache.is_current(cache_generation) {
            return Err(PlaybackDownloadError::message("Playback request cancelled"));
        }
        // Timeline startup normally comes from the backend downloader. A full
        // cache hit bypasses it, so publish readiness only after every byte has
        // been written and flushed successfully.
        output.mark_progressive_startup_ready();
        Ok(())
    }

    async fn write_progressive_chunks<W>(
        &self,
        output: &mut W,
        bytes: &[u8],
        downloaded: &mut u64,
        total: Option<u64>,
        cancellation: &CancellationToken,
        progress: Option<&ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput + ?Sized,
    {
        for chunk in bytes.chunks(PROGRESSIVE_WRITE_CHUNK_SIZE) {
            if cancellation.is_cancelled() {
                return Err(PlaybackDownloadError::message("Playback request cancelled"));
            }
            output.write_all(chunk).await.map_err(|_| {
                PlaybackDownloadError::message("The playback buffer could not be written")
            })?;
            output.flush().await.map_err(|_| {
                PlaybackDownloadError::message("The playback buffer could not be finalized")
            })?;
            *downloaded = downloaded.saturating_add(chunk.len() as u64);
            if let Some(progress) = progress {
                progress(ProgressUpdate::bytes(*downloaded, total));
            }
        }
        Ok(())
    }

    pub(crate) async fn prefetch(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        generation: u64,
    ) -> Result<(), String> {
        let Some(cache) = self.cache.as_ref() else {
            return Ok(());
        };
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let _permit = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            permit = cache.permit() => permit,
        };
        let Some(_permit) = _permit else {
            return Ok(());
        };
        if !cache.is_current(generation) {
            return Ok(());
        }
        let cache_track_token = cache.track_token(track.provider, &track.id).await;
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl,
                soundcloud_token,
                murglar,
                cancellation.clone(),
                false,
                source_cache_epoch,
            )
            .await?;
        let Some(key) = cache_key(&source) else {
            return Ok(());
        };
        if source.size == 0
            && let Some(total) = cache.known_total(&key).await
        {
            source.size = total;
        }
        if source.size == 0 {
            let job_key = AudioCache::job_key(&key, 0);
            if !cache.begin_job(job_key.clone()).await {
                return Ok(());
            }
            let result = self
                .prefetch_unknown_source(
                    cache,
                    &key,
                    &source,
                    &cancellation,
                    generation,
                    &cache_track_token,
                )
                .await;
            cache.finish_job(&job_key).await;
            if result.is_ok() && cache.is_current(generation) {
                let _ = cache.prune().await;
            }
            return result;
        }
        let source_label = playback_source_label(&source);
        let decryption = if source.deezer_track_id.is_some() {
            "deezer_stripes"
        } else {
            "none"
        };
        let Some(size) = cacheable_size(source.size, cache.max_bytes()) else {
            return Ok(());
        };
        let Some((first_start, first_end)) = prefetch_range(size) else {
            return Ok(());
        };
        let first_path = cache.block_path(&key, size, first_start, first_end);
        let first_expected = (first_end - first_start + 1) as usize;
        if cache.read(&first_path, first_expected).await.is_none()
            && !cache
                .mark_prefetch(&key, size, generation, Some(&cache_track_token))
                .await
        {
            return Ok(());
        }
        crate::diagnostics::event(
            "INFO",
            format!(
                "playback prefetch cache source={source_label} generation={generation} format={} decryption={decryption}",
                source.format.label()
            ),
        );
        let job_key = AudioCache::job_key(&key, size);
        if !cache.begin_job(job_key.clone()).await {
            return Ok(());
        }
        let result = self
            .prefetch_blocks(
                cache,
                &key,
                &source.data,
                size,
                source.deezer_track_id.as_deref(),
                source.is_soundcloud,
                &cancellation,
                generation,
                &cache_track_token,
            )
            .await;
        cache.finish_job(&job_key).await;
        if result.is_ok() && cache.is_current(generation) {
            let _ = cache.prune().await;
        }
        result
    }

    async fn prefetch_unknown_source(
        &self,
        cache: &AudioCache,
        key: &str,
        source: &ResolvedSource,
        cancellation: &CancellationToken,
        generation: u64,
        cache_track_token: &CacheTrackToken,
    ) -> Result<(), String> {
        if let SourceData::Backend(backend) = &source.data {
            let Some(total) = backend.probe_size(cancellation).await else {
                return Ok(());
            };
            if total == 0 || total > cache.max_bytes() || total > MAX_AUDIO_SIZE {
                return Ok(());
            }
            let Some((start, end)) = prefetch_range(total) else {
                return Ok(());
            };
            let expected = (end - start + 1) as usize;
            let bytes = backend
                .read_range(start, end, cancellation)
                .await
                .map_err(|error| error.message)?;
            if bytes.len() != expected {
                return Err("The backend source returned an incomplete prefetch range".into());
            }
            cache
                .remember_total_with_token(key, total, generation, Some(cache_track_token))
                .await;
            let path = cache.block_path(key, total, start, end);
            if cache.read(&path, expected).await.is_none() {
                if !cache
                    .mark_prefetch(key, total, generation, Some(cache_track_token))
                    .await
                {
                    return Ok(());
                }
                cache
                    .write_with_token(&path, &bytes, generation, Some(cache_track_token))
                    .await;
            }
            return Ok(());
        }
        let SourceData::Remote(url) = &source.data else {
            return Ok(());
        };
        let response = send_with_retry(
            "media.prefetch_range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(
                        header::RANGE,
                        format!("bytes=0-{}", BLOCK_SIZE.saturating_sub(1)),
                    )
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await?;
        validate_media_response_url(response.url(), source.is_soundcloud)?;
        let status = response.status();
        if !status.is_success() {
            return Err(PlaybackDownloadError::media_status("audio provider", status).message);
        }
        let total = content_range_total(response.headers())
            .or_else(|| {
                (status == StatusCode::OK)
                    .then_some(response.content_length())
                    .flatten()
            })
            .unwrap_or_default();
        if total == 0 || total > cache.max_bytes() || total > MAX_AUDIO_SIZE {
            return Ok(());
        }
        let Some((start, end)) = prefetch_range(total) else {
            return Ok(());
        };
        let expected = (end - start + 1) as usize;
        let (_, _, mut bytes) = read_response_range(response, start, end)
            .await
            .map_err(|error| error.message)?;
        if bytes.len() != expected {
            return Err("The audio provider returned an incomplete prefetch range".into());
        }
        if let Some(track_id) = source.deezer_track_id.as_deref() {
            decrypt_stripes(&mut bytes, track_id, 0)?;
        }
        cache
            .remember_total_with_token(key, total, generation, Some(cache_track_token))
            .await;
        let path = cache.block_path(key, total, start, end);
        if cache.read(&path, expected).await.is_none() {
            if !cache
                .mark_prefetch(key, total, generation, Some(cache_track_token))
                .await
            {
                return Ok(());
            }
            cache
                .write_with_token(&path, &bytes, generation, Some(cache_track_token))
                .await;
        }
        Ok(())
    }

    async fn prefetch_blocks(
        &self,
        cache: &AudioCache,
        key: &str,
        data: &SourceData,
        size: u64,
        deezer_track_id: Option<&str>,
        is_soundcloud: bool,
        cancellation: &CancellationToken,
        generation: u64,
        cache_track_token: &CacheTrackToken,
    ) -> Result<(), String> {
        let Some((start, end)) = prefetch_range(size) else {
            return Ok(());
        };
        if cancellation.is_cancelled() || !cache.is_current(generation) {
            return Ok(());
        }
        let path = cache.block_path(key, size, start, end);
        let expected = (end - start + 1) as usize;
        if cache.read(&path, expected).await.is_some() {
            return Ok(());
        }
        let _guard = cache.lock_for(&path).await;
        if cache.read(&path, expected).await.is_some() {
            return Ok(());
        }
        let bytes = match data {
            SourceData::Inline(bytes) => {
                inline_range(bytes, start, end).map_err(|error| error.message)?
            }
            SourceData::Backend(backend) => backend
                .read_range(start, end, cancellation)
                .await
                .map_err(|error| error.message)?,
            SourceData::Hls(_) => return Ok(()),
            SourceData::Remote(url) => {
                let (request_start, request_end) = aligned_range(start, end, size);
                let response = send_with_retry(
                    "media.prefetch_range",
                    RequestClass::Media,
                    cancellation,
                    || {
                        self.client
                            .get(url)
                            .header(header::USER_AGENT, BROWSER_USER_AGENT)
                            .header(
                                header::RANGE,
                                format!("bytes={request_start}-{request_end}"),
                            )
                            .header(header::ACCEPT_ENCODING, "identity")
                    },
                )
                .await?;
                validate_media_response_url(response.url(), is_soundcloud)?;
                if !response.status().is_success() {
                    let reason = response
                        .status()
                        .canonical_reason()
                        .unwrap_or("unknown status");
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback provider stage=media.prefetch_range status={} retry=false class=final",
                            response.status()
                        ),
                    );
                    return Err(format!(
                        "The audio provider rejected the cache request (HTTP {} {reason})",
                        response.status().as_u16()
                    ));
                }
                let status = response.status();
                let body = read_response_range(response, request_start, request_end)
                    .await
                    .map_err(|error| error.message)?
                    .2;
                let mut bytes = ranged_body(status, &body, request_start, request_end)?;
                if let Some(track_id) = deezer_track_id {
                    decrypt_stripes(&mut bytes, track_id, request_start / STRIPE_SIZE as u64)?;
                }
                trim_range(bytes, start, end, request_start)?
            }
        };
        if bytes.len() != expected {
            return Err("The source returned an incomplete cache block".into());
        }
        cache
            .write_with_token(&path, &bytes, generation, Some(cache_track_token))
            .await;
        Ok(())
    }

    pub(crate) async fn resolve_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        backend: Option<MediaCredentials>,
        cancellation: CancellationToken,
        include_remote_size: bool,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }

        let resolved = match track.provider {
            PlaybackProvider::Deezer => {
                let mut outcome = self
                    .resolve_backend(
                        track,
                        BackendProvider::Deezer,
                        backend.as_ref(),
                        None,
                        true,
                        None,
                        &cancellation,
                        include_remote_size,
                    )
                    .await;
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                if let MediaResolveOutcome::RefreshSource(_) = outcome {
                    crate::diagnostics::event(
                        "WARN",
                        "deezer recovery source=backend reason=legacy_flac_refresh",
                    );
                    outcome = self
                        .resolve_backend(
                            track,
                            BackendProvider::Deezer,
                            backend.as_ref(),
                            None,
                            true,
                            None,
                            &cancellation,
                            include_remote_size,
                        )
                        .await;
                }
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                match outcome {
                    MediaResolveOutcome::Source(source) => ResolvedSource::from_backend(source),
                    MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                        let fallback = self
                            .resolve_deezer_backend_fallback_id(
                                track,
                                deezer_arl.as_ref(),
                                backend.as_ref(),
                                &cancellation,
                                include_remote_size,
                            )
                            .await?;
                        match fallback {
                            MediaResolveOutcome::Source(source) => {
                                ResolvedSource::from_backend(source)
                            }
                            MediaResolveOutcome::Unavailable
                            | MediaResolveOutcome::FallbackToDirect(_) => {
                                ResolvedSource::from_remote(
                                    self.resolve_deezer(
                                        &track.id,
                                        deezer_arl.as_ref(),
                                        false,
                                        &cancellation,
                                        include_remote_size,
                                    )
                                    .await?,
                                )
                            }
                            MediaResolveOutcome::RefreshSource(error) => return Err(error),
                            MediaResolveOutcome::Cancelled => {
                                return Err("Playback request cancelled".into());
                            }
                            MediaResolveOutcome::Fatal(error) => return Err(error),
                        }
                    }
                    MediaResolveOutcome::RefreshSource(error) => return Err(error),
                    MediaResolveOutcome::Cancelled => {
                        return Err("Playback request cancelled".into());
                    }
                    MediaResolveOutcome::Fatal(error) => return Err(error),
                }
            }
            PlaybackProvider::SoundCloud => {
                let prefetched_track = if backend.is_some() {
                    self.fetch_soundcloud_track(&track.id, soundcloud_token.as_ref(), &cancellation)
                        .await
                        .ok()
                } else {
                    None
                };
                if track.downloadable
                    && let Some(token) = soundcloud_token.as_ref()
                    && let Ok(source) = self
                        .resolve_soundcloud_original(
                            &track.id,
                            Some(token),
                            &cancellation,
                            include_remote_size,
                        )
                        .await
                {
                    ResolvedSource::from_remote(source)
                } else {
                    let outcome = self
                        .resolve_backend(
                            track,
                            BackendProvider::SoundCloud,
                            backend.as_ref(),
                            prefetched_track.as_ref(),
                            true,
                            None,
                            &cancellation,
                            include_remote_size,
                        )
                        .await;
                    match outcome {
                        MediaResolveOutcome::Source(source) => ResolvedSource::from_backend(source),
                        MediaResolveOutcome::Unavailable
                        | MediaResolveOutcome::FallbackToDirect(_) => ResolvedSource::from_remote(
                            self.resolve_soundcloud(
                                &track.id,
                                soundcloud_token.as_ref(),
                                &cancellation,
                                include_remote_size,
                                true,
                                prefetched_track,
                            )
                            .await?,
                        ),
                        MediaResolveOutcome::RefreshSource(error)
                        | MediaResolveOutcome::Fatal(error) => return Err(error),
                        MediaResolveOutcome::Cancelled => {
                            return Err("Playback request cancelled".into());
                        }
                    }
                }
            }
        };

        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        Ok(resolved)
    }

    /// Returns a useful total size for context-menu metadata without reading
    /// the media body. Provider metadata is preferred, then inline payload
    /// length, followed by a HEAD and a one-byte range probe for remote URLs.
    pub(crate) async fn source_size_for_info(
        &self,
        source: &ResolvedSource,
        cancellation: &CancellationToken,
    ) -> Result<u64, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        if source.size > 0 {
            return Ok(source.size);
        }
        match &source.data {
            SourceData::Inline(bytes) => Ok(bytes.len() as u64),
            SourceData::Remote(url) => Ok(self
                .probe_remote_size(url, cancellation, source.is_soundcloud)
                .await
                .unwrap_or(0)),
            SourceData::Backend(_) | SourceData::Hls(_) => {
                let Some(cache) = self.cache.as_ref() else {
                    return Ok(0);
                };
                let Some(key) = cache_key(source) else {
                    return Ok(0);
                };
                let Some(total) = cache.known_total(&key).await else {
                    return Ok(0);
                };
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                if cache.is_fully_cached(&key, total).await {
                    Ok(total)
                } else {
                    Ok(0)
                }
            }
        }
    }

    async fn probe_remote_size(
        &self,
        url: &str,
        cancellation: &CancellationToken,
        is_soundcloud: bool,
    ) -> Option<u64> {
        let head = send_with_retry("media.info.head", RequestClass::Media, cancellation, || {
            self.client
                .head(url)
                .header(header::USER_AGENT, BROWSER_USER_AGENT)
                .header(header::ACCEPT_ENCODING, "identity")
        })
        .await
        .ok();
        if let Some(head) = head {
            if validate_media_response_url(head.url(), is_soundcloud).is_ok()
                && let Some(size) =
                    remote_size_from_response(head.status(), head.headers(), head.content_length())
            {
                return Some(size);
            }
        }

        if cancellation.is_cancelled() {
            return None;
        }
        let response = send_with_retry(
            "media.info.range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
                    .header(header::RANGE, "bytes=0-0")
            },
        )
        .await
        .ok()?;
        validate_media_response_url(response.url(), is_soundcloud).ok()?;
        remote_size_from_response(
            response.status(),
            response.headers(),
            response.content_length(),
        )
    }

    pub(crate) async fn download_source(
        &self,
        source: ResolvedSource,
        mut output: File,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), String> {
        self.download_source_inner(source, &mut output, cancellation, progress)
            .await
            .map_err(|error| error.message)
    }

    async fn download_source_inner<W>(
        &self,
        source: ResolvedSource,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        if let SourceData::Backend(backend) = &source.data {
            return backend
                .download(output, cancellation, progress.as_ref())
                .await;
        }
        self.download(
            RemoteAudio {
                data: source.data,
                size: source.size,
                deezer_track_id: source.deezer_track_id,
                format: source.format,
                format_name: source.format_name,
                declared_bitrate: source.declared_bitrate,
                is_soundcloud: source.is_soundcloud,
                cache_identity: source.cache_identity,
            },
            output,
            cancellation,
            progress,
        )
        .await
    }

    pub(crate) async fn resolve_download_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variant: DownloadVariant,
        cancellation: CancellationToken,
    ) -> Result<ResolvedSource, String> {
        // Mirror the original app's download reservation once per user action.
        // Capability probes call the private helper directly and remain
        // ungated, since they have no equivalent in the original app.
        self.limiter.reserve(&cancellation).await?;
        self.resolve_download_source_with_size(
            track,
            deezer_arl,
            soundcloud_token,
            murglar,
            variant,
            cancellation,
            true,
            false,
        )
        .await
    }

    async fn resolve_download_source_with_size(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variant: DownloadVariant,
        cancellation: CancellationToken,
        include_remote_size: bool,
        capability_only: bool,
    ) -> Result<ResolvedSource, String> {
        if track.provider == PlaybackProvider::Deezer {
            let source = match variant {
                DownloadVariant::DeezerFlac => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "FLAC",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                DownloadVariant::DeezerMp3_320 => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "MP3_320",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                DownloadVariant::DeezerMp3_128 => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "MP3_128",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                // Existing callers use Best for the original strict FLAC
                // download action. Preserve that behavior while the new
                // exact variants power the dynamic context submenu.
                DownloadVariant::Best
                | DownloadVariant::Original
                | DownloadVariant::Murglar
                | DownloadVariant::Standard => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "FLAC",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
            };
            return Ok(source);
        }
        // Exact SoundCloud choices are resolved directly. Provider metadata
        // can lag behind the actual download endpoint, so the capability
        // probe must validate the path itself instead of trusting the
        // `downloadable` or `progressive` hints.
        if matches!(
            variant,
            DownloadVariant::Original | DownloadVariant::Murglar | DownloadVariant::Standard
        ) {
            let source = match variant {
                DownloadVariant::Original => {
                    let Some(token) = soundcloud_token.as_ref() else {
                        return Err(
                            "A SoundCloud account is required for the original file.".into()
                        );
                    };
                    self.resolve_soundcloud_original(
                        &track.id,
                        Some(token),
                        &cancellation,
                        include_remote_size,
                    )
                    .await
                    .map(ResolvedSource::from_remote)
                }
                DownloadVariant::Murglar => {
                    self.resolve_soundcloud_backend_download(
                        track,
                        murglar.as_ref(),
                        &cancellation,
                        include_remote_size,
                        capability_only,
                    )
                    .await
                }
                DownloadVariant::Standard => self
                    .resolve_soundcloud(
                        &track.id,
                        soundcloud_token.as_ref(),
                        &cancellation,
                        include_remote_size,
                        false,
                        None,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                _ => unreachable!(),
            }?;
            return Ok(source);
        }
        let order = selection_order(
            track,
            variant,
            deezer_arl.is_some(),
            soundcloud_token.is_some(),
            murglar.is_some(),
        );
        if order.is_empty() {
            return Err("The selected download format is unavailable for this track.".into());
        }
        for selected in order {
            let result = match selected {
                DownloadVariant::Original => self
                    .resolve_soundcloud_original(
                        &track.id,
                        soundcloud_token.as_ref(),
                        &cancellation,
                        include_remote_size,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                DownloadVariant::Murglar => {
                    self.resolve_soundcloud_backend_download(
                        track,
                        murglar.as_ref(),
                        &cancellation,
                        include_remote_size,
                        capability_only,
                    )
                    .await
                }
                DownloadVariant::Standard => self
                    .resolve_soundcloud(
                        &track.id,
                        None,
                        &cancellation,
                        include_remote_size,
                        false,
                        None,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                DownloadVariant::DeezerFlac
                | DownloadVariant::DeezerMp3_320
                | DownloadVariant::DeezerMp3_128 => {
                    Err("The selected Deezer format cannot be used for a SoundCloud track.".into())
                }
                DownloadVariant::Best => unreachable!(),
            };
            match result {
                Ok(source) => {
                    return Ok(source);
                }
                Err(error) if error == "Playback request cancelled" => return Err(error),
                Err(_) => {}
            }
        }
        Err(match variant {
            DownloadVariant::Original => {
                "The original SoundCloud file is unavailable for this track."
            }
            DownloadVariant::Murglar => {
                "Murglar did not return a lossless or high-quality file for this track."
            }
            DownloadVariant::Standard => {
                "A downloadable MP3 128 kbps stream is unavailable for this track."
            }
            DownloadVariant::Best => {
                "No downloadable SoundCloud audio source is available for this track."
            }
            DownloadVariant::DeezerFlac
            | DownloadVariant::DeezerMp3_320
            | DownloadVariant::DeezerMp3_128 => {
                "The selected Deezer format cannot be used for a SoundCloud track."
            }
        }
        .into())
    }

    /// Probe only formats which can be selected in the track download menu.
    /// Each candidate is resolved independently, so account and track
    /// metadata never fabricates a menu entry. The caller owns caching and
    /// cancellation because this method intentionally performs network I/O.
    pub(crate) async fn probe_download_capabilities(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
    ) -> Result<Vec<DownloadChoice>, String> {
        let variants = Self::capability_variants(
            track,
            deezer_arl.is_some(),
            soundcloud_token.is_some(),
            murglar.is_some(),
        );
        let started = Instant::now();
        crate::diagnostics::event(
            "INFO",
            format!(
                "download capability probe start provider={:?} variants={} deezer_arl={} soundcloud_token={} murglar={}",
                track.provider,
                variants.len(),
                deezer_arl.is_some(),
                soundcloud_token.is_some(),
                murglar.is_some(),
            ),
        );
        let result = self
            .probe_download_capabilities_inner(
                track,
                deezer_arl,
                soundcloud_token,
                murglar,
                variants,
                cancellation,
            )
            .await;
        crate::diagnostics::event(
            "INFO",
            format!(
                "download capability probe finish provider={:?} elapsed_ms={} outcome={} choices={}",
                track.provider,
                started.elapsed().as_millis(),
                if result.is_ok() { "ready" } else { "failed" },
                result.as_ref().map_or(0, Vec::len),
            ),
        );
        result
    }

    async fn probe_download_capabilities_inner(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variants: Vec<DownloadVariant>,
        cancellation: CancellationToken,
    ) -> Result<Vec<DownloadChoice>, String> {
        // A capability probe only needs the provider-confirmed format. It
        // must not issue a HEAD/range request for every candidate, since the
        // download path can resolve the size lazily when the user selects a
        // row. Candidates are started together, while the shared provider
        // gate still spaces their provider requests and preserves the retry
        // budget.
        let results = join_all(variants.iter().copied().map(|variant| {
            let cancellation = cancellation.clone();
            let deezer_arl = deezer_arl.clone();
            let soundcloud_token = soundcloud_token.clone();
            let murglar = murglar.clone();
            async move {
                let _direct_deezer_permit =
                    if track.provider == PlaybackProvider::Deezer && deezer_arl.is_some() {
                        Some(Self::acquire_direct_deezer_capability_slot(&cancellation).await?)
                    } else {
                        None
                    };
                self.resolve_download_source_with_size(
                    track,
                    deezer_arl,
                    soundcloud_token,
                    murglar,
                    variant,
                    cancellation,
                    false,
                    true,
                )
                .await
            }
        }))
        .await;
        if cancellation.is_cancelled() {
            return Err("Download capability probe cancelled".into());
        }
        let mut choices = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut failures = Vec::new();
        for (variant, result) in variants.into_iter().zip(results) {
            let source = match result {
                Ok(source) => source,
                Err(error)
                    if cancellation.is_cancelled()
                        || matches!(
                            error.as_str(),
                            "Playback request cancelled" | "Download capability probe cancelled"
                        ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    failures.push((variant, error));
                    continue;
                }
            };
            let key = Self::capability_dedup_key(
                track.provider,
                variant,
                &source.format_name,
                source.declared_bitrate,
            );
            if !seen.insert(key) {
                continue;
            }
            let (label, detail) = download_choice_text(variant, &source);
            choices.push(DownloadChoice {
                variant,
                label,
                detail,
            });
        }
        // A genuinely empty capability set is cacheable, but a probe where
        // every candidate failed for a transport/auth/provider reason must
        // remain retryable. Otherwise one transient outage turns into a
        // permanent "No downloadable formats" row for this account scope.
        // Do not cache a partial menu. If one requested variant failed for a
        // transient, authentication, or rate-limit reason, the successful
        // subset is not a reliable capability result for this account scope.
        if has_unexpected_capability_failure(&failures) {
            return Err(failures
                .into_iter()
                .find(|(variant, error)| !is_expected_capability_absence(*variant, error))
                .map(|(_, error)| error)
                .unwrap_or_else(|| "Download capability probe failed".to_owned()));
        }
        Ok(choices)
    }

    fn capability_variants(
        track: &PlaybackTrack,
        deezer_arl: bool,
        soundcloud_token: bool,
        murglar_token: bool,
    ) -> Vec<DownloadVariant> {
        match track.provider {
            // Keep the track probe and collection menus on the same exact
            // credential policy. Murglar owns FLAC/MP3 320, while the Deezer
            // ARL owns MP3 128. Each candidate is still resolved per track
            // below, so this list only controls which formats are attempted.
            PlaybackProvider::Deezer => {
                deezer_collection_download_choices(deezer_arl, murglar_token)
                    .iter()
                    .map(|choice| choice.variant)
                    .collect()
            }
            PlaybackProvider::SoundCloud => {
                let mut variants = Vec::with_capacity(3);
                if soundcloud_token {
                    variants.push(DownloadVariant::Original);
                }
                if murglar_token {
                    variants.push(DownloadVariant::Murglar);
                }
                // The track response can lag behind the active transcoding
                // endpoint, so the standard path is always validated directly.
                // Its label comes from the resolved provider metadata, never a
                // fabricated SoundCloud quality.
                variants.push(DownloadVariant::Standard);
                variants
            }
        }
    }

    async fn acquire_direct_deezer_capability_slot(
        cancellation: &CancellationToken,
    ) -> Result<tokio::sync::SemaphorePermit<'static>, String> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err("Download capability probe cancelled".into()),
            permit = DIRECT_DEEZER_CAPABILITY_GATE.acquire() => {
                permit.map_err(|_| "Download capability probe stopped unexpectedly".into())
            }
        }
    }

    fn capability_dedup_key(
        _provider: PlaybackProvider,
        _variant: DownloadVariant,
        format_name: &str,
        declared_bitrate: Option<u32>,
    ) -> String {
        let codec = audio_format(format_name)
            .map(AudioFormat::label)
            .unwrap_or_else(|| format_name.trim())
            .to_ascii_uppercase();
        let quality = format!(
            "{}:{}",
            codec,
            declared_bitrate.map_or_else(|| "-".to_owned(), |bitrate| bitrate.to_string())
        );
        quality
    }

    async fn resolve_deezer_exact(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<&DeezerArl>,
        backend: Option<&MediaCredentials>,
        expected: &str,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let backend_quality = match expected {
            "FLAC" => Some("LOSSLESS"),
            "MP3_320" => Some("HIGH_QUALITY"),
            "MP3_128" => None,
            _ => None,
        };
        if let Some(quality) = backend_quality {
            let mut outcome = self
                .resolve_backend(
                    track,
                    BackendProvider::Deezer,
                    backend,
                    None,
                    quality == "HIGH_QUALITY",
                    Some(quality),
                    cancellation,
                    include_remote_size,
                )
                .await;
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            if let MediaResolveOutcome::RefreshSource(_) = outcome {
                crate::diagnostics::event(
                    "WARN",
                    "deezer recovery source=backend reason=legacy_flac_refresh",
                );
                outcome = self
                    .resolve_backend(
                        track,
                        BackendProvider::Deezer,
                        backend,
                        None,
                        quality == "HIGH_QUALITY",
                        Some(quality),
                        cancellation,
                        include_remote_size,
                    )
                    .await;
            }
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            match outcome {
                MediaResolveOutcome::Source(source)
                    if source.metadata().format_name.eq_ignore_ascii_case(expected) =>
                {
                    return Ok(ResolvedSource::from_backend(source));
                }
                MediaResolveOutcome::Source(_)
                | MediaResolveOutcome::Unavailable
                | MediaResolveOutcome::FallbackToDirect(_) => {}
                MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                    return Err(error);
                }
                MediaResolveOutcome::Cancelled => {
                    return Err("Playback request cancelled".into());
                }
            }
        }
        if should_skip_direct_deezer_capability(include_remote_size, deezer_arl) {
            return Err("No Deezer direct source available without an ARL".into());
        }
        let source = self
            .resolve_deezer_exact_direct(
                &track.id,
                deezer_arl,
                expected,
                cancellation,
                include_remote_size,
            )
            .await?;
        if source.format_name.eq_ignore_ascii_case(expected) {
            Ok(ResolvedSource::from_remote(source))
        } else {
            Err(format!(
                "Deezer returned {} while {} was requested.",
                source.format_name, expected
            ))
        }
    }

    async fn resolve_deezer_exact_direct(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        expected: &str,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let preferences: &'static [&'static str] = match expected {
            "FLAC" => &["FLAC"],
            "MP3_320" => &["MP3_320"],
            "MP3_128" => &["MP3_128"],
            _ => return Err("Unsupported Deezer download format.".into()),
        };
        self.resolve_deezer_with_formats(
            track_id,
            arl,
            preferences,
            false,
            cancellation,
            include_remote_size,
        )
        .await
    }

    async fn resolve_soundcloud_original(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let url = format!(
            "https://api-v2.soundcloud.com/tracks/{track_id}/download?client_id={SOUNDCLOUD_CLIENT_ID}"
        );
        let value = response_json(
            send_with_retry(
                "soundcloud.original",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(url.clone(), token),
            )
            .await?,
            "soundcloud.original",
        )
        .await?;
        let url = value
            .get("redirectUri")
            .or_else(|| value.get("redirect_uri"))
            .or_else(|| value.get("url"))
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| "SoundCloud did not return an original download URL".to_string())?;
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| "SoundCloud returned an invalid original URL".to_string())?;
        validate_soundcloud_stream_url(&parsed)?;
        let provider_format = infer_soundcloud_original_format(&value, &parsed);
        let inspection = self
            .inspect_soundcloud_original_media(url, cancellation)
            .await?;
        let format = choose_soundcloud_original_format(
            inspection.sniffed_format,
            inspection.header_format,
            provider_format,
        )
        .ok_or_else(|| SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN.to_owned())?;
        let size = if include_remote_size {
            inspection.size.unwrap_or(0)
        } else {
            0
        };
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let format_name = format.label().to_owned();
        let declared_bitrate = soundcloud_original_bitrate(&value, format);
        Ok(RemoteAudio {
            data: SourceData::Remote(url.into()),
            size,
            deezer_track_id: None,
            is_soundcloud: true,
            format,
            format_name: format_name.clone(),
            declared_bitrate,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::SoundCloud,
                track_id,
                SourceVariant::SoundCloudOriginal,
                format,
                &format_name,
            )),
        })
    }

    async fn inspect_soundcloud_original_media(
        &self,
        url: &str,
        cancellation: &CancellationToken,
    ) -> Result<SoundCloudOriginalInspection, String> {
        let head = send_with_retry(
            "soundcloud.original.media",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .head(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await;

        let (head_format, head_size) = match head {
            Ok(response) => {
                validate_soundcloud_stream_url(response.url())?;
                if response.status().is_success() {
                    (
                        soundcloud_format_from_media_headers(response.headers(), response.url()),
                        remote_size_from_response(
                            response.status(),
                            response.headers(),
                            response.content_length(),
                        ),
                    )
                } else if soundcloud_original_head_can_fallback(response.status()) {
                    (None, None)
                } else {
                    return Err(provider_rejection("soundcloud.original", response.status()));
                }
            }
            Err(error) if error == "Playback request cancelled" => return Err(error),
            Err(_) => (None, None),
        };

        // Always inspect the actual media bytes. Provider metadata and HEAD
        // headers are only fallback candidates because uploader originals can
        // be mislabeled by either source.
        let range = self
            .soundcloud_original_range_probe(url, cancellation)
            .await?;
        Ok(SoundCloudOriginalInspection {
            sniffed_format: range.sniffed_format,
            header_format: range.header_format.or(head_format),
            size: head_size.or(range.size),
        })
    }

    async fn soundcloud_original_range_probe(
        &self,
        url: &str,
        cancellation: &CancellationToken,
    ) -> Result<SoundCloudOriginalInspection, String> {
        let response = send_with_retry(
            "soundcloud.original.media",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
                    .header(
                        header::RANGE,
                        format!("bytes=0-{}", SOUNDCLOUD_ORIGINAL_INSPECTION_BYTES - 1),
                    )
            },
        )
        .await?;
        validate_soundcloud_stream_url(response.url())?;
        if !response.status().is_success() {
            return Err(provider_rejection("soundcloud.original", response.status()));
        }
        let size = remote_size_from_response(
            response.status(),
            response.headers(),
            response.content_length(),
        );
        let header_format =
            soundcloud_format_from_media_headers(response.headers(), response.url());
        let bytes =
            read_bounded_response(response, SOUNDCLOUD_ORIGINAL_INSPECTION_BYTES, cancellation)
                .await?;
        Ok(SoundCloudOriginalInspection {
            sniffed_format: sniff_soundcloud_original_format(&bytes),
            header_format,
            size,
        })
    }

    fn backend_artist_names(track: &PlaybackTrack) -> Vec<String> {
        let mut names = track
            .artists
            .iter()
            .map(|artist| artist.name.trim())
            .filter(|artist| !artist.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if names.is_empty() && !track.artist.trim().is_empty() {
            names.push(track.artist.trim().to_owned());
        }
        names
    }

    fn backend_request(
        track: &PlaybackTrack,
        provider: BackendProvider,
        authoritative: Option<&Value>,
        allow_high_quality: bool,
        exact_quality: Option<&str>,
        include_remote_size: bool,
    ) -> MediaRequest {
        let (track_id, title, artist_names, duration_ms) =
            if provider == BackendProvider::SoundCloud {
                Self::soundcloud_backend_fields(track, authoritative)
            } else {
                (
                    track.id.clone(),
                    track.title.clone(),
                    Self::backend_artist_names(track),
                    track.duration.as_millis().min(u64::MAX as u128) as u64,
                )
            };
        MediaRequest::new(
            provider,
            track_id,
            title,
            artist_names,
            (provider == BackendProvider::Deezer).then(|| track.album.clone()),
            normalize_release_date(&track.release_date),
            duration_ms,
            allow_high_quality,
            exact_quality.map(str::to_owned),
            include_remote_size,
        )
    }

    async fn resolve_backend(
        &self,
        track: &PlaybackTrack,
        provider: BackendProvider,
        credentials: Option<&MediaCredentials>,
        authoritative: Option<&Value>,
        allow_high_quality: bool,
        exact_quality: Option<&str>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> MediaResolveOutcome<BackendSource> {
        #[cfg(test)]
        if let Some(resolve) = &self.backend_resolve_override {
            return resolve(cancellation);
        }
        let Some(credentials) = credentials else {
            return MediaResolveOutcome::Unavailable;
        };
        let request = Self::backend_request(
            track,
            provider,
            authoritative,
            allow_high_quality,
            exact_quality,
            include_remote_size,
        );
        self.media_backend
            .resolve(request, credentials, cancellation)
            .await
    }

    async fn resolve_deezer_backend_fallback_id(
        &self,
        track: &PlaybackTrack,
        arl: Option<&DeezerArl>,
        credentials: Option<&MediaCredentials>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<MediaResolveOutcome<BackendSource>, String> {
        let (Some(arl), Some(credentials)) = (arl, credentials) else {
            return Ok(MediaResolveOutcome::Unavailable);
        };
        let identity = match self
            .fetch_deezer_playback_context(&track.id, Some(arl), cancellation)
            .await
        {
            Ok(context) => context.identity,
            Err(error) if error == "Playback request cancelled" => return Err(error),
            Err(_) => return Ok(MediaResolveOutcome::Unavailable),
        };
        let Some(fallback_id) = deezer_fallback_track_id(&identity, &track.id) else {
            return Ok(MediaResolveOutcome::Unavailable);
        };
        let mut fallback_track = track.clone();
        fallback_track.id = fallback_id.to_owned();
        Ok(self
            .resolve_backend(
                &fallback_track,
                BackendProvider::Deezer,
                Some(credentials),
                None,
                true,
                None,
                cancellation,
                include_remote_size,
            )
            .await)
    }

    pub(crate) async fn record_deezer_listen(&self, payload: Value, arl: DeezerArl) -> bool {
        match self.try_record_deezer_listen(payload, &arl).await {
            Ok(()) => true,
            Err(error) => {
                crate::diagnostics::event(
                    "WARN",
                    format!("deezer listening history update failed reason={error}"),
                );
                false
            }
        }
    }

    async fn try_record_deezer_listen(
        &self,
        payload: Value,
        arl: &DeezerArl,
    ) -> Result<(), String> {
        let session_url = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
        let session_response = self
            .client
            .post(session_url)
            .headers(deezer_headers(Some(arl), None)?)
            .header(header::CONTENT_LENGTH, "0")
            .body("")
            .send()
            .await
            .map_err(request_error)?;
        if !session_response.status().is_success() {
            return Err(format!(
                "session status={}",
                session_response.status().as_u16()
            ));
        }
        let cookies = response_cookies(&session_response);
        let session = response_json(session_response, "deezer.listen.session").await?;
        let check_form = session
            .pointer("/results/checkForm")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "session token missing".to_string())?;
        let response = self
            .deezer_listen_request(
                check_form,
                deezer_headers(Some(arl), Some(&cookies))?,
                &payload,
            )
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(format!("status={}", response.status().as_u16()));
        }
        Ok(())
    }

    fn deezer_listen_request(
        &self,
        check_form: &str,
        headers: HeaderMap,
        payload: &Value,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(format!(
                "https://www.deezer.com/ajax/gw-light.php?method=log.listen&input=3&api_version=1.0&api_token={check_form}"
            ))
            .headers(headers)
            .json(payload)
    }

    pub(crate) async fn record_soundcloud_listen(
        &self,
        report: SoundCloudListenReport,
        token: SoundCloudToken,
    ) -> bool {
        match self.try_record_soundcloud_listen(&report, &token).await {
            Ok(()) => true,
            Err(error) => {
                crate::diagnostics::event(
                    "WARN",
                    format!("soundcloud listening history update failed reason={error}"),
                );
                false
            }
        }
    }

    async fn try_record_soundcloud_listen(
        &self,
        report: &SoundCloudListenReport,
        token: &SoundCloudToken,
    ) -> Result<(), String> {
        let response = self
            .soundcloud_listen_request(token, report)?
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(format!("status={}", response.status().as_u16()));
        }
        Ok(())
    }

    fn soundcloud_listen_request(
        &self,
        token: &SoundCloudToken,
        report: &SoundCloudListenReport,
    ) -> Result<reqwest::RequestBuilder, String> {
        let authorization = token
            .authorization_header()
            .map_err(|_| "The saved SoundCloud session is invalid".to_string())?;
        Ok(self
            .client
            .post("https://api-v2.soundcloud.com/me/play-history")
            .header(header::USER_AGENT, SOUNDCLOUD_MOBILE_USER_AGENT)
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_ENCODING, SOUNDCLOUD_MOBILE_ACCEPT_ENCODING)
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&report.payload()))
    }

    async fn fetch_deezer_playback_context(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        cancellation: &CancellationToken,
    ) -> Result<DeezerPlaybackContext, String> {
        let session_url = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
        let session_headers = deezer_headers(arl, None)?;
        let session_response = send_with_retry(
            "deezer.session",
            RequestClass::Provider,
            cancellation,
            || {
                self.client
                    .post(session_url)
                    .headers(session_headers.clone())
                    .header(header::CONTENT_LENGTH, "0")
                    .body("")
            },
        )
        .await?;
        let cookies = response_cookies(&session_response);
        let session = response_json(session_response, "deezer.session").await?;
        let check_form = session
            .pointer("/results/checkForm")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let license_token = session
            .pointer("/results/USER/OPTIONS/license_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if license_token.is_empty() {
            return Err("Deezer login is required for playback".into());
        }
        let headers = deezer_headers(arl, Some(&cookies))?;
        let song_url = format!(
            "https://www.deezer.com/ajax/gw-light.php?method=deezer.pageTrack&input=3&api_version=1.0&api_token={check_form}"
        );
        let song = response_json(
            send_with_retry("deezer.song", RequestClass::Provider, cancellation, || {
                self.client
                    .post(song_url.clone())
                    .headers(headers.clone())
                    .json(&json!({ "sng_id": track_id }))
            })
            .await?,
            "deezer.song",
        )
        .await?;
        if deezer_track_is_unavailable(&song) {
            return Err(DEEZER_UNAVAILABLE.into());
        }
        Ok(DeezerPlaybackContext {
            headers,
            license_token: license_token.to_owned(),
            identity: deezer_playback_identity(&song, track_id)?,
        })
    }

    async fn resolve_deezer(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        strict_flac: bool,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        self.resolve_deezer_with_formats(
            track_id,
            arl,
            deezer_format_preferences(strict_flac),
            strict_flac,
            cancellation,
            include_remote_size,
        )
        .await
    }

    async fn resolve_deezer_with_formats(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        formats: &[&str],
        strict_flac: bool,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let DeezerPlaybackContext {
            headers,
            license_token,
            identity,
        } = self
            .fetch_deezer_playback_context(track_id, arl, cancellation)
            .await?;
        let formats = formats
            .iter()
            .map(|format| json!({ "cipher": "BF_CBC_STRIPE", "format": format }))
            .collect::<Vec<_>>();
        let media = response_json(
            send_with_retry("deezer.media", RequestClass::Provider, cancellation, || {
                self.client
                    .post("https://media.deezer.com/v1/get_url")
                    .headers(headers.clone())
                    .json(&json!({
                        "license_token": license_token,
                        "media": [{ "type": "FULL", "formats": formats.clone() }],
                        "track_tokens": [identity.track_token.clone()]
                    }))
            })
            .await?,
            "deezer.media",
        )
        .await?;
        let result = media
            .pointer("/data/0/media/0")
            .ok_or_else(|| DEEZER_UNAVAILABLE.to_string())?;
        let url = result
            .pointer("/sources/0/url")
            .and_then(Value::as_str)
            .and_then(|url| url.split('#').next())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| DEEZER_UNAVAILABLE.to_string())?;
        let size = match result
            .get("filesize")
            .and_then(Value::as_u64)
            .filter(|size| *size > 0)
        {
            Some(size) => size,
            None if include_remote_size => self
                .probe_remote_size(url, cancellation, false)
                .await
                .unwrap_or(0),
            None => 0,
        };
        let format_name = result
            .get("format")
            .and_then(Value::as_str)
            .ok_or_else(|| "Deezer did not identify the resolved audio format".to_string())?
            .to_owned();
        let declared_bitrate = declared_bitrate(&format_name);
        let format = audio_format(&format_name)
            .ok_or_else(|| "Deezer did not identify the resolved audio format".to_string())?;
        let format = enforce_deezer_format(format, strict_flac)?;
        crate::diagnostics::event(
            "INFO",
            format!(
                "deezer source path=direct source_id=media_api format={} decryption=once strict_flac={strict_flac} fallback={}",
                format.label(),
                identity.used_fallback,
            ),
        );
        Ok(RemoteAudio {
            data: SourceData::Remote(url.into()),
            size,
            deezer_track_id: Some(identity.source_track_id.clone()),
            format,
            format_name: format_name.clone(),
            declared_bitrate,
            is_soundcloud: false,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::Deezer,
                &identity.source_track_id,
                SourceVariant::Deezer,
                format,
                &format_name,
            )),
        })
    }

    async fn fetch_soundcloud_track(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
    ) -> Result<Value, String> {
        let track_url = format!(
            "https://api-v2.soundcloud.com/tracks/{track_id}?client_id={SOUNDCLOUD_CLIENT_ID}"
        );
        response_json(
            send_with_retry(
                "soundcloud.track",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(track_url.clone(), token),
            )
            .await?,
            "soundcloud.track",
        )
        .await
    }

    async fn resolve_soundcloud(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
        prefer_hls: bool,
        prefetched_track: Option<Value>,
    ) -> Result<RemoteAudio, String> {
        let track = match prefetched_track {
            Some(track) => track,
            None => {
                self.fetch_soundcloud_track(track_id, token, cancellation)
                    .await?
            }
        };
        let track_authorization = soundcloud_track_authorization(&track);
        let transcodings = track
            .pointer("/media/transcodings")
            .and_then(Value::as_array)
            .ok_or_else(|| "SoundCloud did not return audio transcodings".to_string())?;
        let candidates = if prefer_hls {
            soundcloud_playback_transcodings(transcodings)
        } else {
            soundcloud_transcodings(transcodings).collect()
        };
        for selected in candidates {
            let hls = is_soundcloud_hls_transcoding(selected);
            let Some(selected_url) = selected.get("url").and_then(Value::as_str) else {
                continue;
            };
            let Ok(mut url) = reqwest::Url::parse(selected_url) else {
                return Err("SoundCloud returned an invalid transcoding URL".into());
            };
            validate_soundcloud_transcoding_url(&url)?;
            append_soundcloud_transcoding_query(&mut url, track_authorization);
            let transcoding_url = url.to_string();
            let response = match send_with_retry(
                "soundcloud.transcoding",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(transcoding_url.clone(), token),
            )
            .await
            {
                Ok(response) => response,
                Err(error) if error == "Playback request cancelled" => return Err(error),
                Err(error) if is_soundcloud_candidate_absence(&error) => continue,
                Err(error) => return Err(error),
            };
            let value = match response_json(response, "soundcloud.transcoding").await {
                Ok(value) => value,
                Err(error) if is_soundcloud_candidate_absence(&error) => continue,
                Err(error) => return Err(error),
            };
            let Some(stream_url) = value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
            else {
                return Err("SoundCloud transcoding response did not return a stream URL".into());
            };
            let stream_url = reqwest::Url::parse(stream_url)
                .map_err(|_| "SoundCloud returned an invalid stream URL".to_string())?;
            let (data, size) = if hls {
                let manifest_url = stream_url.to_string();
                let descriptor = match super::soundcloud_hls::inspect(
                    &self.client,
                    &manifest_url,
                    cancellation,
                    BROWSER_USER_AGENT,
                )
                .await
                {
                    Ok(descriptor) => descriptor,
                    Err(error) if error == "Playback request cancelled" => return Err(error),
                    Err(_) => continue,
                };
                (SourceData::Hls(descriptor), 0)
            } else {
                validate_soundcloud_stream_url(&stream_url)?;
                let stream_url = stream_url.to_string();
                let size = if include_remote_size {
                    self.probe_remote_size(&stream_url, cancellation, true)
                        .await
                        .unwrap_or(0)
                } else {
                    0
                };
                (SourceData::Remote(stream_url), size)
            };
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            let format = transcoding_format(selected);
            let format_name = if hls {
                AudioFormat::M4a.label().to_owned()
            } else {
                soundcloud_format_name(selected, format)
            };
            return Ok(RemoteAudio {
                data,
                size,
                deezer_track_id: None,
                format,
                format_name: format_name.clone(),
                declared_bitrate: soundcloud_transcoding_bitrate(selected),
                is_soundcloud: true,
                cache_identity: Some(stable_cache_identity(
                    PlaybackProvider::SoundCloud,
                    track_id,
                    SourceVariant::SoundCloudStandard,
                    format,
                    &format_name,
                )),
            });
        }
        Err("No active stream link found for this SoundCloud track".into())
    }

    fn backend_artist_names_for_soundcloud(track: &PlaybackTrack) -> Vec<String> {
        let mut names = Vec::new();
        for artist in &track.artists {
            let name = artist.name.trim();
            if name.is_empty()
                || names
                    .iter()
                    .any(|known: &String| known.eq_ignore_ascii_case(name))
            {
                continue;
            }
            names.push(name.to_owned());
            if names.len() == MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES {
                break;
            }
        }
        if let Some(owner_slug) = Self::soundcloud_owner_slug(&track.service_url)
            && !names
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&owner_slug))
        {
            if names.len() == MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES {
                names.pop();
            }
            names.push(owner_slug);
        }
        if names.is_empty() {
            let fallback = track.artist.trim();
            if !fallback.is_empty() {
                names.push(fallback.to_owned());
            }
        }
        names
    }

    fn soundcloud_backend_fields(
        track: &PlaybackTrack,
        authoritative: Option<&Value>,
    ) -> (String, String, Vec<String>, u64) {
        let authoritative_id = authoritative
            .and_then(|value| {
                deezer_id(value.get("id")).or_else(|| {
                    value
                        .get("urn")
                        .and_then(Value::as_str)
                        .and_then(|urn| urn.strip_prefix("soundcloud:tracks:"))
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                })
            })
            .filter(|id| id.parse::<u64>().is_ok_and(|id| id > 0));
        let title = authoritative
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(track.title.trim())
            .to_owned();
        let uploader = authoritative
            .and_then(|value| {
                [
                    value.pointer("/user/username"),
                    value.pointer("/user/full_name"),
                ]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .find(|name| !name.is_empty())
            })
            .map(str::to_owned);
        let artist_names = uploader
            .map(|uploader| vec![uploader])
            .unwrap_or_else(|| Self::backend_artist_names_for_soundcloud(track));
        let duration_ms = authoritative
            .and_then(|value| {
                [value.get("duration"), value.get("full_duration")]
                    .into_iter()
                    .flatten()
                    .filter_map(|duration| match duration {
                        Value::Number(duration) => duration.as_u64(),
                        Value::String(duration) => duration.trim().parse().ok(),
                        _ => None,
                    })
                    .find(|duration| *duration > 0)
            })
            .unwrap_or_else(|| track.duration.as_millis().min(u64::MAX as u128) as u64);
        (
            authoritative_id.unwrap_or_else(|| track.id.clone()),
            title,
            artist_names,
            duration_ms,
        )
    }

    fn soundcloud_owner_slug(service_url: &str) -> Option<String> {
        let url = reqwest::Url::parse(service_url.trim()).ok()?;
        let host = url.host_str()?;
        let owned_host = host.eq_ignore_ascii_case("soundcloud.com")
            || host.to_ascii_lowercase().ends_with(".soundcloud.com");
        if !url.scheme().eq_ignore_ascii_case("https")
            || !url.port().is_none_or(|port| port == 443)
            || !owned_host
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let segment = url.path_segments()?.next()?;
        let slug = percent_decode_filename(segment).trim().to_owned();
        if slug.is_empty()
            || slug.len() > MAX_SOUNDCLOUD_OWNER_SLUG_LENGTH
            || slug.eq_ignore_ascii_case("sets")
            || slug.eq_ignore_ascii_case("tracks")
            || slug
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        {
            return None;
        }
        Some(slug)
    }

    async fn resolve_soundcloud_backend_download(
        &self,
        track: &PlaybackTrack,
        credentials: Option<&MediaCredentials>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
        capability_only: bool,
    ) -> Result<ResolvedSource, String> {
        let Some(credentials) = credentials else {
            return Err("Murglar returned no HQ source".into());
        };
        let request = Self::backend_request(
            track,
            BackendProvider::SoundCloud,
            None,
            true,
            None,
            include_remote_size,
        );
        if capability_only {
            let outcome = self
                .media_backend
                .probe(request, credentials, cancellation)
                .await;
            return match outcome {
                MediaResolveOutcome::Source(format) => Ok(ResolvedSource {
                    data: SourceData::Inline(Vec::new()),
                    size: 0,
                    deezer_track_id: None,
                    is_soundcloud: true,
                    cache_identity: Some(stable_cache_identity(
                        PlaybackProvider::SoundCloud,
                        &track.id,
                        SourceVariant::BackendSoundCloud,
                        format.format,
                        &format.format_name,
                    )),
                    format: format.format,
                    format_name: format.format_name,
                    declared_bitrate: format.declared_bitrate,
                }),
                MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                    Err("Murglar returned no HQ source".into())
                }
                MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                    Err(error)
                }
                MediaResolveOutcome::Cancelled => Err("Playback request cancelled".into()),
            };
        }
        match self
            .media_backend
            .resolve(request, credentials, cancellation)
            .await
        {
            MediaResolveOutcome::Source(source) => Ok(ResolvedSource::from_backend(source)),
            MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                Err("Murglar returned no HQ source".into())
            }
            MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                Err(error)
            }
            MediaResolveOutcome::Cancelled => Err("Playback request cancelled".into()),
        }
    }

    fn soundcloud_get(
        &self,
        url: String,
        token: Option<&SoundCloudToken>,
    ) -> reqwest::RequestBuilder {
        self.soundcloud_request(self.client.get(url), token)
    }

    fn soundcloud_request(
        &self,
        request: reqwest::RequestBuilder,
        token: Option<&SoundCloudToken>,
    ) -> reqwest::RequestBuilder {
        let request = request.header(header::USER_AGENT, BROWSER_USER_AGENT);
        match token {
            Some(token) => match token.authorization_header() {
                Ok(authorization) => request.header(header::AUTHORIZATION, authorization),
                Err(_) => request,
            },
            None => request,
        }
    }

    async fn download<W>(
        &self,
        remote: RemoteAudio,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        let mut downloaded = 0;
        let remote_url = match remote.data {
            SourceData::Inline(bytes) => {
                if bytes.len() as u64 > MAX_AUDIO_SIZE {
                    return Err("The download is too large".into());
                }
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                output.set_total_hint(Some(bytes.len() as u64));
                output
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "The download could not be written".to_string())?;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(
                        bytes.len() as u64,
                        Some(bytes.len() as u64),
                    ));
                }
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                return Ok(());
            }
            SourceData::Remote(url) => url,
            SourceData::Hls(descriptor) => {
                return soundcloud_hls::download(
                    &self.client,
                    &descriptor,
                    output,
                    cancellation,
                    progress.as_ref(),
                    MAX_AUDIO_SIZE,
                    BROWSER_USER_AGENT,
                )
                .await;
            }
            SourceData::Backend(_) => {
                return Err("The backend source was not handled by its provider boundary".into());
            }
        };
        if remote.size > 0 {
            if remote.size > MAX_AUDIO_SIZE {
                return Err("The download is too large".into());
            }
            output.set_total_hint(Some(remote.size));
            let mut start = 0;
            while start < remote.size {
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                let end = (start + RANGE_CHUNK - 1).min(remote.size - 1);
                let response =
                    send_with_retry("media.range", RequestClass::Media, cancellation, || {
                        self.client
                            .get(&remote_url)
                            .header(header::USER_AGENT, BROWSER_USER_AGENT)
                            .header(header::RANGE, format!("bytes={start}-{end}"))
                            .header(header::ACCEPT_ENCODING, "identity")
                    })
                    .await?;
                validate_media_response_url(response.url(), remote.is_soundcloud)?;
                let status = response.status();
                let content_length = response.content_length();
                if !status.is_success() {
                    return Err(PlaybackDownloadError::media_status(
                        "audio provider",
                        status,
                    ));
                }
                let bytes = response.bytes().await.map_err(request_error)?;
                log_response_diagnostics("audio range", status, content_length);
                let mut bytes = ranged_body(status, &bytes, start, end)?;
                if let Some(track_id) = remote.deezer_track_id.as_deref() {
                    decrypt_stripes(&mut bytes, track_id, start / STRIPE_SIZE as u64)?;
                }
                output
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "The playback buffer could not be written".to_string())?;
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                downloaded += bytes.len() as u64;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(
                        downloaded,
                        (remote.size > 0).then_some(remote.size),
                    ));
                }
                start = end + 1;
            }
        } else {
            let response = send_with_retry("media.full", RequestClass::Media, cancellation, || {
                self.client
                    .get(&remote_url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::RANGE, "bytes=0-")
                    .header(header::ACCEPT_ENCODING, "identity")
            })
            .await?;
            validate_media_response_url(response.url(), remote.is_soundcloud)?;
            if !response.status().is_success() {
                return Err(PlaybackDownloadError::media_status(
                    "audio provider",
                    response.status(),
                ));
            }
            let status = response.status();
            let content_length = response.content_length();
            crate::diagnostics::event(
                "INFO",
                format!("audio full response status={status} content_length={content_length:?}"),
            );
            if response
                .content_length()
                .is_some_and(|size| size > MAX_AUDIO_SIZE)
            {
                return Err("The download is too large".into());
            }
            let total = content_range_total(response.headers()).or(content_length);
            output.set_total_hint(total);
            let mut deezer_stream = remote
                .deezer_track_id
                .as_deref()
                .map(DeezerStripeStream::new)
                .transpose()?;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                let chunk = chunk.map_err(request_error)?;
                downloaded = downloaded.saturating_add(chunk.len() as u64);
                if downloaded > MAX_AUDIO_SIZE {
                    return Err("The download is too large".into());
                }
                if let Some(deezer_stream) = deezer_stream.as_mut() {
                    deezer_stream
                        .write_chunk(&chunk, &mut *output)
                        .await
                        .map_err(|_| "The download could not be written".to_string())?;
                } else {
                    output
                        .write_all(&chunk)
                        .await
                        .map_err(|_| "The download could not be written".to_string())?;
                }
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(downloaded, total));
                }
            }
            if let Some(deezer_stream) = deezer_stream.as_mut() {
                deezer_stream
                    .finish(&mut *output)
                    .await
                    .map_err(|_| "The download could not be written".to_string())?;
            }
            output
                .flush()
                .await
                .map_err(|_| "The playback buffer could not be finalized".to_string())?;
        }
        Ok(())
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
        Some(arl) => format!("arl={}", arl.expose()),
        None => String::new(),
    };
    if let Some(extra) = cookies.filter(|cookies| !cookies.is_empty()) {
        if !cookie.is_empty() {
            cookie.push_str("; ");
        }
        cookie.push_str(extra);
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

fn content_range_total(headers: &header::HeaderMap) -> Option<u64> {
    let value = headers.get(header::CONTENT_RANGE)?.to_str().ok()?;
    let total = value.rsplit_once('/')?.1.trim();
    let total = total.parse::<u64>().ok()?;
    (total > 0).then_some(total)
}

fn cacheable_size(size: u64, max_bytes: u64) -> Option<u64> {
    (size > 0).then_some(size).filter(|size| *size <= max_bytes)
}

fn prefetch_range(size: u64) -> Option<(u64, u64)> {
    (size > 0).then(|| (0, size.min(BLOCK_SIZE) - 1))
}

fn inline_range(bytes: &[u8], start: u64, end: u64) -> Result<Vec<u8>, PlaybackDownloadError> {
    let start = usize::try_from(start)
        .map_err(|_| PlaybackDownloadError::message("The inline source range was invalid"))?;
    let end = usize::try_from(end)
        .map_err(|_| PlaybackDownloadError::message("The inline source range was invalid"))?;
    bytes
        .get(start..=end)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| PlaybackDownloadError::message("The inline source range was unavailable"))
}

fn validate_soundcloud_transcoding_url(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() == "https" && url.host_str() == Some("api-v2.soundcloud.com") {
        Ok(())
    } else {
        Err("SoundCloud returned an unexpected transcoding URL".into())
    }
}

const MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH: usize = 512;

fn soundcloud_track_authorization(track: &Value) -> Option<&str> {
    let raw = track.get("track_authorization").and_then(Value::as_str)?;
    if raw.len() > MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH
        || !raw.is_ascii()
        || raw.chars().any(char::is_control)
    {
        return None;
    }
    let value = raw.trim();
    (!value.is_empty()).then_some(value)
}

fn append_soundcloud_transcoding_query(url: &mut reqwest::Url, track_authorization: Option<&str>) {
    let preserved = url
        .query_pairs()
        .filter(|(key, _)| key != "client_id" && key != "track_authorization")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    let mut query = url.query_pairs_mut();
    for (key, value) in preserved {
        query.append_pair(&key, &value);
    }
    query.append_pair("client_id", SOUNDCLOUD_CLIENT_ID);
    if let Some(track_authorization) = track_authorization {
        query.append_pair("track_authorization", track_authorization);
    }
}

fn validate_soundcloud_stream_url(url: &reqwest::Url) -> Result<(), String> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let allowed = host == "api-v2.soundcloud.com"
        || host == "media.soundcloud.com"
        || host == "sndcdn.com"
        || host.ends_with(".sndcdn.com");
    if url.scheme() == "https" && allowed {
        Ok(())
    } else {
        Err("SoundCloud returned an unexpected stream URL".into())
    }
}

// Logical SoundCloud identity and Murglar/Deezer media transport are intentionally separate.
fn validate_media_response_url(url: &reqwest::Url, is_soundcloud: bool) -> Result<(), String> {
    if is_soundcloud {
        validate_soundcloud_stream_url(url)
    } else {
        Ok(())
    }
}

fn soundcloud_transcodings(transcodings: &[Value]) -> impl Iterator<Item = &Value> {
    transcodings.iter().filter(|item| {
        let protocol = item
            .pointer("/format/protocol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        protocol == "progressive"
            && !is_encrypted_soundcloud_protocol(&protocol)
            && item.get("url").and_then(Value::as_str).is_some()
    })
}

fn soundcloud_playback_transcodings(transcodings: &[Value]) -> Vec<&Value> {
    let mut hls = transcodings
        .iter()
        .filter(|item| is_soundcloud_hls_transcoding(item))
        .collect::<Vec<_>>();
    hls.sort_by_key(|item| Reverse(soundcloud_transcoding_bitrate(item).unwrap_or_default()));
    hls.extend(soundcloud_transcodings(transcodings));
    hls
}

fn is_soundcloud_hls_transcoding(value: &Value) -> bool {
    let protocol = value
        .pointer("/format/protocol")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let mime_type = value
        .pointer("/format/mime_type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let preset = value
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    protocol == "hls"
        && mime_type == "audio/mp4"
        && preset.starts_with("aac_")
        && soundcloud_transcoding_bitrate(value).is_some()
        && value.get("url").and_then(Value::as_str).is_some()
}

fn is_encrypted_soundcloud_protocol(protocol: &str) -> bool {
    ["encrypted", "cbc", "ctr"]
        .iter()
        .any(|term| protocol.contains(term))
}

fn transcoding_format(value: &Value) -> AudioFormat {
    let hint = format!(
        "{} {}",
        value
            .pointer("/format/mime_type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    infer_soundcloud_format(
        &json!({"filename": hint}),
        &reqwest::Url::parse("https://cf-media.sndcdn.com/stream").unwrap(),
    )
}

fn soundcloud_format_name(value: &Value, format: AudioFormat) -> String {
    let hint = format!(
        "{} {}",
        value
            .pointer("/format/mime_type")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        value
            .get("preset")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    if format == AudioFormat::M4a {
        format.label().to_owned()
    } else if hint.contains("aac") {
        "AAC".into()
    } else if hint.contains("opus") {
        "Opus".into()
    } else if hint.contains("ogg") || hint.contains("vorbis") {
        "OGG".into()
    } else if hint.contains("mp3") || hint.contains("mpeg") {
        "MP3".into()
    } else {
        format.label().to_owned()
    }
}

fn soundcloud_transcoding_bitrate(value: &Value) -> Option<u32> {
    value_bitrate(value)
}

fn value_bitrate(value: &Value) -> Option<u32> {
    [
        value.get("bitrate"),
        value.get("quality"),
        value.get("preset"),
        value.get("format"),
        value.get("filename"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|value| match value {
        Value::String(value) => declared_bitrate(value),
        Value::Number(value) => value
            .as_u64()
            .filter(|value| (16..=10_000).contains(value))
            .and_then(|value| u32::try_from(value).ok()),
        _ => None,
    })
    .next()
}

fn declared_bitrate(value: &str) -> Option<u32> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|part| part.parse::<u32>().ok())
        .filter(|value| (16..=10_000).contains(value))
        .next_back()
}

fn soundcloud_original_bitrate(value: &Value, format: AudioFormat) -> Option<u32> {
    let bitrate = value_bitrate(value)?;
    match format {
        AudioFormat::Mp3
        | AudioFormat::OggVorbis
        | AudioFormat::OggOpus
        | AudioFormat::Aac
        | AudioFormat::M4a => (16..=640).contains(&bitrate).then_some(bitrate),
        AudioFormat::Flac | AudioFormat::Wav | AudioFormat::Aiff => Some(bitrate),
    }
}

fn choose_soundcloud_original_format(
    sniffed_format: Option<AudioFormat>,
    header_format: Option<AudioFormat>,
    provider_format: Option<AudioFormat>,
) -> Option<AudioFormat> {
    sniffed_format.or(header_format).or(provider_format)
}

fn soundcloud_original_head_can_fallback(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::FORBIDDEN
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED
    )
}

fn infer_soundcloud_original_format(value: &Value, url: &reqwest::Url) -> Option<AudioFormat> {
    infer_soundcloud_provider_format(value).or_else(|| soundcloud_format_from_url_path(url))
}

fn infer_soundcloud_provider_format(value: &Value) -> Option<AudioFormat> {
    ["original_format", "format", "mime_type", "filename"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(soundcloud_format_from_value))
}

fn soundcloud_format_from_value(value: &Value) -> Option<AudioFormat> {
    match value {
        Value::String(value) => soundcloud_format_from_hint(value),
        Value::Object(_) => [
            "original_format",
            "mime_type",
            "filename",
            "format",
            "extension",
        ]
        .into_iter()
        .find_map(|key| value.get(key).and_then(soundcloud_format_from_value)),
        Value::Array(values) => values.iter().find_map(soundcloud_format_from_value),
        _ => None,
    }
}

fn soundcloud_format_from_hint(value: &str) -> Option<AudioFormat> {
    let value = value.trim().trim_matches('"').trim_matches('\'');
    if value.is_empty() {
        return None;
    }
    if let Some(format) = soundcloud_format_from_mime(value.split(';').next()?.trim()) {
        return Some(format);
    }
    let token = value.to_ascii_lowercase();
    if let Some((_, extension)) = token.rsplit_once('.')
        && let Some(format) = soundcloud_format_from_extension(extension)
    {
        return Some(format);
    }
    if token.contains('/') || token.chars().any(char::is_whitespace) {
        return None;
    }
    match token.as_str() {
        "flac" => Some(AudioFormat::Flac),
        "wav" | "wave" => Some(AudioFormat::Wav),
        "aiff" | "aif" | "aifc" => Some(AudioFormat::Aiff),
        "mp3" | "mpeg" => Some(AudioFormat::Mp3),
        "ogg" | "vorbis" => Some(AudioFormat::OggVorbis),
        "opus" => Some(AudioFormat::OggOpus),
        "aac" | "adts" => Some(AudioFormat::Aac),
        "m4a" | "mp4" => Some(AudioFormat::M4a),
        _ if token.strip_prefix("mp3_").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) =>
        {
            Some(AudioFormat::Mp3)
        }
        _ => token
            .rsplit_once('.')
            .and_then(|(_, extension)| soundcloud_format_from_extension(extension)),
    }
}

fn soundcloud_format_from_mime(value: &str) -> Option<AudioFormat> {
    match value.trim().to_ascii_lowercase().as_str() {
        "audio/flac" | "audio/x-flac" | "application/flac" => Some(AudioFormat::Flac),
        "audio/wav" | "audio/wave" | "audio/x-wav" | "audio/vnd.wave" => Some(AudioFormat::Wav),
        "audio/aiff" | "audio/x-aiff" | "audio/aif" | "audio/x-aif" => Some(AudioFormat::Aiff),
        "audio/mpeg" | "audio/mp3" => Some(AudioFormat::Mp3),
        "audio/ogg" | "application/ogg" => Some(AudioFormat::OggVorbis),
        "audio/opus" => Some(AudioFormat::OggOpus),
        "audio/aac" | "audio/aacp" | "audio/x-aac" => Some(AudioFormat::Aac),
        "audio/mp4" | "video/mp4" | "audio/x-m4a" | "application/mp4" => Some(AudioFormat::M4a),
        _ => None,
    }
}

fn soundcloud_format_from_extension(value: &str) -> Option<AudioFormat> {
    match value.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "flac" => Some(AudioFormat::Flac),
        "wav" | "wave" => Some(AudioFormat::Wav),
        "aiff" | "aif" | "aifc" => Some(AudioFormat::Aiff),
        "mp3" => Some(AudioFormat::Mp3),
        "ogg" | "oga" => Some(AudioFormat::OggVorbis),
        "opus" => Some(AudioFormat::OggOpus),
        "aac" => Some(AudioFormat::Aac),
        "m4a" | "mp4" => Some(AudioFormat::M4a),
        _ => None,
    }
}

fn soundcloud_format_from_url_path(url: &reqwest::Url) -> Option<AudioFormat> {
    let filename = url.path().trim_end_matches('/').rsplit('/').next()?;
    let extension = filename.rsplit_once('.')?.1;
    soundcloud_format_from_extension(extension)
}

fn soundcloud_format_from_media_headers(
    headers: &header::HeaderMap,
    url: &reqwest::Url,
) -> Option<AudioFormat> {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| soundcloud_format_from_mime(value.split(';').next()?.trim()))
        .or_else(|| {
            headers
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .and_then(soundcloud_content_disposition_filename)
                .and_then(|value| soundcloud_format_from_filename(&value))
        })
        .or_else(|| soundcloud_format_from_url_path(url))
}

fn soundcloud_format_from_filename(value: &str) -> Option<AudioFormat> {
    let filename = value
        .trim()
        .trim_matches('"')
        .rsplit(|character| character == '/' || character == '\\')
        .next()?;
    let extension = filename.rsplit_once('.')?.1;
    soundcloud_format_from_extension(extension)
}

fn soundcloud_content_disposition_filename(value: &str) -> Option<String> {
    let mut parameters = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if character == '"' && !escaped {
            quoted = !quoted;
        }
        if character == ';' && !quoted {
            parameters.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
        escaped = character == '\\' && !escaped;
        if character != '\\' {
            escaped = false;
        }
    }
    parameters.push(current);

    for name in ["filename*", "filename"] {
        for parameter in &parameters {
            let Some((key, raw_value)) = parameter.split_once('=') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case(name) {
                continue;
            }
            let filename = raw_value.trim().trim_matches('"');
            let filename = filename
                .split_once("''")
                .map_or(filename, |(_, filename)| filename);
            return Some(percent_decode_filename(filename));
        }
    }
    None
}

fn percent_decode_filename(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (bytes.get(index + 1), bytes.get(index + 2))
            && let (Some(high), Some(low)) = (hex_digit(*high), hex_digit(*low))
        {
            decoded.push(high << 4 | low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn sniff_soundcloud_original_format(bytes: &[u8]) -> Option<AudioFormat> {
    if bytes.starts_with(b"fLaC") {
        return Some(AudioFormat::Flac);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        return Some(AudioFormat::Wav);
    }
    if bytes.len() >= 12
        && bytes.starts_with(b"FORM")
        && (&bytes[8..12] == b"AIFF" || &bytes[8..12] == b"AIFC")
    {
        return Some(AudioFormat::Aiff);
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return Some(AudioFormat::M4a);
    }
    if bytes.starts_with(b"OggS") {
        if bytes
            .windows(b"OpusHead".len())
            .any(|window| window == b"OpusHead")
        {
            return Some(AudioFormat::OggOpus);
        }
        if bytes
            .windows(b"vorbis".len())
            .any(|window| window == b"vorbis")
        {
            return Some(AudioFormat::OggVorbis);
        }
    }
    if bytes.starts_with(b"ID3") || looks_like_aac_adts(bytes) {
        return if bytes.starts_with(b"ID3") {
            Some(AudioFormat::Mp3)
        } else {
            Some(AudioFormat::Aac)
        };
    }
    looks_like_mp3(bytes).then_some(AudioFormat::Mp3)
}

fn looks_like_aac_adts(bytes: &[u8]) -> bool {
    bytes
        .windows(2)
        .any(|window| window[0] == 0xff && (window[1] & 0xf6) == 0xf0)
}

async fn read_bounded_response(
    mut response: Response,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(max_bytes);
    while bytes.len() < max_bytes {
        let chunk = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err("Playback request cancelled".into());
            }
            result = response.chunk() => result.map_err(request_error)?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let remaining = max_bytes - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bytes)
}

fn infer_soundcloud_format(value: &Value, url: &reqwest::Url) -> AudioFormat {
    let hint = format!(
        "{} {url}",
        value
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
    )
    .to_ascii_lowercase();
    if hint.contains("flac") {
        AudioFormat::Flac
    } else if hint.contains("aiff") || hint.contains("aif") {
        AudioFormat::Aiff
    } else if hint.contains("wav") {
        AudioFormat::Wav
    } else if hint.contains("opus") {
        AudioFormat::OggOpus
    } else if hint.contains("ogg") || hint.contains("vorbis") {
        AudioFormat::OggVorbis
    } else if hint.contains("m4a") || hint.contains("audio/mp4") {
        AudioFormat::M4a
    } else if hint.contains("aac") {
        AudioFormat::Aac
    } else {
        AudioFormat::Mp3
    }
}

fn audio_format(value: &str) -> Option<AudioFormat> {
    let value = value.to_ascii_uppercase();
    if value.contains("FLAC") {
        Some(AudioFormat::Flac)
    } else if value.contains("MP3") || value.contains("MPEG") {
        Some(AudioFormat::Mp3)
    } else if value.contains("M4A") || value.contains("MP4") {
        Some(AudioFormat::M4a)
    } else if value.contains("AAC") {
        Some(AudioFormat::Aac)
    } else if value.contains("OPUS") {
        Some(AudioFormat::OggOpus)
    } else if value.contains("OGG") || value.contains("VORBIS") {
        Some(AudioFormat::OggVorbis)
    } else if value.contains("WAV") {
        Some(AudioFormat::Wav)
    } else if value.contains("AIFF") || value.contains("AIFC") {
        Some(AudioFormat::Aiff)
    } else {
        None
    }
}

fn looks_like_mp3(bytes: &[u8]) -> bool {
    bytes.starts_with(b"ID3")
        || bytes.windows(2).take(4096).any(|window| {
            window[0] == 0xff && (window[1] & 0xe0) == 0xe0 && (window[1] & 0x06) != 0
        })
}

fn ranged_body(status: StatusCode, bytes: &[u8], start: u64, end: u64) -> Result<Vec<u8>, String> {
    let expected = (end - start + 1) as usize;
    let offset = if status == StatusCode::PARTIAL_CONTENT {
        0
    } else {
        start as usize
    };
    bytes
        .get(offset..offset + expected)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "The audio provider returned a truncated range".into())
}

async fn read_response_range(
    response: Response,
    start: u64,
    end: u64,
) -> Result<(StatusCode, Option<u64>, Vec<u8>), PlaybackDownloadError> {
    let status = response.status();
    let content_length = response.content_length();
    let expected = (end - start + 1) as usize;
    let mut skip = (status != StatusCode::PARTIAL_CONTENT)
        .then_some(start)
        .unwrap_or(0);
    let mut bytes = Vec::with_capacity(expected);
    let mut stream = response.bytes_stream();
    while bytes.len() < expected {
        let Some(chunk) = stream.next().await else {
            break;
        };
        let chunk = chunk.map_err(request_error)?;
        let skipped = skip.min(chunk.len() as u64) as usize;
        skip -= skipped as u64;
        let chunk = &chunk[skipped..];
        let take = (expected - bytes.len()).min(chunk.len());
        bytes.extend_from_slice(&chunk[..take]);
    }
    if skip != 0 || bytes.len() != expected {
        return Err("The audio provider returned a truncated range".into());
    }
    Ok((status, content_length, bytes))
}

fn aligned_range(start: u64, end: u64, total: u64) -> (u64, u64) {
    let aligned_start = start / STRIPE_SIZE as u64 * STRIPE_SIZE as u64;
    let aligned_end =
        (((end + 1).div_ceil(STRIPE_SIZE as u64) * STRIPE_SIZE as u64) - 1).min(total - 1);
    (aligned_start, aligned_end)
}

fn trim_range(bytes: Vec<u8>, start: u64, end: u64, aligned_start: u64) -> Result<Vec<u8>, String> {
    let skip = (start - aligned_start) as usize;
    let length = (end - start + 1) as usize;
    bytes
        .get(skip..skip + length)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "The audio provider returned a truncated aligned range".into())
}

async fn validate_audio_output(path: &std::path::Path, format: AudioFormat) -> Result<(), String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| "The resolved audio output could not be inspected".to_string())?;
    if metadata.len() == 0 {
        crate::diagnostics::event("ERROR", "audio output failure length=0");
        return Err("The resolved audio output was empty".into());
    }
    if format == AudioFormat::Flac {
        let mut file = File::open(path)
            .await
            .map_err(|_| "The resolved FLAC output could not be inspected".to_string())?;
        let mut header = [0; 4];
        if file.read_exact(&mut header).await.is_err() {
            crate::diagnostics::event(
                "ERROR",
                format!("audio output failure length={}", metadata.len()),
            );
            return Err("The resolved FLAC output was truncated".into());
        }
        if &header != b"fLaC" {
            crate::diagnostics::event(
                "ERROR",
                format!("audio output failure length={}", metadata.len()),
            );
            return Err("The resolved FLAC output did not start with fLaC".into());
        }
    }
    Ok(())
}

fn validate_progressive_prefix(path: &std::path::Path, format: AudioFormat) -> Result<(), String> {
    if format != AudioFormat::Flac {
        return Ok(());
    }
    let mut file = std::fs::File::open(path)
        .map_err(|_| "The resolved FLAC output could not be inspected".to_string())?;
    let mut header = [0; 4];
    file.read_exact(&mut header)
        .map_err(|_| "The resolved FLAC output was truncated".to_string())?;
    if &header != b"fLaC" {
        return Err("The resolved FLAC output did not start with fLaC".into());
    }
    Ok(())
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

fn normalize_release_date(value: &str) -> Option<String> {
    let date = value.trim().split(['T', ' ']).next()?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
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

fn deezer_key(track_id: &str) -> [u8; 16] {
    let hex = format!("{:x}", Md5::digest(track_id.as_bytes()));
    let bytes = hex.as_bytes();
    let mut key = [0; 16];
    for index in 0..16 {
        key[index] = bytes[index] ^ bytes[index + 16] ^ DEEZER_SECRET[index];
    }
    key
}

fn decrypt_stripes(bytes: &mut [u8], track_id: &str, first_stripe: u64) -> Result<(), String> {
    let key = deezer_key(track_id);
    let cipher = Blowfish::new_from_slice(&key)
        .map_err(|_| "The Deezer stream key was invalid".to_string())?;
    for (offset, stripe) in bytes.chunks_mut(STRIPE_SIZE).enumerate() {
        if (first_stripe + offset as u64).is_multiple_of(3) && stripe.len() == STRIPE_SIZE {
            decrypt_cbc(&cipher, stripe);
        }
    }
    Ok(())
}

struct DeezerStripeStream {
    cipher: Blowfish,
    pending: Vec<u8>,
    output: Vec<u8>,
    next_stripe: u64,
}

impl DeezerStripeStream {
    fn new(track_id: &str) -> Result<Self, String> {
        let key = deezer_key(track_id);
        let cipher = Blowfish::new_from_slice(&key)
            .map_err(|_| "The Deezer stream key was invalid".to_string())?;
        Ok(Self {
            cipher,
            pending: Vec::with_capacity(STRIPE_SIZE),
            output: Vec::with_capacity(DEEZER_STREAM_WRITE_BUFFER_SIZE + STRIPE_SIZE),
            next_stripe: 0,
        })
    }

    async fn write_chunk<W>(&mut self, mut chunk: &[u8], output: &mut W) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        while !chunk.is_empty() {
            let needed = STRIPE_SIZE - self.pending.len();
            let take = needed.min(chunk.len());
            self.pending.extend_from_slice(&chunk[..take]);
            chunk = &chunk[take..];
            if self.pending.len() != STRIPE_SIZE {
                continue;
            }
            if self.next_stripe.is_multiple_of(3) {
                decrypt_cbc(&self.cipher, &mut self.pending);
            }
            self.output.extend_from_slice(&self.pending);
            self.pending.clear();
            self.next_stripe += 1;
            if self.output.len() >= DEEZER_STREAM_WRITE_BUFFER_SIZE {
                output.write_all(&self.output).await?;
                self.output.clear();
            }
        }
        Ok(())
    }

    async fn finish<W>(&mut self, output: &mut W) -> std::io::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        if !self.pending.is_empty() {
            self.output.extend_from_slice(&self.pending);
            self.pending.clear();
        }
        if !self.output.is_empty() {
            output.write_all(&self.output).await?;
            self.output.clear();
        }
        Ok(())
    }
}

fn decrypt_cbc(cipher: &Blowfish, bytes: &mut [u8]) {
    let mut previous = [0, 1, 2, 3, 4, 5, 6, 7];
    for block in bytes.chunks_exact_mut(8) {
        let encrypted: [u8; 8] = block.try_into().expect("exact block");
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
        for index in 0..8 {
            block[index] ^= previous[index];
        }
        previous = encrypted;
    }
}

#[cfg(test)]
mod backend_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::PlaybackContext;
    use blowfish::cipher::BlockEncrypt;

    struct TestBackendSource {
        metadata: super::super::media_source::BackendSourceMetadata,
    }

    impl super::super::media_source::BackendSourceOps for TestBackendSource {
        fn metadata(&self) -> super::super::media_source::BackendSourceMetadata {
            self.metadata.clone()
        }

        fn timeline_seek_session(
            &self,
            _cancellation: CancellationToken,
        ) -> Option<Arc<dyn TimelineSeekSession>> {
            None
        }

        fn read_range<'a>(
            &'a self,
            _start: u64,
            _end: u64,
            _cancellation: &'a CancellationToken,
        ) -> super::super::media_source::BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>>
        {
            Box::pin(async { Err(PlaybackDownloadError::message("test backend source")) })
        }

        fn probe_size<'a>(
            &'a self,
            _cancellation: &'a CancellationToken,
        ) -> super::super::media_source::BackendFuture<'a, Option<u64>> {
            Box::pin(async { None })
        }

        fn download<'a>(
            &'a self,
            _output: &'a mut dyn DownloadOutput,
            _cancellation: &'a CancellationToken,
            _progress: Option<&'a ProgressCallback>,
        ) -> super::super::media_source::BackendFuture<'a, Result<(), PlaybackDownloadError>>
        {
            Box::pin(async { Err(PlaybackDownloadError::message("test backend source")) })
        }
    }

    fn cache_test_backend_source(cache_identity: String) -> ResolvedSource {
        let source = BackendSource::from_ops(Arc::new(TestBackendSource {
            metadata: super::super::media_source::BackendSourceMetadata {
                format: AudioFormat::Flac,
                format_name: "FLAC".into(),
                size: 4,
                declared_bitrate: None,
                duration: None,
                timeline: false,
                cacheable: true,
                initial_buffered_fraction: None,
                deezer_track_id: None,
                provenance: super::super::media_source::BackendProvenance::Deezer,
                cache_identity: super::super::media_source::BackendCacheIdentity::new(
                    cache_identity,
                ),
            },
        }));
        let metadata = source.metadata();
        ResolvedSource {
            data: SourceData::Backend(source),
            size: metadata.size,
            deezer_track_id: metadata.deezer_track_id,
            is_soundcloud: false,
            cache_identity: Some(metadata.cache_identity.as_str().to_owned()),
            format: metadata.format,
            format_name: metadata.format_name,
            declared_bitrate: metadata.declared_bitrate,
        }
    }

    #[test]
    fn deezer_playback_identity_prefers_complete_fallback() {
        let response = json!({
            "results": {
                "DATA": {
                    "SNG_ID": "469884852",
                    "TRACK_TOKEN": "unavailable-root-token",
                    "FALLBACK": {
                        "SNG_ID": "2576902492",
                        "TRACK_TOKEN": "playable-fallback-token"
                    }
                }
            }
        });

        assert_eq!(
            deezer_playback_identity(&response, "469884852"),
            Ok(DeezerPlaybackIdentity {
                source_track_id: "2576902492".into(),
                track_token: "playable-fallback-token".into(),
                used_fallback: true,
            })
        );
    }

    #[test]
    fn deezer_playback_identity_accepts_numeric_fallback_id() {
        let response = json!({
            "results": {
                "SNG_ID": "original",
                "TRACK_TOKEN": "root-token",
                "FALLBACK": {
                    "SNG_ID": 2576902492_u64,
                    "TRACK_TOKEN": "fallback-token"
                }
            }
        });

        assert_eq!(
            deezer_playback_identity(&response, "requested").unwrap(),
            DeezerPlaybackIdentity {
                source_track_id: "2576902492".into(),
                track_token: "fallback-token".into(),
                used_fallback: true,
            }
        );
    }

    #[test]
    fn deezer_murglar_fallback_candidate_requires_a_distinct_complete_fallback() {
        let fallback = DeezerPlaybackIdentity {
            source_track_id: "2576902492".into(),
            track_token: "fallback-token".into(),
            used_fallback: true,
        };
        assert_eq!(
            deezer_fallback_track_id(&fallback, "469884852"),
            Some("2576902492")
        );

        let same_id = DeezerPlaybackIdentity {
            source_track_id: "469884852".into(),
            ..fallback
        };
        assert_eq!(deezer_fallback_track_id(&same_id, "469884852"), None);

        let root = DeezerPlaybackIdentity {
            source_track_id: "469884852".into(),
            track_token: "root-token".into(),
            used_fallback: false,
        };
        assert_eq!(deezer_fallback_track_id(&root, "469884852"), None);
    }

    #[test]
    fn deezer_playback_identity_ignores_incomplete_fallback() {
        for fallback in [
            json!({ "SNG_ID": "fallback" }),
            json!({ "TRACK_TOKEN": "fallback-token" }),
            json!({ "SNG_ID": "  ", "TRACK_TOKEN": "fallback-token" }),
            json!({ "SNG_ID": "fallback", "TRACK_TOKEN": "  " }),
        ] {
            let response = json!({
                "results": {
                    "SNG_ID": "root",
                    "TRACK_TOKEN": "root-token",
                    "FALLBACK": fallback
                }
            });
            assert_eq!(
                deezer_playback_identity(&response, "requested").unwrap(),
                DeezerPlaybackIdentity {
                    source_track_id: "root".into(),
                    track_token: "root-token".into(),
                    used_fallback: false,
                }
            );
        }
    }

    #[test]
    fn deezer_playback_identity_uses_requested_id_when_root_id_is_absent() {
        let response = json!({ "results": { "TRACK_TOKEN": "root-token" } });

        assert_eq!(
            deezer_playback_identity(&response, "requested").unwrap(),
            DeezerPlaybackIdentity {
                source_track_id: "requested".into(),
                track_token: "root-token".into(),
                used_fallback: false,
            }
        );
    }

    #[test]
    fn deezer_playback_identity_preserves_missing_token_error() {
        let response = json!({
            "results": {
                "DATA": {
                    "SNG_ID": "469884852",
                    "FALLBACK": { "SNG_ID": "2576902492" }
                }
            }
        });

        assert_eq!(
            deezer_playback_identity(&response, "469884852"),
            Err("Deezer did not return a track token".into())
        );
    }

    #[test]
    fn deezer_unavailable_metadata_is_classified_without_exposing_provider_data() {
        assert!(deezer_track_is_unavailable(&json!({
            "results": {
                "DATA": {
                    "READABLE": false,
                    "TRACK_TOKEN": "redacted"
                }
            }
        })));
        assert!(deezer_track_is_unavailable(&json!({
            "results": {
                "DATA": {
                    "readable": true,
                    "available_countries": []
                }
            }
        })));
        assert!(!deezer_track_is_unavailable(&json!({
            "results": {
                "DATA": {
                    "READABLE": true,
                    "AVAILABLE_COUNTRIES": ["IT"]
                }
            }
        })));
        assert!(!deezer_track_is_unavailable(&json!({
            "results": {
                "DATA": {
                    "READABLE": false,
                    "FALLBACK": {
                        "SNG_ID": "2576902492",
                        "TRACK_TOKEN": "playable-fallback-token"
                    }
                }
            }
        })));
    }

    #[test]
    fn capability_dedup_merges_soundcloud_sources_by_codec_and_bitrate() {
        let original = StreamResolver::capability_dedup_key(
            PlaybackProvider::SoundCloud,
            DownloadVariant::Original,
            "MP3",
            Some(128),
        );
        let murglar = StreamResolver::capability_dedup_key(
            PlaybackProvider::SoundCloud,
            DownloadVariant::Murglar,
            "MP3",
            Some(128),
        );
        let standard = StreamResolver::capability_dedup_key(
            PlaybackProvider::SoundCloud,
            DownloadVariant::Standard,
            "MP3",
            Some(128),
        );
        assert_eq!(original, murglar);
        assert_eq!(murglar, standard);
        assert_eq!(
            original,
            StreamResolver::capability_dedup_key(
                PlaybackProvider::SoundCloud,
                DownloadVariant::Standard,
                "MP3_128",
                Some(128),
            )
        );
        assert_ne!(
            original,
            StreamResolver::capability_dedup_key(
                PlaybackProvider::SoundCloud,
                DownloadVariant::Murglar,
                "MP3",
                Some(320),
            )
        );
    }

    #[test]
    fn capability_dedup_can_merge_equivalent_deezer_formats() {
        let murglar = StreamResolver::capability_dedup_key(
            PlaybackProvider::Deezer,
            DownloadVariant::DeezerMp3_320,
            "MP3_320",
            Some(320),
        );
        let direct = StreamResolver::capability_dedup_key(
            PlaybackProvider::Deezer,
            DownloadVariant::Best,
            "FLAC",
            None,
        );
        let flac = StreamResolver::capability_dedup_key(
            PlaybackProvider::Deezer,
            DownloadVariant::DeezerFlac,
            "FLAC",
            None,
        );
        assert_eq!(
            StreamResolver::capability_dedup_key(
                PlaybackProvider::Deezer,
                DownloadVariant::DeezerMp3_320,
                "MP3_320",
                Some(320),
            ),
            murglar
        );
        assert_eq!(direct, flac);
    }

    #[tokio::test]
    async fn direct_deezer_capability_requests_are_single_flight() {
        let cancellation = CancellationToken::new();
        let first = StreamResolver::acquire_direct_deezer_capability_slot(&cancellation)
            .await
            .unwrap();
        let second = tokio::time::timeout(
            Duration::from_millis(10),
            StreamResolver::acquire_direct_deezer_capability_slot(&cancellation),
        )
        .await;
        assert!(second.is_err());
        drop(first);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                StreamResolver::acquire_direct_deezer_capability_slot(&cancellation),
            )
            .await
            .is_ok()
        );
    }

    #[test]
    fn capability_probe_skips_unauthenticated_direct_deezer_fallbacks() {
        let arl = DeezerArl::from_saved("arl").unwrap();
        assert!(should_skip_direct_deezer_capability(false, None));
        assert!(!should_skip_direct_deezer_capability(false, Some(&arl)));
        assert!(!should_skip_direct_deezer_capability(true, None));
    }

    #[test]
    fn direct_deezer_capability_absence_is_retryable_only_for_transport_errors() {
        assert!(is_expected_capability_absence(
            DownloadVariant::DeezerMp3_128,
            "No Deezer direct source available without an ARL"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::DeezerMp3_128,
            "Murglar request rate limited"
        ));
    }

    #[test]
    fn partial_capability_results_do_not_hide_unexpected_failures() {
        assert!(has_unexpected_capability_failure(&[
            (
                DownloadVariant::DeezerMp3_128,
                "No Deezer direct source available without an ARL".into()
            ),
            (
                DownloadVariant::DeezerFlac,
                "Murglar request rate limited".into()
            ),
        ]));
        assert!(!has_unexpected_capability_failure(&[(
            DownloadVariant::DeezerMp3_128,
            "No Deezer direct source available without an ARL".into()
        ),]));
    }

    #[test]
    fn capability_variants_follow_credentials_and_provider_metadata() {
        let mut soundcloud = PlaybackTrack {
            provider: PlaybackProvider::SoundCloud,
            id: "42".into(),
            title: "title".into(),
            artist: "artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        };
        assert_eq!(
            StreamResolver::capability_variants(&soundcloud, false, true, true),
            vec![
                DownloadVariant::Original,
                DownloadVariant::Murglar,
                DownloadVariant::Standard
            ]
        );
        soundcloud.downloadable = true;
        soundcloud.progressive = true;
        assert_eq!(
            StreamResolver::capability_variants(&soundcloud, false, true, true),
            vec![
                DownloadVariant::Original,
                DownloadVariant::Murglar,
                DownloadVariant::Standard
            ]
        );

        let deezer = PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            ..soundcloud.clone()
        };
        assert_ne!(
            StreamResolver::resolved_source_cache_key(&deezer, false, false),
            StreamResolver::resolved_source_cache_key(&deezer, false, true)
        );
        assert_eq!(
            StreamResolver::resolved_source_cache_key(&deezer, false, false).as_deref(),
            Some("deezer:42:murglar=false")
        );
        assert_ne!(
            StreamResolver::resolved_source_cache_key(&soundcloud, false, false),
            StreamResolver::resolved_source_cache_key(&soundcloud, true, false)
        );
        assert_ne!(
            StreamResolver::resolved_source_cache_key(&soundcloud, true, false),
            StreamResolver::resolved_source_cache_key(&soundcloud, true, true)
        );
        soundcloud.downloadable = false;
        let not_downloadable = StreamResolver::resolved_source_cache_key(&soundcloud, true, false);
        soundcloud.downloadable = true;
        assert_ne!(
            not_downloadable,
            StreamResolver::resolved_source_cache_key(&soundcloud, true, false)
        );
        let expected = [
            (false, false, Vec::<DownloadVariant>::new()),
            (true, false, vec![DownloadVariant::DeezerMp3_128]),
            (
                false,
                true,
                vec![DownloadVariant::DeezerFlac, DownloadVariant::DeezerMp3_320],
            ),
            (
                true,
                true,
                vec![
                    DownloadVariant::DeezerFlac,
                    DownloadVariant::DeezerMp3_320,
                    DownloadVariant::DeezerMp3_128,
                ],
            ),
        ];
        for (deezer_arl, murglar_token, variants) in expected {
            assert_eq!(
                StreamResolver::capability_variants(&deezer, deezer_arl, false, murglar_token,),
                variants,
                "deezer_arl={deezer_arl} murglar={murglar_token}",
            );
        }
    }

    #[test]
    fn source_cache_provenance_keeps_direct_fallback_out_of_murglar_key() {
        let track = PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: "42".into(),
            title: "title".into(),
            artist: "artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        };
        let direct = cache_test_source(
            SourceData::Remote("https://cdn.example.test/direct".into()),
            stable_cache_identity(
                PlaybackProvider::Deezer,
                "42",
                SourceVariant::Deezer,
                AudioFormat::Mp3,
                "MP3_320",
            ),
        );
        let murglar = cache_test_backend_source(stable_cache_identity(
            PlaybackProvider::Deezer,
            "42",
            SourceVariant::BackendDeezer,
            AudioFormat::Flac,
            "FLAC",
        ));
        assert_eq!(
            StreamResolver::resolved_source_cache_key_for_source(&track, true, &direct).as_deref(),
            Some("deezer:42:murglar=false")
        );
        assert_eq!(
            StreamResolver::resolved_source_cache_key_for_source(&track, true, &murglar).as_deref(),
            Some("deezer:42:murglar=true")
        );
    }

    #[tokio::test]
    async fn murglar_eligible_playback_reuses_a_cached_direct_fallback() {
        let resolver = StreamResolver::new().unwrap();
        let track = PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: "cached-direct".into(),
            title: "title".into(),
            artist: "artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        };
        let source = cache_test_source(
            SourceData::Remote("https://cdn.example.test/direct".into()),
            stable_cache_identity(
                PlaybackProvider::Deezer,
                &track.id,
                SourceVariant::Deezer,
                AudioFormat::Mp3,
                "MP3_128",
            ),
        );
        let key = StreamResolver::resolved_source_cache_key(&track, false, false).unwrap();
        resolver.resolved_source_cache.insert(key, source.clone());
        let epoch = resolver.resolved_source_cache.epoch();
        let resolved = resolver
            .resolve_playback_source(
                &track,
                None,
                None,
                Some(crate::murglar_backend::test_media_credentials()),
                CancellationToken::new(),
                false,
                epoch,
            )
            .await
            .unwrap();
        assert_eq!(resolved.cache_identity, source.cache_identity);
        assert!(!resolved.uses_backend());
    }

    #[test]
    fn clearing_resolved_sources_prevents_cross_account_reuse() {
        let resolver = StreamResolver::new().unwrap();
        let source = cache_test_source(
            SourceData::Remote("https://cdn.example.test/audio".into()),
            "source-cache-test".into(),
        );
        resolver
            .resolved_source_cache
            .insert("account-scoped", source);
        assert!(
            resolver
                .resolved_source_cache
                .get("account-scoped")
                .is_some()
        );

        resolver.clear_resolved_source_cache();

        assert!(
            resolver
                .resolved_source_cache
                .get("account-scoped")
                .is_none()
        );
    }

    #[test]
    fn stale_resolution_cannot_reinsert_after_account_clear() {
        let resolver = StreamResolver::new().unwrap();
        let source = cache_test_source(
            SourceData::Remote("https://cdn.example.test/audio".into()),
            "stale-source".into(),
        );
        let stale_epoch = resolver.resolved_source_cache.epoch();
        resolver.clear_resolved_source_cache();

        assert!(!resolver.insert_resolved_source_if_current(
            stale_epoch,
            &CancellationToken::new(),
            "stale-key".into(),
            source,
        ));
        assert!(resolver.resolved_source_cache.get("stale-key").is_none());
    }

    #[test]
    fn arl_only_capability_probe_avoids_failed_high_quality_paths() {
        let track = PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: "logged-track".into(),
            title: "title".into(),
            artist: "artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        };
        let variants = StreamResolver::capability_variants(&track, true, false, false);
        assert_eq!(variants, vec![DownloadVariant::DeezerMp3_128]);
        assert!(!variants.contains(&DownloadVariant::DeezerFlac));
        assert!(!variants.contains(&DownloadVariant::DeezerMp3_320));
    }

    #[test]
    fn content_range_total_accepts_partial_and_unsatisfied_ranges() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::CONTENT_RANGE, "bytes 0-0/123456".parse().unwrap());
        assert_eq!(content_range_total(&headers), Some(123_456));
        headers.insert(header::CONTENT_RANGE, "bytes */654321".parse().unwrap());
        assert_eq!(content_range_total(&headers), Some(654_321));
    }

    #[test]
    fn content_range_total_rejects_unknown_or_invalid_totals() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::CONTENT_RANGE, "bytes 0-0/*".parse().unwrap());
        assert_eq!(content_range_total(&headers), None);
        headers.insert(header::CONTENT_RANGE, "bytes 0-0/0".parse().unwrap());
        assert_eq!(content_range_total(&headers), None);
        headers.insert(header::CONTENT_RANGE, "not-a-range".parse().unwrap());
        assert_eq!(content_range_total(&headers), None);
    }

    #[test]
    fn remote_size_prefers_content_range_total_and_falls_back_to_success_length() {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::CONTENT_RANGE, "bytes 0-0/123456".parse().unwrap());
        assert_eq!(
            remote_size_from_response(StatusCode::PARTIAL_CONTENT, &headers, Some(1)),
            Some(123_456)
        );
        headers.clear();
        assert_eq!(
            remote_size_from_response(StatusCode::PARTIAL_CONTENT, &headers, Some(1)),
            None
        );
        headers.clear();
        assert_eq!(
            remote_size_from_response(StatusCode::OK, &headers, Some(987)),
            Some(987)
        );
        assert_eq!(
            remote_size_from_response(StatusCode::RANGE_NOT_SATISFIABLE, &headers, Some(987)),
            None
        );
    }

    #[test]
    fn unknown_or_over_limit_sources_bypass_cache() {
        assert_eq!(cacheable_size(0, 512), None);
        assert_eq!(cacheable_size(513, 512), None);
        assert_eq!(cacheable_size(512, 512), Some(512));
    }

    #[tokio::test]
    async fn unknown_size_completion_persists_total_and_reuses_cached_blocks() {
        let temp = tempfile::tempdir().unwrap();
        let cache = AudioCache::new(temp.path().into(), 256);
        let key = "deezer:track-42";
        let bytes = b"cached progressive audio";
        let total = bytes.len() as u64;
        let generation = cache.generation();

        cache.remember_total(key, total, generation).await;
        cache_bytes(&cache, key, total, bytes, generation, None).await;

        assert_eq!(cache.known_total(key).await, Some(total));
        assert!(cache.is_fully_cached(key, total).await);
        let path = cache.block_path(key, total, 0, total - 1);
        assert_eq!(
            cache.read(&path, bytes.len()).await.as_deref(),
            Some(bytes.as_slice())
        );
    }

    #[tokio::test]
    async fn progressive_cache_promotion_copies_multiple_blocks_without_snapshotting() {
        let temp = tempfile::tempdir().unwrap();
        let cache = AudioCache::new(temp.path().into(), 8);
        let total = BLOCK_SIZE + 37;
        let file = ProgressiveFile::new(AudioFormat::Flac, Some(total)).unwrap();
        let mut writer = file.writer().unwrap();
        let chunk = vec![b'a'; 64 * 1024];
        for _ in 0..(BLOCK_SIZE as usize / chunk.len()) {
            writer.write_all(&chunk).await.unwrap();
        }
        writer.write_all(&[b'b'; 37]).await.unwrap();
        writer.finish().await.unwrap();

        let generation = cache.generation();
        assert!(cache_writer_blocks(&cache, "dash-multi", total, &writer, generation, None).await);
        cache.remember_total("dash-multi", total, generation).await;
        assert!(cache.is_fully_cached("dash-multi", total).await);
        let first = cache.block_path("dash-multi", total, 0, BLOCK_SIZE - 1);
        let second = cache.block_path("dash-multi", total, BLOCK_SIZE, total - 1);
        assert_eq!(
            cache.read(&first, BLOCK_SIZE as usize).await.unwrap().len(),
            BLOCK_SIZE as usize
        );
        let second_bytes = cache.read(&second, 37).await.unwrap();
        assert_eq!(second_bytes.as_slice(), &[b'b'; 37]);
    }

    fn cache_test_source(data: SourceData, cache_identity: String) -> ResolvedSource {
        ResolvedSource {
            data,
            size: 4,
            deezer_track_id: None,
            is_soundcloud: true,
            cache_identity: Some(cache_identity),
            format: AudioFormat::Mp3,
            format_name: "MP3".into(),
            declared_bitrate: None,
        }
    }

    #[test]
    fn download_choice_text_uses_provider_format_and_collection_quality_detail() {
        let source = ResolvedSource {
            data: SourceData::Inline(Vec::new()),
            size: 0,
            deezer_track_id: None,
            is_soundcloud: true,
            cache_identity: None,
            format: AudioFormat::Mp3,
            format_name: "MP3".into(),
            declared_bitrate: Some(320),
        };
        let (label, detail) = download_choice_text(DownloadVariant::Murglar, &source);
        assert_eq!(label, "MP3 320 kbps");
        assert_eq!(detail, "High quality");

        let (label, detail) = download_choice_text(DownloadVariant::Original, &source);
        assert_eq!(label, "MP3 320 kbps");
        assert_eq!(detail, "Original quality");

        let source = ResolvedSource {
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            declared_bitrate: None,
            ..source
        };
        let (label, detail) = download_choice_text(DownloadVariant::Murglar, &source);
        assert_eq!(label, "FLAC");
        assert_eq!(detail, "Lossless quality");

        let (label, detail) = download_choice_text(DownloadVariant::Standard, &source);
        assert_eq!(label, "FLAC");
        assert_eq!(detail, "Standard quality");
    }

    #[test]
    fn deezer_download_choice_text_uses_collection_quality_details() {
        let sources = [
            (
                DownloadVariant::DeezerFlac,
                AudioFormat::Flac,
                "FLAC",
                None,
                "FLAC",
                "Lossless quality",
            ),
            (
                DownloadVariant::DeezerMp3_320,
                AudioFormat::Mp3,
                "MP3",
                Some(320),
                "MP3 320 kbps",
                "High quality",
            ),
            (
                DownloadVariant::DeezerMp3_128,
                AudioFormat::Mp3,
                "MP3",
                Some(128),
                "MP3 128 kbps",
                "Standard quality",
            ),
        ];

        for (variant, format, format_name, declared_bitrate, label, detail) in sources {
            let source = ResolvedSource {
                data: SourceData::Inline(Vec::new()),
                size: 0,
                deezer_track_id: Some("track".into()),
                is_soundcloud: false,
                cache_identity: None,
                format,
                format_name: format_name.into(),
                declared_bitrate,
            };
            assert_eq!(
                download_choice_text(variant, &source),
                (label.into(), detail.into())
            );
        }
    }

    #[test]
    fn download_choice_text_never_calls_an_unknown_soundcloud_source_flac() {
        let source = ResolvedSource {
            data: SourceData::Inline(Vec::new()),
            size: 0,
            deezer_track_id: None,
            is_soundcloud: true,
            cache_identity: None,
            format: AudioFormat::Mp3,
            format_name: "MP3".into(),
            declared_bitrate: None,
        };
        let (label, _) = download_choice_text(DownloadVariant::Standard, &source);
        assert_eq!(label, "MP3");
        assert_ne!(label, "FLAC");
    }

    #[test]
    fn capability_absence_classification_keeps_empty_results_cacheable() {
        assert!(is_expected_capability_absence(
            DownloadVariant::Standard,
            "No active stream link found for this SoundCloud track"
        ));
        assert!(is_expected_capability_absence(
            DownloadVariant::Murglar,
            "Murglar returned no HQ source"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Standard,
            "The playback request timed out"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Murglar,
            "Murglar API request was rejected (HTTP 429 Too Many Requests)"
        ));
        assert!(is_expected_capability_absence(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 403 Forbidden)"
        ));
        assert!(is_expected_capability_absence(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 404 Not Found)"
        ));
        assert!(is_expected_capability_absence(
            DownloadVariant::Original,
            SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Standard,
            "SoundCloud source request was rejected (HTTP 403 Forbidden)"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 401 Unauthorized)"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 429 Too Many Requests)"
        ));
        assert!(!is_expected_capability_absence(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 503 Service Unavailable)"
        ));
        assert!(!has_unexpected_capability_failure(&[
            (
                DownloadVariant::Original,
                "SoundCloud source request was rejected (HTTP 403 Forbidden)".into()
            ),
            (
                DownloadVariant::Original,
                SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN.into()
            ),
        ]));
        assert!(has_unexpected_capability_failure(&[(
            DownloadVariant::Original,
            "SoundCloud source request was rejected (HTTP 503 Service Unavailable)".into()
        ),]));
        assert!(is_soundcloud_candidate_absence(
            "SoundCloud source request was rejected (HTTP 403 Forbidden)"
        ));
        assert!(is_soundcloud_candidate_absence(
            "SoundCloud source request was rejected (HTTP 404 Not Found)"
        ));
        for error in [
            "SoundCloud source request was rejected (HTTP 401 Unauthorized)",
            "SoundCloud source request was rejected (HTTP 429 Too Many Requests)",
            "SoundCloud source request was rejected (HTTP 503 Service Unavailable)",
            "The soundcloud.transcoding playback request could not be reached",
            "The soundcloud.transcoding provider returned an invalid playback response",
        ] {
            assert!(!is_soundcloud_candidate_absence(error), "{error}");
        }
    }

    #[test]
    fn soundcloud_cache_identity_ignores_signed_url_but_separates_variants() {
        let standard = stable_cache_identity(
            PlaybackProvider::SoundCloud,
            "track-42",
            SourceVariant::SoundCloudStandard,
            AudioFormat::Mp3,
            "MP3",
        );
        let hq = stable_cache_identity(
            PlaybackProvider::SoundCloud,
            "track-42",
            SourceVariant::BackendSoundCloud,
            AudioFormat::Flac,
            "FLAC",
        );
        let first = cache_key(&cache_test_source(
            SourceData::Remote("https://cdn.example/track?expires=1".into()),
            standard.clone(),
        ));
        let second = cache_key(&cache_test_source(
            SourceData::Remote("https://cdn.example/track?expires=2".into()),
            standard,
        ));
        assert_eq!(first, second);
        assert_ne!(
            first,
            cache_key(&cache_test_source(
                SourceData::Remote("https://cdn.example/track?expires=3".into()),
                hq,
            ))
        );
        let source = cache_test_source(
            SourceData::Remote("https://cdnt-stream.dzcdn.net/media/file.flac".into()),
            stable_cache_identity(
                PlaybackProvider::SoundCloud,
                "track-42",
                SourceVariant::BackendSoundCloud,
                AudioFormat::Flac,
                "FLAC",
            ),
        );
        assert_eq!(playback_source_label(&source), "soundcloud");
        assert!(cache_key(&source).unwrap().contains(":murglar:"));
    }

    #[test]
    fn playback_source_label_prefers_logical_soundcloud_over_decryption_metadata() {
        let soundcloud_murglar = ResolvedSource {
            data: SourceData::Remote("https://cdnt-stream.dzcdn.net/media/42/file.flac".into()),
            size: 0,
            deezer_track_id: Some("42".into()),
            is_soundcloud: true,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::SoundCloud,
                "soundcloud-track",
                SourceVariant::BackendSoundCloud,
                AudioFormat::Flac,
                "FLAC",
            )),
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            declared_bitrate: None,
        };
        assert!(!soundcloud_murglar.uses_backend());
        assert_eq!(playback_source_label(&soundcloud_murglar), "soundcloud");

        let deezer = ResolvedSource {
            data: SourceData::Remote("https://cdnt-stream.dzcdn.net/media/42/file.flac".into()),
            size: 0,
            deezer_track_id: Some("42".into()),
            is_soundcloud: false,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::Deezer,
                "42",
                SourceVariant::BackendDeezer,
                AudioFormat::Flac,
                "FLAC",
            )),
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            declared_bitrate: None,
        };
        assert_eq!(playback_source_label(&deezer), "deezer");
    }

    #[test]
    fn inline_cache_ranges_are_sized_and_sliced_without_network() {
        assert_eq!(cacheable_size(4, 512), Some(4));
        assert_eq!(inline_range(b"012345", 1, 3).unwrap(), b"123");
        assert!(inline_range(b"012345", 6, 6).is_err());
    }

    #[tokio::test]
    async fn source_size_for_info_uses_inline_payload_length() {
        let resolver = StreamResolver::new().unwrap();
        let source = ResolvedSource {
            data: SourceData::Inline(vec![1, 2, 3, 4]),
            size: 0,
            deezer_track_id: None,
            is_soundcloud: false,
            cache_identity: None,
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            declared_bitrate: None,
        };
        assert_eq!(
            resolver
                .source_size_for_info(&source, &CancellationToken::new())
                .await
                .unwrap(),
            4
        );
    }

    #[cfg(ralgrum_private_backend)]
    #[cfg(ralgrum_private_backend)]
    #[test]
    fn soundcloud_uses_only_progressive_transcodings_in_provider_order() {
        let values = vec![
            json!({"url":"https://api-v2.soundcloud.com/media/encrypted","format":{"protocol":"encrypted-hls"}}),
            json!({"url":"https://api-v2.soundcloud.com/media/hls","format":{"protocol":"hls"}}),
            json!({"url":"https://api-v2.soundcloud.com/media/progressive","format":{"protocol":"progressive"}}),
            json!({"url":"https://api-v2.soundcloud.com/media/cbc","format":{"protocol":"hls-cbc"}}),
        ];
        assert_eq!(
            soundcloud_transcodings(&values).collect::<Vec<_>>(),
            vec![&values[2]]
        );
        assert!(
            validate_soundcloud_transcoding_url(
                &reqwest::Url::parse("https://evil.test/media").unwrap()
            )
            .is_err()
        );
        assert!(
            validate_soundcloud_stream_url(
                &reqwest::Url::parse("https://cf-media.sndcdn.com/stream.mp3").unwrap()
            )
            .is_ok()
        );
        assert!(
            validate_soundcloud_stream_url(
                &reqwest::Url::parse("https://evil.sndcdn.com.evil.test/stream").unwrap()
            )
            .is_err()
        );
        assert!(
            validate_soundcloud_stream_url(
                &reqwest::Url::parse("http://cf-media.sndcdn.com/stream").unwrap()
            )
            .is_err()
        );
    }

    fn soundcloud_track_with_artists(
        credited_artist: &str,
        artists: Vec<crate::search::TrackArtistRef>,
    ) -> PlaybackTrack {
        PlaybackTrack {
            provider: PlaybackProvider::SoundCloud,
            id: "2348260049".into(),
            title: "Rumpta".into(),
            artist: credited_artist.into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists,
            artwork: String::new(),
            duration: Duration::from_millis(210_672),
            downloadable: false,
            progressive: false,
            explicit: false,
            service_url: String::new(),
        }
    }

    #[test]
    fn soundcloud_murglar_artist_names_prefer_uploader_refs_and_keep_credits() {
        let track = soundcloud_track_with_artists(
            "Solomun, Skrillex",
            vec![crate::search::TrackArtistRef {
                id: "315".into(),
                name: " Skrillex ".into(),
            }],
        );

        assert_eq!(track.artist, "Solomun, Skrillex");
        assert_eq!(
            StreamResolver::backend_artist_names_for_soundcloud(&track),
            vec!["Skrillex".to_owned()]
        );
    }

    #[test]
    fn soundcloud_murglar_artist_names_use_owner_slug_for_legacy_tracks() {
        let mut track = soundcloud_track_with_artists("Solomun, Skrillex", Vec::new());
        track.service_url = "https://soundcloud.com/skrillex/rumpta".into();

        assert_eq!(
            StreamResolver::backend_artist_names_for_soundcloud(&track),
            vec!["skrillex".to_owned()]
        );
        assert_eq!(track.artist, "Solomun, Skrillex");
    }

    #[test]
    fn authoritative_soundcloud_metadata_replaces_every_stale_murglar_match_field() {
        let mut track = soundcloud_track_with_artists(
            "Solomun, Skrillex",
            vec![crate::search::TrackArtistRef {
                id: "stale".into(),
                name: "Wrong uploader".into(),
            }],
        );
        track.id = "stale-id".into();
        track.title = "Stale title".into();
        track.duration = Duration::from_secs(1);
        let authoritative = json!({
            "id": 2_348_260_049_u64,
            "urn": "soundcloud:tracks:2348260049",
            "title": "Rumpta",
            "duration": 210_672,
            "full_duration": 210_626,
            "publisher_metadata": {"artist": "Solomun, Skrillex"},
            "user": {"id": 856_062, "username": "Skrillex"},
            "permalink_url": "https://soundcloud.com/skrillex/rumpta"
        });

        assert_eq!(
            StreamResolver::soundcloud_backend_fields(&track, Some(&authoritative)),
            (
                "2348260049".to_owned(),
                "Rumpta".to_owned(),
                vec!["Skrillex".to_owned()],
                210_672,
            )
        );
        assert_eq!(track.artist, "Solomun, Skrillex");
    }

    #[test]
    fn soundcloud_murglar_artist_names_append_owner_slug_to_incomplete_refs() {
        let mut track = soundcloud_track_with_artists(
            "Solomun, Skrillex",
            vec![crate::search::TrackArtistRef {
                id: "1".into(),
                name: "Solomun".into(),
            }],
        );
        track.service_url = "https://soundcloud.com/skrillex/rumpta".into();

        assert_eq!(
            StreamResolver::backend_artist_names_for_soundcloud(&track),
            vec!["Solomun".to_owned(), "skrillex".to_owned()]
        );
        assert_eq!(track.artist, "Solomun, Skrillex");
    }

    #[test]
    fn soundcloud_owner_slug_requires_a_valid_soundcloud_track_url() {
        for url in [
            "",
            "https://soundcloud.com",
            "https://soundcloud.com/",
            "https://soundcloud.com/sets/rumpta",
            "https://soundcloud.com/tracks/rumpta",
            "http://soundcloud.com/skrillex/rumpta",
            "https://soundcloud.com:444/skrillex/rumpta",
            "https://evil.test/skrillex/rumpta",
            "https://evil.soundcloud.com.evil.test/skrillex/rumpta",
            "https://user@soundcloud.com/skrillex/rumpta",
        ] {
            assert_eq!(
                StreamResolver::soundcloud_owner_slug(url),
                None,
                "unexpected owner slug for {url}"
            );
        }

        assert_eq!(
            StreamResolver::soundcloud_owner_slug(
                "https://soundcloud.com/skril%6Cex/rumpta?utm_source=test"
            ),
            Some("skrillex".to_owned())
        );
    }

    #[test]
    fn soundcloud_murglar_artist_names_deduplicate_owner_slug_case_insensitively() {
        let mut track = soundcloud_track_with_artists(
            "Credited Artist",
            vec![crate::search::TrackArtistRef {
                id: "1".into(),
                name: "SKRILLEX".into(),
            }],
        );
        track.service_url = "https://soundcloud.com/skrillex/rumpta".into();

        assert_eq!(
            StreamResolver::backend_artist_names_for_soundcloud(&track),
            vec!["SKRILLEX".to_owned()]
        );
    }

    #[test]
    fn soundcloud_murglar_artist_names_keep_owner_slug_within_bound() {
        let artists = (0..MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES - 1)
            .map(|index| crate::search::TrackArtistRef {
                id: index.to_string(),
                name: format!("Artist {index}"),
            })
            .collect();
        let mut track = soundcloud_track_with_artists("Credited Artist", artists);
        track.service_url = "https://soundcloud.com/skrillex/rumpta".into();

        let names = StreamResolver::backend_artist_names_for_soundcloud(&track);
        assert_eq!(names.len(), MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES);
        assert_eq!(names.last().map(String::as_str), Some("skrillex"));
    }

    #[test]
    fn soundcloud_murglar_artist_names_reserve_last_slot_for_owner_slug() {
        let artists = (0..MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES + 2)
            .map(|index| crate::search::TrackArtistRef {
                id: index.to_string(),
                name: format!("Artist {index}"),
            })
            .collect();
        let mut track = soundcloud_track_with_artists("Credited Artist", artists);
        track.service_url = "https://soundcloud.com/skrillex/rumpta".into();

        let names = StreamResolver::backend_artist_names_for_soundcloud(&track);
        assert_eq!(names.len(), MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES);
        assert_eq!(names.first().map(String::as_str), Some("Artist 0"));
        assert_eq!(
            names
                .get(MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES - 2)
                .map(String::as_str),
            Some("Artist 6")
        );
        assert_eq!(names.last().map(String::as_str), Some("skrillex"));
    }

    #[test]
    fn soundcloud_murglar_artist_names_trim_deduplicate_and_bound_with_fallback() {
        let mut artists = vec![
            crate::search::TrackArtistRef {
                id: "1".into(),
                name: " One ".into(),
            },
            crate::search::TrackArtistRef {
                id: "2".into(),
                name: "one".into(),
            },
            crate::search::TrackArtistRef {
                id: "3".into(),
                name: "   ".into(),
            },
        ];
        for index in 0..(MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES + 2) {
            artists.push(crate::search::TrackArtistRef {
                id: format!("extra-{index}"),
                name: format!("Artist {index}"),
            });
        }
        let track = soundcloud_track_with_artists("Credited Artist", artists);
        let names = StreamResolver::backend_artist_names_for_soundcloud(&track);

        assert_eq!(names.len(), MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES);
        assert_eq!(names[0], "One");
        assert_eq!(names[1], "Artist 0");
        assert_eq!(names.last().map(String::as_str), Some("Artist 6"));

        let fallback = soundcloud_track_with_artists(
            " Solomun, Skrillex ",
            vec![crate::search::TrackArtistRef {
                id: "empty".into(),
                name: " \t".into(),
            }],
        );
        assert_eq!(
            StreamResolver::backend_artist_names_for_soundcloud(&fallback),
            vec!["Solomun, Skrillex".to_owned()]
        );
    }

    #[test]
    fn soundcloud_track_authorization_accepts_only_bounded_ascii_values() {
        assert_eq!(
            soundcloud_track_authorization(&json!({
                "track_authorization": "  safe-token  "
            })),
            Some("safe-token")
        );
        let invalid_values = vec![
            String::new(),
            "   ".into(),
            "\tvalid-token".into(),
            "valid-token\n".into(),
            "contains\nnewline".into(),
            "contains\u{00e9}".into(),
            "x".repeat(MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH + 1),
        ];
        for value in invalid_values {
            assert!(
                soundcloud_track_authorization(&json!({
                    "track_authorization": value
                }))
                .is_none()
            );
        }
        assert!(
            soundcloud_track_authorization(&json!({
                "track_authorization": 42
            }))
            .is_none()
        );
    }

    #[test]
    fn soundcloud_transcoding_query_uses_shared_client_and_one_authorization_pair() {
        let mut url = reqwest::Url::parse(
            "https://api-v2.soundcloud.com/media/source?keep=value&client_id=stale&track_authorization=stale",
        )
        .unwrap();
        append_soundcloud_transcoding_query(&mut url, Some("safe-token"));
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert_eq!(
            pairs.iter().filter(|(key, _)| key == "client_id").count(),
            1
        );
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "track_authorization")
                .count(),
            1
        );
        assert!(pairs.contains(&("client_id".into(), SOUNDCLOUD_CLIENT_ID.into())));
        assert!(pairs.contains(&("track_authorization".into(), "safe-token".into())));
        assert!(pairs.contains(&("keep".into(), "value".into())));
    }

    #[test]
    fn invalid_soundcloud_authorization_is_omitted_without_leaking_it() {
        let secret = "invalid\nsecret";
        let mut url = reqwest::Url::parse("https://api-v2.soundcloud.com/media/source").unwrap();
        append_soundcloud_transcoding_query(&mut url, None);
        assert!(
            !url.query_pairs()
                .any(|(key, _)| key == "track_authorization")
        );
        assert!(!url.as_str().contains(secret));
        let error = provider_rejection("soundcloud.transcoding", StatusCode::NOT_FOUND);
        assert!(!error.contains(secret));
        assert!(!format!("{error:?}").contains(secret));
    }

    #[test]
    fn soundcloud_playback_prefers_highest_unencrypted_aac_hls_before_progressive() {
        let values = vec![
            json!({
                "url": "https://api-v2.soundcloud.com/media/progressive",
                "format": {"protocol": "progressive", "mime_type": "audio/mpeg"},
                "preset": "mp3_1_0"
            }),
            json!({
                "url": "https://api-v2.soundcloud.com/media/aac96",
                "format": {"protocol": "hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\""},
                "preset": "aac_96k"
            }),
            json!({
                "url": "https://api-v2.soundcloud.com/media/encrypted",
                "format": {"protocol": "hls-cbc", "mime_type": "audio/mp4"},
                "preset": "aac_160k"
            }),
            json!({
                "url": "https://api-v2.soundcloud.com/media/aac160",
                "format": {"protocol": "hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\""},
                "preset": "aac_160k"
            }),
        ];
        let selected = soundcloud_playback_transcodings(&values);
        assert_eq!(selected, vec![&values[3], &values[1], &values[0]]);
        assert!(is_soundcloud_hls_transcoding(&values[3]));
        assert!(!is_soundcloud_hls_transcoding(&values[2]));
        assert_eq!(soundcloud_transcoding_bitrate(&values[3]), Some(160));
        assert_eq!(transcoding_format(&values[3]), AudioFormat::M4a);
        assert_eq!(
            soundcloud_format_name(&values[3], AudioFormat::M4a),
            "M4A/MP4 AAC"
        );
    }

    #[test]
    fn media_response_validation_separates_logical_provider_from_transport() {
        let soundcloud = reqwest::Url::parse("https://cf-media.sndcdn.com/stream.mp3").unwrap();
        let dzcdn = reqwest::Url::parse("https://cdnt-stream.dzcdn.net/media/file.flac").unwrap();
        let unrelated = reqwest::Url::parse("https://media.example.test/audio").unwrap();
        assert!(validate_media_response_url(&soundcloud, true).is_ok());
        assert!(validate_media_response_url(&dzcdn, true).is_err());
        assert!(validate_media_response_url(&unrelated, false).is_ok());
    }

    #[test]
    fn provider_format_mapping_does_not_assume_flac() {
        assert_eq!(audio_format("FLAC"), Some(AudioFormat::Flac));
        assert_eq!(audio_format("MP3_320"), Some(AudioFormat::Mp3));
        assert_eq!(audio_format("audio/mp4"), Some(AudioFormat::M4a));
        assert_eq!(audio_format("unknown"), None);
    }

    #[test]
    fn every_resolved_format_has_a_decoder_hint() {
        let formats = [
            ("FLAC", AudioFormat::Flac, "flac", "audio/flac"),
            ("MP3", AudioFormat::Mp3, "mp3", "audio/mpeg"),
            ("WAV", AudioFormat::Wav, "wav", "audio/wav"),
            ("AIFF/AIFC", AudioFormat::Aiff, "aiff", "audio/aiff"),
            ("Ogg Vorbis", AudioFormat::OggVorbis, "ogg", "audio/ogg"),
            (
                "Ogg Opus",
                AudioFormat::OggOpus,
                "opus",
                "audio/ogg; codecs=opus",
            ),
            ("AAC/ADTS", AudioFormat::Aac, "aac", "audio/aac"),
            ("M4A/MP4 AAC", AudioFormat::M4a, "m4a", "audio/mp4"),
        ];
        for (label, format, extension, mime) in formats {
            assert_eq!(format.label(), label);
            assert_eq!(format.extension(), extension);
            assert_eq!(format.mime_type(), mime);
        }
    }

    #[test]
    fn soundcloud_transcoding_format_uses_provider_metadata() {
        assert_eq!(
            transcoding_format(&json!({"format":{"mime_type":"audio/aac"}})),
            AudioFormat::Aac
        );
        assert_eq!(
            transcoding_format(&json!({"preset":"opus_0_64"})),
            AudioFormat::OggOpus
        );
    }

    #[test]
    fn deezer_listen_request_matches_the_captured_gateway_contract() {
        let resolver = StreamResolver::new().unwrap();
        let payload = json!({
            "next_media": { "media": { "id": "42", "type": "song" } }
        });
        let request = resolver
            .deezer_listen_request("check-form", HeaderMap::new(), &payload)
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://www.deezer.com/ajax/gw-light.php?method=log.listen&input=3&api_version=1.0&api_token=check-form"
        );
        assert_eq!(
            request.body().and_then(|body| body.as_bytes()),
            Some(br#"{"next_media":{"media":{"id":"42","type":"song"}}}"#.as_slice())
        );
    }

    #[test]
    fn soundcloud_listen_request_matches_the_mobile_play_history_contract() {
        let resolver = StreamResolver::new().unwrap();
        let token = SoundCloudToken::from_saved("oauth-sentinel").unwrap();
        let report = SoundCloudListenReport::from_playback(
            &PlaybackContext::SoundCloudCollection {
                context_urn: "soundcloud:playlists:123".into(),
            },
            "456",
        )
        .unwrap();
        let request = resolver
            .soundcloud_listen_request(&token, &report)
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api-v2.soundcloud.com/me/play-history"
        );
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "OAuth oauth-sentinel"
        );
        assert_eq!(
            request.headers()[header::USER_AGENT],
            SOUNDCLOUD_MOBILE_USER_AGENT
        );
        assert_eq!(request.headers()[header::ACCEPT], "*/*");
        assert_eq!(
            request.headers()[header::ACCEPT_ENCODING],
            SOUNDCLOUD_MOBILE_ACCEPT_ENCODING
        );
        assert!(!request.headers().contains_key(header::COOKIE));
        assert_eq!(
            request.headers()[header::CONTENT_TYPE],
            "application/json; charset=UTF-8"
        );
        let payload: Value = serde_json::from_slice(
            request
                .body()
                .and_then(|body| body.as_bytes())
                .expect("SoundCloud listen payload"),
        )
        .unwrap();
        assert_eq!(payload, report.payload());
        assert_eq!(payload, json!({ "track_urn": "soundcloud:tracks:456" }));
    }

    #[test]
    fn soundcloud_listen_request_changes_with_the_saved_credential() {
        let resolver = StreamResolver::new().unwrap();
        let report = SoundCloudListenReport::from_playback(&PlaybackContext::None, "456").unwrap();
        let first = resolver
            .soundcloud_listen_request(
                &SoundCloudToken::from_saved("oauth-first").unwrap(),
                &report,
            )
            .unwrap()
            .build()
            .unwrap();
        let second = resolver
            .soundcloud_listen_request(
                &SoundCloudToken::from_saved("oauth-second").unwrap(),
                &report,
            )
            .unwrap()
            .build()
            .unwrap();

        assert_ne!(
            first.headers()[header::AUTHORIZATION],
            second.headers()[header::AUTHORIZATION]
        );
        assert!(first.headers()[header::AUTHORIZATION].is_sensitive());
        assert!(second.headers()[header::AUTHORIZATION].is_sensitive());
    }

    #[test]
    fn deezer_session_request_matches_empty_post_contract() {
        let arl = DeezerArl::from_saved("credential-sentinel").unwrap();
        let request = reqwest::Client::new()
            .post("https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=")
            .headers(deezer_headers(Some(&arl), None).unwrap())
            .header(header::CONTENT_LENGTH, "0")
            .body("")
            .build()
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token="
        );
        assert_eq!(request.headers()[header::COOKIE], "arl=credential-sentinel");
        assert!(request.headers()[header::COOKIE].is_sensitive());
        assert_eq!(request.headers()[header::CONTENT_LENGTH], "0");
        assert!(!request.headers().contains_key(header::CONTENT_TYPE));
        assert_eq!(
            request.body().and_then(|body| body.as_bytes()),
            Some(&b""[..])
        );
        assert!(!format!("{request:?}").contains("credential-sentinel"));
    }

    #[test]
    fn original_download_format_prefers_filename_metadata() {
        let value = json!({"filename": "recording.flac"});
        assert_eq!(
            infer_soundcloud_original_format(
                &value,
                &reqwest::Url::parse("https://cf-media.sndcdn.com/a.mp3").unwrap()
            ),
            Some(AudioFormat::Flac)
        );
    }

    #[test]
    fn original_download_format_uses_provider_fields_and_url_hints() {
        let url = reqwest::Url::parse("https://cf-media.sndcdn.com/recording.wav").unwrap();
        assert_eq!(
            infer_soundcloud_original_format(&json!({"original_format": "audio/x-wav"}), &url),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            infer_soundcloud_original_format(&json!({"format": {"mime_type": "audio/flac"}}), &url),
            Some(AudioFormat::Flac)
        );
        assert_eq!(
            infer_soundcloud_original_format(&json!({"mime_type": "audio/mpeg"}), &url),
            Some(AudioFormat::Mp3)
        );
        assert_eq!(
            infer_soundcloud_original_format(&json!({}), &url),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            infer_soundcloud_original_format(
                &json!({"filename": "recording.unknown", "format": "application/octet-stream"}),
                &reqwest::Url::parse("https://cf-media.sndcdn.com/stream").unwrap()
            ),
            None
        );
    }

    #[test]
    fn original_download_format_prefers_media_bytes_over_headers_and_provider_hints() {
        let mp3_url = reqwest::Url::parse("https://cf-media.sndcdn.com/track.mp3").unwrap();
        let provider_mp3 =
            infer_soundcloud_original_format(&json!({"filename": "track.mp3"}), &mp3_url);
        let header_mp3 = Some(AudioFormat::Mp3);
        assert_eq!(
            choose_soundcloud_original_format(
                sniff_soundcloud_original_format(b"RIFFxxxxWAVEfmt "),
                header_mp3,
                provider_mp3,
            ),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            choose_soundcloud_original_format(
                sniff_soundcloud_original_format(b"fLaC metadata"),
                header_mp3,
                provider_mp3,
            ),
            Some(AudioFormat::Flac)
        );
    }

    #[test]
    fn soundcloud_original_hints_only_accept_exact_tokens_and_safe_paths() {
        assert_eq!(
            soundcloud_format_from_hint("recording.wav"),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            soundcloud_format_from_hint("My recording.flac"),
            Some(AudioFormat::Flac)
        );
        assert_eq!(soundcloud_format_from_hint("this contains wav"), None);
        assert_eq!(soundcloud_format_from_hint("not-a-flac-format"), None);
        assert_eq!(soundcloud_format_from_hint("artist_mp3_mix"), None);
        assert_eq!(
            soundcloud_format_from_hint("audio/x-wav; charset=binary"),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            soundcloud_format_from_hint("audio/flac; name=mp3"),
            Some(AudioFormat::Flac)
        );

        let url = reqwest::Url::parse("https://cf-media.sndcdn.com/track.mp3?signature=flac#flac")
            .unwrap();
        assert_eq!(
            soundcloud_format_from_url_path(&url),
            Some(AudioFormat::Mp3)
        );
        let url =
            reqwest::Url::parse("https://cf-media.sndcdn.com/track?signature=flac#flac").unwrap();
        assert_eq!(soundcloud_format_from_url_path(&url), None);
    }

    #[test]
    fn soundcloud_original_media_headers_only_use_audio_type_and_filename() {
        let url = reqwest::Url::parse("https://cf-media.sndcdn.com/download").unwrap();
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/octet-stream; name=flac".parse().unwrap(),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            "attachment; name=flac; filename=recording.mp3"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            soundcloud_format_from_media_headers(&headers, &url),
            Some(AudioFormat::Mp3)
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            "attachment; name=wav; filename*=UTF-8''recording%2Eflac"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            soundcloud_format_from_media_headers(&headers, &url),
            Some(AudioFormat::Flac)
        );
        headers.remove(header::CONTENT_DISPOSITION);
        assert_eq!(soundcloud_format_from_media_headers(&headers, &url), None);
    }

    #[test]
    fn captured_soundcloud_original_download_is_recognized_as_wav() {
        let url = reqwest::Url::parse(
            "https://cf-media.sndcdn.com/xmNGc7QbfnCb?Policy=redacted&Signature=redacted&Key-Pair-Id=redacted",
        )
        .unwrap();
        assert!(validate_soundcloud_stream_url(&url).is_ok());

        let mut headers = header::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "audio/wav".parse().unwrap());
        headers.insert(
            header::CONTENT_DISPOSITION,
            "attachment;filename=\"SoundCloud%20Download\"; filename*=utf-8''zedd%20-%20spectrum%20%28jpky%20flip%29.wav"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            soundcloud_format_from_media_headers(&headers, &url),
            Some(AudioFormat::Wav)
        );
        assert_eq!(
            choose_soundcloud_original_format(
                sniff_soundcloud_original_format(b"RIFFxxxxWAVEfmt "),
                soundcloud_format_from_media_headers(&headers, &url),
                None,
            ),
            Some(AudioFormat::Wav)
        );
    }

    #[test]
    fn original_download_format_sniffs_supported_magic_signatures() {
        let mut ogg_opus = b"OggS".to_vec();
        ogg_opus.extend_from_slice(&[0; 32]);
        ogg_opus.extend_from_slice(b"OpusHead");
        let mut ogg_vorbis = b"OggS".to_vec();
        ogg_vorbis.extend_from_slice(&[0; 32]);
        ogg_vorbis.extend_from_slice(b"vorbis");
        let cases = [
            (&b"fLaC metadata"[..], AudioFormat::Flac),
            (&b"RIFFxxxxWAVEfmt "[..], AudioFormat::Wav),
            (&b"FORMxxxxAIFFCOMM"[..], AudioFormat::Aiff),
            (&b"ID3 metadata"[..], AudioFormat::Mp3),
            (&b"\xff\xfb\x90\x64 frame"[..], AudioFormat::Mp3),
            (&b"\xff\xf1\x50\x80 ADTS"[..], AudioFormat::Aac),
            (&b"\0\0\0\x18ftypisom"[..], AudioFormat::M4a),
        ];
        for (bytes, expected) in cases {
            assert_eq!(sniff_soundcloud_original_format(bytes), Some(expected));
        }
        assert_eq!(
            sniff_soundcloud_original_format(&ogg_opus),
            Some(AudioFormat::OggOpus)
        );
        assert_eq!(
            sniff_soundcloud_original_format(&ogg_vorbis),
            Some(AudioFormat::OggVorbis)
        );
        assert_eq!(sniff_soundcloud_original_format(b"not an audio file"), None);
    }

    #[test]
    fn original_download_bitrate_rejects_implausible_compressed_values() {
        assert_eq!(
            soundcloud_original_bitrate(&json!({"bitrate": 320}), AudioFormat::Mp3),
            Some(320)
        );
        assert_eq!(
            soundcloud_original_bitrate(&json!({"bitrate": 2315}), AudioFormat::Mp3),
            None
        );
        assert_eq!(
            soundcloud_original_bitrate(&json!({"bitrate": 2315}), AudioFormat::Wav),
            Some(2315)
        );
    }

    #[test]
    fn range_bodies_handle_partial_and_ignored_range_responses() {
        assert_eq!(
            ranged_body(StatusCode::PARTIAL_CONTENT, b"234", 2, 4).unwrap(),
            b"234"
        );
        assert_eq!(ranged_body(StatusCode::OK, b"01234", 2, 4).unwrap(), b"234");
        assert_eq!(
            ranged_body(StatusCode::OK, b"0123456789", 2, 4).unwrap(),
            b"234"
        );
        assert!(ranged_body(StatusCode::PARTIAL_CONTENT, b"2", 2, 4).is_err());
        assert!(ranged_body(StatusCode::OK, b"01", 2, 4).is_err());
    }

    #[test]
    fn speculative_prefetch_is_limited_to_the_first_cache_block() {
        assert_eq!(prefetch_range(0), None);
        assert_eq!(prefetch_range(1), Some((0, 0)));
        assert_eq!(
            prefetch_range(BLOCK_SIZE),
            Some((0, BLOCK_SIZE.saturating_sub(1)))
        );
        assert_eq!(
            prefetch_range(BLOCK_SIZE + 1),
            Some((0, BLOCK_SIZE.saturating_sub(1)))
        );
    }

    #[test]
    fn response_diagnostics_contain_status_and_length_only() {
        let message =
            response_diagnostics_message("murglar flac probe", StatusCode::OK, Some(2048));
        assert_eq!(
            message,
            "murglar flac probe status=200 OK content_length=Some(2048)"
        );
        assert!(!message.contains("first_bytes"));
        assert!(!message.contains("fLaC"));
    }

    #[test]
    fn invalid_murglar_output_can_fall_back_to_direct_deezer() {
        assert!(should_fallback_to_direct_deezer(
            true,
            "The resolved FLAC output did not start with fLaC"
        ));
        assert!(!should_fallback_to_direct_deezer(
            false,
            "The resolved FLAC output did not start with fLaC"
        ));
        assert!(!should_fallback_to_direct_deezer(
            true,
            "The download is too large"
        ));
    }

    #[test]
    fn eligible_timeline_startup_errors_reach_direct_recovery() {
        assert!(should_recover_timeline_startup_error(true, true, false));
        assert!(!should_recover_timeline_startup_error(false, true, false));
        assert!(!should_recover_timeline_startup_error(true, false, false));
        assert!(!should_recover_timeline_startup_error(true, true, true));
    }

    #[test]
    fn deezer_cache_identity_separates_resolved_quality_formats() {
        let flac = stable_cache_identity(
            PlaybackProvider::Deezer,
            "track-42",
            SourceVariant::Deezer,
            AudioFormat::Flac,
            "FLAC",
        );
        let mp3_320 = stable_cache_identity(
            PlaybackProvider::Deezer,
            "track-42",
            SourceVariant::Deezer,
            AudioFormat::Mp3,
            "MP3_320",
        );
        let mp3_128 = stable_cache_identity(
            PlaybackProvider::Deezer,
            "track-42",
            SourceVariant::Deezer,
            AudioFormat::Mp3,
            "MP3_128",
        );
        assert_ne!(flac, mp3_320);
        assert_ne!(mp3_320, mp3_128);
        assert_eq!(
            mp3_320,
            stable_cache_identity(
                PlaybackProvider::Deezer,
                "track-42",
                SourceVariant::Deezer,
                AudioFormat::Mp3,
                "MP3_320",
            )
        );
        let murglar_flac = stable_cache_identity(
            PlaybackProvider::Deezer,
            "track-42",
            SourceVariant::BackendDeezer,
            AudioFormat::Flac,
            "FLAC",
        );
        assert_ne!(murglar_flac, flac);
    }

    #[test]
    fn strict_deezer_recovery_requests_and_accepts_only_flac() {
        assert_eq!(deezer_format_preferences(true), &["FLAC"][..]);
        assert!(
            deezer_format_preferences(true)
                .iter()
                .all(|format| !format.starts_with("MP3"))
        );
        assert_eq!(
            enforce_deezer_format(AudioFormat::Flac, true),
            Ok(AudioFormat::Flac)
        );
        assert!(enforce_deezer_format(AudioFormat::Mp3, true).is_err());
        assert_eq!(
            enforce_deezer_format(AudioFormat::Mp3, false),
            Ok(AudioFormat::Mp3)
        );
    }

    #[test]
    fn playback_deezer_recovery_keeps_the_non_strict_preference_order() {
        assert_eq!(
            deezer_format_preferences(false),
            &["FLAC", "MP3_320", "MP3_128"][..]
        );
        assert_eq!(
            normalize_release_date("2022-08-05T00:00:00Z"),
            Some("2022-08-05".into())
        );
        assert_eq!(
            normalize_release_date("2022-08-05"),
            Some("2022-08-05".into())
        );
        assert_eq!(normalize_release_date("2022"), None);
    }

    #[test]
    fn deezer_ranges_align_to_stripes_and_trim_to_the_request() {
        assert_eq!(aligned_range(2047, 4096, 10_000), (0, 6143));
        assert_eq!(aligned_range(8193, 9999, 10_000), (8192, 9999));
        let aligned = (0_u8..12).collect::<Vec<_>>();
        assert_eq!(trim_range(aligned, 3, 7, 0).unwrap(), [3, 4, 5, 6, 7]);
    }

    #[tokio::test]
    async fn flac_output_requires_a_nonempty_flac_header() {
        let valid = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(valid.path(), b"fLaCmetadata")
            .await
            .unwrap();
        assert!(
            validate_audio_output(valid.path(), AudioFormat::Flac)
                .await
                .is_ok()
        );

        let invalid = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(invalid.path(), b"not flac").await.unwrap();
        assert!(
            validate_audio_output(invalid.path(), AudioFormat::Flac)
                .await
                .is_err()
        );

        let empty = tempfile::NamedTempFile::new().unwrap();
        assert!(
            validate_audio_output(empty.path(), AudioFormat::Mp3)
                .await
                .is_err()
        );
    }

    #[test]
    fn deezer_decryption_only_changes_every_third_complete_stripe() {
        let mut bytes = vec![7; STRIPE_SIZE * 4 + 10];
        let original = bytes.clone();
        decrypt_stripes(&mut bytes, "3135556", 0).unwrap();
        assert_ne!(&bytes[..STRIPE_SIZE], &original[..STRIPE_SIZE]);
        assert_eq!(
            &bytes[STRIPE_SIZE..STRIPE_SIZE * 3],
            &original[STRIPE_SIZE..STRIPE_SIZE * 3]
        );
        assert_ne!(
            &bytes[STRIPE_SIZE * 3..STRIPE_SIZE * 4],
            &original[STRIPE_SIZE * 3..STRIPE_SIZE * 4]
        );
        assert_eq!(&bytes[STRIPE_SIZE * 4..], &original[STRIPE_SIZE * 4..]);
    }

    #[tokio::test]
    async fn deezer_stream_decryption_matches_one_shot_across_irregular_chunks() {
        let track_id = "3135556";
        let mut encrypted = (0..(STRIPE_SIZE * 4 + 17))
            .map(|index| (index as u8).wrapping_mul(31).wrapping_add(7))
            .collect::<Vec<_>>();
        encrypted[..4].copy_from_slice(b"fLaC");
        let cipher = Blowfish::new_from_slice(&deezer_key(track_id)).unwrap();
        for stripe in [0, 3] {
            let start = stripe * STRIPE_SIZE;
            encrypt_cbc_for_test(&cipher, &mut encrypted[start..start + STRIPE_SIZE]);
        }

        let mut expected = encrypted.clone();
        decrypt_stripes(&mut expected, track_id, 0).unwrap();

        let file = tempfile::NamedTempFile::new().unwrap();
        let mut output = File::from_std(file.reopen().unwrap());
        let mut stream = DeezerStripeStream::new(track_id).unwrap();
        let mut offset = 0;
        for width in [1, 31, 2000, 17, 4095, 13, 1023, 7, 5000] {
            if offset == encrypted.len() {
                break;
            }
            let end = (offset + width).min(encrypted.len());
            stream
                .write_chunk(&encrypted[offset..end], &mut output)
                .await
                .unwrap();
            offset = end;
        }
        assert_eq!(offset, encrypted.len());
        stream.finish(&mut output).await.unwrap();
        output.flush().await.unwrap();
        drop(output);

        let actual = tokio::fs::read(file.path()).await.unwrap();
        assert_eq!(actual, expected);
        assert_eq!(
            &actual[STRIPE_SIZE * 4..],
            &encrypted[STRIPE_SIZE * 4..],
            "the final partial stripe must remain unchanged"
        );
    }

    #[tokio::test]
    async fn deezer_stream_decryption_writes_a_partial_stripe_on_finish() {
        let source = b"partial stripe tail";
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut output = File::from_std(file.reopen().unwrap());
        let mut stream = DeezerStripeStream::new("3135556").unwrap();
        for byte in source.chunks(3) {
            stream.write_chunk(byte, &mut output).await.unwrap();
        }
        stream.finish(&mut output).await.unwrap();
        output.flush().await.unwrap();
        drop(output);

        assert_eq!(tokio::fs::read(file.path()).await.unwrap(), source);
    }

    fn encrypt_cbc_for_test(cipher: &Blowfish, bytes: &mut [u8]) {
        let mut previous = [0, 1, 2, 3, 4, 5, 6, 7];
        for block in bytes.chunks_exact_mut(8) {
            let plaintext: [u8; 8] = block.try_into().expect("exact block");
            for index in 0..8 {
                block[index] = plaintext[index] ^ previous[index];
            }
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
            previous = block.try_into().expect("exact block");
        }
    }

    #[tokio::test]
    async fn cancellation_stops_before_provider_work() {
        let resolver = StreamResolver::new().unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let result = resolver
            .resolve(
                &PlaybackTrack {
                    downloadable: false,
                    progressive: false,
                    provider: PlaybackProvider::SoundCloud,
                    id: "1".into(),
                    title: String::new(),
                    artist: String::new(),
                    album: String::new(),
                    album_id: String::new(),
                    release_date: String::new(),
                    artists: Vec::new(),
                    artwork: String::new(),
                    duration: Duration::ZERO,
                    explicit: false,
                    service_url: String::new(),
                },
                None,
                None,
                None,
                token,
                None,
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn progressive_worker_cancellation_joins_the_download_task() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let buffer = ProgressiveFile::new(AudioFormat::Mp3, None).unwrap();
        let writer = buffer.writer().unwrap();
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let task = tokio::spawn(async move {
            worker_cancellation.cancelled().await;
            worker_stopped.store(true, Ordering::SeqCst);
            (writer, Err("Playback request cancelled".to_string()))
        });

        ProgressiveDownload {
            cancellation: Some(cancellation),
            task: Some(task),
        }
        .cancel_and_join()
        .await;

        assert!(stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cached_progressive_writes_report_small_ordered_chunks() {
        let resolver = StreamResolver::new().unwrap();
        let buffer = ProgressiveFile::new(
            AudioFormat::Mp3,
            Some((PROGRESSIVE_WRITE_CHUNK_SIZE * 2 + 1) as u64),
        )
        .unwrap();
        let mut writer = buffer.writer().unwrap();
        let updates = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = updates.clone();
        let progress: ProgressCallback = Arc::new(move |update| {
            captured.lock().unwrap().push(update.downloaded);
        });
        let mut downloaded = 0;
        let bytes = vec![0_u8; PROGRESSIVE_WRITE_CHUNK_SIZE * 2 + 1];
        resolver
            .write_progressive_chunks(
                &mut writer,
                &bytes,
                &mut downloaded,
                Some(bytes.len() as u64),
                &CancellationToken::new(),
                Some(&progress),
            )
            .await
            .unwrap();

        assert_eq!(downloaded, bytes.len() as u64);
        assert_eq!(
            updates.lock().unwrap().as_slice(),
            &[
                PROGRESSIVE_WRITE_CHUNK_SIZE as u64,
                (PROGRESSIVE_WRITE_CHUNK_SIZE * 2) as u64,
                (PROGRESSIVE_WRITE_CHUNK_SIZE * 2 + 1) as u64
            ]
        );
    }
}
