use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

// Media URLs are short-lived, but a normal listening session can outlast the
// old 90-second window. Media failures still invalidate and refresh entries.
pub(crate) const RESOLVED_SOURCE_TTL: Duration = Duration::from_secs(600);
pub(crate) const RESOLVED_SOURCE_MAX_ENTRIES: usize = 60;

/// Values kept by the resolved-source cache must be safe to replay without
/// retaining a media payload in memory. The resolver implements this for
/// `ResolvedSource` by accepting remote URL and HLS descriptor variants only.
pub(crate) trait SourceCacheValue: Clone {
    fn is_cacheable(&self) -> bool;
}

#[derive(Clone)]
pub(crate) struct ResolvedSourceCache<T: SourceCacheValue> {
    state: Arc<Mutex<CacheState<T>>>,
    ttl: Duration,
    max_entries: usize,
}

/// Coordinates one in-progress provider resolve per cache key. Completed
/// values remain the responsibility of `ResolvedSourceCache`; this map exists
/// only long enough for concurrent callers to share the same network work.
#[derive(Clone)]
pub(crate) struct SourceResolveFlights<T: Clone> {
    entries: Arc<Mutex<HashMap<String, Arc<SourceResolveFlight<T>>>>>,
}

pub(crate) struct SourceResolveFlight<T: Clone> {
    result: Mutex<Option<Result<T, String>>>,
    completed: Notify,
}

impl<T: Clone> SourceResolveFlights<T> {
    pub(crate) fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns the flight for `key` and whether this caller must start the
    /// shared work. Keys are bounded by the resolver cache key space and are
    /// removed as soon as their work completes.
    pub(crate) fn begin(&self, key: String) -> (Arc<SourceResolveFlight<T>>, bool) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(flight) = entries.get(&key) {
            return (flight.clone(), false);
        }
        let flight = Arc::new(SourceResolveFlight {
            result: Mutex::new(None),
            completed: Notify::new(),
        });
        entries.insert(key, flight.clone());
        (flight, true)
    }

    pub(crate) async fn wait(
        &self,
        flight: &Arc<SourceResolveFlight<T>>,
        cancellation: &CancellationToken,
    ) -> Result<T, String> {
        loop {
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            // Register before checking the result so a completion between the
            // check and select cannot strand this waiter.
            let notified = flight.completed.notified();
            if let Some(result) = flight
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                return result;
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
                _ = notified => {}
            }
        }
    }

    pub(crate) fn finish(
        &self,
        key: &str,
        flight: &Arc<SourceResolveFlight<T>>,
        result: Result<T, String>,
    ) {
        {
            let mut stored = flight
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *stored = Some(result);
        }
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, flight))
        {
            entries.remove(key);
        }
        drop(entries);
        flight.completed.notify_waiters();
    }

    /// A resolver epoch reset makes every existing flight stale. Dropping the
    /// map entries lets requests in the new account/session start fresh work;
    /// old workers retain their own `Arc` and can still notify old waiters.
    pub(crate) fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

struct CacheState<T> {
    entries: HashMap<String, CacheEntry<T>>,
    lru: VecDeque<String>,
    epoch: u64,
}

struct CacheEntry<T> {
    stored_at: Instant,
    value: T,
}

impl<T: SourceCacheValue> ResolvedSourceCache<T> {
    pub(crate) fn new() -> Self {
        Self::with_config(RESOLVED_SOURCE_TTL, RESOLVED_SOURCE_MAX_ENTRIES)
    }

    fn with_config(ttl: Duration, max_entries: usize) -> Self {
        assert!(
            max_entries > 0,
            "the resolved-source cache must have capacity"
        );
        Self {
            state: Arc::new(Mutex::new(CacheState {
                entries: HashMap::new(),
                lru: VecDeque::new(),
                epoch: 0,
            })),
            ttl,
            max_entries,
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<T> {
        self.get_at(key, Instant::now())
    }

    fn get_at(&self, key: &str, now: Instant) -> Option<T> {
        if key.is_empty() {
            return None;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let expired = state
            .entries
            .get(key)
            .is_some_and(|entry| now.saturating_duration_since(entry.stored_at) > self.ttl);
        if expired {
            state.entries.remove(key);
            remove_lru_key(&mut state.lru, key);
            return None;
        }
        let value = state.entries.get(key).map(|entry| entry.value.clone());
        if value.is_some() {
            touch_lru(&mut state.lru, key);
        }
        value
    }

    #[cfg(test)]
    pub(crate) fn insert(&self, key: impl Into<String>, value: T) {
        self.insert_at(key.into(), value, Instant::now());
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .epoch
    }

    /// Insert only if no account/session reset has cleared the cache since
    /// the caller started resolving the source.
    pub(crate) fn insert_if_epoch(
        &self,
        expected_epoch: u64,
        key: impl Into<String>,
        value: T,
    ) -> bool {
        let key = key.into();
        if key.is_empty() || !value.is_cacheable() {
            return false;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.epoch != expected_epoch {
            return false;
        }
        state.entries.remove(&key);
        remove_lru_key(&mut state.lru, &key);
        state.entries.insert(
            key.clone(),
            CacheEntry {
                stored_at: Instant::now(),
                value,
            },
        );
        state.lru.push_back(key);
        while state.entries.len() > self.max_entries {
            let Some(oldest) = state.lru.pop_front() else {
                break;
            };
            state.entries.remove(&oldest);
        }
        true
    }

    #[cfg(test)]
    fn insert_at(&self, key: String, value: T, now: Instant) {
        if key.is_empty() || !value.is_cacheable() {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.entries.remove(&key);
        remove_lru_key(&mut state.lru, &key);
        state.entries.insert(
            key.clone(),
            CacheEntry {
                stored_at: now,
                value,
            },
        );
        state.lru.push_back(key);
        while state.entries.len() > self.max_entries {
            let Some(oldest) = state.lru.pop_front() else {
                break;
            };
            state.entries.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn invalidate(&self, key: &str) {
        if key.is_empty() {
            return;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.entries.remove(key);
        remove_lru_key(&mut state.lru, key);
    }

    pub(crate) fn invalidate_if_epoch(&self, expected_epoch: u64, key: &str) -> bool {
        if key.is_empty() {
            return false;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.epoch != expected_epoch {
            return false;
        }
        state.entries.remove(key);
        remove_lru_key(&mut state.lru, key);
        true
    }

    pub(crate) fn clear(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.epoch = state.epoch.wrapping_add(1);
        state.entries.clear();
        state.lru.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .len()
    }
}

fn touch_lru(lru: &mut VecDeque<String>, key: &str) {
    remove_lru_key(lru, key);
    lru.push_back(key.to_owned());
}

fn remove_lru_key(lru: &mut VecDeque<String>, key: &str) {
    if let Some(index) = lru.iter().position(|candidate| candidate == key) {
        lru.remove(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum TestSource {
        Remote(u8),
        Inline(Vec<u8>),
    }

    impl SourceCacheValue for TestSource {
        fn is_cacheable(&self) -> bool {
            matches!(self, Self::Remote(_))
        }
    }

    fn cache(ttl: Duration, max_entries: usize) -> ResolvedSourceCache<TestSource> {
        ResolvedSourceCache::with_config(ttl, max_entries)
    }

    #[test]
    fn hit_refreshes_lru_order() {
        let cache = cache(RESOLVED_SOURCE_TTL, 2);
        let now = Instant::now();
        cache.insert_at("first".into(), TestSource::Remote(1), now);
        cache.insert_at("second".into(), TestSource::Remote(2), now);
        assert_eq!(cache.get_at("first", now), Some(TestSource::Remote(1)));

        cache.insert_at("third".into(), TestSource::Remote(3), now);

        assert!(cache.get_at("first", now).is_some());
        assert!(cache.get_at("second", now).is_none());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn entries_expire_after_the_ttl() {
        let ttl = RESOLVED_SOURCE_TTL;
        let cache = cache(ttl, 2);
        let now = Instant::now();
        cache.insert_at("track".into(), TestSource::Remote(1), now);

        assert!(cache.get_at("track", now + ttl).is_some());
        assert!(
            cache
                .get_at("track", now + ttl + Duration::from_nanos(1))
                .is_none()
        );
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn inline_sources_are_never_retained() {
        let cache = cache(Duration::from_secs(90), 2);
        cache.insert("inline", TestSource::Inline(vec![1, 2, 3]));

        assert!(cache.get("inline").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn invalidating_a_key_removes_it_immediately() {
        let cache = cache(Duration::from_secs(90), 2);
        cache.insert("track", TestSource::Remote(1));
        cache.invalidate("track");

        assert!(cache.get("track").is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn clearing_the_cache_removes_all_entries_and_lru_state() {
        let cache = cache(Duration::from_secs(90), 2);
        cache.insert("first", TestSource::Remote(1));
        cache.insert("second", TestSource::Remote(2));

        cache.clear();

        assert!(cache.get("first").is_none());
        assert!(cache.get("second").is_none());
        cache.insert("replacement", TestSource::Remote(3));
        assert_eq!(cache.get("replacement"), Some(TestSource::Remote(3)));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn stale_epoch_cannot_reinsert_after_a_clear() {
        let cache = cache(Duration::from_secs(90), 2);
        let epoch = cache.epoch();
        cache.clear();

        assert!(!cache.insert_if_epoch(epoch, "stale", TestSource::Remote(1)));
        assert!(cache.get("stale").is_none());
        assert!(cache.insert_if_epoch(cache.epoch(), "current", TestSource::Remote(2)));
        assert_eq!(cache.get("current"), Some(TestSource::Remote(2)));
    }

    #[test]
    fn stale_epoch_cannot_invalidate_a_new_entry() {
        let cache = cache(Duration::from_secs(90), 2);
        let stale_epoch = cache.epoch();
        cache.clear();
        cache.insert_if_epoch(cache.epoch(), "current", TestSource::Remote(2));

        assert!(!cache.invalidate_if_epoch(stale_epoch, "current"));
        assert_eq!(cache.get("current"), Some(TestSource::Remote(2)));
    }

    #[tokio::test]
    async fn same_key_callers_share_one_flight_and_cleanup_after_completion() {
        let flights = SourceResolveFlights::<TestSource>::new();
        let (first, first_starts_work) = flights.begin("track".into());
        let (second, second_starts_work) = flights.begin("track".into());

        assert!(first_starts_work);
        assert!(!second_starts_work);
        assert!(Arc::ptr_eq(&first, &second));

        flights.finish("track", &first, Ok(TestSource::Remote(7)));
        let cancellation = CancellationToken::new();
        assert_eq!(
            flights.wait(&second, &cancellation).await,
            Ok(TestSource::Remote(7))
        );
        assert_eq!(flights.len(), 0);
    }

    #[tokio::test]
    async fn different_keys_start_independent_flights() {
        let flights = SourceResolveFlights::<TestSource>::new();
        let (first, first_starts_work) = flights.begin("first".into());
        let (second, second_starts_work) = flights.begin("second".into());

        assert!(first_starts_work);
        assert!(second_starts_work);
        assert!(!Arc::ptr_eq(&first, &second));

        flights.finish("first", &first, Ok(TestSource::Remote(1)));
        flights.finish("second", &second, Ok(TestSource::Remote(2)));
        let cancellation = CancellationToken::new();
        assert_eq!(
            flights.wait(&first, &cancellation).await,
            Ok(TestSource::Remote(1))
        );
        assert_eq!(
            flights.wait(&second, &cancellation).await,
            Ok(TestSource::Remote(2))
        );
    }

    #[tokio::test]
    async fn cancelling_one_waiter_does_not_cancel_the_shared_flight() {
        let flights = SourceResolveFlights::<TestSource>::new();
        let (flight, starts_work) = flights.begin("track".into());
        assert!(starts_work);

        let cancelled = CancellationToken::new();
        let cancelled_waiter = {
            let flights = flights.clone();
            let flight = flight.clone();
            let cancellation = cancelled.clone();
            tokio::spawn(async move { flights.wait(&flight, &cancellation).await })
        };
        tokio::task::yield_now().await;
        cancelled.cancel();
        assert_eq!(
            cancelled_waiter.await.unwrap(),
            Err("Playback request cancelled".into())
        );

        flights.finish("track", &flight, Ok(TestSource::Remote(9)));
        let active = CancellationToken::new();
        assert_eq!(
            flights.wait(&flight, &active).await,
            Ok(TestSource::Remote(9))
        );
    }

    #[tokio::test]
    async fn failed_and_cleared_flights_do_not_retire_newer_work() {
        let flights = SourceResolveFlights::<TestSource>::new();
        let (old, starts_work) = flights.begin("track".into());
        assert!(starts_work);
        flights.clear();
        let (current, starts_work) = flights.begin("track".into());
        assert!(starts_work);
        assert!(!Arc::ptr_eq(&old, &current));

        flights.finish("track", &old, Err("old failure".into()));
        assert_eq!(flights.len(), 1);
        flights.finish("track", &current, Err("current failure".into()));
        assert_eq!(flights.len(), 0);
    }
}
