use gpui::{Context, Window};
use gpui_component::WindowExt;

use super::{
    model::{Category, Service},
    playlist_client::AddTracksResult,
    playlist_controller::{
        PlaylistCreateRequest, PlaylistCreateStartError, add_tracks_toast_detail,
        add_tracks_toast_kind, creation_root_refresh_eligible, playlist_create_scope_matches,
    },
    playlist_state::PlaylistState,
    view::LibraryView,
};
use crate::search::Provider;

pub(super) enum SoundCloudCreateResult {
    Success(String),
    RolledBack(String),
    CreatedWithoutCover { playlist_id: String, error: String },
}

impl LibraryView {
    pub(crate) fn playlist_catalog(&self, provider: Provider) -> &PlaylistState {
        match provider {
            Provider::Deezer => &self.playlists,
            Provider::SoundCloud => &self.soundcloud_playlists,
        }
    }

    pub(crate) fn playlist_catalog_mut(&mut self, provider: Provider) -> &mut PlaylistState {
        match provider {
            Provider::Deezer => &mut self.playlists,
            Provider::SoundCloud => &mut self.soundcloud_playlists,
        }
    }

    pub(super) fn open_soundcloud_playlist_create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_soundcloud_playlist_create_with_tracks(Vec::new(), window, cx);
    }

    pub(super) fn open_soundcloud_playlist_create_with_tracks(
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
            Provider::SoundCloud,
            window,
            cx,
        );
    }

    pub(crate) fn ensure_soundcloud_playlist_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_soundcloud_playlist_catalog(false, cx);
    }

    pub(crate) fn retry_soundcloud_playlist_catalog(&mut self, cx: &mut Context<Self>) {
        self.load_soundcloud_playlist_catalog(true, cx);
    }

    fn load_soundcloud_playlist_catalog(&mut self, force: bool, cx: &mut Context<Self>) {
        let scope = self.account.read(cx).library_scope();
        self.soundcloud_playlists.set_account_scope(scope.clone());
        let Some(token) = self.account.read(cx).soundcloud_token() else {
            return;
        };
        let Some(generation) = self.soundcloud_playlists.begin_load(&scope, force) else {
            return;
        };
        let Ok(client) = self.soundcloud_library_client() else {
            self.soundcloud_playlists.complete_load(
                &scope,
                generation,
                Err("SoundCloud playlist client could not be created".into()),
            );
            return;
        };
        let scope_for_task = scope.clone();
        let task = self
            .runtime
            .spawn(async move { client.owned_playlists(token).await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("SoundCloud playlist catalog request failed".into()));
            this.update(cx, |this, cx| {
                if this
                    .soundcloud_playlists
                    .complete_load(&scope_for_task, generation, result)
                {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn start_soundcloud_playlist_create(
        &mut self,
        request: PlaylistCreateRequest,
        cx: &mut Context<Self>,
    ) -> Result<u64, PlaylistCreateStartError> {
        let current_scope = self.account.read(cx).library_scope();
        if current_scope != request.account_scope {
            return Err(PlaylistCreateStartError::AccountChanged);
        }
        if request.provider != Provider::SoundCloud {
            return Err(PlaylistCreateStartError::ClientUnavailable);
        }
        self.soundcloud_playlists
            .set_account_scope(current_scope.clone());
        let Some(token) = self.account.read(cx).soundcloud_mobile_token() else {
            return Err(PlaylistCreateStartError::LoginRequired);
        };
        let Ok(client) = self.soundcloud_library_client() else {
            return Err(PlaylistCreateStartError::ClientUnavailable);
        };
        let Some(generation) = self.soundcloud_playlists.begin_create(&current_scope) else {
            return Err(PlaylistCreateStartError::Pending);
        };
        self.soundcloud_playlists.create_phase(
            &current_scope,
            generation,
            super::playlist_state::CreatePhase::Creating,
        );
        let PlaylistCreateRequest {
            title,
            description,
            private,
            cover,
            track_ids,
            provider: _,
            account_scope: _,
        } = request;
        let library_scope = current_scope.clone();
        let included_tracks = track_ids.len();
        let task = self.runtime.spawn(async move {
            let image_data = if let Some(cover) = cover {
                let image_data = tokio::task::spawn_blocking(move || cover.export_base64())
                    .await
                    .map_err(|_| "The cover image task failed.".to_string())??;
                super::soundcloud_client::validate_artwork_base64(&image_data)?;
                Some(image_data)
            } else {
                None
            };
            let playlist_id = client
                .create_playlist(token.clone(), &title, &description, private, &track_ids)
                .await?;
            let Some(image_data) = image_data else {
                return Ok(SoundCloudCreateResult::Success(playlist_id));
            };
            if let Err(error) = client
                .upload_playlist_artwork(&token, &playlist_id, &image_data)
                .await
            {
                let rollback = client.delete_playlist(token.clone(), &playlist_id).await;
                return Ok(cover_upload_failure(playlist_id, error, rollback));
            }
            Ok(SoundCloudCreateResult::Success(playlist_id))
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("SoundCloud playlist creation failed".into()));
            this.update(cx, |this, cx| {
                this.finish_soundcloud_playlist_create(
                    &library_scope,
                    generation,
                    result,
                    included_tracks,
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

    pub(super) fn finish_soundcloud_playlist_create(
        &mut self,
        scope: &str,
        generation: u64,
        result: Result<SoundCloudCreateResult, String>,
        included_tracks: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !playlist_create_scope_matches(&self.account.read(cx).library_scope(), scope) {
            if let Some(detail) = stale_create_warning(&result) {
                crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Warning,
                    "Playlist created on previous SoundCloud account",
                    Some(detail.into()),
                );
            }
            return false;
        }
        let (id, warning) = match result {
            Err(error) | Ok(SoundCloudCreateResult::RolledBack(error)) => {
                return self
                    .soundcloud_playlists
                    .complete_create(scope, generation, Err(error));
            }
            Ok(SoundCloudCreateResult::Success(id)) => (id, None),
            Ok(SoundCloudCreateResult::CreatedWithoutCover { playlist_id, error }) => {
                (playlist_id, Some(error))
            }
        };
        if !self
            .soundcloud_playlists
            .complete_create(scope, generation, Ok(id))
        {
            return false;
        }
        self.soundcloud_playlists.invalidate_catalog();
        self.state.invalidate_soundcloud(Category::Playlists);
        self.load_soundcloud_playlist_catalog(true, cx);
        if creation_root_refresh_eligible(
            self.selection().0,
            self.selection().1,
            self.state.routes.len(),
            Service::SoundCloud,
        ) {
            self.load_service_force(Service::SoundCloud, Category::Playlists, cx);
        }
        let detail = if included_tracks == 0 {
            Some("Your new playlist is ready.".into())
        } else {
            Some(
                add_tracks_toast_detail(
                    &AddTracksResult::Added {
                        count: included_tracks,
                    },
                    None,
                )
                .into(),
            )
        };
        if let Some(warning) = warning {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Warning,
                "Playlist created without cover",
                Some(warning.into()),
            );
        } else {
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Success,
                "Playlist created",
                detail,
            );
        }
        true
    }

    pub(super) fn finish_soundcloud_playlist_add(
        &mut self,
        scope: &str,
        generation: u64,
        playlist_id: &str,
        result: &Result<AddTracksResult, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let playlist_title = self
            .soundcloud_playlists
            .editable(playlist_id)
            .map(|playlist| playlist.title.clone())
            .filter(|title| !title.trim().is_empty());
        if !self
            .soundcloud_playlists
            .complete_add(scope, generation, playlist_id, result)
        {
            return false;
        }
        if let Ok(addition) = result {
            let added = addition.added_count();
            if added > 0
                && !self
                    .soundcloud_playlists
                    .note_track_added(playlist_id, added)
            {
                self.soundcloud_playlists.invalidate_catalog();
                self.load_soundcloud_playlist_catalog(true, cx);
            }
            self.state.invalidate_soundcloud(Category::Playlists);
            let route = self.state.route().clone();
            if route.source == Provider::SoundCloud
                && route.action == "playlistTracks"
                && route.id == playlist_id
            {
                let (generation, route) = self.state.reload_active_route();
                self.load_nested(generation, route, cx);
            }
            if creation_root_refresh_eligible(
                self.selection().0,
                self.selection().1,
                self.state.routes.len(),
                Service::SoundCloud,
            ) {
                self.load_service_force(Service::SoundCloud, Category::Playlists, cx);
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
}

pub(super) fn playlist_login_required(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Deezer login required",
        Provider::SoundCloud => "SoundCloud login required",
    }
}

pub(super) fn playlist_client_unavailable(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Deezer playlist client could not be created",
        Provider::SoundCloud => "SoundCloud playlist client could not be created",
    }
}

pub(super) fn picker_account_changed(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "The Deezer account changed while this picker was open.",
        Provider::SoundCloud => "The SoundCloud account changed while this picker was open.",
    }
}

pub(super) fn picker_add_account_changed(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "The Deezer account changed while the track was being added.",
        Provider::SoundCloud => "The SoundCloud account changed while the track was being added.",
    }
}

pub(super) fn picker_add_failed(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Deezer playlist add request failed",
        Provider::SoundCloud => "SoundCloud playlist add request failed",
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn picker_title(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Add to Deezer playlist",
        Provider::SoundCloud => "Add to SoundCloud playlist",
    }
}

pub(super) fn create_dialog_title(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Create Deezer playlist",
        Provider::SoundCloud => "Create SoundCloud playlist",
    }
}

pub(super) fn create_account_changed_while(provider: Provider, during: &str) -> String {
    match provider {
        Provider::Deezer => format!("The Deezer account changed while {during}."),
        Provider::SoundCloud => format!("The SoundCloud account changed while {during}."),
    }
}

pub(super) fn covers_required(provider: Provider) -> bool {
    provider == Provider::Deezer
}

pub(super) fn covers_supported(provider: Provider) -> bool {
    matches!(provider, Provider::Deezer | Provider::SoundCloud)
}

fn stale_create_warning(result: &Result<SoundCloudCreateResult, String>) -> Option<String> {
    match result {
        Ok(SoundCloudCreateResult::Success(playlist_id)) => Some(format!(
            "Playlist {playlist_id} was created on the previous SoundCloud account and was not opened here."
        )),
        Ok(SoundCloudCreateResult::CreatedWithoutCover { playlist_id, error }) => Some(format!(
            "Playlist {playlist_id} was created on the previous SoundCloud account, but its cover could not be uploaded. {error} It was not opened here."
        )),
        Ok(SoundCloudCreateResult::RolledBack(_)) | Err(_) => None,
    }
}

fn cover_upload_failure(
    playlist_id: String,
    upload_error: String,
    rollback: Result<bool, String>,
) -> SoundCloudCreateResult {
    match rollback {
        Ok(true) => SoundCloudCreateResult::RolledBack(upload_error),
        Ok(false) => SoundCloudCreateResult::CreatedWithoutCover {
            playlist_id,
            error: format!(
                "{upload_error}. The playlist could not be rolled back and still exists."
            ),
        },
        Err(rollback_error) => SoundCloudCreateResult::CreatedWithoutCover {
            playlist_id,
            error: format!(
                "{upload_error}. The playlist could not be rolled back: {rollback_error}."
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Category, Service};

    #[test]
    fn soundcloud_create_open_is_wired_to_the_shared_dialog() {
        let source = include_str!("soundcloud_playlist.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("pub(super) fn open_soundcloud_playlist_create("));
        assert!(production.contains("open_soundcloud_playlist_create_with_tracks(Vec::new()"));
        assert!(production.contains("PlaylistCreateDialog::open("));
        assert!(production.contains("Provider::SoundCloud"));
        assert!(!production.contains("SoundCloud playlist creation will be wired later."));
    }

    #[test]
    fn soundcloud_catalog_load_uses_owned_playlists() {
        let source = include_str!("soundcloud_playlist.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let catalog = production
            .split("fn load_soundcloud_playlist_catalog(")
            .nth(1)
            .and_then(|rest| {
                rest.split("pub(super) fn start_soundcloud_playlist_create(")
                    .next()
            })
            .expect("SoundCloud catalog loader");
        assert!(catalog.contains("client.owned_playlists(token)"));
        assert!(catalog.contains("soundcloud_token()"));
        assert!(!catalog.contains("soundcloud_mobile_token()"));
        assert!(production.contains("ensure_soundcloud_playlist_catalog"));
        assert!(!production.contains("client.catalog("));
    }

    #[test]
    fn soundcloud_create_posts_initial_tracks_in_one_request() {
        let source = include_str!("soundcloud_playlist.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let create = production
            .split("pub(super) fn start_soundcloud_playlist_create(")
            .nth(1)
            .and_then(|rest| {
                rest.split("pub(super) fn finish_soundcloud_playlist_create(")
                    .next()
            })
            .expect("SoundCloud create flow");
        assert!(
            create.contains(
                "create_playlist(token.clone(), &title, &description, private, &track_ids)",
            )
        );
        assert!(create.contains("soundcloud_mobile_token()"));
        assert!(!create.contains("soundcloud_token()"));
        assert!(!production.contains("soundcloud_session_cookies()"));
        assert!(!production.contains("begin_initial_add"));
        assert!(!production.contains("start_playlist_create_add_retry"));
    }

    #[test]
    fn soundcloud_create_reloads_the_active_soundcloud_playlists_root() {
        assert!(creation_root_refresh_eligible(
            Service::SoundCloud,
            Category::Playlists,
            1,
            Service::SoundCloud,
        ));
        assert!(!creation_root_refresh_eligible(
            Service::Deezer,
            Category::Playlists,
            1,
            Service::SoundCloud,
        ));
        assert!(!creation_root_refresh_eligible(
            Service::SoundCloud,
            Category::Playlists,
            2,
            Service::SoundCloud,
        ));
    }

    #[test]
    fn soundcloud_copy_is_provider_specific() {
        assert_eq!(
            PlaylistCreateStartError::LoginRequired.message_for(Provider::SoundCloud),
            "SoundCloud login required"
        );
        assert_eq!(
            PlaylistCreateStartError::ClientUnavailable.message_for(Provider::SoundCloud),
            "SoundCloud playlist client could not be created"
        );
        assert_eq!(
            PlaylistCreateStartError::AccountChanged.message_for(Provider::SoundCloud),
            "The SoundCloud account changed while this dialog was open."
        );
        assert_eq!(
            playlist_login_required(Provider::SoundCloud),
            "SoundCloud login required"
        );
        assert_eq!(
            picker_title(Provider::SoundCloud),
            "Add to SoundCloud playlist"
        );
        assert_eq!(
            create_dialog_title(Provider::SoundCloud),
            "Create SoundCloud playlist"
        );
        assert!(!covers_required(Provider::SoundCloud));
        assert!(covers_required(Provider::Deezer));
        assert!(covers_supported(Provider::SoundCloud));
        assert!(covers_supported(Provider::Deezer));
    }

    #[test]
    fn soundcloud_cover_upload_failure_is_retryable_only_after_rollback() {
        assert!(matches!(
            cover_upload_failure("9001".into(), "upload failed".into(), Ok(true)),
            SoundCloudCreateResult::RolledBack(error) if error == "upload failed"
        ));
        let created = cover_upload_failure(
            "9002".into(),
            "upload failed".into(),
            Err("delete failed".into()),
        );
        assert!(matches!(
            created,
            SoundCloudCreateResult::CreatedWithoutCover { playlist_id, error }
                if playlist_id == "9002"
                    && error.contains("upload failed")
                    && error.contains("delete failed")
        ));
    }

    #[test]
    fn stale_create_warning_only_reports_resources_that_still_exist() {
        let success = stale_create_warning(&Ok(SoundCloudCreateResult::Success("9003".into())))
            .expect("successful stale creation should warn");
        assert!(success.contains("9003"));
        assert!(success.contains("previous SoundCloud account"));

        let without_cover =
            stale_create_warning(&Ok(SoundCloudCreateResult::CreatedWithoutCover {
                playlist_id: "9004".into(),
                error: "The cover upload failed and rollback failed.".into(),
            }))
            .expect("surviving stale creation should warn");
        assert!(without_cover.contains("9004"));
        assert!(without_cover.contains("cover could not be uploaded"));
        assert!(without_cover.contains("rollback failed"));

        assert!(
            stale_create_warning(&Ok(SoundCloudCreateResult::RolledBack(
                "upload failed".into(),
            )))
            .is_none()
        );
        assert!(stale_create_warning(&Err("create failed".into())).is_none());
    }

    #[test]
    fn soundcloud_create_request_keeps_cover_optional() {
        let source = include_str!("playlist_controller.rs");
        assert!(source.contains("pub cover: Option<CoverDraft>"));
        assert!(source.contains("pub provider: crate::search::Provider"));
        let dialog = include_str!("playlist_create_dialog.rs");
        assert!(dialog.contains("cover: self.cover.clone()"));
        assert!(!dialog.contains("cover: self.cover.clone().unwrap()"));

        let view = include_str!("playlist_create_view.rs");
        assert!(view.contains(".when(self.covers_supported()"));
        let soundcloud = include_str!("soundcloud_playlist.rs");
        let production = &soundcloud[..soundcloud.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("upload_playlist_artwork"));
        assert!(production.contains("delete_playlist(token.clone(), &playlist_id)"));
        assert!(production.contains("let Some(image_data) = image_data else"));
    }
}
