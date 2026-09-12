#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod cache;
mod context_menu;
mod domain;
mod downloads;
mod integrations;
mod library;
mod lyrics;
mod murglar_backend;
mod platform;
mod playback;
mod search;
mod settings;
mod shell;
mod ui;

pub(crate) use app::{
    account_session, diagnostics, entity_navigation, navigation_state, paths, settings_transfer,
    window_state,
};
pub(crate) use domain::smart_mix_title;
pub(crate) use integrations::{discord, service_auth, service_auth_webview};
pub(crate) use platform::{browser_link, external_url, media_control, tray};
#[cfg(windows)]
pub(crate) use platform::{windows_chrome, windows_protocol, windows_taskbar};
pub(crate) use ui::{
    app_button, app_tooltip, artwork_cache, assets, browser_scroll, browser_scroll_cursor,
    collection_detail, collection_empty, dialog_layout, drag_cursor, empty_state, motion, music_ui,
    playing_indicator, tab_keyboard, theme, toast,
};

use std::sync::Arc;

use gpui::{App, TitlebarOptions, WindowOptions, prelude::*, px, size};
use gpui_component::Root;
use gpui_platform::application;
use reqwest_client::ReqwestClient;
use tokio::runtime::Builder;

use assets::AppAssets;
use playback::AudioCache;
use settings::{AccountState, SettingsView};
use shell::RalgrumApp;

#[cfg(windows)]
fn start_browser_link_pump(
    cx: &mut App,
    shell: gpui::WeakEntity<RalgrumApp>,
    mut events: futures::channel::mpsc::UnboundedReceiver<
        platform::windows_browser_link::BrowserLinkEvent,
    >,
) {
    use futures::StreamExt as _;

    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            let alive = cx.update(|cx| match event {
                platform::windows_browser_link::BrowserLinkEvent::Restore => {
                    tray::restore_main_window(cx);
                    true
                }
                platform::windows_browser_link::BrowserLinkEvent::Open(entity) => shell
                    .update(cx, |app, cx| {
                        tray::restore_main_window(cx);
                        app.open_browser_link(entity, cx)
                    })
                    .is_ok(),
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

fn main() {
    diagnostics::init();
    diagnostics::event("INFO", "startup begin");
    #[cfg(windows)]
    windows_protocol::sync_registration();
    let raw_browser_link =
        browser_link::find_browser_link_arg(&std::env::args().collect::<Vec<_>>());
    if raw_browser_link.is_some()
        && raw_browser_link
            .as_deref()
            .and_then(browser_link::parse_ralgrum_url)
            .is_none()
    {
        diagnostics::event("WARN", "ignored invalid browser link argument");
    }
    #[cfg(windows)]
    let mut browser_launch = platform::windows_browser_link::prepare(raw_browser_link.clone());
    #[cfg(windows)]
    if matches!(
        &browser_launch,
        platform::windows_browser_link::BrowserLinkLaunch::Forwarded
            | platform::windows_browser_link::BrowserLinkLaunch::Unavailable
    ) {
        if matches!(
            &browser_launch,
            platform::windows_browser_link::BrowserLinkLaunch::Forwarded
        ) {
            diagnostics::event("INFO", "secondary launch forwarded to running app");
        } else {
            diagnostics::event("ERROR", "single-instance startup unavailable");
        }
        return;
    }
    #[cfg(not(windows))]
    let initial_browser_links = raw_browser_link
        .as_deref()
        .and_then(browser_link::parse_ralgrum_url)
        .into_iter()
        .collect::<Vec<_>>();
    eprintln!("ralgruM GPUI starting");
    let http_client = Arc::new(
        ReqwestClient::user_agent("ralgrum-gpui").expect("failed to create image HTTP client"),
    );
    application()
        .with_assets(AppAssets)
        .with_http_client(http_client)
        .run(move |cx: &mut App| {
            diagnostics::event("INFO", "GPUI application callback entered");
            #[cfg(windows)]
            browser_launch.install_owner(cx);
            #[cfg(windows)]
            let mut initial_browser_links = browser_launch.take_initial();
            #[cfg(windows)]
            let browser_events = browser_launch.take_events();
            let device_identity = murglar_backend::load_current_user();
            let account_session = account_session::SessionStore::load_current_user();
            let app_settings = navigation_state::SettingsStore::load_current_user();
            let restore_window = app_settings
                .as_ref()
                .map(|store| store.settings().restore_window)
                .unwrap_or_else(|_| navigation_state::AppSettings::default().restore_window);
            let window_state = window_state::WindowStateStore::load_current_user();
            gpui_component::init(cx);
            context_menu::init(cx);
            theme::configure_component_theme(cx);

            let runtime = Arc::new(
                Builder::new_multi_thread()
                    .worker_threads(std::cmp::min(
                        std::thread::available_parallelism()
                            .map(|p| p.get())
                            .unwrap_or(4),
                        4,
                    ))
                    .enable_all()
                    .build()
                    .expect("failed to start async runtime"),
            );
            let displays = cx
                .displays()
                .iter()
                .map(|display| display.bounds())
                .collect::<Vec<_>>();
            let bounds = window_state::startup_bounds(
                restore_window,
                window_state
                    .as_ref()
                    .and_then(window_state::WindowStateStore::saved),
                &displays,
                cx,
            );
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(bounds),
                    inactive_frame_interval: None,
                    titlebar: Some(TitlebarOptions {
                        title: Some("ralgruM".into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    app_owns_titlebar_drag: true,
                    window_min_size: Some(size(px(900.), px(620.))),
                    ..Default::default()
                },
                move |window, cx| {
                    diagnostics::event("INFO", "main window construction begin");
                    let account = cx.new(|_| AccountState::new(device_identity, account_session));
                    let cache_dir = paths::cache_dir().join("audio-v1");
                    let saved = app_settings.as_ref().map_or_else(
                        |_| navigation_state::AppSettings::default(),
                        |store| store.settings().clone(),
                    );
                    let cache = AudioCache::new(cache_dir, saved.audio_cache_limit_mb);
                    if let Err(error) = runtime.block_on(cache.purge_partial_prefetch()) {
                        diagnostics::event(
                            "WARN",
                            format!("audio cache prefetch cleanup failed: {error}"),
                        );
                    }
                    let settings = cx.new(|cx| {
                        SettingsView::new(
                            account.clone(),
                            runtime.clone(),
                            app_settings,
                            cache.clone(),
                            window,
                            cx,
                        )
                    });
                    settings.update(cx, |settings, cx| settings.fetch_startup_profile(cx));
                    let window_state = cx.new(|cx| {
                        window_state::WindowStateManager::new(
                            settings.clone(),
                            window_state,
                            window,
                            cx,
                        )
                    });
                    let view = cx.new(|cx| {
                        RalgrumApp::new(
                            settings.clone(),
                            account,
                            runtime.clone(),
                            window_state.clone(),
                            window,
                            cx,
                        )
                    });
                    let tray_available = tray::install(cx, window, view.downgrade());
                    for entity in initial_browser_links.drain(..) {
                        view.update(cx, |app, cx| app.open_browser_link(entity, cx));
                    }
                    #[cfg(windows)]
                    if let Some(events) = browser_events {
                        start_browser_link_pump(cx, view.downgrade(), events);
                    }
                    let window_state_on_close = window_state.downgrade();
                    let settings_on_close = settings.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        let keep_running = settings_on_close
                            .upgrade()
                            .map(|settings| settings.read(cx).saved().close_to_tray)
                            .unwrap_or(false);
                        window_state_on_close
                            .update(cx, |state, cx| state.persist_now(window, cx))
                            .ok();
                        if tray::should_hide_on_close(keep_running, tray_available)
                            && !tray::quit_requested(cx)
                        {
                            tray::hide_window(window);
                            return false;
                        }
                        true
                    });
                    diagnostics::event("INFO", "main window construction complete");
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
            diagnostics::event("INFO", "GPUI application activated");
        });
}
