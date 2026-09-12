use gpui::{AnyElement, App, ListState, Pixels, Window, div, prelude::*, px};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

pub(crate) const TRACK_ROW_HEIGHT: f32 = 58.0;
pub(crate) const TRACK_ROW_CONTENT_HEIGHT: f32 = 54.0;
pub(crate) const TRACK_ROW_GAP: f32 = TRACK_ROW_HEIGHT - TRACK_ROW_CONTENT_HEIGHT;
pub(crate) const TRACK_LIST_OVERDRAW_ROWS: f32 = 12.0;
pub(crate) const TRACK_LIST_OVERDRAW: f32 = TRACK_ROW_HEIGHT * TRACK_LIST_OVERDRAW_ROWS;

pub(crate) const CARD_GRID_GAP: f32 = 12.0;
pub(crate) const CARD_GRID_NARROW_GAP: f32 = 9.0;
pub(crate) const CARD_GRID_ROW_HEIGHT_EXTRA: f32 =
    crate::collection_detail::DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX;
pub(crate) const CARD_GRID_OVERDRAW_ROWS: f32 = 2.0;

const LIBRARY_SCROLLBAR_OUTSET_DESKTOP: f32 = crate::music_ui::MAIN_CONTENT_INSET;
const LIBRARY_SCROLLBAR_OUTSET_NARROW: f32 = crate::music_ui::NARROW_MAIN_CONTENT_INSET;

pub(crate) type PageItemBuilder = Arc<dyn Fn(&mut Window, &mut App) -> AnyElement + 'static>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TrackListLayout {
    pub(crate) narrow: bool,
    pub(crate) provider_icon_only: bool,
    pub(crate) header: bool,
}

impl TrackListLayout {
    pub(crate) const fn new(narrow: bool, provider_icon_only: bool, header: bool) -> Self {
        Self {
            narrow,
            provider_icon_only,
            header,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CardGridLayout {
    pub(crate) columns: usize,
    pub(crate) available_width: f32,
    pub(crate) card_width: f32,
    pub(crate) row_gap: f32,
    pub(crate) row_height: Pixels,
    pub(crate) narrow: bool,
}

impl CardGridLayout {
    pub(crate) fn new(columns: u16, available_width: f32, narrow: bool) -> Self {
        Self::new_with_geometry(
            columns,
            available_width,
            narrow,
            card_grid_gap(narrow),
            CARD_GRID_ROW_HEIGHT_EXTRA,
            false,
        )
    }

    pub(crate) fn new_for_visual(
        columns: u16,
        available_width: f32,
        narrow: bool,
        shared_provider_artist_visual: bool,
    ) -> Self {
        if shared_provider_artist_visual {
            Self::new_with_geometry(
                columns,
                available_width,
                narrow,
                crate::collection_detail::DETAIL_CARD_GRID_GAP_PX,
                crate::collection_detail::DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX,
                true,
            )
        } else {
            Self::new(columns, available_width, narrow)
        }
    }

    fn new_with_geometry(
        columns: u16,
        available_width: f32,
        narrow: bool,
        row_gap: f32,
        row_height_extra: f32,
        settle_width: bool,
    ) -> Self {
        let columns = usize::from(columns.max(1));
        let available_width = if available_width.is_finite() {
            let available_width = available_width.max(0.);
            if settle_width {
                available_width.round()
            } else {
                available_width
            }
        } else {
            0.
        };
        let card_width = ((available_width - row_gap * columns.saturating_sub(1) as f32)
            / columns as f32)
            .max(0.);
        let row_height = if !settle_width && row_height_extra == CARD_GRID_ROW_HEIGHT_EXTRA {
            card_grid_row_height_hint(card_width, row_gap)
        } else {
            px(card_width + row_height_extra + row_gap)
        };
        Self {
            columns,
            available_width,
            card_width,
            row_gap,
            row_height,
            narrow,
        }
    }

    pub(crate) fn overdraw(self) -> Pixels {
        px(f32::from(self.row_height) * CARD_GRID_OVERDRAW_ROWS)
    }
}

pub(crate) fn card_grid_gap(narrow: bool) -> f32 {
    if narrow {
        CARD_GRID_NARROW_GAP
    } else {
        CARD_GRID_GAP
    }
}

pub(crate) fn card_grid_row_height_hint(card_width: f32, row_gap: f32) -> Pixels {
    px(card_width.max(0.) + CARD_GRID_ROW_HEIGHT_EXTRA + row_gap.max(0.))
}

pub(crate) fn card_grid_row_count(card_count: usize, columns: usize) -> usize {
    card_count.div_ceil(columns.max(1))
}

pub(crate) fn card_grid_row_range(
    card_count: usize,
    columns: usize,
    row_index: usize,
) -> Option<Range<usize>> {
    let columns = columns.max(1);
    let start = row_index.checked_mul(columns)?;
    (start < card_count).then_some(start..start.saturating_add(columns).min(card_count))
}

pub(crate) fn row_height() -> Pixels {
    gpui::px(TRACK_ROW_HEIGHT)
}

pub(crate) fn row_content_height() -> Pixels {
    gpui::px(TRACK_ROW_CONTENT_HEIGHT)
}

pub(crate) fn row_gap() -> Pixels {
    gpui::px(TRACK_ROW_GAP)
}

pub(crate) fn overdraw() -> Pixels {
    px(TRACK_LIST_OVERDRAW)
}

pub(crate) fn library_scrollbar_outset(narrow: bool) -> Pixels {
    px(if narrow {
        LIBRARY_SCROLLBAR_OUTSET_NARROW
    } else {
        LIBRARY_SCROLLBAR_OUTSET_DESKTOP
    })
}

pub(crate) fn library_vertical_scrollbar<H>(state: &H, narrow: bool) -> AnyElement
where
    H: gpui_component::scroll::ScrollbarHandle + Clone,
{
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right(px(-f32::from(library_scrollbar_outset(narrow))))
        .child(Scrollbar::vertical(state).scrollbar_show(ScrollbarShow::Hover))
        .into_any_element()
}

pub(crate) fn page_list(
    state: ListState,
    builders: Arc<Vec<PageItemBuilder>>,
    narrow: bool,
) -> AnyElement {
    let scrollbar_state = state.clone();
    let list = gpui::list(state, move |index, window, app| {
        builders[index](window, app)
    });
    gpui::div()
        .w_full()
        .flex_1()
        .min_h_0()
        .relative()
        .child(list.w_full().h_full().min_h_0())
        .child(library_vertical_scrollbar(&scrollbar_state, narrow))
        .into_any_element()
}

pub(crate) fn should_virtualize(preview_limit: Option<usize>, item_count: usize) -> bool {
    preview_limit.is_none() && item_count > 0
}

pub(crate) fn should_virtualize_card_grid(preview_limit: Option<usize>, card_count: usize) -> bool {
    preview_limit.is_none() && card_count > 0
}

pub(crate) fn displayed_track_count(item_count: usize, preview_limit: Option<usize>) -> usize {
    preview_limit.map_or(item_count, |limit| limit.min(item_count))
}

pub(crate) fn content_identity(
    route_key: &str,
    filter_key: &str,
    ordered_rows: impl IntoIterator<Item = String>,
) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    route_key.hash(&mut hasher);
    filter_key.hash(&mut hasher);
    for (index, row) in ordered_rows.into_iter().enumerate() {
        index.hash(&mut hasher);
        row.hash(&mut hasher);
    }
    format!("{route_key}:{filter_key}:{}", hasher.finish())
}

#[cfg(test)]
pub(crate) fn list_item_count(track_count: usize, has_header: bool) -> usize {
    track_count + usize::from(has_header)
}

#[cfg(test)]
pub(crate) fn track_index_for_item(item_index: usize, has_header: bool) -> Option<usize> {
    if has_header {
        item_index.checked_sub(1)
    } else {
        Some(item_index)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PageItem {
    Header(usize),
    Track(usize),
    CardRow {
        section_index: usize,
        row_index: usize,
    },
    Tail(usize),
}

pub(crate) fn page_item_uniform_height(
    page_items: &[PageItem],
    card_grid_layout: CardGridLayout,
) -> Pixels {
    if page_items.is_empty() {
        return row_height();
    }
    let total_height = page_items
        .iter()
        .map(|item| match item {
            PageItem::CardRow { .. } => card_grid_layout.row_height,
            PageItem::Track(_) => row_height(),
            PageItem::Header(_) | PageItem::Tail(_) => row_content_height(),
        })
        .map(f32::from)
        .sum::<f32>();
    px(total_height / page_items.len() as f32)
}

#[cfg(test)]
pub(crate) fn page_item_count(header_count: usize, track_count: usize, tail_count: usize) -> usize {
    header_count + track_count + tail_count
}

#[cfg(test)]
pub(crate) fn page_item_for(
    item_index: usize,
    header_count: usize,
    track_count: usize,
    tail_count: usize,
) -> Option<PageItem> {
    if item_index < header_count {
        return Some(PageItem::Header(item_index));
    }
    let track_index = item_index - header_count;
    if track_index < track_count {
        return Some(PageItem::Track(track_index));
    }
    let tail_index = track_index - track_count;
    (tail_index < tail_count).then_some(PageItem::Tail(tail_index))
}

#[cfg(test)]
pub(crate) fn page_item_count_with_card_rows(
    header_count: usize,
    track_count: usize,
    card_row_count: usize,
    tail_count: usize,
) -> usize {
    header_count + track_count + card_row_count + tail_count
}

#[cfg(test)]
pub(crate) fn page_item_for_with_card_rows(
    item_index: usize,
    header_count: usize,
    track_count: usize,
    card_row_count: usize,
    tail_count: usize,
    section_index: usize,
) -> Option<PageItem> {
    if item_index < header_count {
        return Some(PageItem::Header(item_index));
    }
    let track_index = item_index - header_count;
    if track_index < track_count {
        return Some(PageItem::Track(track_index));
    }
    let card_row_index = track_index - track_count;
    if card_row_index < card_row_count {
        return Some(PageItem::CardRow {
            section_index,
            row_index: card_row_index,
        });
    }
    let tail_index = card_row_index - card_row_count;
    (tail_index < tail_count).then_some(PageItem::Tail(tail_index))
}

pub(crate) fn list_state_needs_reset(
    previous: Option<(usize, TrackListLayout)>,
    count: usize,
    layout: TrackListLayout,
) -> bool {
    previous != Some((count, layout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unbounded_non_empty_tracklists_are_virtualized() {
        assert!(should_virtualize(None, 1));
        assert!(!should_virtualize(None, 0));
        assert!(!should_virtualize(Some(5), 204));
    }

    #[test]
    fn only_unbounded_non_empty_card_grids_are_virtualized() {
        assert!(should_virtualize_card_grid(None, 24));
        assert!(!should_virtualize_card_grid(None, 0));
        assert!(!should_virtualize_card_grid(Some(12), 204));
    }

    #[test]
    fn similar_artist_results_use_the_unbounded_card_grid_path() {
        assert!(should_virtualize_card_grid(None, 13));
        assert!(!should_virtualize_card_grid(Some(12), 13));
    }

    #[test]
    fn list_identity_shape_changes_require_a_reset() {
        let layout = TrackListLayout::new(false, false, false);
        assert!(list_state_needs_reset(None, 10, layout));
        assert!(!list_state_needs_reset(Some((10, layout)), 10, layout));
        assert!(list_state_needs_reset(Some((9, layout)), 10, layout));
        assert!(list_state_needs_reset(
            Some((10, layout)),
            10,
            TrackListLayout::new(true, false, false)
        ));
    }

    #[test]
    fn header_item_maps_to_no_track_and_preserves_indices() {
        assert_eq!(list_item_count(204, true), 205);
        assert_eq!(track_index_for_item(0, true), None);
        assert_eq!(track_index_for_item(1, true), Some(0));
        assert_eq!(track_index_for_item(204, true), Some(203));
        assert_eq!(track_index_for_item(203, false), Some(203));
    }

    #[test]
    fn page_items_keep_header_tracks_and_tail_indices_disjoint() {
        assert_eq!(page_item_count(2, 204, 3), 209);
        assert_eq!(page_item_for(0, 2, 204, 3), Some(PageItem::Header(0)));
        assert_eq!(page_item_for(1, 2, 204, 3), Some(PageItem::Header(1)));
        assert_eq!(page_item_for(2, 2, 204, 3), Some(PageItem::Track(0)));
        assert_eq!(page_item_for(205, 2, 204, 3), Some(PageItem::Track(203)));
        assert_eq!(page_item_for(206, 2, 204, 3), Some(PageItem::Tail(0)));
        assert_eq!(page_item_for(208, 2, 204, 3), Some(PageItem::Tail(2)));
        assert_eq!(page_item_for(209, 2, 204, 3), None);
    }

    #[test]
    fn row_height_includes_the_existing_row_gap() {
        assert_eq!(TRACK_ROW_HEIGHT, TRACK_ROW_CONTENT_HEIGHT + TRACK_ROW_GAP);
        assert_eq!(TRACK_ROW_GAP, 4.0);
        assert_eq!(row_height(), gpui::px(58.));
        assert_eq!(row_content_height(), gpui::px(54.));
        assert_eq!(row_gap(), gpui::px(4.));
    }

    #[test]
    fn card_grid_layout_uses_card_geometry_and_a_small_overscan() {
        let layout = CardGridLayout::new(4, 800., false);

        assert_eq!(layout.columns, 4);
        assert_eq!(layout.available_width, 800.);
        assert_eq!(layout.card_width, 191.);
        assert_eq!(layout.row_gap, CARD_GRID_GAP);
        assert_eq!(layout.row_height, px(249.));
        assert_eq!(layout.overdraw(), px(498.));
        assert!(layout.row_height > row_height());
    }

    #[test]
    fn provider_artist_grid_matches_search_geometry_on_desktop_and_narrow() {
        let expected_available_width = 700.;
        let expected_card_width = (expected_available_width - 12. * 2.) / 3.;
        for narrow in [false, true] {
            let layout = CardGridLayout::new_for_visual(3, 700.4, narrow, true);

            assert_eq!(layout.available_width, expected_available_width);
            assert_eq!(layout.card_width, expected_card_width);
            assert_eq!(
                layout.row_gap,
                crate::collection_detail::DETAIL_CARD_GRID_GAP_PX
            );
            assert_eq!(layout.row_height, px(expected_card_width + 58.));
            assert!(
                (f32::from(layout.row_height)
                    - layout.card_width
                    - layout.row_gap
                    - crate::collection_detail::DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX)
                    .abs()
                    < 0.001
            );
        }
    }

    #[test]
    fn native_library_narrow_grid_keeps_its_nine_pixel_gap() {
        let layout = CardGridLayout::new_for_visual(4, 800., true, false);

        assert_eq!(layout.row_gap, CARD_GRID_NARROW_GAP);
        assert_eq!(layout.card_width, (800. - 9. * 3.) / 4.);
        assert_eq!(
            layout.row_height,
            px(layout.card_width + CARD_GRID_ROW_HEIGHT_EXTRA + CARD_GRID_NARROW_GAP)
        );
    }

    #[test]
    fn native_library_grid_uses_shared_collection_card_height_extra() {
        assert_eq!(
            CARD_GRID_ROW_HEIGHT_EXTRA,
            crate::collection_detail::DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX
        );
    }

    #[test]
    fn card_grid_rows_count_and_slice_without_rendering_neighbor_rows() {
        assert_eq!(card_grid_row_count(10, 4), 3);
        assert_eq!(card_grid_row_range(10, 4, 0), Some(0..4));
        assert_eq!(card_grid_row_range(10, 4, 2), Some(8..10));
        assert_eq!(card_grid_row_range(10, 4, 3), None);
    }

    #[test]
    fn page_items_include_card_rows_without_colliding_with_track_or_tail_indices() {
        assert_eq!(page_item_count_with_card_rows(2, 4, 3, 1), 10);
        assert_eq!(
            page_item_for_with_card_rows(6, 2, 4, 3, 1, 7),
            Some(PageItem::CardRow {
                section_index: 7,
                row_index: 0
            })
        );
        assert_eq!(
            page_item_for_with_card_rows(8, 2, 4, 3, 1, 7),
            Some(PageItem::CardRow {
                section_index: 7,
                row_index: 2
            })
        );
        assert_eq!(
            page_item_for_with_card_rows(9, 2, 4, 3, 1, 7),
            Some(PageItem::Tail(0))
        );
    }

    #[test]
    fn mixed_page_uniform_height_weights_cards_tracks_and_non_card_items() {
        let layout = CardGridLayout::new(4, 800., false);
        let page_items = [
            PageItem::Header(0),
            PageItem::Track(0),
            PageItem::CardRow {
                section_index: 0,
                row_index: 0,
            },
            PageItem::Tail(0),
        ];

        assert_eq!(
            page_item_uniform_height(&page_items, layout),
            px((54. + 58. + f32::from(layout.row_height) + 54.) / 4.)
        );
    }

    #[test]
    fn uniform_height_preserves_pure_card_and_non_card_estimates() {
        let layout = CardGridLayout::new(4, 800., false);
        let card_items = [
            PageItem::CardRow {
                section_index: 0,
                row_index: 0,
            },
            PageItem::CardRow {
                section_index: 0,
                row_index: 1,
            },
        ];
        let non_card_items = [PageItem::Header(0), PageItem::Track(0), PageItem::Tail(0)];

        assert_eq!(
            page_item_uniform_height(&card_items, layout),
            layout.row_height
        );
        assert_eq!(
            page_item_uniform_height(&non_card_items, layout),
            px((54. + 58. + 54.) / 3.)
        );
        assert_eq!(page_item_uniform_height(&[], layout), row_height());
    }

    #[test]
    fn list_overdraw_preloads_twelve_track_rows() {
        assert_eq!(TRACK_LIST_OVERDRAW_ROWS, 12.);
        assert_eq!(TRACK_LIST_OVERDRAW, TRACK_ROW_HEIGHT * 12.);
        assert_eq!(overdraw(), gpui::px(696.));
    }

    #[test]
    fn library_scrollbar_outset_tracks_the_content_gutter() {
        assert_eq!(library_scrollbar_outset(false), gpui::px(28.));
        assert_eq!(library_scrollbar_outset(true), gpui::px(12.));
    }

    #[test]
    fn library_scrollbar_reveals_on_hover_while_keeping_auto_hide() {
        let implementation = include_str!("virtualization.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map_or_else(
                || panic!("virtualization implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("Scrollbar::vertical(state)"));
        assert!(implementation.contains("ScrollbarShow::Hover"));
    }

    #[test]
    fn preview_queue_length_matches_the_visible_slice() {
        assert_eq!(displayed_track_count(204, Some(5)), 5);
        assert_eq!(displayed_track_count(3, Some(5)), 3);
        assert_eq!(displayed_track_count(204, None), 204);
    }

    #[test]
    fn content_identity_changes_for_filter_order_and_route() {
        let first = content_identity("route-a", "", ["one".to_owned(), "two".to_owned()]);
        assert_eq!(
            first,
            content_identity("route-a", "", ["one".to_owned(), "two".to_owned()])
        );
        assert_ne!(
            first,
            content_identity("route-a", "", ["two".to_owned(), "one".to_owned()])
        );
        assert_ne!(
            first,
            content_identity("route-a", "filter", ["one".to_owned()])
        );
        assert_ne!(
            first,
            content_identity("route-b", "", ["one".to_owned(), "two".to_owned()])
        );
    }
}
