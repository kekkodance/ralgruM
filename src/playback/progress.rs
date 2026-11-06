use std::time::Duration;

use super::state::PlaybackState;

/// High-frequency playback data, split out of [`PlaybackState`] so the
/// 250 ms transport poll only invalidates the views that show progress.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PlaybackProgress {
    pub(crate) generation: u64,
    pub(crate) position: Duration,
    pub(crate) buffered: Duration,
    pub(crate) suffix_buffered: Option<(Duration, Duration)>,
}

impl PlaybackProgress {
    pub(crate) fn from_state(state: &PlaybackState) -> Self {
        Self {
            generation: state.generation,
            position: state.position,
            buffered: state.buffered,
            suffix_buffered: state.suffix_buffered,
        }
    }

    /// A snapshot belonging to `generation`, or an empty one while the
    /// transport has moved on but the poll has not published yet.
    pub(crate) fn for_generation(self, generation: u64) -> Self {
        if self.generation == generation {
            self
        } else {
            Self {
                generation,
                ..Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_state_copies_transport_progress() {
        let mut state = PlaybackState::default();
        state.generation = 7;
        state.position = Duration::from_secs(12);
        state.buffered = Duration::from_secs(20);
        state.suffix_buffered = Some((Duration::from_secs(30), Duration::from_secs(40)));

        let progress = PlaybackProgress::from_state(&state);

        assert_eq!(progress.generation, 7);
        assert_eq!(progress.position, Duration::from_secs(12));
        assert_eq!(progress.buffered, Duration::from_secs(20));
        assert_eq!(
            progress.suffix_buffered,
            Some((Duration::from_secs(30), Duration::from_secs(40)))
        );
    }

    #[test]
    fn for_generation_keeps_matching_snapshot() {
        let progress = PlaybackProgress {
            generation: 4,
            position: Duration::from_secs(5),
            buffered: Duration::from_secs(8),
            suffix_buffered: Some((Duration::from_secs(9), Duration::from_secs(10))),
        };

        assert_eq!(progress.for_generation(4), progress);
    }

    #[test]
    fn for_generation_drops_stale_progress() {
        let progress = PlaybackProgress {
            generation: 4,
            position: Duration::from_secs(5),
            buffered: Duration::from_secs(8),
            suffix_buffered: Some((Duration::from_secs(9), Duration::from_secs(10))),
        };

        assert_eq!(
            progress.for_generation(5),
            PlaybackProgress {
                generation: 5,
                ..PlaybackProgress::default()
            }
        );
    }
}
