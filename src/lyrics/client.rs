use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{collections::HashMap, future::Future};

use super::core::{
    EmptyLyricsReason, GeniusHit, LyricsProvider, LyricsResponse, LyricsTrack,
    extract_genius_lyrics, extract_genius_lyrics_result, extract_genius_referents,
    extract_musixmatch_lyrics, finalize_genius_hit, genius_search_queries, record_genius_hit,
    select_genius_hit,
};
use reqwest::{Client, Url, header};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MUSIXMATCH_BASE: &str = "https://apic-appmobile.musixmatch.com/ws/1.1";
const MUSIXMATCH_APP_ID: &str = "mac-ios-v2.0";
const MUSIXMATCH_USER_AGENT: &str = "Musixmatch/2025120901 CFNetwork/3860.300.31 Darwin/25.2.0";
const MUSIXMATCH_APP_VERSION: &str = "10.1.1";
const GENIUS_URL: &str = "https://api.genius.com";

#[derive(Clone)]
pub(super) struct LyricsClient {
    http: Client,
    musixmatch_token: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Debug)]
pub(super) struct GeniusAnnotation {
    pub(super) text: String,
    pub(super) votes: i64,
    pub(super) author: String,
}

#[derive(Clone, Debug)]
pub(super) struct GeniusReferent {
    pub(super) fragment: String,
    pub(super) annotations: Vec<GeniusAnnotation>,
}

impl LyricsClient {
    pub(super) fn new() -> Self {
        Self {
            http: Client::builder()
                .https_only(true)
                .timeout(Duration::from_secs(20))
                .user_agent("ralgrum-gpui")
                .build()
                .expect("lyrics client"),
            musixmatch_token: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) async fn load(
        &self,
        provider: LyricsProvider,
        track: LyricsTrack,
        cancel: CancellationToken,
    ) -> Result<LyricsResponse, String> {
        if cancel.is_cancelled() {
            return Err("Lyrics request cancelled".into());
        }
        let request_cancel = cancel.clone();
        let request = async {
            match provider {
                LyricsProvider::Musixmatch => self.musixmatch(&track).await,
                LyricsProvider::Genius => self.genius(&track, request_cancel).await,
            }
        };
        cancellable_request(&cancel, request).await
    }

    pub(super) async fn annotation(
        &self,
        id: &str,
        cancel: CancellationToken,
    ) -> Result<GeniusReferent, String> {
        if cancel.is_cancelled() {
            return Err("Lyrics request cancelled".into());
        }
        let ids = split_genius_referent_ids(id)?;
        let fetches = ids.iter().map(|referent_id| {
            let cancel = cancel.clone();
            async move {
                if cancel.is_cancelled() {
                    return Err("Lyrics request cancelled".into());
                }
                self.fetch_referent_json(referent_id).await
            }
        });
        let mut values = Vec::new();
        let mut last_error = None;
        let results = cancellable_request(&cancel, async {
            Ok(futures::future::join_all(fetches).await)
        })
        .await?;
        for result in results {
            match result {
                Ok(value) => values.push(value),
                Err(error) => last_error = Some(error),
            }
        }
        if values.is_empty() {
            return Err(last_error.unwrap_or_else(|| "Annotation not found".into()));
        }
        merge_genius_referent_values(values.iter())
    }

    async fn fetch_referent_json(&self, id: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(format!("{GENIUS_URL}/referents/{id}"))
            .headers(headers())
            .query(&[("text_format", "plain")])
            .send()
            .await
            .map_err(|error| lyrics_request_error("Genius", error))?;
        crate::provider_response::json::<Value>(response)
            .await
            .map_err(|_| lyrics_json_error("Genius"))
    }

    async fn musixmatch(&self, track: &LyricsTrack) -> Result<LyricsResponse, String> {
        let mut refreshed = false;
        loop {
            let token = self.musixmatch_user_token().await?;
            let value = self.musixmatch_macro(track, &token).await?;
            if musixmatch_status(&value) == Some(401) && !refreshed {
                self.clear_musixmatch_token();
                refreshed = true;
                continue;
            }
            return Ok(extract_musixmatch_lyrics(&value)
                .into_iter()
                .next()
                .unwrap_or(LyricsResponse::Empty {
                    provider: LyricsProvider::Musixmatch,
                    reason: EmptyLyricsReason::NotFound,
                }));
        }
    }

    async fn musixmatch_user_token(&self) -> Result<String, String> {
        if let Some(token) = self.cached_musixmatch_token() {
            return Ok(token);
        }
        let response = self
            .http
            .get(format!("{MUSIXMATCH_BASE}/token.get"))
            .headers(musixmatch_headers())
            .query(&[("format", "json"), ("app_id", MUSIXMATCH_APP_ID)])
            .send()
            .await
            .map_err(|error| lyrics_request_error("Musixmatch", error))?;
        let value = crate::provider_response::json::<Value>(response)
            .await
            .map_err(|_| lyrics_json_error("Musixmatch"))?;
        let token = parse_musixmatch_token(&value)?;
        self.store_musixmatch_token(token.clone());
        Ok(token)
    }

    async fn musixmatch_macro(&self, track: &LyricsTrack, token: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(format!("{MUSIXMATCH_BASE}/macro.subtitles.get"))
            .headers(musixmatch_headers())
            .query(&[
                ("format", "json"),
                ("namespace", "lyrics_richsynched"),
                ("subtitle_format", "mxm"),
                ("app_id", MUSIXMATCH_APP_ID),
                ("usertoken", token),
                ("q_artist", track.artist.as_str()),
                ("q_track", track.title.as_str()),
                (
                    "q_duration",
                    &track
                        .duration
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                ),
                ("q_album", track.album.as_deref().unwrap_or_default()),
            ])
            .send()
            .await
            .map_err(|error| lyrics_request_error("Musixmatch", error))?;
        crate::provider_response::json::<Value>(response)
            .await
            .map_err(|_| lyrics_json_error("Musixmatch"))
    }

    fn cached_musixmatch_token(&self) -> Option<String> {
        self.musixmatch_token
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn store_musixmatch_token(&self, token: String) {
        *self
            .musixmatch_token
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(token);
    }

    fn clear_musixmatch_token(&self) {
        *self
            .musixmatch_token
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    async fn genius(
        &self,
        track: &LyricsTrack,
        cancel: CancellationToken,
    ) -> Result<LyricsResponse, String> {
        let Some(hit) = self.genius_match(track, cancel.clone()).await? else {
            return Ok(LyricsResponse::Empty {
                provider: LyricsProvider::Genius,
                reason: EmptyLyricsReason::NoMatch,
            });
        };
        let id = hit.id;
        let url = validate_genius_url(&hit.url)?;
        if cancel.is_cancelled() {
            return Err("Lyrics request cancelled".into());
        }
        let song_fut = async {
            let response = self
                .http
                .get(format!("{GENIUS_URL}/songs/{id}"))
                .headers(headers())
                .query(&[("text_format", "plain")])
                .send()
                .await
                .map_err(|error| lyrics_request_error("Genius", error))?;
            crate::provider_response::json::<Value>(response)
                .await
                .map_err(|_| lyrics_json_error("Genius"))
        };
        let referents_fut = async {
            let response = self
                .http
                .get(format!("{GENIUS_URL}/referents"))
                .headers(headers())
                .query(&[
                    ("song_id", id.to_string()),
                    ("text_format", "plain".to_owned()),
                    ("per_page", "50".to_owned()),
                ])
                .send()
                .await
                .map_err(|error| lyrics_request_error("Genius", error))?;
            crate::provider_response::json::<Value>(response)
                .await
                .map_err(|_| lyrics_json_error("Genius"))
        };
        let (song, referents) = futures::join!(song_fut, referents_fut);
        let song = song?;
        let Some(text) = extract_genius_lyrics(&song) else {
            return Ok(LyricsResponse::Empty {
                provider: LyricsProvider::Genius,
                reason: EmptyLyricsReason::MatchedWithoutLyrics,
            });
        };
        let referents = referents?;
        Ok(extract_genius_lyrics_result(
            &serde_json::json!({"lyrics": text, "annotations": extract_genius_referents(&referents)}),
            &url,
        ))
    }

    async fn genius_match(
        &self,
        track: &LyricsTrack,
        cancel: CancellationToken,
    ) -> Result<Option<GeniusHit>, String> {
        let mut best_overlap = None;
        let mut best_any = None;
        for query in genius_search_queries(&track.artist, &track.title) {
            if cancel.is_cancelled() {
                return Err("Lyrics request cancelled".into());
            }
            let search = self.genius_search(&query).await?;
            let Some(candidate) = select_genius_hit(&search, &track.artist, &track.title) else {
                continue;
            };
            if let Some(hit) =
                record_genius_hit(&mut best_overlap, &mut best_any, candidate, &track.artist)
            {
                return Ok(Some(hit));
            }
        }
        Ok(finalize_genius_hit(
            best_overlap,
            best_any,
            &track.artist,
            &track.title,
        ))
    }

    async fn genius_search(&self, query: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(format!("{GENIUS_URL}/search/song"))
            .headers(headers())
            .query(&[("q", query)])
            .send()
            .await
            .map_err(|error| lyrics_request_error("Genius", error))?;
        crate::provider_response::json::<Value>(response)
            .await
            .map_err(|_| lyrics_json_error("Genius"))
    }
}

async fn cancellable_request<T>(
    cancel: &CancellationToken,
    request: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Lyrics request cancelled".into()),
        result = request => result,
    }
}

fn musixmatch_headers() -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    headers.insert(header::USER_AGENT, MUSIXMATCH_USER_AGENT.parse().unwrap());
    headers.insert(header::ACCEPT, "application/json".parse().unwrap());
    headers.insert("x-mxm-app-version", MUSIXMATCH_APP_VERSION.parse().unwrap());
    headers
}

fn parse_musixmatch_token(value: &Value) -> Result<String, String> {
    if musixmatch_status(value) != Some(200) {
        return Err("Could not reach Musixmatch.".into());
    }
    value
        .pointer("/message/body/user_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Could not reach Musixmatch.".into())
}

fn musixmatch_status(value: &Value) -> Option<u64> {
    value
        .pointer("/message/header/status_code")
        .and_then(Value::as_u64)
}

fn lyrics_request_error(provider: &str, error: reqwest::Error) -> String {
    if error.is_timeout() {
        format!("{provider} timed out. Try again.")
    } else {
        format!("Could not reach {provider}.")
    }
}

fn lyrics_json_error(provider: &str) -> String {
    format!("{provider} returned an invalid response.")
}

#[cfg(test)]
fn genius_hit(value: &Value) -> Result<Option<(u64, String)>, String> {
    let Some(hit) = super::core::extract_genius_hits(value)
        .into_iter()
        .find_map(|hit| hit.get("result").cloned())
    else {
        return Ok(None);
    };
    let id = hit
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Invalid Genius song ID".to_owned())?;
    let url = hit
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "Invalid Genius URL".to_owned())?;
    Ok(Some((id, validate_genius_url(url)?)))
}

fn split_genius_referent_ids(id: &str) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for part in id.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Invalid Genius annotation".into());
        }
        if !ids.iter().any(|existing| existing == part) {
            ids.push(part.to_owned());
        }
    }
    if ids.is_empty() {
        return Err("Invalid Genius annotation".into());
    }
    Ok(ids)
}

#[cfg(test)]
fn parse_genius_referent(value: &Value) -> Result<GeniusReferent, String> {
    merge_genius_referent_values([value])
}

fn merge_genius_referent_values<'a>(
    values: impl IntoIterator<Item = &'a Value>,
) -> Result<GeniusReferent, String> {
    let mut fragment = String::new();
    let mut by_id: HashMap<String, GeniusAnnotation> = HashMap::new();
    let mut order = Vec::new();
    for value in values {
        let Some(referent) = value
            .pointer("/response/referent")
            .filter(|value| !value.is_null())
            .or_else(|| value.pointer("/response/referents/0"))
        else {
            continue;
        };
        if fragment.is_empty() {
            fragment = referent
                .get("fragment")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
        }
        for annotation_value in referent
            .get("annotations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let annotation = parse_genius_contribution(annotation_value);
            let key = genius_numeric_id(annotation_value.get("id")).unwrap_or_else(|| {
                format!(
                    "anon:{}:{}:{}",
                    annotation.text, annotation.author, annotation.votes
                )
            });
            if let Some(existing) = by_id.get_mut(&key) {
                if annotation.votes > existing.votes {
                    *existing = annotation;
                }
            } else {
                order.push(key.clone());
                by_id.insert(key, annotation);
            }
        }
    }
    let mut annotations = order
        .into_iter()
        .filter_map(|key| by_id.remove(&key))
        .collect::<Vec<_>>();
    annotations.sort_by_key(|annotation| std::cmp::Reverse(annotation.votes));
    if annotations.is_empty() {
        return Err("Annotation not found".into());
    }
    Ok(GeniusReferent {
        fragment,
        annotations,
    })
}

fn genius_numeric_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(number) if number.as_u64().is_some() => Some(number.to_string()),
        Value::String(id) if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) => {
            Some(id.clone())
        }
        _ => None,
    }
}

fn parse_genius_contribution(value: &Value) -> GeniusAnnotation {
    GeniusAnnotation {
        text: value
            .pointer("/body/plain")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("No text explanation available.")
            .to_owned(),
        votes: value
            .get("votes_total")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        author: genius_annotation_author(value),
    }
}

fn genius_annotation_author(annotation: &Value) -> String {
    annotation
        .pointer("/authors/0/user/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|author| !author.is_empty())
        .unwrap_or("Genius Contributor")
        .to_owned()
}

fn headers() -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    headers.insert("X-Genius-Android-Version", "4.2.1".parse().unwrap());
    headers.insert(
        header::USER_AGENT,
        "Genius/4.2.1 (Android; Android 10; google Pixel 3)"
            .parse()
            .unwrap(),
    );
    headers
}

fn validate_genius_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "Invalid Genius URL".to_owned())?;
    if url.scheme() != "https" || url.host_str() != Some("genius.com") {
        return Err("Unsafe Genius URL".into());
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::time::{Duration, timeout};
    use tokio_util::sync::CancellationToken;

    use super::{
        cancellable_request, genius_annotation_author, genius_hit, lyrics_json_error,
        merge_genius_referent_values, parse_genius_referent, parse_musixmatch_token,
        split_genius_referent_ids, validate_genius_url,
    };
    use crate::lyrics::core::{extract_genius_referents, prepare_genius_lyrics};

    #[tokio::test]
    async fn cancellation_interrupts_an_active_request() {
        let cancellation = CancellationToken::new();
        let request_cancellation = cancellation.clone();
        let request = tokio::spawn(async move {
            cancellable_request(&request_cancellation, std::future::pending()).await
        });
        tokio::task::yield_now().await;
        cancellation.cancel();

        let result: Result<(), String> = timeout(Duration::from_millis(100), request)
            .await
            .expect("cancelled request should finish promptly")
            .expect("request task should not panic");
        assert_eq!(result, Err("Lyrics request cancelled".into()));
    }

    #[test]
    fn validates_provider_urls() {
        assert!(validate_genius_url("https://genius.com/song").is_ok());
        assert!(validate_genius_url("http://genius.com/song").is_err());
        assert!(validate_genius_url("https://example.com/song").is_err());
    }

    #[test]
    fn missing_genius_search_hit_is_an_empty_match_instead_of_an_error() {
        assert_eq!(genius_hit(&json!({"response":{"hits":[]}})), Ok(None));
    }

    #[test]
    fn genius_search_hit_keeps_the_provider_identity_and_safe_url() {
        assert_eq!(
            genius_hit(
                &json!({"response":{"hits":[{"result":{"id":42,"url":"https://genius.com/song"}}]}})
            ),
            Ok(Some((42, "https://genius.com/song".into())))
        );
    }

    #[test]
    fn genius_referent_keeps_every_contribution_sorted_by_votes() {
        let referent = parse_genius_referent(&json!({
            "response": {
                "referent": {
                    "fragment": "[Produced by Diplo & Skrillex]",
                    "annotations": [
                        {
                            "body": { "plain": "It's funny how people find this song." },
                            "votes_total": 55,
                            "authors": [{ "user": { "name": "Diplo" } }]
                        },
                        {
                            "body": { "plain": "This song makes me think of the first time." },
                            "votes_total": 70,
                            "authors": [{ "user": { "name": "Skrillex" } }]
                        }
                    ]
                }
            }
        }))
        .expect("referent with two contributions");
        assert_eq!(referent.fragment, "[Produced by Diplo & Skrillex]");
        assert_eq!(referent.annotations.len(), 2);
        assert_eq!(referent.annotations[0].author, "Skrillex");
        assert_eq!(referent.annotations[0].votes, 70);
        assert_eq!(referent.annotations[1].author, "Diplo");
        assert_eq!(referent.annotations[1].votes, 55);
    }

    #[test]
    fn genius_referent_accepts_the_android_referents_array() {
        let referent = parse_genius_referent(&json!({
            "response": {
                "referents": [{
                    "fragment": "[Produced by Diplo & Skrillex]",
                    "annotations": [
                        {
                            "body": { "plain": "It's funny how people find this song." },
                            "votes_total": 55,
                            "authors": [{ "user": { "name": "Diplo" } }]
                        },
                        {
                            "body": { "plain": "This song makes me think of the first time." },
                            "votes_total": 70,
                            "authors": [{ "user": { "name": "Skrillex" } }]
                        }
                    ]
                }]
            }
        }))
        .expect("referents array with two contributions");
        assert_eq!(referent.fragment, "[Produced by Diplo & Skrillex]");
        assert_eq!(referent.annotations.len(), 2);
        assert_eq!(referent.annotations[0].author, "Skrillex");
        assert_eq!(referent.annotations[0].votes, 70);
        assert_eq!(referent.annotations[1].author, "Diplo");
        assert_eq!(referent.annotations[1].votes, 55);
    }

    #[test]
    fn grouped_identical_fragments_yield_two_contributions_after_prepare_and_open_parse() {
        let list = json!({
            "response": {
                "referents": [
                    {
                        "id": 8506070,
                        "fragment": "[Produced by Diplo & Skrillex]",
                        "annotations": [{
                            "id": 1001,
                            "referent_id": 7274811,
                            "body": { "plain": "It's funny how people find this song." },
                            "votes_total": 55,
                            "authors": [{ "user": { "name": "Diplo" } }]
                        }]
                    },
                    {
                        "id": 8505909,
                        "fragment": "[Produced by Diplo & Skrillex]",
                        "annotations": [{
                            "id": 1002,
                            "referent_id": 7274811,
                            "body": { "plain": "This song makes me think of the first time." },
                            "votes_total": 70,
                            "authors": [{ "user": { "name": "Skrillex" } }]
                        }]
                    }
                ]
            }
        });
        let extracted = extract_genius_referents(&list);
        assert_eq!(extracted.len(), 1);
        assert_eq!(
            extracted[0].ids,
            vec![
                "8506070".to_string(),
                "7274811".to_string(),
                "8505909".to_string()
            ]
        );
        let prepared = prepare_genius_lyrics(
            "[Produced by Diplo & Skrillex]\nI need to hear some sounds",
            &extracted,
            "",
        );
        assert_eq!(prepared.ranges.len(), 1);
        assert_eq!(prepared.ranges[0].2, "8506070,7274811,8505909");
        assert_eq!(
            split_genius_referent_ids(&prepared.ranges[0].2).unwrap(),
            ["8506070", "7274811", "8505909"]
        );

        let first = json!({"response": {"referent": list["response"]["referents"][0].clone()}});
        let second = json!({"response": {"referent": list["response"]["referents"][1].clone()}});
        assert_eq!(parse_genius_referent(&first).unwrap().annotations.len(), 1);
        assert_eq!(parse_genius_referent(&second).unwrap().annotations.len(), 1);
        let merged = merge_genius_referent_values([&first, &second]).expect("merged contributions");
        assert_eq!(merged.fragment, "[Produced by Diplo & Skrillex]");
        assert_eq!(merged.annotations.len(), 2);
        assert_eq!(merged.annotations[0].author, "Skrillex");
        assert_eq!(merged.annotations[0].votes, 70);
        assert_eq!(merged.annotations[1].author, "Diplo");
        assert_eq!(merged.annotations[1].votes, 55);
    }

    #[test]
    fn genius_referent_merge_unions_contributions_by_annotation_id() {
        let child = json!({
            "response": {
                "referent": {
                    "fragment": "[Produced by Diplo & Skrillex]",
                    "annotations": [{
                        "id": 1001,
                        "body": { "plain": "It's funny how people find this song." },
                        "votes_total": 55,
                        "authors": [{ "user": { "name": "Diplo" } }]
                    }]
                }
            }
        });
        let parent = json!({
            "response": {
                "referent": {
                    "fragment": "[Produced by Diplo & Skrillex]",
                    "annotations": [
                        {
                            "id": 1001,
                            "body": { "plain": "It's funny how people find this song." },
                            "votes_total": 55,
                            "authors": [{ "user": { "name": "Diplo" } }]
                        },
                        {
                            "id": 1002,
                            "body": { "plain": "This song makes me think of the first time." },
                            "votes_total": 70,
                            "authors": [{ "user": { "name": "Skrillex" } }]
                        }
                    ]
                }
            }
        });
        let merged =
            merge_genius_referent_values([&child, &parent]).expect("unioned contributions");
        assert_eq!(merged.annotations.len(), 2);
        assert_eq!(merged.annotations[0].author, "Skrillex");
        assert_eq!(merged.annotations[1].author, "Diplo");
    }

    #[test]
    fn genius_referent_without_contributions_is_an_error() {
        assert_eq!(
            parse_genius_referent(&json!({
                "response": { "referent": { "fragment": "line", "annotations": [] } }
            }))
            .unwrap_err(),
            "Annotation not found"
        );
    }

    #[test]
    fn missing_or_blank_annotation_author_uses_the_original_fallback() {
        assert_eq!(
            genius_annotation_author(&json!({"authors": []})),
            "Genius Contributor"
        );
        assert_eq!(
            genius_annotation_author(&json!({"authors": [{"user": {"name": "  "}}]})),
            "Genius Contributor"
        );
        assert_eq!(
            genius_annotation_author(&json!({"authors": [{"user": {"name": "  Alice  "}}]})),
            "Alice"
        );
    }

    #[test]
    fn musixmatch_token_requires_a_successful_user_token() {
        assert!(
            parse_musixmatch_token(&json!({"message":{"header":{"status_code":401}}})).is_err()
        );
        assert_eq!(
            parse_musixmatch_token(&json!({
                "message": {
                    "header": { "status_code": 200 },
                    "body": { "user_token": "  token-value  " }
                }
            }))
            .as_deref(),
            Ok("token-value")
        );
    }

    #[test]
    fn lyrics_errors_never_include_request_urls() {
        let error = lyrics_json_error("Musixmatch");
        assert_eq!(error, "Musixmatch returned an invalid response.");
        assert!(!error.contains("http"));
        assert!(!error.contains("usertoken"));
        assert!(!error.contains("musixmatch.com"));
    }
}
