use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Service {
    Local,
    #[default]
    Deezer,
    SoundCloud,
}

impl Service {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Local => "Local",
            Self::Deezer => "Deezer",
            Self::SoundCloud => "SoundCloud",
        }
    }

    pub(crate) const fn categories(self) -> &'static [Category] {
        match self {
            Self::Local => &Category::LOCAL,
            Self::Deezer => &Category::DEEZER,
            Self::SoundCloud => &Category::SOUNDCLOUD,
        }
    }

    pub(crate) const fn default_category(self) -> Category {
        match self {
            Self::Local => Category::Tracks,
            Self::Deezer => Category::Tracks,
            Self::SoundCloud => Category::MyTracks,
        }
    }
}

use crate::search::Provider;

pub(crate) const FLOW_TRACK_DESCRIPTION: &str = "A fresh personalized queue.";

/// A library route is opened once it sits below its selected root category.
/// Keep this depth rule shared by the shell, header, and content boundaries so
/// every detail route gets the same navigation chrome.
pub(crate) const fn is_detail_route(route_depth: usize) -> bool {
    route_depth > 1
}

pub(crate) fn is_deezer_flow_detail(service: Service, route: &Route, route_depth: usize) -> bool {
    service == Service::Deezer
        && route.source == Provider::Deezer
        && route.category == Category::Flow
        && route.action == "flowTracks"
        && is_detail_route(route_depth)
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Category {
    #[default]
    Tracks,
    Albums,
    Artists,
    Playlists,
    History,
    Flow,
    MyTracks,
    Station,
}

impl Category {
    pub(crate) const LOCAL: [Self; 2] = [Self::Tracks, Self::Playlists];
    pub(crate) const DEEZER: [Self; 6] = [
        Self::Tracks,
        Self::Albums,
        Self::Artists,
        Self::Playlists,
        Self::History,
        Self::Flow,
    ];
    pub(crate) const SOUNDCLOUD: [Self; 7] = [
        Self::MyTracks,
        Self::Tracks,
        Self::Albums,
        Self::Artists,
        Self::Playlists,
        Self::History,
        Self::Station,
    ];
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::MyTracks => "My Tracks",
            Self::Tracks => "Tracks",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Playlists => "Playlists",
            Self::History => "History",
            Self::Flow => "Flow",
            Self::Station => "Station",
        }
    }
    pub(crate) const fn action(self) -> &'static str {
        match self {
            Self::MyTracks => "myTracks",
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
            Self::History => "history",
            Self::Flow => "flow",
            Self::Station => "station",
        }
    }
}

/// The heading and supporting copy shown for each service's root library tab.
/// Keep this matrix as the single source of truth for the library shell and
/// the provider loaders so loading and loaded states cannot drift apart.
pub(crate) fn root_copy(service: Service, category: Category) -> (&'static str, &'static str) {
    match (service, category) {
        (Service::Local, Category::Tracks) => (
            "Local Tracks",
            "Tracks saved to your local library from Deezer and SoundCloud.",
        ),
        (Service::Local, Category::Playlists) => (
            "Local Playlists",
            "Playlists organized in your local library with tracks from Deezer and SoundCloud.",
        ),
        (Service::Deezer, Category::Tracks) => ("Liked Tracks", "Tracks you liked on Deezer."),
        (Service::Deezer, Category::Albums) => ("Favorite Albums", "Albums you liked on Deezer."),
        (Service::Deezer, Category::Artists) => {
            ("Followed Artists", "Artists you followed on Deezer.")
        }
        (Service::Deezer, Category::Playlists) => {
            ("Playlists", "Your playlist you saved or created on Deezer.")
        }
        (Service::Deezer, Category::History) => (
            "Listening History",
            "Your most recently played Deezer tracks.",
        ),
        (Service::Deezer, Category::Flow) => ("Flow", "Personalized moods and genres for you."),
        (Service::SoundCloud, Category::MyTracks) => {
            ("My Tracks", "Tracks uploaded on your SoundCloud account.")
        }
        (Service::SoundCloud, Category::Tracks) => {
            ("Liked Tracks", "Tracks you liked on SoundCloud.")
        }
        (Service::SoundCloud, Category::Albums) => ("Albums", "Albums you liked on SoundCloud."),
        (Service::SoundCloud, Category::Artists) => {
            ("Followed Artists", "Artists you followed on SoundCloud.")
        }
        (Service::SoundCloud, Category::Playlists) => (
            "Playlists",
            "Your playlist you saved or created on SoundCloud.",
        ),
        (Service::SoundCloud, Category::History) => (
            "Track History",
            "Your most recently played SoundCloud tracks.",
        ),
        (Service::SoundCloud, Category::Station) => (
            "Station",
            "Start a continuous mix from one of your liked tracks.",
        ),
        _ => unreachable!("unsupported service library category"),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Track {
    /// Original provider for tracks rendered in the account-independent Local library.
    /// Remote library pages leave this unset and use their page provider.
    pub origin: Option<Provider>,
    pub id: String,
    pub title: String,
    pub artist: String,
    pub artists: Vec<crate::search::TrackArtistRef>,
    pub album: String,
    pub album_id: String,
    pub release_date: String,
    pub duration: u64,
    pub artwork: String,
    pub explicit: bool,
    pub ai_generated: bool,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id instead.
    pub service_url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Card {
    pub kind: Category,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub artwork: String,
    /// Canonical provider release date used for navigation and detail
    /// metadata. `badge` remains the compact display value shown on cards.
    pub release_date: String,
    pub badge: String,
    pub source: Provider,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id instead.
    pub service_url: String,
    /// Playlist privacy when the provider reports it. `None` hides the icon.
    pub is_private: Option<bool>,
    /// Local cards stay inside Library and must not be routed through a
    /// provider account or Search detail.
    pub library_service: Option<Service>,
}

impl Card {
    /// Converts a library collection card to the shape used by Search detail
    /// and shared collection-info consumers. Activation decides separately
    /// whether a card stays in Library.
    pub(crate) fn search_card(&self) -> Option<crate::search::Card> {
        if self.library_service == Some(Service::Local) {
            return None;
        }
        let kind = match self.kind {
            Category::Albums => crate::search::ResultType::Albums,
            Category::Artists => crate::search::ResultType::Artists,
            Category::Playlists => crate::search::ResultType::Playlists,
            Category::Flow | Category::Station => return None,
            _ => return None,
        };

        let card = crate::search::Card {
            kind,
            id: self.id.clone(),
            title: self.title.clone(),
            subtitle: self.subtitle.clone(),
            artwork: self.artwork.clone(),
            source: self.source,
            ai_generated: false,
            badge: self.badge.clone(),
            release_date: if self.kind == Category::Albums {
                if self.release_date.trim().is_empty() {
                    self.badge.clone()
                } else {
                    self.release_date.clone()
                }
            } else {
                String::new()
            },
            service_url: self.service_url.clone(),
        };
        crate::search::DetailRoute::from_card(&card)
            .is_some()
            .then_some(card)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Route {
    pub source: Provider,
    pub category: Category,
    pub action: String,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub artwork: String,
    pub release_date: String,
}

impl Route {
    pub(crate) fn root(service: Service, category: Category) -> Self {
        Self {
            source: match service {
                // Local routes never reach a provider client. Track-level origin
                // remains authoritative for mixed-provider playback.
                Service::Local => Provider::Deezer,
                Service::Deezer => Provider::Deezer,
                Service::SoundCloud => Provider::SoundCloud,
            },
            category,
            action: if service == Service::Local {
                match category {
                    Category::Tracks => "localTracks",
                    Category::Playlists => "localPlaylists",
                    _ => category.action(),
                }
                .into()
            } else {
                category.action().into()
            },
            id: String::new(),
            title: category.label().into(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
        }
    }

    pub(crate) fn is_local_root(&self) -> bool {
        self.is_local_tracks_root() || self.is_local_playlists_root()
    }

    pub(crate) fn is_local_tracks_root(&self) -> bool {
        self.action == "localTracks" && self.id.is_empty()
    }

    pub(crate) fn is_local_playlists_root(&self) -> bool {
        self.action == "localPlaylists" && self.id.is_empty()
    }

    pub(crate) fn is_local_playlist_detail(&self) -> bool {
        self.action == "localPlaylistTracks" && !self.id.is_empty()
    }

    pub(crate) fn is_local_route(&self) -> bool {
        self.is_local_root() || self.is_local_playlist_detail()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Page {
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub album_info: Option<crate::search::AlbumInfo>,
    pub artwork: String,
    /// Provider-supplied public web URL of the loaded collection (SoundCloud
    /// permalink). Deezer links are derived from the numeric id instead.
    pub service_url: String,
    pub platform: Option<Service>,
    pub count_noun: String,
    pub meta_text: String,
    pub show_count: bool,
    pub total: usize,
    pub raw_loaded_count: usize,
    pub normalized_count: usize,
    pub authoritative_total: Option<usize>,
    pub tracks: Vec<Track>,
    pub cards: Vec<Card>,
    pub sections: Vec<Section>,
    /// Continuation metadata returned by a Deezer radio request.
    pub next_flow_tuner: Option<super::deezer_radio::FlowTuner>,
    /// Title supplied by a Deezer SmartMix detail endpoint, if present.
    pub resolved_smart_mix_title: Option<String>,
    pub clear_remaining_tracks: bool,
    pub empty_title: String,
    pub empty_description: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Section {
    pub title: String,
    pub description: String,
    pub total: usize,
    pub show_count: bool,
    pub layout: SectionLayout,
    pub preview_limit: Option<usize>,
    pub card_row: bool,
    pub tracks: Vec<Track>,
    pub cards: Vec<Card>,
    pub empty_message: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SectionLayout {
    #[default]
    Tracks,
    Cards,
}

impl Page {
    pub(crate) fn playlist_removal_proven(&self) -> bool {
        self.authoritative_total == Some(self.raw_loaded_count)
            && self.normalized_count == self.raw_loaded_count
    }

    pub(crate) fn uses_sections(&self) -> bool {
        !self.sections.is_empty()
    }

    pub(crate) fn displayed_total(&self) -> usize {
        if self.uses_sections() {
            self.sections
                .iter()
                .map(|section| section.tracks.len() + section.cards.len())
                .sum()
        } else {
            self.tracks.len() + self.cards.len()
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        if self.uses_sections() {
            self.sections
                .iter()
                .all(|section| section.tracks.is_empty() && section.cards.is_empty())
        } else {
            self.tracks.is_empty() && self.cards.is_empty()
        }
    }

    pub(crate) fn owns_top_level_count(&self) -> bool {
        self.show_count && !self.uses_sections()
    }
}

pub(crate) fn section_count_visible(section: &Section) -> bool {
    section.show_count
}

pub(crate) fn value_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(v)) => v.clone(),
        Some(Value::Number(v)) => v.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn release_year(date: &str) -> String {
    date.get(..4)
        .filter(|year| year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_route_depth_starts_below_the_selected_root() {
        assert!(!is_detail_route(0));
        assert!(!is_detail_route(1));
        assert!(is_detail_route(2));
    }

    #[test]
    fn collection_cards_preserve_metadata_for_shared_info_cards() {
        let card = Card {
            kind: Category::Albums,
            id: "42".into(),
            title: "Album".into(),
            subtitle: "Artist".into(),
            artwork: "https://example.test/album.jpg".into(),
            release_date: "2024-05-18".into(),
            badge: "2024".into(),
            source: Provider::SoundCloud,
            service_url: "https://soundcloud.com/artist/sets/album".into(),
            is_private: None,
            library_service: None,
        };

        let search_card = card
            .search_card()
            .expect("album converts to a shared info card");
        assert_eq!(search_card.kind, crate::search::ResultType::Albums);
        assert_eq!(search_card.id, "42");
        assert_eq!(search_card.title, "Album");
        assert_eq!(search_card.subtitle, "Artist");
        assert_eq!(search_card.artwork, "https://example.test/album.jpg");
        assert_eq!(search_card.source, Provider::SoundCloud);
        assert_eq!(search_card.badge, "2024");
        assert_eq!(search_card.release_date, "2024-05-18");
        assert_eq!(
            search_card.service_url,
            "https://soundcloud.com/artist/sets/album"
        );
        let route = crate::search::DetailRoute::from_card(&search_card)
            .expect("album info card retains the canonical detail route");
        assert_eq!(route.kind, crate::search::ResultType::Albums);
        assert_eq!(route.id, "42");
    }

    #[test]
    fn artist_and_playlist_cards_convert_to_search_card_shape() {
        for (kind, expected_kind) in [
            (Category::Artists, crate::search::ResultType::Artists),
            (Category::Playlists, crate::search::ResultType::Playlists),
        ] {
            let card = Card {
                kind,
                id: "7".into(),
                ..Card::default()
            };
            let search_card = card.search_card().expect("collection card converts");
            assert_eq!(search_card.kind, expected_kind);
            assert!(crate::search::DetailRoute::from_card(&search_card).is_some());
        }

        for kind in [Category::Flow, Category::Station, Category::Tracks] {
            let card = Card {
                kind,
                id: "7".into(),
                ..Card::default()
            };
            assert!(card.search_card().is_none());
        }

        assert!(
            Card {
                kind: Category::Artists,
                id: "not-numeric".into(),
                ..Card::default()
            }
            .search_card()
            .is_none()
        );

        assert!(
            Card {
                kind: Category::Playlists,
                id: "local-1".into(),
                library_service: Some(Service::Local),
                ..Card::default()
            }
            .search_card()
            .is_none()
        );
    }

    #[test]
    fn service_categories_match_captured_library_contracts() {
        assert_eq!(
            Service::Local.categories(),
            &[Category::Tracks, Category::Playlists]
        );
        assert_eq!(Service::Local.default_category(), Category::Tracks);
        assert!(Service::Deezer.categories().contains(&Category::Albums));
        assert!(Service::Deezer.categories().contains(&Category::Playlists));
        assert!(Service::Deezer.categories().contains(&Category::Flow));
        assert_eq!(
            Service::SoundCloud.categories(),
            &[
                Category::MyTracks,
                Category::Tracks,
                Category::Albums,
                Category::Artists,
                Category::Playlists,
                Category::History,
                Category::Station,
            ]
        );
    }

    #[test]
    fn local_root_is_distinct_from_provider_routes() {
        let route = Route::root(Service::Local, Category::Tracks);
        assert!(route.is_local_root());
        assert!(route.is_local_tracks_root());
        assert!(!route.is_local_playlists_root());
        assert_eq!(
            root_copy(Service::Local, Category::Tracks).0,
            "Local Tracks"
        );
        let playlists = Route::root(Service::Local, Category::Playlists);
        assert!(playlists.is_local_root());
        assert!(playlists.is_local_playlists_root());
        assert_eq!(playlists.action, "localPlaylists");
        assert_eq!(
            root_copy(Service::Local, Category::Playlists).0,
            "Local Playlists"
        );
    }

    #[test]
    fn root_copy_covers_every_supported_service_category() {
        let expected = [
            (
                Service::Local,
                Category::Tracks,
                "Local Tracks",
                "Tracks saved to your local library from Deezer and SoundCloud.",
            ),
            (
                Service::Local,
                Category::Playlists,
                "Local Playlists",
                "Playlists organized in your local library with tracks from Deezer and SoundCloud.",
            ),
            (
                Service::Deezer,
                Category::Tracks,
                "Liked Tracks",
                "Tracks you liked on Deezer.",
            ),
            (
                Service::Deezer,
                Category::Albums,
                "Favorite Albums",
                "Albums you liked on Deezer.",
            ),
            (
                Service::Deezer,
                Category::Artists,
                "Followed Artists",
                "Artists you followed on Deezer.",
            ),
            (
                Service::Deezer,
                Category::Playlists,
                "Playlists",
                "Your playlist you saved or created on Deezer.",
            ),
            (
                Service::Deezer,
                Category::History,
                "Listening History",
                "Your most recently played Deezer tracks.",
            ),
            (
                Service::Deezer,
                Category::Flow,
                "Flow",
                "Personalized moods and genres for you.",
            ),
            (
                Service::SoundCloud,
                Category::MyTracks,
                "My Tracks",
                "Tracks uploaded on your SoundCloud account.",
            ),
            (
                Service::SoundCloud,
                Category::Tracks,
                "Liked Tracks",
                "Tracks you liked on SoundCloud.",
            ),
            (
                Service::SoundCloud,
                Category::Albums,
                "Albums",
                "Albums you liked on SoundCloud.",
            ),
            (
                Service::SoundCloud,
                Category::Artists,
                "Followed Artists",
                "Artists you followed on SoundCloud.",
            ),
            (
                Service::SoundCloud,
                Category::Playlists,
                "Playlists",
                "Your playlist you saved or created on SoundCloud.",
            ),
            (
                Service::SoundCloud,
                Category::History,
                "Track History",
                "Your most recently played SoundCloud tracks.",
            ),
            (
                Service::SoundCloud,
                Category::Station,
                "Station",
                "Start a continuous mix from one of your liked tracks.",
            ),
        ];

        for (service, category, title, description) in expected {
            assert_eq!(root_copy(service, category), (title, description));
        }
    }

    #[test]
    fn flat_pages_own_their_body_and_displayed_total() {
        let page = Page {
            tracks: vec![Track::default()],
            cards: vec![Card::default()],
            ..Page::default()
        };

        assert!(!page.uses_sections());
        assert_eq!(page.displayed_total(), 2);
        assert!(!page.is_empty());
    }

    #[test]
    fn sectioned_pages_ignore_duplicated_top_level_items() {
        let page = Page {
            tracks: vec![Track::default()],
            sections: vec![Section {
                tracks: vec![Track::default(), Track::default()],
                ..Section::default()
            }],
            ..Page::default()
        };

        assert!(page.uses_sections());
        assert_eq!(page.displayed_total(), 2);
        assert!(!page.is_empty());
        assert!(!page.owns_top_level_count());
    }

    #[test]
    fn artist_owned_pages_do_not_own_a_top_level_count() {
        let page = Page {
            sections: vec![Section::default()],
            ..Page::default()
        };

        assert!(!page.owns_top_level_count());
    }

    #[test]
    fn section_count_visibility_requires_an_explicit_contract() {
        assert!(!section_count_visible(&Section::default()));
        assert!(section_count_visible(&Section {
            show_count: true,
            ..Section::default()
        }));
    }
}
