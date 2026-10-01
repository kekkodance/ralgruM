//! A dedicated thread that owns the ASIO driver's COM lifecycle.
//!
//! The MiniFuse ASIO driver DLL is registered with
//! `ThreadingModel = Apartment`: it is an STA COM server whose object only
//! works correctly when created, used, and released from one thread that
//! has initialized COM. Opening the driver from arbitrary tokio blocking
//! workers (no COM apartment) fails instantly, and releasing it from a
//! thread other than its creator wedges the DLL for the rest of the
//! process: every later load returns "could not be loaded" until restart.
//!
//! Every ASIO COM operation (open stream, drop stream/engine) therefore
//! runs here, on the single dedicated thread created at first use. The
//! thread initializes COM with `CoInitializeEx(STA)` and pumps its message
//! queue while idle, as STA threads that host Apartment-threaded objects
//! must.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

/// Requests executed on the dedicated ASIO thread.
enum AsioRequest {
    /// Opens an output stream on the named ASIO driver.
    Open {
        driver: String,
        /// Reports the opened stream back to the caller.
        reply: Sender<Result<rodio::OutputStream, String>>,
    },
    /// Drops streams on the owner thread so their COM release happens there.
    Drop { streams: Vec<rodio::OutputStream> },
    /// Drops streams and acknowledges completion for callers that must know
    /// the release finished, such as the system-suspend teardown.
    DropAndAck {
        streams: Vec<rodio::OutputStream>,
        ack: Sender<()>,
    },
}

struct AsioOwner {
    sender: Sender<AsioRequest>,
}

impl AsioOwner {
    fn sender(&self) -> Sender<AsioRequest> {
        self.sender.clone()
    }
}

static ASIO_OWNER: std::sync::OnceLock<AsioOwner> = std::sync::OnceLock::new();

#[cfg(windows)]
fn owner_sender() -> Sender<AsioRequest> {
    ASIO_OWNER
        .get_or_init(|| {
            let (sender, receiver) = channel::<AsioRequest>();
            thread::Builder::new()
                .name("asio-com-owner".into())
                .spawn(move || owner_thread(receiver))
                .expect("failed to start the ASIO owner thread");
            AsioOwner { sender }
        })
        .sender()
}

#[cfg(not(windows))]
fn owner_sender() -> Sender<AsioRequest> {
    unreachable!("the ASIO owner thread is Windows only")
}

/// The owner thread body: initializes COM and serves requests forever.
#[cfg(windows)]
fn owner_thread(receiver: Receiver<AsioRequest>) {
    // Apartment model: the driver DLL requires an STA, matching the thread
    // that CoCreateInstance runs on. The result is ignored because COM may
    // already be initialized for this thread (RPC_E_CHANGED_MODE), which is
    // fine: the apartment is then already the right shape.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }
    while let Ok(request) = receiver.recv() {
        match request {
            AsioRequest::Open { driver, reply } => {
                let result = open_stream_on_this_thread(&driver);
                let _ = reply.send(result);
            }
            AsioRequest::Drop { streams } => {
                drop(streams);
            }
            AsioRequest::DropAndAck { streams, ack } => {
                drop(streams);
                let _ = ack.send(());
            }
        }
        // STA threads that create Apartment COM objects must pump messages
        // so the objects can marshal calls when other threads touch them.
        pump_messages();
    }
}

/// Drains any pending window messages without blocking. STA threads that
/// create Apartment COM objects must pump so the objects can marshal calls.
#[cfg(windows)]
fn pump_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW};
    let mut msg = MSG::default();
    unsafe { while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {} }
}

/// Opens the ASIO output stream on the calling (owner) thread.
#[cfg(windows)]
fn open_stream_on_this_thread(driver: &str) -> Result<rodio::OutputStream, String> {
    let device = crate::playback::asio_drivers::find_asio_driver(driver)?;
    rodio::OutputStreamBuilder::from_device(device)
        .and_then(|builder| {
            builder
                .with_error_callback(crate::playback::engine::log_output_stream_error)
                .open_stream_or_fallback()
        })
        .map_err(|error| format!("The ASIO driver \"{driver}\" could not be opened: {error}"))
}

/// Opens an ASIO output stream on the dedicated owner thread. Blocks the
/// caller until the open finishes or fails.
pub(crate) fn open_asio_stream(driver: &str) -> Result<rodio::OutputStream, String> {
    let (reply_tx, reply_rx) = channel();
    owner_sender()
        .send(AsioRequest::Open {
            driver: driver.to_string(),
            reply: reply_tx,
        })
        .map_err(|_| "The ASIO owner thread stopped unexpectedly".to_string())?;
    reply_rx
        .recv()
        .map_err(|_| "The ASIO owner thread stopped unexpectedly".to_string())?
}

/// Drops ASIO output streams on the dedicated owner thread, moving their
/// COM release there. Returns immediately; the drop order is preserved
/// relative to other requests on the owner thread.
pub(crate) fn drop_asio_streams(streams: Vec<rodio::OutputStream>) {
    let _ = owner_sender().send(AsioRequest::Drop { streams });
}

/// Drops ASIO output streams on the owner thread and blocks until the COM
/// release completed. System suspend needs this: the teardown must finish
/// before the OS freezes all threads, or the DLL is left half-exited and
/// every later load fails until restart.
pub(crate) fn drop_asio_streams_blocking(streams: Vec<rodio::OutputStream>) {
    let (ack_tx, ack_rx) = channel();
    let request = AsioRequest::DropAndAck {
        streams,
        ack: ack_tx,
    };
    if owner_sender().send(request).is_ok() {
        let _ = ack_rx.recv();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asio_thread_handles_requests_without_asio_drivers() {
        // Without the MiniFuse driver installed (CI), an open must return
        // a clean error instead of hanging or panicking.
        let result = open_asio_stream("Definitely Not Installed ASIO Driver");
        assert!(result.is_err());
    }
}
