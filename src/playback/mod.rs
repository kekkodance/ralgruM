pub(crate) mod asio_drivers;
pub(crate) mod automation;
mod cache;
mod deezer_extension;
mod download_variant;
mod engine;
mod fade;
mod listen_history;
pub(crate) mod media_source;
pub(crate) mod output_devices;
mod player_bar;
pub(crate) mod progressive;
mod queue;
mod queue_list;
mod queue_rows;
mod ramped_gain;
mod resolve_limiter;
mod resolve_source_cache;
pub(crate) mod resolver;
pub(crate) mod retry;
mod soundcloud_hls;

use queue::{QueueDrag, QueueDragGhost};
mod standby;
mod state;
mod track_info;
mod view;

pub(crate) use crate::murglar_backend::MediaCredentials;
pub(crate) use automation::{AutomationDirective, AutomationTransition};
pub(crate) use cache::{AudioCache, CACHED_DOWNLOAD_INVALID, CachedDownload, LIMITS_MB, Overview};
pub(crate) use deezer_extension::{
    ExtensionObserverKey, duplicate_retry_delay, should_extend_at_tail,
};
pub(crate) use download_variant::{
    DownloadChoice, DownloadVariant, collection_download_availability,
    deezer_collection_download_choices, selection_order,
};
pub(crate) use listen_history::ListenHistorySignal;
#[allow(unused_imports)]
pub(crate) use media_source::{AudioFormat, DownloadOutput, ProgressCallback, ProgressUpdate};
pub(crate) use queue::QueuePanel;
pub(crate) use resolver::{ResolvedSource, StreamResolver};
pub(crate) use state::{
    ContentPreferences, DeezerFlowKind, ExtensionApply, PlaybackContext, PlaybackProvider,
    PlaybackState, PlaybackStatus, PlaybackTrack, PreviousAction, QueueExtensionTicket, RepeatMode,
    RightSidebar, VolumeIconLevel,
};
pub(crate) use track_info::{ResolvedTrackInfo, TrackInfoProbe, describe_track_info};
pub(crate) use view::{AudioOutputSettings, PlaybackModel};

pub(crate) use player_bar::{PlaybackView, PlayerBarMotion, PlayerBarVisual};
