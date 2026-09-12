use super::*;
use crate::account_session::{SessionError, SessionStore};
use crate::murglar_backend::{
    AccountError, AccountExtras, PassStatus, PaymentPlans, ReferralStats,
};
use crate::service_auth::{Service, ValidatedCredentials};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

fn identity() -> DeviceIdentityStatus {
    DeviceIdentityStatus::ReadyExisting(crate::murglar_backend::test_device_identity())
}

fn profile() -> AccountProfile {
    AccountProfile {
        username: "listener".into(),
        email: "listener@example.test".into(),
        permanent_premium: true,
        permanent_premium_status: "Yes",
        pass_expiration_display: "Jan 2, 2030".into(),
        pass_expiration_millis: Some(1_893_542_400_000),
        pass_status: PassStatus::Active,
        pass_limit_exceeded: false,
    }
}

fn account_state(temp: &TempDir) -> AccountState {
    AccountState::new(identity(), SessionStore::load(temp.path()))
}

fn assert_media_credentials_at(state: &AccountState, now: i64) {
    #[cfg(ralgrum_private_backend)]
    assert!(state.murglar_media_credentials_at(now).is_some());
    #[cfg(not(ralgrum_private_backend))]
    assert!(state.murglar_media_credentials_at(now).is_none());
}

fn assert_media_credentials(state: &AccountState) {
    #[cfg(ralgrum_private_backend)]
    assert!(state.murglar_media_credentials().is_some());
    #[cfg(not(ralgrum_private_backend))]
    assert!(state.murglar_media_credentials().is_none());
}

fn account_state_with_saved_accounts(temp: &TempDir) -> AccountState {
    fs::write(
        temp.path().join("auth_session.json"),
        serde_json::to_vec(&json!({
            "murglar": "saved-token",
            "soundcloud": "soundcloud-token",
            "soundcloudMobile": "mobile-token",
            "deezer": "deezer-token",
            "deezerUserId": "42"
        }))
        .unwrap(),
    )
    .unwrap();
    account_state(temp)
}

#[test]
fn soundcloud_desktop_and_mobile_tokens_remain_separate() {
    let temp = TempDir::new().unwrap();
    let state = account_state_with_saved_accounts(&temp);

    assert_eq!(
        state.soundcloud_token().unwrap().expose(),
        "soundcloud-token"
    );
    assert_eq!(
        state.soundcloud_mobile_token().unwrap().expose(),
        "mobile-token"
    );
    assert_eq!(
        state.soundcloud_session_cookies().as_deref(),
        Some("oauth_token=soundcloud-token")
    );
}

#[test]
fn library_scope_changes_with_the_mobile_soundcloud_credential() {
    let first = TempDir::new().unwrap();
    let second = TempDir::new().unwrap();
    let first_state = account_state_with_saved_accounts(&first);
    fs::write(
        second.path().join("auth_session.json"),
        serde_json::to_vec(&json!({
            "murglar": "saved-token",
            "soundcloud": "soundcloud-token",
            "soundcloudMobile": "other-mobile-token",
            "deezer": "deezer-token",
            "deezerUserId": "42"
        }))
        .unwrap(),
    )
    .unwrap();
    let second_state = account_state(&second);

    assert_ne!(first_state.library_scope(), second_state.library_scope());
}

fn account_state_with_saved_pass(temp: &TempDir, pass_expiration_millis: i64) -> AccountState {
    fs::write(
        temp.path().join("auth_session.json"),
        serde_json::to_vec(&json!({
            "murglar": "saved-token",
            "murglarProfileSummary": {
                "username": "listener",
                "passExpiration": "Persisted expiration",
                "passExpirationMillis": pass_expiration_millis
            }
        }))
        .unwrap(),
    )
    .unwrap();
    account_state(temp)
}

fn make_store_unwritable(temp: &TempDir) {
    fs::remove_dir_all(temp.path()).unwrap();
    fs::write(temp.path(), b"not a directory").unwrap();
}

#[test]
fn stale_session_end_after_exchange_persists_without_exposing_token_or_profile() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    state.end_settings_session();

    assert!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Ok(AccessToken::new("token".into()))
            )
            .is_none()
    );
    assert!(state.token.is_none());
    assert!(state.profile.is_none());
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        "token"
    );
    state.begin_settings_session();
    let (_, _, refresh_token) = state.refresh().unwrap();
    assert_eq!(refresh_token, "token");
}

#[test]
fn superseded_login_completion_is_rejected_before_persistence() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (first_generation, _, first_login_epoch) = state.begin_login().unwrap();

    state.end_settings_session();
    state.begin_settings_session();
    let (second_generation, _, second_login_epoch) = state.begin_login().unwrap();
    assert_ne!(first_login_epoch, second_login_epoch);

    assert!(
        state
            .complete_token_exchange(
                first_generation,
                first_login_epoch,
                Ok(AccessToken::new("stale-token".into())),
            )
            .is_none()
    );
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        ""
    );

    assert_eq!(
        state.complete_token_exchange(
            second_generation,
            second_login_epoch,
            Ok(AccessToken::new("current-token".into())),
        ),
        Some("current-token".into())
    );
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        "current-token"
    );
}

#[test]
fn token_exchange_then_profile_success_updates_profile_and_token() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    let token = state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("token".into())),
    );
    assert_eq!(token.as_deref(), Some("token"));
    assert!(state.complete_profile(generation, Ok(profile())));
    assert_eq!(state.token.as_deref(), Some("token"));
    assert_eq!(state.profile.as_ref().unwrap().username, "listener");
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        "token"
    );
    assert_eq!(
        SessionStore::load(temp.path())
            .unwrap()
            .session()
            .murglar_profile_summary()
            .unwrap()
            .pass_expiration_millis,
        Some(1_893_542_400_000)
    );
}

#[test]
fn sidebar_pass_uses_strict_expiry_and_inclusive_three_day_warning() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    let now = 1_700_000_000_000;
    let mut current = profile();
    current.pass_expiration_display = "Nov 17, 2023".into();

    current.pass_expiration_millis = Some(now + PASS_EXPIRING_WARNING_MILLIS + 1);
    state.profile = Some(current.clone());
    assert_eq!(
        state.sidebar_pass_at(now),
        SidebarPass::Active {
            expiration: "Nov 17, 2023".into(),
            expiring_soon: false,
        }
    );

    current.pass_expiration_millis = Some(now + PASS_EXPIRING_WARNING_MILLIS);
    state.profile = Some(current.clone());
    assert_eq!(
        state.sidebar_pass_at(now),
        SidebarPass::Active {
            expiration: "Nov 17, 2023".into(),
            expiring_soon: true,
        }
    );

    current.pass_expiration_millis = Some(now + 1);
    state.profile = Some(current.clone());
    assert!(matches!(
        state.sidebar_pass_at(now),
        SidebarPass::Active {
            expiring_soon: true,
            ..
        }
    ));

    current.pass_expiration_millis = Some(now);
    state.profile = Some(current.clone());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
    current.pass_expiration_millis = Some(now - 1);
    state.profile = Some(current.clone());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
    current.pass_expiration_millis = None;
    state.profile = Some(current);
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
}

#[test]
fn media_credentials_require_a_confirmed_current_pass() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.token = Some("token".into());
    let now = 1_700_000_000_000;

    assert!(state.murglar_credentials().is_some());
    assert!(state.murglar_media_credentials_at(now).is_none());

    let mut current = profile();
    current.pass_expiration_millis = Some(now);
    state.profile = Some(current.clone());
    assert!(state.murglar_media_credentials_at(now).is_none());

    current.pass_expiration_millis = Some(now + 1);
    state.profile = Some(current);
    assert_media_credentials_at(&state, now);
}

#[test]
fn startup_state_with_active_persisted_summary_grants_media_credentials() {
    let temp = TempDir::new().unwrap();
    let now = 1_700_000_000_000;
    let state = account_state_with_saved_pass(&temp, now + 1);

    assert!(state.profile.is_none());
    assert_media_credentials_at(&state, now);
    assert!(matches!(
        state.sidebar_pass_at(now),
        SidebarPass::Active { .. }
    ));
}

#[test]
fn persisted_summary_at_or_before_expiry_denies_media_credentials() {
    let now = 1_700_000_000_000;

    for expiration in [now, now - 1] {
        let temp = TempDir::new().unwrap();
        let state = account_state_with_saved_pass(&temp, expiration);

        assert!(state.murglar_media_credentials_at(now).is_none());
    }
}

#[test]
fn live_inactive_profile_overrides_active_persisted_summary() {
    let temp = TempDir::new().unwrap();
    let now = 1_700_000_000_000;
    let mut state = account_state_with_saved_pass(&temp, now + 1);
    let mut inactive = profile();
    inactive.pass_expiration_millis = Some(now);
    state.profile = Some(inactive);

    assert!(state.murglar_media_credentials_at(now).is_none());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
}

#[test]
fn transient_profile_failure_keeps_startup_summary_entitlement_available() {
    let temp = TempDir::new().unwrap();
    let now = 1_700_000_000_000;
    let mut state = account_state_with_saved_pass(&temp, now + 1);
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();

    assert!(state.complete_profile(generation, Err(AccountError::NetworkUnavailable)));
    assert!(state.profile.is_none());
    assert_media_credentials_at(&state, now);
    assert!(matches!(
        state.sidebar_pass_at(now),
        SidebarPass::Active { .. }
    ));
}

#[test]
fn replacement_login_suppresses_stored_entitlement_until_fresh_profile_is_accepted() {
    let temp = TempDir::new().unwrap();
    let now = 1_700_000_000_000;
    let mut state = account_state_with_saved_pass(&temp, now + 1);
    assert_media_credentials_at(&state, now);

    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    assert!(state.murglar_media_credentials_at(now).is_none());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);

    let profile_token = state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("replacement-token".into())),
    );
    assert_eq!(profile_token.as_deref(), Some("replacement-token"));
    assert!(state.murglar_media_credentials_at(now).is_none());

    let mut fresh = profile();
    fresh.pass_expiration_millis = Some(now + 1);
    assert!(state.complete_profile(generation, Ok(fresh)));
    assert_media_credentials_at(&state, now);
}

#[test]
fn live_profile_survives_summary_persistence_failure_without_retrusting_fallback() {
    let temp = TempDir::new().unwrap();
    let now = 1_700_000_000_000;
    let mut state = account_state_with_saved_pass(&temp, now + 1);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    assert_eq!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Ok(AccessToken::new("replacement-token".into())),
            )
            .as_deref(),
        Some("replacement-token")
    );
    make_store_unwritable(&temp);

    let mut fresh = profile();
    fresh.pass_expiration_millis = Some(now + 2);
    assert!(state.complete_profile(generation, Ok(fresh)));
    assert_media_credentials_at(&state, now);
    assert!(matches!(
        state.sidebar_pass_at(now),
        SidebarPass::Active { .. }
    ));
    assert_eq!(
        state
            .stored_summary()
            .and_then(|summary| summary.pass_expiration_millis),
        Some(now + 1)
    );

    let (refresh_generation, _, _) = state.refresh().unwrap();
    assert!(state.complete_profile(refresh_generation, Err(AccountError::NetworkUnavailable)));
    assert!(state.profile.is_none());
    assert!(state.murglar_media_credentials_at(now).is_none());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
}

#[test]
fn accepting_a_profile_that_changes_media_eligibility_changes_credential_scope() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();
    let before = state.credential_generation();

    assert!(state.complete_profile(generation, Ok(profile())));

    assert_ne!(state.credential_generation(), before);
    assert_media_credentials(&state);
}

#[test]
fn murglar_scope_changes_again_after_token_commit() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let before_login = state.credential_generation();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    let before_commit = state.credential_generation();
    assert_ne!(before_commit, before_login);

    state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("token".into())),
    );

    let after_commit = state.credential_generation();
    assert_ne!(after_commit, before_commit);
    assert_eq!(state.token.as_deref(), Some("token"));
}

#[test]
fn service_scopes_change_again_after_deezer_and_soundcloud_commit() {
    for service in [Service::Deezer, Service::SoundCloud] {
        let temp = TempDir::new().unwrap();
        let mut state = account_state_with_saved_accounts(&temp);
        state.begin_settings_session();
        let before_auth = state.credential_generation();
        let auth_generation = state.begin_service_auth(service).unwrap();
        let before_commit = state.credential_generation();
        assert_ne!(before_commit, before_auth);
        let credentials = match service {
            Service::Deezer => ValidatedCredentials {
                desktop: "new-deezer".into(),
                mobile: None,
                soundcloud_cookies: None,
                deezer_user_id: Some("99".into()),
                identity: Some(ServiceIdentity::new("deezer-user", None).unwrap()),
            },
            Service::SoundCloud => ValidatedCredentials {
                desktop: "new-soundcloud".into(),
                mobile: Some("new-mobile".into()),
                soundcloud_cookies: Some("oauth_token=new-soundcloud; datadome=guard".into()),
                deezer_user_id: None,
                identity: Some(ServiceIdentity::new("soundcloud-user", None).unwrap()),
            },
        };

        state.complete_service_auth(auth_generation, Ok(credentials), service);

        assert_ne!(state.credential_generation(), before_commit);
        assert_eq!(state.service_status_for(service), None);
    }
}

#[test]
fn service_auth_attempts_are_serial_and_completion_is_terminal() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let first_generation = state.begin_service_auth(Service::Deezer).unwrap();
    assert!(state.begin_service_auth(Service::SoundCloud).is_none());
    assert_eq!(
        state.service_status_for(Service::SoundCloud).as_deref(),
        Some("Another sign-in is still finishing. Try again in a moment.")
    );

    state.complete_service_auth(
        first_generation,
        Err("first provider sign-in was cancelled".into()),
        Service::Deezer,
    );
    let second_generation = state.begin_service_auth(Service::Deezer).unwrap();
    assert_ne!(first_generation, second_generation);

    state.complete_service_auth(
        first_generation,
        Ok(ValidatedCredentials {
            desktop: "stale-deezer".into(),
            mobile: None,
            soundcloud_cookies: None,
            deezer_user_id: Some("stale-user".into()),
            identity: Some(ServiceIdentity::new("stale-user", None).unwrap()),
        }),
        Service::Deezer,
    );
    assert!(state.service_loading);
    assert_eq!(state.service_status_for(Service::Deezer), None);
}

#[test]
fn service_auth_invalidates_both_stale_identity_loaders() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (deezer_generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    let (soundcloud_generation, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    assert!(!state.deezer_identity_loading);
    assert!(state.soundcloud_identity_loading);

    state.begin_service_auth(Service::Deezer).unwrap();
    assert!(!state.deezer_identity_loading);
    assert!(!state.soundcloud_identity_loading);

    state.complete_deezer_profile(deezer_generation, Err("stale Deezer profile".into()));
    state.complete_soundcloud_profile(
        soundcloud_generation,
        Err("stale SoundCloud profile".into()),
    );
    assert!(!state.deezer_identity_loading);
    assert!(!state.soundcloud_identity_loading);
}

#[test]
fn provider_identity_refresh_is_serialized_for_the_combined_settings_page() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    assert!(state.should_refresh_service_identity(Service::Deezer));
    let (generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    assert!(!state.should_refresh_service_identity(Service::SoundCloud));

    state.complete_deezer_profile(
        generation,
        Ok(ServiceIdentity::new("deezer-listener", None).unwrap()),
    );
    assert!(state.should_refresh_service_identity(Service::SoundCloud));
}

#[test]
fn provider_identity_failure_allows_the_other_provider_once_without_retrying() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (deezer_generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    state.complete_deezer_profile(deezer_generation, Err("Deezer profile failed".into()));

    assert!(!state.should_refresh_service_identity(Service::Deezer));
    assert!(state.should_refresh_service_identity(Service::SoundCloud));

    let (soundcloud_generation, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    state.complete_soundcloud_profile(
        soundcloud_generation,
        Err("SoundCloud profile failed".into()),
    );

    assert!(!state.should_refresh_service_identity(Service::Deezer));
    assert!(!state.should_refresh_service_identity(Service::SoundCloud));
}

#[test]
fn successful_provider_identity_persistence_clears_its_scoped_storage_error() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    state.service_session_error = Some((Service::Deezer, SessionError::Filesystem));
    state.complete_deezer_profile(
        generation,
        Ok(ServiceIdentity::new("deezer-listener", None).unwrap()),
    );

    assert_eq!(state.service_session_error_for(Service::Deezer), None);
    assert_eq!(state.service_session_error_for(Service::SoundCloud), None);
}

#[test]
fn provider_persistence_does_not_replace_a_global_murglar_error() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    state.session_error = Some(SessionError::ExistingSessionInvalid);

    let generation = state.begin_service_auth(Service::Deezer).unwrap();
    state.complete_service_auth(
        generation,
        Ok(ValidatedCredentials {
            desktop: "new-deezer".into(),
            mobile: None,
            soundcloud_cookies: None,
            deezer_user_id: Some("99".into()),
            identity: Some(ServiceIdentity::new("deezer-user", None).unwrap()),
        }),
        Service::Deezer,
    );

    assert_eq!(
        state.session_error,
        Some(SessionError::ExistingSessionInvalid)
    );
}

#[test]
fn starting_new_provider_auth_resets_only_that_identity_attempt() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (deezer_generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    state.complete_deezer_profile(deezer_generation, Err("Deezer profile failed".into()));
    let (soundcloud_generation, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    state.complete_soundcloud_profile(
        soundcloud_generation,
        Err("SoundCloud profile failed".into()),
    );

    assert!(state.begin_service_auth(Service::Deezer).is_some());
    assert!(!state.service_loading_for(Service::SoundCloud));
    assert!(!state.should_refresh_service_identity(Service::Deezer));
    assert!(!state.should_refresh_service_identity(Service::SoundCloud));

    let generation = state.service_generation;
    state.complete_service_auth(
        generation,
        Ok(ValidatedCredentials {
            desktop: "new-deezer".into(),
            mobile: None,
            soundcloud_cookies: None,
            deezer_user_id: Some("99".into()),
            identity: None,
        }),
        Service::Deezer,
    );
    assert!(state.should_refresh_service_identity(Service::Deezer));
}

#[test]
fn provider_auth_loading_and_storage_errors_are_scoped_to_the_provider() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    state.begin_service_auth(Service::Deezer).unwrap();
    assert!(state.service_loading_for(Service::Deezer));
    assert!(!state.service_loading_for(Service::SoundCloud));

    state.complete_service_auth(
        state.service_generation,
        Err("Deezer login failed".into()),
        Service::Deezer,
    );
    assert!(!state.service_loading_for(Service::Deezer));
    assert_eq!(state.service_session_error_for(Service::SoundCloud), None);
}

#[test]
fn starting_a_cross_provider_identity_refresh_releases_the_stale_loader() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (deezer_generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    let (_, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    assert!(!state.deezer_identity_loading);
    assert!(state.soundcloud_identity_loading);

    state.complete_deezer_profile(deezer_generation, Err("stale Deezer profile".into()));
    assert!(!state.deezer_identity_loading);
    assert!(state.soundcloud_identity_loading);
}

#[test]
fn cross_provider_logout_releases_the_invalidated_identity_loader() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();

    let (soundcloud_generation, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    assert!(state.soundcloud_identity_loading);

    state.logout_service(Service::Deezer);
    assert!(!state.soundcloud_identity_loading);

    state.complete_soundcloud_profile(
        soundcloud_generation,
        Err("stale SoundCloud profile".into()),
    );
    assert!(!state.soundcloud_identity_loading);
}

#[test]
fn service_status_is_not_rendered_for_another_provider() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);

    state.logout_service(Service::Deezer);

    assert_eq!(
        state.service_status_for(Service::Deezer).as_deref(),
        Some("Signed out.")
    );
    assert_eq!(state.service_status_for(Service::SoundCloud), None);
}

#[test]
fn logout_all_rejects_late_murglar_exchange_before_persistence() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();

    state.logout_all();
    let post_logout_scope = state.credential_generation();

    assert!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Ok(AccessToken::new("late-token".into())),
            )
            .is_none()
    );
    assert!(state.token.is_none());
    assert_eq!(state.credential_generation(), post_logout_scope);
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        ""
    );
}

#[test]
fn murglar_logout_rejects_late_exchange_before_persistence() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();

    state.logout();
    let post_logout_scope = state.credential_generation();

    assert!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Ok(AccessToken::new("late-token".into())),
            )
            .is_none()
    );
    assert!(state.token.is_none());
    assert_eq!(state.credential_generation(), post_logout_scope);
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        ""
    );
}

#[test]
fn profile_and_sidebar_use_their_requested_username_fallbacks() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    assert_eq!(state.sidebar_username(), "Not signed in");
    state.profile = Some(AccountProfile {
        username: String::new(),
        ..profile()
    });
    assert_eq!(state.sidebar_username(), "listener@example.test");
    assert_eq!(
        super::super::murglar_panel::fallback(&state.profile.as_ref().unwrap().username),
        "N/A"
    );
    state.profile.as_mut().unwrap().email = "  ".into();
    assert_eq!(state.sidebar_username(), "Murglar user");
}

#[test]
fn logout_clears_account_and_invalidates_pending_login() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("token".into())),
    );
    state.complete_profile(generation, Ok(profile()));
    state.logout();
    assert!(state.token.is_none());
    assert!(state.profile.is_none());
    assert!(!state.loading);
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        ""
    );
}

#[test]
fn device_limit_completion_sets_danger_state() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    assert!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Err(AccountError::DeviceLimitExceeded)
            )
            .is_none()
    );
    assert!(state.device_limit_exceeded);
    assert!(state.status.is_none());
}

#[test]
fn failed_new_login_cannot_leave_a_stale_account() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    state.token = Some("old-token".into());
    state.profile = Some(profile());

    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    assert!(state.token.is_none());
    assert!(state.profile.is_none());
    assert!(
        state
            .complete_token_exchange(
                generation,
                logout_epoch,
                Err(AccountError::InvalidCredentials)
            )
            .is_none()
    );
    assert!(state.token.is_none());
    assert!(state.profile.is_none());
}

#[test]
fn login_without_writable_session_storage_stops_before_loading() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    make_store_unwritable(&temp);

    assert!(state.begin_login().is_none());
    assert!(!state.loading);
    assert_eq!(state.session_error, Some(SessionError::Filesystem));

    let mut state = AccountState::new(identity(), Err(SessionError::Filesystem));
    state.begin_settings_session();
    assert!(state.begin_login().is_none());
    assert!(!state.loading);
    assert_eq!(state.session_error, Some(SessionError::Filesystem));
}

#[test]
fn unrelated_legacy_session_blocks_login_without_replacing_recovery_material() {
    let temp = TempDir::new().unwrap();
    let unrelated = br#"{"unrelated":true}"#;
    fs::write(temp.path().join("auth_session.json"), unrelated).unwrap();
    let mut state = account_state(&temp);

    state.begin_settings_session();

    assert_eq!(
        state.session_error,
        Some(SessionError::ExistingSessionInvalid)
    );
    assert!(state.begin_login().is_none());
    assert!(!state.loading);
    assert!(state.token.is_none());
    assert!(state.soundcloud_token().is_none());
    assert!(
        state
            .murglar_media_credentials_at(1_700_000_000_000)
            .is_none()
    );
    assert_eq!(
        fs::read(temp.path().join("auth_session.json")).unwrap(),
        unrelated
    );
    assert!(!temp.path().join("auth_session.dat").exists());
    assert!(!temp.path().join("auth_session.backup.dat").exists());
}

#[test]
fn persistence_failure_prevents_profile_request_and_exposes_no_profile() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    store.persist_murglar("previous-token".into()).unwrap();
    let mut state = AccountState::new(identity(), Ok(store));
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    make_store_unwritable(&temp);

    let profile_request_token = state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("new-secret-token".into())),
    );
    assert!(profile_request_token.is_none());
    assert!(state.token.is_none());
    assert!(state.profile.is_none());
    assert_eq!(state.session_error, Some(SessionError::Filesystem));
    assert_eq!(
        state.session_store.as_ref().unwrap().session().murglar(),
        "previous-token"
    );
    assert!(
        !state
            .session_error
            .unwrap()
            .to_string()
            .contains("new-secret-token")
    );
}

#[test]
fn profile_failure_retains_durably_persisted_token() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();

    let profile_request_token = state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("saved-token".into())),
    );
    assert_eq!(profile_request_token.as_deref(), Some("saved-token"));
    assert!(state.complete_profile(generation, Err(AccountError::NetworkUnavailable)));

    assert_eq!(state.token.as_deref(), Some("saved-token"));
    assert!(state.profile.is_none());
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        "saved-token"
    );
    let status = state.status.as_deref().unwrap();
    assert!(!status.contains("saved-token"));
}

#[test]
fn unauthorized_and_forbidden_profile_failures_clear_only_murglar_token() {
    for error in [AccountError::Unauthorized, AccountError::Forbidden] {
        let temp = TempDir::new().unwrap();
        let mut state = account_state_with_saved_accounts(&temp);
        state.begin_settings_session();
        let (generation, _, _) = state.refresh().unwrap();
        state.profile = Some(profile());
        let credential_scope = state.credential_generation();

        assert!(state.complete_profile(generation, Err(error.clone())));

        assert!(state.token.is_none());
        assert_ne!(state.credential_generation(), credential_scope);
        assert!(state.profile.is_none());
        let persisted = SessionStore::load(temp.path()).unwrap();
        assert_eq!(persisted.session().murglar(), "");
        assert_eq!(persisted.session().soundcloud(), "soundcloud-token");
        assert_eq!(persisted.session().soundcloud_mobile(), "mobile-token");
        assert_eq!(persisted.session().deezer(), "deezer-token");
        assert_eq!(persisted.session().deezer_user_id(), "42");
    }
}

#[test]
fn failed_auth_invalidation_suppresses_persisted_media_entitlement() {
    let now = 1_700_000_000_000;

    for error in [AccountError::Unauthorized, AccountError::Forbidden] {
        let temp = TempDir::new().unwrap();
        let mut state = account_state_with_saved_pass(&temp, now + 1);
        state.begin_settings_session();
        let (generation, _, _) = state.refresh().unwrap();
        let credential_scope = state.credential_generation();
        make_store_unwritable(&temp);

        assert!(state.complete_profile(generation, Err(error)));

        assert_eq!(state.token.as_deref(), Some("saved-token"));
        assert!(state.profile.is_none());
        assert_eq!(state.session_error, Some(SessionError::Filesystem));
        assert!(state.murglar_media_credentials_at(now).is_none());
        assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
        assert_ne!(state.credential_generation(), credential_scope);
    }
}

#[test]
fn failed_logout_keeps_token_state_but_clears_profile_for_privacy() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state(&temp);
    state.begin_settings_session();
    let (generation, _, logout_epoch) = state.begin_login().unwrap();
    state.complete_token_exchange(
        generation,
        logout_epoch,
        Ok(AccessToken::new("token".into())),
    );
    state.complete_profile(generation, Ok(profile()));
    let now = 1_700_000_000_000;
    assert_media_credentials_at(&state, now);
    make_store_unwritable(&temp);

    state.logout();

    assert_eq!(state.token.as_deref(), Some("token"));
    assert!(state.profile.is_none());
    assert_eq!(state.session_error, Some(SessionError::Filesystem));
    assert_eq!(
        state.session_store.as_ref().unwrap().session().murglar(),
        "token"
    );
    assert!(state.murglar_media_credentials_at(now).is_none());
    assert_eq!(state.sidebar_pass_at(now), SidebarPass::Inactive);
}

#[test]
fn startup_loads_token_without_inventing_a_profile() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    store.session_mut().set_murglar("saved-token".into());
    store.persist().unwrap();

    let state = AccountState::new(identity(), SessionStore::load(temp.path()));
    assert_eq!(state.token.as_deref(), Some("saved-token"));
    assert!(state.profile.is_none());
}

#[test]
fn profile_refresh_requires_an_active_settings_session() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);

    assert!(state.refresh().is_none());
    assert!(!state.settings_active);
    assert!(state.profile.is_none());
}

#[test]
fn profile_refresh_exposes_loading_state_and_prevents_duplicate_loads() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();
    assert!(state.refresh().is_none());
    assert!(state.loading);
    assert!(state.complete_profile(generation, Ok(profile())));
    assert!(state.profile.is_some());

    let (refresh_generation, _, token) = state.refresh().unwrap();
    assert_eq!(token, "saved-token");
    assert!(state.loading);
    assert!(state.complete_profile(refresh_generation, Ok(profile())));
    assert!(!state.loading);
}

#[test]
fn profile_device_limit_keeps_saved_session_and_shows_warning_state() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();

    assert!(state.complete_profile(generation, Err(AccountError::DeviceLimitExceeded)));
    assert_eq!(state.token.as_deref(), Some("saved-token"));
    assert!(state.profile.is_none());
    assert!(state.device_limit_exceeded);
    assert!(state.status.is_none());
}

#[test]
fn saved_deezer_user_id_is_exposed_without_broad_session_access() {
    let temp = TempDir::new().unwrap();
    let state = account_state_with_saved_accounts(&temp);
    assert_eq!(state.deezer_user_id().as_deref(), Some("42"));
}

#[test]
fn saved_provider_profiles_are_loaded_without_exposing_credentials() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join("auth_session.json"),
        serde_json::to_vec(&json!({
            "soundcloud": "soundcloud-token",
            "soundcloudMobile": "mobile-token",
            "soundcloudProfile": {
                "username": "cloud-listener",
                "avatarUrl": "https://i1.sndcdn.com/avatar.jpg"
            },
            "deezer": "deezer-token",
            "deezerUserId": "42",
            "deezerProfile": {
                "username": "deezer-listener",
                "avatarUrl": "https://e-cdns-images.dzcdn.net/user.jpg"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let state = account_state(&temp);
    assert!(state.service_signed_in(Service::SoundCloud));
    assert!(state.service_signed_in(Service::Deezer));
    assert_eq!(
        state.service_username(Service::SoundCloud),
        Some("cloud-listener")
    );
    assert_eq!(
        state.service_avatar_url(Service::Deezer),
        Some("https://e-cdns-images.dzcdn.net/user.jpg")
    );
    assert!(
        !state
            .service_username(Service::Deezer)
            .unwrap()
            .chars()
            .all(|character| character.is_ascii_digit())
    );
}

#[test]
fn deezer_identity_never_exposes_a_numeric_user_id_as_the_username() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.deezer_profile = Some(ServiceIdentity {
        username: "123456".into(),
        avatar_url: "https://e-cdns-images.dzcdn.net/user.jpg".into(),
    });

    assert!(state.service_identity(Service::Deezer).is_none());
    assert!(state.service_username(Service::Deezer).is_none());
}

#[test]
fn stale_deezer_placeholder_does_not_block_profile_refresh() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.deezer_profile = Some(ServiceIdentity {
        username: "Deezer account".into(),
        avatar_url: String::new(),
    });

    let (generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    assert!(state.deezer_identity_loading());
    state.complete_deezer_profile(
        generation,
        Ok(ServiceIdentity::new("actual-listener", None).unwrap()),
    );

    assert_eq!(
        state.service_username(Service::Deezer),
        Some("actual-listener")
    );
}

#[test]
fn stale_provider_identity_completion_is_rejected_after_logout() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    let (generation, _) = state.begin_soundcloud_identity_with_generation().unwrap();
    state.logout_service(Service::SoundCloud);
    state.complete_soundcloud_profile(
        generation,
        Ok(ServiceIdentity::new("late-user", None).unwrap()),
    );

    assert!(state.service_username(Service::SoundCloud).is_none());
    assert!(!state.service_signed_in(Service::SoundCloud));
}

#[test]
fn stale_deezer_identity_completion_is_rejected_after_logout() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    let (generation, _) = state.begin_deezer_identity_with_generation().unwrap();
    state.logout_service(Service::Deezer);
    state.complete_deezer_profile(
        generation,
        Ok(ServiceIdentity::new("late-user", None).unwrap()),
    );

    assert!(state.service_username(Service::Deezer).is_none());
    assert!(!state.service_signed_in(Service::Deezer));
}

#[test]
fn ending_settings_session_invalidates_profile_work_without_clearing_sidebar_or_session() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.profile = Some(profile());
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();

    state.end_settings_session();

    assert!(!state.complete_profile(generation, Err(AccountError::Unauthorized)));
    assert_eq!(state.sidebar_username(), "listener");
    assert_eq!(state.token.as_deref(), Some("saved-token"));
    assert_eq!(
        SessionStore::load(temp.path()).unwrap().session().murglar(),
        "saved-token"
    );
}

#[test]
fn profile_extras_completion_requires_current_settings_lifecycle() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (generation, _, _) = state.refresh().unwrap();
    let extras = AccountExtras {
        referral: Ok(ReferralStats {
            referral_code: "code".into(),
            reward_days: 1,
            invitee_count: 0,
            reward_count: 0,
        }),
        plans: Err(AccountError::NetworkUnavailable),
    };

    assert!(state.complete_extras(generation, extras));
    assert_eq!(state.referral.as_ref().unwrap().referral_code, "code");
    assert!(state.plans.is_none());
    assert_eq!(
        state.plans_status.as_deref(),
        Some("Murglar plans are temporarily unavailable.")
    );
}

fn sample_plans() -> PaymentPlans {
    PaymentPlans {
        premium: None,
        subscriptions: Vec::new(),
        premium_short_description: String::new(),
        cheapest_subscription_per_month_rub: None,
        cheapest_subscription_per_month_usd: None,
    }
}

fn sample_referral(code: &str) -> ReferralStats {
    ReferralStats {
        referral_code: code.into(),
        reward_days: 1,
        invitee_count: 2,
        reward_count: 3,
    }
}

#[test]
fn referral_reload_replaces_referral_without_clearing_plans() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let plans = Arc::new(sample_plans());
    state.referral = Some(Arc::new(sample_referral("OLD")));
    state.plans = Some(plans.clone());

    let (generation, _, token) = state.begin_referral_refresh().unwrap();
    assert_eq!(token, "saved-token");
    assert!(state.referral_reloading);
    assert!(!state.extras_loading);
    assert!(!state.loading);
    assert!(Arc::ptr_eq(state.plans.as_ref().unwrap(), &plans));
    assert_eq!(state.referral.as_ref().unwrap().referral_code, "OLD");
    assert!(state.begin_referral_refresh().is_none());

    assert!(state.complete_referral(
        generation,
        Ok(ReferralStats {
            referral_code: "NEW".into(),
            reward_days: 4,
            invitee_count: 5,
            reward_count: 6,
        }),
    ));
    assert!(!state.referral_reloading);
    assert_eq!(state.referral.as_ref().unwrap().referral_code, "NEW");
    assert_eq!(state.referral.as_ref().unwrap().reward_days, 4);
    assert!(Arc::ptr_eq(state.plans.as_ref().unwrap(), &plans));
    assert!(state.referral_status.is_none());
}

#[test]
fn failed_referral_reload_keeps_existing_referral_and_plans() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let plans = Arc::new(sample_plans());
    state.referral = Some(Arc::new(sample_referral("OLD")));
    state.plans = Some(plans.clone());

    let (generation, _, _) = state.begin_referral_refresh().unwrap();
    assert!(state.complete_referral(generation, Err(AccountError::NetworkUnavailable)));
    assert!(!state.referral_reloading);
    assert_eq!(state.referral.as_ref().unwrap().referral_code, "OLD");
    assert!(Arc::ptr_eq(state.plans.as_ref().unwrap(), &plans));
    assert_eq!(
        state.referral_status.as_deref(),
        Some("Referral details are temporarily unavailable.")
    );
}

#[test]
fn logout_all_atomically_clears_every_service_and_invalidates_pending_work() {
    let temp = TempDir::new().unwrap();
    let mut state = account_state_with_saved_accounts(&temp);
    state.begin_settings_session();
    let (profile_generation, _, _) = state.refresh().unwrap();
    let service_generation = state.begin_service_auth(Service::SoundCloud).unwrap();

    state.logout_all();

    let session = SessionStore::load(temp.path()).unwrap();
    assert_eq!(session.session().murglar(), "");
    assert_eq!(session.session().soundcloud(), "");
    assert_eq!(session.session().deezer(), "");
    assert!(!state.complete_profile(profile_generation, Ok(profile())));
    state.complete_service_auth(
        service_generation,
        Err("late service result".into()),
        Service::SoundCloud,
    );
    assert_eq!(
        state.service_status_for(Service::Deezer).as_deref(),
        Some("All accounts are signed out.")
    );
    assert_eq!(
        state.service_status_for(Service::SoundCloud).as_deref(),
        Some("All accounts are signed out.")
    );
}
