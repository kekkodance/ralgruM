use std::rc::Rc;

use gpui::{
    App, AppContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    KeyBinding, Render, ScrollHandle, SharedString, Subscription, Window, prelude::*,
};
use gpui_component::{Icon, Side};

use crate::browser_scroll::BrowserScrollState;

use super::popup_actions::{
    Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
};

const POPUP_CONTEXT: &str = "PopupMenu";

/// Bindings used by the app-owned popup menu. They intentionally use the same
/// key context as gpui-component so the two menu implementations remain
/// interchangeable to callers.
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", Confirm, Some(POPUP_CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(POPUP_CONTEXT)),
        KeyBinding::new("up", SelectUp, Some(POPUP_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(POPUP_CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(POPUP_CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(POPUP_CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(POPUP_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(POPUP_CONTEXT)),
    ]);
}

pub(crate) enum PopupMenuItem {
    Separator,
    Item {
        icon: Option<Icon>,
        label: SharedString,
        shortcut: Option<SharedString>,
        disabled: bool,
        handler: Option<Rc<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>>,
    },
    ElementItem {
        icon: Option<Icon>,
        disabled: bool,
        dynamic_disabled: Option<Rc<dyn Fn(&mut App) -> bool>>,
        render_disabled: bool,
        render: Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static>,
        handler: Option<Rc<dyn Fn(&gpui::ClickEvent, &mut Window, &mut App)>>,
    },
    Submenu {
        icon: Option<Icon>,
        label: SharedString,
        disabled: bool,
        menu: Entity<PopupMenu>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PopupMenuArrowEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl PopupMenuItem {
    pub(crate) fn new(label: impl Into<SharedString>) -> Self {
        Self::Item {
            icon: None,
            label: label.into(),
            shortcut: None,
            disabled: false,
            handler: None,
        }
    }

    pub(crate) fn element<F, E>(builder: F) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        Self::ElementItem {
            icon: None,
            disabled: false,
            dynamic_disabled: None,
            render_disabled: true,
            render: Box::new(move |window, cx| builder(window, cx).into_any_element()),
            handler: None,
        }
    }

    pub(crate) fn submenu(label: impl Into<SharedString>, menu: Entity<PopupMenu>) -> Self {
        Self::Submenu {
            icon: None,
            label: label.into(),
            disabled: false,
            menu,
        }
    }

    pub(crate) fn separator() -> Self {
        Self::Separator
    }

    pub(crate) fn icon(mut self, icon: impl Into<Icon>) -> Self {
        match &mut self {
            Self::Item { icon: target, .. }
            | Self::ElementItem { icon: target, .. }
            | Self::Submenu { icon: target, .. } => *target = Some(icon.into()),
            Self::Separator => {}
        }
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        if let Self::Item {
            shortcut: target, ..
        } = &mut self
        {
            *target = Some(shortcut.into());
        }
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        match &mut self {
            Self::Item {
                disabled: target, ..
            }
            | Self::ElementItem {
                disabled: target, ..
            }
            | Self::Submenu {
                disabled: target, ..
            } => *target = disabled,
            Self::Separator => {}
        }
        self
    }

    pub(crate) fn disabled_when<F>(mut self, disabled: F) -> Self
    where
        F: Fn(&mut App) -> bool + 'static,
    {
        if let Self::ElementItem {
            dynamic_disabled, ..
        } = &mut self
        {
            *dynamic_disabled = Some(Rc::new(disabled));
        }
        self
    }

    /// Keep an element non-interactive without applying disabled opacity to
    /// its own reactive children.
    pub(crate) fn disabled_visual(mut self, visible: bool) -> Self {
        if let Self::ElementItem {
            render_disabled, ..
        } = &mut self
        {
            *render_disabled = visible;
        }
        self
    }

    pub(crate) fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    {
        match &mut self {
            Self::Item {
                handler: target, ..
            }
            | Self::ElementItem {
                handler: target, ..
            } => *target = Some(Rc::new(handler)),
            Self::Separator | Self::Submenu { .. } => {}
        }
        self
    }

    fn is_clickable(&self) -> bool {
        match self {
            Self::Item { disabled, .. }
            | Self::ElementItem { disabled, .. }
            | Self::Submenu { disabled, .. } => !disabled,
            Self::Separator => false,
        }
    }

    pub(super) fn is_disabled(&self, cx: &mut App) -> bool {
        match self {
            Self::Item { disabled, .. } | Self::Submenu { disabled, .. } => *disabled,
            Self::ElementItem {
                disabled,
                dynamic_disabled,
                ..
            } => {
                *disabled
                    || dynamic_disabled
                        .as_ref()
                        .is_some_and(|disabled| disabled(cx))
            }
            Self::Separator => true,
        }
    }

    pub(super) fn a11y_label(&self) -> Option<SharedString> {
        match self {
            Self::Item { label, .. } | Self::Submenu { label, .. } => Some(label.clone()),
            Self::Separator | Self::ElementItem { .. } => None,
        }
    }
}

pub(crate) struct PopupMenu {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) menu_items: Vec<PopupMenuItem>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) min_width: Option<gpui::Pixels>,
    pub(crate) max_width: Option<gpui::Pixels>,
    pub(crate) max_height: Option<gpui::Pixels>,
    pub(crate) bounds: gpui::Bounds<gpui::Pixels>,
    pub(crate) parent_menu: Option<gpui::WeakEntity<Self>>,
    pub(crate) scrollable: bool,
    pub(crate) select_first_enabled_on_open: bool,
    pub(crate) scroll_handle: ScrollHandle,
    pub(crate) browser_scroll: BrowserScrollState,
    pub(crate) submenu_anchor: (gpui::Anchor, gpui::Pixels),
    pub(crate) show_arrow: bool,
    pub(crate) arrow_edge: PopupMenuArrowEdge,
    pub(crate) arrow_anchor_x: Option<gpui::Pixels>,
    pub(crate) closing: bool,
    pub(crate) close_epoch: u64,
    pub(crate) _subscriptions: Vec<Subscription>,
}

/// Keep an anchored chevron inside the rounded menu corners. Radius is 8px
/// and the chevron canvas is 12px wide, plus 2px of breathing room.
pub(crate) const POPUP_ARROW_EDGE_INSET_PX: f32 = 16.;

impl PopupMenu {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            menu_items: Vec::new(),
            selected_index: None,
            min_width: None,
            max_width: None,
            max_height: None,
            bounds: Default::default(),
            parent_menu: None,
            scrollable: false,
            select_first_enabled_on_open: false,
            scroll_handle: ScrollHandle::default(),
            browser_scroll: BrowserScrollState::for_context_menu(),
            submenu_anchor: (gpui::Anchor::TopLeft, gpui::Pixels::ZERO),
            show_arrow: false,
            arrow_edge: PopupMenuArrowEdge::Bottom,
            arrow_anchor_x: None,
            closing: false,
            close_epoch: 0,
            _subscriptions: Vec::new(),
        }
    }

    pub(crate) fn build(
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let mut menu = f(Self::new(cx), window, cx);
            menu.selected_index =
                initial_selected_index(&menu.menu_items, menu.select_first_enabled_on_open);
            menu
        })
    }

    pub(crate) fn min_w(mut self, width: impl Into<gpui::Pixels>) -> Self {
        self.min_width = Some(width.into());
        self
    }

    pub(crate) fn max_w(mut self, width: impl Into<gpui::Pixels>) -> Self {
        self.max_width = Some(width.into());
        self
    }

    pub(crate) fn max_h(mut self, height: impl Into<gpui::Pixels>) -> Self {
        self.max_height = Some(height.into());
        self
    }

    pub(crate) fn scrollable(mut self, scrollable: bool) -> Self {
        self.scrollable = scrollable;
        self
    }

    pub(crate) fn select_first_enabled_on_open(mut self) -> Self {
        self.select_first_enabled_on_open = true;
        self
    }

    /// Show a small chevron on the bottom edge for menus placed above
    /// their trigger, pointing down toward the trigger button.
    pub(crate) fn with_arrow(mut self) -> Self {
        self.show_arrow = true;
        self.arrow_edge = PopupMenuArrowEdge::Bottom;
        self
    }

    /// Show a small chevron on the top edge for menus placed below
    /// their trigger, pointing up toward the trigger button.
    pub(crate) fn with_top_arrow(mut self) -> Self {
        self.show_arrow = true;
        self.arrow_edge = PopupMenuArrowEdge::Top;
        self
    }

    pub(crate) fn set_arrow_visible(&mut self, visible: bool) {
        self.show_arrow = visible;
    }

    pub(crate) fn set_arrow_edge(&mut self, edge: PopupMenuArrowEdge) {
        self.arrow_edge = edge;
        self.show_arrow = true;
    }

    /// Pin the chevron to a window cross-axis coordinate, usually the trigger
    /// center. The renderer clamps the chevron inside the menu so snapped
    /// menus still point at their button instead of their own center.
    pub(crate) fn set_arrow_anchor(&mut self, cross_axis: gpui::Pixels) {
        self.arrow_anchor_x = Some(cross_axis);
    }

    /// Offset from the menu edge along its cross-axis to the chevron center,
    /// clamped inside the menu. Returns None until the menu has a measured
    /// size.
    pub(crate) fn arrow_center_offset(&self) -> Option<gpui::Pixels> {
        let anchor_x = self.arrow_anchor_x?;
        let (origin, extent) = match self.arrow_edge {
            PopupMenuArrowEdge::Top | PopupMenuArrowEdge::Bottom => {
                (self.bounds.origin.x, self.bounds.size.width)
            }
            PopupMenuArrowEdge::Left | PopupMenuArrowEdge::Right => {
                (self.bounds.origin.y, self.bounds.size.height)
            }
        };
        if extent <= gpui::Pixels::ZERO {
            return None;
        }
        let target = (anchor_x - origin).as_f32();
        let min = POPUP_ARROW_EDGE_INSET_PX;
        let max = (extent.as_f32() - POPUP_ARROW_EDGE_INSET_PX).max(min);
        Some(gpui::px(target.clamp(min, max)))
    }

    pub(crate) fn item(mut self, item: impl Into<PopupMenuItem>) -> Self {
        self.menu_items.push(item.into());
        self
    }

    pub(crate) fn separator(mut self) -> Self {
        if !matches!(self.menu_items.last(), Some(PopupMenuItem::Separator)) {
            self.menu_items.push(PopupMenuItem::separator());
        }
        self
    }

    pub(crate) fn submenu_with_icon(
        mut self,
        icon: Option<Icon>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl Fn(Self, &mut Window, &mut Context<Self>) -> Self + 'static,
    ) -> Self {
        let submenu = Self::build(window, cx, f);
        let parent_menu = cx.entity().downgrade();
        submenu.update(cx, |submenu, _| {
            submenu.parent_menu = Some(parent_menu);
        });
        let item = PopupMenuItem::submenu(label, submenu);
        self.menu_items.push(if let Some(icon) = icon {
            item.icon(icon)
        } else {
            item
        });
        self
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.menu_items.is_empty()
    }

    pub(crate) fn active_submenu(&self) -> Option<Entity<PopupMenu>> {
        let index = self.selected_index?;
        match self.menu_items.get(index) {
            Some(PopupMenuItem::Submenu { menu, .. }) => Some(menu.clone()),
            _ => None,
        }
    }

    pub(crate) fn first_clickable_index(&self) -> Option<usize> {
        self.menu_items
            .iter()
            .enumerate()
            .find_map(|(ix, item)| item.is_clickable().then_some(ix))
    }

    pub(crate) fn update_submenu_anchor(&mut self, window: &Window) {
        let child_width = gpui::px(super::submenu::SUBMENU_WIDTH + super::submenu::SUBMENU_GAP);
        let opens_left = self.bounds.origin.x + self.bounds.size.width + child_width
            > window.viewport_size().width;
        self.submenu_anchor = if opens_left {
            (gpui::Anchor::TopRight, gpui::px(-7.))
        } else {
            (gpui::Anchor::TopLeft, gpui::px(7.))
        };
    }

    pub(crate) fn on_click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        cx.stop_propagation();
        window.prevent_default();
        self.set_selected_index(ix, cx);
        self.confirm(window, cx);
    }

    pub(crate) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let Some(index) = self.selected_index else {
            return;
        };
        if self
            .menu_items
            .get(index)
            .is_none_or(|item| item.is_disabled(cx))
        {
            return;
        }
        let handler = match self.menu_items.get(index) {
            Some(PopupMenuItem::Item { handler, .. })
            | Some(PopupMenuItem::ElementItem { handler, .. }) => handler.clone(),
            _ => None,
        };
        if let Some(handler) = handler {
            handler(&gpui::ClickEvent::default(), window, cx);
        } else {
            return;
        }
        self.dismiss(window, cx);
    }

    pub(crate) fn set_selected_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.selected_index != Some(ix) {
            self.selected_index = Some(ix);
            self.scroll_handle.scroll_to_item(ix);
            cx.notify();
        }
    }

    pub(crate) fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let selected = self.selected_index.unwrap_or(usize::MAX);
        let index = self
            .menu_items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(ix, item)| (ix < selected && item.is_clickable()).then_some(ix))
            .or_else(|| {
                self.menu_items
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(ix, item)| item.is_clickable().then_some(ix))
            });
        if let Some(index) = index {
            self.set_selected_index(index, cx);
        }
    }

    pub(crate) fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(selected) = self.selected_index else {
            if let Some(index) = self.first_clickable_index() {
                self.set_selected_index(index, cx);
            }
            return;
        };
        let index = self
            .menu_items
            .iter()
            .enumerate()
            .find_map(|(ix, item)| (ix > selected && item.is_clickable()).then_some(ix))
            .or_else(|| self.first_clickable_index());
        if let Some(index) = index {
            self.set_selected_index(index, cx);
        }
    }

    pub(crate) fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if let Some(index) = self.first_clickable_index() {
            self.set_selected_index(index, cx);
        }
    }

    pub(crate) fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if let Some(index) = self
            .menu_items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(ix, item)| item.is_clickable().then_some(ix))
        {
            self.set_selected_index(index, cx);
        }
    }

    pub(crate) fn select_left(
        &mut self,
        _: &SelectLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opens_left = matches!(
            self.submenu_anchor.0,
            gpui::Anchor::TopRight | gpui::Anchor::BottomRight
        );
        let handled = if opens_left {
            self.select_submenu(window, cx)
        } else {
            self.unselect_submenu(cx)
        };
        if self.parent_side(cx).is_left() {
            self.focus_parent_menu(window, cx);
        }
        if !handled && self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    pub(crate) fn select_right(
        &mut self,
        _: &SelectRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opens_left = matches!(
            self.submenu_anchor.0,
            gpui::Anchor::TopRight | gpui::Anchor::BottomRight
        );
        let handled = if opens_left {
            self.unselect_submenu(cx)
        } else {
            self.select_submenu(window, cx)
        };
        if self.parent_side(cx).is_right() {
            self.focus_parent_menu(window, cx);
        }
        if !handled && self.parent_menu.is_none() {
            cx.propagate();
        }
    }

    fn select_submenu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.active_submenu() else {
            return false;
        };
        menu.update(cx, |menu, cx| {
            if let Some(index) = menu.first_clickable_index() {
                menu.set_selected_index(index, cx);
            }
            menu.focus_handle.focus(window, cx);
        });
        cx.notify();
        true
    }

    fn unselect_submenu(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(menu) = self.active_submenu() else {
            return false;
        };
        menu.update(cx, |menu, cx| {
            menu.selected_index = None;
            cx.notify();
        });
        true
    }

    fn parent_side(&self, cx: &App) -> Side {
        let Some(parent) = self
            .parent_menu
            .as_ref()
            .and_then(|parent| parent.upgrade())
        else {
            return Side::Left;
        };
        match parent.read(cx).submenu_anchor.0 {
            gpui::Anchor::TopLeft | gpui::Anchor::BottomLeft => Side::Left,
            gpui::Anchor::TopRight | gpui::Anchor::BottomRight => Side::Right,
            _ => Side::Left,
        }
    }

    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if let Some(parent) = self.parent_menu.clone() {
            _ = parent.update(cx, |parent, cx| {
                parent.dismiss(window, cx);
            });
            return;
        }
        // Test builds cannot park the close timer, so delayed dismiss leaks
        // PopupMenu handles and leaves OPEN_CONTEXT_MENUS set, which swallows
        // wheel events in later tests.
        if cfg!(test) || cx.reduce_motion() || crate::motion::INTERACTION_DURATION.is_zero() {
            cx.emit(DismissEvent);
            return;
        }
        self.closing = true;
        self.close_epoch = self.close_epoch.wrapping_add(1);
        self.selected_index = None;
        cx.notify();
        let epoch = self.close_epoch;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(crate::motion::INTERACTION_DURATION)
                .await;
            let _ = this.update_in(cx, |this, _window, cx| {
                if this.closing && this.close_epoch == epoch {
                    cx.emit(DismissEvent);
                }
            });
        })
        .detach();
    }

    pub(crate) fn handle_dismiss(
        &mut self,
        position: &gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.should_dismiss_on_mouse_down_out(position, cx) {
            return;
        }
        self.dismiss(window, cx);
    }

    fn should_dismiss_on_mouse_down_out(
        &self,
        position: &gpui::Point<gpui::Pixels>,
        cx: &App,
    ) -> bool {
        if self.contains_active_descendant(position, cx) {
            return false;
        }
        if let Some(parent) = self
            .parent_menu
            .as_ref()
            .and_then(|parent| parent.upgrade())
            && parent.read(cx).bounds.contains(position)
        {
            return false;
        }
        true
    }

    /// Submenus are rendered in an anchored layer outside their parent's
    /// surface. Keep the root menu alive while the pointer is inside any
    /// active descendant so the child's mouse-up can deliver its click.
    fn contains_active_descendant(&self, position: &gpui::Point<gpui::Pixels>, cx: &App) -> bool {
        let Some(child) = self.active_submenu() else {
            return false;
        };
        let child = child.read(cx);
        child.bounds.contains(position) || child.contains_active_descendant(position, cx)
    }

    fn focus_parent_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent) = self
            .parent_menu
            .as_ref()
            .and_then(|parent| parent.upgrade())
        else {
            return;
        };
        self.selected_index = None;
        parent.update(cx, |parent, cx| {
            parent.focus_handle.focus(window, cx);
            cx.notify();
        });
    }

    pub(crate) fn on_mouse_down_out(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_dismiss(&event.position, window, cx);
    }
}

fn initial_selected_index(items: &[PopupMenuItem], select_first_enabled: bool) -> Option<usize> {
    select_first_enabled.then(|| items.iter().position(PopupMenuItem::is_clickable))?
}

impl EventEmitter<DismissEvent> for PopupMenu {}

impl Focusable for PopupMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PopupMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::popup_render::render_popup(self, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_items_can_disable_activation_without_dimming_children() {
        let item = PopupMenuItem::element(|_, _| gpui::div())
            .disabled(true)
            .disabled_visual(false);

        match item {
            PopupMenuItem::ElementItem {
                disabled,
                render_disabled,
                ..
            } => {
                assert!(disabled);
                assert!(!render_disabled);
            }
            _ => panic!("expected an element menu item"),
        }
    }

    #[test]
    fn shortcut_builder_only_populates_standard_items() {
        let item = PopupMenuItem::new("Copy").shortcut("Ctrl+C");
        match item {
            PopupMenuItem::Item { shortcut, .. } => {
                assert_eq!(shortcut.as_deref(), Some("Ctrl+C"));
            }
            _ => panic!("expected a standard menu item"),
        }

        assert!(matches!(
            PopupMenuItem::separator().shortcut("ignored"),
            PopupMenuItem::Separator
        ));
    }

    #[gpui::test]
    fn arrow_is_opt_in_and_tracks_its_edge(cx: &mut gpui::TestAppContext) {
        let plain = cx.update(|cx| PopupMenu::new(cx));
        assert!(!plain.show_arrow);
        assert!(plain.arrow_anchor_x.is_none());
        let above = cx.update(|cx| PopupMenu::new(cx).with_arrow());
        assert!(above.show_arrow);
        assert_eq!(above.arrow_edge, PopupMenuArrowEdge::Bottom);
        assert!(above.arrow_anchor_x.is_none());
        let below = cx.update(|cx| PopupMenu::new(cx).with_top_arrow());
        assert!(below.show_arrow);
        assert_eq!(below.arrow_edge, PopupMenuArrowEdge::Top);
        assert!(below.arrow_anchor_x.is_none());
    }

    #[gpui::test]
    fn arrow_anchor_offsets_chevron_to_trigger(cx: &mut gpui::TestAppContext) {
        let menu = cx.update(|cx| {
            let mut menu = PopupMenu::new(cx).with_arrow();
            menu.bounds = gpui::Bounds::new(
                gpui::point(gpui::px(100.), gpui::px(50.)),
                gpui::size(gpui::px(254.), gpui::px(120.)),
            );
            menu.set_arrow_anchor(gpui::px(340.));
            menu
        });
        assert_eq!(
            menu.arrow_center_offset(),
            Some(gpui::px(254. - POPUP_ARROW_EDGE_INSET_PX))
        );
    }

    #[gpui::test]
    fn arrow_anchor_clamps_inside_menu(cx: &mut gpui::TestAppContext) {
        let centered = cx.update(|cx| {
            let mut menu = PopupMenu::new(cx).with_arrow();
            menu.bounds = gpui::Bounds::new(
                gpui::point(gpui::px(100.), gpui::px(50.)),
                gpui::size(gpui::px(254.), gpui::px(120.)),
            );
            menu.set_arrow_anchor(gpui::px(227.));
            menu
        });
        assert_eq!(centered.arrow_center_offset(), Some(gpui::px(127.)));

        let far_left = cx.update(|cx| {
            let mut menu = PopupMenu::new(cx).with_arrow();
            menu.bounds = gpui::Bounds::new(
                gpui::point(gpui::px(100.), gpui::px(50.)),
                gpui::size(gpui::px(254.), gpui::px(120.)),
            );
            menu.set_arrow_anchor(gpui::px(0.));
            menu
        });
        assert_eq!(
            far_left.arrow_center_offset(),
            Some(gpui::px(POPUP_ARROW_EDGE_INSET_PX))
        );

        let unmeasured = cx.update(|cx| {
            let mut menu = PopupMenu::new(cx).with_arrow();
            menu.set_arrow_anchor(gpui::px(120.));
            menu
        });
        assert_eq!(unmeasured.arrow_center_offset(), None);
    }

    #[test]
    fn opt_in_initial_selection_uses_first_enabled_item() {
        let items = vec![
            PopupMenuItem::new("Cut").disabled(true),
            PopupMenuItem::separator(),
            PopupMenuItem::new("Copy"),
            PopupMenuItem::new("Paste"),
        ];

        assert_eq!(initial_selected_index(&items, false), None);
        assert_eq!(initial_selected_index(&items, true), Some(2));
    }

    #[test]
    fn dismiss_starts_closing_instead_of_emitting_immediately() {
        let source = include_str!("popup.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("popup production source");
        assert!(source.contains("closing: bool"));
        assert!(source.contains("close_epoch: u64"));
        assert!(source.contains("if self.closing"));
        assert!(source.contains("self.closing = true"));
        assert!(source.contains("crate::motion::INTERACTION_DURATION"));
        assert!(source.contains("contains_active_descendant"));
        assert!(source.contains("should_dismiss_on_mouse_down_out(position, cx)"));
    }

    #[gpui::test]
    fn active_descendant_bounds_keep_root_open_but_outside_click_dismisses(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));
        let child = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));
        let grandchild = cx.update(|cx| cx.new(|cx| PopupMenu::new(cx)));

        cx.update(|cx| {
            root.update(cx, |root, _| {
                root.menu_items
                    .push(PopupMenuItem::submenu("Child", child.clone()));
                root.selected_index = Some(0);
                root.bounds = gpui::Bounds::new(
                    gpui::point(gpui::px(10.), gpui::px(10.)),
                    gpui::size(gpui::px(100.), gpui::px(100.)),
                );
            });
            child.update(cx, |child, _| {
                child
                    .menu_items
                    .push(PopupMenuItem::submenu("Grandchild", grandchild.clone()));
                child.selected_index = Some(0);
                child.parent_menu = Some(root.downgrade());
                child.bounds = gpui::Bounds::new(
                    gpui::point(gpui::px(120.), gpui::px(20.)),
                    gpui::size(gpui::px(100.), gpui::px(100.)),
                );
            });
            grandchild.update(cx, |grandchild, _| {
                grandchild.bounds = gpui::Bounds::new(
                    gpui::point(gpui::px(230.), gpui::px(30.)),
                    gpui::size(gpui::px(100.), gpui::px(100.)),
                );
            });
        });

        cx.update(|cx| {
            let root_read = root.read(cx);
            assert!(
                !root_read.should_dismiss_on_mouse_down_out(
                    &gpui::point(gpui::px(150.), gpui::px(50.)),
                    cx
                )
            );
            assert!(
                !root_read.should_dismiss_on_mouse_down_out(
                    &gpui::point(gpui::px(250.), gpui::px(50.)),
                    cx
                )
            );
            assert!(
                root_read.should_dismiss_on_mouse_down_out(
                    &gpui::point(gpui::px(400.), gpui::px(50.)),
                    cx
                )
            );
            let child_read = child.read(cx);
            assert!(
                !child_read.should_dismiss_on_mouse_down_out(
                    &gpui::point(gpui::px(50.), gpui::px(50.)),
                    cx
                )
            );
        });
    }
}
