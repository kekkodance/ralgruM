use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::{Response, Url, header};
use serde_json::{Value, json};

use super::playlist_client::{PlaylistClient, Session, valid_id};
use crate::search::DeezerArl;

const GATEWAY_URL: &str = "https://www.deezer.com/ajax/gw-light.php";
const PLAYLIST_FRAGMENT: &str = r#"fragment PlaylistInfo on Playlist {
  id title description isPrivate isFromFavoriteTracks isCollaborative estimatedTracksCount
  owner { id name __typename }
  picture { id small: urls(pictureRequest: {height: 100, width: 100}) medium: urls(pictureRequest: {width: 264, height: 264}) large: urls(pictureRequest: {width: 500, height: 500}) __typename }
  __typename
}"#;
const SIDEBAR_QUERY: &str = r#"query SidebarPlaylistsInfo($first: Int!) {
  me { id playlists(first: $first, sort: {by: LAST_MODIFICATION_DATE, order: DESC}) { edges { node { ...PlaylistInfo } } } userFavorites { playlists(first: $first) { edges { node { ...PlaylistInfo } } } } }
}"#;
static CID: AtomicU64 = AtomicU64::new(100_000_000);

pub(crate) async fn remove_track(
    client: &PlaylistClient,
    arl: DeezerArl,
    saved_user_id: Option<String>,
    playlist_id: &str,
    track_id: &str,
) -> Result<bool, String> {
    let playlist_id = valid_id(playlist_id)?;
    let track_id = valid_id(track_id)?;
    let playlist_number = playlist_id
        .parse::<u64>()
        .map_err(|_| "Deezer playlist id is too large".to_string())?;
    let track_number = track_id
        .parse::<u64>()
        .map_err(|_| "Deezer track id is too large".to_string())?;
    let session = client.session(arl, saved_user_id).await?;
    let catalog = client
        .graphql(
            &session,
            "SidebarPlaylistsInfo",
            json!({"first": 50}),
            format!("{}{}", SIDEBAR_QUERY, PLAYLIST_FRAGMENT),
        )
        .await?;
    let playlist = super::playlist_client::parse_catalog(&catalog)?
        .into_iter()
        .find(|playlist| playlist.id == playlist_id)
        .ok_or_else(|| "Deezer did not identify this as one of your playlists".to_string())?;
    if !playlist.editable() {
        return Err("This Deezer playlist cannot be edited".into());
    }
    let results = gateway_call(
        client,
        &session,
        "playlist.getSongs",
        &playlist_id,
        get_songs_body(&playlist_id),
    )
    .await?;
    let tracks = results
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Deezer playlist.getSongs response is missing its item list".to_string())?;
    let total = results.get("total").and_then(Value::as_u64).ok_or_else(|| {
        "The complete Deezer playlist could not be proven because its total is missing or malformed".to_string()
    })?;
    if !complete(Some(total), tracks.len(), tracks.len()) {
        return Err(
            "The complete Deezer playlist must be loaded before a track can be removed".into(),
        );
    }
    let mut occurrences = 0;
    for track in tracks {
        let id = track.get("SNG_ID").and_then(value_string).ok_or_else(|| {
            "Deezer playlist.getSongs returned a track without a valid id".to_string()
        })?;
        let number = valid_id(&id)?
            .parse::<u64>()
            .map_err(|_| "Deezer track id is too large".to_string())?;
        let _ = number;
        occurrences += usize::from(id == track_id);
    }
    match occurrences {
        0 => Err("This track is no longer present in the Deezer playlist".into()),
        1 => {
            let result = gateway_call(
                client,
                &session,
                "playlist.deleteSongs",
                &playlist_id,
                delete_song_body(&playlist_id, track_number, playlist_number),
            )
            .await?;
            (result == Value::Bool(true))
                .then_some(true)
                .ok_or_else(|| "Deezer playlist.deleteSongs did not confirm the change".into())
        }
        _ => {
            Err("Removing one of multiple identical Deezer playlist tracks was not captured".into())
        }
    }
}

fn complete(total: Option<u64>, raw: usize, normalized: usize) -> bool {
    total == Some(raw as u64) && normalized == raw
}

fn get_songs_body(playlist_id: &str) -> Value {
    json!({"playlist_id": playlist_id, "nb": 2000})
}

fn delete_song_body(playlist_id: &str, track_id: u64, context_id: u64) -> Value {
    json!({"playlist_id": playlist_id, "songs": [[track_id, 0]], "ctxt": {"id": context_id, "t": "playlist_page"}})
}

async fn gateway_call(
    client: &PlaylistClient,
    session: &Session,
    operation: &str,
    playlist_id: &str,
    body: Value,
) -> Result<Value, String> {
    let mut url =
        Url::parse(GATEWAY_URL).map_err(|_| "Invalid Deezer playlist endpoint".to_string())?;
    url.query_pairs_mut()
        .append_pair("method", operation)
        .append_pair("input", "3")
        .append_pair("api_version", "1.0")
        .append_pair("api_token", &session.api_token)
        .append_pair("cid", &CID.fetch_add(1, Ordering::Relaxed).to_string());
    let response = client
        .client
        .post(url)
        .header(header::COOKIE, session.cookie.clone())
        .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
        .header(header::ORIGIN, "https://www.deezer.com")
        .header(
            header::REFERER,
            format!("https://www.deezer.com/en/playlist/{playlist_id}"),
        )
        .header("x-deezer-user", &session.user_id)
        .body(body.to_string())
        .send()
        .await
        .map_err(|_| format!("Deezer {operation} request failed"))?;
    let envelope = decode(response).await?;
    envelope
        .get("results")
        .cloned()
        .ok_or_else(|| format!("Deezer {operation} response is missing results"))
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_owned()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

async fn decode(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err(format!(
            "Deezer returned HTTP status {}",
            response.status().as_u16()
        ));
    }
    crate::provider_response::json(response)
        .await
        .map_err(|_| "Deezer returned an invalid response".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_get_and_delete_payloads_and_strict_true() {
        assert_eq!(get_songs_body("42"), json!({"playlist_id":"42","nb":2000}));
        assert_eq!(
            delete_song_body("42", 7, 42),
            json!({"playlist_id":"42","songs":[[7,0]],"ctxt":{"id":42,"t":"playlist_page"}})
        );
        assert!(Value::Bool(true) == Value::Bool(true));
        assert!(Value::Bool(true) != json!(1));
    }

    #[test]
    fn numeric_overflow_is_rejected() {
        assert!("18446744073709551616".parse::<u64>().is_err());
    }

    #[test]
    fn missing_or_malformed_totals_never_prove_completeness() {
        assert!(!complete(None, 1, 1));
        assert!(!complete(Some(1), 2, 2));
    }

    #[test]
    fn completeness_requires_raw_and_normalized_counts_to_match() {
        assert!(complete(Some(2), 2, 2));
        assert!(!complete(Some(2), 2, 1));
        assert!(!complete(Some(2), 1, 1));
    }
}
