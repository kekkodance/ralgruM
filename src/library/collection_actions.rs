use std::collections::HashSet;

use gpui::{App, Context};

use crate::{
    collection_empty::{
        CollectionAction as EmptyCollectionAction, CollectionKind, empty_collection_notice,
    },
    playback::{DownloadVariant, PlaybackTrack},
    search::Provider,
    toast::ToastKind,
};

use super::{
    model::{Card, Category, Track},
    view::LibraryView,
};

enum CollectionAction {
    Queue { last: bool },
    Download { variant: DownloadVariant },
    AddToPlaylist,
}

impl LibraryView {
    pub(crate) fn deezer_arl_available(&self, cx: &App) -> bool {
        self.account.read(cx).deezer_arl().is_some()
    }

    pub(crate) fn murglar_available(&self, cx: &App) -> bool {
        self.account.read(cx).murglar_media_credentials().is_some()
    }

    pub(crate) fn soundcloud_token_available(&self, cx: &App) -> bool {
        self.account.read(cx).soundcloud_token().is_some()
    }

    pub(crate) fn queue_collection(&mut self, card: Card, last: bool, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::Queue { last }, cx);
    }

    pub(crate) fn download_collection(
        &mut self,
        card: Card,
        variant: DownloadVariant,
        cx: &mut Context<Self>,
    ) {
        self.run_collection_action(&card, CollectionAction::Download { variant }, cx);
    }

    pub(crate) fn add_collection_to_playlist(&mut self, card: Card, cx: &mut Context<Self>) {
        self.run_collection_action(&card, CollectionAction::AddToPlaylist, cx);
    }

    fn run_collection_action(
        &mut self,
        card: &Card,
        action: CollectionAction,
        cx: &mut Context<Self>,
    ) {
        if matches!(action, CollectionAction::AddToPlaylist) && !add_to_playlist_eligible(card) {
            return;
        }
        let Some(route) = collection_route(card) else {
            return;
        };
        let empty_notice =
            empty_collection_notice(collection_kind(card), empty_collection_action(&action));
        let account_scope = self.account.read(cx).library_scope();
        let provider = card.source;
        let task = match provider {
            Provider::Deezer => {
                let Some(arl) = self.account.read(cx).deezer_arl() else {
                    collection_toast(
                        "Collection Unavailable",
                        "A Deezer account is required.",
                        cx,
                    );
                    return;
                };
                let Ok(client) = self.client.clone() else {
                    collection_toast(
                        "Collection Unavailable",
                        "The Deezer library client is unavailable.",
                        cx,
                    );
                    return;
                };
                self.runtime
                    .spawn(async move { client.load_route(route, Some(arl)).await })
            }
            Provider::SoundCloud => {
                let Some(token) = self.account.read(cx).soundcloud_token() else {
                    collection_toast(
                        "Collection Unavailable",
                        "A SoundCloud account is required.",
                        cx,
                    );
                    return;
                };
                let Ok(client) = self.soundcloud_library_client() else {
                    collection_toast(
                        "Collection Unavailable",
                        "The SoundCloud library client is unavailable.",
                        cx,
                    );
                    return;
                };
                self.runtime
                    .spawn(async move { client.load_route(route, Some(token)).await })
            }
        };
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Collection request failed".to_owned()));
            this.update_in(cx, |this, window, cx| {
                if this.account.read(cx).library_scope() != account_scope {
                    return;
                }
                match (result, action) {
                    (Ok(page), CollectionAction::AddToPlaylist) => {
                        let track_ids = playlist_track_ids(&page.tracks);
                        if track_ids.is_empty() {
                            collection_toast(empty_notice.0, empty_notice.1, cx);
                        } else {
                            let status_scope = this.add_status_scope();
                            this.open_add_picker(
                                track_ids,
                                status_scope,
                                Provider::Deezer,
                                window,
                                cx,
                            );
                        }
                    }
                    (Ok(page), action) if !page.tracks.is_empty() => {
                        let tracks = page
                            .tracks
                            .iter()
                            .map(|track| PlaybackTrack::from_library(track, provider))
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
                                    );
                                });
                            }
                            CollectionAction::AddToPlaylist => unreachable!(),
                        }
                    }
                    (Ok(_), _) => collection_toast(empty_notice.0, empty_notice.1, cx),
                    (Err(error), _) => collection_toast("Collection Unavailable", &error, cx),
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
        Category::Playlists => CollectionKind::Playlist,
        Category::Albums => CollectionKind::Album,
        _ => unreachable!("collection actions require an album or playlist"),
    }
}

fn empty_collection_action(action: &CollectionAction) -> EmptyCollectionAction {
    match action {
        CollectionAction::Queue { .. } => EmptyCollectionAction::Queue,
        CollectionAction::Download { .. } => EmptyCollectionAction::Download,
        CollectionAction::AddToPlaylist => EmptyCollectionAction::AddToPlaylist,
    }
}

fn collection_route(card: &Card) -> Option<super::model::Route> {
    if !matches!(card.source, Provider::Deezer | Provider::SoundCloud)
        || !matches!(card.kind, Category::Albums | Category::Playlists)
        || card.id.trim().is_empty()
        || !card.id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    super::view::card_route(card.clone())
}

fn add_to_playlist_eligible(card: &Card) -> bool {
    card.source == Provider::Deezer
        && card.kind == Category::Albums
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

fn collection_toast(title: &str, message: &str, cx: &mut Context<LibraryView>) {
    crate::toast::push_global(
        cx,
        ToastKind::Warning,
        title.to_owned(),
        Some(message.to_owned().into()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(kind: Category, source: Provider, id: &str) -> Card {
        Card {
            kind,
            source,
            id: id.to_owned(),
            title: "Collection".to_owned(),
            ..Card::default()
        }
    }

    fn track(id: &str) -> Track {
        Track {
            id: id.to_owned(),
            ..Track::default()
        }
    }

    #[test]
    fn collection_actions_accept_supported_deezer_and_soundcloud_cards() {
        assert!(collection_route(&card(Category::Albums, Provider::Deezer, "42")).is_some());
        assert!(collection_route(&card(Category::Playlists, Provider::Deezer, "42")).is_some());
        assert!(collection_route(&card(Category::Artists, Provider::Deezer, "42")).is_none());
        assert!(collection_route(&card(Category::Albums, Provider::SoundCloud, "42")).is_some());
        assert!(collection_route(&card(Category::Playlists, Provider::SoundCloud, "42")).is_some());
        assert!(collection_route(&card(Category::Albums, Provider::Deezer, "")).is_none());
    }

    #[test]
    fn add_to_playlist_only_accepts_deezer_albums() {
        assert!(add_to_playlist_eligible(&card(
            Category::Albums,
            Provider::Deezer,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            Category::Playlists,
            Provider::Deezer,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            Category::Albums,
            Provider::SoundCloud,
            "42"
        )));
        assert!(!add_to_playlist_eligible(&card(
            Category::Albums,
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
