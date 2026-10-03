pub(crate) mod browser_link;
pub(crate) mod external_url;
pub(crate) mod media_control;
pub(crate) mod tray;

#[cfg(target_os = "linux")]
pub(crate) mod gtk_host;
#[cfg(target_os = "linux")]
pub(crate) mod linux_instance;
#[cfg(target_os = "linux")]
pub(crate) mod linux_power;
#[cfg(target_os = "linux")]
pub(crate) mod linux_protocol;
#[cfg(windows)]
pub(crate) mod windows_browser_link;
#[cfg(windows)]
pub(crate) mod windows_chrome;
#[cfg(windows)]
pub(crate) mod windows_protocol;
#[cfg(windows)]
pub(crate) mod windows_taskbar;
