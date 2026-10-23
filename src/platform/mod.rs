pub(crate) mod browser_link;
pub(crate) mod external_url;
pub(crate) mod media_control;
pub(crate) mod tray;

/// Caption of the main application window. KWin scripting matches on it.
pub(crate) const MAIN_WINDOW_TITLE: &str = "ralgruM";
/// Caption of the detached lyrics/queue window. It must stay distinct from
/// the main window so KWin scripts can target either one.
pub(crate) const POPOUT_WINDOW_TITLE: &str = "ralgruM Popout";

#[cfg(target_os = "linux")]
pub(crate) mod gtk_host;
#[cfg(target_os = "linux")]
pub(crate) mod linux_instance;
#[cfg(target_os = "linux")]
pub(crate) mod linux_power;
#[cfg(target_os = "linux")]
pub(crate) mod linux_protocol;
#[cfg(target_os = "linux")]
pub(crate) mod linux_webkit;
#[cfg(target_os = "linux")]
pub(crate) mod linux_window;
#[cfg(windows)]
pub(crate) mod windows_browser_link;
#[cfg(windows)]
pub(crate) mod windows_chrome;
#[cfg(windows)]
pub(crate) mod windows_protocol;
#[cfg(windows)]
pub(crate) mod windows_taskbar;
