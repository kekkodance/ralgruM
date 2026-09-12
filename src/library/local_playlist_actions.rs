use gpui::{App, Context, Window};

use crate::playback::{DownloadVariant, PlaybackTrack};

use super::view::LibraryView;

#[derive(Clone)]
pub(crate) struct LocalPlaylistActionSnapshot {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) tracks: Vec<PlaybackTrack>,
}

impl LibraryView {
    pub(crate) fn local_playlist_action_snapshot(
        &self,
        playlist_id: &str,
    ) -> Option<LocalPlaylistActionSnapshot> {
        let playlist = self.local_playlists.as_ref().ok()?.playlist(playlist_id)?;
        Some(LocalPlaylistActionSnapshot {
            title: playlist.title.clone(),
            description: playlist.description.clone(),
            tracks: playlist
                .tracks
                .iter()
                .map(super::model::Track::from)
                .filter_map(|track| PlaybackTrack::from_local_library(&track))
                .collect(),
        })
    }

    pub(crate) fn local_playlist_download_available(
        &self,
        snapshot: &LocalPlaylistActionSnapshot,
        cx: &App,
    ) -> bool {
        let account = self.account.read(cx);
        crate::playback::collection_download_availability(
            &snapshot.tracks,
            account.deezer_arl().is_some(),
            account.soundcloud_token().is_some(),
            account.murglar_media_credentials().is_some(),
        )
        .best
    }

    pub(crate) fn queue_local_playlist(
        &mut self,
        playlist_id: String,
        last: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.local_playlist_action_snapshot(&playlist_id) else {
            local_playlist_toast(
                "Playlist unavailable",
                "The Local playlist could not be found.",
                cx,
            );
            return;
        };
        if snapshot.tracks.is_empty() {
            local_playlist_toast(
                "Playlist is empty",
                "Add tracks before adding this playlist to the queue.",
                cx,
            );
            return;
        }
        self.playback.update(cx, |playback, cx| {
            playback.enqueue_tracks(snapshot.tracks, last, cx)
        });
    }

    pub(crate) fn download_local_playlist(&mut self, playlist_id: String, cx: &mut Context<Self>) {
        let Some(snapshot) = self.local_playlist_action_snapshot(&playlist_id) else {
            local_playlist_toast(
                "Playlist unavailable",
                "The Local playlist could not be found.",
                cx,
            );
            return;
        };
        if snapshot.tracks.is_empty() {
            local_playlist_toast(
                "Playlist is empty",
                "Add tracks before downloading this playlist.",
                cx,
            );
            return;
        }
        if !self.local_playlist_download_available(&snapshot, cx) {
            local_playlist_toast(
                "Download unavailable",
                "One or more tracks cannot be downloaded with the connected accounts.",
                cx,
            );
            return;
        }
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        self.downloads.update(cx, |downloads, cx| {
            downloads.start_batch(
                snapshot.tracks,
                deezer_arl,
                soundcloud_token,
                DownloadVariant::Best,
                cx,
            );
        });
    }

    pub(crate) fn open_local_playlist_info(
        &mut self,
        playlist_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(snapshot) = self.local_playlist_action_snapshot(&playlist_id) else {
            local_playlist_toast(
                "Playlist unavailable",
                "The Local playlist could not be found.",
                cx,
            );
            return;
        };
        let duration_seconds = snapshot
            .tracks
            .iter()
            .map(|track| track.duration.as_secs())
            .sum();
        crate::search::open_local_playlist_info_dialog(
            snapshot.title,
            snapshot.description,
            snapshot.tracks.len(),
            duration_seconds,
            window,
            cx,
        );
    }
}

fn local_playlist_toast(title: &str, message: &str, cx: &mut Context<LibraryView>) {
    crate::toast::push_global(
        cx,
        crate::toast::ToastKind::Warning,
        title.to_owned(),
        Some(message.to_owned().into()),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn local_collection_actions_do_not_fetch_a_provider_route() {
        let source = include_str!("local_playlist_actions.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("local_playlist_action_snapshot"));
        assert!(production.contains("enqueue_tracks"));
        assert!(production.contains("start_batch"));
        assert!(production.contains("DownloadVariant::Best"));
        assert!(!production.contains("load_route"));
    }
}
