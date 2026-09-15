use super::*;

impl LibraryView {
    pub(super) fn invalidate_playback_actions(&mut self) -> u64 {
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
        if self.toggle_live_deezer_mix(
            &PlaybackContext::DeezerTrackMix {
                seed_track_id: track_id.clone(),
            },
            cx,
        ) {
            return;
        }
        self.start_deezer_mix(DeezerMixKind::Track(track_id), cx);
    }

    pub(crate) fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        if self.toggle_live_deezer_mix(
            &PlaybackContext::DeezerArtistMix {
                seed_artist_id: artist_id.clone(),
            },
            cx,
        ) {
            return;
        }
        self.start_deezer_mix(DeezerMixKind::Artist(artist_id), cx);
    }

    /// Deezer behavior: the play command on the mix that is already the
    /// active playback context toggles pause and resume instead of
    /// reloading it.
    fn toggle_live_deezer_mix(
        &mut self,
        context: &PlaybackContext,
        cx: &mut Context<Self>,
    ) -> bool {
        let playback = self.playback.read(cx);
        let same_mix_live = playback.context() == context
            && matches!(
                playback.state.status,
                crate::playback::PlaybackStatus::Playing | crate::playback::PlaybackStatus::Paused
            );
        if same_mix_live {
            self.playback.update(cx, |playback, cx| playback.toggle(cx));
        }
        same_mix_live
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
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        let id = id.trim().to_owned();
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return;
        }
        let key = crate::library::DeezerFeedbackKey {
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

    pub(crate) fn deezer_client(&self) -> Result<crate::library::client::LibraryClient, String> {
        self.client.clone()
    }

    pub(crate) fn soundcloud_library_client(
        &self,
    ) -> Result<crate::library::soundcloud_client::SoundCloudLibraryClient, String> {
        self.soundcloud_client.clone()
    }

    fn start_deezer_mix(&mut self, kind: DeezerMixKind, cx: &mut Context<Self>) {
        let action_generation = self.invalidate_playback_actions();
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
        // Feedback first: stop the current audio and open a blank player
        // bar while the mix loads. The queue epoch is captured after this
        // because entering the pending state advances it.
        self.playback
            .update(cx, |playback, cx| playback.begin_pending_load(cx));
        let queue_epoch = self.playback.read(cx).state.queue_epoch();
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
                    playback.update(cx, |playback, cx| {
                        playback.abandon_pending_load_if_epoch(queue_epoch, cx)
                    });
                    return;
                }
                let Ok(batch) = result else {
                    playback.update(cx, |playback, cx| {
                        playback.abandon_pending_load_if_epoch(queue_epoch, cx)
                    });
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not start Deezer mix",
                        Some("Deezer did not return a playable mix.".into()),
                    );
                    return;
                };
                if batch.tracks.is_empty() {
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
                let queue = batch
                    .tracks
                    .iter()
                    .map(|track| PlaybackTrack::from_library(track, Provider::Deezer))
                    .collect::<Vec<_>>();
                playback.update(cx, |playback, cx| {
                    if playback.state.queue_epoch() != queue_epoch {
                        return;
                    }
                    playback.replace_queue(queue, 0, cx);
                    playback.set_context(context.clone(), cx);
                });
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
                let result = playback.update(cx, |playback, cx| {
                    playback.apply_extension_batch(
                        &ticket,
                        additions,
                        batch.next_flow_tuner,
                        batch.continuation_seed,
                        batch_nonempty,
                        cx,
                    )
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
                    let appended = this
                        .state
                        .sync_active_deezer_smart_mix_tracks(config_id, &batch.tracks);
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
}
