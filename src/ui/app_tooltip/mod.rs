pub(crate) mod geometry;

use std::{cell::Cell, collections::HashMap, rc::Rc, time::Duration};

use gpui::{
    AnimationExt, App, Bounds, BoxShadow, Context, ElementId, Entity, FontWeight, Global,
    InteractiveElement, IntoElement, ParentElement, PathBuilder, Pixels, Render, SharedString,
    Size, StatefulInteractiveElement, Styled, Task, WeakEntity, Window, WindowId, canvas, deferred,
    div, px, rgb, rgba,
};
use gpui_component::{ElementExt as _, WindowExt as _};

use self::geometry::{
    ARROW_STROKE_WIDTH, TRIGGER_GAP, TooltipArrowGeometry, TooltipPlacement, estimate_tooltip_size,
    resolve_tooltip_geometry_with_gap, resolve_tooltip_geometry_with_gap_and_end_inset,
    tooltip_arrow_geometry, translate_arrow_point_to_canvas,
};

pub(crate) const APP_TOOLTIP_SHOW_DELAY: Duration = Duration::from_millis(500);
const TOOLTIP_MAX_WIDTH: f32 = 280.0;
const TOOLTIP_PADDING_Y: f32 = 6.0;
const TOOLTIP_PADDING_X: f32 = 9.0;
const TOOLTIP_FONT_SIZE: f32 = 11.0;
const TOOLTIP_LINE_HEIGHT: f32 = 14.85;
const OVERLAY_DEFERRED_PRIORITY: usize = 3;
const TOOLTIP_BACKGROUND: u32 = 0x18181bfa;
const TOOLTIP_BORDER: u32 = 0x3f3f46eb;
/// Distance the tooltip slides while entering, from the direction of its
/// trigger toward its final resting place.
const ENTRY_SLIDE_PX: f32 = 6.;

/// App-owned tooltip overlay state. The shell creates one entity for its
/// window and registers a weak handle so trigger elements do not retain it.
pub(crate) struct AppTooltipOverlay {
    pending: Option<TooltipRequest>,
    visible: Option<VisibleTooltip>,
    generation: u64,
    show_task: Option<Task<()>>,
    measured_size: Option<Size<gpui::Pixels>>,
    last_viewport_sizes: HashMap<gpui::WindowId, Size<gpui::Pixels>>,
    last_had_dialog: bool,
}

#[derive(Clone)]
struct TooltipRequest {
    text: SharedString,
    trigger_bounds: Bounds<gpui::Pixels>,
    trigger_window: WindowId,
    placement: TooltipPlacement,
    trigger_gap: Pixels,
    bubble_horizontal_pin: BubbleHorizontalPin,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum BubbleHorizontalPin {
    None,
    ViewportEndInset(Pixels),
}

struct VisibleTooltip {
    request: TooltipRequest,
    generation: u64,
}

struct AppTooltipGlobal(WeakEntity<AppTooltipOverlay>);

impl Global for AppTooltipGlobal {}

pub(crate) fn set_global(cx: &mut App, overlay: &Entity<AppTooltipOverlay>) {
    cx.set_global(AppTooltipGlobal(overlay.downgrade()));
}

fn global_overlay(cx: &mut App) -> Option<Entity<AppTooltipOverlay>> {
    cx.try_global::<AppTooltipGlobal>()?.0.upgrade()
}

/// The registered tooltip overlay entity, for windows that also need to
/// paint it (the sidebar popout).
pub(crate) fn global_tooltip_overlay(cx: &App) -> Option<Entity<AppTooltipOverlay>> {
    cx.try_global::<AppTooltipGlobal>()
        .and_then(|global| global.0.upgrade())
}

pub(crate) fn dismiss_global(cx: &mut App) {
    if let Some(overlay) = global_overlay(cx) {
        overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
    }
}

/// True when a tooltip trigger lives inside the topmost dialog frame.
///
/// Unknown dialog bounds count as outside so a stale background trigger stays
/// suppressed until the dialog frame has painted and measured itself.
pub(crate) fn trigger_inside_top_dialog(
    trigger: Bounds<gpui::Pixels>,
    top_dialog: Option<Bounds<gpui::Pixels>>,
) -> bool {
    let Some(dialog) = top_dialog else {
        return false;
    };
    dialog.contains(&trigger.center())
}

impl AppTooltipOverlay {
    pub(crate) fn new() -> Self {
        Self {
            pending: None,
            visible: None,
            generation: 0,
            show_task: None,
            measured_size: None,
            last_viewport_sizes: HashMap::new(),
            last_had_dialog: false,
        }
    }

    fn request_show(
        &mut self,
        request: TooltipRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A new trigger supersedes the previous tooltip immediately. The
        // delay only controls when the replacement becomes visible.
        self.show_task = None;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.pending = Some(request);
        self.visible = None;
        self.measured_size = None;
        cx.notify();

        self.show_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(APP_TOOLTIP_SHOW_DELAY).await;
            let _ = this.update_in(cx, |overlay, _, cx| {
                if overlay.generation != generation {
                    return;
                }

                overlay.show_task = None;
                let Some(request) = overlay.pending.take() else {
                    return;
                };
                // The pointer-leaves-trigger check is owned by the hover
                // system: on_hover(false) cancels the pending request. Reading
                // window.mouse_position() here races across windows, so the
                // delay only gates the timing, not the geometry.
                overlay.visible = Some(VisibleTooltip {
                    request,
                    generation,
                });
                overlay.measured_size = None;
                cx.notify();
            });
        }));
    }

    fn request_hide(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.visible = None;
        self.measured_size = None;
        self.show_task = None;
        cx.notify();
    }

    fn clear_without_notify(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = None;
        self.visible = None;
        self.measured_size = None;
        self.show_task = None;
    }

    fn record_measurement(
        &mut self,
        generation: u64,
        size: Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.visible.as_ref().is_some_and(|tooltip| {
            tooltip.generation == generation && self.measured_size != Some(size)
        }) {
            self.measured_size = Some(size);
            cx.notify();
        }
    }
}

impl Render for AppTooltipOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport_size = window.viewport_size();
        let window_id = window.window_handle().window_id();

        // A new viewport invalidates both the trigger bounds and the measured
        // text size. Wait for a fresh hover to avoid a stale placement. The
        // overlay renders once per window, so track the viewport per window
        // instead of letting distinct windows clobber each other's viewports.
        if self
            .last_viewport_sizes
            .get(&window_id)
            .is_some_and(|previous| *previous != viewport_size)
        {
            self.clear_without_notify();
        }
        self.last_viewport_sizes.insert(window_id, viewport_size);

        // Dialogs own the modal interaction layer. Suppress only stale triggers
        // from outside the topmost dialog so tooltips inside the modal still
        // work. The deferred overlay paints above dialogs by design.
        let has_dialog = window.has_active_dialog(cx);
        if self.last_had_dialog && !has_dialog {
            // The dialog closed while its tooltip was pending or visible. The
            // trigger is gone, so drop it instead of stranding a bubble.
            self.clear_without_notify();
        }
        self.last_had_dialog = has_dialog;
        if has_dialog {
            let top_dialog = window.top_dialog_bounds(cx);
            let trigger = self
                .visible
                .as_ref()
                .map(|tooltip| tooltip.request.trigger_bounds)
                .or(self.pending.as_ref().map(|request| request.trigger_bounds));
            let Some(trigger) = trigger else {
                return div().into_any_element();
            };
            if !trigger_inside_top_dialog(trigger, top_dialog) {
                self.clear_without_notify();
                return div().into_any_element();
            }
        }
        // One overlay entity renders in every window, but each window paints
        // only the tooltip triggered inside it.
        if self.visible.as_ref().is_some_and(|tooltip| {
            tooltip.request.trigger_window != window.window_handle().window_id()
        }) {
            return div().into_any_element();
        }

        let Some(visible) = self.visible.as_ref() else {
            return div().into_any_element();
        };

        if visible.request.trigger_window == window.window_handle().window_id()
            && visible.request.trigger_bounds.size.width > px(0.)
            && visible.request.trigger_bounds.size.height > px(0.)
            && !visible
                .request
                .trigger_bounds
                .contains(&window.mouse_position())
        {
            self.clear_without_notify();
            return div().into_any_element();
        }

        let generation = visible.generation;
        let request = visible.request.clone();
        let measured_size = self
            .measured_size
            .unwrap_or_else(|| estimate_tooltip_size(request.text.as_ref(), viewport_size));
        let geometry = match request.bubble_horizontal_pin {
            BubbleHorizontalPin::ViewportEndInset(horizontal_end_inset) => {
                resolve_tooltip_geometry_with_gap_and_end_inset(
                    request.trigger_bounds,
                    measured_size,
                    viewport_size,
                    request.placement,
                    request.trigger_gap,
                    horizontal_end_inset,
                )
            }
            BubbleHorizontalPin::None => resolve_tooltip_geometry_with_gap(
                request.trigger_bounds,
                measured_size,
                viewport_size,
                request.placement,
                request.trigger_gap,
            ),
        };
        let max_width = (viewport_size.width - px(16.))
            .max(px(1.))
            .min(px(TOOLTIP_MAX_WIDTH));
        let overlay = cx.entity().downgrade();
        let arrow_geometry = tooltip_arrow_geometry(geometry);
        let arrow_bounds = arrow_geometry.paint_bounds;

        let arrow = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| paint_arrow(bounds, arrow_geometry, window),
        )
        .absolute()
        .left(px(arrow_bounds.left().as_f32()))
        .top(px(arrow_bounds.top().as_f32()))
        .w(px(arrow_bounds.size.width.as_f32()))
        .h(px(arrow_bounds.size.height.as_f32()));

        let bubble = div()
            .absolute()
            .top(px(geometry.bubble_bounds.top().as_f32()))
            .max_w(max_width)
            .py(px(TOOLTIP_PADDING_Y))
            .px(px(TOOLTIP_PADDING_X))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(TOOLTIP_BORDER))
            .bg(rgba(TOOLTIP_BACKGROUND))
            .text_color(rgb(0xf4f4f5))
            .text_size(px(TOOLTIP_FONT_SIZE))
            .font_weight(FontWeight(450.))
            .line_height(px(TOOLTIP_LINE_HEIGHT))
            .text_center()
            .whitespace_normal()
            .shadow(vec![
                BoxShadow::new(px(0.), px(8.), rgba(0x00000073).into()).blur_radius(px(24.)),
            ])
            .on_prepaint(move |bounds, _, cx| {
                overlay
                    .update(cx, |overlay, cx| {
                        overlay.record_measurement(generation, bounds.size, cx)
                    })
                    .ok();
            })
            .child(request.text);
        let bubble = match request.bubble_horizontal_pin {
            BubbleHorizontalPin::None => bubble.left(px(geometry.bubble_bounds.left().as_f32())),
            BubbleHorizontalPin::ViewportEndInset(horizontal_end_inset) => {
                bubble.right(px(horizontal_end_inset.max(px(0.)).as_f32()))
            }
        };

        // The tooltip enters from the side it appears on: entering from the
        // direction of its trigger. Vertical placements slide along the
        // trigger axis; the end-inset pin places the bubble flush to the
        // viewport edge left of its trigger, so it enters moving left.
        let (enter_dx, enter_dy) = match request.bubble_horizontal_pin {
            BubbleHorizontalPin::ViewportEndInset(_) => (-ENTRY_SLIDE_PX, 0.),
            BubbleHorizontalPin::None => match geometry.placement {
                TooltipPlacement::Top => (0., ENTRY_SLIDE_PX),
                TooltipPlacement::Bottom => (0., -ENTRY_SLIDE_PX),
                TooltipPlacement::Right => (-ENTRY_SLIDE_PX, 0.),
            },
        };

        let animation_id = ElementId::from(("app-tooltip-entry", generation));
        deferred(
            div()
                .id(("app-tooltip", generation))
                .absolute()
                .inset_0()
                // Paint the bubble first so the arrow fill masks the straight
                // border segment beneath its inner half.
                .child(bubble)
                .child(arrow)
                .with_animation(
                    animation_id,
                    crate::motion::interaction(),
                    move |this, delta| {
                        this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                            .top(px(crate::motion::lerp(enter_dy, 0.0, delta)))
                            .left(px(crate::motion::lerp(enter_dx, 0.0, delta)))
                    },
                ),
        )
        .with_priority(OVERLAY_DEFERRED_PRIORITY)
        .into_any_element()
    }
}

fn paint_arrow(bounds: Bounds<gpui::Pixels>, arrow: TooltipArrowGeometry, window: &mut Window) {
    let fill_points = arrow
        .fill_points
        .map(|point| translate_arrow_point_to_canvas(point, arrow.paint_bounds, bounds));
    let mut fill = PathBuilder::fill();
    fill.add_polygon(&fill_points, true);
    if let Ok(path) = fill.build() {
        window.paint_path(path, rgba(TOOLTIP_BACKGROUND));
    }

    let outward_stroke_points = arrow
        .outward_stroke_points
        .map(|point| translate_arrow_point_to_canvas(point, arrow.paint_bounds, bounds));
    let mut stroke = PathBuilder::stroke(ARROW_STROKE_WIDTH);
    stroke.move_to(outward_stroke_points[0]);
    stroke.line_to(outward_stroke_points[1]);
    stroke.line_to(outward_stroke_points[2]);
    if let Ok(path) = stroke.build() {
        window.paint_path(path, rgba(TOOLTIP_BORDER));
    }
}

/// Adds the app-owned tooltip treatment and timing to a GPUI interactive element.
pub(crate) trait AppTooltipExt:
    StatefulInteractiveElement + gpui_component::ElementExt + Sized
{
    fn app_tooltip(self, text: impl Into<SharedString> + 'static) -> Self {
        self.app_tooltip_with_placement(text.into(), TooltipPlacement::Top)
    }

    fn app_tooltip_right(self, text: impl Into<SharedString> + 'static) -> Self {
        self.app_tooltip_with_placement(text.into(), TooltipPlacement::Right)
    }

    fn app_tooltip_with_gap_and_end_inset(
        self,
        text: impl Into<SharedString> + 'static,
        trigger_gap: Pixels,
        horizontal_end_inset: Pixels,
    ) -> Self {
        self.app_tooltip_with_placement_and_gap_and_pin(
            text.into(),
            TooltipPlacement::Top,
            trigger_gap,
            BubbleHorizontalPin::ViewportEndInset(horizontal_end_inset),
        )
    }

    fn app_tooltip_with_placement(self, text: SharedString, placement: TooltipPlacement) -> Self {
        self.app_tooltip_with_placement_and_gap_and_pin(
            text,
            placement,
            TRIGGER_GAP,
            BubbleHorizontalPin::None,
        )
    }

    fn app_tooltip_with_placement_and_gap_and_pin(
        self,
        text: SharedString,
        placement: TooltipPlacement,
        trigger_gap: Pixels,
        bubble_horizontal_pin: BubbleHorizontalPin,
    ) -> Self {
        let trigger_bounds: Rc<Cell<Bounds<gpui::Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let bounds_writer = trigger_bounds.clone();

        self.on_prepaint(move |bounds, _, _| {
            bounds_writer.set(bounds);
        })
        .on_hover({
            let trigger_bounds = trigger_bounds.clone();
            move |hovered, window, cx| {
                let Some(overlay) = global_overlay(cx) else {
                    return;
                };

                if *hovered {
                    let request = TooltipRequest {
                        text: text.clone(),
                        trigger_bounds: trigger_bounds.get(),
                        trigger_window: window.window_handle().window_id(),
                        placement,
                        trigger_gap,
                        bubble_horizontal_pin,
                    };
                    overlay.update(cx, |overlay, cx| overlay.request_show(request, window, cx));
                } else {
                    overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
                }
            }
        })
        .on_scroll_wheel(move |_, _, cx| {
            if let Some(overlay) = global_overlay(cx) {
                overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
            }
        })
        // Dismiss during capture so a descendant that stops the bubble phase
        // cannot leave this app-owned overlay behind while removing its owner.
        .capture_any_mouse_down(move |_, _, cx| {
            if let Some(overlay) = global_overlay(cx) {
                overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
            }
        })
        // Hover state can be recomputed while a pressed control rerenders.
        // Dismiss again at release before an activation removes the trigger.
        .capture_any_mouse_up(move |_, _, cx| {
            if let Some(overlay) = global_overlay(cx) {
                overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
            }
        })
        // Keyboard activation has no pointer event to dismiss its tooltip.
        .capture_key_down(move |_, _, cx| {
            if let Some(overlay) = global_overlay(cx) {
                overlay.update(cx, |overlay, cx| overlay.request_hide(cx));
            }
        })
    }
}

impl<E> AppTooltipExt for E where E: StatefulInteractiveElement + gpui_component::ElementExt {}
#[cfg(test)]
mod tests {
    use super::{
        APP_TOOLTIP_SHOW_DELAY, AppTooltipExt, AppTooltipOverlay, BubbleHorizontalPin,
        TOOLTIP_PADDING_X, TRIGGER_GAP, TooltipPlacement, TooltipRequest, VisibleTooltip,
    };
    use gpui::{
        AnyWindowHandle, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement,
        IntoElement, KeyDownEvent, Modifiers, MouseButton, ParentElement, Render, Role,
        StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window, WindowId,
        div, point, prelude::FluentBuilder, px,
    };
    use gpui_component::Root;

    struct RemovingTooltipOwner {
        overlay: Entity<AppTooltipOverlay>,
        owner_visible: bool,
        re_request_on_mouse_down: bool,
        focus: FocusHandle,
    }

    impl Render for RemovingTooltipOwner {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = cx.entity().downgrade();
            let key_owner = owner.clone();
            let mouse_overlay = self.overlay.clone();
            let re_request_on_mouse_down = self.re_request_on_mouse_down;
            div()
                .size_full()
                .when(self.owner_visible, |this| {
                    this.child(
                        div()
                            .id("removing-tooltip-owner")
                            .debug_selector(|| "removing-tooltip-owner".into())
                            .size(px(40.))
                            .focusable()
                            .track_focus(&self.focus)
                            .tab_stop(true)
                            .role(Role::Button)
                            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                                if crate::tab_keyboard::is_activation_key(
                                    event.keystroke.key.as_str(),
                                ) {
                                    window.prevent_default();
                                    key_owner
                                        .update(cx, |owner, cx| {
                                            owner.owner_visible = false;
                                            cx.notify();
                                        })
                                        .ok();
                                }
                            })
                            .app_tooltip("Remove owner")
                            .child(
                                div()
                                    .id("removing-tooltip-child")
                                    .debug_selector(|| "removing-tooltip-child".into())
                                    .size_full()
                                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                        cx.stop_propagation();
                                        if re_request_on_mouse_down {
                                            mouse_overlay.update(cx, |overlay, cx| {
                                                overlay.request_show(
                                                    TooltipRequest {
                                                        text: "Remove owner".into(),
                                                        trigger_bounds: Bounds::default(),
                                                        trigger_window: window
                                                            .window_handle()
                                                            .window_id(),
                                                        placement: TooltipPlacement::Top,
                                                        trigger_gap: TRIGGER_GAP,
                                                        bubble_horizontal_pin:
                                                            BubbleHorizontalPin::None,
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    })
                                    .on_click(move |_, _, cx| {
                                        owner
                                            .update(cx, |owner, cx| {
                                                owner.owner_visible = false;
                                                cx.notify();
                                            })
                                            .ok();
                                    }),
                            ),
                    )
                })
                .child(self.overlay.clone())
        }
    }

    #[test]
    fn app_tooltip_uses_the_contract_delay() {
        assert_eq!(
            APP_TOOLTIP_SHOW_DELAY,
            std::time::Duration::from_millis(500)
        );
    }

    #[test]
    fn close_player_uses_standard_tooltip_horizontal_padding() {
        assert_eq!(TOOLTIP_PADDING_X, 9.);
    }

    #[test]
    fn tooltip_bubble_pin_contract_distinguishes_close_player_from_standard_tooltips() {
        assert_ne!(
            BubbleHorizontalPin::ViewportEndInset(px(5.)),
            BubbleHorizontalPin::None
        );
        assert_eq!(BubbleHorizontalPin::None, BubbleHorizontalPin::None);
        assert_eq!(
            BubbleHorizontalPin::ViewportEndInset(px(5.)),
            BubbleHorizontalPin::ViewportEndInset(px(5.))
        );
    }

    #[test]
    fn dialog_scoping_allows_inside_triggers_and_suppresses_outside() {
        use gpui::{point, size};

        use super::trigger_inside_top_dialog;

        let dialog = Bounds::new(point(px(100.), px(100.)), size(px(200.), px(200.)));
        let inside = Bounds::new(point(px(150.), px(150.)), size(px(10.), px(10.)));
        let outside = Bounds::new(point(px(10.), px(10.)), size(px(10.), px(10.)));
        assert!(trigger_inside_top_dialog(inside, Some(dialog)));
        assert!(!trigger_inside_top_dialog(outside, Some(dialog)));
        // Unknown dialog bounds and empty triggers count as outside so stale
        // background tooltips stay suppressed until the dialog is measured.
        assert!(!trigger_inside_top_dialog(inside, None));
        assert!(!trigger_inside_top_dialog(Bounds::default(), Some(dialog)));
    }

    #[gpui::test]
    fn descendant_removing_tooltip_owner_clears_visible_overlay(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let overlay = cx.update(|cx| {
            let overlay = cx.new(|_| AppTooltipOverlay::new());
            super::set_global(cx, &overlay);
            overlay
        });
        let window = cx.add_window({
            let overlay = overlay.clone();
            move |window, cx| {
                let view = cx.new(|cx| RemovingTooltipOwner {
                    overlay,
                    owner_visible: true,
                    re_request_on_mouse_down: false,
                    focus: cx.focus_handle(),
                });
                Root::new(view, window, cx)
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let child = visual
            .debug_bounds("removing-tooltip-child")
            .expect("tooltip child bounds");
        visual.update(|_, cx| {
            overlay.update(cx, |overlay, cx| {
                overlay.generation = 1;
                overlay.visible = Some(VisibleTooltip {
                    request: TooltipRequest {
                        text: "Remove owner".into(),
                        trigger_bounds: Bounds::default(),
                        trigger_window: WindowId::from(0),
                        placement: TooltipPlacement::Top,
                        trigger_gap: TRIGGER_GAP,
                        bubble_horizontal_pin: BubbleHorizontalPin::None,
                    },
                    generation: 1,
                });
                cx.notify();
            });
        });
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(visual.read(|cx| overlay.read(cx).visible.is_some()));

        visual.simulate_click(child.center(), Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(visual.debug_bounds("removing-tooltip-owner").is_none());
        assert!(visual.read(|cx| overlay.read(cx).visible.is_none()));
    }

    #[gpui::test]
    fn mouse_up_clears_a_tooltip_re_requested_during_activation(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let overlay = cx.update(|cx| {
            let overlay = cx.new(|_| AppTooltipOverlay::new());
            super::set_global(cx, &overlay);
            overlay
        });
        let window = cx.add_window({
            let overlay = overlay.clone();
            move |window, cx| {
                let view = cx.new(|cx| RemovingTooltipOwner {
                    overlay,
                    owner_visible: true,
                    re_request_on_mouse_down: true,
                    focus: cx.focus_handle(),
                });
                Root::new(view, window, cx)
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let child = visual
            .debug_bounds("removing-tooltip-child")
            .expect("tooltip child bounds");
        visual.simulate_mouse_down(child.center(), MouseButton::Left, Modifiers::default());
        assert!(visual.read(|cx| overlay.read(cx).pending.is_some()));

        visual.simulate_mouse_up(child.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(visual.debug_bounds("removing-tooltip-owner").is_none());
        assert!(visual.read(|cx| {
            let overlay = overlay.read(cx);
            overlay.pending.is_none() && overlay.visible.is_none()
        }));
    }

    #[gpui::test]
    fn keyboard_activation_removing_tooltip_owner_clears_overlay(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let overlay = cx.update(|cx| {
            let overlay = cx.new(|_| AppTooltipOverlay::new());
            super::set_global(cx, &overlay);
            overlay
        });
        let window = cx.add_window({
            let overlay = overlay.clone();
            move |window, cx| {
                let view = cx.new(|cx| RemovingTooltipOwner {
                    overlay,
                    owner_visible: true,
                    re_request_on_mouse_down: false,
                    focus: cx.focus_handle(),
                });
                let focus = view.read(cx).focus.clone();
                focus.focus(window, cx);
                Root::new(view, window, cx)
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        visual.update(|_, cx| {
            overlay.update(cx, |overlay, cx| {
                overlay.generation = 1;
                overlay.visible = Some(VisibleTooltip {
                    request: TooltipRequest {
                        text: "Remove owner".into(),
                        trigger_bounds: Bounds::default(),
                        trigger_window: WindowId::from(0),
                        placement: TooltipPlacement::Top,
                        trigger_gap: TRIGGER_GAP,
                        bubble_horizontal_pin: BubbleHorizontalPin::None,
                    },
                    generation: 1,
                });
                cx.notify();
            });
        });
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert!(visual.read(|cx| overlay.read(cx).visible.is_some()));

        visual.simulate_keystrokes("enter");
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(visual.debug_bounds("removing-tooltip-owner").is_none());
        assert!(visual.read(|cx| overlay.read(cx).visible.is_none()));
    }
    #[gpui::test]
    fn a_tooltip_triggered_in_a_second_window_survives_first_window_renders(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let overlay = cx.update(|cx| {
            let overlay = cx.new(|_| AppTooltipOverlay::new());
            super::set_global(cx, &overlay);
            overlay
        });

        let first = cx.add_window({
            let overlay = overlay.clone();
            move |window, cx| {
                let view = cx.new(|_| TooltipTriggerHost {
                    overlay: overlay.clone(),
                });
                let _ = window;
                Root::new(view, window, cx)
            }
        });
        let second = cx.add_window({
            let overlay = overlay.clone();
            move |window, cx| {
                let view = cx.new(|_| TooltipTriggerHost {
                    overlay: overlay.clone(),
                });
                Root::new(view, window, cx)
            }
        });

        let mut first_visual = VisualTestContext::from_window(AnyWindowHandle::from(first), cx);
        let mut second_visual = VisualTestContext::from_window(AnyWindowHandle::from(second), cx);
        first_visual.run_until_parked();
        second_visual.run_until_parked();
        second_visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        // Hover the trigger in the second window (the popout stand-in).
        second_visual.simulate_mouse_move(point(px(20.), px(20.)), None, Modifiers::default());
        second_visual.run_until_parked();
        let pending_set = second_visual.read(|cx| overlay.read(cx).pending.is_some());
        assert!(pending_set, "hover never set a pending tooltip");

        // Interleave renders from the first window (different viewport)
        // while the show delay is pending, then let the delay elapse.
        first_visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        cx.executor().advance_clock(APP_TOOLTIP_SHOW_DELAY * 2);
        second_visual.run_until_parked();
        second_visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        second_visual.run_until_parked();
        let before_first_render = second_visual.read(|cx| overlay.read(cx).visible.is_some());

        first_visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        second_visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        second_visual.run_until_parked();
        let after_first_render = second_visual.read(|cx| overlay.read(cx).visible.is_some());

        assert!(
            before_first_render,
            "tooltip never became visible even without first-window renders"
        );
        assert!(
            after_first_render,
            "tooltip triggered in the second window must survive first-window renders"
        );
    }

    struct TooltipTriggerHost {
        overlay: Entity<AppTooltipOverlay>,
    }

    impl Render for TooltipTriggerHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let overlay = self.overlay.clone();
            div().size_full().child(
                div()
                    .id("second-window-trigger")
                    .size(px(40.))
                    .top(px(10.))
                    .left(px(10.))
                    .on_hover(move |hovered, window, cx| {
                        if *hovered {
                            let request = TooltipRequest {
                                text: "Second window".into(),
                                trigger_bounds: Bounds::new(
                                    point(px(10.), px(10.)),
                                    gpui::size(px(40.), px(40.)),
                                ),
                                trigger_window: window.window_handle().window_id(),
                                placement: TooltipPlacement::Top,
                                trigger_gap: TRIGGER_GAP,
                                bubble_horizontal_pin: BubbleHorizontalPin::None,
                            };
                            overlay.update(cx, |overlay, cx| {
                                overlay.request_show(request, window, cx);
                            });
                        }
                    })
                    .child(self.overlay.clone()),
            )
        }
    }
}
