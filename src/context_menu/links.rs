use crate::{
    playback::{PlaybackProvider, PlaybackTrack},
    search::Provider,
};

use super::EntityKind;

pub(super) fn valid_id(id: &str) -> Option<&str> {
    let id = id.trim();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

pub(crate) fn canonical_link(provider: Provider, id: &str) -> Option<String> {
    let id = valid_id(id)?;
    match provider {
        Provider::Deezer => Some(format!("https://www.deezer.com/track/{id}")),
        Provider::SoundCloud => None,
    }
}

pub(crate) fn canonical_collection_link(
    provider: Provider,
    kind: &EntityKind,
    id: &str,
) -> Option<String> {
    let id = valid_id(id)?;
    let path = match kind {
        EntityKind::Album => "album",
        EntityKind::Playlist => "playlist",
        EntityKind::Artist => "artist",
        EntityKind::Track => "track",
    };
    match provider {
        Provider::Deezer => Some(format!("https://www.deezer.com/{path}/{id}")),
        Provider::SoundCloud => None,
    }
}

/// Public web link for an entity. Deezer links are derived from the numeric
/// id; SoundCloud links need the provider-supplied permalink URL, mirroring
/// contextServiceUrl in public/js/core/service-urls.js. Returns None when no
/// valid link exists so menu items can render disabled.
pub(super) fn service_link(
    provider: Provider,
    kind: &EntityKind,
    id: &str,
    service_url: &str,
) -> Option<String> {
    match provider {
        Provider::Deezer if matches!(kind, EntityKind::Track) => canonical_link(provider, id),
        Provider::Deezer => canonical_collection_link(provider, kind, id),
        Provider::SoundCloud => {
            let url = service_url.trim();
            (!url.is_empty()).then(|| url.to_owned())
        }
    }
}

pub(super) fn playback_link(track: &PlaybackTrack) -> Option<String> {
    let provider = match track.provider {
        PlaybackProvider::Deezer => Provider::Deezer,
        PlaybackProvider::SoundCloud => Provider::SoundCloud,
    };
    service_link(provider, &EntityKind::Track, &track.id, &track.service_url)
}

pub(super) fn has_service_link(provider: Provider, id: &str, service_url: &str) -> bool {
    match provider {
        Provider::Deezer => valid_id(id).is_some(),
        Provider::SoundCloud => !service_url.trim().is_empty(),
    }
}

pub(super) fn has_playback_link(track: &PlaybackTrack) -> bool {
    match track.provider {
        PlaybackProvider::Deezer => valid_id(&track.id).is_some(),
        PlaybackProvider::SoundCloud => !track.service_url.trim().is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deezer_links_are_derived_from_numeric_ids() {
        assert_eq!(
            service_link(Provider::Deezer, &EntityKind::Track, "42", ""),
            Some("https://www.deezer.com/track/42".into())
        );
        assert_eq!(
            service_link(Provider::Deezer, &EntityKind::Album, "7", ""),
            Some("https://www.deezer.com/album/7".into())
        );
        assert_eq!(
            service_link(Provider::Deezer, &EntityKind::Track, "", ""),
            None
        );
        assert_eq!(
            service_link(Provider::Deezer, &EntityKind::Track, "42/a", ""),
            None
        );
    }

    #[test]
    fn soundcloud_links_require_a_supplied_permalink() {
        assert_eq!(
            service_link(
                Provider::SoundCloud,
                &EntityKind::Track,
                "9",
                "https://soundcloud.com/artist/song"
            ),
            Some("https://soundcloud.com/artist/song".into())
        );
        assert_eq!(
            service_link(Provider::SoundCloud, &EntityKind::Track, "9", ""),
            None
        );
        assert_eq!(
            service_link(Provider::SoundCloud, &EntityKind::Playlist, "9", "  "),
            None
        );
    }
}
