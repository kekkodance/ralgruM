use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::local_playlist_store::LocalPlaylistError;

const COVERS_DIRECTORY: &str = "covers";
const MAX_COVER_BYTES: usize = 4 * 1024 * 1024;

pub(super) struct WrittenArtwork {
    pub(super) reference: String,
}

pub(super) fn write(
    directory: &Path,
    playlist_id: &str,
    jpeg: &[u8],
) -> Result<WrittenArtwork, LocalPlaylistError> {
    validate_jpeg(jpeg)?;
    if !playlist_id.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err(LocalPlaylistError::InvalidItem);
    }
    let digest = Sha256::digest(jpeg);
    let hash = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let reference = format!("{COVERS_DIRECTORY}/{playlist_id}-{hash}.jpg");
    let root = directory
        .canonicalize()
        .map_err(|_| LocalPlaylistError::Filesystem)?;
    let covers = directory.join(COVERS_DIRECTORY);
    fs::create_dir_all(&covers).map_err(|_| LocalPlaylistError::Filesystem)?;
    let covers = covers
        .canonicalize()
        .map_err(|_| LocalPlaylistError::Filesystem)?;
    if !covers.starts_with(&root) {
        return Err(LocalPlaylistError::Filesystem);
    }
    let path = directory.join(Path::new(&reference));
    if path.is_file() && fs::read(&path).ok().as_deref() == Some(jpeg) {
        return Ok(WrittenArtwork { reference });
    }
    let parent = path.parent().ok_or(LocalPlaylistError::Filesystem)?;
    fs::create_dir_all(parent).map_err(|_| LocalPlaylistError::Filesystem)?;
    write_atomic(&path, jpeg)?;
    Ok(WrittenArtwork { reference })
}

pub(super) fn normalize_reference(playlist_id: &str, value: String) -> String {
    let value = value.trim().replace('\\', "/");
    if value.is_empty() {
        return String::new();
    }
    let path = Path::new(&value);
    let mut components = path.components();
    let valid = matches!(components.next(), Some(Component::Normal(part)) if part == COVERS_DIRECTORY)
        && matches!(components.next(), Some(Component::Normal(file)) if valid_filename(playlist_id, file.to_string_lossy().as_ref()))
        && components.next().is_none();
    if valid { value } else { Default::default() }
}

pub(super) fn resolve(directory: &Path, playlist_id: &str, reference: &str) -> Option<PathBuf> {
    let reference = normalize_reference(playlist_id, reference.to_owned());
    if reference.is_empty() {
        return None;
    }
    let root = directory.canonicalize().ok()?;
    let covers = directory.join(COVERS_DIRECTORY).canonicalize().ok()?;
    if !covers.starts_with(&root) {
        return None;
    }
    let path = directory.join(reference).canonicalize().ok()?;
    (path.starts_with(covers) && path.is_file()).then_some(path)
}

pub(crate) fn resolve_current(playlist_id: &str, reference: &str) -> Option<PathBuf> {
    let directory = crate::app::paths::config_dir()?.join("local_library");
    resolve(&directory, playlist_id, reference)
}

pub(super) fn cleanup(directory: &Path, playlist_id: &str, reference: &str) {
    let Some(path) = resolve(directory, playlist_id, reference) else {
        return;
    };
    let _ = fs::remove_file(path);
}

fn valid_filename(playlist_id: &str, file: &str) -> bool {
    let Some(stem) = file.strip_suffix(".jpg") else {
        return false;
    };
    let Some((id, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    id == playlist_id
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && hash.len() == 24
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_jpeg(bytes: &[u8]) -> Result<(), LocalPlaylistError> {
    if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
        return Err(LocalPlaylistError::InvalidItem);
    }
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|_| LocalPlaylistError::InvalidItem)?;
    if image.width() != 512 || image.height() != 512 {
        return Err(LocalPlaylistError::InvalidItem);
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LocalPlaylistError> {
    let parent = path.parent().ok_or(LocalPlaylistError::Filesystem)?;
    let mut temp_path = None;
    let mut file = None;
    for attempt in 0..16u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("cover"),
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(opened) => {
                temp_path = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(LocalPlaylistError::Filesystem),
        }
    }
    let temp_path = temp_path.ok_or(LocalPlaylistError::Filesystem)?;
    let result = (|| {
        let mut file = file.take().ok_or(LocalPlaylistError::Filesystem)?;
        file.write_all(bytes)
            .map_err(|_| LocalPlaylistError::Filesystem)?;
        file.sync_all()
            .map_err(|_| LocalPlaylistError::Filesystem)?;
        super::local_playlist_store::atomic_replace(&temp_path, path)
            .map_err(|_| LocalPlaylistError::Filesystem)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_reference_rejects_paths_and_unowned_names() {
        assert!(normalize_reference("abc", "../cover.jpg".into()).is_empty());
        assert!(normalize_reference("abc", "C:/cover.jpg".into()).is_empty());
        assert!(normalize_reference("abc", "covers/not-owned.jpg".into()).is_empty());
        assert!(
            normalize_reference("other", "covers/abc-0123456789abcdef01234567.jpg".into())
                .is_empty()
        );
        assert_eq!(
            normalize_reference("abc", "covers/abc-0123456789abcdef01234567.jpg".into()),
            "covers/abc-0123456789abcdef01234567.jpg"
        );
    }
}
