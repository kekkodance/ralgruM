use std::time::Duration;

use gpui::{Context, ScrollHandle, Window, point, px};

use super::{
    card_actions::detail_favorite_seeds,
    credential::DeezerArl,
    detail::{DetailState, should_apply_scheduled_detail_scroll_reset, should_reset_detail_scroll},
    models::{Card, Provider, ResultState, ResultType},
    view::SearchView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SearchNavigationBackTarget {
    Detail,
    DiscoverChannel,
}

pub(super) fn search_navigation_back_target(
    detail_open: bool,
    discover_channel_open: bool,
) -> SearchNavigationBackTarget {
    if detail_open || !discover_channel_open {
        SearchNavigationBackTarget::Detail
    } else {
        SearchNavigationBackTarget::DiscoverChannel
    }
}

pub(super) fn should_close_discover_channel_before_detail(
    preserve_discover_channel: bool,
    discover_channel_open: bool,
) -> bool {
    discover_channel_open && !preserve_discover_channel
}

pub(super) fn search_results_root_visible(
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

impl SearchView {
    pub(super) fn reset_detail_scroll(&mut self) {
        let top = point(px(0.), px(0.));
        self.browser_scroll.reset();
        self.scroll.set_offset(top);
        self.scroll = ScrollHandle::new();
        self.scroll.set_offset(top);
    }

    pub(super) fn schedule_forward_detail_scroll_reset(
        &mut self,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
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

    pub(crate) fn detail_open(&self) -> bool {
        !matches!(self.detail.state, DetailState::Closed)
    }

    pub(crate) fn search_results_root_open(&self) -> bool {
        search_results_root_visible(
            &self.search_query,
            &self.state.state,
            self.detail_open(),
            self.discover_channel_open(),
        )
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
        self.cancel_artist_page_ai_request();
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
        self.cancel_artist_page_ai_request();
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
        self.cancel_artist_page_ai_request();
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

    pub(super) fn complete_detail(
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
            self.start_artist_page_ai_enrichment(cx);
            cx.notify();
        }
    }

    pub(super) fn run_detail(
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
                            ai_generated: false,
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

    pub(crate) fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.cancel_artist_page_ai_request();
        self.pending_forward_detail_scroll_reset = None;
        let offset = self
            .detail
            .back_with_scroll(self.active_vertical_scroll_offset());
        self.restore_detail_scroll(offset);
        self.start_artist_page_ai_enrichment(cx);
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
        self.start_artist_page_ai_enrichment(cx);
        cx.notify();
    }
}
