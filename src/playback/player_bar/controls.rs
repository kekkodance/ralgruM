use super::*;

pub(super) fn hover_icon(
    group: &'static str,
    icon_path: &'static str,
    size: f32,
    color: u32,
    hover_color: u32,
) -> AnyElement {
    div()
        .relative()
        .size(px(size))
        .child(
            div()
                .absolute()
                .inset_0()
                .group_hover(group, |style| style.invisible())
                .child(local_icon_svg(icon_path, size, color)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(local_icon_svg(icon_path, size, hover_color)),
        )
        .into_any_element()
}

pub(super) fn transport_button(
    id: &'static str,
    icon_path: &'static str,
    active: bool,
    enabled: bool,
    icon_size: f32,
    label: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .when(enabled, |this| this.group(id))
        .focusable()
        .tab_stop(enabled)
        .role(Role::Button)
        .aria_label(label)
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |this| this.cursor_pointer())
        .when(!enabled, |this| this.opacity(0.4))
        .border_1()
        .border_color(rgba(0x00000000))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .app_tooltip(label)
        .child(hover_icon(
            id,
            icon_path,
            icon_size,
            if active { PRIMARY } else { MUTED },
            if active { PRIMARY } else { FOREGROUND },
        ))
        .on_click(handler)
        .into_any_element()
}

/// Repeat control with the small "1" badge the original pins onto the corner
/// for repeat-one mode (styles.css #btn-repeat[data-mode="one"]).
pub(super) fn repeat_button(
    mode: RepeatMode,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let active = mode != RepeatMode::Off;
    div()
        .id("repeat")
        .when(enabled, |this| this.group("repeat"))
        .focusable()
        .tab_stop(enabled)
        .role(Role::Button)
        .aria_label(mode.label())
        .relative()
        .size(px(REPEAT_CONTROL_SIZE_PX))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |this| this.cursor_pointer())
        .when(!enabled, |this| this.opacity(0.4))
        .border_1()
        .border_color(rgba(0x00000000))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .app_tooltip(mode.label())
        .child(hover_icon(
            "repeat",
            LocalIcon::Repeat.path(),
            14.,
            if active { PRIMARY } else { MUTED },
            if active { PRIMARY } else { FOREGROUND },
        ))
        .when(mode == RepeatMode::One, |this| {
            this.child(
                div()
                    .absolute()
                    .right(px(REPEAT_ONE_BADGE_RIGHT_PX))
                    .bottom(px(REPEAT_ONE_BADGE_BOTTOM_PX))
                    .min_w(px(10.))
                    .text_size(px(7.))
                    .font_weight(FontWeight::BOLD)
                    .line_height(px(7.))
                    .text_center()
                    .text_color(rgb(PRIMARY))
                    .child("1"),
            )
        })
        .on_click(handler)
        .into_any_element()
}

/// Volume toggle following the original's four icon levels
/// (volume-controller.js updateVolumeIcon).
pub(super) fn volume_button(model: Entity<PlaybackModel>, level: VolumeIconLevel) -> AnyElement {
    div()
        .id("mute")
        .group("mute")
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("Mute / Unmute")
        .size(px(VOLUME_MUTE_BUTTON_PX))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(rgba(0x00000000))
        .hover(|style| style.bg(rgb(BORDER)))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .app_tooltip("Mute / Unmute")
        .child(volume_icon(level))
        .on_click(move |_, window, cx| model.update(cx, |model, cx| model.toggle_mute(window, cx)))
        .into_any_element()
}

pub(super) fn volume_icon(level: VolumeIconLevel) -> AnyElement {
    let icon = match level {
        VolumeIconLevel::Muted => LocalIcon::VolumeMuted,
        VolumeIconLevel::Off => LocalIcon::VolumeOff,
        VolumeIconLevel::Low => LocalIcon::VolumeLow,
        VolumeIconLevel::High => LocalIcon::VolumeHigh,
    };
    let (width, height) = volume_icon_dimensions(level);
    div()
        .relative()
        .size(px(VOLUME_ICON_FRAME_PX))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_start()
                .group_hover("mute", |style| style.invisible())
                .child(volume_icon_svg(level, icon.path(), width, height, MUTED)),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_start()
                .invisible()
                .group_hover("mute", |style| style.visible())
                .child(volume_icon_svg(
                    level,
                    icon.path(),
                    width,
                    height,
                    FOREGROUND,
                )),
        )
        .into_any_element()
}

pub(super) fn volume_icon_svg(
    level: VolumeIconLevel,
    path: &'static str,
    width: f32,
    height: f32,
    color: u32,
) -> impl IntoElement {
    div()
        .relative()
        .flex_none()
        .left(px(volume_icon_speaker_offset(level)))
        .child(local_icon_svg_sized(path, width, height, color))
}

pub(super) fn volume_icon_speaker_offset(level: VolumeIconLevel) -> f32 {
    let speaker_start_viewbox = match level {
        VolumeIconLevel::High => 32.,
        VolumeIconLevel::Low | VolumeIconLevel::Off | VolumeIconLevel::Muted => 0.,
    };
    -VOLUME_ICON_EM_HEIGHT_PX * speaker_start_viewbox / 512.
}

pub(super) fn volume_icon_dimensions(level: VolumeIconLevel) -> (f32, f32) {
    let viewbox_width = match level {
        VolumeIconLevel::High => 640.,
        VolumeIconLevel::Low => 448.,
        VolumeIconLevel::Off => 320.,
        VolumeIconLevel::Muted => 576.,
    };
    (
        VOLUME_ICON_EM_HEIGHT_PX * viewbox_width / 512.,
        VOLUME_ICON_EM_HEIGHT_PX,
    )
}

pub(super) fn close_player_tooltip_gap(compact: bool) -> f32 {
    if compact {
        CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX
    } else {
        CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX
    }
}

pub(super) fn close_button(
    compact: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id("close-player")
        .group("close-player")
        .focusable()
        .tab_stop(true)
        .role(Role::Button)
        .aria_label("Close Player")
        .absolute()
        .top(px(5.))
        .right(px(CLOSE_PLAYER_RIGHT_PX))
        .size(px(16.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_1()
        .border_color(rgba(0x00000000))
        .opacity(0.7)
        .hover(|style| style.opacity(1.))
        .focus_visible(|style| style.opacity(1.).border_color(rgb(PRIMARY)))
        .app_tooltip_with_gap_and_end_inset(
            "Close Player",
            px(close_player_tooltip_gap(compact)),
            px(CLOSE_PLAYER_RIGHT_PX),
        )
        .child(
            div()
                .relative()
                .size(px(PLAYER_CLOSE_GLYPH_PX))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .group_hover("close-player", |style| style.invisible())
                        .child(local_icon_svg(
                            LocalIcon::X.path(),
                            PLAYER_CLOSE_GLYPH_PX,
                            MUTED,
                        )),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .invisible()
                        .group_hover("close-player", |style| style.visible())
                        .child(local_icon_svg(
                            LocalIcon::X.path(),
                            PLAYER_CLOSE_GLYPH_PX,
                            FOREGROUND,
                        )),
                ),
        )
        .on_click(handler)
        .into_any_element()
}

pub(super) fn play_pause_button(
    playing: bool,
    enabled: bool,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (bg, fg) = if enabled {
        (FOREGROUND, 0x09090b)
    } else {
        (SURFACE_RAISED, MUTED)
    };
    div()
        .id("play-pause-btn")
        .focusable()
        .tab_stop(enabled)
        .role(Role::Button)
        .aria_label(if playing { "Pause" } else { "Play" })
        .size(px(36.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(bg))
        .border_1()
        .border_color(rgba(0x00000000))
        .when(enabled, |this| this.cursor_pointer())
        .when(enabled, |this| this.hover(|style| style.opacity(0.85)))
        .when(!enabled, |this| this.opacity(0.55))
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .app_tooltip(if playing { "Pause" } else { "Play" })
        .child(local_icon_svg(
            if playing {
                LocalIcon::Pause.path()
            } else {
                LocalIcon::Play.path()
            },
            14.,
            fg,
        ))
        .on_click(handler)
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bare_action_button(
    id: &'static str,
    icon_path: &'static str,
    selected: bool,
    selected_accent: Option<u32>,
    disabled: bool,
    label: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    bare_action_button_with_interactivity(
        id,
        icon_path,
        selected,
        selected_accent,
        disabled,
        true,
        label,
        handler,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bare_action_button_with_interactivity(
    id: &'static str,
    icon_path: &'static str,
    selected: bool,
    selected_accent: Option<u32>,
    disabled: bool,
    interactive: bool,
    label: &'static str,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (color, hover_color, selected_background) = bare_action_visual(selected, selected_accent);
    let button = div()
        .id(id)
        .focusable()
        .tab_stop(interactive && !disabled)
        .role(Role::Button)
        .aria_label(label)
        .when(!disabled, |this| {
            this.group(id).hover(|style| style.bg(rgb(BORDER)))
        })
        .size(px(34.))
        .rounded(px(PLAYER_ACTION_BUTTON_RADIUS_PX))
        .border_1()
        .border_color(rgba(0x00000000))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(selected_background, |this| this.bg(rgb(BORDER)))
        .when(disabled, |this| {
            this.opacity(PLAYER_ACTION_DISABLED_OPACITY)
        })
        .focus_visible(|style| style.border_color(rgb(PRIMARY)))
        .app_tooltip(label)
        .child(hover_icon(id, icon_path, 14.5, color, hover_color));
    if interactive {
        button.on_click(handler).into_any_element()
    } else {
        button.into_any_element()
    }
}

pub(super) fn bare_action_visual(selected: bool, selected_accent: Option<u32>) -> (u32, u32, bool) {
    if !selected {
        return (MUTED, FOREGROUND, false);
    }
    if let Some(accent) = selected_accent {
        (accent, accent, false)
    } else {
        (FOREGROUND, FOREGROUND, true)
    }
}

pub(super) fn favorite_feedback_opacity(pending: bool) -> (f32, f32) {
    if pending { (1., 1.) } else { (0.82, 1.) }
}

pub(super) fn local_icon_svg(path: &'static str, size: f32, color: u32) -> gpui::Svg {
    gpui::svg().path(path).text_color(rgb(color)).size(px(size))
}

pub(super) fn local_icon_svg_sized(
    path: &'static str,
    width: f32,
    height: f32,
    color: u32,
) -> gpui::Svg {
    gpui::svg()
        .path(path)
        .text_color(rgb(color))
        .w(px(width))
        .h(px(height))
}

pub(super) fn time_text(duration: std::time::Duration) -> AnyElement {
    div()
        .w(px(32.))
        .flex_none()
        .text_size(px(11.5))
        .text_center()
        .text_color(rgb(MUTED))
        .child(format!(
            "{}:{:02}",
            duration.as_secs() / 60,
            duration.as_secs() % 60
        ))
        .into_any_element()
}
