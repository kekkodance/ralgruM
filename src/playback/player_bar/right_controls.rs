use super::*;

pub(crate) fn desktop_player_geometry(width: f32, compact: bool) -> (f32, f32, f32, f32) {
    let padding = 16.;
    let column_gap = if compact { 12. } else { 20. };
    let center_width = if compact {
        (width * 0.40).clamp(360., 440.)
    } else {
        (width * 0.40).clamp(360., 520.)
    };
    let side_width = ((width - padding * 2. - column_gap * 2. - center_width) * 0.5).max(0.);
    (padding, column_gap, center_width, side_width)
}

pub(super) fn current_block_width(layout: PlayerBarLayout) -> f32 {
    // The current-track column owns the whole side column. Capping the wide
    // layout here made the title ellipsize while unused width remained beside
    // it.
    layout.side_width
}

pub(super) fn quality_text_animation_key(quality: &str) -> String {
    format!("player-quality-text-{quality}")
}

pub(super) fn quality_text_opacity_endpoints(
    previous_label: &str,
    displayed_label: &str,
) -> (f32, f32) {
    if displayed_label.is_empty() {
        (0., 0.)
    } else if previous_label == displayed_label {
        (1., 1.)
    } else {
        (0., 1.)
    }
}

pub(super) fn quality_badge_opacity_endpoints(
    previous_label: &str,
    displayed_label: &str,
) -> (f32, f32) {
    if displayed_label.is_empty() {
        (0., 0.)
    } else if previous_label.is_empty() {
        (0., 1.)
    } else {
        (1., 1.)
    }
}

pub(super) fn quality_badge_animation_key(generation: u64, quality: &str) -> String {
    format!("player-quality-badge-{generation}-{quality}")
}

pub(super) fn animated_quality_badge(
    generation: u64,
    quality: &str,
    max_width: f32,
    text_opacity_endpoints: (f32, f32),
    badge_opacity_endpoints: (f32, f32),
) -> AnyElement {
    let (from_opacity, target_opacity) = text_opacity_endpoints;
    let text = div()
        .relative()
        .top(px(QUALITY_BADGE_TEXT_OFFSET_PX))
        .min_w_0()
        .truncate()
        .child(quality.to_owned());
    let text: AnyElement = if from_opacity == target_opacity {
        text.opacity(target_opacity).into_any_element()
    } else {
        text.opacity(from_opacity)
            .with_animation(
                quality_text_animation_key(quality),
                crate::motion::interaction(),
                move |this, delta| {
                    this.opacity(crate::motion::lerp(from_opacity, target_opacity, delta))
                },
            )
            .into_any_element()
    };
    let (from_badge_opacity, target_badge_opacity) = badge_opacity_endpoints;
    let badge = quality_badge_shell(max_width).child(text);
    if from_badge_opacity == target_badge_opacity {
        badge.opacity(target_badge_opacity).into_any_element()
    } else {
        badge
            .opacity(from_badge_opacity)
            .with_animation(
                quality_badge_animation_key(generation, quality),
                crate::motion::interaction(),
                move |this, delta| {
                    this.opacity(crate::motion::lerp(
                        from_badge_opacity,
                        target_badge_opacity,
                        delta,
                    ))
                },
            )
            .into_any_element()
    }
}

pub(super) fn quality_badge_shell(max_width: f32) -> gpui::Stateful<gpui::Div> {
    div()
        .id("player-quality-badge")
        .flex_none()
        .h(px(QUALITY_BADGE_HEIGHT_PX))
        .min_w(px(QUALITY_BADGE_MIN_WIDTH_PX))
        .flex()
        .items_center()
        .justify_center()
        .px(px(7.))
        .py(px(2.))
        .rounded(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .max_w(px(max_width))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(SURFACE_RAISED))
        .text_color(rgb(FOREGROUND))
}

pub(super) fn quality_badge_width(window: &Window, quality: &str) -> f32 {
    (text_measure_width(window, quality, 11., FontWeight::SEMIBOLD) + 16.)
        .clamp(QUALITY_BADGE_MIN_WIDTH_PX, 260.)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct RightControlGeometry {
    pub(super) left: f32,
    pub(super) top: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RightControlKind {
    Quality,
    Lyrics,
    Queue,
    Volume,
}

impl RightControlKind {
    pub(super) fn slot(self) -> usize {
        match self {
            Self::Quality => 0,
            Self::Lyrics => 1,
            Self::Queue => 2,
            Self::Volume => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct ScalarMotionVisual {
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ScalarMotion {
    pub(super) initialized: bool,
    pub(super) from: f32,
    pub(super) target: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl ScalarMotion {
    pub(super) fn prepare(
        &mut self,
        target: f32,
        now: Instant,
        animate: bool,
        rebase: bool,
        reduced_motion: bool,
    ) -> ScalarMotionVisual {
        if !self.initialized {
            self.initialized = true;
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if self.target != target {
            if reduced_motion || !animate {
                self.from = target;
                self.target = target;
                self.started_at = None;
            } else if rebase {
                let displayed = self.displayed_at(now);
                self.from = displayed;
                self.target = target;
                self.epoch = self.epoch.wrapping_add(1);
                self.started_at = (displayed != target).then_some(now);
                if displayed == target {
                    self.from = target;
                    self.started_at = None;
                }
            } else if self.started_at.is_some() {
                // The parent mode animation is already running. Resize-only
                // target changes should follow that same clock instead of
                // starting a new eased interpolation for every WM_SIZE event.
                self.target = target;
            } else {
                self.from = target;
                self.target = target;
                self.started_at = None;
            }
        } else if reduced_motion || !animate {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from = self.target;
            self.started_at = None;
        }

        ScalarMotionVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, panel_motion_ease(progress))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::PANEL_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::PANEL_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct RectMotionVisual {
    pub(super) from: RightControlGeometry,
    pub(super) target: RightControlGeometry,
    pub(super) epoch: u64,
    pub(super) active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RectMotion {
    pub(super) initialized: bool,
    pub(super) from: RightControlGeometry,
    pub(super) target: RightControlGeometry,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl RectMotion {
    pub(super) fn prepare(
        &mut self,
        target: RightControlGeometry,
        now: Instant,
        animate: bool,
        rebase: bool,
        reduced_motion: bool,
    ) -> RectMotionVisual {
        if !self.initialized {
            self.initialized = true;
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if self.target != target {
            if reduced_motion || !animate {
                self.from = target;
                self.target = target;
                self.started_at = None;
            } else if rebase {
                let displayed = self.displayed_at(now);
                self.from = displayed;
                self.target = target;
                self.epoch = self.epoch.wrapping_add(1);
                self.started_at = (displayed != target).then_some(now);
                if displayed == target {
                    self.from = target;
                    self.started_at = None;
                }
            } else if self.started_at.is_some() {
                self.target = target;
            } else {
                self.from = target;
                self.target = target;
                self.started_at = None;
            }
        } else if reduced_motion || !animate {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from = self.target;
            self.started_at = None;
        }

        RectMotionVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> RightControlGeometry {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            lerp_right_control_geometry(self.from, self.target, panel_motion_ease(progress))
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::PANEL_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::PANEL_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct FadeMotionVisual {
    pub(super) from: RightControlGeometry,
    pub(super) target: RightControlGeometry,
    pub(super) from_opacity: f32,
    pub(super) target_opacity: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
    pub(super) active: bool,
}

/// Relocating controls use a two-phase fade instead of visible geometry
/// interpolation. The source box remains in place while it fades out, the
/// target box is selected at the midpoint, and it then fades back in.
///
/// `compact` is the only event that starts a fade. A viewport resize during an
/// existing mode transition updates the target geometry in place, preserving
/// the phase and animation identity instead of restarting the fade.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FadeMotion {
    pub(super) compact: Option<bool>,
    pub(super) initialized: bool,
    pub(super) from: RightControlGeometry,
    pub(super) target: RightControlGeometry,
    pub(super) from_opacity: f32,
    pub(super) target_opacity: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Option<Instant>,
}

impl FadeMotion {
    pub(super) fn prepare(
        &mut self,
        compact: bool,
        target: RightControlGeometry,
        now: Instant,
        transition_active: bool,
        reduced_motion: bool,
    ) -> FadeMotionVisual {
        let mode_changed = self.compact != Some(compact);
        if !self.initialized {
            self.initialized = true;
            self.compact = Some(compact);
            self.from = target;
            self.target = target;
            self.from_opacity = 1.;
            self.target_opacity = 1.;
            self.started_at = None;
        } else if mode_changed {
            let (displayed, opacity) = self.displayed_at(now);
            self.compact = Some(compact);
            self.from = displayed;
            self.target = target;
            self.from_opacity = opacity;
            self.target_opacity = 1.;
            self.epoch = self.epoch.wrapping_add(1);
            let needs_animation = displayed != target || opacity != self.target_opacity;
            self.started_at =
                (!reduced_motion && transition_active && needs_animation).then_some(now);
            if reduced_motion || !transition_active || !needs_animation {
                self.from = target;
                self.from_opacity = self.target_opacity;
                self.started_at = None;
            }
        } else if self.target != target {
            if self.started_at.is_some() {
                // Live WM_SIZE retargets keep the existing fade phase and
                // epoch. Only the destination box changes.
                self.target = target;
            } else {
                self.from = target;
                self.target = target;
                self.from_opacity = self.target_opacity;
            }
        }

        if reduced_motion {
            self.from = self.target;
            self.from_opacity = self.target_opacity;
            self.started_at = None;
        } else if self.started_at.is_some()
            && (!transition_active || self.animation_progress(now) >= 1.)
        {
            self.from = self.target;
            self.from_opacity = self.target_opacity;
            self.started_at = None;
        }

        FadeMotionVisual {
            from: self.from,
            target: self.target,
            from_opacity: self.from_opacity,
            target_opacity: self.target_opacity,
            epoch: self.epoch,
            started_at: self.started_at,
            active: self.started_at.is_some(),
        }
    }

    pub(super) fn displayed_at(&self, now: Instant) -> (RightControlGeometry, f32) {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            return (self.target, self.target_opacity);
        }
        if progress < 0.5 {
            let fade_progress = panel_motion_ease(progress * 2.);
            (
                self.from,
                crate::motion::lerp(self.from_opacity, 0., fade_progress),
            )
        } else {
            let fade_progress = panel_motion_ease((progress - 0.5) * 2.);
            (
                self.target,
                crate::motion::lerp(0., self.target_opacity, fade_progress),
            )
        }
    }

    pub(super) fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::PANEL_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::PANEL_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

pub(super) fn player_right_controls(
    side_width: f32,
    generation: u64,
    quality: &str,
    quality_text_opacity: (f32, f32),
    quality_badge_opacity: (f32, f32),
    quality_visual: FadeMotionVisual,
    lyrics: AnyElement,
    queue: AnyElement,
    volume: AnyElement,
    window: &Window,
    right_control_visuals: [RectMotionVisual; 4],
) -> AnyElement {
    let quality_width = quality_badge_width(window, quality);
    let visual = |kind: RightControlKind| right_control_visuals[kind.slot()];

    let quality = right_control_fade(
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(animated_quality_badge(
                generation,
                quality,
                quality_width.max(QUALITY_BADGE_MIN_WIDTH_PX),
                quality_text_opacity,
                quality_badge_opacity,
            ))
            .into_any_element(),
        "player-quality-layout",
        quality_visual,
    );
    let lyrics = right_control(
        lyrics,
        "player-lyrics-layout",
        visual(RightControlKind::Lyrics),
    );
    let queue = right_control(
        queue,
        "player-queue-layout",
        visual(RightControlKind::Queue),
    );
    let volume = right_control(
        volume,
        "player-volume-layout",
        visual(RightControlKind::Volume),
    );
    let controls = div()
        .relative()
        .w(px(side_width))
        .min_w(px(side_width))
        .h_full()
        .flex_none()
        .children(Some(quality))
        .children(Some(lyrics))
        .children(Some(queue))
        .children(Some(volume));
    controls.into_any_element()
}

pub(super) fn right_control_geometry(
    kind: RightControlKind,
    layout: PlayerBarLayout,
    viewport_width: f32,
    compact: bool,
    quality_width: f32,
) -> RightControlGeometry {
    let (_content_width, action_width, action_gap, _volume_shift, second_column_width) =
        compact_player_right_geometry(layout.side_width, viewport_width);
    if compact {
        let action_left = (action_width - (68. + action_gap)) * 0.5;
        let compact_top = match kind {
            RightControlKind::Quality => 17.,
            RightControlKind::Volume => 43.,
            _ => 30.,
        };
        return match kind {
            RightControlKind::Quality => RightControlGeometry {
                left: action_width,
                top: compact_top,
                width: second_column_width,
                height: 24.,
            },
            RightControlKind::Lyrics => RightControlGeometry {
                left: action_left,
                top: compact_top,
                width: 34.,
                height: 34.,
            },
            RightControlKind::Queue => RightControlGeometry {
                left: action_left + 34. + action_gap,
                top: compact_top,
                width: 34.,
                height: 34.,
            },
            RightControlKind::Volume => RightControlGeometry {
                left: action_width - 2.,
                top: compact_top,
                width: second_column_width,
                height: 34.,
            },
        };
    }

    let gap = 12.;
    let group_left = wide_right_group_left(layout.side_width, quality_width);
    let download_left = group_left + quality_width + gap;
    let lyrics_left = download_left + 34. + gap;
    let queue_left = lyrics_left + 34. + gap;
    let volume_left = queue_left + 34. + gap;
    match kind {
        RightControlKind::Quality => RightControlGeometry {
            left: group_left,
            top: 30.,
            width: quality_width,
            height: 34.,
        },
        RightControlKind::Lyrics => RightControlGeometry {
            left: lyrics_left,
            top: 30.,
            width: 34.,
            height: 34.,
        },
        RightControlKind::Queue => RightControlGeometry {
            left: queue_left,
            top: 30.,
            width: 34.,
            height: 34.,
        },
        RightControlKind::Volume => RightControlGeometry {
            left: volume_left,
            top: 30.,
            width: 120.,
            height: 34.,
        },
    }
}

pub(super) fn wide_right_group_left(side_width: f32, quality_width: f32) -> f32 {
    let inner_width = (side_width - 8.).max(0.);
    let gap = 12.;
    let group_width = quality_width + 34. + 34. + 34. + 120. + 14. + gap * 4.;
    (inner_width - group_width).max(0.)
}

pub(super) fn lerp_right_control_geometry(
    from: RightControlGeometry,
    target: RightControlGeometry,
    delta: f32,
) -> RightControlGeometry {
    RightControlGeometry {
        left: crate::motion::lerp(from.left, target.left, delta),
        top: crate::motion::lerp(from.top, target.top, delta),
        width: crate::motion::lerp(from.width, target.width, delta),
        height: crate::motion::lerp(from.height, target.height, delta),
    }
}

pub(super) fn fade_motion_opacity(from: f32, target: f32, delta: f32) -> f32 {
    let delta = delta.clamp(0., 1.);
    if delta < 0.5 {
        crate::motion::lerp(from, 0., panel_motion_ease(delta * 2.))
    } else {
        crate::motion::lerp(0., target, panel_motion_ease((delta - 0.5) * 2.))
    }
}

pub(super) fn fade_motion_geometry(
    from: RightControlGeometry,
    target: RightControlGeometry,
    delta: f32,
) -> RightControlGeometry {
    if delta < 0.5 { from } else { target }
}

pub(super) fn fade_motion_progress(started_at: Instant, now: Instant) -> f32 {
    if crate::motion::PANEL_DURATION.is_zero() {
        return 1.;
    }
    (now.saturating_duration_since(started_at).as_secs_f32()
        / crate::motion::PANEL_DURATION.as_secs_f32())
    .clamp(0., 1.)
}

pub(super) fn right_control(
    element: AnyElement,
    id: &'static str,
    visual: RectMotionVisual,
) -> AnyElement {
    let control = div()
        .absolute()
        .left(px(visual.target.left))
        .top(px(visual.target.top))
        .w(px(visual.target.width))
        .h(px(visual.target.height))
        .child(element);
    if !visual.active {
        return control.into_any_element();
    }
    control
        .left(px(visual.from.left))
        .top(px(visual.from.top))
        .w(px(visual.from.width))
        .h(px(visual.from.height))
        .with_animation(
            (id, visual.epoch),
            crate::motion::panel(),
            move |this, delta| {
                let geometry = lerp_right_control_geometry(visual.from, visual.target, delta);
                this.left(px(geometry.left))
                    .top(px(geometry.top))
                    .w(px(geometry.width))
                    .h(px(geometry.height))
            },
        )
        .into_any_element()
}

pub(super) fn right_control_fade(
    element: AnyElement,
    id: &'static str,
    visual: FadeMotionVisual,
) -> AnyElement {
    let control = div()
        .id(id)
        .absolute()
        .left(px(visual.target.left))
        .top(px(visual.target.top))
        .w(px(visual.target.width))
        .h(px(visual.target.height))
        .child(element);
    if !visual.active {
        return control.opacity(visual.target_opacity).into_any_element();
    }
    control
        .left(px(visual.from.left))
        .top(px(visual.from.top))
        .w(px(visual.from.width))
        .h(px(visual.from.height))
        .opacity(visual.from_opacity)
        .with_animation(
            (id, visual.epoch),
            crate::motion::relocation_fade(),
            move |this, delta| {
                let delta = visual.started_at.map_or(delta, |started_at| {
                    fade_motion_progress(started_at, Instant::now())
                });
                let geometry = fade_motion_geometry(visual.from, visual.target, delta);
                this.left(px(geometry.left))
                    .top(px(geometry.top))
                    .w(px(geometry.width))
                    .h(px(geometry.height))
                    .opacity(fade_motion_opacity(
                        visual.from_opacity,
                        visual.target_opacity,
                        delta,
                    ))
            },
        )
        .into_any_element()
}

pub(super) fn download_position(
    window: &Window,
    viewport_width: f32,
    layout: PlayerBarLayout,
    compact: bool,
    quality: &str,
) -> RightControlGeometry {
    download_position_for_quality_width(
        viewport_width,
        layout,
        compact,
        quality_badge_width(window, quality),
    )
}

pub(super) fn download_position_for_quality_width(
    viewport_width: f32,
    layout: PlayerBarLayout,
    compact: bool,
    quality_width: f32,
) -> RightControlGeometry {
    if compact {
        RightControlGeometry {
            left: (viewport_width + layout.center_width) * 0.5 - 33.,
            top: 19.,
            width: 34.,
            height: 34.,
        }
    } else {
        let right_start = viewport_width - layout.padding - layout.side_width;
        let local = RightControlGeometry {
            left: wide_right_group_left(layout.side_width, quality_width) + quality_width + 12.,
            top: 30.,
            width: 34.,
            height: 34.,
        };
        RightControlGeometry {
            left: right_start + local.left,
            top: local.top,
            width: local.width,
            height: local.height,
        }
    }
}

pub(super) fn compact_player_right_geometry(
    side_width: f32,
    viewport_width: f32,
) -> (f32, f32, f32, f32, f32) {
    let padding_right = (viewport_width * 0.01).clamp(8., 15.);
    let requested_volume_shift = (viewport_width * 0.033 - 25.).clamp(0., 22.);
    let content_width = (side_width - padding_right).max(0.);
    let action_width = (content_width - 120.).max(74.);
    let action_gap = (viewport_width * 0.007).clamp(6., 12.);
    let action_content_width = 68. + action_gap;
    let available_volume_shift = ((action_width - action_content_width) * 0.5).max(0.);
    let volume_shift = requested_volume_shift.min(available_volume_shift);
    let second_column_width = (content_width - action_width).clamp(0., 120.);
    (
        content_width,
        action_width,
        action_gap,
        volume_shift,
        second_column_width,
    )
}

pub(super) fn wide_volume_inset(wide: bool) -> f32 {
    if wide { WIDE_VOLUME_INSET_PX } else { 0. }
}

pub(super) fn volume_container_offsets(wide: bool) -> (f32, f32) {
    (if wide { 14. } else { -2. }, wide_volume_inset(wide))
}

pub(super) fn volume_container_offset_target(wide: bool) -> RightControlGeometry {
    let (left, top) = volume_container_offsets(wide);
    RightControlGeometry {
        left,
        top,
        width: 0.,
        height: 0.,
    }
}
