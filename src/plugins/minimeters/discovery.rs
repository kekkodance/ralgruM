use std::{env, path::PathBuf};

pub(super) const MISSING_MESSAGE: &str = "The CLAP plugin was not found. Make sure MiniMeters is installed and the CLAP plugin was selected in the installer.";

/// Locates the MiniMeters Audio Server CLAP bundle.
///
/// Windows offers a shared CLAP folder layout through the installer, while the
/// Linux build ships its CLAP bundle inside the MiniMeters install prefix
/// (`~/.local/share/MiniMeters` or a `/usr/lib|minimeters` directory).
#[cfg(windows)]
pub(super) fn audio_server_path() -> Result<PathBuf, String> {
    let mut roots = Vec::new();
    if let Some(common) = env::var_os("COMMONPROGRAMFILES") {
        roots.push(PathBuf::from(common).join("CLAP"));
    }
    if let Some(program_files) = env::var_os("ProgramFiles") {
        roots.push(
            PathBuf::from(program_files)
                .join("Common Files")
                .join("CLAP"),
        );
    }
    roots.push(PathBuf::from(r"C:\Program Files\Common Files\CLAP"));
    find_in_roots(roots).ok_or_else(|| MISSING_MESSAGE.into())
}

/// Locates the MiniMeters Audio Server CLAP bundle on Linux.
///
/// The Linux installer keeps everything under one MiniMeters prefix, so the
/// probe is a case-insensitive scan of each prefix for the Audio Server
/// bundle. The case variants exist because different MiniMeters releases
/// have shipped `.clap`, `.Clap`, and `.CLAP` spellings.
#[cfg(target_os = "linux")]
pub(super) fn audio_server_path() -> Result<PathBuf, String> {
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
    find_in_prefixes(roots).ok_or_else(|| MISSING_MESSAGE.into())
}

#[cfg(windows)]
fn find_in_roots(roots: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for root in roots {
        let direct = root.join("MiniMeters - Audio-Server.clap");
        let nested = root
            .join("MiniMeters")
            .join("MiniMeters - Audio-Server.clap");
        for path in [direct, nested] {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Returns the first prefix that contains a file matching the Audio Server
/// bundle name in any case combination.
#[cfg(target_os = "linux")]
fn find_in_prefixes(roots: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && matches_audio_server_name(&path) {
                return Some(path);
            }
        }
    }
    None
}

/// Matches `MiniMeters - Audio-Server` (any case) with a `.clap`-family
/// extension. Pure so the name rules stay testable without the filesystem.
#[cfg(target_os = "linux")]
fn matches_audio_server_name(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some((stem, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    stem.eq_ignore_ascii_case("MiniMeters - Audio-Server") && extension.eq_ignore_ascii_case("clap")
}

#[cfg(test)]
mod tests {
    use super::MISSING_MESSAGE;

    #[cfg(windows)]
    #[test]
    fn finds_only_audio_server_clap_in_supported_layouts() {
        use super::find_in_roots;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("CLAP");
        let mini_meters = root.join("MiniMeters");
        std::fs::create_dir_all(&mini_meters).unwrap();
        std::fs::write(mini_meters.join("MiniMeters.clap"), b"other plugin").unwrap();
        assert!(find_in_roots([root.clone()]).is_none());
        let audio_server = mini_meters.join("MiniMeters - Audio-Server.clap");
        std::fs::write(&audio_server, b"test marker").unwrap();
        assert_eq!(find_in_roots([root]), Some(audio_server));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_only_audio_server_clap_in_supported_prefixes() {
        use super::find_in_prefixes;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("prefix");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("MiniMeters.clap"), b"other plugin").unwrap();
        assert!(find_in_prefixes([root.clone()]).is_none());
        let audio_server = root.join("MiniMeters - Audio-Server.clap");
        std::fs::write(&audio_server, b"test marker").unwrap();
        assert_eq!(find_in_prefixes([root]), Some(audio_server));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prioritizes_earlier_prefixes_and_ignores_unreadable_roots() {
        use super::find_in_prefixes;
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let lower_priority = second.join("minimeters - audio-server.Clap");
        std::fs::write(&lower_priority, b"later prefix").unwrap();
        // An unreadable (missing) root must not abort the scan.
        let missing = temp.path().join("missing");
        let top_priority = first.join("MINIMETERS - AUDIO-SERVER.CLAP");
        std::fs::write(&top_priority, b"earlier prefix").unwrap();
        assert_eq!(
            find_in_prefixes([missing, first, second]),
            Some(top_priority)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn matches_case_insensitive_clap_variants() {
        use super::matches_audio_server_name;
        use std::path::Path;
        assert!(matches_audio_server_name(Path::new(
            "/usr/lib/minimeters/MiniMeters - Audio-Server.clap"
        )));
        assert!(matches_audio_server_name(Path::new(
            "/usr/lib/minimeters/minimeters - audio-server.Clap"
        )));
        assert!(matches_audio_server_name(Path::new(
            "/usr/lib/minimeters/MINIMETERS - AUDIO-SERVER.CLAP"
        )));
        assert!(!matches_audio_server_name(Path::new(
            "/usr/lib/minimeters/MiniMeters.clap"
        )));
        assert!(!matches_audio_server_name(Path::new(
            "/usr/lib/minimeters/MiniMeters - Audio-Server.exe"
        )));
        assert!(!matches_audio_server_name(Path::new("/usr/lib/minimeters")));
    }

    #[test]
    fn missing_message_explains_install_option() {
        assert!(MISSING_MESSAGE.contains("CLAP plugin was selected in the installer"));
    }
}
