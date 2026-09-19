use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnimationExt as _, Context, Entity, EventEmitter, FontWeight, IntoElement, KeyDownEvent,
    Render, ScrollHandle, SharedString, Task, Window, div, point, prelude::*, px, rgb,
};
use gpui_component::IndexPath;
use gpui_component::select::{SelectEvent, SelectState};
use gpui_component::{
    input::{InputEvent, InputState},
    scroll::{Scrollbar, ScrollbarShow},
};
use tokio::runtime::Runtime;

use self::general_panel::cache_limit_label;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    diagnostics,
    navigation_state::{
        AppSettings, LyricsSource, RightSidebarView, SettingsError, SettingsStore, StartPage,
    },
    playback::{AudioCache, LIMITS_MB, Overview, asio_drivers, output_devices},
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED},
};
mod about_panel;
mod account_state;
pub(crate) mod action_button;
mod category_nav;
mod general_panel;
mod layout;
mod logout_all_dialog;
mod murglar_panel;
mod murglar_referral;
mod runtime_persistence;
mod service_panel;
mod settings_transfer_dialog;
mod toggle;

pub(crate) use account_state::{AccountState, SidebarPass};
pub(crate) use action_button::{
    DangerSecondaryButtonOptions, danger_secondary_button, neutral_secondary_button,
};
pub(crate) use toggle::{
    MotionTransition, motion_transition, settings_switch, should_apply_deferred_motion_reduction,
};

const SETTINGS_CATEGORY_BREADCRUMB_OFFSET_PX: f32 = 1.;
const SETTINGS_CONTENT_BOTTOM_PADDING_PX: f32 = 16.;
const CACHE_OVERVIEW_POLL_INTERVAL: Duration = Duration::from_millis(500);
const MURGLAR_USERNAME_PLACEHOLDER: &str = "Who are you?";
const MURGLAR_PASSWORD_PLACEHOLDER: &str = "What's the code?";

#[derive(Clone, Copy, Debug, PartialEq)]
struct CacheMeterVisual {
    from: f32,
    target: f32,
    epoch: u64,
    active: bool,
}

#[derive(Debug)]
struct CacheMeterMotion {
    initialized: bool,
    from: f32,
    target: f32,
    epoch: u64,
    started_at: Option<Instant>,
}

impl Default for CacheMeterMotion {
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

impl CacheMeterMotion {
    fn prepare(&mut self, target: f32, now: Instant, reduced_motion: bool) -> CacheMeterVisual {
        let target = target.clamp(0., 1.);

        if !self.initialized {
            self.initialized = true;
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
        } else if reduced_motion
            || (self.started_at.is_some() && self.animation_progress(now) >= 1.)
        {
            self.from = self.target;
            self.started_at = None;
        }

        CacheMeterVisual {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Category {
    General,
    Murglar,
    Providers,
    About,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SettingsEvent {
    ExitRequested,
    Imported(Box<AppSettings>),
}

pub(crate) struct SettingsView {
    category: Category,
    session_active: bool,
    store: Option<SettingsStore>,
    saved: AppSettings,
    pub(super) draft: AppSettings,
    pub(super) save_error: Option<SharedString>,
    import_sync_pending: bool,
    save_in_flight: bool,
    pending_runtime_save: Option<Task<()>>,
    pub(super) account: Entity<AccountState>,
    pub(super) runtime: Arc<Runtime>,
    pub(super) http_client: reqwest::Client,
    pub(super) username: Entity<InputState>,
    pub(super) password: Entity<InputState>,
    pub(super) deezer_arl: Entity<InputState>,
    pub(super) deezer_user_id_input: Entity<InputState>,
    pub(super) soundcloud_desktop: Entity<InputState>,
    pub(super) soundcloud_mobile: Entity<InputState>,
    pub(super) start_page_select: Entity<SelectState<Vec<SharedString>>>,
    pub(super) lyrics_source_select: Entity<SelectState<Vec<SharedString>>>,
    pub(super) output_device_select: Entity<SelectState<Vec<SharedString>>>,
    output_device_refresh_generation: u64,
    pub(super) cache_limit_select: Entity<SelectState<Vec<SharedString>>>,
    pub(super) cache: AudioCache,
    pub(super) cache_overview: Option<Overview>,
    cache_meter_motion: CacheMeterMotion,
    cache_generation: u64,
    cache_overview_loading: bool,
    cache_watcher_generation: u64,
    cache_watcher: Option<Task<()>>,
    pub(super) expanded_murglar_plan: Option<String>,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub(super) fn refresh_cache_overview(&mut self, cx: &mut Context<Self>) {
        self.cache_generation = self.cache_generation.wrapping_add(1);
        let generation = self.cache_generation;
        let cache = self.cache.clone();
        self.cache_overview_loading = true;
        let task = self.runtime.spawn(async move { cache.overview().await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The cache worker stopped unexpectedly".into()));
            this.update(cx, |this, cx| {
                if generation != this.cache_generation {
                    return;
                }
                this.cache_overview_loading = false;
                match result {
                    Ok(overview) => this.cache_overview = Some(overview),
                    Err(error) => this.save_error = Some(cache_error("read", &error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn clear_audio_cache(&mut self, cx: &mut Context<Self>) {
        self.cache_generation = self.cache_generation.wrapping_add(1);
        let generation = self.cache_generation;
        let cache = self.cache.clone();
        self.cache_overview_loading = true;
        let task = self.runtime.spawn(async move { cache.clear().await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The cache worker stopped unexpectedly".into()));
            this.update(cx, |this, cx| {
                if generation != this.cache_generation {
                    return;
                }
                this.cache_overview_loading = false;
                match result {
                    Ok(overview) => {
                        this.cache_overview = Some(overview);
                        this.save_error = None;
                    }
                    Err(error) => this.save_error = Some(cache_error("clear", &error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn new(
        account: Entity<AccountState>,
        runtime: Arc<Runtime>,
        store: Result<SettingsStore, SettingsError>,
        cache: AudioCache,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&account, |_, _, cx| cx.notify()).detach();
        let (store, saved, save_error) = match store {
            Ok(store) => {
                let saved = store.settings().clone();
                (Some(store), saved, None)
            }
            Err(error) => (None, AppSettings::default(), Some(error.to_string().into())),
        };
        let cache_for_refresh = cache.clone();
        let initial_cache_generation = 0;
        let task = runtime.spawn(async move { cache_for_refresh.overview().await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("The cache worker stopped unexpectedly".into()));
            this.update(cx, |this, cx| {
                if initial_cache_generation != this.cache_generation {
                    return;
                }
                this.cache_overview_loading = false;
                match result {
                    Ok(overview) => this.cache_overview = Some(overview),
                    Err(error) => this.save_error = Some(cache_error("read", &error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        let saved_for_selects = saved.clone();
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(MURGLAR_USERNAME_PLACEHOLDER));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(MURGLAR_PASSWORD_PLACEHOLDER)
                .masked(true)
        });
        cx.subscribe_in(
            &password,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit_login(window, cx);
                }
            },
        )
        .detach();
        let this = Self {
            category: Category::General,
            session_active: false,
            draft: saved_for_selects.clone(),
            saved,
            store,
            save_error,
            import_sync_pending: false,
            save_in_flight: false,
            pending_runtime_save: None,
            account,
            runtime,
            http_client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .build()
                .expect("settings HTTP client configuration should be valid"),
            username,
            password,
            deezer_arl: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste Deezer ARL cookie")
                    .masked(true)
            }),
            deezer_user_id_input: cx
                .new(|cx| InputState::new(window, cx).placeholder("Optional Deezer user ID")),
            soundcloud_desktop: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Paste desktop OAuth token")
                    .masked(true)
            }),
            soundcloud_mobile: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Optional mobile OAuth token")
                    .masked(true)
            }),
            start_page_select: Self::make_select(
                StartPage::ALL
                    .iter()
                    .map(|page| SharedString::from(page.label())),
                Some(Self::self_index(
                    saved_for_selects.start_page,
                    &StartPage::ALL,
                )),
                window,
                cx,
            ),
            lyrics_source_select: Self::make_select(
                LyricsSource::ALL
                    .iter()
                    .map(|source| SharedString::from(source.label())),
                Some(Self::self_index(
                    saved_for_selects.lyrics_source,
                    &LyricsSource::ALL,
                )),
                window,
                cx,
            ),
            output_device_select: { Self::make_select(std::iter::empty(), None, window, cx) },
            output_device_refresh_generation: 0,
            cache_limit_select: Self::make_select(
                LIMITS_MB
                    .iter()
                    .map(|limit| SharedString::from(cache_limit_label(*limit))),
                LIMITS_MB
                    .iter()
                    .position(|limit| *limit == saved_for_selects.audio_cache_limit_mb),
                window,
                cx,
            ),
            cache,
            cache_overview: None,
            cache_meter_motion: CacheMeterMotion::default(),
            cache_generation: initial_cache_generation,
            cache_overview_loading: true,
            cache_watcher_generation: 0,
            cache_watcher: None,
            expanded_murglar_plan: None,
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
        };
        let mut view = this;
        view.subscribe_selects(cx);
        view.refresh_output_device_select(window, cx);
        view.install_runtime_settings_flush(cx);
        view
    }

    fn make_select(
        labels: impl IntoIterator<Item = SharedString>,
        selected: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SelectState<Vec<SharedString>>> {
        let items: Vec<SharedString> = labels.into_iter().collect();
        let selected_index = selected.map(|row| IndexPath::default().row(row));
        cx.new(|cx| SelectState::new(items, selected_index, window, cx))
    }

    fn self_index<T: PartialEq>(value: T, all: &[T]) -> usize {
        all.iter().position(|item| *item == value).unwrap_or(0)
    }

    /// Entries and selected row for the output device picker. WASAPI mode
    /// lists the system default plus devices and selects the saved device;
    /// ASIO mode lists the registry drivers and selects the saved driver,
    /// falling back to the first entry when nothing valid is saved.
    fn output_device_picker(
        asio_mode: bool,
        output_device: Option<&str>,
        asio_driver: Option<&str>,
    ) -> (Vec<SharedString>, Option<usize>) {
        let options = asio_drivers::picker_options(
            asio_mode,
            &asio_drivers::list_registry_asio_drivers(),
            output_devices::output_device_options(&output_devices::list_output_devices()),
        );
        if !asio_mode
            && let Some(name) = output_device
            && !options.iter().any(|option| option == name)
        {
            diagnostics::event(
                "WARN",
                format!("the saved output device \"{name}\" is not available"),
            );
        }
        let saved = if asio_mode {
            asio_driver
        } else {
            output_device
        };
        let selected = output_devices::output_device_selected_index(&options, saved);
        let has_rows = !options.is_empty();
        (
            options.into_iter().map(SharedString::from).collect(),
            has_rows.then_some(selected),
        )
    }

    /// Rebuilds the output device picker for the current draft. The row
    /// list depends on the ASIO mode, so the toggle and every session
    /// reset refresh both the items and the selected row.
    fn refresh_output_device_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        self.output_device_refresh_generation =
            self.output_device_refresh_generation.wrapping_add(1);
        let generation = self.output_device_refresh_generation;
        let asio_mode = self.draft.asio_mode;
        let output_device = self.draft.output_device.clone();
        let asio_driver = self.draft.asio_driver.clone();
        let task = self.runtime.spawn_blocking(move || {
            Self::output_device_picker(asio_mode, output_device.as_deref(), asio_driver.as_deref())
        });
        cx.spawn(async move |this, cx| {
            let Ok((labels, selected)) = task.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if this.output_device_refresh_generation != generation {
                    return;
                }
                this.output_device_select.update(cx, |select, cx| {
                    select.set_items(labels, window, cx);
                    select.set_selected_index(
                        selected.map(|row| IndexPath::default().row(row)),
                        window,
                        cx,
                    );
                });
            })
            .ok();
        })
        .detach();
    }

    fn subscribe_selects(&mut self, cx: &mut Context<Self>) {
        cx.subscribe(
            &self.start_page_select,
            |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && let Some(page) = StartPage::ALL
                        .iter()
                        .find(|page| page.label() == value.as_ref())
                {
                    this.update_draft_and_persist(|draft| draft.start_page = *page, cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &self.lyrics_source_select,
            |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && let Some(source) = LyricsSource::ALL
                        .iter()
                        .find(|item| item.label() == value.as_ref())
                {
                    this.update_draft_and_persist(|draft| draft.lyrics_source = *source, cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &self.cache_limit_select,
            |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event
                    && let Some(limit) = LIMITS_MB
                        .iter()
                        .find(|limit| cache_limit_label(**limit).as_str() == value.as_ref())
                {
                    this.update_draft_and_persist(|draft| draft.audio_cache_limit_mb = *limit, cx);
                }
            },
        )
        .detach();
        cx.subscribe(
            &self.output_device_select,
            |this, _, event: &SelectEvent<Vec<SharedString>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if this.draft.asio_mode {
                        this.update_draft_and_persist(
                            |draft| draft.asio_driver = Some(value.to_string()),
                            cx,
                        );
                    } else {
                        let device =
                            if value.as_ref() == output_devices::SYSTEM_DEFAULT_OUTPUT_LABEL {
                                None
                            } else {
                                Some(value.to_string())
                            };
                        this.update_draft_and_persist(|draft| draft.output_device = device, cx);
                    }
                }
            },
        )
        .detach();
    }

    pub(crate) fn begin_session(
        &mut self,
        initial_category: Category,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        diagnostics::event("INFO", "settings session begin");
        self.stop_cache_watcher();
        self.session_active = true;
        self.save_in_flight = false;
        self.cache_generation = self.cache_generation.wrapping_add(1);
        self.draft = self.saved.clone();
        self.category = initial_category;
        self.expanded_murglar_plan = None;
        murglar_panel::reset_murglar_plan_motion();
        self.save_error = None;
        self.clear_session_inputs(window, cx);
        self.reset_selects(window, cx);
        self.refresh_cache_overview(cx);
        self.start_cache_watcher(cx);
        self.account.update(cx, |account, cx| {
            account.begin_settings_session();
            cx.notify();
        });
        self.select_category(initial_category, window, cx);
        // The session start cleared the account extras, so prefetch the
        // Murglar profile, referral, and plans now instead of waiting for
        // the Murglar category to be opened.
        self.open_murglar_profile(cx);
        cx.notify();
        diagnostics::event("INFO", "settings session ready");
    }

    fn clear_session_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [
            &self.username,
            &self.password,
            &self.deezer_arl,
            &self.deezer_user_id_input,
            &self.soundcloud_desktop,
            &self.soundcloud_mobile,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn reset_selects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start_page = Self::self_index(self.draft.start_page, &StartPage::ALL);
        let lyrics_source = Self::self_index(self.draft.lyrics_source, &LyricsSource::ALL);
        let cache_limit = LIMITS_MB
            .iter()
            .position(|limit| *limit == self.draft.audio_cache_limit_mb);
        self.start_page_select.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::default().row(start_page)), window, cx)
        });
        self.lyrics_source_select.update(cx, |select, cx| {
            select.set_selected_index(Some(IndexPath::default().row(lyrics_source)), window, cx)
        });
        self.cache_limit_select.update(cx, |select, cx| {
            select.set_selected_index(
                cache_limit.map(|row| IndexPath::default().row(row)),
                window,
                cx,
            )
        });
        self.refresh_output_device_select(window, cx);
    }

    pub(crate) fn select_category(
        &mut self,
        category: Category,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.category != category {
            self.browser_scroll.reset();
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
        self.category = category;
        if category == Category::Murglar {
            let should_fetch = {
                let account = self.account.read(cx);
                // Starting a settings session keeps the cached profile so the
                // sidebar remains useful, but it clears the account extras. A
                // cached profile therefore cannot be used as a signal that the
                // referral and payment requests have already run.
                !account.loading
                    && (account.profile.is_none()
                        || account.referral.is_none()
                        || account.plans.is_none())
            };
            if should_fetch {
                self.open_murglar_profile(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn category(&self) -> Category {
        self.category
    }

    pub(crate) fn saved(&self) -> &AppSettings {
        &self.saved
    }

    pub(crate) fn finish_import_sync(&mut self) {
        self.import_sync_pending = false;
    }

    pub(super) fn update_draft_and_persist(
        &mut self,
        update: impl FnOnce(&mut AppSettings),
        cx: &mut Context<Self>,
    ) {
        update(&mut self.draft);
        let result = prepare_settings_for_persistence(&self.draft, &self.saved)
            .and_then(|settings| self.persist_settings_background(settings, true, cx));
        match result {
            Ok(()) => diagnostics::event("INFO", "settings change queued for persistence"),
            Err(error) => self.save_error = Some(error),
        }
        cx.notify();
    }

    pub(crate) fn persist_navigation(
        &mut self,
        settings: AppSettings,
        always: bool,
        cx: &mut Context<Self>,
    ) {
        if self.import_sync_pending {
            return;
        }
        if !always && !self.saved.remember_navigation {
            return;
        }
        if let Err(error) = self.persist_settings_background(settings, false, cx) {
            self.save_error = Some(error);
        }
    }

    pub(crate) fn persist_search_history(
        &mut self,
        search_history: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.import_sync_pending {
            return;
        }
        if self.saved.search_history == search_history {
            return;
        }
        let mut settings = self.saved.clone();
        settings.search_history = search_history;
        if let Err(error) = self.persist_settings_background(settings, false, cx) {
            self.save_error = Some(error);
        }
    }

    pub(crate) fn request_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_in_flight {
            return;
        }
        diagnostics::event("INFO", "settings submission");
        let settings = match prepare_settings_for_persistence(&self.draft, &self.saved) {
            Ok(settings) => settings,
            Err(error) => {
                self.save_error = Some(error);
                cx.notify();
                return;
            }
        };
        let Some(store) = self.store.as_ref() else {
            self.save_error = Some("Settings storage is unavailable.".into());
            cx.notify();
            return;
        };
        let write = store.prepare_persist(settings.clone());
        self.saved = settings.clone();
        self.draft = settings;
        self.save_error = None;
        self.save_in_flight = true;

        let runtime = self.runtime.clone();
        cx.spawn_in(window, async move |this, cx| {
            let worker_write = write.clone();
            let result = runtime.spawn_blocking(move || worker_write.persist()).await;
            this.update_in(cx, |this, window, cx| {
                this.save_in_flight = false;
                let persistence_error = match result {
                    Ok(Ok(_)) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("The settings writer stopped unexpectedly.".to_owned()),
                };
                if write.is_current()
                    && let Some(error) = persistence_error
                {
                    this.save_error = Some(error.into());
                    cx.notify();
                    return;
                }
                if let Some(store) = this.store.as_mut() {
                    store.accept_persisted(&write);
                }
                this.finish_settings_session(window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_settings_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_cache_watcher();
        self.cache_generation = self.cache_generation.wrapping_add(1);
        self.cache_overview_loading = false;
        self.session_active = false;
        self.clear_session_inputs(window, cx);
        self.account.update(cx, |account, cx| {
            account.end_settings_session();
            cx.notify();
        });
        cx.emit(SettingsEvent::ExitRequested);
        diagnostics::event("INFO", "settings submission complete");
        cx.notify();
    }

    pub(crate) fn logout_all(&mut self, cx: &mut Context<Self>) {
        self.account.update(cx, |account, cx| {
            account.logout_all(cx);
            cx.notify();
        });
    }

    fn stop_cache_watcher(&mut self) {
        self.cache_watcher_generation = self.cache_watcher_generation.wrapping_add(1);
        self.cache_watcher = None;
    }

    fn start_cache_watcher(&mut self, cx: &mut Context<Self>) {
        self.stop_cache_watcher();
        let generation = self.cache_watcher_generation;
        let cache = self.cache.clone();
        let runtime = self.runtime.clone();
        let executor = cx.background_executor().clone();
        let mut last_revision = cache.revision();
        let mut refresh_after_loading = true;

        self.cache_watcher = Some(cx.spawn(async move |this, cx| {
            loop {
                executor.timer(CACHE_OVERVIEW_POLL_INTERVAL).await;
                let state = this.update(cx, |this, _| {
                    if !this.session_active || this.cache_watcher_generation != generation {
                        return None;
                    }
                    Some((
                        this.cache.revision(),
                        this.cache_overview_loading,
                        this.cache_generation,
                    ))
                });
                let Ok(Some((revision, loading, overview_generation))) = state else {
                    break;
                };
                if loading {
                    refresh_after_loading = true;
                    continue;
                }
                if !cache_overview_refresh_needed(last_revision, revision, refresh_after_loading) {
                    continue;
                }
                refresh_after_loading = false;

                let result = spawn_cache_overview(runtime.as_ref(), cache.clone())
                    .await
                    .unwrap_or_else(|_| Err("The cache worker stopped unexpectedly".into()));
                let applied = this.update(cx, |this, cx| {
                    if !this.session_active || this.cache_watcher_generation != generation {
                        return None;
                    }
                    if this.cache_generation != overview_generation || this.cache_overview_loading {
                        return Some(false);
                    }
                    match result {
                        Ok(overview) => this.cache_overview = Some(overview),
                        Err(error) => this.save_error = Some(cache_error("read", &error)),
                    }
                    cx.notify();
                    Some(true)
                });
                match applied {
                    Ok(Some(true)) => last_revision = revision,
                    Ok(Some(false)) => last_revision = revision,
                    _ => break,
                }
            }
        }));
    }
}

fn prepare_settings_for_persistence(
    draft: &AppSettings,
    saved: &AppSettings,
) -> Result<AppSettings, SharedString> {
    let mut settings = draft.clone();
    settings.merge_live_state_from(saved);
    settings
        .validate_downloads_dir()
        .map_err(|error| SharedString::from(error.to_string()))?;
    Ok(settings)
}

fn cache_error(operation: &str, error: &str) -> SharedString {
    format!("Could not {operation} the audio cache: {error}").into()
}

fn spawn_cache_overview(
    runtime: &Runtime,
    cache: AudioCache,
) -> tokio::task::JoinHandle<Result<Overview, String>> {
    runtime.spawn(async move { cache.overview().await })
}

fn cache_revision_changed(previous: u64, current: u64) -> bool {
    previous != current
}

fn cache_overview_refresh_needed(
    last_revision: u64,
    current_revision: u64,
    refresh_after_loading: bool,
) -> bool {
    refresh_after_loading || cache_revision_changed(last_revision, current_revision)
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let viewport_width = f32::from(viewport.width);
        let header_padding = layout::content_padding(viewport_width);
        let category = self.category;
        let panel = div()
            .w_full()
            .relative()
            .child(match category {
                Category::General => self.render_general(cx).into_any_element(),
                Category::Murglar => self.render_murglar(cx).into_any_element(),
                Category::Providers => self.render_providers(cx).into_any_element(),
                Category::About => self.render_about(cx).into_any_element(),
            })
            .with_animation(
                format!("settings-category-body-{}", category.label()),
                crate::motion::quick_content(),
                move |this, delta| {
                    this.opacity(crate::motion::lerp(0.0, 1.0, delta))
                        .top(px(crate::motion::lerp(6.0, 0.0, delta)))
                },
            )
            .into_any_element();
        let scroll_content = div()
            .id("settings-panel-scroll-content")
            .size_full()
            .min_h_0()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .px(px(header_padding))
            .pt(px(24.))
            // Breathing room above the player bar. Padding lives inside the
            // scrollable viewport so it only shows at the tail and the
            // scrollbar track still matches the viewport height.
            .pb(px(SETTINGS_CONTENT_BOTTOM_PADDING_PX))
            .child(
                div()
                    .w_full()
                    .max_w(px(layout::SETTINGS_CONTENT_MAX_WIDTH))
                    .mx_auto()
                    .when_some(self.save_error.clone(), |this, error| {
                        this.child(
                            div()
                                .mb(px(12.))
                                .text_size(px(12.))
                                .text_color(rgb(0xf87171))
                                .child(error),
                        )
                    })
                    .child(panel),
            );
        let scroll_viewport = div()
            .id("settings-panel-scroll-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .child(scroll_content)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.scroll).scrollbar_show(ScrollbarShow::Hover)),
            )
            .into_any_element();
        let scroll_panel = browser_scroll_surface(
            "settings-panel-scroll",
            scroll_viewport,
            BrowserScrollTarget::Handle(self.scroll.clone()),
            self.browser_scroll.clone(),
        );
        div()
            .id("settings-page")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .text_size(px(13.))
            // Escape leaves the settings page through the same save path
            // as the sidebar back button, so drafts persist and the session
            // tears down. Inputs see the key first: an open input popover
            // or inline completion is dismissed before the page exits,
            // which matches how desktop forms layer escape. Modal dialogs
            // render in overlays and keep the event for themselves.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && this.session_active {
                    window.prevent_default();
                    cx.stop_propagation();
                    this.request_save(window, cx);
                }
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(header_padding))
                    .py(px(17.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(local_icon(LocalIcon::Settings, FOREGROUND).size_4())
                            .child("Settings"),
                    )
                    .child(div().text_color(rgb(MUTED)).child("/"))
                    .child(
                        div()
                            .relative()
                            .top(px(SETTINGS_CATEGORY_BREADCRUMB_OFFSET_PX))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .child(
                                local_icon(self.category.icon(), self.category.active_icon_color())
                                    .size(px(14.)),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(MUTED))
                                    .child(self.category.label()),
                            ),
                    ),
            )
            .child(scroll_panel)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CACHE_OVERVIEW_POLL_INTERVAL, CacheMeterMotion, Category, MURGLAR_PASSWORD_PLACEHOLDER,
        MURGLAR_USERNAME_PLACEHOLDER, SETTINGS_CATEGORY_BREADCRUMB_OFFSET_PX, cache_error,
        cache_overview_refresh_needed, cache_revision_changed, prepare_settings_for_persistence,
        spawn_cache_overview,
    };
    use crate::navigation_state::{AppSettings, MainDestination, StartPage};
    use crate::playback::AudioCache;
    use std::time::{Duration, Instant};
    use tokio::runtime::Runtime;

    #[test]
    fn persistence_preserves_live_navigation_state() {
        let mut draft = AppSettings::default();
        let mut saved = AppSettings::default();
        draft.start_page = StartPage::Library;
        draft.last_main_tab = MainDestination::Discover;
        draft.search_history = vec!["stale".into()];
        saved.last_main_tab = MainDestination::Downloads;
        saved.search_history = vec!["Skrillex".into()];

        let prepared = prepare_settings_for_persistence(&draft, &saved).unwrap();

        assert_eq!(prepared.start_page, StartPage::Library);
        assert_eq!(prepared.last_main_tab, MainDestination::Downloads);
        assert_eq!(prepared.search_history, ["Skrillex"]);
    }

    #[test]
    fn cache_mutation_errors_are_actionable_settings_messages() {
        assert_eq!(
            cache_error("clear", "access denied").as_ref(),
            "Could not clear the audio cache: access denied"
        );
    }

    #[test]
    fn category_breadcrumb_offset_preserves_the_header_and_moves_only_the_category() {
        assert_eq!(SETTINGS_CATEGORY_BREADCRUMB_OFFSET_PX, 1.);
    }

    #[test]
    fn murglar_login_placeholders_match_the_compact_login_copy() {
        assert_eq!(MURGLAR_USERNAME_PLACEHOLDER, "Who are you?");
        assert_eq!(MURGLAR_PASSWORD_PLACEHOLDER, "What's the code?");
    }

    #[test]
    fn provider_settings_use_one_shared_category() {
        assert_eq!(
            Category::ALL,
            [
                Category::General,
                Category::Murglar,
                Category::Providers,
                Category::About
            ]
        );
        assert_eq!(Category::ALL.len(), 4);
        assert_eq!(Category::ALL.last(), Some(&Category::About));
        assert_eq!(Category::Providers.label(), "Providers");
        assert_eq!(
            Category::Providers.icon().path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/server.svg"
        );
        assert_eq!(Category::Providers.index(), 2);
        assert_eq!(Category::About.label(), "About");
        assert_eq!(
            Category::About.icon().path(),
            "ralgrum/icons/fontawesome-free-7.3.1/solid/circle-info.svg"
        );
        assert_eq!(Category::About.index(), 3);
    }

    #[test]
    fn cache_overview_watcher_only_refreshes_after_a_revision_change() {
        assert_eq!(
            CACHE_OVERVIEW_POLL_INTERVAL,
            std::time::Duration::from_millis(500)
        );
        assert!(!cache_revision_changed(7, 7));
        assert!(cache_revision_changed(7, 8));
    }

    #[test]
    fn cache_overview_watcher_refreshes_after_initial_loading() {
        assert!(cache_overview_refresh_needed(7, 7, true));
        assert!(cache_overview_refresh_needed(7, 8, false));
        assert!(!cache_overview_refresh_needed(7, 7, false));
    }

    #[test]
    fn cache_overview_task_runs_filesystem_work_on_tokio_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::new().unwrap();
        let task = spawn_cache_overview(&runtime, AudioCache::new(directory.path().into(), 256));

        let overview = runtime.block_on(task).unwrap().unwrap();

        assert_eq!(overview.block_count, 0);
    }

    #[test]
    fn cache_meter_animates_increases() {
        let start = Instant::now();
        let mut motion = CacheMeterMotion::default();

        assert!(!motion.prepare(0., start, false).active);
        let visual = motion.prepare(0.75, start, false);

        assert_eq!(visual.from, 0.);
        assert_eq!(visual.target, 0.75);
        assert_eq!(visual.epoch, 1);
        assert!(visual.active);

        let settled = motion.prepare(0.75, start + crate::motion::CONTENT_DURATION, false);
        assert_eq!(settled.from, 0.75);
        assert_eq!(settled.target, 0.75);
        assert!(!settled.active);
    }

    #[test]
    fn cache_meter_animates_decreases_to_zero() {
        let start = Instant::now();
        let mut motion = CacheMeterMotion::default();

        motion.prepare(0.75, start, false);
        let visual = motion.prepare(0., start + Duration::from_millis(1), false);

        assert_eq!(visual.from, 0.75);
        assert_eq!(visual.target, 0.);
        assert_eq!(visual.epoch, 1);
        assert!(visual.active);

        let settled = motion.prepare(0., start + Duration::from_millis(181), false);
        assert_eq!(settled.from, 0.);
        assert_eq!(settled.target, 0.);
        assert!(!settled.active);
    }

    #[test]
    fn cache_meter_retargets_from_the_current_eased_midpoint() {
        let start = Instant::now();
        let mut motion = CacheMeterMotion::default();

        motion.prepare(0., start, false);
        motion.prepare(1., start, false);
        let visual = motion.prepare(0.25, start + crate::motion::CONTENT_DURATION / 2, false);

        assert!((visual.from - gpui::ease_in_out(0.5)).abs() < 0.0001);
        assert_eq!(visual.target, 0.25);
        assert_eq!(visual.epoch, 2);
        assert!(visual.active);
    }

    #[test]
    fn cache_meter_same_target_does_not_restart() {
        let start = Instant::now();
        let mut motion = CacheMeterMotion::default();

        motion.prepare(0., start, false);
        let first = motion.prepare(0.75, start, false);
        let started_at = motion.started_at;
        let same = motion.prepare(0.75, start + Duration::from_millis(50), false);

        assert_eq!(same.epoch, first.epoch);
        assert_eq!(motion.started_at, started_at);
        assert!(same.active);

        let settled = motion.prepare(0.75, start + crate::motion::CONTENT_DURATION, false);
        let stable = motion.prepare(
            0.75,
            start + crate::motion::CONTENT_DURATION + Duration::from_millis(1),
            false,
        );
        assert!(!settled.active);
        assert!(!stable.active);
        assert_eq!(stable.epoch, first.epoch);
    }
}
