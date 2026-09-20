mod components;
mod controller;

use gpui::{App, AppContext as _, Entity, SharedString, Task, Window, prelude::*, px, rgba};
use gpui_component::{WindowExt, slider::SliderState};

use super::settings::Settings;
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
    pending_snap: Option<(ActionSetting, f32)>,
    error: Option<SharedString>,
    repairing: bool,
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

fn open_loaded(settings: Settings, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let dialog = cx.new(|cx| Cs2SettingsDialog::new(settings, cx));
    let centered_top =
        crate::dialog_layout::centered_margin_top(f32::from(window.viewport_size().height), 650.0);
    let content = dialog.clone();
    window.open_dialog(cx, move |dialog_view, _, cx| {
        let closing = content.read(cx).close_motion.closing();
        let close_epoch = content.read(cx).close_motion.epoch();
        let cancel = content.clone();
        dialog_view
            .w(px(570.0))
            .max_w(px(620.0))
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
