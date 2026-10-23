//! Shared single-line text measurement for layout decisions that must match
//! the rendered label width on every platform. Font metrics differ between
//! Windows and Linux, so callers cannot rely on a hardcoded pixel budget.

use gpui::{Font, FontWeight, Hsla, TextRun, Window, px};

use crate::theme::ui_font_family;

/// Returns the shaped width of `text` in pixels without creating a view.
pub(crate) fn text_width(window: &Window, text: &str, font_size: f32, weight: FontWeight) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = Font {
        family: ui_font_family().into(),
        weight,
        ..Font::default()
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.into(), px(font_size), &[run], None)
        .width
        .into()
}
