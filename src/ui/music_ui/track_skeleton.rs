use gpui::{AnyElement, div, prelude::*, px, rgb, rgba};

use crate::{
    library::virtualization, playing_indicator::INDEX_COLUMN_WIDTH, theme::SURFACE_RAISED,
};

use super::{
    TRACK_PROVIDER_COMPACT_COLUMN_WIDTH, TRACK_PROVIDER_DURATION_GAP_PX, TRACK_TITLE_ARTIST_GAP_PX,
    track_provider_column_width,
};

pub(crate) const TRACK_ROW_CHILD_GAP_PX: f32 = 12.;
pub(crate) const TRACK_ROW_PADDING_X_PX: f32 = 10.;
pub(crate) const TRACK_ROW_PADDING_Y_PX: f32 = 6.;
pub(crate) const TRACK_ARTWORK_SIZE_PX: f32 = 40.;
pub(crate) const TRACK_DURATION_WIDTH_PX: f32 = 44.;
pub(crate) const TRACK_ACTION_SIZE_PX: f32 = 25.;
pub(crate) const TRACK_ACTION_GAP_PX: f32 = 4.;
pub(crate) const TRACK_ROW_BORDER_WIDTH_PX: f32 = 2.;
const TRACK_TITLE_LINE_HEIGHT_PX: f32 = 16.;
const TRACK_ARTIST_LINE_HEIGHT_PX: f32 = 14.;
const TRACK_TITLE_SKELETON_HEIGHT_PX: f32 = 11.;
const TRACK_ARTIST_SKELETON_HEIGHT_PX: f32 = 9.;
const TRACK_PROVIDER_ICON_PLACEHOLDER_WIDTH_PX: f32 = 11.;
const TRACK_PROVIDER_LABEL_PLACEHOLDER_WIDTH_PX: f32 = 64.;
const TRACK_DURATION_PLACEHOLDER_WIDTH_PX: f32 = 24.;
const TRACK_DURATION_PLACEHOLDER_HEIGHT_PX: f32 = 9.;
const STANDARD_TRACK_ACTION_COUNT: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrackSkeletonContext {
    NoProvider,
    SearchRoot { provider_compact: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackSkeletonLayout {
    pub(crate) show_index: bool,
    pub(crate) provider_width: f32,
    pub(crate) duration_width: f32,
    pub(crate) action_count: usize,
}

pub(crate) fn track_skeleton_layout(
    _width: f32,
    narrow: bool,
    context: TrackSkeletonContext,
) -> TrackSkeletonLayout {
    let provider_width = match context {
        TrackSkeletonContext::NoProvider => TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
        TrackSkeletonContext::SearchRoot { provider_compact } => {
            track_provider_column_width(provider_compact)
        }
    };
    TrackSkeletonLayout {
        show_index: !narrow,
        provider_width,
        duration_width: TRACK_DURATION_WIDTH_PX,
        action_count: STANDARD_TRACK_ACTION_COUNT,
    }
}

pub(crate) fn track_skeleton_fixed_width(layout: TrackSkeletonLayout) -> f32 {
    let child_count = 4 + usize::from(layout.show_index);
    let gaps = child_count.saturating_sub(1) as f32 * TRACK_ROW_CHILD_GAP_PX;
    TRACK_ROW_BORDER_WIDTH_PX
        + TRACK_ROW_PADDING_X_PX * 2.
        + if layout.show_index {
            INDEX_COLUMN_WIDTH
        } else {
            0.
        }
        + TRACK_ARTWORK_SIZE_PX
        + layout.provider_width
        + TRACK_ACTION_SIZE_PX * layout.action_count as f32
        + TRACK_ACTION_GAP_PX * layout.action_count.saturating_sub(1) as f32
        + gaps
}

pub(crate) fn track_skeleton_text_width(
    width: f32,
    narrow: bool,
    context: TrackSkeletonContext,
) -> f32 {
    (width.max(0.) - track_skeleton_fixed_width(track_skeleton_layout(width, narrow, context)))
        .max(0.)
}

pub(crate) fn track_skeleton_count(available_height: f32, reserved_height: f32) -> usize {
    let row_pitch = f32::from(virtualization::row_height());
    let track_space = (available_height - reserved_height).max(0.);
    ((track_space.max(row_pitch) / row_pitch).ceil() as usize).saturating_add(1)
}

pub(crate) fn track_skeleton_count_with_minimum(
    available_height: f32,
    reserved_height: f32,
    minimum: usize,
) -> usize {
    let row_pitch = f32::from(virtualization::row_height());
    let row_gap = f32::from(virtualization::row_gap());
    let track_space = (available_height - reserved_height).max(0.);
    let count = if track_space == 0. {
        0
    } else {
        ((track_space + row_gap) / row_pitch).ceil() as usize
    };
    count.max(minimum)
}

pub(crate) fn track_skeleton_list(
    count: usize,
    available_width: f32,
    narrow: bool,
    context: TrackSkeletonContext,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(f32::from(virtualization::row_gap())))
        .children((0..count).map(|_| track_skeleton_row(available_width, narrow, context)))
        .into_any_element()
}

fn block(width: gpui::Pixels, height: gpui::Pixels) -> impl IntoElement {
    div()
        .w(width)
        .h(height)
        .rounded(px(4.))
        .bg(rgb(SURFACE_RAISED))
}

fn track_skeleton_row(
    available_width: f32,
    narrow: bool,
    context: TrackSkeletonContext,
) -> AnyElement {
    let layout = track_skeleton_layout(available_width, narrow, context);
    let text_width = track_skeleton_text_width(available_width, narrow, context);
    let title_width = text_width.min(280.);
    let subtitle_width = text_width.min(170.);
    div()
        .min_h(virtualization::row_content_height())
        .w_full()
        .flex()
        .items_center()
        .gap(px(TRACK_ROW_CHILD_GAP_PX))
        .px(px(TRACK_ROW_PADDING_X_PX))
        .py(px(TRACK_ROW_PADDING_Y_PX))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .when(layout.show_index, |this| this.child(index_placeholder()))
        .child(
            div()
                .size(px(TRACK_ARTWORK_SIZE_PX))
                .flex_none()
                .rounded(px(6.))
                .bg(rgb(SURFACE_RAISED)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(TRACK_TITLE_ARTIST_GAP_PX))
                .child(text_line_placeholder(
                    title_width,
                    TRACK_TITLE_LINE_HEIGHT_PX,
                    TRACK_TITLE_SKELETON_HEIGHT_PX,
                ))
                .child(text_line_placeholder(
                    subtitle_width,
                    TRACK_ARTIST_LINE_HEIGHT_PX,
                    TRACK_ARTIST_SKELETON_HEIGHT_PX,
                )),
        )
        .child(provider_column(layout, context))
        .child(actions(layout.action_count))
        .into_any_element()
}

fn text_line_placeholder(width: f32, line_height: f32, block_height: f32) -> AnyElement {
    div()
        .h(px(line_height))
        .flex_none()
        .flex()
        .items_center()
        .child(block(px(width), px(block_height)))
        .into_any_element()
}

fn index_placeholder() -> AnyElement {
    div()
        .w(px(INDEX_COLUMN_WIDTH))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(block(px(12.), px(10.)))
        .into_any_element()
}

fn provider_column(layout: TrackSkeletonLayout, context: TrackSkeletonContext) -> AnyElement {
    let provider_placeholder_width = match context {
        TrackSkeletonContext::NoProvider => 0.,
        TrackSkeletonContext::SearchRoot { provider_compact } => {
            if provider_compact {
                TRACK_PROVIDER_ICON_PLACEHOLDER_WIDTH_PX
            } else {
                TRACK_PROVIDER_LABEL_PLACEHOLDER_WIDTH_PX
            }
        }
    };
    div()
        .w(px(layout.provider_width))
        .h(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(TRACK_PROVIDER_DURATION_GAP_PX))
        .when(provider_placeholder_width > 0., |this| {
            this.child(block(px(provider_placeholder_width), px(11.)))
        })
        .child(
            div()
                .w(px(layout.duration_width))
                .h_full()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .child(block(
                    px(TRACK_DURATION_PLACEHOLDER_WIDTH_PX),
                    px(TRACK_DURATION_PLACEHOLDER_HEIGHT_PX),
                )),
        )
        .into_any_element()
}

fn actions(count: usize) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .gap(px(TRACK_ACTION_GAP_PX))
        .children((0..count).map(|_| {
            div()
                .size(px(TRACK_ACTION_SIZE_PX))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(20.)).rounded(px(10.)).bg(rgb(SURFACE_RAISED)))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music_ui::TRACK_PROVIDER_FULL_COLUMN_WIDTH;

    #[test]
    fn row_layout_matches_stable_track_geometry() {
        let desktop = track_skeleton_layout(1300., false, TrackSkeletonContext::NoProvider);
        assert!(desktop.show_index);
        assert_eq!(desktop.provider_width, TRACK_PROVIDER_COMPACT_COLUMN_WIDTH);
        assert_eq!(desktop.duration_width, TRACK_DURATION_WIDTH_PX);
        assert_eq!(desktop.action_count, 2);
        assert_eq!(track_skeleton_fixed_width(desktop), 268.);

        let narrow = track_skeleton_layout(768., true, TrackSkeletonContext::NoProvider);
        assert!(!narrow.show_index);
        assert_eq!(track_skeleton_fixed_width(narrow), 228.);
    }

    #[test]
    fn search_root_uses_the_real_provider_column_mode() {
        let compact = track_skeleton_layout(
            500.,
            false,
            TrackSkeletonContext::SearchRoot {
                provider_compact: true,
            },
        );
        let full = track_skeleton_layout(
            600.,
            false,
            TrackSkeletonContext::SearchRoot {
                provider_compact: false,
            },
        );
        assert_eq!(compact.provider_width, TRACK_PROVIDER_COMPACT_COLUMN_WIDTH);
        assert_eq!(full.provider_width, TRACK_PROVIDER_FULL_COLUMN_WIDTH);
        assert!(track_skeleton_fixed_width(full) > track_skeleton_fixed_width(compact));
    }

    #[test]
    fn shared_counts_use_the_virtualized_row_pitch_and_heading_space() {
        assert_eq!(track_skeleton_count(100., 0.), 3);
        assert_eq!(track_skeleton_count(580., 72.), 10);
        assert_eq!(track_skeleton_count_with_minimum(760., 88., 8), 12);
        assert_eq!(track_skeleton_count_with_minimum(100., 88., 8), 8);
    }

    #[test]
    fn implementation_keeps_the_index_placeholder_inside_the_fixed_slot() {
        let source = include_str!("track_skeleton.rs");
        assert!(source.contains(".w(px(INDEX_COLUMN_WIDTH))"));
        assert!(source.contains(".child(block(px(12.), px(10.)))"));
        assert!(source.contains(".border_1()"));
        assert!(source.contains("TRACK_TITLE_ARTIST_GAP_PX"));
        assert!(source.contains("TRACK_PROVIDER_DURATION_GAP_PX"));
        assert!(source.contains("TRACK_ACTION_GAP_PX"));
        assert!(source.contains("TRACK_TITLE_LINE_HEIGHT_PX"));
        assert!(source.contains("TRACK_TITLE_SKELETON_HEIGHT_PX"));
        assert_eq!(TRACK_DURATION_PLACEHOLDER_WIDTH_PX, 24.);
        assert!(source.contains(".justify_end()"));
        assert!(source.contains(".size(px(20.))"));
    }
}
