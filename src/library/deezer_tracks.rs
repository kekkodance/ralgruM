use gpui::Context;

use super::{
    model::{Category, Page, Service},
    state::TracksPipelineLoadOutcome,
    tracks_cache::{DeezerTracksCacheKey, enrich_live_page},
    view::{DeezerTracksRetry, LibraryView},
};

impl LibraryView {
    pub(super) fn load_deezer_tracks_progressive(
        &mut self,
        generation: u64,
        token: u64,
        arl: crate::search::DeezerArl,
        cx: &mut Context<Self>,
    ) {
        let Ok(client) = self.client.clone() else {
            let outcome = self.state.fail_tracks_pipeline_load(
                token,
                generation,
                "Library client could not be created".into(),
            );
            if !matches!(outcome, TracksPipelineLoadOutcome::Ignored) {
                cx.notify();
            }
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let cache_arl = arl.clone();
        let task = self
            .runtime
            .spawn(async move { client.load_tracks_progressive(arl, saved_user_id).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer library request failed".into()));
            let load = match result {
                Ok(load) => load,
                Err(error) => {
                    eprintln!("Deezer library request failed: {error}");
                    let outcome = this
                        .update(cx, |this, cx| {
                            let outcome = this
                                .state
                                .fail_tracks_pipeline_load(token, generation, error);
                            if !matches!(outcome, TracksPipelineLoadOutcome::Ignored) {
                                cx.notify();
                            }
                            outcome
                        })
                        .ok();
                    if let Some(TracksPipelineLoadOutcome::Restart { generation, token }) = outcome
                    {
                        this.update(cx, |this, cx| {
                            this.load_deezer_tracks_progressive(
                                generation,
                                token,
                                cache_arl.clone(),
                                cx,
                            );
                        })
                        .ok();
                    }
                    return;
                }
            };

            let cache_and_key = this
                .update(cx, |this, _| {
                    (
                        this.tracks_cache.clone(),
                        this.tracks_cache.key(&cache_arl, Some(&load.user_id)),
                        this.runtime.clone(),
                    )
                })
                .ok();
            let verified_key = cache_and_key.as_ref().and_then(|(_, key, _)| key.clone());
            let cache_task = cache_and_key.and_then(|(cache, key, runtime)| {
                key.map(|key| runtime.spawn(async move { cache.load(&key).await }))
            });
            let initial_page = load.page;
            let continuation = load.continuation;
            let visible = this
                .update(cx, |this, cx| {
                    let visible = if continuation.is_some() {
                        this.state
                            .accept_tracks_preview_pipeline(token, initial_page.clone())
                    } else {
                        this.state
                            .accept_tracks_pipeline_enrichment(token, initial_page.clone())
                    };
                    if visible {
                        this.seed_loaded_root_favorites(Service::Deezer, Category::Tracks, cx);
                        cx.notify();
                    }
                    visible
                })
                .unwrap_or(false);
            if continuation.is_none() {
                if visible || pipeline_is_current(&this, cx, token) {
                    persist_snapshot(this, cx, verified_key, initial_page);
                }
                return;
            }

            let Some(continuation) = continuation else {
                return;
            };
            let retry_continuation = continuation.clone();
            this.update(cx, |this, _| {
                this.deezer_tracks_retry = Some(DeezerTracksRetry::Continuation {
                    token,
                    continuation: retry_continuation,
                });
            })
            .ok();
            run_deezer_tracks_continuation(
                this,
                cx,
                token,
                generation,
                continuation,
                cache_task,
                verified_key,
            )
            .await;
        })
        .detach();
    }

    pub(super) fn retry_deezer_tracks_pending(&mut self, cx: &mut Context<Self>) {
        if !self.state.tracks_pipeline_retryable() {
            return;
        }
        let Some(retry) = self.deezer_tracks_retry.take() else {
            return;
        };
        match retry {
            DeezerTracksRetry::Continuation {
                token,
                continuation,
            } => {
                if !self.state.begin_tracks_pipeline_continuation_retry(token) {
                    self.deezer_tracks_retry = Some(DeezerTracksRetry::Continuation {
                        token,
                        continuation,
                    });
                    return;
                }
                let generation = self.state.active_generation();
                let continuation_for_retry = continuation.clone();
                self.deezer_tracks_retry = Some(DeezerTracksRetry::Continuation {
                    token,
                    continuation: continuation_for_retry,
                });
                cx.spawn(async move |this, cx| {
                    run_deezer_tracks_continuation(
                        this,
                        cx,
                        token,
                        generation,
                        continuation,
                        None,
                        None,
                    )
                    .await;
                })
                .detach();
            }
            DeezerTracksRetry::Hydration {
                token,
                hydration,
                key,
            } => {
                if !self.state.begin_tracks_pipeline_enrichment_retry(token) {
                    self.deezer_tracks_retry = Some(DeezerTracksRetry::Hydration {
                        token,
                        hydration,
                        key,
                    });
                    return;
                }
                let retry_hydration = hydration.clone();
                self.deezer_tracks_retry = Some(DeezerTracksRetry::Hydration {
                    token,
                    hydration: retry_hydration,
                    key: key.clone(),
                });
                let task = self.runtime.spawn(async move { hydration.hydrate().await });
                cx.spawn(async move |this, cx| {
                    let result = task
                        .await
                        .unwrap_or_else(|_| Err("Deezer track enrichment failed".into()));
                    match result {
                        Ok(page) => {
                            let persisted_page = page.clone();
                            let accepted = this
                                .update(cx, |this, cx| {
                                    if !pipeline_is_current_in_entity(this, token) {
                                        return false;
                                    }
                                    this.deezer_tracks_retry = None;
                                    let accepted =
                                        this.state.accept_tracks_pipeline_enrichment(token, page);
                                    if accepted {
                                        this.seed_loaded_root_favorites(
                                            Service::Deezer,
                                            Category::Tracks,
                                            cx,
                                        );
                                        cx.notify();
                                    }
                                    accepted || pipeline_is_current_in_entity(this, token)
                                })
                                .unwrap_or(false);
                            if accepted {
                                persist_snapshot(this, cx, key, persisted_page);
                            }
                        }
                        Err(error) => {
                            eprintln!("Deezer track enrichment retry failed: {error}");
                            this.update(cx, |this, cx| {
                                if this.state.fail_tracks_pipeline_enrichment(token) {
                                    cx.notify();
                                }
                            })
                            .ok();
                        }
                    }
                })
                .detach();
            }
        }
    }
}

async fn run_deezer_tracks_continuation(
    this: gpui::WeakEntity<LibraryView>,
    cx: &mut gpui::AsyncApp,
    token: u64,
    generation: u64,
    continuation: super::client::DeezerTracksContinuation,
    cache_task: Option<tokio::task::JoinHandle<Option<Page>>>,
    verified_key: Option<DeezerTracksCacheKey>,
) {
    let continuation_task = this
        .update(cx, |this, _| {
            this.runtime
                .spawn(async move { continuation.complete().await })
        })
        .ok();
    let Some(continuation_task) = continuation_task else {
        return;
    };
    let result = continuation_task
        .await
        .unwrap_or_else(|_| Err("Deezer Tracks continuation failed".into()));
    let completion = match result {
        Ok(completion) => completion,
        Err(error) => {
            eprintln!("Deezer Tracks continuation failed: {error}");
            this.update(cx, |this, cx| {
                if this.state.fail_tracks_pipeline_continuation(token) {
                    cx.notify();
                }
            })
            .ok();
            return;
        }
    };

    let append_ticket = this
        .update(cx, |this, cx| {
            this.playback
                .read(cx)
                .deezer_library_append_ticket(generation)
        })
        .ok()
        .flatten();
    let exact_queue = completion
        .page
        .tracks
        .iter()
        .map(|track| {
            crate::playback::PlaybackTrack::from_library(track, crate::search::Provider::Deezer)
        })
        .collect::<Vec<_>>();

    let cached_page = if let Some(cache_task) = cache_task {
        cache_task.await.ok().flatten()
    } else {
        None
    };
    let raw_page = completion.page;
    let preview_page = enrich_live_page(cached_page.as_ref(), &raw_page);
    let fallback_page = preview_page.clone();
    let hydration = completion.hydration;
    let visible = this
        .update(cx, |this, cx| {
            let visible = this.state.accept_tracks_tail_pipeline(token, preview_page);
            if visible {
                this.seed_loaded_root_favorites(Service::Deezer, Category::Tracks, cx);
                cx.notify();
            }
            visible
        })
        .unwrap_or(false);
    let append_allowed = this
        .update(cx, |this, _| {
            this.state.tracks_playback_append_allowed(generation)
        })
        .unwrap_or(false);
    if append_allowed && let Some(ticket) = append_ticket {
        this.update(cx, |this, cx| {
            this.playback.update(cx, |playback, cx| {
                playback.append_deezer_library_exact_tail(&ticket, exact_queue, cx);
            });
        })
        .ok();
    }

    persist_snapshot(
        this.clone(),
        cx,
        verified_key.clone(),
        fallback_page.clone(),
    );
    let Some(hydration) = hydration else {
        let became_visible = this
            .update(cx, |this, cx| {
                this.deezer_tracks_retry = None;
                let became_visible = this
                    .state
                    .accept_tracks_pipeline_enrichment(token, fallback_page.clone());
                if became_visible {
                    this.seed_loaded_root_favorites(Service::Deezer, Category::Tracks, cx);
                    cx.notify();
                }
                became_visible
            })
            .unwrap_or(false);
        if became_visible || visible || pipeline_is_current(&this, cx, token) {
            persist_snapshot(this, cx, verified_key, fallback_page);
        }
        return;
    };

    let retry_hydration = hydration.clone();
    this.update(cx, |this, _| {
        this.deezer_tracks_retry = Some(DeezerTracksRetry::Hydration {
            token,
            hydration: retry_hydration,
            key: verified_key.clone(),
        });
    })
    .ok();
    let hydration_task = this
        .update(cx, |this, _| {
            this.runtime.spawn(async move { hydration.hydrate().await })
        })
        .ok();
    let Some(hydration_task) = hydration_task else {
        return;
    };
    finish_deezer_tracks_hydration(this, cx, token, hydration_task, verified_key, fallback_page)
        .await;
}

async fn finish_deezer_tracks_hydration(
    this: gpui::WeakEntity<LibraryView>,
    cx: &mut gpui::AsyncApp,
    token: u64,
    task: tokio::task::JoinHandle<Result<Page, String>>,
    key: Option<DeezerTracksCacheKey>,
    fallback_page: Page,
) {
    let result = task
        .await
        .unwrap_or_else(|_| Err("Deezer track enrichment failed".into()));
    match result {
        Ok(page) => {
            let accepted = this
                .update(cx, |this, cx| {
                    if !pipeline_is_current_in_entity(this, token) {
                        return false;
                    }
                    this.deezer_tracks_retry = None;
                    let accepted = this
                        .state
                        .accept_tracks_pipeline_enrichment(token, page.clone());
                    if accepted {
                        this.seed_loaded_root_favorites(Service::Deezer, Category::Tracks, cx);
                        cx.notify();
                    }
                    accepted || pipeline_is_current_in_entity(this, token)
                })
                .unwrap_or(false);
            if accepted {
                persist_snapshot(this, cx, key, page);
            }
        }
        Err(error) => {
            eprintln!("Deezer track enrichment failed: {error}");
            let accepted = this
                .update(cx, |this, cx| {
                    let accepted = this.state.fail_tracks_pipeline_enrichment(token);
                    if accepted {
                        cx.notify();
                    }
                    accepted || pipeline_is_current_in_entity(this, token)
                })
                .unwrap_or(false);
            if accepted {
                persist_snapshot(this, cx, key, fallback_page);
            }
        }
    }
}

fn pipeline_is_current(
    this: &gpui::WeakEntity<LibraryView>,
    cx: &mut gpui::AsyncApp,
    token: u64,
) -> bool {
    this.update(cx, |this, _| pipeline_is_current_in_entity(this, token))
        .unwrap_or(false)
}

fn pipeline_is_current_in_entity(this: &LibraryView, token: u64) -> bool {
    this.state.tracks_pipeline_token() == Some(token)
}

fn persist_snapshot(
    this: gpui::WeakEntity<LibraryView>,
    cx: &mut gpui::AsyncApp,
    key: Option<DeezerTracksCacheKey>,
    page: Page,
) {
    let cache_and_runtime = this
        .update(cx, |this, _| {
            (this.tracks_cache.clone(), this.runtime.clone())
        })
        .ok();
    let (Some(key), Some((cache, runtime))) = (key, cache_and_runtime) else {
        return;
    };
    let revision = cache.reserve_revision(&key);
    drop(runtime.spawn(async move {
        if let Err(error) = cache.store_revision(&key, &page, revision).await {
            eprintln!("{error}");
        }
    }));
}
