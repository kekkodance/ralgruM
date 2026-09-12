use std::{
    sync::mpsc::{self, Sender},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use tokio::runtime::Runtime;

use crate::playback::{PlaybackProvider, PlaybackState, PlaybackStatus};

const DISCORD_CLIENT_ID: &str = "1528841371283095742";

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
    Update(PresenceTrack),
    Clear,
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
    published: Option<(PresenceTrack, Instant)>,
}

impl ProtocolState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            published: None,
        }
    }

    fn transition(&mut self, enabled: bool, track: Option<&PresenceTrack>) -> ProtocolAction {
        self.enabled = enabled;
        let Some(track) = track else {
            if self.published.take().is_some() {
                return ProtocolAction::Clear;
            }
            return ProtocolAction::None;
        };

        if !enabled {
            if self.published.take().is_some() {
                return ProtocolAction::Clear;
            }
            return ProtocolAction::None;
        }

        if let Some((last, updated_at)) = &self.published {
            let track_changed = last.id != track.id
                || last.provider != track.provider
                || last.title != track.title
                || last.artist != track.artist
                || last.album != track.album
                || last.artwork != track.artwork
                || last.duration != track.duration
                || last.paused != track.paused;

            if !track_changed {
                if track.paused {
                    return ProtocolAction::None;
                }
                let elapsed = updated_at.elapsed();
                let expected_pos = last.position + elapsed;
                let diff = if track.position > expected_pos {
                    track.position - expected_pos
                } else {
                    expected_pos - track.position
                };
                if diff < Duration::from_secs(5) {
                    return ProtocolAction::None;
                }
            }
        }

        self.published = Some((track.clone(), Instant::now()));
        ProtocolAction::Update
    }
}

pub(crate) struct DiscordPresence {
    sender: Sender<WorkerCommand>,
    protocol: ProtocolState,
}

impl DiscordPresence {
    pub(crate) fn new(enabled: bool, runtime: &Runtime) -> Self {
        let (sender, receiver) = mpsc::channel();
        runtime.spawn_blocking(move || {
            let mut client = None;
            while let Ok(command) = receiver.recv() {
                if let Err(error) = handle_command(&mut client, command) {
                    eprintln!("Discord presence unavailable: {error}");
                    client = None;
                }
            }
            if let Some(client) = client.as_mut() {
                let _ = client.clear_activity();
            }
        });
        Self {
            sender,
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
        let track = presence_track(state);
        let command = match self.protocol.transition(enabled, track.as_ref()) {
            ProtocolAction::Update => WorkerCommand::Update(track.expect("playing track exists")),
            ProtocolAction::Clear => WorkerCommand::Clear,
            ProtocolAction::None => return,
        };
        let _ = self.sender.send(command);
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

fn handle_command(
    client: &mut Option<DiscordIpcClient>,
    command: WorkerCommand,
) -> Result<(), String> {
    match command {
        WorkerCommand::Clear => {
            if let Some(client) = client.as_mut() {
                client
                    .clear_activity()
                    .map_err(|error| format!("clear failed: {error}"))?;
            }
        }
        WorkerCommand::Update(track) => {
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
            client
                .as_mut()
                .expect("Discord client was connected")
                .set_activity(activity_for(&track, &state))
                .map_err(|error| format!("update failed: {error}"))?;
        }
    }
    Ok(())
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
        let production = include_str!("discord.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("discord presence");
        assert!(production.contains("large_text(large_text)"));
        assert!(production.contains("track.album"));
        assert!(production.contains("!track.paused"));
        assert!(production.contains("by {}"));
        assert!(!production.contains("Hi-Fi"));
        assert!(!production.contains("| {}"));
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
