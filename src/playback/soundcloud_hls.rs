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
mod tests {
    use super::*;

    impl DownloadOutput for tokio::io::Sink {
        fn set_total_hint(&mut self, _total: Option<u64>) {}
    }

    fn base_url() -> Url {
        Url::parse("https://playback.media-streaming.soundcloud.cloud/path/playlist.m3u8?signature=redacted")
            .unwrap()
    }

    fn valid_manifest() -> &'static str {
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:10\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4?signature=redacted\"\n#EXTINF:10.0,\nsegment000.m4s?signature=redacted\n#EXTINF:4.0,\nsegment001.m4s?signature=redacted\n#EXT-X-ENDLIST\n"
    }

    fn mp4_box(kind: &[u8; 4], body: &[u8], extended: bool) -> Vec<u8> {
        let size = if extended {
            16 + body.len()
        } else {
            8 + body.len()
        };
        let mut output = Vec::with_capacity(size);
        if extended {
            output.extend_from_slice(&1_u32.to_be_bytes());
        } else {
            output.extend_from_slice(
                &u32::try_from(size)
                    .expect("test box must fit a 32-bit size")
                    .to_be_bytes(),
            );
        }
        output.extend_from_slice(kind);
        if extended {
            output.extend_from_slice(&(size as u64).to_be_bytes());
        }
        output.extend_from_slice(body);
        output
    }

    fn test_fragment(
        sequence_number: u32,
        decode_times: &[(u8, u64)],
        extended: bool,
        mdat_payload: &[u8],
    ) -> Vec<u8> {
        let decode_times = decode_times
            .iter()
            .enumerate()
            .map(|(index, &(version, value))| (index as u32 + 1, version, value))
            .collect::<Vec<_>>();
        test_fragment_with_tracks(sequence_number, &decode_times, extended, None, mdat_payload)
    }

    fn test_fragment_with_sidx(
        sequence_number: u32,
        decode_times: &[(u8, u64)],
        extended: bool,
        sidx: Option<Vec<u8>>,
        mdat_payload: &[u8],
    ) -> Vec<u8> {
        let decode_times = decode_times
            .iter()
            .enumerate()
            .map(|(index, &(version, value))| (index as u32 + 1, version, value))
            .collect::<Vec<_>>();
        test_fragment_with_tracks(sequence_number, &decode_times, extended, sidx, mdat_payload)
    }

    fn test_fragment_with_tracks(
        sequence_number: u32,
        decode_times: &[(u32, u8, u64)],
        extended: bool,
        sidx: Option<Vec<u8>>,
        mdat_payload: &[u8],
    ) -> Vec<u8> {
        let mut mfhd_body = vec![0, 0, 0, 0];
        mfhd_body.extend_from_slice(&sequence_number.to_be_bytes());
        let mfhd = mp4_box(b"mfhd", &mfhd_body, extended);

        let mut moof_body = mfhd;
        for &(track_id, version, value) in decode_times {
            let mut tfhd_body = vec![0, 0, 0, 0];
            tfhd_body.extend_from_slice(&track_id.to_be_bytes());
            let tfhd = mp4_box(b"tfhd", &tfhd_body, extended);
            let mut tfdt_body = vec![version, 0, 0, 0];
            match version {
                0 => tfdt_body.extend_from_slice(
                    &u32::try_from(value)
                        .expect("v0 test decode time must fit")
                        .to_be_bytes(),
                ),
                1 => tfdt_body.extend_from_slice(&value.to_be_bytes()),
                _ => panic!("unsupported test tfdt version"),
            }
            let mut traf_body = tfhd;
            traf_body.extend(mp4_box(b"tfdt", &tfdt_body, extended));
            let traf = mp4_box(b"traf", &traf_body, extended);
            moof_body.extend(traf);
        }
        let moof = mp4_box(b"moof", &moof_body, extended);
        let mdat = mp4_box(b"mdat", mdat_payload, false);
        let mut output = sidx.unwrap_or_default();
        output.extend(moof);
        output.extend(mdat);
        output
    }

    fn test_fragment_without_tfhd(sequence_number: u32, decode_time: u64) -> Vec<u8> {
        let mut mfhd_body = vec![0, 0, 0, 0];
        mfhd_body.extend_from_slice(&sequence_number.to_be_bytes());
        let mfhd = mp4_box(b"mfhd", &mfhd_body, false);
        let mut tfdt_body = vec![1, 0, 0, 0];
        tfdt_body.extend_from_slice(&decode_time.to_be_bytes());
        let traf = mp4_box(b"traf", &mp4_box(b"tfdt", &tfdt_body, false), false);
        let mut moof_body = mfhd;
        moof_body.extend(traf);
        let mut output = mp4_box(b"moof", &moof_body, false);
        output.extend(mp4_box(b"mdat", b"payload", false));
        output
    }

    fn test_sidx(
        version: u8,
        reference_id: u32,
        timescale: u32,
        earliest_presentation_time: u64,
        first_offset: u64,
        references: &[(u32, u32, u32)],
        extended: bool,
    ) -> Vec<u8> {
        let mut body = vec![version, 0, 0, 0];
        body.extend_from_slice(&reference_id.to_be_bytes());
        body.extend_from_slice(&timescale.to_be_bytes());
        match version {
            0 => {
                body.extend_from_slice(
                    &u32::try_from(earliest_presentation_time)
                        .expect("v0 test sidx time must fit")
                        .to_be_bytes(),
                );
                body.extend_from_slice(
                    &u32::try_from(first_offset)
                        .expect("v0 test sidx offset must fit")
                        .to_be_bytes(),
                );
            }
            1 => {
                body.extend_from_slice(&earliest_presentation_time.to_be_bytes());
                body.extend_from_slice(&first_offset.to_be_bytes());
            }
            _ => panic!("unsupported test sidx version"),
        }
        body.extend_from_slice(&0_u16.to_be_bytes());
        body.extend_from_slice(
            &u16::try_from(references.len())
                .expect("test sidx reference count must fit")
                .to_be_bytes(),
        );
        for &(reference, duration, sap) in references {
            body.extend_from_slice(&reference.to_be_bytes());
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&sap.to_be_bytes());
        }
        mp4_box(b"sidx", &body, extended)
    }

    fn normalize_for_test(
        rebaser: &mut FragmentTimelineRebaser,
        bytes: Vec<u8>,
    ) -> Result<Vec<u8>, PlaybackDownloadError> {
        rebaser.normalize(bytes, &CancellationToken::new())
    }

    #[test]
    fn parses_simple_vod_playlist_and_preserves_signed_query() {
        let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(parsed.init_url.path(), "/path/init.mp4");
        assert_eq!(parsed.segments[0].path(), "/path/segment000.m4s");
        assert_eq!(parsed.segments[0].query(), Some("signature=redacted"));
        assert_eq!(parsed.buffered_fraction(1), Some(10.0 / 14.0));
        assert_eq!(parsed.buffered_fraction(2), Some(1.0));
    }

    #[test]
    fn init_fragment_cache_is_shared_by_descriptor_clones_and_written_once() {
        let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
        assert!(parsed.cached_init_fragment().is_none());

        parsed.cache_init_fragment(vec![1, 2, 3]);
        parsed.cache_init_fragment(vec![9, 9, 9]);
        let clone = parsed.clone();

        assert_eq!(
            parsed.cached_init_fragment().as_deref().map(Vec::as_slice),
            Some(&[1, 2, 3][..])
        );
        assert_eq!(
            clone.cached_init_fragment().as_deref().map(Vec::as_slice),
            Some(&[1, 2, 3][..])
        );
    }

    #[test]
    fn seek_target_uses_boundaries_and_clamps_to_the_playlist() {
        let parsed = parse_manifest(&base_url(), valid_manifest()).unwrap();
        assert_eq!(
            parsed.seek_target(Duration::ZERO),
            HlsSeekTarget {
                index: 0,
                segment_start: Duration::ZERO,
                offset: Duration::ZERO,
            }
        );
        assert_eq!(
            parsed.seek_target(Duration::from_secs(10)),
            HlsSeekTarget {
                index: 1,
                segment_start: Duration::from_secs(10),
                offset: Duration::ZERO,
            }
        );
        assert_eq!(
            parsed.seek_target(Duration::from_secs(12)),
            HlsSeekTarget {
                index: 1,
                segment_start: Duration::from_secs(10),
                offset: Duration::from_secs(2),
            }
        );
        assert_eq!(parsed.seek_target(Duration::from_secs(60)).index, 1);
        assert_eq!(
            parsed.seek_target(Duration::from_secs(60)).offset,
            Duration::from_secs(4)
        );
    }

    #[test]
    fn seek_target_scales_to_a_forty_minute_timeline() {
        let durations = vec![Duration::from_secs(4); 600];
        let mut starts = Vec::with_capacity(durations.len());
        let mut total = Duration::ZERO;
        for duration in &durations {
            starts.push(total);
            total += *duration;
        }
        let descriptor = HlsDescriptor {
            manifest_url: base_url(),
            init_url: base_url(),
            segments: (0..durations.len())
                .map(|index| base_url().join(&format!("segment{index}.m4s")).unwrap())
                .collect(),
            segment_durations: durations,
            segment_starts: starts,
            total_duration: Some(total),
            init_fragment: Arc::new(OnceLock::new()),
        };
        let target = descriptor.seek_target(Duration::from_secs(2_399));
        assert_eq!(target.index, 599);
        assert_eq!(target.segment_start, Duration::from_secs(2_396));
        assert_eq!(target.offset, Duration::from_secs(3));
    }

    #[test]
    fn ordered_fragment_indices_are_written_only_after_prior_fragments() {
        let mut ready =
            BTreeMap::from([(3_usize, b"third".to_vec()), (1_usize, b"first".to_vec())]);
        let mut assembled = Vec::new();
        let mut next = 1_usize;
        while let Some(fragment) = ready.remove(&next) {
            assembled.extend(fragment);
            next += 1;
        }
        assert_eq!(assembled, b"first");
        ready.insert(2, b"second".to_vec());
        while let Some(fragment) = ready.remove(&next) {
            assembled.extend(fragment);
            next += 1;
        }
        assert_eq!(assembled, b"firstsecondthird");
    }

    #[test]
    fn rebases_v1_sequence_and_decode_time_from_selected_suffix_fragment() {
        let mut rebaser = FragmentTimelineRebaser::default();
        let selected = test_fragment(41, &[(1, 900_000)], false, b"selected");
        let later = test_fragment(44, &[(1, 900_420)], false, b"later");

        let selected = normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();

        assert_eq!(
            parse_fragment_timeline(&selected).unwrap().metadata,
            FragmentTimelineMetadata {
                sequence_number: 1,
                decode_times: BTreeMap::from([(1, 0)]),
                sidx: None,
            }
        );
        assert_eq!(
            parse_fragment_timeline(&later).unwrap().metadata,
            FragmentTimelineMetadata {
                sequence_number: 4,
                decode_times: BTreeMap::from([(1, 420)]),
                sidx: None,
            }
        );
    }

    #[test]
    fn rebases_v0_decode_time_without_widening_the_field() {
        let mut rebaser = FragmentTimelineRebaser::default();
        let selected = test_fragment(7, &[(0, 65_000)], false, b"selected");
        let later = test_fragment(8, &[(0, 65_512)], false, b"later");

        let selected = normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();

        assert_eq!(
            parse_fragment_timeline(&selected).unwrap().metadata,
            FragmentTimelineMetadata {
                sequence_number: 1,
                decode_times: BTreeMap::from([(1, 0)]),
                sidx: None,
            }
        );
        let later_metadata = parse_fragment_timeline(&later).unwrap();
        assert_eq!(
            later_metadata.metadata.decode_times,
            BTreeMap::from([(1, 512)])
        );
        assert_eq!(later_metadata.metadata.decode_times[&1], 512);
        assert_eq!(later_metadata.decode_times[0].width, 4);
    }

    #[test]
    fn rebases_v1_sidx_time_and_preserves_first_offset_and_references() {
        let references = [(0x0000_0123, 44_100, 0x9000_0001)];
        let selected_sidx = test_sidx(1, 7, 44_100, 2_205_696, 321, &references, false);
        let later_sidx = test_sidx(1, 7, 44_100, 2_206_140, 321, &references, false);
        let selected = test_fragment_with_sidx(
            50,
            &[(1, 2_205_696)],
            false,
            Some(selected_sidx),
            b"selected",
        );
        let later =
            test_fragment_with_sidx(51, &[(1, 2_206_140)], false, Some(later_sidx), b"later");
        let mut rebaser = FragmentTimelineRebaser::default();

        let selected = normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();
        let selected_sidx = parse_fragment_timeline(&selected)
            .unwrap()
            .sidx
            .expect("selected fragment must retain sidx");
        let later_sidx = parse_fragment_timeline(&later)
            .unwrap()
            .sidx
            .expect("later fragment must retain sidx");
        assert_eq!(selected_sidx.metadata.earliest_presentation_time, 0);
        assert_eq!(later_sidx.metadata.earliest_presentation_time, 444);

        let original = test_fragment_with_sidx(
            50,
            &[(1, 2_205_696)],
            false,
            Some(test_sidx(1, 7, 44_100, 2_205_696, 321, &references, false)),
            b"selected",
        );
        let original_box = parse_iso_bmff_box(&original, 0, original.len()).unwrap();
        let after_earliest = selected_sidx.earliest_offset + selected_sidx.width;
        assert_eq!(
            &original[after_earliest..original_box.end],
            &selected[after_earliest..original_box.end]
        );
    }

    #[test]
    fn rebases_v0_sidx_time_and_keeps_its_width() {
        let references = [(0x8000_0010, 22_050, 0x7000_0002)];
        let selected_sidx = test_sidx(0, 9, 44_100, 65_000, 17, &references, false);
        let later_sidx = test_sidx(0, 9, 44_100, 65_512, 17, &references, false);
        let selected =
            test_fragment_with_sidx(60, &[(0, 65_000)], false, Some(selected_sidx), b"selected");
        let later = test_fragment_with_sidx(61, &[(0, 65_512)], false, Some(later_sidx), b"later");
        let mut rebaser = FragmentTimelineRebaser::default();

        let selected = normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();
        let selected_sidx = parse_fragment_timeline(&selected)
            .unwrap()
            .sidx
            .expect("selected fragment must retain sidx");
        let later_sidx = parse_fragment_timeline(&later)
            .unwrap()
            .sidx
            .expect("later fragment must retain sidx");
        assert_eq!(selected_sidx.metadata.earliest_presentation_time, 0);
        assert_eq!(later_sidx.metadata.earliest_presentation_time, 512);
        assert_eq!(later_sidx.width, 4);
    }

    #[test]
    fn traverses_extended_size_sidx_and_rebases_its_timeline() {
        let sidx = test_sidx(1, 10, 44_100, 1_000_000, 29, &[], true);
        let later_sidx = test_sidx(1, 10, 44_100, 1_000_001, 29, &[], true);
        let selected =
            test_fragment_with_sidx(70, &[(1, 1_000_000)], true, Some(sidx), b"selected");
        let later =
            test_fragment_with_sidx(71, &[(1, 1_000_001)], true, Some(later_sidx), b"later");
        let mut rebaser = FragmentTimelineRebaser::default();

        normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();
        assert_eq!(
            parse_fragment_timeline(&later)
                .unwrap()
                .sidx
                .unwrap()
                .metadata
                .earliest_presentation_time,
            1
        );
    }

    #[test]
    fn rejects_sidx_presence_and_identity_changes_in_a_seek_suffix() {
        let with_sidx = test_fragment_with_sidx(
            80,
            &[(1, 100)],
            false,
            Some(test_sidx(1, 11, 44_100, 100, 0, &[], false)),
            b"selected",
        );
        let without_sidx = test_fragment(81, &[(1, 101)], false, b"later");
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, with_sidx).unwrap();
        let error = normalize_for_test(&mut rebaser, without_sidx).unwrap_err();
        assert!(error.message.contains("sidx presence changed"));

        let without_sidx = test_fragment(90, &[(1, 100)], false, b"selected");
        let with_sidx = test_fragment_with_sidx(
            91,
            &[(1, 101)],
            false,
            Some(test_sidx(1, 11, 44_100, 101, 0, &[], false)),
            b"later",
        );
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, without_sidx).unwrap();
        let error = normalize_for_test(&mut rebaser, with_sidx).unwrap_err();
        assert!(error.message.contains("sidx presence changed"));

        let selected = test_fragment_with_sidx(
            100,
            &[(1, 100)],
            false,
            Some(test_sidx(1, 12, 44_100, 100, 0, &[], false)),
            b"selected",
        );
        let changed_identity = test_fragment_with_sidx(
            101,
            &[(1, 101)],
            false,
            Some(test_sidx(1, 13, 44_100, 101, 0, &[], false)),
            b"later",
        );
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, selected).unwrap();
        let error = normalize_for_test(&mut rebaser, changed_identity).unwrap_err();
        assert!(error.message.contains("sidx identity or version changed"));
    }

    #[test]
    fn traverses_extended_size_boxes_and_rebases_later_fragment() {
        let mut rebaser = FragmentTimelineRebaser::default();
        let selected = test_fragment(100, &[(1, 1_000_000)], true, b"selected");
        let later = test_fragment(101, &[(1, 1_001_024)], true, b"later");

        normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();

        assert_eq!(
            parse_fragment_timeline(&later).unwrap().metadata,
            FragmentTimelineMetadata {
                sequence_number: 2,
                decode_times: BTreeMap::from([(1, 1_024)]),
                sidx: None,
            }
        );
    }

    #[test]
    fn does_not_rewrite_mdat_bytes_that_resemble_timeline_boxes() {
        let decoy = test_fragment(
            12,
            &[(1, 800)],
            false,
            &[
                0, 0, 0, 16, b'm', b'f', b'h', b'd', 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 16, b't',
                b'f', b'd', b't', 1, 0, 0, 0, 0, 0, 0, 2,
            ],
        );
        let mut rebaser = FragmentTimelineRebaser::default();
        let normalized = normalize_for_test(&mut rebaser, decoy.clone()).unwrap();

        let mdat_start = decoy
            .windows(4)
            .position(|window| window == b"mdat")
            .expect("test fragment must contain mdat")
            - 4;
        assert_eq!(&normalized[mdat_start..], &decoy[mdat_start..]);
        assert_eq!(
            parse_fragment_timeline(&normalized).unwrap().metadata,
            FragmentTimelineMetadata {
                sequence_number: 1,
                decode_times: BTreeMap::from([(1, 0)]),
                sidx: None,
            }
        );
    }

    #[test]
    fn rejects_malformed_and_underflowing_suffix_metadata() {
        let mut malformed_rebaser = FragmentTimelineRebaser::default();
        let malformed = mp4_box(b"mdat", b"not a moof", false);
        let error = normalize_for_test(&mut malformed_rebaser, malformed).unwrap_err();
        assert!(error.message.contains("invalid ISO-BMFF timeline metadata"));

        let mut underflow_rebaser = FragmentTimelineRebaser::default();
        let selected = test_fragment(20, &[(1, 10_000)], false, b"selected");
        let earlier = test_fragment(21, &[(1, 9_999)], false, b"earlier");
        normalize_for_test(&mut underflow_rebaser, selected).unwrap();
        let error = normalize_for_test(&mut underflow_rebaser, earlier).unwrap_err();
        assert!(error.message.contains("decode times are not monotonic"));

        let mut inconsistent_rebaser = FragmentTimelineRebaser::default();
        let selected = test_fragment(30, &[(1, 10_000)], false, b"selected");
        let changed_tracks = test_fragment_with_tracks(
            31,
            &[(1, 1, 10_500), (1, 1, 10_500)],
            false,
            None,
            b"duplicate track IDs",
        );
        normalize_for_test(&mut inconsistent_rebaser, selected).unwrap();
        let error = normalize_for_test(&mut inconsistent_rebaser, changed_tracks).unwrap_err();
        assert!(error.message.contains("duplicate tfhd track IDs"));
    }

    #[test]
    fn normalized_nonzero_suffix_stays_in_order_and_preserves_deltas() {
        let mut rebaser = FragmentTimelineRebaser::default();
        let fragments = [
            test_fragment(90, &[(1, 500_000)], false, b"six"),
            test_fragment(91, &[(1, 500_480)], false, b"seven"),
            test_fragment(92, &[(1, 501_120)], false, b"eight"),
        ];
        let normalized = fragments
            .iter()
            .map(|fragment| normalize_for_test(&mut rebaser, fragment.to_vec()).unwrap())
            .collect::<Vec<_>>();
        let metadata = normalized
            .iter()
            .map(|fragment| parse_fragment_timeline(fragment).unwrap().metadata)
            .collect::<Vec<_>>();

        assert_eq!(
            metadata
                .iter()
                .map(|fragment| fragment.sequence_number)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            metadata
                .iter()
                .map(|fragment| fragment.decode_times[&1])
                .collect::<Vec<_>>(),
            vec![0, 480, 1_120]
        );
    }

    #[test]
    fn compares_reordered_tracks_by_track_id() {
        let selected =
            test_fragment_with_tracks(10, &[(10, 1, 100), (20, 1, 200)], false, None, b"selected");
        let later =
            test_fragment_with_tracks(11, &[(20, 1, 250), (10, 1, 150)], false, None, b"later");
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, selected).unwrap();
        let later = normalize_for_test(&mut rebaser, later).unwrap();
        let metadata = parse_fragment_timeline(&later).unwrap().metadata;

        assert_eq!(metadata.decode_times, BTreeMap::from([(10, 50), (20, 50)]));
        assert_eq!(
            parse_fragment_timeline(&later)
                .unwrap()
                .decode_times
                .iter()
                .map(|field| field.value)
                .collect::<Vec<_>>(),
            vec![50, 50]
        );
    }

    #[test]
    fn rejects_duplicate_missing_and_changed_track_ids() {
        let selected =
            test_fragment_with_tracks(20, &[(10, 1, 100), (20, 1, 200)], false, None, b"selected");
        let changed =
            test_fragment_with_tracks(21, &[(10, 1, 150), (30, 1, 250)], false, None, b"changed");
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, selected).unwrap();
        let error = normalize_for_test(&mut rebaser, changed).unwrap_err();
        assert!(error.message.contains("track IDs changed"));

        let duplicate =
            test_fragment_with_tracks(22, &[(10, 1, 150), (10, 1, 250)], false, None, b"duplicate");
        let error = normalize_for_test(&mut rebaser, duplicate).unwrap_err();
        assert!(error.message.contains("duplicate tfhd track IDs"));

        let missing = test_fragment_without_tfhd(23, 300);
        let error = normalize_for_test(&mut rebaser, missing).unwrap_err();
        assert!(error.message.contains("traf does not contain a tfhd"));
    }

    #[test]
    fn rejects_duplicate_sequence_numbers_after_the_selected_fragment() {
        let selected = test_fragment(30, &[(1, 100)], false, b"selected");
        let duplicate = test_fragment(30, &[(1, 200)], false, b"duplicate");
        let mut rebaser = FragmentTimelineRebaser::default();
        normalize_for_test(&mut rebaser, selected).unwrap();
        let error = normalize_for_test(&mut rebaser, duplicate).unwrap_err();
        assert!(error.message.contains("not strictly increasing"));
    }

    #[test]
    fn requires_a_nonempty_structurally_bounded_top_level_mdat() {
        let missing = test_fragment(40, &[(1, 100)], false, b"payload");
        let mdat_start = missing
            .windows(4)
            .position(|window| window == b"mdat")
            .expect("test fragment must contain mdat")
            - 4;
        let missing = missing[..mdat_start].to_vec();
        let mut rebaser = FragmentTimelineRebaser::default();
        let error = normalize_for_test(&mut rebaser, missing).unwrap_err();
        assert!(error.message.contains("nonempty top-level mdat"));

        let empty = test_fragment(41, &[(1, 100)], false, b"");
        let error = normalize_for_test(&mut rebaser, empty).unwrap_err();
        assert!(error.message.contains("nonempty top-level mdat"));

        let mut truncated = test_fragment(42, &[(1, 100)], false, b"payload");
        let mdat_start = truncated
            .windows(4)
            .position(|window| window == b"mdat")
            .expect("test fragment must contain mdat")
            - 4;
        let claimed_size = u32::try_from(truncated.len() - mdat_start + 1)
            .expect("test fragment must fit a 32-bit size");
        truncated[mdat_start..mdat_start + 4].copy_from_slice(&claimed_size.to_be_bytes());
        let error = normalize_for_test(&mut rebaser, truncated).unwrap_err();
        assert!(error.message.contains("extends past its parent"));
    }

    #[test]
    fn normalize_honors_cancellation_before_parsing() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut rebaser = FragmentTimelineRebaser::default();
        let error = rebaser
            .normalize(
                test_fragment(50, &[(1, 100)], false, b"payload"),
                &cancellation,
            )
            .unwrap_err();
        assert_eq!(error.message, "Playback request cancelled");
    }

    #[test]
    fn fetch_window_counts_pending_and_ready_segments() {
        assert_eq!(fetch_window_slots(0, 0), MEDIA_CONCURRENCY);
        assert_eq!(fetch_window_slots(2, 1), 0);
        assert_eq!(fetch_window_slots(0, 1), MEDIA_CONCURRENCY - 1);
        assert_eq!(fetch_window_slots(usize::MAX, 1), 0);
    }

    #[test]
    fn rejects_master_playlists_and_encrypted_media() {
        for body in [
            "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nchild.m3u8\n",
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-KEY:METHOD=AES-128\n#EXT-X-ENDLIST\n",
        ] {
            assert!(parse_manifest(&base_url(), body).is_err());
        }
    }

    #[test]
    fn requires_vod_endlist_init_map_and_segments() {
        for body in [
            "#EXTM3U\n#EXT-X-ENDLIST\n",
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-ENDLIST\n",
            "#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:1,\nsegment.m4s\n",
        ] {
            assert!(parse_manifest(&base_url(), body).is_err());
        }
    }

    #[test]
    fn rejects_non_soundcloud_playback_hosts() {
        let invalid = Url::parse("https://evil.example.test/playlist.m3u8").unwrap();
        assert!(validate_playback_url(&invalid).is_err());
        let body =
            valid_manifest().replace("segment001.m4s", "https://evil.example.test/segment.m4s");
        assert!(parse_manifest(&base_url(), &body).is_err());
    }

    #[test]
    fn rejects_oversized_manifests_and_segment_lists() {
        let oversized = format!("#EXTM3U\n{}", "x".repeat(MAX_MANIFEST_BYTES));
        assert!(parse_manifest(&base_url(), &oversized).is_err());

        let mut too_many_segments =
            String::from("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-MAP:URI=\"init.mp4\"\n");
        for index in 0..=MAX_SEGMENTS {
            too_many_segments.push_str(&format!("#EXTINF:1,\nsegment{index}.m4s\n"));
        }
        too_many_segments.push_str("#EXT-X-ENDLIST\n");
        assert!(parse_manifest(&base_url(), &too_many_segments).is_err());
    }

    #[tokio::test]
    async fn download_checks_cancellation_before_fetching_media() {
        let descriptor = HlsDescriptor {
            manifest_url: base_url(),
            init_url: Url::parse(
                "https://playback.media-streaming.soundcloud.cloud/path/init.mp4?signature=redacted",
            )
            .unwrap(),
            segments: vec![Url::parse(
                "https://playback.media-streaming.soundcloud.cloud/path/segment.m4s?signature=redacted",
            )
            .unwrap()],
            segment_durations: vec![Duration::from_secs(1)],
            segment_starts: vec![Duration::ZERO],
            total_duration: Some(Duration::from_secs(1)),
            init_fragment: Arc::new(OnceLock::new()),
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut output = tokio::io::sink();
        let result = download(
            &Client::new(),
            &descriptor,
            &mut output,
            &cancellation,
            None,
            1024,
            "test-agent",
        )
        .await;
        assert_eq!(
            result.expect_err("cancelled download must fail").message,
            "Playback request cancelled"
        );
    }
}
