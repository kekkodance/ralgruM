use std::path::{Path, PathBuf};

const ASSET_ENV: &str = "RALGRUM_UPDATE_TEST_ASSET";

pub(super) fn candidate_path() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let candidate = PathBuf::from(std::env::var_os(ASSET_ENV)?);
    valid_candidate(&executable, &candidate, &std::env::temp_dir()).then_some(candidate)
}

fn valid_candidate(executable: &Path, candidate: &Path, temp: &Path) -> bool {
    let Some(directory) = executable.parent() else {
        return false;
    };
    let Some(name) = directory.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(id) = name.strip_prefix("ralgrum-updater-test-") else {
        return false;
    };
    uuid::Uuid::parse_str(id).is_ok()
        && executable
            .file_name()
            .is_some_and(|value| value == "ralgruM.exe")
        && candidate == directory.join("candidate.exe")
        && directory.parent().and_then(|path| path.canonicalize().ok()) == temp.canonicalize().ok()
        && executable.is_file()
        && candidate.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_requires_an_isolated_temp_directory_and_sibling_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp
            .path()
            .join(format!("ralgrum-updater-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("ralgruM.exe");
        let candidate = directory.join("candidate.exe");
        std::fs::write(&executable, b"old").unwrap();
        std::fs::write(&candidate, b"new").unwrap();
        assert!(valid_candidate(&executable, &candidate, temp.path()));
        assert!(!valid_candidate(
            &executable,
            &directory.join("outside.exe"),
            temp.path()
        ));
        assert!(!valid_candidate(&executable, &candidate, &directory));
    }
}
