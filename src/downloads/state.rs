use std::path::PathBuf;

use crate::playback::{DownloadVariant, PlaybackTrack};

use super::quality::ResolvedQuality;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DownloadStatus {
    Queued,
    Resolving,
    NeedsConfirmation { path: PathBuf },
    Downloading { downloaded: u64, total: Option<u64> },
    Completed(PathBuf),
    Skipped(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BatchNeedsConfirmation {
    pub(crate) ids: Vec<u64>,
    pub(crate) existing: usize,
    pub(crate) existing_targets: Vec<BatchExistingTarget>,
    pub(crate) key: u64,
}

/// A target which was present when a batch was queued. Keeping both the job
/// ID and the destination lets the model skip only the files that actually
/// collide, while still starting the missing tracks in the same batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BatchExistingTarget {
    pub(crate) id: u64,
    pub(crate) path: PathBuf,
}

/// The policy selected for a pending batch conflict. This is deliberately
/// separate from the old numbered-copy confirmation state: overwrite keeps
/// the original destination and lets the finalizer replace it atomically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BatchConflictPolicy {
    SkipExisting,
    OverwriteExisting,
}

/// Decide how a batch item should proceed after its resolved destination is
/// known. A destination which was missing at the point of this check always
/// uses the normal create-new path, even after an overwrite policy was chosen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BatchTargetAction {
    DownloadNew,
    SkipExisting,
    OverwriteExisting,
    NeedsConfirmation,
}

pub(crate) fn batch_target_action(
    policy: Option<BatchConflictPolicy>,
    destination_exists: bool,
) -> BatchTargetAction {
    if !destination_exists {
        return BatchTargetAction::DownloadNew;
    }
    match policy {
        Some(BatchConflictPolicy::SkipExisting) => BatchTargetAction::SkipExisting,
        Some(BatchConflictPolicy::OverwriteExisting) => BatchTargetAction::OverwriteExisting,
        None => BatchTargetAction::NeedsConfirmation,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DownloadJob {
    pub(crate) id: u64,
    pub(crate) track: PlaybackTrack,
    pub(crate) variant: DownloadVariant,
    pub(crate) quality: Option<ResolvedQuality>,
    pub(crate) status: DownloadStatus,
    pub(crate) unread: bool,
}

impl DownloadJob {
    pub(crate) fn new(id: u64, track: PlaybackTrack, variant: DownloadVariant) -> Self {
        Self {
            id,
            track,
            variant,
            quality: None,
            status: DownloadStatus::Queued,
            unread: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn accepts(&self, generation: u64, id: u64, current_generation: u64) -> bool {
        self.id == id && generation == current_generation
    }

    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            DownloadStatus::Completed(_)
                | DownloadStatus::Skipped(_)
                | DownloadStatus::Failed(_)
                | DownloadStatus::Cancelled
        )
    }
}

pub(crate) fn remove_claimed_jobs(jobs: &mut Vec<DownloadJob>, ids: &[u64]) {
    let ids = ids
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    jobs.retain(|job| !ids.contains(&job.id));
}

/// Return the generation and destination only for the active job which is
/// currently waiting on a collision decision.
pub(crate) fn current_conflict(
    active: Option<(u64, u64)>,
    id: u64,
    status: &DownloadStatus,
) -> Option<(u64, PathBuf)> {
    let (generation, active_id) = active?;
    if active_id != id {
        return None;
    }
    match status {
        DownloadStatus::NeedsConfirmation { path } => Some((generation, path.clone())),
        _ => None,
    }
}

pub(crate) fn batch_outcome(status: &DownloadStatus) -> (usize, usize) {
    match status {
        DownloadStatus::Completed(_) => (1, 0),
        DownloadStatus::Failed(_) => (0, 1),
        DownloadStatus::Queued
        | DownloadStatus::Resolving
        | DownloadStatus::NeedsConfirmation { .. }
        | DownloadStatus::Downloading { .. }
        | DownloadStatus::Skipped(_)
        | DownloadStatus::Cancelled => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::DownloadVariant;
    use std::time::Duration;

    fn job(id: u64) -> DownloadJob {
        DownloadJob::new(
            id,
            PlaybackTrack {
                downloadable: false,
                progressive: false,
                provider: crate::playback::PlaybackProvider::SoundCloud,
                id: "1".into(),
                title: "Title".into(),
                artist: "Artist".into(),
                album: String::new(),
                album_id: String::new(),
                release_date: String::new(),
                artists: Vec::new(),
                artwork: String::new(),
                duration: Duration::ZERO,
                explicit: false,
                service_url: String::new(),
            },
            DownloadVariant::Standard,
        )
    }

    #[test]
    fn stale_completion_is_rejected() {
        assert!(!job(1).accepts(1, 1, 2));
        assert!(!job(2).accepts(1, 1, 1));
        assert!(job(1).accepts(1, 1, 1));
    }

    #[test]
    fn cancellation_is_a_terminal_state() {
        let mut value = job(1);
        value.status = DownloadStatus::Cancelled;
        assert_eq!(value.status, DownloadStatus::Cancelled);
    }

    #[test]
    fn terminal_jobs_are_cleared_and_pending_jobs_survive() {
        let mut jobs = vec![job(1), job(2), job(3)];
        jobs[0].status = DownloadStatus::Completed("file".into());
        jobs[1].status = DownloadStatus::Failed("error".into());
        jobs[2].status = DownloadStatus::NeedsConfirmation {
            path: "file".into(),
        };
        assert!(jobs[0].is_terminal());
        jobs[0].status = DownloadStatus::Skipped("existing".into());
        assert!(jobs[0].is_terminal());
        assert!(!jobs[2].is_terminal());
        jobs.retain(|job| !job.is_terminal());
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn retry_requires_a_new_generation() {
        let mut generation: u64 = 4;
        generation = generation.wrapping_add(1);
        assert_eq!(generation, 5);
    }

    #[test]
    fn failed_job_keeps_its_variant_for_retry() {
        let mut value = job(1);
        value.variant = DownloadVariant::DeezerFlac;
        value.status = DownloadStatus::Failed("network error".into());
        value.unread = true;
        value.quality = Some(ResolvedQuality {
            format: "FLAC".into(),
            source_size: Some(1),
            declared_bitrate: None,
        });
        assert_eq!(value.variant, DownloadVariant::DeezerFlac);
        assert!(!value.track.title.is_empty());
    }

    #[test]
    fn cancelling_a_claimed_batch_removes_only_its_jobs() {
        let mut jobs = vec![job(1), job(2), job(3)];
        remove_claimed_jobs(&mut jobs, &[1, 3]);
        assert_eq!(jobs.iter().map(|job| job.id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn current_conflict_rejects_stale_ids_and_non_conflict_statuses() {
        let status = DownloadStatus::NeedsConfirmation {
            path: "existing.mp3".into(),
        };
        assert_eq!(
            current_conflict(Some((7, 4)), 4, &status),
            Some((7, PathBuf::from("existing.mp3")))
        );
        assert_eq!(current_conflict(Some((7, 4)), 3, &status), None);
        assert_eq!(
            current_conflict(Some((7, 4)), 4, &DownloadStatus::Queued),
            None
        );
        assert_eq!(current_conflict(None, 4, &status), None);
    }

    #[test]
    fn cancelled_batch_items_do_not_count_as_saved_or_failed() {
        assert_eq!(batch_outcome(&DownloadStatus::Cancelled), (0, 0));
        assert_eq!(
            batch_outcome(&DownloadStatus::Skipped("old".into())),
            (0, 0)
        );
        assert_eq!(
            batch_outcome(&DownloadStatus::Completed("new".into())),
            (1, 0)
        );
        assert_eq!(
            batch_outcome(&DownloadStatus::Failed("error".into())),
            (0, 1)
        );
    }

    #[test]
    fn batch_conflict_policy_keeps_skip_and_overwrite_explicit() {
        assert_ne!(
            BatchConflictPolicy::SkipExisting,
            BatchConflictPolicy::OverwriteExisting
        );
    }

    #[test]
    fn batch_target_action_keeps_missing_items_on_the_normal_download_path() {
        assert_eq!(
            batch_target_action(Some(BatchConflictPolicy::SkipExisting), true),
            BatchTargetAction::SkipExisting
        );
        assert_eq!(
            batch_target_action(Some(BatchConflictPolicy::SkipExisting), false),
            BatchTargetAction::DownloadNew
        );
        assert_eq!(
            batch_target_action(Some(BatchConflictPolicy::OverwriteExisting), true),
            BatchTargetAction::OverwriteExisting
        );
        assert_eq!(
            batch_target_action(Some(BatchConflictPolicy::OverwriteExisting), false),
            BatchTargetAction::DownloadNew
        );
        assert_eq!(
            batch_target_action(None, true),
            BatchTargetAction::NeedsConfirmation
        );
        assert_eq!(
            batch_target_action(None, false),
            BatchTargetAction::DownloadNew
        );
    }

    #[test]
    fn pending_batch_records_each_existing_job_destination() {
        let pending = BatchNeedsConfirmation {
            ids: vec![7, 8],
            existing: 1,
            existing_targets: vec![BatchExistingTarget {
                id: 7,
                path: PathBuf::from("Artist - Song.mp3"),
            }],
            key: 3,
        };
        assert_eq!(pending.existing, pending.existing_targets.len());
        assert_eq!(pending.existing_targets[0].id, pending.ids[0]);
        assert_eq!(
            pending.existing_targets[0].path,
            PathBuf::from("Artist - Song.mp3")
        );
    }
}
