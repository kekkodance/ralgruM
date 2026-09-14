use std::{collections::HashSet, sync::Arc};

use gpui::{App, Context, Entity, Window};

use crate::{
    library::{FavoriteKey, FavoriteState},
    search::{Card, Provider, ResultType, TrackArtistRef},
};

/// Route of the page a track row is rendered on. Mirrors the context.route
/// input of albumRouteForTrackContext in public/js/core/service-urls.js.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContextRoute {
    pub(crate) provider: Provider,
    pub(crate) action: String,
    pub(crate) id: String,
    pub(crate) title: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MenuRouteKind {
    Album,
    Artist,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MenuRoute {
    pub(crate) kind: MenuRouteKind,
    pub(crate) id: String,
    pub(crate) title: String,
}

/// Metadata captured from a track when opening its album from a context menu.
/// Album cards opened from search results carry these same fields, so keeping
/// them on the navigation target avoids a blank detail header on track menus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AlbumNavigationTarget {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) artwork: String,
    pub(crate) release_date: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NavigationTarget {
    Album(AlbumNavigationTarget),
    Artist { id: String, title: String },
}

impl NavigationTarget {
    pub(crate) fn album(
        id: String,
        title: String,
        subtitle: String,
        artwork: String,
        release_date: String,
    ) -> Self {
        Self::Album(AlbumNavigationTarget {
            id,
            title,
            subtitle,
            artwork,
            release_date,
        })
    }

    pub(crate) fn artist(id: String, title: String) -> Self {
        Self::Artist { id, title }
    }

    pub(crate) fn card(self, source: Provider) -> Card {
        match self {
            Self::Album(album) => Card {
                kind: ResultType::Albums,
                source,
                id: album.id,
                title: album.title,
                subtitle: album.subtitle,
                artwork: album.artwork,
                release_date: album.release_date,
                ..Card::default()
            },
            Self::Artist { id, title } => Card {
                kind: ResultType::Artists,
                source,
                id,
                title,
                ..Card::default()
            },
        }
    }

    pub(crate) fn library_card(self, source: Provider) -> crate::library::Card {
        match self {
            Self::Album(album) => crate::library::Card {
                kind: crate::library::Category::Albums,
                source,
                id: album.id,
                title: album.title,
                subtitle: album.subtitle,
                artwork: album.artwork,
                badge: crate::library::release_year(&album.release_date),
                release_date: album.release_date,
                service_url: String::new(),
                is_private: None,
                library_service: None,
            },
            Self::Artist { id, title } => crate::library::Card {
                kind: crate::library::Category::Artists,
                source,
                id,
                title,
                ..crate::library::Card::default()
            },
        }
    }
}

pub(crate) type NavigationOpener = Arc<dyn Fn(NavigationTarget, &mut Window, &mut App)>;

/// Navigation callbacks for the two provider families used by track menus.
/// Keeping the provider split at the wiring boundary lets menu renderers stay
/// provider agnostic while still opening the canonical Search detail route.
#[derive(Clone)]
pub(crate) struct ProviderNavigationOpeners {
    deezer: (NavigationOpener, NavigationOpener),
    soundcloud: (NavigationOpener, NavigationOpener),
}

impl ProviderNavigationOpeners {
    pub(crate) fn new(
        deezer: (NavigationOpener, NavigationOpener),
        soundcloud: (NavigationOpener, NavigationOpener),
    ) -> Self {
        Self { deezer, soundcloud }
    }

    pub(crate) fn for_provider(&self, provider: Provider) -> (NavigationOpener, NavigationOpener) {
        match provider {
            Provider::Deezer => self.deezer.clone(),
            Provider::SoundCloud => self.soundcloud.clone(),
        }
    }
}

fn trimmed_non_empty<'a>(values: &[&'a str]) -> Option<&'a str> {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
}

fn valid_id(id: &str) -> Option<&str> {
    let id = id.trim();
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then_some(id)
}

/// Mirrors albumRouteForTrackContext in public/js/core/service-urls.js for the
/// fields this app models: Deezer prefers the track album data and falls back
/// to an albumTracks context route; SoundCloud accepts the album identity a
/// track carries when it was loaded from an album page, and otherwise only a
/// SoundCloud albumTracks context route. The track-carried identity keeps
/// menus away from the album page (the player bar, the queue) resolving the
/// album exactly like the album tracklist menu does.
pub(crate) fn album_route_for_track(
    provider: Provider,
    album_id: &str,
    album_title: &str,
    context: Option<&ContextRoute>,
) -> Option<MenuRoute> {
    match provider {
        Provider::Deezer => {
            let route_is_album = context.is_some_and(|route| {
                route.provider == Provider::Deezer && route.action == "albumTracks"
            });
            let context = if route_is_album { context } else { None };
            let id = [
                album_id,
                context.map(|route| route.id.as_str()).unwrap_or(""),
            ]
            .into_iter()
            .find_map(valid_id)?;
            let title = trimmed_non_empty(&[
                album_title,
                context.map(|route| route.title.as_str()).unwrap_or(""),
            ])?;
            if id == "0" || title == "Deezer Track" {
                return None;
            }
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: id.to_owned(),
                title: title.to_owned(),
            })
        }
        Provider::SoundCloud => {
            if let (Some(id), Some(title)) = (
                trimmed_non_empty(&[album_id]),
                trimmed_non_empty(&[album_title]),
            ) {
                if title != "SoundCloud Upload" {
                    return Some(MenuRoute {
                        kind: MenuRouteKind::Album,
                        id: id.to_owned(),
                        title: title.to_owned(),
                    });
                }
            }
            let route = context?;
            if route.provider != Provider::SoundCloud || route.action != "albumTracks" {
                return None;
            }
            let id = trimmed_non_empty(&[album_id, &route.id])?;
            let title = trimmed_non_empty(&[album_title, &route.title])?;
            if title == "SoundCloud Upload" {
                return None;
            }
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: id.to_owned(),
                title: title.to_owned(),
            })
        }
    }
}

/// Builds the typed album target used by track context menus. The route ID and
/// title follow the same provider-specific fallback rules as
/// [`album_route_for_track`], while the remaining fields come from the
/// clicked track.
pub(crate) fn album_target_for_track(
    provider: Provider,
    album_id: &str,
    album_title: &str,
    artwork: &str,
    subtitle: &str,
    release_date: &str,
    context: Option<&ContextRoute>,
) -> Option<NavigationTarget> {
    let route = album_route_for_track(provider, album_id, album_title, context)?;
    Some(NavigationTarget::album(
        route.id,
        route.title,
        subtitle.trim().to_owned(),
        artwork.trim().to_owned(),
        release_date.trim().to_owned(),
    ))
}

impl MenuRoute {
    pub(crate) fn navigation_target(&self) -> Option<NavigationTarget> {
        (self.kind == MenuRouteKind::Artist)
            .then(|| NavigationTarget::artist(self.id.clone(), self.title.clone()))
    }
}

/// Mirrors artistRoutesForContext in public/js/core/service-urls.js for track
/// contexts: SoundCloud yields at most one entry, Deezer one entry per artist
/// with a numeric non-empty id.
pub(crate) fn artist_routes_for_track(
    provider: Provider,
    artists: &[TrackArtistRef],
) -> Vec<MenuRoute> {
    match provider {
        Provider::SoundCloud => artists
            .iter()
            .find(|artist| valid_id(&artist.id).is_some() && !artist.name.trim().is_empty())
            .map(|artist| {
                vec![MenuRoute {
                    kind: MenuRouteKind::Artist,
                    id: artist.id.trim().to_owned(),
                    title: artist.name.trim().to_owned(),
                }]
            })
            .unwrap_or_default(),
        Provider::Deezer => {
            let mut seen = HashSet::new();
            artists
                .iter()
                .filter_map(|artist| {
                    let id = valid_id(&artist.id)?;
                    let title = artist.name.trim();
                    if title.is_empty() || !seen.insert(id.to_owned()) {
                        return None;
                    }
                    Some(MenuRoute {
                        kind: MenuRouteKind::Artist,
                        id: id.to_owned(),
                        title: title.to_owned(),
                    })
                })
                .collect()
        }
    }
}

/// Abstraction over the view that owns a track row so search rows and library
/// rows can share the same context menu behavior.
pub(crate) trait TrackMenuHost: 'static {
    fn favorites_entity(&self) -> Entity<FavoriteState>;
    fn local_track_saved(&self, _track: &crate::playback::PlaybackTrack, _cx: &gpui::App) -> bool {
        false
    }
    fn set_local_track_saved(
        &mut self,
        _track: crate::playback::PlaybackTrack,
        _saved: bool,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }
    /// Whether this host owns a cache row and can expose its removal action.
    /// Other track-row hosts keep this capability disabled by default.
    fn can_remove_from_cache(&self, _provider: Provider, _track_id: &str) -> bool {
        false
    }
    fn remove_from_cache(&mut self, _provider: Provider, _track_id: String, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }
    fn resolve_favorite_state(&mut self, _key: FavoriteKey, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }
    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        Self: Sized;
    /// Opens the shared picker for an independent Local playlist. The full
    /// playback track is required so mixed-provider metadata survives the
    /// round trip into the local store.
    fn open_local_playlist_picker(
        &mut self,
        _track: crate::playback::PlaybackTrack,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }
    /// Warms the playlist catalog so the Add to playlist picker opens
    /// instantly. Implementations must reuse the guarded catalog load.
    fn preload_playlist_catalog(&mut self, _provider: Provider, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }
    fn toggle_favorite_state(
        &mut self,
        provider: Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) where
        Self: Sized;

    /// Starts a Deezer track mix through the owning library view. The default
    /// keeps non-library hosts source-compatible while they opt into the
    /// Deezer action surface.
    fn start_deezer_track_mix(&mut self, _track_id: String, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }

    /// Starts a Deezer artist mix through the owning library view.
    fn start_deezer_artist_mix(&mut self, _artist_id: String, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }

    /// Starts a SoundCloud artist station through the existing infinite
    /// station queue and tail-extension flow.
    fn start_soundcloud_artist_station(&mut self, _artist_id: String, _cx: &mut Context<Self>)
    where
        Self: Sized,
    {
    }

    /// Starts a SoundCloud track station through the existing infinite station
    /// queue and tail-extension flow.
    fn start_soundcloud_track_station(
        &mut self,
        _track: crate::playback::PlaybackTrack,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }

    /// Sends a Deezer recommendation feedback operation. Implementations own
    /// request de-duplication and toast reporting.
    fn add_negative_feedback(
        &mut self,
        _kind: crate::library::DeezerFeedbackKind,
        _id: String,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }

    /// Opens a focused Similar Artists route for an artist card.
    fn open_similar_artists(
        &mut self,
        _artist_id: String,
        _title: String,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }

    /// Toggles an artist favorite from an artist-card menu.
    fn toggle_artist_favorite(
        &mut self,
        _provider: Provider,
        _artist_id: String,
        _known_favorite: bool,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }

    /// Opens the track Info dialog. The default keeps non-library hosts
    /// source-compatible while they opt into the Deezer action surface.
    fn open_track_info(
        &mut self,
        _provider: Provider,
        _track_id: String,
        _seed: crate::library::TrackInfo,
        _deezer_arl: Option<crate::search::DeezerArl>,
        _soundcloud_token: Option<crate::search::SoundCloudToken>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }

    /// Warms the track Info cache while the menu is open so the dialog
    /// opens populated. Implementations must reuse the guarded preload.
    fn preload_track_info(
        &mut self,
        _provider: Provider,
        _track_id: String,
        _deezer_arl: Option<crate::search::DeezerArl>,
        _soundcloud_token: Option<crate::search::SoundCloudToken>,
        _cx: &mut Context<Self>,
    ) where
        Self: Sized,
    {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deezer_context(action: &str, id: &str, title: &str) -> ContextRoute {
        ContextRoute {
            provider: Provider::Deezer,
            action: action.into(),
            id: id.into(),
            title: title.into(),
        }
    }

    #[test]
    fn deezer_album_prefers_track_data_and_falls_back_to_context() {
        let from_track = album_route_for_track(
            Provider::Deezer,
            " 12 ",
            "Album",
            Some(&deezer_context("albumTracks", "99", "Other")),
        );
        assert_eq!(
            from_track,
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "12".into(),
                title: "Album".into(),
            })
        );

        let from_context = album_route_for_track(
            Provider::Deezer,
            "",
            "",
            Some(&deezer_context("albumTracks", "99", "Context Album")),
        );
        assert_eq!(
            from_context,
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "99".into(),
                title: "Context Album".into(),
            })
        );

        assert_eq!(
            album_route_for_track(
                Provider::Deezer,
                "",
                "",
                Some(&deezer_context("playlistTracks", "99", "Context Album")),
            ),
            None
        );
        assert_eq!(album_route_for_track(Provider::Deezer, "", "", None), None);
    }

    #[test]
    fn deezer_album_rejects_placeholder_ids_and_titles() {
        let context = deezer_context("albumTracks", "99", "Album");
        assert_eq!(
            album_route_for_track(Provider::Deezer, "0", "Album", Some(&context)),
            None
        );
        assert_eq!(
            album_route_for_track(
                Provider::Deezer,
                "",
                "Album",
                Some(&deezer_context("albumTracks", "", "Album")),
            ),
            None
        );
        assert_eq!(
            album_route_for_track(Provider::Deezer, "12", "Deezer Track", Some(&context)),
            None
        );
        assert_eq!(
            album_route_for_track(
                Provider::Deezer,
                "",
                "",
                Some(&deezer_context("albumTracks", "0", "Album")),
            ),
            None
        );
        assert_eq!(
            album_route_for_track(Provider::Deezer, "12/a", "Album", None),
            None
        );
        assert_eq!(
            album_route_for_track(
                Provider::Deezer,
                "",
                "Album",
                Some(&deezer_context("albumTracks", "99/b", "Album")),
            ),
            None
        );
        assert_eq!(
            album_route_for_track(Provider::Deezer, "12/a", "Album", Some(&context)),
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "99".into(),
                title: "Album".into(),
            })
        );
    }

    #[test]
    fn soundcloud_album_prefers_track_identity_and_falls_back_to_context() {
        // Tracks loaded from a SoundCloud album page carry the album id and
        // title, so the player bar and queue menus resolve the album without
        // any page context, exactly like the album tracklist menu does.
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "7", "Album", None),
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "7".into(),
                title: "Album".into(),
            })
        );
        let wrong_action = ContextRoute {
            provider: Provider::SoundCloud,
            action: "playlistTracks".into(),
            id: "9".into(),
            title: "Playlist".into(),
        };
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "7", "Album", Some(&wrong_action)),
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "7".into(),
                title: "Album".into(),
            })
        );
        let wrong_provider = deezer_context("albumTracks", "9", "Album");
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "7", "Album", Some(&wrong_provider)),
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "7".into(),
                title: "Album".into(),
            })
        );

        let context = ContextRoute {
            provider: Provider::SoundCloud,
            action: "albumTracks".into(),
            id: "9".into(),
            title: "Context Album".into(),
        };
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "", "", Some(&context)),
            Some(MenuRoute {
                kind: MenuRouteKind::Album,
                id: "9".into(),
                title: "Context Album".into(),
            })
        );
        assert_eq!(
            album_route_for_track(
                Provider::SoundCloud,
                "",
                "SoundCloud Upload",
                Some(&context)
            ),
            None
        );
        assert_eq!(
            album_route_for_track(
                Provider::SoundCloud,
                "",
                "",
                Some(&ContextRoute {
                    provider: Provider::SoundCloud,
                    action: "albumTracks".into(),
                    id: String::new(),
                    title: "Context Album".into(),
                })
            ),
            None
        );
    }

    #[test]
    fn soundcloud_album_track_identity_needs_both_id_and_title() {
        // Standalone SoundCloud tracks never carry an album id, so a
        // publisher album name alone must not invent an album entry.
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "", "Album", None),
            None
        );
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "7", "", None),
            None
        );
        assert_eq!(
            album_route_for_track(Provider::SoundCloud, "7", "SoundCloud Upload", None),
            None
        );
    }

    #[test]
    fn album_target_matches_a_populated_album_card_and_detail_route() {
        let target = album_target_for_track(
            Provider::Deezer,
            "12",
            "Album",
            "https://cdn.example/artwork.jpg",
            "Artist",
            "2024-01-02",
            None,
        )
        .expect("track album target");
        let target_card = target.clone().card(Provider::Deezer);
        let normal_card = crate::search::Card {
            kind: ResultType::Albums,
            id: "12".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: "https://cdn.example/artwork.jpg".into(),
            source: Provider::Deezer,
            release_date: "2024-01-02".into(),
            ..crate::search::Card::default()
        };
        assert_eq!(target_card, normal_card);
        assert_eq!(
            crate::search::DetailRoute::from_card(&target_card),
            crate::search::DetailRoute::from_card(&normal_card)
        );
        assert_eq!(target_card.artwork, "https://cdn.example/artwork.jpg");
    }

    #[test]
    fn soundcloud_album_target_keeps_track_metadata() {
        let context = ContextRoute {
            provider: Provider::SoundCloud,
            action: "albumTracks".into(),
            id: "9".into(),
            title: "Context Album".into(),
        };
        let target = album_target_for_track(
            Provider::SoundCloud,
            "",
            "",
            "https://cdn.example/soundcloud-artwork.jpg",
            "Uploader",
            "2023-10-11",
            Some(&context),
        )
        .expect("SoundCloud track album target");
        let card = target.card(Provider::SoundCloud);
        assert_eq!(card.kind, ResultType::Albums);
        assert_eq!(card.id, "9");
        assert_eq!(card.title, "Context Album");
        assert_eq!(card.subtitle, "Uploader");
        assert_eq!(card.artwork, "https://cdn.example/soundcloud-artwork.jpg");
        assert_eq!(card.release_date, "2023-10-11");
        assert!(crate::search::DetailRoute::from_card(&card).is_some());
    }

    #[test]
    fn artist_target_stays_an_artist_card_without_album_metadata() {
        let card =
            NavigationTarget::artist("44".into(), "Artist".into()).card(Provider::SoundCloud);
        assert_eq!(card.kind, ResultType::Artists);
        assert_eq!(card.source, Provider::SoundCloud);
        assert_eq!(card.id, "44");
        assert_eq!(card.title, "Artist");
        assert!(card.subtitle.is_empty());
        assert!(card.artwork.is_empty());
        assert!(card.release_date.is_empty());
    }

    #[test]
    fn deezer_artist_routes_filter_missing_and_non_numeric_ids() {
        let artists = vec![
            TrackArtistRef {
                id: "11".into(),
                name: "First".into(),
            },
            TrackArtistRef {
                id: String::new(),
                name: "No Id".into(),
            },
            TrackArtistRef {
                id: "22/a".into(),
                name: "Bad Id".into(),
            },
            TrackArtistRef {
                id: "33".into(),
                name: String::new(),
            },
            TrackArtistRef {
                id: "44".into(),
                name: "Last".into(),
            },
        ];
        assert_eq!(
            artist_routes_for_track(Provider::Deezer, &artists),
            vec![
                MenuRoute {
                    kind: MenuRouteKind::Artist,
                    id: "11".into(),
                    title: "First".into(),
                },
                MenuRoute {
                    kind: MenuRouteKind::Artist,
                    id: "44".into(),
                    title: "Last".into(),
                },
            ]
        );
    }

    #[test]
    fn soundcloud_artist_routes_yield_at_most_one_entry() {
        let artists = vec![
            TrackArtistRef {
                id: String::new(),
                name: "No Id".into(),
            },
            TrackArtistRef {
                id: " 77 ".into(),
                name: " Kept ".into(),
            },
            TrackArtistRef {
                id: "88".into(),
                name: "Ignored".into(),
            },
        ];
        assert_eq!(
            artist_routes_for_track(Provider::SoundCloud, &artists),
            vec![MenuRoute {
                kind: MenuRouteKind::Artist,
                id: "77".into(),
                title: "Kept".into(),
            }]
        );
        assert!(artist_routes_for_track(Provider::SoundCloud, &[]).is_empty());
    }

    #[test]
    fn deezer_artist_routes_keep_all_collaborators_but_dedupe_ids() {
        let routes = artist_routes_for_track(
            Provider::Deezer,
            &[
                TrackArtistRef {
                    id: "7".into(),
                    name: "Primary".into(),
                },
                TrackArtistRef {
                    id: "7".into(),
                    name: "Primary duplicate".into(),
                },
                TrackArtistRef {
                    id: "8".into(),
                    name: "Collaborator".into(),
                },
            ],
        );
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].id, "7");
        assert_eq!(routes[1].id, "8");
    }
}
