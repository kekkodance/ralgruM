use gpui::{
    AnimationExt, App, ClickEvent, Context, IntoElement, KeyDownEvent, Render, Window, div,
    prelude::*, px, relative, rgb,
};
use gpui_component::input::Input;

use super::soundcloud_playlist::create_dialog_title;
use super::{
    playlist_cover_editor::{CROP_PREVIEW_SIZE, cover_editor},
    playlist_create_dialog::{
        PlaylistCreateDialog, body_max_height, crop_panel_stacks, dialog_width,
        escape_can_close_dialog, initial_tracks_copy,
    },
    playlist_form::{self, private_playlist_switch},
    playlist_state::CreatePhase,
};
use crate::{
    app_button::{primary_button_with_disabled, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    dialog_layout::request_dialog_close,
    search::Provider,
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED, SOUNDCLOUD, SURFACE},
};

impl Render for PlaylistCreateDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        playlist_form::enforce_input_limits(
            &self.title,
            &self.description,
            playlist_form::PlaylistFormTarget::Provider(self.provider),
            window,
            cx,
        );
        if self.completed {
            request_dialog_close(self, window, cx);
        }
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let busy = self.generation.is_some();
        let controls_busy = busy || self.choosing_cover;
        let viewport_size = window.viewport_size();
        let viewport_width = f32::from(viewport_size.width);
        let viewport_height = f32::from(viewport_size.height);
        let body_max_height = body_max_height(viewport_height);
        let stack_crop = crop_panel_stacks(viewport_width, viewport_height);
        let crop_preview_size = if stack_crop {
            (dialog_width(viewport_width) - 60.)
                .max(0.)
                .min(CROP_PREVIEW_SIZE)
        } else {
            CROP_PREVIEW_SIZE
        };
        let partial = self.retry_generation.is_some_and(|generation| {
            !self.track_ids.is_empty()
                && matches!(
                    self.library
                        .read(cx)
                        .playlist_catalog(self.provider)
                        .create_phase_for(&self.account_scope, generation),
                    Some(CreatePhase::Partial(_))
                )
        });

        let dialog = div()
            .id("playlist-create-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                if escape_can_close_dialog(this.generation, this.choosing_cover, this.completed) {
                    request_dialog_close(this, window, cx);
                }
                cx.stop_propagation();
            }))
            .overflow_hidden()
            .rounded(px(8.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .child(dialog_header(self.provider))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .p(px(18.))
                    .max_h(px(body_max_height))
                    .id("playlist-create-body")
                    .overflow_y_scroll()
                    .bg(rgb(BACKGROUND))
                    .when(!self.track_ids.is_empty(), |d| {
                        d.child(
                            div()
                                .mb(px(1.))
                                .text_size(px(12.5))
                                .text_color(rgb(MUTED))
                                .line_height(relative(1.5))
                                .child(initial_tracks_copy(self.track_ids.len())),
                        )
                    })
                    .child(playlist_form::title_field(text_field_context_menu(
                        Input::new(&self.title).bg(rgb(SURFACE)),
                        self.title.clone(),
                    )))
                    .child(playlist_form::description_field(text_field_context_menu(
                        Input::new(&self.description).h(px(92.)).bg(rgb(SURFACE)),
                        self.description.clone(),
                    )))
                    .child(private_playlist_switch(
                        "create-private-row",
                        "create-private",
                        self.private,
                        controls_busy,
                        cx,
                        |this, checked, cx| {
                            this.private = checked;
                            cx.notify();
                        },
                    ))
                    .when(self.covers_supported(), |d| {
                        d.child(cover_editor(
                            self,
                            controls_busy,
                            stack_crop,
                            crop_preview_size,
                            cx,
                        ))
                    })
                    .when_some(self.error.clone(), |d, error| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(DANGER))
                                .child(error),
                        )
                    }),
            )
            .child(dialog_footer(partial, busy, controls_busy, cx));

        if closing {
            dialog
                .with_animation(
                    ("playlist-create-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

fn dialog_header(provider: Provider) -> impl IntoElement {
    let icon = match provider {
        Provider::Deezer => local_icon(LocalIcon::Deezer, 0xa238ff).size_4(),
        Provider::SoundCloud => local_icon(LocalIcon::SoundCloud, SOUNDCLOUD).size_4(),
    };
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
                .child(icon)
                .child(create_dialog_title(provider)),
        )
}

fn dialog_footer(
    partial: bool,
    busy: bool,
    disabled: bool,
    cx: &mut Context<PlaylistCreateDialog>,
) -> impl IntoElement {
    div()
        .flex()
        .justify_end()
        .gap(px(8.))
        .px(px(18.))
        .py(px(14.))
        .border_t_1()
        .border_color(rgb(BORDER))
        .bg(rgb(BACKGROUND))
        .when(partial, |d| {
            d.child(secondary_dialog_button(
                "retry-create-initial-add",
                if busy {
                    "Retrying..."
                } else {
                    "Retry adding track"
                },
                disabled,
                cx.listener(|this, _, _, cx| this.retry_initial_add(cx)),
            ))
        })
        .child(secondary_dialog_button(
            "cancel-create-playlist",
            "Cancel",
            disabled,
            cx.listener(|this, _, window, cx| {
                if this.generation.is_none() && !this.choosing_cover {
                    request_dialog_close(this, window, cx);
                }
            }),
        ))
        .child(primary_dialog_button(
            "save-create-playlist",
            LocalIcon::Plus,
            if busy {
                "Creating..."
            } else {
                "Create playlist"
            },
            disabled,
            cx.listener(|this, _, _, cx| this.save(cx)),
        ))
}

fn primary_dialog_button(
    id: &'static str,
    icon: LocalIcon,
    label: &'static str,
    disabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    primary_button_with_disabled(id, Some(icon), label, disabled, handler)
}

fn secondary_dialog_button(
    id: &'static str,
    label: &'static str,
    disabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    secondary_dialog_button_with_disabled(id, None, label, disabled, handler)
}

#[cfg(test)]
mod tests {
    #[test]
    fn creation_view_uses_shared_cover_editor_and_standard_form_controls() {
        let source = include_str!("playlist_create_view.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("cover_editor("));
        assert!(production.contains(".when(self.covers_supported()"));
        assert!(production.contains("Input::new(&self.description).h(px(92.)).bg(rgb(SURFACE))"));
        assert!(production.contains("private_playlist_switch"));
        assert!(production.contains("secondary_dialog_button_with_disabled"));
        assert!(production.contains("create_dialog_title(provider)"));
    }
}
