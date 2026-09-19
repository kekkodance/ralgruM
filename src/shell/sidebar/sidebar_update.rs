use gpui::{AnimationExt as _, Context, IntoElement, div, prelude::*, px};

use super::{
    COMPACT_SETTINGS_BUTTON_SIZE, RalgrumApp, SIDEBAR_UPDATE_COMPACT_DEFAULT_BOTTOM_PX,
    SIDEBAR_UPDATE_COMPACT_SETTINGS_BOTTOM_PX, SidebarBottomVisual, sidebar_update_opacities_at,
    update_badge_bottom_at,
};

const EXPANDED_LAYER_ID: &str = "sidebar-bottom-expanded-update";
const COMPACT_DEFAULT_LAYER_ID: &str = "sidebar-bottom-compact-default-update";
const COMPACT_SETTINGS_LAYER_ID: &str = "sidebar-bottom-compact-settings-update";

pub(super) fn render_update_layers(
    app: &RalgrumApp,
    compact: bool,
    expanded_width: f32,
    visual: SidebarBottomVisual,
    cx: &mut Context<RalgrumApp>,
) -> impl IntoElement + use<> {
    let (expanded_opacity, compact_default_opacity, compact_settings_opacity) =
        sidebar_update_opacities_at(visual, 0.0);

    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .id(EXPANDED_LAYER_ID)
                .absolute()
                .left_0()
                .bottom(px(update_badge_bottom_at(false, visual, 0.0)))
                .w(px(expanded_width))
                .opacity(expanded_opacity)
                .when(expanded_opacity == 0.0, |this| this.invisible())
                .child(app.update_badge("sidebar-update-button-expanded", false, !compact, cx))
                .with_animation(
                    (EXPANDED_LAYER_ID, visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let (opacity, _, _) = sidebar_update_opacities_at(visual, delta);
                        let this = this
                            .bottom(px(update_badge_bottom_at(false, visual, delta)))
                            .opacity(opacity);
                        if opacity == 0.0 {
                            this.invisible()
                        } else {
                            this.visible()
                        }
                    },
                ),
        )
        .child(
            div()
                .id(COMPACT_DEFAULT_LAYER_ID)
                .absolute()
                .left_0()
                .bottom(px(SIDEBAR_UPDATE_COMPACT_DEFAULT_BOTTOM_PX))
                .w(px(COMPACT_SETTINGS_BUTTON_SIZE))
                .opacity(compact_default_opacity)
                .when(compact_default_opacity == 0.0, |this| this.invisible())
                .child(app.update_badge(
                    "sidebar-update-button-compact-default",
                    true,
                    compact && !app.settings_mode,
                    cx,
                ))
                .with_animation(
                    (COMPACT_DEFAULT_LAYER_ID, visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let (_, opacity, _) = sidebar_update_opacities_at(visual, delta);
                        let this = this.opacity(opacity);
                        if opacity == 0.0 {
                            this.invisible()
                        } else {
                            this.visible()
                        }
                    },
                ),
        )
        .child(
            div()
                .id(COMPACT_SETTINGS_LAYER_ID)
                .absolute()
                .left_0()
                .bottom(px(SIDEBAR_UPDATE_COMPACT_SETTINGS_BOTTOM_PX))
                .w(px(COMPACT_SETTINGS_BUTTON_SIZE))
                .opacity(compact_settings_opacity)
                .when(compact_settings_opacity == 0.0, |this| this.invisible())
                .child(app.update_badge(
                    "sidebar-update-button-compact-settings",
                    true,
                    compact && app.settings_mode,
                    cx,
                ))
                .with_animation(
                    (COMPACT_SETTINGS_LAYER_ID, visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let (_, _, opacity) = sidebar_update_opacities_at(visual, delta);
                        let this = this.opacity(opacity);
                        if opacity == 0.0 {
                            this.invisible()
                        } else {
                            this.visible()
                        }
                    },
                ),
        )
}
