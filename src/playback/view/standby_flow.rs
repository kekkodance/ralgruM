use super::*;

impl PlaybackModel {
    /// Drives the standby lifecycle from the poll loop: prepares the next
    /// track near the end of the current one and acts as a fallback boundary
    /// detector if the watcher task ever dies.
    pub(super) fn advance_standby(&mut self, cx: &mut Context<Self>) {
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
            || self.state.repeat_mode == super::super::state::RepeatMode::One
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
        let queue_epoch = self.state.queue_epoch();
        self.standby = StandbyPhase::Pending;
        let resolve_track = track.clone();
        let task = self.runtime.spawn(async move {
            let audio = resolver
                .resolve_background(&resolve_track, arl, soundcloud, murglar, cancellation)
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
                this.standby_ready(track, generation, queue_epoch, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn standby_ready(
        &mut self,
        track: PlaybackTrack,
        generation: u64,
        queue_epoch: u64,
        result: Result<(PreparedSource, ResolvedTrackInfo), String>,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.standby, StandbyPhase::Pending) {
            return;
        }
        self.standby = StandbyPhase::Idle;
        let next_track_matches = self
            .next_upcoming_track()
            .is_some_and(|next| next.provider == track.provider && next.id == track.id);
        if generation != self.state.generation
            || queue_epoch != self.state.queue_epoch()
            || !self.seamless_playback
            || !matches!(
                self.state.status,
                PlaybackStatus::Playing | PlaybackStatus::Paused
            )
            || !next_track_matches
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
        self.standby = StandbyPhase::Armed(Box::new(ArmedStandby {
            track,
            generation,
            queue_epoch,
            duration,
            quality,
            audio_info: Some(info),
            probe: probe.clone(),
        }));
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

    pub(super) fn standby_boundary(&mut self, probe: &SinkProbe, cx: &mut Context<Self>) {
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
            queue_epoch: self.state.queue_epoch(),
            armed_queue_epoch: armed.queue_epoch,
            armed_provider: armed.track.provider,
            armed_target: armed.track.id.clone(),
            upcoming_first: self
                .next_upcoming_track()
                .map(|track| (track.provider, track.id)),
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
            standby::BoundaryOutcome::Commit => self.commit_standby(*armed, cx),
        }
    }

    /// Moves model state onto the already playing standby source, firing the
    /// same track-start effects as a regular load.
    pub(super) fn commit_standby(&mut self, armed: ArmedStandby, cx: &mut Context<Self>) {
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
        if !self.state.current().is_some_and(|current| {
            current.provider == armed.track.provider && current.id == armed.track.id
        }) {
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
        self.reapply_automation_after_load(cx);
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
    pub(super) fn use_standby_for_skip(&self) -> bool {
        let StandbyPhase::Armed(armed) = &self.standby else {
            return false;
        };
        matches!(
            self.state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) && self.state.queue_epoch() == armed.queue_epoch
            && self.next_upcoming_track().is_some_and(|track| {
                track.provider == armed.track.provider && track.id == armed.track.id
            })
            && self
                .engine
                .as_ref()
                .is_ok_and(|engine| engine.owns_probe(&armed.probe))
    }

    fn next_upcoming_track(&self) -> Option<PlaybackTrack> {
        self.state
            .first_upcoming_index()
            .and_then(|index| self.state.queue.get(index))
            .filter(|track| !self.state.content_blocked(track))
            .cloned()
    }
}
