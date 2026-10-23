//! WebKitGTK compatibility switches that must be in place before the first
//! GTK or WebKit call. Kept in its own module so the startup ordering is
//! explicit and testable without linking GTK on other platforms.

/// Disables WebKitGTK's DMA-BUF renderer unless the user already chose a
/// value. On Wayland compositors with strict explicit synchronization
/// (KWin, notably alongside the NVIDIA driver) WebKit's DMA-BUF path can
/// fail with "Error 71 (Protocol error) dispatching to Wayland display"
/// while the sign-in webview is still blank, which tears down the whole
/// process instead of surfacing a page error.
///
/// Returns true when this call installed the fallback. An existing value is
/// always preserved so users can opt back into hardware rendering.
///
/// # Safety
///
/// `std::env::set_var` is unsafe in edition 2024 because concurrent readers
/// are undefined behavior. Call this from `main` before diagnostics, GTK, or
/// any worker thread starts.
pub(crate) fn disable_dmabuf_renderer_if_unset() -> bool {
    const KEY: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    if std::env::var_os(KEY).is_some() {
        return false;
    }
    // SAFETY: the caller invokes this at the top of main, before any other
    // thread exists in the process.
    unsafe { std::env::set_var(KEY, "1") };
    true
}
