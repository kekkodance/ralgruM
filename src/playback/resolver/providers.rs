use super::*;

impl StreamResolver {
    pub(super) async fn resolve_deezer_exact(
        &self,
        track: &PlaybackTrack,
        deezer_arl: Option<&DeezerArl>,
        backend: Option<&MediaCredentials>,
        expected: &str,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<ResolvedSource, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let backend_quality = match expected {
            "FLAC" => Some("LOSSLESS"),
            "MP3_320" => Some("HIGH_QUALITY"),
            "MP3_128" => None,
            _ => None,
        };
        if let Some(quality) = backend_quality {
            let mut outcome = self
                .resolve_backend(
                    track,
                    BackendProvider::Deezer,
                    backend,
                    None,
                    quality == "HIGH_QUALITY",
                    Some(quality),
                    cancellation,
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
                        backend,
                        None,
                        quality == "HIGH_QUALITY",
                        Some(quality),
                        cancellation,
                        include_remote_size,
                    )
                    .await;
            }
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            match outcome {
                MediaResolveOutcome::Source(source)
                    if source.metadata().format_name.eq_ignore_ascii_case(expected) =>
                {
                    return Ok(ResolvedSource::from_backend(source));
                }
                MediaResolveOutcome::Source(_)
                | MediaResolveOutcome::Unavailable
                | MediaResolveOutcome::FallbackToDirect(_) => {}
                MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                    return Err(error);
                }
                MediaResolveOutcome::Cancelled => {
                    return Err("Playback request cancelled".into());
                }
            }
        }
        if should_skip_direct_deezer_capability(include_remote_size, deezer_arl) {
            return Err("No Deezer direct source available without an ARL".into());
        }
        let source = self
            .resolve_deezer_exact_direct(
                &track.id,
                deezer_arl,
                expected,
                cancellation,
                include_remote_size,
            )
            .await?;
        if source.format_name.eq_ignore_ascii_case(expected) {
            Ok(ResolvedSource::from_remote(source))
        } else {
            Err(format!(
                "Deezer returned {} while {} was requested.",
                source.format_name, expected
            ))
        }
    }

    pub(super) async fn resolve_deezer_exact_direct(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        expected: &str,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let preferences: &'static [&'static str] = match expected {
            "FLAC" => &["FLAC"],
            "MP3_320" => &["MP3_320"],
            "MP3_128" => &["MP3_128"],
            _ => return Err("Unsupported Deezer download format.".into()),
        };
        self.resolve_deezer_with_formats(
            track_id,
            arl,
            preferences,
            false,
            cancellation,
            include_remote_size,
        )
        .await
    }

    pub(super) async fn resolve_soundcloud_original(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let url = format!(
            "https://api-v2.soundcloud.com/tracks/{track_id}/download?client_id={SOUNDCLOUD_CLIENT_ID}"
        );
        let value = response_json(
            send_with_retry(
                "soundcloud.original",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(url.clone(), token),
            )
            .await?,
            "soundcloud.original",
        )
        .await?;
        let url = value
            .get("redirectUri")
            .or_else(|| value.get("redirect_uri"))
            .or_else(|| value.get("url"))
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| "SoundCloud did not return an original download URL".to_string())?;
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| "SoundCloud returned an invalid original URL".to_string())?;
        validate_soundcloud_stream_url(&parsed)?;
        let provider_format = infer_soundcloud_original_format(&value, &parsed);
        let inspection = self
            .inspect_soundcloud_original_media(url, cancellation)
            .await?;
        let format = choose_soundcloud_original_format(
            inspection.sniffed_format,
            inspection.header_format,
            provider_format,
        )
        .ok_or_else(|| SOUNDCLOUD_ORIGINAL_FORMAT_UNKNOWN.to_owned())?;
        let size = if include_remote_size {
            inspection.size.unwrap_or(0)
        } else {
            0
        };
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let format_name = format.label().to_owned();
        let declared_bitrate = soundcloud_original_bitrate(&value, format);
        Ok(RemoteAudio {
            data: SourceData::Remote(url.into()),
            size,
            deezer_track_id: None,
            is_soundcloud: true,
            format,
            format_name: format_name.clone(),
            declared_bitrate,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::SoundCloud,
                track_id,
                SourceVariant::SoundCloudOriginal,
                format,
                &format_name,
            )),
        })
    }

    pub(super) async fn inspect_soundcloud_original_media(
        &self,
        url: &str,
        cancellation: &CancellationToken,
    ) -> Result<SoundCloudOriginalInspection, String> {
        let head = send_with_retry(
            "soundcloud.original.media",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .head(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
            },
        )
        .await;

        let (head_format, head_size) = match head {
            Ok(response) => {
                validate_soundcloud_stream_url(response.url())?;
                if response.status().is_success() {
                    (
                        soundcloud_format_from_media_headers(response.headers(), response.url()),
                        remote_size_from_response(
                            response.status(),
                            response.headers(),
                            response.content_length(),
                        ),
                    )
                } else if soundcloud_original_head_can_fallback(response.status()) {
                    (None, None)
                } else {
                    return Err(provider_rejection("soundcloud.original", response.status()));
                }
            }
            Err(error) if error == "Playback request cancelled" => return Err(error),
            Err(_) => (None, None),
        };

        // Always inspect the actual media bytes. Provider metadata and HEAD
        // headers are only fallback candidates because uploader originals can
        // be mislabeled by either source.
        let range = self
            .soundcloud_original_range_probe(url, cancellation)
            .await?;
        Ok(SoundCloudOriginalInspection {
            sniffed_format: range.sniffed_format,
            header_format: range.header_format.or(head_format),
            size: head_size.or(range.size),
        })
    }

    pub(super) async fn soundcloud_original_range_probe(
        &self,
        url: &str,
        cancellation: &CancellationToken,
    ) -> Result<SoundCloudOriginalInspection, String> {
        let response = send_with_retry(
            "soundcloud.original.media",
            RequestClass::Media,
            cancellation,
            || {
                self.client
                    .get(url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::ACCEPT_ENCODING, "identity")
                    .header(
                        header::RANGE,
                        format!("bytes=0-{}", SOUNDCLOUD_ORIGINAL_INSPECTION_BYTES - 1),
                    )
            },
        )
        .await?;
        validate_soundcloud_stream_url(response.url())?;
        if !response.status().is_success() {
            return Err(provider_rejection("soundcloud.original", response.status()));
        }
        let size = remote_size_from_response(
            response.status(),
            response.headers(),
            response.content_length(),
        );
        let header_format =
            soundcloud_format_from_media_headers(response.headers(), response.url());
        let bytes =
            read_bounded_response(response, SOUNDCLOUD_ORIGINAL_INSPECTION_BYTES, cancellation)
                .await?;
        Ok(SoundCloudOriginalInspection {
            sniffed_format: sniff_soundcloud_original_format(&bytes),
            header_format,
            size,
        })
    }

    pub(super) fn backend_artist_names(track: &PlaybackTrack) -> Vec<String> {
        let mut names = track
            .artists
            .iter()
            .map(|artist| artist.name.trim())
            .filter(|artist| !artist.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if names.is_empty() && !track.artist.trim().is_empty() {
            names.push(track.artist.trim().to_owned());
        }
        names
    }

    pub(super) fn backend_request(
        track: &PlaybackTrack,
        provider: BackendProvider,
        authoritative: Option<&Value>,
        allow_high_quality: bool,
        exact_quality: Option<&str>,
        include_remote_size: bool,
    ) -> MediaRequest {
        let (track_id, title, artist_names, duration_ms) =
            if provider == BackendProvider::SoundCloud {
                Self::soundcloud_backend_fields(track, authoritative)
            } else {
                (
                    track.id.clone(),
                    track.title.clone(),
                    Self::backend_artist_names(track),
                    track.duration.as_millis().min(u64::MAX as u128) as u64,
                )
            };
        MediaRequest::new(
            provider,
            track_id,
            title,
            artist_names,
            (provider == BackendProvider::Deezer).then(|| track.album.clone()),
            normalize_release_date(&track.release_date),
            duration_ms,
            allow_high_quality,
            exact_quality.map(str::to_owned),
            include_remote_size,
        )
    }

    pub(super) async fn resolve_backend(
        &self,
        track: &PlaybackTrack,
        provider: BackendProvider,
        credentials: Option<&MediaCredentials>,
        authoritative: Option<&Value>,
        allow_high_quality: bool,
        exact_quality: Option<&str>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> MediaResolveOutcome<BackendSource> {
        #[cfg(test)]
        if let Some(resolve) = &self.backend_resolve_override {
            return resolve(cancellation);
        }
        let Some(credentials) = credentials else {
            return MediaResolveOutcome::Unavailable;
        };
        let request = Self::backend_request(
            track,
            provider,
            authoritative,
            allow_high_quality,
            exact_quality,
            include_remote_size,
        );
        self.media_backend
            .resolve(request, credentials, cancellation)
            .await
    }

    pub(super) async fn resolve_deezer_backend_fallback_id(
        &self,
        track: &PlaybackTrack,
        arl: Option<&DeezerArl>,
        credentials: Option<&MediaCredentials>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<MediaResolveOutcome<BackendSource>, String> {
        let (Some(arl), Some(credentials)) = (arl, credentials) else {
            return Ok(MediaResolveOutcome::Unavailable);
        };
        let identity = match self
            .fetch_deezer_playback_context(&track.id, Some(arl), cancellation)
            .await
        {
            Ok(context) => context.identity,
            Err(error) if error == "Playback request cancelled" => return Err(error),
            Err(_) => return Ok(MediaResolveOutcome::Unavailable),
        };
        let Some(fallback_id) = deezer_fallback_track_id(&identity, &track.id) else {
            return Ok(MediaResolveOutcome::Unavailable);
        };
        let mut fallback_track = track.clone();
        fallback_track.id = fallback_id.to_owned();
        Ok(self
            .resolve_backend(
                &fallback_track,
                BackendProvider::Deezer,
                Some(credentials),
                None,
                true,
                None,
                cancellation,
                include_remote_size,
            )
            .await)
    }

    pub(crate) async fn record_deezer_listen(&self, payload: Value, arl: DeezerArl) -> bool {
        match self.try_record_deezer_listen(payload, &arl).await {
            Ok(()) => true,
            Err(error) => {
                crate::diagnostics::event(
                    "WARN",
                    format!("deezer listening history update failed reason={error}"),
                );
                false
            }
        }
    }

    pub(super) async fn try_record_deezer_listen(
        &self,
        payload: Value,
        arl: &DeezerArl,
    ) -> Result<(), String> {
        let session_url = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
        let session_response = self
            .client
            .post(session_url)
            .headers(deezer_headers(Some(arl), None)?)
            .header(header::CONTENT_LENGTH, "0")
            .body("")
            .send()
            .await
            .map_err(request_error)?;
        if !session_response.status().is_success() {
            return Err(format!(
                "session status={}",
                session_response.status().as_u16()
            ));
        }
        let cookies = response_cookies(&session_response);
        let session = response_json(session_response, "deezer.listen.session").await?;
        let check_form = session
            .pointer("/results/checkForm")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "session token missing".to_string())?;
        let response = self
            .deezer_listen_request(
                check_form,
                deezer_headers(Some(arl), Some(&cookies))?,
                &payload,
            )
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(format!("status={}", response.status().as_u16()));
        }
        Ok(())
    }

    pub(super) fn deezer_listen_request(
        &self,
        check_form: &str,
        headers: HeaderMap,
        payload: &Value,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(format!(
                "https://www.deezer.com/ajax/gw-light.php?method=log.listen&input=3&api_version=1.0&api_token={check_form}"
            ))
            .headers(headers)
            .json(payload)
    }

    pub(crate) async fn record_soundcloud_listen(
        &self,
        report: SoundCloudListenReport,
        token: SoundCloudToken,
    ) -> bool {
        match self.try_record_soundcloud_listen(&report, &token).await {
            Ok(()) => true,
            Err(error) => {
                crate::diagnostics::event(
                    "WARN",
                    format!("soundcloud listening history update failed reason={error}"),
                );
                false
            }
        }
    }

    pub(super) async fn try_record_soundcloud_listen(
        &self,
        report: &SoundCloudListenReport,
        token: &SoundCloudToken,
    ) -> Result<(), String> {
        let response = self
            .soundcloud_listen_request(token, report)?
            .send()
            .await
            .map_err(request_error)?;
        if !response.status().is_success() {
            return Err(format!("status={}", response.status().as_u16()));
        }
        Ok(())
    }

    pub(super) fn soundcloud_listen_request(
        &self,
        token: &SoundCloudToken,
        report: &SoundCloudListenReport,
    ) -> Result<reqwest::RequestBuilder, String> {
        let authorization = token
            .authorization_header()
            .map_err(|_| "The saved SoundCloud session is invalid".to_string())?;
        Ok(self
            .client
            .post("https://api-v2.soundcloud.com/me/play-history")
            .header(header::USER_AGENT, SOUNDCLOUD_MOBILE_USER_AGENT)
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_ENCODING, SOUNDCLOUD_MOBILE_ACCEPT_ENCODING)
            .header(header::AUTHORIZATION, authorization)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&report.payload()))
    }

    pub(super) async fn fetch_deezer_playback_context(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        cancellation: &CancellationToken,
    ) -> Result<DeezerPlaybackContext, String> {
        let session_url = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
        let session_headers = deezer_headers(arl, None)?;
        let session_response = send_with_retry(
            "deezer.session",
            RequestClass::Provider,
            cancellation,
            || {
                self.client
                    .post(session_url)
                    .headers(session_headers.clone())
                    .header(header::CONTENT_LENGTH, "0")
                    .body("")
            },
        )
        .await?;
        let cookies = response_cookies(&session_response);
        let session = response_json(session_response, "deezer.session").await?;
        let check_form = session
            .pointer("/results/checkForm")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let license_token = session
            .pointer("/results/USER/OPTIONS/license_token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if license_token.is_empty() {
            return Err("Deezer login is required for playback".into());
        }
        let headers = deezer_headers(arl, Some(&cookies))?;
        let song_url = format!(
            "https://www.deezer.com/ajax/gw-light.php?method=deezer.pageTrack&input=3&api_version=1.0&api_token={check_form}"
        );
        let song = response_json(
            send_with_retry("deezer.song", RequestClass::Provider, cancellation, || {
                self.client
                    .post(song_url.clone())
                    .headers(headers.clone())
                    .json(&json!({ "sng_id": track_id }))
            })
            .await?,
            "deezer.song",
        )
        .await?;
        if deezer_track_is_unavailable(&song) {
            return Err(DEEZER_UNAVAILABLE.into());
        }
        Ok(DeezerPlaybackContext {
            headers,
            license_token: license_token.to_owned(),
            identity: deezer_playback_identity(&song, track_id)?,
        })
    }

    pub(super) async fn resolve_deezer(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        strict_flac: bool,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        self.resolve_deezer_with_formats(
            track_id,
            arl,
            deezer_format_preferences(strict_flac),
            strict_flac,
            cancellation,
            include_remote_size,
        )
        .await
    }

    pub(super) async fn resolve_deezer_with_formats(
        &self,
        track_id: &str,
        arl: Option<&DeezerArl>,
        formats: &[&str],
        strict_flac: bool,
        cancellation: &CancellationToken,
        include_remote_size: bool,
    ) -> Result<RemoteAudio, String> {
        let DeezerPlaybackContext {
            headers,
            license_token,
            identity,
        } = self
            .fetch_deezer_playback_context(track_id, arl, cancellation)
            .await?;
        let formats = formats
            .iter()
            .map(|format| json!({ "cipher": "BF_CBC_STRIPE", "format": format }))
            .collect::<Vec<_>>();
        let media = response_json(
            send_with_retry("deezer.media", RequestClass::Provider, cancellation, || {
                self.client
                    .post("https://media.deezer.com/v1/get_url")
                    .headers(headers.clone())
                    .json(&json!({
                        "license_token": license_token,
                        "media": [{ "type": "FULL", "formats": formats.clone() }],
                        "track_tokens": [identity.track_token.clone()]
                    }))
            })
            .await?,
            "deezer.media",
        )
        .await?;
        let result = media
            .pointer("/data/0/media/0")
            .ok_or_else(|| DEEZER_UNAVAILABLE.to_string())?;
        let url = result
            .pointer("/sources/0/url")
            .and_then(Value::as_str)
            .and_then(|url| url.split('#').next())
            .filter(|url| !url.is_empty())
            .ok_or_else(|| DEEZER_UNAVAILABLE.to_string())?;
        let size = match result
            .get("filesize")
            .and_then(Value::as_u64)
            .filter(|size| *size > 0)
        {
            Some(size) => size,
            None if include_remote_size => self
                .probe_remote_size(url, cancellation, false)
                .await
                .unwrap_or(0),
            None => 0,
        };
        let format_name = result
            .get("format")
            .and_then(Value::as_str)
            .ok_or_else(|| "Deezer did not identify the resolved audio format".to_string())?
            .to_owned();
        let declared_bitrate = declared_bitrate(&format_name);
        let format = audio_format(&format_name)
            .ok_or_else(|| "Deezer did not identify the resolved audio format".to_string())?;
        let format = enforce_deezer_format(format, strict_flac)?;
        crate::diagnostics::event(
            "INFO",
            format!(
                "deezer source path=direct source_id=media_api format={} decryption=once strict_flac={strict_flac} fallback={}",
                format.label(),
                identity.used_fallback,
            ),
        );
        Ok(RemoteAudio {
            data: SourceData::Remote(url.into()),
            size,
            deezer_track_id: Some(identity.source_track_id.clone()),
            format,
            format_name: format_name.clone(),
            declared_bitrate,
            is_soundcloud: false,
            cache_identity: Some(stable_cache_identity(
                PlaybackProvider::Deezer,
                &identity.source_track_id,
                SourceVariant::Deezer,
                format,
                &format_name,
            )),
        })
    }

    pub(super) async fn fetch_soundcloud_track(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
    ) -> Result<Value, String> {
        let track_url = format!(
            "https://api-v2.soundcloud.com/tracks/{track_id}?client_id={SOUNDCLOUD_CLIENT_ID}"
        );
        response_json(
            send_with_retry(
                "soundcloud.track",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(track_url.clone(), token),
            )
            .await?,
            "soundcloud.track",
        )
        .await
    }

    pub(super) async fn resolve_soundcloud(
        &self,
        track_id: &str,
        token: Option<&SoundCloudToken>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
        prefer_hls: bool,
        prefetched_track: Option<Value>,
    ) -> Result<RemoteAudio, String> {
        let track = match prefetched_track {
            Some(track) => track,
            None => {
                self.fetch_soundcloud_track(track_id, token, cancellation)
                    .await?
            }
        };
        let track_authorization = soundcloud_track_authorization(&track);
        let transcodings = track
            .pointer("/media/transcodings")
            .and_then(Value::as_array)
            .ok_or_else(|| "SoundCloud did not return audio transcodings".to_string())?;
        let candidates = if prefer_hls {
            soundcloud_playback_transcodings(transcodings)
        } else {
            soundcloud_transcodings(transcodings).collect()
        };
        for selected in candidates {
            let hls = is_soundcloud_hls_transcoding(selected);
            let Some(selected_url) = selected.get("url").and_then(Value::as_str) else {
                continue;
            };
            let Ok(mut url) = reqwest::Url::parse(selected_url) else {
                return Err("SoundCloud returned an invalid transcoding URL".into());
            };
            validate_soundcloud_transcoding_url(&url)?;
            append_soundcloud_transcoding_query(&mut url, track_authorization);
            let transcoding_url = url.to_string();
            let response = match send_with_retry(
                "soundcloud.transcoding",
                RequestClass::Provider,
                cancellation,
                || self.soundcloud_get(transcoding_url.clone(), token),
            )
            .await
            {
                Ok(response) => response,
                Err(error) if error == "Playback request cancelled" => return Err(error),
                Err(error) if is_soundcloud_candidate_absence(&error) => continue,
                Err(error) => return Err(error),
            };
            let value = match response_json(response, "soundcloud.transcoding").await {
                Ok(value) => value,
                Err(error) if is_soundcloud_candidate_absence(&error) => continue,
                Err(error) => return Err(error),
            };
            let Some(stream_url) = value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
            else {
                return Err("SoundCloud transcoding response did not return a stream URL".into());
            };
            let stream_url = reqwest::Url::parse(stream_url)
                .map_err(|_| "SoundCloud returned an invalid stream URL".to_string())?;
            let (data, size) = if hls {
                let manifest_url = stream_url.to_string();
                let descriptor = match super::soundcloud_hls::inspect(
                    &self.client,
                    &manifest_url,
                    cancellation,
                    BROWSER_USER_AGENT,
                )
                .await
                {
                    Ok(descriptor) => descriptor,
                    Err(error) if error == "Playback request cancelled" => return Err(error),
                    Err(_) => continue,
                };
                (SourceData::Hls(Box::new(descriptor)), 0)
            } else {
                validate_soundcloud_stream_url(&stream_url)?;
                let stream_url = stream_url.to_string();
                let size = if include_remote_size {
                    self.probe_remote_size(&stream_url, cancellation, true)
                        .await
                        .unwrap_or(0)
                } else {
                    0
                };
                (SourceData::Remote(stream_url), size)
            };
            if cancellation.is_cancelled() {
                return Err("Playback request cancelled".into());
            }
            let format = transcoding_format(selected);
            let format_name = if hls {
                AudioFormat::M4a.label().to_owned()
            } else {
                soundcloud_format_name(selected, format)
            };
            return Ok(RemoteAudio {
                data,
                size,
                deezer_track_id: None,
                format,
                format_name: format_name.clone(),
                declared_bitrate: soundcloud_transcoding_bitrate(selected),
                is_soundcloud: true,
                cache_identity: Some(stable_cache_identity(
                    PlaybackProvider::SoundCloud,
                    track_id,
                    SourceVariant::SoundCloudStandard,
                    format,
                    &format_name,
                )),
            });
        }
        Err("No active stream link found for this SoundCloud track".into())
    }

    pub(super) fn backend_artist_names_for_soundcloud(track: &PlaybackTrack) -> Vec<String> {
        let mut names = Vec::new();
        for artist in &track.artists {
            let name = artist.name.trim();
            if name.is_empty()
                || names
                    .iter()
                    .any(|known: &String| known.eq_ignore_ascii_case(name))
            {
                continue;
            }
            names.push(name.to_owned());
            if names.len() == MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES {
                break;
            }
        }
        if let Some(owner_slug) = Self::soundcloud_owner_slug(&track.service_url)
            && !names
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&owner_slug))
        {
            if names.len() == MAX_SOUNDCLOUD_BACKEND_ARTIST_NAMES {
                names.pop();
            }
            names.push(owner_slug);
        }
        if names.is_empty() {
            let fallback = track.artist.trim();
            if !fallback.is_empty() {
                names.push(fallback.to_owned());
            }
        }
        names
    }

    pub(super) fn soundcloud_backend_fields(
        track: &PlaybackTrack,
        authoritative: Option<&Value>,
    ) -> (String, String, Vec<String>, u64) {
        let authoritative_id = authoritative
            .and_then(|value| {
                deezer_id(value.get("id")).or_else(|| {
                    value
                        .get("urn")
                        .and_then(Value::as_str)
                        .and_then(|urn| urn.strip_prefix("soundcloud:tracks:"))
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_owned)
                })
            })
            .filter(|id| id.parse::<u64>().is_ok_and(|id| id > 0));
        let title = authoritative
            .and_then(|value| value.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or(track.title.trim())
            .to_owned();
        let uploader = authoritative
            .and_then(|value| {
                [
                    value.pointer("/user/username"),
                    value.pointer("/user/full_name"),
                ]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .find(|name| !name.is_empty())
            })
            .map(str::to_owned);
        let artist_names = uploader
            .map(|uploader| vec![uploader])
            .unwrap_or_else(|| Self::backend_artist_names_for_soundcloud(track));
        let duration_ms = authoritative
            .and_then(|value| {
                [value.get("duration"), value.get("full_duration")]
                    .into_iter()
                    .flatten()
                    .filter_map(|duration| match duration {
                        Value::Number(duration) => duration.as_u64(),
                        Value::String(duration) => duration.trim().parse().ok(),
                        _ => None,
                    })
                    .find(|duration| *duration > 0)
            })
            .unwrap_or_else(|| track.duration.as_millis().min(u64::MAX as u128) as u64);
        (
            authoritative_id.unwrap_or_else(|| track.id.clone()),
            title,
            artist_names,
            duration_ms,
        )
    }

    pub(super) fn soundcloud_owner_slug(service_url: &str) -> Option<String> {
        let url = reqwest::Url::parse(service_url.trim()).ok()?;
        let host = url.host_str()?;
        let owned_host = host.eq_ignore_ascii_case("soundcloud.com")
            || host.to_ascii_lowercase().ends_with(".soundcloud.com");
        if !url.scheme().eq_ignore_ascii_case("https")
            || url.port().is_some_and(|port| port != 443)
            || !owned_host
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        let segment = url.path_segments()?.next()?;
        let slug = percent_decode_filename(segment).trim().to_owned();
        if slug.is_empty()
            || slug.len() > MAX_SOUNDCLOUD_OWNER_SLUG_LENGTH
            || slug.eq_ignore_ascii_case("sets")
            || slug.eq_ignore_ascii_case("tracks")
            || slug
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        {
            return None;
        }
        Some(slug)
    }

    pub(super) async fn resolve_soundcloud_backend_download(
        &self,
        track: &PlaybackTrack,
        credentials: Option<&MediaCredentials>,
        cancellation: &CancellationToken,
        include_remote_size: bool,
        capability_only: bool,
    ) -> Result<ResolvedSource, String> {
        let Some(credentials) = credentials else {
            return Err("Murglar returned no HQ source".into());
        };
        let request = Self::backend_request(
            track,
            BackendProvider::SoundCloud,
            None,
            true,
            None,
            include_remote_size,
        );
        if capability_only {
            let outcome = self
                .media_backend
                .probe(request, credentials, cancellation)
                .await;
            return match outcome {
                MediaResolveOutcome::Source(format) => Ok(ResolvedSource {
                    data: SourceData::Inline(Vec::new()),
                    size: 0,
                    deezer_track_id: None,
                    is_soundcloud: true,
                    cache_identity: Some(stable_cache_identity(
                        PlaybackProvider::SoundCloud,
                        &track.id,
                        SourceVariant::BackendSoundCloud,
                        format.format,
                        &format.format_name,
                    )),
                    format: format.format,
                    format_name: format.format_name,
                    declared_bitrate: format.declared_bitrate,
                }),
                MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                    Err("Murglar returned no HQ source".into())
                }
                MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                    Err(error)
                }
                MediaResolveOutcome::Cancelled => Err("Playback request cancelled".into()),
            };
        }
        match self
            .media_backend
            .resolve(request, credentials, cancellation)
            .await
        {
            MediaResolveOutcome::Source(source) => Ok(ResolvedSource::from_backend(source)),
            MediaResolveOutcome::Unavailable | MediaResolveOutcome::FallbackToDirect(_) => {
                Err("Murglar returned no HQ source".into())
            }
            MediaResolveOutcome::RefreshSource(error) | MediaResolveOutcome::Fatal(error) => {
                Err(error)
            }
            MediaResolveOutcome::Cancelled => Err("Playback request cancelled".into()),
        }
    }

    pub(super) fn soundcloud_get(
        &self,
        url: String,
        token: Option<&SoundCloudToken>,
    ) -> reqwest::RequestBuilder {
        self.soundcloud_request(self.client.get(url), token)
    }

    pub(super) fn soundcloud_request(
        &self,
        request: reqwest::RequestBuilder,
        token: Option<&SoundCloudToken>,
    ) -> reqwest::RequestBuilder {
        let request = request.header(header::USER_AGENT, BROWSER_USER_AGENT);
        match token {
            Some(token) => match token.authorization_header() {
                Ok(authorization) => request.header(header::AUTHORIZATION, authorization),
                Err(_) => request,
            },
            None => request,
        }
    }

    pub(super) async fn download<W>(
        &self,
        remote: RemoteAudio,
        output: &mut W,
        cancellation: &CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> Result<(), PlaybackDownloadError>
    where
        W: DownloadOutput,
    {
        let mut downloaded = 0;
        let remote_url = match remote.data {
            SourceData::Inline(bytes) => {
                if bytes.len() as u64 > MAX_AUDIO_SIZE {
                    return Err("The download is too large".into());
                }
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                output.set_total_hint(Some(bytes.len() as u64));
                output
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "The download could not be written".to_string())?;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(
                        bytes.len() as u64,
                        Some(bytes.len() as u64),
                    ));
                }
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                return Ok(());
            }
            SourceData::Remote(url) => url,
            SourceData::Hls(descriptor) => {
                return soundcloud_hls::download(
                    &self.client,
                    &descriptor,
                    output,
                    cancellation,
                    progress.as_ref(),
                    MAX_AUDIO_SIZE,
                    BROWSER_USER_AGENT,
                )
                .await;
            }
            SourceData::Backend(_) => {
                return Err("The backend source was not handled by its provider boundary".into());
            }
        };
        if remote.size > 0 {
            if remote.size > MAX_AUDIO_SIZE {
                return Err("The download is too large".into());
            }
            output.set_total_hint(Some(remote.size));
            let mut start = 0;
            while start < remote.size {
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                let end = (start + RANGE_CHUNK - 1).min(remote.size - 1);
                let response =
                    send_with_retry("media.range", RequestClass::Media, cancellation, || {
                        self.client
                            .get(&remote_url)
                            .header(header::USER_AGENT, BROWSER_USER_AGENT)
                            .header(header::RANGE, format!("bytes={start}-{end}"))
                            .header(header::ACCEPT_ENCODING, "identity")
                    })
                    .await?;
                validate_media_response_url(response.url(), remote.is_soundcloud)?;
                let status = response.status();
                let content_length = response.content_length();
                if !status.is_success() {
                    return Err(PlaybackDownloadError::media_status(
                        "audio provider",
                        status,
                    ));
                }
                let bytes = response.bytes().await.map_err(request_error)?;
                log_response_diagnostics("audio range", status, content_length);
                let mut bytes = ranged_body(status, &bytes, start, end)?;
                if let Some(track_id) = remote.deezer_track_id.as_deref() {
                    decrypt_stripes(&mut bytes, track_id, start / STRIPE_SIZE as u64)?;
                }
                output
                    .write_all(&bytes)
                    .await
                    .map_err(|_| "The playback buffer could not be written".to_string())?;
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                downloaded += bytes.len() as u64;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(
                        downloaded,
                        (remote.size > 0).then_some(remote.size),
                    ));
                }
                start = end + 1;
            }
        } else {
            let response = send_with_retry("media.full", RequestClass::Media, cancellation, || {
                self.client
                    .get(&remote_url)
                    .header(header::USER_AGENT, BROWSER_USER_AGENT)
                    .header(header::RANGE, "bytes=0-")
                    .header(header::ACCEPT_ENCODING, "identity")
            })
            .await?;
            validate_media_response_url(response.url(), remote.is_soundcloud)?;
            if !response.status().is_success() {
                return Err(PlaybackDownloadError::media_status(
                    "audio provider",
                    response.status(),
                ));
            }
            let status = response.status();
            let content_length = response.content_length();
            crate::diagnostics::event(
                "INFO",
                format!("audio full response status={status} content_length={content_length:?}"),
            );
            if response
                .content_length()
                .is_some_and(|size| size > MAX_AUDIO_SIZE)
            {
                return Err("The download is too large".into());
            }
            let total = content_range_total(response.headers()).or(content_length);
            output.set_total_hint(total);
            let mut deezer_stream = remote
                .deezer_track_id
                .as_deref()
                .map(DeezerStripeStream::new)
                .transpose()?;
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                if cancellation.is_cancelled() {
                    return Err("Playback request cancelled".into());
                }
                let chunk = chunk.map_err(request_error)?;
                downloaded = downloaded.saturating_add(chunk.len() as u64);
                if downloaded > MAX_AUDIO_SIZE {
                    return Err("The download is too large".into());
                }
                if let Some(deezer_stream) = deezer_stream.as_mut() {
                    deezer_stream
                        .write_chunk(&chunk, &mut *output)
                        .await
                        .map_err(|_| "The download could not be written".to_string())?;
                } else {
                    output
                        .write_all(&chunk)
                        .await
                        .map_err(|_| "The download could not be written".to_string())?;
                }
                output
                    .flush()
                    .await
                    .map_err(|_| "The playback buffer could not be finalized".to_string())?;
                if let Some(progress) = &progress {
                    progress(ProgressUpdate::bytes(downloaded, total));
                }
            }
            if let Some(deezer_stream) = deezer_stream.as_mut() {
                deezer_stream
                    .finish(&mut *output)
                    .await
                    .map_err(|_| "The download could not be written".to_string())?;
            }
            output
                .flush()
                .await
                .map_err(|_| "The playback buffer could not be finalized".to_string())?;
        }
        Ok(())
    }
}
