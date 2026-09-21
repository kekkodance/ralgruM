use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Mutex,
};

use futures::{StreamExt as _, stream};
use reqwest::header;
use serde_json::{Value, json};

use super::{
    client::{SearchClient, response_cookies},
    credential::DeezerArl,
    models::ProviderError,
};

const JWT_URL: &str = "https://auth.deezer.com/login/arl?jo=p&rto=c&i=c";
const GRAPHQL_URL: &str = "https://pipe.deezer.com/api";
const ALBUM_BATCH_SIZE: usize = 25;
const ALBUM_BATCH_CONCURRENCY: usize = 2;
const ALBUM_CACHE_LIMIT: usize = 4096;

pub(super) struct DeezerAiCache {
    values: Mutex<AlbumCache>,
    batch_slots: tokio::sync::Semaphore,
}

impl Default for DeezerAiCache {
    fn default() -> Self {
        Self {
            values: Mutex::new(AlbumCache::default()),
            batch_slots: tokio::sync::Semaphore::new(ALBUM_BATCH_CONCURRENCY),
        }
    }
}

#[derive(Default)]
struct AlbumCache {
    values: HashMap<String, bool>,
    order: VecDeque<String>,
}

impl AlbumCache {
    fn insert(&mut self, id: String, value: bool) {
        if self.values.insert(id.clone(), value).is_none() {
            self.order.push_back(id);
        }
        while self.values.len() > ALBUM_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.values.remove(&oldest);
            }
        }
    }
}

impl SearchClient {
    pub(crate) async fn deezer_ai_content(
        &self,
        album_ids: Vec<String>,
        arl: DeezerArl,
    ) -> Result<HashMap<String, bool>, ProviderError> {
        let album_ids = valid_album_ids(album_ids);
        if album_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let (mut result, missing) = self.cached_deezer_ai(&album_ids);
        if missing.is_empty() {
            return Ok(result);
        }

        let session = self.deezer_session(Some(arl)).await?;
        let jwt = deezer_jwt(self, &session).await?;
        let batches = missing
            .chunks(ALBUM_BATCH_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect::<Vec<_>>();
        let fetched = stream::iter(batches.into_iter().map(|ids| {
            let jwt = jwt.clone();
            async move { self.deezer_ai_batch(&ids, &jwt).await }
        }))
        .buffer_unordered(ALBUM_BATCH_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

        let mut first_error = None;
        let mut fetched_any = false;
        for batch in fetched {
            match batch {
                Ok(values) => {
                    fetched_any |= !values.is_empty();
                    result.extend(values);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        if !fetched_any && let Some(error) = first_error {
            return Err(error);
        }
        let mut cache = self
            .deezer_ai
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (id, value) in &result {
            cache.insert(id.clone(), *value);
        }
        Ok(result)
    }

    fn cached_deezer_ai(&self, album_ids: &[String]) -> (HashMap<String, bool>, Vec<String>) {
        let cache = self
            .deezer_ai
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut known = HashMap::new();
        let mut missing = Vec::new();
        for id in album_ids {
            match cache.values.get(id).copied() {
                Some(value) => {
                    known.insert(id.clone(), value);
                }
                None => missing.push(id.clone()),
            }
        }
        (known, missing)
    }

    async fn deezer_ai_batch(
        &self,
        album_ids: &[String],
        jwt: &str,
    ) -> Result<HashMap<String, bool>, ProviderError> {
        let _permit = self
            .deezer_ai
            .batch_slots
            .acquire()
            .await
            .map_err(|_| ProviderError::new("Deezer AI metadata queue closed"))?;
        let (query, variables) = album_query(album_ids);
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {jwt}"))
            .map_err(|_| ProviderError::new("Deezer AI metadata login was invalid"))?;
        authorization.set_sensitive(true);
        let response = self
            .http()
            .post(GRAPHQL_URL)
            .header(header::AUTHORIZATION, authorization)
            .header(header::ORIGIN, "https://www.deezer.com")
            .header(header::REFERER, "https://www.deezer.com/")
            .json(&json!({
                "operationName": "AlbumAIContentBatch",
                "variables": variables,
                "query": query,
            }))
            .send()
            .await
            .map_err(|_| ProviderError::new("Deezer AI metadata request failed"))?;
        if !response.status().is_success() {
            return Err(ProviderError::new(format!(
                "Deezer AI metadata returned {}",
                response.status()
            )));
        }
        let value: Value = crate::provider_response::json(response)
            .await
            .map_err(|_| ProviderError::new("Deezer returned invalid AI metadata"))?;
        let data = value
            .get("data")
            .and_then(Value::as_object)
            .ok_or_else(|| ProviderError::new("Deezer AI metadata response is missing data"))?;
        Ok(album_ids
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                data.get(&format!("album{index}"))
                    .and_then(|album| album.get("hasIdentifiedAIContent"))
                    .and_then(Value::as_bool)
                    .map(|value| (id.clone(), value))
            })
            .collect())
    }
}

async fn deezer_jwt(
    client: &SearchClient,
    session: &super::client::DeezerSession,
) -> Result<String, ProviderError> {
    let response = client
        .http()
        .post(JWT_URL)
        .header(header::ACCEPT, "application/json")
        .header(header::COOKIE, session.request_cookie()?)
        .header(header::CONTENT_LENGTH, "0")
        .header(header::ORIGIN, "https://www.deezer.com")
        .header(header::REFERER, "https://www.deezer.com/")
        .body("")
        .send()
        .await
        .map_err(|_| ProviderError::new("Deezer AI metadata login failed"))?;
    if !response.status().is_success() {
        return Err(ProviderError::new(format!(
            "Deezer AI metadata login returned {}",
            response.status()
        )));
    }
    let cookies = response_cookies(&response);
    if let Some(jar) = session.arl.as_ref().and_then(DeezerArl::attached_jar) {
        jar.refresh(&cookies);
    }
    let value: Value = crate::provider_response::json(response)
        .await
        .map_err(|_| ProviderError::new("Deezer AI metadata login was invalid"))?;
    value
        .get("jwt")
        .and_then(Value::as_str)
        .filter(|jwt| jwt.split('.').count() == 3)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::new("Deezer AI metadata login returned no token"))
}

fn valid_album_ids(album_ids: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    album_ids
        .into_iter()
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

fn album_query(album_ids: &[String]) -> (String, Value) {
    let mut declarations = Vec::with_capacity(album_ids.len());
    let mut selections = Vec::with_capacity(album_ids.len());
    let mut variables = serde_json::Map::with_capacity(album_ids.len());
    for (index, id) in album_ids.iter().enumerate() {
        declarations.push(format!("$id{index}: String!"));
        selections.push(format!(
            "album{index}: album(albumId: $id{index}) {{ hasIdentifiedAIContent }}"
        ));
        variables.insert(format!("id{index}"), Value::String(id.clone()));
    }
    (
        format!(
            "query AlbumAIContentBatch({}) {{ {} }}",
            declarations.join(", "),
            selections.join(" ")
        ),
        Value::Object(variables),
    )
}

#[cfg(test)]
mod tests {
    use super::{album_query, valid_album_ids};

    #[test]
    fn album_ids_are_numeric_unique_and_ordered() {
        assert_eq!(
            valid_album_ids(vec!["2".into(), "bad".into(), "2".into(), "1".into()]),
            ["2", "1"]
        );
    }

    #[test]
    fn album_query_uses_variables_and_stable_aliases() {
        let (query, variables) = album_query(&["922038201".into(), "302127".into()]);
        assert!(query.contains("album0: album(albumId: $id0)"));
        assert!(query.contains("album1: album(albumId: $id1)"));
        assert_eq!(variables["id0"], "922038201");
        assert_eq!(variables["id1"], "302127");
    }
}
