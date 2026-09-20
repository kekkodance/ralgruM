use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::state::MatchState;

const FILE_NAME: &str = "counter_strike_2_plugin.json";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlaybackAction {
    Pause,
    Mute,
    Volume(u8),
}

impl PlaybackAction {
    pub(super) fn label(self) -> String {
        match self {
            Self::Pause => "Pause".into(),
            Self::Mute => "Mute".into(),
            Self::Volume(percent) => format!("{percent}%"),
        }
    }

    pub(super) fn gain(self) -> Option<f32> {
        match self {
            Self::Pause => None,
            Self::Mute => Some(0.0),
            Self::Volume(percent) => Some(f32::from(percent.clamp(1, 100)) / 100.0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    version: u32,
    pub(super) active_round: PlaybackAction,
    pub(super) player_dead: PlaybackAction,
    pub(super) between_rounds: PlaybackAction,
    pub(super) fade_out_ms: u32,
    pub(super) fade_in_ms: u32,
    auth_token: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            active_round: PlaybackAction::Mute,
            player_dead: PlaybackAction::Volume(100),
            between_rounds: PlaybackAction::Volume(100),
            fade_out_ms: 500,
            fade_in_ms: 800,
            auth_token: uuid::Uuid::new_v4().simple().to_string(),
        }
    }
}

impl Settings {
    pub(super) fn load_or_create() -> Result<Self, String> {
        if let Some(settings) = loaded_current() {
            return Ok(settings);
        }
        let _guard = PERSIST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(settings) = loaded_current() {
            return Ok(settings);
        }
        let path = settings_path()?;
        let (settings, needs_create) = match fs::read(&path) {
            Ok(bytes) => (
                serde_json::from_slice::<Self>(&bytes)
                    .map_err(|error| format!("Counter-Strike 2 settings are invalid: {error}"))?,
                false,
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (Self::default(), true),
            Err(error) => {
                return Err(format!(
                    "Counter-Strike 2 settings could not be read: {error}"
                ));
            }
        };
        settings.validate()?;
        if needs_create {
            settings.persist_unlocked()?;
        }
        set_current(settings.clone());
        Ok(settings)
    }

    pub(super) fn begin_background_persist(&self) -> u64 {
        set_current(self.clone());
        SAVE_REVISION.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    pub(super) fn persist_revision(&self, revision: u64) -> Result<bool, String> {
        let _guard = PERSIST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if SAVE_REVISION.load(Ordering::Acquire) != revision {
            return Ok(false);
        }
        self.persist_unlocked()?;
        Ok(true)
    }

    pub(super) fn revision_is_current(revision: u64) -> bool {
        SAVE_REVISION.load(Ordering::Acquire) == revision
    }

    fn persist_unlocked(&self) -> Result<(), String> {
        self.validate()?;
        let path = settings_path()?;
        let parent = path
            .parent()
            .ok_or("Counter-Strike 2 settings path has no parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Counter-Strike 2 settings could not be saved: {error}"))?;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("Counter-Strike 2 settings could not be encoded: {error}"))?;
        let temporary = parent.join(format!(
            ".{FILE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        let result = (|| -> io::Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            replace_file(&temporary, &path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|error| format!("Counter-Strike 2 settings could not be saved: {error}"))?;
        Ok(())
    }

    pub(super) fn action_for(&self, state: MatchState) -> Option<PlaybackAction> {
        match state {
            MatchState::ActiveRound => Some(self.active_round),
            MatchState::PlayerDead => Some(self.player_dead),
            MatchState::BetweenRounds => Some(self.between_rounds),
            MatchState::Inactive => None,
        }
    }

    pub(super) fn fade_out(&self) -> Duration {
        Duration::from_millis(u64::from(self.fade_out_ms))
    }

    pub(super) fn fade_in(&self) -> Duration {
        Duration::from_millis(u64::from(self.fade_in_ms))
    }

    pub(super) fn auth_token(&self) -> &str {
        &self.auth_token
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "Counter-Strike 2 settings use unsupported version {}",
                self.version
            ));
        }
        if self.auth_token.is_empty() {
            return Err("Counter-Strike 2 settings have no authentication token".into());
        }
        for action in [self.active_round, self.player_dead, self.between_rounds] {
            if let PlaybackAction::Volume(percent) = action
                && !(1..=100).contains(&percent)
            {
                return Err("Counter-Strike 2 volume must be between 1% and 100%".into());
            }
        }
        if self.fade_out_ms > 5_000 || self.fade_in_ms > 5_000 {
            return Err("Counter-Strike 2 fade durations must not exceed 5 seconds".into());
        }
        Ok(())
    }
}

pub(super) fn current() -> Settings {
    loaded_current().unwrap_or_else(|| {
        let settings = Settings::default();
        set_current(settings.clone());
        settings
    })
}

fn loaded_current() -> Option<Settings> {
    CURRENT.get().map(|current| {
        current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    })
}

fn set_current(settings: Settings) {
    let initial = settings.clone();
    *CURRENT
        .get_or_init(|| RwLock::new(initial))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings;
}

fn settings_path() -> Result<PathBuf, String> {
    crate::paths::config_dir()
        .map(|directory| directory.join(FILE_NAME))
        .ok_or_else(|| "The ralgruM configuration directory is unavailable".into())
}

static CURRENT: OnceLock<RwLock<Settings>> = OnceLock::new();
static PERSIST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SAVE_REVISION: AtomicU64 = AtomicU64::new(0);

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
    use super::*;

    #[test]
    fn actions_map_to_safe_relative_gains() {
        assert_eq!(PlaybackAction::Pause.gain(), None);
        assert_eq!(PlaybackAction::Mute.gain(), Some(0.0));
        assert_eq!(PlaybackAction::Volume(35).gain(), Some(0.35));
    }

    #[test]
    fn inactive_match_does_not_claim_playback() {
        assert_eq!(Settings::default().action_for(MatchState::Inactive), None);
    }
}
