//! Bottom player bar view: transport controls, seek bar, volume, and the
//! per-track action cluster. Extracted from the playback model module so the
//! model stays free of rendering concerns.

use std::{cell::Cell, rc::Rc, time::Instant};

use gpui::{
    AccessibleAction, AnimationExt, AnyElement, App, Bounds, ClickEvent, Context, Entity, Font,
    FontWeight, Hitbox, HitboxBehavior, Hsla, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Render, Role, TextRun, Window,
    canvas, div, img, point, prelude::*, px, relative, rgb, rgba, size,
};
use gpui_component::slider::{Slider, SliderState, SliderValue};

use crate::{
    app_tooltip::AppTooltipExt,
    assets::LocalIcon,
    context_menu,
    downloads::DownloadModel,
    entity_navigation::{ProviderNavigationOpeners, artist_routes_for_track},
    library::{FavoriteController, FavoriteKey, FavoriteKind, FavoriteState, LibraryView},
    music_ui::TrackArtistNavigation,
    settings::AccountState,
    theme::{
        BORDER, DANGER, FOREGROUND, MUTED, PRIMARY, SCROLLBAR_THUMB, SURFACE, SURFACE_RAISED,
        ui_font_family,
    },
};

use super::view::SEEK_SLIDER_STEP;
use super::{
    PlaybackModel, PlaybackProvider, PlaybackStatus, PlaybackTrack, RepeatMode, RightSidebar,
    VolumeIconLevel,
};

const QUALITY_BADGE_HEIGHT_PX: f32 = 20.;
const QUALITY_BADGE_MIN_WIDTH_PX: f32 = 42.0;
const QUALITY_BADGE_TEXT_OFFSET_PX: f32 = -1.;
const PLAYER_BAR_DESKTOP_HEIGHT_PX: f32 = 94.;
const PLAYER_ACTION_BUTTON_RADIUS_PX: f32 = 6.;
const PLAYER_ACTION_DISABLED_OPACITY: f32 = 0.4;
const PLAYER_CLOSE_GLYPH_PX: f32 = 9.;
const CLOSE_PLAYER_RIGHT_PX: f32 = 5.;
const CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX: f32 = 5.;
const CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX: f32 = 3.;
const VOLUME_GAP_PX: f32 = 7.;
const VOLUME_MUTE_BUTTON_PX: f32 = 34.;
const VOLUME_SLIDER_WIDTH_PX: f32 = 80.;
const VOLUME_SLIDER_CONTROL_HEIGHT_PX: f32 = 24.;
const VOLUME_TRACK_HEIGHT_PX: f32 = 4.;
const VOLUME_THUMB_DIAMETER_PX: f32 = 13.;
const VOLUME_ICON_FRAME_PX: f32 = 16.;
const VOLUME_ICON_EM_HEIGHT_PX: f32 = 14.5;
const WIDE_VOLUME_INSET_PX: f32 = 12.;
const REPEAT_CONTROL_SIZE_PX: f32 = 14.;
const REPEAT_ONE_BADGE_RIGHT_PX: f32 = 2.;
const REPEAT_ONE_BADGE_BOTTOM_PX: f32 = 4.;
const FAVORITE_PINK: u32 = 0xec4899;

#[derive(Clone, Copy, Debug, PartialEq)]
struct BufferedVisual {
    generation: u64,
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

#[derive(Debug)]
struct BufferedMotion {
    generation: Option<u64>,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for BufferedMotion {
    fn default() -> Self {
        Self {
            generation: None,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl BufferedMotion {
    fn prepare(
        &mut self,
        generation: u64,
        target: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> BufferedVisual {
        let target = target.clamp(0., 1.);
        let changed = self.generation != Some(generation) || self.target != target;

        if changed {
            let displayed = self.displayed_at(now);
            self.generation = Some(generation);
            self.from = displayed;
            self.target = target;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && displayed != target).then_some(now);
            if reduced_motion || displayed == target {
                self.from = target;
                self.started_at = None;
            }
        } else if reduced_motion {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() {
            if self.animation_progress(now) >= 1. {
                self.from = self.target;
                self.started_at = None;
            }
        }

        BufferedVisual {
            generation,
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::CONTENT_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::CONTENT_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SeekFillVisual {
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

impl SeekFillVisual {
    fn direct(progress: f32) -> Self {
        let progress = progress.clamp(0., 1.);
        Self {
            from: progress,
            target: progress,
            epoch: 0,
            active: false,
        }
    }
}

#[derive(Debug)]
struct SeekFillMotion {
    last_epoch: u64,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for SeekFillMotion {
    fn default() -> Self {
        Self {
            last_epoch: 0,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl SeekFillMotion {
    fn reset(&mut self, last_epoch: u64) {
        *self = Self {
            last_epoch,
            ..Self::default()
        };
    }

    fn set_displayed(&mut self, progress: f32) {
        let progress = progress.clamp(0., 1.);
        self.from = progress;
        self.target = progress;
        self.started_at = None;
    }

    fn prepare(
        &mut self,
        commit_epoch: u64,
        from: f32,
        target: f32,
        now: Instant,
        reduced_motion: bool,
    ) -> SeekFillVisual {
        let from = from.clamp(0., 1.);
        let target = target.clamp(0., 1.);

        if self.last_epoch != commit_epoch {
            let displayed = if self.started_at.is_some() {
                self.displayed_at(now)
            } else {
                from
            };
            self.last_epoch = commit_epoch;
            self.from = displayed;
            self.target = target;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && displayed != target).then_some(now);
            if reduced_motion || displayed == target {
                self.from = target;
                self.started_at = None;
            }
        } else if reduced_motion {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from = self.target;
            self.started_at = None;
        }

        SeekFillVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::INTERACTION_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::INTERACTION_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VolumeVisual {
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VolumeMotionMode {
    Animated,
    Hold(f32),
    Direct,
}

#[derive(Debug)]
struct VolumeMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for VolumeMotion {
    fn default() -> Self {
        Self {
            initialized: false,
            from: 0.,
            target: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl VolumeMotion {
    fn prepare(
        &mut self,
        target: f32,
        now: Instant,
        reduced_motion: bool,
        mode: VolumeMotionMode,
    ) -> VolumeVisual {
        let target = target.clamp(0., 1.);

        if !self.initialized {
            self.initialized = true;
            let initial = match mode {
                VolumeMotionMode::Hold(held) => held.clamp(0., 1.),
                VolumeMotionMode::Animated | VolumeMotionMode::Direct => target,
            };
            self.from = initial;
            self.target = initial;
            self.started_at = None;
        } else if let VolumeMotionMode::Hold(held) = mode {
            let held = held.clamp(0., 1.);
            self.from = held;
            self.target = held;
            self.started_at = None;
        } else if mode == VolumeMotionMode::Direct {
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if self.target != target {
            let displayed = self.displayed_at(now);
            self.from = displayed;
            self.target = target;
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && displayed != target).then_some(now);
            if reduced_motion || displayed == target {
                self.from = target;
                self.started_at = None;
            }
        } else if reduced_motion {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from = self.target;
            self.started_at = None;
        }

        VolumeVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, gpui::ease_in_out(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return 1.;
        };
        if crate::motion::INTERACTION_DURATION.is_zero() {
            return 1.;
        }
        (now.saturating_duration_since(started_at).as_secs_f32()
            / crate::motion::INTERACTION_DURATION.as_secs_f32())
        .clamp(0., 1.)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayerBarVisual {
    from_height: f32,
    target_height: f32,
    from_opacity: f32,
    target_opacity: f32,
    epoch: u64,
    active: bool,
}

#[derive(Debug)]
struct PlayerBarMotion {
    open: Option<bool>,
    narrow: Option<bool>,
    from_height: f32,
    target_height: f32,
    from_opacity: f32,
    target_opacity: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for PlayerBarMotion {
    fn default() -> Self {
        Self {
            open: None,
            narrow: None,
            from_height: 0.,
            target_height: 0.,
            from_opacity: 0.,
            target_opacity: 0.,
            epoch: 0,
            started_at: None,
        }
    }
}

impl PlayerBarMotion {
    fn prepare(
        &mut self,
        open: bool,
        narrow: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> PlayerBarVisual {
        let target_height = if open && !narrow {
            PLAYER_BAR_DESKTOP_HEIGHT_PX
        } else {
            0.
        };
        let target_opacity = if open { 1. } else { 0. };
        let changed = self.open != Some(open)
            || self.narrow != Some(narrow)
            || self.target_height != target_height
            || self.target_opacity != target_opacity;

        if changed {
            let (displayed_height, displayed_opacity) = self.displayed_at(now);
            self.open = Some(open);
            self.narrow = Some(narrow);
            self.from_height = if narrow {
                target_height
            } else {
                displayed_height
            };
            self.target_height = target_height;
            self.from_opacity = if open {
                target_opacity
            } else {
                displayed_opacity
            };
            self.target_opacity = target_opacity;
            self.epoch = self.epoch.wrapping_add(1);
            let needs_animation = (!narrow && self.from_height != target_height)
                || self.from_opacity != target_opacity;
            self.started_at = (!reduced_motion && needs_animation).then_some(now);
            if reduced_motion || !needs_animation {
                self.from_height = target_height;
                self.from_opacity = target_opacity;
                self.started_at = None;
            }
        } else if reduced_motion {
            self.from_height = self.target_height;
            self.from_opacity = self.target_opacity;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from_height = self.target_height;
            self.from_opacity = self.target_opacity;
            self.started_at = None;
        }

        PlayerBarVisual {
            from_height: self.from_height,
            target_height: self.target_height,
            from_opacity: self.from_opacity,
            target_opacity: self.target_opacity,
            epoch: self.epoch,
            active: self.started_at.is_some(),
        }
    }

    fn displayed_at(&self, now: Instant) -> (f32, f32) {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            (self.target_height, self.target_opacity)
        } else {
            let progress = panel_motion_ease(progress);
            (
                crate::motion::lerp(self.from_height, self.target_height, progress),
                crate::motion::lerp(self.from_opacity, self.target_opacity, progress),
            )
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
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

fn panel_motion_ease(progress: f32) -> f32 {
    1. - (1. - progress.clamp(0., 1.)).powi(5)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayerBarLayout {
    padding: f32,
    column_gap: f32,
    center_width: f32,
    side_width: f32,
}

impl PlayerBarLayout {
    fn from_geometry(geometry: (f32, f32, f32, f32)) -> Self {
        let (padding, column_gap, center_width, side_width) = geometry;
        Self {
            padding,
            column_gap,
            center_width,
            side_width,
        }
    }

    fn geometry(self) -> (f32, f32, f32, f32) {
        (
            self.padding,
            self.column_gap,
            self.center_width,
            self.side_width,
        )
    }

    fn at(self, target: Self, delta: f32) -> Self {
        Self {
            padding: crate::motion::lerp(self.padding, target.padding, delta),
            column_gap: crate::motion::lerp(self.column_gap, target.column_gap, delta),
            center_width: crate::motion::lerp(self.center_width, target.center_width, delta),
            side_width: crate::motion::lerp(self.side_width, target.side_width, delta),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PlayerBarLayoutVisual {
    from: PlayerBarLayout,
    target: PlayerBarLayout,
    epoch: u64,
    active: bool,
    mode_changed: bool,
}

impl PlayerBarLayoutVisual {
    fn at(self, delta: f32) -> PlayerBarLayout {
        self.from.at(self.target, delta)
    }
}

#[derive(Debug)]
struct PlayerBarLayoutMotion {
    compact: Option<bool>,
    from: PlayerBarLayout,
    target: PlayerBarLayout,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for PlayerBarLayoutMotion {
    fn default() -> Self {
        let layout = PlayerBarLayout::from_geometry((0., 0., 0., 0.));
        Self {
            compact: None,
            from: layout,
            target: layout,
            epoch: 0,
            started_at: None,
        }
    }
}

impl PlayerBarLayoutMotion {
    fn prepare(
        &mut self,
        compact: bool,
        target: PlayerBarLayout,
        now: Instant,
        reduced_motion: bool,
    ) -> PlayerBarLayoutVisual {
        let mode_changed = self.compact.is_some() && self.compact != Some(compact);
        let geometry_changed = self.target != target;
        if self.compact.is_none() {
            self.compact = Some(compact);
            self.from = target;
            self.target = target;
            self.started_at = None;
        } else if mode_changed {
            self.from = self.displayed_at(now);
            self.target = target;
            self.compact = Some(compact);
            self.epoch = self.epoch.wrapping_add(1);
            self.started_at = (!reduced_motion && self.from != self.target).then_some(now);
        } else if geometry_changed {
            if self.started_at.is_some() {
                // Live WM_SIZE events are part of the same mode transition.
                // Update only the destination so the original animation clock,
                // easing phase, and element identity keep advancing instead of
                // restarting on every resize frame.
                self.target = target;
            } else {
                // A resize within an idle mode should track the viewport directly.
                self.from = target;
                self.target = target;
                self.started_at = None;
            }
        }

        if reduced_motion {
            self.from = self.target;
            self.started_at = None;
        } else if self.started_at.is_some() && self.animation_progress(now) >= 1. {
            self.from = self.target;
            self.started_at = None;
        }

        PlayerBarLayoutVisual {
            from: self.from,
            target: self.target,
            epoch: self.epoch,
            active: self.started_at.is_some(),
            mode_changed,
        }
    }

    fn displayed_at(&self, now: Instant) -> PlayerBarLayout {
        if self.started_at.is_none() || self.animation_progress(now) >= 1. {
            self.target
        } else {
            self.from
                .at(self.target, panel_motion_ease(self.animation_progress(now)))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
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

pub(crate) struct PlaybackView {
    model: Entity<PlaybackModel>,
    seek_slider: Entity<SliderState>,
    volume_slider: Entity<SliderState>,
    account: Entity<AccountState>,
    downloads: Entity<DownloadModel>,
    library: Entity<LibraryView>,
    favorites: Entity<FavoriteState>,
    favorite_controller: Entity<FavoriteController>,
    external_track_navigation: Option<ProviderNavigationOpeners>,
    seek_control_enabled: Option<bool>,
    seek_pointer_state: Rc<Cell<SeekPointerState>>,
    volume_pointer_state: Rc<Cell<VolumePointerState>>,
    buffered_motion: BufferedMotion,
    seek_fill_motion: SeekFillMotion,
    volume_motion: VolumeMotion,
    last_seek_commit_epoch: u64,
    last_display_progress: f32,
    player_bar_motion: PlayerBarMotion,
    player_bar_layout_motion: PlayerBarLayoutMotion,
    text_width_motion: ScalarMotion,
    favorite_motion: FadeMotion,
    right_control_motion: [RectMotion; 4],
    quality_motion: FadeMotion,
    download_motion: FadeMotion,
    volume_offset_motion: RectMotion,
    last_quality_label: String,
    last_quality_generation: Option<u64>,
}

impl PlaybackView {
    pub(crate) fn new(
        model: Entity<PlaybackModel>,
        account: Entity<AccountState>,
        downloads: Entity<DownloadModel>,
        library: Entity<LibraryView>,
        favorites: Entity<FavoriteState>,
        favorite_controller: Entity<FavoriteController>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&favorites, |_, _, cx| cx.notify()).detach();
        Self {
            seek_slider: model.read(cx).seek_slider.clone(),
            volume_slider: model.read(cx).volume_slider.clone(),
            model,
            account,
            downloads,
            library,
            favorites,
            favorite_controller,
            external_track_navigation: None,
            seek_control_enabled: None,
            seek_pointer_state: Rc::new(Cell::new(SeekPointerState::default())),
            volume_pointer_state: Rc::new(Cell::new(VolumePointerState::default())),
            buffered_motion: BufferedMotion::default(),
            seek_fill_motion: SeekFillMotion::default(),
            volume_motion: VolumeMotion::default(),
            last_seek_commit_epoch: 0,
            last_display_progress: 0.,
            player_bar_motion: PlayerBarMotion::default(),
            player_bar_layout_motion: PlayerBarLayoutMotion::default(),
            text_width_motion: ScalarMotion::default(),
            favorite_motion: FadeMotion::default(),
            right_control_motion: [RectMotion::default(); 4],
            quality_motion: FadeMotion::default(),
            download_motion: FadeMotion::default(),
            volume_offset_motion: RectMotion::default(),
            last_quality_label: String::new(),
            last_quality_generation: None,
        }
    }

    pub(crate) fn set_external_track_navigation(&mut self, openers: ProviderNavigationOpeners) {
        self.external_track_navigation = Some(openers);
    }

    fn download_current(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.model.read(cx).state.current().cloned() else {
            return;
        };
        let (deezer_arl, soundcloud_token) = {
            let account = self.account.read(cx);
            (account.deezer_arl(), account.soundcloud_token())
        };
        self.downloads.update(cx, |downloads, cx| {
            downloads.start(
                track,
                deezer_arl,
                soundcloud_token,
                crate::playback::DownloadVariant::Best,
                cx,
            );
        });
    }

    fn toggle_current_favorite(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.model.read(cx).state.current().cloned() else {
            return;
        };
        let key = FavoriteKey::for_provider(
            search_provider_for_playback(track.provider),
            FavoriteKind::Track,
            track.id,
        );
        self.favorite_controller
            .update(cx, |controller, cx| controller.toggle(key, false, cx));
    }
}

impl Render for PlaybackView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let metrics = crate::music_ui::shell_metrics_for_viewport(
            f32::from(viewport.width),
            f32::from(viewport.height),
        );
        let (
            current,
            status,
            position,
            duration,
            buffered,
            volume,
            generation,
            seek_commit_epoch,
            error,
            shuffle_enabled,
            repeat_mode,
            right_sidebar,
            loading_from_cache,
        ) = {
            let model = self.model.read(cx);
            let state = &model.state;
            (
                state.current().cloned(),
                state.status,
                state.position,
                state.duration,
                state.buffered,
                state.volume,
                state.generation,
                model.seek_commit_epoch(),
                state.error.clone(),
                state.shuffle_enabled,
                state.repeat_mode,
                state.right_sidebar,
                model.loading_from_cache(),
            )
        };
        let seek_control_enabled =
            matches!(status, PlaybackStatus::Playing | PlaybackStatus::Paused);
        if self.seek_control_enabled != Some(seek_control_enabled) {
            self.seek_control_enabled = Some(seek_control_enabled);
            self.model.update(cx, |model, _| {
                model.set_seek_slider_enabled(seek_control_enabled);
            });
        }
        if !seek_control_enabled && self.seek_pointer_state.get().active {
            self.seek_pointer_state.set(SeekPointerState::default());
            window.release_pointer();
        }
        let narrow = metrics.narrow_content;
        let compact = metrics.compact_player;
        let open = status != PlaybackStatus::Empty;
        let quality = if open {
            self.model.read(cx).resolved_quality().map(str::to_owned)
        } else {
            None
        };
        let previous_quality_label = self.last_quality_label.clone();
        if status == PlaybackStatus::Empty {
            self.last_quality_label.clear();
            self.last_quality_generation = None;
        } else {
            update_quality_label_for_generation(
                &mut self.last_quality_label,
                &mut self.last_quality_generation,
                generation,
                quality.as_deref(),
            );
        }
        let quality_label = self.last_quality_label.clone();
        let quality_text_opacity =
            quality_text_opacity_endpoints(&previous_quality_label, &quality_label);
        let quality_badge_opacity =
            quality_badge_opacity_endpoints(&previous_quality_label, &quality_label);
        let now = Instant::now();
        let player_bar_visual =
            self.player_bar_motion
                .prepare(open, narrow, now, cx.reduce_motion());
        let viewport_width = f32::from(viewport.width).max(0.);
        let layout =
            PlayerBarLayout::from_geometry(desktop_player_geometry(viewport_width, compact));
        let layout_visual =
            self.player_bar_layout_motion
                .prepare(compact, layout, now, cx.reduce_motion());
        let (padding, column_gap, center_width, side_width) = layout.geometry();
        if status == PlaybackStatus::Empty {
            if self.volume_pointer_state.get().is_active() {
                self.volume_pointer_state.set(VolumePointerState::default());
                window.release_pointer();
            }
            self.buffered_motion = BufferedMotion::default();
            self.seek_fill_motion.reset(seek_commit_epoch);
            self.last_seek_commit_epoch = seek_commit_epoch;
            self.last_display_progress = 0.;
            return render_player_bar_host(narrow, player_bar_visual, None);
        }
        let favorite_key = current_favorite_key(current.as_ref());
        let (favorite, favorite_pending) = favorite_key.as_ref().map_or((None, false), |key| {
            let favorites = self.favorites.read(cx);
            (favorites.favorite(key), favorites.pending(key))
        });
        let progress = if duration.is_zero() {
            0.0
        } else {
            position.as_secs_f32() / duration.as_secs_f32()
        };
        let seek_preview = self.model.read(cx).seek_preview_fraction();
        let display_progress = seekbar_display_progress(progress, seek_preview);
        let seek_fill_visual = if let Some(preview) = seek_preview {
            self.seek_fill_motion.set_displayed(preview);
            self.last_display_progress = preview;
            SeekFillVisual::direct(preview)
        } else {
            if seek_commit_epoch != self.last_seek_commit_epoch {
                self.last_seek_commit_epoch = seek_commit_epoch;
            }
            let visual = self.seek_fill_motion.prepare(
                seek_commit_epoch,
                self.last_display_progress,
                progress,
                now,
                cx.reduce_motion(),
            );
            let visual = if visual.active {
                visual
            } else {
                SeekFillVisual::direct(progress)
            };
            self.last_display_progress = if visual.active {
                self.seek_fill_motion.displayed_at(now)
            } else {
                progress
            };
            visual
        };
        let display_position = seek_preview
            .map(|_| duration.mul_f32(display_progress))
            .unwrap_or(position);
        let buffered_fraction = if duration.is_zero() {
            0.0
        } else {
            (buffered.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
        };
        let buffered_visual =
            self.buffered_motion
                .prepare(generation, buffered_fraction, now, cx.reduce_motion());
        if self.seek_slider.read(cx).value() != SliderValue::Single(display_progress) {
            self.seek_slider.update(cx, |slider, cx| {
                slider.set_value(display_progress, window, cx);
            });
        }
        if self.volume_slider.read(cx).value() != SliderValue::Single(volume) {
            self.volume_slider.update(cx, |slider, cx| {
                slider.set_value(volume, window, cx);
            });
        }
        let volume_motion_mode = volume_motion_mode_for_render(&self.volume_pointer_state);
        let volume_visual =
            self.volume_motion
                .prepare(volume, now, cx.reduce_motion(), volume_motion_mode);
        let volume_displayed_value = self.volume_motion.displayed_at(now);
        let play_enabled = matches!(
            status,
            PlaybackStatus::Playing
                | PlaybackStatus::Paused
                | PlaybackStatus::Ended
                | PlaybackStatus::Loading
        );
        // During a preloaded track transition the status briefly reports
        // Loading while playback continues. Keep the seek bar gated on the
        // settled states so only the transport button stays continuous.
        let seek_bar_enabled = matches!(
            status,
            PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
        );
        let next_enabled = self.model.read(cx).state.can_next();
        let previous_enabled = current.is_some();
        let download_track = current.clone();
        let downloads_for_button = self.downloads.clone();
        let account_for_button = self.account.clone();
        let download_button = move |cx: &mut Context<Self>, interactive: Rc<Cell<bool>>| {
            if let Some(track) = download_track.clone() {
                let listener_interactive = interactive.clone();
                let menu_interactive = interactive.clone();
                let button = div()
                    .id("player-download")
                    .group("player-download")
                    .hover(|style| style.bg(rgb(BORDER)))
                    .size(px(34.))
                    .rounded(px(PLAYER_ACTION_BUTTON_RADIUS_PX))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .app_tooltip("Download")
                    .child(hover_icon(
                        "player-download",
                        LocalIcon::Download.path(),
                        14.5,
                        MUTED,
                        FOREGROUND,
                    ))
                    .on_click(move |_, _, cx| {
                        if listener_interactive.get() {
                            cx.stop_propagation();
                        }
                    });
                // The player bar sits at the window bottom, so open the format
                // menu above the button with its bottom edge against the button
                // top edge and a chevron pointing down to the button.
                // While the layout motion runs, keep the plain button so a
                // click cannot open the menu mid transition.
                if menu_interactive.get() {
                    context_menu::track_download_button_above(
                        button,
                        track,
                        downloads_for_button.clone(),
                        account_for_button.clone(),
                    )
                    .into_any_element()
                } else {
                    button.into_any_element()
                }
            } else {
                let listener_interactive = interactive.clone();
                bare_action_button_with_interactivity(
                    "player-download",
                    LocalIcon::Download.path(),
                    false,
                    None,
                    !download_available(None),
                    true,
                    "Download",
                    {
                        let listener = cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.download_current(cx);
                        });
                        move |event, window, cx| {
                            if listener_interactive.get() {
                                listener(event, window, cx)
                            }
                        }
                    },
                )
            }
        };
        let lyrics_button = || {
            let model = self.model.clone();
            let selected = right_sidebar == RightSidebar::Lyrics;
            bare_action_button(
                "lyrics-toggle",
                LocalIcon::QuoteRight.path(),
                selected,
                None,
                current.is_none(),
                "Lyrics",
                move |_, _, cx| model.update(cx, |model, cx| model.toggle_lyrics(cx)),
            )
        };
        let queue_button = || {
            let model = self.model.clone();
            let selected = right_sidebar == RightSidebar::Queue;
            bare_action_button(
                "queue-toggle",
                LocalIcon::ListUl.path(),
                selected,
                None,
                current.is_none(),
                "Queue",
                move |_, _, cx| model.update(cx, |model, cx| model.toggle_queue(cx)),
            )
        };

        let artist_navigation = current.as_ref().and_then(|track| {
            current_artist_navigation(
                track,
                status,
                error.as_deref(),
                self.external_track_navigation.as_ref(),
            )
        });
        let title_for_motion = current
            .as_ref()
            .map_or("Nothing playing", |track| track.title.as_str());
        let subtitle_for_motion = current_subtitle(
            current.as_ref(),
            status,
            error.as_deref(),
            loading_from_cache,
        );
        let rendered_artist_for_motion = rendered_current_artist_text(
            current.as_ref(),
            status,
            error.as_deref(),
            subtitle_for_motion,
            artist_navigation.as_ref(),
        );
        let intrinsic_text_width = (!narrow && favorite_key.is_some()).then(|| {
            current_text_intrinsic_width(window, title_for_motion, &rendered_artist_for_motion)
        });
        let text_width_visual = intrinsic_text_width.map(|intrinsic| {
            let target = current_text_width_for_layout(layout, compact, true, intrinsic);
            self.text_width_motion.prepare(
                target,
                now,
                layout_visual.active,
                layout_visual.mode_changed,
                cx.reduce_motion(),
            )
        });
        let favorite_visual = intrinsic_text_width.map(|intrinsic| {
            let target = RightControlGeometry {
                left: favorite_left_for_layout(layout, compact, intrinsic),
                top: favorite_top(compact),
                width: 34.,
                height: 34.,
            };
            self.favorite_motion.prepare(
                compact,
                target,
                now,
                layout_visual.active,
                cx.reduce_motion(),
            )
        });
        let quality_width = quality_badge_width(window, quality_label.as_str());
        let quality_target = right_control_geometry(
            RightControlKind::Quality,
            layout,
            viewport_width,
            compact,
            quality_width,
        );
        let quality_visual = self.quality_motion.prepare(
            compact,
            quality_target,
            now,
            layout_visual.active,
            cx.reduce_motion(),
        );
        let right_control_visuals = std::array::from_fn(|index| {
            let kind = match index {
                0 => RightControlKind::Quality,
                1 => RightControlKind::Lyrics,
                2 => RightControlKind::Queue,
                _ => RightControlKind::Volume,
            };
            let target =
                right_control_geometry(kind, layout, viewport_width, compact, quality_width);
            if kind == RightControlKind::Quality {
                RectMotionVisual {
                    from: target,
                    target,
                    ..RectMotionVisual::default()
                }
            } else {
                self.right_control_motion[index].prepare(
                    target,
                    now,
                    layout_visual.active,
                    layout_visual.mode_changed,
                    cx.reduce_motion(),
                )
            }
        });
        let download_target = download_position(
            window,
            viewport_width,
            layout,
            compact,
            quality_label.as_str(),
        );
        let download_motion_visual = self.download_motion.prepare(
            compact,
            download_target,
            now,
            layout_visual.active,
            cx.reduce_motion(),
        );
        let download_visual = (!narrow).then_some(download_motion_visual);
        let download_interactive = Rc::new(Cell::new(!download_motion_visual.active));
        let favorite_button = favorite_key.is_some().then(|| {
            let (feedback_start, feedback_end) = favorite_feedback_opacity(favorite_pending);
            let interactive = Rc::new(Cell::new(
                favorite_visual.map_or(true, |visual| !visual.active),
            ));
            let listener_interactive = interactive.clone();
            let button = bare_action_button_with_interactivity(
                "player-favorite",
                LocalIcon::Heart.path(),
                favorite == Some(true),
                Some(FAVORITE_PINK),
                favorite_pending,
                true,
                "Favorite Track",
                {
                    let listener = cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_current_favorite(cx);
                    });
                    move |event, window, cx| {
                        if listener_interactive.get() {
                            listener(event, window, cx)
                        }
                    }
                },
            );
            let button = div()
                .size(px(34.))
                .flex_none()
                .child(button)
                .with_animation(
                    format!(
                        "player-favorite-feedback-{}-{favorite_pending}",
                        favorite == Some(true)
                    ),
                    crate::motion::interaction(),
                    move |this, delta| {
                        this.opacity(crate::motion::lerp(feedback_start, feedback_end, delta))
                    },
                )
                .into_any_element();
            (button, interactive)
        });
        let favorite_interactive = favorite_button
            .as_ref()
            .map(|(_, interactive)| interactive.clone());
        let favorite_button = favorite_button.map(|(button, _)| button);
        let volume_offset_visual = self.volume_offset_motion.prepare(
            volume_container_offset_target(!compact),
            now,
            layout_visual.active,
            layout_visual.mode_changed,
            cx.reduce_motion(),
        );

        let current_width_animation = (!narrow && layout_visual.active).then(|| {
            (
                current_block_width(layout_visual.from),
                current_block_width(layout),
                layout_visual.epoch,
            )
        });

        let current_block = match current.as_ref() {
            None => render_current(
                None,
                status,
                error.as_deref(),
                loading_from_cache,
                (!narrow).then_some(layout),
                compact,
                narrow,
                favorite_button,
                current_width_animation,
                None,
                text_width_visual,
                favorite_visual,
                favorite_interactive.clone(),
                window,
            ),
            Some(track) => {
                let rendered = render_current(
                    Some(track),
                    status,
                    error.as_deref(),
                    loading_from_cache,
                    (!narrow).then_some(layout),
                    compact,
                    narrow,
                    favorite_button,
                    current_width_animation,
                    artist_navigation,
                    text_width_visual,
                    favorite_visual,
                    favorite_interactive,
                    window,
                );
                context_menu::current_menu(
                    div().id("current-track-context").child(rendered),
                    track.clone(),
                    self.model.clone(),
                    self.downloads.clone(),
                    self.account.clone(),
                    self.library.clone(),
                    self.external_track_navigation.clone(),
                )
            }
        };

        let download_for_center = narrow.then(|| download_button(cx, download_interactive.clone()));
        let download_for_desktop =
            (!narrow).then(|| download_button(cx, download_interactive.clone()));

        let center = div()
            .when(narrow, |this| this.w_full())
            .when(!narrow, |this| this.w(px(center_width)).flex_none())
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(if narrow { 8. } else { 16. }))
                    // The mobile controls share this row with the extra action
                    // buttons, matching the original mobile footer markup.
                    .when(narrow, |this| {
                        this.child(animated_quality_badge(
                            generation,
                            quality_label.as_str(),
                            260.,
                            quality_text_opacity,
                            quality_badge_opacity,
                        ))
                        .children(download_for_center)
                    })
                    .child(transport_button(
                        "shuffle",
                        LocalIcon::Shuffle.path(),
                        shuffle_enabled,
                        current.is_some(),
                        14.,
                        "Shuffle",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.toggle_shuffle(cx))
                        },
                    ))
                    .child(transport_button(
                        "previous",
                        LocalIcon::Previous.path(),
                        false,
                        previous_enabled,
                        12.,
                        "Previous",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.previous(cx))
                        },
                    ))
                    .child(play_pause_button(
                        matches!(status, PlaybackStatus::Playing | PlaybackStatus::Loading),
                        play_enabled,
                        {
                            let model = self.model.clone();
                            move |_, _, cx| model.update(cx, |model, cx| model.toggle(cx))
                        },
                    ))
                    .child(transport_button(
                        "next",
                        LocalIcon::Next.path(),
                        false,
                        next_enabled,
                        12.,
                        "Next",
                        {
                            let model = self.model.clone();
                            move |_, _, cx| {
                                if model.read(cx).state.can_next() {
                                    model.update(cx, |model, cx| model.next(cx));
                                }
                            }
                        },
                    ))
                    .child(repeat_button(repeat_mode, current.is_some(), {
                        let model = self.model.clone();
                        move |_, _, cx| model.update(cx, |model, cx| model.cycle_repeat(cx))
                    }))
                    .when(narrow, |this| {
                        this.child(lyrics_button()).child(queue_button())
                    }),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(if narrow { 8. } else { 10. }))
                    .child(time_text(display_position))
                    .child(progress_control(
                        self.model.clone(),
                        self.seek_pointer_state.clone(),
                        buffered_visual,
                        seek_fill_visual,
                        seek_bar_enabled,
                        cx,
                    ))
                    .child(time_text(duration)),
            );

        let center = if !narrow && layout_visual.active {
            let center_layout_visual = layout_visual.clone();
            center
                .with_animation(
                    ("player-center-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = center_layout_visual.clone().at(delta).center_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            center.into_any_element()
        };

        let right = if narrow {
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(12.))
                .into_any_element()
        } else {
            let volume_level = self.model.read(cx).state.volume_icon_level();
            let volume = volume_controls(
                self.model.clone(),
                self.volume_slider.clone(),
                self.volume_pointer_state.clone(),
                volume_level,
                volume_visual,
                volume_displayed_value,
                volume_offset_visual,
            );
            player_right_controls(
                side_width,
                generation,
                quality_label.as_str(),
                quality_text_opacity,
                quality_badge_opacity,
                quality_visual,
                lyrics_button(),
                queue_button(),
                volume,
                window,
                right_control_visuals,
            )
        };

        let right = if !narrow && layout_visual.active {
            let right_layout_visual = layout_visual.clone();
            div()
                .w(px(side_width))
                .min_w(px(side_width))
                .h_full()
                .flex_none()
                .child(right)
                .with_animation(
                    ("player-right-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = right_layout_visual.clone().at(delta).side_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            right
        };

        let current_column = div()
            .when(narrow, |this| this.w_full())
            .when(!narrow, |this| this.w(px(side_width)).flex_none())
            .min_w_0()
            .child(current_block);
        let current_column = if !narrow && layout_visual.active {
            let current_layout_visual = layout_visual.clone();
            current_column
                .with_animation(
                    ("player-current-column-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = current_layout_visual.clone().at(delta).side_width;
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            current_column.into_any_element()
        };

        let player_row = div()
            .flex_none()
            .when(narrow, |this| this.h_auto().flex_col())
            .when(!narrow, |this| this.h(px(PLAYER_BAR_DESKTOP_HEIGHT_PX)))
            .w_full()
            .flex()
            .items_center()
            .gap(px(if narrow { 8. } else { column_gap }))
            .px(px(if narrow { 10. } else { padding }))
            .child(current_column)
            .child(center)
            .child(right);

        let player_row = if !narrow && layout_visual.active {
            let row_layout_visual = layout_visual.clone();
            player_row
                .with_animation(
                    ("player-row-layout", layout_visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let layout = row_layout_visual.clone().at(delta);
                        this.gap(px(layout.column_gap)).px(px(layout.padding))
                    },
                )
                .into_any_element()
        } else {
            player_row.into_any_element()
        };

        let player_content = div()
            .relative()
            .w_full()
            .child(player_row)
            .children(download_for_desktop.map(|button| {
                let visual = download_visual.expect("desktop download has a motion visual");
                let interactive = download_interactive.clone();
                let download = div()
                    .id("player-download-layout")
                    .absolute()
                    .left(px(visual.target.left))
                    .top(px(visual.target.top))
                    .w(px(visual.target.width))
                    .h(px(visual.target.height))
                    .child(button);
                if visual.active {
                    download
                        .left(px(visual.from.left))
                        .top(px(visual.from.top))
                        .w(px(visual.from.width))
                        .h(px(visual.from.height))
                        .opacity(visual.from_opacity)
                        .with_animation(
                            ("player-download-layout", visual.epoch),
                            crate::motion::relocation_fade(),
                            move |this, delta| {
                                let delta = visual.started_at.map_or(delta, |started_at| {
                                    fade_motion_progress(started_at, Instant::now())
                                });
                                interactive.set(delta >= 1.);
                                let geometry =
                                    fade_motion_geometry(visual.from, visual.target, delta);
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
                } else {
                    interactive.set(true);
                    download.into_any_element()
                }
            }))
            .child(close_button(compact, {
                let model = self.model.clone();
                move |_, _, cx| model.update(cx, |model, cx| model.close(cx))
            }));

        render_player_bar_host(
            narrow,
            player_bar_visual,
            Some(player_content.into_any_element()),
        )
    }
}

fn render_player_bar_host(
    narrow: bool,
    visual: PlayerBarVisual,
    content: Option<AnyElement>,
) -> AnyElement {
    let has_content = content.is_some();
    let mut host = div()
        .id("player-bar-host")
        .relative()
        .w_full()
        .overflow_hidden()
        .border_t_1()
        .border_color(rgb(BORDER))
        .bg(rgb(0x09090b))
        .children(content);

    if narrow {
        host = if has_content {
            host.h_auto()
        } else {
            host.h(px(0.))
        };
    } else if !visual.active {
        host = host.h(px(visual.target_height));
    }

    if visual.active {
        host.with_animation(
            format!("player-bar-host-{}", visual.epoch),
            crate::motion::panel(),
            move |this, delta| {
                let opacity =
                    crate::motion::lerp(visual.from_opacity, visual.target_opacity, delta);
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
    } else {
        host.opacity(visual.target_opacity).into_any_element()
    }
}

fn current_favorite_key(track: Option<&PlaybackTrack>) -> Option<FavoriteKey> {
    track.map(|track| {
        FavoriteKey::for_provider(
            search_provider_for_playback(track.provider),
            FavoriteKind::Track,
            track.id.clone(),
        )
    })
}

fn search_provider_for_playback(provider: PlaybackProvider) -> crate::search::Provider {
    match provider {
        PlaybackProvider::Deezer => crate::search::Provider::Deezer,
        PlaybackProvider::SoundCloud => crate::search::Provider::SoundCloud,
    }
}

fn current_artist_navigation(
    track: &PlaybackTrack,
    status: PlaybackStatus,
    error: Option<&str>,
    openers: Option<&ProviderNavigationOpeners>,
) -> Option<TrackArtistNavigation> {
    if error.is_some()
        || !matches!(
            status,
            PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
        )
    {
        return None;
    }
    let provider = search_provider_for_playback(track.provider);
    let (_, opener) = openers?.for_provider(provider);
    let routes = artist_routes_for_track(provider, &track.artists);
    (!routes.is_empty()).then_some(TrackArtistNavigation {
        provider,
        routes,
        opener,
    })
}

fn download_available(track: Option<&PlaybackTrack>) -> bool {
    track.is_some()
}

fn update_last_quality_label(last_quality_label: &mut String, resolved_quality: Option<&str>) {
    if let Some(quality) = resolved_quality.filter(|quality| !quality.is_empty()) {
        *last_quality_label = quality.to_owned();
    }
}

fn update_quality_label_for_generation(
    last_label: &mut String,
    last_generation: &mut Option<u64>,
    generation: u64,
    resolved_quality: Option<&str>,
) {
    let generation_changed = *last_generation != Some(generation);
    *last_generation = Some(generation);
    if generation_changed {
        last_label.clear();
    }
    update_last_quality_label(last_label, resolved_quality);
}

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

fn current_block_width(layout: PlayerBarLayout) -> f32 {
    // The current-track column owns the whole side column. Capping the wide
    // layout here made the title ellipsize while unused width remained beside
    // it.
    layout.side_width
}

fn quality_text_animation_key(quality: &str) -> String {
    format!("player-quality-text-{quality}")
}

fn quality_text_opacity_endpoints(previous_label: &str, displayed_label: &str) -> (f32, f32) {
    if displayed_label.is_empty() {
        (0., 0.)
    } else if previous_label == displayed_label {
        (1., 1.)
    } else {
        (0., 1.)
    }
}

fn quality_badge_opacity_endpoints(previous_label: &str, displayed_label: &str) -> (f32, f32) {
    if displayed_label.is_empty() {
        (0., 0.)
    } else if previous_label.is_empty() {
        (0., 1.)
    } else {
        (1., 1.)
    }
}

fn quality_badge_animation_key(generation: u64, quality: &str) -> String {
    format!("player-quality-badge-{generation}-{quality}")
}

fn animated_quality_badge(
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

fn quality_badge_shell(max_width: f32) -> gpui::Stateful<gpui::Div> {
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

fn quality_badge_width(window: &Window, quality: &str) -> f32 {
    (text_measure_width(window, quality, 11., FontWeight::SEMIBOLD) + 16.)
        .clamp(QUALITY_BADGE_MIN_WIDTH_PX, 260.)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RightControlGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RightControlKind {
    Quality,
    Lyrics,
    Queue,
    Volume,
}

impl RightControlKind {
    fn slot(self) -> usize {
        match self {
            Self::Quality => 0,
            Self::Lyrics => 1,
            Self::Queue => 2,
            Self::Volume => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ScalarMotionVisual {
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ScalarMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl ScalarMotion {
    fn prepare(
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

    fn displayed_at(&self, now: Instant) -> f32 {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            crate::motion::lerp(self.from, self.target, panel_motion_ease(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
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
struct RectMotionVisual {
    from: RightControlGeometry,
    target: RightControlGeometry,
    epoch: u64,
    active: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct RectMotion {
    initialized: bool,
    from: RightControlGeometry,
    target: RightControlGeometry,
    epoch: u64,
    started_at: Option<Instant>,
}

impl RectMotion {
    fn prepare(
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

    fn displayed_at(&self, now: Instant) -> RightControlGeometry {
        let progress = self.animation_progress(now);
        if self.started_at.is_none() || progress >= 1. {
            self.target
        } else {
            lerp_right_control_geometry(self.from, self.target, panel_motion_ease(progress))
        }
    }

    fn animation_progress(&self, now: Instant) -> f32 {
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
struct FadeMotionVisual {
    from: RightControlGeometry,
    target: RightControlGeometry,
    from_opacity: f32,
    target_opacity: f32,
    epoch: u64,
    started_at: Option<Instant>,
    active: bool,
}

/// Relocating controls use a two-phase fade instead of visible geometry
/// interpolation. The source box remains in place while it fades out, the
/// target box is selected at the midpoint, and it then fades back in.
///
/// `compact` is the only event that starts a fade. A viewport resize during an
/// existing mode transition updates the target geometry in place, preserving
/// the phase and animation identity instead of restarting the fade.
#[derive(Clone, Copy, Debug, Default)]
struct FadeMotion {
    compact: Option<bool>,
    initialized: bool,
    from: RightControlGeometry,
    target: RightControlGeometry,
    from_opacity: f32,
    target_opacity: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl FadeMotion {
    fn prepare(
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

    fn displayed_at(&self, now: Instant) -> (RightControlGeometry, f32) {
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

    fn animation_progress(&self, now: Instant) -> f32 {
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

fn player_right_controls(
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

fn right_control_geometry(
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

fn wide_right_group_left(side_width: f32, quality_width: f32) -> f32 {
    let inner_width = (side_width - 8.).max(0.);
    let gap = 12.;
    let group_width = quality_width + 34. + 34. + 34. + 120. + 14. + gap * 4.;
    (inner_width - group_width).max(0.)
}

fn lerp_right_control_geometry(
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

fn fade_motion_opacity(from: f32, target: f32, delta: f32) -> f32 {
    let delta = delta.clamp(0., 1.);
    if delta < 0.5 {
        crate::motion::lerp(from, 0., panel_motion_ease(delta * 2.))
    } else {
        crate::motion::lerp(0., target, panel_motion_ease((delta - 0.5) * 2.))
    }
}

fn fade_motion_geometry(
    from: RightControlGeometry,
    target: RightControlGeometry,
    delta: f32,
) -> RightControlGeometry {
    if delta < 0.5 { from } else { target }
}

fn fade_motion_progress(started_at: Instant, now: Instant) -> f32 {
    if crate::motion::PANEL_DURATION.is_zero() {
        return 1.;
    }
    (now.saturating_duration_since(started_at).as_secs_f32()
        / crate::motion::PANEL_DURATION.as_secs_f32())
    .clamp(0., 1.)
}

fn right_control(element: AnyElement, id: &'static str, visual: RectMotionVisual) -> AnyElement {
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

fn right_control_fade(
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

fn download_position(
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

fn download_position_for_quality_width(
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

fn compact_player_right_geometry(
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

fn wide_volume_inset(wide: bool) -> f32 {
    if wide { WIDE_VOLUME_INSET_PX } else { 0. }
}

fn volume_container_offsets(wide: bool) -> (f32, f32) {
    (if wide { 14. } else { -2. }, wide_volume_inset(wide))
}

fn volume_container_offset_target(wide: bool) -> RightControlGeometry {
    let (left, top) = volume_container_offsets(wide);
    RightControlGeometry {
        left,
        top,
        width: 0.,
        height: 0.,
    }
}

#[allow(clippy::too_many_arguments)]
fn render_current(
    track: Option<&PlaybackTrack>,
    status: PlaybackStatus,
    error: Option<&str>,
    loading_from_cache: bool,
    desktop_layout: Option<PlayerBarLayout>,
    compact: bool,
    narrow: bool,
    favorite: Option<AnyElement>,
    width_animation: Option<(f32, f32, u64)>,
    artist_navigation: Option<TrackArtistNavigation>,
    text_width_visual: Option<ScalarMotionVisual>,
    favorite_visual: Option<FadeMotionVisual>,
    favorite_interactive: Option<Rc<Cell<bool>>>,
    window: &mut Window,
) -> AnyElement {
    let block_width = desktop_layout.map_or(380., |layout| layout.side_width);
    let artwork = if narrow { 44. } else { 60. };
    let title = track.map_or("Nothing playing", |track| track.title.as_str());
    let subtitle = current_subtitle(track, status, error, loading_from_cache);
    let rendered_artist =
        rendered_current_artist_text(track, status, error, subtitle, artist_navigation.as_ref());
    // Fixed side widths only on desktop layouts; narrow rows stretch, so there
    // is no cheap width to compare against for overflow tooltips there. The
    // compact heart lives outside this row and does not reserve text width.
    let text_width = (!narrow).then(|| {
        current_text_available_width(
            desktop_layout
                .unwrap_or_else(|| PlayerBarLayout::from_geometry((0., 0., 0., block_width))),
            compact,
            favorite.is_some(),
        )
    });
    let text_block_width = text_width_visual.map(|visual| visual.target);
    let artist_line = if error.is_none()
        && matches!(
            status,
            PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
        ) {
        track
            .map(|track| {
                crate::music_ui::track_artist_line(
                    0,
                    "player-bar",
                    &track.artist,
                    artist_navigation,
                    false,
                )
            })
            .unwrap_or_else(|| div().child(subtitle.to_owned()).into_any_element())
    } else {
        div().child(subtitle.to_owned()).into_any_element()
    };
    let overflow_tooltip =
        |el: gpui::Stateful<gpui::Div>, text: &str, fits: Option<bool>| match fits {
            Some(false) => {
                let text = text.to_owned();
                el.app_tooltip(text)
            }
            _ => el,
        };
    let text_block = div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(overflow_tooltip(
            div()
                .id("player-title")
                .truncate()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
            title,
            text_width.map(|width| text_fits(window, title, 14., FontWeight::SEMIBOLD, px(width))),
        ))
        .child(overflow_tooltip(
            div()
                .id("player-artist")
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(if error.is_some() { DANGER } else { MUTED }))
                .child(artist_line),
            rendered_artist.as_str(),
            text_width.map(|width| {
                text_fits(
                    window,
                    rendered_artist.as_str(),
                    12.,
                    FontWeight::NORMAL,
                    px(width),
                )
            }),
        ));
    let text_block = if let Some(width) = text_block_width {
        let target_width = width;
        let from_width = text_width_visual
            .filter(|visual| visual.active)
            .map(|visual| visual.from);
        let initial_width = from_width.unwrap_or(target_width);
        let text_block = text_block
            .w(px(initial_width))
            .min_w(px(initial_width))
            .flex_none();
        if let (Some(visual), Some(from_width)) =
            (text_width_visual.filter(|visual| visual.active), from_width)
        {
            text_block
                .with_animation(
                    ("player-current-text-layout", visual.epoch),
                    crate::motion::panel(),
                    move |this, delta| {
                        let width = crate::motion::lerp(from_width, target_width, delta);
                        this.w(px(width)).min_w(px(width))
                    },
                )
                .into_any_element()
        } else {
            text_block.into_any_element()
        }
    } else if narrow || compact {
        text_block.flex_1().into_any_element()
    } else {
        text_block.flex_initial().into_any_element()
    };
    let current = div()
        .relative()
        .when(narrow, |this| this.w_full())
        .when(!narrow, |this| this.w(px(block_width)))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .size(px(artwork))
                .flex_none()
                .overflow_hidden()
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(BORDER))
                .bg(rgb(SURFACE))
                .when_some(
                    track.filter(|track| !track.artwork.is_empty()),
                    |this, track| {
                        this.child(
                            img(track.artwork.clone())
                                .id("player-bar-artwork")
                                .size_full()
                                .rounded(px(6.))
                                .object_fit(ObjectFit::Cover),
                        )
                    },
                ),
        )
        .child(text_block);
    let current = if narrow {
        if let Some(interactive) = favorite_interactive {
            interactive.set(true);
        }
        current.children(favorite)
    } else if let (Some(visual), Some(button)) = (favorite_visual, favorite) {
        let favorite = div()
            .id("player-favorite-layout")
            .absolute()
            .top(px(visual.target.top))
            .left(px(visual.target.left))
            .size(px(34.))
            .child(button);
        let favorite: AnyElement = if visual.active {
            let interactive =
                favorite_interactive.expect("favorite fade should have an interaction gate");
            favorite
                .top(px(visual.from.top))
                .left(px(visual.from.left))
                .opacity(visual.from_opacity)
                .with_animation(
                    ("player-favorite-layout", visual.epoch),
                    crate::motion::relocation_fade(),
                    move |this, delta| {
                        let delta = visual.started_at.map_or(delta, |started_at| {
                            fade_motion_progress(started_at, Instant::now())
                        });
                        interactive.set(delta >= 1.);
                        let geometry = fade_motion_geometry(visual.from, visual.target, delta);
                        this.left(px(geometry.left)).top(px(geometry.top)).opacity(
                            fade_motion_opacity(visual.from_opacity, visual.target_opacity, delta),
                        )
                    },
                )
                .into_any_element()
        } else {
            if let Some(interactive) = favorite_interactive {
                interactive.set(true);
            }
            favorite.into_any_element()
        };
        current.child(favorite)
    } else {
        current
    };
    if let Some((from_width, target_width, epoch)) = width_animation {
        current
            .with_animation(
                ("player-current-width", epoch),
                crate::motion::panel(),
                move |this, delta| {
                    let width = crate::motion::lerp(from_width, target_width, delta);
                    this.w(px(width)).min_w(px(width))
                },
            )
            .into_any_element()
    } else {
        current.into_any_element()
    }
}

fn current_subtitle<'a>(
    track: Option<&'a PlaybackTrack>,
    status: PlaybackStatus,
    error: Option<&'a str>,
    loading_from_cache: bool,
) -> &'a str {
    if let Some(err) = error {
        err
    } else {
        match status {
            PlaybackStatus::Loading if !loading_from_cache => "Loading audio...",
            PlaybackStatus::Loading => track.map_or("", |track| track.artist.as_str()),
            PlaybackStatus::Failed => "Playback failed",
            _ => track.map_or("", |track| track.artist.as_str()),
        }
    }
}

fn rendered_current_artist_text(
    track: Option<&PlaybackTrack>,
    status: PlaybackStatus,
    error: Option<&str>,
    subtitle: &str,
    navigation: Option<&TrackArtistNavigation>,
) -> String {
    if error.is_some()
        || !matches!(
            status,
            PlaybackStatus::Playing | PlaybackStatus::Paused | PlaybackStatus::Ended
        )
    {
        return subtitle.to_owned();
    }
    let Some(track) = track else {
        return subtitle.to_owned();
    };
    let Some(navigation) = navigation else {
        return track.artist.clone();
    };
    rendered_artist_text(&track.artist, navigation.provider, &navigation.routes)
}

fn rendered_artist_text(
    artist: &str,
    provider: crate::search::Provider,
    routes: &[crate::entity_navigation::MenuRoute],
) -> String {
    if provider == crate::search::Provider::SoundCloud && routes.len() == 1 {
        artist.to_owned()
    } else {
        routes
            .iter()
            .map(|route| route.title.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn current_text_available_width(layout: PlayerBarLayout, compact: bool, has_favorite: bool) -> f32 {
    let favorite_reservation = (!compact && has_favorite)
        .then_some(34. + 12.)
        .unwrap_or(0.);
    (layout.side_width - 60. - 12. - favorite_reservation).max(0.)
}

fn current_text_intrinsic_width(window: &Window, title: &str, artist: &str) -> f32 {
    text_measure_width(window, title, 14., FontWeight::SEMIBOLD).max(text_measure_width(
        window,
        artist,
        12.,
        FontWeight::NORMAL,
    ))
}

fn current_text_width_for_layout(
    layout: PlayerBarLayout,
    compact: bool,
    has_favorite: bool,
    intrinsic_width: f32,
) -> f32 {
    let available = current_text_available_width(layout, compact, has_favorite);
    if compact {
        available
    } else {
        intrinsic_width.max(0.).min(available)
    }
}

fn favorite_top(compact: bool) -> f32 {
    if compact { 2. } else { 13. }
}

fn favorite_left_for_layout(layout: PlayerBarLayout, compact: bool, intrinsic_width: f32) -> f32 {
    let text_width = current_text_width_for_layout(layout, compact, true, intrinsic_width);
    favorite_left_for_text_width(layout, compact, text_width)
}

fn favorite_left_for_text_width(layout: PlayerBarLayout, compact: bool, text_width: f32) -> f32 {
    if compact {
        (layout.side_width + layout.column_gap - 1.).max(0.)
    } else {
        60. + 12. + text_width.max(0.) + 12.
    }
}

fn text_measure_width(window: &Window, text: &str, font_size: f32, weight: FontWeight) -> f32 {
    if text.is_empty() {
        return 0.;
    }
    let font = Font {
        family: ui_font_family().into(),
        weight,
        ..Font::default()
    };
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text.into(), px(font_size), &[run], None)
        .width
        .into()
}

/// Cheap single-line text measurement against an available width, used to
/// attach tooltips only to overflowing titles and artists.
fn text_fits(
    window: &Window,
    text: &str,
    font_size: f32,
    weight: FontWeight,
    available: Pixels,
) -> bool {
    if text.is_empty() {
        return true;
    }
    px(text_measure_width(window, text, font_size, weight)) <= available
}

fn hover_icon(
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

fn transport_button(
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
fn repeat_button(
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
fn volume_button(model: Entity<PlaybackModel>, level: VolumeIconLevel) -> AnyElement {
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

fn volume_icon(level: VolumeIconLevel) -> AnyElement {
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

fn volume_icon_svg(
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

fn volume_icon_speaker_offset(level: VolumeIconLevel) -> f32 {
    let speaker_start_viewbox = match level {
        VolumeIconLevel::High => 32.,
        VolumeIconLevel::Low | VolumeIconLevel::Off | VolumeIconLevel::Muted => 0.,
    };
    -VOLUME_ICON_EM_HEIGHT_PX * speaker_start_viewbox / 512.
}

fn volume_icon_dimensions(level: VolumeIconLevel) -> (f32, f32) {
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

struct VolumePointerPaintState {
    hitbox: Hitbox,
    logical_bounds: Bounds<Pixels>,
    displayed_value: f32,
}

fn register_volume_pointer_handlers(
    paint: VolumePointerPaintState,
    model: Entity<PlaybackModel>,
    state: Rc<Cell<VolumePointerState>>,
    window: &mut Window,
) {
    let down_hitbox = paint.hitbox.clone();
    let down_displayed_value = paint.displayed_value;
    let down_state = state.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        if event.button == MouseButton::Left && down_hitbox.is_hovered(window) {
            down_state.set(VolumePointerState::pending(
                f32::from(event.position.x),
                f32::from(event.position.y),
                down_displayed_value,
            ));
            window.capture_pointer(down_hitbox.id);
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        }
    });

    let move_model = model.clone();
    let move_state = state.clone();
    let move_bounds = paint.logical_bounds;
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        let mut pointer_state = move_state.get();
        if event.pressed_button == Some(MouseButton::Left) && pointer_state.is_active() {
            let dragging = pointer_state
                .update_move(f32::from(event.position.x), f32::from(event.position.y))
                || matches!(pointer_state.phase, VolumePointerPhase::Dragging);
            move_state.set(pointer_state);
            if dragging {
                let volume = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
                move_model.update(cx, |model, cx| model.set_volume(volume, cx));
            }
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        } else if event.pressed_button.is_none() && pointer_state.is_active() {
            if finish_volume_pointer_interaction(&move_state).is_some() {
                let volume = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
                move_model.update(cx, |model, cx| model.set_volume(volume, cx));
                window.release_pointer();
                window.prevent_default();
                cx.stop_propagation();
                window.refresh();
            } else if matches!(pointer_state.phase, VolumePointerPhase::DirectRelease) {
                // The final direct update was already consumed. Release the
                // capture without writing the same value a second time.
                window.release_pointer();
                window.prevent_default();
                cx.stop_propagation();
            }
        }
    });

    let up_model = model;
    let up_state = state;
    let up_bounds = paint.logical_bounds;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if !phase.capture() {
            return;
        }
        if event.button == MouseButton::Left {
            let released = finish_volume_pointer_interaction(&up_state);
            if let Some(_) = released {
                let volume = pointer_seek_fraction(f32::from(event.position.x), up_bounds);
                up_model.update(cx, |model, cx| model.set_volume(volume, cx));
            } else if !matches!(up_state.get().phase, VolumePointerPhase::DirectRelease) {
                return;
            }
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
            window.refresh();
        }
    });
}

fn volume_controls(
    model: Entity<PlaybackModel>,
    slider: Entity<SliderState>,
    pointer_state: Rc<Cell<VolumePointerState>>,
    level: VolumeIconLevel,
    visual: VolumeVisual,
    displayed_value: f32,
    offset_visual: RectMotionVisual,
) -> AnyElement {
    let pointer_state_for_canvas = pointer_state.clone();
    let thumb_inset = VOLUME_THUMB_DIAMETER_PX * 0.5;
    let container = div()
        .id("volume-container")
        .w_full()
        .min_w_0()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(VOLUME_GAP_PX))
        .child(volume_button(model.clone(), level))
        .child(
            div()
                .id("volume-slider")
                .w(px(VOLUME_SLIDER_WIDTH_PX))
                .h(px(VOLUME_SLIDER_CONTROL_HEIGHT_PX))
                .flex_none()
                .relative()
                .cursor_pointer()
                .child(volume_track(visual))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .overflow_hidden()
                        .cursor_pointer()
                        .child(Slider::new(&slider).horizontal().opacity(0.)),
                )
                .child(
                    canvas(
                        move |bounds, window, _cx| VolumePointerPaintState {
                            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
                            logical_bounds: volume_logical_bounds(bounds),
                            displayed_value,
                        },
                        move |_bounds, paint, window, _cx| {
                            register_volume_pointer_handlers(
                                paint,
                                model.clone(),
                                pointer_state_for_canvas.clone(),
                                window,
                            );
                        },
                    )
                    .cursor_pointer()
                    .absolute()
                    .left(px(-thumb_inset))
                    .right(px(-thumb_inset))
                    .top_0()
                    .bottom_0(),
                ),
        );
    let target_offsets = (offset_visual.target.left, offset_visual.target.top);
    if offset_visual.active {
        container
            .ml(px(offset_visual.from.left))
            .relative()
            .right(px(offset_visual.from.top))
            .with_animation(
                ("player-volume-container-layout", offset_visual.epoch),
                crate::motion::panel(),
                move |this, delta| {
                    this.ml(px(crate::motion::lerp(
                        offset_visual.from.left,
                        target_offsets.0,
                        delta,
                    )))
                    .right(px(crate::motion::lerp(
                        offset_visual.from.top,
                        target_offsets.1,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        container
            .ml(px(target_offsets.0))
            .relative()
            .right(px(target_offsets.1))
            .into_any_element()
    }
}

fn volume_track(visual: VolumeVisual) -> AnyElement {
    let volume_fill = div()
        .absolute()
        .left_0()
        .top_0()
        .bottom_0()
        .w(relative(visual.from))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    let volume_thumb = div()
        .absolute()
        .left(relative(visual.from))
        .top(px((VOLUME_TRACK_HEIGHT_PX - VOLUME_THUMB_DIAMETER_PX) * 0.5))
        .ml(px(-VOLUME_THUMB_DIAMETER_PX * 0.5))
        .size(px(VOLUME_THUMB_DIAMETER_PX))
        .rounded_full()
        .bg(rgb(PRIMARY))
        .cursor_pointer();
    let volume_fill = if visual.active {
        volume_fill
            .with_animation(
                format!("player-volume-fill-{}", visual.epoch),
                crate::motion::interaction(),
                move |this, delta| {
                    this.w(relative(crate::motion::lerp(
                        visual.from,
                        visual.target,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        volume_fill.into_any_element()
    };
    let volume_thumb = if visual.active {
        volume_thumb
            .with_animation(
                format!("player-volume-thumb-{}", visual.epoch),
                crate::motion::interaction(),
                move |this, delta| {
                    this.left(relative(crate::motion::lerp(
                        visual.from,
                        visual.target,
                        delta,
                    )))
                },
            )
            .into_any_element()
    } else {
        volume_thumb.into_any_element()
    };

    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((VOLUME_SLIDER_CONTROL_HEIGHT_PX
            - VOLUME_TRACK_HEIGHT_PX)
            * 0.5))
        .h(px(VOLUME_TRACK_HEIGHT_PX))
        .rounded_full()
        .bg(rgb(SCROLLBAR_THUMB))
        .cursor_pointer()
        .child(volume_fill)
        .child(volume_thumb)
        .into_any_element()
}

fn close_player_tooltip_gap(compact: bool) -> f32 {
    if compact {
        CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX
    } else {
        CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX
    }
}

fn close_button(
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

fn play_pause_button(
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
fn bare_action_button(
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
fn bare_action_button_with_interactivity(
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

fn bare_action_visual(selected: bool, selected_accent: Option<u32>) -> (u32, u32, bool) {
    if !selected {
        return (MUTED, FOREGROUND, false);
    }
    if let Some(accent) = selected_accent {
        (accent, accent, false)
    } else {
        (FOREGROUND, FOREGROUND, true)
    }
}

fn favorite_feedback_opacity(pending: bool) -> (f32, f32) {
    if pending { (1., 1.) } else { (0.82, 1.) }
}

fn local_icon_svg(path: &'static str, size: f32, color: u32) -> gpui::Svg {
    gpui::svg().path(path).text_color(rgb(color)).size(px(size))
}

fn local_icon_svg_sized(path: &'static str, width: f32, height: f32, color: u32) -> gpui::Svg {
    gpui::svg()
        .path(path)
        .text_color(rgb(color))
        .w(px(width))
        .h(px(height))
}

fn seekbar_display_progress(playback_progress: f32, preview: Option<f32>) -> f32 {
    preview.unwrap_or(playback_progress).clamp(0.0, 1.0)
}

fn accessibility_seek_fraction(current: f32, delta: f32, enabled: bool) -> Option<f32> {
    enabled.then_some((current + delta).clamp(0.0, 1.0))
}

fn seek_accessibly(model: Entity<PlaybackModel>, delta: f32, enabled: bool, cx: &mut App) {
    let current = {
        let state = &model.read(cx).state;
        if !matches!(
            state.status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        ) || state.duration.is_zero()
        {
            return;
        }
        state.position.as_secs_f32() / state.duration.as_secs_f32()
    };
    let Some(next) = accessibility_seek_fraction(current, delta, enabled) else {
        return;
    };
    model.update(cx, |model, cx| model.seek_fraction(next, cx));
}

struct SeekPointerPaintState {
    hitbox: Hitbox,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SeekPointerState {
    active: bool,
    moved: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum VolumePointerPhase {
    #[default]
    Idle,
    PendingClick {
        press_x: f32,
        press_y: f32,
        held_value: f32,
    },
    Dragging,
    DirectRelease,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct VolumePointerState {
    phase: VolumePointerPhase,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VolumePointerRelease {
    PendingClick,
    Dragging,
}

const VOLUME_DRAG_THRESHOLD_PX: f32 = 3.;

impl VolumePointerState {
    fn pending(press_x: f32, press_y: f32, held_value: f32) -> Self {
        Self {
            phase: VolumePointerPhase::PendingClick {
                press_x,
                press_y,
                held_value: held_value.clamp(0., 1.),
            },
        }
    }

    fn is_active(self) -> bool {
        !matches!(self.phase, VolumePointerPhase::Idle)
    }

    fn update_move(&mut self, pointer_x: f32, pointer_y: f32) -> bool {
        let VolumePointerPhase::PendingClick {
            press_x, press_y, ..
        } = self.phase
        else {
            return false;
        };
        let dx = pointer_x - press_x;
        let dy = pointer_y - press_y;
        if dx * dx + dy * dy < VOLUME_DRAG_THRESHOLD_PX * VOLUME_DRAG_THRESHOLD_PX {
            return false;
        }
        self.phase = VolumePointerPhase::Dragging;
        true
    }

    fn finish(&mut self) -> Option<VolumePointerRelease> {
        let release = match self.phase {
            VolumePointerPhase::PendingClick { .. } => VolumePointerRelease::PendingClick,
            VolumePointerPhase::Dragging => VolumePointerRelease::Dragging,
            VolumePointerPhase::DirectRelease | VolumePointerPhase::Idle => return None,
        };
        self.phase = if release == VolumePointerRelease::Dragging {
            VolumePointerPhase::DirectRelease
        } else {
            VolumePointerPhase::Idle
        };
        Some(release)
    }

    fn motion_mode(self) -> VolumeMotionMode {
        match self.phase {
            VolumePointerPhase::PendingClick { held_value, .. } => {
                VolumeMotionMode::Hold(held_value)
            }
            VolumePointerPhase::Dragging | VolumePointerPhase::DirectRelease => {
                VolumeMotionMode::Direct
            }
            VolumePointerPhase::Idle => VolumeMotionMode::Animated,
        }
    }
}

fn finish_volume_pointer_interaction(
    state: &Cell<VolumePointerState>,
) -> Option<VolumePointerRelease> {
    let mut pointer_state = state.get();
    if !pointer_state.is_active() {
        return None;
    }
    let release = pointer_state.finish();
    state.set(pointer_state);
    release
}

fn volume_motion_mode_for_render(state: &Cell<VolumePointerState>) -> VolumeMotionMode {
    let pointer_state = state.get();
    let mode = pointer_state.motion_mode();
    if pointer_state.phase == VolumePointerPhase::DirectRelease {
        state.set(VolumePointerState::default());
    }
    mode
}

fn volume_logical_bounds(expanded_bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let inset = px(VOLUME_THUMB_DIAMETER_PX * 0.5);
    Bounds {
        origin: point(expanded_bounds.origin.x + inset, expanded_bounds.origin.y),
        size: size(
            (expanded_bounds.size.width - inset * 2.).max(px(0.)),
            expanded_bounds.size.height,
        ),
    }
}

fn pointer_seek_fraction(pointer_x: f32, bounds: Bounds<Pixels>) -> f32 {
    let width = f32::from(bounds.size.width);
    if width <= 0. {
        return 0.;
    }
    ((pointer_x - f32::from(bounds.origin.x)) / width).clamp(0., 1.)
}

fn register_seek_pointer_handlers(
    paint: SeekPointerPaintState,
    model: Entity<PlaybackModel>,
    state: Rc<Cell<SeekPointerState>>,
    window: &mut Window,
) {
    let down_hitbox = paint.hitbox.clone();
    let down_model = model.clone();
    let down_state = state.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        if event.button == MouseButton::Left && down_hitbox.is_hovered(window) {
            down_state.set(SeekPointerState {
                active: true,
                moved: false,
            });
            down_model.update(cx, |model, _| model.begin_new_seek_pointer_interaction());
            window.capture_pointer(down_hitbox.id);
            window.prevent_default();
            cx.stop_propagation();
        }
    });

    let move_hitbox = paint.hitbox.clone();
    let move_model = model.clone();
    let move_state = state.clone();
    let move_bounds = move_hitbox.bounds;
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        let mut pointer_state = move_state.get();
        if pointer_state.active && event.pressed_button == Some(MouseButton::Left) {
            pointer_state.moved = true;
            move_state.set(pointer_state);
            let fraction = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
            move_model.update(cx, |model, cx| model.preview_seek_fraction(fraction, cx));
            cx.stop_propagation();
        } else if pointer_state.active && event.pressed_button.is_none() {
            move_state.set(SeekPointerState::default());
            let fraction = pointer_seek_fraction(f32::from(event.position.x), move_bounds);
            move_model.update(cx, |model, cx| model.commit_seek_fraction(fraction, cx));
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
        }
    });

    let up_hitbox = paint.hitbox.clone();
    let up_model = model.clone();
    let up_state = state;
    let up_bounds = up_hitbox.bounds;
    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
        if !phase.bubble() {
            return;
        }
        let pointer_state = up_state.get();
        if event.button == MouseButton::Left && pointer_state.active {
            up_state.set(SeekPointerState::default());
            let fraction = pointer_seek_fraction(f32::from(event.position.x), up_bounds);
            up_model.update(cx, |model, cx| model.commit_seek_fraction(fraction, cx));
            window.release_pointer();
            window.prevent_default();
            cx.stop_propagation();
        }
    });
}

fn progress_control(
    model: Entity<PlaybackModel>,
    state: Rc<Cell<SeekPointerState>>,
    buffered: BufferedVisual,
    seek_fill: SeekFillVisual,
    enabled: bool,
    cx: &mut Context<PlaybackView>,
) -> AnyElement {
    // Keep the component slider for pointer interaction and paint the
    // four-pixel track separately so no thumb is visible.
    div()
        .id("playback-progress")
        .flex_1()
        .h(px(24.))
        .relative()
        .when(enabled, {
            let model = model.clone();
            move |this| {
                this.role(Role::Slider)
                    .aria_numeric_value(seek_fill.target as f64)
                    .aria_min_numeric_value(0.)
                    .aria_max_numeric_value(1.)
                    .aria_numeric_value_step(SEEK_SLIDER_STEP as f64)
                    .focusable()
                    .tab_stop(true)
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .focus_visible(|style| style.border_color(rgb(PRIMARY)))
                    .cursor_pointer()
                    .on_a11y_action(AccessibleAction::Increment, {
                        let model = model.clone();
                        move |_, _, cx| seek_accessibly(model.clone(), SEEK_SLIDER_STEP, true, cx)
                    })
                    .on_a11y_action(AccessibleAction::Decrement, move |_, _, cx| {
                        seek_accessibly(model.clone(), -SEEK_SLIDER_STEP, true, cx)
                    })
            }
        })
        .child(
            div()
                .id("playback-track")
                .absolute()
                .top(px(10.))
                .left_0()
                .right_0()
                .h(px(4.))
                .rounded_full()
                .bg(rgb(BORDER))
                .when(buffered.target > 0.0 || buffered.active, move |this| {
                    let buffered_bar = div()
                        .id("playback-buffered")
                        .absolute()
                        .top_0()
                        .left_0()
                        .bottom_0()
                        .w(relative(buffered.from))
                        .rounded_full()
                        .bg(rgba(0x94a3b261))
                        .with_animation(
                            format!(
                                "playback-buffered-{}-{}",
                                buffered.generation, buffered.epoch
                            ),
                            crate::motion::content(),
                            move |this, delta| {
                                this.w(relative(crate::motion::lerp(
                                    buffered.from,
                                    buffered.target,
                                    delta,
                                )))
                            },
                        );
                    this.child(buffered_bar)
                })
                .when(seek_fill.target > 0.0 || seek_fill.active, move |this| {
                    let playback_fill = if seek_fill.active {
                        div()
                            .id("playback-fill")
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .w(relative(seek_fill.from))
                            .rounded_full()
                            .bg(rgb(FOREGROUND))
                            .with_animation(
                                format!("playback-seek-fill-{}", seek_fill.epoch),
                                crate::motion::interaction(),
                                move |this, delta| {
                                    this.w(relative(crate::motion::lerp(
                                        seek_fill.from,
                                        seek_fill.target,
                                        delta,
                                    )))
                                },
                            )
                            .into_any_element()
                    } else {
                        div()
                            .id("playback-fill")
                            .absolute()
                            .top_0()
                            .left_0()
                            .bottom_0()
                            .w(relative(seek_fill.from))
                            .rounded_full()
                            .bg(rgb(FOREGROUND))
                            .into_any_element()
                    };
                    this.child(playback_fill)
                }),
        )
        .when(enabled, {
            let model = model.clone();
            move |this| {
                this.child(
                    canvas(
                        move |bounds, window, _cx| SeekPointerPaintState {
                            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        },
                        move |_bounds, paint, window, _cx| {
                            register_seek_pointer_handlers(
                                paint,
                                model.clone(),
                                state.clone(),
                                window,
                            );
                        },
                    )
                    .absolute()
                    .inset_0()
                    .cursor_pointer(),
                )
            }
        })
        .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
            let delta = match event.keystroke.key.as_str() {
                "left" => -SEEK_SLIDER_STEP,
                "right" => SEEK_SLIDER_STEP,
                _ => return,
            };
            let current = {
                let state = &model.read(cx).state;
                if !matches!(
                    state.status,
                    PlaybackStatus::Playing | PlaybackStatus::Paused
                ) || state.duration.is_zero()
                {
                    return;
                }
                state.position.as_secs_f32() / state.duration.as_secs_f32()
            };
            model.update(cx, |model, cx| model.seek_fraction(current + delta, cx));
        }))
        .into_any_element()
}

fn time_text(duration: std::time::Duration) -> AnyElement {
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

#[cfg(test)]
mod tests {
    use super::{
        CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX, CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX,
        CLOSE_PLAYER_RIGHT_PX, FAVORITE_PINK, FadeMotion, PLAYER_ACTION_BUTTON_RADIUS_PX,
        PLAYER_ACTION_DISABLED_OPACITY, PLAYER_BAR_DESKTOP_HEIGHT_PX, PLAYER_CLOSE_GLYPH_PX,
        PlayerBarLayout, PlayerBarLayoutMotion, PlayerBarMotion, QUALITY_BADGE_HEIGHT_PX,
        QUALITY_BADGE_MIN_WIDTH_PX, QUALITY_BADGE_TEXT_OFFSET_PX, REPEAT_CONTROL_SIZE_PX,
        REPEAT_ONE_BADGE_BOTTOM_PX, REPEAT_ONE_BADGE_RIGHT_PX, RectMotion, RightControlGeometry,
        ScalarMotion, SeekFillMotion, SeekPointerState, VOLUME_DRAG_THRESHOLD_PX, VOLUME_GAP_PX,
        VOLUME_ICON_EM_HEIGHT_PX, VOLUME_ICON_FRAME_PX, VOLUME_MUTE_BUTTON_PX,
        VOLUME_SLIDER_CONTROL_HEIGHT_PX, VOLUME_SLIDER_WIDTH_PX, VOLUME_THUMB_DIAMETER_PX,
        VOLUME_TRACK_HEIGHT_PX, VolumeIconLevel, VolumeMotion, VolumeMotionMode,
        VolumePointerPhase, VolumePointerRelease, VolumePointerState, WIDE_VOLUME_INSET_PX,
        accessibility_seek_fraction, artist_routes_for_track, bare_action_visual,
        close_player_tooltip_gap, current_block_width, current_favorite_key, current_subtitle,
        current_text_available_width, current_text_width_for_layout, desktop_player_geometry,
        download_available, fade_motion_geometry, fade_motion_opacity, favorite_feedback_opacity,
        favorite_left_for_text_width, favorite_top, finish_volume_pointer_interaction,
        pointer_seek_fraction, quality_badge_animation_key, quality_badge_opacity_endpoints,
        quality_text_animation_key, quality_text_opacity_endpoints, rendered_artist_text,
        rendered_current_artist_text, search_provider_for_playback, seekbar_display_progress,
        update_last_quality_label, update_quality_label_for_generation, volume_container_offsets,
        volume_icon_dimensions, volume_icon_speaker_offset, volume_logical_bounds,
        volume_motion_mode_for_render, wide_volume_inset,
    };
    use crate::{
        entity_navigation::{MenuRoute, MenuRouteKind},
        library::{FavoriteKey, FavoriteKind},
        playback::{PlaybackProvider, PlaybackStatus, PlaybackTrack},
        search::{Provider, TrackArtistRef},
        theme::{FOREGROUND, MUTED},
    };
    use gpui::{Bounds, point, px, size};
    use std::{
        cell::Cell,
        rc::Rc,
        time::{Duration, Instant},
    };

    #[test]
    fn selected_player_favorite_uses_pink_without_panel_background() {
        assert_eq!(
            bare_action_visual(true, Some(FAVORITE_PINK)),
            (FAVORITE_PINK, FAVORITE_PINK, false)
        );
        assert_eq!(
            bare_action_visual(false, Some(FAVORITE_PINK)),
            (MUTED, FOREGROUND, false)
        );
        assert_eq!(
            bare_action_visual(true, None),
            (FOREGROUND, FOREGROUND, true)
        );
    }

    #[test]
    fn pending_player_favorite_dims_once_like_tracklist_favorites() {
        let (from, to) = favorite_feedback_opacity(true);
        assert_eq!((from, to), (1., 1.));
        assert_eq!(to * PLAYER_ACTION_DISABLED_OPACITY, 0.4);
    }

    #[test]
    fn current_track_actions_match_provider_availability() {
        let track = |provider| PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider,
            id: "42".into(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::ZERO,
            explicit: false,
            service_url: String::new(),
        };
        let deezer = track(PlaybackProvider::Deezer);
        let soundcloud = track(PlaybackProvider::SoundCloud);

        assert!(!download_available(None));
        assert!(download_available(Some(&deezer)));
        assert_eq!(
            current_favorite_key(Some(&deezer)).unwrap(),
            FavoriteKey::deezer(FavoriteKind::Track, "42".into())
        );
        assert_eq!(
            current_favorite_key(Some(&soundcloud)).unwrap(),
            FavoriteKey::soundcloud(FavoriteKind::Track, "42".into())
        );
        assert!(current_favorite_key(None).is_none());
    }

    #[test]
    fn seekbar_preview_wins_over_playback_progress_until_release() {
        assert_eq!(seekbar_display_progress(0.2, Some(0.8)), 0.8);
        assert_eq!(seekbar_display_progress(0.2, None), 0.2);
        assert_eq!(seekbar_display_progress(1.4, None), 1.0);
        assert_eq!(seekbar_display_progress(-0.4, Some(1.4)), 1.0);
    }

    #[test]
    fn seek_fill_retarget_starts_at_the_eased_displayed_midpoint() {
        let started_at = Instant::now();
        let halfway = started_at + crate::motion::INTERACTION_DURATION / 2;
        let mut motion = SeekFillMotion::default();

        let first = motion.prepare(1, 0., 1., started_at, false);
        assert!(first.active);

        let expected = gpui::ease_in_out(0.5);
        let second = motion.prepare(2, 1., 0., halfway, false);

        assert!(second.active);
        assert!((second.from - expected).abs() < 0.0001);
        assert!((second.from - 1.).abs() > 0.0001);
    }

    #[test]
    fn volume_motion_initializes_at_the_model_value() {
        let now = Instant::now();
        let mut motion = VolumeMotion::default();

        let visual = motion.prepare(0.4, now, false, VolumeMotionMode::Animated);

        assert_eq!(visual.from, 0.4);
        assert_eq!(visual.target, 0.4);
        assert!(!visual.active);
    }

    #[test]
    fn volume_motion_keeps_dragging_direct_and_handles_a_full_mute_jump() {
        let now = Instant::now();
        let mut motion = VolumeMotion::default();
        motion.prepare(1.0, now, false, VolumeMotionMode::Animated);

        let dragged = motion.prepare(
            0.35,
            now + Duration::from_millis(20),
            false,
            VolumeMotionMode::Direct,
        );
        assert_eq!(dragged.from, 0.35);
        assert_eq!(dragged.target, 0.35);
        assert!(!dragged.active);

        let clicked = motion.prepare(
            0.0,
            now + Duration::from_millis(40),
            false,
            VolumeMotionMode::Animated,
        );
        assert_eq!(clicked.from, 0.35);
        assert_eq!(clicked.target, 0.0);
        assert!(clicked.active);
    }

    #[test]
    fn pending_volume_click_holds_the_visible_value_until_release() {
        let started_at = Instant::now();
        let mut motion = VolumeMotion::default();
        motion.prepare(0.8, started_at, false, VolumeMotionMode::Animated);
        let held_value = motion.displayed_at(started_at);
        let mut pointer = VolumePointerState::pending(12., 18., held_value);

        let held = motion.prepare(
            0.15,
            started_at + Duration::from_millis(10),
            false,
            pointer.motion_mode(),
        );
        assert_eq!(held.from, held_value);
        assert_eq!(held.target, held_value);
        assert!(!held.active);

        assert_eq!(pointer.finish(), Some(VolumePointerRelease::PendingClick));
        let released = motion.prepare(
            0.15,
            started_at + Duration::from_millis(10),
            false,
            pointer.motion_mode(),
        );
        assert_eq!(released.from, held_value);
        assert_eq!(released.target, 0.15);
        assert!(released.active);
    }

    #[test]
    fn volume_pointer_crosses_threshold_once_and_enters_dragging() {
        let mut pointer = VolumePointerState::pending(20., 20., 0.5);

        assert!(!pointer.update_move(22., 21.));
        assert!(matches!(
            pointer.phase,
            VolumePointerPhase::PendingClick { .. }
        ));
        assert!(pointer.update_move(24., 20.));
        assert_eq!(pointer.phase, VolumePointerPhase::Dragging);
        assert!(!pointer.update_move(60., 20.));
        assert_eq!(pointer.phase, VolumePointerPhase::Dragging);
    }

    #[test]
    fn every_volume_drag_update_is_direct_and_release_is_direct_too() {
        let now = Instant::now();
        let mut motion = VolumeMotion::default();
        motion.prepare(0.9, now, false, VolumeMotionMode::Animated);
        let mut pointer = VolumePointerState::pending(0., 0., 0.9);
        assert!(pointer.update_move(VOLUME_DRAG_THRESHOLD_PX, 0.));

        for (offset, target) in [(2_u64, 0.65), (4, 0.35), (6, 0.05)] {
            let visual = motion.prepare(
                target,
                now + Duration::from_millis(offset),
                false,
                pointer.motion_mode(),
            );
            assert_eq!(visual.from, target);
            assert_eq!(visual.target, target);
            assert!(!visual.active);
        }

        assert_eq!(pointer.finish(), Some(VolumePointerRelease::Dragging));
        assert_eq!(pointer.phase, VolumePointerPhase::DirectRelease);
        let settled = motion.prepare(
            0.2,
            now + Duration::from_millis(8),
            false,
            pointer.motion_mode(),
        );
        assert_eq!(settled.from, 0.2);
        assert_eq!(settled.target, 0.2);
        assert!(!settled.active);
    }

    #[test]
    fn captured_volume_pointer_writes_drag_values_to_playback_model() {
        let source = include_str!("player_bar.rs");
        let handler = source
            .split_once("fn register_volume_pointer_handlers(")
            .and_then(|(_, source)| source.split_once("fn volume_controls("))
            .map(|(handler, _)| handler)
            .expect("volume pointer handler should exist");

        assert!(
            handler.contains("move_model.update(cx, |model, cx| model.set_volume(volume, cx));")
        );
        assert!(handler.contains("up_model.update(cx, |model, cx| model.set_volume(volume, cx));"));
    }

    #[test]
    fn volume_pointer_release_does_not_depend_on_pointer_being_inside_bounds() {
        let mut pointer = VolumePointerState::pending(10., 10., 0.4);
        assert!(pointer.update_move(10. + VOLUME_DRAG_THRESHOLD_PX, 10.));
        assert_eq!(pointer.finish(), Some(VolumePointerRelease::Dragging));
        assert_eq!(pointer.phase, VolumePointerPhase::DirectRelease);
    }

    #[test]
    fn volume_pointer_release_persists_in_cell_until_render_consumes_it() {
        let stored = Cell::new(VolumePointerState::pending(4., 6., 0.7));
        assert_eq!(
            finish_volume_pointer_interaction(&stored),
            Some(VolumePointerRelease::PendingClick)
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::Idle);

        let mut dragging = VolumePointerState::pending(4., 6., 0.7);
        assert!(dragging.update_move(4. + VOLUME_DRAG_THRESHOLD_PX, 6.));
        stored.set(dragging);
        assert_eq!(
            finish_volume_pointer_interaction(&stored),
            Some(VolumePointerRelease::Dragging)
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::DirectRelease);
        assert_eq!(finish_volume_pointer_interaction(&stored), None);

        assert_eq!(
            volume_motion_mode_for_render(&stored),
            VolumeMotionMode::Direct
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
    }

    #[test]
    fn pending_volume_lost_release_returns_to_animated_idle() {
        let stored = Cell::new(VolumePointerState::pending(8., 10., 0.6));

        assert_eq!(
            finish_volume_pointer_interaction(&stored),
            Some(VolumePointerRelease::PendingClick)
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
        assert_eq!(
            volume_motion_mode_for_render(&stored),
            VolumeMotionMode::Animated
        );
    }

    #[test]
    fn dragging_volume_lost_release_gets_one_final_direct_settle() {
        let mut dragging = VolumePointerState::pending(8., 10., 0.6);
        assert!(dragging.update_move(8. + VOLUME_DRAG_THRESHOLD_PX, 10.));
        let stored = Cell::new(dragging);

        assert_eq!(
            finish_volume_pointer_interaction(&stored),
            Some(VolumePointerRelease::Dragging)
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::DirectRelease);
        assert_eq!(
            volume_motion_mode_for_render(&stored),
            VolumeMotionMode::Direct
        );
        assert_eq!(stored.get().phase, VolumePointerPhase::Idle);
    }

    #[test]
    fn volume_motion_animates_large_jumps_between_discrete_positions() {
        let started_at = Instant::now();
        let mut motion = VolumeMotion::default();
        motion.prepare(0.1, started_at, false, VolumeMotionMode::Animated);

        let visual = motion.prepare(0.9, started_at, false, VolumeMotionMode::Animated);

        assert_eq!(visual.from, 0.1);
        assert_eq!(visual.target, 0.9);
        assert!(visual.active);
        let sampled = motion.displayed_at(started_at + crate::motion::INTERACTION_DURATION / 2);
        let expected = crate::motion::lerp(0.1, 0.9, gpui::ease_in_out(0.5));
        assert!((sampled - expected).abs() < 0.0001);
    }

    #[test]
    fn reduced_motion_snaps_volume_to_each_target() {
        let now = Instant::now();
        let mut motion = VolumeMotion::default();

        let first = motion.prepare(1.0, now, true, VolumeMotionMode::Animated);
        let second = motion.prepare(
            0.0,
            now + Duration::from_millis(20),
            true,
            VolumeMotionMode::Animated,
        );

        assert_eq!(first.from, 1.0);
        assert_eq!(second.from, 0.0);
        assert_eq!(second.target, 0.0);
        assert!(!second.active);
    }

    #[test]
    fn volume_motion_retargets_rapid_changes_from_the_displayed_value() {
        let started_at = Instant::now();
        let first_retarget_at = started_at + Duration::from_millis(30);
        let second_retarget_at = first_retarget_at + Duration::from_millis(25);
        let mut motion = VolumeMotion::default();

        motion.prepare(0.0, started_at, false, VolumeMotionMode::Animated);
        motion.prepare(1.0, first_retarget_at, false, VolumeMotionMode::Animated);
        let first_displayed = motion.displayed_at(first_retarget_at);
        let second = motion.prepare(0.2, first_retarget_at, false, VolumeMotionMode::Animated);
        assert!((second.from - first_displayed).abs() < 0.0001);

        let second_displayed = motion.displayed_at(second_retarget_at);
        let third = motion.prepare(0.8, second_retarget_at, false, VolumeMotionMode::Animated);
        assert!((third.from - second_displayed).abs() < 0.0001);
    }

    #[test]
    fn player_artist_routes_use_the_playback_provider_mapping() {
        assert_eq!(
            search_provider_for_playback(PlaybackProvider::Deezer),
            Provider::Deezer
        );
        assert_eq!(
            search_provider_for_playback(PlaybackProvider::SoundCloud),
            Provider::SoundCloud
        );

        let artists = vec![
            TrackArtistRef {
                id: "11".into(),
                name: "Primary".into(),
            },
            TrackArtistRef {
                id: "legacy/12".into(),
                name: "Legacy".into(),
            },
            TrackArtistRef {
                id: "13".into(),
                name: "Collaborator".into(),
            },
        ];
        let routes = artist_routes_for_track(
            search_provider_for_playback(PlaybackProvider::Deezer),
            &artists,
        );

        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].id, "11");
        assert_eq!(routes[1].id, "13");
    }

    #[test]
    fn rapid_seek_commits_continue_from_the_currently_displayed_value() {
        let started_at = Instant::now();
        let first_retarget_at = started_at + Duration::from_millis(40);
        let second_retarget_at = first_retarget_at + Duration::from_millis(30);
        let mut motion = SeekFillMotion::default();

        motion.prepare(1, 0., 1., started_at, false);
        let first_displayed = motion.displayed_at(first_retarget_at);
        let second = motion.prepare(2, 1., 0.1, first_retarget_at, false);

        assert!((second.from - first_displayed).abs() < 0.0001);
        let second_displayed = motion.displayed_at(second_retarget_at);
        let third = motion.prepare(3, 0.1, 0.8, second_retarget_at, false);

        assert!((third.from - second_displayed).abs() < 0.0001);
        assert!((third.from - 0.1).abs() > 0.0001);
    }

    #[test]
    fn direct_seek_preview_cancels_old_motion_before_release() {
        let started_at = Instant::now();
        let preview = 0.42;
        let release_at = started_at + Duration::from_millis(35);
        let mut motion = SeekFillMotion::default();

        motion.prepare(1, 0.2, 0.8, started_at, false);
        motion.set_displayed(preview);

        assert!(!motion.started_at.is_some());
        assert_eq!(motion.displayed_at(release_at), preview);

        let release = motion.prepare(2, preview, 0.9, release_at, false);

        assert_eq!(release.from, preview);
        assert_eq!(release.target, 0.9);
        assert!(release.active);
    }

    #[test]
    fn reduced_motion_snaps_seek_fill_to_each_commit_target() {
        let now = Instant::now();
        let mut motion = SeekFillMotion::default();

        let first = motion.prepare(1, 0., 1., now, true);
        assert_eq!(first.from, 1.);
        assert_eq!(first.target, 1.);
        assert!(!first.active);

        let second = motion.prepare(2, 1., 0., now + Duration::from_millis(50), true);
        assert_eq!(second.from, 0.);
        assert_eq!(second.target, 0.);
        assert!(!second.active);
    }

    #[test]
    fn accessibility_seek_uses_slider_step_and_clamps() {
        assert!(
            (accessibility_seek_fraction(0.2, super::SEEK_SLIDER_STEP, true).unwrap() - 0.21).abs()
                < f32::EPSILON
        );
        assert_eq!(
            accessibility_seek_fraction(0.995, super::SEEK_SLIDER_STEP, true),
            Some(1.0)
        );
        assert_eq!(
            accessibility_seek_fraction(0.005, -super::SEEK_SLIDER_STEP, true),
            Some(0.0)
        );
    }

    #[test]
    fn disabled_accessibility_seek_is_ignored() {
        assert_eq!(
            accessibility_seek_fraction(0.5, super::SEEK_SLIDER_STEP, false),
            None
        );
    }

    #[test]
    fn pointer_seek_fraction_uses_bounds_origin() {
        let bounds = Bounds {
            origin: point(px(10.), px(0.)),
            size: size(px(100.), px(24.)),
        };

        assert!((pointer_seek_fraction(60.25, bounds) - 0.5025).abs() < 0.000001);
    }

    #[test]
    fn pointer_seek_fraction_clamps_outside_bounds() {
        let bounds = Bounds {
            origin: point(px(10.), px(0.)),
            size: size(px(100.), px(24.)),
        };

        assert_eq!(pointer_seek_fraction(9., bounds), 0.);
        assert_eq!(pointer_seek_fraction(111., bounds), 1.);
    }

    #[test]
    fn pointer_seek_fraction_returns_zero_for_zero_width() {
        let bounds = Bounds {
            origin: point(px(10.), px(0.)),
            size: size(px(0.), px(24.)),
        };

        assert_eq!(pointer_seek_fraction(60.25, bounds), 0.);
    }

    #[test]
    fn expanded_volume_hitbox_keeps_fraction_mapping_on_the_logical_track() {
        let expanded = Bounds {
            origin: point(px(93.5), px(0.)),
            size: size(px(93.), px(24.)),
        };
        let logical = volume_logical_bounds(expanded);

        assert_eq!(logical.origin.x, px(100.));
        assert_eq!(logical.size.width, px(VOLUME_SLIDER_WIDTH_PX));
        assert_eq!(pointer_seek_fraction(93.5, logical), 0.);
        assert_eq!(pointer_seek_fraction(100., logical), 0.);
        assert_eq!(pointer_seek_fraction(180., logical), 1.);
        assert_eq!(pointer_seek_fraction(186.5, logical), 1.);
    }

    #[test]
    fn seek_pointer_state_survives_a_simulated_rerender() {
        let state = Rc::new(Cell::new(SeekPointerState::default()));
        let first_render_state = state.clone();
        first_render_state.set(SeekPointerState {
            active: true,
            moved: true,
        });

        let rerender_state = state.clone();

        assert_eq!(
            rerender_state.get(),
            SeekPointerState {
                active: true,
                moved: true,
            }
        );
    }

    #[test]
    fn opening_player_bar_animates_height_without_fading_content() {
        let now = Instant::now();
        let mut motion = PlayerBarMotion::default();

        let visual = motion.prepare(true, false, now, false);

        assert_eq!(visual.from_height, 0.);
        assert_eq!(visual.target_height, PLAYER_BAR_DESKTOP_HEIGHT_PX);
        assert_eq!(visual.from_opacity, 1.);
        assert_eq!(visual.target_opacity, 1.);
        assert!(visual.active);
    }

    #[test]
    fn closing_player_bar_still_fades_and_collapses() {
        let now = Instant::now();
        let mut motion = PlayerBarMotion::default();
        motion.prepare(true, false, now, true);

        let visual = motion.prepare(false, false, now, false);

        assert_eq!(visual.from_height, PLAYER_BAR_DESKTOP_HEIGHT_PX);
        assert_eq!(visual.target_height, 0.);
        assert_eq!(visual.from_opacity, 1.);
        assert_eq!(visual.target_opacity, 0.);
        assert!(visual.active);
    }

    #[test]
    fn compact_wide_layout_transition_interpolates_and_settles() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let now = Instant::now();
        let mut motion = PlayerBarLayoutMotion::default();

        let initial = motion.prepare(true, compact, now, false);
        assert_eq!(initial.from, compact);
        assert_eq!(initial.target, compact);
        assert!(!initial.active);

        let transition = motion.prepare(false, wide, now, false);
        assert_eq!(transition.from, compact);
        assert_eq!(transition.target, wide);
        assert!(transition.active);

        let midpoint = transition.at(0.5);
        assert!(
            (midpoint.center_width - (compact.center_width + wide.center_width) * 0.5).abs()
                < 0.001
        );
        assert!((midpoint.padding - (compact.padding + wide.padding) * 0.5).abs() < 0.001);

        let settled = motion.prepare(false, wide, now + crate::motion::PANEL_DURATION, false);
        assert_eq!(settled.from, wide);
        assert_eq!(settled.target, wide);
        assert!(!settled.active);
    }

    #[test]
    fn relocating_control_switches_geometry_only_while_fully_faded_out() {
        let from = RightControlGeometry {
            left: 20.,
            top: 17.,
            width: 42.,
            height: 24.,
        };
        let target = RightControlGeometry {
            left: 240.,
            top: 30.,
            width: 120.,
            height: 34.,
        };

        assert_eq!(fade_motion_geometry(from, target, 0.499), from);
        assert_eq!(fade_motion_geometry(from, target, 0.5), target);
        assert_eq!(fade_motion_opacity(1., 1., 0.5), 0.);
    }

    #[test]
    fn relocating_control_same_mode_resize_keeps_fade_epoch_and_start_time() {
        let compact = RightControlGeometry {
            left: 20.,
            top: 17.,
            width: 42.,
            height: 24.,
        };
        let wide = RightControlGeometry {
            left: 240.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let resized_wide = RightControlGeometry {
            left: 260.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let started_at = Instant::now();
        let resize_at = started_at + crate::motion::PANEL_DURATION / 4;
        let mut motion = FadeMotion::default();

        motion.prepare(true, compact, started_at, false, false);
        let transition = motion.prepare(false, wide, started_at, true, false);
        let retargeted = motion.prepare(false, resized_wide, resize_at, true, false);

        assert!(transition.active);
        assert!(retargeted.active);
        assert_eq!(retargeted.epoch, transition.epoch);
        assert_eq!(retargeted.started_at, transition.started_at);
        assert_eq!(retargeted.from, compact);
        assert_eq!(retargeted.target, resized_wide);
    }

    #[test]
    fn relocating_control_reversal_starts_from_current_displayed_state() {
        let compact = RightControlGeometry {
            left: 20.,
            top: 17.,
            width: 42.,
            height: 24.,
        };
        let wide = RightControlGeometry {
            left: 240.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let started_at = Instant::now();
        let reverse_at = started_at + crate::motion::PANEL_DURATION / 4;
        let mut motion = FadeMotion::default();

        motion.prepare(true, compact, started_at, false, false);
        motion.prepare(false, wide, started_at, true, false);
        let (displayed_geometry, displayed_opacity) = motion.displayed_at(reverse_at);
        let reversed = motion.prepare(true, compact, reverse_at, true, false);

        assert!(reversed.active);
        assert_eq!(reversed.from, displayed_geometry);
        assert!((reversed.from_opacity - displayed_opacity).abs() < 0.000001);
    }

    #[test]
    fn play_pause_button_has_no_unconditional_opacity_animation() {
        let source = include_str!("player_bar.rs");
        let function = source
            .split_once("fn play_pause_button(")
            .and_then(|(_, source)| source.split_once("fn bare_action_button("))
            .map(|(function, _)| function)
            .expect("play/pause button function should exist");

        assert!(!function.contains("with_animation"));
        assert!(!function.contains("play-pause-feedback"));
    }

    #[test]
    fn wide_current_track_uses_the_entire_side_column() {
        let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1600., false));

        assert!(layout.side_width > 380.);
        assert_eq!(current_block_width(layout), layout.side_width);
    }

    #[test]
    fn wide_favorite_stays_after_the_intrinsic_text_block() {
        let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));

        assert_eq!(favorite_left_for_text_width(layout, false, 120.), 204.);
        assert_eq!(favorite_left_for_text_width(layout, false, 0.), 84.);
    }

    #[test]
    fn compact_favorite_uses_the_center_column_anchor() {
        let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));

        assert_eq!(favorite_left_for_text_width(layout, true, 0.), 483.);
        assert_eq!(favorite_top(true), 2.);
        assert_eq!(favorite_top(false), 13.);
    }

    #[test]
    fn text_and_favorite_keep_a_gap_through_the_mode_transition() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let intrinsic_text_width = 1000.;
        let compact_text_width =
            current_text_width_for_layout(compact, true, true, intrinsic_text_width);
        let wide_text_width =
            current_text_width_for_layout(wide, false, true, intrinsic_text_width);
        let compact_favorite_left = favorite_left_for_text_width(compact, true, 0.);
        let wide_favorite_left = favorite_left_for_text_width(wide, false, wide_text_width);

        for step in 0..=10 {
            let delta = step as f32 / 10.;
            let text_width = crate::motion::lerp(compact_text_width, wide_text_width, delta);
            let favorite_left =
                crate::motion::lerp(compact_favorite_left, wide_favorite_left, delta);
            let text_right = 60. + 12. + text_width;
            assert!(
                text_right + 11. <= favorite_left + 0.001,
                "text and favorite overlap at transition delta {delta}"
            );
        }

        assert_eq!(
            current_text_available_width(compact, true, true),
            compact_text_width
        );
        assert_eq!(
            current_text_available_width(wide, false, true),
            wide_text_width
        );
    }

    #[test]
    fn favorite_measurement_uses_the_artist_line_text() {
        let routes = vec![
            MenuRoute {
                kind: MenuRouteKind::Artist,
                id: "1".into(),
                title: "Primary".into(),
            },
            MenuRoute {
                kind: MenuRouteKind::Artist,
                id: "2".into(),
                title: "Collaborator".into(),
            },
        ];

        assert_eq!(
            rendered_artist_text("Primary, Collaborator", Provider::Deezer, &routes),
            "Primary, Collaborator"
        );
        assert_eq!(
            rendered_artist_text("Primary, Guest", Provider::Deezer, &routes),
            "Primary, Collaborator"
        );
        assert_eq!(
            rendered_artist_text("Primary, Guest", Provider::SoundCloud, &routes[..1]),
            "Primary, Guest"
        );
    }

    #[test]
    fn live_resize_keeps_the_active_mode_transition_clock_stable() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let resized = PlayerBarLayout::from_geometry(desktop_player_geometry(1500., false));
        let started_at = Instant::now();
        let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = PlayerBarLayoutMotion::default();

        motion.prepare(true, compact, started_at, false);
        let transition = motion.prepare(false, wide, started_at, false);
        let transition_started_at = motion.started_at;
        let progress_before_resize = motion.animation_progress(resize_at);
        let retargeted = motion.prepare(false, resized, resize_at, false);

        assert!(retargeted.active);
        assert!(!retargeted.mode_changed);
        assert_eq!(retargeted.from, transition.from);
        assert_eq!(retargeted.target, resized);
        assert_eq!(retargeted.epoch, transition.epoch);
        assert_eq!(motion.started_at, transition_started_at);
        assert_eq!(motion.animation_progress(resize_at), progress_before_resize);
    }

    #[test]
    fn repeated_same_mode_resize_does_not_restart_the_layout_transition() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let resized_once = PlayerBarLayout::from_geometry(desktop_player_geometry(1500., false));
        let resized_twice = PlayerBarLayout::from_geometry(desktop_player_geometry(1600., false));
        let started_at = Instant::now();
        let resize_once_at = started_at + crate::motion::PANEL_DURATION / 3;
        let resize_twice_at = resize_once_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = PlayerBarLayoutMotion::default();

        motion.prepare(true, compact, started_at, false);
        let transition = motion.prepare(false, wide, started_at, false);
        let transition_started_at = motion.started_at;
        let first = motion.prepare(false, resized_once, resize_once_at, false);
        let second = motion.prepare(false, resized_twice, resize_twice_at, false);

        assert!(first.active);
        assert!(second.active);
        assert_eq!(first.from, transition.from);
        assert_eq!(second.from, transition.from);
        assert_eq!(first.epoch, transition.epoch);
        assert_eq!(second.epoch, transition.epoch);
        assert_eq!(second.target, resized_twice);
        assert_eq!(motion.started_at, transition_started_at);
    }

    #[test]
    fn mode_reversal_still_rebases_from_the_current_layout() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let reversed_compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1439., true));
        let started_at = Instant::now();
        let reverse_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = PlayerBarLayoutMotion::default();

        motion.prepare(true, compact, started_at, false);
        let outward = motion.prepare(false, wide, started_at, false);
        let displayed = motion.displayed_at(reverse_at);
        let reversed = motion.prepare(true, reversed_compact, reverse_at, false);

        assert!(reversed.active);
        assert!(reversed.mode_changed);
        assert_eq!(reversed.from, displayed);
        assert_eq!(reversed.target, reversed_compact);
        assert_eq!(reversed.epoch, outward.epoch.wrapping_add(1));
        assert_eq!(motion.started_at, Some(reverse_at));
    }

    #[test]
    fn breakpoint_wiggle_restarts_only_when_the_mode_actually_changes() {
        let started_at = Instant::now();
        let mut motion = PlayerBarLayoutMotion::default();
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        motion.prepare(true, compact, started_at, false);

        let wide_at = started_at + Duration::from_millis(20);
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let outward = motion.prepare(false, wide, wide_at, false);
        assert!(outward.mode_changed);
        assert_eq!(motion.started_at, Some(wide_at));

        let wide_resize_at = wide_at + Duration::from_millis(12);
        let wider = PlayerBarLayout::from_geometry(desktop_player_geometry(1460., false));
        let same_wide = motion.prepare(false, wider, wide_resize_at, false);
        assert!(!same_wide.mode_changed);
        assert_eq!(same_wide.epoch, outward.epoch);
        assert_eq!(motion.started_at, Some(wide_at));

        let compact_at = wide_resize_at + Duration::from_millis(12);
        let compact_again = PlayerBarLayout::from_geometry(desktop_player_geometry(1439., true));
        let reversed = motion.prepare(true, compact_again, compact_at, false);
        assert!(reversed.mode_changed);
        assert_eq!(reversed.epoch, outward.epoch.wrapping_add(1));
        assert_eq!(motion.started_at, Some(compact_at));

        let compact_resize_at = compact_at + Duration::from_millis(12);
        let narrower = PlayerBarLayout::from_geometry(desktop_player_geometry(1420., true));
        let same_compact = motion.prepare(true, narrower, compact_resize_at, false);
        assert!(!same_compact.mode_changed);
        assert_eq!(same_compact.epoch, reversed.epoch);
        assert_eq!(motion.started_at, Some(compact_at));
    }

    #[test]
    fn scalar_motion_retargets_text_width_from_the_displayed_value() {
        let started_at = Instant::now();
        let retarget_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = ScalarMotion::default();

        motion.prepare(0., started_at, true, false, false);
        let first = motion.prepare(240., started_at, true, true, false);
        assert!(first.active);
        let displayed = motion.displayed_at(retarget_at);
        let retargeted = motion.prepare(96., retarget_at, true, true, false);

        assert!(retargeted.active);
        assert_eq!(retargeted.from, displayed);
        assert_eq!(retargeted.target, 96.);
    }

    #[test]
    fn scalar_motion_updates_resize_targets_without_restarting() {
        let started_at = Instant::now();
        let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = ScalarMotion::default();

        motion.prepare(0., started_at, true, false, false);
        let transition = motion.prepare(240., started_at, true, true, false);
        let transition_started_at = motion.started_at;
        let resized = motion.prepare(180., resize_at, true, false, false);

        assert!(resized.active);
        assert_eq!(resized.from, transition.from);
        assert_eq!(resized.target, 180.);
        assert_eq!(resized.epoch, transition.epoch);
        assert_eq!(motion.started_at, transition_started_at);
    }

    #[test]
    fn rect_motion_retargets_right_control_from_the_displayed_geometry() {
        let started_at = Instant::now();
        let retarget_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = RectMotion::default();
        let from = RightControlGeometry {
            left: 20.,
            top: 17.,
            width: 34.,
            height: 24.,
        };
        let target = RightControlGeometry {
            left: 240.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let resized = RightControlGeometry {
            left: 180.,
            top: 28.,
            width: 100.,
            height: 34.,
        };

        motion.prepare(from, started_at, true, false, false);
        motion.prepare(target, started_at, true, true, false);
        let displayed = motion.displayed_at(retarget_at);
        let retargeted = motion.prepare(resized, retarget_at, true, true, false);

        assert!(retargeted.active);
        assert_eq!(retargeted.from, displayed);
        assert_eq!(retargeted.target, resized);
    }

    #[test]
    fn rect_motion_updates_resize_targets_without_restarting() {
        let started_at = Instant::now();
        let resize_at = started_at + crate::motion::PANEL_DURATION / 3;
        let mut motion = RectMotion::default();
        let from = RightControlGeometry {
            left: 20.,
            top: 17.,
            width: 34.,
            height: 24.,
        };
        let target = RightControlGeometry {
            left: 240.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let resized = RightControlGeometry {
            left: 210.,
            top: 30.,
            width: 110.,
            height: 34.,
        };

        motion.prepare(from, started_at, true, false, false);
        let transition = motion.prepare(target, started_at, true, true, false);
        let transition_started_at = motion.started_at;
        let resized_visual = motion.prepare(resized, resize_at, true, false, false);

        assert!(resized_visual.active);
        assert_eq!(resized_visual.from, transition.from);
        assert_eq!(resized_visual.target, resized);
        assert_eq!(resized_visual.epoch, transition.epoch);
        assert_eq!(motion.started_at, transition_started_at);
    }

    #[test]
    fn scalar_and_rect_motion_snap_when_reduced_motion_is_enabled() {
        let now = Instant::now();
        let mut scalar = ScalarMotion::default();
        scalar.prepare(12., now, true, false, true);
        let scalar_visual = scalar.prepare(80., now, true, true, true);
        assert_eq!(scalar_visual.from, 80.);
        assert_eq!(scalar_visual.target, 80.);
        assert!(!scalar_visual.active);

        let mut rect = RectMotion::default();
        rect.prepare(
            RightControlGeometry {
                left: 0.,
                top: 0.,
                width: 34.,
                height: 34.,
            },
            now,
            true,
            false,
            true,
        );
        let target = RightControlGeometry {
            left: 80.,
            top: 30.,
            width: 120.,
            height: 34.,
        };
        let rect_visual = rect.prepare(target, now, true, true, true);
        assert_eq!(rect_visual.from, target);
        assert_eq!(rect_visual.target, target);
        assert!(!rect_visual.active);
    }

    #[test]
    fn compact_wide_layout_reduced_motion_snaps_without_repaint_state() {
        let compact = PlayerBarLayout::from_geometry(desktop_player_geometry(1440., true));
        let wide = PlayerBarLayout::from_geometry(desktop_player_geometry(1441., false));
        let now = Instant::now();
        let mut motion = PlayerBarLayoutMotion::default();

        motion.prepare(true, compact, now, false);
        let snapped = motion.prepare(false, wide, now, true);

        assert_eq!(snapped.from, wide);
        assert_eq!(snapped.target, wide);
        assert!(!snapped.active);
        assert_eq!(motion.started_at, None);
    }

    #[test]
    fn compact_geometry_matches_the_900px_reference() {
        assert_eq!(desktop_player_geometry(900., true), (16., 12., 360., 242.),);
    }

    #[test]
    fn compact_geometry_reaches_440px_center_at_1440px() {
        assert_eq!(desktop_player_geometry(1440., true), (16., 12., 440., 472.),);
    }

    #[test]
    fn compact_right_keeps_action_track_before_second_column() {
        for width in 769..=1440 {
            let width = width as f32;
            let (content_width, action_width, action_gap, volume_shift, second_column_width) =
                super::compact_player_right_geometry(
                    super::desktop_player_geometry(width, true).3,
                    width,
                );
            let action_content_width = 68. + action_gap;
            let action_right = (action_width + action_content_width) * 0.5;
            let second_column_left = action_width - volume_shift;
            assert!(
                action_right <= second_column_left + 0.001,
                "compact right controls overlap at {width}px"
            );
            assert!(
                action_width + second_column_width <= content_width + 0.001,
                "compact right controls overflow at {width}px"
            );
        }
    }

    #[test]
    fn compact_volume_shifts_two_pixels_left_without_moving_quality() {
        for width in 769..=1440 {
            let width = width as f32;
            let layout = PlayerBarLayout::from_geometry(desktop_player_geometry(width, true));
            let (_, action_width, _, _, _) =
                super::compact_player_right_geometry(layout.side_width, width);
            let quality = super::right_control_geometry(
                super::RightControlKind::Quality,
                layout,
                width,
                true,
                QUALITY_BADGE_MIN_WIDTH_PX,
            );
            let volume = super::right_control_geometry(
                super::RightControlKind::Volume,
                layout,
                width,
                true,
                QUALITY_BADGE_MIN_WIDTH_PX,
            );
            assert_eq!(
                quality.left, action_width,
                "compact quality left moved at {width}px"
            );
            assert_eq!(
                volume.left,
                action_width - 2.,
                "compact volume left is not 2px left at {width}px"
            );
        }
    }

    #[test]
    fn wide_geometry_switches_to_the_520px_center_at_1441px() {
        assert_eq!(
            desktop_player_geometry(1441., false),
            (16., 20., 520., 424.5),
        );
    }

    #[test]
    fn quality_badge_keeps_its_box_and_offsets_only_the_text() {
        assert_eq!(QUALITY_BADGE_HEIGHT_PX, 20.);
        assert_eq!(QUALITY_BADGE_MIN_WIDTH_PX, 42.0);
        assert_eq!(QUALITY_BADGE_TEXT_OFFSET_PX, -1.);
        assert_eq!(PLAYER_CLOSE_GLYPH_PX, 9.);
    }

    #[test]
    fn quality_text_animation_identity_is_stable_across_generations() {
        let previous_generation_label = quality_text_animation_key("FLAC");
        let next_generation_label = quality_text_animation_key("FLAC");

        assert_eq!(previous_generation_label, next_generation_label);
    }

    #[test]
    fn quality_text_animation_identity_changes_for_a_different_label() {
        assert_ne!(
            quality_text_animation_key("FLAC"),
            quality_text_animation_key("MP3")
        );
    }

    #[test]
    fn quality_badge_animation_identity_changes_across_generations_for_same_label() {
        assert_ne!(
            quality_badge_animation_key(1, "FLAC"),
            quality_badge_animation_key(2, "FLAC")
        );
    }

    #[test]
    fn quality_badge_animation_identity_is_stable_for_same_generation_and_label() {
        assert_eq!(
            quality_badge_animation_key(1, "FLAC"),
            quality_badge_animation_key(1, "FLAC")
        );
    }

    #[test]
    fn quality_label_clears_stale_label_on_generation_change_before_resolution() {
        let mut label = String::from("FLAC");
        let mut generation = Some(1);

        update_quality_label_for_generation(&mut label, &mut generation, 2, None);

        assert_eq!(label, "");
        assert_eq!(generation, Some(2));
    }

    #[test]
    fn quality_label_retains_known_quality_when_unavailable_in_same_generation() {
        let mut label = String::from("FLAC");
        let mut generation = Some(1);

        update_quality_label_for_generation(&mut label, &mut generation, 1, None);

        assert_eq!(label, "FLAC");
        assert_eq!(generation, Some(1));
    }

    #[test]
    fn quality_label_stores_immediate_quality_on_generation_change() {
        let mut label = String::from("FLAC");
        let mut generation = Some(1);

        update_quality_label_for_generation(&mut label, &mut generation, 2, Some("MP3"));

        assert_eq!(label, "MP3");
        assert_eq!(generation, Some(2));
    }

    #[test]
    fn quality_badge_opacity_fades_only_when_the_shell_appears() {
        assert_eq!(quality_badge_opacity_endpoints("", ""), (0., 0.));
        assert_eq!(quality_badge_opacity_endpoints("FLAC", ""), (0., 0.));
        assert_eq!(quality_badge_opacity_endpoints("", "FLAC"), (0., 1.));
        assert_eq!(quality_badge_opacity_endpoints("FLAC", "MP3"), (1., 1.));
        assert_eq!(quality_badge_opacity_endpoints("FLAC", "FLAC"), (1., 1.));
    }

    #[test]
    fn quality_text_opacity_fades_first_and_different_labels_only() {
        assert_eq!(quality_text_opacity_endpoints("", ""), (0., 0.));
        assert_eq!(quality_text_opacity_endpoints("", "FLAC"), (0., 1.));
        assert_eq!(quality_text_opacity_endpoints("FLAC", "MP3"), (0., 1.));
        assert_eq!(quality_text_opacity_endpoints("FLAC", "FLAC"), (1., 1.));
    }

    #[test]
    fn quality_text_opacity_keeps_the_retained_label_visible() {
        let mut label = String::from("FLAC");

        update_last_quality_label(&mut label, None);

        assert_eq!(label, "FLAC");
        assert_eq!(quality_text_opacity_endpoints("FLAC", &label), (1., 1.));
    }

    #[test]
    fn quality_badge_shell_does_not_animate_opacity() {
        let source = include_str!("player_bar.rs");
        let shell = source
            .split_once("fn quality_badge_shell(")
            .and_then(|(_, source)| source.split_once("fn quality_badge_width("))
            .map(|(shell, _)| shell)
            .expect("quality badge shell function should exist");

        assert!(!shell.contains("with_animation"));
        assert!(!shell.contains(".opacity("));
    }

    #[test]
    fn quality_badge_retains_the_previous_label_while_unavailable() {
        let mut label = String::from("FLAC");

        update_last_quality_label(&mut label, None);
        assert_eq!(label, "FLAC");

        update_last_quality_label(&mut label, Some("MP3"));
        assert_eq!(label, "MP3");

        update_last_quality_label(&mut label, None);
        assert_eq!(label, "MP3");

        update_last_quality_label(&mut label, Some(""));
        assert_eq!(label, "MP3");
    }

    #[test]
    fn close_player_uses_the_five_pixel_right_inset() {
        assert_eq!(CLOSE_PLAYER_RIGHT_PX, 5.);
    }

    #[test]
    fn close_player_tooltip_gap_matches_layout_density() {
        assert_eq!(CLOSE_PLAYER_COMPACT_TOOLTIP_GAP_PX, 5.);
        assert_eq!(CLOSE_PLAYER_EXPANDED_TOOLTIP_GAP_PX, 3.);
        assert_eq!(close_player_tooltip_gap(true), 5.);
        assert_eq!(close_player_tooltip_gap(false), 3.);
    }

    #[test]
    fn player_action_buttons_use_the_original_radius() {
        assert_eq!(PLAYER_ACTION_BUTTON_RADIUS_PX, 6.);
    }

    #[test]
    fn repeat_one_badge_uses_the_original_inner_control_geometry() {
        assert_eq!(REPEAT_CONTROL_SIZE_PX, 14.);
        assert_eq!(REPEAT_ONE_BADGE_RIGHT_PX, 2.);
        assert_eq!(REPEAT_ONE_BADGE_BOTTOM_PX, 4.);
    }

    #[test]
    fn volume_control_geometry_matches_the_original() {
        assert_eq!(VOLUME_GAP_PX, 7.);
        assert_eq!(VOLUME_MUTE_BUTTON_PX, 34.);
        assert_eq!(VOLUME_SLIDER_WIDTH_PX, 80.);
        assert_eq!(VOLUME_SLIDER_CONTROL_HEIGHT_PX, 24.);
        assert_eq!(VOLUME_TRACK_HEIGHT_PX, 4.);
        assert_eq!(VOLUME_THUMB_DIAMETER_PX, 13.);
        assert_eq!(VOLUME_ICON_FRAME_PX, 16.);
        assert_eq!(VOLUME_ICON_EM_HEIGHT_PX, 14.5);
    }

    #[test]
    fn wide_volume_inset_moves_only_the_volume_control() {
        assert_eq!(WIDE_VOLUME_INSET_PX, 12.);
        assert_eq!(wide_volume_inset(true), 12.);
        assert_eq!(wide_volume_inset(false), 0.);
    }

    #[test]
    fn compact_volume_container_offsets_nudge_two_pixels_left() {
        assert_eq!(volume_container_offsets(false), (-2., 0.));
        assert_eq!(volume_container_offsets(true), (14., WIDE_VOLUME_INSET_PX));
    }

    #[test]
    fn volume_icons_preserve_fontawesome_viewbox_widths() {
        assert_eq!(
            volume_icon_dimensions(VolumeIconLevel::High),
            (18.125, 14.5)
        );
        assert_eq!(
            volume_icon_dimensions(VolumeIconLevel::Low),
            (12.6875, 14.5)
        );
        assert_eq!(volume_icon_dimensions(VolumeIconLevel::Off), (9.0625, 14.5));
        assert_eq!(
            volume_icon_dimensions(VolumeIconLevel::Muted),
            (16.3125, 14.5)
        );
    }

    #[test]
    fn volume_speaker_bodies_share_a_fixed_left_anchor() {
        assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::High), -0.90625);
        assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Low), 0.);
        assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Off), 0.);
        assert_eq!(volume_icon_speaker_offset(VolumeIconLevel::Muted), 0.);
    }

    #[test]
    fn ended_queue_keeps_the_last_track_artist_visible() {
        let track = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: "7".into(),
            title: "Last Song".into(),
            artist: "Last Artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            service_url: String::new(),
        };
        assert_eq!(
            current_subtitle(Some(&track), PlaybackStatus::Ended, None, false),
            "Last Artist"
        );
        assert_eq!(
            current_subtitle(Some(&track), PlaybackStatus::Playing, None, false),
            "Last Artist"
        );
        assert_eq!(
            rendered_current_artist_text(
                Some(&track),
                PlaybackStatus::Ended,
                None,
                "Last Artist",
                None
            ),
            "Last Artist"
        );
        let implementation = include_str!("player_bar.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("player bar implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(!implementation.contains("Queue ended"));
    }

    #[test]
    fn cached_loading_keeps_the_artist_visible() {
        let track = PlaybackTrack {
            downloadable: false,
            progressive: false,
            provider: PlaybackProvider::Deezer,
            id: "7".into(),
            title: "Cached Song".into(),
            artist: "Cached Artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            explicit: false,
            service_url: String::new(),
        };

        assert_eq!(
            current_subtitle(Some(&track), PlaybackStatus::Loading, None, true),
            "Cached Artist"
        );
        assert_eq!(
            current_subtitle(Some(&track), PlaybackStatus::Loading, None, false),
            "Loading audio..."
        );
    }
}
