use super::crypto::deezer_key;
use super::format::MAX_SOUNDCLOUD_TRACK_AUTHORIZATION_LENGTH;
use super::format::{soundcloud_format_from_hint, soundcloud_format_from_url_path};
use super::*;
use crate::playback::PlaybackContext;
use blowfish::cipher::BlockEncrypt;

struct TestBackendSource {
    metadata: super::super::media_source::BackendSourceMetadata,
    probed_size: Option<u64>,
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
    ) -> super::super::media_source::BackendFuture<'a, Result<Vec<u8>, PlaybackDownloadError>> {
        Box::pin(async { Err(PlaybackDownloadError::message("test backend source")) })
    }

    fn probe_size<'a>(
        &'a self,
        _cancellation: &'a CancellationToken,
    ) -> super::super::media_source::BackendFuture<'a, Option<u64>> {
        Box::pin(async { self.probed_size })
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
            cache_identity: super::super::media_source::BackendCacheIdentity::new(cache_identity),
        },
        probed_size: None,
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
        ai_generated: false,
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
        ai_generated: false,
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
        ai_generated: false,
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
            crate::playback::resolve_limiter::ResolvePriority::Interactive,
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
        ai_generated: false,
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
fn download_range_response_requires_exact_partial_content() {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_RANGE, "bytes 10-19/100".parse().unwrap());
    assert!(
        validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
            .is_ok()
    );
    assert!(validate_download_range_response(StatusCode::OK, &headers, 10, 19, 100).is_err());

    headers.insert(header::CONTENT_RANGE, "bytes 9-19/100".parse().unwrap());
    assert!(
        validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
            .is_err()
    );
    headers.insert(header::CONTENT_RANGE, "bytes 10-20/100".parse().unwrap());
    assert!(
        validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
            .is_err()
    );
    headers.insert(header::CONTENT_RANGE, "bytes 10-19/101".parse().unwrap());
    assert!(
        validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
            .is_err()
    );
}

#[test]
fn download_range_response_rejects_missing_or_malformed_header() {
    let mut headers = header::HeaderMap::new();
    assert!(
        validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
            .is_err()
    );
    for value in ["bytes */100", "items 10-19/100", "bytes 10-19/*", "invalid"] {
        headers.insert(header::CONTENT_RANGE, value.parse().unwrap());
        assert!(
            validate_download_range_response(StatusCode::PARTIAL_CONTENT, &headers, 10, 19, 100)
                .is_err()
        );
    }
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

#[tokio::test]
async fn source_size_for_info_probes_an_unknown_backend_source() {
    let resolver = StreamResolver::new().unwrap();
    let backend = BackendSource::from_ops(Arc::new(TestBackendSource {
        metadata: super::super::media_source::BackendSourceMetadata {
            format: AudioFormat::Flac,
            format_name: "FLAC".into(),
            size: 0,
            declared_bitrate: None,
            duration: None,
            timeline: false,
            cacheable: true,
            initial_buffered_fraction: None,
            deezer_track_id: Some("42".into()),
            provenance: super::super::media_source::BackendProvenance::Deezer,
            cache_identity: super::super::media_source::BackendCacheIdentity::new(
                "backend-size-probe",
            ),
        },
        probed_size: Some(35_950_396),
    }));
    let source = ResolvedSource {
        data: SourceData::Backend(backend),
        size: 0,
        deezer_track_id: Some("42".into()),
        is_soundcloud: false,
        cache_identity: Some("backend-size-probe".into()),
        format: AudioFormat::Flac,
        format_name: "FLAC".into(),
        declared_bitrate: None,
    };
    assert_eq!(
        resolver
            .source_size_for_info(&source, &CancellationToken::new())
            .await
            .unwrap(),
        35_950_396
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
        ai_generated: false,
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

    let url =
        reqwest::Url::parse("https://cf-media.sndcdn.com/track.mp3?signature=flac#flac").unwrap();
    assert_eq!(
        soundcloud_format_from_url_path(&url),
        Some(AudioFormat::Mp3)
    );
    let url = reqwest::Url::parse("https://cf-media.sndcdn.com/track?signature=flac#flac").unwrap();
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
    let message = response_diagnostics_message("murglar flac probe", StatusCode::OK, Some(2048));
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
    for block in bytes.as_chunks_mut::<8>().0 {
        let plaintext = *block;
        for index in 0..8 {
            block[index] = plaintext[index] ^ previous[index];
        }
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
        previous = *block;
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
                ai_generated: false,
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

#[tokio::test]
async fn oversized_provider_bodies_are_rejected_by_the_shared_playback_decoder() {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let advertised = crate::provider_response::MAX_PROVIDER_RESPONSE_BYTES as u64 + 1;
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0, "playback fixture request was incomplete");
            request.extend_from_slice(&buffer[..read]);
        }
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {advertised}\r\nConnection: close\r\n\r\n"
        );
    });

    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(&endpoint)
        .send()
        .await
        .unwrap();
    let error = response_json(response, "deezer.media")
        .await
        .expect_err("an oversized provider body must be rejected");
    assert_eq!(
        error,
        "The deezer.media provider returned an invalid playback response"
    );
    server.join().unwrap();
}
