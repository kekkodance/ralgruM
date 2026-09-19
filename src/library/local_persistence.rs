use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use tokio::sync::oneshot;

use super::{
    local_playlist_store::{LocalPlaylist, LocalPlaylistError, LocalPlaylistStore},
    local_store::{LocalLibraryError, LocalLibraryStore, LocalTrack},
};
use crate::search::Provider;

pub(crate) enum LocalLibraryMutation {
    SetSaved { track: Box<LocalTrack>, saved: bool },
    Reorder { from: usize, to: usize },
}

pub(crate) enum LocalPlaylistMutation {
    Create {
        title: String,
        description: String,
        artwork_jpeg: Option<Vec<u8>>,
    },
    CreateWithTracks {
        title: String,
        description: String,
        tracks: Vec<LocalTrack>,
        artwork_jpeg: Option<Vec<u8>>,
    },
    Update {
        id: String,
        title: String,
        description: String,
        artwork_jpeg: Option<Vec<u8>>,
    },
    AddTracks {
        playlist_id: String,
        tracks: Vec<LocalTrack>,
    },
    Delete {
        id: String,
    },
    RemoveTrack {
        playlist_id: String,
        provider: Provider,
        track_id: String,
    },
    Reorder {
        playlist_id: String,
        from: usize,
        to: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalLibraryMutationOutcome {
    Saved,
    Removed,
    Reordered { changed: bool },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LocalPlaylistMutationOutcome {
    Created,
    Updated,
    Added {
        count: usize,
    },
    AlreadyPresent {
        playlist_title: String,
        count: usize,
    },
    Deleted {
        changed: bool,
    },
    Removed {
        changed: bool,
    },
    Reordered {
        changed: bool,
    },
}

pub(crate) struct LocalLibraryMutationResponse {
    pub(crate) revision: u64,
    pub(crate) state: Result<LocalLibraryStore, LocalLibraryError>,
    pub(crate) outcome: Result<LocalLibraryMutationOutcome, LocalLibraryError>,
}

pub(crate) struct LocalPlaylistMutationResponse {
    pub(crate) revision: u64,
    pub(crate) state: Result<LocalPlaylistStore, LocalPlaylistError>,
    pub(crate) outcome: Result<LocalPlaylistMutationOutcome, LocalPlaylistError>,
}

enum Command {
    Library {
        revision: u64,
        mutation: LocalLibraryMutation,
        reply: oneshot::Sender<LocalLibraryMutationResponse>,
    },
    Playlist {
        revision: u64,
        mutation: LocalPlaylistMutation,
        reply: oneshot::Sender<LocalPlaylistMutationResponse>,
    },
    Flush {
        reply: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub(crate) struct LocalStorageWorker {
    sender: Arc<mpsc::Sender<Command>>,
    next_revision: Arc<AtomicU64>,
}

impl LocalStorageWorker {
    pub(crate) fn new(
        library: Result<LocalLibraryStore, LocalLibraryError>,
        playlists: Result<LocalPlaylistStore, LocalPlaylistError>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        let _ = thread::Builder::new()
            .name("local-storage-worker".to_owned())
            .spawn(move || run(receiver, library, playlists));
        Self {
            sender: Arc::new(sender),
            next_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn submit_library(
        &self,
        mutation: LocalLibraryMutation,
    ) -> Result<(u64, oneshot::Receiver<LocalLibraryMutationResponse>), ()> {
        let revision = self
            .next_revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Command::Library {
                revision,
                mutation,
                reply,
            })
            .map_err(|_| ())?;
        Ok((revision, response))
    }

    pub(crate) fn submit_playlist(
        &self,
        mutation: LocalPlaylistMutation,
    ) -> Result<(u64, oneshot::Receiver<LocalPlaylistMutationResponse>), ()> {
        let revision = self
            .next_revision
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        let (reply, response) = oneshot::channel();
        self.sender
            .send(Command::Playlist {
                revision,
                mutation,
                reply,
            })
            .map_err(|_| ())?;
        Ok((revision, response))
    }

    pub(crate) async fn flush(&self) {
        let (reply, response) = oneshot::channel();
        if self.sender.send(Command::Flush { reply }).is_ok() {
            let _ = response.await;
        }
    }
}

fn run(
    receiver: mpsc::Receiver<Command>,
    mut library: Result<LocalLibraryStore, LocalLibraryError>,
    mut playlists: Result<LocalPlaylistStore, LocalPlaylistError>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Library {
                revision,
                mutation,
                reply,
            } => {
                let outcome = apply_library(&mut library, mutation);
                let _ = reply.send(LocalLibraryMutationResponse {
                    revision,
                    state: library.clone(),
                    outcome,
                });
            }
            Command::Playlist {
                revision,
                mutation,
                reply,
            } => {
                let outcome = apply_playlist(&mut playlists, mutation);
                let _ = reply.send(LocalPlaylistMutationResponse {
                    revision,
                    state: playlists.clone(),
                    outcome,
                });
            }
            Command::Flush { reply } => {
                let _ = reply.send(());
            }
        }
    }
}

fn apply_library(
    library: &mut Result<LocalLibraryStore, LocalLibraryError>,
    mutation: LocalLibraryMutation,
) -> Result<LocalLibraryMutationOutcome, LocalLibraryError> {
    let Ok(store) = library else {
        return Err(library.as_ref().unwrap_err().to_owned());
    };
    match mutation {
        LocalLibraryMutation::SetSaved { track, saved } => {
            if saved {
                store
                    .upsert_track(*track)
                    .map(|()| LocalLibraryMutationOutcome::Saved)
            } else {
                store
                    .remove_track(track.provider, &track.id)
                    .map(|_| LocalLibraryMutationOutcome::Removed)
            }
        }
        LocalLibraryMutation::Reorder { from, to } => store
            .reorder_track(from, to)
            .map(|changed| LocalLibraryMutationOutcome::Reordered { changed }),
    }
}

fn apply_playlist(
    playlists: &mut Result<LocalPlaylistStore, LocalPlaylistError>,
    mutation: LocalPlaylistMutation,
) -> Result<LocalPlaylistMutationOutcome, LocalPlaylistError> {
    let Ok(store) = playlists else {
        return Err(playlists.as_ref().unwrap_err().to_owned());
    };
    match mutation {
        LocalPlaylistMutation::Create {
            title,
            description,
            artwork_jpeg,
        } => store
            .create_with_artwork(title, description, artwork_jpeg.as_deref())
            .map(|_: LocalPlaylist| LocalPlaylistMutationOutcome::Created),
        LocalPlaylistMutation::CreateWithTracks {
            title,
            description,
            tracks,
            artwork_jpeg,
        } => store
            .create_with_tracks_and_artwork(title, description, &tracks, artwork_jpeg.as_deref())
            .map(|_: LocalPlaylist| LocalPlaylistMutationOutcome::Created),
        LocalPlaylistMutation::Update {
            id,
            title,
            description,
            artwork_jpeg,
        } => store
            .update_with_artwork(&id, title, description, artwork_jpeg.as_deref())
            .map(|_: LocalPlaylist| LocalPlaylistMutationOutcome::Updated),
        LocalPlaylistMutation::AddTracks {
            playlist_id,
            tracks,
        } => {
            let playlist_title = store
                .playlist(&playlist_id)
                .map(|playlist| playlist.title.clone())
                .ok_or(LocalPlaylistError::NotFound)?;
            let count = tracks.len();
            store.add_tracks(&playlist_id, &tracks).map(|added| {
                if added == 0 {
                    LocalPlaylistMutationOutcome::AlreadyPresent {
                        playlist_title,
                        count,
                    }
                } else {
                    LocalPlaylistMutationOutcome::Added { count: added }
                }
            })
        }
        LocalPlaylistMutation::Delete { id } => store
            .delete(&id)
            .map(|changed| LocalPlaylistMutationOutcome::Deleted { changed }),
        LocalPlaylistMutation::RemoveTrack {
            playlist_id,
            provider,
            track_id,
        } => store
            .remove_track(&playlist_id, provider, &track_id)
            .map(|changed| LocalPlaylistMutationOutcome::Removed { changed }),
        LocalPlaylistMutation::Reorder {
            playlist_id,
            from,
            to,
        } => store
            .reorder_tracks(&playlist_id, from, to)
            .map(|changed| LocalPlaylistMutationOutcome::Reordered { changed }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Provider;
    use tempfile::TempDir;

    #[test]
    fn revisions_are_assigned_in_submission_order() {
        let directory = TempDir::new().unwrap();
        let worker = LocalStorageWorker::new(
            LocalLibraryStore::load_from_directory(directory.path()),
            LocalPlaylistStore::load_from_directory(directory.path()),
        );
        let (first, _) = worker
            .submit_library(LocalLibraryMutation::Reorder { from: 0, to: 1 })
            .unwrap();
        let (second, _) = worker
            .submit_library(LocalLibraryMutation::Reorder { from: 1, to: 0 })
            .unwrap();
        assert!(second > first);
    }

    #[test]
    fn playlist_commands_are_processed_by_one_ordered_owner() {
        let directory = TempDir::new().unwrap();
        let worker = LocalStorageWorker::new(
            LocalLibraryStore::load_from_directory(directory.path()),
            LocalPlaylistStore::load_from_directory(directory.path()),
        );
        let (create_revision, create) = worker
            .submit_playlist(LocalPlaylistMutation::Create {
                title: "Local".into(),
                description: String::new(),
                artwork_jpeg: None,
            })
            .unwrap();
        let created = create.blocking_recv().unwrap();
        let playlist_id = match created.outcome.unwrap() {
            LocalPlaylistMutationOutcome::Created => {
                let store = created.state.unwrap();
                store.playlists()[0].id.clone()
            }
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };
        let (_, add) = worker
            .submit_playlist(LocalPlaylistMutation::AddTracks {
                playlist_id,
                tracks: vec![LocalTrack {
                    provider: Provider::Deezer,
                    id: "1".into(),
                    title: "Track".into(),
                    artist: "Artist".into(),
                    artists: Vec::new(),
                    album: String::new(),
                    album_id: String::new(),
                    release_date: String::new(),
                    duration: 1,
                    artwork: String::new(),
                    explicit: false,
                    service_url: String::new(),
                }],
            })
            .unwrap();
        let added = add.blocking_recv().unwrap();
        assert_eq!(added.revision, create_revision + 1);
        assert_eq!(
            added.outcome.unwrap(),
            LocalPlaylistMutationOutcome::Added { count: 1 }
        );
    }

    #[test]
    fn flush_waits_for_queued_mutations_even_without_response_consumers() {
        let directory = TempDir::new().unwrap();
        let worker = LocalStorageWorker::new(
            LocalLibraryStore::load_from_directory(directory.path()),
            LocalPlaylistStore::load_from_directory(directory.path()),
        );
        let _ = worker
            .submit_playlist(LocalPlaylistMutation::Create {
                title: "Flushed".into(),
                description: String::new(),
                artwork_jpeg: None,
            })
            .unwrap();

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(worker.flush());

        let stored = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(stored.playlists()[0].title, "Flushed");
    }
}
