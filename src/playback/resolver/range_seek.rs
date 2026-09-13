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

/// Serves one byte range of the track, from `[start, end]` inclusive.
pub(super) type RangeFetch = Arc<
    dyn Fn(u64, u64, CancellationToken) -> BoxFuture<'static, Result<Vec<u8>, String>>
        + Send
        + Sync,
>;

/// A timeline seek session for remote progressive MP3 sources.
///
/// SoundCloud and Deezer transcodes are constant bitrate MP3, so a seek
/// target maps to a byte offset and the CDN can serve the rest of the file
/// starting there. Each request fetches that suffix into a fresh progressive
/// buffer, which lets the engine land the seek while the download is still
/// running instead of queueing it until the buffer completes.
pub(crate) struct RangeTimelineSession {
    fetch: RangeFetch,
    total: u64,
    duration: Duration,
    runtime: Handle,
    track_cancellation: CancellationToken,
}

impl RangeTimelineSession {
    pub(super) fn new(
        fetch: RangeFetch,
        total: u64,
        duration: Duration,
        runtime: Handle,
        track_cancellation: CancellationToken,
    ) -> Self {
        Self {
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
        let fraction = (position.as_secs_f64() / seconds).clamp(0.0, 0.99);
        let start = (fraction * self.total as f64) as u64;
        let buffer = ProgressiveFile::new(AudioFormat::Mp3, None)
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
        self.runtime.spawn(async move {
            let error_sender = startup_sender.clone();
            if let Err(error) = write_suffix(
                writer,
                fetch,
                start,
                total,
                reader,
                file,
                startup_sender,
                &worker_cancellation,
            )
            .await
            {
                let _ = error_sender.send(Err(error));
            }
        });
        Ok(TimelineSeekRequest {
            format: AudioFormat::Mp3,
            intra_segment_offset: Duration::ZERO,
            cancellation,
            startup: startup_receiver,
        })
    }
}

async fn write_suffix(
    mut writer: ProgressiveWriter,
    fetch: RangeFetch,
    start: u64,
    total: u64,
    reader: ProgressiveReader,
    file: tempfile::NamedTempFile,
    startup_sender: mpsc::SyncSender<Result<TimelineSeekStartup, String>>,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt as _;

    let mut startup: Option<(ProgressiveReader, tempfile::NamedTempFile)> = Some((reader, file));
    let mut written = 0_u64;
    let mut offset = start.min(total);
    while offset < total {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let end = (offset + SUFFIX_CHUNK - 1).min(total - 1);
        let bytes = fetch(offset, end, cancellation.clone())
            .await
            .map_err(|error| format!("The seek suffix could not be downloaded: {error}"))?;
        writer
            .write_all(&bytes)
            .await
            .map_err(|_| "The seek buffer could not be written".to_string())?;
        writer
            .flush()
            .await
            .map_err(|_| "The seek buffer could not be finalized".to_string())?;
        written = written.saturating_add(bytes.len() as u64);
        offset = end.saturating_add(1);
        if written >= SUFFIX_STARTUP_BYTES || offset >= total {
            if let Some((reader, file)) = startup.take() {
                writer.mark_startup_ready();
                if startup_sender
                    .send(Ok(TimelineSeekStartup { reader, file }))
                    .is_err()
                {
                    return Err("The seek result was cancelled".into());
                }
            }
        }
    }
    if let Some((reader, file)) = startup.take() {
        // The suffix was empty or too small for the startup threshold.
        writer.mark_startup_ready();
        let _ = startup_sender.send(Ok(TimelineSeekStartup { reader, file }));
    }
    writer
        .finish()
        .await
        .map_err(|_| "The seek buffer could not be finalized".to_string())
}

#[cfg(test)]
mod tests {
    use std::{process::Command, time::Duration};

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

    fn sample_rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
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
        let fetch: RangeFetch = {
            let bytes = std::sync::Arc::new(bytes);
            Arc::new(move |start, end, _cancellation| {
                let bytes = bytes.clone();
                Box::pin(async move { Ok(bytes[start as usize..=(end as usize)].to_vec()) })
            })
        };
        let session = RangeTimelineSession::new(
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
        let fetch: RangeFetch = {
            let bytes = std::sync::Arc::new(bytes);
            Arc::new(move |start, end, _cancellation| {
                let bytes = bytes.clone();
                Box::pin(async move { Ok(bytes[start as usize..=(end as usize)].to_vec()) })
            })
        };
        let session = Arc::new(RangeTimelineSession::new(
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
}
