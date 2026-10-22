use std::collections::HashSet;

use gpui::Context;

use crate::{
    collection_empty::{
        CollectionAction as EmptyCollectionAction, CollectionKind, empty_collection_notice,
    },
    playback::{DownloadVariant, PlaybackTrack},
};

use super::{
    detail::DetailRoute,
    models::{Card, Track},
    view::SearchView,
};

enum CollectionAction {
    Queue { last: bool },
    PlayNow,
    Download { variant: DownloadVariant },
    AddToLocalPlaylist,
    AddToProviderPlaylist,
    ImportToLocal,
}

impl SearchView {
    /// Queues an album or playlist card next or last. The collection tracks
    /// are fetched first; the current queue is never replaced.
    pub(crate) fn queue_collection(&mut self, card: Card, last: bool, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::Queue { last }, cx);
    }

    pub(crate) fn play_collection(&mut self, card: Card, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::PlayNow, cx);
    }

    /// Batch downloads an album or playlist card in the chosen format. The
    /// collection tracks are fetched first, then handed to the download model.
    pub(crate) fn download_collection(
        &mut self,
        card: Card,
        variant: DownloadVariant,
        cx: &mut Context<Self>,
    ) {
        self.run_collection_action(&card, CollectionAction::Download { variant }, cx);
    }

    pub(crate) fn add_collection_to_playlist(&mut self, card: Card, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::AddToProviderPlaylist, cx);
    }

    pub(crate) fn add_collection_to_local_playlist(&mut self, card: Card, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::AddToLocalPlaylist, cx);
    }

    pub(crate) fn import_collection_to_local(&mut self, card: Card, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::ImportToLocal, cx);
    }

    fn run_collection_action(
        &mut self,
        card: &Card,
        action: CollectionAction,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            action,
            CollectionAction::AddToLocalPlaylist | CollectionAction::AddToProviderPlaylist
        ) && !add_to_playlist_eligible(card)
        {
            return;
        }
        if matches!(action, CollectionAction::ImportToLocal) && !import_to_local_eligible(card) {
            return;
        }
        if matches!(action, CollectionAction::PlayNow)
            && card.kind != super::models::ResultType::Playlists
        {
            return;
        }
        let Some(route) = collection_route(card) else {
            return;
        };
        let empty_notice =
            empty_collection_notice(collection_kind(card), empty_collection_action(&action));
        let Ok(client) = self.client.clone() else {
            return;
        };
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        let provider = card.source;
        let fallback_title = card.title.clone();
        let account_scope = self.account.read(cx).library_scope();
        let runtime = self.runtime.clone();
        cx.notify();
        let task = self
            .runtime
            .spawn(async move { client.detail(route, deezer_arl, soundcloud_token).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(super::models::ProviderError::new(
                    "Collection request failed",
                ))
            });
            let (artwork_jpeg, cover_attempted) =
                if matches!(&action, CollectionAction::ImportToLocal) {
                    match &result {
                        Ok(page) => {
                            let artwork_url = page.route.artwork.clone();
                            let cover_attempted = !artwork_url.trim().is_empty();
                            let artwork_jpeg = runtime
                                .spawn(async move {
                                    crate::library::playlist_image::download_provider_artwork_jpeg(
                                        provider,
                                        &artwork_url,
                                    )
                                    .await
                                })
                                .await
                                .ok()
                                .flatten();
                            (artwork_jpeg, cover_attempted)
                        }
                        Err(_) => (None, false),
                    }
                } else {
                    (None, false)
                };
            this.update_in(cx, |this, window, cx| {
                if matches!(
                    &action,
                    CollectionAction::ImportToLocal | CollectionAction::PlayNow
                ) && this.account.read(cx).library_scope() != account_scope
                {
                    let title = if matches!(&action, CollectionAction::PlayNow) {
                        "Playlist playback canceled"
                    } else {
                        "Playlist import canceled"
                    };
                    collection_toast(
                        title,
                        "The account changed while the playlist was loading. Try again.".to_owned(),
                        cx,
                    );
                    return;
                }
                match (result, action) {
                    (Ok(page), CollectionAction::AddToProviderPlaylist) => {
                        let track_ids = playlist_track_ids(&page.tracks);
                        if track_ids.is_empty() {
                            collection_toast(empty_notice.0, empty_notice.1.to_owned(), cx);
                        } else {
                            this.open_add_picker(track_ids, provider, window, cx);
                        }
                    }
                    (Ok(page), CollectionAction::AddToLocalPlaylist) => {
                        let tracks = page
                            .tracks
                            .iter()
                            .map(PlaybackTrack::from_search)
                            .collect::<Vec<_>>();
                        if tracks.is_empty() {
                            collection_toast(empty_notice.0, empty_notice.1.to_owned(), cx);
                        } else {
                            this.library.update(cx, |library, cx| {
                                library.open_local_playlist_picker_tracks(tracks, window, cx);
                            });
                        }
                    }
                    (Ok(page), CollectionAction::ImportToLocal) => {
                        if page.authoritative_total != Some(page.raw_loaded_count)
                            || page.normalized_count != page.raw_loaded_count
                        {
                            collection_toast(
                                "Playlist import failed",
                                "The playlist did not load completely, so nothing was imported."
                                    .to_owned(),
                                cx,
                            );
                        } else {
                            let tracks = page
                                .tracks
                                .iter()
                                .map(PlaybackTrack::from_search)
                                .collect::<Vec<_>>();
                            let title = if page.route.title.trim().is_empty() {
                                fallback_title
                            } else {
                                page.route.title
                            };
                            let description = if page.description.trim().is_empty() {
                                page.album_info
                                    .as_ref()
                                    .map(|info| info.description.clone())
                                    .unwrap_or_default()
                            } else {
                                page.description
                            };
                            this.library.update(cx, |library, cx| {
                                library.create_imported_local_playlist(
                                    title,
                                    description,
                                    tracks,
                                    artwork_jpeg,
                                    cover_attempted,
                                    cx,
                                );
                            });
                        }
                    }
                    (Ok(page), CollectionAction::PlayNow) => {
                        if page.authoritative_total != Some(page.raw_loaded_count)
                            || page.normalized_count != page.raw_loaded_count
                        {
                            collection_toast(
                                "Playlist unavailable",
                                "The playlist did not load completely, so it was not played."
                                    .to_owned(),
                                cx,
                            );
                        } else {
                            let tracks = page
                                .tracks
                                .iter()
                                .map(PlaybackTrack::from_search)
                                .collect::<Vec<_>>();
                            if tracks.is_empty() {
                                collection_toast(empty_notice.0, empty_notice.1.to_owned(), cx);
                            } else {
                                this.playback
                                    .update(cx, |playback, cx| playback.play_queue(tracks, cx));
                            }
                        }
                    }
                    (Ok(page), action) if !page.tracks.is_empty() => {
                        let tracks = page
                            .tracks
                            .iter()
                            .map(PlaybackTrack::from_search)
                            .collect::<Vec<_>>();
                        match action {
                            CollectionAction::Queue { last } => {
                                this.playback.update(cx, |playback, cx| {
                                    playback.enqueue_tracks(tracks, last, cx)
                                });
                            }
                            CollectionAction::Download { variant } => {
                                let (deezer_arl, soundcloud_token) = {
                                    let account = this.account.read(cx);
                                    (account.deezer_arl(), account.soundcloud_token())
                                };
                                this.downloads.update(cx, |downloads, cx| {
                                    downloads.start_batch(
                                        tracks,
                                        deezer_arl,
                                        soundcloud_token,
                                        variant,
                                        cx,
                                    )
                                });
                            }
                            CollectionAction::AddToLocalPlaylist
                            | CollectionAction::AddToProviderPlaylist
                            | CollectionAction::PlayNow
                            | CollectionAction::ImportToLocal => unreachable!(),
                        }
                    }
                    (Ok(_), _) => collection_toast(empty_notice.0, empty_notice.1.to_owned(), cx),
                    (Err(error), _) => {
                        collection_toast("Collection unavailable", error.message.clone(), cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn collection_kind(card: &Card) -> CollectionKind {
    match card.kind {
        super::models::ResultType::Playlists => CollectionKind::Playlist,
        super::models::ResultType::Albums => CollectionKind::Album,
        _ => unreachable!("collection actions require an album or playlist"),
    }
}

fn empty_collection_action(action: &CollectionAction) -> EmptyCollectionAction {
    match action {
        CollectionAction::Queue { .. } => EmptyCollectionAction::Queue,
        CollectionAction::PlayNow => EmptyCollectionAction::Queue,
        CollectionAction::Download { .. } => EmptyCollectionAction::Download,
        CollectionAction::AddToLocalPlaylist
        | CollectionAction::AddToProviderPlaylist
        | CollectionAction::ImportToLocal => EmptyCollectionAction::AddToPlaylist,
    }
}

fn collection_toast(title: &str, message: String, cx: &mut Context<SearchView>) {
    crate::toast::push_global(
        cx,
        crate::toast::ToastKind::Warning,
        title.to_owned(),
        Some(gpui::SharedString::from(message)),
    );
}

fn collection_route(card: &Card) -> Option<DetailRoute> {
    let route = DetailRoute::from_card(card)?;
    matches!(
        route.kind,
        super::models::ResultType::Albums | super::models::ResultType::Playlists
    )
    .then_some(route)
}

fn add_to_playlist_eligible(card: &Card) -> bool {
    matches!(
        card.source,
        crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
    ) && card.kind == super::models::ResultType::Albums
        && collection_route(card).is_some()
}

fn import_to_local_eligible(card: &Card) -> bool {
    matches!(
        card.source,
        crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
    ) && card.kind == super::models::ResultType::Playlists
        && collection_route(card).is_some()
}

fn playlist_track_ids(tracks: &[Track]) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for track in tracks {
        let id = track.id.trim();
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        if seen.insert(id.to_owned()) {
            ids.push(id.to_owned());
        }
    }
    ids
}

/// True when the given card page can drive collection menu actions.
pub(crate) fn collection_routable(card: &Card) -> bool {
    collection_route(card).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Provider, ResultType};

    fn card(kind: ResultType, provider: Provider, id: &str) -> Card {
        Card {
            kind,
            id: id.into(),
            title: "Collection".into(),
            source: provider,
            ..Card::default()
        }
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.into(),
            ..Track::default()
        }
    }

    #[test]
    fn only_album_and_playlist_cards_have_collection_routes() {
        assert!(collection_routable(&card(
            ResultType::Albums,
            Provider::SoundCloud,
            "42"
        )));
        assert!(collection_routable(&card(
            ResultType::Playlists,
            Provider::Deezer,
            "42"
        )));
        assert!(!collection_routable(&card(
            ResultType::Artists,
            Provider::Deezer,
            "42"
        )));
        assert!(!collection_routable(&card(
            ResultType::Albums,
            Provider::Deezer,
            "not-numeric"
        )));
    }

    #[test]
    fn add_to_playlist_accepts_provider_albums_only() {
        assert!(add_to_playlist_eligible(&card(
            ResultType::Albums,
            Provider::Deezer,
            "42"
        )));
        assert!(add_to_playlist_eligible(&card(
            ResultType::Albums,
            Provider::SoundCloud,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            ResultType::Playlists,
            Provider::Deezer,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            ResultType::Playlists,
            Provider::SoundCloud,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            ResultType::Albums,
            Provider::Deezer,
            "not-numeric"
        )));
    }

    #[test]
    fn playlist_track_ids_skip_invalid_and_keep_order() {
        assert_eq!(
            playlist_track_ids(&[
                track("42"),
                track("abc"),
                track("7"),
                track("42"),
                track(""),
                track(" 9 "),
            ]),
            vec!["42".to_owned(), "7".to_owned(), "9".to_owned()]
        );
        assert!(playlist_track_ids(&[track("abc"), track("")]).is_empty());
    }
}
