use std::time::Duration;

use reqwest::{
    Client, Method, RequestBuilder, StatusCode, Url,
    header::{self, HeaderValue},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::StreamResolver;
use super::{
    AudioFormat, RemoteAudio, SourceData, SourceVariant, response_cookies, stable_cache_identity,
};
use crate::playback::retry::{RequestClass, send_with_retry};

const KLICKAUD_ORIGIN: &str = "https://www.klickaud.org";
const KLICKAUD_REFERER: &str = "https://www.klickaud.org/";
const KLICKAUD_DOWNLOAD_REFERER: &str = "https://www.klickaud.org/download.php";
const KLICKAUD_EN17_REFERER: &str = "https://www.klickaud.org/en17/";
/// Browser User-Agent on every request; the service rejects bare clients.
const KLICKAUD_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// Total budget for the worker SSE stream.
const SSE_BUDGET: Duration = Duration::from_secs(30);
/// Retry attempts for the overall flow when the service misbehaves early.
const FLOW_ATTEMPTS: u32 = 2;

pub(super) struct KlickaudOutcome {
    pub(super) download_url: String,
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
            match self.klickaud_flow(&web_url, cancellation).await {
                Ok(outcome) => {
                    crate::diagnostics::event("INFO", "klickaud resolve outcome=ready");
                    return Ok(klickaud_remote_audio(track, outcome.download_url));
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

    async fn klickaud_flow(
        &self,
        web_url: &str,
        cancellation: &CancellationToken,
    ) -> Result<KlickaudOutcome, String> {
        // One cookie-jar session across steps 1-4. A fresh client without a
        // cookie store loses the CSRF cookie and download.php answers 302.
        let session = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(SSE_BUDGET)
            .build()
            .map_err(|_| "The klickaud session could not be created".to_string())?;

        let (csrf_token, cookies) = self.klickaud_csrf(&session, cancellation).await?;
        let (grant, mode) = self
            .klickaud_grant(&session, web_url, &csrf_token, &cookies, cancellation)
            .await?;
        if mode != "worker" {
            return Err(format!("klickaud downloadMode={mode} is unsupported"));
        }
        let capability = self
            .klickaud_capability(&session, &grant, web_url, &cookies, cancellation)
            .await?;
        let download_url = self
            .klickaud_worker_sse(&session, web_url, &capability, &cookies, cancellation)
            .await?;
        Ok(KlickaudOutcome { download_url })
    }

    async fn klickaud_csrf(
        &self,
        session: &Client,
        cancellation: &CancellationToken,
    ) -> Result<(String, String), String> {
        let response = send_with_retry(
            "klickaud.csrf",
            RequestClass::Provider,
            cancellation,
            || {
                klickaud_get(
                    session,
                    format!("{KLICKAUD_ORIGIN}/csrf-token-endpoint.php"),
                )
                .header(header::REFERER, KLICKAUD_EN17_REFERER)
                .header(header::ACCEPT, "application/json")
            },
        )
        .await?;
        let cookies = response_cookies(&response);
        let token = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("csrf_token")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|token| !token.is_empty() && token.len() <= 128)
            .ok_or_else(|| "klickaud returned no csrf token".to_string())?;
        crate::diagnostics::event("INFO", "klickaud csrf token acquired");
        Ok((token, cookies))
    }

    async fn klickaud_grant(
        &self,
        session: &Client,
        web_url: &str,
        csrf_token: &str,
        cookies: &str,
        cancellation: &CancellationToken,
    ) -> Result<(String, String), String> {
        let body = format!(
            "value={}&csrf_token={}",
            urlencoding_min(web_url),
            urlencoding_min(csrf_token)
        );
        let response = send_with_retry(
            "klickaud.download",
            RequestClass::Provider,
            cancellation,
            || {
                klickaud_post(session, format!("{KLICKAUD_ORIGIN}/download.php"), &body)
                    .header(header::REFERER, KLICKAUD_EN17_REFERER)
                    .header(header::COOKIE, cookies)
            },
        )
        .await?;
        if response.status() == StatusCode::FOUND
            || response.status() == StatusCode::MOVED_PERMANENTLY
        {
            // The session cookie was not honored; retrying with a fresh
            // session sometimes recovers it.
            return Err("klickaud download.php redirected".into());
        }
        if !response.status().is_success() {
            return Err(format!(
                "klickaud download.php status={}",
                response.status()
            ));
        }
        let html = response
            .text()
            .await
            .map_err(|_| "klickaud download.php response could not be read".to_string())?;
        let grant = extract_html_assignment(&html, "sseGrant")
            .ok_or_else(|| "klickaud did not provide a grant".to_string())?;
        let mode = extract_html_assignment(&html, "downloadMode").unwrap_or_default();
        crate::diagnostics::event("INFO", format!("klickaud grant acquired mode={mode}"));
        Ok((grant, mode))
    }

    async fn klickaud_capability(
        &self,
        session: &Client,
        grant: &str,
        web_url: &str,
        cookies: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let body = format!(
            "{{\"grant\":\"{}\",\"url\":\"{}\"}}",
            json_escape(grant),
            json_escape(web_url)
        );
        let response = send_with_retry(
            "klickaud.capability",
            RequestClass::Provider,
            cancellation,
            || {
                klickaud_post(
                    session,
                    format!("{KLICKAUD_ORIGIN}/sse_capability.php"),
                    &body,
                )
                .header(header::REFERER, KLICKAUD_DOWNLOAD_REFERER)
                .header(header::COOKIE, cookies)
            },
        )
        .await?;
        let capability = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|value| {
                value
                    .get("capability")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|capability| !capability.is_empty() && capability.len() <= 256)
            .ok_or_else(|| "klickaud returned no capability".to_string())?;
        crate::diagnostics::event("INFO", "klickaud capability acquired");
        Ok(capability)
    }

    async fn klickaud_worker_sse(
        &self,
        session: &Client,
        web_url: &str,
        capability: &str,
        cookies: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, String> {
        let url = format!(
            "{KLICKAUD_ORIGIN}/worker_sse.php?url={}&cap={}",
            urlencoding_min(web_url),
            urlencoding_min(capability)
        );
        let request = klickaud_get(session, url)
            .header(header::ACCEPT, "text/event-stream")
            .header(header::REFERER, KLICKAUD_DOWNLOAD_REFERER)
            .header(header::COOKIE, cookies);
        let mut response = tokio::select! {
            _ = cancellation.cancelled() => return Err("Playback request cancelled".into()),
            response = request.send() => response
                .map_err(|_| "klickaud worker could not be reached".to_string())?,
        };
        if !response.status().is_success() {
            return Err(format!("klickaud worker status={}", response.status()));
        }
        let mut parser = SseParser::default();
        let deadline = tokio::time::Instant::now() + SSE_BUDGET;
        let mut stream = response.chunk();
        loop {
            let chunk = tokio::select! {
                _ = cancellation.cancelled() => {
                    return Err("Playback request cancelled".into());
                }
                _ = tokio::time::sleep_until(deadline) => {
                    return Err("klickaud worker timed out".into());
                }
                chunk = stream => chunk
                    .map_err(|_| "klickaud worker stream failed".to_string())?,
            };
            let Some(chunk) = chunk else {
                return Err("klickaud worker stream ended".into());
            };
            match parser.push(&chunk) {
                SseEvent::None => {}
                SseEvent::Ready { download_url } => {
                    let download_url = download_url
                        .filter(|url| Url::parse(url).is_ok_and(|url| url.scheme() == "https"))
                        .ok_or_else(|| "klickaud ready event had no download url".to_string())?;
                    return Ok(download_url);
                }
                SseEvent::Failed => {
                    return Err("klickaud worker reported failure".into());
                }
            }
            stream = response.chunk();
        }
    }
}

/// Parses the `name = "value"` assignments klickaud embeds in download.php.
fn extract_html_assignment(html: &str, name: &str) -> Option<String> {
    let needle = format!("{name} = \"");
    let start = html.find(&needle)? + needle.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    let value = &rest[..end];
    (!value.is_empty() && value.len() <= 256).then(|| value.to_owned())
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Minimal percent-encoding for query and form values.
fn urlencoding_min(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
        }
    }
    encoded
}

fn klickaud_get(session: &Client, url: String) -> RequestBuilder {
    session.request(Method::GET, url).header(
        header::USER_AGENT,
        HeaderValue::from_static(KLICKAUD_USER_AGENT),
    )
}

fn klickaud_post(session: &Client, url: String, body: &str) -> RequestBuilder {
    session
        .request(Method::POST, url)
        .header(
            header::USER_AGENT,
            HeaderValue::from_static(KLICKAUD_USER_AGENT),
        )
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        )
        .body(body.to_owned())
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

fn klickaud_remote_audio(track: &super::PlaybackTrack, download_url: String) -> RemoteAudio {
    RemoteAudio {
        data: SourceData::Remote {
            url: download_url,
            referer: Some(KLICKAUD_REFERER),
        },
        size: 0,
        deezer_track_id: None,
        format: AudioFormat::Mp3,
        format_name: "MP3".to_owned(),
        declared_bitrate: Some(128),
        // dl.klickaud.org is not a SoundCloud host; the sndcdn validators
        // must never see it.
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

#[derive(Default)]
struct SseParser {
    buffer: Vec<u8>,
    event: String,
    data_lines: Vec<String>,
    saw_event_line: bool,
}

enum SseEvent {
    None,
    Ready { download_url: Option<String> },
    Failed,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> SseEvent {
        self.buffer.extend_from_slice(chunk);
        while let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if let Some(event) = self.take_event() {
                    return event;
                }
                continue;
            }
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match field {
                "event" => {
                    self.event = value.to_owned();
                    self.saw_event_line = true;
                }
                "data" => self.data_lines.push(value.to_owned()),
                _ => {}
            }
        }
        SseEvent::None
    }

    fn take_event(&mut self) -> Option<SseEvent> {
        if !self.saw_event_line && self.data_lines.is_empty() {
            return None;
        }
        let event = std::mem::take(&mut self.event);
        let data_lines = std::mem::take(&mut self.data_lines);
        self.saw_event_line = false;
        let data = data_lines.join("\n");
        match event.as_str() {
            "ready" => Some(SseEvent::Ready {
                download_url: serde_json::from_str::<Value>(&data).ok().and_then(|value| {
                    value
                        .get("download_url")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
            }),
            "failed" => Some(SseEvent::Failed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn sse_parser_handles_progress_ready_and_failed() {
        let mut parser = SseParser::default();
        assert!(matches!(
            parser.push(b"event: progress\ndata: {\"pct\":0}\n\n"),
            SseEvent::None
        ));
        assert!(matches!(
            parser.push(b"event: ready\ndata: {\"download_url\":\"https://dl.klickaud.org/download?token=abc\"}\n\n"),
            SseEvent::Ready { download_url: Some(url) } if url == "https://dl.klickaud.org/download?token=abc"
        ));
        assert!(matches!(
            parser.push(b"event: failed\ndata: {\"reason\":\"x\"}\n\n"),
            SseEvent::Failed
        ));
    }

    #[test]
    fn sse_parser_handles_partial_chunks_and_multi_line_data() {
        let mut parser = SseParser::default();
        assert!(matches!(parser.push(b"event: rea"), SseEvent::None));
        assert!(matches!(
            parser.push(b"dy\ndata: {\"download"),
            SseEvent::None
        ));
        assert!(matches!(
            parser.push(b"_url\":\"https://dl.klickaud.org/download?token=split\"}\n\n"),
            SseEvent::Ready {
                download_url: Some(url)
            } if url == "https://dl.klickaud.org/download?token=split"
        ));

        // Multiple data lines are joined with newlines before parsing; JSON
        // whitespace keeps the value valid.
        let mut multi = SseParser::default();
        assert!(matches!(
            multi.push(b"event: ready\ndata: {\"download_url\":\ndata: \"https://dl.klickaud.org/x\"}\n\n"),
            SseEvent::Ready {
                download_url: Some(url)
            } if url == "https://dl.klickaud.org/x"
        ));
    }

    #[test]
    fn html_assignments_are_extracted_with_bounds() {
        let html = r#"var sseGrant = "a".repeat(43).valueOf(); downloadMode = "worker";"#;
        assert_eq!(
            extract_html_assignment(html, "downloadMode"),
            Some("worker".into())
        );
        assert_eq!(extract_html_assignment(html, "missing"), None);
    }

    #[test]
    fn klickaud_source_never_pretends_to_be_soundcloud() {
        let track = test_track();
        let audio =
            klickaud_remote_audio(&track, "https://dl.klickaud.org/download?token=x".into());
        assert!(!audio.is_soundcloud);
        assert_eq!(audio.size, 0);
        assert_eq!(audio.declared_bitrate, Some(128));
        assert_eq!(audio.format, AudioFormat::Mp3);
        assert_eq!(
            audio.cache_identity.as_deref(),
            Some("soundcloud:track-id:standard:mp3:MP3")
        );
        match audio.data {
            SourceData::Remote { referer, .. } => assert_eq!(referer, Some(KLICKAUD_REFERER)),
            _ => panic!("expected a remote source"),
        }
    }

    #[test]
    fn percent_encoding_covers_the_track_url() {
        assert_eq!(
            urlencoding_min("https://soundcloud.com/a b/c?d=e&f"),
            "https%3A%2F%2Fsoundcloud.com%2Fa%20b%2Fc%3Fd%3De%26f"
        );
        assert_eq!(urlencoding_min("abc-DEF_123~."), "abc-DEF_123~.");
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
            service_url: "https://soundcloud.com/artist/track".into(),
        }
    }
}
