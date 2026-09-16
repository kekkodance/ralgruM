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
    state::{
        BatchTargetAction, DownloadFileStat, batch_outcome, batch_target_action, current_conflict,
        file_destination,
    },
};
use crate::{
    playback::{
        AudioCache, CACHED_DOWNLOAD_INVALID, CachedDownload, DownloadVariant, PlaybackProvider,
        PlaybackTrack, ProgressCallback, ProgressUpdate, ResolvedSource, StreamResolver,
    },
    search::{DeezerArl, SoundCloudToken},
    settings::{AccountState, SettingsView},
    toast::{ToastKind, ToastStack},
};

pub(crate) struct DownloadModel {
    pub(crate) jobs: Vec<DownloadJob>,
    pub(crate) platform_status: Option<String>,
    account: Entity<AccountState>,
    settings: Option<Entity<SettingsView>>,
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
    /// File-stat probes in flight, by job id. Each entry pairs the probed
    /// destination with a token from `next_file_stat_probe`; installing a
    /// result requires both to still match, so a probe whose expectation
    /// was dropped or replaced cannot cache facts about an obsolete
    /// destination or a superseded probe.
    file_stat_probes: HashMap<u64, (PathBuf, u64)>,
    next_file_stat_probe: u64,
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
    Resolved(Box<ResolvedSource>),
    Cached {
        cached: CachedDownload,
        fallback: Box<CachedDownloadFallback>,
    },
}

impl DownloadModel {
    pub(crate) fn new(account: Entity<AccountState>, runtime: Arc<Runtime>) -> Self {
        Self {
            jobs: Vec::new(),
            platform_status: None,
            account,
            settings: None,
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
            file_stat_probes: HashMap::new(),
            next_file_stat_probe: 0,
        }
    }

    /// Wire the live settings entity that owns the downloads directory.
    /// When it is missing, downloads fail visibly instead of guessing a
    /// destination.
    pub(crate) fn with_settings(mut self, settings: Entity<SettingsView>) -> Self {
        self.settings = Some(settings);
        self
    }

    /// The live downloads directory from the settings entity. This never
    /// falls back to a default directory: an unavailable settings entity
    /// is an error the caller must surface to the user.
    fn downloads_dir(&self, cx: &Context<Self>) -> Result<PathBuf, String> {
        self.settings
            .as_ref()
            .map(|settings| settings.read(cx).saved().effective_downloads_dir())
            .ok_or_else(|| "The downloads directory could not be read from settings".to_owned())
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
            entity.update(cx, |model, cx| {
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
            Some(extension) => match self.downloads_dir(cx) {
                Ok(downloads_dir) => {
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
                Err(error) => {
                    // Settings are unavailable, so no destination can be
                    // determined. Fail the whole batch visibly instead of
                    // guessing a directory for the conflict pre-check or
                    // the transfer itself.
                    for id in &ids {
                        self.requests.remove(id);
                        if let Some(job) = self.job_mut(*id) {
                            job.status = DownloadStatus::Failed(error.clone());
                            job.unread = true;
                        }
                    }
                    self.notify_toast(ToastKind::Error, "Downloads Failed", Some(error), cx);
                    cx.notify();
                    return ids;
                }
            },
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
            self.invalidate_file_stat(id, cx);
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
                    fallback: Box::new(CachedDownloadFallback {
                        resolver: resolver.clone(),
                        track: resolve_track.clone(),
                        deezer_arl: deezer_arl.clone(),
                        soundcloud_token: soundcloud_token.clone(),
                        murglar: murglar.clone(),
                        variant,
                    }),
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
                .map(|source| DownloadSource::Resolved(Box::new(source)))
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
        // The destination directory is a precondition for any transfer.
        // It comes from the live settings entity, and an unavailable
        // entity fails the job visibly here rather than letting the
        // download fall back to a default directory.
        let downloads_dir = match self.downloads_dir(cx) {
            Ok(downloads_dir) => downloads_dir,
            Err(error) => {
                self.finish(id, generation, DownloadStatus::Failed(error), cx);
                return;
            }
        };
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
        let path = destination_with_extension(&track, &extension, &downloads_dir);
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
                        self.invalidate_file_stat(id, cx);
                        self.emit_conflict(id, cx);
                        cx.notify();
                    }
                }
            } else {
                self.job_mut(id).unwrap().status =
                    DownloadStatus::NeedsConfirmation { path: path.clone() };
                self.invalidate_file_stat(id, cx);
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

    /// Refresh the cached filesystem facts for every job that renders
    /// file information and is still missing a stat. The probe itself
    /// runs on the background executor, never the UI thread, and each
    /// job has at most one probe in flight, so repeated refreshes
    /// coalesce. This is the entry point for page opens and explicit
    /// refreshes; state transitions funnel through `invalidate_file_stat`.
    pub(crate) fn refresh_file_stats(&mut self, cx: &mut Context<Self>) {
        let targets: Vec<(u64, PathBuf)> = self
            .jobs
            .iter()
            .filter(|job| !self.file_stat_probes.contains_key(&job.id))
            .filter_map(|job| {
                file_destination(&job.status)
                    .filter(|_| job.file.is_none())
                    .map(|path| (job.id, path.to_owned()))
            })
            .collect();
        for (id, path) in targets {
            self.next_file_stat_probe = self.next_file_stat_probe.wrapping_add(1);
            let token = self.next_file_stat_probe;
            self.file_stat_probes.insert(id, (path.clone(), token));
            let probe_path = path.clone();
            let probe = cx
                .background_executor()
                .spawn(async move { DownloadFileStat::probe(&probe_path) });
            let entity = cx.entity().clone();
            cx.spawn(async move |_, cx| {
                let stat = probe.await;
                entity.update(cx, |model, cx| {
                    model.retire_file_stat_probe(id, path, token, stat, cx);
                });
            })
            .detach();
        }
    }

    /// Install a completed probe unless it has gone stale. The probe
    /// answers exactly one destination, so its result is cached only
    /// while the tracked expectation still belongs to this probe and the
    /// job still exists, still has no cached stat, and still points at
    /// the probed path. Any status or destination change in between
    /// discards the result instead of caching facts about an obsolete
    /// destination.
    fn retire_file_stat_probe(
        &mut self,
        id: u64,
        path: PathBuf,
        token: u64,
        stat: DownloadFileStat,
        cx: &mut Context<Self>,
    ) {
        let owns_expectation =
            self.file_stat_probes
                .get(&id)
                .is_some_and(|(expected_path, expected_token)| {
                    *expected_token == token && expected_path.as_path() == path.as_path()
                });
        if !owns_expectation {
            // A newer probe took over this job; its expectation must
            // survive this retirement.
            return;
        }
        self.file_stat_probes.remove(&id);
        let install = self.job(id).is_some_and(|job| {
            job.file.is_none() && file_destination(&job.status) == Some(path.as_path())
        });
        if install {
            self.job_mut(id).unwrap().file = Some(stat);
            cx.notify();
        }
    }

    /// Drop the cached stat for a job whose destination just changed and
    /// queue a fresh background probe. The in-flight expectation is
    /// dropped too: its result would be discarded by the staleness guard,
    /// but keeping the entry would delay the fresh probe until the
    /// obsolete one lands.
    fn invalidate_file_stat(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(job) = self.job_mut(id) {
            job.file = None;
        }
        self.file_stat_probes.remove(&id);
        self.refresh_file_stats(cx);
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
        job.file = None;
        // A retry abandons whatever destination the failure described, so
        // any in-flight stat expectation for it is obsolete.
        self.file_stat_probes.remove(&id);
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
        // Latest-value progress channel: every tick overwrites the
        // previous sample, so a fast publisher can never queue more than
        // one update no matter how long the receiver is starved.
        // Completion, failure, and cancellation travel the transfer task
        // result instead, so overwriting a superseded tick cannot lose
        // the outcome.
        let (progress_sender, progress_receiver) =
            tokio::sync::watch::channel((0_u64, None::<u64>));
        let progress: ProgressCallback = Arc::new(move |update: ProgressUpdate| {
            // Sending fails only once the pump is gone, which means the
            // transfer was already retired.
            let _ = progress_sender.send((update.downloaded, update.total));
        });
        let cached_progress = {
            let progress = progress.clone();
            Arc::new(move |downloaded: u64, total: u64| {
                progress(ProgressUpdate::bytes(downloaded, Some(total)));
            })
        };
        spawn_progress_pump(cx, id, generation, progress_receiver);
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
                            .download_source(*source, output, &cancellation, Some(progress))
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
                    model.invalidate_file_stat(id, cx);
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
            job.file = None;
            self.active = None;
            self.record_outcome(id, &status, cx);
            self.start_next(cx);
            self.invalidate_file_stat(id, cx);
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

/// Forward the newest progress sample into the job status. The channel is
/// latest-value: every published tick overwrites the previous sample, so
/// the queue is structurally bounded to one entry no matter how fast the
/// publisher runs or how long the pump is starved, and each wake reads
/// the newest byte counts. Terminal states travel through the transfer
/// task result, never this channel, and the transfer dropping its sender
/// ends the pump, so skipping superseded ticks cannot lose the outcome
/// and the job still converges to its final state.
fn spawn_progress_pump(
    cx: &mut Context<DownloadModel>,
    id: u64,
    generation: u64,
    mut receiver: tokio::sync::watch::Receiver<(u64, Option<u64>)>,
) {
    let progress_entity = cx.entity().clone();
    cx.spawn(async move |_, cx| {
        let mut last_notify: Option<Instant> = None;
        while receiver.changed().await.is_ok() {
            let (downloaded, total) = *receiver.borrow_and_update();
            let is_complete = total.is_some_and(|total| downloaded >= total);
            let should_notify = is_complete
                || last_notify.is_none_or(|last| last.elapsed() >= Duration::from_millis(100));
            if should_notify {
                last_notify = Some(Instant::now());
                progress_entity.update(cx, |model, cx| {
                    if model.accepts_transfer(id, generation)
                        && let Some(job) = model.job_mut(id)
                    {
                        job.status = DownloadStatus::Downloading { downloaded, total };
                        cx.notify();
                    }
                });
            }
        }
    })
    .detach();
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
}
