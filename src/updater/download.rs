use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use futures::StreamExt as _;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::release::Release;

pub(crate) struct Downloaded {
    pub(crate) staged_exe: PathBuf,
    pub(crate) staging_dir: PathBuf,
    cleanup_on_drop: bool,
}

impl Downloaded {
    pub(crate) fn into_paths(mut self) -> (PathBuf, PathBuf) {
        self.cleanup_on_drop = false;
        (self.staged_exe.clone(), self.staging_dir.clone())
    }
}

impl Drop for Downloaded {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            let _ = std::fs::remove_dir_all(&self.staging_dir);
        }
    }
}

pub(crate) async fn download(
    client: &reqwest::Client,
    release: &Release,
    destination: &Path,
    cancellation: &CancellationToken,
    progress: impl FnMut(u64) + Send,
) -> Result<Downloaded, String> {
    let install_dir = destination
        .parent()
        .ok_or("The executable has no parent directory")?;
    let staging_dir = install_dir.join(format!(".ralgruM-update-{}", Uuid::new_v4()));
    tokio::fs::create_dir(&staging_dir)
        .await
        .map_err(|error| format!("Cannot prepare the update beside ralgruM.exe: {error}"))?;
    let staged_exe = staging_dir.join("ralgruM.exe");
    let result = download_into(client, release, &staged_exe, cancellation, progress).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
    }
    result.map(|()| Downloaded {
        staged_exe,
        staging_dir,
        cleanup_on_drop: true,
    })
}

async fn download_into(
    client: &reqwest::Client,
    release: &Release,
    path: &Path,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(u64),
) -> Result<(), String> {
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Update download was cancelled".into()),
        response = client.get(release.asset_url.clone()).timeout(Duration::from_secs(300)).send() => {
            response.map_err(|error| format!("Could not download the update: {error}"))?
        }
    };
    if !response.status().is_success() {
        return Err(format!(
            "Update download returned HTTP {}",
            response.status()
        ));
    }
    if let Some(length) = response.content_length()
        && length != release.size
    {
        return Err("The update download size does not match the GitHub release".into());
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|error| format!("Could not stage the update: {error}"))?;
    let mut stream = response.bytes_stream();
    let mut hash = Sha256::new();
    let mut received = 0_u64;
    loop {
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err("Update download was cancelled".into()),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.map_err(|error| format!("Update download interrupted: {error}"))?;
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or("Update download is too large")?;
        if received > release.size {
            return Err("The update download exceeds its declared size".into());
        }
        hash.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Could not write the update: {error}"))?;
        progress(received);
    }
    file.flush()
        .await
        .map_err(|error| format!("Could not flush the update: {error}"))?;
    file.sync_all()
        .await
        .map_err(|error| format!("Could not sync the update: {error}"))?;
    if received != release.size || hash.finalize().as_slice() != release.digest {
        return Err("The update failed the GitHub SHA-256 and size verification".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Read as _, Write as _},
        net::TcpListener,
        thread,
    };

    use semver::Version;
    use url::Url;

    use super::*;

    fn serve(body: &'static [u8]) -> (Url, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!(
            "http://{}/ralgruM.exe",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn verified_download_accepts_exact_bytes_and_rejects_a_wrong_digest() {
        let body = b"test executable";
        let (url, server) = serve(body);
        let release = Release {
            version: Version::parse("0.6.0").unwrap(),
            asset_url: url,
            page_url: Url::parse("https://github.com/kekkodance/ralgruM/releases/tag/v0.6.0")
                .unwrap(),
            size: body.len() as u64,
            digest: Sha256::digest(body).into(),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ralgruM.exe");
        download_into(
            &reqwest::Client::new(),
            &release,
            &path,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(fs::read(path).unwrap(), body);

        let (url, server) = serve(body);
        let invalid = Release {
            asset_url: url,
            digest: [0; 32],
            ..release
        };
        let path = dir.path().join("invalid.exe");
        assert!(
            download_into(
                &reqwest::Client::new(),
                &invalid,
                &path,
                &CancellationToken::new(),
                |_| {}
            )
            .await
            .is_err()
        );
        server.join().unwrap();
    }

    #[tokio::test]
    async fn cancelled_download_removes_its_staging_directory() {
        let dir = tempfile::tempdir().unwrap();
        let release = Release {
            version: Version::parse("0.6.0").unwrap(),
            asset_url: Url::parse("http://127.0.0.1:1/ralgruM.exe").unwrap(),
            page_url: Url::parse("https://github.com/kekkodance/ralgruM/releases/tag/v0.6.0")
                .unwrap(),
            size: 10,
            digest: [0; 32],
        };
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(
            download(
                &reqwest::Client::new(),
                &release,
                &dir.path().join("ralgruM.exe"),
                &cancellation,
                |_| {}
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
