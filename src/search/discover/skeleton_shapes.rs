use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
};

use sha2::{Digest, Sha256};

use super::model::DiscoverSection;
use crate::search::models::Provider;

const CACHE_DIRECTORY: &str = "discover-v1";
const CACHE_FILE_PREFIX: &str = "skeleton-shapes";
const CACHE_SCHEMA: &str = "ralgrum.discover.skeleton-shapes";
const CACHE_VERSION: u32 = 1;
const MAX_SECTION_POSITIONS: usize = 32;
const MAX_CACHE_FILE_BYTES: usize = 8 * 1024;

/// Remembered card line counts per section position, so the next launch
/// renders loading stand-ins that already match the shape of the
/// carousels that will replace them. Each flag is true when that
/// position's carousel renders single line cards.
#[derive(Clone, Debug, Default)]
pub(crate) struct SkeletonShapesCache {
    deezer: Vec<bool>,
    soundcloud: Vec<bool>,
    storage_path: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct PersistedSkeletonShapes {
    schema: String,
    version: u32,
    deezer: Vec<bool>,
    soundcloud: Vec<bool>,
}

impl SkeletonShapesCache {
    pub(crate) fn load(account_scope: &str) -> Self {
        let root = crate::paths::cache_dir().join(CACHE_DIRECTORY);
        Self::load_from_dir(&root, account_scope)
    }

    fn load_from_dir(root: &Path, account_scope: &str) -> Self {
        let storage_path = cache_path(root, account_scope);
        let mut cache = Self {
            deezer: Vec::new(),
            soundcloud: Vec::new(),
            storage_path: Some(storage_path.clone()),
        };
        let Ok(bytes) = fs::read(&storage_path) else {
            return cache;
        };
        if bytes.len() > MAX_CACHE_FILE_BYTES {
            return cache;
        }
        let Ok(raw_document) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return cache;
        };
        let Some(document) = raw_document.as_object() else {
            return cache;
        };
        let Some(schema) = document.get("schema").and_then(serde_json::Value::as_str) else {
            return cache;
        };
        let Some(version) = document
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
        else {
            return cache;
        };
        if schema != CACHE_SCHEMA || version != CACHE_VERSION {
            return cache;
        }
        cache.deezer = boolean_flags(document.get("deezer"));
        cache.soundcloud = boolean_flags(document.get("soundcloud"));
        cache
    }

    /// Replaces the provider's remembered shapes with the line counts its
    /// freshly loaded sections actually render, then hands the snapshot to
    /// the write-behind writer so the state update never blocks on disk.
    pub(crate) fn record(&mut self, provider: Provider, sections: &[DiscoverSection]) {
        let flags = sections
            .iter()
            .map(section_single_line)
            .take(MAX_SECTION_POSITIONS)
            .collect::<Vec<_>>();
        let remembered = match provider {
            Provider::Deezer => &mut self.deezer,
            Provider::SoundCloud => &mut self.soundcloud,
        };
        if *remembered == flags {
            return;
        }
        *remembered = flags;
        self.persist();
    }

    pub(crate) fn single_line(&self, provider: Provider, index: usize) -> bool {
        let flags = match provider {
            Provider::Deezer => &self.deezer,
            Provider::SoundCloud => &self.soundcloud,
        };
        flags.get(index).is_some_and(|flag| *flag)
    }

    /// How many section positions were remembered for the provider, so a
    /// loading skeleton can mirror the grouped feed layout block by block.
    pub(crate) fn section_count(&self, provider: Provider) -> usize {
        match provider {
            Provider::Deezer => self.deezer.len(),
            Provider::SoundCloud => self.soundcloud.len(),
        }
    }

    fn persist(&self) {
        let Some(path) = self.storage_path.clone() else {
            return;
        };
        schedule_write(path, self.snapshot());
    }

    fn snapshot(&self) -> PersistedSkeletonShapes {
        PersistedSkeletonShapes {
            schema: CACHE_SCHEMA.to_owned(),
            version: CACHE_VERSION,
            deezer: self.deezer.clone(),
            soundcloud: self.soundcloud.clone(),
        }
    }
}

/// A carousel row keeps its subtitle chin when any preview card shows a
/// subtitle row. This mirrors the real card body: discover cards never
/// carry a privacy marker, so subtitle visibility alone decides whether
/// the row renders one or two text lines.
fn section_single_line(section: &DiscoverSection) -> bool {
    !section
        .items
        .iter()
        .take(super::view::DISCOVER_CARD_PREVIEW_LIMIT)
        .any(|item| crate::search::collection_subtitle_is_visible(&item.card.subtitle))
}

/// Collects the boolean flags a provider array carries, ignoring non
/// boolean values and everything past the position cap.
fn boolean_flags(value: Option<&serde_json::Value>) -> Vec<bool> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_bool)
                .take(MAX_SECTION_POSITIONS)
                .collect()
        })
        .unwrap_or_default()
}

fn cache_path(root: &Path, account_scope: &str) -> PathBuf {
    root.join(format!(
        "{CACHE_FILE_PREFIX}-{}.json",
        account_scope_hash(account_scope)
    ))
}

fn account_scope_hash(account_scope: &str) -> String {
    let digest = Sha256::digest(account_scope.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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

/// Pending durable writes, keyed by cache path so the latest snapshot
/// replaces any earlier one that has not reached disk yet.
#[derive(Default)]
struct WriteBehindState {
    pending: HashMap<PathBuf, PersistedSkeletonShapes>,
    enqueued: u64,
    written: u64,
}

/// Single background writer for skeleton-shape snapshots. The in-memory
/// flags stay authoritative for the session; durable writes happen off
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
        .name("discover-skeleton-shapes-writer".to_owned())
        .spawn(move || run_write_behind(spawned))
        .is_ok()
        .then_some(worker)
});

/// Hand a snapshot to the writer thread. Returns as soon as the snapshot
/// is queued; the durable write happens in the background. If the writer
/// thread could not be spawned, fall back to a synchronous write.
fn schedule_write(path: PathBuf, document: PersistedSkeletonShapes) {
    let Some(worker) = WRITE_BEHIND.as_ref() else {
        durable_write(&path, &document);
        return;
    };
    let mut state = worker
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.pending.insert(path, document);
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
        for (path, document) in &batch {
            durable_write(path, document);
        }
        let mut state = worker
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.written = state.written.max(written_through);
        worker.idle.notify_all();
    }
}

fn durable_write(path: &Path, document: &PersistedSkeletonShapes) {
    let Ok(bytes) = serde_json::to_vec_pretty(document) else {
        return;
    };
    let _ = write_atomic(path, &bytes);
}

/// Block until every queued snapshot has been written. Test-only
/// determinism helper; production relies on the writer draining promptly.
#[cfg(test)]
fn flush_write_behind() {
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
    use crate::search::discover::{DiscoverAction, DiscoverItem};
    use crate::search::{Card, ResultType};
    use tempfile::TempDir;

    fn section(provider: Provider, index: usize, card_subtitles: &[&str]) -> DiscoverSection {
        DiscoverSection {
            id: format!("section-{index}"),
            provider,
            title: format!("Section {index}"),
            subtitle: String::new(),
            items: card_subtitles
                .iter()
                .enumerate()
                .map(|(card_index, subtitle)| DiscoverItem {
                    card: Card {
                        kind: ResultType::Albums,
                        id: format!("card-{index}-{card_index}"),
                        title: "Title".into(),
                        subtitle: (*subtitle).into(),
                        source: provider,
                        ..Card::default()
                    },
                    action: DiscoverAction::OpenDetail,
                })
                .collect(),
        }
    }

    #[test]
    fn records_and_reloads_per_provider_card_line_counts() {
        let temp = TempDir::new().unwrap();
        let account_scope = "deezer:user:secret-credential";
        let mut cache = SkeletonShapesCache::load_from_dir(temp.path(), account_scope);
        cache.record(
            Provider::Deezer,
            &[
                // No preview card shows a subtitle, so the row is single line.
                section(Provider::Deezer, 0, &["", "   "]),
                // One visible subtitle among the preview cards stretches
                // the whole carousel row to two lines.
                section(Provider::Deezer, 1, &["", "Artist Name"]),
                // The provider placeholder subtitle stays hidden.
                section(Provider::Deezer, 2, &["SoundCloud"]),
            ],
        );
        cache.record(
            Provider::SoundCloud,
            &[section(Provider::SoundCloud, 0, &["Playlist author"])],
        );
        flush_write_behind();

        let path = cache_path(temp.path(), account_scope);
        let bytes = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(account_scope));
        assert!(!path.to_string_lossy().contains(account_scope));

        let loaded = SkeletonShapesCache::load_from_dir(temp.path(), account_scope);
        assert!(loaded.single_line(Provider::Deezer, 0));
        assert!(!loaded.single_line(Provider::Deezer, 1));
        assert!(loaded.single_line(Provider::Deezer, 2));
        assert!(!loaded.single_line(Provider::SoundCloud, 0));
        // Unknown positions default to the two-line shape.
        assert!(!loaded.single_line(Provider::Deezer, 3));
        assert!(!loaded.single_line(Provider::SoundCloud, 1));

        // Another account scope reads its own file.
        let other = SkeletonShapesCache::load_from_dir(temp.path(), "account-two");
        assert!(!other.single_line(Provider::Deezer, 0));
        assert_ne!(
            cache_path(temp.path(), account_scope),
            cache_path(temp.path(), "account-two")
        );
    }

    #[test]
    fn only_preview_cards_decide_the_row_shape() {
        let mut cache = SkeletonShapesCache::default();
        // Cards beyond the preview limit never render, so a subtitle there
        // cannot stretch the row.
        let mut subtitles = vec![""; super::super::view::DISCOVER_CARD_PREVIEW_LIMIT];
        subtitles.push("Hidden author");
        cache.record(
            Provider::Deezer,
            &[section(Provider::Deezer, 0, &subtitles)],
        );
        assert!(cache.single_line(Provider::Deezer, 0));

        // A visible subtitle inside the preview window makes the row two
        // line.
        let mut subtitles = vec![""; super::super::view::DISCOVER_CARD_PREVIEW_LIMIT - 1];
        subtitles.push("Visible author");
        cache.record(
            Provider::Deezer,
            &[section(Provider::Deezer, 0, &subtitles)],
        );
        assert!(!cache.single_line(Provider::Deezer, 0));
    }

    #[test]
    fn caps_corrupt_and_foreign_documents_are_tolerated() {
        let temp = TempDir::new().unwrap();
        let scope = "account";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();

        // Recording more sections than the position cap keeps only the
        // first cap many positions.
        let mut cache = SkeletonShapesCache::load_from_dir(temp.path(), scope);
        let sections = (0..(MAX_SECTION_POSITIONS + 8))
            .map(|index| {
                let subtitles = if index % 2 == 0 {
                    &[""][..]
                } else {
                    &["Author"][..]
                };
                section(Provider::Deezer, index, subtitles)
            })
            .collect::<Vec<_>>();
        cache.record(Provider::Deezer, &sections);
        flush_write_behind();
        let loaded = SkeletonShapesCache::load_from_dir(temp.path(), scope);
        for index in 0..MAX_SECTION_POSITIONS {
            assert_eq!(loaded.single_line(Provider::Deezer, index), index % 2 == 0);
        }
        assert!(!loaded.single_line(Provider::Deezer, MAX_SECTION_POSITIONS));

        // A foreign schema yields an empty cache.
        fs::write(
            &path,
            serde_json::json!({
                "schema": "foreign.cache",
                "version": CACHE_VERSION,
                "deezer": [true],
                "soundcloud": [true]
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            !SkeletonShapesCache::load_from_dir(temp.path(), scope)
                .single_line(Provider::Deezer, 0)
        );

        // An unknown version is foreign to this build.
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION + 1,
                "deezer": [true],
                "soundcloud": []
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            !SkeletonShapesCache::load_from_dir(temp.path(), scope)
                .single_line(Provider::Deezer, 0)
        );

        // Non boolean values are ignored; the booleans keep their order.
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "deezer": [true, 1, "yes", null, false],
                "soundcloud": []
            })
            .to_string(),
        )
        .unwrap();
        let loaded = SkeletonShapesCache::load_from_dir(temp.path(), scope);
        assert!(loaded.single_line(Provider::Deezer, 0));
        assert!(!loaded.single_line(Provider::Deezer, 1));

        // Corrupt JSON yields an empty cache.
        fs::write(&path, b"not json").unwrap();
        assert!(
            !SkeletonShapesCache::load_from_dir(temp.path(), scope)
                .single_line(Provider::Deezer, 0)
        );

        // An oversized file is rejected even when its content would parse.
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "deezer": [true],
                "soundcloud": [],
                "padding": "x".repeat(MAX_CACHE_FILE_BYTES)
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            !SkeletonShapesCache::load_from_dir(temp.path(), scope)
                .single_line(Provider::Deezer, 0)
        );
    }
}
