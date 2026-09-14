use std::{cell::RefCell, rc::Rc};

use gpui::{
    Anchor, AnyElement, App, Context, DismissEvent, Element, ElementId, Entity, Focusable,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, StyleRefinement, Styled, Subscription, Window,
    anchored, deferred, div, point, prelude::*, px,
};

use super::{ContextMenuLease, PopupMenu, PopupMenuArrowEdge};

type ContextMenuBuilder = Rc<dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu>;

/// Gap between the trigger top edge and the menu bottom edge when the
/// menu is placed above its trigger, leaving room for the chevron.
pub(crate) const CONTEXT_MENU_ABOVE_GAP_PX: f32 = 8.;
/// Gap between the trigger bottom edge and the menu top edge when the
/// menu is placed below its trigger, leaving room for the chevron.
pub(crate) const CONTEXT_MENU_BELOW_GAP_PX: f32 = 8.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContextMenuPlacement {
    Pointer,
    Above,
    Below,
}

impl ContextMenuPlacement {
    fn arrow_edge(self) -> Option<PopupMenuArrowEdge> {
        match self {
            Self::Pointer => None,
            Self::Above => Some(PopupMenuArrowEdge::Bottom),
            Self::Below => Some(PopupMenuArrowEdge::Top),
        }
    }
}

/// Adds an app-owned context popup while retaining the original gpui
/// component builder callback shape.
pub(crate) trait ContextMenuExt: InteractiveElement + ParentElement + Styled {
    fn context_menu(
        mut self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> ContextMenu<Self>
    where
        Self: Sized,
    {
        let id = self
            .interactivity()
            .element_id
            .clone()
            .map(|id| format!("app-context-menu-{id:?}"))
            .unwrap_or_else(|| format!("app-context-menu-{:p}", &self as *const _));
        ContextMenu::new(id, self).menu(f)
    }
}

impl<E: InteractiveElement + ParentElement + Styled> ContextMenuExt for E {}

pub(crate) struct ContextMenu<E: ParentElement + Styled + Sized> {
    id: ElementId,
    element: Option<E>,
    menu: Option<ContextMenuBuilder>,
    ignore_style: StyleRefinement,
    anchor: Anchor,
    placement: ContextMenuPlacement,
    trigger_button: MouseButton,
}

impl<E: ParentElement + Styled> ContextMenu<E> {
    pub(crate) fn new(id: impl Into<ElementId>, element: E) -> Self {
        Self {
            id: id.into(),
            element: Some(element),
            menu: None,
            ignore_style: StyleRefinement::default(),
            anchor: Anchor::TopLeft,
            placement: ContextMenuPlacement::Pointer,
            trigger_button: MouseButton::Right,
        }
    }

    pub(crate) fn open_on(mut self, button: MouseButton) -> Self {
        self.trigger_button = button;
        self
    }

    /// Place the menu above the trigger so its bottom edge sits against
    /// the trigger top edge, with the chevron pointing down to the trigger.
    pub(crate) fn place_above(mut self) -> Self {
        self.anchor = Anchor::BottomCenter;
        self.placement = ContextMenuPlacement::Above;
        self
    }

    /// Place the menu below the trigger so its top edge sits against
    /// the trigger bottom edge, with the chevron pointing up to the trigger.
    pub(crate) fn place_below(mut self) -> Self {
        self.anchor = Anchor::TopCenter;
        self.placement = ContextMenuPlacement::Below;
        self
    }

    pub(crate) fn above_position(trigger: gpui::Bounds<Pixels>) -> Point<Pixels> {
        point(
            trigger.center().x,
            trigger.top() - px(CONTEXT_MENU_ABOVE_GAP_PX),
        )
    }

    pub(crate) fn below_position(trigger: gpui::Bounds<Pixels>) -> Point<Pixels> {
        point(
            trigger.center().x,
            trigger.bottom() + px(CONTEXT_MENU_BELOW_GAP_PX),
        )
    }

    fn menu(
        mut self,
        builder: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> Self {
        self.menu = Some(Rc::new(builder));
        self
    }

    fn with_element_state<R>(
        &mut self,
        id: &GlobalElementId,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&mut Self, &mut ContextMenuState, &mut Window, &mut App) -> R,
    ) -> R {
        window.with_optional_element_state::<ContextMenuState, _>(
            Some(id),
            |element_state, window| {
                let mut element_state = element_state.unwrap().unwrap_or_default();
                let result = f(self, &mut element_state, window, cx);
                (result, Some(element_state))
            },
        )
    }
}

impl<E: ParentElement + Styled> ParentElement for ContextMenu<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: ParentElement + Styled> Styled for ContextMenu<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element
            .as_mut()
            .map(Styled::style)
            .unwrap_or(&mut self.ignore_style)
    }
}

struct ContextMenuSharedState {
    menu_view: Option<Entity<PopupMenu>>,
    lease: Option<ContextMenuLease>,
    position: Point<Pixels>,
    subscription: Option<Subscription>,
}

pub(crate) struct ContextMenuState {
    element: Option<AnyElement>,
    shared_state: Rc<RefCell<ContextMenuSharedState>>,
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self {
            element: None,
            shared_state: Rc::new(RefCell::new(ContextMenuSharedState {
                menu_view: None,
                lease: None,
                position: Default::default(),
                subscription: None,
            })),
        }
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> IntoElement for ContextMenu<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: ParentElement + Styled + IntoElement + 'static> Element for ContextMenu<E> {
    type RequestLayoutState = ContextMenuState;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let anchor = self.anchor;
        self.with_element_state(
            id.expect("context menu element id"),
            window,
            cx,
            |this, state, window, cx| {
                let (position, open) = {
                    let shared = state.shared_state.borrow();
                    (shared.position, shared.lease.is_some())
                };
                let menu_view = state.shared_state.borrow().menu_view.clone();
                let menu_element = if open {
                    let has_items = menu_view
                        .as_ref()
                        .is_some_and(|menu| !menu.read(cx).is_empty());
                    has_items.then(|| {
                        deferred(
                            anchored().child(
                                div()
                                    .w(window.bounds().size.width)
                                    .h(window.bounds().size.height)
                                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                    .child(
                                        anchored()
                                            .position(position)
                                            .anchor(anchor)
                                            .snap_to_window_with_margin(px(8.))
                                            .when_some(menu_view, |this, menu| {
                                                if !menu
                                                    .focus_handle(cx)
                                                    .contains_focused(window, cx)
                                                {
                                                    menu.focus_handle(cx).focus(window, cx);
                                                }
                                                this.child(menu)
                                            }),
                                    ),
                            ),
                        )
                        .with_priority(1)
                        .into_any()
                    })
                } else {
                    None
                };

                let mut element = this
                    .element
                    .take()
                    .expect("context menu element should exist")
                    .children(menu_element)
                    .into_any_element();
                let layout_id = element.request_layout(window, cx);
                let shared_state = state.shared_state.clone();
                (
                    layout_id,
                    ContextMenuState {
                        element: Some(element),
                        shared_state,
                    },
                )
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(element) = &mut state.element {
            element.prepaint(window, cx);
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(element) = &mut state.element {
            element.paint(window, cx);
        }

        let builder = self.menu.clone();
        let trigger_button = self.trigger_button;
        let placement = self.placement;
        self.with_element_state(
            id.expect("context menu element id"),
            window,
            cx,
            |_this, state, window, _cx| {
                let shared_state = state.shared_state.clone();
                let hitbox = hitbox.clone();
                window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, window, cx| {
                    if phase.bubble() && event.button == trigger_button && hitbox.is_hovered(window)
                    {
                        if shared_state.borrow().lease.is_some()
                            || shared_state.borrow().menu_view.is_some()
                        {
                            return;
                        }
                        {
                            let mut shared = shared_state.borrow_mut();
                            shared.menu_view = None;
                            shared.subscription = None;
                            shared.position = match placement {
                                ContextMenuPlacement::Pointer => event.position,
                                ContextMenuPlacement::Above => Self::above_position(hitbox.bounds),
                                ContextMenuPlacement::Below => Self::below_position(hitbox.bounds),
                            };
                        }
                        window.defer(cx, {
                            let shared_state = Rc::downgrade(&shared_state);
                            let builder = builder.clone();
                            move |window, cx| {
                                let Some(shared_state) = shared_state.upgrade() else {
                                    return;
                                };
                                let arrow_edge = placement.arrow_edge();
                                let anchor_x = arrow_edge.map(|_| shared_state.borrow().position.x);
                                let menu = PopupMenu::build(window, cx, move |menu, window, cx| {
                                    if let Some(builder) = builder.as_ref() {
                                        builder(menu, window, cx)
                                    } else {
                                        menu
                                    }
                                });
                                if menu.read(cx).is_empty() {
                                    window.refresh();
                                    return;
                                }
                                if shared_state.borrow().lease.is_some()
                                    || shared_state.borrow().menu_view.is_some()
                                {
                                    return;
                                }
                                menu.update(cx, |menu, _| {
                                    menu.set_arrow_visible(arrow_edge.is_some());
                                    if let Some(arrow_edge) = arrow_edge {
                                        menu.set_arrow_edge(arrow_edge);
                                    }
                                    if let Some(anchor_x) = anchor_x {
                                        menu.set_arrow_anchor(anchor_x);
                                    }
                                });
                                let subscription = window.subscribe(&menu, cx, {
                                    let shared_state = Rc::downgrade(&shared_state);
                                    move |_, _: &DismissEvent, window, _| {
                                        let Some(shared_state) = shared_state.upgrade() else {
                                            return;
                                        };
                                        let mut shared = shared_state.borrow_mut();
                                        shared.menu_view = None;
                                        shared.subscription = None;
                                        shared.lease.take();
                                        window.refresh();
                                    }
                                });
                                let mut shared = shared_state.borrow_mut();
                                shared.lease = Some(ContextMenuLease::acquire());
                                shared.menu_view = Some(menu);
                                shared.subscription = Some(subscription);
                                window.refresh();
                            }
                        });
                    }
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_menu_defaults_to_right_click_and_can_use_left_click() {
        let default_menu = ContextMenu::new("default-context-menu", div());
        assert_eq!(default_menu.trigger_button, MouseButton::Right);
        assert_eq!(default_menu.anchor, Anchor::TopLeft);

        let left_menu = ContextMenu::new("left-context-menu", div()).open_on(MouseButton::Left);
        assert_eq!(left_menu.trigger_button, MouseButton::Left);
        assert_eq!(left_menu.anchor, Anchor::TopLeft);
    }

    #[test]
    fn place_above_anchors_bottom_center_against_trigger_top() {
        let menu = ContextMenu::new("above-menu", div()).place_above();
        assert_eq!(menu.placement, ContextMenuPlacement::Above);
        assert_eq!(menu.anchor, Anchor::BottomCenter);
        assert_eq!(CONTEXT_MENU_ABOVE_GAP_PX, 8.);
    }

    #[test]
    fn above_position_centers_on_trigger_with_gap() {
        let trigger = gpui::Bounds::new(point(px(100.), px(200.)), gpui::size(px(34.), px(34.)));
        let position = ContextMenu::<gpui::Div>::above_position(trigger);
        assert_eq!(position.x, px(117.));
        assert_eq!(position.y, px(192.));
    }

    #[test]
    fn place_below_anchors_top_center_against_trigger_bottom() {
        let menu = ContextMenu::new("below-menu", div()).place_below();
        assert_eq!(menu.placement, ContextMenuPlacement::Below);
        assert_eq!(menu.anchor, Anchor::TopCenter);
        assert_eq!(CONTEXT_MENU_BELOW_GAP_PX, 8.);

        let trigger = gpui::Bounds::new(point(px(100.), px(200.)), gpui::size(px(34.), px(34.)));
        let position = ContextMenu::<gpui::Div>::below_position(trigger);
        assert_eq!(position.x, px(117.));
        assert_eq!(position.y, px(242.));
    }

    #[test]
    fn tearing_down_context_menu_state_releases_its_lease() {
        let baseline = super::super::open_context_menu_count();
        let state = ContextMenuState::default();
        state.shared_state.borrow_mut().lease = Some(ContextMenuLease::acquire());
        assert_eq!(super::super::open_context_menu_count(), baseline + 1);
        drop(state);
        assert_eq!(super::super::open_context_menu_count(), baseline);
    }
}
