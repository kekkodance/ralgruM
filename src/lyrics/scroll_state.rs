use std::{cell::Cell, rc::Rc};

use gpui::{Pixels, Point, ScrollHandle, Size, point, px};
use gpui_component::scroll::ScrollbarHandle;

use super::core::LyricLine;

pub(crate) const LYRICS_OUTPUT_COMPENSATION_SECONDS: f64 = 0.12;

/// Returns whether a synced line has lyric text. Timed blanks are instrumental
/// gaps: they stay interactive and can be the active line, but they render the
/// playing visualizer instead of words.
pub(crate) fn is_highlightable_line(text: &str) -> bool {
    !text.trim().is_empty()
}

/// Selects the latest timed line at the current playback position. Blank
/// lines are retained as targets so the scroll view can center pauses too.
pub(crate) fn timed_line_index(lines: &[LyricLine], position: f64) -> Option<usize> {
    lines
        .iter()
        .rposition(|line| line.time <= position + LYRICS_OUTPUT_COMPENSATION_SECONDS)
}

/// Selects the line that should receive active visual styling, including a
/// timed blank so the gap visualizer can follow playback.
pub(crate) fn active_line_index(lines: &[LyricLine], position: f64) -> Option<usize> {
    timed_line_index(lines, position)
}

pub(crate) fn should_resume_auto_centering(timed_line: Option<usize>) -> bool {
    timed_line.is_some()
}

/// Computes the vertical GPUI scroll offset that centers a child in its
/// viewport. `child_top` is the unscrolled layout coordinate returned by
/// `ScrollHandle::bounds_for_item`; the current offset is applied by GPUI at
/// paint time and must not be added again here.
pub(crate) fn centered_scroll_offset(
    viewport_top: f32,
    viewport_height: f32,
    child_top: f32,
    child_height: f32,
    max_offset: f32,
) -> f32 {
    let viewport_height = viewport_height.max(0.);
    let child_height = child_height.max(0.);
    let max_scroll_top = max_offset.abs().max(0.);
    let target_scroll_top = child_top - viewport_top - ((viewport_height - child_height) / 2.);

    -target_scroll_top.clamp(0., max_scroll_top)
}

/// Freezes the offset reported to the hover scrollbar during auto-center so
/// programmatic line centering does not flash the thumb.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScrollOffsetFreeze {
    frozen: Rc<Cell<Option<(f32, f32)>>>,
}

impl ScrollOffsetFreeze {
    pub(crate) fn hide_for_changed_auto_scroll(&self, offset_changed: bool, x: f32, y: f32) {
        if offset_changed {
            self.freeze_at(x, y);
        }
    }

    pub(crate) fn reveal_for_user_scroll(&self) {
        self.frozen.set(None);
    }

    pub(crate) fn reset(&self) {
        self.frozen.set(None);
    }

    pub(crate) fn frozen_offset(&self) -> Option<(f32, f32)> {
        self.frozen.get()
    }

    fn freeze_at(&self, x: f32, y: f32) {
        if self.frozen.get().is_none() {
            self.frozen.set(Some((x, y)));
        }
    }
}

#[derive(Clone)]
pub(crate) struct LyricsScrollbarHandle {
    inner: ScrollHandle,
    freeze: ScrollOffsetFreeze,
}

impl LyricsScrollbarHandle {
    pub(crate) fn new(inner: ScrollHandle, freeze: ScrollOffsetFreeze) -> Self {
        Self { inner, freeze }
    }
}

impl ScrollbarHandle for LyricsScrollbarHandle {
    fn offset(&self) -> Point<Pixels> {
        match self.freeze.frozen_offset() {
            Some((x, y)) => point(px(x), px(y)),
            None => self.inner.offset(),
        }
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.freeze.reveal_for_user_scroll();
        self.inner.set_offset(offset);
    }

    fn content_size(&self) -> Size<Pixels> {
        (self.inner.max_offset() + self.inner.bounds().size.into()).into()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScrollSuppression {
    generation: u64,
    suppressed: bool,
}

impl ScrollSuppression {
    pub(crate) fn mark_user_scroll(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.suppressed = true;
        self.generation
    }

    pub(crate) fn release_if_current(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.suppressed = false;
        true
    }

    pub(crate) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.suppressed = false;
    }

    pub(crate) fn is_suppressed(self) -> bool {
        self.suppressed
    }

    pub(crate) fn generation(self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(time: f64, text: &str, index: usize) -> LyricLine {
        LyricLine {
            time,
            text: text.to_owned(),
            index,
        }
    }

    #[test]
    fn blank_lines_are_not_lyric_text_but_stay_active() {
        assert!(!is_highlightable_line("  \t"));
        assert!(!is_highlightable_line(""));
        assert!(is_highlightable_line("Verse"));

        let lines = vec![line(1.0, "First", 0), line(2.0, "\n", 1)];
        assert_eq!(timed_line_index(&lines, 2.0), Some(1));
        assert_eq!(active_line_index(&lines, 2.0), Some(1));
        assert_eq!(active_line_index(&lines, 1.8), Some(0));
    }

    #[test]
    fn active_index_transitions_across_timed_gaps() {
        let lines = vec![
            line(0.0, "Intro", 0),
            line(1.0, "", 1),
            line(2.0, "Verse", 2),
        ];

        assert_eq!(active_line_index(&lines, -0.2), None);
        assert_eq!(active_line_index(&lines, 0.8), Some(0));
        assert_eq!(timed_line_index(&lines, 1.0), Some(1));
        assert_eq!(active_line_index(&lines, 1.0), Some(1));
        assert_eq!(timed_line_index(&lines, 1.8), Some(1));
        assert_eq!(active_line_index(&lines, 1.8), Some(1));
        assert_eq!(active_line_index(&lines, 2.0), Some(2));
    }

    #[test]
    fn timed_index_moves_backward_after_a_seek() {
        let lines = vec![
            line(1.0, "First", 0),
            line(2.0, "Second", 1),
            line(3.0, "Third", 2),
        ];

        assert_eq!(timed_line_index(&lines, 3.1), Some(2));
        assert_eq!(timed_line_index(&lines, 2.1), Some(1));
        assert_eq!(timed_line_index(&lines, 1.1), Some(0));
        assert_eq!(timed_line_index(&lines, 0.1), None);
    }

    #[test]
    fn scroll_release_resumes_when_a_timed_line_is_available() {
        assert!(should_resume_auto_centering(Some(3)));
        assert!(!should_resume_auto_centering(None));
    }

    #[test]
    fn centered_offset_uses_positive_gpui_overflow_and_can_be_negative() {
        assert_eq!(centered_scroll_offset(10., 100., 90., 20., 200.), -40.);
        assert_eq!(centered_scroll_offset(10., 100., 0., 20., 200.), 0.);
        assert_eq!(centered_scroll_offset(10., 100., 900., 20., 200.), -200.);
    }

    #[test]
    fn scrollbar_freeze_hides_changed_auto_scroll_until_user_scroll() {
        let freeze = ScrollOffsetFreeze::default();
        assert_eq!(freeze.frozen_offset(), None);

        freeze.hide_for_changed_auto_scroll(false, 0., -40.);
        assert_eq!(freeze.frozen_offset(), None);

        freeze.hide_for_changed_auto_scroll(true, 0., -40.);
        assert_eq!(freeze.frozen_offset(), Some((0., -40.)));

        freeze.hide_for_changed_auto_scroll(true, 0., -80.);
        assert_eq!(freeze.frozen_offset(), Some((0., -40.)));

        freeze.reveal_for_user_scroll();
        assert_eq!(freeze.frozen_offset(), None);

        freeze.hide_for_changed_auto_scroll(true, 0., -80.);
        freeze.reset();
        assert_eq!(freeze.frozen_offset(), None);
    }

    #[test]
    fn lyrics_scrollbar_handle_reports_frozen_offset_until_user_scroll() {
        let freeze = ScrollOffsetFreeze::default();
        let inner = ScrollHandle::new();
        inner.set_offset(point(px(0.), px(-10.)));
        let handle = LyricsScrollbarHandle::new(inner.clone(), freeze.clone());

        freeze.hide_for_changed_auto_scroll(true, 0., -10.);
        inner.set_offset(point(px(0.), px(-80.)));
        assert_eq!(handle.offset(), point(px(0.), px(-10.)));

        freeze.reveal_for_user_scroll();
        assert_eq!(handle.offset(), point(px(0.), px(-80.)));

        freeze.hide_for_changed_auto_scroll(true, 0., -80.);
        handle.set_offset(point(px(0.), px(-20.)));
        assert_eq!(handle.offset(), point(px(0.), px(-20.)));
        assert_eq!(freeze.frozen_offset(), None);
    }

    #[test]
    fn stale_scroll_timer_does_not_resume_auto_centering() {
        let mut state = ScrollSuppression::default();
        let first = state.mark_user_scroll();
        let second = state.mark_user_scroll();

        assert!(state.is_suppressed());
        assert!(!state.release_if_current(first));
        assert!(state.is_suppressed());
        assert!(state.release_if_current(second));
        assert!(!state.is_suppressed());
    }

    #[test]
    fn reset_clears_suppression_and_invalidates_release_ticket() {
        let mut state = ScrollSuppression::default();
        let ticket = state.mark_user_scroll();
        let generation_before_reset = state.generation();

        state.reset();

        assert!(!state.is_suppressed());
        assert!(state.generation() > generation_before_reset);
        assert!(!state.release_if_current(ticket));
    }
}
