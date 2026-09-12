use std::{io, path::Path};

#[cfg(any(not(windows), test))]
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

#[cfg(not(windows))]
async fn create_new_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    // The part and destination are in the same downloads directory. A hard
    // link publishes the completed inode in one operation and fails with
    // AlreadyExists without changing either file. Copying after reserving an
    // empty destination would expose a partially written destination.
    fs::hard_link(part, destination).await.map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            FinalizeError::DestinationExists
        } else {
            FinalizeError::Io(error)
        }
    })?;
    // Publication has succeeded even if cleanup cannot remove the original
    // name. The caller still owns that name and can retry cleanup safely.
    let _ = fs::remove_file(part).await;
    Ok(())
}

#[cfg(windows)]
async fn create_new_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    move_file(part, destination, false).await.map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            FinalizeError::DestinationExists
        } else {
            FinalizeError::Io(error)
        }
    })
}

#[cfg(not(windows))]
async fn replace_existing_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    fs::rename(part, destination)
        .await
        .map_err(FinalizeError::Io)
}

#[cfg(windows)]
async fn replace_existing_file(part: &Path, destination: &Path) -> Result<(), FinalizeError> {
    move_file(part, destination, true)
        .await
        .map_err(FinalizeError::Io)
}

#[cfg(windows)]
async fn move_file(part: &Path, destination: &Path, replace_existing: bool) -> io::Result<()> {
    let part = part.to_owned();
    let destination = destination.to_owned();
    tokio::task::spawn_blocking(move || move_file_sync(&part, &destination, replace_existing))
        .await
        .map_err(|error| {
            io::Error::other(format!("download finalization worker stopped: {error}"))
        })?
}

#[cfg(windows)]
fn move_file_sync(part: &Path, destination: &Path, replace_existing: bool) -> io::Result<()> {
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
    // Both paths share a directory. A rename also works on FAT and exFAT,
    // and omitting REPLACE_EXISTING protects a newly created destination.
    let flags = MOVEFILE_WRITE_THROUGH
        | if replace_existing {
            MOVEFILE_REPLACE_EXISTING
        } else {
            0
        };
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
    async fn new_download_publishes_complete_file_and_consumes_part() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("track.mp3");
        let part = directory.path().join("track.mp3.part");
        fs::write(&part, b"complete audio").await.unwrap();
        assert!(!destination.exists());

        finalize_download(&part, &destination, false).await.unwrap();

        assert_eq!(fs::read(&destination).await.unwrap(), b"complete audio");
        assert!(!part.exists());
    }

    #[tokio::test]
    async fn concurrent_new_downloads_never_overwrite_the_winner() {
        let directory = tempdir().unwrap();
        let destination = directory.path().join("track.mp3");
        let first = directory.path().join("first.part");
        let second = directory.path().join("second.part");
        fs::write(&first, b"first").await.unwrap();
        fs::write(&second, b"second").await.unwrap();

        let (one, two) = tokio::join!(
            finalize_download(&first, &destination, false),
            finalize_download(&second, &destination, false),
        );
        let expected = match (one, two) {
            (Ok(()), Err(FinalizeError::DestinationExists)) => {
                assert_eq!(fs::read(&second).await.unwrap(), b"second");
                b"first".as_slice()
            }
            (Err(FinalizeError::DestinationExists), Ok(())) => {
                assert_eq!(fs::read(&first).await.unwrap(), b"first");
                b"second".as_slice()
            }
            outcomes => panic!("expected one successful finalization: {outcomes:?}"),
        };
        assert_eq!(fs::read(&destination).await.unwrap(), expected);
    }

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
