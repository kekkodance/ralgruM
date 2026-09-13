use crate::service_auth::Service;

#[derive(Debug)]
pub(crate) struct BrowserCredentials {
    pub(crate) desktop: String,
    pub(crate) mobile_authorization: Option<MobileAuthorization>,
    pub(crate) soundcloud_cookies: Option<String>,
    pub(crate) deezer_cookies: Option<String>,
}

#[derive(Debug)]
pub(crate) struct MobileAuthorization {
    pub(crate) code: String,
    pub(crate) verifier: String,
}

#[cfg(windows)]
mod platform {
    use std::{
        collections::HashMap,
        num::NonZeroIsize,
        path::PathBuf,
        sync::{Mutex, OnceLock, mpsc},
        time::{Duration, Instant},
    };

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
    use sha2::{Digest, Sha256};
    use url::Url;
    use uuid::Uuid;
    use windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
            Graphics::Gdi::HBRUSH,
            System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GetSystemMetrics, IDC_ARROW, IsWindow, LoadCursorW, MSG,
                PM_REMOVE, PeekMessageW, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW,
                SetForegroundWindow, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE,
                WM_DESTROY, WM_QUIT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
        core::w,
    };
    use wry::{WebContext, WebViewBuilder};

    use crate::search::DEEZER_USER_AGENT;

    use super::{BrowserCredentials, MobileAuthorization, Service};

    const DEEZER_LOGIN_URL: &str =
        "https://www.deezer.com/login?redirect_type=page&redirect_link=%2Faccount%2F";
    const DEEZER_COOKIE_URL: &str = "https://www.deezer.com/";
    const SOUNDCLOUD_LOGIN_URL: &str = "https://soundcloud.com/signin";
    const SOUNDCLOUD_COOKIE_URL: &str = "https://soundcloud.com/";
    const SOUNDCLOUD_MOBILE_COOKIE_URL: &str = "https://api-mobile.soundcloud.com/";
    const SOUNDCLOUD_AUTHORIZE_URL: &str = "https://secure.soundcloud.com/authorize";
    const SOUNDCLOUD_MOBILE_CLIENT_ID: &str = "SSdQ80vM8nLPhbDBylHl2JFK6ElhBr9B";
    const SOUNDCLOUD_MOBILE_REDIRECT_URI: &str = "sc://auth";
    const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    const COOKIE_CONSENT_REJECTION_SCRIPT: &str = include_str!("service_auth_cookie_consent.js");

    struct ComApartment;

    impl ComApartment {
        fn initialize() -> Result<Self, String> {
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if result.is_ok() {
                Ok(Self)
            } else {
                Err(format!(
                    "Could not initialize the sign-in thread COM apartment: {result:?}"
                ))
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            // This balances the successful initialization owned by this guard.
            unsafe { CoUninitialize() };
        }
    }

    struct WryComUninitialize;

    impl Drop for WryComUninitialize {
        fn drop(&mut self) {
            // WRY's WebView2 backend calls CoInitializeEx during
            // InnerWebView::new_in_hwnd. The guard is armed only immediately
            // before our known-valid builder calls build, so this balances
            // WRY's nested successful call without touching unrelated threads.
            unsafe { CoUninitialize() };
        }
    }

    struct NativeWindow(HWND);

    impl NativeWindow {
        fn is_open(&self) -> bool {
            unsafe { IsWindow(Some(self.0)).as_bool() }
        }

        fn close(&mut self) -> Result<(), String> {
            if !self.is_open() {
                self.0 = HWND::default();
                return Ok(());
            }
            unsafe { DestroyWindow(self.0) }
                .map_err(|error| format!("Could not close the sign-in window: {error}"))?;
            self.0 = HWND::default();
            Ok(())
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
            WM_CLOSE => {
                let _ = unsafe { DestroyWindow(hwnd) };
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    pub(crate) async fn login(service: Service) -> Result<BrowserCredentials, String> {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let thread_name = format!("ralgrum-auth-{}", service.display_name());
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                // WRY initializes WebView2 as an STA and pumps this thread's
                // message queue while creating the environment and controller.
                // A fresh thread avoids reusing a Tokio blocking worker whose
                // apartment may already be MTA. The lock still prevents two
                // native windows from overlapping.
                let result = {
                    let _login_guard = login_lock()
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    match ComApartment::initialize() {
                        Ok(_com_apartment) => run_login(service),
                        Err(error) => Err(error),
                    }
                };
                let _ = result_tx.send(result);
            })
            .map_err(|error| format!("Could not start the sign-in window thread: {error}"))?;
        result_rx
            .await
            .map_err(|_| "The sign-in window thread stopped unexpectedly.".to_owned())?
    }

    fn login_lock() -> &'static Mutex<()> {
        static LOGIN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOGIN_LOCK.get_or_init(|| Mutex::new(()))
    }

    fn run_login(service: Service) -> Result<BrowserCredentials, String> {
        drain_pending_quit_messages();
        let mut wry_com_uninitialize = None;
        let (width, height, title) = match service {
            Service::Deezer => (560, 720, "Sign in to Deezer\0"),
            Service::SoundCloud => (1040, 760, "Sign in to SoundCloud\0"),
        };
        let title: Vec<u16> = title.encode_utf16().collect();
        let instance = unsafe { GetModuleHandleW(None) }
            .map(HINSTANCE::from)
            .map_err(|error| format!("Could not locate the application module: {error}"))?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            hbrBackground: HBRUSH::default(),
            lpszClassName: w!("RalgrumAuthWebView"),
            ..Default::default()
        };
        unsafe { RegisterClassW(&class) };
        let x = unsafe { (GetSystemMetrics(SM_CXSCREEN) - width) / 2 };
        let y = unsafe { (GetSystemMetrics(SM_CYSCREEN) - height) / 2 };
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class.lpszClassName,
                windows::core::PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                x,
                y,
                width,
                height,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|error| format!("Could not create the sign-in window: {error}"))?;
        crate::windows_chrome::apply_app_window_chrome(hwnd);
        let mut window = NativeWindow(hwnd);
        let data_dir = tempfile::Builder::new()
            .prefix("ralgrum-auth-")
            .tempdir()
            .map_err(|error| format!("Could not create the private browser profile: {error}"))?;
        let profile_path = data_dir.path().to_owned();
        let mut context = WebContext::new(Some(profile_path.clone()));
        let (redirect_tx, redirect_rx) = mpsc::channel();
        let mut webview = None;
        let result = (|| -> Result<BrowserCredentials, String> {
            let webview_builder = WebViewBuilder::new_with_web_context(&mut context)
                .with_url(login_url(service))
                .with_user_agent(DEEZER_USER_AGENT)
                .with_focused(true)
                .with_initialization_script_for_main_only(COOKIE_CONSENT_REJECTION_SCRIPT, false)
                .with_navigation_handler(move |raw_url| {
                    if is_soundcloud_redirect(&raw_url) {
                        let _ = redirect_tx.send(raw_url);
                        return false;
                    }
                    navigation_allowed(&raw_url)
                });
            // These builder methods only populate attributes and cannot set
            // WebViewBuilder::error. NativeWindow is a live Win32 handle, so
            // build is guaranteed to enter WRY's WebView2 backend, whose first
            // operation is its STA CoInitializeEx call.
            wry_com_uninitialize = Some(WryComUninitialize);
            let built_webview = webview_builder
                .build(&window)
                .map_err(|error| format!("Could not initialize WebView2: {error}"))?;
            webview = Some(built_webview);
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
            }
            webview
                .as_ref()
                .expect("WebView was just initialized")
                .focus()
                .map_err(|error| format!("Could not focus the sign-in window: {error}"))?;

            let started = Instant::now();
            let mut next_poll = Instant::now();
            let desktop = loop {
                if started.elapsed() >= LOGIN_TIMEOUT {
                    return Err(format!("{} sign-in timed out.", service.display_name()));
                }
                if !window.is_open() {
                    return Err(format!("{} sign-in was cancelled.", service.display_name()));
                }
                if Instant::now() >= next_poll {
                    next_poll = Instant::now() + POLL_INTERVAL;
                    if let Some(value) = webview
                        .as_ref()
                        .expect("WebView was just initialized")
                        .cookies_for_url(cookie_url(service))
                        .map_err(|error| format!("Could not inspect the sign-in session: {error}"))?
                        .into_iter()
                        .find(|cookie| cookie.name() == cookie_name(service))
                        .map(|cookie| cookie.value().trim().to_owned())
                        .filter(|value| !value.is_empty())
                    {
                        break value;
                    }
                }
                pump_one_message(service, &window)?;
            };

            let mobile_authorization = if service == Service::SoundCloud {
                let (verifier, challenge) = pkce_pair();
                let state = Uuid::new_v4().simple().to_string();
                webview
                    .as_ref()
                    .expect("WebView was just initialized")
                    .load_url(&soundcloud_authorize_url(&challenge, &state)?)
                    .map_err(|error| {
                        format!("Could not open SoundCloud mobile authorization: {error}")
                    })?;
                Some(loop {
                    if started.elapsed() >= LOGIN_TIMEOUT {
                        return Err("SoundCloud mobile authorization timed out.".into());
                    }
                    if !window.is_open() {
                        return Err("SoundCloud sign-in was cancelled.".into());
                    }
                    if let Ok(redirect) = redirect_rx.try_recv() {
                        break MobileAuthorization {
                            code: soundcloud_authorization_code(&redirect, &state)?,
                            verifier,
                        };
                    }
                    pump_one_message(service, &window)?;
                })
            } else {
                None
            };

            let deezer_cookies = if service == Service::Deezer {
                // The ARL is captured separately in the loop above, so every
                // other deezer.com cookie joins the persistent jar here.
                let cookies = webview
                    .as_ref()
                    .expect("WebView was just initialized")
                    .cookies_for_url(DEEZER_COOKIE_URL)
                    .map_err(|error| {
                        format!("Could not preserve the Deezer sign-in session: {error}")
                    })?;
                let value = cookies
                    .into_iter()
                    .filter_map(|cookie| {
                        let name = cookie.name().trim();
                        let value = cookie.value().trim();
                        let keep = !name.is_empty()
                            && !value.is_empty()
                            && !name.eq_ignore_ascii_case("arl");
                        keep.then(|| format!("{name}={value}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                (!value.is_empty()).then_some(value)
            } else {
                None
            };
            let soundcloud_cookies = if service == Service::SoundCloud {
                let cookies = webview
                    .as_ref()
                    .expect("WebView was just initialized")
                    .cookies_for_url(SOUNDCLOUD_MOBILE_COOKIE_URL)
                    .map_err(|error| {
                        format!("Could not preserve the SoundCloud sign-in session: {error}")
                    })?;
                let value = cookies
                    .into_iter()
                    .filter_map(|cookie| {
                        let name = cookie.name().trim();
                        let value = cookie.value().trim();
                        (!name.is_empty() && !value.is_empty()).then(|| format!("{name}={value}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                (!value.is_empty()).then_some(value)
            } else {
                None
            };
            Ok(BrowserCredentials {
                desktop,
                mobile_authorization,
                soundcloud_cookies,
                deezer_cookies,
            })
        })();

        // Retain the exact unique profile path until WebView2, its context, and
        // the host HWND have all gone away, then remove it on a delayed retry
        // worker. This also runs for build, focus, timeout, cancellation, and
        // cookie errors.
        drop(webview);
        drop(context);
        let profile_path = data_dir.keep();
        let close_result = window.close();
        drop(window);
        drop(wry_com_uninitialize);
        schedule_profile_cleanup(profile_path);

        match result {
            Err(error) => Err(error),
            Ok(credentials) => close_result.map(|_| credentials),
        }
    }

    const PROFILE_CLEANUP_RETRIES: usize = 60;
    const PROFILE_CLEANUP_DELAY: Duration = Duration::from_millis(100);

    fn schedule_profile_cleanup(profile_path: PathBuf) {
        let fallback_path = profile_path.clone();
        let worker = std::thread::Builder::new()
            .name("ralgrum-auth-profile-cleanup".into())
            .spawn(move || cleanup_profile(profile_path));
        if worker.is_err() {
            // The profile is private and uniquely named, so a synchronous
            // best-effort fallback is safe if the process cannot start a worker.
            cleanup_profile(fallback_path);
        }
    }

    fn cleanup_profile(profile_path: PathBuf) {
        std::thread::sleep(PROFILE_CLEANUP_DELAY);
        for attempt in 0..PROFILE_CLEANUP_RETRIES {
            match std::fs::remove_dir_all(&profile_path) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) if attempt + 1 < PROFILE_CLEANUP_RETRIES => {
                    drop(error);
                    std::thread::sleep(PROFILE_CLEANUP_DELAY);
                }
                Err(_) => return,
            }
        }
    }

    fn pump_one_message(service: Service, window: &NativeWindow) -> Result<(), String> {
        if !window.is_open() {
            return Err(format!("{} sign-in was cancelled.", service.display_name()));
        }
        let mut message = MSG::default();
        if unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                return Err(format!("{} sign-in was cancelled.", service.display_name()));
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !window.is_open() {
            return Err(format!("{} sign-in was cancelled.", service.display_name()));
        }
        Ok(())
    }

    fn drain_pending_quit_messages() {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE) }.as_bool() {}
    }

    fn login_url(service: Service) -> &'static str {
        match service {
            Service::Deezer => DEEZER_LOGIN_URL,
            Service::SoundCloud => SOUNDCLOUD_LOGIN_URL,
        }
    }

    fn cookie_url(service: Service) -> &'static str {
        match service {
            Service::Deezer => DEEZER_COOKIE_URL,
            Service::SoundCloud => SOUNDCLOUD_COOKIE_URL,
        }
    }

    fn cookie_name(service: Service) -> &'static str {
        match service {
            Service::Deezer => "arl",
            Service::SoundCloud => "oauth_token",
        }
    }

    fn navigation_allowed(raw_url: &str) -> bool {
        Url::parse(raw_url)
            .map(|url| matches!(url.scheme(), "https" | "about"))
            .unwrap_or(false)
    }

    fn is_soundcloud_redirect(raw_url: &str) -> bool {
        Url::parse(raw_url)
            .map(|url| url.scheme() == "sc" && url.host_str() == Some("auth"))
            .unwrap_or(false)
    }

    fn pkce_pair() -> (String, String) {
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        (verifier, challenge)
    }

    fn soundcloud_authorize_url(challenge: &str, state: &str) -> Result<String, String> {
        let mut url = Url::parse(SOUNDCLOUD_AUTHORIZE_URL).map_err(|error| error.to_string())?;
        url.query_pairs_mut()
            .append_pair("client_id", SOUNDCLOUD_MOBILE_CLIENT_ID)
            .append_pair("redirect_uri", SOUNDCLOUD_MOBILE_REDIRECT_URI)
            .append_pair("response_type", "code")
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", state)
            .append_pair("display", "popup");
        Ok(url.into())
    }

    fn soundcloud_authorization_code(
        raw_url: &str,
        expected_state: &str,
    ) -> Result<String, String> {
        let url = Url::parse(raw_url).map_err(|_| "SoundCloud returned an invalid redirect.")?;
        let parameters = url.query_pairs().collect::<HashMap<_, _>>();
        if parameters.get("state").map(|value| value.as_ref()) != Some(expected_state) {
            return Err("SoundCloud returned an invalid mobile authorization state.".into());
        }
        parameters
            .get("code")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "SoundCloud did not return a mobile authorization code.".into())
    }

    impl Service {
        fn display_name(self) -> &'static str {
            match self {
                Self::Deezer => "Deezer",
                Self::SoundCloud => "SoundCloud",
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn history_played_at(value: &serde_json::Value, track_id: u64) -> Option<u64> {
            value
                .get("collection")?
                .as_array()?
                .iter()
                .find(|item| {
                    item.get("track_id").and_then(serde_json::Value::as_u64) == Some(track_id)
                })?
                .get("played_at")?
                .as_u64()
        }

        #[test]
        fn navigation_policy_allows_only_https_and_about() {
            assert!(navigation_allowed("https://soundcloud.com/signin"));
            assert!(navigation_allowed("about:blank"));
            assert!(!navigation_allowed("http://soundcloud.com/signin"));
            assert!(!navigation_allowed("file:///C:/secret"));
            assert!(!navigation_allowed("sc://auth?code=x&state=y"));
        }

        #[test]
        fn recognizes_and_validates_soundcloud_redirect() {
            let redirect = "sc://auth?code=authorization-code&state=expected-state";
            assert!(is_soundcloud_redirect(redirect));
            assert_eq!(
                soundcloud_authorization_code(redirect, "expected-state").unwrap(),
                "authorization-code"
            );
            assert!(soundcloud_authorization_code(redirect, "wrong-state").is_err());
            assert!(!is_soundcloud_redirect("sc://other?code=x"));
        }

        #[test]
        fn pkce_values_are_url_safe_and_stateful_authorize_url_is_correct() {
            let (verifier, challenge) = pkce_pair();
            assert_eq!(verifier.len(), 64);
            assert!(!challenge.contains('='));
            let url = Url::parse(&soundcloud_authorize_url(&challenge, "state").unwrap()).unwrap();
            let parameters = url.query_pairs().collect::<HashMap<_, _>>();
            assert_eq!(
                parameters.get("state").map(|value| value.as_ref()),
                Some("state")
            );
            assert_eq!(
                parameters.get("redirect_uri").map(|value| value.as_ref()),
                Some(SOUNDCLOUD_MOBILE_REDIRECT_URI)
            );
        }

        #[test]
        fn soundcloud_history_lookup_reads_matching_track_timestamp() {
            let history = serde_json::json!({
                "collection": [
                    {"track_id": 41, "played_at": 1_700_000_000},
                    {"track_id": 42, "played_at": 1_700_000_123}
                ]
            });

            assert_eq!(history_played_at(&history, 42), Some(1_700_000_123));
            assert_eq!(history_played_at(&history, 99), None);
            assert_eq!(history_played_at(&serde_json::json!({}), 42), None);
        }
    }
}

#[cfg(windows)]
pub(crate) use platform::login;

#[cfg(not(windows))]
pub(crate) async fn login(_service: Service) -> Result<BrowserCredentials, String> {
    Err("Embedded service sign-in is currently supported only on Windows.".into())
}
