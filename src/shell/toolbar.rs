use gpui::{
    AnimationExt as _, AnyElement, App, ClickEvent, Context, CursorStyle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, Role, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::input::Input;
use std::time::Instant;

use crate::{
    app_button::plain_x_button,
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    context_menu::text_field_context_menu,
    library::Service as LibraryService,
    search::{SEARCH_PLACEHOLDER, Source},
    theme::{BORDER, FOREGROUND, MUTED, SCROLLBAR_THUMB, SURFACE, SURFACE_RAISED},
};

use super::{
    Nav, RalgrumApp, RectSelectorMotion,
    search_suggestions::render_search_suggestions,
    source_tabs::{LibraryServiceItem, PlatformItem, SourceItem},
};

pub(crate) const SEARCH_MAX_WIDTH: f32 = 520.;
pub(crate) const TOOLBAR_HORIZONTAL_INSET: f32 = 28.;
pub(crate) const TOOLBAR_GAP: f32 = 20.;
pub(crate) const TOOLBAR_MIN_SEARCH_WIDTH: f32 = 300.;
const TOOLBAR_BOTTOM_MARGIN_PX: f32 = 20.;
const DISCOVER_CHANNEL_TOOLBAR_BOTTOM_MARGIN_PX: f32 = 4.;
pub(crate) const SEARCH_SOURCE_SELECTOR_WIDTH: f32 = 286.;
pub(crate) const LIBRARY_SERVICE_SELECTOR_WIDTH: f32 = 300.;
const SELECTOR_ITEM_GAP: f32 = 2.;
const SELECTOR_PADDING: f32 = 3.;
const SOURCE_ALL_WIDTH: f32 = 70.;
const PLATFORM_SERVICE_FULL_WIDTH: f32 = 214.;
const PLATFORM_SERVICE_COMPACT_WIDTH: f32 = 70.;
const LIBRARY_PLATFORM_SERVICE_FULL_WIDTH: f32 = 286.;
const LIBRARY_PLATFORM_SERVICE_COMPACT_WIDTH: f32 = 106.;
const PLATFORM_SOURCE_RESERVE_FULL_WIDTH: f32 = 78.;
const PLATFORM_SOURCE_RESERVE_COMPACT_WIDTH: f32 = 6.;
const SEARCH_INPUT_ACTION_PADDING_PX: f32 = 44.;
const SEARCH_INPUT_CLEAR_PADDING_PX: f32 = 72.;
/// The text inset of the search field: its left padding, plus the 1px
/// frame border the padded content box starts behind. The blurred hint
/// overlay must sit exactly where the editor paints the focused native
/// placeholder, or the hint visibly jumps when focus swaps the two.
const SEARCH_INPUT_TEXT_INSET_PX: f32 = 36. + 1.;

fn search_input_trailing_padding(clear_visible: bool) -> f32 {
    if clear_visible {
        SEARCH_INPUT_CLEAR_PADDING_PX
    } else {
        SEARCH_INPUT_ACTION_PADDING_PX
    }
}

fn toolbar_bottom_margin(discover_channel_open: bool) -> f32 {
    if discover_channel_open {
        DISCOVER_CHANNEL_TOOLBAR_BOTTOM_MARGIN_PX
    } else {
        TOOLBAR_BOTTOM_MARGIN_PX
    }
}

fn should_show_search_suggestions(show_search_source: bool, suggestions_visible: bool) -> bool {
    show_search_source && suggestions_visible
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchPlaceholderVisibility {
    custom_overlay: bool,
    native_input: bool,
}

fn search_placeholder_visibility(
    show_search_source: bool,
    has_query: bool,
    focused: bool,
) -> SearchPlaceholderVisibility {
    if !show_search_source || has_query {
        return SearchPlaceholderVisibility {
            custom_overlay: false,
            native_input: false,
        };
    }
    SearchPlaceholderVisibility {
        custom_overlay: !focused,
        native_input: focused,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ToolbarGeometryVisual {
    from_source_width: f32,
    target_source_width: f32,
    from_library_width: f32,
    target_library_width: f32,
    from_route_fraction: f32,
    target_route_fraction: f32,
    epoch: u64,
}

impl ToolbarGeometryVisual {
    pub(super) fn source_width_at(self, delta: f32) -> f32 {
        crate::motion::lerp(self.from_source_width, self.target_source_width, delta)
    }

    pub(super) fn library_width_at(self, delta: f32) -> f32 {
        crate::motion::lerp(self.from_library_width, self.target_library_width, delta)
    }

    pub(super) fn route_fraction_at(self, delta: f32) -> f32 {
        crate::motion::lerp(self.from_route_fraction, self.target_route_fraction, delta)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ToolbarGeometryMotion {
    initialized: bool,
    from_source_width: f32,
    target_source_width: f32,
    from_library_width: f32,
    target_library_width: f32,
    from_route_fraction: f32,
    target_route_fraction: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl ToolbarGeometryMotion {
    pub(super) fn will_change(
        &self,
        target_source_width: f32,
        target_library_width: f32,
        target_route_fraction: f32,
    ) -> bool {
        self.initialized
            && (self.target_source_width != target_source_width
                || self.target_library_width != target_library_width
                || self.target_route_fraction != target_route_fraction)
    }

    fn displayed_at(&self, now: Instant) -> (f32, f32, f32) {
        if !self.initialized {
            return (
                self.target_source_width,
                self.target_library_width,
                self.target_route_fraction,
            );
        }

        let Some(started_at) = self.started_at else {
            return (
                self.target_source_width,
                self.target_library_width,
                self.target_route_fraction,
            );
        };
        let progress = crate::motion::clamp_unit(
            now.saturating_duration_since(started_at).as_secs_f32()
                / crate::motion::CONTENT_DURATION.as_secs_f32(),
        );
        let delta = gpui::ease_in_out(progress);
        (
            crate::motion::lerp(self.from_source_width, self.target_source_width, delta),
            crate::motion::lerp(self.from_library_width, self.target_library_width, delta),
            crate::motion::lerp(self.from_route_fraction, self.target_route_fraction, delta),
        )
    }

    pub(super) fn prepare(
        &mut self,
        target_source_width: f32,
        target_library_width: f32,
        target_route_fraction: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> ToolbarGeometryVisual {
        if !self.initialized {
            self.initialized = true;
            self.from_source_width = target_source_width;
            self.target_source_width = target_source_width;
            self.from_library_width = target_library_width;
            self.target_library_width = target_library_width;
            self.from_route_fraction = target_route_fraction;
            self.target_route_fraction = target_route_fraction;
            self.started_at = None;
        } else if self.will_change(
            target_source_width,
            target_library_width,
            target_route_fraction,
        ) {
            let (source_width, library_width, route_fraction) = self.displayed_at(now);
            self.from_source_width = source_width;
            self.from_library_width = library_width;
            self.from_route_fraction = route_fraction;
            self.target_source_width = target_source_width;
            self.target_library_width = target_library_width;
            self.target_route_fraction = target_route_fraction;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion).then_some(now);
        }

        if reduced_motion {
            self.from_source_width = self.target_source_width;
            self.from_library_width = self.target_library_width;
            self.from_route_fraction = self.target_route_fraction;
            self.started_at = None;
        } else if let Some(started_at) = self.started_at
            && now.saturating_duration_since(started_at) >= crate::motion::CONTENT_DURATION
        {
            self.from_source_width = self.target_source_width;
            self.from_library_width = self.target_library_width;
            self.from_route_fraction = self.target_route_fraction;
            self.started_at = None;
        }

        ToolbarGeometryVisual {
            from_source_width: self.from_source_width,
            target_source_width: self.target_source_width,
            from_library_width: self.from_library_width,
            target_library_width: self.target_library_width,
            from_route_fraction: self.from_route_fraction,
            target_route_fraction: self.target_route_fraction,
            epoch: self.epoch,
        }
    }
}

const DETAIL_TOOLBAR_BACK_SLOT_GAP: f32 = TOOLBAR_GAP;
const DETAIL_TOOLBAR_STACKED_BACK_SLOT_GAP: f32 = 12.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DetailToolbarVisual {
    from_progress: f32,
    target_progress: f32,
    epoch: u64,
}

impl DetailToolbarVisual {
    fn progress_at(self, delta: f32) -> f32 {
        crate::motion::lerp(self.from_progress, self.target_progress, delta)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct DetailToolbarMotion {
    initialized: bool,
    from_progress: f32,
    target_progress: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl DetailToolbarMotion {
    fn displayed_at(&self, now: Instant) -> f32 {
        if !self.initialized {
            return self.target_progress;
        }
        let Some(started_at) = self.started_at else {
            return self.target_progress;
        };
        let progress = crate::motion::clamp_unit(
            now.saturating_duration_since(started_at).as_secs_f32()
                / crate::motion::CONTENT_DURATION.as_secs_f32(),
        );
        crate::motion::lerp(
            self.from_progress,
            self.target_progress,
            gpui::ease_in_out(progress),
        )
    }

    pub(super) fn prepare(
        &mut self,
        detail_open: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> DetailToolbarVisual {
        let target_progress = if detail_open { 1. } else { 0. };
        if !self.initialized {
            self.initialized = true;
            self.from_progress = target_progress;
            self.target_progress = target_progress;
            self.started_at = None;
        } else if self.target_progress != target_progress {
            self.from_progress = self.displayed_at(now);
            self.target_progress = target_progress;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion).then_some(now);
        }

        if reduced_motion {
            self.from_progress = target_progress;
            self.target_progress = target_progress;
            self.started_at = None;
        } else if let Some(started_at) = self.started_at
            && now.saturating_duration_since(started_at) >= crate::motion::CONTENT_DURATION
        {
            self.from_progress = self.target_progress;
            self.started_at = None;
        }

        DetailToolbarVisual {
            from_progress: self.from_progress,
            target_progress: self.target_progress,
            epoch: self.epoch,
        }
    }
}

fn detail_toolbar_phases(progress: f32) -> (f32, f32) {
    let progress = crate::motion::clamp_unit(progress);
    (
        crate::motion::clamp_unit(progress * 2.),
        crate::motion::clamp_unit(progress * 2. - 1.),
    )
}

fn detail_back_is_interactive(detail_open: bool, focus_ready: bool) -> bool {
    detail_open && focus_ready
}

fn sync_detail_toolbar_interaction(
    app: &mut RalgrumApp,
    detail_open: bool,
    back_opacity: f32,
    reduced_motion: bool,
    cx: &mut Context<RalgrumApp>,
) {
    let target_changed = app.detail_toolbar_focus_target != detail_open;
    let reduction_reveals_open_control =
        reduced_motion && detail_open && !app.detail_toolbar_focus_ready;
    if !target_changed && !reduction_reveals_open_control {
        return;
    }

    app.detail_toolbar_focus_target = detail_open;
    app.detail_toolbar_focus_generation = app.detail_toolbar_focus_generation.wrapping_add(1);
    app.detail_toolbar_focus_task = None;
    if !detail_open {
        app.detail_toolbar_focus_ready = false;
        return;
    }
    if reduced_motion || back_opacity > 0. {
        app.detail_toolbar_focus_ready = true;
        return;
    }

    app.detail_toolbar_focus_ready = false;
    let generation = app.detail_toolbar_focus_generation;
    let executor = cx.background_executor().clone();
    app.detail_toolbar_focus_task = Some(cx.spawn(async move |this, cx| {
        executor.timer(crate::motion::CONTENT_DURATION / 2).await;
        this.update(cx, |this, cx| {
            if this.detail_toolbar_focus_generation == generation
                && this.detail_toolbar_focus_target
            {
                this.detail_toolbar_focus_ready = true;
                this.detail_toolbar_focus_task = None;
                cx.notify();
            }
        })
        .ok();
    }));
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SelectorItemGeometry {
    x: f32,
    width: f32,
}

fn selector_item_geometry(widths: &[f32], selected_index: usize) -> SelectorItemGeometry {
    let selected_index = selected_index.min(widths.len().saturating_sub(1));
    SelectorItemGeometry {
        x: SELECTOR_PADDING
            + widths.iter().take(selected_index).copied().sum::<f32>()
            + selected_index as f32 * SELECTOR_ITEM_GAP,
        width: widths.get(selected_index).copied().unwrap_or(0.0),
    }
}

fn source_item_width(item: SourceItem, compact: bool) -> f32 {
    if compact && !matches!(item.source, Source::All) {
        item.compact_width
    } else {
        item.full_width
    }
}

fn selector_shell_width(widths: &[f32]) -> f32 {
    2. * SELECTOR_PADDING
        + widths.iter().copied().sum::<f32>()
        + widths.len().saturating_sub(1) as f32 * SELECTOR_ITEM_GAP
}

fn source_selector_width(items: &[SourceItem; 3], compact: bool) -> f32 {
    let widths = items.map(|item| source_item_width(item, compact));
    selector_shell_width(&widths)
}

fn library_selector_width(items: &[LibraryServiceItem; 3], compact: bool) -> f32 {
    let widths = items.map(|item| {
        if compact {
            item.compact_width
        } else {
            item.full_width
        }
    });
    selector_shell_width(&widths)
}

fn toolbar_route_opacities(fraction: f32) -> (f32, f32) {
    let fraction = crate::motion::clamp_unit(fraction);
    (1. - fraction, fraction)
}

fn toolbar_search_layer_opacities(visual: ToolbarGeometryVisual) -> (f32, f32) {
    toolbar_route_opacities(visual.target_route_fraction)
}

fn toolbar_selector_width(source_width: f32, library_width: f32, fraction: f32) -> f32 {
    crate::motion::lerp(source_width, library_width, fraction)
}

fn platform_service_width(visual: ToolbarGeometryVisual, delta: f32) -> f32 {
    let route_fraction = visual.route_fraction_at(delta);
    let selector_width = toolbar_selector_width(
        visual.source_width_at(delta),
        visual.library_width_at(delta),
        route_fraction,
    );
    let source_reserve = crate::motion::lerp(
        PLATFORM_SOURCE_RESERVE_FULL_WIDTH,
        PLATFORM_SOURCE_RESERVE_COMPACT_WIDTH,
        route_fraction,
    );
    selector_width - source_reserve
}

fn platform_service_responsive_visual(
    visual: ToolbarGeometryVisual,
) -> crate::motion::ResponsiveModeVisual {
    let from = platform_service_compactness(
        platform_service_width(visual, 0.),
        visual.route_fraction_at(0.),
    );
    let target = platform_service_compactness(
        platform_service_width(visual, 1.),
        visual.route_fraction_at(1.),
    );
    crate::motion::ResponsiveModeVisual {
        from,
        target,
        target_compact: target >= 1.0,
        epoch: visual.epoch,
    }
}

fn platform_service_compactness(width: f32, route_fraction: f32) -> f32 {
    let full_width = crate::motion::lerp(
        PLATFORM_SERVICE_FULL_WIDTH,
        LIBRARY_PLATFORM_SERVICE_FULL_WIDTH,
        route_fraction,
    );
    let compact_width = crate::motion::lerp(
        PLATFORM_SERVICE_COMPACT_WIDTH,
        LIBRARY_PLATFORM_SERVICE_COMPACT_WIDTH,
        route_fraction,
    );
    crate::motion::clamp_unit((full_width - width) / (full_width - compact_width))
}

fn platform_all_width(visual: ToolbarGeometryVisual, delta: f32) -> f32 {
    crate::motion::lerp(
        PLATFORM_SOURCE_RESERVE_FULL_WIDTH - PLATFORM_SOURCE_RESERVE_COMPACT_WIDTH,
        0.,
        visual.route_fraction_at(delta),
    )
}

fn source_selector_geometry(
    items: &[SourceItem; 3],
    selected_index: usize,
    compact: bool,
) -> SelectorItemGeometry {
    let widths = items.map(|item| source_item_width(item, compact));
    selector_item_geometry(&widths, selected_index)
}

fn library_selector_geometry(
    items: &[LibraryServiceItem; 3],
    selected_index: usize,
    compact: bool,
) -> SelectorItemGeometry {
    let widths = items.map(|item| {
        if compact {
            item.compact_width
        } else {
            item.full_width
        }
    });
    selector_item_geometry(&widths, selected_index)
}

fn selector_indicator(id: &'static str, motion: RectSelectorMotion) -> impl IntoElement {
    let from_x = motion.from_x;
    let from_width = motion.from_width;
    let target_x = motion.target_x;
    let target_width = motion.target_width;
    div()
        .absolute()
        .left(px(from_x))
        .top(px(SELECTOR_PADDING))
        .w(px(from_width))
        .h(px(30.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x818cf8d9))
        .bg(rgba(0x6366f13d))
        .with_animation(
            (id, motion.epoch),
            motion.animation(),
            move |this, delta| {
                this.left(px(crate::motion::lerp(from_x, target_x, delta)))
                    .w(px(crate::motion::lerp(from_width, target_width, delta)))
            },
        )
}

pub(crate) fn toolbar_available_width(
    viewport_width: f32,
    metrics: &crate::music_ui::ShellMetrics,
    right_sidebar_open: bool,
    toolbar_inset: f32,
) -> f32 {
    let right_sidebar = if right_sidebar_open && !metrics.narrow_content {
        metrics.right_sidebar_width
    } else {
        0.
    };
    (viewport_width - metrics.sidebar_width - right_sidebar - 2. * toolbar_inset).max(0.)
}

pub(crate) fn toolbar_shows_full_labels(available_width: f32, selector_width: f32) -> bool {
    available_width >= selector_width + TOOLBAR_GAP + TOOLBAR_MIN_SEARCH_WIDTH
}

fn search_detail_back_label(external_detail_return: Option<super::Nav>) -> &'static str {
    if external_detail_return == Some(super::Nav::Library) {
        "Back to library"
    } else if external_detail_return == Some(super::Nav::Cache) {
        "Back to cache"
    } else {
        "Back to search results"
    }
}

fn search_navigation_back_label(
    discover_channel_open: bool,
    search_results_root_open: bool,
    external_detail_return: Option<super::Nav>,
) -> &'static str {
    if discover_channel_open || search_results_root_open {
        "Back to Discover"
    } else {
        search_detail_back_label(external_detail_return)
    }
}

#[cfg(test)]
fn toolbar_route_identity(nav: Nav) -> &'static str {
    match nav {
        Nav::Discover => "discover",
        Nav::Library => "library",
        Nav::Downloads => "downloads",
        Nav::Cache => "cache",
    }
}

fn render_detail_back_button<Click, Key>(
    id: &'static str,
    group: &'static str,
    aria_label: &'static str,
    tooltip: &'static str,
    interactive: bool,
    on_click: Click,
    on_key_down: Key,
) -> AnyElement
where
    Click: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    Key: Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
{
    div()
        .id(id)
        .group(group)
        .flex_shrink_0()
        .w(px(crate::music_ui::DETAIL_BACK_BUTTON_SIZE))
        .h(px(crate::music_ui::DETAIL_BACK_BUTTON_SIZE))
        .mr(px(crate::music_ui::DETAIL_BACK_BUTTON_MARGIN_RIGHT))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::music_ui::DETAIL_BACK_BUTTON_RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE))
        .text_color(rgb(MUTED))
        .when(interactive, |this| this.cursor_pointer())
        .when(!interactive, |this| this.cursor(CursorStyle::Arrow))
        .when(interactive, |this| {
            this.focusable()
                .tab_stop(true)
                .role(Role::Button)
                .aria_label(aria_label)
                .hover(|style| {
                    style
                        .border_color(rgb(SCROLLBAR_THUMB))
                        .bg(rgb(BORDER))
                        .text_color(rgb(FOREGROUND))
                })
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .app_tooltip(tooltip)
        })
        .on_click(move |event, window, cx| {
            crate::ui::app_tooltip::dismiss_global(cx);
            if interactive {
                on_click(event, window, cx);
            }
        })
        .on_key_down(move |event, window, cx| {
            crate::ui::app_tooltip::dismiss_global(cx);
            if interactive {
                on_key_down(event, window, cx);
            }
        })
        .child(
            div()
                .relative()
                .size(px(crate::music_ui::DETAIL_BACK_ICON_SIZE))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(group, |style| style.invisible())
                        .child(local_icon(LocalIcon::ArrowLeft, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(local_icon(LocalIcon::ArrowLeft, FOREGROUND).size_full()),
                ),
        )
        .into_any_element()
}

pub(super) fn render_top_toolbar(
    app: &mut RalgrumApp,
    window: &Window,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement + use<> {
    let search_detail_open = app.nav == super::Nav::Discover && app.search.read(cx).detail_open();
    let search_channel_open =
        app.nav == super::Nav::Discover && app.search.read(cx).discover_channel_open();
    let search_results_root_open =
        app.nav == super::Nav::Discover && app.search.read(cx).search_results_root_open();
    let library_detail_open = app.nav == super::Nav::Library && app.library.read(cx).detail_open();
    let search_navigation_open =
        search_detail_open || search_channel_open || search_results_root_open;
    let detail_open = search_navigation_open || library_detail_open;
    let search_detail_back_label = search_navigation_back_label(
        search_channel_open,
        search_results_root_open,
        app.external_detail_return,
    );
    let library_detail_back_label = if app.library.read(cx).discover_flow_returns_to_discover() {
        "Back to Discover"
    } else {
        "Back to library"
    };
    let show_search_source = app.nav == super::Nav::Discover;
    let show_library_service = app.nav == super::Nav::Library;
    let source_items = [
        SourceItem {
            label: "All",
            icon: LocalIcon::EarthAmericas,
            source: Source::All,
            full_width: SOURCE_ALL_WIDTH,
            compact_width: SOURCE_ALL_WIDTH,
            label_width: 16.,
        },
        SourceItem {
            label: "Deezer",
            icon: LocalIcon::Deezer,
            source: Source::Deezer,
            full_width: 90.,
            compact_width: 34.,
            label_width: 40.,
        },
        SourceItem {
            label: "SoundCloud",
            icon: LocalIcon::SoundCloud,
            source: Source::SoundCloud,
            full_width: 122.,
            compact_width: 34.,
            label_width: 72.,
        },
    ];
    let library_items = [
        LibraryServiceItem {
            label: "Local",
            icon: LocalIcon::FolderOpen,
            service: LibraryService::Local,
            full_width: 70.,
            compact_width: 34.,
            label_width: 32.,
        },
        LibraryServiceItem {
            label: "Deezer",
            icon: LocalIcon::Deezer,
            service: LibraryService::Deezer,
            full_width: 90.,
            compact_width: 34.,
            label_width: 40.,
        },
        LibraryServiceItem {
            label: "SoundCloud",
            icon: LocalIcon::SoundCloud,
            service: LibraryService::SoundCloud,
            full_width: 122.,
            compact_width: 34.,
            label_width: 72.,
        },
    ];
    let search_entity = app.search.read(cx).input.clone();
    let show_search_clear = !search_entity.read(cx).value().is_empty();
    let search_clear_active = show_search_source && show_search_clear;
    let search_placeholder_visibility = search_placeholder_visibility(
        show_search_source,
        show_search_clear,
        search_entity.focus_handle(cx).is_focused(window),
    );
    let now = Instant::now();
    let search_clear_visual =
        app.search_clear_motion
            .prepare(search_clear_active, now, cx.reduce_motion());
    let search_clear_settled_hidden =
        !search_clear_active && search_clear_visual.from == 0. && search_clear_visual.target == 0.;
    let suggestion_rows = app.search.read(cx).suggestion_rows(cx);
    let selected_suggestion = app.search.read(cx).selected_suggestion();
    let show_suggestions = should_show_search_suggestions(
        show_search_source,
        app.search.read(cx).suggestions_visible(cx),
    );
    let library_entity = app.library.read(cx).input.clone();
    let show_library_clear = !library_entity.read(cx).value().is_empty();
    let search_input = text_field_context_menu(
        Input::new(&search_entity)
            .appearance(true)
            .bordered(true)
            .focus_bordered(true)
            .min_h(px(38.))
            .max_h(px(38.))
            .pl(px(36.))
            .pr(px(search_input_trailing_padding(show_search_clear)))
            .text_size(px(13.5))
            .rounded(px(8.))
            .tab_index(if show_search_source { 0 } else { -1 }),
        search_entity.clone(),
    );
    let placeholder_input = search_entity.clone();
    let search_placeholder = div()
        .id("music-search-placeholder")
        .absolute()
        .top(px(0.))
        .left(px(SEARCH_INPUT_TEXT_INSET_PX))
        .right(px(SEARCH_INPUT_ACTION_PADDING_PX))
        .h_full()
        .min_w_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .cursor_text()
        .text_size(px(13.5))
        .text_color(rgb(MUTED))
        .when(!search_placeholder_visibility.custom_overlay, |this| {
            this.invisible()
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            placeholder_input.focus_handle(cx).focus(window, cx);
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .child(SEARCH_PLACEHOLDER),
        );
    let clear_search = plain_x_button(
        "clear-search",
        "clear-search",
        "Clear search",
        search_clear_active,
    )
    .absolute()
    .top(px(8.))
    .right(px(38.))
    .opacity(search_clear_visual.target)
    .when(search_clear_settled_hidden, |this| this.invisible())
    .when(!search_clear_active, |this| this.cursor(CursorStyle::Arrow))
    .when(search_clear_active, |this| {
        this.app_tooltip("Clear search")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.search
                        .update(cx, |search, cx| search.clear_query(window, cx));
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.search
                        .update(cx, |search, cx| search.clear_query(window, cx));
                }
            }))
    })
    .with_animation(
        ("search-clear-presence", search_clear_visual.epoch),
        crate::motion::content(),
        move |this, delta| {
            this.opacity(crate::motion::lerp(
                search_clear_visual.from,
                search_clear_visual.target,
                delta,
            ))
        },
    );
    let library_input = text_field_context_menu(
        Input::new(&library_entity)
            .appearance(true)
            .bordered(true)
            .focus_bordered(true)
            .min_h(px(38.))
            .max_h(px(38.))
            .pl(px(36.))
            .pr(px(44.))
            .text_size(px(13.5))
            .rounded(px(8.))
            .tab_index(if show_library_service { 0 } else { -1 }),
        library_entity.clone(),
    );
    let clear_library = plain_x_button(
        "clear-library-search",
        "clear-library-search",
        "Clear library search",
        show_library_service && show_library_clear,
    )
    .absolute()
    .top(px(8.))
    .right(px(4.))
    .when(!show_library_clear, |this| this.invisible())
    .when(!show_library_clear, |this| this.cursor(CursorStyle::Arrow))
    .when(show_library_clear, |this| {
        this.app_tooltip("Clear library search")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.library
                        .update(cx, |library, cx| library.clear_query(window, cx));
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.library
                        .update(cx, |library, cx| library.clear_query(window, cx));
                }
            }))
    });
    let submit_search = div()
        .id("submit-search")
        .focusable()
        .tab_stop(show_search_source)
        .role(Role::Button)
        .aria_label("Search")
        .absolute()
        .top(px(4.))
        .right(px(4.))
        .w(px(30.))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rgb(crate::theme::PRIMARY))
        .text_color(rgb(crate::theme::FOREGROUND))
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .child(local_icon(LocalIcon::ArrowRight, crate::theme::FOREGROUND).size(px(12.)))
        .app_tooltip("Search")
        .on_click(cx.listener(RalgrumApp::submit_search))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                window.prevent_default();
                this.submit_search_now(cx);
            }
        }));
    let viewport_width = f32::from(window.viewport_size().width);
    let metrics = crate::music_ui::shell_metrics(viewport_width);
    let toolbar_inset = if metrics.narrow_content {
        metrics.gutter
    } else {
        TOOLBAR_HORIZONTAL_INSET
    };
    let right_sidebar_open =
        app.playback.read(cx).state.right_sidebar != crate::playback::RightSidebar::Closed;
    let toolbar_width =
        toolbar_available_width(viewport_width, &metrics, right_sidebar_open, toolbar_inset);
    let search_source_labels = metrics.toolbar_stacked
        || toolbar_shows_full_labels(toolbar_width, SEARCH_SOURCE_SELECTOR_WIDTH);
    let library_service_labels = metrics.toolbar_stacked
        || toolbar_shows_full_labels(toolbar_width, LIBRARY_SERVICE_SELECTOR_WIDTH);
    let source_compact = !search_source_labels;
    let library_compact = !library_service_labels;
    let source_selector_target_width = source_selector_width(&source_items, source_compact);
    let library_selector_target_width = library_selector_width(&library_items, library_compact);
    let route_target_fraction = if app.nav == Nav::Library { 1. } else { 0. };
    let geometry_will_change = app.toolbar_geometry_motion.will_change(
        source_selector_target_width,
        library_selector_target_width,
        route_target_fraction,
    );
    let toolbar_geometry_visual = app.toolbar_geometry_motion.prepare(
        source_selector_target_width,
        library_selector_target_width,
        route_target_fraction,
        now,
        cx.reduce_motion(),
    );
    let source_index = match app.search.read(cx).source() {
        Source::All => 0,
        Source::Deezer => 1,
        Source::SoundCloud => 2,
    };
    let source_geometry = source_selector_geometry(&source_items, source_index, source_compact);
    let library_index = match app.library.read(cx).selection().0 {
        LibraryService::Local => 0,
        LibraryService::Deezer => 1,
        LibraryService::SoundCloud => 2,
    };
    let library_geometry =
        library_selector_geometry(&library_items, library_index, library_compact);
    let platform_geometry = if show_library_service {
        library_geometry
    } else {
        source_geometry
    };
    app.source_selector_motion.retarget(
        platform_geometry.x,
        platform_geometry.width,
        now,
        cx.reduce_motion(),
        geometry_will_change,
    );
    let platform_indicator = app.source_selector_motion;
    let selector_fraction = toolbar_geometry_visual.target_route_fraction;
    let selector_host_width = toolbar_selector_width(
        toolbar_geometry_visual.target_source_width,
        toolbar_geometry_visual.target_library_width,
        selector_fraction,
    );
    let platform_service_responsive = platform_service_responsive_visual(toolbar_geometry_visual);
    let platform_service_target_width = platform_service_width(toolbar_geometry_visual, 1.);
    let platform_all_target_width = platform_all_width(toolbar_geometry_visual, 1.);
    let platform_all_target_opacity = toolbar_route_opacities(selector_fraction).0;
    let platform_all = div()
        .absolute()
        .left(px(SELECTOR_PADDING))
        .top(px(SELECTOR_PADDING))
        .h(px(30.))
        .w(px(platform_all_target_width))
        .overflow_hidden()
        .opacity(platform_all_target_opacity)
        .with_animation(
            ("toolbar-selector-all", toolbar_geometry_visual.epoch),
            crate::motion::content(),
            move |this, delta| {
                this.w(px(platform_all_width(toolbar_geometry_visual, delta)))
                    .opacity(
                        toolbar_route_opacities(toolbar_geometry_visual.route_fraction_at(delta)).0,
                    )
            },
        )
        .child(app.platform_item(
            PlatformItem::Search(source_items[0]),
            platform_service_responsive,
            show_search_source,
            cx,
        ));
    let platform_service_items: Vec<PlatformItem> = if show_library_service {
        library_items
            .into_iter()
            .map(PlatformItem::Library)
            .collect()
    } else {
        source_items[1..]
            .iter()
            .copied()
            .map(PlatformItem::Search)
            .collect()
    };
    let platform_service_children = platform_service_items
        .into_iter()
        .map(|item| app.platform_item(item, platform_service_responsive, true, cx))
        .collect::<Vec<_>>();
    let platform_services = div()
        .absolute()
        .right(px(SELECTOR_PADDING))
        .top(px(SELECTOR_PADDING))
        .h(px(30.))
        .w(px(platform_service_target_width))
        .flex()
        .gap(px(SELECTOR_ITEM_GAP))
        .overflow_hidden()
        .with_animation(
            ("toolbar-selector-services", toolbar_geometry_visual.epoch),
            crate::motion::content(),
            move |this, delta| this.w(px(platform_service_width(toolbar_geometry_visual, delta))),
        )
        .children(platform_service_children);
    let platform_selector = div()
        .id("music-selector-platform")
        .absolute()
        .inset_0()
        .size_full()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        .role(Role::TabList)
        .aria_label("Search source or library service")
        .child(selector_indicator(
            "music-selector-platform",
            platform_indicator,
        ))
        .child(platform_all)
        .child(platform_services);
    let (source_route_opacity, library_route_opacity) =
        toolbar_search_layer_opacities(toolbar_geometry_visual);
    let discover_search_layer = div()
        .id("music-search-discover-layer")
        .absolute()
        .inset_0()
        .size_full()
        .when(show_search_source, |this| this.occlude())
        .opacity(source_route_opacity)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            let direction = match event.keystroke.key.as_str() {
                "up" => Some(-1),
                "down" => Some(1),
                "escape" => {
                    if this.search.read(cx).suggestions_visible(cx) {
                        window.prevent_default();
                        cx.stop_propagation();
                        this.search
                            .update(cx, |search, cx| search.dismiss_suggestions(cx));
                    }
                    None
                }
                "delete" => {
                    if this
                        .search
                        .update(cx, |search, cx| search.remove_selected_search_history(cx))
                    {
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                    None
                }
                _ => None,
            };
            if let Some(direction) = direction
                && this.search.update(cx, |search, cx| {
                    search.select_relative_suggestion(direction, cx)
                })
            {
                window.prevent_default();
                cx.stop_propagation();
            }
        }))
        .child(search_input)
        .child(search_placeholder)
        .child(
            local_icon(LocalIcon::MagnifyingGlass, MUTED)
                .size(px(14.))
                .absolute()
                .left(px(12.))
                .top(px(12.)),
        )
        .child(clear_search)
        .child(submit_search)
        .when(show_suggestions, |this| {
            this.child(render_search_suggestions(
                suggestion_rows,
                selected_suggestion,
                cx,
            ))
        })
        .into_any_element();
    let library_search_layer = div()
        .id("music-search-library-layer")
        .absolute()
        .inset_0()
        .size_full()
        .when(show_library_service, |this| this.occlude())
        .opacity(library_route_opacity)
        .child(library_input)
        .child(
            local_icon(LocalIcon::MagnifyingGlass, MUTED)
                .size(px(14.))
                .absolute()
                .left(px(12.))
                .top(px(12.)),
        )
        .child(clear_library)
        .into_any_element();
    let search_layers = if show_search_source {
        [library_search_layer, discover_search_layer]
    } else {
        [discover_search_layer, library_search_layer]
    };
    let detail_toolbar_visual =
        app.detail_toolbar_motion
            .prepare(detail_open, now, cx.reduce_motion());
    let detail_toolbar_initial_progress = detail_toolbar_visual.from_progress;
    let (detail_toolbar_initial_slot, detail_toolbar_initial_back) =
        detail_toolbar_phases(detail_toolbar_initial_progress);
    sync_detail_toolbar_interaction(
        app,
        detail_open,
        detail_toolbar_initial_back,
        cx.reduce_motion(),
        cx,
    );
    let detail_back_slot_width = crate::music_ui::DETAIL_BACK_BUTTON_SIZE
        + crate::music_ui::DETAIL_BACK_BUTTON_MARGIN_RIGHT
        + DETAIL_TOOLBAR_BACK_SLOT_GAP;
    let detail_back_slot_height =
        crate::music_ui::DETAIL_BACK_BUTTON_SIZE + DETAIL_TOOLBAR_STACKED_BACK_SLOT_GAP;
    let detail_back_interactive =
        detail_back_is_interactive(detail_open, app.detail_toolbar_focus_ready);
    let detail_back_button = if library_detail_open {
        render_detail_back_button(
            "library-detail-back",
            "library-detail-back",
            library_detail_back_label,
            library_detail_back_label,
            detail_back_interactive,
            cx.listener(|this, _, _, cx| {
                this.library.update(cx, |library, cx| library.back(cx));
            }),
            cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.library.update(cx, |library, cx| library.back(cx));
                }
            }),
        )
    } else {
        render_detail_back_button(
            "search-detail-back",
            "search-detail-back",
            search_detail_back_label,
            search_detail_back_label,
            detail_back_interactive,
            cx.listener(|this, _, window, cx| {
                this.close_search_toolbar_target(window, cx);
            }),
            cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    this.close_search_toolbar_target(window, cx);
                }
            }),
        )
    };
    let detail_back_slot = div()
        .id("detail-toolbar-back-slot")
        .flex_shrink_0()
        .overflow_hidden()
        .when(metrics.toolbar_stacked, |this| {
            this.w_full()
                .flex()
                .items_center()
                .h(px(detail_back_slot_height * detail_toolbar_initial_slot))
        })
        .when(!metrics.toolbar_stacked, |this| {
            this.w(px(detail_back_slot_width * detail_toolbar_initial_slot))
                .h(px(38.))
        })
        .opacity(detail_toolbar_initial_back)
        .when(!detail_open && detail_toolbar_initial_slot <= 0., |this| {
            this.occlude()
        })
        .with_animation(
            ("detail-toolbar-back-slot", detail_toolbar_visual.epoch),
            crate::motion::content(),
            move |this, delta| {
                let (slot, back) = detail_toolbar_phases(detail_toolbar_visual.progress_at(delta));
                if metrics.toolbar_stacked {
                    this.h(px(detail_back_slot_height * slot)).opacity(back)
                } else {
                    this.w(px(detail_back_slot_width * slot)).opacity(back)
                }
            },
        )
        .child(detail_back_button);
    let search_and_selector = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .when(metrics.toolbar_stacked, |this| this.w_full())
        .flex()
        .items_center()
        .gap(px(20.))
        .child(
            div()
                .id("music-search")
                .role(Role::Group)
                .aria_label("Music search")
                .relative()
                .h(px(38.))
                .flex_1()
                .min_w_0()
                .relative()
                .when(metrics.toolbar_stacked, |this| this.w_full())
                .when(!metrics.toolbar_stacked, |this| {
                    this.max_w(px(SEARCH_MAX_WIDTH))
                })
                .on_mouse_down_out(cx.listener(|_, _, window, cx| window.blur(cx)))
                .children(search_layers),
        )
        .child(
            div()
                .relative()
                .ml_auto()
                .flex_none()
                .h(px(38.))
                .overflow_hidden()
                .w(px(selector_host_width))
                .with_animation(
                    ("toolbar-selector-host", toolbar_geometry_visual.epoch),
                    crate::motion::content(),
                    move |this, delta| {
                        this.w(px(toolbar_selector_width(
                            toolbar_geometry_visual.source_width_at(delta),
                            toolbar_geometry_visual.library_width_at(delta),
                            toolbar_geometry_visual.route_fraction_at(delta),
                        )))
                    },
                )
                .child(platform_selector),
        );
    div()
        .h(px(38.))
        .when(metrics.toolbar_stacked, |this| this.flex_col().h_auto())
        .mt(px(20.))
        .mb(px(toolbar_bottom_margin(
            search_channel_open && !search_detail_open,
        )))
        .flex()
        .items_center()
        .mx(px(toolbar_inset))
        .child(detail_back_slot)
        .child(search_and_selector)
}

#[cfg(test)]
mod tests {
    use super::{
        DetailToolbarMotion, LIBRARY_SERVICE_SELECTOR_WIDTH, SEARCH_MAX_WIDTH,
        SEARCH_SOURCE_SELECTOR_WIDTH, SelectorItemGeometry, TOOLBAR_GAP, TOOLBAR_HORIZONTAL_INSET,
        TOOLBAR_MIN_SEARCH_WIDTH, ToolbarGeometryMotion, ToolbarGeometryVisual,
        detail_toolbar_phases, library_selector_width, platform_all_width,
        platform_service_compactness, platform_service_responsive_visual, platform_service_width,
        selector_item_geometry, selector_shell_width, source_selector_width,
        toolbar_available_width, toolbar_route_identity, toolbar_route_opacities,
        toolbar_search_layer_opacities, toolbar_selector_width, toolbar_shows_full_labels,
    };
    use crate::shell::Nav;
    use std::time::{Duration, Instant};

    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.001,
            "expected {expected}, got {actual}"
        );
    }

    fn geometry_host_width(visual: ToolbarGeometryVisual, delta: f32) -> f32 {
        toolbar_selector_width(
            visual.source_width_at(delta),
            visual.library_width_at(delta),
            visual.route_fraction_at(delta),
        )
    }

    #[test]
    fn search_box_matches_reference_width() {
        assert_eq!(SEARCH_MAX_WIDTH, 520.);
        assert_eq!(TOOLBAR_HORIZONTAL_INSET, 28.);
    }

    #[test]
    fn toolbar_width_accounts_for_sidebar_and_right_panel() {
        let metrics = crate::music_ui::shell_metrics(1200.);

        assert_eq!(
            toolbar_available_width(1200., &metrics, false, TOOLBAR_HORIZONTAL_INSET),
            1076.
        );
        assert_eq!(
            toolbar_available_width(1200., &metrics, true, TOOLBAR_HORIZONTAL_INSET),
            716.
        );
    }

    #[test]
    fn selector_labels_stay_visible_until_the_toolbar_needs_icon_only_mode() {
        assert!(toolbar_shows_full_labels(
            716.,
            SEARCH_SOURCE_SELECTOR_WIDTH
        ));
        assert!(toolbar_shows_full_labels(
            716.,
            LIBRARY_SERVICE_SELECTOR_WIDTH
        ));
        assert!(!toolbar_shows_full_labels(
            570.,
            SEARCH_SOURCE_SELECTOR_WIDTH
        ));
        assert!(!toolbar_shows_full_labels(
            570.,
            LIBRARY_SERVICE_SELECTOR_WIDTH
        ));
    }

    #[test]
    fn selector_label_cutoffs_include_the_larger_search_reserve() {
        assert_eq!(TOOLBAR_MIN_SEARCH_WIDTH, 300.);
        for selector_width in [SEARCH_SOURCE_SELECTOR_WIDTH, LIBRARY_SERVICE_SELECTOR_WIDTH] {
            let cutoff = selector_width + TOOLBAR_GAP + TOOLBAR_MIN_SEARCH_WIDTH;
            assert!(!toolbar_shows_full_labels(cutoff - 0.1, selector_width));
            assert!(toolbar_shows_full_labels(cutoff, selector_width));
        }
    }

    #[test]
    fn toolbar_route_identity_changes_with_the_search_context() {
        assert_eq!(toolbar_route_identity(Nav::Discover), "discover");
        assert_eq!(toolbar_route_identity(Nav::Library), "library");
        assert_eq!(toolbar_route_identity(Nav::Downloads), "downloads");
        assert_eq!(toolbar_route_identity(Nav::Cache), "cache");
    }

    #[test]
    fn narrow_stacked_toolbar_has_room_for_full_labels() {
        let metrics = crate::music_ui::shell_metrics(768.);
        let width = toolbar_available_width(768., &metrics, true, metrics.gutter);

        assert_eq!(width, 744.);
        assert!(toolbar_shows_full_labels(
            width,
            SEARCH_SOURCE_SELECTOR_WIDTH
        ));
    }

    #[test]
    fn selector_geometry_includes_inner_padding_and_item_gaps() {
        let widths = [70.0, 90.0, 122.0];

        assert_eq!(
            selector_item_geometry(&widths, 0),
            SelectorItemGeometry {
                x: 3.0,
                width: 70.0
            }
        );
        assert_eq!(
            selector_item_geometry(&widths, 1),
            SelectorItemGeometry {
                x: 75.0,
                width: 90.0
            }
        );
        assert_eq!(
            selector_item_geometry(&widths, 2),
            SelectorItemGeometry {
                x: 167.0,
                width: 122.0
            }
        );
    }

    #[test]
    fn compact_and_library_selector_geometry_uses_current_item_widths() {
        assert_eq!(
            selector_item_geometry(&[70.0, 34.0, 34.0], 2),
            SelectorItemGeometry {
                x: 111.0,
                width: 34.0
            }
        );
        assert_eq!(
            selector_item_geometry(&[70.0, 90.0, 122.0], 2),
            SelectorItemGeometry {
                x: 167.0,
                width: 122.0
            }
        );
        assert_eq!(
            selector_item_geometry(&[34.0, 34.0, 34.0], 2),
            SelectorItemGeometry {
                x: 75.0,
                width: 34.0
            }
        );
    }

    #[test]
    fn selector_shell_width_matches_all_route_label_modes() {
        assert_eq!(selector_shell_width(&[70., 90., 122.]), 292.);
        assert_eq!(selector_shell_width(&[70., 34., 34.]), 148.);
        assert_eq!(selector_shell_width(&[70., 90., 122.]), 292.);
        assert_eq!(selector_shell_width(&[34., 34., 34.]), 112.);

        let source_items = [
            super::SourceItem {
                label: "All",
                icon: crate::assets::LocalIcon::EarthAmericas,
                source: crate::search::Source::All,
                full_width: 70.,
                compact_width: 70.,
                label_width: 16.,
            },
            super::SourceItem {
                label: "Deezer",
                icon: crate::assets::LocalIcon::Deezer,
                source: crate::search::Source::Deezer,
                full_width: 90.,
                compact_width: 34.,
                label_width: 40.,
            },
            super::SourceItem {
                label: "SoundCloud",
                icon: crate::assets::LocalIcon::SoundCloud,
                source: crate::search::Source::SoundCloud,
                full_width: 122.,
                compact_width: 34.,
                label_width: 72.,
            },
        ];
        let library_items = [
            super::LibraryServiceItem {
                label: "Local",
                icon: crate::assets::LocalIcon::FolderOpen,
                service: crate::library::Service::Local,
                full_width: 70.,
                compact_width: 34.,
                label_width: 32.,
            },
            super::LibraryServiceItem {
                label: "Deezer",
                icon: crate::assets::LocalIcon::Deezer,
                service: crate::library::Service::Deezer,
                full_width: 90.,
                compact_width: 34.,
                label_width: 40.,
            },
            super::LibraryServiceItem {
                label: "SoundCloud",
                icon: crate::assets::LocalIcon::SoundCloud,
                service: crate::library::Service::SoundCloud,
                full_width: 122.,
                compact_width: 34.,
                label_width: 72.,
            },
        ];

        assert_eq!(source_selector_width(&source_items, false), 292.);
        assert_eq!(source_selector_width(&source_items, true), 148.);
        assert_eq!(library_selector_width(&library_items, false), 292.);
        assert_eq!(library_selector_width(&library_items, true), 112.);
    }

    #[test]
    fn shared_service_visual_stays_right_anchored_at_mixed_route_widths() {
        let visual = ToolbarGeometryVisual {
            from_source_width: 148.,
            target_source_width: 148.,
            from_library_width: 112.,
            target_library_width: 112.,
            from_route_fraction: 0.,
            target_route_fraction: 1.,
            epoch: 0,
        };

        assert_close(platform_service_width(visual, 0.), 70.);
        assert_close(platform_service_width(visual, 0.5), 88.);
        assert_close(platform_service_width(visual, 1.), 106.);
        assert_close(platform_all_width(visual, 0.), 72.);
        assert_close(platform_all_width(visual, 1.), 0.);
    }

    #[test]
    fn shared_service_visual_matches_both_selector_modes() {
        for (source_width, library_width, route_fraction, expected) in [
            (292., 292., 0., 214.),
            (148., 112., 0., 70.),
            (292., 292., 1., 286.),
            (148., 112., 1., 106.),
        ] {
            let visual = ToolbarGeometryVisual {
                from_source_width: source_width,
                target_source_width: source_width,
                from_library_width: library_width,
                target_library_width: library_width,
                from_route_fraction: route_fraction,
                target_route_fraction: route_fraction,
                epoch: 0,
            };
            assert_close(platform_service_width(visual, 0.), expected);
        }
    }

    #[test]
    fn toolbar_geometry_initializes_settled_at_exact_targets() {
        assert_eq!(toolbar_route_opacities(0.), (1., 0.));
        assert_eq!(toolbar_route_opacities(1.), (0., 1.));

        let now = Instant::now();
        let mut motion = ToolbarGeometryMotion::default();
        let visual = motion.prepare(292., 292., 0., now, false);

        assert_eq!(
            visual,
            ToolbarGeometryVisual {
                from_source_width: 292.,
                target_source_width: 292.,
                from_library_width: 292.,
                target_library_width: 292.,
                from_route_fraction: 0.,
                target_route_fraction: 0.,
                epoch: 0,
            }
        );
        assert_eq!(motion.started_at, None);
        assert_eq!(geometry_host_width(visual, 0.), 292.);
        assert_eq!(geometry_host_width(visual, 1.), 292.);
    }

    #[test]
    fn search_layers_switch_to_the_target_route_without_cross_fading() {
        let visual = ToolbarGeometryVisual {
            from_source_width: 292.,
            target_source_width: 292.,
            from_library_width: 292.,
            target_library_width: 292.,
            from_route_fraction: 0.,
            target_route_fraction: 1.,
            epoch: 1,
        };

        assert_eq!(toolbar_search_layer_opacities(visual), (0., 1.));
        assert_eq!(
            toolbar_route_opacities(visual.route_fraction_at(0.5)),
            (0.5, 0.5)
        );
    }

    #[test]
    fn toolbar_geometry_route_reversal_captures_the_live_halfway_state() {
        let now = Instant::now();
        let halfway = now + crate::motion::CONTENT_DURATION / 2;
        let halfway_delta = gpui::ease_in_out(0.5);
        let mut motion = ToolbarGeometryMotion::default();
        motion.prepare(292., 292., 0., now, false);
        let forward = motion.prepare(148., 112., 1., now, false);
        let live_source_width = forward.source_width_at(halfway_delta);
        let live_library_width = forward.library_width_at(halfway_delta);
        let live_route_fraction = forward.route_fraction_at(halfway_delta);
        let live_host_width = geometry_host_width(forward, halfway_delta);

        let reversed = motion.prepare(148., 112., 0., halfway, false);

        assert_close(reversed.source_width_at(0.), live_source_width);
        assert_close(reversed.library_width_at(0.), live_library_width);
        assert_close(reversed.route_fraction_at(0.), live_route_fraction);
        assert_close(geometry_host_width(reversed, 0.), live_host_width);
        assert_eq!(reversed.target_source_width, 148.);
        assert_eq!(reversed.target_library_width, 112.);
        assert_eq!(reversed.target_route_fraction, 0.);
        assert_eq!(reversed.epoch, 2);
    }

    #[test]
    fn toolbar_geometry_derives_live_responsive_visuals_after_interruption() {
        let now = Instant::now();
        let halfway = now + crate::motion::CONTENT_DURATION / 2;
        let halfway_delta = gpui::ease_in_out(0.5);
        let mut motion = ToolbarGeometryMotion::default();
        motion.prepare(292., 292., 0., now, false);
        let forward = motion.prepare(148., 112., 1., now, false);
        let live_service_compactness = platform_service_compactness(
            platform_service_width(forward, halfway_delta),
            forward.route_fraction_at(halfway_delta),
        );

        let interrupted = motion.prepare(148., 112., 0., halfway, false);
        let service = platform_service_responsive_visual(interrupted);

        assert_close(service.from, live_service_compactness);
        assert_eq!(service.target, 1.);
        assert!(service.target_compact);
        assert_eq!(service.epoch, interrupted.epoch);
    }

    #[test]
    fn toolbar_geometry_responsive_retarget_captures_all_live_components() {
        let now = Instant::now();
        let halfway = now + crate::motion::CONTENT_DURATION / 2;
        let halfway_delta = gpui::ease_in_out(0.5);
        let mut motion = ToolbarGeometryMotion::default();
        motion.prepare(292., 292., 0., now, false);
        let compacting = motion.prepare(148., 112., 1., now, false);
        let live_source_width = compacting.source_width_at(halfway_delta);
        let live_library_width = compacting.library_width_at(halfway_delta);
        let live_route_fraction = compacting.route_fraction_at(halfway_delta);
        let live_host_width = geometry_host_width(compacting, halfway_delta);

        let retargeted = motion.prepare(292., 292., 1., halfway, false);

        assert_close(retargeted.source_width_at(0.), live_source_width);
        assert_close(retargeted.library_width_at(0.), live_library_width);
        assert_close(retargeted.route_fraction_at(0.), live_route_fraction);
        assert_close(geometry_host_width(retargeted, 0.), live_host_width);
        assert_eq!(retargeted.target_source_width, 292.);
        assert_eq!(retargeted.target_library_width, 292.);
        assert_eq!(retargeted.target_route_fraction, 1.);
        assert_eq!(retargeted.epoch, 2);
    }

    #[test]
    fn toolbar_geometry_reduced_motion_settles_immediately() {
        let now = Instant::now();
        let mut motion = ToolbarGeometryMotion::default();
        motion.prepare(292., 292., 0., now, false);
        motion.prepare(148., 112., 1., now, false);

        let reduced = motion.prepare(148., 112., 1., now + Duration::from_millis(1), true);

        assert_eq!(reduced.source_width_at(0.), 148.);
        assert_eq!(reduced.library_width_at(0.), 112.);
        assert_eq!(reduced.route_fraction_at(0.), 1.);
        assert_eq!(geometry_host_width(reduced, 0.), 112.);
        assert_eq!(motion.started_at, None);
    }

    #[test]
    fn toolbar_geometry_settles_after_content_duration() {
        let now = Instant::now();
        let mut motion = ToolbarGeometryMotion::default();
        motion.prepare(292., 292., 0., now, false);
        motion.prepare(148., 112., 1., now, false);

        let settled = motion.prepare(148., 112., 1., now + crate::motion::CONTENT_DURATION, false);

        assert_eq!(settled.source_width_at(0.), 148.);
        assert_eq!(settled.library_width_at(0.), 112.);
        assert_eq!(settled.route_fraction_at(0.), 1.);
        assert_eq!(motion.started_at, None);
    }

    #[test]
    fn detail_toolbar_phases_move_search_before_back_button() {
        assert_eq!(detail_toolbar_phases(0.), (0., 0.));
        assert_eq!(detail_toolbar_phases(0.25), (0.5, 0.));
        assert_eq!(detail_toolbar_phases(0.5), (1., 0.));
        assert_eq!(detail_toolbar_phases(0.75), (1., 0.5));
        assert_eq!(detail_toolbar_phases(1.), (1., 1.));
    }

    #[test]
    fn detail_toolbar_motion_reverses_from_the_live_progress() {
        let now = Instant::now();
        let halfway = now + crate::motion::CONTENT_DURATION / 2;
        let mut motion = DetailToolbarMotion::default();
        motion.prepare(false, now, false);
        let opening = motion.prepare(true, now, false);
        let live = opening.progress_at(gpui::ease_in_out(0.5));
        let closing = motion.prepare(false, halfway, false);

        assert_close(closing.from_progress, live);
        assert_eq!(closing.target_progress, 0.);
        assert_eq!(closing.epoch, 2);
    }

    #[test]
    fn detail_toolbar_reduced_motion_settles_without_intermediate_focus_slot() {
        let now = Instant::now();
        let mut motion = DetailToolbarMotion::default();
        motion.prepare(false, now, false);
        let opened = motion.prepare(true, now, true);
        assert_eq!(opened.from_progress, 1.);
        assert_eq!(opened.target_progress, 1.);
        assert_eq!(detail_toolbar_phases(opened.progress_at(0.)), (1., 1.));
    }
}
