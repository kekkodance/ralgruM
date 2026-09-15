use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use gpui::{App, Bounds, Context, Entity, Pixels, Point, Size, Task, Window, WindowBounds, px};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::SettingsView;

const PRIMARY_FILE: &str = "window_state.json";
const BACKUP_FILE: &str = "window_state.backup.json";
const MIN_WIDTH: f32 = 900.;
const MIN_HEIGHT: f32 = 620.;
const SAVE_DELAY: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedWindowState {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    maximized: bool,
}

impl SavedWindowState {
    fn repaired(self) -> Option<Self> {
        if !self.width.is_finite()
            || !self.height.is_finite()
            || self.width <= 0.
            || self.height <= 0.
        {
            return None;
        }

        Some(Self {
            x: self.x,
            y: self.y,
            width: self.width.max(MIN_WIDTH).round(),
            height: self.height.max(MIN_HEIGHT).round(),
            maximized: self.maximized,
        })
    }

    fn has_valid_position(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn from_bounds(bounds: Bounds<Pixels>, maximized: bool) -> Self {
        Self {
            x: f32::from(bounds.origin.x).round(),
            y: f32::from(bounds.origin.y).round(),
            width: f32::from(bounds.size.width).round(),
            height: f32::from(bounds.size.height).round(),
            maximized,
        }
    }

    fn bounds(self) -> Bounds<Pixels> {
        Bounds {
            origin: Point::new(px(self.x), px(self.y)),
            size: Size::new(px(self.width), px(self.height)),
        }
    }
}

pub(crate) struct WindowStateStore {
    directory: PathBuf,
    saved: Option<SavedWindowState>,
    writes: Arc<WindowWriteCoordinator>,
}

#[derive(Default)]
struct WindowWriteCoordinator {
    latest: AtomicU64,
    writer: Mutex<()>,
}

struct WindowStateWrite {
    directory: PathBuf,
    state: SavedWindowState,
    revision: u64,
    coordinator: Arc<WindowWriteCoordinator>,
}

impl WindowStateWrite {
    fn persist(self) -> io::Result<bool> {
        let _guard = self
            .coordinator
            .writer
            .lock()
            .map_err(|_| io::Error::other("window state writer was poisoned"))?;
        if self.coordinator.latest.load(Ordering::SeqCst) != self.revision {
            return Ok(false);
        }
        fs::create_dir_all(&self.directory)?;
        let encoded = encode(self.state);
        atomic_write(&self.directory.join(PRIMARY_FILE), &encoded)?;
        atomic_write(&self.directory.join(BACKUP_FILE), &encoded)?;
        Ok(true)
    }
}

impl WindowStateStore {
    pub(crate) fn load_current_user() -> Option<Self> {
        let directory = super::paths::config_dir()?;
        Some(Self::load(&directory))
    }

    fn load(directory: &Path) -> Self {
        let _ = fs::create_dir_all(directory);
        let primary_path = directory.join(PRIMARY_FILE);
        let backup_path = directory.join(BACKUP_FILE);
        let primary = read_state(&primary_path);
        let backup = read_state(&backup_path);
        let saved = primary.or(backup);

        if let Some(saved) = saved {
            let encoded = encode(saved);
            if primary != Some(saved) {
                let _ = atomic_write(&primary_path, &encoded);
            }
            if backup != Some(saved) {
                let _ = atomic_write(&backup_path, &encoded);
            }
        }

        Self {
            directory: directory.to_owned(),
            saved,
            writes: Arc::default(),
        }
    }

    pub(crate) fn saved(&self) -> Option<SavedWindowState> {
        self.saved
    }

    fn persist(&mut self, saved: SavedWindowState) -> io::Result<()> {
        self.prepare_persist(saved).persist()?;
        Ok(())
    }

    fn prepare_persist(&mut self, saved: SavedWindowState) -> WindowStateWrite {
        let revision = self
            .writes
            .latest
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        self.saved = Some(saved);
        WindowStateWrite {
            directory: self.directory.clone(),
            state: saved,
            revision,
            coordinator: self.writes.clone(),
        }
    }
}

pub(crate) fn startup_bounds(
    restore_window: bool,
    saved: Option<SavedWindowState>,
    displays: &[Bounds<Pixels>],
    cx: &App,
) -> WindowBounds {
    let centered = Bounds::centered(None, Size::new(px(1180.), px(760.)), cx);
    let Some(saved) = restore_window
        .then_some(saved)
        .flatten()
        .and_then(|state| state.repaired())
    else {
        return WindowBounds::Windowed(centered);
    };

    let bounds = if saved.has_valid_position()
        && displays
            .iter()
            .any(|display| position_is_visible(saved, *display))
    {
        saved.bounds()
    } else {
        Bounds {
            origin: centered.origin,
            size: saved.bounds().size,
        }
    };

    if saved.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

fn position_is_visible(window: SavedWindowState, display: Bounds<Pixels>) -> bool {
    let left = f32::from(display.origin.x);
    let top = f32::from(display.origin.y);
    let right = left + f32::from(display.size.width);
    let bottom = top + f32::from(display.size.height);
    window.x + window.width.min(120.) >= left
        && window.x <= right - 80.
        && window.y + window.height.min(80.) >= top
        && window.y <= bottom - 40.
}

pub(crate) struct WindowStateManager {
    settings: Entity<SettingsView>,
    store: Option<WindowStateStore>,
    normal: SavedWindowState,
    pending_save: Option<Task<()>>,
}

impl WindowStateManager {
    pub(crate) fn new(
        settings: Entity<SettingsView>,
        store: Option<WindowStateStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let normal = store
            .as_ref()
            .and_then(WindowStateStore::saved)
            .and_then(SavedWindowState::repaired)
            .unwrap_or_else(|| SavedWindowState::from_bounds(window.bounds(), false));
        let mut last_size_state = (window.bounds().size, window.is_maximized());
        cx.observe_window_bounds(window, move |this, window, cx| {
            let current_size_state = (window.bounds().size, window.is_maximized());
            if last_size_state != current_size_state {
                last_size_state = current_size_state;
                cx.notify();
            }
            this.schedule_save(window, cx);
        })
        .detach();
        Self {
            settings,
            store,
            normal,
            pending_save: None,
        }
    }

    fn schedule_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !should_start_save(self.pending_save.is_some()) {
            return;
        }
        self.pending_save = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                this.pending_save.take();
                this.persist_now_background(window, cx);
            })
            .ok();
        }));
    }

    pub(crate) fn persist_now(&mut self, window: &Window, cx: &App) {
        let Some(state) = self.state_for_persistence(window, cx) else {
            return;
        };
        if let Some(store) = self.store.as_mut() {
            let _ = store.persist(state);
        }
    }

    fn persist_now_background(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(state) = self.state_for_persistence(window, cx) else {
            return;
        };
        let Some(write) = self
            .store
            .as_mut()
            .map(|store| store.prepare_persist(state))
        else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(error) = write.persist() {
                    crate::diagnostics::event(
                        "WARN",
                        format!("Could not save window state: {error}"),
                    );
                }
            })
            .detach();
    }

    fn state_for_persistence(&mut self, window: &Window, cx: &App) -> Option<SavedWindowState> {
        if !self.settings.read(cx).saved().restore_window {
            return None;
        }
        let state =
            state_for_persistence(self.normal, window.window_bounds(), window.is_maximized());
        self.normal = SavedWindowState {
            maximized: false,
            ..state
        };
        Some(state)
    }
}

fn should_start_save(has_pending_save: bool) -> bool {
    !has_pending_save
}

fn state_for_persistence(
    previous_normal: SavedWindowState,
    window_bounds: WindowBounds,
    maximized: bool,
) -> SavedWindowState {
    let mut state = if maximized {
        match window_bounds {
            WindowBounds::Maximized(restore_bounds) => {
                SavedWindowState::from_bounds(restore_bounds, true)
            }
            _ => previous_normal,
        }
    } else {
        SavedWindowState::from_bounds(window_bounds.get_bounds(), false)
    };
    state.maximized = maximized;
    state
}

fn read_state(path: &Path) -> Option<SavedWindowState> {
    serde_json::from_slice::<SavedWindowState>(&fs::read(path).ok()?)
        .ok()?
        .repaired()
}

fn encode(state: SavedWindowState) -> Vec<u8> {
    serde_json::to_vec_pretty(&state).expect("window state is serializable")
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(io::ErrorKind::InvalidInput)?;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let result = file
                    .write_all(contents)
                    .and_then(|_| file.sync_all())
                    .and_then(|_| atomic_rename(&temporary, path));
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::ErrorKind::AlreadyExists.into())
}

#[cfg(windows)]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn state(x: f32, y: f32, width: f32, height: f32, maximized: bool) -> SavedWindowState {
        SavedWindowState {
            x,
            y,
            width,
            height,
            maximized,
        }
    }

    #[test]
    fn repairs_size_without_losing_maximized_state() {
        let repaired = state(10., 20., 400., 300., true).repaired().unwrap();
        assert_eq!((repaired.width, repaired.height), (900., 620.));
        assert!(repaired.maximized);
        assert!(state(0., 0., f32::NAN, 700., false).repaired().is_none());
    }

    #[test]
    fn visibility_matches_original_titlebar_margins() {
        let display = Bounds {
            origin: Point::new(px(0.), px(0.)),
            size: Size::new(px(1920.), px(1080.)),
        };
        assert!(position_is_visible(
            state(-120., -80., 900., 620., false),
            display
        ));
        assert!(position_is_visible(
            state(1840., 1040., 900., 620., false),
            display
        ));
        assert!(!position_is_visible(
            state(-121., 0., 900., 620., false),
            display
        ));
        assert!(!position_is_visible(
            state(1841., 0., 900., 620., false),
            display
        ));
    }

    #[test]
    fn store_round_trips_and_repairs_a_corrupt_primary() {
        let temp = TempDir::new().unwrap();
        let saved = state(12., 34., 1000., 700., true);
        let mut store = WindowStateStore::load(temp.path());
        store.persist(saved).unwrap();
        fs::write(temp.path().join(PRIMARY_FILE), b"not json").unwrap();

        let repaired = WindowStateStore::load(temp.path());
        assert_eq!(repaired.saved(), Some(saved));
        assert_eq!(read_state(&temp.path().join(PRIMARY_FILE)), Some(saved));
    }

    #[test]
    fn superseded_background_write_cannot_overwrite_the_latest_window_state() {
        let temp = TempDir::new().unwrap();
        let mut store = WindowStateStore::load(temp.path());
        let older = store.prepare_persist(state(10., 20., 900., 620., false));
        let latest_state = state(30., 40., 1100., 720., true);
        let latest = store.prepare_persist(latest_state);

        assert!(!older.persist().unwrap());
        assert!(latest.persist().unwrap());
        assert_eq!(
            read_state(&temp.path().join(PRIMARY_FILE)),
            Some(latest_state)
        );
        assert_eq!(
            read_state(&temp.path().join(BACKUP_FILE)),
            Some(latest_state)
        );
    }

    #[test]
    fn maximized_capture_preserves_authoritative_restore_bounds() {
        let previous = state(1., 2., 900., 620., false);
        let restore = state(30., 40., 1100., 700., false).bounds();

        assert_eq!(
            state_for_persistence(previous, WindowBounds::Maximized(restore), true),
            state(30., 40., 1100., 700., true)
        );
        assert_eq!(
            state_for_persistence(previous, WindowBounds::Windowed(restore), true),
            state(1., 2., 900., 620., true)
        );
    }

    #[test]
    fn save_schedule_coalesces_bounds_events_while_timer_is_pending() {
        assert!(should_start_save(false));
        assert!(!should_start_save(true));
    }
}
