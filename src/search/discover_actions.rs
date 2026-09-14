use gpui::{Context, point, px};

use super::{
    credential::DeezerArl,
    discover,
    models::{Card, Provider, ResultState, ResultType},
    search_state::ActiveRequest,
    view::SearchView,
};

pub(super) fn discover_provider_has_credentials(
    provider: Provider,
    deezer_available: bool,
    soundcloud_available: bool,
) -> bool {
    match provider {
        Provider::Deezer => deezer_available,
        Provider::SoundCloud => soundcloud_available,
    }
}

impl SearchView {
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
