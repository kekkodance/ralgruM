use gpui::{Context, Window};
use gpui_component::WindowExt;

use super::{
    local_playlist_delete_dialog::LocalPlaylistDeleteDialog,
    local_playlist_dialog::LocalPlaylistDialog,
    local_playlist_store::{LocalPlaylistError, LocalPlaylistStore},
    local_playlist_view::{local_playlist_page, local_playlists_page},
    model::Service,
    state::Status,
    view::LibraryView,
};
use crate::search::Provider;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalPlaylistAddResult {
    Added { count: usize },
    AlreadyPresent { playlist_title: String },
}

fn local_picker_storage_error(
    local_playlists: &Result<LocalPlaylistStore, LocalPlaylistError>,
) -> Option<LocalPlaylistError> {
    local_playlists.as_ref().err().copied()
}

fn local_added_tracks_message(count: usize) -> String {
    if count == 1 {
        "1 track added.".to_owned()
    } else {
        format!("{count} tracks added.")
    }
}

impl LibraryView {
    pub(crate) fn open_local_playlist_picker(
        &mut self,
        track: crate::playback::PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        if let Some(error) = local_picker_storage_error(&self.local_playlists) {
            push_local_error(cx, "Local playlist storage is unavailable", Some(error));
            return;
        }
        super::playlist_picker::PlaylistPicker::open_local(cx.entity(), vec![track], window, cx);
    }

    pub(crate) fn open_local_playlist_create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.state.service != Service::Local {
            return;
        }
        LocalPlaylistDialog::open_create(cx.entity(), window, cx);
    }

    pub(crate) fn open_local_playlist_editor(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.state.service != Service::Local {
            return;
        }
        let Some(playlist) = self
            .local_playlists
            .as_ref()
            .ok()
            .and_then(|store| store.playlist(&id).cloned())
        else {
            push_local_error(cx, "Local playlist could not be opened for editing", None);
            return;
        };
        let existing_cover_path = self
            .local_playlists
            .as_ref()
            .ok()
            .and_then(|store| store.artwork_path(&playlist.id, &playlist.artwork));
        LocalPlaylistDialog::open_edit(cx.entity(), playlist, existing_cover_path, window, cx);
    }

    pub(crate) fn open_local_playlist_delete(
        &mut self,
        id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) || self.state.service != Service::Local {
            return;
        }
        if self
            .local_playlists
            .as_ref()
            .ok()
            .and_then(|store| store.playlist(&id))
            .is_none()
        {
            push_local_error(cx, "Local playlist could not be found", None);
            return;
        }
        LocalPlaylistDeleteDialog::open(cx.entity(), id, title, window, cx);
    }

    pub(crate) fn create_local_playlist(
        &mut self,
        title: String,
        description: String,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.create_local_playlist_with_artwork(title, description, None, cx)
    }

    pub(crate) fn create_local_playlist_with_artwork(
        &mut self,
        title: String,
        description: String,
        artwork_jpeg: Option<&[u8]>,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let result = match self.local_playlists.as_mut() {
            Ok(store) => store
                .create_with_artwork(title, description, artwork_jpeg)
                .map(|_| ()),
            Err(error) => Err(*error),
        };
        if result.is_ok() {
            self.refresh_local_playlist_page();
            cx.notify();
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Success,
                "Local playlist created",
                Some("Your new playlist is ready.".into()),
            );
        }
        result
    }

    pub(crate) fn create_local_playlist_with_tracks(
        &mut self,
        title: String,
        description: String,
        tracks: &[crate::playback::PlaybackTrack],
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.create_local_playlist_with_tracks_and_artwork(title, description, tracks, None, cx)
    }

    pub(crate) fn create_local_playlist_with_tracks_and_artwork(
        &mut self,
        title: String,
        description: String,
        tracks: &[crate::playback::PlaybackTrack],
        artwork_jpeg: Option<&[u8]>,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let local_tracks = tracks
            .iter()
            .map(super::local_store::LocalTrack::from)
            .collect::<Vec<_>>();
        let result = match self.local_playlists.as_mut() {
            Ok(store) => store
                .create_with_tracks_and_artwork(title, description, &local_tracks, artwork_jpeg)
                .map(|_| ()),
            Err(error) => Err(*error),
        };
        if result.is_ok() {
            self.refresh_local_playlist_page();
            cx.notify();
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Success,
                "Local playlist created",
                Some("Your track was added to the new playlist.".into()),
            );
        }
        result
    }

    pub(crate) fn update_local_playlist(
        &mut self,
        id: String,
        title: String,
        description: String,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.update_local_playlist_with_artwork(id, title, description, None, cx)
    }

    pub(crate) fn update_local_playlist_with_artwork(
        &mut self,
        id: String,
        title: String,
        description: String,
        artwork_jpeg: Option<&[u8]>,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let result = match self.local_playlists.as_mut() {
            Ok(store) => store
                .update_with_artwork(&id, title, description, artwork_jpeg)
                .map(|_| ()),
            Err(error) => Err(*error),
        };
        if result.is_ok() {
            self.refresh_local_playlist_page();
            cx.notify();
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Success,
                "Local playlist updated",
                Some("Your changes were saved.".into()),
            );
        }
        result
    }

    pub(crate) fn add_tracks_to_local_playlist(
        &mut self,
        playlist_id: String,
        tracks: &[crate::playback::PlaybackTrack],
        cx: &mut Context<Self>,
    ) -> Result<LocalPlaylistAddResult, LocalPlaylistError> {
        if tracks.is_empty() {
            return Err(LocalPlaylistError::InvalidItem);
        }
        let local_tracks = tracks
            .iter()
            .map(super::local_store::LocalTrack::from)
            .collect::<Vec<_>>();
        let result = match self.local_playlists.as_mut() {
            Ok(store) => {
                let (playlist_title, duplicate) = store
                    .playlist(&playlist_id)
                    .map(|playlist| {
                        let duplicate = local_tracks.iter().any(|track| {
                            playlist.tracks.iter().any(|saved| {
                                saved.provider == track.provider && saved.id == track.id.trim()
                            })
                        });
                        (playlist.title.clone(), duplicate)
                    })
                    .ok_or(LocalPlaylistError::NotFound)?;
                if duplicate {
                    Ok(LocalPlaylistAddResult::AlreadyPresent { playlist_title })
                } else {
                    store.add_tracks(&playlist_id, &local_tracks).map(|()| {
                        LocalPlaylistAddResult::Added {
                            count: local_tracks.len(),
                        }
                    })
                }
            }
            Err(error) => Err(*error),
        };
        match &result {
            Ok(LocalPlaylistAddResult::Added { count }) => {
                self.refresh_local_playlist_page();
                cx.notify();
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Success,
                    "Added to Local playlist",
                    Some(local_added_tracks_message(*count).into()),
                );
            }
            Ok(LocalPlaylistAddResult::AlreadyPresent { playlist_title }) => {
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Info,
                    "Already in Local playlist",
                    Some(playlist_title.clone().into()),
                );
            }
            Err(_) => {}
        }
        result
    }

    pub(crate) fn delete_local_playlist(
        &mut self,
        id: String,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let result = match self.local_playlists.as_mut() {
            Ok(store) => store.delete(&id),
            Err(error) => Err(*error),
        };
        match result {
            Ok(true) => {
                let deleted_detail =
                    self.state.route().is_local_playlist_detail() && self.state.route().id == id;
                if deleted_detail {
                    let _ = self.state.back();
                    self.clear_track_list_states();
                    self.reset_detail_scroll();
                }
                self.refresh_local_playlist_page();
                cx.notify();
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Success,
                    "Local playlist deleted",
                    Some("The playlist was removed from your Local library.".into()),
                );
                Ok(())
            }
            Ok(false) => Err(LocalPlaylistError::NotFound),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn remove_local_playlist_track(
        &mut self,
        playlist_id: String,
        provider: Provider,
        track_id: String,
        cx: &mut Context<Self>,
    ) {
        let result = match self.local_playlists.as_mut() {
            Ok(store) => store
                .remove_track(&playlist_id, provider, &track_id)
                .map(|removed| removed.then_some(())),
            Err(error) => Err(*error),
        };
        match result {
            Ok(Some(())) => {
                self.refresh_local_playlist_page();
                cx.notify();
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Success,
                    "Track removed from local playlist",
                    None,
                );
            }
            Ok(None) => push_local_error(cx, "Track is no longer in this local playlist", None),
            Err(error) => push_local_error(cx, "Local playlist could not be updated", Some(error)),
        }
    }

    fn refresh_local_playlist_page(&mut self) {
        let route = self.state.route().clone();
        let page = match self.local_playlists.as_ref() {
            Ok(store) if route.is_local_playlist_detail() => {
                store.playlist(&route.id).map(local_playlist_page)
            }
            Ok(store) if route.is_local_playlists_root() => {
                Some(local_playlists_page(store.playlists()))
            }
            _ => None,
        };
        if let Some(page) = page {
            self.state.status = if page.is_empty() {
                Status::Empty
            } else {
                Status::Results
            };
            self.state.page = Some(page);
        } else if route.is_local_playlist_detail() {
            self.state.status = Status::Failed("The local playlist could not be found.".into());
            self.state.page = None;
        }
    }
}

fn push_local_error(
    cx: &mut Context<LibraryView>,
    title: &'static str,
    error: Option<LocalPlaylistError>,
) {
    crate::toast::push_global(
        cx,
        crate::toast::ToastKind::Error,
        title,
        error.map(|error| error.to_string().into()),
    );
}

#[cfg(test)]
mod tests {
    use super::super::local_playlist_store::LocalPlaylistError;

    #[test]
    fn local_mutations_refresh_the_visible_page_without_reloading_the_route() {
        let source = include_str!("local_playlist_controller.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(source.contains("fn refresh_local_playlist_page(&mut self)"));
        assert!(source.contains("self.state.page = Some(page)"));
        assert!(!source.contains("self.load_service(Service::Local"));
        assert!(!source.contains("self.load_nested("));
    }

    #[test]
    fn local_delete_returns_to_the_local_playlists_root() {
        let source = include_str!("local_playlist_controller.rs");
        let delete = source
            .split("pub(crate) fn delete_local_playlist")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) fn remove_local_playlist_track")
                    .next()
            })
            .expect("local delete controller");
        assert!(delete.contains("self.state.route().is_local_playlist_detail()"));
        assert!(delete.contains("self.state.back()"));
        assert!(delete.contains("self.refresh_local_playlist_page()"));
    }

    #[test]
    fn local_track_addition_uses_full_playback_metadata_and_refreshes_in_place() {
        let source = include_str!("local_playlist_controller.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("LocalTrack::from"));
        assert!(production.contains("add_tracks_to_local_playlist"));
        assert!(production.contains("AlreadyPresent"));
        assert!(production.contains("self.refresh_local_playlist_page()"));
        assert!(production.contains("ToastKind::Info"));
    }

    #[test]
    fn local_picker_does_not_open_when_storage_loading_failed() {
        assert_eq!(
            super::local_picker_storage_error(&Err(LocalPlaylistError::Filesystem)),
            Some(LocalPlaylistError::Filesystem)
        );
        let source = include_str!("local_playlist_controller.rs");
        let open = source
            .split("pub(crate) fn open_local_playlist_picker")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) fn open_local_playlist_create")
                    .next()
            })
            .expect("local picker opener");
        assert!(open.contains("local_picker_storage_error"));
        assert!(open.contains("return;"));
    }

    #[test]
    fn local_added_track_toast_uses_singular_and_plural_copy() {
        assert_eq!(super::local_added_tracks_message(1), "1 track added.");
        assert_eq!(super::local_added_tracks_message(2), "2 tracks added.");
    }
}
