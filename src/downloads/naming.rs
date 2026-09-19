use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const MAX_DOWNLOAD_STEM_UTF16: usize = 180;

pub(crate) fn sanitize_filename(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let sanitized = sanitized.trim().trim_matches('.');
    if sanitized.is_empty() {
        "download".into()
    } else {
        sanitized.chars().take(180).collect()
    }
}

pub(crate) fn download_filename(artist: &str, title: &str, extension: &str) -> String {
    let artist = sanitize_filename(artist);
    let title = sanitize_filename(title);
    let stem = format!("{artist} - {title}");
    if stem.encode_utf16().count() <= MAX_DOWNLOAD_STEM_UTF16 {
        return format!("{stem}.{extension}");
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    stem.hash(&mut hasher);
    let suffix = format!("-{:016x}", hasher.finish());
    let budget = MAX_DOWNLOAD_STEM_UTF16 - suffix.len() - 3;
    let artist_units = artist.encode_utf16().count();
    let title_units = title.encode_utf16().count();
    let artist_budget = (budget / 2)
        .max(budget.saturating_sub(title_units))
        .min(artist_units);
    let title_budget = budget - artist_budget;
    let artist = take_utf16(&artist, artist_budget);
    let title = take_utf16(&title, title_budget);
    format!("{artist} - {title}{suffix}.{extension}")
}

fn take_utf16(value: &str, limit: usize) -> &str {
    let mut units = 0;
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = units + character.len_utf16();
        if next > limit {
            break;
        }
        units = next;
        end = index + character.len_utf8();
    }
    &value[..end]
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BatchTargetInspection {
    pub(crate) existing: Vec<PathBuf>,
    pub(crate) duplicates: Vec<PathBuf>,
}

/// The key used for destinations within one batch. Windows paths compare
/// without regard to case, so a later spelling must not overwrite a file
/// just written by an earlier job.
pub(crate) fn destination_identity(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    #[cfg(not(windows))]
    {
        path.to_path_buf()
    }
}

pub(crate) fn inspect_batch_targets<I>(paths: I) -> BatchTargetInspection
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut seen = std::collections::HashSet::new();
    let mut inspection = BatchTargetInspection::default();
    for path in paths {
        if path.exists() {
            inspection.existing.push(path.clone());
        }
        if !seen.insert(destination_identity(&path)) {
            inspection.duplicates.push(path);
        }
    }
    inspection
}

#[cfg(test)]
pub(crate) fn batch_candidate_ids<I>(tracks: I, expected: Option<usize>) -> Option<Vec<String>>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = std::collections::HashSet::new();
    let ids = tracks.into_iter().collect::<Vec<_>>();
    let complete = expected == Some(ids.len())
        && ids
            .iter()
            .all(|id| !id.trim().is_empty() && seen.insert(id.clone()));
    complete.then_some(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn sanitizes_provider_names() {
        assert_eq!(sanitize_filename("A:/B?*\n"), "A__B___");
        assert_eq!(sanitize_filename("..."), "download");
    }

    #[test]
    fn long_download_names_leave_room_for_the_transfer_suffix() {
        let name = download_filename(&"😀".repeat(180), &"b".repeat(180), "flac");
        assert!(name.encode_utf16().count() <= MAX_DOWNLOAD_STEM_UTF16 + 5);
        assert!(name.contains(" - b"));
        assert!(name.ends_with(".flac"));
    }

    #[test]
    fn inspects_existing_and_duplicate_batch_targets_without_reserving_paths() {
        let directory = tempdir().unwrap();
        let existing = directory.path().join("one.mp3");
        std::fs::write(&existing, []).unwrap();
        let missing = directory.path().join("two.mp3");
        assert_eq!(
            inspect_batch_targets(vec![existing.clone(), missing, existing.clone()]),
            BatchTargetInspection {
                existing: vec![existing.clone(), existing.clone()],
                duplicates: vec![existing]
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn batch_destinations_are_case_insensitive_on_windows() {
        let inspection = inspect_batch_targets([
            PathBuf::from("C:/Downloads/Artist - Song.mp3"),
            PathBuf::from("C:/Downloads/artist - song.MP3"),
        ]);
        assert_eq!(inspection.duplicates.len(), 1);
    }

    #[test]
    fn batch_candidates_require_complete_unique_track_ids() {
        assert_eq!(
            batch_candidate_ids(vec!["1".into(), "2".into()], Some(2))
                .unwrap()
                .len(),
            2
        );
        assert!(batch_candidate_ids(vec!["1".into()], Some(2)).is_none());
        assert!(batch_candidate_ids(vec!["1".into(), "1".into()], Some(2)).is_none());
        assert!(batch_candidate_ids(vec!["".into()], Some(1)).is_none());
    }
}
