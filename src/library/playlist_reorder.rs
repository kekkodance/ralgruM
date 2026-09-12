use std::collections::HashSet;

use crate::search::Provider;

use super::model::{Page, Track};

pub(crate) fn reorder_items<T: Clone>(items: &[T], from: usize, to: usize) -> Option<Vec<T>> {
    if from >= items.len() || to >= items.len() || from == to {
        return None;
    }
    let mut reordered = items.to_vec();
    let item = reordered.remove(from);
    reordered.insert(to, item);
    Some(reordered)
}

pub(crate) fn page_eligible(page: &Page, editable: bool) -> bool {
    matches!(
        page.platform,
        Some(super::model::Service::Deezer | super::model::Service::SoundCloud)
    ) && editable
        && page.total > 1
        && (page.platform != Some(super::model::Service::SoundCloud)
            || page.total <= super::soundcloud_client::MAX_PLAYLIST_TRACKS)
        && page.authoritative_total == Some(page.raw_loaded_count)
        && page.normalized_count == page.raw_loaded_count
        && page.tracks.len() == page.total
        && unique_numeric_ids(&page.tracks)
}

pub(crate) fn move_eligible(page: &Page, editable: bool, filtered: bool, pending: bool) -> bool {
    !filtered && !pending && page_eligible(page, editable)
}

pub(crate) fn route_eligible(
    provider: Provider,
    action: &str,
    playlist_id: &str,
    active_playlist_id: &str,
) -> bool {
    matches!(provider, Provider::Deezer | Provider::SoundCloud)
        && action == "playlistTracks"
        && !playlist_id.is_empty()
        && playlist_id == active_playlist_id
}

pub(crate) fn detail_eligible(
    provider: Provider,
    kind: crate::search::ResultType,
    editable: bool,
    total: Option<usize>,
    raw_loaded_count: usize,
    normalized_count: usize,
    tracks: &[crate::search::Track],
) -> bool {
    matches!(provider, Provider::Deezer | Provider::SoundCloud)
        && kind == crate::search::ResultType::Playlists
        && editable
        && total.is_some_and(|total| {
            total > 1
                && total == raw_loaded_count
                && (provider != Provider::SoundCloud
                    || total <= super::soundcloud_client::MAX_PLAYLIST_TRACKS)
        })
        && normalized_count == raw_loaded_count
        && tracks.len() == raw_loaded_count
        && all_unique_numeric_ids(tracks, |track| &track.id)
}

fn unique_numeric_ids(tracks: &[Track]) -> bool {
    all_unique_numeric_ids(tracks, |track| &track.id)
}

fn all_unique_numeric_ids<T>(items: &[T], id_fn: impl Fn(&T) -> &str) -> bool {
    if !items.iter().all(|item| valid_numeric_id(id_fn(item))) {
        return false;
    }
    if items.len() <= 32 {
        for i in 0..items.len() {
            let id_i = id_fn(&items[i]);
            for item in items.iter().skip(i + 1) {
                if id_i == id_fn(item) {
                    return false;
                }
            }
        }
        true
    } else {
        let mut seen = HashSet::with_capacity(items.len());
        for item in items {
            if !seen.insert(id_fn(item)) {
                return false;
            }
        }
        true
    }
}

fn valid_numeric_id(id: &str) -> bool {
    id.parse::<u64>().is_ok_and(|id| id > 0)
        && !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Provider, ResultType};

    #[test]
    fn reorder_order_moves_one_item_without_mutating_input() {
        let source = ["a", "b", "c"];
        assert_eq!(reorder_items(&source, 0, 2), Some(vec!["b", "c", "a"]));
        assert_eq!(source, ["a", "b", "c"]);
        assert!(reorder_items(&source, 3, 0).is_none());
    }

    #[test]
    fn detail_route_eligibility_rejects_lossy_subset_and_non_library_pages() {
        let tracks = vec![
            crate::search::Track {
                id: "1".into(),
                ..Default::default()
            },
            crate::search::Track {
                id: "2".into(),
                ..Default::default()
            },
        ];
        assert!(detail_eligible(
            Provider::Deezer,
            ResultType::Playlists,
            true,
            Some(2),
            2,
            2,
            &tracks
        ));
        assert!(detail_eligible(
            Provider::SoundCloud,
            ResultType::Playlists,
            true,
            Some(2),
            2,
            2,
            &tracks
        ));
        assert!(!detail_eligible(
            Provider::Deezer,
            ResultType::Playlists,
            true,
            Some(3),
            2,
            2,
            &tracks
        ));
        assert!(!detail_eligible(
            Provider::Deezer,
            ResultType::Playlists,
            true,
            Some(1),
            1,
            1,
            &tracks[..1]
        ));
        let duplicate = vec![
            crate::search::Track {
                id: "1".into(),
                ..Default::default()
            },
            crate::search::Track {
                id: "1".into(),
                ..Default::default()
            },
        ];
        assert!(!detail_eligible(
            Provider::SoundCloud,
            ResultType::Playlists,
            true,
            Some(2),
            2,
            2,
            &duplicate
        ));
        let oversized = (1..=super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1)
            .map(|id| crate::search::Track {
                id: id.to_string(),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        assert!(!detail_eligible(
            Provider::SoundCloud,
            ResultType::Playlists,
            true,
            Some(oversized.len()),
            oversized.len(),
            oversized.len(),
            &oversized,
        ));
    }

    #[test]
    fn move_eligibility_rejects_filter_and_pending_states() {
        let page = Page {
            platform: Some(super::super::model::Service::Deezer),
            total: 2,
            authoritative_total: Some(2),
            raw_loaded_count: 2,
            normalized_count: 2,
            tracks: vec![
                Track {
                    id: "1".into(),
                    ..Default::default()
                },
                Track {
                    id: "2".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(move_eligible(&page, true, false, false));
        assert!(!move_eligible(&page, true, true, false));
        assert!(!move_eligible(&page, true, false, true));
        assert!(!move_eligible(&page, false, false, false));

        let oversized_soundcloud = Page {
            platform: Some(super::super::model::Service::SoundCloud),
            total: super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1,
            authoritative_total: Some(super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1),
            raw_loaded_count: super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1,
            normalized_count: super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1,
            tracks: (1..=super::super::soundcloud_client::MAX_PLAYLIST_TRACKS + 1)
                .map(|id| Track {
                    id: id.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        assert!(!move_eligible(&oversized_soundcloud, true, false, false));
    }

    #[test]
    fn route_eligibility_rejects_colliding_non_playlist_routes() {
        assert!(route_eligible(
            Provider::Deezer,
            "playlistTracks",
            "42",
            "42"
        ));
        assert!(!route_eligible(Provider::Deezer, "albumTracks", "42", "42"));
        assert!(!route_eligible(Provider::Deezer, "artist", "42", "42"));
        assert!(!route_eligible(Provider::Deezer, "playlists", "42", "42"));
        assert!(route_eligible(
            Provider::SoundCloud,
            "playlistTracks",
            "42",
            "42"
        ));
        assert!(!route_eligible(
            Provider::Deezer,
            "playlistTracks",
            "42",
            "7"
        ));
    }

    #[test]
    fn zero_and_non_numeric_track_ids_are_not_eligible() {
        assert!(!valid_numeric_id("0"));
        assert!(!valid_numeric_id(""));
        assert!(!valid_numeric_id("1.5"));
        assert!(!valid_numeric_id("track"));
        assert!(valid_numeric_id("1"));
    }
}
