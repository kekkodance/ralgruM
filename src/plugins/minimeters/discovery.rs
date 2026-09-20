use std::{env, path::PathBuf};

pub(super) const MISSING_MESSAGE: &str = "The CLAP plugin was not found. Make sure MiniMeters is installed and the CLAP plugin was selected in the installer.";

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

#[cfg(test)]
mod tests {
    use super::{MISSING_MESSAGE, find_in_roots};

    #[test]
    fn finds_only_audio_server_clap_in_supported_layouts() {
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

    #[test]
    fn missing_message_explains_install_option() {
        assert!(MISSING_MESSAGE.contains("CLAP plugin was selected in the installer"));
    }
}
