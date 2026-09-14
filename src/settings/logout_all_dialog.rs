use super::SettingsView;
use crate::{
    app_button::{danger_secondary_button_with_loading, secondary_dialog_button_with_disabled},
    assets::{LocalIcon, local_icon},
    dialog_layout::{DialogCloseMotion, DialogCloseTarget, request_dialog_close},
    theme::{BACKGROUND, BORDER, FOREGROUND},
};
use gpui::{
    AnimationExt, Context, Entity, IntoElement, KeyDownEvent, Render, Window, div, prelude::*, px,
    relative, rgb,
};
use gpui_component::WindowExt;

pub(crate) struct LogoutAllDialog {
    settings: Entity<SettingsView>,
    close_motion: DialogCloseMotion,
}

impl SettingsView {
    pub(crate) fn confirm_logout_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        LogoutAllDialog::open(cx.entity(), window, cx);
    }
}

impl LogoutAllDialog {
    fn open(settings: Entity<SettingsView>, window: &mut Window, cx: &mut Context<SettingsView>) {
        let dialog = cx.new(|_| Self {
            settings,
            close_motion: DialogCloseMotion::default(),
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
                    cancel_handle.update(cx, |this, cx| request_dialog_close(this, window, cx));
                    false
                })
                .closing(closing, close_epoch)
                .child(dialog_content.clone())
        });
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.update(cx, |settings, cx| {
            settings.logout_all(cx);
        });
        request_dialog_close(self, window, cx);
    }
}

impl DialogCloseTarget for LogoutAllDialog {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion {
        &mut self.close_motion
    }
}

impl Render for LogoutAllDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let closing = self.close_motion.closing();
        let close_epoch = self.close_motion.epoch();
        let dialog = div()
            .id("logout-all-dialog")
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
                    .bg(rgb(BACKGROUND))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(local_icon(LocalIcon::LogOut, FOREGROUND).size_4())
                            .child("Log out of all services?"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(18.))
                    .bg(rgb(BACKGROUND))
                    .child(div().text_size(px(13.)).line_height(relative(1.5)).child(
                        "This signs you out of Murglar, SoundCloud, and Deezer on this device.",
                    )),
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
                        "cancel-logout-all",
                        None,
                        "Cancel",
                        false,
                        cx.listener(|this, _, window, cx| {
                            request_dialog_close(this, window, cx);
                        }),
                    ))
                    .child(danger_secondary_button_with_loading(
                        "confirm-logout-all",
                        Some(LocalIcon::LogOut),
                        "Log Out All",
                        false,
                        false,
                        cx.listener(|this, _, window, cx| this.confirm(window, cx)),
                    )),
            );

        if closing {
            dialog
                .with_animation(
                    ("logout-all-dialog-close", close_epoch),
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
