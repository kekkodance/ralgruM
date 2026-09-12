use std::{path::PathBuf, sync::Arc};

use gpui::{
    AnimationExt, Context, Entity, Image, ImageFormat, ImageSource, IntoElement, KeyDownEvent,
    Render, SharedString, Window, div, prelude::*, px, rgb,
};
use gpui_component::{
    WindowExt,
    input::{Input, InputState},
    slider::{SliderState, SliderValue},
};

use super::{
    local_persistence::LocalPlaylistMutationOutcome,
    local_playlist_controller::LocalPlaylistCompletion,
    local_playlist_store::LocalPlaylist,
    playlist_cover_editor::{CROP_PREVIEW_SIZE, PlaylistCoverEditor, cover_editor},
    playlist_create_dialog::{
        body_max_height, crop_panel_stacks, dialog_max_height, dialog_width,
        estimated_dialog_height,
    },
    playlist_form::{self, PlaylistFormTarget},
    playlist_image::{CoverDraft, CoverImageFormat},
    view::LibraryView,
};
use crate::{
    app_button::{primary_button_with_loading, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, SURFACE},
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum LocalPlaylistDialogMode {
    Create,
    Edit { id: String },
}

pub(crate) struct LocalPlaylistDialog {
    library: Entity<LibraryView>,
    mode: LocalPlaylistDialogMode,
    title: Entity<InputState>,
    description: Entity<InputState>,
    zoom_slider: Entity<SliderState>,
    cover: Option<CoverDraft>,
    cover_preview: Option<Arc<Image>>,
    existing_cover_path: Option<PathBuf>,
    initial_tracks: Vec<crate::playback::PlaybackTrack>,
    choosing_cover: bool,
    chooser_generation: u64,
    saving: bool,
    error: Option<SharedString>,
    completed: bool,
    close_motion: DialogCloseMotion,
}

impl LocalPlaylistDialog {
    pub(crate) fn open_create(
        library: Entity<LibraryView>,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        Self::open(
            library,
            LocalPlaylistDialogMode::Create,
            None,
            None,
            Vec::new(),
            window,
            cx,
        );
    }

    pub(crate) fn open_create_with_tracks(
        library: Entity<LibraryView>,
        tracks: Vec<crate::playback::PlaybackTrack>,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        Self::open(
            library,
            LocalPlaylistDialogMode::Create,
            None,
            None,
            tracks,
            window,
            cx,
        );
    }

    pub(crate) fn open_edit(
        library: Entity<LibraryView>,
        playlist: LocalPlaylist,
        existing_cover_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        Self::open(
            library,
            LocalPlaylistDialogMode::Edit {
                id: playlist.id.clone(),
            },
            Some(playlist),
            existing_cover_path,
            Vec::new(),
            window,
            cx,
        );
    }

    fn open(
        library: Entity<LibraryView>,
        mode: LocalPlaylistDialogMode,
        playlist: Option<LocalPlaylist>,
        existing_cover_path: Option<PathBuf>,
        initial_tracks: Vec<crate::playback::PlaybackTrack>,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let title_value = playlist
            .as_ref()
            .map(|playlist| playlist.title.clone())
            .unwrap_or_default();
        let description_value = playlist
            .as_ref()
            .map(|playlist| playlist.description.clone())
            .unwrap_or_default();
        let title = cx.new(|cx| {
            playlist_form::title_input(InputState::new(window, cx), PlaylistFormTarget::Local)
        });
        title.update(cx, |input, cx| {
            input.set_value(title_value, window, cx);
        });
        let description = cx.new(|cx| {
            playlist_form::description_input(InputState::new(window, cx), PlaylistFormTarget::Local)
        });
        description.update(cx, |input, cx| {
            input.set_value(description_value, window, cx);
        });
        let zoom_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(3.)
                .step(0.01)
                .default_value(1.)
        });
        let title_for_focus = title.clone();
        let dialog = cx.new(|cx| {
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
            cx.observe(&library, |_, _, cx| cx.notify()).detach();
            Self {
                library,
                mode,
                title,
                description,
                zoom_slider,
                cover: None,
                cover_preview: None,
                existing_cover_path,
                initial_tracks,
                choosing_cover: false,
                chooser_generation: 0,
                saving: false,
                error: None,
                completed: false,
                close_motion: DialogCloseMotion::default(),
            }
        });
        window.open_dialog(cx, move |dialog_view, dialog_window, cx| {
            let state = dialog.read(cx);
            let viewport = dialog_window.viewport_size();
            let viewport_width = f32::from(viewport.width);
            let viewport_height = f32::from(viewport.height);
            let width = dialog_width(viewport_width);
            let estimated_height = estimated_dialog_height(
                viewport_height,
                state.cover.is_some() || state.existing_cover_path.is_some(),
                crop_panel_stacks(viewport_width, viewport_height),
                !state.initial_tracks.is_empty(),
                state.error.is_some(),
                true,
            );
            let centered_top =
                crate::dialog_layout::centered_margin_top(viewport_height, estimated_height);
            let cancel_handle = dialog.clone();
            dialog_view
                .w(px(width))
                .max_w(px(width))
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
                    let can_close = {
                        let state = cancel_handle.read(cx);
                        !state.completed && !state.saving && !state.choosing_cover
                    };
                    if can_close {
                        cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    }
                    false
                })
                .closing(state.close_motion.closing(), state.close_motion.epoch())
                .child(dialog.clone())
        });
        title_for_focus.update(cx, |title, cx| title.focus(window, cx));
    }

    fn choose_cover(&mut self, cx: &mut Context<Self>) {
        self.start_cover_load(
            || {
                super::playlist_image::choose_cover_with_preview_for(
                    crate::search::Provider::SoundCloud,
                )
            },
            cx,
        );
    }

    fn start_cover_load<F>(&mut self, operation: F, cx: &mut Context<Self>)
    where
        F: FnOnce() -> Result<Option<(CoverDraft, Vec<u8>)>, String> + Send + 'static,
    {
        if self.choosing_cover || self.saving {
            return;
        }
        self.choosing_cover = true;
        self.chooser_generation = self.chooser_generation.wrapping_add(1);
        let generation = self.chooser_generation;
        let task = self.library.read(cx).runtime.spawn_blocking(operation);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The cover image could not be loaded.".into()));
            this.update_in(cx, |this, window, cx| {
                if generation != this.chooser_generation {
                    return;
                }
                this.choosing_cover = false;
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

    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let [path] = paths else {
            self.error = Some("Drop one image file at a time.".into());
            cx.notify();
            return;
        };
        if !super::playlist_image::is_supported_cover_path_for(
            path,
            crate::search::Provider::SoundCloud,
        ) {
            self.error = Some("Choose a supported image file.".into());
            cx.notify();
            return;
        }
        let path = path.clone();
        self.start_cover_load(
            move || super::playlist_image::cover_with_preview_from_path(&path).map(Some),
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
        if self.saving || self.choosing_cover {
            return;
        }
        let title = self.title.read(cx).text().to_string();
        let description = self.description.read(cx).text().to_string();
        if let Err(error) = validate_draft(&title, &description) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let Some(cover) = self.cover.clone() else {
            self.finish_save(title, description, None, cx);
            return;
        };
        self.saving = true;
        let task = self
            .library
            .read(cx)
            .runtime
            .spawn_blocking(move || cover.export_jpeg());
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The cover image could not be prepared.".into()));
            this.update(cx, |this, cx| {
                self::LocalPlaylistDialog::finish_cover_export(this, title, description, result, cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_cover_export(
        &mut self,
        title: String,
        description: String,
        result: Result<Vec<u8>, String>,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        match result {
            Ok(jpeg) => self.finish_save(title, description, Some(jpeg), cx),
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn finish_save(
        &mut self,
        title: String,
        description: String,
        artwork_jpeg: Option<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        let initial_tracks = self.initial_tracks.clone();
        self.saving = true;
        let dialog = cx.entity().clone();
        let result = match &self.mode {
            LocalPlaylistDialogMode::Create => self.library.update(cx, |library, cx| {
                let dialog = dialog.clone();
                let completion: LocalPlaylistCompletion = Box::new(
                    move |result: Result<
                        LocalPlaylistMutationOutcome,
                        super::local_playlist_store::LocalPlaylistError,
                    >,
                          _view: &mut LibraryView,
                          cx: &mut Context<LibraryView>| {
                        dialog.update(cx, |dialog, cx| {
                            dialog.saving = false;
                            match result {
                                Ok(LocalPlaylistMutationOutcome::Created) => {
                                    dialog.error = None;
                                    dialog.completed = true;
                                }
                                Err(error) => dialog.error = Some(error.to_string().into()),
                                _ => {}
                            }
                            cx.notify();
                        });
                    },
                );
                if initial_tracks.is_empty() {
                    library.create_local_playlist_with_artwork(
                        title.clone(),
                        description.clone(),
                        artwork_jpeg.as_deref(),
                        completion,
                        cx,
                    )
                } else {
                    library.create_local_playlist_with_tracks_and_artwork(
                        title.clone(),
                        description.clone(),
                        &initial_tracks,
                        artwork_jpeg.as_deref(),
                        completion,
                        cx,
                    )
                }
            }),
            LocalPlaylistDialogMode::Edit { id } => self.library.update(cx, |library, cx| {
                let dialog = dialog.clone();
                let completion: LocalPlaylistCompletion = Box::new(
                    move |result: Result<
                        LocalPlaylistMutationOutcome,
                        super::local_playlist_store::LocalPlaylistError,
                    >,
                          _view: &mut LibraryView,
                          cx: &mut Context<LibraryView>| {
                        dialog.update(cx, |dialog, cx| {
                            dialog.saving = false;
                            match result {
                                Ok(LocalPlaylistMutationOutcome::Updated) => {
                                    dialog.error = None;
                                    dialog.completed = true;
                                }
                                Err(error) => dialog.error = Some(error.to_string().into()),
                                _ => {}
                            }
                            cx.notify();
                        });
                    },
                );
                library.update_local_playlist_with_artwork(
                    id.clone(),
                    title.clone(),
                    description.clone(),
                    artwork_jpeg.as_deref(),
                    completion,
                    cx,
                )
            }),
        };
        if let Err(error) = result {
            self.saving = false;
            self.error = Some(error.to_string().into());
        }
        cx.notify();
    }
}

impl DialogCloseTarget for LocalPlaylistDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl PlaylistCoverEditor for LocalPlaylistDialog {
    fn provider(&self) -> crate::search::Provider {
        crate::search::Provider::SoundCloud
    }

    fn cover(&self) -> Option<&CoverDraft> {
        self.cover.as_ref()
    }

    fn cover_preview(&self) -> Option<Arc<Image>> {
        self.cover_preview.clone()
    }

    fn existing_cover_source(&self) -> Option<ImageSource> {
        self.existing_cover_path.clone().map(Into::into)
    }

    fn zoom_slider(&self) -> &Entity<SliderState> {
        &self.zoom_slider
    }

    fn choose_cover(&mut self, cx: &mut Context<Self>) {
        LocalPlaylistDialog::choose_cover(self, cx);
    }

    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        LocalPlaylistDialog::drop_cover_paths(self, paths, cx);
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
        LocalPlaylistDialog::pan_cover(
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
        LocalPlaylistDialog::nudge_cover(self, pointer_delta_x, pointer_delta_y, preview_size, cx);
    }

    fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        LocalPlaylistDialog::nudge_zoom(self, delta, window, cx);
    }
}

impl Render for LocalPlaylistDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        playlist_form::enforce_input_limits(
            &self.title,
            &self.description,
            PlaylistFormTarget::Local,
            window,
            cx,
        );
        if self.completed {
            request_dialog_close(self, window, cx);
        }
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let editing = matches!(self.mode, LocalPlaylistDialogMode::Edit { .. });
        let busy = self.saving || self.choosing_cover;
        let viewport = window.viewport_size();
        let viewport_width = f32::from(viewport.width);
        let viewport_height = f32::from(viewport.height);
        let stack_crop = crop_panel_stacks(viewport_width, viewport_height);
        let crop_preview_size = if stack_crop {
            (dialog_width(viewport_width) - 60.)
                .max(0.)
                .min(CROP_PREVIEW_SIZE)
        } else {
            CROP_PREVIEW_SIZE
        };
        let dialog = div()
            .id("local-playlist-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                if !this.saving && !this.choosing_cover {
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(
                                local_icon(LocalIcon::FolderOpen, crate::theme::PRIMARY).size_4(),
                            )
                            .child(if editing {
                                "Edit local playlist"
                            } else {
                                "Create local playlist"
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .p(px(18.))
                    .max_h(px(body_max_height(viewport_height)))
                    .id("local-playlist-dialog-body")
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
                    .child(cover_editor(self, busy, stack_crop, crop_preview_size, cx))
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
                        "cancel-local-playlist",
                        None,
                        "Cancel",
                        busy,
                        cx.listener(|this, _, window, cx| request_dialog_close(this, window, cx)),
                    ))
                    .child(primary_button_with_loading(
                        "save-local-playlist",
                        Some(LocalIcon::Check),
                        if editing {
                            "Save changes"
                        } else {
                            "Create playlist"
                        },
                        busy,
                        busy,
                        cx.listener(|this, _, _, cx| this.save(cx)),
                    )),
            );
        if closing {
            dialog
                .with_animation(
                    ("local-playlist-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

fn validate_draft(title: &str, description: &str) -> Result<(), &'static str> {
    playlist_form::validate_draft(PlaylistFormTarget::Local, title, description)
}

#[cfg(test)]
mod tests {
    use super::validate_draft;

    #[test]
    fn local_playlist_draft_validation_matches_store_limits() {
        assert_eq!(validate_draft("  ", ""), Err("Enter a playlist title."));
        assert!(validate_draft(&"a".repeat(100), &"b".repeat(4000)).is_ok());
        assert!(validate_draft(&"a".repeat(101), "").is_err());
        assert!(validate_draft("Name", &"b".repeat(4001)).is_err());
        assert!(validate_draft("Name", "One\nTwo").is_ok());
        assert!(validate_draft("Name\0", "").is_err());
    }

    #[test]
    fn local_form_has_shared_create_and_edit_paths() {
        let source = include_str!("local_playlist_dialog.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(source.contains("open_create"));
        assert!(source.contains("open_edit"));
        assert!(source.contains("create_local_playlist"));
        assert!(source.contains("create_local_playlist_with_tracks"));
        assert!(source.contains("open_create_with_tracks"));
        assert!(source.contains("update_local_playlist"));
        assert!(!source.contains("private_playlist_switch"));
        assert!(source.contains("cover_editor("));
        assert!(source.contains("impl PlaylistCoverEditor for LocalPlaylistDialog"));
        assert!(source.contains("export_jpeg"));
        assert!(source.contains("existing_cover_source"));
        assert!(source.contains("estimated_dialog_height"));
        assert!(source.contains("crop_panel_stacks"));
        assert!(source.contains("body_max_height"));
    }

    #[test]
    fn edit_open_passes_artwork_without_reading_library_during_construction() {
        let dialog = include_str!("local_playlist_dialog.rs");
        let production = &dialog[..dialog.find("#[cfg(test)]").unwrap()];
        let open_start = production
            .find("    fn open(")
            .expect("dialog construction should exist");
        let open_end = open_start
            + production[open_start..]
                .find("    fn choose_cover")
                .expect("dialog construction should end before cover actions");
        let construction = &production[open_start..open_end];
        assert!(construction.contains("existing_cover_path: Option<PathBuf>"));
        assert!(!construction.contains("library.read(cx)"));

        let controller = include_str!("local_playlist_controller.rs");
        assert!(controller.contains("store.artwork_path(&playlist.id, &playlist.artwork)"));
        assert!(
            controller.contains(
                "LocalPlaylistDialog::open_edit(cx.entity(), playlist, existing_cover_path"
            )
        );
    }

    #[test]
    fn multiline_description_does_not_use_single_line_input_validator() {
        let source = include_str!("local_playlist_dialog.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(source.contains("playlist_form::description_input"));
        let form = include_str!("playlist_form.rs");
        let description_start = form
            .find("pub(super) fn description_input")
            .expect("shared description input builder should exist");
        let description_end = form[description_start..]
            .find("pub(super) fn title_field")
            .map(|offset| description_start + offset)
            .expect("shared description input builder should end");
        let description_builder = &form[description_start..description_end];
        assert!(description_builder.contains(".multi_line(true)"));
        assert!(!description_builder.contains(".validate("));
    }
}
