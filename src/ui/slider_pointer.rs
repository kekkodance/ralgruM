use gpui::{
    App, Bounds, CursorStyle, Hitbox, HitboxBehavior, IntoElement, Pixels, Window, canvas,
    prelude::*, px,
};

#[derive(Clone)]
pub(crate) struct SliderPointerPaint {
    pub(crate) hitbox: Hitbox,
    pub(crate) logical_bounds: Bounds<Pixels>,
}

pub(crate) fn slider_logical_bounds(
    expanded_bounds: Bounds<Pixels>,
    thumb_diameter: f32,
) -> Bounds<Pixels> {
    let inset = px(thumb_diameter * 0.5);
    Bounds {
        origin: gpui::point(expanded_bounds.origin.x + inset, expanded_bounds.origin.y),
        size: gpui::size(
            (expanded_bounds.size.width - inset * 2.).max(px(0.)),
            expanded_bounds.size.height,
        ),
    }
}

pub(crate) fn slider_pointer_cursor(active: bool) -> CursorStyle {
    if active {
        CursorStyle::ClosedHand
    } else {
        CursorStyle::OpenHand
    }
}

pub(crate) fn slider_pointer_surface(
    thumb_diameter: f32,
    active: bool,
    on_paint: impl Fn(SliderPointerPaint, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let thumb_inset = thumb_diameter * 0.5;
    canvas(
        move |bounds, window, _| SliderPointerPaint {
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
            logical_bounds: slider_logical_bounds(bounds, thumb_diameter),
        },
        move |_, paint, window, cx| on_paint(paint, window, cx),
    )
    .absolute()
    .left(px(-thumb_inset))
    .right(px(-thumb_inset))
    .top_0()
    .bottom_0()
    .cursor(slider_pointer_cursor(active))
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, CursorStyle, point, px, size};

    use super::{slider_logical_bounds, slider_pointer_cursor};

    #[test]
    fn expanded_surface_keeps_endpoints_on_the_visible_track() {
        let expanded = Bounds {
            origin: point(px(20.0), px(0.0)),
            size: size(px(113.0), px(24.0)),
        };
        let logical = slider_logical_bounds(expanded, 13.0);

        assert_eq!(logical.origin.x, px(26.5));
        assert_eq!(logical.size.width, px(100.0));
    }

    #[test]
    fn pointer_cursor_reflects_the_drag_state() {
        assert_eq!(slider_pointer_cursor(false), CursorStyle::OpenHand);
        assert_eq!(slider_pointer_cursor(true), CursorStyle::ClosedHand);
    }
}
