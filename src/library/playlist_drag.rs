use std::time::Duration;

use gpui::{
    Bounds, Context, DragMoveEvent, IntoElement, ListState, Pixels, Render, Window, div, point,
    prelude::*, px,
};

use crate::drag_cursor::{
    DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned,
};

use super::view::LibraryView;

const DRAG_EDGE_PX: f32 = 42.;
const DRAG_SCROLL_STEP_PX: f32 = 20.;
pub(crate) const DRAG_SCROLL_TICK: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
pub(crate) struct PlaylistTrackDrag {
    pub(crate) from_index: usize,
}

pub(crate) struct PlaylistTrackDragGhost {
    pub(crate) title: String,
}

#[derive(Clone)]
pub(crate) struct PlaylistDragAutoScroll {
    pointer_y: Pixels,
    viewport: Bounds<Pixels>,
    list_state: ListState,
}

impl PlaylistDragAutoScroll {
    pub(crate) fn from_event(
        event: &DragMoveEvent<PlaylistTrackDrag>,
        list_state: ListState,
    ) -> Self {
        Self {
            pointer_y: event.event.position.y,
            viewport: event.bounds,
            list_state,
        }
    }

    pub(crate) fn step(&self) {
        let delta = drag_scroll_delta(self.pointer_y, self.viewport);
        if delta == 0. {
            return;
        }
        let current_offset = self.list_state.scroll_px_offset_for_scrollbar();
        let max_offset = self.list_state.max_offset_for_scrollbar();
        let target_y =
            clamp_drag_offset(f32::from(current_offset.y), f32::from(max_offset.y), delta);
        self.list_state
            .set_offset_from_scrollbar(point(current_offset.x, px(target_y)));
    }
}

fn drag_scroll_delta(pointer_y: Pixels, viewport: Bounds<Pixels>) -> f32 {
    if pointer_y < viewport.top() + px(DRAG_EDGE_PX) {
        -DRAG_SCROLL_STEP_PX
    } else if pointer_y > viewport.bottom() - px(DRAG_EDGE_PX) {
        DRAG_SCROLL_STEP_PX
    } else {
        0.
    }
}

fn clamp_drag_offset(current_offset_px: f32, max_offset_px: f32, delta_px: f32) -> f32 {
    (current_offset_px - delta_px).clamp(-max_offset_px.max(0.), 0.)
}

impl LibraryView {
    pub(super) fn update_playlist_drag_autoscroll(
        &mut self,
        event: &DragMoveEvent<PlaylistTrackDrag>,
        list_state: ListState,
        cx: &mut Context<Self>,
    ) {
        self.playlist_drag_scroll = Some(PlaylistDragAutoScroll::from_event(event, list_state));
        self.run_playlist_drag_autoscroll(cx);
    }

    fn step_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.playlist_drag_scroll.as_ref() else {
            self.playlist_drag_scroll_running = false;
            return false;
        };
        if !cx.has_active_drag() {
            self.playlist_drag_scroll = None;
            self.playlist_drag_scroll_running = false;
            cx.notify();
            return false;
        }
        drag.step();
        cx.notify();
        true
    }

    fn run_playlist_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.playlist_drag_scroll_running {
            return;
        }
        self.playlist_drag_scroll_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(DRAG_SCROLL_TICK).await;
                let alive = this
                    .update(cx, |view, cx| view.step_playlist_drag_autoscroll(cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }
}

impl Render for PlaylistTrackDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(0.))
            .h(px(0.))
            .overflow_hidden()
            .opacity(0.)
            .child(self.title.clone())
    }
}

pub(super) fn playlist_drag_surface(
    surface: gpui::Stateful<gpui::Div>,
    list_state: ListState,
    cx: &mut Context<LibraryView>,
) -> gpui::Stateful<gpui::Div> {
    surface
        .on_mouse_up_out(gpui::MouseButton::Left, |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::playlist());
            }
        })
        .on_mouse_up(gpui::MouseButton::Left, |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::playlist());
            }
        })
        .on_drag_move::<PlaylistTrackDrag>(cx.listener(move |this, event, window, cx| {
            this.update_playlist_drag_autoscroll(event, list_state.clone(), cx);
            set_drag_cursor_owned(
                window,
                DragCursorState::Grabbing,
                DragCursorOwner::playlist(),
            );
            cx.set_active_drag_cursor_style(grabbing_cursor(), window);
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_edges_match_the_queue_autoscroll_contract() {
        let viewport = Bounds {
            origin: point(px(0.), px(100.)),
            size: gpui::size(px(300.), px(400.)),
        };
        assert_eq!(drag_scroll_delta(px(120.), viewport), -20.);
        assert_eq!(drag_scroll_delta(px(300.), viewport), 0.);
        assert_eq!(drag_scroll_delta(px(480.), viewport), 20.);
    }

    #[test]
    fn drag_scroll_offset_stays_inside_the_list() {
        assert_eq!(clamp_drag_offset(0., 200., -20.), 0.);
        assert_eq!(clamp_drag_offset(0., 200., 20.), -20.);
        assert_eq!(clamp_drag_offset(-190., 200., 20.), -200.);
    }
}
