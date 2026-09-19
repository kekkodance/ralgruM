use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

const HELPER_ARG: &str = "--ralgrum-update-helper";
const CONFIRM_ARG: &str = "--ralgrum-update-confirm";

fn last_error_path() -> PathBuf {
    crate::paths::cache_dir()
        .join("updater")
        .join("last-error.txt")
}

fn report_error(error: &str) {
    let path = last_error_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, error);
}

pub(crate) fn take_last_error() -> Option<String> {
    let path = last_error_path();
    let error = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(path);
    Some(error)
}

#[derive(Serialize, Deserialize)]
struct Transaction {
    target: PathBuf,
    staged: PathBuf,
    staging_dir: PathBuf,
    backup: PathBuf,
    parent_pid: u32,
    digest: [u8; 32],
}

fn transaction_dir(id: Uuid) -> PathBuf {
    crate::paths::cache_dir()
        .join("updater")
        .join(id.to_string())
}

pub(crate) fn start(staged: &Path, staging_dir: &Path, digest: [u8; 32]) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (staged, staging_dir, digest);
        return Err("Automatic installation is currently available only on Windows".into());
    }
    #[cfg(windows)]
    {
        let target = std::env::current_exe().map_err(|error| error.to_string())?;
        let parent = target
            .parent()
            .ok_or("The app executable has no parent directory")?;
        let id = Uuid::new_v4();
        let dir = transaction_dir(id);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("Cannot create update handoff: {error}"))?;
        let transaction = Transaction {
            target: target.clone(),
            staged: staged.to_owned(),
            staging_dir: staging_dir.to_owned(),
            backup: parent.join(format!(".ralgruM-backup-{id}.exe")),
            parent_pid: std::process::id(),
            digest,
        };
        let result = (|| {
            validate(&transaction)?;
            let helper = dir.join("helper.exe");
            fs::copy(&target, &helper)
                .map_err(|error| format!("Cannot copy the update helper: {error}"))?;
            let manifest = serde_json::to_vec(&transaction).map_err(|error| error.to_string())?;
            fs::write(dir.join("transaction.json"), manifest)
                .map_err(|error| format!("Cannot write the update handoff: {error}"))?;
            Command::new(helper)
                .arg(HELPER_ARG)
                .arg(id.to_string())
                .spawn()
                .map_err(|error| format!("Cannot launch the update helper: {error}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        result
    }
}

fn validate(tx: &Transaction) -> Result<(), String> {
    if !tx.target.is_absolute()
        || !tx.staged.is_absolute()
        || !tx.backup.is_absolute()
        || tx
            .target
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().eq_ignore_ascii_case("ralgruM.exe"))
        || tx.staged != tx.staging_dir.join("ralgruM.exe")
        || tx.staging_dir.parent() != tx.target.parent()
        || tx.backup.parent() != tx.target.parent()
        || !tx
            .backup
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".ralgruM-backup-"))
    {
        return Err("The update handoff paths are invalid".into());
    }
    Ok(())
}

pub(crate) fn dispatch() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() == 3 && args[1] == HELPER_ARG {
        if let Err(error) = run_helper(&args[2]) {
            eprintln!("Update helper failed: {error}");
        }
        return true;
    }
    false
}

pub(crate) fn confirmation_id() -> Option<Uuid> {
    let args = std::env::args().collect::<Vec<_>>();
    (args.len() == 3 && args[1] == CONFIRM_ARG)
        .then(|| Uuid::parse_str(&args[2]).ok())
        .flatten()
}

pub(crate) fn confirm(id: Uuid) {
    let dir = transaction_dir(id);
    if let Err(error) = fs::write(dir.join("confirmed"), b"ready") {
        crate::diagnostics::event("WARN", format!("Could not confirm updated launch: {error}"));
    } else {
        std::thread::spawn(move || {
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if dir.join("finished").exists() && fs::remove_dir_all(&dir).is_ok() {
                    break;
                }
            }
        });
    }
}

fn verify(path: &Path, digest: [u8; 32]) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).map_err(|error| error.to_string())?;
    if hash.finalize().as_slice() != digest {
        return Err("The staged executable changed after verification".into());
    }
    Ok(())
}

#[cfg(windows)]
fn replace_file(target: &Path, replacement: &Path, backup: Option<&Path>) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let target = wide(target);
    let replacement = wide(replacement);
    let backup = backup.map(wide);
    let backup_ptr = backup
        .as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr());
    let result = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            backup_ptr,
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        Err(format!(
            "Could not replace ralgruM.exe: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn replace_with_retry(
    target: &Path,
    replacement: &Path,
    backup: Option<&Path>,
) -> Result<(), String> {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match replace_file(target, replacement, backup) {
            Ok(()) => return Ok(()),
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(250)),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn run_helper(raw_id: &str) -> Result<(), String> {
    use std::{
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };

    let id = Uuid::parse_str(raw_id).map_err(|_| "Invalid update handoff ID")?;
    let dir = transaction_dir(id);
    if std::env::current_exe().ok().as_deref() != Some(dir.join("helper.exe").as_path()) {
        return Err("The update helper is running from the wrong location".into());
    }
    let tx: Transaction = serde_json::from_slice(
        &fs::read(dir.join("transaction.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    validate(&tx)?;
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, tx.parent_pid) };
    if handle.is_null() {
        return Err("Cannot wait for the old app process".into());
    }
    let wait = unsafe { WaitForSingleObject(handle, 60_000) };
    unsafe {
        CloseHandle(handle);
    }
    if wait != WAIT_OBJECT_0 {
        return Err("The old app did not exit for the update".into());
    }
    if let Err(error) = verify(&tx.staged, tx.digest) {
        let _ = fs::write(dir.join("error.txt"), &error);
        report_error(&error);
        let _ = Command::new(&tx.target).spawn();
        return Err(error);
    }
    if let Err(error) = replace_with_retry(&tx.target, &tx.staged, Some(&tx.backup)) {
        let _ = fs::write(dir.join("error.txt"), &error);
        report_error(&error);
        let _ = Command::new(&tx.target).spawn();
        return Err(error);
    }
    let mut child = match Command::new(&tx.target)
        .arg(CONFIRM_ARG)
        .arg(id.to_string())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            replace_with_retry(&tx.target, &tx.backup, None)?;
            report_error(&format!("Updated app did not launch: {error}"));
            let _ = Command::new(&tx.target).spawn();
            return Err(format!("Updated app did not launch: {error}"));
        }
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if dir.join("confirmed").exists() {
            let _ = fs::remove_file(&tx.backup);
            let _ = fs::remove_dir_all(&tx.staging_dir);
            let _ = fs::remove_file(dir.join("transaction.json"));
            let _ = fs::write(dir.join("finished"), b"done");
            return Ok(());
        }
        if child
            .try_wait()
            .map_err(|error| error.to_string())?
            .is_some()
        {
            replace_with_retry(&tx.target, &tx.backup, None)?;
            report_error(
                "Updated app exited before its window was ready; restored the previous version",
            );
            let _ = Command::new(&tx.target).spawn();
            return Err(
                "Updated app exited before its window was ready; restored the previous version"
                    .into(),
            );
        }
        if Instant::now() >= deadline {
            let _ = fs::write(
                dir.join("error.txt"),
                "The updated app did not confirm startup. The backup was kept.",
            );
            return Err("Updated app did not confirm startup; backup kept".into());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(not(windows))]
fn run_helper(_: &str) -> Result<(), String> {
    Err("Windows update helper is unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_handoffs_outside_the_install_directory() {
        let root = std::env::current_dir().unwrap();
        let tx = Transaction {
            target: root.join("ralgruM.exe"),
            staged: root.join("elsewhere").join("ralgruM.exe"),
            staging_dir: root.join(".ralgruM-update-test"),
            backup: root.join(".ralgruM-backup-test.exe"),
            parent_pid: 1,
            digest: [0; 32],
        };
        assert!(validate(&tx).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn replaces_and_restores_the_exact_executable_name() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("ralgruM.exe");
        let staged_dir = root.path().join(".ralgruM-update-test");
        fs::create_dir(&staged_dir).unwrap();
        let staged = staged_dir.join("ralgruM.exe");
        let backup = root.path().join(".ralgruM-backup-test.exe");
        fs::write(&target, b"previous").unwrap();
        fs::write(&staged, b"next").unwrap();
        replace_file(&target, &staged, Some(&backup)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"next");
        assert_eq!(fs::read(&backup).unwrap(), b"previous");
        assert!(!staged.exists());
        replace_file(&target, &backup, None).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"previous");
    }
}
