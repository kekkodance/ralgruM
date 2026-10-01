use super::*;
use crate::playback::engine::{OpenOutputSwitch, OutputReloadSpec, OutputSwitch};

#[derive(Clone, Copy)]
pub(super) struct OutputResume {
    pub(super) generation: u64,
    pub(super) position: Duration,
    pub(super) playing: bool,
}

pub(super) fn restore_output_state(state: &mut PlaybackState, resume: OutputResume) {
    state.seek(resume.position);
    state.status = if resume.playing {
        PlaybackStatus::Playing
    } else {
        PlaybackStatus::Paused
    };
}

fn needs_asio_bridge(current: &AudioOutputTarget, target: &AudioOutputTarget) -> bool {
    let AudioOutputTarget::AsioDriver(_) = target else {
        return false;
    };
    if current == target {
        return false;
    }
    match current {
        AudioOutputTarget::AsioDriver(_) => true,
        AudioOutputTarget::Device(_) | AudioOutputTarget::SystemDefault => false,
    }
}

/// Polls of the 250ms playback loop before a wake-stalled output counts as
/// wedged; four seconds cover slow stream opens without firing otherwise.
const WAKE_STALL_GRACE_POLLS: u32 = 16;
/// Retries of the selected output after a wake before playback pauses.
const WAKE_OUTPUT_RETRIES: u32 = 10;
/// Delay between wake retries, giving USB audio time to re-enumerate. USB
/// interfaces can take half a minute to come back after a system sleep.
const WAKE_RETRY_DELAY: Duration = Duration::from_secs(3);

impl PlaybackModel {
    pub(super) fn restore_output_position(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(resume) = self
            .pending_output_resume
            .filter(|resume| resume.generation == generation)
        else {
            return;
        };
        let Some(engine) = self.engine.as_mut().ok() else {
            self.pending_output_resume = None;
            return;
        };
        engine.pause();
        let outcome = if resume.position.is_zero() {
            Ok(SeekOutcome::Applied)
        } else {
            engine.seek_for_output_restore(resume.position.min(self.state.duration))
        };
        let completion = engine.take_seek_completion();
        let fallback_position = engine.position();
        match outcome {
            Ok(SeekOutcome::Applied | SeekOutcome::AppliedStandbyDropped) => {
                self.pending_output_resume = None;
                restore_output_state(&mut self.state, resume);
                self.sync_transport_after_fade_cancel();
            }
            Ok(SeekOutcome::Deferred) => {
                self.state.seek(resume.position);
                self.state.status = PlaybackStatus::Paused;
                self.arm_seek_completion(completion, cx);
            }
            Err(error) => {
                self.pending_output_resume = None;
                restore_output_state(
                    &mut self.state,
                    OutputResume {
                        position: fallback_position,
                        ..resume
                    },
                );
                self.sync_transport_after_fade_cancel();
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Error,
                    "Could not restore playback position",
                    Some(error.into()),
                );
            }
        }
        self.sync_discord();
    }

    /// Moves playback onto the saved output selection. Imports and repeat
    /// changes leave the stream alone when it is already on that target.
    pub(crate) fn set_audio_output(
        &mut self,
        asio_mode: bool,
        output_device: Option<String>,
        asio_driver: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.set_audio_output_with_reason(asio_mode, output_device, asio_driver, false, cx);
    }

    /// Reinitializes the audio output after the system wakes from sleep. A
    /// sleeping OS can leave an ASIO driver's buffer-switch interrupt dead
    /// while every layer above still reports the same target, so the switch
    /// must be forced even when the saved target matches the engine's. The
    /// output must never change: a failed reopen retries the same target,
    /// and a backend that still refuses to advance pauses instead of
    /// migrating playback to another device.
    /// Releases the audio output before the system sleeps. The teardown must
    /// run to completion while the driver and the hardware still work: a
    /// surviving callback would fire into dead hardware, and the shared ASIO
    /// stream slot must be cleared so the wake path performs a genuinely
    /// fresh open. The reference implementation does this synchronously in
    /// its suspend handler; deferring the drop lets Windows freeze threads
    /// mid-DLL-exit, which wedges the driver for the rest of the process.
    /// PBT_APMSUSPEND handlers are allowed to block briefly, and a clean
    /// ASIOExit plus COM Release on live hardware takes milliseconds.
    pub(crate) fn suspend_audio_output_for_sleep(&mut self, cx: &mut Context<Self>) {
        self.wake_stall_probe = None;
        if let Ok(mut engine) =
            std::mem::replace(&mut self.engine, Err("released for sleep".into()))
        {
            engine.recycle_for_driver_reload();
            let streams = engine.take_asio_streams_for_suspend();
            // Blocking: the COM release and ASIOExit must complete on the
            // ASIO owner thread before the OS freezes all threads, or the
            // Apartment-threaded driver DLL is left half-exited and every
            // later load fails until restart.
            crate::playback::asio_thread::drop_asio_streams(streams);
        }
        // Silence the transport projection until the wake restores playback.
        if self.state.toggle().is_some_and(|playing| !playing) {
            self.cancel_user_fade();
            self.sync_discord();
        }
        cx.notify();
    }

    pub(crate) fn reinitialize_audio_output_after_wake(
        &mut self,
        asio_mode: bool,
        output_device: Option<String>,
        asio_driver: Option<String>,
        cx: &mut Context<Self>,
    ) {
        // A wake and the driver's own reset request can both arrive while a
        // recovery switch is already in flight. Recreating the engine then
        // would tear down the very session the pending switch is rebuilding,
        // and the overlapping teardown and load crash inside the driver.
        // One recovery at a time; the in-flight switch completes or fails
        // on its own and re-arms the retry chain if needed.
        if self.pending_output_target.is_some() || self.wake_stall_probe.is_some() {
            return;
        }
        self.wake_retries_left = WAKE_OUTPUT_RETRIES;
        if asio_driver.is_some() {
            // The slept-through ASIO session's callback keeps firing into
            // half-revived hardware, which is the clicking heard after a
            // wake. Recycle the engine immediately so every callback is
            // removed before anything else runs; the retry loop rebuilds a
            // live session once the driver returns.
            if let Ok(engine) = self.engine.as_mut() {
                engine.recycle_for_driver_reload();
            }
            self.engine = Err("The audio output is being reloaded after the wake".into());
            // No engine means no audio can advance. Pause the transport
            // right away so the Discord presence and the media session do
            // not keep projecting the track forward while the position is
            // frozen.
            if self.state.toggle().is_some_and(|playing| !playing) {
                self.cancel_user_fade();
                self.sync_discord();
            }
        } else if let Ok(engine) = self.engine.as_mut() {
            engine.mark_stream_slept_through();
        }
        self.wake_stall_probe = Some(WakeStallProbe {
            last_position: self.state.position,
            poll_count: 0,
        });
        // USB interfaces need 1.5-3 seconds after Windows wake to negotiate;
        // the first reopen attempt is delayed so it does not burn instantly.
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(WAKE_RETRY_DELAY).await;
            this.update(cx, |this, cx| {
                this.set_audio_output_with_reason(asio_mode, output_device, asio_driver, true, cx);
            })
            .ok();
        })
        .detach();
    }

    fn set_audio_output_with_reason(
        &mut self,
        asio_mode: bool,
        output_device: Option<String>,
        asio_driver: Option<String>,
        force_reopen: bool,
        cx: &mut Context<Self>,
    ) {
        if self.pending_output_target.is_some() {
            self.queued_output_request = Some((asio_mode, output_device, asio_driver));
            return;
        }
        self.output_switch_epoch = self.output_switch_epoch.wrapping_add(1);
        let epoch = self.output_switch_epoch;
        let saved_device = output_device.clone();
        let saved_driver = asio_driver.clone();
        let task = self.runtime.spawn_blocking(move || {
            resolve_saved_output_target(asio_mode, saved_device.as_deref(), saved_driver.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let Ok(target) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.last_selected_output = Some(target.clone());
                if this.output_switch_epoch == epoch {
                    this.begin_audio_output_switch(
                        target,
                        asio_mode,
                        output_device,
                        asio_driver,
                        epoch,
                        force_reopen,
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_audio_output_switch(
        &mut self,
        target: AudioOutputTarget,
        asio_mode: bool,
        output_device: Option<String>,
        asio_driver: Option<String>,
        epoch: u64,
        force_reopen: bool,
        cx: &mut Context<Self>,
    ) {
        if self.engine.is_err() && force_reopen {
            // The engine was recycled after a wake (its driver object was
            // poisoned). Rebuilding opens the ASIO driver, which blocks for
            // seconds while the device is missing, so the rebuild runs on a
            // worker and this method is re-entered with the result. Without
            // the rebuild, every subsequent switch silently early-returns
            // and the retry chain dies with playback frozen.
            let rebuild_target = target.clone();
            let rebuild_volume = self.state.volume;
            let rebuild_runtime = self.runtime.clone();
            cx.spawn(async move |this, cx| {
                let rebuilt = rebuild_runtime
                    .spawn_blocking(move || rebuild_engine(&rebuild_target, rebuild_volume))
                    .await
                    .unwrap_or_else(|_| None);
                this.update(cx, |this, cx| {
                    match rebuilt {
                        Some(engine) => {
                            this.engine = Ok(engine);
                            if let Some((asio_mode, output_device, asio_driver)) =
                                this.queued_output_request.take()
                            {
                                this.set_audio_output(asio_mode, output_device, asio_driver, cx);
                            } else if let Some(target) = this.last_selected_output.clone() {
                                // Re-enter the switch path with the rebuilt
                                // engine so the source reloads onto the fresh
                                // sink, exactly like a wake retry without the
                                // recycle this time.
                                let (asio_mode, output_device, asio_driver) = match target {
                                    AudioOutputTarget::AsioDriver(name) => (true, None, Some(name)),
                                    AudioOutputTarget::Device(name) => (false, Some(name), None),
                                    AudioOutputTarget::SystemDefault => (false, None, None),
                                };
                                this.set_audio_output_with_reason(
                                    asio_mode,
                                    output_device,
                                    asio_driver,
                                    true,
                                    cx,
                                );
                            }
                        }
                        None => {
                            this.handle_wake_open_failure(
                                "The audio engine could not be rebuilt after the wake".into(),
                                cx,
                            );
                        }
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        self.pending_output_target = None;
        let Ok(engine) = self.engine.as_ref() else {
            return;
        };
        if *engine.output_target() == target && !force_reopen {
            self.sync_transport_after_fade_cancel();
            return;
        }
        let bridge_asio = needs_asio_bridge(engine.output_target(), &target);
        if bridge_asio {
            self.asio_bridge_origin = Some(engine.output_target().clone());
        } else if !matches!(target, AudioOutputTarget::AsioDriver(_)) {
            self.asio_bridge_origin = None;
        }
        let bridge_driver = match &target {
            AudioOutputTarget::AsioDriver(name) if bridge_asio => Some(name.clone()),
            _ => None,
        };
        let bridge_current_device = match engine.output_target() {
            AudioOutputTarget::Device(name) => Some(name.clone()),
            _ => None,
        };
        let reload = engine.output_reload_spec();
        let had_reload = reload.is_some();
        let probe = engine.sink_probe();
        let generation = self.state.generation;
        self.pending_output_target = Some(target.clone());
        self.cancel_user_fade_and_sync_transport();
        // Freeze the old position while the replacement stream and decoder
        // are prepared. The engine remains available to other UI actions.
        if self.state.status == PlaybackStatus::Playing
            && let Ok(engine) = &self.engine
        {
            engine.pause();
        }
        // MiniFuse's ASIO driver opens reliably on the UI thread during an
        // ordinary switch, where the previous stream is also destroyed.
        // Wake-driven reopens go to a blocking worker instead: while the USB
        // interface re-enumerates, the open blocks for seconds, and doing
        // that on the UI thread freezes the whole app across every retry.
        // A freshly rebuilt wake engine already owns a live ASIO session on
        // this driver: opening another against the same process-global
        // driver shares its buffers with parked sessions and kills both.
        // Take the live stream and prepare the switch onto it instead.
        let reused_asio_stream = if matches!(target, AudioOutputTarget::AsioDriver(_))
            && !bridge_asio
            && force_reopen
            && let Ok(engine) = self.engine.as_mut()
            && *engine.output_target() == target
            && engine.sink_empty()
        {
            engine.take_stream_for_switch()
        } else {
            None
        };
        let open_asio_task =
            if matches!(target, AudioOutputTarget::AsioDriver(_)) && !bridge_asio && force_reopen {
                let open_target = target.clone();
                let open_reload = reload.clone();
                Some(self.runtime.spawn_blocking(move || {
                    RodioEngine::open_asio_output_switch(open_target, open_reload.as_ref())
                }))
            } else {
                None
            };
        let open_asio = if matches!(target, AudioOutputTarget::AsioDriver(_))
            && !bridge_asio
            && open_asio_task.is_none()
            && reused_asio_stream.is_none()
        {
            Some(RodioEngine::open_asio_output_switch(
                target.clone(),
                reload.as_ref(),
            ))
        } else {
            None
        };
        let reused_position = reload
            .as_ref()
            .map_or(Duration::ZERO, OutputReloadSpec::position);
        let reused_target = target.clone();
        let decode_task = reused_asio_stream.is_some().then(|| {
            let reload = reload.clone();
            self.runtime
                .spawn_blocking(move || RodioEngine::decode_output_reload(reload))
        });
        let decode_task = decode_task.or_else(|| {
            open_asio.as_ref().filter(|result| result.is_ok()).map(|_| {
                let reload = reload.clone();
                self.runtime
                    .spawn_blocking(move || RodioEngine::decode_output_reload(reload))
            })
        });
        let task_runtime = self.runtime.clone();
        let reload_for_task = reload.clone();
        let task = open_asio.is_none().then(move || {
            task_runtime.spawn_blocking(move || {
                if let Some(driver) = bridge_driver {
                    RodioEngine::prepare_asio_bridge(
                        &driver,
                        bridge_current_device.as_deref(),
                        reload_for_task,
                    )
                } else {
                    RodioEngine::prepare_output_switch(target, reload_for_task)
                }
            })
        });
        let decode_runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = if let Some(stream) = reused_asio_stream {
                decode_task
                    .unwrap()
                    .await
                    .map(|source| {
                        OpenOutputSwitch::new_reused(stream, reused_position, reused_target)
                            .finish(source)
                    })
                    .map_err(|_| "The output switch decoder stopped unexpectedly".into())
            } else {
                match open_asio_task {
                    Some(open_task) => match open_task.await.unwrap_or_else(|_| {
                        Err("The output switch worker stopped unexpectedly".into())
                    }) {
                        Ok(open) => {
                            let decode_reload = reload.clone();
                            decode_runtime
                                .spawn_blocking(move || {
                                    RodioEngine::decode_output_reload(decode_reload)
                                })
                                .await
                                .map(|source| open.finish(source))
                                .map_err(|_| {
                                    "The output switch decoder stopped unexpectedly".into()
                                })
                        }
                        Err(error) => Err(error),
                    },
                    None => match open_asio {
                        Some(Ok(open)) => decode_task
                            .unwrap()
                            .await
                            .map(|source| open.finish(source))
                            .map_err(|_| "The output switch decoder stopped unexpectedly".into()),
                        Some(Err(error)) => Err(error),
                        None => task.unwrap().await.unwrap_or_else(|_| {
                            Err("The output switch worker stopped unexpectedly".into())
                        }),
                    },
                }
            };
            this.update(cx, |this, cx| {
                if this.output_switch_epoch != epoch {
                    return;
                }
                this.pending_output_target = None;
                if let Some((asio_mode, output_device, asio_driver)) =
                    this.queued_output_request.take()
                {
                    drop(result);
                    this.asio_bridge_origin = None;
                    this.set_audio_output(asio_mode, output_device, asio_driver, cx);
                    return;
                }
                let current = this.engine.as_ref().ok();
                let source_changed = generation != this.state.generation
                    || current.is_none_or(|engine| !engine.owns_probe(&probe));
                let position_moved = had_reload
                    && current.is_some_and(|engine| {
                        result.as_ref().is_ok_and(|prepared| {
                            engine.sink_probe().queued() != 0
                                && engine.position().abs_diff(prepared.position())
                                    > Duration::from_millis(10)
                        })
                    });
                if source_changed || position_moved {
                    this.set_audio_output(asio_mode, output_device, asio_driver, cx);
                    return;
                }
                match result {
                    Ok(prepared) => {
                        let position = current.map_or(Duration::ZERO, AudioEngine::position);
                        let playing = this.state.status == PlaybackStatus::Playing;
                        let switch = this
                            .engine
                            .as_mut()
                            .unwrap()
                            .set_output(prepared, force_reopen);
                        this.standby = StandbyPhase::Idle;
                        if switch == OutputSwitch::SourceLost {
                            if let Some(generation) = this.state.reload_current_source() {
                                this.pending_output_resume = Some(OutputResume {
                                    generation,
                                    position,
                                    playing,
                                });
                                this.start(generation, cx);
                            }
                        } else if !bridge_asio {
                            this.sync_transport_after_fade_cancel();
                        }
                        if bridge_asio {
                            this.set_audio_output(asio_mode, output_device, asio_driver, cx);
                        } else {
                            this.asio_bridge_origin = None;
                        }
                    }
                    Err(error) => {
                        diagnostics::event("WARN", format!("audio output switch failed: {error}"));
                        if force_reopen {
                            if this.wake_retries_left > 0 {
                                // A wake can race USB re-enumeration: the saved
                                // output is briefly missing even though it comes
                                // back moments later. Retry the same target
                                // instead of surfacing a failure to the user.
                                this.wake_retries_left -= 1;
                                diagnostics::event(
                                    "WARN",
                                    format!(
                                        "retrying the audio output after the wake ({})",
                                        this.wake_retries_left
                                    ),
                                );
                                this.retry_wake_output(cx);
                                cx.notify();
                                return;
                            }
                            // The selected output never came back after the
                            // wake. Pause on it rather than migrating: the
                            // user picks when and where to continue.
                            this.wake_stall_probe = None;
                            if this.state.toggle().is_some_and(|playing| !playing) {
                                this.cancel_user_fade();
                                this.sync_transport_after_fade_cancel();
                                this.sync_discord();
                            }
                            crate::toast::push_global(
                                cx,
                                crate::toast::ToastKind::Error,
                                "The audio output could not be restored",
                                Some(
                                    "Playback was paused. Pick a working output to \
                                     continue."
                                        .into(),
                                ),
                            );
                            cx.notify();
                            return;
                        }
                        this.sync_transport_after_fade_cancel();
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            "Could not switch output",
                            Some(error.into()),
                        );
                        if let Some(origin) = this.asio_bridge_origin.take() {
                            match origin {
                                AudioOutputTarget::SystemDefault => {
                                    this.set_audio_output(false, None, None, cx)
                                }
                                AudioOutputTarget::Device(name) => {
                                    this.set_audio_output(false, Some(name), None, cx)
                                }
                                AudioOutputTarget::AsioDriver(name) => {
                                    this.set_audio_output(true, None, Some(name), cx)
                                }
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Verifies playback actually advanced after the wake-driven output
    /// reopen. A wedged backend keeps the position frozen even though every
    /// layer above still reports a live stream. The selected output is
    /// never abandoned: retries re-open the same target, and once they run
    /// out playback pauses at the frozen position, waiting for the user
    /// to choose an output.
    pub(super) fn check_wake_stall(&mut self, position: Duration, cx: &mut Context<Self>) -> bool {
        let Some(mut probe) = self.wake_stall_probe else {
            return false;
        };
        if self.state.status != PlaybackStatus::Playing || self.pending_output_target.is_some() {
            return false;
        }
        if position != probe.last_position {
            self.wake_stall_probe = None;
            return false;
        }
        probe.poll_count += 1;
        self.wake_stall_probe = Some(probe);
        if probe.poll_count < WAKE_STALL_GRACE_POLLS {
            return false;
        }
        self.wake_stall_probe = None;
        if self.wake_retries_left > 0 {
            self.wake_retries_left -= 1;
            diagnostics::event(
                "WARN",
                format!(
                    "playback stalled at {}ms after the system wake; retrying the selected \
                     audio output ({})",
                    position.as_millis(),
                    self.wake_retries_left
                ),
            );
            // A stream that plays nothing while unpaused is wedged exactly
            // like a slept-through one: its driver's buffer switch is dead,
            // and dropping it can fault inside the driver. Park it instead
            // of letting the recycle below tear it down on a live path.
            if let Ok(engine) = self.engine.as_mut() {
                engine.mark_stream_slept_through();
            }
            self.retry_wake_output(cx);
            return true;
        }
        diagnostics::event(
            "WARN",
            format!(
                "playback stalled at {}ms after the system wake; pausing on the selected \
                 audio output",
                position.as_millis()
            ),
        );
        crate::toast::push_global(
            cx,
            crate::toast::ToastKind::Error,
            "The audio output stopped responding",
            Some("Playback was paused. Pick a working output to continue.".into()),
        );
        if let Some(playing) = self.state.toggle()
            && playing
        {
            self.cancel_user_fade();
            self.sync_transport_after_fade_cancel();
            self.sync_discord();
            cx.notify();
        }
        // USB interfaces can re-enumerate long after the wake retries are
        // spent. Give the selected output one more chance window the next
        // time playback is requested rather than staying on the dead session.
        self.wake_retries_left = WAKE_OUTPUT_RETRIES;
        true
    }

    /// Handles a failed wake-driven open. The selected output is retried
    /// indefinitely: some interfaces take minutes to re-enumerate their ASIO
    /// driver after a system sleep, and abandoning the retry strands the
    /// app on a dead session until a manual restart. The retries stay quiet
    /// so they never interrupt the user.
    fn handle_wake_open_failure(&mut self, error: String, cx: &mut Context<Self>) {
        diagnostics::event("WARN", format!("audio output switch failed: {error}"));
        if self.wake_retries_left > 0 {
            self.wake_retries_left -= 1;
        } else {
            // Budget spent: pause playback once, then keep polling the
            // output so it recovers on its own when the driver returns.
            self.wake_retries_left = 0;
            self.wake_stall_probe = None;
            if self.state.toggle().is_some_and(|playing| !playing) {
                self.cancel_user_fade();
                self.sync_transport_after_fade_cancel();
                self.sync_discord();
            }
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "The audio output could not be restored",
                Some("Playback was paused. Pick a working output to continue.".into()),
            );
            cx.notify();
        }
        diagnostics::event(
            "WARN",
            format!(
                "retrying the audio output after the wake ({})",
                self.wake_retries_left
            ),
        );
        self.retry_wake_output(cx);
        cx.notify();
    }

    /// Re-arms the wake probe and re-opens the selected output target,
    /// delayed to let USB audio re-enumerate after the wake, and without
    /// ever substituting a different output.
    fn retry_wake_output(&mut self, cx: &mut Context<Self>) {
        self.wake_stall_probe = Some(WakeStallProbe {
            last_position: self.state.position,
            poll_count: 0,
        });
        let target = self
            .engine
            .as_ref()
            .ok()
            .map(|engine| engine.output_target().clone())
            // The engine was dropped by a previous retry's recycle; keep
            // retrying the output the user selected.
            .or_else(|| {
                self.engine
                    .as_ref()
                    .err()
                    .and_then(|_| self.last_selected_output.clone())
            });
        let Some(target) = target else {
            return;
        };
        let is_asio = matches!(target, AudioOutputTarget::AsioDriver(_));
        let (asio_mode, output_device, asio_driver) = match target {
            AudioOutputTarget::AsioDriver(name) => (true, None, Some(name)),
            AudioOutputTarget::Device(name) => (false, Some(name), None),
            AudioOutputTarget::SystemDefault => (false, None, None),
        };
        if is_asio {
            // A slept-through ASIO driver object is poisoned in this process:
            // USB re-enumeration invalidates its session, and every open
            // through it keeps failing while a fresh process succeeds. Drop
            // the entire engine so the last driver handle releases and ASIO
            // exits; the reopen below then performs the full driver load a
            // fresh app launch does.
            if let Ok(engine) = self.engine.as_mut() {
                engine.recycle_for_driver_reload();
            }
            self.engine = Err("The audio output is being reloaded after the wake".into());
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(WAKE_RETRY_DELAY).await;
            this.update(cx, |this, cx| {
                this.set_audio_output_with_reason(asio_mode, output_device, asio_driver, true, cx);
            })
            .ok();
        })
        .detach();
    }
}

/// Builds a fresh engine for the target the way app startup does. Used after
/// a wake recycled the previous engine to unload its poisoned ASIO driver.
fn rebuild_engine(target: &AudioOutputTarget, volume: f32) -> Option<RodioEngine> {
    if let AudioOutputTarget::AsioDriver(name) = target {
        // The previous engine was recycled: the driver was fully unloaded, so
        // its shared stream slot must be cleared or the rebuild would reuse
        // the dead ASIO buffers.
        rodio::cpal::clear_shared_asio_streams(name);
    }
    let engine = match RodioEngine::new(target.clone()) {
        Ok(engine) => engine,
        Err(error) => {
            diagnostics::event(
                "WARN",
                format!("audio engine rebuild after wake failed: {error}"),
            );
            return None;
        }
    };
    engine.set_volume(volume);
    Some(engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_resume_preserves_position_and_pause_intent() {
        let mut state = PlaybackState::default();
        state.duration = Duration::from_secs(180);
        state.status = PlaybackStatus::Loading;
        restore_output_state(
            &mut state,
            OutputResume {
                generation: 7,
                position: Duration::from_secs(43),
                playing: false,
            },
        );
        assert_eq!(state.position, Duration::from_secs(43));
        assert_eq!(state.status, PlaybackStatus::Paused);

        restore_output_state(
            &mut state,
            OutputResume {
                generation: 8,
                position: Duration::from_secs(79),
                playing: true,
            },
        );
        assert_eq!(state.position, Duration::from_secs(79));
        assert_eq!(state.status, PlaybackStatus::Playing);
    }

    #[test]
    fn changing_asio_drivers_requires_releasing_the_old_driver_first() {
        let first = AudioOutputTarget::AsioDriver("first".into());
        let second = AudioOutputTarget::AsioDriver("second".into());
        assert!(needs_asio_bridge(&first, &second));
        assert!(!needs_asio_bridge(&first, &first));
        assert!(!needs_asio_bridge(
            &first,
            &AudioOutputTarget::SystemDefault
        ));
        assert!(!needs_asio_bridge(
            &AudioOutputTarget::Device("Headphones (MiniFuse 2)".into()),
            &AudioOutputTarget::AsioDriver("MiniFuse ASIO Driver".into()),
        ));
        assert!(!needs_asio_bridge(
            &AudioOutputTarget::Device("T24D390 (NVIDIA High Definition Audio)".into()),
            &AudioOutputTarget::AsioDriver("MiniFuse ASIO Driver".into()),
        ));
    }
}
