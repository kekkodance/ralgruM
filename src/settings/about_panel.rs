use crate::{
    assets::{LocalIcon, local_icon},
    theme::{FOREGROUND, MUTED, PRIMARY},
};
use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Role, div, prelude::*, px, rgb, rgba,
};

use super::{SettingsView, service_panel::settings_card};

const AUTHOR_GITHUB_URL: &str = "https://github.com/kekkodance";
const APP_MARK_PX: f32 = 96.;
const ABOUT_PANEL_TOP_PADDING_PX: f32 = 12.;
const HEADING_ICON_OPTICAL_OFFSET_PX: f32 = 1.;
// The clickable notice name keeps a transparent 1px border so showing focus
// never moves layout, which leaves its text 1px above the muted note.
// Nudge it down the same way card_heading nudges its icon.
const NOTICE_LINK_OPTICAL_OFFSET_PX: f32 = 1.;

impl SettingsView {
    pub(super) fn render_about(&mut self, _: &mut Context<Self>) -> AnyElement {
        let identity = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.))
            .child(
                local_icon(LocalIcon::Music, PRIMARY)
                    .size(px(APP_MARK_PX))
                    .flex_none(),
            )
            .child(
                div()
                    .text_size(px(21.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(FOREGROUND))
                    .child("ralgruM"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(env!("CARGO_PKG_VERSION")),
            )
            .child(author_row())
            .into_any_element();
        let thanks = settings_card()
            .child(card_heading(LocalIcon::Heart, "Special Thanks"))
            .child(thanks_line("BadManners Team", ", for making it possible"))
            .child(thanks_line("Furby", ", for testing"))
            .child(thanks_line("Sigma", ", for the app icon"))
            .into_any_element();
        let notices = settings_card()
            .child(card_heading(LocalIcon::List, "Third-party notices"))
            .child(font_awesome_notice())
            .child(linked_notice_line(
                "about-chromium",
                "Chromium",
                " cursor resources are licensed under the BSD 3-Clause License.",
                crate::external_url::open_chromium,
                "Could not open Chromium",
            ))
            .child(linked_notice_line(
                "about-gpui",
                "GPUI",
                " is developed by Zed Industries.",
                crate::external_url::open_gpui,
                "Could not open GPUI",
            ))
            .child(linked_notice_line(
                "about-gpui-component",
                "gpui-component",
                " is licensed under Apache 2.0.",
                crate::external_url::open_gpui_component,
                "Could not open gpui-component",
            ))
            .child(linked_notice_line(
                "about-asio-sdk",
                "Steinberg ASIO SDK",
                " sources are used under the GPLv3 option.",
                crate::external_url::open_asio_sdk,
                "Could not open ASIO SDK",
            ))
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .pt(px(ABOUT_PANEL_TOP_PADDING_PX))
            .gap(px(18.))
            .children([identity, thanks, notices])
            .into_any_element()
    }
}

fn author_row() -> Div {
    div().child(
        div()
            .id("about-author-github")
            .flex()
            .items_center()
            .gap(px(7.))
            .focusable()
            .tab_stop(true)
            .role(Role::Button)
            .aria_label("Open kekkodance on GitHub")
            .rounded(px(3.))
            .border_1()
            .border_color(rgba(0x00000000))
            .text_size(px(12.))
            .text_color(rgb(FOREGROUND))
            .cursor_pointer()
            .hover(|style| {
                style
                    .text_color(rgb(PRIMARY))
                    .text_decoration_1()
                    .text_decoration_color(rgb(PRIMARY))
            })
            .focus_visible(|style| style.border_color(rgb(PRIMARY)))
            .on_click(|_, _, cx| {
                if let Err(error) = crate::external_url::open_github_profile(AUTHOR_GITHUB_URL) {
                    crate::toast::push_global(
                        cx,
                        crate::toast::ToastKind::Error,
                        "Could not open GitHub",
                        Some(error.into()),
                    );
                }
            })
            .child(local_icon(LocalIcon::GitHub, FOREGROUND).size(px(13.)))
            .child("kekkodance"),
    )
}

fn thanks_line(name: &'static str, note: &'static str) -> Div {
    notice_line(name, note)
}

fn notice_line(name: &'static str, note: &'static str) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .text_size(px(12.5))
        .line_height(px(18.75))
        .child(div().flex_none().text_color(rgb(FOREGROUND)).child(name))
        .child(div().flex_none().text_color(rgb(MUTED)).child(note))
}

fn font_awesome_notice() -> Div {
    linked_notice_line(
        "about-font-awesome",
        "Font Awesome",
        " Free icons are licensed under CC BY 4.0.",
        crate::external_url::open_font_awesome,
        "Could not open Font Awesome",
    )
}

fn linked_notice_line(
    id: &'static str,
    name: &'static str,
    note: &'static str,
    open: fn() -> Result<(), String>,
    error_title: &'static str,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .text_size(px(12.5))
        .line_height(px(18.75))
        .child(
            div()
                .id(id)
                .focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(format!("Open {name}"))
                .relative()
                .top(px(NOTICE_LINK_OPTICAL_OFFSET_PX))
                .flex_none()
                .rounded(px(3.))
                .border_1()
                .border_color(rgba(0x00000000))
                .text_color(rgb(FOREGROUND))
                .cursor_pointer()
                .hover(|style| {
                    style
                        .text_color(rgb(PRIMARY))
                        .text_decoration_1()
                        .text_decoration_color(rgb(PRIMARY))
                })
                .focus_visible(|style| style.border_color(rgb(PRIMARY)))
                .on_click(move |_, _, cx| {
                    if let Err(error) = open() {
                        crate::toast::push_global(
                            cx,
                            crate::toast::ToastKind::Error,
                            error_title,
                            Some(error.into()),
                        );
                    }
                })
                .child(name),
        )
        .child(div().flex_none().text_color(rgb(MUTED)).child(note))
}

fn card_heading(icon: LocalIcon, title: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(FOREGROUND))
        .child(
            div()
                .relative()
                .top(px(HEADING_ICON_OPTICAL_OFFSET_PX))
                .flex_none()
                .child(local_icon(icon, MUTED).size(px(13.))),
        )
        .child(title)
}
