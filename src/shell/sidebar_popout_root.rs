use gpui::{App, Context, ParentElement, Render, Styled, Window, div, prelude::*, rgb};

use crate::{
    lyrics::LyricsPanel,
    playback::{PlaybackModel, QueuePanel, RightSidebar},
    theme::{BACKGROUND, BORDER},
    ui::app_tooltip::global_tooltip_overlay,
};

/// Root view for the detached sidebar window. Hosts the same live panel
/// entities the in-app sidebar uses, so lyrics sync and queue state carry
/// over without duplication.
pub(crate) struct SidebarPopoutRoot {
    playback: gpui::Entity<PlaybackModel>,
    lyrics: gpui::Entity<LyricsPanel>,
    queue: gpui::Entity<QueuePanel>,
    closing: bool,
    window_handle: Option<gpui::WindowHandle<gpui_component::Root>>,
}

impl SidebarPopoutRoot {
    pub(crate) fn new(
        playback: gpui::Entity<PlaybackModel>,
        lyrics: gpui::Entity<LyricsPanel>,
        queue: gpui::Entity<QueuePanel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        Self {
            playback,
            lyrics,
            queue,
            closing: false,
            window_handle: None,
        }
    }

    fn current_view(&self, cx: &App) -> Option<RightSidebar> {
        self.playback
            .read_with(cx, |model, _| model.state.right_sidebar_popped())
    }

    /// Docks the popped view back into the app and closes this window.
    /// Called by OS-level close requests so Alt+F4 and the taskbar behave
    /// like the panel close button.
    pub(crate) fn request_dock(&mut self, cx: &mut App) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.playback.update(cx, |playback, cx| {
            playback.state.dock_sidebar();
            cx.notify();
        });
        if let Some(handle) = self.window_handle.as_ref() {
            super::sidebar_popout::forget_closed_popout(handle);
        }
    }
}

impl Render for SidebarPopoutRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.window_handle.is_none() {
            self.window_handle = window.window_handle().downcast::<gpui_component::Root>();
        }
        let view = self.current_view(cx);

        let content = match view {
            Some(RightSidebar::Lyrics) => self.lyrics.clone().into_any_element(),
            Some(RightSidebar::Queue) => self.queue.clone().into_any_element(),
            Some(RightSidebar::Closed) | None => {
                if !self.closing {
                    self.closing = true;
                    let handle = window.window_handle().downcast::<gpui_component::Root>();
                    if let Some(handle) = handle {
                        super::sidebar_popout::forget_closed_popout(&handle);
                    }
                    cx.defer_in(window, move |_, window, _| {
                        window.remove_window();
                    });
                }
                div().into_any_element()
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .border_1()
            .border_color(rgb(BORDER))
            .child(content)
            .when_some(global_tooltip_overlay(cx), |this, overlay| {
                this.child(overlay)
            })
    }
}
