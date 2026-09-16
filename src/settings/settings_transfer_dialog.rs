use std::path::Path;

use super::SettingsView;
use crate::{
    app_button::{
        danger_secondary_dialog_button_with_disabled, secondary_dialog_button_with_disabled,
    },
    assets::{LocalIcon, local_icon},
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    navigation_state::SettingsError,
    settings_transfer::{self, SettingsTransferError},
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED},
};
use gpui::{
    AnimationExt, Context, Entity, IntoElement, KeyDownEvent, Render, Window, div, prelude::*, px,
    relative, rgb,
};
use gpui_component::WindowExt;

const TRANSFER_ACTION_WIDTH_PX: f32 = 88.;

pub(crate) struct SettingsTransferDialog {
    settings: Entity<SettingsView>,
    close_motion: DialogCloseMotion,
}

impl SettingsView {
    pub(crate) fn open_settings_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        SettingsTransferDialog::open(cx.entity(), window, cx);
    }

    fn import_settings_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), SettingsTransferError> {
        if self.store.is_none() {
            return Err(SettingsTransferError::Settings(
                SettingsError::ConfigDirectoryUnavailable,
            ));
        }
        let mut settings = settings_transfer::import_settings(path)?;
        settings
            .validate_imported_downloads_dir()
            .map_err(SettingsTransferError::Settings)?;
        let store = self.store.as_mut().expect("checked above");
        store
            .persist(settings.clone())
            .map_err(SettingsTransferError::Settings)?;
        self.saved = settings.clone();
        self.draft = settings.clone();
        self.reset_selects(window, cx);
        self.save_error = None;
        self.import_sync_pending = true;
        cx.emit(super::SettingsEvent::Imported(Box::new(settings)));
        cx.notify();
        Ok(())
    }

    fn export_settings_file(&self, path: &Path) -> Result<(), SettingsTransferError> {
        settings_transfer::export_settings(path, self.saved())
    }
}

impl SettingsTransferDialog {
    fn open(settings: Entity<SettingsView>, window: &mut Window, cx: &mut Context<SettingsView>) {
        let dialog = cx.new(|_| Self {
            settings,
            close_motion: DialogCloseMotion::default(),
        });
        let centered_top = crate::dialog_layout::centered_margin_top(
            f32::from(window.viewport_size().height),
            280.,
        );
        let dialog_content = dialog.clone();
        window.open_dialog(cx, move |dialog_view, _, cx| {
            let closing = dialog_content.read(cx).close_motion.closing();
            let close_epoch = dialog_content.read(cx).close_motion.epoch();
            let cancel_handle = dialog_content.clone();
            dialog_view
                .w(px(500.))
                .max_w(px(560.))
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
                .closing(closing, close_epoch)
                .child(dialog_content.clone())
        });
    }

    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        cx.spawn_in(window, async move |this, cx| {
            let file_picker = rfd::AsyncFileDialog::new()
                .add_filter("JSON settings", &["json"])
                .set_title("Import settings")
                .pick_file();
            let Some(file) = file_picker.await else {
                return;
            };
            let path = file.path().to_path_buf();
            let _ = this.update_in(cx, |dialog, window, cx| {
                let result = settings.update(cx, |settings, cx| {
                    settings.import_settings_file(&path, window, cx)
                });
                match result {
                    Ok(()) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Success,
                            "Settings imported",
                            None,
                        );
                        request_dialog_close(dialog, window, cx);
                    }
                    Err(error) => {
                        window.close_dialog(cx);
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            "Could not import settings",
                            Some(error.to_string().into()),
                        );
                    }
                }
            });
        })
        .detach();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.settings.clone();
        cx.spawn_in(window, async move |this, cx| {
            let file_picker = rfd::AsyncFileDialog::new()
                .add_filter("JSON settings", &["json"])
                .set_title("Export settings")
                .set_file_name("ralgrum-settings.json")
                .save_file();
            let Some(file) = file_picker.await else {
                return;
            };
            let path = file.path().to_path_buf();
            let _ = this.update_in(cx, |dialog, window, cx| {
                let result = settings.read(cx).export_settings_file(&path);
                match result {
                    Ok(()) => {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Success,
                            "Settings exported",
                            None,
                        );
                        request_dialog_close(dialog, window, cx);
                    }
                    Err(error) => crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not export settings",
                        Some(error.to_string().into()),
                    ),
                }
            });
        })
        .detach();
    }
}

impl DialogCloseTarget for SettingsTransferDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for SettingsTransferDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog = div()
            .id("settings-transfer-dialog")
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
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .relative()
                            .size(px(16.))
                            .top(px(1.))
                            .child(local_icon(LocalIcon::Upload, FOREGROUND).size(px(14.))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Import / Export"),
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
                            .child(
                                "Save a copy of your settings, or restore them from a settings file.",
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .line_height(relative(1.45))
                            .text_color(rgb(MUTED))
                            .child(
                                "Accounts and sign-in details stay on this device and are never included.",
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_start()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BACKGROUND))
                    .child(secondary_dialog_button_with_disabled(
                        "cancel-settings-transfer",
                        None,
                        "Cancel",
                        false,
                        cx.listener(|this, _, window, cx| {
                            request_dialog_close(this, window, cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .child(danger_secondary_dialog_button_with_disabled(
                        "import-settings",
                        Some(LocalIcon::Download),
                        "Import",
                        false,
                        cx.listener(|this, _, window, cx| this.import(window, cx)),
                    ).w(px(TRANSFER_ACTION_WIDTH_PX)))
                    .child(secondary_dialog_button_with_disabled(
                        "export-settings",
                        Some(LocalIcon::Upload),
                        "Export",
                        false,
                        cx.listener(|this, _, window, cx| this.export(window, cx)),
                    ).w(px(TRANSFER_ACTION_WIDTH_PX))),
            );

        if closing {
            dialog
                .with_animation(
                    ("settings-transfer-dialog-close", close_epoch),
                    crate::motion::dialog_close(),
                    |this, delta| this.opacity(crate::motion::lerp(1.0, 0.0, delta)),
                )
                .into_any_element()
        } else {
            dialog.into_any_element()
        }
    }
}
