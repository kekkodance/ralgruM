use gpui::{AnimationExt as _, AnyElement, div, prelude::*, px, rgb};

use crate::{
    music_ui::{
        TrackSkeletonContext, card_carousel_has_overflow, card_grid_skeleton_count,
        card_row_metrics, track_skeleton_count, track_skeleton_count_with_minimum,
        track_skeleton_list,
    },
    theme::{BORDER, SURFACE, SURFACE_RAISED},
};

use super::{
    detail::DetailRoute,
    models::{Provider, ResultType},
    results_view::SEARCH_SECTION_HEADER_HEIGHT_PX,
};

const CARD_COUNT: usize = 12;
const CAROUSEL_SKELETON_CARD_COUNT: usize = 32;
const COLLECTION_DETAIL_MIN_TRACKS: usize = 8;
const COLLECTION_DETAIL_HEADING_HEIGHT: f32 = 76.;
const COLLECTION_DETAIL_SECTION_GAP: f32 = 12.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LoadingShape {
    All,
    Tracks,
    Cards,
    CollectionDetail,
    DeezerArtistDetail,
    SoundCloudArtistDetail,
}

pub(super) fn shape(result_type: ResultType, route: Option<&DetailRoute>) -> LoadingShape {
    if let Some(route) = route {
        return match (route.kind, route.provider) {
            (ResultType::Artists, Provider::Deezer) => LoadingShape::DeezerArtistDetail,
            (ResultType::Artists, Provider::SoundCloud) => LoadingShape::SoundCloudArtistDetail,
            _ => LoadingShape::CollectionDetail,
        };
    }
    match result_type {
        ResultType::All => LoadingShape::All,
        ResultType::Tracks => LoadingShape::Tracks,
        ResultType::Albums | ResultType::Artists | ResultType::Playlists => LoadingShape::Cards,
    }
}

fn search_root_track_context(narrow: bool, provider_icon_only: bool) -> TrackSkeletonContext {
    TrackSkeletonContext::SearchRoot {
        provider_compact: narrow || provider_icon_only,
    }
}

pub(super) fn render(
    result_type: ResultType,
    route: Option<&DetailRoute>,
    columns: u16,
    narrow: bool,
    provider_icon_only: bool,
    available_width: f32,
    available_height: f32,
) -> AnyElement {
    let body: AnyElement = match shape(result_type, route) {
        LoadingShape::All => div()
            .flex()
            .flex_col()
            .gap(px(36.))
            .child(section(
                SectionKind::Tracks,
                0,
                5,
                columns,
                true,
                narrow,
                SectionChrome::SearchResults,
                search_root_track_context(narrow, provider_icon_only),
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                1,
                CARD_COUNT,
                columns,
                true,
                narrow,
                SectionChrome::SearchResults,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                2,
                CARD_COUNT,
                columns,
                true,
                narrow,
                SectionChrome::SearchResults,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                3,
                CARD_COUNT,
                columns,
                true,
                narrow,
                SectionChrome::SearchResults,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .into_any_element(),
        LoadingShape::Tracks => section(
            SectionKind::Tracks,
            0,
            dedicated_track_count(available_height),
            columns,
            false,
            narrow,
            SectionChrome::SearchResults,
            search_root_track_context(narrow, provider_icon_only),
            available_width,
        )
        .into_any_element(),
        LoadingShape::Cards => section(
            SectionKind::Cards,
            0,
            card_grid_skeleton_count(columns, available_width, available_height),
            columns,
            false,
            narrow,
            SectionChrome::SearchResults,
            TrackSkeletonContext::NoProvider,
            available_width,
        )
        .into_any_element(),
        LoadingShape::CollectionDetail => div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(heading())
            .child(track_skeleton_list(
                collection_detail_track_count(available_height),
                available_width,
                narrow,
                TrackSkeletonContext::NoProvider,
            ))
            .into_any_element(),
        LoadingShape::DeezerArtistDetail => div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(heading())
            .child(section(
                SectionKind::Tracks,
                0,
                5,
                columns,
                false,
                narrow,
                SectionChrome::Artist,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                1,
                CARD_COUNT,
                columns,
                false,
                narrow,
                SectionChrome::Artist,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                2,
                CARD_COUNT,
                columns,
                false,
                narrow,
                SectionChrome::Artist,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .child(section(
                SectionKind::Cards,
                3,
                CARD_COUNT,
                columns,
                false,
                narrow,
                SectionChrome::Artist,
                TrackSkeletonContext::NoProvider,
                available_width,
            ))
            .into_any_element(),
        LoadingShape::SoundCloudArtistDetail => div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(artist_heading())
            .child(artist_tracks_section(available_width, narrow))
            .into_any_element(),
    };
    div()
        .child(body)
        .w_full()
        .flex()
        .flex_col()
        .gap(px(22.))
        .with_animation(
            "search-skeleton-pulse",
            crate::motion::skeleton_loop(),
            |this, delta| this.opacity(crate::motion::lerp(0.72, 0.96, delta)),
        )
        .into_any_element()
}

fn block(width: gpui::Pixels, height: gpui::Pixels) -> impl IntoElement {
    div()
        .w(width)
        .h(height)
        .rounded(px(4.))
        .bg(rgb(SURFACE_RAISED))
}

fn heading() -> impl IntoElement {
    div()
        .min_h(px(76.))
        .flex()
        .items_center()
        .gap(px(16.))
        .child(
            div()
                .size(px(76.))
                .flex_none()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE_RAISED)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(9.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(block(px(230.), px(18.)))
                        .child(block(px(18.), px(13.))),
                )
                .child(block(px(330.), px(10.))),
        )
}

fn artist_heading() -> impl IntoElement {
    div()
        .w_full()
        .min_h(px(76.))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(
                    div()
                        .size(px(76.))
                        .flex_none()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(SURFACE_RAISED)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(block(px(230.), px(18.)))
                        .child(block(px(150.), px(12.))),
                ),
        )
        .child(block(px(76.), px(20.)))
}

fn artist_tracks_section(available_width: f32, narrow: bool) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(9.))
        .child(block(px(104.), px(15.)))
        .child(track_skeleton_list(
            8,
            available_width,
            narrow,
            TrackSkeletonContext::NoProvider,
        ))
        .into_any_element()
}

fn collection_detail_track_count(available_height: f32) -> usize {
    track_skeleton_count_with_minimum(
        available_height,
        COLLECTION_DETAIL_HEADING_HEIGHT + COLLECTION_DETAIL_SECTION_GAP,
        COLLECTION_DETAIL_MIN_TRACKS,
    )
}

fn dedicated_track_count(available_height: f32) -> usize {
    track_skeleton_count(available_height, SEARCH_SECTION_HEADER_HEIGHT_PX + 10.)
}

fn card_grid(count: usize, columns: u16) -> impl IntoElement {
    div()
        .w_full()
        .grid()
        .grid_cols(columns)
        .gap(px(12.))
        .children((0..count).map(|_| card()))
}

fn card() -> gpui::Div {
    div()
        .min_w_0()
        .p(px(8.))
        .rounded(px(12.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .child(
            div()
                .w_full()
                .aspect_square()
                .rounded(px(8.))
                .bg(rgb(SURFACE_RAISED)),
        )
        .child(
            div()
                .mt(px(10.))
                .flex()
                .flex_col()
                .gap(px(7.))
                .child(block(px(110.), px(11.)))
                .child(block(px(70.), px(9.))),
        )
}

#[derive(Clone, Copy)]
enum SectionKind {
    Tracks,
    Cards,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SectionChrome {
    SearchResults,
    Artist,
}

fn section(
    kind: SectionKind,
    section_index: usize,
    count: usize,
    columns: u16,
    preview: bool,
    narrow: bool,
    chrome: SectionChrome,
    context: TrackSkeletonContext,
    available_width: f32,
) -> impl IntoElement {
    let (card_width, row_gap) = card_row_metrics(narrow);
    div()
        .flex()
        .flex_col()
        .gap(px(match chrome {
            SectionChrome::SearchResults => 10.,
            SectionChrome::Artist => 9.,
        }))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .when(chrome == SectionChrome::SearchResults, |this| {
                    this.h(px(SEARCH_SECTION_HEADER_HEIGHT_PX))
                })
                .child(block(px(104.), px(13.)))
                .child(block(px(54.), px(9.))),
        )
        .child(match kind {
            SectionKind::Tracks => track_skeleton_list(count, available_width, narrow, context),
            SectionKind::Cards if preview => {
                let count = count.max(CAROUSEL_SKELETON_CARD_COUNT);
                div()
                    .child(crate::music_ui::horizontal_scroll_boundary(
                        format!(
                            "{}-boundary",
                            crate::music_ui::horizontal_scroll_id(
                                "search-skeleton-card-scroll",
                                "cards",
                                section_index,
                            )
                        ),
                        crate::music_ui::card_carousel(
                            crate::music_ui::horizontal_scroll_id(
                                "search-skeleton-card-scroll",
                                "cards",
                                section_index,
                            ),
                            crate::music_ui::CardCarouselState::new(),
                            card_width,
                            row_gap,
                            card_carousel_has_overflow(
                                count,
                                card_width,
                                row_gap,
                                available_width,
                                0.,
                            ),
                            div()
                                .flex()
                                .flex_none()
                                .gap(px(row_gap))
                                .children((0..count).map(|_| card_grid_item(card_width)))
                                .into_any_element(),
                        ),
                    ))
                    .into_any_element()
            }
            SectionKind::Cards => card_grid(count, columns).into_any_element(),
        })
}

fn card_grid_item(width: f32) -> impl IntoElement {
    card().w(px(width)).flex_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(kind: ResultType, provider: Provider) -> DetailRoute {
        DetailRoute {
            provider,
            kind,
            id: "1".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        }
    }

    #[test]
    fn classifies_search_tabs() {
        assert_eq!(shape(ResultType::All, None), LoadingShape::All);
        assert_eq!(shape(ResultType::Tracks, None), LoadingShape::Tracks);
        assert_eq!(shape(ResultType::Albums, None), LoadingShape::Cards);
        assert_eq!(shape(ResultType::Artists, None), LoadingShape::Cards);
        assert_eq!(shape(ResultType::Playlists, None), LoadingShape::Cards);
    }

    #[test]
    fn classifies_detail_routes_by_provider_and_kind() {
        assert_eq!(
            shape(
                ResultType::All,
                Some(&route(ResultType::Artists, Provider::Deezer))
            ),
            LoadingShape::DeezerArtistDetail
        );
        assert_eq!(
            shape(
                ResultType::All,
                Some(&route(ResultType::Artists, Provider::SoundCloud))
            ),
            LoadingShape::SoundCloudArtistDetail
        );
        assert_eq!(
            shape(
                ResultType::All,
                Some(&route(ResultType::Albums, Provider::Deezer))
            ),
            LoadingShape::CollectionDetail
        );
        assert_eq!(
            shape(
                ResultType::All,
                Some(&route(ResultType::Playlists, Provider::SoundCloud))
            ),
            LoadingShape::CollectionDetail
        );
    }

    #[test]
    fn collection_detail_tracks_fill_tall_viewports() {
        assert_eq!(collection_detail_track_count(400.), 8);
        assert_eq!(collection_detail_track_count(760.), 12);
        assert_eq!(collection_detail_track_count(1000.), 16);
    }

    #[test]
    fn dedicated_tracks_loading_fills_taller_viewports() {
        assert!(dedicated_track_count(1_000.) > dedicated_track_count(500.));
        assert!(dedicated_track_count(500.) >= 8);
    }

    #[test]
    fn soundcloud_artist_loading_matches_the_artist_tracks_structure() {
        let implementation = include_str!("skeleton.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("search skeleton implementation is missing"),
                |(implementation, _)| implementation,
            );
        let branch = implementation
            .split_once("LoadingShape::SoundCloudArtistDetail =>")
            .and_then(|(_, branch)| branch.split_once("    };"))
            .map(|(branch, _)| branch)
            .expect("SoundCloud artist loading branch");
        assert!(branch.contains("artist_heading()"));
        assert!(branch.contains("artist_tracks_section(available_width, narrow)"));
        assert!(!branch.contains("card_grid("));
    }

    #[test]
    fn track_loader_contexts_distinguish_search_roots_from_details() {
        let source = include_str!("skeleton.rs");
        assert_eq!(
            search_root_track_context(true, false),
            TrackSkeletonContext::SearchRoot {
                provider_compact: true
            }
        );
        assert_eq!(
            search_root_track_context(false, false),
            TrackSkeletonContext::SearchRoot {
                provider_compact: false
            }
        );
        assert!(source.contains("TrackSkeletonContext::NoProvider"));
        assert!(source.contains("track_skeleton_count_with_minimum("));
    }

    #[test]
    fn search_result_sections_reserve_the_loaded_header_height() {
        assert_eq!(SEARCH_SECTION_HEADER_HEIGHT_PX, 28.);
        let source = include_str!("skeleton.rs");
        assert!(source.contains("this.h(px(SEARCH_SECTION_HEADER_HEIGHT_PX))"));
        assert!(source.contains("SectionChrome::SearchResults => 10."));
        assert!(source.contains("SectionChrome::Artist => 9."));
    }
}
