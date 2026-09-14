use super::service_panel::settings_card;
use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon, widget_icon},
    murglar_backend::ReferralStats,
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED, PRIMARY},
};
use gpui::{
    App, Div, FontWeight, IntoElement, Role, Stateful, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::{Sizable, Size, spinner::Spinner};

pub(super) fn card(
    stats: &ReferralStats,
    reloading: bool,
    copy_action: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    reload_action: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let code = stats.referral_code.trim().to_owned();
    settings_card()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(local_icon(LocalIcon::ShareNodes, FOREGROUND).size(px(13.)))
                        .child("Referral rewards"),
                )
                .child(reload_button(reloading, reload_action)),
        )
        .child(
            div()
                .text_size(px(11.5))
                .line_height(px(17.25))
                .text_color(rgb(MUTED))
                .child("Share your referral code and track the reward days credited by Murglar."),
        )
        .child(referral_code_row(code, copy_action))
        .child(referral_stat_grid(stats))
}

fn reload_button(
    reloading: bool,
    reload_action: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id("reload-murglar-referral")
        .focusable()
        .tab_stop(!reloading)
        .role(Role::Button)
        .aria_label("Reload referral rewards")
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .bg(rgba(0x00000000))
        .text_color(rgb(MUTED))
        .app_tooltip("Reload referral rewards")
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .when(reloading, |this| {
            this.child(
                Spinner::new()
                    .icon(widget_icon(LocalIcon::Spinner))
                    .with_size(Size::Size(px(13.)))
                    .color(rgb(MUTED).into()),
            )
        })
        .when(!reloading, |this| {
            this.cursor_pointer()
                .hover(|style| {
                    style
                        .bg(rgb(BORDER))
                        .border_color(rgb(BORDER))
                        .text_color(rgb(FOREGROUND))
                })
                .child(local_icon(LocalIcon::RotateRight, MUTED).size(px(13.)))
                .on_click(reload_action)
        })
}

fn referral_code_row(
    code: String,
    copy_action: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .h(px(38.))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .overflow_hidden()
                .px(px(11.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(BACKGROUND))
                .text_color(rgb(0xc7d2fe))
                .text_size(px(12.))
                .font_family("Consolas")
                .truncate()
                .child(display_code(&code)),
        )
        .when(has_code(&code), |this| this.child(copy_button(copy_action)))
}

fn copy_button(
    copy_action: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id("copy-murglar-referral-code")
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("Copy referral code")
        .size(px(38.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(BACKGROUND))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BORDER)).text_color(rgb(FOREGROUND)))
        .app_tooltip("Copy referral code")
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(local_icon(LocalIcon::Copy, MUTED).size(px(14.)))
        .on_click(copy_action)
}

fn referral_stat_grid(stats: &ReferralStats) -> Div {
    div().flex().gap(px(8.)).children([
        stat_cell("Reward days", stats.reward_days.to_string()),
        stat_cell("Invitees", stats.invitee_count.to_string()),
        stat_cell("Rewards", stats.reward_count.to_string()),
    ])
}

fn stat_cell(label: &'static str, value: String) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(3.))
        .py(px(10.))
        .px(px(8.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgba(0x09090b7a))
        .rounded(px(6.))
        .child(
            div()
                .text_size(px(16.))
                .text_color(rgb(FOREGROUND))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(
            div()
                .max_w_full()
                .text_size(px(10.5))
                .text_color(rgb(MUTED))
                .truncate()
                .child(label),
        )
}

fn has_code(code: &str) -> bool {
    !code.trim().is_empty()
}

fn display_code(value: &str) -> String {
    if value.trim().is_empty() {
        "N/A".to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::has_code;

    #[test]
    fn copy_action_requires_nonempty_referral_code() {
        assert!(has_code("JOIN-ME"));
        assert!(!has_code(""));
        assert!(!has_code("  \t"));
    }
}
