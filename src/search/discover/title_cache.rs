use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

#[cfg(test)]
use super::cache_store::flush_write_behind;
use super::cache_store::{WriteHookSlot, schedule_write};
use super::model::{DiscoverAction, DiscoverSection};
use crate::smart_mix_title::specific_smart_mix_title;

const CACHE_DIRECTORY: &str = "discover-v1";
const CACHE_FILE_PREFIX: &str = "smart-titles";
const CACHE_SCHEMA: &str = "ralgrum.discover.smart-titles";
const CACHE_VERSION: u32 = 2;
const LEGACY_CACHE_VERSION: u32 = 1;
const MAX_SMART_TITLE_ENTRIES: usize = 32;
const MAX_CACHE_FILE_BYTES: usize = 64 * 1024;
const MAX_STABLE_ID_CHARS: usize = 128;

/// Test observation point for this cache's durable writes. The slot is
/// per cache, so tests of other Discover caches can never clear or fire
/// this hook, and every queued write carries it to the shared writer.
static WRITE_HOOK: WriteHookSlot = WriteHookSlot::new();

#[derive(Clone, Debug, Default)]
pub(crate) struct SmartTitleCache {
    entries: HashMap<String, String>,
    endpoint_authoritative: HashSet<String>,
    order: VecDeque<String>,
    storage_path: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct PersistedSmartTitles {
    schema: String,
    version: u32,
    entries: Vec<PersistedSmartTitle>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct PersistedSmartTitle {
    id: String,
    title: String,
    #[serde(default)]
    endpoint_authoritative: bool,
}

impl SmartTitleCache {
    pub(crate) fn load(account_scope: &str) -> Self {
        let root = crate::paths::cache_dir().join(CACHE_DIRECTORY);
        Self::load_from_dir(&root, account_scope)
    }

    fn load_from_dir(root: &Path, account_scope: &str) -> Self {
        let storage_path = cache_path(root, account_scope);
        let mut cache = Self {
            storage_path: Some(storage_path.clone()),
            ..Self::default()
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
        let Some(entries) = document
            .get("entries")
            .and_then(serde_json::Value::as_array)
        else {
            return cache;
        };
        if schema != CACHE_SCHEMA
            || !matches!(version, LEGACY_CACHE_VERSION | CACHE_VERSION)
            || entries.len() > MAX_SMART_TITLE_ENTRIES
        {
            return cache;
        }

        let mut seen = HashSet::with_capacity(entries.len());
        for raw_entry in entries {
            let Ok(entry) = serde_json::from_value::<PersistedSmartTitle>(raw_entry.clone()) else {
                continue;
            };
            let Some(id) = normalized_stable_id(&entry.id) else {
                continue;
            };
            let Some(title) = specific_smart_mix_title(&entry.title) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                return Self {
                    storage_path: Some(storage_path),
                    ..Self::default()
                };
            }
            cache.order.push_back(id.clone());
            cache.entries.insert(id.clone(), title.to_owned());
            if version == CACHE_VERSION && entry.endpoint_authoritative {
                cache.endpoint_authoritative.insert(id);
            }
        }
        if version == LEGACY_CACHE_VERSION && !cache.entries.is_empty() {
            cache.persist();
        }
        cache
    }

    pub(crate) fn apply(&mut self, sections: &mut [DiscoverSection]) {
        let mut changed = false;
        for section in sections {
            if section.provider != crate::search::Provider::Deezer {
                continue;
            }
            for item in &mut section.items {
                if !matches!(
                    item.action,
                    DiscoverAction::PlayDeezerFlow { smart_mix: true }
                ) {
                    continue;
                }
                let Some(key) = normalized_stable_id(&item.card.id) else {
                    continue;
                };
                if let Some(title) = self.endpoint_title(&key) {
                    if item.card.title != title {
                        item.card.title = title;
                        changed = true;
                    }
                } else if let Some(title) = specific_smart_mix_title(&item.card.title) {
                    changed |= self.remember(&key, title);
                } else if let Some(title) = self.get(&key) {
                    item.card.title = title;
                }
            }
        }
        if changed {
            self.persist();
        }
    }

    fn remember(&mut self, key: &str, title: &str) -> bool {
        let Some(key) = normalized_stable_id(key) else {
            return false;
        };
        let Some(title) = specific_smart_mix_title(title) else {
            return false;
        };
        if self.endpoint_authoritative.contains(&key) {
            return false;
        }
        let changed = self.entries.get(&key).is_none_or(|old| old != title);
        self.entries.insert(key.clone(), title.to_owned());
        self.touch(&key);
        self.enforce_bound();
        changed
    }

    pub(crate) fn remember_endpoint_title(
        &mut self,
        key: &str,
        title: &str,
    ) -> Option<(String, String, bool)> {
        let key = normalized_stable_id(key)?;
        let title = specific_smart_mix_title(title)?.to_owned();
        let title_changed = self.entries.get(&key).is_none_or(|old| old != &title);
        let source_changed = self.endpoint_authoritative.insert(key.clone());
        let changed = title_changed || source_changed;
        self.entries.insert(key.clone(), title.clone());
        self.touch(&key);
        self.enforce_bound();
        if changed {
            self.persist();
        }
        Some((key, title, changed))
    }

    fn endpoint_title(&mut self, key: &str) -> Option<String> {
        if !self.endpoint_authoritative.contains(key) {
            return None;
        }
        let title = self.entries.get(key).cloned()?;
        self.touch(key);
        Some(title)
    }

    fn enforce_bound(&mut self) {
        while self.order.len() > MAX_SMART_TITLE_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
                self.endpoint_authoritative.remove(&oldest);
            }
        }
    }

    fn get(&mut self, key: &str) -> Option<String> {
        let title = self.entries.get(key).cloned()?;
        self.touch(key);
        Some(title)
    }

    fn touch(&mut self, key: &str) {
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(key.to_owned());
    }

    fn persist(&self) {
        let Some(path) = self.storage_path.clone() else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&self.snapshot()) else {
            return;
        };
        schedule_write(path, bytes, &WRITE_HOOK);
    }

    fn snapshot(&self) -> PersistedSmartTitles {
        let entries = self
            .order
            .iter()
            .filter_map(|id| {
                self.entries.get(id).map(|title| PersistedSmartTitle {
                    id: id.clone(),
                    title: title.clone(),
                    endpoint_authoritative: self.endpoint_authoritative.contains(id),
                })
            })
            .collect::<Vec<_>>();
        PersistedSmartTitles {
            schema: CACHE_SCHEMA.to_owned(),
            version: CACHE_VERSION,
            entries,
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
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

fn normalized_stable_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_STABLE_ID_CHARS
        || value.chars().any(char::is_control)
    {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Card, Provider, ResultType};
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };
    use tempfile::TempDir;

    fn sections(key: &str, title: &str) -> Vec<DiscoverSection> {
        vec![DiscoverSection {
            id: "section".into(),
            provider: Provider::Deezer,
            title: "Made for you".into(),
            subtitle: String::new(),
            items: vec![crate::search::discover::DiscoverItem {
                card: Card {
                    kind: ResultType::All,
                    id: key.into(),
                    title: title.into(),
                    source: Provider::Deezer,
                    ..Card::default()
                },
                action: DiscoverAction::PlayDeezerFlow { smart_mix: true },
            }],
        }]
    }

    /// Serializes tests that install a write hook: they block the shared
    /// writer thread, so they must not overlap each other.
    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Removes the installed write hook when the test scope ends, even on
    /// panic.
    struct WriteHookGuard;

    impl Drop for WriteHookGuard {
        fn drop(&mut self) {
            WRITE_HOOK.set(None);
        }
    }

    #[test]
    fn restores_last_authoritative_title_after_placeholder_refresh() {
        let mut cache = SmartTitleCache::default();
        let mut first = sections("monthly-top", "Electro Dance");
        cache.apply(&mut first);

        let mut refreshed = sections("monthly-top", "Daily 1");
        cache.apply(&mut refreshed);
        assert_eq!(refreshed[0].items[0].card.title, "Electro Dance");
    }

    #[test]
    fn placeholders_and_generic_mix_names_are_not_cached() {
        let mut cache = SmartTitleCache::default();
        for title in ["Mix", "Flow", "Daily", "Daily 2", "Daily Mix"] {
            let mut sections = sections(title, title);
            cache.apply(&mut sections);
        }
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = SmartTitleCache::default();
        for index in 0..(MAX_SMART_TITLE_ENTRIES + 5) {
            let key = format!("mix-{index}");
            let title = format!("Title {index}");
            let mut sections = sections(&key, &title);
            cache.apply(&mut sections);
        }
        assert_eq!(cache.len(), MAX_SMART_TITLE_ENTRIES);
    }

    #[test]
    fn daily_drive_is_not_a_placeholder() {
        assert_eq!(specific_smart_mix_title("Daily Drive"), Some("Daily Drive"));
    }

    #[test]
    fn dated_daily_placeholders_are_not_authoritative() {
        assert_eq!(specific_smart_mix_title("Daily 1 - 08/09/26"), None);
    }

    #[test]
    fn endpoint_title_wins_over_a_later_uppercase_homepage_refresh() {
        let mut cache = SmartTitleCache::default();
        let mut initial = sections("inspired-by-3", "IL MEGLIO DEL MIO AGOSTO");
        cache.apply(&mut initial);
        assert_eq!(
            cache.remember_endpoint_title("inspired-by-3", "Il Meglio del Mio Agosto"),
            Some((
                "inspired-by-3".into(),
                "Il Meglio del Mio Agosto".into(),
                true
            ))
        );

        let mut refreshed = sections("inspired-by-3", "IL MEGLIO DEL MIO AGOSTO");
        cache.apply(&mut refreshed);
        assert_eq!(refreshed[0].items[0].card.title, "Il Meglio del Mio Agosto");
        assert!(cache.endpoint_authoritative.contains("inspired-by-3"));
    }

    #[test]
    fn persists_and_loads_valid_titles_without_raw_account_scope() {
        let temp = TempDir::new().unwrap();
        let account_scope = "deezer:user:secret-credential";
        let mut cache = SmartTitleCache::load_from_dir(temp.path(), account_scope);
        let mut sections = sections("monthly-top", "Electro Dance");
        cache.apply(&mut sections);
        flush_write_behind();

        let path = cache_path(temp.path(), account_scope);
        let bytes = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(account_scope));
        assert!(!path.to_string_lossy().contains(account_scope));
        let loaded = SmartTitleCache::load_from_dir(temp.path(), account_scope);
        assert_eq!(
            loaded.entries.get("monthly-top"),
            Some(&"Electro Dance".into())
        );
    }

    #[test]
    fn account_hash_isolated_cache_paths() {
        let temp = TempDir::new().unwrap();
        let mut first = SmartTitleCache::load_from_dir(temp.path(), "account-one");
        assert!(
            first
                .remember_endpoint_title("monthly-top", "Electro Dance")
                .is_some()
        );
        flush_write_behind();

        let second = SmartTitleCache::load_from_dir(temp.path(), "account-two");
        assert!(second.entries.is_empty());
        assert!(second.endpoint_authoritative.is_empty());
        assert_ne!(
            cache_path(temp.path(), "account-one"),
            cache_path(temp.path(), "account-two")
        );
    }

    #[test]
    fn endpoint_authority_persists_and_reloads() {
        let temp = TempDir::new().unwrap();
        let scope = "account-one";
        let mut cache = SmartTitleCache::load_from_dir(temp.path(), scope);
        cache
            .remember_endpoint_title("inspired-by-3", "Nuove Uscite")
            .unwrap();
        flush_write_behind();

        let mut loaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        let mut refreshed = sections("inspired-by-3", "NUOVE USCITE");
        loaded.apply(&mut refreshed);
        assert_eq!(refreshed[0].items[0].card.title, "Nuove Uscite");
        assert!(loaded.endpoint_authoritative.contains("inspired-by-3"));
    }

    #[test]
    fn mutation_returns_before_the_durable_write_completes() {
        let _serial = HOOK_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = TempDir::new().unwrap();
        let scope = "slow-disk";
        let mut cache = SmartTitleCache::load_from_dir(temp.path(), scope);
        let base = temp.path().to_path_buf();

        // Declared first so it is dropped last: uninstalling the hook must
        // wait until a blocked write has been released.
        let _guard = WriteHookGuard;
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        WRITE_HOOK.set(Some(Box::new(move |path: &Path| {
            if !path.starts_with(&base) {
                return;
            }
            let _ = started_tx.send(());
            // Simulate a slow cache directory for this test's paths only.
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
        })));

        let (returned_tx, returned_rx) = mpsc::channel();
        let mut sections = sections("monthly-top", "Electro Dance");
        let applier = thread::spawn(move || {
            cache.apply(&mut sections);
            let _ = returned_tx.send(());
            cache
        });

        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("durable write started");
        returned_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("apply returned before the durable write completed");

        release_tx.send(()).unwrap();
        let cache = applier.join().unwrap();
        assert_eq!(
            cache.entries.get("monthly-top"),
            Some(&"Electro Dance".into())
        );

        flush_write_behind();
        let reloaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        assert_eq!(
            reloaded.entries.get("monthly-top"),
            Some(&"Electro Dance".into())
        );
    }

    #[test]
    fn rapid_mutations_coalesce_into_the_final_state() {
        let _serial = HOOK_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = TempDir::new().unwrap();
        let scope = "coalesce";
        let mut cache = SmartTitleCache::load_from_dir(temp.path(), scope);
        let base = temp.path().to_path_buf();

        // Declared first so it is dropped last: uninstalling the hook must
        // wait until a blocked write has been released.
        let _guard = WriteHookGuard;
        let writes = Arc::new(AtomicUsize::new(0));
        let first = Arc::new(AtomicBool::new(true));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let counted = writes.clone();
        let gate = first.clone();
        WRITE_HOOK.set(Some(Box::new(move |path: &Path| {
            if !path.starts_with(&base) {
                return;
            }
            counted.fetch_add(1, Ordering::SeqCst);
            if gate.swap(false, Ordering::SeqCst) {
                let _ = started_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
            }
        })));

        cache
            .remember_endpoint_title("monthly-top", "Title One")
            .unwrap();
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first durable write started");

        // Rapid successive mutations while the first durable write is still
        // in flight; every snapshot replaces the pending one.
        for title in [
            "Title Two",
            "Title Three",
            "Title Four",
            "Title Five",
            "Title Six",
        ] {
            cache.remember_endpoint_title("monthly-top", title).unwrap();
        }
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert_eq!(cache.entries.get("monthly-top"), Some(&"Title Six".into()));

        release_tx.send(()).unwrap();
        flush_write_behind();

        // Six mutations collapsed into two durable writes: the one that was
        // already in flight plus one write of the final coalesced snapshot.
        assert_eq!(writes.load(Ordering::SeqCst), 2);
        let reloaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        assert_eq!(
            reloaded.entries.get("monthly-top"),
            Some(&"Title Six".into())
        );
    }

    #[test]
    fn rejects_corrupt_foreign_and_oversized_documents() {
        let temp = TempDir::new().unwrap();
        let scope = "account";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": "foreign.cache",
                "version": CACHE_VERSION,
                "entries": [{"id": "x", "title": "Title"}]
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );

        fs::write(&path, b"not json").unwrap();
        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );

        let entries = (0..=MAX_SMART_TITLE_ENTRIES)
            .map(|index| serde_json::json!({"id": format!("id-{index}"), "title": "Title"}))
            .collect::<Vec<_>>();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "entries": entries
            })
            .to_string(),
        )
        .unwrap();
        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn rejects_placeholder_and_invalid_entries() {
        let temp = TempDir::new().unwrap();
        let scope = "account";
        let path = cache_path(temp.path(), scope);
        for title in ["Mix", "Flow", "Daily", "Daily 2"] {
            let document = PersistedSmartTitles {
                schema: CACHE_SCHEMA.into(),
                version: CACHE_VERSION,
                entries: vec![PersistedSmartTitle {
                    id: "monthly-top".into(),
                    title: title.into(),
                    endpoint_authoritative: false,
                }],
            };
            fs::create_dir_all(temp.path()).unwrap();
            fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
            assert!(
                SmartTitleCache::load_from_dir(temp.path(), scope)
                    .entries
                    .is_empty()
            );
        }
        let document = PersistedSmartTitles {
            schema: CACHE_SCHEMA.into(),
            version: CACHE_VERSION,
            entries: vec![PersistedSmartTitle {
                id: "bad\nkey".into(),
                title: "Title".into(),
                endpoint_authoritative: false,
            }],
        };
        fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn migrates_v1_entries_to_v2_after_validated_load() {
        let temp = TempDir::new().unwrap();
        let scope = "legacy-account";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": LEGACY_CACHE_VERSION,
                "entries": [{"id": "inspired-by-3", "title": "Electro Dance"}]
            })
            .to_string(),
        )
        .unwrap();

        let loaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        flush_write_behind();
        assert_eq!(
            loaded.entries.get("inspired-by-3"),
            Some(&"Electro Dance".into())
        );
        assert!(!loaded.endpoint_authoritative.contains("inspired-by-3"));
        let migrated: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            migrated.get("version"),
            Some(&serde_json::json!(CACHE_VERSION))
        );
        assert_eq!(
            migrated["entries"][0]["endpoint_authoritative"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn rejects_unknown_cache_versions() {
        let temp = TempDir::new().unwrap();
        let scope = "unknown-version";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION + 1,
                "entries": [{"id": "mix", "title": "Electro Dance"}]
            })
            .to_string(),
        )
        .unwrap();

        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn invalid_rows_are_skipped_without_losing_valid_rows() {
        let temp = TempDir::new().unwrap();
        let scope = "partial-document";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "entries": [
                    {"id": "first", "title": "Electro Dance"},
                    {"id": "placeholder", "title": "Daily"},
                    {"id": 42, "title": "Malformed row"},
                    {"id": "second", "title": "Nuove Uscite"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let loaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries.get("first"), Some(&"Electro Dance".into()));
        assert_eq!(loaded.entries.get("second"), Some(&"Nuove Uscite".into()));
    }

    #[test]
    fn duplicate_stable_ids_reject_the_document_deterministically() {
        let temp = TempDir::new().unwrap();
        let scope = "duplicate-document";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "entries": [
                    {"id": "same", "title": "First"},
                    {"id": "same", "title": "Second"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        assert!(
            SmartTitleCache::load_from_dir(temp.path(), scope)
                .entries
                .is_empty()
        );
    }

    #[test]
    fn invalid_row_does_not_claim_id_from_a_later_valid_row() {
        let temp = TempDir::new().unwrap();
        let scope = "invalid-duplicate-document";
        let path = cache_path(temp.path(), scope);
        fs::create_dir_all(temp.path()).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                "schema": CACHE_SCHEMA,
                "version": CACHE_VERSION,
                "entries": [
                    {"id": "same", "title": "Daily"},
                    {"id": "same", "title": "Electro Dance"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let loaded = SmartTitleCache::load_from_dir(temp.path(), scope);
        assert_eq!(loaded.entries.get("same"), Some(&"Electro Dance".into()));
    }
}
