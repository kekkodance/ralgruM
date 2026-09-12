use std::{cell::Cell, rc::Rc, time::Instant};

use gpui::{ScrollHandle, Window, point, px};

use crate::motion::CONTENT_DURATION;

const NEAR_EQUAL_OFFSET: f32 = 0.5;

pub(super) fn start(
    scroll_handle: ScrollHandle,
    snap_epoch: Rc<Cell<u64>>,
    pending_target: Rc<Cell<Option<f32>>>,
    target: f32,
    reduced_motion: bool,
    window: &mut Window,
) {
    let token = increment_epoch(&snap_epoch);
    let offset = scroll_handle.offset();
    let current = f32::from(offset.x);

    if reduced_motion || CONTENT_DURATION.is_zero() || (target - current).abs() <= NEAR_EQUAL_OFFSET
    {
        scroll_handle.set_offset(point(px(target), offset.y));
        pending_target.set(None);
        window.refresh();
        return;
    }

    schedule(
        scroll_handle,
        snap_epoch,
        token,
        current,
        target,
        Instant::now(),
        pending_target,
        window,
    );
}

pub(super) fn cancel(snap_epoch: &Cell<u64>, pending_target: &Cell<Option<f32>>) {
    pending_target.set(None);
    increment_epoch(snap_epoch);
}

pub(super) fn increment_epoch(snap_epoch: &Cell<u64>) -> u64 {
    let token = snap_epoch.get().wrapping_add(1);
    snap_epoch.set(token);
    token
}

fn token_is_current(snap_epoch: &Cell<u64>, token: u64) -> bool {
    snap_epoch.get() == token
}

fn progress(start: Instant, now: Instant) -> f32 {
    if CONTENT_DURATION.is_zero() {
        return 1.;
    }
    (now.saturating_duration_since(start).as_secs_f32() / CONTENT_DURATION.as_secs_f32())
        .clamp(0., 1.)
}

fn interpolate(start: f32, target: f32, progress: f32) -> f32 {
    crate::motion::lerp(start, target, gpui::ease_in_out(progress))
}

fn schedule(
    scroll_handle: ScrollHandle,
    snap_epoch: Rc<Cell<u64>>,
    token: u64,
    current: f32,
    target: f32,
    start: Instant,
    pending_target: Rc<Cell<Option<f32>>>,
    window: &Window,
) {
    window.on_next_frame(move |window, _cx| {
        if !token_is_current(&snap_epoch, token) {
            return;
        }

        let progress = progress(start, Instant::now());
        let x = interpolate(current, target, progress);
        let y = scroll_handle.offset().y;
        scroll_handle.set_offset(point(px(x), y));
        window.refresh();

        if progress < 1. {
            schedule(
                scroll_handle,
                snap_epoch,
                token,
                current,
                target,
                start,
                pending_target,
                window,
            );
        } else {
            clear_pending_if_current(&snap_epoch, token, &pending_target);
        }
    });
}

fn clear_pending_if_current(
    snap_epoch: &Cell<u64>,
    token: u64,
    pending_target: &Cell<Option<f32>>,
) -> bool {
    if !token_is_current(snap_epoch, token) {
        return false;
    }
    pending_target.set(None);
    true
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        cancel, clear_pending_if_current, increment_epoch, interpolate, progress, token_is_current,
    };

    #[test]
    fn progress_has_stable_endpoints() {
        let start = Instant::now();

        assert_eq!(progress(start, start), 0.);
        assert_eq!(progress(start, start + super::CONTENT_DURATION), 1.);
        assert_eq!(
            progress(
                start,
                start + super::CONTENT_DURATION + Duration::from_millis(1)
            ),
            1.
        );
    }

    #[test]
    fn interpolation_eases_between_offsets_and_keeps_endpoints() {
        assert_eq!(interpolate(-100., -300., 0.), -100.);
        assert!((interpolate(-100., -300., 0.5) + 200.).abs() < 0.0001);
        assert_eq!(interpolate(-100., -300., 1.), -300.);
    }

    #[test]
    fn newer_epoch_tokens_cancel_older_callbacks() {
        let epoch = std::cell::Cell::new(0);
        let first = increment_epoch(&epoch);
        let second = increment_epoch(&epoch);

        assert!(!token_is_current(&epoch, first));
        assert!(token_is_current(&epoch, second));
    }

    #[test]
    fn stale_completion_preserves_a_newer_pending_target() {
        let epoch = std::cell::Cell::new(0);
        let pending = std::cell::Cell::new(Some(-162.));
        let stale = increment_epoch(&epoch);

        pending.set(Some(-324.));
        increment_epoch(&epoch);

        assert!(!clear_pending_if_current(&epoch, stale, &pending));
        assert_eq!(pending.get(), Some(-324.));
    }

    #[test]
    fn current_completion_clears_pending_target() {
        let epoch = std::cell::Cell::new(0);
        let pending = std::cell::Cell::new(Some(-324.));
        let token = increment_epoch(&epoch);

        assert!(clear_pending_if_current(&epoch, token, &pending));
        assert_eq!(pending.get(), None);
    }

    #[test]
    fn manual_cancel_clears_pending_and_invalidates_motion() {
        let epoch = std::cell::Cell::new(0);
        let pending = std::cell::Cell::new(Some(-162.));
        let token = increment_epoch(&epoch);

        cancel(&epoch, &pending);

        assert_eq!(pending.get(), None);
        assert!(!token_is_current(&epoch, token));
    }
}
