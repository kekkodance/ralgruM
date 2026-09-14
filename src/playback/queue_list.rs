use std::ops::Range;

use super::PlaybackTrack;

/// The persistent identity of the items held by the queue list state.
///
/// Queue rows are indexed by their position in the upcoming index vector. The
/// loading row is tracked separately because it is inserted after the tracks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct QueueListIdentity {
    pub(super) indices: Vec<usize>,
    pub(super) has_loading_row: bool,
}

impl QueueListIdentity {
    pub(super) fn new(indices: &[usize], has_loading_row: bool) -> Self {
        Self {
            indices: indices.to_vec(),
            has_loading_row,
        }
    }

    pub(super) fn item_count(&self) -> usize {
        self.indices.len() + usize::from(self.has_loading_row)
    }
}

/// How a persistent list state should be updated when its queue identity
/// changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum QueueListChange {
    Unchanged,
    Splice {
        old_range: Range<usize>,
        new_count: usize,
    },
    Reset {
        item_count: usize,
    },
}

/// Return the smallest safe list-state mutation for a queue identity change.
///
/// Appends are spliced before an existing loading row so the row keeps its
/// position at the tail. Any reorder, removal, or replacement resets the list
/// because preserving a pixel anchor for different queue items is unsafe.
pub(super) fn list_change(old: &QueueListIdentity, new: &QueueListIdentity) -> QueueListChange {
    if old == new {
        return QueueListChange::Unchanged;
    }

    if new.indices.starts_with(&old.indices) {
        let old_track_count = old.indices.len();
        let new_track_count = new.indices.len();
        let old_tail_count = usize::from(old.has_loading_row);
        let new_tail_count = new_track_count - old_track_count + usize::from(new.has_loading_row);
        return QueueListChange::Splice {
            old_range: old_track_count..old_track_count + old_tail_count,
            new_count: new_tail_count,
        };
    }

    QueueListChange::Reset {
        item_count: new.item_count(),
    }
}

/// Map an item range emitted by [`gpui::ListScrollEvent`] to track rows.
///
/// The final item can be a loading sentinel, so callers must clamp the end to
/// the track count before indexing their owned row snapshots.
#[cfg(test)]
pub(super) fn visible_track_range(visible_range: Range<usize>, track_count: usize) -> Range<usize> {
    visible_range.start.min(track_count)..visible_range.end.min(track_count)
}

pub(super) fn track_index_for_list_item(item_index: usize, track_count: usize) -> Option<usize> {
    (item_index < track_count).then_some(item_index)
}

/// Decide whether a list scroll event is close enough to the tail to extend an
/// infinite queue. The end of the visible range is exact when it reaches the
/// track count, while the row threshold preserves the old pixel-based trigger.
pub(super) fn should_extend(
    visible_range: Range<usize>,
    track_count: usize,
    row_height_px: f32,
    threshold_px: f32,
    is_infinite: bool,
    extension_in_flight: bool,
) -> bool {
    if !is_infinite || extension_in_flight || track_count == 0 {
        return false;
    }

    let threshold_rows = (threshold_px / row_height_px.max(1.0)).ceil() as usize;
    let threshold_start = track_count.saturating_sub(threshold_rows);
    let visible_end = visible_range.end.min(track_count);
    visible_end >= threshold_start
}

/// A callback-owned queue row snapshot. Keeping the track value here means a
/// lazy list callback never borrows playback state after the render returns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QueueRowSnapshot {
    pub(super) ordinal: usize,
    pub(super) index: usize,
    pub(super) count: usize,
    pub(super) track: PlaybackTrack,
    pub(super) blocked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_excludes_loading_row() {
        assert_eq!(visible_track_range(3..6, 5), 3..5);
        assert_eq!(visible_track_range(8..10, 5), 5..5);
        assert_eq!(track_index_for_list_item(4, 5), Some(4));
        assert_eq!(track_index_for_list_item(5, 5), None);
    }

    #[test]
    fn append_and_loading_row_changes_splice_without_reset() {
        let old = QueueListIdentity::new(&[10, 11], true);
        let new = QueueListIdentity::new(&[10, 11, 12], true);
        assert_eq!(
            list_change(&old, &new),
            QueueListChange::Splice {
                old_range: 2..3,
                new_count: 2,
            }
        );

        let old = QueueListIdentity::new(&[10, 11], false);
        let new = QueueListIdentity::new(&[10, 11], true);
        assert_eq!(
            list_change(&old, &new),
            QueueListChange::Splice {
                old_range: 2..2,
                new_count: 1,
            }
        );
    }

    #[test]
    fn reordered_or_removed_rows_reset_identity() {
        let old = QueueListIdentity::new(&[10, 11, 12], false);
        let reordered = QueueListIdentity::new(&[10, 12, 11], false);
        assert_eq!(
            list_change(&old, &reordered),
            QueueListChange::Reset { item_count: 3 }
        );

        let removed = QueueListIdentity::new(&[10, 11], false);
        assert_eq!(
            list_change(&old, &removed),
            QueueListChange::Reset { item_count: 2 }
        );
    }

    #[test]
    fn extension_threshold_and_end_semantics_are_pure() {
        assert!(!should_extend(0..2, 10, 59.0, 180.0, true, false));
        assert!(should_extend(6..10, 10, 59.0, 180.0, true, false));
        assert!(should_extend(9..10, 10, 59.0, 180.0, true, false));
        assert!(!should_extend(9..10, 10, 59.0, 180.0, false, false));
        assert!(!should_extend(9..10, 10, 59.0, 180.0, true, true));
    }
}
