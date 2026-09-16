use gpui::px;
use std::sync::atomic::{AtomicUsize, Ordering};

static OPEN_CONTEXT_MENUS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn context_menu_is_open() -> bool {
    OPEN_CONTEXT_MENUS.load(Ordering::Relaxed) > 0
}

#[cfg(test)]
fn open_context_menu_count() -> usize {
    OPEN_CONTEXT_MENUS.load(Ordering::Relaxed)
}

pub(crate) struct ContextMenuLease;

impl ContextMenuLease {
    pub(crate) fn acquire() -> Self {
        OPEN_CONTEXT_MENUS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for ContextMenuLease {
    fn drop(&mut self) {
        OPEN_CONTEXT_MENUS
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_sub(1))
            })
            .ok();
    }
}

mod context_ext;
mod palette;
mod popup;
mod popup_render;
mod text_field;

mod card_menu;
mod collection_button;
mod discover_card;
mod download_menu;
mod items;
mod links;
mod queue_menu;
mod submenu;
mod track_info;
mod track_menu;

mod popup_actions {
    gpui::actions!(
        app_context_menu,
        [
            Cancel,
            Confirm,
            SelectDown,
            SelectFirst,
            SelectLast,
            SelectLeft,
            SelectRight,
            SelectUp
        ]
    );
}

pub(crate) use card_menu::{
    artist_card_menu, card_menu, card_menu_button, library_card_menu, library_card_menu_button,
    local_playlist_card_menu, local_playlist_card_menu_button,
};
pub(crate) use collection_button::collection_download_button;
pub(crate) use context_ext::ContextMenuExt;
pub(crate) use discover_card::{
    DiscoverMenuAction, DiscoverMenuPrimary, discover_card_menu_with_actions,
};
pub(crate) use items::{compact_checked_action_item, lyrics_copy_menu};
pub(crate) use palette::{
    CONTEXT_MENU_BORDER, CONTEXT_MENU_FOREGROUND, CONTEXT_MENU_HOVER,
    CONTEXT_MENU_HOVER_FOREGROUND, CONTEXT_MENU_SEPARATOR, CONTEXT_MENU_SURFACE,
};
pub(crate) use popup::{POPUP_ARROW_EDGE_INSET_PX, PopupMenu, PopupMenuArrowEdge, PopupMenuItem};
pub(crate) use popup_render::{MENU_ARROW_CANVAS_PX, MENU_ARROW_OVERHANG_PX, menu_arrow};
pub(crate) use queue_menu::queue_menu;
#[allow(unused_imports)]
pub(crate) use text_field::{
    TextFieldMenuOptions, text_field_context_menu, text_field_context_menu_with_options,
};
pub(crate) use track_menu::{
    current_menu, track_download_button_above, track_menu, track_menu_button,
};

pub(crate) const ENTITY_MENU_WIDTH: f32 = 272.0;
pub(crate) const COMPACT_MENU_WIDTH: f32 = 144.0;

pub(crate) fn init(cx: &mut gpui::App) {
    popup::init(cx);
}

/// Keep entity context menus at the fixed width used by the original app.
pub(crate) fn style_entity_menu(menu: PopupMenu) -> PopupMenu {
    menu.min_w(px(ENTITY_MENU_WIDTH))
        .max_w(px(ENTITY_MENU_WIDTH))
}

use links::{has_playback_link, has_service_link, service_link, valid_id};

use crate::{
    playback::{PlaybackProvider, PlaybackTrack},
    search::{Card, Provider, ResultType, TrackArtistRef},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EntityKind {
    Track,
    Album,
    Playlist,
    Artist,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuEntity {
    pub(crate) kind: EntityKind,
    pub(crate) provider: Provider,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) track: Option<PlaybackTrack>,
    pub(crate) has_tracks: bool,
    pub(crate) album_id: String,
    pub(crate) artists: Vec<TrackArtistRef>,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id instead.
    pub(crate) service_url: String,
}

impl MenuEntity {
    pub(crate) fn service_link(&self) -> Option<String> {
        service_link(self.provider, &self.kind, &self.id, &self.service_url)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActionAvailability {
    pub(crate) play_next: bool,
    pub(crate) play_last: bool,
    pub(crate) download: bool,
    pub(crate) add_to_playlist: bool,
    pub(crate) favorite: bool,
    pub(crate) station: bool,
    pub(crate) negative_feedback: bool,
    pub(crate) lyrics: bool,
    pub(crate) copy_title: bool,
    pub(crate) copy_link: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueueActionAvailability {
    pub(crate) play_now: bool,
    pub(crate) play_next: bool,
    pub(crate) play_last: bool,
    pub(crate) move_up: bool,
    pub(crate) move_down: bool,
    pub(crate) remove: bool,
    pub(crate) copy_link: bool,
}

pub(crate) fn queue_action_availability(
    track: &PlaybackTrack,
    ordinal: usize,
    count: usize,
    explicit_blocked: bool,
) -> QueueActionAvailability {
    QueueActionAvailability {
        play_now: !explicit_blocked,
        play_next: !explicit_blocked,
        play_last: !explicit_blocked,
        move_up: ordinal > 0,
        move_down: ordinal + 1 < count,
        remove: true,
        copy_link: has_playback_link(track),
    }
}

pub(crate) fn action_availability(
    entity: &MenuEntity,
    explicit_blocked: bool,
) -> ActionAvailability {
    let valid = valid_id(&entity.id).is_some();
    let track = entity.kind == EntityKind::Track;
    let collection = matches!(entity.kind, EntityKind::Album | EntityKind::Playlist);
    let deezer = entity.provider == Provider::Deezer && valid;
    let soundcloud = entity.provider == Provider::SoundCloud && valid;
    ActionAvailability {
        play_next: (track || (collection && entity.has_tracks)) && !explicit_blocked,
        play_last: (track || (collection && entity.has_tracks)) && !explicit_blocked,
        download: track,
        add_to_playlist: track && (deezer || soundcloud),
        favorite: deezer || soundcloud,
        station: track && (deezer || soundcloud),
        negative_feedback: track && deezer,
        lyrics: track,
        copy_title: !entity.title.trim().is_empty(),
        copy_link: has_service_link(entity.provider, &entity.id, &entity.service_url),
    }
}

#[cfg(test)]
pub(crate) fn library_track_entity(
    track: &crate::library::Track,
    provider: Provider,
) -> MenuEntity {
    let playback_track = PlaybackTrack::from_library(track, provider);
    MenuEntity {
        kind: EntityKind::Track,
        provider,
        id: playback_track.id.clone(),
        title: playback_track.title.clone(),
        album_id: playback_track.album_id.clone(),
        artists: playback_track.artists.clone(),
        service_url: playback_track.service_url.clone(),
        track: Some(playback_track),
        has_tracks: true,
    }
}

pub(crate) fn card_entity(card: &Card, has_tracks: bool) -> Option<MenuEntity> {
    let kind = match card.kind {
        ResultType::Albums => EntityKind::Album,
        ResultType::Playlists => EntityKind::Playlist,
        ResultType::Artists => EntityKind::Artist,
        _ => return None,
    };
    valid_id(&card.id).map(|_| MenuEntity {
        kind,
        provider: card.source,
        id: card.id.clone(),
        title: card.title.clone(),
        track: None,
        has_tracks,
        album_id: String::new(),
        artists: Vec::new(),
        service_url: card.service_url.clone(),
    })
}

pub(crate) fn library_card_entity(
    card: &crate::library::Card,
    has_tracks: bool,
) -> Option<MenuEntity> {
    let kind = match card.kind {
        crate::library::Category::Albums => EntityKind::Album,
        crate::library::Category::Playlists => EntityKind::Playlist,
        crate::library::Category::Artists => EntityKind::Artist,
        _ => return None,
    };
    valid_id(&card.id).map(|_| MenuEntity {
        kind,
        provider: card.source,
        id: card.id.clone(),
        title: card.title.clone(),
        track: None,
        has_tracks,
        album_id: String::new(),
        artists: Vec::new(),
        service_url: card.service_url.clone(),
    })
}

/// Builds a menu entity for a queue row so the standard track menu items can
/// be composed with the queue-specific ones.
pub(crate) fn queue_track_entity(track: &PlaybackTrack) -> MenuEntity {
    let provider = match track.provider {
        PlaybackProvider::Deezer => Provider::Deezer,
        PlaybackProvider::SoundCloud => Provider::SoundCloud,
    };
    MenuEntity {
        kind: EntityKind::Track,
        provider,
        id: track.id.clone(),
        title: track.title.clone(),
        album_id: track.album_id.clone(),
        artists: track.artists.clone(),
        service_url: track.service_url.clone(),
        track: Some(track.clone()),
        has_tracks: true,
    }
}

pub(crate) fn copied_toast(label: &str, cx: &mut gpui::App) {
    crate::toast::push_global(
        cx,
        crate::toast::ToastKind::Success,
        format!("{label} copied"),
        Some("Copied to the clipboard.".into()),
    );
}

#[cfg(test)]
mod tests;
