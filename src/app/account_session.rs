use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const PRIMARY_FILE: &str = "auth_session.dat";
const BACKUP_FILE: &str = "auth_session.backup.dat";
const LEGACY_PRIMARY_FILE: &str = "auth_session.json";
const LEGACY_BACKUP_FILE: &str = "auth_session.backup.json";
const LEGACY_DEEZER_SID_FILE: &str = "deezer_sid.txt";
const MAX_PROTECTED_FILE_LEN: u64 = 16 + 256 * 1024;
const MAX_LEGACY_FILE_LEN: u64 = 64 * 1024;

#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AuthSession {
    murglar: String,
    soundcloud: String,
    soundcloud_mobile: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    soundcloud_cookies: String,
    deezer: String,
    deezer_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deezer_cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    soundcloud_profile: Option<ServiceIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deezer_profile: Option<ServiceIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    murglar_profile_summary: Option<MurglarProfileSummary>,
}

/// The non-secret provider identity shown in settings next to a saved session.
/// Tokens and ARL cookies intentionally remain separate from this cache.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ServiceIdentity {
    pub(crate) username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) avatar_url: String,
}

impl ServiceIdentity {
    pub(crate) fn new(username: impl Into<String>, avatar_url: Option<String>) -> Option<Self> {
        let username = username.into().trim().to_owned();
        if username.is_empty() {
            return None;
        }
        Some(Self {
            username,
            avatar_url: avatar_url.unwrap_or_default(),
        })
    }

    /// Persisted profiles are not trusted just because they came from the
    /// local session file. Only plain HTTPS image URLs are safe to hand to the
    /// image loader, and URLs with credentials or query data are rejected.
    pub(crate) fn safe_avatar_url(&self) -> Option<&str> {
        let value = self.avatar_url.trim();
        let parsed = url::Url::parse(value).ok()?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return None;
        }
        Some(value)
    }
}

/// Cached copy of the Murglar account summary shown in the sidebar before the
/// next profile refresh completes, mirroring the original's localStorage entry.
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct MurglarProfileSummary {
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) premium: String,
    pub(crate) pass_expiration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pass_expiration_millis: Option<i64>,
}

impl MurglarProfileSummary {
    pub(crate) fn display_name(&self) -> &str {
        if !self.username.trim().is_empty() {
            &self.username
        } else if !self.email.trim().is_empty() {
            &self.email
        } else {
            "Murglar user"
        }
    }
}

impl AuthSession {
    pub(crate) fn murglar(&self) -> &str {
        &self.murglar
    }

    pub(crate) fn deezer(&self) -> &str {
        &self.deezer
    }

    pub(crate) fn deezer_user_id(&self) -> &str {
        &self.deezer_user_id
    }

    pub(crate) fn deezer_cookies(&self) -> Option<&str> {
        self.deezer_cookies
            .as_deref()
            .map(str::trim)
            .filter(|cookies| !cookies.is_empty())
    }

    pub(crate) fn soundcloud(&self) -> &str {
        &self.soundcloud
    }

    pub(crate) fn soundcloud_mobile(&self) -> &str {
        &self.soundcloud_mobile
    }

    pub(crate) fn soundcloud_cookies(&self) -> &str {
        &self.soundcloud_cookies
    }

    pub(crate) fn soundcloud_profile(&self) -> Option<&ServiceIdentity> {
        self.soundcloud_profile.as_ref()
    }

    pub(crate) fn deezer_profile(&self) -> Option<&ServiceIdentity> {
        self.deezer_profile.as_ref()
    }

    pub(crate) fn murglar_profile_summary(&self) -> Option<&MurglarProfileSummary> {
        self.murglar_profile_summary.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_murglar(&mut self, token: String) {
        self.murglar = token;
    }
}

#[derive(Clone)]
pub(crate) struct SessionStore {
    directory: PathBuf,
    legacy_directory: Option<PathBuf>,
    session: AuthSession,
}

impl SessionStore {
    pub(crate) fn load_current_user() -> Result<Self, SessionError> {
        let (directory, legacy_directory) =
            super::paths::session_directories().ok_or(SessionError::ConfigDirectoryUnavailable)?;
        Self::load_in_directories(&directory, legacy_directory.as_deref())
    }

    /// Custom stores are isolated: only the explicitly supplied directory is touched.
    #[cfg(test)]
    pub(crate) fn load(directory: &Path) -> Result<Self, SessionError> {
        Self::load_in_directories(directory, None)
    }

    pub(super) fn load_in_directories(
        directory: &Path,
        legacy_directory: Option<&Path>,
    ) -> Result<Self, SessionError> {
        fs::create_dir_all(directory).map_err(|_| SessionError::Filesystem)?;
        let primary_path = directory.join(PRIMARY_FILE);
        let backup_path = directory.join(BACKUP_FILE);
        let mut session = if path_exists(&primary_path)? || path_exists(&backup_path)? {
            load_protected_pair(&primary_path, &backup_path)?
        } else {
            match load_legacy_directories(directory, legacy_directory)? {
                Some(session) => {
                    write_session_pair(&primary_path, &backup_path, &session)?;
                    session
                }
                None => {
                    return Ok(Self {
                        directory: directory.to_owned(),
                        legacy_directory: legacy_directory.map(Path::to_owned),
                        session: AuthSession::default(),
                    });
                }
            }
        };

        // The pre-refactor app kept the Deezer sid cookie in its own file.
        // Consume it before deletion so a migrated Deezer session keeps its
        // sid, but only when a Deezer ARL already exists: a stray sid file
        // must never fabricate a signed-in Deezer account.
        if session.deezer_cookies.is_none() && !session.deezer.trim().is_empty() {
            if let Some(sid) = read_legacy_deezer_sid(directory, legacy_directory)? {
                session.deezer_cookies = Some(sid);
                write_session_pair(&primary_path, &backup_path, &session)?;
            }
        }

        remove_legacy_files(directory, legacy_directory)?;

        Ok(Self {
            directory: directory.to_owned(),
            legacy_directory: legacy_directory.map(Path::to_owned),
            session,
        })
    }

    pub(crate) fn session(&self) -> &AuthSession {
        &self.session
    }

    #[cfg(test)]
    pub(crate) fn session_mut(&mut self) -> &mut AuthSession {
        &mut self.session
    }

    pub(crate) fn persist(&self) -> Result<(), SessionError> {
        fs::create_dir_all(&self.directory).map_err(|_| SessionError::Filesystem)?;
        write_session_pair(
            &self.directory.join(PRIMARY_FILE),
            &self.directory.join(BACKUP_FILE),
            &self.session,
        )?;
        remove_legacy_files(&self.directory, self.legacy_directory.as_deref())
    }

    pub(crate) fn preflight_persist(&self) -> Result<(), SessionError> {
        self.persist()
    }

    pub(crate) fn persist_murglar(&mut self, token: String) -> Result<(), SessionError> {
        let previous = std::mem::replace(&mut self.session.murglar, token);
        if let Err(error) = self.persist() {
            self.session.murglar = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn clear_murglar(&mut self) -> Result<(), SessionError> {
        self.persist_murglar(String::new())
    }

    pub(crate) fn persist_profile_summary(
        &mut self,
        summary: Option<MurglarProfileSummary>,
    ) -> Result<(), SessionError> {
        let previous = self.session.murglar_profile_summary.clone();
        self.session.murglar_profile_summary = summary;
        if let Err(error) = self.persist() {
            self.session.murglar_profile_summary = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_services(
        &mut self,
        murglar: Option<String>,
        deezer: Option<(String, String)>,
        soundcloud: Option<(String, Option<String>)>,
    ) -> Result<(), SessionError> {
        let previous = self.session.clone();
        if let Some(token) = murglar {
            self.session.murglar = token;
        }
        if let Some((arl, user_id)) = deezer {
            self.session.deezer = arl;
            self.session.deezer_user_id = user_id;
            if self.session.deezer.trim().is_empty() {
                self.session.deezer_profile = None;
                self.session.deezer_cookies = None;
            }
        }
        if let Some((desktop, mobile)) = soundcloud {
            self.session.soundcloud = desktop;
            self.session.soundcloud_mobile = mobile.unwrap_or_default();
            self.session.soundcloud_cookies.clear();
            if self.session.soundcloud.trim().is_empty() {
                self.session.soundcloud_profile = None;
            }
        }
        if let Err(error) = self.persist() {
            self.session = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_service_login(
        &mut self,
        service: crate::service_auth::Service,
        desktop: String,
        mobile: Option<String>,
        soundcloud_cookies: Option<String>,
        deezer_cookies: Option<String>,
        deezer_user_id: Option<String>,
        profile: Option<ServiceIdentity>,
    ) -> Result<(), SessionError> {
        let previous = self.session.clone();
        match service {
            crate::service_auth::Service::Deezer => {
                self.session.deezer = desktop;
                self.session.deezer_user_id = deezer_user_id.unwrap_or_default();
                self.session.deezer_cookies = deezer_cookies;
                self.session.deezer_profile = profile;
            }
            crate::service_auth::Service::SoundCloud => {
                self.session.soundcloud = desktop;
                self.session.soundcloud_mobile = mobile.unwrap_or_default();
                self.session.soundcloud_cookies = soundcloud_cookies.unwrap_or_default();
                self.session.soundcloud_profile = profile;
            }
        }
        if let Err(error) = self.persist() {
            self.session = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_service_profile(
        &mut self,
        service: crate::service_auth::Service,
        profile: Option<ServiceIdentity>,
    ) -> Result<(), SessionError> {
        let previous = self.session.clone();
        match service {
            crate::service_auth::Service::Deezer => self.session.deezer_profile = profile,
            crate::service_auth::Service::SoundCloud => self.session.soundcloud_profile = profile,
        }
        if let Err(error) = self.persist() {
            self.session = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn persist_deezer_cookies(
        &mut self,
        cookies: Option<String>,
    ) -> Result<(), SessionError> {
        let previous = self.session.deezer_cookies.clone();
        self.session.deezer_cookies = cookies;
        if let Err(error) = self.persist() {
            self.session.deezer_cookies = previous;
            let _ = self.persist();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn clear_service(
        &mut self,
        service: crate::service_auth::Service,
    ) -> Result<(), SessionError> {
        match service {
            crate::service_auth::Service::Deezer => {
                self.persist_services(None, Some((String::new(), String::new())), None)
            }
            crate::service_auth::Service::SoundCloud => {
                self.persist_services(None, None, Some((String::new(), Some(String::new()))))
            }
        }
    }

    pub(crate) fn clear_all_services(&mut self) -> Result<(), SessionError> {
        self.persist_services(
            Some(String::new()),
            Some((String::new(), String::new())),
            Some((String::new(), Some(String::new()))),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionError {
    ConfigDirectoryUnavailable,
    Filesystem,
    SecureStorage,
    ExistingSessionInvalid,
    Serialization,
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConfigDirectoryUnavailable => "Account session storage is unavailable.",
            Self::Filesystem => "The account session could not be stored.",
            Self::SecureStorage => "Secure account storage is unavailable.",
            Self::ExistingSessionInvalid => "The saved account session is invalid.",
            Self::Serialization => "The account session could not be prepared.",
        })
    }
}

impl std::error::Error for SessionError {}

enum StoredSession {
    Missing,
    Valid(Box<AuthSession>),
    Invalid,
}

fn read_session(path: &Path) -> Result<StoredSession, SessionError> {
    let Some(bytes) = read_bounded(path, MAX_PROTECTED_FILE_LEN)? else {
        return Ok(StoredSession::Missing);
    };
    let mut plaintext = match super::session_protection::unprotect(&bytes) {
        Ok(plaintext) => plaintext,
        Err(_) => return Ok(StoredSession::Invalid),
    };
    let parsed = serde_json::from_slice(&plaintext).ok();
    super::session_protection::wipe(&mut plaintext);
    Ok(match parsed {
        Some(session) => StoredSession::Valid(Box::new(session)),
        None => StoredSession::Invalid,
    })
}

fn read_legacy_session(path: &Path) -> Result<StoredSession, SessionError> {
    let Some(mut bytes) = read_bounded(path, MAX_LEGACY_FILE_LEN)? else {
        return Ok(StoredSession::Missing);
    };
    let parsed = parse_legacy_session(&bytes);
    super::session_protection::wipe(&mut bytes);
    Ok(match parsed {
        Some(session) => StoredSession::Valid(Box::new(session)),
        None => StoredSession::Invalid,
    })
}

fn load_legacy_directories(
    directory: &Path,
    legacy_directory: Option<&Path>,
) -> Result<Option<AuthSession>, SessionError> {
    let mut invalid = false;
    for directory in std::iter::once(directory).chain(legacy_directory) {
        match load_legacy_pair(
            &directory.join(LEGACY_PRIMARY_FILE),
            &directory.join(LEGACY_BACKUP_FILE),
        ) {
            Ok(Some(session)) => return Ok(Some(session)),
            Ok(None) => {}
            Err(SessionError::ExistingSessionInvalid) => invalid = true,
            Err(error) => return Err(error),
        }
    }
    if invalid {
        Err(SessionError::ExistingSessionInvalid)
    } else {
        Ok(None)
    }
}

fn parse_legacy_session(bytes: &[u8]) -> Option<AuthSession> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let fields = value.as_object()?;
    // Empty credentials are valid only when at least one session field is
    // present. Optional fields may be absent in older session schemas.
    if ![
        "murglar",
        "soundcloud",
        "soundcloudMobile",
        "soundcloudCookies",
        "deezer",
        "deezerUserId",
        "soundcloudProfile",
        "deezerProfile",
        "murglarProfileSummary",
    ]
    .iter()
    .any(|field| fields.contains_key(*field))
    {
        return None;
    }
    serde_json::from_value(value).ok()
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Option<Vec<u8>>, SessionError> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > maximum => Ok(Some(Vec::new())),
        Ok(_) => match fs::read(path) {
            Ok(bytes) if bytes.len() as u64 <= maximum => Ok(Some(bytes)),
            Ok(_) => Ok(Some(Vec::new())),
            Err(_) => Err(SessionError::Filesystem),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(SessionError::Filesystem),
    }
}

fn path_exists(path: &Path) -> Result<bool, SessionError> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(SessionError::Filesystem),
    }
}

fn load_protected_pair(
    primary_path: &Path,
    backup_path: &Path,
) -> Result<AuthSession, SessionError> {
    let primary = read_session(primary_path)?;
    let backup = read_session(backup_path)?;

    match (primary, backup) {
        (StoredSession::Valid(primary), StoredSession::Valid(backup)) => {
            if primary != backup {
                write_protected_session(backup_path, &primary)?;
            }
            Ok(*primary)
        }
        (StoredSession::Valid(session), StoredSession::Missing | StoredSession::Invalid) => {
            write_protected_session(backup_path, &session)?;
            Ok(*session)
        }
        (StoredSession::Missing | StoredSession::Invalid, StoredSession::Valid(session)) => {
            write_protected_session(primary_path, &session)?;
            Ok(*session)
        }
        _ => Err(SessionError::ExistingSessionInvalid),
    }
}

fn load_legacy_pair(
    primary_path: &Path,
    backup_path: &Path,
) -> Result<Option<AuthSession>, SessionError> {
    let primary = read_legacy_session(primary_path)?;
    let backup = read_legacy_session(backup_path)?;

    match (primary, backup) {
        (StoredSession::Valid(primary), StoredSession::Valid(_)) => Ok(Some(*primary)),
        (StoredSession::Valid(session), StoredSession::Missing | StoredSession::Invalid)
        | (StoredSession::Missing | StoredSession::Invalid, StoredSession::Valid(session)) => {
            Ok(Some(*session))
        }
        (StoredSession::Missing, StoredSession::Missing) => Ok(None),
        _ => Err(SessionError::ExistingSessionInvalid),
    }
}

fn encode_plaintext(session: &AuthSession) -> Result<Vec<u8>, SessionError> {
    serde_json::to_vec_pretty(session).map_err(|_| SessionError::Serialization)
}

fn encode_protected(session: &AuthSession) -> Result<Vec<u8>, SessionError> {
    let mut plaintext = encode_plaintext(session)?;
    let result =
        super::session_protection::protect(&plaintext).map_err(|_| SessionError::SecureStorage);
    super::session_protection::wipe(&mut plaintext);
    result
}

fn write_protected_session(path: &Path, session: &AuthSession) -> Result<(), SessionError> {
    let protected = encode_protected(session)?;
    atomic_write(path, &protected)
}

fn write_session_pair(
    primary_path: &Path,
    backup_path: &Path,
    session: &AuthSession,
) -> Result<(), SessionError> {
    let protected = encode_protected(session)?;
    atomic_write(primary_path, &protected)?;
    atomic_write(backup_path, &protected)
}

fn read_legacy_deezer_sid(
    directory: &Path,
    legacy_directory: Option<&Path>,
) -> Result<Option<String>, SessionError> {
    for directory in std::iter::once(directory).chain(legacy_directory) {
        let path = directory.join(LEGACY_DEEZER_SID_FILE);
        let Some(bytes) = read_bounded(&path, MAX_LEGACY_FILE_LEN)? else {
            continue;
        };
        let content = String::from_utf8_lossy(&bytes).trim().to_owned();
        if content.is_empty() {
            continue;
        }
        return Ok(Some(if content.contains('=') {
            content
        } else {
            format!("sid={content}")
        }));
    }
    Ok(None)
}

fn remove_legacy_files(
    directory: &Path,
    legacy_directory: Option<&Path>,
) -> Result<(), SessionError> {
    for directory in std::iter::once(directory).chain(legacy_directory) {
        for file_name in [
            LEGACY_PRIMARY_FILE,
            LEGACY_BACKUP_FILE,
            LEGACY_DEEZER_SID_FILE,
        ] {
            match fs::remove_file(directory.join(file_name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(SessionError::Filesystem),
            }
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), SessionError> {
    let parent = path.parent().ok_or(SessionError::Filesystem)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SessionError::Filesystem)?;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
        match write_new_synced(&temporary, contents) {
            Ok(()) => {
                let result = atomic_rename(&temporary, path);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result.map_err(|_| SessionError::Filesystem);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                let _ = fs::remove_file(&temporary);
                return Err(SessionError::Filesystem);
            }
        }
    }
    Err(SessionError::Filesystem)
}

fn write_new_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(windows)]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(test)]
mod tests;
