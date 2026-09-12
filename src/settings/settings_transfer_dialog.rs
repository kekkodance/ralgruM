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
        cx.emit(super::SettingsEvent::Imported(settings));
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
                            "Could Not Import Settings",
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
                        "Could Not Export Settings",
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

#[cfg(test)]
mod tests {
    #[test]
    fn transfer_dialog_contract_keeps_account_copy_out_of_the_explanation() {
        let source = include_str!("settings_transfer_dialog.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(production.contains("Import / Export"));
        assert!(
            production
                .contains("Save a copy of your settings, or restore them from a settings file.")
        );
        assert!(
            production.contains(
                "Accounts and sign-in details stay on this device and are never included."
            )
        );
        let import_start = production.find("    fn import(").expect("import action");
        let export_start = production.find("    fn export(").expect("export action");
        let close_impl = production
            .find("\n}\n\nimpl DialogCloseTarget")
            .expect("dialog implementation end");
        let import = &production[import_start..export_start];
        let export = &production[export_start..close_impl];
        for (body, title, picker_method) in [
            (import, "Import settings", ".pick_file()"),
            (export, "Export settings", ".save_file()"),
        ] {
            assert!(body.contains("rfd::AsyncFileDialog::new()"));
            assert!(body.contains("add_filter(\"JSON settings\", &[\"json\"]"));
            assert!(body.contains(&format!("set_title(\"{title}\")")));
            assert!(body.contains(picker_method));
            assert!(body.contains("file.path().to_path_buf()"));
            assert!(body.contains("Err(error) =>"));
            assert!(body.contains("crate::toast::push_global"));
            assert!(body.contains("request_dialog_close(dialog, window, cx)"));
            assert!(
                body.find("cx.spawn_in(window").unwrap()
                    < body.find("rfd::AsyncFileDialog").unwrap()
            );
            assert!(body.find("rfd::AsyncFileDialog").unwrap() < body.find(".await").unwrap());
            assert!(body.find(".await").unwrap() < body.find("this.update_in").unwrap());
        }
        assert!(!production.contains("prompt_for_paths"));
        assert!(!production.contains("prompt_for_new_path"));
        assert!(production.contains("flex_1()"));
        assert!(production.contains("LocalIcon::Download"));
        assert!(production.contains("LocalIcon::Upload"));
        let import_button = production
            .split_once(".child(danger_secondary_dialog_button_with_disabled(")
            .expect("danger import button")
            .1
            .split_once(".child(secondary_dialog_button_with_disabled(")
            .expect("neutral export button follows import")
            .0;
        assert!(import_button.contains("\"import-settings\""));
        assert!(import_button.contains("Some(LocalIcon::Download)"));
        assert!(import_button.contains("\"Import\""));
        assert!(import_button.contains(".w(px(TRANSFER_ACTION_WIDTH_PX))"));

        let export_button = production
            .split_once("\"export-settings\"")
            .expect("export button")
            .1;
        assert!(export_button.contains("Some(LocalIcon::Upload)"));
        assert!(export_button.contains("\"Export\""));
        assert!(export_button.contains(".w(px(TRANSFER_ACTION_WIDTH_PX))"));
        assert!(production.contains("const TRANSFER_ACTION_WIDTH_PX: f32 = 88."));
        assert!(!production.contains("rfd::FileDialog::new"));
        assert!(production.contains("ralgrum-settings.json"));
        assert!(production.contains("SettingsTransferDialog"));
    }

    #[test]
    fn transfer_dialog_closes_import_failures_before_toasting() {
        let source = include_str!("settings_transfer_dialog.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        let import = production
            .split("    fn import(")
            .nth(1)
            .expect("import action");
        let import = &import[..import.find("    fn export(").unwrap()];
        let error = import
            .split_once("Err(error) => {")
            .map(|(_, body)| body)
            .expect("import error arm");
        assert!(error.contains("window.close_dialog(cx)"));
        assert!(error.contains("crate::toast::push_global"));
        assert!(
            error.find("window.close_dialog(cx)").unwrap()
                < error.find("crate::toast::push_global").unwrap()
        );
        assert!(import.contains("this.update_in(cx"));
        assert!(import.find("Ok(())").unwrap() < import.find("request_dialog_close").unwrap());
    }
}
