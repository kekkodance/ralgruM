use reqwest::header::HeaderValue;

use super::models::ProviderError;

#[derive(Clone)]
pub(crate) struct DeezerArl(Box<str>);

#[derive(Clone)]
pub(crate) struct SoundCloudToken(Box<str>);

impl DeezerArl {
    pub(crate) fn from_saved(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()
            && value.len() <= 1024
            && !value
                .chars()
                .any(|character| character.is_control() || matches!(character, ';' | ',')))
        .then(|| Self(value.into()))
    }

    pub(crate) fn cookie_header(&self) -> Result<HeaderValue, ProviderError> {
        let mut value = HeaderValue::from_str(&format!("arl={}", self.0))
            .map_err(|_| ProviderError::new("The saved Deezer session is invalid"))?;
        value.set_sensitive(true);
        Ok(value)
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
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
}
