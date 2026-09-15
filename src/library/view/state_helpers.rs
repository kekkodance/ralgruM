use super::*;

pub(super) fn scope_reload(state: &mut LibraryState, scope: String) -> Option<(Service, Category)> {
    state
        .set_account_scope(scope)
        .then_some((state.service, state.category))
}

pub(super) fn favorite_kind_for_root(category: Category) -> Option<FavoriteKind> {
    match category {
        Category::Tracks => Some(FavoriteKind::Track),
        Category::Albums => Some(FavoriteKind::Album),
        Category::Artists => Some(FavoriteKind::Artist),
        Category::Playlists => Some(FavoriteKind::Playlist),
        Category::History | Category::Flow | Category::MyTracks | Category::Station => None,
    }
}

pub(super) fn root_favorite_ids(category: Category, page: &Page) -> Vec<String> {
    let Some(_) = favorite_kind_for_root(category) else {
        return Vec::new();
    };
    if category == Category::Tracks {
        page.tracks
            .iter()
            .map(|track| track.id.clone())
            .filter(|id| !id.trim().is_empty())
            .collect()
    } else {
        page.cards
            .iter()
            .map(|card| card.id.clone())
            .filter(|id| !id.trim().is_empty())
            .collect()
    }
}

pub(super) fn root_favorite_keys(
    service: Service,
    category: Category,
    page: &Page,
) -> Vec<FavoriteKey> {
    if service == Service::Local {
        return Vec::new();
    }
    let Some(kind) = favorite_kind_for_root(category) else {
        return Vec::new();
    };
    let provider = match service {
        Service::Local => unreachable!("Local items are not provider favorites"),
        Service::Deezer => Provider::Deezer,
        Service::SoundCloud => Provider::SoundCloud,
    };
    root_favorite_ids(category, page)
        .into_iter()
        .map(|id| FavoriteKey::for_provider(provider, kind, id))
        .collect()
}

pub(super) fn category_focus_slot_count() -> usize {
    Service::Deezer
        .categories()
        .len()
        .max(Service::SoundCloud.categories().len())
}

pub(super) fn discover_flow_return_active(
    origin: Option<(Service, Category)>,
    service: Service,
    route: &Route,
    route_depth: usize,
) -> bool {
    origin.is_some() && is_deezer_flow_detail(service, route, route_depth)
}

pub(super) fn active_deezer_flow_kind(
    context: &PlaybackContext,
    config_id: &str,
) -> Option<DeezerFlowKind> {
    match context {
        PlaybackContext::DeezerFlow {
            config_id: active,
            kind,
            ..
        } if active == config_id => Some(kind.clone()),
        _ => None,
    }
}

pub(super) fn reuse_cached_flow_page(flow_kind: &DeezerFlowKind, cached: Option<&Page>) -> bool {
    *flow_kind == DeezerFlowKind::SmartMix && cached.is_some()
}

pub(super) fn smart_mix_title_event(config_id: &str, title: &str) -> Option<LibraryEvent> {
    let config_id = config_id.trim();
    let title = specific_smart_mix_title(title)?;
    (!config_id.is_empty()).then_some(())?;
    Some(LibraryEvent::SmartMixTitleResolved {
        config_id: config_id.to_owned(),
        title: title.to_owned(),
    })
}

pub(super) struct TrackListCache {
    pub(super) state: ListState,
    pub(super) browser_scroll: BrowserScrollState,
    pub(super) shape: (usize, TrackListLayout),
    pub(super) row_identities: Vec<String>,
}

pub(super) struct CardGridCache {
    pub(super) state: ListState,
    pub(super) browser_scroll: BrowserScrollState,
    pub(super) item_count: usize,
    pub(super) layout: CardGridLayout,
    pub(super) uniform_height: Pixels,
    pub(super) ordered_content_identity: Vec<String>,
    pub(super) card_count: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CardGridChange {
    Unchanged,
    Remeasure,
    Reflow,
    Reset,
}

pub(super) fn card_grid_anchor(
    state: &ListState,
    previous_count: usize,
    previous_layout: CardGridLayout,
    previous_uniform_height: Pixels,
    next_count: usize,
    next_layout: CardGridLayout,
    next_uniform_height: Pixels,
    previous_card_count: Option<usize>,
) -> Option<ListOffset> {
    if previous_count == 0 || next_count == 0 {
        return None;
    }
    let current = state.logical_scroll_top();
    let item_ix = if previous_layout.columns == next_layout.columns {
        current.item_ix.min(next_count)
    } else {
        let card_count = previous_card_count?;
        if current.item_ix >= previous_count {
            next_count
        } else {
            let card_index = current
                .item_ix
                .saturating_mul(previous_layout.columns)
                .min(card_count);
            (card_index / next_layout.columns).min(next_count)
        }
    };
    let fraction = if f32::from(previous_uniform_height) > 0. {
        (f32::from(current.offset_in_item) / f32::from(previous_uniform_height)).clamp(0., 1.)
    } else {
        0.
    };
    Some(ListOffset {
        item_ix,
        offset_in_item: px(fraction * f32::from(next_uniform_height)),
    })
}

pub(super) fn card_grid_change(
    previous_count: usize,
    previous_layout: CardGridLayout,
    previous_identity: &[String],
    next_count: usize,
    next_layout: CardGridLayout,
    next_identity: &[String],
) -> CardGridChange {
    if previous_identity != next_identity {
        return CardGridChange::Reset;
    }
    if previous_count == next_count && previous_layout == next_layout {
        return CardGridChange::Unchanged;
    }
    if previous_count != next_count || previous_layout.columns != next_layout.columns {
        CardGridChange::Reflow
    } else {
        CardGridChange::Remeasure
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TrackListChange {
    Unchanged,
    Append { old_count: usize, added: usize },
    Reset,
}

pub(super) fn track_list_change(
    previous_rows: &[String],
    previous_shape: (usize, TrackListLayout),
    next_rows: &[String],
    next_layout: TrackListLayout,
) -> TrackListChange {
    if previous_shape.0 != previous_rows.len() || previous_shape.1 != next_layout {
        return TrackListChange::Reset;
    }
    if previous_rows == next_rows {
        return TrackListChange::Unchanged;
    }
    if next_rows.len() > previous_rows.len() && next_rows.starts_with(previous_rows) {
        return TrackListChange::Append {
            old_count: previous_rows.len(),
            added: next_rows.len() - previous_rows.len(),
        };
    }
    TrackListChange::Reset
}

pub(super) const MAX_TRACK_LIST_STATES: usize = 24;
pub(super) const MAX_CARD_GRID_STATES: usize = 24;
