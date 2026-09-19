use futures::{StreamExt, stream::FuturesUnordered};
use reqwest::{Client, Response, Url, header};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use super::{
    progressive::{ProgressiveFile, TimelineSeekRequest, TimelineSeekSession, TimelineSeekStartup},
    resolver::AudioFormat,
    resolver::{DownloadOutput, PlaybackDownloadError, ProgressCallback, ProgressUpdate},
    retry::{RequestClass, is_expired_media_status, send_with_retry},
};

const PLAYBACK_HOST: &str = "playback.media-streaming.soundcloud.cloud";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SEGMENTS: usize = 4096;
const MEDIA_CONCURRENCY: usize = 3;

type SegmentFuture =
    Pin<Box<dyn Future<Output = (usize, Result<Vec<u8>, PlaybackDownloadError>)> + Send>>;

/// A validated SoundCloud VOD playlist. URLs are intentionally kept private so
/// they cannot accidentally appear in diagnostics or a derived Debug value.
#[derive(Clone)]
pub(super) struct HlsDescriptor {
    manifest_url: Url,
    init_url: Url,
    segments: Vec<Url>,
    segment_durations: Vec<Duration>,
    segment_starts: Vec<Duration>,
    total_duration: Option<Duration>,
    init_fragment: Arc<OnceLock<Arc<Vec<u8>>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HlsSeekTarget {
    pub(crate) index: usize,
    pub(crate) segment_start: Duration,
    pub(crate) offset: Duration,
}

impl HlsDescriptor {
    fn cache_init_fragment(&self, bytes: Vec<u8>) {
        let _ = self.init_fragment.set(Arc::new(bytes));
    }

    fn cached_init_fragment(&self) -> Option<Arc<Vec<u8>>> {
        self.init_fragment.get().cloned()
    }

    pub(super) fn duration(&self) -> Option<Duration> {
        self.total_duration
    }

    pub(crate) fn seek_target(&self, position: Duration) -> HlsSeekTarget {
        let total = self.total_duration.unwrap_or_default();
        let target = position.min(total);
        let mut low = 0_usize;
        let mut high = self.segment_starts.len();
        while low < high {
            let middle = low + (high - low) / 2;
            if self.segment_starts[middle] <= target {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let index = low
            .saturating_sub(1)
            .min(self.segments.len().saturating_sub(1));
        let segment_start = self
            .segment_starts
            .get(index)
            .copied()
            .unwrap_or(Duration::ZERO);
        HlsSeekTarget {
            index,
            segment_start,
            offset: target.saturating_sub(segment_start),
        }
    }

    pub(super) fn buffered_fraction(&self, completed_segments: usize) -> Option<f32> {
        let total = self.duration()?.as_secs_f64();
        if total <= 0.0 {
            return None;
        }
        let buffered = self
            .segment_durations
            .iter()
            .take(completed_segments)
            .map(Duration::as_secs_f64)
            .sum::<f64>();
        Some((buffered / total).clamp(0.0, 1.0) as f32)
    }
}

/// Owns the validated HLS session for one resolved track. Seeking through this
/// session never resolves the provider again. It fetches the init fragment and
/// only the selected media fragment before handing a progressive suffix to the
/// decoder, then appends later fragments in order in the background.
pub(crate) struct HlsSeekSession {
    client: Client,
    descriptor: HlsDescriptor,
    runtime: Handle,
    track_cancellation: CancellationToken,
    max_size: u64,
    user_agent: String,
}

impl HlsSeekSession {
    pub(super) fn new(
        client: Client,
        descriptor: HlsDescriptor,
        runtime: Handle,
        track_cancellation: CancellationToken,
        max_size: u64,
        user_agent: impl Into<String>,
    ) -> Self {
        Self {
            client,
            descriptor,
            runtime,
            track_cancellation,
            max_size,
            user_agent: user_agent.into(),
        }
    }

    fn request_timeline(&self, position: Duration) -> Result<TimelineSeekRequest, String> {
        let target = self.descriptor.seek_target(position);
        let buffer = ProgressiveFile::new(AudioFormat::M4a, None)
            .map_err(|_| "A temporary HLS seek buffer could not be created".to_string())?;
        let reader = buffer
            .reader()
            .map_err(|_| "The HLS seek buffer could not be opened".to_string())?;
        let mut writer = buffer
            .writer()
            .map_err(|_| "The HLS seek buffer could not be opened".to_string())?;
        let file = buffer.into_file();
        let cancellation = self.track_cancellation.child_token();
        let worker_cancellation = cancellation.clone();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let client = self.client.clone();
        let descriptor = self.descriptor.clone();
        let user_agent = self.user_agent.clone();
        let max_size = self.max_size;
        let cached_init = self.descriptor.cached_init_fragment();
        self.runtime.spawn(async move {
            let budget = Arc::new(AtomicU64::new(0));
            let result = async {
                let mut downloaded = 0_u64;
                let init = match cached_init {
                    Some(init) => {
                        reserve_budget(&budget, init.len() as u64, max_size)?;
                        (*init).clone()
                    }
                    None => {
                        let init = fetch_media_bytes(
                            &client,
                            &descriptor.init_url,
                            &worker_cancellation,
                            &user_agent,
                            budget.clone(),
                            max_size,
                        )
                        .await?;
                        descriptor.cache_init_fragment(init.clone());
                        init
                    }
                };
                write_fragment(&mut writer, &init, &mut downloaded, &worker_cancellation).await?;
                let fragment = fetch_media_bytes(
                    &client,
                    &descriptor.segments[target.index],
                    &worker_cancellation,
                    &user_agent,
                    budget.clone(),
                    max_size,
                )
                .await?;
                let mut timeline_rebaser = FragmentTimelineRebaser::default();
                let fragment = timeline_rebaser.normalize(fragment, &worker_cancellation)?;
                write_fragment(
                    &mut writer,
                    &fragment,
                    &mut downloaded,
                    &worker_cancellation,
                )
                .await?;
                writer.mark_startup_ready();
                if startup_sender
                    .send(Ok(TimelineSeekStartup {
                        reader,
                        file,
                        intra_segment_offset: None,
                    }))
                    .is_err()
                {
                    return Err(PlaybackDownloadError::message(
                        "The HLS seek result was cancelled",
                    ));
                }
                for index in target.index.saturating_add(1)..descriptor.segments.len() {
                    let fragment = fetch_media_bytes(
                        &client,
                        &descriptor.segments[index],
                        &worker_cancellation,
                        &user_agent,
                        budget.clone(),
                        max_size,
                    )
                    .await?;
                    let fragment = timeline_rebaser.normalize(fragment, &worker_cancellation)?;
                    write_fragment(
                        &mut writer,
                        &fragment,
                        &mut downloaded,
                        &worker_cancellation,
                    )
                    .await?;
                }
                writer.finish().await.map_err(|_| {
                    PlaybackDownloadError::message("The HLS seek buffer could not be finalized")
                })?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                writer.fail(error.message.clone());
                let _ = startup_sender.send(Err(error.message));
            }
        });
        Ok(TimelineSeekRequest {
            format: AudioFormat::M4a,
            intra_segment_offset: target.offset,
            cancellation,
            startup: startup_receiver,
        })
    }
}

impl TimelineSeekSession for HlsSeekSession {
    fn request(&self, position: Duration) -> Result<TimelineSeekRequest, String> {
        self.request_timeline(position)
    }
}

/// Fetch and validate a simple SoundCloud media playlist before selecting HLS
/// as the playback source. Invalid or unsupported playlists are returned as a
/// plain error so the resolver can try its progressive fallback.
pub(super) async fn inspect(
    client: &Client,
    manifest_url: &str,
    cancellation: &CancellationToken,
    user_agent: &str,
) -> Result<HlsDescriptor, String> {
    let manifest_url = parse_playback_url(manifest_url)?;
    let response = send_with_retry(
        "soundcloud.hls.manifest",
        RequestClass::Media,
        cancellation,
        || {
            client
                .get(manifest_url.clone())
                .header(header::USER_AGENT, user_agent)
                .header(
                    header::ACCEPT,
                    "application/vnd.apple.mpegurl, application/x-mpegURL, */*",
                )
                .header(header::ACCEPT_ENCODING, "identity")
        },
    )
    .await?;
    validate_response(&response)?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_MANIFEST_BYTES as u64)
    {
        return Err("SoundCloud returned an oversized HLS manifest".into());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => {
            return Err("Playback request cancelled".into());
        }
        chunk = stream.next() => chunk,
    } {
        let chunk =
            chunk.map_err(|_| "SoundCloud returned an unreadable HLS manifest".to_string())?;
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "SoundCloud returned an oversized HLS manifest".to_string())?;
        if next_len > MAX_MANIFEST_BYTES {
            return Err("SoundCloud returned an oversized HLS manifest".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if cancellation.is_cancelled() {
        return Err("Playback request cancelled".into());
    }
    let body = std::str::from_utf8(&bytes)
        .map_err(|_| "SoundCloud returned a non-text HLS manifest".to_string())?;
    parse_manifest(&manifest_url, body)
}

/// Download the validated fragmented MP4 playlist directly into the existing
/// playback/download writer. The init fragment and first media segment are
/// enough for Symphonia to start decoding, while later segments continue in
/// order on the same writer.
pub(super) async fn download<W>(
    client: &Client,
    descriptor: &HlsDescriptor,
    output: &mut W,
    cancellation: &CancellationToken,
    progress: Option<&ProgressCallback>,
    max_size: u64,
    user_agent: &str,
) -> Result<(), PlaybackDownloadError>
where
    W: DownloadOutput,
{
    validate_playback_url(&descriptor.manifest_url).map_err(PlaybackDownloadError::message)?;
    output.set_total_hint(None);
    let budget = Arc::new(AtomicU64::new(0));
    let mut downloaded = 0_u64;
    let init = fetch_media_bytes(
        client,
        &descriptor.init_url,
        cancellation,
        user_agent,
        budget.clone(),
        max_size,
    )
    .await?;
    write_fragment(output, &init, &mut downloaded, cancellation).await?;
    descriptor.cache_init_fragment(init);

    let first_segment = fetch_media_bytes(
        client,
        &descriptor.segments[0],
        cancellation,
        user_agent,
        budget.clone(),
        max_size,
    )
    .await?;
    write_fragment(output, &first_segment, &mut downloaded, cancellation).await?;
    output.mark_progressive_startup_ready();
    report_progress(progress, downloaded, descriptor.buffered_fraction(1));

    let mut pending: FuturesUnordered<SegmentFuture> = FuturesUnordered::new();
    let mut ready = BTreeMap::new();
    let mut next_to_fetch = 1_usize;
    let mut next_to_write = 1_usize;
    fill_pending(
        &mut pending,
        descriptor,
        0,
        &mut next_to_fetch,
        client,
        cancellation,
        user_agent,
        Arc::clone(&budget),
        max_size,
    );
    while let Some((index, result)) = pending.next().await {
        ready.insert(index, result?);
        let mut drained = false;
        while let Some(segment) = ready.remove(&next_to_write) {
            if cancellation.is_cancelled() {
                return Err(PlaybackDownloadError::message("Playback request cancelled"));
            }
            write_fragment(output, &segment, &mut downloaded, cancellation).await?;
            next_to_write += 1;
            drained = true;
            report_progress(
                progress,
                downloaded,
                descriptor.buffered_fraction(next_to_write),
            );
        }
        if drained {
            fill_pending(
                &mut pending,
                descriptor,
                ready.len(),
                &mut next_to_fetch,
                client,
                cancellation,
                user_agent,
                Arc::clone(&budget),
                max_size,
            );
        }
    }
    Ok(())
}

fn fill_pending(
    pending: &mut FuturesUnordered<SegmentFuture>,
    descriptor: &HlsDescriptor,
    ready_len: usize,
    next_to_fetch: &mut usize,
    client: &Client,
    cancellation: &CancellationToken,
    user_agent: &str,
    budget: Arc<AtomicU64>,
    max_size: u64,
) {
    while *next_to_fetch < descriptor.segments.len()
        && fetch_window_slots(pending.len(), ready_len) > 0
    {
        let index = *next_to_fetch;
        let url = descriptor.segments[index].clone();
        let client = client.clone();
        let cancellation = cancellation.clone();
        let user_agent = user_agent.to_owned();
        let budget = Arc::clone(&budget);
        pending.push(Box::pin(async move {
            let result =
                fetch_media_bytes(&client, &url, &cancellation, &user_agent, budget, max_size)
                    .await;
            (index, result)
        }));
        *next_to_fetch += 1;
    }
}

fn fetch_window_slots(pending_len: usize, ready_len: usize) -> usize {
    MEDIA_CONCURRENCY.saturating_sub(pending_len.saturating_add(ready_len))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct IsoBmffBox {
    kind: [u8; 4],
    start: usize,
    header_len: usize,
    end: usize,
}

impl IsoBmffBox {
    fn payload_start(self) -> usize {
        self.start + self.header_len
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FragmentDecodeTime {
    track_id: u32,
    value: u64,
    value_offset: usize,
    width: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FragmentTimelineMetadata {
    sequence_number: u32,
    decode_times: BTreeMap<u32, u64>,
    sidx: Option<FragmentSidxMetadata>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FragmentSidxMetadata {
    version: u8,
    reference_id: u32,
    timescale: u32,
    earliest_presentation_time: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParsedSidx {
    metadata: FragmentSidxMetadata,
    earliest_offset: usize,
    width: usize,
}

#[derive(Debug)]
struct ParsedFragmentTimeline {
    metadata: FragmentTimelineMetadata,
    sequence_offset: usize,
    decode_times: Vec<FragmentDecodeTime>,
    sidx: Option<ParsedSidx>,
}

#[derive(Default, Debug)]
struct FragmentTimelineRebaser {
    baseline: Option<FragmentTimelineMetadata>,
    previous: Option<FragmentTimelineMetadata>,
}

impl FragmentTimelineRebaser {
    fn normalize(
        &mut self,
        bytes: Vec<u8>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, PlaybackDownloadError> {
        ensure_not_cancelled(cancellation)?;
        let parsed = parse_fragment_timeline(&bytes).map_err(invalid_fragment_metadata)?;
        ensure_not_cancelled(cancellation)?;
        let metadata = parsed.metadata.clone();
        let baseline = self.baseline.get_or_insert_with(|| metadata.clone());
        if !same_track_ids(&metadata.decode_times, &baseline.decode_times) {
            return Err(invalid_fragment_metadata(
                "media fragment track IDs changed within the seek suffix",
            ));
        }
        match (&baseline.sidx, &metadata.sidx) {
            (Some(baseline), Some(current)) => {
                if current.version != baseline.version
                    || current.reference_id != baseline.reference_id
                    || current.timescale != baseline.timescale
                {
                    return Err(invalid_fragment_metadata(
                        "media fragment sidx identity or version changed within the seek suffix",
                    ));
                }
            }
            (None, None) => {}
            _ => {
                return Err(invalid_fragment_metadata(
                    "media fragment sidx presence changed within the seek suffix",
                ));
            }
        }
        if let Some(previous) = &self.previous {
            if metadata.sequence_number <= previous.sequence_number {
                return Err(invalid_fragment_metadata(
                    "media fragment sequence numbers are not strictly increasing",
                ));
            }
            if metadata.decode_times.iter().any(|(track_id, current)| {
                previous
                    .decode_times
                    .get(track_id)
                    .is_none_or(|previous| current < previous)
            }) {
                return Err(invalid_fragment_metadata(
                    "media fragment decode times are not monotonic",
                ));
            }
            if let (Some(current), Some(previous)) = (&metadata.sidx, &previous.sidx)
                && current.earliest_presentation_time < previous.earliest_presentation_time
            {
                return Err(invalid_fragment_metadata(
                    "media fragment sidx presentation times are not monotonic",
                ));
            }
        }

        let normalized_sequence = metadata
            .sequence_number
            .checked_sub(baseline.sequence_number)
            .and_then(|delta| delta.checked_add(1))
            .ok_or_else(|| {
                invalid_fragment_metadata("media fragment sequence number cannot be rebased")
            })?;
        let mut normalized_decode_times = Vec::with_capacity(parsed.decode_times.len());
        for field in &parsed.decode_times {
            ensure_not_cancelled(cancellation)?;
            let track_id = field.track_id;
            let current = metadata.decode_times.get(&track_id).ok_or_else(|| {
                invalid_fragment_metadata("media fragment track IDs changed within the seek suffix")
            })?;
            let baseline = baseline.decode_times.get(&track_id).ok_or_else(|| {
                invalid_fragment_metadata("media fragment track IDs changed within the seek suffix")
            })?;
            normalized_decode_times.push(current.checked_sub(*baseline).ok_or_else(|| {
                invalid_fragment_metadata(
                    "media fragment decode time precedes the selected seek fragment",
                )
            })?);
        }
        ensure_not_cancelled(cancellation)?;
        if parsed
            .decode_times
            .iter()
            .zip(&normalized_decode_times)
            .any(|(field, value)| field.width == 4 && *value > u32::MAX as u64)
        {
            return Err(invalid_fragment_metadata(
                "rebased media fragment decode time does not fit its version",
            ));
        }
        let normalized_sidx_earliest = match (&metadata.sidx, &baseline.sidx) {
            (Some(current), Some(baseline)) => Some(
                current
                    .earliest_presentation_time
                    .checked_sub(baseline.earliest_presentation_time)
                    .ok_or_else(|| {
                        invalid_fragment_metadata(
                            "media fragment sidx presentation time precedes the selected seek fragment",
                        )
                    })?,
            ),
            (None, None) => None,
            _ => {
                return Err(invalid_fragment_metadata(
                    "media fragment sidx presence changed within the seek suffix",
                ));
            }
        };
        if let (Some(parsed_sidx), Some(value)) = (&parsed.sidx, normalized_sidx_earliest)
            && parsed_sidx.width == 4
            && value > u32::MAX as u64
        {
            return Err(invalid_fragment_metadata(
                "rebased media fragment sidx presentation time does not fit its version",
            ));
        }

        ensure_not_cancelled(cancellation)?;
        let mut normalized = bytes;
        normalized[parsed.sequence_offset..parsed.sequence_offset + 4]
            .copy_from_slice(&normalized_sequence.to_be_bytes());
        for (field, value) in parsed.decode_times.iter().zip(normalized_decode_times) {
            ensure_not_cancelled(cancellation)?;
            write_unsigned(&mut normalized, field.value_offset, field.width, value)
                .map_err(invalid_fragment_metadata)?;
        }
        if let (Some(parsed_sidx), Some(value)) = (&parsed.sidx, normalized_sidx_earliest) {
            ensure_not_cancelled(cancellation)?;
            write_unsigned(
                &mut normalized,
                parsed_sidx.earliest_offset,
                parsed_sidx.width,
                value,
            )
            .map_err(invalid_fragment_metadata)?;
        }
        ensure_not_cancelled(cancellation)?;
        self.previous = Some(metadata);
        Ok(normalized)
    }
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), PlaybackDownloadError> {
    if cancellation.is_cancelled() {
        Err(PlaybackDownloadError::message("Playback request cancelled"))
    } else {
        Ok(())
    }
}

fn invalid_fragment_metadata(reason: impl Into<String>) -> PlaybackDownloadError {
    PlaybackDownloadError::message(format!(
        "SoundCloud HLS seek fragment has invalid ISO-BMFF timeline metadata: {}",
        reason.into()
    ))
}

fn parse_fragment_timeline(bytes: &[u8]) -> Result<ParsedFragmentTimeline, String> {
    let mut offset = 0_usize;
    let mut moof = None;
    let mut sidx = None;
    let mut has_nonempty_mdat = false;
    while offset < bytes.len() {
        let box_info = parse_iso_bmff_box(bytes, offset, bytes.len())?;
        if box_info.kind == *b"moof" {
            if moof.replace(box_info).is_some() {
                return Err("media fragment contains multiple moof boxes".into());
            }
        } else if box_info.kind == *b"sidx" {
            if sidx.is_some() {
                return Err("media fragment contains multiple sidx boxes".into());
            }
            sidx = Some(parse_sidx(bytes, box_info)?);
        } else if box_info.kind == *b"mdat" && box_info.payload_start() < box_info.end {
            has_nonempty_mdat = true;
        }
        offset = box_info.end;
    }
    let moof = moof.ok_or_else(|| "media fragment does not contain a moof box".to_string())?;

    let mut offset = moof.payload_start();
    let mut sequence_offset = None;
    let mut decode_times = Vec::new();
    let mut track_times = BTreeMap::new();
    let mut traf_count = 0_usize;
    while offset < moof.end {
        let child = parse_iso_bmff_box(bytes, offset, moof.end)?;
        if child.kind == *b"mfhd" {
            if sequence_offset.is_some() {
                return Err("media fragment contains multiple mfhd boxes".into());
            }
            let payload = child.payload_start();
            if child.end - payload < 8 {
                return Err("media fragment mfhd box is truncated".into());
            }
            if bytes[payload] != 0 {
                return Err("media fragment mfhd version is unsupported".into());
            }
            sequence_offset = Some(payload + 4);
        } else if child.kind == *b"traf" {
            traf_count += 1;
            let decode_time = parse_traf_timeline(bytes, child)?;
            if track_times
                .insert(decode_time.track_id, decode_time.value)
                .is_some()
            {
                return Err("media fragment contains duplicate tfhd track IDs".into());
            }
            decode_times.push(decode_time);
        }
        offset = child.end;
    }
    let sequence_offset = sequence_offset.ok_or_else(|| {
        "media fragment does not contain the required mfhd sequence number".to_string()
    })?;
    if traf_count == 0 {
        return Err("media fragment does not contain a traf box".into());
    }
    if decode_times.is_empty() {
        return Err("media fragment does not contain a tfdt decode time".into());
    }
    if !has_nonempty_mdat {
        return Err("media fragment does not contain a nonempty top-level mdat box".into());
    }

    let sequence_number = read_u32(bytes, sequence_offset)?;
    let metadata = FragmentTimelineMetadata {
        sequence_number,
        decode_times: track_times,
        sidx: sidx.map(|parsed| parsed.metadata),
    };
    Ok(ParsedFragmentTimeline {
        metadata,
        sequence_offset,
        decode_times,
        sidx,
    })
}

fn parse_sidx(bytes: &[u8], sidx: IsoBmffBox) -> Result<ParsedSidx, String> {
    let payload = sidx.payload_start();
    let payload_len = sidx.end - payload;
    if payload_len < 4 {
        return Err("media fragment sidx box is truncated".into());
    }
    let version = bytes[payload];
    let (width, earliest_offset, first_offset_offset, reference_count_offset) = match version {
        0 => (4, payload + 12, payload + 16, payload + 22),
        1 => (8, payload + 12, payload + 20, payload + 30),
        _ => return Err("media fragment sidx version is unsupported".into()),
    };
    let minimum_payload_len = reference_count_offset
        .checked_add(2)
        .and_then(|end| end.checked_sub(payload))
        .ok_or_else(|| "media fragment sidx boundary overflowed".to_string())?;
    if payload_len < minimum_payload_len {
        return Err("media fragment sidx box is truncated".into());
    }
    let reference_id = read_u32(bytes, payload + 4)?;
    let timescale = read_u32(bytes, payload + 8)?;
    if timescale == 0 {
        return Err("media fragment sidx timescale is zero".into());
    }
    let earliest_presentation_time = if width == 4 {
        u64::from(read_u32(bytes, earliest_offset)?)
    } else {
        read_u64(bytes, earliest_offset)?
    };
    let _first_offset = if width == 4 {
        u64::from(read_u32(bytes, first_offset_offset)?)
    } else {
        read_u64(bytes, first_offset_offset)?
    };
    let reference_count = usize::from(read_u16(bytes, reference_count_offset)?);
    let references_start = reference_count_offset
        .checked_add(2)
        .ok_or_else(|| "media fragment sidx boundary overflowed".to_string())?;
    let references_len = reference_count
        .checked_mul(12)
        .ok_or_else(|| "media fragment sidx reference list is too large".to_string())?;
    let references_end = references_start
        .checked_add(references_len)
        .ok_or_else(|| "media fragment sidx boundary overflowed".to_string())?;
    if references_end > sidx.end {
        return Err("media fragment sidx references are truncated".into());
    }
    Ok(ParsedSidx {
        metadata: FragmentSidxMetadata {
            version,
            reference_id,
            timescale,
            earliest_presentation_time,
        },
        earliest_offset,
        width,
    })
}

fn parse_traf_timeline(bytes: &[u8], traf: IsoBmffBox) -> Result<FragmentDecodeTime, String> {
    let mut offset = traf.payload_start();
    let mut track_id = None;
    let mut decode_time = None;
    while offset < traf.end {
        let child = parse_iso_bmff_box(bytes, offset, traf.end)?;
        if child.kind == *b"tfhd" {
            if track_id.is_some() {
                return Err("media fragment traf contains multiple tfhd boxes".into());
            }
            let payload = child.payload_start();
            if child.end - payload < 8 {
                return Err("media fragment tfhd box is truncated".into());
            }
            if bytes[payload] != 0 {
                return Err("media fragment tfhd version is unsupported".into());
            }
            let id = read_u32(bytes, payload + 4)?;
            if id == 0 {
                return Err("media fragment tfhd track ID is zero".into());
            }
            track_id = Some(id);
        } else if child.kind == *b"tfdt" {
            if decode_time.is_some() {
                return Err("media fragment traf contains multiple tfdt boxes".into());
            }
            let payload = child.payload_start();
            if child.end - payload < 8 {
                return Err("media fragment tfdt box is truncated".into());
            }
            let version = bytes[payload];
            let (width, value) = match version {
                0 => (4, u64::from(read_u32(bytes, payload + 4)?)),
                1 => (8, read_u64(bytes, payload + 4)?),
                _ => return Err("media fragment tfdt version is unsupported".into()),
            };
            if child.end - payload < 4 + width {
                return Err("media fragment tfdt value is truncated".into());
            }
            decode_time = Some(FragmentDecodeTime {
                track_id: 0,
                value,
                value_offset: payload + 4,
                width,
            });
        }
        offset = child.end;
    }
    let track_id =
        track_id.ok_or_else(|| "media fragment traf does not contain a tfhd".to_string())?;
    let mut decode_time =
        decode_time.ok_or_else(|| "media fragment traf does not contain a tfdt box".to_string())?;
    decode_time.track_id = track_id;
    Ok(decode_time)
}

fn same_track_ids(left: &BTreeMap<u32, u64>, right: &BTreeMap<u32, u64>) -> bool {
    left.len() == right.len()
        && left
            .keys()
            .zip(right.keys())
            .all(|(left, right)| left == right)
}

fn parse_iso_bmff_box(bytes: &[u8], start: usize, parent_end: usize) -> Result<IsoBmffBox, String> {
    if start > parent_end || parent_end > bytes.len() {
        return Err("media fragment box boundary is invalid".into());
    }
    let header_end = start
        .checked_add(8)
        .ok_or_else(|| "media fragment box boundary overflowed".to_string())?;
    if header_end > parent_end {
        return Err("media fragment box header is truncated".into());
    }
    let size32 = read_u32(bytes, start)?;
    let kind = bytes[start + 4..start + 8]
        .try_into()
        .map_err(|_| "media fragment box type is truncated".to_string())?;
    let (header_len, size) = if size32 == 1 {
        let extended_end = start
            .checked_add(16)
            .ok_or_else(|| "media fragment extended box header overflowed".to_string())?;
        if extended_end > parent_end {
            return Err("media fragment extended box header is truncated".into());
        }
        let size = read_u64(bytes, start + 8)?;
        let size = usize::try_from(size)
            .map_err(|_| "media fragment extended box size is too large".to_string())?;
        (16, size)
    } else if size32 == 0 {
        (8, parent_end - start)
    } else {
        (8, size32 as usize)
    };
    if size < header_len {
        return Err("media fragment box size is smaller than its header".into());
    }
    let end = start
        .checked_add(size)
        .ok_or_else(|| "media fragment box boundary overflowed".to_string())?;
    if end > parent_end {
        return Err("media fragment box extends past its parent".into());
    }
    Ok(IsoBmffBox {
        kind,
        start,
        header_len,
        end,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| "media fragment integer boundary overflowed".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| "media fragment integer is truncated".to_string())?;
    Ok(u32::from_be_bytes(value.try_into().map_err(|_| {
        "media fragment integer is truncated".to_string()
    })?))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let end = offset
        .checked_add(2)
        .ok_or_else(|| "media fragment integer boundary overflowed".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| "media fragment integer is truncated".to_string())?;
    Ok(u16::from_be_bytes(value.try_into().map_err(|_| {
        "media fragment integer is truncated".to_string()
    })?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| "media fragment integer boundary overflowed".to_string())?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| "media fragment integer is truncated".to_string())?;
    Ok(u64::from_be_bytes(value.try_into().map_err(|_| {
        "media fragment integer is truncated".to_string()
    })?))
}

fn write_unsigned(bytes: &mut [u8], offset: usize, width: usize, value: u64) -> Result<(), String> {
    match width {
        4 => {
            let value = u32::try_from(value)
                .map_err(|_| "media fragment integer does not fit its box version".to_string())?;
            let end = offset
                .checked_add(4)
                .ok_or_else(|| "media fragment integer boundary overflowed".to_string())?;
            let output = bytes
                .get_mut(offset..end)
                .ok_or_else(|| "media fragment integer is truncated".to_string())?;
            output.copy_from_slice(&value.to_be_bytes());
        }
        8 => {
            let end = offset
                .checked_add(8)
                .ok_or_else(|| "media fragment integer boundary overflowed".to_string())?;
            let output = bytes
                .get_mut(offset..end)
                .ok_or_else(|| "media fragment integer is truncated".to_string())?;
            output.copy_from_slice(&value.to_be_bytes());
        }
        _ => return Err("media fragment integer width is unsupported".into()),
    }
    Ok(())
}

async fn write_fragment<W>(
    output: &mut W,
    bytes: &[u8],
    downloaded: &mut u64,
    cancellation: &CancellationToken,
) -> Result<(), PlaybackDownloadError>
where
    W: DownloadOutput,
{
    if cancellation.is_cancelled() {
        return Err(PlaybackDownloadError::message("Playback request cancelled"));
    }
    *downloaded = downloaded
        .checked_add(bytes.len() as u64)
        .ok_or_else(|| PlaybackDownloadError::message("The download is too large"))?;
    output
        .write_all(bytes)
        .await
        .map_err(|_| PlaybackDownloadError::message("The playback buffer could not be written"))?;
    output.flush().await.map_err(|_| {
        PlaybackDownloadError::message("The playback buffer could not be finalized")
    })?;
    if cancellation.is_cancelled() {
        return Err(PlaybackDownloadError::message("Playback request cancelled"));
    }
    Ok(())
}

fn report_progress(
    progress: Option<&ProgressCallback>,
    downloaded: u64,
    buffered_fraction: Option<f32>,
) {
    let Some(fraction) = buffered_fraction else {
        return;
    };
    if let Some(progress) = progress {
        progress(ProgressUpdate::buffered(downloaded, fraction));
    }
}

async fn fetch_media_bytes(
    client: &Client,
    url: &Url,
    cancellation: &CancellationToken,
    user_agent: &str,
    budget: Arc<AtomicU64>,
    max_size: u64,
) -> Result<Vec<u8>, PlaybackDownloadError> {
    let response = fetch_media(client, url, cancellation, user_agent).await?;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::select! {
        _ = cancellation.cancelled() => {
            return Err(PlaybackDownloadError::message("Playback request cancelled"));
        }
        chunk = stream.next() => chunk,
    } {
        let chunk = chunk.map_err(|_| {
            PlaybackDownloadError::message("SoundCloud HLS media could not be read")
        })?;
        reserve_budget(&budget, chunk.len() as u64, max_size)?;
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn reserve_budget(
    budget: &AtomicU64,
    amount: u64,
    max_size: u64,
) -> Result<(), PlaybackDownloadError> {
    let mut current = budget.load(Ordering::Relaxed);
    loop {
        let next = current
            .checked_add(amount)
            .ok_or_else(|| PlaybackDownloadError::message("The download is too large"))?;
        if next > max_size {
            return Err(PlaybackDownloadError::message("The download is too large"));
        }
        match budget.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

async fn fetch_media(
    client: &Client,
    url: &Url,
    cancellation: &CancellationToken,
    user_agent: &str,
) -> Result<Response, PlaybackDownloadError> {
    let response = send_with_retry(
        "soundcloud.hls.segment",
        RequestClass::Media,
        cancellation,
        || {
            client
                .get(url.clone())
                .header(header::USER_AGENT, user_agent)
                .header(header::ACCEPT_ENCODING, "identity")
        },
    )
    .await
    .map_err(PlaybackDownloadError::message)?;
    if !response.status().is_success() {
        return Err(PlaybackDownloadError::media_status(
            "SoundCloud HLS",
            response.status(),
        ));
    }
    validate_playback_url(response.url()).map_err(PlaybackDownloadError::message)?;
    Ok(response)
}

fn validate_response(response: &Response) -> Result<(), String> {
    if !response.status().is_success() {
        let status = response.status();
        let reason = status.canonical_reason().unwrap_or("unknown status");
        return Err(if is_expired_media_status(status) {
            format!(
                "SoundCloud HLS media URL was rejected (HTTP {} {reason})",
                status.as_u16()
            )
        } else {
            format!(
                "SoundCloud HLS request was rejected (HTTP {} {reason})",
                status.as_u16()
            )
        });
    }
    validate_playback_url(response.url())
}

fn parse_playback_url(value: &str) -> Result<Url, String> {
    let url =
        Url::parse(value).map_err(|_| "SoundCloud returned an invalid HLS URL".to_string())?;
    validate_playback_url(&url)?;
    Ok(url)
}

fn validate_playback_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "https"
        || url
            .host_str()
            .is_none_or(|host| !host.eq_ignore_ascii_case(PLAYBACK_HOST))
        || url.username() != ""
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return Err("SoundCloud returned an unexpected HLS media URL".into());
    }
    Ok(())
}

fn parse_manifest(base_url: &Url, body: &str) -> Result<HlsDescriptor, String> {
    if body.len() > MAX_MANIFEST_BYTES {
        return Err("SoundCloud returned an oversized HLS manifest".into());
    }
    let mut lines = body.lines().map(str::trim).filter(|line| !line.is_empty());
    if lines.next() != Some("#EXTM3U") {
        return Err("SoundCloud returned an invalid HLS manifest".into());
    }

    let mut playlist_type_vod = false;
    let mut endlist = false;
    let mut map_url = None;
    let mut segments = Vec::new();
    let mut segment_durations = Vec::new();
    let mut pending_segment = false;

    for line in lines {
        if line.starts_with('#') {
            if line == "#EXT-X-PLAYLIST-TYPE:VOD" {
                playlist_type_vod = true;
            } else if line == "#EXT-X-ENDLIST" {
                endlist = true;
            } else if let Some(attributes) = line.strip_prefix("#EXT-X-MAP:") {
                if map_url.is_some() {
                    return Err("SoundCloud returned multiple HLS init maps".into());
                }
                let raw = quoted_attribute(attributes, "URI")
                    .ok_or_else(|| "SoundCloud returned an invalid HLS init map".to_string())?;
                map_url = Some(resolve_media_url(base_url, raw)?);
            } else if line.starts_with("#EXTINF:") {
                let duration = line
                    .strip_prefix("#EXTINF:")
                    .and_then(|value| value.split_once(',').map(|(duration, _)| duration))
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|duration| duration.is_finite() && *duration >= 0.0);
                if duration.is_none() || pending_segment {
                    return Err("SoundCloud returned an invalid HLS segment duration".into());
                }
                let duration = Duration::try_from_secs_f64(duration.unwrap())
                    .map_err(|_| "SoundCloud returned an invalid HLS segment duration")?;
                segment_durations.push(duration);
                pending_segment = true;
            } else if line == "#EXT-X-VERSION:7"
                || line.starts_with("#EXT-X-VERSION:")
                || line.starts_with("#EXT-X-TARGETDURATION:")
                || line.starts_with("#EXT-X-MEDIA-SEQUENCE:")
            {
                // These tags do not alter the ordered VOD fragment layout.
            } else {
                return Err("SoundCloud returned an unsupported HLS manifest".into());
            }
        } else {
            if !pending_segment {
                return Err("SoundCloud returned an HLS segment without EXTINF".into());
            }
            if segments.len() >= MAX_SEGMENTS {
                return Err("SoundCloud returned too many HLS segments".into());
            }
            segments.push(resolve_media_url(base_url, line)?);
            pending_segment = false;
        }
    }

    if pending_segment {
        return Err("SoundCloud returned an HLS segment without a URI".into());
    }
    if !playlist_type_vod || !endlist {
        return Err("SoundCloud returned a non-VOD HLS manifest".into());
    }
    let Some(init_url) = map_url else {
        return Err("SoundCloud HLS manifest did not provide an init map".into());
    };
    if segments.is_empty() {
        return Err("SoundCloud HLS manifest did not provide media segments".into());
    }

    let mut segment_starts = Vec::with_capacity(segment_durations.len());
    let mut total_duration = Some(Duration::ZERO);
    for duration in &segment_durations {
        let Some(total) = total_duration else {
            break;
        };
        segment_starts.push(total);
        total_duration = total.checked_add(*duration);
    }
    if segment_starts.len() != segments.len() {
        return Err("SoundCloud returned an invalid HLS timeline".into());
    }

    Ok(HlsDescriptor {
        manifest_url: base_url.clone(),
        init_url,
        segments,
        segment_durations,
        segment_starts,
        total_duration,
        init_fragment: Arc::new(OnceLock::new()),
    })
}

fn resolve_media_url(base_url: &Url, raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("SoundCloud returned an empty HLS media URI".into());
    }
    let mut url = base_url
        .join(raw)
        .map_err(|_| "SoundCloud returned an invalid HLS media URI".to_string())?;
    if url.query().is_none()
        && !raw.contains('?')
        && let Some(query) = base_url.query()
    {
        url.set_query(Some(query));
    }
    validate_playback_url(&url)?;
    Ok(url)
}

fn quoted_attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let mut start = 0;
    let mut quoted = false;
    let mut parts = Vec::new();
    for (index, character) in attributes.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&attributes[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&attributes[start..]);
    parts.into_iter().find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        if key.trim() != name {
            return None;
        }
        let value = value.trim();
        value.strip_prefix('"')?.strip_suffix('"')
    })
}

#[cfg(test)]
mod tests;
