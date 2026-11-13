//! Browser-backed klickaud resolver.
//!
//! klickaud gates `sse_capability.php` behind a Cloudflare Turnstile token
//! bound to the download grant. The token can only be minted by a real
//! browser, so the whole same-origin flow runs inside a hidden WebView2 and
//! only the final delivery URL and its session cookies cross back into Rust.

/// What the browser flow hands back: the delivery URL plus the session
/// cookies it is bound to. The delivery token is useless without the PHP
/// session that prepared it, and `reqwest` has no access to the WebView's
/// cookie store.
pub(super) struct DownloadTarget {
    pub url: String,
    pub cookies: Option<String>,
}

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub(super) async fn resolve_download_url(
    web_url: &str,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<DownloadTarget, String> {
    windows::resolve_download_url(web_url, cancellation).await
}

#[cfg(windows)]
pub(super) fn preview_verification_if_requested() -> Result<(), String> {
    windows::preview_verification_if_requested()
}

#[cfg(not(windows))]
pub(super) async fn resolve_download_url(
    _web_url: &str,
    _cancellation: &tokio_util::sync::CancellationToken,
) -> Result<DownloadTarget, String> {
    Err("KlickAud resolution requires the Windows WebView2 runtime.".to_owned())
}
