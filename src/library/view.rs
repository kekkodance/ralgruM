use super::{
    client::LibraryClient,
    deezer_radio::{DeezerRadioBatch, FlowMode},
    favorite_state::{FavoriteKey, FavoriteKind, FavoriteState},
    local_persistence::{
        LocalLibraryMutation, LocalLibraryMutationOutcome, LocalLibraryMutationResponse,
    },
    model::{Card, Category, Page, Route, Service, Track, is_deezer_flow_detail, is_detail_route},
    playlist_client::PlaylistClient,
    soundcloud_client::SoundCloudLibraryClient,
    state::LibraryState,
    virtualization::{self, CardGridLayout, TrackListLayout},
};
use crate::{
    app_tooltip::AppTooltipExt,
    assets::LocalIcon,
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    downloads::DownloadModel,
    motion::SegmentedSelectorMotion,
    music_ui::{
        CardCarouselState, CardGridMotion, ResizeSettledColumns, ResizeSettledTarget,
        category_tab_tooltip, category_tabs_icon_only, effective_content_width,
    },
    playback::{
        DeezerFlowKind, ExtensionObserverKey, PlaybackContext, PlaybackModel, PlaybackTrack,
        QueueExtensionTicket, duplicate_retry_delay, should_extend_at_tail,
    },
    playing_indicator::PlayingSnapshot,
    search::Provider,
    settings::{AccountState, Category as SettingsCategory, SettingsView},
    smart_mix_title::specific_smart_mix_title,
    theme::{BORDER, FOREGROUND, MUTED, SURFACE_RAISED},
};
use gpui::{
    AnimationExt as _, AnyElement, Context, ElementId, Entity, EventEmitter, FocusHandle,
    FontWeight, IntoElement, KeyDownEvent, ListAlignment, ListOffset, ListState, Pixels, Render,
    ScrollHandle, StatefulInteractiveElement, Subscription, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::{
    input::{InputEvent, InputState},
    scroll::ScrollableElement,
};
use std::{cell::RefCell, collections::HashMap, sync::Arc, time::Instant};
use tokio::runtime::Runtime;

pub(crate) use super::local_playlist_view::{local_playlist_page, local_playlists_page};

const FLOW_REQUEST_FAILURE: &str = "Flow request failed";
const LIBRARY_CONTENT_BOTTOM_PADDING_PX: f32 = 16.;

type LocalLibraryCompletion = Box<
    dyn FnOnce(
            Result<LocalLibraryMutationOutcome, super::local_store::LocalLibraryError>,
            &mut LibraryView,
            &mut Context<LibraryView>,
        ) + Send,
>;

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;

enum Credential {
    Deezer(crate::search::DeezerArl),
    SoundCloud(crate::search::SoundCloudToken),
}

struct ExtensionBatch {
    tracks: Vec<Track>,
    provider: Provider,
    clear_remaining_tracks: bool,
    next_flow_tuner: Option<super::deezer_radio::FlowTuner>,
    continuation_seed: Option<String>,
}

enum DeezerMixKind {
    Track(String),
    Artist(String),
}

impl DeezerMixKind {
    fn context(&self) -> PlaybackContext {
        match self {
            Self::Track(track_id) => PlaybackContext::DeezerTrackMix {
                seed_track_id: track_id.clone(),
            },
            Self::Artist(artist_id) => PlaybackContext::DeezerArtistMix {
                seed_artist_id: artist_id.clone(),
            },
        }
    }
}

impl ExtensionBatch {
    fn from_deezer(batch: DeezerRadioBatch) -> Self {
        Self {
            tracks: batch.tracks,
            provider: Provider::Deezer,
            clear_remaining_tracks: batch.clear_remaining_tracks,
            next_flow_tuner: batch.next_flow_tuner,
            continuation_seed: None,
        }
    }

    fn from_soundcloud(page: Page) -> Self {
        let continuation_seed = page
            .tracks
            .last()
            .map(|track| track.id.trim().to_owned())
            .filter(|id| !id.is_empty());
        Self {
            tracks: page.tracks,
            provider: Provider::SoundCloud,
            clear_remaining_tracks: false,
            next_flow_tuner: None,
            continuation_seed,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LibraryEvent {
    SettingsOpened,
    SelectionChanged,
    NavigationRequested,
    DiscoverRequested,
    SmartMixTitleResolved { config_id: String, title: String },
}

fn scope_reload(state: &mut LibraryState, scope: String) -> Option<(Service, Category)> {
    state
        .set_account_scope(scope)
        .then_some((state.service, state.category))
}

fn favorite_kind_for_root(category: Category) -> Option<FavoriteKind> {
    match category {
        Category::Tracks => Some(FavoriteKind::Track),
        Category::Albums => Some(FavoriteKind::Album),
        Category::Artists => Some(FavoriteKind::Artist),
        Category::Playlists => Some(FavoriteKind::Playlist),
        Category::History | Category::Flow | Category::MyTracks | Category::Station => None,
    }
}

fn root_favorite_ids(category: Category, page: &Page) -> Vec<String> {
    let Some(_) = favorite_kind_for_root(category) else {
        return Vec::new();
    };
    if category == Category::Tracks {
        page.tracks
            .iter()
            .map(|track| track.id.clone())
            .filter(|id| !id.trim().is_empty())
            .collect()
    } else {
        page.cards
            .iter()
            .map(|card| card.id.clone())
            .filter(|id| !id.trim().is_empty())
            .collect()
    }
}

fn root_favorite_keys(service: Service, category: Category, page: &Page) -> Vec<FavoriteKey> {
    if service == Service::Local {
        return Vec::new();
    }
    let Some(kind) = favorite_kind_for_root(category) else {
        return Vec::new();
    };
    let provider = match service {
        Service::Local => unreachable!("Local items are not provider favorites"),
        Service::Deezer => Provider::Deezer,
        Service::SoundCloud => Provider::SoundCloud,
    };
    root_favorite_ids(category, page)
        .into_iter()
        .map(|id| FavoriteKey::for_provider(provider, kind, id))
        .collect()
}

fn category_focus_slot_count() -> usize {
    Service::Deezer
        .categories()
        .len()
        .max(Service::SoundCloud.categories().len())
}

fn discover_flow_return_active(
    origin: Option<(Service, Category)>,
    service: Service,
    route: &Route,
    route_depth: usize,
) -> bool {
    origin.is_some() && is_deezer_flow_detail(service, route, route_depth)
}

fn active_deezer_flow_kind(context: &PlaybackContext, config_id: &str) -> Option<DeezerFlowKind> {
    match context {
        PlaybackContext::DeezerFlow {
            config_id: active,
            kind,
            ..
        } if active == config_id => Some(kind.clone()),
        _ => None,
    }
}

fn reuse_cached_flow_page(flow_kind: &DeezerFlowKind, cached: Option<&Page>) -> bool {
    *flow_kind == DeezerFlowKind::SmartMix && cached.is_some()
}

fn smart_mix_title_event(config_id: &str, title: &str) -> Option<LibraryEvent> {
    let config_id = config_id.trim();
    let title = specific_smart_mix_title(title)?;
    (!config_id.is_empty()).then_some(())?;
    Some(LibraryEvent::SmartMixTitleResolved {
        config_id: config_id.to_owned(),
        title: title.to_owned(),
    })
}

pub(super) fn card_route(card: Card) -> Option<Route> {
    let local = card.library_service == Some(Service::Local);
    let action = if local && card.kind == Category::Playlists {
        "localPlaylistTracks"
    } else {
        match card.kind {
            Category::Albums => "albumTracks",
            Category::Artists if card.source == crate::search::Provider::SoundCloud => {
                "artistTracks"
            }
            Category::Artists => "artist",
            Category::Playlists => "playlistTracks",
            Category::Flow => "flowTracks",
            Category::Station => "stationTracks",
            _ => return None,
        }
    };
    if card.id.is_empty() {
        return None;
    }
    Some(Route {
        source: if local {
            crate::search::Provider::Deezer
        } else {
            card.source
        },
        category: card.kind,
        action: action.into(),
        id: card.id,
        title: card.title,
        subtitle: card.subtitle,
        artwork: card.artwork,
        release_date: if card.kind == Category::Albums {
            if card.release_date.trim().is_empty() {
                card.badge
            } else {
                card.release_date
            }
        } else {
            String::new()
        },
    })
}

pub(crate) struct LibraryView {
    pub(crate) input: Entity<InputState>,
    pub(super) account: Entity<AccountState>,
    settings: Entity<SettingsView>,
    pub(super) runtime: Arc<Runtime>,
    pub(super) client: Result<LibraryClient, String>,
    pub(super) tracks_cache: super::tracks_cache::DeezerTracksCache,
    pub(super) local_store:
        Result<super::local_store::LocalLibraryStore, super::local_store::LocalLibraryError>,
    pub(super) local_playlists: Result<
        super::local_playlist_store::LocalPlaylistStore,
        super::local_playlist_store::LocalPlaylistError,
    >,
    pub(super) local_persistence: super::local_persistence::LocalStorageWorker,
    _local_persistence_quit: Subscription,
    pub(super) local_library_revision: u64,
    pub(super) local_playlist_revision: u64,
    local_library_reorder_pending: bool,
    local_playlist_reorder_pending: bool,
    soundcloud_client: Result<SoundCloudLibraryClient, String>,
    pub(super) playlist_client: Result<PlaylistClient, String>,
    pub(crate) playlists: super::playlist_state::PlaylistState,
    pub(crate) soundcloud_playlists: super::playlist_state::PlaylistState,
    pub(super) state: LibraryState,
    library_load_cancel: Option<tokio::task::AbortHandle>,
    detail_load_cancel: Option<tokio::task::AbortHandle>,
    favorite_catalog_cancels: HashMap<FavoriteKind, FavoriteCatalogRequest>,
    pub(super) deezer_tracks_retry: Option<DeezerTracksRetry>,
    pub(super) favorites: Entity<FavoriteState>,
    pub(crate) playback: Entity<PlaybackModel>,
    pub(super) downloads: Entity<DownloadModel>,
    deezer_actions: super::deezer_actions::DeezerActionState,
    pub(super) playing: PlayingSnapshot,
    category_tab_focus: Vec<FocusHandle>,
    category_motion: SegmentedSelectorMotion,
    category_tabs_responsive: crate::motion::ResponsiveModeMotion,
    card_scroll_handles: RefCell<HashMap<String, CardCarouselState>>,
    pub(super) card_columns: ResizeSettledColumns,
    pub(super) card_grid_motion: CardGridMotion,
    pub(super) card_available_width: f32,
    track_list_states: RefCell<HashMap<String, TrackListCache>>,
    pub(super) playlist_drag_scroll: Option<super::playlist_drag::PlaylistDragAutoScroll>,
    pub(super) playlist_drag_scroll_running: bool,
    card_grid_states: RefCell<HashMap<String, CardGridCache>>,
    pub(super) scroll: ScrollHandle,
    pub(super) browser_scroll: BrowserScrollState,
    pub(super) similar_artists: super::deezer_similar::SimilarArtistsNavigation,
    pub(super) artist_section_expanded: Option<usize>,
    pub(super) flow_mode: FlowMode,
    pub(super) flow_catalog_option: Option<String>,
    flow_detail_kind: DeezerFlowKind,
    discover_flow_return: Option<(Service, Category)>,
    pub(super) external_track_navigation:
        Option<crate::entity_navigation::ProviderNavigationOpeners>,
    extension_observer_key: Option<ExtensionObserverKey>,
    playback_action_generation: u64,
    album_info_prefetch: crate::search::AlbumInfoPrefetch,
}

pub(super) enum DeezerTracksRetry {
    Initial {
        arl: crate::search::DeezerArl,
    },
    Continuation {
        token: u64,
        continuation: super::client::DeezerTracksContinuation,
    },
    Hydration {
        token: u64,
        hydration: super::client::DeezerTracksHydration,
        key: Option<super::tracks_cache::DeezerTracksCacheKey>,
    },
}

pub(super) struct FavoriteCatalogRequest {
    generation: u64,
    abort: tokio::task::AbortHandle,
}

struct TrackListCache {
    state: ListState,
    browser_scroll: BrowserScrollState,
    shape: (usize, TrackListLayout),
    row_identities: Vec<String>,
}

struct CardGridCache {
    state: ListState,
    browser_scroll: BrowserScrollState,
    item_count: usize,
    layout: CardGridLayout,
    uniform_height: Pixels,
    ordered_content_identity: Vec<String>,
    card_count: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CardGridChange {
    Unchanged,
    Remeasure,
    Reflow,
    Reset,
}

fn card_grid_anchor(
    state: &ListState,
    previous_count: usize,
    previous_layout: CardGridLayout,
    previous_uniform_height: Pixels,
    next_count: usize,
    next_layout: CardGridLayout,
    next_uniform_height: Pixels,
    previous_card_count: Option<usize>,
) -> Option<ListOffset> {
    if previous_count == 0 || next_count == 0 {
        return None;
    }
    let current = state.logical_scroll_top();
    let item_ix = if previous_layout.columns == next_layout.columns {
        current.item_ix.min(next_count)
    } else {
        let card_count = previous_card_count?;
        if current.item_ix >= previous_count {
            next_count
        } else {
            let card_index = current
                .item_ix
                .saturating_mul(previous_layout.columns)
                .min(card_count);
            (card_index / next_layout.columns).min(next_count)
        }
    };
    let fraction = if f32::from(previous_uniform_height) > 0. {
        (f32::from(current.offset_in_item) / f32::from(previous_uniform_height)).clamp(0., 1.)
    } else {
        0.
    };
    Some(ListOffset {
        item_ix,
        offset_in_item: px(fraction * f32::from(next_uniform_height)),
    })
}

fn card_grid_change(
    previous_count: usize,
    previous_layout: CardGridLayout,
    previous_identity: &[String],
    next_count: usize,
    next_layout: CardGridLayout,
    next_identity: &[String],
) -> CardGridChange {
    if previous_identity != next_identity {
        return CardGridChange::Reset;
    }
    if previous_count == next_count && previous_layout == next_layout {
        return CardGridChange::Unchanged;
    }
    if previous_count != next_count || previous_layout.columns != next_layout.columns {
        CardGridChange::Reflow
    } else {
        CardGridChange::Remeasure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrackListChange {
    Unchanged,
    Append { old_count: usize, added: usize },
    Reset,
}

fn track_list_change(
    previous_rows: &[String],
    previous_shape: (usize, TrackListLayout),
    next_rows: &[String],
    next_layout: TrackListLayout,
) -> TrackListChange {
    if previous_shape.0 != previous_rows.len() || previous_shape.1 != next_layout {
        return TrackListChange::Reset;
    }
    if previous_rows == next_rows {
        return TrackListChange::Unchanged;
    }
    if next_rows.len() > previous_rows.len() && next_rows.starts_with(previous_rows) {
        return TrackListChange::Append {
            old_count: previous_rows.len(),
            added: next_rows.len() - previous_rows.len(),
        };
    }
    TrackListChange::Reset
}

const MAX_TRACK_LIST_STATES: usize = 24;
const MAX_CARD_GRID_STATES: usize = 24;

impl EventEmitter<LibraryEvent> for LibraryView {}

impl crate::search::AlbumInfoCacheHost for LibraryView {
    fn album_info_generation(&self) -> u64 {
        self.album_info_prefetch.generation()
    }

    fn store_album_info_page(&mut self, page: &crate::search::DetailPage) {
        self.album_info_prefetch.store_page(page.clone());
    }
}

impl LibraryView {
    pub(crate) fn open_card_info(
        &mut self,
        card: Card,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(search_card) = card.search_card() else {
            return;
        };
        let account = self.account.read(cx);
        let prefetched = self.prefetched_card_info(&card);
        crate::search::open_card_info_dialog_with_prefetch(
            search_card,
            self.runtime.clone(),
            crate::search::SearchClient::new().ok(),
            account.deezer_arl(),
            account.soundcloud_token(),
            prefetched,
            window,
            cx,
        );
    }

    fn prefetched_card_info(
        &self,
        card: &Card,
    ) -> Option<(crate::search::DetailRoute, crate::search::AlbumInfo)> {
        let search_card = card.search_card()?;
        let route = crate::search::DetailRoute::from_card(&search_card)?;
        if let Some(page) = self.state.page.as_ref()
            && let Some(info) = page.album_info.as_ref().filter(|info| info.has_content())
        {
            let current = self.state.route();
            if current.id == card.id
                && current.source == card.source
                && matches!(
                    (current.category, route.kind),
                    (Category::Albums, crate::search::ResultType::Albums)
                        | (Category::Playlists, crate::search::ResultType::Playlists)
                )
            {
                return Some((
                    crate::search::DetailRoute {
                        provider: current.source,
                        kind: route.kind,
                        id: current.id.clone(),
                        title: current.title.clone(),
                        subtitle: current.subtitle.clone(),
                        artwork: current.artwork.clone(),
                        release_date: current.release_date.clone(),
                    },
                    info.clone(),
                ));
            }
        }
        self.album_info_prefetch.cached(&route)
    }

    /// Starts a background detail fetch for a library album or playlist card
    /// so the Info dialog can open already populated. Called when a context
    /// menu opens and when a card is hovered. Cached and in-flight requests
    /// are skipped. The dialog still fetches on its own when prefetch has not
    /// completed.
    pub(crate) fn prefetch_card_info(&mut self, card: Card, cx: &mut Context<Self>) {
        let Some(search_card) = card.search_card() else {
            return;
        };
        let Some(key) = crate::search::AlbumInfoCacheKey::from_card(&search_card) else {
            return;
        };
        let Some(route) = crate::search::DetailRoute::from_card(&search_card) else {
            return;
        };
        if self.album_info_prefetch.cached(&route).is_some() {
            return;
        }
        if self.prefetched_card_info(&card).is_some() {
            if let Some(page) = self.state.page.as_ref().and_then(|page| {
                let current = self.state.route();
                (current.id == card.id && current.source == card.source)
                    .then(|| page.clone())
                    .filter(|page| {
                        page.album_info
                            .as_ref()
                            .is_some_and(|info| info.has_content())
                    })
            }) {
                let detail_page = crate::search::DetailPage {
                    route: route.clone(),
                    tracks: Vec::new(),
                    total: Some(page.total),
                    raw_loaded_count: page.raw_loaded_count,
                    normalized_count: page.normalized_count,
                    authoritative_total: page.authoritative_total,
                    artist: None,
                    description: page.description.clone(),
                    album_info: page.album_info.clone(),
                };
                self.album_info_prefetch.store_page(detail_page);
            }
            return;
        }
        if !self.album_info_prefetch.begin(key.clone()) {
            return;
        }
        let account = self.account.read(cx);
        let account_scope = account.library_scope();
        let prefetch_generation = self.album_info_prefetch.generation();
        let (deezer_arl, soundcloud_token) = (account.deezer_arl(), account.soundcloud_token());
        let has_account = match route.provider {
            crate::search::Provider::Deezer => deezer_arl.is_some(),
            crate::search::Provider::SoundCloud => soundcloud_token.is_some(),
        };
        if !has_account {
            self.album_info_prefetch.fail(&key);
            return;
        }
        let Ok(client) = crate::search::SearchClient::new() else {
            self.album_info_prefetch.fail(&key);
            return;
        };
        let task = self
            .runtime
            .spawn(async move { client.detail(route, deezer_arl, soundcloud_token).await });
        cx.spawn(async move |this, cx| {
            let Ok(result) = task.await else {
                this.update(cx, |this, cx| {
                    if this.album_info_prefetch.generation() == prefetch_generation
                        && this.account.read(cx).library_scope() == account_scope
                    {
                        this.album_info_prefetch.fail(&key);
                    }
                })
                .ok();
                return;
            };
            this.update(cx, |this, cx| {
                if this.album_info_prefetch.generation() != prefetch_generation
                    || this.account.read(cx).library_scope() != account_scope
                {
                    return;
                }
                match result {
                    Ok(page) => this.album_info_prefetch.complete(key, page),
                    Err(_) => this.album_info_prefetch.fail(&key),
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn new(
        account: Entity<AccountState>,
        settings: Entity<SettingsView>,
        favorites: Entity<FavoriteState>,
        runtime: Arc<Runtime>,
        playback: Entity<PlaybackModel>,
        downloads: Entity<DownloadModel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search this library page..."));
        cx.subscribe(&input, move |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        // Track rows follow the current track and the explicit-block
        // preference without repainting on the position poll.
        cx.observe(&playback, |this, playback, cx| {
            let (snapshot, key, should_extend, in_flight) = {
                let playback_model = playback.read(cx);
                let playback_state = &playback_model.state;
                (
                    PlayingSnapshot::from_playback(playback_state),
                    playback_model.extension_observer_key(),
                    matches!(
                        playback_state.status,
                        crate::playback::PlaybackStatus::Playing
                            | crate::playback::PlaybackStatus::Paused
                    ) && should_extend_at_tail(playback_state),
                    playback_model.extension_in_flight(),
                )
            };
            if this.playing != snapshot {
                this.playing = snapshot;
                cx.notify();
            }
            if should_extend && !in_flight && this.extension_observer_key.as_ref() != Some(&key) {
                this.extension_observer_key = Some(key);
                let playback = playback.clone();
                cx.spawn(async move |this, cx| {
                    this.update(cx, |this, cx| {
                        this.extend_playback_queue(playback, cx);
                    })
                    .ok();
                })
                .detach();
            } else if !should_extend {
                this.extension_observer_key = Some(key);
            }
        })
        .detach();
        let playing = PlayingSnapshot::from_playback(&playback.read(cx).state);
        let account_scope = account.read(cx).library_scope();
        let playlists = super::playlist_state::PlaylistState::new(account_scope.clone());
        let soundcloud_playlists = super::playlist_state::PlaylistState::new(account_scope.clone());
        let local_store = super::local_store::LocalLibraryStore::load_current_user();
        let local_playlists = super::local_playlist_store::LocalPlaylistStore::load_current_user();
        let local_persistence = super::local_persistence::LocalStorageWorker::new(
            local_store.clone(),
            local_playlists.clone(),
        );
        let persistence_for_quit = local_persistence.clone();
        let local_persistence_quit = cx.on_app_quit(move |_, _| {
            let persistence = persistence_for_quit.clone();
            async move { persistence.flush().await }
        });
        Self {
            input,
            account,
            settings,
            runtime,
            client: LibraryClient::new(),
            tracks_cache: super::tracks_cache::DeezerTracksCache::current_user(),
            local_store,
            local_playlists,
            local_persistence,
            _local_persistence_quit: local_persistence_quit,
            local_library_revision: 0,
            local_playlist_revision: 0,
            local_library_reorder_pending: false,
            local_playlist_reorder_pending: false,
            soundcloud_client: SoundCloudLibraryClient::new(),
            playlist_client: PlaylistClient::new(),
            playlists,
            soundcloud_playlists,
            state: LibraryState::default(),
            library_load_cancel: None,
            detail_load_cancel: None,
            favorite_catalog_cancels: HashMap::new(),
            deezer_tracks_retry: None,
            favorites,
            playback,
            downloads,
            deezer_actions: super::deezer_actions::DeezerActionState::new(account_scope),
            playing,
            category_tab_focus: (0..category_focus_slot_count())
                .map(|_| cx.focus_handle())
                .collect(),
            category_motion: SegmentedSelectorMotion::default(),
            category_tabs_responsive: crate::motion::ResponsiveModeMotion::default(),
            card_scroll_handles: RefCell::new(HashMap::new()),
            card_columns: ResizeSettledColumns::new(),
            card_grid_motion: CardGridMotion::default(),
            card_available_width: 0.,
            track_list_states: RefCell::new(HashMap::new()),
            playlist_drag_scroll: None,
            playlist_drag_scroll_running: false,
            card_grid_states: RefCell::new(HashMap::new()),
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            similar_artists: super::deezer_similar::SimilarArtistsNavigation::default(),
            artist_section_expanded: None,
            flow_mode: FlowMode::Default,
            flow_catalog_option: None,
            flow_detail_kind: DeezerFlowKind::Flow,
            discover_flow_return: None,
            external_track_navigation: None,
            extension_observer_key: None,
            playback_action_generation: 0,
            album_info_prefetch: crate::search::AlbumInfoPrefetch::default(),
        }
    }

    pub(super) fn seed_loaded_root_favorites(
        &mut self,
        service: Service,
        category: Category,
        cx: &mut Context<Self>,
    ) {
        if !self.state.selected_root_active(category) {
            return;
        }
        let Some(page) = self.state.page.as_ref() else {
            return;
        };
        let keys = root_favorite_keys(service, category, page);
        if keys.is_empty() {
            return;
        }
        self.favorites.update(cx, |favorites, _| {
            for key in keys {
                favorites.set_known(key, true);
            }
        });
    }

    pub(super) fn track_list_state_with_rows(
        &self,
        identity: &str,
        count: usize,
        layout: TrackListLayout,
        row_identities: &[String],
    ) -> ListState {
        let mut states = self.track_list_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            match track_list_change(&cache.row_identities, cache.shape, row_identities, layout) {
                TrackListChange::Unchanged => {}
                TrackListChange::Append { old_count, added } => {
                    cache.state.splice(old_count..old_count, added);
                    cache.state = cache
                        .state
                        .clone()
                        .with_uniform_item_height(virtualization::row_height());
                }
                TrackListChange::Reset => {
                    cache
                        .state
                        .reset_with_uniform_height(count, virtualization::row_height());
                }
            }
            cache.shape = (count, layout);
            cache.row_identities = row_identities.to_owned();
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
                row_identities: row_identities.to_owned(),
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

    pub(super) fn card_grid_state_with_rows(
        &self,
        identity: &str,
        count: usize,
        layout: CardGridLayout,
        ordered_content_identity: &[String],
        card_count: Option<usize>,
    ) -> ListState {
        self.card_grid_state_with_uniform_height(
            identity,
            count,
            layout,
            layout.row_height,
            ordered_content_identity,
            card_count,
        )
    }

    pub(super) fn mixed_page_state_with_rows(
        &self,
        identity: &str,
        count: usize,
        layout: CardGridLayout,
        page_items: &[virtualization::PageItem],
        ordered_content_identity: &[String],
    ) -> ListState {
        let uniform_height = virtualization::page_item_uniform_height(page_items, layout);
        self.card_grid_state_with_uniform_height(
            identity,
            count,
            layout,
            uniform_height,
            ordered_content_identity,
            None,
        )
    }

    fn card_grid_state_with_uniform_height(
        &self,
        identity: &str,
        count: usize,
        layout: CardGridLayout,
        uniform_height: Pixels,
        ordered_content_identity: &[String],
        card_count: Option<usize>,
    ) -> ListState {
        let mut states = self.card_grid_states.borrow_mut();
        if let Some(cache) = states.get_mut(identity) {
            let change = card_grid_change(
                cache.item_count,
                cache.layout,
                &cache.ordered_content_identity,
                count,
                layout,
                ordered_content_identity,
            );
            match change {
                CardGridChange::Unchanged if cache.uniform_height == uniform_height => {}
                CardGridChange::Reset => {
                    cache.state.reset_with_uniform_height(count, uniform_height);
                    cache.browser_scroll.reset();
                }
                CardGridChange::Unchanged | CardGridChange::Remeasure | CardGridChange::Reflow => {
                    let anchor = card_grid_anchor(
                        &cache.state,
                        cache.item_count,
                        cache.layout,
                        cache.uniform_height,
                        count,
                        layout,
                        uniform_height,
                        cache.card_count,
                    );
                    cache.state.reset_with_uniform_height(count, uniform_height);
                    if let Some(anchor) = anchor {
                        cache.state.scroll_to(anchor);
                    }
                    cache.browser_scroll.reset();
                }
            }
            cache.item_count = count;
            cache.layout = layout;
            cache.uniform_height = uniform_height;
            cache.ordered_content_identity = ordered_content_identity.to_owned();
            cache.card_count = card_count;
            return cache.state.clone();
        }
        if states.len() >= MAX_CARD_GRID_STATES
            && let Some(oldest) = states.keys().next().cloned()
        {
            states.remove(&oldest);
        }
        let state = ListState::new(count, ListAlignment::Top, layout.overdraw())
            .with_uniform_item_height(uniform_height);
        states.insert(
            identity.to_owned(),
            CardGridCache {
                state: state.clone(),
                browser_scroll: BrowserScrollState::new(),
                item_count: count,
                layout,
                uniform_height,
                ordered_content_identity: ordered_content_identity.to_owned(),
                card_count,
            },
        );
        state
    }

    pub(super) fn card_grid_browser_scroll(&self, identity: &str) -> BrowserScrollState {
        self.card_grid_states
            .borrow()
            .get(identity)
            .map(|cache| cache.browser_scroll.clone())
            .unwrap_or_default()
    }

    pub(super) fn clear_track_list_states(&mut self) {
        self.track_list_states.get_mut().clear();
        self.card_grid_states.get_mut().clear();
    }

    pub(super) fn reset_detail_scroll(&mut self) {
        self.browser_scroll.reset();
        self.scroll = ScrollHandle::new();
        for cache in self.track_list_states.get_mut().values() {
            reset_list_state_to_top(&cache.state);
        }
        for cache in self.card_grid_states.get_mut().values() {
            reset_list_state_to_top(&cache.state);
        }
    }

    pub(crate) fn toggle_artist_section(&mut self, section_index: usize, cx: &mut Context<Self>) {
        let route = self.state.route();
        if !is_detail_route(self.state.routes.len()) || route.category != Category::Artists {
            return;
        }
        let Some(page) = self.state.page.as_ref() else {
            return;
        };
        if section_index >= page.sections.len() {
            return;
        }
        self.artist_section_expanded =
            (self.artist_section_expanded != Some(section_index)).then_some(section_index);
        self.reset_detail_scroll();
        cx.notify();
    }

    pub(super) fn card_scroll_handle(&self, id: &str) -> CardCarouselState {
        self.card_scroll_handles
            .borrow_mut()
            .entry(id.to_owned())
            .or_insert_with(CardCarouselState::new)
            .clone()
    }

    fn reset_discover_flow_context(&mut self) {
        self.discover_flow_return = None;
        self.flow_detail_kind = DeezerFlowKind::Flow;
    }

    pub(crate) fn load(&mut self, service: Service, category: Category, cx: &mut Context<Self>) {
        self.load_service(service, category, cx);
    }

    pub(crate) fn load_force(
        &mut self,
        service: Service,
        category: Category,
        cx: &mut Context<Self>,
    ) {
        self.load_service_force(service, category, cx);
    }
    pub(crate) fn reload_root(&mut self, cx: &mut Context<Self>) {
        let (service, category) = self.selection();
        self.load_service_force(service, category, cx);
    }

    pub(crate) fn invalidate_deezer_history(&mut self, cx: &mut Context<Self>) {
        self.state.invalidate_deezer(Category::History);
        if self.state.deezer_root_active(Category::History) {
            self.load_service_inner(Service::Deezer, Category::History, true, true, cx);
        }
    }

    pub(crate) fn invalidate_soundcloud_history(&mut self, cx: &mut Context<Self>) {
        self.state
            .invalidate_provider(Provider::SoundCloud, Category::History);
        if self.state.service == Service::SoundCloud
            && self.state.selected_root_active(Category::History)
        {
            self.load_service_inner(Service::SoundCloud, Category::History, true, true, cx);
        }
    }

    fn invalidate_playback_actions(&mut self) -> u64 {
        self.playback_action_generation = self.playback_action_generation.wrapping_add(1);
        self.extension_observer_key = None;
        self.playback_action_generation
    }

    pub(crate) fn begin_playback_action(&mut self) -> u64 {
        self.invalidate_playback_actions()
    }

    pub(crate) fn is_playback_action_current(&self, generation: u64) -> bool {
        self.playback_action_generation == generation
    }

    pub(crate) fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        self.start_deezer_mix(DeezerMixKind::Track(track_id), cx);
    }

    pub(crate) fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        self.start_deezer_mix(DeezerMixKind::Artist(artist_id), cx);
    }

    pub(crate) fn start_soundcloud_artist_station(
        &mut self,
        artist_id: String,
        cx: &mut Context<Self>,
    ) {
        let artist_id = artist_id.trim().to_owned();
        if artist_id.is_empty() || !artist_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let action_generation = self.invalidate_playback_actions();
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
        let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start SoundCloud station",
                Some("A SoundCloud account is required.".into()),
            );
            return;
        };
        let Ok(client) = self.soundcloud_client.clone() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start SoundCloud station",
                Some("The SoundCloud library client is unavailable.".into()),
            );
            return;
        };
        let playback = self.playback.clone();
        let task = self
            .runtime
            .spawn(async move { client.load_artist_station(artist_id, token).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("SoundCloud station request failed".into()));
            this.update(cx, |this, cx| {
                let still_current = this.playback_action_generation == action_generation
                    && this.playback.read(cx).state.queue_epoch() == queue_epoch;
                if !still_current {
                    return;
                }
                let Ok(page) = result else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not start SoundCloud station",
                        Some("SoundCloud did not return a playable station.".into()),
                    );
                    return;
                };
                let Some(seed_track_id) = page.tracks.last().map(|track| track.id.clone()) else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "SoundCloud station is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                    return;
                };
                let queue = page
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, Provider::SoundCloud))
                    .collect::<Vec<_>>();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.replace_queue(queue, 0, cx);
                    playback.set_context(PlaybackContext::SoundCloudStation { seed_track_id }, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn start_soundcloud_track_station(
        &mut self,
        track: PlaybackTrack,
        cx: &mut Context<Self>,
    ) {
        let track_id = track.id.trim().to_owned();
        if track_id.is_empty() || !track_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let action_generation = self.invalidate_playback_actions();
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
        let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start SoundCloud station",
                Some("A SoundCloud account is required.".into()),
            );
            return;
        };
        let Ok(client) = self.soundcloud_client.clone() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start SoundCloud station",
                Some("The SoundCloud library client is unavailable.".into()),
            );
            return;
        };
        let playback = self.playback.clone();
        let task = self.runtime.spawn(async move {
            client
                .load_station(track_id, track.title, track.artist, track.artwork, token)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("SoundCloud station request failed".into()));
            this.update(cx, |this, cx| {
                let still_current = this.playback_action_generation == action_generation
                    && this.playback.read(cx).state.queue_epoch() == queue_epoch;
                if !still_current {
                    return;
                }
                let Ok(page) = result else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not start SoundCloud station",
                        Some("SoundCloud did not return a playable station.".into()),
                    );
                    return;
                };
                let Some(seed_track_id) = page.tracks.last().map(|track| track.id.clone()) else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "SoundCloud station is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                    return;
                };
                let queue = page
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, Provider::SoundCloud))
                    .collect::<Vec<_>>();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.replace_queue(queue, 0, cx);
                    playback.set_context(PlaybackContext::SoundCloudStation { seed_track_id }, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn add_negative_feedback(
        &mut self,
        kind: super::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        let id = id.trim().to_owned();
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let key = super::DeezerFeedbackKey {
            kind,
            id: id.clone(),
        };
        let Some(ticket) = self.deezer_actions.begin_feedback(key) else {
            return;
        };
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.deezer_actions.finish_feedback(&ticket);
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not update recommendations",
                Some("A Deezer account is required.".into()),
            );
            return;
        };
        let Ok(client) = self.client.clone() else {
            self.deezer_actions.finish_feedback(&ticket);
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not update recommendations",
                Some("The Deezer library client is unavailable.".into()),
            );
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let task = self.runtime.spawn(async move {
            client
                .add_negative_feedback(kind, &id, arl, saved_user_id)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer recommendation request failed".into()));
            this.update(cx, |this, cx| {
                if !this.deezer_actions.finish_feedback(&ticket) {
                    return;
                }
                match result {
                    Ok(()) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Recommendation updated",
                        Some("Deezer will show fewer similar recommendations.".into()),
                    ),
                    Err(_) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not update recommendations",
                        Some("Deezer did not confirm the recommendation change.".into()),
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn deezer_client(&self) -> Result<super::client::LibraryClient, String> {
        self.client.clone()
    }

    pub(crate) fn soundcloud_library_client(
        &self,
    ) -> Result<super::soundcloud_client::SoundCloudLibraryClient, String> {
        self.soundcloud_client.clone()
    }

    fn start_deezer_mix(&mut self, kind: DeezerMixKind, cx: &mut Context<Self>) {
        let action_generation = self.invalidate_playback_actions();
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start Deezer mix",
                Some("A Deezer account is required.".into()),
            );
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let Ok(client) = self.client.clone() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start Deezer mix",
                Some("The Deezer library client is unavailable.".into()),
            );
            return;
        };
        let playback = self.playback.clone();
        let context = kind.context();
        let task = match &kind {
            DeezerMixKind::Track(track_id) => {
                let track_id = track_id.clone();
                self.runtime.spawn(async move {
                    client.load_track_mix(&track_id, arl, saved_user_id).await
                })
            }
            DeezerMixKind::Artist(artist_id) => {
                let artist_id = artist_id.clone();
                self.runtime.spawn(async move {
                    client.load_artist_mix(&artist_id, arl, saved_user_id).await
                })
            }
        };
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer mix request failed".into()));
            this.update(cx, |this, cx| {
                let still_current = this.playback_action_generation == action_generation
                    && this.playback.read(cx).state.queue_epoch() == queue_epoch;
                if !still_current {
                    return;
                }
                let Ok(batch) = result else {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not start Deezer mix",
                        Some("Deezer did not return a playable mix.".into()),
                    );
                    return;
                };
                if batch.tracks.is_empty() {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "Deezer mix is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                    return;
                }
                let queue = batch
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, Provider::Deezer))
                    .collect::<Vec<_>>();
                let queue_len = queue.len();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.replace_queue(queue, 0, cx);
                    playback.set_context(context.clone(), cx);
                });
                if queue_len == 0 {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "Deezer mix is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn start_deezer_flow(
        &mut self,
        card: Card,
        smart_mix: bool,
        cx: &mut Context<Self>,
    ) {
        // Deezer behavior: the play command on the mix that is already the
        // active playback context toggles pause and resume instead of
        // reloading it.
        {
            let playback = self.playback.read(cx);
            let same_mix_live = matches!(
                playback.context(),
                PlaybackContext::DeezerFlow { config_id, .. } if config_id == card.id.trim()
            ) && matches!(
                playback.state.status,
                crate::playback::PlaybackStatus::Playing | crate::playback::PlaybackStatus::Paused
            );
            if same_mix_live {
                self.playback.update(cx, |playback, cx| playback.toggle(cx));
                return;
            }
        }
        self.start_deezer_flow_with_mode(card, smart_mix, FlowMode::Default, cx);
    }

    fn start_deezer_flow_with_mode(
        &mut self,
        card: Card,
        smart_mix: bool,
        mode: FlowMode,
        cx: &mut Context<Self>,
    ) {
        if card.source != Provider::Deezer || card.id.trim().is_empty() {
            return;
        }
        let action_generation = self.invalidate_playback_actions();
        let account_scope = self.account.read(cx).library_scope();
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start Deezer Flow",
                Some("A Deezer account is required.".into()),
            );
            return;
        };
        let Ok(client) = self.client.clone() else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not start Deezer Flow",
                Some("The Deezer library client is unavailable.".into()),
            );
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let route = Route {
            source: Provider::Deezer,
            category: Category::Flow,
            action: "flowTracks".into(),
            id: card.id,
            title: card.title,
            subtitle: card.subtitle,
            artwork: card.artwork,
            release_date: String::new(),
        };
        let config_id = route.id.clone();
        let flow_kind = if smart_mix {
            DeezerFlowKind::SmartMix
        } else {
            DeezerFlowKind::Flow
        };
        // Feedback first: stop the current audio and open a blank player
        // bar while the mix page loads. The queue epoch is captured after
        // this because entering the pending state advances it.
        self.playback
            .update(cx, |playback, cx| playback.begin_pending_load(cx));
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
        let playback = self.playback.clone();
        let task = self.runtime.spawn(async move {
            client
                .load_flow_radio_page(route, mode, arl, saved_user_id, smart_mix)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(FLOW_REQUEST_FAILURE.to_owned()));
            this.update(cx, |this, cx| {
                if this.playback_action_generation != action_generation
                    || this.playback.read(cx).state.queue_epoch() != queue_epoch
                    || this.account.read(cx).library_scope() != account_scope
                {
                    // The click was superseded; close the blank bar again
                    // unless something else took over playback.
                    playback.update(cx, |playback, cx| {
                        playback.abandon_pending_load_if_epoch(queue_epoch, cx)
                    });
                    return;
                }
                let Ok(page) = result else {
                    playback.update(cx, |playback, cx| {
                        playback.abandon_pending_load_if_epoch(queue_epoch, cx)
                    });
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not start Deezer Flow",
                        Some("Deezer did not return a playable mix.".into()),
                    );
                    return;
                };
                if page.tracks.is_empty() {
                    playback.update(cx, |playback, cx| {
                        playback.abandon_pending_load_if_epoch(queue_epoch, cx)
                    });
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Info,
                        "Deezer mix is empty",
                        Some("No playable tracks were returned.".into()),
                    );
                    return;
                }
                if smart_mix && let Some(title) = page.resolved_smart_mix_title.as_deref() {
                    this.emit_smart_mix_title_resolved(&config_id, title, cx);
                }
                let queue = page
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, Provider::Deezer))
                    .collect::<Vec<_>>();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.apply_flow_page(
                        config_id,
                        mode,
                        page.next_flow_tuner,
                        flow_kind,
                        queue,
                        page.clear_remaining_tracks,
                        false,
                        cx,
                    );
                });
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn select_flow_mode(&mut self, mode: FlowMode, cx: &mut Context<Self>) {
        if self.flow_detail_kind == DeezerFlowKind::SmartMix {
            return;
        }
        self.flow_mode = mode;
        let route = self.state.route().clone();
        if route.source != Provider::Deezer || route.action != "flowTracks" {
            let direct_flow = match self.playback.read(cx).context() {
                PlaybackContext::DeezerFlow {
                    config_id, kind, ..
                } => Some((config_id.clone(), kind.clone())),
                _ => None,
            };
            if let Some((config_id, kind)) = direct_flow {
                self.start_deezer_flow_with_mode(
                    Card {
                        kind: Category::Flow,
                        id: config_id,
                        source: Provider::Deezer,
                        ..Card::default()
                    },
                    kind == DeezerFlowKind::SmartMix,
                    mode,
                    cx,
                );
                return;
            }
            self.invalidate_playback_actions();
            cx.notify();
            return;
        }
        self.invalidate_playback_actions();
        let generation = self.state.reload_active_route().0;
        self.load_flow_route(generation, route, mode, cx);
    }

    pub(crate) fn select_playback_flow_mode(&mut self, mode: FlowMode, cx: &mut Context<Self>) {
        let Some((config_id, kind)) = (match self.playback.read(cx).context() {
            PlaybackContext::DeezerFlow {
                config_id, kind, ..
            } => Some((config_id.clone(), kind.clone())),
            _ => None,
        }) else {
            return;
        };
        if kind == DeezerFlowKind::SmartMix {
            return;
        }
        self.flow_mode = mode;
        self.start_deezer_flow_with_mode(
            Card {
                kind: Category::Flow,
                id: config_id,
                source: Provider::Deezer,
                ..Card::default()
            },
            kind == DeezerFlowKind::SmartMix,
            mode,
            cx,
        );
    }

    pub(crate) fn select_flow_catalog_option(&mut self, option_id: String, cx: &mut Context<Self>) {
        self.flow_catalog_option = Some(option_id);
        cx.notify();
    }

    pub(crate) fn extend_playback_queue(
        &mut self,
        playback: Entity<PlaybackModel>,
        cx: &mut Context<Self>,
    ) {
        let Some(ticket) = playback.update(cx, |playback, _| playback.begin_extension_ticket())
        else {
            return;
        };
        self.start_extension_request(playback, ticket, cx);
        cx.notify();
    }

    fn start_extension_request(
        &self,
        playback: Entity<PlaybackModel>,
        ticket: QueueExtensionTicket,
        cx: &mut Context<Self>,
    ) {
        let context = ticket.context.clone();
        let task = match self.extension_task(&context, cx) {
            Ok(task) => task,
            Err(_) => {
                playback.update(cx, |playback, cx| {
                    playback.finish_extension_for_ticket(&ticket, 0, cx);
                });
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let outcome = task
                .await
                .unwrap_or_else(|_| Err("Queue extension request failed".into()));
            this.update(cx, |this, cx| {
                let Ok(batch) = outcome else {
                    playback.update(cx, |playback, cx| {
                        playback.finish_extension_for_ticket(&ticket, 0, cx);
                    });
                    return;
                };
                let batch_nonempty = !batch.tracks.is_empty();
                let additions = batch
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, batch.provider))
                    .collect::<Vec<_>>();
                let (result, current_track_id) = playback.update(cx, |playback, cx| {
                    let result = playback.apply_extension_batch(
                        &ticket,
                        additions,
                        batch.clear_remaining_tracks,
                        batch.next_flow_tuner,
                        batch.continuation_seed,
                        batch_nonempty,
                        cx,
                    );
                    let current_track_id = playback.state.current_id().map(str::to_owned);
                    (result, current_track_id)
                });
                let Ok(retry_ticket) = result else {
                    return;
                };
                if retry_ticket.is_none()
                    && this.flow_detail_kind == DeezerFlowKind::SmartMix
                    && let PlaybackContext::DeezerFlow {
                        config_id,
                        kind: DeezerFlowKind::SmartMix,
                        ..
                    } = &ticket.context
                {
                    let appended = this.state.sync_active_deezer_smart_mix_tracks(
                        config_id,
                        current_track_id.as_deref(),
                        &batch.tracks,
                        batch.clear_remaining_tracks,
                    );
                    if appended > 0 {
                        cx.notify();
                    }
                }
                if let PlaybackContext::SoundCloudStation { seed_track_id } = &ticket.context {
                    let appended = this
                        .state
                        .append_active_soundcloud_station_tracks(seed_track_id, &batch.tracks);
                    if appended > 0 {
                        cx.notify();
                    }
                }
                if let Some(retry_ticket) = retry_ticket {
                    this.schedule_extension_retry(playback, retry_ticket, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_extension_retry(
        &self,
        playback: Entity<PlaybackModel>,
        ticket: QueueExtensionTicket,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(duplicate_retry_delay(0)).await;
            this.update(cx, |this, cx| {
                this.start_extension_request(playback, ticket, cx);
            })
            .ok();
        })
        .detach();
    }

    fn extension_task(
        &self,
        context: &PlaybackContext,
        cx: &Context<Self>,
    ) -> Result<tokio::task::JoinHandle<Result<ExtensionBatch, String>>, String> {
        match context {
            PlaybackContext::DeezerFlow {
                config_id, tuner, ..
            } => {
                let tuner = tuner
                    .clone()
                    .ok_or_else(|| "Flow tuner unavailable".to_string())?;
                let arl = self
                    .account
                    .read(cx)
                    .deezer_arl()
                    .ok_or_else(|| "Deezer account required".to_string())?;
                let client = self.client.clone()?;
                let config_id = config_id.clone();
                let saved_user_id = self.account.read(cx).deezer_user_id();
                Ok(self.runtime.spawn(async move {
                    client
                        .load_flow_radio(&config_id, tuner, arl, saved_user_id)
                        .await
                        .map(ExtensionBatch::from_deezer)
                }))
            }
            PlaybackContext::DeezerTrackMix { seed_track_id } => {
                let arl = self
                    .account
                    .read(cx)
                    .deezer_arl()
                    .ok_or_else(|| "Deezer account required".to_string())?;
                let client = self.client.clone()?;
                let seed_track_id = seed_track_id.clone();
                let saved_user_id = self.account.read(cx).deezer_user_id();
                Ok(self.runtime.spawn(async move {
                    client
                        .load_track_mix(&seed_track_id, arl, saved_user_id)
                        .await
                        .map(ExtensionBatch::from_deezer)
                }))
            }
            PlaybackContext::DeezerArtistMix { seed_artist_id } => {
                let arl = self
                    .account
                    .read(cx)
                    .deezer_arl()
                    .ok_or_else(|| "Deezer account required".to_string())?;
                let client = self.client.clone()?;
                let seed_artist_id = seed_artist_id.clone();
                let saved_user_id = self.account.read(cx).deezer_user_id();
                Ok(self.runtime.spawn(async move {
                    client
                        .load_artist_mix(&seed_artist_id, arl, saved_user_id)
                        .await
                        .map(ExtensionBatch::from_deezer)
                }))
            }
            PlaybackContext::SoundCloudStation { seed_track_id } => {
                let token = self
                    .account
                    .read(cx)
                    .soundcloud_mobile_token()
                    .ok_or_else(|| "SoundCloud account required".to_string())?;
                let client = self.soundcloud_client.clone()?;
                let seed_track_id = seed_track_id.clone();
                Ok(self.runtime.spawn(async move {
                    client
                        .load_station(
                            seed_track_id,
                            String::new(),
                            String::new(),
                            String::new(),
                            token,
                        )
                        .await
                        .map(ExtensionBatch::from_soundcloud)
                }))
            }
            PlaybackContext::None
            | PlaybackContext::DeezerLibraryTracks { .. }
            | PlaybackContext::SoundCloudCollection { .. } => {
                Err("No infinite queue context".into())
            }
        }
    }

    pub(crate) fn open_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        let category = settings_category_for_service(self.state.service);
        self.settings.update(cx, |settings, cx| {
            settings.begin_session(category, window, cx);
        });
        cx.emit(LibraryEvent::SettingsOpened);
    }

    pub(crate) fn selection(&self) -> (Service, Category) {
        (self.state.service, self.state.category)
    }

    pub(crate) fn detail_open(&self) -> bool {
        is_detail_route(self.state.routes.len()) || self.similar_artists.is_open()
    }

    pub(crate) fn discover_flow_returns_to_discover(&self) -> bool {
        discover_flow_return_active(
            self.discover_flow_return,
            self.state.service,
            self.state.route(),
            self.state.routes.len(),
        )
    }

    pub(super) fn flow_mode_context_label(&self) -> &'static str {
        super::flow_controls::flow_mode_context_label(
            self.flow_detail_kind == DeezerFlowKind::SmartMix,
        )
    }

    pub(super) fn flow_detail_kind(&self) -> DeezerFlowKind {
        self.flow_detail_kind.clone()
    }

    pub(crate) fn set_external_track_navigation(
        &mut self,
        openers: crate::entity_navigation::ProviderNavigationOpeners,
    ) {
        self.external_track_navigation = Some(openers);
    }

    pub(crate) fn external_track_navigation_openers(
        &self,
    ) -> Option<crate::entity_navigation::ProviderNavigationOpeners> {
        self.external_track_navigation.clone()
    }

    pub(crate) fn query(&self, cx: &Context<Self>) -> String {
        self.input.read(cx).value().to_string()
    }

    pub(crate) fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.read(cx).value().is_empty() {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
    }

    pub(crate) fn move_reorderable_track(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        let route = self.state.route().clone();
        if route.is_local_playlist_detail() {
            if !self.query(cx).trim().is_empty() {
                return;
            }
            if self.local_playlist_reorder_pending {
                return;
            }
            self.local_playlist_reorder_pending = true;
            let result = self.enqueue_local_playlist_mutation(
                super::local_persistence::LocalPlaylistMutation::Reorder {
                    playlist_id: route.id,
                    from,
                    to,
                },
                Box::new(move |result, view, cx| {
                    view.local_playlist_reorder_pending = false;
                    if let Err(error) = result {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            "Local playlist could not be reordered",
                            Some(error.to_string().into()),
                        );
                    }
                    cx.notify();
                }),
                cx,
            );
            if let Err(error) = result {
                self.local_playlist_reorder_pending = false;
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Error,
                    "Local playlist could not be reordered",
                    Some(error.to_string().into()),
                );
            }
            return;
        }
        if !route.is_local_tracks_root() {
            self.move_playlist_track(from, to, cx);
            return;
        }
        if !self.query(cx).trim().is_empty() {
            return;
        }
        if self.local_library_reorder_pending {
            return;
        }
        self.local_library_reorder_pending = true;
        let result = self.enqueue_local_library_mutation(
            LocalLibraryMutation::Reorder { from, to },
            Box::new(move |result, view, cx| {
                view.local_library_reorder_pending = false;
                if let Err(error) = result {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Local library could not be reordered",
                        Some(error.to_string().into()),
                    );
                }
                cx.notify();
            }),
            cx,
        );
        if let Err(error) = result {
            self.local_library_reorder_pending = false;
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Local library could not be reordered",
                Some(error.to_string().into()),
            );
        }
    }

    pub(super) fn enqueue_local_library_mutation(
        &mut self,
        mutation: LocalLibraryMutation,
        completion: LocalLibraryCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), super::local_store::LocalLibraryError> {
        let (_, response) = self
            .local_persistence
            .submit_library(mutation)
            .map_err(|_| super::local_store::LocalLibraryError::Filesystem)?;
        let entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let result = response
                .await
                .map_err(|_| super::local_store::LocalLibraryError::Filesystem);
            entity.update(cx, |view, cx| {
                let result = match result {
                    Ok(response) => view.apply_local_library_response(response, cx),
                    Err(error) => Err(error),
                };
                completion(result, view, cx);
            });
        })
        .detach();
        Ok(())
    }

    fn apply_local_library_response(
        &mut self,
        response: LocalLibraryMutationResponse,
        cx: &mut Context<Self>,
    ) -> Result<LocalLibraryMutationOutcome, super::local_store::LocalLibraryError> {
        if response.revision > self.local_library_revision {
            self.local_library_revision = response.revision;
            self.local_store = response.state;
            self.refresh_local_library_page();
            cx.notify();
        }
        response.outcome
    }

    fn refresh_local_library_page(&mut self) {
        if !self.state.route().is_local_tracks_root() {
            return;
        }
        let page = match self.local_store.as_ref() {
            Ok(store) => local_tracks_page(store.tracks()),
            Err(error) => {
                self.state.status = super::state::Status::Failed(error.to_string());
                self.state.page = None;
                return;
            }
        };
        self.state.status = if page.is_empty() {
            super::state::Status::Empty
        } else {
            super::state::Status::Results
        };
        self.state.page = Some(page);
    }

    pub(crate) fn account_scope_changed(&mut self, scope: String, cx: &mut Context<Self>) {
        self.cancel_library_load();
        self.cancel_detail_load();
        self.cancel_favorite_catalog_loads();
        self.reset_discover_flow_context();
        self.invalidate_playback_actions();
        self.deezer_tracks_retry = None;
        self.deezer_actions.set_account_scope(scope.clone());
        self.similar_artists.reset();
        self.artist_section_expanded = None;
        self.album_info_prefetch.clear();
        self.playlists.set_account_scope(scope.clone());
        self.soundcloud_playlists.set_account_scope(scope.clone());
        if self.state.service == Service::Local {
            if self.state.set_account_scope(scope)
                && let Ok(client) = self.client.clone()
            {
                client.clear_bootstrap_cache();
            }
            cx.notify();
            return;
        }
        self.clear_track_list_states();
        if let Some((service, category)) = scope_reload(&mut self.state, scope) {
            if let Ok(client) = self.client.clone() {
                client.clear_bootstrap_cache();
            }
            self.load_service(service, category, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn track_favorites_available(&self) -> bool {
        track_favorites_available_for_route(self.state.route())
    }

    pub(crate) fn add_status_scope(&self) -> String {
        let route = self.state.route();
        format!(
            "library:{}:{}:{}",
            route.source.label(),
            route.action,
            route.id
        )
    }

    pub(super) fn cancel_library_load(&mut self) {
        if let Some(handle) = self.library_load_cancel.take() {
            handle.abort();
        }
    }

    pub(super) fn set_library_load_cancel(&mut self, handle: tokio::task::AbortHandle) {
        self.library_load_cancel = Some(handle);
    }

    pub(super) fn cancel_detail_load(&mut self) {
        if let Some(handle) = self.detail_load_cancel.take() {
            handle.abort();
        }
    }

    pub(super) fn cancel_favorite_catalog_loads(&mut self) {
        for request in self
            .favorite_catalog_cancels
            .drain()
            .map(|(_, request)| request)
        {
            request.abort.abort();
        }
    }

    pub(super) fn set_favorite_catalog_cancel(
        &mut self,
        kind: FavoriteKind,
        generation: u64,
        abort: tokio::task::AbortHandle,
    ) {
        if let Some(previous) = self
            .favorite_catalog_cancels
            .insert(kind, FavoriteCatalogRequest { generation, abort })
        {
            previous.abort.abort();
        }
    }

    pub(super) fn clear_favorite_catalog_cancel(&mut self, kind: FavoriteKind, generation: u64) {
        if self
            .favorite_catalog_cancels
            .get(&kind)
            .is_some_and(|request| request.generation == generation)
        {
            self.favorite_catalog_cancels.remove(&kind);
        }
    }

    fn load_service(&mut self, service: Service, category: Category, cx: &mut Context<Self>) {
        self.reset_discover_flow_context();
        self.load_service_inner(service, category, false, false, cx);
        cx.emit(LibraryEvent::SelectionChanged);
    }

    pub(super) fn load_service_force(
        &mut self,
        service: Service,
        category: Category,
        cx: &mut Context<Self>,
    ) {
        self.reset_discover_flow_context();
        self.load_service_inner(service, category, true, false, cx);
    }

    fn load_service_inner(
        &mut self,
        service: Service,
        category: Category,
        force: bool,
        preserve_visible_page: bool,
        cx: &mut Context<Self>,
    ) {
        self.cancel_library_load();
        self.cancel_detail_load();
        if self.state.service != service {
            self.category_motion = SegmentedSelectorMotion::default();
        }
        self.invalidate_playback_actions();
        self.similar_artists.reset();
        self.artist_section_expanded = None;
        self.flow_catalog_option = None;
        if !preserve_visible_page {
            self.clear_track_list_states();
            self.reset_detail_scroll();
        }
        if service == Service::Local {
            let generation = if preserve_visible_page {
                self.state.reload_preserving_page(Service::Local, category)
            } else {
                self.state.reload(Service::Local, category)
            };
            let result = match category {
                Category::Tracks => self
                    .local_store
                    .as_ref()
                    .map(|store| local_tracks_page(store.tracks()))
                    .map_err(|error| error.to_string()),
                Category::Playlists => self
                    .local_playlists
                    .as_ref()
                    .map(|store| local_playlists_page(store.playlists()))
                    .map_err(|error| error.to_string()),
                _ => Err("The selected Local category is unavailable".into()),
            };
            self.state.complete_nested_without_cache(generation, result);
            cx.notify();
            return;
        }
        let account_scope = self.account.read(cx).library_scope();
        self.deezer_actions.set_account_scope(account_scope.clone());
        if self.state.set_account_scope(account_scope) {
            if let Ok(client) = self.client.clone() {
                client.clear_bootstrap_cache();
            }
            self.favorites
                .update(cx, |favorites, _| favorites.reset_account());
        }
        let (generation, cached) = if force {
            (
                if preserve_visible_page {
                    self.state.reload_preserving_page(service, category)
                } else {
                    self.state.reload(service, category)
                },
                None,
            )
        } else {
            self.state.select(service, category)
        };
        if cached.is_some() {
            self.seed_loaded_root_favorites(service, category, cx);
            if service == Service::Deezer
                && category == Category::Tracks
                && self.state.tracks_pipeline_retryable()
            {
                self.retry_deezer_tracks_pending(cx);
            }
            cx.notify();
            return;
        }
        let Some(credential) = (match service {
            Service::Local => unreachable!("Local library is loaded before credentials"),
            Service::Deezer => self.account.read(cx).deezer_arl().map(Credential::Deezer),
            Service::SoundCloud => self
                .account
                .read(cx)
                .soundcloud_token()
                .map(Credential::SoundCloud),
        }) else {
            self.state.account_required();
            cx.notify();
            return;
        };
        if service == Service::Deezer && category == Category::Tracks {
            let Credential::Deezer(arl) = credential else {
                unreachable!("Deezer Tracks requires Deezer credentials");
            };
            if self.state.tracks_pipeline_active() {
                return;
            }
            let token = self.state.begin_tracks_pipeline();
            self.load_deezer_tracks_progressive(generation, token, arl, cx);
            return;
        }
        let task = match (service, credential) {
            (Service::Deezer, Credential::Deezer(arl)) => {
                let Ok(client) = self.client.clone() else {
                    let result = Err("Library client could not be created".into());
                    if preserve_visible_page {
                        self.state.complete_preserving_page(generation, result);
                    } else {
                        self.state.complete(generation, result);
                    }
                    cx.notify();
                    return;
                };
                let saved_user_id = self.account.read(cx).deezer_user_id();
                self.runtime
                    .spawn(async move { client.load(category, arl, saved_user_id).await })
            }
            (Service::SoundCloud, Credential::SoundCloud(token)) => {
                let Ok(client) = self.soundcloud_client.clone() else {
                    let result = Err("Library client could not be created".into());
                    if preserve_visible_page {
                        self.state.complete_preserving_page(generation, result);
                    } else {
                        self.state.complete(generation, result);
                    }
                    cx.notify();
                    return;
                };
                self.runtime
                    .spawn(async move { client.load(category, token).await })
            }
            _ => unreachable!(),
        };
        self.library_load_cancel = Some(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(format!("{} library request failed", service.label())));
            if let Err(error) = &result {
                eprintln!("{} library request failed: {error}", service.label());
            }
            this.update(cx, |this, cx| {
                let completed = if preserve_visible_page {
                    this.state.complete_preserving_page(generation, result)
                } else {
                    this.state.complete(generation, result)
                };
                if completed {
                    this.seed_loaded_root_favorites(service, category, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_similar_artists(
        &mut self,
        artist_id: String,
        title: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let artist_id = artist_id.trim().to_owned();
        if artist_id.is_empty() || !artist_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let generation = self.similar_artists.open(artist_id.clone(), title);
        self.invalidate_playback_actions();
        self.reset_detail_scroll();
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.similar_artists
                .complete(generation, Err("Deezer account required".into()));
            cx.notify();
            return;
        };
        let Ok(client) = self.client.clone() else {
            self.similar_artists
                .complete(generation, Err("Similar Artists client unavailable".into()));
            cx.notify();
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let task = self.runtime.spawn(async move {
            client
                .load_similar_artists(&artist_id, arl, saved_user_id)
                .await
        });
        self.detail_load_cancel = Some(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Similar Artists request failed".into()));
            this.update(cx, |this, cx| {
                let result = result.map(|related| {
                    related
                        .artists
                        .into_iter()
                        .map(|mut card| {
                            card.source = Provider::Deezer;
                            card
                        })
                        .collect::<Vec<_>>()
                });
                if this.similar_artists.complete(generation, result) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_card(&mut self, card: Card, cx: &mut Context<Self>) {
        self.similar_artists.reset();
        self.artist_section_expanded = None;
        self.playlists.clear_remove_feedback();
        let Some(route) = card_route(card) else {
            return;
        };
        self.cancel_library_load();
        self.cancel_detail_load();
        self.invalidate_playback_actions();
        self.flow_catalog_option = None;
        if route.action == "flowTracks" {
            self.flow_mode = FlowMode::Default;
            if self.discover_flow_return.is_none() {
                self.flow_detail_kind = DeezerFlowKind::Flow;
            }
        }
        self.reset_detail_scroll();
        let (generation, cached) = self.state.push(route.clone());
        if route.action == "flowTracks" {
            if reuse_cached_flow_page(&self.flow_detail_kind, cached.as_ref()) {
                if let Some(page) = cached.as_ref()
                    && let Some(title) = page.resolved_smart_mix_title.as_deref()
                {
                    self.emit_smart_mix_title_resolved(&route.id, title, cx);
                }
                cx.notify();
                return;
            }
            let generation = self.state.reload_active_route().0;
            self.load_flow_route(generation, route, FlowMode::Default, cx);
            return;
        }
        if cached.is_some() {
            cx.notify();
        } else {
            self.load_nested(generation, route, cx);
        }
    }

    fn emit_smart_mix_title_resolved(&self, config_id: &str, title: &str, cx: &mut Context<Self>) {
        if let Some(event) = smart_mix_title_event(config_id, title) {
            cx.emit(event);
        }
    }

    pub(crate) fn open_discover_flow(
        &mut self,
        card: crate::search::Card,
        smart_mix: bool,
        cx: &mut Context<Self>,
    ) {
        if card.source != Provider::Deezer || card.id.trim().is_empty() {
            return;
        }
        if self.discover_flow_return.is_none() {
            self.discover_flow_return = Some(self.selection());
        }
        self.invalidate_playback_actions();
        self.category_motion = SegmentedSelectorMotion::default();
        self.similar_artists.reset();
        self.artist_section_expanded = None;
        self.flow_catalog_option = None;
        self.flow_detail_kind = if smart_mix {
            DeezerFlowKind::SmartMix
        } else {
            DeezerFlowKind::Flow
        };
        let account_scope = self.account.read(cx).library_scope();
        self.deezer_actions.set_account_scope(account_scope.clone());
        self.state.set_account_scope(account_scope);
        self.state.select(Service::Deezer, Category::Flow);
        let library_card = Card {
            kind: Category::Flow,
            id: card.id,
            title: card.title,
            subtitle: card.subtitle,
            artwork: card.artwork,
            source: Provider::Deezer,
            ..Card::default()
        };
        self.open_card(library_card, cx);
        cx.emit(LibraryEvent::NavigationRequested);
    }

    pub(crate) fn back(&mut self, cx: &mut Context<Self>) {
        self.invalidate_playback_actions();
        self.flow_catalog_option = None;
        self.artist_section_expanded = None;
        self.clear_track_list_states();
        if self.discover_flow_returns_to_discover()
            && let Some((service, category)) = self.discover_flow_return.take()
        {
            self.similar_artists.reset();
            self.flow_mode = FlowMode::Default;
            self.flow_detail_kind = DeezerFlowKind::Flow;
            self.state.select(service, category);
            self.reset_detail_scroll();
            cx.emit(LibraryEvent::DiscoverRequested);
            cx.notify();
            return;
        }
        if self.similar_artists.is_open() {
            self.similar_artists.back();
            self.reset_detail_scroll();
            cx.notify();
            return;
        }
        let Some((generation, route, cached)) = self.state.back() else {
            return;
        };
        self.reset_detail_scroll();
        if cached.is_some() {
            cx.notify();
            return;
        }
        if !is_detail_route(self.state.routes.len()) {
            self.load_service(
                if route.is_local_route() {
                    Service::Local
                } else {
                    match route.source {
                        crate::search::Provider::Deezer => Service::Deezer,
                        crate::search::Provider::SoundCloud => Service::SoundCloud,
                    }
                },
                route.category,
                cx,
            );
        } else {
            self.load_nested(generation, route, cx);
        }
    }

    pub(super) fn load_nested(&mut self, generation: u64, route: Route, cx: &mut Context<Self>) {
        self.cancel_library_load();
        self.cancel_detail_load();
        if route.is_local_playlist_detail() {
            let result = match self.local_playlists.as_ref() {
                Ok(store) => store
                    .playlist(&route.id)
                    .map(local_playlist_page)
                    .ok_or_else(|| "The local playlist could not be found.".into()),
                Err(error) => Err(error.to_string()),
            };
            if self.state.complete_nested_without_cache(generation, result) {
                cx.notify();
            }
            return;
        }
        let (arl, token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        let has_credentials = match route.source {
            crate::search::Provider::Deezer => arl.is_some(),
            crate::search::Provider::SoundCloud => token.is_some(),
        };
        if !has_credentials {
            self.state.account_required();
            cx.notify();
            return;
        };
        let service = match route.source {
            crate::search::Provider::Deezer => Service::Deezer,
            crate::search::Provider::SoundCloud => Service::SoundCloud,
        };
        if service == Service::Deezer && route.action == "flowTracks" {
            self.load_flow_route(generation, route, self.flow_mode, cx);
            return;
        }
        let task = match service {
            Service::Local => unreachable!("Local library does not load nested provider routes"),
            Service::Deezer => {
                let Ok(client) = self.client.clone() else {
                    self.state.complete_nested(
                        generation,
                        Err("Library client could not be created".into()),
                    );
                    cx.notify();
                    return;
                };
                self.runtime
                    .spawn(async move { client.load_route(route, arl).await })
            }
            Service::SoundCloud => {
                let Ok(client) = self.soundcloud_client.clone() else {
                    self.state.complete_nested(
                        generation,
                        Err("Library client could not be created".into()),
                    );
                    cx.notify();
                    return;
                };
                self.runtime
                    .spawn(async move { client.load_route(route, token).await })
            }
        };
        self.detail_load_cancel = Some(task.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(format!("{} library request failed", service.label())));
            if let Err(error) = &result {
                eprintln!("{} library detail request failed: {error}", service.label());
            }
            this.update(cx, |this, cx| {
                if this.state.complete_nested(generation, result) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_flow_route(
        &mut self,
        generation: u64,
        route: Route,
        mode: FlowMode,
        cx: &mut Context<Self>,
    ) {
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.state.account_required();
            cx.notify();
            return;
        };
        let account_scope = self.account.read(cx).library_scope();
        let action_generation = self.playback_action_generation;
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let smart_mix = self.flow_detail_kind == DeezerFlowKind::SmartMix;
        let smart_mix_for_completion = smart_mix;
        let Ok(client) = self.client.clone() else {
            self.state.complete_nested_without_cache(
                generation,
                Err("Library client could not be created".into()),
            );
            cx.notify();
            return;
        };
        let expected_route = route.clone();
        let task = self.runtime.spawn(async move {
            client
                .load_flow_radio_page(route, mode, arl, saved_user_id, smart_mix)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err(FLOW_REQUEST_FAILURE.into()));
            this.update(cx, |this, cx| {
                if this.state.route() != &expected_route
                    || this.account.read(cx).library_scope() != account_scope
                    || this.playback_action_generation != action_generation
                {
                    return;
                }
                let flow_page = result.as_ref().ok().map(|page| {
                    (
                        page.title.clone(),
                        page.tracks
                            .iter()
                            .map(|track| PlaybackTrack::from_library(track, Provider::Deezer))
                            .collect::<Vec<_>>(),
                        page.next_flow_tuner.clone(),
                        page.clear_remaining_tracks,
                    )
                });
                let resolved_title = result
                    .as_ref()
                    .ok()
                    .and_then(|page| page.resolved_smart_mix_title.clone());
                let completed = if smart_mix_for_completion {
                    this.state.complete_nested(generation, result)
                } else {
                    this.state.complete_nested_without_cache(generation, result)
                };
                if completed {
                    if smart_mix_for_completion && let Some(title) = resolved_title.as_deref() {
                        this.state
                            .update_active_deezer_smart_mix_title(&expected_route.id, title);
                        this.emit_smart_mix_title_resolved(&expected_route.id, title, cx);
                    }
                    if let Some((_, tracks, tuner, clear_remaining)) = flow_page {
                        let active_kind = active_deezer_flow_kind(
                            this.playback.read(cx).context(),
                            &expected_route.id,
                        );
                        if let Some(active_kind) = active_kind {
                            let playback = this.playback.clone();
                            playback.update(cx, |playback, cx| {
                                playback.apply_flow_page(
                                    expected_route.id.clone(),
                                    mode,
                                    tuner,
                                    active_kind,
                                    tracks,
                                    clear_remaining,
                                    true,
                                    cx,
                                );
                            });
                        }
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
    fn tab(
        &self,
        category: Category,
        index: usize,
        responsive: crate::motion::ResponsiveModeVisual,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.state.category == category;
        let compact_selected = selected && responsive.target_compact;
        let service = self.state.service;
        let focus = self.category_tab_focus[index].clone();
        let tab_focus = self.category_tab_focus.clone();
        let categories = service.categories();
        let icon = match (service, category) {
            (Service::Local, Category::Tracks) => LocalIcon::Music,
            (_, Category::MyTracks) => LocalIcon::CloudArrowUp,
            (Service::Deezer, Category::Tracks) => LocalIcon::Music,
            (Service::SoundCloud, Category::Tracks) => LocalIcon::Music,
            (_, Category::Artists) => LocalIcon::UserGroup,
            (_, Category::Albums) => LocalIcon::CompactDisc,
            (_, Category::Playlists) => LocalIcon::ListUl,
            (_, Category::History) => LocalIcon::ClockRotateLeft,
            (_, Category::Station) => LocalIcon::Radio,
            (_, Category::Flow) => LocalIcon::Infinity,
        };
        let (from_gap, target_gap) = responsive.endpoints(7., 0.);
        let (from_padding, target_padding) = responsive.endpoints(8., 0.);
        let label_width = crate::music_ui::category_tab_text_width(category.label());
        let (from_label_width, target_label_width) = responsive.endpoints(label_width, 0.);
        let (from_opacity, target_opacity) = responsive.endpoints(1., 0.);
        let label = div()
            .flex_none()
            .overflow_hidden()
            .whitespace_nowrap()
            .max_w(px(target_label_width))
            .opacity(target_opacity)
            .child(category.label())
            .with_animation(
                (
                    ElementId::from(("library-category-label", index)),
                    responsive.epoch.to_string(),
                ),
                crate::motion::content(),
                move |this, delta| {
                    this.max_w(px(crate::motion::lerp(
                        from_label_width,
                        target_label_width,
                        delta,
                    )))
                    .opacity(crate::motion::lerp(
                        from_opacity,
                        target_opacity,
                        delta,
                    ))
                },
            )
            .into_any_element();
        let tab = div()
            .id(category.label())
            .track_focus(&focus)
            .tab_stop(selected)
            .role(gpui::Role::Tab)
            .aria_label(category.label())
            .aria_selected(selected)
            .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
            .min_w_0()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .h(px(30.))
            .flex_1()
            .gap(px(target_gap))
            .px(px(target_padding))
            .rounded(px(6.))
            .border_1()
            .border_color(if compact_selected {
                rgba(0x818cf8d9)
            } else {
                rgba(0x00000000)
            })
            .bg(if compact_selected {
                rgba(0x6366f13d)
            } else {
                rgba(0x00000000)
            })
            .text_color(rgb(if selected { FOREGROUND } else { MUTED }))
            .text_size(px(12.5))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .when_some(
                category_tab_tooltip(responsive.target_compact, category.label()),
                |this, tooltip| this.app_tooltip(tooltip),
            )
            .hover(|style| style.text_color(rgb(FOREGROUND)))
            .on_click(cx.listener(move |this, _, _, cx| this.load_service(service, category, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.load_service(service, category, cx);
                    return;
                }
                let Some(next) = crate::tab_keyboard::next_tab_index(
                    event.keystroke.key.as_str(),
                    index,
                    categories.len(),
                ) else {
                    return;
                };
                window.prevent_default();
                let next_category = categories[next];
                this.load_service(service, next_category, cx);
                tab_focus[next].focus(window, cx);
            }))
            .child(crate::music_ui::category_tab_icon(
                icon,
                if selected { FOREGROUND } else { MUTED },
                if category == Category::History {
                    crate::music_ui::CATEGORY_TAB_ICON_SIZE - 1.
                } else {
                    crate::music_ui::CATEGORY_TAB_ICON_SIZE
                },
            ))
            .child(label);
        tab.with_animation(
            (
                ElementId::from(("library-category-tab", index)),
                responsive.epoch.to_string(),
            ),
            crate::motion::content(),
            move |this, delta| {
                this.gap(px(crate::motion::lerp(from_gap, target_gap, delta)))
                    .px(px(crate::motion::lerp(from_padding, target_padding, delta)))
            },
        )
        .into_any_element()
    }
}

fn settings_category_for_service(_: Service) -> SettingsCategory {
    SettingsCategory::Providers
}

fn track_favorites_available_for_route(route: &Route) -> bool {
    !route.is_local_route()
        && matches!(
            route.source,
            crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
        )
}

fn local_tracks_page(tracks: &[super::local_store::LocalTrack]) -> Page {
    let tracks = tracks.iter().map(Track::from).collect::<Vec<_>>();
    Page {
        title: "Local Tracks".into(),
        description: "Tracks saved to your local library from Deezer and SoundCloud.".into(),
        platform: Some(Service::Local),
        count_noun: "track".into(),
        total: tracks.len(),
        raw_loaded_count: tracks.len(),
        normalized_count: tracks.len(),
        authoritative_total: Some(tracks.len()),
        tracks,
        empty_title: "No local tracks yet".into(),
        empty_description: "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file.".into(),
        ..Page::default()
    }
}

fn playback_provider(track: &crate::playback::PlaybackTrack) -> Provider {
    match track.provider {
        crate::playback::PlaybackProvider::Deezer => Provider::Deezer,
        crate::playback::PlaybackProvider::SoundCloud => Provider::SoundCloud,
    }
}

impl ResizeSettledTarget for LibraryView {
    fn commit_resize(&mut self, request: crate::music_ui::ResizeRequest) -> bool {
        self.card_columns.commit(request)
    }
}

fn library_content_scroll(
    content: gpui::AnyElement,
    contained: bool,
    scroll: &ScrollHandle,
    browser_scroll: BrowserScrollState,
) -> gpui::AnyElement {
    if contained {
        return div()
            .id("library-content")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(content)
            .into_any_element();
    }
    let scroll_content = div()
        .id("library-content-scroll")
        .flex_1()
        .min_h_0()
        .track_scroll(scroll)
        .overflow_y_scroll()
        .vertical_scrollbar(scroll)
        .child(content)
        .into_any_element();
    browser_scroll_surface(
        "library-content",
        scroll_content,
        BrowserScrollTarget::Handle(scroll.clone()),
        browser_scroll,
    )
}

fn library_content_transition_identity(state: &LibraryState) -> String {
    if matches!(state.status, super::state::Status::AccountRequired) {
        return format!(
            "library-content-body:{}:account-required",
            state.service.label()
        );
    }

    let route = state.route();
    if state.routes.len() == 1
        && matches!(
            state.category,
            Category::Albums | Category::Artists | Category::Playlists
        )
    {
        return format!("library-content-body:{}:root-cards", state.service.label());
    }

    format!(
        "library-content-body:{}:{}:{}:{}",
        state.service.label(),
        state.category.label(),
        route.action,
        route.id
    )
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let metrics = crate::music_ui::shell_metrics(f32::from(window.viewport_size().width));
        let right_sidebar_open =
            self.playback.read(cx).state.right_sidebar != crate::playback::RightSidebar::Closed;
        let available_width = effective_content_width(&metrics, right_sidebar_open);
        self.card_available_width = available_width;
        let available_height = f32::from(window.viewport_size().height);
        if let Some(request) = self.card_columns.observe_width(available_width) {
            crate::music_ui::schedule_resize(cx, window, request);
        }
        let columns = self.card_columns.columns();
        self.card_grid_motion
            .prepare(columns, available_width, now, cx.reduce_motion());
        let narrow = metrics.narrow_content;
        let small_page_heading = f32::from(window.viewport_size().width)
            <= super::content_view::PAGE_HEADING_SMALL_MAX_VIEWPORT;
        let gutter = crate::music_ui::main_content_inset(&metrics);
        let categories = self.state.service.categories();
        let selected_category_index = categories
            .iter()
            .position(|category| *category == self.state.category)
            .unwrap_or_default();
        let category_visual = self.category_motion.prepare(
            selected_category_index,
            categories.len(),
            now,
            cx.reduce_motion(),
        );
        let category_labels = self
            .state
            .service
            .categories()
            .iter()
            .map(|category| category.label())
            .collect::<Vec<_>>();
        let use_category_tab_icons = category_tabs_icon_only(available_width, &category_labels);
        let category_responsive =
            self.category_tabs_responsive
                .prepare(use_category_tab_icons, now, cx.reduce_motion());
        let detail_open = self.detail_open();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .px(px(gutter))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(super::content_view::render_main_header(
                        self,
                        small_page_heading,
                        cx,
                    ))
                    .when(!detail_open, |this| {
                        this.child(
                            div()
                                .id("library-category-tabs")
                                .role(gpui::Role::TabList)
                                .aria_label("Library category")
                                .w_full()
                                .h(px(38.))
                                .flex()
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
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .when(!use_category_tab_icons, |this| {
                                            this.child(crate::motion::segmented_selector_indicator(
                                                category_visual,
                                            ))
                                        })
                                        .children(categories.iter().copied().enumerate().map(
                                            |(index, category)| {
                                                self.tab(category, index, category_responsive, cx)
                                            },
                                        )),
                                ),
                        )
                    }),
            )
            .child({
                let similar_open = self.similar_artists.is_open();
                let similar_virtualized =
                    self.similar_artists.current.as_ref().is_some_and(|entry| {
                        matches!(
                            &entry.state,
                            super::deezer_similar::SimilarArtistsState::Results(cards)
                                if !cards.is_empty()
                        )
                    });
                let virtualized = if similar_open {
                    similar_virtualized
                } else {
                    super::content_view::uses_virtualized_scroll(self, cx)
                };
                let loading =
                    !similar_open && matches!(self.state.status, super::state::Status::Loading);
                let contained = virtualized || loading;
                let body = if similar_open {
                    super::deezer_similar::render(
                        self,
                        &cx.entity(),
                        &self.similar_artists,
                        columns,
                        narrow,
                        cx,
                    )
                } else {
                    super::content_view::render_content(
                        self,
                        columns,
                        available_width,
                        available_height,
                        narrow,
                        cx,
                    )
                };
                let content = div()
                    .w_full()
                    .px(px(gutter))
                    .when(contained, |this| {
                        this.size_full().flex().flex_col().min_h_0()
                    })
                    // Breathing room above the player bar. Padding lives on
                    // the inner content so it only shows at the tail for div
                    // scrolls. Virtualized lists own their scroll state, so
                    // they must not get a persistent outer gap.
                    .when(!contained, |this| {
                        this.pb(px(LIBRARY_CONTENT_BOTTOM_PADDING_PX))
                    })
                    .child(body);
                let content = if similar_open {
                    content.into_any_element()
                } else {
                    let content_identity = library_content_transition_identity(&self.state);
                    content
                        .relative()
                        .with_animation(
                            content_identity,
                            crate::motion::quick_content(),
                            |this, delta| {
                                this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                                    .top(px(crate::motion::lerp(4.0, 0.0, delta)))
                            },
                        )
                        .into_any_element()
                };
                library_content_scroll(
                    content,
                    contained,
                    &self.scroll,
                    self.browser_scroll.clone(),
                )
            })
    }
}

fn reset_list_state_to_top(state: &ListState) {
    state.scroll_to(ListOffset::default());
}

impl crate::entity_navigation::TrackMenuHost for LibraryView {
    fn favorites_entity(&self) -> Entity<FavoriteState> {
        self.favorites.clone()
    }

    fn local_track_saved(&self, track: &crate::playback::PlaybackTrack, _cx: &gpui::App) -> bool {
        let provider = playback_provider(track);
        self.local_store.as_ref().is_ok_and(|store| {
            store
                .tracks()
                .iter()
                .any(|saved| saved.provider == provider && saved.id == track.id)
        })
    }

    fn set_local_track_saved(
        &mut self,
        track: crate::playback::PlaybackTrack,
        saved: bool,
        cx: &mut Context<Self>,
    ) {
        let title = track.title.clone();
        let result = self.enqueue_local_library_mutation(
            LocalLibraryMutation::SetSaved {
                track: Box::new(super::local_store::LocalTrack::from(&track)),
                saved,
            },
            Box::new(move |result, _, cx| match result {
                Ok(LocalLibraryMutationOutcome::Saved)
                | Ok(LocalLibraryMutationOutcome::Removed) => {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        if saved {
                            "Saved to Local"
                        } else {
                            "Removed from Local"
                        },
                        Some(title.into()),
                    );
                }
                Err(error) => crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Error,
                    "Local library could not be updated",
                    Some(error.to_string().into()),
                ),
                Ok(_) => {}
            }),
            cx,
        );
        if let Err(error) = result {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Local library could not be updated",
                Some(error.to_string().into()),
            );
        }
    }

    fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        LibraryView::resolve_favorite_state(self, key, cx);
    }

    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: crate::search::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scope = self.add_status_scope();
        self.open_add_picker(vec![track_id], scope, provider, window, cx)
    }

    fn open_local_playlist_picker(
        &mut self,
        track: crate::playback::PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        LibraryView::open_local_playlist_picker(self, track, window, cx);
    }

    fn preload_playlist_catalog(
        &mut self,
        provider: crate::search::Provider,
        cx: &mut Context<Self>,
    ) {
        match provider {
            crate::search::Provider::Deezer => {
                self.ensure_playlist_catalog(cx);
            }
            crate::search::Provider::SoundCloud => {
                self.ensure_soundcloud_playlist_catalog(cx);
            }
        }
    }

    fn toggle_favorite_state(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        use super::favorite_state::{FavoriteKey, FavoriteKind};
        self.toggle_favorite(
            FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id),
            known_favorite,
            cx,
        )
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
        kind: super::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.add_negative_feedback(kind, id, cx);
    }

    fn toggle_artist_favorite(
        &mut self,
        provider: crate::search::Provider,
        artist_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        self.toggle_favorite(
            super::FavoriteKey::for_provider(provider, super::FavoriteKind::Artist, artist_id),
            known_favorite,
            cx,
        );
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

    fn open_track_info(
        &mut self,
        provider: crate::search::Provider,
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
        provider: crate::search::Provider,
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
mod progressive_track_list_tests {
    use super::{TrackListChange, TrackListLayout, track_list_change};

    fn layout() -> TrackListLayout {
        TrackListLayout::new(false, false, false)
    }

    #[test]
    fn exact_prefix_growth_is_spliced_without_resetting_scroll() {
        let previous = vec!["one".into(), "two".into()];
        let next = vec!["one".into(), "two".into(), "three".into()];
        assert_eq!(
            track_list_change(&previous, (previous.len(), layout()), &next, layout()),
            TrackListChange::Append {
                old_count: 2,
                added: 1
            }
        );
    }

    #[test]
    fn reorder_shrink_and_layout_changes_reset_the_list() {
        let previous = vec!["one".into(), "two".into()];
        assert_eq!(
            track_list_change(
                &previous,
                (previous.len(), layout()),
                &["two".into(), "one".into()],
                layout()
            ),
            TrackListChange::Reset
        );
        assert_eq!(
            track_list_change(
                &previous,
                (previous.len(), layout()),
                &["one".into()],
                layout()
            ),
            TrackListChange::Reset
        );
        assert_eq!(
            track_list_change(
                &previous,
                (previous.len(), layout()),
                &["one".into(), "two".into(), "three".into()],
                TrackListLayout::new(true, false, false)
            ),
            TrackListChange::Reset
        );
    }

    #[test]
    fn unchanged_rows_do_not_touch_the_list_state() {
        let rows = vec!["one".into(), "two".into()];
        assert_eq!(
            track_list_change(&rows, (rows.len(), layout()), &rows, layout()),
            TrackListChange::Unchanged
        );
    }
}

#[cfg(test)]
mod card_grid_tests {
    use super::{CardGridChange, CardGridLayout, card_grid_change};

    fn layout(width: f32) -> CardGridLayout {
        CardGridLayout::new(4, width, false)
    }

    #[test]
    fn content_replacement_and_reorder_reset_card_grid_identity() {
        let first = vec!["card-a".to_owned(), "card-b".to_owned()];
        let replaced = vec!["card-a-artwork-b".to_owned(), "card-b".to_owned()];
        let reordered = vec!["card-b".to_owned(), "card-a".to_owned()];
        let previous = layout(800.);

        assert_eq!(
            card_grid_change(1, previous, &first, 1, previous, &replaced,),
            CardGridChange::Reset
        );
        assert_eq!(
            card_grid_change(1, previous, &first, 1, previous, &reordered,),
            CardGridChange::Reset
        );
    }

    #[test]
    fn same_column_width_changes_remeasure_and_column_changes_reflow() {
        let identity = vec!["card-a".to_owned(), "card-b".to_owned()];
        let narrow_width = layout(780.);
        let wider_width = layout(800.);

        assert_ne!(narrow_width, wider_width);
        assert_eq!(
            card_grid_change(1, narrow_width, &identity, 1, wider_width, &identity,),
            CardGridChange::Remeasure
        );

        let reflow = CardGridLayout::new(3, 800., false);
        assert_eq!(
            card_grid_change(1, narrow_width, &identity, 1, reflow, &identity,),
            CardGridChange::Reflow
        );
    }
}
