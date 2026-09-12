use crate::search::Provider;

use super::{
    local_playlist_store::LocalPlaylist,
    model::{Card, Category, Page, Service, Track},
};

pub(crate) fn local_playlists_page(playlists: &[LocalPlaylist]) -> Page {
    let cards = playlists
        .iter()
        .map(|playlist| Card {
            kind: Category::Playlists,
            id: playlist.id.clone(),
            title: playlist.title.clone(),
            subtitle: playlist.description.clone(),
            artwork: playlist.artwork.clone(),
            release_date: String::new(),
            badge: playlist.tracks.len().to_string(),
            source: Provider::Deezer,
            service_url: String::new(),
            is_private: None,
            library_service: Some(Service::Local),
        })
        .collect::<Vec<_>>();
    Page {
        title: "Local Playlists".into(),
        description:
            "Playlists organized in your local library with tracks from Deezer and SoundCloud."
                .into(),
        platform: Some(Service::Local),
        count_noun: "playlist".into(),
        total: cards.len(),
        raw_loaded_count: cards.len(),
        normalized_count: cards.len(),
        authoritative_total: Some(cards.len()),
        cards,
        empty_title: "No local playlists yet".into(),
        empty_description: "Create a local playlist to organize tracks from all platforms.".into(),
        ..Page::default()
    }
}

pub(crate) fn local_playlist_page(playlist: &LocalPlaylist) -> Page {
    let tracks = playlist.tracks.iter().map(Track::from).collect::<Vec<_>>();
    Page {
        title: playlist.title.clone(),
        description: playlist.description.clone(),
        artwork: playlist.artwork.clone(),
        platform: Some(Service::Local),
        count_noun: "track".into(),
        total: tracks.len(),
        raw_loaded_count: tracks.len(),
        normalized_count: tracks.len(),
        authoritative_total: Some(tracks.len()),
        tracks,
        empty_title: "This local playlist is empty".into(),
        empty_description:
            "Add tracks from Deezer or SoundCloud to organize their references here. Audio files are not downloaded.".into(),
        ..Page::default()
    }
}
