use std::{sync::Arc, sync::mpsc, time::Duration};

use futures::future::BoxFuture;
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use super::super::progressive::{
    ProgressiveFile, ProgressiveReader, ProgressiveWriter, TimelineSeekRequest,
    TimelineSeekSession, TimelineSeekStartup,
};
use super::AudioFormat;

/// Enough suffix bytes for the decoder probe and a moment of playback.
const SUFFIX_STARTUP_BYTES: u64 = 64 * 1024;
/// Range requests stay small so the first one returns quickly.
const SUFFIX_CHUNK: u64 = 256 * 1024;
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
/// discard so the engine lands on the target. Each request fetches into a
/// fresh progressive buffer, which lets the engine land the seek while the
/// download is still running instead of queueing it until the buffer
/// completes.
pub(crate) struct RangeTimelineSession {
    format: RangeSeekFormat,
    fetch: RangeFetch,
    total: u64,
    duration: Duration,
    runtime: Handle,
    track_cancellation: CancellationToken,
}

impl RangeTimelineSession {
    pub(super) fn new(
        format: RangeSeekFormat,
        fetch: RangeFetch,
        total: u64,
        duration: Duration,
        runtime: Handle,
        track_cancellation: CancellationToken,
    ) -> Self {
        Self {
            format,
            fetch,
            total,
            duration,
            runtime,
            track_cancellation,
        }
    }
}

impl TimelineSeekSession for RangeTimelineSession {
    fn request(&self, position: Duration) -> Result<TimelineSeekRequest, String> {
        let seconds = self.duration.as_secs_f64();
        if !(seconds > 0.0) {
            return Err("The track duration is unknown".into());
        }
        if self.total == 0 {
            return Err("The track size is unknown".into());
        }
        let format = self.format;
        let fraction = (position.as_secs_f64() / seconds).clamp(0.0, 0.99);
        let start = (fraction * self.total as f64) as u64;
        let buffer = ProgressiveFile::new(format.audio(), None)
            .map_err(|_| "A temporary seek buffer could not be created".to_string())?;
        let reader = buffer
            .reader()
            .map_err(|_| "The seek buffer could not be opened".to_string())?;
        let writer = buffer
            .writer()
            .map_err(|_| "The seek buffer could not be opened".to_string())?;
        let file = buffer.into_file();
        let cancellation = self.track_cancellation.child_token();
        let worker_cancellation = cancellation.clone();
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let fetch = self.fetch.clone();
        let total = self.total;
        let duration = self.duration;
        self.runtime.spawn(async move {
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
        let (header_len, frame) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                self.writer.fail(error.clone());
                return Err(error);
            }
        };
        self.written = header_len;
        self.intra_segment_offset = Some(target.saturating_sub(frame.position));
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

        while start < self.total {
            if self.cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            let end = (start + SUFFIX_CHUNK - 1).min(self.total - 1);
            let bytes = (self.fetch)(start, end, self.cancellation.clone())
                .await
                .map_err(|error| format!("The seek suffix could not be downloaded: {error}"))?;
            self.writer
                .write_all(&bytes)
                .await
                .map_err(|_| "The seek buffer could not be written".to_string())?;
            self.writer
                .flush()
                .await
                .map_err(|_| "The seek buffer could not be finalized".to_string())?;
            self.written = self.written.saturating_add(bytes.len() as u64);
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
        Ok(())
    }

    fn release_startup(&mut self) -> Result<(), String> {
        let Some((reader, file)) = self.startup.take() else {
            return Ok(());
        };
        self.writer.mark_startup_ready();
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
) -> Result<(u64, FlacFrame), String> {
    use tokio::io::AsyncWriteExt as _;

    let (header, stream_info) = flac_file_header(fetch.clone(), total, cancellation).await?;
    let frame = flac_frame_near(
        fetch,
        total,
        duration,
        &stream_info,
        target,
        start,
        cancellation,
    )
    .await?;
    writer
        .write_all(&header)
        .await
        .map_err(|_| "The seek buffer could not be written".to_string())?;
    writer
        .flush()
        .await
        .map_err(|_| "The seek buffer could not be finalized".to_string())?;
    Ok((header.len() as u64, frame))
}

/// Fetches the `fLaC` marker and metadata blocks from the start of the file.
/// A mid-file fragment has no stream header of its own, so the decoder needs
/// them in front of the suffix frames.
async fn flac_file_header(
    fetch: RangeFetch,
    total: u64,
    cancellation: &CancellationToken,
) -> Result<(Vec<u8>, FlacStreamInfo), String> {
    let mut probe_end = FLAC_HEADER_PROBE_BYTES.min(total);
    loop {
        let bytes = fetch(0, probe_end - 1, cancellation.clone())
            .await
            .map_err(|error| format!("The FLAC seek header could not be downloaded: {error}"))?;
        match parse_flac_header(&bytes) {
            Ok(header) => return Ok(header),
            Err(FlacHeaderError::Incomplete) => {
                let next = probe_end.saturating_mul(8).min(total);
                if next <= probe_end || probe_end >= FLAC_HEADER_PROBE_LIMIT {
                    return Err("The FLAC seek header was oversized".into());
                }
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
    if !(seconds > 0.0) {
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
    if !(span > 0.0) {
        return lower.offset;
    }
    let aim = target.saturating_sub(FLAC_AIM_AHEAD);
    let into_span = aim.saturating_sub(lower.position).as_secs_f64().min(span);
    let fraction = (into_span / span).clamp(0.0, 1.0);
    let span_bytes = upper.offset.saturating_sub(lower.offset) as f64;
    lower.offset.saturating_add((fraction * span_bytes) as u64)
}

/// Stream parameters read from the FLAC STREAMINFO metadata block.
struct FlacStreamInfo {
    sample_rate: u32,
    channels: u32,
    bits_per_sample: u32,
    block_len_min: u64,
    block_len_max: u64,
}

impl FlacStreamInfo {
    /// Upper bound on one frame's byte size, used to size probe requests.
    fn max_frame_bytes(&self) -> u64 {
        self.block_len_max
            .saturating_mul(u64::from(self.channels))
            .saturating_mul(u64::from(self.bits_per_sample.div_ceil(8)))
            .saturating_add(64)
    }
}

#[derive(Clone, Copy)]
struct FlacFrame {
    offset: u64,
    position: Duration,
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
        }
        cursor = payload_end;
        if last {
            let info = info.expect("STREAMINFO is the first metadata block");
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
    }
}

/// Finds the first frame header in a probe buffer and reports where the frame
/// starts and when it plays. False sync matches inside frame data are weeded
/// out with the same checks symphonia applies: reserved fields, the header
/// CRC-8, and consistency with the stream parameters.
fn parse_first_flac_frame(bytes: &[u8], info: &FlacStreamInfo, base: u64) -> Option<FlacFrame> {
    let mut index = 0;
    while index + 2 <= bytes.len() {
        if bytes[index] == 0xff && (bytes[index + 1] & 0xfe) == 0xf8 {
            if let Some(frame) = parse_flac_frame_header(bytes, index, info, base) {
                return Some(frame);
            }
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
    let sync = u16::from_be_bytes([bytes[index], bytes[index + 1]]);
    let variable_blocks = sync & 0x01 != 0;
    let block_size_code = u32::from(bytes[index + 2] >> 4);
    let sample_rate_code = u32::from(bytes[index + 2] & 0x0f);
    let channel_code = u32::from(bytes[index + 3] >> 4);
    let sample_size_code = u32::from((bytes[index + 3] >> 1) & 0x07);
    if bytes[index + 3] & 0x01 != 0
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

    fn memory_fetch(bytes: Arc<Vec<u8>>) -> (RangeFetch, Arc<Mutex<Vec<(u64, u64)>>>) {
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
                .find(|(start, end)| end - start + 1 == SUFFIX_CHUNK)
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
            AudioEngine, RodioEngine, RodioEngine as Engine, SeekOutcome,
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
        let (fetch, _ranges) = memory_fetch(Arc::new(bytes));
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Mp3,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
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

        let Ok(mut engine) = RodioEngine::new() else {
            eprintln!("no audio device; skipping mid download seek test");
            return;
        };
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());
        engine.load(prepared, 1.0);

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
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mid_download_flac_seek_applies_during_the_download() {
        use super::super::super::engine::{
            AudioEngine, RodioEngine, RodioEngine as Engine, SeekOutcome,
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
        let (fetch, _ranges) = memory_fetch(Arc::new(bytes));
        let session = Arc::new(RangeTimelineSession::new(
            RangeSeekFormat::Flac,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            CancellationToken::new(),
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

        let Ok(mut engine) = RodioEngine::new() else {
            eprintln!("no audio device; skipping mid download FLAC seek test");
            return;
        };
        let prepared = PreparedSource::new(source, Some(duration), file)
            .with_progressive_seek(progressive_seek.unwrap());
        engine.load(prepared, 1.0);

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
            AudioEngine, RodioEngine, RodioEngine as Engine, SeekOutcome,
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

        let Ok(mut engine) = RodioEngine::new() else {
            eprintln!("no audio device; skipping seek spam test");
            return;
        };
        engine.load(prepared, 1.0);

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

        // Every superseded fetch must have observed its cancellation and
        // no fetch may outlive its request.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let log = log.lock().unwrap();
        assert!(
            log.cancelled >= 4,
            "the superseded rapid seeks must cancel their fetches, log was {log:?}"
        );
        assert_eq!(
            log.started,
            log.cancelled + log.completed,
            "every started fetch must end cancelled or completed, log was {log:?}"
        );
    }
}
