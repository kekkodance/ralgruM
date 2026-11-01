use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use reqwest::{Client, header::USER_AGENT};
use serde_json::Value;

use super::http::read_response_limited;
use crate::search::DEEZER_USER_AGENT;

pub(crate) const DESKTOP_CLIENT_ID: &str = "emAJdGEj1mm9yjoCD2jkixmgqrGIyfpi";
pub(crate) const MOBILE_CLIENT_ID: &str = "SSdQ80vM8nLPhbDBylHl2JFK6ElhBr9B";
pub(crate) const TOKEN_ENDPOINTS: [&str; 2] = [
    "https://secure.soundcloud.com/oauth/token",
    "https://api-auth.soundcloud.com/oauth/token",
];
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;
const REFRESH_MARGIN_SECONDS: i64 = 15 * 60;
const FALLBACK_TOKEN_LIFETIME_SECONDS: i64 = 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RefreshTokenRequest {
    pub(crate) refresh_token: String,
    pub(crate) client_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SoundCloudRefreshRequest {
    pub(crate) desktop: Option<RefreshTokenRequest>,
    pub(crate) mobile: Option<RefreshTokenRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RefreshedToken {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) expires_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SoundCloudRefreshCompletion {
    pub(crate) desktop: Option<RefreshedToken>,
    pub(crate) mobile: Option<RefreshedToken>,
    pub(crate) errors: Vec<String>,
}

impl SoundCloudRefreshCompletion {
    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Self {
            errors: vec![error.into()],
            ..Self::default()
        }
    }
}

/// A token is refreshed shortly before it expires so an in-flight request
/// cannot cross the expiry boundary while the refresh is still running.
pub(crate) fn refresh_due(expires_at: Option<i64>, now: i64) -> bool {
    expires_at.is_none_or(|expires_at| expires_at <= now.saturating_add(REFRESH_MARGIN_SECONDS))
}

pub(crate) async fn refresh_tokens(
    client: &Client,
    request: &SoundCloudRefreshRequest,
    now: i64,
) -> SoundCloudRefreshCompletion {
    let mut completion = SoundCloudRefreshCompletion::default();
    if let Some(desktop) = &request.desktop {
        match refresh_token(client, desktop, now).await {
            Ok(token) => completion.desktop = Some(token),
            Err(error) => completion.errors.push(format!("desktop session: {error}")),
        }
    }
    if let Some(mobile) = &request.mobile {
        match refresh_token(client, mobile, now).await {
            Ok(token) => completion.mobile = Some(token),
            Err(error) => completion.errors.push(format!("mobile session: {error}")),
        }
    }
    completion
}

async fn refresh_token(
    client: &Client,
    request: &RefreshTokenRequest,
    now: i64,
) -> Result<RefreshedToken, String> {
    let mut last_error = String::new();
    for endpoint in TOKEN_ENDPOINTS {
        let response = match token_request(client, endpoint, request).send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = error.to_string();
                continue;
            }
        };
        let status = response.status();
        let body = read_response_limited(response, MAX_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                format!("SoundCloud returned an unreadable token response: {error}")
            })?;
        let data = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        if status.is_success() {
            return parse_token_response(&data, &request.refresh_token, now);
        }
        last_error = format!(
            "{status}: {}",
            data.get("error_description")
                .or_else(|| data.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("request rejected")
        );
    }
    Err(format!("SoundCloud token refresh failed ({last_error})."))
}

fn token_request(
    client: &Client,
    endpoint: &str,
    request: &RefreshTokenRequest,
) -> reqwest::RequestBuilder {
    client
        .post(endpoint)
        .query(&[("client_id", request.client_id.as_str())])
        .header(USER_AGENT, DEEZER_USER_AGENT)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", request.refresh_token.as_str()),
            ("client_id", request.client_id.as_str()),
        ])
}

pub(crate) fn parse_token_response(
    data: &Value,
    previous_refresh_token: &str,
    now: i64,
) -> Result<RefreshedToken, String> {
    let access_token = data
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "SoundCloud did not return an OAuth token.".to_string())?;
    let refresh_token = data
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| previous_refresh_token.trim())
        .to_owned();
    if refresh_token.is_empty() {
        return Err("SoundCloud did not return a refresh token.".to_string());
    }
    let expires_at = data
        .get("expires_in")
        .and_then(|value| {
            value.as_i64().or_else(|| {
                value
                    .as_u64()
                    .map(|seconds| seconds.min(i64::MAX as u64) as i64)
            })
        })
        .map(|seconds| now.saturating_add(seconds.max(0)))
        .unwrap_or_else(|| token_expires_at(access_token, now));
    Ok(RefreshedToken {
        access_token: access_token.to_owned(),
        refresh_token,
        expires_at: Some(expires_at),
    })
}

/// Expiry for a token that was just issued. The response lifetime wins, then
/// the JWT claim, and finally the conservative one-hour fallback so a token
/// without readable metadata is still refreshed on a schedule.
pub(crate) fn token_expires_at(token: &str, now: i64) -> i64 {
    token_expiry(token).unwrap_or_else(|| now.saturating_add(FALLBACK_TOKEN_LIFETIME_SECONDS))
}

pub(crate) fn token_expiry(token: &str) -> Option<i64> {
    token_claims(token)?.get("exp").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|seconds| seconds as i64))
    })
}

pub(crate) fn token_client_id(token: &str) -> Option<String> {
    token_claims(token)?
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn token_claims(token: &str) -> Option<Value> {
    let token = token.trim();
    let token = token.strip_prefix("OAuth ").unwrap_or(token);
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        format!(
            "{}.{}.signature",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    #[test]
    fn reads_expiry_and_client_id_from_a_jwt() {
        let token = jwt(json!({"exp": 1_700_000_000_i64, "client_id": "web-client"}));
        assert_eq!(token_expiry(&token), Some(1_700_000_000));
        assert_eq!(token_client_id(&token).as_deref(), Some("web-client"));
        assert_eq!(token_expiry("opaque-token"), None);
        assert_eq!(token_client_id("opaque-token"), None);
    }

    #[test]
    fn token_expiry_falls_back_to_one_hour_when_metadata_is_missing() {
        let now = 1_700_000_000;
        assert_eq!(token_expires_at("opaque-token", now), now + 3_600);
        assert_eq!(
            token_expires_at(&jwt(json!({"exp": 1_700_000_123_i64})), now),
            1_700_000_123
        );
    }

    #[test]
    fn refresh_window_keeps_a_safety_margin() {
        let now = 1_700_000_000;
        assert!(refresh_due(None, now));
        assert!(refresh_due(Some(now), now));
        assert!(refresh_due(Some(now + REFRESH_MARGIN_SECONDS), now));
        assert!(!refresh_due(Some(now + REFRESH_MARGIN_SECONDS + 1), now));
    }

    #[test]
    fn token_response_rotates_the_refresh_token_and_records_expiry() {
        let token = parse_token_response(
            &json!({
                "access_token": "new-access",
                "refresh_token": "new-refresh",
                "expires_in": 3600
            }),
            "old-refresh",
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(token.access_token, "new-access");
        assert_eq!(token.refresh_token, "new-refresh");
        assert_eq!(token.expires_at, Some(1_700_003_600));
    }

    #[test]
    fn token_response_keeps_the_previous_refresh_token_when_not_rotated() {
        let token = parse_token_response(
            &json!({"access_token": "new-access", "expires_in": 60}),
            "old-refresh",
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(token.refresh_token, "old-refresh");
    }

    #[test]
    fn refresh_request_carries_only_the_public_client_id_and_refresh_token() {
        let client = Client::new();
        let request = token_request(
            &client,
            TOKEN_ENDPOINTS[0],
            &RefreshTokenRequest {
                refresh_token: "refresh-sentinel".into(),
                client_id: "client-sentinel".into(),
            },
        )
        .build()
        .unwrap();

        assert_eq!(request.method(), Method::POST);
        let query = request.url().query_pairs().collect::<Vec<_>>();
        assert_eq!(query, vec![("client_id".into(), "client-sentinel".into())]);
        assert!(
            request
                .url()
                .as_str()
                .starts_with("https://secure.soundcloud.com/oauth/token?")
        );
        let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
        let body = std::str::from_utf8(body).unwrap();
        assert!(body.contains("grant_type=refresh_token"));
        assert!(body.contains("refresh_token=refresh-sentinel"));
        assert!(body.contains("client_id=client-sentinel"));
        assert!(!body.contains("client_secret"));
    }
}
