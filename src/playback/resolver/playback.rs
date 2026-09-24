use super::*;

use futures::FutureExt;

use crate::playback::resolve_limiter::ResolvePriority;

impl StreamResolver {
    /// Resolve a source for playback, reusing the short-lived remote URL
    /// cache for playback and seamless prefetches. Downloads and metadata
    /// probes deliberately call `resolve_source` directly so they never
    /// populate this cache.
    pub(super) async fn resolve_playback_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        backend: Option<MediaCredentials>,
        cancellation: CancellationToken,
        priority: ResolvePriority,
        refresh: bool,
        expected_cache_epoch: u64,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() || self.resolved_source_cache.epoch() != expected_cache_epoch
        {
            return Err("Playback request cancelled".into());
        }
        let soundcloud_token_available = soundcloud_token.is_some();
        let key =
            Self::resolved_source_cache_key(track, soundcloud_token_available, backend.is_some());
        if refresh {
            if let Some(key) = key.as_deref() {
                self.resolved_source_cache
                    .invalidate_if_epoch(expected_cache_epoch, key);
            }
        } else if let Some(source) = key
            .as_deref()
            .and_then(|key| self.resolved_source_cache.get(key))
        {
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            if backend.is_some() && !source.uses_backend() {
                if let Some(key) = key.as_deref() {
                    self.resolved_source_cache
                        .invalidate_if_epoch(expected_cache_epoch, key);
                }
            } else {
                return Ok(source);
            }
        }
        if !refresh && backend.is_some() {
            let direct_key =
                Self::resolved_source_cache_key(track, soundcloud_token_available, false);
            if let Some(source) = direct_key
                .as_deref()
                .and_then(|key| self.resolved_source_cache.get(key))
            {
                if !source.uses_backend() {
                    if cancellation.is_cancelled()
                        || self.resolved_source_cache.epoch() != expected_cache_epoch
                    {
                        return Err("Playback request cancelled".into());
                    }
                    return Ok(source);
                }
                if let Some(direct_key) = direct_key.as_deref() {
                    self.resolved_source_cache
                        .invalidate_if_epoch(expected_cache_epoch, direct_key);
                }
            }
        }

        let Some(key) = key else {
            // Empty identifiers cannot be placed in a bounded keyed flight.
            // Keep the existing direct behavior for these malformed tracks.
            self.limiter.reserve(priority, &cancellation).await?;
            return self
                .resolve_source(
                    track,
                    deezer_arl,
                    soundcloud_token,
                    backend,
                    cancellation,
                    true,
                )
                .await;
        };

        self.resolve_playback_flight(
            key,
            track.clone(),
            deezer_arl,
            soundcloud_token,
            backend,
            refresh,
            cancellation,
            expected_cache_epoch,
            priority,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_playback_flight(
        &self,
        key: String,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        backend: Option<MediaCredentials>,
        refresh: bool,
        cancellation: CancellationToken,
        expected_cache_epoch: u64,
        priority: ResolvePriority,
    ) -> Result<ResolvedSource, String> {
        let (flight, starts_work) = self
            .source_resolve_flights
            .begin(key.clone(), priority == ResolvePriority::Interactive);
        if starts_work {
            let resolver = self.clone();
            let flights = self.source_resolve_flights.clone();
            let worker_flight = flight.clone();
            tokio::spawn(async move {
                // The shared worker survives one waiter leaving, but the final
                // waiter cancels it. An interactive join also promotes a
                // speculative background reservation before it spends budget.
                let worker_cancellation = worker_flight.cancellation();
                let result = std::panic::AssertUnwindSafe(async {
                    loop {
                        let promoted = worker_flight.priority_changed();
                        if worker_flight.is_interactive() {
                            resolver
                                .limiter
                                .reserve(ResolvePriority::Interactive, &worker_cancellation)
                                .await?;
                            break;
                        }
                        tokio::select! {
                            biased;
                            _ = promoted => continue,
                            result = resolver.limiter.reserve(
                                ResolvePriority::Background,
                                &worker_cancellation,
                            ) => {
                                result?;
                                break;
                            }
                        }
                    }
                    if resolver.resolved_source_cache.epoch() != expected_cache_epoch {
                        return Err("Playback request cancelled".into());
                    }
                    // A different flight can complete while this one waits
                    // for the provider budget. Recheck before any network IO.
                    if !refresh
                        && let Some(source) = resolver.resolved_source_cache.get(&key)
                        && (backend.is_none() || source.uses_backend())
                    {
                        return Ok(source);
                    }
                    let source = resolver
                        .resolve_source(
                            &track,
                            deezer_arl,
                            soundcloud_token.clone(),
                            backend,
                            worker_cancellation,
                            true,
                        )
                        .await?;
                    if let Some(cache_key) = StreamResolver::resolved_source_cache_key_for_source(
                        &track,
                        soundcloud_token.is_some(),
                        &source,
                    ) {
                        resolver.resolved_source_cache.insert_if_epoch(
                            expected_cache_epoch,
                            cache_key,
                            source.clone(),
                        );
                    }
                    Ok(source)
                })
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("The playback source worker stopped unexpectedly".into()));
                flights.finish(&key, &worker_flight, result);
            });
        }

        let result = self
            .source_resolve_flights
            .wait(&flight, &cancellation)
            .await;
        if cancellation.is_cancelled() || self.resolved_source_cache.epoch() != expected_cache_epoch
        {
            return Err("Playback request cancelled".into());
        }
        result
    }

    #[allow(dead_code)]
    pub(crate) async fn resolve(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<ResolvedAudio, String> {
        self.resolve_with_priority(
            track,
            deezer_arl,
            soundcloud_token,
            murglar,
            cancellation,
            progress,
            ResolvePriority::Interactive,
        )
        .await
    }

    pub(crate) async fn resolve_background(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
    ) -> Result<ResolvedAudio, String> {
        self.resolve_with_priority(
            track,
            deezer_arl,
            soundcloud_token,
            murglar,
            cancellation,
            None,
            ResolvePriority::Background,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn resolve_with_priority(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
        priority: ResolvePriority,
    ) -> Result<ResolvedAudio, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let murglar_fallback =
            track.provider == PlaybackProvider::Deezer && murglar.is_some() && deezer_arl.is_some();
        let cache_track_token = match self.cache.as_ref() {
            Some(cache) => Some(cache.track_token(track.provider, &track.id).await),
            None => None,
        };
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                cancellation.clone(),
                priority,
                false,
                source_cache_epoch,
            )
            .await?;
        let mut format = source.format;
        let mut declared_bitrate = source.declared_bitrate;
        let file = tempfile::Builder::new()
            .prefix("ralgrum-playback-")
            .suffix(&format!(".{}", format.extension()))
            .tempfile()
            .map_err(|_| "A temporary playback file could not be created".to_string())?;
        let path = file.path().to_owned();
        let mut source_budget = RetryBudget::default();
        loop {
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            let mut output = File::from_std(
                file.reopen()
                    .map_err(|_| "The playback buffer could not be opened".to_string())?,
            );
            output
                .set_len(0)
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            output
                .seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let result = self
                .download_playback(
                    source,
                    &mut output,
                    &cancellation,
                    progress.clone(),
                    Some(track),
                    cache_track_token.as_ref(),
                )
                .await;
            match result {
                Ok(()) => break,
                Err(error) if error.refresh_source && source_budget.take_source_refresh() => {
                    if cancellation.is_cancelled() {
                        return Err("Playback request cancelled".into());
                    }
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback source refresh track_id={} reason=expired_media budget=1",
                            track.id
                        ),
                    );
                    source = self
                        .resolve_playback_source(
                            track,
                            deezer_arl.clone(),
                            soundcloud_token.clone(),
                            murglar.clone(),
                            cancellation.clone(),
                            priority,
                            true,
                            source_cache_epoch,
                        )
                        .await?;
                    format = source.format;
                    declared_bitrate = source.declared_bitrate;
                }
                Err(error) => return Err(error.message),
            }
        }
        if let Err(error) = validate_audio_output(&path, format).await {
            if !should_fallback_to_direct_deezer(murglar_fallback, &error) {
                return Err(error);
            }
            crate::diagnostics::event(
                "WARN",
                format!("deezer recovery mode=playback source=direct reason={error}"),
            );
            if let Some(key) = Self::resolved_source_cache_key(
                track,
                soundcloud_token.is_some(),
                murglar.is_some(),
            ) {
                self.resolved_source_cache
                    .invalidate_if_epoch(source_cache_epoch, &key);
            }
            let direct = self
                .resolve_deezer(&track.id, deezer_arl.as_ref(), false, &cancellation, false)
                .await?;
            let mut retry_output = File::from_std(
                file.reopen()
                    .map_err(|_| "The playback buffer could not be reopened".to_string())?,
            );
            retry_output
                .set_len(0)
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            retry_output
                .seek(std::io::SeekFrom::Start(0))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let retry_format = direct.format;
            let retry_declared_bitrate = direct.declared_bitrate;
            let direct = ResolvedSource {
                data: direct.data,
                size: direct.size,
                deezer_track_id: direct.deezer_track_id,
                is_soundcloud: direct.is_soundcloud,
                cache_identity: direct.cache_identity,
                format: direct.format,
                format_name: direct.format_name,
                declared_bitrate: direct.declared_bitrate,
            };
            if let Some(key) = Self::resolved_source_cache_key_for_source(
                track,
                soundcloud_token.is_some(),
                &direct,
            ) {
                self.insert_resolved_source_if_current(
                    source_cache_epoch,
                    &cancellation,
                    key,
                    direct.clone(),
                );
            }
            self.download_playback(
                direct,
                &mut retry_output,
                &cancellation,
                progress,
                Some(track),
                cache_track_token.as_ref(),
            )
            .await
            .map_err(|error| error.message)?;
            validate_audio_output(&path, retry_format).await?;
            format = retry_format;
            declared_bitrate = retry_declared_bitrate;
        }
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        Ok(ResolvedAudio {
            path,
            file,
            duration: (!track.duration.is_zero()).then_some(track.duration),
            format,
            declared_bitrate,
        })
    }

    pub(crate) async fn resolve_progressive(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<ResolvedProgressiveAudio, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let cache_track_token = match self.cache.as_ref() {
            Some(cache) => Some(cache.track_token(track.provider, &track.id).await),
            None => None,
        };
        let murglar_fallback =
            track.provider == PlaybackProvider::Deezer && murglar.is_some() && deezer_arl.is_some();
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                cancellation.clone(),
                ResolvePriority::Interactive,
                false,
                source_cache_epoch,
            )
            .await?;
        if source.size == 0
            && let (Some(cache), Some(key)) = (self.cache.as_ref(), cache_key(&source))
            && let Some(total) = cache.known_total(&key).await
        {
            source.size = total;
        }
        let initially_fully_cached = match (self.cache.as_ref(), cache_key(&source)) {
            (Some(cache), Some(key)) if source.size > 0 && source.size <= cache.max_bytes() => {
                cache.is_fully_cached(&key, source.size).await
            }
            _ => false,
        };
        if initially_fully_cached
            && let (Some(cache), Some(key)) = (self.cache.as_ref(), cache_key(&source))
        {
            cache
                .remember_track_with_token(
                    track,
                    &key,
                    (source.size > 0).then_some(source.size),
                    cache_track_token.as_ref(),
                )
                .await;
        }
        let mut fully_cached = initially_fully_cached;
        // One pause gate per track: timeline seeks on this source hold it
        // to stall the front download while the seek suffix is fetched.
        let pause_gate = DownloadPauseGate::new();
        let mut buffer =
            ProgressiveFile::new(source.format, (source.size > 0).then_some(source.size))
                .map_err(|_| "A temporary playback file could not be created".to_string())?;
        buffer.set_pause_gate(pause_gate.clone());
        let mut writer = buffer
            .writer()
            .map_err(|_| "The playback buffer could not be opened".to_string())?;
        let mut reader = buffer
            .reader()
            .map_err(|_| "The playback buffer could not be opened".to_string())?;
        let playback_exposed = Arc::new(tokio::sync::Mutex::new(false));
        let mut worker_cancellation = cancellation.child_token();
        let mut task = self.spawn_progressive_download(
            track.clone(),
            deezer_arl.clone(),
            soundcloud_token.clone(),
            murglar.clone(),
            source.clone(),
            worker_cancellation.clone(),
            progress.clone(),
            writer,
            playback_exposed.clone(),
            cache_track_token.clone(),
            source_cache_epoch,
        );
        let timeline_startup = matches!(&source.data, SourceData::Hls(_))
            || matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
        let minimum = startup_bytes(source.format, (source.size > 0).then_some(source.size));
        let wait = tokio::task::spawn_blocking(move || {
            let result = if timeline_startup {
                reader.wait_until_startup_ready()
            } else {
                reader.wait_until_ready(minimum)
            };
            (reader, result)
        })
        .await;
        let (returned_reader, ready) = match wait {
            Ok(result) => result,
            Err(_) => {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err("The playback worker stopped unexpectedly".into());
            }
        };
        reader = returned_reader;
        let startup_error = match ready {
            Ok(()) => None,
            Err(error)
                if should_recover_timeline_startup_error(
                    timeline_startup,
                    murglar_fallback,
                    cancellation.is_cancelled() || worker_cancellation.is_cancelled(),
                ) =>
            {
                Some(error)
            }
            Err(error) => {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        };

        let (mut format, mut declared_bitrate) = buffer
            .metadata()
            .unwrap_or((source.format, source.declared_bitrate));
        let metadata_minimum = startup_bytes(format, reader.total());
        if !timeline_startup && metadata_minimum > minimum {
            let wait = tokio::task::spawn_blocking(move || {
                let result = reader.wait_until_ready(metadata_minimum);
                (reader, result)
            })
            .await;
            let (returned_reader, ready) = match wait {
                Ok(result) => result,
                Err(_) => {
                    worker_cancellation.cancel();
                    let _ = task.await;
                    return Err("The playback worker stopped unexpectedly".into());
                }
            };
            reader = returned_reader;
            if let Err(error) = ready {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        }
        let prefix_error =
            startup_error.or_else(|| validate_progressive_prefix(buffer.path(), format).err());
        if let Some(error) = prefix_error {
            if !murglar_fallback {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
            worker_cancellation.cancel();
            let (returned_writer, _) = task
                .await
                .map_err(|_| "The playback worker stopped unexpectedly".to_string())?;
            writer = returned_writer;
            if let Some(key) = Self::resolved_source_cache_key(
                track,
                soundcloud_token.is_some(),
                murglar.is_some(),
            ) {
                self.resolved_source_cache
                    .invalidate_if_epoch(source_cache_epoch, &key);
            }
            let direct = self
                .resolve_deezer(&track.id, deezer_arl.as_ref(), false, &cancellation, false)
                .await?;
            source = ResolvedSource {
                data: direct.data,
                size: direct.size,
                deezer_track_id: direct.deezer_track_id,
                is_soundcloud: direct.is_soundcloud,
                cache_identity: direct.cache_identity,
                format: direct.format,
                format_name: direct.format_name,
                declared_bitrate: direct.declared_bitrate,
            };
            if let Some(key) = Self::resolved_source_cache_key_for_source(
                track,
                soundcloud_token.is_some(),
                &source,
            ) {
                self.insert_resolved_source_if_current(
                    source_cache_epoch,
                    &cancellation,
                    key,
                    source.clone(),
                );
            }
            fully_cached = false;
            format = source.format;
            declared_bitrate = source.declared_bitrate;
            writer
                .reset((source.size > 0).then_some(source.size))
                .await
                .map_err(|_| "The playback buffer could not be reset".to_string())?;
            let minimum = startup_bytes(source.format, (source.size > 0).then_some(source.size));
            task = self.spawn_progressive_download(
                track.clone(),
                deezer_arl.clone(),
                soundcloud_token.clone(),
                murglar.clone(),
                source.clone(),
                {
                    worker_cancellation = cancellation.child_token();
                    worker_cancellation.clone()
                },
                progress.clone(),
                writer,
                playback_exposed.clone(),
                cache_track_token.clone(),
                source_cache_epoch,
            );
            let wait = tokio::task::spawn_blocking(move || {
                let result = reader.wait_until_ready(minimum);
                (reader, result)
            })
            .await;
            let (returned_reader, ready) = match wait {
                Ok(result) => result,
                Err(_) => {
                    worker_cancellation.cancel();
                    let _ = task.await;
                    return Err("The playback worker stopped unexpectedly".into());
                }
            };
            reader = returned_reader;
            if let Err(error) = ready {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
            if let Err(error) = validate_progressive_prefix(buffer.path(), format) {
                worker_cancellation.cancel();
                let _ = task.await;
                return Err(error);
            }
        }

        let total = reader
            .total()
            .or_else(|| (source.size > 0).then_some(source.size));
        if let Some((metadata_format, metadata_bitrate)) = buffer.metadata() {
            format = metadata_format;
            declared_bitrate = metadata_bitrate;
        }
        fully_cached = if fully_cached && total == Some(source.size) {
            match (self.cache.as_ref(), cache_key(&source)) {
                (Some(cache), Some(key)) => cache.is_fully_cached(&key, source.size).await,
                _ => false,
            }
        } else {
            false
        };
        let initial_downloaded = if fully_cached {
            total.unwrap_or_default()
        } else {
            reader.written()
        };
        let initial_buffered_fraction = match &source.data {
            SourceData::Hls(descriptor) => descriptor.buffered_fraction(1),
            SourceData::Backend(source) => source.metadata().initial_buffered_fraction,
            SourceData::Remote(_) | SourceData::Inline(_) => None,
        };
        let duration = (!track.duration.is_zero())
            .then_some(track.duration)
            .or_else(|| match &source.data {
                SourceData::Hls(descriptor) => descriptor.duration(),
                SourceData::Backend(source) => source.metadata().duration,
                SourceData::Remote(_) | SourceData::Inline(_) => None,
            });
        let front_buffer = (buffer.path().to_path_buf(), reader.completion());
        let timeline_seek_session: Option<Arc<dyn TimelineSeekSession>> = match &source.data {
            SourceData::Hls(descriptor) => Some(Arc::new(soundcloud_hls::HlsSeekSession::new(
                self.client.clone(),
                (**descriptor).clone(),
                tokio::runtime::Handle::current(),
                cancellation.clone(),
                MAX_AUDIO_SIZE,
                BROWSER_USER_AGENT,
            ))),
            SourceData::Backend(source) => source
                .timeline_seek_session(cancellation.clone())
                .or_else(|| {
                    self.backend_range_seek_session(
                        source,
                        total,
                        duration,
                        &cancellation,
                        pause_gate.clone(),
                        Some(front_buffer.clone()),
                    )
                }),
            SourceData::Remote(url) => self.progressive_range_seek_session(
                url,
                &source,
                total,
                duration,
                &cancellation,
                pause_gate.clone(),
                Some(front_buffer),
            ),
            SourceData::Inline(_) => None,
        };
        if fully_cached && let (Some(progress), Some(total)) = (&progress, total) {
            progress(ProgressUpdate::bytes(total, Some(total)));
        }
        *playback_exposed.lock().await = true;
        let worker = ProgressiveDownload {
            cancellation: Some(worker_cancellation),
            task: Some(task),
        };
        Ok(ResolvedProgressiveAudio {
            reader,
            file: buffer.into_file(),
            duration,
            format,
            total,
            timeline_size_unknown: source.timeline_size_unknown(),
            declared_bitrate,
            initial_downloaded,
            initial_buffered_fraction,
            fully_cached,
            timeline_seek_session,
            worker: Some(worker),
        })
    }

    /// Builds the byte range seek session for a remote progressive source.
    /// Constant bitrate MP3 maps a seek target to a byte offset directly,
    /// while FLAC additionally parses frame headers from the fetched suffix
    /// to discard up to the exact target. The CDN serves the rest of the
    /// file from that offset, so the seek lands while the download is still
    /// running.
    pub(super) fn progressive_range_seek_session(
        &self,
        url: &str,
        source: &ResolvedSource,
        total: Option<u64>,
        duration: Option<Duration>,
        cancellation: &CancellationToken,
        pause: DownloadPauseGate,
        front_buffer: Option<(std::path::PathBuf, ProgressiveCompletion)>,
    ) -> Option<Arc<dyn TimelineSeekSession>> {
        let format = range_seek::RangeSeekFormat::from_audio(source.format)?;
        let total = total.filter(|total| *total > 0)?;
        let duration = duration.filter(|duration| !duration.is_zero())?;
        let resolver = self.clone();
        let url = url.to_string();
        let deezer_track_id = source.deezer_track_id.clone();
        let is_soundcloud = source.is_soundcloud;
        let fetch: range_seek::RangeFetch = Arc::new(move |start, end, fetch_cancellation| {
            let resolver = resolver.clone();
            let url = url.clone();
            let deezer_track_id = deezer_track_id.clone();
            Box::pin(async move {
                resolver
                    .fetch_cached_range(
                        &url,
                        start,
                        end,
                        total,
                        deezer_track_id.as_deref(),
                        is_soundcloud,
                        &fetch_cancellation,
                    )
                    .await
                    .map_err(|error| error.message)
            })
        });
        let session = range_seek::RangeTimelineSession::new(
            format,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            cancellation.clone(),
            pause,
        );
        let session = match front_buffer {
            Some((path, completion)) => session.with_front_buffer(path, completion),
            None => session,
        };
        Some(Arc::new(session))
    }

    /// Builds a byte range seek session for a progressive backend source that
    /// does not provide its own timeline session, such as Deezer MP3 and FLAC
    /// resolved through the murglar backend. Byte ranges go through the
    /// backend source, which widens requests to Deezer stripe boundaries and
    /// decrypts them, so a suffix starting mid-stripe still decrypts
    /// correctly.
    pub(super) fn backend_range_seek_session(
        &self,
        source: &BackendSource,
        total: Option<u64>,
        duration: Option<Duration>,
        cancellation: &CancellationToken,
        pause: DownloadPauseGate,
        front_buffer: Option<(std::path::PathBuf, ProgressiveCompletion)>,
    ) -> Option<Arc<dyn TimelineSeekSession>> {
        let metadata = source.metadata();
        if metadata.timeline {
            return None;
        }
        let format = range_seek::RangeSeekFormat::from_audio(metadata.format)?;
        let total = total.filter(|total| *total > 0)?;
        let duration = duration.filter(|duration| !duration.is_zero())?;
        let backend = source.clone();
        let fetch: range_seek::RangeFetch = Arc::new(move |start, end, fetch_cancellation| {
            let backend = backend.clone();
            Box::pin(async move {
                backend
                    .read_range(start, end, MediaFetchScope::Playback, &fetch_cancellation)
                    .await
                    .map_err(|error| error.message)
            })
        });
        let session = range_seek::RangeTimelineSession::new(
            format,
            fetch,
            total,
            duration,
            tokio::runtime::Handle::current(),
            cancellation.clone(),
            pause,
        );
        let session = match front_buffer {
            Some((path, completion)) => session.with_front_buffer(path, completion),
            None => session,
        };
        Some(Arc::new(session))
    }

    pub(super) fn spawn_progressive_download(
        &self,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        source: ResolvedSource,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
        writer: ProgressiveWriter,
        playback_exposed: Arc<tokio::sync::Mutex<bool>>,
        cache_track_token: Option<CacheTrackToken>,
        source_cache_epoch: u64,
    ) -> tokio::task::JoinHandle<(ProgressiveWriter, Result<(), String>)> {
        let resolver = self.clone();
        tokio::spawn(async move {
            resolver
                .download_progressive(
                    track,
                    deezer_arl,
                    soundcloud_token,
                    murglar,
                    source,
                    cancellation,
                    progress,
                    writer,
                    playback_exposed,
                    cache_track_token,
                    source_cache_epoch,
                )
                .await
        })
    }

    pub(super) async fn download_progressive(
        &self,
        track: PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        mut source: ResolvedSource,
        cancellation: CancellationToken,
        progress: Option<ProgressCallback>,
        mut writer: ProgressiveWriter,
        playback_exposed: Arc<tokio::sync::Mutex<bool>>,
        cache_track_token: Option<CacheTrackToken>,
        source_cache_epoch: u64,
    ) -> (ProgressiveWriter, Result<(), String>) {
        let source_generation = self.cache.as_ref().map(AudioCache::generation);
        let mut source_budget = RetryBudget::default();
        let mut dash_fallback_attempted = false;
        loop {
            if cancellation.is_cancelled() {
                writer.cancel();
                return (writer, Err("Playback request cancelled".into()));
            }
            let exposure_guard = playback_exposed.lock().await;
            if *exposure_guard {
                let message = "The playback source expired after playback started".to_string();
                writer.fail(message.clone());
                return (writer, Err(message));
            }
            writer.set_metadata(source.format, source.declared_bitrate);
            if let Err(error) = writer.reset((source.size > 0).then_some(source.size)).await {
                let message = format!("The playback buffer could not be reset: {error}");
                writer.fail(message.clone());
                return (writer, Err(message));
            }
            drop(exposure_guard);
            let source_is_timeline = matches!(&source.data, SourceData::Hls(_))
                || matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
            let source_is_backend_timeline =
                matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline);
            let unknown_source = source.size == 0 || source_is_timeline;
            let cache_source = unknown_source.then(|| source.clone());
            match self
                .download_playback(
                    source,
                    &mut writer,
                    &cancellation,
                    progress.clone(),
                    Some(&track),
                    cache_track_token.as_ref(),
                )
                .await
            {
                Ok(()) => {
                    if let Err(error) = writer.finish().await {
                        let message =
                            format!("The playback buffer could not be finalized: {error}");
                        writer.fail(message.clone());
                        return (writer, Err(message));
                    }
                    if writer.is_complete()
                        && let Some(progress) = &progress
                        && let Some(total) = writer.total()
                    {
                        progress(ProgressUpdate::complete(total));
                    }
                    if let Some(cache_source) = cache_source.as_ref() {
                        self.persist_unknown_progressive_cache(
                            &track,
                            &writer,
                            cache_source,
                            source_generation,
                            cache_track_token.as_ref(),
                        )
                        .await;
                    }
                    return (writer, Ok(()));
                }
                Err(error) if error.refresh_source && source_budget.take_source_refresh() => {
                    if cancellation.is_cancelled() {
                        writer.cancel();
                        return (writer, Err("Playback request cancelled".into()));
                    }
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback source refresh track_id={} reason=expired_media budget=1",
                            track.id
                        ),
                    );
                    match self
                        .resolve_playback_source(
                            &track,
                            deezer_arl.clone(),
                            soundcloud_token.clone(),
                            murglar.clone(),
                            cancellation.clone(),
                            ResolvePriority::Interactive,
                            true,
                            source_cache_epoch,
                        )
                        .await
                    {
                        Ok(next) => source = next,
                        Err(message) => {
                            if message == "Playback request cancelled" {
                                writer.cancel();
                            } else {
                                writer.fail(message.clone());
                            }
                            return (writer, Err(message));
                        }
                    }
                }
                Err(error) => {
                    let message = error.message;
                    if message == "Playback request cancelled" {
                        writer.cancel();
                        return (writer, Err(message));
                    }
                    let can_fallback_to_direct = !dash_fallback_attempted
                        && source_is_backend_timeline
                        && track.provider == PlaybackProvider::Deezer
                        && deezer_arl.is_some()
                        && murglar.is_some()
                        && !*playback_exposed.lock().await;
                    if can_fallback_to_direct {
                        dash_fallback_attempted = true;
                        if let Some(key) = Self::resolved_source_cache_key(
                            &track,
                            soundcloud_token.is_some(),
                            true,
                        ) {
                            self.resolved_source_cache
                                .invalidate_if_epoch(source_cache_epoch, &key);
                        }
                        match self
                            .resolve_deezer(
                                &track.id,
                                deezer_arl.as_ref(),
                                false,
                                &cancellation,
                                false,
                            )
                            .await
                        {
                            Ok(next) => {
                                source = ResolvedSource::from_remote(next);
                                continue;
                            }
                            Err(fallback_error) => {
                                writer.fail(fallback_error.clone());
                                return (writer, Err(fallback_error));
                            }
                        }
                    }
                    writer.fail(message.clone());
                    return (writer, Err(message));
                }
            }
        }
    }

    pub(super) async fn persist_unknown_progressive_cache(
        &self,
        track: &PlaybackTrack,
        writer: &ProgressiveWriter,
        source: &ResolvedSource,
        source_generation: Option<u64>,
        cache_track_token: Option<&CacheTrackToken>,
    ) {
        let Some(cache) = self.cache.as_ref() else {
            return;
        };
        let Some(key) = cache_key(source) else {
            return;
        };
        let Some(total) = writer
            .total()
            .filter(|total| cacheable_size(*total, cache.max_bytes()).is_some())
        else {
            return;
        };
        let generation = source_generation.unwrap_or_else(|| cache.generation());
        if !cache.is_current(generation) || !cache.is_track_current(cache_track_token).await {
            return;
        }
        if !cache_writer_blocks(cache, &key, total, writer, generation, cache_track_token).await {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "playback cache source={} skipped_unknown_size total={}",
                    playback_source_label(source),
                    total
                ),
            );
            return;
        }
        cache
            .remember_total_with_token(&key, total, generation, cache_track_token)
            .await;
        if cache.is_fully_cached(&key, total).await {
            cache
                .remember_track_with_token(track, &key, Some(total), cache_track_token)
                .await;
        }
        let _ = cache.prune().await;
    }

    pub(super) async fn fetch_cached_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
        total: u64,
        deezer_track_id: Option<&str>,
        is_soundcloud: bool,
        cancellation: &CancellationToken,
    ) -> Result<Vec<u8>, PlaybackDownloadError> {
        let (request_start, request_end) = aligned_range(start, end, total);
        let response = send_with_retry(
            "media.cache_range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(
                        header::RANGE,
                        format!("bytes={request_start}-{request_end}"),
                    )
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await?;
        validate_media_response_url(response.url(), is_soundcloud)
            .map_err(PlaybackDownloadError::message)?;
        if !response.status().is_success() {
            return Err(PlaybackDownloadError::media_status(
                "audio provider",
                response.status(),
            ));
        }
        let (status, content_length, body) =
            read_response_range(response, request_start, request_end).await?;
        log_response_diagnostics("playback cache range", status, content_length);
        let mut bytes = body;
        if let Some(track_id) = deezer_track_id {
            decrypt_stripes(&mut bytes, track_id, request_start / STRIPE_SIZE as u64)?;
        }
        Ok(trim_range(bytes, start, end, request_start)?)
    }

    pub(super) async fn download_playback<W>(
        &self,
        source: ResolvedSource,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
        catalog_track: Option<&PlaybackTrack>,
        cache_track_token: Option<&CacheTrackToken>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        let source_label = playback_source_label(&source);
        let decryption = if source.deezer_track_id.is_some() {
            "deezer_stripes"
        } else {
            "none"
        };
        let Some(cache) = self.cache.as_ref() else {
            return self
                .download_source_inner(
                    source,
                    output,
                    cancellation,
                    progress,
                    MediaFetchScope::Playback,
                )
                .await;
        };
        let cache_generation = cache.generation();
        let Some(size) = cacheable_size(source.size, cache.max_bytes()) else {
            return self
                .download_source_inner(
                    source,
                    output,
                    cancellation,
                    progress,
                    MediaFetchScope::Playback,
                )
                .await;
        };
        let Some(key) = cache_key(&source) else {
            return self
                .download_source_inner(
                    source,
                    output,
                    cancellation,
                    progress,
                    MediaFetchScope::Playback,
                )
                .await;
        };
        if matches!(&source.data, SourceData::Backend(source) if source.metadata().timeline)
            && !cache.is_fully_cached(&key, size).await
        {
            return self
                .download_source_inner(
                    source,
                    output,
                    cancellation,
                    progress,
                    MediaFetchScope::Playback,
                )
                .await;
        }
        cache.promote_prefetch(&key, size).await;
        let remote_url = match &source.data {
            SourceData::Remote(url) => Some(url.as_str()),
            SourceData::Backend(_) | SourceData::Hls(_) | SourceData::Inline(_) => None,
        };
        crate::diagnostics::event(
            "INFO",
            format!(
                "playback cache source={source_label} generation={cache_generation} format={} decryption={decryption}",
                source.format.label()
            ),
        );
        output.set_total_hint(Some(size));
        let mut downloaded = 0_u64;
        let mut start = 0;
        while start < size {
            if cancellation.is_cancelled() {
                return Err(PlaybackDownloadError::message("Playback request cancelled"));
            }
            let end = start.saturating_add(BLOCK_SIZE - 1).min(size - 1);
            let path = cache.block_path(&key, size, start, end);
            let expected = (end - start + 1) as usize;
            let bytes = if let Some(bytes) = cache.read(&path, expected).await {
                bytes
            } else {
                let _guard = cache.lock_for(&path).await;
                if let Some(bytes) = cache.read(&path, expected).await {
                    bytes
                } else if let SourceData::Inline(bytes) = &source.data {
                    let bytes = inline_range(bytes, start, end)?;
                    if bytes.len() != expected {
                        return Err(
                            "The inline source did not contain a complete cache block".into()
                        );
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                } else if start == 0
                    && let SourceData::Backend(backend) = &source.data
                    && let Some(prefix_len) = output
                        .progressive_startup_bytes(source.format, Some(size))
                        .filter(|prefix_len| *prefix_len > 0 && *prefix_len < expected as u64)
                {
                    let prefix_end = prefix_len - 1;
                    let prefix = backend
                        .read_range(0, prefix_end, MediaFetchScope::Playback, cancellation)
                        .await?;
                    if prefix.len() != prefix_len as usize {
                        return Err(
                            "The backend source returned an incomplete startup prefix".into()
                        );
                    }
                    self.write_progressive_chunks(
                        output,
                        &prefix,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;

                    let rest_start = prefix_len;
                    let rest = backend
                        .read_range(rest_start, end, MediaFetchScope::Playback, cancellation)
                        .await?;
                    let expected_rest = expected - prefix.len();
                    if rest.len() != expected_rest {
                        return Err("The backend source returned an incomplete cache block".into());
                    }
                    let mut block = prefix;
                    block.extend_from_slice(&rest);
                    cache
                        .write_with_token(&path, &block, cache_generation, cache_track_token)
                        .await;

                    self.write_progressive_chunks(
                        output,
                        &rest,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;
                    start = end + 1;
                    continue;
                } else if let SourceData::Backend(backend) = &source.data {
                    let bytes = backend
                        .read_range(start, end, MediaFetchScope::Playback, cancellation)
                        .await?;
                    if bytes.len() != expected {
                        return Err("The backend source returned an incomplete cache block".into());
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                } else if start == 0
                    && let Some(url) = remote_url
                    && let Some(prefix_len) = output
                        .progressive_startup_bytes(source.format, Some(size))
                        .filter(|prefix_len| *prefix_len > 0 && *prefix_len < expected as u64)
                {
                    let prefix_end = prefix_len - 1;
                    let prefix = self
                        .fetch_cached_range(
                            url,
                            0,
                            prefix_end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    if prefix.len() != prefix_len as usize {
                        return Err(
                            "The audio provider returned an incomplete startup prefix".into()
                        );
                    }
                    self.write_progressive_chunks(
                        output,
                        &prefix,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;

                    let rest_start = prefix_len;
                    let rest = self
                        .fetch_cached_range(
                            url,
                            rest_start,
                            end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    let expected_rest = expected - prefix.len();
                    if rest.len() != expected_rest {
                        return Err("The audio provider returned an incomplete cache block".into());
                    }
                    let mut block = prefix;
                    block.extend_from_slice(&rest);
                    cache
                        .write_with_token(&path, &block, cache_generation, cache_track_token)
                        .await;

                    self.write_progressive_chunks(
                        output,
                        &rest,
                        &mut downloaded,
                        Some(size),
                        cancellation,
                        progress.as_ref(),
                    )
                    .await?;
                    start = end + 1;
                    continue;
                } else {
                    let Some(url) = remote_url else {
                        return Err("The remote source URL was unavailable".into());
                    };
                    let bytes = self
                        .fetch_cached_range(
                            url,
                            start,
                            end,
                            size,
                            source.deezer_track_id.as_deref(),
                            source.is_soundcloud,
                            cancellation,
                        )
                        .await?;
                    if bytes.len() != expected {
                        return Err("The audio provider returned an incomplete cache block".into());
                    }
                    cache
                        .write_with_token(&path, &bytes, cache_generation, cache_track_token)
                        .await;
                    bytes
                }
            };
            self.write_progressive_chunks(
                output,
                &bytes,
                &mut downloaded,
                Some(size),
                cancellation,
                progress.as_ref(),
            )
            .await?;
            start = end + 1;
        }
        if let Some(track) = catalog_track
            && cache.is_fully_cached(&key, size).await
        {
            cache
                .remember_track_with_token(track, &key, Some(size), cache_track_token)
                .await;
        }
        if !cancellation.is_cancelled() {
            let _ = cache.prune().await;
        }
        if cancellation.is_cancelled() {
            return Err(PlaybackDownloadError::message("Playback request cancelled"));
        }
        // Timeline startup normally comes from the backend downloader. A full
        // cache hit bypasses it, so publish readiness only after every byte has
        // been written and flushed successfully.
        output.mark_progressive_startup_ready();
        Ok(())
    }

    pub(super) async fn write_progressive_chunks<W>(
        &self,
        output: &mut W,
        bytes: &[u8],
        downloaded: &mut u64,
        total: Option<u64>,
        cancellation: &CancellationToken,
        progress: Option<&ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput + ?Sized,
    {
        for chunk in bytes.chunks(PROGRESSIVE_WRITE_CHUNK_SIZE) {
            if cancellation.is_cancelled() {
                return Err(PlaybackDownloadError::message("Playback request cancelled"));
            }
            output.write_all(chunk).await.map_err(|_| {
                PlaybackDownloadError::message("The playback buffer could not be written")
            })?;
            output.flush().await.map_err(|_| {
                PlaybackDownloadError::message("The playback buffer could not be finalized")
            })?;
            *downloaded = downloaded.saturating_add(chunk.len() as u64);
            if let Some(progress) = progress {
                progress(ProgressUpdate::bytes(*downloaded, total));
            }
        }
        Ok(())
    }

    pub(crate) async fn prefetch(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
        generation: u64,
    ) -> Result<(), String> {
        let Some(cache) = self.cache.as_ref() else {
            return Ok(());
        };
        let source_cache_epoch = self.resolved_source_cache.epoch();
        let _permit = tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            permit = cache.permit() => permit,
        };
        let Some(_permit) = _permit else {
            return Ok(());
        };
        if !cache.is_current(generation) {
            return Ok(());
        }
        let cache_track_token = cache.track_token(track.provider, &track.id).await;
        let mut source = self
            .resolve_playback_source(
                track,
                deezer_arl,
                soundcloud_token,
                murglar,
                cancellation.clone(),
                ResolvePriority::Background,
                false,
                source_cache_epoch,
            )
            .await?;
        let Some(key) = cache_key(&source) else {
            return Ok(());
        };
        if source.size == 0
            && let Some(total) = cache.known_total(&key).await
        {
            source.size = total;
        }
        if source.size == 0 {
            let job_key = AudioCache::job_key(&key, 0);
            if !cache.begin_job(job_key.clone()).await {
                return Ok(());
            }
            let result = self
                .prefetch_unknown_source(
                    cache,
                    &key,
                    &source,
                    &cancellation,
                    generation,
                    &cache_track_token,
                )
                .await;
            cache.finish_job(&job_key).await;
            if result.is_ok() && cache.is_current(generation) {
                let _ = cache.prune().await;
            }
            return result;
        }
        let source_label = playback_source_label(&source);
        let decryption = if source.deezer_track_id.is_some() {
            "deezer_stripes"
        } else {
            "none"
        };
        let Some(size) = cacheable_size(source.size, cache.max_bytes()) else {
            return Ok(());
        };
        let Some((first_start, first_end)) = prefetch_range(size) else {
            return Ok(());
        };
        let first_path = cache.block_path(&key, size, first_start, first_end);
        let first_expected = (first_end - first_start + 1) as usize;
        if cache.read(&first_path, first_expected).await.is_none()
            && !cache
                .mark_prefetch(&key, size, generation, Some(&cache_track_token))
                .await
        {
            return Ok(());
        }
        crate::diagnostics::event(
            "INFO",
            format!(
                "playback prefetch cache source={source_label} generation={generation} format={} decryption={decryption}",
                source.format.label()
            ),
        );
        let job_key = AudioCache::job_key(&key, size);
        if !cache.begin_job(job_key.clone()).await {
            return Ok(());
        }
        let result = self
            .prefetch_blocks(
                cache,
                &key,
                &source.data,
                size,
                source.deezer_track_id.as_deref(),
                source.is_soundcloud,
                &cancellation,
                generation,
                &cache_track_token,
            )
            .await;
        cache.finish_job(&job_key).await;
        if result.is_ok() && cache.is_current(generation) {
            let _ = cache.prune().await;
        }
        result
    }

    pub(super) async fn prefetch_unknown_source(
        &self,
        cache: &AudioCache,
        key: &str,
        source: &ResolvedSource,
        cancellation: &CancellationToken,
        generation: u64,
        cache_track_token: &CacheTrackToken,
    ) -> Result<(), String> {
        if let SourceData::Backend(backend) = &source.data {
            let Some(total) = backend
                .probe_size(MediaFetchScope::Background, cancellation)
                .await
            else {
                return Ok(());
            };
            if total == 0 || total > cache.max_bytes() || total > MAX_AUDIO_SIZE {
                return Ok(());
            }
            let Some((start, end)) = prefetch_range(total) else {
                return Ok(());
            };
            let expected = (end - start + 1) as usize;
            let bytes = backend
                .read_range(start, end, MediaFetchScope::Background, cancellation)
                .await
                .map_err(|error| error.message)?;
            if bytes.len() != expected {
                return Err("The backend source returned an incomplete prefetch range".into());
            }
            cache
                .remember_total_with_token(key, total, generation, Some(cache_track_token))
                .await;
            let path = cache.block_path(key, total, start, end);
            if cache.read(&path, expected).await.is_none() {
                if !cache
                    .mark_prefetch(key, total, generation, Some(cache_track_token))
                    .await
                {
                    return Ok(());
                }
                cache
                    .write_with_token(&path, &bytes, generation, Some(cache_track_token))
                    .await;
            }
            return Ok(());
        }
        let SourceData::Remote(url) = &source.data else {
            return Ok(());
        };
        let response = send_with_retry(
            "media.prefetch_range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(
                        header::RANGE,
                        format!("bytes=0-{}", BLOCK_SIZE.saturating_sub(1)),
                    )
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await?;
        validate_media_response_url(response.url(), source.is_soundcloud)?;
        let status = response.status();
        if !status.is_success() {
            return Err(PlaybackDownloadError::media_status("audio provider", status).message);
        }
        let total = content_range_total(response.headers())
            .or_else(|| {
                (status == StatusCode::OK)
                    .then_some(response.content_length())
                    .flatten()
            })
            .unwrap_or_default();
        if total == 0 || total > cache.max_bytes() || total > MAX_AUDIO_SIZE {
            return Ok(());
        }
        let Some((start, end)) = prefetch_range(total) else {
            return Ok(());
        };
        let expected = (end - start + 1) as usize;
        let (_, _, mut bytes) = read_response_range(response, start, end)
            .await
            .map_err(|error| error.message)?;
        if bytes.len() != expected {
            return Err("The audio provider returned an incomplete prefetch range".into());
        }
        if let Some(track_id) = source.deezer_track_id.as_deref() {
            decrypt_stripes(&mut bytes, track_id, 0)?;
        }
        cache
            .remember_total_with_token(key, total, generation, Some(cache_track_token))
            .await;
        let path = cache.block_path(key, total, start, end);
        if cache.read(&path, expected).await.is_none() {
            if !cache
                .mark_prefetch(key, total, generation, Some(cache_track_token))
                .await
            {
                return Ok(());
            }
            cache
                .write_with_token(&path, &bytes, generation, Some(cache_track_token))
                .await;
        }
        Ok(())
    }

    pub(super) async fn prefetch_blocks(
        &self,
        cache: &AudioCache,
        key: &str,
        data: &SourceData,
        size: u64,
        deezer_track_id: Option<&str>,
        is_soundcloud: bool,
        cancellation: &CancellationToken,
        generation: u64,
        cache_track_token: &CacheTrackToken,
    ) -> Result<(), String> {
        let Some((start, end)) = prefetch_range(size) else {
            return Ok(());
        };
        if cancellation.is_cancelled() || !cache.is_current(generation) {
            return Ok(());
        }
        let path = cache.block_path(key, size, start, end);
        let expected = (end - start + 1) as usize;
        if cache.read(&path, expected).await.is_some() {
            return Ok(());
        }
        let _guard = cache.lock_for(&path).await;
        if cache.read(&path, expected).await.is_some() {
            return Ok(());
        }
        let bytes = match data {
            SourceData::Inline(bytes) => {
                inline_range(bytes, start, end).map_err(|error| error.message)?
            }
            SourceData::Backend(backend) => backend
                .read_range(start, end, MediaFetchScope::Background, cancellation)
                .await
                .map_err(|error| error.message)?,
            SourceData::Hls(_) => return Ok(()),
            SourceData::Remote(url) => {
                let (request_start, request_end) = aligned_range(start, end, size);
                let response = send_with_retry(
                    "media.prefetch_range",
                    RequestClass::Media,
                    cancellation,
                    || {
                        self.client
                            .get(url)
                            .header(header::USER_AGENT, BROWSER_USER_AGENT)
                            .header(
                                header::RANGE,
                                format!("bytes={request_start}-{request_end}"),
                            )
                            .header(header::ACCEPT_ENCODING, "identity")
                    },
                )
                .await?;
                validate_media_response_url(response.url(), is_soundcloud)?;
                if !response.status().is_success() {
                    let reason = response
                        .status()
                        .canonical_reason()
                        .unwrap_or("unknown status");
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "playback provider stage=media.prefetch_range status={} retry=false class=final",
                            response.status()
                        ),
                    );
                    return Err(format!(
                        "The audio provider rejected the cache request (HTTP {} {reason})",
                        response.status().as_u16()
                    ));
                }
                let status = response.status();
                let body = read_response_range(response, request_start, request_end)
                    .await
                    .map_err(|error| error.message)?
                    .2;
                let mut bytes = ranged_body(status, &body, request_start, request_end)?;
                if let Some(track_id) = deezer_track_id {
                    decrypt_stripes(&mut bytes, track_id, request_start / STRIPE_SIZE as u64)?;
                }
                trim_range(bytes, start, end, request_start)?
            }
        };
        if bytes.len() != expected {
            return Err("The source returned an incomplete cache block".into());
        }
        cache
            .write_with_token(&path, &bytes, generation, Some(cache_track_token))
            .await;
        Ok(())
    }
}
