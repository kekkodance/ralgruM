use std::path::PathBuf;

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct BatchTargetInspection {
    pub(crate) existing: Vec<PathBuf>,
    pub(crate) duplicates: Vec<PathBuf>,
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
        if !seen.insert(path.clone()) {
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
