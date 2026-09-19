use gpui::{
    Context, FontWeight, IntoElement, KeyDownEvent, Role, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::{Sizable, Size, spinner::Spinner};

use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon, widget_icon},
    theme::FOREGROUND,
    toast::{ToastKind, push_global},
    updater::model::Status,
};

use super::{
    RalgrumApp,
    sidebar::{SIDEBAR_ICON_GLYPH_SIZE, SIDEBAR_ICON_SLOT_SIZE_PX},
};

fn update_icon(status: &Status) -> LocalIcon {
    if matches!(status, Status::Ready | Status::Installing) {
        LocalIcon::RotateRight
    } else {
        LocalIcon::Download
    }
}

impl RalgrumApp {
    fn activate_update(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.updater.read(cx).status, Status::Error(_)) {
            if let Some(url) = self
                .updater
                .read(cx)
                .release
                .as_ref()
                .map(|release| release.page_url.clone())
                && let Err(error) = crate::external_url::open_update_release(&url)
            {
                push_global(
                    cx,
                    ToastKind::Error,
                    "Cannot open the release page",
                    Some(error.into()),
                );
            }
            return;
        }
        let ready = matches!(self.updater.read(cx).status, Status::Ready);
        if ready {
            match self
                .updater
                .update(cx, |updater, cx| updater.begin_install(cx))
            {
                Ok(task) => {
                    cx.spawn(async move |this, cx| {
                        let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
                        this.update_in(cx, |this, window, cx| match result {
                            Ok(()) => {
                                this.updater
                                    .update(cx, |updater, _| updater.handoff_started());
                                this._window_state
                                    .update(cx, |state, cx| state.persist_now(window, cx));
                                cx.quit();
                            }
                            Err(error) => {
                                this.updater.update(cx, |updater, cx| {
                                    updater.record_error(error.clone(), cx)
                                });
                                push_global(
                                    cx,
                                    ToastKind::Error,
                                    "Update could not start",
                                    Some(error.into()),
                                );
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
                Err(error) => {
                    self.updater
                        .update(cx, |updater, cx| updater.record_error(error.clone(), cx));
                    push_global(
                        cx,
                        ToastKind::Error,
                        "Update could not start",
                        Some(error.into()),
                    );
                }
            }
        } else {
            self.updater
                .update(cx, |updater, cx| updater.start_download(cx));
        }
    }

    pub(super) fn update_badge(
        &self,
        button_id: &'static str,
        compact: bool,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let updater = self.updater.read(cx);
        let icon = update_icon(&updater.status);
        let spinning = matches!(
            updater.status,
            Status::Downloading { .. } | Status::Installing
        );
        let version = updater
            .release
            .as_ref()
            .map(|release| release.version.to_string());
        let (label, tooltip, enabled) = match (&updater.status, version) {
            (Status::Available, Some(version)) => (
                format!("Update to v{version}"),
                format!("Download ralgruM v{version}"),
                true,
            ),
            (Status::Downloading { received }, Some(version)) => {
                let total = updater.release.as_ref().map_or(1, |release| release.size);
                let percent = received.saturating_mul(100) / total;
                (
                    format!("Downloading {percent}%"),
                    format!("Downloading ralgruM v{version}: {percent}%"),
                    false,
                )
            }
            (Status::Ready, Some(version)) => (
                "Restart to update".to_owned(),
                format!("Install ralgruM v{version} and restart"),
                true,
            ),
            (Status::Installing, Some(_)) => (
                "Preparing update".to_owned(),
                "Preparing the update helper".to_owned(),
                false,
            ),
            (Status::Error(error), Some(_)) => (
                "Open release".to_owned(),
                format!("Update failed: {error}. Click for the manual download."),
                true,
            ),
            _ => (String::new(), String::new(), false),
        };
        let visible = !label.is_empty();
        div().w_full().when(visible, |this| {
            this.child(
                div()
                    .id(button_id)
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .when(interactive, |this| this.focusable().tab_stop(true))
                    .w_full()
                    .h(px(36.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(0x818cf8d9))
                    .bg(rgba(0x6366f13d))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(7.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(FOREGROUND))
                    .when(enabled && interactive, |this| {
                        this.cursor_pointer()
                            .hover(|this| this.bg(rgba(0x6366f166)))
                    })
                    .child(
                        div()
                            .size(px(SIDEBAR_ICON_SLOT_SIZE_PX))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(spinning, |this| {
                                this.child(
                                    Spinner::new()
                                        .icon(widget_icon(LocalIcon::Spinner))
                                        .with_size(Size::Size(px(SIDEBAR_ICON_GLYPH_SIZE)))
                                        .color(rgb(FOREGROUND).into()),
                                )
                            })
                            .when(!spinning, |this| {
                                this.child(
                                    local_icon(icon, FOREGROUND).size(px(SIDEBAR_ICON_GLYPH_SIZE)),
                                )
                            }),
                    )
                    .when(!compact, |this| this.child(label))
                    .when(compact, |this| this.app_tooltip_right(tooltip))
                    .when(enabled && interactive, |this| {
                        this.on_click(
                            cx.listener(|this, _, window, cx| this.activate_update(window, cx)),
                        )
                        .on_key_down(cx.listener(
                            |this, event: &KeyDownEvent, window, cx| {
                                if crate::tab_keyboard::is_activation_key(
                                    event.keystroke.key.as_str(),
                                ) {
                                    window.prevent_default();
                                    this.activate_update(window, cx);
                                }
                            },
                        ))
                    }),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LocalIcon, Status, update_icon};

    #[test]
    fn downloaded_update_uses_restart_icon() {
        assert_eq!(update_icon(&Status::Available), LocalIcon::Download);
        assert_eq!(update_icon(&Status::Ready), LocalIcon::RotateRight);
        assert_eq!(update_icon(&Status::Installing), LocalIcon::RotateRight);
    }
}
