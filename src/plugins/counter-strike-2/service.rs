use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use cs2_gsi::{GameState, GameStateListener, events::NewGameState};
use futures::{StreamExt as _, channel::mpsc};
use gpui::App;

use super::{PORT, settings, state};
use crate::playback::{AutomationDirective, AutomationTransition};

const STALE_AFTER: Duration = Duration::from_secs(16);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ConnectionStatus {
    Disabled,
    Waiting,
    Connected,
    Error(String),
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeSnapshot {
    pub(super) connection: ConnectionStatus,
    pub(super) match_state: state::MatchState,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            connection: ConnectionStatus::Disabled,
            match_state: state::MatchState::Inactive,
        }
    }
}

enum ServiceEvent {
    Ready,
    Snapshot(Box<GameState>),
    Disconnected,
    Error(String),
}

struct Worker {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
static RUNTIME: OnceLock<Mutex<RuntimeSnapshot>> = OnceLock::new();
static SESSION: AtomicU64 = AtomicU64::new(0);

pub(super) fn validate_port() -> Result<(), String> {
    if WORKER
        .get()
        .and_then(|worker| worker.lock().ok())
        .is_some_and(|worker| worker.is_some())
    {
        // A recently disabled worker still owns the port until its listener
        // finishes shutting down. `enable` joins it before binding again.
        return Ok(());
    }
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, PORT))
        .map(|_| ())
        .map_err(|error| {
            format!("Counter-Strike 2 integration port 127.0.0.1:{PORT} is unavailable: {error}")
        })
}

pub(super) fn enable(cx: &mut App) {
    // Validation loads the settings and installs the GSI config before this
    // callback runs. Reuse that in-memory snapshot so enabling never blocks
    // the UI thread on filesystem work.
    let settings = settings::current();
    set_connection(ConnectionStatus::Waiting);
    let session = SESSION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    let (events, mut receiver) = mpsc::unbounded();
    cx.spawn(async move |cx| {
        while let Some(event) = receiver.next().await {
            let alive = cx.update(|cx| handle_event(session, event, cx));
            if !alive {
                break;
            }
        }
    })
    .detach();

    let mut worker = WORKER.get_or_init(|| Mutex::new(None)).lock().unwrap();
    let previous = worker.take();
    if let Some(previous) = &previous {
        previous.stop.store(true, Ordering::Release);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let token = settings.auth_token().to_owned();
    match thread::Builder::new()
        .name("counter-strike-2-gsi".into())
        .spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.thread.join();
            }
            run_listener(token, worker_stop, events);
        }) {
        Ok(thread) => *worker = Some(Worker { stop, thread }),
        Err(error) => {
            let message = format!("Counter-Strike 2 listener could not start: {error}");
            set_connection(ConnectionStatus::Error(message.clone()));
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Counter-Strike 2 integration could not start",
                Some(message.into()),
            );
        }
    }
}

pub(super) fn disable(cx: &mut App) {
    SESSION.fetch_add(1, Ordering::AcqRel);
    if let Some(worker) = WORKER.get().and_then(|worker| worker.lock().ok())
        && let Some(worker) = worker.as_ref()
    {
        worker.stop.store(true, Ordering::Release);
    }
    {
        let mut runtime = runtime().lock().unwrap();
        runtime.connection = ConnectionStatus::Disabled;
        runtime.match_state = state::MatchState::Inactive;
    }
    crate::playback::automation::release_global(cx);
}

pub(super) fn snapshot() -> RuntimeSnapshot {
    runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub(super) fn reapply_current(cx: &mut App) {
    let match_state = snapshot().match_state;
    apply_match_state(match_state, cx);
}

fn run_listener(token: String, stop: Arc<AtomicBool>, events: mpsc::UnboundedSender<ServiceEvent>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = events.unbounded_send(ServiceEvent::Error(format!(
                "Counter-Strike 2 listener runtime could not start: {error}"
            )));
            return;
        }
    };
    runtime.block_on(async move {
        let listener = GameStateListener::new(PORT);
        let last_valid = Arc::new(Mutex::new(None::<Instant>));
        let handler_last_valid = Arc::clone(&last_valid);
        let handler_events = events.clone();
        listener.on(move |event: &NewGameState| {
            if event.state.auth.get("token") != Some(token.as_str()) {
                return;
            }
            *handler_last_valid
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
            let _ = handler_events
                .unbounded_send(ServiceEvent::Snapshot(Box::new(event.state.clone())));
        });
        if let Err(error) = listener.start().await {
            let _ = events.unbounded_send(ServiceEvent::Error(format!(
                "Counter-Strike 2 listener could not bind to 127.0.0.1:{PORT}: {error}"
            )));
            return;
        }
        let _ = events.unbounded_send(ServiceEvent::Ready);
        let mut stale_reported = false;
        while !stop.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let stale = last_valid
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_some_and(|last| last.elapsed() >= STALE_AFTER);
            if stale && !stale_reported {
                stale_reported = true;
                let _ = events.unbounded_send(ServiceEvent::Disconnected);
            } else if !stale {
                stale_reported = false;
            }
        }
        let _ = listener.stop().await;
    });
}

fn handle_event(session: u64, event: ServiceEvent, cx: &mut App) -> bool {
    if SESSION.load(Ordering::Acquire) != session {
        return false;
    }
    match event {
        ServiceEvent::Ready => set_connection(ConnectionStatus::Waiting),
        ServiceEvent::Snapshot(snapshot) => {
            set_connection(ConnectionStatus::Connected);
            let next = state::resolve(&snapshot);
            let changed = {
                let mut runtime = runtime().lock().unwrap();
                let changed = runtime.match_state != next;
                runtime.match_state = next;
                changed
            };
            if changed {
                apply_match_state(next, cx);
            }
        }
        ServiceEvent::Disconnected => {
            set_connection(ConnectionStatus::Waiting);
            let changed = {
                let mut runtime = runtime().lock().unwrap();
                let changed = runtime.match_state != state::MatchState::Inactive;
                runtime.match_state = state::MatchState::Inactive;
                changed
            };
            if changed {
                apply_match_state(state::MatchState::Inactive, cx);
            }
        }
        ServiceEvent::Error(error) => {
            {
                let mut runtime = runtime().lock().unwrap();
                runtime.connection = ConnectionStatus::Error(error.clone());
                runtime.match_state = state::MatchState::Inactive;
            }
            crate::playback::automation::release_global(cx);
            crate::toast::push_global(
                cx,
                crate::toast::ToastKind::Error,
                "Counter-Strike 2 integration stopped",
                Some(error.into()),
            );
        }
    }
    true
}

fn apply_match_state(match_state: state::MatchState, cx: &mut App) {
    let settings = settings::current();
    let directive = match settings.action_for(match_state) {
        None => AutomationDirective::Release,
        Some(action) => match action.gain() {
            None => AutomationDirective::Pause,
            Some(gain) => AutomationDirective::Gain(gain),
        },
    };
    crate::playback::automation::apply_global(
        AutomationTransition {
            directive,
            fade_out: settings.fade_out(),
            fade_in: settings.fade_in(),
        },
        cx,
    );
}

fn runtime() -> &'static Mutex<RuntimeSnapshot> {
    RUNTIME.get_or_init(|| Mutex::new(RuntimeSnapshot::default()))
}

fn set_connection(connection: ConnectionStatus) {
    runtime()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connection = connection;
}
