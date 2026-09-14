use std::{path::PathBuf, sync::Arc};

use gpui::{AppContext, Context, Entity, Image, ImageFormat, SharedString, Window, prelude::*, px};
use gpui_component::{
    WindowExt,
    input::{InputEvent, InputState},
    slider::{SliderState, SliderValue},
};

use super::{
    model::{Card, Category},
    playlist_controller::PlaylistCreateRequest,
    playlist_cover_editor::PlaylistCoverEditor,
    playlist_form::{self, PlaylistFormTarget},
    playlist_image::{CoverDraft, CoverImageFormat},
    playlist_limits,
    playlist_state::CreatePhase,
    soundcloud_playlist::{
        covers_required as provider_covers_required, covers_supported as provider_covers_supported,
        create_account_changed_while,
    },
    view::LibraryView,
};
use crate::dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close};
use crate::search::Provider;

pub(crate) struct PlaylistCreateDialog {
    pub(super) library: Entity<LibraryView>,
    pub(super) title: Entity<InputState>,
    pub(super) description: Entity<InputState>,
    pub(super) zoom_slider: Entity<SliderState>,
    pub(super) private: bool,
    pub(super) cover: Option<CoverDraft>,
    pub(super) cover_preview: Option<Arc<Image>>,
    pub(super) track_ids: Vec<String>,
    pub(super) provider: Provider,
    pub(super) account_scope: String,
    pub(super) generation: Option<u64>,
    pub(super) retry_generation: Option<u64>,
    pub(super) choosing_cover: bool,
    pub(super) chooser_generation: u64,
    pub(super) error: Option<SharedString>,
    pub(super) completed: bool,
    pub(super) close_motion: DialogCloseMotion,
}

#[allow(dead_code)]
pub(super) const TITLE_MAX_CHARS: usize = playlist_limits::DEEZER_TITLE_MAX_CHARS;
#[allow(dead_code)]
pub(super) const DESCRIPTION_MAX_CHARS: usize = playlist_limits::DEEZER_DESCRIPTION_MAX_CHARS;

impl PlaylistCreateDialog {
    pub(crate) fn open(
        library: Entity<LibraryView>,
        track_ids: Vec<String>,
        account_scope: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let target = PlaylistFormTarget::Provider(provider);
        let title = cx.new(|cx| playlist_form::title_input(InputState::new(window, cx), target));
        let description =
            cx.new(|cx| playlist_form::description_input(InputState::new(window, cx), target));
        let zoom_slider = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(3.)
                .step(0.01)
                .default_value(1.)
        });
        let title_for_focus = title.clone();
        let entity = cx.new(|cx| {
            let title_for_changes = title.clone();
            cx.subscribe(&title_for_changes, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            let description_for_changes = description.clone();
            cx.subscribe(&description_for_changes, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
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
            cx.observe(&library, |this, library, cx| {
                let phase = this.generation.and_then(|generation| {
                    library
                        .read(cx)
                        .playlist_catalog(this.provider)
                        .create_phase_for(&this.account_scope, generation)
                });
                this.sync_create_phase(phase, cx);
                cx.notify();
            })
            .detach();
            Self {
                library: library.clone(),
                title,
                description,
                zoom_slider: zoom_slider.clone(),
                private: true,
                cover: None,
                cover_preview: None,
                track_ids,
                provider,
                account_scope,
                generation: None,
                retry_generation: None,
                choosing_cover: false,
                chooser_generation: 0,
                error: None,
                completed: false,
                close_motion: DialogCloseMotion::default(),
            }
        });
        window.open_dialog(cx, move |dialog, dialog_window, cx| {
            let viewport_width = f32::from(dialog_window.viewport_size().width);
            let viewport_height = f32::from(dialog_window.viewport_size().height);
            let dialog_width = dialog_width(viewport_width);
            let dialog_state = entity.read(cx);
            let dialog_height = estimated_dialog_height(
                viewport_height,
                dialog_state.cover.is_some(),
                crop_panel_stacks(viewport_width, viewport_height),
                !dialog_state.track_ids.is_empty(),
                dialog_state.error.is_some(),
                provider_covers_supported(dialog_state.provider),
            );
            let centered_top =
                crate::dialog_layout::centered_margin_top(viewport_height, dialog_height);
            let cancel_entity = entity.clone();
            dialog
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
                .keyboard(dialog_keyboard_shortcuts_enabled())
                .on_cancel(move |_, window, cx| {
                    let should_close = {
                        let dialog = cancel_entity.read(cx);
                        escape_can_close_dialog(
                            dialog.generation,
                            dialog.choosing_cover,
                            dialog.completed,
                        )
                    };
                    if should_close {
                        cancel_entity.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    }
                    false
                })
                .closing(
                    dialog_state.close_motion.closing(),
                    dialog_state.close_motion.epoch(),
                )
                .child(entity.clone())
        });
        title_for_focus.update(cx, |title, cx| title.focus(window, cx));
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        let provider = self.provider;
        self.start_cover_load(
            move || super::playlist_image::choose_cover_with_preview_for(provider),
            "The cover image chooser failed.",
            cx,
        );
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
        if !can_choose_cover(
            self.generation.is_some(),
            self.choosing_cover,
            current_scope != self.account_scope,
        ) {
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
                let scope_matches =
                    library.read(cx).account.read(cx).library_scope() == account_scope;
                if !accept_chooser_completion(chooser_generation, this.chooser_generation) {
                    return;
                }
                this.choosing_cover = false;
                if !scope_matches {
                    this.error = Some(
                        create_account_changed_while(this.provider, "the cover image was loading")
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

    pub(super) fn choose_cover(&mut self, cx: &mut Context<Self>) {
        self.choose(cx);
    }

    pub(super) fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
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

    pub(super) fn pan_cover(
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

    pub(super) fn nudge_cover(
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

    pub(super) fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn save(&mut self, cx: &mut Context<Self>) {
        if self.completed || !dialog_actions_enabled(self.generation, self.choosing_cover) {
            return;
        }
        let title = self.title.read(cx).text().to_string();
        let description = self.description.read(cx).text().to_string();
        if let Err(error) = validate_create_draft(
            self.provider,
            &title,
            &description,
            self.cover.is_some(),
            provider_covers_required(self.provider),
        ) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let request = PlaylistCreateRequest {
            account_scope: self.account_scope.clone(),
            title,
            description,
            private: self.private,
            cover: self.cover.clone(),
            track_ids: self.track_ids.clone(),
            provider: self.provider,
        };
        match self.library.update(cx, |library, cx| match self.provider {
            Provider::Deezer => library.start_playlist_create(request, cx),
            Provider::SoundCloud => library.start_soundcloud_playlist_create(request, cx),
        }) {
            Ok(generation) => {
                self.generation = Some(generation);
                self.retry_generation = Some(generation);
                self.error = None;
            }
            Err(error) => self.error = Some(error.message_for(self.provider).into()),
        }
        cx.notify();
    }

    pub(super) fn retry_initial_add(&mut self, cx: &mut Context<Self>) {
        if self.generation.is_some() || self.choosing_cover {
            return;
        }
        let Some(generation) = self.retry_generation else {
            self.error = Some("The initial track retry is no longer available.".into());
            cx.notify();
            return;
        };
        if self.track_ids.is_empty() {
            self.error = Some("The initial track retry is no longer available.".into());
            cx.notify();
            return;
        }
        match self.library.update(cx, |library, cx| {
            library.start_playlist_create_add_retry(
                &self.account_scope,
                generation,
                self.track_ids.clone(),
                cx,
            )
        }) {
            Ok(()) => {
                self.generation = Some(generation);
                self.error = None;
            }
            Err(error) => self.error = Some(error.message().into()),
        }
        cx.notify();
    }

    fn sync_create_phase(&mut self, phase: Option<CreatePhase>, cx: &mut Context<Self>) {
        if self.generation.is_none() {
            return;
        }
        let Some(phase) = phase else {
            self.generation = None;
            self.error = Some(
                create_account_changed_while(self.provider, "the playlist was being created")
                    .into(),
            );
            cx.notify();
            return;
        };
        match phase {
            CreatePhase::Succeeded => self.completed = true,
            CreatePhase::Partial(error) | CreatePhase::Failed(error) => {
                self.generation = None;
                self.error = Some(error.into());
            }
            CreatePhase::Idle
            | CreatePhase::Uploading
            | CreatePhase::Creating
            | CreatePhase::Adding => {}
        }
    }
}

impl DialogCloseTarget for PlaylistCreateDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }

    fn after_dialog_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(playlist_id) = created_playlist_id_for_navigation(
            self.completed,
            self.library
                .read(cx)
                .playlist_catalog(self.provider)
                .create
                .playlist_id
                .clone(),
        ) else {
            return;
        };
        let title = self.title.read(cx).text().to_string();
        let title = title.trim();
        let title = if title.is_empty() {
            "Playlist".to_owned()
        } else {
            title.to_owned()
        };
        self.library.update(cx, |library, cx| {
            library.open_card(
                Card {
                    kind: Category::Playlists,
                    id: playlist_id,
                    title,
                    source: self.provider,
                    ..Default::default()
                },
                cx,
            );
        });
    }
}

pub(super) fn created_playlist_id_for_navigation(
    completed: bool,
    playlist_id: Option<String>,
) -> Option<String> {
    completed.then_some(playlist_id).flatten().filter(|id| {
        !id.trim().is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
    })
}

impl PlaylistCoverEditor for PlaylistCreateDialog {
    fn provider(&self) -> crate::search::Provider {
        self.provider
    }

    fn cover(&self) -> Option<&CoverDraft> {
        self.cover.as_ref()
    }

    fn cover_preview(&self) -> Option<Arc<Image>> {
        self.cover_preview.clone()
    }

    fn zoom_slider(&self) -> &Entity<SliderState> {
        &self.zoom_slider
    }

    fn choose_cover(&mut self, cx: &mut Context<Self>) {
        PlaylistCreateDialog::choose_cover(self, cx);
    }

    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        PlaylistCreateDialog::drop_cover_paths(self, paths, cx);
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
        PlaylistCreateDialog::pan_cover(
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
        PlaylistCreateDialog::nudge_cover(self, pointer_delta_x, pointer_delta_y, preview_size, cx);
    }

    fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>) {
        PlaylistCreateDialog::nudge_zoom(self, delta, window, cx);
    }
}

fn can_choose_cover(busy: bool, choosing: bool, account_changed: bool) -> bool {
    !busy && !choosing && !account_changed
}

pub(super) fn dialog_actions_enabled(generation: Option<u64>, choosing_cover: bool) -> bool {
    generation.is_none() && !choosing_cover
}

pub(super) const fn dialog_keyboard_shortcuts_enabled() -> bool {
    false
}

pub(super) fn escape_can_close_dialog(
    generation: Option<u64>,
    choosing_cover: bool,
    completed: bool,
) -> bool {
    !completed && dialog_actions_enabled(generation, choosing_cover)
}

pub(super) fn dialog_max_height(viewport_height: f32) -> f32 {
    (viewport_height - 32.).max(0.)
}

pub(super) fn initial_tracks_copy(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "This playlist will start with 1 selected track.".into(),
        n => format!("This playlist will start with {n} selected tracks."),
    }
}

const COVER_CHOOSER_HEIGHT: f32 = 92.;

pub(super) fn estimated_dialog_height(
    viewport_height: f32,
    has_cover: bool,
    stacked_crop: bool,
    has_initial_track: bool,
    has_error: bool,
    covers_enabled: bool,
) -> f32 {
    let mut content_height: f32 = 489.;
    if !covers_enabled {
        content_height -= COVER_CHOOSER_HEIGHT;
    }
    if has_initial_track {
        content_height += 33.;
    }
    if covers_enabled && has_cover {
        content_height += if stacked_crop { 260. } else { 138. };
    }
    if has_error {
        content_height += 28.;
    }
    content_height.min(dialog_max_height(viewport_height))
}

pub(super) fn dialog_width(viewport_width: f32) -> f32 {
    (viewport_width - 32.).clamp(0., 560.)
}

pub(super) fn body_max_height(viewport_height: f32) -> f32 {
    (dialog_max_height(viewport_height) - 124.).max(0.)
}

pub(super) fn crop_panel_stacks(viewport_width: f32, viewport_height: f32) -> bool {
    dialog_width(viewport_width) < 480. || viewport_height < 620.
}

fn accept_chooser_completion(expected: u64, current: u64) -> bool {
    expected == current
}

fn validate_create_draft(
    provider: Provider,
    title: &str,
    description: &str,
    cover: bool,
    cover_required: bool,
) -> Result<(), &'static str> {
    playlist_form::validate_draft(PlaylistFormTarget::Provider(provider), title, description)?;
    if cover_required && !cover {
        return Err("Choose and crop a playlist cover.");
    }
    Ok(())
}

impl PlaylistCreateDialog {
    pub(super) fn covers_supported(&self) -> bool {
        provider_covers_supported(self.provider)
    }
}

#[cfg(test)]
mod tests {
    use crate::search::Provider;

    use super::{
        DESCRIPTION_MAX_CHARS, TITLE_MAX_CHARS, accept_chooser_completion, body_max_height,
        can_choose_cover, created_playlist_id_for_navigation, crop_panel_stacks,
        dialog_actions_enabled, dialog_keyboard_shortcuts_enabled, dialog_max_height, dialog_width,
        escape_can_close_dialog, estimated_dialog_height, initial_tracks_copy,
        validate_create_draft,
    };
    use crate::library::playlist_form::{clamp_selection_to_prefix, truncate_input};
    #[test]
    fn creation_limits_and_cover_are_mandatory() {
        assert_eq!(
            validate_create_draft(Provider::Deezer, "", "", true, true),
            Err("Enter a playlist title.")
        );
        assert_eq!(
            validate_create_draft(Provider::Deezer, "Title", "", false, true),
            Err("Choose and crop a playlist cover.")
        );
        assert!(
            validate_create_draft(
                Provider::Deezer,
                &"a".repeat(50),
                &"b".repeat(200),
                true,
                true
            )
            .is_ok()
        );
        assert!(validate_create_draft(Provider::Deezer, &"a".repeat(51), "", true, true).is_err());
        assert_eq!(
            validate_create_draft(Provider::Deezer, "Title", &"b".repeat(201), true, true),
            Err("Playlist descriptions can contain at most 200 characters.")
        );
        assert!(
            validate_create_draft(
                Provider::Deezer,
                "Title",
                "First line\nSecond line",
                true,
                true
            )
            .is_ok()
        );
        assert_eq!(
            validate_create_draft(Provider::Deezer, "Title", "Body\u{0000}", true, true),
            Err("Playlist descriptions cannot contain unsupported control characters.")
        );
    }

    #[test]
    fn soundcloud_create_does_not_require_cover() {
        assert!(validate_create_draft(Provider::SoundCloud, "Title", "", false, false).is_ok());
        assert!(
            validate_create_draft(Provider::SoundCloud, "Title", "Notes", false, false).is_ok()
        );
        assert_eq!(
            validate_create_draft(Provider::SoundCloud, "", "", false, false),
            Err("Enter a playlist title.")
        );
    }

    #[test]
    fn soundcloud_creation_uses_the_official_text_boundaries() {
        assert!(
            validate_create_draft(
                Provider::SoundCloud,
                &"a".repeat(100),
                &"b".repeat(4000),
                false,
                false,
            )
            .is_ok()
        );
        assert!(
            validate_create_draft(Provider::SoundCloud, &"a".repeat(101), "", false, false,)
                .is_err()
        );
        assert_eq!(
            validate_create_draft(
                Provider::SoundCloud,
                "Title",
                &"b".repeat(4001),
                false,
                false,
            ),
            Err("SoundCloud playlist descriptions can contain at most 4000 characters.")
        );
        assert!(
            validate_create_draft(
                Provider::SoundCloud,
                "é🎵",
                "First line\nSecond line",
                false,
                false,
            )
            .is_ok()
        );
    }

    #[test]
    fn chooser_state_suppresses_duplicates_and_stale_completions() {
        assert!(can_choose_cover(false, false, false));
        assert!(!can_choose_cover(false, true, false));
        assert!(!can_choose_cover(true, false, false));
        assert!(!can_choose_cover(false, false, true));
        assert!(accept_chooser_completion(2, 2));
        assert!(!accept_chooser_completion(1, 2));
    }

    #[test]
    fn dialog_actions_stay_guarded_while_busy() {
        assert!(dialog_actions_enabled(None, false));
        assert!(!dialog_actions_enabled(Some(9), false));
        assert!(!dialog_actions_enabled(None, true));
        assert!(!dialog_actions_enabled(Some(9), true));
    }

    #[test]
    fn playlist_dialog_does_not_enable_global_confirm_shortcut() {
        assert!(!dialog_keyboard_shortcuts_enabled());
        assert!(escape_can_close_dialog(None, false, false));
        assert!(!escape_can_close_dialog(Some(9), false, false));
        assert!(!escape_can_close_dialog(None, true, false));
        assert!(!escape_can_close_dialog(None, false, true));
    }

    #[test]
    fn dialog_and_body_respect_short_viewports() {
        assert_eq!(dialog_max_height(760.), 728.);
        assert_eq!(body_max_height(760.), 604.);
        assert_eq!(dialog_max_height(300.), 268.);
        assert_eq!(body_max_height(300.), 144.);
        assert_eq!(dialog_max_height(20.), 0.);
        assert_eq!(body_max_height(20.), 0.);
    }

    #[test]
    fn dialog_width_stays_within_safe_viewport_margins() {
        assert_eq!(dialog_width(1200.), 560.);
        assert_eq!(dialog_width(560.), 528.);
        assert_eq!(dialog_width(32.), 0.);
        assert_eq!(dialog_width(20.), 0.);
    }

    #[test]
    fn dialog_uses_content_height_for_centering_instead_of_the_viewport_cap() {
        assert_eq!(
            estimated_dialog_height(900., false, false, false, false, true),
            489.
        );
        assert_eq!(
            estimated_dialog_height(900., true, false, false, false, true),
            627.
        );
        assert_eq!(
            estimated_dialog_height(560., true, true, true, true, true),
            528.
        );
        assert_eq!(
            estimated_dialog_height(900., false, false, false, false, false),
            397.
        );
    }

    #[test]
    fn crop_panel_reflows_for_narrow_or_short_viewports() {
        assert!(!crop_panel_stacks(1200., 800.));
        assert!(crop_panel_stacks(480., 800.));
        assert!(crop_panel_stacks(1200., 619.));
    }

    #[test]
    fn input_truncation_counts_unicode_scalars_and_preserves_lines() {
        assert_eq!(truncate_input("é🎵abc", 2), "é🎵");
        assert_eq!(truncate_input("a\nb\nc", 3), "a\nb");
        assert_eq!(TITLE_MAX_CHARS, 50);
        assert_eq!(DESCRIPTION_MAX_CHARS, 200);
    }

    #[test]
    fn truncation_clamps_selection_to_the_preserved_prefix() {
        let limit = "é🎵".len();
        let selection = 2..12;
        assert_eq!(clamp_selection_to_prefix(selection, limit), 2..limit);
    }

    #[test]
    fn create_dialog_dismisses_on_escape_and_overlay_click_only_when_idle() {
        assert!(escape_can_close_dialog(None, false, false));
        assert!(!escape_can_close_dialog(Some(9), false, false));
        assert!(!escape_can_close_dialog(None, true, false));
        assert!(!escape_can_close_dialog(None, false, true));
    }

    #[test]
    fn created_playlist_navigates_to_detail_only_for_valid_completed_ids() {
        assert_eq!(
            created_playlist_id_for_navigation(true, Some("42".into())),
            Some("42".into())
        );
        assert_eq!(
            created_playlist_id_for_navigation(false, Some("42".into())),
            None
        );
        assert_eq!(created_playlist_id_for_navigation(true, None), None);
        assert_eq!(
            created_playlist_id_for_navigation(true, Some("".into())),
            None
        );
        assert_eq!(
            created_playlist_id_for_navigation(true, Some("abc".into())),
            None
        );
    }

    #[test]
    fn initial_tracks_copy_uses_singular_and_plural() {
        assert_eq!(initial_tracks_copy(0), "");
        assert_eq!(
            initial_tracks_copy(1),
            "This playlist will start with 1 selected track."
        );
        assert_eq!(
            initial_tracks_copy(12),
            "This playlist will start with 12 selected tracks."
        );
    }
}
