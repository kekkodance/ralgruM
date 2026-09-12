use gpui::{
    Anchor, AnchoredPositionMode, Bounds, Context, Edges, ElementId, InteractiveElement,
    IntoElement, ParentElement, PathBuilder, Role, StatefulInteractiveElement, Styled, Window,
    anchored, canvas, deferred, div, point, prelude::FluentBuilder, px, rgb, rgba,
};
use gpui_component::{
    ActiveTheme, ElementExt, Icon,
    scroll::{Scrollbar, ScrollbarShow},
    v_flex,
};

use crate::browser_scroll::{BrowserScrollTarget, browser_scroll_overlays};

use super::{
    CONTEXT_MENU_BORDER, CONTEXT_MENU_FOREGROUND, CONTEXT_MENU_HOVER,
    CONTEXT_MENU_HOVER_FOREGROUND, CONTEXT_MENU_SEPARATOR, CONTEXT_MENU_SURFACE, PopupMenu,
    PopupMenuArrowEdge, PopupMenuItem,
    palette::{
        CONTEXT_MENU_ACTION_COLUMN_GAP, CONTEXT_MENU_ACTION_HORIZONTAL_PADDING,
        CONTEXT_MENU_ACTION_ROW_HEIGHT, CONTEXT_MENU_ACTION_VERTICAL_PADDING,
        CONTEXT_MENU_ICON_COLUMN_WIDTH, CONTEXT_MENU_ICON_SIZE, CONTEXT_MENU_ITEM_GAP,
        CONTEXT_MENU_LABEL_SIZE, CONTEXT_MENU_POPUP_PADDING, CONTEXT_MENU_SHORTCUT_SIZE,
        TEXT_FIELD_CONTEXT_MENU_DISABLED, TEXT_FIELD_CONTEXT_MENU_DISABLED_OPACITY,
        TEXT_FIELD_CONTEXT_MENU_ICON, TEXT_FIELD_CONTEXT_MENU_SELECTED_SHORTCUT,
    },
    submenu::SUBMENU_GAP,
};

// The anchored child is laid out after its trigger row, so move it back to
// the trigger's top before applying the viewport fit calculation.
const SUBMENU_LOCAL_Y_OFFSET: f32 = -CONTEXT_MENU_ACTION_ROW_HEIGHT;
const DEFAULT_ACTION_ROW_WIDTH: f32 = 272. - 2. - 12.;
const MENU_RADIUS: f32 = 8.;
// Chevron for anchored menus. Matches the tooltip arrow shape (7px diamond,
// outward V) so the arrow points toward its trigger.
const MENU_ARROW_SIDE_PX: f32 = 7.;
const MENU_ARROW_STROKE_PX: f32 = 1.;
pub(crate) const MENU_ARROW_CANVAS_PX: f32 = 12.;
pub(crate) const MENU_ARROW_OVERHANG_PX: f32 = 6.;

pub(super) fn render_popup(
    menu: &mut PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> impl IntoElement {
    menu.update_submenu_anchor(window);
    let view = cx.entity().clone();
    let max_height = menu.max_height.unwrap_or_else(|| {
        (window.viewport_size().height - px(16.)).max(px(CONTEXT_MENU_ACTION_ROW_HEIGHT))
    });
    let max_width = menu.max_width.unwrap_or(px(500.));
    let row_width = menu
        .max_width
        .or(menu.min_width)
        .map(|width| (width.as_f32() - 14.).max(1.))
        .unwrap_or(DEFAULT_ACTION_ROW_WIDTH);
    let items_count = menu.menu_items.len();
    let children: Vec<_> = menu
        .menu_items
        .iter()
        .enumerate()
        .filter(|(ix, item)| !(*ix + 1 == items_count && matches!(item, PopupMenuItem::Separator)))
        .map(|(ix, item)| render_item(menu, ix, item, row_width, window, cx))
        .collect();
    let closing = menu.closing;
    let animation_id = if closing {
        ElementId::from(("app-popup-menu-exit", menu.close_epoch))
    } else {
        ElementId::from(("app-popup-menu-entry", cx.entity().entity_id()))
    };
    let show_arrow = menu.show_arrow;
    let arrow_edge = menu.arrow_edge;
    let arrow_offset = menu.arrow_center_offset();
    let scroll_handle = menu.scroll_handle.clone();
    let browser_scroll = menu.browser_scroll.clone();

    let [scroll_events, scroll_cursor] = browser_scroll_overlays(
        BrowserScrollTarget::Handle(scroll_handle.clone()),
        browser_scroll,
    );
    let items = v_flex()
        .id("app-popup-menu-items")
        .w_full()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&scroll_handle)
        .p(px(CONTEXT_MENU_POPUP_PADDING))
        .gap(px(CONTEXT_MENU_ITEM_GAP))
        .children(children);

    let surface = v_flex()
        .id("app-popup-menu")
        .debug_selector(|| "app-popup-menu-surface".into())
        .role(Role::Menu)
        .key_context("PopupMenu")
        .track_focus(&menu.focus_handle)
        .on_action(cx.listener(PopupMenu::select_up))
        .on_action(cx.listener(PopupMenu::select_down))
        .on_action(cx.listener(PopupMenu::select_first))
        .on_action(cx.listener(PopupMenu::select_last))
        .on_action(cx.listener(PopupMenu::select_left))
        .on_action(cx.listener(PopupMenu::select_right))
        .on_action(
            cx.listener(|menu, _: &super::popup_actions::Confirm, window, cx| {
                menu.confirm(window, cx)
            }),
        )
        .on_action(
            cx.listener(|menu, _: &super::popup_actions::Cancel, window, cx| {
                menu.dismiss(window, cx)
            }),
        )
        .on_mouse_down_out(cx.listener(PopupMenu::on_mouse_down_out))
        .relative()
        .occlude()
        .min_w(px(120.))
        .when_some(menu.min_width, |this, width| this.min_w(width))
        .max_w(max_width)
        .max_h(max_height)
        .min_h_0()
        .border_1()
        .border_color(rgba(CONTEXT_MENU_BORDER))
        .rounded(px(MENU_RADIUS))
        .bg(rgba(CONTEXT_MENU_SURFACE))
        .text_color(rgb(CONTEXT_MENU_FOREGROUND))
        .shadow_lg()
        .child(items)
        .child(
            div()
                .id("app-popup-menu-scrollbar")
                .absolute()
                .inset_0()
                .child(Scrollbar::vertical(&scroll_handle).scrollbar_show(ScrollbarShow::Always)),
        )
        .child(scroll_events)
        .child(scroll_cursor);
    if !show_arrow {
        let view = view.clone();
        return animate_popup(
            surface.on_prepaint(move |bounds, _, cx| {
                view.update(cx, |menu, _| menu.bounds = bounds);
            }),
            animation_id,
            closing,
            arrow_edge,
        )
        .into_any_element();
    }
    let arrow = div()
        .absolute()
        .when(matches!(arrow_edge, PopupMenuArrowEdge::Top), |this| {
            this.top(px(-MENU_ARROW_OVERHANG_PX))
        })
        .when(matches!(arrow_edge, PopupMenuArrowEdge::Bottom), |this| {
            this.bottom(px(-MENU_ARROW_OVERHANG_PX))
        })
        .when(matches!(arrow_edge, PopupMenuArrowEdge::Left), |this| {
            this.left(px(-MENU_ARROW_OVERHANG_PX))
        })
        .when(matches!(arrow_edge, PopupMenuArrowEdge::Right), |this| {
            this.right(px(-MENU_ARROW_OVERHANG_PX))
        })
        .when_some(arrow_offset, |this, offset| {
            this.when(
                matches!(
                    arrow_edge,
                    PopupMenuArrowEdge::Top | PopupMenuArrowEdge::Bottom
                ),
                |this| this.left(px(offset.as_f32() - MENU_ARROW_CANVAS_PX / 2.)),
            )
            .when(
                matches!(
                    arrow_edge,
                    PopupMenuArrowEdge::Left | PopupMenuArrowEdge::Right
                ),
                |this| this.top(px(offset.as_f32() - MENU_ARROW_CANVAS_PX / 2.)),
            )
        })
        .when(arrow_offset.is_none(), |this| {
            this.when(
                matches!(
                    arrow_edge,
                    PopupMenuArrowEdge::Top | PopupMenuArrowEdge::Bottom
                ),
                |this| this.left(px(0.)).right(px(0.)).flex().justify_center(),
            )
            .when(
                matches!(
                    arrow_edge,
                    PopupMenuArrowEdge::Left | PopupMenuArrowEdge::Right
                ),
                |this| this.top(px(0.)).bottom(px(0.)).flex().items_center(),
            )
        })
        .child(menu_arrow(arrow_edge));
    // Measure the wrapper, not the padded surface. The probe is an absolute
    // size_full canvas, so inside the padded surface it would report an inset
    // box. The wrapper has no padding or border, so its probe equals the
    // menu border box and the chevron can align to the trigger exactly.
    // Animate the wrapper so the chevron fades and slides with the menu.
    animate_popup(
        div()
            .relative()
            .child(surface)
            .child(arrow)
            .on_prepaint(move |bounds, _, cx| {
                view.update(cx, |menu, _| menu.bounds = bounds);
            }),
        animation_id,
        closing,
        arrow_edge,
    )
    .into_any_element()
}

fn animate_popup<E>(
    element: E,
    animation_id: ElementId,
    closing: bool,
    arrow_edge: PopupMenuArrowEdge,
) -> gpui::AnimationElement<E>
where
    E: gpui::Styled + gpui::AnimationExt + 'static,
{
    let opening_offset = match arrow_edge {
        PopupMenuArrowEdge::Top | PopupMenuArrowEdge::Left => -3.,
        PopupMenuArrowEdge::Bottom | PopupMenuArrowEdge::Right => 3.,
    };
    element.with_animation(
        animation_id,
        crate::motion::interaction(),
        move |this, delta| {
            let offset = if closing {
                crate::motion::lerp(0.0, opening_offset, delta)
            } else {
                crate::motion::lerp(opening_offset, 0.0, delta)
            };
            let this = this.opacity(if closing {
                crate::motion::lerp(1.0, 0.0, delta)
            } else {
                crate::motion::lerp(0.0, 1.0, delta)
            });
            match arrow_edge {
                PopupMenuArrowEdge::Top | PopupMenuArrowEdge::Bottom => this.top(px(offset)),
                PopupMenuArrowEdge::Left | PopupMenuArrowEdge::Right => this.left(px(offset)),
            }
        },
    )
}

pub(crate) fn menu_arrow(edge: PopupMenuArrowEdge) -> impl IntoElement {
    div()
        .debug_selector(|| "app-popup-menu-arrow".into())
        .w(px(MENU_ARROW_CANVAS_PX))
        .h(px(MENU_ARROW_CANVAS_PX))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| paint_menu_arrow(bounds, window, edge),
            )
            .w(px(MENU_ARROW_CANVAS_PX))
            .h(px(MENU_ARROW_CANVAS_PX)),
        )
}

fn menu_arrow_half_diagonal() -> f32 {
    MENU_ARROW_SIDE_PX / std::f32::consts::SQRT_2
}

fn paint_menu_arrow(bounds: Bounds<gpui::Pixels>, window: &mut Window, edge: PopupMenuArrowEdge) {
    let center = bounds.center();
    let half = menu_arrow_half_diagonal();
    let left = point(center.x - px(half), center.y);
    let top = point(center.x, center.y - px(half));
    let right = point(center.x + px(half), center.y);
    let bottom = point(center.x, center.y + px(half));
    let (vertices, tip, stroke_start, stroke_end) = match edge {
        PopupMenuArrowEdge::Top => ([left, bottom, right, top], top, left, right),
        PopupMenuArrowEdge::Bottom => ([left, top, right, bottom], bottom, left, right),
        PopupMenuArrowEdge::Left => ([top, right, bottom, left], left, top, bottom),
        PopupMenuArrowEdge::Right => ([top, left, bottom, right], right, top, bottom),
    };
    let mut fill = PathBuilder::fill();
    fill.add_polygon(&vertices, true);
    if let Ok(path) = fill.build() {
        window.paint_path(path, rgba(CONTEXT_MENU_SURFACE));
    }
    let mut stroke = PathBuilder::stroke(px(MENU_ARROW_STROKE_PX));
    stroke.move_to(stroke_start);
    stroke.line_to(tip);
    stroke.line_to(stroke_end);
    if let Ok(path) = stroke.build() {
        window.paint_path(path, rgba(CONTEXT_MENU_BORDER));
    }
}

fn render_item(
    menu: &PopupMenu,
    ix: usize,
    item: &PopupMenuItem,
    row_width: f32,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> gpui::AnyElement {
    let selected = menu.selected_index == Some(ix);
    let id = format!("app-popup-menu-item-{}-{ix}", cx.entity().entity_id());
    let is_submenu = matches!(item, PopupMenuItem::Submenu { .. });
    let disabled = item.is_disabled(cx);
    let visually_disabled = match item {
        PopupMenuItem::ElementItem {
            render_disabled, ..
        } => disabled && *render_disabled,
        PopupMenuItem::Item { disabled, .. } | PopupMenuItem::Submenu { disabled, .. } => *disabled,
        PopupMenuItem::Separator => true,
    };
    let has_shortcut = matches!(
        item,
        PopupMenuItem::Item {
            shortcut: Some(_),
            ..
        }
    );

    let base_row = div()
        .id(id.clone())
        .w_full()
        .flex_none()
        .relative()
        .role(Role::MenuItem)
        .when_some(item.a11y_label(), |this, label| this.aria_label(label))
        .aria_selected(selected)
        .when(visually_disabled, |this| {
            this.when(has_shortcut, |this| {
                this.text_color(rgb(TEXT_FIELD_CONTEXT_MENU_DISABLED))
                    .opacity(TEXT_FIELD_CONTEXT_MENU_DISABLED_OPACITY)
            })
            .when(!has_shortcut, |this| {
                this.text_color(cx.theme().muted_foreground).opacity(0.72)
            })
        })
        .when(!disabled && selected, |this| {
            this.bg(rgb(CONTEXT_MENU_HOVER))
                .text_color(rgb(CONTEXT_MENU_HOVER_FOREGROUND))
                .rounded(px(6.))
        })
        .when(!disabled, |this| {
            this.on_hover(cx.listener(move |menu, hovered, _, cx| {
                if *hovered {
                    menu.set_selected_index(ix, cx);
                } else if !is_submenu && menu.selected_index == Some(ix) {
                    menu.selected_index = None;
                    cx.notify();
                }
            }))
        })
        .when(!disabled, |this| this.cursor_pointer());

    match item {
        PopupMenuItem::Separator => div()
            .id(id)
            .debug_selector(|| "app-popup-menu-separator".into())
            .flex_none()
            .h(px(1.))
            .min_h(px(1.))
            .my(px(5.))
            .mx(px(6.))
            .bg(rgba(CONTEXT_MENU_SEPARATOR))
            .into_any_element(),
        PopupMenuItem::ElementItem { render, .. } => {
            let mut row = base_row;
            if !disabled {
                row = row.on_click(cx.listener(move |menu, _, window, cx| {
                    menu.on_click(ix, window, cx);
                }));
            }
            row.child((render)(window, cx)).into_any_element()
        }
        PopupMenuItem::Item {
            icon,
            label,
            shortcut,
            disabled,
            ..
        } => {
            let mut row = base_row;
            if !disabled {
                row = row.on_click(cx.listener(move |menu, _, window, cx| {
                    menu.on_click(ix, window, cx);
                }));
            }
            row.child(standard_action_row(
                label.clone(),
                icon.clone(),
                shortcut.clone(),
                *disabled,
                selected,
                row_width,
            ))
            .into_any_element()
        }
        PopupMenuItem::Submenu {
            icon,
            label,
            disabled,
            menu: child,
        } => {
            let mut row = base_row;
            let trigger_width = row_width;
            let (anchor, offset_x) = menu.submenu_anchor;
            let child = child.clone();
            row = row.debug_selector(|| "app-popup-submenu-trigger".into());
            row = row.child(super::items::submenu_action_row(
                label.clone(),
                icon.clone(),
                row_width,
            ));
            if *disabled || !selected {
                row.into_any_element()
            } else {
                row.child(
                    deferred(
                        anchored()
                            .anchor(anchor)
                            .position_mode(AnchoredPositionMode::Local)
                            .offset(point(
                                if matches!(anchor, Anchor::TopRight | Anchor::BottomRight) {
                                    px(offset_x.as_f32())
                                } else {
                                    px(trigger_width + SUBMENU_GAP)
                                },
                                px(SUBMENU_LOCAL_Y_OFFSET),
                            ))
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .id("app-popup-submenu")
                                    .debug_selector(|| "app-popup-submenu-surface".into())
                                    .child(child),
                            ),
                    )
                    .with_priority(2),
                )
                .into_any_element()
            }
        }
    }
}

fn standard_action_row(
    label: gpui::SharedString,
    icon: Option<Icon>,
    shortcut: Option<gpui::SharedString>,
    disabled: bool,
    selected: bool,
    width: f32,
) -> impl IntoElement {
    let text_field_style = shortcut.is_some();
    div()
        .w(px(width))
        .max_w_full()
        .min_w_0()
        .min_h(px(CONTEXT_MENU_ACTION_ROW_HEIGHT))
        .px(px(CONTEXT_MENU_ACTION_HORIZONTAL_PADDING))
        .py(px(CONTEXT_MENU_ACTION_VERTICAL_PADDING))
        .flex()
        .items_center()
        .gap(px(CONTEXT_MENU_ACTION_COLUMN_GAP))
        .child(
            div()
                .w(px(CONTEXT_MENU_ICON_COLUMN_WIDTH))
                .flex()
                .items_center()
                .justify_center()
                .when(text_field_style, |this| {
                    this.text_color(rgb(if disabled {
                        TEXT_FIELD_CONTEXT_MENU_DISABLED
                    } else {
                        TEXT_FIELD_CONTEXT_MENU_ICON
                    }))
                })
                .children(icon.map(|icon| icon.size(px(CONTEXT_MENU_ICON_SIZE)))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(CONTEXT_MENU_LABEL_SIZE))
                .font_weight(gpui::FontWeight(450.))
                .truncate()
                .overflow_hidden()
                .child(label),
        )
        .children(shortcut.map(|shortcut| {
            div()
                .flex_none()
                .text_size(px(CONTEXT_MENU_SHORTCUT_SIZE))
                .font_weight(gpui::FontWeight(450.))
                .text_color(rgb(if selected && !disabled {
                    TEXT_FIELD_CONTEXT_MENU_SELECTED_SHORTCUT
                } else {
                    TEXT_FIELD_CONTEXT_MENU_DISABLED
                }))
                .child(shortcut)
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Focusable, Render, TestAppContext, VisualTestContext, div, px, size};

    struct ArrowHost {
        menu: gpui::Entity<PopupMenu>,
    }

    impl Render for ArrowHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            div().size_full().child(
                div()
                    .absolute()
                    .left(px(100.))
                    .top(px(100.))
                    .child(self.menu.clone()),
            )
        }
    }

    #[test]
    fn arrow_half_diagonal_matches_tooltip_diamond() {
        let half = menu_arrow_half_diagonal();
        assert!((half - 7. / std::f32::consts::SQRT_2).abs() < 0.0001);
        assert_eq!(MENU_ARROW_SIDE_PX, 7.);
        assert_eq!(MENU_ARROW_CANVAS_PX, 12.);
        assert_eq!(MENU_ARROW_OVERHANG_PX, 6.);
    }

    #[gpui::test]
    fn above_menu_renders_centered_chevron_straddling_bottom_edge(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.open_window(size(px(600.), px(500.)), |window, cx| {
            let menu = PopupMenu::build(window, cx, |menu, _, _| {
                menu.min_w(px(200.))
                    .max_w(px(200.))
                    .with_arrow()
                    .item(PopupMenuItem::new("First"))
            });
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            ArrowHost { menu }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let surface = visual
            .debug_bounds("app-popup-menu-surface")
            .expect("menu surface bounds");
        let arrow = visual
            .debug_bounds("app-popup-menu-arrow")
            .expect("menu chevron bounds");
        assert_eq!(arrow.size.width, px(MENU_ARROW_CANVAS_PX));
        assert_eq!(arrow.size.height, px(MENU_ARROW_CANVAS_PX));
        let surface_center = surface.center().x;
        let arrow_center = arrow.center().x;
        assert!(
            (surface_center.as_f32() - arrow_center.as_f32()).abs() < 1.,
            "chevron should be centered under the menu"
        );
        assert!(
            arrow.origin.y < surface.bottom(),
            "chevron should overlap the menu bottom border"
        );
        assert!(
            arrow.bottom() > surface.bottom(),
            "chevron tip should extend below the menu toward the button"
        );
    }

    #[gpui::test]
    fn above_menu_aligns_chevron_to_trigger(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.open_window(size(px(600.), px(500.)), |window, cx| {
            let menu = PopupMenu::build(window, cx, |menu, _, _| {
                menu.min_w(px(200.))
                    .max_w(px(200.))
                    .with_arrow()
                    .item(PopupMenuItem::new("First"))
            });
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            ArrowHost { menu }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let surface = visual
            .debug_bounds("app-popup-menu-surface")
            .expect("menu surface bounds");
        let trigger_x = surface.origin.x + px(200. - 16.);
        visual.cx.update(|cx| {
            let root = window
                .root(cx)
                .expect("menu test root")
                .read(cx)
                .menu
                .clone();
            root.update(cx, |menu, _| {
                menu.set_arrow_anchor(trigger_x);
            });
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let surface = visual
            .debug_bounds("app-popup-menu-surface")
            .expect("menu surface bounds");
        let arrow = visual
            .debug_bounds("app-popup-menu-arrow")
            .expect("menu chevron bounds");
        assert!(
            (arrow.center().x.as_f32() - trigger_x.as_f32()).abs() < 1.5,
            "chevron should point at the trigger near the menu edge"
        );
        assert!(
            arrow.center().x > surface.center().x,
            "edge trigger should move the chevron off center"
        );
        assert!(
            arrow.origin.x >= surface.origin.x + px(10.),
            "chevron should stay inside the menu"
        );
        assert!(
            arrow.bottom() <= surface.bottom() + px(8.),
            "chevron should keep the 8px gap placement"
        );
    }

    #[gpui::test]
    fn plain_menu_has_no_chevron(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.open_window(size(px(600.), px(500.)), |window, cx| {
            let menu = PopupMenu::build(window, cx, |menu, _, _| {
                menu.min_w(px(200.))
                    .max_w(px(200.))
                    .item(PopupMenuItem::new("First"))
            });
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            ArrowHost { menu }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(visual.debug_bounds("app-popup-menu-arrow").is_none());
    }

    #[test]
    fn popup_always_clamps_height_to_the_viewport() {
        let production = include_str!("popup_render.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("popup renderer");
        assert!(production.contains(".max_h(max_height)"));
        assert!(production.contains(".overflow_y_scroll()"));
        assert!(production.contains(".flex_none()"));
        assert!(production.contains(".min_h(px(1.))"));
        assert!(production.contains("app-popup-menu-items"));
        assert!(production.contains("Scrollbar::vertical"));
        assert!(production.contains("ScrollbarShow::Always"));
        assert!(production.contains("browser_scroll_overlays"));
        assert!(production.contains("animate_popup("));
        assert!(production.contains(".child(arrow)"));
        assert!(!production.contains(".when(menu.scrollable"));
    }

    #[gpui::test]
    fn short_popup_stays_content_sized(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let window = cx.open_window(size(px(600.), px(500.)), |window, cx| {
            let menu = PopupMenu::build(window, cx, |menu, _, _| {
                menu.min_w(px(200.))
                    .max_w(px(200.))
                    .item(PopupMenuItem::new("Copy line"))
            });
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            ArrowHost { menu }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let surface = visual
            .debug_bounds("app-popup-menu-surface")
            .expect("menu surface bounds");
        assert!(
            f32::from(surface.size.height) < 80.,
            "short menu height was {}",
            f32::from(surface.size.height)
        );
    }

    #[gpui::test]
    fn tall_popup_shrinks_to_the_viewport_instead_of_clipping(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let viewport_height = 160.;
        let window = cx.open_window(size(px(400.), px(viewport_height)), |window, cx| {
            let menu = PopupMenu::build(window, cx, |menu, _, _| {
                let mut menu = menu.min_w(px(200.)).max_w(px(200.));
                for index in 0..20 {
                    if index == 4 || index == 12 {
                        menu = menu.separator();
                    }
                    menu = menu.item(PopupMenuItem::new(format!("Item {index}")));
                }
                menu
            });
            menu.update(cx, |menu, cx| {
                menu.focus_handle(cx).focus(window, cx);
            });
            ArrowHost { menu }
        });
        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let surface = visual
            .debug_bounds("app-popup-menu-surface")
            .expect("menu surface bounds");
        let max_height = viewport_height - 16.;
        assert!(
            f32::from(surface.size.height) <= max_height + 1.,
            "tall menu height was {}, max {}",
            f32::from(surface.size.height),
            max_height
        );
        let scroll_range = window
            .update(cx, |host, _, cx| {
                host.menu.read(cx).scroll_handle.max_offset().y
            })
            .expect("menu host");
        assert!(
            f32::from(scroll_range).abs() > 0.,
            "a capped menu should expose a vertical scroll range"
        );
        let separator = visual
            .debug_bounds("app-popup-menu-separator")
            .expect("capped menus must keep separator lines");
        assert!(
            f32::from(separator.size.height) >= 1.,
            "separator height was {}",
            f32::from(separator.size.height)
        );
    }
}
