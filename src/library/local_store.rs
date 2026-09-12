use std::{
    collections::HashSet,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    app::paths,
    search::{Provider, TrackArtistRef},
};

const FORMAT: &str = "ralgrum-local-library";
const SCHEMA_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const PRIMARY_FILE: &str = "local_library.json";
const BACKUP_FILE: &str = "local_library.backup.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum StoredProvider {
    Deezer,
    Soundcloud,
}

impl From<Provider> for StoredProvider {
    fn from(value: Provider) -> Self {
        match value {
            Provider::Deezer => Self::Deezer,
            Provider::SoundCloud => Self::Soundcloud,
        }
    }
}

impl From<StoredProvider> for Provider {
    fn from(value: StoredProvider) -> Self {
        match value {
            StoredProvider::Deezer => Self::Deezer,
            StoredProvider::Soundcloud => Self::SoundCloud,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum LocalItemKind {
    Track,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredArtist {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredItem {
    kind: LocalItemKind,
    provider: StoredProvider,
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
    service_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalEnvelope {
    format: String,
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    items: Vec<StoredItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalTrack {
    pub(crate) provider: Provider,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) artists: Vec<TrackArtistRef>,
    pub(crate) album: String,
    pub(crate) album_id: String,
    pub(crate) release_date: String,
    pub(crate) duration: u64,
    pub(crate) artwork: String,
    pub(crate) explicit: bool,
    pub(crate) service_url: String,
}

impl LocalTrack {
    fn into_stored(self) -> StoredItem {
        StoredItem {
            kind: LocalItemKind::Track,
            provider: self.provider.into(),
            id: self.id,
            title: self.title,
            artist: self.artist,
            artists: self
                .artists
                .into_iter()
                .map(|artist| StoredArtist {
                    id: artist.id,
                    name: artist.name,
                })
                .collect(),
            album: self.album,
            album_id: self.album_id,
            release_date: self.release_date,
            duration: self.duration,
            artwork: self.artwork,
            explicit: self.explicit,
            service_url: self.service_url,
        }
    }
}

impl From<&LocalTrack> for super::model::Track {
    fn from(track: &LocalTrack) -> Self {
        Self {
            origin: Some(track.provider),
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            artists: track.artists.clone(),
            album: track.album.clone(),
            album_id: track.album_id.clone(),
            release_date: track.release_date.clone(),
            duration: track.duration,
            artwork: track.artwork.clone(),
            explicit: track.explicit,
            service_url: track.service_url.clone(),
        }
    }
}

impl From<&crate::playback::PlaybackTrack> for LocalTrack {
    fn from(track: &crate::playback::PlaybackTrack) -> Self {
        Self {
            provider: match track.provider {
                crate::playback::PlaybackProvider::Deezer => Provider::Deezer,
                crate::playback::PlaybackProvider::SoundCloud => Provider::SoundCloud,
            },
            id: track.id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            artists: track.artists.clone(),
            album: track.album.clone(),
            album_id: track.album_id.clone(),
            release_date: track.release_date.clone(),
            duration: track.duration.as_secs(),
            artwork: track.artwork.clone(),
            explicit: track.explicit,
            service_url: track.service_url.clone(),
        }
    }
}

impl TryFrom<StoredItem> for LocalTrack {
    type Error = LocalLibraryError;

    fn try_from(mut item: StoredItem) -> Result<Self, Self::Error> {
        item.id = normalize_id(item.id)?;
        item.title = normalize_text(item.title)?;
        item.artist = normalize_text(item.artist)?;
        item.album = normalize_text(item.album)?;
        item.album_id = normalize_text(item.album_id)?;
        item.release_date = normalize_text(item.release_date)?;
        item.artwork = normalize_text(item.artwork)?;
        item.service_url = normalize_text(item.service_url)?;
        let artists = item
            .artists
            .into_iter()
            .map(|mut artist| {
                artist.id = normalize_text(artist.id)?;
                artist.name = normalize_text(artist.name)?;
                Ok(TrackArtistRef {
                    id: artist.id,
                    name: artist.name,
                })
            })
            .collect::<Result<Vec<_>, LocalLibraryError>>()?;
        Ok(Self {
            provider: item.provider.into(),
            id: item.id,
            title: item.title,
            artist: item.artist,
            artists,
            album: item.album,
            album_id: item.album_id,
            release_date: item.release_date,
            duration: item.duration,
            artwork: item.artwork,
            explicit: item.explicit,
            service_url: item.service_url,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LocalLibraryStore {
    directory: PathBuf,
    tracks: Vec<LocalTrack>,
}

impl LocalLibraryStore {
    #[allow(dead_code)]
    pub(crate) fn load_current_user() -> Result<Self, LocalLibraryError> {
        let directory = paths::config_dir()
            .ok_or(LocalLibraryError::Unavailable)?
            .join("local_library");
        Self::load_from_directory(&directory)
    }

    pub(crate) fn load_from_directory(directory: &Path) -> Result<Self, LocalLibraryError> {
        Self::load(directory)
    }

    fn load(directory: &Path) -> Result<Self, LocalLibraryError> {
        fs::create_dir_all(directory).map_err(|_| LocalLibraryError::Filesystem)?;
        let primary = read_file(&directory.join(PRIMARY_FILE))?;
        let backup = read_file(&directory.join(BACKUP_FILE))?;
        let tracks = match (primary, backup) {
            (StoredFile::Valid(primary), StoredFile::Valid(backup)) => {
                if primary != backup {
                    write_atomic(&directory.join(BACKUP_FILE), &encode_tracks(&primary)?)?;
                }
                primary
            }
            (StoredFile::Valid(items), StoredFile::Missing | StoredFile::Invalid(_)) => {
                write_atomic(&directory.join(BACKUP_FILE), &encode_tracks(&items)?)?;
                items
            }
            (StoredFile::Missing | StoredFile::Invalid(_), StoredFile::Valid(items)) => {
                write_atomic(&directory.join(PRIMARY_FILE), &encode_tracks(&items)?)?;
                items
            }
            (StoredFile::Missing, StoredFile::Missing) => Vec::new(),
            (StoredFile::Missing, StoredFile::Invalid(error))
            | (StoredFile::Invalid(error), StoredFile::Missing)
            | (StoredFile::Invalid(error), StoredFile::Invalid(_)) => return Err(error),
        };
        Ok(Self {
            directory: directory.to_owned(),
            tracks,
        })
    }

    pub(crate) fn tracks(&self) -> &[LocalTrack] {
        &self.tracks
    }

    #[allow(dead_code)]
    pub(crate) fn add_track(&mut self, track: LocalTrack) -> Result<(), LocalLibraryError> {
        let track = normalize_track(track)?;
        if self
            .tracks
            .iter()
            .any(|existing| existing.provider == track.provider && existing.id == track.id)
        {
            return Err(LocalLibraryError::DuplicateItem);
        }
        let mut tracks = self.tracks.clone();
        tracks.push(track);
        self.persist_tracks(tracks)
    }

    #[allow(dead_code)]
    pub(crate) fn upsert_track(&mut self, track: LocalTrack) -> Result<(), LocalLibraryError> {
        let track = normalize_track(track)?;
        let mut tracks = self.tracks.clone();
        if let Some(existing) = tracks
            .iter_mut()
            .find(|existing| existing.provider == track.provider && existing.id == track.id)
        {
            *existing = track;
        } else {
            tracks.push(track);
        }
        self.persist_tracks(tracks)
    }

    #[allow(dead_code)]
    pub(crate) fn remove_track(
        &mut self,
        provider: Provider,
        id: &str,
    ) -> Result<bool, LocalLibraryError> {
        let id = normalize_id(id.to_owned())?;
        let mut tracks = self.tracks.clone();
        let before = tracks.len();
        tracks.retain(|track| !(track.provider == provider && track.id == id));
        if before == tracks.len() {
            return Ok(false);
        }
        self.persist_tracks(tracks)?;
        Ok(true)
    }

    pub(crate) fn reorder_track(
        &mut self,
        from: usize,
        to: usize,
    ) -> Result<bool, LocalLibraryError> {
        if from >= self.tracks.len() || to >= self.tracks.len() || from == to {
            return Ok(false);
        }
        let mut tracks = self.tracks.clone();
        let track = tracks.remove(from);
        tracks.insert(to, track);
        self.persist_tracks(tracks)?;
        Ok(true)
    }

    fn persist_tracks(&mut self, tracks: Vec<LocalTrack>) -> Result<(), LocalLibraryError> {
        let encoded = encode_tracks(&tracks)?;
        fs::create_dir_all(&self.directory).map_err(|_| LocalLibraryError::Filesystem)?;
        write_atomic(&self.directory.join(BACKUP_FILE), &encoded)?;
        write_atomic(&self.directory.join(PRIMARY_FILE), &encoded)?;
        self.tracks = tracks;
        Ok(())
    }
}

fn normalize_track(mut track: LocalTrack) -> Result<LocalTrack, LocalLibraryError> {
    track.id = normalize_id(track.id)?;
    track.title = normalize_text(track.title)?;
    track.artist = normalize_text(track.artist)?;
    track.album = normalize_text(track.album)?;
    track.album_id = normalize_text(track.album_id)?;
    track.release_date = normalize_text(track.release_date)?;
    track.artwork = normalize_text(track.artwork)?;
    track.service_url = normalize_text(track.service_url)?;
    for artist in &mut track.artists {
        artist.id = normalize_text(std::mem::take(&mut artist.id))?;
        artist.name = normalize_text(std::mem::take(&mut artist.name))?;
    }
    Ok(track)
}

fn normalize_id(value: String) -> Result<String, LocalLibraryError> {
    let value = value.trim().to_owned();
    if value.is_empty()
        || value.len() > 256
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(LocalLibraryError::InvalidItem);
    }
    Ok(value)
}

fn normalize_text(value: String) -> Result<String, LocalLibraryError> {
    let value = value.trim().to_owned();
    if value.len() > 16 * 1024 || value.chars().any(char::is_control) {
        return Err(LocalLibraryError::InvalidItem);
    }
    Ok(value)
}

fn decode_items(items: Vec<StoredItem>) -> Result<Vec<LocalTrack>, LocalLibraryError> {
    let mut seen = HashSet::new();
    let mut tracks = Vec::with_capacity(items.len());
    for item in items {
        let track = LocalTrack::try_from(item)?;
        if !seen.insert((track.provider, LocalItemKind::Track, track.id.clone())) {
            return Err(LocalLibraryError::DuplicateItem);
        }
        tracks.push(track);
    }
    Ok(tracks)
}

fn encode_items(items: &[StoredItem]) -> Result<Vec<u8>, LocalLibraryError> {
    serde_json::to_vec_pretty(&LocalEnvelope {
        format: FORMAT.into(),
        schema_version: SCHEMA_VERSION,
        items: items.to_vec(),
    })
    .map_err(|_| LocalLibraryError::Serialization)
}

fn encode_tracks(tracks: &[LocalTrack]) -> Result<Vec<u8>, LocalLibraryError> {
    let items = tracks
        .iter()
        .cloned()
        .map(LocalTrack::into_stored)
        .collect::<Vec<_>>();
    encode_items(&items)
}

enum StoredFile {
    Missing,
    Valid(Vec<LocalTrack>),
    Invalid(LocalLibraryError),
}

fn read_file(path: &Path) -> Result<StoredFile, LocalLibraryError> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(StoredFile::Missing),
        Err(_) => return Err(LocalLibraryError::Filesystem),
    };
    let metadata = file.metadata().map_err(|_| LocalLibraryError::Filesystem)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Ok(StoredFile::Invalid(LocalLibraryError::InvalidFile));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalLibraryError::Filesystem)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(StoredFile::Invalid(LocalLibraryError::InvalidFile));
    }
    let envelope = match serde_json::from_slice::<LocalEnvelope>(&bytes) {
        Ok(envelope) => envelope,
        Err(_) => return Ok(StoredFile::Invalid(LocalLibraryError::InvalidFile)),
    };
    if envelope.format != FORMAT || envelope.schema_version != SCHEMA_VERSION {
        return Ok(StoredFile::Invalid(LocalLibraryError::InvalidFile));
    }
    match decode_items(envelope.items) {
        Ok(tracks) => Ok(StoredFile::Valid(tracks)),
        Err(error) => Ok(StoredFile::Invalid(error)),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LocalLibraryError> {
    let parent = path.parent().ok_or(LocalLibraryError::Filesystem)?;
    fs::create_dir_all(parent).map_err(|_| LocalLibraryError::Filesystem)?;
    let mut temp_path = None;
    let mut file = None;
    for attempt in 0..16u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("local"),
            std::process::id(),
            attempt
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(opened) => {
                temp_path = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(LocalLibraryError::Filesystem),
        }
    }
    let temp_path = temp_path.ok_or(LocalLibraryError::Filesystem)?;
    let result = (|| {
        let mut file = file.take().ok_or(LocalLibraryError::Filesystem)?;
        file.write_all(bytes)
            .map_err(|_| LocalLibraryError::Filesystem)?;
        file.sync_all().map_err(|_| LocalLibraryError::Filesystem)?;
        atomic_replace(&temp_path, path).map_err(|_| LocalLibraryError::Filesystem)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW};
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalLibraryError {
    Unavailable,
    Filesystem,
    InvalidFile,
    InvalidItem,
    DuplicateItem,
    Serialization,
}

impl fmt::Display for LocalLibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Local library storage is unavailable.",
            Self::Filesystem => "Local library storage could not be accessed.",
            Self::InvalidFile => "The local library file is invalid.",
            Self::InvalidItem => "The local library contains an invalid track.",
            Self::DuplicateItem => "The local library contains duplicate tracks.",
            Self::Serialization => "The local library could not be prepared.",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use super::*;

    fn track(provider: Provider, id: &str) -> LocalTrack {
        LocalTrack {
            provider,
            id: id.into(),
            title: format!("Track {id}"),
            artist: "Artist".into(),
            artists: vec![TrackArtistRef {
                id: "artist".into(),
                name: "Artist".into(),
            }],
            album: "Album".into(),
            album_id: "album".into(),
            release_date: "2024".into(),
            duration: 180,
            artwork: "artwork".into(),
            explicit: false,
            service_url: "https://example.test/track".into(),
        }
    }

    fn load_store(temp: &TempDir) -> LocalLibraryStore {
        LocalLibraryStore::load_from_directory(temp.path()).unwrap()
    }

    fn write(path: &Path, value: &str) {
        fs::write(path, value).unwrap();
    }

    #[test]
    fn missing_files_start_with_an_empty_local_library() {
        let temp = TempDir::new().unwrap();
        let store = load_store(&temp);
        assert!(store.tracks().is_empty());
    }

    #[test]
    fn round_trip_preserves_normalized_track_metadata() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        let mut track = track(Provider::Deezer, " 42 ");
        track.title = "  Title  ".into();
        store.add_track(track).unwrap();

        let loaded = load_store(&temp);
        assert_eq!(loaded.tracks()[0].id, "42");
        assert_eq!(loaded.tracks()[0].title, "Title");
        assert_eq!(loaded.tracks()[0].provider, Provider::Deezer);
    }

    #[test]
    fn playback_track_conversion_keeps_mixed_provider_metadata() {
        let playback = crate::playback::PlaybackTrack {
            provider: crate::playback::PlaybackProvider::SoundCloud,
            id: "sc-42".into(),
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_id: "album-42".into(),
            release_date: "2024-02-03".into(),
            artists: vec![TrackArtistRef {
                id: "artist-42".into(),
                name: "Artist".into(),
            }],
            artwork: "https://cdn.example/cover.jpg".into(),
            duration: std::time::Duration::from_secs(241),
            downloadable: true,
            progressive: true,
            explicit: true,
            service_url: "https://soundcloud.example/track".into(),
        };
        let local = LocalTrack::from(&playback);
        assert_eq!(local.provider, Provider::SoundCloud);
        assert_eq!(local.id, playback.id);
        assert_eq!(local.title, playback.title);
        assert_eq!(local.artist, playback.artist);
        assert_eq!(local.artists, playback.artists);
        assert_eq!(local.album, playback.album);
        assert_eq!(local.album_id, playback.album_id);
        assert_eq!(local.release_date, playback.release_date);
        assert_eq!(local.duration, 241);
        assert_eq!(local.artwork, playback.artwork);
        assert_eq!(local.explicit, playback.explicit);
        assert_eq!(local.service_url, playback.service_url);
    }

    #[test]
    fn same_id_from_each_provider_can_coexist() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "42")).unwrap();
        store.add_track(track(Provider::SoundCloud, "42")).unwrap();
        assert_eq!(load_store(&temp).tracks().len(), 2);
    }

    #[test]
    fn duplicate_normalized_ids_are_rejected() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "42")).unwrap();
        assert!(matches!(
            store.add_track(track(Provider::Deezer, " 42 ")),
            Err(LocalLibraryError::DuplicateItem)
        ));
    }

    #[test]
    fn upsert_replaces_only_the_same_provider_identity() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "42")).unwrap();
        let mut replacement = track(Provider::Deezer, "42");
        replacement.title = "Replacement".into();
        store.upsert_track(replacement).unwrap();

        assert_eq!(store.tracks().len(), 1);
        assert_eq!(store.tracks()[0].title, "Replacement");
    }

    #[test]
    fn remove_track_persists_by_provider_identity() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "42")).unwrap();
        store.add_track(track(Provider::SoundCloud, "42")).unwrap();

        assert!(store.remove_track(Provider::Deezer, "42").unwrap());
        let loaded = load_store(&temp);
        assert_eq!(loaded.tracks().len(), 1);
        assert_eq!(loaded.tracks()[0].provider, Provider::SoundCloud);
        assert!(!store.remove_track(Provider::Deezer, "42").unwrap());
    }

    #[test]
    fn reorder_track_persists_the_requested_order() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "1")).unwrap();
        store.add_track(track(Provider::SoundCloud, "2")).unwrap();
        store.add_track(track(Provider::Deezer, "3")).unwrap();

        assert!(store.reorder_track(0, 2).unwrap());
        assert_eq!(
            load_store(&temp)
                .tracks()
                .iter()
                .map(|track| track.id.as_str())
                .collect::<Vec<_>>(),
            vec!["2", "3", "1"]
        );
        assert!(!store.reorder_track(1, 1).unwrap());
        assert!(!store.reorder_track(9, 0).unwrap());
    }

    #[test]
    fn malformed_marker_version_unknown_fields_and_oversized_files_are_rejected() {
        let cases = [
            r#"{}"#,
            r#"{"format":"wrong","schemaVersion":1,"items":[]}"#,
            r#"{"format":"ralgrum-local-library","schemaVersion":2,"items":[]}"#,
            r#"{"format":"ralgrum-local-library","schemaVersion":1,"items":[],"auth":"token"}"#,
            "not json",
            r#"{"format":"ralgrum-local-library","schemaVersion":1,"items":[]}{}"#,
        ];
        for (index, value) in cases.into_iter().enumerate() {
            let temp = TempDir::new().unwrap();
            write(&temp.path().join(PRIMARY_FILE), value);
            write(&temp.path().join(BACKUP_FILE), value);
            assert!(
                matches!(
                    LocalLibraryStore::load_from_directory(temp.path()),
                    Err(LocalLibraryError::InvalidFile)
                ),
                "case {index}"
            );
        }

        let temp = TempDir::new().unwrap();
        let oversized = "x".repeat(MAX_FILE_BYTES as usize + 1);
        write(&temp.path().join(PRIMARY_FILE), &oversized);
        write(&temp.path().join(BACKUP_FILE), &oversized);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::InvalidFile)
        ));
    }

    #[test]
    fn invalid_ids_and_duplicate_file_items_are_rejected() {
        let temp = TempDir::new().unwrap();
        let item = serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        let value = serde_json::json!({
            "format": FORMAT,
            "schemaVersion": SCHEMA_VERSION,
            "items": [item.clone(), item]
        });
        let encoded = serde_json::to_string(&value).unwrap();
        write(&temp.path().join(PRIMARY_FILE), &encoded);
        write(&temp.path().join(BACKUP_FILE), &encoded);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::DuplicateItem)
        ));

        let temp = TempDir::new().unwrap();
        let mut item = serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        item["id"] = serde_json::Value::String("bad id".into());
        let value = serde_json::json!({
            "format": FORMAT,
            "schemaVersion": SCHEMA_VERSION,
            "items": [item]
        });
        let encoded = serde_json::to_string(&value).unwrap();
        write(&temp.path().join(PRIMARY_FILE), &encoded);
        write(&temp.path().join(BACKUP_FILE), &encoded);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::InvalidItem)
        ));

        let temp = TempDir::new().unwrap();
        let mut item = serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        item["id"] = serde_json::Value::String("  ".into());
        let value = serde_json::json!({
            "format": FORMAT,
            "schemaVersion": SCHEMA_VERSION,
            "items": [item]
        });
        let encoded = serde_json::to_string(&value).unwrap();
        write(&temp.path().join(PRIMARY_FILE), &encoded);
        write(&temp.path().join(BACKUP_FILE), &encoded);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::InvalidItem)
        ));
    }

    #[test]
    fn unknown_item_fields_and_non_track_kinds_are_rejected() {
        let temp = TempDir::new().unwrap();
        let mut item = serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        item["unexpected"] = serde_json::Value::Bool(true);
        let value = serde_json::json!({
            "format": FORMAT,
            "schemaVersion": SCHEMA_VERSION,
            "items": [item]
        });
        let encoded = serde_json::to_string(&value).unwrap();
        write(&temp.path().join(PRIMARY_FILE), &encoded);
        write(&temp.path().join(BACKUP_FILE), &encoded);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::InvalidFile)
        ));

        let temp = TempDir::new().unwrap();
        let mut item = serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        item["kind"] = serde_json::Value::String("album".into());
        let value = serde_json::json!({
            "format": FORMAT,
            "schemaVersion": SCHEMA_VERSION,
            "items": [item]
        });
        let encoded = serde_json::to_string(&value).unwrap();
        write(&temp.path().join(PRIMARY_FILE), &encoded);
        write(&temp.path().join(BACKUP_FILE), &encoded);
        assert!(matches!(
            LocalLibraryStore::load_from_directory(temp.path()),
            Err(LocalLibraryError::InvalidFile)
        ));
    }

    #[test]
    fn valid_backup_recovers_a_corrupt_primary() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::SoundCloud, "7")).unwrap();
        write(&temp.path().join(PRIMARY_FILE), "broken");

        let loaded = load_store(&temp);
        assert_eq!(loaded.tracks()[0].id, "7");
        assert!(
            serde_json::from_slice::<LocalEnvelope>(
                &fs::read(temp.path().join(PRIMARY_FILE)).unwrap()
            )
            .is_ok()
        );
    }

    #[test]
    fn valid_backup_recovers_a_structurally_valid_primary_with_invalid_items() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::SoundCloud, "7")).unwrap();
        let mut invalid =
            serde_json::to_value(track(Provider::Deezer, "42").into_stored()).unwrap();
        invalid["id"] = serde_json::Value::String("bad id".into());
        write(
            &temp.path().join(PRIMARY_FILE),
            &serde_json::json!({
                "format": FORMAT,
                "schemaVersion": SCHEMA_VERSION,
                "items": [invalid]
            })
            .to_string(),
        );

        let loaded = load_store(&temp);
        assert_eq!(loaded.tracks()[0].provider, Provider::SoundCloud);
        assert_eq!(loaded.tracks()[0].id, "7");
    }

    #[test]
    fn envelope_contains_no_account_or_auth_fields() {
        let temp = TempDir::new().unwrap();
        let mut store = load_store(&temp);
        store.add_track(track(Provider::Deezer, "1")).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join(PRIMARY_FILE)).unwrap()).unwrap();
        let encoded = value.to_string();
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("arl"));
        assert!(!encoded.contains("account"));
        assert!(!encoded.contains("session"));
    }
}
