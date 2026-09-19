use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header;
use serde_json::{Value, json};

use super::playlist_client::{PlaylistClient, valid_id};
use crate::search::DeezerArl;

const GATEWAY_URL: &str = "https://www.deezer.com/ajax/gw-light.php";
static CID: AtomicU64 = AtomicU64::new(100_000_000);

impl PlaylistClient {
    pub(crate) async fn delete(
        &self,
        arl: DeezerArl,
        saved_user_id: Option<String>,
        playlist_id: &str,
    ) -> Result<bool, String> {
        let playlist_id = valid_id(playlist_id)?;
        let session = self.session(arl, saved_user_id).await?;
        let playlist = self
            .catalog_for_session(&session)
            .await?
            .into_iter()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or_else(|| "Deezer did not identify this as one of your playlists".to_string())?;
        if playlist.is_from_favorite_tracks {
            return Err("Deezer's Favorite Tracks playlist cannot be deleted here".into());
        }
        if playlist.is_collaborative {
            return Err("Collaborative Deezer playlists cannot be deleted here".into());
        }
        let response = self
            .client
            .post(gateway_url(&session.api_token)?)
            .header(header::COOKIE, session.cookie.clone())
            .header(header::ACCEPT, "*/*")
            .header(header::ACCEPT_LANGUAGE, "en")
            .header("Accept-Charset", "UTF-8")
            .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
            .header(header::ORIGIN, "https://www.deezer.com")
            .header(
                header::REFERER,
                format!("https://www.deezer.com/en/playlist/{playlist_id}"),
            )
            .header("x-deezer-user", &session.user_id)
            .body(delete_body(&playlist_id).to_string())
            .send()
            .await
            .map_err(|_| "Deezer playlist.delete request failed".to_string())?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("Deezer returned HTTP status {}", status.as_u16()));
        }
        let value: Value = crate::provider_response::json(response)
            .await
            .map_err(|_| "Deezer returned an invalid response".to_string())?;
        if let Some(error) = value.get("error")
            && deezer_envelope_has_error(error)
        {
            return Err("Deezer playlist.delete failed".into());
        }
        if value.get("results") == Some(&Value::Bool(true)) {
            Ok(true)
        } else {
            Err("Deezer playlist.delete did not confirm the change".into())
        }
    }
}

fn delete_body(id: &str) -> Value {
    json!({ "playlist_id": id })
}

fn deezer_envelope_has_error(error: &Value) -> bool {
    match error {
        Value::Null | Value::Bool(false) => false,
        Value::Array(items) => !items.is_empty(),
        Value::Object(items) => !items.is_empty(),
        Value::String(message) => !message.is_empty(),
        Value::Number(number) => number.as_i64() != Some(0),
        _ => true,
    }
}

fn gateway_url(api_token: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(GATEWAY_URL)
        .map_err(|_| "Deezer gateway address is invalid".to_string())?;
    url.query_pairs_mut()
        .append_pair("method", "playlist.delete")
        .append_pair("input", "3")
        .append_pair("api_version", "1.0")
        .append_pair("api_token", api_token)
        .append_pair("cid", &CID.fetch_add(1, Ordering::Relaxed).to_string());
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_contract_has_exact_body_and_gateway_shape() {
        assert_eq!(delete_body("42"), json!({"playlist_id":"42"}));
        assert!(valid_id("42x").is_err());
        let url = gateway_url("token").unwrap();
        assert!(url.starts_with("https://www.deezer.com/ajax/gw-light.php?method=playlist.delete&input=3&api_version=1.0&api_token=token&cid="));
    }

    #[test]
    fn delete_response_accepts_empty_error_array() {
        assert!(!deezer_envelope_has_error(&json!([])));
        assert!(!deezer_envelope_has_error(&json!(false)));
        assert!(!deezer_envelope_has_error(&Value::Null));
        assert!(deezer_envelope_has_error(&json!(["some error"])));
        assert!(deezer_envelope_has_error(
            &json!({"GATEWAY_ERROR": "failed"})
        ));
    }
}
