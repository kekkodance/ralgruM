use std::{cell::RefCell, collections::HashMap, sync::Arc};

use gpui::{AppContext, Context, Entity, FocusHandle, ScrollHandle, Window};
use gpui_component::input::{InputEvent, InputState};
use tokio::runtime::Runtime;

use crate::{
    browser_scroll::BrowserScrollState,
    downloads::DownloadModel,
    library::{FavoriteKey, FavoriteState, LibraryView},
    motion::{ResponsiveModeMotion, SegmentedSelectorMotion},
    music_ui::{CardCarouselState, CardGridMotion, ResizeSettledColumns, ResizeSettledTarget},
    playback::PlaybackModel,
    playing_indicator::PlayingSnapshot,
    settings::{AccountState, SettingsView},
};

use super::{
    SEARCH_PLACEHOLDER,
    album_info_cache::AlbumInfoPrefetch,
    card_actions::FavoriteAction,
    client::SearchClient,
    detail::DetailNavigation,
    discover::DiscoverState,
    feed_states::{CardGridCache, DiscoverFeedCache, TrackListCache},
    models::{Provider, ResultType, SearchState},
    playlist_sync::PlaylistReorderSnapshot,
    search_state::{ActiveRequest, ActiveSearchRequest},
    suggestions::SuggestionState,
};

// Composition re-exports: sibling modules and card menus consume these
// through `view::` paths.
pub(super) use super::{card_actions::favorite_kind, feed_states::CardGridLayout};

#[cfg(test)]
use super::card_actions::detail_favorite_seeds;
#[cfg(test)]
use super::detail_navigation::{
    SearchNavigationBackTarget, search_navigation_back_target, search_results_root_visible,
    should_close_discover_channel_before_detail,
};
#[cfg(test)]
use super::discover_actions::discover_provider_has_credentials;
#[cfg(test)]
use super::feed_states::{
    card_grid_cache_needs_reset, search_card_grid_cache_key, update_discover_feed_cache,
};
#[cfg(test)]
use super::models::{Card, ResultState, Source, Track};
#[cfg(test)]
use super::search_state::all_search_missing_accounts;
#[cfg(test)]
use crate::library::FavoriteKind;
#[cfg(test)]
use gpui::{ListAlignment, ListState, point};

pub(crate) struct SearchView {
    pub(crate) input: Entity<InputState>,
    pub(super) runtime: Arc<Runtime>,
    pub(super) client: Result<SearchClient, super::models::ProviderError>,
    pub(super) state: SearchState,
    pub(super) account: Entity<AccountState>,
    pub(super) settings: Entity<SettingsView>,
    pub(super) library: Entity<LibraryView>,
    pub(crate) favorites: Entity<FavoriteState>,
    pub(crate) playback: Entity<PlaybackModel>,
    pub(super) downloads: Entity<DownloadModel>,
    pub(super) account_scope: String,
    pub(super) detail: DetailNavigation,
    pub(super) type_tab_focus: Vec<FocusHandle>,
    pub(super) type_tab_motion: SegmentedSelectorMotion,
    pub(super) result_count_responsive: ResponsiveModeMotion,
    pub(super) last_result_count_label: String,
    pub(super) scroll: ScrollHandle,
    pub(super) browser_scroll: BrowserScrollState,
    /// Outer scroll offsets kept per result tab. Tab switches restore the
    /// returning tab's viewport instead of resetting shared scroll state.
    pub(super) result_scroll_offsets: HashMap<ResultType, gpui::Point<gpui::Pixels>>,
    /// Entrance animation key armed only when fresh results complete. Tab
    /// switches re-present cached results and must not replay it.
    pub(super) results_entrance_key: Option<String>,
    pub(super) search_active: bool,
    pub(super) playing: PlayingSnapshot,
    pub(super) playlist_update_revision: u64,
    pub(super) soundcloud_playlist_update_revision: u64,
    pub(super) playlist_content_revision: u64,
    pub(super) playlist_delete_revision: u64,
    pub(super) soundcloud_playlist_delete_revision: u64,
    pub(super) playlist_remove_revision: u64,
    pub(super) playlist_reorder_provider: Option<Provider>,
    pub(super) playlist_reorder_revision: u64,
    pub(super) playlist_reorder_snapshot: Option<PlaylistReorderSnapshot>,
    pub(super) playlist_drag_scroll: Option<crate::library::playlist_drag::PlaylistDragAutoScroll>,
    pub(super) playlist_drag_scroll_running: bool,
    pub(super) pending_forward_detail_scroll_reset: Option<u64>,
    pub(super) card_scroll_handles: RefCell<HashMap<String, CardCarouselState>>,
    pub(super) card_columns: ResizeSettledColumns,
    pub(super) card_grid_motion: CardGridMotion,
    pub(super) track_list_states: RefCell<HashMap<String, TrackListCache>>,
    pub(super) card_grid_states: RefCell<HashMap<String, CardGridCache>>,
    pub(super) discover_feed_states: RefCell<HashMap<String, DiscoverFeedCache>>,
    pub(super) search_query: String,
    pub(super) favorite_actions: HashMap<FavoriteKey, FavoriteAction>,
    pub(super) album_info_prefetch: AlbumInfoPrefetch,
    pub(super) suggestions: SuggestionState,
    pub(super) discover: DiscoverState,
    pub(super) search_request: Option<ActiveSearchRequest>,
    pub(super) deezer_ai_enrichment_request: Option<ActiveRequest>,
    pub(super) artist_page_ai_request: Option<ActiveRequest>,
    pub(super) suggestion_request: Option<ActiveRequest>,
    pub(super) discover_requests: HashMap<Provider, ActiveRequest>,
    pub(super) discover_channel_request: Option<ActiveRequest>,
    pub(super) smart_mix_enrichment_request: Option<ActiveRequest>,
    pub(super) smart_mix_enrichment_started_generation: Option<u64>,
    pub(super) smart_mix_enrichment_was_cancelled: bool,
    pub(super) next_request_id: u64,
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
        self.cancel_artist_page_ai_request();
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
            deezer_ai_enrichment_request: None,
            artist_page_ai_request: None,
            suggestion_request: None,
            discover_requests: HashMap::new(),
            discover_channel_request: None,
            smart_mix_enrichment_request: None,
            smart_mix_enrichment_started_generation: None,
            smart_mix_enrichment_was_cancelled: false,
            next_request_id: 0,
        }
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
#[path = "view_tests.rs"]
mod tests;
