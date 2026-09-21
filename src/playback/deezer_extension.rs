use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use super::{PlaybackContext, PlaybackState};

pub(crate) const MAX_DUPLICATE_RETRIES: usize = 3;
pub(crate) const DUPLICATE_RETRY_DELAY: Duration = Duration::from_secs(1);
pub(crate) const FRESH_TAIL_THRESHOLD: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExtensionObserverKey {
    pub(crate) context: PlaybackContext,
    pub(crate) current_index: Option<usize>,
    pub(crate) queue_len: usize,
    pub(crate) queue_fingerprint: u64,
}

pub(crate) fn observer_key(state: &PlaybackState) -> ExtensionObserverKey {
    let mut hasher = DefaultHasher::new();
    for track in &state.queue {
        track.provider.hash(&mut hasher);
        track.id.hash(&mut hasher);
    }
    ExtensionObserverKey {
        context: state.context.clone(),
        current_index: state.current_index,
        queue_len: state.queue.len(),
        queue_fingerprint: hasher.finish(),
    }
}

/// Count only the fresh physical tail.  Tracks before the current index are
/// already consumed, even when Repeat All would make them appear upcoming.
pub(crate) fn fresh_playable_tail(state: &PlaybackState) -> usize {
    let Some(current) = state
        .current_index
        .filter(|index| *index < state.queue.len())
    else {
        return 0;
    };
    state.queue[current + 1..]
        .iter()
        .filter(|track| !state.explicit_blocked(track))
        .count()
}

pub(crate) fn should_extend_at_tail(state: &PlaybackState) -> bool {
    state.context.can_extend() && fresh_playable_tail(state) <= FRESH_TAIL_THRESHOLD
}

pub(crate) fn should_retry_duplicate_batch(
    batch_nonempty: bool,
    added: usize,
    has_next_flow_tuner: bool,
    retries: usize,
) -> bool {
    batch_nonempty && added == 0 && has_next_flow_tuner && retries < MAX_DUPLICATE_RETRIES
}

pub(crate) fn duplicate_retry_delay(_retry: usize) -> Duration {
    DUPLICATE_RETRY_DELAY
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        library::{FlowMode, FlowTuner},
        playback::{PlaybackContext, PlaybackProvider, PlaybackTrack, RepeatMode},
    };

    fn track(id: &str, explicit: bool) -> PlaybackTrack {
        PlaybackTrack {
            provider: PlaybackProvider::Deezer,
            id: id.into(),
            title: id.into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            downloadable: false,
            progressive: false,
            explicit,
            ai_generated: false,
            service_url: String::new(),
        }
    }

    #[test]
    fn repeat_all_does_not_hide_a_short_fresh_tail() {
        let mut state = PlaybackState::default();
        state.replace(
            vec![track("a", false), track("b", false), track("c", false)],
            2,
        );
        state.set_repeat_mode(RepeatMode::All);
        state.context = PlaybackContext::DeezerTrackMix {
            seed_track_id: "a".into(),
        };
        assert_eq!(fresh_playable_tail(&state), 0);
        assert!(should_extend_at_tail(&state));
    }

    #[test]
    fn explicit_tracks_do_not_count_as_fresh_playable_tail() {
        let mut state = PlaybackState::default();
        state.replace(
            vec![
                track("a", false),
                track("blocked", true),
                track("fresh", false),
            ],
            0,
        );
        state.set_skip_explicit(true);
        assert_eq!(fresh_playable_tail(&state), 1);
    }

    #[test]
    fn observer_key_changes_for_context_and_queue_shape_but_not_position() {
        let mut state = PlaybackState::default();
        state.replace(vec![track("a", false), track("b", false)], 0);
        state.set_repeat_mode(RepeatMode::All);
        let first = observer_key(&state);
        state.position = Duration::from_secs(2);
        assert_eq!(first, observer_key(&state));
        state.context = PlaybackContext::DeezerFlow {
            config_id: "config".into(),
            mode: FlowMode::Default,
            tuner: Some(FlowTuner::initial(FlowMode::Default)),
            kind: crate::playback::DeezerFlowKind::Flow,
        };
        assert_ne!(first, observer_key(&state));
        state.queue.push(track("c", false));
        assert_ne!(first, observer_key(&state));
    }

    #[test]
    fn duplicate_retry_is_bounded_and_delayed() {
        assert!(should_retry_duplicate_batch(true, 0, true, 0));
        assert!(should_retry_duplicate_batch(true, 0, true, 2));
        assert!(!should_retry_duplicate_batch(true, 0, true, 3));
        assert!(!should_retry_duplicate_batch(false, 0, true, 0));
        assert!(!should_retry_duplicate_batch(true, 1, true, 0));
        assert_eq!(duplicate_retry_delay(0), Duration::from_secs(1));
    }
}
