use std::{
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::state::{PlaybackContext, PlaybackProvider};

const DEEZER_HISTORY_BIT: u8 = 1;
const SOUNDCLOUD_HISTORY_BIT: u8 = 1 << 1;

pub(super) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn deezer_next_media(track_id: &str) -> Value {
    json!({
        "next_media": {
            "media": {
                "id": track_id,
                "type": "song"
            }
        }
    })
}

fn numeric_id(value: &str) -> Option<String> {
    let id = value.trim();
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let id = id.parse::<u64>().ok()?;
    (id > 0).then(|| id.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SoundCloudListenReport {
    track_urn: String,
}

impl SoundCloudListenReport {
    pub(crate) fn from_playback(_context: &PlaybackContext, track_id: &str) -> Option<Self> {
        let track_id = numeric_id(track_id)?;
        let track_urn = format!("soundcloud:tracks:{track_id}");
        Some(Self { track_urn })
    }

    pub(crate) fn payload(&self) -> Value {
        json!({ "track_urn": self.track_urn })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ListenHistoryChanges(u8);

impl ListenHistoryChanges {
    pub(crate) fn contains(self, provider: PlaybackProvider) -> bool {
        self.0 & history_bit(provider) != 0
    }
}

#[derive(Debug, Default)]
pub(crate) struct ListenHistorySignal {
    changed: AtomicU8,
}

impl ListenHistorySignal {
    pub(crate) fn mark(&self, provider: PlaybackProvider) {
        self.changed
            .fetch_or(history_bit(provider), Ordering::Release);
    }

    pub(crate) fn take(&self) -> ListenHistoryChanges {
        ListenHistoryChanges(self.changed.swap(0, Ordering::Acquire))
    }
}

const fn history_bit(provider: PlaybackProvider) -> u8 {
    match provider {
        PlaybackProvider::Deezer => DEEZER_HISTORY_BIT,
        PlaybackProvider::SoundCloud => SOUNDCLOUD_HISTORY_BIT,
    }
}

pub(super) struct DeezerListenSession {
    track_id: String,
    stream_id: String,
    started_at: u64,
    listened: Duration,
    playing_since: Option<Instant>,
    pause_count: u32,
    seek_count: u32,
    shuffle: bool,
}

impl DeezerListenSession {
    pub(super) fn start(track_id: String, shuffle: bool, playing: bool) -> Self {
        Self::start_at(
            track_id,
            shuffle,
            playing,
            Instant::now(),
            unix_timestamp(),
            Uuid::new_v4().to_string(),
        )
    }

    fn start_at(
        track_id: String,
        shuffle: bool,
        playing: bool,
        now: Instant,
        timestamp: u64,
        stream_id: String,
    ) -> Self {
        Self {
            track_id,
            stream_id,
            started_at: timestamp,
            listened: Duration::ZERO,
            playing_since: playing.then_some(now),
            pause_count: 0,
            seek_count: 0,
            shuffle,
        }
    }

    pub(super) fn set_playing(&mut self, playing: bool) {
        self.set_playing_at(playing, Instant::now());
    }

    fn set_playing_at(&mut self, playing: bool, now: Instant) {
        match (self.playing_since, playing) {
            (Some(started), false) => {
                self.listened = self
                    .listened
                    .saturating_add(now.saturating_duration_since(started));
                self.playing_since = None;
                self.pause_count = self.pause_count.saturating_add(1);
            }
            (None, true) => self.playing_since = Some(now),
            _ => {}
        }
    }

    pub(super) fn record_seek(&mut self) {
        self.seek_count = self.seek_count.saturating_add(1);
    }

    pub(super) fn finish(mut self) -> Value {
        self.finish_at(Instant::now(), unix_timestamp())
    }

    fn finish_at(&mut self, now: Instant, timestamp: u64) -> Value {
        if let Some(started) = self.playing_since.take() {
            self.listened = self
                .listened
                .saturating_add(now.saturating_duration_since(started));
        }
        json!({
            "params": {
                "media": {
                    "id": self.track_id,
                    "type": "song",
                    "format": "MP3_128"
                },
                "type": 0,
                "stat": {
                    "seek": self.seek_count,
                    "pause": self.pause_count,
                    "sync": 0
                },
                "lt": self.listened.as_secs(),
                "payload": {},
                "ls": [],
                "ts_listen": self.started_at,
                "timestamp": timestamp,
                "is_shuffle": self.shuffle,
                "stream_id": self.stream_id
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundcloud_report_matches_the_mobile_history_contract() {
        let report = SoundCloudListenReport::from_playback(
            &PlaybackContext::SoundCloudCollection {
                context_urn: " soundcloud:playlists:00123 ".into(),
            },
            "000456",
        )
        .unwrap();
        assert_eq!(
            report.payload(),
            json!({ "track_urn": "soundcloud:tracks:456" })
        );
    }

    #[test]
    fn soundcloud_report_rejects_non_numeric_track_ids() {
        assert!(SoundCloudListenReport::from_playback(&PlaybackContext::None, "456/a").is_none());
        assert!(SoundCloudListenReport::from_playback(&PlaybackContext::None, "").is_none());
    }

    #[test]
    fn soundcloud_report_rejects_zero_and_overflow_ids() {
        let overflow = "18446744073709551616";
        for track_id in ["0", "0000", overflow] {
            assert!(
                SoundCloudListenReport::from_playback(&PlaybackContext::None, track_id).is_none(),
                "track ID should be rejected: {track_id}"
            );
        }
    }

    #[test]
    fn listen_history_signal_coalesces_provider_changes() {
        let signal = ListenHistorySignal::default();
        signal.mark(PlaybackProvider::Deezer);
        signal.mark(PlaybackProvider::SoundCloud);
        let changes = signal.take();
        assert!(changes.contains(PlaybackProvider::Deezer));
        assert!(changes.contains(PlaybackProvider::SoundCloud));
        assert_eq!(signal.take(), ListenHistoryChanges::default());
    }

    #[test]
    fn next_media_matches_the_captured_deezer_shape() {
        assert_eq!(
            deezer_next_media("469884852"),
            json!({ "next_media": { "media": { "id": "469884852", "type": "song" } } })
        );
    }

    #[test]
    fn completed_listen_counts_only_playing_time() {
        let started = Instant::now();
        let mut session = DeezerListenSession::start_at(
            "469884852".into(),
            true,
            true,
            started,
            100,
            "stream-id".into(),
        );
        session.set_playing_at(false, started + Duration::from_secs(4));
        session.set_playing_at(true, started + Duration::from_secs(9));
        session.record_seek();
        let payload = session.finish_at(started + Duration::from_secs(12), 112);

        assert_eq!(payload["params"]["lt"], 7);
        assert_eq!(payload["params"]["stat"]["pause"], 1);
        assert_eq!(payload["params"]["stat"]["seek"], 1);
        assert_eq!(payload["params"]["media"]["format"], "MP3_128");
        assert_eq!(payload["params"]["ts_listen"], 100);
        assert_eq!(payload["params"]["timestamp"], 112);
        assert_eq!(payload["params"]["is_shuffle"], true);
        assert_eq!(payload["params"]["stream_id"], "stream-id");
    }

    #[test]
    fn zero_length_listens_are_still_reported_like_the_capture() {
        let started = Instant::now();
        let mut session = DeezerListenSession::start_at(
            "track".into(),
            false,
            false,
            started,
            200,
            "stream".into(),
        );
        let payload = session.finish_at(started, 200);
        assert_eq!(payload["params"]["lt"], 0);
        assert_eq!(payload["params"]["stat"]["pause"], 0);
        assert_eq!(payload["params"]["stat"]["seek"], 0);
    }
}
