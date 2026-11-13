use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::StreamResolver;
use super::{AudioFormat, RemoteAudio, SourceData, SourceVariant, stable_cache_identity};

const KLICKAUD_REFERER: &str = "https://www.klickaud.org/";
/// Retry attempts for the overall flow when the service misbehaves early.
const FLOW_ATTEMPTS: u32 = 2;

mod browser;

#[cfg(windows)]
pub(crate) fn preview_verification_if_requested() -> Result<(), String> {
    browser::preview_verification_if_requested()
}

impl StreamResolver {
    /// Resolves a Go+ gated SoundCloud track through klickaud. Any failure is
    /// returned as an error string the caller treats as "fall through to the
    /// normal chain"; cancellation propagates as the shared cancel message.
    pub(super) async fn resolve_klickaud(
        &self,
        track: &super::PlaybackTrack,
        track_json: &Value,
        cancellation: &CancellationToken,
    ) -> Result<RemoteAudio, String> {
        if cancellation.is_cancelled() {
            return Err("Playback request cancelled".into());
        }
        let web_url = klickaud_track_url(track, track_json)?;
        let mut last_error = String::new();
        for attempt in 0..FLOW_ATTEMPTS {
            if attempt > 0 {
                tokio::select! {
                    _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
                    _ = tokio::time::sleep(Duration::from_millis(750 + u64::from(attempt) * 250)) => {}
                }
            }
            let outcome = browser::resolve_download_url(&web_url, cancellation)
                .await
                .and_then(|target| {
                    validate_download_url(&target.url).map(|url| (url, target.cookies))
                });
            match outcome {
                Ok((download_url, cookies)) => {
                    crate::diagnostics::event("INFO", "klickaud resolve outcome=ready");
                    if let Ok(mut sources) = self.klickaud_sources.lock() {
                        sources.insert(track.id.clone());
                    }
                    return Ok(klickaud_remote_audio(track, download_url, cookies));
                }
                Err(error) => {
                    if cancellation.is_cancelled() || error == "Playback request cancelled" {
                        return Err("Playback request cancelled".into());
                    }
                    crate::diagnostics::event(
                        "WARN",
                        format!(
                            "klickaud resolve attempt={} failed reason={error}",
                            attempt + 1
                        ),
                    );
                    last_error = error;
                }
            }
        }
        Err(last_error)
    }

    /// Marks a track whose klickaud resolution failed this session so the
    /// capability probe stops offering the fabricated standard row.
    pub(super) fn record_klickaud_failure(&self, track: &super::PlaybackTrack) {
        if let Ok(mut failures) = self.klickaud_failures.lock() {
            failures.insert(track.id.clone());
        }
    }

    /// Last-resort standard download after the native SoundCloud stream
    /// failed. A real click resolves through klickaud; a capability probe
    /// fabricates the menu row instead of running the flow on their server.
    /// The original error surfaces when klickaud cannot help either.
    pub(super) async fn klickaud_standard_fallback(
        &self,
        track: &super::PlaybackTrack,
        track_json: Option<&Value>,
        capability_only: bool,
        cancellation: &CancellationToken,
        native_error: String,
    ) -> Result<RemoteAudio, String> {
        let attempted_and_failed = self
            .klickaud_failures
            .lock()
            .is_ok_and(|failures| failures.contains(&track.id));
        if attempted_and_failed {
            return Err(native_error);
        }
        if capability_only {
            return Ok(klickaud_placeholder_source(track));
        }
        match track_json.map(|track_json| self.resolve_klickaud(track, track_json, cancellation)) {
            Some(result) => match result.await {
                Ok(source) => Ok(source),
                Err(error) if error == "Playback request cancelled" => Err(error),
                Err(_) => Err(native_error),
            },
            None => Err(native_error),
        }
    }
}

/// The delivery URL must stay on klickaud: the browser returns it to Rust and
/// the media client follows it without another browser check.
fn validate_download_url(raw: &str) -> Result<String, String> {
    let url =
        Url::parse(raw).map_err(|_| "klickaud returned an invalid download url".to_string())?;
    if url.scheme() != "https" {
        return Err("klickaud returned a non-https download url".into());
    }
    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "klickaud returned a download url without a host".to_string())?;
    if host != "klickaud.org" && !host.ends_with(".klickaud.org") {
        return Err("klickaud returned a download url on an unexpected host".into());
    }
    Ok(url.into())
}

/// The SoundCloud web URL for a track, preferring the canonical permalink
/// from the track JSON and falling back to the stored service URL.
fn klickaud_track_url(track: &super::PlaybackTrack, track_json: &Value) -> Result<String, String> {
    for candidate in [
        track_json
            .get("permalink_url")
            .and_then(Value::as_str)
            .map(str::to_owned),
        track_json
            .get("permalinkUrl")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Some(track.service_url.clone()),
    ]
    .into_iter()
    .flatten()
    {
        let url = candidate.trim().to_owned();
        if url.is_empty() {
            continue;
        }
        if let Ok(parsed) = Url::parse(&url)
            && parsed.scheme() == "https"
            && let Some(host) = parsed.host_str()
            && host.eq_ignore_ascii_case("soundcloud.com")
        {
            return Ok(url);
        }
    }
    Err("The SoundCloud track URL is unavailable".to_string())
}

/// The source shape the capability probe resolves for a gated track: static
/// metadata only, no network. The URL is a placeholder that is never
/// fetched; the probe only reads format and bitrate for the menu row.
pub(super) fn klickaud_placeholder_source(track: &super::PlaybackTrack) -> RemoteAudio {
    RemoteAudio {
        data: SourceData::Remote {
            url: format!("about:klickaud-{}", track.id),
            referer: Some(KLICKAUD_REFERER),
            cookies: None,
        },
        size: 0,
        deezer_track_id: None,
        format: AudioFormat::Mp3,
        format_name: "MP3".to_owned(),
        declared_bitrate: Some(128),
        is_soundcloud: false,
        cache_identity: None,
    }
}

fn klickaud_remote_audio(
    track: &super::PlaybackTrack,
    download_url: String,
    cookies: Option<String>,
) -> RemoteAudio {
    RemoteAudio {
        data: SourceData::Remote {
            url: download_url,
            referer: Some(KLICKAUD_REFERER),
            cookies,
        },
        size: 0,
        deezer_track_id: None,
        format: AudioFormat::Mp3,
        format_name: "MP3".to_owned(),
        declared_bitrate: Some(128),
        // The delivery host is klickaud, not SoundCloud; the sndcdn
        // validators must never see it.
        is_soundcloud: false,
        cache_identity: Some(stable_cache_identity(
            super::PlaybackProvider::SoundCloud,
            &track.id,
            SourceVariant::SoundCloudStandard,
            AudioFormat::Mp3,
            "MP3",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "live network probe for manual klickaud verification"]
    async fn live_klickaud_flow_reaches_playable_audio() {
        let resolver = StreamResolver::new().unwrap();
        let track = test_track();
        let track_json = serde_json::json!({
            "policy": "SNIP",
            "permalink_url": "https://soundcloud.com/kendrick-lamar-music/money-trees",
            "media": {"transcodings": [{"snipped": true}]},
        });
        let cancellation = CancellationToken::new();
        let source = resolver
            .resolve_klickaud(&track, &track_json, &cancellation)
            .await
            .unwrap_or_else(|error| panic!("klickaud flow failed: {error}"));
        let resolved = super::super::ResolvedSource::from_remote(source);
        let size = resolver
            .source_size_for_info(&resolved, &cancellation)
            .await
            .expect("klickaud size probe");
        assert!(size > 0, "klickaud delivery size");
        let download_path =
            std::env::temp_dir().join(format!("ralgrum-klickaud-live-{}.mp3", std::process::id()));
        let file = tokio::fs::File::create(&download_path).await.unwrap();
        resolver
            .download_source(resolved.clone(), file, &cancellation, None)
            .await
            .expect("klickaud download");
        let downloaded = tokio::fs::metadata(&download_path).await.unwrap().len();
        let _ = tokio::fs::remove_file(&download_path).await;
        assert!(downloaded > 1_000_000, "download size: {downloaded}");
        let SourceData::Remote {
            url,
            referer,
            cookies,
        } = resolved.data
        else {
            panic!("expected remote source");
        };
        assert!(
            url.starts_with("https://www.klickaud.org/worker_sse.php?")
                || url.starts_with("https://dl.klickaud.org/"),
            "url: {url}"
        );
        let cookies = cookies.expect("klickaud delivery requires the browser session cookies");
        let client = reqwest::Client::builder()
            .user_agent(super::super::BROWSER_USER_AGENT)
            .build()
            .unwrap();
        let request = super::super::apply_media_headers(
            client
                .get(&url)
                .header(reqwest::header::RANGE, "bytes=0-1023")
                .header(reqwest::header::ACCEPT_ENCODING, "identity"),
            referer,
            Some(cookies.as_str()),
        );
        let response = request
            .send()
            .await
            .unwrap_or_else(|error| panic!("klickaud delivery failed: {error}"));
        assert_eq!(
            response.status(),
            reqwest::StatusCode::PARTIAL_CONTENT,
            "delivery status"
        );
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("audio/mpeg")
        );
        let bytes = response.bytes().await.unwrap();
        assert!(!bytes.is_empty(), "delivery body");
    }

    #[test]
    fn download_url_must_stay_on_klickaud() {
        assert!(
            validate_download_url("https://www.klickaud.org/worker_sse.php?delivery=x&name=y")
                .is_ok()
        );
        assert!(validate_download_url("https://dl.klickaud.org/download?token=x").is_ok());
        assert!(validate_download_url("https://klickaud.org/download?token=x").is_ok());
        assert!(validate_download_url("http://www.klickaud.org/worker_sse.php").is_err());
        assert!(validate_download_url("https://evil.example/download").is_err());
        assert!(validate_download_url("https://klickaud.org.evil.example/x").is_err());
    }

    #[test]
    fn track_url_prefers_the_canonical_permalink() {
        let track = test_track();
        let track_json = serde_json::json!({
            "permalink_url": "https://soundcloud.com/artist/canonical",
            "permalinkUrl": "https://soundcloud.com/artist/alternate",
        });
        assert_eq!(
            klickaud_track_url(&track, &track_json).unwrap(),
            "https://soundcloud.com/artist/canonical"
        );

        let invalid = serde_json::json!({
            "permalink_url": "https://example.com/not-soundcloud",
        });
        assert_eq!(
            klickaud_track_url(&track, &invalid).unwrap(),
            "https://soundcloud.com/artist/track"
        );
    }

    #[test]
    fn no_playable_transcoding_detects_encrypted_only_tracks() {
        use crate::playback::resolver::soundcloud_track_has_no_playable_transcoding;

        let encrypted_only = serde_json::json!({
            "policy": "MONETIZE",
            "media": {"transcodings": [
                {"format": {"protocol": "cbc-encrypted-hls", "mime_type": "audio/mp4"}, "preset": "aac_160k", "url": "https://api-v2.soundcloud.com/x"},
                {"format": {"protocol": "hls", "mime_type": "audio/mpeg"}, "preset": "mp3_1_0", "url": "https://api-v2.soundcloud.com/x"},
            ]},
        });
        assert!(soundcloud_track_has_no_playable_transcoding(
            &encrypted_only
        ));

        let with_progressive = serde_json::json!({
            "media": {"transcodings": [
                {"format": {"protocol": "progressive", "mime_type": "audio/mpeg"}, "preset": "mp3_1_0", "url": "https://api-v2.soundcloud.com/x"},
            ]},
        });
        assert!(!soundcloud_track_has_no_playable_transcoding(
            &with_progressive
        ));

        let with_plain_hls = serde_json::json!({
            "media": {"transcodings": [
                {"format": {"protocol": "hls", "mime_type": "audio/mp4; codecs=\"mp4a.40.2\""}, "preset": "aac_160k", "url": "https://api-v2.soundcloud.com/x"},
            ]},
        });
        assert!(!soundcloud_track_has_no_playable_transcoding(
            &with_plain_hls
        ));

        assert!(!soundcloud_track_has_no_playable_transcoding(
            &serde_json::json!({"media": {"transcodings": []}})
        ));
        assert!(!soundcloud_track_has_no_playable_transcoding(
            &serde_json::json!({})
        ));
    }

    use crate::playback::resolver::soundcloud_track_is_go_plus_gated;
    use crate::playback::{PlaybackProvider, PlaybackTrack};

    #[test]
    fn gate_requires_snip_and_all_transcodings_snipped() {
        let gated = serde_json::json!({
            "policy": "SNIP",
            "media": {"transcodings": [
                {"snipped": true},
                {"snipped": true},
            ]},
        });
        assert!(soundcloud_track_is_go_plus_gated(&gated));

        let monetize = serde_json::json!({
            "policy": "MONETIZE",
            "media": {"transcodings": [{"snipped": true}]},
        });
        assert!(!soundcloud_track_is_go_plus_gated(&monetize));

        let partly_unsnipped = serde_json::json!({
            "policy": "SNIP",
            "media": {"transcodings": [
                {"snipped": true},
                {"snipped": false},
            ]},
        });
        assert!(!soundcloud_track_is_go_plus_gated(&partly_unsnipped));

        let empty = serde_json::json!({
            "policy": "SNIP",
            "media": {"transcodings": []},
        });
        assert!(!soundcloud_track_is_go_plus_gated(&empty));

        let no_transcodings = serde_json::json!({"policy": "SNIP"});
        assert!(!soundcloud_track_is_go_plus_gated(&no_transcodings));
    }

    #[test]
    fn klickaud_source_never_pretends_to_be_soundcloud() {
        let track = test_track();
        let audio = klickaud_remote_audio(
            &track,
            "https://dl.klickaud.org/download?token=x".into(),
            Some("PHPSESSID=session".into()),
        );
        assert!(!audio.is_soundcloud);
        assert_eq!(audio.size, 0);
        assert_eq!(audio.declared_bitrate, Some(128));
        assert_eq!(audio.format, AudioFormat::Mp3);
        assert_eq!(
            audio.cache_identity.as_deref(),
            Some("soundcloud:track-id:standard:mp3:MP3")
        );
        match audio.data {
            SourceData::Remote {
                referer, cookies, ..
            } => {
                assert_eq!(referer, Some(KLICKAUD_REFERER));
                assert_eq!(cookies.as_deref(), Some("PHPSESSID=session"));
            }
            _ => panic!("expected a remote source"),
        }
    }

    fn test_track() -> PlaybackTrack {
        use std::time::Duration;
        PlaybackTrack {
            provider: PlaybackProvider::SoundCloud,
            id: "track-id".into(),
            title: "T".into(),
            artist: "A".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(200),
            downloadable: false,
            progressive: false,
            explicit: false,
            ai_generated: false,
            go_plus: false,
            service_url: "https://soundcloud.com/artist/track".into(),
        }
    }
}
