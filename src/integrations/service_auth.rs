use futures::StreamExt as _;
use reqwest::header::{
    AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, USER_AGENT,
};
use serde_json::Value;

use crate::account_session::ServiceIdentity;
use crate::search::{DeezerArl, SoundCloudToken};
const DEEZER_USER_DATA_URL: &str = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
const DEEZER_PROFILE_IMAGE_ORIGIN: &str = "https://cdn-images.dzcdn.net";
const DEEZER_PROFILE_IMAGE_SIZE: &str = "100x100-000000-80-0-0.jpg";
const SOUNDCLOUD_API: &str = "https://api-v2.soundcloud.com/me";
// These OAuth validation IDs are tied to the desktop and mobile token flows.
// Playback and catalog API requests use the shared search client ID instead.
const SOUNDCLOUD_DESKTOP_CLIENT_ID: &str = "emAJdGEj1mm9yjoCD2jkixmgqrGIyfpi";
const SOUNDCLOUD_MOBILE_CLIENT_ID: &str = "SSdQ80vM8nLPhbDBylHl2JFK6ElhBr9B";
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
const MAX_DEEZER_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SOUNDCLOUD_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_SOUNDCLOUD_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Service {
    Deezer,
    SoundCloud,
}

#[derive(Debug)]
pub(crate) struct ValidatedCredentials {
    pub(crate) desktop: String,
    pub(crate) mobile: Option<String>,
    pub(crate) soundcloud_cookies: Option<String>,
    pub(crate) deezer_user_id: Option<String>,
    pub(crate) identity: Option<ServiceIdentity>,
}

#[derive(Debug)]
struct SoundCloudAuthenticatedUser {
    identity: ServiceIdentity,
    stable_id: String,
}

pub(crate) async fn web_login(
    client: &reqwest::Client,
    service: Service,
) -> Result<ValidatedCredentials, String> {
    let captured = crate::service_auth_webview::login(service).await?;
    match service {
        Service::Deezer => validate(client, service, captured.desktop, None, None).await,
        Service::SoundCloud => {
            let desktop = SoundCloudToken::from_saved(&captured.desktop)
                .ok_or_else(|| "SoundCloud returned an invalid desktop OAuth token.".to_string())?;
            let desktop_user =
                validate_soundcloud_token(client, &desktop, SOUNDCLOUD_DESKTOP_CLIENT_ID).await?;
            let authorization = captured
                .mobile_authorization
                .ok_or_else(|| "SoundCloud did not return a mobile authorization.".to_string())?;
            let mobile = exchange_soundcloud_mobile_token(
                client,
                &authorization.code,
                &authorization.verifier,
            )
            .await?;
            let mobile = SoundCloudToken::from_saved(&mobile)
                .ok_or_else(|| "SoundCloud returned an invalid mobile OAuth token.".to_string())?;
            let mobile_user =
                validate_soundcloud_token(client, &mobile, SOUNDCLOUD_MOBILE_CLIENT_ID).await?;
            let identity = soundcloud_pair_identity(desktop_user, mobile_user)?;
            Ok(ValidatedCredentials {
                desktop: captured.desktop,
                mobile: Some(mobile.expose().to_owned()),
                soundcloud_cookies: captured.soundcloud_cookies,
                deezer_user_id: None,
                identity: Some(identity),
            })
        }
    }
}

pub(crate) async fn validate(
    client: &reqwest::Client,
    service: Service,
    desktop: String,
    mobile: Option<String>,
    user_id: Option<String>,
) -> Result<ValidatedCredentials, String> {
    match service {
        Service::Deezer => validate_deezer(client, desktop, user_id).await,
        Service::SoundCloud => validate_soundcloud(client, desktop, mobile).await,
    }
}

async fn validate_deezer(
    client: &reqwest::Client,
    desktop: String,
    user_id: Option<String>,
) -> Result<ValidatedCredentials, String> {
    let arl = DeezerArl::from_saved(&desktop)
        .ok_or_else(|| "Enter a valid Deezer ARL cookie.".to_string())?;
    let response = deezer_validation_request(
        client,
        arl.cookie_header()
            .map_err(|_| "The Deezer session is invalid")?,
    )
    .send()
    .await
    .map_err(|error| format!("Could not validate the Deezer session: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_owned();
    let content_encoding = response
        .headers()
        .get(CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("identity")
        .to_owned();
    let body = read_response_limited(response, MAX_DEEZER_RESPONSE_BYTES)
        .await
        .map_err(|error| {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "deezer account response status={status} content_type={content_type} content_encoding={content_encoding} body_length=0 body_read_error"
                ),
            );
            format!(
                "Could not read the Deezer account response ({status}, {content_type}): {error}"
            )
        })?;
    let body_length = body.len();
    let result = parse_deezer_profile_response(status, &content_type, &body);
    let profile = match result {
        Ok(value) => value,
        Err(error) => {
            let prefix = safe_response_prefix(&content_type, &body);
            crate::diagnostics::event(
                "WARN",
                format!(
                    "deezer account response status={status} content_type={content_type} content_encoding={content_encoding} body_length={body_length}{}",
                    prefix.map_or(String::new(), |prefix| format!(" safe_prefix={prefix}"))
                ),
            );
            return Err(error);
        }
    };
    if let Some(expected) = user_id.filter(|value| !value.trim().is_empty())
        && expected.trim() != profile.user_id
    {
        return Err("The Deezer user ID does not match the ARL session.".into());
    }
    Ok(ValidatedCredentials {
        desktop: arl.expose().to_owned(),
        mobile: None,
        soundcloud_cookies: None,
        deezer_user_id: Some(profile.user_id),
        identity: profile.identity,
    })
}

async fn read_response_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!("response body exceeded the {limit}-byte limit"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|error| format!("response body read failed: {error}"))?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(format!("response body exceeded the {limit}-byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn deezer_validation_request(
    client: &reqwest::Client,
    cookie: reqwest::header::HeaderValue,
) -> reqwest::RequestBuilder {
    client
        .post(DEEZER_USER_DATA_URL)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .header(COOKIE, cookie)
        .header(CONTENT_LENGTH, "0")
        .body("")
}

#[cfg(test)]
fn parse_deezer_account_response(
    status: reqwest::StatusCode,
    content_type: &str,
    body: &[u8],
) -> Result<String, String> {
    parse_deezer_profile_response(status, content_type, body).map(|profile| profile.user_id)
}

#[derive(Debug, Eq, PartialEq)]
struct DeezerProfile {
    user_id: String,
    identity: Option<ServiceIdentity>,
}

fn parse_deezer_profile_response(
    status: reqwest::StatusCode,
    content_type: &str,
    body: &[u8],
) -> Result<DeezerProfile, String> {
    let text = std::str::from_utf8(body).map_err(|error| {
        format!(
            "Deezer returned an unreadable account response ({status}, {content_type}); response text was not UTF-8: {error}"
        )
    })?;
    if !status.is_success() {
        let reason = if text.trim_start().starts_with('<') {
            "the provider returned HTML, likely a login, rate-limit, or gateway page"
        } else {
            "the provider rejected the session"
        };
        return Err(format!(
            "Deezer rejected the session ({status}, {content_type}): {reason}. Try signing in again."
        ));
    }
    if text.trim_start().starts_with('<') {
        return Err(format!(
            "Deezer returned HTML instead of account JSON ({content_type}). The provider may be rate-limiting this request; try signing in again."
        ));
    }
    let data: Value = serde_json::from_str(text).map_err(|error| {
        format!(
            "Deezer returned an unreadable account response ({status}, {content_type}); expected JSON: {error}"
        )
    })?;
    let user = data.pointer("/results/USER").ok_or_else(|| {
        "Deezer did not return a signed-in user in the expected account response.".to_string()
    })?;
    let user_id = user
        .get("USER_ID")
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|id| id.to_string()))
        })
        .filter(|value| value != "0" && !value.is_empty())
        .ok_or_else(|| {
            "Deezer did not return a signed-in user in the expected account response.".to_string()
        })?;
    let username = nonempty_string(user.get("BLOG_NAME")).filter(|name| is_public_name(name));
    let avatar_url = deezer_profile_image_url(user.get("USER_PICTURE"));
    Ok(DeezerProfile {
        user_id,
        identity: username.and_then(|username| ServiceIdentity::new(username, avatar_url)),
    })
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn is_public_name(value: &str) -> bool {
    !value.chars().all(|character| character.is_ascii_digit())
}

fn profile_image_url(value: Option<&Value>) -> Option<String> {
    let value = nonempty_string(value)?;
    if value.len() == 32 && value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Some(format!(
            "{DEEZER_PROFILE_IMAGE_ORIGIN}/images/user/{}/{DEEZER_PROFILE_IMAGE_SIZE}",
            value.to_ascii_lowercase()
        ));
    }
    let parsed = url::Url::parse(&value).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(value)
}

fn deezer_profile_image_url(value: Option<&Value>) -> Option<String> {
    let value = nonempty_string(value)?;
    if value.len() != 32 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{DEEZER_PROFILE_IMAGE_ORIGIN}/images/user/{}/{DEEZER_PROFILE_IMAGE_SIZE}",
        value.to_ascii_lowercase()
    ))
}

fn safe_response_prefix(content_type: &str, body: &[u8]) -> Option<String> {
    if !content_type.to_ascii_lowercase().starts_with("text/html") {
        return None;
    }
    let prefix = std::str::from_utf8(body)
        .ok()?
        .trim()
        .chars()
        .take(96)
        .collect::<String>();
    let lower = prefix.to_ascii_lowercase();
    if !(lower.starts_with("<!doctype html") || lower.starts_with("<html"))
        || [
            "arl",
            "cookie",
            "authorization",
            "token",
            "password",
            "user_id",
            "results",
        ]
        .iter()
        .any(|secret| lower.contains(secret))
    {
        return None;
    }
    Some("<!doctype html>".into())
}

async fn validate_soundcloud(
    client: &reqwest::Client,
    desktop: String,
    mobile: Option<String>,
) -> Result<ValidatedCredentials, String> {
    let desktop = SoundCloudToken::from_saved(&desktop)
        .ok_or_else(|| "Enter a valid SoundCloud desktop OAuth token.".to_string())?;
    let desktop_user =
        validate_soundcloud_token(client, &desktop, SOUNDCLOUD_DESKTOP_CLIENT_ID).await?;
    let mobile = mobile
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Enter a valid SoundCloud mobile OAuth token.".to_string())
        .and_then(|value| {
            SoundCloudToken::from_saved(&value)
                .ok_or_else(|| "Enter a valid SoundCloud mobile OAuth token.".to_string())
        })?;
    // Both credentials must authenticate the same stable SoundCloud account.
    // The mobile response is deliberately not persisted as a second display
    // identity; the desktop profile remains the canonical settings identity.
    let mobile_user =
        validate_soundcloud_token(client, &mobile, SOUNDCLOUD_MOBILE_CLIENT_ID).await?;
    let identity = soundcloud_pair_identity(desktop_user, mobile_user)?;
    Ok(ValidatedCredentials {
        desktop: desktop.expose().to_owned(),
        mobile: Some(mobile.expose().to_owned()),
        soundcloud_cookies: None,
        deezer_user_id: None,
        identity: Some(identity),
    })
}

pub(crate) async fn soundcloud_identity(
    client: &reqwest::Client,
    token: &SoundCloudToken,
) -> Result<ServiceIdentity, String> {
    validate_soundcloud_token(client, token, SOUNDCLOUD_DESKTOP_CLIENT_ID)
        .await
        .map(|user| user.identity)
}

pub(crate) async fn deezer_identity(
    client: &reqwest::Client,
    arl: &DeezerArl,
) -> Result<ServiceIdentity, String> {
    validate_deezer(client, arl.expose().to_owned(), None)
        .await?
        .identity
        .ok_or_else(|| "Deezer did not return a public account profile.".into())
}

fn soundcloud_stable_id(profile: &Value) -> Option<String> {
    profile
        .get("id")
        .and_then(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .or_else(|| value.as_u64().map(|id| id.to_string()))
        })
        .filter(|id| id != "0")
}

fn soundcloud_pair_identity(
    desktop: SoundCloudAuthenticatedUser,
    mobile: SoundCloudAuthenticatedUser,
) -> Result<ServiceIdentity, String> {
    if desktop.stable_id != mobile.stable_id {
        return Err(
            "The SoundCloud desktop and mobile sessions belong to different accounts.".into(),
        );
    }
    Ok(desktop.identity)
}

async fn validate_soundcloud_token(
    client: &reqwest::Client,
    token: &SoundCloudToken,
    client_id: &str,
) -> Result<SoundCloudAuthenticatedUser, String> {
    let response = client
        .get(SOUNDCLOUD_API)
        .query(&[("client_id", client_id)])
        .header(
            AUTHORIZATION,
            token
                .authorization_header()
                .map_err(|_| "The SoundCloud session is invalid")?,
        )
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .await
        .map_err(|error| format!("Could not validate the SoundCloud session: {error}"))?;
    if response.status().is_success() {
        let body = read_response_limited(response, MAX_SOUNDCLOUD_RESPONSE_BYTES)
            .await
            .map_err(|error| format!("SoundCloud returned an unreadable profile: {error}"))?;
        let profile: Value = serde_json::from_slice(&body)
            .map_err(|error| format!("SoundCloud returned an unreadable profile: {error}"))?;
        let stable_id = soundcloud_stable_id(&profile).ok_or_else(|| {
            "SoundCloud returned an invalid authenticated user identity.".to_string()
        })?;
        let username = profile
            .get("username")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|username| !username.is_empty())
            .ok_or_else(|| "SoundCloud returned an invalid authenticated user.".to_string())?;
        let avatar_url = profile_image_url(profile.get("avatar_url"));
        let identity = ServiceIdentity::new(username, avatar_url)
            .ok_or_else(|| "SoundCloud returned an invalid authenticated user.".to_owned())?;
        Ok(SoundCloudAuthenticatedUser {
            identity,
            stable_id,
        })
    } else {
        Err(format!(
            "SoundCloud rejected the session ({}).",
            response.status()
        ))
    }
}

async fn exchange_soundcloud_mobile_token(
    client: &reqwest::Client,
    code: &str,
    verifier: &str,
) -> Result<String, String> {
    const MOBILE_CLIENT_ID: &str = "SSdQ80vM8nLPhbDBylHl2JFK6ElhBr9B";
    const REDIRECT_URI: &str = "sc://auth";
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", REDIRECT_URI),
        ("client_id", MOBILE_CLIENT_ID),
    ];
    let mut last_error = String::new();
    for endpoint in [
        "https://secure.soundcloud.com/oauth/token",
        "https://api-auth.soundcloud.com/oauth/token",
    ] {
        let response = client
            .post(endpoint)
            .query(&[("client_id", MOBILE_CLIENT_ID)])
            .header(USER_AGENT, USER_AGENT_VALUE)
            .form(&form)
            .send()
            .await
            .map_err(|error| {
                format!("Could not exchange the SoundCloud mobile session: {error}")
            })?;
        let status = response.status();
        let body = read_response_limited(response, MAX_SOUNDCLOUD_TOKEN_RESPONSE_BYTES)
            .await
            .map_err(|error| {
                format!("SoundCloud returned an unreadable token response: {error}")
            })?;
        let data = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        if status.is_success() {
            let token = data
                .get("access_token")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "SoundCloud did not return a mobile OAuth token.".to_string())?;
            return SoundCloudToken::from_saved(token)
                .map(|token| token.expose().to_owned())
                .ok_or_else(|| "SoundCloud returned an invalid mobile OAuth token.".to_string());
        }
        last_error = format!(
            "{status}: {}",
            data.get("error_description")
                .or_else(|| data.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("request rejected")
        );
    }
    Err(format!(
        "SoundCloud mobile token exchange failed ({last_error})."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::{Client, Method, StatusCode, header};

    #[test]
    fn services_are_distinct() {
        assert_ne!(Service::Deezer, Service::SoundCloud);
    }

    #[test]
    fn deezer_validation_request_matches_empty_post_contract() {
        let arl = DeezerArl::from_saved("credential-sentinel").unwrap();
        let request = deezer_validation_request(&Client::new(), arl.cookie_header().unwrap())
            .build()
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.url().as_str(), DEEZER_USER_DATA_URL);
        assert_eq!(request.headers()[header::COOKIE], "arl=credential-sentinel");
        assert!(request.headers()[header::COOKIE].is_sensitive());
        assert_eq!(request.headers()[header::CONTENT_LENGTH], "0");
        assert!(!request.headers().contains_key(header::CONTENT_TYPE));
        assert_eq!(
            request.body().and_then(reqwest::Body::as_bytes),
            Some(&b""[..])
        );
        assert!(!format!("{request:?}").contains("credential-sentinel"));
    }

    #[test]
    fn parses_deezer_account_json_fixtures() {
        let string_id = br#"{"results":{"USER":{"USER_ID":"12345"}}}"#;
        let numeric_id = br#"{"results":{"USER":{"USER_ID":67890}}}"#;
        assert_eq!(
            parse_deezer_account_response(StatusCode::OK, "application/json", string_id),
            Ok("12345".into())
        );
        assert_eq!(
            parse_deezer_account_response(StatusCode::OK, "application/json", numeric_id),
            Ok("67890".into())
        );
    }

    #[test]
    fn parses_provider_identity_fields_without_using_deezer_id_as_name() {
        let body = br#"{
            "results": {
                "USER": {
                    "USER_ID": "12345",
                    "BLOG_NAME": "deezer-listener",
                    "USER_PICTURE": "0123456789ABCDEF0123456789abcdef"
                }
            }
        }"#;
        let profile =
            parse_deezer_profile_response(StatusCode::OK, "application/json", body).unwrap();
        assert_eq!(profile.user_id, "12345");
        let identity = profile.identity.unwrap();
        assert_eq!(identity.username, "deezer-listener");
        assert_eq!(
            identity.avatar_url,
            "https://cdn-images.dzcdn.net/images/user/0123456789abcdef0123456789abcdef/100x100-000000-80-0-0.jpg"
        );
        assert!(
            !identity
                .username
                .chars()
                .all(|character| character.is_ascii_digit())
        );
    }

    #[test]
    fn parses_captured_deezer_profile_fields_and_builds_the_safe_avatar_url() {
        let body = br#"{
            "results": {
                "USER": {
                    "USER_ID": 67890,
                    "BLOG_NAME": "profile-name",
                    "USER_PICTURE": "0123456789ABCDEF0123456789abcdef"
                }
            }
        }"#;
        let profile =
            parse_deezer_profile_response(StatusCode::OK, "application/json", body).unwrap();

        assert_eq!(profile.user_id, "67890");
        let identity = profile.identity.unwrap();
        assert_eq!(identity.username, "profile-name");
        assert_eq!(
            identity.avatar_url,
            "https://cdn-images.dzcdn.net/images/user/0123456789abcdef0123456789abcdef/100x100-000000-80-0-0.jpg"
        );
    }

    #[test]
    fn ignores_numeric_blog_names_and_does_not_fall_back_to_other_fields() {
        let body = br#"{
            "results": {
                "USER": {
                    "USER_ID": "67890",
                    "BLOG_NAME": "67890",
                    "DISPLAY_NAME": "profile-name"
                }
            }
        }"#;
        let profile =
            parse_deezer_profile_response(StatusCode::OK, "application/json", body).unwrap();

        assert!(profile.identity.is_none());
    }

    #[test]
    fn deezer_profile_image_url_requires_a_strict_hash() {
        assert_eq!(
            deezer_profile_image_url(Some(&serde_json::json!("0123456789ABCDEF0123456789abcdef")))
                .as_deref(),
            Some(
                "https://cdn-images.dzcdn.net/images/user/0123456789abcdef0123456789abcdef/100x100-000000-80-0-0.jpg"
            )
        );
        assert!(
            deezer_profile_image_url(Some(&serde_json::json!("0123456789abcdef0123456789abcde")))
                .is_none()
        );
        assert!(
            deezer_profile_image_url(Some(&serde_json::json!("0123456789abcdef0123456789abcdeg")))
                .is_none()
        );
        assert!(
            deezer_profile_image_url(Some(&serde_json::json!(
                "https://cdn-images.dzcdn.net/avatar.jpg"
            )))
            .is_none()
        );
    }

    #[test]
    fn profile_image_urls_reject_embedded_credentials_and_queries() {
        assert_eq!(
            profile_image_url(Some(&serde_json::json!("0123456789ABCDEF0123456789abcdef")))
                .as_deref(),
            Some(
                "https://cdn-images.dzcdn.net/images/user/0123456789abcdef0123456789abcdef/100x100-000000-80-0-0.jpg"
            )
        );
        assert!(
            profile_image_url(Some(&serde_json::json!("https://cdn.example/avatar.jpg"))).is_some()
        );
        assert!(
            profile_image_url(Some(&serde_json::json!(
                "https://user:secret@cdn.example/avatar.jpg"
            )))
            .is_none()
        );
        assert!(
            profile_image_url(Some(&serde_json::json!(
                "https://cdn.example/avatar.jpg?token=secret"
            )))
            .is_none()
        );
        assert!(profile_image_url(Some(&serde_json::json!("not-a-picture-hash"))).is_none());
        assert!(
            profile_image_url(Some(&serde_json::json!("0123456789abcdef0123456789abcde")))
                .is_none()
        );
    }

    #[test]
    fn reports_deezer_html_and_invalid_json_without_echoing_bodies() {
        let html = b"<!doctype html><html><title>Too Many Requests</title></html>";
        let html_error =
            parse_deezer_account_response(StatusCode::TOO_MANY_REQUESTS, "text/html", html)
                .unwrap_err();
        assert!(html_error.contains("429 Too Many Requests"));
        assert!(html_error.contains("rate-limit"));
        assert!(!html_error.contains("<title>"));
        assert_eq!(
            safe_response_prefix("text/html; charset=utf-8", html).as_deref(),
            Some("<!doctype html>")
        );

        let invalid = b"not-json-provider-sentinel";
        let decode_error =
            parse_deezer_account_response(StatusCode::OK, "application/json", invalid).unwrap_err();
        assert!(decode_error.contains("expected JSON"));
        assert!(!decode_error.contains("not-json-provider-sentinel"));
        assert_eq!(safe_response_prefix("application/json", invalid), None);
    }

    #[test]
    fn suppresses_html_prefixes_that_might_contain_credentials() {
        let html = b"<!doctype html><html>cookie=credential-sentinel</html>";
        assert_eq!(safe_response_prefix("text/html", html), None);
    }

    #[test]
    fn soundcloud_identity_requires_a_nonzero_stable_user_id() {
        assert_eq!(
            soundcloud_stable_id(&serde_json::json!({"id": 12345})),
            Some("12345".into())
        );
        assert_eq!(
            soundcloud_stable_id(&serde_json::json!({"id": " 12345 "})),
            Some("12345".into())
        );
        assert_eq!(soundcloud_stable_id(&serde_json::json!({"id": 0})), None);
        assert_eq!(
            soundcloud_stable_id(&serde_json::json!({"username": "listener"})),
            None
        );
    }

    #[test]
    fn soundcloud_desktop_and_mobile_profiles_must_match() {
        let desktop = SoundCloudAuthenticatedUser {
            identity: ServiceIdentity::new("desktop", None).unwrap(),
            stable_id: "42".into(),
        };
        let mobile = SoundCloudAuthenticatedUser {
            identity: ServiceIdentity::new("mobile", None).unwrap(),
            stable_id: "42".into(),
        };
        assert_eq!(
            soundcloud_pair_identity(desktop, mobile).unwrap().username,
            "desktop"
        );

        let desktop = SoundCloudAuthenticatedUser {
            identity: ServiceIdentity::new("desktop", None).unwrap(),
            stable_id: "42".into(),
        };
        let mobile = SoundCloudAuthenticatedUser {
            identity: ServiceIdentity::new("mobile", None).unwrap(),
            stable_id: "99".into(),
        };
        let error = soundcloud_pair_identity(desktop, mobile).unwrap_err();
        assert!(error.contains("different accounts"));
    }
}
