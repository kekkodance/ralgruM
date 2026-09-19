use std::{path::PathBuf, sync::Arc};

use futures::{StreamExt as _, channel::mpsc};
use gpui::{Context, EventEmitter};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use super::{
    download,
    release::{self, Release},
};

pub(crate) enum Status {
    Checking,
    Current,
    Available,
    Downloading { received: u64 },
    Ready,
    Installing,
    Error(String),
}

pub(crate) struct UpdaterModel {
    runtime: Arc<Runtime>,
    client: reqwest::Client,
    pub(crate) status: Status,
    pub(crate) release: Option<Release>,
    staged_exe: Option<PathBuf>,
    staging_dir: Option<PathBuf>,
    cancellation: CancellationToken,
}

enum DownloadEvent {
    Progress(u64),
    Finished(Result<download::Downloaded, String>),
}

impl EventEmitter<()> for UpdaterModel {}

impl UpdaterModel {
    pub(crate) fn has_offer(&self) -> bool {
        self.release.is_some()
            && matches!(
                self.status,
                Status::Available
                    | Status::Downloading { .. }
                    | Status::Ready
                    | Status::Installing
                    | Status::Error(_)
            )
    }

    pub(crate) fn new(runtime: Arc<Runtime>, cx: &mut Context<Self>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("ralgruM/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let url = attempt.url();
                let trusted = url.scheme() == "https"
                    && url.host_str().is_some_and(|host| {
                        host == "github.com"
                            || host == "api.github.com"
                            || host.ends_with(".githubusercontent.com")
                    });
                if trusted && attempt.previous().len() < 8 {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .expect("updater HTTP client configuration is valid");
        let this = Self {
            runtime,
            client,
            status: Status::Checking,
            release: None,
            staged_exe: None,
            staging_dir: None,
            cancellation: CancellationToken::new(),
        };
        this.check(cx);
        this
    }

    pub(crate) fn check(&self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let task = self
            .runtime
            .spawn(async move { release::fetch_latest(&client).await });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|error| Err(error.to_string()));
            this.update(cx, |this, cx| {
                match result {
                    Ok(Some(release)) => {
                        this.release = Some(release);
                        this.status = Status::Available;
                    }
                    Ok(None) => this.status = Status::Current,
                    Err(error) => this.status = Status::Error(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn start_download(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Available | Status::Error(_)) {
            return;
        }
        let Some(release) = self.release.clone() else {
            return;
        };
        let Ok(destination) = std::env::current_exe() else {
            self.status = Status::Error("Cannot find the running executable".into());
            cx.notify();
            return;
        };
        self.status = Status::Downloading { received: 0 };
        cx.notify();
        let client = self.client.clone();
        let (sender, mut receiver) = mpsc::unbounded::<DownloadEvent>();
        let total = release.size;
        let cancellation = self.cancellation.clone();
        let task = self.runtime.spawn(async move {
            let mut last_reported = 0;
            let result =
                download::download(&client, &release, &destination, &cancellation, |received| {
                    if received.saturating_sub(last_reported) >= 1024 * 1024 || received == total {
                        last_reported = received;
                        let _ = sender.unbounded_send(DownloadEvent::Progress(received));
                    }
                })
                .await;
            let _ = sender.unbounded_send(DownloadEvent::Finished(result));
        });
        drop(task);
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                let finished = matches!(event, DownloadEvent::Finished(_));
                if this
                    .update(cx, |this, cx| {
                        match event {
                            DownloadEvent::Progress(received) => {
                                this.status = Status::Downloading { received };
                            }
                            DownloadEvent::Finished(Ok(downloaded)) => {
                                let (staged_exe, staging_dir) = downloaded.into_paths();
                                this.staged_exe = Some(staged_exe);
                                this.staging_dir = Some(staging_dir);
                                this.status = Status::Ready;
                            }
                            DownloadEvent::Finished(Err(error)) => {
                                this.status = Status::Error(error)
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if finished {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn begin_install(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<tokio::task::JoinHandle<Result<(), String>>, String> {
        if !matches!(self.status, Status::Ready) {
            return Err("The update is not ready".into());
        }
        if cfg!(debug_assertions) {
            return Err("Debug builds only download and verify updates. Automatic installation is enabled in release builds.".into());
        }
        let staged = self
            .staged_exe
            .as_ref()
            .ok_or("The verified update is missing")?
            .clone();
        let dir = self
            .staging_dir
            .as_ref()
            .ok_or("The update staging directory is missing")?
            .clone();
        let digest = self
            .release
            .as_ref()
            .ok_or("The release metadata is missing")?
            .digest;
        self.status = Status::Installing;
        cx.notify();
        Ok(self.runtime.spawn_blocking(move || {
            let result = super::helper::start(&staged, &dir, digest);
            if result.is_err() {
                let _ = std::fs::remove_dir_all(&dir);
            }
            result
        }))
    }

    pub(crate) fn handoff_started(&mut self) {
        self.staged_exe = None;
        self.staging_dir = None;
    }

    pub(crate) fn record_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.status = Status::Error(error);
        cx.notify();
    }
}

impl Drop for UpdaterModel {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if !matches!(self.status, Status::Installing)
            && let Some(dir) = self.staging_dir.take()
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
