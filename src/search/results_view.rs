use std::time::Instant;

use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    theme::{BORDER, FOREGROUND, MUTED, SURFACE, SURFACE_RAISED},
};
use gpui::{
    AnimationExt as _, AnyElement, Context, FontWeight, IntoElement, KeyDownEvent, Render,
    ScrollHandle, StatefulInteractiveElement, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::scroll::{ScrollableElement, Scrollbar, ScrollbarShow};

use super::{
    models::{Groups, ResultState, ResultType, Source},
    rows_view::render_tracks,
    view::SearchView,
};

fn all_results_are_track_only(groups: &Groups) -> bool {
    groups.is_track_only()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SearchCategoryGeometry {
    pub(super) left: f32,
    pub(super) width: f32,
}

const SEARCH_CATEGORY_LABELS: [&str; 5] = ["All", "Tracks", "Albums", "Artists", "Playlists"];
const SEARCH_CATEGORY_ICON_WIDTH: f32 = 13.;
const SEARCH_CATEGORY_LABEL_CHAR_WIDTH: f32 = 6.5;
const SEARCH_CATEGORY_LABEL_GAP: f32 = 7.;
const SEARCH_CATEGORY_HORIZONTAL_PADDING: f32 = 16.;
const SEARCH_CATEGORY_BORDER_WIDTH: f32 = 2.;
const SEARCH_CATEGORY_GAP: f32 = 2.;
const SEARCH_RESULT_COUNT_MIN_WIDTH: f32 = 72.;
const SEARCH_RESULT_COUNT_MAX_WIDTH: f32 = 160.;
const SEARCH_RESULT_COUNT_CHAR_WIDTH: f32 = 7.2;
const SEARCH_RESULTS_BOTTOM_PADDING_PX: f32 = 16.;
pub(super) const SEARCH_SECTION_HEADER_HEIGHT_PX: f32 = 28.;
const SEARCH_VIEW_ALL_HEIGHT_PX: f32 = 22.;
const SEARCH_VIEW_ALL_GAP_PX: f32 = 9.;
const SEARCH_VIEW_ALL_CHROME_OFFSET_PX: f32 = 1.;
const SEARCH_TRACK_PREVIEW_LIMIT: usize = 5;
const SEARCH_CARD_PREVIEW_LIMIT: usize = 12;

fn search_category_item_width(label: &str, icon_only: bool) -> f32 {
    if icon_only {
        crate::music_ui::CATEGORY_TAB_ICON_WIDTH
    } else {
        SEARCH_CATEGORY_ICON_WIDTH
            + SEARCH_CATEGORY_LABEL_GAP
            + label.chars().count() as f32 * SEARCH_CATEGORY_LABEL_CHAR_WIDTH
            + SEARCH_CATEGORY_HORIZONTAL_PADDING
            + SEARCH_CATEGORY_BORDER_WIDTH
    }
}

fn search_result_count_width(label: &str) -> f32 {
    (label.chars().count() as f32 * SEARCH_RESULT_COUNT_CHAR_WIDTH + 2.)
        .clamp(SEARCH_RESULT_COUNT_MIN_WIDTH, SEARCH_RESULT_COUNT_MAX_WIDTH)
}

fn should_show_result_count(result_type: ResultType, show_search_result_count: bool) -> bool {
    result_type == ResultType::All && show_search_result_count
}

pub(super) fn search_category_geometry(icon_only: bool) -> Vec<SearchCategoryGeometry> {
    let mut left = 0.;
    SEARCH_CATEGORY_LABELS
        .into_iter()
        .map(|label| {
            let geometry = SearchCategoryGeometry {
                left,
                width: search_category_item_width(label, icon_only),
            };
            left += geometry.width + SEARCH_CATEGORY_GAP;
            geometry
        })
        .collect()
}

fn search_category_geometry_at_position(
    geometry: &[SearchCategoryGeometry],
    position: f32,
) -> SearchCategoryGeometry {
    if geometry.is_empty() {
        return SearchCategoryGeometry {
            left: 0.,
            width: 0.,
        };
    }
    let position = position.clamp(0., geometry.len().saturating_sub(1) as f32);
    let start_index = position.floor() as usize;
    let end_index = (start_index + 1).min(geometry.len() - 1);
    let fraction = position - start_index as f32;
    let start = geometry[start_index];
    let end = geometry[end_index];
    SearchCategoryGeometry {
        left: crate::motion::lerp(start.left, end.left, fraction),
        width: crate::motion::lerp(start.width, end.width, fraction),
    }
}

fn result_type_index(result_type: ResultType) -> usize {
    match result_type {
        ResultType::All => 0,
        ResultType::Tracks => 1,
        ResultType::Albums => 2,
        ResultType::Artists => 3,
        ResultType::Playlists => 4,
    }
}

fn search_category_indicator(
    geometry: Vec<SearchCategoryGeometry>,
    visual: crate::motion::SegmentedSelectorVisual,
) -> impl IntoElement {
    let initial = search_category_geometry_at_position(&geometry, visual.position);
    div()
        .absolute()
        .top_0()
        .left(px(initial.left))
        .w(px(initial.width))
        .h(px(30.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x818cf8d9))
        .bg(rgba(0x6366f13d))
        .with_animation(
            ("search-category-indicator", visual.epoch),
            crate::motion::content(),
            move |this, delta| {
                let geometry = search_category_geometry_at_position(
                    &geometry,
                    visual.animated_position(delta),
                );
                this.left(px(geometry.left)).w(px(geometry.width))
            },
        )
}

pub(super) fn is_dedicated_card_result(result_type: ResultType) -> bool {
    matches!(
        result_type,
        ResultType::Albums | ResultType::Artists | ResultType::Playlists
    )
}

pub(super) fn should_virtualize_results(
    result_type: ResultType,
    state: &ResultState,
    groups: &Groups,
) -> bool {
    matches!(state, ResultState::Results)
        && ((is_dedicated_card_result(result_type) && groups.count(result_type) > 0)
            || (!groups.tracks.is_empty()
                && (result_type == ResultType::Tracks
                    || (result_type == ResultType::All && all_results_are_track_only(groups)))))
}

fn preview_limit(result_type: ResultType) -> usize {
    match result_type {
        ResultType::Tracks => SEARCH_TRACK_PREVIEW_LIMIT,
        ResultType::Albums | ResultType::Artists | ResultType::Playlists => {
            SEARCH_CARD_PREVIEW_LIMIT
        }
        ResultType::All => 0,
    }
}

fn is_preview_expandable(result_type: ResultType, count: usize) -> bool {
    if result_type == ResultType::All {
        return false;
    }
    count > preview_limit(result_type)
}

fn view_toggle_label(all: bool) -> &'static str {
    if all { "View all" } else { "View less" }
}

fn should_show_section_count(all: bool, result_type: ResultType, groups: &Groups) -> bool {
    if !all {
        return true;
    }
    if result_type == ResultType::Tracks && groups.is_track_only() {
        return false;
    }
    true
}

fn view_all_gap(show_section_count: bool) -> f32 {
    if show_section_count {
        SEARCH_VIEW_ALL_GAP_PX
    } else {
        0.
    }
}

fn section_count_element(count: usize) -> impl IntoElement {
    div()
        .flex_none()
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .whitespace_nowrap()
        .child(format!("{count} found"))
}

// Static toggle button. View all in the All preview navigates to the
// dedicated virtualized tab, while View less on a dedicated tab returns to
// All. The button uses natural width so it hugs its label. No animation runs here.
fn view_toggle_button(
    section: ResultType,
    label: &str,
    dedicated_return: bool,
    cx: &mut Context<SearchView>,
) -> AnyElement {
    let label_owned = label.to_owned();
    let aria_label = format!(
        "{} {}",
        label_owned.to_ascii_lowercase(),
        section.label().to_ascii_lowercase()
    );
    let base = div()
        .id(format!(
            "view-toggle-{}",
            section.label().to_ascii_lowercase()
        ))
        .focusable()
        .tab_stop(true)
        .role(gpui::Role::Button)
        .aria_label(aria_label)
        .px(px(7.))
        .h(px(SEARCH_VIEW_ALL_HEIGHT_PX))
        .relative()
        .top(px(SEARCH_VIEW_ALL_CHROME_OFFSET_PX))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .whitespace_nowrap()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .text_size(px(11.))
        .text_color(rgb(FOREGROUND))
        .cursor_pointer()
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .child(
            div()
                .relative()
                .top(px(-SEARCH_VIEW_ALL_CHROME_OFFSET_PX))
                .child(label_owned),
        );
    if dedicated_return {
        base.on_click(cx.listener(move |this, _, _, cx| {
            this.return_to_all_from_dedicated(section, cx);
        }))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                window.prevent_default();
                this.return_to_all_from_dedicated(section, cx);
            }
        }))
        .into_any_element()
    } else {
        base.on_click(cx.listener(move |this, _, _, cx| {
            this.select_type(section, cx);
        }))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                window.prevent_default();
                this.select_type(section, cx);
            }
        }))
        .into_any_element()
    }
}

// Every available section action uses a natural-width slot. Sections without
// more results reserve no invisible button, so their count stays right-aligned.
// Dedicated tabs always show View less to return to All.
fn view_toggle_slot(
    section: ResultType,
    all: bool,
    expandable: bool,
    show_section_count: bool,
    track_only_all: bool,
    cx: &mut Context<SearchView>,
) -> AnyElement {
    if track_only_all || (all && !expandable) {
        return div().flex_none().w(px(0.)).into_any_element();
    }
    if !all {
        let button = view_toggle_button(section, "View less", true, cx);
        return div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .ml(px(view_all_gap(show_section_count)))
            .child(button)
            .into_any_element();
    }
    let label = view_toggle_label(true);
    let button = view_toggle_button(section, label, false, cx);
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .ml(px(view_all_gap(show_section_count)))
        .child(button)
        .into_any_element()
}

pub(super) fn search_results_content_identity(source: Source, query: &str) -> String {
    format!(
        "search-results-body:{:?}:{}",
        source,
        query.trim().to_lowercase()
    )
}

fn search_results_scroll(
    content: AnyElement,
    virtualized: bool,
    scroll: &ScrollHandle,
    browser_scroll: BrowserScrollState,
) -> AnyElement {
    if virtualized {
        return div()
            .id("search-results-scroll")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .into_any_element();
    }
    let scroll_area = div()
        .id("search-results-scroll-content")
        .size_full()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_y_scroll()
        .child(content);
    let scroll_content = div()
        .id("search-results-scroll-viewport")
        .relative()
        .flex_1()
        .min_h_0()
        .child(scroll_area)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(scroll).scrollbar_show(ScrollbarShow::Hover)),
        )
        .into_any_element();
    browser_scroll_surface(
        "search-results-scroll",
        scroll_content,
        BrowserScrollTarget::Handle(scroll.clone()),
        browser_scroll,
    )
}

impl SearchView {
    fn type_chip(
        &self,
        result_type: ResultType,
        index: usize,
        icon_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.state.result_type == result_type;
        let focus = self.type_tab_focus[index].clone();
        let tab_focus = self.type_tab_focus.clone();
        let icon = match result_type {
            ResultType::All => LocalIcon::List,
            ResultType::Tracks => LocalIcon::Music,
            ResultType::Albums => LocalIcon::CompactDisc,
            ResultType::Artists => LocalIcon::UserGroup,
            ResultType::Playlists => LocalIcon::ListUl,
        };
        div()
            .id(result_type.label())
            .track_focus(&focus)
            .tab_stop(selected)
            .role(gpui::Role::Tab)
            .aria_selected(selected)
            .aria_label(result_type.label())
            .h(px(30.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .when(icon_only, |this| {
                this.w(px(crate::music_ui::CATEGORY_TAB_ICON_WIDTH))
                    .px(px(0.))
                    .gap(px(0.))
            })
            .when(!icon_only, |this| {
                this.w(px(search_category_item_width(result_type.label(), false)))
                    .flex_none()
                    .px(px(8.))
                    .gap(px(7.))
            })
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(0x00000000))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(if selected { FOREGROUND } else { MUTED }))
            .bg(rgba(0x00000000))
            .cursor_pointer()
            .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
            .when_some(
                crate::music_ui::category_tab_tooltip(icon_only, result_type.label()),
                |this, tooltip| this.app_tooltip(tooltip),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.select_type(result_type, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.select_type(result_type, cx);
                    return;
                }
                let Some(next) = crate::tab_keyboard::next_tab_index(
                    event.keystroke.key.as_str(),
                    index,
                    tab_focus.len(),
                ) else {
                    return;
                };
                window.prevent_default();
                let next_type = [
                    ResultType::All,
                    ResultType::Tracks,
                    ResultType::Albums,
                    ResultType::Artists,
                    ResultType::Playlists,
                ][next];
                this.select_type(next_type, cx);
                tab_focus[next].focus(window, cx);
            }))
            .child(crate::music_ui::category_tab_icon(
                icon,
                if selected { FOREGROUND } else { MUTED },
                12.,
            ))
            .when(!icon_only, |this| this.child(result_type.label()))
            .into_any_element()
    }

    fn content(
        &self,
        columns: u16,
        available_width: f32,
        available_height: f32,
        narrow: bool,
        provider_icon_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.state.state {
            ResultState::Initial if self.should_show_discover(cx) => super::discover::render(
                self,
                &cx.entity(),
                self.state.source,
                available_width,
                available_height,
                narrow,
                cx,
            ),
            ResultState::Initial => crate::empty_state::render(
                LocalIcon::Headphones,
                "Start searching for music",
                "Type an artist, song, or album in the search bar above to listen in FLAC & HQ.",
                None,
            ),
            ResultState::Loading => super::skeleton::render(
                self.state.result_type,
                None,
                columns,
                narrow,
                provider_icon_only,
                available_width,
                available_height,
            ),
            ResultState::Empty => message(
                LocalIcon::CompactDisc,
                "No results found",
                "Try a different search or result category.",
            ),
            ResultState::Failed(error) => {
                message(LocalIcon::TriangleExclamation, "Search Failed", error)
            }
            ResultState::Results => {
                self.results(columns, available_width, narrow, provider_icon_only, cx)
            }
        }
    }

    fn results(
        &self,
        columns: u16,
        available_width: f32,
        narrow: bool,
        provider_icon_only: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all = self.state.result_type == ResultType::All;
        let track_only_all = all && all_results_are_track_only(&self.state.groups);
        let dedicated_card_result = !all
            && is_dedicated_card_result(self.state.result_type)
            && self.state.groups.count(self.state.result_type) > 0;
        let track_identity = format!(
            "search-results-tracks:{:?}:{:?}:{}",
            self.state.source,
            self.state.result_type,
            self.input.read(cx).value()
        );
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .when(
                self.state.result_type == ResultType::Tracks
                    || track_only_all
                    || dedicated_card_result,
                |this| this.flex_1().min_h_0(),
            )
            .gap(px(if all { 36. } else { 22. }));
        if let Some(warning) = &self.state.warning {
            content = content.child(
                div()
                    .px(px(11.))
                    .py(px(9.))
                    .rounded(px(8.))
                    .bg(rgb(0x211b10))
                    .border_1()
                    .border_color(rgb(0x664619))
                    .text_size(px(12.))
                    .text_color(rgb(0xd4d4d8))
                    .child(warning.clone()),
            );
        }
        for (section_index, result_type) in self.state.result_type.categories().iter().enumerate() {
            let result_type = *result_type;
            let count = self.state.groups.count(result_type);
            if all && count == 0 {
                continue;
            }
            let expandable = all && !track_only_all && is_preview_expandable(result_type, count);
            let scroll_id = crate::music_ui::horizontal_scroll_id(
                "search-card-preview-scroll",
                result_type.label(),
                section_index,
            );
            let carousel_state = self.card_scroll_handle(&scroll_id);
            // The All preview always stays bounded. View all navigates to the
            // dedicated virtualized tab instead of rendering up to 300 rows
            // inline, which exhausted the glyph atlas.
            let body = match result_type {
                ResultType::Tracks => render_tracks(
                    self,
                    &self.state.groups.tracks,
                    all && !track_only_all,
                    narrow,
                    provider_icon_only,
                    true,
                    true,
                    self.favorites.clone(),
                    None,
                    false,
                    self.playback.clone(),
                    self.downloads.clone(),
                    self.account.clone(),
                    None,
                    self.playing.clone(),
                    &track_identity,
                    cx,
                ),
                ResultType::Albums => super::cards_view::render_cards(
                    self,
                    &cx.entity(),
                    &self.state.groups.albums,
                    result_type.label(),
                    section_index,
                    all,
                    columns,
                    available_width,
                    narrow,
                    self.favorites.clone(),
                    self.account.clone(),
                    carousel_state.clone(),
                ),
                ResultType::Artists => super::cards_view::render_cards(
                    self,
                    &cx.entity(),
                    &self.state.groups.artists,
                    result_type.label(),
                    section_index,
                    all,
                    columns,
                    available_width,
                    narrow,
                    self.favorites.clone(),
                    self.account.clone(),
                    carousel_state.clone(),
                ),
                ResultType::Playlists => super::cards_view::render_cards(
                    self,
                    &cx.entity(),
                    &self.state.groups.playlists,
                    result_type.label(),
                    section_index,
                    all,
                    columns,
                    available_width,
                    narrow,
                    self.favorites.clone(),
                    self.account.clone(),
                    carousel_state,
                ),
                ResultType::All => continue,
            };
            let show_section_count =
                should_show_section_count(all, result_type, &self.state.groups);
            let view_toggle = view_toggle_slot(
                result_type,
                all,
                expandable,
                show_section_count,
                track_only_all,
                cx,
            );
            content = content.child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .when(
                        (result_type == ResultType::Tracks && (!all || track_only_all))
                            || dedicated_card_result,
                        |this| this.flex_1().min_h_0(),
                    )
                    .min_h_0()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(SEARCH_SECTION_HEADER_HEIGHT_PX))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(result_type.label()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .when(show_section_count, |this| {
                                        this.child(section_count_element(count))
                                    })
                                    .child(view_toggle),
                            ),
                    )
                    .child(body),
            );
        }
        let content_identity =
            search_results_content_identity(self.state.source, self.search_query());
        // The entrance animation runs once per fresh result set. Reusing the
        // same key across tabs is intentional, but the tab switch remounts
        // this subtree under a different element id, which would restart a
        // shared animation every time. Render the settled end state unless
        // fresh results just armed the gate, so All to Tracks switches keep
        // the heading and rows exactly where they are.
        let entrance = self.results_entrance_key.as_deref() == Some(content_identity.as_str());
        let body = content.relative();
        if entrance {
            body.with_animation(
                content_identity,
                crate::motion::quick_content(),
                |this, delta| {
                    this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                        .top(px(crate::motion::lerp(4.0, 0.0, delta)))
                },
            )
            .into_any_element()
        } else {
            body.into_any_element()
        }
    }
}

impl Render for SearchView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_discover(cx);
        let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
        let right_sidebar_open =
            self.playback.read(cx).state.right_sidebar != crate::playback::RightSidebar::Closed;
        let available_width =
            crate::music_ui::effective_content_width(&metrics, right_sidebar_open);
        let available_height = f32::from(window.viewport_size().height);
        if let Some(request) = self.card_columns.observe_width(available_width) {
            crate::music_ui::schedule_resize(cx, window, request);
        }
        let columns = self.card_columns.columns();
        let now = Instant::now();
        self.card_grid_motion
            .prepare(columns, available_width, now, cx.reduce_motion());
        let narrow = metrics.narrow_content;
        let gutter = crate::music_ui::main_content_inset(&metrics);
        let provider_icon_only = crate::music_ui::compact_track_provider(available_width);
        // Search category tabs always keep their labels. The tab row scrolls
        // horizontally at narrow widths instead of collapsing to icons.
        let type_tabs_icon_only = false;
        let type_tab_visual = self.type_tab_motion.prepare(
            result_type_index(self.state.result_type),
            SEARCH_CATEGORY_LABELS.len(),
            now,
            cx.reduce_motion(),
        );
        let show_search_result_count = crate::music_ui::show_search_result_count(available_width);
        let detail_open = !matches!(self.detail.state, super::detail::DetailState::Closed);
        let virtualized_scroll = self.uses_virtualized_scroll(cx);
        let count = self.state.groups.count(self.state.result_type);
        let count_label = if self.state.result_type == ResultType::All {
            "results"
        } else {
            self.state.result_type.label()
        };
        let count_is_visible =
            should_show_result_count(self.state.result_type, show_search_result_count);
        let loading = matches!(self.state.state, ResultState::Loading);
        let current_count_label = format!("{count} {} found", count_label.to_ascii_lowercase());
        if count_is_visible && !loading {
            self.last_result_count_label = current_count_label;
        }
        let retained_count_label = if self.last_result_count_label.is_empty() {
            "Loading…"
        } else {
            self.last_result_count_label.as_str()
        };
        let displayed_count_label = if loading {
            "Loading…"
        } else {
            retained_count_label
        };
        let count_width = search_result_count_width(retained_count_label);
        let count_visual =
            self.result_count_responsive
                .prepare(!count_is_visible, now, cx.reduce_motion());
        let (count_from_width, count_target_width) = count_visual.endpoints(count_width, 0.);
        let (count_from_opacity, count_target_opacity) = count_visual.endpoints(1., 0.);
        let (count_from_margin, count_target_margin) = count_visual.endpoints(10., 0.);
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(
                !detail_open && !matches!(self.state.state, ResultState::Initial),
                |this| {
                    this.child(
                        div()
                            .w_full()
                            .px(px(gutter))
                            .min_h(px(50.))
                            .pb(px(12.))
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(px(0.))
                            .child(crate::music_ui::horizontal_scroll_boundary(
                                "search-result-type",
                                div()
                                    .id("search-result-type-tabs")
                                    .role(gpui::Role::TabList)
                                    .aria_label("Search result type")
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_x_scrollbar()
                                    .child(
                                        div()
                                            .flex()
                                            .flex_none()
                                            .items_center()
                                            .gap(px(2.))
                                            .p(px(3.))
                                            .rounded(px(8.))
                                            .border_1()
                                            .border_color(rgb(BORDER))
                                            .bg(rgb(SURFACE_RAISED))
                                            .child(
                                                div()
                                                    .relative()
                                                    .flex()
                                                    .flex_none()
                                                    .items_center()
                                                    .gap(px(2.))
                                                    .child(search_category_indicator(
                                                        search_category_geometry(
                                                            type_tabs_icon_only,
                                                        ),
                                                        type_tab_visual,
                                                    ))
                                                    .children([
                                                        self.type_chip(
                                                            ResultType::All,
                                                            0,
                                                            type_tabs_icon_only,
                                                            cx,
                                                        ),
                                                        self.type_chip(
                                                            ResultType::Tracks,
                                                            1,
                                                            type_tabs_icon_only,
                                                            cx,
                                                        ),
                                                        self.type_chip(
                                                            ResultType::Albums,
                                                            2,
                                                            type_tabs_icon_only,
                                                            cx,
                                                        ),
                                                        self.type_chip(
                                                            ResultType::Artists,
                                                            3,
                                                            type_tabs_icon_only,
                                                            cx,
                                                        ),
                                                        self.type_chip(
                                                            ResultType::Playlists,
                                                            4,
                                                            type_tabs_icon_only,
                                                            cx,
                                                        ),
                                                    ]),
                                            ),
                                    )
                                    .into_any_element(),
                            ))
                            .child(
                                div()
                                    .id("search-result-count")
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .w(px(count_target_width))
                                    .ml(px(count_target_margin))
                                    .opacity(count_target_opacity)
                                    .text_size(px(13.))
                                    .text_color(rgb(MUTED))
                                    .child(displayed_count_label.to_owned())
                                    .with_animation(
                                        ("search-result-count", count_visual.epoch),
                                        crate::motion::content(),
                                        move |this, delta| {
                                            this.w(px(crate::motion::lerp(
                                                count_from_width,
                                                count_target_width,
                                                delta,
                                            )))
                                            .ml(px(crate::motion::lerp(
                                                count_from_margin,
                                                count_target_margin,
                                                delta,
                                            )))
                                            .opacity(
                                                crate::motion::lerp(
                                                    count_from_opacity,
                                                    count_target_opacity,
                                                    delta,
                                                ),
                                            )
                                        },
                                    ),
                            ),
                    )
                },
            )
            .child({
                let content = div()
                    .w_full()
                    .px(px(gutter))
                    .when(virtualized_scroll, |this| {
                        this.size_full().flex().flex_col().min_h_0()
                    })
                    // Breathing room above the player bar. Padding lives on
                    // the inner content so it only shows at the tail for
                    // div scrolls. Virtualized lists own their own scroll
                    // state, so they must not get a persistent outer gap.
                    .when(!virtualized_scroll, |this| {
                        this.pb(px(SEARCH_RESULTS_BOTTOM_PADDING_PX))
                    })
                    .child(if detail_open {
                        self.detail_content(
                            columns,
                            available_width,
                            available_height,
                            narrow,
                            provider_icon_only,
                            cx,
                        )
                    } else {
                        self.content(
                            columns,
                            available_width,
                            available_height,
                            narrow,
                            provider_icon_only,
                            cx,
                        )
                    });
                search_results_scroll(
                    content.into_any_element(),
                    virtualized_scroll,
                    &self.scroll,
                    self.browser_scroll.clone(),
                )
            })
    }
}

fn message(icon: LocalIcon, title: &str, description: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .py(px(60.))
        .px(px(20.))
        .text_center()
        .text_color(rgb(MUTED))
        .child(local_icon(icon, MUTED).size(px(40.)))
        .child(
            div()
                .mt(px(12.))
                .mb(px(4.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .when(!description.is_empty(), |this| {
            this.child(div().text_size(px(13.)).child(description.to_owned()))
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(count: usize) -> Groups {
        Groups {
            tracks: (0..count)
                .map(|index| super::super::models::Track {
                    id: index.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Groups::default()
        }
    }

    fn cards(count: usize, kind: ResultType) -> Vec<super::super::models::Card> {
        (0..count)
            .map(|index| super::super::models::Card {
                kind,
                id: index.to_string(),
                source: super::super::models::Provider::Deezer,
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn all_track_only_results_expand_both_large_and_small_track_sets() {
        for count in [3, 8] {
            let groups = tracks(count);
            assert!(all_results_are_track_only(&groups));
            assert!(should_virtualize_results(
                ResultType::All,
                &ResultState::Results,
                &groups,
            ));
        }
    }

    #[test]
    fn mixed_all_results_keep_the_preview_mode() {
        let mut groups = tracks(8);
        groups.albums.push(Default::default());
        assert!(!all_results_are_track_only(&groups));
        assert_eq!(preview_limit(ResultType::Tracks), 5);
        assert_eq!(preview_limit(ResultType::Albums), 12);
        assert!(is_preview_expandable(ResultType::Tracks, 8));
        assert!(!is_preview_expandable(ResultType::Tracks, 5));
        assert!(is_preview_expandable(ResultType::Albums, 13));
        assert!(!is_preview_expandable(ResultType::Albums, 12));
        assert!(!should_virtualize_results(
            ResultType::All,
            &ResultState::Results,
            &groups,
        ));
    }

    #[test]
    fn tracks_tab_remains_virtualized_and_empty_results_do_not() {
        let groups = tracks(8);
        assert!(should_virtualize_results(
            ResultType::Tracks,
            &ResultState::Results,
            &groups,
        ));
        assert!(!should_virtualize_results(
            ResultType::All,
            &ResultState::Results,
            &Groups::default(),
        ));
        assert!(!should_virtualize_results(
            ResultType::All,
            &ResultState::Loading,
            &groups,
        ));
    }

    #[test]
    fn dedicated_card_categories_choose_virtualization() {
        for result_type in [
            ResultType::Albums,
            ResultType::Artists,
            ResultType::Playlists,
        ] {
            let cards = cards(25, result_type);
            let mut groups = Groups::default();
            match result_type {
                ResultType::Albums => groups.albums = cards,
                ResultType::Artists => groups.artists = cards,
                ResultType::Playlists => groups.playlists = cards,
                ResultType::All | ResultType::Tracks => unreachable!(),
            }
            assert!(is_dedicated_card_result(result_type));
            assert!(should_virtualize_results(
                result_type,
                &ResultState::Results,
                &groups,
            ));
        }
    }

    #[test]
    fn all_card_results_keep_bounded_non_virtual_preview_mode() {
        let groups = Groups {
            albums: cards(25, ResultType::Albums),
            ..Groups::default()
        };
        assert!(!is_dedicated_card_result(ResultType::All));
        assert!(!should_virtualize_results(
            ResultType::All,
            &ResultState::Results,
            &groups,
        ));
        assert_eq!(super::super::cards_view::card_display_count(25, true), 24);
    }

    #[test]
    fn dedicated_card_surface_owns_list_scroll() {
        let implementation = include_str!("cards_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("cards view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("gpui::list(state.clone()"));
        assert!(implementation.contains("library_vertical_scrollbar("));
        assert!(implementation.contains("&fixed_scroll"));
        assert!(implementation.contains("BrowserScrollTarget::FixedList(fixed_scroll)"));
    }

    #[test]
    fn search_main_content_wrapper_keeps_horizontal_gutter_with_inner_bottom_padding() {
        let implementation = include_str!("results_view.rs")
            .split_once("impl Render for SearchView")
            .map_or_else(
                || panic!("search view renderer must have an implementation section"),
                |(_, implementation)| implementation,
            );
        let start_marker = "let content = div()\n                    .w_full()\n                    .px(px(gutter))";
        let start = implementation
            .find(start_marker)
            .unwrap_or_else(|| panic!("search main content wrapper marker is missing"));
        let end = implementation[start..]
            .find(".child(if detail_open")
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("search main content wrapper end marker is missing"));
        let wrapper = &implementation[start..end];

        assert!(wrapper.contains(".px(px(gutter))"));
        // Inner bottom padding only for div scrolls. Virtualized lists own
        // their scroll state and must not get a persistent outer gap.
        assert!(wrapper.contains(".when(!virtualized_scroll"));
        assert!(wrapper.contains(".pb(px(SEARCH_RESULTS_BOTTOM_PADDING_PX))"));
        assert!(!wrapper.contains(".py("));
        assert_eq!(super::SEARCH_RESULTS_BOTTOM_PADDING_PX, 16.);
    }

    #[test]
    fn whole_results_animation_key_uses_source_and_normalized_query() {
        let deezer_tracks = search_results_content_identity(Source::Deezer, "  QuErY  ");

        assert_eq!(deezer_tracks, "search-results-body:Deezer:query");
        assert_ne!(
            deezer_tracks,
            search_results_content_identity(Source::SoundCloud, "query")
        );
        assert_eq!(
            deezer_tracks,
            search_results_content_identity(Source::Deezer, "query")
        );
        assert_ne!(
            deezer_tracks,
            search_results_content_identity(Source::Deezer, "different")
        );
    }

    #[test]
    fn results_entrance_animation_is_gated_on_fresh_results() {
        let implementation = include_str!("results_view.rs")
            .split_once("fn results(")
            .and_then(|(_, rest)| rest.split_once("impl Render for SearchView"))
            .map_or_else(
                || panic!("search results must remain before the view renderer"),
                |(results, _)| results,
            );

        // Tab switches remount this subtree under a different element id,
        // which would restart a shared animation every time. Only the key
        // armed by a fresh completion plays; re-presented tabs settle.
        assert!(implementation.contains("results_entrance_key.as_deref()"));
        assert!(implementation.contains("if entrance {"));
        assert!(implementation.contains("with_animation("));
        assert!(implementation.contains("} else {"));
    }

    #[test]
    fn search_category_geometry_matches_rendered_chip_widths() {
        for icon_only in [false, true] {
            let geometry = search_category_geometry(icon_only);
            let rendered_widths = SEARCH_CATEGORY_LABELS
                .iter()
                .map(|label| search_category_item_width(label, icon_only))
                .collect::<Vec<_>>();

            assert_eq!(geometry.first().map(|item| item.left), Some(0.));
            assert_eq!(geometry.len(), rendered_widths.len());
            for (item, rendered_width) in geometry.iter().zip(rendered_widths.iter()) {
                assert_eq!(item.width, *rendered_width);
            }

            let final_right_edge = geometry.last().map_or(0., |item| item.left + item.width);
            let expected_right_edge = rendered_widths.iter().sum::<f32>()
                + rendered_widths.len().saturating_sub(1) as f32 * SEARCH_CATEGORY_GAP;
            assert_eq!(final_right_edge, expected_right_edge);
        }
    }

    #[test]
    fn search_result_count_visibility_is_limited_to_all_results() {
        assert!(should_show_result_count(ResultType::All, true));
        assert!(!should_show_result_count(ResultType::All, false));
        for result_type in [
            ResultType::Tracks,
            ResultType::Albums,
            ResultType::Artists,
            ResultType::Playlists,
        ] {
            assert!(!should_show_result_count(result_type, true));
            assert!(!should_show_result_count(result_type, false));
        }
    }

    #[test]
    fn search_result_count_width_is_deterministic_and_bounded() {
        let short = search_result_count_width("Loading…");
        let count = search_result_count_width("897 results found");
        let long = search_result_count_width(&"x".repeat(200));

        assert_eq!(short, search_result_count_width("Loading…"));
        assert!(short >= SEARCH_RESULT_COUNT_MIN_WIDTH);
        assert!(count > short);
        assert_eq!(long, SEARCH_RESULT_COUNT_MAX_WIDTH);
    }

    #[test]
    fn search_result_count_slot_is_persistent_and_animates_width_and_opacity() {
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view tests must have an implementation section"),
                |(implementation, _)| implementation,
            );

        assert!(implementation.contains(".id(\"search-result-count\")"));
        assert!(implementation.contains(".flex_none()"));
        assert!(implementation.contains(".overflow_hidden()"));
        assert!(implementation.contains(".whitespace_nowrap()"));
        assert!(implementation.contains(".w(px(count_target_width))"));
        assert!(implementation.contains(".ml(px(count_target_margin))"));
        assert!(implementation.contains(".opacity(count_target_opacity)"));
        assert!(implementation.contains("count_visual.epoch"));
        assert!(implementation.contains("count_visual.endpoints(10., 0.)"));
        assert!(!implementation.contains(".when(count_is_visible"));
    }

    #[test]
    fn track_only_all_results_keep_full_list_without_inline_toggle() {
        assert_eq!(preview_limit(ResultType::Tracks), 5);
        assert!(is_preview_expandable(ResultType::Tracks, 8));
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("if track_only_all"));
        assert!(implementation.contains("fn view_toggle_slot"));
    }

    #[test]
    fn track_only_groups_hide_the_all_preview_count_but_keep_tracks_tab() {
        let track_only = tracks(9);
        assert!(track_only.is_track_only());
        assert!(all_results_are_track_only(&track_only));
        assert!(!should_show_section_count(
            true,
            ResultType::Tracks,
            &track_only
        ));
        assert!(should_show_section_count(
            false,
            ResultType::Tracks,
            &track_only
        ));
    }

    #[test]
    fn mixed_groups_keep_both_all_and_tracks_counts() {
        let mut mixed = tracks(9);
        mixed.albums.push(Default::default());
        assert!(!mixed.is_track_only());
        assert!(!all_results_are_track_only(&mixed));
        assert!(should_show_section_count(true, ResultType::Tracks, &mixed));
        assert!(should_show_section_count(false, ResultType::Tracks, &mixed));
        assert!(should_show_section_count(false, ResultType::Albums, &mixed));
    }

    #[test]
    fn search_results_scroll_uses_the_view_owned_handle() {
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains(".track_scroll(scroll)"));
        assert!(implementation.contains(".overflow_y_scroll()"));
        assert!(implementation.contains("Scrollbar::vertical(scroll)"));
        assert!(implementation.contains("ScrollbarShow::Hover"));
        assert!(implementation.contains("search-results-scroll-viewport"));
        let nested_scrollbar_call = [".vertical_scrollbar", "(scroll)"].concat();
        assert!(!implementation.contains(&nested_scrollbar_call));
        let wrapped_scroll_call = [".overflow_y_scrollbar", "()"].concat();
        assert!(!implementation.contains(&wrapped_scroll_call));
    }

    struct ScrollOwnerProbe {
        scroll: ScrollHandle,
    }

    struct SpacingProbe {
        all_wrappers: bool,
        scroll: ScrollHandle,
        header_y: std::rc::Rc<std::cell::Cell<Option<f32>>>,
        row_y: std::rc::Rc<std::cell::Cell<Option<f32>>>,
    }

    fn spacing_marker(state: std::rc::Rc<std::cell::Cell<Option<f32>>>) -> AnyElement {
        gpui::canvas(
            move |bounds, _, _: &mut gpui::App| {
                state.set(Some(f32::from(bounds.origin.y)));
            },
            |_, _, _, _| {},
        )
        .w_full()
        .h(px(SEARCH_SECTION_HEADER_HEIGHT_PX))
        .into_any_element()
    }

    fn spacing_row() -> AnyElement {
        div().w_full().h(px(54.)).flex_none().into_any_element()
    }

    impl Render for SpacingProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let header = spacing_marker(self.header_y.clone());
            let first_row = spacing_marker(self.row_y.clone());
            let body_all = div()
                .w_full()
                .flex()
                .flex_col()
                .child(first_row)
                .child(spacing_row())
                .into_any_element();
            let list_state = gpui::ListState::new(
                2,
                gpui::ListAlignment::Top,
                crate::library::virtualization::overdraw(),
            )
            .with_uniform_item_height(crate::library::virtualization::row_height());
            let list_browser = BrowserScrollState::new();
            let scrollbar_state = list_state.clone();
            let list_row = self.row_y.clone();
            let track_list = gpui::list(list_state.clone(), move |index, _, _| {
                if index == 0 {
                    spacing_marker(list_row.clone())
                } else {
                    spacing_row()
                }
            });
            let body_tracks = crate::browser_scroll::browser_scroll_surface(
                "search-track-list-scroll",
                div()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(track_list.w_full().h_full().min_h_0())
                    .child(
                        div().absolute().inset_0().child(
                            gpui_component::scroll::Scrollbar::vertical(&scrollbar_state)
                                .scrollbar_show(gpui_component::scroll::ScrollbarShow::Hover),
                        ),
                    )
                    .into_any_element(),
                crate::browser_scroll::BrowserScrollTarget::List(list_state),
                list_browser,
            );
            // Faithful replica of the Render wrapper chain for the mixed All
            // preview (outer div scroll) and the dedicated Tracks tab
            // (virtualized outer), sharing the tab bar, gutter, results, and
            // section wrappers verbatim.
            let content: AnyElement = if self.all_wrappers {
                div()
                    .w_full()
                    .px(px(28.))
                    .pb(px(SEARCH_RESULTS_BOTTOM_PADDING_PX))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .relative()
                            .gap(px(36.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .min_h_0()
                                    .gap(px(10.))
                                    .child(header)
                                    .child(body_all),
                            ),
                    )
                    .into_any_element()
            } else {
                div()
                    .w_full()
                    .px(px(28.))
                    .size_full()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .gap(px(22.))
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_h_0()
                                    .min_h_0()
                                    .gap(px(10.))
                                    .child(header)
                                    .child(body_tracks),
                            ),
                    )
                    .into_any_element()
            };
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().h(px(50.)).flex_none())
                .child(search_results_scroll(
                    content,
                    !self.all_wrappers,
                    &self.scroll,
                    BrowserScrollState::new(),
                ))
        }
    }

    #[gpui::test]
    fn all_preview_and_tracks_tab_track_sections_share_origins(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        for all_wrappers in [true, false] {
            let header_y = std::rc::Rc::new(std::cell::Cell::new(None));
            let row_y = std::rc::Rc::new(std::cell::Cell::new(None));
            let window = cx.add_window({
                let header_y = header_y.clone();
                let row_y = row_y.clone();
                move |_, _| SpacingProbe {
                    all_wrappers,
                    scroll: ScrollHandle::new(),
                    header_y,
                    row_y,
                }
            });
            let mut probe =
                gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
            probe.run_until_parked();
            probe.update(|window, cx| {
                _ = window.draw(cx);
            });
            if all_wrappers {
                assert_eq!(header_y.get(), Some(50.));
                assert_eq!(row_y.get(), Some(88.));
            } else {
                // The dedicated Tracks tab must place its heading and first
                // row exactly where the All preview does.
                assert_eq!(header_y.get(), Some(50.));
                assert_eq!(row_y.get(), Some(88.));
            }
        }
    }

    #[test]
    fn track_section_header_height_is_fixed_regardless_of_view_toggle() {
        assert_eq!(super::SEARCH_SECTION_HEADER_HEIGHT_PX, 28.);
        assert_eq!(super::SEARCH_VIEW_ALL_HEIGHT_PX, 22.);
        assert_eq!(super::SEARCH_VIEW_ALL_CHROME_OFFSET_PX, 1.);
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        // The section header keeps a fixed height so the All preview and the
        // dedicated tab measure identically. The toggle button uses natural
        // width hugging its label, a fixed height with
        // a 1px chrome offset, and never uses vertical padding.
        assert!(implementation.contains(".h(px(SEARCH_SECTION_HEADER_HEIGHT_PX))"));
        assert!(implementation.contains(".h(px(SEARCH_VIEW_ALL_HEIGHT_PX))"));
        assert!(implementation.contains(".top(px(SEARCH_VIEW_ALL_CHROME_OFFSET_PX))"));
        assert!(implementation.contains(".top(px(-SEARCH_VIEW_ALL_CHROME_OFFSET_PX))"));
        assert!(implementation.contains(".justify_center()"));
        let toggle_start = implementation
            .find("fn view_toggle_button")
            .unwrap_or_else(|| panic!("view toggle button is missing"));
        let toggle_window =
            &implementation[toggle_start..(toggle_start + 2500).min(implementation.len())];
        assert!(!toggle_window.contains(".py(px(3.))"));
        assert!(toggle_window.contains(".px(px(7.))"));
        assert!(!toggle_window.contains(".w(px(SEARCH_VIEW_ALL_SLOT_WIDTH_PX))"));
        assert!(toggle_window.contains(".justify_center()"));
        assert!(toggle_window.contains("\"view-toggle-"));
    }

    #[test]
    fn view_toggle_slot_is_static_with_view_all_and_view_less() {
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        // No width, margin, or opacity animation remains for the toggle slot.
        assert!(!implementation.contains("SEARCH_VIEW_ALL_SLOT_MAX_WIDTH_PX"));
        assert!(!implementation.contains("SEARCH_VIEW_ALL_ANIMATION_ID"));
        assert!(!implementation.contains("fn tracks_view_all_slot"));
        assert!(!implementation.contains("fn static_view_all_slot"));
        assert!(!implementation.contains("view_all_motion"));
        assert!(!implementation.contains("view_all_target_max_width"));
        assert!(!implementation.contains("view_all_from_max_width"));
        // Available actions use natural width. A section without more results
        // reserves no invisible toggle, keeping the result count at the edge.
        assert!(implementation.contains("fn view_toggle_slot"));
        assert!(implementation.contains("fn view_toggle_button"));
        assert!(implementation.contains("\"View all\""));
        assert!(implementation.contains("\"View less\""));
        assert!(implementation.contains("fn view_toggle_label"));
        assert!(implementation.contains("return_to_all_from_dedicated"));
        assert!(implementation.contains("is_preview_expandable"));
        assert!(implementation.contains("this.select_type(section"));
        assert!(!implementation.contains("toggle_preview_expanded"));
        assert!(!implementation.contains("is_preview_expanded"));
        assert!(!implementation.contains("expanded_preview"));
        assert!(!implementation.contains("render_tracks_expanded_inline"));
        assert!(!implementation.contains("render_cards_expanded_inline"));
        // The button uses natural width for View all and dedicated states.
        let slot_start = implementation
            .find("fn view_toggle_slot")
            .unwrap_or_else(|| panic!("view toggle slot is missing"));
        let slot_end = implementation[slot_start..]
            .find("pub(super) fn search_results_content_identity")
            .map(|offset| slot_start + offset)
            .unwrap_or_else(|| panic!("toggle slot end is missing"));
        let slot = &implementation[slot_start..slot_end];
        assert!(!slot.contains(".w(px(SEARCH_VIEW_ALL_SLOT_WIDTH_PX))"));
        assert!(slot.contains(".justify_center()"));
        assert!(!slot.contains(".justify_end()"));
        assert!(!slot.contains("with_animation"));
        assert!(!slot.contains(".max_w("));
        assert!(!slot.contains(".when(show_view_all"));
        assert!(slot.contains("all && !expandable"));
        assert!(!slot.contains(".opacity(0.)"));
    }

    #[test]
    fn preview_limits_toggle_between_bounded_and_full_inline_lists() {
        assert_eq!(
            preview_limit(ResultType::Tracks),
            super::SEARCH_TRACK_PREVIEW_LIMIT
        );
        assert_eq!(
            preview_limit(ResultType::Albums),
            super::SEARCH_CARD_PREVIEW_LIMIT
        );
        assert_eq!(
            preview_limit(ResultType::Artists),
            super::SEARCH_CARD_PREVIEW_LIMIT
        );
        assert_eq!(
            preview_limit(ResultType::Playlists),
            super::SEARCH_CARD_PREVIEW_LIMIT
        );
        assert_eq!(super::SEARCH_TRACK_PREVIEW_LIMIT, 5);
        assert_eq!(super::SEARCH_CARD_PREVIEW_LIMIT, 12);
        assert_eq!(view_toggle_label(true), "View all");
        assert_eq!(view_toggle_label(false), "View less");
        let implementation = include_str!("results_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("results view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        // The All preview always stays bounded. Full lists live only on the
        // dedicated virtualized tabs reached via select_type, so no large
        // non-virtualized inline draw can exhaust the glyph atlas.
        assert!(!implementation.contains("fn render_tracks_expanded_inline"));
        assert!(!implementation.contains("fn render_cards_expanded_inline"));
        assert!(!implementation.contains("SearchTrackRows::new"));
        assert!(!implementation.contains("track_row_slot"));
        assert!(!implementation.contains("render_card_grid_row"));
        assert!(implementation.contains("super::cards_view::render_cards"));
        assert!(implementation.contains("this.select_type(section"));
    }

    impl Render for ScrollOwnerProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let content = div()
                .w_full()
                .flex()
                .flex_col()
                .children((0..20).map(|_| div().h(px(50.)).flex_none()))
                .into_any_element();
            div().size_full().flex().child(search_results_scroll(
                content,
                false,
                &self.scroll,
                crate::browser_scroll::BrowserScrollState::new(),
            ))
        }
    }

    #[gpui::test]
    fn search_results_scroll_handle_owns_wheel_and_reset(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let scroll = ScrollHandle::new();
        let window = cx.add_window({
            let scroll = scroll.clone();
            move |_, _| ScrollOwnerProbe { scroll }
        });
        let mut cx = gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(20.), px(20.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(-120.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(scroll.offset().y < px(0.));

        scroll.set_offset(gpui::point(px(0.), px(0.)));
        assert_eq!(scroll.offset(), gpui::point(px(0.), px(0.)));
    }
}
