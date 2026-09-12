#![cfg_attr(not(ralgrum_private_backend), allow(dead_code))]

use std::fmt;

use reqwest::Client;
use tokio_util::sync::CancellationToken;

use crate::{
    murglar_backend::{AccessToken, AccountError, AccountExtras, AccountProfile},
    playback::media_source::{BackendFormat, BackendSource, MediaRequest, MediaResolveOutcome},
};

#[derive(Clone, Default)]
pub(crate) struct MurglarClient;

impl MurglarClient {
    pub(crate) fn new(_identity: DeviceIdentity) -> Result<Self, AccountError> {
        Ok(Self)
    }

    pub(crate) fn with_client(_client: Client, _identity: DeviceIdentity) -> Self {
        Self
    }

    pub(crate) async fn exchange_token(
        &self,
        _username: String,
        _password: String,
    ) -> Result<AccessToken, AccountError> {
        Err(AccountError::NetworkUnavailable)
    }

    pub(crate) async fn account_profile(
        &self,
        _token: &str,
    ) -> Result<AccountProfile, AccountError> {
        Err(AccountError::NetworkUnavailable)
    }

    pub(crate) async fn account_extras(&self, _token: &str) -> AccountExtras {
        Self::empty_extras()
    }

    pub(crate) async fn generate_payment_link(
        &self,
        _token: &str,
        _promotion_id: &str,
        _merchant_id: &str,
    ) -> Result<String, AccountError> {
        Err(AccountError::NetworkUnavailable)
    }

    pub(crate) fn empty_extras() -> AccountExtras {
        AccountExtras {
            referral: Err(AccountError::NetworkUnavailable),
            plans: Err(AccountError::NetworkUnavailable),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct DeviceIdentity;

impl DeviceIdentity {
    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        Self
    }
}

#[cfg(test)]
pub(crate) fn test_device_identity() -> DeviceIdentity {
    DeviceIdentity::fixture()
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceIdentity")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeviceIdentityError {
    Serialization,
    HardwareCollectionUnavailable,
}

impl DeviceIdentityError {
    pub(crate) fn account_message(self) -> &'static str {
        match self {
            Self::Serialization => "Account identity data could not be prepared.",
            Self::HardwareCollectionUnavailable => {
                "Account identity hardware information is unavailable."
            }
        }
    }
}

impl fmt::Display for DeviceIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.account_message())
    }
}

impl std::error::Error for DeviceIdentityError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DeviceIdentityStatus {
    ReadyExisting(DeviceIdentity),
    ReadyCreated(DeviceIdentity),
    Error(DeviceIdentityError),
}

pub(crate) fn load_current_user() -> DeviceIdentityStatus {
    DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable)
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct MediaCredentials;

impl fmt::Debug for MediaCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaCredentials")
            .finish_non_exhaustive()
    }
}

pub(crate) fn media_credentials(
    _identity: &DeviceIdentity,
    _token: &str,
) -> Option<MediaCredentials> {
    None
}

#[cfg(test)]
pub(crate) fn test_media_credentials() -> MediaCredentials {
    MediaCredentials
}

#[derive(Clone, Default)]
pub(crate) struct MediaBackend;

impl MediaBackend {
    pub(crate) fn new(_client: Client) -> Self {
        Self
    }

    pub(crate) async fn resolve(
        &self,
        _request: MediaRequest,
        _credentials: &MediaCredentials,
        cancellation: &CancellationToken,
    ) -> MediaResolveOutcome<BackendSource> {
        if cancellation.is_cancelled() {
            MediaResolveOutcome::Cancelled
        } else {
            MediaResolveOutcome::Unavailable
        }
    }

    pub(crate) async fn probe(
        &self,
        _request: MediaRequest,
        _credentials: &MediaCredentials,
        cancellation: &CancellationToken,
    ) -> MediaResolveOutcome<BackendFormat> {
        if cancellation.is_cancelled() {
            MediaResolveOutcome::Cancelled
        } else {
            MediaResolveOutcome::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_reports_unavailable_identity() {
        assert_eq!(
            load_current_user(),
            DeviceIdentityStatus::Error(DeviceIdentityError::HardwareCollectionUnavailable)
        );
    }

    #[test]
    fn stub_does_not_create_media_credentials() {
        assert!(media_credentials(&DeviceIdentity::fixture(), "secret-token").is_none());
    }
}
