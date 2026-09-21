use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::search::{DeezerArl, TrackArtistRef};

use super::{
    client::root_page,
    model::{Category, Page, Track},
};

const CACHE_DIRECTORY: &str = "library-v1";
const CACHE_DOMAIN: &[u8] = b"ralgrum/deezer-tracks/v1\0";
const CACHE_SCHEMA_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CACHED_TRACKS: usize = 10_000;

#[derive(Clone)]
pub(super) struct DeezerTracksCache {
    directory: PathBuf,
    coordinator: Arc<CacheCoordinator>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct DeezerTracksCacheKey(String);

#[derive(Default)]
struct CacheCoordinator {
    locks: Mutex<HashMap<DeezerTracksCacheKey, Arc<tokio::sync::Mutex<()>>>>,
    latest_revisions: Mutex<HashMap<DeezerTracksCacheKey, u64>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredTracks {
    schema_version: u32,
    total: usize,
    tracks: Vec<StoredTrack>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredTrack {
    id: String,
    title: String,
    artist: String,
    artists: Vec<StoredArtist>,
    album: String,
    album_id: String,
    release_date: String,
    duration: u64,
    artwork: String,
    explicit: bool,
    #[serde(default)]
    ai_generated: bool,
    service_url: String,
}

#[derive(Deserialize, Serialize)]
struct StoredArtist {
    id: String,
    name: String,
}

impl DeezerTracksCache {
    pub(super) fn current_user() -> Self {
        let directory = crate::paths::cache_dir().join(CACHE_DIRECTORY);
        Self {
            directory,
            coordinator: Arc::new(CacheCoordinator::default()),
        }
    }

    #[cfg(test)]
    fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            coordinator: Arc::new(CacheCoordinator::default()),
        }
    }

    /// Bind a snapshot to the exact saved account and credential pair without
    /// exposing either value in its filename.
    pub(super) fn key(
        &self,
        arl: &DeezerArl,
        user_id: Option<&str>,
    ) -> Option<DeezerTracksCacheKey> {
        let user_id = user_id?.trim();
        if user_id.is_empty() || !user_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut digest = Sha256::new();
        digest.update(CACHE_DOMAIN);
        digest.update((user_id.len() as u64).to_le_bytes());
        digest.update(user_id.as_bytes());
        digest.update((arl.expose().len() as u64).to_le_bytes());
        digest.update(arl.expose().as_bytes());
        Some(DeezerTracksCacheKey(hex_digest(digest.finalize())))
    }

    pub(super) async fn load(&self, key: &DeezerTracksCacheKey) -> Option<Page> {
        let lock = self.lock_for(key);
        let guard = lock.clone().lock_owned().await;
        let path = self.path(key);
        let bytes = match read_bounded(&path).await {
            Ok(bytes) => bytes,
            Err(CacheReadError::Missing) => return None,
            Err(CacheReadError::Invalid) => {
                let _ = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let _ = fs::remove_file(path);
                })
                .await;
                return None;
            }
        };
        let stored: StoredTracks = match serde_json::from_slice(&bytes) {
            Ok(stored) => stored,
            Err(_) => {
                let _ = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let _ = fs::remove_file(path);
                })
                .await;
                return None;
            }
        };
        match stored.into_page() {
            Some(page) => Some(page),
            None => {
                let _ = tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    let _ = fs::remove_file(path);
                })
                .await;
                None
            }
        }
    }

    #[cfg(test)]
    pub(super) async fn store(
        &self,
        key: &DeezerTracksCacheKey,
        page: &Page,
    ) -> Result<(), String> {
        let revision = self.reserve_revision(key);
        self.store_revision(key, page, revision).await
    }

    pub(super) fn reserve_revision(&self, key: &DeezerTracksCacheKey) -> u64 {
        let mut revisions = self
            .coordinator
            .latest_revisions
            .lock()
            .expect("cache revision coordinator poisoned");
        let revision = revisions
            .get(key)
            .copied()
            .unwrap_or_default()
            .wrapping_add(1);
        revisions.insert(key.clone(), revision);
        revision
    }

    pub(super) async fn store_revision(
        &self,
        key: &DeezerTracksCacheKey,
        page: &Page,
        revision: u64,
    ) -> Result<(), String> {
        let stored = StoredTracks::from_page(page)?;
        let encoded = serde_json::to_vec(&stored)
            .map_err(|_| "Deezer Tracks cache could not be encoded".to_owned())?;
        if encoded.len() as u64 > MAX_CACHE_BYTES {
            return Err("Deezer Tracks cache exceeds its size limit".to_owned());
        }
        let lock = self.lock_for(key);
        let guard = lock.clone().lock_owned().await;
        let latest = self
            .coordinator
            .latest_revisions
            .lock()
            .expect("cache revision coordinator poisoned")
            .get(key)
            .copied()
            .unwrap_or_default();
        if revision < latest {
            return Ok(());
        }
        let path = self.path(key);
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            atomic_write(&path, &encoded)
        })
        .await
        .map_err(|_| "Deezer Tracks cache writer stopped unexpectedly".to_owned())?
        .map_err(|_| "Deezer Tracks cache could not be stored".to_owned())
    }

    fn path(&self, key: &DeezerTracksCacheKey) -> PathBuf {
        self.directory.join(format!("{}.json", key.0))
    }

    fn lock_for(&self, key: &DeezerTracksCacheKey) -> Arc<tokio::sync::Mutex<()>> {
        self.coordinator
            .locks
            .lock()
            .expect("cache lock coordinator poisoned")
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

impl StoredTracks {
    fn from_page(page: &Page) -> Result<Self, String> {
        if page.uses_sections()
            || !page.cards.is_empty()
            || page.tracks.len() > MAX_CACHED_TRACKS
            || page.total != page.tracks.len()
            || page.authoritative_total != Some(page.raw_loaded_count)
            || page.normalized_count != page.raw_loaded_count
            || page.tracks.len() != page.raw_loaded_count
        {
            return Err("Deezer Tracks snapshot is not complete".to_owned());
        }
        Ok(Self {
            schema_version: CACHE_SCHEMA_VERSION,
            total: page.total,
            tracks: page.tracks.iter().map(StoredTrack::from).collect(),
        })
    }

    fn into_page(self) -> Option<Page> {
        if self.schema_version != CACHE_SCHEMA_VERSION
            || self.tracks.len() > MAX_CACHED_TRACKS
            || self.total != self.tracks.len()
        {
            return None;
        }
        let mut page = root_page(Category::Tracks, self.total);
        page.tracks = self.tracks.into_iter().map(Track::from).collect();
        page.raw_loaded_count = page.tracks.len();
        page.normalized_count = page.tracks.len();
        page.authoritative_total = Some(self.total);
        Some(page)
    }
}

impl From<&Track> for StoredTrack {
    fn from(track: &Track) -> Self {
        Self {
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            artists: track.artists.iter().map(StoredArtist::from).collect(),
            album: track.album.clone(),
            album_id: track.album_id.clone(),
            release_date: track.release_date.clone(),
            duration: track.duration,
            artwork: track.artwork.clone(),
            explicit: track.explicit,
            ai_generated: track.ai_generated,
            service_url: track.service_url.clone(),
        }
    }
}

impl From<StoredTrack> for Track {
    fn from(track: StoredTrack) -> Self {
        Self {
            origin: None,
            id: track.id,
            title: track.title,
            artist: track.artist,
            artists: track
                .artists
                .into_iter()
                .map(TrackArtistRef::from)
                .collect(),
            album: track.album,
            album_id: track.album_id,
            release_date: track.release_date,
            duration: track.duration,
            artwork: track.artwork,
            explicit: track.explicit,
            ai_generated: track.ai_generated,
            service_url: track.service_url,
        }
    }
}

impl From<&TrackArtistRef> for StoredArtist {
    fn from(artist: &TrackArtistRef) -> Self {
        Self {
            id: artist.id.clone(),
            name: artist.name.clone(),
        }
    }
}

impl From<StoredArtist> for TrackArtistRef {
    fn from(artist: StoredArtist) -> Self {
        Self {
            id: artist.id,
            name: artist.name,
        }
    }
}

/// Enrich only tracks whose live IDs were returned by Deezer. The live page
/// always owns membership, ordering, duplicate count, totals, and completeness.
pub(super) fn enrich_live_page(cached: Option<&Page>, live: &Page) -> Page {
    let Some(cached) = cached else {
        return live.clone();
    };
    let mut cached_by_id: std::collections::HashMap<&str, Vec<&Track>> =
        std::collections::HashMap::new();
    for track in &cached.tracks {
        let id = track.id.trim();
        if !id.is_empty() {
            cached_by_id.entry(id).or_default().push(track);
        }
    }
    let mut occurrence_by_id = std::collections::HashMap::<String, usize>::new();
    let mut page = live.clone();
    for track in &mut page.tracks {
        let id = track.id.trim().to_owned();
        if id.is_empty() {
            continue;
        }
        let occurrence = occurrence_by_id.entry(id.clone()).or_default();
        if let Some(cached_track) = cached_by_id
            .get(id.as_str())
            .and_then(|tracks| tracks.get(*occurrence))
        {
            let ai_generated = track.ai_generated;
            *track = (*cached_track).clone();
            track.ai_generated = ai_generated;
        }
        *occurrence += 1;
    }
    page
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

enum CacheReadError {
    Missing,
    Invalid,
}

async fn read_bounded(path: &Path) -> Result<Vec<u8>, CacheReadError> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CacheReadError::Missing);
        }
        Err(_) => return Err(CacheReadError::Invalid),
    };
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| CacheReadError::Invalid)?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return Err(CacheReadError::Invalid);
    }
    Ok(bytes)
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing cache directory"))?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid cache file name"))?;
    for _ in 0..8 {
        let temporary = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
        match write_new_synced(&temporary, contents) {
            Ok(()) => {
                let result = atomic_rename(&temporary, path);
                if result.is_err() {
                    let _ = fs::remove_file(&temporary);
                }
                return result;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary cache file",
    ))
}

fn write_new_synced(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(windows)]
fn atomic_rename(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: Both paths are null-terminated UTF-16 buffers that remain alive for this call.
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

    fn fixture_page(marker: &str) -> Page {
        let mut page = root_page(Category::Tracks, 2);
        page.tracks = vec![
            Track {
                origin: None,
                id: "7".into(),
                title: format!("First {marker}"),
                artist: "One, Two".into(),
                artists: vec![
                    TrackArtistRef {
                        id: "11".into(),
                        name: "One".into(),
                    },
                    TrackArtistRef {
                        id: "12".into(),
                        name: "Two".into(),
                    },
                ],
                album: "Album".into(),
                album_id: "30".into(),
                release_date: "2026-08-27".into(),
                duration: 123,
                artwork: "https://example.test/cover.jpg".into(),
                explicit: true,
                ai_generated: true,
                service_url: String::new(),
            },
            Track {
                id: "7".into(),
                title: "Duplicate".into(),
                artist: "One".into(),
                duration: 456,
                ..Track::default()
            },
        ];
        page.raw_loaded_count = page.tracks.len();
        page.normalized_count = page.tracks.len();
        page.authoritative_total = Some(page.tracks.len());
        page
    }

    fn key(cache: &DeezerTracksCache) -> DeezerTracksCacheKey {
        cache
            .key(
                &DeezerArl::from_saved("sentinel-secret").unwrap(),
                Some("12345678901234567890"),
            )
            .unwrap()
    }

    #[test]
    fn key_is_scoped_without_exposing_account_or_credential() {
        let cache = DeezerTracksCache::new(PathBuf::from("cache"));
        let key = key(&cache);
        let path = cache.path(&key).to_string_lossy().into_owned();
        assert!(!path.contains("sentinel-secret"));
        assert!(!path.contains("12345678901234567890"));
        assert_ne!(
            key.0,
            cache
                .key(
                    &DeezerArl::from_saved("another-secret").unwrap(),
                    Some("12345678901234567890")
                )
                .unwrap()
                .0
        );
        assert_ne!(
            key.0,
            cache
                .key(
                    &DeezerArl::from_saved("sentinel-secret").unwrap(),
                    Some("98765432109876543210")
                )
                .unwrap()
                .0
        );
    }

    #[tokio::test]
    async fn snapshot_round_trip_preserves_order_duplicates_and_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        let page = fixture_page("cached");
        cache.store(&key, &page).await.unwrap();
        let loaded = cache.load(&key).await.unwrap();
        assert_eq!(loaded.total, 2);
        assert_eq!(loaded.tracks, page.tracks);
    }

    #[tokio::test]
    async fn atomic_store_replaces_an_existing_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        cache.store(&key, &fixture_page("old")).await.unwrap();
        cache.store(&key, &fixture_page("new")).await.unwrap();
        assert_eq!(cache.load(&key).await.unwrap().tracks[0].title, "First new");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn newer_snapshot_wins_when_detached_writers_finish_out_of_order() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        let old = fixture_page("old");
        let new = fixture_page("new");
        let old_revision = cache.reserve_revision(&key);
        let new_revision = cache.reserve_revision(&key);

        let (new_result, old_result) = tokio::join!(
            cache.store_revision(&key, &new, new_revision),
            cache.store_revision(&key, &old, old_revision),
        );
        new_result.unwrap();
        old_result.unwrap();
        assert_eq!(cache.load(&key).await.unwrap().tracks[0].title, "First new");
    }

    #[tokio::test]
    async fn invalid_read_cleanup_cannot_delete_a_concurrent_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        fs::write(cache.path(&key), b"not json").unwrap();
        let page = fixture_page("replacement");

        let (loaded, stored) = tokio::join!(cache.load(&key), cache.store(&key, &page));
        assert!(loaded.is_none());
        stored.unwrap();
        assert_eq!(
            cache.load(&key).await.unwrap().tracks[0].title,
            "First replacement"
        );
    }

    #[tokio::test]
    async fn corrupt_and_wrong_version_snapshots_are_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        fs::create_dir_all(directory.path()).unwrap();
        fs::write(cache.path(&key), b"not json").unwrap();
        assert!(cache.load(&key).await.is_none());

        let mut stored = StoredTracks::from_page(&fixture_page("version")).unwrap();
        stored.schema_version += 1;
        fs::write(cache.path(&key), serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(cache.load(&key).await.is_none());
        assert!(!cache.path(&key).exists());
    }

    #[tokio::test]
    async fn oversized_snapshot_is_read_only_to_the_bound_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let cache = DeezerTracksCache::new(directory.path().into());
        let key = key(&cache);
        fs::create_dir_all(directory.path()).unwrap();
        fs::write(cache.path(&key), vec![b'x'; MAX_CACHE_BYTES as usize + 1]).unwrap();
        assert!(cache.load(&key).await.is_none());
        assert!(!cache.path(&key).exists());
    }

    #[test]
    fn only_complete_root_track_pages_are_serialized() {
        let mut partial = fixture_page("partial");
        partial.total = 3;
        assert!(StoredTracks::from_page(&partial).is_err());
        let mut cards = fixture_page("cards");
        cards.cards.push(super::super::model::Card::default());
        assert!(StoredTracks::from_page(&cards).is_err());
        let mut mismatched_counts = fixture_page("counts");
        mismatched_counts.raw_loaded_count = 1;
        assert!(StoredTracks::from_page(&mismatched_counts).is_err());
    }

    #[test]
    fn enrichment_never_introduces_cached_membership_or_order() {
        let cached = fixture_page("cached metadata");
        let mut live = fixture_page("live metadata");
        live.tracks.remove(0);
        live.tracks.push(Track {
            id: "8".into(),
            title: "Fresh only".into(),
            ..Track::default()
        });
        live.total = live.tracks.len();
        live.raw_loaded_count = live.tracks.len();
        live.normalized_count = live.tracks.len();
        live.authoritative_total = Some(live.tracks.len());

        let enriched = enrich_live_page(Some(&cached), &live);
        assert_eq!(
            enriched
                .tracks
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            vec!["7", "8"]
        );
        assert_eq!(enriched.tracks[0].title, "First cached metadata");
        assert_eq!(enriched.tracks[1].title, "Fresh only");
        assert_eq!(enriched.total, live.total);
    }

    #[test]
    fn enrichment_matches_duplicate_occurrences_without_changing_live_shape() {
        let mut cached = fixture_page("rich cache");
        cached.tracks[0].ai_generated = false;
        let live = fixture_page("raw response");
        let enriched = enrich_live_page(Some(&cached), &live);
        assert_eq!(enriched.tracks[0].title, "First rich cache");
        assert!(enriched.tracks[0].ai_generated);
        assert_eq!(enriched.tracks[1].title, "Duplicate");
        assert_eq!(enrich_live_page(None, &live).tracks, live.tracks);
    }
}
