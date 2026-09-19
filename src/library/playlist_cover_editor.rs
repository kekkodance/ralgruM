use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

use gpui::{
    Context, CursorStyle, DragMoveEvent, Entity, ExternalPaths, Image, ImageSource, IntoElement,
    KeyDownEvent, ObjectFit, Pixels, Point, Render, Role, ScrollDelta, ScrollWheelEvent,
    SharedString, Window, div, img, prelude::*, px, relative, rgb, rgba,
};
use gpui_component::slider::{Slider, SliderState};

use super::playlist_image::CoverDraft;
use crate::{
    assets::{LocalIcon, local_icon},
    search::Provider,
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED, PRIMARY, SCROLLBAR_THUMB, SURFACE_RAISED},
};

pub(super) const CROP_PREVIEW_SIZE: f32 = 168.;
pub(super) const ZOOM_TRACK_HEIGHT: f32 = 4.;
pub(super) const ZOOM_THUMB_SIZE: f32 = 10.;
pub(super) const ZOOM_CONTROL_HEIGHT: f32 = 24.;

pub(super) trait PlaylistCoverEditor: Sized + 'static {
    fn provider(&self) -> Provider;
    fn cover(&self) -> Option<&CoverDraft>;
    fn cover_preview(&self) -> Option<Arc<Image>>;
    fn existing_cover_url(&self) -> Option<SharedString> {
        None
    }
    fn existing_cover_source(&self) -> Option<ImageSource> {
        self.existing_cover_url().map(Into::into)
    }
    fn zoom_slider(&self) -> &Entity<SliderState>;
    fn choose_cover(&mut self, cx: &mut Context<Self>);
    fn drop_cover_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>);
    fn pan_cover(
        &mut self,
        start_pan_x: i32,
        start_pan_y: i32,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    );
    fn nudge_cover(
        &mut self,
        pointer_delta_x: f32,
        pointer_delta_y: f32,
        preview_size: f32,
        cx: &mut Context<Self>,
    );
    fn nudge_zoom(&mut self, delta: f32, window: &mut Window, cx: &mut Context<Self>);
}

#[derive(Clone)]
struct CoverPanDrag {
    start_pan_x: i32,
    start_pan_y: i32,
    start_offset: Rc<Cell<Option<Point<Pixels>>>>,
    preview_size: f32,
}

struct CoverPanGhost;

impl Render for CoverPanGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.)).opacity(0.)
    }
}

pub(super) fn cover_editor<T: PlaylistCoverEditor>(
    dialog: &T,
    disabled: bool,
    stack_crop: bool,
    preview_size: f32,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let has_cover = dialog.cover().is_some() || dialog.existing_cover_source().is_some();
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(px(12.))
                .text_color(rgb(FOREGROUND))
                .child("Cover image"),
        )
        .when(!has_cover, |d| {
            d.child(cover_chooser(false, disabled, dialog.provider(), cx))
        })
        .when(has_cover, |d| {
            d.child(crop_panel(
                dialog,
                disabled,
                stack_crop,
                preview_size,
                dialog.provider(),
                cx,
            ))
        })
}

fn cover_chooser<T: PlaylistCoverEditor>(
    has_cover: bool,
    disabled: bool,
    provider: Provider,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let label = if has_cover {
        "Change cover image"
    } else {
        "Choose cover image"
    };
    div()
        .id("choose-playlist-cover")
        .focusable()
        .tab_stop(!disabled)
        .role(Role::Button)
        .aria_label(label)
        .w_full()
        .min_h(px(54.))
        .flex()
        .items_center()
        .gap(px(11.))
        .px(px(12.))
        .py(px(9.))
        .border_1()
        .border_dashed()
        .border_color(rgb(0x3f3f46))
        .rounded(px(6.))
        .bg(rgb(BACKGROUND))
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(|style| style.border_color(rgb(0x71717a)).bg(rgb(SURFACE_RAISED)))
                .drag_over::<ExternalPaths>(move |style, paths, _, _| {
                    if dropped_cover_is_supported(paths, provider) {
                        style.border_color(rgb(PRIMARY)).bg(rgba(0x6366f11a))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                    cx.stop_propagation();
                    this.drop_cover_paths(paths.paths(), cx);
                }))
                .on_click(cx.listener(|this, _, _, cx| this.choose_cover(cx)))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        this.choose_cover(cx);
                    }
                }))
        })
        .when(disabled, |d| d.opacity(0.6))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .child(
            div()
                .size(px(32.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .bg(rgb(SURFACE_RAISED))
                .child(local_icon(LocalIcon::Image, 0xd4d4d8).size(px(14.))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label),
                )
                .when(!has_cover, |d| {
                    d.child(
                        div()
                            .text_size(px(10.5))
                            .line_height(relative(1.35))
                            .text_color(rgb(MUTED))
                            .child(super::playlist_image::cover_extensions_label(provider)),
                    )
                }),
        )
}

fn dropped_cover_is_supported(paths: &ExternalPaths, provider: Provider) -> bool {
    let [path] = paths.paths() else {
        return false;
    };
    super::playlist_image::is_supported_cover_path_for(path, provider)
}

fn crop_panel<T: PlaylistCoverEditor>(
    dialog: &T,
    disabled: bool,
    stacked: bool,
    preview_size: f32,
    provider: Provider,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let replacement_selected = dialog.cover().is_some();
    div()
        .flex()
        .gap(px(14.))
        .p(px(12.))
        .border_1()
        .border_color(rgb(BORDER))
        .rounded(px(6.))
        .bg(rgb(BACKGROUND))
        .when(stacked, |d| d.flex_col().items_center())
        .child(crop_preview(dialog, disabled, preview_size, cx))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .flex_col()
                .when(stacked, |d| d.w_full())
                .justify_center()
                .gap(px(10.))
                .child(cover_chooser(true, disabled, provider, cx))
                .when(replacement_selected, |d| {
                    d.child(zoom_control(dialog, disabled, cx)).child(
                        div()
                            .text_size(px(10.5))
                            .line_height(relative(1.35))
                            .text_color(rgb(MUTED))
                            .child("Drag to reposition. Scroll or use the slider to zoom."),
                    )
                }),
        )
}

fn crop_preview<T: PlaylistCoverEditor>(
    dialog: &T,
    disabled: bool,
    preview_size: f32,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let mut preview = div()
        .id("playlist-cover-crop")
        .focusable()
        .tab_stop(!disabled)
        .role(Role::Image)
        .aria_label("Cover crop. Use the arrow keys or drag to reposition the image")
        .relative()
        .w(px(preview_size))
        .h(px(preview_size))
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .bg(rgb(BACKGROUND));

    let Some(cover) = dialog.cover() else {
        return preview
            .when_some(dialog.existing_cover_source(), |d, artwork| {
                d.child(
                    crate::artwork_reveal::artwork_reveal(
                        "playlist-cover-existing-reveal",
                        artwork,
                    )
                    .size_full(),
                )
            })
            .when(disabled, |d| d.opacity(0.6));
    };
    let geometry = cover.display_geometry(preview_size);
    let drag = CoverPanDrag {
        start_pan_x: cover.pan_x,
        start_pan_y: cover.pan_y,
        start_offset: Rc::new(Cell::new(None)),
        preview_size,
    };
    preview = preview.when_some(dialog.cover_preview(), |d, image| {
        d.child(
            img(ImageSource::Image(image))
                .absolute()
                .left(px(geometry.left))
                .top(px(geometry.top))
                .w(px(geometry.width))
                .h(px(geometry.height))
                .object_fit(ObjectFit::Fill),
        )
    });
    if disabled {
        return preview.opacity(0.6);
    }
    preview
        .cursor(CursorStyle::OpenHand)
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
            let delta = match event.delta {
                ScrollDelta::Lines(lines) => cover_wheel_zoom_delta(lines.y, false),
                ScrollDelta::Pixels(pixels) => cover_wheel_zoom_delta(f32::from(pixels.y), true),
            };
            let Some(delta) = delta else {
                return;
            };
            window.prevent_default();
            cx.stop_propagation();
            this.nudge_zoom(delta, window, cx);
        }))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            let (x, y) = match event.keystroke.key.as_str() {
                "left" => (-8., 0.),
                "right" => (8., 0.),
                "up" => (0., -8.),
                "down" => (0., 8.),
                _ => return,
            };
            window.prevent_default();
            this.nudge_cover(x, y, preview_size, cx);
        }))
        .on_drag(drag, |drag, offset, window, cx| {
            drag.start_offset.set(Some(offset));
            cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, window);
            cx.new(|_| CoverPanGhost)
        })
        .on_drag_move::<CoverPanDrag>(cx.listener(
            |this, event: &DragMoveEvent<CoverPanDrag>, window, cx| {
                let drag = event.drag(cx);
                let Some(offset) = drag.start_offset.get() else {
                    return;
                };
                let start = event.bounds.origin + offset;
                let delta = event.event.position - start;
                this.pan_cover(
                    drag.start_pan_x,
                    drag.start_pan_y,
                    f32::from(delta.x),
                    f32::from(delta.y),
                    drag.preview_size,
                    cx,
                );
                cx.set_active_drag_cursor_style(CursorStyle::ClosedHand, window);
            },
        ))
}

pub(super) fn cover_wheel_zoom_delta(vertical_delta: f32, precise: bool) -> Option<f32> {
    if !vertical_delta.is_finite() || vertical_delta == 0. {
        return None;
    }
    let ticks = if precise {
        vertical_delta / 20.
    } else {
        vertical_delta
    };
    Some(ticks.clamp(-1., 1.) * 0.05)
}

fn zoom_control<T: PlaylistCoverEditor>(
    dialog: &T,
    disabled: bool,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let zoom = dialog
        .cover()
        .map(|cover| cover.zoom as f32 / 100.)
        .unwrap_or(1.);
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(9.))
        .child(local_icon(LocalIcon::MagnifyingGlass, MUTED).size(px(13.)))
        .child(
            div()
                .id("playlist-cover-zoom")
                .focusable()
                .tab_stop(!disabled)
                .role(Role::Group)
                .aria_label("Cover image zoom")
                .h(px(ZOOM_CONTROL_HEIGHT))
                .flex_1()
                .relative()
                .when(!disabled, |this| {
                    this.cursor_pointer()
                        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
                        .on_key_down(cx.listener(|dialog, event: &KeyDownEvent, window, cx| {
                            let delta = match event.keystroke.key.as_str() {
                                "left" => -0.05,
                                "right" => 0.05,
                                _ => return,
                            };
                            window.prevent_default();
                            dialog.nudge_zoom(delta, window, cx);
                        }))
                })
                .child(zoom_track((zoom - 1.) / 2.))
                .child(
                    Slider::new(dialog.zoom_slider())
                        .horizontal()
                        .disabled(disabled)
                        .opacity(0.),
                ),
        )
}

fn zoom_track(progress: f32) -> impl IntoElement {
    let progress = progress.clamp(0., 1.);
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((ZOOM_CONTROL_HEIGHT - ZOOM_TRACK_HEIGHT) * 0.5))
        .h(px(ZOOM_TRACK_HEIGHT))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(relative(progress))
                .rounded_full()
                .bg(rgb(PRIMARY)),
        )
        .child(
            div()
                .absolute()
                .left(relative(progress))
                .top(px((ZOOM_TRACK_HEIGHT - ZOOM_THUMB_SIZE) * 0.5))
                .ml(px(-ZOOM_THUMB_SIZE * 0.5))
                .size(px(ZOOM_THUMB_SIZE))
                .rounded_full()
                .bg(rgb(PRIMARY)),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        CROP_PREVIEW_SIZE, ZOOM_CONTROL_HEIGHT, ZOOM_THUMB_SIZE, ZOOM_TRACK_HEIGHT,
        cover_wheel_zoom_delta,
    };

    #[test]
    fn crop_editor_uses_compact_volume_style_geometry() {
        assert_eq!(CROP_PREVIEW_SIZE, 168.);
        assert_eq!(ZOOM_CONTROL_HEIGHT, 24.);
        assert_eq!(ZOOM_TRACK_HEIGHT, 4.);
        assert_eq!(ZOOM_THUMB_SIZE, 10.);
    }

    #[test]
    fn crop_wheel_zoom_bounds_wheel_notches_and_preserves_precise_input() {
        assert_eq!(cover_wheel_zoom_delta(3., false), Some(0.05));
        assert_eq!(cover_wheel_zoom_delta(-3., false), Some(-0.05));
        assert_eq!(cover_wheel_zoom_delta(20., true), Some(0.05));
        assert_eq!(cover_wheel_zoom_delta(10., true), Some(0.025));
        assert_eq!(cover_wheel_zoom_delta(-10., true), Some(-0.025));
        assert_eq!(cover_wheel_zoom_delta(0., true), None);
        assert_eq!(cover_wheel_zoom_delta(f32::NAN, true), None);
    }
}
