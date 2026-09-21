use super::*;
use crate::playback::media_source::{
    BackendCacheIdentity, BackendFuture, BackendProvenance, BackendSourceMetadata, BackendSourceOps,
};
use std::sync::Mutex;
use tokio::sync::Notify;

pub(super) type BackendResolveOverride =
    Arc<dyn Fn(&CancellationToken) -> MediaResolveOutcome<BackendSource> + Send + Sync>;

struct RecordingBackendSource {
    metadata: super::super::media_source::BackendSourceMetadata,
    bytes: Arc<Vec<u8>>,
    ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    first_read_started: Arc<Notify>,
    second_read_started: Arc<Notify>,
    allow_second_read: Arc<Notify>,
}

impl super::super::media_source::BackendSourceOps for RecordingBackendSource {
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
        start: u64,
        end: u64,
        _cancellation: &'a CancellationToken,
    ) -> super::super::media_source::BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>> {
        let bytes = self.bytes.clone();
        let ranges = self.ranges.clone();
        let first_read_started = self.first_read_started.clone();
        let second_read_started = self.second_read_started.clone();
        let allow_second_read = self.allow_second_read.clone();
        Box::pin(async move {
            let read_number = {
                let mut ranges = ranges.lock().unwrap();
                ranges.push((start, end));
                ranges.len()
            };
            match read_number {
                1 => first_read_started.notify_one(),
                2 => {
                    second_read_started.notify_one();
                    allow_second_read.notified().await;
                }
                _ => {}
            }

            let start = usize::try_from(start)
                .map_err(|_| PlaybackDownloadError::message("test range start overflow"))?;
            let end = usize::try_from(end)
                .map_err(|_| PlaybackDownloadError::message("test range end overflow"))?;
            if start > end || end >= bytes.len() {
                return Err(PlaybackDownloadError::message("test range out of bounds"));
            }
            Ok(bytes[start..=end].to_vec())
        })
    }

    fn probe_size<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
    ) -> super::super::media_source::BackendFuture<'a, Option<u64>> {
        Box::pin(async { Some(self.metadata.size) })
    }

    fn download<'a>(
        &'a self,
        _output: &'a mut dyn DownloadOutput,
        _cancellation: &'a CancellationToken,
        _progress: Option<&'a ProgressCallback>,
    ) -> super::super::media_source::BackendFuture<'a, Result<(), PlaybackDownloadError>> {
        Box::pin(async { Err(PlaybackDownloadError::message("test backend source")) })
    }
}
#[tokio::test]
async fn backend_cache_download_writes_startup_prefix_before_remaining_block() {
    let total = BLOCK_SIZE + 37;
    let bytes: Arc<Vec<u8>> = Arc::new((0..total).map(|index| (index % 251) as u8).collect());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let first_read_started = Arc::new(Notify::new());
    let second_read_started = Arc::new(Notify::new());
    let allow_second_read = Arc::new(Notify::new());
    let source = BackendSource::from_ops(Arc::new(RecordingBackendSource {
        metadata: super::super::media_source::BackendSourceMetadata {
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            size: total,
            declared_bitrate: None,
            duration: None,
            timeline: false,
            cacheable: true,
            initial_buffered_fraction: None,
            deezer_track_id: None,
            provenance: super::super::media_source::BackendProvenance::Deezer,
            cache_identity: super::super::media_source::BackendCacheIdentity::new(
                "backend-prefix-test",
            ),
        },
        bytes: bytes.clone(),
        ranges: ranges.clone(),
        first_read_started: first_read_started.clone(),
        second_read_started: second_read_started.clone(),
        allow_second_read: allow_second_read.clone(),
    }));
    let resolved = ResolvedSource {
        data: SourceData::Backend(source),
        size: total,
        deezer_track_id: None,
        is_soundcloud: false,
        cache_identity: Some("backend-prefix-test".into()),
        format: AudioFormat::Flac,
        format_name: "FLAC".into(),
        declared_bitrate: None,
    };
    let temp = tempfile::tempdir().unwrap();
    let resolver = StreamResolver::new()
        .unwrap()
        .with_cache(AudioCache::new(temp.path().into(), 8));
    let file = ProgressiveFile::new(AudioFormat::Flac, Some(total)).unwrap();
    let mut reader = file.reader().unwrap();
    let mut writer = file.writer().unwrap();
    let cleanup = file.writer().unwrap();
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(async move {
        let result = resolver
            .download_playback(resolved, &mut writer, &cancellation, None, None, None)
            .await;
        let finish = writer.finish().await;
        (result, finish)
    });

    if tokio::time::timeout(Duration::from_secs(2), first_read_started.notified())
        .await
        .is_err()
    {
        cleanup.cancel();
        task.abort();
        let _ = task.await;
        panic!("backend prefix request did not start");
    }
    let startup = startup_bytes(AudioFormat::Flac, Some(total));
    let ready_reader = file.reader().unwrap();
    let mut ready = tokio::task::spawn_blocking(move || ready_reader.wait_until_ready(startup));
    let readiness = tokio::time::timeout(Duration::from_secs(2), &mut ready).await;
    if readiness.is_err() {
        cleanup.cancel();
        task.abort();
        let _ = task.await;
        let _ = ready.await;
        panic!("backend startup prefix did not release the reader");
    }
    readiness.unwrap().unwrap().unwrap();
    assert_eq!(reader.written(), startup);
    assert_eq!(ranges.lock().unwrap().first(), Some(&(0, startup - 1)));

    tokio::time::timeout(Duration::from_secs(2), second_read_started.notified())
        .await
        .expect("backend remainder request did not start");
    allow_second_read.notify_one();
    let (result, finish) = task.await.unwrap();
    assert!(result.is_ok(), "backend download failed: {result:?}");
    finish.unwrap();

    let ranges = ranges.lock().unwrap().clone();
    assert_eq!(
        ranges,
        vec![
            (0, startup - 1),
            (startup, BLOCK_SIZE - 1),
            (BLOCK_SIZE, total - 1),
        ]
    );
    assert!(ranges.windows(2).all(|pair| pair[0].1 + 1 == pair[1].0));
    let requested = ranges
        .iter()
        .map(|(start, end)| end - start + 1)
        .sum::<u64>();
    assert_eq!(requested, total);
    let mut actual = Vec::new();
    reader.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, *bytes);
}

struct OfflineBackendSource {
    metadata: BackendSourceMetadata,
    remote_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

impl BackendSourceOps for OfflineBackendSource {
    fn metadata(&self) -> BackendSourceMetadata {
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
    ) -> BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>> {
        Box::pin(async move {
            self.remote_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("Remote ranges are unavailable in this fixture".into())
        })
    }

    fn probe_size<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Option<u64>> {
        Box::pin(async { Some(self.metadata.size) })
    }

    fn download<'a>(
        &'a self,
        output: &'a mut dyn DownloadOutput,
        _cancellation: &'a CancellationToken,
        _progress: Option<&'a ProgressCallback>,
    ) -> BackendFuture<'a, Result<(), PlaybackDownloadError>> {
        Box::pin(async move {
            self.remote_attempts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.metadata.timeline {
                return Err("Remote timeline downloads are unavailable in this fixture".into());
            }
            output.write_all(b"refreshed audio").await.unwrap();
            output.flush().await.unwrap();
            Ok(())
        })
    }
}

fn offline_backend(
    total: u64,
    timeline: bool,
    format: AudioFormat,
) -> (BackendSource, Arc<std::sync::atomic::AtomicUsize>) {
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let source = BackendSource::from_ops(Arc::new(OfflineBackendSource {
        metadata: BackendSourceMetadata {
            format,
            format_name: match format {
                AudioFormat::Mp3 => "MP3_320".into(),
                _ => format.label().into(),
            },
            size: total,
            declared_bitrate: None,
            duration: Some(Duration::from_secs(1)),
            timeline,
            cacheable: true,
            initial_buffered_fraction: None,
            deezer_track_id: None,
            provenance: BackendProvenance::Deezer,
            cache_identity: BackendCacheIdentity::new("offline-backend-regression"),
        },
        remote_attempts: attempts.clone(),
    }));
    (source, attempts)
}

struct CachedTimeline {
    _directory: tempfile::TempDir,
    resolver: StreamResolver,
    source: ResolvedSource,
    bytes: Vec<u8>,
    remote_attempts: Arc<std::sync::atomic::AtomicUsize>,
}

async fn cached_timeline(total: u64) -> CachedTimeline {
    let directory = tempfile::tempdir().unwrap();
    let cache = AudioCache::new(directory.path().into(), 8);
    let bytes = (0..total)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let (backend, remote_attempts) = offline_backend(total, true, AudioFormat::M4a);
    let source = ResolvedSource::from_backend(backend);
    let key = cache_key(&source).unwrap();
    cache_bytes(&cache, &key, total, &bytes, cache.generation(), None).await;
    assert!(cache.is_fully_cached(&key, total).await);
    CachedTimeline {
        _directory: directory,
        resolver: StreamResolver::new().unwrap().with_cache(cache),
        source,
        bytes,
        remote_attempts,
    }
}

#[tokio::test]
async fn fully_cached_backend_timeline_releases_startup_without_remote_ranges() {
    let fixture = cached_timeline(BLOCK_SIZE + 37).await;
    let total = fixture.bytes.len() as u64;
    let file = ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let mut writer = file.writer().unwrap();
    let ready_reader = file.reader().unwrap();
    let mut ready = tokio::task::spawn_blocking(move || ready_reader.wait_until_startup_ready());

    let result = fixture
        .resolver
        .download_playback(
            fixture.source,
            &mut writer,
            &CancellationToken::new(),
            None,
            None,
            None,
        )
        .await;
    // Do not finish the writer: completion also releases startup waiters and
    // would conceal the missing cached-path readiness signal.
    let readiness = tokio::time::timeout(Duration::from_secs(2), &mut ready).await;
    if readiness.is_err() {
        writer.cancel();
        let _ = ready.await;
        panic!("fully cached timeline did not release its startup waiter: {result:?}");
    }
    result.unwrap();
    readiness.unwrap().unwrap().unwrap();
    assert!(!writer.is_complete());
    assert_eq!(
        fixture
            .remote_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let mut actual = vec![0; total as usize];
    file.reader().unwrap().read_exact(&mut actual).unwrap();
    assert_eq!(actual, fixture.bytes);
    writer.finish().await.unwrap();
}

#[tokio::test]
async fn cached_timeline_cancelled_on_last_chunk_does_not_release_startup() {
    let fixture = cached_timeline(PROGRESSIVE_WRITE_CHUNK_SIZE as u64 + 1).await;
    let total = fixture.bytes.len() as u64;
    let file = ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let mut writer = file.writer().unwrap();
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let progress: ProgressCallback = Arc::new(move |update| {
        if update.downloaded == total {
            cancel.cancel();
        }
    });
    let result = fixture
        .resolver
        .download_playback(
            fixture.source,
            &mut writer,
            &cancellation,
            Some(progress),
            None,
            None,
        )
        .await;
    writer.cancel();
    assert!(result.is_err());
    assert!(file.reader().unwrap().wait_until_startup_ready().is_err());
    assert_eq!(
        fixture
            .remote_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn cache_maintenance_does_not_interrupt_cached_playback() {
    let fixture = cached_timeline(PROGRESSIVE_WRITE_CHUNK_SIZE as u64 + 1).await;
    let total = fixture.bytes.len() as u64;
    let file = ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let mut writer = file.writer().unwrap();
    let cache = fixture.resolver.cache.clone().unwrap();
    let progress: ProgressCallback = Arc::new(move |update| {
        if update.downloaded == total {
            cache.cancel();
        }
    });
    let result = fixture
        .resolver
        .download_playback(
            fixture.source,
            &mut writer,
            &CancellationToken::new(),
            Some(progress),
            None,
            None,
        )
        .await;
    result.unwrap();
    file.reader().unwrap().wait_until_startup_ready().unwrap();
    let mut actual = vec![0; total as usize];
    file.reader().unwrap().read_exact(&mut actual).unwrap();
    assert_eq!(actual, fixture.bytes);
    writer.finish().await.unwrap();
}

#[tokio::test]
async fn cache_maintenance_does_not_interrupt_cached_progressive_playback() {
    let fixture = cached_timeline(PROGRESSIVE_WRITE_CHUNK_SIZE as u64 + 1).await;
    let total = fixture.bytes.len() as u64;
    let file = ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let writer = file.writer().unwrap();
    let cache = fixture.resolver.cache.clone().unwrap();
    let source_cache_epoch = fixture.resolver.resolved_source_cache.epoch();
    let progress: ProgressCallback = Arc::new(move |update| {
        if update.downloaded == total {
            cache.cancel();
        }
    });
    let track = PlaybackTrack {
        provider: PlaybackProvider::Deezer,
        id: "offline-backend-regression".into(),
        title: "Fixture track".into(),
        artist: "Fixture artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(1),
        downloadable: false,
        progressive: true,
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    let (mut writer, result) = fixture
        .resolver
        .download_progressive(
            track,
            None,
            None,
            None,
            fixture.source,
            CancellationToken::new(),
            Some(progress),
            writer,
            Arc::new(tokio::sync::Mutex::new(false)),
            None,
            source_cache_epoch,
        )
        .await;
    result.unwrap();
    file.reader().unwrap().wait_until_startup_ready().unwrap();
    let mut actual = vec![0; total as usize];
    file.reader().unwrap().read_exact(&mut actual).unwrap();
    assert_eq!(actual, fixture.bytes);
    writer.finish().await.unwrap();
}

#[tokio::test]
async fn cached_timeline_missing_final_block_does_not_release_startup() {
    let fixture = cached_timeline(BLOCK_SIZE + 37).await;
    let total = fixture.bytes.len() as u64;
    let cache = fixture.resolver.cache.as_ref().unwrap();
    let final_block = cache.block_path(
        &cache_key(&fixture.source).unwrap(),
        total,
        BLOCK_SIZE,
        total - 1,
    );
    let file = ProgressiveFile::new(AudioFormat::M4a, Some(total)).unwrap();
    let mut writer = file.writer().unwrap();
    let progress: ProgressCallback = Arc::new(move |update| {
        if update.downloaded == BLOCK_SIZE {
            std::fs::remove_file(&final_block).unwrap();
        }
    });
    let result = fixture
        .resolver
        .download_playback(
            fixture.source,
            &mut writer,
            &CancellationToken::new(),
            Some(progress),
            None,
            None,
        )
        .await;
    writer.fail("incomplete cached timeline");
    assert!(result.is_err());
    assert_eq!(file.reader().unwrap().written(), BLOCK_SIZE);
    assert!(file.reader().unwrap().wait_until_startup_ready().is_err());
    assert_eq!(
        fixture
            .remote_attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[derive(Clone, Copy, Debug)]
enum ResolvePath {
    Playback,
    ExactFlac,
}

const RESOLVE_PATHS: [ResolvePath; 2] = [ResolvePath::Playback, ResolvePath::ExactFlac];

impl ResolvePath {
    async fn resolve(
        self,
        resolver: &StreamResolver,
        cancellation: CancellationToken,
    ) -> Result<ResolvedSource, String> {
        let track = PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: "refresh-regression".into(),
            title: "Fixture track".into(),
            artist: "Fixture artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: false,
            explicit: false,
            ai_generated: false,
            service_url: String::new(),
        };
        let credentials = crate::murglar_backend::test_media_credentials();
        match self {
            Self::Playback => {
                resolver
                    .resolve_source(&track, None, None, Some(credentials), cancellation, false)
                    .await
            }
            Self::ExactFlac => {
                resolver
                    .resolve_deezer_exact(
                        &track,
                        None,
                        Some(&credentials),
                        "FLAC",
                        &cancellation,
                        false,
                    )
                    .await
            }
        }
    }
}

fn scripted_resolver(
    outcomes: Vec<MediaResolveOutcome<BackendSource>>,
    cancel_on_attempt: Option<usize>,
) -> (StreamResolver, Arc<std::sync::atomic::AtomicUsize>) {
    let mut resolver = StreamResolver::new().unwrap();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = attempts.clone();
    let outcomes = Mutex::new(std::collections::VecDeque::from(outcomes));
    resolver.backend_resolve_override = Some(Arc::new(move |cancellation| {
        let attempt = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if cancel_on_attempt == Some(attempt) {
            cancellation.cancel();
        }
        outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra backend resolution")
    }));
    (resolver, attempts)
}

fn refreshed_source(format: AudioFormat) -> MediaResolveOutcome<BackendSource> {
    MediaResolveOutcome::Source(offline_backend(b"refreshed audio".len() as u64, false, format).0)
}

#[tokio::test]
async fn source_and_exact_resolution_retry_refresh_once_and_return_downloadable_audio() {
    for path in RESOLVE_PATHS {
        let (resolver, attempts) = scripted_resolver(
            vec![
                MediaResolveOutcome::RefreshSource("expired media".into()),
                refreshed_source(AudioFormat::Flac),
            ],
            None,
        );
        let source = path
            .resolve(&resolver, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(source.format, AudioFormat::Flac);
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut output = File::from_std(file.reopen().unwrap());
        resolver
            .download_source_inner(source, &mut output, &CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(
            tokio::fs::read(file.path()).await.unwrap(),
            b"refreshed audio"
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn source_and_exact_resolution_stop_on_a_second_refresh() {
    for path in RESOLVE_PATHS {
        let (resolver, attempts) = scripted_resolver(
            vec![
                MediaResolveOutcome::RefreshSource("first refresh".into()),
                MediaResolveOutcome::RefreshSource("refresh exhausted".into()),
            ],
            None,
        );
        assert_eq!(
            path.resolve(&resolver, CancellationToken::new())
                .await
                .err()
                .as_deref(),
            Some("refresh exhausted")
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn source_and_exact_resolution_stop_on_fatal_or_cancelled_outcomes() {
    for path in RESOLVE_PATHS {
        for refresh_first in [false, true] {
            for cancelled in [false, true] {
                let mut outcomes = Vec::new();
                if refresh_first {
                    outcomes.push(MediaResolveOutcome::RefreshSource("expired media".into()));
                }
                outcomes.push(if cancelled {
                    MediaResolveOutcome::Cancelled
                } else {
                    MediaResolveOutcome::Fatal("backend refused source".into())
                });
                let (resolver, attempts) = scripted_resolver(outcomes, None);
                let result = path.resolve(&resolver, CancellationToken::new()).await;
                assert_eq!(
                    result.err().as_deref(),
                    Some(if cancelled {
                        "Playback request cancelled"
                    } else {
                        "backend refused source"
                    })
                );
                assert_eq!(
                    attempts.load(std::sync::atomic::Ordering::SeqCst),
                    if refresh_first { 2 } else { 1 },
                    "{path:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn source_and_exact_resolution_respect_cancellation_before_refresh() {
    for path in RESOLVE_PATHS {
        let (resolver, attempts) = scripted_resolver(
            vec![MediaResolveOutcome::RefreshSource("expired media".into())],
            Some(1),
        );
        let cancellation = CancellationToken::new();
        let result = path.resolve(&resolver, cancellation.clone()).await;
        assert!(cancellation.is_cancelled());
        assert_eq!(result.err().as_deref(), Some("Playback request cancelled"));
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn source_and_exact_resolution_discard_refreshed_source_after_cancellation() {
    for path in RESOLVE_PATHS {
        let (resolver, attempts) = scripted_resolver(
            vec![
                MediaResolveOutcome::RefreshSource("expired media".into()),
                refreshed_source(AudioFormat::Flac),
            ],
            Some(2),
        );
        let result = path.resolve(&resolver, CancellationToken::new()).await;
        assert_eq!(result.err().as_deref(), Some("Playback request cancelled"));
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn source_and_exact_resolution_skip_backend_when_already_cancelled() {
    for path in RESOLVE_PATHS {
        let (resolver, attempts) = scripted_resolver(Vec::new(), None);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = path.resolve(&resolver, cancellation).await;
        assert_eq!(result.err().as_deref(), Some("Playback request cancelled"));
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{path:?}"
        );
    }
}

#[tokio::test]
async fn exact_resolution_rejects_wrong_quality_after_refresh() {
    let (resolver, attempts) = scripted_resolver(
        vec![
            MediaResolveOutcome::RefreshSource("expired media".into()),
            refreshed_source(AudioFormat::Mp3),
        ],
        None,
    );
    assert!(
        ResolvePath::ExactFlac
            .resolve(&resolver, CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// A progressive backend source whose byte ranges and downloads are served
/// from memory, standing in for a Deezer track resolved through the murglar
/// backend.
struct SeekFixtureBackend {
    metadata: BackendSourceMetadata,
    bytes: Arc<Vec<u8>>,
}

impl BackendSourceOps for SeekFixtureBackend {
    fn metadata(&self) -> BackendSourceMetadata {
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
        start: u64,
        end: u64,
        _cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>> {
        let bytes = self.bytes.clone();
        Box::pin(async move {
            let start = usize::try_from(start)
                .map_err(|_| PlaybackDownloadError::message("test range start overflow"))?;
            let end = usize::try_from(end)
                .map_err(|_| PlaybackDownloadError::message("test range end overflow"))?;
            if start > end || end >= bytes.len() {
                return Err(PlaybackDownloadError::message("test range out of bounds"));
            }
            Ok(bytes[start..=end].to_vec())
        })
    }

    fn probe_size<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
    ) -> BackendFuture<'a, Option<u64>> {
        Box::pin(async { Some(self.metadata.size) })
    }

    fn download<'a>(
        &'a self,
        output: &'a mut dyn DownloadOutput,
        _cancellation: &'a CancellationToken,
        _progress: Option<&'a ProgressCallback>,
    ) -> BackendFuture<'a, Result<(), PlaybackDownloadError>> {
        let bytes = self.bytes.clone();
        Box::pin(async move {
            output.set_total_hint(Some(bytes.len() as u64));
            output.write_all(&bytes).await.map_err(|_| {
                PlaybackDownloadError::message("test download could not be written")
            })?;
            output.flush().await.map_err(|_| {
                PlaybackDownloadError::message("test download could not be finalized")
            })?;
            Ok(())
        })
    }
}

fn seek_fixture_source(format: AudioFormat, bytes: Arc<Vec<u8>>) -> BackendSource {
    let size = bytes.len() as u64;
    BackendSource::from_ops(Arc::new(SeekFixtureBackend {
        metadata: BackendSourceMetadata {
            format,
            format_name: match format {
                AudioFormat::Mp3 => "MP3_320".into(),
                _ => format.label().into(),
            },
            size,
            declared_bitrate: None,
            duration: None,
            timeline: false,
            cacheable: true,
            initial_buffered_fraction: None,
            deezer_track_id: Some("42".into()),
            provenance: BackendProvenance::Deezer,
            cache_identity: BackendCacheIdentity::new("deezer-range-session-test"),
        },
        bytes,
    }))
}

/// Deterministic fixture bytes with a valid FLAC header prefix so the
/// progressive prefix validation passes without reaching the network.
fn flac_fixture_bytes() -> Vec<u8> {
    let mut bytes = b"fLaC\x80\x00\x00\x22".to_vec();
    let mut stream_info = [0_u8; 34];
    stream_info[0..2].copy_from_slice(&4096_u16.to_be_bytes());
    stream_info[2..4].copy_from_slice(&4096_u16.to_be_bytes());
    // 44.1kHz, 2 channels, 16 bits per sample, 4 seconds of samples.
    let packed = (u64::from(44_100_u32) << 44) | (1_u64 << 41) | (15_u64 << 36) | (4_u64 * 44_100);
    stream_info[10..18].copy_from_slice(&packed.to_be_bytes());
    bytes.extend_from_slice(&stream_info);
    bytes.extend((0..64 * 1024).map(|index| (index % 251) as u8));
    bytes
}

async fn resolve_deezer_progressive_fixture(
    format: AudioFormat,
    bytes: Arc<Vec<u8>>,
) -> Result<ResolvedProgressiveAudio, String> {
    let mut resolver = StreamResolver::new().unwrap();
    let backend = seek_fixture_source(format, bytes);
    resolver.backend_resolve_override = Some(Arc::new(move |_| {
        MediaResolveOutcome::Source(backend.clone())
    }));
    let track = PlaybackTrack {
        provider: PlaybackProvider::Deezer,
        id: "deezer-range-session".into(),
        title: "Fixture track".into(),
        artist: "Fixture artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(4),
        downloadable: false,
        progressive: false,
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    };
    resolver
        .resolve_progressive(
            &track,
            None,
            None,
            Some(crate::murglar_backend::test_media_credentials()),
            CancellationToken::new(),
            None,
        )
        .await
}

#[tokio::test]
async fn deezer_backend_progressive_mp3_attaches_a_range_seek_session() {
    let bytes: Arc<Vec<u8>> = Arc::new((0..128 * 1024).map(|index| (index % 251) as u8).collect());
    let audio = resolve_deezer_progressive_fixture(AudioFormat::Mp3, bytes)
        .await
        .unwrap();
    assert!(
        audio.timeline_seek_session.is_some(),
        "a Deezer backend MP3 source must land mid-download seeks through a range session"
    );
}

#[tokio::test]
async fn deezer_backend_progressive_flac_attaches_a_range_seek_session() {
    let audio =
        resolve_deezer_progressive_fixture(AudioFormat::Flac, Arc::new(flac_fixture_bytes()))
            .await
            .unwrap();
    assert!(
        audio.timeline_seek_session.is_some(),
        "a Deezer backend FLAC source must land mid-download seeks through a range session"
    );
}

#[tokio::test]
async fn backend_range_sessions_skip_timelines_and_unknown_sizes() {
    let resolver = StreamResolver::new().unwrap();
    let cancellation = CancellationToken::new();
    let (progressive, _) = offline_backend(1_000, false, AudioFormat::Flac);
    assert!(
        resolver
            .backend_range_seek_session(
                &progressive,
                Some(1_000),
                Some(Duration::from_secs(4)),
                &cancellation,
                DownloadPauseGate::new()
            )
            .is_some()
    );
    let (timeline, _) = offline_backend(0, true, AudioFormat::Flac);
    assert!(
        resolver
            .backend_range_seek_session(
                &timeline,
                None,
                None,
                &cancellation,
                DownloadPauseGate::new()
            )
            .is_none(),
        "timeline sources keep their own seek sessions"
    );
    let (unknown, _) = offline_backend(0, false, AudioFormat::Mp3);
    assert!(
        resolver
            .backend_range_seek_session(
                &unknown,
                None,
                Some(Duration::from_secs(4)),
                &cancellation,
                DownloadPauseGate::new()
            )
            .is_none(),
        "unknown sizes stay on the deferred path"
    );
}

#[tokio::test]
async fn direct_deezer_progressive_sources_attach_range_seek_sessions() {
    let resolver = StreamResolver::new().unwrap();
    let cancellation = CancellationToken::new();
    let url = "https://cdnt-stream.dzcdn.net/media/42/file";
    let source = |format: AudioFormat| ResolvedSource {
        data: SourceData::Remote(url.into()),
        size: 0,
        deezer_track_id: Some("42".into()),
        is_soundcloud: false,
        cache_identity: None,
        format,
        format_name: format.label().into(),
        declared_bitrate: None,
    };
    let session = |format: AudioFormat| {
        resolver.progressive_range_seek_session(
            url,
            &source(format),
            Some(262_144),
            Some(Duration::from_secs(4)),
            &cancellation,
            DownloadPauseGate::new(),
        )
    };
    assert!(
        session(AudioFormat::Mp3).is_some(),
        "direct Deezer MP3 keeps its range session"
    );
    assert!(
        session(AudioFormat::Flac).is_some(),
        "direct Deezer FLAC now lands mid-download seeks through a range session"
    );
    assert!(
        session(AudioFormat::Wav).is_none(),
        "formats without a range strategy stay on the deferred path"
    );
    assert!(
        session(AudioFormat::M4a).is_none(),
        "M4A keeps its completed-buffer reload path"
    );
}
