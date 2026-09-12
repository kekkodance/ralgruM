use std::{thread, time::Duration};

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
    remove_pending_link,
};

const EVENT_NAME: windows::core::PCWSTR = w!("Local\\ralgruM-browser-links-v1-event");
const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\ralgruM-browser-links-v1-owner");
const EVENT_OPEN_RETRIES: usize = 25;

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

pub(crate) enum BrowserLinkLaunch {
    Forwarded,
    Local {
        initial: Vec<BrowserEntity>,
    },
    Owner {
        mutex: OwnedKernelHandle,
        initial: Vec<BrowserEntity>,
        events: UnboundedReceiver<BrowserEntity>,
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
            Self::Forwarded => Vec::new(),
            Self::Local { initial } | Self::Owner { initial, .. } => std::mem::take(initial),
        }
    }

    pub(crate) fn take_events(&mut self) -> Option<UnboundedReceiver<BrowserEntity>> {
        match self {
            Self::Owner { events, .. } => Some(std::mem::replace(events, unbounded().1)),
            _ => None,
        }
    }
}

/// Establishes the process handoff before GPUI starts. A protocol-only
/// secondary process forwards its validated link and exits. Ordinary second
/// launches remain ordinary launches and do not change existing behavior.
pub(crate) fn prepare(raw: Option<String>) -> BrowserLinkLaunch {
    let valid = raw.and_then(|raw| parse_ralgrum_url(&raw).map(|entity| (raw, entity)));

    let Ok(mutex) = (unsafe { CreateMutexW(None, false, MUTEX_NAME) }) else {
        return local(valid.map(|(_, entity)| entity));
    };
    let already_owned = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = OwnedKernelHandle(mutex);

    if already_owned {
        let Some((raw, entity)) = valid else {
            return BrowserLinkLaunch::Local {
                initial: Vec::new(),
            };
        };
        let Some((_, queued_path)) = enqueue_pending_link(&raw) else {
            return local(Some(entity));
        };
        if signal_owner() {
            return BrowserLinkLaunch::Forwarded;
        }
        remove_pending_link(&queued_path);
        return local(Some(entity));
    }

    let Ok(event) = (unsafe { CreateEventW(None, false, false, EVENT_NAME) }) else {
        return local(valid.map(|(_, entity)| entity));
    };
    let mut direct_fallback = None;
    if let Some((raw, entity)) = valid
        && enqueue_pending_link(&raw).is_none()
    {
        direct_fallback = Some(entity);
    }

    let mut initial = drain_pending_links();
    initial.extend(direct_fallback);
    let (sender, events) = unbounded();
    if !start_listener(OwnedKernelHandle(event), sender) {
        return BrowserLinkLaunch::Local { initial };
    }
    BrowserLinkLaunch::Owner {
        mutex,
        initial,
        events,
    }
}

fn local(entity: Option<BrowserEntity>) -> BrowserLinkLaunch {
    BrowserLinkLaunch::Local {
        initial: entity.into_iter().collect(),
    }
}

fn signal_owner() -> bool {
    for attempt in 0..EVENT_OPEN_RETRIES {
        if let Ok(event) = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, EVENT_NAME) } {
            let event = OwnedKernelHandle(event);
            if unsafe { SetEvent(event.0) }.is_ok() {
                return true;
            }
        }
        if attempt + 1 < EVENT_OPEN_RETRIES {
            thread::sleep(Duration::from_millis(10));
        }
    }
    false
}

fn start_listener(event: OwnedKernelHandle, sender: UnboundedSender<BrowserEntity>) -> bool {
    thread::Builder::new()
        .name("browser-link-handoff".into())
        .spawn(move || {
            loop {
                let wait = unsafe { WaitForSingleObject(event.raw(), INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    break;
                }
                for entity in drain_pending_links() {
                    if sender.unbounded_send(entity).is_err() {
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

    #[test]
    fn local_launch_keeps_a_valid_entity() {
        let entity = parse_ralgrum_url(
            "ralgrum://open?provider=deezer&type=track&id=1&url=https%3A%2F%2Fwww.deezer.com%2Ftrack%2F1",
        )
        .unwrap();
        let mut launch = local(Some(entity.clone()));
        assert_eq!(launch.take_initial(), vec![entity]);
    }

    #[test]
    fn forwarding_retry_is_short_and_bounded() {
        assert_eq!(EVENT_OPEN_RETRIES, 25);
        assert!(
            Duration::from_millis(10) * (EVENT_OPEN_RETRIES.saturating_sub(1) as u32)
                < Duration::from_millis(250)
        );
    }
}
