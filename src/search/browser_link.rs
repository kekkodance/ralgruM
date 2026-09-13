use reqwest::{Url, header};
use serde_json::{Value, json};

use crate::{
    browser_link::{
        BrowserEntity, BrowserKind, BrowserProvider, deezer_entity_slug, is_soundcloud_short_url,
        parse_deezer_canonical_entity,
    },
    playback::PlaybackTrack,
    toast::{ToastKind, push_global},
};

use super::{
    SearchView,
    client::{SOUNDCLOUD_CLIENT_ID, SearchClient},
    credential::{DeezerArl, SoundCloudToken},
    models::{Card, Provider, ProviderError, ResultType, Track},
    normalize::{normalize_card, normalize_tracks},
};

enum ResolvedBrowserLink {
    Track(Track),
    Card(Card),
}

impl SearchClient {
    async fn resolve_browser_link(
        &self,
        entity: BrowserEntity,
        deezer_arl: Option<DeezerArl>,
        soundcloud_token: Option<SoundCloudToken>,
    ) -> Result<ResolvedBrowserLink, ProviderError> {
        match entity.provider {
            BrowserProvider::Deezer => self.resolve_deezer_browser_link(entity, deezer_arl).await,
            BrowserProvider::SoundCloud => {
                self.resolve_soundcloud_browser_link(entity, soundcloud_token.as_ref())
                    .await
            }
        }
    }

    async fn resolve_deezer_browser_link(
        &self,
        entity: BrowserEntity,
        arl: Option<DeezerArl>,
    ) -> Result<ResolvedBrowserLink, ProviderError> {
        let entity = self.expand_deezer_short_link(entity).await?;
        if entity.kind != BrowserKind::Track {
            return Ok(ResolvedBrowserLink::Card(Card {
                kind: result_type(entity.kind),
                id: entity.id,
                title: entity.title,
                source: Provider::Deezer,
                service_url: entity.url,
                ..Card::default()
            }));
        }
        let arl = arl.ok_or_else(|| ProviderError::new("Sign in to Deezer to play this track"))?;
        let session = self.deezer_session(Some(arl)).await?;
        let response = self
            .deezer_gateway(
                &session,
                "song.getListData",
                json!({"sng_ids": [entity.id]}),
            )
            .await?;
        let item = response
            .pointer("/results/data/0")
            .ok_or_else(|| ProviderError::new("Deezer could not find this track"))?;
        let track = normalize_tracks(Provider::Deezer, std::slice::from_ref(item))
            .into_iter()
            .next()
            .filter(|track| !track.id.is_empty())
            .ok_or_else(|| ProviderError::new("Deezer returned an invalid track"))?;
        Ok(ResolvedBrowserLink::Track(track))
    }

    /// Share shortlinks carry an opaque code instead of a numeric id, so the
    /// redirect chain is followed natively and the canonical page parsed.
    /// Returns the entity unchanged when it already carries an id.
    async fn expand_deezer_short_link(
        &self,
        mut entity: BrowserEntity,
    ) -> Result<BrowserEntity, ProviderError> {
        if !entity.id.is_empty() {
            return Ok(entity);
        }
        let final_url = self
            .follow_short_link(&entity.url)
            .await
            .ok_or_else(|| ProviderError::new("Deezer could not resolve this share link"))?;
        let (kind, id) = parse_deezer_canonical_entity(&final_url)
            .ok_or_else(|| ProviderError::new("Deezer could not resolve this share link"))?;
        entity.kind = kind;
        entity.id = id.clone();
        entity.url = format!("https://www.deezer.com/{}/{id}", deezer_entity_slug(kind));
        Ok(entity)
    }

    /// Follows a share shortlink to its final URL. Web pages cannot do this
    /// (CORS hides cross origin redirect targets), but native code can.
    async fn follow_short_link(&self, url: &str) -> Option<String> {
        let response = self.http().get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        Some(response.url().as_str().to_owned())
    }

    async fn resolve_soundcloud_browser_link(
        &self,
        entity: BrowserEntity,
        token: Option<&SoundCloudToken>,
    ) -> Result<ResolvedBrowserLink, ProviderError> {
        let short = is_soundcloud_short_url(&entity.url);
        let mut entity = entity;
        if short {
            entity.url = self
                .follow_short_link(&entity.url)
                .await
                .ok_or_else(|| ProviderError::new("SoundCloud could not resolve this link"))?;
        }
        let mut endpoint = Url::parse("https://api-v2.soundcloud.com/resolve")
            .map_err(|_| ProviderError::new("Invalid SoundCloud resolve endpoint"))?;
        endpoint
            .query_pairs_mut()
            .append_pair("url", &entity.url)
            .append_pair("client_id", SOUNDCLOUD_CLIENT_ID);
        let mut request = self.http().get(endpoint);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, token.authorization_header()?);
        }
        let response = request
            .send()
            .await
            .map_err(|_| ProviderError::new("SoundCloud could not resolve this link"))?;
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
        // Shorts arrive with a placeholder kind, so it is read back from the
        // resolved payload; canonical links stay strictly checked.
        let requested_kind = if short {
            browser_kind_from_soundcloud_payload(&value)
                .ok_or_else(|| ProviderError::new("SoundCloud returned an invalid item"))?
        } else {
            entity.kind
        };
        normalize_soundcloud_entity(requested_kind, &value)
    }
}

fn browser_kind_from_soundcloud_payload(value: &Value) -> Option<BrowserKind> {
    match value.get("kind").and_then(Value::as_str)? {
        "track" => Some(BrowserKind::Track),
        "user" => Some(BrowserKind::Artist),
        // api-v2 reports both albums and playlists as playlists.
        "playlist" => Some(BrowserKind::Playlist),
        _ => None,
    }
}

fn normalize_soundcloud_entity(
    requested_kind: BrowserKind,
    value: &Value,
) -> Result<ResolvedBrowserLink, ProviderError> {
    let actual_kind = value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let kind_matches = match requested_kind {
        BrowserKind::Track => actual_kind == "track",
        BrowserKind::Artist => actual_kind == "user",
        BrowserKind::Album | BrowserKind::Playlist => actual_kind == "playlist",
    };
    if !kind_matches {
        return Err(ProviderError::new("The SoundCloud link type did not match"));
    }
    if requested_kind == BrowserKind::Track {
        let track = normalize_tracks(Provider::SoundCloud, std::slice::from_ref(value))
            .into_iter()
            .next()
            .filter(|track| !track.id.is_empty())
            .ok_or_else(|| ProviderError::new("SoundCloud returned an invalid track"))?;
        return Ok(ResolvedBrowserLink::Track(track));
    }
    let card = normalize_card(Provider::SoundCloud, result_type(requested_kind), value);
    if card.id.is_empty() {
        return Err(ProviderError::new("SoundCloud returned an invalid item"));
    }
    Ok(ResolvedBrowserLink::Card(card))
}

const fn result_type(kind: BrowserKind) -> ResultType {
    match kind {
        BrowserKind::Track => ResultType::Tracks,
        BrowserKind::Album => ResultType::Albums,
        BrowserKind::Playlist => ResultType::Playlists,
        BrowserKind::Artist => ResultType::Artists,
    }
}

impl SearchView {
    pub(crate) fn open_browser_link(
        &mut self,
        entity: BrowserEntity,
        cx: &mut gpui::Context<Self>,
    ) {
        let Ok(client) = self.client.clone() else {
            push_global(cx, ToastKind::Error, "Could not open browser link", None);
            return;
        };
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        let task = self.runtime.spawn(async move {
            client
                .resolve_browser_link(entity, deezer_arl, soundcloud_token)
                .await
        });
        cx.spawn(async move |this, cx| {
            let resolved = match task.await {
                Ok(Ok(resolved)) => resolved,
                Ok(Err(error)) => {
                    cx.update(|cx| {
                        push_global(
                            cx,
                            ToastKind::Error,
                            "Could not open browser link",
                            Some(error.message.into()),
                        )
                    });
                    return;
                }
                Err(_) => {
                    cx.update(|cx| {
                        push_global(cx, ToastKind::Error, "Could not open browser link", None)
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| match resolved {
                ResolvedBrowserLink::Track(track) => {
                    this.playback.update(cx, |playback, cx| {
                        playback.replace_queue(vec![PlaybackTrack::from_search(&track)], 0, cx)
                    });
                }
                ResolvedBrowserLink::Card(card) => this.open_external_card(card, cx),
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn soundcloud_track_resolution_rejects_a_collection_payload() {
        let error = normalize_soundcloud_entity(
            BrowserKind::Track,
            &json!({"kind": "playlist", "id": 42, "title": "Wrong"}),
        )
        .err()
        .expect("mismatched payload must fail");
        assert_eq!(error.message, "The SoundCloud link type did not match");
    }

    #[test]
    fn soundcloud_resolved_track_uses_the_shared_normalizer() {
        let resolved = normalize_soundcloud_entity(
            BrowserKind::Track,
            &json!({
                "kind": "track",
                "id": 42,
                "title": "Artist - Song",
                "duration": 123000,
                "permalink_url": "https://soundcloud.com/artist/song",
                "user": {"id": 7, "username": "Artist"}
            }),
        )
        .expect("track payload");
        let ResolvedBrowserLink::Track(track) = resolved else {
            panic!("expected track")
        };
        assert_eq!(track.id, "42");
        assert_eq!(track.title, "Song");
        assert_eq!(track.duration, 123);
    }

    #[test]
    fn soundcloud_resolved_artist_uses_the_shared_card_normalizer() {
        let resolved = normalize_soundcloud_entity(
            BrowserKind::Artist,
            &json!({
                "kind": "user",
                "id": 7,
                "username": "Artist",
                "permalink_url": "https://soundcloud.com/artist"
            }),
        )
        .expect("artist payload");
        let ResolvedBrowserLink::Card(card) = resolved else {
            panic!("expected card")
        };
        assert_eq!(card.id, "7");
        assert_eq!(card.kind, ResultType::Artists);
    }
}
