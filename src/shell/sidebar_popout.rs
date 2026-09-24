use std::sync::{LazyLock, Mutex};

use gpui::{
    App, AppContext, Bounds, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
    px, size,
};

use crate::{lyrics::LyricsPanel, playback::PlaybackModel, playback::QueuePanel, windows_chrome};

use super::sidebar_popout_root::SidebarPopoutRoot;

static POPOUT_WINDOW: LazyLock<Mutex<Option<WindowHandle<gpui_component::Root>>>> =
    LazyLock::new(|| Mutex::new(None));

const POPOUT_MIN_WIDTH: f32 = 320.;
const POPOUT_MIN_HEIGHT: f32 = 400.;
const POPOUT_INITIAL_WIDTH: f32 = 400.;
const POPOUT_INITIAL_HEIGHT: f32 = 640.;

/// Result of a popout lifecycle request.
enum PopoutSlot {
    Occupied(WindowHandle<gpui_component::Root>),
    Empty,
}

fn take_slot() -> PopoutSlot {
    let Ok(slot) = POPOUT_WINDOW.lock() else {
        return PopoutSlot::Empty;
    };
    match *slot {
        Some(handle) => PopoutSlot::Occupied(handle),
        None => PopoutSlot::Empty,
    }
}

/// Clears the stored handle only when it still matches, so a stale close
/// cannot drop a freshly reopened window.
fn clear_slot(handle: &WindowHandle<gpui_component::Root>) {
    if let Ok(mut slot) = POPOUT_WINDOW.lock() {
        if slot.as_ref() == Some(handle) {
            *slot = None;
        }
    }
}
/// Opens the detached sidebar window, or focuses it when it already exists.
/// The popout reads its hosted view from the playback state, so switching
/// between Lyrics and Queue only needs this call.
pub(super) fn open_or_focus_popout(
    playback: &gpui::Entity<PlaybackModel>,
    lyrics: &gpui::Entity<LyricsPanel>,
    queue: &gpui::Entity<QueuePanel>,
    cx: &mut App,
) {
    if let PopoutSlot::Occupied(handle) = take_slot() {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
        clear_slot(&handle);
    }
    let bounds = Bounds::centered(
        None,
        size(px(POPOUT_INITIAL_WIDTH), px(POPOUT_INITIAL_HEIGHT)),
        cx,
    );
    let playback = playback.clone();
    let closed_playback = playback.clone();
    let lyrics = lyrics.clone();
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("ralgruM".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_owns_titlebar_drag: true,
            focus: true,
            show: true,
            is_resizable: true,
            window_min_size: Some(size(px(POPOUT_MIN_WIDTH), px(POPOUT_MIN_HEIGHT))),
            ..Default::default()
        },
        move |window, cx| build_root(&playback, &lyrics, &queue, window, cx),
    );
    match opened {
        Ok(handle) => {
            if let Ok(mut slot) = POPOUT_WINDOW.lock() {
                *slot = Some(handle);
            }
            // Reconcile when the window dies through any path the shell did
            // not drive (renderer reset, platform close, crash): clear the
            // stale handle and drop the popped state so the panels go back
            // to docked headers instead of dragging the main window.
            let window_id = handle.window_id();
            let playback = closed_playback.clone();
            cx.on_window_closed(move |cx, closed_id| {
                if closed_id != window_id {
                    return;
                }
                let still_current = POPOUT_WINDOW
                    .lock()
                    .map(|slot| {
                        slot.as_ref()
                            .is_some_and(|handle| handle.window_id() == window_id)
                    })
                    .unwrap_or(false);
                if still_current {
                    if let Ok(mut slot) = POPOUT_WINDOW.lock() {
                        *slot = None;
                    }
                    crate::diagnostics::event(
                        "WARN",
                        "sidebar popout window closed outside the dock path",
                    );
                    playback.update(cx, |playback, cx| {
                        playback.state.close_sidebar();
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!("sidebar popout could not be opened: {error}"),
            );
        }
    }
}

/// Closes the detached sidebar window when one exists. The caller decides
/// whether the hosted view redocks; this only handles the window.
pub(super) fn close_popout(cx: &mut App) {
    if let PopoutSlot::Occupied(handle) = take_slot() {
        if handle
            .update(cx, |_, window, _| window.remove_window())
            .is_err()
        {
            clear_slot(&handle);
        }
    }
}

/// Clears the static slot after the window has closed. Invoked from the
/// root's deferred close so a later reopen cannot race a dying handle.
pub(super) fn forget_closed_popout(handle: &WindowHandle<gpui_component::Root>) {
    clear_slot(handle);
}

fn build_root(
    playback: &gpui::Entity<PlaybackModel>,
    lyrics: &gpui::Entity<LyricsPanel>,
    queue: &gpui::Entity<QueuePanel>,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Entity<gpui_component::Root> {
    if let Some(raw) = crate::media_control::window_handle(window) {
        windows_chrome::apply_app_window_chrome(windows::Win32::Foundation::HWND(raw as _));
    }
    let view =
        cx.new(|cx| SidebarPopoutRoot::new(playback.clone(), lyrics.clone(), queue.clone(), cx));
    let weak = view.downgrade();
    window.on_window_should_close(cx, move |_, cx| {
        weak.update(cx, |root, cx| {
            root.request_dock(cx);
            true
        })
        .unwrap_or(true)
    });
    // The queue panel's scrollbars and context-menu dialogs resolve the
    // window root through gpui_component::Root; wrap the view so those
    // lookups succeed instead of panicking.
    cx.new(|cx| gpui_component::Root::new(view, window, cx))
}
