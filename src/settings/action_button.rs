use crate::{
    app_button::{
        DANGER_SECONDARY_DISABLED_TEXT, DANGER_SECONDARY_HEIGHT,
        DANGER_SECONDARY_HORIZONTAL_PADDING, DANGER_SECONDARY_HOVER_BACKGROUND,
        DANGER_SECONDARY_HOVER_BORDER, DANGER_SECONDARY_HOVER_TEXT,
        DANGER_SECONDARY_NORMAL_BACKGROUND, DANGER_SECONDARY_NORMAL_BORDER,
        DANGER_SECONDARY_NORMAL_TEXT, DANGER_SECONDARY_RADIUS, DANGER_SECONDARY_SERVICE_HEIGHT,
        SETTINGS_SECONDARY_HORIZONTAL_PADDING, SETTINGS_SECONDARY_ICON_SIZE,
        SETTINGS_SECONDARY_MIN_HEIGHT, SETTINGS_SECONDARY_SMALL_ICON_SIZE,
        SETTINGS_SECONDARY_VERTICAL_PADDING,
    },
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    theme::{BORDER, FOREGROUND, MUTED},
};
use gpui::{
    App, ClickEvent, CursorStyle, Div, ElementId, FontWeight, Role, Stateful, Window, div,
    prelude::*, px, rgb, rgba,
};

#[derive(Clone, Copy)]
pub(crate) struct DangerSecondaryButtonOptions {
    pub(crate) height: f32,
    pub(crate) full_width: bool,
    pub(crate) icon_only: bool,
}

impl DangerSecondaryButtonOptions {
    pub(crate) const SIDEBAR: Self = Self {
        height: DANGER_SECONDARY_HEIGHT,
        full_width: true,
        icon_only: false,
    };

    pub(crate) const SERVICE: Self = Self {
        height: DANGER_SECONDARY_SERVICE_HEIGHT,
        full_width: true,
        icon_only: false,
    };

    pub(crate) const SIDEBAR_COMPACT: Self = Self {
        height: 36.,
        full_width: false,
        icon_only: true,
    };
}

pub(crate) fn danger_secondary_button(
    id: impl Into<ElementId>,
    label: &'static str,
    disabled: bool,
    options: DangerSecondaryButtonOptions,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let text_color = if disabled {
        DANGER_SECONDARY_DISABLED_TEXT
    } else {
        DANGER_SECONDARY_NORMAL_TEXT
    };
    let control = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .h(px(options.height))
        .when(options.icon_only, |this| this.w(px(options.height)))
        .when(options.full_width, |this| this.w_full())
        .when(!options.icon_only, |this| {
            this.px(px(DANGER_SECONDARY_HORIZONTAL_PADDING))
        })
        .flex()
        .items_center()
        .justify_center()
        .when(!options.icon_only, |this| this.gap(px(7.)))
        .when(!disabled, |this| this.group(label))
        .rounded(px(DANGER_SECONDARY_RADIUS))
        .border_1()
        .border_color(rgba(DANGER_SECONDARY_NORMAL_BORDER))
        .bg(rgba(DANGER_SECONDARY_NORMAL_BACKGROUND))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(text_color))
        .child(
            div()
                .relative()
                .size(px(13.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(label, |style| style.invisible())
                        .child(local_icon(LocalIcon::LogOut, text_color).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(label, |style| style.visible())
                        .child(
                            local_icon(LocalIcon::LogOut, DANGER_SECONDARY_HOVER_TEXT).size_full(),
                        ),
                ),
        );
    let control = control.when(!options.icon_only, |this| this.child(label));

    if disabled {
        control
            .opacity(0.62)
            .cursor(CursorStyle::OperationNotAllowed)
    } else {
        control
            .focusable()
            .tab_stop(true)
            .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
            .cursor_pointer()
            .hover(|style| {
                style
                    .text_color(rgb(DANGER_SECONDARY_HOVER_TEXT))
                    .border_color(rgba(DANGER_SECONDARY_HOVER_BORDER))
                    .bg(rgba(DANGER_SECONDARY_HOVER_BACKGROUND))
            })
            .on_click(on_click)
    }
}

pub(crate) fn neutral_secondary_button(
    id: impl Into<ElementId>,
    icon: LocalIcon,
    label: &'static str,
    options: DangerSecondaryButtonOptions,
    icon_size: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .h(px(options.height))
        .when(options.icon_only, |this| this.w(px(options.height)))
        .when(options.full_width, |this| this.w_full())
        .when(!options.icon_only, |this| {
            this.px(px(DANGER_SECONDARY_HORIZONTAL_PADDING))
        })
        .flex()
        .items_center()
        .justify_center()
        .when(!options.icon_only, |this| this.gap(px(7.)))
        .group(label)
        .rounded(px(DANGER_SECONDARY_RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgba(0x00000000))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .focusable()
        .tab_stop(true)
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .hover(|style| {
            style
                .text_color(rgb(FOREGROUND))
                .border_color(rgb(BORDER))
                .bg(rgb(BORDER))
        })
        .child(
            div()
                .relative()
                .size(px(13.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .group_hover(label, |style| style.invisible())
                        .child(local_icon(icon, MUTED).size(px(icon_size))),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .invisible()
                        .group_hover(label, |style| style.visible())
                        .child(local_icon(icon, FOREGROUND).size(px(icon_size))),
                ),
        )
        .when(!options.icon_only, |this| this.child(label))
        .on_click(on_click)
}

pub(super) fn secondary_action_button(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: &'static str,
    tooltip: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_action_button_with_icon_size(
        id,
        icon,
        label,
        tooltip,
        SETTINGS_SECONDARY_ICON_SIZE,
        on_click,
    )
}

pub(super) fn secondary_action_button_small_icon(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: &'static str,
    tooltip: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_action_button_with_icon_size(
        id,
        icon,
        label,
        tooltip,
        SETTINGS_SECONDARY_SMALL_ICON_SIZE,
        on_click,
    )
}

fn secondary_action_button_with_icon_size(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: &'static str,
    tooltip: &'static str,
    icon_size: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label)
        .min_h(px(SETTINGS_SECONDARY_MIN_HEIGHT))
        .flex_none()
        .py(px(SETTINGS_SECONDARY_VERTICAL_PADDING))
        .px(px(SETTINGS_SECONDARY_HORIZONTAL_PADDING))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .rounded(px(DANGER_SECONDARY_RADIUS))
        .border_1()
        .border_color(rgb(crate::theme::BORDER))
        .bg(rgba(0x00000000))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(crate::theme::FOREGROUND))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(crate::theme::BORDER)))
        .app_tooltip(tooltip)
        .when_some(icon, |this, icon| {
            this.child(local_icon(icon, crate::theme::FOREGROUND).size(px(icon_size)))
        })
        .child(label)
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::{
        DANGER_SECONDARY_HEIGHT, DANGER_SECONDARY_HORIZONTAL_PADDING,
        DANGER_SECONDARY_HOVER_BACKGROUND, DANGER_SECONDARY_HOVER_BORDER,
        DANGER_SECONDARY_HOVER_TEXT, DANGER_SECONDARY_NORMAL_BACKGROUND,
        DANGER_SECONDARY_NORMAL_BORDER, DANGER_SECONDARY_NORMAL_TEXT, DANGER_SECONDARY_RADIUS,
        DANGER_SECONDARY_SERVICE_HEIGHT, SETTINGS_SECONDARY_HORIZONTAL_PADDING,
        SETTINGS_SECONDARY_ICON_SIZE, SETTINGS_SECONDARY_MIN_HEIGHT,
        SETTINGS_SECONDARY_SMALL_ICON_SIZE, SETTINGS_SECONDARY_VERTICAL_PADDING,
    };

    #[test]
    fn danger_secondary_contract_matches_reference() {
        assert_eq!(DANGER_SECONDARY_NORMAL_TEXT, 0xfca5a5);
        assert_eq!(DANGER_SECONDARY_NORMAL_BORDER, 0xef44444d);
        assert_eq!(DANGER_SECONDARY_NORMAL_BACKGROUND, 0x00000000);
        assert_eq!(DANGER_SECONDARY_HOVER_TEXT, 0xfecaca);
        assert_eq!(DANGER_SECONDARY_HOVER_BORDER, 0xef44448c);
        assert_eq!(DANGER_SECONDARY_HOVER_BACKGROUND, 0xef444424);
        assert_eq!(DANGER_SECONDARY_HEIGHT, 34.);
        assert_eq!(DANGER_SECONDARY_SERVICE_HEIGHT, 40.);
        assert_eq!(DANGER_SECONDARY_HORIZONTAL_PADDING, 16.);
        assert_eq!(DANGER_SECONDARY_RADIUS, 6.);
        assert_eq!(SETTINGS_SECONDARY_MIN_HEIGHT, 32.);
        assert_eq!(SETTINGS_SECONDARY_VERTICAL_PADDING, 6.);
        assert_eq!(SETTINGS_SECONDARY_HORIZONTAL_PADDING, 10.);
        assert_eq!(SETTINGS_SECONDARY_ICON_SIZE, 13.);
        assert_eq!(SETTINGS_SECONDARY_SMALL_ICON_SIZE, 11.);
    }

    #[test]
    fn compact_sidebar_action_is_icon_only() {
        let options = super::DangerSecondaryButtonOptions::SIDEBAR_COMPACT;

        assert_eq!(options.height, 36.);
        assert!(!options.full_width);
        assert!(options.icon_only);
    }

    #[test]
    fn neutral_sidebar_action_matches_logout_dimensions() {
        let expanded = super::DangerSecondaryButtonOptions::SIDEBAR;
        let compact = super::DangerSecondaryButtonOptions::SIDEBAR_COMPACT;

        assert_eq!(expanded.height, DANGER_SECONDARY_HEIGHT);
        assert!(expanded.full_width);
        assert!(!expanded.icon_only);
        assert_eq!(compact.height, 36.);
        assert!(!compact.full_width);
        assert!(compact.icon_only);
    }
}
