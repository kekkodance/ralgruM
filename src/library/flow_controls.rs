use gpui::{AnyElement, Entity, FontWeight, KeyDownEvent, Window, div, prelude::*, px, rgb, rgba};
use gpui_component::{Icon, Sizable, button::Button, button::ButtonVariants, menu::DropdownMenu};

use crate::app_tooltip::AppTooltipExt;
use crate::theme::{BORDER, FOREGROUND, MUTED, SURFACE_RAISED};

use super::{
    deezer_radio::FlowMode,
    model::{Category, FlowCatalog, FlowCatalogOption, Page, Service, is_detail_route},
    view::LibraryView,
};

const SELECTED_BORDER: u32 = 0x818cf8;
const SELECTED_BACKGROUND: u32 = 0x6366f1;
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
    Catalog,
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
    if !is_detail_route(route_depth) && action == "flow" {
        FlowControlKind::Catalog
    } else if is_detail_route(route_depth) && action == "flowTracks" {
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

pub(super) fn catalog_options(catalog: &FlowCatalog) -> Vec<&FlowCatalogOption> {
    catalog
        .options
        .iter()
        .filter(|option| matches!(option.label.as_str(), "Moods" | "Genres"))
        .collect()
}

pub(super) fn selected_catalog_option_id<'a>(
    catalog: &'a FlowCatalog,
    selected: Option<&str>,
) -> Option<&'a str> {
    let options = catalog_options(catalog);
    selected
        .and_then(|selected| {
            options
                .iter()
                .find(|option| option.id == selected)
                .map(|option| option.id.as_str())
        })
        .or_else(|| {
            options
                .iter()
                .find(|option| option.id == catalog.default_option_id)
                .map(|option| option.id.as_str())
        })
        .or_else(|| options.first().map(|option| option.id.as_str()))
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
    page: &Page,
    host: &Entity<LibraryView>,
) -> Option<AnyElement> {
    render_page_controls_snapshot(
        flow_control_kind(view),
        view.flow_mode,
        view.flow_mode_context_label(),
        view.flow_catalog_option.as_deref(),
        page,
        host,
    )
}

pub(super) fn render_page_controls_snapshot(
    kind: FlowControlKind,
    flow_mode: FlowMode,
    mode_label: &'static str,
    flow_catalog_option: Option<&str>,
    page: &Page,
    host: &Entity<LibraryView>,
) -> Option<AnyElement> {
    match kind {
        FlowControlKind::Catalog => page
            .flow_catalog
            .as_ref()
            .and_then(|catalog| render_catalog_selector(flow_catalog_option, catalog, host)),
        FlowControlKind::Mode => Some(render_mode_selector(flow_mode, host, mode_label)),
        FlowControlKind::None => None,
    }
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

fn render_catalog_selector(
    flow_catalog_option: Option<&str>,
    catalog: &FlowCatalog,
    host: &Entity<LibraryView>,
) -> Option<AnyElement> {
    let selected = selected_catalog_option_id(catalog, flow_catalog_option)?;
    let options = catalog_options(catalog)
        .into_iter()
        .map(|option| (option.id.clone(), option.label.clone()))
        .collect::<Vec<_>>();
    Some(render_segmented(
        "library-flow-catalog-selector",
        options,
        Some(selected.to_owned()),
        host,
        |host, option_id, app| {
            host.update(app, |this, cx| {
                this.select_flow_catalog_option(option_id, cx)
            });
        },
    ))
}

fn render_segmented<T, F>(
    id: &'static str,
    options: Vec<(T, String)>,
    selected: Option<T>,
    host: &Entity<LibraryView>,
    on_select: F,
) -> AnyElement
where
    T: Clone + Eq + 'static,
    F: Fn(Entity<LibraryView>, T, &mut gpui::App) + Copy + 'static,
{
    div()
        .id(id)
        .w_full()
        .flex()
        .flex_wrap()
        .gap(px(3.))
        .p(px(3.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        .role(gpui::Role::TabList)
        .aria_label(if id == "library-flow-mode-selector" {
            "Flow mode"
        } else {
            "Flow catalog"
        })
        .children(options.into_iter().map(|(value, label)| {
            let active = selected.as_ref() == Some(&value);
            let click_value = value.clone();
            let key_value = value.clone();
            let click_host = host.clone();
            let key_host = host.clone();
            div()
                .id(format!("{id}-{label}"))
                .min_w_0()
                .flex_1()
                .h(px(30.))
                .px(px(10.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .when(active, |this| this.rounded(px(6.)))
                .border_1()
                .border_color(if active {
                    rgba((SELECTED_BORDER << 8) | 0xd9)
                } else {
                    rgba(0x00000000)
                })
                .bg(if active {
                    rgba((SELECTED_BACKGROUND << 8) | 0x3d)
                } else {
                    rgba(0x00000000)
                })
                .text_color(rgb(if active { FOREGROUND } else { MUTED }))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .role(gpui::Role::Tab)
                .aria_label(label.clone())
                .aria_selected(active)
                .focusable()
                .tab_stop(true)
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .hover(|style| style.text_color(rgb(FOREGROUND)))
                .on_click(move |_, _, app| on_select(click_host.clone(), click_value.clone(), app))
                .on_key_down(move |event: &KeyDownEvent, window: &mut Window, app| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        on_select(key_host.clone(), key_value.clone(), app);
                    }
                })
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::model::{FlowCatalogMembership, FlowCatalogOption};

    fn catalog() -> FlowCatalog {
        FlowCatalog {
            default_option_id: "moods".into(),
            options: vec![
                FlowCatalogOption {
                    id: "moods".into(),
                    label: "Moods".into(),
                },
                FlowCatalogOption {
                    id: "genres".into(),
                    label: "Genres".into(),
                },
            ],
            memberships: vec![FlowCatalogMembership {
                config_id: "mix".into(),
                option_ids: vec!["moods".into(), "genres".into()],
            }],
        }
    }

    #[test]
    fn default_selection_uses_metadata_default_option() {
        assert_eq!(selected_catalog_option_id(&catalog(), None), Some("moods"));
    }

    #[test]
    fn invalid_selection_falls_back_to_metadata_default() {
        assert_eq!(
            selected_catalog_option_id(&catalog(), Some("missing")),
            Some("moods")
        );
    }

    #[test]
    fn catalog_labels_are_exact_and_provider_order_is_preserved() {
        let catalog = catalog();
        let labels = catalog_options(&catalog)
            .into_iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["Moods", "Genres"]);
    }

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
    fn flow_mode_popup_uses_the_compact_checked_action_rows() {
        assert_eq!(crate::context_menu::COMPACT_MENU_WIDTH, 144.);
        let source = include_str!("flow_controls.rs");
        assert!(source.contains("compact_checked_action_item"));
        assert!(source.contains(".max_w(px(crate::context_menu::COMPACT_MENU_WIDTH))"));
    }

    #[test]
    fn flow_mode_trigger_delegates_hover_to_the_ghost_button_variant() {
        let source = include_str!("flow_controls.rs");
        let trigger = source
            .split("fn render_mode_selector_with_ids")
            .nth(1)
            .and_then(|source| source.split("fn render_catalog_selector").next())
            .expect("mode selector render function");
        assert!(trigger.contains(".ghost()"));
        assert!(!trigger.contains(".hover("));
    }

    #[test]
    fn flow_mode_tooltip_is_stable_and_does_not_include_the_selected_mode() {
        let source = include_str!("flow_controls.rs");
        let trigger = source
            .split("fn render_mode_selector_with_ids")
            .nth(1)
            .and_then(|source| source.split("fn render_catalog_selector").next())
            .expect("mode selector render function");
        assert!(trigger.contains(".app_tooltip(mode_label)"));
        assert!(trigger.contains(".aria_label(mode_label)"));
        assert!(!trigger.contains("format!(\"Flow mode:"));
    }

    #[test]
    fn mode_context_labels_distinguish_flow_and_smart_mix() {
        assert_eq!(flow_mode_context_label(false), "Flow mode");
        assert_eq!(flow_mode_context_label(true), "Mix mode");
    }

    #[test]
    fn queue_mode_trigger_has_the_named_optical_offset() {
        assert_eq!(QUEUE_MODE_OPTICAL_OFFSET_PX, 2.);
        let source = include_str!("flow_controls.rs");
        let queue = source
            .split("pub(crate) fn render_queue_mode_selector")
            .nth(1)
            .and_then(|source| {
                source
                    .split("pub(crate) const fn flow_mode_context_label")
                    .next()
            })
            .expect("queue mode selector source");
        assert!(queue.contains(".top(px(QUEUE_MODE_OPTICAL_OFFSET_PX))"));
    }

    #[test]
    fn catalog_and_mode_controls_are_visible_on_their_respective_routes() {
        assert_eq!(
            control_kind(Service::Deezer, Category::Flow, "flow", 1),
            FlowControlKind::Catalog
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
        assert_eq!(
            hide_smart_mix_mode_control(
                FlowControlKind::Catalog,
                &crate::playback::DeezerFlowKind::SmartMix,
            ),
            FlowControlKind::Catalog
        );
    }
}
