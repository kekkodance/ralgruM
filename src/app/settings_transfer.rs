use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize, de};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{navigation_state::AppSettings, navigation_state::SettingsError, paths};

pub(crate) const SETTINGS_FORMAT: &str = "ralgrum-settings";
pub(crate) const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_SETTINGS_FILE_BYTES: u64 = 128 * 1024;

#[derive(Debug)]
pub(crate) enum SettingsTransferError {
    InternalConfigPath,
    FileTooLarge,
    Read,
    Write,
    InvalidFormat,
    Serialization,
    Settings(SettingsError),
}

impl fmt::Display for SettingsTransferError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InternalConfigPath => {
                "Choose a settings file outside the app's internal configuration folder."
            }
            Self::FileTooLarge => "The settings file is too large.",
            Self::Read => "The settings file could not be read.",
            Self::Write => "The settings file could not be written.",
            Self::InvalidFormat => "The settings file is invalid.",
            Self::Serialization => "The settings file could not be prepared.",
            Self::Settings(error) => return error.fmt(formatter),
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SettingsTransferError {}

#[derive(Serialize)]
struct ExportEnvelope<'a> {
    format: &'static str,
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    settings: &'a AppSettings,
}

pub(crate) fn export_settings(
    path: &Path,
    settings: &AppSettings,
) -> Result<(), SettingsTransferError> {
    reject_internal_config_path(path)?;
    let settings_value =
        serde_json::to_value(settings).map_err(|_| SettingsTransferError::Serialization)?;
    let settings_object = settings_value
        .as_object()
        .ok_or(SettingsTransferError::Serialization)?;
    let expected_keys = expected_settings_keys()?;
    ensure_exact_keys(settings_object, expected_keys.iter().map(String::as_str))?;
    validate_settings_values(settings_object)?;
    let envelope = ExportEnvelope {
        format: SETTINGS_FORMAT,
        schema_version: SETTINGS_SCHEMA_VERSION,
        settings,
    };
    let encoded =
        serde_json::to_vec_pretty(&envelope).map_err(|_| SettingsTransferError::Serialization)?;
    atomic_write(path, &encoded)
}

pub(crate) fn import_settings(path: &Path) -> Result<AppSettings, SettingsTransferError> {
    reject_internal_config_path(path)?;
    let bytes = read_bounded(path)?;
    decode_settings(&bytes)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, SettingsTransferError> {
    let metadata = fs::metadata(path).map_err(|_| SettingsTransferError::Read)?;
    if metadata.len() > MAX_SETTINGS_FILE_BYTES {
        return Err(SettingsTransferError::FileTooLarge);
    }

    let file = File::open(path).map_err(|_| SettingsTransferError::Read)?;
    let mut bytes = Vec::with_capacity((MAX_SETTINGS_FILE_BYTES as usize).min(8192));
    file.take(MAX_SETTINGS_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SettingsTransferError::Read)?;
    if bytes.len() as u64 > MAX_SETTINGS_FILE_BYTES {
        return Err(SettingsTransferError::FileTooLarge);
    }
    Ok(bytes)
}

fn decode_settings(bytes: &[u8]) -> Result<AppSettings, SettingsTransferError> {
    if bytes.len() as u64 > MAX_SETTINGS_FILE_BYTES {
        return Err(SettingsTransferError::FileTooLarge);
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = NoDuplicateValue::deserialize(&mut deserializer)
        .map_err(|_| SettingsTransferError::InvalidFormat)?
        .0;
    deserializer
        .end()
        .map_err(|_| SettingsTransferError::InvalidFormat)?;

    let root = value
        .as_object()
        .ok_or(SettingsTransferError::InvalidFormat)?;
    ensure_exact_keys(root, ["format", "schemaVersion", "settings"])?;

    if root.get("format").and_then(Value::as_str) != Some(SETTINGS_FORMAT) {
        return Err(SettingsTransferError::InvalidFormat);
    }
    if root.get("schemaVersion").and_then(Value::as_u64) != Some(SETTINGS_SCHEMA_VERSION as u64) {
        return Err(SettingsTransferError::InvalidFormat);
    }

    let settings = root
        .get("settings")
        .and_then(Value::as_object)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    let expected_keys = expected_settings_keys()?;
    ensure_exact_keys(settings, expected_keys.iter().map(String::as_str))?;
    validate_settings_values(settings)?;

    serde_json::from_value(Value::Object(settings.clone()))
        .map_err(|_| SettingsTransferError::InvalidFormat)
}

struct NoDuplicateValue(Value);

impl<'de> Deserialize<'de> for NoDuplicateValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NoDuplicateVisitor;

        impl<'de> Visitor<'de> for NoDuplicateVisitor {
            type Value = NoDuplicateValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::from(value)))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::from(value)))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .map(NoDuplicateValue)
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::String(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(NoDuplicateValue(Value::Null))
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<NoDuplicateValue>()? {
                    values.push(value.0);
                }
                Ok(NoDuplicateValue(Value::Array(values)))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate JSON object key"));
                    }
                    let value = map.next_value::<NoDuplicateValue>()?;
                    values.insert(key, value.0);
                }
                Ok(NoDuplicateValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(NoDuplicateVisitor)
    }
}

fn expected_settings_keys() -> Result<BTreeSet<String>, SettingsTransferError> {
    let defaults = serde_json::to_value(AppSettings::default())
        .map_err(|_| SettingsTransferError::Serialization)?;
    let object = defaults
        .as_object()
        .ok_or(SettingsTransferError::Serialization)?;
    Ok(object.keys().cloned().collect())
}

fn ensure_exact_keys<'a>(
    object: &Map<String, Value>,
    expected: impl IntoIterator<Item = &'a str>,
) -> Result<(), SettingsTransferError> {
    let expected: BTreeSet<&str> = expected.into_iter().collect();
    if object.len() != expected.len() || object.keys().any(|key| !expected.contains(key.as_str())) {
        return Err(SettingsTransferError::InvalidFormat);
    }
    Ok(())
}

fn validate_settings_values(settings: &Map<String, Value>) -> Result<(), SettingsTransferError> {
    validate_string_enum(
        settings,
        "startPage",
        ["last", "search", "library", "downloads"],
    )?;
    validate_string_enum(
        settings,
        "lastMainTab",
        ["search", "library", "downloads", "cache"],
    )?;
    validate_string_enum(settings, "sourceFilter", ["all", "deezer", "soundcloud"])?;
    validate_string_enum(
        settings,
        "searchType",
        ["all", "tracks", "albums", "artists", "playlists"],
    )?;
    validate_string_enum(settings, "motionPreference", ["system", "full", "reduced"])?;
    validate_string_enum(settings, "lyricsSource", ["musixmatch", "genius"])?;
    validate_string_enum(settings, "repeatMode", ["off", "all", "one"])?;
    validate_string_enum(
        settings,
        "libraryService",
        ["local", "deezer", "soundcloud"],
    )?;
    validate_string_enum(settings, "rightSidebarView", ["lyrics", "queue"])?;

    let categories = settings
        .get("libraryCategories")
        .and_then(Value::as_object)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    ensure_exact_keys(categories, ["local", "deezer", "soundcloud"])?;
    validate_string_enum(categories, "local", ["tracks", "playlists"])?;
    validate_string_enum(
        categories,
        "deezer",
        [
            "tracks",
            "albums",
            "artists",
            "playlists",
            "history",
            "flow",
        ],
    )?;
    validate_string_enum(
        categories,
        "soundcloud",
        [
            "my-tracks",
            "tracks",
            "albums",
            "artists",
            "playlists",
            "history",
            "station",
        ],
    )?;

    let volume = settings
        .get("volume")
        .and_then(Value::as_f64)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(SettingsTransferError::InvalidFormat);
    }

    let cache_limit = settings
        .get("audioCacheLimitMb")
        .and_then(Value::as_u64)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    if !matches!(cache_limit, 256 | 512 | 1024 | 4096) {
        return Err(SettingsTransferError::InvalidFormat);
    }

    let history = settings
        .get("searchHistory")
        .and_then(Value::as_array)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    if history.len() > 5 {
        return Err(SettingsTransferError::InvalidFormat);
    }
    let mut normalized_history = BTreeSet::new();
    for value in history {
        let entry = value.as_str().ok_or(SettingsTransferError::InvalidFormat)?;
        if entry.is_empty()
            || entry.trim() != entry
            || entry.chars().count() > 256
            || !normalized_history.insert(entry.to_lowercase())
        {
            return Err(SettingsTransferError::InvalidFormat);
        }
    }

    Ok(())
}

fn validate_string_enum<const N: usize>(
    object: &Map<String, Value>,
    key: &str,
    allowed: [&str; N],
) -> Result<(), SettingsTransferError> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .ok_or(SettingsTransferError::InvalidFormat)?;
    if allowed.into_iter().any(|candidate| candidate == value) {
        Ok(())
    } else {
        Err(SettingsTransferError::InvalidFormat)
    }
}

fn reject_internal_config_path(path: &Path) -> Result<(), SettingsTransferError> {
    let Some(config_dir) = paths::config_dir() else {
        return Ok(());
    };
    if is_internal_config_path(path, &config_dir) {
        Err(SettingsTransferError::InternalConfigPath)
    } else {
        Ok(())
    }
}

fn is_internal_config_path(path: &Path, config_dir: &Path) -> bool {
    let lexical_path = lexical_location(path);
    let lexical_config_dir = lexical_location(config_dir);
    if is_same_or_descendant(&lexical_path, &lexical_config_dir) {
        return true;
    }
    let path = normalized_location(path);
    let config_dir = normalized_location(config_dir);
    is_same_or_descendant(&path, &config_dir)
}

fn is_same_or_descendant(path: &Path, ancestor: &Path) -> bool {
    let mut path_components = path.components();
    let mut ancestor_components = ancestor.components();
    loop {
        match (ancestor_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(ancestor), Some(path)) => {
                let ancestor = ancestor.as_os_str().to_string_lossy();
                let path = path.as_os_str().to_string_lossy();
                if if cfg!(windows) {
                    !ancestor.eq_ignore_ascii_case(&path)
                } else {
                    ancestor != path
                } {
                    return false;
                }
            }
        }
    }
}

fn normalized_location(path: &Path) -> PathBuf {
    let absolute = absolute_location(path);
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Prefix(prefix) => resolved.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => {
                resolved.push(component.as_os_str());
                if let Ok(canonical) = fs::canonicalize(&resolved) {
                    resolved = canonical;
                }
            }
        }
    }
    resolved
}

fn lexical_location(path: &Path) -> PathBuf {
    lexical_normalize(&absolute_location(path))
}

fn absolute_location(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir | Component::Normal(_) => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), SettingsTransferError> {
    let parent = path.parent().ok_or(SettingsTransferError::Write)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SettingsTransferError::Write)?;

    for _ in 0..8 {
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
        match write_new_synced(&temporary, contents) {
            Ok(()) => {
                let result = atomic_rename(&temporary, path);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result.map_err(|_| SettingsTransferError::Write);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                let _ = fs::remove_file(&temporary);
                return Err(SettingsTransferError::Write);
            }
        }
    }
    Err(SettingsTransferError::Write)
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
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tempfile::TempDir;

    fn envelope(settings: Value) -> Value {
        json!({
            "format": SETTINGS_FORMAT,
            "schemaVersion": SETTINGS_SCHEMA_VERSION,
            "settings": settings,
        })
    }

    fn valid_value() -> Value {
        envelope(serde_json::to_value(AppSettings::default()).unwrap())
    }

    fn assert_invalid(value: Value) {
        let encoded = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            decode_settings(&encoded),
            Err(SettingsTransferError::InvalidFormat)
        ));
    }

    #[test]
    fn valid_round_trip_has_exact_envelope_and_no_account_keys() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.json");
        let settings = AppSettings {
            audio_cache_limit_mb: 4096,
            search_history: vec!["Tracks".into(), "Albums".into()],
            ..AppSettings::default()
        };
        export_settings(&path, &settings).unwrap();

        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["format"], SETTINGS_FORMAT);
        assert_eq!(value["schemaVersion"], SETTINGS_SCHEMA_VERSION);
        assert_eq!(
            value["settings"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<BTreeSet<_>>(),
            expected_settings_keys()
                .unwrap()
                .iter()
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(import_settings(&path).unwrap(), settings);
        let keys = value["settings"].as_object().unwrap();
        for key in [
            "murglar",
            "soundcloud",
            "soundcloudMobile",
            "soundcloudCookies",
            "deezer",
            "deezerUserId",
            "deviceIdentity",
            "token",
        ] {
            assert!(!keys.contains_key(key), "unexpected account key: {key}");
        }
    }

    #[test]
    fn accepts_local_playlists_as_a_persisted_navigation_category() {
        let mut value = valid_value();
        value["settings"]["libraryCategories"]["local"] = Value::String("playlists".into());
        let decoded = decode_settings(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(decoded.library_categories.local, "playlists");
    }

    #[test]
    fn rejects_empty_unrelated_and_auth_shaped_values() {
        assert_invalid(json!({}));
        assert_invalid(json!({"murglar": "token"}));
        assert_invalid(json!({
            "format": SETTINGS_FORMAT,
            "schemaVersion": SETTINGS_SCHEMA_VERSION,
            "settings": {"murglar": "token"}
        }));
    }

    #[test]
    fn rejects_malformed_and_trailing_json() {
        assert!(matches!(
            decode_settings(b"{"),
            Err(SettingsTransferError::InvalidFormat)
        ));
        let mut encoded = serde_json::to_vec(&valid_value()).unwrap();
        encoded.extend_from_slice(b" {} ");
        assert!(matches!(
            decode_settings(&encoded),
            Err(SettingsTransferError::InvalidFormat)
        ));
    }

    #[test]
    fn rejects_duplicate_keys_at_every_object_depth() {
        let settings =
            serde_json::to_string(&serde_json::to_value(AppSettings::default()).unwrap()).unwrap();
        let duplicate_format = format!(
            r#"{{"format":"{SETTINGS_FORMAT}","format":"{SETTINGS_FORMAT}","schemaVersion":1,"settings":{settings}}}"#
        );
        let duplicate_settings = format!(
            r#"{{"format":"{SETTINGS_FORMAT}","schemaVersion":1,"settings":{settings},"settings":{settings}}}"#
        );

        let mut duplicate_app_field = settings.clone();
        duplicate_app_field.pop();
        duplicate_app_field.push_str(r#","volume":0.8}"#);
        let duplicate_app_field = format!(
            r#"{{"format":"{SETTINGS_FORMAT}","schemaVersion":1,"settings":{duplicate_app_field}}}"#
        );

        let categories = serde_json::to_string(
            &serde_json::to_value(&AppSettings::default().library_categories).unwrap(),
        )
        .unwrap();
        let mut duplicate_categories = categories.clone();
        duplicate_categories.pop();
        duplicate_categories.push_str(r#","deezer":"tracks"}"#);
        let settings_with_duplicate_categories = settings.replacen(
            &format!(r#""libraryCategories":{categories}"#),
            &format!(r#""libraryCategories":{duplicate_categories}"#),
            1,
        );
        let duplicate_categories = format!(
            r#"{{"format":"{SETTINGS_FORMAT}","schemaVersion":1,"settings":{settings_with_duplicate_categories}}}"#
        );

        for value in [
            duplicate_format,
            duplicate_settings,
            duplicate_app_field,
            duplicate_categories,
        ] {
            assert!(matches!(
                decode_settings(value.as_bytes()),
                Err(SettingsTransferError::InvalidFormat)
            ));
        }
    }

    #[test]
    fn rejects_missing_or_unknown_envelope_and_nested_fields() {
        let mut root = valid_value();
        root.as_object_mut().unwrap().remove("format");
        assert_invalid(root);

        let mut root = valid_value();
        root.as_object_mut()
            .unwrap()
            .insert("extra".into(), Value::Null);
        assert_invalid(root);

        let mut settings = valid_value();
        settings["settings"]
            .as_object_mut()
            .unwrap()
            .remove("volume");
        assert_invalid(settings);

        let mut settings = valid_value();
        settings["settings"]
            .as_object_mut()
            .unwrap()
            .insert("token".into(), Value::String("secret".into()));
        assert_invalid(settings);

        let mut categories = valid_value();
        categories["settings"]["libraryCategories"]
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), Value::String("tracks".into()));
        assert_invalid(categories);
    }

    #[test]
    fn rejects_wrong_marker_version_and_non_object_settings() {
        let mut wrong_marker = valid_value();
        wrong_marker["format"] = Value::String("other-settings".into());
        assert_invalid(wrong_marker);

        let mut wrong_version = valid_value();
        wrong_version["schemaVersion"] = json!(2);
        assert_invalid(wrong_version);

        let mut non_object = valid_value();
        non_object["settings"] = Value::Array(Vec::new());
        assert_invalid(non_object);
    }

    #[test]
    fn rejects_each_strict_enum_family() {
        let cases = [
            "startPage",
            "lastMainTab",
            "sourceFilter",
            "searchType",
            "motionPreference",
            "lyricsSource",
            "repeatMode",
            "libraryService",
            "rightSidebarView",
        ];
        for key in cases {
            let mut value = valid_value();
            value["settings"][key] = Value::String("unknown".into());
            assert_invalid(value);
        }
    }

    #[test]
    fn rejects_invalid_volume_cache_history_and_categories() {
        for volume in [json!(-0.1), json!(1.1), json!("0.5")] {
            let mut value = valid_value();
            value["settings"]["volume"] = volume;
            assert_invalid(value);
        }
        for limit in [json!(128), json!(768), json!(4097), json!(512.5)] {
            let mut value = valid_value();
            value["settings"]["audioCacheLimitMb"] = limit;
            assert_invalid(value);
        }

        for history in [
            json!(["one", "two", "three", "four", "five", "six"]),
            json!([" one"]),
            json!([""]),
            json!(["one", "ONE"]),
        ] {
            let mut value = valid_value();
            value["settings"]["searchHistory"] = history;
            assert_invalid(value);
        }

        let mut categories = valid_value();
        categories["settings"]["libraryCategories"]["deezer"] = Value::String("station".into());
        assert_invalid(categories);

        let mut categories = valid_value();
        categories["settings"]["libraryCategories"]["soundcloud"] = Value::String("flow".into());
        assert_invalid(categories);
    }

    #[test]
    fn rejects_oversized_input_before_decoding() {
        let bytes = vec![b' '; MAX_SETTINGS_FILE_BYTES as usize + 1];
        assert!(matches!(
            decode_settings(&bytes),
            Err(SettingsTransferError::FileTooLarge)
        ));
    }

    #[test]
    fn rejects_paths_inside_internal_config_directory() {
        let config = TempDir::new().unwrap();
        let path = config.path().join("settings.json");
        assert!(is_internal_config_path(&path, config.path()));
        assert!(is_internal_config_path(
            &config.path().join("nested").join("settings.json"),
            config.path()
        ));
        assert!(is_internal_config_path(
            &config
                .path()
                .join("nested")
                .join("..")
                .join("settings.json"),
            config.path()
        ));
        assert!(!is_internal_config_path(
            &config.path().parent().unwrap().join("settings.json"),
            config.path()
        ));

        let current = std::env::current_dir().unwrap();
        let relative_config = TempDir::new_in(&current).unwrap();
        if let Ok(relative) = relative_config.path().strip_prefix(&current) {
            assert!(is_internal_config_path(
                &relative.join("nested").join("settings.json"),
                relative_config.path()
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_parent_traversal_after_a_symlink_into_config() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let config = root.path().join("config");
        let external = root.path().join("external");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&external).unwrap();
        symlink(&config, external.join("link")).unwrap();

        let path = external
            .join("link")
            .join("..")
            .join("config")
            .join("nested")
            .join("settings.json");
        assert!(is_internal_config_path(&path, &config));
    }

    #[test]
    fn atomic_export_round_trips_and_replaces_existing_file() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("settings.json");
        export_settings(&path, &AppSettings::default()).unwrap();
        let first = fs::read(&path).unwrap();

        let changed = AppSettings {
            volume: 0.25,
            ..AppSettings::default()
        };
        export_settings(&path, &changed).unwrap();
        assert_ne!(first, fs::read(&path).unwrap());
        assert_eq!(import_settings(&path).unwrap(), changed);
        assert_eq!(
            fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
    }
}
