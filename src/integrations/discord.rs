use std::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use tokio::runtime::Runtime;

use crate::playback::{PlaybackProvider, PlaybackState, PlaybackStatus};

const DISCORD_CLIENT_ID: &str = "1528841371283095742";
const DISCORD_RETRY_BACKOFF: [Duration; 5] = [
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(625),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
const DISCORD_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Eq, PartialEq)]
struct PresenceTrack {
    id: String,
    title: String,
    artist: String,
    album: String,
    artwork: String,
    provider: PlaybackProvider,
    duration: Duration,
    position: Duration,
    paused: bool,
}

#[derive(Debug)]
enum WorkerCommand {
    Update {
        sequence: u64,
        track: PresenceTrack,
        queued_at: Instant,
    },
    Clear {
        sequence: u64,
    },
}

impl WorkerCommand {
    fn sequence(&self) -> u64 {
        match self {
            Self::Update { sequence, .. } | Self::Clear { sequence } => *sequence,
        }
    }
}

#[derive(Debug)]
enum WorkerEvent {
    Acknowledged(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtocolAction {
    Update,
    Clear,
    None,
}

#[derive(Debug)]
struct ProtocolState {
    enabled: bool,
    desired: Option<PresenceTrack>,
    acknowledged: Option<(PresenceTrack, Instant)>,
    pending: Option<PendingPresence>,
    next_sequence: u64,
}

#[derive(Debug)]
struct PendingPresence {
    sequence: u64,
    target: Option<PresenceTrack>,
}

impl ProtocolState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            desired: None,
            acknowledged: None,
            pending: None,
            next_sequence: 0,
        }
    }

    fn transition(&mut self, enabled: bool, track: Option<&PresenceTrack>) -> ProtocolAction {
        self.enabled = enabled;
        self.desired = enabled.then(|| track.cloned()).flatten();

        // A pending command has not been acknowledged by Discord yet. Keep
        // the latest requested state queued, replacing an older state when a
        // seek, pause, or track change arrives before the worker finishes.
        if let Some(pending) = &self.pending {
            if pending_targets_match(pending.target.as_ref(), self.desired.as_ref()) {
                return ProtocolAction::None;
            }
            return self.queue_desired();
        }

        match (&self.desired, &self.acknowledged) {
            (None, None) => ProtocolAction::None,
            (None, Some(_)) => self.queue_desired(),
            (Some(_), None) => self.queue_desired(),
            (Some(track), Some((last, updated_at))) => {
                if tracks_need_update(last, *updated_at, track) {
                    self.queue_desired()
                } else {
                    ProtocolAction::None
                }
            }
        }
    }

    fn queue_desired(&mut self) -> ProtocolAction {
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending = Some(PendingPresence {
            sequence: self.next_sequence,
            target: self.desired.clone(),
        });
        if self.desired.is_some() {
            ProtocolAction::Update
        } else {
            ProtocolAction::Clear
        }
    }

    fn pending_command(&self, action: ProtocolAction) -> Option<WorkerCommand> {
        let pending = self.pending.as_ref()?;
        Some(match action {
            ProtocolAction::Update => WorkerCommand::Update {
                sequence: pending.sequence,
                track: pending.target.clone()?,
                queued_at: Instant::now(),
            },
            ProtocolAction::Clear => WorkerCommand::Clear {
                sequence: pending.sequence,
            },
            ProtocolAction::None => return None,
        })
    }

    fn acknowledge(&mut self, sequence: u64) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if pending.sequence != sequence {
            self.pending = Some(pending);
            return;
        }
        self.acknowledged = pending.target.map(|track| (track, Instant::now()));
    }

    fn fail(&mut self, sequence: u64) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.sequence == sequence)
        {
            self.pending = None;
        }
    }

    #[cfg(test)]
    fn pending_sequence(&self) -> Option<u64> {
        self.pending.as_ref().map(|pending| pending.sequence)
    }
}

fn tracks_need_update(last: &PresenceTrack, updated_at: Instant, current: &PresenceTrack) -> bool {
    if metadata_changed(last, current) {
        return true;
    }
    if current.paused {
        return false;
    }
    let elapsed = updated_at.elapsed();
    let expected_pos = last.position.saturating_add(elapsed);
    let diff = current.position.abs_diff(expected_pos);
    diff >= Duration::from_secs(5)
}

fn metadata_changed(last: &PresenceTrack, current: &PresenceTrack) -> bool {
    last.id != current.id
        || last.provider != current.provider
        || last.title != current.title
        || last.artist != current.artist
        || last.album != current.album
        || last.artwork != current.artwork
        || last.duration != current.duration
        || last.paused != current.paused
}

fn pending_targets_match(pending: Option<&PresenceTrack>, desired: Option<&PresenceTrack>) -> bool {
    match (pending, desired) {
        (None, None) => true,
        (Some(pending), Some(desired)) => {
            !metadata_changed(pending, desired)
                && (desired.paused
                    || desired.position.abs_diff(pending.position) < Duration::from_secs(5))
        }
        _ => false,
    }
}

pub(crate) struct DiscordPresence {
    sender: Sender<WorkerCommand>,
    events: Receiver<WorkerEvent>,
    protocol: ProtocolState,
}

impl DiscordPresence {
    pub(crate) fn new(enabled: bool, runtime: &Runtime) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        runtime.spawn_blocking(move || {
            run_worker(receiver, event_sender);
        });
        Self {
            sender,
            events,
            protocol: ProtocolState::new(enabled),
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool, state: &PlaybackState) {
        self.sync(enabled, state);
    }

    pub(crate) fn playback_changed(&mut self, state: &PlaybackState) {
        self.sync(self.protocol.enabled, state);
    }

    fn sync(&mut self, enabled: bool, state: &PlaybackState) {
        while let Ok(WorkerEvent::Acknowledged(sequence)) = self.events.try_recv() {
            self.protocol.acknowledge(sequence);
        }
        let track = presence_track(state);
        let action = self.protocol.transition(enabled, track.as_ref());
        let Some(command) = self.protocol.pending_command(action) else {
            return;
        };
        let sequence = command.sequence();
        if self.sender.send(command).is_err() {
            self.protocol.fail(sequence);
        }
    }
}

fn presence_track(state: &PlaybackState) -> Option<PresenceTrack> {
    let track = state
        .current()
        .filter(|track| !track.title.trim().is_empty())?;
    matches!(
        state.status,
        PlaybackStatus::Playing | PlaybackStatus::Loading | PlaybackStatus::Paused
    )
    .then(|| PresenceTrack {
        id: track.id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        artwork: track.artwork.clone(),
        provider: track.provider,
        duration: state.duration,
        position: state.position,
        paused: state.status == PlaybackStatus::Paused,
    })
}

fn run_worker(receiver: Receiver<WorkerCommand>, events: Sender<WorkerEvent>) {
    let mut client = None;
    let Ok(mut command) = receiver.recv() else {
        return;
    };
    let mut retries = 0;
    let mut acknowledged_sequence = None;
    'worker: loop {
        // Drain commands that arrived while the previous request was in
        // flight. Only the newest desired state needs to reach Discord.
        while let Ok(next) = receiver.try_recv() {
            command = next;
            retries = 0;
        }

        match handle_command(&mut client, &command) {
            Ok(()) => {
                if acknowledged_sequence != Some(command.sequence()) {
                    if events
                        .send(WorkerEvent::Acknowledged(command.sequence()))
                        .is_err()
                    {
                        break 'worker;
                    }
                    acknowledged_sequence = Some(command.sequence());
                }
                retries = 0;
                // Keep publishing the latest state occasionally so a
                // Discord restart during an unchanged track is recovered
                // without relying on another playback transition.
                match receiver.recv_timeout(DISCORD_HEARTBEAT_INTERVAL) {
                    Ok(next) => command = next,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break 'worker,
                }
            }
            Err(error) => {
                if retries == 0 {
                    eprintln!("Discord presence unavailable: {error}");
                }
                client = None;
                let delay = DISCORD_RETRY_BACKOFF[retries.min(DISCORD_RETRY_BACKOFF.len() - 1)];
                retries += 1;
                match receiver.recv_timeout(delay) {
                    Ok(next) => {
                        command = next;
                        retries = 0;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break 'worker,
                }
            }
        }
    }
    if let Some(client) = client.as_mut() {
        let _ = client.clear_activity();
    }
}

fn handle_command(
    client: &mut Option<DiscordIpcClient>,
    command: &WorkerCommand,
) -> Result<(), String> {
    match command {
        WorkerCommand::Clear { .. } => {
            if let Some(client) = client.as_mut() {
                client
                    .clear_activity()
                    .map_err(|error| format!("clear failed: {error}"))?;
            }
        }
        WorkerCommand::Update {
            track, queued_at, ..
        } => {
            if client.is_none() {
                let mut connection = DiscordIpcClient::new(DISCORD_CLIENT_ID);
                connection
                    .connect()
                    .map_err(|error| format!("Discord is not running: {error}"))?;
                *client = Some(connection);
            }
            let state = if track.artist.trim().is_empty() {
                "Listening to music".to_owned()
            } else {
                format!("by {}", track.artist)
            };
            let track = track_for_publish(track, *queued_at);
            client
                .as_mut()
                .expect("Discord client was connected")
                .set_activity(activity_for(&track, &state))
                .map_err(|error| format!("update failed: {error}"))?;
        }
    }
    Ok(())
}

fn track_for_publish(track: &PresenceTrack, queued_at: Instant) -> PresenceTrack {
    let mut track = track.clone();
    if !track.paused {
        track.position = track
            .position
            .saturating_add(queued_at.elapsed())
            .min(track.duration);
    }
    track
}

fn activity_for<'a>(track: &'a PresenceTrack, state: &'a str) -> activity::Activity<'a> {
    let (small_image, small_text) = match track.provider {
        PlaybackProvider::SoundCloud => ("soundcloud", "SoundCloud HQ"),
        PlaybackProvider::Deezer => ("deezer", "Deezer FLAC"),
    };
    let large_image = if track.artwork.trim().is_empty() {
        "ralgrum_logo"
    } else {
        track.artwork.as_str()
    };
    let large_text = if track.album.trim().is_empty() {
        "ralgruM"
    } else {
        track.album.as_str()
    };
    let assets = activity::Assets::new()
        .large_image(large_image)
        .large_text(large_text)
        .small_image(small_image)
        .small_text(small_text);
    let mut presence = activity::Activity::new()
        .activity_type(activity::ActivityType::Listening)
        .details(&track.title)
        .state(state)
        .assets(assets);
    // Discord's listening progress bar runs off start/end. Omit both while
    // paused so the activity stays put and the bar stops instead of clearing.
    if !track.paused && !track.duration.is_zero() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let position_ms = track.position.min(track.duration).as_millis() as i64;
        let duration_ms = track.duration.as_millis() as i64;
        presence = presence.timestamps(
            activity::Timestamps::new()
                .start(now_ms - position_ms)
                .end(now_ms + duration_ms - position_ms),
        );
    }
    presence
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::PlaybackTrack;

    fn playing_state() -> PlaybackState {
        let mut state = PlaybackState::default();
        let generation = state
            .replace(
                vec![PlaybackTrack {
                    downloadable: false,
                    progressive: false,
                    provider: PlaybackProvider::Deezer,
                    id: "1".into(),
                    title: "Track".into(),
                    artist: "Artist".into(),
                    album: String::new(),
                    album_id: String::new(),
                    release_date: String::new(),
                    artists: Vec::new(),
                    artwork: String::new(),
                    duration: Duration::from_secs(60),
                    explicit: false,
                    ai_generated: false,
                    service_url: String::new(),
                }],
                0,
            )
            .unwrap();
        state.loaded(generation, None);
        state
    }

    fn test_track(id: &str, pos_secs: u64) -> PresenceTrack {
        PresenceTrack {
            id: id.into(),
            title: format!("Track {id}"),
            artist: "Artist".into(),
            album: "Album".into(),
            artwork: "Artwork".into(),
            provider: PlaybackProvider::Deezer,
            duration: Duration::from_secs(120),
            position: Duration::from_secs(pos_secs),
            paused: false,
        }
    }

    #[test]
    fn protocol_updates_and_clears_on_playback_transitions() {
        let mut protocol = ProtocolState::new(true);
        let track = test_track("1", 0);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
        assert_eq!(protocol.transition(true, None), ProtocolAction::Clear);
        assert_eq!(protocol.transition(true, None), ProtocolAction::None);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
    }

    #[test]
    fn protocol_toggle_clears_and_restores_active_playback() {
        let mut protocol = ProtocolState::new(true);
        let track = test_track("1", 0);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
        assert_eq!(
            protocol.transition(false, Some(&track)),
            ProtocolAction::Clear
        );
        assert_eq!(
            protocol.transition(false, Some(&track)),
            ProtocolAction::None
        );
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
    }

    #[test]
    fn protocol_debounces_small_position_updates() {
        let mut protocol = ProtocolState::new(true);
        let track1 = test_track("1", 0);
        assert_eq!(
            protocol.transition(true, Some(&track1)),
            ProtocolAction::Update
        );

        let track2 = test_track("1", 1);
        assert_eq!(
            protocol.transition(true, Some(&track2)),
            ProtocolAction::None
        );

        let track_seeked = test_track("1", 20);
        assert_eq!(
            protocol.transition(true, Some(&track_seeked)),
            ProtocolAction::Update
        );

        let track_next = test_track("2", 20);
        assert_eq!(
            protocol.transition(true, Some(&track_next)),
            ProtocolAction::Update
        );
    }

    #[test]
    fn failed_update_is_requeued_after_the_worker_reports_failure() {
        let mut protocol = ProtocolState::new(true);
        let track = test_track("1", 0);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
        let sequence = protocol.pending_sequence().unwrap();
        protocol.fail(sequence);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
        assert_ne!(protocol.pending_sequence(), Some(sequence));
    }

    #[test]
    fn acknowledged_state_debounces_position_without_marking_queued_state_published() {
        let mut protocol = ProtocolState::new(true);
        let track = test_track("1", 0);
        assert_eq!(
            protocol.transition(true, Some(&track)),
            ProtocolAction::Update
        );
        protocol.acknowledge(protocol.pending_sequence().unwrap());
        let nearby = test_track("1", 1);
        assert_eq!(
            protocol.transition(true, Some(&nearby)),
            ProtocolAction::None
        );
    }

    #[test]
    fn queued_presence_position_catches_up_when_retrying() {
        let track = test_track("1", 10);
        let published = track_for_publish(&track, Instant::now() - Duration::from_secs(8));
        assert!(published.position >= Duration::from_secs(18));
        assert!(published.position <= track.duration);

        let mut paused = track.clone();
        paused.paused = true;
        assert_eq!(
            track_for_publish(&paused, Instant::now() - Duration::from_secs(8)).position,
            paused.position
        );
    }

    #[test]
    fn snapshot_keeps_playing_loading_and_paused_with_a_nonempty_title() {
        let mut state = playing_state();
        assert!(!presence_track(&state).expect("playing publishes").paused);
        state.status = PlaybackStatus::Loading;
        assert!(!presence_track(&state).expect("loading publishes").paused);
        state.status = PlaybackStatus::Paused;
        assert!(
            presence_track(&state)
                .expect("paused still publishes")
                .paused
        );
        state.status = PlaybackStatus::Playing;
        state.queue[0].title = "  ".into();
        assert!(presence_track(&state).is_none());
    }

    #[test]
    fn pause_updates_in_place_instead_of_clearing() {
        let mut protocol = ProtocolState::new(true);
        let playing = test_track("1", 10);
        assert_eq!(
            protocol.transition(true, Some(&playing)),
            ProtocolAction::Update
        );

        let mut paused = playing.clone();
        paused.paused = true;
        assert_eq!(
            protocol.transition(true, Some(&paused)),
            ProtocolAction::Update
        );
        assert_eq!(
            protocol.transition(true, Some(&paused)),
            ProtocolAction::None
        );

        let mut resumed = paused.clone();
        resumed.paused = false;
        assert_eq!(
            protocol.transition(true, Some(&resumed)),
            ProtocolAction::Update
        );
    }

    #[test]
    fn artwork_hover_text_is_the_album_not_a_player_subtitle() {
        let track = test_track("1", 0);
        let activity = serde_json::to_value(activity_for(&track, "by Artist")).unwrap();
        assert_eq!(activity["details"], "Track 1");
        assert_eq!(activity["state"], "by Artist");
        assert_eq!(activity["assets"]["large_text"], "Album");
        assert_eq!(activity["assets"]["small_text"], "Deezer FLAC");
        assert!(!activity.to_string().contains("Hi-Fi"));
        assert!(!activity.to_string().contains("| Artist"));

        let mut paused = track;
        paused.paused = true;
        let paused_activity = serde_json::to_value(activity_for(&paused, "by Artist")).unwrap();
        assert!(paused_activity.get("timestamps").is_none());
    }

    #[test]
    fn loading_the_next_track_updates_instead_of_clearing() {
        let mut protocol = ProtocolState::new(true);
        let first = test_track("1", 10);
        assert_eq!(
            protocol.transition(true, Some(&first)),
            ProtocolAction::Update
        );

        let mut loading = playing_state();
        loading.status = PlaybackStatus::Loading;
        loading.queue[0].id = "2".into();
        loading.queue[0].title = "Track 2".into();
        let next = presence_track(&loading).expect("loading still has a current track");
        assert_eq!(next.id, "2");
        assert_eq!(
            protocol.transition(true, Some(&next)),
            ProtocolAction::Update
        );
        assert_ne!(
            protocol.transition(true, Some(&next)),
            ProtocolAction::Clear
        );
    }
}
