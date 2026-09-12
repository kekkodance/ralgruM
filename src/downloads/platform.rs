use std::{path::Path, process::Command};

pub(crate) fn open_downloads_folder(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("The downloads folder could not be created: {error}"))?;
    open_path(path)
}

pub(crate) fn reveal_file(path: &Path) -> Result<(), String> {
    let path = path
        .canonicalize()
        .map_err(|_| "The completed file could not be revealed".to_string())?;
    if !path.is_file() {
        return Err("The completed file could not be revealed".into());
    }
    #[cfg(target_os = "windows")]
    {
        let (executable, select_argument, path) = windows_reveal_args(&path);
        Command::new(executable)
            .arg(select_argument)
            .arg(path)
            .spawn()
            .map_err(|_| "The completed file could not be revealed".to_string())
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map_err(|_| "The completed file could not be revealed".to_string())
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Mirrors the Tauri app: reveal by opening the containing folder
        // in the desktop file manager via xdg-open.
        let parent = path
            .parent()
            .ok_or_else(|| "The completed file could not be revealed".to_string())?;
        Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|_| "The completed file could not be revealed".to_string())
            .map(|_| ())
    }
}

fn windows_reveal_args(path: &Path) -> (&'static str, &'static str, &Path) {
    ("explorer.exe", "/select,", path)
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(path).spawn();
    result
        .map(|_| ())
        .map_err(|_| "The downloads folder could not be opened".to_string())
}

#[cfg(test)]
mod tests {
    use super::windows_reveal_args;
    use std::path::Path;

    #[test]
    fn windows_reveal_args_keep_paths_with_spaces_as_one_argument() {
        let path = Path::new(r"C:\Users\Test User\Music\Track with spaces.mp3");

        assert_eq!(windows_reveal_args(path).0, "explorer.exe");
        assert_eq!(windows_reveal_args(path).1, "/select,");
        assert_eq!(windows_reveal_args(path).2, path);
    }
}
