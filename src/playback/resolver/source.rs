use super::*;

use crate::playback::resolve_limiter::ResolvePriority;

impl StreamResolver {
    pub(crate) async fn resolve_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        backend: Option<MediaCredentials>,
        cancellation: CancellationToken,
        include_remote_size: bool,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }

        let resolved = match track.provider {
            PlaybackProvider::Deezer => {
                let mut outcome = self
                    .resolve_backend(
                        track,
                        BackendProvider::Deezer,
                        backend.as_ref(),
                        None,
                        true,
                        None,
                        &cancellation,
                        include_remote_size,
                    )
                    .await;
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                if let MediaResolveOutcome::RefreshSource(_) = outcome {
                    crate::diagnostics::event(
                        "WARN",
                        "deezer recovery source=backend reason=legacy_flac_refresh",
                    );
                    outcome = self
                        .resolve_backend(
                            track,
                            BackendProvider::Deezer,
                            backend.as_ref(),
                            None,
                            true,
                            None,
                            &cancellation,
                            include_remote_size,
                        )
                        .await;
                }
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                match outcome {
                    MediaResolveOutcome::Source(source) => ResolvedSource::from_backend(source),
                    MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                        let fallback = self
                            .resolve_deezer_backend_fallback_id(
                                track,
                                deezer_arl.as_ref(),
                                backend.as_ref(),
                                &cancellation,
                                include_remote_size,
                            )
                            .await?;
                        match fallback {
                            MediaResolveOutcome::Source(source) => {
                                ResolvedSource::from_backend(source)
                            }
                            MediaResolveOutcome::Unavailable
                            | MediaResolveOutcome::FallbackToDirect(_) => {
                                ResolvedSource::from_remote(
                                    self.resolve_deezer(
                                        &track.id,
                                        deezer_arl.as_ref(),
                                        false,
                                        &cancellation,
                                        include_remote_size,
                                    )
                                    .await?,
                                )
                            }
                            MediaResolveOutcome::RefreshSource(error) => return Err(error),
                            MediaResolveOutcome::Cancelled => {
                                return Err("Playback request cancelled".into());
                            }
                            MediaResolveOutcome::Fatal(error) => return Err(error),
                        }
                    }
                    MediaResolveOutcome::RefreshSource(error) => return Err(error),
                    MediaResolveOutcome::Cancelled => {
                        return Err("Playback request cancelled".into());
                    }
                    MediaResolveOutcome::Fatal(error) => return Err(error),
                }
            }
            PlaybackProvider::SoundCloud => {
                let prefetched_track = if backend.is_some() {
                    self.fetch_soundcloud_track(&track.id, soundcloud_token.as_ref(), &cancellation)
                        .await
                        .ok()
                } else {
                    None
                };
                if track.downloadable
                    && let Some(token) = soundcloud_token.as_ref()
                    && let Ok(source) = self
                        .resolve_soundcloud_original(
                            &track.id,
                            Some(token),
                            &cancellation,
                            include_remote_size,
                        )
                        .await
                {
                    ResolvedSource::from_remote(source)
                } else {
                    let outcome = self
                        .resolve_backend(
                            track,
                            BackendProvider::SoundCloud,
                            backend.as_ref(),
                            prefetched_track.as_ref(),
                            true,
                            None,
                            &cancellation,
                            include_remote_size,
                        )
                        .await;
                    match outcome {
                        MediaResolveOutcome::Source(source) => ResolvedSource::from_backend(source),
                        MediaResolveOutcome::Unavailable
                        | MediaResolveOutcome::FallbackToDirect(_) => ResolvedSource::from_remote(
                            self.resolve_soundcloud(
                                &track.id,
                                soundcloud_token.as_ref(),
                                &cancellation,
                                include_remote_size,
                                true,
                                prefetched_track,
                            )
                            .await?,
                        ),
                        MediaResolveOutcome::RefreshSource(error)
                        | MediaResolveOutcome::Fatal(error) => return Err(error),
                        MediaResolveOutcome::Cancelled => {
                            return Err("Playback request cancelled".into());
                        }
                    }
                }
            }
        };

        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        Ok(resolved)
    }

    /// Returns a useful total size for context-menu metadata without reading
    /// the media body. Provider metadata is preferred, then inline payload
    /// length, followed by a HEAD and a one-byte range probe for remote URLs.
    pub(crate) async fn source_size_for_info(
        &self,
        source: &ResolvedSource,
        cancellation: &CancellationToken,
    ) -> Result<u64, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        if source.size > 0 {
            return Ok(source.size);
        }
        match &source.data {
            SourceData::Inline(bytes) => Ok(bytes.len() as u64),
            SourceData::Remote(url) => Ok(self
                .probe_remote_size(url, cancellation, source.is_soundcloud)
                .await
                .unwrap_or(0)),
            SourceData::Backend(_) | SourceData::Hls(_) => {
                let Some(cache) = self.cache.as_ref() else {
                    return Ok(0);
                };
                let Some(key) = cache_key(source) else {
                    return Ok(0);
                };
                let Some(total) = cache.known_total(&key).await else {
                    return Ok(0);
                };
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                if cache.is_fully_cached(&key, total).await {
                    Ok(total)
                } else {
                    Ok(0)
                }
            }
        }
    }

    pub(super) async fn probe_remote_size(
        &self,
        url: &str,
        cancellation: &CancellationToken,
        is_soundcloud: bool,
    ) -> Option<u64> {
        let head = send_with_retry("media.info.head", RequestClass::Media, cancellation, || {
            self.client
                .head(url)
                .header(header::USER_AGENT, BROWSER_USER_AGENT)
                .header(header::ACCEPT_ENCODING, "identity")
        })
        .await
        .ok();
        if let Some(head) = head
            && validate_media_response_url(head.url(), is_soundcloud).is_ok()
            && let Some(size) =
                remote_size_from_response(head.status(), head.headers(), head.content_length())
        {
            return Some(size);
        }

        if cancellation.is_cancelled() {
            return None;
        }
        let response = send_with_retry(
            "media.info.range",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
                    .header(header::RANGE, "bytes=0-0")
            },
        )
        .await
        .ok()?;
        validate_media_response_url(response.url(), is_soundcloud).ok()?;
        remote_size_from_response(
            response.status(),
            response.headers(),
            response.content_length(),
        )
    }

    pub(crate) async fn download_source(
        &self,
        source: ResolvedSource,
        mut output: File,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), String> {
        self.download_source_inner(source, &mut output, cancellation, progress)
            .await
            .map_err(|error| error.message)
    }

    pub(super) async fn download_source_inner<W>(
        &self,
        source: ResolvedSource,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        if let SourceData::Backend(backend) = &source.data {
            return backend
                .download(output, cancellation, progress.as_ref())
                .await;
        }
        self.download(
            RemoteAudio {
                data: source.data,
                size: source.size,
                deezer_track_id: source.deezer_track_id,
                format: source.format,
                format_name: source.format_name,
                declared_bitrate: source.declared_bitrate,
                is_soundcloud: source.is_soundcloud,
                cache_identity: source.cache_identity,
            },
            output,
            cancellation,
            progress,
        )
        .await
    }

    pub(crate) async fn resolve_download_source(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variant: DownloadVariant,
        cancellation: CancellationToken,
    ) -> Result<ResolvedSource, String> {
        // Mirror the original app's download reservation once per user action.
        // Capability probes call the private helper directly and remain
        // ungated, since they have no equivalent in the original app.
        self.limiter
            .reserve(ResolvePriority::Background, &cancellation)
            .await?;
        self.resolve_download_source_with_size(
            track,
            deezer_arl,
            soundcloud_token,
            murglar,
            variant,
            cancellation,
            true,
            false,
        )
        .await
    }

    pub(super) async fn resolve_download_source_with_size(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variant: DownloadVariant,
        cancellation: CancellationToken,
        include_remote_size: bool,
        capability_only: bool,
    ) -> Result<ResolvedSource, String> {
        if track.provider == PlaybackProvider::Deezer {
            let source = match variant {
                DownloadVariant::DeezerFlac => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "FLAC",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                DownloadVariant::DeezerMp3_320 => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "MP3_320",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                DownloadVariant::DeezerMp3_128 => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "MP3_128",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
                // Existing callers use Best for the original strict FLAC
                // download action. Preserve that behavior while the new
                // exact variants power the dynamic context submenu.
                DownloadVariant::Best
                | DownloadVariant::Original
                | DownloadVariant::Murglar
                | DownloadVariant::Standard => {
                    self.resolve_deezer_exact(
                        track,
                        deezer_arl.as_ref(),
                        murglar.as_ref(),
                        "FLAC",
                        &cancellation,
                        include_remote_size,
                    )
                    .await?
                }
            };
            return Ok(source);
        }
        // Exact SoundCloud choices are resolved directly. Provider metadata
        // can lag behind the actual download endpoint, so the capability
        // probe must validate the path itself instead of trusting the
        // `downloadable` or `progressive` hints.
        if matches!(
            variant,
            DownloadVariant::Original | DownloadVariant::Murglar | DownloadVariant::Standard
        ) {
            let source = match variant {
                DownloadVariant::Original => {
                    let Some(token) = soundcloud_token.as_ref() else {
                        return Err(
                            "A SoundCloud account is required for the original file.".into()
                        );
                    };
                    self.resolve_soundcloud_original(
                        &track.id,
                        Some(token),
                        &cancellation,
                        include_remote_size,
                    )
                    .await
                    .map(ResolvedSource::from_remote)
                }
                DownloadVariant::Murglar => {
                    self.resolve_soundcloud_backend_download(
                        track,
                        murglar.as_ref(),
                        &cancellation,
                        include_remote_size,
                        capability_only,
                    )
                    .await
                }
                DownloadVariant::Standard => self
                    .resolve_soundcloud(
                        &track.id,
                        soundcloud_token.as_ref(),
                        &cancellation,
                        include_remote_size,
                        false,
                        None,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                _ => unreachable!(),
            }?;
            return Ok(source);
        }
        let order = selection_order(
            track,
            variant,
            deezer_arl.is_some(),
            soundcloud_token.is_some(),
            murglar.is_some(),
        );
        if order.is_empty() {
            return Err("The selected download format is unavailable for this track.".into());
        }
        for selected in order {
            let result = match selected {
                DownloadVariant::Original => self
                    .resolve_soundcloud_original(
                        &track.id,
                        soundcloud_token.as_ref(),
                        &cancellation,
                        include_remote_size,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                DownloadVariant::Murglar => {
                    self.resolve_soundcloud_backend_download(
                        track,
                        murglar.as_ref(),
                        &cancellation,
                        include_remote_size,
                        capability_only,
                    )
                    .await
                }
                DownloadVariant::Standard => self
                    .resolve_soundcloud(
                        &track.id,
                        None,
                        &cancellation,
                        include_remote_size,
                        false,
                        None,
                    )
                    .await
                    .map(ResolvedSource::from_remote),
                DownloadVariant::DeezerFlac
                | DownloadVariant::DeezerMp3_320
                | DownloadVariant::DeezerMp3_128 => {
                    Err("The selected Deezer format cannot be used for a SoundCloud track.".into())
                }
                DownloadVariant::Best => unreachable!(),
            };
            match result {
                Ok(source) => {
                    return Ok(source);
                }
                Err(error) if error == "Playback request cancelled" => return Err(error),
                Err(_) => {}
            }
        }
        Err(match variant {
            DownloadVariant::Original => {
                "The original SoundCloud file is unavailable for this track."
            }
            DownloadVariant::Murglar => {
                "Murglar did not return a lossless or high-quality file for this track."
            }
            DownloadVariant::Standard => {
                "A downloadable MP3 128 kbps stream is unavailable for this track."
            }
            DownloadVariant::Best => {
                "No downloadable SoundCloud audio source is available for this track."
            }
            DownloadVariant::DeezerFlac
            | DownloadVariant::DeezerMp3_320
            | DownloadVariant::DeezerMp3_128 => {
                "The selected Deezer format cannot be used for a SoundCloud track."
            }
        }
        .into())
    }

    /// Probe only formats which can be selected in the track download menu.
    /// Each candidate is resolved independently, so account and track
    /// metadata never fabricates a menu entry. The caller owns caching and
    /// cancellation because this method intentionally performs network I/O.
    pub(crate) async fn probe_download_capabilities(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        cancellation: CancellationToken,
    ) -> Result<Vec<DownloadChoice>, String> {
        let variants = Self::capability_variants(
            track,
            deezer_arl.is_some(),
            soundcloud_token.is_some(),
            murglar.is_some(),
        );
        let started = Instant::now();
        crate::diagnostics::event(
            "INFO",
            format!(
                "download capability probe start provider={:?} variants={} deezer_arl={} soundcloud_token={} murglar={}",
                track.provider,
                variants.len(),
                deezer_arl.is_some(),
                soundcloud_token.is_some(),
                murglar.is_some(),
            ),
        );
        let result = self
            .probe_download_capabilities_inner(
                track,
                deezer_arl,
                soundcloud_token,
                murglar,
                variants,
                cancellation,
            )
            .await;
        crate::diagnostics::event(
            "INFO",
            format!(
                "download capability probe finish provider={:?} elapsed_ms={} outcome={} choices={}",
                track.provider,
                started.elapsed().as_millis(),
                if result.is_ok() { "ready" } else { "failed" },
                result.as_ref().map_or(0, Vec::len),
            ),
        );
        result
    }

    pub(super) async fn probe_download_capabilities_inner(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
        variants: Vec<DownloadVariant>,
        cancellation: CancellationToken,
    ) -> Result<Vec<DownloadChoice>, String> {
        // A capability probe only needs the provider-confirmed format. It
        // must not issue a HEAD/range request for every candidate, since the
        // download path can resolve the size lazily when the user selects a
        // row. Candidates are started together, while the shared provider
        // gate still spaces their provider requests and preserves the retry
        // budget.
        let results = join_all(variants.iter().copied().map(|variant| {
            let cancellation = cancellation.clone();
            let deezer_arl = deezer_arl.clone();
            let soundcloud_token = soundcloud_token.clone();
            let murglar = murglar.clone();
            async move {
                let _direct_deezer_permit =
                    if track.provider == PlaybackProvider::Deezer && deezer_arl.is_some() {
                        Some(Self::acquire_direct_deezer_capability_slot(&cancellation).await?)
                    } else {
                        None
                    };
                self.resolve_download_source_with_size(
                    track,
                    deezer_arl,
                    soundcloud_token,
                    murglar,
                    variant,
                    cancellation,
                    false,
                    true,
                )
                .await
            }
        }))
        .await;
        if cancellation.is_cancelled() {
            return Err("Download capability probe cancelled".into());
        }
        let mut choices = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut failures = Vec::new();
        for (variant, result) in variants.into_iter().zip(results) {
            let source = match result {
                Ok(source) => source,
                Err(error)
                    if cancellation.is_cancelled()
                        || matches!(
                            error.as_str(),
                            "Playback request cancelled" | "Download capability probe cancelled"
                        ) =>
                {
                    return Err(error);
                }
                Err(error) => {
                    failures.push((variant, error));
                    continue;
                }
            };
            let key = Self::capability_dedup_key(
                track.provider,
                variant,
                &source.format_name,
                source.declared_bitrate,
            );
            if !seen.insert(key) {
                continue;
            }
            let (label, detail) = download_choice_text(variant, &source);
            choices.push(DownloadChoice {
                variant,
                label,
                detail,
            });
        }
        // A genuinely empty capability set is cacheable, but a probe where
        // every candidate failed for a transport/auth/provider reason must
        // remain retryable. Otherwise one transient outage turns into a
        // permanent "No downloadable formats" row for this account scope.
        // Do not cache a partial menu. If one requested variant failed for a
        // transient, authentication, or rate-limit reason, the successful
        // subset is not a reliable capability result for this account scope.
        if has_unexpected_capability_failure(&failures) {
            return Err(failures
                .into_iter()
                .find(|(variant, error)| !is_expected_capability_absence(*variant, error))
                .map(|(_, error)| error)
                .unwrap_or_else(|| "Download capability probe failed".to_owned()));
        }
        Ok(choices)
    }

    pub(super) fn capability_variants(
        track: &PlaybackTrack,
        deezer_arl: bool,
        soundcloud_token: bool,
        murglar_token: bool,
    ) -> Vec<DownloadVariant> {
        match track.provider {
            // Keep the track probe and collection menus on the same exact
            // credential policy. Murglar owns FLAC/MP3 320, while the Deezer
            // ARL owns MP3 128. Each candidate is still resolved per track
            // below, so this list only controls which formats are attempted.
            PlaybackProvider::Deezer => {
                deezer_collection_download_choices(deezer_arl, murglar_token)
                    .iter()
                    .map(|choice| choice.variant)
                    .collect()
            }
            PlaybackProvider::SoundCloud => {
                let mut variants = Vec::with_capacity(3);
                if soundcloud_token {
                    variants.push(DownloadVariant::Original);
                }
                if murglar_token {
                    variants.push(DownloadVariant::Murglar);
                }
                // The track response can lag behind the active transcoding
                // endpoint, so the standard path is always validated directly.
                // Its label comes from the resolved provider metadata, never a
                // fabricated SoundCloud quality.
                variants.push(DownloadVariant::Standard);
                variants
            }
        }
    }

    pub(super) async fn acquire_direct_deezer_capability_slot(
        cancellation: &CancellationToken,
    ) -> Result<tokio::sync::SemaphorePermit<'static>, String> {
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err("Download capability probe cancelled".into()),
            permit = DIRECT_DEEZER_CAPABILITY_GATE.acquire() => {
                permit.map_err(|_| "Download capability probe stopped unexpectedly".into())
            }
        }
    }

    pub(super) fn capability_dedup_key(
        _provider: PlaybackProvider,
        _variant: DownloadVariant,
        format_name: &str,
        declared_bitrate: Option<u32>,
    ) -> String {
        let codec = audio_format(format_name)
            .map(AudioFormat::label)
            .unwrap_or_else(|| format_name.trim())
            .to_ascii_uppercase();
        let quality = format!(
            "{}:{}",
            codec,
            declared_bitrate.map_or_else(|| "-".to_owned(), |bitrate| bitrate.to_string())
        );
        quality
    }
}
