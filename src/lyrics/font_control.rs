use gpui::{
    AnyElement, App, Entity, FontWeight, IntoElement, MouseButton, Window, div, prelude::*, px,
    relative, rgb,
};
use gpui_component::slider::{Slider, SliderState};

use crate::{
    assets::LocalIcon,
    context_menu::{CONTEXT_MENU_ACTION_HORIZONTAL_PADDING, ContextMenuExt, PopupMenuItem},
    theme::{MUTED, PRIMARY, SCROLLBAR_THUMB},
};

use super::{LYRIC_FONT_SCALE_MAX, LYRIC_FONT_SCALE_MIN};

const FONT_MENU_WIDTH_PX: f32 = 208.;
const FONT_SLIDER_CONTROL_HEIGHT_PX: f32 = 24.;
const FONT_SLIDER_TRACK_HEIGHT_PX: f32 = 4.;
const FONT_SLIDER_THUMB_DIAMETER_PX: f32 = 13.;
const FONT_MENU_LABEL_SIZE_PX: f32 = 12.5;
const FONT_MENU_VALUE_SIZE_PX: f32 = 12.;

/// Detached-window text size control. The trigger opens the app-owned popup
/// menu below the header button, reusing the same surface and chevron as the
/// other anchored dropdowns. The slider mirrors the player bar volume track
/// and drives the shared [`SliderState`] owned by the lyrics panel.
pub(super) fn popout_font_size_control(slider: Entity<SliderState>) -> AnyElement {
    let menu_slider = slider.clone();
    crate::music_ui::ghost_icon_trigger_element(
        "lyrics-font-size-trigger",
        LocalIcon::TextHeight,
        crate::i18n::text("Text size"),
        12.,
        0.,
    )
    .context_menu(move |menu, _window, cx| {
        let slider_for_row = menu_slider.clone();
        cx.observe(&menu_slider, |_, _, cx| cx.notify()).detach();
        menu.min_w(px(FONT_MENU_WIDTH_PX))
            .max_w(px(FONT_MENU_WIDTH_PX))
            .item(
                PopupMenuItem::element(move |window, cx| {
                    font_size_row(&slider_for_row, window, cx)
                })
                // The slider supplies its own pointer handling. Keep the row
                // out of menu selection so hover paints do not sit behind it
                // and clicking the control cannot dismiss the menu.
                .disabled(true)
                .disabled_visual(false),
            )
    })
    .open_on(MouseButton::Left)
    .place_below()
    .into_any_element()
}

fn font_size_row(slider: &Entity<SliderState>, _window: &mut Window, cx: &mut App) -> AnyElement {
    let percent = font_scale_value(slider, cx);
    let fraction = font_scale_fraction(percent);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(9.))
        .px(px(CONTEXT_MENU_ACTION_HORIZONTAL_PADDING))
        .py(px(1.))
        .child(
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(FONT_MENU_LABEL_SIZE_PX))
                        .font_weight(FontWeight(450.))
                        .child(crate::i18n::text("Text size")),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(FONT_MENU_VALUE_SIZE_PX))
                        .text_color(rgb(MUTED))
                        .child(format_font_scale(percent)),
                ),
        )
        .child(font_size_slider(slider, fraction))
        .into_any_element()
}

fn font_size_slider(slider: &Entity<SliderState>, fraction: f32) -> AnyElement {
    let inset = px(FONT_SLIDER_THUMB_DIAMETER_PX * 0.5);
    div()
        .relative()
        .w_full()
        .h(px(FONT_SLIDER_CONTROL_HEIGHT_PX))
        .cursor_pointer()
        .child(font_size_track(fraction, inset))
        // Invisible gpui-component slider owns clicks and drags. It is inset
        // by the thumb radius so pointer travel matches the painted track.
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(inset)
                .right(inset)
                .overflow_hidden()
                .child(Slider::new(slider).horizontal().opacity(0.)),
        )
        .into_any_element()
}

fn font_size_track(fraction: f32, inset: gpui::Pixels) -> AnyElement {
    let fraction = fraction.clamp(0., 1.);
    let fill = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(relative(fraction))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    let thumb = div()
        .absolute()
        .left(relative(fraction))
        .top(px((FONT_SLIDER_TRACK_HEIGHT_PX
            - FONT_SLIDER_THUMB_DIAMETER_PX)
            * 0.5))
        .ml(px(-FONT_SLIDER_THUMB_DIAMETER_PX * 0.5))
        .size(px(FONT_SLIDER_THUMB_DIAMETER_PX))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    div()
        .absolute()
        .top(px((FONT_SLIDER_CONTROL_HEIGHT_PX
            - FONT_SLIDER_TRACK_HEIGHT_PX)
            * 0.5))
        .left(inset)
        .right(inset)
        .h(px(FONT_SLIDER_TRACK_HEIGHT_PX))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .cursor_pointer()
        .child(fill)
        .child(thumb)
        .into_any_element()
}

fn font_scale_value(slider: &Entity<SliderState>, cx: &App) -> f32 {
    slider.read(cx).value().end()
}

fn font_scale_fraction(percent: f32) -> f32 {
    let range = LYRIC_FONT_SCALE_MAX - LYRIC_FONT_SCALE_MIN;
    if range <= 0. {
        return 0.;
    }
    ((percent - LYRIC_FONT_SCALE_MIN) / range).clamp(0., 1.)
}

fn format_font_scale(percent: f32) -> String {
    format!("{percent:.0}%")
}

#[cfg(test)]
mod tests {
    use super::{font_scale_fraction, format_font_scale};
    use crate::lyrics::{LYRIC_FONT_SCALE_MAX, LYRIC_FONT_SCALE_MIN, lyric_font_size_for_scale};

    #[test]
    fn slider_fraction_spans_the_font_scale_range() {
        assert_eq!(font_scale_fraction(LYRIC_FONT_SCALE_MIN), 0.);
        assert_eq!(font_scale_fraction(LYRIC_FONT_SCALE_MAX), 1.);
        assert_eq!(
            font_scale_fraction((LYRIC_FONT_SCALE_MIN + LYRIC_FONT_SCALE_MAX) * 0.5),
            0.5
        );
        assert_eq!(font_scale_fraction(LYRIC_FONT_SCALE_MIN - 10.), 0.);
        assert_eq!(font_scale_fraction(LYRIC_FONT_SCALE_MAX + 10.), 1.);
    }

    #[test]
    fn value_label_is_a_stable_percentage() {
        assert_eq!(format_font_scale(100.), "100%");
        assert_eq!(format_font_scale(150.), "150%");
        assert_eq!(format_font_scale(200.), "200%");
    }

    #[test]
    fn scale_maps_from_default_to_double_size() {
        assert_eq!(lyric_font_size_for_scale(100.), 15.);
        assert_eq!(lyric_font_size_for_scale(150.), 22.5);
        assert_eq!(lyric_font_size_for_scale(200.), 30.);
    }
}
