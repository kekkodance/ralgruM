use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

pub(crate) const CATEGORIES: [ResultType; 4] = [
    ResultType::Tracks,
    ResultType::Albums,
    ResultType::Artists,
    ResultType::Playlists,
];

// A snapshot can contain several result groups and artwork URLs. Keep enough
// recent searches for normal source and type switching without retaining every
// query entered in a long-lived session.
const SEARCH_RESULT_CACHE_LIMIT: usize = 48;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Source {
    #[default]
    All,
    Deezer,
    SoundCloud,
}

impl Source {
    pub(crate) const fn providers(self) -> &'static [Provider] {
        match self {
            Self::All => &[Provider::Deezer, Provider::SoundCloud],
            Self::Deezer => &[Provider::Deezer],
            Self::SoundCloud => &[Provider::SoundCloud],
        }
    }

    const fn exclusive_provider(self) -> Option<Provider> {
        match self {
            Self::All => None,
            Self::Deezer => Some(Provider::Deezer),
            Self::SoundCloud => Some(Provider::SoundCloud),
        }
    }

    fn covers(self, wanted: Self) -> bool {
        self == Self::All || self == wanted
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum ResultType {
    #[default]
    All,
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl ResultType {
    fn covers(self, wanted: Self) -> bool {
        self == Self::All || self == wanted
    }

    pub(crate) const fn categories(self) -> &'static [ResultType] {
        match self {
            Self::All => &CATEGORIES,
            Self::Tracks => &[Self::Tracks],
            Self::Albums => &[Self::Albums],
            Self::Artists => &[Self::Artists],
            Self::Playlists => &[Self::Playlists],
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Tracks => "Tracks",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::Playlists => "Playlists",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) enum Provider {
    Deezer,
    #[default]
    SoundCloud,
}

impl Provider {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Deezer => "Deezer",
            Self::SoundCloud => "SoundCloud",
        }
    }

    const fn all_source_order(self) -> u8 {
        match self {
            Self::Deezer => 0,
            Self::SoundCloud => 1,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchRequest {
    pub(crate) provider: Provider,
    pub(crate) category: ResultType,
    pub(crate) query: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RawResult {
    pub(crate) request: SearchRequest,
    pub(crate) data: Result<Vec<Value>, ProviderError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProviderError {
    pub(crate) message: String,
}

impl ProviderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: sanitize_error(&message.into()),
        }
    }
}

fn sanitize_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if ["arl", "token", "authorization", "client_id", "cookie"]
        .iter()
        .any(|secret| lower.contains(secret))
    {
        "Provider request failed".into()
    } else {
        message.to_owned()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Track {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) artists: Vec<TrackArtistRef>,
    pub(crate) album: String,
    pub(crate) album_id: String,
    pub(crate) release_date: String,
    pub(crate) duration: u64,
    pub(crate) downloadable: bool,
    pub(crate) progressive: bool,
    pub(crate) artwork: String,
    pub(crate) source: Provider,
    pub(crate) explicit: bool,
    pub(crate) favorite: Option<bool>,
    /// Provider-supplied public web URL. SoundCloud fills this from the
    /// permalink; Deezer links are derived from the numeric id instead.
    pub(crate) service_url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackArtistRef {
    pub(crate) id: String,
    pub(crate) name: String,
}

/// Collects artist references while deduplicating by id or lowercase name,
/// keeping the first occurrence order.
#[derive(Default)]
pub(crate) struct TrackArtistCollector {
    artists: Vec<TrackArtistRef>,
    seen_ids: HashSet<String>,
    seen_names: HashSet<String>,
}

impl TrackArtistCollector {
    pub(crate) fn push(&mut self, id: &str, name: &str) {
        let id = id.trim();
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let name_key = name.to_lowercase();
        if (!id.is_empty() && !self.seen_ids.insert(id.to_owned()))
            || !self.seen_names.insert(name_key)
        {
            return;
        }
        self.artists.push(TrackArtistRef {
            id: id.to_owned(),
            name: name.to_owned(),
        });
    }

    pub(crate) fn finish(self) -> Vec<TrackArtistRef> {
        self.artists
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Card {
    pub(crate) kind: ResultType,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) artwork: String,
    pub(crate) source: Provider,
    pub(crate) badge: String,
    pub(crate) release_date: String,
    /// Provider-supplied public web URL (SoundCloud permalink). Deezer
    /// collection links are derived from the numeric id instead.
    pub(crate) service_url: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ArtistPage {
    pub(crate) profile: Card,
    pub(crate) favorite: Option<bool>,
    pub(crate) fans: Option<u64>,
    pub(crate) popular_tracks: Vec<Track>,
    pub(crate) popular_total: usize,
    pub(crate) similar_artists: Vec<Card>,
    pub(crate) similar_total: usize,
    pub(crate) albums: Vec<Card>,
    pub(crate) albums_total: usize,
    pub(crate) featured: Vec<Card>,
    pub(crate) featured_total: usize,
    pub(crate) playlists: Vec<Card>,
    pub(crate) playlists_total: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Groups {
    pub(crate) tracks: Vec<Track>,
    pub(crate) albums: Vec<Card>,
    pub(crate) artists: Vec<Card>,
    pub(crate) playlists: Vec<Card>,
}

impl Groups {
    pub(crate) fn count(&self, result_type: ResultType) -> usize {
        match result_type {
            ResultType::All => {
                self.tracks.len() + self.albums.len() + self.artists.len() + self.playlists.len()
            }
            ResultType::Tracks => self.tracks.len(),
            ResultType::Albums => self.albums.len(),
            ResultType::Artists => self.artists.len(),
            ResultType::Playlists => self.playlists.len(),
        }
    }

    pub(crate) fn is_track_only(&self) -> bool {
        !self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }

    fn retaining(&self, provider: Provider) -> Self {
        Self {
            tracks: self
                .tracks
                .iter()
                .filter(|track| track.source == provider)
                .cloned()
                .collect(),
            albums: self
                .albums
                .iter()
                .filter(|card| card.source == provider)
                .cloned()
                .collect(),
            artists: self
                .artists
                .iter()
                .filter(|card| card.source == provider)
                .cloned()
                .collect(),
            playlists: self
                .playlists
                .iter()
                .filter(|card| card.source == provider)
                .cloned()
                .collect(),
        }
    }

    fn extend_from(&mut self, other: &Self) {
        self.tracks.extend(other.tracks.iter().cloned());
        self.albums.extend(other.albums.iter().cloned());
        self.artists.extend(other.artists.iter().cloned());
        self.playlists.extend(other.playlists.iter().cloned());
    }

    fn sort_by_provider_order(&mut self) {
        self.tracks
            .sort_by_key(|track| track.source.all_source_order());
        self.albums
            .sort_by_key(|card| card.source.all_source_order());
        self.artists
            .sort_by_key(|card| card.source.all_source_order());
        self.playlists
            .sort_by_key(|card| card.source.all_source_order());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResultState {
    Initial,
    Loading,
    Results,
    Empty,
    Failed(String),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SearchCacheKey {
    source: Source,
    result_type: ResultType,
    query: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SearchCacheSnapshot {
    state: ResultState,
    groups: Groups,
    warning: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchState {
    pub(crate) source: Source,
    pub(crate) result_type: ResultType,
    pub(crate) state: ResultState,
    pub(crate) groups: Groups,
    pub(crate) warning: Option<String>,
    generation: u64,
    query: String,
    result_cache: HashMap<SearchCacheKey, SearchCacheSnapshot>,
    incremental_failures: Vec<(Provider, ProviderError)>,
    result_cache_order: VecDeque<SearchCacheKey>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            source: Source::All,
            result_type: ResultType::All,
            state: ResultState::Initial,
            groups: Groups::default(),
            warning: None,
            generation: 0,
            query: String::new(),
            result_cache: HashMap::new(),
            incremental_failures: Vec::new(),
            result_cache_order: VecDeque::new(),
        }
    }
}

impl SearchState {
    pub(crate) fn account_scope_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.query.clear();
        self.groups = Groups::default();
        self.warning = None;
        self.state = ResultState::Initial;
        self.result_cache.clear();
        self.incremental_failures.clear();
        self.result_cache_order.clear();
    }
    pub(crate) fn submit(&mut self, query: &str) -> Option<SearchJob> {
        self.result_type = ResultType::All;
        self.start(query)
    }

    pub(crate) fn select_source(&mut self, source: Source, query: &str) -> Option<SearchJob> {
        self.source = source;
        self.start(query)
    }

    pub(crate) fn select_type(
        &mut self,
        result_type: ResultType,
        query: &str,
    ) -> Option<SearchJob> {
        self.result_type = result_type;
        self.start(query)
    }

    fn start(&mut self, query: &str) -> Option<SearchJob> {
        let query = query.trim();
        if query.is_empty() {
            self.generation = self.generation.wrapping_add(1);
            self.query.clear();
            self.groups = Groups::default();
            self.warning = None;
            self.state = ResultState::Initial;
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.incremental_failures.clear();
        self.query = query.to_owned();
        if self.restore_cached() {
            return None;
        }
        let missing = self.missing_providers();
        let mut groups = Groups::default();
        for provider in self.source.providers() {
            if let Some(snapshot) = self.provider_snapshot(*provider) {
                groups.extend_from(&snapshot.groups);
            }
        }
        self.groups = groups;
        self.warning = None;
        self.state = ResultState::Loading;
        let providers = if missing.is_empty() {
            self.source.providers().to_vec()
        } else {
            missing
        };
        Some(SearchJob {
            generation: self.generation,
            requests: self
                .result_type
                .categories()
                .iter()
                .flat_map(|category| {
                    providers
                        .iter()
                        .copied()
                        .map(move |provider| SearchRequest {
                            provider,
                            category: *category,
                            query: query.to_owned(),
                        })
                })
                .collect(),
        })
    }

    pub(crate) fn complete(&mut self, generation: u64, results: Vec<RawResult>) -> bool {
        self.complete_with_missing_accounts(generation, results, &[])
    }

    pub(crate) fn complete_with_missing_accounts(
        &mut self,
        generation: u64,
        results: Vec<RawResult>,
        missing_accounts: &[Provider],
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        let mut failures = Vec::new();
        for result in results {
            match result.data {
                Ok(items) => crate::search::normalize::append(
                    &mut self.groups,
                    result.request.provider,
                    result.request.category,
                    items,
                ),
                Err(error) => failures.push((result.request.provider, error)),
            }
        }
        if self.source == Source::All {
            self.groups.sort_by_provider_order();
        }
        let total = self.groups.count(self.result_type);
        self.warning = partial_warning(&failures, missing_accounts);
        self.state = if total > 0 {
            ResultState::Results
        } else if !failures.is_empty()
            && failures.len() == self.source.providers().len() * self.result_type.categories().len()
        {
            ResultState::Failed(failures[0].1.message.clone())
        } else {
            ResultState::Empty
        };
        if !self.query.is_empty()
            && matches!(&self.state, ResultState::Results | ResultState::Empty)
        {
            let key = self.cache_key();
            self.cache_insert(
                key,
                SearchCacheSnapshot {
                    state: self.state.clone(),
                    groups: self.groups.clone(),
                    warning: self.warning.clone(),
                },
            );
        }
        true
    }

    /// Applies one independently completed provider batch. Results become
    /// visible immediately, while a cache snapshot is recorded only after the
    /// final batch so a later navigation can never restore a partial search.
    pub(crate) fn complete_incremental_with_missing_accounts(
        &mut self,
        generation: u64,
        results: Vec<RawResult>,
        missing_accounts: &[Provider],
        final_batch: bool,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        for result in results {
            match result.data {
                Ok(items) => crate::search::normalize::append(
                    &mut self.groups,
                    result.request.provider,
                    result.request.category,
                    items,
                ),
                Err(error) => self
                    .incremental_failures
                    .push((result.request.provider, error)),
            }
        }
        if self.source == Source::All {
            self.groups.sort_by_provider_order();
        }
        let total = self.groups.count(self.result_type);
        self.warning = partial_warning(&self.incremental_failures, missing_accounts);
        self.state = if total > 0 {
            ResultState::Results
        } else if final_batch
            && !self.incremental_failures.is_empty()
            && self.incremental_failures.len()
                == self.source.providers().len() * self.result_type.categories().len()
        {
            ResultState::Failed(self.incremental_failures[0].1.message.clone())
        } else if final_batch {
            ResultState::Empty
        } else {
            ResultState::Loading
        };
        if final_batch {
            if !self.query.is_empty()
                && matches!(&self.state, ResultState::Results | ResultState::Empty)
            {
                let key = self.cache_key();
                self.cache_insert(
                    key,
                    SearchCacheSnapshot {
                        state: self.state.clone(),
                        groups: self.groups.clone(),
                        warning: self.warning.clone(),
                    },
                );
            }
            self.incremental_failures.clear();
        }
        true
    }

    fn cache_key(&self) -> SearchCacheKey {
        SearchCacheKey {
            source: self.source,
            result_type: self.result_type,
            query: normalize_cache_query(&self.query),
        }
    }

    fn restore_cached(&mut self) -> bool {
        let key = self.cache_key();
        if let Some(snapshot) = self.cache_get(&key) {
            self.apply_snapshot(snapshot);
            return true;
        }
        let Some(snapshot) = self.derive_snapshot() else {
            return false;
        };
        self.apply_snapshot(snapshot.clone());
        self.cache_insert(key, snapshot);
        true
    }

    fn apply_snapshot(&mut self, snapshot: SearchCacheSnapshot) {
        self.state = snapshot.state;
        self.groups = snapshot.groups;
        self.warning = snapshot.warning;
    }

    fn derive_snapshot(&mut self) -> Option<SearchCacheSnapshot> {
        if self.source == Source::All
            && let Some(merged) = self.merge_provider_snapshots()
        {
            return Some(merged);
        }
        let query = normalize_cache_query(&self.query);
        let parents = [
            SearchCacheKey {
                source: Source::All,
                result_type: self.result_type,
                query: query.clone(),
            },
            SearchCacheKey {
                source: self.source,
                result_type: ResultType::All,
                query: query.clone(),
            },
            SearchCacheKey {
                source: Source::All,
                result_type: ResultType::All,
                query,
            },
        ];
        for parent_key in parents {
            if parent_key.source == self.source && parent_key.result_type == self.result_type {
                continue;
            }
            if !parent_key.source.covers(self.source)
                || !parent_key.result_type.covers(self.result_type)
            {
                continue;
            }
            let Some(parent) = self.cache_get(&parent_key) else {
                continue;
            };
            return Some(narrow_snapshot(&parent, self.source, self.result_type));
        }
        None
    }

    fn merge_provider_snapshots(&mut self) -> Option<SearchCacheSnapshot> {
        let deezer = self.provider_snapshot(Provider::Deezer)?;
        let soundcloud = self.provider_snapshot(Provider::SoundCloud)?;
        Some(merge_snapshots(deezer, soundcloud, self.result_type))
    }

    fn missing_providers(&mut self) -> Vec<Provider> {
        self.source
            .providers()
            .iter()
            .copied()
            .filter(|provider| self.provider_snapshot(*provider).is_none())
            .collect()
    }

    fn provider_snapshot(&mut self, provider: Provider) -> Option<SearchCacheSnapshot> {
        let source = match provider {
            Provider::Deezer => Source::Deezer,
            Provider::SoundCloud => Source::SoundCloud,
        };
        let keys = [
            SearchCacheKey {
                source,
                result_type: self.result_type,
                query: normalize_cache_query(&self.query),
            },
            SearchCacheKey {
                source,
                result_type: ResultType::All,
                query: normalize_cache_query(&self.query),
            },
        ];
        for key in keys {
            if !key.result_type.covers(self.result_type) {
                continue;
            }
            let Some(parent) = self.cache_get(&key) else {
                continue;
            };
            return Some(narrow_snapshot(&parent, source, self.result_type));
        }
        None
    }

    fn cache_get(&mut self, key: &SearchCacheKey) -> Option<SearchCacheSnapshot> {
        let snapshot = self.result_cache.get(key).cloned()?;
        self.touch_cache_key(key);
        Some(snapshot)
    }

    fn cache_insert(&mut self, key: SearchCacheKey, snapshot: SearchCacheSnapshot) {
        if !self.result_cache.contains_key(&key)
            && self.result_cache.len() >= SEARCH_RESULT_CACHE_LIMIT
        {
            if let Some(oldest) = self.result_cache_order.pop_front() {
                self.result_cache.remove(&oldest);
            }
        }
        self.result_cache.insert(key.clone(), snapshot);
        self.touch_cache_key(&key);
    }

    fn touch_cache_key(&mut self, key: &SearchCacheKey) {
        if let Some(position) = self
            .result_cache_order
            .iter()
            .position(|candidate| candidate == key)
        {
            self.result_cache_order.remove(position);
        }
        self.result_cache_order.push_back(key.clone());
    }
}

fn normalize_cache_query(query: &str) -> String {
    query.trim().to_lowercase()
}

fn narrow_snapshot(
    parent: &SearchCacheSnapshot,
    source: Source,
    result_type: ResultType,
) -> SearchCacheSnapshot {
    let groups = match source.exclusive_provider() {
        Some(provider) => parent.groups.retaining(provider),
        None => parent.groups.clone(),
    };
    let state = if groups.count(result_type) > 0 {
        ResultState::Results
    } else {
        ResultState::Empty
    };
    SearchCacheSnapshot {
        state,
        groups,
        warning: narrow_warning(parent.warning.as_deref(), source),
    }
}

fn merge_snapshots(
    deezer: SearchCacheSnapshot,
    soundcloud: SearchCacheSnapshot,
    result_type: ResultType,
) -> SearchCacheSnapshot {
    let mut groups = deezer.groups;
    groups.extend_from(&soundcloud.groups);
    let warning = merge_provider_warnings(deezer.warning.as_deref(), soundcloud.warning.as_deref());
    let state = if groups.count(result_type) > 0 {
        ResultState::Results
    } else {
        ResultState::Empty
    };
    SearchCacheSnapshot {
        state,
        groups,
        warning,
    }
}

fn merge_provider_warnings(deezer: Option<&str>, soundcloud: Option<&str>) -> Option<String> {
    match (
        deezer.is_some_and(|warning| warning.contains("Deezer")),
        soundcloud.is_some_and(|warning| warning.contains("SoundCloud")),
    ) {
        (true, true) => {
            Some("Deezer and SoundCloud did not return every requested category.".into())
        }
        (true, false) => Some("Deezer did not return every requested category.".into()),
        (false, true) => Some("SoundCloud did not return every requested category.".into()),
        (false, false) => None,
    }
}

fn narrow_warning(warning: Option<&str>, source: Source) -> Option<String> {
    let warning = warning?;
    match source {
        Source::All => Some(warning.to_owned()),
        Source::Deezer => warning
            .contains("Deezer")
            .then(|| "Deezer did not return every requested category.".into()),
        Source::SoundCloud => warning
            .contains("SoundCloud")
            .then(|| "SoundCloud did not return every requested category.".into()),
    }
}

fn partial_warning(
    failures: &[(Provider, ProviderError)],
    missing_accounts: &[Provider],
) -> Option<String> {
    let deezer = failures
        .iter()
        .any(|(provider, _)| *provider == Provider::Deezer && !missing_accounts.contains(provider));
    let soundcloud = failures.iter().any(|(provider, _)| {
        *provider == Provider::SoundCloud && !missing_accounts.contains(provider)
    });
    match (deezer, soundcloud) {
        (true, true) => {
            Some("Deezer and SoundCloud did not return every requested category.".into())
        }
        (true, false) => Some("Deezer did not return every requested category.".into()),
        (false, true) => Some("SoundCloud did not return every requested category.".into()),
        (false, false) => None,
    }
}

pub(crate) struct SearchJob {
    pub(crate) generation: u64,
    pub(crate) requests: Vec<SearchRequest>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_expands_categories_with_deezer_before_soundcloud() {
        let mut state = SearchState::default();
        let job = state.submit(" query ").unwrap();
        assert_eq!(job.requests.len(), 8);
        for pair in job.requests.chunks_exact(2) {
            assert_eq!(pair[0].provider, Provider::Deezer);
            assert_eq!(pair[1].provider, Provider::SoundCloud);
            assert_eq!(pair[0].category, pair[1].category);
            assert_eq!(pair[0].query, "query");
        }
    }

    #[test]
    fn transitions_rerun_nonempty_query_and_submit_resets_type() {
        let mut state = SearchState {
            result_type: ResultType::Artists,
            ..SearchState::default()
        };
        assert_eq!(state.submit("x").unwrap().requests.len(), 8);
        assert_eq!(state.result_type, ResultType::All);
        assert_eq!(
            state
                .select_type(ResultType::Albums, "x")
                .unwrap()
                .requests
                .len(),
            2
        );
        assert!(state.select_source(Source::SoundCloud, " ").is_none());
    }

    #[test]
    fn stale_completion_is_ignored() {
        let mut state = SearchState::default();
        let first = state.submit("first").unwrap();
        let _second = state.submit("second").unwrap();
        assert!(!state.complete(first.generation, Vec::new()));
        assert_eq!(state.state, ResultState::Loading);
    }

    #[test]
    fn incremental_batches_publish_early_without_caching_a_partial_snapshot() {
        let mut state = SearchState::default();
        let job = state.submit("query").unwrap();
        let deezer = job
            .requests
            .iter()
            .find(|request| {
                request.provider == Provider::Deezer && request.category == ResultType::Tracks
            })
            .cloned()
            .unwrap();
        assert!(state.complete_incremental_with_missing_accounts(
            job.generation,
            vec![RawResult {
                request: deezer,
                data: Ok(vec![serde_json::json!({"id": 1, "title": "Track"})]),
            }],
            &[],
            false,
        ));
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks.len(), 1);
        assert!(state.result_cache.is_empty());

        let remaining = job
            .requests
            .into_iter()
            .filter(|request| {
                request.provider != Provider::Deezer || request.category != ResultType::Tracks
            })
            .map(|request| RawResult {
                request,
                data: Ok(Vec::new()),
            })
            .collect();
        assert!(state.complete_incremental_with_missing_accounts(
            job.generation,
            remaining,
            &[],
            true,
        ));
        assert_eq!(state.result_cache.len(), 1);
    }

    #[test]
    fn cache_hit_restores_terminal_snapshot_for_normalized_query() {
        let mut state = SearchState::default();
        let job = state.select_type(ResultType::Tracks, " query ").unwrap();
        assert!(state.complete(
            job.generation,
            vec![
                RawResult {
                    request: job.requests[0].clone(),
                    data: Ok(vec![serde_json::json!({
                        "id": 1,
                        "title": "Track"
                    })]),
                },
                RawResult {
                    request: job.requests[1].clone(),
                    data: Err(ProviderError::new("temporary failure")),
                },
            ],
        ));
        let cached_state = state.state.clone();
        let cached_groups = state.groups.clone();
        let cached_warning = state.warning.clone();
        assert_eq!(cached_state, ResultState::Results);
        assert!(cached_warning.is_some());

        assert!(state.submit("other").is_some());
        assert_eq!(state.state, ResultState::Loading);
        assert!(state.select_type(ResultType::Tracks, "  QuErY\t").is_none());
        assert_eq!(state.state, cached_state);
        assert_eq!(state.groups, cached_groups);
        assert_eq!(state.warning, cached_warning);
    }

    #[test]
    fn cache_is_separated_by_result_type_and_query() {
        let mut state = SearchState::default();
        let track_job = state.select_type(ResultType::Tracks, "shared").unwrap();
        assert!(state.complete(
            track_job.generation,
            vec![RawResult {
                request: track_job.requests[0].clone(),
                data: Ok(vec![serde_json::json!({
                    "id": 1,
                    "title": "Track"
                })]),
            }],
        ));
        let tracks = state.groups.tracks.clone();

        let album_job = state.select_type(ResultType::Albums, "shared").unwrap();
        assert_eq!(state.state, ResultState::Loading);
        assert!(state.complete(album_job.generation, Vec::new()));
        assert_eq!(state.state, ResultState::Empty);

        assert!(state.select_type(ResultType::Tracks, " shared ").is_none());
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks, tracks);

        let other_query_job = state.select_type(ResultType::Tracks, "other").unwrap();
        assert!(state.complete(other_query_job.generation, Vec::new()));
        assert_eq!(state.state, ResultState::Empty);

        assert!(state.select_type(ResultType::Albums, "shared").is_none());
        assert_eq!(state.state, ResultState::Empty);
        assert!(state.select_type(ResultType::Tracks, "shared").is_none());
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks, tracks);
    }

    #[test]
    fn result_cache_uses_normalized_lru_eviction() {
        let mut state = SearchState::default();
        let snapshot = SearchCacheSnapshot {
            state: ResultState::Empty,
            groups: Groups::default(),
            warning: None,
        };
        for index in 0..SEARCH_RESULT_CACHE_LIMIT {
            state.cache_insert(
                SearchCacheKey {
                    source: Source::SoundCloud,
                    result_type: ResultType::Tracks,
                    query: normalize_cache_query(&format!(" Query {index} ")),
                },
                snapshot.clone(),
            );
        }
        let first = SearchCacheKey {
            source: Source::SoundCloud,
            result_type: ResultType::Tracks,
            query: "query 0".into(),
        };
        assert!(state.cache_get(&first).is_some());

        state.cache_insert(
            SearchCacheKey {
                source: Source::SoundCloud,
                result_type: ResultType::Tracks,
                query: "newest".into(),
            },
            snapshot,
        );

        assert_eq!(state.result_cache.len(), SEARCH_RESULT_CACHE_LIMIT);
        assert!(state.result_cache.contains_key(&first));
        assert!(!state.result_cache.contains_key(&SearchCacheKey {
            source: Source::SoundCloud,
            result_type: ResultType::Tracks,
            query: "query 1".into(),
        }));
        assert_eq!(normalize_cache_query("  MiXeD Case\t"), "mixed case");
    }

    #[test]
    fn account_scope_change_clears_search_result_cache() {
        let mut state = SearchState::default();
        let job = state.select_type(ResultType::Tracks, "query").unwrap();
        assert!(state.complete(
            job.generation,
            vec![RawResult {
                request: job.requests[0].clone(),
                data: Ok(vec![serde_json::json!({
                    "id": 1,
                    "title": "Track"
                })]),
            }],
        ));
        assert_eq!(state.result_cache.len(), 1);

        state.account_scope_changed();

        assert!(state.result_cache.is_empty());
        assert!(state.select_type(ResultType::Tracks, "query").is_some());
        assert_eq!(state.state, ResultState::Loading);
    }

    #[test]
    fn failed_results_are_not_cached() {
        let mut state = SearchState::default();
        let job = state.select_source(Source::SoundCloud, "query").unwrap();
        let failures = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                request,
                data: Err(ProviderError::new("network unavailable")),
            })
            .collect();
        assert!(state.complete(job.generation, failures));
        assert!(matches!(state.state, ResultState::Failed(_)));
        assert!(state.result_cache.is_empty());

        assert!(state.select_source(Source::SoundCloud, " query ").is_some());
        assert_eq!(state.state, ResultState::Loading);
    }

    #[test]
    fn account_scope_change_resets_results_and_invalidates_pending_completion() {
        let mut state = SearchState {
            source: Source::Deezer,
            result_type: ResultType::Tracks,
            groups: Groups {
                tracks: vec![Track {
                    id: "42".into(),
                    favorite: Some(true),
                    ..Track::default()
                }],
                ..Groups::default()
            },
            warning: Some("old account warning".into()),
            ..SearchState::default()
        };
        let job = state.select_type(ResultType::Tracks, "first").unwrap();
        state.groups.tracks.push(Track {
            id: "42".into(),
            favorite: Some(true),
            ..Track::default()
        });
        state.warning = Some("old account warning".into());
        state.account_scope_changed();

        assert_eq!(state.source, Source::Deezer);
        assert_eq!(state.result_type, ResultType::Tracks);
        assert_eq!(state.state, ResultState::Initial);
        assert_eq!(state.groups, Groups::default());
        assert!(state.warning.is_none());
        assert!(state.query.is_empty());
        assert!(!state.complete(job.generation, Vec::new()));
    }

    #[test]
    fn empty_query_clears_results_and_invalidates_pending_work() {
        let mut state = SearchState::default();
        let job = state.submit("first").unwrap();
        assert!(state.select_source(Source::SoundCloud, " ").is_none());
        assert_eq!(state.state, ResultState::Initial);
        assert_eq!(state.groups, Groups::default());
        assert!(!state.complete(job.generation, Vec::new()));
    }

    #[test]
    fn exact_error_states_are_selected() {
        let mut state = SearchState::default();
        assert!(state.select_type(ResultType::Tracks, "").is_none());
        assert_eq!(state.state, ResultState::Initial);
        let job = state.select_source(Source::SoundCloud, "x").unwrap();
        let request = job.requests[0].clone();
        state.complete(
            job.generation,
            vec![RawResult {
                request,
                data: Err(ProviderError::new("network unavailable")),
            }],
        );
        assert_eq!(
            state.state,
            ResultState::Failed("network unavailable".into())
        );
    }

    #[test]
    fn deezer_search_runs_without_an_account() {
        let mut state = SearchState::default();
        let job = state.select_source(Source::Deezer, "query").unwrap();
        assert_eq!(state.state, ResultState::Loading);
        assert_eq!(job.requests.len(), 4);
        assert!(
            job.requests
                .iter()
                .all(|request| request.provider == Provider::Deezer)
        );
        assert!(job.requests.iter().all(|request| request.query == "query"));
    }

    #[test]
    fn all_source_switch_reuses_cached_all_results() {
        let mut state = SearchState::default();
        let job = state.submit("query").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({
                        "id": match request.provider {
                            Provider::Deezer => 1,
                            Provider::SoundCloud => 2,
                        },
                        "title": "Track"
                    })])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect();
        assert!(state.complete(job.generation, results));
        assert_eq!(state.groups.tracks.len(), 2);

        assert!(state.select_source(Source::Deezer, "query").is_none());
        assert_eq!(state.source, Source::Deezer);
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks.len(), 1);
        assert!(
            state
                .groups
                .tracks
                .iter()
                .all(|track| track.source == Provider::Deezer)
        );

        assert!(state.select_source(Source::SoundCloud, "query").is_none());
        assert_eq!(state.groups.tracks.len(), 1);
        assert!(
            state
                .groups
                .tracks
                .iter()
                .all(|track| track.source == Provider::SoundCloud)
        );

        assert!(state.select_source(Source::All, "query").is_none());
        assert_eq!(state.groups.tracks.len(), 2);
    }

    #[test]
    fn all_type_switch_reuses_cached_all_results() {
        let mut state = SearchState::default();
        let job = state.submit("query").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: match request.category {
                    ResultType::Tracks => Ok(vec![serde_json::json!({
                        "id": 1,
                        "title": "Track"
                    })]),
                    ResultType::Albums => Ok(vec![serde_json::json!({
                        "id": 2,
                        "title": "Album"
                    })]),
                    _ => Ok(Vec::new()),
                },
                request,
            })
            .collect();
        assert!(state.complete(job.generation, results));

        assert!(state.select_type(ResultType::Tracks, "query").is_none());
        assert_eq!(state.result_type, ResultType::Tracks);
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks.len(), 2);
        assert_eq!(state.groups.albums.len(), 2);

        assert!(state.select_type(ResultType::Albums, "query").is_none());
        assert_eq!(state.result_type, ResultType::Albums);
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.count(ResultType::Albums), 2);
    }

    #[test]
    fn dedicated_source_search_only_fetches_the_missing_provider_for_all() {
        let mut state = SearchState::default();
        let job = state.select_source(Source::Deezer, "query").unwrap();
        assert!(state.complete(
            job.generation,
            vec![RawResult {
                request: job.requests[0].clone(),
                data: Ok(vec![serde_json::json!({
                    "id": 1,
                    "title": "Track"
                })]),
            }],
        ));

        let job = state.select_source(Source::All, "query").unwrap();
        assert_eq!(state.state, ResultState::Loading);
        assert_eq!(state.groups.tracks.len(), 1);
        assert!(
            job.requests
                .iter()
                .all(|request| request.provider == Provider::SoundCloud)
        );
    }

    #[test]
    fn all_reuses_cached_deezer_and_soundcloud_searches() {
        let mut state = SearchState::default();
        let deezer = state.select_source(Source::Deezer, "query").unwrap();
        assert!(state.complete(
            deezer.generation,
            vec![RawResult {
                request: deezer.requests[0].clone(),
                data: Ok(vec![serde_json::json!({
                    "id": 1,
                    "title": "Deezer"
                })]),
            }],
        ));
        let soundcloud = state.select_source(Source::SoundCloud, "query").unwrap();
        assert!(state.complete(
            soundcloud.generation,
            vec![RawResult {
                request: soundcloud.requests[0].clone(),
                data: Ok(vec![serde_json::json!({
                    "id": 2,
                    "title": "SoundCloud"
                })]),
            }],
        ));

        assert!(state.select_source(Source::All, "query").is_none());
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks.len(), 2);
        assert!(
            state
                .groups
                .tracks
                .iter()
                .any(|track| track.source == Provider::Deezer)
        );
        assert!(
            state
                .groups
                .tracks
                .iter()
                .any(|track| track.source == Provider::SoundCloud)
        );
        assert_eq!(
            state
                .groups
                .tracks
                .iter()
                .map(|track| track.source)
                .collect::<Vec<_>>(),
            vec![Provider::Deezer, Provider::SoundCloud]
        );
    }

    #[test]
    fn all_source_orders_late_deezer_before_cached_soundcloud_and_reentry() {
        let mut state = SearchState::default();
        let soundcloud = state.select_source(Source::SoundCloud, "query").unwrap();
        let soundcloud_request = soundcloud
            .requests
            .iter()
            .find(|request| request.category == ResultType::Tracks)
            .cloned()
            .unwrap();
        assert!(state.complete(
            soundcloud.generation,
            vec![RawResult {
                request: soundcloud_request,
                data: Ok(vec![serde_json::json!({
                    "id": 2,
                    "title": "SoundCloud"
                })]),
            }],
        ));

        let all = state
            .select_source(Source::All, "query")
            .expect("missing Deezer provider should be fetched");
        assert_eq!(
            state
                .groups
                .tracks
                .iter()
                .map(|track| track.source)
                .collect::<Vec<_>>(),
            vec![Provider::SoundCloud]
        );
        assert!(
            all.requests
                .iter()
                .all(|request| request.provider == Provider::Deezer)
        );
        let deezer_request = all
            .requests
            .iter()
            .find(|request| request.category == ResultType::Tracks)
            .cloned()
            .unwrap();
        assert!(state.complete(
            all.generation,
            vec![RawResult {
                request: deezer_request,
                data: Ok(vec![serde_json::json!({
                    "id": 1,
                    "title": "Deezer"
                })]),
            }],
        ));
        assert_eq!(
            state
                .groups
                .tracks
                .iter()
                .map(|track| track.source)
                .collect::<Vec<_>>(),
            vec![Provider::Deezer, Provider::SoundCloud]
        );
        assert_eq!(
            state
                .groups
                .tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Deezer", "SoundCloud"]
        );

        assert!(state.select_source(Source::All, "query").is_none());
        assert_eq!(
            state
                .groups
                .tracks
                .iter()
                .map(|track| track.source)
                .collect::<Vec<_>>(),
            vec![Provider::Deezer, Provider::SoundCloud]
        );
    }

    #[test]
    fn derived_single_source_view_drops_the_other_provider_warning() {
        let mut state = SearchState::default();
        let job = state.submit("query").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.provider == Provider::Deezer {
                    Err(ProviderError::new("Deezer search timed out"))
                } else if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({"id": 1, "title": "Track"})])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect();
        assert!(state.complete(job.generation, results));
        assert_eq!(
            state.warning.as_deref(),
            Some("Deezer did not return every requested category.")
        );

        assert!(state.select_source(Source::SoundCloud, "query").is_none());
        assert!(state.warning.is_none());
        assert_eq!(state.state, ResultState::Results);

        assert!(state.select_source(Source::Deezer, "query").is_none());
        assert_eq!(
            state.warning.as_deref(),
            Some("Deezer did not return every requested category.")
        );
        assert_eq!(state.state, ResultState::Empty);
    }

    #[test]
    fn all_keeps_soundcloud_results_without_account_warning_for_missing_deezer() {
        let mut state = SearchState::default();
        let job = state.submit("x").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.provider == Provider::Deezer {
                    Err(ProviderError::new("Deezer login required"))
                } else if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({"id": 1, "title": "Track"})])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect();
        state.complete_with_missing_accounts(job.generation, results, &[Provider::Deezer]);
        assert_eq!(state.state, ResultState::Results);
        assert_eq!(state.groups.tracks.len(), 1);
        assert!(state.warning.is_none());
    }

    #[test]
    fn server_login_error_does_not_override_known_local_account_state() {
        let mut state = SearchState::default();
        let job = state.submit("x").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.provider == Provider::Deezer {
                    Err(ProviderError::new("Deezer login required"))
                } else if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({"id": 1, "title": "Track"})])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect();

        assert!(state.complete(job.generation, results));
        assert_eq!(
            state.warning.as_deref(),
            Some("Deezer did not return every requested category.")
        );
    }

    #[test]
    fn all_still_reports_real_provider_failures_after_account_filtering() {
        let mut state = SearchState::default();
        let job = state.submit("x").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.provider == Provider::Deezer {
                    Err(ProviderError::new("Deezer search timed out"))
                } else if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({"id": 1, "title": "Track"})])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect();
        assert!(state.complete(job.generation, results));
        assert_eq!(
            state.warning.as_deref(),
            Some("Deezer did not return every requested category.")
        );
    }

    #[test]
    fn known_missing_account_suppresses_provider_warning_regardless_of_error_text() {
        let mut state = SearchState::default();
        let job = state.submit("x").unwrap();
        let results = job
            .requests
            .iter()
            .cloned()
            .map(|request| RawResult {
                data: if request.provider == Provider::SoundCloud {
                    Err(ProviderError::new("SoundCloud returned 401"))
                } else if request.category == ResultType::Tracks {
                    Ok(vec![serde_json::json!({"id": 1, "title": "Track"})])
                } else {
                    Ok(Vec::new())
                },
                request,
            })
            .collect::<Vec<_>>();

        assert!(state.complete_with_missing_accounts(
            job.generation,
            results.clone(),
            &[Provider::SoundCloud],
        ));
        assert_eq!(state.state, ResultState::Results);
        assert!(state.warning.is_none());

        let mut ordinary_failure = SearchState::default();
        let ordinary_job = ordinary_failure.submit("x").unwrap();
        assert!(ordinary_failure.complete(ordinary_job.generation, results));
        assert_eq!(
            ordinary_failure.warning.as_deref(),
            Some("SoundCloud did not return every requested category.")
        );
    }

    #[test]
    fn track_only_detection_requires_tracks_and_no_other_categories() {
        let mut groups = Groups::default();
        assert!(!groups.is_track_only());
        groups.tracks.push(Track::default());
        assert!(groups.is_track_only());
        groups.albums.push(Card::default());
        assert!(!groups.is_track_only());
        groups.albums.clear();
        groups.artists.push(Card::default());
        assert!(!groups.is_track_only());
        groups.artists.clear();
        groups.playlists.push(Card::default());
        assert!(!groups.is_track_only());
    }

    #[test]
    fn errors_never_retain_secret_shaped_messages() {
        for message in [
            "ARL=secret",
            "Authorization bearer secret",
            "client_id=secret",
        ] {
            assert_eq!(
                ProviderError::new(message).message,
                "Provider request failed"
            );
        }
    }
}
