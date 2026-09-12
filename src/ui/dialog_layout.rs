use gpui::{Context, Window, px};
use gpui_component::WindowExt;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DialogCloseMotion {
    closing: bool,
    epoch: u64,
}

impl DialogCloseMotion {
    pub(crate) fn closing(self) -> bool {
        self.closing
    }

    pub(crate) fn epoch(self) -> u64 {
        self.epoch
    }
}

pub(crate) trait DialogCloseTarget: Sized + 'static {
    fn dialog_close_motion(&mut self) -> &mut DialogCloseMotion;

    fn after_dialog_close(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
}

pub(crate) fn request_dialog_close<T: DialogCloseTarget>(
    target: &mut T,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    if target.dialog_close_motion().closing {
        return;
    }
    if cx.reduce_motion() || crate::motion::DIALOG_DURATION.is_zero() {
        window.close_dialog(cx);
        target.after_dialog_close(window, cx);
        return;
    }

    let epoch = {
        let motion = target.dialog_close_motion();
        motion.closing = true;
        motion.epoch = motion.epoch.wrapping_add(1);
        motion.epoch
    };
    cx.notify();
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor()
            .timer(crate::motion::DIALOG_DURATION)
            .await;
        let _ = this.update_in(cx, |target, window, cx| {
            let motion = target.dialog_close_motion();
            if motion.closing && motion.epoch == epoch {
                window.close_dialog(cx);
                target.after_dialog_close(window, cx);
            }
        });
    })
    .detach();
}

pub(crate) fn centered_margin_top(viewport_height: f32, dialog_height: f32) -> gpui::Pixels {
    px(((viewport_height - dialog_height) / 2.).max(0.))
}

#[cfg(test)]
mod tests {
    use super::{DialogCloseMotion, centered_margin_top};

    #[test]
    fn centers_within_viewport_and_never_goes_negative() {
        assert_eq!(centered_margin_top(760., 720.), gpui::px(20.));
        assert_eq!(centered_margin_top(760., 300.), gpui::px(230.));
        assert_eq!(centered_margin_top(400., 720.), gpui::px(0.));
    }

    #[test]
    fn close_motion_starts_idle_so_escape_and_overlay_can_trigger_it_once() {
        let motion = DialogCloseMotion::default();
        assert!(!motion.closing());
        assert_eq!(motion.epoch(), 0);
    }
}
