//! Shared write-behind persistence for the Discover caches.
//!
//! One background writer thread serves every Discover cache. Snapshots
//! travel as serialized bytes and coalesce per cache file path, so rapid
//! mutations collapse into one durable write of the latest state. Each
//! cache owns a write hook slot that travels with its queued writes, so
//! one cache's tests can never observe, delay, or clear another cache's
//! writes.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
};

/// Observation point a cache's tests install to watch and delay that
/// cache's durable writes. The hook runs before the real write, which
/// still happens once the hook returns. Production builds never install
/// one.
pub(crate) type WriteHook = Box<dyn Fn(&Path) + Send>;

/// A single cache's write hook slot. The slot travels along with every
/// write that cache queues, so the shared writer runs the owning cache's
/// hook, and only that cache's, just before the durable write. Slots are
/// per cache, so tests of one Discover cache can never clear or fire
/// another cache's hook.
pub(crate) struct WriteHookSlot {
    hook: Mutex<Option<WriteHook>>,
}

impl WriteHookSlot {
    pub(crate) const fn new() -> Self {
        Self {
            hook: Mutex::new(None),
        }
    }

    /// Install or remove the hook. Removing waits for any in-flight hook
    /// call to finish first, so a scope guard cannot uninstall a hook a
    /// blocked write is still running.
    #[cfg(test)]
    pub(crate) fn set(&self, hook: Option<WriteHook>) {
        *self
            .hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = hook;
    }

    /// Run the installed hook, if any. The lock is held across the call,
    /// so hook calls stay ordered and uninstalling always waits for the
    /// running one.
    fn run(&self, path: &Path) {
        let guard = self
            .hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(hook) = guard.as_ref() {
            hook(path);
        }
    }
}

/// A queued snapshot: the serialized bytes to persist plus the owning
/// cache's write hook slot.
struct PendingWrite {
    bytes: Vec<u8>,
    slot: &'static WriteHookSlot,
}

/// Pending durable writes, keyed by cache path so the latest snapshot
/// replaces any earlier one that has not reached disk yet.
#[derive(Default)]
struct WriteBehindState {
    pending: HashMap<PathBuf, PendingWrite>,
    enqueued: u64,
    written: u64,
}

/// Single background writer for every Discover cache. The in-memory
/// state stays authoritative for the session; durable writes happen off
/// the state-update path and coalesce per path, so the last state wins.
#[derive(Default)]
struct WriteBehind {
    state: Mutex<WriteBehindState>,
    idle: Condvar,
}

static WRITE_BEHIND: LazyLock<Option<Arc<WriteBehind>>> = LazyLock::new(|| {
    let worker = Arc::new(WriteBehind::default());
    let spawned = worker.clone();
    std::thread::Builder::new()
        .name("discover-cache-writer".to_owned())
        .spawn(move || run_write_behind(spawned))
        .is_ok()
        .then_some(worker)
});

/// Hand a serialized snapshot to the writer thread. Returns as soon as
/// the snapshot is queued; the durable write happens in the background.
/// If the writer thread could not be spawned, fall back to a synchronous
/// write. The slot's hook, when installed, runs just before the durable
/// write.
pub(crate) fn schedule_write(path: PathBuf, bytes: Vec<u8>, slot: &'static WriteHookSlot) {
    let Some(worker) = WRITE_BEHIND.as_ref() else {
        durable_write(&path, &bytes, slot);
        return;
    };
    let mut state = worker
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.pending.insert(path, PendingWrite { bytes, slot });
    state.enqueued += 1;
    drop(state);
    worker.idle.notify_all();
}

fn run_write_behind(worker: Arc<WriteBehind>) {
    loop {
        let mut state = worker
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while state.pending.is_empty() {
            state = worker
                .idle
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        let batch = std::mem::take(&mut state.pending);
        let written_through = state.enqueued;
        drop(state);
        for (path, write) in &batch {
            durable_write(path, &write.bytes, write.slot);
        }
        let mut state = worker
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.written = state.written.max(written_through);
        worker.idle.notify_all();
    }
}

fn durable_write(path: &Path, bytes: &[u8], slot: &WriteHookSlot) {
    slot.run(path);
    let _ = write_atomic(path, bytes);
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path"))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache filename"))?;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let result = file
                    .write_all(bytes)
                    .and_then(|_| file.sync_all())
                    .and_then(|_| atomic_replace(&temporary, path));
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cache temporary file contention",
    ))
}

#[cfg(windows)]
fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
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
fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// Block until every queued snapshot has been written. Test-only
/// determinism helper; production relies on the writer draining promptly.
#[cfg(test)]
pub(crate) fn flush_write_behind() {
    let Some(worker) = WRITE_BEHIND.as_ref() else {
        return;
    };
    let mut state = worker
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let target = state.enqueued;
    while state.written < target {
        state = worker
            .idle
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use tempfile::TempDir;

    #[test]
    fn write_hooks_stay_isolated_per_slot() {
        static FIRST: WriteHookSlot = WriteHookSlot::new();
        static SECOND: WriteHookSlot = WriteHookSlot::new();
        let temp = TempDir::new().unwrap();
        let (seen_tx, seen_rx) = mpsc::channel::<PathBuf>();
        FIRST.set(Some(Box::new(move |path: &Path| {
            let _ = seen_tx.send(path.to_path_buf());
        })));

        let first_path = temp.path().join("first.json");
        let second_path = temp.path().join("second.json");
        schedule_write(first_path.clone(), b"first".to_vec(), &FIRST);
        schedule_write(second_path.clone(), b"second".to_vec(), &SECOND);
        flush_write_behind();

        // Only the hooked slot's write ran the hook; both writes still
        // reached disk.
        assert_eq!(seen_rx.try_recv().unwrap(), first_path);
        assert!(seen_rx.try_recv().is_err());
        assert_eq!(fs::read(&first_path).unwrap(), b"first");
        assert_eq!(fs::read(&second_path).unwrap(), b"second");

        // Removing the hook stops later writes from being observed.
        FIRST.set(None);
        schedule_write(first_path.clone(), b"again".to_vec(), &FIRST);
        flush_write_behind();
        assert_eq!(fs::read(&first_path).unwrap(), b"again");
        assert!(seen_rx.try_recv().is_err());
    }
}
