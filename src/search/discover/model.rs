use std::collections::{HashMap, VecDeque};

use super::deezer::{MAX_SMART_MIX_ENRICHMENT_IDS, valid_smart_mix_id};
use crate::search::models::{Card, Provider};
use crate::smart_mix_title::specific_smart_mix_title;

use super::title_cache::SmartTitleCache;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiscoverAction {
    OpenDetail,
    PlayDeezerTrack(String),
    PlayDeezerFlow { smart_mix: bool },
    OpenDeezerChannel(String),
    OpenSoundCloudSelection(Vec<String>),
    None,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoverItem {
    pub(crate) card: Card,
    pub(crate) action: DiscoverAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoverSection {
    pub(crate) id: String,
    pub(crate) provider: Provider,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) items: Vec<DiscoverItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiscoverStatus {
    Idle,
    Loading,
    Ready,
    AccountRequired,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiscoverChannelStatus {
    Closed,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoverChannelState {
    pub(crate) slug: String,
    pub(crate) title: String,
    pub(crate) status: DiscoverChannelStatus,
    pub(crate) sections: Vec<DiscoverSection>,
    generation: u64,
}

#[derive(Clone, Debug)]
struct CachedChannel {
    title: String,
    sections: Vec<DiscoverSection>,
}

const MAX_CACHED_CHANNELS: usize = 8;

impl Default for DiscoverChannelState {
    fn default() -> Self {
        Self {
            slug: String::new(),
            title: String::new(),
            status: DiscoverChannelStatus::Closed,
            sections: Vec::new(),
            generation: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiscoverProviderState {
    pub(crate) status: DiscoverStatus,
    pub(crate) sections: Vec<DiscoverSection>,
    generation: u64,
}

impl Default for DiscoverProviderState {
    fn default() -> Self {
        Self {
            status: DiscoverStatus::Idle,
            sections: Vec::new(),
            generation: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoverState {
    account_scope: String,
    next_generation: u64,
    deezer: DiscoverProviderState,
    soundcloud: DiscoverProviderState,
    channel: DiscoverChannelState,
    channel_cache: HashMap<String, CachedChannel>,
    channel_cache_order: VecDeque<String>,
    smart_titles: SmartTitleCache,
}

impl DiscoverState {
    pub(crate) fn new(account_scope: String) -> Self {
        let smart_titles = SmartTitleCache::load(&account_scope);
        Self {
            account_scope,
            next_generation: 0,
            deezer: DiscoverProviderState::default(),
            soundcloud: DiscoverProviderState::default(),
            channel: DiscoverChannelState::default(),
            channel_cache: HashMap::new(),
            channel_cache_order: VecDeque::new(),
            smart_titles,
        }
    }

    pub(crate) fn provider(&self, provider: Provider) -> &DiscoverProviderState {
        match provider {
            Provider::Deezer => &self.deezer,
            Provider::SoundCloud => &self.soundcloud,
        }
    }

    pub(crate) fn ready_generation(&self, provider: Provider) -> Option<u64> {
        matches!(self.provider(provider).status, DiscoverStatus::Ready)
            .then_some(self.provider(provider).generation)
    }

    pub(crate) fn reset_account_scope(&mut self, account_scope: String) {
        if self.account_scope == account_scope {
            return;
        }
        self.account_scope = account_scope;
        self.next_generation = self.next_generation.wrapping_add(1);
        self.deezer = DiscoverProviderState::default();
        self.soundcloud = DiscoverProviderState::default();
        self.channel = DiscoverChannelState::default();
        self.channel_cache.clear();
        self.channel_cache_order.clear();
        self.smart_titles = SmartTitleCache::load(&self.account_scope);
    }

    pub(crate) fn channel_open(&self) -> bool {
        !matches!(self.channel.status, DiscoverChannelStatus::Closed)
    }

    pub(crate) fn channel(&self) -> &DiscoverChannelState {
        &self.channel
    }

    #[cfg(test)]
    pub(crate) fn start_channel(&mut self, slug: String) -> Option<(u64, String)> {
        self.start_channel_with_title(slug.clone(), slug)
    }

    pub(crate) fn start_channel_with_title(
        &mut self,
        slug: String,
        title: String,
    ) -> Option<(u64, String)> {
        if slug.trim().is_empty() {
            return None;
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.channel = DiscoverChannelState {
            title,
            slug,
            status: DiscoverChannelStatus::Loading,
            sections: Vec::new(),
            generation,
        };
        Some((generation, self.account_scope.clone()))
    }

    pub(crate) fn complete_channel(
        &mut self,
        generation: u64,
        account_scope: &str,
        slug: &str,
        result: Result<(String, Vec<DiscoverSection>), String>,
    ) -> bool {
        if self.account_scope != account_scope
            || self.channel.generation != generation
            || self.channel.slug != slug
            || !matches!(self.channel.status, DiscoverChannelStatus::Loading)
        {
            return false;
        }
        match result {
            Ok((title, mut sections)) => {
                self.smart_titles.apply(&mut sections);
                if !title.trim().is_empty() {
                    self.channel.title = title;
                }
                self.channel.sections = sections;
                self.channel.status = DiscoverChannelStatus::Ready;
                self.cache_channel();
            }
            Err(error) => {
                self.channel.sections.clear();
                self.channel.status = DiscoverChannelStatus::Failed(error);
            }
        }
        true
    }

    pub(crate) fn complete_channel_from_cache(
        &mut self,
        generation: u64,
        account_scope: &str,
        slug: &str,
    ) -> bool {
        if self.account_scope != account_scope
            || self.channel.generation != generation
            || self.channel.slug != slug
            || !matches!(self.channel.status, DiscoverChannelStatus::Loading)
        {
            return false;
        }
        let Some(mut cached) = self.channel_cache.get(slug).cloned() else {
            return false;
        };
        self.smart_titles.apply(&mut cached.sections);
        self.touch_channel_cache(slug);
        self.channel.title = cached.title;
        self.channel.sections = cached.sections;
        self.channel.status = DiscoverChannelStatus::Ready;
        true
    }

    pub(crate) fn retry_channel(&mut self) -> Option<(u64, String, String)> {
        if !matches!(self.channel.status, DiscoverChannelStatus::Failed(_)) {
            return None;
        }
        let slug = self.channel.slug.clone();
        let title = self.channel.title.clone();
        let (generation, account_scope) = self.start_channel_with_title(slug.clone(), title)?;
        Some((generation, account_scope, slug))
    }

    pub(crate) fn close_channel(&mut self) -> bool {
        if !self.channel_open() {
            return false;
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        self.channel = DiscoverChannelState::default();
        true
    }

    fn cache_channel(&mut self) {
        let slug = self.channel.slug.clone();
        self.channel_cache.insert(
            slug.clone(),
            CachedChannel {
                title: self.channel.title.clone(),
                sections: self.channel.sections.clone(),
            },
        );
        self.touch_channel_cache(&slug);
        while self.channel_cache_order.len() > MAX_CACHED_CHANNELS {
            if let Some(oldest) = self.channel_cache_order.pop_front() {
                self.channel_cache.remove(&oldest);
            }
        }
    }

    fn touch_channel_cache(&mut self, slug: &str) {
        self.channel_cache_order
            .retain(|candidate| candidate != slug);
        self.channel_cache_order.push_back(slug.to_owned());
    }

    pub(crate) fn mark_account_required(&mut self, provider: Provider) {
        let state = self.provider_mut(provider);
        if !matches!(state.status, DiscoverStatus::Loading) {
            state.status = DiscoverStatus::AccountRequired;
            state.sections.clear();
        }
    }

    pub(crate) fn retry(&mut self, provider: Provider) -> bool {
        let state = self.provider_mut(provider);
        if matches!(state.status, DiscoverStatus::Failed(_)) {
            state.status = DiscoverStatus::Idle;
            state.sections.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn cancel_loading(&mut self, provider: Provider) -> bool {
        if !matches!(self.provider(provider).status, DiscoverStatus::Loading) {
            return false;
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let state = self.provider_mut(provider);
        state.generation = generation;
        state.status = DiscoverStatus::Idle;
        state.sections.clear();
        true
    }

    pub(crate) fn start(&mut self, provider: Provider) -> Option<(u64, String)> {
        if !matches!(self.provider(provider).status, DiscoverStatus::Idle) {
            return None;
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let state = self.provider_mut(provider);
        state.generation = generation;
        state.status = DiscoverStatus::Loading;
        state.sections.clear();
        Some((generation, self.account_scope.clone()))
    }

    pub(crate) fn complete(
        &mut self,
        provider: Provider,
        generation: u64,
        account_scope: &str,
        result: Result<Vec<DiscoverSection>, String>,
    ) -> bool {
        if self.account_scope != account_scope {
            return false;
        }
        if self.provider(provider).generation != generation
            || !matches!(self.provider(provider).status, DiscoverStatus::Loading)
        {
            return false;
        }
        match result {
            Ok(sections) => {
                let mut sections = sections;
                if provider == Provider::Deezer {
                    self.smart_titles.apply(&mut sections);
                }
                let state = self.provider_mut(provider);
                state.sections = sections;
                state.status = DiscoverStatus::Ready;
            }
            Err(error) => {
                let state = self.provider_mut(provider);
                state.sections.clear();
                state.status = DiscoverStatus::Failed(error);
            }
        }
        true
    }

    pub(crate) fn unresolved_smart_mix_ids(&self) -> Vec<String> {
        if !matches!(self.deezer.status, DiscoverStatus::Ready) {
            return Vec::new();
        }
        let mut ids = Vec::new();
        for section in &self.deezer.sections {
            if section.provider != Provider::Deezer {
                continue;
            }
            for item in &section.items {
                if !matches!(
                    item.action,
                    DiscoverAction::PlayDeezerFlow { smart_mix: true }
                ) || specific_smart_mix_title(&item.card.title).is_some()
                {
                    continue;
                }
                let Some(id) = valid_smart_mix_id(&item.card.id) else {
                    continue;
                };
                if !ids.iter().any(|candidate| candidate == &id) {
                    ids.push(id);
                }
                if ids.len() >= MAX_SMART_MIX_ENRICHMENT_IDS {
                    return ids;
                }
            }
        }
        ids
    }

    pub(crate) fn apply_enriched_smart_mix_titles(
        &mut self,
        generation: u64,
        account_scope: &str,
        titles: &[(String, String)],
    ) -> bool {
        if self.account_scope != account_scope
            || self.deezer.generation != generation
            || !matches!(self.deezer.status, DiscoverStatus::Ready)
        {
            return false;
        }
        let unresolved = self.unresolved_smart_mix_ids();
        let mut changed = false;
        for (config_id, title) in titles {
            let Some(config_id) = valid_smart_mix_id(config_id) else {
                continue;
            };
            if unresolved.iter().any(|candidate| candidate == &config_id) {
                changed |= self.apply_smart_mix_title(&config_id, title);
            }
        }
        changed
    }

    pub(crate) fn apply_smart_mix_title(&mut self, config_id: &str, title: &str) -> bool {
        let Some((config_id, title, mut changed)) =
            self.smart_titles.remember_endpoint_title(config_id, title)
        else {
            return false;
        };

        changed |= apply_smart_mix_title_to_sections(&mut self.deezer.sections, &config_id, &title);
        changed |=
            apply_smart_mix_title_to_sections(&mut self.channel.sections, &config_id, &title);
        for cached in self.channel_cache.values_mut() {
            changed |= apply_smart_mix_title_to_sections(&mut cached.sections, &config_id, &title);
        }
        changed
    }

    fn provider_mut(&mut self, provider: Provider) -> &mut DiscoverProviderState {
        match provider {
            Provider::Deezer => &mut self.deezer,
            Provider::SoundCloud => &mut self.soundcloud,
        }
    }
}

fn apply_smart_mix_title_to_sections(
    sections: &mut [DiscoverSection],
    config_id: &str,
    title: &str,
) -> bool {
    let mut changed = false;
    for section in sections {
        if section.provider != Provider::Deezer {
            continue;
        }
        for item in &mut section.items {
            if !matches!(
                item.action,
                DiscoverAction::PlayDeezerFlow { smart_mix: true }
            ) || item.card.source != Provider::Deezer
                || item.card.id.trim() != config_id
            {
                continue;
            }
            if item.card.title != title {
                item.card.title = title.to_owned();
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::models::{ResultType, Source};

    fn section(provider: Provider, title: &str) -> DiscoverSection {
        DiscoverSection {
            id: title.to_owned(),
            provider,
            title: title.to_owned(),
            subtitle: String::new(),
            items: vec![DiscoverItem {
                card: Card {
                    kind: ResultType::Albums,
                    id: "1".into(),
                    source: provider,
                    ..Card::default()
                },
                action: DiscoverAction::OpenDetail,
            }],
        }
    }

    fn smart_section(items: &[(&str, &str)]) -> DiscoverSection {
        DiscoverSection {
            id: "smart".into(),
            provider: Provider::Deezer,
            title: "Made for you".into(),
            subtitle: String::new(),
            items: items
                .iter()
                .map(|(id, title)| DiscoverItem {
                    card: Card {
                        kind: ResultType::All,
                        id: (*id).into(),
                        title: (*title).into(),
                        source: Provider::Deezer,
                        ..Card::default()
                    },
                    action: DiscoverAction::PlayDeezerFlow { smart_mix: true },
                })
                .collect(),
        }
    }

    #[test]
    fn unresolved_smart_mix_ids_are_deduplicated_and_bounded() {
        let mut state = DiscoverState::new(format!("test-unresolved-{}", uuid::Uuid::new_v4()));
        let (generation, account_scope) = state.start(Provider::Deezer).unwrap();
        let mut items = (0..9)
            .map(|index| {
                let id = format!("inspired-by-{index}");
                let title = "Mix".to_owned();
                (id, title)
            })
            .collect::<Vec<_>>();
        items.push(("inspired-by-0".into(), "Mix".into()));
        let mut section = smart_section(&[]);
        section.items = items
            .iter()
            .map(|(id, title)| DiscoverItem {
                card: Card {
                    kind: ResultType::All,
                    id: id.clone(),
                    title: title.clone(),
                    source: Provider::Deezer,
                    ..Card::default()
                },
                action: DiscoverAction::PlayDeezerFlow { smart_mix: true },
            })
            .collect();
        assert!(state.complete(
            Provider::Deezer,
            generation,
            &account_scope,
            Ok(vec![section]),
        ));
        let ids = state.unresolved_smart_mix_ids();
        assert_eq!(ids.len(), MAX_SMART_MIX_ENRICHMENT_IDS);
        assert_eq!(ids[0], "inspired-by-0");
        assert_eq!(ids[7], "inspired-by-7");
    }

    #[test]
    fn smart_mix_enrichment_requires_current_ready_generation_and_scope() {
        let mut state = DiscoverState::new(format!("test-enrichment-{}", uuid::Uuid::new_v4()));
        let (generation, account_scope) = state.start(Provider::Deezer).unwrap();
        assert!(state.complete(
            Provider::Deezer,
            generation,
            &account_scope,
            Ok(vec![smart_section(&[("inspired-by-1", "Mix")])]),
        ));
        assert!(!state.apply_enriched_smart_mix_titles(
            generation.wrapping_sub(1),
            &account_scope,
            &[("inspired-by-1".into(), "Electro Dance".into())],
        ));
        assert!(!state.apply_enriched_smart_mix_titles(
            generation,
            "other-scope",
            &[("inspired-by-1".into(), "Electro Dance".into())],
        ));
        assert!(state.apply_enriched_smart_mix_titles(
            generation,
            &account_scope,
            &[("inspired-by-1".into(), "Electro Dance".into())],
        ));
        assert_eq!(
            state.provider(Provider::Deezer).sections[0].items[0]
                .card
                .title,
            "Electro Dance"
        );
    }

    #[test]
    fn stale_completion_is_rejected_after_account_scope_changes() {
        let mut state = DiscoverState::new("one".into());
        let (generation, scope) = state.start(Provider::Deezer).unwrap();
        state.reset_account_scope("two".into());
        assert!(!state.complete(
            Provider::Deezer,
            generation,
            &scope,
            Ok(vec![section(Provider::Deezer, "old")]),
        ));
        assert!(matches!(
            state.provider(Provider::Deezer).status,
            DiscoverStatus::Idle
        ));
    }

    #[test]
    fn provider_failures_are_independent_and_retryable() {
        let mut state = DiscoverState::new("scope".into());
        let (deezer_generation, scope) = state.start(Provider::Deezer).unwrap();
        let (soundcloud_generation, _) = state.start(Provider::SoundCloud).unwrap();
        assert!(state.complete(
            Provider::Deezer,
            deezer_generation,
            &scope,
            Err("Deezer failed".into()),
        ));
        assert!(matches!(
            state.provider(Provider::SoundCloud).status,
            DiscoverStatus::Loading
        ));
        assert!(state.retry(Provider::Deezer));
        assert!(state.start(Provider::Deezer).is_some());
        assert_eq!(
            soundcloud_generation,
            state.provider(Provider::SoundCloud).generation
        );
    }

    #[test]
    fn cancelling_a_loading_provider_returns_it_to_idle_and_rejects_completion() {
        let mut state = DiscoverState::new("scope".into());
        let (generation, scope) = state.start(Provider::Deezer).unwrap();

        assert!(state.cancel_loading(Provider::Deezer));
        assert!(matches!(
            state.provider(Provider::Deezer).status,
            DiscoverStatus::Idle
        ));
        assert!(!state.complete(
            Provider::Deezer,
            generation,
            &scope,
            Ok(vec![section(Provider::Deezer, "stale")]),
        ));
        assert!(!state.cancel_loading(Provider::Deezer));
    }

    #[test]
    fn successful_results_are_kept_until_account_scope_changes() {
        let mut state = DiscoverState::new("scope".into());
        let (generation, scope) = state.start(Provider::SoundCloud).unwrap();
        assert!(state.complete(
            Provider::SoundCloud,
            generation,
            &scope,
            Ok(vec![section(Provider::SoundCloud, "Ready")]),
        ));
        assert_eq!(state.provider(Provider::SoundCloud).sections.len(), 1);
        state.reset_account_scope("new-scope".into());
        assert!(state.provider(Provider::SoundCloud).sections.is_empty());
        assert!(matches!(
            state.provider(Provider::SoundCloud).status,
            DiscoverStatus::Idle
        ));
    }

    #[test]
    fn source_filters_discover_providers_without_mixing_sections() {
        assert_eq!(
            Source::All.providers(),
            &[Provider::Deezer, Provider::SoundCloud]
        );
        assert_eq!(Source::Deezer.providers(), &[Provider::Deezer]);
        assert_eq!(Source::SoundCloud.providers(), &[Provider::SoundCloud]);
    }

    #[test]
    fn channel_completion_is_generation_safe_and_close_keeps_root_state() {
        let mut state = DiscoverState::new("scope".into());
        let (root_generation, scope) = state.start(Provider::Deezer).unwrap();
        assert!(state.complete(
            Provider::Deezer,
            root_generation,
            &scope,
            Ok(vec![section(Provider::Deezer, "home")]),
        ));
        let (generation, channel_scope) = state.start_channel("dance".into()).unwrap();
        assert!(!state.complete_channel(
            generation.wrapping_sub(1),
            &channel_scope,
            "dance",
            Ok(("Stale".into(), vec![])),
        ));
        assert!(state.complete_channel(
            generation,
            &channel_scope,
            "dance",
            Ok((
                "Dance & EDM".into(),
                vec![section(Provider::Deezer, "tracks")]
            )),
        ));
        assert!(state.channel_open());
        assert_eq!(state.channel().title, "Dance & EDM");
        assert_eq!(state.provider(Provider::Deezer).sections[0].title, "home");
        assert!(state.close_channel());
        assert!(!state.channel_open());
        assert_eq!(state.provider(Provider::Deezer).sections[0].title, "home");
    }

    #[test]
    fn channel_start_keeps_the_clicked_title_until_the_response_arrives() {
        let mut state = DiscoverState::new("scope".into());
        state
            .start_channel_with_title("dance".into(), "Dance & EDM".into())
            .unwrap();
        assert_eq!(state.channel().title, "Dance & EDM");
    }

    #[test]
    fn channel_retry_restarts_only_failed_channel() {
        let mut state = DiscoverState::new("scope".into());
        let (generation, scope) = state.start_channel("dance".into()).unwrap();
        assert!(state.complete_channel(generation, &scope, "dance", Err("failed".into()),));
        let (retry_generation, retry_scope, slug) = state.retry_channel().unwrap();
        assert!(retry_generation > generation);
        assert_eq!(retry_scope, scope);
        assert_eq!(slug, "dance");
        assert!(matches!(
            state.channel().status,
            DiscoverChannelStatus::Loading
        ));
    }

    #[test]
    fn ready_channels_are_cached_and_reopened_without_a_new_request() {
        let mut state = DiscoverState::new("scope".into());
        let (generation, scope) = state
            .start_channel_with_title("dance".into(), "Dance & EDM".into())
            .unwrap();
        assert!(state.complete_channel(
            generation,
            &scope,
            "dance",
            Ok((
                "Dance & EDM".into(),
                vec![section(Provider::Deezer, "ready")]
            )),
        ));
        assert!(state.close_channel());

        let (cached_generation, cached_scope) = state
            .start_channel_with_title("dance".into(), "Dance & EDM".into())
            .unwrap();
        assert!(state.complete_channel_from_cache(cached_generation, &cached_scope, "dance",));
        assert!(matches!(
            state.channel().status,
            DiscoverChannelStatus::Ready
        ));
        assert_eq!(state.channel().sections[0].title, "ready");
    }

    #[test]
    fn channel_completion_applies_smart_mix_titles_before_publishing() {
        let mut state = DiscoverState::new("scope".into());
        let mut first = section(Provider::Deezer, "mix");
        first.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        first.items[0].card.id = "monthly-top".into();
        first.items[0].card.title = "Electro Dance".into();
        let (generation, scope) = state.start_channel("dance".into()).unwrap();
        assert!(state.complete_channel(
            generation,
            &scope,
            "dance",
            Ok(("Dance & EDM".into(), vec![first])),
        ));
        assert!(state.close_channel());

        let (generation, scope) = state.start_channel("dance".into()).unwrap();
        let mut refreshed = section(Provider::Deezer, "mix");
        refreshed.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        refreshed.items[0].card.id = "monthly-top".into();
        refreshed.items[0].card.title = "Daily 1".into();
        assert!(state.complete_channel(
            generation,
            &scope,
            "dance",
            Ok(("Dance & EDM".into(), vec![refreshed])),
        ));
        assert_eq!(
            state.channel().sections[0].items[0].card.title,
            "Electro Dance"
        );
    }

    #[test]
    fn cached_channel_reuses_a_title_learned_after_it_was_cached() {
        let mut state = DiscoverState::new("scope".into());
        let mut placeholder = section(Provider::Deezer, "mix");
        placeholder.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        placeholder.items[0].card.id = "inspired-by-1".into();
        placeholder.items[0].card.title = "Mix".into();
        let (generation, scope) = state.start_channel("dance".into()).unwrap();
        assert!(state.complete_channel(
            generation,
            &scope,
            "dance",
            Ok(("Dance & EDM".into(), vec![placeholder])),
        ));
        assert!(state.close_channel());

        let mut authoritative = section(Provider::Deezer, "made-for-you");
        authoritative.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        authoritative.items[0].card.id = "inspired-by-1".into();
        authoritative.items[0].card.title = "Electro Dance".into();
        let (root_generation, root_scope) = state.start(Provider::Deezer).unwrap();
        assert!(state.complete(
            Provider::Deezer,
            root_generation,
            &root_scope,
            Ok(vec![authoritative]),
        ));

        let (cached_generation, cached_scope) = state.start_channel("dance".into()).unwrap();
        assert!(state.complete_channel_from_cache(cached_generation, &cached_scope, "dance",));
        assert_eq!(
            state.channel().sections[0].items[0].card.title,
            "Electro Dance"
        );
    }

    #[test]
    fn smart_mix_title_handoff_updates_provider_open_channel_and_cached_channel() {
        let mut state = DiscoverState::new("scope".into());
        state.smart_titles = SmartTitleCache::default();
        let mut provider_mix = section(Provider::Deezer, "provider-mix");
        provider_mix.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        provider_mix.items[0].card.id = "inspired-by-3".into();
        provider_mix.items[0].card.title = "IL MEGLIO DEL MIO AGOSTO".into();
        let mut ordinary = section(Provider::Deezer, "ordinary");
        ordinary.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: false };
        ordinary.items[0].card.id = "inspired-by-3".into();
        ordinary.items[0].card.title = "ORDINARY".into();
        provider_mix.items.extend(ordinary.items);
        let mut mismatched = section(Provider::Deezer, "mismatched");
        mismatched.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        mismatched.items[0].card.id = "other-mix".into();
        mismatched.items[0].card.title = "OTHER".into();
        provider_mix.items.extend(mismatched.items);
        let (provider_generation, scope) = state.start(Provider::Deezer).unwrap();
        assert!(state.complete(
            Provider::Deezer,
            provider_generation,
            &scope,
            Ok(vec![provider_mix]),
        ));

        let mut channel_mix = section(Provider::Deezer, "channel-mix");
        channel_mix.items[0].action = DiscoverAction::PlayDeezerFlow { smart_mix: true };
        channel_mix.items[0].card.id = "inspired-by-3".into();
        channel_mix.items[0].card.title = "IL MEGLIO DEL MIO AGOSTO".into();
        let (channel_generation, channel_scope) = state.start_channel("dance".into()).unwrap();
        assert!(state.complete_channel(
            channel_generation,
            &channel_scope,
            "dance",
            Ok(("Dance".into(), vec![channel_mix])),
        ));
        assert!(state.channel_cache.contains_key("dance"));

        assert!(state.apply_smart_mix_title(" inspired-by-3 ", "Il Meglio del Mio Agosto"));
        assert_eq!(
            state.provider(Provider::Deezer).sections[0].items[0]
                .card
                .title,
            "Il Meglio del Mio Agosto"
        );
        assert_eq!(
            state.provider(Provider::Deezer).sections[0].items[1]
                .card
                .title,
            "ORDINARY"
        );
        assert_eq!(
            state.provider(Provider::Deezer).sections[0].items[2]
                .card
                .title,
            "OTHER"
        );
        assert_eq!(
            state.channel().sections[0].items[0].card.title,
            "Il Meglio del Mio Agosto"
        );
        assert_eq!(
            state.channel_cache["dance"].sections[0].items[0].card.title,
            "Il Meglio del Mio Agosto"
        );
    }
}
