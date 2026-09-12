use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const RESOLVE_WINDOW: Duration = Duration::from_secs(10);
const RESOLVE_LIMIT: usize = 6;
const PROVIDER_MIN_INTERVAL: Duration = Duration::from_millis(1200);
const CANCELLED_MESSAGE: &str = "Playback request cancelled";

/// Coordinates the two resolve budgets shared by playback and downloads.
///
/// The original app used a sliding-window reservation before each stream
/// resolve, then paced provider commands at a minimum interval in the
/// backend. Keeping both gates here makes that policy apply to every clone of
/// a resolver while allowing cancellation to abandon stale skip requests.
#[derive(Clone)]
pub(crate) struct ResolveLimiter {
    state: Arc<ResolveLimiterState>,
}

struct ResolveLimiterState {
    recent_resolves: Mutex<SlidingWindow>,
    reservation_queue: AsyncMutex<()>,
    provider_next: AsyncMutex<Instant>,
    window: Duration,
    limit: usize,
    provider_min_interval: Duration,
}

#[derive(Debug, Default)]
struct SlidingWindow {
    recent: VecDeque<Instant>,
}

impl SlidingWindow {
    fn hold_off_at(&mut self, now: Instant, window: Duration, limit: usize) -> Duration {
        self.prune(now, window);
        if limit == 0 || self.recent.len() < limit {
            return Duration::ZERO;
        }

        let oldest_in_window = self.recent[self.recent.len() - limit];
        oldest_in_window
            .checked_add(window)
            .map_or(Duration::ZERO, |deadline| {
                deadline.saturating_duration_since(now)
            })
    }

    fn note_at(&mut self, now: Instant, window: Duration) {
        self.prune(now, window);
        self.recent.push_back(now);
    }

    fn prune(&mut self, now: Instant, window: Duration) {
        while self
            .recent
            .front()
            .is_some_and(|timestamp| now.saturating_duration_since(*timestamp) >= window)
        {
            self.recent.pop_front();
        }
    }
}

impl ResolveLimiter {
    pub(crate) fn new() -> Self {
        static SHARED: OnceLock<ResolveLimiter> = OnceLock::new();
        SHARED
            .get_or_init(|| Self::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, PROVIDER_MIN_INTERVAL))
            .clone()
    }

    fn with_config(window: Duration, limit: usize, provider_min_interval: Duration) -> Self {
        assert!(limit > 0, "the resolve window limit must be positive");
        Self {
            state: Arc::new(ResolveLimiterState {
                recent_resolves: Mutex::new(SlidingWindow::default()),
                reservation_queue: AsyncMutex::new(()),
                provider_next: AsyncMutex::new(Instant::now()),
                window,
                limit,
                provider_min_interval,
            }),
        }
    }

    /// Returns the current sliding-window delay without recording a resolve.
    pub(crate) fn hold_off(&self) -> Duration {
        let mut recent = self
            .state
            .recent_resolves
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.hold_off_at(Instant::now(), self.state.window, self.state.limit)
    }

    /// Records a resolve at the current monotonic clock instant.
    pub(crate) fn note_resolve(&self) {
        let mut recent = self
            .state
            .recent_resolves
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recent.note_at(Instant::now(), self.state.window);
    }

    #[cfg(test)]
    fn recorded_resolves(&self) -> usize {
        self.state
            .recent_resolves
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recent
            .len()
    }

    /// Reserves the next provider resolve in FIFO order.
    ///
    /// This combines the original app's sliding-window reservation with its
    /// provider minimum interval. The reservation is not recorded until both
    /// waits complete, so cancelling a stale request never spends resolve
    /// budget on a request that will not be sent.
    pub(crate) async fn reserve(&self, cancellation: &CancellationToken) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }

        let _queue = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
            queue = self.state.reservation_queue.lock() => queue,
        };

        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }

        let hold_off = self.hold_off();
        if !hold_off.is_zero() {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                _ = tokio::time::sleep(hold_off) => {}
            }
        }

        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }

        self.wait_for_provider_slot(cancellation).await?;
        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }

        self.note_resolve();
        Ok(())
    }

    /// Waits for the next provider request slot, preserving a 1.2 second
    /// minimum spacing between provider resolve commands.
    pub(crate) async fn wait_for_provider_slot(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }

        let mut next = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
            next = self.state.provider_next.lock() => next,
        };
        let now = Instant::now();
        if *next > now {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                _ = tokio::time::sleep(*next - now) => {}
            }
        }

        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }
        *next = Instant::now() + self.state.provider_min_interval;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_resolve_has_no_sliding_window_delay() {
        let now = Instant::now();
        let mut window = SlidingWindow::default();

        assert_eq!(
            window.hold_off_at(now, RESOLVE_WINDOW, RESOLVE_LIMIT),
            Duration::ZERO
        );
        window.note_at(now, RESOLVE_WINDOW);
        assert_eq!(
            window.hold_off_at(now, RESOLVE_WINDOW, RESOLVE_LIMIT),
            Duration::ZERO
        );
    }

    #[test]
    fn seventh_resolve_waits_for_the_oldest_in_window_to_expire() {
        let now = Instant::now();
        let mut window = SlidingWindow::default();
        for offset in 0..RESOLVE_LIMIT {
            window.note_at(now + Duration::from_secs(offset as u64), RESOLVE_WINDOW);
        }

        let check_at = now + Duration::from_secs(RESOLVE_LIMIT as u64 - 1);
        assert_eq!(
            window.hold_off_at(check_at, RESOLVE_WINDOW, RESOLVE_LIMIT),
            Duration::from_secs(RESOLVE_WINDOW.as_secs() - RESOLVE_LIMIT as u64 + 1)
        );
    }

    #[test]
    fn expired_timestamps_slide_out_of_the_window() {
        let now = Instant::now();
        let mut window = SlidingWindow::default();
        for _ in 0..RESOLVE_LIMIT {
            window.note_at(now, RESOLVE_WINDOW);
        }

        assert_eq!(
            window.hold_off_at(now + RESOLVE_WINDOW, RESOLVE_WINDOW, RESOLVE_LIMIT),
            Duration::ZERO
        );
    }

    #[tokio::test]
    async fn cancellation_during_hold_off_does_not_record_a_resolve() {
        let limiter =
            ResolveLimiter::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, Duration::from_millis(1));
        for _ in 0..RESOLVE_LIMIT {
            limiter.note_resolve();
        }
        let cancellation = CancellationToken::new();
        let task_limiter = limiter.clone();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move { task_limiter.reserve(&task_cancellation).await });
        tokio::task::yield_now().await;
        cancellation.cancel();

        assert_eq!(task.await.unwrap(), Err(CANCELLED_MESSAGE.into()));
        assert_eq!(limiter.recorded_resolves(), RESOLVE_LIMIT);
        assert!(limiter.hold_off() > Duration::ZERO);
    }

    #[tokio::test]
    async fn two_reservations_are_spaced_by_the_provider_interval() {
        let test_interval = Duration::from_millis(20);
        assert_eq!(PROVIDER_MIN_INTERVAL, Duration::from_millis(1200));
        let limiter = ResolveLimiter::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, test_interval);
        let cancellation = CancellationToken::new();

        limiter.reserve(&cancellation).await.unwrap();
        let started = Instant::now();
        limiter.reserve(&cancellation).await.unwrap();

        assert!(started.elapsed() >= test_interval);
    }

    #[test]
    fn clones_share_the_sliding_window_budget() {
        let limiter =
            ResolveLimiter::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, Duration::from_millis(1));
        let clone = limiter.clone();

        for _ in 0..RESOLVE_LIMIT {
            limiter.note_resolve();
        }

        assert!(clone.hold_off() > Duration::ZERO);
    }

    #[test]
    fn separately_created_limiters_share_the_process_budget() {
        let first = ResolveLimiter::new();
        let second = ResolveLimiter::new();

        assert!(Arc::ptr_eq(&first.state, &second.state));
    }

    #[tokio::test]
    async fn cancellation_while_queued_does_not_block_the_next_reservation() {
        let limiter = ResolveLimiter::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, Duration::ZERO);
        let queue_guard = limiter.state.reservation_queue.lock().await;

        let queued_cancellation = CancellationToken::new();
        let queued = tokio::spawn({
            let limiter = limiter.clone();
            let cancellation = queued_cancellation.clone();
            async move { limiter.reserve(&cancellation).await }
        });
        tokio::task::yield_now().await;
        queued_cancellation.cancel();
        assert_eq!(queued.await.unwrap(), Err(CANCELLED_MESSAGE.into()));
        drop(queue_guard);

        let next_cancellation = CancellationToken::new();
        assert!(limiter.reserve(&next_cancellation).await.is_ok());
    }
}
