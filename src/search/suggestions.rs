use std::{collections::HashSet, time::Duration};

pub(super) const MAX_SEARCH_HISTORY: usize = 5;
pub(super) const MAX_SUGGESTION_ROWS: usize = 10;
pub(super) const SUGGESTION_DEBOUNCE: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SuggestionKind {
    History,
    SoundCloud,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SuggestionRow {
    pub(crate) query: String,
    pub(crate) kind: SuggestionKind,
}

#[derive(Debug)]
pub(super) struct SuggestionState {
    enabled: bool,
    focused: bool,
    generation: u64,
    request_query: String,
    history: Vec<String>,
    remote: Vec<String>,
    selected: Option<usize>,
}

impl SuggestionState {
    pub(super) fn new(enabled: bool, history: Vec<String>) -> Self {
        Self {
            enabled,
            focused: false,
            generation: 0,
            request_query: String::new(),
            history: normalize_history(history),
            remote: Vec::new(),
            selected: None,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false;
        }
        self.enabled = enabled;
        self.invalidate_request();
        true
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.invalidate_request();
        }
    }

    pub(super) fn is_focused(&self) -> bool {
        self.focused
    }

    pub(super) fn begin_request(&mut self, query: &str) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.request_query = query.trim().to_owned();
        self.selected = None;
        self.generation
    }

    pub(super) fn invalidate_request(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.request_query.clear();
        self.remote.clear();
        self.selected = None;
    }

    pub(super) fn complete_request(
        &mut self,
        generation: u64,
        query: &str,
        remote: Vec<String>,
    ) -> bool {
        if generation != self.generation
            || !self.enabled
            || !self.focused
            || !self.request_query.eq_ignore_ascii_case(query.trim())
        {
            return false;
        }
        self.remote = remote;
        self.clamp_selection(query);
        true
    }

    pub(super) fn rows(&self, query: &str) -> Vec<SuggestionRow> {
        if !self.focused {
            return Vec::new();
        }
        merge_suggestions(query, &self.history, &self.remote)
    }

    pub(super) fn visible(&self, query: &str) -> bool {
        !self.rows(query).is_empty()
    }

    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(super) fn select_relative(&mut self, query: &str, direction: i32) -> bool {
        let count = self.rows(query).len();
        if count == 0 {
            self.selected = None;
            return false;
        }
        let next = match (self.selected, direction.is_negative()) {
            (None, false) => 0,
            (None, true) => count - 1,
            (Some(0), true) => count - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % count,
        };
        self.selected = Some(next);
        true
    }

    pub(super) fn selected_query(&self, query: &str) -> Option<String> {
        self.selected
            .and_then(|index| self.rows(query).get(index).map(|row| row.query.clone()))
    }

    pub(super) fn selected_history_query(&self, query: &str) -> Option<String> {
        self.selected.and_then(|index| {
            self.rows(query)
                .get(index)
                .and_then(|row| (row.kind == SuggestionKind::History).then(|| row.query.clone()))
        })
    }

    pub(super) fn record(&mut self, query: &str) -> bool {
        let previous = self.history.clone();
        record_history(&mut self.history, query);
        self.invalidate_request();
        previous != self.history
    }

    pub(super) fn remove_history(&mut self, query: &str) -> bool {
        let previous_len = self.history.len();
        self.history
            .retain(|candidate| !candidate.eq_ignore_ascii_case(query));
        self.selected = None;
        previous_len != self.history.len()
    }

    pub(super) fn history(&self) -> &[String] {
        &self.history
    }

    pub(super) fn replace_history(&mut self, history: Vec<String>) -> bool {
        let normalized = normalize_history(history);
        if self.history == normalized {
            return false;
        }
        self.history = normalized;
        self.invalidate_request();
        true
    }

    fn clamp_selection(&mut self, query: &str) {
        let count = self.rows(query).len();
        if self.selected.is_some_and(|index| index >= count) {
            self.selected = None;
        }
    }
}

fn normalize_history(history: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for query in history {
        let query = query.trim();
        if !query.is_empty() && seen.insert(query.to_lowercase()) {
            normalized.push(query.to_owned());
        }
        if normalized.len() == MAX_SEARCH_HISTORY {
            break;
        }
    }
    normalized
}

fn record_history(history: &mut Vec<String>, query: &str) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    history.retain(|candidate| !candidate.eq_ignore_ascii_case(query));
    history.insert(0, query.to_owned());
    history.truncate(MAX_SEARCH_HISTORY);
}

fn merge_suggestions(query: &str, history: &[String], remote: &[String]) -> Vec<SuggestionRow> {
    let query = query.trim();
    let query_lower = query.to_lowercase();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();

    for candidate in history {
        let trimmed = candidate.trim();
        if trimmed.is_empty()
            || (!query_lower.is_empty() && !trimmed.to_lowercase().starts_with(&query_lower))
            || !seen.insert(trimmed.to_lowercase())
        {
            continue;
        }
        rows.push(SuggestionRow {
            query: trimmed.to_owned(),
            kind: SuggestionKind::History,
        });
        if rows.len() == MAX_SUGGESTION_ROWS {
            return rows;
        }
    }

    if query.is_empty() {
        return rows;
    }
    for candidate in remote {
        let trimmed = candidate.trim();
        if trimmed.is_empty()
            || (!query_lower.is_empty() && !trimmed.to_lowercase().starts_with(&query_lower))
            || !seen.insert(trimmed.to_lowercase())
        {
            continue;
        }
        rows.push(SuggestionRow {
            query: trimmed.to_owned(),
            kind: SuggestionKind::SoundCloud,
        });
        if rows.len() == MAX_SUGGESTION_ROWS {
            break;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_trimmed_deduplicated_and_bounded() {
        let mut history = Vec::new();
        for index in 0..12 {
            record_history(&mut history, &format!(" query {index} "));
        }
        record_history(&mut history, "QUERY 4");

        assert_eq!(history.len(), MAX_SEARCH_HISTORY);
        assert_eq!(history[0], "QUERY 4");
        assert_eq!(
            history.iter().filter(|query| query.ends_with('4')).count(),
            1
        );
    }

    #[test]
    fn persisted_history_order_is_kept_and_limited_to_five() {
        let mut state = SuggestionState::new(
            true,
            ["one", "two", "three", "four", "five", "six"]
                .map(str::to_owned)
                .to_vec(),
        );
        state.set_focused(true);

        assert_eq!(
            state
                .rows("")
                .into_iter()
                .map(|row| row.query)
                .collect::<Vec<_>>(),
            ["one", "two", "three", "four", "five"]
        );
    }

    #[test]
    fn replacing_history_reuses_normalization_rules() {
        let mut state = SuggestionState::new(true, vec!["old".into()]);
        assert!(
            state.replace_history(vec![" one ".into(), "ONE".into(), "".into(), "two".into(),])
        );
        assert_eq!(state.history(), ["one", "two"]);
        assert!(!state.replace_history(vec!["one".into(), "two".into()]));
    }

    #[test]
    fn recent_history_precedes_remote_results_without_duplicates() {
        let rows = merge_suggestions(
            "skr",
            &["Skrillex".into(), "Daft Punk".into()],
            &["skrillex".into(), "Skrilla".into()],
        );

        assert_eq!(
            rows,
            vec![
                SuggestionRow {
                    query: "Skrillex".into(),
                    kind: SuggestionKind::History,
                },
                SuggestionRow {
                    query: "Skrilla".into(),
                    kind: SuggestionKind::SoundCloud,
                },
            ]
        );
    }

    #[test]
    fn empty_query_only_shows_recent_history() {
        let rows = merge_suggestions(
            "",
            &["Skrillex".into(), "Daft Punk".into()],
            &["unused".into()],
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.kind == SuggestionKind::History));
    }

    #[test]
    fn stale_or_disabled_remote_results_are_ignored() {
        let mut state = SuggestionState::new(true, Vec::new());
        state.set_focused(true);
        let stale = state.begin_request("skr");
        let current = state.begin_request("skrillex");

        assert!(!state.complete_request(stale, "skr", vec!["skrilla".into()]));
        assert!(state.complete_request(current, "skrillex", vec!["skrillex live".into()]));
        state.set_enabled(false);
        assert!(!state.complete_request(current, "skrillex", vec!["ignored".into()]));
        assert!(state.rows("skrillex").is_empty());
    }

    #[test]
    fn suggestion_debounce_matches_content_motion_and_generation_stays_scoped() {
        assert_eq!(SUGGESTION_DEBOUNCE, Duration::from_millis(220));

        let mut state = SuggestionState::new(true, Vec::new());
        state.set_focused(true);
        let stale = state.begin_request("sk");
        let current = state.begin_request("skr");

        assert!(!state.complete_request(stale, "sk", vec!["stale".into()]));
        assert!(state.complete_request(current, "skr", vec!["current".into()]));
    }

    #[test]
    fn pending_request_retains_relevant_remote_rows_and_filters_unrelated_rows() {
        let mut state = SuggestionState::new(true, Vec::new());
        state.set_focused(true);
        let first = state.begin_request("sk");
        assert!(state.complete_request(first, "sk", vec!["Skrillex".into(), "Daft Punk".into()]));

        let second = state.begin_request("skr");
        assert_eq!(
            state
                .rows("skr")
                .into_iter()
                .map(|row| row.query)
                .collect::<Vec<_>>(),
            vec!["Skrillex"]
        );
        assert!(state.complete_request(second, "skr", vec!["Skrilla".into()]));
        assert_eq!(state.rows("skr")[0].query, "Skrilla");
    }

    #[test]
    fn explicit_invalidation_clears_retained_remote_rows() {
        let mut state = SuggestionState::new(true, Vec::new());
        state.set_focused(true);
        let request = state.begin_request("sk");
        assert!(state.complete_request(request, "sk", vec!["Skrillex".into()]));
        assert!(!state.rows("sk").is_empty());

        state.invalidate_request();
        assert!(state.rows("sk").is_empty());
    }

    #[test]
    fn stale_response_cannot_replace_retained_rows_for_a_newer_generation() {
        let mut state = SuggestionState::new(true, Vec::new());
        state.set_focused(true);
        let initial = state.begin_request("sk");
        assert!(state.complete_request(initial, "sk", vec!["Skrillex".into()]));

        let stale = state.begin_request("skr");
        let current = state.begin_request("skri");
        assert!(!state.complete_request(stale, "skr", vec!["Wrong result".into()]));
        assert_eq!(state.rows("skri")[0].query, "Skrillex");
        assert!(state.complete_request(current, "skri", vec!["Skrilla".into()]));
    }

    #[test]
    fn only_selected_history_rows_can_be_removed_from_the_keyboard() {
        let mut state = SuggestionState::new(true, vec!["Skrillex".into()]);
        state.set_focused(true);
        assert!(state.select_relative("", 1));
        assert_eq!(state.selected_history_query(""), Some("Skrillex".into()));

        let request = state.begin_request("sk");
        assert!(state.complete_request(request, "sk", vec!["skream".into()]));
        assert!(state.select_relative("sk", 1));
        assert!(state.select_relative("sk", 1));
        assert_eq!(state.selected_history_query("sk"), None);
    }
}
