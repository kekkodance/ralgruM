use std::{
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const APP_DIR_NAME: &str = "ralgruM";
const LEGACY_CONFIG_DIR_NAME: &str = "app.ralgrum.player";
const LEGACY_CACHE_DIR_NAME: &str = "ralgrum";

const CONFIG_MARKERS: &[&str] = &[
    "auth_session.dat",
    "auth_session.backup.dat",
    "auth_session.json",
    "auth_session.backup.json",
    "deezer_sid.txt",
    "window_state.json",
    "window_state.backup.json",
    "general_settings.json",
    "general_settings.backup.json",
    "app_preferences.json",
    "device_config.json",
    "device_config.backup.json",
];

const CACHE_MARKERS: &[&str] = &["artwork-v1", "library-v1", "audio-v1"];

pub(crate) fn config_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        dirs::config_dir().map(|base| {
            prepare_app_dir(
                base.join(APP_DIR_NAME),
                base.join(LEGACY_CONFIG_DIR_NAME),
                CONFIG_MARKERS,
            )
        })
    })
    .clone()
}

/// The only production authority for session migration and plaintext cleanup.
/// Arbitrary session-store paths must not discover the current user's directories.
pub(super) fn session_directories() -> Option<(PathBuf, Option<PathBuf>)> {
    let active = config_dir()?;
    let legacy = dirs::config_dir()?.join(LEGACY_CONFIG_DIR_NAME);
    let legacy = (!paths_refer_to_same_location(&active, &legacy)).then_some(legacy);
    Some((active, legacy))
}

pub(crate) fn cache_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = dirs::cache_dir().unwrap_or_else(|| PathBuf::from("."));
        prepare_app_dir(
            base.join(APP_DIR_NAME),
            base.join(LEGACY_CACHE_DIR_NAME),
            CACHE_MARKERS,
        )
    })
    .clone()
}

fn prepare_app_dir(new_dir: PathBuf, old_dir: PathBuf, markers: &[&str]) -> PathBuf {
    if paths_refer_to_same_location(&new_dir, &old_dir) {
        let _ = restore_desired_casing(&new_dir);
        return new_dir;
    }
    if needs_migration(&new_dir, &old_dir, markers) {
        // Copy then use the new directory so a failed migration cannot delete data.
        let _ = copy_tree(&old_dir, &new_dir);
    }
    let _ = restore_desired_casing(&new_dir);
    new_dir
}

fn needs_migration(new_dir: &Path, old_dir: &Path, markers: &[&str]) -> bool {
    old_dir.is_dir()
        && markers
            .iter()
            .any(|name| old_dir.join(name).exists() && !new_dir.join(name).exists())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    let mut first_error = None;
    let entries = match fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }
        };
        let src = entry.path();
        let dest = to.join(entry.file_name());
        let metadata = match fs::symlink_metadata(&src) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        let result = if metadata.is_dir() {
            copy_tree(&src, &dest)
        } else if metadata.is_file() {
            copy_file_if_absent(&src, &dest)
        } else {
            Ok(())
        };
        if let Err(error) = result
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn copy_file_if_absent(from: &Path, to: &Path) -> io::Result<()> {
    if to.exists() {
        return Ok(());
    }
    match fs::copy(from, to) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn paths_refer_to_same_location(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (fs::canonicalize(left), fs::canonicalize(right)) {
        return left == right;
    }
    #[cfg(windows)]
    {
        fn key(path: &Path) -> Option<String> {
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().ok()?.join(path)
            };
            Some(
                resolved
                    .to_string_lossy()
                    .replace('/', "\\")
                    .to_ascii_lowercase(),
            )
        }
        key(left)
            .zip(key(right))
            .is_some_and(|(left, right)| left == right)
    }
    #[cfg(not(windows))]
    false
}

fn restore_desired_casing(path: &Path) -> io::Result<()> {
    let desired = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "missing directory name"))?;
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let Some(actual) = on_disk_file_name(parent, desired) else {
        return Ok(());
    };
    if actual == desired {
        return Ok(());
    }
    let actual_path = parent.join(&actual);
    let staging = parent.join(format!(".{desired}.casing-tmp"));
    fs::rename(&actual_path, &staging)?;
    if let Err(error) = fs::rename(&staging, parent.join(desired)) {
        let _ = fs::rename(&staging, &actual_path);
        return Err(error);
    }
    Ok(())
}

fn on_disk_file_name(parent: &Path, desired: &str) -> Option<std::ffi::OsString> {
    fs::read_dir(parent)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .find(|name| name.eq_ignore_ascii_case(desired))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(path: &Path, contents: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn copies_legacy_tree_when_new_dir_is_missing_expected_files() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("app.ralgrum.player");
        let new = temp.path().join("ralgruM");
        write_file(&old.join("auth_session.json"), b"session");
        write_file(&old.join("nested").join("keep.txt"), b"keep");

        let prepared = prepare_app_dir(new.clone(), old.clone(), CONFIG_MARKERS);
        assert_eq!(prepared, new);
        assert_eq!(fs::read(new.join("auth_session.json")).unwrap(), b"session");
        assert_eq!(
            fs::read(new.join("nested").join("keep.txt")).unwrap(),
            b"keep"
        );
        assert_eq!(fs::read(old.join("auth_session.json")).unwrap(), b"session");
    }

    #[test]
    fn later_config_migration_cannot_resurrect_cleaned_session_plaintext() {
        use super::super::account_session::SessionStore;

        let temp = TempDir::new().unwrap();
        let old = temp.path().join("app.ralgrum.player");
        let new = temp.path().join("ralgruM");
        let legacy_files = [
            "auth_session.json",
            "auth_session.backup.json",
            "deezer_sid.txt",
        ];
        for file_name in legacy_files {
            write_file(&old.join(file_name), br#"{"murglar":"legacy-token"}"#);
        }

        prepare_app_dir(new.clone(), old.clone(), CONFIG_MARKERS);
        let store = SessionStore::load_in_directories(&new, Some(&old)).unwrap();
        assert_eq!(store.session().murglar(), "legacy-token");

        // A later migration is triggered by an unrelated missing configuration.
        write_file(&old.join("general_settings.json"), b"keep-settings");
        prepare_app_dir(new.clone(), old.clone(), CONFIG_MARKERS);

        assert_eq!(
            fs::read(new.join("general_settings.json")).unwrap(),
            b"keep-settings"
        );
        for directory in [&new, &old] {
            for file_name in legacy_files {
                assert!(!directory.join(file_name).exists());
            }
        }
        // Both encrypted files can still independently recover the session.
        fs::remove_file(new.join("auth_session.dat")).unwrap();
        assert_eq!(
            SessionStore::load(&new).unwrap().session().murglar(),
            "legacy-token"
        );
        fs::remove_file(new.join("auth_session.backup.dat")).unwrap();
        assert_eq!(
            SessionStore::load(&new).unwrap().session().murglar(),
            "legacy-token"
        );
    }

    #[test]
    fn does_not_overwrite_existing_new_files() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        write_file(&old.join("auth_session.json"), b"old-session");
        write_file(&old.join("window_state.json"), b"old-window");
        write_file(&new.join("auth_session.json"), b"new-session");

        prepare_app_dir(new.clone(), old, CONFIG_MARKERS);
        assert_eq!(
            fs::read(new.join("auth_session.json")).unwrap(),
            b"new-session"
        );
        assert_eq!(
            fs::read(new.join("window_state.json")).unwrap(),
            b"old-window"
        );
    }

    #[test]
    fn skips_copy_when_legacy_dir_is_absent() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("missing");
        let new = temp.path().join("ralgruM");
        let prepared = prepare_app_dir(new.clone(), old, CONFIG_MARKERS);
        assert_eq!(prepared, new);
        assert!(!new.exists());
    }

    #[test]
    fn copies_cache_subtrees_into_an_existing_new_dir() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("legacy-cache");
        let new = temp.path().join("ralgruM");
        write_file(&old.join("artwork-v1").join("cover.image"), b"art");
        write_file(&old.join("library-v1").join("tracks.json"), b"tracks");
        write_file(&new.join("logs").join("gpui.log"), b"log");

        prepare_app_dir(new.clone(), old, CACHE_MARKERS);
        assert_eq!(
            fs::read(new.join("artwork-v1").join("cover.image")).unwrap(),
            b"art"
        );
        assert_eq!(
            fs::read(new.join("library-v1").join("tracks.json")).unwrap(),
            b"tracks"
        );
        assert_eq!(fs::read(new.join("logs").join("gpui.log")).unwrap(), b"log");
    }

    #[test]
    fn does_not_copy_a_directory_into_itself() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("ralgrum");
        let new = temp.path().join("ralgruM");
        write_file(&old.join("artwork-v1").join("cover.image"), b"art");

        prepare_app_dir(new.clone(), old.clone(), CACHE_MARKERS);
        assert!(
            old.join("artwork-v1").join("cover.image").exists()
                || new.join("artwork-v1").join("cover.image").exists()
        );
        assert!(!new.join("ralgruM").join("artwork-v1").exists());
        assert!(!new.join("ralgrum").join("artwork-v1").exists());
    }

    #[cfg(windows)]
    #[test]
    fn restores_user_facing_folder_casing() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("ralgrum");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("keep.txt"), b"x").unwrap();
        let new = temp.path().join("ralgruM");

        prepare_app_dir(new, old, &["keep.txt"]);
        assert_eq!(
            on_disk_file_name(temp.path(), "ralgruM").as_deref(),
            Some(std::ffi::OsStr::new("ralgruM"))
        );
    }
}
