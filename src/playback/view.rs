use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{AppContext, Context, Entity, Task, Window};
use gpui_component::slider::{SliderEvent, SliderState, SliderValue};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::{
    discord::DiscordPresence,
    downloads::format_bitrate,
    media_control::{MediaRequest, MediaSession},
    settings::AccountState,
};

use super::state::{ExactQueueAppend, ExactQueueAppendTicket};
use super::{
    AudioCache, DeezerFlowKind, ExtensionApply, PlaybackContext, PlaybackProvider, PlaybackState,
    PlaybackStatus, PlaybackTrack, PreviousAction, QueueExtensionTicket, ResolvedTrackInfo,
    RightSidebar,
    deezer_extension::{self, ExtensionObserverKey},
    engine::{AudioEngine, RodioEngine, SeekCompletion, SeekOutcome},
    fade::{
        USER_FADE_FRAME, USER_FADE_SETTLE_TIMEOUT, UserFadeSupervisor, UserToggleFadeDecision,
        user_toggle_fade_decision,
    },
    listen_history::{
        DeezerListenSession, ListenHistorySignal, SoundCloudListenReport, deezer_next_media,
    },
    resolver::{ProgressCallback, ProgressUpdate, StreamResolver},
    standby::{self, ArmedStandby, PreparedSource, SinkProbe, StandbyPhase, WatchTick},
};

pub(crate) const SEEK_SLIDER_STEP: f32 = 0.01;
pub(crate) const MAX_CONSECUTIVE_AUTO_SKIPS: usize = 5;

pub(crate) struct PlaybackModel {
    pub(crate) state: PlaybackState,
    account: Entity<AccountState>,
    runtime: Arc<Runtime>,
    resolver: Result<StreamResolver, String>,
    engine: Result<RodioEngine, String>,
    user_fade_supervisor: UserFadeSupervisor,
    user_fade_task: Option<Task<()>>,
    cancellation: CancellationToken,
    pub(crate) seek_slider: Entity<SliderState>,
    pub(crate) volume_slider: Entity<SliderState>,
    seek_slider_interaction: SeekSliderInteraction,
    seek_commit_epoch: u64,
    discord: DiscordPresence,
    media: MediaSession,
    cache: AudioCache,
    background_audio_cache: bool,
    seamless_playback: bool,
    record_deezer_plays: bool,
    deezer_listen: Option<DeezerListenSession>,
    standby: StandbyPhase,
    listen_history_changed: Arc<ListenHistorySignal>,
    extension_in_flight: bool,
    extension_exhausted: bool,
    duplicate_extension_retries: usize,
    pub(crate) consecutive_failures: usize,
    resolved_quality: Option<String>,
    current_audio_info: Option<(PlaybackProvider, String, ResolvedTrackInfo)>,
    loading_from_cache: bool,
    download_progress: Arc<Mutex<DownloadProgress>>,
}

#[derive(Clone, Copy, Debug)]
struct DownloadProgress {
    generation: u64,
    downloaded: u64,
    total: Option<u64>,
    buffered_fraction: Option<f32>,
    fully_buffered: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SeekSliderInteraction {
    generation: Option<u64>,
    preview_fraction: Option<f32>,
    cancelled: bool,
    pointer_change: Option<SliderValue>,
    control_disabled: bool,
}

impl SeekSliderInteraction {
    fn preview(&mut self, generation: u64, fraction: f32) -> bool {
        if self.cancelled || self.control_disabled {
            return false;
        }
        if self.generation.is_some_and(|active| active != generation) {
            self.cancel();
            return false;
        }
        self.generation.get_or_insert(generation);
        self.preview_fraction = Some(fraction);
        true
    }

    fn commit(&mut self, generation: u64, fraction: f32) -> Option<f32> {
        let accepted = !self.cancelled && self.generation == Some(generation);
        self.clear_interaction();
        accepted.then_some(fraction)
    }

    fn record_pointer_change(&mut self, value: SliderValue) {
        self.pointer_change = Some(value);
    }

    fn suppress_pointer_change(&mut self, value: SliderValue) -> bool {
        let matches = self.pointer_change == Some(value);
        self.pointer_change = None;
        matches
    }

    fn cancel(&mut self) {
        self.preview_fraction = None;
        if self.generation.is_some() {
            self.cancelled = true;
        }
    }

    fn begin_new_pointer_interaction(&mut self) {
        if !self.control_disabled {
            self.clear_interaction();
            self.pointer_change = None;
        }
    }

    fn begin_pointer_interaction(&mut self, generation: u64) {
        self.begin_new_pointer_interaction();
        if !self.control_disabled {
            self.generation = Some(generation);
        }
    }

    fn set_control_enabled(&mut self, enabled: bool) {
        if enabled {
            if self.control_disabled {
                self.control_disabled = false;
            }
        } else {
            if self.generation.is_some() || self.preview_fraction.is_some() {
                self.preview_fraction = None;
                self.cancelled = true;
            } else {
                self.clear_interaction();
            }
            self.control_disabled = true;
        }
    }

    fn clear_interaction(&mut self) {
        self.generation = None;
        self.preview_fraction = None;
        self.cancelled = false;
    }
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self::for_generation(0)
    }
}

impl DownloadProgress {
    fn for_generation(generation: u64) -> Self {
        Self {
            generation,
            downloaded: 0,
            total: None,
            buffered_fraction: None,
            fully_buffered: false,
        }
    }

    fn adopt_initial(&mut self, initial: Self) {
        if self.generation != initial.generation || initial.fully_buffered {
            *self = initial;
            return;
        }
        if self.fully_buffered {
            return;
        }
        self.downloaded = self.downloaded.max(initial.downloaded);
        self.total = self.total.or(initial.total);
        self.buffered_fraction = match (self.buffered_fraction, initial.buffered_fraction) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (current, next) => current.or(next),
        };
    }

    fn fraction(&self) -> Option<f32> {
        if let Some(fraction) = self.buffered_fraction {
            return fraction.is_finite().then_some(fraction.clamp(0.0, 1.0));
        }
        let total = self.total.filter(|total| *total > 0)?;
        Some((self.downloaded as f32 / total as f32).clamp(0.0, 1.0))
    }
}

fn apply_download_progress(state: &mut PlaybackState, progress: DownloadProgress) -> bool {
    if progress.generation != state.generation {
        return false;
    }
    let Some(fraction) = progress.fraction() else {
        return false;
    };
    if state.duration.is_zero() {
        return false;
    }
    let next_buffered = state.duration.mul_f32(fraction);
    if next_buffered <= state.buffered {
        return false;
    }
    let buffered = state.buffered;
    state.set_buffered_fraction(fraction);
    state.buffered != buffered
}

fn completed_download_size(progress: DownloadProgress) -> Option<u64> {
    (progress.fully_buffered
        && progress
            .total
            .is_some_and(|total| total > 0 && total == progress.downloaded))
    .then_some(progress.downloaded)
}

fn effective_sink_gain(status: PlaybackStatus, user_volume: f32) -> f32 {
    if status == PlaybackStatus::Paused {
        0.0
    } else {
        user_volume
    }
}

fn pause_silently(set_volume: impl FnOnce(), pause: impl FnOnce()) {
    set_volume();
    pause();
}

impl PlaybackModel {
    fn reset_download_progress(&self) {
        if let Ok(mut progress) = self.download_progress.lock() {
            *progress = DownloadProgress::for_generation(self.state.generation);
        }
    }

    pub(crate) fn new(
        account: Entity<AccountState>,
        runtime: Arc<Runtime>,
        discord_presence: bool,
        cache: AudioCache,
        background_audio_cache: bool,
        seamless_playback: bool,
        record_deezer_plays: bool,
        listen_history_changed: Arc<ListenHistorySignal>,
        playback_preferences: (f32, bool, super::state::RepeatMode, bool),
        skip_explicit: bool,
        sidebar_preferences: (bool, RightSidebar),
        media: MediaSession,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut state = PlaybackState::default();
        state.restore_volume(playback_preferences.0, playback_preferences.1);
        state.set_repeat_mode(playback_preferences.2);
        state.set_shuffle_enabled(playback_preferences.3);
        let _ = state.set_skip_explicit(skip_explicit);
        state.restore_sidebar(sidebar_preferences.0, sidebar_preferences.1);
        let seek_slider = cx.new(|_| {
            SliderState::new()
                .max(1.)
                .step(SEEK_SLIDER_STEP)
                .default_value(0.)
        });
        let volume = state.volume;
        let volume_slider = cx.new(move |_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(volume)
        });
        let engine = RodioEngine::new();
        if let Ok(engine) = &engine {
            engine.set_volume(volume);
        }
        let mut model = Self {
            state,
            account,
            runtime: runtime.clone(),
            resolver: StreamResolver::new().map(|resolver| resolver.with_cache(cache.clone())),
            engine,
            user_fade_supervisor: UserFadeSupervisor::default(),
            user_fade_task: None,
            cancellation: CancellationToken::new(),
            seek_slider: seek_slider.clone(),
            volume_slider: volume_slider.clone(),
            seek_slider_interaction: SeekSliderInteraction::default(),
            seek_commit_epoch: 0,
            discord: DiscordPresence::new(discord_presence, &runtime),
            media,
            cache,
            background_audio_cache,
            seamless_playback,
            record_deezer_plays,
            deezer_listen: None,
            standby: StandbyPhase::default(),
            listen_history_changed,
            extension_in_flight: false,
            extension_exhausted: false,
            duplicate_extension_retries: 0,
            consecutive_failures: 0,
            resolved_quality: None,
            current_audio_info: None,
            loading_from_cache: false,
            download_progress: Arc::new(Mutex::new(DownloadProgress::default())),
        };
        cx.subscribe(&seek_slider, |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                this.seek_slider_interaction.record_pointer_change(*value);
            }
            match seek_slider_action(event) {
                SeekSliderAction::Preview(fraction) => this.preview_seek_fraction(fraction, cx),
                SeekSliderAction::Commit(fraction) => this.commit_seek_fraction(fraction, cx),
                SeekSliderAction::Ignore => {}
            }
        })
        .detach();
        cx.observe(&seek_slider, |this, slider, cx| {
            let value = slider.read(cx).value();
            if this.seek_slider_interaction.suppress_pointer_change(value) {
                return;
            }
            let current = if matches!(
                this.state.status,
                PlaybackStatus::Playing | PlaybackStatus::Paused
            ) && !this.state.duration.is_zero()
            {
                Some(this.state.position.as_secs_f32() / this.state.duration.as_secs_f32())
            } else {
                None
            };
            let Some(fraction) = seek_slider_accessibility_commit(
                value,
                current,
                this.seek_slider_interaction.preview_fraction,
            ) else {
                return;
            };
            this.seek_fraction(fraction, cx);
        })
        .detach();
        cx.subscribe(&volume_slider, |this, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(SliderValue::Single(volume)) = event {
                this.set_volume(*volume, cx);
            }
        })
        .detach();
        // OS media events arrive on foreign threads; the channel drains on
        // the main thread exactly like the poll loop below.
        if let Some(mut events) = model.media.take_events() {
            cx.spawn(async move |this, cx| {
                while let Some(request) = futures::StreamExt::next(&mut events).await {
                    if this
                        .update(cx, |this, cx| this.apply_media_request(request, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_millis(250)).await;
                if this
                    .update(cx, |this, cx| {
                        this.poll(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        model
    }

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
        self.runtime.spawn(async move {
            let _ = cache.set_limit(limit_mb).await;
        });
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
        cx: &mut Context<Self>,
    ) {
        let preserve_current = clear_remaining
            && matches!(
                &self.state.context,
                PlaybackContext::DeezerFlow { config_id: active, .. } if active == &config_id
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

    fn cancel_seek_slider_interaction(&mut self) {
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

    /// Apply one radio response and, when it contains only duplicates, return
    /// the fresh continuation ticket for a delayed retry.  The state method
    /// performs the ticket check and queue mutation as one operation.
    pub(crate) fn apply_extension_batch(
        &mut self,
        ticket: &QueueExtensionTicket,
        additions: Vec<PlaybackTrack>,
        clear_remaining: bool,
        next_flow_tuner: Option<crate::library::FlowTuner>,
        continuation_seed: Option<String>,
        batch_nonempty: bool,
        cx: &mut Context<Self>,
    ) -> Result<Option<QueueExtensionTicket>, ()> {
        let has_next_tuner = next_flow_tuner.is_some();
        match self.state.apply_extension(
            ticket,
            additions,
            clear_remaining,
            next_flow_tuner,
            continuation_seed,
        ) {
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

    fn reset_extension_state(&mut self) {
        self.extension_in_flight = false;
        self.extension_exhausted = false;
        self.duplicate_extension_retries = 0;
    }

    fn sync_discord(&mut self) {
        self.discord.playback_changed(&self.state);
        // The media session rides the same lifecycle hooks as Discord presence.
        self.media.playback_changed(&self.state);
    }

    fn cancel_user_fade(&mut self) {
        self.user_fade_task.take();
        self.user_fade_supervisor.cancel();
        self.sync_transport_after_fade_cancel();
    }

    fn cancel_user_fade_and_sync_transport(&mut self) {
        self.cancel_user_fade();
    }

    fn sync_transport_after_fade_cancel(&self) {
        let Ok(engine) = &self.engine else {
            return;
        };
        match self.state.status {
            PlaybackStatus::Playing => {
                engine.reset_transport_gain(1.0);
                engine.set_volume(effective_sink_gain(self.state.status, self.state.volume));
                engine.set_playback_intent(true);
                engine.play();
            }
            PlaybackStatus::Paused => {
                engine.reset_transport_gain(0.0);
                engine.set_playback_intent(false);
                pause_silently(|| engine.set_volume(0.0), || engine.pause());
            }
            _ => {
                engine.reset_transport_gain(1.0);
                engine.set_volume(effective_sink_gain(self.state.status, self.state.volume));
            }
        }
    }

    fn user_toggle_may_fade(&mut self, cx: &mut Context<Self>) -> bool {
        let (decision, boundary_probe) = match &self.standby {
            StandbyPhase::Armed(armed) => (
                user_toggle_fade_decision(
                    true,
                    armed.probe.queued(),
                    self.state.duration,
                    armed.probe.position(),
                ),
                Some(armed.probe.clone()),
            ),
            _ => (UserToggleFadeDecision::Fade, None),
        };
        match decision {
            UserToggleFadeDecision::Fade => true,
            UserToggleFadeDecision::Immediate => false,
            UserToggleFadeDecision::Boundary => {
                if let Some(probe) = boundary_probe {
                    self.standby_boundary(&probe, cx);
                }
                true
            }
        }
    }

    fn finish_user_toggle_immediately(&mut self, playing: bool) {
        self.user_fade_task.take();
        self.user_fade_supervisor.cancel();
        if let Ok(engine) = &self.engine {
            engine.set_playback_intent(playing);
            if playing {
                engine.reset_transport_gain(1.0);
                engine.set_volume(self.state.volume);
                engine.play();
            } else {
                engine.reset_transport_gain(0.0);
                pause_silently(|| engine.set_volume(0.0), || engine.pause());
                self.state.position = engine.position().min(self.state.duration);
            }
        }
    }

    fn start_user_fade(&mut self, playing: bool, may_fade: bool, cx: &mut Context<Self>) {
        let Some(probe) = self.engine.as_ref().ok().map(AudioEngine::sink_probe) else {
            return;
        };
        if !may_fade || self.state.volume <= f32::EPSILON {
            self.finish_user_toggle_immediately(playing);
            return;
        }

        self.user_fade_task.take();
        let target_gain = if playing { 1.0 } else { 0.0 };
        let plan = self.user_fade_supervisor.begin();
        let generation = self.state.generation;
        if let Ok(engine) = &self.engine {
            engine.set_transport_gain_target(target_gain);
            engine.set_playback_intent(playing);
            engine.set_volume(self.state.volume);
            if playing {
                engine.play();
            }
        }

        let executor = cx.background_executor().clone();
        let started = std::time::Instant::now();
        self.user_fade_task = Some(cx.spawn(async move |this, cx| {
            loop {
                executor.timer(USER_FADE_FRAME).await;
                let timed_out = started.elapsed() >= USER_FADE_SETTLE_TIMEOUT;
                let finished = this
                    .update(cx, |this, cx| {
                        if generation != this.state.generation
                            || !this
                                .engine
                                .as_ref()
                                .is_ok_and(|engine| engine.owns_probe(&probe))
                        {
                            return true;
                        }
                        if !this.user_fade_supervisor.is_current(plan) {
                            return true;
                        }
                        let Ok(engine) = &this.engine else {
                            this.user_fade_supervisor.complete(plan);
                            return true;
                        };
                        let settled = engine.transport_gain_settled(target_gain);
                        if !settled && !timed_out {
                            return false;
                        }
                        if !playing {
                            if !settled {
                                engine.reset_transport_gain(0.0);
                            }
                            pause_silently(|| engine.set_volume(0.0), || engine.pause());
                            this.state.position = engine.position().min(this.state.duration);
                            cx.notify();
                        }
                        this.user_fade_supervisor.complete(plan);
                        true
                    })
                    .unwrap_or(true);
                if finished {
                    break;
                }
            }
        }));
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.consecutive_failures = 0;
        if let Some(generation) = self.state.select(index) {
            self.start(generation, cx);
        }
    }

    fn handle_start_failure(
        &mut self,
        generation: u64,
        track: &PlaybackTrack,
        error: &str,
        cx: &mut Context<Self>,
    ) {
        self.state.fail(generation, error.to_owned());
        self.sync_discord();
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Error,
            "Playback Error",
            Some(format!("Could not play \"{}\": {error}", track.title).into()),
        );
        if self.state.first_upcoming_index().is_some()
            && self.consecutive_failures < MAX_CONSECUTIVE_AUTO_SKIPS
        {
            self.consecutive_failures += 1;
            if let Some(next_generation) = self.state.next() {
                self.start(next_generation, cx);
            }
        }
    }

    fn start(&mut self, generation: u64, cx: &mut Context<Self>) {
        self.finish_deezer_listen(cx);
        self.cancel_user_fade();
        self.cancel_seek_slider_interaction();
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.standby = StandbyPhase::Idle;
        self.resolved_quality = None;
        self.current_audio_info = None;
        self.sync_discord();
        if let Ok(engine) = self.engine.as_mut() {
            engine.stop();
        }
        let Some(track) = self.state.current().cloned() else {
            return;
        };
        self.loading_from_cache = self.cache.is_known_complete_track(&track);
        let resolver = match self.resolver.clone() {
            Ok(resolver) => resolver,
            Err(error) => {
                self.handle_start_failure(generation, &track, &error, cx);
                cx.notify();
                return;
            }
        };
        if let Some(error) = self.engine.as_ref().err().cloned() {
            self.handle_start_failure(generation, &track, &error, cx);
            cx.notify();
            return;
        }
        let (arl, soundcloud, murglar) = {
            let account = self.account.read(cx);
            (
                account.deezer_arl(),
                account.soundcloud_token(),
                account.murglar_media_credentials(),
            )
        };
        let cancellation = self.cancellation.clone();
        let history_resolver = resolver.clone();
        let history_track = track.clone();
        let history_arl = arl.clone();
        let prefetch_resolver = resolver.clone();
        let prefetch_arl = arl.clone();
        let prefetch_soundcloud = soundcloud.clone();
        let prefetch_murglar = murglar.clone();
        let track_provider = track.provider;
        let track_id = track.id.clone();
        if let Ok(mut progress) = self.download_progress.lock() {
            *progress = DownloadProgress::for_generation(generation);
        }
        let progress_target = self.download_progress.clone();
        let progress_callback: ProgressCallback = Arc::new(move |update: ProgressUpdate| {
            if let Ok(mut progress) = progress_target.lock() {
                if progress.generation == generation && !progress.fully_buffered {
                    progress.downloaded = update.downloaded;
                    progress.total = update.total;
                    if update.completed {
                        progress.fully_buffered = true;
                    }
                    if let Some(fraction) = update.buffered_fraction {
                        progress.buffered_fraction = Some(
                            progress
                                .buffered_fraction
                                .map_or(fraction, |current| current.max(fraction)),
                        );
                    }
                }
            }
        });
        let resolve_track = track.clone();
        let task = self.runtime.spawn(async move {
            let mut audio = resolver
                .resolve_progressive(
                    &resolve_track,
                    arl,
                    soundcloud,
                    murglar,
                    cancellation,
                    Some(progress_callback),
                )
                .await?;
            let worker = audio.take_progressive_worker();
            let info = ResolvedTrackInfo {
                format: audio.format,
                bytes: audio.total.unwrap_or(0),
                timeline_size_unknown: audio.timeline_size_unknown,
                declared_bitrate: audio.declared_bitrate,
            };
            let initial_progress = DownloadProgress {
                generation,
                downloaded: audio.initial_downloaded,
                total: audio.total,
                buffered_fraction: audio.initial_buffered_fraction,
                fully_buffered: audio.fully_cached,
            };
            let prepared =
                match tokio::task::spawn_blocking(move || RodioEngine::decode_progressive(audio))
                    .await
                {
                    Ok(Ok(prepared)) => prepared,
                    Ok(Err(error)) => {
                        worker.cancel_and_join().await;
                        return Err(error);
                    }
                    Err(_) => {
                        worker.cancel_and_join().await;
                        return Err("The playback worker stopped unexpectedly".to_string());
                    }
                };
            Ok::<_, String>((prepared, info, initial_progress, worker))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The playback worker stopped unexpectedly".into()));
            let worker = this.update(cx, |this, cx| {
                if generation != this.state.generation {
                    return result.ok().map(|(_, _, _, worker)| worker);
                }
                let cleanup = match result {
                    Ok((prepared, info, initial_progress, worker)) => {
                        this.cancel_user_fade();
                        match this.engine.as_mut() {
                            Ok(engine) => {
                                let duration = engine.load(prepared, this.state.volume);
                                worker.detach();
                                this.consecutive_failures = 0;
                                if let Ok(mut progress) = this.download_progress.lock() {
                                    progress.adopt_initial(initial_progress);
                                }
                                this.apply_loaded_buffer_state(
                                    generation,
                                    duration,
                                    initial_progress,
                                );
                                this.current_audio_info =
                                    Some((track_provider, track_id.clone(), info));
                                this.resolved_quality = quality_label(
                                    player_format_label(info.format),
                                    info.declared_bitrate,
                                    Some(info.bytes),
                                    duration,
                                );
                                this.sync_discord();
                                this.prefetch_adjacent(
                                    &prefetch_resolver,
                                    prefetch_arl.clone(),
                                    prefetch_soundcloud.clone(),
                                    prefetch_murglar.clone(),
                                );
                                this.begin_listen_reporting(
                                    &history_resolver,
                                    &history_track,
                                    history_arl,
                                    cx,
                                );
                                None
                            }
                            Err(error) => {
                                let err = error.clone();
                                this.handle_start_failure(generation, &track, &err, cx);
                                Some(worker)
                            }
                        }
                    }
                    Err(error) if error != "Playback request cancelled" => {
                        this.handle_start_failure(generation, &track, &error, cx);
                        None
                    }
                    Err(_) => None,
                };
                cx.notify();
                cleanup
            });
            if let Ok(Some(worker)) = worker {
                worker.cancel_and_join().await;
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn loading_from_cache(&self) -> bool {
        self.loading_from_cache
    }

    fn prefetch_adjacent(
        &self,
        resolver: &StreamResolver,
        arl: Option<crate::search::DeezerArl>,
        soundcloud: Option<crate::search::SoundCloudToken>,
        murglar: Option<super::MediaCredentials>,
    ) {
        let Some(track) = adjacent_track(self.background_audio_cache, &self.state).cloned() else {
            return;
        };
        let resolver = resolver.clone();
        let cancellation = self.cancellation.clone();
        let generation = self.cache.generation();
        self.runtime.spawn(async move {
            let _ = resolver
                .prefetch(&track, arl, soundcloud, murglar, cancellation, generation)
                .await;
        });
    }

    /// Drives the standby lifecycle from the poll loop: prepares the next
    /// track near the end of the current one and acts as a fallback boundary
    /// detector if the watcher task ever dies.
    fn advance_standby(&mut self, cx: &mut Context<Self>) {
        match &self.standby {
            StandbyPhase::Armed(_) => {
                let Some(engine) = self.engine.as_ref().ok() else {
                    self.standby = StandbyPhase::Idle;
                    return;
                };
                let probe = engine.sink_probe();
                if standby::watch_tick(probe.queued(), probe.is_paused()) == WatchTick::Boundary {
                    self.standby_boundary(&probe, cx);
                }
            }
            StandbyPhase::Idle => self.begin_standby(cx),
            StandbyPhase::Pending => {}
        }
    }

    fn begin_standby(&mut self, cx: &mut Context<Self>) {
        if !self.seamless_playback
            || self.engine.is_err()
            || self.resolver.is_err()
            || self.state.repeat_mode == super::state::RepeatMode::One
            || !standby::should_prepare_status(
                self.state.status,
                self.state.duration,
                self.state.position,
            )
        {
            return;
        }
        let Some(next_track) = self.next_upcoming_track() else {
            return;
        };
        self.prepare_standby(next_track, cx);
    }

    /// Resolves, downloads, and decodes the next track off the UI thread so
    /// it can be appended to the live sink without a gap later.
    fn prepare_standby(&mut self, track: PlaybackTrack, cx: &mut Context<Self>) {
        let Ok(resolver) = self.resolver.clone() else {
            return;
        };
        let (arl, soundcloud, murglar) = {
            let account = self.account.read(cx);
            (
                account.deezer_arl(),
                account.soundcloud_token(),
                account.murglar_media_credentials(),
            )
        };
        let cancellation = self.cancellation.clone();
        let generation = self.state.generation;
        self.standby = StandbyPhase::Pending;
        let resolve_track = track.clone();
        let task = self.runtime.spawn(async move {
            let audio = resolver
                .resolve(&resolve_track, arl, soundcloud, murglar, cancellation, None)
                .await?;
            let file_size = std::fs::metadata(&audio.path).ok().map(|m| m.len());
            let info = ResolvedTrackInfo {
                format: audio.format,
                bytes: file_size.unwrap_or(0),
                timeline_size_unknown: false,
                declared_bitrate: audio.declared_bitrate,
            };
            let prepared = tokio::task::spawn_blocking(move || RodioEngine::decode(audio))
                .await
                .map_err(|_| "The playback worker stopped unexpectedly".to_string())??;
            Ok((prepared, info))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The playback worker stopped unexpectedly".into()));
            this.update(cx, |this, cx| {
                this.standby_ready(track, generation, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn standby_ready(
        &mut self,
        track: PlaybackTrack,
        generation: u64,
        result: Result<(PreparedSource, ResolvedTrackInfo), String>,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.standby, StandbyPhase::Pending) {
            return;
        }
        self.standby = StandbyPhase::Idle;
        if generation != self.state.generation
            || !self.seamless_playback
            || !matches!(
                self.state.status,
                PlaybackStatus::Playing | PlaybackStatus::Paused
            )
            || self.next_upcoming_id().as_deref() != Some(track.id.as_str())
        {
            // Stale or superseded; the normal on-end path takes over.
            return;
        }
        let Ok((prepared, info)) = result else {
            // Resolve or decode failed; keep today's on-end behavior.
            return;
        };
        let duration = prepared.duration();
        let quality = quality_label(
            player_format_label(info.format),
            info.declared_bitrate,
            Some(info.bytes),
            duration,
        );
        let Ok(engine) = self.engine.as_mut() else {
            return;
        };
        let probe = engine.sink_probe();
        engine.append_standby(prepared);
        self.standby = StandbyPhase::Armed(ArmedStandby {
            track,
            generation,
            duration,
            quality,
            audio_info: Some(info),
            probe: probe.clone(),
        });
        self.watch_standby_boundary(probe, cx);
    }

    /// Dedicated watcher for the track boundary. The 250 ms poll is far too
    /// coarse to line the state switch up with the audio, so this task polls
    /// the sink finely once the end is close.
    fn watch_standby_boundary(&mut self, probe: SinkProbe, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let track_duration = self.state.duration;
        let start_position = self.state.position;
        let start_instant = std::time::Instant::now();
        cx.spawn(async move |this, cx| {
            loop {
                let paused = probe.is_paused();
                match standby::watch_tick(probe.queued(), paused) {
                    WatchTick::Stop => return,
                    WatchTick::Boundary => break,
                    WatchTick::Continue => {}
                }
                let elapsed = start_instant.elapsed();
                let remaining = track_duration.saturating_sub(start_position + elapsed);
                executor
                    .timer(standby::poll_interval(remaining, paused))
                    .await;
            }
            this.update(cx, |this, cx| this.standby_boundary(&probe, cx))
                .ok();
        })
        .detach();
    }

    fn standby_boundary(&mut self, probe: &SinkProbe, cx: &mut Context<Self>) {
        let StandbyPhase::Armed(armed) = std::mem::take(&mut self.standby) else {
            return;
        };
        let outcome = standby::boundary_outcome(&standby::BoundaryCheck {
            sink_current: self
                .engine
                .as_ref()
                .is_ok_and(|engine| engine.owns_probe(probe)),
            status: self.state.status,
            generation: self.state.generation,
            armed_generation: armed.generation,
            armed_target: armed.track.id.clone(),
            upcoming_first: self.next_upcoming_id(),
        });
        match outcome {
            standby::BoundaryOutcome::Ignore => {}
            standby::BoundaryOutcome::Replace => {
                self.finish_deezer_listen(cx);
                self.cancel_user_fade();
                // The queued audio no longer matches the queue; load the real
                // next track the normal way.
                if let Some(generation) = self.state.next() {
                    self.start(generation, cx);
                } else {
                    if let Ok(engine) = self.engine.as_mut() {
                        engine.stop();
                    }
                    self.sync_discord();
                    cx.notify();
                }
            }
            standby::BoundaryOutcome::Commit => self.commit_standby(armed, cx),
        }
    }

    /// Moves model state onto the already playing standby source, firing the
    /// same track-start effects as a regular load.
    fn commit_standby(&mut self, armed: ArmedStandby, cx: &mut Context<Self>) {
        self.finish_deezer_listen(cx);
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        let Some(generation) = self.state.next() else {
            if let Ok(engine) = self.engine.as_mut() {
                engine.stop();
            }
            self.sync_discord();
            cx.notify();
            return;
        };
        if self.state.current_id() != Some(armed.track.id.as_str()) {
            // The selection diverged; fall back to a regular load.
            self.start(generation, cx);
            return;
        }
        let ArmedStandby {
            track: armed_track,
            duration: armed_duration,
            quality: armed_quality,
            audio_info: armed_audio_info,
            ..
        } = armed;
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        if let Ok(mut progress) = self.download_progress.lock() {
            *progress = DownloadProgress::for_generation(generation);
        }
        self.state.loaded_fully_buffered(generation, armed_duration);
        self.resolved_quality = armed_quality;
        self.current_audio_info =
            armed_audio_info.map(|info| (armed_track.provider, armed_track.id.clone(), info));
        if let Ok(engine) = self.engine.as_mut() {
            engine.activate_standby();
        }
        if self
            .engine
            .as_ref()
            .is_ok_and(|engine| engine.sink_probe().is_paused())
        {
            self.state.status = PlaybackStatus::Paused;
        }
        self.sync_discord();
        if let Ok(resolver) = self.resolver.clone() {
            let (arl, soundcloud, murglar) = {
                let account = self.account.read(cx);
                (
                    account.deezer_arl(),
                    account.soundcloud_token(),
                    account.murglar_media_credentials(),
                )
            };
            self.prefetch_adjacent(&resolver, arl.clone(), soundcloud.clone(), murglar);
            self.begin_listen_reporting(&resolver, &armed_track, arl, cx);
        }
        cx.notify();
    }

    /// A manual skip can hand off to the armed standby instantly, mirroring
    /// the seamless handoff the automatic boundary uses.
    fn use_standby_for_skip(&self) -> bool {
        let StandbyPhase::Armed(armed) = &self.standby else {
            return false;
        };
        matches!(
            self.state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) && self.next_upcoming_id().as_deref() == Some(armed.track.id.as_str())
            && self
                .engine
                .as_ref()
                .is_ok_and(|engine| engine.owns_probe(&armed.probe))
    }

    fn next_upcoming_id(&self) -> Option<String> {
        self.next_upcoming_track().map(|track| track.id)
    }

    fn next_upcoming_track(&self) -> Option<PlaybackTrack> {
        self.state
            .first_upcoming_index()
            .and_then(|index| self.state.queue.get(index))
            .filter(|track| !self.state.explicit_blocked(track))
            .cloned()
    }

    fn record_deezer_listen(
        &self,
        resolver: &StreamResolver,
        payload: serde_json::Value,
        arl: crate::search::DeezerArl,
        completed_listen: bool,
        cx: &mut Context<Self>,
    ) {
        let resolver = resolver.clone();
        let history_changed = self.listen_history_changed.clone();
        let entity = cx.entity().clone();
        let account_scope = self.account.read(cx).library_scope();
        let task = self
            .runtime
            .spawn(async move { resolver.record_deezer_listen(payload, arl).await });
        cx.spawn(async move |_, cx| {
            let request_succeeded = task.await.is_ok_and(|succeeded| succeeded);
            if listen_history_changed_on_completion(completed_listen, request_succeeded) {
                let _ = entity.update(cx, |this, cx| {
                    if !listen_history_scope_matches(
                        &account_scope,
                        &this.account.read(cx).library_scope(),
                    ) {
                        return;
                    }
                    history_changed.mark(PlaybackProvider::Deezer);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn begin_listen_reporting(
        &mut self,
        resolver: &StreamResolver,
        track: &PlaybackTrack,
        deezer_arl: Option<crate::search::DeezerArl>,
        cx: &mut Context<Self>,
    ) {
        match track.provider {
            PlaybackProvider::Deezer if self.record_deezer_plays => {
                let Some(arl) = deezer_arl else {
                    return;
                };
                self.deezer_listen = Some(DeezerListenSession::start(
                    track.id.clone(),
                    self.state.shuffle_enabled,
                    self.state.status == PlaybackStatus::Playing,
                ));
                self.record_deezer_listen(resolver, deezer_next_media(&track.id), arl, false, cx);
            }
            PlaybackProvider::SoundCloud => {
                let Some(report) =
                    SoundCloudListenReport::from_playback(&self.state.context, &track.id)
                else {
                    return;
                };
                let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
                    return;
                };
                self.record_soundcloud_listen(resolver, report, token, cx);
            }
            _ => {}
        }
    }

    fn record_soundcloud_listen(
        &self,
        resolver: &StreamResolver,
        report: SoundCloudListenReport,
        token: crate::search::SoundCloudToken,
        cx: &mut Context<Self>,
    ) {
        let resolver = resolver.clone();
        let history_changed = self.listen_history_changed.clone();
        let entity = cx.entity().clone();
        let account_scope = self.account.read(cx).library_scope();
        let task = self
            .runtime
            .spawn(async move { resolver.record_soundcloud_listen(report, token).await });
        cx.spawn(async move |_, cx| {
            if task.await.is_ok_and(|succeeded| succeeded) {
                let _ = entity.update(cx, |this, cx| {
                    if !listen_history_scope_matches(
                        &account_scope,
                        &this.account.read(cx).library_scope(),
                    ) {
                        return;
                    }
                    history_changed.mark(PlaybackProvider::SoundCloud);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn finish_deezer_listen(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.deezer_listen.take() else {
            return;
        };
        if !self.record_deezer_plays {
            return;
        }
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            return;
        };
        let Ok(resolver) = self.resolver.clone() else {
            return;
        };
        self.record_deezer_listen(&resolver, session.finish(), arl, true, cx);
    }

    /// Applies an OS media control request, mirroring the original app's
    /// mediaSession handlers (public/js/core/app.js): play and pause only act
    /// in their matching direction, and seek keys with nothing playable fall
    /// back to skipping tracks.
    fn apply_media_request(&mut self, request: MediaRequest, cx: &mut Context<Self>) {
        let playable = matches!(
            self.state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        );
        match request {
            MediaRequest::Play => {
                if media_play_should_toggle(self.state.status) {
                    self.toggle(cx);
                }
            }
            MediaRequest::Pause => {
                if matches!(
                    self.state.status,
                    PlaybackStatus::Loading | PlaybackStatus::Playing
                ) {
                    self.toggle(cx);
                }
            }
            MediaRequest::Toggle => self.toggle(cx),
            MediaRequest::Next => {
                if self.state.can_next() {
                    self.next(cx);
                }
            }
            MediaRequest::Previous => self.previous(cx),
            MediaRequest::SeekForward(step) => {
                if playable {
                    self.seek_by(step.as_secs() as i64, cx);
                } else if self.state.can_next() {
                    self.next(cx);
                }
            }
            MediaRequest::SeekBackward(step) => {
                if playable {
                    self.seek_by(-(step.as_secs() as i64), cx);
                } else {
                    self.previous(cx);
                }
            }
            MediaRequest::SeekPosition(position) => self.seek_to(position, cx),
        }
    }

    pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.state.status == PlaybackStatus::Loading {
            self.cancel_pending_load(cx);
            return;
        }
        if self.state.status == PlaybackStatus::Ended {
            if let Some(index) = self.state.current_index {
                if let Some(generation) = self.state.select(index) {
                    self.start(generation, cx);
                    return;
                }
            }
        }
        let may_fade = self.user_toggle_may_fade(cx);
        if let Some(playing) = self.state.toggle() {
            if let Some(session) = self.deezer_listen.as_mut() {
                session.set_playing(playing);
            }
            self.start_user_fade(playing, may_fade, cx);
            self.sync_discord();
            cx.notify();
        }
    }

    fn cancel_pending_load(&mut self, cx: &mut Context<Self>) {
        self.cancellation.cancel();
        self.cache.cancel();
        self.cancel_user_fade();
        self.cancel_seek_slider_interaction();
        self.standby = StandbyPhase::Idle;
        self.reset_extension_state();
        self.reset_download_progress();
        if let Ok(engine) = self.engine.as_mut() {
            engine.stop();
        }
        if self.state.stop_loading() {
            self.sync_discord();
            cx.notify();
        }
    }

    pub(crate) fn previous(&mut self, cx: &mut Context<Self>) {
        self.consecutive_failures = 0;
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if self.state.status == PlaybackStatus::Ended {
            if let Some(index) = self.state.current_index {
                if let Some(generation) = self.state.select(index) {
                    self.start(generation, cx);
                    return;
                }
            }
        }
        match self.state.previous() {
            PreviousAction::None => {}
            PreviousAction::Restart => {
                if let Some(session) = self.deezer_listen.as_mut() {
                    session.record_seek();
                }
                let standby_dropped = self
                    .engine
                    .as_mut()
                    .ok()
                    .and_then(|engine| engine.seek(Duration::ZERO).ok())
                    .is_some_and(|outcome| outcome == SeekOutcome::AppliedStandbyDropped);
                if standby_dropped {
                    self.standby = StandbyPhase::Idle;
                }
                self.sync_discord();
                cx.notify();
            }
            PreviousAction::Load(generation) => self.start(generation, cx),
        }
    }

    pub(crate) fn next(&mut self, cx: &mut Context<Self>) {
        self.consecutive_failures = 0;
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if self.use_standby_for_skip()
            && let StandbyPhase::Armed(armed) = std::mem::take(&mut self.standby)
        {
            if let Ok(engine) = self.engine.as_mut() {
                engine.skip_to_standby();
            }
            self.commit_standby(armed, cx);
            return;
        }
        self.finish_deezer_listen(cx);
        if let Some(generation) = self.state.next() {
            self.start(generation, cx);
        } else {
            self.sync_discord();
            cx.notify();
        }
    }

    pub(crate) fn seek_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if !matches!(
            self.state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) || self.state.duration.is_zero()
        {
            return;
        }
        self.seek_commit_epoch = self.seek_commit_epoch.wrapping_add(1);
        if let Some(session) = self.deezer_listen.as_mut() {
            session.record_seek();
        }
        let position = self.state.duration.mul_f32(fraction.clamp(0.0, 1.0));
        self.request_seek(position, cx);
        self.sync_discord();
        cx.notify();
    }

    pub(crate) fn seek_to(&mut self, position: Duration, cx: &mut Context<Self>) {
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if !matches!(
            self.state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) || self.state.duration.is_zero()
        {
            return;
        }
        if let Some(session) = self.deezer_listen.as_mut() {
            session.record_seek();
        }
        self.request_seek(position, cx);
        self.sync_discord();
        cx.notify();
    }

    fn seek_by(&mut self, seconds: i64, cx: &mut Context<Self>) {
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if self.state.duration.is_zero() {
            return;
        }
        if let Some(session) = self.deezer_listen.as_mut() {
            session.record_seek();
        }
        let position = if seconds < 0 {
            self.state
                .position
                .saturating_sub(Duration::from_secs(seconds.unsigned_abs()))
        } else {
            self.state
                .position
                .saturating_add(Duration::from_secs(seconds as u64))
        };
        self.request_seek(position, cx);
        self.sync_discord();
        cx.notify();
    }

    fn arm_seek_completion(&mut self, completion: Option<SeekCompletion>, cx: &mut Context<Self>) {
        let Some(completion) = completion else {
            return;
        };
        let generation = self.state.generation;
        cx.spawn(async move |this, cx| {
            completion.wait().await;
            let _ = this.update(cx, |this, cx| {
                if this.state.generation == generation {
                    this.poll(cx);
                }
            });
        })
        .detach();
    }

    fn request_seek(&mut self, position: Duration, cx: &mut Context<Self>) {
        let position = position.min(self.state.duration);
        let (outcome, completion) = match self.engine.as_mut() {
            Ok(engine) => {
                let outcome = engine.seek(position);
                let completion = engine.take_seek_completion();
                (outcome, completion)
            }
            Err(_) => {
                self.state.seek(position);
                return;
            }
        };
        match outcome {
            Ok(SeekOutcome::Applied) => {
                self.state.seek(position);
            }
            Ok(SeekOutcome::AppliedStandbyDropped) => {
                self.state.seek(position);
                self.standby = StandbyPhase::Idle;
            }
            Ok(SeekOutcome::Deferred) if completion.is_some() => {
                self.state.seek(position);
            }
            Ok(SeekOutcome::Deferred) => {}
            Err(error) => self.state.error = Some(error),
        }
        self.arm_seek_completion(completion, cx);
    }

    pub(crate) fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        let volume = self.state.set_volume(volume);
        if let Ok(engine) = &self.engine {
            engine.set_volume(effective_sink_gain(self.state.status, volume));
        }
        cx.notify();
    }

    pub(crate) fn apply_imported_preferences(
        &mut self,
        volume: f32,
        muted: bool,
        repeat_mode: super::state::RepeatMode,
        shuffle_enabled: bool,
        right_sidebar_open: bool,
        right_sidebar: RightSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.restore_volume(volume, muted);
        self.state.set_repeat_mode(repeat_mode);
        self.state.set_shuffle_enabled(shuffle_enabled);
        self.state
            .restore_sidebar(right_sidebar_open, right_sidebar);
        if let Ok(engine) = &self.engine {
            engine.set_volume(effective_sink_gain(self.state.status, self.state.volume));
        }
        let volume = self.state.volume;
        self.volume_slider
            .update(cx, |slider, cx| slider.set_value(volume, window, cx));
        cx.notify();
    }

    pub(crate) fn toggle_mute(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let volume = self.state.toggle_mute();
        if let Ok(engine) = &self.engine {
            engine.set_volume(effective_sink_gain(self.state.status, volume));
        }
        self.volume_slider
            .update(cx, |slider, cx| slider.set_value(volume, window, cx));
        cx.notify();
    }

    pub(crate) fn toggle_lyrics(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_sidebar(RightSidebar::Lyrics);
        cx.notify();
    }

    /// Opens the lyrics sidebar for any track. When the track is playing the
    /// panel keeps following playback; otherwise the panel loads lyrics for
    /// the context track without touching playback.
    pub(crate) fn open_lyrics_for(&mut self, track: &PlaybackTrack, cx: &mut Context<Self>) {
        self.state.open_lyrics_context(track);
        cx.notify();
    }

    /// Queues a batch of tracks next or last. With nothing playing the batch
    /// becomes the queue, matching addToQueue in the original app.
    pub(crate) fn enqueue_tracks(
        &mut self,
        additions: Vec<PlaybackTrack>,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        if additions.is_empty() {
            return;
        }
        if self.state.current_index.is_none() {
            self.replace_queue(additions, 0, cx);
            return;
        }
        let should_play = self.state.status == PlaybackStatus::Ended;
        let count = self.state.insert_queue_additions(additions, last);
        self.reset_extension_state();
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Success,
            if last {
                "Added to End of Queue"
            } else {
                "Playing Next"
            },
            Some(
                format!(
                    "{count} {} added.",
                    if count == 1 { "track" } else { "tracks" }
                )
                .into(),
            ),
        );
        if should_play {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    /// Supplies a read-only probe that resolves a track's format and size
    /// through the playback resolver without starting playback.
    pub(crate) fn track_info_probe(&self, cx: &gpui::App) -> Option<super::TrackInfoProbe> {
        let account = self.account.read(cx);
        Some(super::TrackInfoProbe::new(
            self.resolver.clone().ok()?,
            self.runtime.clone(),
            account.deezer_arl(),
            account.soundcloud_token(),
            account.murglar_media_credentials(),
        ))
    }

    pub(crate) fn toggle_queue(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_sidebar(RightSidebar::Queue);
        cx.notify();
    }

    pub(crate) fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_shuffle();
        cx.notify();
    }

    pub(crate) fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.state.cycle_repeat_mode();
        cx.notify();
    }

    pub(crate) fn playback_preferences(&self) -> (f32, bool, super::state::RepeatMode, bool) {
        (
            self.state.persisted_volume(),
            self.state.muted,
            self.state.repeat_mode,
            self.state.shuffle_enabled,
        )
    }

    fn remove(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.remove(index) {
            self.reset_extension_state();
            cx.notify();
        }
    }

    pub(crate) fn select_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select(index, cx);
    }

    pub(crate) fn remove_from_queue(&mut self, index: usize, cx: &mut Context<Self>) {
        self.remove(index, cx);
    }

    pub(crate) fn enqueue_next(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.enqueue_next(index) {
            if self.state.status == PlaybackStatus::Ended {
                self.next(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(crate) fn enqueue_last(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.state.enqueue_last(index) {
            if self.state.status == PlaybackStatus::Ended {
                self.next(cx);
            } else {
                cx.notify();
            }
        }
    }

    pub(crate) fn enqueue_track(
        &mut self,
        track: PlaybackTrack,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.current_index.is_none() {
            self.replace_queue(vec![track], 0, cx);
            return;
        }
        let should_play = self.state.status == PlaybackStatus::Ended;
        self.state.insert_queue_additions(vec![track], last);
        self.reset_extension_state();
        if should_play {
            self.next(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn reorder(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if self.state.reorder(from, to) {
            self.reset_extension_state();
            cx.notify();
        }
    }

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.finish_deezer_listen(cx);
        self.cancel_user_fade();
        self.cancel_seek_slider_interaction();
        self.cancellation.cancel();
        self.cache.cancel();
        self.standby = StandbyPhase::Idle;
        if let Ok(engine) = self.engine.as_mut() {
            engine.stop();
        }
        self.state.clear();
        self.reset_extension_state();
        self.reset_download_progress();
        self.current_audio_info = None;
        self.sync_discord();
        cx.notify();
    }

    pub(crate) fn account_scope_changed(&mut self, cx: &mut Context<Self>) {
        self.deezer_listen = None;
        self.cancel_user_fade();
        self.cancel_seek_slider_interaction();
        self.cancellation.cancel();
        if let Ok(resolver) = &self.resolver {
            resolver.clear_resolved_source_cache();
        }
        self.cache.cancel();
        self.standby = StandbyPhase::Idle;
        if let Ok(engine) = self.engine.as_mut() {
            engine.stop();
        }
        self.state.clear();
        self.reset_extension_state();
        self.reset_download_progress();
        self.current_audio_info = None;
        self.sync_discord();
        cx.notify();
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let progress = self.download_progress.lock().ok().map(|progress| *progress);
        let completed_audio_info = self.adopt_completed_audio_size(progress);
        let buffered_changed = matches!(
            self.state.status,
            PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
        ) && progress
            .is_some_and(|progress| apply_download_progress(&mut self.state, progress));
        match self.state.status {
            PlaybackStatus::Loading => {
                if buffered_changed || completed_audio_info {
                    cx.notify();
                }
            }
            PlaybackStatus::Playing | PlaybackStatus::Paused => {
                let (deferred_seek, position, ended, completion) = {
                    let Ok(engine) = self.engine.as_mut() else {
                        if buffered_changed || completed_audio_info {
                            cx.notify();
                        }
                        return;
                    };
                    let deferred_seek = engine.apply_deferred_seek();
                    let completion = engine.take_seek_completion();
                    (
                        deferred_seek,
                        engine.position().min(self.state.duration),
                        self.state.status == PlaybackStatus::Playing && engine.ended(),
                        completion,
                    )
                };
                self.arm_seek_completion(completion, cx);
                let standby_dropped = match deferred_seek {
                    Ok(SeekOutcome::AppliedStandbyDropped) => true,
                    Ok(_) => false,
                    Err(error) => {
                        self.state.error = Some(error);
                        false
                    }
                };
                self.state.position = position;
                if standby_dropped {
                    self.standby = StandbyPhase::Idle;
                }
                if ended {
                    self.next(cx);
                    return;
                }
                if buffered_changed
                    || completed_audio_info
                    || self.state.status == PlaybackStatus::Playing
                {
                    cx.notify();
                }
                if self.state.status == PlaybackStatus::Playing {
                    self.advance_standby(cx);
                }
            }
            _ => {
                if buffered_changed || completed_audio_info {
                    cx.notify();
                }
            }
        }
    }

    fn adopt_completed_audio_size(&mut self, progress: Option<DownloadProgress>) -> bool {
        let Some(progress) = progress else {
            return false;
        };
        if progress.generation != self.state.generation {
            return false;
        }
        let Some(size) = completed_download_size(progress) else {
            return false;
        };
        let Some(current_track) = self.state.current() else {
            return false;
        };
        let current_info_matches_track =
            self.current_audio_info
                .as_ref()
                .is_some_and(|(provider, track_id, _)| {
                    *provider == current_track.provider && track_id == &current_track.id
                });
        if !current_info_matches_track {
            return false;
        }
        let (format, declared_bitrate) = {
            let Some((_, _, info)) = self.current_audio_info.as_mut() else {
                return false;
            };
            if info.bytes > 0 {
                return false;
            }
            info.bytes = size;
            (info.format, info.declared_bitrate)
        };
        self.resolved_quality = quality_label(
            player_format_label(format),
            declared_bitrate,
            Some(size),
            Some(self.state.duration),
        );
        true
    }

    fn apply_loaded_buffer_state(
        &mut self,
        generation: u64,
        duration: Option<Duration>,
        initial_progress: DownloadProgress,
    ) {
        let progress = self
            .download_progress
            .lock()
            .ok()
            .map(|progress| *progress)
            .unwrap_or(initial_progress);
        if progress.fully_buffered {
            self.state.loaded_fully_buffered(generation, duration);
        } else {
            self.state.loaded_progressive(generation, duration);
            apply_download_progress(&mut self.state, progress);
        }
    }
}

fn adjacent_track(enabled: bool, state: &PlaybackState) -> Option<&PlaybackTrack> {
    if !enabled {
        return None;
    }
    state
        .upcoming_indices()
        .first()
        .and_then(|index| state.queue.get(*index))
        .filter(|track| !state.explicit_blocked(track))
}

const fn listen_history_changed_on_completion(
    completed_listen: bool,
    request_succeeded: bool,
) -> bool {
    completed_listen && request_succeeded
}

fn listen_history_scope_matches(scheduled_scope: &str, current_scope: &str) -> bool {
    scheduled_scope == current_scope
}

fn media_play_should_toggle(status: PlaybackStatus) -> bool {
    matches!(
        status,
        PlaybackStatus::Loading | PlaybackStatus::Paused | PlaybackStatus::Ended
    )
}

fn quality_label(
    format: &str,
    declared_bitrate: Option<u32>,
    file_size: Option<u64>,
    duration: Option<Duration>,
) -> Option<String> {
    if let Some(bitrate) = declared_bitrate.filter(|bitrate| *bitrate > 0) {
        return Some(format_bitrate(format, u64::from(bitrate)));
    }
    let seconds = duration?.as_secs();
    if seconds == 0 {
        return Some(format.to_owned());
    }
    match file_size {
        Some(bytes) if bytes > 0 => {
            let kbps = (bytes * 8) / (seconds * 1000);
            Some(format_bitrate(format, kbps))
        }
        _ => Some(format.to_owned()),
    }
}

fn player_format_label(format: super::resolver::AudioFormat) -> &'static str {
    match format {
        super::resolver::AudioFormat::M4a => "AAC",
        _ => format.label(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SeekSliderAction {
    Preview(f32),
    Commit(f32),
    Ignore,
}

fn seek_slider_action(event: &SliderEvent) -> SeekSliderAction {
    match event {
        SliderEvent::Change(SliderValue::Single(fraction)) => {
            SeekSliderAction::Preview(fraction.clamp(0., 1.))
        }
        SliderEvent::Release(SliderValue::Single(fraction)) => {
            SeekSliderAction::Commit(fraction.clamp(0., 1.))
        }
        SliderEvent::Change(SliderValue::Range(_, _))
        | SliderEvent::Release(SliderValue::Range(_, _)) => SeekSliderAction::Ignore,
    }
}

fn seek_slider_accessibility_commit(
    value: SliderValue,
    current_fraction: Option<f32>,
    preview_fraction: Option<f32>,
) -> Option<f32> {
    let SliderValue::Single(value) = value else {
        return None;
    };
    let current_fraction = current_fraction?;
    if preview_fraction.is_some() {
        return None;
    }
    let value = value.clamp(0.0, 1.0);
    (value - current_fraction)
        .abs()
        .gt(&0.0001)
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::{
        DownloadProgress, SEEK_SLIDER_STEP, SeekSliderAction, SeekSliderInteraction,
        adjacent_track, apply_download_progress, completed_download_size, effective_sink_gain,
        listen_history_changed_on_completion, listen_history_scope_matches,
        media_play_should_toggle, pause_silently, player_format_label, quality_label,
        seek_slider_accessibility_commit, seek_slider_action,
    };
    use crate::playback::fade::{USER_FADE_DURATION, USER_FADE_SETTLE_TIMEOUT};
    use crate::playback::{PlaybackProvider, PlaybackState, PlaybackStatus, PlaybackTrack};
    use gpui_component::slider::{SliderEvent, SliderValue};

    #[test]
    fn paused_playback_keeps_the_effective_sink_gain_silent() {
        assert_eq!(effective_sink_gain(PlaybackStatus::Paused, 0.8), 0.0);
        assert_eq!(effective_sink_gain(PlaybackStatus::Paused, 0.2), 0.0);
        assert_eq!(effective_sink_gain(PlaybackStatus::Playing, 0.8), 0.8);
    }

    #[test]
    fn silent_pause_mutes_before_pausing() {
        let steps = RefCell::new(Vec::new());
        pause_silently(
            || {
                steps.borrow_mut().push("mute");
            },
            || steps.borrow_mut().push("pause"),
        );
        assert_eq!(*steps.borrow(), ["mute", "pause"]);
    }

    #[test]
    fn production_pause_paths_use_the_silent_pause_helper() {
        let source = include_str!("view.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert_eq!(production.matches("pause_silently(").count(), 4);
        assert_eq!(production.matches("engine.set_volume(0.0)").count(), 3);

        let helper = &production[production.find("fn pause_silently").unwrap()..];
        assert!(helper.find("set_volume();").unwrap() < helper.find("pause();").unwrap());

        let compact: String = production.split_whitespace().collect();
        assert!(!compact.contains("engine.pause();engine.set_volume(self.state.volume);"));
    }

    #[test]
    fn user_fade_publishes_one_target_and_does_not_step_sink_volume() {
        let source = include_str!("view.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let fade_start = production.find("fn start_user_fade").unwrap();
        let fade_end = production[fade_start..].find("\n    fn select").unwrap();
        let fade = &production[fade_start..fade_start + fade_end];
        let setup_end = fade.find("let executor").unwrap();
        let setup = &fade[..setup_end];
        let supervision = &fade[fade.find("cx.spawn").unwrap()..];

        assert_eq!(
            fade.matches("set_transport_gain_target(target_gain)")
                .count(),
            1
        );
        let play_branch = setup.find("if playing {").unwrap();
        assert!(!setup[..play_branch].contains("engine.play();"));
        assert_eq!(setup.matches("engine.play();").count(), 1);
        assert_eq!(supervision.matches("engine.set_volume").count(), 1);
        assert!(supervision.contains("pause_silently(|| engine.set_volume(0.0)"));
    }

    #[test]
    fn user_fade_settle_timeout_has_room_for_the_sample_ramp() {
        assert!(USER_FADE_SETTLE_TIMEOUT > USER_FADE_DURATION);
        assert_eq!(
            USER_FADE_SETTLE_TIMEOUT,
            USER_FADE_DURATION + Duration::from_millis(250)
        );
    }

    #[test]
    fn quality_label_combines_format_and_computed_bitrate() {
        assert_eq!(
            quality_label("FLAC", None, Some(1_000_000), Some(Duration::from_secs(10))),
            Some("FLAC 800kbps".into())
        );
        assert_eq!(
            quality_label("MP3", None, Some(161_250), Some(Duration::from_secs(10))),
            Some("MP3 128kbps".into())
        );
        assert_eq!(
            quality_label("MP3", None, None, Some(Duration::from_secs(10))),
            Some("MP3".into())
        );
        assert_eq!(
            quality_label("MP3", None, Some(100), Some(Duration::from_secs(0))),
            Some("MP3".into())
        );
        assert_eq!(
            quality_label("MP3", None, Some(0), Some(Duration::from_secs(5))),
            Some("MP3".into())
        );
        assert_eq!(quality_label("MP3", None, Some(100), None), None);
        assert_eq!(
            quality_label(
                player_format_label(super::super::resolver::AudioFormat::M4a),
                Some(160),
                None,
                None
            ),
            Some("AAC 160kbps".into())
        );
        assert_eq!(
            quality_label(
                player_format_label(super::super::resolver::AudioFormat::M4a),
                Some(160),
                Some(100),
                Some(Duration::from_secs(1))
            ),
            Some("AAC 160kbps".into())
        );
    }

    #[test]
    fn deezer_history_changes_only_after_a_successful_completed_listen() {
        assert!(listen_history_changed_on_completion(true, true));
        assert!(!listen_history_changed_on_completion(true, false));
        assert!(!listen_history_changed_on_completion(false, true));
        assert!(!listen_history_changed_on_completion(false, false));
    }

    #[test]
    fn listen_history_completion_requires_the_same_account_scope() {
        assert!(listen_history_scope_matches("account-a", "account-a"));
        assert!(!listen_history_scope_matches("account-a", "account-b"));
    }

    #[test]
    fn background_prefetch_is_preference_gated_and_uses_the_next_track() {
        let mut state = PlaybackState::default();
        let tracks = ["current", "next"].map(|id| PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: id.into(),
            title: id.into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::ZERO,
            explicit: false,
            service_url: String::new(),
        });
        state.replace(tracks.into(), 0);

        assert!(adjacent_track(false, &state).is_none());
        assert_eq!(
            adjacent_track(true, &state).map(|track| track.id.as_str()),
            Some("next")
        );
        state.select(1);
        assert!(adjacent_track(true, &state).is_none());
    }

    #[test]
    fn loading_toggle_cancels_without_start_failure_handling() {
        let source = include_str!("view.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("if self.state.status == PlaybackStatus::Loading"));
        assert!(production.contains("self.cancel_pending_load(cx);"));
        assert!(production.contains("fn cancel_pending_load(&mut self, cx: &mut Context<Self>)"));
        assert!(production.contains("PlaybackStatus::Loading | PlaybackStatus::Paused"));
        assert!(production.contains("PlaybackStatus::Loading | PlaybackStatus::Playing"));
    }

    #[test]
    fn media_play_restarts_ended_playback() {
        assert!(!media_play_should_toggle(PlaybackStatus::Empty));
        assert!(media_play_should_toggle(PlaybackStatus::Loading));
        assert!(media_play_should_toggle(PlaybackStatus::Paused));
        assert!(media_play_should_toggle(PlaybackStatus::Ended));
        assert!(!media_play_should_toggle(PlaybackStatus::Playing));
        assert!(!media_play_should_toggle(PlaybackStatus::Failed));
    }

    #[test]
    fn seek_slider_previews_changes_and_commits_once_on_release() {
        assert_eq!(SEEK_SLIDER_STEP, 0.01);
        assert_eq!(
            seek_slider_action(&SliderEvent::Change(SliderValue::Single(0.42))),
            SeekSliderAction::Preview(0.42)
        );
        assert_eq!(
            seek_slider_action(&SliderEvent::Release(SliderValue::Single(0.42))),
            SeekSliderAction::Commit(0.42)
        );
    }

    #[test]
    fn seek_slider_click_is_a_preview_followed_by_one_commit() {
        let click = [
            seek_slider_action(&SliderEvent::Change(SliderValue::Single(0.75))),
            seek_slider_action(&SliderEvent::Release(SliderValue::Single(0.75))),
        ];
        assert_eq!(
            click,
            [
                SeekSliderAction::Preview(0.75),
                SeekSliderAction::Commit(0.75)
            ]
        );
    }

    #[test]
    fn seek_pointer_release_commits_once() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.5025));
        assert_eq!(interaction.commit(7, 0.5025), Some(0.5025));
        assert_eq!(interaction.commit(7, 0.5025), None);
    }

    #[test]
    fn seek_slider_ignores_range_events() {
        assert_eq!(
            seek_slider_action(&SliderEvent::Change(SliderValue::Range(0.2, 0.8))),
            SeekSliderAction::Ignore
        );
        assert_eq!(
            seek_slider_action(&SliderEvent::Release(SliderValue::Range(0.2, 0.8))),
            SeekSliderAction::Ignore
        );
    }

    #[test]
    fn seek_slider_actions_clamp_to_the_playback_range() {
        assert_eq!(
            seek_slider_action(&SliderEvent::Change(SliderValue::Single(-0.2))),
            SeekSliderAction::Preview(0.0)
        );
        assert_eq!(
            seek_slider_action(&SliderEvent::Release(SliderValue::Single(1.2))),
            SeekSliderAction::Commit(1.0)
        );
    }

    #[test]
    fn stale_release_after_track_transition_is_ignored() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));

        interaction.cancel();

        assert_eq!(interaction.commit(8, 0.75), None);
    }

    #[test]
    fn held_drag_cannot_rearm_until_release() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));
        interaction.cancel();

        assert!(!interaction.preview(8, 0.5));
        assert_eq!(interaction.commit(8, 0.5), None);
        assert!(interaction.preview(8, 0.75));
        assert_eq!(interaction.commit(8, 0.75), Some(0.75));
    }

    #[test]
    fn slider_accessibility_value_commits_only_when_enabled_and_not_previewing() {
        assert_eq!(
            seek_slider_accessibility_commit(SliderValue::Single(0.31), Some(0.2), None),
            Some(0.31)
        );
        assert_eq!(
            seek_slider_accessibility_commit(SliderValue::Single(0.31), Some(0.2), Some(0.31)),
            None
        );
        assert_eq!(
            seek_slider_accessibility_commit(SliderValue::Single(0.31), None, None),
            None
        );
    }

    #[test]
    fn cancelled_pointer_change_is_suppressed_before_accessibility_inference() {
        let mut interaction = SeekSliderInteraction::default();
        let value = SliderValue::Single(0.6);
        interaction.record_pointer_change(value);
        interaction.preview(7, 0.6);
        interaction.cancel();

        let should_seek = !interaction.suppress_pointer_change(value)
            && seek_slider_accessibility_commit(value, Some(0.2), interaction.preview_fraction)
                .is_some();
        assert!(!should_seek);
    }

    #[test]
    fn seamless_cancel_stays_blocked_until_release() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));
        interaction.cancel();

        assert!(!interaction.preview(8, 0.5));
        assert_eq!(interaction.commit(8, 0.5), None);
        assert!(interaction.preview(8, 0.75));
        assert_eq!(interaction.commit(8, 0.75), Some(0.75));
    }

    #[test]
    fn disabled_cycle_allows_the_first_new_click() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));
        interaction.set_control_enabled(false);
        assert_eq!(interaction.preview_fraction, None);
        assert!(!interaction.preview(7, 0.5));

        interaction.set_control_enabled(true);
        interaction.begin_new_pointer_interaction();
        assert!(interaction.preview(8, 0.75));
        assert_eq!(interaction.commit(8, 0.75), Some(0.75));
    }

    #[test]
    fn held_drag_across_disabled_cycle_stays_rejected_without_new_pointer_down() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));
        interaction.set_control_enabled(false);
        interaction.set_control_enabled(true);

        assert!(!interaction.preview(8, 0.5));
        assert_eq!(interaction.commit(8, 0.5), None);
    }

    #[test]
    fn fresh_pointer_down_after_disabled_cycle_rearms_the_first_click() {
        let mut interaction = SeekSliderInteraction::default();
        assert!(interaction.preview(7, 0.25));
        interaction.set_control_enabled(false);
        interaction.set_control_enabled(true);
        interaction.begin_new_pointer_interaction();

        assert!(interaction.preview(8, 0.75));
        assert_eq!(interaction.commit(8, 0.75), Some(0.75));
    }

    #[test]
    fn progressive_download_progress_updates_buffered_time_after_load() {
        let mut state = PlaybackState::default();
        let track = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: "progressive".into(),
            title: "Progressive".into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::ZERO,
            explicit: false,
            service_url: String::new(),
        };
        let generation = state.replace(vec![track], 0).unwrap();
        assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
        assert_eq!(state.buffered, Duration::ZERO);

        assert!(apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 25,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(5));
        assert!(apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 75,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(15));
        assert!(!apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 25,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(15));
        assert!(!apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 75,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));

        let mut full = PlaybackState::default();
        let generation = full
            .replace(vec![state.current().unwrap().clone()], 0)
            .unwrap();
        assert!(full.loaded_fully_buffered(generation, Some(Duration::from_secs(20))));
        assert_eq!(full.buffered, Duration::from_secs(20));
    }

    #[test]
    fn cached_initial_progress_marks_the_buffer_full_without_regressing() {
        let mut progress = DownloadProgress {
            generation: 7,
            downloaded: 24,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        };
        progress.adopt_initial(DownloadProgress {
            generation: 7,
            downloaded: 100,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: true,
        });
        assert!(progress.fully_buffered);
        assert_eq!(progress.fraction(), Some(1.0));

        progress.adopt_initial(DownloadProgress {
            generation: 7,
            downloaded: 25,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        });
        assert_eq!(progress.downloaded, 100);
        assert!(progress.fully_buffered);
    }

    #[test]
    fn explicit_buffered_fraction_updates_without_a_byte_total() {
        let mut state = PlaybackState::default();
        let track = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::SoundCloud,
            id: "hls".into(),
            title: "HLS".into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(20),
            explicit: false,
            service_url: String::new(),
        };
        let generation = state.replace(vec![track], 0).unwrap();
        assert!(state.loaded_progressive(generation, Some(Duration::from_secs(20))));
        assert!(apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 100,
                total: None,
                buffered_fraction: Some(0.25),
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(5));
        assert!(!apply_download_progress(
            &mut state,
            DownloadProgress {
                generation,
                downloaded: 80,
                total: None,
                buffered_fraction: Some(0.1),
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(5));
    }

    #[test]
    fn completed_download_size_requires_an_exact_completed_total() {
        assert_eq!(
            completed_download_size(DownloadProgress {
                generation: 1,
                downloaded: 42,
                total: Some(42),
                buffered_fraction: Some(1.0),
                fully_buffered: true,
            }),
            Some(42)
        );
        assert_eq!(
            completed_download_size(DownloadProgress {
                generation: 1,
                downloaded: 42,
                total: Some(43),
                buffered_fraction: Some(1.0),
                fully_buffered: true,
            }),
            None
        );
        assert_eq!(
            completed_download_size(DownloadProgress {
                generation: 1,
                downloaded: 42,
                total: Some(42),
                buffered_fraction: Some(1.0),
                fully_buffered: false,
            }),
            None
        );
    }

    #[test]
    fn partial_initial_progress_does_not_overwrite_newer_callback_progress() {
        let mut progress = DownloadProgress {
            generation: 7,
            downloaded: 75,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        };
        progress.adopt_initial(DownloadProgress {
            generation: 7,
            downloaded: 25,
            total: Some(100),
            buffered_fraction: None,
            fully_buffered: false,
        });
        assert_eq!(progress.downloaded, 75);
        assert_eq!(progress.total, Some(100));
    }

    #[test]
    fn stale_download_progress_cannot_update_a_new_generation() {
        let mut state = PlaybackState::default();
        let track = |id: &str| PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: id.into(),
            title: id.into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            service_url: String::new(),
        };
        let first = state.replace(vec![track("first")], 0).unwrap();
        assert!(state.loaded_progressive(first, Some(Duration::from_secs(10))));
        assert!(apply_download_progress(
            &mut state,
            DownloadProgress {
                generation: first,
                downloaded: 50,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_secs(5));

        let second = state.replace(vec![track("second")], 0).unwrap();
        assert_ne!(first, second);
        assert_eq!(state.buffered, Duration::ZERO);
        assert!(!apply_download_progress(
            &mut state,
            DownloadProgress {
                generation: first,
                downloaded: 100,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::ZERO);
        assert!(apply_download_progress(
            &mut state,
            DownloadProgress {
                generation: second,
                downloaded: 25,
                total: Some(100),
                buffered_fraction: None,
                fully_buffered: false,
            },
        ));
        assert_eq!(state.buffered, Duration::from_millis(2500));
    }

    #[test]
    fn consecutive_auto_skip_failure_limit_is_five() {
        assert_eq!(super::MAX_CONSECUTIVE_AUTO_SKIPS, 5);
    }

    #[test]
    fn auto_skip_logic_bounds_consecutive_failures() {
        let mut state = PlaybackState::default();
        let tracks: Vec<PlaybackTrack> = (0..10)
            .map(|i| PlaybackTrack {
                downloadable: false,
                progressive: false,
                provider: PlaybackProvider::Deezer,
                id: format!("track-{i}"),
                title: format!("Track {i}"),
                artist: String::new(),
                album: String::new(),
                album_id: String::new(),
                release_date: String::new(),
                artists: Vec::new(),
                artwork: String::new(),
                duration: Duration::from_secs(10),
                explicit: false,
                service_url: String::new(),
            })
            .collect();
        let _gen = state.replace(tracks, 0).unwrap();
        assert_eq!(state.current_id(), Some("track-0"));

        let mut consecutive_failures = 0;
        let mut skipped_count = 0;

        for _ in 0..10 {
            if state.first_upcoming_index().is_some()
                && consecutive_failures < super::MAX_CONSECUTIVE_AUTO_SKIPS
            {
                consecutive_failures += 1;
                if state.next().is_some() {
                    skipped_count += 1;
                }
            }
        }

        assert_eq!(consecutive_failures, 5);
        assert_eq!(skipped_count, 5);
        assert_eq!(state.current_id(), Some("track-5"));
    }

    #[test]
    fn enqueue_actions_auto_play_when_playback_status_is_ended() {
        let mut state = PlaybackState::default();
        let track0 = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: "track-0".into(),
            title: "Track 0".into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            service_url: String::new(),
        };
        let _gen = state.replace(vec![track0], 0).unwrap();
        state.status = PlaybackStatus::Ended;

        let track1 = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: "track-1".into(),
            title: "Track 1".into(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            service_url: String::new(),
        };

        state.insert_queue_additions(vec![track1], true);
        assert_eq!(state.status, PlaybackStatus::Ended);
        assert_eq!(state.queue.len(), 2);
        let _next_gen = state.next().unwrap();
        assert_eq!(state.current_id(), Some("track-1"));
    }
}
