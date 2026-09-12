use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{ScrollHandle, Window, ease_in_out, point, px};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MotionKind {
    Noop,
    Snap,
    Animate,
}

#[derive(Clone, Default)]
pub(crate) struct ScrollMotion {
    epoch: Rc<Cell<u64>>,
}

impl ScrollMotion {
    pub(crate) fn cancel(&self) {
        self.next_epoch();
    }

    pub(crate) fn animate(
        &self,
        scroll: &ScrollHandle,
        from: f32,
        target: f32,
        window: &Window,
        reduced_motion: bool,
    ) {
        let token = self.next_epoch();
        match motion_kind(
            from,
            target,
            reduced_motion,
            crate::motion::CONTENT_DURATION,
        ) {
            MotionKind::Noop => {}
            MotionKind::Snap => {
                let offset = scroll.offset();
                scroll.set_offset(point(offset.x, px(target)));
            }
            MotionKind::Animate => schedule(
                scroll.clone(),
                self.epoch.clone(),
                token,
                from,
                target,
                Instant::now(),
                window,
            ),
        }
    }

    fn next_epoch(&self) -> u64 {
        let token = self.epoch.get().wrapping_add(1);
        self.epoch.set(token);
        token
    }
}

fn schedule(
    scroll: ScrollHandle,
    epoch: Rc<Cell<u64>>,
    token: u64,
    from: f32,
    target: f32,
    started_at: Instant,
    window: &Window,
) {
    window.on_next_frame(move |window, _cx| {
        if !is_current(&epoch, token) {
            return;
        }

        let progress = animation_progress(crate::motion::CONTENT_DURATION, started_at.elapsed());
        let offset = interpolated_offset(from, target, progress);
        let current = scroll.offset();
        scroll.set_offset(point(current.x, px(offset)));
        window.refresh();

        if progress < 1. {
            schedule(scroll, epoch, token, from, target, started_at, window);
        }
    });
}

fn motion_kind(from: f32, target: f32, reduced_motion: bool, duration: Duration) -> MotionKind {
    if from == target {
        MotionKind::Noop
    } else if reduced_motion || duration.is_zero() {
        MotionKind::Snap
    } else {
        MotionKind::Animate
    }
}

fn is_current(epoch: &Cell<u64>, token: u64) -> bool {
    epoch.get() == token
}

fn animation_progress(duration: Duration, elapsed: Duration) -> f32 {
    if duration.is_zero() {
        return 1.;
    }
    (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
}

fn interpolated_offset(from: f32, target: f32, progress: f32) -> f32 {
    crate::motion::lerp(from, target, ease_in_out(progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_and_backward_interpolation_follow_ease_in_out() {
        let forward = interpolated_offset(0., -100., 0.5);
        let backward = interpolated_offset(-100., 0., 0.5);

        assert!((forward + 50.).abs() < 0.0001);
        assert!((backward + 50.).abs() < 0.0001);
        assert_eq!(interpolated_offset(10., -20., 0.), 10.);
        assert_eq!(interpolated_offset(10., -20., 1.), -20.);
    }

    #[test]
    fn retarget_starts_from_the_live_mid_animation_offset() {
        let live_offset = -37.;

        assert_eq!(interpolated_offset(live_offset, 80., 0.), live_offset);
        assert_eq!(interpolated_offset(live_offset, 80., 1.), 80.);
    }

    #[test]
    fn cancellation_invalidates_the_current_epoch() {
        let motion = ScrollMotion::default();
        let first = motion.next_epoch();

        assert!(is_current(&motion.epoch, first));
        motion.cancel();
        assert!(!is_current(&motion.epoch, first));
    }

    #[test]
    fn reduced_motion_and_equal_target_do_not_animate() {
        assert_eq!(
            motion_kind(0., 0., false, Duration::from_millis(180)),
            MotionKind::Noop
        );
        assert_eq!(
            motion_kind(0., -10., true, Duration::from_millis(180)),
            MotionKind::Snap
        );
        assert_eq!(
            motion_kind(0., -10., false, Duration::ZERO),
            MotionKind::Snap
        );
    }

    #[test]
    fn progress_clamps_to_the_animation_bounds() {
        let duration = Duration::from_millis(180);

        assert_eq!(animation_progress(duration, Duration::ZERO), 0.);
        assert_eq!(animation_progress(duration, duration), 1.);
        assert_eq!(animation_progress(duration, duration + duration), 1.);
    }
}
