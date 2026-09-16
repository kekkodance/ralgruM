use std::process::Command;

const PAYMENT_HOSTS: &[&str] = &[
    "t.me",
    "telegram.me",
    "murglar.app",
    "pay.oxapay.com",
    "payform.aurapay.tech",
];
const FONT_AWESOME_URL: &str = "https://fontawesome.com";
const CHROMIUM_URL: &str =
    "https://chromium.googlesource.com/chromium/src/+/main/ui/resources/cursors/";
const GPUI_URL: &str = "https://github.com/zed-industries/zed";
const GPUI_COMPONENT_URL: &str = "https://github.com/longbridge/gpui-component";
const ASIO_SDK_URL: &str = "https://www.steinberg.net/asiosdk";

pub(crate) fn open_payment_url(value: &str) -> Result<(), String> {
    let url = validate_payment_url(value)?;
    open(url.as_str(), "payment page")
}

pub(crate) fn open_murglar_status() -> Result<(), String> {
    open("https://murglar.app/status", "Murglar status page")
}

pub(crate) fn open_github_profile(value: &str) -> Result<(), String> {
    let url = validate_github_profile_url(value)?;
    open(url.as_str(), "GitHub profile")
}

pub(crate) fn open_font_awesome() -> Result<(), String> {
    open(FONT_AWESOME_URL, "Font Awesome site")
}

pub(crate) fn open_chromium() -> Result<(), String> {
    open(CHROMIUM_URL, "Chromium source")
}

pub(crate) fn open_gpui() -> Result<(), String> {
    open(GPUI_URL, "GPUI project")
}

pub(crate) fn open_gpui_component() -> Result<(), String> {
    open(GPUI_COMPONENT_URL, "gpui-component project")
}

pub(crate) fn open_asio_sdk() -> Result<(), String> {
    open(ASIO_SDK_URL, "ASIO SDK site")
}

fn validate_payment_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| format!("Invalid Murglar payment link: {error}"))?;
    if url.scheme() != "https" {
        return Err("Murglar payment links must use HTTPS".into());
    }
    if !url
        .host_str()
        .is_some_and(|host| PAYMENT_HOSTS.contains(&host))
    {
        return Err("Murglar returned an unrecognized payment host".into());
    }
    Ok(url)
}

fn validate_github_profile_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| format!("Invalid GitHub profile link: {error}"))?;
    if url.scheme() != "https" {
        return Err("GitHub profile links must use HTTPS".into());
    }
    if url.host_str() != Some("github.com") {
        return Err("GitHub profile links must use github.com".into());
    }
    Ok(url)
}

fn open(url: &str, destination: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let result = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(url).spawn();
    result
        .map(|_| ())
        .map_err(|error| format!("Could not open the {destination}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{validate_github_profile_url, validate_payment_url};

    #[test]
    fn payment_urls_accept_every_captured_payment_destination() {
        for url in [
            "https://t.me/murglar_payments_bot?start=fixture",
            "https://murglar.app/docs/en/en-paypal.html",
            "https://pay.oxapay.com/merchant/invoice",
            "https://murglar.app/docs/en/en-guide-crypto.html",
            "https://payform.aurapay.tech/order-id",
        ] {
            assert!(validate_payment_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn payment_urls_require_https_and_an_exact_allowlisted_host() {
        assert!(validate_payment_url("https://telegram.me/pay").is_ok());
        for url in [
            "http://murglar.app/pay",
            "http://t.me/murglar_payments_bot?start=fixture",
            "https://evil.example/pay",
            "https://murglar.app.evil.example/pay",
            "https://t.me.evil.example/murglar_payments_bot",
            "https://evil-t.me/murglar_payments_bot",
            "not a url",
        ] {
            assert!(validate_payment_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn github_profile_urls_accept_https_github_com() {
        assert!(validate_github_profile_url("https://github.com/kekkodance").is_ok());
    }

    #[test]
    fn font_awesome_opens_the_official_site() {
        assert_eq!(super::FONT_AWESOME_URL, "https://fontawesome.com");
    }

    #[test]
    fn third_party_notice_urls_match_the_repository_metadata() {
        assert_eq!(
            super::CHROMIUM_URL,
            "https://chromium.googlesource.com/chromium/src/+/main/ui/resources/cursors/"
        );
        assert_eq!(super::GPUI_URL, "https://github.com/zed-industries/zed");
        assert_eq!(
            super::GPUI_COMPONENT_URL,
            "https://github.com/longbridge/gpui-component"
        );
        assert_eq!(super::ASIO_SDK_URL, "https://www.steinberg.net/asiosdk");
    }

    #[test]
    fn github_profile_urls_require_https_and_github_com() {
        for url in [
            "http://github.com/kekkodance",
            "https://evil.example/kekkodance",
            "https://github.com.evil.example/kekkodance",
            "https://gitlab.com/kekkodance",
            "not a url",
        ] {
            assert!(validate_github_profile_url(url).is_err(), "{url}");
        }
    }
}
