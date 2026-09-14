use gpui::{Context, ListAlignment, ListState, px};

use crate::browser_scroll::BrowserScrollState;
use crate::library::virtualization::{self, TrackListLayout};
use crate::music_ui::CardCarouselState;

use super::{
    detail::DetailState,
    models::{ResultType, Source},
    results_view::should_virtualize_results,
    view::SearchView,
};

pub(super) struct TrackListCache {
    state: ListState,
    browser_scroll: BrowserScrollState,
    shape: (usize, TrackListLayout),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CardGridLayout {
    pub(super) columns: usize,
    pub(super) settled_width: f32,
    pub(super) card_width: f32,
    pub(super) row_height: f32,
}

impl CardGridLayout {
    pub(super) const fn new(
        columns: usize,
        settled_width: f32,
        card_width: f32,
        row_height: f32,
    ) -> Self {
        Self {
            columns,
            settled_width,
            card_width,
            row_height,
        }
    }
}

pub(super) struct CardGridCache {
    pub(super) state: ListState,
    pub(super) browser_scroll: BrowserScrollState,
    pub(super) content_signature: u64,
    pub(super) layout: CardGridLayout,
    pub(super) row_count: usize,
    pub(super) card_count: usize,
}

pub(super) struct DiscoverFeedCache {
    pub(super) state: ListState,
    pub(super) browser_scroll: BrowserScrollState,
    pub(super) content_identity: String,
    pub(super) item_count: usize,
}

const MAX_TRACK_LIST_STATES: usize = 24;
const MAX_CARD_GRID_STATES: usize = 24;
const MAX_DISCOVER_FEED_STATES: usize = 6;
const CARD_GRID_OVERDRAW_ROWS: f32 = 2.;

const fn source_identity(source: Source) -> &'static str {
    match source {
        Source::All => "all",
        Source::Deezer => "deezer",
        Source::SoundCloud => "soundcloud",
    }
}

pub(super) fn search_card_grid_cache_key(
    account_scope: &str,
    source: Source,
    result_type: ResultType,
    query: &str,
    section_identity: &str,
) -> String {
    format!(
        "search-card-grid:{account_scope}:{}:{}:{query}:{section_identity}",
        source_identity(source),
        result_type.label(),
    )
}

pub(super) fn card_grid_cache_needs_reset(
    cache: &CardGridCache,
    content_signature: u64,
    card_count: usize,
    row_count: usize,
    layout: CardGridLayout,
) -> bool {
    cache.content_signature != content_signature
        || cache.card_count != card_count
        || cache.row_count != row_count
        || cache.layout != layout
}

fn preserve_card_grid_anchor(
    cache: &mut CardGridCache,
    row_count: usize,
    layout: CardGridLayout,
    row_height: gpui::Pixels,
) {
    let previous = cache.state.logical_scroll_top();
    let old_columns = cache.layout.columns.max(1);
    let old_row = previous.item_ix.min(cache.row_count.saturating_sub(1));
    let card_index = old_row
        .saturating_mul(old_columns)
        .min(cache.card_count.saturating_sub(1));
    let new_row = (card_index / layout.columns.max(1)).min(row_count);
    let offset_in_item = px(f32::from(previous.offset_in_item)
        .max(0.)
        .min(f32::from(row_height)));
    cache.state.reset_with_uniform_height(row_count, row_height);
    cache.state.scroll_to(gpui::ListOffset {
        item_ix: new_row,
        offset_in_item,
    });
    cache.browser_scroll.reset();
}

/// Reconcile a cached Discover feed with the current render inputs.
///
/// Rows measure their natural heights and the list state is created with
/// `measure_all`, so every row is measured in the first prepaint. GPUI re-arms
/// that measure pass whenever the layout width changes and re-measures all
/// rows within the same prepaint, so resizes need no handling here. Only
/// content changes act: the reset re-arms the measure pass and clears the
/// scroll for the fresh feed.
pub(super) fn update_discover_feed_cache(
    cache: &mut DiscoverFeedCache,
    content_identity: &str,
    item_count: usize,
) {
    let content_changed =
        cache.content_identity != content_identity || cache.item_count != item_count;
    if content_changed {
        cache.state.reset(item_count);
        cache.browser_scroll.reset();
        cache.content_identity = content_identity.to_owned();
        cache.item_count = item_count;
    }
}

impl SearchView {
    pub(super) fn track_list_state(
        &self,
        identity: &str,
        count: usize,
        layout: TrackListLayout,
    ) -> ListState {
        let mut states = self.track_list_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            if virtualization::list_state_needs_reset(Some(cache.shape), count, layout) {
                cache
                    .state
                    .reset_with_uniform_height(count, virtualization::row_height());
                cache.shape = (count, layout);
            }
            return cache.state.clone();
        }
        if states.len() >= MAX_TRACK_LIST_STATES
            && let Some(oldest) = states.keys().next().cloned()
        {
            states.remove(&oldest);
        }
        let state = ListState::new(count, ListAlignment::Top, virtualization::overdraw())
            .with_uniform_item_height(virtualization::row_height());
        states.insert(
            identity.to_owned(),
            TrackListCache {
                state: state.clone(),
                browser_scroll: BrowserScrollState::new(),
                shape: (count, layout),
            },
        );
        state
    }

    /// Cached measured-all list state for pages whose rows have natural
    /// heights, such as the artist section page. `measure_all` lays out every
    /// row in the first prepaint, and gpui re-arms that pass on every width
    /// change, so the scrollbar extent stays exact without pinning row
    /// heights.
    pub(super) fn measured_track_list_state(
        &self,
        identity: &str,
        count: usize,
        layout: TrackListLayout,
    ) -> ListState {
        let mut states = self.track_list_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            if virtualization::list_state_needs_reset(Some(cache.shape), count, layout) {
                cache.state.reset(count);
                cache.shape = (count, layout);
            }
            return cache.state.clone();
        }
        if states.len() >= MAX_TRACK_LIST_STATES
            && let Some(oldest) = states.keys().next().cloned()
        {
            states.remove(&oldest);
        }
        let state =
            ListState::new(count, ListAlignment::Top, virtualization::overdraw()).measure_all();
        states.insert(
            identity.to_owned(),
            TrackListCache {
                state: state.clone(),
                browser_scroll: BrowserScrollState::new(),
                shape: (count, layout),
            },
        );
        state
    }

    pub(super) fn track_list_browser_scroll(&self, identity: &str) -> BrowserScrollState {
        self.track_list_states
            .borrow()
            .get(identity)
            .map(|cache| cache.browser_scroll.clone())
            .unwrap_or_default()
    }

    pub(super) fn card_grid_cache_key(&self, section_identity: &str) -> String {
        if let Some(route) = &self.detail.route {
            return format!(
                "detail-card-grid:{}:{}:{}:{}:{}",
                self.account_scope,
                route.provider.label(),
                route.kind.label(),
                route.id,
                section_identity,
            );
        }
        search_card_grid_cache_key(
            &self.account_scope,
            self.state.source,
            self.state.result_type,
            &self.search_query,
            section_identity,
        )
    }

    pub(super) fn card_grid_state(
        &self,
        identity: &str,
        content_signature: u64,
        card_count: usize,
        layout: CardGridLayout,
    ) -> (ListState, BrowserScrollState) {
        let columns = layout.columns.max(1);
        let row_count = card_count.div_ceil(columns);
        let row_height = px(layout.row_height.max(1.));
        let mut states = self.card_grid_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            let content_changed =
                cache.content_signature != content_signature || cache.card_count != card_count;
            let layout_changed = cache.layout != layout || cache.row_count != row_count;
            if card_grid_cache_needs_reset(cache, content_signature, card_count, row_count, layout)
                && content_changed
            {
                cache.state.reset_with_uniform_height(row_count, row_height);
                cache.browser_scroll.reset();
            } else if card_grid_cache_needs_reset(
                cache,
                content_signature,
                card_count,
                row_count,
                layout,
            ) && layout_changed
            {
                preserve_card_grid_anchor(cache, row_count, layout, row_height);
            }
            cache.content_signature = content_signature;
            cache.layout = layout;
            cache.row_count = row_count;
            cache.card_count = card_count;
            return (cache.state.clone(), cache.browser_scroll.clone());
        }
        if states.len() >= MAX_CARD_GRID_STATES
            && let Some(oldest) = states.keys().next().cloned()
        {
            states.remove(&oldest);
        }
        let state = ListState::new(
            row_count,
            ListAlignment::Top,
            px(layout.row_height.max(1.) * CARD_GRID_OVERDRAW_ROWS),
        )
        .with_uniform_item_height(row_height);
        let browser_scroll = BrowserScrollState::new();
        states.insert(
            identity.to_owned(),
            CardGridCache {
                state: state.clone(),
                browser_scroll: browser_scroll.clone(),
                content_signature,
                layout,
                row_count,
                card_count,
            },
        );
        (state, browser_scroll)
    }

    pub(super) fn discover_feed_state(
        &self,
        identity: &str,
        content_identity: &str,
        item_count: usize,
        overdraw: gpui::Pixels,
    ) -> (ListState, BrowserScrollState) {
        let mut states = self.discover_feed_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            update_discover_feed_cache(cache, content_identity, item_count);
            return (cache.state.clone(), cache.browser_scroll.clone());
        }
        if states.len() >= MAX_DISCOVER_FEED_STATES
            && let Some(oldest) = states.keys().next().cloned()
        {
            states.remove(&oldest);
        }
        // measure_all lays out every row in the first prepaint, so the
        // scrollbar is exact from the first frame whatever natural heights
        // the rows measure. GPUI re-arms the measure pass on every width
        // change, re-measuring all rows at the new width within the same
        // prepaint, so the extent never collapses during a resize.
        let state = ListState::new(item_count, ListAlignment::Top, overdraw)
            .measure_all()
            .width_independent_heights();
        let browser_scroll = BrowserScrollState::new();
        states.insert(
            identity.to_owned(),
            DiscoverFeedCache {
                state: state.clone(),
                browser_scroll: browser_scroll.clone(),
                content_identity: content_identity.to_owned(),
                item_count,
            },
        );
        (state, browser_scroll)
    }

    fn discover_feed_cache_identity(&self) -> String {
        if self.discover.channel_open() {
            format!(
                "discover-feed:{:?}:channel:{}",
                self.state.source,
                self.discover.channel().slug
            )
        } else {
            format!("discover-feed:{:?}:home", self.state.source)
        }
    }

    fn discover_feed_scroll_offset(&self) -> Option<gpui::Point<gpui::Pixels>> {
        let identity = self.discover_feed_cache_identity();
        self.discover_feed_states
            .borrow()
            .get(&identity)
            .map(|cache| cache.state.scroll_px_offset_for_scrollbar())
    }

    pub(super) fn clear_discover_feed_states(&mut self) {
        self.discover_feed_states.get_mut().clear();
    }

    fn card_grid_scroll_offset(&self) -> Option<gpui::Point<gpui::Pixels>> {
        let identity = self.card_grid_cache_key(self.state.result_type.label());
        let states = self.card_grid_states.borrow();
        let cache = states.get(&identity)?;
        Some(
            crate::browser_scroll::FixedListScrollHandle::new(
                cache.state.clone(),
                cache.row_count,
                px(cache.layout.row_height),
            )
            .scroll_offset(),
        )
    }

    pub(super) fn dedicated_card_results_active(&self) -> bool {
        self.detail.route.is_none()
            && matches!(self.state.state, super::models::ResultState::Results)
            && super::results_view::is_dedicated_card_result(self.state.result_type)
            && self.state.groups.count(self.state.result_type) > 0
    }

    pub(super) fn active_vertical_scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
        // Discover owns its vertical ListState while the feed itself is the
        // active view. Once a detail route exists, use its own scroll handle
        // so nested detail history is preserved.
        if self.detail.route.is_none()
            && self.search_query.is_empty()
            && matches!(self.state.state, super::models::ResultState::Initial)
            && let Some(offset) = self.discover_feed_scroll_offset()
        {
            return offset;
        }
        if self.dedicated_card_results_active() {
            self.card_grid_scroll_offset()
                .unwrap_or_else(|| self.scroll.offset())
        } else {
            self.scroll.offset()
        }
    }

    pub(super) fn restore_card_grid_scroll(&self, offset: gpui::Point<gpui::Pixels>) -> bool {
        let identity = self.card_grid_cache_key(self.state.result_type.label());
        let states = self.card_grid_states.borrow();
        let Some(cache) = states.get(&identity) else {
            return false;
        };
        crate::browser_scroll::FixedListScrollHandle::new(
            cache.state.clone(),
            cache.row_count,
            px(cache.layout.row_height),
        )
        .set_scroll_offset(offset);
        true
    }

    pub(super) fn clear_track_list_states(&mut self) {
        self.track_list_states.get_mut().clear();
    }

    pub(super) fn clear_card_grid_states(&mut self) {
        self.card_grid_states.get_mut().clear();
    }

    pub(super) fn card_scroll_handle(&self, id: &str) -> CardCarouselState {
        self.card_scroll_handles
            .borrow_mut()
            .entry(id.to_owned())
            .or_insert_with(CardCarouselState::new)
            .clone()
    }

    pub(super) fn uses_virtualized_scroll(&self, cx: &Context<Self>) -> bool {
        if self.should_show_discover(cx) {
            // Discover owns a variable-height virtualized feed even before a
            // provider has returned its first section.
            return true;
        }
        if !matches!(self.detail.state, DetailState::Closed) {
            return matches!(
                &self.detail.state,
                DetailState::Loading
                    if self
                        .detail
                        .route
                        .as_ref()
                        .is_some_and(|route| route.kind == ResultType::Artists)
            ) || matches!(
                &self.detail.state,
                DetailState::Results(page)
                    if (page.artist.is_none() && !page.tracks.is_empty())
                        || matches!(
                            self.detail.expanded_artist_section,
                            Some(super::detail::ArtistSection::PopularTracks)
                        ) && page
                            .artist
                            .as_ref()
                            .is_some_and(|artist| !artist.popular_tracks.is_empty())
                        || self.detail.expanded_artist_section.is_some()
                            && page.artist.is_some()
            );
        }
        should_virtualize_results(
            self.state.result_type,
            &self.state.state,
            &self.state.groups,
        )
    }
}
