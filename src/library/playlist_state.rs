use std::collections::{HashMap, HashSet};

use super::playlist_client::{AddTracksResult, OwnedPlaylist};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReorderRoute {
    pub account_scope: String,
    pub provider: crate::search::Provider,
    pub action: String,
    pub playlist_id: String,
    pub page_generation: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CatalogStatus {
    Unavailable,
    Loading,
    Ready,
    Failed(String),
}

pub(crate) struct PlaylistState {
    account_scope: String,
    generation: u64,
    update_generation: u64,
    add_generation: u64,
    delete_generation: u64,
    remove_generation: u64,
    reorder_generation: u64,
    pub status: CatalogStatus,
    playlists: HashMap<String, OwnedPlaylist>,
    playlist_order: Vec<String>,
    pub updated: Option<OwnedPlaylist>,
    pub update_revision: u64,
    pub content_revision: u64,
    pub added_to: Option<String>,
    pub add_status: Option<String>,
    pub delete_pending: bool,
    pub delete_error: Option<String>,
    pub deleted: Option<String>,
    pub delete_revision: u64,
    pub remove_pending: bool,
    pub remove_error: Option<String>,
    pub remove_playlist: Option<String>,
    pub remove_track: Option<String>,
    pub remove_revision: u64,
    pub reorder_pending: bool,
    pub reorder_error: Option<String>,
    pub reorder_route: Option<ReorderRoute>,
    pub reorder_previous: Option<Vec<String>>,
    pub reorder_revision: u64,
    pub reorder_revision_route: Option<ReorderRoute>,
    pub reorder_completion_revision: u64,
    pub reorder_completion_route: Option<ReorderRoute>,
    pub reorder_completion_succeeded: Option<bool>,
    add_scope: Option<String>,
    pub create: CreateState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum CreatePhase {
    #[default]
    Idle,
    Uploading,
    Creating,
    Adding,
    Succeeded,
    Partial(String),
    Failed(String),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CreateState {
    pub phase: CreatePhase,
    pub account_scope: String,
    pub playlist_id: Option<String>,
    generation: u64,
}

impl Default for PlaylistState {
    fn default() -> Self {
        Self {
            account_scope: String::new(),
            generation: 0,
            update_generation: 0,
            add_generation: 0,
            delete_generation: 0,
            remove_generation: 0,
            reorder_generation: 0,
            status: CatalogStatus::Unavailable,
            playlists: HashMap::new(),
            playlist_order: Vec::new(),
            updated: None,
            update_revision: 0,
            content_revision: 0,
            added_to: None,
            add_status: None,
            delete_pending: false,
            delete_error: None,
            deleted: None,
            delete_revision: 0,
            remove_pending: false,
            remove_error: None,
            remove_playlist: None,
            remove_track: None,
            remove_revision: 0,
            reorder_pending: false,
            reorder_error: None,
            reorder_route: None,
            reorder_previous: None,
            reorder_revision: 0,
            reorder_revision_route: None,
            reorder_completion_revision: 0,
            reorder_completion_route: None,
            reorder_completion_succeeded: None,
            add_scope: None,
            create: CreateState {
                phase: CreatePhase::Idle,
                account_scope: String::new(),
                playlist_id: None,
                generation: 0,
            },
        }
    }
}

impl PlaylistState {
    pub(crate) fn new(account_scope: String) -> Self {
        let mut state = Self::default();
        state.set_account_scope(account_scope);
        state
    }

    pub(crate) fn set_account_scope(&mut self, scope: String) -> bool {
        if self.account_scope == scope {
            return false;
        }
        self.account_scope = scope;
        self.generation = self.generation.wrapping_add(1);
        self.update_generation = self.update_generation.wrapping_add(1);
        self.add_generation = self.add_generation.wrapping_add(1);
        self.delete_generation = self.delete_generation.wrapping_add(1);
        self.remove_generation = self.remove_generation.wrapping_add(1);
        self.reorder_generation = self.reorder_generation.wrapping_add(1);
        self.status = CatalogStatus::Unavailable;
        self.playlists.clear();
        self.playlist_order.clear();
        self.updated = None;
        self.added_to = None;
        self.add_status = None;
        self.delete_pending = false;
        self.delete_error = None;
        self.remove_pending = false;
        self.remove_error = None;
        self.remove_playlist = None;
        self.remove_track = None;
        self.reorder_pending = false;
        self.reorder_error = None;
        self.reorder_route = None;
        self.reorder_previous = None;
        self.reorder_completion_route = None;
        self.reorder_completion_succeeded = None;
        self.deleted = None;
        self.add_scope = None;
        self.create = CreateState {
            phase: CreatePhase::Idle,
            account_scope: self.account_scope.clone(),
            playlist_id: None,
            generation: 0,
        };
        true
    }

    pub(crate) fn begin_load(&mut self, scope: &str, force: bool) -> Option<u64> {
        if scope != self.account_scope
            || (!force && matches!(self.status, CatalogStatus::Loading | CatalogStatus::Ready))
        {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.status = CatalogStatus::Loading;
        Some(self.generation)
    }

    pub(crate) fn complete_load(
        &mut self,
        scope: &str,
        generation: u64,
        result: Result<Vec<OwnedPlaylist>, String>,
    ) -> bool {
        if scope != self.account_scope || generation != self.generation {
            return false;
        }
        match result {
            Ok(playlists) => {
                self.playlist_order = playlists
                    .iter()
                    .map(|playlist| playlist.id.clone())
                    .collect();
                self.playlists = playlists
                    .into_iter()
                    .map(|playlist| (playlist.id.clone(), playlist))
                    .collect();
                self.status = CatalogStatus::Ready;
            }
            Err(error) => {
                self.playlists.clear();
                self.playlist_order.clear();
                self.status = CatalogStatus::Failed(error);
            }
        }
        true
    }

    pub(crate) fn is_editable(&self, id: &str) -> bool {
        matches!(self.status, CatalogStatus::Ready)
            && self.playlists.get(id).is_some_and(|p| p.editable())
    }

    pub(crate) fn editable(&self, id: &str) -> Option<OwnedPlaylist> {
        matches!(self.status, CatalogStatus::Ready)
            .then(|| self.playlists.get(id))
            .flatten()
            .filter(|playlist| playlist.editable())
            .cloned()
    }

    pub(crate) fn editable_playlists(&self) -> Vec<OwnedPlaylist> {
        if !matches!(self.status, CatalogStatus::Ready) {
            return Vec::new();
        }
        let mut playlists: Vec<_> = self
            .playlist_order
            .iter()
            .filter_map(|id| self.playlists.get(id))
            .filter(|playlist| playlist.editable())
            .cloned()
            .collect();
        let order_set: HashSet<&str> = self.playlist_order.iter().map(String::as_str).collect();
        let mut fallback: Vec<(String, &OwnedPlaylist)> = self
            .playlists
            .iter()
            .filter(|(id, playlist)| !order_set.contains(id.as_str()) && playlist.editable())
            .map(|(_, playlist)| (playlist.title.to_lowercase(), playlist))
            .collect();
        fallback.sort_by(|(left_title, left), (right_title, right)| {
            left_title
                .cmp(right_title)
                .then_with(|| left.id.cmp(&right.id))
        });
        playlists.extend(fallback.into_iter().map(|(_, playlist)| playlist.clone()));
        playlists
    }

    pub(crate) fn begin_update(&mut self, scope: &str) -> Option<u64> {
        if scope != self.account_scope {
            return None;
        }
        self.update_generation = self.update_generation.wrapping_add(1);
        Some(self.update_generation)
    }

    pub(crate) fn begin_delete(&mut self, scope: &str, id: &str) -> Option<u64> {
        if scope != self.account_scope || !self.is_editable(id) || self.delete_pending {
            return None;
        }
        self.delete_generation = self.delete_generation.wrapping_add(1);
        self.delete_pending = true;
        self.delete_error = None;
        Some(self.delete_generation)
    }

    pub(crate) fn begin_remove(
        &mut self,
        scope: &str,
        playlist: &str,
        track: &str,
        proven: bool,
    ) -> Option<u64> {
        if scope != self.account_scope
            || !proven
            || !self.is_editable(playlist)
            || self.remove_pending
            || super::playlist_client::valid_id(track).is_err()
        {
            return None;
        }
        self.remove_generation = self.remove_generation.wrapping_add(1);
        self.remove_pending = true;
        self.remove_error = None;
        self.remove_playlist = Some(playlist.into());
        self.remove_track = Some(track.into());
        Some(self.remove_generation)
    }

    pub(crate) fn begin_reorder(
        &mut self,
        scope: &str,
        route: ReorderRoute,
        previous: Vec<String>,
        submitted: &[String],
    ) -> Option<u64> {
        if scope != self.account_scope
            || route.account_scope != self.account_scope
            || !matches!(
                route.provider,
                crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
            )
            || route.action != "playlistTracks"
            || !self.is_editable(&route.playlist_id)
            || self.reorder_pending
            || previous.len() < 2
            || previous.len() != submitted.len()
            || !unique_reorder_ids(&previous)
            || !unique_reorder_ids(submitted)
            || previous.iter().collect::<HashSet<_>>() != submitted.iter().collect::<HashSet<_>>()
        {
            return None;
        }
        self.reorder_generation = self.reorder_generation.wrapping_add(1);
        self.reorder_pending = true;
        self.reorder_error = None;
        self.reorder_route = Some(route);
        self.reorder_previous = Some(previous);
        Some(self.reorder_generation)
    }

    pub(crate) fn complete_reorder(
        &mut self,
        scope: &str,
        generation: u64,
        route: &ReorderRoute,
        result: &Result<bool, String>,
    ) -> bool {
        if scope != self.account_scope
            || self.reorder_route.as_ref() != Some(route)
            || generation != self.reorder_generation
        {
            return false;
        }
        self.reorder_pending = false;
        self.reorder_completion_revision = self.reorder_completion_revision.wrapping_add(1);
        self.reorder_completion_route = Some(route.clone());
        self.reorder_completion_succeeded = Some(*result == Ok(true));
        if let Err(error) = result {
            self.reorder_error = Some(error.clone());
        } else if *result == Ok(true) {
            self.reorder_revision = self.reorder_revision.wrapping_add(1);
            self.reorder_revision_route = Some(route.clone());
            self.reorder_previous = None;
        } else {
            self.reorder_error = Some(
                match route.provider {
                    crate::search::Provider::Deezer => "Deezer did not save the playlist order",
                    crate::search::Provider::SoundCloud => {
                        "SoundCloud did not save the playlist order"
                    }
                }
                .into(),
            );
        }
        true
    }

    pub(crate) fn take_reorder_previous(&mut self, route: &ReorderRoute) -> Option<Vec<String>> {
        (self.reorder_route.as_ref() == Some(route) && !self.reorder_pending)
            .then(|| self.reorder_previous.take())
            .flatten()
    }

    pub(crate) fn clear_remove_feedback(&mut self) {
        self.remove_generation = self.remove_generation.wrapping_add(1);
        self.remove_pending = false;
        self.remove_error = None;
        self.remove_playlist = None;
        self.remove_track = None;
    }

    pub(crate) fn complete_remove(
        &mut self,
        scope: &str,
        generation: u64,
        result: &Result<bool, String>,
    ) -> bool {
        if scope != self.account_scope || generation != self.remove_generation {
            return false;
        }
        self.remove_pending = false;
        if let Err(error) = result {
            self.remove_error = Some(error.clone());
        } else {
            self.remove_revision = self.remove_revision.wrapping_add(1);
        }
        true
    }

    pub(crate) fn remove_status_for(
        &self,
        provider: crate::search::Provider,
        action: &str,
        playlist: &str,
    ) -> Option<(&'static str, String)> {
        if provider != crate::search::Provider::Deezer
            || action != "playlistTracks"
            || self.remove_playlist.as_deref() != Some(playlist)
        {
            return None;
        }
        if self.remove_pending {
            return None;
        }
        self.remove_error.clone().map(|error| ("error", error))
    }

    pub(crate) fn complete_delete(
        &mut self,
        scope: &str,
        generation: u64,
        result: &Result<bool, String>,
    ) -> bool {
        if scope != self.account_scope || generation != self.delete_generation {
            return false;
        }
        self.delete_pending = false;
        if let Err(error) = result {
            self.delete_error = Some(error.clone());
        } else {
            self.delete_revision = self.delete_revision.wrapping_add(1);
        }
        true
    }

    pub(crate) fn invalidate_catalog(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.status = CatalogStatus::Unavailable;
        self.playlists.clear();
        self.playlist_order.clear();
    }

    pub(crate) fn note_deleted(&mut self, id: String) {
        self.deleted = Some(id);
    }

    pub(crate) fn begin_add(&mut self, scope: &str) -> Option<u64> {
        if scope != self.account_scope {
            return None;
        }
        self.add_generation = self.add_generation.wrapping_add(1);
        self.added_to = None;
        self.add_status = None;
        Some(self.add_generation)
    }

    pub(crate) fn set_add_scope(&mut self, scope: String) {
        self.add_scope = Some(scope);
        self.added_to = None;
        self.add_status = None;
    }

    #[allow(dead_code)]
    pub(crate) fn add_status_for(&self, scope: &str) -> Option<String> {
        (self.add_scope.as_deref() == Some(scope))
            .then(|| self.add_status.clone())
            .flatten()
    }

    pub(crate) fn complete_add(
        &mut self,
        scope: &str,
        generation: u64,
        playlist_id: &str,
        result: &Result<AddTracksResult, String>,
    ) -> bool {
        if scope != self.account_scope || generation != self.add_generation {
            return false;
        }
        if let Ok(result) = result {
            self.added_to = Some(playlist_id.to_owned());
            self.content_revision = self.content_revision.wrapping_add(1);
            self.add_status = Some(result.status_message());
        }
        true
    }

    pub(crate) fn note_track_added(&mut self, playlist_id: &str, count: usize) -> bool {
        let Some(entry) = self.playlists.get_mut(playlist_id) else {
            return false;
        };
        if let Some(existing) = entry.track_count {
            entry.track_count = Some(existing.saturating_add(count as u64));
        }
        true
    }

    pub(crate) fn complete_update(
        &mut self,
        scope: &str,
        generation: u64,
        result: &Result<OwnedPlaylist, String>,
    ) -> bool {
        if scope != self.account_scope || generation != self.update_generation {
            return false;
        }
        if let Ok(playlist) = result {
            self.playlists.insert(playlist.id.clone(), playlist.clone());
            self.updated = Some(playlist.clone());
            self.update_revision = self.update_revision.wrapping_add(1);
        }
        true
    }

    pub(crate) fn begin_create(&mut self, scope: &str) -> Option<u64> {
        if scope != self.account_scope
            || !matches!(
                self.create.phase,
                CreatePhase::Idle
                    | CreatePhase::Succeeded
                    | CreatePhase::Partial(_)
                    | CreatePhase::Failed(_)
            )
        {
            return None;
        }
        self.create.generation = self.create.generation.wrapping_add(1);
        self.create.account_scope = scope.into();
        self.create.playlist_id = None;
        self.create.phase = CreatePhase::Uploading;
        Some(self.create.generation)
    }

    pub(crate) fn create_phase_for(&self, scope: &str, generation: u64) -> Option<CreatePhase> {
        (scope == self.account_scope
            && self.create.account_scope == scope
            && self.create.generation == generation)
            .then(|| self.create.phase.clone())
    }

    pub(crate) fn create_phase(
        &mut self,
        scope: &str,
        generation: u64,
        phase: CreatePhase,
    ) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
        {
            return false;
        }
        self.create.phase = phase;
        true
    }

    pub(crate) fn complete_create(
        &mut self,
        scope: &str,
        generation: u64,
        id: Result<String, String>,
    ) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
        {
            return false;
        }
        match id {
            Ok(id) => {
                self.create.playlist_id = Some(id);
                self.create.phase = CreatePhase::Succeeded;
            }
            Err(error) => self.create.phase = CreatePhase::Failed(error),
        }
        true
    }

    pub(crate) fn partial_create(&mut self, scope: &str, generation: u64, error: String) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
        {
            return false;
        }
        self.create.phase = CreatePhase::Partial(error);
        true
    }

    pub(crate) fn begin_initial_add(
        &mut self,
        scope: &str,
        generation: u64,
        playlist_id: &str,
    ) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
            || !matches!(self.create.phase, CreatePhase::Creating)
        {
            return false;
        }
        self.create.playlist_id = Some(playlist_id.into());
        self.create.phase = CreatePhase::Adding;
        true
    }

    pub(crate) fn begin_create_add_retry(
        &mut self,
        scope: &str,
        generation: u64,
        playlist_id: &str,
    ) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
            || !matches!(self.create.phase, CreatePhase::Partial(_))
            || self.create.playlist_id.as_deref() != Some(playlist_id)
        {
            return false;
        }
        self.create.phase = CreatePhase::Adding;
        true
    }

    pub(crate) fn complete_create_add_retry(
        &mut self,
        scope: &str,
        generation: u64,
        result: Result<AddTracksResult, String>,
    ) -> bool {
        if scope != self.account_scope
            || self.create.account_scope != scope
            || self.create.generation != generation
            || self.create.playlist_id.is_none()
        {
            return false;
        }
        self.create.phase = match result {
            Ok(_) => CreatePhase::Succeeded,
            Err(error) => CreatePhase::Partial(format!(
                "The initial track could not be added: {error}. The playlist was not rolled back."
            )),
        };
        true
    }
}

pub(crate) fn matching_playlist_route(
    provider: crate::search::Provider,
    action: &str,
    route_id: &str,
    playlist_id: &str,
) -> bool {
    matches!(
        provider,
        crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
    ) && action == "playlistTracks"
        && route_id == playlist_id
}

fn unique_reorder_ids(ids: &[String]) -> bool {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().all(|id| {
        let Ok(id) = super::playlist_client::valid_id(id) else {
            return false;
        };
        let Ok(id) = id.parse::<u64>() else {
            return false;
        };
        id > 0 && seen.insert(id)
    })
}

pub(crate) fn updated_playlist_subtitle(previous: &str, owner_name: &str) -> String {
    let owner_name = owner_name.trim();
    if owner_name.is_empty() {
        previous.to_owned()
    } else {
        owner_name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::playlist_client::{OwnedPlaylist, PlaylistOwner};
    use crate::search::Provider;

    #[test]
    fn stale_account_and_update_completions_are_rejected() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        let update = state.begin_update("one").unwrap();
        let add = state.begin_add("one").unwrap();
        state.set_account_scope("two".into());
        assert!(!state.complete_load("one", load, Ok(vec![playlist()])));
        assert!(!state.complete_update("one", update, &Ok(playlist())));
        assert!(!state.complete_add(
            "one",
            add,
            "42",
            &Ok(super::super::playlist_client::AddTracksResult::Added { count: 1 })
        ));
        assert!(state.editable("42").is_none());
    }

    #[test]
    fn updated_playlist_subtitle_prefers_trimmed_owner_and_preserves_fallback() {
        assert_eq!(
            updated_playlist_subtitle("Old owner", "  New owner  "),
            "New owner"
        );
        assert_eq!(updated_playlist_subtitle("Old owner", "  "), "Old owner");
    }

    #[test]
    fn editable_catalog_preserves_query_order() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        assert!(state.editable_playlists().is_empty());
        let generation = state.begin_load("one", false).unwrap();
        let mut second = playlist();
        second.id = "9".into();
        second.title = "alpha".into();
        let mut blocked = playlist();
        blocked.id = "8".into();
        blocked.title = "Beta".into();
        blocked.is_collaborative = true;
        state.complete_load("one", generation, Ok(vec![playlist(), second, blocked]));
        assert_eq!(
            state
                .editable_playlists()
                .into_iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            ["42", "9"]
        );
    }

    #[test]
    fn opening_a_new_add_attempt_clears_previous_feedback() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let generation = state.begin_add("one").unwrap();
        state.complete_add(
            "one",
            generation,
            "42",
            &Ok(super::super::playlist_client::AddTracksResult::Added { count: 1 }),
        );
        state.begin_add("one");
        assert!(state.add_status.is_none());
        assert!(state.added_to.is_none());
    }

    #[test]
    fn add_feedback_is_visible_only_in_its_origin_scope() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        state.set_add_scope("search:one".into());
        let generation = state.begin_add("one").unwrap();
        state.complete_add(
            "one",
            generation,
            "42",
            &Ok(super::super::playlist_client::AddTracksResult::AlreadyPresent { count: 1 }),
        );
        assert!(state.add_status_for("search:one").is_some());
        assert!(state.add_status_for("search:two").is_none());
        state.set_add_scope("detail:42".into());
        assert!(state.add_status_for("search:one").is_none());
    }

    #[test]
    fn stale_completion_cannot_replace_a_reopened_editor_generation() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let stale = state.begin_update("one").unwrap();
        let current = state.begin_update("one").unwrap();

        assert!(!state.complete_update("one", stale, &Ok(playlist())));
        assert!(state.updated.is_none());
        assert!(state.complete_update("one", current, &Ok(playlist())));
        assert_eq!(state.updated.as_ref().unwrap().id, "42");
    }

    #[test]
    fn exact_route_refresh_eligibility_is_required() {
        use crate::search::Provider;
        assert!(matching_playlist_route(
            Provider::Deezer,
            "playlistTracks",
            "42",
            "42"
        ));
        assert!(matching_playlist_route(
            Provider::SoundCloud,
            "playlistTracks",
            "42",
            "42"
        ));
        assert!(!matching_playlist_route(
            Provider::Deezer,
            "playlistTracks",
            "7",
            "42"
        ));
        assert!(!matching_playlist_route(
            Provider::Deezer,
            "albumTracks",
            "42",
            "42"
        ));
    }

    #[test]
    fn delete_route_requires_authoritative_editable_catalog_entry() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        assert!(state.begin_delete("one", "42").is_none());
        let load = state.begin_load("one", false).unwrap();
        let mut blocked = playlist();
        blocked.is_collaborative = true;
        state.complete_load("one", load, Ok(vec![blocked]));
        assert!(state.begin_delete("one", "42").is_none());
    }

    #[test]
    fn accepted_add_records_truthful_status_and_exact_destination() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let generation = state.begin_add("one").unwrap();
        assert!(state.complete_add(
            "one",
            generation,
            "42",
            &Ok(super::super::playlist_client::AddTracksResult::AlreadyPresent { count: 1 })
        ));
        assert_eq!(state.added_to.as_deref(), Some("42"));
        assert_eq!(
            state.add_status.as_deref(),
            Some("Track was already in that playlist.")
        );
        assert_eq!(state.content_revision, 1);
    }

    #[test]
    fn delete_pending_and_backend_errors_are_account_scoped() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let generation = state.begin_delete("one", "42").unwrap();
        assert!(state.delete_pending);
        assert!(state.begin_delete("one", "42").is_none());
        assert!(state.complete_delete("one", generation, &Err("backend".into())));
        assert!(!state.delete_pending);
        assert_eq!(state.delete_error.as_deref(), Some("backend"));
    }

    #[test]
    fn stale_delete_completion_is_rejected_after_account_change() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let generation = state.begin_delete("one", "42").unwrap();
        state.set_account_scope("two".into());
        assert!(!state.complete_delete("one", generation, &Ok(true)));
        assert!(!state.delete_pending);
    }

    #[test]
    fn removal_transitions_pending_failure_success_and_stale_completion() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let pending = state.begin_remove("one", "42", "7", true).unwrap();
        assert!(state.remove_pending);
        assert!(
            state
                .remove_status_for(Provider::Deezer, "playlistTracks", "42")
                .is_none()
        );
        assert!(state.complete_remove("one", pending, &Err("failed".into())));
        assert_eq!(state.remove_error.as_deref(), Some("failed"));
        let success = state.begin_remove("one", "42", "7", true).unwrap();
        assert!(state.complete_remove("one", success, &Ok(true)));
        assert_eq!(state.remove_revision, 1);
        let stale = state.begin_remove("one", "42", "7", true).unwrap();
        let current = state.begin_remove("one", "42", "8", true);
        assert!(current.is_none());
        assert!(!state.complete_remove("two", stale, &Ok(true)));
    }

    #[test]
    fn removal_stays_silent_without_a_red_banner() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let pending = state.begin_remove("one", "42", "7", true).unwrap();
        assert!(
            state
                .remove_status_for(Provider::Deezer, "playlistTracks", "42")
                .is_none()
        );
        assert!(state.complete_remove("one", pending, &Ok(true)));
        assert!(
            state
                .remove_status_for(Provider::Deezer, "playlistTracks", "42")
                .is_none()
        );
        let source = include_str!("playlist_state.rs");
        let production = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(!production.contains("Removing track from playlist"));
        let content = include_str!("content_view.rs");
        let content_production = &content[..content.find("#[cfg(test)]").unwrap()];
        assert!(!content_production.contains("Removing track from playlist"));
        assert!(!content_production.contains("status:remove:"));
        let detail = include_str!("../search/detail_view.rs");
        assert!(!detail.contains("Removing track from playlist"));
    }

    #[test]
    fn removal_eligibility_requires_deezer_playlist_route_editable_catalog_and_valid_id() {
        use crate::search::Provider;
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        assert!(state.begin_remove("one", "42", "7", true).is_some());
        state.clear_remove_feedback();
        assert!(state.begin_remove("one", "42", "bad", true).is_none());
        assert!(
            state
                .remove_status_for(Provider::SoundCloud, "playlistTracks", "42")
                .is_none()
        );
        assert!(
            state
                .remove_status_for(Provider::Deezer, "albumTracks", "42")
                .is_none()
        );
    }

    #[test]
    fn reorder_rolls_back_failure_and_increments_revision_only_on_success() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let generation = state
            .begin_reorder(
                "one",
                ReorderRoute {
                    account_scope: "one".into(),
                    provider: Provider::Deezer,
                    action: "playlistTracks".into(),
                    playlist_id: "42".into(),
                    page_generation: Some(1),
                },
                vec!["1".into(), "2".into()],
                &["2".into(), "1".into()],
            )
            .unwrap();
        let route = state.reorder_route.clone().unwrap();
        assert!(state.complete_reorder("one", generation, &route, &Err("failed".into())));
        assert_eq!(state.reorder_completion_revision, 1);
        assert_eq!(state.reorder_completion_route.as_ref(), Some(&route));
        assert_eq!(state.reorder_completion_succeeded, Some(false));
        assert_eq!(
            state.take_reorder_previous(&route),
            Some(vec!["1".into(), "2".into()])
        );
        assert_eq!(state.reorder_revision, 0);
        let generation = state
            .begin_reorder(
                "one",
                route.clone(),
                vec!["1".into(), "2".into()],
                &["2".into(), "1".into()],
            )
            .unwrap();
        assert!(state.complete_reorder("one", generation, &route, &Ok(true)));
        assert_eq!(state.reorder_revision, 1);
        assert_eq!(state.reorder_completion_revision, 2);
        assert_eq!(state.reorder_completion_succeeded, Some(true));
        assert!(state.take_reorder_previous(&route).is_none());
    }

    #[test]
    fn soundcloud_reorder_accepts_playlist_routes_and_uses_neutral_failure_copy() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let route = ReorderRoute {
            account_scope: "one".into(),
            provider: Provider::SoundCloud,
            action: "playlistTracks".into(),
            playlist_id: "42".into(),
            page_generation: Some(1),
        };
        let generation = state
            .begin_reorder(
                "one",
                route.clone(),
                vec!["1".into(), "2".into()],
                &["2".into(), "1".into()],
            )
            .unwrap();
        assert!(state.complete_reorder("one", generation, &route, &Ok(false)));
        assert_eq!(
            state.reorder_error.as_deref(),
            Some("SoundCloud did not save the playlist order")
        );
        assert!(
            state
                .begin_reorder(
                    "one",
                    route,
                    vec!["1".into(), "1".into()],
                    &["1".into(), "1".into()]
                )
                .is_none()
        );
        assert!(
            state
                .begin_reorder(
                    "one",
                    ReorderRoute {
                        account_scope: "one".into(),
                        provider: Provider::SoundCloud,
                        action: "playlistTracks".into(),
                        playlist_id: "42".into(),
                        page_generation: Some(1),
                    },
                    vec!["1".into(), "2".into()],
                    &["2".into(), "3".into()]
                )
                .is_none()
        );
    }

    #[test]
    fn successful_reorder_allows_an_immediate_second_reorder_for_both_providers() {
        for provider in [Provider::Deezer, Provider::SoundCloud] {
            let mut state = PlaylistState::default();
            state.set_account_scope("one".into());
            let load = state.begin_load("one", false).unwrap();
            state.complete_load("one", load, Ok(vec![playlist()]));
            let route = ReorderRoute {
                account_scope: "one".into(),
                provider,
                action: "playlistTracks".into(),
                playlist_id: "42".into(),
                page_generation: None,
            };
            let first = state
                .begin_reorder(
                    "one",
                    route.clone(),
                    vec!["1".into(), "2".into()],
                    &["2".into(), "1".into()],
                )
                .unwrap();
            assert!(state.complete_reorder("one", first, &route, &Ok(true)));
            let second = state
                .begin_reorder(
                    "one",
                    route.clone(),
                    vec!["2".into(), "1".into()],
                    &["1".into(), "2".into()],
                )
                .unwrap();
            assert!(state.complete_reorder("one", second, &route, &Ok(true)));
            assert_eq!(state.reorder_revision, 2);
        }
    }

    #[test]
    fn stale_reorder_completion_does_not_clear_current_pending_state() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        state.complete_load("one", load, Ok(vec![playlist()]));
        let stale = state
            .begin_reorder(
                "one",
                ReorderRoute {
                    account_scope: "one".into(),
                    provider: Provider::Deezer,
                    action: "playlistTracks".into(),
                    playlist_id: "42".into(),
                    page_generation: Some(1),
                },
                vec!["1".into(), "2".into()],
                &["2".into(), "1".into()],
            )
            .unwrap();
        let route = state.reorder_route.clone().unwrap();
        assert!(state.set_account_scope("two".into()));
        assert!(!state.complete_reorder("one", stale, &route, &Ok(true)));
        assert!(!state.reorder_pending);
        assert_eq!(state.reorder_revision, 0);
        assert_eq!(state.reorder_completion_revision, 0);
    }

    #[test]
    fn create_phases_reject_duplicates_and_stale_accounts_and_retain_partial_id() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let generation = state.begin_create("one").unwrap();
        assert!(state.begin_create("one").is_none());
        assert!(state.create_phase("one", generation, CreatePhase::Creating));
        assert!(state.complete_create("one", generation, Ok("42".into())));
        assert!(state.create_phase("one", generation, CreatePhase::Adding));
        assert!(state.partial_create("one", generation, "track failed".into()));
        assert_eq!(state.create.playlist_id.as_deref(), Some("42"));
        assert_eq!(
            state.create.phase,
            CreatePhase::Partial("track failed".into())
        );
        state.set_account_scope("two".into());
        assert!(!state.complete_create("one", generation, Ok("7".into())));
    }

    #[test]
    fn completed_create_can_begin_a_fresh_operation_without_retaining_id() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let generation = state.begin_create("one").unwrap();
        state.complete_create("one", generation, Ok("42".into()));
        let next = state.begin_create("one").unwrap();
        assert_ne!(next, generation);
        assert_eq!(state.create.phase, CreatePhase::Uploading);
        assert!(state.create.playlist_id.is_none());
    }

    #[test]
    fn startup_scope_can_create_before_the_playlist_catalog_loads() {
        let mut state = PlaylistState::new("one".into());
        assert!(state.begin_create("one").is_some());
        assert_eq!(state.create.phase, CreatePhase::Uploading);
    }

    #[test]
    fn partial_retry_reuses_the_created_id_and_does_not_change_generation() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let generation = state.begin_create("one").unwrap();
        state.complete_create("one", generation, Ok("42".into()));
        state.partial_create("one", generation, "failed".into());
        assert!(state.begin_create_add_retry("one", generation, "42"));
        assert_eq!(state.create.playlist_id.as_deref(), Some("42"));
        assert!(state.complete_create_add_retry(
            "one",
            generation,
            Ok(super::super::playlist_client::AddTracksResult::Added { count: 1 })
        ));
        assert_eq!(state.create.phase, CreatePhase::Succeeded);
    }

    #[test]
    fn initial_track_add_records_the_created_id_before_the_request_runs() {
        let mut state = PlaylistState::new("one".into());
        let generation = state.begin_create("one").unwrap();
        assert!(state.create_phase("one", generation, CreatePhase::Creating));
        assert!(state.begin_initial_add("one", generation, "42"));
        assert_eq!(state.create.playlist_id.as_deref(), Some("42"));
        assert_eq!(state.create.phase, CreatePhase::Adding);
        assert!(!state.begin_initial_add("one", generation, "42"));
    }

    #[test]
    fn track_count_bump_updates_cached_entry_without_refetch() {
        let mut state = PlaylistState::default();
        state.set_account_scope("one".into());
        let load = state.begin_load("one", false).unwrap();
        let mut counted = playlist();
        counted.track_count = Some(7);
        let mut uncounted = playlist();
        uncounted.id = "9".into();
        state.complete_load("one", load, Ok(vec![counted, uncounted]));
        assert!(state.note_track_added("42", 1));
        assert_eq!(state.editable("42").unwrap().track_count, Some(8));
        assert!(state.note_track_added("42", 3));
        assert_eq!(state.editable("42").unwrap().track_count, Some(11));
        assert!(state.note_track_added("9", 2));
        assert_eq!(state.editable("9").unwrap().track_count, None);
        assert!(!state.note_track_added("missing", 1));
    }

    fn playlist() -> OwnedPlaylist {
        OwnedPlaylist {
            id: "42".into(),
            title: "Title".into(),
            description: String::new(),
            is_private: false,
            is_from_favorite_tracks: false,
            is_collaborative: false,
            owner: PlaylistOwner {
                id: "7".into(),
                name: "Owner".into(),
            },
            artwork: String::new(),
            track_count: None,
        }
    }
}
