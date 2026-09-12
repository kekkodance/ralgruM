use std::{io, path::Path};

use tokio::fs;

#[derive(Debug)]
pub(crate) enum FinalizeError {
    DestinationExists,
    Io(io::Error),
}

impl FinalizeError {
    pub(crate) fn into_io(self) -> io::Error {
        match self {
            Self::DestinationExists => io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the destination already exists",
            ),
            Self::Io(error) => error,
        }
    }
}

/// Move a completed partial download into place.
///
/// The partial file is only finalized after the caller has finished writing
/// it. Replacing an existing destination therefore leaves the old file
/// untouched for the entire transfer and swaps it only once the new file is
/// complete.
pub(crate) async fn finalize_download(
    part: &Path,
    destination: &Path,
    replace_existing: bool,
) -> Result<(), FinalizeError> {
    if replace_existing {
        replace_existing_file(part, destination).await
    } else {
        create_new_file(part, destination).await
    }
}

async fn create_new_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    // Reserve the destination before copying so a normal download never
    // replaces a file which appeared after the initial collision check.
    let reservation = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                FinalizeError::DestinationExists
            } else {
                FinalizeError::Io(error)
            }
        })?;
    drop(reservation);
    if let Err(error) = fs::copy(part, destination).await {
        let _ = fs::remove_file(destination).await;
        return Err(FinalizeError::Io(error));
    }
    fs::remove_file(part).await.map_err(FinalizeError::Io)
}

#[cfg(not(windows))]
async fn replace_existing_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    fs::rename(part, destination)
        .await
        .map_err(FinalizeError::Io)
}

#[cfg(windows)]
async fn replace_existing_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    let part = part.to_owned();
    let destination = destination.to_owned();
    let result =
        tokio::task::spawn_blocking(move || replace_existing_file_sync(&part, &destination))
            .await
            .map_err(|error| {
                FinalizeError::Io(io::Error::other(format!(
                    "replacement worker stopped: {error}"
                )))
            })?;
    result.map_err(FinalizeError::Io)
}

#[cfg(windows)]
fn replace_existing_file_sync(part: &Path, destination: &Path) -> io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let part = part
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    // SAFETY: both buffers are NUL-terminated UTF-16 strings which remain
    // alive for the duration of the call.
    let moved = unsafe { MoveFileExW(part.as_ptr(), destination.as_ptr(), flags) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn replacement_swaps_only_after_the_part_exists() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("track.mp3");
        let part = directory.path().join("track.mp3.part");
        fs::write(&destination, b"old").await.unwrap();
        fs::write(&part, b"new").await.unwrap();

        finalize_download(&part, &destination, true).await.unwrap();

        assert_eq!(fs::read(&destination).await.unwrap(), b"new");
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn failed_replacement_preserves_the_existing_file() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("track.mp3");
        let part = directory.path().join("missing.mp3.part");
        fs::write(&destination, b"old").await.unwrap();

        assert!(finalize_download(&part, &destination, true).await.is_err());
        assert_eq!(fs::read(&destination).await.unwrap(), b"old");
    }

    #[tokio::test]
    async fn a_new_collision_preserves_both_destination_and_completed_part() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("track.mp3");
        let part = directory.path().join("track.mp3.part");
        fs::write(&destination, b"old").await.unwrap();
        fs::write(&part, b"new").await.unwrap();

        assert!(matches!(
            finalize_download(&part, &destination, false).await,
            Err(FinalizeError::DestinationExists)
        ));
        assert_eq!(fs::read(&destination).await.unwrap(), b"old");
        assert_eq!(fs::read(&part).await.unwrap(), b"new");
    }
}
