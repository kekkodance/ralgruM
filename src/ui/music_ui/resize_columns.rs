use std::time::Duration;

use gpui::{Context, Window};

use super::fitted_columns;

pub(crate) const COLUMN_RESIZE_DEBOUNCE: Duration = crate::motion::PANEL_SETTLING_DURATION;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResizeRequest {
    generation: u64,
    columns: u16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizeSettledColumns {
    last_width: Option<f32>,
    columns: u16,
    generation: u64,
    pending_columns: Option<u16>,
}

impl ResizeSettledColumns {
    pub(crate) const fn new() -> Self {
        Self {
            last_width: None,
            columns: 0,
            generation: 0,
            pending_columns: None,
        }
    }

    pub(crate) fn columns(&self) -> u16 {
        self.columns
    }

    /// Record a measured width and return a debounced commit request when the
    /// initial layout has already been established.
    pub(crate) fn observe_width(&mut self, width: f32) -> Option<ResizeRequest> {
        let width = normalize_width(width);
        if self.last_width == Some(width) {
            return None;
        }
        self.last_width = Some(width);

        let columns = fitted_columns(width);
        if self.columns == 0 {
            self.columns = columns;
            self.pending_columns = None;
            return None;
        }

        if columns == self.columns {
            if self.pending_columns.take().is_some() {
                self.generation = self.generation.wrapping_add(1);
            }
            return None;
        }

        self.generation = self.generation.wrapping_add(1);
        self.pending_columns = Some(columns);
        Some(ResizeRequest {
            generation: self.generation,
            columns,
        })
    }

    pub(crate) fn commit(&mut self, request: ResizeRequest) -> bool {
        if request.generation != self.generation
            || self.pending_columns != Some(request.columns)
            || self.columns == request.columns
        {
            return false;
        }
        self.columns = request.columns;
        self.pending_columns = None;
        true
    }
}

impl Default for ResizeSettledColumns {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait ResizeSettledTarget {
    fn commit_resize(&mut self, request: ResizeRequest) -> bool;
}

/// Schedule the latest resize request and ignore timers made stale by a later
/// width measurement. The target decides whether the committed columns changed.
pub(crate) fn schedule_resize<T: ResizeSettledTarget + 'static>(
    cx: &mut Context<T>,
    window: &Window,
    request: ResizeRequest,
) {
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(COLUMN_RESIZE_DEBOUNCE).await;
        this.update_in(cx, |this, _window, cx| {
            if this.commit_resize(request) {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

fn normalize_width(width: f32) -> f32 {
    if width.is_finite() { width.max(0.) } else { 0. }
}

#[cfg(test)]
mod tests {
    use super::{COLUMN_RESIZE_DEBOUNCE, ResizeSettledColumns, fitted_columns};

    #[test]
    fn initial_width_is_applied_without_a_timer() {
        let mut state = ResizeSettledColumns::new();

        assert_eq!(state.observe_width(380.), None);
        assert_eq!(state.columns(), fitted_columns(380.));
    }

    #[test]
    fn width_changes_within_a_column_band_do_not_schedule_rebuilds() {
        let mut state = ResizeSettledColumns::new();
        state.observe_width(380.);

        assert_eq!(state.observe_width(390.), None);

        assert_eq!(state.columns(), fitted_columns(380.));
    }

    #[test]
    fn stale_timer_requests_cannot_commit() {
        let mut state = ResizeSettledColumns::new();
        state.observe_width(380.);
        let stale = state.observe_width(700.).expect("first resize request");
        let latest = state.observe_width(900.).expect("second resize request");

        assert!(!state.commit(stale));
        assert!(state.commit(latest));
        assert_eq!(state.columns(), fitted_columns(900.));
    }

    #[test]
    fn returning_to_committed_band_cancels_pending_resize() {
        let mut state = ResizeSettledColumns::new();
        state.observe_width(900.);
        let stale = state.observe_width(700.).expect("resize request");

        assert_eq!(state.observe_width(900.), None);
        assert!(!state.commit(stale));
        assert_eq!(state.columns(), fitted_columns(900.));
    }

    #[test]
    fn invalid_widths_are_safe() {
        let mut state = ResizeSettledColumns::new();

        assert_eq!(state.observe_width(f32::NAN), None);
        assert_eq!(state.columns(), fitted_columns(0.));
        assert_eq!(state.observe_width(f32::INFINITY), None);
    }

    #[test]
    fn resize_debounce_covers_the_panel_transition() {
        assert!(COLUMN_RESIZE_DEBOUNCE > crate::motion::PANEL_DURATION);
        assert_eq!(
            COLUMN_RESIZE_DEBOUNCE,
            crate::motion::PANEL_DURATION + crate::motion::ANIMATION_FRAME_DURATION
        );
    }
}
