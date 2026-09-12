use gpui::{AnimationExt as _, AnyElement, div, prelude::*, px, rgb};

use super::model::Route;
use crate::music_ui::{
    TrackSkeletonContext, card_grid_skeleton_count, track_skeleton_count, track_skeleton_list,
};
use crate::theme::{BORDER, SURFACE, SURFACE_RAISED};

const ARTIST_TRACK_ROWS: usize = 5;
const COLLECTION_HEADING_HEIGHT_PX: f32 = 72.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LoadingShape {
    Tracks { collection_heading: bool },
    Cards,
    Artist,
}

pub(super) fn shape(route: &Route) -> LoadingShape {
    match route.action.as_str() {
        "artist" => LoadingShape::Artist,
        "tracks" | "history" | "myTracks" | "albumTracks" | "playlistTracks" | "artistTracks"
        | "stationTracks" | "flowTracks" => LoadingShape::Tracks {
            collection_heading: matches!(route.action.as_str(), "albumTracks" | "playlistTracks"),
        },
        _ => LoadingShape::Cards,
    }
}

fn route_has_nested_heading(route: &Route) -> bool {
    route.action != route.category.action() || !route.id.is_empty()
}

fn heading_visible(route: &Route, requested: bool) -> bool {
    requested && route_has_nested_heading(route)
}

pub(super) fn track_count(available_height: f32, collection_heading: bool) -> usize {
    track_skeleton_count(
        available_height,
        if collection_heading {
            COLLECTION_HEADING_HEIGHT_PX
        } else {
            Default::default()
        },
    )
}

pub(super) fn render(
    route: &Route,
    columns: u16,
    available_width: f32,
    available_height: f32,
    narrow: bool,
    show_heading: bool,
) -> AnyElement {
    let body = match shape(route) {
        LoadingShape::Tracks {
            collection_heading: has_collection_heading,
        } => div()
            .when(has_collection_heading && show_heading, |this| {
                this.child(collection_heading())
            })
            .child(track_skeleton_list(
                track_count(available_height, has_collection_heading && show_heading),
                available_width,
                narrow,
                TrackSkeletonContext::NoProvider,
            )),
        LoadingShape::Cards => div().child(card_grid(
            card_grid_skeleton_count(columns, available_width, available_height),
            columns,
        )),
        LoadingShape::Artist => div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(section(
                SectionKind::Tracks,
                ARTIST_TRACK_ROWS,
                columns,
                available_width,
                narrow,
            ))
            .child(section(
                SectionKind::Cards,
                usize::from(columns),
                columns,
                available_width,
                narrow,
            ))
            .child(section(
                SectionKind::Cards,
                usize::from(columns),
                columns,
                available_width,
                narrow,
            ))
            .child(section(
                SectionKind::Cards,
                usize::from(columns),
                columns,
                available_width,
                narrow,
            )),
    };

    div()
        .flex()
        .flex_col()
        .gap(px(24.))
        .when(heading_visible(route, show_heading), |this| {
            this.child(heading())
        })
        .child(body)
        .with_animation(
            "library-skeleton-pulse",
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
        .min_h(px(45.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(block(px(230.), px(18.)))
        .child(block(px(430.), px(10.)))
}

fn collection_heading() -> impl IntoElement {
    div()
        .h(px(72.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .size(px(64.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE_RAISED)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(block(px(220.), px(17.)))
                .child(block(px(150.), px(10.))),
        )
}

fn card_grid(count: usize, columns: u16) -> impl IntoElement {
    div()
        .w_full()
        .grid()
        .grid_cols(columns)
        .gap(px(12.))
        .children((0..count).map(|_| card()))
}

fn card() -> impl IntoElement {
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

fn section(
    kind: SectionKind,
    count: usize,
    columns: u16,
    available_width: f32,
    narrow: bool,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .w(px(104.))
                        .h(px(13.))
                        .rounded(px(4.))
                        .bg(rgb(SURFACE_RAISED)),
                )
                .child(
                    div()
                        .w(px(54.))
                        .h(px(9.))
                        .rounded(px(3.))
                        .bg(rgb(SURFACE_RAISED)),
                ),
        )
        .child(match kind {
            SectionKind::Tracks => track_skeleton_list(
                count,
                available_width,
                narrow,
                TrackSkeletonContext::NoProvider,
            ),
            SectionKind::Cards => card_grid(count, columns).into_any_element(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Provider;

    fn route(action: &str) -> Route {
        Route {
            source: Provider::Deezer,
            category: crate::library::Category::Tracks,
            action: action.into(),
            id: String::new(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        }
    }

    #[test]
    fn classifies_root_and_nested_loading_shapes() {
        assert_eq!(
            shape(&route("tracks")),
            LoadingShape::Tracks {
                collection_heading: false
            }
        );
        assert_eq!(
            shape(&route("albumTracks")),
            LoadingShape::Tracks {
                collection_heading: true
            }
        );
        assert_eq!(
            shape(&route("playlistTracks")),
            LoadingShape::Tracks {
                collection_heading: true
            }
        );
        assert_eq!(
            shape(&route("flowTracks")),
            LoadingShape::Tracks {
                collection_heading: false
            }
        );
        assert_eq!(shape(&route("artist")), LoadingShape::Artist);
        assert_eq!(shape(&route("artists")), LoadingShape::Cards);
    }

    #[test]
    fn root_loading_omits_the_duplicate_page_heading() {
        assert!(!heading_visible(&route("tracks"), false));
        assert!(heading_visible(&route("albumTracks"), true));
    }

    #[test]
    fn track_count_grows_with_available_height() {
        assert_eq!(track_count(100., false), 3);
        assert_eq!(track_count(580., false), 11);
        assert!(track_count(1_200., false) > track_count(580., false));
    }

    #[test]
    fn nested_collection_track_count_reserves_its_heading() {
        assert_eq!(track_count(580., true), 10);
        assert!(track_count(580., true) < track_count(580., false));
        let source = include_str!("skeleton.rs");
        assert!(source.contains("has_collection_heading && show_heading"));
    }
}
