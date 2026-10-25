use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, app_mark, local_icon},
    theme::{BORDER, CHROME, FOREGROUND, MUTED, SURFACE_RAISED},
};
use gpui::{
    ClickEvent, Context, IntoElement, MouseButton, Role, Window, WindowControlArea, div,
    prelude::*, px, rgb,
};
use gpui_component::InteractiveElementExt as _;

use super::RalgrumApp;

#[derive(Default)]
struct TitlebarDragState {
    should_move: bool,
}

pub(super) fn render_titlebar(
    _app: &RalgrumApp,
    window: &mut Window,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement {
    let (maximize_icon, maximize_label) = if window.is_maximized() {
        (LocalIcon::WindowRestore, "Restore")
    } else {
        (LocalIcon::WindowMaximize, "Maximize")
    };
    let drag_state = window.use_state(cx, |_, _| TitlebarDragState::default());
    let linux = cfg!(target_os = "linux");
    div()
        .h(px(32.))
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .when(linux, |this| {
            this.on_mouse_down_out(window.listener_for(&drag_state, |state, _, _, _| {
                state.should_move = false;
            }))
            .on_mouse_down(
                MouseButton::Left,
                window.listener_for(&drag_state, |state, _, _, _| {
                    state.should_move = true;
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                window.listener_for(&drag_state, |state, _, _, _| {
                    state.should_move = false;
                }),
            )
            .on_mouse_move(window.listener_for(
                &drag_state,
                |state, _, window, _| {
                    if state.should_move {
                        state.should_move = false;
                        window.start_window_move();
                    }
                },
            ))
        })
        .child(
            div()
                .id("main-titlebar-drag-region")
                .h_full()
                .flex_1()
                .flex()
                .items_center()
                .px(px(11.))
                .gap(px(7.))
                .window_control_area(WindowControlArea::Drag)
                .when(linux, |this| {
                    this.on_double_click(|_, window, _| window.zoom_window())
                        .on_mouse_down(MouseButton::Right, |event, window, _| {
                            window.show_window_menu(event.position)
                        })
                })
                .text_size(px(11.5))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(rgb(0xd4d4d8))
                .when(
                    crate::music_ui::window_content_width(window) <= 768.,
                    |this| {
                        this.child(
                            div()
                                .id("mobile-sidebar-toggle")
                                .focusable()
                                .tab_stop(true)
                                .role(Role::Button)
                                .aria_label(crate::i18n::text("Toggle navigation"))
                                .size(px(32.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_RAISED)))
                                .focus_visible(|style| {
                                    style.border_1().border_color(rgb(crate::theme::PRIMARY))
                                })
                                .app_tooltip(crate::i18n::text("Toggle navigation"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.toggle_mobile_sidebar(cx);
                                }))
                                .child(local_icon(LocalIcon::List, MUTED).size(px(15.))),
                        )
                    },
                )
                .child(app_mark().size(px(12.)).relative().top(px(0.5)))
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
                    cx,
                ))
                .child(title_control(
                    maximize_icon,
                    maximize_label,
                    WindowControlArea::Max,
                    cx,
                ))
                .child(title_control(
                    LocalIcon::X,
                    "Close",
                    WindowControlArea::Close,
                    cx,
                )),
        )
}

fn title_control(
    icon: LocalIcon,
    label: &'static str,
    area: WindowControlArea,
    cx: &Context<RalgrumApp>,
) -> impl IntoElement {
    let close = area == WindowControlArea::Close;
    let group_id = label;
    let label = crate::i18n::text(label);

    div()
        .id(group_id)
        .group(group_id)
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
        .when(cfg!(target_os = "linux"), |this| {
            this.on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
        })
        .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
            if cfg!(target_os = "linux") {
                cx.stop_propagation();
            } else if !matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            match area {
                WindowControlArea::Min => window.minimize_window(),
                WindowControlArea::Max => window.zoom_window(),
                WindowControlArea::Close => app.request_close(window, cx),
                _ => {}
            }
        }))
        .child(
            div()
                .size(if close { px(10.5) } else { px(11.) })
                .relative()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(group_id, |style| style.invisible())
                        .child(local_icon(icon, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(group_id, |style| style.visible())
                        .child(
                            local_icon(icon, if close { 0xffffff } else { FOREGROUND }).size_full(),
                        ),
                ),
        )
}
