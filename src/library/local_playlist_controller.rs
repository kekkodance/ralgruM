use gpui::{Context, Window};
use gpui_component::WindowExt;

use super::{
    local_persistence::{
        LocalPlaylistMutation, LocalPlaylistMutationOutcome, LocalPlaylistMutationResponse,
    },
    local_playlist_delete_dialog::LocalPlaylistDeleteDialog,
    local_playlist_dialog::LocalPlaylistDialog,
    local_playlist_store::{LocalPlaylistError, LocalPlaylistStore},
    local_playlist_view::{local_playlist_page, local_playlists_page},
    model::Service,
    state::{LibraryState, Status},
    view::LibraryView,
};
use crate::search::Provider;

pub(crate) type LocalPlaylistCompletion = Box<
    dyn FnOnce(
            Result<LocalPlaylistMutationOutcome, LocalPlaylistError>,
            &mut LibraryView,
            &mut Context<LibraryView>,
        ) + Send,
>;

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

fn leave_deleted_local_playlist(state: &mut LibraryState, playlist_id: &str) -> bool {
    if state.route().is_local_playlist_detail() && state.route().id == playlist_id {
        let _ = state.back();
        true
    } else {
        false
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

    pub(crate) fn create_local_playlist_with_artwork(
        &mut self,
        title: String,
        description: String,
        artwork_jpeg: Option<&[u8]>,
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::Create {
                title,
                description,
                artwork_jpeg: artwork_jpeg.map(ToOwned::to_owned),
            },
            Box::new(move |result, view, cx| {
                if matches!(result, Ok(LocalPlaylistMutationOutcome::Created)) {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Local playlist created",
                        Some("Your new playlist is ready.".into()),
                    );
                }
                completion(result, view, cx);
            }),
            cx,
        )
    }

    pub(crate) fn create_local_playlist_with_tracks_and_artwork(
        &mut self,
        title: String,
        description: String,
        tracks: &[crate::playback::PlaybackTrack],
        artwork_jpeg: Option<&[u8]>,
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let local_tracks = tracks
            .iter()
            .map(super::local_store::LocalTrack::from)
            .collect::<Vec<_>>();
        self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::CreateWithTracks {
                title,
                description,
                tracks: local_tracks,
                artwork_jpeg: artwork_jpeg.map(ToOwned::to_owned),
            },
            Box::new(move |result, view, cx| {
                if matches!(result, Ok(LocalPlaylistMutationOutcome::Created)) {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Local playlist created",
                        Some("Your track was added to the new playlist.".into()),
                    );
                }
                completion(result, view, cx);
            }),
            cx,
        )
    }

    pub(crate) fn update_local_playlist_with_artwork(
        &mut self,
        id: String,
        title: String,
        description: String,
        artwork_jpeg: Option<&[u8]>,
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::Update {
                id,
                title,
                description,
                artwork_jpeg: artwork_jpeg.map(ToOwned::to_owned),
            },
            Box::new(move |result, view, cx| {
                if matches!(result, Ok(LocalPlaylistMutationOutcome::Updated)) {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Local playlist updated",
                        Some("Your changes were saved.".into()),
                    );
                }
                completion(result, view, cx);
            }),
            cx,
        )
    }

    pub(crate) fn add_tracks_to_local_playlist(
        &mut self,
        playlist_id: String,
        tracks: &[crate::playback::PlaybackTrack],
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        if tracks.is_empty() {
            return Err(LocalPlaylistError::InvalidItem);
        }
        let local_tracks = tracks
            .iter()
            .map(super::local_store::LocalTrack::from)
            .collect::<Vec<_>>();
        self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::AddTracks {
                playlist_id,
                tracks: local_tracks,
            },
            Box::new(move |result, view, cx| {
                match &result {
                    Ok(LocalPlaylistMutationOutcome::Added { count }) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Success,
                            "Added to Local playlist",
                            Some(local_added_tracks_message(*count).into()),
                        );
                    }
                    Ok(LocalPlaylistMutationOutcome::AlreadyPresent { playlist_title }) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Info,
                            "Already in Local playlist",
                            Some(playlist_title.clone().into()),
                        );
                    }
                    _ => {}
                }
                completion(result, view, cx);
            }),
            cx,
        )
    }

    pub(crate) fn delete_local_playlist(
        &mut self,
        id: String,
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::Delete { id: id.clone() },
            Box::new(move |result, view, cx| {
                let result = match result {
                    Ok(LocalPlaylistMutationOutcome::Deleted { changed: true }) => {
                        if leave_deleted_local_playlist(&mut view.state, &id) {
                            view.clear_track_list_states();
                            view.reset_detail_scroll();
                            view.refresh_local_playlist_page();
                        }
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Success,
                            "Local playlist deleted",
                            Some("The playlist was removed from your Local library.".into()),
                        );
                        Ok(LocalPlaylistMutationOutcome::Deleted { changed: true })
                    }
                    Ok(LocalPlaylistMutationOutcome::Deleted { changed: false }) => {
                        Err(LocalPlaylistError::NotFound)
                    }
                    Ok(other) => Ok(other),
                    Err(error) => Err(error),
                };
                completion(result, view, cx);
            }),
            cx,
        )
    }

    pub(crate) fn remove_local_playlist_track(
        &mut self,
        playlist_id: String,
        provider: Provider,
        track_id: String,
        cx: &mut Context<Self>,
    ) {
        let result = self.enqueue_local_playlist_mutation(
            LocalPlaylistMutation::RemoveTrack {
                playlist_id,
                provider,
                track_id,
            },
            Box::new(move |result, _, cx| match result {
                Ok(LocalPlaylistMutationOutcome::Removed { changed: true }) => {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Success,
                        "Track removed from local playlist",
                        None,
                    );
                }
                Ok(LocalPlaylistMutationOutcome::Removed { changed: false }) => {
                    push_local_error(cx, "Track is no longer in this local playlist", None)
                }
                Err(error) => {
                    push_local_error(cx, "Local playlist could not be updated", Some(error))
                }
                _ => {}
            }),
            cx,
        );
        if let Err(error) = result {
            push_local_error(cx, "Local playlist could not be updated", Some(error));
        }
    }

    pub(crate) fn enqueue_local_playlist_mutation(
        &mut self,
        mutation: LocalPlaylistMutation,
        completion: LocalPlaylistCompletion,
        cx: &mut Context<Self>,
    ) -> Result<(), LocalPlaylistError> {
        let (_, response) = self
            .local_persistence
            .submit_playlist(mutation)
            .map_err(|_| LocalPlaylistError::Filesystem)?;
        let entity = cx.entity().clone();
        cx.spawn(async move |_, cx| {
            let result = response.await.map_err(|_| LocalPlaylistError::Filesystem);
            entity.update(cx, |view, cx| {
                let result = match result {
                    Ok(response) => view.apply_local_playlist_response(response, cx),
                    Err(error) => Err(error),
                };
                completion(result, view, cx);
            });
        })
        .detach();
        Ok(())
    }

    pub(super) fn apply_local_playlist_response(
        &mut self,
        response: LocalPlaylistMutationResponse,
        cx: &mut Context<Self>,
    ) -> Result<LocalPlaylistMutationOutcome, LocalPlaylistError> {
        if response.revision > self.local_playlist_revision {
            self.local_playlist_revision = response.revision;
            self.local_playlists = response.state;
            self.refresh_local_playlist_page();
            cx.notify();
        }
        response.outcome
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
    use super::{LibraryState, Provider, Service};
    use crate::library::model::Route;

    #[test]
    fn local_delete_returns_to_the_local_playlists_root() {
        let mut state = LibraryState::default();
        state.reload(Service::Local, super::super::model::Category::Playlists);
        state.push(Route {
            source: Provider::Deezer,
            category: super::super::model::Category::Playlists,
            action: "localPlaylistTracks".into(),
            id: "playlist".into(),
            title: "Playlist".into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        });

        assert!(super::leave_deleted_local_playlist(&mut state, "playlist"));
        assert_eq!(
            state.route(),
            &Route::root(Service::Local, super::super::model::Category::Playlists)
        );
        assert!(!super::leave_deleted_local_playlist(&mut state, "playlist"));
    }

    #[test]
    fn local_added_track_toast_uses_singular_and_plural_copy() {
        assert_eq!(super::local_added_tracks_message(1), "1 track added.");
        assert_eq!(super::local_added_tracks_message(2), "2 tracks added.");
    }
}
