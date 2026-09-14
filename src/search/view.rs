use std::{cell::RefCell, collections::HashMap, sync::Arc, time::Duration};

use gpui::{
    App, AppContext, Context, DragMoveEvent, Entity, FocusHandle, Focusable, ListAlignment,
    ListState, ScrollHandle, Window, point, px,
};
use gpui_component::input::{InputEvent, InputState};
use tokio::{runtime::Runtime, task::AbortHandle};

use crate::{
    browser_scroll::BrowserScrollState,
    downloads::DownloadModel,
    library::{FavoriteKey, FavoriteKind, FavoriteState, LibraryView},
    motion::{ResponsiveModeMotion, SegmentedSelectorMotion},
    music_ui::{CardCarouselState, CardGridMotion, ResizeSettledColumns, ResizeSettledTarget},
    playback::PlaybackModel,
    playing_indicator::PlayingSnapshot,
    settings::{AccountState, SettingsView},
};

use crate::library::virtualization::{self, TrackListLayout};

use super::{
    SEARCH_PLACEHOLDER,
    album_info_cache::{AlbumInfoCacheKey, AlbumInfoPrefetch, info_for_detail_state},
    client::SearchClient,
    credential::DeezerArl,
    detail::{
        DetailNavigation, DetailState, should_apply_scheduled_detail_scroll_reset,
        should_reset_detail_scroll,
    },
    discover::{self, DiscoverState},
    models::{Card, Provider, ResultState, ResultType, SearchJob, SearchState, Source, Track},
    results_view::{search_results_content_identity, should_virtualize_results},
    suggestions::{SUGGESTION_DEBOUNCE, SuggestionRow, SuggestionState},
};

pub(crate) struct SearchView {
    pub(crate) input: Entity<InputState>,
    pub(super) runtime: Arc<Runtime>,
    pub(super) client: Result<SearchClient, super::models::ProviderError>,
    pub(super) state: SearchState,
    pub(super) account: Entity<AccountState>,
    settings: Entity<SettingsView>,
    pub(super) library: Entity<LibraryView>,
    pub(crate) favorites: Entity<FavoriteState>,
    pub(crate) playback: Entity<PlaybackModel>,
    pub(super) downloads: Entity<DownloadModel>,
    account_scope: String,
    pub(super) detail: DetailNavigation,
    pub(super) type_tab_focus: Vec<FocusHandle>,
    pub(super) type_tab_motion: SegmentedSelectorMotion,
    pub(super) result_count_responsive: ResponsiveModeMotion,
    pub(super) last_result_count_label: String,
    pub(super) scroll: ScrollHandle,
    pub(super) browser_scroll: BrowserScrollState,
    /// Outer scroll offsets kept per result tab. Tab switches restore the
    /// returning tab's viewport instead of resetting shared scroll state.
    result_scroll_offsets: HashMap<ResultType, gpui::Point<gpui::Pixels>>,
    /// Entrance animation key armed only when fresh results complete. Tab
    /// switches re-present cached results and must not replay it.
    pub(super) results_entrance_key: Option<String>,
    search_active: bool,
    pub(super) playing: PlayingSnapshot,
    playlist_update_revision: u64,
    soundcloud_playlist_update_revision: u64,
    playlist_content_revision: u64,
    playlist_delete_revision: u64,
    soundcloud_playlist_delete_revision: u64,
    playlist_remove_revision: u64,
    playlist_reorder_provider: Option<Provider>,
    playlist_reorder_revision: u64,
    playlist_reorder_snapshot: Option<PlaylistReorderSnapshot>,
    playlist_drag_scroll: Option<crate::library::playlist_drag::PlaylistDragAutoScroll>,
    playlist_drag_scroll_running: bool,
    pending_forward_detail_scroll_reset: Option<u64>,
    card_scroll_handles: RefCell<HashMap<String, CardCarouselState>>,
    pub(super) card_columns: ResizeSettledColumns,
    pub(super) card_grid_motion: CardGridMotion,
    track_list_states: RefCell<HashMap<String, TrackListCache>>,
    card_grid_states: RefCell<HashMap<String, CardGridCache>>,
    discover_feed_states: RefCell<HashMap<String, DiscoverFeedCache>>,
    search_query: String,
    favorite_actions: HashMap<FavoriteKey, FavoriteAction>,
    album_info_prefetch: AlbumInfoPrefetch,
    suggestions: SuggestionState,
    pub(super) discover: DiscoverState,
    search_request: Option<ActiveSearchRequest>,
    suggestion_request: Option<ActiveRequest>,
    discover_requests: HashMap<Provider, ActiveRequest>,
    discover_channel_request: Option<ActiveRequest>,
    smart_mix_enrichment_request: Option<ActiveRequest>,
    smart_mix_enrichment_started_generation: Option<u64>,
    smart_mix_enrichment_was_cancelled: bool,
    next_request_id: u64,
}

struct ActiveRequest {
    generation: u64,
    id: u64,
    abort: AbortHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchRequestKey {
    source: Source,
    result_type: ResultType,
    query: String,
}

struct ActiveSearchRequest {
    generation: u64,
    id: u64,
    aborts: Vec<AbortHandle>,
    pending_batches: usize,
    key: SearchRequestKey,
}

impl Drop for SearchView {
    fn drop(&mut self) {
        self.cancel_search_request();
        self.cancel_suggestion_request();
        for request in self.discover_requests.drain().map(|(_, request)| request) {
            request.abort.abort();
        }
        self.cancel_discover_channel_request();
        self.cancel_smart_mix_enrichment_request();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchNavigationBackTarget {
    Detail,
    DiscoverChannel,
}

fn search_navigation_back_target(
    detail_open: bool,
    discover_channel_open: bool,
) -> SearchNavigationBackTarget {
    if detail_open || !discover_channel_open {
        SearchNavigationBackTarget::Detail
    } else {
        SearchNavigationBackTarget::DiscoverChannel
    }
}

fn should_close_discover_channel_before_detail(
    preserve_discover_channel: bool,
    discover_channel_open: bool,
) -> bool {
    discover_channel_open && !preserve_discover_channel
}

fn search_results_root_visible(
    search_query: &str,
    state: &ResultState,
    detail_open: bool,
    discover_channel_open: bool,
) -> bool {
    !detail_open
        && !discover_channel_open
        && !search_query.trim().is_empty()
        && !matches!(state, ResultState::Initial)
}

fn discover_provider_has_credentials(
    provider: Provider,
    deezer_available: bool,
    soundcloud_available: bool,
) -> bool {
    match provider {
        Provider::Deezer => deezer_available,
        Provider::SoundCloud => soundcloud_available,
    }
}

fn all_search_missing_accounts(source: Source, soundcloud_available: bool) -> Vec<Provider> {
    // Deezer search runs anonymously. Only SoundCloud account state can
    // leave an All search missing one of its providers.
    if source != Source::All || soundcloud_available {
        return Vec::new();
    }
    vec![Provider::SoundCloud]
}

struct PlaylistReorderSnapshot {
    account_scope: String,
    provider: Provider,
    playlist_id: String,
    view_id: u64,
    tracks: Vec<Track>,
}

struct TrackListCache {
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

struct CardGridCache {
    state: ListState,
    browser_scroll: BrowserScrollState,
    content_signature: u64,
    layout: CardGridLayout,
    row_count: usize,
    card_count: usize,
}

struct DiscoverFeedCache {
    state: ListState,
    browser_scroll: BrowserScrollState,
    content_identity: String,
    item_count: usize,
}

struct FavoriteAction {
    expected: bool,
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

fn search_card_grid_cache_key(
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

fn card_grid_cache_needs_reset(
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
fn update_discover_feed_cache(
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
    pub(crate) fn new(
        account: Entity<AccountState>,
        settings: Entity<SettingsView>,
        library: Entity<LibraryView>,
        favorites: Entity<FavoriteState>,
        runtime: Arc<Runtime>,
        playback: Entity<PlaybackModel>,
        downloads: Entity<DownloadModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let account_scope = account.read(cx).library_scope();
        // The toolbar paints the responsive placeholder while blurred. The
        // native placeholder remains available while focused so the caret is
        // painted above the hint instead of behind an overlay.
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(""));
        cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::Change => {
                this.cancel_discover_requests();
                this.cancel_discover_channel_request();
                this.discover.close_channel();
                this.suggestions.set_focused(true);
                this.refresh_suggestions(cx);
            }
            InputEvent::Focus => {
                this.input.update(cx, |input, cx| {
                    input.set_placeholder(SEARCH_PLACEHOLDER, window, cx);
                });
                this.suggestions.set_focused(true);
                this.refresh_suggestions(cx);
            }
            InputEvent::Blur => {
                this.input.update(cx, |input, cx| {
                    input.set_placeholder("", window, cx);
                });
                this.cancel_suggestion_request();
                this.suggestions.set_focused(false);
                cx.notify();
            }
            InputEvent::PressEnter { .. } if this.search_active => {
                if let Some(query) = this.selected_suggestion_query(cx) {
                    this.replace_query(query, window, cx);
                }
                this.submit(cx);
            }
            _ => {}
        })
        .detach();
        cx.observe(&library, |this, _, cx| {
            this.sync_favorite_actions(cx);
        })
        .detach();
        cx.observe(&favorites, |this, _, cx| {
            this.sync_favorite_actions(cx);
        })
        .detach();
        // Rows re-render when the current track, its playing state, or the
        // explicit-block preference moves; the position poll alone changes
        // nothing here.
        cx.observe(&playback, |this, playback, cx| {
            let snapshot = PlayingSnapshot::from_playback(&playback.read(cx).state);
            if this.playing != snapshot {
                this.playing = snapshot;
                cx.notify();
            }
        })
        .detach();
        let playing = PlayingSnapshot::from_playback(&playback.read(cx).state);
        let saved_settings = settings.read(cx).saved();
        let suggestions = SuggestionState::new(
            saved_settings.soundcloud_search_suggestions,
            saved_settings.search_history.clone(),
        );
        Self {
            input,
            runtime,
            client: SearchClient::new(),
            state: SearchState::default(),
            account,
            settings,
            library,
            favorites,
            playback,
            downloads,
            account_scope: account_scope.clone(),
            detail: DetailNavigation::default(),
            type_tab_focus: (0..5).map(|_| cx.focus_handle()).collect(),
            type_tab_motion: SegmentedSelectorMotion::default(),
            result_count_responsive: ResponsiveModeMotion::default(),
            last_result_count_label: String::new(),
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            result_scroll_offsets: HashMap::new(),
            results_entrance_key: None,
            search_active: true,
            playing,
            playlist_update_revision: 0,
            soundcloud_playlist_update_revision: 0,
            playlist_content_revision: 0,
            playlist_delete_revision: 0,
            soundcloud_playlist_delete_revision: 0,
            playlist_remove_revision: 0,
            playlist_reorder_provider: None,
            playlist_reorder_revision: 0,
            playlist_reorder_snapshot: None,
            playlist_drag_scroll: None,
            playlist_drag_scroll_running: false,
            pending_forward_detail_scroll_reset: None,
            card_scroll_handles: RefCell::new(HashMap::new()),
            card_columns: ResizeSettledColumns::new(),
            card_grid_motion: CardGridMotion::default(),
            track_list_states: RefCell::new(HashMap::new()),
            card_grid_states: RefCell::new(HashMap::new()),
            discover_feed_states: RefCell::new(HashMap::new()),
            search_query: String::new(),
            favorite_actions: HashMap::new(),
            album_info_prefetch: AlbumInfoPrefetch::default(),
            suggestions,
            discover: DiscoverState::new(account_scope.clone()),
            search_request: None,
            suggestion_request: None,
            discover_requests: HashMap::new(),
            discover_channel_request: None,
            smart_mix_enrichment_request: None,
            smart_mix_enrichment_started_generation: None,
            smart_mix_enrichment_was_cancelled: false,
            next_request_id: 0,
        }
    }

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

    pub(super) fn update_playlist_drag_autoscroll(
        &mut self,
        event: &DragMoveEvent<crate::library::playlist_drag::PlaylistTrackDrag>,
        scroll: crate::browser_scroll::FixedListScrollHandle,
        cx: &mut Context<Self>,
    ) {
        self.playlist_drag_scroll =
            Some(crate::library::playlist_drag::PlaylistDragAutoScroll::from_event(event, scroll));
        self.run_playlist_drag_autoscroll(cx);
    }

    fn step_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.playlist_drag_scroll.as_ref() else {
            self.playlist_drag_scroll_running = false;
            return false;
        };
        if !cx.has_active_drag() {
            self.playlist_drag_scroll = None;
            self.playlist_drag_scroll_running = false;
            cx.notify();
            return false;
        }
        drag.step();
        cx.notify();
        true
    }

    fn run_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.playlist_drag_scroll_running {
            return;
        }
        self.playlist_drag_scroll_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor
                    .timer(crate::library::playlist_drag::DRAG_SCROLL_TICK)
                    .await;
                let alive = this
                    .update(cx, |view, cx| view.step_playlist_drag_autoscroll(cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
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

    fn clear_discover_feed_states(&mut self) {
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

    fn dedicated_card_results_active(&self) -> bool {
        self.detail.route.is_none()
            && matches!(self.state.state, super::models::ResultState::Results)
            && super::results_view::is_dedicated_card_result(self.state.result_type)
            && self.state.groups.count(self.state.result_type) > 0
    }

    fn active_vertical_scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
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

    fn restore_card_grid_scroll(&self, offset: gpui::Point<gpui::Pixels>) -> bool {
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

    fn clear_track_list_states(&mut self) {
        self.track_list_states.get_mut().clear();
    }

    fn clear_card_grid_states(&mut self) {
        self.card_grid_states.get_mut().clear();
    }

    fn sync_favorite_actions(&mut self, cx: &mut Context<Self>) {
        let completed = {
            let favorites = self.favorites.read(cx);
            self.favorite_actions
                .iter()
                .filter(|&(key, _)| !favorites.pending(key))
                .map(|(key, action)| (key.clone(), action.expected))
                .collect::<Vec<_>>()
        };
        if completed.is_empty() {
            return;
        }
        for (key, _) in completed {
            self.favorite_actions.remove(&key);
        }
        cx.notify();
    }

    fn begin_favorite_action(
        &mut self,
        key: FavoriteKey,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        let (expected, pending) = {
            let favorites = self.favorites.read(cx);
            (
                !favorites.favorite(&key).unwrap_or(known_favorite),
                favorites.pending(&key),
            )
        };
        if pending {
            return;
        }
        self.favorite_actions
            .insert(key.clone(), FavoriteAction { expected });
        self.library.update(cx, |library, cx| {
            library.toggle_favorite(key, known_favorite, cx);
        });
        self.sync_favorite_actions(cx);
    }

    fn reset_detail_scroll(&mut self) {
        let top = point(px(0.), px(0.));
        self.browser_scroll.reset();
        self.scroll.set_offset(top);
        self.scroll = ScrollHandle::new();
        self.scroll.set_offset(top);
    }

    fn schedule_forward_detail_scroll_reset(&mut self, generation: u64, cx: &mut Context<Self>) {
        let top = point(px(0.), px(0.));
        self.scroll.set_offset(top);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            this.update(cx, |this, cx| {
                if should_apply_scheduled_detail_scroll_reset(
                    this.pending_forward_detail_scroll_reset,
                    generation,
                ) {
                    this.scroll.set_offset(top);
                    this.pending_forward_detail_scroll_reset = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn restore_detail_scroll(&mut self, offset: gpui::Point<gpui::Pixels>) {
        if self.dedicated_card_results_active() && self.restore_card_grid_scroll(offset) {
            self.browser_scroll.reset();
            return;
        }
        self.browser_scroll.reset();
        self.scroll.set_offset(offset);
        self.scroll = ScrollHandle::new();
        self.scroll.set_offset(offset);
    }

    pub(super) fn card_scroll_handle(&self, id: &str) -> CardCarouselState {
        self.card_scroll_handles
            .borrow_mut()
            .entry(id.to_owned())
            .or_insert_with(CardCarouselState::new)
            .clone()
    }

    pub(crate) fn set_search_active(&mut self, active: bool) {
        self.search_active = active;
        if !active {
            self.cancel_suggestion_request();
            self.suggestions.set_focused(false);
        }
    }

    fn cancel_search_request(&mut self) {
        if let Some(request) = self.search_request.take() {
            for abort in request.aborts {
                abort.abort();
            }
        }
    }

    fn next_request_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.next_request_id
    }

    fn search_request_is_active(
        &self,
        source: Source,
        result_type: ResultType,
        query: &str,
    ) -> bool {
        self.search_request.as_ref().is_some_and(|request| {
            request.key
                == SearchRequestKey {
                    source,
                    result_type,
                    query: query.trim().to_owned(),
                }
        })
    }

    fn finish_search_request_batch(&mut self, generation: u64, id: u64) -> Option<bool> {
        let request = self
            .search_request
            .as_mut()
            .filter(|request| request.generation == generation && request.id == id)?;
        request.pending_batches = request.pending_batches.saturating_sub(1);
        let final_batch = request.pending_batches == 0;
        if final_batch {
            self.search_request = None;
        }
        Some(final_batch)
    }

    fn cancel_suggestion_request(&mut self) {
        if let Some(request) = self.suggestion_request.take() {
            request.abort.abort();
        }
    }

    fn clear_suggestion_request(&mut self, generation: u64, id: u64) {
        if self
            .suggestion_request
            .as_ref()
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.suggestion_request = None;
        }
    }

    fn cancel_discover_provider_request(&mut self, provider: Provider) {
        if let Some(request) = self.discover_requests.remove(&provider) {
            request.abort.abort();
        }
        self.discover.cancel_loading(provider);
    }

    fn clear_discover_provider_request(&mut self, provider: Provider, generation: u64, id: u64) {
        if self
            .discover_requests
            .get(&provider)
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.discover_requests.remove(&provider);
        }
    }

    fn cancel_discover_requests(&mut self) {
        for request in self.discover_requests.drain().map(|(_, request)| request) {
            request.abort.abort();
        }
        self.discover.cancel_loading(Provider::Deezer);
        self.discover.cancel_loading(Provider::SoundCloud);
        self.cancel_smart_mix_enrichment_request();
    }

    fn cancel_discover_channel_request(&mut self) {
        if let Some(request) = self.discover_channel_request.take() {
            request.abort.abort();
        }
    }

    fn clear_discover_channel_request(&mut self, generation: u64, id: u64) {
        if self
            .discover_channel_request
            .as_ref()
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.discover_channel_request = None;
        }
    }

    fn cancel_smart_mix_enrichment_request(&mut self) {
        if let Some(request) = self.smart_mix_enrichment_request.take() {
            request.abort.abort();
            self.smart_mix_enrichment_was_cancelled = true;
        }
    }

    fn clear_smart_mix_enrichment_request(&mut self, generation: u64, id: u64) {
        if self
            .smart_mix_enrichment_request
            .as_ref()
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.smart_mix_enrichment_request = None;
        }
    }

    pub(crate) fn suggestion_rows(&self, cx: &App) -> Vec<SuggestionRow> {
        let query = self.input.read(cx).value();
        self.suggestions.rows(query.as_ref())
    }

    pub(crate) fn suggestions_visible(&self, cx: &App) -> bool {
        let query = self.input.read(cx).value();
        self.search_active && self.suggestions.visible(query.as_ref())
    }

    pub(crate) fn selected_suggestion(&self) -> Option<usize> {
        self.suggestions.selected()
    }

    pub(crate) fn select_relative_suggestion(
        &mut self,
        direction: i32,
        cx: &mut Context<Self>,
    ) -> bool {
        let query = self.input.read(cx).value().to_string();
        let changed = self.suggestions.select_relative(&query, direction);
        if changed {
            cx.notify();
        }
        changed
    }

    pub(crate) fn dismiss_suggestions(&mut self, cx: &mut Context<Self>) {
        self.cancel_suggestion_request();
        self.suggestions.set_focused(false);
        cx.notify();
    }

    pub(crate) fn choose_suggestion(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_query(query, window, cx);
        self.submit(cx);
    }

    pub(crate) fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_discover_requests();
        self.cancel_discover_channel_request();
        self.discover.close_channel();
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.input.focus_handle(cx).focus(window, cx);
        self.suggestions.set_focused(true);
        self.refresh_suggestions(cx);
    }

    pub(crate) fn return_to_discover_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.submit(cx);
        self.suggestions.set_focused(false);
        cx.notify();
    }

    pub(crate) fn remove_search_history(&mut self, query: &str, cx: &mut Context<Self>) {
        if !self.suggestions.remove_history(query) {
            return;
        }
        self.persist_search_history(cx);
        cx.notify();
    }

    pub(crate) fn remove_selected_search_history(&mut self, cx: &mut Context<Self>) -> bool {
        let query = self.input.read(cx).value().to_string();
        let Some(history_query) = self.suggestions.selected_history_query(&query) else {
            return false;
        };
        self.remove_search_history(&history_query, cx);
        true
    }

    pub(crate) fn set_soundcloud_suggestions_enabled(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if !enabled {
            self.cancel_suggestion_request();
        }
        if self.suggestions.set_enabled(enabled) && enabled && self.suggestions.is_focused() {
            self.refresh_suggestions(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn apply_imported_preferences(
        &mut self,
        source: Source,
        result_type: ResultType,
        suggestions_enabled: bool,
        history: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.suggestions.replace_history(history) {
            self.cancel_suggestion_request();
        }
        self.set_soundcloud_suggestions_enabled(suggestions_enabled, cx);
        if self.source() != source {
            self.select_source(source, cx);
        }
        self.restore_result_type(result_type, cx);
        cx.notify();
    }

    fn selected_suggestion_query(&self, cx: &Context<Self>) -> Option<String> {
        let query = self.input.read(cx).value();
        self.suggestions.selected_query(query.as_ref())
    }

    fn replace_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_suggestion_request();
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.suggestions.invalidate_request();
    }

    fn persist_search_history(&mut self, cx: &mut Context<Self>) {
        let history = self.suggestions.history().to_vec();
        self.settings.update(cx, |settings, _| {
            settings.persist_search_history(history);
        });
    }

    fn refresh_suggestions(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().trim().to_owned();
        if !query.is_empty()
            && self.suggestions.enabled()
            && self.suggestions.is_focused()
            && self
                .suggestion_request
                .as_ref()
                .is_some_and(|_| self.suggestions.request_matches(&query))
        {
            return;
        }
        self.cancel_suggestion_request();
        let generation = self.suggestions.begin_request(&query);
        cx.notify();
        if query.is_empty() || !self.suggestions.enabled() || !self.suggestions.is_focused() {
            return;
        }
        let Ok(client) = self.client.clone() else {
            return;
        };
        let task_query = query.clone();
        let task = self.runtime.spawn(async move {
            tokio::time::sleep(SUGGESTION_DEBOUNCE).await;
            client.soundcloud_suggestions(&task_query).await
        });
        let request_id = self.next_request_id();
        self.suggestion_request = Some(ActiveRequest {
            generation,
            id: request_id,
            abort: task.abort_handle(),
        });
        cx.spawn(async move |this, cx| {
            let remote = task.await.ok().and_then(Result::ok).unwrap_or_default();
            this.update(cx, |this, cx| {
                this.clear_suggestion_request(generation, request_id);
                if this
                    .suggestions
                    .complete_request(generation, &query, remote)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn account_scope_changed(&mut self, scope: String, cx: &mut Context<Self>) {
        if self.account_scope != scope {
            self.cancel_search_request();
            self.cancel_suggestion_request();
            if let Ok(client) = &self.client {
                client.clear_deezer_sessions();
            }
            self.suggestions.invalidate_request();
            self.cancel_discover_requests();
            self.cancel_discover_channel_request();
            self.smart_mix_enrichment_started_generation = None;
            self.smart_mix_enrichment_was_cancelled = false;
            self.clear_track_list_states();
            self.clear_card_grid_states();
            self.clear_discover_feed_states();
            self.card_scroll_handles.get_mut().clear();
            self.result_scroll_offsets.clear();
            self.results_entrance_key = None;
            self.favorite_actions.clear();
            self.album_info_prefetch.clear();
            self.pending_forward_detail_scroll_reset = None;
            self.playlist_reorder_snapshot = None;
            self.reset_detail_scroll();
            self.account_scope = scope;
            self.discover
                .reset_account_scope(self.account_scope.clone());
            self.search_query.clear();
            self.state.account_scope_changed();
            self.detail.reset();
            cx.notify();
        }
    }

    pub(super) fn should_show_discover(&self, _cx: &Context<Self>) -> bool {
        !self.detail_open()
            && self.search_query.is_empty()
            && matches!(self.state.state, ResultState::Initial)
    }

    pub(super) fn ensure_discover(&mut self, cx: &mut Context<Self>) {
        if !self.should_show_discover(cx) {
            self.cancel_discover_requests();
            return;
        }
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        for provider in self.state.source.providers() {
            if !discover_provider_has_credentials(
                *provider,
                deezer_arl.is_some(),
                soundcloud_token.is_some(),
            ) {
                self.cancel_discover_provider_request(*provider);
                self.discover.mark_account_required(*provider);
                continue;
            }
            let Some((generation, account_scope)) = self.discover.start(*provider) else {
                continue;
            };
            let client = self.client.clone();
            let arl = deezer_arl.clone();
            let enrichment_arl = arl.clone();
            let enrichment_scope = account_scope.clone();
            let token = soundcloud_token.clone();
            let provider = *provider;
            let task = self.runtime.spawn(async move {
                match client {
                    Ok(client) => discover::load(provider, &client, arl, token).await,
                    Err(error) => Err(error.message),
                }
            });
            let request_id = self.next_request_id();
            self.discover_requests.insert(
                provider,
                ActiveRequest {
                    generation,
                    id: request_id,
                    abort: task.abort_handle(),
                },
            );
            cx.spawn(async move |this, cx| {
                let result = task
                    .await
                    .unwrap_or_else(|_| Err("Discover request failed".to_owned()));
                this.update(cx, |this, cx| {
                    this.clear_discover_provider_request(provider, generation, request_id);
                    let accepted =
                        this.discover
                            .complete(provider, generation, &account_scope, result);
                    if accepted {
                        if provider == Provider::Deezer {
                            this.start_smart_mix_title_enrichment(
                                generation,
                                enrichment_scope.clone(),
                                enrichment_arl.clone(),
                                cx,
                            );
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        if self.state.source.providers().contains(&Provider::Deezer)
            && let Some(generation) = self.discover.ready_generation(Provider::Deezer)
            && self.smart_mix_enrichment_request.is_none()
            && (self.smart_mix_enrichment_started_generation != Some(generation)
                || self.smart_mix_enrichment_was_cancelled)
        {
            self.start_smart_mix_title_enrichment(
                generation,
                self.account_scope.clone(),
                deezer_arl,
                cx,
            );
        }
    }

    fn start_smart_mix_title_enrichment(
        &mut self,
        generation: u64,
        account_scope: String,
        arl: Option<DeezerArl>,
        cx: &mut Context<Self>,
    ) {
        let Some(arl) = arl else {
            return;
        };
        let ids = self.discover.unresolved_smart_mix_ids();
        if ids.is_empty() {
            return;
        }
        self.cancel_smart_mix_enrichment_request();
        self.smart_mix_enrichment_started_generation = Some(generation);
        self.smart_mix_enrichment_was_cancelled = false;
        let Ok(client) = self.client.clone() else {
            return;
        };
        let task = self
            .runtime
            .spawn(async move { discover::enrich_smart_mix_titles(&client, arl, ids).await });
        let request_id = self.next_request_id();
        self.smart_mix_enrichment_request = Some(ActiveRequest {
            generation,
            id: request_id,
            abort: task.abort_handle(),
        });
        cx.spawn(async move |this, cx| {
            let titles = task.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.clear_smart_mix_enrichment_request(generation, request_id);
                this.smart_mix_enrichment_was_cancelled = false;
                if this.discover.apply_enriched_smart_mix_titles(
                    generation,
                    &account_scope,
                    &titles,
                ) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn retry_discover(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.discover.retry(provider) {
            self.ensure_discover(cx);
            cx.notify();
        }
    }

    pub(crate) fn source(&self) -> Source {
        self.state.source
    }

    pub(crate) fn result_type(&self) -> ResultType {
        self.state.result_type
    }

    pub(super) fn search_query(&self) -> &str {
        &self.search_query
    }

    pub(super) fn return_to_all_from_dedicated(
        &mut self,
        _result_type: ResultType,
        cx: &mut Context<Self>,
    ) {
        self.select_type(ResultType::All, cx);
    }

    pub(crate) fn restore_result_type(&mut self, result_type: ResultType, cx: &mut Context<Self>) {
        if self.state.result_type != result_type {
            self.select_type(result_type, cx);
        }
    }

    pub(crate) fn detail_open(&self) -> bool {
        !matches!(self.detail.state, DetailState::Closed)
    }

    pub(crate) fn open_account_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.open_settings(window, cx));
    }

    pub(crate) fn search_results_root_open(&self) -> bool {
        search_results_root_visible(
            &self.search_query,
            &self.state.state,
            self.detail_open(),
            self.discover_channel_open(),
        )
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

    pub(crate) fn select_source(&mut self, source: Source, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if self.search_request_is_active(source, self.state.result_type, &query) {
            self.cancel_suggestion_request();
            self.suggestions.set_focused(false);
            cx.notify();
            return;
        }
        self.cancel_search_request();
        self.cancel_suggestion_request();
        self.cancel_discover_requests();
        self.cancel_discover_channel_request();
        self.discover.close_channel();
        self.clear_track_list_states();
        self.result_scroll_offsets.clear();
        self.results_entrance_key = None;
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        self.detail.reset();
        self.search_query = query.trim().to_owned();
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        let job = self.state.select_source(source, &query);
        self.run(job, deezer_arl, soundcloud_token, cx);
    }

    pub(super) fn select_type(&mut self, result_type: ResultType, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if self.search_request_is_active(self.state.source, result_type, &query) {
            self.cancel_suggestion_request();
            self.suggestions.set_focused(false);
            cx.notify();
            return;
        }
        self.cancel_search_request();
        self.cancel_suggestion_request();
        self.cancel_discover_requests();
        self.cancel_discover_channel_request();
        self.discover.close_channel();
        // Tab switches keep per-tab view state. Track lists and card grids
        // are keyed by source, type, query, and content, so reuse them
        // instead of clearing. Only stash the outgoing tab's outer offset.
        let previous_type = self.state.result_type;
        self.result_scroll_offsets
            .insert(previous_type, self.scroll.offset());
        self.pending_forward_detail_scroll_reset = None;
        self.detail.reset();
        // Re-presenting cached results must not replay the entrance
        // animation. Fresh fetches re-arm it when they complete.
        self.results_entrance_key = None;
        self.search_query = query.trim().to_owned();
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        let job = self.state.select_type(result_type, &query);
        // Reuse the same scroll handle so the scroll surface is not
        // remounted. Fresh fetches start at the top; cache hits restore
        // the returning tab's saved viewport.
        self.browser_scroll.reset();
        if job.is_some() {
            self.scroll.set_offset(point(px(0.), px(0.)));
        } else if let Some(offset) = self.result_scroll_offsets.get(&result_type).copied() {
            self.scroll.set_offset(offset);
        } else {
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        self.run(job, deezer_arl, soundcloud_token, cx);
    }

    pub(crate) fn submit(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if self.search_request_is_active(self.state.source, ResultType::All, &query) {
            self.cancel_suggestion_request();
            self.suggestions.set_focused(false);
            cx.notify();
            return;
        }
        self.cancel_search_request();
        self.cancel_suggestion_request();
        self.cancel_discover_requests();
        self.cancel_discover_channel_request();
        self.discover.close_channel();
        self.clear_track_list_states();
        self.clear_card_grid_states();
        self.result_scroll_offsets.clear();
        self.results_entrance_key = None;
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        self.detail.reset();
        self.search_query = query.trim().to_owned();
        if self.suggestions.record(&query) {
            self.persist_search_history(cx);
        }
        self.suggestions.set_focused(false);
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        let job = self.state.submit(&query);
        self.run(job, deezer_arl, soundcloud_token, cx);
    }

    fn search_credentials(
        &self,
        cx: &Context<Self>,
    ) -> (
        Option<DeezerArl>,
        Option<super::credential::SoundCloudToken>,
    ) {
        let account = self.account.read(cx);
        (account.deezer_arl(), account.soundcloud_token())
    }

    pub(crate) fn open_card_info(
        &mut self,
        card: Card,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        let prefetched = self.prefetched_card_info(&card);
        super::album_info::open_card_info_dialog_with_prefetch(
            card,
            self.runtime.clone(),
            self.client.clone().ok(),
            deezer_arl,
            soundcloud_token,
            prefetched,
            window,
            cx,
        );
    }

    fn prefetched_card_info(
        &self,
        card: &Card,
    ) -> Option<(super::detail::DetailRoute, super::album_info::AlbumInfo)> {
        let route = super::detail::DetailRoute::from_card(card)?;
        if let Some(cached) = info_for_detail_state(&self.detail.state, &route) {
            return Some(cached);
        }
        self.album_info_prefetch.cached(&route)
    }

    /// Starts a background detail fetch for an album or playlist card so the
    /// Info dialog can open already populated. Called when a context menu
    /// opens. Duplicate and cached requests are skipped, and other kinds are
    /// ignored. The dialog still fetches on its own when prefetch has not
    /// completed.
    pub(crate) fn prefetch_card_info(&mut self, card: Card, cx: &mut Context<Self>) {
        let Some(key) = AlbumInfoCacheKey::from_card(&card) else {
            return;
        };
        let Some(route) = super::detail::DetailRoute::from_card(&card) else {
            return;
        };
        if self.album_info_prefetch.cached(&route).is_some() {
            return;
        }
        if let Some(page) = match &self.detail.state {
            DetailState::Results(page) | DetailState::Empty(page)
                if page.route.provider == route.provider
                    && page.route.kind == route.kind
                    && page.route.id == route.id =>
            {
                Some((**page).clone())
            }
            _ => None,
        } {
            self.album_info_prefetch.store_page(page);
            return;
        }
        if !self.album_info_prefetch.begin(key.clone()) {
            return;
        }
        let (deezer_arl, soundcloud_token) = self.search_credentials(cx);
        // Deezer detail loads anonymously; SoundCloud detail needs its token.
        let has_account = match route.provider {
            Provider::Deezer => true,
            Provider::SoundCloud => soundcloud_token.is_some(),
        };
        if !has_account {
            self.album_info_prefetch.fail(&key);
            return;
        }
        let Ok(client) = self.client.clone() else {
            self.album_info_prefetch.fail(&key);
            return;
        };
        let task = self
            .runtime
            .spawn(async move { client.detail(route, deezer_arl, soundcloud_token).await });
        let account_scope = self.account_scope.clone();
        let cache_generation = self.album_info_prefetch.generation();
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(super::models::ProviderError::new(
                    "Collection request failed",
                ))
            });
            this.update(cx, |this, _| {
                if this.account_scope != account_scope
                    || this.album_info_prefetch.generation() != cache_generation
                {
                    return;
                }
                match result {
                    Ok(page) => this.album_info_prefetch.complete_for_generation(
                        cache_generation,
                        key,
                        page,
                    ),
                    Err(_) => this
                        .album_info_prefetch
                        .fail_for_generation(cache_generation, &key),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn toggle_track_favorite(
        &mut self,
        provider: Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.begin_favorite_action(
            FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id),
            known_favorite,
            cx,
        );
    }

    pub(crate) fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| {
            library.start_deezer_track_mix(track_id, cx);
        });
    }

    pub(crate) fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| {
            library.start_deezer_artist_mix(artist_id, cx);
        });
    }

    pub(crate) fn open_deezer_flow(&mut self, card: Card, smart_mix: bool, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| {
            library.open_discover_flow(card, smart_mix, cx);
        });
    }

    pub(crate) fn apply_smart_mix_title(
        &mut self,
        config_id: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        if self.discover.apply_smart_mix_title(config_id, title) {
            cx.notify();
        }
    }

    pub(crate) fn play_deezer_flow(&mut self, card: Card, smart_mix: bool, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| {
            library.start_deezer_flow(
                crate::library::Card {
                    kind: crate::library::Category::Flow,
                    id: card.id,
                    title: card.title,
                    subtitle: card.subtitle,
                    artwork: card.artwork,
                    source: Provider::Deezer,
                    ..crate::library::Card::default()
                },
                smart_mix,
                cx,
            );
        });
    }

    pub(crate) fn open_deezer_channel(&mut self, card: Card, slug: String, cx: &mut Context<Self>) {
        let Some(slug) = discover::valid_channel_slug(&slug) else {
            return;
        };
        self.cancel_discover_requests();
        self.cancel_discover_channel_request();
        self.pending_forward_detail_scroll_reset = None;
        self.detail.reset();
        let title = if card.title.trim().is_empty() {
            slug.clone()
        } else {
            card.title
        };
        let Some((generation, account_scope)) =
            self.discover.start_channel_with_title(slug.clone(), title)
        else {
            return;
        };
        if !self
            .discover
            .complete_channel_from_cache(generation, &account_scope, &slug)
        {
            self.load_deezer_channel(generation, account_scope, slug, cx);
        }
        cx.notify();
    }

    pub(crate) fn discover_channel_open(&self) -> bool {
        self.discover.channel_open()
    }

    pub(crate) fn close_discover_channel(&mut self, cx: &mut Context<Self>) {
        self.cancel_discover_channel_request();
        if self.discover.close_channel() {
            cx.notify();
        }
    }

    pub(crate) fn retry_discover_channel(&mut self, cx: &mut Context<Self>) {
        let Some((generation, account_scope, slug)) = self.discover.retry_channel() else {
            return;
        };
        if !self
            .discover
            .complete_channel_from_cache(generation, &account_scope, &slug)
        {
            self.load_deezer_channel(generation, account_scope, slug, cx);
        }
        cx.notify();
    }

    fn load_deezer_channel(
        &mut self,
        generation: u64,
        account_scope: String,
        slug: String,
        cx: &mut Context<Self>,
    ) {
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.discover.complete_channel(
                generation,
                &account_scope,
                &slug,
                Err("Deezer account required".into()),
            );
            return;
        };
        let Ok(client) = self.client.clone() else {
            self.discover.complete_channel(
                generation,
                &account_scope,
                &slug,
                Err("Deezer Discover is unavailable".into()),
            );
            return;
        };
        self.cancel_discover_channel_request();
        let request_slug = slug.clone();
        let task = self
            .runtime
            .spawn(async move { discover::load_channel(&client, arl, &request_slug).await });
        let request_id = self.next_request_id();
        self.discover_channel_request = Some(ActiveRequest {
            generation,
            id: request_id,
            abort: task.abort_handle(),
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer channel request failed".into()));
            this.update(cx, |this, cx| {
                this.clear_discover_channel_request(generation, request_id);
                if this
                    .discover
                    .complete_channel(generation, &account_scope, &slug, result)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_soundcloud_discover_selection(
        &mut self,
        card: Card,
        track_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if card.source != Provider::SoundCloud {
            return;
        }
        let ids = validated_soundcloud_discover_track_ids(track_ids);
        if ids.is_empty() {
            return;
        }
        let token = self.account.read(cx).soundcloud_token();
        let previous_scroll = self.active_vertical_scroll_offset();
        self.scroll.set_offset(point(px(0.), px(0.)));
        let route = super::detail::DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Playlists,
            id: card.id.clone(),
            title: card.title.clone(),
            subtitle: card.subtitle.clone(),
            artwork: card.artwork.clone(),
            release_date: String::new(),
            service_url: card.service_url.clone(),
        };
        let opened = self
            .detail
            .open_route_with_scroll(route, token.is_some(), previous_scroll);
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        let Some((generation, route)) = opened else {
            cx.notify();
            return;
        };
        let Some(token) = token else {
            cx.notify();
            return;
        };
        let Ok(client) = self.client.clone() else {
            self.complete_detail(
                generation,
                Err(super::models::ProviderError::new(
                    "Collection client could not be created",
                )),
                cx,
            );
            return;
        };
        let account_scope = self.account_scope.clone();
        self.pending_forward_detail_scroll_reset = Some(generation);
        self.schedule_forward_detail_scroll_reset(generation, cx);
        cx.notify();
        let task = self.runtime.spawn(async move {
            let items = client
                .hydrate_soundcloud_discover_track_values(&ids, &token)
                .await?;
            let tracks = super::normalize::normalize_tracks(Provider::SoundCloud, &items);
            let total = tracks.len();
            Ok(super::detail::DetailPage {
                route,
                tracks,
                total: Some(total),
                raw_loaded_count: total,
                normalized_count: total,
                authoritative_total: Some(total),
                artist: None,
                description: String::new(),
                album_info: None,
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(super::models::ProviderError::new(
                    "SoundCloud selection request failed",
                ))
            });
            this.update(cx, |this, cx| {
                if this.account_scope == account_scope {
                    this.complete_detail(generation, result, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn play_soundcloud_discover_selection(
        &mut self,
        card: Card,
        track_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if card.source != Provider::SoundCloud {
            return;
        }
        let ids = validated_soundcloud_discover_track_ids(track_ids);
        if ids.is_empty() {
            return;
        }
        let Some(token) = self.account.read(cx).soundcloud_token() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not play SoundCloud selection",
                Some("A SoundCloud account is required.".into()),
            );
            return;
        };
        let Ok(client) = self.client.clone() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not play SoundCloud selection",
                Some("The SoundCloud library client is unavailable.".into()),
            );
            return;
        };
        let account_scope = self.account_scope.clone();
        let action_generation = self
            .library
            .update(cx, |library, _| library.begin_playback_action());
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
        let playback = self.playback.clone();
        let context_urn = card.id;
        let task = self.runtime.spawn(async move {
            let items = client
                .hydrate_soundcloud_discover_track_values(&ids, &token)
                .await?;
            Ok(super::normalize::normalize_tracks(
                Provider::SoundCloud,
                &items,
            ))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(super::models::ProviderError::new(
                    "SoundCloud selection request failed",
                ))
            });
            this.update(cx, |this, cx| {
                if this.account_scope != account_scope
                    || !this
                        .library
                        .read(cx)
                        .is_playback_action_current(action_generation)
                    || playback.read(cx).state.queue_epoch() != queue_epoch
                {
                    return;
                }
                let Ok(tracks) = result else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not play SoundCloud selection",
                        Some("SoundCloud did not return playable tracks.".into()),
                    );
                    return;
                };
                if tracks.is_empty() {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "SoundCloud selection is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                    return;
                }
                let queue = tracks
                    .iter()
                    .map(crate::playback::PlaybackTrack::from_search)
                    .collect::<Vec<_>>();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.replace_queue(queue, 0, cx);
                    playback.set_context(
                        crate::playback::PlaybackContext::SoundCloudCollection { context_urn },
                        cx,
                    );
                });
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn start_soundcloud_artist_station(
        &mut self,
        artist_id: String,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.start_soundcloud_artist_station(artist_id, cx);
        });
    }

    pub(crate) fn start_soundcloud_track_station(
        &mut self,
        track: crate::playback::PlaybackTrack,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.start_soundcloud_track_station(track, cx);
        });
    }

    pub(crate) fn add_negative_feedback(
        &mut self,
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.add_negative_feedback(kind, id, cx);
        });
    }

    pub(crate) fn toggle_artist_favorite(
        &mut self,
        provider: Provider,
        artist_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.begin_favorite_action(
            FavoriteKey::for_provider(provider, FavoriteKind::Artist, artist_id),
            known_favorite,
            cx,
        );
    }

    pub(crate) fn open_add_picker(
        &mut self,
        track_ids: Vec<String>,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if track_ids.is_empty() {
            return;
        }
        let status_scope = self.add_status_scope(cx);
        self.library.update(cx, |library, cx| {
            library.open_add_picker(track_ids, status_scope, provider, window, cx)
        });
    }

    pub(super) fn add_status_scope(&self, cx: &gpui::Context<Self>) -> String {
        if let Some(route) = &self.detail.route {
            return format!(
                "detail:{}:{}:{}",
                route.provider.label(),
                route.kind.label(),
                route.id
            );
        }
        format!(
            "search:{:?}:{:?}:{}",
            self.state.source,
            self.state.result_type,
            self.input.read(cx).value()
        )
    }

    pub(crate) fn toggle_collection_favorite(
        &mut self,
        card: Card,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = favorite_kind(card.source, card.kind, &card.id) else {
            return;
        };
        self.begin_favorite_action(
            FavoriteKey::for_provider(card.source, kind, card.id),
            known_favorite,
            cx,
        );
    }

    fn run(
        &mut self,
        job: Option<SearchJob>,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<super::credential::SoundCloudToken>,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let Some(job) = job else { return };
        let Ok(client) = self.client.clone() else {
            self.state.complete(job.generation, Vec::new());
            cx.notify();
            return;
        };
        let generation = job.generation;
        let request_key = SearchRequestKey {
            source: self.state.source,
            result_type: self.state.result_type,
            query: self.search_query.clone(),
        };
        let account_scope = self.account_scope.clone();
        let account_required =
            all_search_missing_accounts(self.state.source, soundcloud_token.is_some());
        let request_id = self.next_request_id();
        let mut tasks = Vec::with_capacity(job.requests.len());
        for request in job.requests {
            let task_client = client.clone();
            let task_arl = deezer_arl.clone();
            let task_token = soundcloud_token.clone();
            tasks.push(self.runtime.spawn(async move {
                task_client
                    .execute(vec![request], task_arl, task_token)
                    .await
            }));
        }
        if tasks.is_empty() {
            self.state.complete_incremental_with_missing_accounts(
                generation,
                Vec::new(),
                &account_required,
                true,
            );
            cx.notify();
            return;
        }
        self.search_request = Some(ActiveSearchRequest {
            generation,
            id: request_id,
            aborts: tasks
                .iter()
                .map(tokio::task::JoinHandle::abort_handle)
                .collect(),
            pending_batches: tasks.len(),
            key: request_key,
        });
        for task in tasks {
            let account_scope = account_scope.clone();
            let account_required = account_required.clone();
            cx.spawn(async move |this, cx| {
                let results = task.await.unwrap_or_default();
                for result in &results {
                    if let Err(error) = &result.data {
                        eprintln!(
                            "Search request failed: {} {}: {}",
                            result.request.provider.label(),
                            result.request.category.label(),
                            error.message
                        );
                    }
                }
                this.update(cx, |this, cx| {
                    let Some(final_batch) =
                        this.finish_search_request_batch(generation, request_id)
                    else {
                        return;
                    };
                    if this.account_scope == account_scope
                        && this.state.complete_incremental_with_missing_accounts(
                            generation,
                            results,
                            &account_required,
                            final_batch,
                        )
                    {
                        if final_batch && !account_required.is_empty() {
                            crate::toast::push_global(
                                cx,
                                crate::toast::ToastKind::Warning,
                                "SoundCloud account required",
                                Some("Log in to SoundCloud from Settings to continue.".into()),
                            );
                        }
                        for track in &this.state.groups.tracks {
                            if let Some(favorite) = track.favorite {
                                this.favorites.update(cx, |favorites, _| {
                                    favorites.set_known(
                                        FavoriteKey::for_provider(
                                            track.source,
                                            FavoriteKind::Track,
                                            track.id.clone(),
                                        ),
                                        favorite,
                                    )
                                });
                            }
                        }
                        if matches!(this.state.state, ResultState::Results) {
                            let identity = search_results_content_identity(
                                this.state.source,
                                this.search_query(),
                            );
                            this.results_entrance_key = Some(identity);
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    pub(crate) fn open_card(&mut self, card: Card, cx: &mut Context<Self>) {
        self.open_card_from_context(card, false, cx);
    }

    pub(crate) fn open_discover_card(&mut self, card: Card, cx: &mut Context<Self>) {
        self.open_card_from_context(card, true, cx);
    }

    fn open_card_from_context(
        &mut self,
        card: Card,
        preserve_discover_channel: bool,
        cx: &mut Context<Self>,
    ) {
        if should_close_discover_channel_before_detail(
            preserve_discover_channel,
            self.discover_channel_open(),
        ) {
            self.cancel_discover_channel_request();
            self.discover.close_channel();
        }
        self.library
            .update(cx, |library, _| library.playlists.clear_remove_feedback());
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        // Deezer detail loads anonymously; SoundCloud detail needs its token.
        let has_account = match card.source {
            Provider::Deezer => true,
            Provider::SoundCloud => soundcloud_token.is_some(),
        };
        let valid_route = super::detail::DetailRoute::from_card(&card).is_some();
        let previous_scroll = self.active_vertical_scroll_offset();
        self.scroll.set_offset(point(px(0.), px(0.)));
        let opened = self
            .detail
            .open_with_scroll(&card, has_account, previous_scroll);
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        let Some((generation, route)) = opened else {
            if !valid_route {
                self.restore_detail_scroll(previous_scroll);
            }
            cx.notify();
            return;
        };
        self.pending_forward_detail_scroll_reset = Some(generation);
        self.schedule_forward_detail_scroll_reset(generation, cx);
        if route.kind == ResultType::Playlists {
            self.library.update(cx, |library, cx| match route.provider {
                Provider::Deezer => library.ensure_playlist_catalog(cx),
                Provider::SoundCloud => library.ensure_soundcloud_playlist_catalog(cx),
            });
        }
        cx.notify();
        self.run_detail(generation, route, deezer_arl, soundcloud_token, cx);
    }

    /// Opens a detail card requested by another main destination.  External
    /// navigation must reuse the Search detail renderer, but it must not
    /// replace the user's query or result groups.  Clearing only the detail
    /// navigation also makes the toolbar back action return to the caller
    /// instead of exposing an unrelated stale detail stack.
    pub(crate) fn open_external_card(&mut self, card: Card, cx: &mut Context<Self>) {
        self.cancel_discover_channel_request();
        self.discover.close_channel();
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        self.library
            .update(cx, |library, _| library.playlists.clear_remove_feedback());
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        // Deezer detail loads anonymously; SoundCloud detail needs its token.
        let has_account = match card.source {
            Provider::Deezer => true,
            Provider::SoundCloud => soundcloud_token.is_some(),
        };
        let opened = self.detail.replace(&card, has_account);
        let Some((generation, route)) = opened else {
            cx.notify();
            return;
        };
        self.pending_forward_detail_scroll_reset = Some(generation);
        if route.kind == ResultType::Playlists {
            self.library.update(cx, |library, cx| match route.provider {
                Provider::Deezer => library.ensure_playlist_catalog(cx),
                Provider::SoundCloud => library.ensure_soundcloud_playlist_catalog(cx),
            });
        }
        cx.notify();
        self.run_detail(generation, route, deezer_arl, soundcloud_token, cx);
    }

    pub(crate) fn open_similar_artists(
        &mut self,
        artist_id: String,
        title: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if artist_id.trim().is_empty() || !artist_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let card = Card {
            kind: ResultType::Artists,
            id: artist_id,
            title,
            source: Provider::Deezer,
            ..Card::default()
        };
        let deezer_arl = self.account.read(cx).deezer_arl();
        let has_account = deezer_arl.is_some();
        let previous_scroll = self.active_vertical_scroll_offset();
        self.scroll.set_offset(point(px(0.), px(0.)));
        let opened = self.detail.open_focused_artist_section(
            &card,
            has_account,
            super::detail::ArtistSection::SimilarArtists,
            previous_scroll,
        );
        self.pending_forward_detail_scroll_reset = None;
        self.reset_detail_scroll();
        let Some((generation, route)) = opened else {
            cx.notify();
            return;
        };
        self.pending_forward_detail_scroll_reset = Some(generation);
        self.schedule_forward_detail_scroll_reset(generation, cx);
        cx.notify();
        let saved_user_id = self.account.read(cx).deezer_user_id();
        self.run_similar_artists(generation, route, deezer_arl, saved_user_id, cx);
    }

    fn complete_detail(
        &mut self,
        generation: u64,
        result: Result<super::detail::DetailPage, super::models::ProviderError>,
        cx: &mut Context<Self>,
    ) {
        let favorite_seeds = detail_favorite_seeds(&result);
        let accepted = self.detail.complete(generation, result);
        if should_reset_detail_scroll(
            self.pending_forward_detail_scroll_reset,
            generation,
            accepted,
        ) {
            self.pending_forward_detail_scroll_reset = None;
            self.reset_detail_scroll();
            self.pending_forward_detail_scroll_reset = Some(generation);
            self.schedule_forward_detail_scroll_reset(generation, cx);
        }
        if accepted {
            if !favorite_seeds.is_empty() {
                self.favorites.update(cx, |favorites, _| {
                    for (key, favorite) in favorite_seeds {
                        favorites.set_known(key, favorite);
                    }
                });
            }
            cx.notify();
        }
    }

    fn run_detail(
        &mut self,
        generation: u64,
        route: super::detail::DetailRoute,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<super::credential::SoundCloudToken>,
        cx: &mut Context<Self>,
    ) {
        let Ok(client) = self.client.clone() else {
            self.complete_detail(
                generation,
                Err(super::models::ProviderError::new(
                    "Collection client could not be created",
                )),
                cx,
            );
            return;
        };
        let task = self
            .runtime
            .spawn(async move { client.detail(route, deezer_arl, soundcloud_token).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(super::models::ProviderError::new(
                    "Collection request failed",
                ))
            });
            this.update(cx, |this, cx| {
                this.complete_detail(generation, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn run_similar_artists(
        &mut self,
        generation: u64,
        route: super::detail::DetailRoute,
        deezer_arl: Option<DeezerArl>,
        saved_user_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(arl) = deezer_arl else {
            self.complete_detail(
                generation,
                Err(super::models::ProviderError::new("Deezer account required")),
                cx,
            );
            return;
        };
        let Ok(client) = self.library.read(cx).deezer_client() else {
            self.complete_detail(
                generation,
                Err(super::models::ProviderError::new(
                    "Similar Artists client could not be created",
                )),
                cx,
            );
            return;
        };
        let account_scope = self.account_scope.clone();
        let request_route = route.clone();
        let expected_route = route;
        let task = self.runtime.spawn(async move {
            client
                .load_similar_artists(&request_route.id, arl, saved_user_id)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Similar Artists request failed".to_owned()));
            this.update(cx, |this, cx| {
                if this.account_scope != account_scope {
                    return;
                }
                let result = result.map(|related| {
                    let similar_artists = related
                        .artists
                        .into_iter()
                        .map(|artist| Card {
                            kind: ResultType::Artists,
                            id: artist.id,
                            title: artist.title,
                            subtitle: artist.subtitle,
                            artwork: artist.artwork,
                            source: Provider::Deezer,
                            badge: artist.badge,
                            release_date: String::new(),
                            service_url: String::new(),
                        })
                        .collect::<Vec<_>>();
                    super::detail::DetailPage {
                        route: expected_route.clone(),
                        tracks: Vec::new(),
                        total: None,
                        raw_loaded_count: 0,
                        normalized_count: 0,
                        authoritative_total: None,
                        artist: Some(super::models::ArtistPage {
                            profile: Card::default(),
                            similar_total: related.total,
                            similar_artists,
                            ..super::models::ArtistPage::default()
                        }),
                        description: String::new(),
                        album_info: None,
                    }
                });
                this.complete_detail(
                    generation,
                    result.map_err(super::models::ProviderError::new),
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn sync_playlist_update(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.detail.route.as_ref().map(|route| route.provider) else {
            return;
        };
        let updated = {
            let library = self.library.read(cx);
            let catalog = library.playlist_catalog(provider);
            let revision = catalog.update_revision;
            let seen_revision = match provider {
                Provider::Deezer => &mut self.playlist_update_revision,
                Provider::SoundCloud => &mut self.soundcloud_playlist_update_revision,
            };
            if revision == *seen_revision {
                return;
            }
            *seen_revision = revision;
            catalog.updated.clone()
        };
        let Some(updated) = updated else { return };
        let Some(current_route) = self.detail.route.as_ref() else {
            return;
        };
        if current_route.kind != ResultType::Playlists
            || !crate::library::playlist_state::matching_playlist_route(
                current_route.provider,
                "playlistTracks",
                &current_route.id,
                &updated.id,
            )
        {
            return;
        }
        let title = updated.title.clone();
        let subtitle = crate::library::playlist_state::updated_playlist_subtitle(
            &current_route.subtitle,
            &updated.owner.name,
        );
        let artwork = (!updated.artwork.is_empty()).then(|| updated.artwork.clone());
        let description = updated.description.clone();
        let Some(route) = self.detail.route.as_mut() else {
            return;
        };
        route.title = title.clone();
        route.subtitle = subtitle.clone();
        if let Some(artwork) = artwork.as_deref() {
            route.artwork = artwork.to_owned();
        }
        match &mut self.detail.state {
            DetailState::Results(page) | DetailState::Empty(page) => {
                page.route.title = title;
                page.route.subtitle = subtitle;
                if let Some(artwork) = artwork {
                    page.route.artwork = artwork;
                }
                page.description = description;
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn sync_playlist_content(&mut self, cx: &mut Context<Self>) {
        let playlist_id = {
            let library = self.library.read(cx);
            let revision = library.playlists.content_revision;
            if revision == self.playlist_content_revision {
                return;
            }
            self.playlist_content_revision = revision;
            library.playlists.added_to.clone()
        };
        let Some(playlist_id) = playlist_id else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if route.kind != ResultType::Playlists
            || !crate::library::playlist_state::matching_playlist_route(
                route.provider,
                "playlistTracks",
                &route.id,
                &playlist_id,
            )
        {
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }

    pub(crate) fn sync_playlist_reorder(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.detail.route.as_ref().map(|route| route.provider) else {
            return;
        };
        cx.notify();
        let completion = {
            let library = self.library.read(cx);
            let catalog = library.playlist_catalog(provider);
            let revision = catalog.reorder_completion_revision;
            if self.playlist_reorder_provider == Some(provider)
                && revision == self.playlist_reorder_revision
            {
                return;
            }
            self.playlist_reorder_provider = Some(provider);
            self.playlist_reorder_revision = revision;
            (
                catalog.reorder_completion_route.clone(),
                catalog.reorder_completion_succeeded,
            )
        };
        let (Some(revision_route), Some(succeeded)) = completion else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if !crate::library::playlist_state::matching_playlist_route(
            route.provider,
            "playlistTracks",
            &route.id,
            &revision_route.playlist_id,
        ) || revision_route.account_scope != self.account_scope
        {
            return;
        }
        let matching_snapshot = self
            .playlist_reorder_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.account_scope == self.account_scope
                    && snapshot.provider == route.provider
                    && snapshot.playlist_id == route.id
                    && snapshot.view_id == self.detail.view_id()
            });
        if matching_snapshot {
            if !succeeded {
                let snapshot = self
                    .playlist_reorder_snapshot
                    .take()
                    .expect("matching playlist reorder snapshot");
                if let DetailState::Results(page) = &mut self.detail.state {
                    page.tracks = snapshot.tracks;
                }
            } else {
                self.playlist_reorder_snapshot = None;
            }
            cx.notify();
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }

    pub(crate) fn playlist_reorder_detail_eligible(&self, cx: &Context<SearchView>) -> bool {
        let Some(route) = self.detail.route.as_ref() else {
            return false;
        };
        let DetailState::Results(page) = &self.detail.state else {
            return false;
        };
        let catalog = self.library.read(cx).playlist_catalog(route.provider);
        !catalog.reorder_pending
            && crate::library::playlist_reorder::detail_eligible(
                route.provider,
                route.kind,
                catalog.is_editable(&route.id),
                page.total,
                page.raw_loaded_count,
                page.normalized_count,
                &page.tracks,
            )
    }

    pub(crate) fn move_playlist_track(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if !self.playlist_reorder_detail_eligible(cx) {
            return;
        }
        let Some(route) = self.detail.route.clone() else {
            return;
        };
        let DetailState::Results(page) = &self.detail.state else {
            return;
        };
        let original_tracks = page.tracks.clone();
        let previous = page
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let Some(reordered) =
            crate::library::playlist_reorder::reorder_items(&page.tracks, from, to)
        else {
            return;
        };
        let submitted = reordered
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let view_id = self.detail.view_id();
        let started = self.library.update(cx, |library, cx| {
            library.start_playlist_reorder(
                route.provider,
                route.id.clone(),
                None,
                previous,
                submitted,
                cx,
            )
        });
        if !started || self.detail.view_id() != view_id {
            return;
        }
        let Some(active_route) = self.detail.route.as_ref() else {
            return;
        };
        if active_route.provider != route.provider
            || active_route.kind != route.kind
            || active_route.id != route.id
        {
            return;
        }
        let DetailState::Results(page) = &mut self.detail.state else {
            return;
        };
        page.tracks = reordered;
        self.playlist_reorder_snapshot = Some(PlaylistReorderSnapshot {
            account_scope: self.account_scope.clone(),
            provider: route.provider,
            playlist_id: route.id,
            view_id,
            tracks: original_tracks,
        });
        cx.notify();
    }

    pub(crate) fn playlist_editable(&self, provider: Provider, id: &str, cx: &App) -> bool {
        self.library.read(cx).playlist_editable(provider, id)
    }

    pub(crate) fn is_playlist_owned(
        &self,
        provider: Provider,
        id: &str,
        subtitle: &str,
        cx: &App,
    ) -> bool {
        if self.playlist_editable(provider, id, cx) {
            return true;
        }
        if provider != Provider::Deezer {
            return false;
        }
        let account = self.account.read(cx);
        if let Some(profile) = account.deezer_profile()
            && !profile.username.is_empty()
            && profile.username.eq_ignore_ascii_case(subtitle.trim())
        {
            return true;
        }
        if let Some(user_id) = account.deezer_user_id()
            && !user_id.is_empty()
            && user_id.trim() == subtitle.trim()
        {
            return true;
        }
        false
    }

    pub(crate) fn open_playlist_editor(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.open_playlist_editor(provider, id, window, cx)
        });
    }

    pub(crate) fn open_playlist_delete(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.open_playlist_delete(provider, id, window, cx)
        });
    }

    pub(crate) fn sync_playlist_delete(&mut self, cx: &mut Context<Self>) -> Option<Provider> {
        let deleted = {
            let library = self.library.read(cx);
            if library.playlists.delete_revision != self.playlist_delete_revision {
                self.playlist_delete_revision = library.playlists.delete_revision;
                Some((Provider::Deezer, library.playlists.deleted.clone()))
            } else if library.soundcloud_playlists.delete_revision
                != self.soundcloud_playlist_delete_revision
            {
                self.soundcloud_playlist_delete_revision =
                    library.soundcloud_playlists.delete_revision;
                Some((
                    Provider::SoundCloud,
                    library.soundcloud_playlists.deleted.clone(),
                ))
            } else {
                None
            }
        };
        let (provider, deleted) = deleted?;
        let deleted = deleted?;
        if self.detail.route.as_ref().is_some_and(|route| {
            route.provider == provider && route.kind == ResultType::Playlists && route.id == deleted
        }) {
            self.pending_forward_detail_scroll_reset = None;
            self.detail.close_all();
            cx.notify();
            return Some(provider);
        }
        None
    }

    pub(crate) fn sync_playlist_remove(&mut self, cx: &mut Context<Self>) {
        let playlist_id = {
            let library = self.library.read(cx);
            let revision = library.playlists.remove_revision;
            if revision == self.playlist_remove_revision {
                return;
            }
            self.playlist_remove_revision = revision;
            library.playlists.remove_playlist.clone()
        };
        let Some(playlist_id) = playlist_id else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if !crate::library::playlist_state::matching_playlist_route(
            route.provider,
            "playlistTracks",
            &route.id,
            &playlist_id,
        ) {
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }

    pub(crate) fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.pending_forward_detail_scroll_reset = None;
        let offset = self
            .detail
            .back_with_scroll(self.active_vertical_scroll_offset());
        self.restore_detail_scroll(offset);
        cx.notify();
    }

    pub(crate) fn close_detail_for_main_navigation(&mut self, cx: &mut Context<Self>) {
        if self.detail_open() {
            self.close_detail(cx);
        }
    }

    pub(crate) fn close_search_navigation(&mut self, cx: &mut Context<Self>) {
        match search_navigation_back_target(self.detail_open(), self.discover_channel_open()) {
            SearchNavigationBackTarget::Detail => self.close_detail(cx),
            SearchNavigationBackTarget::DiscoverChannel => self.close_discover_channel(cx),
        }
    }

    pub(crate) fn toggle_artist_section(
        &mut self,
        section: super::detail::ArtistSection,
        cx: &mut Context<Self>,
    ) {
        self.detail.toggle_artist_section(section);
        self.reset_detail_scroll();
        cx.notify();
    }
}

impl ResizeSettledTarget for SearchView {
    fn commit_resize(&mut self, request: crate::music_ui::ResizeRequest) -> bool {
        self.card_columns.commit(request)
    }
}

impl super::album_info_cache::AlbumInfoCacheHost for SearchView {
    fn album_info_generation(&self) -> u64 {
        self.album_info_prefetch.generation()
    }

    fn store_album_info_page(&mut self, page: &super::detail::DetailPage) {
        self.album_info_prefetch.store_page(page.clone());
    }
}

fn detail_favorite_seeds(
    result: &Result<super::detail::DetailPage, super::models::ProviderError>,
) -> Vec<(FavoriteKey, bool)> {
    let Ok(page) = result else {
        return Vec::new();
    };
    let mut seeds = Vec::new();
    if page.route.provider == Provider::Deezer
        && page.route.kind == ResultType::Artists
        && let Some(favorite) = page.artist.as_ref().and_then(|artist| artist.favorite)
        && !page.route.id.trim().is_empty()
    {
        seeds.push((
            FavoriteKey::for_provider(
                Provider::Deezer,
                FavoriteKind::Artist,
                page.route.id.clone(),
            ),
            favorite,
        ));
    }
    for track in page.tracks.iter().chain(
        page.artist
            .iter()
            .flat_map(|artist| artist.popular_tracks.iter()),
    ) {
        if let Some(favorite) = track.favorite
            && !track.id.trim().is_empty()
        {
            seeds.push((
                FavoriteKey::for_provider(track.source, FavoriteKind::Track, track.id.clone()),
                favorite,
            ));
        }
    }
    seeds
}

pub(super) fn favorite_kind(
    provider: Provider,
    kind: ResultType,
    id: &str,
) -> Option<FavoriteKind> {
    let supported_provider = matches!(provider, Provider::Deezer | Provider::SoundCloud);
    if !supported_provider || super::detail::validate_id(id).is_err() {
        return None;
    }
    match kind {
        ResultType::Albums => Some(FavoriteKind::Album),
        ResultType::Artists => Some(FavoriteKind::Artist),
        ResultType::Playlists => Some(FavoriteKind::Playlist),
        ResultType::All | ResultType::Tracks => None,
    }
}

fn validated_soundcloud_discover_track_ids(track_ids: Vec<String>) -> Vec<String> {
    let mut ids = Vec::new();
    for id in track_ids {
        let id = id.trim();
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if ids.iter().any(|existing: &String| existing == id) {
            continue;
        }
        ids.push(id.to_owned());
        if ids.len() == 500 {
            break;
        }
    }
    ids
}

impl crate::entity_navigation::TrackMenuHost for SearchView {
    fn favorites_entity(&self) -> Entity<FavoriteState> {
        self.favorites.clone()
    }

    fn local_track_saved(&self, track: &crate::playback::PlaybackTrack, cx: &gpui::App) -> bool {
        crate::entity_navigation::TrackMenuHost::local_track_saved(self.library.read(cx), track, cx)
    }

    fn set_local_track_saved(
        &mut self,
        track: crate::playback::PlaybackTrack,
        saved: bool,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            crate::entity_navigation::TrackMenuHost::set_local_track_saved(
                library, track, saved, cx,
            )
        });
    }

    fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| {
            library.resolve_favorite_state(key, cx);
        });
    }

    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_add_picker(vec![track_id], provider, window, cx)
    }

    fn open_local_playlist_picker(
        &mut self,
        track: crate::playback::PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.open_local_playlist_picker(track, window, cx);
        });
    }

    fn preload_playlist_catalog(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.library.update(cx, |library, cx| match provider {
            Provider::Deezer => library.ensure_playlist_catalog(cx),
            Provider::SoundCloud => library.ensure_soundcloud_playlist_catalog(cx),
        });
    }

    fn toggle_favorite_state(
        &mut self,
        provider: Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.toggle_track_favorite(provider, track_id, known_favorite, cx)
    }

    fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        self.start_deezer_track_mix(track_id, cx);
    }

    fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.start_deezer_artist_mix(artist_id, cx);
    }

    fn start_soundcloud_artist_station(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.start_soundcloud_artist_station(artist_id, cx);
    }

    fn start_soundcloud_track_station(
        &mut self,
        track: crate::playback::PlaybackTrack,
        cx: &mut Context<Self>,
    ) {
        self.start_soundcloud_track_station(track, cx);
    }

    fn add_negative_feedback(
        &mut self,
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.add_negative_feedback(kind, id, cx);
    }

    fn open_similar_artists(
        &mut self,
        artist_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_similar_artists(artist_id, title, window, cx);
    }

    fn toggle_artist_favorite(
        &mut self,
        provider: Provider,
        artist_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.toggle_artist_favorite(provider, artist_id, known_favorite, cx);
    }

    fn open_track_info(
        &mut self,
        provider: Provider,
        track_id: String,
        seed: crate::library::TrackInfo,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.runtime.clone();
        crate::library::open_track_info_dialog(
            provider,
            track_id,
            seed,
            deezer_arl,
            soundcloud_token,
            runtime,
            window,
            cx,
        );
    }

    fn preload_track_info(
        &mut self,
        provider: Provider,
        track_id: String,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        _cx: &mut Context<Self>,
    ) {
        crate::library::preload_track_info(
            provider,
            track_id,
            deezer_arl,
            soundcloud_token,
            &self.runtime,
        );
    }
}

#[cfg(test)]
mod favorite_tests {
    use super::*;

    fn favorite_track(provider: Provider, id: &str, favorite: Option<bool>) -> Track {
        Track {
            id: id.into(),
            source: provider,
            favorite,
            ..Track::default()
        }
    }

    #[test]
    fn collection_favorites_support_captured_deezer_and_soundcloud_entities() {
        assert_eq!(
            favorite_kind(Provider::Deezer, ResultType::Albums, "42"),
            Some(FavoriteKind::Album)
        );
        assert_eq!(
            favorite_kind(Provider::SoundCloud, ResultType::Albums, "42"),
            Some(FavoriteKind::Album)
        );
        assert_eq!(
            favorite_kind(Provider::SoundCloud, ResultType::Artists, "42"),
            Some(FavoriteKind::Artist)
        );
        assert_eq!(
            favorite_kind(Provider::SoundCloud, ResultType::Playlists, "42"),
            Some(FavoriteKind::Playlist)
        );
        assert!(favorite_kind(Provider::Deezer, ResultType::Albums, "").is_none());
        assert!(favorite_kind(Provider::Deezer, ResultType::Tracks, "42").is_none());
        assert!(favorite_kind(Provider::SoundCloud, ResultType::Tracks, "42").is_none());
    }

    #[test]
    fn detail_favorite_seeds_extract_artist_and_track_membership() {
        let result = Ok(super::super::detail::DetailPage {
            route: super::super::detail::DetailRoute {
                provider: Provider::Deezer,
                kind: ResultType::Artists,
                id: "artist-1".into(),
                ..super::super::detail::DetailRoute {
                    provider: Provider::Deezer,
                    kind: ResultType::Artists,
                    id: String::new(),
                    title: String::new(),
                    subtitle: String::new(),
                    artwork: String::new(),
                    release_date: String::new(),
                    service_url: String::new(),
                }
            },
            tracks: vec![favorite_track(Provider::Deezer, "page-track", Some(true))],
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(super::super::models::ArtistPage {
                favorite: Some(true),
                popular_tracks: vec![favorite_track(
                    Provider::Deezer,
                    "popular-track",
                    Some(false),
                )],
                ..super::super::models::ArtistPage::default()
            }),
            description: String::new(),
            album_info: None,
        });

        let seeds = detail_favorite_seeds(&result);

        assert!(seeds.contains(&(
            FavoriteKey::for_provider(Provider::Deezer, FavoriteKind::Artist, "artist-1".into()),
            true
        )));
        assert!(seeds.contains(&(
            FavoriteKey::for_provider(Provider::Deezer, FavoriteKind::Track, "page-track".into()),
            true
        )));
        assert!(seeds.contains(&(
            FavoriteKey::for_provider(
                Provider::Deezer,
                FavoriteKind::Track,
                "popular-track".into()
            ),
            false
        )));
        assert_eq!(seeds.len(), 3);
    }

    #[test]
    fn detail_favorite_seeds_ignore_errors_unknown_values_and_empty_ids() {
        let error = Err(super::super::models::ProviderError::new("failed"));
        assert!(detail_favorite_seeds(&error).is_empty());

        let result = Ok(super::super::detail::DetailPage {
            route: super::super::detail::DetailRoute {
                provider: Provider::SoundCloud,
                kind: ResultType::Albums,
                id: "42".into(),
                title: String::new(),
                subtitle: String::new(),
                artwork: String::new(),
                release_date: String::new(),
                service_url: String::new(),
            },
            tracks: vec![favorite_track(Provider::Deezer, "", None)],
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: None,
            description: String::new(),
            album_info: None,
        });
        assert!(detail_favorite_seeds(&result).is_empty());
    }
}

#[cfg(test)]
mod account_required_tests {
    use super::*;

    #[test]
    fn discover_credential_gate_covers_each_provider_and_all() {
        assert!(discover_provider_has_credentials(
            Provider::Deezer,
            true,
            false
        ));
        assert!(!discover_provider_has_credentials(
            Provider::Deezer,
            false,
            true
        ));
        assert!(discover_provider_has_credentials(
            Provider::SoundCloud,
            false,
            true
        ));
        assert!(!discover_provider_has_credentials(
            Provider::SoundCloud,
            true,
            false
        ));

        assert_eq!(
            Source::All
                .providers()
                .iter()
                .map(|provider| discover_provider_has_credentials(*provider, true, true))
                .collect::<Vec<_>>(),
            vec![true, true]
        );
        assert_eq!(
            Source::All
                .providers()
                .iter()
                .map(|provider| discover_provider_has_credentials(*provider, false, false))
                .collect::<Vec<_>>(),
            vec![false, false]
        );
    }

    #[test]
    fn all_search_missing_accounts_reports_only_soundcloud() {
        assert!(all_search_missing_accounts(Source::All, true).is_empty());
        assert_eq!(
            all_search_missing_accounts(Source::All, false),
            vec![Provider::SoundCloud]
        );
        assert!(all_search_missing_accounts(Source::Deezer, false).is_empty());
        assert!(all_search_missing_accounts(Source::SoundCloud, false).is_empty());
    }

    #[test]
    fn account_required_toast_only_runs_after_an_accepted_completion() {
        let run = include_str!("view.rs")
            .split_once("fn run(")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn open_card"))
            .map_or_else(
                || panic!("search run must remain before open_card"),
                |(run, _)| run,
            );
        let complete = run
            .find("this.state.complete_incremental_with_missing_accounts(")
            .expect("search completion guard");
        let toast = run
            .find("crate::toast::push_global(")
            .expect("account-required toast dispatch");
        assert!(complete < toast);
        assert!(run.contains(
            "this.account_scope == account_scope\n                        && this.state.complete_incremental_with_missing_accounts"
        ));

        let select_type = include_str!("view.rs")
            .split_once("pub(super) fn select_type")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn submit"))
            .map_or_else(
                || panic!("search select_type must remain before submit"),
                |(body, _)| body,
            );
        assert!(!select_type.contains("missing_account_providers"));
        assert!(!select_type.contains("account_required_toast_copy"));
    }
}

#[cfg(test)]
mod scroll_tests {
    use gpui::px;

    use super::*;

    #[test]
    fn forward_detail_keeps_previous_list_offset_and_new_state_starts_at_top() {
        let previous = ListState::new(8, ListAlignment::Top, px(8.));
        previous.scroll_to(gpui::ListOffset {
            item_ix: 4,
            offset_in_item: px(3.),
        });
        let current = ListState::new(8, ListAlignment::Top, px(8.));

        let previous_offset = previous.logical_scroll_top();
        assert_eq!(previous_offset.item_ix, 4);
        assert_eq!(previous_offset.offset_in_item, px(3.));
        let current_offset = current.logical_scroll_top();
        assert_eq!(current_offset.item_ix, 0);
        assert_eq!(current_offset.offset_in_item, px(0.));
    }

    #[test]
    fn detail_back_preserves_the_dedicated_card_grid_offset() {
        let card = Card {
            kind: ResultType::Albums,
            id: "42".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        let search_offset = point(px(0.), px(-320.));
        let mut navigation = DetailNavigation::default();

        assert!(
            navigation
                .open_with_scroll(&card, true, search_offset)
                .is_some()
        );
        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            search_offset
        );
    }

    #[test]
    fn channel_detail_back_restores_channel_identity_and_scroll_before_closing_channel() {
        let mut discover = DiscoverState::new("scope".into());
        let (generation, account_scope) = discover
            .start_channel_with_title("dance".into(), "Dance & EDM".into())
            .unwrap();
        assert!(discover.complete_channel(
            generation,
            &account_scope,
            "dance",
            Ok(("Dance & EDM".into(), Vec::new())),
        ));

        let card = Card {
            kind: ResultType::Albums,
            id: "42".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        let channel_scroll = point(px(0.), px(-240.));
        let mut navigation = DetailNavigation::default();
        assert!(
            navigation
                .open_with_scroll(&card, true, channel_scroll)
                .is_some()
        );
        assert!(!should_close_discover_channel_before_detail(
            true,
            discover.channel_open()
        ));
        assert_eq!(
            search_navigation_back_target(true, discover.channel_open()),
            SearchNavigationBackTarget::Detail
        );
        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            channel_scroll
        );
        assert!(discover.channel_open());
        assert_eq!(discover.channel().slug, "dance");

        assert_eq!(
            search_navigation_back_target(false, discover.channel_open()),
            SearchNavigationBackTarget::DiscoverChannel
        );
        assert!(discover.close_channel());
        assert!(!discover.channel_open());
    }

    #[test]
    fn ordinary_detail_open_still_clears_an_unrelated_channel_context() {
        assert!(should_close_discover_channel_before_detail(false, true));
        assert!(!should_close_discover_channel_before_detail(false, false));
        assert!(!should_close_discover_channel_before_detail(true, true));

        let source = include_str!("view.rs");
        let open_card = source
            .split_once("pub(crate) fn open_card(&mut self")
            .and_then(|(_, rest)| {
                rest.split_once("/// Opens a detail card requested by another main destination.")
            })
            .map(|(method, _)| method)
            .expect("ordinary card opener should remain before external opener");
        assert!(open_card.contains("self.open_card_from_context(card, false, cx)"));
        assert!(source.contains("self.open_card_from_context(card, true, cx)"));
        let external_card = source
            .split_once("pub(crate) fn open_external_card")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn open_similar_artists"))
            .map(|(method, _)| method)
            .expect("external card opener should remain before similar artists");
        assert!(external_card.contains("self.discover.close_channel();"));
    }

    #[test]
    fn detail_back_has_priority_over_a_retained_discover_channel() {
        assert_eq!(
            search_navigation_back_target(true, true),
            SearchNavigationBackTarget::Detail
        );
        assert_eq!(
            search_navigation_back_target(false, true),
            SearchNavigationBackTarget::DiscoverChannel
        );
        assert_eq!(
            search_navigation_back_target(false, false),
            SearchNavigationBackTarget::Detail
        );

        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(crate) fn close_search_navigation")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn toggle_artist_section"))
            .map(|(method, _)| method)
            .expect("search navigation close method");
        let detail_branch = method
            .find("SearchNavigationBackTarget::Detail")
            .expect("detail back branch");
        let channel_branch = method
            .find("SearchNavigationBackTarget::DiscoverChannel")
            .expect("channel back branch");
        assert!(detail_branch < channel_branch);
    }

    #[test]
    fn submitted_search_results_expose_a_root_navigation_target_only_at_the_root() {
        assert!(search_results_root_visible(
            "ambient",
            &ResultState::Results,
            false,
            false
        ));
        assert!(search_results_root_visible(
            "ambient",
            &ResultState::Loading,
            false,
            false
        ));
        assert!(search_results_root_visible(
            "ambient",
            &ResultState::Empty,
            false,
            false
        ));
        assert!(!search_results_root_visible(
            "ambient",
            &ResultState::Initial,
            false,
            false
        ));
        assert!(!search_results_root_visible(
            "",
            &ResultState::Results,
            false,
            false
        ));
        assert!(!search_results_root_visible(
            "ambient",
            &ResultState::Results,
            true,
            false
        ));
        assert!(!search_results_root_visible(
            "ambient",
            &ResultState::Results,
            false,
            true
        ));
    }

    #[test]
    fn main_navigation_closes_detail_without_closing_discover_channel() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(crate) fn close_detail_for_main_navigation")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn close_search_navigation"))
            .map(|(method, _)| method)
            .expect("main-navigation detail helper should remain focused");
        assert!(method.contains("if self.detail_open()"));
        assert!(method.contains("self.close_detail(cx)"));
        assert!(!method.contains("close_discover_channel"));
    }

    #[test]
    fn discover_feed_cache_reacts_only_to_content_changes() {
        let implementation = include_str!("view.rs");
        let method = implementation
            .split_once("fn update_discover_feed_cache")
            .and_then(|(_, rest)| rest.split_once("impl SearchView"))
            .map(|(method, _)| method)
            .expect("discover feed cache update implementation");
        assert!(method.contains("cache.state.reset(item_count)"));
        assert!(method.contains("cache.browser_scroll.reset()"));
        // Width changes re-measure every row inside the same prepaint, so
        // the cache tracks neither layout width nor pending hint restores.
        assert!(!method.contains("width_changed"));
        assert!(!method.contains("hint_restore_pending"));
        assert!(!method.contains("with_uniform_item_height"));
        assert!(!method.contains("reset_with_uniform_height"));
        let state_method = implementation
            .split_once("pub(super) fn discover_feed_state")
            .and_then(|(_, rest)| rest.split_once("fn discover_feed_cache_identity"))
            .map(|(method, _)| method)
            .expect("discover feed state implementation");
        assert!(state_method.contains(".measure_all()"));
        assert!(!state_method.contains("with_uniform_item_height"));
    }

    #[test]
    fn discover_feed_content_change_resets_the_fresh_feed_to_the_top() {
        let state = ListState::new(8, ListAlignment::Top, px(8.)).measure_all();
        state.scroll_to(gpui::ListOffset {
            item_ix: 4,
            offset_in_item: px(3.),
        });
        let mut cache = DiscoverFeedCache {
            state: state.clone(),
            browser_scroll: BrowserScrollState::new(),
            content_identity: "feed-a".to_owned(),
            item_count: 8,
        };

        // Same content: the cache leaves the measured state and scroll alone.
        update_discover_feed_cache(&mut cache, "feed-a", 8);
        assert_eq!(state.logical_scroll_top().item_ix, 4);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 3.);

        // A content change resets to the top with the new item count, and
        // the reset re-arms the full measure pass for the next prepaint.
        update_discover_feed_cache(&mut cache, "feed-b", 12);
        assert_eq!(state.item_count(), 12);
        assert_eq!(cache.item_count, 12);
        assert_eq!(state.logical_scroll_top().item_ix, 0);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 0.);
    }

    #[test]
    fn discover_feed_scroll_offset_uses_the_list_state_pixel_position() {
        let state =
            ListState::new(8, ListAlignment::Top, px(8.)).with_uniform_item_height(px(100.));
        state.scroll_to(gpui::ListOffset {
            item_ix: 3,
            offset_in_item: px(7.),
        });

        assert_eq!(
            state.scroll_px_offset_for_scrollbar(),
            point(px(0.), px(-307.))
        );
    }

    #[gpui::test]
    fn discover_feed_measured_rows_keep_extent_and_scroll_across_resizes(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::prelude::*;

        // Mixed natural row heights that also change with the width, like a
        // feed where one-line genre sections sit between two-line sections
        // and rows shrink at the narrow breakpoint.
        const TALL_ROWS: [f32; 10] = [50., 32., 50., 32., 50., 32., 50., 32., 50., 32.];
        const SHORT_ROWS: [f32; 10] = [40., 26., 40., 26., 40., 26., 40., 26., 40., 26.];
        struct FeedRows(ListState, [f32; 10]);
        impl gpui::Render for FeedRows {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                let state = self.0.clone();
                let row_heights = self.1;
                gpui::div().size_full().child(
                    gpui::list(state, move |ix, _, _| {
                        gpui::div()
                            .h(px(row_heights[ix]))
                            .w_full()
                            .into_any_element()
                    })
                    .w_full()
                    .h_full(),
                )
            }
        }

        let state = ListState::new(10, ListAlignment::Top, px(0.)).measure_all();
        let mut cache = DiscoverFeedCache {
            state: state.clone(),
            browser_scroll: BrowserScrollState::new(),
            content_identity: "feed-a".to_owned(),
            item_count: 10,
        };
        let cx = cx.add_empty_window();
        let view = cx.update(|_, cx| cx.new(|_| FeedRows(state.clone(), TALL_ROWS)));
        let draw = |cx: &mut gpui::VisualTestContext, width: f32, row_heights: [f32; 10]| {
            cx.update(|_, cx| {
                view.update(cx, |rows, _| rows.1 = row_heights);
            });
            cx.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(width), px(200.)),
                {
                    let view = view.clone();
                    move |_, _| view.into_any_element()
                },
            );
        };

        // Every row is measured in the first prepaint, so the extent is
        // exact from the first frame even with mixed heights.
        let tall_total: f32 = TALL_ROWS.iter().sum();
        draw(cx, 100., TALL_ROWS);
        assert_eq!(
            f32::from(state.max_offset_for_scrollbar().y),
            tall_total - 200.
        );
        assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), 0.);

        let tall_top_of_item_2: f32 = TALL_ROWS.iter().take(2).sum();
        state.scroll_to(gpui::ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        draw(cx, 100., TALL_ROWS);
        assert_eq!(
            f32::from(state.scroll_px_offset_for_scrollbar().y),
            -tall_top_of_item_2
        );

        // A width change wipes every measured height and re-arms the
        // measure pass within the same prepaint, so the extent is exact at
        // the new widths and heights immediately, with no collapsed
        // scrollbar frame in between. The item-anchored scroll position
        // survives and re-resolves against the new heights.
        let short_total: f32 = SHORT_ROWS.iter().sum();
        let short_top_of_item_2: f32 = SHORT_ROWS.iter().take(2).sum();
        draw(cx, 200., SHORT_ROWS);
        assert_eq!(
            f32::from(state.max_offset_for_scrollbar().y),
            short_total - 200.
        );
        assert_eq!(
            f32::from(state.scroll_px_offset_for_scrollbar().y),
            -short_top_of_item_2
        );
        assert_eq!(state.logical_scroll_top().item_ix, 2);
        assert_eq!(f32::from(state.logical_scroll_top().offset_in_item), 0.);

        // A content change resets the feed to the top and the next
        // prepaint re-measures every row, so scrolling keeps working.
        update_discover_feed_cache(&mut cache, "feed-b", 10);
        draw(cx, 200., SHORT_ROWS);
        assert_eq!(
            f32::from(state.max_offset_for_scrollbar().y),
            short_total - 200.
        );
        assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), 0.);
        assert_eq!(state.logical_scroll_top().item_ix, 0);
        state.scroll_by(px(100.));
        draw(cx, 200., SHORT_ROWS);
        assert_eq!(f32::from(state.scroll_px_offset_for_scrollbar().y), -100.);
    }

    #[test]
    fn active_vertical_scroll_offset_checks_the_discover_list_before_outer_scroll() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("fn active_vertical_scroll_offset")
            .and_then(|(_, rest)| rest.split_once("fn restore_card_grid_scroll"))
            .map(|(method, _)| method)
            .expect("active vertical scroll offset implementation");
        assert!(method.contains("self.detail.route.is_none()"));
        assert!(method.contains("discover_feed_scroll_offset"));
        assert!(method.contains("ResultState::Initial"));
    }
}

#[cfg(test)]
mod playback_selection_tests {
    #[test]
    fn soundcloud_selection_is_guarded_by_library_action_generation_and_queue_epoch() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(crate) fn play_soundcloud_discover_selection")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn start_soundcloud_artist_station"))
            .map(|(method, _)| method)
            .expect("SoundCloud selection playback method should remain focused");

        assert!(method.contains("begin_playback_action"));
        assert!(method.contains("is_playback_action_current(action_generation)"));
        assert!(method.contains("let queue_epoch = self.playback.read(cx).state.queue_epoch()"));
        assert!(method.contains("playback.read(cx).state.queue_epoch() != queue_epoch"));
    }
}

#[cfg(test)]
mod card_grid_tests {
    use super::*;

    #[test]
    fn card_cache_identity_includes_account_source_type_query_and_section() {
        let first = search_card_grid_cache_key(
            "account-a",
            Source::Deezer,
            ResultType::Albums,
            "one",
            "Albums",
        );
        assert_eq!(
            first,
            search_card_grid_cache_key(
                "account-a",
                Source::Deezer,
                ResultType::Albums,
                "one",
                "Albums"
            )
        );
        for (scope, source, result_type, query, section) in [
            (
                "account-b",
                Source::Deezer,
                ResultType::Albums,
                "one",
                "Albums",
            ),
            (
                "account-a",
                Source::SoundCloud,
                ResultType::Albums,
                "one",
                "Albums",
            ),
            (
                "account-a",
                Source::Deezer,
                ResultType::Artists,
                "one",
                "Albums",
            ),
            (
                "account-a",
                Source::Deezer,
                ResultType::Albums,
                "two",
                "Albums",
            ),
            (
                "account-a",
                Source::Deezer,
                ResultType::Albums,
                "one",
                "Artists",
            ),
        ] {
            assert_ne!(
                first,
                search_card_grid_cache_key(scope, source, result_type, query, section)
            );
        }
    }

    #[test]
    fn card_cache_invalidates_content_and_layout_signatures() {
        let layout = CardGridLayout::new(4, 800., 194., 260.);
        let cache = CardGridCache {
            state: ListState::new(3, ListAlignment::Top, px(520.)),
            browser_scroll: BrowserScrollState::new(),
            content_signature: 11,
            layout,
            row_count: 3,
            card_count: 12,
        };
        assert!(!card_grid_cache_needs_reset(&cache, 11, 12, 3, layout));
        assert!(card_grid_cache_needs_reset(&cache, 12, 12, 3, layout));
        assert!(card_grid_cache_needs_reset(
            &cache,
            11,
            12,
            3,
            CardGridLayout::new(5, 800., 154., 220.)
        ));
    }

    #[test]
    fn fresh_submit_clears_card_grid_states_even_for_the_same_results() {
        let submit = include_str!("view.rs")
            .split_once("pub(crate) fn submit")
            .and_then(|(_, rest)| rest.split_once("fn search_credentials"))
            .map_or_else(
                || panic!("search submit must remain before search credentials"),
                |(submit, _)| submit,
            );

        assert!(submit.contains("self.clear_card_grid_states();"));
        let clear_index = submit
            .find("self.clear_card_grid_states();")
            .expect("fresh submit must clear card grids");
        let run_index = submit
            .find("let job = self.state.submit(&query);")
            .expect("fresh submit must create a search job");
        assert!(clear_index < run_index);
    }

    fn view_select_type_section() -> &'static str {
        include_str!("view.rs")
            .split_once("pub(super) fn select_type")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn submit"))
            .map_or_else(
                || panic!("search select_type must remain before submit"),
                |(select_type, _)| select_type,
            )
    }

    #[test]
    fn tab_switch_reuses_per_tab_list_and_scroll_state() {
        let select_type = view_select_type_section();

        // Track lists and card grids stay cached across tabs: their keys
        // already include source, type, query, and content.
        assert!(!select_type.contains("self.clear_track_list_states();"));
        assert!(!select_type.contains("self.clear_card_grid_states();"));
        // Replacing the view scroll handle would remount the scroll surface
        // and jump to the top on every switch.
        assert!(!select_type.contains("ScrollHandle::new()"));
        assert!(select_type.contains("self.result_scroll_offsets"));
        assert!(select_type.contains("self.scroll.set_offset("));
        assert!(select_type.contains("self.run(job,"));
    }

    #[test]
    fn tab_switch_never_replays_the_results_entrance() {
        let select_type = view_select_type_section();
        assert!(select_type.contains("self.results_entrance_key = None;"));

        let run = include_str!("view.rs")
            .split_once("fn run(")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn open_card"))
            .map_or_else(
                || panic!("search run must remain before open_card"),
                |(run, _)| run,
            );
        // Only fresh completions re-arm the entrance. Cached tab switches
        // restore synchronously without completing.
        assert!(run.contains("this.results_entrance_key = Some("));
        assert!(run.contains("ResultState::Results"));
    }

    #[test]
    fn fresh_submit_resets_per_tab_scroll_and_entrance_state() {
        let submit = include_str!("view.rs")
            .split_once("pub(crate) fn submit")
            .and_then(|(_, rest)| rest.split_once("fn search_credentials"))
            .map_or_else(
                || panic!("search submit must remain before search credentials"),
                |(submit, _)| submit,
            );

        assert!(submit.contains("self.result_scroll_offsets.clear();"));
        assert!(submit.contains("self.results_entrance_key = None;"));
    }

    #[test]
    fn discover_stays_visible_for_uncommitted_input_only() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(super) fn should_show_discover")
            .and_then(|(_, rest)| rest.split_once("pub(super) fn ensure_discover"))
            .map(|(method, _)| method)
            .expect("discover visibility helper should remain focused");
        assert!(!method.contains("input.read(cx)"));
        assert!(method.contains("self.search_query.is_empty()"));
        assert!(method.contains("ResultState::Initial"));
    }

    #[test]
    fn discover_home_reset_clears_input_through_the_fresh_submit_path() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(crate) fn return_to_discover_home")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn remove_search_history"))
            .map(|(method, _)| method)
            .expect("discover home reset should remain near query controls");
        assert!(method.contains("set_value(\"\", window, cx)"));
        assert!(method.contains("self.submit(cx);"));
        let submit = source
            .split_once("pub(crate) fn submit")
            .and_then(|(_, rest)| rest.split_once("fn search_credentials"))
            .expect("fresh submit should remain available")
            .0;
        assert!(submit.contains("self.detail.reset();"));
        assert!(submit.contains("self.result_scroll_offsets.clear();"));
    }
}

#[cfg(test)]
mod playlist_update_tests {
    #[test]
    fn playlist_update_sync_patches_loaded_detail_without_reloading() {
        let source = include_str!("view.rs");
        let method = source
            .split_once("pub(crate) fn sync_playlist_update")
            .and_then(|(_, rest)| rest.split_once("pub(crate) fn sync_playlist_content"))
            .map(|(method, _)| method)
            .expect("playlist update sync method should remain present");
        assert!(method.contains("DetailState::Results(page) | DetailState::Empty(page)"));
        assert!(!method.contains("detail.reload()"));
        assert!(!method.contains("run_detail("));
        assert!(method.contains("page.description = description"));
        assert!(method.contains("cx.notify()"));
    }
}
