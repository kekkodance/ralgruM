#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollectionKind {
    Album,
    Playlist,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CollectionAction {
    Queue,
    Download,
    AddToPlaylist,
}

/// Copy for an empty collection action is kept in one place so library and
/// search collection cards describe the same operation consistently.
pub(crate) const fn empty_collection_notice(
    kind: CollectionKind,
    action: CollectionAction,
) -> (&'static str, &'static str) {
    match (kind, action) {
        (CollectionKind::Album, CollectionAction::Queue)
        | (CollectionKind::Album, CollectionAction::AddToPlaylist) => {
            ("Album Empty", "This album has no tracks to add.")
        }
        (CollectionKind::Album, CollectionAction::Download) => {
            ("Album Empty", "This album has no tracks to download.")
        }
        (CollectionKind::Playlist, CollectionAction::Queue)
        | (CollectionKind::Playlist, CollectionAction::AddToPlaylist) => {
            ("Playlist Empty", "This playlist has no tracks to add.")
        }
        (CollectionKind::Playlist, CollectionAction::Download) => {
            ("Playlist Empty", "This playlist has no tracks to download.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CollectionAction, CollectionKind, empty_collection_notice};

    #[test]
    fn empty_collection_notice_is_kind_and_action_aware() {
        assert_eq!(
            empty_collection_notice(CollectionKind::Album, CollectionAction::Queue),
            ("Album Empty", "This album has no tracks to add.")
        );
        assert_eq!(
            empty_collection_notice(CollectionKind::Album, CollectionAction::Download),
            ("Album Empty", "This album has no tracks to download.")
        );
        assert_eq!(
            empty_collection_notice(CollectionKind::Playlist, CollectionAction::Queue),
            ("Playlist Empty", "This playlist has no tracks to add.")
        );
        assert_eq!(
            empty_collection_notice(CollectionKind::Playlist, CollectionAction::Download),
            ("Playlist Empty", "This playlist has no tracks to download.")
        );
        assert_eq!(
            empty_collection_notice(CollectionKind::Album, CollectionAction::AddToPlaylist),
            ("Album Empty", "This album has no tracks to add.")
        );
        assert_eq!(
            empty_collection_notice(CollectionKind::Playlist, CollectionAction::AddToPlaylist),
            ("Playlist Empty", "This playlist has no tracks to add.")
        );
    }
}
