use gpui::{
    AnimationExt, Context, Entity, IntoElement, KeyDownEvent, Render, SharedString, Window, div,
    prelude::*, px, relative, rgb,
};
use gpui_component::WindowExt;

use super::{
    local_persistence::LocalPlaylistMutationOutcome,
    local_playlist_controller::LocalPlaylistCompletion, local_playlist_store::LocalPlaylistError,
    view::LibraryView,
};
use crate::{
    app_button::{
        danger_secondary_dialog_button_with_disabled, secondary_dialog_button_with_disabled,
    },
    assets::{LocalIcon, local_icon},
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND},
};

pub(crate) struct LocalPlaylistDeleteDialog {
    library: Entity<LibraryView>,
    playlist_id: String,
    title: String,
    error: Option<SharedString>,
    busy: bool,
    completed: bool,
    close_motion: DialogCloseMotion,
}

impl LocalPlaylistDeleteDialog {
    pub(crate) fn open(
        library: Entity<LibraryView>,
        playlist_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let dialog = cx.new(|cx| {
            cx.observe(&library, |_, _, cx| cx.notify()).detach();
            Self {
                library,
                playlist_id,
                title,
                error: None,
                busy: false,
                completed: false,
                close_motion: DialogCloseMotion::default(),
            }
        });
        let centered_top = crate::dialog_layout::centered_margin_top(
            f32::from(window.viewport_size().height),
            240.,
        );
        let dialog_content = dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, cx| {
            let state = dialog_content.read(cx);
            let cancel_handle = dialog_content.clone();
            dialog_view
                .w(px(460.))
                .max_w(px(520.))
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
                    cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    false
                })
                .closing(state.close_motion.closing(), state.close_motion.epoch())
                .child(dialog_content.clone())
        });
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.completed || self.busy {
            return;
        }
        self.busy = true;
        let dialog = cx.entity().clone();
        let result = self.library.update(cx, |library, cx| {
            let completion: LocalPlaylistCompletion = Box::new(
                move |result: Result<LocalPlaylistMutationOutcome, LocalPlaylistError>,
                      _view: &mut LibraryView,
                      cx: &mut Context<LibraryView>| {
                    dialog.update(cx, |dialog, cx| {
                        dialog.busy = false;
                        match result {
                            Ok(LocalPlaylistMutationOutcome::Deleted { changed: true }) => {
                                dialog.error = None;
                                dialog.completed = true;
                            }
                            Ok(LocalPlaylistMutationOutcome::Deleted { changed: false }) => {
                                dialog.error = Some("The local playlist could not be found.".into())
                            }
                            Err(error) => dialog.error = Some(error.to_string().into()),
                            _ => {}
                        }
                        cx.notify();
                    });
                },
            );
            library.delete_local_playlist(self.playlist_id.clone(), completion, cx)
        });
        if let Err(error) = result {
            self.busy = false;
            self.error = Some(error.to_string().into());
        }
        cx.notify();
    }
}

impl DialogCloseTarget for LocalPlaylistDeleteDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for LocalPlaylistDeleteDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.completed {
            request_dialog_close(self, window, cx);
        }
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog = div()
            .id("local-playlist-delete-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                request_dialog_close(this, window, cx);
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
                            .child(local_icon(LocalIcon::TrashCan, DANGER).size_4())
                            .child("Delete local playlist"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(18.))
                    .bg(rgb(BACKGROUND))
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.5))
                            .child(format!(
                                "Delete \"{}\" from your Local library? This cannot be undone.",
                                self.title
                            )),
                    )
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
                        "cancel-local-playlist-delete",
                        None,
                        "Cancel",
                        false,
                        cx.listener(|this, _, window, cx| request_dialog_close(this, window, cx)),
                    ))
                    .child(danger_secondary_dialog_button_with_disabled(
                        "confirm-local-playlist-delete",
                        Some(LocalIcon::TrashCan),
                        "Delete playlist",
                        self.busy,
                        cx.listener(|this, _, _, cx| this.submit(cx)),
                    )),
            );
        if closing {
            dialog
                .with_animation(
                    ("local-playlist-delete-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {}
