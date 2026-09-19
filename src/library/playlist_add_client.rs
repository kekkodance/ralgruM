use std::collections::HashSet;

use serde_json::{Value, json};

use super::playlist_client::{AddTracksResult, PlaylistClient, Session, valid_id};

const MAX_PLAYLIST_SNAPSHOT: usize = 2000;

pub(super) async fn fallback_add_tracks(
    client: &PlaylistClient,
    session: &Session,
    playlist_id: &str,
    requested: &[String],
) -> Result<AddTracksResult, String> {
    let before = playlist_track_ids(client, session, playlist_id).await?;
    let missing = requested
        .iter()
        .filter(|id| !before.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let duplicated = requested.len() - missing.len();
    if missing.is_empty() {
        return Ok(AddTracksResult::AlreadyPresent { count: duplicated });
    }

    client
        .gateway(
            session,
            "playlist.addSongs",
            add_songs_body(playlist_id, &missing)?,
        )
        .await?;

    let after = playlist_track_ids(client, session, playlist_id)
        .await
        .map_err(|error| {
            format!("Deezer playlist add may have succeeded but could not be checked: {error}")
        })?;
    if missing.iter().any(|id| !after.contains(id.as_str())) {
        return Err("Deezer playlist.addSongs did not confirm every requested track".into());
    }
    if duplicated == 0 {
        Ok(AddTracksResult::Added {
            count: missing.len(),
        })
    } else {
        Ok(AddTracksResult::Partial {
            added: missing.len(),
            duplicated,
        })
    }
}

async fn playlist_track_ids(
    client: &PlaylistClient,
    session: &Session,
    playlist_id: &str,
) -> Result<HashSet<String>, String> {
    let result = client
        .gateway(
            session,
            "playlist.getSongs",
            json!({ "playlist_id": playlist_id, "start": 0, "nb": MAX_PLAYLIST_SNAPSHOT }),
        )
        .await?;
    parse_playlist_track_ids(&result)
}

fn parse_playlist_track_ids(result: &Value) -> Result<HashSet<String>, String> {
    let tracks = result
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "Deezer playlist.getSongs response is missing tracks".to_owned())?;
    let total = result
        .get("total")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Deezer playlist.getSongs response is missing its total".to_owned())?;
    if total != tracks.len() as u64 {
        return Err(
            "The complete Deezer playlist must be loaded before tracks can be added".into(),
        );
    }
    tracks
        .iter()
        .map(|track| {
            let id = track
                .get("SNG_ID")
                .and_then(|value| match value {
                    Value::String(id) => Some(id.clone()),
                    Value::Number(id) => Some(id.to_string()),
                    _ => None,
                })
                .ok_or_else(|| {
                    "Deezer playlist.getSongs returned a track without an id".to_owned()
                })?;
            valid_id(&id)
        })
        .collect()
}

fn add_songs_body(playlist_id: &str, ids: &[String]) -> Result<Value, String> {
    let songs = ids
        .iter()
        .map(|id| {
            valid_id(id)?
                .parse::<u64>()
                .map(|number| json!([number, 0]))
                .map_err(|_| "Deezer track id is too large".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "playlist_id": playlist_id,
        "songs": songs,
        "offset": -1,
        "ctxt": { "id": null, "t": null }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_add_payload_matches_the_playlist_write_contract() {
        assert_eq!(
            add_songs_body("42", &["7".into(), "8".into()]).unwrap(),
            json!({
                "playlist_id": "42",
                "songs": [[7, 0], [8, 0]],
                "offset": -1,
                "ctxt": {"id": null, "t": null}
            })
        );
    }

    #[test]
    fn playlist_snapshot_must_be_complete() {
        let snapshot = json!({"total": 2, "data": [{"SNG_ID": "7"}, {"SNG_ID": 8}]});
        assert_eq!(
            parse_playlist_track_ids(&snapshot).unwrap(),
            HashSet::from(["7".into(), "8".into()])
        );
        assert!(parse_playlist_track_ids(&json!({"total": 3, "data": [{"SNG_ID": "7"}]})).is_err());
    }
}
