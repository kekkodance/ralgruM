//! Single-instance coordination for Linux.
//!
//! Windows uses a named kernel mutex plus a named event; Linux uses an
//! abstract Unix domain socket with the same semantics: the first process
//! binds the socket and becomes the owner, later processes connect, forward
//! their browser link, and exit. Abstract namespace sockets live in kernel
//! memory only, so there is no filesystem path to clean up when the owner
//! dies: the binding disappears with the process.

use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::{UnixListener, UnixStream},
    thread,
    time::Duration,
};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::browser_link::{BrowserEntity, drain_pending_links, parse_ralgrum_url};
use crate::diagnostics;

/// Secondary launches should fail fast when the owner does not answer, so a
/// stuck browser-link click never leaves the user with a hanging process.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Abstract sockets bind without a race window, but the secondary retries
/// once to cover the shutdown gap where the connect failed and the bind
/// happened in the same instant.
const RECONNECT_DELAY: Duration = Duration::from_millis(250);
/// Payloads are one line each and the parser rejects anything longer, so a
/// large bound keeps a malicious peer from ballooning the read buffer.
const MAX_PAYLOAD_BYTES: usize = 4096;

/// Events forwarded from secondary launches to the running owner.
#[derive(Debug)]
pub(crate) enum BrowserLinkEvent {
    Restore,
    Open(BrowserEntity),
}

/// Result of the startup ownership handoff.
#[derive(Debug)]
pub(crate) enum BrowserLinkLaunch {
    /// This process forwarded its link and must exit.
    Forwarded,
    /// Single-instance checks are unavailable; keep launching.
    Unavailable,
    /// This process owns the application.
    Owner {
        /// Links already queued before the app started.
        initial: Vec<BrowserEntity>,
        /// Live link stream from later secondary launches.
        events: UnboundedReceiver<BrowserLinkEvent>,
    },
}

impl BrowserLinkLaunch {
    pub(crate) fn install_owner(&mut self, cx: &mut gpui::App) {
        // The owner badge is the bound listener held by the accept thread; it
        // only needs to outlive that thread, which the global keeps alive
        // independently. Nothing has to be stored on the GPUI side.
        if let Self::Owner { .. } = self {
            let _ = cx;
        }
    }

    pub(crate) fn take_initial(&mut self) -> Vec<BrowserEntity> {
        match self {
            Self::Forwarded | Self::Unavailable => Vec::new(),
            Self::Owner { initial, .. } => std::mem::take(initial),
        }
    }

    pub(crate) fn take_events(&mut self) -> Option<UnboundedReceiver<BrowserLinkEvent>> {
        match self {
            Self::Forwarded | Self::Unavailable => None,
            Self::Owner { events, .. } => Some(std::mem::replace(events, unbounded().1)),
        }
    }
}

/// Establishes the process handoff before GPUI starts. Every process after
/// the owner forwards its optional link or restore request and exits, so a
/// second launch never creates another application window.
pub(crate) fn prepare(raw: Option<String>) -> BrowserLinkLaunch {
    prepare_with_address(raw, socket_address())
}

fn prepare_with_address(raw: Option<String>, address: String) -> BrowserLinkLaunch {
    // A secondary with an unparseable link still restores the running window,
    // mirroring the Windows path where a queued link that fails validation
    // degrades into a plain restore request.
    let payload = match raw {
        Some(raw) if parse_ralgrum_url(&raw).is_some() => Payload::Link(raw),
        _ => Payload::Restore,
    };

    // Try the owner first. ECONNREFUSED means no process is bound yet, so
    // this launch becomes the owner candidate.
    if let Ok(stream) = UnixStream::connect(&address) {
        match forward_payload(stream, &payload) {
            Ok(()) => return BrowserLinkLaunch::Forwarded,
            Err(error) => {
                diagnostics::event(
                    "ERROR",
                    format!("secondary launch could not reach the running app: {error}"),
                );
                return BrowserLinkLaunch::Unavailable;
            }
        }
    }

    let listener = match UnixListener::bind(&address) {
        Ok(listener) => listener,
        Err(error) => {
            // The connect failing while the bind reports the address in use
            // points at a dying owner. Retry once; a second refusal means
            // an external process squats the name and the owner handoff
            // cannot be honored.
            thread::sleep(RECONNECT_DELAY);
            if let Ok(stream) = UnixStream::connect(&address) {
                match forward_payload(stream, &payload) {
                    Ok(()) => return BrowserLinkLaunch::Forwarded,
                    Err(error) => {
                        diagnostics::event(
                            "ERROR",
                            format!("secondary launch could not reach the running app: {error}"),
                        );
                        return BrowserLinkLaunch::Unavailable;
                    }
                }
            }
            diagnostics::event(
                "ERROR",
                format!(
                    "single-instance socket {address:?} could not be bound: {error}; \
                     refusing to start unmanaged"
                ),
            );
            return BrowserLinkLaunch::Unavailable;
        }
    };

    // Links queued by earlier launches (for example a browser click that
    // started the app, or a queued link written by an older build) must be
    // consumed before the first accept, exactly like the Windows owner.
    let initial = drain_pending_links();
    let (sender, events) = unbounded::<BrowserLinkEvent>();
    if !start_listener(listener, sender) {
        diagnostics::event(
            "ERROR",
            "browser-link listener could not start; refusing to start without IPC",
        );
        return BrowserLinkLaunch::Unavailable;
    }
    BrowserLinkLaunch::Owner { initial, events }
}

/// The wire payload sent by a secondary launch.
enum Payload {
    Restore,
    Link(String),
}

impl Payload {
    fn encode(&self) -> Vec<u8> {
        match self {
            Self::Restore => b"restore\n".to_vec(),
            Self::Link(raw) => {
                let mut line = raw.clone().into_bytes();
                line.push(b'\n');
                line
            }
        }
    }
}

fn forward_payload(mut stream: UnixStream, payload: &Payload) -> std::io::Result<()> {
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    stream.write_all(&payload.encode())?;
    stream.flush()?;
    // Half-close so the owner sees end of input, then wait for the owner to
    // close its side. The polite EOF wait confirms the payload was read, and
    // the read timeout keeps a stuck owner from hanging the secondary.
    stream.shutdown(Shutdown::Write)?;
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let mut sink = [0u8; 64];
    loop {
        match stream.read(&mut sink) {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                // The payload was written and flushed, so the owner will
                // read it from its socket buffer even though it is slow to
                // close its side. Treat the delivery as complete.
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}

fn start_listener(listener: UnixListener, sender: UnboundedSender<BrowserLinkEvent>) -> bool {
    thread::Builder::new()
        .name("browser-link-handoff".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else {
                    continue;
                };
                if let Some(event) = read_event(stream)
                    && sender.unbounded_send(event).is_err()
                {
                    // The receiver was dropped (app shutdown); stop the
                    // accept loop so the thread does not linger.
                    return;
                }
            }
        })
        .is_ok()
}

/// Reads one secondary connection and turns its line into an event. The
/// socket is dropped at the end of the call, which signals EOF to the
/// secondary.
fn read_event(mut stream: UnixStream) -> Option<BrowserLinkEvent> {
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let mut line = Vec::new();
    let mut chunk = [0u8; 256];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if let Some(newline) = chunk[..read].iter().position(|&byte| byte == b'\n') {
                    line.extend_from_slice(&chunk[..newline]);
                    break;
                }
                line.extend_from_slice(&chunk[..read]);
                if line.len() > MAX_PAYLOAD_BYTES {
                    diagnostics::event(
                        "WARN",
                        "dropping oversized browser-link payload from a secondary launch",
                    );
                    return None;
                }
            }
            Err(error) => {
                diagnostics::event(
                    "WARN",
                    format!("browser-link payload could not be read: {error}"),
                );
                return None;
            }
        }
    }
    if line == b"restore" {
        return Some(BrowserLinkEvent::Restore);
    }
    let raw = match std::str::from_utf8(&line) {
        Ok(raw) => raw,
        Err(error) => {
            diagnostics::event(
                "WARN",
                format!("browser-link payload was not valid UTF-8: {error}"),
            );
            return None;
        }
    };
    let entity = parse_ralgrum_url(raw);
    if entity.is_none() {
        diagnostics::event(
            "WARN",
            "dropping invalid ralgrum:// link from a secondary launch",
        );
    }
    entity.map(BrowserLinkEvent::Open)
}

/// Returns the abstract socket address used for single-instance checks.
pub(crate) fn socket_address() -> String {
    "\0ralgrum-browser-links-v1".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_payload_is_a_bare_line() {
        assert_eq!(Payload::Restore.encode(), b"restore\n");
    }

    #[test]
    fn link_payload_keeps_the_url_verbatim() {
        let raw = "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1";
        assert_eq!(
            Payload::Link(raw.to_owned()).encode(),
            format!("{raw}\n").into_bytes()
        );
    }

    #[test]
    fn socket_address_uses_the_linux_abstract_namespace() {
        let address = socket_address();
        assert!(address.starts_with('\0'));
        assert!(!address[1..].contains('\0'));
        assert_eq!(address, "\0ralgrum-browser-links-v1");
    }

    #[test]
    fn connect_timeout_is_bounded() {
        assert!(CONNECT_TIMEOUT <= Duration::from_secs(5));
    }

    #[test]
    fn second_launch_is_forwarded_and_reaches_the_owner() {
        // A filesystem-backed socket stands in for the abstract namespace
        // one: the std API surface used by prepare is identical, only the
        // address shape differs.
        let directory = std::env::temp_dir()
            .join(format!("ralgrum-instance-test-{}", std::process::id()))
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&directory).unwrap();
        let address = directory.join("socket");
        let address = address.to_str().unwrap().to_owned();

        let mut owner = prepare_with_address(None, address.clone());
        assert!(matches!(owner, BrowserLinkLaunch::Owner { .. }));
        let mut events = owner
            .take_events()
            .expect("owner should expose its listener");

        let secondary = prepare_with_address(None, address.clone());
        assert!(matches!(secondary, BrowserLinkLaunch::Forwarded));
        assert!(matches!(
            futures::executor::block_on(futures::StreamExt::next(&mut events)),
            Some(BrowserLinkEvent::Restore)
        ));

        let link = "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1";
        let secondary = prepare_with_address(Some(link.to_owned()), address.clone());
        assert!(matches!(secondary, BrowserLinkLaunch::Forwarded));
        match futures::executor::block_on(futures::StreamExt::next(&mut events)) {
            Some(BrowserLinkEvent::Open(entity)) => assert_eq!(entity.id, "1"),
            other => panic!("expected an open event, got {other:?}"),
        }

        // Dropping the receiver closes the channel but the accept thread is
        // parked in accept(); one more connection wakes it so it observes
        // the closed sender and exits instead of lingering.
        drop(events);
        drop(owner);
        let _ = UnixStream::connect(&address);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn invalid_link_argument_degrades_to_a_restore_request() {
        // A secondary with an unparseable link must still restore the
        // running window instead of exiting silently, mirroring the
        // Windows semantics of the queued-link fallback.
        let directory = std::env::temp_dir()
            .join(format!("ralgrum-instance-test-{}", std::process::id()))
            .join(uuid::Uuid::new_v4().simple().to_string());
        std::fs::create_dir_all(&directory).unwrap();
        let address = directory.join("socket");
        let address = address.to_str().unwrap().to_owned();

        let mut owner = prepare_with_address(None, address.clone());
        let mut events = owner.take_events().expect("owner listener");

        let secondary = prepare_with_address(Some("not-a-link".to_owned()), address.clone());
        assert!(matches!(secondary, BrowserLinkLaunch::Forwarded));
        assert!(matches!(
            futures::executor::block_on(futures::StreamExt::next(&mut events)),
            Some(BrowserLinkEvent::Restore)
        ));

        drop(events);
        drop(owner);
        let _ = UnixStream::connect(&address);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
