use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    theme::{BORDER, CHROME, FOREGROUND, MUTED, PRIMARY, SURFACE_RAISED},
};
use gpui::{
    ClickEvent, Context, IntoElement, Role, Window, WindowControlArea, div, prelude::*, px, rgb,
};

use super::RalgrumApp;

pub(super) fn render_titlebar(
    _app: &RalgrumApp,
    window: &Window,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement {
    let (maximize_icon, maximize_label) = if window.is_maximized() {
        (LocalIcon::WindowRestore, "Restore")
    } else {
        (LocalIcon::WindowMaximize, "Maximize")
    };
    div()
        .h(px(32.))
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(
            div()
                .h_full()
                .flex_1()
                .flex()
                .items_center()
                .px(px(11.))
                .gap(px(7.))
                .window_control_area(WindowControlArea::Drag)
                .text_size(px(11.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(0xd4d4d8))
                .when(f32::from(window.viewport_size().width) <= 768., |this| {
                    this.child(
                        div()
                            .id("mobile-sidebar-toggle")
                            .focusable()
                            .tab_stop(true)
                            .role(Role::Button)
                            .aria_label("Toggle navigation")
                            .size(px(32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_RAISED)))
                            .focus_visible(|style| {
                                style.border_1().border_color(rgb(crate::theme::PRIMARY))
                            })
                            .app_tooltip("Toggle navigation")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.toggle_mobile_sidebar(cx);
                            }))
                            .child(local_icon(LocalIcon::List, MUTED).size(px(15.))),
                    )
                })
                .child(local_icon(LocalIcon::Music, PRIMARY).size(px(10.)))
                .child("ralgruM"),
        )
        .child(
            div()
                .h_full()
                .flex()
                .child(title_control(
                    LocalIcon::Minus,
                    "Minimize",
                    WindowControlArea::Min,
                ))
                .child(title_control(
                    maximize_icon,
                    maximize_label,
                    WindowControlArea::Max,
                ))
                .child(title_control(
                    LocalIcon::X,
                    "Close",
                    WindowControlArea::Close,
                )),
        )
}

fn title_control(
    icon: LocalIcon,
    label: &'static str,
    area: WindowControlArea,
) -> impl IntoElement {
    let close = area == WindowControlArea::Close;

    div()
        .id(label)
        .group(label)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label)
        .w(px(46.))
        .h(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(if close { px(12.) } else { px(11.) })
        .text_color(rgb(MUTED))
        .window_control_area(area)
        .hover(move |style| {
            style
                .bg(rgb(if close { 0xc42b1c } else { SURFACE_RAISED }))
                .text_color(rgb(if close { 0xffffff } else { FOREGROUND }))
        })
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .app_tooltip(label)
        .on_click(move |event, window, _| {
            if !matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }

            match area {
                WindowControlArea::Min => window.minimize_window(),
                WindowControlArea::Max => window.zoom_window(),
                WindowControlArea::Close => window.remove_window(),
                _ => {}
            }
        })
        .child(
            div()
                .size(if close { px(10.5) } else { px(11.) })
                .relative()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(label, |style| style.invisible())
                        .child(local_icon(icon, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(label, |style| style.visible())
                        .child(
                            local_icon(icon, if close { 0xffffff } else { FOREGROUND }).size_full(),
                        ),
                ),
        )
}
