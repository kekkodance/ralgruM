use gpui::{
    AnimationExt as _, AnyElement, App, ClickEvent, CursorStyle, Div, ElementId, FontWeight,
    HighlightStyle, InteractiveText, ObjectFit, ScrollHandle, ScrollWheelEvent, SharedString,
    StyledText, TextAlign, UnderlineStyle, Window, canvas, div, fill, img, prelude::*, px, rgb,
    rgba, svg,
};
use std::{
    cell::Cell,
    collections::HashMap,
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::{Mutex, OnceLock},
    time::Instant,
};
use url::Url;

mod card_grid_motion;
mod card_scrollbar;
mod carousel_motion;
mod resize_columns;
mod track_skeleton;

pub(crate) use card_grid_motion::{CardGridMotion, CardGridVisual, animate_grid_card};
pub(crate) use resize_columns::{
    ResizeRequest, ResizeSettledColumns, ResizeSettledTarget, schedule_resize,
};
pub(crate) use track_skeleton::{
    TRACK_ACTION_GAP_PX, TRACK_ACTION_SIZE_PX, TRACK_ARTWORK_SIZE_PX, TRACK_DURATION_WIDTH_PX,
    TRACK_ROW_CHILD_GAP_PX, TRACK_ROW_PADDING_X_PX, TRACK_ROW_PADDING_Y_PX, TrackSkeletonContext,
    track_skeleton_count, track_skeleton_count_with_minimum, track_skeleton_list,
};

use crate::{
    app_button::{DANGER_SECONDARY_HOVER_BACKGROUND, DANGER_SECONDARY_HOVER_TEXT},
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    entity_navigation::{MenuRoute, MenuRouteKind, NavigationOpener, NavigationTarget},
    playing_indicator::{RowPlaying, index_slot},
    search::Provider,
    theme::{
        BACKGROUND, BORDER, DANGER, DEEZER, FOREGROUND, MUTED, PRIMARY, SCROLLBAR_THUMB,
        SOUNDCLOUD, SURFACE, SURFACE_RAISED,
    },
};

pub(crate) fn fitted_columns(available_width: f32) -> u16 {
    (((available_width + 12.) / 192.).ceil().max(2.)) as u16
}

pub(crate) const MOBILE_MAX: f32 = 768.;
pub(crate) const COMPACT_MAX: f32 = 1200.;
pub(crate) const COMPACT_PLAYER_MAX: f32 = 1440.;
// The original toolbar switches to its stacked layout with the narrow/mobile
// content breakpoint. Keeping the desktop/compact range in one row lets the
// search field participate in normal flex sizing when the right panel opens.
pub(crate) const TOOLBAR_STACK_MAX: f32 = MOBILE_MAX;
#[cfg(test)]
pub(crate) const SETTINGS_FOLDER_STACK_MAX: f32 = 640.;
pub(crate) const MOBILE_DRAWER_WIDTH: f32 = 280.;

// Detail back controls share these dimensions across the search toolbar and
// nested library routes. Keeping the geometry centralized prevents Flow and
// other library detail pages from drifting from the original square control.
pub(crate) const DETAIL_BACK_BUTTON_SIZE: f32 = 38.;
pub(crate) const DETAIL_BACK_BUTTON_RADIUS: f32 = 6.;
pub(crate) const NAVIGATION_ICON_GLYPH_SIZE: f32 = 16.;
pub(crate) const DETAIL_BACK_ICON_SIZE: f32 = NAVIGATION_ICON_GLYPH_SIZE;
pub(crate) const DETAIL_BACK_BUTTON_MARGIN_RIGHT: f32 = -8.;

// Track rows keep the provider label until the row's effective content area
// is genuinely constrained. This is intentionally based on content width,
// rather than the viewport breakpoint used by the original CSS, because the
// left and right chrome can consume very different amounts of space.
pub(crate) const TRACK_PROVIDER_LABEL_MIN_WIDTH: f32 = 540.;
pub(crate) const TRACK_PROVIDER_FULL_COLUMN_WIDTH: f32 = 148.;
pub(crate) const TRACK_PROVIDER_COMPACT_COLUMN_WIDTH: f32 = 76.;
pub(crate) const TRACK_PROVIDER_DURATION_GAP_PX: f32 = 2.;
pub(crate) const TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX: f32 = 1.;
pub(crate) const TRACK_TITLE_ARTIST_GAP_PX: f32 = 1.;

pub(crate) fn compact_desktop_viewport(width: f32) -> bool {
    ((MOBILE_MAX + 1.)..=COMPACT_MAX).contains(&width)
}

pub(crate) fn narrow_content_viewport(width: f32) -> bool {
    width <= MOBILE_MAX
}

pub(crate) fn compact_track_provider(content_width: f32) -> bool {
    content_width < TRACK_PROVIDER_LABEL_MIN_WIDTH
}

pub(crate) fn track_provider_column_width(compact: bool) -> f32 {
    if compact {
        TRACK_PROVIDER_COMPACT_COLUMN_WIDTH
    } else {
        TRACK_PROVIDER_FULL_COLUMN_WIDTH
    }
}

fn track_provider_column_endpoints(visual: crate::motion::ResponsiveModeVisual) -> (f32, f32) {
    visual.endpoints(
        track_provider_column_width(false),
        track_provider_column_width(true),
    )
}

fn track_provider_label_width(provider: Provider) -> f32 {
    provider.label().chars().count() as f32 * 6.5
}

pub(crate) struct ShellMetrics {
    pub(crate) compact_desktop: bool,
    pub(crate) narrow_content: bool,
    pub(crate) sidebar_width: f32,
    pub(crate) gutter: f32,
    pub(crate) available_width: f32,
    pub(crate) right_sidebar_width: f32,
    pub(crate) toolbar_stacked: bool,
    pub(crate) compact_player: bool,
}

pub(crate) const CATEGORY_LABEL_FIT_BUFFER: f32 = 48.;
pub(crate) const CATEGORY_TAB_ICON_WIDTH: f32 = 34.;
pub(crate) const CATEGORY_TAB_ICON_SIZE: f32 = 14.;

pub(crate) fn category_tab_icon(icon: LocalIcon, color: u32, size: f32) -> AnyElement {
    div()
        .size(px(CATEGORY_TAB_ICON_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(local_icon(icon, color).size(px(size)))
        .into_any_element()
}
pub(crate) const RESULT_COUNT_MIN_WIDTH: f32 = 640.;
pub(crate) const MAIN_CONTENT_INSET: f32 = 28.;
pub(crate) const NARROW_MAIN_CONTENT_INSET: f32 = 12.;

pub(crate) fn main_content_inset(metrics: &ShellMetrics) -> f32 {
    if metrics.narrow_content {
        NARROW_MAIN_CONTENT_INSET
    } else {
        MAIN_CONTENT_INSET
    }
}

pub(crate) fn effective_content_width(metrics: &ShellMetrics, right_sidebar_open: bool) -> f32 {
    let center_width = (metrics.available_width + 2. * metrics.gutter).max(0.);
    let right_sidebar = if right_sidebar_open && !metrics.narrow_content {
        metrics.right_sidebar_width
    } else {
        0.
    };
    (center_width - right_sidebar - 2. * main_content_inset(metrics)).max(0.)
}

pub(crate) fn category_tabs_label_width(labels: &[&str]) -> f32 {
    const ICON_WIDTH: f32 = 13.;
    const LABEL_GAP: f32 = 7.;
    const HORIZONTAL_PADDING: f32 = 16.;
    const BORDER_WIDTH: f32 = 2.;
    const GROUP_PADDING: f32 = 6.;
    const TAB_GAP: f32 = 2.;

    labels
        .iter()
        .map(|label| {
            ICON_WIDTH
                + LABEL_GAP
                + category_tab_text_width(label)
                + HORIZONTAL_PADDING
                + BORDER_WIDTH
        })
        .sum::<f32>()
        + GROUP_PADDING
        + labels.len().saturating_sub(1) as f32 * TAB_GAP
        + BORDER_WIDTH
}

pub(crate) fn category_tab_text_width(label: &str) -> f32 {
    const LABEL_CHAR_WIDTH: f32 = 6.5;
    const FONT_METRIC_ALLOWANCE: f32 = 4.;

    label.chars().count() as f32 * LABEL_CHAR_WIDTH + FONT_METRIC_ALLOWANCE
}

pub(crate) fn category_tabs_icon_only(available_width: f32, labels: &[&str]) -> bool {
    available_width < category_tabs_label_width(labels) + CATEGORY_LABEL_FIT_BUFFER
}

pub(crate) fn category_tab_tooltip(icon_only: bool, label: &'static str) -> Option<&'static str> {
    icon_only.then_some(label)
}

pub(crate) fn show_search_result_count(available_width: f32) -> bool {
    available_width > RESULT_COUNT_MIN_WIDTH
}

pub(crate) fn shell_metrics(width: f32) -> ShellMetrics {
    shell_metrics_for_viewport(width, f32::INFINITY)
}

pub(crate) fn shell_metrics_for_viewport(width: f32, _height: f32) -> ShellMetrics {
    let compact_desktop = compact_desktop_viewport(width);
    let narrow_content = narrow_content_viewport(width);
    let sidebar_width = if narrow_content {
        0.
    } else if compact_desktop {
        68.
    } else {
        240.
    };
    let gutter = if narrow_content {
        12.
    } else if compact_desktop {
        16.
    } else {
        28.
    };
    ShellMetrics {
        compact_desktop,
        narrow_content,
        sidebar_width,
        gutter,
        available_width: (width - sidebar_width - gutter * 2.).max(0.),
        right_sidebar_width: if narrow_content {
            width.max(0.)
        } else {
            (width * 0.30).clamp(320., 400.)
        },
        toolbar_stacked: width <= TOOLBAR_STACK_MAX,
        compact_player: ((MOBILE_MAX + 1.)..=COMPACT_PLAYER_MAX).contains(&width),
    }
}

pub(crate) fn card_row_metrics(narrow: bool) -> (f32, f32) {
    if narrow { (126., 9.) } else { (150., 12.) }
}

/// Return enough complete card-grid rows to cover the available viewport and
/// one additional row for a stable skeleton edge.
pub(crate) fn card_grid_skeleton_count(
    columns: u16,
    available_width: f32,
    available_height: f32,
) -> usize {
    const CARD_GRID_GAP: f32 = 12.;
    const CARD_GRID_ROW_EXTRA: f32 = 37.;

    let columns = usize::from(columns.max(2));
    let gaps = CARD_GRID_GAP * columns.saturating_sub(1) as f32;
    let card_width = ((available_width.max(0.) - gaps) / columns as f32).max(1.);
    let row_pitch = card_width + CARD_GRID_ROW_EXTRA + CARD_GRID_GAP;
    let rows = ((available_height.max(row_pitch) / row_pitch).ceil() as usize).saturating_add(1);
    columns.saturating_mul(rows)
}

#[cfg(test)]
pub(crate) fn visible_preview_cards(available_width: f32, narrow: bool) -> usize {
    let (card_width, gap) = card_row_metrics(narrow);
    (((available_width.max(card_width) + gap) / (card_width + gap)).floor() as usize).max(1)
}

/// Whether a horizontally scrolling card row still has content beyond its
/// visible right edge. Scroll offsets are negative in GPUI, while the maximum
/// offset is reported as a positive extent.
pub(crate) fn card_row_has_more(scroll_offset: f32, max_offset: f32) -> bool {
    let extent = max_offset.abs();
    let distance = (-scroll_offset).clamp(0., extent);
    extent - distance > 2.
}

pub(crate) fn card_carousel_has_overflow(
    item_count: usize,
    card_width: f32,
    row_gap: f32,
    viewport_width: f32,
    horizontal_padding: f32,
) -> bool {
    if item_count == 0 {
        return false;
    }
    let content_width = item_count as f32 * card_width.max(0.)
        + item_count.saturating_sub(1) as f32 * row_gap.max(0.)
        + horizontal_padding.max(0.) * 2.;
    card_row_has_more(0., (content_width - viewport_width.max(0.)).max(0.))
}

pub(crate) fn horizontal_scroll_id(prefix: &str, identity: &str, index: usize) -> String {
    format!("{prefix}-{identity}-{index}")
}

pub(crate) fn should_consume_horizontal_scroll(x: f32, y: f32) -> bool {
    x != 0.0 && (y == 0.0 || x.abs() > y.abs())
}

pub(crate) fn horizontal_scroll_boundary(
    id: impl Into<ElementId>,
    child: AnyElement,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .on_scroll_wheel(|event: &ScrollWheelEvent, window, cx| {
            let delta = event.delta.pixel_delta(window.line_height());
            // Let vertical wheel events bubble to the page. Only horizontal
            // trackpad motion belongs to this row.
            if should_consume_horizontal_scroll(f32::from(delta.x), f32::from(delta.y)) {
                cx.stop_propagation();
            }
        })
        .child(child)
}

pub(crate) const CAROUSEL_ARROW_WIDTH: f32 = 10.;
pub(crate) const CAROUSEL_ARROW_TRACK_GAP: f32 = 2.;
pub(crate) const CAROUSEL_TRACK_INSET: f32 = CAROUSEL_ARROW_WIDTH + CAROUSEL_ARROW_TRACK_GAP;
pub(crate) const CAROUSEL_CONTROL_HEIGHT: f32 = 10.;
pub(crate) const CAROUSEL_THUMB_HEIGHT: f32 = 6.;
pub(crate) const CAROUSEL_CARD_ROW_BOTTOM_GAP: f32 = 8.;
pub(crate) const CAROUSEL_CONTENT_BOTTOM_PADDING: f32 =
    CAROUSEL_CARD_ROW_BOTTOM_GAP + CAROUSEL_CONTROL_HEIGHT;
pub(crate) const CAROUSEL_STABLE_CARD_LIMIT: usize = 64;

pub(crate) fn card_carousel_display_count(item_count: usize, base_limit: usize) -> usize {
    item_count.min(base_limit.max(CAROUSEL_STABLE_CARD_LIMIT))
}

#[derive(Clone)]
pub(crate) struct CardCarouselState {
    pub(crate) scroll_handle: ScrollHandle,
    pub(crate) drag: Rc<Cell<Option<CarouselDrag>>>,
    pub(crate) pending_target: Rc<Cell<Option<f32>>>,
    pub(crate) snap_epoch: Rc<Cell<u64>>,
}

impl CardCarouselState {
    pub(crate) fn new() -> Self {
        Self {
            scroll_handle: ScrollHandle::new(),
            drag: Rc::new(Cell::new(None)),
            pending_target: Rc::new(Cell::new(None)),
            snap_epoch: Rc::new(Cell::new(0)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CarouselDrag {
    pub(crate) pointer_x: f32,
    pub(crate) start_offset: f32,
    pub(crate) scale: f32,
    pub(crate) mode: CarouselDragMode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CarouselDragMode {
    Viewport,
    Scrollbar,
}

/// Paint the original card-row right-edge fade from the live scroll bounds.
/// Keeping this in the shared carousel means search, library, and detail card
/// sections all use the same boundary behavior.
fn card_row_fade(scroll_handle: ScrollHandle) -> impl IntoElement {
    canvas(
        move |_, _, _| {
            card_row_has_more(
                f32::from(scroll_handle.offset().x),
                f32::from(scroll_handle.max_offset().x),
            )
        },
        |bounds, has_more, window, _| {
            if has_more {
                window.paint_quad(fill(
                    bounds,
                    gpui::linear_gradient(
                        90.,
                        gpui::linear_color_stop(gpui::transparent_black(), 0.),
                        gpui::linear_color_stop(rgb(BACKGROUND), 1.),
                    ),
                ));
            }
        },
    )
    .absolute()
    .top(px(2.))
    .right(px(0.))
    .bottom(px(CAROUSEL_CARD_ROW_BOTTOM_GAP))
    .w(px(52.))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CarouselDirection {
    Previous,
    Next,
}

/// Calculate the adjacent card-aligned horizontal offset.
///
/// GPUI reports the maximum horizontal extent as positive, while offsets are
/// zero at the left edge and negative at the right edge. A final partial card
/// interval is treated as the right edge once it is within half a card pitch.
pub(crate) fn carousel_target_offset(
    current: f32,
    max_offset: f32,
    card_pitch: f32,
    direction: CarouselDirection,
) -> f32 {
    let extent = max_offset.abs();
    let current = current.clamp(-extent, 0.);
    let card_pitch = card_pitch.max(0.);
    if extent == 0. || card_pitch <= f32::EPSILON {
        return current;
    }

    let distance = (-current).clamp(0., extent);
    let ratio = distance / card_pitch;
    let nearest_boundary = ratio.round();
    let at_boundary = (ratio - nearest_boundary).abs() <= 0.001;
    let last_full_boundary = (extent / card_pitch).floor() * card_pitch;
    let final_partial_replaces_boundary = extent - last_full_boundary <= card_pitch * 0.5;
    if direction == CarouselDirection::Previous
        && distance >= last_full_boundary
        && final_partial_replaces_boundary
    {
        return -(last_full_boundary - card_pitch).max(0.);
    }
    let index = match direction {
        CarouselDirection::Previous => {
            if at_boundary {
                nearest_boundary as i32 - 1
            } else {
                ratio.floor() as i32
            }
        }
        CarouselDirection::Next => {
            if at_boundary {
                nearest_boundary as i32 + 1
            } else {
                ratio.ceil() as i32
            }
        }
    }
    .max(0) as f32;
    let regular_distance = (index * card_pitch).min(extent);
    if direction == CarouselDirection::Next && extent - regular_distance <= card_pitch * 0.5 {
        -extent
    } else {
        -regular_distance
    }
}

pub(crate) fn carousel_arrow_target(
    current: f32,
    max_offset: f32,
    card_pitch: f32,
    direction: CarouselDirection,
) -> f32 {
    carousel_target_offset(current, max_offset, card_pitch, direction)
}

fn carousel_arrow_target_from_pending(
    live_current: f32,
    pending_target: Option<f32>,
    max_offset: f32,
    card_pitch: f32,
    direction: CarouselDirection,
) -> f32 {
    carousel_arrow_target(
        pending_target.unwrap_or(live_current),
        max_offset,
        card_pitch,
        direction,
    )
}

/// Snap a dragged row to its nearest card boundary, including the final edge.
pub(crate) fn carousel_snap_offset(current: f32, max_offset: f32, card_pitch: f32) -> f32 {
    let extent = max_offset.abs();
    let current = current.clamp(-extent, 0.);
    let card_pitch = card_pitch.max(0.);
    if extent == 0. || card_pitch <= f32::EPSILON {
        return current;
    }

    let distance = (-current).clamp(0., extent);
    let regular_distance = ((distance / card_pitch).round() * card_pitch).min(extent);
    let regular_error = (regular_distance - distance).abs();
    if extent - distance <= regular_error {
        -extent
    } else {
        -regular_distance
    }
}

/// Clamp a middle-button drag offset to the scrollable range.
pub(crate) fn carousel_drag_offset(start_offset: f32, pointer_delta: f32, max_offset: f32) -> f32 {
    let extent = max_offset.abs();
    (start_offset + pointer_delta).clamp(-extent, 0.)
}

/// Render a card row with a persistent horizontal scrollbar and native-like
/// arrow controls at each end of the track. The caller owns the handle so it
/// survives view updates and can be shared by the scrollbar and arrows.
pub(crate) fn card_carousel(
    id: impl Into<String>,
    state: CardCarouselState,
    card_width: f32,
    row_gap: f32,
    controls_available: bool,
    content: AnyElement,
) -> AnyElement {
    let id = id.into();
    let viewport_id = format!("{id}-viewport");
    let pitch = card_width + row_gap;
    let previous = carousel_arrow(
        format!("{id}-previous"),
        CarouselDirection::Previous,
        state.clone(),
        pitch,
    );
    let next = carousel_arrow(
        format!("{id}-next"),
        CarouselDirection::Next,
        state.clone(),
        pitch,
    );
    div()
        .id(id)
        .relative()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .id(viewport_id)
                .flex()
                .flex_1()
                .min_w_0()
                .track_scroll(&state.scroll_handle)
                .overflow_x_scroll()
                .restrict_scroll_to_axis()
                .when(controls_available, |this| {
                    this.pb(px(CAROUSEL_CONTENT_BOTTOM_PADDING))
                })
                .child(content),
        )
        .child(card_row_fade(state.scroll_handle.clone()))
        .child(card_scrollbar::card_scrollbar(state.clone(), pitch))
        .when(controls_available, |this| this.child(previous).child(next))
        .into_any_element()
}

fn carousel_arrow(
    id: String,
    direction: CarouselDirection,
    state: CardCarouselState,
    card_pitch: f32,
) -> impl IntoElement {
    let hover_group: SharedString = format!("carousel-arrow-hover-{id}").into();
    let tooltip = match direction {
        CarouselDirection::Previous => "Scroll left",
        CarouselDirection::Next => "Scroll right",
    };
    let click_state = state.clone();
    let key_state = state.clone();
    div()
        .id(id)
        .absolute()
        .bottom(px(0.))
        .when(direction == CarouselDirection::Previous, |this| {
            this.left(px(0.))
        })
        .when(direction == CarouselDirection::Next, |this| {
            this.right(px(0.))
        })
        .w(px(CAROUSEL_ARROW_WIDTH))
        .h(px(CAROUSEL_CONTROL_HEIGHT))
        .flex()
        .items_center()
        .justify_center()
        .focusable()
        .tab_stop(true)
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .cursor_pointer()
        .group(hover_group.clone())
        .text_color(rgb(SCROLLBAR_THUMB))
        .hover(|style| style.bg(rgba(0x71717a2e)).text_color(rgb(FOREGROUND)))
        .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
        .on_click(move |_, window, cx| {
            activate_carousel_arrow(
                click_state.clone(),
                direction,
                card_pitch,
                cx.reduce_motion(),
                window,
            );
        })
        .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
            if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                window.prevent_default();
                activate_carousel_arrow(
                    key_state.clone(),
                    direction,
                    card_pitch,
                    cx.reduce_motion(),
                    window,
                );
            }
        })
        .app_tooltip(tooltip)
        .child(carousel_arrow_icon(direction, hover_group))
}

const CAROUSEL_ARROW_ICON_SIZE: f32 = 6.;
const CAROUSEL_ARROW_PREVIOUS_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 6 6"><path fill="currentColor" d="M1 3 5 0v6z"/></svg>"#;
const CAROUSEL_ARROW_NEXT_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 6 6"><path fill="currentColor" d="M1 0 5 3 1 6z"/></svg>"#;

fn carousel_arrow_icon(
    direction: CarouselDirection,
    hover_group: SharedString,
) -> impl IntoElement {
    div()
        .relative()
        .size(px(CAROUSEL_ARROW_ICON_SIZE))
        .flex_none()
        .child(
            div()
                .absolute()
                .inset_0()
                .group_hover(hover_group.clone(), |style| style.invisible())
                .child(carousel_arrow_svg(direction, SCROLLBAR_THUMB)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(hover_group, |style| style.visible())
                .child(carousel_arrow_svg(direction, FOREGROUND)),
        )
}

fn carousel_arrow_svg(direction: CarouselDirection, color: u32) -> gpui::Svg {
    let data = match direction {
        CarouselDirection::Previous => CAROUSEL_ARROW_PREVIOUS_SVG,
        CarouselDirection::Next => CAROUSEL_ARROW_NEXT_SVG,
    };
    svg()
        .data(data)
        .size(px(CAROUSEL_ARROW_ICON_SIZE))
        .text_color(rgb(color))
        .flex_none()
}

fn activate_carousel_arrow(
    state: CardCarouselState,
    direction: CarouselDirection,
    card_pitch: f32,
    reduced_motion: bool,
    window: &mut Window,
) {
    let live_current = f32::from(state.scroll_handle.offset().x);
    let target = carousel_arrow_target_from_pending(
        live_current,
        state.pending_target.get(),
        f32::from(state.scroll_handle.max_offset().x),
        card_pitch,
        direction,
    );
    state.pending_target.set(Some(target));
    carousel_motion::start(
        state.scroll_handle,
        state.snap_epoch,
        state.pending_target,
        target,
        reduced_motion,
        window,
    );
}

pub(crate) const COLLECTION_PRIVACY_ICON_SIZE: f32 = 12.;
pub(crate) const COLLECTION_PRIVACY_ICON_GAP: f32 = 6.;
pub(crate) const COLLECTION_PRIVACY_OPTICAL_OFFSET_PX: f32 = 1.;

pub(crate) fn playlist_privacy_icon(is_private: bool) -> LocalIcon {
    if is_private {
        LocalIcon::Lock
    } else {
        LocalIcon::EarthAmericas
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collection_card(
    title: &str,
    subtitle: &str,
    artwork: &str,
    kind: CardKind,
    provider: Option<Provider>,
    badge: Option<&str>,
    is_private: Option<bool>,
    id: &str,
) -> AnyElement {
    collection_card_with_title_alignment_source(
        title,
        subtitle,
        CardArtworkSource::Remote(artwork.to_owned()),
        kind,
        provider,
        badge,
        is_private,
        id,
        CollectionCardTitleAlignment::KindDefault,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn local_collection_card(
    title: &str,
    subtitle: &str,
    artwork: Option<PathBuf>,
    kind: CardKind,
    badge: Option<&str>,
    is_private: Option<bool>,
    id: &str,
) -> AnyElement {
    collection_card_with_title_alignment_source(
        title,
        subtitle,
        CardArtworkSource::Local(artwork),
        kind,
        None,
        badge,
        is_private,
        id,
        CollectionCardTitleAlignment::KindDefault,
    )
}

enum CardArtworkSource {
    Remote(String),
    Local(Option<PathBuf>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollectionCardTitleAlignment {
    KindDefault,
    Left,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn collection_card_with_title_alignment(
    title: &str,
    subtitle: &str,
    artwork: &str,
    kind: CardKind,
    provider: Option<Provider>,
    badge: Option<&str>,
    is_private: Option<bool>,
    id: &str,
    title_alignment: CollectionCardTitleAlignment,
) -> AnyElement {
    collection_card_with_title_alignment_source(
        title,
        subtitle,
        CardArtworkSource::Remote(artwork.to_owned()),
        kind,
        provider,
        badge,
        is_private,
        id,
        title_alignment,
    )
}

#[allow(clippy::too_many_arguments)]
fn collection_card_with_title_alignment_source(
    title: &str,
    subtitle: &str,
    artwork: CardArtworkSource,
    kind: CardKind,
    provider: Option<Provider>,
    badge: Option<&str>,
    is_private: Option<bool>,
    id: &str,
    title_alignment: CollectionCardTitleAlignment,
) -> AnyElement {
    let show_subtitle = crate::search::collection_subtitle_is_visible(subtitle)
        && !subtitle.eq_ignore_ascii_case(title);
    div()
        .flex()
        .flex_col()
        .gap(px(7.))
        .child(card_artwork(artwork, kind, provider, badge, id))
        .child(
            div()
                .min_w_0()
                .w_full()
                .px(px(2.))
                .pt(px(2.))
                .pb(px(3.))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_align(if title_alignment == CollectionCardTitleAlignment::Left {
                            TextAlign::Left
                        } else if matches!(kind, CardKind::Flow) {
                            TextAlign::Center
                        } else {
                            TextAlign::Left
                        })
                        .truncate()
                        .child(title.to_owned()),
                )
                .when(show_subtitle || is_private.is_some(), |this| {
                    this.child(
                        div()
                            .mt(px(2.))
                            .min_h(px(16.))
                            .flex()
                            .items_center()
                            .gap(px(COLLECTION_PRIVACY_ICON_GAP))
                            .min_w_0()
                            .when(show_subtitle, |this| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .flex_shrink_1()
                                        .text_size(px(11.))
                                        .text_color(rgb(MUTED))
                                        .truncate()
                                        .child(subtitle.to_owned()),
                                )
                            })
                            .when_some(is_private, |this, is_private| {
                                this.child(
                                    div()
                                        .id(format!(
                                            "collection-card-privacy-{}-{}",
                                            if id.is_empty() { title } else { id },
                                            if is_private { "private" } else { "public" }
                                        ))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .relative()
                                        .top(px(COLLECTION_PRIVACY_OPTICAL_OFFSET_PX))
                                        .app_tooltip(if is_private { "Private" } else { "Public" })
                                        .child(
                                            local_icon(playlist_privacy_icon(is_private), MUTED)
                                                .size(px(COLLECTION_PRIVACY_ICON_SIZE)),
                                        ),
                                )
                            }),
                    )
                }),
        )
        .into_any_element()
}

pub(crate) struct TrackRowDisplay {
    pub(crate) provider: Option<Provider>,
    pub(crate) narrow: bool,
    pub(crate) provider_icon_only: bool,
}

pub(crate) struct TrackArtistNavigation {
    pub(crate) provider: Provider,
    pub(crate) routes: Vec<MenuRoute>,
    pub(crate) opener: NavigationOpener,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn track_row_with_action(
    index: usize,
    artist_hover_scope: &str,
    title: &str,
    artist: &str,
    artist_navigation: Option<TrackArtistNavigation>,
    artwork: &str,
    duration: u64,
    display: TrackRowDisplay,
    explicit: bool,
    playing: RowPlaying,
    playing_slot: Option<AnyElement>,
    blocked: bool,
    action: Option<AnyElement>,
    secondary_action: Option<AnyElement>,
    tertiary_action: Option<AnyElement>,
    cx: &App,
) -> Div {
    let TrackRowDisplay {
        provider,
        narrow,
        provider_icon_only,
    } = display;
    // The playing row takes the accent color for its title. An explicit-blocked
    // row is grayed through the row-level opacity, matching the original's
    // graying of blocked rows.
    let title_color = if playing.is_current() {
        PRIMARY
    } else {
        FOREGROUND
    };
    let provider_compact = narrow || provider_icon_only;
    let provider_animation_key = format!("{artist_hover_scope}-{index}");
    let provider_visual =
        provider.map(|_| provider_mode_visual(provider_compact, cx.reduce_motion()));
    let (from_column_width, target_column_width) = provider_visual
        .map(track_provider_column_endpoints)
        .unwrap_or((
            TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
            TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
        ));
    let provider_column = div()
        .w(px(target_column_width))
        .flex_none()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(TRACK_PROVIDER_DURATION_GAP_PX))
        .when_some(provider, |this, provider| {
            this.child(provider_badge(
                provider,
                provider_visual.expect("provider rows have a responsive visual"),
                provider_animation_key.clone(),
            ))
        })
        .child(
            div()
                .w(px(TRACK_DURATION_WIDTH_PX))
                .flex_none()
                .text_size(px(12.))
                .text_color(rgb(MUTED))
                .text_align(TextAlign::Right)
                .child(format!("{}:{:02}", duration / 60, duration % 60)),
        );
    let provider_column = if let Some(provider_visual) = provider_visual {
        provider_column
            .with_animation(
                ElementId::named_usize(
                    format!("track-provider-column-{provider_animation_key}"),
                    provider_visual.epoch as usize,
                ),
                crate::motion::content(),
                move |this, delta| {
                    this.w(px(crate::motion::lerp(
                        from_column_width,
                        target_column_width,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        provider_column.into_any_element()
    };
    div()
        .min_h(crate::library::virtualization::row_content_height())
        .w_full()
        .flex()
        .items_center()
        .gap(px(TRACK_ROW_CHILD_GAP_PX))
        .px(px(TRACK_ROW_PADDING_X_PX))
        .py(px(TRACK_ROW_PADDING_Y_PX))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .hover(|style| style.bg(rgb(SURFACE)).border_color(rgb(BORDER)))
        .when(blocked, |this| {
            this.opacity(0.42).cursor(CursorStyle::OperationNotAllowed)
        })
        .when(!narrow, |this| {
            this.child(
                playing_slot.unwrap_or_else(|| index_slot(index, playing, cx).into_any_element()),
            )
        })
        .child(artwork_view(artwork))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(TRACK_TITLE_ARTIST_GAP_PX))
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(13.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(title_color))
                                .truncate()
                                .child(title.to_owned()),
                        )
                        .when(explicit, |this| this.child(explicit_badge())),
                )
                .child(track_artist_line(
                    index,
                    artist_hover_scope,
                    artist,
                    artist_navigation,
                    blocked,
                )),
        )
        .child(provider_column)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(TRACK_ACTION_GAP_PX))
                .when_some(action, |this, action| this.child(action))
                .when_some(secondary_action, |this, action| this.child(action))
                .when_some(tertiary_action, |this, action| this.child(action)),
        )
}

pub(crate) fn track_artist_line(
    index: usize,
    scope: &str,
    artist: &str,
    navigation: Option<TrackArtistNavigation>,
    blocked: bool,
) -> AnyElement {
    let hover_key = ArtistHoverKey {
        scope: scope.to_owned(),
        row: index,
        text: artist.to_owned(),
    };
    let Some(navigation) = navigation.filter(|navigation| {
        !blocked
            && navigation
                .routes
                .iter()
                .any(|route| route.kind == MenuRouteKind::Artist)
    }) else {
        set_artist_hover(&hover_key, None);
        return div()
            .text_size(px(12.))
            .text_color(rgb(MUTED))
            .truncate()
            .child(artist.to_owned())
            .into_any_element();
    };

    let provider = navigation.provider;
    let routes = navigation
        .routes
        .into_iter()
        .filter(|route| route.kind == MenuRouteKind::Artist)
        .collect::<Vec<_>>();
    let (text, clickable_ranges) = track_artist_text_and_ranges(artist, &routes, provider);
    let hovered_range =
        artist_hover(&hover_key).and_then(|route_index| clickable_ranges.get(route_index).cloned());
    let highlights = hovered_range.into_iter().map(|range| {
        (
            range,
            HighlightStyle {
                color: Some(rgb(FOREGROUND).into()),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(rgb(FOREGROUND).into()),
                    wavy: false,
                }),
                ..HighlightStyle::default()
            },
        )
    });
    let routes_for_click = routes;
    let opener = navigation.opener;
    let hover_ranges = clickable_ranges.clone();
    let text_for_hover = hover_key.clone();
    let interactive_text = InteractiveText::new(
        format!("track-artist-text-{scope}-{index}"),
        StyledText::new(text.clone()).with_highlights(highlights),
    )
    .on_click(clickable_ranges, move |route_index, window, cx| {
        if let Some(route) = routes_for_click.get(route_index) {
            cx.stop_propagation();
            opener(
                NavigationTarget::artist(route.id.clone(), route.title.clone()),
                window,
                cx,
            );
        }
    })
    .on_hover(move |index, _, _, _| {
        set_artist_hover(&text_for_hover, artist_route_at(index, &hover_ranges));
    });

    div()
        .id(format!("track-artists-{scope}-{index}"))
        .min_w_0()
        .max_w_full()
        .self_start()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .truncate()
        .on_hover({
            let hover_key = hover_key.clone();
            move |hovered, _, _| {
                if !*hovered {
                    set_artist_hover(&hover_key, None);
                }
            }
        })
        .aria_label(format!("Artists: {text}"))
        .child(interactive_text)
        .into_any_element()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ArtistHoverKey {
    scope: String,
    row: usize,
    text: String,
}

static ARTIST_HOVER: OnceLock<Mutex<HashMap<ArtistHoverKey, usize>>> = OnceLock::new();
static TRACK_PROVIDER_MODE: OnceLock<Mutex<crate::motion::ResponsiveModeMotion>> = OnceLock::new();

fn provider_mode_visual(
    target_compact: bool,
    reduced_motion: bool,
) -> crate::motion::ResponsiveModeVisual {
    let motion = TRACK_PROVIDER_MODE
        .get_or_init(|| Mutex::new(crate::motion::ResponsiveModeMotion::default()));
    motion
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .prepare(target_compact, Instant::now(), reduced_motion)
}

fn artist_hover_registry() -> &'static Mutex<HashMap<ArtistHoverKey, usize>> {
    ARTIST_HOVER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn artist_hover(key: &ArtistHoverKey) -> Option<usize> {
    artist_hover_registry()
        .lock()
        .ok()
        .and_then(|hover| hover.get(key).copied())
}

fn set_artist_hover(key: &ArtistHoverKey, route_index: Option<usize>) {
    if let Ok(mut hover) = artist_hover_registry().lock() {
        if let Some(route_index) = route_index {
            hover.clear();
            hover.insert(key.clone(), route_index);
        } else {
            hover.remove(key);
        }
    }
}

fn artist_route_at(index: Option<usize>, ranges: &[Range<usize>]) -> Option<usize> {
    index.and_then(|index| ranges.iter().position(|range| range.contains(&index)))
}

fn artist_text_and_ranges(routes: &[MenuRoute]) -> (String, Vec<Range<usize>>) {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(routes.len());
    for (index, route) in routes.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        let start = text.len();
        text.push_str(&route.title);
        ranges.push(start..text.len());
    }
    (text, ranges)
}

fn track_artist_text_and_ranges(
    artist: &str,
    routes: &[MenuRoute],
    provider: Provider,
) -> (String, Vec<Range<usize>>) {
    if provider == Provider::SoundCloud && routes.len() == 1 {
        let text = artist.to_owned();
        let end = text.len();
        return (text, vec![0..end]);
    }
    artist_text_and_ranges(routes)
}

fn explicit_badge() -> Div {
    div()
        .relative()
        .top(px(2.))
        .w(px(14.))
        .h(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(rgba(0xef444466))
        .bg(rgba(0xef444426))
        .text_size(px(8.))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(rgb(DANGER))
        .child("E")
}

pub(crate) fn playlist_header_action_button(
    id: SharedString,
    icon: LocalIcon,
    color: u32,
    hover_color: u32,
    disabled: bool,
    tooltip: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover_group = id.clone();
    let button = div()
        .id(id)
        .group(hover_group.clone())
        .focusable()
        .tab_stop(!disabled)
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor(row_action_cursor(disabled))
        .when(!disabled, |this| this.hover(|style| style.bg(rgb(BORDER))))
        .when(disabled, |this| this.opacity(0.4))
        .app_tooltip(tooltip)
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            local_icon(icon, color)
                .size(px(14.5))
                .group_hover(hover_group, move |style| style.text_color(rgb(hover_color))),
        );
    if disabled {
        button.into_any_element()
    } else {
        button.on_click(handler).into_any_element()
    }
}

pub(crate) fn playlist_header_delete_button(
    id: SharedString,
    tooltip: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover_group = id.clone();
    div()
        .id(id)
        .group(hover_group.clone())
        .focusable()
        .tab_stop(true)
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor_pointer()
        .app_tooltip(tooltip)
        .hover(|style| style.bg(rgba(0xef44441f)).border_color(rgba(0xef444452)))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            local_icon(LocalIcon::TrashCan, MUTED)
                .size(px(14.5))
                .group_hover(hover_group, |style| style.text_color(rgb(0xf87171))),
        )
        .on_click(handler)
        .into_any_element()
}

pub(crate) fn playlist_header_more_button(id: SharedString) -> gpui::Stateful<Div> {
    let hover_group = id.clone();
    div()
        .id(id)
        .group(hover_group.clone())
        .role(gpui::Role::Button)
        .aria_label("More options")
        .size(px(34.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .app_tooltip(row_action_tooltip(LocalIcon::EllipsisVertical))
        .hover(|style| style.bg(rgb(BORDER)))
        .child(
            div()
                .relative()
                .w(px(4.))
                .h(px(16.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(hover_group.clone(), |style| style.invisible())
                        .child(
                            local_icon(LocalIcon::EllipsisVertical, MUTED)
                                .w(px(4.))
                                .h(px(16.)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .child(
                            local_icon(LocalIcon::EllipsisVertical, FOREGROUND)
                                .w(px(4.))
                                .h(px(16.)),
                        ),
                ),
        )
        .on_click(|_, _, cx| cx.stop_propagation())
}

pub(crate) const DANGER_REMOVE_HIT_TARGET_PX: f32 = 26.;
pub(crate) const DANGER_REMOVE_ICON_SIZE_PX: f32 = 10.;

pub(crate) fn danger_remove_button(
    id: SharedString,
    tooltip: &'static str,
    parent_hover_group: Option<SharedString>,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover_group = id.clone();
    div()
        .id(id)
        .group(hover_group.clone())
        .focusable()
        .tab_stop(true)
        .size(px(DANGER_REMOVE_HIT_TARGET_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .rounded(px(5.))
        .border_1()
        .border_color(rgba(0x00000000))
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .app_tooltip(tooltip)
        .text_size(px(11.))
        .text_color(rgb(0x71717a))
        .opacity(0.65)
        .focus_visible(|style| style.opacity(1.).border_color(rgb(PRIMARY)))
        .when_some(parent_hover_group, |this, parent_hover_group| {
            this.group_hover(parent_hover_group, |style| style.opacity(1.))
        })
        .child(
            local_icon(LocalIcon::X, 0x71717a)
                .size(px(DANGER_REMOVE_ICON_SIZE_PX))
                .group_hover(hover_group, |style| style.text_color(rgb(0xf87171))),
        )
        .on_click(handler)
        .into_any_element()
}

pub(crate) fn danger_row_action_button(
    id: SharedString,
    icon: LocalIcon,
    tooltip: &'static str,
    disabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover_group = id.clone();
    let hover_background = div()
        .absolute()
        .left(px(-2.))
        .top(px(-2.))
        .size(px(29.))
        .rounded(px(7.))
        .invisible()
        .group_hover(hover_group.clone(), |style| {
            style.visible().bg(rgba(DANGER_SECONDARY_HOVER_BACKGROUND))
        });
    let icon_view = if disabled {
        div()
            .size(px(13.))
            .child(local_icon(icon, MUTED).size_full())
            .into_any_element()
    } else {
        div()
            .relative()
            .size(px(13.))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .group_hover(hover_group.clone(), |style| style.invisible())
                    .child(local_icon(icon, MUTED).size_full()),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .invisible()
                    .group_hover(hover_group.clone(), |style| style.visible())
                    .child(local_icon(icon, DANGER_SECONDARY_HOVER_TEXT).size_full()),
            )
            .into_any_element()
    };
    let button = div()
        .id(id)
        .group(hover_group)
        .focusable()
        .tab_stop(!disabled)
        .relative()
        .size(px(TRACK_ACTION_SIZE_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor(row_action_cursor(disabled))
        .role(gpui::Role::Button)
        .aria_label(tooltip)
        .app_tooltip(tooltip)
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .when(disabled, |this| this.opacity(0.4))
        .when(!disabled, |this| this.child(hover_background))
        .child(icon_view);
    if disabled {
        button.into_any_element()
    } else {
        button.on_click(handler).into_any_element()
    }
}

pub(crate) fn row_more_button(id: SharedString) -> gpui::Stateful<Div> {
    let hover_group = id.clone();
    div()
        .id(id)
        .group(hover_group.clone())
        .relative()
        .size(px(TRACK_ACTION_SIZE_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label("More options")
        .app_tooltip(row_action_tooltip(LocalIcon::EllipsisVertical))
        .child(
            div()
                .absolute()
                .left(px(-2.))
                .top(px(-2.))
                .size(px(29.))
                .rounded(px(7.))
                .invisible()
                .group_hover(hover_group.clone(), |style| {
                    style.visible().bg(rgba(0x71717a2e))
                }),
        )
        .child(
            div()
                .relative()
                .w(px(4.))
                .h(px(16.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(hover_group.clone(), |style| style.invisible())
                        .child(
                            local_icon(LocalIcon::EllipsisVertical, MUTED)
                                .w(px(4.))
                                .h(px(16.)),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .child(
                            local_icon(LocalIcon::EllipsisVertical, FOREGROUND)
                                .w(px(4.))
                                .h(px(16.)),
                        ),
                ),
        )
        .on_click(|_, _, cx| cx.stop_propagation())
}

pub(crate) fn row_download_button(id: SharedString) -> gpui::Stateful<Div> {
    let hover_group = id.clone();
    div()
        .id(id)
        .group(hover_group.clone())
        .relative()
        .size(px(TRACK_ACTION_SIZE_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label("Download")
        .app_tooltip(row_action_tooltip(LocalIcon::Download))
        .child(
            div()
                .absolute()
                .left(px(-2.))
                .top(px(-2.))
                .size(px(29.))
                .rounded(px(7.))
                .invisible()
                .group_hover(hover_group.clone(), |style| {
                    style.visible().bg(rgba(0x71717a2e))
                }),
        )
        .child(
            div()
                .relative()
                .size(px(13.))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(hover_group.clone(), |style| style.invisible())
                        .child(local_icon(LocalIcon::Download, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(hover_group, |style| style.visible())
                        .child(local_icon(LocalIcon::Download, FOREGROUND).size_full()),
                ),
        )
        .on_click(|_, _, cx| cx.stop_propagation())
}

fn row_action_cursor(disabled: bool) -> CursorStyle {
    if disabled {
        CursorStyle::Arrow
    } else {
        CursorStyle::PointingHand
    }
}

pub(crate) const PANEL_CLOSE_ICON_SIZE_PX: f32 = 10.;

pub(crate) fn ghost_close_button(
    id: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    ghost_close_button_with_icon_size(id, PANEL_CLOSE_ICON_SIZE_PX, handler)
}

pub(crate) fn ghost_close_button_with_icon_size(
    id: &'static str,
    icon_size: f32,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .group(id)
        .focusable()
        .tab_stop(true)
        .role(gpui::Role::Button)
        .aria_label("Close")
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor_pointer()
        .app_tooltip("Close")
        .hover(|style| style.bg(rgb(BORDER)).border_color(rgb(BORDER)))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            div()
                .relative()
                .size(px(icon_size))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(id, |style| style.invisible())
                        .child(local_icon(LocalIcon::X, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(id, |style| style.visible())
                        .child(local_icon(LocalIcon::X, FOREGROUND).size_full()),
                ),
        )
        .on_click(handler)
        .into_any_element()
}

fn row_action_tooltip(icon: LocalIcon) -> &'static str {
    match icon {
        LocalIcon::Download => "Download",
        LocalIcon::TrashCan => "Remove",
        LocalIcon::X => "Remove",
        LocalIcon::Pen => "Edit",
        LocalIcon::EllipsisVertical => "More options",
        LocalIcon::Heart => "Favorite",
        LocalIcon::ChevronUp => "Move up",
        LocalIcon::ChevronDown => "Move down",
        _ => "Action",
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CardKind {
    Album,
    Artist,
    Flow,
    Playlist,
    Other,
}

fn card_artwork(
    artwork: CardArtworkSource,
    kind: CardKind,
    provider: Option<Provider>,
    badge: Option<&str>,
    id: &str,
) -> AnyElement {
    let icon = match kind {
        CardKind::Artist => LocalIcon::User,
        CardKind::Playlist => LocalIcon::ListUl,
        _ => LocalIcon::CompactDisc,
    };
    let missing = match &artwork {
        CardArtworkSource::Remote(artwork) => artwork.is_empty(),
        CardArtworkSource::Local(path) => path.is_none(),
    };
    div()
        .relative()
        .w_full()
        .aspect_square()
        .overflow_hidden()
        .rounded(px(8.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        .when(missing, |this| {
            this.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(local_icon(icon, 0x52525b).size(px(30.))),
            )
        })
        .when(!missing, |this| {
            let image = match artwork {
                CardArtworkSource::Remote(artwork) => img(artwork),
                CardArtworkSource::Local(Some(path)) => img(path),
                CardArtworkSource::Local(None) => unreachable!(),
            };
            this.child(
                image
                    .id(format!("collection-artwork-{id}"))
                    .size_full()
                    .rounded(px(8.))
                    .object_fit(ObjectFit::Cover),
            )
        })
        .when_some(provider, |this, provider| {
            this.child(
                div()
                    .absolute()
                    .top(px(7.))
                    .left(px(7.))
                    .w(px(21.))
                    .h(px(21.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(0xffffff1f))
                    .bg(rgba(0x09090bc7))
                    .child(provider_logo(provider).size(px(9.))),
            )
        })
        .when_some(badge.filter(|badge| !badge.is_empty()), |this, badge| {
            this.child(
                div()
                    .absolute()
                    .bottom(px(7.))
                    .right(px(7.))
                    .h(px(19.))
                    .px(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(rgba(0xffffff24))
                    .bg(rgba(0x09090bd1))
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xe4e4e7))
                    .when(matches!(kind, CardKind::Playlist), |this| {
                        this.child(
                            div()
                                .relative()
                                .top(px(1.))
                                .child(local_icon(LocalIcon::Music, 0xe4e4e7).size(px(8.))),
                        )
                    })
                    .child(badge.to_uppercase()),
            )
        })
        .into_any_element()
}

fn track_artwork_url(artwork: &str) -> String {
    let Ok(mut url) = Url::parse(artwork) else {
        return artwork.to_owned();
    };
    if url.scheme() != "https" {
        return artwork.to_owned();
    }
    let Some(host) = url.host_str() else {
        return artwork.to_owned();
    };
    let path = url.path();
    let Some((directory, filename)) = path.rsplit_once('/') else {
        return artwork.to_owned();
    };
    let resized = if host.eq_ignore_ascii_case("e-cdns-images.dzcdn.net") {
        filename
            .strip_prefix("500x500")
            .filter(|suffix| suffix.starts_with('.') || suffix.starts_with('-'))
            .map(|suffix| format!("120x120{suffix}"))
    } else if host.eq_ignore_ascii_case("sndcdn.com")
        || host.to_ascii_lowercase().ends_with(".sndcdn.com")
    {
        soundcloud_thumbnail_filename(filename)
    } else {
        None
    };
    let Some(resized) = resized else {
        return artwork.to_owned();
    };
    url.set_path(&format!("{directory}/{resized}"));
    url.to_string()
}

fn soundcloud_thumbnail_filename(filename: &str) -> Option<String> {
    if let Some((prefix, suffix)) = filename.rsplit_once("-t500x500")
        && !prefix.is_empty()
        && suffix.starts_with('.')
    {
        return Some(format!("{prefix}-t120x120{suffix}"));
    }
    if let Some((prefix, suffix)) = filename.rsplit_once("-large")
        && !prefix.is_empty()
        && suffix.starts_with('.')
    {
        return Some(format!("{prefix}-t120x120{suffix}"));
    }
    None
}

fn artwork_view(artwork: &str) -> AnyElement {
    let artwork = track_artwork_url(artwork);
    div()
        .w(px(TRACK_ARTWORK_SIZE_PX))
        .h(px(TRACK_ARTWORK_SIZE_PX))
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .bg(rgb(SURFACE_RAISED))
        .when(!artwork.is_empty(), |this| {
            this.child(
                img(artwork.clone())
                    .size_full()
                    .rounded(px(6.))
                    .object_fit(ObjectFit::Cover),
            )
        })
        .into_any_element()
}

fn provider_badge(
    provider: Provider,
    responsive: crate::motion::ResponsiveModeVisual,
    motion_key: String,
) -> AnyElement {
    let color = match provider {
        Provider::Deezer => DEEZER,
        Provider::SoundCloud => SOUNDCLOUD,
    };
    let (from_label_width, target_label_width) =
        responsive.endpoints(track_provider_label_width(provider), 0.);
    let (from_opacity, target_opacity) = responsive.endpoints(1., 0.);
    let label = div()
        .flex_none()
        .overflow_hidden()
        .whitespace_nowrap()
        .w(px(target_label_width))
        .opacity(target_opacity)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(color))
        .child(provider.label())
        .with_animation(
            ElementId::named_usize(
                format!("track-provider-label-{motion_key}"),
                responsive.epoch as usize,
            ),
            crate::motion::content(),
            move |this, delta| {
                this.w(px(crate::motion::lerp(
                    from_label_width,
                    target_label_width,
                    delta,
                )))
                .opacity(crate::motion::lerp(
                    from_opacity,
                    target_opacity,
                    delta,
                ))
            },
        )
        .into_any_element();
    div()
        .h(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .relative()
                .top(px(TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX))
                .id(format!("track-provider-logo-{motion_key}"))
                .app_tooltip(provider.label())
                .child(provider_logo(provider).size(px(11.))),
        )
        .child(label)
        .into_any_element()
}

fn provider_logo(provider: Provider) -> gpui::Svg {
    let (icon, color) = match provider {
        Provider::Deezer => (LocalIcon::Deezer, DEEZER),
        Provider::SoundCloud => (LocalIcon::SoundCloud, SOUNDCLOUD),
    };
    local_icon(icon, color)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Instant};

    use crate::{assets::LocalIcon, motion::ResponsiveModeMotion, search::Provider};

    use super::{
        ArtistHoverKey, CAROUSEL_ARROW_TRACK_GAP, CAROUSEL_ARROW_WIDTH,
        CAROUSEL_CARD_ROW_BOTTOM_GAP, CAROUSEL_CONTENT_BOTTOM_PADDING, CAROUSEL_CONTROL_HEIGHT,
        CAROUSEL_THUMB_HEIGHT, CAROUSEL_TRACK_INSET, CATEGORY_LABEL_FIT_BUFFER,
        CATEGORY_TAB_ICON_WIDTH, COLLECTION_PRIVACY_ICON_GAP, COLLECTION_PRIVACY_ICON_SIZE,
        COLLECTION_PRIVACY_OPTICAL_OFFSET_PX, CarouselDirection, MAIN_CONTENT_INSET, MOBILE_MAX,
        MenuRoute, MenuRouteKind, NARROW_MAIN_CONTENT_INSET, TOOLBAR_STACK_MAX,
        TRACK_PROVIDER_COMPACT_COLUMN_WIDTH, TRACK_PROVIDER_DURATION_GAP_PX,
        TRACK_PROVIDER_FULL_COLUMN_WIDTH, TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX,
        TRACK_PROVIDER_LABEL_MIN_WIDTH, TRACK_TITLE_ARTIST_GAP_PX, artist_hover,
        artist_hover_registry, artist_route_at, artist_text_and_ranges,
        card_carousel_display_count, card_carousel_has_overflow, card_grid_skeleton_count,
        card_row_has_more, card_row_metrics, carousel_arrow_target_from_pending,
        carousel_drag_offset, carousel_snap_offset, carousel_target_offset, category_tab_tooltip,
        category_tabs_icon_only, category_tabs_label_width, compact_desktop_viewport,
        compact_track_provider, effective_content_width, fitted_columns, horizontal_scroll_id,
        main_content_inset, narrow_content_viewport, playlist_privacy_icon, row_action_cursor,
        set_artist_hover, shell_metrics, shell_metrics_for_viewport,
        should_consume_horizontal_scroll, show_search_result_count, track_artist_text_and_ranges,
        track_artwork_url, track_provider_column_endpoints, visible_preview_cards,
    };

    #[test]
    fn consumes_horizontal_only_wheel() {
        assert!(should_consume_horizontal_scroll(10., 0.));
    }

    #[test]
    fn collection_privacy_matches_picker_lock_and_earth_icons() {
        assert_eq!(playlist_privacy_icon(true), LocalIcon::Lock);
        assert_eq!(playlist_privacy_icon(false), LocalIcon::EarthAmericas);
        assert_eq!(COLLECTION_PRIVACY_ICON_SIZE, 12.);
        assert_eq!(COLLECTION_PRIVACY_ICON_GAP, 6.);
        assert_eq!(COLLECTION_PRIVACY_OPTICAL_OFFSET_PX, 1.);
        let source = include_str!("mod.rs");
        let card = source
            .split("pub(crate) fn collection_card(")
            .nth(1)
            .unwrap()
            .split("pub(crate) struct TrackRowDisplay")
            .next()
            .unwrap();
        assert!(card.contains("playlist_privacy_icon(is_private)"));
        assert!(card.contains("\"Private\""));
        assert!(card.contains("\"Public\""));
        assert!(card.contains(".top(px(COLLECTION_PRIVACY_OPTICAL_OFFSET_PX))"));
        assert!(!card.contains("LocalIcon::Globe"));
        assert!(source.contains("collection-artwork-"));
    }

    #[test]
    fn artist_ranges_follow_combined_text_byte_offsets() {
        let routes = vec![
            MenuRoute {
                kind: MenuRouteKind::Artist,
                id: "1".into(),
                title: "A".into(),
            },
            MenuRoute {
                kind: MenuRouteKind::Artist,
                id: "2".into(),
                title: "Björk".into(),
            },
        ];
        let (text, ranges) = artist_text_and_ranges(&routes);

        assert_eq!(text, "A, Björk");
        assert_eq!(&text[ranges[0].clone()], "A");
        assert_eq!(&text[ranges[1].clone()], "Björk");
    }

    #[test]
    fn soundcloud_single_artist_route_keeps_credited_display_text() {
        let routes = vec![MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "55".into(),
            title: "TRVCY".into(),
        }];

        let visual_provider_badge: Option<Provider> = None;
        let (text, ranges) =
            track_artist_text_and_ranges("Skrillex", &routes, Provider::SoundCloud);

        assert!(visual_provider_badge.is_none());
        assert_eq!(text, "Skrillex");
        assert_eq!(&text[ranges[0].clone()], "Skrillex");
        assert_eq!(routes[0].id, "55");
        assert_eq!(routes[0].title, "TRVCY");
    }

    #[test]
    fn deezer_single_route_uses_the_route_title_instead_of_the_full_credit() {
        let routes = vec![MenuRoute {
            kind: MenuRouteKind::Artist,
            id: "1".into(),
            title: "Primary".into(),
        }];

        let (text, ranges) =
            track_artist_text_and_ranges("Primary, Guest", &routes, Provider::Deezer);

        assert_eq!(text, "Primary");
        assert_eq!(&text[ranges[0].clone()], "Primary");
    }

    #[test]
    fn artist_hover_selects_only_the_range_under_the_pointer() {
        let ranges = vec![0..5, 7..12];

        assert_eq!(artist_route_at(Some(2), &ranges), Some(0));
        assert_eq!(artist_route_at(Some(6), &ranges), None);
        assert_eq!(artist_route_at(Some(8), &ranges), Some(1));
        assert_eq!(artist_route_at(None, &ranges), None);
    }

    #[test]
    fn artist_hover_registry_keeps_only_the_active_row() {
        let first = ArtistHoverKey {
            scope: "virtualized-list".into(),
            row: 1,
            text: "First Artist".into(),
        };
        let second = ArtistHoverKey {
            scope: "virtualized-list".into(),
            row: 2_499,
            text: "Last Artist".into(),
        };
        artist_hover_registry().lock().unwrap().clear();

        set_artist_hover(&first, Some(0));
        set_artist_hover(&second, Some(0));

        let hover = artist_hover_registry().lock().unwrap();
        assert_eq!(hover.len(), 1);
        assert!(!hover.contains_key(&first));
        drop(hover);
        assert_eq!(artist_hover(&second), Some(0));

        set_artist_hover(&second, None);
        assert_eq!(artist_hover(&second), None);
    }

    #[test]
    fn track_artwork_uses_small_provider_thumbnails() {
        assert_eq!(
            track_artwork_url("https://e-cdns-images.dzcdn.net/images/cover/hash/500x500.jpg"),
            "https://e-cdns-images.dzcdn.net/images/cover/hash/120x120.jpg"
        );
        assert_eq!(
            track_artwork_url("https://i1.sndcdn.com/track-t500x500.jpg"),
            "https://i1.sndcdn.com/track-t120x120.jpg"
        );
        assert_eq!(
            track_artwork_url("https://i1.sndcdn.com/track-large.jpg"),
            "https://i1.sndcdn.com/track-t120x120.jpg"
        );
    }

    #[test]
    fn track_artwork_leaves_unknown_variants_unchanged() {
        let unknown = "https://example.com/track-large.jpg";
        assert_eq!(track_artwork_url(unknown), unknown);
        assert_eq!(
            track_artwork_url("https://e-cdns-images.dzcdn.net/images/cover/hash/250x250.jpg"),
            "https://e-cdns-images.dzcdn.net/images/cover/hash/250x250.jpg"
        );
        assert_eq!(track_artwork_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn card_row_fade_disappears_at_the_right_endpoint_with_tolerance() {
        assert!(card_row_has_more(0., 500.));
        assert!(card_row_has_more(-497., 500.));
        assert!(!card_row_has_more(-498., 500.));
        assert!(!card_row_has_more(-500., 500.));
    }

    #[test]
    fn carousel_controls_and_spacing_only_exist_for_overflowing_rows() {
        assert!(!card_carousel_has_overflow(0, 150., 12., 474., 0.));
        assert!(!card_carousel_has_overflow(1, 150., 12., 474., 0.));
        assert!(!card_carousel_has_overflow(3, 150., 12., 474., 0.));
        assert!(card_carousel_has_overflow(4, 150., 12., 474., 0.));
        assert!(card_carousel_has_overflow(3, 150., 12., 474., 2.));
        assert!(card_carousel_has_overflow(64, 150., 12., 7680., 0.));
    }

    #[test]
    fn carousel_card_window_is_fixed_across_viewport_sizes() {
        assert_eq!(card_carousel_display_count(4, 12), 4);
        assert_eq!(card_carousel_display_count(40, 12), 40);
        assert_eq!(card_carousel_display_count(100, 12), 64);
    }

    #[test]
    fn consumes_horizontal_dominant_wheel() {
        assert!(should_consume_horizontal_scroll(-10., 4.));
    }

    #[test]
    fn preserves_vertical_dominant_wheel() {
        assert!(!should_consume_horizontal_scroll(4., -10.));
    }

    #[test]
    fn preserves_zero_wheel() {
        assert!(!should_consume_horizontal_scroll(0., 0.));
    }

    #[test]
    fn disabled_row_actions_use_the_default_cursor() {
        assert!(matches!(row_action_cursor(true), gpui::CursorStyle::Arrow));
        assert!(matches!(
            row_action_cursor(false),
            gpui::CursorStyle::PointingHand
        ));
    }

    #[test]
    fn danger_row_action_uses_shared_palette_and_stays_inert_when_disabled() {
        let source = include_str!("mod.rs");
        assert!(source.contains("pub(crate) fn danger_row_action_button"));
        assert!(source.contains("DANGER_SECONDARY_HOVER_BACKGROUND"));
        assert!(source.contains("DANGER_SECONDARY_HOVER_TEXT"));
        assert!(source.contains(".role(gpui::Role::Button)"));
        assert!(source.contains("if disabled {\n        button.into_any_element()"));
    }

    #[test]
    fn vertical_wheel_is_not_considered_horizontal() {
        assert!(!should_consume_horizontal_scroll(0., 12.));
        assert!(!should_consume_horizontal_scroll(4., 10.));
    }

    #[test]
    fn carousel_targets_one_card_pitch_and_clamps_to_edges() {
        assert_eq!(
            carousel_target_offset(0., 500., 162., CarouselDirection::Next),
            -162.
        );
        assert_eq!(
            carousel_target_offset(-162., 500., 162., CarouselDirection::Previous),
            0.
        );
        assert_eq!(
            carousel_target_offset(-40., 500., 162., CarouselDirection::Next),
            -162.
        );
        assert_eq!(
            carousel_target_offset(-200., 500., 162., CarouselDirection::Previous),
            -162.
        );
        assert_eq!(
            carousel_target_offset(-162., 500., 162., CarouselDirection::Next),
            -324.
        );
        assert_eq!(
            carousel_target_offset(-324., 500., 162., CarouselDirection::Next),
            -500.
        );
        assert_eq!(
            carousel_target_offset(-500., 500., 162., CarouselDirection::Previous),
            -324.
        );
        assert_eq!(
            carousel_target_offset(-450., 500., 162., CarouselDirection::Next),
            -500.
        );
        assert_eq!(
            carousel_target_offset(-40., 500., 162., CarouselDirection::Previous),
            0.
        );
    }

    #[test]
    fn carousel_target_normalizes_invalid_bounds_and_pitch() {
        assert_eq!(
            carousel_target_offset(80., 20., -5., CarouselDirection::Next),
            0.
        );
        assert_eq!(
            carousel_target_offset(-80., -20., 5., CarouselDirection::Next),
            -20.
        );
    }

    #[test]
    fn carousel_arrow_retargets_from_the_live_displayed_offset() {
        assert_eq!(
            carousel_arrow_target_from_pending(-75., None, 500., 162., CarouselDirection::Next),
            -162.
        );
        assert_eq!(
            carousel_arrow_target_from_pending(
                -75.,
                Some(-162.),
                500.,
                162.,
                CarouselDirection::Next,
            ),
            -324.
        );
        assert_eq!(
            carousel_arrow_target_from_pending(
                -200.,
                None,
                500.,
                162.,
                CarouselDirection::Previous,
            ),
            -162.
        );
    }

    #[test]
    fn three_rapid_next_and_previous_clicks_accumulate_card_destinations() {
        let next_pending = Cell::new(None);
        for _ in 0..3 {
            let target = carousel_arrow_target_from_pending(
                0.,
                next_pending.get(),
                500.,
                162.,
                CarouselDirection::Next,
            );
            next_pending.set(Some(target));
        }
        assert_eq!(next_pending.get(), Some(-500.));

        let previous_pending = Cell::new(None);
        for _ in 0..3 {
            let target = carousel_arrow_target_from_pending(
                -500.,
                previous_pending.get(),
                500.,
                162.,
                CarouselDirection::Previous,
            );
            previous_pending.set(Some(target));
        }
        assert_eq!(previous_pending.get(), Some(0.));
    }

    #[test]
    fn pending_arrow_targets_clamp_at_the_scroll_endpoints() {
        assert_eq!(
            carousel_arrow_target_from_pending(
                0.,
                Some(-500.),
                500.,
                162.,
                CarouselDirection::Next,
            ),
            -500.
        );
        assert_eq!(
            carousel_arrow_target_from_pending(
                -500.,
                Some(0.),
                500.,
                162.,
                CarouselDirection::Previous,
            ),
            0.
        );
    }

    #[test]
    fn carousel_snap_prefers_nearest_regular_or_final_boundary() {
        assert_eq!(carousel_snap_offset(-450., 500., 162.), -486.);
        assert_eq!(carousel_snap_offset(-495., 500., 162.), -500.);
        assert_eq!(carousel_snap_offset(-200., 500., 162.), -162.);
    }

    #[test]
    fn carousel_drag_offset_clamps_to_scroll_range() {
        assert_eq!(carousel_drag_offset(-450., -100., 500.), -500.);
        assert_eq!(carousel_drag_offset(-40., -100., 500.), -140.);
        assert_eq!(carousel_drag_offset(-40., 100., 500.), 0.);
    }

    #[test]
    fn carousel_controls_match_native_track_metrics() {
        assert_eq!(CAROUSEL_ARROW_WIDTH, 10.);
        assert_eq!(CAROUSEL_ARROW_TRACK_GAP, 2.);
        assert_eq!(CAROUSEL_TRACK_INSET, 12.);
        assert_eq!(CAROUSEL_CONTROL_HEIGHT, 10.);
        assert_eq!(CAROUSEL_THUMB_HEIGHT, 6.);
        assert_eq!(crate::theme::SCROLLBAR_THUMB, 0x3f3f46);
    }

    #[test]
    fn carousel_track_inset_keeps_thumb_endpoints_symmetric() {
        let width = 100.;
        let left = CAROUSEL_TRACK_INSET;
        let right = width - CAROUSEL_TRACK_INSET;

        assert_eq!(left, CAROUSEL_ARROW_WIDTH + CAROUSEL_ARROW_TRACK_GAP);
        assert_eq!(
            width - right,
            CAROUSEL_ARROW_WIDTH + CAROUSEL_ARROW_TRACK_GAP
        );
        assert_eq!(left, width - right);
    }

    #[test]
    fn carousel_content_padding_preserves_the_native_card_row_gap() {
        assert_eq!(CAROUSEL_CARD_ROW_BOTTOM_GAP, 8.);
        assert_eq!(CAROUSEL_CONTENT_BOTTOM_PADDING, 18.);
        assert_eq!(
            CAROUSEL_CONTENT_BOTTOM_PADDING,
            CAROUSEL_CARD_ROW_BOTTOM_GAP + CAROUSEL_CONTROL_HEIGHT
        );
    }

    #[test]
    fn horizontal_scroll_ids_are_stable_and_section_qualified() {
        assert_eq!(
            horizontal_scroll_id("search-card-preview-scroll", "Albums", 0),
            "search-card-preview-scroll-Albums-0"
        );
        assert_ne!(
            horizontal_scroll_id("search-card-preview-scroll", "Albums", 0),
            horizontal_scroll_id("search-card-preview-scroll", "Artists", 1)
        );
    }

    #[test]
    fn provider_logo_uses_app_tooltip_with_provider_label() {
        let source = include_str!("mod.rs");
        let badge = source
            .split("fn provider_badge(")
            .nth(1)
            .and_then(|source| source.split("fn provider_logo(").next())
            .expect("provider badge function");
        assert!(badge.contains("provider_logo(provider)"));
        assert!(badge.contains(".app_tooltip(provider.label())"));
        assert_eq!(Provider::Deezer.label(), "Deezer");
        assert_eq!(Provider::SoundCloud.label(), "SoundCloud");
    }

    #[test]
    fn track_provider_labels_follow_effective_content_width() {
        assert!(narrow_content_viewport(768.));
        assert!(!compact_desktop_viewport(768.));
        assert!(!compact_track_provider(768.));

        assert!(!narrow_content_viewport(769.));
        assert!(compact_desktop_viewport(769.));
        assert!(!compact_track_provider(769.));

        assert!(!narrow_content_viewport(1200.));
        assert!(compact_desktop_viewport(1200.));
        assert!(!compact_track_provider(1200.));

        assert!(!narrow_content_viewport(1201.));
        assert!(!compact_desktop_viewport(1201.));
        assert!(compact_track_provider(539.));
        assert!(compact_track_provider(TRACK_PROVIDER_LABEL_MIN_WIDTH - 0.1));
        assert!(!compact_track_provider(TRACK_PROVIDER_LABEL_MIN_WIDTH));
    }

    #[test]
    fn track_provider_motion_preserves_full_and_narrow_endpoints() {
        let mut motion = ResponsiveModeMotion::default();
        let now = Instant::now();
        let full = motion.prepare(false, now, false);
        assert_eq!(
            track_provider_column_endpoints(full),
            (
                TRACK_PROVIDER_FULL_COLUMN_WIDTH,
                TRACK_PROVIDER_FULL_COLUMN_WIDTH
            )
        );

        let narrow = motion.prepare(true, now, false);
        assert_eq!(
            track_provider_column_endpoints(narrow),
            (
                TRACK_PROVIDER_FULL_COLUMN_WIDTH,
                TRACK_PROVIDER_COMPACT_COLUMN_WIDTH
            )
        );
        assert_eq!(motion.prepare(true, now, false), narrow);

        let settled_at = now + crate::motion::CONTENT_DURATION;
        let settled = motion.prepare(true, settled_at, false);
        assert_eq!(settled.from, 1.0);
        assert_eq!(settled.target, 1.0);

        let expanded = motion.prepare(false, settled_at, false);
        assert_eq!(
            track_provider_column_endpoints(expanded),
            (
                TRACK_PROVIDER_COMPACT_COLUMN_WIDTH,
                TRACK_PROVIDER_FULL_COLUMN_WIDTH
            )
        );
    }

    #[test]
    fn track_provider_duration_gap_is_named_and_compact() {
        assert_eq!(TRACK_PROVIDER_DURATION_GAP_PX, 2.);
        let source = include_str!("mod.rs");
        let provider_column = source
            .split("let provider_column = div()")
            .nth(1)
            .and_then(|source| source.split("let provider_column = if").next())
            .expect("provider column source");
        assert!(provider_column.contains(".gap(px(TRACK_PROVIDER_DURATION_GAP_PX))"));
    }

    #[test]
    fn track_provider_logo_uses_a_one_pixel_optical_offset() {
        assert_eq!(TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX, 1.);
        let source = include_str!("mod.rs");
        let badge = source
            .split("fn provider_badge(")
            .nth(1)
            .and_then(|source| source.split("fn provider_logo(").next())
            .expect("provider badge source");
        assert!(badge.contains(".top(px(TRACK_PROVIDER_ICON_OPTICAL_OFFSET_PX))"));
    }

    #[test]
    fn track_title_artist_gap_is_shared_with_the_tracklist_stack() {
        assert_eq!(TRACK_TITLE_ARTIST_GAP_PX, 1.);
        let source = include_str!("mod.rs");
        let track_row = source
            .split("pub(crate) fn track_row_with_action(")
            .nth(1)
            .and_then(|source| source.split("pub(crate) fn track_artist_line(").next())
            .expect("track row source");
        assert!(track_row.contains(".gap(px(TRACK_TITLE_ARTIST_GAP_PX))"));
    }

    #[test]
    fn responsive_contract_has_exact_boundary_behavior() {
        for width in [
            459., 460., 461., 559., 560., 561., 639., 640., 641., 767., 768., 769., 899., 900.,
            901., 1199., 1200., 1201., 1439., 1440., 1441.,
        ] {
            let metrics = shell_metrics_for_viewport(width, 620.);
            assert_eq!(metrics.narrow_content, width <= MOBILE_MAX);
            assert_eq!(metrics.compact_desktop, (769. ..=1200.).contains(&width));
            assert_eq!(metrics.compact_player, (769. ..=1440.).contains(&width));
            assert_eq!(metrics.toolbar_stacked, width <= TOOLBAR_STACK_MAX);
        }
    }

    #[test]
    fn card_row_metrics_follow_the_frozen_breakpoint_sizes() {
        assert_eq!(card_row_metrics(false), (150., 12.));
        assert_eq!(card_row_metrics(true), (126., 9.));
    }

    #[test]
    fn preview_visibility_has_one_card_minimum() {
        assert_eq!(visible_preview_cards(0., false), 1);
        assert_eq!(visible_preview_cards(150., false), 1);
    }

    #[test]
    fn preview_visibility_accounts_for_card_gap() {
        assert_eq!(visible_preview_cards(312., false), 2);
        assert_eq!(visible_preview_cards(474., false), 3);
        assert_eq!(visible_preview_cards(261., true), 2);
    }

    #[test]
    fn shell_metrics_use_compact_desktop_only() {
        let narrow = shell_metrics(768.);
        assert_eq!((narrow.sidebar_width, narrow.gutter), (0., 12.));
        assert_eq!(narrow.available_width, 744.);

        let compact_start = shell_metrics(769.);
        assert_eq!(
            (compact_start.sidebar_width, compact_start.gutter),
            (68., 16.)
        );
        assert_eq!(compact_start.available_width, 669.);

        let compact_end = shell_metrics(1200.);
        assert_eq!((compact_end.sidebar_width, compact_end.gutter), (68., 16.));
        assert_eq!(compact_end.available_width, 1100.);

        let desktop = shell_metrics(1201.);
        assert_eq!((desktop.sidebar_width, desktop.gutter), (240., 28.));
        assert_eq!(desktop.available_width, 905.);
    }

    #[test]
    fn shell_metrics_never_returns_negative_content_width() {
        assert_eq!(shell_metrics(100.).available_width, 76.);
    }

    #[test]
    fn effective_content_width_accounts_for_open_right_sidebar() {
        let metrics = shell_metrics(1200.);
        assert_eq!(effective_content_width(&metrics, false), 1076.);
        assert_eq!(effective_content_width(&metrics, true), 716.);

        let narrow = shell_metrics(768.);
        assert_eq!(
            effective_content_width(&narrow, true),
            narrow.available_width
        );
    }

    #[test]
    fn fitted_columns_follow_the_effective_center_width() {
        let metrics = shell_metrics(1200.);
        assert_eq!(fitted_columns(effective_content_width(&metrics, false)), 6);
        assert_eq!(fitted_columns(effective_content_width(&metrics, true)), 4);
    }

    #[test]
    fn card_grid_skeleton_count_grows_with_height_in_complete_rows() {
        let short = card_grid_skeleton_count(4, 740., 500.);
        let tall = card_grid_skeleton_count(4, 740., 1_200.);

        assert_eq!(short % 4, 0);
        assert_eq!(tall % 4, 0);
        assert!(tall > short);
    }

    #[test]
    fn search_and_library_insets_match_the_original_breakpoint() {
        assert_eq!(
            main_content_inset(&shell_metrics(768.)),
            NARROW_MAIN_CONTENT_INSET
        );
        assert_eq!(main_content_inset(&shell_metrics(769.)), MAIN_CONTENT_INSET);
        assert_eq!(
            main_content_inset(&shell_metrics(1201.)),
            MAIN_CONTENT_INSET
        );
    }

    #[test]
    fn category_tab_labels_switch_to_balanced_icons_with_fit_buffer() {
        let labels = ["All", "Tracks", "Albums", "Artists", "Playlists"];
        let label_width = category_tabs_label_width(&labels);
        assert!(!category_tabs_icon_only(
            label_width + CATEGORY_LABEL_FIT_BUFFER,
            &labels
        ));
        assert!(category_tabs_icon_only(label_width + 47., &labels));
        assert_eq!(CATEGORY_TAB_ICON_WIDTH, 34.);
        assert_eq!(super::category_tab_text_width("Albums"), 43.);
    }

    #[test]
    fn category_tab_tooltips_are_only_for_icon_only_tabs() {
        assert_eq!(category_tab_tooltip(true, "Tracks"), Some("Tracks"));
        assert_eq!(category_tab_tooltip(false, "Tracks"), None);
    }

    #[test]
    fn search_result_count_is_hidden_at_or_below_header_boundary() {
        assert!(!show_search_result_count(640.));
        assert!(!show_search_result_count(320.));
        assert!(show_search_result_count(641.));
    }
}
