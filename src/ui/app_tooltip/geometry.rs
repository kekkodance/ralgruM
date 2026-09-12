use gpui::{Bounds, Pixels, Point, Size, point, px, size};

pub(crate) const VIEWPORT_MARGIN: Pixels = px(8.);
pub(crate) const TRIGGER_GAP: Pixels = px(9.);
pub(crate) const ARROW_SIDE: Pixels = px(7.);
pub(crate) const ARROW_INSET: Pixels = px(9.);
pub(crate) const ARROW_STROKE_WIDTH: Pixels = px(1.);
pub(crate) const ARROW_PAINT_PADDING: Pixels = px(1.);
pub(crate) const TOP_SAFE_MARGIN: Pixels = px(40.);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TooltipPlacement {
    Top,
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TooltipGeometry {
    pub(crate) bubble_bounds: Bounds<Pixels>,
    pub(crate) placement: TooltipPlacement,
    /// Absolute coordinate along the bubble edge where the arrow is painted.
    pub(crate) arrow_offset: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TooltipArrowGeometry {
    /// The complete 7px-square diamond after rotation.
    pub(crate) fill_points: [Point<Pixels>; 4],
    /// The two outward-facing edges, represented as one open V path.
    pub(crate) outward_stroke_points: [Point<Pixels>; 3],
    /// Explicit canvas bounds, including one pixel of AA/stroke padding.
    pub(crate) paint_bounds: Bounds<Pixels>,
}

#[cfg(test)]
pub(crate) fn resolve_tooltip_geometry(
    trigger_bounds: Bounds<Pixels>,
    tooltip_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    preferred: TooltipPlacement,
) -> TooltipGeometry {
    resolve_tooltip_geometry_with_gap(
        trigger_bounds,
        tooltip_size,
        viewport_size,
        preferred,
        TRIGGER_GAP,
    )
}

pub(crate) fn resolve_tooltip_geometry_with_gap(
    trigger_bounds: Bounds<Pixels>,
    tooltip_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    preferred: TooltipPlacement,
    trigger_gap: Pixels,
) -> TooltipGeometry {
    resolve_tooltip_geometry_with_gap_and_end_inset(
        trigger_bounds,
        tooltip_size,
        viewport_size,
        preferred,
        trigger_gap,
        VIEWPORT_MARGIN,
    )
}

/// Resolve tooltip geometry with a caller-provided horizontal end inset.
///
/// The inset is used only for horizontal viewport clamping. The normal
/// viewport margin remains in effect vertically, so an edge-pinned tooltip
/// can match the control it belongs to without changing the placement of
/// ordinary tooltips.
pub(crate) fn resolve_tooltip_geometry_with_gap_and_end_inset(
    trigger_bounds: Bounds<Pixels>,
    tooltip_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    preferred: TooltipPlacement,
    trigger_gap: Pixels,
    horizontal_end_inset: Pixels,
) -> TooltipGeometry {
    let trigger_gap = trigger_gap.max(px(0.));
    let horizontal_end_inset = horizontal_end_inset.max(px(0.));
    let centered_x = trigger_bounds.center().x - tooltip_size.width * 0.5;
    let centered_y = trigger_bounds.center().y - tooltip_size.height * 0.5;
    let x = clamp_axis_with_end_inset(
        centered_x,
        tooltip_size.width,
        viewport_size.width,
        VIEWPORT_MARGIN,
        horizontal_end_inset,
    );
    let y = clamp_axis(
        centered_y,
        tooltip_size.height,
        viewport_size.height,
        VIEWPORT_MARGIN,
    );

    let geometry = match preferred {
        TooltipPlacement::Right => {
            let bubble_y = clamp_axis(
                y,
                tooltip_size.height,
                viewport_size.height,
                TOP_SAFE_MARGIN,
            );
            let bubble_x = clamp_axis_with_end_inset(
                trigger_bounds.right() + trigger_gap,
                tooltip_size.width,
                viewport_size.width,
                VIEWPORT_MARGIN,
                horizontal_end_inset,
            );
            TooltipGeometry {
                bubble_bounds: Bounds::new(point(bubble_x, bubble_y), tooltip_size),
                placement: TooltipPlacement::Right,
                arrow_offset: clamp_arrow_offset(
                    trigger_bounds.center().y,
                    bubble_y,
                    tooltip_size.height,
                ),
            }
        }
        TooltipPlacement::Top | TooltipPlacement::Bottom => {
            let above_y = trigger_bounds.top() - trigger_gap - tooltip_size.height;
            let below_y = trigger_bounds.bottom() + trigger_gap;
            let bottom_limit = (viewport_size.height - VIEWPORT_MARGIN).max(VIEWPORT_MARGIN);
            let top_fits = above_y >= TOP_SAFE_MARGIN;
            let bottom_fits = below_y + tooltip_size.height <= bottom_limit;

            let placement = match preferred {
                TooltipPlacement::Top if top_fits => TooltipPlacement::Top,
                TooltipPlacement::Bottom if bottom_fits => TooltipPlacement::Bottom,
                TooltipPlacement::Top if bottom_fits => TooltipPlacement::Bottom,
                TooltipPlacement::Bottom if top_fits => TooltipPlacement::Top,
                _ => {
                    let available_above = (trigger_bounds.top() - VIEWPORT_MARGIN).max(px(0.));
                    let available_below = (bottom_limit - trigger_bounds.bottom()).max(px(0.));
                    if available_below >= available_above {
                        TooltipPlacement::Bottom
                    } else {
                        TooltipPlacement::Top
                    }
                }
            };

            let raw_y = if placement == TooltipPlacement::Top {
                above_y
            } else {
                below_y
            };
            let bubble_y = clamp_axis(
                raw_y,
                tooltip_size.height,
                viewport_size.height,
                TOP_SAFE_MARGIN,
            );
            let bubble_bounds = Bounds::new(point(x, bubble_y), tooltip_size);
            let placement = placement_for_arrow(trigger_bounds, bubble_bounds, placement);

            TooltipGeometry {
                bubble_bounds,
                placement,
                arrow_offset: clamp_arrow_offset(
                    trigger_bounds.center().x,
                    bubble_bounds.left(),
                    tooltip_size.width,
                ),
            }
        }
    };

    geometry
}

/// Keep the arrow oriented toward the trigger when clamping makes a tooltip
/// overlap it. A placement that fits keeps its normal direction.
fn placement_for_arrow(
    trigger_bounds: Bounds<Pixels>,
    bubble_bounds: Bounds<Pixels>,
    fallback: TooltipPlacement,
) -> TooltipPlacement {
    if trigger_bounds.top() >= bubble_bounds.bottom() {
        TooltipPlacement::Top
    } else if trigger_bounds.bottom() <= bubble_bounds.top() {
        TooltipPlacement::Bottom
    } else if trigger_bounds.center().y >= bubble_bounds.center().y {
        TooltipPlacement::Top
    } else if trigger_bounds.center().y < bubble_bounds.center().y {
        TooltipPlacement::Bottom
    } else {
        fallback
    }
}

/// Clamp an axis while preserving the requested edge margin, even when the
/// measured content is wider than the available viewport.
fn clamp_axis(origin: Pixels, length: Pixels, viewport: Pixels, margin: Pixels) -> Pixels {
    let max_origin = (viewport - margin - length).max(margin);
    origin.max(margin).min(max_origin)
}

fn clamp_axis_with_end_inset(
    origin: Pixels,
    length: Pixels,
    viewport: Pixels,
    start_margin: Pixels,
    end_inset: Pixels,
) -> Pixels {
    let max_origin = (viewport - end_inset - length).max(start_margin);
    origin.max(start_margin).min(max_origin)
}

/// Keep the arrow away from rounded bubble corners when the bubble has been
/// clamped against a viewport edge.
pub(crate) fn clamp_arrow_offset(
    anchor: Pixels,
    bubble_start: Pixels,
    bubble_length: Pixels,
) -> Pixels {
    let min = bubble_start + ARROW_INSET;
    let max = (bubble_start + bubble_length - ARROW_INSET).max(min);
    anchor.max(min).min(max)
}

pub(crate) fn estimate_tooltip_size(text: &str, viewport_size: Size<Pixels>) -> Size<Pixels> {
    let max_width = (viewport_size.width - px(16.)).max(px(32.)).min(px(280.));
    let estimated_width = px((text.chars().count() as f32 * 6.1) + 18.);
    size(estimated_width.min(max_width), px(28.))
}

/// Resolve the exact rotated 7px square used by the tooltip arrow.
pub(crate) fn tooltip_arrow_geometry(geometry: TooltipGeometry) -> TooltipArrowGeometry {
    let bubble = geometry.bubble_bounds;
    let half_diagonal = arrow_half_diagonal();
    let (center, outward_stroke_points) = match geometry.placement {
        TooltipPlacement::Top => {
            let center = point(geometry.arrow_offset, bubble.bottom() + px(0.5));
            (
                center,
                [
                    point(center.x - half_diagonal, center.y),
                    point(center.x, center.y + half_diagonal),
                    point(center.x + half_diagonal, center.y),
                ],
            )
        }
        TooltipPlacement::Bottom => {
            let center = point(geometry.arrow_offset, bubble.top() - px(0.5));
            (
                center,
                [
                    point(center.x - half_diagonal, center.y),
                    point(center.x, center.y - half_diagonal),
                    point(center.x + half_diagonal, center.y),
                ],
            )
        }
        TooltipPlacement::Right => {
            let center = point(bubble.left() - px(0.5), geometry.arrow_offset);
            (
                center,
                [
                    point(center.x, center.y - half_diagonal),
                    point(center.x - half_diagonal, center.y),
                    point(center.x, center.y + half_diagonal),
                ],
            )
        }
    };

    let fill_points = [
        point(center.x - half_diagonal, center.y),
        point(center.x, center.y - half_diagonal),
        point(center.x + half_diagonal, center.y),
        point(center.x, center.y + half_diagonal),
    ];
    let paint_bounds = arrow_paint_bounds(&fill_points, &outward_stroke_points);

    TooltipArrowGeometry {
        fill_points,
        outward_stroke_points,
        paint_bounds,
    }
}

fn arrow_half_diagonal() -> Pixels {
    ARROW_SIDE * (1. / std::f32::consts::SQRT_2)
}

/// Convert a global arrow point into the window coordinate space expected by
/// GPUI path painting while keeping it aligned with the canvas bounds.
pub(crate) fn translate_arrow_point_to_canvas(
    global_point: Point<Pixels>,
    arrow_bounds: Bounds<Pixels>,
    canvas_bounds: Bounds<Pixels>,
) -> Point<Pixels> {
    point(
        canvas_bounds.origin.x + global_point.x - arrow_bounds.origin.x,
        canvas_bounds.origin.y + global_point.y - arrow_bounds.origin.y,
    )
}

fn arrow_paint_bounds(
    fill_points: &[Point<Pixels>; 4],
    stroke_points: &[Point<Pixels>; 3],
) -> Bounds<Pixels> {
    let mut min_x = fill_points[0].x;
    let mut max_x = fill_points[0].x;
    let mut min_y = fill_points[0].y;
    let mut max_y = fill_points[0].y;

    for point in fill_points.iter().chain(stroke_points.iter()) {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }

    Bounds::from_corners(
        point(min_x - ARROW_PAINT_PADDING, min_y - ARROW_PAINT_PADDING),
        point(max_x + ARROW_PAINT_PADDING, max_y + ARROW_PAINT_PADDING),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn top_center_uses_trigger_center_and_gap() {
        let geometry = resolve_tooltip_geometry(
            bounds(100., 100., 40., 20.),
            size(px(120.), px(30.)),
            size(px(400.), px(300.)),
            TooltipPlacement::Top,
        );

        assert_eq!(geometry.placement, TooltipPlacement::Top);
        assert_eq!(geometry.bubble_bounds.origin, point(px(60.), px(61.)));
        assert_eq!(geometry.arrow_offset, px(120.));
    }

    #[test]
    fn top_flips_bottom_near_titlebar() {
        let geometry = resolve_tooltip_geometry(
            bounds(100., 12., 40., 20.),
            size(px(120.), px(30.)),
            size(px(400.), px(300.)),
            TooltipPlacement::Top,
        );

        assert_eq!(geometry.placement, TooltipPlacement::Bottom);
        assert_eq!(geometry.bubble_bounds.origin.y, px(41.));
    }

    #[test]
    fn right_placement_centers_on_trigger_and_keeps_gap() {
        let geometry = resolve_tooltip_geometry(
            bounds(40., 120., 30., 20.),
            size(px(100.), px(40.)),
            size(px(400.), px(300.)),
            TooltipPlacement::Right,
        );

        assert_eq!(geometry.placement, TooltipPlacement::Right);
        assert_eq!(geometry.bubble_bounds.origin, point(px(79.), px(110.)));
        assert_eq!(geometry.arrow_offset, px(130.));
    }

    #[test]
    fn viewport_clamp_preserves_eight_pixel_edge_margin() {
        let geometry = resolve_tooltip_geometry(
            bounds(0., 100., 12., 20.),
            size(px(180.), px(40.)),
            size(px(200.), px(300.)),
            TooltipPlacement::Top,
        );

        assert_eq!(geometry.bubble_bounds.left(), px(8.));
        assert_eq!(geometry.arrow_offset, px(17.));
    }

    #[test]
    fn arrow_offset_is_clamped_inside_edge_clamped_bubble() {
        assert_eq!(clamp_arrow_offset(px(0.), px(8.), px(120.)), px(17.));
        assert_eq!(clamp_arrow_offset(px(400.), px(8.), px(120.)), px(119.));
    }

    #[test]
    fn right_placement_respects_titlebar_safe_margin() {
        let geometry = resolve_tooltip_geometry(
            bounds(40., 12., 30., 20.),
            size(px(100.), px(40.)),
            size(px(400.), px(300.)),
            TooltipPlacement::Right,
        );

        assert_eq!(geometry.bubble_bounds.origin.y, TOP_SAFE_MARGIN);
        assert_eq!(geometry.arrow_offset, TOP_SAFE_MARGIN + ARROW_INSET);
    }

    #[test]
    fn tooltip_arrow_has_a_full_diamond_and_outward_v() {
        let geometry = TooltipGeometry {
            bubble_bounds: bounds(40., 40., 120., 30.),
            placement: TooltipPlacement::Top,
            arrow_offset: px(100.),
        };
        let arrow = tooltip_arrow_geometry(geometry);
        let [left, top, right, bottom] = arrow.fill_points;
        let [stroke_left, stroke_tip, stroke_right] = arrow.outward_stroke_points;

        assert_eq!(left.y, right.y);
        assert_eq!(top.x, bottom.x);
        assert_eq!(left.x, stroke_left.x);
        assert_eq!(right.x, stroke_right.x);
        assert_eq!(stroke_tip.x, px(100.));
        assert!(stroke_tip.y > left.y);
        assert_eq!(stroke_tip, bottom);
        assert_eq!(
            top.y,
            geometry.bubble_bounds.bottom() + px(0.5) - arrow_half_diagonal()
        );
    }

    #[test]
    fn tooltip_arrow_shapes_match_each_placement_and_skip_hidden_edge() {
        let bubble = bounds(100., 100., 120., 30.);
        let offset = px(160.);

        let top = tooltip_arrow_geometry(TooltipGeometry {
            bubble_bounds: bubble,
            placement: TooltipPlacement::Top,
            arrow_offset: offset,
        });
        assert_eq!(
            top.outward_stroke_points,
            [top.fill_points[0], top.fill_points[3], top.fill_points[2],]
        );

        let bottom = tooltip_arrow_geometry(TooltipGeometry {
            bubble_bounds: bubble,
            placement: TooltipPlacement::Bottom,
            arrow_offset: offset,
        });
        assert_eq!(
            bottom.outward_stroke_points,
            [
                bottom.fill_points[0],
                bottom.fill_points[1],
                bottom.fill_points[2],
            ]
        );

        let right = tooltip_arrow_geometry(TooltipGeometry {
            bubble_bounds: bubble,
            placement: TooltipPlacement::Right,
            arrow_offset: px(115.),
        });
        assert_eq!(
            right.outward_stroke_points,
            [
                right.fill_points[1],
                right.fill_points[0],
                right.fill_points[3],
            ]
        );

        assert!(top.outward_stroke_points[1].y > bubble.bottom());
        assert!(bottom.outward_stroke_points[1].y < bubble.top());
        assert!(right.outward_stroke_points[1].x < bubble.left());
    }

    #[test]
    fn tooltip_arrow_paint_bounds_enclose_shape_with_padding() {
        let geometry = TooltipGeometry {
            bubble_bounds: bounds(40., 40., 120., 30.),
            placement: TooltipPlacement::Top,
            arrow_offset: px(100.),
        };
        let arrow = tooltip_arrow_geometry(geometry);
        let bounds = arrow.paint_bounds;

        assert!(bounds.size.width > px(0.));
        assert!(bounds.size.height > px(0.));
        for point in arrow
            .fill_points
            .iter()
            .chain(arrow.outward_stroke_points.iter())
        {
            assert!(point.x >= bounds.left() + ARROW_PAINT_PADDING);
            assert!(point.x <= bounds.right() - ARROW_PAINT_PADDING);
            assert!(point.y >= bounds.top() + ARROW_PAINT_PADDING);
            assert!(point.y <= bounds.bottom() - ARROW_PAINT_PADDING);
        }
    }

    #[test]
    fn arrow_point_translation_preserves_local_offset_at_nonzero_canvas_origin() {
        let global_point = point(px(110.), px(205.));
        let arrow_bounds = bounds(100., 200., 20., 20.);
        let canvas_bounds = bounds(300., 400., 20., 20.);

        assert_eq!(
            translate_arrow_point_to_canvas(global_point, arrow_bounds, canvas_bounds),
            point(px(310.), px(405.))
        );
    }

    #[test]
    fn top_tooltip_near_right_edge_clamps_bubble_and_arrow() {
        let geometry = resolve_tooltip_geometry(
            bounds(770., 100., 30., 20.),
            size(px(180.), px(30.)),
            size(px(800.), px(600.)),
            TooltipPlacement::Top,
        );

        assert_eq!(geometry.placement, TooltipPlacement::Top);
        assert_eq!(geometry.bubble_bounds.left(), px(612.));
        assert_eq!(geometry.bubble_bounds.right(), px(800.) - VIEWPORT_MARGIN);
        assert_eq!(
            geometry.arrow_offset,
            geometry.bubble_bounds.right() - ARROW_INSET
        );

        let arrow = tooltip_arrow_geometry(geometry);
        let [left, tip, right] = arrow.outward_stroke_points;
        assert_eq!(tip.x, geometry.arrow_offset);
        assert_eq!(
            tip.y,
            geometry.bubble_bounds.bottom() + px(0.5) + arrow_half_diagonal()
        );
        assert_eq!(left.y, right.y);
        assert!(tip.y > left.y);
    }

    #[test]
    fn custom_horizontal_end_inset_pins_bubble_and_arrow_to_requested_edge() {
        let geometry = resolve_tooltip_geometry_with_gap_and_end_inset(
            bounds(770., 100., 30., 20.),
            size(px(180.), px(30.)),
            size(px(800.), px(600.)),
            TooltipPlacement::Top,
            TRIGGER_GAP,
            px(5.),
        );

        assert_eq!(geometry.bubble_bounds.right(), px(795.));
        assert_eq!(geometry.arrow_offset, px(785.));

        let arrow = tooltip_arrow_geometry(geometry);
        assert_eq!(arrow.outward_stroke_points[1].x, geometry.arrow_offset);
    }

    #[test]
    fn top_custom_gap_moves_bubble_without_changing_horizontal_alignment() {
        let trigger = bounds(300., 100., 40., 20.);
        let tooltip_size = size(px(100.), px(30.));
        let viewport_size = size(px(800.), px(600.));
        let default =
            resolve_tooltip_geometry(trigger, tooltip_size, viewport_size, TooltipPlacement::Top);
        let custom = resolve_tooltip_geometry_with_gap(
            trigger,
            tooltip_size,
            viewport_size,
            TooltipPlacement::Top,
            px(5.),
        );

        assert_eq!(custom.placement, TooltipPlacement::Top);
        assert_eq!(custom.bubble_bounds.bottom(), trigger.top() - px(5.));
        assert_eq!(custom.bubble_bounds.left(), default.bubble_bounds.left());
        assert_eq!(custom.bubble_bounds.right(), default.bubble_bounds.right());
        assert_eq!(custom.arrow_offset, default.arrow_offset);

        let tip = tooltip_arrow_geometry(custom).outward_stroke_points[1];
        assert_eq!(
            tip.y,
            trigger.top() - px(5.) + px(0.5) + arrow_half_diagonal()
        );
    }

    #[test]
    fn top_three_pixel_gap_reaches_two_pixels_into_sixteen_pixel_trigger() {
        let trigger = bounds(300., 100., 16., 16.);
        let tooltip_size = size(px(100.), px(30.));
        let viewport_size = size(px(800.), px(600.));
        let default =
            resolve_tooltip_geometry(trigger, tooltip_size, viewport_size, TooltipPlacement::Top);
        let custom = resolve_tooltip_geometry_with_gap(
            trigger,
            tooltip_size,
            viewport_size,
            TooltipPlacement::Top,
            px(3.),
        );

        assert_eq!(custom.placement, TooltipPlacement::Top);
        assert_eq!(custom.bubble_bounds.left(), default.bubble_bounds.left());
        assert_eq!(custom.bubble_bounds.right(), default.bubble_bounds.right());
        assert_eq!(custom.arrow_offset, default.arrow_offset);

        let tip = tooltip_arrow_geometry(custom).outward_stroke_points[1];
        assert_eq!(
            tip.y,
            trigger.top() - px(3.) + px(0.5) + arrow_half_diagonal()
        );
        assert!(tip.y < trigger.bottom());
    }

    #[test]
    fn close_player_estimate_uses_standard_tooltip_width() {
        let viewport = size(px(800.), px(600.));
        let estimated = estimate_tooltip_size("Close Player", viewport);

        assert_eq!(estimated, size(px(91.2), px(28.)));
    }
}
