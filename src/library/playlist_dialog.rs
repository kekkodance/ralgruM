use std::{path::PathBuf, sync::Arc};

use gpui::{
    AnimationExt, Context, Entity, Image, ImageFormat, IntoElement, KeyDownEvent, Render,
    SharedString, Window, div, prelude::*, px, rgb,
};
use gpui_component::{
    WindowExt,
    input::{Input, InputState},
    slider::{SliderState, SliderValue},
};
use tokio::task::JoinHandle;

use super::{
    LibraryView,
    playlist_client::OwnedPlaylist,
    playlist_cover_editor::PlaylistCoverEditor,
    playlist_create_dialog::{
        body_max_height, crop_panel_stacks, dialog_max_height, dialog_width,
        estimated_dialog_height,
    },
    playlist_form::{self, PlaylistFormTarget},
    playlist_image::{CoverDraft, CoverImageFormat},
};
use crate::search::Provider;
use crate::{
    app_button::{primary_button_with_loading, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, SURFACE},
};

use super::playlist_form::private_playlist_switch;

pub(crate) struct PlaylistDialog {
    library: Entity<LibraryView>,
    playlist: OwnedPlaylist,
    provider: Provider,
    title: Entity<InputState>,
    description: Entity<InputState>,
    zoom_slider: Entity<SliderState>,
    private: bool,
    cover: Option<CoverDraft>,
    cover_preview: Option<Arc<Image>>,
    choosing_cover: bool,
    chooser_generation: u64,
    busy: bool,
    error: Option<SharedString>,
    account_scope: String,
    completed: bool,
    close_motion: DialogCloseMotion,
}

impl PlaylistDialog {
    pub(crate) fn open(
        library: Entity<LibraryView>,
        playlist: OwnedPlaylist,
        account_scope: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let target = PlaylistFormTarget::Provider(provider);
        let title = cx.new(|cx| {
            let mut input = playlist_form::title_input(InputState::new(window, cx), target);
            input.set_value(playlist.title.clone(), window, cx);
            input
        });
        let description = cx.new(|cx| {
            let mut input = playlist_form::description_input(InputState::new(window, cx), target);
            input.set_value(playlist.description.clone(), window, cx);
            input
        });
        let zoom_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(3.)
                .step(0.01)
                .default_value(1.)
        });
        let title_for_focus = title.clone();
        let private = playlist.is_private;
        let dialog = cx.new(|cx| {
            cx.observe(&library, |_, _, cx| cx.notify()).detach();
            let zoom_for_changes = zoom_slider.clone();
            cx.observe(&zoom_for_changes, |this: &mut Self, slider, cx| {
                let SliderValue::Single(zoom) = slider.read(cx).value() else {
                    return;
                };
                if let Some(cover) = this.cover.as_mut() {
                    cover.set_zoom((zoom * 100.).round() as u16);
                    cx.notify();
                }
            })
            .detach();
            Self {
                library: library.clone(),
                playlist,
                provider,
                title,
                description,
                zoom_slider: zoom_slider.clone(),
                private,
                cover: None,
                cover_preview: None,
                choosing_cover: false,
                chooser_generation: 0,
                busy: false,
                error: None,
                account_scope,
                completed: false,
                close_motion: DialogCloseMotion::default(),
            }
        });
        window.open_dialog(cx, move |dialog_view, dialog_window, cx| {
            let viewport_width = f32::from(dialog_window.viewport_size().width);
            let viewport_height = f32::from(dialog_window.viewport_size().height);
            let dialog_width = dialog_width(viewport_width);
            let state = dialog.read(cx);
            let has_cover = state.cover.is_some() || !state.playlist.artwork.trim().is_empty();
            let dialog_height = estimated_dialog_height(
                viewport_height,
                has_cover,
                crop_panel_stacks(viewport_width, viewport_height),
                false,
                state.error.is_some(),
                true,
            );
            let centered_top =
                crate::dialog_layout::centered_margin_top(viewport_height, dialog_height);
            let cancel_handle = dialog.clone();
            dialog_view
                .w(px(dialog_width))
                .max_w(px(dialog_width))
                .max_h(px(dialog_max_height(viewport_height)))
                .margin_top(centered_top)
                .p_0()
                .gap_0()
                .bg(gpui::rgba(0x00000000))
                .border_0()
                .rounded(px(8.))
                .close_button(false)
                .overlay(true)
                .overlay_closable(true)
                .keyboard(false)
                .on_cancel(move |_, window, cx| {
                    let should_close = {
                        let state = cancel_handle.read(cx);
                        can_close_dialog(state.busy, state.choosing_cover)
                    };
                    if should_close {
                        cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    }
                    false
                })
                .closing(state.close_motion.closing(), state.close_motion.epoch())
                .child(dialog.clone())
        });
        title_for_focus.update(cx, |title, cx| title.focus(window, cx));
    }

    fn start_cover_load<F>(
        &mut self,
        operation: F,
        task_error: &'static str,
        cx: &mut Context<Self>,
    ) where
        F: FnOnce() -> Result<Option<(CoverDraft, Vec<u8>)>, String> + Send + 'static,
    {
        let current_scope = self.library.read(cx).account.read(cx).library_scope();
        if self.busy || self.choosing_cover || current_scope != self.account_scope {
            return;
        }
        self.choosing_cover = true;
        self.chooser_generation = self.chooser_generation.wrapping_add(1);
        let chooser_generation = self.chooser_generation;
        let account_scope = self.account_scope.clone();
        let library = self.library.clone();
        let task = self.library.read(cx).runtime.spawn_blocking(operation);
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| Err(task_error.into()));
            this.update_in(cx, |this, window, cx| {
                if chooser_generation != this.chooser_generation {
                    return;
                }
                this.choosing_cover = false;
                if library.read(cx).account.read(cx).library_scope() != account_scope {
                    this.error = Some(
                        format!(
                            "The {} account changed while the cover image was loading.",
                            this.provider.label()
                        )
                        .into(),
                    );
                    cx.notify();
                    return;
                }
                match result {
                    Ok(Some((cover, preview))) => {
                        let format = match cover.format {
                            CoverImageFormat::Gif => ImageFormat::Gif,
                            CoverImageFormat::Jpeg => ImageFormat::Jpeg,
                            CoverImageFormat::Png => ImageFormat::Png,
                            CoverImageFormat::Webp => ImageFormat::Webp,
                        };
                        this.zoom_slider
                            .update(cx, |slider, cx| slider.set_value(1., window, cx));
                        this.cover_preview = Some(Arc::new(Image::from_bytes(format, preview)));
                        this.cover = Some(cover);
                        this.error = None;
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn choose_cover(&mut self, cx: &mut Context<Self>) {
        let provider = self.provider;
        self.start_cover_load(
            move || super::playlist_image::choose_cover_with_preview_for(provider),
            "The cover image chooser failed.",
            cx,
        );
    }

    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let [path] = paths else {
            self.error = Some("Drop one image file at a time.".into());
            cx.notify();
            return;
        };
        if !super::playlist_image::is_supported_cover_path_for(path, self.provider) {
            self.error = Some(
                format!(
                    "Drop a {} image file.",
                    super::playlist_image::cover_extensions_label(self.provider)
                )
                .into(),
            );
            cx.notify();
            return;
        }
        let path = path.clone();
        self.start_cover_load(
            move || super::playlist_image::cover_with_preview_from_path(&path).map(Some),
            "The dropped cover image could not be loaded.",
            cx,
        );
    }

    fn pan_cover(
        &mut self,
        start_pan_x: i32,
        start_pan_y: i32,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    ) {
        if let Some(cover) = self.cover.as_mut() {
            cover.pan_from_drag(
                start_pan_x,
                start_pan_y,
                pointer_delta_x,
                pointer_delta_y,
                preview_size,
            );
            cx.notify();
        }
    }

    fn nudge_cover(
        &mut self,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(cover) = self.cover.as_ref() else {
            return;
        };
        self.pan_cover(
            cover.pan_x,
            cover.pan_y,
            pointer_delta_x,
            pointer_delta_y,
            preview_size,
            cx,
        );
    }

    fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        let SliderValue::Single(current) = self.zoom_slider.read(cx).value() else {
            return;
        };
        let next = (current + delta).clamp(1., 3.);
        let Some(cover) = self.cover.as_mut() else {
            return;
        };
        cover.set_zoom((next * 100.).round() as u16);
        self.zoom_slider
            .update(cx, |slider, cx| slider.set_value(next, window, cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.choosing_cover {
            return;
        }
        let title = self.title.read(cx).text().to_string();
        let description = self.description.read(cx).text().to_string();
        if let Err(error) = validate_draft(self.provider, &title, &description) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let picture_base64 = match self.cover.as_ref().map(CoverDraft::export_base64) {
            Some(Ok(picture)) => Some(picture),
            Some(Err(error)) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
            None => None,
        };
        let provider = self.provider;
        let scope = self.library.read(cx).account.read(cx).library_scope();
        if scope != self.account_scope {
            self.fail_preflight(
                playlist_preflight_error(provider, false, true, true).unwrap(),
                cx,
            );
            return;
        }
        let Some(generation) = self.library.update(cx, |library, _| {
            library.begin_playlist_update(&self.account_scope, provider)
        }) else {
            self.fail_preflight(
                playlist_preflight_error(provider, false, true, true).unwrap(),
                cx,
            );
            return;
        };
        let id = self.playlist.id.clone();
        let playlist = self.playlist.clone();
        let is_private = self.private;
        let account_scope = self.account_scope.clone();
        self.busy = true;
        self.error = None;
        let library = self.library.clone();
        let runtime = self.library.read(cx).runtime.clone();
        let task = match provider {
            Provider::Deezer => {
                let Some(arl) = self.library.read(cx).account.read(cx).deezer_arl() else {
                    self.busy = false;
                    self.fail_preflight(
                        playlist_preflight_error(provider, true, false, true).unwrap(),
                        cx,
                    );
                    return;
                };
                let user_id = self.library.read(cx).account.read(cx).deezer_user_id();
                let client = match self.library.read(cx).playlist_client.clone() {
                    Ok(client) => client,
                    Err(_) => {
                        self.busy = false;
                        self.fail_preflight(
                            playlist_preflight_error(provider, true, true, false).unwrap(),
                            cx,
                        );
                        return;
                    }
                };
                runtime.spawn(async move {
                    client
                        .update(
                            arl,
                            user_id,
                            &id,
                            &title,
                            &description,
                            is_private,
                            picture_base64.as_deref(),
                        )
                        .await
                })
            }
            Provider::SoundCloud => {
                let Some(token) = self
                    .library
                    .read(cx)
                    .account
                    .read(cx)
                    .soundcloud_mobile_token()
                else {
                    self.busy = false;
                    self.fail_preflight(
                        playlist_preflight_error(provider, true, false, true).unwrap(),
                        cx,
                    );
                    return;
                };
                let client = match self.library.read(cx).soundcloud_library_client() {
                    Ok(client) => client,
                    Err(_) => {
                        self.busy = false;
                        self.fail_preflight(
                            playlist_preflight_error(provider, true, true, false).unwrap(),
                            cx,
                        );
                        return;
                    }
                };
                runtime.spawn(async move {
                    client
                        .update_playlist(
                            token,
                            playlist,
                            &title,
                            &description,
                            is_private,
                            picture_base64.as_deref(),
                        )
                        .await
                })
            }
        };
        cx.spawn(async move |this, cx| {
            let result = task_result(task, provider).await;
            this.update(cx, |this, cx| {
                this.busy = false;
                let accepted = library.update(cx, |library, cx| {
                    library.finish_playlist_update(
                        &account_scope,
                        generation,
                        provider,
                        &result,
                        cx,
                    )
                });
                if !accepted {
                    this.error = Some(
                        format!(
                            "The {} account changed while the playlist update was in progress.",
                            provider.label()
                        )
                        .into(),
                    );
                } else if let Err(error) = &result {
                    this.error = Some(error.clone().into());
                } else {
                    this.completed = true;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn fail_preflight(&mut self, error: &'static str, cx: &mut Context<Self>) {
        self.error = Some(error.into());
        cx.notify();
    }
}

impl DialogCloseTarget for PlaylistDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

fn playlist_preflight_error(
    provider: Provider,
    scope_matches: bool,
    has_credential: bool,
    has_client: bool,
) -> Option<&'static str> {
    if !scope_matches {
        Some(match provider {
            Provider::Deezer => "The Deezer account changed while this playlist editor was open.",
            Provider::SoundCloud => {
                "The SoundCloud account changed while this playlist editor was open."
            }
        })
    } else if !has_credential {
        Some(match provider {
            Provider::Deezer => "Deezer login required",
            Provider::SoundCloud => "SoundCloud login required",
        })
    } else if !has_client {
        Some(match provider {
            Provider::Deezer => "Deezer playlist client could not be created",
            Provider::SoundCloud => "SoundCloud playlist client could not be created",
        })
    } else {
        None
    }
}

fn can_close_dialog(busy: bool, choosing_cover: bool) -> bool {
    !busy && !choosing_cover
}

async fn task_result(
    task: JoinHandle<Result<OwnedPlaylist, String>>,
    provider: Provider,
) -> Result<OwnedPlaylist, String> {
    task.await.unwrap_or_else(|_| {
        Err(format!(
            "{} playlist update request failed",
            provider.label()
        ))
    })
}

impl Render for PlaylistDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        playlist_form::enforce_input_limits(
            &self.title,
            &self.description,
            PlaylistFormTarget::Provider(self.provider),
            window,
            cx,
        );
        let account_changed =
            self.library.read(cx).account.read(cx).library_scope() != self.account_scope;
        if self.completed
            || (account_changed
                && can_close_dialog(self.busy, self.choosing_cover)
                && self.error.is_none())
        {
            request_dialog_close(self, window, cx);
        }
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let busy = self.busy;
        let controls_busy = busy || self.choosing_cover;
        let viewport_size = window.viewport_size();
        let viewport_width = f32::from(viewport_size.width);
        let viewport_height = f32::from(viewport_size.height);
        let body_max_height = body_max_height(viewport_height);
        let stack_crop = crop_panel_stacks(viewport_width, viewport_height);
        let crop_preview_size = if stack_crop {
            (dialog_width(viewport_width) - 60.)
                .clamp(0., super::playlist_cover_editor::CROP_PREVIEW_SIZE)
        } else {
            super::playlist_cover_editor::CROP_PREVIEW_SIZE
        };
        let dialog = div()
            .id("playlist-edit-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                if can_close_dialog(this.busy, this.choosing_cover) {
                    request_dialog_close(this, window, cx);
                }
                cx.stop_propagation();
            }))
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(FOREGROUND))
            .bg(rgb(BACKGROUND))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(16.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(
                                local_icon(
                                    if self.provider == Provider::Deezer {
                                        LocalIcon::Deezer
                                    } else {
                                        LocalIcon::SoundCloud
                                    },
                                    if self.provider == Provider::Deezer {
                                        0xa238ff
                                    } else {
                                        0xff5500
                                    },
                                )
                                .size_4(),
                            )
                            .child(format!("Edit {} playlist", self.provider.label())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .p(px(18.))
                    .max_h(px(body_max_height))
                    .id("playlist-edit-body")
                    .overflow_y_scroll()
                    .bg(rgb(BACKGROUND))
                    .child(playlist_form::title_field(text_field_context_menu(
                        Input::new(&self.title).bg(rgb(SURFACE)),
                        self.title.clone(),
                    )))
                    .child(playlist_form::description_field(text_field_context_menu(
                        Input::new(&self.description).h(px(92.)).bg(rgb(SURFACE)),
                        self.description.clone(),
                    )))
                    .child(private_playlist_switch(
                        "edit-private-row",
                        "edit-private",
                        self.private,
                        controls_busy,
                        cx,
                        |this, checked, cx| {
                            this.private = checked;
                            cx.notify();
                        },
                    ))
                    .child(super::playlist_cover_editor::cover_editor(
                        self,
                        controls_busy,
                        stack_crop,
                        crop_preview_size,
                        cx,
                    ))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_color(rgb(DANGER))
                                .text_size(px(12.))
                                .child(error),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(secondary_dialog_button_with_disabled(
                        "cancel-playlist-edit",
                        None,
                        "Cancel",
                        controls_busy,
                        cx.listener(|this, _, window, cx| {
                            if can_close_dialog(this.busy, this.choosing_cover) {
                                request_dialog_close(this, window, cx);
                            }
                        }),
                    ))
                    .child(primary_button_with_loading(
                        "save-playlist-edit",
                        Some(LocalIcon::Check),
                        if busy { "Saving..." } else { "Save changes" },
                        controls_busy,
                        busy,
                        cx.listener(|this, _, _, cx| this.save(cx)),
                    )),
            );

        if closing {
            dialog
                .with_animation(
                    ("playlist-edit-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

impl PlaylistCoverEditor for PlaylistDialog {
    fn provider(&self) -> Provider {
        self.provider
    }

    fn cover(&self) -> Option<&CoverDraft> {
        self.cover.as_ref()
    }

    fn cover_preview(&self) -> Option<Arc<Image>> {
        self.cover_preview.clone()
    }

    fn existing_cover_url(&self) -> Option<SharedString> {
        let artwork = self.playlist.artwork.trim();
        (!artwork.is_empty()).then(|| artwork.to_owned().into())
    }

    fn zoom_slider(&self) -> &Entity<SliderState> {
        &self.zoom_slider
    }

    fn choose_cover(&mut self, cx: &mut Context<Self>) {
        PlaylistDialog::choose_cover(self, cx);
    }

    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        PlaylistDialog::drop_cover_paths(self, paths, cx);
    }

    fn pan_cover(
        &mut self,
        start_pan_x: i32,
        start_pan_y: i32,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    ) {
        PlaylistDialog::pan_cover(
            self,
            start_pan_x,
            start_pan_y,
            pointer_delta_x,
            pointer_delta_y,
            preview_size,
            cx,
        );
    }

    fn nudge_cover(
        &mut self,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    ) {
        PlaylistDialog::nudge_cover(self, pointer_delta_x, pointer_delta_y, preview_size, cx);
    }

    fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        PlaylistDialog::nudge_zoom(self, delta, window, cx);
    }
}

fn validate_draft(provider: Provider, title: &str, description: &str) -> Result<(), &'static str> {
    playlist_form::validate_draft(PlaylistFormTarget::Provider(provider), title, description)
}

#[cfg(test)]
mod tests {
    use super::{can_close_dialog, playlist_preflight_error, validate_draft};
    use crate::search::Provider;

    #[test]
    fn dialog_validation_matches_original_limits_and_copy() {
        assert_eq!(
            validate_draft(Provider::Deezer, "  ", ""),
            Err("Enter a playlist title.")
        );
        assert!(validate_draft(Provider::Deezer, &"a".repeat(50), &"b".repeat(200)).is_ok());
        assert!(validate_draft(Provider::Deezer, &"a".repeat(51), "").is_err());
        assert!(validate_draft(Provider::Deezer, "Title", &"b".repeat(201)).is_err());
        assert!(validate_draft(Provider::Deezer, "Title", "First line\nSecond line").is_ok());
    }

    #[test]
    fn soundcloud_edit_uses_the_official_text_boundaries() {
        assert!(validate_draft(Provider::SoundCloud, &"a".repeat(100), &"b".repeat(4000),).is_ok());
        assert!(validate_draft(Provider::SoundCloud, &"a".repeat(101), "").is_err());
        assert!(validate_draft(Provider::SoundCloud, "Title", &"b".repeat(4001)).is_err());
        assert!(validate_draft(Provider::SoundCloud, "é🎵", "First line\nSecond line").is_ok());
    }

    #[test]
    fn pending_update_or_cover_selection_suppresses_dialog_close() {
        assert!(can_close_dialog(false, false));
        assert!(!can_close_dialog(true, false));
        assert!(!can_close_dialog(false, true));
    }

    #[test]
    fn playlist_preflight_fails_closed_with_visible_errors() {
        assert_eq!(
            playlist_preflight_error(Provider::Deezer, false, true, true),
            Some("The Deezer account changed while this playlist editor was open.")
        );
        assert_eq!(
            playlist_preflight_error(Provider::Deezer, true, false, true),
            Some("Deezer login required")
        );
        assert_eq!(
            playlist_preflight_error(Provider::Deezer, true, true, false),
            Some("Deezer playlist client could not be created")
        );
        assert_eq!(
            playlist_preflight_error(Provider::Deezer, true, true, true),
            None
        );
        assert_eq!(
            playlist_preflight_error(Provider::SoundCloud, true, false, true),
            Some("SoundCloud login required")
        );
    }
}
