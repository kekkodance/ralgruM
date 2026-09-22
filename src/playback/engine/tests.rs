use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{path::PathBuf, process::Command};

use super::*;

#[test]
fn prepared_output_can_cross_the_worker_boundary() {
    fn assert_send<T: Send>() {}
    assert_send::<OutputReloadSpec>();
    assert_send::<PreparedOutputSwitch>();
}

const MP3: &str = "SUQzBAAAAAAAI1RTU0UAAAAPAAADTGF2ZjYyLjEyLjEwMQAAAAAAAAAAAAAA/+M4wAAAAAAAAAAAAEluZm8AAAAPAAAAAwAAAbAAqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq1dXV1dXV1dXV1dXV1dXV1dXV1dXV1dXV1dXV1dXV1dXV////////////////////////////////////////////AAAAAExhdmM2Mi4yOAAAAAAAAAAAAAAAACQC8AAAAAAAAAGwJxQu6wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA/+MYxAAMSJKUeU8AAAQJKAL3ve973vSlKUpSlL3u/f337xKp8t4t4m4uZc1WGxACAYrB9//+U9/R/gQ5z/QqyypquH9pKtLZ/+MYxAkOONqsAZgwAPpZTWjVJzuG5TKo07UpBdEwrDLwHpekaeFCSmCJCSNVVtBO9BgrWo4ShIGso6CsS5IkTWxjn/qSKSYp/+MYxAsMKMY0AckIAQy0qKSWMkQqFTOSlLVQqGXoSVlZE0qhlFCyaFDAp5sLgV+Jv/+KTEFNRTMuMTAwqqqqqqqqqqqqqqqq";
const FLAC: &str = "ZkxhQwAAACICQAJAAADsAADsAfQA8AAAAZDc4wSniVrUdsvJq5fhZrQqhAAALg0AAABMYXZmNjIuMTIuMTAxAQAAABUAAABlbmNvZGVyPUxhdmY2Mi4xMi4xMDH/+HQIAAGPJE4BIgU/CkgNtg/BD8QODAqP5jFGjwprR9+4EI+MQUE+rwigEg/MnK73aeqOqCKCCKMEqUAoBkDhZBKmRJISwRonpphaIWIoDYlEKQwkgSUIOI5cJhKCi8jTWkoQBhCUkRCiFCYGEMx7TrCYiQTERk07QsJhYULLIYhk0siRQhWXzRqwRhQlC5GJGuKgsEYJlya+ExEigomE7kZSRChFCRieQ1aKEkJK4hiE4rBYoLJl6dkwTCBAMMCKVlITAMIZg0jQoGBMAp0SKWgQokKNAiRoMSBBhQ5E+xl9K5IOo9ApgGAqOw==";
const WAV: &str = "UklGRmYDAABXQVZFZm10IBAAAAABAAEAQB8AAIA+AAACABAATElTVBoAAABJTkZPSVNGVA4AAABMYXZmNjIuMTIuMTAxAGRhdGEgAwAAIgE/BUgKtg3BD8QPDA6PCucFfwAR+zH2f/Jj8CHwv/EN9aX5/v52BGcJOw1+D+0Peg5QC9AGggEG/AP3D/Ok8AnwUvFX9Lz4/v19A5IIpAw3D/0P4A4ADLUHgQIB/dz3rfPz8AHw8/Cs89v3Af2AArUHAAzfDv0PNw+kDJMIfgMA/r34V/RS8Qnwo/AP8wL3BfyBAc8GTwt5Du0Pfw87DWcJdwT//qb5DfW/8SHwY/B+8jL2DvuAAOMFlAoEDs0Ptw/FDTMKbAUAAJX6zvU78krwM/D78Wz1HPp///EEzgmCDZwP3w9BDvQKWwYCAYrB9//+U9/R/gQ5z/QqyypquH9pKtLZ/+MYxAkOONqsAZgwAPpZTWjVJzuG5TKo07UpBdEwrDLwHpekaeFCSmCJCSNVVtBO9BgrWo4ShIGso6CsS5IkTWxjn/qSKSYp/+MYxAsMKMY0AckIAQy0qKSWMkQqFTOSlLVQqGXoSVlZE0qhlFCyaFDAp5sLgV+Jv/+KTEFNRTMuMTAwqqqqqqqqqqqqqqqq";

#[test]
fn hinted_rodio_decoder_reads_core_format_fixtures() {
    for (format, encoded) in [
        (AudioFormat::Mp3, MP3),
        (AudioFormat::Flac, FLAC),
        (AudioFormat::Wav, WAV),
    ] {
        let file = tempfile::Builder::new()
            .suffix(&format!(".{}", format.extension()))
            .tempfile()
            .unwrap();
        std::fs::write(file.path(), STANDARD.decode(encoded).unwrap()).unwrap();
        let mut decoder = RodioEngine::decoder(file.path(), format).unwrap();
        assert!(
            decoder.next().is_some(),
            "{} fixture was empty",
            format.label()
        );
    }
}

#[test]
fn decode_prepares_a_source_with_its_decoded_duration() {
    let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    std::fs::write(file.path(), STANDARD.decode(WAV).unwrap()).unwrap();
    let audio = ResolvedAudio {
        path: file.path().to_owned(),
        file,
        duration: None,
        format: AudioFormat::Wav,
        declared_bitrate: None,
    };
    let prepared = RodioEngine::decode(audio).unwrap();
    assert!(prepared.duration().is_some());
}

#[test]
fn decode_errors_include_safe_format_and_underlying_error() {
    let file = tempfile::Builder::new().suffix(".flac").tempfile().unwrap();
    std::fs::write(file.path(), b"not audio").unwrap();
    let error = match RodioEngine::decoder(file.path(), AudioFormat::Flac) {
        Ok(_) => panic!("invalid FLAC unexpectedly decoded"),
        Err(error) => error,
    };
    assert!(error.starts_with("Could not decode FLAC audio: "));
    assert!(!error.contains("http"));
}

#[test]
fn progressive_seek_keeps_only_the_latest_pending_target_and_can_cancel_it() {
    let file = super::super::progressive::ProgressiveFile::new(AudioFormat::Wav, None).unwrap();
    let reader = file.reader().unwrap();
    let completion = reader.completion();
    completion.request_seek(Duration::from_secs(2));
    completion.request_seek(Duration::from_secs(3));
    assert_eq!(completion.pending_seek(), Some(Duration::from_secs(3)));
    completion.clear_pending_seek();
    assert_eq!(completion.pending_seek(), None);
}

#[test]
fn reported_position_preserves_the_preseeked_reload_offset() {
    assert_eq!(
        reported_position(Duration::from_secs(42), Duration::from_millis(250)),
        Duration::from_millis(42_250)
    );
    assert_eq!(
        reported_position(Duration::ZERO, Duration::from_millis(250)),
        Duration::from_millis(250)
    );
}

#[test]
fn pending_hls_seek_reports_the_requested_position_without_moving_the_sink() {
    assert_eq!(
        reported_position_with_pending(
            Some(Duration::from_secs(2_399)),
            Duration::from_secs(12),
            Duration::from_millis(250),
        ),
        Duration::from_secs(2_399)
    );
    assert_eq!(
        reported_position_with_pending(None, Duration::from_secs(12), Duration::from_millis(250),),
        Duration::from_millis(12_250)
    );
}

#[tokio::test]
async fn seek_completion_waits_for_the_worker_signal() {
    let (sender, receiver) = oneshot::channel();
    let completion = SeekCompletion { receiver };
    sender.send(()).unwrap();
    completion.wait().await;
}

#[test]
fn timeline_seek_uses_the_latest_shared_playback_intent() {
    let intent = Arc::new(AtomicBool::new(true));
    intent.store(false, Ordering::Release);
    assert!(!intent.load(Ordering::Acquire));
    intent.store(true, Ordering::Release);
    assert!(intent.load(Ordering::Acquire));
    assert!(!should_pause_after_seek(Some(true), true));
    assert!(!should_pause_after_seek(Some(true), false));
    assert!(should_pause_after_seek(Some(false), true));
    assert!(should_pause_after_seek(Some(false), false));
    assert!(should_pause_after_seek(None, true));
    assert!(!should_pause_after_seek(None, false));
}

#[test]
fn active_timeline_cancellation_transfers_from_pending_and_cancels_replaced_work() {
    let old = CancellationToken::new();
    let pending = CancellationToken::new();
    let mut active = Some(old.clone());

    replace_active_timeline_cancellation(&mut active, Some(pending.clone()));

    assert!(old.is_cancelled());
    assert!(!pending.is_cancelled());
    assert!(
        active
            .as_ref()
            .is_some_and(|cancellation| !cancellation.is_cancelled())
    );

    cancel_active_timeline_cancellation(&mut active);

    assert!(pending.is_cancelled());
    assert!(active.is_none());
}

#[test]
fn local_reseek_keeps_the_suffix_writer_token() {
    let path = PathBuf::from("growing-suffix.flac");
    let completion = ProgressiveCompletion::for_completed_buffer(AudioFormat::Flac);
    let landed = LandedSuffixSource {
        path: path.clone(),
        completion,
        base: Duration::from_secs(10),
    };
    let writer = CancellationToken::new();
    let mut active = Some(writer.clone());
    let transferred = take_reused_suffix_cancellation(&path, Some(&landed), &mut active, None);
    assert!(active.is_none());
    assert!(!writer.is_cancelled());

    let (_sender, receiver) = mpsc::sync_channel(1);
    let mut pending = PendingProgressiveReload {
        position: Duration::from_secs(11),
        cancellation: Arc::new(AtomicBool::new(false)),
        timeline_cancellation: transferred,
        suffix_path: Some(path.clone()),
        suffix_base: Some(Duration::from_secs(10)),
        receiver,
    };
    let transferred_again =
        take_reused_suffix_cancellation(&path, Some(&landed), &mut active, Some(&mut pending));
    assert!(pending.timeline_cancellation.is_none());
    assert!(!transferred_again.unwrap().is_cancelled());
    assert!(!writer.is_cancelled());
}

#[test]
fn standby_activation_discards_the_previous_progressive_seek() {
    let file = super::super::progressive::ProgressiveFile::new(AudioFormat::M4a, None).unwrap();
    let reader = file.reader().unwrap();
    let completion = reader.completion();
    completion.request_seek(Duration::from_secs(12));
    let mut progressive_seek = Some(ProgressiveSeek {
        path: std::path::PathBuf::from("old-track.m4a"),
        format: AudioFormat::M4a,
        completion: completion.clone(),
        timeline_seek_session: None,
    });

    discard_progressive_seek(&mut progressive_seek);

    assert!(progressive_seek.is_none());
    assert_eq!(completion.pending_seek(), None);
}

#[test]
fn sequential_seeked_decoders_do_not_restart_from_zero() {
    let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    std::fs::write(file.path(), STANDARD.decode(WAV).unwrap()).unwrap();
    let mut from_start = RodioEngine::decoder(file.path(), AudioFormat::Wav).unwrap();
    let first_sample = from_start.next().unwrap();
    let mut first_seek =
        RodioEngine::decoder_at(file.path(), AudioFormat::Wav, Duration::from_millis(1)).unwrap();
    let first_seek_sample = first_seek.next().unwrap();
    let mut second_seek =
        RodioEngine::decoder_at(file.path(), AudioFormat::Wav, Duration::from_millis(2)).unwrap();
    let second_seek_sample = second_seek.next().unwrap();
    assert_ne!(first_seek_sample, first_sample);
    assert_ne!(second_seek_sample, first_seek_sample);
}

#[test]
fn bounded_discard_uses_only_the_intra_segment_offset() {
    let samples = (0..10_000).map(|value| value as f32).collect::<Vec<_>>();
    let mut source = SamplesBuffer::new(1, 1_000, samples);
    discard_decoder_samples(&mut source, Duration::from_millis(7), None).unwrap();
    assert_eq!(source.next(), Some(7.0));
}

#[test]
fn bounded_discard_honors_cancellation_before_long_work() {
    let mut source = SamplesBuffer::new(1, 1_000, vec![0.0; 100_000]);
    let cancellation = AtomicBool::new(true);
    let error = discard_decoder_samples(&mut source, Duration::from_secs(40), Some(&cancellation))
        .expect_err("cancelled discard must stop before decoding the track prefix");
    assert_eq!(error, "Playback request cancelled");
}

#[test]
fn fragmented_aac_seek_reaches_the_requested_nonzero_region() {
    let Some(file) = make_fragmented_aac_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping fragmented AAC fixture test");
        return;
    };
    let mut from_start = RodioEngine::decoder(file.path(), AudioFormat::M4a).unwrap();
    let start_samples = from_start.by_ref().take(4_096).collect::<Vec<_>>();
    let mut seeked =
        RodioEngine::decoder_at(file.path(), AudioFormat::M4a, Duration::from_millis(800)).unwrap();
    let seeked_samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
    let start_rms = sample_rms(&start_samples);
    let seeked_rms = sample_rms(&seeked_samples);
    assert!(start_rms < 0.05, "start RMS was {start_rms}");
    assert!(seeked_rms > 0.2, "seeked RMS was {seeked_rms}");
}

#[tokio::test]
async fn fragmented_m4a_progressive_source_keeps_the_reload_seek_path() {
    let Some(file) = make_fragmented_aac_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping fragmented M4A reload test");
        return;
    };
    let bytes = std::fs::read(file.path()).unwrap();
    let total = bytes.len() as u64;
    let buffer =
        super::super::progressive::ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let mut writer = buffer.writer().unwrap();
    use tokio::io::AsyncWriteExt as _;
    writer.write_all(&bytes).await.unwrap();
    writer.flush().await.unwrap();
    writer.finish().await.unwrap();
    let audio = ResolvedProgressiveAudio {
        reader: buffer.reader().unwrap(),
        file: buffer.into_file(),
        duration: None,
        format: AudioFormat::M4a,
        total: Some(total),
        timeline_size_unknown: false,
        declared_bitrate: Some(160),
        initial_downloaded: total,
        initial_buffered_fraction: None,
        fully_cached: false,
        timeline_seek_session: None,
        worker: None,
    };
    let prepared = RodioEngine::decode_progressive(audio).unwrap();
    let (_sink_source, _file, progressive_seek) = prepared.into_parts();
    let progressive_seek =
        progressive_seek.expect("fragmented M4A must seek through the discard-based reload");
    assert_eq!(progressive_seek.format, AudioFormat::M4a);
    assert!(progressive_seek.completion.is_complete());
    assert!(progressive_seek.timeline_seek_session.is_none());
    let mut seeked = RodioEngine::decoder_at(
        &progressive_seek.path,
        AudioFormat::M4a,
        Duration::from_millis(800),
    )
    .unwrap();
    let seeked_samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
    let seeked_rms = sample_rms(&seeked_samples);
    assert!(
        seeked_rms > 0.2,
        "the reload must land in the tone region, RMS was {seeked_rms}"
    );
}

#[tokio::test]
async fn mp3_progressive_source_seeks_through_the_deferred_reload() {
    let Some(file) = make_mp3_tone_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping MP3 reload test");
        return;
    };
    let bytes = std::fs::read(file.path()).unwrap();
    let total = bytes.len() as u64;
    let frontier =
        super::super::progressive::startup_bytes(AudioFormat::Mp3, Some(total)).min(total);
    let buffer =
        super::super::progressive::ProgressiveFile::new(AudioFormat::Mp3, Some(total)).unwrap();
    let mut writer = buffer.writer().unwrap();
    use tokio::io::AsyncWriteExt as _;
    writer.write_all(&bytes[..frontier as usize]).await.unwrap();
    writer.flush().await.unwrap();
    let audio = ResolvedProgressiveAudio {
        reader: buffer.reader().unwrap(),
        file: buffer.into_file(),
        duration: Some(Duration::from_secs(4)),
        format: AudioFormat::Mp3,
        total: Some(total),
        timeline_size_unknown: false,
        declared_bitrate: Some(128),
        initial_downloaded: frontier,
        initial_buffered_fraction: None,
        fully_cached: false,
        timeline_seek_session: None,
        worker: None,
    };
    let prepared = RodioEngine::decode_progressive(audio).unwrap();
    let (_sink_source, _file, progressive_seek) = prepared.into_parts();
    // A progressive MP3 must never fall back to the sink seek: the
    // decoder reads through the shared reader, so a sink seek past the
    // downloaded frontier would stall the audio thread until the
    // download catches up. The reload path queues the target instead.
    let progressive_seek =
        progressive_seek.expect("progressive MP3 must seek through the deferred reload");
    assert_eq!(progressive_seek.format, AudioFormat::Mp3);
    assert!(
        !progressive_seek.completion.is_complete(),
        "a partial download must not count as complete"
    );
    assert!(progressive_seek.timeline_seek_session.is_none());
    progressive_seek
        .completion
        .request_seek(Duration::from_millis(3_500));
    assert_eq!(
        progressive_seek.completion.pending_seek(),
        Some(Duration::from_millis(3_500))
    );

    writer.write_all(&bytes[frontier as usize..]).await.unwrap();
    writer.flush().await.unwrap();
    writer.finish().await.unwrap();
    assert!(progressive_seek.completion.is_complete());
    assert_eq!(
        progressive_seek.completion.take_pending_seek(),
        Some(Duration::from_millis(3_500))
    );
    let mut seeked = RodioEngine::decoder_at(
        &progressive_seek.path,
        AudioFormat::Mp3,
        Duration::from_millis(3_500),
    )
    .unwrap();
    let seeked_samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
    let seeked_rms = sample_rms(&seeked_samples);
    assert!(
        seeked_rms > 0.2,
        "the reload must land in the tone region, RMS was {seeked_rms}"
    );
}

#[tokio::test]
async fn progressive_reload_seeks_inside_the_existing_growing_file() {
    let Some(file) = make_mp3_tone_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping local progressive seek test");
        return;
    };
    let bytes = std::fs::read(file.path()).unwrap();
    let total = bytes.len() as u64;
    let frontier = total.saturating_mul(98) / 100;
    let buffer =
        super::super::progressive::ProgressiveFile::new(AudioFormat::Mp3, Some(total)).unwrap();
    let path = buffer.path().to_path_buf();
    let reader = buffer.reader().unwrap();
    let completion = reader.completion();
    let mut writer = buffer.writer().unwrap();
    use tokio::io::AsyncWriteExt as _;
    writer.write_all(&bytes[..frontier as usize]).await.unwrap();
    writer.flush().await.unwrap();
    assert!(!completion.is_complete());

    let cancellation = AtomicBool::new(false);
    let mut seeked = RodioEngine::progressive_decoder_at_with_cancellation(
        &path,
        AudioFormat::Mp3,
        Duration::from_millis(3_500),
        &completion,
        &cancellation,
    )
    .unwrap();
    let samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
    let rms = sample_rms(&samples);
    assert!(
        rms > 0.2,
        "the local progressive reload must land in the tone region, RMS was {rms}"
    );
    assert_eq!(completion.written(), frontier);
}

#[tokio::test]
async fn progressive_local_seek_never_reads_past_the_flushed_frontier() {
    let Some(file) = make_flac_tone_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping frontier local seek test");
        return;
    };
    let bytes = std::fs::read(file.path()).unwrap();
    let total = bytes.len() as u64;
    let frontier = total.saturating_mul(40) / 100;
    let buffer =
        super::super::progressive::ProgressiveFile::new(AudioFormat::Flac, Some(total)).unwrap();
    let path = buffer.path().to_path_buf();
    let reader = buffer.reader().unwrap();
    let completion = reader.completion();
    let mut writer = buffer.writer().unwrap();
    use tokio::io::AsyncWriteExt as _;
    writer.write_all(&bytes[..frontier as usize]).await.unwrap();
    writer.flush().await.unwrap();
    // Drop the writer without finishing so no further bytes ever arrive; any
    // read past the frontier would block forever.
    drop(writer);
    drop(reader);
    assert!(!completion.is_complete());

    let cancellation = AtomicBool::new(false);
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || {
            let mut seeked = RodioEngine::progressive_decoder_at_with_cancellation(
                &path,
                AudioFormat::Flac,
                Duration::from_secs(1),
                &completion,
                &cancellation,
            )?;
            let samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
            Ok::<_, String>(sample_rms(&samples))
        }),
    )
    .await
    .expect("the local seek must not wait for bytes past the frontier")
    .expect("the seek worker must not panic")
    .expect("the local progressive reload must succeed");
    assert!(
        outcome > 0.2,
        "the local progressive reload must land in the tone region, RMS was {outcome}"
    );
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
            "aevalsrc=0.7*sin(2*PI*880*t):s=44100:d=4",
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

#[test]
fn hls_suffix_decodes_the_target_segment_without_prefix_media() {
    let Some((directory, init, segments)) = make_hls_fmp4_fixture() else {
        eprintln!("HLS fMP4 fixture could not be generated; skipping suffix fixture test");
        return;
    };
    let Some(target) = segments.get(2) else {
        panic!("ffmpeg HLS fixture did not produce enough media segments");
    };
    let suffix = tempfile::Builder::new().suffix(".m4a").tempfile().unwrap();
    let mut bytes = std::fs::read(init).unwrap();
    bytes.extend(std::fs::read(target).unwrap());
    std::fs::write(suffix.path(), bytes).unwrap();

    let mut decoder = RodioEngine::decoder(suffix.path(), AudioFormat::M4a).unwrap();
    discard_decoder_samples(&mut decoder, Duration::from_millis(100), None).unwrap();
    let samples = decoder.by_ref().take(4_096).collect::<Vec<_>>();
    let rms = sample_rms(&samples);

    assert!(
        rms > 0.2,
        "target HLS suffix should contain the later tone, RMS was {rms}"
    );
    drop(directory);
}

fn make_fragmented_aac_fixture() -> Option<tempfile::NamedTempFile> {
    let file = tempfile::Builder::new().suffix(".m4a").tempfile().ok()?;
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=if(lt(t\\,0.6)\\,0\\,0.7*sin(2*PI*880*t)):s=44100:d=1.2",
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
            "-y",
        ])
        .arg(file.path())
        .status()
        .ok()?;
    status.success().then_some(file)
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

fn make_hls_fmp4_fixture() -> Option<(tempfile::TempDir, PathBuf, Vec<PathBuf>)> {
    let directory = tempfile::tempdir().ok()?;
    let playlist = directory.path().join("playlist.m3u8");
    let init = directory.path().join("init.mp4");
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
            "aac",
            "-b:a",
            "96k",
            "-f",
            "hls",
            "-hls_time",
            "1",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_type",
            "fmp4",
            "-hls_flags",
            "independent_segments",
            "-hls_fmp4_init_filename",
        ])
        .arg(&init)
        .arg("-hls_segment_filename")
        .arg(directory.path().join("segment%03d.m4s"))
        .arg(&playlist)
        .status()
        .ok()?;
    if !status.success() || !init.is_file() {
        return None;
    }
    let body = std::fs::read_to_string(playlist).ok()?;
    let segments = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| directory.path().join(line))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    (segments.len() >= 3).then_some((directory, init, segments))
}

fn sample_rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Pins the standby handoff seek mechanism: a track armed through the
/// standby path is decoded by `decode` from a fully downloaded buffer,
/// and its M4A variant must carry a completed-buffer reload so an
/// in-track seek after the auto-advance defers through the discard
/// reload instead of falling back to the sink seek, which reports
/// success but silently restarts a fragmented MP4 from zero.
#[test]
fn standby_fragmented_m4a_source_seeks_through_the_completed_reload() {
    let Some(file) = make_fragmented_aac_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping standby M4A seek test");
        return;
    };
    let audio = ResolvedAudio {
        path: file.path().to_owned(),
        file,
        duration: None,
        format: AudioFormat::M4a,
        declared_bitrate: Some(160),
    };
    let prepared = RodioEngine::decode(audio).unwrap();
    let (_source, _file, progressive_seek) = prepared.into_parts();
    let progressive_seek =
        progressive_seek.expect("a standby fragmented M4A source must keep the reload seek path");
    assert_eq!(progressive_seek.format, AudioFormat::M4a);
    assert!(
        progressive_seek.completion.is_complete(),
        "the standby buffer finished downloading before it was armed"
    );
    assert!(progressive_seek.timeline_seek_session.is_none());

    let mut seeked = RodioEngine::decoder_at(
        &progressive_seek.path,
        AudioFormat::M4a,
        Duration::from_millis(800),
    )
    .unwrap();
    let seeked_samples = seeked.by_ref().take(4_096).collect::<Vec<_>>();
    let seeked_rms = sample_rms(&seeked_samples);
    assert!(
        seeked_rms > 0.2,
        "the completed-buffer reload must land in the tone region, RMS was {seeked_rms}"
    );

    // The fallback this replaces: a direct sink seek on the same file
    // claims success while the decoder restarts from the silent prefix,
    // which is exactly the auto-advance seek bug.
    let mut sink_seeking = RodioEngine::decoder(&progressive_seek.path, AudioFormat::M4a).unwrap();
    assert!(sink_seeking.try_seek(Duration::from_millis(800)).is_ok());
    let sink_samples = sink_seeking.by_ref().take(4_096).collect::<Vec<_>>();
    let sink_rms = sample_rms(&sink_samples);
    assert!(
        sink_rms < 0.05,
        "a sink seek on fragmented MP4 must not be trusted, RMS was {sink_rms}"
    );
}

/// The engine-level pin of the same mechanism: seeking a track that was
/// loaded through the standby construction must defer through the
/// completed-buffer reload and land at the target, never take the sink
/// seek branch that silently restarts a fragmented MP4.
#[test]
fn standby_m4a_track_seek_defers_through_the_reload_and_lands() {
    let Some(file) = make_fragmented_aac_fixture() else {
        eprintln!("ffmpeg is unavailable; skipping standby M4A engine seek test");
        return;
    };
    let audio = ResolvedAudio {
        path: file.path().to_owned(),
        file,
        duration: None,
        format: AudioFormat::M4a,
        declared_bitrate: Some(160),
    };
    let prepared = RodioEngine::decode(audio).unwrap();
    let Ok(mut engine) = RodioEngine::new(AudioOutputTarget::SystemDefault) else {
        eprintln!("no audio device; skipping standby M4A engine seek test");
        return;
    };
    engine.load(prepared, 1.0, true);

    let outcome = engine.seek(Duration::from_millis(800)).unwrap();
    assert_eq!(
        outcome,
        SeekOutcome::Deferred,
        "a standby fragmented M4A source must seek through the reload, not the sink"
    );

    let mut applied = None;
    for _ in 0..200 {
        match engine.apply_deferred_seek().unwrap() {
            SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped => {
                applied = Some(engine.position());
                break;
            }
            SeekOutcome::Deferred => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    let position = applied.expect("the standby seek must apply through the reload");
    assert!(
        position >= Duration::from_millis(700) && position < Duration::from_secs(2),
        "the standby seek must land near the target, was {position:?}"
    );
}

/// Formats whose complete local files seek correctly through the sink
/// keep the plain standby decode with no reload bookkeeping.
#[test]
fn standby_non_m4a_sources_keep_the_sink_seek() {
    let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
    std::fs::write(file.path(), STANDARD.decode(WAV).unwrap()).unwrap();
    let audio = ResolvedAudio {
        path: file.path().to_owned(),
        file,
        duration: None,
        format: AudioFormat::Wav,
        declared_bitrate: None,
    };
    let prepared = RodioEngine::decode(audio).unwrap();
    let (path, format) = prepared
        .output_reopen()
        .expect("complete WAV can be reopened");
    assert_eq!(format, AudioFormat::Wav);
    let reload = super::OutputReloadSpec {
        path,
        format,
        position: Duration::ZERO,
    };
    assert!(RodioEngine::reload_front_source(&reload).is_some());
    let (_source, _file, progressive_seek) = prepared.into_parts();
    assert!(
        progressive_seek.is_none(),
        "a WAV standby source seeks through the sink like before"
    );
}
