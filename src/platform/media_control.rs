//! OS media session integration (Windows SMTC, macOS remote commands).
//!
//! Mirrors the mediaSession handlers of the original Tauri app
//! (public/js/core/app.js): metadata for the current track, playback state,
//! and remote control events (play, pause, previous, next, and seeks) routed
//! back into the playback model. Events arrive on OS threads, so they are
//! forwarded through an unbounded channel that the model drains on the main
//! thread. Linux MPRIS is not wired up (see Cargo.toml).

use std::time::Duration;

use crate::playback::{PlaybackProvider, PlaybackState, PlaybackStatus};

/// The seek offset the original app fell back to when the OS did not provide
/// one (`details.seekOffset || 10`).
pub(crate) const DEFAULT_SEEK_STEP: Duration = Duration::from_secs(10);

/// A remote control request translated from an OS media event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MediaRequest {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    SeekForward(Duration),
    SeekBackward(Duration),
    SeekPosition(Duration),
}

/// The track data the OS media interface shows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrackSnapshot {
    provider: PlaybackProvider,
    id: String,
    title: String,
    artist: String,
    album: String,
    artwork: String,
    duration: Duration,
}

fn track_snapshot(state: &PlaybackState) -> Option<TrackSnapshot> {
    let track = state.current()?;
    Some(TrackSnapshot {
        provider: track.provider,
        id: track.id.clone(),
        title: track.title.trim().to_owned(),
        artist: track.artist.trim().to_owned(),
        album: track.album.trim().to_owned(),
        artwork: track.artwork.trim().to_owned(),
        duration: state.duration,
    })
}

/// The playback status and progress pushed to the OS interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PlaybackCommand {
    playing: bool,
    position: Duration,
}

/// What a state sync needs to publish, deduplicated against what the OS
/// interface already shows.
#[derive(Debug, PartialEq)]
enum Plan {
    Attach {
        track: TrackSnapshot,
        playback: PlaybackCommand,
    },
    Playback(PlaybackCommand),
    Clear,
}

/// Diffs playback state against what was last published, so the OS controls
/// are only touched when something actually changed.
#[derive(Debug, Default)]
struct SessionState {
    published: Option<TrackSnapshot>,
    playback: Option<PlaybackCommand>,
}

impl SessionState {
    fn plan(&mut self, state: &PlaybackState) -> Option<Plan> {
        let Some(track) = track_snapshot(state) else {
            if self.published.take().is_some() {
                self.playback = None;
                return Some(Plan::Clear);
            }
            return None;
        };
        let playback = PlaybackCommand {
            playing: state.status == PlaybackStatus::Playing,
            position: Duration::from_secs(state.position.min(state.duration).as_secs()),
        };
        if self.published.as_ref() != Some(&track) {
            self.published = Some(track.clone());
            self.playback = Some(playback);
            return Some(Plan::Attach { track, playback });
        }
        if self.playback != Some(playback) {
            self.playback = Some(playback);
            return Some(Plan::Playback(playback));
        }
        None
    }
}

/// Extracts the Win32 window handle GPUI created for the main window. SMTC
/// needs an HWND to register with, and the value crosses no threads here.
#[cfg(windows)]
pub(crate) fn window_handle(window: &gpui::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // Called as a trait function: Window also has an inherent window_handle
    // method that returns the unrelated AnyWindowHandle id.
    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
            _ => None,
        },
        Err(error) => {
            crate::diagnostics::event("DEBUG", format!("main window handle unavailable: {error}"));
            None
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn window_handle(_window: &gpui::Window) -> Option<isize> {
    // macOS remote commands do not need a window handle.
    None
}

#[cfg(any(windows, target_os = "macos"))]
mod native {
    use futures::channel::mpsc::{UnboundedReceiver, unbounded};
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig, SeekDirection,
    };

    use super::{
        DEFAULT_SEEK_STEP, MediaRequest, Plan, PlaybackCommand, SessionState, TrackSnapshot,
        track_snapshot,
    };
    use crate::{diagnostics, playback::PlaybackState};

    const APP_NAME: &str = "ralgruM";

    pub(crate) struct MediaSession {
        controls: Option<MediaControls>,
        events: Option<UnboundedReceiver<MediaRequest>>,
        session: SessionState,
        #[cfg(windows)]
        taskbar: Option<crate::windows_taskbar::TaskbarControls>,
    }

    impl MediaSession {
        /// Attaches OS media controls for the given window handle. Failure is
        /// expected on machines where the controls cannot be registered, so it
        /// only logs and leaves the session inert.
        pub(crate) fn new(hwnd: Option<isize>) -> Self {
            let (sender, events) = unbounded::<MediaRequest>();
            #[cfg(windows)]
            let taskbar = hwnd.and_then(|hwnd| {
                crate::windows_taskbar::TaskbarControls::install(hwnd, sender.clone())
                    .map_err(|error| {
                        diagnostics::event(
                            "DEBUG",
                            format!("Windows taskbar integration unavailable: {error}"),
                        );
                    })
                    .ok()
            });
            let config = PlatformConfig {
                dbus_name: APP_NAME,
                display_name: APP_NAME,
                hwnd: hwnd.map(|handle| handle as *mut std::ffi::c_void),
            };
            let controls = MediaControls::new(config)
                .and_then(|mut controls| {
                    controls.attach(move |event| {
                        if let Some(request) = translate(event) {
                            let _ = sender.unbounded_send(request);
                        }
                    })?;
                    Ok(controls)
                })
                .map_err(|error| {
                    diagnostics::event("DEBUG", format!("OS media controls unavailable: {error}"));
                })
                .ok();
            #[cfg(windows)]
            let has_taskbar = taskbar.is_some();
            #[cfg(not(windows))]
            let has_taskbar = false;
            Self {
                events: (controls.is_some() || has_taskbar).then_some(events),
                controls,
                session: SessionState::default(),
                #[cfg(windows)]
                taskbar,
            }
        }

        /// The OS event stream, available once so the model can drain it on
        /// the main thread.
        pub(crate) fn take_events(&mut self) -> Option<UnboundedReceiver<MediaRequest>> {
            self.events.take()
        }

        pub(crate) fn playback_changed(&mut self, state: &PlaybackState) {
            #[cfg(windows)]
            if let Some(taskbar) = &self.taskbar {
                taskbar.update(crate::windows_taskbar::TaskbarPlaybackState {
                    has_track: track_snapshot(state).is_some(),
                    can_toggle: matches!(
                        state.status,
                        crate::playback::PlaybackStatus::Playing
                            | crate::playback::PlaybackStatus::Paused
                    ),
                    playing: state.status == crate::playback::PlaybackStatus::Playing,
                });
            }
            let Some(controls) = self.controls.as_mut() else {
                return;
            };
            match self.session.plan(state) {
                Some(Plan::Attach { track, playback }) => {
                    publish(controls.set_metadata(metadata_for(&track)), "metadata");
                    publish(controls.set_playback(playback_for(playback)), "playback");
                }
                Some(Plan::Playback(playback)) => {
                    publish(controls.set_playback(playback_for(playback)), "playback");
                }
                Some(Plan::Clear) => {
                    publish(controls.set_metadata(MediaMetadata::default()), "metadata");
                    publish(controls.set_playback(MediaPlayback::Stopped), "playback");
                }
                None => {}
            }
        }
    }

    fn publish(result: Result<(), souvlaki::Error>, what: &str) {
        if let Err(error) = result {
            diagnostics::event("DEBUG", format!("media {what} update failed: {error}"));
        }
    }

    fn metadata_for(track: &TrackSnapshot) -> MediaMetadata<'_> {
        MediaMetadata {
            title: non_empty(&track.title),
            artist: non_empty(&track.artist),
            album: non_empty(&track.album),
            cover_url: non_empty(&track.artwork),
            duration: (!track.duration.is_zero()).then_some(track.duration),
        }
    }

    fn non_empty(value: &str) -> Option<&str> {
        (!value.is_empty()).then_some(value)
    }

    fn playback_for(playback: PlaybackCommand) -> MediaPlayback {
        let progress = Some(MediaPosition(playback.position));
        if playback.playing {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        }
    }

    fn translate(event: MediaControlEvent) -> Option<MediaRequest> {
        match event {
            MediaControlEvent::Play => Some(MediaRequest::Play),
            MediaControlEvent::Pause => Some(MediaRequest::Pause),
            MediaControlEvent::Toggle => Some(MediaRequest::Toggle),
            MediaControlEvent::Next => Some(MediaRequest::Next),
            MediaControlEvent::Previous => Some(MediaRequest::Previous),
            MediaControlEvent::Seek(SeekDirection::Forward) => {
                Some(MediaRequest::SeekForward(DEFAULT_SEEK_STEP))
            }
            MediaControlEvent::Seek(SeekDirection::Backward) => {
                Some(MediaRequest::SeekBackward(DEFAULT_SEEK_STEP))
            }
            MediaControlEvent::SeekBy(SeekDirection::Forward, duration) => {
                Some(MediaRequest::SeekForward(duration))
            }
            MediaControlEvent::SeekBy(SeekDirection::Backward, duration) => {
                Some(MediaRequest::SeekBackward(duration))
            }
            MediaControlEvent::SetPosition(MediaPosition(position)) => {
                Some(MediaRequest::SeekPosition(position))
            }
            // The original app registered no handlers for stop, volume,
            // raise, quit, or URI events, so they are dropped here too.
            MediaControlEvent::Stop
            | MediaControlEvent::SetVolume(_)
            | MediaControlEvent::OpenUri(_)
            | MediaControlEvent::Raise
            | MediaControlEvent::Quit => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Duration;

        use crate::playback::PlaybackProvider;

        #[test]
        fn translates_transport_and_seek_events() {
            assert_eq!(translate(MediaControlEvent::Play), Some(MediaRequest::Play));
            assert_eq!(
                translate(MediaControlEvent::Pause),
                Some(MediaRequest::Pause)
            );
            assert_eq!(
                translate(MediaControlEvent::Toggle),
                Some(MediaRequest::Toggle)
            );
            assert_eq!(translate(MediaControlEvent::Next), Some(MediaRequest::Next));
            assert_eq!(
                translate(MediaControlEvent::Previous),
                Some(MediaRequest::Previous)
            );
            assert_eq!(
                translate(MediaControlEvent::Seek(SeekDirection::Backward)),
                Some(MediaRequest::SeekBackward(DEFAULT_SEEK_STEP))
            );
            assert_eq!(
                translate(MediaControlEvent::SeekBy(
                    SeekDirection::Forward,
                    Duration::from_secs(5)
                )),
                Some(MediaRequest::SeekForward(Duration::from_secs(5)))
            );
            assert_eq!(
                translate(MediaControlEvent::SetPosition(MediaPosition(
                    Duration::from_secs(42)
                ))),
                Some(MediaRequest::SeekPosition(Duration::from_secs(42)))
            );
        }

        #[test]
        fn drops_events_the_original_app_ignored() {
            assert_eq!(translate(MediaControlEvent::Stop), None);
            assert_eq!(translate(MediaControlEvent::SetVolume(0.5)), None);
            assert_eq!(translate(MediaControlEvent::OpenUri("x".into())), None);
            assert_eq!(translate(MediaControlEvent::Raise), None);
            assert_eq!(translate(MediaControlEvent::Quit), None);
        }

        #[test]
        fn metadata_hides_empty_fields_and_zero_duration() {
            let empty = TrackSnapshot {
                provider: PlaybackProvider::Deezer,
                id: "1".into(),
                title: "Title".into(),
                artist: String::new(),
                album: String::new(),
                artwork: String::new(),
                duration: Duration::ZERO,
            };
            let metadata = metadata_for(&empty);
            assert_eq!(metadata.title, Some("Title"));
            assert_eq!(metadata.artist, None);
            assert_eq!(metadata.album, None);
            assert_eq!(metadata.cover_url, None);
            assert_eq!(metadata.duration, None);

            let full = TrackSnapshot {
                artist: "Artist".into(),
                album: "Album".into(),
                artwork: "https://art.example/cover.jpg".into(),
                duration: Duration::from_secs(61),
                ..empty
            };
            let metadata = metadata_for(&full);
            assert_eq!(metadata.artist, Some("Artist"));
            assert_eq!(metadata.album, Some("Album"));
            assert_eq!(metadata.cover_url, Some("https://art.example/cover.jpg"));
            assert_eq!(metadata.duration, Some(Duration::from_secs(61)));
        }

        #[test]
        fn playback_maps_position_to_playing_and_paused() {
            let command = PlaybackCommand {
                playing: true,
                position: Duration::from_secs(3),
            };
            assert_eq!(
                playback_for(command),
                MediaPlayback::Playing {
                    progress: Some(MediaPosition(Duration::from_secs(3)))
                }
            );
            let command = PlaybackCommand {
                playing: false,
                position: Duration::from_secs(3),
            };
            assert_eq!(
                playback_for(command),
                MediaPlayback::Paused {
                    progress: Some(MediaPosition(Duration::from_secs(3)))
                }
            );
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod stub {
    use futures::channel::mpsc::UnboundedReceiver;

    use super::MediaRequest;
    use crate::playback::PlaybackState;

    pub(crate) struct MediaSession;

    impl MediaSession {
        pub(crate) fn new(_hwnd: Option<isize>) -> Self {
            Self
        }

        pub(crate) fn take_events(&mut self) -> Option<UnboundedReceiver<MediaRequest>> {
            None
        }

        pub(crate) fn playback_changed(&mut self, _state: &PlaybackState) {}
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(crate) use native::MediaSession;
#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) use stub::MediaSession;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DEFAULT_SEEK_STEP, Plan, PlaybackCommand, SessionState, TrackSnapshot};
    use crate::playback::{PlaybackProvider, PlaybackState, PlaybackTrack};

    fn queue() -> Vec<PlaybackTrack> {
        ["first", "second"]
            .map(|id| PlaybackTrack {
                downloadable: false,
                progressive: false,
                provider: PlaybackProvider::Deezer,
                id: id.into(),
                title: format!("Track {id}"),
                artist: "  Artist  ".into(),
                album: "Album".into(),
                album_id: String::new(),
                release_date: String::new(),
                artists: Vec::new(),
                artwork: "https://art.example/cover.jpg".into(),
                duration: Duration::from_secs(120),
                explicit: false,
                ai_generated: false,
                service_url: String::new(),
            })
            .into_iter()
            .collect()
    }

    fn playing_state() -> PlaybackState {
        let mut state = PlaybackState::default();
        let generation = state.replace(queue(), 0).unwrap();
        assert!(state.loaded(generation, Some(Duration::from_secs(120))));
        state.seek(Duration::from_secs(10));
        state
    }

    #[test]
    fn default_seek_step_matches_the_original_ten_seconds() {
        assert_eq!(DEFAULT_SEEK_STEP, Duration::from_secs(10));
    }

    #[test]
    fn plan_attaches_track_then_deduplicates_repeated_syncs() {
        let mut session = SessionState::default();
        let state = playing_state();

        let Plan::Attach { track, playback } = session.plan(&state).unwrap() else {
            panic!("first sync attaches");
        };
        assert_eq!(
            track,
            TrackSnapshot {
                provider: PlaybackProvider::Deezer,
                id: "first".into(),
                title: "Track first".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                artwork: "https://art.example/cover.jpg".into(),
                duration: Duration::from_secs(120),
            }
        );
        assert_eq!(
            playback,
            PlaybackCommand {
                playing: true,
                position: Duration::from_secs(10),
            }
        );
        assert_eq!(session.plan(&state), None);
    }

    #[test]
    fn plan_publishes_playback_changes_without_reattaching_metadata() {
        let mut session = SessionState::default();
        let mut state = playing_state();
        assert!(matches!(session.plan(&state), Some(Plan::Attach { .. })));

        state.toggle();
        assert_eq!(
            session.plan(&state),
            Some(Plan::Playback(PlaybackCommand {
                playing: false,
                position: Duration::from_secs(10),
            }))
        );

        state.toggle();
        state.seek(Duration::from_secs(90));
        assert_eq!(
            session.plan(&state),
            Some(Plan::Playback(PlaybackCommand {
                playing: true,
                position: Duration::from_secs(90),
            }))
        );
    }

    #[test]
    fn plan_reattaches_when_the_track_or_its_duration_changes() {
        let mut session = SessionState::default();
        let mut state = playing_state();
        assert!(matches!(session.plan(&state), Some(Plan::Attach { .. })));

        let generation = state.select(1).unwrap();
        state.loaded(generation, Some(Duration::from_secs(180)));
        let Plan::Attach { track, playback } = session.plan(&state).unwrap() else {
            panic!("new track attaches");
        };
        assert_eq!(track.id, "second");
        assert_eq!(track.duration, Duration::from_secs(180));
        assert!(playback.playing);
    }

    #[test]
    fn plan_treats_any_non_playing_status_as_paused() {
        let mut session = SessionState::default();
        let mut state = PlaybackState::default();
        state.replace(queue(), 0).unwrap();
        assert_eq!(state.status, crate::playback::PlaybackStatus::Loading);
        let Plan::Attach { playback, .. } = session.plan(&state).unwrap() else {
            panic!("loading track attaches as paused");
        };
        assert!(!playback.playing);
        assert_eq!(playback.position, Duration::ZERO);
    }

    #[test]
    fn plan_clears_once_the_player_closes() {
        let mut session = SessionState::default();
        let mut state = playing_state();
        assert!(matches!(session.plan(&state), Some(Plan::Attach { .. })));

        state.clear();
        assert_eq!(session.plan(&state), Some(Plan::Clear));
        assert_eq!(session.plan(&state), None);

        state.replace(queue(), 1);
        assert!(matches!(session.plan(&state), Some(Plan::Attach { .. })));
    }

    #[test]
    fn plan_ignores_subsecond_position_updates() {
        let mut session = SessionState::default();
        let mut state = playing_state();
        assert!(matches!(session.plan(&state), Some(Plan::Attach { .. })));

        state.seek(Duration::from_millis(10_200));
        assert_eq!(session.plan(&state), None);

        state.seek(Duration::from_millis(10_800));
        assert_eq!(session.plan(&state), None);

        state.seek(Duration::from_millis(11_000));
        assert_eq!(
            session.plan(&state),
            Some(Plan::Playback(PlaybackCommand {
                playing: true,
                position: Duration::from_secs(11),
            }))
        );
    }
}
