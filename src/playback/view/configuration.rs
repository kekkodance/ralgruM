use super::*;

impl PlaybackModel {
    pub(crate) fn replace_queue(
        &mut self,
        queue: Vec<PlaybackTrack>,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        self.finish_deezer_listen(cx);
        self.consecutive_failures = 0;
        self.cancel_seek_slider_interaction();
        self.reset_extension_state();
        if let Some(generation) = self.state.replace(queue, index) {
            self.start(generation, cx);
        } else {
            self.reset_download_progress();
            self.current_audio_info = None;
            self.resolved_quality = None;
            self.sync_discord();
        }
    }

    pub(crate) fn set_discord_presence(&mut self, enabled: bool) {
        self.discord.set_enabled(enabled, &self.state);
    }

    pub(crate) fn set_skip_explicit(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.state.set_skip_explicit(enabled) {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn set_cache_limit(&mut self, limit_mb: u64) {
        let cache = self.cache.clone();
        let task = self.runtime.spawn(async move {
            let _ = cache.set_limit(limit_mb).await;
        });
        drop(task);
    }

    pub(crate) fn set_background_audio_cache(&mut self, enabled: bool) {
        self.background_audio_cache = enabled;
        if !enabled {
            self.cache.cancel();
        }
    }

    pub(crate) fn set_seamless_playback(&mut self, enabled: bool) {
        if self.seamless_playback == enabled {
            return;
        }
        self.seamless_playback = enabled;
        if !enabled && matches!(self.standby, StandbyPhase::Pending) {
            // Armed audio is already queued and is still handed off cleanly;
            // only stop waiting on an in-flight preparation.
            self.standby = StandbyPhase::Idle;
        }
    }

    pub(crate) fn set_provider_play_reporting(&mut self, deezer: bool) {
        self.record_deezer_plays = deezer;
        if !deezer {
            self.deezer_listen = None;
        }
    }

    pub(crate) fn set_context(&mut self, context: PlaybackContext, cx: &mut Context<Self>) {
        if self.state.replace_context(context) {
            self.reset_extension_state();
            cx.notify();
        }
    }

    pub(crate) fn context(&self) -> &PlaybackContext {
        &self.state.context
    }

    pub(crate) fn deezer_library_append_ticket(
        &self,
        load_id: u64,
    ) -> Option<ExactQueueAppendTicket> {
        self.state.deezer_library_append_ticket(load_id)
    }

    pub(crate) fn append_deezer_library_exact_tail(
        &mut self,
        ticket: &ExactQueueAppendTicket,
        exact_queue: Vec<PlaybackTrack>,
        cx: &mut Context<Self>,
    ) -> bool {
        match self
            .state
            .append_deezer_library_exact_tail(ticket, exact_queue)
        {
            ExactQueueAppend::Stale => false,
            ExactQueueAppend::Applied {
                added,
                resume_index,
            } => {
                if let Some(index) = resume_index {
                    self.select(index, cx);
                } else if added > 0 {
                    cx.notify();
                }
                true
            }
        }
    }

    pub(crate) fn apply_flow_page(
        &mut self,
        config_id: String,
        mode: crate::library::FlowMode,
        tuner: Option<crate::library::FlowTuner>,
        kind: DeezerFlowKind,
        tracks: Vec<PlaybackTrack>,
        clear_remaining: bool,
        keep_playing_track: bool,
        cx: &mut Context<Self>,
    ) {
        // Navigating to a mix page that is already playing must not cut the
        // audio. An explicit play command restarts the mix from its first
        // track even when that mix is already the active context.
        let preserve_current = self.state.should_preserve_flow_current(
            &config_id,
            clear_remaining,
            keep_playing_track,
        );
        let context = PlaybackContext::DeezerFlow {
            config_id,
            mode,
            tuner,
            kind,
        };
        if preserve_current {
            self.state.replace_remaining_tracks(tracks);
            self.state.replace_context(context);
            self.reset_extension_state();
            cx.notify();
        } else {
            self.replace_queue(tracks, 0, cx);
            self.set_context(context, cx);
        }
    }

    pub(crate) fn extension_in_flight(&self) -> bool {
        self.extension_in_flight
    }

    pub(crate) fn resolved_quality(&self) -> Option<&str> {
        self.resolved_quality.as_deref()
    }

    pub(crate) fn seek_preview_fraction(&self) -> Option<f32> {
        self.seek_slider_interaction.preview_fraction
    }

    pub(crate) fn seek_commit_epoch(&self) -> u64 {
        self.seek_commit_epoch
    }

    pub(crate) fn set_seek_slider_enabled(&mut self, enabled: bool) {
        self.seek_slider_interaction.set_control_enabled(enabled);
    }

    pub(crate) fn begin_new_seek_pointer_interaction(&mut self) {
        self.seek_slider_interaction
            .begin_pointer_interaction(self.state.generation);
    }

    pub(crate) fn preview_seek_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let fraction = fraction.clamp(0., 1.);
        if self
            .seek_slider_interaction
            .preview(self.state.generation, fraction)
        {
            cx.notify();
        }
    }

    pub(crate) fn commit_seek_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let fraction = fraction.clamp(0., 1.);
        if let Some(fraction) = self
            .seek_slider_interaction
            .commit(self.state.generation, fraction)
        {
            self.seek_fraction(fraction, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn cancel_seek_slider_interaction(&mut self) {
        self.seek_slider_interaction.cancel();
    }

    pub(crate) fn current_track_info(&self, track: &PlaybackTrack) -> Option<ResolvedTrackInfo> {
        let (provider, id, info) = self.current_audio_info.as_ref()?;
        (*provider == track.provider && id == &track.id).then_some(*info)
    }

    pub(crate) fn track_info_credential_generation(&self, cx: &gpui::App) -> u128 {
        self.account.read(cx).credential_generation()
    }

    pub(crate) fn track_info_cache_revision(&self) -> u64 {
        self.cache.revision()
    }

    pub(crate) fn begin_extension_ticket(&mut self) -> Option<QueueExtensionTicket> {
        if self.extension_in_flight || self.extension_exhausted {
            return None;
        }
        let ticket = self.state.extension_ticket()?;
        self.extension_in_flight = true;
        self.duplicate_extension_retries = 0;
        Some(ticket)
    }

    pub(crate) fn finish_extension(&mut self, added: usize, cx: &mut Context<Self>) {
        self.extension_in_flight = false;
        self.duplicate_extension_retries = 0;
        if added == 0 {
            self.extension_exhausted = true;
        }
        cx.notify();
    }

    pub(crate) fn finish_extension_for_ticket(
        &mut self,
        ticket: &QueueExtensionTicket,
        added: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.state.extension_ticket_is_current(ticket) {
            return false;
        }
        self.finish_extension(added, cx);
        true
    }

    /// Apply one radio response and return a fresh continuation ticket when
    /// it contains only duplicates. The state method performs the ticket
    /// check and queue mutation as one operation.
    pub(crate) fn apply_extension_batch(
        &mut self,
        ticket: &QueueExtensionTicket,
        additions: Vec<PlaybackTrack>,
        next_flow_tuner: Option<crate::library::FlowTuner>,
        continuation_seed: Option<String>,
        batch_nonempty: bool,
        cx: &mut Context<Self>,
    ) -> Result<Option<QueueExtensionTicket>, ()> {
        let has_next_tuner = next_flow_tuner.is_some();
        match self
            .state
            .apply_extension(ticket, additions, next_flow_tuner, continuation_seed)
        {
            ExtensionApply::Stale => Err(()),
            ExtensionApply::Applied { added } => {
                let resume_after_extension =
                    added > 0 && self.state.status == PlaybackStatus::Ended;
                if deezer_extension::should_retry_duplicate_batch(
                    batch_nonempty,
                    added,
                    has_next_tuner,
                    self.duplicate_extension_retries,
                ) {
                    self.duplicate_extension_retries += 1;
                    let retry = self.state.extension_ticket();
                    cx.notify();
                    Ok(retry)
                } else {
                    self.extension_in_flight = false;
                    self.duplicate_extension_retries = 0;
                    self.extension_exhausted = added == 0;
                    if resume_after_extension {
                        self.next(cx);
                    }
                    cx.notify();
                    Ok(None)
                }
            }
        }
    }

    pub(crate) fn extension_observer_key(&self) -> ExtensionObserverKey {
        deezer_extension::observer_key(&self.state)
    }

    pub(super) fn reset_extension_state(&mut self) {
        self.extension_in_flight = false;
        self.extension_exhausted = false;
        self.duplicate_extension_retries = 0;
    }
}
