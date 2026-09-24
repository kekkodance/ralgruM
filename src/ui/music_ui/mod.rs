use gpui::{
    AnimationExt as _, AnyElement, App, Bounds, ClickEvent, CursorStyle, Div, ElementId,
    FontWeight, HighlightStyle, InteractiveText, Pixels, ScrollHandle, ScrollWheelEvent,
    SharedString, Stateful, StyledText, TextAlign, UnderlineStyle, Window, canvas, div, fill,
    point, prelude::*, px, rgb, rgba, size, svg,
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
mod artwork;
mod card_grid_motion;
mod card_scrollbar;
mod carousel_motion;
mod resize_columns;
mod track_skeleton;

pub(crate) use artwork::CardKind;
#[cfg(test)]
use artwork::track_artwork_url;
use artwork::{artwork_view, card_artwork, provider_badge};
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
const CAROUSEL_EDGE_FADE_WIDTH: f32 = 52.;
const CAROUSEL_EDGE_TOP_INSET: f32 = 2.;
/// Fraction of a card pitch an edge darkening keeps as lead room: the
/// darkening starts dissolving while that much scrollable content still
/// remains beyond the edge, instead of clinging to the final pixels.
const CAROUSEL_EDGE_TRIGGER_PITCH_RATIO: f32 = 0.5;
/// Lower bound for the edge trigger, matching the overflow tolerance shared
/// with `card_row_has_more`.
const CAROUSEL_EDGE_MIN_TRIGGER: f32 = 2.;

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
    pub(crate) controls_fade: Rc<Cell<card_scrollbar::CarouselControlsFade>>,
}

impl CardCarouselState {
    pub(crate) fn new() -> Self {
        Self {
            scroll_handle: ScrollHandle::new(),
            drag: Rc::new(Cell::new(None)),
            pending_target: Rc::new(Cell::new(None)),
            snap_epoch: Rc::new(Cell::new(0)),
            controls_fade: Rc::new(Cell::new(card_scrollbar::CarouselControlsFade::default())),
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

/// Scrollable travel, in pixels, over which an edge darkening ramps from
/// transparent to fully saturated. Half a card of lead room gives the
/// darkening comfortable space to dissolve before the row reaches its
/// end, and rows that barely overflow scale the ramp down so short
/// carousels keep the affordance.
pub(crate) fn carousel_edge_trigger(card_pitch: f32, max_offset: f32) -> f32 {
    let extent = max_offset.abs().max(0.);
    let lead = card_pitch.max(0.) * CAROUSEL_EDGE_TRIGGER_PITCH_RATIO;
    (extent * 0.5).min(lead).max(CAROUSEL_EDGE_MIN_TRIGGER)
}

/// One carousel edge darkening's intensity from how far the row can still
/// travel toward that edge: transparent at the very edge, ramping up as
/// more scrollable content remains beyond it, and saturating once a
/// comfortable stretch remains. The mapping is pure, so the darkening
/// tracks the scroll position directly and moves exactly as fast as the
/// row itself, animated or snapped.
pub(crate) fn carousel_edge_intensity(travel: f32, ramp: f32) -> f32 {
    if ramp <= 0. {
        return 0.;
    }
    (travel / ramp).clamp(0., 1.)
}

/// The darkening gradient for one carousel edge: opaque page background at
/// the viewport edge, fading to transparent over the cards.
fn card_row_edge_gradient(background_at_start: bool) -> gpui::Background {
    let (start, end) = if background_at_start {
        (
            gpui::linear_color_stop(rgb(BACKGROUND), 0.),
            gpui::linear_color_stop(gpui::transparent_black(), 1.),
        )
    } else {
        (
            gpui::linear_color_stop(gpui::transparent_black(), 0.),
            gpui::linear_color_stop(rgb(BACKGROUND), 1.),
        )
    };
    gpui::linear_gradient(90., start, end)
}

/// Paint the card-row edge darkenings from the live scroll bounds. Keeping
/// this in the shared carousel means search, library, and detail card
/// sections all use the same boundary behavior. Both edges share one ramp
/// so they mirror each other: each darkening's strength grows with the
/// scrollable content beyond its edge and saturates once a comfortable
/// stretch remains, dissolving only as the row approaches that edge.
fn card_row_fade(state: CardCarouselState, card_pitch: f32) -> impl IntoElement {
    canvas(
        move |_, _, _| (state, card_pitch),
        |bounds, (state, card_pitch), window, _cx| {
            paint_card_row_edge_fades(bounds, &state, card_pitch, window);
        },
    )
    .absolute()
    .inset_0()
}

fn paint_card_row_edge_fades(
    bounds: Bounds<Pixels>,
    state: &CardCarouselState,
    card_pitch: f32,
    window: &mut Window,
) {
    let offset = f32::from(state.scroll_handle.offset().x);
    let extent = f32::from(state.scroll_handle.max_offset().x).abs();
    let scrolled = (-offset).clamp(0., extent);
    let ramp = carousel_edge_trigger(card_pitch, extent);
    let height =
        f32::from(bounds.size.height) - CAROUSEL_EDGE_TOP_INSET - CAROUSEL_CARD_ROW_BOTTOM_GAP;
    let width = f32::from(bounds.size.width);
    if height <= 0. || width < CAROUSEL_EDGE_FADE_WIDTH {
        return;
    }
    let fade_size = size(px(CAROUSEL_EDGE_FADE_WIDTH), px(height));
    let top = bounds.origin.y + px(CAROUSEL_EDGE_TOP_INSET);
    let left_intensity = carousel_edge_intensity(scrolled, ramp);
    if left_intensity > 0. {
        window.paint_quad(fill(
            Bounds {
                origin: point(bounds.origin.x, top),
                size: fade_size,
            },
            card_row_edge_gradient(true).opacity(left_intensity),
        ));
    }
    let right_intensity = carousel_edge_intensity(extent - scrolled, ramp);
    if right_intensity > 0. {
        window.paint_quad(fill(
            Bounds {
                origin: point(bounds.origin.x + px(width - CAROUSEL_EDGE_FADE_WIDTH), top),
                size: fade_size,
            },
            card_row_edge_gradient(false).opacity(right_intensity),
        ));
    }
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

/// Render a card row with a horizontal scrollbar and native-like arrow
/// controls at each end of the track. The scrollbar and both arrows are
/// one unit: they fade in together when the row starts overflowing, fade
/// out together when it stops, and while it keeps overflowing they show
/// on scroll activity and idle-fade out like the app's vertical
/// scrollbars. The controls anchor to the card row itself, so fading
/// only ever changes their opacity, never their position. The caller
/// owns the handle so it survives view updates and can be shared by the
/// scrollbar and arrows.
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
    // The caller's overflow estimate also decides where the control
    // strip's reserve lives: inside the scroll viewport when the row is
    // expected to overflow, or in the space below the carousel when it
    // is not, which keeps stacked sections a stable height. The scrollbar
    // and both arrows anchor to the card row either way, so the reserve
    // moving never moves the fading controls.
    let strip_reserved = controls_available;
    // Both arrows sample the same shared fade the scrollbar drives, so the
    // whole unit appears and disappears together.
    let controls_opacity = state
        .controls_fade
        .get()
        .render_opacity(Instant::now(), controls_available);
    let previous = carousel_arrow(
        format!("{id}-previous"),
        CarouselDirection::Previous,
        state.clone(),
        pitch,
        controls_opacity,
        strip_reserved,
    );
    let next = carousel_arrow(
        format!("{id}-next"),
        CarouselDirection::Next,
        state.clone(),
        pitch,
        controls_opacity,
        strip_reserved,
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
                .when(strip_reserved, |this| {
                    this.pb(px(CAROUSEL_CONTENT_BOTTOM_PADDING))
                })
                .child(content),
        )
        .child(card_row_fade(state.clone(), pitch))
        .child(card_scrollbar::card_scrollbar(
            state.clone(),
            pitch,
            strip_reserved,
        ))
        .child(previous)
        .child(next)
        .into_any_element()
}

fn carousel_arrow(
    id: String,
    direction: CarouselDirection,
    state: CardCarouselState,
    card_pitch: f32,
    opacity: f32,
    strip_reserved: bool,
) -> AnyElement {
    let base = div()
        .absolute()
        // The arrows ride the same anchored strip as the scrollbar: with
        // the reserve inside the viewport they pin to the canvas bottom,
        // and without it they drop below the carousel into the space the
        // caller reserved, so they never slide up into the cards while
        // fading out.
        .bottom(px(if strip_reserved {
            0.
        } else {
            -CAROUSEL_CONTENT_BOTTOM_PADDING
        }))
        .when(direction == CarouselDirection::Previous, |this| {
            this.left(px(0.))
        })
        .when(direction == CarouselDirection::Next, |this| {
            this.right(px(0.))
        })
        .w(px(CAROUSEL_ARROW_WIDTH))
        .h(px(CAROUSEL_CONTROL_HEIGHT));
    // A hidden arrow keeps its lane but paints nothing and, without an id,
    // hover, cursor, or handlers, inserts no hitbox, so it cannot
    // intercept pointer events or join the tab order while hidden.
    if opacity <= 0. {
        return base.into_any_element();
    }
    let hover_group: SharedString = format!("carousel-arrow-hover-{id}").into();
    let tooltip = match direction {
        CarouselDirection::Previous => "Scroll left",
        CarouselDirection::Next => "Scroll right",
    };
    let click_state = state.clone();
    let key_state = state.clone();
    base.flex()
        .items_center()
        .justify_center()
        .opacity(opacity)
        .id(id)
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
        .into_any_element()
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

/// Reserved line box for collection card titles. Pinning it keeps every card
/// at a deterministic height so pinned carousel and grid rows never diverge
/// from their uniform height hints.
pub(crate) const COLLECTION_CARD_TITLE_LINE_HEIGHT_PX: f32 = 16.;
/// Height of the collection card subtitle row. The row renders only when it
/// carries subtitle text or a privacy icon, so textless cards drop the empty
/// chin; grid row pitch formulas still reserve this line for the uniform
/// card height.
pub(crate) const COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX: f32 = 16.;

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
        false,
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
        false,
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
    Center,
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
    ai_generated: bool,
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
        ai_generated,
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
    ai_generated: bool,
) -> AnyElement {
    // The subtitle renders whenever it carries real information. Eponymous
    // releases (an album named after its artist) must still show the artist,
    // so only the placeholder filter applies. The subtitle row renders only
    // when it carries text or a privacy icon so single line cards lose the
    // empty chin; carousels and grids stretch cards to the tallest sibling
    // and pinned grid rows keep measuring the reserved pitch.
    let show_subtitle = crate::search::collection_subtitle_is_visible(subtitle);
    let has_subtitle_row = show_subtitle || is_private.is_some();
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
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .when(
                            matches!(title_alignment, CollectionCardTitleAlignment::Center)
                                || matches!(
                                    title_alignment,
                                    CollectionCardTitleAlignment::KindDefault
                                ) && matches!(kind, CardKind::Flow),
                            |this| this.justify_center(),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_shrink_1()
                                .truncate()
                                .child(title.to_owned()),
                        )
                        .when(ai_generated, |this| this.child(ai_content_badge()))
                        .text_size(px(12.5))
                        .line_height(px(COLLECTION_CARD_TITLE_LINE_HEIGHT_PX))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_align(TextAlign::Left),
                )
                .when(has_subtitle_row, |this| {
                    this.child(
                        div()
                            .mt(px(2.))
                            .h(px(COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX))
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
                                        .line_height(px(COLLECTION_CARD_SUBTITLE_ROW_HEIGHT_PX))
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackRowLabels {
    pub(crate) explicit: bool,
    pub(crate) ai_generated: bool,
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
    labels: TrackRowLabels,
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
        .child(artwork_view(artwork, &provider_animation_key))
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
                        .when(labels.explicit || labels.ai_generated, |this| {
                            this.child(track_content_labels(labels))
                        }),
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
        return (text, std::iter::once(0..end).collect());
    }
    artist_text_and_ranges(routes)
}

pub(crate) fn track_content_labels(labels: TrackRowLabels) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(4.))
        .when(labels.explicit, |this| this.child(explicit_badge()))
        .when(labels.ai_generated, |this| {
            this.child(ai_content_badge_with_top(2.))
        })
}

pub(crate) fn explicit_badge() -> Stateful<Div> {
    content_badge("E", 14., DANGER, 0xef444466, 0xef444426, "Explicit", 2., 0.)
}

pub(crate) fn ai_content_badge() -> Stateful<Div> {
    ai_content_badge_with_top(1.)
}

/// Player bar variant: the taller title row needs the badge one pixel lower
/// than the card rows do, and its smaller text sits a pixel high inside the
/// pill, so the glyph gets an extra upward nudge there.
pub(crate) fn ai_content_badge_with_top(top: f32) -> Stateful<Div> {
    content_badge(
        "AI",
        18.,
        PRIMARY,
        0x6366f166,
        0x6366f126,
        "AI generated",
        top,
        0.,
    )
}

/// Player bar badges: same geometry as `ai_content_badge_with_top` with the
/// label lifted one pixel inside the pill.
pub(crate) fn player_badge(label: PlayerBadge, top: f32) -> Stateful<Div> {
    match label {
        PlayerBadge::Explicit => content_badge(
            "E", 14., DANGER, 0xef444466, 0xef444426, "Explicit", top, -1.,
        ),
        PlayerBadge::Ai => content_badge(
            "AI",
            18.,
            PRIMARY,
            0x6366f166,
            0x6366f126,
            "AI generated",
            top,
            -1.,
        ),
    }
}

pub(crate) enum PlayerBadge {
    Explicit,
    Ai,
}

#[allow(clippy::too_many_arguments)]
fn content_badge(
    label: &'static str,
    width: f32,
    color: u32,
    border: u32,
    bg: u32,
    tooltip: &'static str,
    top: f32,
    label_top: f32,
) -> Stateful<Div> {
    div()
        .id(label)
        .app_tooltip(tooltip)
        .relative()
        .top(px(top))
        .w(px(width))
        .h(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(rgba(border))
        .bg(rgba(bg))
        .text_size(px(8.))
        .font_weight(FontWeight::EXTRA_BOLD)
        .text_color(rgb(color))
        .child(
            // The glyph advance leaves the ink sitting left of the visual
            // center, so nudge the label half a pixel right. `label_top`
            // lifts or drops the glyph inside the pill for the contexts
            // whose baseline sits differently than the card rows.
            div()
                .relative()
                .left(px(0.5))
                .top(px(label_top))
                .child(label),
        )
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
    ghost_icon_button(
        id,
        LocalIcon::X,
        "Close",
        // The legacy sizing predates height-normalized icons; 10px width on
        // a 384x512 viewBox yields a 13.3px visual height. Keep it exact.
        PANEL_CLOSE_ICON_SIZE_PX.max(icon_size) / LocalIcon::X.aspect_ratio(),
        handler,
    )
}

/// A compact ghost-styled header action button with a hover crossfade between
/// muted and foreground icon states. Shared by panel close and detach
/// controls so both get identical hit areas and focus treatment.
pub(crate) fn ghost_icon_button(
    id: &'static str,
    icon: LocalIcon,
    label: &'static str,
    icon_size: f32,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    ghost_icon_button_with_nudge(id, icon, label, icon_size, 0., handler)
}

/// Same as `ghost_icon_button` with a vertical optical nudge applied to the
/// icon. `icon_size` is the target visual height; the box width is derived
/// from the icon's viewBox aspect so mixed-aspect glyphs render at equal
/// visual sizes.
pub(crate) fn ghost_icon_button_with_nudge(
    id: &'static str,
    icon: LocalIcon,
    label: &'static str,
    icon_size: f32,
    nudge_up: f32,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .group(id)
        .focusable()
        .tab_stop(true)
        .role(gpui::Role::Button)
        .aria_label(label)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor_pointer()
        .app_tooltip(label)
        .hover(|style| style.bg(rgb(BORDER)).border_color(rgb(BORDER)))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            div()
                .relative()
                // GPUI rasterizes SVGs scaled by width, letting height follow
                // the viewBox aspect. Scaling the box width by the aspect
                // ratio renders every glyph at the same visual height.
                .w(px(icon_size * icon.aspect_ratio()))
                .h(px(icon_size))
                .when(nudge_up > 0., |this| this.top(px(-nudge_up)))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover(id, |style| style.invisible())
                        .child(local_icon(icon, MUTED).size_full()),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover(id, |style| style.visible())
                        .child(local_icon(icon, FOREGROUND).size_full()),
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
