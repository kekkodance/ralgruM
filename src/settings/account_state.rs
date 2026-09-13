use std::sync::Arc;

use crate::service_auth::{Service, ValidatedCredentials};
use chrono::Utc;
use gpui::SharedString;

use crate::{
    account_session::{ServiceIdentity, SessionError, SessionStore},
    murglar_backend::{
        AccessToken, AccountError, AccountExtras, AccountProfile, DeviceIdentity,
        DeviceIdentityStatus, MediaCredentials, PaymentPlans, ReferralStats, media_credentials,
    },
    search::{DeezerArl, DeezerCookieJar, SoundCloudToken},
};

pub(crate) struct AccountState {
    pub(super) identity: DeviceIdentityStatus,
    settings_active: bool,
    generation: u64,
    pub(super) loading: bool,
    pub(super) status: Option<SharedString>,
    pub(super) device_limit_exceeded: bool,
    pub(super) token: Option<String>,
    pub(super) profile: Option<AccountProfile>,
    pub(super) referral: Option<Arc<ReferralStats>>,
    pub(super) plans: Option<Arc<PaymentPlans>>,
    pub(super) extras_loading: bool,
    pub(super) referral_reloading: bool,
    pub(super) referral_status: Option<SharedString>,
    pub(super) referral_copy_status: Option<SharedString>,
    pub(super) plans_status: Option<SharedString>,
    pub(super) session_store: Option<SessionStore>,
    pub(super) session_error: Option<SessionError>,
    deezer_cookie_jar: DeezerCookieJar,
    pub(super) service_generation: u64,
    pub(super) service_loading: bool,
    service_loading_service: Option<Service>,
    pub(super) service_status: Option<ServiceStatus>,
    service_session_error: Option<(Service, SessionError)>,
    pub(super) deezer_profile: Option<ServiceIdentity>,
    pub(super) deezer_identity_loading: bool,
    deezer_identity_attempted: bool,
    pub(super) soundcloud_profile: Option<ServiceIdentity>,
    pub(super) soundcloud_username: Option<String>,
    pub(super) soundcloud_identity_loading: bool,
    soundcloud_identity_attempted: bool,
    service_identity_generation: u64,
    /// Generation for the currently running Murglar login exchange. It is
    /// independent from the settings view generation so a login may finish
    /// and persist after the settings view closes.
    murglar_login_epoch: u64,
    active_murglar_login_epoch: Option<u64>,
    murglar_summary_trusted: bool,
}

const PASS_EXPIRING_WARNING_MILLIS: i64 = 3 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SidebarPass {
    Active {
        expiration: String,
        expiring_soon: bool,
    },
    Inactive,
}

/// A service-facing notice carries its scope with the message instead of
/// relying on the currently selected settings category to interpret text.
/// `All` is reserved for the explicit account-wide sign-out action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ServiceStatus {
    scope: ServiceStatusScope,
    message: SharedString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceStatusScope {
    Service(Service),
    All,
}

impl ServiceStatus {
    fn for_service(service: Service, message: impl Into<SharedString>) -> Self {
        Self {
            scope: ServiceStatusScope::Service(service),
            message: message.into(),
        }
    }

    fn all(message: impl Into<SharedString>) -> Self {
        Self {
            scope: ServiceStatusScope::All,
            message: message.into(),
        }
    }

    fn applies_to(&self, service: Service) -> bool {
        match self.scope {
            ServiceStatusScope::All => true,
            ServiceStatusScope::Service(scoped) => scoped == service,
        }
    }
}

impl AccountState {
    pub(crate) fn new(
        identity: DeviceIdentityStatus,
        session_store: Result<SessionStore, SessionError>,
    ) -> Self {
        let (session_store, session_error) = match session_store {
            Ok(store) => (Some(store), None),
            Err(error) => (None, Some(error)),
        };
        let token = session_store
            .as_ref()
            .map(|store| store.session().murglar())
            .filter(|token| !token.is_empty())
            .map(str::to_owned);
        let soundcloud_profile = session_store
            .as_ref()
            .and_then(|store| store.session().soundcloud_profile().cloned());
        let deezer_profile = session_store
            .as_ref()
            .and_then(|store| store.session().deezer_profile().cloned());
        let deezer_cookie_jar = DeezerCookieJar::new(
            session_store
                .as_ref()
                .and_then(|store| store.session().deezer_cookies().map(str::to_owned)),
        );
        let murglar_summary_trusted = session_store
            .as_ref()
            .is_some_and(|store| store.session().murglar_profile_summary().is_some());
        let soundcloud_username = soundcloud_profile
            .as_ref()
            .map(|profile| profile.username.clone());
        Self {
            identity,
            settings_active: false,
            generation: 0,
            loading: false,
            status: None,
            device_limit_exceeded: false,
            token,
            profile: None,
            referral: None,
            plans: None,
            extras_loading: false,
            referral_reloading: false,
            referral_status: None,
            referral_copy_status: None,
            plans_status: None,
            session_store,
            session_error,
            deezer_cookie_jar,
            service_generation: 0,
            service_loading: false,
            service_loading_service: None,
            service_status: None,
            service_session_error: None,
            deezer_profile,
            deezer_identity_loading: false,
            deezer_identity_attempted: false,
            soundcloud_profile,
            soundcloud_username,
            soundcloud_identity_loading: false,
            soundcloud_identity_attempted: false,
            service_identity_generation: 0,
            murglar_login_epoch: 0,
            active_murglar_login_epoch: None,
            murglar_summary_trusted,
        }
    }

    pub(super) fn begin_settings_session(&mut self) {
        self.settings_active = true;
        self.generation = self.generation.wrapping_add(1);
        self.service_generation = self.service_generation.wrapping_add(1);
        self.invalidate_service_identity_requests();
        self.loading = false;
        self.service_loading = false;
        self.service_loading_service = None;
        self.status = None;
        self.service_status = None;
        self.service_session_error = None;
        self.deezer_identity_attempted = false;
        self.soundcloud_identity_attempted = false;
        if self.session_store.is_some() {
            self.session_error = None;
        }
        self.device_limit_exceeded = false;
        self.extras_loading = false;
        self.referral_reloading = false;
        self.referral = None;
        self.plans = None;
        self.referral_status = None;
        self.referral_copy_status = None;
        self.plans_status = None;
    }

    pub(super) fn refresh(&mut self) -> Option<(u64, DeviceIdentity, String)> {
        if !self.settings_active || self.loading {
            return None;
        }
        if self.token.is_none() {
            self.token = self
                .session_store
                .as_ref()
                .map(|store| store.session().murglar())
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
        }
        self.status = None;
        self.service_status = None;
        self.device_limit_exceeded = false;
        let identity = match &self.identity {
            DeviceIdentityStatus::ReadyExisting(identity)
            | DeviceIdentityStatus::ReadyCreated(identity) => identity.clone(),
            DeviceIdentityStatus::Error(_) => return None,
        };
        let token = self.token.clone()?;
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        self.referral = None;
        self.plans = None;
        self.extras_loading = true;
        self.referral_reloading = false;
        self.referral_status = None;
        self.referral_copy_status = None;
        self.plans_status = None;
        Some((self.generation, identity, token))
    }

    pub(super) fn begin_referral_refresh(&mut self) -> Option<(u64, DeviceIdentity, String)> {
        if !self.settings_active || self.loading || self.extras_loading || self.referral_reloading {
            return None;
        }
        if self.token.is_none() {
            self.token = self
                .session_store
                .as_ref()
                .map(|store| store.session().murglar())
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
        }
        let identity = match &self.identity {
            DeviceIdentityStatus::ReadyExisting(identity)
            | DeviceIdentityStatus::ReadyCreated(identity) => identity.clone(),
            DeviceIdentityStatus::Error(_) => return None,
        };
        let token = self.token.clone()?;
        self.referral_reloading = true;
        Some((self.generation, identity, token))
    }

    /// Fetches the Murglar profile for the sidebar at startup. Unlike the
    /// settings refresh this runs while the settings page is inactive, matching the
    /// original's boot-time profile load.
    pub(super) fn begin_startup_profile(&mut self) -> Option<(u64, DeviceIdentity, String)> {
        if self.loading || self.profile.is_some() || self.settings_active {
            return None;
        }
        if self.token.is_none() {
            self.token = self
                .session_store
                .as_ref()
                .map(|store| store.session().murglar())
                .filter(|token| !token.is_empty())
                .map(str::to_owned);
        }
        let identity = match &self.identity {
            DeviceIdentityStatus::ReadyExisting(identity)
            | DeviceIdentityStatus::ReadyCreated(identity) => identity.clone(),
            DeviceIdentityStatus::Error(_) => return None,
        };
        let token = self.token.clone()?;
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        Some((self.generation, identity, token))
    }

    pub(super) fn end_settings_session(&mut self) {
        self.settings_active = false;
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.status = None;
        self.service_status = None;
        self.device_limit_exceeded = false;
        self.extras_loading = false;
        self.referral_reloading = false;
        self.referral_status = None;
        self.referral_copy_status = None;
        self.plans_status = None;
        self.service_generation = self.service_generation.wrapping_add(1);
        self.service_loading = false;
        self.service_loading_service = None;
        self.invalidate_service_identity_requests();
        self.service_session_error = None;
        self.deezer_identity_attempted = false;
        self.soundcloud_identity_attempted = false;
    }

    pub(super) fn begin_login(&mut self) -> Option<(u64, DeviceIdentity, u64)> {
        if !self.settings_active || self.loading {
            return None;
        }
        let Some(store) = &self.session_store else {
            self.session_error = Some(SessionError::Filesystem);
            return None;
        };
        if let Err(error) = store.preflight_persist() {
            self.session_error = Some(error);
            return None;
        }
        let identity = match &self.identity {
            DeviceIdentityStatus::ReadyExisting(identity)
            | DeviceIdentityStatus::ReadyCreated(identity) => identity.clone(),
            DeviceIdentityStatus::Error(_) => return None,
        };
        self.generation = self.generation.wrapping_add(1);
        self.murglar_summary_trusted = false;
        self.murglar_login_epoch = self.murglar_login_epoch.wrapping_add(1);
        self.active_murglar_login_epoch = Some(self.murglar_login_epoch);
        self.loading = true;
        self.status = None;
        self.device_limit_exceeded = false;
        self.token = None;
        self.profile = None;
        self.referral = None;
        self.plans = None;
        self.extras_loading = false;
        self.referral_reloading = false;
        self.referral_status = None;
        self.referral_copy_status = None;
        self.plans_status = None;
        self.session_error = None;
        Some((self.generation, identity, self.murglar_login_epoch))
    }

    pub(super) fn complete_token_exchange(
        &mut self,
        generation: u64,
        login_epoch: u64,
        result: Result<AccessToken, AccountError>,
    ) -> Option<String> {
        // Validate every completion before touching durable storage. The
        // settings generation changes when the view closes, so it cannot be
        // used here: a still-current login is allowed to finish in the
        // background. A new login or logout advances this dedicated epoch.
        if self.active_murglar_login_epoch != Some(login_epoch) {
            return None;
        }
        match result {
            Ok(token) => {
                self.active_murglar_login_epoch = None;
                let persisted_token = match self.session_store.as_mut() {
                    Some(store) => {
                        if let Err(error) = store.persist_murglar(token.into_inner()) {
                            if self.settings_active && generation == self.generation {
                                self.loading = false;
                                self.session_error = Some(error);
                                self.status = None;
                                self.device_limit_exceeded = false;
                            }
                            return None;
                        }
                        store.session().murglar().to_owned()
                    }
                    None => {
                        if self.settings_active && generation == self.generation {
                            self.loading = false;
                            self.token = None;
                            self.profile = None;
                            self.session_error = Some(SessionError::Filesystem);
                            self.status = None;
                            self.device_limit_exceeded = false;
                        }
                        return None;
                    }
                };
                if !self.settings_active || generation != self.generation {
                    return None;
                }
                self.token = Some(persisted_token.clone());
                self.profile = None;
                self.session_error = None;
                self.status = None;
                self.device_limit_exceeded = false;
                self.service_generation = self.service_generation.wrapping_add(1);
                Some(persisted_token)
            }
            Err(error) => {
                self.active_murglar_login_epoch = None;
                if !self.settings_active || generation != self.generation {
                    return None;
                }
                self.loading = false;
                self.token = None;
                self.profile = None;
                self.device_limit_exceeded = error == AccountError::DeviceLimitExceeded;
                self.status = (!self.device_limit_exceeded).then(|| error.to_string().into());
                None
            }
        }
    }

    pub(super) fn complete_profile(
        &mut self,
        generation: u64,
        result: Result<AccountProfile, AccountError>,
    ) -> bool {
        // Startup fetches complete while the settings page is inactive, so only the
        // generation guard applies here.
        if generation != self.generation {
            return false;
        }
        self.loading = false;
        match result {
            Ok(profile) => {
                let summary = crate::account_session::MurglarProfileSummary {
                    username: profile.username.clone(),
                    email: profile.email.clone(),
                    premium: profile.permanent_premium_status.to_string(),
                    pass_expiration: profile.pass_expiration_display.clone(),
                    pass_expiration_millis: profile.pass_expiration_millis,
                };
                self.murglar_summary_trusted = self
                    .session_store
                    .as_mut()
                    .is_some_and(|store| store.persist_profile_summary(Some(summary)).is_ok());
                self.profile = Some(profile);
                self.status = None;
                self.device_limit_exceeded = false;
            }
            Err(error @ (AccountError::Unauthorized | AccountError::Forbidden)) => {
                self.murglar_summary_trusted = false;
                self.profile = None;
                self.device_limit_exceeded = false;
                let persisted = self
                    .session_store
                    .as_mut()
                    .ok_or(SessionError::Filesystem)
                    .and_then(|store| store.persist_murglar(String::new()));
                match persisted {
                    Ok(()) => {
                        self.generation = self.generation.wrapping_add(1);
                        self.token = None;
                        if let Some(store) = self.session_store.as_mut() {
                            let _ = store.persist_profile_summary(None);
                        }
                        self.session_error = None;
                    }
                    Err(persist_error) => self.session_error = Some(persist_error),
                }
                self.status = Some(error.to_string().into());
            }
            Err(AccountError::DeviceLimitExceeded) => {
                self.profile = None;
                self.device_limit_exceeded = true;
                self.status = None;
            }
            Err(error) => {
                self.profile = None;
                self.device_limit_exceeded = false;
                self.status = Some(format!("Saved the Murglar session, but the account profile could not be loaded: {error}").into());
            }
        }
        true
    }

    pub(super) fn complete_extras(&mut self, generation: u64, extras: AccountExtras) -> bool {
        if !self.settings_active || generation != self.generation || self.token.is_none() {
            return false;
        }
        self.extras_loading = false;
        self.referral_reloading = false;
        let referral_failed = extras.referral.is_err();
        let plans_failed = extras.plans.is_err();
        self.referral = extras.referral.ok().map(Arc::new);
        self.plans = extras.plans.ok().map(Arc::new);
        self.referral_status =
            referral_failed.then(|| "Referral details are temporarily unavailable.".into());
        self.plans_status =
            plans_failed.then(|| "Murglar plans are temporarily unavailable.".into());
        true
    }

    pub(super) fn complete_referral(
        &mut self,
        generation: u64,
        result: Result<ReferralStats, AccountError>,
    ) -> bool {
        if !self.settings_active
            || generation != self.generation
            || self.token.is_none()
            || !self.referral_reloading
        {
            return false;
        }
        self.referral_reloading = false;
        match result {
            Ok(stats) => {
                self.referral = Some(Arc::new(stats));
                self.referral_status = None;
            }
            Err(_) => {
                self.referral_status = Some("Referral details are temporarily unavailable.".into());
            }
        }
        true
    }

    pub(super) fn logout(&mut self) {
        self.murglar_login_epoch = self.murglar_login_epoch.wrapping_add(1);
        self.active_murglar_login_epoch = None;
        self.generation = self.generation.wrapping_add(1);
        self.murglar_summary_trusted = false;
        self.loading = false;
        self.status = None;
        self.device_limit_exceeded = false;
        self.profile = None;
        self.referral = None;
        self.plans = None;
        self.extras_loading = false;
        self.referral_reloading = false;
        self.referral_status = None;
        self.referral_copy_status = None;
        self.plans_status = None;
        let persisted = self
            .session_store
            .as_mut()
            .ok_or(SessionError::Filesystem)
            .and_then(|store| store.persist_murglar(String::new()));
        match persisted {
            Ok(()) => {
                self.token = None;
                if let Some(store) = self.session_store.as_mut() {
                    let _ = store.persist_profile_summary(None);
                }
                self.session_error = None;
            }
            Err(error) => self.session_error = Some(error),
        }
    }

    pub(crate) fn sidebar_username(&self) -> &str {
        if let Some(profile) = &self.profile {
            if !profile.username.trim().is_empty() {
                return &profile.username;
            } else if !profile.email.trim().is_empty() {
                return &profile.email;
            } else {
                return "Murglar user";
            }
        }
        if let Some(summary) = self.stored_summary() {
            return summary.display_name();
        }
        "Not signed in"
    }

    fn stored_summary(&self) -> Option<&crate::account_session::MurglarProfileSummary> {
        self.session_store
            .as_ref()
            .and_then(|store| store.session().murglar_profile_summary())
    }

    pub(crate) fn deezer_arl(&self) -> Option<DeezerArl> {
        self.session_store.as_ref().and_then(|store| {
            DeezerArl::from_saved_with_jar(store.session().deezer(), &self.deezer_cookie_jar)
        })
    }

    pub(crate) fn deezer_user_id(&self) -> Option<String> {
        self.session_store
            .as_ref()
            .map(|store| store.session().deezer_user_id().trim())
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }

    pub(crate) fn deezer_profile(&self) -> Option<&crate::account_session::ServiceIdentity> {
        self.session_store
            .as_ref()
            .and_then(|store| store.session().deezer_profile())
    }

    pub(crate) fn library_scope(&self) -> String {
        let Some(store) = &self.session_store else {
            return String::new();
        };
        format!(
            "{}\n{}\n{}\n{}",
            store.session().deezer(),
            store.session().deezer_user_id(),
            store.session().soundcloud(),
            store.session().soundcloud_mobile()
        )
    }

    pub(crate) fn soundcloud_token(&self) -> Option<SoundCloudToken> {
        self.session_store
            .as_ref()
            .and_then(|store| SoundCloudToken::from_saved(store.session().soundcloud()))
    }

    pub(crate) fn soundcloud_mobile_token(&self) -> Option<SoundCloudToken> {
        self.session_store
            .as_ref()
            .and_then(|store| SoundCloudToken::from_saved(store.session().soundcloud_mobile()))
    }

    pub(crate) fn soundcloud_session_cookies(&self) -> Option<String> {
        let session = self.session_store.as_ref()?.session();
        let saved = session.soundcloud_cookies().trim();
        if !saved.is_empty() {
            return Some(saved.to_owned());
        }
        SoundCloudToken::from_saved(session.soundcloud())
            .map(|token| format!("oauth_token={}", token.expose()))
    }

    pub(crate) fn service_identity(&self, service: Service) -> Option<&ServiceIdentity> {
        let profile = match service {
            Service::Deezer => self.deezer_profile.as_ref(),
            Service::SoundCloud => self.soundcloud_profile.as_ref(),
        }?;
        if profile.username.trim().is_empty()
            || (service == Service::Deezer
                && (profile
                    .username
                    .trim()
                    .chars()
                    .all(|character| character.is_ascii_digit())
                    || profile
                        .username
                        .trim()
                        .eq_ignore_ascii_case("Deezer account")))
        {
            None
        } else {
            Some(profile)
        }
    }

    #[cfg(test)]
    pub(crate) fn service_username(&self, service: Service) -> Option<&str> {
        self.service_identity(service)
            .map(|profile| profile.username.as_str())
    }

    #[cfg(test)]
    pub(crate) fn service_avatar_url(&self, service: Service) -> Option<&str> {
        self.service_identity(service)
            .and_then(crate::account_session::ServiceIdentity::safe_avatar_url)
    }

    pub(super) fn service_status_for(&self, service: Service) -> Option<SharedString> {
        self.service_status
            .as_ref()
            .filter(|status| status.applies_to(service))
            .map(|status| status.message.clone())
    }

    pub(super) fn service_loading_for(&self, service: Service) -> bool {
        self.service_loading && self.service_loading_service == Some(service)
    }

    pub(super) fn service_session_error_for(&self, service: Service) -> Option<SharedString> {
        self.service_session_error
            .filter(|(scope, _)| *scope == service)
            .map(|(_, error)| error.to_string().into())
    }

    pub(super) fn should_refresh_service_identity(&self, service: Service) -> bool {
        self.service_signed_in(service)
            && self.service_identity(service).is_none()
            && !self.service_identity_attempted(service)
            && !self.service_loading
            && !self.deezer_identity_loading
            && !self.soundcloud_identity_loading
    }

    fn service_identity_attempted(&self, service: Service) -> bool {
        match service {
            Service::Deezer => self.deezer_identity_attempted,
            Service::SoundCloud => self.soundcloud_identity_attempted,
        }
    }

    fn reset_service_identity_attempt(&mut self, service: Service) {
        match service {
            Service::Deezer => self.deezer_identity_attempted = false,
            Service::SoundCloud => self.soundcloud_identity_attempted = false,
        }
    }

    fn clear_service_session_error(&mut self, service: Service) {
        if self
            .service_session_error
            .is_some_and(|(scope, _)| scope == service)
        {
            self.service_session_error = None;
        }
    }

    fn set_service_status(&mut self, service: Service, message: impl Into<SharedString>) {
        self.service_status = Some(ServiceStatus::for_service(service, message));
    }

    fn set_all_service_status(&mut self, message: impl Into<SharedString>) {
        self.service_status = Some(ServiceStatus::all(message));
    }

    pub(super) fn begin_service_auth(&mut self, service: Service) -> Option<u64> {
        if self.service_loading {
            self.set_service_status(
                service,
                "Another sign-in is still finishing. Try again in a moment.",
            );
            return None;
        }
        if self.session_store.is_none() {
            self.session_error = Some(SessionError::Filesystem);
            self.service_session_error = Some((service, SessionError::Filesystem));
            return None;
        }
        self.service_generation = self.service_generation.wrapping_add(1);
        self.invalidate_service_identity_requests();
        self.service_loading = true;
        self.service_loading_service = Some(service);
        self.service_status = None;
        self.clear_service_session_error(service);
        Some(self.service_generation)
    }

    pub(super) fn complete_service_auth(
        &mut self,
        generation: u64,
        result: Result<ValidatedCredentials, String>,
        service: Service,
    ) {
        if generation != self.service_generation {
            return;
        }
        self.service_loading = false;
        self.service_loading_service = None;
        // A service-auth attempt is terminal even when validation or persistence
        // fails. Advancing the generation prevents a duplicate or delayed
        // completion from being applied to a later provider attempt.
        self.service_generation = self.service_generation.wrapping_add(1);
        match result {
            Ok(credentials) => {
                let profile = credentials.identity.clone();
                let profile_loaded = profile.is_some();
                let deezer_cookies = match service {
                    Service::Deezer => credentials
                        .deezer_cookies
                        .clone()
                        .or_else(|| self.deezer_cookie_jar.snapshot()),
                    Service::SoundCloud => None,
                };
                let result = self
                    .session_store
                    .as_mut()
                    .ok_or(SessionError::Filesystem)
                    .and_then(|store| {
                        store.persist_service_login(
                            service,
                            credentials.desktop,
                            credentials.mobile,
                            credentials.soundcloud_cookies,
                            deezer_cookies,
                            credentials.deezer_user_id,
                            profile.clone(),
                        )
                    });
                let saved = result.is_ok();
                match result {
                    Ok(()) => {
                        self.clear_service_session_error(service);
                        self.service_status = None;
                    }
                    Err(error) => {
                        if self.session_store.is_none() {
                            self.session_error = Some(error);
                        }
                        self.service_session_error = Some((service, error));
                        self.set_service_status(service, "The account could not be saved.");
                    }
                }
                if saved {
                    self.clear_service_session_error(service);
                    match service {
                        Service::Deezer => {
                            self.deezer_profile = profile;
                            self.deezer_identity_loading = false;
                            if let Some(harvest) = &credentials.deezer_cookies {
                                self.deezer_cookie_jar =
                                    DeezerCookieJar::new(Some(harvest.clone()));
                            }
                        }
                        Service::SoundCloud => {
                            self.soundcloud_username =
                                profile.as_ref().map(|profile| profile.username.clone());
                            self.soundcloud_profile = profile;
                            self.soundcloud_identity_loading = false;
                        }
                    }
                    if !profile_loaded {
                        self.reset_service_identity_attempt(service);
                    }
                }
            }
            Err(error) => self.set_service_status(service, error),
        }
    }

    pub(super) fn service_signed_in(&self, service: Service) -> bool {
        let Some(store) = &self.session_store else {
            return false;
        };
        match service {
            Service::Deezer => {
                !store.session().deezer().trim().is_empty()
                    && !store.session().deezer_user_id().trim().is_empty()
            }
            Service::SoundCloud => {
                !store.session().soundcloud().trim().is_empty()
                    && !store.session().soundcloud_mobile().trim().is_empty()
            }
        }
    }

    pub(super) fn logout_service(&mut self, service: Service) {
        self.service_generation = self.service_generation.wrapping_add(1);
        self.invalidate_service_identity_requests();
        self.service_loading = false;
        self.service_loading_service = None;
        self.reset_service_identity_attempt(service);
        let result = self
            .session_store
            .as_mut()
            .ok_or(SessionError::Filesystem)
            .and_then(|store| store.clear_service(service));
        let cleared = result.is_ok();
        match result {
            Ok(()) => {
                self.clear_service_session_error(service);
                self.set_service_status(service, "Signed out.")
            }
            Err(error) => {
                if self.session_store.is_none() {
                    self.session_error = Some(error);
                }
                self.service_session_error = Some((service, error));
                self.set_service_status(service, "The account could not be signed out.");
            }
        }
        if cleared {
            match service {
                Service::Deezer => {
                    self.deezer_profile = None;
                    self.deezer_cookie_jar = DeezerCookieJar::default();
                }
                Service::SoundCloud => {
                    self.soundcloud_profile = None;
                    self.soundcloud_username = None;
                }
            }
        }
    }

    pub(super) fn logout_all(&mut self) {
        self.murglar_login_epoch = self.murglar_login_epoch.wrapping_add(1);
        self.active_murglar_login_epoch = None;
        self.generation = self.generation.wrapping_add(1);
        self.murglar_summary_trusted = false;
        self.service_generation = self.service_generation.wrapping_add(1);
        self.invalidate_service_identity_requests();
        self.loading = false;
        self.service_loading = false;
        self.service_loading_service = None;
        self.service_session_error = None;
        self.deezer_identity_attempted = false;
        self.soundcloud_identity_attempted = false;
        self.profile = None;
        self.referral = None;
        self.plans = None;
        self.extras_loading = false;
        self.referral_reloading = false;
        let result = self
            .session_store
            .as_mut()
            .ok_or(SessionError::Filesystem)
            .and_then(SessionStore::clear_all_services);
        match result {
            Ok(()) => {
                self.token = None;
                self.deezer_profile = None;
                self.deezer_cookie_jar = DeezerCookieJar::default();
                self.soundcloud_username = None;
                self.soundcloud_profile = None;
                self.session_error = None;
                self.set_all_service_status("All accounts are signed out.");
            }
            Err(error) => {
                self.session_error = Some(error);
                self.set_all_service_status("The account sessions could not be cleared.");
            }
        }
    }

    pub(crate) fn murglar_credentials(&self) -> Option<(DeviceIdentity, String)> {
        let identity = match &self.identity {
            DeviceIdentityStatus::ReadyExisting(identity)
            | DeviceIdentityStatus::ReadyCreated(identity) => identity.clone(),
            DeviceIdentityStatus::Error(_) => return None,
        };
        let token = self
            .token
            .clone()
            .filter(|token| !token.trim().is_empty())?;
        Some((identity, token))
    }

    pub(crate) fn murglar_media_credentials(&self) -> Option<MediaCredentials> {
        self.murglar_media_credentials_at(Utc::now().timestamp_millis())
    }

    fn murglar_media_credentials_at(&self, now_millis: i64) -> Option<MediaCredentials> {
        self.has_confirmed_active_pass_at(now_millis)
            .then(|| self.murglar_credentials())
            .flatten()
            .and_then(|(identity, token)| media_credentials(&identity, &token))
    }

    fn has_confirmed_active_pass_at(&self, now_millis: i64) -> bool {
        let expiration = match self.profile.as_ref() {
            Some(profile) => profile.pass_expiration_millis,
            None if self.murglar_summary_trusted => self
                .stored_summary()
                .and_then(|summary| summary.pass_expiration_millis),
            None => None,
        };
        expiration.is_some_and(|expiration| expiration > now_millis)
    }

    /// Opaque account credential generation for short-lived capability caches.
    /// It deliberately exposes no token material and changes whenever a
    /// service or Murglar account scope changes.
    pub(crate) fn credential_generation(&self) -> u128 {
        ((self.generation as u128) << 66)
            | ((self.service_generation as u128) << 2)
            | (u128::from(self.has_confirmed_active_pass_at(Utc::now().timestamp_millis())) << 1)
            | u128::from(self.murglar_summary_trusted)
    }

    pub(super) fn soundcloud_identity_loading(&self) -> bool {
        self.soundcloud_identity_loading
    }

    pub(super) fn begin_soundcloud_identity_with_generation(
        &mut self,
    ) -> Option<(u64, SoundCloudToken)> {
        if self.soundcloud_identity_loading
            || self.soundcloud_profile.is_some()
            || self.soundcloud_identity_attempted
        {
            return None;
        }
        let token = self.soundcloud_token()?;
        let generation = self.invalidate_service_identity_requests();
        self.soundcloud_identity_loading = true;
        self.soundcloud_identity_attempted = true;
        self.clear_service_session_error(Service::SoundCloud);
        Some((generation, token))
    }

    pub(super) fn complete_soundcloud_profile(
        &mut self,
        generation: u64,
        result: Result<ServiceIdentity, String>,
    ) {
        if generation != self.service_identity_generation {
            return;
        }
        self.soundcloud_identity_loading = false;
        match result {
            Ok(profile) => {
                let persisted =
                    self.session_store
                        .as_mut()
                        .map_or(Err(SessionError::Filesystem), |store| {
                            store
                                .persist_service_profile(Service::SoundCloud, Some(profile.clone()))
                        });
                match persisted {
                    Ok(()) => {
                        self.clear_service_session_error(Service::SoundCloud);
                        self.soundcloud_username = Some(profile.username.clone());
                        self.soundcloud_profile = Some(profile);
                    }
                    Err(error) => {
                        if self.session_store.is_none() {
                            self.session_error = Some(error);
                        }
                        self.service_session_error = Some((Service::SoundCloud, error));
                    }
                }
            }
            Err(error) => self.set_service_status(Service::SoundCloud, error),
        }
    }

    pub(super) fn begin_deezer_identity_with_generation(&mut self) -> Option<(u64, DeezerArl)> {
        if self.deezer_identity_loading
            || self.service_identity(Service::Deezer).is_some()
            || self.deezer_identity_attempted
        {
            return None;
        }
        let arl = self.deezer_arl()?;
        let generation = self.invalidate_service_identity_requests();
        self.deezer_identity_loading = true;
        self.deezer_identity_attempted = true;
        self.clear_service_session_error(Service::Deezer);
        Some((generation, arl))
    }

    pub(super) fn deezer_identity_loading(&self) -> bool {
        self.deezer_identity_loading
    }

    pub(super) fn complete_deezer_profile(
        &mut self,
        generation: u64,
        result: Result<ServiceIdentity, String>,
    ) {
        if generation != self.service_identity_generation {
            return;
        }
        self.deezer_identity_loading = false;
        match result {
            Ok(profile) => {
                let jar_snapshot = self.deezer_cookie_jar.snapshot();
                let persisted = self
                    .session_store
                    .as_mut()
                    .ok_or(SessionError::Filesystem)
                    .and_then(|store| {
                        store.persist_service_profile(Service::Deezer, Some(profile.clone()))?;
                        match jar_snapshot {
                            Some(cookies) => store.persist_deezer_cookies(Some(cookies)),
                            None => Ok(()),
                        }
                    });
                if let Err(error) = persisted {
                    if self.session_store.is_none() {
                        self.session_error = Some(error);
                    }
                    self.service_session_error = Some((Service::Deezer, error));
                } else {
                    self.clear_service_session_error(Service::Deezer);
                    self.deezer_profile = Some(profile);
                }
            }
            Err(error) => self.set_service_status(Service::Deezer, error),
        }
    }

    pub(super) fn cancel_service_identity(&mut self, generation: u64, service: Service) {
        if generation != self.service_identity_generation {
            return;
        }
        match service {
            Service::Deezer => self.deezer_identity_loading = false,
            Service::SoundCloud => self.soundcloud_identity_loading = false,
        }
    }

    fn invalidate_service_identity_requests(&mut self) -> u64 {
        self.service_identity_generation = self.service_identity_generation.wrapping_add(1);
        self.deezer_identity_loading = false;
        self.soundcloud_identity_loading = false;
        self.service_identity_generation
    }

    pub(crate) fn sidebar_premium(&self) -> &'static str {
        if let Some(profile) = &self.profile {
            return profile.permanent_premium_status;
        }
        match self.stored_summary() {
            Some(summary) if summary.premium == "Yes" => "Yes",
            _ => "No",
        }
    }

    pub(crate) fn sidebar_pass(&self) -> SidebarPass {
        self.sidebar_pass_at(Utc::now().timestamp_millis())
    }

    fn sidebar_pass_at(&self, now_millis: i64) -> SidebarPass {
        let (expiration, expiration_millis) = if let Some(profile) = &self.profile {
            (
                profile.pass_expiration_display.as_str(),
                profile.pass_expiration_millis,
            )
        } else if self.murglar_summary_trusted {
            let Some(summary) = self.stored_summary() else {
                return SidebarPass::Inactive;
            };
            (
                summary.pass_expiration.as_str(),
                summary.pass_expiration_millis,
            )
        } else {
            return SidebarPass::Inactive;
        };
        let Some(expiration_millis) = expiration_millis.filter(|value| *value > now_millis) else {
            return SidebarPass::Inactive;
        };
        SidebarPass::Active {
            expiration: expiration.to_owned(),
            expiring_soon: expiration_millis - now_millis <= PASS_EXPIRING_WARNING_MILLIS,
        }
    }
}

#[cfg(test)]
mod tests;
