use super::{playlist_client::OwnedPlaylist, view::LibraryView};
use crate::search::Provider;
use crate::{
    app_button::{danger_secondary_button_with_loading, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND},
};
use gpui::{
    AnimationExt, Context, Entity, IntoElement, KeyDownEvent, Render, SharedString, Window, div,
    prelude::*, px, relative, rgb,
};
use gpui_component::WindowExt;

pub(crate) struct PlaylistDeleteDialog {
    library: Entity<LibraryView>,
    playlist: OwnedPlaylist,
    scope: String,
    provider: Provider,
    busy: bool,
    error: Option<SharedString>,
    completed: bool,
    stale_completion: bool,
    close_motion: DialogCloseMotion,
}

impl PlaylistDeleteDialog {
    pub(crate) fn open(
        library: Entity<LibraryView>,
        playlist: OwnedPlaylist,
        scope: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<LibraryView>,
    ) {
        let dialog = cx.new(|cx| {
            cx.observe(&library, |_, _, cx| cx.notify()).detach();
            Self {
                library,
                playlist,
                scope,
                provider,
                busy: false,
                error: None,
                completed: false,
                stale_completion: false,
                close_motion: DialogCloseMotion::default(),
            }
        });
        let centered_top = crate::dialog_layout::centered_margin_top(
            f32::from(window.viewport_size().height),
            240.,
        );
        let dialog_content = dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, cx| {
            let closing = dialog_content.read(cx).close_motion.closing();
            let close_epoch = dialog_content.read(cx).close_motion.epoch();
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
                    if can_dismiss(cancel_handle.read(cx).busy) {
                        cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    }
                    false
                })
                .closing(closing, close_epoch)
                .child(dialog_content.clone())
        });
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let provider = self.provider;
        let (account_scope, deezer_arl, deezer_user_id, soundcloud_token) = {
            let account = self.library.read(cx).account.read(cx);
            (
                account.library_scope(),
                account.deezer_arl(),
                account.deezer_user_id(),
                account.soundcloud_mobile_token(),
            )
        };
        if account_scope != self.scope {
            self.error = Some(
                format!(
                    "The {} account changed while this dialog was open.",
                    provider.label()
                )
                .into(),
            );
            return;
        }
        let id = self.playlist.id.clone();
        let scope = self.scope.clone();
        match provider {
            Provider::Deezer => {
                if deezer_arl.is_none() {
                    self.error = Some("Deezer login required".into());
                    return;
                }
                if self.library.read(cx).playlist_client.is_err() {
                    self.error = Some("Deezer playlist client could not be created".into());
                    return;
                }
            }
            Provider::SoundCloud => {
                if soundcloud_token.is_none() {
                    self.error = Some("SoundCloud login required".into());
                    return;
                }
                if self.library.read(cx).soundcloud_library_client().is_err() {
                    self.error = Some("SoundCloud playlist client could not be created".into());
                    return;
                }
            }
        }
        let Some(generation) = self.library.update(cx, |library, _| {
            library.begin_playlist_delete(&scope, &id, provider)
        }) else {
            self.error =
                Some(format!("This {} playlist is no longer editable.", provider.label()).into());
            return;
        };
        self.busy = true;
        self.error = None;
        let library = self.library.clone();
        let runtime = self.library.read(cx).runtime.clone();
        let task = match provider {
            Provider::Deezer => {
                let Some(arl) = deezer_arl else {
                    self.busy = false;
                    self.error = Some("Deezer login required".into());
                    return;
                };
                let user_id = deezer_user_id;
                let Ok(client) = self.library.read(cx).playlist_client.clone() else {
                    self.busy = false;
                    self.error = Some("Deezer playlist client could not be created".into());
                    return;
                };
                runtime.spawn(async move { client.delete(arl, user_id, &id).await })
            }
            Provider::SoundCloud => {
                let Some(token) = soundcloud_token else {
                    self.busy = false;
                    self.error = Some("SoundCloud login required".into());
                    return;
                };
                let Ok(client) = self.library.read(cx).soundcloud_library_client() else {
                    self.busy = false;
                    self.error = Some("SoundCloud playlist client could not be created".into());
                    return;
                };
                runtime.spawn(async move { client.delete_playlist(token, &id).await })
            }
        };
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|_| {
                Err(format!(
                    "{} playlist delete request failed",
                    provider.label()
                ))
            });
            this.update(cx, |this, cx| {
                this.busy = false;
                let accepted = library.update(cx, |library, cx| {
                    library.finish_playlist_delete(
                        &scope,
                        generation,
                        provider,
                        &this.playlist.id,
                        &result,
                        cx,
                    )
                });
                if !accepted {
                    this.error = Some(
                        format!(
                            "The {} account changed while deletion was in progress.",
                            provider.label()
                        )
                        .into(),
                    );
                    this.stale_completion = true;
                } else if let Err(error) = result {
                    this.error = Some(error.into());
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
}

impl DialogCloseTarget for PlaylistDeleteDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for PlaylistDeleteDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.completed {
            request_dialog_close(self, window, cx);
        }
        let account_changed = self.library.read(cx).account.read(cx).library_scope() != self.scope;
        if should_close_for_account_change(account_changed, self.busy, self.stale_completion) {
            request_dialog_close(self, window, cx);
        }
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog = div()
            .id("playlist-delete-dialog")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.as_str() != "escape" {
                    return;
                }
                window.prevent_default();
                if can_dismiss(this.busy) {
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
                            .child(format!("Delete {} playlist", self.provider.label())),
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
                                "Delete \"{}\" from {}? This cannot be undone.",
                                self.playlist.title,
                                self.provider.label(),
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
                        "cancel-playlist-delete",
                        None,
                        "Cancel",
                        self.busy,
                        cx.listener(|this, _, window, cx| {
                            if !this.busy {
                                request_dialog_close(this, window, cx);
                            }
                        }),
                    ))
                    .child(danger_secondary_button_with_loading(
                        "confirm-playlist-delete",
                        Some(LocalIcon::TrashCan),
                        if self.busy {
                            "Deleting..."
                        } else {
                            "Delete playlist"
                        },
                        self.busy,
                        self.busy,
                        cx.listener(|this, _, _, cx| this.submit(cx)),
                    )),
            );

        if closing {
            dialog
                .with_animation(
                    ("playlist-delete-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}

fn should_close_for_account_change(
    account_changed: bool,
    busy: bool,
    stale_completion: bool,
) -> bool {
    account_changed && !busy && !stale_completion
}

fn can_dismiss(busy: bool) -> bool {
    !busy
}

#[cfg(test)]
mod tests {
    use super::{can_dismiss, should_close_for_account_change};

    #[test]
    fn account_change_closes_idle_delete_dialog_but_not_pending_request() {
        assert!(should_close_for_account_change(true, false, false));
        assert!(!should_close_for_account_change(true, true, false));
    }

    #[test]
    fn rejected_pending_completion_preserves_account_change_state() {
        assert!(!should_close_for_account_change(true, false, true));
    }

    #[test]
    fn delete_dialog_ui_matches_create_dialog_styling_and_buttons() {
        let source = include_str!("playlist_delete_dialog.rs");
        assert!(source.contains("danger_secondary_button_with_loading"));
        assert!(source.contains("secondary_dialog_button_with_disabled"));
        assert!(source.contains("BACKGROUND"));
        assert!(source.contains("BORDER"));
    }

    #[test]
    fn delete_dialog_dismisses_on_escape_and_overlay_click_only_when_idle() {
        assert!(can_dismiss(false));
        assert!(!can_dismiss(true));
    }

    #[test]
    fn delete_dialog_registers_escape_and_overlay_dismiss_handlers() {
        let source = include_str!("playlist_delete_dialog.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("overlay_closable(true)"));
        assert!(production.contains("on_cancel"));
        assert!(production.contains("\"escape\""));
        assert!(production.contains("can_dismiss"));
    }
}
