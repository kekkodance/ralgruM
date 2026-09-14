use gpui::{Context, Window};

use crate::library::{FavoriteKey, FavoriteKind};

use super::{
    album_info_cache::{AlbumInfoCacheKey, info_for_detail_state},
    detail::DetailState,
    models::{Card, Provider, ResultType},
    view::SearchView,
};

pub(super) struct FavoriteAction {
    expected: bool,
}

pub(super) fn detail_favorite_seeds(
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

impl SearchView {
    pub(super) fn sync_favorite_actions(&mut self, cx: &mut Context<Self>) {
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
}
