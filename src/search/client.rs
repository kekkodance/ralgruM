use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use reqwest::{Client, Response, Url, header};
use serde_json::{Value, json};

use super::{
    credential::{DeezerArl, SoundCloudToken},
    models::{Provider, ProviderError, RawResult, ResultType, SearchRequest},
};

pub(crate) const SOUNDCLOUD_CLIENT_ID: &str = "Pb72ranhoyt6gw7hM7TkzUItXlMWSNSo";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const DEEZER_USER_DATA_URL: &str = "https://www.deezer.com/ajax/gw-light.php?method=deezer.getUserData&input=3&api_version=1.0&api_token=";
const DEEZER_SESSION_CACHE_TTL: Duration = Duration::from_secs(30);

const fn deezer_spec(category: ResultType) -> Option<(&'static str, u32, u32)> {
    match category {
        ResultType::Tracks => Some(("TRACK", 100, 0)),
        ResultType::Albums => Some(("ALBUM", 100, 0)),
        ResultType::Artists => Some(("ARTIST", 100, 0)),
        ResultType::Playlists => Some(("PLAYLIST", 100, 0)),
        ResultType::All => None,
    }
}

#[derive(Clone)]
pub(crate) struct SearchClient {
    client: Client,
    deezer_sessions: Arc<DeezerSessionCache>,
}

#[derive(Default)]
struct DeezerSessionCache {
    slots: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<CachedDeezerSession>>>>>,
}

#[derive(Clone)]
struct CachedDeezerSession {
    session: DeezerSession,
    refreshed_at: Instant,
}

impl SearchClient {
    pub(crate) fn new() -> Result<Self, ProviderError> {
        Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(4)
            .user_agent(USER_AGENT)
            .build()
            .map(Self::with_http_client)
            .map_err(|_| ProviderError::new("Search client could not be created"))
    }

    pub(crate) fn with_http_client(client: Client) -> Self {
        Self {
            client,
            deezer_sessions: Arc::new(DeezerSessionCache::default()),
        }
    }

    pub(crate) async fn execute(
        &self,
        requests: Vec<SearchRequest>,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
    ) -> Vec<RawResult> {
        let (deezer_requests, soundcloud_requests): (Vec<_>, Vec<_>) = requests
            .into_iter()
            .partition(|request| request.provider == Provider::Deezer);
        let deezer_fut = async {
            if deezer_requests.is_empty() {
                return Vec::new();
            }
            let deezer_session = match deezer_arl {
                Some(arl) => self.deezer_session(arl).await.map(Some),
                None => Ok(None),
            };
            let deezer = deezer_requests.into_iter().map(|request| {
                let client = self.clone();
                let deezer_session = deezer_session.clone();
                async move {
                    let data = match deezer_session {
                        Ok(Some(session)) => client.deezer(&request, &session).await,
                        Ok(None) => Err(ProviderError::new("Deezer login required")),
                        Err(error) => Err(error),
                    };
                    RawResult { request, data }
                }
            });
            futures::future::join_all(deezer).await
        };
        let soundcloud_fut = async {
            let soundcloud =
                soundcloud_requests
                    .iter()
                    .flat_map(soundcloud_fanout)
                    .map(|request| {
                        let client = self.clone();
                        let token = soundcloud_token.clone();
                        async move {
                            let data = client.soundcloud(&request, token.as_ref()).await;
                            RawResult { request, data }
                        }
                    });
            futures::future::join_all(soundcloud).await
        };
        let (mut results, soundcloud_results) = futures::join!(deezer_fut, soundcloud_fut);
        results.extend(soundcloud_results);
        results
    }

    pub(crate) async fn soundcloud_suggestions(
        &self,
        query: &str,
    ) -> Result<Vec<String>, ProviderError> {
        let response = soundcloud_suggestions_request(&self.client, query)?
            .send()
            .await
            .map_err(classify_request_error)?;
        if !response.status().is_success() {
            return Err(ProviderError::new(format!(
                "SoundCloud returned {}",
                response.status()
            )));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| ProviderError::new("SoundCloud returned invalid suggestions"))?;
        parse_soundcloud_suggestions(&value)
    }

    pub(super) async fn deezer_session(
        &self,
        arl: DeezerArl,
    ) -> Result<DeezerSession, ProviderError> {
        let key = arl.expose().to_owned();
        let slot = self
            .deezer_sessions
            .slots
            .lock()
            .expect("Deezer session cache lock poisoned")
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
            .clone();
        let mut cached = slot.lock().await;
        if let Some(entry) = cached
            .as_ref()
            .filter(|entry| entry.refreshed_at.elapsed() < DEEZER_SESSION_CACHE_TTL)
        {
            return Ok(entry.session.clone());
        }
        let session = self.deezer_session_uncached(arl).await?;
        *cached = Some(CachedDeezerSession {
            session: session.clone(),
            refreshed_at: Instant::now(),
        });
        Ok(session)
    }

    pub(crate) fn clear_deezer_sessions(&self) {
        self.deezer_sessions
            .slots
            .lock()
            .expect("Deezer session cache lock poisoned")
            .clear();
    }

    async fn deezer_session_uncached(
        &self,
        arl: DeezerArl,
    ) -> Result<DeezerSession, ProviderError> {
        let response = deezer_session_request(&self.client, arl.cookie_header()?)
            .send()
            .await
            .map_err(classify_deezer_error)?;
        let cookies = response_cookies(&response);
        let value = deezer_json(response).await?;
        let check_form = value
            .pointer("/results/checkForm")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if check_form.is_empty() {
            return Err(ProviderError::new("Deezer login required"));
        }
        let user_id = value_string(value.pointer("/results/USER/USER_ID"));
        Ok(DeezerSession {
            arl,
            check_form,
            cookies,
            user_id,
        })
    }

    async fn deezer(
        &self,
        request: &SearchRequest,
        session: &DeezerSession,
    ) -> Result<Vec<Value>, ProviderError> {
        let (output, nb, start) = deezer_spec(request.category)
            .ok_or_else(|| ProviderError::new("Unsupported Deezer search category"))?;
        let mut cookie = session.arl.cookie_header()?;
        if !session.cookies.is_empty() {
            let arl = cookie
                .to_str()
                .map_err(|_| ProviderError::new("The saved Deezer session is invalid"))?;
            cookie = header::HeaderValue::from_str(&format!("{arl}; {}", session.cookies))
                .map_err(|_| ProviderError::new("Deezer returned an invalid session"))?;
            cookie.set_sensitive(true);
        }
        let mut url = Url::parse("https://www.deezer.com/ajax/gw-light.php")
            .map_err(|_| ProviderError::new("Invalid Deezer search endpoint"))?;
        url.query_pairs_mut()
            .append_pair("method", "search.music")
            .append_pair("input", "3")
            .append_pair("api_version", "1.0")
            .append_pair("api_token", &session.check_form);
        let response = self
            .client
            .post(url)
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json; charset=UTF-8")
            .json(&json!({
                "query": request.query,
                "filter": "all",
                "output": output,
                "nb": nb,
                "start": start
            }))
            .send()
            .await
            .map_err(classify_deezer_error)?;
        let value = deezer_json(response).await?;
        value
            .pointer("/results/data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ProviderError::new("Deezer returned an invalid search response"))
    }

    async fn soundcloud(
        &self,
        request: &SearchRequest,
        token: Option<&SoundCloudToken>,
    ) -> Result<Vec<Value>, ProviderError> {
        let response = soundcloud_request(&self.client, request, token)?
            .send()
            .await
            .map_err(classify_request_error)?;
        if !response.status().is_success() {
            return Err(ProviderError::new(format!(
                "SoundCloud returned {}",
                response.status()
            )));
        }
        let value: Value = response
            .json()
            .await
            .map_err(|_| ProviderError::new("SoundCloud returned an invalid response"))?;
        value
            .get("collection")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| ProviderError::new("SoundCloud returned an invalid search response"))
    }
}

fn soundcloud_request(
    client: &Client,
    request: &SearchRequest,
    token: Option<&SoundCloudToken>,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let (path, limit) = soundcloud_spec(request.category)?;
    let query = url::form_urlencoded::byte_serialize(request.query.as_bytes()).collect::<String>();
    let url = Url::parse(&format!(
        "https://api-v2.soundcloud.com/search/{path}?client_id={SOUNDCLOUD_CLIENT_ID}&q={query}&limit={limit}&offset=0"
    ))
        .map_err(|_| ProviderError::new("Invalid SoundCloud search endpoint"))?;
    let request = client.get(url);
    match token {
        Some(token) => Ok(request.header(header::AUTHORIZATION, token.authorization_header()?)),
        None => Ok(request),
    }
}

fn soundcloud_suggestions_request(
    client: &Client,
    query: &str,
) -> Result<reqwest::RequestBuilder, ProviderError> {
    let mut url = Url::parse("https://api-v2.soundcloud.com/search/queries")
        .map_err(|_| ProviderError::new("Invalid SoundCloud suggestions endpoint"))?;
    url.query_pairs_mut()
        .append_pair("q", query.trim())
        .append_pair("client_id", SOUNDCLOUD_CLIENT_ID)
        .append_pair("limit", "10");
    Ok(client.get(url))
}

fn parse_soundcloud_suggestions(value: &Value) -> Result<Vec<String>, ProviderError> {
    let collection = value
        .get("collection")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::new("SoundCloud returned invalid suggestions"))?;
    let mut seen = HashSet::new();
    Ok(collection
        .iter()
        .filter_map(|item| {
            item.get("query")
                .and_then(Value::as_str)
                .or_else(|| item.get("output").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .filter(|query| seen.insert(query.to_lowercase()))
        .take(10)
        .map(str::to_owned)
        .collect())
}

fn deezer_session_request(client: &Client, arl: header::HeaderValue) -> reqwest::RequestBuilder {
    client
        .post(DEEZER_USER_DATA_URL)
        .header(header::COOKIE, arl)
        .header(header::CONTENT_LENGTH, "0")
        .body("")
}

impl SearchClient {
    pub(super) fn http(&self) -> &Client {
        &self.client
    }
}

#[derive(Clone)]
pub(super) struct DeezerSession {
    pub(super) arl: DeezerArl,
    pub(super) check_form: String,
    pub(super) cookies: String,
    pub(super) user_id: Option<String>,
}

fn value_string(value: Option<&Value>) -> Option<String> {
    value?
        .as_str()
        .map(str::to_owned)
        .or_else(|| value?.as_u64().map(|id| id.to_string()))
        .filter(|value| !value.trim().is_empty())
}

fn response_cookies(response: &Response) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>()
        .join("; ")
}

pub(super) async fn deezer_json(response: Response) -> Result<Value, ProviderError> {
    if !response.status().is_success() {
        return Err(ProviderError::new(format!(
            "Deezer returned {}",
            response.status()
        )));
    }
    let value = response
        .json()
        .await
        .map_err(|_| ProviderError::new("Deezer returned an invalid response"))?;
    validate_deezer_envelope(value)
}

fn validate_deezer_envelope(value: Value) -> Result<Value, ProviderError> {
    let has_error = value.get("error").is_some_and(|error| match error {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
        Value::String(message) => !message.is_empty(),
        _ => true,
    });
    if has_error {
        Err(ProviderError::new("Deezer returned an error"))
    } else {
        Ok(value)
    }
}

fn classify_deezer_error(error: reqwest::Error) -> ProviderError {
    let message = if error.is_timeout() {
        "Deezer search timed out"
    } else if error.is_connect() {
        "Deezer could not be reached"
    } else {
        "Deezer search failed"
    };
    ProviderError::new(message)
}

fn classify_request_error(error: reqwest::Error) -> ProviderError {
    let message = if error.is_timeout() {
        "SoundCloud search timed out"
    } else if error.is_connect() {
        "SoundCloud could not be reached"
    } else {
        "SoundCloud search failed"
    };
    ProviderError::new(message)
}

fn soundcloud_spec(category: ResultType) -> Result<(&'static str, u32), ProviderError> {
    match category {
        ResultType::Tracks => Ok(("tracks", 200)),
        ResultType::Albums => Ok(("albums", 50)),
        ResultType::Artists => Ok(("users", 200)),
        ResultType::Playlists => Ok(("playlists_without_albums", 50)),
        ResultType::All => Err(ProviderError::new("Unsupported SoundCloud search category")),
    }
}

// Deezer handles ResultType::All by fanning the query out across every
// category and merging the per-category responses. SoundCloud has no single
// "all" endpoint, so requests that still carry All expand the same way before
// they reach soundcloud_spec.
fn soundcloud_fanout(request: &SearchRequest) -> Vec<SearchRequest> {
    request
        .category
        .categories()
        .iter()
        .map(|category| SearchRequest {
            provider: request.provider,
            category: *category,
            query: request.query.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header;

    #[test]
    fn soundcloud_uses_captured_category_paths_and_limits() {
        assert_eq!(
            soundcloud_spec(ResultType::Tracks).unwrap(),
            ("tracks", 200)
        );
        assert_eq!(soundcloud_spec(ResultType::Albums).unwrap(), ("albums", 50));
        assert_eq!(
            soundcloud_spec(ResultType::Artists).unwrap(),
            ("users", 200)
        );
        assert_eq!(
            soundcloud_spec(ResultType::Playlists).unwrap(),
            ("playlists_without_albums", 50)
        );
    }

    #[test]
    fn soundcloud_all_fans_out_across_categories_like_deezer() {
        let search = SearchRequest {
            provider: Provider::SoundCloud,
            category: ResultType::All,
            query: "query".into(),
        };
        let expanded = soundcloud_fanout(&search);
        assert_eq!(
            expanded
                .iter()
                .map(|request| request.category)
                .collect::<Vec<_>>(),
            ResultType::All.categories().to_vec()
        );
        assert!(expanded.iter().all(|request| {
            request.provider == Provider::SoundCloud && request.query == "query"
        }));
        for category in [ResultType::Tracks, ResultType::Albums, ResultType::Artists] {
            let focused = SearchRequest {
                provider: Provider::SoundCloud,
                category,
                query: "query".into(),
            };
            assert_eq!(soundcloud_fanout(&focused), vec![focused]);
        }
    }

    #[test]
    fn deezer_uses_captured_outputs_and_page_contract() {
        assert_eq!(deezer_spec(ResultType::Tracks), Some(("TRACK", 100, 0)));
        assert_eq!(deezer_spec(ResultType::Albums), Some(("ALBUM", 100, 0)));
        assert_eq!(deezer_spec(ResultType::Artists), Some(("ARTIST", 100, 0)));
        assert_eq!(
            deezer_spec(ResultType::Playlists),
            Some(("PLAYLIST", 100, 0))
        );
        assert_eq!(deezer_spec(ResultType::All), None);
    }

    #[test]
    fn soundcloud_search_requests_match_authenticated_original_contract() {
        let client = Client::new();
        let token = SoundCloudToken::from_saved("search-token-sentinel").unwrap();
        for (category, path, limit) in [
            (ResultType::Tracks, "tracks", 200),
            (ResultType::Albums, "albums", 50),
            (ResultType::Artists, "users", 200),
            (ResultType::Playlists, "playlists_without_albums", 50),
        ] {
            let search = SearchRequest {
                provider: Provider::SoundCloud,
                category,
                query: "a & b".into(),
            };
            let request = soundcloud_request(&client, &search, Some(&token))
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(
                request.url().as_str(),
                format!(
                    "https://api-v2.soundcloud.com/search/{path}?client_id={SOUNDCLOUD_CLIENT_ID}&q=a+%26+b&limit={limit}&offset=0"
                )
            );
            assert_eq!(
                request.headers()[header::AUTHORIZATION],
                "OAuth search-token-sentinel"
            );
            assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
            assert!(!format!("{request:?}").contains("search-token-sentinel"));
        }
    }

    #[test]
    fn signed_out_soundcloud_search_does_not_add_auth_headers() {
        let client = Client::new();
        let search = SearchRequest {
            provider: Provider::SoundCloud,
            category: ResultType::Tracks,
            query: "query".into(),
        };
        let request = soundcloud_request(&client, &search, None)
            .unwrap()
            .build()
            .unwrap();
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        assert!(!request.headers().contains_key(header::COOKIE));
    }

    #[test]
    fn soundcloud_suggestions_are_anonymous_and_match_the_capture() {
        let request = soundcloud_suggestions_request(&Client::new(), " skrillex ")
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(request.url().path(), "/search/queries");
        assert_eq!(
            request.url().query_pairs().collect::<Vec<_>>(),
            vec![
                ("q".into(), "skrillex".into()),
                ("client_id".into(), SOUNDCLOUD_CLIENT_ID.into()),
                ("limit".into(), "10".into()),
            ]
        );
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        assert!(!request.headers().contains_key(header::COOKIE));
    }

    #[test]
    fn soundcloud_suggestion_parser_prefers_query_and_deduplicates() {
        let value = json!({
            "collection": [
                {"output": "Skrillex", "query": "skrillex"},
                {"output": "SKRILLEX", "query": "SKRILLEX"},
                {"output": "Skrillex live", "query": "skrillex live"},
                {"output": "fallback only"},
                {"query": "   "}
            ]
        });
        assert_eq!(
            parse_soundcloud_suggestions(&value).unwrap(),
            ["skrillex", "skrillex live", "fallback only"]
        );
        assert!(parse_soundcloud_suggestions(&json!({})).is_err());
    }

    #[test]
    fn deezer_error_envelopes_are_fixed_and_sanitized() {
        assert_eq!(
            validate_deezer_envelope(json!({"error": {"SECRET": "raw body"}}))
                .unwrap_err()
                .message,
            "Deezer returned an error"
        );
        assert!(validate_deezer_envelope(json!({"error": [], "results": {}})).is_ok());
    }

    #[test]
    fn deezer_session_bootstrap_matches_post_contract() {
        let arl = super::super::credential::DeezerArl::from_saved("sentinel").unwrap();
        let request = deezer_session_request(&Client::new(), arl.cookie_header().unwrap())
            .build()
            .unwrap();
        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), DEEZER_USER_DATA_URL);
        assert_eq!(request.headers()[header::COOKIE], "arl=sentinel");
        assert_eq!(request.headers()[header::CONTENT_LENGTH], "0");
        assert_eq!(
            request.body().and_then(reqwest::Body::as_bytes),
            Some(&b""[..])
        );
    }
}
