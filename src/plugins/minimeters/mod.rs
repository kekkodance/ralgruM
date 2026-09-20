mod discovery;
mod host;
mod launcher;
pub(crate) mod tap;

use super::PluginDefinition;
use gpui::App;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

pub(super) const DEFINITION: PluginDefinition = PluginDefinition {
    id: "minimeters",
    name: "MiniMeters",
    version: "1.0.0",
    author: "ralgruM",
    description: "Send playback audio to MiniMeters. Set Audio Source in MiniMeters to Audio Server (Plug-In).",
    validate_enable,
    on_enable: enable,
    on_disable: disable,
    open_settings: None,
};

struct Worker {
    stop: Arc<AtomicBool>,
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
            if let Err(error) = host::run(&path, state, stop_for_thread) {
                crate::diagnostics::event("ERROR", format!("MiniMeters CLAP host failed: {error}"));
            }
        });
    match thread {
        Ok(thread) => {
            tap::shared().enabled.store(true, Ordering::Release);
            *worker = Some(Worker { stop, thread });
        }
        Err(error) => {
            crate::diagnostics::event("ERROR", format!("MiniMeters host thread failed: {error}"))
        }
    }
}

fn disable(_: &mut App) {
    tap::shared().enabled.store(false, Ordering::Release);
    tap::clear();
    if let Some(worker) = WORKER.get().and_then(|worker| worker.lock().ok())
        && let Some(worker) = worker.as_ref()
    {
        worker.stop.store(true, Ordering::Release);
    }
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
