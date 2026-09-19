use super::*;
use crate::playback::engine::OutputSwitch;

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
        let target = resolve_saved_output_target(
            asio_mode,
            output_device.as_deref(),
            asio_driver.as_deref(),
        );
        if let Some(pending) = self.pending_output_target.as_ref() {
            if pending == &target {
                self.queued_output_request = None;
            } else {
                self.queued_output_request = Some((asio_mode, output_device, asio_driver));
            }
            return;
        }
        self.output_switch_epoch = self.output_switch_epoch.wrapping_add(1);
        let epoch = self.output_switch_epoch;
        self.pending_output_target = None;
        let Ok(engine) = self.engine.as_ref() else {
            return;
        };
        if *engine.output_target() == target {
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
        // MiniFuse's ASIO driver opens reliably on the UI thread, where the
        // previous stream is also destroyed. Decode on a worker afterwards.
        let open_asio = if matches!(target, AudioOutputTarget::AsioDriver(_)) && !bridge_asio {
            Some(RodioEngine::open_asio_output_switch(
                target.clone(),
                reload.as_ref(),
            ))
        } else {
            None
        };
        let decode_task = open_asio.as_ref().filter(|result| result.is_ok()).map(|_| {
            let reload = reload.clone();
            self.runtime
                .spawn_blocking(move || RodioEngine::decode_output_reload(reload))
        });
        let task = open_asio.is_none().then(|| {
            self.runtime.spawn_blocking(move || {
                if let Some(driver) = bridge_driver {
                    RodioEngine::prepare_asio_bridge(
                        &driver,
                        bridge_current_device.as_deref(),
                        reload,
                    )
                } else {
                    RodioEngine::prepare_output_switch(target, reload)
                }
            })
        });
        cx.spawn(async move |this, cx| {
            let result = match open_asio {
                Some(Ok(open)) => decode_task
                    .unwrap()
                    .await
                    .map(|source| open.finish(source))
                    .map_err(|_| "The output switch decoder stopped unexpectedly".into()),
                Some(Err(error)) => Err(error),
                None => task.unwrap().await.unwrap_or_else(|_| {
                    Err("The output switch worker stopped unexpectedly".into())
                }),
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
                        let switch = this.engine.as_mut().unwrap().set_output(prepared);
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
