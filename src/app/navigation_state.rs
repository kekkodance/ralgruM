use std::{
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::playback::RepeatMode;

#[path = "settings_write.rs"]
mod persistence;

pub(crate) use persistence::SettingsWrite;

const PRIMARY_FILE: &str = "general_settings.json";
const BACKUP_FILE: &str = "general_settings.backup.json";
const LEGACY_PREFERENCES_FILE: &str = "app_preferences.json";
const DEFAULT_AUDIO_CACHE_LIMIT_MB: u64 = 512;

fn normalize_audio_cache_limit_mb(limit_mb: u64) -> u64 {
    match limit_mb {
        256 | 512 | 1024 | 4096 => limit_mb,
        _ => DEFAULT_AUDIO_CACHE_LIMIT_MB,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyPreferences {
    audio_cache_limit_mb: u64,
    #[serde(default)]
    downloads_dir: Option<PathBuf>,
}

impl LegacyPreferences {
    fn into_settings(self) -> AppSettings {
        let mut settings = AppSettings::default();
        settings.audio_cache_limit_mb = normalize_audio_cache_limit_mb(self.audio_cache_limit_mb);
        settings.downloads_dir = self.downloads_dir;
        settings
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StartPage {
    #[default]
    Last,
    #[serde(rename = "search")]
    Discover,
    Library,
    Downloads,
}

impl StartPage {
    pub(crate) const ALL: [Self; 4] = [Self::Last, Self::Discover, Self::Library, Self::Downloads];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Last => "Last used",
            Self::Discover => "Discover",
            Self::Library => "Library",
            Self::Downloads => "Downloads",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MotionPreference {
    #[default]
    System,
    Full,
    Reduced,
}

impl MotionPreference {
    pub(crate) fn is_reduced(self) -> bool {
        match self {
            Self::System => system_prefers_reduced_motion(),
            Self::Full => false,
            Self::Reduced => true,
        }
    }
}

#[cfg(windows)]
fn system_prefers_reduced_motion() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };

    let mut client_area_animation: i32 = 0;
    // SAFETY: the pointer references a live Win32 BOOL-sized value for the
    // duration of the call, and this action writes only to that output.
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut client_area_animation as *mut i32).cast()),
            Default::default(),
        )
    };

    result.is_ok() && client_area_animation == 0
}

#[cfg(not(windows))]
fn system_prefers_reduced_motion() -> bool {
    false
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LyricsSource {
    #[default]
    Musixmatch,
    Genius,
}

impl LyricsSource {
    pub(crate) const ALL: [Self; 2] = [Self::Musixmatch, Self::Genius];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Musixmatch => "Musixmatch",
            Self::Genius => "Genius",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MainDestination {
    #[default]
    #[serde(rename = "search")]
    Discover,
    Library,
    Downloads,
    Cache,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RightSidebarView {
    #[default]
    Lyrics,
    Queue,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchSource {
    #[default]
    All,
    Deezer,
    Soundcloud,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LibraryService {
    Local,
    #[default]
    Deezer,
    Soundcloud,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct LibraryCategories {
    pub(crate) local: String,
    pub(crate) deezer: String,
    pub(crate) soundcloud: String,
}

impl Default for LibraryCategories {
    fn default() -> Self {
        Self {
            local: "tracks".into(),
            deezer: "tracks".into(),
            soundcloud: "my-tracks".into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct AppSettings {
    #[serde(deserialize_with = "deserialize_start_page")]
    pub(crate) start_page: StartPage,
    pub(crate) remember_navigation: bool,
    #[serde(deserialize_with = "deserialize_main_destination")]
    pub(crate) last_main_tab: MainDestination,
    #[serde(deserialize_with = "deserialize_search_source")]
    pub(crate) source_filter: SearchSource,
    pub(crate) search_type: String,
    pub(crate) restore_window: bool,
    pub(crate) close_to_tray: bool,
    pub(crate) motion_preference: MotionPreference,
    pub(crate) block_explicit_content: bool,
    pub(crate) seamless_playback: bool,
    pub(crate) remember_playback_modes: bool,
    pub(crate) lyrics_source: LyricsSource,
    pub(crate) volume: f32,
    pub(crate) muted: bool,
    pub(crate) repeat_mode: RepeatMode,
    pub(crate) shuffle_enabled: bool,
    #[serde(deserialize_with = "deserialize_library_service")]
    pub(crate) library_service: LibraryService,
    pub(crate) library_categories: LibraryCategories,
    pub(crate) downloads_dir: Option<PathBuf>,
    pub(crate) discord_presence: bool,
    pub(crate) audio_cache_limit_mb: u64,
    pub(crate) background_audio_cache: bool,
    pub(crate) record_deezer_plays: bool,
    pub(crate) soundcloud_search_suggestions: bool,
    pub(crate) search_history: Vec<String>,
    #[serde(alias = "lyricsOpen")]
    pub(crate) right_sidebar_open: bool,
    #[serde(deserialize_with = "deserialize_right_sidebar_view")]
    pub(crate) right_sidebar_view: RightSidebarView,
}

impl AppSettings {
    pub(crate) fn merge_live_state_from(&mut self, current: &Self) {
        self.last_main_tab = current.last_main_tab;
        self.source_filter = current.source_filter;
        self.search_type = current.search_type.clone();
        self.library_service = current.library_service;
        self.library_categories = current.library_categories.clone();
        self.volume = current.volume;
        self.muted = current.muted;
        self.repeat_mode = current.repeat_mode;
        self.shuffle_enabled = current.shuffle_enabled;
        self.right_sidebar_open = current.right_sidebar_open;
        self.right_sidebar_view = current.right_sidebar_view;
        self.search_history = current.search_history.clone();
    }

    pub(crate) fn apply_volume_preferences(&mut self, volume: f32, muted: bool) -> bool {
        let changed = self.volume != volume || self.muted != muted;
        self.volume = volume;
        self.muted = muted;
        changed
    }

    pub(crate) fn apply_playback_modes(
        &mut self,
        repeat_mode: RepeatMode,
        shuffle_enabled: bool,
    ) -> bool {
        if !self.remember_playback_modes {
            return false;
        }
        let changed = self.repeat_mode != repeat_mode || self.shuffle_enabled != shuffle_enabled;
        self.repeat_mode = repeat_mode;
        self.shuffle_enabled = shuffle_enabled;
        changed
    }

    pub(crate) fn effective_downloads_dir(&self) -> PathBuf {
        if let Some(dir) = &self.downloads_dir {
            return dir.clone();
        }
        static ONCE_DOWNLOAD_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        ONCE_DOWNLOAD_DIR
            .get_or_init(|| dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")))
            .clone()
    }

    pub(crate) fn validate_downloads_dir(&mut self) -> Result<(), SettingsError> {
        let Some(path) = self.downloads_dir.clone() else {
            return Ok(());
        };
        fs::create_dir_all(&path).map_err(|_| SettingsError::DownloadDirectory)?;
        let path = fs::canonicalize(path).map_err(|_| SettingsError::DownloadDirectory)?;
        if !path.is_dir() {
            return Err(SettingsError::DownloadDirectory);
        }
        self.downloads_dir = Some(path);
        Ok(())
    }

    pub(crate) fn validate_imported_downloads_dir(&mut self) -> Result<(), SettingsError> {
        let Some(path) = self.downloads_dir.clone() else {
            return Ok(());
        };
        let path = fs::canonicalize(path).map_err(|_| SettingsError::DownloadDirectory)?;
        if !path.is_dir() {
            return Err(SettingsError::DownloadDirectory);
        }
        self.downloads_dir = Some(path);
        Ok(())
    }
}

fn deserialize_start_page<'de, D>(deserializer: D) -> Result<StartPage, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "search" => StartPage::Discover,
        "library" => StartPage::Library,
        "downloads" => StartPage::Downloads,
        _ => StartPage::Last,
    })
}

fn deserialize_main_destination<'de, D>(deserializer: D) -> Result<MainDestination, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "library" => MainDestination::Library,
        "downloads" => MainDestination::Downloads,
        "cache" => MainDestination::Cache,
        _ => MainDestination::Discover,
    })
}

fn deserialize_search_source<'de, D>(deserializer: D) -> Result<SearchSource, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "deezer" => SearchSource::Deezer,
        "soundcloud" => SearchSource::Soundcloud,
        _ => SearchSource::All,
    })
}

fn deserialize_library_service<'de, D>(deserializer: D) -> Result<LibraryService, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(match value.as_str() {
        "local" => LibraryService::Local,
        "soundcloud" => LibraryService::Soundcloud,
        _ => LibraryService::Deezer,
    })
}

fn deserialize_right_sidebar_view<'de, D>(deserializer: D) -> Result<RightSidebarView, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(if value == "queue" {
        RightSidebarView::Queue
    } else {
        RightSidebarView::Lyrics
    })
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            start_page: StartPage::Last,
            remember_navigation: true,
            last_main_tab: MainDestination::Discover,
            restore_window: true,
            close_to_tray: true,
            motion_preference: MotionPreference::System,
            block_explicit_content: false,
            seamless_playback: true,
            remember_playback_modes: true,
            lyrics_source: LyricsSource::Musixmatch,
            volume: 0.8,
            muted: false,
            repeat_mode: RepeatMode::Off,
            shuffle_enabled: false,
            source_filter: SearchSource::All,
            search_type: "all".into(),
            library_service: LibraryService::Deezer,
            library_categories: LibraryCategories::default(),
            downloads_dir: None,
            discord_presence: true,
            audio_cache_limit_mb: DEFAULT_AUDIO_CACHE_LIMIT_MB,
            background_audio_cache: true,
            record_deezer_plays: true,
            soundcloud_search_suggestions: true,
            search_history: Vec::new(),
            right_sidebar_open: false,
            right_sidebar_view: RightSidebarView::Lyrics,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SettingsStore {
    directory: PathBuf,
    settings: AppSettings,
    writes: std::sync::Arc<persistence::WriteCoordinator>,
}

impl SettingsStore {
    pub(crate) fn load_current_user() -> Result<Self, SettingsError> {
        let directory =
            super::paths::config_dir().ok_or(SettingsError::ConfigDirectoryUnavailable)?;
        Self::load(&directory)
    }

    fn load(directory: &Path) -> Result<Self, SettingsError> {
        fs::create_dir_all(directory).map_err(|_| SettingsError::Filesystem)?;
        let primary_path = directory.join(PRIMARY_FILE);
        let backup_path = directory.join(BACKUP_FILE);
        let primary = read_settings(&primary_path)?;
        let backup = read_settings(&backup_path)?;
        let settings = match (primary, backup) {
            (StoredSettings::Valid(primary), StoredSettings::Valid(backup)) => {
                if primary != backup {
                    atomic_write(&backup_path, &encode(&primary)?)?;
                }
                primary
            }
            (
                StoredSettings::Valid(settings),
                StoredSettings::Missing | StoredSettings::Invalid,
            ) => {
                atomic_write(&backup_path, &encode(&settings)?)?;
                settings
            }
            (
                StoredSettings::Missing | StoredSettings::Invalid,
                StoredSettings::Valid(settings),
            ) => {
                atomic_write(&primary_path, &encode(&settings)?)?;
                settings
            }
            (StoredSettings::Missing, StoredSettings::Missing) => {
                let settings = read_legacy_preferences(&directory.join(LEGACY_PREFERENCES_FILE));
                let encoded = encode(&settings)?;
                atomic_write(&primary_path, &encoded)?;
                atomic_write(&backup_path, &encoded)?;
                settings
            }
            _ => return Err(SettingsError::ExistingSettingsInvalid),
        };
        Ok(Self {
            directory: directory.to_owned(),
            settings,
            writes: Default::default(),
        })
    }

    pub(crate) fn settings(&self) -> &AppSettings {
        &self.settings
    }

    #[cfg(test)]
    pub(crate) fn downloads_dir(&self) -> Option<&Path> {
        self.settings.downloads_dir.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn set_downloads_dir(
        &mut self,
        downloads_dir: Option<PathBuf>,
    ) -> Result<(), SettingsError> {
        let mut settings = self.settings.clone();
        settings.downloads_dir = downloads_dir;
        self.persist(settings)
    }

    pub(crate) fn prepare_persist(&self, settings: AppSettings) -> SettingsWrite {
        SettingsWrite::new(self.directory.clone(), settings, self.writes.clone())
    }

    pub(crate) fn accept_persisted(&mut self, write: &SettingsWrite) {
        if write.is_current() {
            self.settings = write.settings().clone();
        }
    }

    pub(crate) fn persist(&mut self, settings: AppSettings) -> Result<(), SettingsError> {
        let write = self.prepare_persist(settings);
        if write.persist()? {
            self.settings = write.settings().clone();
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsError {
    ConfigDirectoryUnavailable,
    Filesystem,
    ExistingSettingsInvalid,
    Serialization,
    DownloadDirectory,
}

impl fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ConfigDirectoryUnavailable => "Settings storage is unavailable.",
            Self::Filesystem => "Settings could not be stored.",
            Self::ExistingSettingsInvalid => "The saved settings are invalid.",
            Self::Serialization => "Settings could not be prepared.",
            Self::DownloadDirectory => "The downloads folder could not be created or accessed.",
        })
    }
}

enum StoredSettings {
    Missing,
    Valid(AppSettings),
    Invalid,
}

fn read_settings(path: &Path) -> Result<StoredSettings, SettingsError> {
    match fs::read(path) {
        Ok(bytes) => Ok(match serde_json::from_slice::<AppSettings>(&bytes) {
            Ok(mut settings) => {
                settings.audio_cache_limit_mb =
                    normalize_audio_cache_limit_mb(settings.audio_cache_limit_mb);
                StoredSettings::Valid(settings)
            }
            Err(_) => StoredSettings::Invalid,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(StoredSettings::Missing),
        Err(_) => Err(SettingsError::Filesystem),
    }
}

fn read_legacy_preferences(path: &Path) -> AppSettings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LegacyPreferences>(&bytes).ok())
        .map(LegacyPreferences::into_settings)
        .unwrap_or_default()
}

fn encode(settings: &AppSettings) -> Result<Vec<u8>, SettingsError> {
    serde_json::to_vec_pretty(settings).map_err(|_| SettingsError::Serialization)
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), SettingsError> {
    let parent = path.parent().ok_or(SettingsError::Filesystem)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(SettingsError::Filesystem)?;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
        match write_new_synced(&temporary, contents) {
            Ok(()) => {
                let result = atomic_rename(&temporary, path);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result.map_err(|_| SettingsError::Filesystem);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                let _ = fs::remove_file(&temporary);
                return Err(SettingsError::Filesystem);
            }
        }
    }
    Err(SettingsError::Filesystem)
}

fn write_new_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)
}

#[cfg(windows)]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
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
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn defaults_match_original_public_settings_state() {
        assert_eq!(
            serde_json::to_value(AppSettings::default()).unwrap(),
            json!({
                "startPage": "last",
                "rememberNavigation": true,
                "lastMainTab": "search",
                "sourceFilter": "all",
                "searchType": "all",
                "libraryService": "deezer",
                "libraryCategories": {
                    "local": "tracks",
                    "deezer": "tracks",
                    "soundcloud": "my-tracks"
                },
                "downloadsDir": null,
                "discordPresence": true,
                "restoreWindow": true,
                "closeToTray": true,
                "motionPreference": "system",
                "blockExplicitContent": false,
                "seamlessPlayback": true,
                "rememberPlaybackModes": true,
                "lyricsSource": "musixmatch",
                "volume": 0.800000011920929,
                "muted": false,
                "repeatMode": "off",
                "shuffleEnabled": false
                ,"audioCacheLimitMb": 512
                ,"backgroundAudioCache": true
                ,"recordDeezerPlays": true
                ,"soundcloudSearchSuggestions": true
                ,"searchHistory": []
                ,"rightSidebarOpen": false
                ,"rightSidebarView": "lyrics"
            })
        );
    }

    #[test]
    fn legacy_preferences_preserve_four_gigabyte_cache_limit() {
        let settings = LegacyPreferences {
            audio_cache_limit_mb: 4096,
            downloads_dir: None,
        }
        .into_settings();

        assert_eq!(settings.audio_cache_limit_mb, 4096);
    }

    #[test]
    fn settings_store_round_trip_preserves_four_gigabyte_cache_limit() {
        let temp = TempDir::new().unwrap();
        let mut store = SettingsStore::load(temp.path()).unwrap();
        let mut settings = store.settings().clone();
        settings.audio_cache_limit_mb = 4096;
        store.persist(settings.clone()).unwrap();

        let loaded = SettingsStore::load(temp.path()).unwrap();
        assert_eq!(loaded.settings(), &settings);
    }

    #[test]
    fn stored_unsupported_cache_limit_falls_back_to_default() {
        let temp = TempDir::new().unwrap();
        let store = SettingsStore::load(temp.path()).unwrap();
        let mut tampered = store.settings().clone();
        tampered.audio_cache_limit_mb = 9999;
        let encoded = serde_json::to_vec_pretty(&tampered).unwrap();
        fs::write(temp.path().join(PRIMARY_FILE), &encoded).unwrap();
        fs::write(temp.path().join(BACKUP_FILE), &encoded).unwrap();

        let loaded = SettingsStore::load(temp.path()).unwrap();
        assert_eq!(
            loaded.settings().audio_cache_limit_mb,
            DEFAULT_AUDIO_CACHE_LIMIT_MB
        );
    }

    #[test]
    fn settings_round_trip_and_repair_a_missing_copy() {
        let temp = TempDir::new().unwrap();
        let mut store = SettingsStore::load(temp.path()).unwrap();
        let mut settings = store.settings().clone();
        settings.start_page = StartPage::Library;
        settings.source_filter = SearchSource::Soundcloud;
        settings.library_categories.soundcloud = "history".into();
        store.persist(settings.clone()).unwrap();
        fs::remove_file(temp.path().join(BACKUP_FILE)).unwrap();

        let loaded = SettingsStore::load(temp.path()).unwrap();
        assert_eq!(loaded.settings(), &settings);
        assert!(temp.path().join(BACKUP_FILE).exists());
    }

    #[test]
    fn load_repairs_a_mismatched_valid_backup_from_primary() {
        let temp = TempDir::new().unwrap();
        let mut store = SettingsStore::load(temp.path()).unwrap();
        let mut primary = store.settings().clone();
        primary.start_page = StartPage::Library;
        store.persist(primary.clone()).unwrap();

        let mut stale_backup = primary.clone();
        stale_backup.start_page = StartPage::Downloads;
        fs::write(
            temp.path().join(BACKUP_FILE),
            serde_json::to_vec_pretty(&stale_backup).unwrap(),
        )
        .unwrap();

        let loaded = SettingsStore::load(temp.path()).unwrap();
        assert_eq!(loaded.settings(), &primary);
        assert_eq!(
            serde_json::from_slice::<AppSettings>(
                &fs::read(temp.path().join(BACKUP_FILE)).unwrap()
            )
            .unwrap(),
            primary
        );
    }

    #[test]
    fn unknown_or_missing_fields_use_original_defaults() {
        let settings: AppSettings = serde_json::from_value(json!({
            "startPage": "unknown",
            "rememberNavigation": false,
            "lastMainTab": "unknown",
            "sourceFilter": "unknown",
            "libraryService": "unknown"
        }))
        .unwrap();
        assert_eq!(settings.start_page, StartPage::Last);
        assert_eq!(settings.last_main_tab, MainDestination::Discover);
        assert_eq!(settings.source_filter, SearchSource::All);
        assert_eq!(settings.library_service, LibraryService::Deezer);
        assert_eq!(settings.downloads_dir, None);
        assert!(settings.discord_presence);
        assert!(settings.record_deezer_plays);
        assert!(settings.soundcloud_search_suggestions);
        assert!(settings.search_history.is_empty());

        let settings: AppSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(settings, AppSettings::default());
        assert!(settings.close_to_tray);
    }

    #[test]
    fn cache_main_destination_deserializes_and_round_trips() {
        let settings: AppSettings = serde_json::from_value(json!({
            "lastMainTab": "cache"
        }))
        .unwrap();
        assert_eq!(settings.last_main_tab, MainDestination::Cache);
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["lastMainTab"],
            "cache"
        );
    }

    #[test]
    fn deezer_play_preference_round_trips_and_defaults_to_enabled() {
        let defaults: AppSettings = serde_json::from_value(json!({})).unwrap();
        assert!(defaults.record_deezer_plays);

        let mut settings = AppSettings::default();
        settings.record_deezer_plays = false;
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(encoded["recordDeezerPlays"], false);
        let decoded: AppSettings = serde_json::from_value(encoded).unwrap();
        assert!(!decoded.record_deezer_plays);
    }

    #[test]
    fn soundcloud_suggestion_preference_and_search_history_round_trip() {
        let defaults: AppSettings = serde_json::from_value(json!({})).unwrap();
        assert!(defaults.soundcloud_search_suggestions);
        assert!(defaults.search_history.is_empty());

        let mut settings = AppSettings::default();
        settings.soundcloud_search_suggestions = false;
        settings.search_history = vec!["Skrillex".into(), "Daft Punk".into()];
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(encoded["soundcloudSearchSuggestions"], false);
        assert_eq!(encoded["searchHistory"], json!(["Skrillex", "Daft Punk"]));
        let decoded: AppSettings = serde_json::from_value(encoded).unwrap();
        assert!(!decoded.soundcloud_search_suggestions);
        assert_eq!(decoded.search_history, settings.search_history);
    }

    #[test]
    fn search_history_survives_store_reload() {
        let temp = TempDir::new().unwrap();
        let mut store = SettingsStore::load(temp.path()).unwrap();
        let mut settings = store.settings().clone();
        settings.search_history = vec!["Skrillex".into(), "Daft Punk".into()];
        store.persist(settings).unwrap();
        drop(store);

        let reloaded = SettingsStore::load(temp.path()).unwrap();
        assert_eq!(
            reloaded.settings().search_history,
            ["Skrillex", "Daft Punk"]
        );
    }

    #[test]
    fn close_to_tray_preference_round_trips_and_old_settings_default_to_enabled() {
        let defaults: AppSettings = serde_json::from_value(json!({})).unwrap();
        assert!(defaults.close_to_tray);

        let mut settings = AppSettings::default();
        settings.close_to_tray = false;
        let encoded = serde_json::to_value(&settings).unwrap();
        assert_eq!(encoded["closeToTray"], false);
        assert!(
            !serde_json::from_value::<AppSettings>(encoded)
                .unwrap()
                .close_to_tray
        );
    }

    #[test]
    fn motion_preference_resolves_system_and_explicit_choices() {
        assert!(!MotionPreference::Full.is_reduced());
        assert!(MotionPreference::Reduced.is_reduced());
        assert_eq!(
            MotionPreference::System.is_reduced(),
            system_prefers_reduced_motion()
        );
        assert_eq!(
            AppSettings::default().motion_preference,
            MotionPreference::System
        );
    }

    #[test]
    fn general_and_playback_preferences_round_trip() {
        let temp = TempDir::new().unwrap();
        let mut store = SettingsStore::load(temp.path()).unwrap();
        let mut settings = store.settings().clone();
        settings.restore_window = false;
        settings.motion_preference = MotionPreference::Reduced;
        settings.block_explicit_content = true;
        settings.seamless_playback = false;
        settings.remember_playback_modes = true;
        settings.lyrics_source = LyricsSource::Genius;
        settings.volume = 0.35;
        settings.muted = true;
        settings.repeat_mode = RepeatMode::One;
        settings.shuffle_enabled = true;
        store.persist(settings.clone()).unwrap();

        assert_eq!(
            SettingsStore::load(temp.path()).unwrap().settings(),
            &settings
        );
    }

    #[test]
    fn volume_persists_independently_while_playback_modes_are_gated() {
        let mut settings = AppSettings::default();
        assert!(settings.apply_volume_preferences(0.4, true));
        assert!(settings.apply_playback_modes(RepeatMode::All, true));
        assert!(!settings.apply_volume_preferences(0.4, true));
        assert!(!settings.apply_playback_modes(RepeatMode::All, true));

        settings.remember_playback_modes = false;
        assert!(settings.apply_volume_preferences(0.9, false));
        assert!(!settings.apply_playback_modes(RepeatMode::One, false));
        assert_eq!(settings.volume, 0.9);
        assert!(!settings.muted);
        assert_eq!(settings.repeat_mode, RepeatMode::All);
        assert!(settings.shuffle_enabled);
    }

    #[test]
    fn sidebar_defaults_normalize_unknown_view_and_migrate_lyrics_open() {
        let defaults: AppSettings = serde_json::from_value(json!({})).unwrap();
        assert!(!defaults.right_sidebar_open);
        assert_eq!(defaults.right_sidebar_view, RightSidebarView::Lyrics);

        let legacy: AppSettings = serde_json::from_value(json!({ "lyricsOpen": true })).unwrap();
        assert!(legacy.right_sidebar_open);
        assert_eq!(legacy.right_sidebar_view, RightSidebarView::Lyrics);

        let normalized: AppSettings = serde_json::from_value(json!({
            "rightSidebarOpen": true,
            "rightSidebarView": "unknown"
        }))
        .unwrap();
        assert!(normalized.right_sidebar_open);
        assert_eq!(normalized.right_sidebar_view, RightSidebarView::Lyrics);
    }

    #[test]
    fn custom_downloads_directory_is_persisted_and_reset() {
        let temp = TempDir::new().unwrap();
        let downloads = temp.path().join("music");
        let mut store = SettingsStore::load(temp.path()).unwrap();

        store.set_downloads_dir(Some(downloads.clone())).unwrap();
        assert_eq!(store.downloads_dir(), Some(downloads.as_path()));
        assert_eq!(store.settings().effective_downloads_dir(), downloads);

        store.set_downloads_dir(None).unwrap();
        assert_eq!(store.downloads_dir(), None);
    }

    #[test]
    fn downloads_directory_is_created_canonicalized_and_rejects_files() {
        let temp = TempDir::new().unwrap();
        let mut settings = AppSettings {
            downloads_dir: Some(temp.path().join("new").join("music")),
            ..AppSettings::default()
        };
        settings.validate_downloads_dir().unwrap();
        let downloads = settings.downloads_dir.unwrap();
        assert!(downloads.is_absolute());
        assert!(downloads.is_dir());

        let file = temp.path().join("not-a-directory");
        fs::write(&file, b"file").unwrap();
        let mut settings = AppSettings {
            downloads_dir: Some(file),
            ..AppSettings::default()
        };
        assert_eq!(
            settings.validate_downloads_dir(),
            Err(SettingsError::DownloadDirectory)
        );
    }

    #[test]
    fn imported_downloads_directory_must_exist_without_being_created() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing").join("music");
        let mut settings = AppSettings {
            downloads_dir: Some(missing.clone()),
            ..AppSettings::default()
        };

        assert_eq!(
            settings.validate_imported_downloads_dir(),
            Err(SettingsError::DownloadDirectory)
        );
        assert!(!missing.exists());
    }
}
