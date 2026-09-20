mod components;
mod controller;
mod slider;

use gpui::{
    App, AppContext as _, Entity, ScrollHandle, SharedString, Task, Window, prelude::*, px, rgba,
};
use gpui_component::{WindowExt, slider::SliderState};

use self::slider::SliderPointerState;
use super::settings::Settings;
use crate::browser_scroll::BrowserScrollState;
use crate::dialog_layout::DialogCloseMotion;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionSetting {
    ActiveRound,
    PlayerDead,
    BetweenRounds,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FadeSetting {
    Out,
    In,
}

pub(super) struct Cs2SettingsDialog {
    settings: Settings,
    active_round: Entity<SliderState>,
    player_dead: Entity<SliderState>,
    between_rounds: Entity<SliderState>,
    fade_out: Entity<SliderState>,
    fade_in: Entity<SliderState>,
    slider_pointer: SliderPointerState,
    error: Option<SharedString>,
    repairing: bool,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
    close_motion: DialogCloseMotion,
    _status_poll: Task<()>,
}

pub(super) fn open(window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let task = cx
        .background_executor()
        .spawn(async { Settings::load_or_create() });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(settings) => open_loaded(settings, window, cx),
                Err(error) => crate::toast::push_global(
                    cx,
                    crate::toast::ToastKind::Error,
                    "Counter-Strike 2 settings could not be opened",
                    Some(error.into()),
                ),
            });
        })
        .detach();
}

/// Keep the whole dialog inside the viewport with a small margin, matching the
/// other centered dialogs.
fn dialog_max_height(viewport_height: f32) -> f32 {
    (viewport_height - 32.).max(0.)
}

fn open_loaded(settings: Settings, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let dialog = cx.new(|cx| Cs2SettingsDialog::new(settings, cx));
    let content = dialog.clone();
    window.open_dialog(cx, move |dialog_view, dialog_window, cx| {
        let viewport_height = f32::from(dialog_window.viewport_size().height);
        let max_h = dialog_max_height(viewport_height);
        let centered_top =
            crate::dialog_layout::centered_margin_top(viewport_height, max_h.min(650.0));
        let closing = content.read(cx).close_motion.closing();
        let close_epoch = content.read(cx).close_motion.epoch();
        let cancel = content.clone();
        dialog_view
            .w(px(570.0))
            .max_w(px(620.0))
            .max_h(px(max_h))
            .margin_top(centered_top)
            .p_0()
            .gap_0()
            .bg(rgba(0x00000000))
            .border_0()
            .rounded(px(8.0))
            .close_button(false)
            .overlay(true)
            .overlay_closable(true)
            .keyboard(false)
            .on_cancel(move |_, window, cx| {
                cancel.update(cx, |this, cx| {
                    crate::dialog_layout::request_dialog_close(this, window, cx)
                });
                false
            })
            .closing(closing, close_epoch)
            .child(content.clone())
    });
}
