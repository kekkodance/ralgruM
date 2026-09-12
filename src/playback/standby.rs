use std::{path::PathBuf, sync::Arc, time::Duration};

use rodio::{Sink, Source};

use super::{PlaybackStatus, PlaybackTrack, ResolvedTrackInfo};
use super::{
    progressive::{ProgressiveCompletion, TimelineSeekSession},
    resolver::AudioFormat,
};

/// How much playback time must remain before the next queue track is
/// resolved, downloaded, and decoded into the standby buffer.
pub(crate) const PREPARE_WINDOW: Duration = Duration::from_secs(30);
/// Once the current track is this close to its end the boundary watcher
/// polls the sink finely so the handoff lines up with the audio.
const FINE_POLL_REMAINING: Duration = Duration::from_secs(2);
const COARSE_POLL: Duration = Duration::from_millis(250);
const FINE_POLL: Duration = Duration::from_millis(10);

/// A fully decoded track that can be appended to the live sink with no
/// further work. The backing temp file must outlive playback, so it travels
/// with the source until the engine takes ownership.
pub(crate) struct PreparedSource {
    source: Box<dyn Source + Send>,
    file: tempfile::NamedTempFile,
    duration: Option<Duration>,
    progressive_seek: Option<ProgressiveSeek>,
}

#[derive(Clone)]
pub(crate) struct ProgressiveSeek {
    pub(crate) path: PathBuf,
    pub(crate) format: AudioFormat,
    pub(crate) completion: ProgressiveCompletion,
    pub(crate) timeline_seek_session: Option<Arc<dyn TimelineSeekSession>>,
}

impl PreparedSource {
    pub(crate) fn new<S>(
        source: S,
        duration: Option<Duration>,
        file: tempfile::NamedTempFile,
    ) -> Self
    where
        S: Source + Send + 'static,
    {
        Self {
            source: Box::new(source),
            file,
            duration,
            progressive_seek: None,
        }
    }

    pub(crate) fn with_progressive_seek(mut self, progressive_seek: ProgressiveSeek) -> Self {
        self.progressive_seek = Some(progressive_seek);
        self
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Box<dyn Source + Send>,
        tempfile::NamedTempFile,
        Option<ProgressiveSeek>,
    ) {
        (self.source, self.file, self.progressive_seek)
    }
}

/// Cloneable handle to the live sink. The boundary watcher uses it without
/// borrowing the engine, and stale watchers are detected by pointer identity
/// against the engine's current sink.
#[derive(Clone)]
pub(crate) struct SinkProbe {
    sink: Arc<Sink>,
}

impl SinkProbe {
    pub(crate) fn new(sink: Arc<Sink>) -> Self {
        Self { sink }
    }
    pub(crate) fn queued(&self) -> usize {
        self.sink.len()
    }
    pub(crate) fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }
    pub(crate) fn position(&self) -> Duration {
        self.sink.get_pos()
    }
    pub(crate) fn is_sink(&self, sink: &Arc<Sink>) -> bool {
        Arc::ptr_eq(&self.sink, sink)
    }
}

/// A standby armed into the live sink, waiting for the current track to end.
pub(crate) struct ArmedStandby {
    pub(crate) track: PlaybackTrack,
    pub(crate) generation: u64,
    pub(crate) queue_epoch: u64,
    pub(crate) duration: Option<Duration>,
    pub(crate) quality: Option<String>,
    pub(crate) audio_info: Option<ResolvedTrackInfo>,
    pub(crate) probe: SinkProbe,
}

/// Standby lifecycle owned by the playback model. Idle means nothing is being
/// prepared, Pending means a resolve and decode task is in flight, and Armed
/// means the decoded source is already queued in the sink.
#[derive(Default)]
pub(crate) enum StandbyPhase {
    #[default]
    Idle,
    Pending,
    Armed(ArmedStandby),
}

/// A standby is only prepared while playing without repeat one, when a next
/// track exists and the current track runs out inside the prepare window.
/// Tracks shorter than the window qualify immediately.
pub(crate) fn should_prepare_status(
    status: PlaybackStatus,
    duration: Duration,
    position: Duration,
) -> bool {
    status == PlaybackStatus::Playing
        && !duration.is_zero()
        && duration.saturating_sub(position) <= PREPARE_WINDOW
}

/// What the boundary watcher should do after observing the sink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WatchTick {
    /// Keep waiting; playback continues normally.
    Continue,
    /// The current source finished and the standby is now playing.
    Boundary,
    /// The sink was stopped or replaced; the watcher is obsolete.
    Stop,
}

/// One queued source means the current track ended and the standby took over.
/// Zero queued sources means the watcher can no longer observe a boundary.
/// One queued source means the boundary has occurred, including when a pause
/// lands immediately after the standby source takes over.
pub(crate) fn watch_tick(queued: usize, _paused: bool) -> WatchTick {
    if queued == 0 {
        WatchTick::Stop
    } else if queued == 1 {
        WatchTick::Boundary
    } else {
        WatchTick::Continue
    }
}

/// Coarse polling while there is plenty of track left, fine polling once the
/// boundary is seconds away. Paused sinks never cross a boundary, so they
/// always poll coarsely.
pub(crate) fn poll_interval(remaining: Duration, paused: bool) -> Duration {
    if paused || remaining > FINE_POLL_REMAINING {
        COARSE_POLL
    } else {
        FINE_POLL
    }
}

/// What the model should do when the current source ends with an armed
/// standby: hand state over to it, load a different track the normal way, or
/// ignore a boundary from an obsolete sink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundaryOutcome {
    Commit,
    Replace,
    Ignore,
}

pub(crate) struct BoundaryCheck {
    pub(crate) sink_current: bool,
    pub(crate) status: PlaybackStatus,
    pub(crate) generation: u64,
    pub(crate) armed_generation: u64,
    pub(crate) queue_epoch: u64,
    pub(crate) armed_queue_epoch: u64,
    pub(crate) armed_provider: super::PlaybackProvider,
    pub(crate) armed_target: String,
    pub(crate) upcoming_first: Option<(super::PlaybackProvider, String)>,
}

pub(crate) fn boundary_outcome(check: &BoundaryCheck) -> BoundaryOutcome {
    if !check.sink_current
        || check.armed_generation != check.generation
        || !matches!(
            check.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        )
    {
        return BoundaryOutcome::Ignore;
    }
    if check.queue_epoch != check.armed_queue_epoch {
        return BoundaryOutcome::Replace;
    }
    if check
        .upcoming_first
        .as_ref()
        .is_some_and(|(provider, target)| {
            *provider == check.armed_provider && target == &check.armed_target
        })
    {
        BoundaryOutcome::Commit
    } else {
        BoundaryOutcome::Replace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::{PlaybackProvider, PlaybackStatus};
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn preparation_requires_playing_near_the_end_with_a_known_duration() {
        assert!(should_prepare_status(
            PlaybackStatus::Playing,
            Duration::from_secs(100),
            Duration::from_secs(95),
        ));
        assert!(should_prepare_status(
            PlaybackStatus::Playing,
            Duration::from_secs(5),
            Duration::ZERO,
        ));
        assert!(!should_prepare_status(
            PlaybackStatus::Playing,
            Duration::from_secs(100),
            Duration::from_secs(10),
        ));
        assert!(!should_prepare_status(
            PlaybackStatus::Paused,
            Duration::from_secs(100),
            Duration::from_secs(95),
        ));
        assert!(!should_prepare_status(
            PlaybackStatus::Playing,
            Duration::ZERO,
            Duration::ZERO,
        ));
    }

    #[test]
    fn watch_tick_reports_boundary_only_for_the_live_handoff() {
        assert_eq!(watch_tick(2, false), WatchTick::Continue);
        assert_eq!(watch_tick(2, true), WatchTick::Continue);
        assert_eq!(watch_tick(3, false), WatchTick::Continue);
        assert_eq!(watch_tick(1, false), WatchTick::Boundary);
        assert_eq!(watch_tick(1, true), WatchTick::Boundary);
        assert_eq!(watch_tick(0, false), WatchTick::Stop);
        assert_eq!(watch_tick(0, true), WatchTick::Stop);
    }

    #[test]
    fn pausing_on_either_side_of_an_armed_boundary_keeps_metadata_coherent() {
        assert_eq!(watch_tick(2, true), WatchTick::Continue);
        assert_eq!(watch_tick(1, true), WatchTick::Boundary);
    }

    #[test]
    fn poll_interval_is_fine_only_close_to_the_boundary_and_while_playing() {
        assert_eq!(
            poll_interval(Duration::from_secs(30), false),
            Duration::from_millis(250)
        );
        assert_eq!(
            poll_interval(Duration::from_secs(2), false),
            Duration::from_millis(10)
        );
        assert_eq!(
            poll_interval(Duration::ZERO, false),
            Duration::from_millis(10)
        );
        assert_eq!(
            poll_interval(Duration::from_millis(500), true),
            Duration::from_millis(250)
        );
    }

    fn boundary_check(
        sink_current: bool,
        status: PlaybackStatus,
        generation: u64,
        armed_target: &str,
        upcoming_first: Option<&str>,
    ) -> BoundaryCheck {
        BoundaryCheck {
            sink_current,
            status,
            generation,
            armed_generation: generation,
            queue_epoch: 4,
            armed_queue_epoch: 4,
            armed_provider: PlaybackProvider::Deezer,
            armed_target: armed_target.into(),
            upcoming_first: upcoming_first
                .map(|target| (PlaybackProvider::Deezer, target.to_owned())),
        }
    }

    #[test]
    fn boundary_outcome_ignores_stale_sinks_generations_and_idle_statuses() {
        let live = |sink_current, status, generation| BoundaryCheck {
            sink_current,
            status,
            generation,
            armed_generation: 7,
            queue_epoch: 4,
            armed_queue_epoch: 4,
            armed_provider: PlaybackProvider::Deezer,
            armed_target: "next".into(),
            upcoming_first: Some((PlaybackProvider::Deezer, "next".into())),
        };
        assert_eq!(
            boundary_outcome(&live(false, PlaybackStatus::Playing, 7)),
            BoundaryOutcome::Ignore
        );
        assert_eq!(
            boundary_outcome(&live(true, PlaybackStatus::Loading, 7)),
            BoundaryOutcome::Ignore
        );
        assert_eq!(
            boundary_outcome(&live(true, PlaybackStatus::Ended, 7)),
            BoundaryOutcome::Ignore
        );
        assert_eq!(
            boundary_outcome(&live(true, PlaybackStatus::Playing, 8)),
            BoundaryOutcome::Ignore
        );
        assert_eq!(
            boundary_outcome(&live(true, PlaybackStatus::Playing, 7)),
            BoundaryOutcome::Commit
        );
    }

    #[test]
    fn boundary_outcome_replaces_when_the_queue_changed_under_the_standby() {
        let reordered = boundary_check(true, PlaybackStatus::Playing, 7, "next", Some("different"));
        assert_eq!(boundary_outcome(&reordered), BoundaryOutcome::Replace);

        let queue_ended = boundary_check(true, PlaybackStatus::Playing, 7, "next", None);
        assert_eq!(boundary_outcome(&queue_ended), BoundaryOutcome::Replace);

        let matching = boundary_check(true, PlaybackStatus::Paused, 7, "next", Some("next"));
        assert_eq!(boundary_outcome(&matching), BoundaryOutcome::Commit);

        let mut changed_epoch =
            boundary_check(true, PlaybackStatus::Playing, 7, "next", Some("next"));
        changed_epoch.queue_epoch = 5;
        assert_eq!(boundary_outcome(&changed_epoch), BoundaryOutcome::Replace);

        let mut changed_provider =
            boundary_check(true, PlaybackStatus::Playing, 7, "next", Some("next"));
        changed_provider.upcoming_first = Some((PlaybackProvider::SoundCloud, "next".into()));
        assert_eq!(
            boundary_outcome(&changed_provider),
            BoundaryOutcome::Replace
        );
    }

    #[test]
    fn offline_sink_queue_plays_sources_back_to_back_and_resets_position() {
        // rodio keeps the same sink unbroken across queued sources, the source
        // count marks the boundary, and the position clock restarts with the
        // standby source. Those three properties are what the handoff relies on.
        let (sink, mut queue) = Sink::new();
        let sink = Arc::new(sink);
        sink.append(SamplesBuffer::new(1, 48_000, vec![0.5; 480]));
        sink.append(SamplesBuffer::new(1, 48_000, vec![-0.5; 480]));
        let probe = SinkProbe::new(sink);
        assert_eq!(probe.queued(), 2);

        for _ in 0..480 {
            assert_eq!(queue.next(), Some(0.5));
        }
        assert_eq!(probe.queued(), 2);
        assert_eq!(queue.next(), Some(-0.5));
        assert_eq!(probe.queued(), 1);

        // Advance past the next 5 ms control tick of the second source.
        for _ in 0..249 {
            assert_eq!(queue.next(), Some(-0.5));
        }
        let position = probe.position();
        assert!(
            position >= Duration::from_millis(4) && position < Duration::from_millis(10),
            "position {position:?} should track the standby source, not the whole queue"
        );

        for _ in 0..230 {
            assert_eq!(queue.next(), Some(-0.5));
        }
        assert_eq!(queue.next(), Some(0.0));
        assert_eq!(probe.queued(), 0);
    }

    #[test]
    fn prepared_standby_parts_retain_progressive_seek_metadata() {
        let buffer =
            super::super::progressive::ProgressiveFile::new(AudioFormat::Flac, None).unwrap();
        let reader = buffer.reader().unwrap();
        let completion = reader.completion();
        let path = buffer.path().to_owned();
        let file = buffer.into_file();
        let prepared = PreparedSource::new(
            SamplesBuffer::new(1, 48_000, vec![0.0; 48]),
            Some(Duration::from_millis(1)),
            file,
        )
        .with_progressive_seek(ProgressiveSeek {
            path,
            format: AudioFormat::Flac,
            completion,
            timeline_seek_session: None,
        });

        let (_, _, progressive_seek) = prepared.into_parts();

        assert!(progressive_seek.is_some());
    }
}
