use gpui::{App, Context, Focusable, Window, point, px};
use tokio::task::AbortHandle;

use crate::library::{FavoriteKey, FavoriteKind};

use super::{
    credential::DeezerArl,
    models::{Provider, ResultState, ResultType, SearchJob, Source},
    results_view::search_results_content_identity,
    suggestions::{SUGGESTION_DEBOUNCE, SuggestionRow},
    view::SearchView,
};

pub(super) struct ActiveRequest {
    pub(super) generation: u64,
    pub(super) id: u64,
    pub(super) abort: AbortHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchRequestKey {
    source: Source,
    result_type: ResultType,
    query: String,
}

pub(super) struct ActiveSearchRequest {
    generation: u64,
    id: u64,
    aborts: Vec<AbortHandle>,
    pending_batches: usize,
    key: SearchRequestKey,
}

pub(super) fn all_search_missing_accounts(
    source: Source,
    soundcloud_available: bool,
) -> Vec<Provider> {
    // Deezer search runs anonymously. Only SoundCloud account state can
    // leave an All search missing one of its providers.
    if source != Source::All || soundcloud_available {
        return Vec::new();
    }
    vec![Provider::SoundCloud]
}

impl SearchView {
    pub(crate) fn set_search_active(&mut self, active: bool) {
        self.search_active = active;
        if !active {
            self.cancel_suggestion_request();
            self.suggestions.set_focused(false);
        }
    }

    pub(super) fn cancel_search_request(&mut self) {
        if let Some(request) = self.search_request.take() {
            for abort in request.aborts {
                abort.abort();
            }
        }
        self.cancel_deezer_ai_enrichment_request();
    }

    fn cancel_deezer_ai_enrichment_request(&mut self) {
        if let Some(request) = self.deezer_ai_enrichment_request.take() {
            request.abort.abort();
        }
    }

    fn clear_deezer_ai_enrichment_request(&mut self, generation: u64, id: u64) {
        if self
            .deezer_ai_enrichment_request
            .as_ref()
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.deezer_ai_enrichment_request = None;
        }
    }

    pub(super) fn next_request_id(&mut self) -> u64 {
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

    pub(super) fn cancel_suggestion_request(&mut self) {
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

    pub(super) fn cancel_discover_provider_request(&mut self, provider: Provider) {
        if let Some(request) = self.discover_requests.remove(&provider) {
            request.abort.abort();
        }
        self.discover.cancel_loading(provider);
    }

    pub(super) fn clear_discover_provider_request(
        &mut self,
        provider: Provider,
        generation: u64,
        id: u64,
    ) {
        if self
            .discover_requests
            .get(&provider)
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.discover_requests.remove(&provider);
        }
    }

    pub(super) fn cancel_discover_requests(&mut self) {
        for request in self.discover_requests.drain().map(|(_, request)| request) {
            request.abort.abort();
        }
        self.discover.cancel_loading(Provider::Deezer);
        self.discover.cancel_loading(Provider::SoundCloud);
        self.cancel_smart_mix_enrichment_request();
    }

    pub(super) fn cancel_discover_channel_request(&mut self) {
        if let Some(request) = self.discover_channel_request.take() {
            request.abort.abort();
        }
    }

    pub(super) fn clear_discover_channel_request(&mut self, generation: u64, id: u64) {
        if self
            .discover_channel_request
            .as_ref()
            .is_some_and(|request| request.generation == generation && request.id == id)
        {
            self.discover_channel_request = None;
        }
    }

    pub(super) fn cancel_smart_mix_enrichment_request(&mut self) {
        if let Some(request) = self.smart_mix_enrichment_request.take() {
            request.abort.abort();
            self.smart_mix_enrichment_was_cancelled = true;
        }
    }

    pub(super) fn clear_smart_mix_enrichment_request(&mut self, generation: u64, id: u64) {
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

    pub(super) fn selected_suggestion_query(&self, cx: &Context<Self>) -> Option<String> {
        let query = self.input.read(cx).value();
        self.suggestions.selected_query(query.as_ref())
    }

    pub(super) fn replace_query(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_suggestion_request();
        self.input
            .update(cx, |input, cx| input.set_value(query, window, cx));
        self.suggestions.invalidate_request();
    }

    fn persist_search_history(&mut self, cx: &mut Context<Self>) {
        let history = self.suggestions.history().to_vec();
        self.settings.update(cx, |settings, cx| {
            settings.persist_search_history(history, cx);
        });
    }

    pub(super) fn refresh_suggestions(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn open_account_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.library
            .update(cx, |library, cx| library.open_settings(window, cx));
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

    pub(super) fn search_credentials(
        &self,
        cx: &Context<Self>,
    ) -> (
        Option<DeezerArl>,
        Option<super::credential::SoundCloudToken>,
    ) {
        let account = self.account.read(cx);
        (account.deezer_arl(), account.soundcloud_token())
    }

    fn run(
        &mut self,
        job: Option<SearchJob>,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<super::credential::SoundCloudToken>,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let Some(job) = job else {
            self.start_deezer_ai_enrichment(deezer_arl, cx);
            return;
        };
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
            let deezer_ai_arl = deezer_arl.clone();
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
                        if final_batch {
                            this.start_deezer_ai_enrichment(deezer_ai_arl, cx);
                        }
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn start_deezer_ai_enrichment(
        &mut self,
        deezer_arl: Option<DeezerArl>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_deezer_ai_enrichment_request();
        let Some(arl) = deezer_arl else { return };
        let Ok(client) = self.client.clone() else {
            return;
        };
        let album_ids = self
            .state
            .groups
            .tracks
            .iter()
            .filter(|track| track.source == Provider::Deezer)
            .map(|track| track.album_id.clone())
            .chain(
                self.state
                    .groups
                    .albums
                    .iter()
                    .filter(|card| card.source == Provider::Deezer)
                    .map(|card| card.id.clone()),
            )
            .collect::<Vec<_>>();
        if album_ids.is_empty() {
            return;
        }
        let generation = self.state.generation();
        let account_scope = self.account_scope.clone();
        let task = self
            .runtime
            .spawn(async move { client.deezer_ai_content(album_ids, arl).await });
        let request_id = self.next_request_id();
        self.deezer_ai_enrichment_request = Some(ActiveRequest {
            generation,
            id: request_id,
            abort: task.abort_handle(),
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.clear_deezer_ai_enrichment_request(generation, request_id);
                let Ok(Ok(albums)) = result else {
                    return;
                };
                if this.account_scope == account_scope {
                    let search_changed = this.state.apply_deezer_ai_content(generation, &albums);
                    this.playback.update(cx, |playback, cx| {
                        if playback.state.apply_deezer_ai_content(&albums) {
                            cx.notify();
                        }
                    });
                    if search_changed {
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}
