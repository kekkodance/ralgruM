use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{Context, Entity, EventEmitter, Task};
use tokio::{fs, io::AsyncSeekExt, runtime::Runtime};
use tokio_util::sync::CancellationToken;

use super::capability::{CapabilityCache, CapabilityKey, CapabilityProbeRegistry, CapabilityState};
use super::{
    BatchConflictPolicy, BatchExistingTarget, BatchNeedsConfirmation, DownloadJob, DownloadStatus,
    finalize::{FinalizeError, finalize_download},
    inspect_batch_targets,
    quality::ResolvedQuality,
    sanitize_filename,
    state::{BatchTargetAction, batch_outcome, batch_target_action, current_conflict},
};
use crate::{
    navigation_state::{AppSettings, SettingsStore},
    playback::{
        AudioCache, CACHED_DOWNLOAD_INVALID, CachedDownload, DownloadVariant, PlaybackProvider,
        PlaybackTrack, ProgressCallback, ProgressUpdate, ResolvedSource, StreamResolver,
    },
    search::{DeezerArl, SoundCloudToken},
    settings::AccountState,
    toast::{ToastKind, ToastStack},
};

pub(crate) struct DownloadModel {
    pub(crate) jobs: Vec<DownloadJob>,
    pub(crate) platform_status: Option<String>,
    account: Entity<AccountState>,
    runtime: Arc<Runtime>,
    resolver: Result<StreamResolver, String>,
    active: Option<(u64, u64, CancellationToken)>,
    cache: Option<AudioCache>,
    sources: HashMap<u64, DownloadSource>,
    pending_parts: HashMap<u64, PathBuf>,
    next_id: u64,
    requests: HashMap<u64, (Option<DeezerArl>, Option<SoundCloudToken>, DownloadVariant)>,
    generation: u64,
    pub(crate) pending_batch: Option<BatchNeedsConfirmation>,
    next_batch_key: u64,
    batch_policies: HashMap<u64, BatchConflictPolicy>,
    toasts: Option<gpui::WeakEntity<ToastStack>>,
    batch_mode: bool,
    batch_ids: std::collections::HashSet<u64>,
    batch_saved: usize,
    batch_failed: usize,
    capabilities: CapabilityCache,
    capability_cancellations: CapabilityProbeRegistry,
    capability_tasks: HashMap<CapabilityKey, Task<()>>,
}

/// Capability notification for an open download row. `None` invalidates an
/// old account scope; a `Failed` state is a retryable probe failure. The key
/// contains only provider, track identity, and account scope. It never carries
/// credentials or resolved media URLs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CapabilityChanged {
    pub(crate) key: CapabilityKey,
    pub(crate) state: Option<CapabilityState>,
}

impl EventEmitter<CapabilityChanged> for DownloadModel {}

/// User-facing download lifecycle notifications. The shell subscribes to
/// these events so completion and conflict notices can be shown outside the
/// downloads page.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DownloadNotice {
    Conflict {
        id: u64,
        title: String,
        quality: String,
    },
    Completed {
        title: String,
        quality: String,
    },
    ConflictCleared {
        id: u64,
    },
    BatchConflict {
        key: u64,
        existing: usize,
    },
    BatchConflictCleared {
        key: u64,
    },
    BatchCompleted {
        saved: usize,
    },
}

impl EventEmitter<DownloadNotice> for DownloadModel {}

enum DownloadTransferError {
    DestinationExists,
    Cancelled,
    Failed(String),
}

struct CachedDownloadFallback {
    resolver: Result<StreamResolver, String>,
    track: PlaybackTrack,
    deezer_arl: Option<DeezerArl>,
    soundcloud_token: Option<SoundCloudToken>,
    murglar: Option<crate::playback::MediaCredentials>,
    variant: DownloadVariant,
}

enum DownloadSource {
    Resolved(ResolvedSource),
    Cached {
        cached: CachedDownload,
        fallback: CachedDownloadFallback,
    },
}

impl DownloadModel {
    pub(crate) fn new(account: Entity<AccountState>, runtime: Arc<Runtime>) -> Self {
        Self {
            jobs: Vec::new(),
            platform_status: None,
            account,
            runtime,
            resolver: StreamResolver::new(),
            active: None,
            cache: None,
            sources: HashMap::new(),
            pending_parts: HashMap::new(),
            next_id: 0,
            requests: HashMap::new(),
            generation: 0,
            pending_batch: None,
            next_batch_key: 0,
            batch_policies: HashMap::new(),
            toasts: None,
            batch_mode: false,
            batch_ids: std::collections::HashSet::new(),
            batch_saved: 0,
            batch_failed: 0,
            capabilities: CapabilityCache::default(),
            capability_cancellations: CapabilityProbeRegistry::default(),
            capability_tasks: HashMap::new(),
        }
    }

    pub(crate) fn with_toasts(mut self, toasts: gpui::WeakEntity<ToastStack>) -> Self {
        self.toasts = Some(toasts);
        self
    }

    pub(crate) fn with_cache(mut self, cache: AudioCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Begin a lazy, account-scoped capability probe for one track. The
    /// context-menu body observes the completion event, so an already-open
    /// submenu can replace its checking row without being rebuilt.
    pub(crate) fn ensure_capabilities(
        &mut self,
        track: PlaybackTrack,
        cx: &mut Context<Self>,
    ) -> CapabilityState {
        // Keep the task handles alive while a probe is running. Completed
        // handles are retired on the next menu request, rather than from
        // inside their own callback.
        self.capability_tasks.retain(|_, task| !task.is_ready());
        let account_scope = self.account.read(cx).credential_generation();
        let key = CapabilityKey::new(&track, account_scope);
        if let Some(state) = self.capabilities.state(&key) {
            return state;
        }
        self.capabilities.begin(key.clone());
        let Some(resolver) = self.resolver.clone().ok() else {
            self.capabilities.finish(key, Vec::new());
            return CapabilityState::Ready(Vec::new());
        };
        let (deezer_arl, soundcloud_token, murglar) = {
            let account = self.account.read(cx);
            (
                account.deezer_arl(),
                account.soundcloud_token(),
                account.murglar_media_credentials(),
            )
        };
        let probe = self.capability_cancellations.begin(key.clone());
        let cancellation = probe.token.clone();
        let probe_id = probe.id;
        let task = self.runtime.spawn({
            let track = track.clone();
            async move {
                resolver
                    .probe_download_capabilities(
                        &track,
                        deezer_arl,
                        soundcloud_token,
                        murglar,
                        cancellation,
                    )
                    .await
            }
        });
        let entity = cx.entity().clone();
        let completion_key = key.clone();
        let task = cx.spawn(async move |_, cx| {
            let result = task
                .await
                .map_err(|_| "Download capability probe stopped unexpectedly".to_string())
                .and_then(|result| result);
            let _ = entity.update(cx, |model, cx| {
                let current_probe = model
                    .capability_cancellations
                    .finish(&completion_key, probe_id);
                if current_probe
                    && model.account.read(cx).credential_generation()
                        == completion_key.account_scope
                {
                    let state = match result {
                        Ok(choices) => {
                            model
                                .capabilities
                                .finish(completion_key.clone(), choices.clone());
                            Some(CapabilityState::Ready(choices))
                        }
                        Err(_) => {
                            model.capabilities.fail(completion_key.clone());
                            Some(CapabilityState::Failed)
                        }
                    };
                    cx.emit(CapabilityChanged {
                        key: completion_key.clone(),
                        state,
                    });
                    cx.notify();
                }
            });
        });
        self.capability_tasks.insert(key, task);
        CapabilityState::Checking
    }

    fn notify_toast(
        &self,
        kind: ToastKind,
        title: &str,
        description: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(toasts) = &self.toasts else {
            return;
        };
        toasts
            .update(cx, |stack, cx| {
                stack.push(kind, title.to_owned(), description.map(Into::into), cx)
            })
            .ok();
    }

    pub(crate) fn start(
        &mut self,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        variant: DownloadVariant,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        self.jobs.push(DownloadJob::new(id, track, variant));
        self.requests
            .insert(id, (deezer_arl, soundcloud_token, variant));
        self.start_next(cx);
        cx.notify();
        id
    }

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

        let batch_key = self.next_batch_key();
        let extension = ids.first().and_then(|id| {
            self.job(*id)
                .and_then(|job| batch_download_extension(job.track.provider, variant))
        });
        let existing_targets = match extension {
            Some(extension) => {
                let settings = SettingsStore::load_current_user()
                    .map(|store| store.settings().clone())
                    .unwrap_or_else(|_| AppSettings::default());
                let downloads_dir = settings.effective_downloads_dir();
                let target_paths = ids
                    .iter()
                    .filter_map(|id| {
                        self.job(*id)
                            .map(|job| destination_guess(&job.track, extension, &downloads_dir))
                    })
                    .collect::<Vec<_>>();
                let inspection = inspect_batch_targets(target_paths.clone());
                let existing_paths = inspection
                    .existing
                    .into_iter()
                    .collect::<std::collections::HashSet<_>>();
                ids.iter()
                    .zip(target_paths)
                    .filter(|(_, path)| existing_paths.contains(path))
                    .map(|(id, path)| BatchExistingTarget { id: *id, path })
                    .collect::<Vec<_>>()
            }
            None => Vec::new(),
        };
        self.begin_batch_tracking(&ids);
        if !existing_targets.is_empty() {
            self.pending_batch = Some(BatchNeedsConfirmation {
                ids: ids.clone(),
                existing: existing_targets.len(),
                existing_targets,
                key: batch_key,
            });
            cx.emit(DownloadNotice::BatchConflict {
                key: batch_key,
                existing: self
                    .pending_batch
                    .as_ref()
                    .map_or(0, |batch| batch.existing),
            });
        } else {
            self.notify_toast(
                ToastKind::Info,
                "Downloads Started",
                Some(format!("Queued {} tracks.", ids.len())),
                cx,
            );
            self.start_next(cx);
        }
        cx.notify();
        ids
    }

    fn next_batch_key(&mut self) -> u64 {
        self.next_batch_key = self.next_batch_key.wrapping_add(1);
        self.next_batch_key
    }

    fn begin_batch_tracking(&mut self, ids: &[u64]) {
        let continuing_batch = !self.batch_ids.is_empty();
        self.batch_mode = true;
        self.batch_ids.extend(ids.iter().copied());
        if !continuing_batch {
            self.batch_saved = 0;
            self.batch_failed = 0;
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
        let mut skipped = Vec::new();
        for target in &batch.existing_targets {
            let can_skip = target.path.exists()
                && self
                    .job(target.id)
                    .is_some_and(|job| matches!(job.status, DownloadStatus::Queued));
            if can_skip {
                self.requests.remove(&target.id);
                if let Some(job) = self.job_mut(target.id) {
                    job.status = DownloadStatus::Skipped(target.path.clone());
                    job.unread = true;
                }
                skipped.push(target.id);
            }
        }
        for id in skipped {
            if let Some(status) = self.job(id).map(|job| job.status.clone()) {
                self.record_outcome(id, &status, cx);
            }
        }
        if !self.batch_ids.is_empty() {
            self.notify_toast(
                ToastKind::Info,
                "Downloads Started",
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
                "Downloads Started",
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

    fn cancel_pending_batch(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = self.pending_batch.take() else {
            return;
        };
        let key = batch.key;
        cx.emit(DownloadNotice::BatchConflictCleared { key });
        let ids = batch
            .ids
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        super::remove_claimed_jobs(&mut self.jobs, &ids.iter().copied().collect::<Vec<_>>());
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

    fn start_next(&mut self, cx: &mut Context<Self>) {
        if self.pending_batch.is_some() {
            return;
        }
        if self.active.is_some() {
            return;
        }
        let Some(id) = self
            .jobs
            .iter()
            .find(|job| matches!(job.status, DownloadStatus::Queued))
            .map(|job| job.id)
        else {
            return;
        };
        let Some((deezer_arl, soundcloud_token, variant)) = self.requests.remove(&id) else {
            return;
        };
        let Some(track) = self.job(id).map(|job| job.track.clone()) else {
            return;
        };
        self.job_mut(id).unwrap().status = DownloadStatus::Resolving;
        let murglar = self.account.read(cx).murglar_media_credentials();
        let generation = self.generation;
        let cancellation = CancellationToken::new();
        self.active = Some((generation, id, cancellation.clone()));
        let resolve_track = track.clone();
        let resolver = self.resolver.clone();
        let cache = self.cache.clone();
        let task = self.runtime.spawn(async move {
            if let Some(cache) = cache.as_ref()
                && let Some(cached) = cache
                    .complete_download_variant(&resolve_track, variant, &cancellation)
                    .await
            {
                return Ok(DownloadSource::Cached {
                    cached,
                    fallback: CachedDownloadFallback {
                        resolver: resolver.clone(),
                        track: resolve_track.clone(),
                        deezer_arl: deezer_arl.clone(),
                        soundcloud_token: soundcloud_token.clone(),
                        murglar: murglar.clone(),
                        variant,
                    },
                });
            }
            let resolver = resolver?;
            resolver
                .resolve_download_source(
                    &resolve_track,
                    deezer_arl,
                    soundcloud_token,
                    murglar,
                    variant,
                    cancellation,
                )
                .await
                .map(DownloadSource::Resolved)
        });
        let entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The download worker stopped unexpectedly".into()));
            entity.update(cx, |model, cx| {
                model.resolved(id, generation, track, result, cx)
            });
        })
        .detach();
    }

    fn resolved(
        &mut self,
        id: u64,
        generation: u64,
        track: PlaybackTrack,
        result: Result<DownloadSource, String>,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts(id, generation) {
            return;
        }
        let source = match result {
            Ok(source) => source,
            Err(error)
                if error == "Download request cancelled"
                    || error == "Playback request cancelled" =>
            {
                self.finish(id, generation, DownloadStatus::Cancelled, cx);
                return;
            }
            Err(error) => {
                self.finish(id, generation, DownloadStatus::Failed(error), cx);
                return;
            }
        };
        let settings = SettingsStore::load_current_user()
            .map(|store| store.settings().clone())
            .unwrap_or_else(|_| AppSettings::default());
        let (extension, format_name, source_size, declared_bitrate) = match &source {
            DownloadSource::Resolved(source) => (
                source.format.extension().to_owned(),
                source.format_name.clone(),
                source.size(),
                source.declared_bitrate,
            ),
            DownloadSource::Cached { cached, .. } => (
                cached.extension.clone(),
                cached.format_name.clone(),
                cached.total,
                None,
            ),
        };
        let path =
            destination_with_extension(&track, &extension, &settings.effective_downloads_dir());
        self.job_mut(id).unwrap().quality = Some(ResolvedQuality {
            format: format_name,
            source_size: (source_size > 0).then_some(source_size),
            declared_bitrate,
        });
        self.sources.insert(id, source);
        if path.exists() {
            if self.batch_ids.contains(&id) {
                match batch_target_action(self.batch_conflict_policy_for(id), true) {
                    BatchTargetAction::SkipExisting => {
                        self.sources.remove(&id);
                        self.finish(id, generation, DownloadStatus::Skipped(path), cx);
                    }
                    BatchTargetAction::OverwriteExisting => {
                        self.download_to(id, generation, path, true, cx);
                    }
                    BatchTargetAction::DownloadNew | BatchTargetAction::NeedsConfirmation => {
                        self.job_mut(id).unwrap().status =
                            DownloadStatus::NeedsConfirmation { path: path.clone() };
                        self.emit_conflict(id, cx);
                        cx.notify();
                    }
                }
            } else {
                self.job_mut(id).unwrap().status =
                    DownloadStatus::NeedsConfirmation { path: path.clone() };
                self.emit_conflict(id, cx);
                cx.notify();
            }
        } else {
            if self.batch_ids.contains(&id) {
                debug_assert_eq!(
                    batch_target_action(self.batch_conflict_policy_for(id), false),
                    BatchTargetAction::DownloadNew
                );
            }
            self.download_to(id, generation, path, false, cx);
        }
    }

    pub(crate) fn clear_terminal(&mut self, cx: &mut Context<Self>) {
        self.jobs.retain(|job| !job.is_terminal());
        cx.notify();
    }

    pub(crate) fn unread_count(&self) -> usize {
        self.jobs.iter().filter(|job| job.unread).count()
    }

    pub(crate) fn mark_read(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for job in &mut self.jobs {
            changed |= job.unread;
            job.unread = false;
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn set_platform_status(
        &mut self,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.platform_status = result.err();
        cx.notify();
    }

    pub(crate) fn cancel(&mut self, id: u64, cx: &mut Context<Self>) {
        if matches!(
            self.job(id).map(|job| &job.status),
            Some(DownloadStatus::Queued)
        ) {
            self.requests.remove(&id);
            let job = self.job_mut(id).unwrap();
            job.status = DownloadStatus::Cancelled;
            job.unread = true;
            let status = DownloadStatus::Cancelled;
            self.record_outcome(id, &status, cx);
        } else if let Some((_, active_id, token)) = &self.active
            && *active_id == id
        {
            let conflict_invalidated = self
                .job(id)
                .is_some_and(|job| matches!(job.status, DownloadStatus::NeedsConfirmation { .. }));
            token.cancel();
            if let Some(job) = self.job_mut(id) {
                job.status = DownloadStatus::Cancelled;
                job.unread = true;
            }
            self.active = None;
            self.generation = self.generation.wrapping_add(1);
            self.sources.remove(&id);
            self.remove_pending_part(id);
            if conflict_invalidated {
                cx.emit(DownloadNotice::ConflictCleared { id });
            }
            let status = DownloadStatus::Cancelled;
            self.record_outcome(id, &status, cx);
            self.start_next(cx);
        }
        cx.notify();
    }

    pub(crate) fn retry(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(job) = self.job(id) else {
            return;
        };
        if !matches!(job.status, DownloadStatus::Failed(_)) {
            return;
        }
        let track = job.track.clone();
        let variant = job.variant;
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        let job = self.job_mut(id).unwrap();
        job.track = track;
        job.status = DownloadStatus::Queued;
        job.unread = false;
        job.quality = None;
        self.requests
            .insert(id, (deezer_arl, soundcloud_token, variant));
        self.start_next(cx);
        cx.notify();
    }

    /// Ignore the current collision and keep the existing file. Stale or
    /// queued IDs are intentionally ignored so an old toast cannot advance a
    /// newer active download.
    pub(crate) fn ignore_conflict(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((generation, path)) = self.conflict_target(id) else {
            return;
        };
        self.sources.remove(&id);
        self.remove_pending_part(id);
        cx.emit(DownloadNotice::ConflictCleared { id });
        self.finish(id, generation, DownloadStatus::Skipped(path), cx);
        cx.notify();
    }

    /// Retry the current collision into the original destination. The source
    /// remains owned by the model until this method validates the active
    /// conflict, and the finalizer only replaces the old file after the new
    /// partial download is complete.
    pub(crate) fn overwrite_conflict(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some((generation, path)) = self.conflict_target(id) else {
            return;
        };
        if let Some(part) = self.pending_parts.get(&id).cloned() {
            let Some(cancellation) = self
                .active
                .as_ref()
                .map(|(_, _, cancellation)| cancellation.clone())
            else {
                return;
            };
            if let Some(job) = self.job_mut(id) {
                job.status = DownloadStatus::Downloading {
                    downloaded: 0,
                    total: None,
                };
            }
            cx.emit(DownloadNotice::ConflictCleared { id });
            self.finalize_pending_part(id, generation, path, part, cancellation, cx);
            cx.notify();
            return;
        }
        if !self.sources.contains_key(&id) {
            return;
        }
        if let Some(job) = self.job_mut(id) {
            job.status = DownloadStatus::Downloading {
                downloaded: 0,
                total: None,
            };
        }
        cx.emit(DownloadNotice::ConflictCleared { id });
        self.download_to(id, generation, path, true, cx);
        cx.notify();
    }

    pub(crate) fn account_scope_changed(&mut self, cx: &mut Context<Self>) {
        let invalidated = self.capabilities.keys().collect::<Vec<_>>();
        let batch_conflict_key = self.pending_batch.as_ref().map(|batch| batch.key);
        let conflict_id = self.active.as_ref().and_then(|(_, id, _)| {
            self.job(*id)
                .is_some_and(|job| matches!(job.status, DownloadStatus::NeedsConfirmation { .. }))
                .then_some(*id)
        });
        if let Some((_, id, token)) = self.active.take() {
            token.cancel();
            if let Some(job) = self.job_mut(id) {
                job.status = DownloadStatus::Cancelled;
                job.unread = true;
            }
            self.sources.remove(&id);
        }
        for (_, part) in self.pending_parts.drain() {
            let _ = std::fs::remove_file(part);
        }
        if let Some(id) = conflict_id {
            cx.emit(DownloadNotice::ConflictCleared { id });
        }
        if let Some(key) = batch_conflict_key {
            cx.emit(DownloadNotice::BatchConflictCleared { key });
        }
        for job in &mut self.jobs {
            if matches!(job.status, DownloadStatus::Queued) {
                job.status = DownloadStatus::Cancelled;
                job.unread = true;
            }
        }
        self.requests.clear();
        self.pending_batch = None;
        self.batch_policies.clear();
        self.batch_ids.clear();
        self.batch_mode = false;
        self.batch_saved = 0;
        self.batch_failed = 0;
        self.generation = self.generation.wrapping_add(1);
        self.capability_cancellations.cancel_all();
        self.capability_tasks.clear();
        self.capabilities.clear();
        for key in invalidated {
            // A missing state means that the open row must recompute its key
            // and start a fresh probe for the new credential generation.
            cx.emit(CapabilityChanged { key, state: None });
        }
        cx.notify();
    }

    pub(crate) fn retry_capabilities(
        &mut self,
        track: PlaybackTrack,
        cx: &mut Context<Self>,
    ) -> CapabilityState {
        let key = CapabilityKey::new(&track, self.account.read(cx).credential_generation());
        self.capabilities.remove(&key);
        self.ensure_capabilities(track, cx)
    }

    fn conflict_target(&self, id: u64) -> Option<(u64, PathBuf)> {
        let active = self
            .active
            .as_ref()
            .map(|(generation, active_id, _)| (*generation, *active_id));
        current_conflict(active, id, &self.job(id)?.status)
    }

    fn batch_conflict_policy_for(&self, id: u64) -> Option<BatchConflictPolicy> {
        self.batch_policies.get(&id).copied()
    }

    fn emit_conflict(&self, id: u64, cx: &mut Context<Self>) {
        let Some(job) = self.job(id) else {
            return;
        };
        cx.emit(DownloadNotice::Conflict {
            id,
            title: job.track.title.clone(),
            quality: quality_label(job, None),
        });
    }

    fn remove_pending_part(&mut self, id: u64) {
        if let Some(part) = self.pending_parts.remove(&id) {
            let _ = std::fs::remove_file(part);
        }
    }

    fn finalize_pending_part(
        &mut self,
        id: u64,
        generation: u64,
        path: PathBuf,
        part: PathBuf,
        cancellation: CancellationToken,
        cx: &mut Context<Self>,
    ) {
        let part_for_task = part.clone();
        let destination = path.clone();
        let task = self.runtime.spawn(async move {
            if cancellation.is_cancelled() {
                return Err("Download request cancelled".to_owned());
            }
            finalize_download(&part_for_task, &destination, true)
                .await
                .map_err(|error| error.into_io().to_string())
        });
        let entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The download worker stopped unexpectedly".into()));
            let accepted = entity.update(cx, |model, cx| {
                if !model.accepts_transfer(id, generation) {
                    return false;
                }
                model.pending_parts.remove(&id);
                if result.is_err() {
                    let _ = std::fs::remove_file(&part);
                }
                let status = match result {
                    Ok(()) => DownloadStatus::Completed(path),
                    Err(error)
                        if error == "Download request cancelled"
                            || error == "Playback request cancelled" =>
                    {
                        DownloadStatus::Cancelled
                    }
                    Err(error) => DownloadStatus::Failed(error),
                };
                model.finish(id, generation, status, cx);
                cx.notify();
                true
            });
            if !accepted {
                let _ = fs::remove_file(&part).await;
            }
        })
        .detach();
    }

    fn download_to(
        &mut self,
        id: u64,
        generation: u64,
        path: PathBuf,
        replace_existing: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.sources.remove(&id) else {
            return;
        };
        let Some((_, _, cancellation)) = &self.active else {
            return;
        };
        let cancellation = cancellation.clone();
        let resolver = self.resolver.clone();
        let cache = self.cache.clone();
        let part = part_path(&path, id, generation);
        let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::unbounded_channel();
        let progress: ProgressCallback = Arc::new(move |update: ProgressUpdate| {
            let _ = progress_sender.send((update.downloaded, update.total));
        });
        let cached_progress = {
            let progress = progress.clone();
            Arc::new(move |downloaded: u64, total: u64| {
                progress(ProgressUpdate::bytes(downloaded, Some(total)));
            })
        };
        let progress_entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let mut last_notify: Option<Instant> = None;
            while let Some((downloaded, total)) = progress_receiver.recv().await {
                let is_complete = total.is_some_and(|total| downloaded >= total);
                let should_notify = is_complete
                    || last_notify
                        .map_or(true, |last| last.elapsed() >= Duration::from_millis(100));
                if should_notify {
                    last_notify = Some(Instant::now());
                    progress_entity.update(cx, |model, cx| {
                        if model.accepts_transfer(id, generation) {
                            if let Some(job) = model.job_mut(id) {
                                job.status = DownloadStatus::Downloading { downloaded, total };
                                cx.notify();
                            }
                        }
                    });
                }
            }
        })
        .detach();
        let output_path = path.clone();
        let part_for_callback = part.clone();
        let task = self.runtime.spawn(async move {
            let result = async {
                if let Some(parent) = part.parent() {
                    fs::create_dir_all(parent).await.map_err(|_| {
                        DownloadTransferError::Failed(
                            "The downloads directory could not be created".to_owned(),
                        )
                    })?;
                }
                let output = fs::File::create(&part).await.map_err(|_| {
                    DownloadTransferError::Failed(
                        "The partial download could not be created".to_owned(),
                    )
                })?;
                let transfer = match source {
                    DownloadSource::Resolved(source) => {
                        let resolver = resolver.map_err(DownloadTransferError::Failed)?;
                        resolver
                            .download_source(source, output, &cancellation, Some(progress))
                            .await
                    }
                    DownloadSource::Cached { cached, fallback } => {
                        let Some(cache) = cache else {
                            return Err(DownloadTransferError::Failed(
                                "The audio cache is unavailable".to_owned(),
                            ));
                        };
                        let mut output = output;
                        match cache
                            .copy_cached_variant(
                                &cached,
                                &mut output,
                                &cancellation,
                                Some(cached_progress.as_ref()),
                            )
                            .await
                        {
                            Ok(()) => Ok(()),
                            Err(error) if error == CACHED_DOWNLOAD_INVALID => {
                                async {
                                    if cancellation.is_cancelled() {
                                        return Err("Download request cancelled".to_owned());
                                    }
                                    output.set_len(0).await.map_err(|_| {
                                        "The partial download could not be reset".to_owned()
                                    })?;
                                    output.seek(std::io::SeekFrom::Start(0)).await.map_err(
                                        |_| "The partial download could not be reset".to_owned(),
                                    )?;
                                    let resolver = fallback.resolver?;
                                    let source = resolver
                                        .resolve_download_source(
                                            &fallback.track,
                                            fallback.deezer_arl,
                                            fallback.soundcloud_token,
                                            fallback.murglar,
                                            fallback.variant,
                                            cancellation.clone(),
                                        )
                                        .await?;
                                    resolver
                                        .download_source(
                                            source,
                                            output,
                                            &cancellation,
                                            Some(progress),
                                        )
                                        .await
                                }
                                .await
                            }
                            Err(error) => Err(error),
                        }
                    }
                };
                transfer.map_err(|error| {
                    if error == "Download request cancelled"
                        || error == "Playback request cancelled"
                    {
                        DownloadTransferError::Cancelled
                    } else {
                        DownloadTransferError::Failed(error)
                    }
                })?;
                if cancellation.is_cancelled() {
                    return Err(DownloadTransferError::Cancelled);
                }
                finalize_download(&part, &output_path, replace_existing)
                    .await
                    .map_err(|error| match error {
                        FinalizeError::DestinationExists => {
                            DownloadTransferError::DestinationExists
                        }
                        FinalizeError::Io(error) => DownloadTransferError::Failed(format!(
                            "The completed download could not be moved into place: {error}"
                        )),
                    })
            }
            .await;
            if !matches!(result, Err(DownloadTransferError::DestinationExists)) {
                let _ = fs::remove_file(&part).await;
            }
            result
        });
        let entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(DownloadTransferError::Failed(
                    "The download worker stopped unexpectedly".into(),
                ))
            });
            let retain_part = matches!(result, Err(DownloadTransferError::DestinationExists));
            let accepted = entity.update(cx, |model, cx| {
                if !model.accepts_transfer(id, generation) {
                    return false;
                }
                if retain_part {
                    model.pending_parts.insert(id, part_for_callback.clone());
                    if let Some(job) = model.job_mut(id) {
                        job.status = DownloadStatus::NeedsConfirmation { path: path.clone() };
                    }
                    model.emit_conflict(id, cx);
                    cx.notify();
                    return true;
                }
                let status = match result {
                    Ok(()) => DownloadStatus::Completed(path),
                    Err(DownloadTransferError::Cancelled) => DownloadStatus::Cancelled,
                    Err(DownloadTransferError::Failed(error)) => DownloadStatus::Failed(error),
                    Err(DownloadTransferError::DestinationExists) => unreachable!(),
                };
                model.finish(id, generation, status, cx);
                cx.notify();
                true
            });
            if retain_part && !accepted {
                let _ = fs::remove_file(&part_for_callback).await;
            }
        })
        .detach();
    }

    fn accepts(&self, id: u64, generation: u64) -> bool {
        self.active
            .as_ref()
            .is_some_and(|(current, active, _)| *active == id && *current == generation)
    }

    fn accepts_transfer(&self, id: u64, generation: u64) -> bool {
        self.accepts(id, generation)
            && self.job(id).is_some_and(|job| {
                matches!(
                    job.status,
                    DownloadStatus::Resolving | DownloadStatus::Downloading { .. }
                )
            })
    }
    fn job(&self, id: u64) -> Option<&DownloadJob> {
        self.jobs.iter().find(|job| job.id == id)
    }
    fn job_mut(&mut self, id: u64) -> Option<&mut DownloadJob> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }
    fn finish(&mut self, id: u64, generation: u64, status: DownloadStatus, cx: &mut Context<Self>) {
        if self.accepts(id, generation) {
            let unread = matches!(
                status,
                DownloadStatus::Completed(_)
                    | DownloadStatus::Skipped(_)
                    | DownloadStatus::Failed(_)
                    | DownloadStatus::Cancelled
            );
            let job = self.job_mut(id).unwrap();
            job.status = status.clone();
            job.unread = unread;
            self.active = None;
            self.record_outcome(id, &status, cx);
            self.start_next(cx);
        }
    }

    fn record_outcome(&mut self, id: u64, status: &DownloadStatus, cx: &mut Context<Self>) {
        if self.batch_ids.remove(&id) {
            self.batch_policies.remove(&id);
            let (saved, failed) = batch_outcome(status);
            self.batch_saved += saved;
            self.batch_failed += failed;
            if self.batch_ids.is_empty() {
                let (saved, failed) = (self.batch_saved, self.batch_failed);
                let pending_conflict_key = self.pending_batch.take().map(|batch| batch.key);
                self.batch_mode = false;
                self.batch_policies.clear();
                if let Some(key) = pending_conflict_key {
                    cx.emit(DownloadNotice::BatchConflictCleared { key });
                }
                if saved > 0 || failed > 0 {
                    if failed > 0 {
                        self.notify_toast(
                            ToastKind::Warning,
                            "Downloads Finished with Errors",
                            Some(format!(
                                "{saved} saved; {failed} unavailable in that format."
                            )),
                            cx,
                        );
                    } else {
                        cx.emit(DownloadNotice::BatchCompleted { saved });
                    }
                }
            }
            return;
        }
        match status {
            DownloadStatus::Completed(path) => {
                if let Some(job) = self.job(id) {
                    cx.emit(DownloadNotice::Completed {
                        title: job.track.title.clone(),
                        quality: quality_label(
                            job,
                            std::fs::metadata(path).ok().map(|metadata| metadata.len()),
                        ),
                    });
                }
            }
            DownloadStatus::Failed(error) => {
                self.notify_toast(ToastKind::Error, "Download Failed", Some(error.clone()), cx);
            }
            _ => {}
        }
    }
}

fn quality_label(job: &DownloadJob, downloaded_size: Option<u64>) -> String {
    job.quality
        .as_ref()
        .map(|quality| quality.label(job.track.duration, downloaded_size))
        .unwrap_or_else(|| "Quality unavailable".to_owned())
}

fn destination_guess(track: &PlaybackTrack, extension: &str, directory: &Path) -> PathBuf {
    directory.join(format!(
        "{} - {}.{extension}",
        sanitize_filename(&track.artist),
        sanitize_filename(&track.title)
    ))
}

fn batch_download_extension(
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

fn destination_with_extension(track: &PlaybackTrack, extension: &str, directory: &Path) -> PathBuf {
    directory.join(format!(
        "{} - {}.{extension}",
        sanitize_filename(&track.artist),
        sanitize_filename(&track.title)
    ))
}

fn part_path(path: &Path, id: u64, generation: u64) -> PathBuf {
    path.with_file_name(format!(
        ".{}.{}.{}.part",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("download"),
        id,
        generation,
    ))
}

#[cfg(test)]
mod tests {
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

    fn quality_job() -> DownloadJob {
        let mut job = DownloadJob::new(
            1,
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
                service_url: String::new(),
            },
            DownloadVariant::Standard,
        );
        job.quality = Some(ResolvedQuality {
            format: "MP3".into(),
            source_size: Some(80_000),
            declared_bitrate: None,
        });
        job
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
}
