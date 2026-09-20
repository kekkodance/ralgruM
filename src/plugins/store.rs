use std::{
    collections::BTreeMap,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "plugin_settings.json";

#[derive(Clone, Debug)]
pub(crate) struct PluginStore {
    path: PathBuf,
    enabled: BTreeMap<String, bool>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPlugins {
    version: u32,
    enabled: BTreeMap<String, bool>,
}

#[derive(Debug)]
pub(crate) enum PluginStoreError {
    DirectoryUnavailable,
    Io(io::Error),
    Invalid(String),
}

impl fmt::Display for PluginStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryUnavailable => formatter.write_str("Plugin storage is unavailable."),
            Self::Io(error) => write!(formatter, "Could not access plugin settings: {error}"),
            Self::Invalid(error) => write!(formatter, "Invalid plugin settings: {error}"),
        }
    }
}

impl PluginStore {
    pub(crate) fn load_current_user() -> Result<Self, PluginStoreError> {
        let directory = crate::paths::config_dir().ok_or(PluginStoreError::DirectoryUnavailable)?;
        Self::load(directory.join(FILE_NAME))
    }

    fn load(path: PathBuf) -> Result<Self, PluginStoreError> {
        let enabled = match fs::read(&path) {
            Ok(bytes) => {
                let stored: StoredPlugins = serde_json::from_slice(&bytes)
                    .map_err(|error| PluginStoreError::Invalid(error.to_string()))?;
                if stored.version != 1 {
                    return Err(PluginStoreError::Invalid(format!(
                        "unsupported version {}",
                        stored.version
                    )));
                }
                stored.enabled
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(PluginStoreError::Io(error)),
        };
        Ok(Self { path, enabled })
    }

    pub(crate) fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(true)
    }

    pub(crate) fn with_enabled(&self, id: &str, enabled: bool) -> Result<Self, PluginStoreError> {
        let mut next = self.clone();
        next.enabled.insert(id.to_owned(), enabled);
        next.persist()?;
        Ok(next)
    }

    fn persist(&self) -> Result<(), PluginStoreError> {
        let contents = serde_json::to_vec_pretty(&StoredPlugins {
            version: 1,
            enabled: self.enabled.clone(),
        })
        .map_err(|error| PluginStoreError::Invalid(error.to_string()))?;
        let parent = self.path.parent().ok_or_else(|| {
            PluginStoreError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing directory",
            ))
        })?;
        fs::create_dir_all(parent).map_err(PluginStoreError::Io)?;
        let temporary = parent.join(format!(
            ".{FILE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        let write_result = (|| -> io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&contents)?;
            file.sync_all()?;
            replace_file(&temporary, &self.path)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        write_result.map_err(PluginStoreError::Io)
    }
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
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
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::{PluginStore, PluginStoreError};
    use std::fs;

    #[test]
    fn missing_file_enables_plugins_by_default_and_changes_survive_reload() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugin_settings.json");
        let store = PluginStore::load(path.clone()).unwrap();
        assert!(store.is_enabled("future-plugin"));
        assert!(!path.exists());

        let disabled = store.with_enabled("future-plugin", false).unwrap();
        assert!(!disabled.is_enabled("future-plugin"));
        assert!(
            !PluginStore::load(path.clone())
                .unwrap()
                .is_enabled("future-plugin")
        );
        let enabled = disabled.with_enabled("future-plugin", true).unwrap();
        assert!(enabled.is_enabled("future-plugin"));
        assert!(PluginStore::load(path).unwrap().is_enabled("future-plugin"));
    }

    #[test]
    fn invalid_or_future_settings_are_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plugin_settings.json");
        for contents in [r#"{"version":2,"enabled":{}}"#, "not json"] {
            fs::write(&path, contents).unwrap();
            assert!(matches!(
                PluginStore::load(path.clone()),
                Err(PluginStoreError::Invalid(_))
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }
}
