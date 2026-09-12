use crate::{
    context_menu::{
        DiscoverMenuAction, DiscoverMenuPrimary, card_entity, card_menu,
        discover_card_menu_with_actions,
    },
    music_ui::{
        CardCarouselState, CardKind, animate_grid_card, card_carousel, card_carousel_has_overflow,
        card_row_metrics, horizontal_scroll_boundary, horizontal_scroll_id,
    },
};
use gpui::{
    AnyElement, Entity, KeyDownEvent, StatefulInteractiveElement, div, prelude::*, px, rgb,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::Range,
    sync::Arc,
};

use super::{
    detail::DetailRoute,
    discover::{DiscoverAction, DiscoverItem},
    models::Card,
    view::{CardGridLayout, SearchView},
};
use crate::collection_detail::{
    CollectionCardPresentation, collection_card_carousel_content,
    collection_card_content_with_presentation, collection_card_frame,
};
use crate::library::FavoriteState;
use crate::settings::AccountState;
use crate::ui::music_ui::CollectionCardTitleAlignment;

const CARD_GRID_GAP: f32 = crate::collection_detail::DETAIL_CARD_GRID_GAP_PX;
const CARD_GRID_BORDER: f32 = 2.;
const CARD_GRID_TITLE_HEIGHT: f32 = 16.;
const CARD_GRID_SUBTITLE_HEIGHT: f32 = 16.;
const SEARCH_ALL_PREVIEW_CARD_LIMIT: usize = 24;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_cards(
    view: &SearchView,
    host: &Entity<SearchView>,
    cards: &[Card],
    section_identity: &str,
    section_index: usize,
    preview: bool,
    columns: u16,
    available_width: f32,
    narrow: bool,
    _favorites: Entity<FavoriteState>,
    account: Entity<AccountState>,
    carousel_state: CardCarouselState,
) -> AnyElement {
    let (card_width, row_gap) = card_row_metrics(narrow);
    let displayed_count = card_display_count(cards.len(), preview);
    let end = displayed_count.min(cards.len());
    let cards = &cards[..end];
    if preview {
        let controls_available =
            card_carousel_has_overflow(cards.len(), card_width, row_gap, available_width, 2.);
        let scroll_id = horizontal_scroll_id(
            "search-card-preview-scroll",
            section_identity,
            section_index,
        );
        return div()
            .relative()
            .w_full()
            .child(horizontal_scroll_boundary(
                format!("{scroll_id}-boundary"),
                card_carousel(
                    scroll_id,
                    carousel_state,
                    card_width,
                    row_gap,
                    controls_available,
                    collection_card_carousel_content(
                        row_gap,
                        true,
                        cards
                            .iter()
                            .map(|card| render_card(host, card, true, narrow, &account)),
                    )
                    .into_any_element(),
                ),
            ))
            .into_any_element();
    }

    render_virtualized_cards(
        view,
        host,
        cards,
        section_identity,
        columns,
        available_width,
        narrow,
        account,
    )
}

fn render_virtualized_cards(
    view: &SearchView,
    host: &Entity<SearchView>,
    cards: &[Card],
    section_identity: &str,
    columns: u16,
    available_width: f32,
    narrow: bool,
    account: Entity<AccountState>,
) -> AnyElement {
    let columns = columns.max(1);
    let layout = card_grid_layout(available_width, columns, narrow);
    let row_count = card_grid_row_count(cards.len(), columns);
    let cache_key = view.card_grid_cache_key(section_identity);
    let (state, browser_scroll) = view.card_grid_state(
        &cache_key,
        card_content_signature(cards),
        cards.len(),
        layout,
    );
    let fixed_scroll = crate::browser_scroll::FixedListScrollHandle::new(
        state.clone(),
        row_count,
        px(layout.row_height.max(1.)),
    );
    let cards = Arc::new(cards.to_vec());
    let host = host.clone();
    let visual = view.card_grid_motion.visual();
    let height_extra = (layout.row_height - layout.card_width - CARD_GRID_GAP).max(0.);
    let list = gpui::list(state.clone(), move |row_index, _window, _app| {
        let Some(range) = card_grid_row_range(row_index, cards.len(), columns) else {
            return div().into_any_element();
        };
        div()
            .w_full()
            .h(px(layout.row_height))
            .flex_none()
            .pb(px(CARD_GRID_GAP))
            .child(render_card_grid_row(
                &host,
                &cards[range.clone()],
                columns,
                narrow,
                &account,
                visual,
                range.start,
                height_extra,
            ))
            .into_any_element()
    });
    let content = div()
        .w_full()
        .flex_1()
        .min_h_0()
        .relative()
        .child(list.w_full().h_full().min_h_0())
        .child(crate::library::virtualization::library_vertical_scrollbar(
            &fixed_scroll,
            narrow,
        ))
        .into_any_element();
    crate::browser_scroll::browser_scroll_surface(
        format!("search-card-grid-scroll-{section_identity}"),
        content,
        crate::browser_scroll::BrowserScrollTarget::FixedList(fixed_scroll),
        browser_scroll,
    )
}

pub(super) fn render_card_grid_row(
    host: &Entity<SearchView>,
    cards: &[Card],
    columns: u16,
    narrow: bool,
    account: &Entity<AccountState>,
    visual: crate::music_ui::CardGridVisual,
    card_index_base: usize,
    height_extra: f32,
) -> AnyElement {
    div()
        .w_full()
        .grid()
        .grid_cols(columns.max(1))
        .gap(px(CARD_GRID_GAP))
        .children(cards.iter().enumerate().map(|(index, card)| {
            let element = render_card(host, card, false, narrow, account);
            if visual.animating {
                animate_grid_card(
                    element,
                    card_index_base + index,
                    visual,
                    CARD_GRID_GAP,
                    height_extra,
                )
            } else {
                element
            }
        }))
        .into_any_element()
}

pub(super) fn render_card(
    host: &Entity<SearchView>,
    card: &Card,
    carousel: bool,
    narrow: bool,
    account: &Entity<AccountState>,
) -> AnyElement {
    render_card_with_presentation(
        host,
        card,
        carousel,
        narrow,
        account,
        false,
        CollectionCardPresentation::default(),
        None,
    )
}

pub(super) fn render_card_with_presentation(
    host: &Entity<SearchView>,
    card: &Card,
    carousel: bool,
    narrow: bool,
    account: &Entity<AccountState>,
    preserve_discover_channel: bool,
    presentation: CollectionCardPresentation,
    discover_occurrence: Option<(&str, usize)>,
) -> AnyElement {
    let route = DetailRoute::from_card(card);
    let card_arc = Arc::new(card.clone());
    let card_content = div()
        .id(card_element_id_for_occurrence(
            "search-card-open",
            card,
            discover_occurrence,
        ))
        .when(route.is_some(), |this| {
            let card_for_click = card_arc.clone();
            let card_for_key = card_arc.clone();
            this.focusable()
                .tab_stop(true)
                .role(gpui::Role::Button)
                .aria_label(format!(
                    "Open {} {} {}",
                    card.source.label(),
                    card.kind.label(),
                    card.title
                ))
                .cursor_pointer()
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .on_click({
                    let host = host.clone();
                    move |_, _, app| {
                        host.update(app, |this, cx| {
                            if preserve_discover_channel {
                                this.open_discover_card((*card_for_click).clone(), cx);
                            } else {
                                this.open_card((*card_for_click).clone(), cx);
                            }
                        });
                    }
                })
                .on_key_down({
                    let host = host.clone();
                    move |event: &KeyDownEvent, window, app| {
                        if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                            window.prevent_default();
                            host.update(app, |this, cx| {
                                if preserve_discover_channel {
                                    this.open_discover_card((*card_for_key).clone(), cx);
                                } else {
                                    this.open_card((*card_for_key).clone(), cx);
                                }
                            });
                        }
                    }
                })
        })
        .min_w_0()
        .child(collection_card_content_with_presentation(
            &card.title,
            &card.subtitle,
            &card.artwork,
            card_kind(card.kind),
            card.source,
            &card.badge,
            None,
            &card.id,
            presentation,
        ));
    let element = collection_card_frame(carousel, narrow, card_content.into_any_element()).id(
        card_element_id_for_occurrence("search-card", card, discover_occurrence),
    );
    // Start the about info fetch on hover so a following right-click menu
    // can open the Info dialog already populated. Non collection cards and
    // cached entries are ignored inside the prefetch itself.
    let prefetch_host = host.clone();
    let prefetch_card = (*card_arc).clone();
    let element = element.on_hover(move |hovered, _, cx| {
        if *hovered {
            prefetch_host.update(cx, |view, cx| {
                view.prefetch_card_info(prefetch_card.clone(), cx);
            });
        }
    });
    match card_entity(card, false) {
        Some(entity) => card_menu(
            element,
            entity,
            (*card_arc).clone(),
            host.clone(),
            account.clone(),
        )
        .into_any_element(),
        None => element.into_any_element(),
    }
}

pub(super) fn render_discover_card(
    host: &Entity<SearchView>,
    item: &DiscoverItem,
    section_id: &str,
    item_index: usize,
    narrow: bool,
    account: &Entity<AccountState>,
) -> AnyElement {
    if item.action == DiscoverAction::OpenDetail {
        return render_card_with_presentation(
            host,
            &item.card,
            true,
            narrow,
            account,
            true,
            discover_card_presentation(item),
            Some((section_id, item_index)),
        );
    }

    let card = &item.card;
    let card_arc = Arc::new(card.clone());
    let action = item.action.clone();
    let action_for_key = action.clone();
    let actionable = action != DiscoverAction::None;
    let action_label = match action {
        DiscoverAction::PlayDeezerTrack(_) => "Play Deezer mix for",
        DiscoverAction::PlayDeezerFlow { smart_mix: true } => "Open Deezer mix",
        DiscoverAction::PlayDeezerFlow { smart_mix: false } => "Open Deezer Flow",
        DiscoverAction::OpenDeezerChannel(_) => "Open Deezer channel",
        DiscoverAction::OpenSoundCloudSelection(_) => "Open SoundCloud selection",
        DiscoverAction::OpenDetail | DiscoverAction::None => "Open",
    };
    let presentation = discover_card_presentation(item);
    let click_card = card_arc.clone();
    let key_card = card_arc.clone();
    let content = div()
        .id(discover_card_element_id(
            "discover-card-action",
            section_id,
            item_index,
            card,
        ))
        .when(actionable, |this| {
            let click_host = host.clone();
            let click_action = action.clone();
            let key_host = host.clone();
            this.focusable()
                .tab_stop(true)
                .role(gpui::Role::Button)
                .aria_label(format!("{action_label} {}", card.title))
                .cursor_pointer()
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .on_click(move |_, _, app| {
                    dispatch_discover_action(
                        &click_host,
                        click_action.clone(),
                        (*click_card).clone(),
                        app,
                    );
                })
                .on_key_down(move |event: &KeyDownEvent, window, app| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        dispatch_discover_action(
                            &key_host,
                            action_for_key.clone(),
                            (*key_card).clone(),
                            app,
                        );
                    }
                })
        })
        .min_w_0()
        .child(collection_card_content_with_presentation(
            &card.title,
            &card.subtitle,
            &card.artwork,
            discover_card_kind(item),
            card.source,
            &card.badge,
            None,
            &card.id,
            presentation,
        ));

    let element = collection_card_frame(true, narrow, content.into_any_element()).id(
        discover_card_element_id("discover-card", section_id, item_index, &card_arc),
    );
    if let Some(menu_actions) = discover_menu_actions(&action) {
        let menu_entries = menu_actions
            .iter()
            .copied()
            .map(|primary| {
                let host = host.clone();
                let action = action.clone();
                let card = (*card_arc).clone();
                DiscoverMenuAction::new(primary, move |_, _, app| {
                    dispatch_discover_menu_action(
                        &host,
                        primary,
                        action.clone(),
                        card.clone(),
                        app,
                    );
                })
            })
            .collect();
        discover_card_menu_with_actions(element, card.title.clone(), menu_entries)
            .into_any_element()
    } else {
        element.into_any_element()
    }
}

fn discover_card_presentation(item: &DiscoverItem) -> CollectionCardPresentation {
    let mut presentation = CollectionCardPresentation::discover();
    if matches!(
        item.action,
        DiscoverAction::PlayDeezerFlow { smart_mix: false }
    ) {
        presentation.title_alignment = CollectionCardTitleAlignment::KindDefault;
    }
    presentation
}

fn discover_menu_actions(action: &DiscoverAction) -> Option<&'static [DiscoverMenuPrimary]> {
    const TRACK_MIX: &[DiscoverMenuPrimary] = &[DiscoverMenuPrimary::PlayMix];
    const FLOW: &[DiscoverMenuPrimary] =
        &[DiscoverMenuPrimary::PlayFlow, DiscoverMenuPrimary::Open];
    const SMART_MIX: &[DiscoverMenuPrimary] =
        &[DiscoverMenuPrimary::PlayMix, DiscoverMenuPrimary::Open];
    const CHANNEL: &[DiscoverMenuPrimary] = &[DiscoverMenuPrimary::Open];
    const SOUND_CLOUD_SELECTION: &[DiscoverMenuPrimary] = &[
        DiscoverMenuPrimary::PlaySelection,
        DiscoverMenuPrimary::Open,
    ];

    match action {
        DiscoverAction::PlayDeezerTrack(_) => Some(TRACK_MIX),
        DiscoverAction::PlayDeezerFlow { smart_mix: true } => Some(SMART_MIX),
        DiscoverAction::PlayDeezerFlow { smart_mix: false } => Some(FLOW),
        DiscoverAction::OpenDeezerChannel(_) => Some(CHANNEL),
        DiscoverAction::OpenSoundCloudSelection(_) => Some(SOUND_CLOUD_SELECTION),
        DiscoverAction::OpenDetail | DiscoverAction::None => None,
    }
}

fn dispatch_discover_menu_action(
    host: &Entity<SearchView>,
    primary: DiscoverMenuPrimary,
    action: DiscoverAction,
    card: Card,
    app: &mut gpui::App,
) {
    host.update(app, |view, cx| match (primary, action) {
        (DiscoverMenuPrimary::PlayMix, DiscoverAction::PlayDeezerTrack(track_id)) => {
            view.start_deezer_track_mix(track_id, cx);
        }
        (DiscoverMenuPrimary::PlayMix, DiscoverAction::PlayDeezerFlow { smart_mix: true }) => {
            view.play_deezer_flow(card, true, cx);
        }
        (DiscoverMenuPrimary::PlayFlow, DiscoverAction::PlayDeezerFlow { smart_mix: false }) => {
            view.play_deezer_flow(card, false, cx);
        }
        (
            DiscoverMenuPrimary::PlaySelection,
            DiscoverAction::OpenSoundCloudSelection(track_ids),
        ) => {
            view.play_soundcloud_discover_selection(card, track_ids, cx);
        }
        (DiscoverMenuPrimary::Open, DiscoverAction::PlayDeezerTrack(track_id)) => {
            view.start_deezer_track_mix(track_id, cx);
        }
        (DiscoverMenuPrimary::Open, DiscoverAction::PlayDeezerFlow { smart_mix }) => {
            view.open_deezer_flow(card, smart_mix, cx);
        }
        (DiscoverMenuPrimary::Open, DiscoverAction::OpenDeezerChannel(slug)) => {
            view.open_deezer_channel(card, slug, cx);
        }
        (DiscoverMenuPrimary::Open, DiscoverAction::OpenSoundCloudSelection(track_ids)) => {
            view.open_soundcloud_discover_selection(card, track_ids, cx);
        }
        _ => {}
    });
}

fn dispatch_discover_action(
    host: &Entity<SearchView>,
    action: DiscoverAction,
    card: Card,
    app: &mut gpui::App,
) {
    host.update(app, |view, cx| match action {
        DiscoverAction::OpenDetail | DiscoverAction::None => {}
        DiscoverAction::PlayDeezerTrack(track_id) => {
            view.start_deezer_track_mix(track_id, cx);
        }
        DiscoverAction::PlayDeezerFlow { smart_mix } => {
            view.open_deezer_flow(card, smart_mix, cx);
        }
        DiscoverAction::OpenDeezerChannel(slug) => {
            view.open_deezer_channel(card, slug, cx);
        }
        DiscoverAction::OpenSoundCloudSelection(track_ids) => {
            view.open_soundcloud_discover_selection(card, track_ids, cx);
        }
    });
}

pub(super) fn card_grid_row_count(card_count: usize, columns: u16) -> usize {
    card_count.div_ceil(usize::from(columns.max(1)))
}

pub(super) fn card_grid_row_range(
    row_index: usize,
    card_count: usize,
    columns: u16,
) -> Option<Range<usize>> {
    let columns = usize::from(columns.max(1));
    let start = row_index.checked_mul(columns)?;
    (start < card_count).then_some(start..(start + columns).min(card_count))
}

pub(super) fn card_display_count(card_count: usize, preview: bool) -> usize {
    if preview {
        card_count.min(SEARCH_ALL_PREVIEW_CARD_LIMIT)
    } else {
        card_count
    }
}

pub(super) fn card_grid_layout(available_width: f32, columns: u16, narrow: bool) -> CardGridLayout {
    let columns = usize::from(columns.max(1));
    let settled_width = if available_width.is_finite() {
        available_width.max(0.).round()
    } else {
        0.
    };
    let card_width = ((settled_width - CARD_GRID_GAP * (columns.saturating_sub(1) as f32))
        / columns as f32)
        .max(0.);
    let outer_padding = if narrow { 6. } else { 8. };
    let artwork_width = (card_width - outer_padding * 2. - CARD_GRID_BORDER).max(0.);
    let text_block_height = 2. + CARD_GRID_TITLE_HEIGHT + 2. + CARD_GRID_SUBTITLE_HEIGHT + 3.;
    let card_height =
        outer_padding * 2. + CARD_GRID_BORDER + artwork_width + 7. + text_block_height;
    CardGridLayout::new(
        columns,
        settled_width,
        card_width,
        card_height + CARD_GRID_GAP,
    )
}

pub(super) fn stable_card_identity(card: &Card) -> String {
    let service_identity = if !card.id.trim().is_empty() {
        format!("id:{}", card.id.trim())
    } else if !card.service_url.trim().is_empty() {
        format!("url:{}", card.service_url.trim())
    } else {
        let mut hasher = DefaultHasher::new();
        card.title.hash(&mut hasher);
        card.subtitle.hash(&mut hasher);
        card.artwork.hash(&mut hasher);
        card.badge.hash(&mut hasher);
        card.release_date.hash(&mut hasher);
        card.service_url.hash(&mut hasher);
        format!("fields:{:016x}", hasher.finish())
    };
    format!(
        "{}:{}:{}",
        card.source.label(),
        card.kind.label(),
        service_identity
    )
}

pub(super) fn card_content_signature(cards: &[Card]) -> u64 {
    let mut hasher = DefaultHasher::new();
    "search-card-content-v1".hash(&mut hasher);
    for card in cards {
        stable_card_identity(card).hash(&mut hasher);
    }
    hasher.finish()
}

fn card_kind(kind: super::models::ResultType) -> CardKind {
    match kind {
        super::models::ResultType::Artists => CardKind::Artist,
        super::models::ResultType::Playlists => CardKind::Playlist,
        super::models::ResultType::Albums => CardKind::Album,
        _ => CardKind::Other,
    }
}

fn discover_card_kind(item: &DiscoverItem) -> CardKind {
    if matches!(item.action, DiscoverAction::PlayDeezerFlow { .. }) {
        CardKind::Flow
    } else {
        card_kind(item.card.kind)
    }
}

fn card_element_id(prefix: &str, card: &Card) -> String {
    format!("{prefix}-{}", stable_card_identity(card))
}

fn card_element_id_for_occurrence(
    prefix: &str,
    card: &Card,
    discover_occurrence: Option<(&str, usize)>,
) -> String {
    discover_occurrence.map_or_else(
        || card_element_id(prefix, card),
        |(section_id, item_index)| discover_card_element_id(prefix, section_id, item_index, card),
    )
}

fn discover_card_element_id(
    prefix: &str,
    section_id: &str,
    item_index: usize,
    card: &Card,
) -> String {
    format!(
        "{prefix}-section:{section_id}-item:{item_index}-{}",
        stable_card_identity(card)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Provider, ResultType, view};

    fn card(kind: ResultType, id: &str) -> Card {
        Card {
            kind,
            id: id.into(),
            title: format!("{kind:?}"),
            source: Provider::Deezer,
            ..Card::default()
        }
    }

    #[test]
    fn row_count_and_slices_use_clamped_columns() {
        assert_eq!(card_grid_row_count(0, 4), 0);
        assert_eq!(card_grid_row_count(7, 3), 3);
        assert_eq!(card_grid_row_count(3, 0), 3);
        assert_eq!(card_grid_row_range(0, 7, 3), Some(0..3));
        assert_eq!(card_grid_row_range(2, 7, 3), Some(6..7));
        assert_eq!(card_grid_row_range(3, 7, 3), None);
    }

    #[test]
    fn card_element_ids_are_stable_and_position_independent() {
        let album = card(ResultType::Albums, "42");
        assert_eq!(
            card_element_id("search-card", &album),
            card_element_id("search-card", &album)
        );
        assert_ne!(
            card_element_id("search-card", &album),
            card_element_id("search-card", &card(ResultType::Artists, "42"))
        );
        assert_ne!(
            stable_card_identity(&album),
            stable_card_identity(&Card {
                source: Provider::SoundCloud,
                ..album.clone()
            })
        );
    }

    #[test]
    fn duplicate_new_releases_occurrences_have_stable_scoped_element_ids() {
        let new_releases = card(ResultType::Playlists, "new-releases");

        let first_frame =
            discover_card_element_id("discover-card", "made-for-you", 0, &new_releases);
        let second_frame =
            discover_card_element_id("discover-card", "new-releases", 0, &new_releases);
        let third_frame =
            discover_card_element_id("discover-card", "made-for-you", 1, &new_releases);
        let first_action =
            discover_card_element_id("discover-card-action", "made-for-you", 0, &new_releases);
        let second_action =
            discover_card_element_id("discover-card-action", "new-releases", 0, &new_releases);
        let third_action =
            discover_card_element_id("discover-card-action", "made-for-you", 1, &new_releases);

        assert_ne!(first_frame, second_frame);
        assert_ne!(first_frame, third_frame);
        assert_ne!(first_action, second_action);
        assert_ne!(first_action, third_action);
        assert_eq!(
            first_frame,
            discover_card_element_id("discover-card", "made-for-you", 0, &new_releases)
        );
        assert_eq!(
            first_action,
            discover_card_element_id("discover-card-action", "made-for-you", 0, &new_releases)
        );
        assert!(first_frame.ends_with(&stable_card_identity(&new_releases)));
    }

    #[test]
    fn duplicate_openable_discover_cards_use_occurrence_scoped_element_ids() {
        let album = card(ResultType::Albums, "42");
        let first_open =
            card_element_id_for_occurrence("search-card-open", &album, Some(("made-for-you", 0)));
        let second_open =
            card_element_id_for_occurrence("search-card-open", &album, Some(("new-releases", 0)));
        let first_frame =
            card_element_id_for_occurrence("search-card", &album, Some(("made-for-you", 0)));

        assert_ne!(first_open, second_open);
        assert!(first_open.contains("section:made-for-you-item:0"));
        assert!(first_frame.contains("section:made-for-you-item:0"));
        assert_eq!(
            first_open,
            card_element_id_for_occurrence("search-card-open", &album, Some(("made-for-you", 0)),)
        );
        assert_eq!(
            card_element_id_for_occurrence("search-card-open", &album, None),
            card_element_id("search-card-open", &album)
        );
    }

    #[test]
    fn missing_provider_ids_use_stable_card_fields() {
        let first = Card {
            kind: ResultType::Playlists,
            title: "Mix".into(),
            subtitle: "Artist".into(),
            artwork: "artwork".into(),
            source: Provider::SoundCloud,
            ..Card::default()
        };
        let second = first.clone();
        assert_eq!(stable_card_identity(&first), stable_card_identity(&second));
    }

    #[test]
    fn previews_keep_a_stable_ultrawide_sized_window() {
        assert_eq!(SEARCH_ALL_PREVIEW_CARD_LIMIT, 24);
        assert_eq!(card_display_count(40, true), 24);
        assert_eq!(card_display_count(4, true), 4);
        assert_eq!(card_display_count(40, false), 40);
        assert_eq!(card_display_count(100, true), 24);
    }

    #[test]
    fn card_content_signature_follows_ordered_stable_identities() {
        let first = [card(ResultType::Albums, "1"), card(ResultType::Albums, "2")];
        let second = [card(ResultType::Albums, "2"), card(ResultType::Albums, "1")];
        assert_ne!(
            card_content_signature(&first),
            card_content_signature(&second)
        );
        assert_eq!(
            card_content_signature(&first),
            card_content_signature(&first)
        );
    }

    #[test]
    fn card_layout_includes_settled_width_and_card_row_gap() {
        let layout = card_grid_layout(700., 3, false);
        assert_eq!(layout.columns, 3);
        assert_eq!(layout.settled_width, 700.);
        assert!(layout.card_width > 0.);
        assert!(layout.row_height > layout.card_width);
    }

    #[test]
    fn discover_flows_use_the_centered_library_flow_card_kind() {
        let item = DiscoverItem {
            card: card(ResultType::All, "flow-id"),
            action: DiscoverAction::PlayDeezerFlow { smart_mix: false },
        };
        assert!(matches!(discover_card_kind(&item), CardKind::Flow));
        assert_eq!(
            discover_card_presentation(&item).title_alignment,
            CollectionCardTitleAlignment::KindDefault
        );

        let smart_mix = DiscoverItem {
            card: card(ResultType::All, "smart-mix-id"),
            action: DiscoverAction::PlayDeezerFlow { smart_mix: true },
        };
        assert_eq!(
            discover_card_presentation(&smart_mix).title_alignment,
            CollectionCardTitleAlignment::Left
        );

        let ordinary = DiscoverItem {
            card: card(ResultType::Playlists, "42"),
            action: DiscoverAction::OpenDetail,
        };
        assert!(matches!(discover_card_kind(&ordinary), CardKind::Playlist));
        assert_eq!(
            discover_card_presentation(&ordinary).title_alignment,
            CollectionCardTitleAlignment::Left
        );
    }

    #[test]
    fn discover_detail_cards_use_the_channel_preserving_opener() {
        let production = include_str!("cards_view.rs")
            .split_once("#[cfg(test)]")
            .map_or(include_str!("cards_view.rs"), |(production, _)| production);
        let discover_detail = production
            .split_once("if item.action == DiscoverAction::OpenDetail")
            .and_then(|(_, rest)| rest.split_once("let card = &item.card;"))
            .map(|(branch, _)| branch)
            .expect("Discover detail card branch");
        let call = discover_detail
            .split_once("render_card_with_presentation(")
            .and_then(|(_, rest)| rest.split_once(");"))
            .map(|(call, _)| call)
            .expect("Discover detail card presentation call");
        assert!(call.contains("true"));
        assert!(call.contains("discover_card_presentation(item)"));
        assert!(call.contains("Some((section_id, item_index))"));
        assert!(production.contains("this.open_discover_card"));
    }

    #[test]
    fn special_discover_cards_get_explicit_action_menus() {
        assert_eq!(
            discover_menu_actions(&DiscoverAction::PlayDeezerTrack("42".into())),
            Some(&[DiscoverMenuPrimary::PlayMix][..])
        );
        assert_eq!(
            discover_menu_actions(&DiscoverAction::PlayDeezerFlow { smart_mix: false }),
            Some(&[DiscoverMenuPrimary::PlayFlow, DiscoverMenuPrimary::Open][..])
        );
        assert_eq!(
            discover_menu_actions(&DiscoverAction::PlayDeezerFlow { smart_mix: true }),
            Some(&[DiscoverMenuPrimary::PlayMix, DiscoverMenuPrimary::Open][..])
        );
        assert_eq!(
            discover_menu_actions(&DiscoverAction::OpenDeezerChannel("dance".into())),
            Some(&[DiscoverMenuPrimary::Open][..])
        );
        assert_eq!(
            discover_menu_actions(&DiscoverAction::OpenSoundCloudSelection(vec!["1".into()])),
            Some(
                &[
                    DiscoverMenuPrimary::PlaySelection,
                    DiscoverMenuPrimary::Open
                ][..]
            )
        );
        assert_eq!(discover_menu_actions(&DiscoverAction::OpenDetail), None);
        let source = include_str!("cards_view.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("cards view production source");
        assert!(source.contains("discover_card_menu_with_actions("));
        assert!(source.contains("play_soundcloud_discover_selection"));
    }

    #[test]
    fn mixed_collection_cards_keep_routes_and_actions() {
        let cards = [
            card(ResultType::Albums, "42"),
            card(ResultType::Artists, "43"),
            card(ResultType::Playlists, "44"),
        ];
        let ids = cards
            .iter()
            .map(|card| {
                assert!(DetailRoute::from_card(card).is_some());
                assert!(view::favorite_kind(card.source, card.kind, &card.id).is_some());
                card_element_id("search-card-favorite", card)
            })
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), cards.len());
    }

    #[test]
    fn card_grid_scrollbar_uses_the_shared_viewport_edge_outset() {
        let implementation = include_str!("cards_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("cards view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("library_vertical_scrollbar("));
        assert!(implementation.contains("&fixed_scroll"));
        assert!(implementation.contains("narrow"));
    }
}
