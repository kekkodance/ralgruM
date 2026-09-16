use super::models::{ArtistPage, Card, Provider, ProviderError, ResultType, Track};
use chrono::{Datelike, NaiveDate};
use gpui::{Pixels, Point, point, px};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArtistSection {
    PopularTracks,
    SimilarArtists,
    Albums,
    Featured,
    Playlists,
}

pub(crate) fn artist_section_preview_limit(section: ArtistSection) -> usize {
    match section {
        ArtistSection::PopularTracks => 5,
        ArtistSection::SimilarArtists
        | ArtistSection::Albums
        | ArtistSection::Featured
        | ArtistSection::Playlists => 12,
    }
}

pub(crate) fn artist_section_has_items(loaded_count: usize) -> bool {
    loaded_count > 0
}

pub(crate) fn artist_section_shows_action(
    section: ArtistSection,
    loaded_count: usize,
    expanded: bool,
) -> bool {
    artist_section_shows_action_for_counts(
        artist_section_preview_limit(section),
        loaded_count,
        expanded,
    )
}

pub(crate) fn artist_section_shows_action_for_counts(
    preview_limit: usize,
    loaded_count: usize,
    expanded: bool,
) -> bool {
    expanded || loaded_count > preview_limit
}

pub(crate) fn artist_section_is_visible(
    section: ArtistSection,
    expanded: Option<ArtistSection>,
) -> bool {
    expanded.is_none_or(|selected| selected == section)
}

pub(crate) fn artist_section_should_render(
    section: ArtistSection,
    expanded: Option<ArtistSection>,
    loaded_count: usize,
) -> bool {
    artist_section_is_visible(section, expanded) && artist_section_has_items(loaded_count)
}

pub(crate) fn soundcloud_tracks_only(provider: Provider, artist: &ArtistPage) -> bool {
    provider == Provider::SoundCloud
        && !artist.popular_tracks.is_empty()
        && artist.similar_artists.is_empty()
        && artist.albums.is_empty()
        && artist.featured.is_empty()
        && artist.playlists.is_empty()
}

/// The section an artist page renders as its expanded section list.
///
/// A tracks-only SoundCloud artist page is always the full tracklist: the
/// inline section body is a zero-basis flex item that collapses to nothing
/// inside the page scroll, so the page must take the section-list layout
/// that fills the viewport instead.
pub(crate) fn artist_page_expanded_section(
    provider: Provider,
    artist: &ArtistPage,
    expanded: Option<ArtistSection>,
) -> Option<ArtistSection> {
    if soundcloud_tracks_only(provider, artist) {
        Some(ArtistSection::PopularTracks)
    } else {
        expanded
    }
}

/// Whether a loaded detail page renders content that owns a virtualized
/// scroll surface filling the viewport. Collection pages with tracks and
/// expanded or tracks-only artist pages do; every other page scrolls the
/// plain page surface.
pub(crate) fn detail_results_use_virtualized_scroll(
    page: &DetailPage,
    expanded_artist_section: Option<ArtistSection>,
) -> bool {
    if let Some(artist) = &page.artist {
        return artist_page_expanded_section(page.route.provider, artist, expanded_artist_section)
            .is_some();
    }
    !page.tracks.is_empty()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DetailRoute {
    pub(crate) provider: Provider,
    pub(crate) kind: ResultType,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) artwork: String,
    pub(crate) release_date: String,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer links
    /// are derived from the numeric id instead.
    pub(crate) service_url: String,
}

impl DetailRoute {
    pub(crate) fn from_card(card: &Card) -> Option<Self> {
        if !matches!(
            card.kind,
            ResultType::Artists | ResultType::Albums | ResultType::Playlists
        ) {
            return None;
        }
        let id = validate_id(&card.id).ok()?;
        Some(Self {
            provider: card.source,
            kind: card.kind,
            id,
            title: card.title.clone(),
            subtitle: card.subtitle.clone(),
            artwork: card.artwork.clone(),
            release_date: card.release_date.clone(),
            service_url: card.service_url.clone(),
        })
    }
}

pub(crate) fn detail_metadata(route: &DetailRoute) -> String {
    if route.kind == ResultType::Albums {
        [
            collection_subtitle_for_display(&route.subtitle),
            format_release_date(&route.release_date),
        ]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" • ")
    } else {
        collection_subtitle_for_display(&route.subtitle)
    }
}

pub(crate) fn collection_subtitle_is_visible(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.eq_ignore_ascii_case("soundcloud")
        && !value.eq_ignore_ascii_case("new!")
}

fn collection_subtitle_for_display(value: &str) -> String {
    if collection_subtitle_is_visible(value) {
        value.to_owned()
    } else {
        String::new()
    }
}

pub(crate) fn format_release_date(raw_date: &str) -> String {
    let value = raw_date.trim();
    let Some(date) = parse_release_date(value) else {
        return value.to_owned();
    };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{} {}, {}",
        MONTHS[date.month0() as usize],
        date.day(),
        date.year()
    )
}

pub(crate) fn parse_release_date(raw_date: &str) -> Option<NaiveDate> {
    let value = raw_date.trim();
    let date_text = value.get(..10)?;
    NaiveDate::parse_from_str(date_text, "%Y-%m-%d").ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DetailState {
    Closed,
    Loading,
    Results(Box<DetailPage>),
    Empty(Box<DetailPage>),
    AccountRequired,
    Failed(String),
}

pub(crate) struct DetailNavigation {
    pub(crate) route: Option<DetailRoute>,
    pub(crate) state: DetailState,
    generation: u64,
    view_id: u64,
    pub(crate) expanded_artist_section: Option<ArtistSection>,
    focused_artist_section: Option<ArtistSection>,
    stack: Vec<DetailEntry>,
    search_scroll: Point<Pixels>,
}

pub(crate) fn should_reset_detail_scroll(
    pending_generation: Option<u64>,
    completed_generation: u64,
    completion_accepted: bool,
) -> bool {
    completion_accepted && pending_generation == Some(completed_generation)
}

pub(crate) fn should_apply_scheduled_detail_scroll_reset(
    pending_generation: Option<u64>,
    scheduled_generation: u64,
) -> bool {
    pending_generation == Some(scheduled_generation)
}

struct DetailEntry {
    route: DetailRoute,
    state: DetailState,
    expanded_artist_section: Option<ArtistSection>,
    focused_artist_section: Option<ArtistSection>,
    view_id: u64,
    scroll: Point<Pixels>,
}

impl Default for DetailNavigation {
    fn default() -> Self {
        Self {
            route: None,
            state: DetailState::Closed,
            generation: 0,
            view_id: 0,
            expanded_artist_section: None,
            focused_artist_section: None,
            stack: Vec::new(),
            search_scroll: point(px(0.), px(0.)),
        }
    }
}

impl DetailNavigation {
    #[cfg(test)]
    pub(crate) fn open(&mut self, card: &Card, has_account: bool) -> Option<(u64, DetailRoute)> {
        self.open_with_scroll(card, has_account, point(px(0.), px(0.)))
    }

    pub(crate) fn open_with_scroll(
        &mut self,
        card: &Card,
        has_account: bool,
        scroll: Point<Pixels>,
    ) -> Option<(u64, DetailRoute)> {
        let route = DetailRoute::from_card(card)?;
        self.open_route_with_scroll(route, has_account, scroll)
    }

    pub(crate) fn open_route_with_scroll(
        &mut self,
        route: DetailRoute,
        has_account: bool,
        scroll: Point<Pixels>,
    ) -> Option<(u64, DetailRoute)> {
        if let Some(current_route) = self.route.take() {
            self.stack.push(DetailEntry {
                route: current_route,
                state: std::mem::replace(&mut self.state, DetailState::Closed),
                expanded_artist_section: self.expanded_artist_section.take(),
                focused_artist_section: self.focused_artist_section.take(),
                view_id: self.view_id,
                scroll,
            });
        } else {
            self.search_scroll = scroll;
        }
        self.generation = self.generation.wrapping_add(1);
        self.view_id = self.view_id.wrapping_add(1);
        self.route = Some(route.clone());
        self.expanded_artist_section = None;
        self.focused_artist_section = None;
        self.state = if has_account {
            DetailState::Loading
        } else {
            DetailState::AccountRequired
        };
        has_account.then_some((self.generation, route))
    }

    /// Replaces an existing detail stack for a navigation request originating
    /// outside Search.  The caller's search results remain owned by
    /// `SearchView`; only detail history and its scroll position are reset.
    pub(crate) fn replace(&mut self, card: &Card, has_account: bool) -> Option<(u64, DetailRoute)> {
        self.reset();
        self.open_with_scroll(card, has_account, point(px(0.), px(0.)))
    }

    pub(crate) fn open_focused_artist_section(
        &mut self,
        card: &Card,
        has_account: bool,
        section: ArtistSection,
        scroll: Point<Pixels>,
    ) -> Option<(u64, DetailRoute)> {
        let opened = self.open_with_scroll(card, has_account, scroll);
        if opened.is_some() {
            self.expanded_artist_section = Some(section);
            self.focused_artist_section = Some(section);
        }
        opened
    }

    #[cfg(test)]
    pub(crate) fn back(&mut self) {
        self.back_with_scroll(point(px(0.), px(0.)));
    }

    pub(crate) fn back_with_scroll(&mut self, _scroll: Point<Pixels>) -> Point<Pixels> {
        if self.focused_artist_section.take().is_none()
            && self.expanded_artist_section.take().is_some()
        {
            return point(px(0.), px(0.));
        }
        self.generation = self.generation.wrapping_add(1);
        if let Some(previous) = self.stack.pop() {
            self.route = Some(previous.route);
            self.state = previous.state;
            self.expanded_artist_section = previous.expanded_artist_section;
            self.focused_artist_section = previous.focused_artist_section;
            self.view_id = previous.view_id;
            previous.scroll
        } else {
            self.view_id = self.view_id.wrapping_add(1);
            self.route = None;
            self.state = DetailState::Closed;
            self.search_scroll
        }
    }

    pub(crate) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.view_id = self.view_id.wrapping_add(1);
        self.route = None;
        self.state = DetailState::Closed;
        self.expanded_artist_section = None;
        self.focused_artist_section = None;
        self.stack.clear();
        self.search_scroll = point(px(0.), px(0.));
    }

    pub(crate) fn close_all(&mut self) {
        self.reset();
    }

    pub(crate) fn complete(
        &mut self,
        generation: u64,
        result: Result<DetailPage, ProviderError>,
    ) -> bool {
        if generation != self.generation || self.route.is_none() {
            return false;
        }
        self.state = match result {
            Ok(page) => page.state(),
            Err(error) => DetailState::Failed(error.message),
        };
        true
    }

    pub(crate) fn toggle_artist_section(&mut self, section: ArtistSection) {
        let next = (self.expanded_artist_section != Some(section)).then_some(section);
        if self.expanded_artist_section != next {
            self.expanded_artist_section = next;
            if self.focused_artist_section == Some(section) && next.is_none() {
                self.focused_artist_section = None;
            }
        }
    }

    pub(crate) fn view_id(&self) -> u64 {
        self.view_id
    }

    pub(crate) fn reload(&mut self) -> Option<(u64, DetailRoute)> {
        let route = self.route.clone()?;
        self.generation = self.generation.wrapping_add(1);
        self.state = DetailState::Loading;
        Some((self.generation, route))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DetailPage {
    pub(crate) route: DetailRoute,
    pub(crate) tracks: Vec<Track>,
    pub(crate) total: Option<usize>,
    pub(crate) raw_loaded_count: usize,
    pub(crate) normalized_count: usize,
    pub(crate) authoritative_total: Option<usize>,
    pub(crate) artist: Option<ArtistPage>,
    pub(crate) description: String,
    pub(crate) album_info: Option<super::album_info::AlbumInfo>,
}

impl DetailPage {
    pub(crate) fn state(self) -> DetailState {
        if self.tracks.is_empty() && self.artist.as_ref().is_none_or(artist_is_empty) {
            DetailState::Empty(Box::new(self))
        } else {
            DetailState::Results(Box::new(self))
        }
    }
}

fn artist_is_empty(artist: &ArtistPage) -> bool {
    artist.popular_tracks.is_empty()
        && artist.similar_artists.is_empty()
        && artist.albums.is_empty()
        && artist.featured.is_empty()
        && artist.playlists.is_empty()
}

pub(crate) fn validate_id(id: &str) -> Result<String, ProviderError> {
    let id = id.trim();
    if id.is_empty() || id.len() > 32 || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ProviderError::new("A valid collection ID is required"));
    }
    Ok(id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_only_open_valid_detail_cards() {
        let mut card = Card {
            kind: ResultType::Artists,
            id: "1".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        assert!(DetailRoute::from_card(&card).is_some());
        card.kind = ResultType::Albums;
        assert!(DetailRoute::from_card(&card).is_some());
        card.id.clear();
        assert!(DetailRoute::from_card(&card).is_none());
        card.id = "not-numeric".into();
        assert!(DetailRoute::from_card(&card).is_none());
    }

    #[test]
    fn detail_routes_carry_the_card_service_url() {
        // The detail page's more menu copies the same link the search card
        // menu copies, so the route must keep the card's provider URL.
        let card = Card {
            kind: ResultType::Albums,
            id: "42".into(),
            source: Provider::SoundCloud,
            service_url: "https://soundcloud.com/artist/sets/album".into(),
            ..Card::default()
        };
        let route = DetailRoute::from_card(&card).expect("valid album card");
        assert_eq!(
            route.service_url,
            "https://soundcloud.com/artist/sets/album"
        );
    }

    #[test]
    fn external_replace_discards_stale_detail_stack_and_starts_at_top() {
        let first = Card {
            kind: ResultType::Artists,
            id: "1".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        let second = Card {
            kind: ResultType::Albums,
            id: "2".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        let mut navigation = DetailNavigation::default();
        navigation.open_with_scroll(&first, true, point(px(0.), px(80.)));
        navigation.open_with_scroll(&second, true, point(px(0.), px(120.)));
        assert!(!navigation.stack.is_empty());

        let external = Card {
            kind: ResultType::Artists,
            id: "3".into(),
            title: "External Artist".into(),
            subtitle: "12 tracks".into(),
            artwork: "https://example.test/artist.jpg".into(),
            source: Provider::SoundCloud,
            ..Card::default()
        };
        navigation.replace(&external, true);

        assert!(navigation.stack.is_empty());
        assert_eq!(
            navigation.route.as_ref().map(|route| route.id.as_str()),
            Some("3")
        );
        let route = navigation.route.as_ref().expect("external route");
        assert_eq!(route.title, "External Artist");
        assert_eq!(route.subtitle, "12 tracks");
        assert_eq!(route.artwork, "https://example.test/artist.jpg");
        assert_eq!(navigation.search_scroll, point(px(0.), px(0.)));
    }

    #[test]
    fn validates_numeric_ids_with_length_limit() {
        assert_eq!(validate_id(" 123 ").unwrap(), "123");
        assert!(validate_id("123/a").is_err());
        assert!(validate_id(&"1".repeat(33)).is_err());
    }

    #[test]
    fn formats_iso_release_dates_without_changing_other_values() {
        assert_eq!(format_release_date("2011-06-07"), "Jun 7, 2011");
        assert_eq!(format_release_date(" 2011 "), "2011");
        assert_eq!(format_release_date("Spring 2011"), "Spring 2011");
        assert_eq!(format_release_date("2011-02-29"), "2011-02-29");
    }

    #[test]
    fn collection_subtitle_placeholders_are_hidden_everywhere() {
        for placeholder in ["SoundCloud", " soundcloud ", "New!", " new! ", ""] {
            assert!(!collection_subtitle_is_visible(placeholder));
        }
        assert!(collection_subtitle_is_visible("Featuring Skrillex"));

        let mut route = DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Playlists,
            id: "1".into(),
            title: "Selection".into(),
            subtitle: " SoundCloud ".into(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        assert_eq!(detail_metadata(&route), "");
        route.subtitle = "New!".into();
        assert_eq!(detail_metadata(&route), "");
        route.subtitle = "Curated for you".into();
        assert_eq!(detail_metadata(&route), "Curated for you");
    }

    #[test]
    fn back_rejects_stale_artist_completion_and_preserves_search() {
        let card = Card {
            kind: ResultType::Artists,
            id: "7".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        let mut search = crate::search::models::SearchState::default();
        search.source = crate::search::models::Source::SoundCloud;
        search.result_type = ResultType::Artists;
        search.groups.artists.push(card.clone());
        let before = search.clone();
        let mut navigation = DetailNavigation::default();
        let (generation, route) = navigation.open(&card, true).unwrap();
        navigation.back();
        assert!(!navigation.complete(
            generation,
            Ok(DetailPage {
                route,
                tracks: Vec::new(),
                total: Some(0),
                raw_loaded_count: 0,
                normalized_count: 0,
                authoritative_total: Some(0),
                artist: Some(ArtistPage::default()),
                description: String::new(),
                album_info: None,
            })
        ));
        assert_eq!(search.source, before.source);
        assert_eq!(search.result_type, before.result_type);
        assert_eq!(search.groups, before.groups);
        assert_eq!(navigation.state, DetailState::Closed);
    }

    #[test]
    fn empty_artist_and_track_pages_select_the_empty_state() {
        let route = DetailRoute {
            provider: Provider::SoundCloud,
            kind: ResultType::Artists,
            id: "1".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        let empty_tracks = DetailPage {
            route: route.clone(),
            tracks: Vec::new(),
            total: Some(0),
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: Some(0),
            artist: None,
            description: String::new(),
            album_info: None,
        };
        assert!(matches!(
            empty_tracks.state(),
            DetailState::Empty(page) if page.tracks.is_empty()
        ));
        let empty_artist = DetailPage {
            route,
            tracks: Vec::new(),
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(ArtistPage::default()),
            description: String::new(),
            album_info: None,
        };
        assert!(matches!(
            empty_artist.state(),
            DetailState::Empty(page) if page.artist.is_some()
        ));
    }

    #[test]
    fn empty_playlist_state_retains_album_info_for_the_info_action() {
        let info = crate::search::AlbumInfo {
            description: "An empty playlist with metadata".into(),
            ..crate::search::AlbumInfo::default()
        };
        let page = DetailPage {
            route: DetailRoute {
                provider: Provider::Deezer,
                kind: ResultType::Playlists,
                id: "42".into(),
                title: "Empty playlist".into(),
                subtitle: "Owner".into(),
                artwork: String::new(),
                release_date: String::new(),
                service_url: String::new(),
            },
            tracks: Vec::new(),
            total: Some(0),
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: Some(0),
            artist: None,
            description: String::new(),
            album_info: Some(info.clone()),
        };

        assert!(matches!(
            page.state(),
            DetailState::Empty(retained) if retained.album_info == Some(info)
        ));
    }

    #[test]
    fn populated_artist_arrays_prevent_empty_state_even_when_totals_are_zero() {
        let route = DetailRoute {
            provider: Provider::Deezer,
            kind: ResultType::Artists,
            id: "1".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        let page = DetailPage {
            route,
            tracks: Vec::new(),
            total: Some(0),
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(ArtistPage {
                albums: vec![Card::default()],
                ..ArtistPage::default()
            }),
            description: String::new(),
            album_info: None,
        };
        assert!(matches!(page.state(), DetailState::Results(_)));
    }

    #[test]
    fn reported_artist_totals_do_not_prevent_empty_state_without_loaded_items() {
        let page = DetailPage {
            route: DetailRoute {
                provider: Provider::Deezer,
                kind: ResultType::Artists,
                id: "1".into(),
                title: String::new(),
                subtitle: String::new(),
                artwork: String::new(),
                release_date: String::new(),
                service_url: String::new(),
            },
            tracks: Vec::new(),
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(ArtistPage {
                popular_total: 10,
                albums_total: 20,
                featured_total: 30,
                playlists_total: 40,
                ..ArtistPage::default()
            }),
            description: String::new(),
            album_info: None,
        };
        assert!(matches!(page.state(), DetailState::Empty(retained) if retained.artist.is_some()));
    }

    #[test]
    fn soundcloud_tracks_only_requires_soundcloud_tracks_and_empty_other_sections() {
        let tracks_only = ArtistPage {
            popular_tracks: vec![Track::default()],
            popular_total: 1,
            ..ArtistPage::default()
        };
        assert!(soundcloud_tracks_only(Provider::SoundCloud, &tracks_only));
        assert!(!soundcloud_tracks_only(Provider::Deezer, &tracks_only));
        assert!(!soundcloud_tracks_only(
            Provider::SoundCloud,
            &ArtistPage::default()
        ));

        let with_albums = ArtistPage {
            popular_tracks: vec![Track::default()],
            albums: vec![Card::default()],
            ..ArtistPage::default()
        };
        assert!(!soundcloud_tracks_only(Provider::SoundCloud, &with_albums));

        let with_playlists = ArtistPage {
            popular_tracks: vec![Track::default()],
            playlists: vec![Card::default()],
            ..ArtistPage::default()
        };
        assert!(!soundcloud_tracks_only(
            Provider::SoundCloud,
            &with_playlists
        ));

        let with_similar = ArtistPage {
            popular_tracks: vec![Track::default()],
            similar_artists: vec![Card::default()],
            ..ArtistPage::default()
        };
        assert!(!soundcloud_tracks_only(Provider::SoundCloud, &with_similar));

        let with_featured = ArtistPage {
            popular_tracks: vec![Track::default()],
            featured: vec![Card::default()],
            ..ArtistPage::default()
        };
        assert!(!soundcloud_tracks_only(
            Provider::SoundCloud,
            &with_featured
        ));
    }

    #[test]
    fn tracks_only_artist_pages_always_render_the_section_list() {
        let tracks_only = ArtistPage {
            popular_tracks: vec![Track::default()],
            ..ArtistPage::default()
        };
        // The full tracklist must render through the section-list layout
        // even before the user expands anything, and any remembered
        // expansion must not redirect the page.
        assert_eq!(
            artist_page_expanded_section(Provider::SoundCloud, &tracks_only, None),
            Some(ArtistSection::PopularTracks)
        );
        assert_eq!(
            artist_page_expanded_section(
                Provider::SoundCloud,
                &tracks_only,
                Some(ArtistSection::Albums)
            ),
            Some(ArtistSection::PopularTracks)
        );

        let with_albums = ArtistPage {
            popular_tracks: vec![Track::default()],
            albums: vec![Card::default()],
            ..ArtistPage::default()
        };
        assert_eq!(
            artist_page_expanded_section(Provider::SoundCloud, &with_albums, None),
            None
        );
        assert_eq!(
            artist_page_expanded_section(
                Provider::SoundCloud,
                &with_albums,
                Some(ArtistSection::Albums)
            ),
            Some(ArtistSection::Albums)
        );

        // Deezer artist pages keep the plain preview layout.
        assert_eq!(
            artist_page_expanded_section(Provider::Deezer, &tracks_only, None),
            None
        );
    }

    #[test]
    fn detail_results_own_the_viewport_only_for_virtualized_content() {
        let route = |provider| DetailRoute {
            provider,
            kind: ResultType::Artists,
            id: "7".into(),
            title: String::new(),
            subtitle: String::new(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        };
        let artist_page = |artist| DetailPage {
            route: route(Provider::SoundCloud),
            tracks: Vec::new(),
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(artist),
            description: String::new(),
            album_info: None,
        };

        // A tracks-only SoundCloud artist page fills the viewport: the
        // inline section body would collapse inside the page scroll and
        // hide the tracklist.
        let tracks_only = artist_page(ArtistPage {
            popular_tracks: vec![Track::default()],
            ..ArtistPage::default()
        });
        assert!(detail_results_use_virtualized_scroll(&tracks_only, None));

        // Artist pages with other sections stay on the page scroll until a
        // section is expanded.
        let with_albums = artist_page(ArtistPage {
            popular_tracks: vec![Track::default()],
            albums: vec![Card::default()],
            ..ArtistPage::default()
        });
        assert!(!detail_results_use_virtualized_scroll(&with_albums, None));
        assert!(detail_results_use_virtualized_scroll(
            &with_albums,
            Some(ArtistSection::PopularTracks)
        ));

        // Collection pages own the viewport while they have tracks.
        let mut collection = tracks_only;
        collection.route = route(Provider::SoundCloud);
        collection.artist = None;
        collection.tracks = vec![Track::default()];
        assert!(detail_results_use_virtualized_scroll(&collection, None));
        collection.tracks.clear();
        assert!(!detail_results_use_virtualized_scroll(&collection, None));
    }

    #[test]
    fn artist_sections_use_loaded_items_for_visibility_and_actions() {
        assert!(!artist_section_has_items(0));
        assert!(!artist_section_shows_action(
            ArtistSection::Albums,
            12,
            false
        ));
        assert!(artist_section_shows_action(
            ArtistSection::Albums,
            13,
            false
        ));
        assert!(artist_section_shows_action(ArtistSection::Albums, 1, true));
        assert!(!artist_section_shows_action(
            ArtistSection::PopularTracks,
            5,
            false
        ));
        assert!(artist_section_shows_action(
            ArtistSection::PopularTracks,
            6,
            false
        ));
    }

    #[test]
    fn artist_section_rendering_skips_empty_middle_sections() {
        let expanded = None;
        assert!(artist_section_should_render(
            ArtistSection::PopularTracks,
            expanded,
            1
        ));
        assert!(!artist_section_should_render(
            ArtistSection::SimilarArtists,
            expanded,
            0
        ));
        assert!(!artist_section_should_render(
            ArtistSection::Albums,
            expanded,
            0
        ));
        assert!(!artist_section_should_render(
            ArtistSection::Featured,
            expanded,
            0
        ));
        assert!(artist_section_should_render(
            ArtistSection::Playlists,
            expanded,
            1
        ));
    }

    #[test]
    fn artist_section_expansion_is_focused_and_toggleable() {
        let mut navigation = DetailNavigation::default();
        let view_id = navigation.view_id();
        navigation.toggle_artist_section(ArtistSection::Albums);
        assert_eq!(
            navigation.expanded_artist_section,
            Some(ArtistSection::Albums)
        );
        assert_eq!(navigation.view_id(), view_id);
        navigation.toggle_artist_section(ArtistSection::Featured);
        assert_eq!(
            navigation.expanded_artist_section,
            Some(ArtistSection::Featured)
        );
        assert_eq!(navigation.view_id(), view_id);
        navigation.toggle_artist_section(ArtistSection::Featured);
        assert_eq!(navigation.expanded_artist_section, None);
        assert_eq!(navigation.view_id(), view_id);
    }

    #[test]
    fn expanded_artist_sections_hide_unselected_sections() {
        let sections = [
            ArtistSection::PopularTracks,
            ArtistSection::Albums,
            ArtistSection::Featured,
            ArtistSection::Playlists,
        ];
        for section in sections {
            assert!(artist_section_is_visible(section, None));
            assert!(artist_section_is_visible(section, Some(section)));
            assert!(sections.iter().all(|other| {
                artist_section_is_visible(*other, Some(section)) == (*other == section)
            }));
        }
    }

    #[test]
    fn nested_detail_gets_a_fresh_view_id_and_back_restores_scroll() {
        let artist = card(ResultType::Artists, "7");
        let album = card(ResultType::Albums, "8");
        let mut navigation = DetailNavigation::default();
        navigation.open(&artist, true).unwrap();
        let artist_view_id = navigation.view_id();

        navigation
            .open_with_scroll(&album, true, point(px(0.), px(-42.)))
            .unwrap();
        assert_ne!(navigation.view_id(), artist_view_id);

        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            point(px(0.), px(-42.))
        );
        assert_eq!(navigation.view_id(), artist_view_id);
    }

    #[test]
    fn detail_scroll_reset_requires_the_accepted_pending_generation() {
        assert!(should_reset_detail_scroll(Some(7), 7, true));
        assert!(!should_reset_detail_scroll(Some(6), 7, true));
        assert!(!should_reset_detail_scroll(Some(7), 7, false));
        assert!(!should_reset_detail_scroll(None, 7, true));
    }

    #[test]
    fn scheduled_forward_reset_cannot_rewind_a_newer_route() {
        assert!(should_apply_scheduled_detail_scroll_reset(Some(7), 7));
        assert!(!should_apply_scheduled_detail_scroll_reset(Some(8), 7));
        assert!(!should_apply_scheduled_detail_scroll_reset(None, 7));
    }

    #[test]
    fn forward_artist_navigation_starts_at_top_and_back_restores_search_offset() {
        let artist = card(ResultType::Artists, "7");
        let search_offset = point(px(0.), px(-128.));
        let mut navigation = DetailNavigation::default();
        let (generation, route) = navigation
            .open_with_scroll(&artist, true, search_offset)
            .unwrap();
        assert!(navigation.complete(generation, Ok(artist_page(route))));
        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            search_offset
        );
        assert_eq!(navigation.state, DetailState::Closed);
    }

    #[test]
    fn account_required_forward_route_starts_at_top_and_back_restores_search_offset() {
        let artist = card(ResultType::Artists, "7");
        let search_offset = point(px(0.), px(-128.));
        assert!(DetailRoute::from_card(&artist).is_some());
        let mut navigation = DetailNavigation::default();
        assert!(
            navigation
                .open_with_scroll(&artist, false, search_offset)
                .is_none()
        );
        assert!(matches!(navigation.state, DetailState::AccountRequired));
        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            search_offset
        );
        assert_eq!(navigation.state, DetailState::Closed);
    }

    #[test]
    fn nested_collection_back_restores_completed_artist() {
        let artist = card(ResultType::Artists, "7");
        let album = card(ResultType::Albums, "8");
        let mut navigation = DetailNavigation::default();
        let (artist_generation, artist_route) = navigation.open(&artist, true).unwrap();
        assert!(navigation.complete(artist_generation, Ok(artist_page(artist_route.clone()))));

        let (album_generation, album_route) = navigation.open(&album, true).unwrap();
        assert!(navigation.complete(
            album_generation,
            Ok(DetailPage {
                route: album_route,
                tracks: vec![Track::default()],
                total: Some(1),
                raw_loaded_count: 1,
                normalized_count: 1,
                authoritative_total: Some(1),
                artist: None,
                description: String::new(),
                album_info: None,
            })
        ));

        navigation.back();
        assert_eq!(navigation.route.as_ref(), Some(&artist_route));
        assert!(
            matches!(navigation.state, DetailState::Results(ref page) if page.artist.is_some())
        );
    }

    #[test]
    fn back_collapses_artist_section_before_leaving_detail() {
        let artist = card(ResultType::Artists, "7");
        let mut navigation = DetailNavigation::default();
        navigation.open(&artist, true).unwrap();
        let artist_view_id = navigation.view_id();
        navigation.toggle_artist_section(ArtistSection::Albums);

        navigation.back();
        assert_eq!(
            navigation.route.as_ref().map(|route| &route.id),
            Some(&artist.id)
        );
        assert_eq!(navigation.expanded_artist_section, None);
        assert_eq!(navigation.view_id(), artist_view_id);

        navigation.back();
        assert_eq!(navigation.state, DetailState::Closed);
    }

    #[test]
    fn focused_similar_artist_route_back_leaves_detail_in_one_step() {
        let artist = card(ResultType::Artists, "7");
        let mut navigation = DetailNavigation::default();
        let (generation, route) = navigation
            .open_focused_artist_section(
                &artist,
                true,
                ArtistSection::SimilarArtists,
                point(px(0.), px(-21.)),
            )
            .unwrap();
        assert!(navigation.complete(
            generation,
            Ok(DetailPage {
                route,
                tracks: Vec::new(),
                total: None,
                raw_loaded_count: 0,
                normalized_count: 0,
                authoritative_total: None,
                artist: Some(ArtistPage {
                    similar_artists: vec![card(ResultType::Artists, "8")],
                    similar_total: 1,
                    ..ArtistPage::default()
                }),
                description: String::new(),
                album_info: None,
            })
        ));
        assert_eq!(
            navigation.back_with_scroll(point(px(0.), px(0.))),
            point(px(0.), px(-21.))
        );
        assert_eq!(navigation.state, DetailState::Closed);
    }

    #[test]
    fn stale_child_completion_cannot_replace_restored_artist() {
        let artist = card(ResultType::Artists, "7");
        let playlist = card(ResultType::Playlists, "9");
        let mut navigation = DetailNavigation::default();
        let (artist_generation, artist_route) = navigation.open(&artist, true).unwrap();
        navigation.complete(artist_generation, Ok(artist_page(artist_route.clone())));
        let (playlist_generation, playlist_route) = navigation.open(&playlist, true).unwrap();

        navigation.back();
        assert!(!navigation.complete(
            playlist_generation,
            Ok(DetailPage {
                route: playlist_route,
                tracks: vec![Track::default()],
                total: Some(1),
                raw_loaded_count: 1,
                normalized_count: 1,
                authoritative_total: Some(1),
                artist: None,
                description: String::new(),
                album_info: None,
            })
        ));
        assert_eq!(navigation.route.as_ref(), Some(&artist_route));
        assert!(
            matches!(navigation.state, DetailState::Results(ref page) if page.artist.is_some())
        );
    }

    #[test]
    fn reset_clears_the_whole_detail_stack_and_rejects_pending_work() {
        let artist = card(ResultType::Artists, "7");
        let album = card(ResultType::Albums, "8");
        let mut navigation = DetailNavigation::default();
        navigation.open(&artist, true).unwrap();
        let (generation, route) = navigation.open(&album, true).unwrap();

        navigation.reset();
        navigation.back();
        assert_eq!(navigation.state, DetailState::Closed);
        assert!(!navigation.complete(
            generation,
            Ok(DetailPage {
                route,
                tracks: vec![Track::default()],
                total: Some(1),
                raw_loaded_count: 1,
                normalized_count: 1,
                authoritative_total: Some(1),
                artist: None,
                description: String::new(),
                album_info: None,
            })
        ));
    }

    #[test]
    fn deleting_search_playlist_closes_detail_and_preserves_results() {
        let playlist = card(ResultType::Playlists, "42");
        let mut search = crate::search::models::SearchState::default();
        search.groups.playlists.push(playlist.clone());
        let before = search.clone();
        let mut navigation = DetailNavigation::default();
        navigation.open(&playlist, true);
        navigation.back();
        assert_eq!(navigation.state, DetailState::Closed);
        assert_eq!(search.groups, before.groups);
    }

    #[test]
    fn deleting_nested_search_playlist_closes_the_entire_detail_stack() {
        let artist = card(ResultType::Artists, "7");
        let album = card(ResultType::Albums, "8");
        let playlist = card(ResultType::Playlists, "42");
        let mut search = crate::search::models::SearchState::default();
        search.groups.artists.push(artist.clone());
        search.groups.albums.push(album.clone());
        search.groups.playlists.push(playlist.clone());
        search.source = crate::search::models::Source::Deezer;
        search.result_type = ResultType::Playlists;
        let before = search.clone();
        let mut navigation = DetailNavigation::default();
        navigation.open(&artist, true);
        navigation.open(&album, true);
        let (generation, route) = navigation.open(&playlist, true).unwrap();

        navigation.close_all();

        assert_eq!(navigation.state, DetailState::Closed);
        assert!(navigation.route.is_none());
        assert!(!navigation.complete(
            generation,
            Ok(DetailPage {
                route,
                tracks: vec![Track::default()],
                total: Some(1),
                raw_loaded_count: 1,
                normalized_count: 1,
                authoritative_total: Some(1),
                artist: None,
                description: String::new(),
                album_info: None,
            })
        ));
        assert_eq!(search.source, before.source);
        assert_eq!(search.result_type, before.result_type);
        assert_eq!(search.groups, before.groups);
    }

    fn card(kind: ResultType, id: &str) -> Card {
        Card {
            kind,
            id: id.into(),
            source: Provider::Deezer,
            ..Card::default()
        }
    }

    fn artist_page(route: DetailRoute) -> DetailPage {
        DetailPage {
            route,
            tracks: Vec::new(),
            total: None,
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: None,
            artist: Some(ArtistPage {
                popular_tracks: vec![Track::default()],
                popular_total: 1,
                ..ArtistPage::default()
            }),
            description: String::new(),
            album_info: None,
        }
    }
}
