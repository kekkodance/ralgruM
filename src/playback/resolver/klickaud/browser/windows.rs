use std::{
    num::NonZeroIsize,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute},
            Gdi::{
                GetMonitorInfoW, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
            },
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetModuleHandleW,
            Threading::GetCurrentProcessId,
        },
        UI::{
            HiDpi::{GetDpiForSystem, GetDpiForWindow},
            Input::KeyboardAndMouse::ReleaseCapture,
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, EnumWindows, GetForegroundWindow, GetSystemMetrics,
                GetWindowRect, GetWindowThreadProcessId, HTCAPTION, IDC_ARROW, IsIconic, IsWindow,
                IsWindowVisible, LoadCursorW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, PeekMessageW,
                RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_MINIMIZE, SW_RESTORE,
                SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, ShowWindowAsync,
                TranslateMessage, WM_CLOSE, WM_DESTROY, WM_NCCALCSIZE, WM_NCLBUTTONDOWN, WM_QUIT,
                WNDCLASSW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_MINIMIZEBOX,
                WS_SYSMENU,
            },
        },
    },
    core::{BOOL, w},
};
use wry::{
    BackgroundThrottlingPolicy, NewWindowResponse, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows,
};

use super::DownloadTarget;

/// The site routes its current converter page from the origin root, today to
/// `/en17/`. Starting at the root keeps the flow working when that path moves.
const LANDING_URL: &str = "https://www.klickaud.org/";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
const WINDOW_TITLE: &str = "KlickAud verification";
const WINDOW_WIDTH: i32 = 560;
const WINDOW_HEIGHT: i32 = 460;
const ROBOT_ICON: &str =
    include_str!("../../../../../assets/icons/fontawesome-free-7.3.1/solid/robot.svg");
const XMARK_ICON: &str =
    include_str!("../../../../../assets/icons/fontawesome-free-7.3.1/solid/xmark.svg");
const MINUS_ICON: &str =
    include_str!("../../../../../assets/icons/fontawesome-free-7.3.1/solid/minus.svg");
const FLOW_JS: &str = include_str!("flow.js");
const FLOW_TIMEOUT: Duration = Duration::from_secs(150);
const FLOW_DEADLINE: Duration = Duration::from_secs(160);
const CLEANUP_GRACE: Duration = Duration::from_secs(2);
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const PUMP_INTERVAL: Duration = Duration::from_millis(10);
const PROFILE_ROOT: &str = "klickaud-webview";
const PROFILE_PREFIX: &str = "klickaud-";
const PROFILE_SWEEP_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) async fn resolve_download_url(
    web_url: &str,
    cancellation: &CancellationToken,
) -> Result<DownloadTarget, String> {
    if cancellation.is_cancelled() {
        return Err("Playback request cancelled".into());
    }
    let _flow = flow_lock().lock().await;
    if cancellation.is_cancelled() {
        return Err("Playback request cancelled".into());
    }

    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let abort = Arc::new(AtomicBool::new(false));
    let _abort_guard = AbortOnDrop(abort.clone());
    let thread_abort = abort.clone();
    let thread_cancellation = cancellation.clone();
    let thread_url = web_url.to_owned();
    let thread = std::thread::Builder::new()
        .name("ralgrum-klickaud-webview".into())
        .spawn(move || run_flow(&thread_url, &thread_cancellation, &thread_abort, done_tx))
        .map_err(|error| format!("Could not start the KlickAud verification thread: {error}"))?;

    let outcome = tokio::select! {
        _ = cancellation.cancelled() => None,
        result = &mut done_rx => Some(result),
        _ = tokio::time::sleep(FLOW_DEADLINE) => None,
    };

    let result = match outcome {
        Some(Ok(result)) => result,
        Some(Err(_)) => Err("The KlickAud verification thread stopped unexpectedly.".into()),
        None => {
            let cancelled = cancellation.is_cancelled();
            abort.store(true, Ordering::Relaxed);
            let _ = tokio::time::timeout(CLEANUP_GRACE, &mut done_rx).await;
            if cancelled {
                Err("Playback request cancelled".into())
            } else {
                Err("KlickAud verification timed out.".into())
            }
        }
    };

    // The thread sends its result only after the WebView, its context, and the
    // host window have been dropped, so this join normally returns immediately.
    let join = tokio::task::spawn_blocking(move || {
        let _ = thread.join();
    });
    let _ = tokio::time::timeout(JOIN_TIMEOUT, join).await;
    result
}

fn flow_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

struct AbortOnDrop(Arc<AtomicBool>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn run_flow(
    web_url: &str,
    cancellation: &CancellationToken,
    abort: &AtomicBool,
    done: tokio::sync::oneshot::Sender<Result<DownloadTarget, String>>,
) {
    let result = run_flow_inner(web_url, cancellation, abort);
    let _ = done.send(result);
}

fn run_flow_inner(
    web_url: &str,
    cancellation: &CancellationToken,
    abort: &AtomicBool,
) -> Result<DownloadTarget, String> {
    crate::diagnostics::event("INFO", "klickaud browser flow started");
    if cancellation.is_cancelled() || abort.load(Ordering::Relaxed) {
        return Err("Playback request cancelled".into());
    }

    let _com = ComApartment::initialize()?;
    let profile = open_profile()?;
    let hwnd = create_window()?;
    let window = NativeWindow(hwnd);
    let mut context = WebContext::new(Some(profile.path().to_owned()));
    let (ipc_tx, ipc_rx) = mpsc::channel::<String>();
    let preview = preview_requested();
    let script = flow_script(web_url, preview, false)?;
    let wry_com = WryComUninitialize;
    let webview = match WebViewBuilder::new_with_web_context(&mut context)
        .with_url(LANDING_URL)
        .with_user_agent(USER_AGENT)
        // Keep the browser chrome out of the verification surface: no
        // WebView2 context menu and no browser accelerator keys.
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_devtools(false)
        .with_background_color((0x09, 0x09, 0x0b, 0xff))
        .with_background_throttling(BackgroundThrottlingPolicy::Disabled)
        .with_initialization_script_for_main_only(script, false)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            let _ = ipc_tx.send(request.body().to_owned());
        })
        .build(&window)
    {
        Ok(webview) => webview,
        Err(error) => {
            drop(context);
            drop(window);
            drop(wry_com);
            drop(profile);
            return Err(format!(
                "Could not initialize the KlickAud WebView2 browser: {error}"
            ));
        }
    };
    let browser = Browser {
        webview,
        _context: context,
        window,
        _wry_com: wry_com,
        _profile: profile,
    };

    let started = Instant::now();
    let mut outcome = None;
    // In preview mode the window stays up after the flow finishes so the
    // layout can be inspected, and the successful outcome survives the close.
    while outcome.is_none() || preview {
        if cancellation.is_cancelled() || abort.load(Ordering::Relaxed) {
            if outcome.is_none() {
                let _ = browser
                    .webview
                    .evaluate_script("window.__kaCancel && window.__kaCancel();");
                outcome = Some(Err("Playback request cancelled".into()));
            }
            break;
        }
        if started.elapsed() >= FLOW_TIMEOUT {
            if outcome.is_none() {
                let _ = browser
                    .webview
                    .evaluate_script("window.__kaCancel && window.__kaCancel();");
                outcome = Some(Err("KlickAud verification timed out.".into()));
            }
            break;
        }
        if !browser.window.is_open() {
            if outcome.is_none() {
                outcome = Some(Err("KlickAud verification was cancelled.".into()));
            }
            break;
        }

        let mut pumped = false;
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            pumped = true;
            if message.message == WM_QUIT {
                if outcome.is_none() {
                    outcome = Some(Err("KlickAud verification was cancelled.".into()));
                }
                break;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if outcome.is_some() && !preview {
            break;
        }

        while let Ok(raw) = ipc_rx.try_recv() {
            let Some(event) = parse_ipc(&raw) else {
                continue;
            };
            match event {
                BrowserEvent::Progress(step) => {
                    crate::diagnostics::event("INFO", format!("klickaud browser step={step}"));
                }
                BrowserEvent::Interactive => {
                    crate::diagnostics::event(
                        "INFO",
                        "klickaud browser verification requires interaction",
                    );
                    reveal_window(hwnd);
                    let _ = browser.webview.focus();
                }
                BrowserEvent::Preview => {
                    crate::diagnostics::event(
                        "INFO",
                        "klickaud browser preview mode showing the verification window",
                    );
                    reveal_window(hwnd);
                    let _ = browser.webview.focus();
                }
                BrowserEvent::Minimize => minimize_window(hwnd),
                BrowserEvent::Verified => {
                    if !preview {
                        unsafe {
                            let _ = ShowWindow(hwnd, SW_HIDE);
                        }
                    }
                }
                BrowserEvent::Drag => unsafe {
                    let _ = ReleaseCapture();
                    SendMessageW(
                        hwnd,
                        WM_NCLBUTTONDOWN,
                        Some(WPARAM(HTCAPTION as usize)),
                        Some(LPARAM(0)),
                    );
                },
                BrowserEvent::Close => {
                    // Hide before the WebView2 surface is torn down, otherwise
                    // the controller clears to white for a frame on exit.
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                    if outcome.is_none() {
                        outcome = Some(Err("KlickAud verification was cancelled.".into()));
                    }
                    break;
                }
                BrowserEvent::Done(url) => {
                    outcome = Some(Ok(url));
                    if !preview {
                        break;
                    }
                }
                BrowserEvent::Error(error) => {
                    outcome = Some(Err(error));
                    if !preview {
                        break;
                    }
                }
            }
        }

        if !pumped {
            std::thread::sleep(PUMP_INTERVAL);
        }
    }

    let result = outcome
        .unwrap_or_else(|| Err("KlickAud verification stopped unexpectedly.".into()))
        .map(|url| DownloadTarget {
            url,
            cookies: session_cookies(&browser.webview),
        });
    match &result {
        Ok(_) => crate::diagnostics::event("INFO", "klickaud browser flow ready"),
        Err(error) => crate::diagnostics::event(
            "WARN",
            format!("klickaud browser flow failed reason={error}"),
        ),
    }
    result
}

/// Preview mode pins the verification window open so its styling can be
/// inspected even when Cloudflare solves the challenge without interaction.
/// Enable it with `RALGRUM_KLICKAUD_PREVIEW=1` before launching the app.
fn preview_requested() -> bool {
    std::env::var("RALGRUM_KLICKAUD_PREVIEW")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Opens the verification window without running a track flow, so the chrome
/// and overlay can be inspected on demand. Enabled by the same preview
/// environment variable used by the in-flow preview.
pub(super) fn preview_verification_if_requested() -> Result<(), String> {
    if !preview_requested() {
        return Ok(());
    }
    std::thread::Builder::new()
        .name("ralgrum-klickaud-preview".into())
        .spawn(|| {
            if let Err(error) = run_preview_window() {
                crate::diagnostics::event(
                    "WARN",
                    format!("klickaud preview window failed reason={error}"),
                );
            }
        })
        .map(|_| ())
        .map_err(|error| format!("Could not start the KlickAud preview thread: {error}"))
}

fn run_preview_window() -> Result<(), String> {
    crate::diagnostics::event("INFO", "klickaud preview window started");
    let _com = ComApartment::initialize()?;
    let profile = open_profile()?;
    let hwnd = create_window()?;
    let window = NativeWindow(hwnd);
    let mut context = WebContext::new(Some(profile.path().to_owned()));
    let (ipc_tx, ipc_rx) = mpsc::channel::<String>();
    let script = flow_script("", true, true)?;
    let wry_com = WryComUninitialize;
    let webview = match WebViewBuilder::new_with_web_context(&mut context)
        .with_url("about:blank")
        .with_user_agent(USER_AGENT)
        // Keep the browser chrome out of the verification surface: no
        // WebView2 context menu and no browser accelerator keys.
        .with_browser_accelerator_keys(false)
        .with_default_context_menus(false)
        .with_devtools(false)
        .with_background_color((0x09, 0x09, 0x0b, 0xff))
        .with_background_throttling(BackgroundThrottlingPolicy::Disabled)
        .with_initialization_script_for_main_only(script, false)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            let _ = ipc_tx.send(request.body().to_owned());
        })
        .build(&window)
    {
        Ok(webview) => webview,
        Err(error) => {
            drop(context);
            drop(window);
            drop(wry_com);
            drop(profile);
            return Err(format!(
                "Could not initialize the KlickAud preview WebView2 browser: {error}"
            ));
        }
    };
    let browser = Browser {
        webview,
        _context: context,
        window,
        _wry_com: wry_com,
        _profile: profile,
    };

    while browser.window.is_open() {
        let mut pumped = false;
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            pumped = true;
            if message.message == WM_QUIT {
                return Ok(());
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        while let Ok(raw) = ipc_rx.try_recv() {
            match parse_ipc(&raw) {
                Some(BrowserEvent::Preview) => {
                    reveal_window(hwnd);
                    let _ = browser.webview.focus();
                }
                Some(BrowserEvent::Minimize) => minimize_window(hwnd),
                Some(BrowserEvent::Drag) => unsafe {
                    let _ = ReleaseCapture();
                    SendMessageW(
                        hwnd,
                        WM_NCLBUTTONDOWN,
                        Some(WPARAM(HTCAPTION as usize)),
                        Some(LPARAM(0)),
                    );
                },
                Some(BrowserEvent::Close) => {
                    // Hide before the WebView2 surface is torn down, otherwise
                    // the controller clears to white for a frame on exit.
                    unsafe {
                        let _ = ShowWindow(hwnd, SW_HIDE);
                    }
                    return Ok(());
                }
                _ => {}
            }
        }
        if !pumped {
            std::thread::sleep(PUMP_INTERVAL);
        }
    }
    Ok(())
}

fn flow_script(web_url: &str, preview: bool, preview_only: bool) -> Result<String, String> {
    Ok(FLOW_JS
        .replace(
            "__TRACK_URL__",
            &serde_json::to_string(web_url)
                .map_err(|error| format!("Could not serialize the KlickAud track URL: {error}"))?,
        )
        .replace("__PREVIEW__", if preview { "true" } else { "false" })
        .replace(
            "__PREVIEW_ONLY__",
            if preview_only { "true" } else { "false" },
        )
        .replace(
            "__ROBOT_ICON__",
            &serde_json::to_string(ROBOT_ICON)
                .map_err(|error| format!("Could not serialize the KlickAud robot icon: {error}"))?,
        )
        .replace(
            "__XMARK_ICON__",
            &serde_json::to_string(XMARK_ICON)
                .map_err(|error| format!("Could not serialize the KlickAud close icon: {error}"))?,
        )
        .replace(
            "__MINUS_ICON__",
            &serde_json::to_string(MINUS_ICON).map_err(|error| {
                format!("Could not serialize the KlickAud minimize icon: {error}")
            })?,
        ))
}

struct Browser {
    webview: WebView,
    _context: WebContext,
    window: NativeWindow,
    _wry_com: WryComUninitialize,
    _profile: tempfile::TempDir,
}

enum BrowserEvent {
    Progress(String),
    Interactive,
    Preview,
    Verified,
    Drag,
    Minimize,
    Close,
    Done(String),
    Error(String),
}

fn parse_ipc(raw: &str) -> Option<BrowserEvent> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    match value.get("type").and_then(Value::as_str)? {
        "progress" => {
            let step = value.get("step").and_then(Value::as_str)?;
            Some(BrowserEvent::Progress(step.to_owned()))
        }
        "interactive" => Some(BrowserEvent::Interactive),
        "preview" => Some(BrowserEvent::Preview),
        "verified" => Some(BrowserEvent::Verified),
        "drag" => Some(BrowserEvent::Drag),
        "minimize" => Some(BrowserEvent::Minimize),
        "close" => Some(BrowserEvent::Close),
        "done" => {
            let url = value.get("url").and_then(Value::as_str)?.trim();
            (!url.is_empty()).then(|| BrowserEvent::Done(url.to_owned()))
        }
        "error" => {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .unwrap_or("KlickAud verification failed.");
            Some(BrowserEvent::Error(message.to_owned()))
        }
        _ => None,
    }
}

/// The delivery URL checks the session that prepared it, so the media client
/// must replay the browser's cookies. Only the name/value pairs are sent;
/// Cookie attributes are transport metadata, not part of the request header.
fn session_cookies(webview: &WebView) -> Option<String> {
    let cookies = match webview.cookies_for_url("https://www.klickaud.org/") {
        Ok(cookies) => cookies,
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!("klickaud session cookie read failed reason={error}"),
            );
            return None;
        }
    };
    let header = cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");
    (!header.is_empty()).then_some(header)
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, String> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            Ok(Self)
        } else {
            Err(format!(
                "Could not initialize the KlickAud thread COM apartment: {result:?}"
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct WryComUninitialize;

impl Drop for WryComUninitialize {
    fn drop(&mut self) {
        // Wry's WebView2 backend calls CoInitializeEx while building the
        // webview. This balances that nested successful call.
        unsafe { CoUninitialize() };
    }
}

struct NativeWindow(HWND);

impl NativeWindow {
    fn is_open(&self) -> bool {
        unsafe { IsWindow(Some(self.0)).as_bool() }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        if self.is_open() {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}

impl HasWindowHandle for NativeWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, raw_window_handle::HandleError> {
        let handle = Win32WindowHandle::new(
            NonZeroIsize::new(self.0.0 as isize)
                .ok_or(raw_window_handle::HandleError::Unavailable)?,
        );
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // A normal top-level window gets a native caption from the system even
        // with a custom titlebar. Keep the frame calculation but reclaim the
        // caption height so only the in-page titlebar is visible.
        WM_NCCALCSIZE => {
            if wparam.0 == 0 {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            unsafe {
                let params = lparam.0 as *mut NCCALCSIZE_PARAMS;
                let saved_top = (*params).rgrc[0].top;
                let result = DefWindowProcW(hwnd, message, wparam, lparam);
                (*params).rgrc[0].top = saved_top;
                result
            }
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn create_window() -> Result<HWND, String> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|error| {
        format!("Could not locate the application module for KlickAud: {error}")
    })?;
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: HINSTANCE::from(instance),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        hbrBackground: HBRUSH::default(),
        lpszClassName: w!("RalgrumKlickaudWebView"),
        ..Default::default()
    };
    unsafe { RegisterClassW(&class) };

    let anchor = app_window();
    let scale = window_scale(anchor);
    let width = (WINDOW_WIDTH as f32 * scale).round().max(1.) as i32;
    let height = (WINDOW_HEIGHT as f32 * scale).round().max(1.) as i32;
    let (x, y) = window_origin(anchor, width, height);
    let title: Vec<u16> = format!("{WINDOW_TITLE}\0").encode_utf16().collect();
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_APPWINDOW,
            class.lpszClassName,
            windows::core::PCWSTR(title.as_ptr()),
            WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(HINSTANCE::from(instance)),
            None,
        )
    }
    .map_err(|error| format!("Could not create the KlickAud verification window: {error}"))?;
    crate::windows_chrome::apply_app_window_chrome(hwnd);
    apply_rounded_corners(hwnd);
    // The window is created hidden, so Windows never recalculates the frame
    // after the initial pass. Force it so the caption height is reclaimed.
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    Ok(hwnd)
}

/// The verification window follows the app, so it opens centered on the
/// monitor that currently holds the main window instead of the primary
/// monitor.
fn window_origin(anchor: Option<HWND>, width: i32, height: i32) -> (i32, i32) {
    let monitor =
        unsafe { MonitorFromWindow(anchor.unwrap_or_default(), MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        let work = info.rcWork;
        let work_width = work.right - work.left;
        let work_height = work.bottom - work.top;
        return (
            work.left + (work_width - width).max(0) / 2,
            work.top + (work_height - height).max(0) / 2,
        );
    }
    (
        unsafe { (GetSystemMetrics(SM_CXSCREEN) - width) / 2 },
        unsafe { (GetSystemMetrics(SM_CYSCREEN) - height) / 2 },
    )
}

/// Uses the same asynchronous show command as the app's native minimize
/// control, so Windows runs the normal minimize animation.
fn minimize_window(hwnd: HWND) {
    let _ = unsafe { ShowWindowAsync(hwnd, SW_MINIMIZE) };
}

/// Brings the verification window back after the titlebar minimize control.
/// Windows reports a minimized window as iconic, so restore it before
/// re-centering and showing it again.
fn reveal_window(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
    }
    position_window(hwnd);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Re-centers a hidden verification window when it is about to be revealed,
/// so it follows the app if the app moved to another monitor while the
/// browser flow was running.
fn position_window(hwnd: HWND) {
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let (x, y) = window_origin(app_window(), width, height);
    let _ = unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

fn window_scale(anchor: Option<HWND>) -> f32 {
    let dpi = match anchor {
        Some(hwnd) => unsafe { GetDpiForWindow(hwnd) },
        None => 0,
    };
    let dpi = if dpi == 0 {
        unsafe { GetDpiForSystem() }
    } else {
        dpi
    };
    if dpi == 0 { 1. } else { dpi as f32 / 96. }
}

fn app_window() -> Option<HWND> {
    let pid = unsafe { GetCurrentProcessId() };
    let foreground = unsafe { GetForegroundWindow() };
    if !foreground.is_invalid() && window_process_id(foreground) == pid {
        return Some(foreground);
    }
    let mut search = AppWindowSearch {
        pid,
        best: None,
        best_area: 0,
    };
    let _ = unsafe {
        EnumWindows(
            Some(enum_app_window),
            LPARAM(&mut search as *mut AppWindowSearch as isize),
        )
    };
    search.best
}

fn window_process_id(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

struct AppWindowSearch {
    pid: u32,
    best: Option<HWND>,
    best_area: i64,
}

unsafe extern "system" fn enum_app_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = unsafe { &mut *(lparam.0 as *mut AppWindowSearch) };
    if window_process_id(hwnd) != search.pid || !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return true.into();
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return true.into();
    }
    let width = i64::from(rect.right - rect.left).max(0);
    let height = i64::from(rect.bottom - rect.top).max(0);
    let area = width * height;
    if area > search.best_area {
        search.best_area = area;
        search.best = Some(hwnd);
    }
    true.into()
}

fn apply_rounded_corners(hwnd: HWND) {
    let preference = DWMWCP_ROUND;
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        )
    };
}

fn open_profile() -> Result<tempfile::TempDir, String> {
    let root = crate::paths::cache_dir().join(PROFILE_ROOT);
    std::fs::create_dir_all(&root).map_err(|error| {
        format!("Could not create the KlickAud browser profile directory: {error}")
    })?;
    sweep_stale_profiles(&root);
    tempfile::Builder::new()
        .prefix(PROFILE_PREFIX)
        .tempdir_in(&root)
        .map_err(|error| format!("Could not create the KlickAud browser profile: {error}"))
}

fn sweep_stale_profiles(root: &Path) {
    let now = SystemTime::now();
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(PROFILE_PREFIX)
        {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.is_dir())
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= PROFILE_SWEEP_AGE);
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MINUS_ICON, ROBOT_ICON, XMARK_ICON, flow_script};

    #[test]
    fn flow_script_embeds_the_track_and_the_font_awesome_icons() {
        let script =
            flow_script("https://soundcloud.com/artist/track", false, false).expect("flow script");
        assert!(script.contains("\"https://soundcloud.com/artist/track\""));
        assert!(script.contains("ka-titlebar-icon"));
        assert!(script.contains("ka-window-controls"));
        assert!(script.contains("ka-min"));
        assert!(script.contains("ka-close"));
        assert!(script.contains("const PREVIEW = false;"));
        assert!(script.contains("const PREVIEW_ONLY = false;"));
        assert!(!script.contains("__TRACK_URL__"));
        assert!(!script.contains("__PREVIEW__"));
        assert!(!script.contains("__PREVIEW_ONLY__"));
        assert!(!script.contains("__ROBOT_ICON__"));
        assert!(!script.contains("__XMARK_ICON__"));
        assert!(!script.contains("__MINUS_ICON__"));
        assert!(ROBOT_ICON.contains("viewBox=\"0 0 640 512\""));
        assert!(XMARK_ICON.contains("viewBox=\"0 0 384 512\""));
        assert!(MINUS_ICON.contains("viewBox=\"0 0 448 512\""));
    }

    #[test]
    fn flow_script_isolates_the_overlay_in_a_shadow_root() {
        let script =
            flow_script("https://soundcloud.com/artist/track", false, false).expect("flow script");
        assert!(script.contains("ka-verification-host"));
        assert!(script.contains("attachShadow({ mode: 'open' })"));
        assert!(script.contains("root.appendChild(overlay)"));
        assert!(script.contains("document.body.appendChild(host)"));
        assert!(!script.contains("document.body.appendChild(overlay)"));
        assert!(!script.contains("document.getElementById('ka-close')"));
        assert!(!script.contains("document.getElementById('ka-min')"));
    }

    #[test]
    fn flow_script_enables_preview_mode_when_requested() {
        let script =
            flow_script("https://soundcloud.com/artist/track", true, false).expect("flow script");
        assert!(script.contains("const PREVIEW = true;"));
        assert!(script.contains("const PREVIEW_ONLY = false;"));
        assert!(!script.contains("__PREVIEW__"));
        assert!(!script.contains("__PREVIEW_ONLY__"));
    }

    #[test]
    fn flow_script_can_render_the_overlay_without_a_track_flow() {
        let script = flow_script("", true, true).expect("flow script");
        assert!(script.contains("const PREVIEW = true;"));
        assert!(script.contains("const PREVIEW_ONLY = true;"));
        assert!(!script.contains("__PREVIEW_ONLY__"));
    }
}
