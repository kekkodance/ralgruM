use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

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
    pub(crate) ai_generated: bool,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id when a link is needed.
    pub(crate) service_url: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ContentPreferences {
    pub(crate) block_explicit: bool,
    pub(crate) block_ai: bool,
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
            ai_generated: track.ai_generated,
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
            ai_generated: track.ai_generated,
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
    pub(crate) suffix_buffered: Option<(Duration, Duration)>,
    pub(crate) volume: f32,
    pub(crate) muted: bool,
    pub(crate) repeat_mode: RepeatMode,
    pub(crate) shuffle_enabled: bool,
    pub(crate) skip_explicit: bool,
    pub(crate) block_ai: bool,
    pub(crate) error: Option<String>,
    pub(crate) generation: u64,
    pub(crate) right_sidebar: RightSidebar,
    pub(crate) right_sidebar_view: RightSidebar,
    pub(crate) context: PlaybackContext,
    queue_epoch: u64,
    // A SmartMix page request exists before the transport has a track to
    // load. Keep that UI state separate from the audio-loading status so a
    // transport lifecycle update cannot hide the player bar mid-request.
    pending_queue_load: bool,
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
            suffix_buffered: None,
            volume: 0.8,
            muted: false,
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            skip_explicit: false,
            block_ai: false,
            error: None,
            generation: 0,
            right_sidebar: RightSidebar::Closed,
            right_sidebar_view: RightSidebar::Lyrics,
            context: PlaybackContext::None,
            queue_epoch: 0,
            pending_queue_load: false,
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
    pub(crate) fn apply_deezer_ai_content(&mut self, albums: &HashMap<String, bool>) -> bool {
        let mut changed = false;
        for track in &mut self.queue {
            if track.provider != PlaybackProvider::Deezer || track.album_id.is_empty() {
                continue;
            }
            let Some(ai_generated) = albums.get(&track.album_id).copied() else {
                continue;
            };
            if track.ai_generated != ai_generated {
                track.ai_generated = ai_generated;
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn current(&self) -> Option<&PlaybackTrack> {
        self.current_index.and_then(|index| self.queue.get(index))
    }

    pub(crate) fn content_blocked(&self, track: &PlaybackTrack) -> bool {
        self.explicit_blocked(track) || self.ai_blocked(track)
    }

    pub(crate) fn explicit_blocked(&self, track: &PlaybackTrack) -> bool {
        self.skip_explicit && track.explicit
    }

    pub(crate) fn ai_blocked(&self, track: &PlaybackTrack) -> bool {
        self.block_ai && track.ai_generated
    }

    pub(crate) fn set_skip_explicit(&mut self, enabled: bool) -> bool {
        self.skip_explicit = enabled;
        self.current()
            .is_some_and(|track| self.content_blocked(track))
    }

    pub(crate) fn set_block_ai(&mut self, enabled: bool) -> bool {
        self.block_ai = enabled;
        self.current()
            .is_some_and(|track| self.content_blocked(track))
    }

    #[cfg(test)]
    pub(crate) fn current_id(&self) -> Option<&str> {
        self.current().map(|track| track.id.as_str())
    }

    pub(crate) fn replace(&mut self, queue: Vec<PlaybackTrack>, index: usize) -> Option<u64> {
        self.bump_queue_epoch();
        if queue.is_empty() || index >= queue.len() {
            self.clear();
            return None;
        }
        let index = if self.content_blocked(&queue[index]) {
            match queue.iter().position(|track| !self.content_blocked(track)) {
                Some(first_playable) => first_playable,
                None => {
                    self.clear();
                    return None;
                }
            }
        } else {
            index
        };
        self.pending_queue_load = false;
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

    /// Re-resolve the current source after an output change without treating
    /// it as a new queue selection. Navigation history, play-next order, and
    /// the lyrics context belong to the track and must survive the reload.
    pub(crate) fn reload_current_source(&mut self) -> Option<u64> {
        self.current()?;
        self.generation = self.generation.wrapping_add(1);
        self.begin_loading();
        Some(self.generation)
    }

    fn select_with_history(&mut self, index: usize, record_current: bool) -> Option<u64> {
        if index >= self.queue.len() || self.content_blocked(&self.queue[index]) {
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
        self.suffix_buffered = None;
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
        if fully_buffered {
            self.suffix_buffered = None;
        }
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
        self.suffix_buffered = None;
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
        self.suffix_buffered = None;
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
        self.pending_queue_load = true;
        self.status = PlaybackStatus::Loading;
        true
    }

    pub(crate) fn player_bar_open(&self) -> bool {
        self.pending_queue_load || self.status != PlaybackStatus::Empty
    }

    pub(crate) fn pending_queue_load(&self) -> bool {
        self.pending_queue_load
    }

    /// Close the player bar again when a pending load never produced a
    /// track. Loads that already selected a track are left alone.
    pub(crate) fn abandon_pending_load(&mut self) -> bool {
        if !self.pending_queue_load || self.current_index.is_some() {
            return false;
        }
        self.bump_queue_epoch();
        self.generation = self.generation.wrapping_add(1);
        self.pending_queue_load = false;
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
                .is_some_and(|track| !self.content_blocked(track))
        }) {
            return true;
        }
        if self.shuffle_enabled {
            return self.shuffle_order.iter().any(|&index| {
                self.queue
                    .get(index)
                    .is_some_and(|track| !self.content_blocked(track))
            });
        }
        let Some(current) = self.current_index else {
            return false;
        };
        (current + 1..self.queue.len()).any(|index| !self.content_blocked(&self.queue[index]))
    }

    pub(crate) fn next(&mut self) -> Option<u64> {
        if self.repeat_mode == RepeatMode::One
            && !self
                .current()
                .is_some_and(|track| self.content_blocked(track))
        {
            return self.begin_selection();
        }
        let next_index = self
            .next_play_next_index()
            .or_else(|| self.next_queue_index());
        let Some(index) = next_index else {
            self.status = PlaybackStatus::Ended;
            self.position = self.duration;
            self.suffix_buffered = None;
            return None;
        };
        self.select(index)
    }

    fn next_play_next_index(&mut self) -> Option<usize> {
        while let Some(index) = self.play_next.pop() {
            if self
                .queue
                .get(index)
                .is_some_and(|track| !self.content_blocked(track))
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
            if !self.content_blocked(&self.queue[index]) {
                return Some(index);
            }
        }
        (self.repeat_mode == RepeatMode::All)
            .then(|| (0..self.queue.len()).find(|index| !self.content_blocked(&self.queue[*index])))
            .flatten()
    }

    fn next_shuffle_index(&mut self) -> Option<usize> {
        while let Some(index) = self.shuffle_order.pop() {
            if self
                .queue
                .get(index)
                .is_some_and(|track| !self.content_blocked(track))
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
                .is_some_and(|track| !self.content_blocked(track))
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
            if !self.content_blocked(&self.queue[previous]) {
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
        self.suffix_buffered = None;
        self.error = None;
        // Like closePlayer's setRightSidebarOpen(false, { persist: false }) in
        // the original app, clearing playback only collapses the sidebar for
        // now. The remembered right_sidebar_open preference stands so the next
        // replace() reopens the panel per that standing preference.
        self.right_sidebar = RightSidebar::Closed;
        self.context = PlaybackContext::None;
        self.pending_queue_load = false;
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
                .position(|track| !self.content_blocked(track))
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
    /// tuner.  Extensions append in the order returned by the provider, so
    /// every previously listed unplayed track stays above the fresh batch,
    /// matching SoundCloud station queues.
    pub(crate) fn apply_extension(
        &mut self,
        ticket: &QueueExtensionTicket,
        additions: Vec<PlaybackTrack>,
        next_flow_tuner: Option<library::FlowTuner>,
        continuation_seed: Option<String>,
    ) -> ExtensionApply {
        if !self.extension_ticket_is_current(ticket) {
            return ExtensionApply::Stale;
        }

        let added = self.append_tracks(additions);
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
#[path = "state_tests.rs"]
mod tests;
