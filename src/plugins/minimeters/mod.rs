mod discovery;
mod host;
mod launcher;
pub(crate) mod tap;
mod ui;

use super::PluginDefinition;
use gpui::App;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

pub(super) enum HostCommand {
    OpenEditor {
        parent_hwnd: isize,
        scale: f64,
        response: futures::channel::oneshot::Sender<Result<(u32, u32), String>>,
    },
    CloseEditor {
        response: futures::channel::oneshot::Sender<()>,
    },
}

pub(super) const DEFINITION: PluginDefinition = PluginDefinition {
    id: "minimeters",
    name: "MiniMeters Integration",
    icon: crate::assets::LocalIcon::WaveSquare,
    description: "Send playback audio to MiniMeters. Set Audio Source in MiniMeters to Audio Server (Plug-In).",
    validate_enable,
    on_enable: enable,
    on_disable: disable,
    open_ui: Some(open_ui),
    open_settings: None,
};

struct Worker {
    stop: Arc<AtomicBool>,
    commands: mpsc::Sender<HostCommand>,
    thread: std::thread::JoinHandle<()>,
}

static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();

fn validate_enable() -> Result<(), String> {
    let path = discovery::audio_server_path()?;
    launcher::validate_installation()?;
    host::validate_audio_server(&path)
}

fn enable(_: &mut App) {
    let Ok(path) = discovery::audio_server_path() else {
        return;
    };
    let state = tap::shared();
    let mut worker = WORKER.get_or_init(|| Mutex::new(None)).lock().unwrap();
    let previous = worker.take();
    if let Some(previous) = &previous {
        previous.stop.store(true, Ordering::Release);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let (commands, command_receiver) = mpsc::channel();
    let commands_for_worker = commands.clone();
    let thread = std::thread::Builder::new()
        .name("minimeters-clap-host".into())
        .spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.thread.join();
            }
            if stop_for_thread.load(Ordering::Acquire) {
                return;
            }
            if let Err(error) = launcher::launch_if_needed() {
                crate::diagnostics::event("WARN", format!("MiniMeters launch failed: {error}"));
            }
            if let Err(error) = host::run(&path, state, stop_for_thread, command_receiver) {
                crate::diagnostics::event("ERROR", format!("MiniMeters CLAP host failed: {error}"));
            }
        });
    match thread {
        Ok(thread) => {
            tap::shared().enabled.store(true, Ordering::Release);
            *worker = Some(Worker {
                stop,
                commands: commands_for_worker,
                thread,
            });
        }
        Err(error) => {
            crate::diagnostics::event("ERROR", format!("MiniMeters host thread failed: {error}"))
        }
    }
}

fn disable(cx: &mut App) {
    tap::shared().enabled.store(false, Ordering::Release);
    tap::clear();
    ui::close(cx);
    if let Some(worker) = WORKER.get().and_then(|worker| worker.lock().ok())
        && let Some(worker) = worker.as_ref()
    {
        worker.stop.store(true, Ordering::Release);
    }
}

fn open_ui(_: &mut gpui::Window, cx: &mut App) {
    ui::open(cx);
}

fn command_sender() -> Option<mpsc::Sender<HostCommand>> {
    WORKER
        .get()
        .and_then(|worker| worker.lock().ok())
        .and_then(|worker| {
            worker
                .as_ref()
                .filter(|worker| !worker.stop.load(Ordering::Acquire))
                .map(|worker| worker.commands.clone())
        })
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    #[ignore = "checks the user's installed MiniMeters and Audio Server CLAP"]
    fn installed_minimeters_passes_enable_validation() {
        super::validate_enable().unwrap();
    }
}
