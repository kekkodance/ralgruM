use gpui::{Context, SharedString, Window, px};
use gpui_component::Icon;

use crate::assets::LocalIcon;

use super::PopupMenu;
use super::items;

/// Keep the child menu close to the width of the original context submenu.
pub(crate) const SUBMENU_WIDTH: f32 = 254.;
/// Keep the pointer traversal gap and the placement calculation in sync.
pub(crate) const SUBMENU_GAP: f32 = 7.;

const SUBMENU_WINDOW_MARGIN: f32 = 8.;

fn submenu_max_height(viewport_height: f32) -> f32 {
    (viewport_height - SUBMENU_WINDOW_MARGIN * 2.).max(items::ACTION_ROW_HEIGHT)
}

pub(super) fn style_submenu_popup(menu: PopupMenu, window: &Window) -> PopupMenu {
    menu.min_w(px(SUBMENU_WIDTH))
        .max_w(px(SUBMENU_WIDTH))
        .max_h(px(submenu_max_height(
            window.viewport_size().height.as_f32(),
        )))
        .scrollable(true)
}

/// Add an app-styled submenu. The app-owned PopupMenu keeps the parent trigger
/// selected while the pointer crosses the seven-pixel child gap.
pub(super) fn styled_submenu_with_icon<F>(
    menu: PopupMenu,
    icon: LocalIcon,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
    f: F,
) -> PopupMenu
where
    F: Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
{
    menu.submenu_with_icon(
        Some(Icon::default().path(icon.path())),
        label,
        window,
        cx,
        move |menu, window, cx| style_submenu_popup(f(menu, window, cx), window),
    )
}

/// The collection download button is a regular component dropdown, not an
/// entity context menu. Keep that unrelated popup on gpui-component's native
/// interaction path.
pub(super) fn native_styled_submenu_with_icon<F>(
    menu: gpui_component::menu::PopupMenu,
    icon: LocalIcon,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut gpui::Context<gpui_component::menu::PopupMenu>,
    f: F,
) -> gpui_component::menu::PopupMenu
where
    F: Fn(
            gpui_component::menu::PopupMenu,
            &mut Window,
            &mut gpui::Context<gpui_component::menu::PopupMenu>,
        ) -> gpui_component::menu::PopupMenu
        + 'static,
{
    menu.submenu_with_icon(
        Some(Icon::default().path(icon.path())),
        label,
        window,
        cx,
        move |menu, window, cx| {
            f(menu, window, cx)
                .min_w(px(SUBMENU_WIDTH))
                .max_w(px(SUBMENU_WIDTH))
                .max_h(px(submenu_max_height(
                    window.viewport_size().height.as_f32(),
                )))
                .scrollable(true)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::PopupMenuItem;
    use super::*;
    use gpui::{
        AppContext, Focusable, InteractiveElement, Modifiers, ParentElement, Render,
        StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, point, size,
    };

    struct MenuView {
        menu: gpui::Entity<PopupMenu>,
        first_row: gpui::Entity<HoverProbe>,
        second_row: gpui::Entity<HoverProbe>,
        host_top: f32,
    }

    struct HoverProbe {
        selector: &'static str,
        hovered: bool,
    }

    impl Render for HoverProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
            let selector = self.selector;
            gpui::div()
                .id(selector)
                .debug_selector(move || selector.into())
                .h(px(items::ACTION_ROW_HEIGHT))
                .w(px(SUBMENU_WIDTH))
                .on_hover(cx.listener(|this, hovered, _, cx| {
                    this.hovered = *hovered;
                    cx.notify();
                }))
        }
    }

    impl Render for MenuView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            gpui::div().size_full().child(
                gpui::div()
                    .absolute()
                    .left(px(100.))
                    .top(px(self.host_top))
                    .debug_selector(|| "popup-host".into())
                    .child(self.menu.clone()),
            )
        }
    }

    fn draw_app_submenu(
        cx: &mut TestAppContext,
    ) -> (
        VisualTestContext,
        gpui::Entity<HoverProbe>,
        gpui::Entity<HoverProbe>,
    ) {
        draw_app_submenu_at(cx, 900., 600., 100.)
    }

    fn draw_app_submenu_at(
        cx: &mut TestAppContext,
        viewport_width: f32,
        viewport_height: f32,
        host_top: f32,
    ) -> (
        VisualTestContext,
        gpui::Entity<HoverProbe>,
        gpui::Entity<HoverProbe>,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::context_menu::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        let window = cx.open_window(
            size(px(viewport_width), px(viewport_height)),
            |window, cx| {
                let first_row = cx.new(|_| HoverProbe {
                    selector: "native-submenu-row-one",
                    hovered: false,
                });
                let second_row = cx.new(|_| HoverProbe {
                    selector: "native-submenu-row-two",
                    hovered: false,
                });
                let first_row_for_menu = first_row.clone();
                let second_row_for_menu = second_row.clone();
                let menu = PopupMenu::build(window, cx, |menu, window, cx| {
                    styled_submenu_with_icon(
                        menu,
                        LocalIcon::Download,
                        "Download",
                        window,
                        cx,
                        move |menu, _, _| {
                            let first_row = first_row_for_menu.clone();
                            let second_row = second_row_for_menu.clone();
                            menu.item(
                                PopupMenuItem::element(move |_, _| first_row.clone())
                                    .icon(Icon::empty()),
                            )
                            .item(
                                PopupMenuItem::element(move |_, _| second_row.clone())
                                    .icon(Icon::empty()),
                            )
                        },
                    )
                    .min_w(px(super::super::ENTITY_MENU_WIDTH))
                    .max_w(px(super::super::ENTITY_MENU_WIDTH))
                });
                menu.update(cx, |menu, cx| {
                    menu.focus_handle(cx).focus(window, cx);
                });
                MenuView {
                    menu,
                    first_row,
                    second_row,
                    host_top,
                }
            },
        );

        let mut visual = VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let root = window.root(cx).expect("menu test root");
        let (first_row, second_row) = cx.update(|cx| {
            let root = root.read(cx);
            (root.first_row.clone(), root.second_row.clone())
        });
        (visual, first_row, second_row)
    }

    #[gpui::test]
    fn app_submenu_uses_parent_hitbox_and_child_selection(cx: &mut TestAppContext) {
        let (mut visual, first_row, second_row) = draw_app_submenu(cx);
        let parent = visual
            .debug_bounds("popup-host")
            .expect("parent menu bounds");

        // The app-owned trigger owns this hitbox and keeps the child mounted
        // while the pointer crosses the seven-pixel gap.
        visual.simulate_mouse_move(
            point(parent.origin.x + px(20.), parent.origin.y + px(20.)),
            None,
            Modifiers::default(),
        );
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(visual.debug_bounds("native-submenu-row-one").is_some());
        let trigger = visual
            .debug_bounds("app-popup-submenu-trigger")
            .expect("submenu trigger bounds");
        let child_surface = visual
            .debug_bounds("app-popup-submenu-surface")
            .expect("submenu surface bounds");
        assert_eq!(
            child_surface.origin.y, trigger.origin.y,
            "submenu surface should align with the trigger top"
        );
        let first = visual
            .debug_bounds("native-submenu-row-one")
            .expect("first child row");
        visual.simulate_mouse_move(
            point(first.origin.x + px(12.), first.origin.y + px(12.)),
            None,
            Modifiers::default(),
        );
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        let (first_hovered, second_hovered) = visual
            .cx
            .update(|cx| (first_row.read(cx).hovered, second_row.read(cx).hovered));
        assert!(first_hovered);
        assert!(!second_hovered);

        let second = visual
            .debug_bounds("native-submenu-row-two")
            .expect("second child row");
        visual.simulate_mouse_move(
            point(second.origin.x + px(12.), second.origin.y + px(12.)),
            None,
            Modifiers::default(),
        );
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(second.origin.y > first.origin.y);
        let (first_hovered, second_hovered) = visual
            .cx
            .update(|cx| (first_row.read(cx).hovered, second_row.read(cx).hovered));
        assert!(!first_hovered);
        assert!(second_hovered);
        assert!(parent.size.width >= px(super::super::ENTITY_MENU_WIDTH));
    }

    #[gpui::test]
    fn app_submenu_clamps_to_lower_viewport_edge(cx: &mut TestAppContext) {
        let (mut visual, _, _) = draw_app_submenu_at(cx, 900., 220., 180.);
        let parent = visual
            .debug_bounds("popup-host")
            .expect("parent menu bounds");
        visual.simulate_mouse_move(
            point(parent.origin.x + px(20.), parent.origin.y + px(20.)),
            None,
            Modifiers::default(),
        );
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let trigger = visual
            .debug_bounds("app-popup-submenu-trigger")
            .expect("submenu trigger bounds");
        let child_surface = visual
            .debug_bounds("app-popup-submenu-surface")
            .expect("submenu surface bounds");
        assert!(child_surface.origin.y < trigger.origin.y);
        assert!(child_surface.bottom() <= px(220. - 8.));
    }

    #[gpui::test]
    fn app_submenu_keyboard_navigation_opens_child(cx: &mut TestAppContext) {
        let (mut visual, _, _) = draw_app_submenu(cx);
        visual.simulate_keystrokes("down right");
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(visual.debug_bounds("native-submenu-row-one").is_some());
    }
}
