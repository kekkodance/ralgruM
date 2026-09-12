include!(concat!(env!("OUT_DIR"), "/murglar_backend_selector.rs"));
mod types;

#[cfg(test)]
pub(crate) use implementation::DeviceIdentityError;
pub(crate) use implementation::MediaCredentials;
pub(crate) use implementation::MurglarClient;
#[cfg(test)]
pub(crate) use implementation::test_device_identity;
#[cfg(test)]
pub(crate) use implementation::test_media_credentials;
pub(crate) use implementation::{DeviceIdentity, DeviceIdentityStatus, load_current_user};
pub(crate) use implementation::{MediaBackend, media_credentials};
pub(crate) use types::{
    AccessToken, AccountError, AccountExtras, AccountProfile, PaymentPlan, PaymentPlans,
    ReferralStats,
};
#[cfg(any(ralgrum_private_backend, test))]
#[allow(unused_imports)]
pub(crate) use types::{PassStatus, PaymentAmount, PaymentMerchant, ValidationError};

#[cfg(test)]
mod privacy_tests;
