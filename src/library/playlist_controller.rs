use gpui::{Context, Window};
use gpui_component::WindowExt;

use super::{
    model::Category,
    playlist_client::{AddTracksResult, OwnedPlaylist},
    playlist_image::CoverDraft,
    playlist_state::ReorderRoute,
    view::LibraryView,
};
use crate::search::Provider;

fn reorder_page_matches(
    route: &super::model::Route,
    page_generation: u64,
    reorder_route: &ReorderRoute,
) -> bool {
    route.source == reorder_route.provider
        && super::playlist_state::matching_playlist_route(
            route.source,
            &route.action,
            &route.id,
            &reorder_route.playlist_id,
        )
        && reorder_route.page_generation == Some(page_generation)
}

pub(super) struct PlaylistCreateRequest {
    pub account_scope: String,
    pub title: String,
    pub description: String,
    pub private: bool,
    pub cover: Option<CoverDraft>,
    pub track_ids: Vec<String>,
    pub provider: crate::search::Provider,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlaylistCreateStartError {
    AccountChanged,
    LoginRequired,
    ClientUnavailable,
    Pending,
    RetryUnavailable,
}

impl PlaylistCreateStartError {
    pub(super) const fn message(self) -> &'static str {
        self.message_for(crate::search::Provider::Deezer)
    }

    pub(super) const fn message_for(self, provider: crate::search::Provider) -> &'static str {
        match (self, provider) {
            (Self::AccountChanged, crate::search::Provider::Deezer) => {
                "The Deezer account changed while this dialog was open."
            }
            (Self::AccountChanged, crate::search::Provider::SoundCloud) => {
                "The SoundCloud account changed while this dialog was open."
            }
            (Self::LoginRequired, crate::search::Provider::Deezer) => "Deezer login required",
            (Self::LoginRequired, crate::search::Provider::SoundCloud) => {
                "SoundCloud login required"
            }
            (Self::ClientUnavailable, crate::search::Provider::Deezer) => {
                "Deezer playlist client could not be created"
            }
            (Self::ClientUnavailable, crate::search::Provider::SoundCloud) => {
                "SoundCloud playlist client could not be created"
            }
            (Self::Pending, _) => "Another playlist operation is still pending.",
            (Self::RetryUnavailable, _) => "The initial track retry is no longer available.",
        }
    }
}

impl LibraryView {
    pub(super) fn open_playlist_create(
        &mut self,
        track_ids: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let account_scope = self.account.read(cx).library_scope();
        super::playlist_create_dialog::PlaylistCreateDialog::open(
            cx.entity(),
            track_ids,
            account_scope,
            crate::search::Provider::Deezer,
            window,
            cx,
        );
    }

    pub(super) fn finish_playlist_create(
        &mut self,
        scope: &str,
        generation: u64,
        result: Result<(String, Option<Result<AddTracksResult, String>>), String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !playlist_create_scope_matches(&self.account.read(cx).library_scope(), scope) {
            return false;
        }
        let (id, add) = match result {
            Ok(result) => result,
            Err(error) => {
                return self
                    .playlists
                    .complete_create(scope, generation, Err(error));
            }
        };
        if add.is_some()
            && !self.playlists.create_phase(
                scope,
                generation,
                super::playlist_state::CreatePhase::Adding,
            )
        {
            return false;
        }
        let Some(add) = add else {
            if !self.playlists.complete_create(scope, generation, Ok(id)) {
                return false;
            }
            self.playlists.invalidate_catalog();
            self.state.invalidate_deezer(Category::Playlists);
            self.load_playlist_catalog(true, cx);
            if creation_root_refresh_eligible(
                self.selection().0,
                self.selection().1,
                self.state.routes.len(),
                super::model::Service::Deezer,
            ) {
                self.load_service_force(super::model::Service::Deezer, Category::Playlists, cx);
            }
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Success,
                "Playlist created",
                Some("Your new playlist is ready.".into()),
            );
            return true;
        };
        if let Err(error) = &add {
            if !self
                .playlists
                .complete_create(scope, generation, Ok(id.clone()))
            {
                return false;
            }
            if !self.playlists.partial_create(
                scope,
                generation,
                format!(
                    "Playlist {id} was created, but the initial track could not be added: {error}. The playlist was not rolled back."
                ),
            ) {
                return false;
            }
        } else {
            if !self.playlists.complete_create(scope, generation, Ok(id)) {
                return false;
            }
        }
        let add_succeeded = add
            .as_ref()
            .ok()
            .map(|result| add_tracks_toast_detail(result, None));
        self.playlists.invalidate_catalog();
        self.state.invalidate_deezer(Category::Playlists);
        self.load_playlist_catalog(true, cx);
        if creation_root_refresh_eligible(
            self.selection().0,
            self.selection().1,
            self.state.routes.len(),
            super::model::Service::Deezer,
        ) {
            self.load_service_force(super::model::Service::Deezer, Category::Playlists, cx);
        }
        if let Some(detail) = add_succeeded {
            let kind = match &add {
                Ok(result) => add_tracks_toast_kind(result),
                Err(_) => crate::toast::ToastKind::Success,
            };
            crate::toast::push_global(cx, kind, "Playlist created", Some(detail.into()));
        }
        true
    }

    pub(super) fn start_playlist_create(
        &mut self,
        request: PlaylistCreateRequest,
        cx: &mut Context<Self>,
    ) -> Result<u64, PlaylistCreateStartError> {
        let current_scope = self.account.read(cx).library_scope();
        if current_scope != request.account_scope {
            return Err(PlaylistCreateStartError::AccountChanged);
        }
        if request.provider != crate::search::Provider::Deezer {
            return Err(PlaylistCreateStartError::ClientUnavailable);
        }
        self.playlists.set_account_scope(current_scope.clone());
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            return Err(PlaylistCreateStartError::LoginRequired);
        };
        let user_id = self.account.read(cx).deezer_user_id();
        let Ok(client) = self.playlist_client.clone() else {
            return Err(PlaylistCreateStartError::ClientUnavailable);
        };
        let Some(generation) = self.playlists.begin_create(&current_scope) else {
            return Err(PlaylistCreateStartError::Pending);
        };
        self.playlists.create_phase(
            &current_scope,
            generation,
            super::playlist_state::CreatePhase::Creating,
        );
        let PlaylistCreateRequest {
            account_scope: _,
            title,
            description,
            private,
            cover,
            track_ids,
            provider: _,
        } = request;
        let library_scope = current_scope.clone();
        let add_runtime = self.runtime.clone();
        let add_client = client.clone();
        let add_arl = arl.clone();
        let add_user_id = user_id.clone();
        let task = self.runtime.spawn(async move {
            let cover = cover.ok_or_else(|| "Choose and crop a playlist cover.".to_string())?;
            let picture = tokio::task::spawn_blocking(move || cover.export_base64())
                .await
                .map_err(|_| "The cover image task failed.".to_string())??;
            let created = client
                .create(arl, user_id, &title, &description, private, &picture)
                .await?;
            Ok::<_, String>(created.id)
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer playlist creation failed".into()));
            let playlist_id = match result {
                Ok(id) => id,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.finish_playlist_create(&library_scope, generation, Err(error), cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            if track_ids.is_empty() {
                this.update(cx, |this, cx| {
                    this.finish_playlist_create(
                        &library_scope,
                        generation,
                        Ok((playlist_id, None)),
                        cx,
                    );
                    cx.notify();
                })
                .ok();
                return;
            }
            let can_add = this
                .update(cx, |this, cx| {
                    let started =
                        this.playlists
                            .begin_initial_add(&library_scope, generation, &playlist_id);
                    cx.notify();
                    started
                })
                .unwrap_or(false);
            if !can_add {
                return;
            }
            let add_playlist_id = playlist_id.clone();
            let add_task = add_runtime.spawn(async move {
                add_client
                    .add_tracks(add_arl, add_user_id, &add_playlist_id, &track_ids)
                    .await
            });
            let add_result = add_task
                .await
                .unwrap_or_else(|_| Err("Deezer playlist add request failed".into()));
            this.update(cx, |this, cx| {
                this.finish_playlist_create(
                    &library_scope,
                    generation,
                    Ok((playlist_id, Some(add_result))),
                    cx,
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
        Ok(generation)
    }

    pub(super) fn start_playlist_create_add_retry(
        &mut self,
        scope: &str,
        generation: u64,
        track_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), PlaylistCreateStartError> {
        if self.account.read(cx).library_scope() != scope {
            return Err(PlaylistCreateStartError::AccountChanged);
        }
        if track_ids.is_empty() {
            return Err(PlaylistCreateStartError::RetryUnavailable);
        }
        let Some(playlist_id) = self.playlists.create.playlist_id.clone() else {
            return Err(PlaylistCreateStartError::RetryUnavailable);
        };
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            return Err(PlaylistCreateStartError::LoginRequired);
        };
        let user_id = self.account.read(cx).deezer_user_id();
        let Ok(client) = self.playlist_client.clone() else {
            return Err(PlaylistCreateStartError::ClientUnavailable);
        };
        if !self
            .playlists
            .begin_create_add_retry(scope, generation, &playlist_id)
        {
            return Err(PlaylistCreateStartError::RetryUnavailable);
        }
        let account_scope = scope.to_owned();
        let task_playlist_id = playlist_id.clone();
        let task = self.runtime.spawn(async move {
            client
                .add_tracks(arl, user_id, &task_playlist_id, &track_ids)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer playlist add request failed".into()));
            this.update(cx, |this, cx| {
                this.finish_playlist_create_add_retry(
                    &account_scope,
                    generation,
                    &playlist_id,
                    &result,
                    cx,
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn finish_playlist_create_add_retry(
        &mut self,
        scope: &str,
        generation: u64,
        _playlist_id: &str,
        result: &Result<AddTracksResult, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .playlists
            .complete_create_add_retry(scope, generation, result.clone())
        {
            return false;
        }
        if result.is_ok() {
            self.state.invalidate_deezer(Category::Playlists);
            self.playlists.invalidate_catalog();
            self.load_playlist_catalog(true, cx);
            let detail = match result {
                Ok(result) => add_tracks_toast_detail(result, None),
                Err(_) => unreachable!(),
            };
            let kind = match result {
                Ok(result) => add_tracks_toast_kind(result),
                Err(_) => crate::toast::ToastKind::Success,
            };
            crate::toast::push_global(cx, kind, "Track added", Some(detail.into()));
        }
        cx.notify();
        true
    }
    pub(crate) fn playlist_editable(&self, provider: Provider, id: &str) -> bool {
        self.playlist_catalog(provider).is_editable(id)
    }

    pub(crate) fn is_playlist_owned(
        &self,
        provider: Provider,
        id: &str,
        subtitle: &str,
        cx: &gpui::App,
    ) -> bool {
        if self.playlist_editable(provider, id) {
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

    pub(crate) fn move_playlist_track(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let route = self.state.route().clone();
        if !super::playlist_reorder::route_eligible(
            route.source,
            &route.action,
            &route.id,
            &route.id,
        ) || !self.query(cx).trim().is_empty()
            || self.state.routes.len() <= 1
        {
            return;
        }
        if !matches!(
            self.playlist_catalog(route.source).status,
            super::playlist_state::CatalogStatus::Ready
        ) {
            match route.source {
                Provider::Deezer => self.ensure_playlist_catalog(cx),
                Provider::SoundCloud => self.ensure_soundcloud_playlist_catalog(cx),
            }
            return;
        }
        let editable = self.playlist_catalog(route.source).is_editable(&route.id);
        let reorder_pending = self.playlist_catalog(route.source).reorder_pending;
        let Some(page) = self.state.page.as_ref() else {
            return;
        };
        if !super::playlist_reorder::move_eligible(page, editable, false, reorder_pending) {
            return;
        }
        let previous = page
            .tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        let Some(reordered) = super::playlist_reorder::reorder_items(&page.tracks, from, to) else {
            return;
        };
        let submitted = reordered
            .iter()
            .map(|track| track.id.clone())
            .collect::<Vec<_>>();
        if !self.start_playlist_reorder(
            route.source,
            route.id,
            Some(self.state.active_generation()),
            previous,
            submitted,
            cx,
        ) {
            return;
        }
        let Some(page) = self.state.page.as_mut() else {
            return;
        };
        page.tracks = reordered;
        cx.notify();
    }

    pub(crate) fn start_playlist_reorder(
        &mut self,
        provider: crate::search::Provider,
        playlist_id: String,
        page_generation: Option<u64>,
        previous: Vec<String>,
        submitted: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let scope = self.account.read(cx).library_scope();
        let reorder_route = ReorderRoute {
            account_scope: scope.clone(),
            provider,
            action: "playlistTracks".into(),
            playlist_id: playlist_id.clone(),
            page_generation,
        };
        let Some(generation) = self.playlist_catalog_mut(provider).begin_reorder(
            &scope,
            reorder_route.clone(),
            previous,
            &submitted,
        ) else {
            return false;
        };
        let task = match provider {
            crate::search::Provider::Deezer => {
                let Some(arl) = self.account.read(cx).deezer_arl() else {
                    self.finish_playlist_reorder(
                        &scope,
                        generation,
                        &reorder_route,
                        &Err("Deezer account required".into()),
                        cx,
                    );
                    return true;
                };
                let saved_user_id = self.account.read(cx).deezer_user_id();
                let Ok(client) = self.playlist_client.clone() else {
                    self.finish_playlist_reorder(
                        &scope,
                        generation,
                        &reorder_route,
                        &Err("Deezer playlist client could not be created".into()),
                        cx,
                    );
                    return true;
                };
                self.runtime.spawn(async move {
                    client
                        .reorder(arl, saved_user_id, &playlist_id, &submitted)
                        .await
                })
            }
            crate::search::Provider::SoundCloud => {
                let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
                    self.finish_playlist_reorder(
                        &scope,
                        generation,
                        &reorder_route,
                        &Err("SoundCloud account required".into()),
                        cx,
                    );
                    return true;
                };
                let Ok(client) = self.soundcloud_library_client() else {
                    self.finish_playlist_reorder(
                        &scope,
                        generation,
                        &reorder_route,
                        &Err("SoundCloud playlist client could not be created".into()),
                        cx,
                    );
                    return true;
                };
                self.runtime.spawn(async move {
                    client
                        .reorder_playlist(token, &playlist_id, &submitted)
                        .await
                })
            }
        };
        let scope_for_task = scope.clone();
        let task_failure = match reorder_route.provider {
            crate::search::Provider::Deezer => "Deezer playlist reorder failed",
            crate::search::Provider::SoundCloud => "SoundCloud playlist reorder failed",
        };
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(task_failure.into()));
            this.update(cx, |this, cx| {
                this.finish_playlist_reorder(
                    &scope_for_task,
                    generation,
                    &reorder_route,
                    &result,
                    cx,
                );
            })
            .ok();
        })
        .detach();
        cx.notify();
        true
    }

    fn finish_playlist_reorder(
        &mut self,
        scope: &str,
        generation: u64,
        reorder_route: &ReorderRoute,
        result: &Result<bool, String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .playlist_catalog_mut(reorder_route.provider)
            .complete_reorder(scope, generation, reorder_route, result)
        {
            return;
        }
        let route = self.state.route().clone();
        let same_route =
            reorder_page_matches(&route, self.state.active_generation(), reorder_route)
                && self.account.read(cx).library_scope() == reorder_route.account_scope;
        if *result != Ok(true)
            && same_route
            && let Some(previous) = self
                .playlist_catalog_mut(reorder_route.provider)
                .take_reorder_previous(reorder_route)
            && let Some(page) = self.state.page.as_mut()
        {
            let by_id = page
                .tracks
                .iter()
                .cloned()
                .map(|track| (track.id.clone(), track))
                .collect::<std::collections::HashMap<_, _>>();
            page.tracks = previous
                .into_iter()
                .filter_map(|id| by_id.get(&id).cloned())
                .collect();
        }
        if *result != Ok(true) {
            let detail = self
                .playlist_catalog(reorder_route.provider)
                .reorder_error
                .clone()
                .unwrap_or_else(|| "The playlist order could not be saved.".into());
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Playlist order was not saved",
                Some(detail.into()),
            );
        }
        cx.notify();
    }

    pub(crate) fn remove_track_from_playlist(
        &mut self,
        playlist_id: String,
        track_id: String,
        proven: bool,
        cx: &mut Context<Self>,
    ) {
        let scope = self.account.read(cx).library_scope();
        let Some(generation) = self
            .playlists
            .begin_remove(&scope, &playlist_id, &track_id, proven)
        else {
            return;
        };
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            self.playlists.complete_remove(
                &scope,
                generation,
                &Err("Deezer account required".into()),
            );
            cx.notify();
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let Ok(client) = self.playlist_client.clone() else {
            self.playlists.complete_remove(
                &scope,
                generation,
                &Err("Deezer playlist client could not be created".into()),
            );
            cx.notify();
            return;
        };
        let scope_for_task = scope.clone();
        let playlist_for_task = playlist_id.clone();
        let task = self.runtime.spawn(async move {
            client
                .remove_track(arl, saved_user_id, &playlist_for_task, &track_id)
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer playlist track removal failed".into()));
            this.update(cx, |this, cx| {
                if !this
                    .playlists
                    .complete_remove(&scope_for_task, generation, &result)
                {
                    return;
                }
                if result.is_ok() {
                    this.state.invalidate_deezer(Category::Playlists);
                    this.playlists.invalidate_catalog();
                    let route = this.state.route().clone();
                    if super::playlist_state::matching_playlist_route(
                        route.source,
                        &route.action,
                        &route.id,
                        &playlist_id,
                    ) {
                        let (generation, route) = this.state.reload_active_route();
                        this.load_nested(generation, route, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn ensure_playlist_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_playlist_catalog(false, cx);
    }

    pub(crate) fn retry_playlist_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_playlist_catalog(true, cx);
    }

    fn load_playlist_catalog(&mut self, force: bool, cx: &mut Context<Self>) {
        let scope = self.account.read(cx).library_scope();
        if self.playlists.set_account_scope(scope.clone()) {
            self.favorites
                .update(cx, |favorites, _| favorites.reset_account());
        }
        let Some(arl) = self.account.read(cx).deezer_arl() else {
            return;
        };
        let Some(generation) = self.playlists.begin_load(&scope, force) else {
            return;
        };
        let saved_user_id = self.account.read(cx).deezer_user_id();
        let Ok(client) = self.playlist_client.clone() else {
            self.playlists.complete_load(
                &scope,
                generation,
                Err("Deezer playlist client could not be created".into()),
            );
            return;
        };
        let scope_for_task = scope.clone();
        let task = self
            .runtime
            .spawn(async move { client.catalog(arl, saved_user_id).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Deezer playlist catalog request failed".into()));
            this.update(cx, |this, cx| {
                if this
                    .playlists
                    .complete_load(&scope_for_task, generation, result)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_playlist_editor(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let account_scope = self.account.read(cx).library_scope();
        let Some(playlist) = self.playlist_catalog(provider).editable(&id) else {
            match provider {
                Provider::Deezer => self.ensure_playlist_catalog(cx),
                Provider::SoundCloud => self.ensure_soundcloud_playlist_catalog(cx),
            }
            return;
        };
        let library = cx.entity();
        cx.defer_in(window, move |_, window, cx| {
            super::playlist_dialog::PlaylistDialog::open(
                library,
                playlist,
                account_scope,
                provider,
                window,
                cx,
            );
        });
    }

    pub(crate) fn open_playlist_delete(
        &mut self,
        provider: Provider,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        let scope = self.account.read(cx).library_scope();
        let Some(playlist) = self.playlist_catalog(provider).editable(&id) else {
            match provider {
                Provider::Deezer => self.ensure_playlist_catalog(cx),
                Provider::SoundCloud => self.ensure_soundcloud_playlist_catalog(cx),
            }
            return;
        };
        let library = cx.entity();
        cx.defer_in(window, move |_, window, cx| {
            super::playlist_delete_dialog::PlaylistDeleteDialog::open(
                library, playlist, scope, provider, window, cx,
            );
        });
    }

    pub(crate) fn open_add_picker(
        &mut self,
        track_ids: Vec<String>,
        status_scope: String,
        provider: crate::search::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if track_ids.is_empty() || window.has_active_dialog(cx) {
            return;
        }
        match provider {
            crate::search::Provider::Deezer => {
                self.playlists.set_add_scope(status_scope);
                self.ensure_playlist_catalog(cx);
            }
            crate::search::Provider::SoundCloud => {
                self.soundcloud_playlists.set_add_scope(status_scope);
                self.ensure_soundcloud_playlist_catalog(cx);
            }
        }
        let account_scope = self.account.read(cx).library_scope();
        super::playlist_picker::PlaylistPicker::open(
            cx.entity(),
            track_ids,
            account_scope,
            provider,
            window,
            cx,
        );
    }
    pub(super) fn begin_playlist_add(
        &mut self,
        scope: &str,
        playlist_id: &str,
        provider: crate::search::Provider,
    ) -> Option<u64> {
        let catalog = self.playlist_catalog_mut(provider);
        if !catalog.is_editable(playlist_id) {
            return None;
        }
        catalog.begin_add(scope)
    }

    pub(super) fn finish_playlist_add(
        &mut self,
        scope: &str,
        generation: u64,
        playlist_id: &str,
        result: &Result<AddTracksResult, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let playlist_title = self
            .playlists
            .editable(playlist_id)
            .map(|playlist| playlist.title.clone())
            .filter(|title| !title.trim().is_empty());
        if !self
            .playlists
            .complete_add(scope, generation, playlist_id, result)
        {
            return false;
        }
        if let Ok(addition) = result {
            let added = addition.added_count();
            if added > 0 && !self.playlists.note_track_added(playlist_id, added) {
                self.playlists.invalidate_catalog();
                self.load_playlist_catalog(true, cx);
            }
            self.state.invalidate_deezer(Category::Playlists);
            let route = self.state.route().clone();
            if super::playlist_state::matching_playlist_route(
                route.source,
                &route.action,
                &route.id,
                playlist_id,
            ) {
                let (generation, route) = self.state.reload_active_route();
                self.load_nested(generation, route, cx);
            }
            let detail: gpui::SharedString =
                add_tracks_toast_detail(addition, playlist_title.as_deref()).into();
            crate::toast::push_global(
                cx,
                add_tracks_toast_kind(addition),
                "Added to playlist",
                Some(detail),
            );
        }
        cx.notify();
        true
    }

    pub(super) fn begin_playlist_update(&mut self, scope: &str, provider: Provider) -> Option<u64> {
        self.playlist_catalog_mut(provider).begin_update(scope)
    }

    pub(super) fn finish_playlist_update(
        &mut self,
        scope: &str,
        generation: u64,
        provider: Provider,
        result: &Result<OwnedPlaylist, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .playlist_catalog_mut(provider)
            .complete_update(scope, generation, result)
        {
            return false;
        }
        if let Ok(playlist) = result {
            self.state.patch_playlist(provider, playlist);
            let route = self.state.route().clone();
            if super::playlist_state::matching_playlist_route(
                route.source,
                &route.action,
                &route.id,
                &playlist.id,
            ) {
                cx.notify();
            }
        }
        cx.notify();
        true
    }

    pub(super) fn begin_playlist_delete(
        &mut self,
        scope: &str,
        id: &str,
        provider: Provider,
    ) -> Option<u64> {
        self.playlist_catalog_mut(provider).begin_delete(scope, id)
    }

    pub(super) fn finish_playlist_delete(
        &mut self,
        scope: &str,
        generation: u64,
        provider: Provider,
        playlist_id: &str,
        result: &Result<bool, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .playlist_catalog_mut(provider)
            .complete_delete(scope, generation, result)
        {
            return false;
        }
        if result.is_err() {
            cx.notify();
            return true;
        }
        self.favorites.update(cx, |favorites, _| {
            favorites.remove(&crate::library::FavoriteKey::for_provider(
                provider,
                crate::library::FavoriteKind::Playlist,
                playlist_id.to_owned(),
            ));
        });
        match provider {
            Provider::Deezer => self.state.invalidate_deezer(Category::Playlists),
            Provider::SoundCloud => self.state.invalidate_soundcloud(Category::Playlists),
        }
        self.playlist_catalog_mut(provider).invalidate_catalog();
        self.playlist_catalog_mut(provider)
            .note_deleted(playlist_id.to_owned());
        let route = self.state.route().clone();
        if super::playlist_state::matching_playlist_route(
            route.source,
            &route.action,
            &route.id,
            playlist_id,
        ) {
            let (_, parent, _) = self.state.back().expect("playlist detail has a parent");
            if self.state.routes.len() == 1 {
                let service = match provider {
                    Provider::Deezer => super::model::Service::Deezer,
                    Provider::SoundCloud => super::model::Service::SoundCloud,
                };
                self.load_service_force(service, parent.category, cx);
            } else {
                let (generation, route) = self.state.reload_active_route();
                self.load_nested(generation, route, cx);
            }
        } else if creation_root_refresh_eligible(
            self.selection().0,
            self.selection().1,
            self.state.routes.len(),
            match provider {
                Provider::Deezer => super::model::Service::Deezer,
                Provider::SoundCloud => super::model::Service::SoundCloud,
            },
        ) {
            self.load_service_force(
                match provider {
                    Provider::Deezer => super::model::Service::Deezer,
                    Provider::SoundCloud => super::model::Service::SoundCloud,
                },
                Category::Playlists,
                cx,
            );
        }
        cx.notify();
        true
    }
}

pub(super) fn playlist_create_scope_matches(current_scope: &str, completion_scope: &str) -> bool {
    current_scope == completion_scope
}

pub(super) fn add_tracks_toast_kind(result: &AddTracksResult) -> crate::toast::ToastKind {
    match result {
        AddTracksResult::Added { .. } | AddTracksResult::Partial { .. } => {
            crate::toast::ToastKind::Success
        }
        AddTracksResult::AlreadyPresent { .. } => crate::toast::ToastKind::Warning,
    }
}

pub(super) fn add_tracks_toast_detail(
    result: &AddTracksResult,
    playlist_title: Option<&str>,
) -> String {
    match (result, playlist_title) {
        (AddTracksResult::Added { count: 1 }, Some(title)) => {
            format!("Track added to {title}.")
        }
        (AddTracksResult::Added { count }, Some(title)) => {
            format!("{count} tracks added to {title}.")
        }
        _ => result.status_message(),
    }
}

pub(super) fn creation_root_refresh_eligible(
    service: super::model::Service,
    category: Category,
    route_depth: usize,
    created: super::model::Service,
) -> bool {
    service == created && category == Category::Playlists && route_depth == 1
}

#[cfg(test)]
mod create_tests {
    use super::{
        PlaylistCreateStartError, add_tracks_toast_detail, add_tracks_toast_kind,
        creation_root_refresh_eligible, playlist_create_scope_matches, reorder_page_matches,
    };
    use crate::{
        library::{Category, Service, model::Route, playlist_state::ReorderRoute},
        search::Provider,
    };

    #[test]
    fn reorder_rollback_requires_the_original_provider_and_page_generation() {
        let route = Route {
            source: Provider::Deezer,
            category: Category::Playlists,
            action: "playlistTracks".into(),
            id: "42".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        };
        let reorder = ReorderRoute {
            account_scope: "scope".into(),
            provider: Provider::Deezer,
            action: "playlistTracks".into(),
            playlist_id: "42".into(),
            page_generation: Some(7),
        };
        assert!(reorder_page_matches(&route, 7, &reorder));
        assert!(!reorder_page_matches(&route, 8, &reorder));
        let mut search_reorder = reorder.clone();
        search_reorder.page_generation = None;
        assert!(!reorder_page_matches(&route, 7, &search_reorder));
        let mut soundcloud_route = route;
        soundcloud_route.source = Provider::SoundCloud;
        assert!(!reorder_page_matches(&soundcloud_route, 7, &reorder));
    }

    #[test]
    fn creation_reloads_only_the_active_deezer_playlists_root() {
        assert!(creation_root_refresh_eligible(
            Service::Deezer,
            Category::Playlists,
            1,
            Service::Deezer,
        ));
        assert!(!creation_root_refresh_eligible(
            Service::Deezer,
            Category::Playlists,
            2,
            Service::Deezer,
        ));
        assert!(!creation_root_refresh_eligible(
            Service::Deezer,
            Category::Tracks,
            1,
            Service::Deezer,
        ));
        assert!(!creation_root_refresh_eligible(
            Service::SoundCloud,
            Category::Playlists,
            1,
            Service::Deezer,
        ));
    }

    #[test]
    fn playlist_create_completion_rejects_a_late_account_scope() {
        assert!(playlist_create_scope_matches("account-a", "account-a"));
        assert!(!playlist_create_scope_matches("account-b", "account-a"));
    }

    #[test]
    fn pending_copy_is_reserved_for_an_actual_in_scope_operation() {
        assert_eq!(
            PlaylistCreateStartError::AccountChanged.message(),
            "The Deezer account changed while this dialog was open."
        );
        assert_eq!(
            PlaylistCreateStartError::Pending.message(),
            "Another playlist operation is still pending."
        );
        assert_eq!(
            PlaylistCreateStartError::LoginRequired
                .message_for(crate::search::Provider::SoundCloud),
            "SoundCloud login required"
        );
        assert_eq!(
            PlaylistCreateStartError::ClientUnavailable
                .message_for(crate::search::Provider::SoundCloud),
            "SoundCloud playlist client could not be created"
        );
    }

    #[test]
    fn already_present_uses_warning_toast_while_added_stays_success() {
        use super::super::playlist_client::AddTracksResult;
        assert_eq!(
            add_tracks_toast_kind(&AddTracksResult::Added { count: 1 }),
            crate::toast::ToastKind::Success
        );
        assert_eq!(
            add_tracks_toast_kind(&AddTracksResult::AlreadyPresent { count: 4 }),
            crate::toast::ToastKind::Warning
        );
        assert_eq!(
            add_tracks_toast_kind(&AddTracksResult::Partial {
                added: 2,
                duplicated: 1
            }),
            crate::toast::ToastKind::Success
        );
        assert_eq!(
            add_tracks_toast_detail(&AddTracksResult::Added { count: 1 }, Some("Late Night")),
            "Track added to Late Night."
        );
        assert_eq!(
            add_tracks_toast_detail(&AddTracksResult::Added { count: 8 }, Some("Late Night")),
            "8 tracks added to Late Night."
        );
        assert_eq!(
            add_tracks_toast_detail(&AddTracksResult::AlreadyPresent { count: 8 }, None),
            "Those tracks were already in that playlist."
        );
        assert_eq!(
            add_tracks_toast_detail(
                &AddTracksResult::Partial {
                    added: 5,
                    duplicated: 3
                },
                Some("Late Night")
            ),
            "Added 5 tracks; 3 were already in the playlist."
        );
    }
}
