use gpui::{App, Context, DragMoveEvent, Window};

use super::{
    detail::DetailState,
    models::{Provider, ResultType, Track},
    view::SearchView,
};

pub(super) struct PlaylistReorderSnapshot {
    account_scope: String,
    provider: Provider,
    playlist_id: String,
    view_id: u64,
    tracks: Vec<Track>,
}

impl SearchView {
    pub(super) fn update_playlist_drag_autoscroll(
        &mut self,
        event: &DragMoveEvent<crate::library::playlist_drag::PlaylistTrackDrag>,
        scroll: crate::browser_scroll::FixedListScrollHandle,
        cx: &mut Context<Self>,
    ) {
        self.playlist_drag_scroll =
            Some(crate::library::playlist_drag::PlaylistDragAutoScroll::from_event(event, scroll));
        self.run_playlist_drag_autoscroll(cx);
    }

    fn step_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.playlist_drag_scroll.as_ref() else {
            self.playlist_drag_scroll_running = false;
            return false;
        };
        if !cx.has_active_drag() {
            self.playlist_drag_scroll = None;
            self.playlist_drag_scroll_running = false;
            cx.notify();
            return false;
        }
        drag.step();
        cx.notify();
        true
    }

    fn run_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.playlist_drag_scroll_running {
            return;
        }
        self.playlist_drag_scroll_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor
                    .timer(crate::library::playlist_drag::DRAG_SCROLL_TICK)
                    .await;
                let alive = this
                    .update(cx, |view, cx| view.step_playlist_drag_autoscroll(cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn sync_playlist_update(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.detail.route.as_ref().map(|route| route.provider) else {
            return;
        };
        let updated = {
            let library = self.library.read(cx);
            let catalog = library.playlist_catalog(provider);
            let revision = catalog.update_revision;
            let seen_revision = match provider {
                Provider::Deezer => &mut self.playlist_update_revision,
                Provider::SoundCloud => &mut self.soundcloud_playlist_update_revision,
            };
            if revision == *seen_revision {
                return;
            }
            *seen_revision = revision;
            catalog.updated.clone()
        };
        let Some(updated) = updated else { return };
        let Some(current_route) = self.detail.route.as_ref() else {
            return;
        };
        if current_route.kind != ResultType::Playlists
            || !crate::library::playlist_state::matching_playlist_route(
                current_route.provider,
                "playlistTracks",
                &current_route.id,
                &updated.id,
            )
        {
            return;
        }
        let title = updated.title.clone();
        let subtitle = crate::library::playlist_state::updated_playlist_subtitle(
            &current_route.subtitle,
            &updated.owner.name,
        );
        let artwork = (!updated.artwork.is_empty()).then(|| updated.artwork.clone());
        let description = updated.description.clone();
        let Some(route) = self.detail.route.as_mut() else {
            return;
        };
        route.title = title.clone();
        route.subtitle = subtitle.clone();
        if let Some(artwork) = artwork.as_deref() {
            route.artwork = artwork.to_owned();
        }
        match &mut self.detail.state {
            DetailState::Results(page) | DetailState::Empty(page) => {
                page.route.title = title;
                page.route.subtitle = subtitle;
                if let Some(artwork) = artwork {
                    page.route.artwork = artwork;
                }
                page.description = description;
            }
            _ => {}
        }
        cx.notify();
    }

    pub(crate) fn sync_playlist_content(&mut self, cx: &mut Context<Self>) {
        let playlist_id = {
            let library = self.library.read(cx);
            let revision = library.playlists.content_revision;
            if revision == self.playlist_content_revision {
                return;
            }
            self.playlist_content_revision = revision;
            library.playlists.added_to.clone()
        };
        let Some(playlist_id) = playlist_id else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if route.kind != ResultType::Playlists
            || !crate::library::playlist_state::matching_playlist_route(
                route.provider,
                "playlistTracks",
                &route.id,
                &playlist_id,
            )
        {
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }

    pub(crate) fn sync_playlist_reorder(&mut self, cx: &mut Context<Self>) {
        let Some(provider) = self.detail.route.as_ref().map(|route| route.provider) else {
            return;
        };
        cx.notify();
        let completion = {
            let library = self.library.read(cx);
            let catalog = library.playlist_catalog(provider);
            let revision = catalog.reorder_completion_revision;
            if self.playlist_reorder_provider == Some(provider)
                && revision == self.playlist_reorder_revision
            {
                return;
            }
            self.playlist_reorder_provider = Some(provider);
            self.playlist_reorder_revision = revision;
            (
                catalog.reorder_completion_route.clone(),
                catalog.reorder_completion_succeeded,
            )
        };
        let (Some(revision_route), Some(succeeded)) = completion else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if !crate::library::playlist_state::matching_playlist_route(
            route.provider,
            "playlistTracks",
            &route.id,
            &revision_route.playlist_id,
        ) || revision_route.account_scope != self.account_scope
        {
            return;
        }
        let matching_snapshot = self
            .playlist_reorder_snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.account_scope == self.account_scope
                    && snapshot.provider == route.provider
                    && snapshot.playlist_id == route.id
                    && snapshot.view_id == self.detail.view_id()
            });
        if matching_snapshot {
            if !succeeded {
                let snapshot = self
                    .playlist_reorder_snapshot
                    .take()
                    .expect("matching playlist reorder snapshot");
                if let DetailState::Results(page) = &mut self.detail.state {
                    page.tracks = snapshot.tracks;
                }
            } else {
                self.playlist_reorder_snapshot = None;
            }
            cx.notify();
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }

    pub(crate) fn playlist_reorder_detail_eligible(&self, cx: &Context<SearchView>) -> bool {
        let Some(route) = self.detail.route.as_ref() else {
            return false;
        };
        let DetailState::Results(page) = &self.detail.state else {
            return false;
        };
        let catalog = self.library.read(cx).playlist_catalog(route.provider);
        !catalog.reorder_pending
            && crate::library::playlist_reorder::detail_eligible(
                route.provider,
                route.kind,
                catalog.is_editable(&route.id),
                page.total,
                page.raw_loaded_count,
                page.normalized_count,
                &page.tracks,
            )
    }

    pub(crate) fn move_playlist_track(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        if !self.playlist_reorder_detail_eligible(cx) {
            return;
        }
        let Some(route) = self.detail.route.clone() else {
            return;
        };
        let DetailState::Results(page) = &self.detail.state else {
            return;
        };
        let original_tracks = page.tracks.clone();
        let previous = page
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let Some(reordered) =
            crate::library::playlist_reorder::reorder_items(&page.tracks, from, to)
        else {
            return;
        };
        let submitted = reordered
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let view_id = self.detail.view_id();
        let started = self.library.update(cx, |library, cx| {
            library.start_playlist_reorder(
                route.provider,
                route.id.clone(),
                None,
                previous,
                submitted,
                cx,
            )
        });
        if !started || self.detail.view_id() != view_id {
            return;
        }
        let Some(active_route) = self.detail.route.as_ref() else {
            return;
        };
        if active_route.provider != route.provider
            || active_route.kind != route.kind
            || active_route.id != route.id
        {
            return;
        }
        let DetailState::Results(page) = &mut self.detail.state else {
            return;
        };
        page.tracks = reordered;
        self.playlist_reorder_snapshot = Some(PlaylistReorderSnapshot {
            account_scope: self.account_scope.clone(),
            provider: route.provider,
            playlist_id: route.id,
            view_id,
            tracks: original_tracks,
        });
        cx.notify();
    }

    pub(crate) fn playlist_editable(&self, provider: Provider, id: &str, cx: &App) -> bool {
        self.library.read(cx).playlist_editable(provider, id)
    }

    pub(crate) fn is_playlist_owned(
        &self,
        provider: Provider,
        id: &str,
        subtitle: &str,
        cx: &App,
    ) -> bool {
        if self.playlist_editable(provider, id, cx) {
            return true;
        }
        if provider != Provider::Deezer {
            return false;
        }
        let account = self.account.read(cx);
        if let Some(profile) = account.deezer_profile()
            && !profile.username.is_empty()
            && profile.username.eq_ignore_ascii_case(subtitle.trim())
        {
            return true;
        }
        if let Some(user_id) = account.deezer_user_id()
            && !user_id.is_empty()
            && user_id.trim() == subtitle.trim()
        {
            return true;
        }
        false
    }

    pub(crate) fn open_playlist_editor(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.open_playlist_editor(provider, id, window, cx)
        });
    }

    pub(crate) fn open_playlist_delete(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.library.update(cx, |library, cx| {
            library.open_playlist_delete(provider, id, window, cx)
        });
    }

    pub(crate) fn sync_playlist_delete(&mut self, cx: &mut Context<Self>) -> Option<Provider> {
        let deleted = {
            let library = self.library.read(cx);
            if library.playlists.delete_revision != self.playlist_delete_revision {
                self.playlist_delete_revision = library.playlists.delete_revision;
                Some((Provider::Deezer, library.playlists.deleted.clone()))
            } else if library.soundcloud_playlists.delete_revision
                != self.soundcloud_playlist_delete_revision
            {
                self.soundcloud_playlist_delete_revision =
                    library.soundcloud_playlists.delete_revision;
                Some((
                    Provider::SoundCloud,
                    library.soundcloud_playlists.deleted.clone(),
                ))
            } else {
                None
            }
        };
        let (provider, deleted) = deleted?;
        let deleted = deleted?;
        if self.detail.route.as_ref().is_some_and(|route| {
            route.provider == provider && route.kind == ResultType::Playlists && route.id == deleted
        }) {
            self.pending_forward_detail_scroll_reset = None;
            self.detail.close_all();
            cx.notify();
            return Some(provider);
        }
        None
    }

    pub(crate) fn sync_playlist_remove(&mut self, cx: &mut Context<Self>) {
        let playlist_id = {
            let library = self.library.read(cx);
            let revision = library.playlists.remove_revision;
            if revision == self.playlist_remove_revision {
                return;
            }
            self.playlist_remove_revision = revision;
            library.playlists.remove_playlist.clone()
        };
        let Some(playlist_id) = playlist_id else {
            return;
        };
        let Some(route) = self.detail.route.as_ref() else {
            return;
        };
        if !crate::library::playlist_state::matching_playlist_route(
            route.provider,
            "playlistTracks",
            &route.id,
            &playlist_id,
        ) {
            return;
        }
        let Some((generation, route)) = self.detail.reload() else {
            return;
        };
        self.pending_forward_detail_scroll_reset = None;
        let account = self.account.read(cx);
        self.run_detail(
            generation,
            route,
            account.deezer_arl(),
            account.soundcloud_token(),
            cx,
        );
    }
}
