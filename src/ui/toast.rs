use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnimationExt, App, ClickEvent, Context, ElementId, Entity, FontWeight, Global, IntoElement,
    MouseButton, ParentElement, Render, Role, SharedString, Styled, WeakEntity, Window, div,
    prelude::*, px, rgb, rgba,
};

use crate::{
    app_button::{
        DANGER_SECONDARY_HOVER_BACKGROUND, DANGER_SECONDARY_HOVER_BORDER,
        DANGER_SECONDARY_HOVER_TEXT, DANGER_SECONDARY_NORMAL_BACKGROUND,
        DANGER_SECONDARY_NORMAL_BORDER, DANGER_SECONDARY_NORMAL_TEXT,
    },
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    theme::{BORDER, FOREGROUND, MUTED, PRIMARY},
};

const TOAST_LIFETIME: Duration = Duration::from_secs(6);
const TOAST_LIFETIME_TICK: Duration = Duration::from_millis(250);
const INFO_COLOR: u32 = 0x818cf8;
const SUCCESS_COLOR: u32 = 0x10b981;
const WARNING_COLOR: u32 = 0xf59e0b;
const ERROR_COLOR: u32 = 0xef4444;
const TOAST_MIN_WIDTH_PX: f32 = 280.;
const TOAST_MAX_WIDTH_PX: f32 = 380.;
const TOAST_PADDING_Y_PX: f32 = 12.;
const TOAST_PADDING_X_PX: f32 = 16.;
const TOAST_RADIUS_PX: f32 = 12.;
const TOAST_ACTION_HEIGHT_PX: f32 = 30.;
const TOAST_ACTION_HORIZONTAL_PADDING_PX: f32 = 12.;
const TOAST_ACTION_RADIUS_PX: f32 = 6.;
const TOAST_ACTION_TEXT_SIZE_PX: f32 = 13.;
const TOAST_CLOSE_SIZE_PX: f32 = 28.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    fn icon(self) -> LocalIcon {
        match self {
            Self::Info => LocalIcon::CircleInfo,
            Self::Success => LocalIcon::CircleCheck,
            Self::Warning => LocalIcon::TriangleExclamation,
            Self::Error => LocalIcon::CircleXmark,
        }
    }

    fn color(self) -> u32 {
        match self {
            Self::Info => INFO_COLOR,
            Self::Success => SUCCESS_COLOR,
            Self::Warning => WARNING_COLOR,
            Self::Error => ERROR_COLOR,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToastActionKind {
    Secondary,
    DangerSecondary,
}

pub(crate) type ToastActionCallback = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(Clone)]
pub(crate) struct ToastAction {
    pub(crate) label: SharedString,
    pub(crate) kind: ToastActionKind,
    pub(crate) callback: ToastActionCallback,
}

impl ToastAction {
    pub(crate) fn new(
        label: impl Into<SharedString>,
        kind: ToastActionKind,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            kind,
            callback: Rc::new(callback),
        }
    }

    pub(crate) fn secondary(
        label: impl Into<SharedString>,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self::new(label, ToastActionKind::Secondary, callback)
    }

    pub(crate) fn danger_secondary(
        label: impl Into<SharedString>,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self::new(label, ToastActionKind::DangerSecondary, callback)
    }
}

struct Toast {
    id: u64,
    kind: ToastKind,
    title: SharedString,
    description: Option<SharedString>,
    key: Option<SharedString>,
    actions: Vec<ToastAction>,
    persistent: bool,
    /// Hovering pauses the auto-dismiss countdown, like the original's
    /// mouseenter clearTimeout in public/js/ui/toasts.js.
    hovered: bool,
    remaining: Duration,
}

impl Toast {
    /// Advances the countdown by one tick unless hovered. Returns true when
    /// the lifetime ran out and the toast should be dismissed.
    fn advance_lifetime(&mut self, tick: Duration) -> bool {
        if self.persistent {
            return false;
        }
        if !self.hovered {
            self.remaining = self.remaining.saturating_sub(tick);
        }
        self.remaining.is_zero()
    }
}

pub(crate) struct ToastStack {
    toasts: Vec<Toast>,
    next_id: u64,
}

fn remove_toasts_with_key(toasts: &mut Vec<Toast>, key: &str) -> bool {
    let original_len = toasts.len();
    toasts.retain(|toast| {
        toast
            .key
            .as_ref()
            .is_none_or(|toast_key| toast_key.as_str() != key)
    });
    toasts.len() != original_len
}

fn insert_toast(toasts: &mut Vec<Toast>, toast: Toast) {
    if let Some(key) = toast.key.as_ref() {
        remove_toasts_with_key(toasts, key.as_str());
    }
    toasts.push(toast);
}

fn take_action_from(
    toasts: &mut Vec<Toast>,
    toast_id: u64,
    action_index: usize,
) -> Option<ToastActionCallback> {
    let toast_index = toasts.iter().position(|toast| toast.id == toast_id)?;
    let callback = toasts
        .get(toast_index)?
        .actions
        .get(action_index)?
        .callback
        .clone();
    toasts.remove(toast_index);
    Some(callback)
}

struct ToastGlobal(WeakEntity<ToastStack>);

impl Global for ToastGlobal {}

pub(crate) fn set_global(cx: &mut App, stack: &Entity<ToastStack>) {
    cx.set_global(ToastGlobal(stack.downgrade()));
}

pub(crate) fn push_global(
    cx: &mut App,
    kind: ToastKind,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
) {
    let Some(stack) = cx.try_global::<ToastGlobal>().and_then(|g| g.0.upgrade()) else {
        return;
    };
    stack.update(cx, |stack, cx| stack.push(kind, title, description, cx));
}

impl ToastStack {
    pub(crate) fn new() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 0,
        }
    }

    pub(crate) fn push(
        &mut self,
        kind: ToastKind,
        title: impl Into<SharedString>,
        description: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        insert_toast(
            &mut self.toasts,
            Toast {
                id,
                kind,
                title: title.into(),
                description,
                key: None,
                actions: Vec::new(),
                persistent: false,
                hovered: false,
                remaining: TOAST_LIFETIME,
            },
        );
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            // The countdown drains in small ticks so a hover can pause it and
            // the remaining time survives after the pointer leaves.
            loop {
                executor.timer(TOAST_LIFETIME_TICK).await;
                let expired = this
                    .update(cx, |stack, cx| stack.tick_lifetime(id, cx))
                    .unwrap_or(true);
                if expired {
                    return;
                }
            }
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn push_actionable_keyed(
        &mut self,
        key: impl Into<SharedString>,
        kind: ToastKind,
        title: impl Into<SharedString>,
        description: Option<SharedString>,
        actions: impl IntoIterator<Item = ToastAction>,
        cx: &mut Context<Self>,
    ) {
        self.push_actionable_with_key(Some(key.into()), kind, title, description, actions, cx);
    }

    fn push_actionable_with_key(
        &mut self,
        key: Option<SharedString>,
        kind: ToastKind,
        title: impl Into<SharedString>,
        description: Option<SharedString>,
        actions: impl IntoIterator<Item = ToastAction>,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        insert_toast(
            &mut self.toasts,
            Toast {
                id,
                kind,
                title: title.into(),
                description,
                key,
                actions: actions.into_iter().collect(),
                persistent: true,
                hovered: false,
                // Persistent toasts never consult this value and do not start a
                // countdown task. Keeping the field initialized preserves the
                // ordinary toast lifetime representation.
                remaining: TOAST_LIFETIME,
            },
        );
        cx.notify();
    }

    /// Advances one toast's countdown, removing it when it runs out. Returns
    /// true when the toast is gone and its timer loop should stop.
    fn tick_lifetime(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id) else {
            return true;
        };
        if toast.persistent {
            return false;
        }
        if toast.advance_lifetime(TOAST_LIFETIME_TICK) {
            self.remove(id, cx);
            return true;
        }
        false
    }

    fn set_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        if let Some(toast) = self.toasts.iter_mut().find(|toast| toast.id == id)
            && toast.hovered != hovered
        {
            toast.hovered = hovered;
            cx.notify();
        }
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.toasts.iter().any(|toast| toast.id == id) {
            self.toasts.retain(|toast| toast.id != id);
            cx.notify();
        }
    }

    /// Removes the toast before returning its callback so stale UI events
    /// cannot invoke an action more than once. The callback is kept outside
    /// the stack while it runs, so it cannot form a lasting stack cycle.
    fn take_action(
        &mut self,
        toast_id: u64,
        action_index: usize,
        cx: &mut Context<Self>,
    ) -> Option<ToastActionCallback> {
        let callback = take_action_from(&mut self.toasts, toast_id, action_index)?;
        cx.notify();
        Some(callback)
    }

    pub(crate) fn remove_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if remove_toasts_with_key(&mut self.toasts, key) {
            cx.notify();
        }
    }
}

fn consume_pointer_event(window: &mut Window, cx: &mut App) {
    window.prevent_default();
    cx.stop_propagation();
}

fn toast_action_button(
    id: impl Into<ElementId>,
    label: SharedString,
    kind: ToastActionKind,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let mut button = div()
        .id(id)
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label(label.clone())
        .h(px(TOAST_ACTION_HEIGHT_PX))
        .px(px(TOAST_ACTION_HORIZONTAL_PADDING_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(TOAST_ACTION_RADIUS_PX))
        .text_size(px(TOAST_ACTION_TEXT_SIZE_PX))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer();
    button = match kind {
        ToastActionKind::Secondary => button
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgba(0x00000000))
            .text_color(rgb(FOREGROUND))
            .hover(|style| style.bg(rgb(BORDER)))
            .focus_visible(|style| style.border_color(rgb(PRIMARY))),
        ToastActionKind::DangerSecondary => button
            .border_1()
            .border_color(rgba(DANGER_SECONDARY_NORMAL_BORDER))
            .bg(rgba(DANGER_SECONDARY_NORMAL_BACKGROUND))
            .text_color(rgb(DANGER_SECONDARY_NORMAL_TEXT))
            .hover(|style| {
                style
                    .text_color(rgb(DANGER_SECONDARY_HOVER_TEXT))
                    .border_color(rgba(DANGER_SECONDARY_HOVER_BORDER))
                    .bg(rgba(DANGER_SECONDARY_HOVER_BACKGROUND))
            })
            .focus_visible(|style| style.border_color(rgb(PRIMARY))),
    };

    button
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(label)
}

fn toast_close_button(
    id: impl Into<ElementId>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(TOAST_CLOSE_SIZE_PX))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(TOAST_ACTION_RADIUS_PX))
        .cursor_pointer()
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("Dismiss notification")
        .hover(|style| style.bg(rgb(BORDER)))
        .focus_visible(|style| style.border_1().border_color(rgb(PRIMARY)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(local_icon(LocalIcon::X, MUTED).size(px(12.)))
}

impl Render for ToastStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col_reverse()
            .w(px(360.))
            .max_w_full()
            .children(self.toasts.iter().map(|toast| {
                let id = toast.id;
                let animation_id = toast.key.as_ref().map_or_else(
                    || ElementId::from(("toast-entry", id)),
                    |key| ElementId::from((ElementId::from("toast-entry"), key.clone())),
                );
                let close = cx.entity().downgrade();
                let hover = cx.entity().downgrade();
                let action_buttons =
                    toast
                        .actions
                        .iter()
                        .enumerate()
                        .map(|(action_index, action)| {
                            let action_host = cx.entity().downgrade();
                            let label = action.label.clone();
                            let kind = action.kind;
                            let action_selector = format!("toast-action-{id}-{action_index}");
                            toast_action_button(
                                action_selector.clone(),
                                label,
                                kind,
                                move |event, window, cx| {
                                    consume_pointer_event(window, cx);
                                    let callback = action_host
                                        .update(cx, |stack, cx| {
                                            stack.take_action(id, action_index, cx)
                                        })
                                        .ok()
                                        .flatten();
                                    if let Some(callback) = callback {
                                        callback(event, window, cx);
                                    }
                                },
                            )
                            .debug_selector(move || action_selector)
                        });
                let mut toast_element = div()
                    .id(("toast", id))
                    .mt(px(10.))
                    .min_w(px(TOAST_MIN_WIDTH_PX))
                    .max_w(px(TOAST_MAX_WIDTH_PX))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .relative()
                    .py(px(TOAST_PADDING_Y_PX))
                    .px(px(TOAST_PADDING_X_PX))
                    .rounded(px(TOAST_RADIUS_PX))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgba(0x121214f5))
                    .occlude()
                    .on_click(|_, window, cx| {
                        consume_pointer_event(window, cx);
                    })
                    .on_hover(move |hovered: &bool, _, cx| {
                        let _ = hover.update(cx, |stack, cx| stack.set_hovered(id, *hovered, cx));
                    })
                    .child(
                        div()
                            .id(("toast-surface", id))
                            .absolute()
                            .inset_0()
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .on_mouse_up(MouseButton::Left, |_, window, cx| {
                                window.prevent_default();
                                cx.stop_propagation();
                            })
                            .on_click(|_, window, cx| {
                                consume_pointer_event(window, cx);
                            }),
                    )
                    .child(local_icon(toast.kind.icon(), toast.kind.color()).size(px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(0xffffff))
                                    .child(toast.title.clone()),
                            )
                            .when_some(toast.description.clone(), |this, description| {
                                this.child(
                                    div()
                                        .mt(px(2.))
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(description),
                                )
                            })
                            .when(!toast.actions.is_empty(), |this| {
                                this.child(
                                    div()
                                        .mt(px(8.))
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(6.))
                                        .children(action_buttons),
                                )
                            }),
                    );
                if !toast.persistent {
                    toast_element = toast_element.child(
                        div()
                            .id(("toast-close-tooltip", id))
                            .flex_shrink_0()
                            .app_tooltip("Dismiss notification")
                            .child(toast_close_button(
                                ("toast-close", id),
                                move |_, window, cx| {
                                    consume_pointer_event(window, cx);
                                    let _ = close.update(cx, |stack, cx| stack.remove(id, cx));
                                },
                            )),
                    );
                }
                toast_element.with_animation(
                    animation_id,
                    crate::motion::content(),
                    |this, delta| {
                        this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                            .top(px(crate::motion::lerp(8.0, 0.0, delta)))
                    },
                )
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Duration};

    use gpui::{
        AnyWindowHandle, AppContext, Context, Entity, InteractiveElement, IntoElement, Modifiers,
        ParentElement, Render, StatefulInteractiveElement, Styled, TestAppContext,
        VisualTestContext, Window, div,
    };

    use super::{
        SharedString, TOAST_ACTION_HEIGHT_PX, TOAST_ACTION_HORIZONTAL_PADDING_PX,
        TOAST_ACTION_RADIUS_PX, TOAST_ACTION_TEXT_SIZE_PX, TOAST_LIFETIME, TOAST_MAX_WIDTH_PX,
        TOAST_MIN_WIDTH_PX, TOAST_PADDING_X_PX, TOAST_PADDING_Y_PX, TOAST_RADIUS_PX, Toast,
        ToastAction, ToastKind, insert_toast, remove_toasts_with_key, take_action_from,
    };

    struct ToastClickThroughHost {
        toasts: Entity<super::ToastStack>,
        track_clicks: Rc<Cell<usize>>,
    }

    impl Render for ToastClickThroughHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let track_clicks = self.track_clicks.clone();
            div()
                .size_full()
                .child(
                    div()
                        .id("track-like-element")
                        .debug_selector(|| "track-like-element".into())
                        .size_full()
                        .on_click(move |_, _, _| {
                            track_clicks.set(track_clicks.get() + 1);
                        }),
                )
                .child(div().absolute().inset_0().child(self.toasts.clone()))
        }
    }

    fn toast() -> Toast {
        Toast {
            id: 0,
            kind: ToastKind::Info,
            title: "Title".into(),
            description: None,
            key: None,
            actions: Vec::new(),
            persistent: false,
            hovered: false,
            remaining: TOAST_LIFETIME,
        }
    }

    #[test]
    fn lifetime_drains_in_ticks_and_expires_exactly() {
        let mut toast = toast();
        let tick = Duration::from_millis(250);
        let ticks = (TOAST_LIFETIME.as_millis() / tick.as_millis()) as u32;
        for _ in 0..ticks - 1 {
            assert!(!toast.advance_lifetime(tick));
        }
        assert!(toast.advance_lifetime(tick));
    }

    #[test]
    fn hovering_pauses_the_countdown_and_resumes_after() {
        let mut toast = toast();
        let tick = Duration::from_millis(500);
        assert!(!toast.advance_lifetime(tick));
        toast.hovered = true;
        for _ in 0..20 {
            // A hovered toast never expires while the pointer stays on it.
            assert!(!toast.advance_lifetime(tick));
        }
        assert_eq!(toast.remaining, TOAST_LIFETIME - tick);
        toast.hovered = false;
        let mut drained = 0;
        while !toast.advance_lifetime(tick) {
            drained += 1;
        }
        // Only the un-hovered time remained, so the resume kept the countdown
        // where the hover started it.
        assert_eq!(
            toast.remaining + tick * (drained + 1),
            TOAST_LIFETIME - tick
        );
    }

    #[test]
    fn geometry_matches_the_original_toast_box() {
        assert_eq!(TOAST_MIN_WIDTH_PX, 280.);
        assert_eq!(TOAST_MAX_WIDTH_PX, 380.);
        assert_eq!(TOAST_PADDING_Y_PX, 12.);
        assert_eq!(TOAST_PADDING_X_PX, 16.);
        assert_eq!(TOAST_RADIUS_PX, 12.);
    }

    #[test]
    fn action_controls_match_the_app_button_contract() {
        assert_eq!(TOAST_ACTION_HEIGHT_PX, 30.);
        assert_eq!(TOAST_ACTION_HORIZONTAL_PADDING_PX, 12.);
        assert_eq!(TOAST_ACTION_RADIUS_PX, 6.);
        assert_eq!(TOAST_ACTION_TEXT_SIZE_PX, 13.);
    }

    #[gpui::test]
    fn overwrite_click_does_not_click_through_to_the_track(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });

        let track_clicks = Rc::new(Cell::new(0));
        let overwrite_clicks = Rc::new(Cell::new(0));
        let toasts = cx.update(|cx| cx.new(|_| super::ToastStack::new()));
        cx.update(|cx| {
            let overwrite_clicks = overwrite_clicks.clone();
            toasts.update(cx, |stack, cx| {
                stack.push_actionable_keyed(
                    "download-conflict-7",
                    super::ToastKind::Warning,
                    "File already exists",
                    Some("Song Title · MP3 320kbps".into()),
                    [
                        super::ToastAction::secondary("Ignore", |_, _, _| {}),
                        super::ToastAction::danger_secondary("Overwrite", move |_, _, _| {
                            overwrite_clicks.set(overwrite_clicks.get() + 1);
                        }),
                    ],
                    cx,
                );
            });
        });

        let window = cx.add_window({
            let toasts = toasts.clone();
            let track_clicks = track_clicks.clone();
            move |_, _| ToastClickThroughHost {
                toasts,
                track_clicks,
            }
        });
        let mut visual = VisualTestContext::from_window(AnyWindowHandle::from(window), cx);
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        let overwrite_bounds = visual
            .debug_bounds("toast-action-0-1")
            .expect("Overwrite toast action should be rendered");
        visual.simulate_click(overwrite_bounds.center(), Modifiers::default());
        visual.run_until_parked();
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert_eq!(
            overwrite_clicks.get(),
            1,
            "overwrite={}, track={}",
            overwrite_clicks.get(),
            track_clicks.get()
        );
        assert_eq!(track_clicks.get(), 0);
        assert!(visual.debug_bounds("toast-action-0-1").is_none());
    }

    #[test]
    fn persistent_toast_does_not_drain_lifetime() {
        let mut toast = toast();
        toast.persistent = true;
        for _ in 0..100 {
            assert!(!toast.advance_lifetime(Duration::from_secs(1)));
        }
        assert_eq!(toast.remaining, TOAST_LIFETIME);
    }

    #[test]
    fn keyed_insert_replaces_the_previous_prompt() {
        let mut toasts = vec![Toast {
            key: Some("download-1".into()),
            ..toast()
        }];
        let replacement = Toast {
            id: 1,
            title: "Replacement".into(),
            key: Some("download-1".into()),
            ..toast()
        };
        insert_toast(&mut toasts, replacement);

        assert_eq!(toasts.len(), 1);
        assert_eq!(toasts[0].id, 1);
        assert_eq!(toasts[0].title, "Replacement");
    }

    #[test]
    fn keyed_removal_only_removes_matching_prompts() {
        let mut toasts = vec![
            Toast {
                key: Some("download-1".into()),
                ..toast()
            },
            Toast {
                id: 1,
                key: Some("download-2".into()),
                ..toast()
            },
        ];

        assert!(remove_toasts_with_key(&mut toasts, "download-1"));
        assert_eq!(toasts.len(), 1);
        assert_eq!(
            toasts[0].key.as_ref().map(SharedString::as_str),
            Some("download-2")
        );
        assert!(!remove_toasts_with_key(&mut toasts, "download-1"));
    }

    #[test]
    fn action_extraction_is_one_shot_and_removes_the_toast() {
        let mut toast = toast();
        toast
            .actions
            .push(ToastAction::secondary("Ignore", |_, _, _| {}));
        let mut toasts = vec![toast];

        assert!(take_action_from(&mut toasts, 0, 0).is_some());
        assert!(take_action_from(&mut toasts, 0, 0).is_none());
        assert!(toasts.is_empty());
    }
}
