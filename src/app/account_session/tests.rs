use super::*;
use serde_json::json;
use tempfile::TempDir;

fn session() -> AuthSession {
    AuthSession {
        murglar: "murglar-token".into(),
        soundcloud: "soundcloud-token".into(),
        soundcloud_mobile: "mobile-token".into(),
        soundcloud_cookies: "oauth_token=soundcloud-token".into(),
        deezer: "deezer-token".into(),
        deezer_user_id: "42".into(),
        soundcloud_profile: None,
        deezer_profile: None,
        murglar_profile_summary: None,
    }
}

fn stored_value(path: &Path) -> serde_json::Value {
    let bytes = fs::read(path).unwrap();
    let mut plaintext = super::super::session_protection::unprotect(&bytes).unwrap();
    let value = serde_json::from_slice(&plaintext).unwrap();
    super::super::session_protection::wipe(&mut plaintext);
    value
}

fn protected_session(session: &AuthSession) -> Vec<u8> {
    let mut plaintext = encode_plaintext(session).unwrap();
    let protected = super::super::session_protection::protect(&plaintext).unwrap();
    super::super::session_protection::wipe(&mut plaintext);
    protected
}

#[test]
fn session_round_trips_with_exact_original_field_names() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    *store.session_mut() = session();
    store.persist().unwrap();

    let encrypted = fs::read(temp.path().join(PRIMARY_FILE)).unwrap();
    assert!(
        !encrypted
            .windows(b"murglar-token".len())
            .any(|window| window == b"murglar-token")
    );
    assert!(serde_json::from_slice::<serde_json::Value>(&encrypted).is_err());
    let value = stored_value(&temp.path().join(PRIMARY_FILE));
    assert_eq!(
        value,
        json!({
            "murglar": "murglar-token",
            "soundcloud": "soundcloud-token",
            "soundcloudMobile": "mobile-token",
            "soundcloudCookies": "oauth_token=soundcloud-token",
            "deezer": "deezer-token",
            "deezerUserId": "42"
        })
    );
    assert!(SessionStore::load(temp.path()).unwrap().session() == &session());
}

#[test]
fn profile_summary_round_trips_and_clears() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();

    store
        .persist_profile_summary(Some(MurglarProfileSummary {
            username: "kekkofan".into(),
            email: "k@420blaze.it".into(),
            premium: "Yes".into(),
            pass_expiration: "Aug 22, 2026".into(),
            pass_expiration_millis: Some(1_777_075_200_000),
        }))
        .unwrap();
    let loaded = SessionStore::load(temp.path()).unwrap();
    let summary = loaded
        .session()
        .murglar_profile_summary()
        .expect("summary missing");
    assert_eq!(summary.display_name(), "kekkofan");
    assert_eq!(summary.premium, "Yes");
    assert_eq!(summary.pass_expiration_millis, Some(1_777_075_200_000));

    let value = stored_value(&temp.path().join(PRIMARY_FILE));
    assert_eq!(
        value["murglarProfileSummary"]["username"],
        json!("kekkofan")
    );
    assert_eq!(
        value["murglarProfileSummary"]["passExpirationMillis"],
        json!(1_777_075_200_000_i64)
    );

    store.persist_profile_summary(None).unwrap();
    let cleared = SessionStore::load(temp.path()).unwrap();
    assert!(cleared.session().murglar_profile_summary().is_none());
}

#[test]
fn legacy_profile_summary_without_expiration_millis_still_loads() {
    let temp = TempDir::new().unwrap();
    fs::write(
        temp.path().join(LEGACY_PRIMARY_FILE),
        serde_json::to_vec(&json!({
            "murglar": "token",
            "murglarProfileSummary": {
                "username": "listener",
                "passExpiration": "Aug 22, 2026"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded = SessionStore::load(temp.path()).unwrap();
    let summary = loaded.session().murglar_profile_summary().unwrap();
    assert_eq!(summary.pass_expiration, "Aug 22, 2026");
    assert_eq!(summary.pass_expiration_millis, None);
    assert!(temp.path().join(PRIMARY_FILE).exists());
    assert!(temp.path().join(BACKUP_FILE).exists());
    assert!(!temp.path().join(LEGACY_PRIMARY_FILE).exists());
}

#[test]
fn legacy_pair_and_deezer_sid_are_removed_after_protected_migration() {
    let temp = TempDir::new().unwrap();
    let legacy = json!({
        "murglar": "legacy-murglar-token",
        "soundcloud": "legacy-soundcloud-token",
        "deezer": "legacy-deezer-token",
        "deezerUserId": "42"
    });
    fs::write(
        temp.path().join(LEGACY_PRIMARY_FILE),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    fs::write(
        temp.path().join(LEGACY_BACKUP_FILE),
        serde_json::to_vec(&legacy).unwrap(),
    )
    .unwrap();
    fs::write(temp.path().join(LEGACY_DEEZER_SID_FILE), b"legacy-sid").unwrap();

    let loaded = SessionStore::load(temp.path()).unwrap();
    assert_eq!(loaded.session().murglar(), "legacy-murglar-token");
    assert_eq!(loaded.session().deezer(), "legacy-deezer-token");
    assert!(temp.path().join(PRIMARY_FILE).exists());
    assert!(temp.path().join(BACKUP_FILE).exists());
    assert!(!temp.path().join(LEGACY_PRIMARY_FILE).exists());
    assert!(!temp.path().join(LEGACY_BACKUP_FILE).exists());
    assert!(!temp.path().join(LEGACY_DEEZER_SID_FILE).exists());

    for path in [PRIMARY_FILE, BACKUP_FILE] {
        let bytes = fs::read(temp.path().join(path)).unwrap();
        assert!(
            !bytes
                .windows(b"legacy-murglar-token".len())
                .any(|window| window == b"legacy-murglar-token")
        );
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
    }

    fs::write(
        temp.path().join(LEGACY_PRIMARY_FILE),
        serde_json::to_vec(&json!({"murglar": "should-not-win"})).unwrap(),
    )
    .unwrap();
    let reloaded = SessionStore::load(temp.path()).unwrap();
    assert_eq!(reloaded.session().murglar(), "legacy-murglar-token");
    assert!(!temp.path().join(LEGACY_PRIMARY_FILE).exists());
}

#[test]
fn provider_profiles_round_trip_without_secret_fields() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    store
        .persist_service_login(
            crate::service_auth::Service::SoundCloud,
            "desktop-secret".into(),
            Some("mobile-secret".into()),
            Some("oauth_token=desktop-secret; datadome=guard".into()),
            None,
            ServiceIdentity::new(
                "cloud-listener",
                Some("https://i1.sndcdn.com/avatar.jpg".into()),
            ),
        )
        .unwrap();

    let value = stored_value(&temp.path().join(PRIMARY_FILE));
    assert_eq!(value["soundcloudProfile"]["username"], "cloud-listener");
    assert_eq!(
        value["soundcloudProfile"]["avatarUrl"],
        "https://i1.sndcdn.com/avatar.jpg"
    );
    assert_eq!(value["soundcloud"], "desktop-secret");
    assert_eq!(value["soundcloudMobile"], "mobile-secret");
    assert_eq!(
        value["soundcloudCookies"],
        "oauth_token=desktop-secret; datadome=guard"
    );
    assert_eq!(
        SessionStore::load(temp.path())
            .unwrap()
            .session()
            .soundcloud_profile()
            .map(|profile| profile.username.as_str()),
        Some("cloud-listener")
    );

    let mut loaded = SessionStore::load(temp.path()).unwrap();
    loaded
        .clear_service(crate::service_auth::Service::SoundCloud)
        .unwrap();
    assert!(loaded.session().soundcloud_profile().is_none());
    assert!(loaded.session().soundcloud_cookies().is_empty());
}

#[test]
fn persisted_avatar_urls_are_limited_to_plain_https_urls() {
    let safe =
        ServiceIdentity::new("listener", Some("https://cdn.example/avatar.jpg".into())).unwrap();
    assert_eq!(
        safe.safe_avatar_url(),
        Some("https://cdn.example/avatar.jpg")
    );

    for avatar in [
        "http://cdn.example/avatar.jpg",
        "https://user:secret@cdn.example/avatar.jpg",
        "https://cdn.example/avatar.jpg?token=secret",
        "https://cdn.example/avatar.jpg#fragment",
    ] {
        let identity = ServiceIdentity {
            username: "listener".into(),
            avatar_url: avatar.into(),
        };
        assert_eq!(identity.safe_avatar_url(), None, "{avatar}");
    }
}

#[test]
fn changing_murglar_preserves_other_fields() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    *store.session_mut() = session();
    store.session_mut().set_murglar(String::new());
    store.persist().unwrap();

    let loaded = SessionStore::load(temp.path()).unwrap();
    assert_eq!(loaded.session().murglar(), "");
    assert_eq!(loaded.session().soundcloud, "soundcloud-token");
    assert_eq!(loaded.session().soundcloud_mobile, "mobile-token");
    assert_eq!(
        loaded.session().soundcloud_cookies(),
        "oauth_token=soundcloud-token"
    );
    assert_eq!(loaded.session().deezer, "deezer-token");
    assert_eq!(loaded.session().deezer_user_id, "42");
}

#[test]
fn service_updates_and_clear_operations_preserve_other_credentials() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    *store.session_mut() = session();

    store
        .persist_services(
            Some("new-murglar".into()),
            Some(("new-deezer".into(), "84".into())),
            Some(("new-desktop".into(), Some("new-mobile".into()))),
        )
        .unwrap();
    assert_eq!(store.session().murglar(), "new-murglar");
    assert_eq!(store.session().deezer(), "new-deezer");
    assert_eq!(store.session().deezer_user_id(), "84");
    assert_eq!(store.session().soundcloud(), "new-desktop");
    assert_eq!(store.session().soundcloud_mobile, "new-mobile");
    assert!(store.session().soundcloud_cookies().is_empty());

    store
        .clear_service(crate::service_auth::Service::Deezer)
        .unwrap();
    assert_eq!(store.session().deezer(), "");
    assert_eq!(store.session().deezer_user_id(), "");
    assert_eq!(store.session().soundcloud(), "new-desktop");

    store
        .clear_service(crate::service_auth::Service::SoundCloud)
        .unwrap();
    store.clear_murglar().unwrap();
    assert_eq!(store.session().soundcloud(), "");
    assert_eq!(store.session().soundcloud_mobile, "");
    assert!(store.session().soundcloud_cookies().is_empty());
    assert_eq!(store.session().murglar(), "");
}

#[test]
fn clearing_all_services_is_one_persisted_session_transition() {
    let temp = TempDir::new().unwrap();
    let mut store = SessionStore::load(temp.path()).unwrap();
    *store.session_mut() = session();
    store.persist().unwrap();

    store.clear_all_services().unwrap();

    let loaded = SessionStore::load(temp.path()).unwrap();
    assert!(loaded.session() == &AuthSession::default());
    assert_eq!(
        fs::read(temp.path().join(PRIMARY_FILE)).unwrap(),
        fs::read(temp.path().join(BACKUP_FILE)).unwrap()
    );
}

#[test]
fn valid_copy_repairs_an_invalid_peer() {
    for primary_is_valid in [true, false] {
        let temp = TempDir::new().unwrap();
        let valid_name = if primary_is_valid {
            PRIMARY_FILE
        } else {
            BACKUP_FILE
        };
        let peer_name = if primary_is_valid {
            BACKUP_FILE
        } else {
            PRIMARY_FILE
        };
        fs::write(temp.path().join(valid_name), protected_session(&session())).unwrap();
        fs::write(temp.path().join(peer_name), b"invalid").unwrap();

        assert!(SessionStore::load(temp.path()).unwrap().session() == &session());
        assert_eq!(
            stored_value(&temp.path().join(PRIMARY_FILE)),
            stored_value(&temp.path().join(BACKUP_FILE))
        );
    }
}

#[test]
fn missing_sessions_default_without_writes() {
    let temp = TempDir::new().unwrap();

    assert!(SessionStore::load(temp.path()).unwrap().session() == &AuthSession::default());
    assert!(!temp.path().join(PRIMARY_FILE).exists());
    assert!(!temp.path().join(BACKUP_FILE).exists());
}

#[test]
fn valid_mismatch_chooses_primary_and_repairs_backup() {
    let temp = TempDir::new().unwrap();
    let mut other = session();
    other.set_murglar("other".into());
    fs::write(
        temp.path().join(PRIMARY_FILE),
        protected_session(&session()),
    )
    .unwrap();
    fs::write(temp.path().join(BACKUP_FILE), protected_session(&other)).unwrap();

    assert!(SessionStore::load(temp.path()).unwrap().session() == &session());
    assert_eq!(
        stored_value(&temp.path().join(BACKUP_FILE)),
        stored_value(&temp.path().join(PRIMARY_FILE))
    );
}

#[test]
fn unrecoverable_sessions_fail_closed() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(PRIMARY_FILE), b"invalid").unwrap();
    assert!(matches!(
        SessionStore::load(temp.path()),
        Err(SessionError::ExistingSessionInvalid)
    ));

    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(PRIMARY_FILE), b"invalid").unwrap();
    fs::write(temp.path().join(BACKUP_FILE), b"also invalid").unwrap();
    assert!(matches!(
        SessionStore::load(temp.path()),
        Err(SessionError::ExistingSessionInvalid)
    ));

    let temp = TempDir::new().unwrap();
    let mut corrupted = protected_session(&session());
    let last = corrupted.len() - 1;
    corrupted[last] ^= 1;
    fs::write(temp.path().join(PRIMARY_FILE), corrupted).unwrap();
    assert!(matches!(
        SessionStore::load(temp.path()),
        Err(SessionError::ExistingSessionInvalid)
    ));
}

fn write_legacy_material(directory: &Path) -> Vec<u8> {
    fs::create_dir_all(directory).unwrap();
    let bytes = serde_json::to_vec(&json!({"murglar": "legacy-token"})).unwrap();
    fs::write(directory.join(LEGACY_PRIMARY_FILE), &bytes).unwrap();
    fs::write(directory.join(LEGACY_BACKUP_FILE), &bytes).unwrap();
    fs::write(directory.join(LEGACY_DEEZER_SID_FILE), b"legacy-sid").unwrap();
    bytes
}

fn assert_legacy_material(directory: &Path, expected: &[u8]) {
    assert_eq!(
        fs::read(directory.join(LEGACY_PRIMARY_FILE)).unwrap(),
        expected
    );
    assert_eq!(
        fs::read(directory.join(LEGACY_BACKUP_FILE)).unwrap(),
        expected
    );
    assert_eq!(
        fs::read(directory.join(LEGACY_DEEZER_SID_FILE)).unwrap(),
        b"legacy-sid"
    );
}

fn assert_legacy_removed(directory: &Path) {
    for file_name in [
        LEGACY_PRIMARY_FILE,
        LEGACY_BACKUP_FILE,
        LEGACY_DEEZER_SID_FILE,
    ] {
        assert!(
            !directory.join(file_name).exists(),
            "legacy material remains: {file_name}"
        );
    }
}

fn assert_encrypted_pair(directory: &Path, token: &str) {
    for file_name in [PRIMARY_FILE, BACKUP_FILE] {
        match read_session(&directory.join(file_name)).unwrap() {
            StoredSession::Valid(session) => assert_eq!(session.murglar(), token),
            _ => panic!("protected session must be independently recoverable"),
        }
    }
}

#[test]
fn unrelated_legacy_json_is_rejected_without_deleting_recovery_material() {
    for payload in [
        json!({"unrelated": true}),
        json!({}),
        json!([]),
        json!(null),
        json!({"murglar": 42}),
    ] {
        let temp = TempDir::new().unwrap();
        let active = temp.path().join("ralgruM");
        let legacy = temp.path().join("app.ralgrum.player");
        let bytes = serde_json::to_vec(&payload).unwrap();
        for directory in [&active, &legacy] {
            write_legacy_material(directory);
            fs::write(directory.join(LEGACY_PRIMARY_FILE), &bytes).unwrap();
            fs::write(directory.join(LEGACY_BACKUP_FILE), &bytes).unwrap();
        }

        assert!(matches!(
            SessionStore::load_in_directories(&active, Some(&legacy)),
            Err(SessionError::ExistingSessionInvalid)
        ));

        for directory in [&active, &legacy] {
            assert_legacy_material(directory, &bytes);
            assert!(!directory.join(PRIMARY_FILE).exists());
            assert!(!directory.join(BACKUP_FILE).exists());
        }
    }
}

#[test]
fn recognized_empty_legacy_credentials_remain_valid() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(LEGACY_PRIMARY_FILE), br#"{"murglar":""}"#).unwrap();

    let store = SessionStore::load(temp.path()).unwrap();

    assert!(store.session() == &AuthSession::default());
    assert_encrypted_pair(temp.path(), "");
    assert_legacy_removed(temp.path());
}

#[test]
fn valid_legacy_backup_recovers_an_unrelated_primary() {
    let temp = TempDir::new().unwrap();
    write_legacy_material(temp.path());
    fs::write(
        temp.path().join(LEGACY_PRIMARY_FILE),
        br#"{"unrelated":true}"#,
    )
    .unwrap();

    let store = SessionStore::load(temp.path()).unwrap();

    assert_eq!(store.session().murglar(), "legacy-token");
    assert_encrypted_pair(temp.path(), "legacy-token");
    assert_legacy_removed(temp.path());
}

#[test]
fn load_and_persist_clean_only_the_two_explicit_session_directories() {
    let temp = TempDir::new().unwrap();
    let active = temp.path().join("ralgruM");
    let legacy = temp.path().join("app.ralgrum.player");
    let unrelated = temp.path().join("unrelated");
    write_legacy_material(&active);
    write_legacy_material(&legacy);
    let untouched = write_legacy_material(&unrelated);
    fs::write(legacy.join("general_settings.json"), b"keep-settings").unwrap();

    let store = SessionStore::load_in_directories(&active, Some(&legacy)).unwrap();

    assert_eq!(store.session().murglar(), "legacy-token");
    assert_encrypted_pair(&active, "legacy-token");
    assert_legacy_removed(&active);
    assert_legacy_removed(&legacy);
    assert_legacy_material(&unrelated, &untouched);

    // Cleanup also applies when plaintext appears after the store was loaded.
    write_legacy_material(&active);
    write_legacy_material(&legacy);
    store.persist().unwrap();

    assert_encrypted_pair(&active, "legacy-token");
    assert_legacy_removed(&active);
    assert_legacy_removed(&legacy);
    assert_legacy_material(&unrelated, &untouched);
    assert_eq!(
        fs::read(legacy.join("general_settings.json")).unwrap(),
        b"keep-settings"
    );
}

#[test]
fn custom_load_does_not_discover_a_sibling_legacy_directory() {
    let temp = TempDir::new().unwrap();
    let active = temp.path().join("ralgruM");
    let sibling = temp.path().join("app.ralgrum.player");
    let untouched = write_legacy_material(&sibling);

    let store = SessionStore::load(&active).unwrap();
    assert!(store.session() == &AuthSession::default());
    store.persist().unwrap();

    assert_legacy_material(&sibling, &untouched);
    assert_encrypted_pair(&active, "");
}

#[test]
fn distinct_legacy_backup_recovers_when_active_migration_copy_is_invalid() {
    let temp = TempDir::new().unwrap();
    let active = temp.path().join("ralgruM");
    let legacy = temp.path().join("app.ralgrum.player");
    fs::create_dir(&active).unwrap();
    fs::create_dir(&legacy).unwrap();
    fs::write(active.join(LEGACY_PRIMARY_FILE), b"invalid").unwrap();
    fs::write(
        legacy.join(LEGACY_BACKUP_FILE),
        br#"{"soundcloud":"saved-cloud"}"#,
    )
    .unwrap();

    let store = SessionStore::load_in_directories(&active, Some(&legacy)).unwrap();

    assert_eq!(store.session().soundcloud(), "saved-cloud");
    assert_eq!(
        stored_value(&active.join(BACKUP_FILE))["soundcloud"],
        "saved-cloud"
    );
    assert_legacy_removed(&active);
    assert_legacy_removed(&legacy);
}

#[test]
fn failed_encrypted_write_retains_both_directory_plaintext_until_pair_is_repaired() {
    for blocked_file in [PRIMARY_FILE, BACKUP_FILE] {
        let temp = TempDir::new().unwrap();
        let active = temp.path().join("ralgruM");
        let legacy = temp.path().join("app.ralgrum.player");
        let mut store = SessionStore::load_in_directories(&active, Some(&legacy)).unwrap();
        store.session_mut().set_murglar("new-token".into());
        let expected = write_legacy_material(&active);
        write_legacy_material(&legacy);
        fs::create_dir(active.join(blocked_file)).unwrap();

        assert_eq!(store.persist(), Err(SessionError::Filesystem));

        assert_legacy_material(&active, &expected);
        assert_legacy_material(&legacy, &expected);
        if blocked_file == BACKUP_FILE {
            assert_eq!(
                stored_value(&active.join(PRIMARY_FILE))["murglar"],
                "new-token"
            );
        }

        fs::remove_dir(active.join(blocked_file)).unwrap();
        let recovered = SessionStore::load_in_directories(&active, Some(&legacy)).unwrap();
        let expected_token = if blocked_file == BACKUP_FILE {
            "new-token"
        } else {
            "legacy-token"
        };
        assert_eq!(recovered.session().murglar(), expected_token);
        assert_encrypted_pair(&active, expected_token);
        assert_legacy_removed(&active);
        assert_legacy_removed(&legacy);
    }
}

#[test]
fn obsolete_sid_is_retained_until_an_encrypted_pair_exists() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(LEGACY_DEEZER_SID_FILE), b"legacy-sid").unwrap();

    let store = SessionStore::load(temp.path()).unwrap();

    assert_eq!(
        fs::read(temp.path().join(LEGACY_DEEZER_SID_FILE)).unwrap(),
        b"legacy-sid"
    );
    assert!(!temp.path().join(PRIMARY_FILE).exists());
    store.persist().unwrap();
    assert_encrypted_pair(temp.path(), "");
    assert_legacy_removed(temp.path());
}

#[test]
fn protected_pair_repair_retains_plaintext_on_failure_then_cleans_both_directories() {
    let temp = TempDir::new().unwrap();
    let active = temp.path().join("ralgruM");
    let legacy = temp.path().join("app.ralgrum.player");
    let expected = write_legacy_material(&active);
    write_legacy_material(&legacy);
    fs::write(active.join(BACKUP_FILE), protected_session(&session())).unwrap();
    fs::create_dir(active.join(PRIMARY_FILE)).unwrap();

    assert!(matches!(
        SessionStore::load_in_directories(&active, Some(&legacy)),
        Err(SessionError::Filesystem)
    ));
    assert_legacy_material(&active, &expected);
    assert_legacy_material(&legacy, &expected);

    fs::remove_dir(active.join(PRIMARY_FILE)).unwrap();
    let recovered = SessionStore::load_in_directories(&active, Some(&legacy)).unwrap();

    assert!(recovered.session() == &session());
    assert_encrypted_pair(&active, "murglar-token");
    assert_legacy_removed(&active);
    assert_legacy_removed(&legacy);
}

#[test]
fn same_directory_alias_does_not_prevent_safe_plaintext_cleanup() {
    let temp = TempDir::new().unwrap();
    write_legacy_material(temp.path());
    let alias = temp.path().join(".");

    let store = SessionStore::load_in_directories(temp.path(), Some(&alias)).unwrap();
    store.persist().unwrap();

    assert_encrypted_pair(temp.path(), "legacy-token");
    assert_legacy_removed(temp.path());
}
