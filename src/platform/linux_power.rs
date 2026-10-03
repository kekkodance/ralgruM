//! System suspend and wake notifications from logind.
//!
//! The vendored GPUI Windows backend feeds suspend and wake into the app
//! observers via `WM_POWERBROADCAST`, but the upstream Linux backend (the
//! pinned Zed `gpui_linux` code) has no power notification support at all:
//! the trait methods exist, yet nothing invokes them. This module fills the
//! gap app-side: it subscribes to logind's `PrepareForSleep` D-Bus signal on
//! the system bus and forwards the transitions into the shell through the
//! same `handle_system_suspend` / `handle_system_wake` entry points, so
//! sleeping with audio playing gets the same teardown-and-recover story on
//! Linux that it already has on Windows.

use std::sync::OnceLock;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};

use crate::shell::RalgrumApp;

/// One power transition reported by logind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PowerEvent {
    /// The system is about to suspend: `start == true`.
    Suspend,
    /// The system just resumed: `start == false`.
    Wake,
}

/// Maps the boolean argument of logind's `PrepareForSleep` signal to the
/// corresponding app event. Kept as a pure function so the mapping (and its
/// tolerance of unexpected payload shapes) stays unit-testable without a
/// system bus.
pub(crate) fn prepare_for_sleep_to_event(start: bool) -> PowerEvent {
    if start {
        PowerEvent::Suspend
    } else {
        PowerEvent::Wake
    }
}

/// Guard against a second `install` call spawning a second listener thread
/// (and a second `PrepareForSleep` match rule). The connection itself lives
/// on the background thread, so once the flag is taken there is no shared
/// state left here to hand out.
static INSTALLED: OnceLock<()> = OnceLock::new();

/// Installs the logind listener.
///
/// A dedicated OS thread owns a blocking zbus connection to the system bus
/// for the lifetime of the process and forwards each `PrepareForSleep`
/// transition through an unbounded channel into a GPUI task. The pump runs
/// on the app's async executor and only touches the shell via
/// [`gpui::WeakEntity`], so the GPUI threading rule (app state is main
/// thread only) is respected and a dropped shell simply makes later events
/// no-ops.
///
/// If logind is unreachable or the subscription fails, the error is logged
/// and the thread terminates cleanly: audio keeps playing, only the
/// suspend/wake recovery is lost. Calling `install` more than once is a
/// no-op after the first successful start.
pub(crate) fn install(cx: &mut gpui::App, shell: gpui::WeakEntity<RalgrumApp>) {
    if INSTALLED.set(()).is_err() {
        // A listener is already running; a second connection would only
        // duplicate every event.
        return;
    }

    let (sender, mut receiver) = unbounded::<PowerEvent>();

    spawn_logind_thread(sender);

    cx.spawn(async move |cx| {
        while let Some(event) = receiver.next().await {
            let alive = cx.update(|cx| {
                shell
                    .update(cx, |this, cx| match event {
                        PowerEvent::Wake => this.handle_system_wake(cx),
                        PowerEvent::Suspend => this.handle_system_suspend(cx),
                    })
                    .is_ok()
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

/// Runs the blocking D-Bus subscription on its own named thread.
///
/// The thread is detached on purpose: it must outlive the caller and its
/// only job is to pump signals into the channel, which stays open for the
/// process lifetime. Errors are logged via the diagnostics event log and
/// end the thread; the sender half is then dropped, which closes the
/// channel and lets the GPUI pump finish.
fn spawn_logind_thread(sender: UnboundedSender<PowerEvent>) {
    let result = std::thread::Builder::new()
        .name("ralgrum-logind-power".to_string())
        .spawn(move || run_logind_loop(sender));
    if let Err(error) = result {
        crate::diagnostics::event(
            "WARN",
            format!("failed to spawn the logind power thread: {error}"),
        );
    }
}

/// Connects to the system bus and iterates `PrepareForSleep` until the
/// connection dies. Returns (i.e. ends the thread) on any setup or receive
/// error; see [`install`] for the failure posture.
fn run_logind_loop(sender: UnboundedSender<PowerEvent>) {
    let connection = match zbus::blocking::Connection::system() {
        Ok(connection) => connection,
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "logind power notifications unavailable (system bus: {error}); audio recovery across sleep is disabled"
                ),
            );
            return;
        }
    };

    let logind = match zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    ) {
        Ok(logind) => logind,
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "logind power notifications unavailable (login1 proxy: {error}); audio recovery across sleep is disabled"
                ),
            );
            return;
        }
    };

    let signals = match logind.receive_signal("PrepareForSleep") {
        Ok(signals) => signals,
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!(
                    "logind power notifications unavailable (PrepareForSleep subscription: {error}); audio recovery across sleep is disabled"
                ),
            );
            return;
        }
    };

    for message in signals {
        // `PrepareForSleep` carries a single boolean body member: `start`.
        // A malformed body (for example a future logind revision changing
        // the signature) is skipped rather than fatal; the next well-formed
        // signal still gets through.
        let start = match message.body().deserialize::<bool>() {
            Ok(start) => start,
            Err(error) => {
                crate::diagnostics::event(
                    "WARN",
                    format!("discarding a malformed PrepareForSleep signal: {error}"),
                );
                continue;
            }
        };
        let event = prepare_for_sleep_to_event(start);
        let delivered = sender.unbounded_send(event);
        if delivered.is_err() {
            // The receiving pump is gone, which only happens when the app
            // is shutting down. Nothing left to watch for.
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_prepare_for_sleep_start_true_to_suspend() {
        assert_eq!(prepare_for_sleep_to_event(true), PowerEvent::Suspend);
    }

    #[test]
    fn maps_prepare_for_sleep_start_false_to_wake() {
        assert_eq!(prepare_for_sleep_to_event(false), PowerEvent::Wake);
    }

    #[test]
    fn power_events_are_distinguishable() {
        assert_ne!(PowerEvent::Suspend, PowerEvent::Wake);
    }
}
