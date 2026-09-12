use std::collections::HashMap;

use tokio_util::sync::CancellationToken;

use crate::playback::{DownloadChoice, PlaybackProvider, PlaybackTrack};

/// Account-scoped identity for one track's format probe. The scope is an
/// opaque generation from AccountState, never a token or signed URL.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CapabilityKey {
    pub(crate) provider: PlaybackProvider,
    pub(crate) track_id: String,
    pub(crate) account_scope: u128,
}

impl CapabilityKey {
    pub(crate) fn new(track: &PlaybackTrack, account_scope: u128) -> Self {
        Self {
            provider: track.provider,
            track_id: track.id.clone(),
            account_scope,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CapabilityState {
    Checking,
    Failed,
    Ready(Vec<DownloadChoice>),
}

#[derive(Default)]
pub(crate) struct CapabilityCache {
    values: HashMap<CapabilityKey, CapabilityState>,
}

#[derive(Default)]
pub(crate) struct CapabilityProbeRegistry {
    tokens: HashMap<CapabilityKey, ProbeHandle>,
    next_id: u64,
}

#[derive(Clone)]
pub(crate) struct ProbeHandle {
    pub(crate) token: CancellationToken,
    pub(crate) id: u64,
}

impl CapabilityProbeRegistry {
    pub(crate) fn begin(&mut self, key: CapabilityKey) -> ProbeHandle {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        let token = CancellationToken::new();
        if let Some(previous) = self.tokens.insert(
            key,
            ProbeHandle {
                token: token.clone(),
                id,
            },
        ) {
            // A retry for the same account/track must not leave the previous
            // provider request running in the background.
            previous.token.cancel();
        }
        ProbeHandle { token, id }
    }

    pub(crate) fn finish(&mut self, key: &CapabilityKey, id: u64) -> bool {
        if self.tokens.get(key).is_some_and(|handle| handle.id == id) {
            self.tokens.remove(key);
            true
        } else {
            false
        }
    }

    pub(crate) fn cancel_all(&mut self) {
        for token in self.tokens.values() {
            token.token.cancel();
        }
        self.tokens.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.tokens.len()
    }
}

impl CapabilityCache {
    pub(crate) fn state(&self, key: &CapabilityKey) -> Option<CapabilityState> {
        self.values.get(key).cloned()
    }

    pub(crate) fn begin(&mut self, key: CapabilityKey) -> bool {
        if self.values.contains_key(&key) {
            return false;
        }
        self.values.insert(key, CapabilityState::Checking);
        true
    }

    pub(crate) fn finish(&mut self, key: CapabilityKey, choices: Vec<DownloadChoice>) {
        self.values.insert(key, CapabilityState::Ready(choices));
    }

    pub(crate) fn fail(&mut self, key: CapabilityKey) {
        self.values.insert(key, CapabilityState::Failed);
    }

    pub(crate) fn remove(&mut self, key: &CapabilityKey) {
        self.values.remove(key);
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = CapabilityKey> + '_ {
        self.values.keys().cloned()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn track(provider: PlaybackProvider, id: &str) -> PlaybackTrack {
        PlaybackTrack {
            provider,
            id: id.into(),
            title: "title".into(),
            artist: "artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(1),
            downloadable: false,
            progressive: true,
            explicit: false,
            service_url: String::new(),
        }
    }

    #[test]
    fn key_is_scoped_by_provider_track_and_account_generation() {
        let deezer = CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 1);
        assert_ne!(
            deezer,
            CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 2)
        );
        assert_ne!(
            deezer,
            CapabilityKey::new(&track(PlaybackProvider::SoundCloud, "42"), 1)
        );
        assert_ne!(
            deezer,
            CapabilityKey::new(&track(PlaybackProvider::Deezer, "43"), 1)
        );
    }

    #[test]
    fn account_scope_invalidation_clears_pending_and_ready_entries() {
        let mut cache = CapabilityCache::default();
        let key = CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 1);
        assert!(cache.begin(key.clone()));
        assert_eq!(cache.state(&key), Some(CapabilityState::Checking));
        cache.finish(key.clone(), Vec::new());
        assert_eq!(cache.state(&key), Some(CapabilityState::Ready(Vec::new())));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.state(&key), None);
    }

    #[test]
    fn failed_probe_is_retryable_instead_of_becoming_a_cached_empty_menu() {
        let mut cache = CapabilityCache::default();
        let key = CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 1);
        assert!(cache.begin(key.clone()));
        cache.fail(key.clone());
        assert_eq!(cache.state(&key), Some(CapabilityState::Failed));
        cache.remove(&key);
        assert_eq!(cache.state(&key), None);
        assert!(cache.begin(key));
    }

    #[test]
    fn probe_registry_cancels_in_flight_requests_on_scope_change() {
        let mut registry = CapabilityProbeRegistry::default();
        let key = CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 1);
        let handle = registry.begin(key);
        assert!(!handle.token.is_cancelled());
        assert_eq!(registry.len(), 1);
        registry.cancel_all();
        assert!(handle.token.is_cancelled());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn replacing_a_probe_cancels_the_previous_handle() {
        let mut registry = CapabilityProbeRegistry::default();
        let key = CapabilityKey::new(&track(PlaybackProvider::Deezer, "42"), 1);
        let previous = registry.begin(key.clone());
        let current = registry.begin(key.clone());
        assert!(previous.token.is_cancelled());
        assert!(!current.token.is_cancelled());
        assert!(!registry.finish(&key, previous.id));
        assert_eq!(registry.len(), 1);
        assert!(registry.finish(&key, current.id));
        assert_eq!(registry.len(), 0);
    }
}
