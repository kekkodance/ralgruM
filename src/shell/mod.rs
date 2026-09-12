use gpui::{
    AnimationExt as _, AnyElement, ClickEvent, Context, Entity, FocusHandle, IntoElement, Render,
    ScrollHandle, Task, WeakEntity, Window, div, prelude::*, px, rgb,
};
use gpui_component::Root;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;

use crate::{
    app_tooltip::{AppTooltipOverlay, dismiss_global as dismiss_app_tooltip},
    artwork_cache::ArtworkCache,
    browser_link::{BrowserEntity, BrowserKind},
    browser_scroll::BrowserScrollState,
    cache::CacheView,
    downloads::DownloadModel,
    entity_navigation::{NavigationOpener, NavigationTarget, ProviderNavigationOpeners},
    library::{
        Category as LibraryCategory, FavoriteController, FavoriteState, LibraryEvent, LibraryView,
        Service as LibraryService,
    },
    lyrics::{LyricsPanel, LyricsTrackInput},
    navigation_state::{RightSidebarView, StartPage},
    playback::{
        ListenHistorySignal, PlaybackModel, PlaybackProvider, PlaybackView, QueuePanel,
        RightSidebar,
    },
    search::{Provider, SearchView, Source},
    settings::{
        AccountState, Category as SettingsCategory, MotionTransition, SettingsEvent, SettingsView,
        motion_transition, should_apply_deferred_motion_reduction,
    },
    theme::{BACKGROUND, BORDER, FOREGROUND, ui_font_family},
    toast::ToastStack,
    window_state::WindowStateManager,
};

mod chrome;
use chrome::render_titlebar;

mod sidebar;
use sidebar::render_sidebar;

mod sidebar_badge;
use sidebar_badge::SidebarDownloadBadgeMotion;

mod navigation;
use navigation::{
    library_category_for_service, library_selection, nav_from_stored, search_active_for_nav,
    source_from_stored,
};

mod settings_import;

mod downloads;
use downloads::render_downloads;

mod download_notices;
use download_notices::subscribe_download_notices;

mod search_suggestions;

mod toolbar;
use toolbar::render_top_toolbar;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Nav {
    Discover,
    Library,
    Downloads,
    Cache,
}

pub(crate) struct RalgrumApp {
    nav: Nav,
    settings_mode: bool,
    import_sync_in_progress: bool,
    external_detail_return: Option<Nav>,
    last_library_selection: (LibraryService, LibraryCategory),
    last_account_scope: String,
    last_download_scope: u128,
    last_lyrics_track_identity: Option<(PlaybackProvider, String)>,
    search: Entity<SearchView>,
    settings: Entity<SettingsView>,
    account: Entity<AccountState>,
    pub(super) downloads: Entity<DownloadModel>,
    library: Entity<LibraryView>,
    playback_view: Entity<PlaybackView>,
    playback: Entity<PlaybackModel>,
    lyrics: Entity<LyricsPanel>,
    queue: Entity<QueuePanel>,
    cache_view: Entity<CacheView>,
    right_sidebar_transition: RightSidebarTransition,
    right_sidebar_close_task: Option<Task<()>>,
    settings_motion_generation: u64,
    sidebar_width_motion: SidebarWidthMotion,
    sidebar_bottom_motion: SidebarBottomMotion,
    sidebar_download_badge_motion: SidebarDownloadBadgeMotion,
    source_selector_motion: RectSelectorMotion,
    toolbar_geometry_motion: toolbar::ToolbarGeometryMotion,
    detail_toolbar_motion: toolbar::DetailToolbarMotion,
    detail_toolbar_focus_ready: bool,
    detail_toolbar_focus_target: bool,
    detail_toolbar_focus_generation: u64,
    detail_toolbar_focus_task: Option<Task<()>>,
    search_clear_motion: crate::motion::ResponsiveModeMotion,
    mobile_sidebar_open: bool,
    toasts: Entity<ToastStack>,
    app_tooltip: Entity<AppTooltipOverlay>,
    artwork_cache: Entity<ArtworkCache>,
    source_tab_focus: Vec<FocusHandle>,
    library_service_tab_focus: Vec<FocusHandle>,
    settings_category_focus: Vec<FocusHandle>,
    downloads_scroll: ScrollHandle,
    downloads_browser_scroll: BrowserScrollState,
    _window_state: Entity<WindowStateManager>,
}

fn external_navigation_opener(
    shell: WeakEntity<RalgrumApp>,
    provider: Provider,
) -> NavigationOpener {
    Arc::new(
        move |target: NavigationTarget, _window: &mut Window, cx: &mut gpui::App| {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            shell.update(cx, |app, cx| {
                if app.external_detail_return.is_none() {
                    app.external_detail_return = Some(app.nav);
                }
                app.nav = Nav::Discover;
                app.search.update(cx, |search, cx| {
                    search.set_search_active(true);
                    search.open_external_card(target.card(provider), cx);
                });
                app.persist_navigation(cx);
                cx.notify();
            });
        },
    )
}

fn external_navigation_openers(shell: WeakEntity<RalgrumApp>) -> ProviderNavigationOpeners {
    ProviderNavigationOpeners::new(
        (
            external_navigation_opener(shell.clone(), Provider::Deezer),
            external_navigation_opener(shell.clone(), Provider::Deezer),
        ),
        (
            external_navigation_opener(shell.clone(), Provider::SoundCloud),
            external_navigation_opener(shell, Provider::SoundCloud),
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RightSidebarTransitionAction {
    None,
    CancelClose,
    ScheduleClose { epoch: u64, started_at: Instant },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RightSidebarTransition {
    displayed_sidebar: RightSidebar,
    desired_sidebar: RightSidebar,
    closing: bool,
    epoch: u64,
    close_started_at: Option<Instant>,
    content_switch: bool,
    // Settings unmounts lyrics/queue; remounting must not replay enter.
    skip_enter: bool,
}

impl RightSidebarTransition {
    fn new(sidebar: RightSidebar) -> Self {
        Self {
            displayed_sidebar: sidebar,
            desired_sidebar: sidebar,
            closing: false,
            epoch: 0,
            close_started_at: None,
            content_switch: false,
            skip_enter: false,
        }
    }

    fn skip_enter_if_already_open(&mut self) {
        self.skip_enter = self.displayed_sidebar != RightSidebar::Closed
            && self.desired_sidebar == self.displayed_sidebar
            && !self.closing;
    }

    fn request(
        &mut self,
        desired_sidebar: RightSidebar,
        reduced_motion: bool,
        now: Instant,
    ) -> RightSidebarTransitionAction {
        if self.desired_sidebar == desired_sidebar {
            if reduced_motion && self.closing {
                self.epoch = self.epoch.wrapping_add(1);
                self.displayed_sidebar = RightSidebar::Closed;
                self.closing = false;
                self.close_started_at = None;
                self.content_switch = false;
                self.skip_enter = false;
                return RightSidebarTransitionAction::CancelClose;
            }
            return RightSidebarTransitionAction::None;
        }

        let was_open = self.displayed_sidebar != RightSidebar::Closed;
        let is_open = desired_sidebar != RightSidebar::Closed;
        self.content_switch = was_open && is_open;
        self.skip_enter = false;

        self.desired_sidebar = desired_sidebar;
        self.epoch = self.epoch.wrapping_add(1);

        match desired_sidebar {
            RightSidebar::Closed => {
                self.close_started_at = None;
                if reduced_motion || self.displayed_sidebar == RightSidebar::Closed {
                    self.displayed_sidebar = RightSidebar::Closed;
                    self.closing = false;
                    RightSidebarTransitionAction::CancelClose
                } else {
                    self.closing = true;
                    self.close_started_at = Some(now);
                    RightSidebarTransitionAction::ScheduleClose {
                        epoch: self.epoch,
                        started_at: now,
                    }
                }
            }
            sidebar => {
                self.displayed_sidebar = sidebar;
                self.closing = false;
                self.close_started_at = None;
                RightSidebarTransitionAction::CancelClose
            }
        }
    }

    #[cfg(test)]
    fn finish_close(&mut self, epoch: u64, now: Instant) -> bool {
        if self.epoch != epoch
            || self.desired_sidebar != RightSidebar::Closed
            || !self.closing
            || self.close_started_at.is_none_or(|started_at| {
                now.duration_since(started_at) < crate::motion::PANEL_DURATION
            })
        {
            return false;
        }

        self.complete_close(epoch)
    }

    fn finish_close_after_timer(&mut self, epoch: u64) -> bool {
        if self.epoch != epoch || self.desired_sidebar != RightSidebar::Closed || !self.closing {
            return false;
        }

        self.complete_close(epoch)
    }

    fn complete_close(&mut self, epoch: u64) -> bool {
        if self.epoch != epoch || self.desired_sidebar != RightSidebar::Closed || !self.closing {
            return false;
        }

        self.displayed_sidebar = RightSidebar::Closed;
        self.closing = false;
        self.close_started_at = None;
        self.content_switch = false;
        self.skip_enter = false;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarWidthMotion {
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Instant,
    initialized: bool,
}

impl Default for SidebarWidthMotion {
    fn default() -> Self {
        Self {
            from: 0.0,
            target: 0.0,
            epoch: 0,
            started_at: Instant::now(),
            initialized: false,
        }
    }
}

impl SidebarWidthMotion {
    fn retarget(&mut self, target: f32, now: Instant, reduced_motion: bool) {
        if !self.initialized {
            self.from = target;
            self.target = target;
            self.started_at = now;
            self.initialized = true;
            return;
        }

        if self.target == target {
            if reduced_motion || crate::motion::PANEL_DURATION.is_zero() {
                self.from = target;
            }
            return;
        }

        self.from = self.displayed_width(now);
        self.target = target;
        self.started_at = now;
        self.epoch = self.epoch.wrapping_add(1);

        if reduced_motion || crate::motion::PANEL_DURATION.is_zero() {
            self.from = target;
        }
    }

    fn displayed_width(self, now: Instant) -> f32 {
        if !self.initialized || self.from == self.target {
            return self.target;
        }

        let duration = crate::motion::PANEL_DURATION;
        if duration.is_zero() {
            return self.target;
        }

        let progress = crate::motion::clamp_unit(
            now.saturating_duration_since(self.started_at).as_secs_f32() / duration.as_secs_f32(),
        );
        let eased = 1.0 - (1.0 - progress).powi(5);
        crate::motion::lerp(self.from, self.target, eased)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarBottomMotion {
    compact_from: f32,
    compact_target: f32,
    settings_from: f32,
    settings_target: f32,
    started_at: Instant,
    epoch: u64,
    initialized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SidebarBottomVisual {
    pub(super) compact_from: f32,
    pub(super) compact_target: f32,
    pub(super) settings_from: f32,
    pub(super) settings_target: f32,
    pub(super) epoch: u64,
}

impl Default for SidebarBottomMotion {
    fn default() -> Self {
        Self {
            compact_from: 0.0,
            compact_target: 0.0,
            settings_from: 0.0,
            settings_target: 0.0,
            started_at: Instant::now(),
            epoch: 0,
            initialized: false,
        }
    }
}

impl SidebarBottomMotion {
    fn prepare(
        &mut self,
        compact: bool,
        settings: bool,
        now: Instant,
        reduced_motion: bool,
    ) -> SidebarBottomVisual {
        let compact_target = compact as u8 as f32;
        let settings_target = settings as u8 as f32;

        if !self.initialized {
            self.compact_from = compact_target;
            self.compact_target = compact_target;
            self.settings_from = settings_target;
            self.settings_target = settings_target;
            self.started_at = now;
            self.initialized = true;
        } else if self.compact_target != compact_target || self.settings_target != settings_target {
            let (compact_from, settings_from) = self.displayed_fractions(now);
            self.compact_from = compact_from;
            self.compact_target = compact_target;
            self.settings_from = settings_from;
            self.settings_target = settings_target;
            self.started_at = now;
            self.epoch = self.epoch.wrapping_add(1);
        } else if self.animation_progress(now) >= 1.0 {
            self.compact_from = self.compact_target;
            self.settings_from = self.settings_target;
        }

        if reduced_motion || crate::motion::PANEL_DURATION.is_zero() {
            self.compact_from = self.compact_target;
            self.settings_from = self.settings_target;
        }

        SidebarBottomVisual {
            compact_from: self.compact_from,
            compact_target: self.compact_target,
            settings_from: self.settings_from,
            settings_target: self.settings_target,
            epoch: self.epoch,
        }
    }

    fn displayed_fractions(self, now: Instant) -> (f32, f32) {
        let progress = self.animation_progress(now);
        let eased = 1.0 - (1.0 - progress).powi(5);
        (
            crate::motion::lerp(self.compact_from, self.compact_target, eased),
            crate::motion::lerp(self.settings_from, self.settings_target, eased),
        )
    }

    fn animation_progress(self, now: Instant) -> f32 {
        if !self.initialized || crate::motion::PANEL_DURATION.is_zero() {
            return 1.0;
        }

        crate::motion::clamp_unit(
            now.saturating_duration_since(self.started_at).as_secs_f32()
                / crate::motion::PANEL_DURATION.as_secs_f32(),
        )
    }
}

impl SidebarBottomVisual {
    pub(super) fn fractions_at(self, delta: f32) -> (f32, f32) {
        (
            crate::motion::lerp(self.compact_from, self.compact_target, delta),
            crate::motion::lerp(self.settings_from, self.settings_target, delta),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
enum RectSelectorTransition {
    #[default]
    Interaction,
    Responsive,
}

impl RectSelectorTransition {
    fn duration(self) -> Duration {
        match self {
            Self::Interaction => crate::motion::INTERACTION_DURATION,
            Self::Responsive => crate::motion::CONTENT_DURATION,
        }
    }

    fn eased_progress(self, progress: f32) -> f32 {
        let progress = crate::motion::clamp_unit(progress);
        match self {
            Self::Interaction => 1.0 - (1.0 - progress).powi(5),
            Self::Responsive => gpui::ease_in_out(progress),
        }
    }

    fn animation(self) -> gpui::Animation {
        match self {
            Self::Interaction => crate::motion::interaction(),
            Self::Responsive => crate::motion::content(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RectSelectorMotion {
    pub(super) from_x: f32,
    pub(super) from_width: f32,
    pub(super) target_x: f32,
    pub(super) target_width: f32,
    pub(super) epoch: u64,
    pub(super) started_at: Instant,
    transition: RectSelectorTransition,
    initialized: bool,
}

impl Default for RectSelectorMotion {
    fn default() -> Self {
        Self {
            from_x: 0.0,
            from_width: 0.0,
            target_x: 0.0,
            target_width: 0.0,
            epoch: 0,
            started_at: Instant::now(),
            transition: RectSelectorTransition::default(),
            initialized: false,
        }
    }
}

impl RectSelectorMotion {
    pub(super) fn retarget(
        &mut self,
        target_x: f32,
        target_width: f32,
        now: Instant,
        reduced_motion: bool,
        force_rebase: bool,
    ) {
        let transition = if force_rebase {
            RectSelectorTransition::Responsive
        } else {
            RectSelectorTransition::Interaction
        };

        if !self.initialized {
            self.from_x = target_x;
            self.from_width = target_width;
            self.target_x = target_x;
            self.target_width = target_width;
            self.started_at = now;
            self.transition = transition;
            self.initialized = true;
            return;
        }

        if !force_rebase && self.target_x == target_x && self.target_width == target_width {
            if reduced_motion || self.animation_progress(now) >= 1.0 {
                self.from_x = target_x;
                self.from_width = target_width;
            }
            return;
        }

        let (from_x, from_width) = self.visible_geometry(now);
        self.from_x = from_x;
        self.from_width = from_width;
        self.target_x = target_x;
        self.target_width = target_width;
        self.started_at = now;
        self.transition = transition;
        self.epoch = self.epoch.wrapping_add(1);

        if reduced_motion || self.transition.duration().is_zero() {
            self.from_x = target_x;
            self.from_width = target_width;
        }
    }

    pub(super) fn animation(self) -> gpui::Animation {
        self.transition.animation()
    }

    pub(super) fn visible_geometry(self, now: Instant) -> (f32, f32) {
        if !self.initialized
            || (self.from_x == self.target_x && self.from_width == self.target_width)
        {
            return (self.target_x, self.target_width);
        }

        let duration = self.transition.duration();
        if duration.is_zero() {
            return (self.target_x, self.target_width);
        }

        let progress = self.animation_progress(now);
        let eased = self.transition.eased_progress(progress);
        (
            crate::motion::lerp(self.from_x, self.target_x, eased),
            crate::motion::lerp(self.from_width, self.target_width, eased),
        )
    }

    fn animation_progress(self, now: Instant) -> f32 {
        let duration = self.transition.duration();
        if !self.initialized || self.from_x == self.target_x && self.from_width == self.target_width
        {
            return 1.0;
        }
        if duration.is_zero() {
            return 1.0;
        }

        crate::motion::clamp_unit(
            now.saturating_duration_since(self.started_at).as_secs_f32() / duration.as_secs_f32(),
        )
    }
}

fn playback_sidebar_exits_settings(previous: RightSidebar, current: RightSidebar) -> bool {
    previous != current && matches!(current, RightSidebar::Lyrics | RightSidebar::Queue)
}

fn right_sidebar_layout_width(sidebar_width: f32, closing: bool, delta: f32) -> f32 {
    if closing {
        crate::motion::lerp(sidebar_width, 0.0, delta)
    } else {
        crate::motion::lerp(0.0, sidebar_width, delta)
    }
}

impl RalgrumApp {
    pub(crate) fn open_browser_link(&mut self, entity: BrowserEntity, cx: &mut Context<Self>) {
        if entity.kind != BrowserKind::Track {
            if self.external_detail_return.is_none() {
                self.external_detail_return = Some(self.nav);
            }
            self.settings_mode = false;
            self.nav = Nav::Discover;
            self.search
                .update(cx, |search, _| search.set_search_active(true));
            self.persist_navigation(cx);
        }
        self.search
            .update(cx, |search, cx| search.open_browser_link(entity, cx));
        cx.notify();
    }

    pub(crate) fn new(
        settings: Entity<SettingsView>,
        account: Entity<AccountState>,
        runtime: Arc<Runtime>,
        window_state: Entity<WindowStateManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&window_state, |_, _, cx| cx.notify()).detach();
        let saved = settings.read(cx).saved().clone();
        cx.set_reduce_motion(saved.motion_preference.is_reduced());
        let cache = settings.read(cx).cache.clone();
        let artwork_cache = ArtworkCache::new_entity(runtime.clone(), cx);
        let initial_account_scope = account.read(cx).library_scope();
        let listen_history_changed = Arc::new(ListenHistorySignal::default());
        let favorites = cx.new(|_| FavoriteState::default());
        let media =
            crate::media_control::MediaSession::new(crate::media_control::window_handle(window));
        let playback = cx.new(|cx| {
            PlaybackModel::new(
                account.clone(),
                runtime.clone(),
                saved.discord_presence,
                cache.clone(),
                saved.background_audio_cache,
                saved.seamless_playback,
                saved.record_deezer_plays,
                listen_history_changed.clone(),
                (
                    saved.volume,
                    saved.muted,
                    if saved.remember_playback_modes {
                        saved.repeat_mode
                    } else {
                        crate::playback::RepeatMode::Off
                    },
                    saved.remember_playback_modes && saved.shuffle_enabled,
                ),
                saved.block_explicit_content,
                (
                    saved.right_sidebar_open,
                    match saved.right_sidebar_view {
                        RightSidebarView::Lyrics => RightSidebar::Lyrics,
                        RightSidebarView::Queue => RightSidebar::Queue,
                    },
                ),
                media,
                cx,
            )
        });
        let toasts = cx.new(|_| ToastStack::new());
        crate::toast::set_global(cx, &toasts);
        let app_tooltip = cx.new(|_| AppTooltipOverlay::new());
        crate::app_tooltip::set_global(cx, &app_tooltip);
        let downloads = cx.new(|_| {
            DownloadModel::new(account.clone(), runtime.clone())
                .with_cache(cache.clone())
                .with_toasts(toasts.downgrade())
        });
        subscribe_download_notices(&downloads, cx);
        let favorite_controller = cx
            .new(|_| FavoriteController::new(account.clone(), favorites.clone(), runtime.clone()));
        let lyrics = cx.new(|_| LyricsPanel::new(playback.clone(), runtime.clone()));
        lyrics.update(cx, |lyrics, cx| {
            lyrics.set_default_provider(saved.lyrics_source, cx)
        });
        let library = cx.new(|cx| {
            LibraryView::new(
                account.clone(),
                settings.clone(),
                favorites.clone(),
                runtime.clone(),
                playback.clone(),
                downloads.clone(),
                window,
                cx,
            )
        });
        let playback_view = cx.new(|cx| {
            PlaybackView::new(
                playback.clone(),
                account.clone(),
                downloads.clone(),
                library.clone(),
                favorites.clone(),
                favorite_controller.clone(),
                cx,
            )
        });
        let search = cx.new(|cx| {
            SearchView::new(
                account.clone(),
                settings.clone(),
                library.clone(),
                favorites.clone(),
                runtime.clone(),
                playback.clone(),
                downloads.clone(),
                window,
                cx,
            )
        });
        let queue = cx.new(|_| {
            QueuePanel::new(
                playback.clone(),
                &library,
                &search,
                downloads.clone(),
                account.clone(),
                favorites.clone(),
            )
        });
        let external_navigation = external_navigation_openers(cx.entity().downgrade());
        let cache_view = cx.new(|cx| {
            CacheView::new(
                cache.clone(),
                runtime.clone(),
                playback.clone(),
                &library,
                external_navigation.clone(),
                downloads.clone(),
                account.clone(),
                favorites.clone(),
                cx,
            )
        });
        cx.subscribe_in(
            &settings,
            window,
            |this, _, event: &SettingsEvent, window, cx| match event {
                SettingsEvent::ExitRequested => {
                    this.settings_mode = false;
                    this.right_sidebar_transition.skip_enter_if_already_open();
                    this.close_mobile_sidebar(cx);
                    cx.notify();
                }
                SettingsEvent::Imported(settings) => {
                    this.apply_imported_settings(settings.clone(), window, cx);
                }
            },
        )
        .detach();
        cx.subscribe(&library, |this, _, event: &LibraryEvent, cx| match event {
            LibraryEvent::SettingsOpened => {
                this.settings_mode = true;
                this.close_mobile_sidebar(cx);
                cx.notify();
            }
            LibraryEvent::SelectionChanged => this.persist_navigation(cx),
            LibraryEvent::NavigationRequested => {
                this.external_detail_return = None;
                this.settings_mode = false;
                this.nav = Nav::Library;
                this.search
                    .update(cx, |search, _| search.set_search_active(false));
                this.close_mobile_sidebar(cx);
                cx.notify();
            }
            LibraryEvent::DiscoverRequested => {
                this.external_detail_return = None;
                this.settings_mode = false;
                this.nav = Nav::Discover;
                this.search
                    .update(cx, |search, _| search.set_search_active(true));
                this.close_mobile_sidebar(cx);
                this.persist_navigation(cx);
                cx.notify();
            }
            LibraryEvent::SmartMixTitleResolved { config_id, title } => {
                this.search.update(cx, |search, cx| {
                    search.apply_smart_mix_title(config_id, title, cx);
                });
            }
        })
        .detach();
        let lyrics_for_playback = lyrics.clone();
        let mut last_shell_playback = {
            let state = &playback.read(cx).state;
            (
                state.right_sidebar,
                state.current().map(|t| (t.provider, t.id.clone())),
                state
                    .lyrics_display_track()
                    .map(|t| (t.provider, t.id.clone())),
                state.status,
            )
        };
        cx.observe(&playback, move |this, playback, cx| {
            let (sync_args, current) = {
                let state = &playback.read(cx).state;
                let is_lyrics = state.right_sidebar == RightSidebar::Lyrics;
                let lyrics_track = if is_lyrics {
                    state
                        .lyrics_display_track()
                        .map(LyricsTrackInput::from_playback)
                } else {
                    state.current().map(LyricsTrackInput::from_playback)
                };
                let position = if is_lyrics && state.lyrics_follows_playback() {
                    state.position.as_secs_f64()
                } else {
                    0.0
                };
                let sync_args = Some((lyrics_track, position, is_lyrics));
                let current = (
                    state.right_sidebar,
                    state.current().map(|t| (t.provider, t.id.clone())),
                    state
                        .lyrics_display_track()
                        .map(|t| (t.provider, t.id.clone())),
                    state.status,
                );
                (sync_args, current)
            };
            this.sync_right_sidebar_transition(current.0, cx);
            let sidebar_exits_settings =
                playback_sidebar_exits_settings(last_shell_playback.0, current.0);
            let exited_settings =
                sidebar_exits_settings && (this.settings_mode || this.mobile_sidebar_open);
            if sidebar_exits_settings {
                this.settings_mode = false;
                this.mobile_sidebar_open = false;
            }
            if let Some((lyrics_track, position, is_lyrics)) = sync_args {
                lyrics_for_playback.update(cx, |lyrics, cx| {
                    lyrics.sync_track(is_lyrics, lyrics_track, position, cx);
                });
            }
            if exited_settings || last_shell_playback != current {
                last_shell_playback = current;
                cx.notify();
            }
        })
        .detach();
        let history_library = library.clone();
        let listen_history_changed = listen_history_changed.clone();
        cx.observe(&playback, move |_, _, cx| {
            let changes = listen_history_changed.take();
            if changes.contains(PlaybackProvider::Deezer) {
                history_library.update(cx, |library, cx| library.invalidate_deezer_history(cx));
            }
            if changes.contains(PlaybackProvider::SoundCloud) {
                history_library.update(cx, |library, cx| library.invalidate_soundcloud_history(cx));
            }
        })
        .detach();
        let settings_for_playback = settings.clone();
        cx.observe(&playback, move |_, playback, cx| {
            let playback = playback.read(cx);
            let preferences = playback.playback_preferences();
            let sidebar = playback.state.sidebar_preferences();
            let should_persist = {
                let saved = settings_for_playback.read(cx).saved();
                saved.volume != preferences.0
                    || saved.muted != preferences.1
                    || (saved.remember_playback_modes
                        && (saved.repeat_mode != preferences.2
                            || saved.shuffle_enabled != preferences.3))
                    || saved.right_sidebar_open != sidebar.0
                    || saved.right_sidebar_view
                        != match sidebar.1 {
                            RightSidebar::Lyrics => RightSidebarView::Lyrics,
                            RightSidebar::Queue | RightSidebar::Closed => RightSidebarView::Queue,
                        }
            };
            if should_persist {
                settings_for_playback.update(cx, |settings, cx| {
                    settings.persist_runtime_state(
                        preferences,
                        (
                            sidebar.0,
                            match sidebar.1 {
                                RightSidebar::Lyrics => RightSidebarView::Lyrics,
                                RightSidebar::Queue | RightSidebar::Closed => {
                                    RightSidebarView::Queue
                                }
                            },
                        ),
                        cx,
                    );
                });
            }
        })
        .detach();
        let library_for_account = library.clone();
        let search_for_account = search.clone();
        let favorites_for_account = favorites.clone();
        let playback_for_account = playback.clone();
        let downloads_for_account = downloads.clone();
        cx.observe(&account, move |this, account, cx| {
            let scope = account.read(cx).library_scope();
            let download_scope = account.read(cx).credential_generation();
            let library_scope_changed = update_account_scope(&mut this.last_account_scope, &scope);
            let download_scope_changed =
                update_download_scope(&mut this.last_download_scope, download_scope);
            if library_scope_changed {
                this.external_detail_return = None;
                favorites_for_account.update(cx, |favorites, _| favorites.reset_account());
                library_for_account.update(cx, |library, cx| {
                    library.account_scope_changed(scope.clone(), cx);
                });
                search_for_account.update(cx, |search, cx| {
                    search.account_scope_changed(scope, cx);
                });
                playback_for_account.update(cx, |playback, cx| {
                    playback.account_scope_changed(cx);
                });
            }
            if download_scope_changed {
                downloads_for_account.update(cx, |downloads, cx| {
                    downloads.account_scope_changed(cx);
                });
            }
            cx.notify();
        })
        .detach();
        let initial_source = if saved.remember_navigation {
            source_from_stored(saved.source_filter)
        } else {
            Source::All
        };
        search.update(cx, |search, cx| search.select_source(initial_source, cx));
        if saved.remember_navigation {
            let result_type = match saved.search_type.as_str() {
                "tracks" => crate::search::ResultType::Tracks,
                "albums" => crate::search::ResultType::Albums,
                "artists" => crate::search::ResultType::Artists,
                "playlists" => crate::search::ResultType::Playlists,
                _ => crate::search::ResultType::All,
            };
            search.update(cx, |search, cx| search.restore_result_type(result_type, cx));
        }
        let initial_nav = match saved.start_page {
            StartPage::Last if saved.remember_navigation => nav_from_stored(saved.last_main_tab),
            StartPage::Library => Nav::Library,
            StartPage::Downloads => Nav::Downloads,
            StartPage::Last | StartPage::Discover => Nav::Discover,
        };
        search.update(cx, |search, _| {
            search.set_search_active(search_active_for_nav(initial_nav))
        });
        if initial_nav == Nav::Library {
            let (service, category) = library_selection(&saved);
            library.update(cx, |library, cx| library.load(service, category, cx));
        }
        let initial_library_selection = library.read(cx).selection();
        cx.observe(&library, |this, library, cx| {
            let selection = library.read(cx).selection();
            if selection != this.last_library_selection
                && !library.read(cx).discover_flow_returns_to_discover()
            {
                this.last_library_selection = selection;
                this.persist_navigation(cx);
            }
            this.search
                .update(cx, |search, cx| search.sync_playlist_update(cx));
            this.search
                .update(cx, |search, cx| search.sync_playlist_content(cx));
            let closed_viewed_playlist = this
                .search
                .update(cx, |search, cx| search.sync_playlist_delete(cx));
            if let Some(provider) = closed_viewed_playlist {
                this.external_detail_return = None;
                this.nav = Nav::Library;
                this.search.update(cx, |search, _| {
                    search.set_search_active(search_active_for_nav(Nav::Library))
                });
                this.library.update(cx, |library, cx| {
                    library.load_force(
                        match provider {
                            Provider::Deezer => crate::library::Service::Deezer,
                            Provider::SoundCloud => crate::library::Service::SoundCloud,
                        },
                        crate::library::Category::Playlists,
                        cx,
                    );
                });
                this.persist_navigation(cx);
            }
            this.search
                .update(cx, |search, cx| search.sync_playlist_remove(cx));
            this.search
                .update(cx, |search, cx| search.sync_playlist_reorder(cx));
            cx.notify();
        })
        .detach();
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        cx.observe(&downloads, |_, _, cx| cx.notify()).detach();
        let mut last_applied_settings = {
            let saved = settings.read(cx).saved();
            (
                saved.discord_presence,
                saved.audio_cache_limit_mb,
                saved.background_audio_cache,
                saved.seamless_playback,
                saved.record_deezer_plays,
                saved.block_explicit_content,
                saved.lyrics_source,
                saved.soundcloud_search_suggestions,
                saved.motion_preference.is_reduced(),
            )
        };
        cx.observe(&settings, move |this, _, cx| {
            let saved = this.settings.read(cx).saved();
            let current = (
                saved.discord_presence,
                saved.audio_cache_limit_mb,
                saved.background_audio_cache,
                saved.seamless_playback,
                saved.record_deezer_plays,
                saved.block_explicit_content,
                saved.lyrics_source,
                saved.soundcloud_search_suggestions,
                saved.motion_preference.is_reduced(),
            );
            if last_applied_settings != current {
                let previous_motion_reduced = last_applied_settings.8;
                last_applied_settings = current;
                let (
                    discord_presence,
                    cache_limit,
                    background_cache,
                    seamless_playback,
                    record_deezer_plays,
                    block_explicit,
                    lyrics_source,
                    soundcloud_search_suggestions,
                    motion_reduced,
                ) = current;
                this.playback.update(cx, |playback, cx| {
                    playback.set_discord_presence(discord_presence);
                    playback.set_cache_limit(cache_limit);
                    playback.set_background_audio_cache(background_cache);
                    playback.set_seamless_playback(seamless_playback);
                    playback.set_provider_play_reporting(record_deezer_plays);
                    playback.set_skip_explicit(block_explicit, cx);
                });
                match motion_transition(previous_motion_reduced, motion_reduced) {
                    MotionTransition::Unchanged => {}
                    MotionTransition::EnableImmediately => {
                        this.settings_motion_generation =
                            this.settings_motion_generation.wrapping_add(1);
                        cx.set_reduce_motion(false);
                    }
                    MotionTransition::DisableAfter(delay) => {
                        this.settings_motion_generation =
                            this.settings_motion_generation.wrapping_add(1);
                        let generation = this.settings_motion_generation;
                        let shell = cx.entity().downgrade();
                        let executor = cx.background_executor().clone();
                        cx.spawn(async move |_, cx| {
                            executor.timer(delay).await;
                            let _ = shell.update(cx, |this, cx| {
                                let still_reduced = this
                                    .settings
                                    .read(cx)
                                    .saved()
                                    .motion_preference
                                    .is_reduced();
                                if should_apply_deferred_motion_reduction(
                                    generation,
                                    this.settings_motion_generation,
                                    still_reduced,
                                ) {
                                    cx.set_reduce_motion(true);
                                }
                            });
                        })
                        .detach();
                    }
                }
                let right_sidebar = this.playback.read(cx).state.right_sidebar;
                this.sync_right_sidebar_transition(right_sidebar, cx);
                this.lyrics.update(cx, |lyrics, cx| {
                    lyrics.set_default_provider(lyrics_source, cx);
                });
                this.search.update(cx, |search, cx| {
                    search.set_soundcloud_suggestions_enabled(soundcloud_search_suggestions, cx);
                });
                this.persist_navigation(cx);
            }
            // Settings category/session state is rendered by the shell too.
            // Keep the observer one-way so persistence updates cannot loop.
            cx.notify();
        })
        .detach();
        library.update(cx, |library, _| {
            library.set_external_track_navigation(external_navigation.clone());
        });
        playback_view.update(cx, |playback_view, _| {
            playback_view.set_external_track_navigation(external_navigation);
        });
        let initial_right_sidebar = playback.read(cx).state.right_sidebar;
        Self {
            nav: initial_nav,
            settings_mode: false,
            import_sync_in_progress: false,
            external_detail_return: None,
            last_library_selection: initial_library_selection,
            last_account_scope: initial_account_scope,
            last_download_scope: account.read(cx).credential_generation(),
            last_lyrics_track_identity: None,
            search,
            settings,
            account,
            downloads,
            library,
            playback_view,
            playback,
            lyrics,
            queue,
            cache_view,
            right_sidebar_transition: RightSidebarTransition::new(initial_right_sidebar),
            right_sidebar_close_task: None,
            settings_motion_generation: 0,
            sidebar_width_motion: SidebarWidthMotion::default(),
            sidebar_bottom_motion: SidebarBottomMotion::default(),
            sidebar_download_badge_motion: SidebarDownloadBadgeMotion::default(),
            source_selector_motion: RectSelectorMotion::default(),
            toolbar_geometry_motion: toolbar::ToolbarGeometryMotion::default(),
            detail_toolbar_motion: toolbar::DetailToolbarMotion::default(),
            detail_toolbar_focus_ready: false,
            detail_toolbar_focus_target: false,
            detail_toolbar_focus_generation: 0,
            detail_toolbar_focus_task: None,
            search_clear_motion: crate::motion::ResponsiveModeMotion::default(),
            mobile_sidebar_open: false,
            toasts,
            app_tooltip,
            artwork_cache,
            source_tab_focus: (0..3).map(|_| cx.focus_handle()).collect(),
            library_service_tab_focus: (0..3).map(|_| cx.focus_handle()).collect(),
            settings_category_focus: (0..SettingsCategory::ALL.len())
                .map(|_| cx.focus_handle())
                .collect(),
            downloads_scroll: ScrollHandle::new(),
            downloads_browser_scroll: BrowserScrollState::new(),
            _window_state: window_state,
        }
    }

    fn sync_right_sidebar_transition(
        &mut self,
        desired_sidebar: RightSidebar,
        cx: &mut Context<Self>,
    ) {
        let action = self.right_sidebar_transition.request(
            desired_sidebar,
            cx.reduce_motion() || crate::motion::PANEL_DURATION.is_zero(),
            Instant::now(),
        );
        match action {
            RightSidebarTransitionAction::None => {}
            RightSidebarTransitionAction::CancelClose => {
                self.right_sidebar_close_task = None;
            }
            RightSidebarTransitionAction::ScheduleClose { epoch, started_at } => {
                self.right_sidebar_close_task = None;
                let executor = cx.background_executor().clone();
                let remaining = crate::motion::PANEL_DURATION.saturating_sub(started_at.elapsed());
                self.right_sidebar_close_task = Some(cx.spawn(async move |this, cx| {
                    executor.timer(remaining).await;
                    let _ = this.update(cx, |this, cx| {
                        if this
                            .right_sidebar_transition
                            .finish_close_after_timer(epoch)
                        {
                            this.right_sidebar_close_task = None;
                            cx.notify();
                        }
                    });
                }));
            }
        }
    }

    pub(super) fn toggle_mobile_sidebar(&mut self, cx: &mut Context<Self>) {
        self.mobile_sidebar_open = !self.mobile_sidebar_open;
        cx.notify();
    }

    pub(super) fn close_mobile_sidebar(&mut self, cx: &mut Context<Self>) {
        if self.mobile_sidebar_open {
            self.mobile_sidebar_open = false;
            cx.notify();
        }
    }

    fn select_source(&mut self, source: Source, _: &mut Window, cx: &mut Context<Self>) {
        self.external_detail_return = None;
        self.nav = Nav::Discover;
        self.search.update(cx, |search, cx| {
            search.set_search_active(search_active_for_nav(Nav::Discover));
            search.select_source(source, cx);
        });
        self.persist_navigation(cx);
    }

    fn select_library_service(
        &mut self,
        service: LibraryService,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.external_detail_return = None;
        let saved = self.settings.read(cx).saved().clone();
        let category = library_category_for_service(&saved, service);
        self.library
            .update(cx, |library, cx| library.load(service, category, cx));
        self.persist_navigation(cx);
    }

    pub(crate) fn select_library(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.external_detail_return = None;
        self.nav = Nav::Library;
        let saved = self.settings.read(cx).saved().clone();
        let (service, category) = library_selection(&saved);
        self.library
            .update(cx, |library, cx| library.load(service, category, cx));
        self.persist_navigation(cx);
        cx.notify();
    }

    fn submit_search(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.submit_search_now(cx);
    }

    fn submit_search_now(&mut self, cx: &mut Context<Self>) {
        self.external_detail_return = None;
        self.nav = Nav::Discover;
        self.search.update(cx, |search, cx| search.submit(cx));
    }

    pub(super) fn close_search_detail_state(&mut self, cx: &mut Context<Self>) {
        dismiss_app_tooltip(cx);
        self.search
            .update(cx, |search, cx| search.close_search_navigation(cx));
        if !self.search.read(cx).detail_open()
            && !self.search.read(cx).discover_channel_open()
            && let Some(destination) = self.external_detail_return.take()
        {
            self.nav = destination;
            self.search.update(cx, |search, _| {
                search.set_search_active(search_active_for_nav(destination))
            });
            self.persist_navigation(cx);
            cx.notify();
        }
    }

    pub(super) fn close_search_toolbar_target(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        dismiss_app_tooltip(cx);
        if self.nav == Nav::Discover && self.search.read(cx).search_results_root_open() {
            self.external_detail_return = None;
            self.search.update(cx, |search, cx| {
                search.return_to_discover_home(window, cx);
            });
            self.persist_navigation(cx);
            cx.notify();
        } else {
            self.close_search_detail_state(cx);
        }
    }
}

fn update_account_scope(last_scope: &mut String, observed_scope: &str) -> bool {
    if last_scope == observed_scope {
        return false;
    }
    observed_scope.clone_into(last_scope);
    true
}

fn update_download_scope(last_scope: &mut u128, observed_scope: u128) -> bool {
    if *last_scope == observed_scope {
        return false;
    }
    *last_scope = observed_scope;
    true
}

#[cfg(test)]
mod tests {
    use super::{
        RectSelectorMotion, RectSelectorTransition, RightSidebarTransition,
        RightSidebarTransitionAction, SidebarBottomMotion, SidebarWidthMotion,
        playback_sidebar_exits_settings, right_sidebar_layout_width, update_account_scope,
        update_download_scope,
    };
    use crate::{motion, playback::RightSidebar};
    use std::time::{Duration, Instant};

    #[test]
    fn account_observer_only_transitions_when_scope_changes() {
        let mut last_scope = "deezer\n42\nsoundcloud".to_owned();

        assert!(!update_account_scope(
            &mut last_scope,
            "deezer\n42\nsoundcloud"
        ));
        assert!(update_account_scope(
            &mut last_scope,
            "other-deezer\n7\nother-soundcloud"
        ));
        assert_eq!(last_scope, "other-deezer\n7\nother-soundcloud");
    }

    #[test]
    fn download_observer_tracks_credential_generation_separately() {
        let mut last_scope = 7;
        assert!(!update_download_scope(&mut last_scope, 7));
        assert!(update_download_scope(&mut last_scope, 8));
        assert_eq!(last_scope, 8);
    }

    #[test]
    fn sidebar_transition_retains_panel_until_close_duration() {
        let started_at = Instant::now();
        let mut transition = RightSidebarTransition::new(RightSidebar::Queue);

        assert_eq!(
            transition.request(RightSidebar::Closed, false, started_at),
            RightSidebarTransitionAction::ScheduleClose {
                epoch: 1,
                started_at,
            }
        );
        assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);
        assert!(transition.closing);
        assert!(!transition.finish_close(1, started_at));
        assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);

        assert!(transition.finish_close(1, started_at + motion::PANEL_DURATION));
        assert_eq!(transition.displayed_sidebar, RightSidebar::Closed);
        assert!(!transition.closing);
    }

    #[test]
    fn sidebar_transition_reopen_invalidates_stale_close_completion() {
        let started_at = Instant::now();
        let mut transition = RightSidebarTransition::new(RightSidebar::Lyrics);
        let close_epoch = match transition.request(RightSidebar::Closed, false, started_at) {
            RightSidebarTransitionAction::ScheduleClose { epoch, .. } => epoch,
            action => panic!("unexpected transition action: {action:?}"),
        };

        assert_eq!(
            transition.request(RightSidebar::Lyrics, false, started_at),
            RightSidebarTransitionAction::CancelClose
        );
        assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
        assert_eq!(transition.desired_sidebar, RightSidebar::Lyrics);
        assert!(!transition.closing);
        assert!(!transition.finish_close(close_epoch, started_at + motion::PANEL_DURATION));
    }

    #[test]
    fn sidebar_transition_between_open_panels_marks_content_switch() {
        let now = Instant::now();
        let mut transition = RightSidebarTransition::new(RightSidebar::Queue);
        assert!(!transition.content_switch);

        assert_eq!(
            transition.request(RightSidebar::Lyrics, false, now),
            RightSidebarTransitionAction::CancelClose
        );
        assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
        assert!(transition.content_switch);

        assert_eq!(
            transition.request(RightSidebar::Queue, false, now),
            RightSidebarTransitionAction::CancelClose
        );
        assert_eq!(transition.displayed_sidebar, RightSidebar::Queue);
        assert!(transition.content_switch);

        assert_eq!(
            transition.request(RightSidebar::Closed, false, now),
            RightSidebarTransitionAction::ScheduleClose {
                epoch: transition.epoch,
                started_at: now,
            }
        );
        assert!(!transition.content_switch);
    }

    #[test]
    fn sidebar_transition_skips_enter_when_settings_remounts_open_panel() {
        let now = Instant::now();
        let mut transition = RightSidebarTransition::new(RightSidebar::Queue);
        transition.skip_enter_if_already_open();
        assert!(transition.skip_enter);
        assert_eq!(
            transition.request(RightSidebar::Queue, false, now),
            RightSidebarTransitionAction::None
        );
        assert!(transition.skip_enter);

        assert_eq!(
            transition.request(RightSidebar::Closed, false, now),
            RightSidebarTransitionAction::ScheduleClose {
                epoch: 1,
                started_at: now,
            }
        );
        assert!(!transition.skip_enter);
        assert!(transition.closing);
    }

    #[test]
    fn sidebar_transition_does_not_skip_enter_when_closed_or_closing() {
        let now = Instant::now();
        let mut closed = RightSidebarTransition::new(RightSidebar::Closed);
        closed.skip_enter_if_already_open();
        assert!(!closed.skip_enter);

        let mut closing = RightSidebarTransition::new(RightSidebar::Lyrics);
        assert_eq!(
            closing.request(RightSidebar::Closed, false, now),
            RightSidebarTransitionAction::ScheduleClose {
                epoch: 1,
                started_at: now,
            }
        );
        closing.skip_enter_if_already_open();
        assert!(!closing.skip_enter);
    }

    #[test]
    fn sidebar_transition_real_open_keeps_enter_animation() {
        let now = Instant::now();
        let mut transition = RightSidebarTransition::new(RightSidebar::Closed);
        transition.skip_enter_if_already_open();
        assert_eq!(
            transition.request(RightSidebar::Lyrics, false, now),
            RightSidebarTransitionAction::CancelClose
        );
        assert!(!transition.skip_enter);
        assert!(!transition.content_switch);
        assert_eq!(transition.displayed_sidebar, RightSidebar::Lyrics);
    }

    #[test]
    fn playback_sidebar_transition_matrix_exits_settings_only_on_panel_change() {
        let sidebars = [
            RightSidebar::Closed,
            RightSidebar::Queue,
            RightSidebar::Lyrics,
        ];
        for previous in sidebars {
            for current in sidebars {
                let expected = previous != current
                    && matches!(current, RightSidebar::Queue | RightSidebar::Lyrics);
                assert_eq!(
                    playback_sidebar_exits_settings(previous, current),
                    expected,
                    "unexpected transition: {previous:?} -> {current:?}"
                );
            }
        }
    }

    #[test]
    fn sidebar_width_motion_initializes_at_first_rendered_width() {
        let started_at = Instant::now();
        let mut motion = SidebarWidthMotion::default();

        motion.retarget(240.0, started_at, false);

        assert!(motion.initialized);
        assert_eq!(motion.from, 240.0);
        assert_eq!(motion.target, 240.0);
        assert_eq!(motion.epoch, 0);
        assert_eq!(motion.displayed_width(started_at), 240.0);
    }

    #[test]
    fn sidebar_width_motion_retargets_from_current_displayed_width() {
        let started_at = Instant::now();
        let mut motion = SidebarWidthMotion::default();
        motion.retarget(240.0, started_at, false);
        motion.retarget(68.0, started_at, false);

        let retarget_at = started_at + Duration::from_millis(40);
        let displayed = motion.displayed_width(retarget_at);
        motion.retarget(240.0, retarget_at, false);

        assert_eq!(motion.from, displayed);
        assert_eq!(motion.target, 240.0);
        assert_eq!(motion.epoch, 2);
        assert_eq!(motion.displayed_width(retarget_at), displayed);
        assert_eq!(
            motion.displayed_width(retarget_at + motion::PANEL_DURATION),
            240.0
        );
    }

    #[test]
    fn sidebar_width_motion_reduced_motion_snaps_to_target() {
        let started_at = Instant::now();
        let mut motion = SidebarWidthMotion::default();
        motion.retarget(240.0, started_at, false);
        motion.retarget(68.0, started_at, true);

        assert_eq!(motion.from, 68.0);
        assert_eq!(motion.target, 68.0);
        assert_eq!(motion.displayed_width(started_at), 68.0);
    }

    #[test]
    fn sidebar_bottom_motion_initializes_both_fractions_at_the_first_endpoint() {
        let started_at = Instant::now();
        let mut motion = SidebarBottomMotion::default();

        let visual = motion.prepare(false, true, started_at, false);

        assert!(motion.initialized);
        assert_eq!(motion.compact_from, 0.0);
        assert_eq!(motion.compact_target, 0.0);
        assert_eq!(motion.settings_from, 1.0);
        assert_eq!(motion.settings_target, 1.0);
        assert_eq!(visual.fractions_at(0.0), (0.0, 1.0));
        assert_eq!(visual.epoch, 0);
    }

    #[test]
    fn sidebar_bottom_motion_retargets_from_current_fractions_without_jumping() {
        let started_at = Instant::now();
        let mut motion = SidebarBottomMotion::default();
        motion.prepare(false, false, started_at, false);
        motion.prepare(true, true, started_at, false);

        let reversal_at = started_at + Duration::from_millis(40);
        let displayed = motion.displayed_fractions(reversal_at);
        let visual = motion.prepare(false, false, reversal_at, false);

        assert_eq!(motion.compact_from, displayed.0);
        assert_eq!(motion.settings_from, displayed.1);
        assert_eq!(visual.fractions_at(0.0), displayed);
        assert_eq!(visual.compact_target, 0.0);
        assert_eq!(visual.settings_target, 0.0);
        assert_eq!(visual.epoch, 2);
        assert_eq!(motion.displayed_fractions(reversal_at), displayed);
    }

    #[test]
    fn sidebar_bottom_motion_reduced_motion_snaps_both_fractions() {
        let started_at = Instant::now();
        let mut motion = SidebarBottomMotion::default();
        motion.prepare(false, false, started_at, false);

        let visual = motion.prepare(true, true, started_at, true);

        assert_eq!(motion.compact_from, 1.0);
        assert_eq!(motion.compact_target, 1.0);
        assert_eq!(motion.settings_from, 1.0);
        assert_eq!(motion.settings_target, 1.0);
        assert_eq!(visual.fractions_at(0.0), (1.0, 1.0));
    }

    #[test]
    fn right_sidebar_layout_width_has_open_and_close_endpoints() {
        assert_eq!(right_sidebar_layout_width(360.0, false, 0.0), 0.0);
        assert_eq!(right_sidebar_layout_width(360.0, false, 1.0), 360.0);
        assert_eq!(right_sidebar_layout_width(360.0, true, 0.0), 360.0);
        assert_eq!(right_sidebar_layout_width(360.0, true, 1.0), 0.0);
    }

    #[test]
    fn selector_motion_settles_first_render_and_animates_retargets() {
        let start = Instant::now();
        let mut motion = RectSelectorMotion::default();

        motion.retarget(3.0, 70.0, start, false, false);
        assert_eq!(motion.visible_geometry(start), (3.0, 70.0));
        assert_eq!(motion.epoch, 0);

        motion.retarget(75.0, 90.0, start, false, false);
        assert_eq!(motion.epoch, 1);
        assert_eq!(motion.transition, RectSelectorTransition::Interaction);
        assert_eq!(motion.transition.duration(), motion::INTERACTION_DURATION);
        assert_eq!(motion.visible_geometry(start), (3.0, 70.0));
        assert_ne!(
            motion.visible_geometry(start + Duration::from_millis(50)),
            (3.0, 70.0)
        );
        assert_eq!(
            motion.visible_geometry(start + motion::INTERACTION_DURATION),
            (75.0, 90.0)
        );
    }

    #[test]
    fn selector_motion_uses_content_transition_for_responsive_retargets() {
        let start = Instant::now();
        let mut motion = RectSelectorMotion::default();
        motion.retarget(3.0, 70.0, start, false, true);
        motion.retarget(167.0, 122.0, start, false, true);

        assert_eq!(motion.transition, RectSelectorTransition::Responsive);
        assert_eq!(motion.transition.duration(), motion::CONTENT_DURATION);
        let expected = gpui::ease_in_out(0.5);
        let (x, width) = motion.visible_geometry(start + motion::CONTENT_DURATION / 2);
        assert!((x - motion::lerp(3.0, 167.0, expected)).abs() < 0.0001);
        assert!((width - motion::lerp(70.0, 122.0, expected)).abs() < 0.0001);
    }

    #[test]
    fn selector_motion_retargets_from_current_eased_geometry() {
        let start = Instant::now();
        let mut motion = RectSelectorMotion::default();
        motion.retarget(3.0, 70.0, start, false, true);
        motion.retarget(167.0, 122.0, start, false, true);

        let retarget_at = start + Duration::from_millis(40);
        let visible = motion.visible_geometry(retarget_at);
        motion.retarget(3.0, 70.0, retarget_at, false, false);

        assert_eq!(motion.from_x, visible.0);
        assert_eq!(motion.from_width, visible.1);
        assert_eq!(motion.target_x, 3.0);
        assert_eq!(motion.target_width, 70.0);
        assert_eq!(motion.epoch, 2);
        assert_eq!(motion.transition, RectSelectorTransition::Interaction);
        assert_eq!(motion.visible_geometry(retarget_at), visible);
    }

    #[test]
    fn selector_motion_force_rebases_live_geometry_with_an_unchanged_target() {
        let start = Instant::now();
        let mut motion = RectSelectorMotion::default();
        motion.retarget(3.0, 70.0, start, false, false);
        motion.retarget(167.0, 122.0, start, false, false);

        let rebase_at = start + motion::INTERACTION_DURATION / 2;
        let visible = motion.visible_geometry(rebase_at);
        let epoch = motion.epoch;
        motion.retarget(167.0, 122.0, rebase_at, false, true);

        assert_eq!(motion.from_x, visible.0);
        assert_eq!(motion.from_width, visible.1);
        assert_eq!(motion.target_x, 167.0);
        assert_eq!(motion.target_width, 122.0);
        assert_eq!(motion.epoch, epoch + 1);
        assert_eq!(motion.transition, RectSelectorTransition::Responsive);
        assert_eq!(motion.visible_geometry(rebase_at), visible);
    }

    #[test]
    fn selector_motion_reduced_motion_snaps_to_target() {
        let start = Instant::now();
        let mut motion = RectSelectorMotion::default();
        motion.retarget(3.0, 70.0, start, true, true);
        motion.retarget(75.0, 34.0, start, true, true);

        assert_eq!(motion.from_x, 75.0);
        assert_eq!(motion.from_width, 34.0);
        assert_eq!(motion.visible_geometry(start), (75.0, 34.0));
    }
}

mod source_tabs;

impl Render for RalgrumApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let viewport_width = f32::from(viewport.width);
        let metrics =
            crate::music_ui::shell_metrics_for_viewport(viewport_width, f32::from(viewport.height));
        let now = Instant::now();
        if !metrics.narrow_content {
            self.sidebar_width_motion
                .retarget(metrics.sidebar_width, now, cx.reduce_motion());
        }
        let sidebar_width_motion = self.sidebar_width_motion;
        let sidebar_bottom_visual = self.sidebar_bottom_motion.prepare(
            metrics.compact_desktop,
            self.settings_mode,
            now,
            cx.reduce_motion(),
        );
        let sidebar_download_badge_visual = self.sidebar_download_badge_motion.prepare(
            metrics.compact_desktop,
            self.downloads.read(cx).unread_count(),
            now,
            cx.reduce_motion(),
        );
        let (right_sidebar, current, position) = {
            let state = &self.playback.read(cx).state;
            (
                state.right_sidebar,
                state.current().cloned(),
                state.position,
            )
        };
        // The lyrics panel shows the playing track unless a context track was
        // requested from a menu; a context track has no playback position, so
        // synced highlighting stays parked at the top.
        let (lyrics_track, lyrics_follows_playback) = {
            let state = &self.playback.read(cx).state;
            if right_sidebar == RightSidebar::Lyrics {
                (
                    state.lyrics_display_track().cloned(),
                    state.lyrics_follows_playback(),
                )
            } else {
                (state.current().cloned(), true)
            }
        };
        let lyrics_track_identity = lyrics_track.as_ref().map(|t| (t.provider, t.id.clone()));
        let lyrics_track_changed = self.last_lyrics_track_identity != lyrics_track_identity;
        if right_sidebar == RightSidebar::Lyrics || lyrics_track_changed {
            self.last_lyrics_track_identity = lyrics_track_identity;
            self.lyrics.update(cx, |lyrics, cx| {
                lyrics.sync_track(
                    right_sidebar == RightSidebar::Lyrics,
                    lyrics_track.as_ref().map(LyricsTrackInput::from_playback),
                    if lyrics_follows_playback {
                        position.as_secs_f64()
                    } else {
                        0.0
                    },
                    cx,
                );
            });
        }
        let main_page_key = if self.settings_mode {
            "settings"
        } else {
            match self.nav {
                Nav::Discover => "discover",
                Nav::Library => "library",
                Nav::Downloads => "downloads",
                Nav::Cache => "cache",
            }
        };
        let main_page = if self.settings_mode {
            self.settings.clone().into_any_element()
        } else if self.nav == Nav::Discover {
            div()
                .flex_1()
                .min_h_0()
                .child(self.search.clone())
                .into_any_element()
        } else if self.nav == Nav::Library {
            div()
                .flex_1()
                .min_h_0()
                .child(self.library.clone())
                .into_any_element()
        } else if self.nav == Nav::Downloads {
            render_downloads(self, window, cx).into_any_element()
        } else {
            self.cache_view.clone().into_any_element()
        };
        let main_page = div()
            .id("shell-main-page")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .relative()
            .child(main_page)
            .with_animation(
                format!("shell-main-page-{main_page_key}"),
                crate::motion::content(),
                |this, delta| {
                    this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                        .left(px(crate::motion::lerp(8.0, 0.0, delta)))
                },
            );
        let displayed_right_sidebar = self.right_sidebar_transition.displayed_sidebar;
        let right_sidebar_closing = self.right_sidebar_transition.closing;
        let right_sidebar_epoch = self.right_sidebar_transition.epoch;
        let right_sidebar_content_switch = self.right_sidebar_transition.content_switch;
        let right_sidebar_skip_animation = right_sidebar_content_switch
            || (self.right_sidebar_transition.skip_enter && !right_sidebar_closing);
        let right_sidebar_key = match displayed_right_sidebar {
            RightSidebar::Lyrics => "lyrics",
            RightSidebar::Queue => "queue",
            RightSidebar::Closed => "closed",
        };
        let right_sidebar_width = metrics.right_sidebar_width;
        let desktop_sidebar_content = match (right_sidebar_closing, displayed_right_sidebar) {
            (true, _) | (_, RightSidebar::Closed) => div().into_any_element(),
            (false, RightSidebar::Lyrics) => self.lyrics.clone().into_any_element(),
            (false, RightSidebar::Queue) => self.queue.clone().into_any_element(),
        };
        let mobile_sidebar_content = match (right_sidebar_closing, displayed_right_sidebar) {
            (true, _) | (_, RightSidebar::Closed) => div().into_any_element(),
            (false, RightSidebar::Lyrics) => self.lyrics.clone().into_any_element(),
            (false, RightSidebar::Queue) => self.queue.clone().into_any_element(),
        };
        let right_sidebar_is_rendered = displayed_right_sidebar != RightSidebar::Closed;
        div()
            .image_cache(self.artwork_cache.clone())
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .font_family(ui_font_family())
            .text_color(rgb(FOREGROUND))
            .child(render_titlebar(self, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!metrics.narrow_content, |this| {
                        this.child(
                            div()
                                .h_full()
                                .flex_none()
                                .child(render_sidebar(
                                    self,
                                    window,
                                    cx,
                                    sidebar_bottom_visual,
                                    sidebar_download_badge_visual,
                                ))
                                .with_animation(
                                    ("shell-sidebar-width", sidebar_width_motion.epoch),
                                    crate::motion::panel(),
                                    move |this, delta| {
                                        let width = crate::motion::lerp(
                                            sidebar_width_motion.from,
                                            sidebar_width_motion.target,
                                            delta,
                                        );
                                        this.w(px(width)).min_w(px(width))
                                    },
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .when(
                                !self.settings_mode
                                    && self.nav != Nav::Downloads
                                    && self.nav != Nav::Cache,
                                |this| {
                                this.child(render_top_toolbar(self, window, cx))
                                },
                            )
                            .child(main_page),
                    )
                    .when(
                        !self.settings_mode
                            && right_sidebar_is_rendered
                            && !metrics.narrow_content,
                        |this| {
                            let inner_sidebar = div()
                                .h_full()
                                .min_h_0()
                                .w(px(right_sidebar_width))
                                .flex_none()
                                .relative()
                                .border_l_1()
                                .border_color(rgb(BORDER))
                                // Original .right-sidebar-inner uses 24px padding,
                                // but queue and lyrics views manage their own bottom spacing.
                                .pt(px(24.))
                                .px(px(24.))
                                .pb(px(0.))
                                .child(desktop_sidebar_content);
                            let outer_sidebar = div()
                                .h_full()
                                .min_h_0()
                                .flex_none()
                                .overflow_hidden();
                            let desktop_sidebar: AnyElement = if right_sidebar_skip_animation {
                                outer_sidebar
                                    .w(px(right_sidebar_width))
                                    .min_w(px(right_sidebar_width))
                                    .child(inner_sidebar)
                                    .into_any_element()
                            } else {
                                outer_sidebar
                                    .child(inner_sidebar)
                                    .with_animation(
                                        format!(
                                            "shell-right-sidebar-{right_sidebar_key}-{right_sidebar_epoch}"
                                        ),
                                        crate::motion::panel(),
                                        move |this, delta| {
                                            let width = right_sidebar_layout_width(
                                                right_sidebar_width,
                                                right_sidebar_closing,
                                                delta,
                                            );
                                            let this = this.w(px(width)).min_w(px(width));
                                            if right_sidebar_closing {
                                                this.opacity(crate::motion::lerp(1.0, 0.0, delta))
                                                    .right(px(crate::motion::lerp(
                                                        0.0, -14.0, delta,
                                                    )))
                                            } else {
                                                this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                                                    .right(px(crate::motion::lerp(
                                                        -14.0, 0.0, delta,
                                                    )))
                                            }
                                        },
                                    )
                                    .into_any_element()
                            };
                            this.child(desktop_sidebar)
                        },
                    ),
            )
            .when(metrics.narrow_content && self.mobile_sidebar_open, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .top(px(32.))
                        .flex()
                        .child(
                            div()
                                .relative()
                                .w(px(crate::music_ui::MOBILE_DRAWER_WIDTH))
                                .h_full()
                                 .child(render_sidebar(
                                     self,
                                     window,
                                     cx,
                                     sidebar_bottom_visual,
                                     sidebar_download_badge_visual,
                                 ))
                                .with_animation(
                                    "mobile-sidebar-drawer",
                                    crate::motion::panel(),
                                    |this, delta| {
                                        this.opacity(crate::motion::lerp(0.0, 1.0, delta)).left(px(
                                            crate::motion::lerp(
                                                -crate::music_ui::MOBILE_DRAWER_WIDTH,
                                                0.0,
                                                delta,
                                            ),
                                        ))
                                    },
                                ),
                        )
                        .child(
                            div()
                                .id("mobile-sidebar-backdrop")
                                .flex_1()
                                .h_full()
                                .bg(rgb(0x000000))
                                .opacity(0.5)
                                .focusable()
                                .tab_stop(true)
                                .aria_label("Close navigation")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.close_mobile_sidebar(cx)),
                                )
                                .with_animation(
                                    "mobile-sidebar-backdrop-transition",
                                    crate::motion::panel(),
                                    |this, delta| {
                                        this.opacity(crate::motion::lerp(0.0, 0.5, delta))
                                    },
                                ),
                        ),
                )
            })
            .when(
                metrics.narrow_content
                    && !self.settings_mode
                    && right_sidebar_is_rendered,
                |this| {
                    let mobile_sidebar = div()
                        .size_full()
                        .relative()
                        .bg(rgb(BACKGROUND))
                        .child(mobile_sidebar_content);
                    let mobile_sidebar: AnyElement = if right_sidebar_skip_animation {
                        mobile_sidebar.into_any_element()
                    } else {
                        mobile_sidebar
                            .with_animation(
                                format!(
                                    "shell-mobile-right-sidebar-{right_sidebar_key}-{right_sidebar_epoch}"
                                ),
                                crate::motion::panel(),
                                move |this, delta| {
                                    if right_sidebar_closing {
                                        this.opacity(crate::motion::lerp(1.0, 0.0, delta))
                                            .right(px(crate::motion::lerp(
                                                0.0, -14.0, delta,
                                            )))
                                    } else {
                                        this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                                            .right(px(crate::motion::lerp(
                                                -14.0, 0.0, delta,
                                            )))
                                    }
                                },
                            )
                            .into_any_element()
                    };
                    this.child(
                        div()
                            .absolute()
                            .inset_0()
                            .top(px(32.))
                            .child(mobile_sidebar),
                    )
                },
            )
            .child(self.playback_view.clone())
            .child(
                div()
                    .absolute()
                    .bottom(px(if current.is_some() { 110. } else { 24. }))
                    .right(px(24.))
                    .child(self.toasts.clone()),
            )
            .children(Root::render_dialog_layer(window, cx))
            .child(self.app_tooltip.clone())
    }
}
