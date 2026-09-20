//! Bottom player bar view: transport controls, seek bar, volume, and the
//! per-track action cluster. Extracted from the playback model module so the
//! model stays free of rendering concerns.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use gpui::{
    AccessibleAction, AnimationExt, AnyElement, AnyImageCache, App, Bounds, ClickEvent, Context,
    Entity, Font, FontWeight, Hitbox, HitboxBehavior, Hsla, ImageCacheError, ImageSource,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, Render, RenderImage, Resource, Role, TextRun, Window, canvas, div, img,
    prelude::*, px, relative, rgb, rgba,
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
        BORDER, FOREGROUND, MUTED, PRIMARY, SCROLLBAR_THUMB, SURFACE, SURFACE_RAISED,
        ui_font_family,
    },
    ui::artwork_cache::ArtworkCache,
};

use super::view::SEEK_SLIDER_STEP;
use super::{
    PlaybackModel, PlaybackProvider, PlaybackStatus, PlaybackTrack, RepeatMode, RightSidebar,
    VolumeIconLevel,
};

const QUALITY_BADGE_HEIGHT_PX: f32 = 20.;
const QUALITY_BADGE_MIN_WIDTH_PX: f32 = 42.0;
const QUALITY_BADGE_TEXT_OFFSET_PX: f32 = -1.;
pub(crate) const PLAYER_BAR_DESKTOP_HEIGHT_PX: f32 = 94.;
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
    player_bar_layout_motion: PlayerBarLayoutMotion,
    text_width_motion: ScalarMotion,
    favorite_motion: FadeMotion,
    right_control_motion: [RectMotion; 4],
    quality_motion: FadeMotion,
    download_motion: FadeMotion,
    volume_offset_motion: RectMotion,
    last_quality_label: String,
    last_quality_generation: Option<u64>,
    artwork_hold: ArtworkHold,
}

impl PlaybackView {
    pub(crate) fn new(
        model: Entity<PlaybackModel>,
        account: Entity<AccountState>,
        downloads: Entity<DownloadModel>,
        library: Entity<LibraryView>,
        favorites: Entity<FavoriteState>,
        favorite_controller: Entity<FavoriteController>,
        artwork_cache: Entity<ArtworkCache>,
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
            artwork_hold: ArtworkHold::new(artwork_cache),
            external_track_navigation: None,
            seek_control_enabled: None,
            seek_pointer_state: Rc::new(Cell::new(SeekPointerState::default())),
            volume_pointer_state: Rc::new(Cell::new(VolumePointerState::default())),
            buffered_motion: BufferedMotion::default(),
            seek_fill_motion: SeekFillMotion::default(),
            volume_motion: VolumeMotion::default(),
            last_seek_commit_epoch: 0,
            last_display_progress: 0.,
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

mod motion;
use motion::*;
pub(crate) use motion::{PlayerBarMotion, PlayerBarVisual};

mod volume;
use volume::*;

mod render;

mod current_track;
use current_track::*;

mod right_controls;
use right_controls::*;

mod controls;
use controls::*;

mod seek;
use seek::*;

#[cfg(test)]
mod tests;
