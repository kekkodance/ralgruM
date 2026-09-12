pub(crate) mod browser_link;
pub(crate) mod external_url;
pub(crate) mod media_control;
pub(crate) mod tray;

#[cfg(windows)]
pub(crate) mod windows_browser_link;
#[cfg(windows)]
pub(crate) mod windows_chrome;
#[cfg(windows)]
pub(crate) mod windows_protocol;
#[cfg(windows)]
pub(crate) mod windows_taskbar;
