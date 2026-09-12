use std::{path::PathBuf, thread, time::Duration};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{App, Global};
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0},
        System::Threading::{
            CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
            WaitForSingleObject,
        },
    },
    core::w,
};

use crate::browser_link::{
    BrowserEntity, drain_pending_links, enqueue_pending_link, parse_ralgrum_url,
};

const EVENT_NAME: windows::core::PCWSTR = w!("Local\\ralgruM-browser-links-v1-event");
const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\ralgruM-browser-links-v1-owner");
const EVENT_OPEN_RETRIES: usize = 100;
const EVENT_OPEN_RETRY_DELAY: Duration = Duration::from_millis(25);

type EnqueueLink = fn(&str) -> Option<(BrowserEntity, PathBuf)>;
type DrainLinks = fn() -> Vec<BrowserEntity>;

pub(crate) struct OwnedKernelHandle(HANDLE);

unsafe impl Send for OwnedKernelHandle {}
unsafe impl Sync for OwnedKernelHandle {}

impl OwnedKernelHandle {
    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedKernelHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct BrowserLinkOwner {
    _mutex: OwnedKernelHandle,
}

impl Global for BrowserLinkOwner {}

#[derive(Debug)]
pub(crate) enum BrowserLinkEvent {
    Restore,
    Open(BrowserEntity),
}

pub(crate) enum BrowserLinkLaunch {
    Forwarded,
    Unavailable,
    Owner {
        mutex: OwnedKernelHandle,
        initial: Vec<BrowserEntity>,
        events: UnboundedReceiver<BrowserLinkEvent>,
    },
}

impl BrowserLinkLaunch {
    pub(crate) fn install_owner(&mut self, cx: &mut App) {
        if let Self::Owner { mutex, .. } = self {
            let mutex = std::mem::replace(mutex, OwnedKernelHandle(HANDLE::default()));
            cx.set_global(BrowserLinkOwner { _mutex: mutex });
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
            Self::Owner { events, .. } => Some(std::mem::replace(events, unbounded().1)),
            _ => None,
        }
    }
}

/// Establishes the process handoff before GPUI starts. Every process after the
/// owner forwards its optional link or restore request and exits, so a second
/// launch never creates another application window.
pub(crate) fn prepare(raw: Option<String>) -> BrowserLinkLaunch {
    prepare_with_names(raw, MUTEX_NAME, EVENT_NAME)
}

fn prepare_with_names(
    raw: Option<String>,
    mutex_name: windows::core::PCWSTR,
    event_name: windows::core::PCWSTR,
) -> BrowserLinkLaunch {
    prepare_with_names_and_queue(
        raw,
        mutex_name,
        event_name,
        enqueue_pending_link,
        drain_pending_links,
    )
}

fn prepare_with_names_and_queue(
    raw: Option<String>,
    mutex_name: windows::core::PCWSTR,
    event_name: windows::core::PCWSTR,
    enqueue: EnqueueLink,
    drain: DrainLinks,
) -> BrowserLinkLaunch {
    let valid = raw.and_then(|raw| parse_ralgrum_url(&raw).map(|entity| (raw, entity)));

    let Ok(mutex) = (unsafe { CreateMutexW(None, false, mutex_name) }) else {
        crate::diagnostics::event(
            "ERROR",
            "single-instance mutex could not be created; refusing to start unmanaged",
        );
        return BrowserLinkLaunch::Unavailable;
    };
    let already_owned = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = OwnedKernelHandle(mutex);

    if already_owned {
        if let Some((raw, _entity)) = valid
            && enqueue(&raw).is_none()
        {
            crate::diagnostics::event("WARN", "could not queue browser link for the running app");
        }
        if !signal_owner(event_name) {
            crate::diagnostics::event(
                "WARN",
                "running app did not accept the secondary launch request",
            );
        }
        // The mutex was acquired with ERROR_ALREADY_EXISTS. Keep this process
        // from becoming a second owner even when the handoff event is not
        // available yet. The queued link remains for the owner to consume.
        return BrowserLinkLaunch::Forwarded;
    }

    let Ok(event) = (unsafe { CreateEventW(None, false, false, event_name) }) else {
        crate::diagnostics::event(
            "ERROR",
            "browser-link event could not be created; refusing to start without IPC",
        );
        return BrowserLinkLaunch::Unavailable;
    };
    let mut direct_fallback = None;
    if let Some((raw, entity)) = valid
        && enqueue(&raw).is_none()
    {
        direct_fallback = Some(entity);
    }

    let mut initial = drain();
    initial.extend(direct_fallback);
    let (sender, events) = unbounded::<BrowserLinkEvent>();
    if !start_listener(OwnedKernelHandle(event), sender, drain) {
        crate::diagnostics::event(
            "ERROR",
            "browser-link listener could not start; refusing to start without IPC",
        );
        return BrowserLinkLaunch::Unavailable;
    }
    BrowserLinkLaunch::Owner {
        mutex,
        initial,
        events,
    }
}

fn signal_owner(event_name: windows::core::PCWSTR) -> bool {
    for attempt in 0..EVENT_OPEN_RETRIES {
        if let Ok(event) = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, event_name) } {
            let event = OwnedKernelHandle(event);
            if unsafe { SetEvent(event.0) }.is_ok() {
                return true;
            }
        }
        if attempt + 1 < EVENT_OPEN_RETRIES {
            thread::sleep(EVENT_OPEN_RETRY_DELAY);
        }
    }
    false
}

fn start_listener(
    event: OwnedKernelHandle,
    sender: UnboundedSender<BrowserLinkEvent>,
    drain: DrainLinks,
) -> bool {
    thread::Builder::new()
        .name("browser-link-handoff".into())
        .spawn(move || {
            loop {
                let wait = unsafe { WaitForSingleObject(event.raw(), INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    break;
                }
                if sender.unbounded_send(BrowserLinkEvent::Restore).is_err() {
                    return;
                }
                for entity in drain() {
                    if sender
                        .unbounded_send(BrowserLinkEvent::Open(entity))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt as _;

    fn isolated_enqueue(_: &str) -> Option<(BrowserEntity, PathBuf)> {
        None
    }

    fn isolated_drain() -> Vec<BrowserEntity> {
        Vec::new()
    }

    #[test]
    fn forwarding_retry_is_short_and_bounded() {
        assert_eq!(EVENT_OPEN_RETRIES, 100);
        assert!(
            EVENT_OPEN_RETRY_DELAY * (EVENT_OPEN_RETRIES.saturating_sub(1) as u32)
                < Duration::from_secs(3)
        );
    }

    #[test]
    fn second_regular_launch_is_forwarded_and_restores_the_owner() {
        let suffix = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4().simple());
        let mutex_name_storage = format!("Local\\ralgrum-test-mutex-{suffix}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let event_name_storage = format!("Local\\ralgrum-test-event-{suffix}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mutex_name = windows::core::PCWSTR::from_raw(mutex_name_storage.as_ptr());
        let event_name = windows::core::PCWSTR::from_raw(event_name_storage.as_ptr());

        let mut owner = prepare_with_names_and_queue(
            None,
            mutex_name,
            event_name,
            isolated_enqueue,
            isolated_drain,
        );
        assert!(matches!(&owner, BrowserLinkLaunch::Owner { .. }));
        let mut events = owner
            .take_events()
            .expect("owner should expose its listener");

        let secondary = prepare_with_names_and_queue(
            None,
            mutex_name,
            event_name,
            isolated_enqueue,
            isolated_drain,
        );
        assert!(matches!(secondary, BrowserLinkLaunch::Forwarded));
        assert!(matches!(
            futures::executor::block_on(events.next()),
            Some(BrowserLinkEvent::Restore)
        ));

        // Let the listener observe the dropped receiver and exit instead of
        // leaving a test thread waiting on the named event.
        drop(events);
        drop(owner);
        let _ = signal_owner(event_name);
    }
}
