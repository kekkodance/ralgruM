use std::{
    collections::HashSet,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    app::paths,
    search::{Provider, TrackArtistRef},
};

use super::local_store::LocalTrack;

const FORMAT: &str = "ralgrum-local-playlists";
const SCHEMA_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const PRIMARY_FILE: &str = "local_playlists.json";
const BACKUP_FILE: &str = "local_playlists.backup.json";
pub(crate) const LOCAL_PLAYLIST_TITLE_MAX_CHARS: usize =
    super::playlist_limits::LOCAL_TITLE_MAX_CHARS;
pub(crate) const LOCAL_PLAYLIST_DESCRIPTION_MAX_CHARS: usize =
    super::playlist_limits::LOCAL_DESCRIPTION_MAX_CHARS;

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum LocalItemKind {
    Track,
    Album,
    Artist,
    Playlist,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredArtist {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTrack {
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
struct StoredPlaylist {
    id: String,
    title: String,
    description: String,
    artwork: String,
    tracks: Vec<StoredTrack>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalPlaylistEnvelope {
    format: String,
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    playlists: Vec<StoredPlaylist>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalPlaylist {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) artwork: String,
    pub(crate) tracks: Vec<LocalTrack>,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalPlaylistStore {
    directory: PathBuf,
    playlists: Vec<LocalPlaylist>,
}

impl LocalPlaylistStore {
    pub(crate) fn load_current_user() -> Result<Self, LocalPlaylistError> {
        let directory = paths::config_dir()
            .ok_or(LocalPlaylistError::Unavailable)?
            .join("local_library");
        Self::load_from_directory(&directory)
    }

    pub(crate) fn load_from_directory(directory: &Path) -> Result<Self, LocalPlaylistError> {
        fs::create_dir_all(directory).map_err(|_| LocalPlaylistError::Filesystem)?;
        let primary = read_file(&directory.join(PRIMARY_FILE))?;
        let backup = read_file(&directory.join(BACKUP_FILE))?;
        let playlists = match (primary, backup) {
            (StoredFile::Valid(primary), StoredFile::Valid(backup)) => {
                if primary != backup {
                    write_atomic(&directory.join(BACKUP_FILE), &encode_playlists(&primary)?)?;
                }
                primary
            }
            (StoredFile::Valid(playlists), StoredFile::Missing | StoredFile::Invalid(_)) => {
                write_atomic(&directory.join(BACKUP_FILE), &encode_playlists(&playlists)?)?;
                playlists
            }
            (StoredFile::Missing | StoredFile::Invalid(_), StoredFile::Valid(playlists)) => {
                write_atomic(
                    &directory.join(PRIMARY_FILE),
                    &encode_playlists(&playlists)?,
                )?;
                playlists
            }
            (StoredFile::Missing, StoredFile::Missing) => Vec::new(),
            (StoredFile::Missing, StoredFile::Invalid(error))
            | (StoredFile::Invalid(error), StoredFile::Missing)
            | (StoredFile::Invalid(error), StoredFile::Invalid(_)) => return Err(error),
        };
        Ok(Self {
            directory: directory.to_owned(),
            playlists,
        })
    }

    pub(crate) fn playlists(&self) -> &[LocalPlaylist] {
        &self.playlists
    }

    pub(crate) fn playlist(&self, id: &str) -> Option<&LocalPlaylist> {
        self.playlists.iter().find(|playlist| playlist.id == id)
    }

    #[cfg(test)]
    pub(crate) fn create(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.create_with_artwork(title, description, None)
    }

    pub(crate) fn create_with_artwork(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        artwork_jpeg: Option<&[u8]>,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        let title = normalize_playlist_title(title.into())?;
        let description = normalize_playlist_description(description.into())?;
        let id = Uuid::new_v4().simple().to_string();
        let written = artwork_jpeg
            .map(|jpeg| super::local_playlist_artwork::write(&self.directory, &id, jpeg))
            .transpose()?;
        let playlist = LocalPlaylist {
            id,
            title,
            description,
            artwork: written
                .as_ref()
                .map(|artwork| artwork.reference.clone())
                .unwrap_or_default(),
            tracks: Vec::new(),
        };
        let mut playlists = self.playlists.clone();
        playlists.push(playlist.clone());
        if let Err(error) = self.persist(playlists) {
            if error == LocalPlaylistError::Serialization
                && let Some(written) = &written
            {
                super::local_playlist_artwork::cleanup(
                    &self.directory,
                    &playlist.id,
                    &written.reference,
                );
            }
            return Err(error);
        }
        Ok(playlist)
    }

    #[cfg(test)]
    pub(crate) fn create_with_tracks(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        tracks: &[LocalTrack],
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.create_with_tracks_and_artwork(title, description, tracks, None)
    }

    pub(crate) fn create_with_tracks_and_artwork(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        tracks: &[LocalTrack],
        artwork_jpeg: Option<&[u8]>,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        let title = normalize_playlist_title(title.into())?;
        let description = normalize_playlist_description(description.into())?;
        let mut seen = HashSet::with_capacity(tracks.len());
        let mut normalized_tracks = Vec::with_capacity(tracks.len());
        for track in tracks {
            let track = normalize_track(track.clone())?;
            if !seen.insert((track.provider, track.id.clone())) {
                return Err(LocalPlaylistError::DuplicateItem);
            }
            normalized_tracks.push(track);
        }
        let id = Uuid::new_v4().simple().to_string();
        let written = artwork_jpeg
            .map(|jpeg| super::local_playlist_artwork::write(&self.directory, &id, jpeg))
            .transpose()?;
        let playlist = LocalPlaylist {
            id,
            title,
            description,
            artwork: written
                .as_ref()
                .map(|artwork| artwork.reference.clone())
                .unwrap_or_default(),
            tracks: normalized_tracks,
        };
        let mut playlists = self.playlists.clone();
        playlists.push(playlist.clone());
        if let Err(error) = self.persist(playlists) {
            if error == LocalPlaylistError::Serialization
                && let Some(written) = &written
            {
                super::local_playlist_artwork::cleanup(
                    &self.directory,
                    &playlist.id,
                    &written.reference,
                );
            }
            return Err(error);
        }
        Ok(playlist)
    }

    #[cfg(test)]
    pub(crate) fn update(
        &mut self,
        playlist_id: &str,
        title: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        self.update_with_artwork(playlist_id, title, description, None)
    }

    pub(crate) fn update_with_artwork(
        &mut self,
        playlist_id: &str,
        title: impl Into<String>,
        description: impl Into<String>,
        artwork_jpeg: Option<&[u8]>,
    ) -> Result<LocalPlaylist, LocalPlaylistError> {
        let playlist_id = normalize_id(playlist_id.to_owned())?;
        let title = normalize_playlist_title(title.into())?;
        let description = normalize_playlist_description(description.into())?;
        let old_artwork = self
            .playlist(&playlist_id)
            .ok_or(LocalPlaylistError::NotFound)?
            .artwork
            .clone();
        let written = artwork_jpeg
            .map(|jpeg| super::local_playlist_artwork::write(&self.directory, &playlist_id, jpeg))
            .transpose()?;
        let mut playlists = self.playlists.clone();
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or(LocalPlaylistError::NotFound)?;
        playlist.title = title;
        playlist.description = description;
        if let Some(written) = &written {
            playlist.artwork = written.reference.clone();
        }
        let updated = playlist.clone();
        if let Err(error) = self.persist(playlists) {
            // Encoding fails before any metadata write. Filesystem failures
            // may have committed the backup, so retain its referenced cover.
            if error == LocalPlaylistError::Serialization
                && let Some(written) = &written
                && written.reference != old_artwork
            {
                super::local_playlist_artwork::cleanup(
                    &self.directory,
                    &playlist_id,
                    &written.reference,
                );
            }
            return Err(error);
        }
        if written.is_some() && updated.artwork != old_artwork {
            super::local_playlist_artwork::cleanup(&self.directory, &playlist_id, &old_artwork);
        }
        Ok(updated)
    }

    pub(crate) fn delete(&mut self, id: &str) -> Result<bool, LocalPlaylistError> {
        let id = normalize_id(id.to_owned())?;
        let mut playlists = self.playlists.clone();
        let old_artwork = playlists
            .iter()
            .find(|playlist| playlist.id == id)
            .map(|playlist| playlist.artwork.clone());
        let before = playlists.len();
        playlists.retain(|playlist| playlist.id != id);
        if before == playlists.len() {
            return Ok(false);
        }
        self.persist(playlists)?;
        if let Some(old_artwork) = old_artwork {
            super::local_playlist_artwork::cleanup(&self.directory, &id, &old_artwork);
        }
        Ok(true)
    }

    pub(crate) fn artwork_path(&self, playlist_id: &str, reference: &str) -> Option<PathBuf> {
        super::local_playlist_artwork::resolve(&self.directory, playlist_id, reference)
    }

    pub(crate) fn add_tracks(
        &mut self,
        playlist_id: &str,
        tracks: &[LocalTrack],
    ) -> Result<(), LocalPlaylistError> {
        let playlist_id = normalize_id(playlist_id.to_owned())?;
        let mut playlists = self.playlists.clone();
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or(LocalPlaylistError::NotFound)?;
        let mut seen = playlist
            .tracks
            .iter()
            .map(|track| (track.provider, track.id.clone()))
            .collect::<HashSet<_>>();
        let mut additions = Vec::with_capacity(tracks.len());
        for track in tracks {
            let track = normalize_track(track.clone())?;
            if !seen.insert((track.provider, track.id.clone())) {
                return Err(LocalPlaylistError::DuplicateItem);
            }
            additions.push(track);
        }
        playlist.tracks.extend(additions);
        self.persist(playlists)
    }

    pub(crate) fn remove_track(
        &mut self,
        playlist_id: &str,
        provider: Provider,
        id: &str,
    ) -> Result<bool, LocalPlaylistError> {
        let playlist_id = normalize_id(playlist_id.to_owned())?;
        let id = normalize_id(id.to_owned())?;
        let mut playlists = self.playlists.clone();
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or(LocalPlaylistError::NotFound)?;
        let Some(index) = playlist
            .tracks
            .iter()
            .position(|track| track.provider == provider && track.id == id)
        else {
            return Ok(false);
        };
        playlist.tracks.remove(index);
        self.persist(playlists)?;
        Ok(true)
    }

    pub(crate) fn reorder_tracks(
        &mut self,
        playlist_id: &str,
        from: usize,
        to: usize,
    ) -> Result<bool, LocalPlaylistError> {
        let playlist_id = normalize_id(playlist_id.to_owned())?;
        let mut playlists = self.playlists.clone();
        let playlist = playlists
            .iter_mut()
            .find(|playlist| playlist.id == playlist_id)
            .ok_or(LocalPlaylistError::NotFound)?;
        if from >= playlist.tracks.len() || to >= playlist.tracks.len() || from == to {
            return Ok(false);
        }
        let track = playlist.tracks.remove(from);
        playlist.tracks.insert(to, track);
        self.persist(playlists)?;
        Ok(true)
    }

    fn persist(&mut self, playlists: Vec<LocalPlaylist>) -> Result<(), LocalPlaylistError> {
        let encoded = encode_playlists(&playlists)?;
        let previous = encode_playlists(&self.playlists)?;
        fs::create_dir_all(&self.directory).map_err(|_| LocalPlaylistError::Filesystem)?;
        write_pair(&self.directory, &encoded, &previous)?;
        self.playlists = playlists;
        Ok(())
    }
}

fn write_pair(directory: &Path, next: &[u8], previous: &[u8]) -> Result<(), LocalPlaylistError> {
    write_pair_with(directory, next, previous, write_atomic)
}

fn write_pair_with<F>(
    directory: &Path,
    next: &[u8],
    previous: &[u8],
    mut writer: F,
) -> Result<(), LocalPlaylistError>
where
    F: FnMut(&Path, &[u8]) -> Result<(), LocalPlaylistError>,
{
    let backup = directory.join(BACKUP_FILE);
    let primary = directory.join(PRIMARY_FILE);
    writer(&backup, next)?;
    if let Err(error) = writer(&primary, next) {
        let _ = writer(&backup, previous);
        return Err(error);
    }
    Ok(())
}

fn normalize_playlist(mut playlist: LocalPlaylist) -> Result<LocalPlaylist, LocalPlaylistError> {
    playlist.id = normalize_id(playlist.id)?;
    playlist.title = normalize_playlist_title(playlist.title)?;
    playlist.description = normalize_playlist_description(playlist.description)?;
    playlist.artwork =
        super::local_playlist_artwork::normalize_reference(&playlist.id, playlist.artwork);
    let mut seen = HashSet::new();
    let mut tracks = Vec::with_capacity(playlist.tracks.len());
    for track in playlist.tracks {
        let track = normalize_track(track)?;
        if !seen.insert((track.provider, track.id.clone())) {
            return Err(LocalPlaylistError::DuplicateItem);
        }
        tracks.push(track);
    }
    playlist.tracks = tracks;
    Ok(playlist)
}

fn normalize_track(mut track: LocalTrack) -> Result<LocalTrack, LocalPlaylistError> {
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

fn normalize_id(value: String) -> Result<String, LocalPlaylistError> {
    let value = value.trim().to_owned();
    if value.is_empty()
        || value.len() > 256
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(LocalPlaylistError::InvalidItem);
    }
    Ok(value)
}

fn normalize_playlist_title(value: String) -> Result<String, LocalPlaylistError> {
    let value = value.trim().to_owned();
    if value.is_empty()
        || value.chars().count() > LOCAL_PLAYLIST_TITLE_MAX_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LocalPlaylistError::InvalidItem);
    }
    Ok(value)
}

fn normalize_playlist_description(value: String) -> Result<String, LocalPlaylistError> {
    let value = value.trim().to_owned();
    if value.chars().count() > LOCAL_PLAYLIST_DESCRIPTION_MAX_CHARS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(LocalPlaylistError::InvalidItem);
    }
    Ok(value)
}

fn normalize_text(value: String) -> Result<String, LocalPlaylistError> {
    let value = value.trim().to_owned();
    if value.len() > 16 * 1024 || value.chars().any(char::is_control) {
        return Err(LocalPlaylistError::InvalidItem);
    }
    Ok(value)
}

fn stored_track(track: &LocalTrack) -> StoredTrack {
    StoredTrack {
        kind: LocalItemKind::Track,
        provider: track.provider.into(),
        id: track.id.clone(),
        title: track.title.clone(),
        artist: track.artist.clone(),
        artists: track
            .artists
            .iter()
            .map(|artist| StoredArtist {
                id: artist.id.clone(),
                name: artist.name.clone(),
            })
            .collect(),
        album: track.album.clone(),
        album_id: track.album_id.clone(),
        release_date: track.release_date.clone(),
        duration: track.duration,
        artwork: track.artwork.clone(),
        explicit: track.explicit,
        service_url: track.service_url.clone(),
    }
}

fn local_track(track: StoredTrack) -> Result<LocalTrack, LocalPlaylistError> {
    if track.kind != LocalItemKind::Track {
        return Err(LocalPlaylistError::InvalidItem);
    }
    normalize_track(LocalTrack {
        provider: track.provider.into(),
        id: track.id,
        title: track.title,
        artist: track.artist,
        artists: track
            .artists
            .into_iter()
            .map(|artist| TrackArtistRef {
                id: artist.id,
                name: artist.name,
            })
            .collect(),
        album: track.album,
        album_id: track.album_id,
        release_date: track.release_date,
        duration: track.duration,
        artwork: track.artwork,
        explicit: track.explicit,
        service_url: track.service_url,
    })
}

fn stored_playlist(playlist: &LocalPlaylist) -> StoredPlaylist {
    StoredPlaylist {
        id: playlist.id.clone(),
        title: playlist.title.clone(),
        description: playlist.description.clone(),
        artwork: playlist.artwork.clone(),
        tracks: playlist.tracks.iter().map(stored_track).collect(),
    }
}

fn local_playlist(playlist: StoredPlaylist) -> Result<LocalPlaylist, LocalPlaylistError> {
    let tracks = playlist
        .tracks
        .into_iter()
        .map(local_track)
        .collect::<Result<Vec<_>, _>>()?;
    normalize_playlist(LocalPlaylist {
        id: playlist.id,
        title: playlist.title,
        description: playlist.description,
        artwork: playlist.artwork,
        tracks,
    })
}

fn decode_playlists(
    playlists: Vec<StoredPlaylist>,
) -> Result<Vec<LocalPlaylist>, LocalPlaylistError> {
    let mut seen = HashSet::new();
    playlists
        .into_iter()
        .map(local_playlist)
        .collect::<Result<Vec<_>, _>>()
        .and_then(|playlists| {
            for playlist in &playlists {
                if !seen.insert(playlist.id.clone()) {
                    return Err(LocalPlaylistError::DuplicateItem);
                }
            }
            Ok(playlists)
        })
}

fn encode_playlists(playlists: &[LocalPlaylist]) -> Result<Vec<u8>, LocalPlaylistError> {
    let playlists = playlists.iter().map(stored_playlist).collect::<Vec<_>>();
    let encoded = serde_json::to_vec_pretty(&LocalPlaylistEnvelope {
        format: FORMAT.into(),
        schema_version: SCHEMA_VERSION,
        playlists,
    })
    .map_err(|_| LocalPlaylistError::Serialization)?;
    if encoded.len() as u64 > MAX_FILE_BYTES {
        return Err(LocalPlaylistError::Serialization);
    }
    Ok(encoded)
}

enum StoredFile {
    Missing,
    Valid(Vec<LocalPlaylist>),
    Invalid(LocalPlaylistError),
}

fn read_file(path: &Path) -> Result<StoredFile, LocalPlaylistError> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(StoredFile::Missing),
        Err(_) => return Err(LocalPlaylistError::Filesystem),
    };
    let metadata = file
        .metadata()
        .map_err(|_| LocalPlaylistError::Filesystem)?;
    if metadata.len() > MAX_FILE_BYTES {
        return Ok(StoredFile::Invalid(LocalPlaylistError::InvalidFile));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalPlaylistError::Filesystem)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Ok(StoredFile::Invalid(LocalPlaylistError::InvalidFile));
    }
    let envelope = match serde_json::from_slice::<LocalPlaylistEnvelope>(&bytes) {
        Ok(envelope) => envelope,
        Err(_) => return Ok(StoredFile::Invalid(LocalPlaylistError::InvalidFile)),
    };
    if envelope.format != FORMAT || envelope.schema_version != SCHEMA_VERSION {
        return Ok(StoredFile::Invalid(LocalPlaylistError::InvalidFile));
    }
    match decode_playlists(envelope.playlists) {
        Ok(playlists) => Ok(StoredFile::Valid(playlists)),
        Err(error) => Ok(StoredFile::Invalid(error)),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LocalPlaylistError> {
    let parent = path.parent().ok_or(LocalPlaylistError::Filesystem)?;
    fs::create_dir_all(parent).map_err(|_| LocalPlaylistError::Filesystem)?;
    let mut temp_path = None;
    let mut file = None;
    for attempt in 0..16u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("local-playlists"),
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
            Err(_) => return Err(LocalPlaylistError::Filesystem),
        }
    }
    let temp_path = temp_path.ok_or(LocalPlaylistError::Filesystem)?;
    let result = (|| {
        let mut file = file.take().ok_or(LocalPlaylistError::Filesystem)?;
        file.write_all(bytes)
            .map_err(|_| LocalPlaylistError::Filesystem)?;
        file.sync_all()
            .map_err(|_| LocalPlaylistError::Filesystem)?;
        atomic_replace(&temp_path, path).map_err(|_| LocalPlaylistError::Filesystem)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
pub(super) fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
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
pub(super) fn atomic_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalPlaylistError {
    Unavailable,
    Filesystem,
    InvalidFile,
    InvalidItem,
    DuplicateItem,
    NotFound,
    Serialization,
}

impl fmt::Display for LocalPlaylistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "Local playlist storage is unavailable.",
            Self::Filesystem => "Local playlist storage could not be accessed.",
            Self::InvalidFile => "The local playlist file is invalid.",
            Self::InvalidItem => "The local playlist contains invalid data.",
            Self::DuplicateItem => "The local playlist contains duplicate items.",
            Self::NotFound => "The local playlist could not be found.",
            Self::Serialization => "The local playlist could not be prepared.",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor};

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
            artwork: "https://example.test/artwork".into(),
            explicit: false,
            service_url: "https://example.test/track".into(),
        }
    }

    fn cover_jpeg(color: [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(512, 512, image::Rgb(color));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    #[test]
    fn empty_startup_has_no_playlists() {
        let directory = TempDir::new().unwrap();
        let store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert!(store.playlists().is_empty());
    }

    #[test]
    fn round_trip_preserves_mixed_provider_tracks_and_order() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create(" Mix ", " description ").unwrap();
        store
            .add_tracks(
                &playlist.id,
                &[
                    track(Provider::Deezer, "1"),
                    track(Provider::SoundCloud, "1"),
                ],
            )
            .unwrap();
        drop(store);
        let store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.playlist(&playlist.id).unwrap();
        assert_eq!(playlist.title, "Mix");
        assert_eq!(playlist.description, "description");
        assert_eq!(
            playlist
                .tracks
                .iter()
                .map(|track| (track.provider, track.id.as_str()))
                .collect::<Vec<_>>(),
            vec![(Provider::Deezer, "1"), (Provider::SoundCloud, "1")]
        );
    }

    #[test]
    fn duplicate_provider_and_id_is_rejected_but_cross_provider_id_is_allowed() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create("Mix", "").unwrap();
        store
            .add_tracks(&playlist.id, &[track(Provider::Deezer, "1")])
            .unwrap();
        assert_eq!(
            store.add_tracks(&playlist.id, &[track(Provider::Deezer, "1")]),
            Err(LocalPlaylistError::DuplicateItem)
        );
        store
            .add_tracks(&playlist.id, &[track(Provider::SoundCloud, "1")])
            .unwrap();
        assert_eq!(store.playlist(&playlist.id).unwrap().tracks.len(), 2);
    }

    #[test]
    fn reorder_and_remove_persist() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create("Mix", "").unwrap();
        store
            .add_tracks(
                &playlist.id,
                &[track(Provider::Deezer, "1"), track(Provider::Deezer, "2")],
            )
            .unwrap();
        assert!(store.reorder_tracks(&playlist.id, 0, 1).unwrap());
        assert!(
            store
                .remove_track(&playlist.id, Provider::Deezer, "1")
                .unwrap()
        );
        let reloaded = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(reloaded.playlist(&playlist.id).unwrap().tracks[0].id, "2");
    }

    #[test]
    fn update_persists_trimmed_metadata_without_touching_tracks() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create("Original", "Old description").unwrap();
        store
            .add_tracks(&playlist.id, &[track(Provider::SoundCloud, "7")])
            .unwrap();

        let updated = store
            .update(&playlist.id, "  Updated  ", "  New description  ")
            .unwrap();
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.description, "New description");
        assert_eq!(updated.tracks.len(), 1);

        let reloaded = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(reloaded.playlist(&playlist.id), Some(&updated));
    }

    #[test]
    fn update_enforces_limits_and_keeps_the_previous_playlist_on_failure() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create("Original", "Description").unwrap();
        let original = store.playlist(&playlist.id).unwrap().clone();

        assert_eq!(
            store.update(
                &playlist.id,
                "x".repeat(LOCAL_PLAYLIST_TITLE_MAX_CHARS + 1),
                "Description",
            ),
            Err(LocalPlaylistError::InvalidItem)
        );
        assert_eq!(store.playlist(&playlist.id), Some(&original));

        assert_eq!(
            store.update(
                &playlist.id,
                "Updated",
                "x".repeat(LOCAL_PLAYLIST_DESCRIPTION_MAX_CHARS + 1),
            ),
            Err(LocalPlaylistError::InvalidItem)
        );
        assert_eq!(store.playlist(&playlist.id), Some(&original));

        assert_eq!(
            store.update(&playlist.id, "Updated", "bad\u{0000}description"),
            Err(LocalPlaylistError::InvalidItem)
        );
        let reloaded = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(reloaded.playlist(&playlist.id), Some(&original));
    }

    #[test]
    fn create_with_tracks_persists_metadata_and_all_track_fields_atomically() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let track = track(Provider::SoundCloud, "sc-42");
        let playlist = store
            .create_with_tracks(" Mix ", " Description ", std::slice::from_ref(&track))
            .unwrap();
        assert_eq!(playlist.title, "Mix");
        assert_eq!(playlist.description, "Description");
        assert_eq!(playlist.tracks, vec![track.clone()]);

        let reloaded = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(reloaded.playlist(&playlist.id).unwrap().tracks, vec![track]);
    }

    #[test]
    fn create_with_duplicate_tracks_does_not_create_a_partial_playlist() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let track = track(Provider::Deezer, "42");
        assert_eq!(
            store.create_with_tracks("Mix", "", &[track.clone(), track]),
            Err(LocalPlaylistError::DuplicateItem)
        );
        assert!(store.playlists().is_empty());
        assert!(
            LocalPlaylistStore::load_from_directory(directory.path())
                .unwrap()
                .playlists()
                .is_empty()
        );
    }

    #[test]
    fn malformed_unknown_marker_version_and_fields_are_rejected() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join(PRIMARY_FILE);
        for contents in [
            "{}",
            r#"{"format":"wrong","schemaVersion":1,"playlists":[]}"#,
            r#"{"format":"ralgrum-local-playlists","schemaVersion":2,"playlists":[]}"#,
            r#"{"format":"ralgrum-local-playlists","schemaVersion":1,"playlists":[],"auth":"x"}"#,
            r#"{"format":"ralgrum-local-playlists","schemaVersion":1,"playlists":[{"id":"p","title":"P","description":"","artwork":"","tracks":[],"auth":"x"}]}"#,
            r#"{"format":"ralgrum-local-playlists","schemaVersion":1,"playlists":[]}{"extra":true}"#,
        ] {
            fs::write(&path, contents).unwrap();
            assert!(matches!(
                LocalPlaylistStore::load_from_directory(directory.path()),
                Err(LocalPlaylistError::InvalidFile)
            ));
            let _ = fs::remove_file(directory.path().join(BACKUP_FILE));
        }
    }

    #[test]
    fn oversized_file_is_rejected_before_decoding() {
        let directory = TempDir::new().unwrap();
        fs::write(
            directory.path().join(PRIMARY_FILE),
            vec![b' '; MAX_FILE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(matches!(
            LocalPlaylistStore::load_from_directory(directory.path()),
            Err(LocalPlaylistError::InvalidFile)
        ));
    }

    #[test]
    fn oversized_encoded_playlist_is_rejected_before_state_or_files_change() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let original = store.create("Original", "").unwrap();
        let primary_before = fs::read(directory.path().join(PRIMARY_FILE)).unwrap();
        let backup_before = fs::read(directory.path().join(BACKUP_FILE)).unwrap();
        let playlists_before = store.playlists.clone();
        let tracks = (0..1_200)
            .map(|index| {
                let mut track = track(Provider::Deezer, &format!("large-{index}"));
                track.title = "x".repeat(16 * 1024);
                track
            })
            .collect::<Vec<_>>();
        let oversized = LocalPlaylist {
            id: original.id.clone(),
            title: original.title.clone(),
            description: original.description.clone(),
            artwork: original.artwork.clone(),
            tracks,
        };
        let mut next = store.playlists.clone();
        next[0] = oversized;

        assert_eq!(store.persist(next), Err(LocalPlaylistError::Serialization));
        assert_eq!(store.playlists, playlists_before);
        assert_eq!(
            fs::read(directory.path().join(PRIMARY_FILE)).unwrap(),
            primary_before
        );
        assert_eq!(
            fs::read(directory.path().join(BACKUP_FILE)).unwrap(),
            backup_before
        );
    }

    #[test]
    fn duplicate_playlist_ids_and_blank_track_ids_are_rejected() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let first = store.create("One", "").unwrap();
        let mut second = first.clone();
        second.title = "Two".into();
        assert_eq!(
            decode_playlists(vec![stored_playlist(&first), stored_playlist(&second)]),
            Err(LocalPlaylistError::DuplicateItem)
        );
        assert_eq!(
            store.add_tracks(&first.id, &[track(Provider::Deezer, " ")]),
            Err(LocalPlaylistError::InvalidItem)
        );
    }

    #[test]
    fn valid_backup_recovers_primary_and_repairs_it() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let playlist = store.create("One", "").unwrap();
        store
            .add_tracks(&playlist.id, &[track(Provider::Deezer, "1")])
            .unwrap();
        let primary = directory.path().join(PRIMARY_FILE);
        let backup = directory.path().join(BACKUP_FILE);
        fs::write(&primary, b"not-json").unwrap();
        let recovered = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(recovered.playlists().len(), 1);
        assert_eq!(fs::read(&primary).unwrap(), fs::read(&backup).unwrap());
    }

    #[test]
    fn primary_wins_over_a_differing_valid_backup() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        store.create("Primary", "").unwrap();
        let backup = directory.path().join(BACKUP_FILE);
        let mut older = store.playlists().to_vec();
        older[0].title = "Older backup".into();
        fs::write(&backup, encode_playlists(&older).unwrap()).unwrap();

        let loaded = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert_eq!(loaded.playlists()[0].title, "Primary");
        assert_eq!(
            fs::read(&backup).unwrap(),
            fs::read(directory.path().join(PRIMARY_FILE)).unwrap()
        );
    }

    #[test]
    fn account_independent_file_contains_no_auth_fields() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        store.create("Local", "").unwrap();
        let contents = fs::read_to_string(directory.path().join(PRIMARY_FILE)).unwrap();
        assert!(!contents.contains("token"));
        assert!(!contents.contains("arl"));
        assert!(!contents.contains("account"));
    }

    #[test]
    fn failed_metadata_save_keeps_the_existing_cover() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let jpeg = cover_jpeg([12, 34, 56]);
        let playlist = store.create_with_artwork("Local", "", Some(&jpeg)).unwrap();
        let cover = store.artwork_path(&playlist.id, &playlist.artwork).unwrap();
        let backup = directory.path().join(BACKUP_FILE);
        fs::remove_file(&backup).unwrap();
        fs::create_dir(&backup).unwrap();

        assert_eq!(
            store.update_with_artwork(&playlist.id, "Renamed", "", Some(&jpeg)),
            Err(LocalPlaylistError::Filesystem),
        );
        assert_eq!(fs::read(cover).unwrap(), jpeg);
        assert_eq!(store.playlist(&playlist.id).unwrap().title, "Local");
    }

    #[test]
    fn artwork_is_app_owned_replaced_and_removed_with_the_playlist() {
        let directory = TempDir::new().unwrap();
        let mut store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        let first = cover_jpeg([12, 34, 56]);
        let playlist = store
            .create_with_artwork("Local", "Description", Some(&first))
            .unwrap();
        let first_path = store.artwork_path(&playlist.id, &playlist.artwork).unwrap();
        assert!(first_path.is_file());
        assert!(store.artwork_path("another", &playlist.artwork).is_none());

        let second = cover_jpeg([90, 80, 70]);
        let updated = store
            .update_with_artwork(&playlist.id, "Local", "Description", Some(&second))
            .unwrap();
        let second_path = store.artwork_path(&updated.id, &updated.artwork).unwrap();
        assert_ne!(first_path, second_path);
        assert!(!first_path.exists());
        assert!(second_path.is_file());

        assert!(store.delete(&playlist.id).unwrap());
        assert!(!second_path.exists());
    }

    #[test]
    fn invalid_or_missing_artwork_references_do_not_break_the_library() {
        let directory = TempDir::new().unwrap();
        let json = r#"{"format":"ralgrum-local-playlists","schemaVersion":1,"playlists":[{"id":"p","title":"P","description":"","artwork":"../outside.jpg","tracks":[]}]}"#;
        fs::write(directory.path().join(PRIMARY_FILE), json).unwrap();
        let store = LocalPlaylistStore::load_from_directory(directory.path()).unwrap();
        assert!(store.playlist("p").unwrap().artwork.is_empty());
        assert!(
            store
                .artwork_path("p", "covers/p-0123456789abcdef01234567.jpg")
                .is_none()
        );
    }

    #[test]
    fn failed_primary_commit_restores_the_previous_backup_metadata() {
        let directory = TempDir::new().unwrap();
        let previous = b"previous";
        let next = b"next";
        let mut backup_writes = Vec::new();
        let result = write_pair_with(directory.path(), next, previous, |path, bytes| {
            if path.ends_with(PRIMARY_FILE) {
                return Err(LocalPlaylistError::Filesystem);
            }
            backup_writes.push(bytes.to_vec());
            Ok(())
        });
        assert_eq!(result, Err(LocalPlaylistError::Filesystem));
        assert_eq!(backup_writes, vec![next.to_vec(), previous.to_vec()]);
    }
}
