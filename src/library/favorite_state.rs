use std::collections::{HashMap, HashSet};

use crate::search::Provider;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FavoriteKind {
    Track,
    Album,
    Artist,
    Playlist,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FavoriteKey {
    pub provider: Provider,
    pub kind: FavoriteKind,
    pub id: String,
}

impl FavoriteKey {
    pub(crate) fn deezer(kind: FavoriteKind, id: String) -> Self {
        Self {
            provider: Provider::Deezer,
            kind,
            id,
        }
    }

    pub(crate) fn soundcloud(kind: FavoriteKind, id: String) -> Self {
        Self {
            provider: Provider::SoundCloud,
            kind,
            id,
        }
    }

    pub(crate) fn for_provider(provider: Provider, kind: FavoriteKind, id: String) -> Self {
        match provider {
            Provider::Deezer => Self::deezer(kind, id),
            Provider::SoundCloud => Self::soundcloud(kind, id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FavoriteMutation {
    pub generation: u64,
    pub key: FavoriteKey,
    pub favorite: bool,
    previous: Option<bool>,
}

#[derive(Default)]
pub(crate) struct FavoriteState {
    generation: u64,
    values: HashMap<FavoriteKey, bool>,
    pending: HashSet<FavoriteKey>,
    resolving: HashSet<FavoriteKey>,
    loaded_catalogs: HashSet<(Provider, FavoriteKind)>,
    loading_catalogs: HashSet<(Provider, FavoriteKind)>,
}

impl FavoriteState {
    pub(crate) fn reset_account(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.values.clear();
        self.pending.clear();
        self.resolving.clear();
        self.loaded_catalogs.clear();
        self.loading_catalogs.clear();
    }

    pub(crate) fn favorite(&self, key: &FavoriteKey) -> Option<bool> {
        self.values.get(key).copied().or_else(|| {
            self.loaded_catalogs
                .contains(&(key.provider, key.kind))
                .then_some(false)
        })
    }

    pub(crate) fn begin_catalog_load(
        &mut self,
        provider: Provider,
        kind: FavoriteKind,
    ) -> Option<u64> {
        let catalog = (provider, kind);
        if self.loaded_catalogs.contains(&catalog) || !self.loading_catalogs.insert(catalog) {
            return None;
        }
        Some(self.generation)
    }

    pub(crate) fn catalog_loading(&self, provider: Provider, kind: FavoriteKind) -> bool {
        self.loading_catalogs.contains(&(provider, kind))
    }

    pub(crate) fn finish_catalog_load(
        &mut self,
        provider: Provider,
        kind: FavoriteKind,
        generation: u64,
        ids: Option<Vec<String>>,
    ) -> bool {
        let catalog = (provider, kind);
        if generation != self.generation || !self.loading_catalogs.remove(&catalog) {
            return false;
        }
        self.resolving
            .retain(|key| key.provider != provider || key.kind != kind);
        let Some(ids) = ids else {
            return true;
        };
        for id in ids {
            let key = FavoriteKey::for_provider(provider, kind, id);
            if !self.pending(&key) {
                self.values.insert(key, true);
            }
        }
        self.loaded_catalogs.insert(catalog);
        true
    }

    pub(crate) fn set_known(&mut self, key: FavoriteKey, favorite: bool) {
        self.resolving.remove(&key);
        if !self.pending(&key) {
            self.values.insert(key, favorite);
        }
    }

    pub(crate) fn remove(&mut self, key: &FavoriteKey) {
        self.pending.remove(key);
        self.resolving.remove(key);
        self.values.remove(key);
    }

    pub(crate) fn pending(&self, key: &FavoriteKey) -> bool {
        self.pending.contains(key)
    }

    pub(crate) fn resolving(&self, key: &FavoriteKey) -> bool {
        self.resolving.contains(key)
    }

    pub(crate) fn begin_resolve(&mut self, key: FavoriteKey) -> bool {
        if self.favorite(&key).is_some() || self.pending(&key) {
            return false;
        }
        self.resolving.insert(key)
    }

    pub(crate) fn finish_resolve(&mut self, key: &FavoriteKey, favorite: Option<bool>) -> bool {
        if !self.resolving.remove(key) {
            return false;
        }
        if let Some(favorite) = favorite
            && !self.pending(key)
        {
            self.values.insert(key.clone(), favorite);
        }
        true
    }

    pub(crate) fn begin(
        &mut self,
        key: FavoriteKey,
        known_favorite: bool,
    ) -> Option<FavoriteMutation> {
        if !self.pending.insert(key.clone()) {
            return None;
        }
        self.resolving.remove(&key);
        let previous = self.favorite(&key);
        let favorite = !previous.unwrap_or(known_favorite);
        self.values.insert(key.clone(), favorite);
        Some(FavoriteMutation {
            generation: self.generation,
            key,
            favorite,
            previous,
        })
    }

    pub(crate) fn complete(
        &mut self,
        mutation: &FavoriteMutation,
        result: Result<(), String>,
    ) -> bool {
        if mutation.generation != self.generation || !self.pending.remove(&mutation.key) {
            return false;
        }
        if result.is_err() {
            if let Some(previous) = mutation.previous {
                self.values.insert(mutation.key.clone(), previous);
            } else {
                self.values.remove(&mutation.key);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(kind: FavoriteKind) -> FavoriteKey {
        FavoriteKey::deezer(kind, "42".into())
    }

    #[test]
    fn mutation_is_optimistic_and_suppresses_duplicates() {
        let mut state = FavoriteState::default();
        let key = key(FavoriteKind::Track);
        let mutation = state.begin(key.clone(), true).unwrap();
        assert_eq!(state.favorite(&key), Some(false));
        assert!(state.pending(&key));
        assert!(state.begin(key.clone(), true).is_none());
        assert!(state.complete(&mutation, Ok(())));
        assert!(!state.pending(&key));
        assert_eq!(state.favorite(&key), Some(false));
    }

    #[test]
    fn failure_rolls_back_to_previous_state() {
        let mut state = FavoriteState::default();
        let key = key(FavoriteKind::Track);
        state.set_known(key.clone(), true);
        let mutation = state.begin(key.clone(), true).unwrap();
        assert!(state.complete(&mutation, Err("request failed".into())));
        assert_eq!(state.favorite(&key), Some(true));
    }

    #[test]
    fn account_reset_rejects_stale_completion() {
        let mut state = FavoriteState::default();
        let key = key(FavoriteKind::Track);
        let mutation = state.begin(key.clone(), true).unwrap();
        state.reset_account();
        assert!(!state.complete(&mutation, Err("stale".into())));
        assert_eq!(state.favorite(&key), None);
    }

    #[test]
    fn unknown_state_is_explicit_until_seeded() {
        let mut state = FavoriteState::default();
        let key = key(FavoriteKind::Track);
        assert_eq!(state.favorite(&key), None);
        state.set_known(key.clone(), true);
        assert_eq!(state.favorite(&key), Some(true));
    }

    #[test]
    fn unknown_failure_rolls_back_to_unknown() {
        let mut state = FavoriteState::default();
        let key = key(FavoriteKind::Track);
        let mutation = state.begin(key.clone(), false).unwrap();
        assert_eq!(state.favorite(&key), Some(true));
        assert!(state.complete(&mutation, Err("request failed".into())));
        assert_eq!(state.favorite(&key), None);
    }

    #[test]
    fn provider_and_entity_kind_prevent_id_collisions() {
        let mut state = FavoriteState::default();
        let track = key(FavoriteKind::Track);
        let album = key(FavoriteKind::Album);
        let soundcloud = FavoriteKey {
            provider: Provider::SoundCloud,
            kind: FavoriteKind::Track,
            id: "42".into(),
        };
        state.set_known(track.clone(), true);
        state.set_known(album.clone(), false);
        state.set_known(soundcloud.clone(), false);
        assert_eq!(state.favorite(&track), Some(true));
        assert_eq!(state.favorite(&album), Some(false));
        assert_eq!(state.favorite(&soundcloud), Some(false));
    }

    #[test]
    fn provider_constructor_keeps_soundcloud_mutations_separate() {
        assert_eq!(
            FavoriteKey::for_provider(Provider::SoundCloud, FavoriteKind::Album, "42".into()),
            FavoriteKey::soundcloud(FavoriteKind::Album, "42".into())
        );
    }

    #[test]
    fn completed_catalog_makes_missing_entries_known_not_favorites() {
        for kind in [
            FavoriteKind::Track,
            FavoriteKind::Album,
            FavoriteKind::Artist,
            FavoriteKind::Playlist,
        ] {
            let mut state = FavoriteState::default();
            let favorite = FavoriteKey::deezer(kind, "1".into());
            let other = FavoriteKey::deezer(kind, "2".into());
            let generation = state.begin_catalog_load(Provider::Deezer, kind).unwrap();

            assert!(state.finish_catalog_load(
                Provider::Deezer,
                kind,
                generation,
                Some(vec!["1".into()])
            ));
            assert_eq!(state.favorite(&favorite), Some(true));
            assert_eq!(state.favorite(&other), Some(false));
        }
    }

    #[test]
    fn account_reset_rejects_stale_catalog_completion() {
        let mut state = FavoriteState::default();
        let generation = state
            .begin_catalog_load(Provider::Deezer, FavoriteKind::Track)
            .unwrap();
        state.reset_account();

        assert!(!state.finish_catalog_load(
            Provider::Deezer,
            FavoriteKind::Track,
            generation,
            Some(vec!["1".into()])
        ));
        assert_eq!(
            state.favorite(&FavoriteKey::deezer(FavoriteKind::Track, "1".into())),
            None
        );
    }
}
