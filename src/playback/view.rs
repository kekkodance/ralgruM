use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{AppContext, Context, Entity, Task, Window};
use gpui_component::slider::{SliderEvent, SliderState, SliderValue};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::{
    diagnostics,
    discord::DiscordPresence,
    downloads::format_bitrate,
    media_control::{MediaRequest, MediaSession},
    settings::AccountState,
};

mod ai_content;
mod automation_control;
mod configuration;
mod helpers;
mod listen_reporting;
mod output_switch;
mod queue_actions;
mod standby_flow;
use helpers::*;
use output_switch::{OutputResume, restore_output_state};

use super::state::{ExactQueueAppend, ExactQueueAppendTicket};
use super::{
    AudioCache, ContentPreferences, DeezerFlowKind, ExtensionApply, PlaybackContext,
    PlaybackProvider, PlaybackState, PlaybackStatus, PlaybackTrack, PreviousAction,
    QueueExtensionTicket, ResolvedTrackInfo, RightSidebar, asio_drivers,
    deezer_extension::{self, ExtensionObserverKey},
    engine::{AudioEngine, AudioOutputTarget, RodioEngine, SeekCompletion, SeekOutcome},
    fade::{
        USER_FADE_FRAME, USER_FADE_SETTLE_TIMEOUT, UserFadeSupervisor, UserToggleFadeDecision,
        user_toggle_fade_decision,
    },
    listen_history::{
        DeezerListenSession, ListenHistorySignal, SoundCloudListenReport, deezer_next_media,
    },
    progressive::TimelineSuffixState,
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
    automation: super::automation::AutomationState,
    automation_task: Option<Task<()>>,
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
    output_switch_epoch: u64,
    pending_output_target: Option<AudioOutputTarget>,
    asio_bridge_origin: Option<AudioOutputTarget>,
    queued_output_request: Option<(bool, Option<String>, Option<String>)>,
    pending_output_resume: Option<OutputResume>,
    ai_client: Result<crate::search::SearchClient, String>,
    ai_enrichment_abort: Option<tokio::task::AbortHandle>,
    ai_enrichment_id: u64,
    ai_warning_shown: bool,
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

fn apply_buffered_progress(
    state: &mut PlaybackState,
    progress: Option<DownloadProgress>,
    suffix: Option<TimelineSuffixState>,
) -> bool {
    if !matches!(
        state.status,
        PlaybackStatus::Loading | PlaybackStatus::Playing | PlaybackStatus::Paused
    ) {
        return false;
    }
    let mut changed = false;
    if progress.is_some_and(|update| apply_download_progress(state, update)) {
        changed = true;
    }
    let suffix_changed = match suffix {
        Some(suffix) => apply_timeline_suffix_progress(state, &suffix),
        None => state.suffix_buffered.take().is_some(),
    };
    if suffix_changed {
        changed = true;
    }
    changed
}

/// Keep the suffix as a separate range so a missing middle section is never
/// painted as downloaded by the single contiguous front-buffer indicator.
fn apply_timeline_suffix_progress(state: &mut PlaybackState, suffix: &TimelineSuffixState) -> bool {
    let next = if state.duration.is_zero() || suffix.total == 0 || suffix.written == 0 {
        None
    } else {
        let fraction = (suffix.written as f32 / suffix.total as f32).clamp(0.0, 1.0);
        let remaining = state.duration.saturating_sub(suffix.base);
        Some((
            suffix.base,
            (suffix.base + remaining.mul_f32(fraction)).min(state.duration),
        ))
    };
    if state.suffix_buffered == next {
        return false;
    }
    state.suffix_buffered = next;
    true
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

/// Resolves the saved output selection into an engine target. ASIO mode
/// plays through the saved driver, falling back to the first installed
/// one. A saved WASAPI endpoint is checked when its stream is opened, so
/// output changes do not enumerate devices on the UI thread.
fn resolve_saved_output_target(
    asio_mode: bool,
    output_device: Option<&str>,
    asio_driver: Option<&str>,
) -> AudioOutputTarget {
    let target = if asio_mode {
        let registry_drivers = asio_drivers::list_registry_asio_drivers();
        asio_drivers::effective_target(true, asio_driver, &registry_drivers, output_device, &[])
    } else {
        output_device
            .map(|name| AudioOutputTarget::Device(name.to_owned()))
            .unwrap_or(AudioOutputTarget::SystemDefault)
    };
    let saved_name = if asio_mode {
        asio_driver
    } else {
        output_device
    };
    if let Some(name) = saved_name
        && target == AudioOutputTarget::SystemDefault
    {
        diagnostics::event(
            "WARN",
            format!(
                "the saved {} \"{name}\" is unavailable, using the system default",
                if asio_mode {
                    "ASIO driver"
                } else {
                    "audio output device"
                }
            ),
        );
    }
    target
}

/// Saved output routing the playback engine starts with: the regular
/// output device choice plus the ASIO mode and driver selection.
pub(crate) struct AudioOutputSettings {
    pub(crate) output_device: Option<String>,
    pub(crate) asio_mode: bool,
    pub(crate) asio_driver: Option<String>,
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
        audio_output: AudioOutputSettings,
        seamless_playback: bool,
        record_deezer_plays: bool,
        listen_history_changed: Arc<ListenHistorySignal>,
        playback_preferences: (f32, bool, super::state::RepeatMode, bool),
        content_preferences: ContentPreferences,
        sidebar_preferences: (bool, RightSidebar),
        media: MediaSession,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut state = PlaybackState::default();
        state.restore_volume(playback_preferences.0, playback_preferences.1);
        state.set_repeat_mode(playback_preferences.2);
        state.set_shuffle_enabled(playback_preferences.3);
        let _ = state.set_skip_explicit(content_preferences.block_explicit);
        let _ = state.set_block_ai(content_preferences.block_ai);
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
        let target = resolve_saved_output_target(
            audio_output.asio_mode,
            audio_output.output_device.as_deref(),
            audio_output.asio_driver.as_deref(),
        );
        let engine = match RodioEngine::new(target.clone()) {
            Ok(engine) => Ok(engine),
            // An unloadable saved endpoint must not brick the player.
            Err(error) => match target {
                AudioOutputTarget::AsioDriver(name) | AudioOutputTarget::Device(name) => {
                    diagnostics::event(
                        "WARN",
                        format!(
                            "the audio output \"{name}\" could not be opened at startup, \
                             using the system default: {error}"
                        ),
                    );
                    RodioEngine::new(AudioOutputTarget::SystemDefault)
                }
                AudioOutputTarget::SystemDefault => Err(error),
            },
        };
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
            automation: super::automation::AutomationState::default(),
            automation_task: None,
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
            output_switch_epoch: 0,
            pending_output_target: None,
            asio_bridge_origin: None,
            queued_output_request: None,
            pending_output_resume: None,
            ai_client: crate::search::SearchClient::new().map_err(|error| error.message),
            ai_enrichment_abort: None,
            ai_enrichment_id: 0,
            ai_warning_shown: false,
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
        if self
            .pending_output_resume
            .is_some_and(|resume| resume.generation == generation)
        {
            self.pending_output_resume = None;
        }
        self.state.fail(generation, error.to_owned());
        self.sync_discord();
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Error,
            "Playback error",
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
        let restoring_output = self
            .pending_output_resume
            .is_some_and(|resume| resume.generation == generation);
        if !restoring_output {
            self.pending_output_resume = None;
            self.finish_deezer_listen(cx);
        }
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
            if let Ok(mut progress) = progress_target.lock()
                && progress.generation == generation
                && !progress.fully_buffered
            {
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
                                let restoring_output = this
                                    .pending_output_resume
                                    .is_some_and(|resume| resume.generation == generation);
                                let duration =
                                    engine.load(prepared, this.state.volume, !restoring_output);
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
                                this.reapply_automation_after_load(cx);
                                this.restore_output_position(generation, cx);
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
                                if !restoring_output {
                                    this.begin_listen_reporting(
                                        &history_resolver,
                                        &history_track,
                                        history_arl,
                                        cx,
                                    );
                                }
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
        // A manual pause keeps the current automation gain while the
        // transport fades down, avoiding a brief volume jump. Manual play
        // resets it before the transport fades back in.
        self.release_automation_for_manual_control(self.state.status != PlaybackStatus::Playing);
        if self.state.status == PlaybackStatus::Loading {
            self.cancel_pending_load(cx);
            return;
        }
        if self.state.status == PlaybackStatus::Ended
            && let Some(index) = self.state.current_index
            && let Some(generation) = self.state.select(index)
        {
            self.start(generation, cx);
            return;
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
        self.pending_output_resume = None;
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
        if self.state.abandon_pending_load() {
            cx.notify();
            return;
        }
        if self.state.stop_loading() {
            self.sync_discord();
            cx.notify();
        }
    }

    /// Open the player bar blank while a queued playback source is still
    /// being fetched, e.g. a smart mix page load. Current audio stops so
    /// the blank bar is honest about the state.
    pub(crate) fn begin_pending_load(&mut self, cx: &mut Context<Self>) {
        self.pending_output_resume = None;
        self.cancellation.cancel();
        self.cache.cancel();
        self.cancel_user_fade();
        self.cancel_seek_slider_interaction();
        self.standby = StandbyPhase::Idle;
        self.loading_from_cache = false;
        self.resolved_quality = None;
        self.current_audio_info = None;
        self.reset_extension_state();
        self.reset_download_progress();
        if let Ok(engine) = self.engine.as_mut() {
            engine.stop();
        }
        if self.state.begin_pending_load() {
            self.sync_discord();
            cx.notify();
        }
    }

    /// Close the player bar when a pending source load never produced a
    /// track.
    pub(crate) fn abandon_pending_load(&mut self, cx: &mut Context<Self>) {
        if self.state.abandon_pending_load() {
            cx.notify();
        }
    }

    /// Close a pending player only when it still belongs to the request that
    /// is completing. A newer SmartMix click owns a different queue epoch.
    pub(crate) fn abandon_pending_load_if_epoch(
        &mut self,
        expected_queue_epoch: u64,
        cx: &mut Context<Self>,
    ) {
        if self.state.queue_epoch() == expected_queue_epoch {
            self.abandon_pending_load(cx);
        }
    }

    pub(crate) fn previous(&mut self, cx: &mut Context<Self>) {
        self.consecutive_failures = 0;
        self.cancel_user_fade_and_sync_transport();
        self.cancel_seek_slider_interaction();
        if self.state.status == PlaybackStatus::Ended
            && let Some(index) = self.state.current_index
            && let Some(generation) = self.state.select(index)
        {
            self.start(generation, cx);
            return;
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
            self.commit_standby(*armed, cx);
            return;
        }
        self.finish_deezer_listen(cx);
        if let Some(generation) = self.state.next() {
            self.start(generation, cx);
        } else {
            self.standby = StandbyPhase::Idle;
            if let Ok(engine) = self.engine.as_mut() {
                engine.stop();
            }
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
        self.pending_output_resume = None;
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
            Err(error) => crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Could not seek",
                Some(error.into()),
            ),
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

    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.pending_output_resume = None;
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
        self.pending_output_resume = None;
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
        // Keep the independently advancing front and suffix ranges visible
        // without painting the unfetched gap between them.
        let timeline_suffix = match self.state.status {
            PlaybackStatus::Playing | PlaybackStatus::Paused => self
                .engine
                .as_ref()
                .ok()
                .and_then(|engine| engine.timeline_suffix_state()),
            _ => None,
        };
        let buffered_changed = apply_buffered_progress(&mut self.state, progress, timeline_suffix);
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
                let restore_landed = matches!(
                    deferred_seek,
                    Ok(SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped)
                );
                let restore_failed = deferred_seek.is_err();
                let standby_dropped = match deferred_seek {
                    Ok(SeekOutcome::AppliedStandbyDropped) => true,
                    Ok(_) => false,
                    Err(error) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            "Could not seek",
                            Some(error.into()),
                        );
                        false
                    }
                };
                self.state.position = position;
                if (restore_landed || restore_failed)
                    && let Some(resume) = self
                        .pending_output_resume
                        .filter(|resume| resume.generation == self.state.generation)
                {
                    self.pending_output_resume = None;
                    restore_output_state(
                        &mut self.state,
                        OutputResume {
                            position: if restore_failed {
                                position
                            } else {
                                resume.position
                            },
                            ..resume
                        },
                    );
                    self.sync_transport_after_fade_cancel();
                    self.sync_discord();
                }
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

#[cfg(test)]
#[path = "view_tests.rs"]
mod tests;
