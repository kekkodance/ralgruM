use futures::future::BoxFuture;
use std::{
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    sync::mpsc,
    time::Duration,
};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use super::super::progressive::{
    DownloadPauseGate, DownloadPauseGuard, LandedSuffixSource, ProgressiveCompletion,
    ProgressiveFile, ProgressiveReader, ProgressiveWriter, TimelineSeekRequest,
    TimelineSeekSession, TimelineSeekStartup, TimelineSuffixState,
};
use super::AudioFormat;
use super::flac_coverage::{FlacCoverage, FlacScanner};

/// Enough suffix bytes for the decoder probe and a moment of playback.
const SUFFIX_STARTUP_BYTES: u64 = 64 * 1024;
/// The first range stays small so playback resumes quickly.
const SUFFIX_STARTUP_CHUNK: u64 = 256 * 1024;
/// Later ranges use the backend's normal transfer size to avoid throttling
/// the remaining buffer with many sequential requests.
const SUFFIX_STREAM_CHUNK: u64 = 1024 * 1024;
/// Keep an idle suffix near its decoder's startup region. Once that reader
/// starts consuming, let the active suffix finish without throttling playback.
const SUFFIX_PREFETCH_BYTES: u64 = 1024 * 1024;
/// First probe size for the FLAC file header (fLaC marker plus metadata).
const FLAC_HEADER_PROBE_BYTES: u64 = 64 * 1024;
/// Upper bound for the FLAC header probe before the seek gives up.
const FLAC_HEADER_PROBE_LIMIT: u64 = 4 * 1024 * 1024;
/// FLAC probe attempts that home in on a frame at or before the target.
/// Variable bitrate streams bracket the target first and then interpolate
/// inside the bracket, so the budget only runs low on extreme profiles.
const FLAC_SEEK_PROBES: usize = 10;
/// Distance the probes leave between their aim point and the target, from
/// either direction, so the frame they find lands just short of it.
const FLAC_AIM_AHEAD: Duration = Duration::from_secs(2);
/// Largest FLAC gap bridged by discarding instead of another probe.
const FLAC_FORWARD_LIMIT: Duration = Duration::from_secs(4);

/// Serves one byte range of the track, from `[start, end]` inclusive.
pub(super) type RangeFetch = Arc<
    dyn Fn(u64, u64, CancellationToken) -> BoxFuture<'static, Result<Vec<u8>, String>>
        + Send
        + Sync,
>;

/// The progressive formats a range session can land mid-download.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RangeSeekFormat {
    /// Constant bitrate MP3 maps a seek target straight to a byte offset.
    Mp3,
    /// FLAC is variable bitrate, so the suffix position is corrected by
    /// parsing frame headers and discarding up to the exact target.
    Flac,
}

impl RangeSeekFormat {
    pub(super) fn from_audio(format: AudioFormat) -> Option<Self> {
        match format {
            AudioFormat::Mp3 => Some(Self::Mp3),
            AudioFormat::Flac => Some(Self::Flac),
            _ => None,
        }
    }

    fn audio(self) -> AudioFormat {
        match self {
            Self::Mp3 => AudioFormat::Mp3,
            Self::Flac => AudioFormat::Flac,
        }
    }
}

/// A timeline seek session for remote progressive sources.
///
/// SoundCloud and Deezer transcodes are constant bitrate MP3, so a seek
/// target maps to a byte offset and the CDN can serve the rest of the file
/// starting there. Deezer FLAC is variable bitrate: the session fetches the
/// file header, parses frame headers around the estimated offset to learn
/// the exact timeline position of the suffix, and reports the remaining
/// discard so the engine lands on the target. Targets already covered by
/// the active progressive prefix stay on that growing file. Other targets
/// fetch into a fresh progressive buffer so they can land before the front
/// download completes.
pub(crate) struct RangeTimelineSession {
    format: RangeSeekFormat,
    fetch: RangeFetch,
    total: u64,
    duration: Duration,
    runtime: Handle,
    track_cancellation: CancellationToken,
    pause: DownloadPauseGate,
    suffix: Arc<std::sync::Mutex<Option<SuffixTracker>>>,
    front_coverage: Arc<std::sync::Mutex<Option<FrontBufferCoverage>>>,
    landed: Arc<std::sync::Mutex<Option<LandedSuffix>>>,
}

enum FrontBufferMap {
    Mp3,
    Flac {
        audio_start: u64,
        stream_info: FlacStreamInfo,
        coverage: FlacCoverage,
    },
}

struct FrontBufferCoverage {
    completion: ProgressiveCompletion,
    total: u64,
    duration: Duration,
    map: FrontBufferMap,
}

/// A suffix this session fetched far enough to serve local seeks. Its
/// buffer file remains a local seek source for any position it spans, so
/// later seeks inside it neither fetch again nor park the front download.
/// The record is published as soon as the startup handoff happens, then
/// refreshed as the fetch keeps landing more bytes.
struct LandedSuffix {
    base: Duration,
    path: PathBuf,
    completion: ProgressiveCompletion,
    /// Byte offset on the source file where this suffix's frames start.
    base_bytes: u64,
    /// Bytes of staged header written before the frames.
    header_len: u64,
    flac_info: Option<FlacStreamInfo>,
    flac_coverage: Option<FlacCoverage>,
    cancellation: CancellationToken,
}

impl FrontBufferCoverage {
    fn from_file(
        format: RangeSeekFormat,
        path: &std::path::Path,
        completion: ProgressiveCompletion,
        total: u64,
        duration: Duration,
    ) -> Option<Self> {
        let map = match format {
            RangeSeekFormat::Mp3 => FrontBufferMap::Mp3,
            RangeSeekFormat::Flac => {
                use std::io::Read as _;

                let available = completion.written().min(FLAC_HEADER_PROBE_LIMIT);
                if available == 0 {
                    return None;
                }
                let mut bytes = Vec::with_capacity(usize::try_from(available).ok()?);
                std::fs::File::open(path)
                    .ok()?
                    .take(available)
                    .read_to_end(&mut bytes)
                    .ok()?;
                let (header, stream_info) = parse_flac_header(&bytes).ok()?;
                let audio_start = header.len() as u64;
                let coverage = FlacCoverage::new();
                let mut scanner =
                    FlacScanner::new(stream_info.clone(), audio_start, coverage.clone());
                scanner.ingest(audio_start, &bytes[header.len()..]);
                FrontBufferMap::Flac {
                    audio_start,
                    stream_info,
                    coverage,
                }
            }
        };
        Some(Self {
            completion,
            total,
            duration,
            map,
        })
    }

    fn contains(&self, position: Duration) -> bool {
        if self.completion.is_complete() {
            return true;
        }
        let start = match &self.map {
            FrontBufferMap::Mp3 => {
                let seconds = self.duration.as_secs_f64();
                if !seconds.is_finite() || seconds <= 0.0 {
                    return false;
                }
                let fraction = (position.as_secs_f64() / seconds).clamp(0.0, 0.99);
                (fraction * self.total as f64) as u64
            }
            FrontBufferMap::Flac {
                audio_start,
                stream_info,
                coverage,
            } => {
                if coverage.contains(position, self.completion.written()) {
                    return true;
                }
                let Some(target_offset) =
                    stream_info.safe_target_end(position, *audio_start, self.total)
                else {
                    return false;
                };
                // A FLAC seek-point bracket does not bound byte rate within
                // the bracket. Require the next point to be flushed before
                // claiming an earlier target can be reached locally.
                let ready_through = target_offset
                    .saturating_add(stream_info.max_frame_bytes())
                    .min(self.total);
                return self.completion.written() >= ready_through;
            }
        };
        let ready_through = start.saturating_add(SUFFIX_STARTUP_BYTES).min(self.total);
        self.completion.written() >= ready_through
    }
}

/// Byte progress of the suffix a session is fetching. The seek worker
/// refines the counters as the fetch advances, so the buffering indicator
/// can track the buffer that actually gates playback.
struct SuffixTracker {
    base: Duration,
    progress: Arc<SuffixProgress>,
}

struct SuffixProgress {
    written: AtomicU64,
    total: AtomicU64,
}

impl RangeTimelineSession {
    pub(super) fn new(
        format: RangeSeekFormat,
        fetch: RangeFetch,
        total: u64,
        duration: Duration,
        runtime: Handle,
        track_cancellation: CancellationToken,
        pause: DownloadPauseGate,
    ) -> Self {
        Self {
            format,
            fetch,
            total,
            duration,
            runtime,
            track_cancellation,
            pause,
            suffix: Arc::new(std::sync::Mutex::new(None)),
            front_coverage: Arc::new(std::sync::Mutex::new(None)),
            landed: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Reuses the flushed prefix of the active progressive download before
    /// asking the provider for a range. This keeps seeks inside the current
    /// buffer local and avoids downloading the same bytes twice.
    pub(super) fn with_front_buffer(
        mut self,
        path: PathBuf,
        completion: ProgressiveCompletion,
    ) -> Self {
        let initial = FrontBufferCoverage::from_file(
            self.format,
            &path,
            completion.clone(),
            self.total,
            self.duration,
        );
        if let Ok(mut coverage) = self.front_coverage.lock() {
            *coverage = initial;
        }
        if self.format == RangeSeekFormat::Flac {
            let coverage = self.front_coverage.clone();
            let probe_path = path.clone();
            let probe_completion = completion.clone();
            let track_cancellation = self.track_cancellation.clone();
            let total = self.total;
            let duration = self.duration;
            self.runtime.spawn(async move {
                let mut last_probe = 0;
                let mut scanner: Option<FlacScanner> = None;
                loop {
                    if track_cancellation.is_cancelled() || !probe_completion.is_usable() {
                        break;
                    }
                    let written = probe_completion.written();
                    if scanner.is_none() {
                        let parsed = coverage.lock().ok().and_then(|value| {
                            let front = value.as_ref()?;
                            let FrontBufferMap::Flac {
                                audio_start,
                                stream_info,
                                coverage,
                            } = &front.map
                            else {
                                return None;
                            };
                            Some((*audio_start, stream_info.clone(), coverage.clone()))
                        });
                        if let Some((audio_start, info, verified)) = parsed {
                            scanner = Some(FlacScanner::new(info, audio_start, verified));
                        } else if written != last_probe {
                            last_probe = written;
                            let path = probe_path.clone();
                            let completion = probe_completion.clone();
                            let parsed = tokio::task::spawn_blocking(move || {
                                FrontBufferCoverage::from_file(
                                    RangeSeekFormat::Flac,
                                    &path,
                                    completion,
                                    total,
                                    duration,
                                )
                            })
                            .await
                            .ok()
                            .flatten();
                            if let Some(parsed) = parsed
                                && let Ok(mut value) = coverage.lock()
                            {
                                *value = Some(parsed);
                            }
                        }
                    }
                    if let Some(mut current) = scanner.take() {
                        if written > current.next_offset() {
                            let path = probe_path.clone();
                            let scanned = tokio::task::spawn_blocking(move || {
                                let _ = current.scan_file_to(&path, written);
                                current
                            })
                            .await;
                            scanner = scanned.ok();
                        } else {
                            scanner = Some(current);
                        }
                    }
                    if probe_completion.is_complete() {
                        break;
                    }
                    tokio::select! {
                        () = track_cancellation.cancelled() => break,
                        () = tokio::time::sleep(Duration::from_millis(25)) => {}
                    }
                }
            });
        }
        let remote = self.fetch;
        self.fetch = Arc::new(move |start, end, cancellation| {
            let remote = remote.clone();
            let path = path.clone();
            let completion = completion.clone();
            Box::pin(async move {
                fetch_reusing_front_buffer(remote, path, completion, start, end, cancellation).await
            })
        });
        self
    }
}

async fn fetch_reusing_front_buffer(
    remote: RangeFetch,
    path: PathBuf,
    completion: ProgressiveCompletion,
    start: u64,
    end: u64,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, String> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

    if cancellation.is_cancelled() {
        return Err("Playback request cancelled".into());
    }
    let written = completion.written();
    if start >= written {
        return remote(start, end, cancellation).await;
    }
    let local_end = end.min(written.saturating_sub(1));
    let local_len = local_end
        .checked_sub(start)
        .and_then(|length| length.checked_add(1))
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(|| "The buffered seek range was invalid".to_string())?;
    let local = async {
        let mut file = tokio::fs::File::open(path).await?;
        file.seek(std::io::SeekFrom::Start(start)).await?;
        let mut bytes = vec![0; local_len];
        file.read_exact(&mut bytes).await?;
        Ok::<_, std::io::Error>(bytes)
    }
    .await;
    let Ok(mut bytes) = local else {
        // The front buffer is only an optimization. If it disappears during
        // teardown, the provider range remains a valid seek source.
        return remote(start, end, cancellation).await;
    };
    if local_end < end {
        let mut tail = remote(local_end + 1, end, cancellation).await?;
        bytes.append(&mut tail);
    }
    Ok(bytes)
}

impl RangeTimelineSession {
    /// A landed suffix serves a local seek when the position sits inside its
    /// span and its flushed frontier already covers the target's bytes. The
    /// fetch keeps writing after the record is published, so a growing
    /// suffix serves anything up to what has actually landed on disk.
    fn landed_suffix_source(&self, position: Duration) -> Option<LandedSuffixSource> {
        let landed = self.landed.lock().ok()?;
        let landed = landed.as_ref()?;
        if landed.cancellation.is_cancelled() && !landed.completion.is_complete() {
            return None;
        }
        if position < landed.base || position >= self.duration {
            return None;
        }
        let seconds = self.duration.as_secs_f64();
        if !seconds.is_finite() || seconds <= 0.0 {
            return None;
        }
        // Linear estimate over the suffix's remaining span: the same class
        // of map the MP3 front coverage uses. The startup margin absorbs
        // variable-bitrate drift and guarantees the decoder's frame probe
        // and refine reads stay inside flushed bytes.
        let remaining_bytes = self.total.saturating_sub(landed.base_bytes).max(1);
        let remaining_time = self.duration.saturating_sub(landed.base).as_secs_f64();
        let into = position
            .saturating_sub(landed.base)
            .as_secs_f64()
            .min(remaining_time);
        if landed
            .flac_coverage
            .as_ref()
            .is_some_and(|coverage| coverage.contains(position, landed.completion.written()))
            && landed.completion.is_usable()
        {
            return Some(LandedSuffixSource {
                path: landed.path.clone(),
                completion: landed.completion.clone(),
                base: landed.base,
            });
        }
        let needed_source = if position == landed.base {
            landed.base_bytes
        } else if let Some(info) = landed.flac_info.as_ref() {
            info.safe_target_end(position, landed.header_len, self.total)?
        } else {
            landed.base_bytes + (into / remaining_time * remaining_bytes as f64) as u64
        };
        let needed_in_suffix = landed
            .header_len
            .saturating_add(needed_source.saturating_sub(landed.base_bytes));
        if !landed.completion.is_usable() {
            return None;
        }
        let covered = landed.completion.written();
        let margin = landed
            .flac_info
            .as_ref()
            .map_or(SUFFIX_STARTUP_BYTES, FlacStreamInfo::max_frame_bytes);
        let required = needed_in_suffix
            .saturating_add(margin)
            .min(landed.header_len + remaining_bytes);
        if required > covered || needed_source > self.total {
            return None;
        }
        Some(LandedSuffixSource {
            path: landed.path.clone(),
            completion: landed.completion.clone(),
            base: landed.base,
        })
    }
}

impl TimelineSeekSession for RangeTimelineSession {
    fn can_seek_from_front(&self, position: Duration) -> bool {
        self.front_coverage
            .lock()
            .ok()
            .is_some_and(|front| front.as_ref().is_some_and(|front| front.contains(position)))
    }

    fn landed_suffix(&self, position: Duration) -> Option<LandedSuffixSource> {
        self.landed_suffix_source(position)
    }

    fn request(&self, position: Duration) -> Result<TimelineSeekRequest, String> {
        let seconds = self.duration.as_secs_f64();
        if !seconds.is_finite() || seconds <= 0.0 {
            return Err("The track duration is unknown".into());
        }
        if self.total == 0 {
            return Err("The track size is unknown".to_string());
        }
        let format = self.format;
        let fraction = (position.as_secs_f64() / seconds).clamp(0.0, 0.99);
        let start = (fraction * self.total as f64) as u64;
        let progress = Arc::new(SuffixProgress {
            written: AtomicU64::new(0),
            total: AtomicU64::new(self.total.saturating_sub(start).max(1)),
        });
        // The newest request owns the reported suffix state, so a
        // superseded fetch stops feeding the indicator immediately.
        if let Ok(mut suffix) = self.suffix.lock() {
            *suffix = Some(SuffixTracker {
                base: position,
                progress: progress.clone(),
            });
        }
        if let Ok(mut landed) = self.landed.lock() {
            *landed = None;
        }
        let buffer = ProgressiveFile::new(format.audio(), None)
            .map_err(|_| "A temporary seek buffer could not be created".to_string())?;
        let reader = buffer
            .reader()
            .map_err(|_| "The seek buffer could not be opened".to_string())?;
        let writer = buffer
            .writer()
            .map_err(|_| "The seek buffer could not be opened".to_string())?;
        let path = buffer.path().to_path_buf();
        let completion = reader.completion();
        let file = buffer.into_file();
        let cancellation = self.track_cancellation.child_token();
        let worker_cancellation = cancellation.clone();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let fetch = self.fetch.clone();
        let total = self.total;
        let duration = self.duration;
        let landed = self.landed.clone();
        // The front download's pause gate is shared with the writer through
        // the SuffixWriter's startup handoff: the gate protects the small
        // startup range requests from competing with the front stream, and
        // the release happens as soon as the handoff lands the decoder. The
        // remaining tail keeps fetching without parking the front download.
        let pause_guard = self.pause.hold();
        let runtime = self.runtime.clone();
        runtime.spawn(async move {
            let error_sender = startup_sender.clone();
            let result = {
                let suffix = SuffixWriter {
                    writer,
                    fetch,
                    total,
                    startup: Some((reader, file)),
                    startup_sender,
                    cancellation: &worker_cancellation,
                    intra_segment_offset: None,
                    written: 0,
                    progress: progress.clone(),
                    landed: Some(landed),
                    base: position,
                    completion,
                    path,
                    frame_base: start,
                    header_len: 0,
                    flac_info: None,
                    flac_coverage: None,
                    flac_scanner: None,
                    published: false,
                    pause_guard: Some(pause_guard),
                    startup_read_frontier: 0,
                };
                match format {
                    RangeSeekFormat::Mp3 => suffix.write_mp3(start).await,
                    RangeSeekFormat::Flac => suffix.write_flac(start, duration, position).await,
                }
            };
            if let Err(error) = result {
                let _ = error_sender.send(Err(error));
            }
        });
        Ok(TimelineSeekRequest {
            format: format.audio(),
            intra_segment_offset: Duration::ZERO,
            cancellation,
            startup: startup_receiver,
        })
    }

    fn suffix_state(&self) -> Option<TimelineSuffixState> {
        let suffix = self.suffix.lock().ok()?;
        let tracker = suffix.as_ref()?;
        Some(TimelineSuffixState {
            pending: false,
            base: tracker.base,
            written: tracker.progress.written.load(Ordering::Relaxed),
            total: tracker.progress.total.load(Ordering::Relaxed),
        })
    }
}

/// Streams a seek suffix into a fresh progressive buffer and hands the reader
/// to the engine once enough bytes are in place to build a decoder.
struct SuffixWriter<'a> {
    writer: ProgressiveWriter,
    fetch: RangeFetch,
    total: u64,
    startup: Option<(ProgressiveReader, tempfile::NamedTempFile)>,
    startup_sender: mpsc::SyncSender<Result<TimelineSeekStartup, String>>,
    cancellation: &'a CancellationToken,
    intra_segment_offset: Option<Duration>,
    written: u64,
    progress: Arc<SuffixProgress>,
    landed: Option<Arc<std::sync::Mutex<Option<LandedSuffix>>>>,
    base: Duration,
    completion: ProgressiveCompletion,
    path: PathBuf,
    /// Source byte offset where this suffix's fetched frames start. The
    /// suffix file is header bytes then this stream, so positions inside the
    /// suffix map to (offset minus base) plus the header length.
    frame_base: u64,
    /// Staged header bytes written before the frame stream.
    header_len: u64,
    flac_info: Option<FlacStreamInfo>,
    flac_coverage: Option<FlacCoverage>,
    flac_scanner: Option<FlacScanner>,
    /// Whether the startup handoff published the landed record already.
    published: bool,
    /// Gate holding the front download back. Dropped at the startup
    /// handoff so the front download resumes while the tail streams.
    pause_guard: Option<DownloadPauseGuard>,
    /// Reader position when the decoder became available. Movement beyond
    /// this point means the seek decoder is consuming the suffix.
    startup_read_frontier: u64,
}

impl SuffixWriter<'_> {
    /// Streams the suffix for a constant bitrate MP3 source.
    async fn write_mp3(self, start: u64) -> Result<(), String> {
        self.write_from(start).await
    }

    /// Streams a FLAC suffix: fetches the file header, homes in on a frame at
    /// or before the target, then streams frames from there while the engine
    /// discards the remainder up to the exact target.
    async fn write_flac(
        mut self,
        start: u64,
        duration: Duration,
        target: Duration,
    ) -> Result<(), String> {
        let staged = flac_suffix_start(
            self.fetch.clone(),
            self.total,
            duration,
            start,
            target,
            self.cancellation,
            &mut self.writer,
        )
        .await;
        let (header_len, frame, info) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                self.writer.fail(error.clone());
                return Err(error);
            }
        };
        self.written = header_len;
        self.header_len = header_len;
        let coverage = FlacCoverage::new();
        self.flac_scanner = Some(FlacScanner::new(info.clone(), header_len, coverage.clone()));
        self.flac_coverage = Some(coverage);
        self.flac_info = Some(info);
        self.frame_base = frame.offset;
        self.intra_segment_offset = Some(target.saturating_sub(frame.position));
        // The playable suffix is the frame range, not the staged header, so
        // the indicator's denominator switches to it once it is known.
        self.progress.total.store(
            self.total.saturating_sub(frame.offset).max(1),
            Ordering::Relaxed,
        );
        self.progress.written.store(0, Ordering::Relaxed);
        self.write_from(frame.offset).await
    }

    async fn write_from(mut self, start: u64) -> Result<(), String> {
        let result = self.write_ranges(start).await;
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                // Unblock any decoder already reading this buffer so a
                // failed fetch surfaces instead of stalling playback.
                self.writer.fail(error.clone());
                Err(error)
            }
        }
    }

    async fn write_ranges(&mut self, mut start: u64) -> Result<(), String> {
        use tokio::io::AsyncWriteExt as _;

        let mut fetched = 0_u64;
        while start < self.total {
            if self.cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            if self.startup.is_none()
                && self.completion.read_frontier() <= self.startup_read_frontier
            {
                while self.written
                    > self
                        .completion
                        .read_frontier()
                        .saturating_add(SUFFIX_PREFETCH_BYTES)
                {
                    if self.completion.read_frontier() > self.startup_read_frontier {
                        break;
                    }
                    tokio::select! {
                        () = self.cancellation.cancelled() => return Err("Playback request cancelled".into()),
                        () = tokio::time::sleep(Duration::from_millis(50)) => {}
                    }
                }
            }
            let chunk = if fetched == 0 {
                SUFFIX_STARTUP_CHUNK
            } else {
                SUFFIX_STREAM_CHUNK
            };
            let end = (start + chunk - 1).min(self.total - 1);
            let bytes = (self.fetch)(start, end, self.cancellation.clone())
                .await
                .map_err(|error| format!("The seek suffix could not be downloaded: {error}"))?;
            let file_offset = self.written;
            self.writer
                .write_all(&bytes)
                .await
                .map_err(|_| "The seek buffer could not be written".to_string())?;
            self.writer
                .flush()
                .await
                .map_err(|_| "The seek buffer could not be finalized".to_string())?;
            self.written = self.written.saturating_add(bytes.len() as u64);
            if let Some(scanner) = self.flac_scanner.as_mut() {
                scanner.ingest(file_offset, &bytes);
            }
            fetched = fetched.saturating_add(bytes.len() as u64);
            self.progress.written.store(fetched, Ordering::Relaxed);
            start = end.saturating_add(1);
            if self.written >= SUFFIX_STARTUP_BYTES || start >= self.total {
                self.release_startup()?;
            }
        }
        // The suffix was empty or too small for the startup threshold.
        self.release_startup()?;
        self.writer
            .finish()
            .await
            .map_err(|_| "The seek buffer could not be finalized".to_string())?;
        // The suffix is complete: its buffer is a local seek source for
        // every position it spans.
        self.publish_landed();
        Ok(())
    }

    /// Publishes or refreshes the session's landed record. The buffer
    /// serves local seeks from the startup handoff onward, growing as the
    /// fetch keeps landing bytes.
    fn publish_landed(&mut self) {
        let Some(landed) = self.landed.clone() else {
            return;
        };
        let mut landed = landed.lock().unwrap_or_else(|error| error.into_inner());
        *landed = Some(LandedSuffix {
            base: self.base,
            path: self.path.clone(),
            completion: self.completion.clone(),
            base_bytes: self.frame_base,
            header_len: self.header_len,
            flac_info: self.flac_info.clone(),
            flac_coverage: self.flac_coverage.clone(),
            cancellation: self.cancellation.clone(),
        });
    }

    fn release_startup(&mut self) -> Result<(), String> {
        let Some((reader, file)) = self.startup.take() else {
            return Ok(());
        };
        self.writer.mark_startup_ready();
        // The engine can rebuild a decoder from this buffer as soon as the
        // handoff happens, so the landed record must exist before it.
        if !self.published {
            self.publish_landed();
            self.published = true;
        }
        self.startup_read_frontier = self.completion.read_frontier();
        // The startup range requests are done and the decoder can land,
        // so the front download may stream again while the tail continues.
        self.pause_guard = None;
        if self
            .startup_sender
            .send(Ok(TimelineSeekStartup {
                reader,
                file,
                intra_segment_offset: self.intra_segment_offset,
            }))
            .is_err()
        {
            return Err("The seek result was cancelled".into());
        }
        Ok(())
    }
}
/// Fetches the FLAC file header and locates the frame the suffix starts at.
async fn flac_suffix_start(
    fetch: RangeFetch,
    total: u64,
    duration: Duration,
    start: u64,
    target: Duration,
    cancellation: &CancellationToken,
    writer: &mut ProgressiveWriter,
) -> Result<(u64, FlacFrame, FlacStreamInfo), String> {
    use tokio::io::AsyncWriteExt as _;

    let (header, stream_info) = flac_file_header(fetch.clone(), total, cancellation).await?;
    let frame = match stream_info.seek_frame_at_or_before(target, header.len() as u64, total) {
        Some(frame) => frame,
        None => {
            flac_frame_near(
                fetch,
                total,
                duration,
                &stream_info,
                target,
                start,
                cancellation,
            )
            .await?
        }
    };
    let mut header = header;
    // The suffix file re-bases its frame bytes at the frame the seek
    // picked, so the seek table must follow: its offsets are shifted by
    // the same distance and points that fall before the frame become
    // placeholders. A later local seek on the suffix buffer then lands
    // through the table exactly like it would on the original file.
    let skip = frame.offset.saturating_sub(header.len() as u64);
    rebase_flac_seek_table(&mut header, skip);
    writer
        .write_all(&header)
        .await
        .map_err(|_| "The seek buffer could not be written".to_string())?;
    writer
        .flush()
        .await
        .map_err(|_| "The seek buffer could not be finalized".to_string())?;
    Ok((header.len() as u64, frame, stream_info))
}

/// Shifts every SEEKTABLE offset in a FLAC header by `skip` bytes, marking
/// the points that land before the suffix start as placeholders.
fn rebase_flac_seek_table(header: &mut [u8], skip: u64) {
    let mut cursor = 4;
    while let Some(prefix) = header.get(cursor..cursor + 4) {
        let last = prefix[0] & 0x80 != 0;
        let block_type = prefix[0] & 0x7f;
        let length = (u32::from(prefix[1]) << 16 | u32::from(prefix[2]) << 8 | u32::from(prefix[3]))
            as usize;
        let payload_start = cursor + 4;
        let Some(payload_end) = payload_start.checked_add(length) else {
            return;
        };
        if header.get(..payload_end).is_none() {
            return;
        }
        if block_type == 3 {
            for point in header[payload_start..payload_end].chunks_exact_mut(18) {
                let offset = u64::from_be_bytes(point[8..16].try_into().unwrap());
                if offset >= skip {
                    point[8..16].copy_from_slice(&(offset - skip).to_be_bytes());
                } else {
                    point[..8].fill(0xff);
                }
            }
            return;
        }
        cursor = payload_end;
        if last {
            return;
        }
    }
}

/// Fetches the `fLaC` marker and metadata blocks from the start of the file.
async fn flac_file_header(
    fetch: RangeFetch,
    total: u64,
    cancellation: &CancellationToken,
) -> Result<(Vec<u8>, FlacStreamInfo), String> {
    let mut probe_end = FLAC_HEADER_PROBE_BYTES.min(total);
    let mut bytes = fetch(0, probe_end - 1, cancellation.clone())
        .await
        .map_err(|error| format!("The FLAC seek header could not be downloaded: {error}"))?;
    loop {
        match parse_flac_header(&bytes) {
            Ok(header) => return Ok(header),
            Err(FlacHeaderError::Incomplete) => {
                if probe_end >= FLAC_HEADER_PROBE_LIMIT {
                    return Err("The FLAC seek header was oversized".into());
                }
                // Grow the probe by fetching only the new tail, so a large
                // cover art block costs one transfer of its own bytes
                // instead of re-reading the whole prefix at every step.
                let next = (probe_end * 4).min(total).min(FLAC_HEADER_PROBE_LIMIT);
                if next <= probe_end {
                    return Err("The FLAC seek header was oversized".into());
                }
                let tail = fetch(probe_end, next - 1, cancellation.clone())
                    .await
                    .map_err(|error| {
                        format!("The FLAC seek header could not be downloaded: {error}")
                    })?;
                bytes.extend_from_slice(&tail);
                probe_end = next;
            }
            Err(FlacHeaderError::Invalid) => {
                return Err("The FLAC seek header was invalid".into());
            }
        }
    }
}
/// Locates a FLAC frame at or before the seek target.
///
/// The byte ratio estimate only approximates a variable bitrate stream, so
/// each probe parses the first frame header it finds to learn the exact
/// timeline position. The probes keep a bracket around the target: the
/// closest frame at or before it and the closest frame past it. Once both
/// bounds exist the next probe interpolates the target's byte offset inside
/// the bracket, which converges for any bitrate profile; a lone bound is
/// re-aimed with the bitrate measured between the last two probes. The
/// returned frame is always at or before the target: a frame past it would
/// zero the discard and resume playback past the requested position.
async fn flac_frame_near(
    fetch: RangeFetch,
    total: u64,
    duration: Duration,
    stream_info: &FlacStreamInfo,
    target: Duration,
    estimate: u64,
    cancellation: &CancellationToken,
) -> Result<FlacFrame, String> {
    let seconds = duration.as_secs_f64();
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("The track duration is unknown".into());
    }
    let average_bytes_per_second = (total as f64 / seconds).max(1.0);
    let probe_len = stream_info
        .max_frame_bytes()
        .clamp(64 * 1024, 4 * 1024 * 1024);
    let mut probe_start = estimate.min(total - 1);
    let mut lower: Option<FlacFrame> = None;
    let mut upper: Option<FlacFrame> = None;
    let mut previous: Option<FlacFrame> = None;
    for _ in 0..FLAC_SEEK_PROBES {
        // A window reaching past the upper bound would re-find the frame
        // that set it, so the probe stays inside the bracket.
        let bracket_end = upper.map_or(total - 1, |frame| frame.offset.saturating_sub(1));
        let end = (probe_start + probe_len - 1)
            .min(total - 1)
            .min(bracket_end);
        if probe_start > end {
            break;
        }
        let bytes = fetch(probe_start, end, cancellation.clone())
            .await
            .map_err(|error| format!("The FLAC seek probe could not be downloaded: {error}"))?;
        let Some(parsed) = parse_first_flac_frame(&bytes, stream_info, probe_start) else {
            break;
        };
        // Consecutive probes measure the local bitrate, which a variable
        // bitrate stream makes far more accurate than the file average.
        let rate =
            local_bytes_per_second(&parsed, previous.as_ref()).unwrap_or(average_bytes_per_second);
        previous = Some(parsed);
        if parsed.position > target {
            if upper
                .as_ref()
                .is_some_and(|frame| parsed.position >= frame.position)
            {
                break;
            }
            upper = Some(parsed);
            probe_start = match lower.as_ref() {
                Some(lower) => interpolated_offset(lower, &parsed, target),
                None => {
                    let back =
                        bytes_for(parsed.position - target + FLAC_AIM_AHEAD, rate, 1.1).max(1);
                    let next = parsed.offset.saturating_sub(back).min(total - 1);
                    if next >= probe_start {
                        // The backup cannot move closer: fall back to the
                        // file start, whose first frame always sits at or
                        // before the target.
                        0
                    } else {
                        next
                    }
                }
            };
        } else {
            if lower
                .as_ref()
                .is_some_and(|frame| parsed.position <= frame.position)
            {
                break;
            }
            lower = Some(parsed);
            if target - parsed.position <= FLAC_FORWARD_LIMIT {
                break;
            }
            probe_start = match upper.as_ref() {
                Some(upper) => interpolated_offset(&parsed, upper, target),
                None => parsed
                    .offset
                    .saturating_add(bytes_for(
                        target - parsed.position - FLAC_AIM_AHEAD,
                        rate,
                        1.0,
                    ))
                    .min(total - 1),
            };
        }
    }
    lower.ok_or_else(|| "The FLAC seek frame could not be located".to_string())
}

/// Byte rate between two probes, which tracks the local bitrate of a
/// variable bitrate stream far better than the file average.
fn local_bytes_per_second(current: &FlacFrame, previous: Option<&FlacFrame>) -> Option<f64> {
    let previous = previous?;
    let span = current.position.max(previous.position) - current.position.min(previous.position);
    let seconds = span.as_secs_f64();
    let bytes = current.offset.abs_diff(previous.offset);
    (seconds > 0.0).then(|| (bytes as f64 / seconds).max(1.0))
}

/// Interpolates the byte offset of the seek target between two bracketing
/// frames, aiming a couple of seconds short of it so the frame the probe
/// finds lands at or before the target.
fn interpolated_offset(lower: &FlacFrame, upper: &FlacFrame, target: Duration) -> u64 {
    let span = upper.position.saturating_sub(lower.position).as_secs_f64();
    if !span.is_finite() || span <= 0.0 {
        return lower.offset;
    }
    let aim = target.saturating_sub(FLAC_AIM_AHEAD);
    let into_span = aim.saturating_sub(lower.position).as_secs_f64().min(span);
    let fraction = (into_span / span).clamp(0.0, 1.0);
    let span_bytes = upper.offset.saturating_sub(lower.offset) as f64;
    lower.offset.saturating_add((fraction * span_bytes) as u64)
}

/// Stream parameters read from the FLAC STREAMINFO metadata block.
#[derive(Clone)]
pub(super) struct FlacStreamInfo {
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u32,
    block_len_min: u64,
    block_len_max: u64,
    seek_points: Vec<FlacSeekPoint>,
}

impl FlacStreamInfo {
    /// Upper bound on one frame's byte size, used to size probe requests.
    fn max_frame_bytes(&self) -> u64 {
        self.block_len_max
            .saturating_mul(u64::from(self.channels))
            .saturating_mul(u64::from(self.bits_per_sample.div_ceil(8)))
            .saturating_add(64)
    }

    fn seek_frame_at_or_before(
        &self,
        target: Duration,
        audio_start: u64,
        total: u64,
    ) -> Option<FlacFrame> {
        let target_sample = target
            .as_nanos()
            .saturating_mul(u128::from(self.sample_rate))
            / 1_000_000_000;
        let point = self
            .seek_points
            .iter()
            .filter(|point| u128::from(point.sample) <= target_sample)
            .max_by_key(|point| point.sample)?;
        let offset = audio_start.checked_add(point.offset)?;
        if offset >= total || self.sample_rate == 0 {
            return None;
        }
        let nanos =
            u128::from(point.sample).saturating_mul(1_000_000_000) / u128::from(self.sample_rate);
        Some(FlacFrame {
            offset,
            position: Duration::from_nanos(u64::try_from(nanos).ok()?),
        })
    }

    /// Conservative end of the seek-point bracket containing the target.
    /// Byte interpolation inside a variable-bitrate FLAC bracket is not a
    /// proof that a target frame is already in the progressive prefix.
    fn safe_target_end(&self, target: Duration, audio_start: u64, total: u64) -> Option<u64> {
        if self.sample_rate == 0 {
            return None;
        }
        if target.is_zero() {
            return Some(audio_start.min(total));
        }
        let target_sample = target
            .as_nanos()
            .saturating_mul(u128::from(self.sample_rate))
            / 1_000_000_000;
        if let Some(exact) = self
            .seek_points
            .iter()
            .find(|point| u128::from(point.sample) == target_sample)
        {
            return Some(audio_start.saturating_add(exact.offset).min(total));
        }
        let upper = self
            .seek_points
            .iter()
            .filter(|point| u128::from(point.sample) > target_sample)
            .min_by_key(|point| point.sample);
        // No upper point means the complete tail is needed. At an exact
        // point, its own frame and a small decoder probe are enough.
        let offset = upper.map_or(total.saturating_sub(audio_start), |point| point.offset);
        Some(audio_start.saturating_add(offset).min(total))
    }
}

#[derive(Clone, Copy)]
struct FlacSeekPoint {
    sample: u64,
    offset: u64,
}

#[derive(Clone, Copy)]
pub(super) struct FlacFrame {
    pub(super) offset: u64,
    pub(super) position: Duration,
}

#[derive(Debug, Eq, PartialEq)]
enum FlacHeaderError {
    Invalid,
    Incomplete,
}

/// Splits the FLAC file header (marker plus metadata blocks) off the front of
/// a prefix of the file, reporting the stream parameters on the way.
fn parse_flac_header(bytes: &[u8]) -> Result<(Vec<u8>, FlacStreamInfo), FlacHeaderError> {
    if !bytes.starts_with(b"fLaC") {
        return Err(FlacHeaderError::Invalid);
    }
    let mut cursor = 4;
    let mut info = None;
    let mut seek_points = Vec::new();
    loop {
        let Some(prefix) = bytes.get(cursor..cursor + 4) else {
            return Err(FlacHeaderError::Incomplete);
        };
        let last = prefix[0] & 0x80 != 0;
        let block_type = prefix[0] & 0x7f;
        if block_type == 0x7f {
            return Err(FlacHeaderError::Invalid);
        }
        let length = (u32::from(prefix[1]) << 16 | u32::from(prefix[2]) << 8 | u32::from(prefix[3]))
            as usize;
        let payload_start = cursor + 4;
        let Some(payload_end) = payload_start.checked_add(length) else {
            return Err(FlacHeaderError::Invalid);
        };
        let Some(payload) = bytes.get(payload_start..payload_end) else {
            return Err(FlacHeaderError::Incomplete);
        };
        if info.is_none() {
            if block_type != 0 || payload.len() != 34 {
                return Err(FlacHeaderError::Invalid);
            }
            info = Some(parse_flac_stream_info(payload));
        } else if block_type == 3 {
            if payload.len() % 18 != 0 {
                return Err(FlacHeaderError::Invalid);
            }
            for point in payload.chunks_exact(18) {
                let sample = u64::from_be_bytes(point[0..8].try_into().unwrap());
                let offset = u64::from_be_bytes(point[8..16].try_into().unwrap());
                if sample != u64::MAX {
                    seek_points.push(FlacSeekPoint { sample, offset });
                }
            }
        }
        cursor = payload_end;
        if last {
            let mut info = info.expect("STREAMINFO is the first metadata block");
            info.seek_points = seek_points;
            return Ok((bytes[..cursor].to_vec(), info));
        }
    }
}

fn parse_flac_stream_info(payload: &[u8]) -> FlacStreamInfo {
    let block_len_min = u64::from(u16::from_be_bytes([payload[0], payload[1]]));
    let block_len_max = u64::from(u16::from_be_bytes([payload[2], payload[3]]));
    let sample_rate = (u32::from(payload[10]) << 12)
        | (u32::from(payload[11]) << 4)
        | (u32::from(payload[12]) >> 4);
    let channels = u32::from((payload[12] >> 1) & 0x07) + 1;
    let bits_per_sample = u32::from(((payload[12] & 0x01) << 4) | (payload[13] >> 4)) + 1;
    FlacStreamInfo {
        sample_rate,
        channels,
        bits_per_sample,
        block_len_min,
        block_len_max,
        seek_points: Vec::new(),
    }
}

/// Finds the first frame header in a probe buffer and reports where the frame
/// starts and when it plays. False sync matches inside frame data are weeded
/// out with the same checks symphonia applies: reserved fields, the header
/// CRC-8, and consistency with the stream parameters.
pub(super) fn parse_first_flac_frame(
    bytes: &[u8],
    info: &FlacStreamInfo,
    base: u64,
) -> Option<FlacFrame> {
    let mut index = 0;
    while index + 2 <= bytes.len() {
        if bytes[index] == 0xff
            && (bytes[index + 1] & 0xfe) == 0xf8
            && let Some(frame) = parse_flac_frame_header(bytes, index, info, base)
        {
            return Some(frame);
        }
        index += 1;
    }
    None
}

fn parse_flac_frame_header(
    bytes: &[u8],
    index: usize,
    info: &FlacStreamInfo,
    base: u64,
) -> Option<FlacFrame> {
    let header = bytes.get(index..index.checked_add(4)?)?;
    let sync = u16::from_be_bytes([header[0], header[1]]);
    let variable_blocks = sync & 0x01 != 0;
    let block_size_code = u32::from(header[2] >> 4);
    let sample_rate_code = u32::from(header[2] & 0x0f);
    let channel_code = u32::from(header[3] >> 4);
    let sample_size_code = u32::from((header[3] >> 1) & 0x07);
    if header[3] & 0x01 != 0
        || block_size_code == 0
        || sample_rate_code == 0x0f
        || channel_code >= 0x0b
        || matches!(sample_size_code, 0x03 | 0x07)
    {
        return None;
    }
    let mut cursor = index + 4;
    let number = decode_utf8_number(bytes, &mut cursor)?;
    let block_samples = match block_size_code {
        1 => 192,
        2..=5 => 576u64 << (block_size_code - 2),
        6 => {
            let extra = u64::from(*bytes.get(cursor)?);
            cursor += 1;
            extra + 1
        }
        7 => {
            let extra = bytes.get(cursor..cursor + 2)?;
            cursor += 2;
            let value = u16::from_be_bytes([extra[0], extra[1]]);
            if value == 0xffff {
                return None;
            }
            u64::from(value) + 1
        }
        8..=15 => 256u64 << (block_size_code - 8),
        _ => return None,
    };
    let sample_rate = match sample_rate_code {
        1 => 88_200,
        2 => 176_400,
        3 => 192_000,
        4 => 8_000,
        5 => 16_000,
        6 => 22_050,
        7 => 24_000,
        8 => 32_000,
        9 => 44_100,
        10 => 48_000,
        11 => 96_000,
        12 => {
            let extra = u32::from(*bytes.get(cursor)?);
            cursor += 1;
            extra * 1_000
        }
        13 => {
            let extra = bytes.get(cursor..cursor + 2)?;
            cursor += 2;
            u32::from(u16::from_be_bytes([extra[0], extra[1]]))
        }
        14 => {
            let extra = bytes.get(cursor..cursor + 2)?;
            cursor += 2;
            u32::from(u16::from_be_bytes([extra[0], extra[1]])) * 10
        }
        _ => info.sample_rate,
    };
    if sample_rate != info.sample_rate || block_samples > info.block_len_max {
        return None;
    }
    let expected_crc = *bytes.get(cursor)?;
    if crc8(&bytes[index..cursor]) != expected_crc {
        return None;
    }
    let sample_position = if variable_blocks {
        number
    } else if info.block_len_min == info.block_len_max {
        number.checked_mul(info.block_len_min)?
    } else {
        number.checked_mul(block_samples)?
    };
    Some(FlacFrame {
        offset: base + index as u64,
        position: samples_to_duration(sample_position, info.sample_rate),
    })
}

/// Decodes the UTF-8-like coded number that opens every FLAC frame header,
/// mirroring the reader symphonia uses for the same field.
fn decode_utf8_number(bytes: &[u8], cursor: &mut usize) -> Option<u64> {
    let first = *bytes.get(*cursor)?;
    *cursor += 1;
    let mask = match first {
        0x00..=0x7f => return Some(u64::from(first)),
        0xc0..=0xdf => 0x1f,
        0xe0..=0xef => 0x0f,
        0xf0..=0xf7 => 0x07,
        0xf8..=0xfb => 0x03,
        0xfc..=0xfd => 0x01,
        0xfe => 0x00,
        _ => return None,
    };
    let mut value = u64::from(first & mask);
    for _ in 2..mask.leading_zeros() {
        let byte = *bytes.get(*cursor)?;
        *cursor += 1;
        value = (value << 6) | u64::from(byte & 0x3f);
    }
    Some(value)
}

/// CRC-8 with the CCITT polynomial 0x07, as used by FLAC frame headers.
fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0_u8;
    for byte in bytes {
        crc ^= *byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn samples_to_duration(samples: u64, sample_rate: u32) -> Duration {
    if sample_rate == 0 {
        return Duration::ZERO;
    }
    let nanos = (u128::from(samples) * 1_000_000_000) / u128::from(sample_rate);
    Duration::from_nanos(nanos.min(u64::MAX as u128) as u64)
}

fn bytes_for(duration: Duration, bytes_per_second: f64, scale: f64) -> u64 {
    (duration.as_secs_f64() * bytes_per_second * scale) as u64
}

#[cfg(test)]
mod tests {
    use std::{process::Command, sync::Mutex, time::Duration};

    use rodio::Source as _;
    use tokio_util::sync::CancellationToken;

    use super::super::super::progressive::TimelineSeekSession;
    use super::*;

    #[test]
    fn truncated_flac_sync_at_probe_end_is_ignored() {
        let info = FlacStreamInfo {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            block_len_min: 256,
            block_len_max: 4_096,
            seek_points: Vec::new(),
        };

        for bytes in [
            &[0xff, 0xf8][..],
            &[0x00, 0xff, 0xf9][..],
            &[0xff, 0xf8, 0x80][..],
        ] {
            assert!(parse_first_flac_frame(bytes, &info, 0).is_none());
        }
    }

    #[test]
    fn flac_seek_table_selects_the_closest_point_before_the_target() {
        let info = FlacStreamInfo {
            sample_rate: 44_100,
            channels: 2,
            bits_per_sample: 16,
            block_len_min: 256,
            block_len_max: 4_096,
            seek_points: vec![
                FlacSeekPoint {
                    sample: 0,
                    offset: 0,
                },
                FlacSeekPoint {
                    sample: 441_000,
                    offset: 1_000,
                },
                FlacSeekPoint {
                    sample: 882_000,
                    offset: 2_000,
                },
            ],
        };
        let frame = info
            .seek_frame_at_or_before(Duration::from_secs(11), 8_736, 20_000)
            .unwrap();
        assert_eq!(frame.offset, 9_736);
        assert_eq!(frame.position, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn flac_front_coverage_requires_the_target_region_to_be_flushed() {
        use tokio::io::AsyncWriteExt as _;

        let buffer = ProgressiveFile::new(AudioFormat::Flac, Some(20_000)).unwrap();
        let reader = buffer.reader().unwrap();
        let completion = reader.completion();
        let mut writer = buffer.writer().unwrap();
        let coverage = FrontBufferCoverage {
            completion,
            total: 20_000,
            duration: Duration::from_secs(30),
            map: FrontBufferMap::Flac {
                audio_start: 100,
                coverage: FlacCoverage::new(),
                stream_info: FlacStreamInfo {
                    sample_rate: 100,
                    channels: 1,
                    bits_per_sample: 8,
                    block_len_min: 1,
                    block_len_max: 1,
                    seek_points: vec![
                        FlacSeekPoint {
                            sample: 0,
                            offset: 0,
                        },
                        FlacSeekPoint {
                            sample: 1_000,
                            offset: 1_000,
                        },
                        FlacSeekPoint {
                            sample: 2_000,
                            offset: 2_000,
                        },
                    ],
                },
            },
        };

        // A variable bitrate bracket cannot prove the target's byte
        // position, so require the next seek point and decoder margin.
        writer.write_all(&vec![0; 1_264]).await.unwrap();
        writer.flush().await.unwrap();
        assert!(
            !coverage.contains(Duration::from_secs(11)),
            "the upper seek point still sits beyond the flushed prefix"
        );
        writer.write_all(&vec![0; 901]).await.unwrap();
        writer.flush().await.unwrap();
        assert!(coverage.contains(Duration::from_secs(11)));
        // The earlier target's upper bracket is also covered now.
        assert!(coverage.contains(Duration::from_secs(5)));
        assert!(coverage.contains(Duration::ZERO));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn front_flac_header_is_retried_when_the_initial_prefix_is_short() {
        use tokio::io::AsyncWriteExt as _;

        let mut info = vec![0; 34];
        info[..2].copy_from_slice(&256_u16.to_be_bytes());
        info[2..4].copy_from_slice(&256_u16.to_be_bytes());
        let packed = (1_000_u64 << 44) | (15_u64 << 36) | 3_000;
        info[10..18].copy_from_slice(&packed.to_be_bytes());
        let mut bytes = b"fLaC".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 34]);
        bytes.extend_from_slice(&info);
        bytes.extend_from_slice(&[0x83, 0, 0, 36]);
        for (sample, offset) in [(0_u64, 0_u64), (1_000, 1_000)] {
            bytes.extend_from_slice(&sample.to_be_bytes());
            bytes.extend_from_slice(&offset.to_be_bytes());
            bytes.extend_from_slice(&100_u16.to_be_bytes());
        }
        bytes.resize(3_000, 0);
        let buffer = ProgressiveFile::new(AudioFormat::Flac, Some(bytes.len() as u64)).unwrap();
        let completion = buffer.reader().unwrap().completion();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..8]).await.unwrap();
        writer.flush().await.unwrap();
        let (fetch, _) = memory_fetch(Arc::new(bytes.clone()));
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            bytes.len() as u64,
            Duration::from_secs(3),
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        )
        .with_front_buffer(buffer.path().to_path_buf(), completion);
        assert!(!session.can_seek_from_front(Duration::ZERO));
        writer.write_all(&bytes[8..]).await.unwrap();
        writer.flush().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !session.can_seek_from_front(Duration::ZERO) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("front coverage must discover the completed FLAC header");
    }

    fn make_mp3_tone_fixture() -> Option<tempfile::NamedTempFile> {
        let file = tempfile::Builder::new().suffix(".mp3").tempfile().ok()?;
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=if(lt(t\\,3)\\,0\\,0.7*sin(2*PI*880*t)):s=44100:d=4",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                "-f",
                "mp3",
                "-y",
            ])
            .arg(file.path())
            .status()
            .ok()?;
        status.success().then_some(file)
    }

    fn make_flac_tone_fixture() -> Option<tempfile::NamedTempFile> {
        let file = tempfile::Builder::new().suffix(".flac").tempfile().ok()?;
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc=if(lt(t\\,2)\\,0\\,0.7*sin(2*PI*880*t)):s=44100:d=4",
                "-c:a",
                "flac",
                "-f",
                "flac",
                "-y",
            ])
            .arg(file.path())
            .status()
            .ok()?;
        status.success().then_some(file)
    }

    fn sample_rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    type RecordedRanges = Arc<Mutex<Vec<(u64, u64)>>>;

    fn memory_fetch(bytes: Arc<Vec<u8>>) -> (RangeFetch, RecordedRanges) {
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let recorded = ranges.clone();
        let fetch: RangeFetch = Arc::new(move |start, end, _cancellation| {
            let bytes = bytes.clone();
            let recorded = recorded.clone();
            Box::pin(async move {
                recorded.lock().unwrap().push((start, end));
                let start = usize::try_from(start).expect("test range start fits");
                let end = usize::try_from(end).expect("test range end fits");
                Ok(bytes[start..=end].to_vec())
            })
        });
        (fetch, ranges)
    }

    #[tokio::test]
    async fn range_fetch_reuses_the_flushed_front_buffer_before_network() {
        use tokio::io::AsyncWriteExt as _;

        let bytes = Arc::new(
            (0..160)
                .map(|index| u8::try_from(index).unwrap())
                .collect::<Vec<_>>(),
        );
        let buffer = ProgressiveFile::new(AudioFormat::Mp3, Some(bytes.len() as u64)).unwrap();
        let path = buffer.path().to_path_buf();
        let reader = buffer.reader().unwrap();
        let completion = reader.completion();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..100]).await.unwrap();
        writer.flush().await.unwrap();

        let (remote, ranges) = memory_fetch(bytes.clone());
        let local = fetch_reusing_front_buffer(
            remote.clone(),
            path.clone(),
            completion.clone(),
            20,
            39,
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(local, bytes[20..=39]);
        assert!(ranges.lock().unwrap().is_empty());

        let split =
            fetch_reusing_front_buffer(remote, path, completion, 90, 119, CancellationToken::new())
                .await
                .unwrap();
        assert_eq!(split, bytes[90..=119]);
        assert_eq!(*ranges.lock().unwrap(), vec![(100, 119)]);
    }

    #[derive(Debug)]
    struct FetchLog {
        started: usize,
        cancelled: usize,
        completed: usize,
    }

    /// A range fetch with latency that honors cancellation, plus a log of
    /// how every fetch ended. Superseded seek workers must observe their
    /// cancellation instead of keeping the fetch alive.
    fn delayed_cancellable_fetch(
        bytes: Arc<Vec<u8>>,
        delay: Duration,
    ) -> (RangeFetch, Arc<Mutex<FetchLog>>) {
        let log = Arc::new(Mutex::new(FetchLog {
            started: 0,
            cancelled: 0,
            completed: 0,
        }));
        let recorded = log.clone();
        let fetch: RangeFetch = Arc::new(move |start, end, cancellation| {
            let bytes = bytes.clone();
            let recorded = recorded.clone();
            Box::pin(async move {
                recorded.lock().unwrap().started += 1;
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        recorded.lock().unwrap().cancelled += 1;
                        Err("Playback request cancelled".to_string())
                    }
                    _ = tokio::time::sleep(delay) => {
                        recorded.lock().unwrap().completed += 1;
                        let start = usize::try_from(start).expect("test range start fits");
                        let end = usize::try_from(end).expect("test range end fits");
                        Ok(bytes[start..=end].to_vec())
                    }
                }
            })
        });
        (fetch, log)
    }

    /// Mirrors the discard the engine applies to a timeline startup.
    fn discard_samples(decoder: &mut rodio::Decoder<ProgressiveReader>, position: Duration) {
        let target = position.as_nanos()
            * u128::from(u64::from(decoder.sample_rate()))
            * u128::from(u64::from(decoder.channels()))
            / 1_000_000_000;
        for _ in 0..target {
            decoder.next();
        }
    }

    /// Appends the UTF-8-like coded number every FLAC frame header carries.
    fn push_flac_coded_number(bytes: &mut Vec<u8>, number: u64) {
        if number < 0x80 {
            bytes.push(u8::try_from(number).unwrap());
        } else if number < 0x800 {
            bytes.push(0xc0 | u8::try_from(number >> 6).unwrap());
            bytes.push(0x80 | u8::try_from(number & 0x3f).unwrap());
        } else {
            panic!("the fixture keeps frame numbers below 2048");
        }
    }

    /// One valid fixed-blocksize FLAC frame header: sync, 256 samples per
    /// block, the sample rate left to STREAMINFO, stereo, 16 bit, then the
    /// coded frame number and the CRC-8 the parser verifies.
    fn flac_frame_header_bytes(number: u64) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xf8, 0x80, 0x18];
        push_flac_coded_number(&mut bytes, number);
        let crc = crc8(&bytes);
        bytes.push(crc);
        bytes
    }

    /// Builds a synthetic variable bitrate FLAC file: a cheap quiet intro of
    /// 512-byte frames followed by an expensive loud tail of 3430-byte
    /// frames, each covering 256 samples at 1 kHz. The quiet intro costs a
    /// quarter of the file average, so the byte ratio estimate for a target
    /// inside it overshoots deep into the loud tail.
    fn synthetic_vbr_flac() -> (Arc<Vec<u8>>, Vec<(u64, Duration)>) {
        const SAMPLE_RATE: u64 = 1_000;
        const BLOCK_SAMPLES: u16 = 256;
        const QUIET_FRAME_BYTES: usize = 512;
        const LOUD_FRAME_BYTES: usize = 3_430;
        const QUIET_FRAMES: u64 = 235;
        const LOUD_FRAMES: u64 = 272;

        let mut stream_info = Vec::new();
        stream_info.extend_from_slice(&BLOCK_SAMPLES.to_be_bytes());
        stream_info.extend_from_slice(&BLOCK_SAMPLES.to_be_bytes());
        stream_info.extend_from_slice(&[0; 6]);
        let packed = (SAMPLE_RATE << 44)
            | (u64::from(2_u16 - 1) << 41)
            | (u64::from(16_u8 - 1) << 36)
            | ((QUIET_FRAMES + LOUD_FRAMES) * u64::from(BLOCK_SAMPLES));
        stream_info.extend_from_slice(&packed.to_be_bytes());
        stream_info.extend_from_slice(&[0; 16]);
        assert_eq!(stream_info.len(), 34);

        let mut bytes = b"fLaC".to_vec();
        bytes.push(0x80);
        bytes.extend_from_slice(&34_u32.to_be_bytes()[1..]);
        bytes.extend_from_slice(&stream_info);

        let mut frames = Vec::new();
        let mut number = 0_u64;
        for _ in 0..QUIET_FRAMES {
            frames.push((bytes.len() as u64, number));
            let header = flac_frame_header_bytes(number);
            bytes.extend_from_slice(&header);
            bytes.resize(bytes.len() + QUIET_FRAME_BYTES - header.len(), 0);
            number += 1;
        }
        for _ in 0..LOUD_FRAMES {
            frames.push((bytes.len() as u64, number));
            let header = flac_frame_header_bytes(number);
            bytes.extend_from_slice(&header);
            bytes.resize(bytes.len() + LOUD_FRAME_BYTES - header.len(), 0);
            number += 1;
        }
        let positions = frames
            .into_iter()
            .map(|(offset, number)| {
                (
                    offset,
                    Duration::from_nanos(
                        number * u64::from(BLOCK_SAMPLES) * 1_000_000_000 / SAMPLE_RATE,
                    ),
                )
            })
            .collect();
        (Arc::new(bytes), positions)
    }

    /// A mid-download seek into the cheap intro of a variable bitrate FLAC
    /// must hand the engine a suffix that starts at or before the target and
    /// reports the exact discard to reach it. The unbracketed probing this
    /// pins used to exhaust its attempts while overshooting and return a
    /// frame past the target, which zeroed the discard and resumed playback
    /// past the requested position.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flac_seek_never_lands_past_the_target_on_variable_bitrate() {
        let (bytes, positions) = synthetic_vbr_flac();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(130);
        for target in [Duration::from_secs(20), Duration::from_secs(50)] {
            let (fetch, ranges) = memory_fetch(bytes.clone());
            let session = RangeTimelineSession::new(
                RangeSeekFormat::Flac,
                fetch,
                total,
                duration,
                tokio::runtime::Handle::current(),
                CancellationToken::new(),
                DownloadPauseGate::new(),
            );
            let request = TimelineSeekSession::request(&session, target).unwrap();
            assert_eq!(request.format, AudioFormat::Flac);
            let startup = request
                .startup
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap();
            let discard = startup
                .intra_segment_offset
                .expect("the session must report the discard to the target");

            // The suffix stream starts where the first full chunk was
            // fetched; probe and header requests stay at 64 KiB.
            let ranges = ranges.lock().unwrap().clone();
            let suffix_start = ranges
                .iter()
                .find(|(start, end)| end - start + 1 == SUFFIX_STARTUP_CHUNK)
                .map(|(start, _)| *start)
                .expect("the suffix must be streamed in full chunks");
            let position = positions
                .iter()
                .find(|(offset, _)| *offset == suffix_start)
                .map(|(_, position)| *position)
                .expect("the suffix must start at a frame header");

            assert!(
                position <= target,
                "the suffix must start at or before the target, started at {position:?}"
            );
            assert_eq!(
                discard,
                target - position,
                "the discard must bridge from the suffix start to the target"
            );
            assert!(
                target - position <= FLAC_FORWARD_LIMIT,
                "the discard must stay bounded, was {}",
                (target - position).as_secs_f32()
            );
            drop(startup.file);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn range_seek_session_lands_the_suffix_at_the_target() {
        let Some(file) = make_mp3_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping range seek session test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let (fetch, _ranges) = memory_fetch(Arc::new(bytes));
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );

        let request = TimelineSeekSession::request(&session, Duration::from_millis(3_500)).unwrap();
        assert_eq!(request.format, AudioFormat::Mp3);
        assert_eq!(request.intra_segment_offset, Duration::ZERO);
        let startup = request.startup.recv().unwrap().unwrap();
        assert_eq!(startup.intra_segment_offset, None);

        // The suffix decoder must land inside the tone region that starts at
        // 3s, not in the silence at the start of the track.
        let mut decoder = rodio::Decoder::builder()
            .with_data(startup.reader)
            .with_hint("mp3")
            .build()
            .unwrap();
        let samples = (0..4_096)
            .map(|_| decoder.next().unwrap())
            .collect::<Vec<_>>();
        let rms = sample_rms(&samples);
        assert!(
            rms > 0.2,
            "the range suffix must land in the tone region, RMS was {rms}"
        );
        drop(startup.file);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flac_range_seek_session_discards_up_to_the_target() {
        let Some(file) = make_flac_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping FLAC range seek session test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let (fetch, ranges) = memory_fetch(Arc::new(bytes));
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );

        let request = TimelineSeekSession::request(&session, Duration::from_millis(3_500)).unwrap();
        assert_eq!(request.format, AudioFormat::Flac);
        assert_eq!(request.intra_segment_offset, Duration::ZERO);
        let startup = request.startup.recv().unwrap().unwrap();
        // The byte ratio estimate lands inside the tone but before the
        // target, so the session must report a real discard to reach 3.5s.
        let discard = startup
            .intra_segment_offset
            .expect("the FLAC session must report a discard");
        assert!(
            discard >= Duration::from_millis(100),
            "the FLAC discard was too small: {discard:?}"
        );
        assert!(
            discard <= Duration::from_secs(2),
            "the FLAC discard was too large: {discard:?}"
        );

        // The data must come from a mid-file piece, not the whole file.
        let ranges = ranges.lock().unwrap().clone();
        assert!(
            ranges.iter().any(|(start, _)| *start > total / 4),
            "the FLAC seek must fetch a mid-file piece, ranges were {ranges:?}"
        );

        let mut decoder = rodio::Decoder::builder()
            .with_data(startup.reader)
            .with_hint("flac")
            .build()
            .unwrap();
        discard_samples(&mut decoder, discard);
        let samples = (0..4_096)
            .map(|_| decoder.next().unwrap())
            .collect::<Vec<_>>();
        let rms = sample_rms(&samples);
        assert!(
            rms > 0.2,
            "the corrected FLAC suffix must land in the tone region, RMS was {rms}"
        );
        drop(startup.file);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flac_range_seek_backs_up_when_the_estimate_overshoots() {
        let Some(file) = make_flac_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping FLAC overshoot test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let (fetch, _ranges) = memory_fetch(Arc::new(bytes));
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );

        // 1.9s sits in the silent intro while the byte ratio estimate lands
        // inside the tone, so the session must back up before the target and
        // discard the distance instead of starting in the tone.
        let request = TimelineSeekSession::request(&session, Duration::from_millis(1_900)).unwrap();
        let startup = request.startup.recv().unwrap().unwrap();
        let discard = startup
            .intra_segment_offset
            .expect("the FLAC session must report a discard");
        assert!(
            discard >= Duration::from_millis(1_500),
            "the backed up FLAC discard was too small: {discard:?}"
        );

        let mut decoder = rodio::Decoder::builder()
            .with_data(startup.reader)
            .with_hint("flac")
            .build()
            .unwrap();
        discard_samples(&mut decoder, discard);
        let samples = (0..4_096)
            .map(|_| decoder.next().unwrap())
            .collect::<Vec<_>>();
        let rms = sample_rms(&samples);
        assert!(
            rms < 0.02,
            "the corrected FLAC suffix must land in the silent intro, RMS was {rms}"
        );
        drop(startup.file);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mid_download_mp3_seek_applies_during_the_download() {
        use super::super::super::engine::{
            AudioEngine, AudioOutputTarget, RodioEngine, RodioEngine as Engine, SeekOutcome,
        };
        use super::super::super::progressive::ProgressiveFile;
        use super::super::super::standby::PreparedSource;
        use super::super::ResolvedProgressiveAudio;
        use tokio::io::AsyncWriteExt as _;

        let Some(file) = make_mp3_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping mid download seek test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let frontier =
            super::super::super::progressive::startup_bytes(AudioFormat::Mp3, Some(total))
                .min(total);
        let buffer = ProgressiveFile::new(AudioFormat::Mp3, Some(total)).unwrap();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..frontier as usize]).await.unwrap();
        writer.flush().await.unwrap();
        let (fetch, ranges) = memory_fetch(Arc::new(bytes));
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        ));
        let audio = ResolvedProgressiveAudio {
            reader: buffer.reader().unwrap(),
            file: buffer.into_file(),
            duration: Some(duration),
            format: AudioFormat::Mp3,
            total: Some(total),
            timeline_size_unknown: false,
            declared_bitrate: Some(128),
            initial_downloaded: frontier,
            initial_buffered_fraction: None,
            fully_cached: false,
            timeline_seek_session: Some(session),
            worker: None,
        };
        let prepared = Engine::decode_progressive(audio).unwrap();
        let (source, file, progressive_seek) = prepared.into_parts();
        assert!(
            progressive_seek
                .as_ref()
                .is_some_and(|seek| seek.timeline_seek_session.is_some())
        );

        let Ok(mut engine) = RodioEngine::new(AudioOutputTarget::SystemDefault) else {
            eprintln!("no audio device; skipping mid download seek test");
            return;
        };
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());
        engine.load(prepared, 1.0, true);

        // The buffer only holds the startup prefix, so the seek must go
        // through the session and apply while the download is still short.
        let started = std::time::Instant::now();
        let outcome = engine.seek(Duration::from_millis(3_500)).unwrap();
        assert_eq!(outcome, SeekOutcome::Deferred);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the seek must defer instantly, took {:?}",
            started.elapsed()
        );

        let mut applied = None;
        for _ in 0..80 {
            match engine.apply_deferred_seek().unwrap() {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    applied = Some(engine.position());
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let position = applied.expect("the mid download seek must apply");
        assert!(
            position >= Duration::from_millis(3_400),
            "the seek must land at the target, was {position:?}"
        );
        let fetched_before_local_seek = ranges.lock().unwrap().len();
        assert_eq!(
            engine.seek(Duration::from_millis(3_700)).unwrap(),
            SeekOutcome::Deferred
        );
        let mut landed_again = false;
        for _ in 0..80 {
            match engine.apply_deferred_seek().unwrap() {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    landed_again = true;
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        assert!(landed_again, "a buffered suffix reseek must apply");
        assert_eq!(
            ranges.lock().unwrap().len(),
            fetched_before_local_seek,
            "a buffered suffix reseek must not fetch another network range"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mid_download_flac_seek_applies_during_the_download() {
        use super::super::super::engine::{
            AudioEngine, AudioOutputTarget, RodioEngine, RodioEngine as Engine, SeekOutcome,
        };
        use super::super::super::progressive::ProgressiveFile;
        use super::super::super::standby::PreparedSource;
        use super::super::ResolvedProgressiveAudio;
        use tokio::io::AsyncWriteExt as _;

        let Some(file) = make_flac_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping mid download FLAC seek test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        // The frontier must reach past the FLAC metadata header (ffmpeg
        // pads it to about 8KB) plus a few frames so the decoder probe can
        // complete against the static buffer; a live source keeps writing.
        let frontier =
            super::super::super::progressive::startup_bytes(AudioFormat::Flac, Some(total))
                .max(32 * 1024)
                .min(total);
        let buffer = ProgressiveFile::new(AudioFormat::Flac, Some(total)).unwrap();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..frontier as usize]).await.unwrap();
        writer.flush().await.unwrap();
        let (fetch, ranges) = memory_fetch(Arc::new(bytes));
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        ));
        let audio = ResolvedProgressiveAudio {
            reader: buffer.reader().unwrap(),
            file: buffer.into_file(),
            duration: Some(duration),
            format: AudioFormat::Flac,
            total: Some(total),
            timeline_size_unknown: false,
            declared_bitrate: Some(1411),
            initial_downloaded: frontier,
            initial_buffered_fraction: None,
            fully_cached: false,
            timeline_seek_session: Some(session),
            worker: None,
        };
        let prepared = Engine::decode_progressive(audio).unwrap();
        let (source, file, progressive_seek) = prepared.into_parts();
        assert!(
            progressive_seek
                .as_ref()
                .is_some_and(|seek| seek.timeline_seek_session.is_some())
        );

        let Ok(mut engine) = RodioEngine::new(AudioOutputTarget::SystemDefault) else {
            eprintln!("no audio device; skipping mid download FLAC seek test");
            return;
        };
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());
        engine.load(prepared, 1.0, true);

        let started = std::time::Instant::now();
        let outcome = engine.seek(Duration::from_millis(3_500)).unwrap();
        assert_eq!(outcome, SeekOutcome::Deferred);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the FLAC seek must defer instantly, took {:?}",
            started.elapsed()
        );

        let mut applied = None;
        for _ in 0..80 {
            match engine.apply_deferred_seek().unwrap() {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    applied = Some(engine.position());
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let position = applied.expect("the mid download FLAC seek must apply");
        assert!(
            position >= Duration::from_millis(3_400),
            "the FLAC seek must land at the target, was {position:?}"
        );
        let fetched_before_local_seek = ranges.lock().unwrap().len();
        assert_eq!(
            engine.seek(Duration::from_millis(3_700)).unwrap(),
            SeekOutcome::Deferred
        );
        let mut landed_again = false;
        for _ in 0..80 {
            match engine.apply_deferred_seek().unwrap() {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    landed_again = true;
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        assert!(landed_again, "a buffered FLAC suffix reseek must apply");
        assert_eq!(
            ranges.lock().unwrap().len(),
            fetched_before_local_seek,
            "a buffered FLAC suffix reseek must not fetch another network range"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn buffered_flac_seek_uses_the_local_front_path() {
        use tokio::io::AsyncWriteExt as _;

        let Some(file) = make_flac_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping buffered FLAC seek test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let frontier = bytes.len() * 3 / 4;
        let buffer = ProgressiveFile::new(AudioFormat::Flac, Some(bytes.len() as u64)).unwrap();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..frontier]).await.unwrap();
        writer.flush().await.unwrap();
        let completion = buffer.reader().unwrap().completion();
        let remote: RangeFetch = Arc::new(|start, end, _| {
            Box::pin(async move { Err(format!("unexpected remote request for {start}..={end}")) })
        });
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            remote,
            bytes.len() as u64,
            Duration::from_secs(4),
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        )
        .with_front_buffer(buffer.path().to_path_buf(), completion);
        assert!(
            session.can_seek_from_front(Duration::from_millis(500)),
            "a verified FLAC frame in the front buffer must take the local path"
        );
        let mut decoder = rodio::Decoder::builder()
            .with_data(buffer.reader().unwrap())
            .with_hint("flac")
            .with_byte_len(frontier as u64)
            .build()
            .unwrap();
        decoder.try_seek(Duration::from_millis(500)).unwrap();
        assert!(decoder.next().is_some());
    }

    #[test]
    fn flac_frame_coverage_grows_with_flushed_suffix_chunks() {
        let Some(file) = make_flac_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping FLAC coverage test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let (header, info) = parse_flac_header(&bytes).unwrap();
        let start = header.len();
        let split = start + (bytes.len() - start) / 2;
        let coverage = FlacCoverage::new();
        let mut scanner = FlacScanner::new(info, start as u64, coverage.clone());
        scanner.ingest(start as u64, &bytes[start..split]);
        assert!(coverage.contains(Duration::from_millis(500), split as u64));
        assert!(!coverage.contains(Duration::from_millis(3_500), split as u64));
        scanner.ingest(split as u64, &bytes[split..]);
        assert!(coverage.contains(Duration::from_secs(3), bytes.len() as u64));
    }

    /// Rapid seeks on a session-bearing source must never destroy the
    /// buffer a newer seek needs. Every landed timeline seek retains one
    /// suffix buffer, so a plain retention FIFO evicts the live track
    /// buffer after two landed seeks; every later completed-buffer seek
    /// then fails with an unopenable playback buffer. The spam must also
    /// defer instantly, land only the final position, and cancel every
    /// superseded fetch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seek_spam_never_loses_the_live_track_buffer() {
        use super::super::super::engine::{
            AudioEngine, AudioOutputTarget, RodioEngine, RodioEngine as Engine, SeekOutcome,
        };
        use super::super::super::progressive::ProgressiveFile;
        use super::super::super::standby::PreparedSource;
        use super::super::ResolvedProgressiveAudio;
        use tokio::io::AsyncWriteExt as _;

        let Some(file) = make_mp3_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping seek spam test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let frontier =
            super::super::super::progressive::startup_bytes(AudioFormat::Mp3, Some(total))
                .min(total);
        let buffer = ProgressiveFile::new(AudioFormat::Mp3, Some(total)).unwrap();
        let buffer_path = buffer.path().to_owned();
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..frontier as usize]).await.unwrap();
        writer.flush().await.unwrap();
        let (fetch, log) =
            delayed_cancellable_fetch(Arc::new(bytes.clone()), Duration::from_millis(30));
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        ));
        let audio = ResolvedProgressiveAudio {
            reader: buffer.reader().unwrap(),
            file: buffer.into_file(),
            duration: Some(duration),
            format: AudioFormat::Mp3,
            total: Some(total),
            timeline_size_unknown: false,
            declared_bitrate: Some(128),
            initial_downloaded: frontier,
            initial_buffered_fraction: None,
            fully_cached: false,
            timeline_seek_session: Some(session),
            worker: None,
        };
        let prepared = Engine::decode_progressive(audio).unwrap();
        let (source, file, progressive_seek) = prepared.into_parts();
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());

        let Ok(mut engine) = RodioEngine::new(AudioOutputTarget::SystemDefault) else {
            eprintln!("no audio device; skipping seek spam test");
            return;
        };
        engine.load(prepared, 1.0, true);

        // Phase 1: fire seeks back to back with no polling between them,
        // the way a dragged seekbar spams random positions. Each new seek
        // supersedes the pending reload of the previous one.
        for position_ms in [500_u64, 2_500, 1_500, 3_500, 1_000] {
            let started = std::time::Instant::now();
            let outcome = engine
                .seek(Duration::from_millis(position_ms))
                .expect("a rapid seek must defer without an error");
            assert_eq!(outcome, SeekOutcome::Deferred);
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "the seek must defer instantly, took {:?}",
                started.elapsed()
            );
        }
        let mut applied = None;
        for _ in 0..200 {
            match engine
                .apply_deferred_seek()
                .expect("a superseded seek must never surface an error")
            {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    applied = Some(engine.position());
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let position = applied.expect("the final rapid seek must land");
        assert!(
            position >= Duration::from_millis(950) && position < Duration::from_millis(2_000),
            "only the final rapid seek may land, position was {position:?}"
        );

        // Phase 2: keep seeking with each request landing, so several
        // suffix buffers are retained while the track buffer is still
        // downloading.
        for position_ms in [3_000_u64, 800, 2_200, 1_200] {
            let outcome = engine
                .seek(Duration::from_millis(position_ms))
                .expect("a landed-seek spam must defer without an error");
            assert_eq!(outcome, SeekOutcome::Deferred);
            let mut landed = None;
            for _ in 0..200 {
                match engine
                    .apply_deferred_seek()
                    .expect("a landed seek must never surface an error")
                {
                    SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                        landed = Some(engine.position());
                        break;
                    }
                    SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
                }
            }
            let landed = landed.expect("each spam seek must land");
            assert!(
                landed >= Duration::from_millis(position_ms.saturating_sub(100))
                    && landed < Duration::from_millis(position_ms + 1_500),
                "the seek to {position_ms}ms must land near its target, was {landed:?}"
            );
        }

        // The live track buffer must still exist for the completed-buffer
        // seek path that reopens it by path.
        assert!(
            buffer_path.exists(),
            "the live track buffer must survive the seek spam"
        );

        // Complete the background download. A seek on the completed buffer
        // must open the original buffer and land locally instead of
        // failing with an unopenable playback buffer.
        writer.write_all(&bytes[frontier as usize..]).await.unwrap();
        writer.flush().await.unwrap();
        writer.finish().await.unwrap();
        let outcome = engine
            .seek(Duration::from_millis(2_000))
            .expect("a seek on the completed buffer must open the live buffer");
        assert_eq!(outcome, SeekOutcome::Applied);
        let position = engine.position();
        assert!(
            position >= Duration::from_millis(1_950) && position < Duration::from_millis(3_500),
            "the completed-buffer seek must land at the target, was {position:?}"
        );

        // A worker can be cancelled before it enters the instrumented fetch,
        // but every fetch that did start must have reached a terminal state.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let log = log.lock().unwrap();
        assert_eq!(
            log.completed, 2,
            "only landed fetches may complete: {log:?}"
        );
        assert_eq!(
            log.started,
            log.cancelled + log.completed,
            "every started fetch must end cancelled or completed, log was {log:?}"
        );
    }

    /// A near-end seek on a fresh track must resume as soon as its suffix
    /// fetch lands, never after the original front download completes.
    /// The front download is parked by the seek's pause gate while the
    /// suffix is fetched, and the engine lands the seek from the suffix
    /// while the front buffer is still incomplete.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn near_end_seek_lands_while_the_front_download_is_paused() {
        use super::super::super::engine::{
            AudioEngine, AudioOutputTarget, RodioEngine, RodioEngine as Engine, SeekOutcome,
        };
        use super::super::super::progressive::ProgressiveFile;
        use super::super::super::standby::PreparedSource;
        use super::super::ResolvedProgressiveAudio;
        use tokio::io::AsyncWriteExt as _;

        let Some(file) = make_mp3_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping near end seek test");
            return;
        };
        let bytes = std::fs::read(file.path()).unwrap();
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let frontier =
            super::super::super::progressive::startup_bytes(AudioFormat::Mp3, Some(total))
                .min(total);

        // The front buffer holds only its startup prefix. Its download
        // shares the pause gate with the session, exactly like a live one.
        let mut buffer = ProgressiveFile::new(AudioFormat::Mp3, Some(total)).unwrap();
        let gate = DownloadPauseGate::new();
        buffer.set_pause_gate(gate.clone());
        let mut writer = buffer.writer().unwrap();
        writer.write_all(&bytes[..frontier as usize]).await.unwrap();
        writer.flush().await.unwrap();

        // The suffix fetch blocks until the test releases it, so the seek
        // can only land through the controlled fetch, never by accident.
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let fetch: RangeFetch = {
            let bytes = Arc::new(bytes.clone());
            let release = release_rx.clone();
            Arc::new(move |start, end, _cancellation| {
                let bytes = bytes.clone();
                let mut release = release.clone();
                Box::pin(async move {
                    while !*release.borrow_and_update() {
                        if release.changed().await.is_err() {
                            break;
                        }
                    }
                    let start = usize::try_from(start).expect("test range start fits");
                    let end = usize::try_from(end).expect("test range end fits");
                    Ok(bytes[start..=end].to_vec())
                })
            })
        };
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            gate,
        ));
        let audio = ResolvedProgressiveAudio {
            reader: buffer.reader().unwrap(),
            file: buffer.into_file(),
            duration: Some(duration),
            format: AudioFormat::Mp3,
            total: Some(total),
            timeline_size_unknown: false,
            declared_bitrate: Some(128),
            initial_downloaded: frontier,
            initial_buffered_fraction: None,
            fully_cached: false,
            timeline_seek_session: Some(session),
            worker: None,
        };
        let prepared = Engine::decode_progressive(audio).unwrap();
        let (source, file, progressive_seek) = prepared.into_parts();
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());

        let Ok(mut engine) = RodioEngine::new(AudioOutputTarget::SystemDefault) else {
            eprintln!("no audio device; skipping near end seek test");
            return;
        };
        engine.load(prepared, 1.0, true);

        // The front download keeps streaming behind the seek, but its next
        // chunk must park once the seek holds the gate.
        let (front_parked_tx, front_parked_rx) = tokio::sync::oneshot::channel::<()>();
        let (front_continue_tx, front_continue_rx) = tokio::sync::oneshot::channel::<()>();
        let mut front = tokio::spawn(async move {
            front_continue_rx.await.unwrap();
            writer.write_all(&bytes[frontier as usize..]).await.unwrap();
            writer.flush().await.unwrap();
            front_parked_tx.send(()).unwrap();
        });

        let started = std::time::Instant::now();
        let outcome = engine.seek(Duration::from_millis(3_900)).unwrap();
        assert_eq!(outcome, SeekOutcome::Deferred);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the seek must defer instantly, took {:?}",
            started.elapsed()
        );
        let pending = engine
            .timeline_suffix_state()
            .expect("a pending seek must expose its suffix state");
        assert!(pending.pending, "the suffix must be pending before landing");
        assert_eq!(pending.base, Duration::from_millis(3_900));

        // Let the front download attempt its next chunk: the held gate
        // parks it, so it cannot finish while the suffix fetch is blocked.
        front_continue_tx.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut front)
                .await
                .is_err(),
            "the front download must be parked while the suffix is fetched"
        );
        for _ in 0..6 {
            assert_eq!(
                engine.apply_deferred_seek().unwrap(),
                SeekOutcome::Deferred,
                "the seek must not land while its suffix fetch is blocked"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Releasing the fetch is all the seek needs: it must land from the
        // suffix while the front buffer is still incomplete.
        release_tx.send(true).unwrap();
        let mut applied = None;
        for _ in 0..200 {
            match engine.apply_deferred_seek().unwrap() {
                SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                    applied = Some(engine.position());
                    break;
                }
                SeekOutcome::Deferred => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
        let position = applied.expect("the near end seek must land from the suffix");
        assert!(
            position >= Duration::from_millis(3_800),
            "the seek must land at the target, was {position:?}"
        );
        let landed = engine
            .timeline_suffix_state()
            .expect("a landed seek must expose its suffix state");
        assert!(
            !landed.pending,
            "the suffix must not be pending after landing"
        );
        assert_eq!(landed.base, position);
        assert!(landed.written > 0, "the landed suffix must report progress");

        // With the fetch task finished the gate is released, so the parked
        // front download completes and later completed-buffer seeks keep
        // working.
        front_parked_rx
            .await
            .expect("the front download must resume once the gate is released");
        front.await.unwrap();
    }

    /// The session must report the suffix a request is fetching, so the
    /// buffering indicator can track the buffer that gates playback.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn range_session_reports_suffix_state_while_fetching() {
        let Some(file) = make_mp3_tone_fixture() else {
            eprintln!("ffmpeg is unavailable; skipping suffix state test");
            return;
        };
        let bytes = Arc::new(std::fs::read(file.path()).unwrap());
        let total = bytes.len() as u64;
        let duration = Duration::from_secs(4);
        let target = Duration::from_millis(3_500);
        let (fetch, _log) = delayed_cancellable_fetch(bytes.clone(), Duration::from_millis(40));
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );
        let start = ((target.as_secs_f64() / duration.as_secs_f64()) * total as f64) as u64;

        let request = TimelineSeekSession::request(&session, target).unwrap();
        let state = TimelineSeekSession::suffix_state(&session)
            .expect("the session must report its fetching suffix");
        assert_eq!(state.base, target);
        assert_eq!(state.written, 0, "nothing is staged before the fetch lands");
        assert_eq!(state.total, total - start);

        let startup = request.startup.recv().unwrap().unwrap();
        let mut state = TimelineSeekSession::suffix_state(&session).unwrap();
        for _ in 0..80 {
            state = TimelineSeekSession::suffix_state(&session).unwrap();
            if state.written >= state.total {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            state.written, state.total,
            "the whole suffix must eventually be reported as staged"
        );
        drop(startup.file);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn suffix_tail_waits_for_reader_demand_after_startup() {
        use std::io::Read as _;

        let bytes = Arc::new(vec![0x55; 8 * 1024 * 1024]);
        let total = bytes.len() as u64;
        let (fetch, _ranges) = memory_fetch(bytes);
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            Duration::from_secs(100),
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );
        let request = TimelineSeekSession::request(&session, Duration::ZERO).unwrap();
        let startup = tokio::task::spawn_blocking(move || request.startup.recv().unwrap().unwrap())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let staged = TimelineSeekSession::suffix_state(&session).unwrap();
        assert!(staged.written >= SUFFIX_STARTUP_CHUNK);
        assert!(
            staged.written < staged.total,
            "idle suffix must not consume the entire connection"
        );
        assert!(session.landed_suffix(Duration::ZERO).is_some());
        assert!(session.landed_suffix(Duration::from_secs(50)).is_none());

        let mut reader = startup.reader;
        let mut initial_audio = [0; 4096];
        reader.read_exact(&mut initial_audio).unwrap();
        let mut completed = None;
        for _ in 0..100 {
            let state = TimelineSeekSession::suffix_state(&session).unwrap();
            if state.written == state.total {
                completed = Some(state);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(
            completed.map(|state| state.written),
            Some(total),
            "once the active reader starts, the suffix must finish without staying one MiB ahead"
        );

        let read = tokio::task::spawn_blocking(move || {
            let mut output = Vec::new();
            reader.read_to_end(&mut output).unwrap();
            drop(startup.file);
            output.len()
        });
        let len = tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(len as u64 + initial_audio.len() as u64, total);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_partial_suffix_is_not_offered_for_local_reseek() {
        let bytes = Arc::new(vec![0x55; 8 * 1024 * 1024]);
        let (fetch, _) = memory_fetch(bytes.clone());
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            bytes.len() as u64,
            Duration::from_secs(100),
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );
        let request = session.request(Duration::ZERO).unwrap();
        let cancellation = request.cancellation.clone();
        let startup = tokio::task::spawn_blocking(move || request.startup.recv().unwrap().unwrap())
            .await
            .unwrap();
        assert!(session.landed_suffix(Duration::ZERO).is_some());
        cancellation.cancel();
        assert!(session.landed_suffix(Duration::ZERO).is_none());
        drop(startup);
    }

    /// A FLAC header larger than the first probe must be fetched by growing
    /// into its own tail instead of re-reading the whole prefix at every
    /// step, so a cover art block costs one transfer of its own bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flac_header_probe_grows_by_fetching_only_the_new_tail() {
        const PADDING: usize = 300 * 1024;
        const FRAMES: u64 = 16;
        const FRAME_BYTES: usize = 640;
        let mut stream_info = Vec::new();
        stream_info.extend_from_slice(&256_u16.to_be_bytes());
        stream_info.extend_from_slice(&256_u16.to_be_bytes());
        stream_info.extend_from_slice(&[0; 6]);
        let packed = (1_000_u64 << 44) | (1_u64 << 41) | (15_u64 << 36) | (FRAMES * 256);
        stream_info.extend_from_slice(&packed.to_be_bytes());
        stream_info.extend_from_slice(&[0; 16]);
        let mut bytes = b"fLaC".to_vec();
        bytes.push(0x00);
        bytes.extend_from_slice(&34_u32.to_be_bytes()[1..]);
        bytes.extend_from_slice(&stream_info);
        bytes.push(0x80 | 1);
        bytes.extend_from_slice(&(PADDING as u32).to_be_bytes()[1..]);
        bytes.resize(bytes.len() + PADDING, 0);
        for number in 0..FRAMES {
            let header = flac_frame_header_bytes(number);
            bytes.extend_from_slice(&header);
            bytes.resize(bytes.len() + FRAME_BYTES - header.len(), 0);
        }
        let header_len = bytes.len() - FRAMES as usize * FRAME_BYTES;
        let bytes = Arc::new(bytes);
        let total = bytes.len() as u64;
        let duration = Duration::from_millis(4_096);

        let (fetch, ranges) = memory_fetch(bytes.clone());
        let session = RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
            DownloadPauseGate::new(),
        );
        let request = TimelineSeekSession::request(&session, Duration::from_secs(4)).unwrap();
        let startup = request
            .startup
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        drop(startup.file);

        let ranges = ranges.lock().unwrap().clone();
        let mut coverage = vec![0_u8; header_len];
        for (start, end) in &ranges {
            for offset in *start..=*end {
                if let Some(slot) = coverage.get_mut(usize::try_from(offset).unwrap()) {
                    *slot += 1;
                }
            }
        }
        assert!(
            coverage.iter().all(|&count| count == 1),
            "every header byte must be fetched exactly once, ranges were {ranges:?}"
        );
    }
}
