use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio_util::sync::CancellationToken;

const RESOLVE_WINDOW: Duration = Duration::from_secs(10);
const RESOLVE_LIMIT: usize = 6;
const PROVIDER_MIN_INTERVAL: Duration = Duration::from_millis(1200);
const CANCELLED_MESSAGE: &str = "Playback request cancelled";
const MAX_INTERACTIVE_BURST: usize = 3;

/// Current-track work must not wait behind speculative prefetches or queued
/// downloads. Background work still receives a reserved turn after a small
/// interactive burst so it cannot starve forever.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResolvePriority {
    Interactive,
    Background,
}

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
    provider_next: Mutex<Instant>,
    interactive_waiters: AtomicUsize,
    background_waiters: AtomicUsize,
    interactive_streak: AtomicUsize,
    priority_changed: Notify,
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
                provider_next: Mutex::new(Instant::now()),
                interactive_waiters: AtomicUsize::new(0),
                background_waiters: AtomicUsize::new(0),
                interactive_streak: AtomicUsize::new(0),
                priority_changed: Notify::new(),
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

    /// Reserves the next provider resolve. Interactive playback is preferred
    /// over background prefetch and downloads, while a background reservation
    /// is admitted after a bounded interactive burst.
    ///
    /// This combines the original app's sliding-window reservation with its
    /// provider minimum interval. The reservation is not recorded until both
    /// waits complete, so cancelling a stale request never spends resolve
    /// budget on a request that will not be sent.
    pub(crate) async fn reserve(
        &self,
        priority: ResolvePriority,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Err(CANCELLED_MESSAGE.into());
        }
        let _waiter = PriorityWaiter::new(&self.state, priority);

        loop {
            let queue = self.acquire_turn(priority, cancellation).await?;
            let now = Instant::now();
            let hold_off = self.hold_off();
            let provider_hold_off = self
                .state
                .provider_next
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .saturating_duration_since(now);
            let delay = hold_off.max(provider_hold_off);

            if delay.is_zero() {
                if cancellation.is_cancelled() {
                    return Err(CANCELLED_MESSAGE.into());
                }
                self.note_resolve();
                *self
                    .state
                    .provider_next
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Instant::now() + self.state.provider_min_interval;
                self.note_priority_turn(priority);
                drop(queue);
                return Ok(());
            }

            // Do not keep the admission lock while sleeping. A current-track
            // request arriving during a background wait can claim the next
            // safe provider slot, and every contender rechecks both budgets.
            drop(queue);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }

    async fn acquire_turn(
        &self,
        priority: ResolvePriority,
        cancellation: &CancellationToken,
    ) -> Result<tokio::sync::MutexGuard<'_, ()>, String> {
        loop {
            let notified = self.state.priority_changed.notified();
            if !self.priority_can_run(priority) {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                    _ = notified => continue,
                }
            }
            let queue = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                queue = self.state.reservation_queue.lock() => queue,
            };
            if self.priority_can_run(priority) {
                return Ok(queue);
            }
            // Register before releasing the queue guard, then recheck the
            // condition. This closes the notification window where the
            // preferred class could change between the check and the wait.
            let notified = self.state.priority_changed.notified();
            drop(queue);
            if self.priority_can_run(priority) {
                continue;
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(CANCELLED_MESSAGE.into()),
                _ = notified => {}
            }
        }
    }

    fn priority_can_run(&self, priority: ResolvePriority) -> bool {
        let interactive = self.state.interactive_waiters.load(Ordering::Acquire);
        let background = self.state.background_waiters.load(Ordering::Acquire);
        let force_background = background > 0
            && self.state.interactive_streak.load(Ordering::Acquire) >= MAX_INTERACTIVE_BURST;
        match priority {
            ResolvePriority::Interactive => !force_background,
            ResolvePriority::Background => interactive == 0 || force_background,
        }
    }

    fn note_priority_turn(&self, priority: ResolvePriority) {
        match priority {
            ResolvePriority::Interactive => {
                self.state.interactive_streak.fetch_add(1, Ordering::AcqRel);
            }
            ResolvePriority::Background => {
                self.state.interactive_streak.store(0, Ordering::Release);
            }
        }
        self.state.priority_changed.notify_waiters();
    }
}

struct PriorityWaiter<'a> {
    state: &'a ResolveLimiterState,
    priority: ResolvePriority,
}

impl<'a> PriorityWaiter<'a> {
    fn new(state: &'a ResolveLimiterState, priority: ResolvePriority) -> Self {
        match priority {
            ResolvePriority::Interactive => {
                state.interactive_waiters.fetch_add(1, Ordering::AcqRel);
            }
            ResolvePriority::Background => {
                state.background_waiters.fetch_add(1, Ordering::AcqRel);
            }
        }
        state.priority_changed.notify_waiters();
        Self { state, priority }
    }
}

impl Drop for PriorityWaiter<'_> {
    fn drop(&mut self) {
        match self.priority {
            ResolvePriority::Interactive => {
                self.state
                    .interactive_waiters
                    .fetch_sub(1, Ordering::AcqRel);
            }
            ResolvePriority::Background => {
                self.state.background_waiters.fetch_sub(1, Ordering::AcqRel);
            }
        }
        self.state.priority_changed.notify_waiters();
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
        let task = tokio::spawn(async move {
            task_limiter
                .reserve(ResolvePriority::Interactive, &task_cancellation)
                .await
        });
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

        limiter
            .reserve(ResolvePriority::Interactive, &cancellation)
            .await
            .unwrap();
        let started = Instant::now();
        limiter
            .reserve(ResolvePriority::Interactive, &cancellation)
            .await
            .unwrap();

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
            async move {
                limiter
                    .reserve(ResolvePriority::Interactive, &cancellation)
                    .await
            }
        });
        tokio::task::yield_now().await;
        queued_cancellation.cancel();
        assert_eq!(queued.await.unwrap(), Err(CANCELLED_MESSAGE.into()));
        drop(queue_guard);

        let next_cancellation = CancellationToken::new();
        assert!(
            limiter
                .reserve(ResolvePriority::Interactive, &next_cancellation)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn interactive_reservation_runs_before_a_waiting_background_request() {
        let limiter = ResolveLimiter::with_config(RESOLVE_WINDOW, RESOLVE_LIMIT, Duration::ZERO);
        let gate = limiter.state.reservation_queue.lock().await;
        let background = tokio::spawn({
            let limiter = limiter.clone();
            async move {
                let cancellation = CancellationToken::new();
                limiter
                    .reserve(ResolvePriority::Background, &cancellation)
                    .await
            }
        });
        tokio::task::yield_now().await;
        let interactive = tokio::spawn({
            let limiter = limiter.clone();
            async move {
                let cancellation = CancellationToken::new();
                limiter
                    .reserve(ResolvePriority::Interactive, &cancellation)
                    .await
            }
        });
        tokio::task::yield_now().await;
        drop(gate);

        interactive.await.unwrap().unwrap();
        background.await.unwrap().unwrap();
        assert_eq!(limiter.recorded_resolves(), 2);
    }

    #[tokio::test]
    async fn background_gets_a_turn_after_a_bounded_interactive_burst() {
        let limiter = ResolveLimiter::with_config(RESOLVE_WINDOW, 16, Duration::ZERO);
        let background_waiter = PriorityWaiter::new(&limiter.state, ResolvePriority::Background);
        for _ in 0..MAX_INTERACTIVE_BURST {
            limiter.note_priority_turn(ResolvePriority::Interactive);
        }

        assert!(!limiter.priority_can_run(ResolvePriority::Interactive));
        assert!(limiter.priority_can_run(ResolvePriority::Background));
        drop(background_waiter);
    }
}
