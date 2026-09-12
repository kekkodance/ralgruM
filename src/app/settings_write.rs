use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use super::{
    AppSettings, BACKUP_FILE, PRIMARY_FILE, SettingsError, atomic_write, encode,
    normalize_audio_cache_limit_mb,
};

#[derive(Default)]
pub(super) struct WriteCoordinator {
    latest: AtomicU64,
    writer: Mutex<()>,
}

#[derive(Clone)]
pub(crate) struct SettingsWrite {
    directory: PathBuf,
    settings: AppSettings,
    revision: u64,
    coordinator: Arc<WriteCoordinator>,
}

impl SettingsWrite {
    pub(super) fn new(
        directory: PathBuf,
        mut settings: AppSettings,
        coordinator: Arc<WriteCoordinator>,
    ) -> Self {
        settings.audio_cache_limit_mb =
            normalize_audio_cache_limit_mb(settings.audio_cache_limit_mb);
        let revision = coordinator
            .latest
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        Self {
            directory,
            settings,
            revision,
            coordinator,
        }
    }

    pub(crate) fn is_current(&self) -> bool {
        self.coordinator.latest.load(Ordering::SeqCst) == self.revision
    }

    pub(super) fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// Serialize replacements and skip work superseded before it acquires the writer.
    pub(crate) fn persist(&self) -> Result<bool, SettingsError> {
        let _guard = self
            .coordinator
            .writer
            .lock()
            .map_err(|_| SettingsError::Filesystem)?;
        if !self.is_current() {
            return Ok(false);
        }
        let encoded = encode(&self.settings)?;
        std::fs::create_dir_all(&self.directory).map_err(|_| SettingsError::Filesystem)?;
        atomic_write(&self.directory.join(BACKUP_FILE), &encoded)?;
        atomic_write(&self.directory.join(PRIMARY_FILE), &encoded)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn latest_preferences_win_even_when_workers_finish_out_of_order() {
        let directory = TempDir::new().unwrap();
        let coordinator = Arc::new(WriteCoordinator::default());
        let mut first = AppSettings::default();
        first.volume = 0.2;
        let older = SettingsWrite::new(directory.path().into(), first, coordinator.clone());
        let mut last = AppSettings::default();
        last.volume = 0.7;
        let newer = SettingsWrite::new(directory.path().into(), last.clone(), coordinator);
        assert!(newer.persist().unwrap());
        assert!(!older.persist().unwrap());
        let saved: AppSettings =
            serde_json::from_slice(&std::fs::read(directory.path().join(PRIMARY_FILE)).unwrap())
                .unwrap();
        assert_eq!(saved, last);
    }

    #[test]
    fn superseded_pending_write_does_not_touch_disk() {
        let directory = TempDir::new().unwrap();
        let coordinator = Arc::new(WriteCoordinator::default());
        let older = SettingsWrite::new(
            directory.path().join("pending"),
            AppSettings::default(),
            coordinator.clone(),
        );
        let _newer = SettingsWrite::new(
            directory.path().join("new"),
            AppSettings::default(),
            coordinator,
        );
        assert!(!older.persist().unwrap());
        assert!(!directory.path().join("pending").exists());
    }
}
