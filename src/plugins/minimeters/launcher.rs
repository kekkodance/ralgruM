use std::{env, path::PathBuf, process::Command};

pub(super) fn launch_if_needed() -> Result<(), String> {
    let Some(path) = executable_path() else {
        return Err("MiniMeters.exe was not found".into());
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
    executable_path()
        .map(|_| ())
        .ok_or_else(|| "MiniMeters.exe was not found. Make sure MiniMeters is installed.".into())
}

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

#[cfg(not(windows))]
fn is_running() -> bool {
    false
}
