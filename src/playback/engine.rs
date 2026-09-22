use std::{
    fs::File,
    io::{BufReader, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use futures::channel::oneshot;
use ogg::reading::PacketReader;
use opus_decoder::OpusDecoder;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{
    Decoder, OutputStream, OutputStreamBuilder, Sink, Source, buffer::SamplesBuffer,
    source::SeekError,
};
use tokio_util::sync::CancellationToken;

use crate::diagnostics;

use super::asio_drivers::find_asio_driver;
use super::output_devices::find_output_device;
use super::progressive::{
    ProgressiveCompletion, ProgressiveReader, TimelineSeekSession, TimelineSuffixState,
};
use super::ramped_gain::RampedGain;
use super::resolver::{AudioFormat, ResolvedAudio, ResolvedProgressiveAudio};
use super::standby::{PreparedSource, ProgressiveSeek, SinkProbe};

pub(crate) type DecodedSource = Box<dyn Source + Send>;

struct PendingProgressiveReload {
    position: Duration,
    cancellation: Arc<AtomicBool>,
    timeline_cancellation: Option<CancellationToken>,
    receiver: mpsc::Receiver<Result<(DecodedSource, Option<tempfile::NamedTempFile>), String>>,
}

struct ProgressiveReloadResult {
    source: DecodedSource,
    position: Duration,
    file: Option<tempfile::NamedTempFile>,
    timeline_cancellation: Option<CancellationToken>,
}

pub(crate) struct SeekCompletion {
    receiver: oneshot::Receiver<()>,
}

impl SeekCompletion {
    pub(crate) async fn wait(self) {
        let _ = self.receiver.await;
    }
}

pub(crate) trait AudioEngine {
    fn load(&mut self, prepared: PreparedSource, volume: f32, playing: bool) -> Option<Duration>;
    fn play(&self);
    fn pause(&self);
    fn stop(&mut self);
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, String>;
    fn seek_for_output_restore(&mut self, position: Duration) -> Result<SeekOutcome, String> {
        self.seek(position)
    }
    fn take_seek_completion(&mut self) -> Option<SeekCompletion> {
        None
    }
    fn set_playback_intent(&self, _playing: bool) {}
    fn apply_deferred_seek(&mut self) -> Result<SeekOutcome, String> {
        Ok(SeekOutcome::Deferred)
    }

    /// Live state of the suffix a timeline seek landed on or is fetching,
    /// so the buffering indicator can track what will actually play. None
    /// when no timeline suffix drives playback.
    fn timeline_suffix_state(&self) -> Option<TimelineSuffixState> {
        None
    }
    fn set_transport_gain_target(&self, target: f32);
    fn reset_transport_gain(&self, gain: f32);
    fn transport_gain_settled(&self, target: f32) -> bool;
    fn set_automation_gain_target(&self, target: f32, duration: Duration);
    fn reset_automation_gain(&self, gain: f32);
    fn automation_gain_settled(&self, target: f32) -> bool;
    fn set_volume(&self, volume: f32);
    fn position(&self) -> Duration;
    fn ended(&self) -> bool;
    fn set_output(&mut self, prepared: PreparedOutputSwitch) -> OutputSwitch;
    fn output_target(&self) -> &AudioOutputTarget;
    fn append_standby(&mut self, prepared: PreparedSource);
    fn skip_to_standby(&mut self);
    fn activate_standby(&mut self);
    fn sink_probe(&self) -> SinkProbe;
    fn owns_probe(&self, probe: &SinkProbe) -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SeekOutcome {
    Applied,
    AppliedStandbyDropped,
    Deferred,
}

/// Output device the engine plays through. Device names are the endpoint
/// names the default host reports, which stay stable on Windows. ASIO
/// driver names come from the registry's ASIO key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AudioOutputTarget {
    SystemDefault,
    Device(String),
    AsioDriver(String),
}

/// Outcome of switching the engine to another output target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputSwitch {
    /// The front source was re-created at the same position, or nothing
    /// was playing to begin with.
    SourceRestored,
    /// The stream moved, but the front buffer could not be re-created, for
    /// example while its download is still in flight.
    SourceLost,
}

#[derive(Clone)]
pub(crate) struct OutputReloadSpec {
    path: PathBuf,
    format: AudioFormat,
    position: Duration,
}

pub(crate) struct PreparedOutputSwitch {
    stream: OutputStream,
    source: Option<DecodedSource>,
    position: Duration,
    target: AudioOutputTarget,
}

pub(crate) struct OpenOutputSwitch {
    stream: OutputStream,
    position: Duration,
    target: AudioOutputTarget,
}

impl OpenOutputSwitch {
    pub(crate) fn finish(self, source: Option<DecodedSource>) -> PreparedOutputSwitch {
        PreparedOutputSwitch {
            stream: self.stream,
            source,
            position: self.position,
            target: self.target,
        }
    }
}

impl PreparedOutputSwitch {
    pub(crate) fn position(&self) -> Duration {
        self.position
    }
}

pub(crate) struct RodioEngine {
    stream: OutputStream,
    sink: Arc<Sink>,
    retained_files: Vec<tempfile::NamedTempFile>,
    progressive_seek: Option<ProgressiveSeek>,
    standby_progressive_seek: Option<ProgressiveSeek>,
    front_reopen: Option<(PathBuf, AudioFormat)>,
    standby_reopen: Option<(PathBuf, AudioFormat)>,
    position_base: Duration,
    pending_progressive_reload: Option<PendingProgressiveReload>,
    pending_seek_completion: Option<SeekCompletion>,
    pending_position: Option<Duration>,
    active_timeline_cancellation: Option<CancellationToken>,
    /// Timeline position of the landed suffix the sink is playing, when a
    /// timeline seek superseded the front buffer.
    active_suffix: Option<Duration>,
    playback_intent: Arc<AtomicBool>,
    transport_gain: Arc<RampedGain>,
    automation_gain: Arc<RampedGain>,
    output_target: AudioOutputTarget,
}

impl RodioEngine {
    pub(crate) fn new(output_target: AudioOutputTarget) -> Result<Self, String> {
        let stream = Self::open_output_stream(&output_target)?;
        let sink = Arc::new(Sink::connect_new(stream.mixer()));
        Ok(Self {
            stream,
            sink,
            retained_files: Vec::new(),
            progressive_seek: None,
            standby_progressive_seek: None,
            front_reopen: None,
            standby_reopen: None,
            position_base: Duration::ZERO,
            pending_progressive_reload: None,
            pending_seek_completion: None,
            pending_position: None,
            active_timeline_cancellation: None,
            active_suffix: None,
            playback_intent: Arc::new(AtomicBool::new(false)),
            transport_gain: Arc::new(RampedGain::default()),
            automation_gain: Arc::new(RampedGain::default()),
            output_target,
        })
    }

    fn open_output_stream(target: &AudioOutputTarget) -> Result<OutputStream, String> {
        match target {
            AudioOutputTarget::SystemDefault => OutputStreamBuilder::open_default_stream()
                .map_err(|_| "No usable audio output device was found".to_string()),
            AudioOutputTarget::Device(name) => {
                let device = find_output_device(name).ok_or_else(|| {
                    format!("The audio output device \"{name}\" is not available")
                })?;
                OutputStreamBuilder::from_device(device)
                    .and_then(|builder| {
                        builder
                            .with_error_callback(log_output_stream_error)
                            .open_stream_or_fallback()
                    })
                    .map_err(|_| format!("The audio output device \"{name}\" could not be opened"))
            }
            AudioOutputTarget::AsioDriver(name) => Self::open_asio_output_stream(name),
        }
    }

    /// Opens a stream through the named ASIO driver, logging the driver's
    /// default output config once it is live. The lookup loads and
    /// initializes drivers, and ASIO keeps a single driver loaded per
    /// process, so a different live ASIO stream makes the lookup fail; the
    /// caller then keeps the previous stream.
    fn open_asio_output_stream(name: &str) -> Result<OutputStream, String> {
        let device = find_asio_driver(name)?;
        let config = device.default_output_config().ok();
        let stream = OutputStreamBuilder::from_device(device)
            .and_then(|builder| {
                builder
                    .with_error_callback(log_output_stream_error)
                    .open_stream_or_fallback()
            })
            .map_err(|error| format!("The ASIO driver \"{name}\" could not be opened: {error}"))?;
        if let Some(config) = config {
            diagnostics::event(
                "INFO",
                format!(
                    "ASIO driver \"{name}\" opened at {} Hz, {} channels, sample format {}",
                    config.sample_rate().0,
                    config.channels(),
                    config.sample_format(),
                ),
            );
        }
        Ok(stream)
    }

    /// A complete file can be decoded for a device switch without changing
    /// the ordinary seek path used by a standby track.
    pub(crate) fn output_reload_spec(&self) -> Option<OutputReloadSpec> {
        let (path, format) = match self.progressive_seek.as_ref() {
            Some(seek) if seek.completion.is_complete() => (seek.path.clone(), seek.format),
            Some(_) => return None,
            None => self.front_reopen.clone()?,
        };
        Some(OutputReloadSpec {
            path,
            format,
            position: self.reported_position(),
        })
    }

    fn reload_front_source(spec: &OutputReloadSpec) -> Option<DecodedSource> {
        if spec.format == AudioFormat::OggOpus {
            // rodio cannot decode Ogg Opus, so the hand decoder rebuilds
            // the buffer and discards samples up to the position.
            let mut samples = Self::opus(&spec.path).ok()?;
            discard_decoder_samples(&mut samples, spec.position, None).ok()?;
            return Some(Box::new(samples));
        }
        Self::decoder_at(&spec.path, spec.format, spec.position).ok()
    }

    pub(crate) fn decode_output_reload(reload: Option<OutputReloadSpec>) -> Option<DecodedSource> {
        reload.as_ref().and_then(Self::reload_front_source)
    }

    /// ASIO initialization and teardown stay on the UI thread for drivers
    /// that require the same thread for both operations. The decoder is
    /// prepared separately on a worker while this stream stays on the UI.
    pub(crate) fn open_asio_output_switch(
        target: AudioOutputTarget,
        reload: Option<&OutputReloadSpec>,
    ) -> Result<OpenOutputSwitch, String> {
        let AudioOutputTarget::AsioDriver(name) = &target else {
            return Err("The selected output is not an ASIO driver".into());
        };
        let stream = Self::open_asio_output_stream(name)?;
        Ok(OpenOutputSwitch {
            stream,
            position: reload.map_or(Duration::ZERO, |spec| spec.position),
            target,
        })
    }

    /// WASAPI stream opening and decoder positioning can block, so callers
    /// prepare both on a background worker.
    pub(crate) fn prepare_output_switch(
        target: AudioOutputTarget,
        reload: Option<OutputReloadSpec>,
    ) -> Result<PreparedOutputSwitch, String> {
        let (stream, target) = match Self::open_output_stream(&target) {
            Ok(stream) => (stream, target),
            Err(error) if matches!(target, AudioOutputTarget::Device(_)) => {
                diagnostics::event(
                    "WARN",
                    format!(
                        "the selected output could not be opened, using the system default: {error}"
                    ),
                );
                (
                    Self::open_output_stream(&AudioOutputTarget::SystemDefault)?,
                    AudioOutputTarget::SystemDefault,
                )
            }
            Err(error) => return Err(error),
        };
        let position = reload.as_ref().map_or(Duration::ZERO, |spec| spec.position);
        let source = Self::decode_output_reload(reload);
        Ok(PreparedOutputSwitch {
            stream,
            source,
            position,
            target,
        })
    }

    /// Move through an unrelated WASAPI endpoint before opening another
    /// ASIO stream. Some interfaces cannot open ASIO while their own WASAPI
    /// endpoint is still active, and two ASIO drivers cannot coexist.
    pub(crate) fn prepare_asio_bridge(
        driver_name: &str,
        current_device: Option<&str>,
        reload: Option<OutputReloadSpec>,
    ) -> Result<PreparedOutputSwitch, String> {
        let host = rodio::cpal::default_host();
        let devices = host
            .output_devices()
            .map_err(|error| format!("An ASIO bridge output could not be listed: {error}"))?;
        for device in devices {
            let Ok(name) = device.name() else {
                continue;
            };
            if current_device == Some(name.as_str())
                || asio_endpoint_matches_driver(&name, driver_name)
            {
                continue;
            }
            let stream = OutputStreamBuilder::from_device(device).and_then(|builder| {
                builder
                    .with_error_callback(log_output_stream_error)
                    .open_stream_or_fallback()
            });
            if let Ok(stream) = stream {
                let position = reload.as_ref().map_or(Duration::ZERO, |spec| spec.position);
                let source = reload.as_ref().and_then(Self::reload_front_source);
                return Ok(PreparedOutputSwitch {
                    stream,
                    source,
                    position,
                    target: AudioOutputTarget::Device(name),
                });
            }
        }
        if current_device.is_none() {
            return Self::prepare_output_switch(AudioOutputTarget::SystemDefault, reload);
        }
        Err("No independent audio output is available to release the current device before opening ASIO".into())
    }

    fn wrap_source(&self, source: DecodedSource) -> DecodedSource {
        Box::new(crate::plugins::minimeters::tap::wrap(
            self.automation_gain.wrap(self.transport_gain.wrap(source)),
        ))
    }

    fn decoder(path: &Path, format: AudioFormat) -> Result<Decoder<BufReader<File>>, String> {
        let file = File::open(path)
            .map_err(|error| format!("The playback buffer could not be opened: {error}"))?;
        let byte_len = file
            .metadata()
            .map_err(|error| format!("The playback buffer could not be inspected: {error}"))?
            .len();
        Decoder::builder()
            .with_data(BufReader::with_capacity(64 * 1024, file))
            .with_byte_len(byte_len)
            .with_hint(format.extension())
            .with_mime_type(format.mime_type())
            .build()
            .map_err(|error| format!("Could not decode {} audio: {error}", format.label()))
    }

    fn decoder_at(
        path: &Path,
        format: AudioFormat,
        position: Duration,
    ) -> Result<DecodedSource, String> {
        let mut decoder = Self::decoder(path, format)?;
        if format == AudioFormat::M4a {
            discard_decoder_samples(&mut decoder, position, None)?;
        } else {
            decoder
                .try_seek(position)
                .map_err(|error| format!("This stream could not seek to that position: {error}"))?;
        }
        Ok(Box::new(decoder))
    }

    fn decoder_at_with_cancellation(
        path: &Path,
        format: AudioFormat,
        position: Duration,
        cancellation: &AtomicBool,
    ) -> Result<DecodedSource, String> {
        let mut decoder = Self::decoder(path, format)?;
        if format == AudioFormat::M4a {
            discard_decoder_samples(&mut decoder, position, Some(cancellation))?;
        } else {
            decoder
                .try_seek(position)
                .map_err(|error| format!("This stream could not seek to that position: {error}"))?;
        }
        Ok(Box::new(decoder))
    }

    fn progressive_decoder_at_with_cancellation(
        path: &Path,
        format: AudioFormat,
        position: Duration,
        completion: &ProgressiveCompletion,
        cancellation: &AtomicBool,
    ) -> Result<DecodedSource, String> {
        if cancellation.load(Ordering::Acquire) {
            return Err("Playback request cancelled".into());
        }
        let reader = completion
            .open_reader(path)
            .map_err(|error| format!("The playback buffer could not be opened: {error}"))?;
        let total = reader.total();
        // The local seek's binary search must stay inside the flushed frontier.
        // Telling symphonia the full file size makes its seek probes read past
        // the frontier, where the progressive reader blocks on its wait and
        // races the ongoing front download with fallback range fetches.
        let available = completion.written();
        let mut builder = Decoder::builder()
            .with_data(reader)
            .with_hint(format.extension())
            .with_mime_type(format.mime_type());
        let byte_len = total.map(|total| total.min(available));
        if let Some(byte_len) = byte_len {
            builder = builder.with_byte_len(byte_len);
        }
        let mut decoder = builder
            .build()
            .map_err(|error| format!("Could not decode {} audio: {error}", format.label()))?;
        if cancellation.load(Ordering::Acquire) {
            return Err("Playback request cancelled".into());
        }
        decoder
            .try_seek(position)
            .map_err(|error| format!("This stream could not seek to that position: {error}"))?;
        Ok(Box::new(decoder))
    }

    fn opus(path: &Path) -> Result<SamplesBuffer, String> {
        let file = File::open(path)
            .map_err(|error| format!("The playback buffer could not be opened: {error}"))?;
        let mut packets = PacketReader::new(BufReader::new(file));
        let head = packets
            .read_packet_expected()
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        if head.data.len() < 19 || &head.data[..8] != b"OpusHead" {
            return Err("Could not decode Ogg Opus audio: invalid OpusHead".into());
        }
        let channels = usize::from(head.data[9]);
        let pre_skip = usize::from(u16::from_le_bytes([head.data[10], head.data[11]]));
        let gain = i16::from_le_bytes([head.data[16], head.data[17]]);
        if gain != 0 {
            return Err("Could not decode Ogg Opus audio: output gain is unsupported".into());
        }
        let tags = packets
            .read_packet_expected()
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        if !tags.data.starts_with(b"OpusTags") {
            return Err("Could not decode Ogg Opus audio: invalid OpusTags".into());
        }
        let mut decoder = OpusDecoder::new(48_000, channels)
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        let mut samples = Vec::with_capacity(120 * 48_000 * channels);
        let mut decoded = vec![0.0; OpusDecoder::MAX_FRAME_SIZE_48K * channels];
        let mut decoded_frames = 0;
        while let Some(packet) = packets
            .read_packet()
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?
        {
            let frames = decoder
                .decode_float(&packet.data, &mut decoded, false)
                .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
            let start_frames = pre_skip.saturating_sub(decoded_frames).min(frames);
            decoded_frames += frames;
            samples.extend_from_slice(&decoded[start_frames * channels..frames * channels]);
        }
        Ok(SamplesBuffer::new(channels as u16, 48_000, samples))
    }

    /// Decodes fully resolved audio into a source that can be appended to a
    /// sink later without further work. The standby path runs this on a
    /// blocking worker while the current track keeps playing.
    pub(crate) fn decode(audio: ResolvedAudio) -> Result<PreparedSource, String> {
        let path = audio.path.clone();
        let format = audio.format;
        if audio.format == AudioFormat::OggOpus {
            let decoded = Self::opus(&audio.path)?;
            let duration = decoded.total_duration().or(audio.duration);
            return Ok(
                PreparedSource::new(decoded, duration, audio.file).with_output_reopen(path, format)
            );
        }
        let decoded = Self::decoder(&audio.path, audio.format)?;
        let duration = decoded.total_duration().or(audio.duration);
        // A resolved M4A buffer is a fragmented MP4 concatenation whose
        // decoder reports a zero duration, so a sink seek clamps the target
        // to zero and silently restarts the track. The download already
        // finished before the source was prepared, so arming it with a
        // completed-buffer reload keeps seeks on an auto-advanced track on
        // the same discard path a finished progressive download uses.
        if audio.format == AudioFormat::M4a {
            return Ok(PreparedSource::new(decoded, duration, audio.file)
                .with_output_reopen(path.clone(), format)
                .with_progressive_seek(ProgressiveSeek {
                    path,
                    format: audio.format,
                    completion: ProgressiveCompletion::for_completed_buffer(audio.format),
                    timeline_seek_session: None,
                }));
        }
        Ok(PreparedSource::new(decoded, duration, audio.file).with_output_reopen(path, format))
    }

    pub(crate) fn decode_progressive(
        audio: ResolvedProgressiveAudio,
    ) -> Result<PreparedSource, String> {
        if audio.format == AudioFormat::OggOpus {
            let decoded = ProgressiveOpus::new(audio.reader, audio.duration)?;
            let duration = decoded.total_duration().or(audio.duration);
            return Ok(PreparedSource::new(decoded, duration, audio.file));
        }
        let path = audio.file.path().to_owned();
        let completion = audio.reader.completion();
        let mut builder = Decoder::builder()
            .with_data(audio.reader)
            .with_hint(audio.format.extension())
            .with_mime_type(audio.format.mime_type());
        if let Some(total) = audio.total {
            builder = builder.with_byte_len(total);
        }
        let decoded = builder
            .build()
            .map_err(|error| format!("Could not decode {} audio: {error}", audio.format.label()))?;
        let duration = decoded.total_duration().or(audio.duration);
        // Every source reaching this point decodes through the shared
        // progressive reader, which blocks at the downloaded frontier. A
        // sink seek parses the stream through that reader, so any target
        // past the frontier would stall the audio thread and freeze the
        // interface until the download catches up. Route every live source
        // through the deferred reload instead: seeks issued while the
        // buffer is still downloading are queued, and once the file is
        // complete a fresh decoder is positioned at the requested spot.
        // Fragmented MP4 keeps its sample count in movie fragments instead
        // of the movie header, so the decoder reports a zero total
        // duration; the reload positions such files by discarding samples
        // because rodio clamps byte seeks to that zero duration, which
        // silently restarts the track.
        let progressive_seek = ProgressiveSeek {
            path,
            format: audio.format,
            completion,
            timeline_seek_session: audio.timeline_seek_session,
        };
        Ok(PreparedSource::new(decoded, duration, audio.file)
            .with_progressive_seek(progressive_seek))
    }

    fn install_progressive_source(
        &mut self,
        source: DecodedSource,
        position: Duration,
        file: Option<tempfile::NamedTempFile>,
        resume_after: Option<bool>,
        timeline_cancellation: Option<CancellationToken>,
    ) -> bool {
        let timeline_reload = timeline_cancellation.is_some();
        replace_active_timeline_cancellation(
            &mut self.active_timeline_cancellation,
            timeline_cancellation,
        );
        // A timeline reload replaces the front buffer with a suffix; any
        // other install goes back to playing a whole-file decoder.
        self.active_suffix = timeline_reload.then_some(position);
        let standby_dropped = self.sink.len() > 1;
        if standby_dropped {
            discard_progressive_seek(&mut self.standby_progressive_seek);
            self.standby_reopen = None;
        }
        let should_pause = should_pause_after_seek(resume_after, self.sink.is_paused());
        let volume = self.sink.volume();
        self.sink.stop();
        let sink = Arc::new(Sink::connect_new(self.stream.mixer()));
        sink.set_volume(volume);
        sink.append(self.wrap_source(source));
        if should_pause {
            sink.pause();
        } else {
            sink.play();
        }
        self.sink = sink;
        self.position_base = position;
        self.pending_position = None;
        if let Some(file) = file {
            self.retain_playback_file(file);
        }
        standby_dropped
    }

    /// Buffer paths the live seek state still opens by name. Completed
    /// buffer seeks and M4A reloads reopen these paths to build a fresh
    /// decoder, so dropping the owning temp file would make every later
    /// seek fail with an unopenable playback buffer.
    fn live_seek_paths(&self) -> [Option<&Path>; 4] {
        [
            self.progressive_seek
                .as_ref()
                .map(|seek| seek.path.as_path()),
            self.standby_progressive_seek
                .as_ref()
                .map(|seek| seek.path.as_path()),
            self.front_reopen.as_ref().map(|(path, _)| path.as_path()),
            self.standby_reopen.as_ref().map(|(path, _)| path.as_path()),
        ]
    }

    /// Retains a playback buffer file, evicting the oldest superseded one.
    ///
    /// Rapid mid-download seeks land one suffix buffer per request, so a
    /// plain FIFO trim would evict the current track's buffer after two
    /// landed seeks even though the engine still opens it by path. Only
    /// files no live seek path names and that are not the newest buffer
    /// feeding the sink are ever dropped.
    fn retain_playback_file(&mut self, file: tempfile::NamedTempFile) {
        self.retained_files.push(file);
        while self.retained_files.len() > 2 {
            let live = self.live_seek_paths();
            let Some(index) = self
                .retained_files
                .iter()
                .take(self.retained_files.len() - 1)
                .position(|file| live.iter().flatten().all(|path| file.path() != *path))
            else {
                break;
            };
            self.retained_files.remove(index);
        }
    }

    fn cancel_pending_progressive_reload(&mut self) {
        cancel_active_timeline_cancellation(&mut self.active_timeline_cancellation);
        self.pending_seek_completion.take();
        if let Some(pending) = self.pending_progressive_reload.take() {
            pending.cancellation.store(true, Ordering::Release);
            if let Some(cancellation) = pending.timeline_cancellation {
                cancellation.cancel();
            }
            if self.playback_intent.load(Ordering::Acquire) {
                self.sink.play();
            }
        }
        self.pending_position = None;
    }

    fn schedule_progressive_reload(&mut self, position: Duration) -> Result<(), String> {
        let (path, format, completion, timeline_seek_session, complete) = {
            let progressive_seek = self
                .progressive_seek
                .as_ref()
                .ok_or_else(|| "The progressive source is unavailable".to_string())?;
            (
                progressive_seek.path.clone(),
                progressive_seek.format,
                progressive_seek.completion.clone(),
                progressive_seek.timeline_seek_session.clone(),
                progressive_seek.completion.is_complete(),
            )
        };
        self.cancel_pending_progressive_reload();
        if let Some(timeline_seek_session) = timeline_seek_session
            && !complete
            && !timeline_seek_session.can_seek_from_front(position)
        {
            return self.schedule_timeline_reload(timeline_seek_session, position);
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let (completion_sender, completion_receiver) = oneshot::channel();
        self.sink.pause();
        let spawn_result = thread::Builder::new()
            .name("ralgrum-progressive-seek".into())
            .spawn(move || {
                let result = if complete {
                    Self::decoder_at_with_cancellation(
                        &path,
                        format,
                        position,
                        &worker_cancellation,
                    )
                } else {
                    Self::progressive_decoder_at_with_cancellation(
                        &path,
                        format,
                        position,
                        &completion,
                        &worker_cancellation,
                    )
                };
                let _ = sender.send(result.map(|source| (source, None)));
                let _ = completion_sender.send(());
            });
        if spawn_result.is_err() {
            if self.playback_intent.load(Ordering::Acquire) {
                self.sink.play();
            }
            return Err("The playback seek worker could not be started".to_string());
        }
        self.pending_position = Some(position);
        self.pending_seek_completion = Some(SeekCompletion {
            receiver: completion_receiver,
        });
        self.pending_progressive_reload = Some(PendingProgressiveReload {
            position,
            cancellation,
            timeline_cancellation: None,
            receiver,
        });
        Ok(())
    }

    fn schedule_timeline_reload(
        &mut self,
        session: Arc<dyn TimelineSeekSession>,
        position: Duration,
    ) -> Result<(), String> {
        self.cancel_pending_progressive_reload();
        let request = session.request(position)?;
        let timeline_cancellation = request.cancellation.clone();
        let format = request.format;
        let intra_segment = request.intra_segment_offset;
        let startup = request.startup;
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = cancellation.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        let (completion_sender, completion_receiver) = oneshot::channel();
        self.sink.pause();
        let spawn_result = thread::Builder::new()
            .name("ralgrum-timeline-seek".into())
            .spawn(move || {
                let result: Result<(DecodedSource, Option<tempfile::NamedTempFile>), String> =
                    (|| {
                        let startup = startup.recv().map_err(|_| {
                            "The timeline seek worker stopped unexpectedly".to_string()
                        })??;
                        // A session that lands its fragment before the
                        // target reports the exact discard once the
                        // fetch completes; otherwise the request already
                        // carried the intra-segment offset.
                        let intra_segment = startup.intra_segment_offset.unwrap_or(intra_segment);
                        let mut decoder = Decoder::builder()
                            .with_data(startup.reader)
                            .with_hint(format.extension())
                            .with_mime_type(format.mime_type())
                            .build()
                            .map_err(|error| {
                                format!("Could not decode {} audio: {error}", format.label())
                            })?;
                        discard_decoder_samples(
                            &mut decoder,
                            intra_segment,
                            Some(&worker_cancellation),
                        )?;
                        Ok((Box::new(decoder) as DecodedSource, Some(startup.file)))
                    })();
                let _ = sender.send(result);
                let _ = completion_sender.send(());
            });
        if spawn_result.is_err() {
            timeline_cancellation.cancel();
            if self.playback_intent.load(Ordering::Acquire) {
                self.sink.play();
            }
            return Err("The timeline seek worker could not be started".to_string());
        }
        self.pending_position = Some(position);
        self.pending_seek_completion = Some(SeekCompletion {
            receiver: completion_receiver,
        });
        self.pending_progressive_reload = Some(PendingProgressiveReload {
            position,
            cancellation,
            timeline_cancellation: Some(timeline_cancellation),
            receiver,
        });
        Ok(())
    }

    fn take_progressive_reload_result(
        &mut self,
    ) -> Result<Option<ProgressiveReloadResult>, String> {
        let result = match self.pending_progressive_reload.as_ref() {
            Some(pending) => pending.receiver.try_recv(),
            None => return Ok(None),
        };
        match result {
            Ok(Ok(source)) => {
                let pending = self
                    .pending_progressive_reload
                    .take()
                    .expect("pending progressive reload disappeared");
                self.pending_position = None;
                self.pending_seek_completion.take();
                Ok(Some(ProgressiveReloadResult {
                    source: source.0,
                    position: pending.position,
                    file: source.1,
                    timeline_cancellation: pending.timeline_cancellation,
                }))
            }
            Ok(Err(error)) => {
                self.pending_seek_completion.take();
                if let Some(pending) = self.pending_progressive_reload.take() {
                    if let Some(cancellation) = pending.timeline_cancellation {
                        cancellation.cancel();
                    }
                    if self.playback_intent.load(Ordering::Acquire) {
                        self.sink.play();
                    }
                }
                self.pending_position = None;
                // The failed fetch superseded any landed suffix, so the
                // indicator can no longer trust the session's counters.
                self.active_suffix = None;
                Err(error)
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_seek_completion.take();
                if let Some(pending) = self.pending_progressive_reload.take() {
                    if let Some(cancellation) = pending.timeline_cancellation {
                        cancellation.cancel();
                    }
                    if self.playback_intent.load(Ordering::Acquire) {
                        self.sink.play();
                    }
                }
                self.pending_position = None;
                self.active_suffix = None;
                Err("The playback seek worker stopped unexpectedly".into())
            }
        }
    }

    fn reported_position(&self) -> Duration {
        reported_position_with_pending(
            self.pending_position,
            self.position_base,
            self.sink.get_pos(),
        )
    }
}

pub(crate) fn asio_endpoint_matches_driver(device: &str, driver: &str) -> bool {
    let device = device.to_ascii_lowercase();
    driver
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| {
            word.len() >= 5
                && !matches!(word, "driver" | "audio" | "sound" | "device")
                && device.contains(word)
        })
}

fn log_output_stream_error(error: rodio::cpal::StreamError) {
    diagnostics::event("WARN", format!("audio output stream error: {error}"));
}

fn cancel_active_timeline_cancellation(active: &mut Option<CancellationToken>) {
    if let Some(cancellation) = active.take() {
        cancellation.cancel();
    }
}

fn replace_active_timeline_cancellation(
    active: &mut Option<CancellationToken>,
    replacement: Option<CancellationToken>,
) {
    cancel_active_timeline_cancellation(active);
    *active = replacement;
}

fn reported_position(base: Duration, sink_position: Duration) -> Duration {
    base.saturating_add(sink_position)
}

fn reported_position_with_pending(
    pending: Option<Duration>,
    base: Duration,
    sink_position: Duration,
) -> Duration {
    pending.unwrap_or_else(|| reported_position(base, sink_position))
}

fn should_pause_after_seek(resume_after: Option<bool>, current_paused: bool) -> bool {
    resume_after.map_or(current_paused, |resume| !resume)
}

fn discard_decoder_samples<D>(
    decoder: &mut D,
    position: Duration,
    cancellation: Option<&AtomicBool>,
) -> Result<(), String>
where
    D: Source<Item = f32>,
{
    let target_samples = position
        .as_nanos()
        .saturating_mul(u128::from(decoder.sample_rate()))
        .saturating_mul(u128::from(decoder.channels()))
        / 1_000_000_000;
    let target_samples = u64::try_from(target_samples).unwrap_or(u64::MAX);
    for index in 0..target_samples {
        if index % 4096 == 0 && cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return Err("Playback request cancelled".into());
        }
        if decoder.next().is_none() {
            break;
        }
    }
    Ok(())
}

fn discard_progressive_seek(progressive_seek: &mut Option<ProgressiveSeek>) {
    if let Some(progressive_seek) = progressive_seek.take() {
        progressive_seek.completion.clear_pending_seek();
    }
}

struct ProgressiveOpus {
    packets: Option<PacketReader<ProgressiveReader>>,
    decoder: OpusDecoder,
    channels: u16,
    pre_skip: usize,
    decoded_frames: usize,
    samples: Vec<f32>,
    cursor: usize,
    duration: Option<Duration>,
}

impl ProgressiveOpus {
    fn new(reader: ProgressiveReader, duration: Option<Duration>) -> Result<Self, String> {
        let (packets, channels, pre_skip) = Self::packet_reader(reader)?;
        let decoder = OpusDecoder::new(48_000, usize::from(channels))
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        Ok(Self {
            packets: Some(packets),
            decoder,
            channels,
            pre_skip,
            decoded_frames: 0,
            samples: Vec::new(),
            cursor: 0,
            duration,
        })
    }

    fn packet_reader(
        reader: ProgressiveReader,
    ) -> Result<(PacketReader<ProgressiveReader>, u16, usize), String> {
        let mut packets = PacketReader::new(reader);
        let head = packets
            .read_packet_expected()
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        if head.data.len() < 19 || &head.data[..8] != b"OpusHead" {
            return Err("Could not decode Ogg Opus audio: invalid OpusHead".into());
        }
        let channels = u16::from(head.data[9]);
        let pre_skip = usize::from(u16::from_le_bytes([head.data[10], head.data[11]]));
        let gain = i16::from_le_bytes([head.data[16], head.data[17]]);
        if gain != 0 {
            return Err("Could not decode Ogg Opus audio: output gain is unsupported".into());
        }
        let tags = packets
            .read_packet_expected()
            .map_err(|error| format!("Could not decode Ogg Opus audio: {error}"))?;
        if !tags.data.starts_with(b"OpusTags") {
            return Err("Could not decode Ogg Opus audio: invalid OpusTags".into());
        }
        Ok((packets, channels, pre_skip))
    }
}

impl Iterator for ProgressiveOpus {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.cursor < self.samples.len() {
                let sample = self.samples[self.cursor];
                self.cursor += 1;
                return Some(sample);
            }
            self.samples.clear();
            self.cursor = 0;
            let packet = self.packets.as_mut()?.read_packet().ok()??;
            let mut decoded =
                vec![0.0; OpusDecoder::MAX_FRAME_SIZE_48K * usize::from(self.channels)];
            let frames = self
                .decoder
                .decode_float(&packet.data, &mut decoded, false)
                .ok()?;
            let start_frames = self
                .pre_skip
                .saturating_sub(self.decoded_frames)
                .min(frames);
            self.decoded_frames += frames;
            self.samples.extend_from_slice(
                &decoded[start_frames * usize::from(self.channels)
                    ..frames * usize::from(self.channels)],
            );
        }
    }
}

impl Source for ProgressiveOpus {
    fn current_span_len(&self) -> Option<usize> {
        let remaining = self.samples.len().saturating_sub(self.cursor);
        (remaining > 0).then_some(remaining)
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        48_000
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let target = self
            .duration
            .map_or(position, |duration| position.min(duration));
        let packets = self.packets.take().ok_or_else(|| {
            SeekError::Other(Box::new(std::io::Error::other(
                "The Ogg Opus stream is unavailable",
            )))
        })?;
        let mut reader = packets.into_inner();
        reader
            .seek(SeekFrom::Start(0))
            .map_err(|error| SeekError::Other(Box::new(error)))?;
        let (packets, channels, pre_skip) = Self::packet_reader(reader)
            .map_err(|error| SeekError::Other(Box::new(std::io::Error::other(error))))?;
        self.packets = Some(packets);
        self.channels = channels;
        self.pre_skip = pre_skip;
        self.decoded_frames = 0;
        self.samples.clear();
        self.cursor = 0;
        self.decoder = OpusDecoder::new(48_000, usize::from(channels)).map_err(|error| {
            SeekError::Other(Box::new(std::io::Error::other(error.to_string())))
        })?;
        let samples = (target.as_secs_f64() * 48_000.0).floor() as u64 * u64::from(channels);
        for _ in 0..samples {
            if self.next().is_none() {
                break;
            }
        }
        Ok(())
    }
}

impl AudioEngine for RodioEngine {
    fn load(&mut self, prepared: PreparedSource, volume: f32, playing: bool) -> Option<Duration> {
        crate::plugins::minimeters::tap::clear();
        self.set_playback_intent(false);
        self.cancel_pending_progressive_reload();
        self.active_suffix = None;
        discard_progressive_seek(&mut self.progressive_seek);
        discard_progressive_seek(&mut self.standby_progressive_seek);
        self.sink.stop();
        self.sink = Arc::new(Sink::connect_new(self.stream.mixer()));
        self.sink.set_volume(volume);
        self.transport_gain.reset(1.0);
        crate::plugins::minimeters::tap::set_volume(volume);
        self.position_base = Duration::ZERO;
        let duration = prepared.duration();
        self.front_reopen = prepared.output_reopen();
        self.standby_reopen = None;
        let (source, file, progressive_seek) = prepared.into_parts();
        self.sink.append(self.wrap_source(source));
        self.set_playback_intent(playing);
        if playing {
            self.sink.play();
        } else {
            self.sink.pause();
        }
        self.progressive_seek = progressive_seek;
        self.retain_playback_file(file);
        duration
    }

    fn play(&self) {
        self.set_playback_intent(true);
        self.sink.play();
    }
    fn pause(&self) {
        self.set_playback_intent(false);
        self.sink.pause();
        crate::plugins::minimeters::tap::clear();
    }
    fn stop(&mut self) {
        self.set_playback_intent(false);
        crate::plugins::minimeters::tap::clear();
        self.cancel_pending_progressive_reload();
        self.active_suffix = None;
        discard_progressive_seek(&mut self.progressive_seek);
        discard_progressive_seek(&mut self.standby_progressive_seek);
        self.front_reopen = None;
        self.standby_reopen = None;
        self.sink.stop();
    }
    fn seek(&mut self, position: Duration) -> Result<SeekOutcome, String> {
        crate::plugins::minimeters::tap::clear();
        if let Some(progressive_seek) = self.progressive_seek.as_ref() {
            if !progressive_seek.completion.is_complete() {
                // While the buffer is still downloading, a timeline session
                // can land inside its existing prefix or fetch a suffix at
                // the target. Sources without one queue the target until the
                // download completes.
                if progressive_seek.timeline_seek_session.is_some() {
                    self.schedule_progressive_reload(position)?;
                    return Ok(SeekOutcome::Deferred);
                }
                progressive_seek.completion.request_seek(position);
                self.pending_position = Some(position);
                return Ok(SeekOutcome::Deferred);
            }
            // The buffer is complete, so a fresh decoder can be positioned
            // locally instead of fetching anything over the network again.
            progressive_seek.completion.clear_pending_seek();
            if progressive_seek.format == AudioFormat::M4a {
                self.schedule_progressive_reload(position)?;
                return Ok(SeekOutcome::Deferred);
            }
            let decoder =
                Self::decoder_at(&progressive_seek.path, progressive_seek.format, position)?;
            let standby_dropped =
                self.install_progressive_source(decoder, position, None, None, None);
            return Ok(if standby_dropped {
                SeekOutcome::AppliedStandbyDropped
            } else {
                SeekOutcome::Applied
            });
        }
        self.sink
            .try_seek(position)
            .map_err(|_| "This stream could not seek to that position".into())
            .map(|_| {
                self.position_base = Duration::ZERO;
                SeekOutcome::Applied
            })
    }
    fn seek_for_output_restore(&mut self, position: Duration) -> Result<SeekOutcome, String> {
        if self
            .progressive_seek
            .as_ref()
            .is_some_and(|seek| seek.completion.is_complete())
        {
            self.schedule_progressive_reload(position)?;
            Ok(SeekOutcome::Deferred)
        } else {
            self.seek(position)
        }
    }
    fn take_seek_completion(&mut self) -> Option<SeekCompletion> {
        self.pending_seek_completion.take()
    }
    fn timeline_suffix_state(&self) -> Option<TimelineSuffixState> {
        let session = self
            .progressive_seek
            .as_ref()?
            .timeline_seek_session
            .as_ref()?;
        let mut state = session.suffix_state()?;
        if let Some(pending) = self.pending_progressive_reload.as_ref() {
            state.pending = true;
            state.base = pending.position;
        } else {
            // Only report a landed suffix; without one the session's
            // counters describe a fetch the engine no longer plays.
            state.base = self.active_suffix?;
        }
        Some(state)
    }
    fn set_playback_intent(&self, playing: bool) {
        self.playback_intent.store(playing, Ordering::Release);
    }
    fn apply_deferred_seek(&mut self) -> Result<SeekOutcome, String> {
        if let Some(result) = self.take_progressive_reload_result()? {
            let resume_after = self.playback_intent.load(Ordering::Acquire);
            let standby_dropped = self.install_progressive_source(
                result.source,
                result.position,
                result.file,
                Some(resume_after),
                result.timeline_cancellation,
            );
            return Ok(if standby_dropped {
                SeekOutcome::AppliedStandbyDropped
            } else {
                SeekOutcome::Applied
            });
        }
        let Some(progressive_seek) = self.progressive_seek.as_ref() else {
            return Ok(SeekOutcome::Deferred);
        };
        if progressive_seek.timeline_seek_session.is_some() {
            return Ok(SeekOutcome::Deferred);
        }
        if !progressive_seek.completion.is_complete() {
            return Ok(SeekOutcome::Deferred);
        }
        let Some(position) = progressive_seek.completion.take_pending_seek() else {
            return Ok(SeekOutcome::Deferred);
        };
        if progressive_seek.format == AudioFormat::M4a {
            self.schedule_progressive_reload(position)?;
            return Ok(SeekOutcome::Deferred);
        }
        let decoder = Self::decoder_at(&progressive_seek.path, progressive_seek.format, position)?;
        let standby_dropped = self.install_progressive_source(decoder, position, None, None, None);
        Ok(if standby_dropped {
            SeekOutcome::AppliedStandbyDropped
        } else {
            SeekOutcome::Applied
        })
    }
    fn set_transport_gain_target(&self, target: f32) {
        self.transport_gain.set_target(target);
    }
    fn reset_transport_gain(&self, gain: f32) {
        self.transport_gain.reset(gain);
    }
    fn transport_gain_settled(&self, target: f32) -> bool {
        self.transport_gain.is_at_target(target)
    }
    fn set_automation_gain_target(&self, target: f32, duration: Duration) {
        self.automation_gain
            .set_target_with_duration(target, duration);
    }
    fn reset_automation_gain(&self, gain: f32) {
        self.automation_gain.reset(gain);
    }
    fn automation_gain_settled(&self, target: f32) -> bool {
        self.automation_gain.is_at_target(target)
    }
    /// Commits a stream and decoder prepared off the UI thread. The caller
    /// holds the old sink paused while preparing, so its captured position
    /// remains exact and this handoff performs no blocking driver or decode
    /// work on the UI thread.
    fn set_output(&mut self, prepared: PreparedOutputSwitch) -> OutputSwitch {
        let PreparedOutputSwitch {
            stream,
            source,
            position,
            target,
        } = prepared;
        let had_source = !self.sink.empty();
        let restored = had_source && source.is_some();

        // The pending reload workers and the armed standby die with the old
        // sink; their buffers can no longer reach a mixer.
        self.cancel_pending_progressive_reload();
        discard_progressive_seek(&mut self.standby_progressive_seek);
        self.standby_reopen = None;
        self.sink.stop();
        self.stream = stream;

        if let Some(source) = source.filter(|_| had_source) {
            // The positioned install recreates the sink on the new stream
            // through the same path a completed-buffer seek uses, rebasing
            // the position probe onto the reloaded source.
            self.install_progressive_source(source, position, None, Some(false), None);
        } else {
            let sink = Arc::new(Sink::connect_new(self.stream.mixer()));
            sink.set_volume(self.sink.volume());
            if had_source {
                // The buffer could not be reloaded. Keep the position probe
                // at the captured timeline spot and hold the sink paused so
                // an empty sink is not mistaken for a finished track while
                // the model reloads it on the new device.
                diagnostics::event(
                    "INFO",
                    format!(
                        "audio output switch is reloading an in-flight source at {}ms",
                        position.as_millis()
                    ),
                );
                self.position_base = position;
                sink.pause();
            } else {
                sink.pause();
            }
            self.sink = sink;
        }
        let label = match &target {
            AudioOutputTarget::SystemDefault => "the system default".to_owned(),
            AudioOutputTarget::Device(name) | AudioOutputTarget::AsioDriver(name) => {
                format!("\"{name}\"")
            }
        };
        diagnostics::event("INFO", format!("audio output switched to {label}"));
        self.output_target = target;
        if had_source && !restored {
            OutputSwitch::SourceLost
        } else {
            OutputSwitch::SourceRestored
        }
    }
    fn output_target(&self) -> &AudioOutputTarget {
        &self.output_target
    }
    fn set_volume(&self, volume: f32) {
        self.sink.set_volume(volume);
        crate::plugins::minimeters::tap::set_volume(volume);
    }
    fn position(&self) -> Duration {
        self.reported_position()
    }
    fn ended(&self) -> bool {
        !self.sink.is_paused() && self.sink.empty()
    }
    fn append_standby(&mut self, prepared: PreparedSource) {
        discard_progressive_seek(&mut self.standby_progressive_seek);
        self.standby_reopen = prepared.output_reopen();
        let (source, file, progressive_seek) = prepared.into_parts();
        self.sink.append(self.wrap_source(source));
        self.standby_progressive_seek = progressive_seek;
        self.retain_playback_file(file);
    }

    fn skip_to_standby(&mut self) {
        self.cancel_pending_progressive_reload();
        self.active_suffix = None;
        self.sink.skip_one();
    }
    fn activate_standby(&mut self) {
        self.cancel_pending_progressive_reload();
        self.active_suffix = None;
        discard_progressive_seek(&mut self.progressive_seek);
        self.progressive_seek = self.standby_progressive_seek.take();
        self.front_reopen = self.standby_reopen.take();
        self.position_base = Duration::ZERO;
    }
    fn sink_probe(&self) -> SinkProbe {
        SinkProbe::new(self.sink.clone())
    }
    fn owns_probe(&self, probe: &SinkProbe) -> bool {
        probe.is_sink(&self.sink)
    }
}

#[cfg(test)]
mod tests;
