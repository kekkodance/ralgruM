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
                "Could Not Open Chromium",
            ))
            .child(linked_notice_line(
                "about-gpui",
                "GPUI",
                " is developed by Zed Industries.",
                crate::external_url::open_gpui,
                "Could Not Open GPUI",
            ))
            .child(linked_notice_line(
                "about-gpui-component",
                "gpui-component",
                " is licensed under Apache 2.0.",
                crate::external_url::open_gpui_component,
                "Could Not Open gpui-component",
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
                        "Could Not Open GitHub",
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
        " Free 7.3.1 icons are licensed under CC BY 4.0.",
        crate::external_url::open_font_awesome,
        "Could Not Open Font Awesome",
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

#[cfg(test)]
mod tests {
    #[test]
    fn about_panel_shows_identity_and_font_awesome_notice() {
        let source = include_str!("about_panel.rs");
        assert!(source.contains("CARGO_PKG_VERSION"));
        assert!(source.contains("ralgruM"));
        assert!(source.contains("kekkodance"));
        assert!(source.contains("github.com/kekkodance"));
        assert!(source.contains("LocalIcon::Music"));
        assert!(source.contains("LocalIcon::GitHub"));
        assert!(source.contains("Special Thanks"));
        assert!(source.contains("BadManners Team"));
        assert!(source.contains("Furby"));
        assert!(source.contains("Sigma"));
        assert!(source.contains("Font Awesome"));
        assert!(source.contains("open_font_awesome"));
        assert!(source.contains("Chromium"));
        assert!(source.contains("about-chromium"));
        assert!(source.contains("open_chromium"));
        assert!(source.contains("BSD 3-Clause License"));
        assert!(source.contains("GPUI"));
        assert!(source.contains("about-gpui"));
        assert!(source.contains("open_gpui"));
        assert!(source.contains("Zed Industries"));
        assert!(source.contains("gpui-component"));
        assert!(source.contains("about-gpui-component"));
        assert!(source.contains("open_gpui_component"));
        assert!(source.contains("Apache 2.0"));
        assert!(!source.contains(concat!("Luc", "ide")));
        assert!(source.contains("ABOUT_PANEL_TOP_PADDING_PX"));
        assert!(source.contains("HEADING_ICON_OPTICAL_OFFSET_PX"));
        assert!(!source.contains(concat!("Murglar develop", "ers")));
        assert!(!source.contains(concat!("app-icon", ".png")));
        assert!(!source.contains(concat!("Auth", "or")));
        assert!(!source.contains(concat!(".child(\"", "About\")")));
        assert!(!source.contains(concat!("THIRD_PARTY", "_NOTICES")));
        assert!(!source.contains(concat!("full ", "notice")));
    }

    #[test]
    fn linked_notice_text_is_a_direct_baseline_child() {
        let source = include_str!("about_panel.rs");
        let linked_notice = source
            .split_once("fn linked_notice_line(")
            .and_then(|(_, rest)| rest.split_once("fn card_heading("))
            .map(|(body, _)| body)
            .expect("linked notice implementation should exist");

        assert!(linked_notice.contains(".items_baseline()"));
        assert!(linked_notice.contains(".child(\n            div()\n                .id(id)"));
        assert!(!linked_notice.contains("div().child(\n                div()"));
    }
}
