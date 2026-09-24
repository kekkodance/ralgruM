use gpui::{
    AnimationExt, App, Context, ParentElement, Render, Styled, Window, div, prelude::*, px, rgb,
};

use crate::{
    assets::LocalIcon,
    lyrics::LyricsPanel,
    playback::{PlaybackModel, QueuePanel, RightSidebar},
    theme::BACKGROUND,
    ui::app_tooltip::global_tooltip_overlay,
    ui::artwork_cache::global_cache as global_artwork_cache,
};

/// Root view for the detached sidebar window. Hosts the same live panel
/// entities the in-app sidebar uses, so lyrics sync and queue state carry
/// over without duplication.
pub(crate) struct SidebarPopoutRoot {
    playback: gpui::Entity<PlaybackModel>,
    lyrics: gpui::Entity<LyricsPanel>,
    queue: gpui::Entity<QueuePanel>,
    always_on_top: bool,
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
            always_on_top: false,
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
        let content_key = match view {
            Some(RightSidebar::Lyrics) => "lyrics",
            Some(RightSidebar::Queue) => "queue",
            Some(RightSidebar::Closed) | None => "closed",
        };

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

        let content = div()
            .id("sidebar-popout-content")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(content)
            .with_animation(
                format!("sidebar-popout-content-{content_key}"),
                crate::motion::content(),
                |this, delta| {
                    this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                        .left(px(crate::motion::lerp(8.0, 0.0, delta)))
                },
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            // Queue rows resolve their artwork through the window's image
            // cache stack, so the popout must register the shared cache.
            .when_some(global_artwork_cache(cx), |this, cache| {
                this.image_cache(cache)
            })
            .child(
                // Drag strip spanning the window top so the whole upper
                // edge moves the window, plus the always-on-top control.
                div()
                    .id("sidebar-popout-drag-strip")
                    .flex_none()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .px(px(10.))
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .child(crate::music_ui::ghost_icon_button_with_nudge(
                        "sidebar-popout-always-on-top",
                        if self.always_on_top {
                            LocalIcon::Thumbtack
                        } else {
                            LocalIcon::ThumbtackSlash
                        },
                        if self.always_on_top {
                            "Disable always on top"
                        } else {
                            "Enable always on top"
                        },
                        11.,
                        0.,
                        cx.listener(|this, _, window, cx| {
                            this.always_on_top = !this.always_on_top;
                            window.set_always_on_top(this.always_on_top);
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    // Match the in-app right sidebar inner wrapper: 24px
                    // padding, with the panels managing their own bottom
                    // spacing. The top padding moved into the drag strip.
                    .px(px(24.))
                    .pb(px(0.))
                    .child(content),
            )
            .when_some(global_tooltip_overlay(cx), |this, overlay| {
                this.child(overlay)
            })
    }
}
