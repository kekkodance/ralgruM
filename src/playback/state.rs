use std::{collections::HashSet, time::Duration};

use crate::{library, search};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum PlaybackProvider {
    Deezer,
    SoundCloud,
}

impl From<search::Provider> for PlaybackProvider {
    fn from(value: search::Provider) -> Self {
        match value {
            search::Provider::Deezer => Self::Deezer,
            search::Provider::SoundCloud => Self::SoundCloud,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlaybackTrack {
    pub(crate) provider: PlaybackProvider,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) album: String,
    pub(crate) album_id: String,
    pub(crate) release_date: String,
    pub(crate) artists: Vec<crate::search::TrackArtistRef>,
    pub(crate) artwork: String,
    pub(crate) duration: Duration,
    pub(crate) downloadable: bool,
    pub(crate) progressive: bool,
    pub(crate) explicit: bool,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id when a link is needed.
    pub(crate) service_url: String,
}

fn queue_identity(tracks: &[PlaybackTrack]) -> Vec<(PlaybackProvider, String)> {
    tracks
        .iter()
        .map(|track| (track.provider, track.id.clone()))
        .collect()
}

fn adjust_queue_indices(indices: &mut Vec<usize>, removed: usize) {
    indices.retain(|queued| *queued != removed);
    for queued in indices {
        if *queued > removed {
            *queued -= 1;
        }
    }
}

const SHUFFLE_SEED_FALLBACK: u64 = 0x9E37_79B9_7F4A_7C15;

fn normalize_shuffle_seed(seed: u64) -> u64 {
    if seed == 0 {
        SHUFFLE_SEED_FALLBACK
    } else {
        seed
    }
}

fn fold_shuffle_entropy(bytes: &[u8; 16]) -> u64 {
    let mut low = 0u64;
    let mut high = 0u64;
    for (index, byte) in bytes[..8].iter().enumerate() {
        low |= u64::from(*byte) << (index * 8);
    }
    for (index, byte) in bytes[8..].iter().enumerate() {
        high |= u64::from(*byte) << (index * 8);
    }
    normalize_shuffle_seed(low ^ high.rotate_left(29))
}

fn fresh_shuffle_seed() -> u64 {
    fold_shuffle_entropy(uuid::Uuid::new_v4().as_bytes())
}

impl PlaybackTrack {
    pub(crate) fn from_search(track: &search::Track) -> Self {
        Self {
            provider: track.source.into(),
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            album_id: track.album_id.clone(),
            release_date: track.release_date.clone(),
            artists: track.artists.clone(),
            artwork: track.artwork.clone(),
            duration: Duration::from_secs(track.duration),
            downloadable: track.downloadable,
            progressive: track.progressive,
            explicit: track.explicit,
            service_url: track.service_url.clone(),
        }
    }

    pub(crate) fn from_library(track: &library::Track, provider: search::Provider) -> Self {
        Self {
            provider: provider.into(),
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            album_id: track.album_id.clone(),
            release_date: track.release_date.clone(),
            artists: track.artists.clone(),
            artwork: track.artwork.clone(),
            duration: Duration::from_secs(track.duration),
            downloadable: false,
            progressive: false,
            explicit: track.explicit,
            service_url: track.service_url.clone(),
        }
    }

    pub(crate) fn from_local_library(track: &library::Track) -> Option<Self> {
        let provider = track.origin?;
        let mut playback = Self::from_library(track, provider);
        if provider == search::Provider::SoundCloud {
            // Local library storage keeps provider identity and metadata, but
            // not the transient SoundCloud media flags. Public SoundCloud
            // tracks retain their progressive fallback when restored.
            playback.progressive = true;
        }
        Some(playback)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PlaybackStatus {
    #[default]
    Empty,
    Loading,
    Playing,
    Paused,
    Failed,
    Ended,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RightSidebar {
    #[default]
    Closed,
    Lyrics,
    Queue,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

impl RepeatMode {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Off => "Repeat off",
            Self::All => "Repeat all",
            Self::One => "Repeat one",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DeezerFlowKind {
    Flow,
    SmartMix,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlaybackContext {
    None,
    DeezerLibraryTracks {
        load_id: u64,
    },
    DeezerFlow {
        config_id: String,
        mode: library::FlowMode,
        tuner: Option<library::FlowTuner>,
        kind: DeezerFlowKind,
    },
    DeezerTrackMix {
        seed_track_id: String,
    },
    DeezerArtistMix {
        seed_artist_id: String,
    },
    SoundCloudCollection {
        context_urn: String,
    },
    SoundCloudStation {
        seed_track_id: String,
    },
}

impl PlaybackContext {
    pub(crate) fn is_infinite(&self) -> bool {
        matches!(
            self,
            Self::DeezerFlow {
                kind: DeezerFlowKind::Flow,
                ..
            } | Self::DeezerTrackMix { .. }
                | Self::DeezerArtistMix { .. }
                | Self::SoundCloudStation { .. }
        )
    }

    pub(crate) fn can_extend(&self) -> bool {
        match self {
            Self::DeezerFlow {
                kind: DeezerFlowKind::Flow,
                tuner,
                ..
            } => tuner.is_some(),
            Self::DeezerFlow {
                kind: DeezerFlowKind::SmartMix,
                ..
            }
            | Self::None
            | Self::DeezerLibraryTracks { .. }
            | Self::SoundCloudCollection { .. } => false,
            Self::DeezerTrackMix { .. }
            | Self::DeezerArtistMix { .. }
            | Self::SoundCloudStation { .. } => true,
        }
    }

    pub(crate) fn soundcloud_collection(
        provider: search::Provider,
        action: &str,
        id: &str,
    ) -> Option<Self> {
        if provider != search::Provider::SoundCloud {
            return None;
        }
        let id = id.trim();
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let context_urn = match action {
            "albumTracks" | "playlistTracks" => format!("soundcloud:playlists:{id}"),
            "artistTracks" => format!("soundcloud:users:{id}"),
            _ => return None,
        };
        Some(Self::SoundCloudCollection { context_urn })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExactQueueAppendTicket {
    epoch: u64,
    load_id: u64,
    expected_len: usize,
    expected_prefix: Vec<(PlaybackProvider, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExactQueueAppend {
    Stale,
    Applied {
        added: usize,
        resume_index: Option<usize>,
    },
}

/// Identity captured when a radio request starts.  Queue replacement and
/// external context changes advance the state's epoch, making old tickets
/// harmless when their requests eventually finish.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueueExtensionTicket {
    pub(crate) epoch: u64,
    pub(crate) context: PlaybackContext,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExtensionApply {
    Stale,
    Applied { added: usize },
}

#[derive(Debug)]
pub(crate) struct PlaybackState {
    pub(crate) status: PlaybackStatus,
    pub(crate) queue: Vec<PlaybackTrack>,
    pub(crate) current_index: Option<usize>,
    pub(crate) position: Duration,
    pub(crate) duration: Duration,
    pub(crate) buffered: Duration,
    pub(crate) volume: f32,
    pub(crate) muted: bool,
    pub(crate) repeat_mode: RepeatMode,
    pub(crate) shuffle_enabled: bool,
    pub(crate) skip_explicit: bool,
    pub(crate) error: Option<String>,
    pub(crate) generation: u64,
    pub(crate) right_sidebar: RightSidebar,
    pub(crate) right_sidebar_view: RightSidebar,
    pub(crate) context: PlaybackContext,
    queue_epoch: u64,
    /// Track whose lyrics the right sidebar should show even when it is not
    /// the playing track. Mirrors lyricsController.openContextTrack in the
    /// original app: lyrics are fetched for the menu track without touching
    /// playback.
    lyrics_context: Option<PlaybackTrack>,
    right_sidebar_open: bool,
    // Navigation decks use physical queue positions instead of provider IDs.
    // A library can legitimately contain the same provider/track more than
    // once, and an ID-only deck would select the first occurrence repeatedly.
    play_next: Vec<usize>,
    history: Vec<usize>,
    shuffle_order: Vec<usize>,
    shuffle_played: HashSet<usize>,
    shuffle_seed: u64,
    last_nonzero_volume: f32,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Empty,
            queue: Vec::new(),
            current_index: None,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            buffered: Duration::ZERO,
            volume: 0.8,
            muted: false,
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            skip_explicit: false,
            error: None,
            generation: 0,
            right_sidebar: RightSidebar::Closed,
            right_sidebar_view: RightSidebar::Lyrics,
            context: PlaybackContext::None,
            queue_epoch: 0,
            lyrics_context: None,
            right_sidebar_open: false,
            play_next: Vec::new(),
            history: Vec::new(),
            shuffle_order: Vec::new(),
            shuffle_played: HashSet::new(),
            shuffle_seed: fresh_shuffle_seed(),
            last_nonzero_volume: 0.8,
        }
    }
}

impl PlaybackState {
    pub(crate) fn current(&self) -> Option<&PlaybackTrack> {
        self.current_index.and_then(|index| self.queue.get(index))
    }

    pub(crate) fn explicit_blocked(&self, track: &PlaybackTrack) -> bool {
        self.skip_explicit && track.explicit
    }

    pub(crate) fn set_skip_explicit(&mut self, enabled: bool) -> bool {
        self.skip_explicit = enabled;
        self.current()
            .is_some_and(|track| self.explicit_blocked(track))
    }

    pub(crate) fn current_id(&self) -> Option<&str> {
        self.current().map(|track| track.id.as_str())
    }

    pub(crate) fn replace(&mut self, queue: Vec<PlaybackTrack>, index: usize) -> Option<u64> {
        self.bump_queue_epoch();
        if queue.is_empty() || index >= queue.len() {
            self.clear();
            return None;
        }
        let index = if self.explicit_blocked(&queue[index]) {
            match queue.iter().position(|track| !self.explicit_blocked(track)) {
                Some(first_playable) => first_playable,
                None => {
                    self.clear();
                    return None;
                }
            }
        } else {
            index
        };
        self.queue = queue;
        self.current_index = Some(index);
        self.context = PlaybackContext::None;
        if self.right_sidebar_open {
            self.right_sidebar = self.right_sidebar_view;
        }
        self.history.clear();
        self.play_next.clear();
        self.reset_shuffle_pass();
        self.begin_selection()
    }

    pub(crate) fn select(&mut self, index: usize) -> Option<u64> {
        self.select_with_history(index, true)
    }

    fn select_with_history(&mut self, index: usize, record_current: bool) -> Option<u64> {
        if index >= self.queue.len() || self.explicit_blocked(&self.queue[index]) {
            return None;
        }
        if record_current
            && let Some(current) = self.current_index.filter(|current| *current != index)
        {
            self.history.push(current);
        }
        self.current_index = Some(index);
        self.play_next.retain(|queued| *queued != index);
        self.shuffle_played.insert(index);
        self.shuffle_order.retain(|queued| *queued != index);
        self.begin_selection()
    }

    pub(crate) fn upcoming_indices(&self) -> Vec<usize> {
        let Some(current) = self.current_index.filter(|index| *index < self.queue.len()) else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        seen.insert(current);
        let mut upcoming = Vec::with_capacity(self.queue.len().saturating_sub(1));
        let mut add = |index: usize| {
            if index < self.queue.len() && seen.insert(index) {
                upcoming.push(index);
            }
        };

        for index in self.play_next.iter().rev() {
            add(*index);
        }
        if self.shuffle_enabled {
            for index in self.shuffle_order.iter().rev() {
                add(*index);
            }
        } else {
            for index in current + 1..self.queue.len() {
                add(index);
            }
            if self.repeat_mode == RepeatMode::All {
                for index in 0..current {
                    add(index);
                }
            }
        }
        upcoming
    }

    pub(crate) fn first_upcoming_index(&self) -> Option<usize> {
        self.upcoming_indices().into_iter().next()
    }

    fn set_upcoming_order(&mut self, indices: &[usize]) {
        self.play_next = indices.iter().rev().copied().collect();
    }

    pub(crate) fn enqueue_next(&mut self, index: usize) -> bool {
        let mut upcoming = self.upcoming_indices();
        let Some(position) = upcoming.iter().position(|queued| *queued == index) else {
            return false;
        };
        let selected = upcoming.remove(position);
        upcoming.insert(0, selected);
        self.set_upcoming_order(&upcoming);
        true
    }

    pub(crate) fn enqueue_last(&mut self, index: usize) -> bool {
        let mut upcoming = self.upcoming_indices();
        let Some(position) = upcoming.iter().position(|queued| *queued == index) else {
            return false;
        };
        let selected = upcoming.remove(position);
        upcoming.push(selected);
        self.set_upcoming_order(&upcoming);
        true
    }

    pub(crate) fn set_repeat_mode(&mut self, mode: RepeatMode) {
        self.repeat_mode = mode;
    }
    pub(crate) fn cycle_repeat_mode(&mut self) -> RepeatMode {
        self.repeat_mode = self.repeat_mode.next();
        self.repeat_mode
    }
    pub(crate) fn set_shuffle_enabled(&mut self, enabled: bool) {
        if self.shuffle_enabled != enabled {
            self.shuffle_enabled = enabled;
            self.reset_shuffle_pass();
        }
    }
    pub(crate) fn toggle_shuffle(&mut self) -> bool {
        self.set_shuffle_enabled(!self.shuffle_enabled);
        self.shuffle_enabled
    }

    fn begin_selection(&mut self) -> Option<u64> {
        self.generation = self.generation.wrapping_add(1);
        // A new playing track takes the lyrics panel back to following
        // playback, matching the original's pane behavior.
        self.lyrics_context = None;
        self.begin_loading();
        Some(self.generation)
    }

    fn begin_loading(&mut self) {
        self.status = PlaybackStatus::Loading;
        self.position = Duration::ZERO;
        self.buffered = Duration::ZERO;
        self.duration = self
            .current()
            .map_or(Duration::ZERO, |track| track.duration);
        self.error = None;
    }

    fn reset_shuffle_pass(&mut self) {
        self.shuffle_order.clear();
        self.shuffle_played.clear();
        let current = self.current_index;
        if let Some(index) = current {
            self.shuffle_played.insert(index);
        }
        if self.shuffle_enabled {
            self.shuffle_order = (0..self.queue.len())
                .filter(|index| Some(*index) != current)
                .collect();
            self.shuffle_pending_order();
        }
    }

    fn shuffle_pending_order(&mut self) {
        Self::shuffle_indices(&mut self.shuffle_order, &mut self.shuffle_seed);
    }

    fn shuffle_indices(indices: &mut [usize], seed: &mut u64) {
        for index in (1..indices.len()).rev() {
            *seed ^= *seed << 7;
            *seed ^= *seed >> 9;
            indices.swap(index, *seed as usize % (index + 1));
        }
    }

    #[cfg(test)]
    pub(crate) fn loaded(&mut self, generation: u64, duration: Option<Duration>) -> bool {
        self.loaded_fully_buffered(generation, duration)
    }

    pub(crate) fn loaded_fully_buffered(
        &mut self,
        generation: u64,
        duration: Option<Duration>,
    ) -> bool {
        self.set_loaded(generation, duration, true)
    }

    pub(crate) fn loaded_progressive(
        &mut self,
        generation: u64,
        duration: Option<Duration>,
    ) -> bool {
        self.set_loaded(generation, duration, false)
    }

    fn set_loaded(
        &mut self,
        generation: u64,
        duration: Option<Duration>,
        fully_buffered: bool,
    ) -> bool {
        if generation != self.generation || self.status != PlaybackStatus::Loading {
            return false;
        }
        let buffered_fraction = if self.duration.is_zero() {
            0.
        } else {
            (self.buffered.as_secs_f32() / self.duration.as_secs_f32()).clamp(0., 1.)
        };
        if let Some(duration) = duration.filter(|duration| !duration.is_zero()) {
            self.duration = duration;
        }
        self.buffered = if fully_buffered {
            self.duration
        } else {
            self.duration.mul_f32(buffered_fraction)
        };
        self.status = PlaybackStatus::Playing;
        true
    }

    pub(crate) fn set_buffered_fraction(&mut self, fraction: f32) {
        if !self.duration.is_zero() {
            self.buffered = self.duration.mul_f32(fraction.clamp(0.0, 1.0));
        }
    }

    pub(crate) fn fail(&mut self, generation: u64, error: String) -> bool {
        if generation != self.generation {
            return false;
        }
        self.status = PlaybackStatus::Failed;
        self.error = Some(error);
        true
    }

    pub(crate) fn stop_loading(&mut self) -> bool {
        if self.status != PlaybackStatus::Loading {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.status = PlaybackStatus::Ended;
        self.position = Duration::ZERO;
        self.buffered = Duration::ZERO;
        self.error = None;
        true
    }

    /// Open the player bar in a blank loading state before any track exists,
    /// e.g. while the smart mix page is still being fetched after a click.
    /// Any current playback is cleared and the queue epoch advances, so the
    /// bar goes blank immediately and stale loads cannot complete into the
    /// pending state.
    pub(crate) fn begin_pending_load(&mut self) -> bool {
        // Re-entering the pending state must still advance the queue epoch.
        // Otherwise an older request can mistake a newer pending load for its
        // own and close the player bar when that older request completes.
        self.clear();
        self.status = PlaybackStatus::Loading;
        true
    }

    /// Close the player bar again when a pending load never produced a
    /// track. Loads that already selected a track are left alone.
    pub(crate) fn abandon_pending_load(&mut self) -> bool {
        if self.status != PlaybackStatus::Loading || self.current_index.is_some() {
            return false;
        }
        self.bump_queue_epoch();
        self.generation = self.generation.wrapping_add(1);
        self.status = PlaybackStatus::Empty;
        self.error = None;
        true
    }

    /// Whether applying a flow page should keep the currently playing track.
    /// Only pure navigation into an already-playing smart mix preserves the
    /// audio; an explicit play restarts the mix from its first track, and
    /// flows that append rather than replace never preserve.
    pub(crate) fn should_preserve_flow_current(
        &self,
        config_id: &str,
        clear_remaining: bool,
        keep_playing_track: bool,
    ) -> bool {
        keep_playing_track
            && clear_remaining
            && matches!(
                &self.context,
                PlaybackContext::DeezerFlow { config_id: active, .. } if active == config_id
            )
    }

    pub(crate) fn toggle(&mut self) -> Option<bool> {
        match self.status {
            PlaybackStatus::Playing => {
                self.status = PlaybackStatus::Paused;
                Some(false)
            }
            PlaybackStatus::Paused => {
                self.status = PlaybackStatus::Playing;
                Some(true)
            }
            _ => None,
        }
    }

    pub(crate) fn can_next(&self) -> bool {
        if self.repeat_mode == RepeatMode::One {
            return self.current_index.is_some();
        }
        if self.repeat_mode == RepeatMode::All {
            return !self.queue.is_empty();
        }
        if self.play_next.iter().any(|&index| {
            self.queue
                .get(index)
                .is_some_and(|track| !self.explicit_blocked(track))
        }) {
            return true;
        }
        if self.shuffle_enabled {
            return self.shuffle_order.iter().any(|&index| {
                self.queue
                    .get(index)
                    .is_some_and(|track| !self.explicit_blocked(track))
            });
        }
        let Some(current) = self.current_index else {
            return false;
        };
        (current + 1..self.queue.len()).any(|index| !self.explicit_blocked(&self.queue[index]))
    }

    pub(crate) fn next(&mut self) -> Option<u64> {
        if self.repeat_mode == RepeatMode::One
            && !self
                .current()
                .is_some_and(|track| self.explicit_blocked(track))
        {
            return self.begin_selection();
        }
        let next_index = self
            .next_play_next_index()
            .or_else(|| self.next_queue_index());
        let Some(index) = next_index else {
            self.status = PlaybackStatus::Ended;
            self.position = self.duration;
            return None;
        };
        self.select(index)
    }

    fn next_play_next_index(&mut self) -> Option<usize> {
        while let Some(index) = self.play_next.pop() {
            if self
                .queue
                .get(index)
                .is_some_and(|track| !self.explicit_blocked(track))
            {
                return Some(index);
            }
        }
        None
    }

    fn next_queue_index(&mut self) -> Option<usize> {
        if self.shuffle_enabled {
            if let Some(index) = self.next_shuffle_index() {
                return Some(index);
            }
            if self.repeat_mode == RepeatMode::All {
                self.reset_shuffle_pass();
                return self.next_shuffle_index();
            }
            return None;
        }
        let current = self.current_index?;
        for index in current + 1..self.queue.len() {
            if !self.explicit_blocked(&self.queue[index]) {
                return Some(index);
            }
        }
        (self.repeat_mode == RepeatMode::All)
            .then(|| {
                (0..self.queue.len()).find(|index| !self.explicit_blocked(&self.queue[*index]))
            })
            .flatten()
    }

    fn next_shuffle_index(&mut self) -> Option<usize> {
        while let Some(index) = self.shuffle_order.pop() {
            if self
                .queue
                .get(index)
                .is_some_and(|track| !self.explicit_blocked(track))
            {
                return Some(index);
            }
        }
        None
    }

    pub(crate) fn previous(&mut self) -> PreviousAction {
        if self.position > Duration::from_secs(3) {
            self.position = Duration::ZERO;
            return PreviousAction::Restart;
        }
        while let Some(index) = self.history.pop() {
            if self
                .queue
                .get(index)
                .is_some_and(|track| !self.explicit_blocked(track))
            {
                return PreviousAction::Load(
                    self.select_with_history(index, false)
                        .expect("history track is valid"),
                );
            }
        }
        let Some(index) = self.current_index else {
            return PreviousAction::None;
        };
        let mut candidate = index.checked_sub(1);
        let mut wrapped = false;
        loop {
            let Some(previous) = candidate else {
                if self.repeat_mode == RepeatMode::All && !wrapped {
                    wrapped = true;
                    candidate = Some(self.queue.len() - 1);
                    continue;
                }
                self.position = Duration::ZERO;
                return PreviousAction::Restart;
            };
            if !self.explicit_blocked(&self.queue[previous]) {
                return PreviousAction::Load(
                    self.select_with_history(previous, false)
                        .expect("previous index is valid"),
                );
            }
            candidate = previous.checked_sub(1);
        }
    }
    pub(crate) fn seek(&mut self, position: Duration) -> Duration {
        self.position = position.min(self.duration);
        self.position
    }

    pub(crate) fn set_volume(&mut self, volume: f32) -> f32 {
        self.volume = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            0.8
        };
        self.muted = self.volume == 0.0;
        if self.volume > 0.0 {
            self.last_nonzero_volume = self.volume;
        }
        self.volume
    }

    pub(crate) fn toggle_mute(&mut self) -> f32 {
        if self.muted || self.volume == 0.0 {
            self.set_volume(self.last_nonzero_volume.max(0.01))
        } else {
            self.last_nonzero_volume = self.volume;
            self.set_volume(0.0)
        }
    }
    pub(crate) fn restore_volume(&mut self, volume: f32, muted: bool) -> f32 {
        self.set_volume(volume);
        if muted && self.volume > 0.0 {
            self.last_nonzero_volume = self.volume;
            self.volume = 0.0;
            self.muted = true;
        }
        self.volume
    }

    pub(crate) fn persisted_volume(&self) -> f32 {
        if self.muted {
            self.last_nonzero_volume
        } else {
            self.volume
        }
    }

    pub(crate) fn reorder(&mut self, from: usize, to: usize) -> bool {
        let mut upcoming = self.upcoming_indices();
        if from >= upcoming.len() || to >= upcoming.len() || from == to {
            return false;
        }
        let selected = upcoming.remove(from);
        upcoming.insert(to, selected);
        self.set_upcoming_order(&upcoming);
        self.bump_queue_epoch();
        true
    }

    pub(crate) fn remove(&mut self, index: usize) -> bool {
        if index >= self.queue.len() || self.current_index == Some(index) {
            return false;
        }
        self.queue.remove(index);
        adjust_queue_indices(&mut self.play_next, index);
        adjust_queue_indices(&mut self.history, index);
        adjust_queue_indices(&mut self.shuffle_order, index);
        self.shuffle_played = self
            .shuffle_played
            .drain()
            .filter_map(|queued| match queued.cmp(&index) {
                std::cmp::Ordering::Less => Some(queued),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(queued - 1),
            })
            .collect();
        if self.current_index.is_some_and(|current| index < current) {
            self.current_index = self.current_index.map(|current| current - 1);
        }
        self.bump_queue_epoch();
        true
    }

    pub(crate) fn clear(&mut self) {
        self.bump_queue_epoch();
        self.generation = self.generation.wrapping_add(1);
        self.status = PlaybackStatus::Empty;
        self.queue.clear();
        self.current_index = None;
        self.position = Duration::ZERO;
        self.duration = Duration::ZERO;
        self.buffered = Duration::ZERO;
        self.error = None;
        // Like closePlayer's setRightSidebarOpen(false, { persist: false }) in
        // the original app, clearing playback only collapses the sidebar for
        // now. The remembered right_sidebar_open preference stands so the next
        // replace() reopens the panel per that standing preference.
        self.right_sidebar = RightSidebar::Closed;
        self.context = PlaybackContext::None;
        self.lyrics_context = None;
        self.play_next.clear();
        self.history.clear();
        self.shuffle_order.clear();
        self.shuffle_played.clear();
    }

    pub(crate) fn append_tracks(&mut self, additions: Vec<PlaybackTrack>) -> usize {
        if additions.is_empty() {
            return 0;
        }
        let mut existing: HashSet<(PlaybackProvider, String)> = self
            .queue
            .iter()
            .map(|track| (track.provider, track.id.clone()))
            .collect();
        let fresh: Vec<PlaybackTrack> = additions
            .into_iter()
            .filter(|track| existing.insert((track.provider, track.id.clone())))
            .collect();
        let count = fresh.len();
        if count == 0 {
            return 0;
        }
        let first_new = self.queue.len();
        self.queue.extend(fresh);
        if self.shuffle_enabled {
            self.shuffle_order.extend(first_new..self.queue.len());
        }
        self.bump_queue_epoch();
        count
    }

    /// Adds context-menu tracks without collapsing duplicate occurrences.
    ///
    /// The shuffle deck is stored in reverse playback order because navigation
    /// pops from its end. New "last" tracks therefore belong at the front of
    /// that deck, after every track already shown as upcoming. "Next" tracks
    /// become the priority prefix in their source order, matching the original
    /// app's `playNextPriority` behavior.
    pub(crate) fn insert_queue_additions(
        &mut self,
        additions: Vec<PlaybackTrack>,
        last: bool,
    ) -> usize {
        if additions.is_empty() {
            return 0;
        }

        let first_new = self.queue.len();
        let count = additions.len();
        self.queue.extend(additions);
        let new_indices: Vec<usize> = (first_new..first_new + count).collect();

        if self.shuffle_enabled {
            let mut shuffled_additions = new_indices.clone();
            Self::shuffle_indices(&mut shuffled_additions, &mut self.shuffle_seed);
            self.shuffle_order
                .splice(0..0, shuffled_additions.into_iter().rev());
        }

        if !last {
            let mut upcoming = new_indices;
            upcoming.extend(
                self.upcoming_indices()
                    .into_iter()
                    .filter(|index| *index < first_new),
            );
            self.set_upcoming_order(&upcoming);
        }

        self.bump_queue_epoch();
        count
    }

    pub(crate) fn queue_epoch(&self) -> u64 {
        self.queue_epoch
    }

    pub(crate) fn extension_ticket(&self) -> Option<QueueExtensionTicket> {
        self.context.can_extend().then(|| QueueExtensionTicket {
            epoch: self.queue_epoch,
            context: self.context.clone(),
        })
    }

    pub(crate) fn extension_ticket_is_current(&self, ticket: &QueueExtensionTicket) -> bool {
        self.queue_epoch == ticket.epoch && self.context == ticket.context
    }

    pub(crate) fn deezer_library_append_ticket(
        &self,
        load_id: u64,
    ) -> Option<ExactQueueAppendTicket> {
        matches!(
            &self.context,
            PlaybackContext::DeezerLibraryTracks { load_id: active } if *active == load_id
        )
        .then(|| ExactQueueAppendTicket {
            epoch: self.queue_epoch,
            load_id,
            expected_len: self.queue.len(),
            expected_prefix: queue_identity(&self.queue),
        })
    }

    /// Appends only the exact tail of a later snapshot from the same finite
    /// Deezer library load. The captured epoch rejects replacement, removal,
    /// reordering, and unrelated queue edits while the ordered prefix check
    /// makes metadata enrichment safe without collapsing duplicate tracks.
    pub(crate) fn append_deezer_library_exact_tail(
        &mut self,
        ticket: &ExactQueueAppendTicket,
        exact_queue: Vec<PlaybackTrack>,
    ) -> ExactQueueAppend {
        if self.queue_epoch != ticket.epoch
            || self.context
                != (PlaybackContext::DeezerLibraryTracks {
                    load_id: ticket.load_id,
                })
            || self.queue.len() != ticket.expected_len
            || queue_identity(&self.queue) != ticket.expected_prefix
            || exact_queue.len() < ticket.expected_len
            || queue_identity(&exact_queue[..ticket.expected_len]) != ticket.expected_prefix
        {
            return ExactQueueAppend::Stale;
        }

        let resume_index = (self.status == PlaybackStatus::Ended
            && self.current_index == ticket.expected_len.checked_sub(1))
        .then(|| {
            exact_queue[ticket.expected_len..]
                .iter()
                .position(|track| !self.explicit_blocked(track))
                .map(|offset| ticket.expected_len + offset)
        })
        .flatten();
        let additions = exact_queue.into_iter().skip(ticket.expected_len);
        let old_len = self.queue.len();
        self.queue.extend(additions);
        let added = self.queue.len() - old_len;
        if added > 0 {
            if self.shuffle_enabled {
                let mut new_indices: Vec<usize> = (old_len..self.queue.len()).collect();
                Self::shuffle_indices(&mut new_indices, &mut self.shuffle_seed);
                self.shuffle_order
                    .splice(0..0, new_indices.into_iter().rev());
            }
            self.bump_queue_epoch();
        }
        ExactQueueAppend::Applied {
            added,
            resume_index,
        }
    }

    pub(crate) fn replace_context(&mut self, context: PlaybackContext) -> bool {
        if self.context == context {
            return false;
        }
        self.context = context;
        self.bump_queue_epoch();
        true
    }

    /// Apply one successful radio response atomically with its continuation
    /// tuner.  A Flow response can replace only the unplayed remainder while
    /// retaining the current track and history; ordinary responses append in
    /// the order returned by Deezer.
    pub(crate) fn apply_extension(
        &mut self,
        ticket: &QueueExtensionTicket,
        additions: Vec<PlaybackTrack>,
        clear_remaining: bool,
        next_flow_tuner: Option<library::FlowTuner>,
        continuation_seed: Option<String>,
    ) -> ExtensionApply {
        if !self.extension_ticket_is_current(ticket) {
            return ExtensionApply::Stale;
        }

        let added = if clear_remaining {
            self.replace_remaining(additions)
        } else {
            self.append_tracks(additions)
        };
        if let Some(next_seed) = continuation_seed.filter(|seed| !seed.trim().is_empty())
            && let PlaybackContext::SoundCloudStation { seed_track_id } = &mut self.context
            && seed_track_id != &next_seed
        {
            *seed_track_id = next_seed;
            self.bump_queue_epoch();
        }
        if let PlaybackContext::DeezerFlow { tuner, .. } = &mut self.context {
            *tuner = next_flow_tuner;
        }
        ExtensionApply::Applied { added }
    }

    /// Replace the unplayed portion without disturbing audio state. History
    /// can contain a row physically after the current index after a
    /// Previous/queue selection, so recover it by queue position before
    /// discarding the remainder.
    fn replace_remaining(&mut self, additions: Vec<PlaybackTrack>) -> usize {
        let Some(current_index) = self.current_index.filter(|index| *index < self.queue.len())
        else {
            return self.append_tracks(additions);
        };
        let mut preserved = Vec::new();
        let mut seen = HashSet::new();
        let mut history_indices = Vec::new();
        for index in &self.history {
            let Some(track) = self.queue.get(*index).cloned() else {
                continue;
            };
            if *index == current_index || !seen.insert((track.provider, track.id.clone())) {
                continue;
            }
            history_indices.push(preserved.len());
            preserved.push(track);
        }
        let current = self.queue[current_index].clone();
        if seen.insert((current.provider, current.id.clone())) {
            preserved.push(current);
        }

        let preserved_count = preserved.len();
        let mut fresh = Vec::new();
        for track in additions {
            if seen.insert((track.provider, track.id.clone())) {
                fresh.push(track);
            }
        }
        let added = fresh.len();
        preserved.extend(fresh);
        self.queue = preserved;
        self.current_index = preserved_count.checked_sub(1);
        self.history = history_indices;
        self.play_next.clear();
        self.rebuild_shuffle_after_remainder_replace(preserved_count);
        self.bump_queue_epoch();
        added
    }

    pub(crate) fn replace_remaining_tracks(&mut self, additions: Vec<PlaybackTrack>) -> usize {
        self.replace_remaining(additions)
    }

    fn rebuild_shuffle_after_remainder_replace(&mut self, preserved_count: usize) {
        self.shuffle_order.clear();
        if !self.shuffle_enabled {
            return;
        }
        self.shuffle_played.clear();
        if let Some(current) = self.current_index {
            self.shuffle_played.insert(current);
        }
        self.shuffle_order.extend(preserved_count..self.queue.len());
        self.shuffle_pending_order();
    }

    fn bump_queue_epoch(&mut self) {
        self.queue_epoch = self.queue_epoch.wrapping_add(1);
    }

    pub(crate) fn toggle_sidebar(&mut self, sidebar: RightSidebar) -> RightSidebar {
        if self.right_sidebar == sidebar {
            self.right_sidebar = RightSidebar::Closed;
            self.right_sidebar_open = false;
        } else if sidebar != RightSidebar::Closed && self.current().is_none() {
            return self.right_sidebar;
        } else {
            self.right_sidebar = sidebar;
            self.right_sidebar_open = true;
        }
        if sidebar != RightSidebar::Lyrics {
            self.lyrics_context = None;
        }
        if sidebar != RightSidebar::Closed {
            self.right_sidebar_view = sidebar;
        }
        self.right_sidebar
    }

    /// Opens the lyrics sidebar for a track that may not be playing. Playback
    /// is untouched; the panel fetches lyrics for the given context track.
    pub(crate) fn open_lyrics_context(&mut self, track: &PlaybackTrack) {
        if self
            .current()
            .is_some_and(|current| current.provider == track.provider && current.id == track.id)
        {
            self.lyrics_context = None;
        } else {
            self.lyrics_context = Some(track.clone());
        }
        self.right_sidebar = RightSidebar::Lyrics;
        self.right_sidebar_open = true;
        self.right_sidebar_view = RightSidebar::Lyrics;
    }

    /// The track the lyrics sidebar should display: the context track when
    /// one was requested, otherwise the playing track.
    pub(crate) fn lyrics_display_track(&self) -> Option<&PlaybackTrack> {
        self.lyrics_context.as_ref().or_else(|| self.current())
    }

    pub(crate) fn lyrics_follows_playback(&self) -> bool {
        self.lyrics_context.is_none()
    }

    pub(crate) fn close_sidebar(&mut self) {
        self.right_sidebar = RightSidebar::Closed;
        self.right_sidebar_open = false;
        self.lyrics_context = None;
    }

    pub(crate) fn restore_sidebar(&mut self, open: bool, view: RightSidebar) {
        self.right_sidebar_view = view;
        self.right_sidebar_open = open;
        self.right_sidebar = if open && self.current().is_some() {
            view
        } else {
            RightSidebar::Closed
        };
    }

    pub(crate) fn sidebar_preferences(&self) -> (bool, RightSidebar) {
        (self.right_sidebar_open, self.right_sidebar_view)
    }

    pub(crate) fn volume_icon_level(&self) -> VolumeIconLevel {
        if self.muted || self.volume == 0.0 {
            VolumeIconLevel::Muted
        } else if self.volume <= 0.33 {
            VolumeIconLevel::Off
        } else if self.volume <= 0.66 {
            VolumeIconLevel::Low
        } else {
            VolumeIconLevel::High
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VolumeIconLevel {
    Muted,
    Off,
    Low,
    High,
}

pub(crate) enum PreviousAction {
    None,
    Restart,
    Load(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks() -> Vec<PlaybackTrack> {
        (0..3)
            .map(|id| PlaybackTrack {
                downloadable: false,
                progressive: false,
                provider: PlaybackProvider::Deezer,
                id: id.to_string(),
                title: format!("Track {id}"),
                artist: "Artist".into(),
                album: String::new(),
                album_id: String::new(),
                release_date: String::new(),
                artists: Vec::new(),
                artwork: String::new(),
                duration: Duration::from_secs(10),
                explicit: false,
                service_url: String::new(),
            })
            .collect()
    }

    fn fixed_shuffle_queue() -> Vec<PlaybackTrack> {
        let templates = tracks();
        (0..8)
            .map(|id| {
                let mut track = templates[id % templates.len()].clone();
                track.id = id.to_string();
                track.title = format!("Track {id}");
                track
            })
            .collect()
    }

    #[test]
    fn shuffle_seed_normalization_is_nonzero_and_preserves_nonzero_values() {
        assert_eq!(normalize_shuffle_seed(0), SHUFFLE_SEED_FALLBACK);
        assert_eq!(normalize_shuffle_seed(1), 1);
        assert_eq!(normalize_shuffle_seed(u64::MAX), u64::MAX);
    }

    #[test]
    fn shuffle_entropy_folding_uses_both_uuid_halves() {
        let zero = [0u8; 16];
        let mut low = zero;
        low[0] = 1;
        let mut high = zero;
        high[8] = 1;

        assert_eq!(fold_shuffle_entropy(&zero), SHUFFLE_SEED_FALLBACK);
        assert_ne!(fold_shuffle_entropy(&low), fold_shuffle_entropy(&high));
        assert_ne!(fold_shuffle_entropy(&low), 0);
        assert_ne!(fold_shuffle_entropy(&high), 0);
    }

    #[test]
    fn fixed_shuffle_seeds_produce_distinct_complete_orders_with_current_pinned() {
        let queue = fixed_shuffle_queue();

        let mut first = PlaybackState {
            shuffle_seed: 0x0123_4567_89AB_CDEF,
            ..PlaybackState::default()
        };
        let _ = first.replace(queue.clone(), 3);
        first.set_shuffle_enabled(true);
        let first_order = first.upcoming_indices();
        assert_eq!(first_order, vec![5, 0, 2, 6, 4, 1, 7]);
        assert!(!first_order.contains(&3));
        assert_eq!(
            first_order.iter().copied().collect::<HashSet<_>>(),
            (0..8).filter(|index| *index != 3).collect::<HashSet<_>>()
        );

        let mut second = PlaybackState {
            shuffle_seed: 0xFEDC_BA98_7654_3211,
            ..PlaybackState::default()
        };
        let _ = second.replace(queue, 3);
        second.set_shuffle_enabled(true);
        let second_order = second.upcoming_indices();
        assert_eq!(second_order, vec![6, 7, 2, 1, 0, 4, 5]);
        assert!(!second_order.contains(&3));
        assert_ne!(first_order, second_order);
    }

    #[test]
    fn source_conversions_preserve_all_artist_refs_and_ids() {
        let artists = vec![
            crate::search::TrackArtistRef {
                id: "11".into(),
                name: "Primary".into(),
            },
            crate::search::TrackArtistRef {
                id: "22".into(),
                name: "Collaborator".into(),
            },
        ];
        let search_track = crate::search::Track {
            artists: artists.clone(),
            artist: "Primary, Collaborator".into(),
            ..crate::search::Track::default()
        };
        let library_track = crate::library::Track {
            artists: artists.clone(),
            artist: "Primary, Collaborator".into(),
            ..crate::library::Track::default()
        };

        assert_eq!(PlaybackTrack::from_search(&search_track).artists, artists);
        assert_eq!(
            PlaybackTrack::from_library(&library_track, crate::search::Provider::Deezer).artists,
            artists
        );
    }

    fn explicit_tracks(ids: &[usize]) -> Vec<PlaybackTrack> {
        tracks()
            .into_iter()
            .enumerate()
            .map(|(index, mut track)| {
                track.explicit = ids.contains(&index);
                track
            })
            .collect()
    }

    #[test]
    fn playback_preferences_restore_volume_mute_repeat_and_shuffle() {
        let mut state = PlaybackState::default();

        state.restore_volume(0.35, true);
        state.set_repeat_mode(RepeatMode::One);
        state.set_shuffle_enabled(true);

        assert_eq!((state.volume, state.muted), (0.0, true));
        assert_eq!(state.persisted_volume(), 0.35);
        assert_eq!(state.repeat_mode, RepeatMode::One);
        assert!(state.shuffle_enabled);
        assert_eq!(state.toggle_mute(), 0.35);
    }

    #[test]
    fn volume_icons_match_original_thresholds_and_mute_restores_volume() {
        let mut state = PlaybackState::default();
        state.set_volume(0.0);
        assert_eq!(state.volume_icon_level(), VolumeIconLevel::Muted);
        state.set_volume(0.33);
        assert_eq!(state.volume_icon_level(), VolumeIconLevel::Off);
        state.set_volume(0.66);
        assert_eq!(state.volume_icon_level(), VolumeIconLevel::Low);
        state.set_volume(0.67);
        assert_eq!(state.volume_icon_level(), VolumeIconLevel::High);

        assert_eq!(state.toggle_mute(), 0.0);
        assert_eq!(state.toggle_mute(), 0.67);
    }

    #[test]
    fn sidebar_restores_preference_and_persists_toggle_transitions() {
        let mut state = PlaybackState::default();
        state.restore_sidebar(true, RightSidebar::Lyrics);
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Lyrics));

        state.replace(tracks(), 0);
        assert_eq!(state.right_sidebar, RightSidebar::Lyrics);
        state.toggle_sidebar(RightSidebar::Lyrics);
        assert_eq!(state.sidebar_preferences(), (false, RightSidebar::Lyrics));
        state.toggle_sidebar(RightSidebar::Queue);
        assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
        state.clear();
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
    }

    #[test]
    fn closing_the_player_keeps_the_standing_sidebar_preference() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.toggle_sidebar(RightSidebar::Queue);
        assert_eq!(state.right_sidebar, RightSidebar::Queue);

        // closePlayer hides the sidebar without persisting the close; the
        // preference stands and the next track reopens the panel.
        state.clear();
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(state.sidebar_preferences(), (true, RightSidebar::Queue));
        state.replace(tracks(), 0);
        assert_eq!(state.right_sidebar, RightSidebar::Queue);

        // An explicit toggle-close is a real preference change and stays closed.
        state.toggle_sidebar(RightSidebar::Queue);
        state.clear();
        state.replace(tracks(), 0);
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(state.sidebar_preferences(), (false, RightSidebar::Queue));
    }

    #[test]
    fn replacement_loading_and_stale_completion_are_ordered() {
        let mut state = PlaybackState::default();
        let first = state.replace(tracks(), 1).unwrap();
        let second = state.select(2).unwrap();
        assert!(!state.loaded_progressive(first, None));
        assert!(!state.loaded(first, None));
        assert!(state.loaded(second, Some(Duration::from_secs(12))));
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.duration, Duration::from_secs(12));
    }

    #[test]
    fn pending_load_opens_the_bar_blank_and_closes_without_a_track() {
        let mut state = PlaybackState::default();
        assert_eq!(state.status, PlaybackStatus::Empty);

        assert!(state.begin_pending_load());
        assert_eq!(state.status, PlaybackStatus::Loading);
        assert_eq!(state.current_index, None);
        let first_epoch = state.queue_epoch();

        // A newer pending source owns a distinct epoch, so an older request
        // cannot close or populate its player bar.
        assert!(state.begin_pending_load());
        assert_ne!(state.queue_epoch(), first_epoch);

        let pending_epoch = state.queue_epoch();
        assert!(state.abandon_pending_load());
        assert_eq!(state.status, PlaybackStatus::Empty);
        assert_ne!(state.queue_epoch(), pending_epoch);

        // Once a real load selected a track, abandon no longer applies.
        state.begin_pending_load();
        state.replace(tracks(), 0);
        assert_eq!(state.current_index, Some(0));
        assert!(!state.abandon_pending_load());
        assert_eq!(state.status, PlaybackStatus::Loading);
    }

    #[test]
    fn explicit_play_restarts_a_mix_that_is_already_the_active_context() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 1);
        state.replace_context(PlaybackContext::DeezerFlow {
            config_id: "mix-1".into(),
            mode: library::FlowMode::Default,
            tuner: None,
            kind: DeezerFlowKind::SmartMix,
        });

        // Navigating into the already-playing mix keeps the current track.
        assert!(state.should_preserve_flow_current("mix-1", true, true));
        // An explicit play command restarts from the first track.
        assert!(!state.should_preserve_flow_current("mix-1", true, false));
        // A different mix, or a flow that appends, never preserves.
        assert!(!state.should_preserve_flow_current("mix-2", true, true));
        assert!(!state.should_preserve_flow_current("mix-1", false, true));

        // The restart path replaces the queue and selects the first track.
        state.replace(tracks(), 0);
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn pending_load_clears_a_playing_queue_so_the_bar_goes_blank() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 1);
        state.loaded(state.generation, None);
        assert_eq!(state.status, PlaybackStatus::Playing);
        assert_eq!(state.current_index, Some(1));

        assert!(state.begin_pending_load());
        assert_eq!(state.status, PlaybackStatus::Loading);
        assert_eq!(state.current_index, None);
        assert!(state.queue.is_empty());
        assert!(matches!(state.context, PlaybackContext::None));

        // The real load that follows supersedes the pending state cleanly.
        state.replace(tracks(), 0);
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn pending_load_keeps_the_sidebar_closed_until_tracks_exist() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.toggle_sidebar(RightSidebar::Queue);
        assert_eq!(state.right_sidebar, RightSidebar::Queue);

        state.begin_pending_load();
        assert_eq!(state.status, PlaybackStatus::Loading);
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(state.current(), None);
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Lyrics),
            RightSidebar::Closed
        );
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Queue),
            RightSidebar::Closed
        );

        state.replace(tracks(), 0);
        assert_eq!(state.right_sidebar, RightSidebar::Queue);
    }

    #[test]
    fn previous_restarts_after_three_seconds_then_moves_back() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 1);
        state.loaded(state.generation, None);
        state.seek(Duration::from_secs(4));
        assert!(matches!(state.previous(), PreviousAction::Restart));
        assert_eq!(state.current_index, Some(1));
        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn repeated_previous_walks_back_through_history_without_ping_ponging() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        assert!(state.next().is_some());
        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(2));
        assert_eq!(state.history, vec![0, 1]);

        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.history, vec![0]);

        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(0));
        assert!(state.history.is_empty());

        assert!(matches!(state.previous(), PreviousAction::Restart));
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn next_and_end_never_wrap_a_finite_queue() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 1);
        assert!(state.next().is_some());
        assert!(state.next().is_none());
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert_eq!(state.current_index, Some(2));
    }

    #[test]
    fn duplicate_queue_rows_keep_sequential_navigation_occurrence_safe() {
        let mut duplicate = tracks()[0].clone();
        duplicate.title = "Second occurrence".into();
        let queue = vec![tracks()[0].clone(), duplicate, tracks()[1].clone()];
        let mut state = PlaybackState::default();
        state.replace(queue, 0);

        assert_eq!(state.upcoming_indices(), vec![1, 2]);
        state.next();
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.current().unwrap().title, "Second occurrence");
        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(0));

        state.next();
        assert_eq!(state.current_index, Some(1));
        state.next();
        assert_eq!(state.current_index, Some(2));
        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(1));
    }

    #[test]
    fn duplicate_queue_rows_remain_distinct_in_shuffle_and_direct_selection() {
        let mut duplicate = tracks()[0].clone();
        duplicate.title = "Second occurrence".into();
        let mut state = PlaybackState::default();
        state.replace(vec![tracks()[0].clone(), duplicate, tracks()[1].clone()], 2);
        state.set_shuffle_enabled(true);

        let upcoming = state.upcoming_indices();
        assert_eq!(upcoming.len(), 2);
        assert!(upcoming.contains(&0));
        assert!(upcoming.contains(&1));
        assert!(state.select(1).is_some());
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.current().unwrap().title, "Second occurrence");
    }

    #[test]
    fn explicit_blocked_helper_is_pure_and_defaults_off() {
        let mut state = PlaybackState::default();
        let queue = explicit_tracks(&[1]);
        assert!(!state.explicit_blocked(&queue[1]));
        state.skip_explicit = true;
        assert!(state.explicit_blocked(&queue[1]));
        assert!(!state.explicit_blocked(&queue[0]));
    }

    #[test]
    fn select_refuses_blocked_tracks_without_changing_generation() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1]), 0);
        state.set_skip_explicit(true);
        let generation = state.generation;
        assert!(state.select(1).is_none());
        assert_eq!(state.current_index, Some(0));
        assert_eq!(state.generation, generation);
        assert!(state.select(2).is_some());
    }

    #[test]
    fn next_skips_blocked_tracks_sequentially_and_ends_like_exhaustion() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1, 2]), 0);
        state.set_skip_explicit(true);
        assert_eq!(state.current_id(), Some("0"));
        assert!(state.next().is_none());
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert_eq!(state.current_index, Some(0));

        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1]), 0);
        state.set_skip_explicit(true);
        assert_eq!(state.next().map(|_| ()), Some(()));
        assert_eq!(state.current_id(), Some("2"));
        assert!(state.next().is_none());
        assert_eq!(state.status, PlaybackStatus::Ended);
    }

    #[test]
    fn repeat_all_next_wraps_to_the_first_non_blocked_track() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[0, 1]), 2);
        state.set_skip_explicit(true);
        state.set_repeat_mode(RepeatMode::All);
        assert!(state.next().is_some());
        assert_eq!(state.current_id(), Some("2"));
    }

    #[test]
    fn shuffle_next_skips_blocked_tracks_and_repeat_all_redeals() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1, 2]), 0);
        state.set_skip_explicit(true);
        state.set_shuffle_enabled(true);
        assert!(state.next().is_none());
        assert_eq!(state.status, PlaybackStatus::Ended);

        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1]), 0);
        state.set_skip_explicit(true);
        state.set_shuffle_enabled(true);
        state.set_repeat_mode(RepeatMode::All);
        for _ in 0..5 {
            assert!(state.next().is_some());
        }
    }

    #[test]
    fn replace_picks_the_first_playable_track_in_queue_order() {
        let mut state = PlaybackState::default();
        state.set_skip_explicit(true);
        let generation = state.replace(explicit_tracks(&[1]), 1).unwrap();
        assert_eq!(state.current_index, Some(0));
        assert!(state.loaded(generation, None));
    }

    #[test]
    fn replace_clears_when_every_track_is_blocked() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.set_skip_explicit(true);
        assert!(state.replace(explicit_tracks(&[0, 1, 2]), 1).is_none());
        assert_eq!(state.status, PlaybackStatus::Empty);
        assert!(state.queue.is_empty());
        assert_eq!(state.current_index, None);
    }

    #[test]
    fn toggling_skip_explicit_reports_a_blocked_current_track() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1]), 0);
        assert!(!state.set_skip_explicit(true));
        assert!(state.select(1).is_none());

        assert!(!state.set_skip_explicit(false));
        state.replace(explicit_tracks(&[1, 2]), 1);
        assert!(state.set_skip_explicit(true));
    }

    #[test]
    fn repeat_one_moves_past_an_explicit_current_track_when_skipping_is_enabled() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[0]), 0);
        state.set_repeat_mode(RepeatMode::One);
        assert!(state.set_skip_explicit(true));

        let generation = state.generation;
        assert!(state.next().is_some());
        assert_ne!(state.generation, generation);
        assert_eq!(state.current_id(), Some("1"));
        assert!(!state.current().is_some_and(|track| track.explicit));
    }

    #[test]
    fn previous_skips_blocked_history_entries_and_walks_past_blocked_tracks() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[1]), 2);
        state.set_skip_explicit(true);
        state.loaded(state.generation, None);
        state.seek(Duration::from_secs(4));
        assert!(matches!(state.previous(), PreviousAction::Restart));
        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(0));
    }

    #[test]
    fn previous_with_repeat_all_wraps_once_past_blocked_tracks() {
        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[0, 2]), 1);
        state.set_skip_explicit(true);
        state.set_repeat_mode(RepeatMode::All);
        assert!(matches!(state.previous(), PreviousAction::Load(_)));
        assert_eq!(state.current_index, Some(1));

        let mut state = PlaybackState::default();
        state.replace(explicit_tracks(&[0, 1, 2]), 2);
        state.set_skip_explicit(true);
        state.set_repeat_mode(RepeatMode::All);
        assert!(matches!(state.previous(), PreviousAction::Restart));
        assert_eq!(state.current_index, Some(2));
    }

    #[test]
    fn removal_preserves_current_track_and_adjusts_its_index() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 1);
        assert!(!state.remove(1));
        assert!(state.remove(0));
        assert_eq!(state.current_index, Some(0));
        assert_eq!(state.current().unwrap().id, "1");
    }

    #[test]
    fn repeated_play_next_is_last_action_first_without_duplicates() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);

        assert!(state.enqueue_next(2));
        assert!(state.enqueue_next(1));
        assert_eq!(state.upcoming_indices(), vec![1, 2]);
        assert!(state.enqueue_next(2));
        assert_eq!(state.upcoming_indices(), vec![2, 1]);

        state.next();
        assert_eq!(state.current_id(), Some("2"));
        state.next();
        assert_eq!(state.current_id(), Some("1"));
    }

    #[test]
    fn play_last_moves_an_existing_track_to_the_end() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);

        assert!(state.enqueue_last(1));
        assert_eq!(state.upcoming_indices(), vec![2, 1]);
        assert_eq!(
            state.queue.iter().filter(|track| track.id == "1").count(),
            1
        );
        assert!(!state.enqueue_last(0));
    }

    #[test]
    fn shuffled_play_next_registers_new_duplicate_occurrences_in_source_order() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.set_shuffle_enabled(true);
        let previous_upcoming = state.upcoming_indices();
        let previous_epoch = state.queue_epoch();

        let mut current_duplicate = state.queue[0].clone();
        current_duplicate.title = "Current duplicate".into();
        let mut second_duplicate = state.queue[1].clone();
        second_duplicate.title = "Second duplicate".into();
        assert_eq!(
            state.insert_queue_additions(vec![current_duplicate, second_duplicate], false),
            2
        );

        let upcoming = state.upcoming_indices();
        assert_eq!(&upcoming[..2], &[3, 4]);
        assert_eq!(&upcoming[2..], previous_upcoming);
        assert_eq!(state.current_index, Some(0));
        assert_ne!(state.queue_epoch(), previous_epoch);

        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(3));
        assert_eq!(state.current().unwrap().title, "Current duplicate");
        assert!(state.next().is_some());
        assert_eq!(state.current_index, Some(4));
        assert_eq!(state.current().unwrap().title, "Second duplicate");
        for expected in previous_upcoming {
            assert!(state.next().is_some());
            assert_eq!(state.current_index, Some(expected));
        }
        assert!(state.next().is_none());
    }

    #[test]
    fn shuffled_play_last_runs_after_the_existing_deck_and_survives_repeat_all() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.set_shuffle_enabled(true);
        state.set_repeat_mode(RepeatMode::All);
        let previous_upcoming = state.upcoming_indices();

        let mut duplicate = state.queue[1].clone();
        duplicate.title = "Duplicate occurrence".into();
        let mut another = state.queue[2].clone();
        another.title = "Another occurrence".into();
        assert_eq!(
            state.insert_queue_additions(vec![duplicate, another], true),
            2
        );

        let upcoming = state.upcoming_indices();
        assert_eq!(&upcoming[..previous_upcoming.len()], previous_upcoming);
        let added_order = upcoming[previous_upcoming.len()..].to_vec();
        assert_eq!(
            added_order.iter().copied().collect::<HashSet<_>>(),
            HashSet::from([3, 4])
        );

        for expected in previous_upcoming {
            assert!(state.next().is_some());
            assert_eq!(state.current_index, Some(expected));
        }
        for expected in added_order {
            assert!(state.next().is_some());
            assert_eq!(state.current_index, Some(expected));
        }
        assert!(state.next().is_some());
    }

    #[test]
    fn shuffled_existing_queue_actions_move_physical_occurrences_only() {
        let mut duplicate = tracks()[1].clone();
        duplicate.title = "Duplicate occurrence".into();
        let mut state = PlaybackState::default();
        state.replace(
            vec![
                tracks()[0].clone(),
                tracks()[1].clone(),
                duplicate,
                tracks()[2].clone(),
            ],
            0,
        );
        state.set_shuffle_enabled(true);

        assert!(state.enqueue_next(2));
        assert_eq!(state.upcoming_indices().first(), Some(&2));
        assert_eq!(state.current_index, Some(0));
        assert!(state.enqueue_last(2));
        assert_eq!(state.upcoming_indices().last(), Some(&2));
        assert_eq!(state.queue[1].title, "Track 1");
        assert_eq!(state.queue[2].title, "Duplicate occurrence");

        assert!(state.remove(1));
        assert_eq!(state.upcoming_indices().last(), Some(&1));
        assert_eq!(state.queue[1].title, "Duplicate occurrence");
    }

    #[test]
    fn reorder_preserves_current_and_controls_the_normal_future_order() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);

        assert!(state.reorder(1, 0));
        assert_eq!(state.current_id(), Some("0"));
        assert_eq!(state.upcoming_indices(), vec![2, 1]);
        assert!(!state.reorder(0, 0));
        state.next();
        assert_eq!(state.current_id(), Some("2"));
        state.next();
        assert_eq!(state.current_id(), Some("1"));
    }

    #[test]
    fn reorder_controls_shuffle_and_repeat_all_future_order() {
        let mut shuffled = PlaybackState::default();
        shuffled.replace(tracks(), 0);
        shuffled.set_shuffle_enabled(true);
        assert!(shuffled.reorder(1, 0));
        let expected = shuffled.upcoming_indices();
        shuffled.next();
        assert_eq!(shuffled.current_index, Some(expected[0]));

        let mut repeated = PlaybackState::default();
        repeated.replace(tracks(), 1);
        repeated.set_repeat_mode(RepeatMode::All);
        assert_eq!(repeated.upcoming_indices(), vec![2, 0]);
        assert!(repeated.reorder(1, 0));
        assert_eq!(repeated.current_id(), Some("1"));
        assert_eq!(repeated.upcoming_indices(), vec![0, 2]);
        repeated.next();
        assert_eq!(repeated.current_id(), Some("0"));
    }

    #[test]
    fn buffered_fraction_clamps_and_guards_zero_duration() {
        let mut state = PlaybackState::default();
        state.set_buffered_fraction(0.75);
        assert_eq!(state.buffered, Duration::ZERO);
        let generation = state.replace(tracks(), 0).unwrap();
        assert!(state.loaded(generation, Some(Duration::from_secs(10))));
        state.set_buffered_fraction(0.5);
        assert_eq!(state.buffered, Duration::from_secs(5));
        state.set_buffered_fraction(2.0);
        assert_eq!(state.buffered, Duration::from_secs(10));
        state.set_buffered_fraction(-1.0);
        assert_eq!(state.buffered, Duration::ZERO);
    }

    #[test]
    fn progressive_load_preserves_partial_buffer_and_full_load_fills_it() {
        let mut state = PlaybackState::default();
        let generation = state.replace(tracks(), 0).unwrap();
        state.set_buffered_fraction(0.25);
        assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
        assert_eq!(state.buffered, Duration::from_secs(5));

        let mut full = PlaybackState::default();
        let generation = full.replace(tracks(), 0).unwrap();
        assert!(full.loaded_fully_buffered(generation, Some(Duration::from_secs(20))));
        assert_eq!(full.buffered, Duration::from_secs(20));
    }

    #[test]
    fn stopping_a_loading_track_leaves_a_stable_retryable_state() {
        let mut state = PlaybackState::default();
        let generation = state.replace(tracks(), 0).unwrap();
        assert_eq!(state.status, PlaybackStatus::Loading);
        assert!(state.stop_loading());
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert_eq!(state.position, Duration::ZERO);
        assert_eq!(state.buffered, Duration::ZERO);
        assert!(state.error.is_none());
        assert!(!state.stop_loading());
        assert_ne!(state.generation, generation);
    }

    #[test]
    fn seek_volume_and_mute_are_clamped_and_restored() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        assert_eq!(state.seek(Duration::from_secs(20)), Duration::from_secs(10));
        assert_eq!(state.set_volume(2.0), 1.0);
        assert_eq!(state.toggle_mute(), 0.0);
        assert_eq!(state.toggle_mute(), 1.0);
        assert_eq!(state.set_volume(f32::NAN), 0.8);
    }

    #[test]
    fn clear_invalidates_pending_selection() {
        let mut state = PlaybackState::default();
        let generation = state.replace(tracks(), 0).unwrap();
        state.clear();
        assert!(!state.loaded(generation, None));
        assert_eq!(state.status, PlaybackStatus::Empty);
    }

    #[test]
    fn lyrics_are_closed_by_default_and_toggle_only_with_a_track() {
        let mut state = PlaybackState::default();
        assert_eq!(state.right_sidebar, RightSidebar::Closed);
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Lyrics),
            RightSidebar::Closed
        );
        state.replace(tracks(), 0);
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Lyrics),
            RightSidebar::Lyrics
        );
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Queue),
            RightSidebar::Queue
        );
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Queue),
            RightSidebar::Closed
        );
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Lyrics),
            RightSidebar::Lyrics
        );
        assert_eq!(
            state.toggle_sidebar(RightSidebar::Lyrics),
            RightSidebar::Closed
        );
    }

    #[test]
    fn context_lyrics_display_a_non_playing_track_without_touching_playback() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        let context = PlaybackTrack {
            id: "99".into(),
            ..tracks().into_iter().next().unwrap()
        };

        state.open_lyrics_context(&context);
        assert_eq!(state.right_sidebar, RightSidebar::Lyrics);
        assert_eq!(state.lyrics_display_track(), Some(&context));
        assert!(!state.lyrics_follows_playback());
        assert_eq!(state.current_id(), Some("0"));

        // A selection change hands the panel back to the playing track.
        state.select(1);
        assert_eq!(
            state.lyrics_display_track().map(|t| t.id.as_str()),
            Some("1")
        );
        assert!(state.lyrics_follows_playback());

        // Requesting lyrics for the playing track keeps following playback.
        state.open_lyrics_context(&tracks()[1]);
        assert_eq!(
            state.lyrics_display_track().map(|t| t.id.as_str()),
            Some("1")
        );
        assert!(state.lyrics_follows_playback());

        // Closing or switching the sidebar drops the context track.
        state.open_lyrics_context(&context);
        state.toggle_sidebar(RightSidebar::Queue);
        assert!(state.lyrics_follows_playback());
    }

    #[test]
    fn first_upcoming_index_matches_first_element_of_upcoming_indices() {
        let mut state = PlaybackState::default();
        assert_eq!(state.first_upcoming_index(), None);

        state.replace(tracks(), 0);
        assert_eq!(
            state.first_upcoming_index(),
            state.upcoming_indices().first().copied()
        );
        assert_eq!(state.first_upcoming_index(), Some(1));

        state.enqueue_next(2);
        assert_eq!(
            state.first_upcoming_index(),
            state.upcoming_indices().first().copied()
        );
        assert_eq!(state.first_upcoming_index(), Some(2));

        state.set_shuffle_enabled(true);
        assert_eq!(
            state.first_upcoming_index(),
            state.upcoming_indices().first().copied()
        );

        let mut repeat_state = PlaybackState::default();
        repeat_state.replace(tracks(), 2);
        assert_eq!(repeat_state.first_upcoming_index(), None);
        repeat_state.set_repeat_mode(RepeatMode::All);
        assert_eq!(repeat_state.first_upcoming_index(), Some(0));
        assert_eq!(
            repeat_state.first_upcoming_index(),
            repeat_state.upcoming_indices().first().copied()
        );
    }

    #[test]
    fn contexts_are_infinite_but_flow_requires_a_returned_tuner() {
        let flow_without_tuner = PlaybackContext::DeezerFlow {
            config_id: "flow".into(),
            mode: library::FlowMode::Discovery,
            tuner: None,
            kind: DeezerFlowKind::Flow,
        };
        assert!(flow_without_tuner.is_infinite());
        assert!(!flow_without_tuner.can_extend());
        let smart_mix = PlaybackContext::DeezerFlow {
            config_id: "inspired-by-3".into(),
            mode: library::FlowMode::Default,
            tuner: Some(library::FlowTuner::initial(library::FlowMode::Default)),
            kind: DeezerFlowKind::SmartMix,
        };
        assert!(!smart_mix.is_infinite());
        assert!(!smart_mix.can_extend());
        assert!(
            PlaybackContext::DeezerTrackMix {
                seed_track_id: "1".into()
            }
            .can_extend()
        );
        assert!(
            PlaybackContext::DeezerArtistMix {
                seed_artist_id: "2".into()
            }
            .is_infinite()
        );
        let library = PlaybackContext::DeezerLibraryTracks { load_id: 7 };
        assert!(!library.is_infinite());
        assert!(!library.can_extend());
    }

    #[test]
    fn exact_library_tail_preserves_duplicates_order_and_current_audio_state() {
        let mut state = PlaybackState::default();
        state.replace(vec![tracks()[0].clone(), tracks()[1].clone()], 1);
        state.loaded(state.generation, Some(Duration::from_secs(20)));
        state.position = Duration::from_secs(7);
        state.buffered = Duration::from_secs(13);
        state.history.push(0);
        state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 41 });
        let ticket = state.deezer_library_append_ticket(41).unwrap();
        let generation = state.generation;
        let status = state.status;
        let index = state.current_index;
        let position = state.position;
        let duration = state.duration;
        let buffered = state.buffered;
        let history = state.history.clone();
        let mut enriched_prefix = tracks()[0].clone();
        enriched_prefix.title = "metadata changed".into();
        let duplicate = tracks()[1].clone();
        let tail = tracks()[2].clone();

        assert_eq!(
            state.append_deezer_library_exact_tail(
                &ticket,
                vec![enriched_prefix, tracks()[1].clone(), duplicate, tail],
            ),
            ExactQueueAppend::Applied {
                added: 2,
                resume_index: None,
            }
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["0", "1", "1", "2"]
        );
        assert_eq!(state.queue[0].title, tracks()[0].title);
        assert_eq!(state.generation, generation);
        assert_eq!(state.status, status);
        assert_eq!(state.current_index, index);
        assert_eq!(state.position, position);
        assert_eq!(state.duration, duration);
        assert_eq!(state.buffered, buffered);
        assert_eq!(state.history, history);
    }

    #[test]
    fn exact_library_tail_rejects_stale_context_epoch_length_and_prefix() {
        let partial = vec![tracks()[0].clone(), tracks()[1].clone()];
        let full = tracks();

        let mut replaced = PlaybackState::default();
        replaced.replace(partial.clone(), 0);
        replaced.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 1 });
        assert!(replaced.deezer_library_append_ticket(99).is_none());
        let replacement_ticket = replaced.deezer_library_append_ticket(1).unwrap();
        replaced.replace(vec![tracks()[2].clone()], 0);
        assert_eq!(
            replaced.append_deezer_library_exact_tail(&replacement_ticket, full.clone()),
            ExactQueueAppend::Stale
        );

        let mut reordered = PlaybackState::default();
        let mut reordered_full = full.clone();
        reordered_full.push(PlaybackTrack {
            id: "3".into(),
            ..tracks()[2].clone()
        });
        reordered.replace(full.clone(), 0);
        reordered.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 2 });
        let reorder_ticket = reordered.deezer_library_append_ticket(2).unwrap();
        assert!(reordered.reorder(0, 1));
        assert_eq!(
            reordered.append_deezer_library_exact_tail(&reorder_ticket, reordered_full),
            ExactQueueAppend::Stale
        );

        let mut wrong_context = PlaybackState::default();
        wrong_context.replace(partial.clone(), 0);
        wrong_context.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 3 });
        let context_ticket = wrong_context.deezer_library_append_ticket(3).unwrap();
        wrong_context.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 4 });
        assert_eq!(
            wrong_context.append_deezer_library_exact_tail(&context_ticket, full.clone()),
            ExactQueueAppend::Stale
        );

        let mut changed_length = PlaybackState::default();
        changed_length.replace(partial.clone(), 0);
        changed_length.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 5 });
        let length_ticket = changed_length.deezer_library_append_ticket(5).unwrap();
        changed_length.queue.push(tracks()[2].clone());
        assert_eq!(
            changed_length.append_deezer_library_exact_tail(&length_ticket, full.clone()),
            ExactQueueAppend::Stale
        );

        let mut wrong_prefix = PlaybackState::default();
        wrong_prefix.replace(partial.clone(), 0);
        wrong_prefix.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 6 });
        let prefix_ticket = wrong_prefix.deezer_library_append_ticket(6).unwrap();
        assert_eq!(
            wrong_prefix.append_deezer_library_exact_tail(
                &prefix_ticket,
                vec![
                    tracks()[1].clone(),
                    tracks()[0].clone(),
                    tracks()[2].clone()
                ],
            ),
            ExactQueueAppend::Stale
        );

        let mut replayed = PlaybackState::default();
        replayed.replace(partial, 0);
        replayed.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 7 });
        let replay_ticket = replayed.deezer_library_append_ticket(7).unwrap();
        assert_eq!(
            replayed.append_deezer_library_exact_tail(&replay_ticket, full.clone()),
            ExactQueueAppend::Applied {
                added: 1,
                resume_index: None,
            }
        );
        assert_eq!(
            replayed.append_deezer_library_exact_tail(&replay_ticket, full),
            ExactQueueAppend::Stale
        );
    }

    #[test]
    fn exact_library_tail_reports_when_an_ended_tail_can_resume() {
        let mut state = PlaybackState::default();
        state.replace(vec![tracks()[0].clone()], 0);
        state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 8 });
        assert!(state.next().is_none());
        let ticket = state.deezer_library_append_ticket(8).unwrap();
        let generation = state.generation;
        assert_eq!(
            state.append_deezer_library_exact_tail(
                &ticket,
                vec![
                    tracks()[0].clone(),
                    tracks()[0].clone(),
                    tracks()[2].clone()
                ],
            ),
            ExactQueueAppend::Applied {
                added: 2,
                resume_index: Some(1),
            }
        );
        assert_eq!(state.generation, generation);
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert!(state.select(1).is_some());
        assert_eq!(state.current_index, Some(1));
    }

    #[test]
    fn exact_library_tail_joins_the_remaining_shuffle_deck() {
        let make_track = |index: usize| PlaybackTrack {
            id: index.to_string(),
            title: format!("Track {index}"),
            ..tracks()[0].clone()
        };
        let full: Vec<_> = (0..24).map(make_track).collect();
        let mut state = PlaybackState::default();
        state.replace(full[..3].to_vec(), 0);
        state.shuffle_seed = SHUFFLE_SEED_FALLBACK;
        state.set_shuffle_enabled(true);
        state.replace_context(PlaybackContext::DeezerLibraryTracks { load_id: 9 });
        state.play_next.push(1);
        let ticket = state.deezer_library_append_ticket(9).unwrap();
        let existing_upcoming = state.upcoming_indices();
        assert_eq!(existing_upcoming, vec![1, 2]);

        assert_eq!(
            state.append_deezer_library_exact_tail(&ticket, full.clone()),
            ExactQueueAppend::Applied {
                added: 21,
                resume_index: None,
            }
        );
        let upcoming = state.upcoming_indices();
        let expected_tail = vec![
            8, 7, 17, 20, 9, 12, 11, 21, 4, 23, 10, 13, 18, 22, 15, 5, 14, 3, 6, 16, 19,
        ];
        assert_eq!(
            &upcoming[..existing_upcoming.len()],
            existing_upcoming.as_slice()
        );
        assert_eq!(
            &upcoming[existing_upcoming.len()..],
            expected_tail.as_slice()
        );
        assert!(!upcoming.contains(&state.current_index.unwrap()));
        assert_eq!(upcoming.len(), full.len() - 1);
        assert_eq!(
            upcoming.iter().copied().collect::<HashSet<_>>(),
            (1..full.len()).collect::<HashSet<_>>()
        );
    }

    #[test]
    fn stale_extension_ticket_cannot_touch_a_replaced_queue() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 0);
        state.replace_context(PlaybackContext::DeezerTrackMix {
            seed_track_id: "0".into(),
        });
        let ticket = state.extension_ticket().unwrap();
        state.replace(vec![tracks()[2].clone()], 0);
        assert_eq!(
            state.apply_extension(&ticket, vec![tracks()[1].clone()], false, None, None),
            ExtensionApply::Stale
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["2"]
        );
    }

    #[test]
    fn clear_remaining_preserves_history_and_current_without_audio_reset() {
        let mut state = PlaybackState::default();
        state.replace(tracks(), 2);
        state.select(0);
        state.loaded(state.generation, Some(Duration::from_secs(10)));
        state.position = Duration::from_secs(3);
        state.buffered = Duration::from_secs(5);
        state.context = PlaybackContext::DeezerFlow {
            config_id: "flow".into(),
            mode: library::FlowMode::Default,
            tuner: Some(library::FlowTuner::initial(library::FlowMode::Default)),
            kind: DeezerFlowKind::Flow,
        };
        let ticket = state.extension_ticket().unwrap();
        let generation = state.generation;
        let status = state.status;
        let position = state.position;
        let duration = state.duration;
        let buffered = state.buffered;
        assert_eq!(
            state.apply_extension(
                &ticket,
                vec![
                    tracks()[1].clone(),
                    PlaybackTrack {
                        id: "new".into(),
                        ..tracks()[1].clone()
                    }
                ],
                true,
                Some(library::FlowTuner::initial(library::FlowMode::Discovery)),
                None,
            ),
            ExtensionApply::Applied { added: 2 }
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "0", "1", "new"]
        );
        assert_eq!(state.current_index, Some(1));
        assert_eq!(state.history, vec![0]);
        assert_eq!(state.generation, generation);
        assert_eq!(state.status, status);
        assert_eq!(state.position, position);
        assert_eq!(state.duration, duration);
        assert_eq!(state.buffered, buffered);
        match state.context {
            PlaybackContext::DeezerFlow { mode, tuner, .. } => {
                assert_eq!(mode, library::FlowMode::Default);
                assert_eq!(
                    tuner.unwrap().as_str(),
                    library::FlowTuner::initial(library::FlowMode::Discovery).as_str()
                );
            }
            _ => panic!("expected Flow context"),
        }
    }

    #[test]
    fn extension_appends_new_tracks_once_in_response_order() {
        let mut state = PlaybackState::default();
        state.replace(vec![tracks()[0].clone(), tracks()[1].clone()], 0);
        state.replace_context(PlaybackContext::DeezerTrackMix {
            seed_track_id: "0".into(),
        });
        let ticket = state.extension_ticket().unwrap();
        let mut duplicate = tracks()[1].clone();
        duplicate.title = "duplicate".into();
        let mut fresh = tracks()[2].clone();
        fresh.title = "fresh".into();
        assert_eq!(
            state.apply_extension(
                &ticket,
                vec![duplicate.clone(), fresh.clone(), duplicate],
                false,
                None,
                None,
            ),
            ExtensionApply::Applied { added: 1 }
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["0", "1", "2"]
        );
        assert_eq!(state.queue[2].title, "fresh");
    }

    #[test]
    fn queue_epoch_advances_when_extension_changes_the_queue() {
        let mut state = PlaybackState::default();
        state.replace(vec![tracks()[0].clone()], 0);
        state.replace_context(PlaybackContext::DeezerTrackMix {
            seed_track_id: "0".into(),
        });
        let ticket = state.extension_ticket().unwrap();
        let epoch = state.queue_epoch();

        assert_eq!(
            state.apply_extension(&ticket, vec![tracks()[1].clone()], false, None, None),
            ExtensionApply::Applied { added: 1 }
        );
        assert_ne!(state.queue_epoch(), epoch);
        assert!(!state.extension_ticket_is_current(&ticket));
    }

    #[test]
    fn soundcloud_station_extension_advances_to_the_raw_response_seed() {
        let mut seed = tracks()[0].clone();
        seed.provider = PlaybackProvider::SoundCloud;
        seed.id = "1957877095".into();
        let mut prior_track = tracks()[1].clone();
        prior_track.provider = PlaybackProvider::SoundCloud;
        prior_track.id = "986413711".into();
        let mut state = PlaybackState::default();
        state.replace(vec![prior_track.clone(), seed.clone()], 0);
        state.replace_context(PlaybackContext::SoundCloudStation {
            seed_track_id: "1957877095".into(),
        });
        let ticket = state.extension_ticket().unwrap();
        let previous_epoch = state.queue_epoch();

        let mut duplicate = seed;
        duplicate.title = "duplicate seed".into();
        let mut final_track = duplicate.clone();
        final_track.id = "2132666208".into();
        final_track.title = "final station track".into();
        let raw_response_seed = prior_track.id.clone();

        assert_eq!(
            state.apply_extension(
                &ticket,
                vec![duplicate, final_track, prior_track],
                false,
                None,
                Some(raw_response_seed.clone()),
            ),
            ExtensionApply::Applied { added: 1 }
        );
        assert_eq!(
            state
                .queue
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["986413711", "1957877095", "2132666208"]
        );
        assert_eq!(
            state.context,
            PlaybackContext::SoundCloudStation {
                seed_track_id: raw_response_seed.clone(),
            }
        );
        assert_ne!(state.queue_epoch(), previous_epoch);
        assert!(!state.extension_ticket_is_current(&ticket));
        assert_eq!(
            state.extension_ticket().unwrap().context,
            PlaybackContext::SoundCloudStation {
                seed_track_id: raw_response_seed,
            }
        );
    }

    #[test]
    fn can_next_reports_false_on_last_track_without_repeat() {
        let mut state = PlaybackState::default();
        let track_list = tracks();
        state.replace(track_list.clone(), 0);
        assert!(state.can_next());

        // Move to the last track
        state.select(track_list.len() - 1);
        assert!(!state.can_next());

        // With repeat all, can_next is true
        state.repeat_mode = RepeatMode::All;
        assert!(state.can_next());

        // With repeat one, can_next is true
        state.repeat_mode = RepeatMode::One;
        assert!(state.can_next());

        // With repeat off but an index in play_next
        state.repeat_mode = RepeatMode::Off;
        assert!(!state.can_next());
        state.play_next.push(0);
        assert!(state.can_next());
    }
}
