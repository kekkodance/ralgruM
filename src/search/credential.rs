use std::sync::{Arc, RwLock};

use reqwest::header::HeaderValue;

use super::models::ProviderError;

pub(crate) const DEEZER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

#[derive(Clone)]
pub(crate) struct DeezerArl {
    value: Box<str>,
    jar: Option<DeezerCookieJar>,
}

#[derive(Clone)]
pub(crate) struct SoundCloudToken(Box<str>);

impl DeezerArl {
    pub(crate) fn from_saved(value: &str) -> Option<Self> {
        Self::from_saved_parts(value, None)
    }

    pub(crate) fn from_saved_with_jar(value: &str, jar: &DeezerCookieJar) -> Option<Self> {
        Self::from_saved_parts(value, Some(jar.clone()))
    }

    fn from_saved_parts(value: &str, jar: Option<DeezerCookieJar>) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()
            && value.len() <= 1024
            && !value
                .chars()
                .any(|character| character.is_control() || matches!(character, ';' | ',')))
        .then(|| Self {
            value: value.into(),
            jar,
        })
    }

    pub(crate) fn cookie_header(&self) -> Result<HeaderValue, ProviderError> {
        let mut header = format!("arl={}", self.value);
        if let Some(jar) = self.jar.as_ref()
            && let Some(cookies) = jar.snapshot().filter(|cookies| !cookies.is_empty())
        {
            header.push_str("; ");
            header.push_str(&cookies);
        }
        let mut value = HeaderValue::from_str(&header)
            .map_err(|_| ProviderError::new("The saved Deezer session is invalid"))?;
        value.set_sensitive(true);
        Ok(value)
    }

    pub(crate) fn attached_jar(&self) -> Option<&DeezerCookieJar> {
        self.jar.as_ref()
    }

    pub(crate) fn expose(&self) -> &str {
        &self.value
    }
}

/// Process-wide jar of non-ARL deezer.com cookies (sid, datadome, ...).
/// AccountState owns one instance and mints every DeezerArl from it.
#[derive(Clone, Default)]
pub(crate) struct DeezerCookieJar {
    cookies: Arc<RwLock<Option<String>>>,
}

impl DeezerCookieJar {
    pub(crate) fn new(initial: Option<String>) -> Self {
        Self {
            cookies: Arc::new(RwLock::new(
                initial.filter(|cookies| !cookies.trim().is_empty()),
            )),
        }
    }

    pub(crate) fn snapshot(&self) -> Option<String> {
        self.cookies
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Merge Set-Cookie parts ("sid=..; datadome=..") into the jar.
    pub(crate) fn refresh(&self, parts: &str) {
        let mut guard = self
            .cookies
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let merged = merge_cookie_parts(guard.as_deref().unwrap_or(""), parts);
        *guard = (!merged.is_empty()).then_some(merged);
    }
}

/// Merge semicolon-separated `name=value` cookie parts. The `arl` cookie is
/// skipped in both arguments because it is persisted separately. A repeated
/// name replaces its earlier value instead of duplicating, first-appearance
/// order is preserved, and empty or malformed parts are dropped.
pub(crate) fn merge_cookie_parts(existing: &str, parts: &str) -> String {
    fn merge_into(cookies: &mut Vec<(String, String)>, raw: &str) {
        for part in raw.split(';') {
            let Some((name, value)) = part.split_once('=') else {
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            let contains_control =
                |text: &str| text.chars().any(|character| character.is_control());
            if name.is_empty()
                || value.is_empty()
                || name.eq_ignore_ascii_case("arl")
                || contains_control(name)
                || contains_control(value)
            {
                continue;
            }
            match cookies
                .iter_mut()
                .find(|(existing_name, _)| existing_name == name)
            {
                Some(entry) => entry.1 = value.to_owned(),
                None => cookies.push((name.to_owned(), value.to_owned())),
            }
        }
    }

    let mut cookies = Vec::new();
    merge_into(&mut cookies, existing);
    merge_into(&mut cookies, parts);
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

impl SoundCloudToken {
    pub(crate) fn from_saved(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()).then(|| Self(value.into()))
    }

    pub(crate) fn authorization_header(&self) -> Result<HeaderValue, ProviderError> {
        let authorization = if self.0.starts_with("OAuth ") {
            self.0.to_string()
        } else {
            format!("OAuth {}", self.0)
        };
        let mut value = HeaderValue::from_str(&authorization)
            .map_err(|_| ProviderError::new("The saved SoundCloud session is invalid"))?;
        value.set_sensitive(true);
        Ok(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_credentials_are_trimmed_and_headers_are_sensitive() {
        let credential = DeezerArl::from_saved(" sentinel ").unwrap();
        let header = credential.cookie_header().unwrap();
        assert_eq!(header.to_str().unwrap(), "arl=sentinel");
        assert!(header.is_sensitive());
        assert!(DeezerArl::from_saved("  ").is_none());

        let token = SoundCloudToken::from_saved(" soundcloud-sentinel ").unwrap();
        let header = token.authorization_header().unwrap();
        assert_eq!(header.to_str().unwrap(), "OAuth soundcloud-sentinel");
        assert!(header.is_sensitive());
        assert!(SoundCloudToken::from_saved("  ").is_none());
    }

    #[test]
    fn deezer_arl_rejects_provider_boundary_violations() {
        assert!(DeezerArl::from_saved(&"a".repeat(1024)).is_some());
        assert!(DeezerArl::from_saved(&"a".repeat(1025)).is_none());
        for value in ["abc;def", "abc,def", "abc\ndef", "abc\0def"] {
            assert!(DeezerArl::from_saved(value).is_none(), "{value:?}");
        }
    }

    #[test]
    fn soundcloud_only_prepends_missing_exact_oauth_prefix() {
        let bare = SoundCloudToken::from_saved("bare").unwrap();
        assert_eq!(bare.authorization_header().unwrap(), "OAuth bare");
        let prefixed = SoundCloudToken::from_saved("OAuth exact").unwrap();
        assert_eq!(prefixed.authorization_header().unwrap(), "OAuth exact");
        let differently_cased = SoundCloudToken::from_saved("oauth lower").unwrap();
        assert_eq!(
            differently_cased.authorization_header().unwrap(),
            "OAuth oauth lower"
        );
    }

    #[test]
    fn merge_cookie_parts_merges_replaces_and_keeps_first_appearance_order() {
        assert_eq!(
            merge_cookie_parts("sid=first; datadome=guard", "dzr_uniq_id=abc; sid=second"),
            "sid=second; datadome=guard; dzr_uniq_id=abc"
        );
        assert_eq!(merge_cookie_parts("", "sid=only"), "sid=only");
        assert_eq!(merge_cookie_parts("sid=only", ""), "sid=only");
        assert_eq!(merge_cookie_parts("", ""), "");
        assert_eq!(merge_cookie_parts("no-value-yet", "sid=kept"), "sid=kept");
    }

    #[test]
    fn merge_cookie_parts_skips_arl_and_drops_malformed_parts() {
        assert_eq!(
            merge_cookie_parts("arl=secret; sid=first", "arl=server; sid=second"),
            "sid=second"
        );
        assert_eq!(merge_cookie_parts("ARL=secret", "sid=kept"), "sid=kept");
        assert_eq!(
            merge_cookie_parts("broken; =no-name; sid=; sid=kept", ""),
            "sid=kept"
        );
    }

    #[test]
    fn cookie_header_appends_the_jar_only_when_it_is_not_empty() {
        let bare = DeezerArl::from_saved("sentinel").unwrap();
        assert!(bare.attached_jar().is_none());
        assert_eq!(bare.cookie_header().unwrap(), "arl=sentinel");

        let empty_jar =
            DeezerArl::from_saved_with_jar("sentinel", &DeezerCookieJar::new(None)).unwrap();
        assert_eq!(empty_jar.cookie_header().unwrap(), "arl=sentinel");

        let jar = DeezerCookieJar::new(Some("sid=session; datadome=guard".into()));
        let attached = DeezerArl::from_saved_with_jar("sentinel", &jar).unwrap();
        assert_eq!(
            attached.cookie_header().unwrap(),
            "arl=sentinel; sid=session; datadome=guard"
        );
        assert!(attached.cookie_header().unwrap().is_sensitive());

        attached
            .attached_jar()
            .unwrap()
            .refresh("arl=rotated; sid=rotated");
        assert_eq!(
            attached.cookie_header().unwrap(),
            "arl=sentinel; sid=rotated; datadome=guard"
        );
    }
}
