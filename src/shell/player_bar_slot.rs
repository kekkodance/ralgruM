use gpui::{AnimationExt as _, AnyElement, Entity, IntoElement, div, prelude::*, px};

use crate::playback::{PlaybackView, PlayerBarVisual};

pub(super) fn render_player_bar_slot(
    playback_view: Entity<PlaybackView>,
    narrow: bool,
    visual: PlayerBarVisual,
) -> AnyElement {
    let slot = div()
        .id("shell-player-bar-slot")
        .w_full()
        .flex_none()
        .overflow_hidden()
        .child(playback_view);

    if !visual.active {
        return slot
            .when(!narrow, |this| this.h(px(visual.target_height)))
            .opacity(visual.target_opacity)
            .into_any_element();
    }

    slot.with_animation(
        format!("shell-player-bar-slot-{}", visual.epoch),
        crate::motion::panel(),
        move |this, delta| {
            let opacity = crate::motion::lerp(visual.from_opacity, visual.target_opacity, delta);
            if narrow {
                this.opacity(opacity)
            } else {
                this.h(px(crate::motion::lerp(
                    visual.from_height,
                    visual.target_height,
                    delta,
                )))
                .opacity(opacity)
            }
        },
    )
    .into_any_element()
}
