use std::collections::{HashMap, HashSet};

use super::{
    album_info::{AlbumInfo, album_info_for_page},
    detail::{DetailPage, DetailRoute, DetailState},
    models::{Card, Provider, ResultType},
};

const MAX_CACHED_ALBUM_INFO_PAGES: usize = 64;

/// Cache key for album and playlist "about" info. Only album and playlist
/// routes produce a key, so artist and track cards never enter the cache.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct AlbumInfoCacheKey {
    provider: Provider,
    kind: ResultType,
    id: String,
}

impl AlbumInfoCacheKey {
    pub(crate) fn from_route(route: &DetailRoute) -> Option<Self> {
        if !matches!(route.kind, ResultType::Albums | ResultType::Playlists) {
            return None;
        }
        if route.id.trim().is_empty() {
            return None;
        }
        Some(Self {
            provider: route.provider,
            kind: route.kind,
            id: route.id.clone(),
        })
    }

    pub(crate) fn from_card(card: &Card) -> Option<Self> {
        DetailRoute::from_card(card)
            .as_ref()
            .and_then(Self::from_route)
    }

    #[cfg(test)]
    pub(crate) fn test_key(provider: Provider, kind: ResultType, id: &str) -> Self {
        Self {
            provider,
            kind,
            id: id.to_owned(),
        }
    }
}

/// Small bounded cache of fetched detail pages used to populate the Info
/// dialog synchronously. The full page is kept because the dialog needs both
/// the authoritative route (SoundCloud titles can change after fetch) and the
/// parsed album info. Only pages with visible info content are stored.
#[derive(Default)]
pub(crate) struct AlbumInfoPrefetch {
    pages: HashMap<AlbumInfoCacheKey, DetailPage>,
    in_flight: HashSet<AlbumInfoCacheKey>,
    generation: u64,
}

impl AlbumInfoPrefetch {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn cached(&self, route: &DetailRoute) -> Option<(DetailRoute, AlbumInfo)> {
        let key = AlbumInfoCacheKey::from_route(route)?;
        let page = self.pages.get(&key)?;
        let info = album_info_for_page(page)?;
        Some((page.route.clone(), info))
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, key: &AlbumInfoCacheKey) -> bool {
        self.pages.contains_key(key)
    }

    #[cfg(test)]
    pub(crate) fn is_in_flight(&self, key: &AlbumInfoCacheKey) -> bool {
        self.in_flight.contains(key)
    }

    /// Marks a key as loading. Returns false when the key is already cached
    /// or another fetch is running, so callers avoid duplicate requests.
    pub(crate) fn begin(&mut self, key: AlbumInfoCacheKey) -> bool {
        if self.pages.contains_key(&key) || !self.in_flight.insert(key) {
            return false;
        }
        true
    }

    pub(crate) fn complete(&mut self, key: AlbumInfoCacheKey, page: DetailPage) {
        self.in_flight.remove(&key);
        self.store_page(page);
    }

    pub(crate) fn complete_for_generation(
        &mut self,
        generation: u64,
        key: AlbumInfoCacheKey,
        page: DetailPage,
    ) {
        if generation == self.generation {
            self.complete(key, page);
        }
    }

    pub(crate) fn fail_for_generation(&mut self, generation: u64, key: &AlbumInfoCacheKey) {
        if generation == self.generation {
            self.fail(key);
        }
    }

    pub(crate) fn fail(&mut self, key: &AlbumInfoCacheKey) {
        self.in_flight.remove(key);
    }

    pub(crate) fn store_page(&mut self, page: DetailPage) {
        let Some(key) = AlbumInfoCacheKey::from_route(&page.route) else {
            return;
        };
        if album_info_for_page(&page).is_none() {
            return;
        }
        if self.pages.len() >= MAX_CACHED_ALBUM_INFO_PAGES
            && !self.pages.contains_key(&key)
            && let Some(oldest) = self.pages.keys().next().cloned()
        {
            self.pages.remove(&oldest);
        }
        self.pages.insert(key, page);
    }

    pub(crate) fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pages.clear();
        self.in_flight.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.pages.len()
    }
}

/// Hosts that can store a fetched detail page for later Info dialog use.
/// Implemented by SearchView and LibraryView so the shared dialog fetch can
/// populate either view cache without knowing the concrete view type.
pub(crate) trait AlbumInfoCacheHost {
    fn album_info_generation(&self) -> u64;
    fn store_album_info_page(&mut self, page: &DetailPage);
}

/// Reuses an already loaded detail page for the same collection route, so the
/// Info dialog does not refetch when the user is already viewing the album or
/// playlist. Returns the authoritative route plus info when available.
pub(crate) fn info_for_detail_state(
    state: &DetailState,
    route: &DetailRoute,
) -> Option<(DetailRoute, AlbumInfo)> {
    let page = match state {
        DetailState::Results(page) | DetailState::Empty(page) => page.as_ref(),
        _ => return None,
    };
    if page.route.provider != route.provider
        || page.route.kind != route.kind
        || page.route.id != route.id
    {
        return None;
    }
    let info = album_info_for_page(page)?;
    Some((page.route.clone(), info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_reset_invalidates_prefetches_even_when_the_account_returns() {
        let mut cache = AlbumInfoPrefetch::default();
        let key = AlbumInfoCacheKey::test_key(Provider::Deezer, ResultType::Albums, "42");
        cache.begin(key.clone());
        let before = cache.generation();
        cache.clear();
        cache.clear();
        assert!(cache.begin(key.clone()));
        cache.fail_for_generation(before, &key);
        assert!(cache.is_in_flight(&key));
        cache.complete_for_generation(
            before,
            key.clone(),
            page_with_info(ResultType::Albums, "42", "old account"),
        );
        assert!(!cache.contains(&key));
        assert!(cache.is_in_flight(&key));
        cache.complete_for_generation(
            cache.generation(),
            key.clone(),
            page_with_info(ResultType::Albums, "42", "current account"),
        );
        assert!(cache.contains(&key));
    }

    fn route(kind: ResultType, id: &str) -> DetailRoute {
        DetailRoute {
            provider: Provider::Deezer,
            kind,
            id: id.into(),
            title: "Title".into(),
            subtitle: "Artist".into(),
            artwork: String::new(),
            release_date: String::new(),
            service_url: String::new(),
        }
    }

    fn page_with_info(kind: ResultType, id: &str, label: &str) -> DetailPage {
        DetailPage {
            route: route(kind, id),
            tracks: Vec::new(),
            total: Some(0),
            raw_loaded_count: 0,
            normalized_count: 0,
            authoritative_total: Some(0),
            artist: None,
            description: String::new(),
            album_info: Some(AlbumInfo {
                label: label.into(),
                ..AlbumInfo::default()
            }),
        }
    }

    #[test]
    fn cache_keys_only_cover_album_and_playlist_routes() {
        let album = route(ResultType::Albums, "42");
        let playlist = route(ResultType::Playlists, "7");
        assert!(AlbumInfoCacheKey::from_route(&album).is_some());
        assert!(AlbumInfoCacheKey::from_route(&playlist).is_some());
        assert!(AlbumInfoCacheKey::from_route(&route(ResultType::Artists, "42")).is_none());
        assert!(AlbumInfoCacheKey::from_route(&route(ResultType::Tracks, "42")).is_none());
        assert!(AlbumInfoCacheKey::from_route(&route(ResultType::Albums, "  ")).is_none());

        let card = Card {
            kind: ResultType::Albums,
            id: "42".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        assert!(AlbumInfoCacheKey::from_card(&card).is_some());
        let artist_card = Card {
            kind: ResultType::Artists,
            id: "42".into(),
            source: Provider::Deezer,
            ..Card::default()
        };
        assert!(AlbumInfoCacheKey::from_card(&artist_card).is_none());
    }

    #[test]
    fn prefetch_returns_the_authoritative_route_and_info() {
        let mut prefetch = AlbumInfoPrefetch::default();
        let page = page_with_info(ResultType::Albums, "42", "Some Label");
        prefetch.store_page(page.clone());
        let stored = route(ResultType::Albums, "42");
        let (cached_route, info) = prefetch.cached(&stored).expect("cached info");
        assert_eq!(cached_route, page.route);
        assert_eq!(info.label, "Some Label");
        assert!(prefetch.cached(&route(ResultType::Albums, "43")).is_none());
    }

    #[test]
    fn begin_rejects_duplicates_and_complete_stores_only_useful_pages() {
        let mut prefetch = AlbumInfoPrefetch::default();
        let key = AlbumInfoCacheKey::test_key(Provider::Deezer, ResultType::Albums, "42");
        assert!(prefetch.begin(key.clone()));
        assert!(!prefetch.begin(key.clone()));
        assert!(prefetch.is_in_flight(&key));
        prefetch.complete(
            key.clone(),
            page_with_info(ResultType::Albums, "42", "Label"),
        );
        assert!(!prefetch.is_in_flight(&key));
        assert!(prefetch.contains(&key));
        assert!(!prefetch.begin(key.clone()));

        let mut empty = AlbumInfoPrefetch::default();
        let key = AlbumInfoCacheKey::test_key(Provider::Deezer, ResultType::Albums, "9");
        assert!(empty.begin(key.clone()));
        empty.complete(
            key.clone(),
            DetailPage {
                route: route(ResultType::Albums, "9"),
                tracks: Vec::new(),
                total: Some(0),
                raw_loaded_count: 0,
                normalized_count: 0,
                authoritative_total: Some(0),
                artist: None,
                description: String::new(),
                album_info: None,
            },
        );
        assert!(!empty.contains(&key));

        let mut failed = AlbumInfoPrefetch::default();
        assert!(failed.begin(key.clone()));
        failed.fail(&key);
        assert!(!failed.is_in_flight(&key));
        assert!(failed.begin(key));
    }

    #[test]
    fn cache_stays_bounded() {
        let mut prefetch = AlbumInfoPrefetch::default();
        for index in 0..(MAX_CACHED_ALBUM_INFO_PAGES + 5) {
            prefetch.store_page(page_with_info(
                ResultType::Albums,
                &index.to_string(),
                "Label",
            ));
        }
        assert!(prefetch.len() <= MAX_CACHED_ALBUM_INFO_PAGES);
    }

    #[test]
    fn detail_state_reuse_requires_the_exact_collection_route() {
        let page = page_with_info(ResultType::Playlists, "42", "Label");
        let state = DetailState::Results(Box::new(page.clone()));
        let matching = route(ResultType::Playlists, "42");
        assert!(info_for_detail_state(&state, &matching).is_some());
        assert!(info_for_detail_state(&state, &route(ResultType::Playlists, "43")).is_none());
        assert!(info_for_detail_state(&state, &route(ResultType::Albums, "42")).is_none());
        assert!(info_for_detail_state(&DetailState::Loading, &matching).is_none());

        let empty_info = DetailPage {
            album_info: Some(AlbumInfo::default()),
            ..page
        };
        let empty_state = DetailState::Empty(Box::new(empty_info));
        assert!(info_for_detail_state(&empty_state, &matching).is_none());
    }
}
