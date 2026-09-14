use gpui::{AnyElement, Entity, div, prelude::*, px, rgb};
use gpui_component::{Icon, Sizable, button::Button, button::ButtonVariants, menu::DropdownMenu};

use crate::app_tooltip::AppTooltipExt;
use crate::theme::MUTED;

use super::{
    deezer_radio::FlowMode,
    model::{Category, Service, is_detail_route},
    view::LibraryView,
};

const FLOW_MODE_SELECTOR_ID: &str = "library-flow-mode-selector";
const FLOW_MODE_TOOLTIP_ID: &str = "library-flow-mode-tooltip";
const QUEUE_FLOW_MODE_SELECTOR_ID: &str = "queue-flow-mode-selector";
const QUEUE_FLOW_MODE_TOOLTIP_ID: &str = "queue-flow-mode-tooltip";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlowModeTarget {
    Library,
    Playback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FlowControlKind {
    Mode,
    None,
}

pub(super) fn control_kind(
    service: Service,
    category: Category,
    action: &str,
    route_depth: usize,
) -> FlowControlKind {
    if service != Service::Deezer || category != Category::Flow {
        return FlowControlKind::None;
    }
    if is_detail_route(route_depth) && action == "flowTracks" {
        FlowControlKind::Mode
    } else {
        FlowControlKind::None
    }
}

pub(super) fn flow_control_kind(view: &LibraryView) -> FlowControlKind {
    let route = view.state.route();
    hide_smart_mix_mode_control(
        control_kind(
            view.state.service,
            view.state.category,
            &route.action,
            view.state.routes.len(),
        ),
        &view.flow_detail_kind(),
    )
}

fn hide_smart_mix_mode_control(
    kind: FlowControlKind,
    flow_detail_kind: &crate::playback::DeezerFlowKind,
) -> FlowControlKind {
    if kind == FlowControlKind::Mode
        && matches!(flow_detail_kind, crate::playback::DeezerFlowKind::SmartMix)
    {
        FlowControlKind::None
    } else {
        kind
    }
}

pub(super) fn flow_mode_options() -> [(FlowMode, &'static str); 2] {
    [
        (FlowMode::Default, "Default"),
        (FlowMode::Discovery, "Discovery"),
    ]
}

#[cfg(test)]
pub(super) fn flow_mode_label(mode: FlowMode) -> &'static str {
    flow_mode_options()
        .into_iter()
        .find_map(|(candidate, label)| (candidate == mode).then_some(label))
        .unwrap_or("Default")
}

#[cfg(test)]
pub(super) fn flow_mode_for_label(label: &str) -> Option<FlowMode> {
    flow_mode_options()
        .into_iter()
        .find_map(|(mode, candidate)| (candidate == label).then_some(mode))
}

pub(super) fn render_page_controls(
    view: &LibraryView,
    host: &Entity<LibraryView>,
) -> Option<AnyElement> {
    (flow_control_kind(view) == FlowControlKind::Mode)
        .then(|| render_mode_selector(view.flow_mode, host, view.flow_mode_context_label()))
}

pub(crate) fn render_mode_selector(
    mode: FlowMode,
    host: &Entity<LibraryView>,
    mode_label: &'static str,
) -> AnyElement {
    render_mode_selector_with_ids(
        mode,
        host,
        FLOW_MODE_TOOLTIP_ID,
        FLOW_MODE_SELECTOR_ID,
        FlowModeTarget::Library,
        mode_label,
    )
}

pub(crate) fn render_queue_mode_selector(
    mode: FlowMode,
    host: &Entity<LibraryView>,
    mode_label: &'static str,
) -> AnyElement {
    div()
        .relative()
        .top(px(QUEUE_MODE_OPTICAL_OFFSET_PX))
        .child(render_mode_selector_with_ids(
            mode,
            host,
            QUEUE_FLOW_MODE_TOOLTIP_ID,
            QUEUE_FLOW_MODE_SELECTOR_ID,
            FlowModeTarget::Playback,
            mode_label,
        ))
        .into_any_element()
}

const QUEUE_MODE_OPTICAL_OFFSET_PX: f32 = 2.;

pub(crate) const fn flow_mode_context_label(smart_mix: bool) -> &'static str {
    if smart_mix { "Mix mode" } else { "Flow mode" }
}

fn render_mode_selector_with_ids(
    mode: FlowMode,
    host: &Entity<LibraryView>,
    tooltip_id: &'static str,
    selector_id: &'static str,
    target: FlowModeTarget,
    mode_label: &'static str,
) -> AnyElement {
    let menu_host = host.clone();
    div()
        .id(tooltip_id)
        .flex_none()
        .size(px(22.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        // Keep the tooltip anchored to this stable 22px wrapper. The selected
        // mode belongs in the popup's checked state, not in the tooltip text,
        // so opening the menu cannot move the tooltip as its width changes.
        .app_tooltip(mode_label)
        .aria_label(mode_label)
        .child(
            Button::new(selector_id)
                .xsmall()
                // Button scales custom icon sizes from its component size.
                .with_size(px(14.666666))
                .ghost()
                .size(px(22.))
                .rounded(px(6.))
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .icon(Icon::default().path(crate::assets::LocalIcon::Sliders.path()))
                .dropdown_menu(move |menu, _, _| {
                    flow_mode_options().into_iter().fold(
                        menu.min_w(px(crate::context_menu::COMPACT_MENU_WIDTH))
                            .max_w(px(crate::context_menu::COMPACT_MENU_WIDTH)),
                        |menu, (option, label)| {
                            let host = menu_host.clone();
                            menu.item(crate::context_menu::compact_checked_action_item(
                                label,
                                option == mode,
                                move |_, _, app| {
                                    host.update(app, |this, cx| match target {
                                        FlowModeTarget::Library => {
                                            this.select_flow_mode(option, cx)
                                        }
                                        FlowModeTarget::Playback => {
                                            this.select_playback_flow_mode(option, cx)
                                        }
                                    });
                                },
                            ))
                        },
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_mode_labels_map_to_the_exact_modes() {
        assert_eq!(flow_mode_label(FlowMode::Default), "Default");
        assert_eq!(flow_mode_label(FlowMode::Discovery), "Discovery");
        assert_eq!(flow_mode_for_label("Default"), Some(FlowMode::Default));
        assert_eq!(flow_mode_for_label("Discovery"), Some(FlowMode::Discovery));
    }

    #[test]
    fn opened_flow_mode_control_has_one_icon_trigger_and_two_menu_options() {
        assert_eq!(FLOW_MODE_SELECTOR_ID, "library-flow-mode-selector");
        assert_eq!(FLOW_MODE_TOOLTIP_ID, "library-flow-mode-tooltip");
        assert_eq!(QUEUE_FLOW_MODE_SELECTOR_ID, "queue-flow-mode-selector");
        assert_eq!(QUEUE_FLOW_MODE_TOOLTIP_ID, "queue-flow-mode-tooltip");
        assert_eq!(flow_mode_options().len(), 2);
        assert_eq!(
            flow_mode_options()
                .into_iter()
                .map(|(_, label)| label)
                .collect::<Vec<_>>(),
            ["Default", "Discovery"]
        );
    }

    #[test]
    fn mode_context_labels_distinguish_flow_and_smart_mix() {
        assert_eq!(flow_mode_context_label(false), "Flow mode");
        assert_eq!(flow_mode_context_label(true), "Mix mode");
    }

    #[test]
    fn mode_control_is_visible_only_on_flow_detail_routes() {
        assert_eq!(
            control_kind(Service::Deezer, Category::Flow, "flow", 1),
            FlowControlKind::None
        );
        assert_eq!(
            control_kind(Service::Deezer, Category::Flow, "flowTracks", 2),
            FlowControlKind::Mode
        );
        assert_eq!(
            control_kind(Service::Deezer, Category::Flow, "flowTracks", 1),
            FlowControlKind::None
        );
        assert_eq!(
            control_kind(Service::Deezer, Category::Tracks, "tracks", 1),
            FlowControlKind::None
        );
    }

    #[test]
    fn smart_mix_hides_only_the_detail_mode_control() {
        assert_eq!(
            hide_smart_mix_mode_control(
                FlowControlKind::Mode,
                &crate::playback::DeezerFlowKind::SmartMix,
            ),
            FlowControlKind::None
        );
        assert_eq!(
            hide_smart_mix_mode_control(
                FlowControlKind::Mode,
                &crate::playback::DeezerFlowKind::Flow,
            ),
            FlowControlKind::Mode
        );
    }
}
