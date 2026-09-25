use std::{fmt, future::Future, pin::Pin, sync::Arc, time::Duration};

use reqwest::StatusCode;
use tokio::{fs::File, io::AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{
    progressive::{ProgressiveWriter, TimelineSeekSession, startup_bytes},
    retry::{RequestClass, RetryClass, classify_status},
};

pub(crate) type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AudioFormat {
    Flac,
    Mp3,
    Wav,
    Aiff,
    OggVorbis,
    OggOpus,
    Aac,
    M4a,
}

impl AudioFormat {
    /// Map a file extension back to the audio format it stores.
    pub(crate) fn from_extension(extension: &str) -> Option<Self> {
        Some(match extension.trim().to_ascii_lowercase().as_str() {
            "flac" => Self::Flac,
            "mp3" => Self::Mp3,
            "wav" => Self::Wav,
            "aiff" | "aif" | "aifc" => Self::Aiff,
            "ogg" => Self::OggVorbis,
            "opus" => Self::OggOpus,
            "aac" => Self::Aac,
            "m4a" | "mp4" => Self::M4a,
            _ => return None,
        })
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Flac => "FLAC",
            Self::Mp3 => "MP3",
            Self::Wav => "WAV",
            Self::Aiff => "AIFF/AIFC",
            Self::OggVorbis => "Ogg Vorbis",
            Self::OggOpus => "Ogg Opus",
            Self::Aac => "AAC/ADTS",
            Self::M4a => "M4A/MP4 AAC",
        }
    }

    pub(crate) const fn extension(self) -> &'static str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::OggVorbis => "ogg",
            Self::OggOpus => "opus",
            Self::Aac => "aac",
            Self::M4a => "m4a",
        }
    }

    pub(crate) const fn mime_type(self) -> &'static str {
        match self {
            Self::Flac => "audio/flac",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Aiff => "audio/aiff",
            Self::OggVorbis => "audio/ogg",
            Self::OggOpus => "audio/ogg; codecs=opus",
            Self::Aac => "audio/aac",
            Self::M4a => "audio/mp4",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ProgressUpdate {
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    pub(crate) buffered_fraction: Option<f32>,
    pub(crate) completed: bool,
}

impl ProgressUpdate {
    pub(crate) const fn bytes(downloaded: u64, total: Option<u64>) -> Self {
        Self {
            downloaded,
            total,
            buffered_fraction: None,
            completed: false,
        }
    }

    pub(crate) fn buffered(downloaded: u64, fraction: f32) -> Self {
        Self {
            downloaded,
            total: None,
            buffered_fraction: fraction.is_finite().then_some(fraction.clamp(0.0, 1.0)),
            completed: false,
        }
    }

    pub(crate) const fn complete(total: u64) -> Self {
        Self {
            downloaded: total,
            total: Some(total),
            buffered_fraction: Some(1.0),
            completed: true,
        }
    }
}

pub(crate) type ProgressCallback = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

pub(crate) trait DownloadOutput: AsyncWrite + Unpin + Send {
    fn set_total_hint(&mut self, total: Option<u64>);

    fn mark_progressive_startup_ready(&mut self) {}

    fn progressive_startup_bytes(&self, _format: AudioFormat, _total: Option<u64>) -> Option<u64> {
        None
    }
}

impl DownloadOutput for File {
    fn set_total_hint(&mut self, _total: Option<u64>) {}
}

impl DownloadOutput for ProgressiveWriter {
    fn set_total_hint(&mut self, total: Option<u64>) {
        self.set_total(total);
    }

    fn mark_progressive_startup_ready(&mut self) {
        self.mark_startup_ready();
    }

    fn progressive_startup_bytes(&self, format: AudioFormat, total: Option<u64>) -> Option<u64> {
        Some(startup_bytes(format, total))
    }
}

#[derive(Debug)]
pub(crate) struct PlaybackDownloadError {
    pub(crate) message: String,
    pub(crate) refresh_source: bool,
}

impl PlaybackDownloadError {
    pub(crate) fn media_status(stage: &'static str, status: StatusCode) -> Self {
        let reason = status.canonical_reason().unwrap_or("unknown status");
        Self {
            message: format!(
                "The {stage} media URL was rejected (HTTP {} {reason})",
                status.as_u16()
            ),
            refresh_source: matches!(
                classify_status(status, RequestClass::Media),
                RetryClass::ExpiredMedia
            ),
        }
    }

    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            refresh_source: false,
        }
    }
}

impl From<String> for PlaybackDownloadError {
    fn from(message: String) -> Self {
        Self::message(message)
    }
}

impl From<&str> for PlaybackDownloadError {
    fn from(message: &str) -> Self {
        Self::message(message)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackendProvider {
    Deezer,
    SoundCloud,
}

#[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
#[derive(Clone, Debug)]
pub(crate) struct MediaRequest {
    pub(crate) provider: BackendProvider,
    pub(crate) track_id: String,
    pub(crate) title: String,
    pub(crate) artist_names: Vec<String>,
    pub(crate) album_name: Option<String>,
    pub(crate) release_date: Option<String>,
    pub(crate) duration_ms: u64,
    pub(crate) allow_high_quality: bool,
    pub(crate) exact_quality: Option<String>,
    pub(crate) include_remote_size: bool,
}

impl MediaRequest {
    pub(crate) fn new(
        provider: BackendProvider,
        track_id: impl Into<String>,
        title: impl Into<String>,
        artist_names: Vec<String>,
        album_name: Option<String>,
        release_date: Option<String>,
        duration_ms: u64,
        allow_high_quality: bool,
        exact_quality: Option<String>,
        include_remote_size: bool,
    ) -> Self {
        Self {
            provider,
            track_id: track_id.into(),
            title: title.into(),
            artist_names,
            album_name,
            release_date,
            duration_ms,
            allow_high_quality,
            exact_quality,
            include_remote_size,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackendFormat {
    pub(crate) format: AudioFormat,
    pub(crate) format_name: String,
    pub(crate) declared_bitrate: Option<u32>,
}

#[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackendProvenance {
    Deezer,
    SoundCloud,
}

/// Distinguishes range fetches for the track the user is listening to from
/// background work like next-track prefetch and library downloads. Playback
/// fetches run unthrottled once the source is resolved; background fetches
/// keep sharing the process-wide request budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaFetchScope {
    Playback,
    Background,
}

#[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BackendCacheIdentity(String);

impl BackendCacheIdentity {
    #[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackendSourceMetadata {
    pub(crate) format: AudioFormat,
    pub(crate) format_name: String,
    pub(crate) size: u64,
    pub(crate) declared_bitrate: Option<u32>,
    pub(crate) duration: Option<Duration>,
    pub(crate) timeline: bool,
    pub(crate) cacheable: bool,
    pub(crate) initial_buffered_fraction: Option<f32>,
    pub(crate) deezer_track_id: Option<String>,
    pub(crate) provenance: BackendProvenance,
    pub(crate) cache_identity: BackendCacheIdentity,
}

pub(crate) trait BackendSourceOps: Send + Sync {
    fn metadata(&self) -> BackendSourceMetadata;

    fn timeline_seek_session(
        &self,
        cancellation: CancellationToken,
    ) -> Option<Arc<dyn TimelineSeekSession>>;

    fn read_range<'a>(
        &'a self,
        start: u64,
        end: u64,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>>;

    fn probe_size<'a>(
        &'a self,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Option<u64>>;

    fn download<'a>(
        &'a self,
        output: &'a mut dyn DownloadOutput,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
        progress: Option<&'a ProgressCallback>,
    ) -> BackendFuture<'a, Result<(), PlaybackDownloadError>>;
}

#[derive(Clone)]
pub(crate) struct BackendSource(Arc<dyn BackendSourceOps>);

impl fmt::Debug for BackendSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BackendSource")
            .field("metadata", &self.metadata())
            .finish()
    }
}

impl BackendSource {
    #[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
    pub(crate) fn from_ops(ops: Arc<dyn BackendSourceOps>) -> Self {
        Self(ops)
    }

    pub(crate) fn metadata(&self) -> BackendSourceMetadata {
        self.0.metadata()
    }

    pub(crate) fn is_cacheable(&self) -> bool {
        self.metadata().cacheable
    }

    pub(crate) fn timeline_seek_session(
        &self,
        cancellation: CancellationToken,
    ) -> Option<Arc<dyn TimelineSeekSession>> {
        self.0.timeline_seek_session(cancellation)
    }

    pub(crate) fn read_range<'a>(
        &'a self,
        start: u64,
        end: u64,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>> {
        self.0.read_range(start, end, scope, cancellation)
    }

    pub(crate) fn probe_size<'a>(
        &'a self,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Option<u64>> {
        self.0.probe_size(scope, cancellation)
    }

    pub(crate) fn download<'a>(
        &'a self,
        output: &'a mut dyn DownloadOutput,
        scope: MediaFetchScope,
        cancellation: &'a CancellationToken,
        progress: Option<&'a ProgressCallback>,
    ) -> BackendFuture<'a, Result<(), PlaybackDownloadError>> {
        self.0.download(output, scope, cancellation, progress)
    }
}

#[cfg_attr(not(ralgrum_private_backend), allow(dead_code))]
#[derive(Debug)]
pub(crate) enum MediaResolveOutcome<T> {
    Source(T),
    Unavailable,
    FallbackToDirect(String),
    RefreshSource(String),
    Cancelled,
    Fatal(String),
}
