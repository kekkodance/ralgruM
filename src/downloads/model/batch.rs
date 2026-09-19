use super::*;

impl DownloadModel {
    pub(crate) fn start_batch<I>(
        &mut self,
        tracks: I,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        variant: DownloadVariant,
        cx: &mut Context<Self>,
    ) -> Vec<u64>
    where
        I: IntoIterator<Item = PlaybackTrack>,
    {
        // A new batch supersedes an unanswered batch prompt. Remove its
        // claimed jobs before adding the replacement so none of its queued
        // requests can become orphaned behind the new prompt.
        self.cancel_pending_batch(cx);

        let ids: Vec<u64> = tracks
            .into_iter()
            .map(|track| {
                self.next_id = self.next_id.wrapping_add(1);
                let id = self.next_id;
                self.jobs.push(DownloadJob::new(id, track, variant));
                self.requests
                    .insert(id, (deezer_arl.clone(), soundcloud_token.clone(), variant));
                id
            })
            .collect();
        if ids.is_empty() {
            return ids;
        }

        let downloads_dir = match self.downloads_dir(cx) {
            Ok(directory) => directory,
            Err(error) => {
                for id in &ids {
                    self.requests.remove(id);
                    if let Some(job) = self.job_mut(*id) {
                        job.status = DownloadStatus::Failed(error.clone());
                        job.unread = true;
                    }
                }
                self.notify_toast(ToastKind::Error, "Downloads failed", Some(error), cx);
                cx.notify();
                return ids;
            }
        };
        let target_paths = ids
            .iter()
            .filter_map(|id| {
                let job = self.job(*id)?;
                let extension = batch_download_extension(job.track.provider, variant)?;
                Some((
                    *id,
                    destination_with_extension(&job.track, extension, &downloads_dir),
                ))
            })
            .collect::<Vec<_>>();
        let batch_key = self.next_batch_key();
        self.begin_batch_tracking(&ids);
        self.pending_batch = Some(BatchNeedsConfirmation {
            ids: ids.clone(),
            existing: 0,
            existing_targets: Vec::new(),
            key: batch_key,
        });
        let task = self.runtime.spawn_blocking(move || {
            let inspection =
                inspect_batch_targets(target_paths.iter().map(|(_, path)| path.clone()));
            batch_conflict_targets(target_paths, inspection)
        });
        let entity = cx.entity();
        let batch_ids = ids.clone();
        cx.spawn(async move |_, cx| {
            let targets = task.await;
            entity.update(cx, |model, cx| {
                let Some(batch) = model
                    .pending_batch
                    .as_mut()
                    .filter(|batch| batch.key == batch_key)
                else {
                    return;
                };
                match targets {
                    Ok(targets) if !targets.is_empty() => {
                        batch.existing = targets.len();
                        batch.existing_targets = targets;
                        cx.emit(DownloadNotice::BatchConflict {
                            key: batch_key,
                            existing: batch.existing,
                        });
                    }
                    Ok(_) => {
                        model.pending_batch = None;
                        model.notify_toast(
                            ToastKind::Info,
                            "Downloads started",
                            Some(format!("Queued {} tracks.", batch_ids.len())),
                            cx,
                        );
                        model.start_next(cx);
                    }
                    Err(_) => {
                        model.pending_batch = None;
                        let error = "The batch destination check stopped unexpectedly".to_owned();
                        for id in &batch_ids {
                            model.requests.remove(id);
                            if let Some(job) = model.job_mut(*id) {
                                job.status = DownloadStatus::Failed(error.clone());
                                job.unread = true;
                            }
                            model.batch_ids.remove(id);
                        }
                        if model.batch_ids.is_empty() {
                            model.batch_mode = false;
                            model.batch_saved = 0;
                            model.batch_failed = 0;
                        }
                        model.notify_toast(ToastKind::Error, "Downloads failed", Some(error), cx);
                        model.start_next(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        ids
    }

    fn next_batch_key(&mut self) -> u64 {
        self.next_batch_key = self.next_batch_key.wrapping_add(1);
        self.next_batch_key
    }

    pub(super) fn begin_batch_tracking(&mut self, ids: &[u64]) {
        let continuing_batch = !self.batch_ids.is_empty();
        self.batch_mode = true;
        self.batch_ids.extend(ids.iter().copied());
        if !continuing_batch {
            self.batch_saved = 0;
            self.batch_failed = 0;
            self.batch_written_paths.clear();
        }
    }

    pub(crate) fn ignore_batch_conflict(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(batch) = self.take_pending_batch(key, cx) else {
            return;
        };
        for target in &batch.existing_targets {
            self.batch_policies
                .insert(target.id, BatchConflictPolicy::SkipExisting);
        }
        if !self.batch_ids.is_empty() {
            self.notify_toast(
                ToastKind::Info,
                "Downloads started",
                Some(format!("Queued {} tracks.", self.batch_ids.len())),
                cx,
            );
        }
        self.start_next(cx);
        cx.notify();
    }

    pub(crate) fn overwrite_batch_conflict(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(_batch) = self.take_pending_batch(key, cx) else {
            return;
        };
        for target in &_batch.existing_targets {
            self.batch_policies
                .insert(target.id, BatchConflictPolicy::OverwriteExisting);
        }
        if !self.batch_ids.is_empty() {
            self.notify_toast(
                ToastKind::Info,
                "Downloads started",
                Some(format!("Queued {} tracks.", self.batch_ids.len())),
                cx,
            );
        }
        self.start_next(cx);
        cx.notify();
    }

    fn take_pending_batch(
        &mut self,
        key: u64,
        cx: &mut Context<Self>,
    ) -> Option<BatchNeedsConfirmation> {
        if self
            .pending_batch
            .as_ref()
            .is_none_or(|batch| batch.key != key)
        {
            return None;
        }
        let batch = self.pending_batch.take()?;
        cx.emit(DownloadNotice::BatchConflictCleared { key });
        Some(batch)
    }

    pub(super) fn cancel_pending_batch(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = self.pending_batch.take() else {
            return;
        };
        let key = batch.key;
        cx.emit(DownloadNotice::BatchConflictCleared { key });
        let ids = batch
            .ids
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        super::super::remove_claimed_jobs(&mut self.jobs, &ids.iter().copied().collect::<Vec<_>>());
        self.requests.retain(|id, _| !ids.contains(id));
        self.batch_policies.retain(|id, _| !ids.contains(id));
        self.batch_ids.retain(|id| !ids.contains(id));
        if self.batch_ids.is_empty() {
            self.batch_mode = false;
            self.batch_saved = 0;
            self.batch_failed = 0;
        }
        self.start_next(cx);
        cx.notify();
    }
}

pub(super) fn batch_conflict_targets(
    targets: Vec<(u64, PathBuf)>,
    inspection: BatchTargetInspection,
) -> Vec<BatchExistingTarget> {
    let existing = inspection
        .existing
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let mut seen = std::collections::HashSet::new();
    targets
        .into_iter()
        .filter_map(|(id, path)| {
            let duplicate = !seen.insert(destination_identity(&path));
            (existing.contains(&path) || duplicate).then_some(BatchExistingTarget { id, path })
        })
        .collect()
}

pub(super) fn batch_download_extension(
    provider: PlaybackProvider,
    variant: DownloadVariant,
) -> Option<&'static str> {
    match (provider, variant) {
        (PlaybackProvider::SoundCloud, DownloadVariant::Standard)
        | (PlaybackProvider::Deezer, DownloadVariant::DeezerMp3_320)
        | (PlaybackProvider::Deezer, DownloadVariant::DeezerMp3_128) => Some("mp3"),
        (PlaybackProvider::Deezer, DownloadVariant::Best)
        | (PlaybackProvider::Deezer, DownloadVariant::DeezerFlac) => Some("flac"),
        _ => None,
    }
}

pub(super) fn destination_with_extension(
    track: &PlaybackTrack,
    extension: &str,
    directory: &Path,
) -> PathBuf {
    directory.join(download_filename(&track.artist, &track.title, extension))
}
