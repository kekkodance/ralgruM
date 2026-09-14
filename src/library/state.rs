use std::collections::{HashMap, HashSet};

use super::{
    client::DEEZER_SESSION_EXPIRED,
    model::{Card, Category, Page, Route, Service, Track},
    playlist_client::OwnedPlaylist,
};
use crate::smart_mix_title::specific_smart_mix_title;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Status {
    Initial,
    Loading,
    AccountRequired,
    Empty,
    Results,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TracksPipelineStage {
    Loading,
    Preview,
    RawPendingHydration,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TracksPipelineLoadOutcome {
    Ignored,
    Failed,
    Restart { generation: u64, token: u64 },
}

struct TracksPipeline {
    token: u64,
    stage: TracksPipelineStage,
    page: Option<Page>,
    in_flight: bool,
}

fn cache_route(route: &Route) -> Route {
    if route.source == crate::search::Provider::Deezer
        && route.category == Category::Flow
        && route.action == "flowTracks"
    {
        return Route {
            id: route.id.trim().to_owned(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            ..route.clone()
        };
    }
    route.clone()
}

fn cache_key(route: &Route, account_scope: &str) -> (Route, String) {
    (cache_route(route), account_scope.to_owned())
}

pub(crate) struct LibraryState {
    pub service: Service,
    pub category: Category,
    pub status: Status,
    pub page: Option<Page>,
    pub routes: Vec<Route>,
    generation: u64,
    tracks_enrichment_generation: Option<u64>,
    tracks_playback_generation_floor: u64,
    next_tracks_pipeline_token: u64,
    tracks_pipeline: Option<TracksPipeline>,
    tracks_session_expired: bool,
    cache: HashMap<(Route, String), Page>,
    account_scope: String,
}

impl Default for LibraryState {
    fn default() -> Self {
        Self {
            service: Service::Deezer,
            category: Category::Tracks,
            status: Status::Initial,
            page: None,
            routes: vec![Route::root(Service::Deezer, Category::Tracks)],
            generation: 0,
            tracks_enrichment_generation: None,
            tracks_playback_generation_floor: 0,
            next_tracks_pipeline_token: 0,
            tracks_pipeline: None,
            tracks_session_expired: false,
            cache: HashMap::new(),
            account_scope: String::new(),
        }
    }
}

impl LibraryState {
    pub(crate) fn set_account_scope(&mut self, scope: String) -> bool {
        if self.account_scope == scope {
            return false;
        }
        self.account_scope = scope;
        self.tracks_pipeline = None;
        self.tracks_session_expired = false;
        if self.service == Service::Local {
            return false;
        }
        self.generation = self.generation.wrapping_add(1);
        self.tracks_playback_generation_floor = self.generation;
        self.tracks_enrichment_generation = None;
        self.page = None;
        self.status = Status::Initial;
        self.routes = vec![Route::root(self.service, self.category)];
        true
    }

    pub(crate) fn select(&mut self, service: Service, category: Category) -> (u64, Option<Page>) {
        self.select_with_cache(service, category, true)
    }

    pub(crate) fn reload(&mut self, service: Service, category: Category) -> u64 {
        self.select_with_cache(service, category, false).0
    }

    pub(crate) fn reload_preserving_page(&mut self, service: Service, category: Category) -> u64 {
        let visible_page = self.page.take();
        let generation = self.reload(service, category);
        if let Some(page) = visible_page {
            self.status = if page.is_empty() {
                Status::Empty
            } else {
                Status::Results
            };
            self.page = Some(page);
        }
        generation
    }

    fn select_with_cache(
        &mut self,
        service: Service,
        category: Category,
        use_cache: bool,
    ) -> (u64, Option<Page>) {
        self.service = service;
        self.category = category;
        self.routes = vec![Route::root(service, category)];
        self.generation = self.generation.wrapping_add(1);
        self.tracks_enrichment_generation = None;
        if service == Service::Deezer && category == Category::Tracks {
            self.tracks_playback_generation_floor = self.generation;
        }
        self.page = None;
        self.status = Status::Loading;
        if !use_cache && service == Service::Deezer && category == Category::Tracks {
            self.tracks_pipeline = None;
        }
        if use_cache {
            return self.restore_cached_active_route();
        }
        (self.generation, None)
    }

    pub(crate) fn complete(&mut self, generation: u64, result: Result<Page, String>) -> bool {
        self.complete_with_cache(generation, result, true)
    }

    pub(crate) fn complete_preserving_page(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        if let Err(error) = result.as_ref()
            && self.page.is_some()
        {
            self.tracks_enrichment_generation = None;
            if error.as_str() == DEEZER_SESSION_EXPIRED {
                // The session-expired card must win over the visible page.
                self.status = Status::Failed(error.clone());
                self.page = None;
            }
            return true;
        }
        self.complete(generation, result)
    }

    #[cfg(test)]
    pub(crate) fn complete_tracks_preview(&mut self, generation: u64, page: Page) -> bool {
        if generation != self.generation || !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.status = if page.is_empty() {
            Status::Empty
        } else {
            Status::Results
        };
        self.page = Some(page);
        self.tracks_enrichment_generation = Some(generation);
        true
    }

    pub(crate) fn begin_tracks_pipeline(&mut self) -> u64 {
        self.next_tracks_pipeline_token = self.next_tracks_pipeline_token.wrapping_add(1);
        let token = self.next_tracks_pipeline_token;
        self.tracks_pipeline = Some(TracksPipeline {
            token,
            stage: TracksPipelineStage::Loading,
            page: None,
            in_flight: true,
        });
        token
    }

    pub(crate) fn fail_tracks_pipeline_load(
        &mut self,
        token: u64,
        generation: u64,
        error: String,
    ) -> TracksPipelineLoadOutcome {
        if self
            .tracks_pipeline
            .as_ref()
            .is_none_or(|pipeline| pipeline.token != token)
        {
            return TracksPipelineLoadOutcome::Ignored;
        }
        if error == DEEZER_SESSION_EXPIRED {
            if self.fail_tracks_session_expired(error) {
                return TracksPipelineLoadOutcome::Failed;
            }
            return TracksPipelineLoadOutcome::Ignored;
        }
        let has_visible_snapshot = generation == self.generation
            && self.deezer_root_active(Category::Tracks)
            && self.page.is_some();
        if !has_visible_snapshot {
            self.tracks_pipeline = None;
        }
        if generation != self.generation
            && self.deezer_root_active(Category::Tracks)
            && self.page.is_none()
        {
            let token = self.begin_tracks_pipeline();
            return TracksPipelineLoadOutcome::Restart {
                generation: self.generation,
                token,
            };
        }
        if generation == self.generation && self.deezer_root_active(Category::Tracks) {
            if has_visible_snapshot {
                if let Some(pipeline) = self
                    .tracks_pipeline
                    .as_mut()
                    .filter(|pipeline| pipeline.token == token)
                {
                    pipeline.stage = TracksPipelineStage::Preview;
                    pipeline.in_flight = false;
                }
                return TracksPipelineLoadOutcome::Ignored;
            }
            self.status = Status::Failed(error);
            self.page = None;
            TracksPipelineLoadOutcome::Failed
        } else {
            TracksPipelineLoadOutcome::Ignored
        }
    }

    /// An expired Deezer session invalidates every liked-tracks snapshot:
    /// the pipeline, the in-memory route cache entry, and the visible page
    /// all go away so neither a tab re-select nor a late disk-cache result
    /// can resurrect stale tracks. Returns whether the Deezer Tracks root
    /// is the active view and should surface the failure.
    fn fail_tracks_session_expired(&mut self, error: String) -> bool {
        self.tracks_pipeline = None;
        self.tracks_session_expired = true;
        self.cache.remove(&cache_key(
            &Route::root(Service::Deezer, Category::Tracks),
            &self.account_scope,
        ));
        if !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.status = Status::Failed(error);
        self.page = None;
        true
    }

    pub(crate) fn fail_tracks_pipeline_continuation(&mut self, token: u64) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::Preview;
        pipeline.in_flight = false;
        self.deezer_root_active(Category::Tracks)
    }

    pub(crate) fn tracks_pipeline_active(&self) -> bool {
        self.tracks_pipeline.as_ref().is_some_and(|pipeline| {
            pipeline.in_flight
                || matches!(
                    pipeline.stage,
                    TracksPipelineStage::Loading | TracksPipelineStage::Preview
                )
        })
    }

    pub(crate) fn tracks_pipeline_retryable(&self) -> bool {
        self.tracks_pipeline.as_ref().is_some_and(|pipeline| {
            !pipeline.in_flight
                && matches!(
                    pipeline.stage,
                    TracksPipelineStage::Preview | TracksPipelineStage::RawPendingHydration
                )
        })
    }

    pub(crate) fn begin_tracks_pipeline_continuation_retry(&mut self, token: u64) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        if pipeline.stage != TracksPipelineStage::Preview || pipeline.in_flight {
            return false;
        }
        pipeline.in_flight = true;
        true
    }

    pub(crate) fn accept_tracks_preview_pipeline(&mut self, token: u64, page: Page) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::Preview;
        pipeline.page = Some(page.clone());
        pipeline.in_flight = true;
        if !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.set_visible_tracks_page(page);
        true
    }

    pub(crate) fn accept_tracks_cached_pipeline(&mut self, token: u64, page: Page) -> bool {
        if self.tracks_session_expired {
            // A disk snapshot from the expired session must stay hidden even
            // when a fresh pipeline races it against the live load.
            return false;
        }
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::Preview;
        pipeline.page = Some(page.clone());
        pipeline.in_flight = true;
        if !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.set_visible_tracks_page(page);
        true
    }

    pub(crate) fn accept_tracks_tail_pipeline(&mut self, token: u64, page: Page) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::RawPendingHydration;
        pipeline.page = Some(page.clone());
        pipeline.in_flight = true;
        self.cache.insert(
            cache_key(
                &Route::root(Service::Deezer, Category::Tracks),
                &self.account_scope,
            ),
            page.clone(),
        );
        if !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.set_visible_tracks_page(page);
        true
    }

    pub(crate) fn accept_tracks_pipeline_enrichment(&mut self, token: u64, page: Page) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::Complete;
        pipeline.page = Some(page.clone());
        pipeline.in_flight = false;
        self.cache.insert(
            cache_key(
                &Route::root(Service::Deezer, Category::Tracks),
                &self.account_scope,
            ),
            page.clone(),
        );
        if !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.set_visible_tracks_page(page);
        true
    }

    pub(crate) fn fail_tracks_pipeline_enrichment(&mut self, token: u64) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        pipeline.stage = TracksPipelineStage::RawPendingHydration;
        pipeline.in_flight = false;
        self.deezer_root_active(Category::Tracks)
    }

    pub(crate) fn begin_tracks_pipeline_enrichment_retry(&mut self, token: u64) -> bool {
        let Some(pipeline) = self
            .tracks_pipeline
            .as_mut()
            .filter(|pipeline| pipeline.token == token)
        else {
            return false;
        };
        if pipeline.stage != TracksPipelineStage::RawPendingHydration || pipeline.in_flight {
            return false;
        }
        pipeline.in_flight = true;
        true
    }

    pub(crate) fn tracks_pipeline_token(&self) -> Option<u64> {
        self.tracks_pipeline.as_ref().map(|pipeline| pipeline.token)
    }

    fn set_visible_tracks_page(&mut self, page: Page) {
        self.tracks_session_expired = false;
        self.status = if page.is_empty() {
            Status::Empty
        } else {
            Status::Results
        };
        self.page = Some(page);
    }

    #[cfg(test)]
    pub(crate) fn complete_tracks_tail(&mut self, generation: u64, page: Page) -> bool {
        if generation != self.generation || !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        let cache_key = cache_key(self.route(), &self.account_scope);
        if !self.complete_tracks_preview(generation, page.clone()) {
            return false;
        }
        self.cache.insert(cache_key, page);
        true
    }

    pub(crate) fn active_tracks_load_id(&self) -> Option<u64> {
        self.deezer_root_active(Category::Tracks)
            .then_some(self.generation)
    }

    pub(crate) fn active_generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn tracks_playback_append_allowed(&self, load_id: u64) -> bool {
        load_id >= self.tracks_playback_generation_floor
    }

    #[cfg(test)]
    pub(crate) fn complete_tracks_enrichment(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
    ) -> bool {
        if generation != self.generation || !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.tracks_enrichment_generation = None;
        let Ok(page) = result else {
            return false;
        };
        self.complete(generation, Ok(page))
    }

    /// A stale snapshot remains usable when its background refresh fails.
    /// Without a snapshot, retain the existing first-load failure behavior.
    /// An expired Deezer session is the exception: the snapshot is dropped
    /// so the session-expired card can take over the view.
    #[cfg(test)]
    pub(crate) fn complete_tracks_refresh_failure(
        &mut self,
        generation: u64,
        error: String,
    ) -> bool {
        if generation != self.generation || !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.tracks_enrichment_generation = None;
        if error == DEEZER_SESSION_EXPIRED {
            return self.fail_tracks_session_expired(error);
        }
        if self.page.is_some() {
            return false;
        }
        self.status = Status::Failed(error);
        true
    }

    /// Finish an accepted enrichment attempt while keeping its raw preview.
    /// The return value says whether the failed result still belongs to the
    /// active Tracks generation and is therefore safe to persist as fallback.
    #[cfg(test)]
    pub(crate) fn accept_tracks_enrichment_failure(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.deezer_root_active(Category::Tracks) {
            return false;
        }
        self.tracks_enrichment_generation = None;
        self.page.is_some()
    }

    pub(crate) fn complete_nested(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
    ) -> bool {
        self.complete_with_cache(generation, result, true)
    }

    pub(crate) fn complete_nested_without_cache(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
    ) -> bool {
        self.complete_with_cache(generation, result, false)
    }

    fn complete_with_cache(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
        cache: bool,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.tracks_enrichment_generation = None;
        match result {
            Ok(page) => {
                self.status = if page.is_empty() {
                    Status::Empty
                } else {
                    Status::Results
                };
                if cache {
                    self.cache
                        .insert(cache_key(self.route(), &self.account_scope), page.clone());
                }
                self.page = Some(page);
            }
            Err(error) => self.status = Status::Failed(error),
        }
        true
    }

    pub(crate) fn account_required(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.tracks_playback_generation_floor = self.generation;
        self.tracks_enrichment_generation = None;
        self.status = Status::AccountRequired;
        self.page = None;
    }

    pub(crate) fn invalidate_deezer(&mut self, category: Category) {
        self.invalidate_provider(crate::search::Provider::Deezer, category);
    }

    pub(crate) fn invalidate_soundcloud(&mut self, category: Category) {
        self.invalidate_provider(crate::search::Provider::SoundCloud, category);
    }

    pub(crate) fn patch_playlist(
        &mut self,
        provider: crate::search::Provider,
        playlist: &OwnedPlaylist,
    ) {
        fn patch_card(
            card: &mut Card,
            provider: crate::search::Provider,
            playlist: &OwnedPlaylist,
        ) {
            if card.source != provider || card.kind != Category::Playlists || card.id != playlist.id
            {
                return;
            }
            card.title.clone_from(&playlist.title);
            card.subtitle = super::playlist_state::updated_playlist_subtitle(
                &card.subtitle,
                &playlist.owner.name,
            );
            if !playlist.artwork.is_empty() {
                card.artwork.clone_from(&playlist.artwork);
            }
            card.is_private = Some(playlist.is_private);
        }

        fn patch_page(
            page: &mut Page,
            provider: crate::search::Provider,
            playlist: &OwnedPlaylist,
        ) {
            for card in &mut page.cards {
                patch_card(card, provider, playlist);
            }
            for section in &mut page.sections {
                for card in &mut section.cards {
                    patch_card(card, provider, playlist);
                }
            }
        }

        let active_matches = self.routes.last().is_some_and(|route| {
            route.source == provider
                && super::playlist_state::matching_playlist_route(
                    route.source,
                    &route.action,
                    &route.id,
                    &playlist.id,
                )
        });
        if active_matches {
            if let Some(route) = self.routes.last_mut() {
                route.title.clone_from(&playlist.title);
                route.subtitle = playlist.owner.name.clone();
                if !playlist.artwork.is_empty() {
                    route.artwork.clone_from(&playlist.artwork);
                }
            }
            if let Some(page) = self.page.as_mut() {
                page.title.clone_from(&playlist.title);
                page.subtitle = playlist.owner.name.clone();
                page.description.clone_from(&playlist.description);
                if !playlist.artwork.is_empty() {
                    page.artwork.clone_from(&playlist.artwork);
                }
            }
        }
        if let Some(page) = self.page.as_mut() {
            patch_page(page, provider, playlist);
        }
        for ((route, scope), page) in &mut self.cache {
            if scope == &self.account_scope {
                patch_page(page, provider, playlist);
                if route.source == provider
                    && super::playlist_state::matching_playlist_route(
                        route.source,
                        &route.action,
                        &route.id,
                        &playlist.id,
                    )
                {
                    page.title.clone_from(&playlist.title);
                    page.subtitle = playlist.owner.name.clone();
                    page.description.clone_from(&playlist.description);
                    if !playlist.artwork.is_empty() {
                        page.artwork.clone_from(&playlist.artwork);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn patch_deezer_playlist(&mut self, playlist: &OwnedPlaylist) {
        self.patch_playlist(crate::search::Provider::Deezer, playlist);
    }

    pub(crate) fn invalidate_provider(
        &mut self,
        provider: crate::search::Provider,
        category: Category,
    ) {
        self.cache.retain(|(route, scope), _| {
            !(route.source == provider
                && route.category == category
                && scope == &self.account_scope)
        });
        if provider == crate::search::Provider::Deezer && category == Category::Tracks {
            self.tracks_pipeline = None;
            // Block every older live Tracks load from extending playback even
            // when its UI route was already left. The next generated load ID
            // remains eligible without changing an unrelated active route.
            self.tracks_playback_generation_floor = self.generation.wrapping_add(1);
            let tracks_load_pending = self.deezer_root_active(Category::Tracks)
                && (self.status == Status::Loading
                    || self.tracks_enrichment_generation == Some(self.generation));
            if tracks_load_pending {
                self.generation = self.generation.wrapping_add(1);
                self.tracks_enrichment_generation = None;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn invalidate_soundcloud_history(&mut self) {
        self.cache.retain(|(route, scope), _| {
            !(route.source == crate::search::Provider::SoundCloud
                && route.category == Category::History
                && scope == &self.account_scope)
        });
    }

    pub(crate) fn reload_active_route(&mut self) -> (u64, Route) {
        let reloads_deezer_tracks = self.deezer_root_active(Category::Tracks);
        self.generation = self.generation.wrapping_add(1);
        self.tracks_enrichment_generation = None;
        if reloads_deezer_tracks {
            self.tracks_playback_generation_floor = self.generation;
            self.tracks_pipeline = None;
        }
        self.status = Status::Loading;
        self.page = None;
        (self.generation, self.route().clone())
    }

    pub(crate) fn deezer_root_active(&self, category: Category) -> bool {
        self.service == Service::Deezer && self.category == category && self.routes.len() == 1
    }

    pub(crate) fn selected_root_active(&self, category: Category) -> bool {
        self.category == category && self.routes.len() == 1
    }

    pub(crate) fn push(&mut self, route: Route) -> (u64, Option<Page>) {
        self.generation = self.generation.wrapping_add(1);
        self.tracks_enrichment_generation = None;
        self.routes.push(route);
        self.page = None;
        self.status = Status::Loading;
        self.restore_cached_active_route()
    }

    pub(crate) fn back(&mut self) -> Option<(u64, Route, Option<Page>)> {
        if self.routes.len() <= 1 {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.tracks_enrichment_generation = None;
        self.routes.pop();
        self.page = None;
        self.status = Status::Loading;
        let (generation, cached) = self.restore_cached_active_route();
        Some((generation, self.route().clone(), cached))
    }

    fn restore_cached_active_route(&mut self) -> (u64, Option<Page>) {
        if self.deezer_root_active(Category::Tracks)
            && let Some(page) = self
                .tracks_pipeline
                .as_ref()
                .and_then(|pipeline| pipeline.page.clone())
        {
            self.status = if page.is_empty() {
                Status::Empty
            } else {
                Status::Results
            };
            self.page = Some(page.clone());
            return (self.generation, Some(page));
        }
        if let Some(page) = self
            .cache
            .get(&cache_key(self.route(), &self.account_scope))
        {
            self.status = if page.is_empty() {
                Status::Empty
            } else {
                Status::Results
            };
            let page = page.clone();
            self.page = Some(page);
            (self.generation, self.page.clone())
        } else {
            (self.generation, None)
        }
    }

    #[cfg(test)]
    pub(crate) fn has_cached(&self, service: Service, category: Category) -> bool {
        self.cache.contains_key(&cache_key(
            &Route::root(service, category),
            &self.account_scope,
        ))
    }

    pub(crate) fn route(&self) -> &Route {
        self.routes.last().expect("library always has a root route")
    }

    pub(crate) fn append_active_soundcloud_station_tracks(
        &mut self,
        expected_seed: &str,
        additions: &[Track],
    ) -> usize {
        let active_route = self.route().clone();
        let cache_key = cache_key(&active_route, &self.account_scope);
        let expected_seed = expected_seed.trim();
        if active_route.source != crate::search::Provider::SoundCloud
            || active_route.action != "stationTracks"
            || self.status != Status::Results
            || expected_seed.is_empty()
        {
            return 0;
        }

        let Some(page) = self.page.as_mut() else {
            return 0;
        };
        let Some(last_track_id) = page.tracks.last().map(|track| track.id.trim()) else {
            return 0;
        };
        if last_track_id.is_empty()
            || !last_track_id
                .chars()
                .all(|character| character.is_ascii_digit())
            || last_track_id != expected_seed
        {
            return 0;
        }

        let mut existing_ids = page
            .tracks
            .iter()
            .map(|track| track.id.trim().to_owned())
            .collect::<HashSet<_>>();
        let fresh = additions
            .iter()
            .filter(|track| existing_ids.insert(track.id.trim().to_owned()))
            .cloned()
            .collect::<Vec<_>>();
        if fresh.is_empty() {
            return 0;
        }

        let appended = fresh.len();
        page.tracks.extend(fresh);
        page.total = page.tracks.len();
        self.cache.insert(cache_key, page.clone());
        appended
    }

    /// Synchronize a Deezer SmartMix page with a queue-extension response.
    ///
    /// The playback queue owns the cursor, and queue extensions append like
    /// SoundCloud stations, so the page keeps every listed track and adds
    /// only the fresh batch below them.
    pub(crate) fn sync_active_deezer_smart_mix_tracks(
        &mut self,
        expected_config_id: &str,
        additions: &[Track],
    ) -> usize {
        let active_route = self.route().clone();
        let expected_config_id = expected_config_id.trim();
        if active_route.source != crate::search::Provider::Deezer
            || active_route.category != Category::Flow
            || active_route.action != "flowTracks"
            || active_route.id.trim() != expected_config_id
            || expected_config_id.is_empty()
            || self.status != Status::Results
        {
            return 0;
        }

        let cache_key = cache_key(&active_route, &self.account_scope);
        let Some(page) = self.page.as_mut() else {
            return 0;
        };
        let mut existing_ids = page
            .tracks
            .iter()
            .map(|track| track.id.trim().to_owned())
            .filter(|id| !id.is_empty())
            .collect::<HashSet<_>>();
        let fresh = additions
            .iter()
            .filter(|track| !track.id.trim().is_empty())
            .filter(|track| existing_ids.insert(track.id.trim().to_owned()))
            .cloned()
            .collect::<Vec<_>>();
        if fresh.is_empty() {
            return 0;
        }

        let appended = fresh.len();
        page.tracks.extend(fresh);
        page.total = page.tracks.len();
        self.cache.insert(cache_key, page.clone());
        appended
    }

    pub(crate) fn update_active_deezer_smart_mix_title(
        &mut self,
        expected_config_id: &str,
        title: &str,
    ) -> bool {
        let expected_config_id = expected_config_id.trim();
        let Some(title) = specific_smart_mix_title(title) else {
            return false;
        };
        let active_route = self.route();
        if expected_config_id.is_empty()
            || active_route.source != crate::search::Provider::Deezer
            || active_route.category != Category::Flow
            || active_route.action != "flowTracks"
            || active_route.id.trim() != expected_config_id
            || self.status != Status::Results
        {
            return false;
        }
        let cache_key = cache_key(active_route, &self.account_scope);

        let mut changed = false;
        if let Some(route) = self.routes.last_mut()
            && route.title != title
        {
            route.title = title.to_owned();
            changed = true;
        }
        if let Some(page) = self.page.as_mut()
            && page.title != title
        {
            page.title = title.to_owned();
            changed = true;
        }
        if changed && let Some(page) = self.cache.get_mut(&cache_key) {
            page.title = title.to_owned();
        }
        changed
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
