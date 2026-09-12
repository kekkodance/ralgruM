use std::collections::{HashMap, HashSet};

use super::{
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
        if result.is_err() && self.page.is_some() {
            self.tracks_enrichment_generation = None;
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
        self.tracks_pipeline = None;
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
            self.status = Status::Failed(error);
            self.page = None;
            TracksPipelineLoadOutcome::Failed
        } else {
            TracksPipelineLoadOutcome::Ignored
        }
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
    /// The playback queue owns the cursor, so the caller supplies its current
    /// track id.  When Deezer asks us to clear the remaining queue, preserve
    /// everything through the last displayed occurrence of that track.  If
    /// the cursor is not represented by the page, preserve the entire page as
    /// the conservative fallback.
    pub(crate) fn sync_active_deezer_smart_mix_tracks(
        &mut self,
        expected_config_id: &str,
        current_track_id: Option<&str>,
        additions: &[Track],
        clear_remaining_tracks: bool,
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
        let preserved_len = if clear_remaining_tracks {
            current_track_id
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .and_then(|current_id| {
                    page.tracks
                        .iter()
                        .rposition(|track| track.id.trim() == current_id)
                })
                .map_or(page.tracks.len(), |index| index + 1)
        } else {
            page.tracks.len()
        };

        let removed = page.tracks.len().saturating_sub(preserved_len);
        let mut existing_ids = page.tracks[..preserved_len]
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
        if fresh.is_empty() && removed == 0 {
            return 0;
        }

        if removed > 0 {
            page.tracks.truncate(preserved_len);
        }
        let changed = removed + fresh.len();
        page.tracks.extend(fresh);
        page.total = page.tracks.len();
        self.cache.insert(cache_key, page.clone());
        changed
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
mod tests {
    use super::*;

    #[test]
    fn stale_completion_does_not_replace_current_page() {
        let mut state = LibraryState::default();
        let old = state.select(Service::Deezer, Category::Tracks).0;
        let new = state.select(Service::Deezer, Category::History).0;
        assert!(!state.complete(old, Ok(Page::default())));
        assert_eq!(state.category, Category::History);
        assert!(state.complete(new, Ok(Page::default())));
    }

    #[test]
    fn cache_is_used_after_success() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(
            generation,
            Ok(Page {
                total: 1,
                ..Page::default()
            }),
        );
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
    }

    #[test]
    fn forced_root_reload_bypasses_cache_without_discarding_it() {
        let mut state = LibraryState::default();
        let cached_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(
            cached_generation,
            Ok(Page {
                title: "cached".into(),
                total: 1,
                ..Page::default()
            }),
        );

        let reload_generation = state.reload(Service::Deezer, Category::Tracks);
        assert!(state.page.is_none());
        assert_eq!(state.status, Status::Loading);
        assert!(!state.complete(cached_generation, Ok(Page::default())));
        assert!(state.complete(reload_generation, Err("failed".into())));
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
    }

    #[test]
    fn background_root_reload_keeps_the_visible_page() {
        let mut state = LibraryState::default();
        let loaded = state.select(Service::Deezer, Category::History).0;
        state.complete(
            loaded,
            Ok(Page {
                title: "Visible history".into(),
                total: 1,
                tracks: vec![Track::default()],
                ..Page::default()
            }),
        );

        let refresh = state.reload_preserving_page(Service::Deezer, Category::History);

        assert_eq!(state.status, Status::Results);
        assert_eq!(
            state.page.as_ref().map(|page| page.title.as_str()),
            Some("Visible history")
        );
        assert!(state.complete_preserving_page(refresh, Err("offline".into())));
        assert_eq!(state.status, Status::Results);
        assert_eq!(
            state.page.as_ref().map(|page| page.title.as_str()),
            Some("Visible history")
        );
    }

    #[test]
    fn cache_is_scoped_by_service() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(generation, Ok(Page::default()));
        assert!(
            state
                .select(Service::SoundCloud, Category::Tracks)
                .1
                .is_none()
        );
    }

    #[test]
    fn soundcloud_history_invalidation_removes_only_history_cache() {
        let mut state = LibraryState::default();
        let history = state.select(Service::SoundCloud, Category::History).0;
        state.complete(
            history,
            Ok(Page {
                total: 1,
                ..Page::default()
            }),
        );
        let tracks = state.select(Service::SoundCloud, Category::Tracks).0;
        state.complete(
            tracks,
            Ok(Page {
                total: 1,
                ..Page::default()
            }),
        );
        state.invalidate_soundcloud_history();
        assert!(!state.has_cached(Service::SoundCloud, Category::History));
        assert!(state.has_cached(Service::SoundCloud, Category::Tracks));
    }

    #[test]
    fn deezer_history_invalidation_removes_only_deezer_history_cache() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        for (service, category) in [
            (Service::Deezer, Category::History),
            (Service::Deezer, Category::Tracks),
            (Service::SoundCloud, Category::History),
        ] {
            let generation = state.select(service, category).0;
            state.complete(
                generation,
                Ok(Page {
                    total: 1,
                    ..Page::default()
                }),
            );
        }

        state.invalidate_deezer(Category::History);

        assert!(!state.has_cached(Service::Deezer, Category::History));
        assert!(state.has_cached(Service::Deezer, Category::Tracks));
        assert!(state.has_cached(Service::SoundCloud, Category::History));
    }

    #[test]
    fn changing_account_scope_hides_visible_page_and_preserves_scoped_caches() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(
            generation,
            Ok(Page {
                title: "first account".into(),
                total: 1,
                ..Page::default()
            }),
        );
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());

        assert!(state.set_account_scope("two".into()));
        assert!(state.page.is_none());
        assert_eq!(state.status, Status::Initial);
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_none());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(
            generation,
            Ok(Page {
                title: "second account".into(),
                total: 1,
                ..Page::default()
            }),
        );

        assert!(state.set_account_scope("one".into()));
        assert_eq!(
            state
                .select(Service::Deezer, Category::Tracks)
                .1
                .unwrap()
                .title,
            "first account"
        );
    }

    #[test]
    fn account_scope_change_invalidates_pending_completion_but_keeps_old_cache() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let old_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(old_generation, Ok(Page::default()));

        let pending_generation = state.select(Service::Deezer, Category::History).0;
        assert!(state.set_account_scope("two".into()));
        assert!(!state.complete(pending_generation, Ok(Page::default())));

        assert!(state.set_account_scope("one".into()));
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
    }

    #[test]
    fn missing_account_and_empty_page_have_distinct_states() {
        let mut state = LibraryState::default();
        state.account_required();
        assert_eq!(state.status, Status::AccountRequired);
        let generation = state.select(Service::Deezer, Category::History).0;
        state.complete(generation, Ok(Page::default()));
        assert_eq!(state.status, Status::Empty);
        assert!(state.page.is_some());
    }

    #[test]
    fn nested_routes_push_and_back_to_the_exact_root() {
        let mut state = LibraryState::default();
        state.select(Service::Deezer, Category::Albums);
        let route = Route {
            source: crate::search::Provider::Deezer,
            category: Category::Albums,
            action: "albumTracks".into(),
            id: "42".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: String::new(),
            release_date: String::new(),
        };
        state.push(route.clone());
        assert_eq!(
            state.routes,
            vec![Route::root(Service::Deezer, Category::Albums), route]
        );
        let (_, root, _) = state.back().unwrap();
        assert_eq!(root, Route::root(Service::Deezer, Category::Albums));
        assert!(state.back().is_none());
    }

    #[test]
    fn back_rejects_a_stale_nested_completion() {
        let mut state = LibraryState::default();
        let (generation, _) = state.push(Route {
            source: crate::search::Provider::Deezer,
            category: Category::Playlists,
            action: "playlistTracks".into(),
            id: "7".into(),
            title: "Playlist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        state.back();
        assert!(!state.complete(generation, Ok(Page::default())));
        assert_eq!(state.routes.len(), 1);
    }

    #[test]
    fn nested_page_does_not_replace_the_root_cache() {
        let mut state = LibraryState::default();
        let root_generation = state.select(Service::SoundCloud, Category::Artists).0;
        let root = Page {
            title: "Followed Artists".into(),
            ..Page::default()
        };
        state.complete(root_generation, Ok(root.clone()));

        let (nested_generation, _) = state.push(Route {
            source: crate::search::Provider::SoundCloud,
            category: Category::Artists,
            action: "artistTracks".into(),
            id: "42".into(),
            title: "Artist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        state.complete_nested(
            nested_generation,
            Ok(Page {
                title: "Artist".into(),
                ..Page::default()
            }),
        );

        assert_eq!(
            state
                .select(Service::SoundCloud, Category::Artists)
                .1
                .unwrap()
                .title,
            root.title
        );
    }

    #[test]
    fn track_mutation_invalidates_only_the_active_accounts_deezer_tracks_cache() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(generation, Ok(Page::default()));
        state.invalidate_deezer(Category::Tracks);
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_none());

        state.set_account_scope("two".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(generation, Ok(Page::default()));
        state.set_account_scope("one".into());
        state.invalidate_deezer(Category::Tracks);
        state.set_account_scope("two".into());
        assert!(state.select(Service::Deezer, Category::Tracks).1.is_some());
    }

    #[test]
    fn provider_invalidation_keeps_the_other_services_collection_cache() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());

        let generation = state.select(Service::Deezer, Category::Albums).0;
        state.complete(generation, Ok(Page::default()));
        let generation = state.select(Service::SoundCloud, Category::Albums).0;
        state.complete(generation, Ok(Page::default()));

        state.invalidate_provider(crate::search::Provider::SoundCloud, Category::Albums);

        assert!(state.has_cached(Service::Deezer, Category::Albums));
        assert!(!state.has_cached(Service::SoundCloud, Category::Albums));
    }

    #[test]
    fn playlist_delete_pops_exact_detail_and_invalidates_only_playlist_cache() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Playlists).0;
        state.complete(generation, Ok(Page::default()));
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete(generation, Ok(Page::default()));
        state.select(Service::Deezer, Category::Playlists);
        state.push(Route {
            source: crate::search::Provider::Deezer,
            category: Category::Playlists,
            action: "playlistTracks".into(),
            id: "42".into(),
            title: "Title".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        state.invalidate_deezer(Category::Playlists);
        state.back();
        assert_eq!(state.routes.len(), 1);
        assert!(!state.has_cached(Service::Deezer, Category::Playlists));
        assert!(state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn playlist_update_patches_visible_and_cached_cards_before_refresh() {
        use crate::library::{
            model::{Card, Section},
            playlist_client::{OwnedPlaylist, PlaylistOwner},
        };

        fn playlist_card(artwork: &str) -> Card {
            Card {
                kind: Category::Playlists,
                id: "42".into(),
                title: "Old title".into(),
                subtitle: "Old owner".into(),
                artwork: artwork.into(),
                source: crate::search::Provider::Deezer,
                ..Card::default()
            }
        }

        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Playlists).0;
        state.complete(
            generation,
            Ok(Page {
                cards: vec![playlist_card("old-root")],
                sections: vec![Section {
                    cards: vec![playlist_card("old-section")],
                    ..Section::default()
                }],
                ..Page::default()
            }),
        );
        let playlist = OwnedPlaylist {
            id: "42".into(),
            title: "New title".into(),
            description: String::new(),
            is_private: false,
            is_from_favorite_tracks: false,
            is_collaborative: false,
            owner: PlaylistOwner {
                id: "7".into(),
                name: "New owner".into(),
            },
            artwork: "new-art".into(),
            track_count: None,
        };

        state.patch_deezer_playlist(&playlist);

        let page = state.page.as_ref().unwrap();
        for card in [&page.cards[0], &page.sections[0].cards[0]] {
            assert_eq!(card.title, "New title");
            assert_eq!(card.subtitle, "New owner");
            assert_eq!(card.artwork, "new-art");
        }
        let cached = state
            .cache
            .get(&(state.route().clone(), "one".into()))
            .unwrap();
        assert_eq!(cached.cards[0].artwork, "new-art");
    }

    #[test]
    fn deezer_tracks_root_is_the_only_favorite_refresh_route() {
        let mut state = LibraryState::default();
        assert!(state.deezer_root_active(Category::Tracks));

        state.select(Service::Deezer, Category::History);
        assert!(!state.deezer_root_active(Category::Tracks));

        state.select(Service::SoundCloud, Category::Tracks);
        assert!(!state.deezer_root_active(Category::Tracks));

        state.select(Service::Deezer, Category::Tracks);
        state.push(Route {
            source: crate::search::Provider::Deezer,
            category: Category::Albums,
            action: "albumTracks".into(),
            id: "42".into(),
            title: "Album".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        assert!(!state.deezer_root_active(Category::Tracks));
    }

    #[test]
    fn collection_refresh_eligibility_requires_the_exact_active_root() {
        let mut state = LibraryState::default();
        for category in [Category::Albums, Category::Artists, Category::Playlists] {
            state.select(Service::Deezer, category);
            assert!(state.deezer_root_active(category));
            assert!(!state.deezer_root_active(Category::Tracks));
        }
        state.select(Service::Deezer, Category::Albums);
        state.push(Route {
            source: crate::search::Provider::Deezer,
            category: Category::Albums,
            action: "albumTracks".into(),
            id: "42".into(),
            title: "Album".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        assert!(!state.deezer_root_active(Category::Albums));
    }

    #[test]
    fn deezer_history_refresh_eligibility_requires_the_exact_active_root() {
        let mut state = LibraryState::default();
        state.select(Service::Deezer, Category::History);
        assert!(state.deezer_root_active(Category::History));

        state.push(Route {
            source: crate::search::Provider::Deezer,
            category: Category::History,
            action: "historyTrack".into(),
            id: "42".into(),
            title: "Track".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        assert!(!state.deezer_root_active(Category::History));

        state.select(Service::SoundCloud, Category::History);
        assert!(!state.deezer_root_active(Category::History));
        state.select(Service::Deezer, Category::Tracks);
        assert!(!state.deezer_root_active(Category::History));
    }

    #[test]
    fn selected_root_eligibility_includes_soundcloud_collections() {
        let mut state = LibraryState::default();
        state.select(Service::SoundCloud, Category::Artists);
        assert!(state.selected_root_active(Category::Artists));

        state.push(Route {
            source: crate::search::Provider::SoundCloud,
            category: Category::Artists,
            action: "artistTracks".into(),
            id: "42".into(),
            title: "Artist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });
        assert!(!state.selected_root_active(Category::Artists));
    }

    #[test]
    fn account_scope_changes_preserve_the_local_page() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Local, Category::Tracks).0;
        state.complete_nested_without_cache(generation, Ok(tracks_page("Local Tracks")));
        let page = state.page.clone();
        let route = state.route().clone();

        assert!(!state.set_account_scope("another account".into()));
        assert_eq!(state.route(), &route);
        assert_eq!(state.status, Status::Results);
        assert_eq!(
            state.page.as_ref().map(|page| format!("{page:?}")),
            page.as_ref().map(|page| format!("{page:?}"))
        );
    }

    fn tracks_page(title: &str) -> Page {
        Page {
            title: title.into(),
            tracks: vec![super::super::model::Track {
                id: "42".into(),
                ..super::super::model::Track::default()
            }],
            ..Page::default()
        }
    }

    fn soundcloud_station_route() -> Route {
        Route {
            source: crate::search::Provider::SoundCloud,
            category: Category::Station,
            action: "stationTracks".into(),
            id: "station".into(),
            title: "Station".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        }
    }

    fn soundcloud_track(id: &str) -> Track {
        Track {
            id: id.into(),
            ..Track::default()
        }
    }

    fn active_soundcloud_station_state(ids: &[&str]) -> LibraryState {
        let mut state = LibraryState::default();
        state.set_account_scope("soundcloud-account".into());
        state.select(Service::SoundCloud, Category::Station);
        let generation = state.push(soundcloud_station_route()).0;
        state.complete_nested(
            generation,
            Ok(Page {
                show_count: false,
                total: ids.len(),
                raw_loaded_count: 17,
                normalized_count: 13,
                authoritative_total: Some(99),
                tracks: ids.iter().map(|id| soundcloud_track(id)).collect(),
                ..Page::default()
            }),
        );
        state
    }

    fn smart_mix_route() -> Route {
        Route {
            source: crate::search::Provider::Deezer,
            category: Category::Flow,
            action: "flowTracks".into(),
            id: "inspired-by-3".into(),
            title: "Riddim Dubstep".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        }
    }

    fn active_deezer_smart_mix_state(ids: &[&str]) -> LibraryState {
        let mut state = LibraryState::default();
        state.set_account_scope("deezer-account".into());
        state.select(Service::Deezer, Category::Flow);
        let generation = state.push(smart_mix_route()).0;
        state.complete_nested(
            generation,
            Ok(Page {
                total: ids.len(),
                tracks: ids.iter().map(|id| soundcloud_track(id)).collect(),
                ..Page::default()
            }),
        );
        state
    }

    fn assert_state_unchanged(
        state: &LibraryState,
        route: &Route,
        page: &Option<Page>,
        cache: &HashMap<(Route, String), Page>,
    ) {
        assert_eq!(state.route(), route);
        assert_eq!(state.status, Status::Results);
        assert_eq!(
            state.page.as_ref().map(|page| format!("{page:?}")),
            page.as_ref().map(|page| format!("{page:?}"))
        );
        assert_eq!(format!("{:?}", state.cache), format!("{:?}", cache));
    }

    #[test]
    fn soundcloud_station_extension_appends_fresh_tracks_and_updates_cache() {
        let mut state = active_soundcloud_station_state(&["10", "123"]);
        let route = state.route().clone();
        let additions = [
            soundcloud_track(" 123 "),
            soundcloud_track("200"),
            soundcloud_track("200"),
            soundcloud_track("300"),
        ];

        let appended = state.append_active_soundcloud_station_tracks(" 123 ", &additions);

        assert_eq!(appended, 2);
        let page = state.page.as_ref().unwrap();
        assert_eq!(
            page.tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["10", "123", "200", "300"]
        );
        assert_eq!(page.total, page.tracks.len());
        assert!(!page.show_count);
        assert_eq!(page.raw_loaded_count, 17);
        assert_eq!(page.normalized_count, 13);
        assert_eq!(page.authoritative_total, Some(99));

        state.back();
        let (_, cached) = state.push(route);
        let cached = cached.unwrap();
        assert_eq!(
            cached
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["10", "123", "200", "300"]
        );
        assert_eq!(state.page.as_ref().unwrap().tracks, cached.tracks);
    }

    #[test]
    fn soundcloud_station_extension_rejects_invalid_guards_without_mutation() {
        let mut mismatched_seed = active_soundcloud_station_state(&["10", "123"]);
        let route = mismatched_seed.route().clone();
        let page = mismatched_seed.page.clone();
        let cache = mismatched_seed.cache.clone();
        assert_eq!(
            mismatched_seed
                .append_active_soundcloud_station_tracks("999", &[soundcloud_track("200")]),
            0
        );
        assert_state_unchanged(&mismatched_seed, &route, &page, &cache);

        for (source, action) in [
            (crate::search::Provider::Deezer, "stationTracks"),
            (crate::search::Provider::SoundCloud, "station"),
        ] {
            let mut wrong_route = active_soundcloud_station_state(&["10", "123"]);
            wrong_route.routes.last_mut().unwrap().source = source;
            wrong_route.routes.last_mut().unwrap().action = action.into();
            let route = wrong_route.route().clone();
            let page = wrong_route.page.clone();
            let cache = wrong_route.cache.clone();
            assert_eq!(
                wrong_route
                    .append_active_soundcloud_station_tracks("123", &[soundcloud_track("200")]),
                0
            );
            assert_state_unchanged(&wrong_route, &route, &page, &cache);
        }

        for invalid_last_id in ["", "not-numeric"] {
            let mut invalid_page = active_soundcloud_station_state(&["10", "123"]);
            invalid_page
                .page
                .as_mut()
                .unwrap()
                .tracks
                .last_mut()
                .unwrap()
                .id = invalid_last_id.into();
            let route = invalid_page.route().clone();
            let page = invalid_page.page.clone();
            let cache = invalid_page.cache.clone();
            assert_eq!(
                invalid_page
                    .append_active_soundcloud_station_tracks("123", &[soundcloud_track("200")]),
                0
            );
            assert_state_unchanged(&invalid_page, &route, &page, &cache);
        }
    }

    #[test]
    fn soundcloud_station_extension_returns_zero_for_duplicate_only_batches() {
        let mut state = active_soundcloud_station_state(&["10", "123"]);
        let route = state.route().clone();
        let page = state.page.clone();
        let cache = state.cache.clone();

        assert_eq!(
            state.append_active_soundcloud_station_tracks(
                "123",
                &[
                    soundcloud_track(" 123 "),
                    soundcloud_track("10"),
                    soundcloud_track("10"),
                ]
            ),
            0
        );
        assert_state_unchanged(&state, &route, &page, &cache);
    }

    #[test]
    fn smart_mix_extension_appends_fresh_tracks_and_updates_cache() {
        let mut state = active_deezer_smart_mix_state(&["1", "2"]);
        let route = state.route().clone();

        let appended = state.sync_active_deezer_smart_mix_tracks(
            "inspired-by-3",
            Some("2"),
            &[
                soundcloud_track("2"),
                soundcloud_track("3"),
                soundcloud_track("3"),
            ],
            false,
        );

        assert_eq!(appended, 1);
        assert_eq!(
            state
                .page
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3"]
        );
        assert_eq!(state.page.as_ref().unwrap().total, 3);
        assert_eq!(
            state
                .cache
                .get(&cache_key(&route, "deezer-account"))
                .unwrap()
                .tracks
                .len(),
            3
        );
    }

    #[test]
    fn smart_mix_clear_extension_replaces_only_unplayed_visible_tail() {
        let mut state = active_deezer_smart_mix_state(&["1", "2", "3", "4"]);

        let appended = state.sync_active_deezer_smart_mix_tracks(
            "inspired-by-3",
            Some("2"),
            &[
                soundcloud_track("3"),
                soundcloud_track("5"),
                soundcloud_track("5"),
            ],
            true,
        );

        assert_eq!(appended, 4);
        assert_eq!(
            state
                .page
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3", "5"]
        );
    }

    #[test]
    fn smart_mix_clear_extension_preserves_page_when_cursor_is_not_visible() {
        let mut state = active_deezer_smart_mix_state(&["1", "2", "3"]);

        let appended = state.sync_active_deezer_smart_mix_tracks(
            "inspired-by-3",
            Some("missing"),
            &[soundcloud_track("4")],
            true,
        );

        assert_eq!(appended, 1);
        assert_eq!(
            state
                .page
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "3", "4"]
        );
    }

    #[test]
    fn smart_mix_extension_rejects_stale_or_mismatched_pages_without_mutation() {
        let mut state = active_deezer_smart_mix_state(&["1", "2"]);
        let route = state.route().clone();
        let page = state.page.clone();
        let cache = state.cache.clone();

        assert_eq!(
            state.sync_active_deezer_smart_mix_tracks(
                "other-mix",
                Some("2"),
                &[soundcloud_track("3")],
                false,
            ),
            0
        );
        assert_state_unchanged(&state, &route, &page, &cache);

        state.status = Status::Loading;
        assert_eq!(
            state.sync_active_deezer_smart_mix_tracks(
                "inspired-by-3",
                Some("2"),
                &[soundcloud_track("3")],
                false,
            ),
            0
        );
        assert_eq!(
            state.page.as_ref().map(|page| format!("{page:?}")),
            page.as_ref().map(|page| format!("{page:?}"))
        );
        assert_eq!(format!("{:?}", state.cache), format!("{:?}", cache));
    }

    #[test]
    fn smart_mix_duplicate_only_clear_batch_still_truncates_visible_tail() {
        let mut state = active_deezer_smart_mix_state(&["1", "2", "3"]);

        assert_eq!(
            state.sync_active_deezer_smart_mix_tracks(
                "inspired-by-3",
                Some("2"),
                &[soundcloud_track("1"), soundcloud_track("2")],
                true,
            ),
            1
        );
        assert_eq!(
            state
                .page
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        assert_eq!(state.page.as_ref().unwrap().total, 2);
        assert_eq!(
            state
                .cache
                .get(&cache_key(state.route(), "deezer-account"))
                .unwrap()
                .tracks
                .len(),
            2
        );
    }

    #[test]
    fn smart_mix_title_handoff_updates_the_active_route_and_page() {
        let mut state = active_deezer_smart_mix_state(&["1", "2"]);
        state.routes.last_mut().unwrap().title = "daily".into();
        state.page.as_mut().unwrap().title = "daily".into();

        assert!(state.update_active_deezer_smart_mix_title("inspired-by-3", "Electro Dance"));
        assert_eq!(state.route().title, "Electro Dance");
        assert_eq!(state.page.as_ref().unwrap().title, "Electro Dance");
        assert!(!state.update_active_deezer_smart_mix_title("other-mix", "Stale title"));
        assert_eq!(state.route().title, "Electro Dance");
        assert_eq!(
            state
                .cache
                .get(&cache_key(state.route(), "deezer-account"))
                .map(|page| page.title.as_str()),
            Some("Electro Dance")
        );
    }

    #[test]
    fn smart_mix_title_handoff_rejects_generic_placeholders() {
        let mut state = active_deezer_smart_mix_state(&["1", "2"]);
        state.routes.last_mut().unwrap().title = "Electro Dance".into();
        state.page.as_mut().unwrap().title = "Electro Dance".into();

        assert!(!state.update_active_deezer_smart_mix_title("inspired-by-3", "daily"));
        assert_eq!(state.route().title, "Electro Dance");
        assert_eq!(state.page.as_ref().unwrap().title, "Electro Dance");
    }

    #[test]
    fn smart_mix_cache_identity_ignores_presentation_fields() {
        let mut state = active_deezer_smart_mix_state(&["1"]);
        state.back();

        let mut reopened = smart_mix_route();
        reopened.id = " inspired-by-3 ".into();
        reopened.title = "A different card title".into();
        reopened.subtitle = "A different subtitle".into();
        reopened.artwork = "new-artwork".into();
        reopened.release_date = "2026-09-10".into();

        let (_, cached) = state.push(reopened);
        assert_eq!(cached.as_ref().map(|page| page.tracks.len()), Some(1));
    }

    #[test]
    fn smart_mix_cache_isolated_by_account_scope() {
        let mut state = active_deezer_smart_mix_state(&["1"]);
        state.back();
        assert!(state.set_account_scope("another-deezer-account".into()));

        let (_, cached) = state.push(smart_mix_route());
        assert!(cached.is_none());
    }

    #[test]
    fn preview_completion_is_visible_but_not_cached() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;

        assert!(state.complete_tracks_preview(generation, tracks_page("raw")));
        assert_eq!(state.status, Status::Results);
        assert_eq!(state.page.as_ref().unwrap().title, "raw");
        assert!(!state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn accepted_tail_preserves_the_stable_load_id_and_is_reused_on_return() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));
        assert!(state.complete_tracks_tail(generation, tracks_page("full")));
        assert_eq!(state.active_tracks_load_id(), Some(generation));
        assert!(state.has_cached(Service::Deezer, Category::Tracks));

        state.select(Service::Deezer, Category::Albums);
        let (_, cached) = state.select(Service::Deezer, Category::Tracks);
        assert_eq!(
            cached.as_ref().map(|page| page.title.as_str()),
            Some("full")
        );
    }

    #[test]
    fn stale_tail_cannot_publish_or_expose_append_data() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("prefix"));
        state.select(Service::Deezer, Category::Albums);
        assert!(!state.complete_tracks_tail(generation, tracks_page("stale")));
        assert_eq!(state.active_tracks_load_id(), None);
    }

    #[test]
    fn a_new_deezer_tracks_root_blocks_the_previous_playback_tail() {
        let mut state = LibraryState::default();
        let old_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(old_generation, tracks_page("old"));
        state.select(Service::Deezer, Category::History);

        let new_generation = state.select(Service::Deezer, Category::Tracks).0;

        assert!(!state.tracks_playback_append_allowed(old_generation));
        assert!(state.tracks_playback_append_allowed(new_generation));
    }

    #[test]
    fn enrichment_completion_replaces_and_caches_the_preview() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));

        assert!(state.complete_tracks_enrichment(generation, Ok(tracks_page("rich"))));
        assert_eq!(state.page.as_ref().unwrap().title, "rich");
        assert!(state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn stale_enrichment_cannot_replace_preview() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));
        state.reload(Service::Deezer, Category::Tracks);

        assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
        assert!(state.page.is_none());
        assert!(!state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn enrichment_failure_keeps_preview() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));

        assert!(!state.complete_tracks_enrichment(generation, Err("failed".into())));
        assert_eq!(state.status, Status::Results);
        assert_eq!(state.page.as_ref().unwrap().title, "raw");
        assert!(!state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn refresh_failure_keeps_a_disk_snapshot_visible() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("disk snapshot"));

        assert!(!state.complete_tracks_refresh_failure(generation, "offline".into()));
        assert_eq!(state.status, Status::Results);
        assert_eq!(state.page.as_ref().unwrap().title, "disk snapshot");
    }

    #[test]
    fn refresh_failure_without_a_snapshot_keeps_first_load_error_behavior() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;

        assert!(state.complete_tracks_refresh_failure(generation, "offline".into()));
        assert_eq!(state.status, Status::Failed("offline".into()));
        assert!(state.page.is_none());
    }

    #[test]
    fn late_refresh_failure_cannot_touch_another_account() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let old_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(old_generation, tracks_page("old snapshot"));
        state.set_account_scope("two".into());
        let current_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(current_generation, tracks_page("new snapshot"));

        assert!(!state.complete_tracks_refresh_failure(old_generation, "late".into()));
        assert_eq!(state.status, Status::Results);
        assert_eq!(state.page.as_ref().unwrap().title, "new snapshot");
    }

    #[test]
    fn failed_enrichment_accepts_only_the_current_raw_preview() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));

        assert!(state.accept_tracks_enrichment_failure(generation));
        assert_eq!(state.page.as_ref().unwrap().title, "raw");
        let stale = generation;
        state.reload(Service::Deezer, Category::Tracks);
        assert!(!state.accept_tracks_enrichment_failure(stale));
    }

    #[test]
    fn favorite_invalidation_rejects_pending_enrichment() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));
        state.invalidate_deezer(Category::Tracks);

        assert!(!state.tracks_playback_append_allowed(generation));
        assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
        assert_eq!(state.page.as_ref().unwrap().title, "raw");
        assert!(!state.has_cached(Service::Deezer, Category::Tracks));
    }

    #[test]
    fn favorite_invalidation_rejects_a_pending_initial_load() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;

        state.invalidate_deezer(Category::Tracks);

        assert!(!state.complete_tracks_preview(generation, tracks_page("stale")));
        assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
        assert!(!state.tracks_playback_append_allowed(generation));
        assert!(state.page.is_none());
        assert_eq!(state.status, Status::Loading);
    }

    #[test]
    fn favorite_invalidation_rejects_hydration_after_the_tail() {
        let mut state = LibraryState::default();
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(generation, tracks_page("raw"));
        state.complete_tracks_tail(generation, tracks_page("full"));

        state.invalidate_deezer(Category::Tracks);

        assert!(!state.complete_tracks_enrichment(generation, Ok(tracks_page("stale"))));
        assert!(!state.tracks_playback_append_allowed(generation));
    }

    #[test]
    fn navigation_rejects_enrichment_from_the_previous_tracks_root() {
        let mut state = LibraryState::default();
        let tracks_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(tracks_generation, tracks_page("raw"));
        let history_generation = state.select(Service::Deezer, Category::History).0;
        state.complete(history_generation, Ok(tracks_page("history")));

        assert!(state.tracks_playback_append_allowed(tracks_generation));
        assert!(!state.complete_tracks_enrichment(tracks_generation, Ok(tracks_page("stale"))));
        assert_eq!(state.page.as_ref().unwrap().title, "history");
    }

    #[test]
    fn track_mutation_after_navigation_blocks_the_old_playback_tail() {
        let mut state = LibraryState::default();
        let tracks_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(tracks_generation, tracks_page("prefix"));
        state.select(Service::Deezer, Category::History);
        assert!(state.tracks_playback_append_allowed(tracks_generation));

        state.invalidate_deezer(Category::Tracks);

        assert!(!state.tracks_playback_append_allowed(tracks_generation));
        assert_eq!(state.category, Category::History);
    }

    #[test]
    fn account_change_rejects_enrichment_from_the_previous_scope() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let old_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(old_generation, tracks_page("old raw"));

        state.set_account_scope("two".into());
        let new_generation = state.select(Service::Deezer, Category::Tracks).0;
        state.complete_tracks_preview(new_generation, tracks_page("new raw"));

        assert!(!state.tracks_playback_append_allowed(old_generation));
        assert!(state.tracks_playback_append_allowed(new_generation));
        assert!(!state.complete_tracks_enrichment(old_generation, Ok(tracks_page("old rich"))));
        assert_eq!(state.page.as_ref().unwrap().title, "new raw");
    }

    #[test]
    fn progressive_preview_is_reused_after_leaving_and_returning() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let generation = state.select(Service::Deezer, Category::Tracks).0;
        let token = state.begin_tracks_pipeline();
        assert!(state.accept_tracks_preview_pipeline(token, tracks_page("preview")));

        state.select(Service::Deezer, Category::History);
        let (_, cached) = state.select(Service::Deezer, Category::Tracks);

        assert_eq!(
            cached.as_ref().map(|page| page.title.as_str()),
            Some("preview")
        );
        assert!(state.tracks_pipeline_active());
        assert_ne!(generation, state.active_generation());
    }

    #[test]
    fn stale_tracks_load_failure_restarts_once_for_the_returned_route() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        let old_generation = state.select(Service::Deezer, Category::Tracks).0;
        let old_token = state.begin_tracks_pipeline();

        state.select(Service::Deezer, Category::History);
        let returned_generation = state.select(Service::Deezer, Category::Tracks).0;
        let outcome = state.fail_tracks_pipeline_load(old_token, old_generation, "offline".into());

        let TracksPipelineLoadOutcome::Restart { generation, token } = outcome else {
            panic!("a returned Tracks route should restart the failed base request");
        };
        assert_eq!(generation, returned_generation);
        assert_ne!(token, old_token);
        assert!(state.tracks_pipeline_active());

        assert_eq!(
            state.fail_tracks_pipeline_load(token, generation, "offline".into()),
            TracksPipelineLoadOutcome::Failed
        );
        assert!(matches!(state.status, Status::Failed(_)));
    }

    #[test]
    fn raw_tail_survives_navigation_and_enrichment_can_resume() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        state.select(Service::Deezer, Category::Tracks);
        let token = state.begin_tracks_pipeline();
        state.accept_tracks_preview_pipeline(token, tracks_page("preview"));
        state.select(Service::Deezer, Category::History);

        assert!(!state.accept_tracks_tail_pipeline(token, tracks_page("raw")));
        assert!(state.has_cached(Service::Deezer, Category::Tracks));
        assert!(!state.fail_tracks_pipeline_enrichment(token));

        let (_, cached) = state.select(Service::Deezer, Category::Tracks);
        assert_eq!(cached.as_ref().map(|page| page.title.as_str()), Some("raw"));
        assert!(state.tracks_pipeline_retryable());
    }

    #[test]
    fn continuation_failure_is_retryable_after_return_without_refetching_preview() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        state.select(Service::Deezer, Category::Tracks);
        let token = state.begin_tracks_pipeline();
        assert!(state.accept_tracks_preview_pipeline(token, tracks_page("preview")));

        state.select(Service::Deezer, Category::History);
        state.select(Service::Deezer, Category::Tracks);
        assert!(state.fail_tracks_pipeline_continuation(token));
        assert!(state.tracks_pipeline_retryable());
        assert!(state.begin_tracks_pipeline_continuation_retry(token));
        assert!(!state.tracks_pipeline_retryable());
        assert_eq!(
            state.page.as_ref().map(|page| page.title.as_str()),
            Some("preview")
        );
    }

    #[test]
    fn progressive_pipeline_rejects_a_previous_account() {
        let mut state = LibraryState::default();
        state.set_account_scope("one".into());
        state.select(Service::Deezer, Category::Tracks);
        let token = state.begin_tracks_pipeline();
        state.set_account_scope("two".into());
        state.select(Service::Deezer, Category::Tracks);

        assert!(!state.accept_tracks_preview_pipeline(token, tracks_page("old")));
        assert!(state.page.is_none());
        assert_eq!(state.tracks_pipeline_token(), None);
    }
}
