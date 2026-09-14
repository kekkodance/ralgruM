use super::{LyricsCacheKey, LyricsResponse};
use std::collections::{HashMap, VecDeque};

pub const DEFAULT_LYRICS_CACHE_CAPACITY: usize = 80;

pub trait LyricsCacheStore {
    fn get(&self, key: &LyricsCacheKey) -> Option<&LyricsResponse>;
    fn set(&mut self, key: LyricsCacheKey, value: LyricsResponse);
}

pub struct LyricsCache {
    entries: HashMap<LyricsCacheKey, LyricsResponse>,
    order: VecDeque<LyricsCacheKey>,
    capacity: usize,
}

impl Default for LyricsCache {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_LYRICS_CACHE_CAPACITY)
    }
}

impl LyricsCache {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }
}

impl LyricsCacheStore for LyricsCache {
    fn get(&self, key: &LyricsCacheKey) -> Option<&LyricsResponse> {
        self.entries.get(key)
    }

    fn set(&mut self, key: LyricsCacheKey, value: LyricsResponse) {
        if self.capacity == 0 {
            return;
        }
        if let Some(position) = self.order.iter().position(|item| item == &key) {
            self.order.remove(position);
        }
        self.entries.insert(key.clone(), value);
        self.order.push_back(key);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LyricsProvider, LyricsTrack};
    use super::*;
    #[test]
    fn cache_separates_provider_and_track_identity() {
        let track = LyricsTrack {
            stable_id: Some("7".into()),
            ..Default::default()
        };
        let mut cache = LyricsCache::default();
        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track),
            LyricsResponse::Plain {
                text: "x".into(),
                url: None,
            },
        );
        assert!(
            cache
                .get(&LyricsCacheKey::new(LyricsProvider::Musixmatch, &track))
                .is_none()
        );
        assert!(
            cache
                .get(&LyricsCacheKey::new(LyricsProvider::Genius, &track))
                .is_some()
        );
    }

    #[test]
    fn stable_id_takes_precedence_over_metadata() {
        let first = LyricsTrack {
            stable_id: Some("track-1".into()),
            artist: "A".into(),
            title: "One".into(),
            ..Default::default()
        };
        let second = LyricsTrack {
            stable_id: Some(" track-1 ".into()),
            artist: "B".into(),
            title: "Two".into(),
            ..Default::default()
        };
        assert_eq!(
            LyricsCacheKey::new(LyricsProvider::Genius, &first),
            LyricsCacheKey::new(LyricsProvider::Genius, &second)
        );
    }

    #[test]
    fn metadata_fallback_normalizes_text_but_separates_track_fields() {
        let first = LyricsTrack {
            artist: " The  Artist ".into(),
            title: "SONG".into(),
            album: Some("Album".into()),
            duration: Some(180),
            stable_id: None,
        };
        let normalized = LyricsTrack {
            artist: "the artist".into(),
            title: " song ".into(),
            album: Some(" album ".into()),
            duration: Some(180),
            stable_id: None,
        };
        let other_duration = LyricsTrack {
            duration: Some(181),
            ..normalized.clone()
        };
        assert_eq!(
            LyricsCacheKey::new(LyricsProvider::Musixmatch, &first),
            LyricsCacheKey::new(LyricsProvider::Musixmatch, &normalized)
        );
        assert_ne!(
            LyricsCacheKey::new(LyricsProvider::Musixmatch, &first),
            LyricsCacheKey::new(LyricsProvider::Musixmatch, &other_duration)
        );
    }

    #[test]
    fn evicts_oldest_when_capacity_exceeded() {
        let mut cache = LyricsCache::default();
        for index in 0..81 {
            let track = LyricsTrack {
                stable_id: Some(format!("track-{index}")),
                ..Default::default()
            };
            cache.set(
                LyricsCacheKey::new(LyricsProvider::Genius, &track),
                LyricsResponse::Plain {
                    text: format!("{index}"),
                    url: None,
                },
            );
        }

        let first = LyricsTrack {
            stable_id: Some("track-0".into()),
            ..Default::default()
        };
        let second = LyricsTrack {
            stable_id: Some("track-1".into()),
            ..Default::default()
        };
        let last = LyricsTrack {
            stable_id: Some("track-80".into()),
            ..Default::default()
        };

        assert!(
            cache
                .get(&LyricsCacheKey::new(LyricsProvider::Genius, &first))
                .is_none()
        );
        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &second)),
            Some(&LyricsResponse::Plain {
                text: "1".into(),
                url: None
            })
        );
        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &last)),
            Some(&LyricsResponse::Plain {
                text: "80".into(),
                url: None
            })
        );
    }

    #[test]
    fn replacement_refreshes_recency() {
        let mut cache = LyricsCache::with_capacity(2);
        let track1 = LyricsTrack {
            stable_id: Some("track-1".into()),
            ..Default::default()
        };
        let track2 = LyricsTrack {
            stable_id: Some("track-2".into()),
            ..Default::default()
        };
        let track3 = LyricsTrack {
            stable_id: Some("track-3".into()),
            ..Default::default()
        };

        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track1),
            LyricsResponse::Plain {
                text: "1".into(),
                url: None,
            },
        );
        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track2),
            LyricsResponse::Plain {
                text: "2".into(),
                url: None,
            },
        );
        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track1),
            LyricsResponse::Plain {
                text: "1-updated".into(),
                url: None,
            },
        );
        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track3),
            LyricsResponse::Plain {
                text: "3".into(),
                url: None,
            },
        );

        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &track1)),
            Some(&LyricsResponse::Plain {
                text: "1-updated".into(),
                url: None
            })
        );
        assert!(
            cache
                .get(&LyricsCacheKey::new(LyricsProvider::Genius, &track2))
                .is_none()
        );
        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &track3)),
            Some(&LyricsResponse::Plain {
                text: "3".into(),
                url: None
            })
        );
    }

    #[test]
    fn get_does_not_refresh_recency() {
        let mut cache = LyricsCache::with_capacity(2);
        let track1 = LyricsTrack {
            stable_id: Some("track-1".into()),
            ..Default::default()
        };
        let track2 = LyricsTrack {
            stable_id: Some("track-2".into()),
            ..Default::default()
        };
        let track3 = LyricsTrack {
            stable_id: Some("track-3".into()),
            ..Default::default()
        };

        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track1),
            LyricsResponse::Plain {
                text: "1".into(),
                url: None,
            },
        );
        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track2),
            LyricsResponse::Plain {
                text: "2".into(),
                url: None,
            },
        );

        let _ = cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &track1));

        cache.set(
            LyricsCacheKey::new(LyricsProvider::Genius, &track3),
            LyricsResponse::Plain {
                text: "3".into(),
                url: None,
            },
        );

        assert!(
            cache
                .get(&LyricsCacheKey::new(LyricsProvider::Genius, &track1))
                .is_none()
        );
        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &track2)),
            Some(&LyricsResponse::Plain {
                text: "2".into(),
                url: None
            })
        );
        assert_eq!(
            cache.get(&LyricsCacheKey::new(LyricsProvider::Genius, &track3)),
            Some(&LyricsResponse::Plain {
                text: "3".into(),
                url: None
            })
        );
    }
}
