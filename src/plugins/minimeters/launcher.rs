use std::{env, path::PathBuf, process::Command};

pub(super) fn launch_if_needed() -> Result<(), String> {
    let Some(path) = executable_path() else {
        return Err("The MiniMeters executable was not found".into());
    };
    if is_running() {
        return Ok(());
    }
    Command::new(&path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("could not launch {}: {error}", path.display()))
}

pub(super) fn validate_installation() -> Result<(), String> {
    executable_path().map(|_| ()).ok_or_else(|| {
        "The MiniMeters executable was not found. Make sure MiniMeters is installed.".into()
    })
}

/// Locates the MiniMeters executable.
///
/// Windows installs under Program Files with an `.exe`, while the Linux build
/// names the binary `MiniMeters` inside the same install prefixes the CLAP
/// bundle is probed in.
#[cfg(windows)]
fn executable_path() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(root));
    }
    if let Some(root) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(root));
    }
    roots.push(PathBuf::from(r"C:\Program Files"));
    roots
        .into_iter()
        .map(|root| root.join("MiniMeters").join("MiniMeters.exe"))
        .find(|path| path.is_file())
}

/// Locates the MiniMeters executable on Linux.
#[cfg(target_os = "linux")]
fn executable_path() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        roots.push(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("MiniMeters"),
        );
    }
    roots.push(PathBuf::from("/usr/lib/minimeters"));
    roots.push(PathBuf::from("/usr/share/minimeters"));
    roots
        .into_iter()
        .map(|root| root.join("MiniMeters"))
        .find(|path| path.is_file())
}

#[cfg(windows)]
fn is_running() -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    // SAFETY: The initialized entry buffer is valid for the snapshot enumeration calls.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("MiniMeters.exe")
                {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        found
    }
}

/// Detects a running MiniMeters process on Linux by scanning `/proc/*/comm`.
///
/// `/proc/<pid>/comm` holds the process command name without a path or
/// extension, so the match is the plain `MiniMeters` name. Entries owned by
/// other users (or already-exiting processes) fail to read and are skipped,
/// mirroring the snapshot semantics on Windows where a failed snapshot simply
/// reports "not running".
#[cfg(target_os = "linux")]
fn is_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .chars()
                .all(|c| c.is_ascii_digit())
        })
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm_matches_minimeters(&comm))
        })
}

/// Pure `/proc/<pid>/comm` matcher: the file ends with a newline and holds no
/// path components, so trim and compare the bare process name.
#[cfg(target_os = "linux")]
fn comm_matches_minimeters(comm: &str) -> bool {
    comm.trim().eq_ignore_ascii_case("MiniMeters")
}

#[cfg(target_os = "macos")]
fn is_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn comm_matches_exact_name_only() {
        use super::comm_matches_minimeters;
        assert!(comm_matches_minimeters("MiniMeters\n"));
        assert!(comm_matches_minimeters("minimeters\n"));
        assert!(!comm_matches_minimeters("MiniMeters.exe\n"));
        assert!(!comm_matches_minimeters("MiniMetersHelper\n"));
        assert!(!comm_matches_minimeters("other\n"));
    }
}
