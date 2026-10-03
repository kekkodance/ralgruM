//! The single GTK thread shared by every Linux desktop integration.
//!
//! Two features of the Linux port need GTK in the process: the tray icon
//! (tray-icon's gtk feature rides on libappindicator, and muda's menu
//! items only fire their activation callbacks while a GTK loop iterates)
//! and the sign-in webview (wry's WebKitGTK backend requires `gtk::init`
//! on the thread that creates the webview). The `gtk` crate enforces a
//! single GTK thread per process: it records whichever thread first called
//! `gtk::init` as the main thread and panics when a second thread tries to
//! initialize. A private thread per feature would therefore crash the app,
//! so this module owns exactly one "ralgrum-gtk" thread and every GTK call
//! in the process is dispatched onto it.
//!
//! The pump alternates between two steps forever: run every pending
//! submitted closure, then run one non-blocking GTK iteration so GTK events
//! (menu activation, webview rendering) get dispatched. A short sleep after
//! a fully idle cycle keeps the thread off the CPU when nothing is
//! happening.
//!
//! Closures are `Send` and run entirely on the GTK thread, so widgets
//! created inside a closure stay there for their whole lifetime, which is
//! what GTK's thread affinity requires. Return values travel back through
//! channels the caller owns and the closure closes itself.

use std::sync::{OnceLock, mpsc};

/// How long the pump sleeps when a cycle found no closures, no GTK events,
/// and no idle work. Fifty milliseconds keeps menu and webview
/// interactions feeling instant while letting the thread idle at a tiny
/// fraction of a core on typical hardware.
const IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(50);

/// A job submitted to the GTK thread.
type Job = Box<dyn FnOnce() + Send + 'static>;

/// Handle to the shared GTK thread. Cloning is cheap and every clone
/// submits to the same thread; the thread itself is never joined and ends
/// with the process.
#[derive(Clone)]
pub(crate) struct GtkHostHandle {
    jobs: mpsc::Sender<Job>,
}

impl GtkHostHandle {
    /// Runs the closure on the GTK thread. Never blocks the caller beyond
    /// channel send time; results come back through a channel the closure
    /// closes itself.
    pub(crate) fn submit(&self, job: impl FnOnce() + Send + 'static) {
        if self.jobs.send(Box::new(job)).is_err() {
            crate::diagnostics::event("WARN", "gtk host thread is gone; dropping the job");
        }
    }
}

/// The process-wide GTK thread. The `OnceLock` (rather than a `LazyLock`)
/// is deliberate: starting the host runs a thread spawn plus `gtk::init`,
/// and both failures must reach the caller as an error instead of
/// panicking inside a lazy initializer.
static HOST: OnceLock<GtkHostHandle> = OnceLock::new();

/// Starts the shared GTK thread, or returns a handle to it when it is
/// already running. The returned error only describes a thread spawn or
/// `gtk::init` failure, both of which leave the host unavailable for the
/// rest of the process.
pub(crate) fn spawn() -> Result<GtkHostHandle, String> {
    if let Some(handle) = HOST.get() {
        return Ok(handle.clone());
    }

    // Start a candidate thread. A concurrent caller may be doing the same
    // at this moment; `HOST.set` at the end picks exactly one winner, and
    // the loser's thread keeps running a pump nobody talks to, which is
    // harmless (it sleeps when idle and is never joined).
    let (sender, receiver) = mpsc::channel::<Job>();
    let spawned = std::thread::Builder::new()
        .name("ralgrum-gtk".to_string())
        .spawn(move || run(receiver));
    let handle = GtkHostHandle { jobs: sender };
    let init_result = match spawned {
        Ok(_) => {
            // gtk::init must run on the thread that will own the widgets,
            // and callers must not see success before it finished. This
            // barrier enforces both: the closure below is the first job
            // the pump ever runs, and `recv` blocks the spawning thread
            // until gtk::init's outcome is known.
            let (init_sender, init_receiver) = mpsc::channel::<Result<(), String>>();
            handle.submit(move || {
                // This is the only gtk::init call in the process. Failure
                // means GTK is unusable for this session (missing display,
                // broken theme); every caller degrades individually.
                let result = gtk::init().map_err(|error| format!("gtk::init failed: {error}"));
                let _ = init_sender.send(result);
            });
            init_receiver
                .recv()
                .map_err(|_| "gtk host thread died before gtk::init".to_string())?
        }
        Err(error) => Err(format!("could not spawn the gtk host thread: {error}")),
    };

    // Publish only a successful host. A losing racer re-reads the winner's
    // handle below; a failed racer (spawn error or gtk::init failure)
    // reports the failure to its own caller, and a later retry can still
    // succeed when the failure was transient.
    if init_result.is_ok() && HOST.set(handle).is_ok() {
        // This caller won the publish race, so its initialized host is
        // the process-wide one. Re-reading HOST keeps a single source of
        // truth for the returned handle.
        return Ok(HOST.get().expect("just published").clone());
    }
    match HOST.get() {
        // A concurrent winner already initialized successfully, so this
        // caller's candidate (successful or not) is irrelevant.
        Some(handle) => Ok(handle.clone()),
        None => Err(init_result.unwrap_err()),
    }
}

/// The pump body, running on the GTK thread for the process lifetime.
///
/// Each cycle: drain every submitted closure (so bursts of work run in
/// order and promptly), then run one non-blocking GTK iteration (which is
/// what lets menu activation callbacks and webview events fire). When
/// nothing happened the cycle sleeps briefly, which keeps the thread idle
/// when the desktop is quiet.
fn run(receiver: mpsc::Receiver<Job>) {
    loop {
        let mut worked = false;

        // Drain all pending jobs. The channel is unbounded, so this loop
        // only ends when the queue is momentarily empty; new arrivals wait
        // for the next cycle.
        while let Ok(job) = receiver.try_recv() {
            worked = true;
            job();
        }

        // One non-blocking iteration dispatches whatever GTK queued on the
        // default main context, which is the only context this app uses.
        if gtk::main_iteration_do(false) {
            worked = true;
        }

        if !worked {
            std::thread::sleep(IDLE_SLEEP);
        }
    }
}
