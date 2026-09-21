use super::*;
use crate::{
    account_session::SessionError,
    murglar_backend::{DeviceIdentityError, DeviceIdentityStatus},
    playback::PlaybackProvider,
    settings::AccountState,
};
use gpui::{AppContext, TestAppContext};
use std::time::Duration;
use tempfile::tempdir;

fn soundcloud_track() -> PlaybackTrack {
    PlaybackTrack {
        downloadable: true,
        progressive: false,
        provider: PlaybackProvider::SoundCloud,
        id: "1".into(),
        title: "Song".into(),
        artist: "Artist".into(),
        album: String::new(),
        album_id: String::new(),
        release_date: String::new(),
        artists: Vec::new(),
        artwork: String::new(),
        duration: Duration::from_secs(10),
        explicit: false,
        ai_generated: false,
        service_url: String::new(),
    }
}

fn quality_job() -> DownloadJob {
    let mut job = DownloadJob::new(1, soundcloud_track(), DownloadVariant::Standard);
    job.quality = Some(ResolvedQuality {
        format: "MP3".into(),
        source_size: Some(80_000),
        declared_bitrate: None,
    });
    job
}

fn error_account(cx: &mut gpui::TestAppContext) -> Entity<AccountState> {
    cx.update(|cx| {
        cx.new(|_| {
            AccountState::new(
                DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                Err(SessionError::ConfigDirectoryUnavailable),
            )
        })
    })
}

#[test]
fn notice_quality_uses_the_final_file_size_over_the_source_hint() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("song.mp3");
    std::fs::write(&path, vec![0; 160_000]).unwrap();

    assert_eq!(
        quality_label(
            &quality_job(),
            std::fs::metadata(&path).ok().map(|metadata| metadata.len())
        ),
        "MP3 128kbps"
    );
    assert_eq!(quality_label(&quality_job(), None), "MP3 64kbps");
}

#[test]
fn exact_deezer_variants_use_their_collision_extensions() {
    assert_eq!(
        batch_download_extension(PlaybackProvider::Deezer, DownloadVariant::DeezerFlac),
        Some("flac")
    );
    assert_eq!(
        batch_download_extension(PlaybackProvider::Deezer, DownloadVariant::DeezerMp3_320),
        Some("mp3")
    );
    assert_eq!(
        batch_download_extension(PlaybackProvider::Deezer, DownloadVariant::DeezerMp3_128),
        Some("mp3")
    );
    assert_eq!(
        batch_download_extension(PlaybackProvider::Deezer, DownloadVariant::Best),
        Some("flac")
    );
    assert_eq!(
        batch_download_extension(PlaybackProvider::SoundCloud, DownloadVariant::Standard),
        Some("mp3")
    );
}

#[test]
fn batch_conflicts_include_later_jobs_with_the_same_destination() {
    let path = PathBuf::from("same.mp3");
    let other = PathBuf::from("other.flac");
    let targets = batch_conflict_targets(
        vec![(1, path.clone()), (2, path.clone()), (3, other.clone())],
        BatchTargetInspection {
            existing: vec![other.clone()],
            duplicates: vec![path.clone()],
        },
    );
    assert_eq!(
        targets,
        vec![
            BatchExistingTarget { id: 2, path },
            BatchExistingTarget { id: 3, path: other },
        ]
    );
}

#[gpui::test]
fn a_later_batch_job_cannot_overwrite_an_earlier_completed_destination(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("same.mp3");
    std::fs::write(&path, b"first job").unwrap();
    let later_path = if cfg!(windows) {
        directory.path().join("SAME.MP3")
    } else {
        path.clone()
    };
    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        model.jobs.push(DownloadJob::new(
            2,
            soundcloud_track(),
            DownloadVariant::Standard,
        ));
        model.batch_ids.extend([1, 2]);
        model
            .batch_policies
            .insert(2, BatchConflictPolicy::OverwriteExisting);
        model.active = Some((0, 2, CancellationToken::new()));
        model
    });

    model.update(cx, |model, cx| {
        model.record_outcome(1, &DownloadStatus::Completed(path.clone()), cx);
        assert!(
            model
                .batch_written_paths
                .contains(&destination_identity(&path))
        );
        model.resolved_destination(2, 0, later_path, true, cx);
        assert!(matches!(model.jobs[0].status, DownloadStatus::Skipped(_)));
        assert!(model.batch_written_paths.is_empty());
    });
    assert_eq!(std::fs::read(path).unwrap(), b"first job");
}

#[tokio::test]
async fn cancelled_job_cleanup_preserves_a_newer_download_to_the_same_destination() {
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("track.mp3");
    let old_part = part_path(&destination, 1, 4);
    let new_part = part_path(&destination, 2, 5);
    fs::write(&old_part, b"cancelled transfer").await.unwrap();
    fs::write(&new_part, b"new completed transfer")
        .await
        .unwrap();

    fs::remove_file(&old_part).await.unwrap();
    super::super::finalize::finalize_download(&new_part, &destination, false)
        .await
        .unwrap();

    assert_eq!(
        fs::read(&destination).await.unwrap(),
        b"new completed transfer"
    );
    assert!(!new_part.exists());
}

#[test]
fn partial_paths_are_unique_to_the_job_generation() {
    let destination = PathBuf::from("downloads/Artist - Song.mp3");
    assert_ne!(part_path(&destination, 1, 4), part_path(&destination, 2, 4));
    assert_ne!(part_path(&destination, 1, 4), part_path(&destination, 1, 5));
    assert!(
        part_path(&destination, 1, 4)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(".part")
    );
}

#[gpui::test]
fn later_no_conflict_batch_does_not_replace_an_earlier_job_policy(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let account = cx.new(|_| {
            AccountState::new(
                DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                Err(SessionError::ConfigDirectoryUnavailable),
            )
        });
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        model.batch_ids.insert(7);
        model
            .batch_policies
            .insert(7, BatchConflictPolicy::OverwriteExisting);

        model.begin_batch_tracking(&[8]);

        assert_eq!(
            model.batch_conflict_policy_for(7),
            Some(BatchConflictPolicy::OverwriteExisting)
        );
        assert_eq!(model.batch_conflict_policy_for(8), None);
    });
}

#[gpui::test]
fn retry_requeues_a_failed_job_without_creating_a_duplicate(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let account = cx.new(|_| {
            AccountState::new(
                DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                Err(SessionError::ConfigDirectoryUnavailable),
            )
        });
        let model = cx.new(|_| {
            let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
            let mut job = quality_job();
            job.status = DownloadStatus::Failed("network error".into());
            job.unread = true;
            model.jobs.push(job);
            model.next_id = 1;
            model.active = Some((0, 99, CancellationToken::new()));
            model
        });

        model.update(cx, |model, cx| {
            model.retry(1, cx);
            assert_eq!(model.jobs.len(), 1);
            assert_eq!(model.jobs[0].id, 1);
            assert_eq!(model.jobs[0].status, DownloadStatus::Queued);
            assert!(!model.jobs[0].unread);
            assert!(model.jobs[0].quality.is_none());
            assert_eq!(model.jobs[0].variant, DownloadVariant::Standard);
            let request = model
                .requests
                .get(&1)
                .expect("retry should restore the download request");
            assert!(request.0.is_none());
            assert!(request.1.is_none());
            assert_eq!(request.2, DownloadVariant::Standard);
        });
    });
}

#[gpui::test]
fn retry_ignores_jobs_that_are_not_failed(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let account = cx.new(|_| {
            AccountState::new(
                DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable),
                Err(SessionError::ConfigDirectoryUnavailable),
            )
        });
        let model = cx.new(|_| {
            let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
            let mut job = quality_job();
            job.status = DownloadStatus::Cancelled;
            job.unread = true;
            model.jobs.push(job);
            model
        });

        model.update(cx, |model, cx| {
            model.retry(1, cx);
            assert_eq!(model.jobs.len(), 1);
            assert_eq!(model.jobs[0].status, DownloadStatus::Cancelled);
            assert!(model.jobs[0].unread);
            assert!(model.jobs[0].quality.is_some());
            assert!(model.requests.is_empty());
        });
    });
}

#[gpui::test]
fn batch_without_live_settings_fails_instead_of_guessing_a_directory(cx: &mut TestAppContext) {
    let account = error_account(cx);
    let model = cx.new(|_| DownloadModel::new(account, Arc::new(Runtime::new().unwrap())));

    let ids = model.update(cx, |model, cx| {
        model.start_batch(
            [soundcloud_track()],
            None,
            None,
            DownloadVariant::Standard,
            cx,
        )
    });

    model.update(cx, |model, _| {
        assert_eq!(ids.len(), 1);
        let job = &model.jobs[0];
        let DownloadStatus::Failed(error) = &job.status else {
            panic!("expected a visible failure, got {:?}", job.status);
        };
        assert!(error.contains("downloads directory"));
        assert!(job.unread);
        assert!(model.requests.is_empty());
        assert!(model.pending_batch.is_none());
        assert!(!model.batch_mode);
    });
}

#[gpui::test]
fn resolved_download_without_live_settings_fails_instead_of_saving(cx: &mut TestAppContext) {
    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Resolving;
        model.jobs.push(job);
        model.next_id = 1;
        model.active = Some((0, 1, CancellationToken::new()));
        model
    });

    model.update(cx, |model, cx| {
        // The missing settings guard fires before the transfer result
        // is interpreted, so not even a resolved stream can fall back
        // to a default directory.
        model.resolved(
            1,
            0,
            soundcloud_track(),
            Err("the resolved stream expired".into()),
            cx,
        );
        let job = &model.jobs[0];
        let DownloadStatus::Failed(error) = &job.status else {
            panic!("expected a visible failure, got {:?}", job.status);
        };
        assert!(error.contains("downloads directory"));
        assert!(job.unread);
        assert!(model.active.is_none());
        assert!(model.requests.is_empty());
    });
}

#[gpui::test]
fn file_stats_are_probed_once_and_cached_in_the_job(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("song.mp3");
    std::fs::write(&path, vec![0; 160_000]).unwrap();

    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Completed(path.clone());
        model.jobs.push(job);
        model.next_id = 1;
        model
    });

    model.update(cx, |model, cx| {
        model.refresh_file_stats(cx);
        // A refresh while a probe is still in flight schedules no
        // second probe.
        model.refresh_file_stats(cx);
        assert_eq!(model.file_stat_probes.len(), 1);
    });
    cx.run_until_parked();

    model.update(cx, |model, cx| {
        let job = &model.jobs[0];
        let stat = job.file.as_ref().expect("the probe should have landed");
        assert!(stat.exists);
        assert_eq!(stat.size, Some(160_000));
        assert!(stat.modified.is_some());
        assert_eq!(job.file_size(), Some(160_000));
        assert!(model.file_stat_probes.is_empty());

        // A cached stat is reused; later refreshes do not re-probe.
        model.refresh_file_stats(cx);
        assert!(model.file_stat_probes.is_empty());
    });
}

#[gpui::test]
fn finishing_a_download_invalidates_and_reprobes_the_cached_stat(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("song.mp3");
    std::fs::write(&path, vec![0; 160_000]).unwrap();

    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Downloading {
            downloaded: 8,
            total: Some(8),
        };
        // A stale stat from an earlier conflict about the same file.
        job.file = Some(DownloadFileStat::default());
        model.jobs.push(job);
        model.next_id = 1;
        model.active = Some((0, 1, CancellationToken::new()));
        model
    });

    model.update(cx, |model, cx| {
        model.finish(1, 0, DownloadStatus::Completed(path), cx);
        assert!(model.jobs[0].file.is_none());
    });
    cx.run_until_parked();

    model.update(cx, |model, _| {
        let stat = model.jobs[0].file.as_ref().expect("a fresh probe landed");
        assert_eq!(stat.size, Some(160_000));
        assert_eq!(model.jobs[0].file_size(), Some(160_000));
    });
}

#[gpui::test]
fn file_stat_probe_results_are_discarded_when_the_destination_changes(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let first = directory.path().join("song.mp3");
    std::fs::write(&first, vec![0; 160_000]).unwrap();

    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Completed(first.clone());
        model.jobs.push(job);
        model.next_id = 1;
        model
    });

    model.update(cx, |model, cx| {
        model.refresh_file_stats(cx);
        assert_eq!(model.file_stat_probes.len(), 1);
        // The destination changes while the probe is still in flight.
        model.jobs[0].status = DownloadStatus::Completed(directory.path().join("renamed.flac"));
    });
    cx.run_until_parked();

    model.update(cx, |model, _| {
        // The probe answered the old destination, so its result is
        // dropped instead of being cached for the new one.
        assert!(model.jobs[0].file.is_none());
        assert!(model.file_stat_probes.is_empty());
    });
}

#[gpui::test]
fn an_invalidated_stat_is_reprobed_for_the_new_destination(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let first = directory.path().join("song.mp3");
    let second = directory.path().join("song.flac");
    std::fs::write(&first, vec![0; 160_000]).unwrap();
    std::fs::write(&second, vec![0; 80_000]).unwrap();

    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Completed(first.clone());
        model.jobs.push(job);
        model.next_id = 1;
        model
    });

    // Start a probe for the first destination, then change the
    // destination and invalidate before it can land.
    model.update(cx, |model, cx| model.refresh_file_stats(cx));
    model.update(cx, |model, cx| {
        model.jobs[0].status = DownloadStatus::Completed(second.clone());
        model.invalidate_file_stat(1, cx);
    });
    cx.run_until_parked();

    model.update(cx, |model, _| {
        // Both probes land, but only the one that still owns the
        // job's expectation may install its result.
        let stat = model.jobs[0]
            .file
            .as_ref()
            .expect("the fresh probe should have landed");
        assert_eq!(stat.size, Some(80_000));
        assert!(model.file_stat_probes.is_empty());
    });
}

#[gpui::test]
fn a_superseded_probe_cannot_install_its_result(cx: &mut TestAppContext) {
    let directory = tempdir().unwrap();
    let path = directory.path().join("song.mp3");
    std::fs::write(&path, vec![0; 160_000]).unwrap();

    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Completed(path.clone());
        model.jobs.push(job);
        model.next_id = 1;
        model
    });

    model.update(cx, |model, cx| {
        // A newer probe owns the job's expectation when an older
        // probe for the same destination lands.
        model.file_stat_probes.insert(1, (path.clone(), 2));
        model.retire_file_stat_probe(1, path.clone(), 1, DownloadFileStat::probe(&path), cx);
        assert!(model.jobs[0].file.is_none());
        assert_eq!(model.file_stat_probes.get(&1), Some(&(path.clone(), 2)));

        // The newer probe still installs normally.
        model.retire_file_stat_probe(1, path.clone(), 2, DownloadFileStat::probe(&path), cx);
        assert_eq!(
            model.jobs[0].file.as_ref().map(|stat| stat.size),
            Some(Some(160_000))
        );
        assert!(model.file_stat_probes.is_empty());
    });
}

#[tokio::test]
async fn a_burst_producer_leaves_exactly_one_pending_progress_sample() {
    let (sender, mut receiver) = tokio::sync::watch::channel((0_u64, None::<u64>));

    // The receiver is blocked for the whole burst: it never polls
    // while the publisher runs, which is the starvation window the
    // latest-value channel has to survive without buffering.
    let total = 10_000 * 64 * 1024;
    for tick in 1..=10_000 {
        sender.send((tick * 64 * 1024, Some(total))).unwrap();
    }

    // Every send succeeded without backpressure, and a single
    // observation drains the entire burst: the receiver sees only the
    // final sample and nothing remains pending afterwards, so the
    // channel holds at most one entry no matter how many sends
    // preceded it.
    receiver.changed().await.unwrap();
    assert_eq!(*receiver.borrow_and_update(), (total, Some(total)));
    assert!(!receiver.has_changed().unwrap());

    // Closing the producer wakes the receiver once more with an
    // error, which is how the pump exits; the terminal state arrives
    // on the transfer result path instead.
    drop(sender);
    assert!(receiver.changed().await.is_err());
}

#[gpui::test]
fn progress_pump_collapses_bursts_and_converges_to_the_final_tick(cx: &mut TestAppContext) {
    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Downloading {
            downloaded: 0,
            total: None,
        };
        model.jobs.push(job);
        model.next_id = 1;
        model.active = Some((0, 1, CancellationToken::new()));
        model
    });

    let (sender, receiver) = tokio::sync::watch::channel((0_u64, None::<u64>));
    model.update(cx, |_, cx| spawn_progress_pump(cx, 1, 0, receiver));

    let total = 2_000 * 64 * 1024;
    for tick in 1..2_000 {
        sender.send((tick * 64 * 1024, Some(total))).unwrap();
    }
    // The terminal progress tick must survive the burst.
    sender.send((total, Some(total))).unwrap();
    drop(sender);
    cx.run_until_parked();

    model.update(cx, |model, _| {
        assert_eq!(
            model.jobs[0].status,
            DownloadStatus::Downloading {
                downloaded: total,
                total: Some(total),
            },
        );
    });
}

#[gpui::test]
fn progress_pump_applies_the_newest_tick_of_a_throttled_burst(cx: &mut TestAppContext) {
    let account = error_account(cx);
    let model = cx.new(|_| {
        let mut model = DownloadModel::new(account, Arc::new(Runtime::new().unwrap()));
        let mut job = quality_job();
        job.status = DownloadStatus::Downloading {
            downloaded: 0,
            total: None,
        };
        model.jobs.push(job);
        model.next_id = 1;
        model.active = Some((0, 1, CancellationToken::new()));
        model
    });

    let (sender, receiver) = tokio::sync::watch::channel((0_u64, None::<u64>));
    model.update(cx, |_, cx| spawn_progress_pump(cx, 1, 0, receiver));

    let total = 2_000 * 64 * 1024;
    let newest = 1_999 * 64 * 1024;
    for tick in 1..2_000 {
        sender.send((tick * 64 * 1024, Some(total))).unwrap();
    }
    drop(sender);
    cx.run_until_parked();

    model.update(cx, |model, _| {
        // The whole burst collapses into one update, so the newest
        // byte count is applied instead of the first throttled one.
        assert_eq!(
            model.jobs[0].status,
            DownloadStatus::Downloading {
                downloaded: newest,
                total: Some(total),
            },
        );
    });
}
