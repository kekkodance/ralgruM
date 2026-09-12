use gpui::{
    App, ClickEvent, CursorStyle, Div, ElementId, FontWeight, Role, SharedString, Stateful, Window,
    div, prelude::*, px, rgb, rgba,
};
use gpui_component::{Sizable, Size, spinner::Spinner};

use crate::{
    assets::{LocalIcon, local_icon, widget_icon},
    theme::{BORDER, FOREGROUND, PRIMARY},
};

/// The palette used by destructive secondary actions throughout the app.
///
/// Keeping these values at the app level means custom controls such as toasts
/// and the settings sidebar can share the same visual contract as the logout
/// action without copying color literals into each view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ButtonPalette {
    pub(crate) normal_text: u32,
    pub(crate) normal_border: u32,
    pub(crate) normal_background: u32,
    pub(crate) hover_text: u32,
    pub(crate) hover_border: u32,
    pub(crate) hover_background: u32,
}

pub(crate) const DANGER_SECONDARY_PALETTE: ButtonPalette = ButtonPalette {
    normal_text: 0xfca5a5,
    normal_border: 0xef44444d,
    normal_background: 0x00000000,
    hover_text: 0xfecaca,
    hover_border: 0xef44448c,
    hover_background: 0xef444424,
};

pub(crate) const DANGER_SECONDARY_NORMAL_TEXT: u32 = DANGER_SECONDARY_PALETTE.normal_text;
pub(crate) const DANGER_SECONDARY_NORMAL_BORDER: u32 = DANGER_SECONDARY_PALETTE.normal_border;
pub(crate) const DANGER_SECONDARY_NORMAL_BACKGROUND: u32 =
    DANGER_SECONDARY_PALETTE.normal_background;
pub(crate) const DANGER_SECONDARY_HOVER_TEXT: u32 = DANGER_SECONDARY_PALETTE.hover_text;
pub(crate) const DANGER_SECONDARY_HOVER_BORDER: u32 = DANGER_SECONDARY_PALETTE.hover_border;
pub(crate) const DANGER_SECONDARY_HOVER_BACKGROUND: u32 = DANGER_SECONDARY_PALETTE.hover_background;
pub(crate) const DANGER_SECONDARY_DISABLED_TEXT: u32 = 0x52525b;
pub(crate) const DANGER_SECONDARY_HEIGHT: f32 = 34.;
pub(crate) const DANGER_SECONDARY_SERVICE_HEIGHT: f32 = 40.;
pub(crate) const DANGER_SECONDARY_HORIZONTAL_PADDING: f32 = 16.;
pub(crate) const DANGER_SECONDARY_RADIUS: f32 = 6.;

pub(crate) const SETTINGS_SECONDARY_MIN_HEIGHT: f32 = 32.;
pub(crate) const SETTINGS_SECONDARY_VERTICAL_PADDING: f32 = 6.;
pub(crate) const SETTINGS_SECONDARY_HORIZONTAL_PADDING: f32 = 10.;
pub(crate) const SETTINGS_SECONDARY_ICON_SIZE: f32 = 13.;
pub(crate) const SETTINGS_SECONDARY_SMALL_ICON_SIZE: f32 = 11.;

pub(crate) const PRIMARY_BUTTON_BACKGROUND: u32 = 0x6366f1ff;
pub(crate) const PRIMARY_BUTTON_FOREGROUND: u32 = 0xfafafa;
pub(crate) const PRIMARY_BUTTON_BORDER: u32 = 0x818cf8c2;
pub(crate) const PRIMARY_BUTTON_HOVER_BACKGROUND: u32 = 0x5558e8ff;
pub(crate) const PRIMARY_BUTTON_HOVER_BORDER: u32 = 0xa5b4fcff;
pub(crate) const PRIMARY_BUTTON_HEIGHT: f32 = 34.;
pub(crate) const PRIMARY_BUTTON_HORIZONTAL_PADDING: f32 = 13.;
pub(crate) const PRIMARY_BUTTON_ICON_GAP: f32 = 7.;
pub(crate) const PRIMARY_BUTTON_ICON_SIZE: f32 = 13.;
pub(crate) const PRIMARY_BUTTON_RADIUS: f32 = 6.;

pub(crate) const PLAIN_X_HIT_SIZE: f32 = 24.;
pub(crate) const PLAIN_X_ICON_SIZE: f32 = 9.;

pub(crate) const SECONDARY_PAGE_ACTION_HEIGHT: f32 = 32.;
pub(crate) const SECONDARY_PAGE_ACTION_HORIZONTAL_PADDING: f32 = 11.;
pub(crate) const SECONDARY_PAGE_ACTION_RADIUS: f32 = 6.;
const SECONDARY_PAGE_ACTION_PALETTE: ButtonPalette = ButtonPalette {
    normal_text: FOREGROUND,
    normal_border: BORDER,
    normal_background: 0x00000000,
    hover_text: FOREGROUND,
    hover_border: BORDER,
    hover_background: BORDER,
};

fn palette_color(color: u32) -> gpui::Rgba {
    if color == 0 || color > 0x00ff_ffff {
        rgba(color)
    } else {
        rgb(color)
    }
}

/// Build a compact X control whose pointer feedback changes only the glyph.
/// The transparent border reserves space for the shared keyboard focus ring.
pub(crate) fn plain_x_button(
    id: impl Into<ElementId>,
    group: impl Into<SharedString>,
    aria_label: impl Into<SharedString>,
    tab_stop: bool,
) -> Stateful<Div> {
    let group = group.into();
    div()
        .id(id)
        .group(group.clone())
        .role(Role::Button)
        .aria_label(aria_label.into())
        .size(px(PLAIN_X_HIT_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor(CursorStyle::PointingHand)
        .when(tab_stop, |this| {
            this.focusable()
                .tab_stop(true)
                .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        })
        .child(
            div()
                .relative()
                .size(px(PLAIN_X_ICON_SIZE))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(group.clone(), |style| style.invisible())
                        .child(local_icon(LocalIcon::X, crate::theme::MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(local_icon(LocalIcon::X, FOREGROUND).size_full()),
                ),
        )
}
/// Build a normal app action button with the shared component sizing and
/// optional local icon. Callers should use this helper for page-level primary
/// actions so icon spacing and the primary variant stay consistent.
pub(crate) fn primary_button(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    primary_button_with_loading(id, icon, label, false, false, on_click)
}

pub(crate) fn primary_button_with_disabled(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    primary_button_with_loading(id, icon, label, disabled, false, on_click)
}

/// Build a primary action that can show an inline loading spinner while it is
/// inert. Loading is treated as disabled for pointer and keyboard interaction,
/// while retaining the spinner so the user gets immediate feedback.
pub(crate) fn primary_button_with_loading(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    loading: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    primary_button_with_dimensions(
        id,
        icon,
        label,
        PRIMARY_BUTTON_HEIGHT,
        PRIMARY_BUTTON_HORIZONTAL_PADDING,
        primary_button_is_disabled(disabled, loading),
        loading,
        on_click,
    )
}

pub(crate) fn danger_secondary_button_with_loading(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    loading: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    let is_disabled = disabled || loading;
    let text_color = if is_disabled {
        DANGER_SECONDARY_DISABLED_TEXT
    } else {
        DANGER_SECONDARY_PALETTE.normal_text
    };
    div()
        .id(id)
        .focusable()
        .tab_stop(!is_disabled)
        .role(Role::Button)
        .aria_label(label.clone())
        .h(px(DANGER_SECONDARY_HEIGHT))
        .flex_none()
        .px(px(DANGER_SECONDARY_HORIZONTAL_PADDING))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .rounded(px(DANGER_SECONDARY_RADIUS))
        .border_1()
        .border_color(rgba(DANGER_SECONDARY_PALETTE.normal_border))
        .bg(rgba(DANGER_SECONDARY_PALETTE.normal_background))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(text_color))
        .whitespace_nowrap()
        .when(!is_disabled, |this| {
            this.cursor(CursorStyle::PointingHand)
                .hover(|style| {
                    style
                        .text_color(rgb(DANGER_SECONDARY_PALETTE.hover_text))
                        .border_color(rgba(DANGER_SECONDARY_PALETTE.hover_border))
                        .bg(rgba(DANGER_SECONDARY_PALETTE.hover_background))
                })
                .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
        })
        .when(is_disabled, |this| {
            this.opacity(0.62).cursor(CursorStyle::OperationNotAllowed)
        })
        .when(loading, |this| {
            this.child(
                Spinner::new()
                    .icon(widget_icon(LocalIcon::Spinner))
                    .with_size(Size::Size(px(13.)))
                    .color(rgb(text_color).into()),
            )
        })
        .when(!loading, |this| {
            this.when_some(icon, |this, icon| {
                this.child(
                    div()
                        .relative()
                        .size(px(PRIMARY_BUTTON_ICON_SIZE))
                        .when(
                            matches!(icon, LocalIcon::Download | LocalIcon::Upload),
                            |this| this.top(px(0.5)),
                        )
                        .child(local_icon(icon, text_color).size_full()),
                )
            })
        })
        .child(label)
        .when(!is_disabled, |this| this.on_click(on_click))
}

fn primary_button_is_disabled(disabled: bool, loading: bool) -> bool {
    disabled || loading
}

pub(crate) fn secondary_page_action_button_with_disabled(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_page_action_button_with_palette(
        id,
        icon,
        label,
        disabled,
        SECONDARY_PAGE_ACTION_PALETTE,
        on_click,
    )
}

fn secondary_page_action_button_with_palette(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    palette: ButtonPalette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .focusable()
        .tab_stop(!disabled)
        .role(Role::Button)
        .aria_label(label.clone())
        .h(px(SECONDARY_PAGE_ACTION_HEIGHT))
        .flex_none()
        .px(px(SECONDARY_PAGE_ACTION_HORIZONTAL_PADDING))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(PRIMARY_BUTTON_ICON_GAP))
        .rounded(px(SECONDARY_PAGE_ACTION_RADIUS))
        .border_1()
        .border_color(palette_color(palette.normal_border))
        .bg(palette_color(palette.normal_background))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(palette.normal_text))
        .whitespace_nowrap()
        .when(!disabled, |this| {
            this.cursor(CursorStyle::PointingHand)
                .hover(|style| {
                    style
                        .border_color(palette_color(palette.hover_border))
                        .bg(palette_color(palette.hover_background))
                        .text_color(rgb(palette.hover_text))
                })
                .active(|style| {
                    style
                        .border_color(palette_color(palette.hover_border))
                        .bg(palette_color(palette.hover_background))
                        .text_color(rgb(palette.hover_text))
                })
                .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
        })
        .when(disabled, |this| {
            this.opacity(0.5).cursor(CursorStyle::Arrow)
        })
        .when_some(icon, |this, icon| {
            this.child(
                div()
                    .relative()
                    .size(px(PRIMARY_BUTTON_ICON_SIZE))
                    .when(
                        matches!(icon, LocalIcon::Download | LocalIcon::Upload),
                        |this| this.top(px(0.5)),
                    )
                    .child(local_icon(icon, palette.normal_text).size_full()),
            )
        })
        .child(label)
        .when(!disabled, |this| this.on_click(on_click))
}

/// Build a secondary dialog action that shares the primary action's height.
/// Dialog footers use this so adjacent primary and secondary actions align.
pub(crate) fn secondary_dialog_button_with_disabled(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_page_action_button_with_disabled(id, icon, label, disabled, on_click)
        .h(px(PRIMARY_BUTTON_HEIGHT))
}

/// Build a destructive secondary dialog action with the same geometry as the
/// neutral dialog action and the shared danger palette.
pub(crate) fn danger_secondary_dialog_button_with_disabled(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    secondary_page_action_button_with_palette(
        id,
        icon,
        label,
        disabled,
        DANGER_SECONDARY_PALETTE,
        on_click,
    )
    .h(px(PRIMARY_BUTTON_HEIGHT))
}

fn primary_button_with_dimensions(
    id: impl Into<ElementId>,
    icon: Option<LocalIcon>,
    label: impl Into<SharedString>,
    height: f32,
    horizontal_padding: f32,
    disabled: bool,
    loading: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .focusable()
        .tab_stop(!disabled)
        .role(Role::Button)
        .aria_label(label.clone())
        .h(px(height))
        .flex_none()
        .px(px(horizontal_padding))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(PRIMARY_BUTTON_ICON_GAP))
        .rounded(px(PRIMARY_BUTTON_RADIUS))
        .border_1()
        .border_color(rgba(PRIMARY_BUTTON_BORDER))
        .bg(rgba(PRIMARY_BUTTON_BACKGROUND))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(PRIMARY_BUTTON_FOREGROUND))
        .whitespace_nowrap()
        .when(!disabled, |this| {
            this.cursor(CursorStyle::PointingHand)
                .hover(|style| {
                    style
                        .border_color(rgba(PRIMARY_BUTTON_HOVER_BORDER))
                        .bg(rgba(PRIMARY_BUTTON_HOVER_BACKGROUND))
                })
                .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
        })
        .when(disabled, |this| {
            this.opacity(0.5).cursor(CursorStyle::Arrow)
        })
        .when(loading, |this| {
            this.child(
                Spinner::new()
                    .icon(widget_icon(LocalIcon::Spinner))
                    .with_size(Size::Size(px(PRIMARY_BUTTON_ICON_SIZE)))
                    .color(rgb(PRIMARY_BUTTON_FOREGROUND).into()),
            )
        })
        .when(!loading, |this| {
            this.when_some(icon, |this, icon| {
                this.child(
                    local_icon(icon, PRIMARY_BUTTON_FOREGROUND).size(px(PRIMARY_BUTTON_ICON_SIZE)),
                )
            })
        })
        .child(label)
        .when(!disabled, |this| this.on_click(on_click))
}

#[cfg(test)]
mod tests;
