mod capability;
mod finalize;
mod model;
mod naming;
mod platform;
mod quality;
mod state;

pub(crate) use capability::{CapabilityKey, CapabilityState};
pub(crate) use model::{CapabilityChanged, DownloadModel, DownloadNotice};
pub(crate) use naming::inspect_batch_targets;
pub(crate) use naming::sanitize_filename;
pub(crate) use platform::{open_downloads_folder, reveal_file};
pub(crate) use quality::{format_bitrate, format_label};
pub(crate) use state::{
    BatchConflictPolicy, BatchExistingTarget, BatchNeedsConfirmation, DownloadJob, DownloadStatus,
    remove_claimed_jobs,
};
