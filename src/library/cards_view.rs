use gpui::{
    AnyElement, App, KeyDownEvent, StatefulInteractiveElement, Window, div, prelude::*, px, rgb,
};
use std::{rc::Rc, sync::Arc};

use crate::{
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    collection_detail::{
        collection_card_carousel_content, collection_card_content, collection_card_frame,
    },
    context_menu,
    music_ui::{
        CardKind, animate_grid_card, card_carousel, card_carousel_display_count,
        card_carousel_has_overflow, card_row_metrics, collection_card, horizontal_scroll_boundary,
        horizontal_scroll_id, local_collection_card,
    },
    theme::{BORDER, SURFACE},
};

use super::{
    model::{Card, Category},
    view::LibraryView,
    virtualization::{self, CardGridLayout},
};

#[derive(Clone)]
pub(super) struct CardsRenderSnapshot {
    favorite_roots: [bool; 3],
    carousel_state: Option<crate::music_ui::CardCarouselState>,
    grid_visual: crate::music_ui::CardGridVisual,
    available_width: f32,
    shared_provider_artist_visual: bool,
    grid_height_extra: f32,
}

impl CardsRenderSnapshot {
    pub(super) fn from_view(view: &LibraryView, scroll_id: Option<&str>) -> Self {
        let shared_provider_artist_visual = provider_artist_detail(view);
        Self {
            favorite_roots: [
                view.state.selected_root_active(Category::Albums),
                view.state.selected_root_active(Category::Artists),
                view.state.selected_root_active(Category::Playlists),
            ],
            carousel_state: scroll_id.map(|id| view.card_scroll_handle(id)),
            grid_visual: view.card_grid_motion.visual(),
            available_width: view.card_available_width,
            shared_provider_artist_visual,
            grid_height_extra: if shared_provider_artist_visual {
                crate::collection_detail::DETAIL_CARD_GRID_ROW_HEIGHT_EXTRA_PX
            } else {
                virtualization::CARD_GRID_ROW_HEIGHT_EXTRA
            },
        }
    }

    fn favorite_root(&self, kind: Category) -> bool {
        match kind {
            Category::Albums => self.favorite_roots[0],
            Category::Artists => self.favorite_roots[1],
            Category::Playlists => self.favorite_roots[2],
            _ => false,
        }
    }
}

pub(super) fn cards(
    view: &LibraryView,
    host: &gpui::Entity<LibraryView>,
    cards: &[Card],
    section_identity: &str,
    section_index: usize,
    columns: u16,
    preview_limit: Option<usize>,
    card_row: bool,
    narrow: bool,
    _app: &gpui::App,
) -> AnyElement {
    if virtualization::should_virtualize_card_grid(preview_limit, cards.len()) {
        return virtualized_cards(
            view,
            host,
            cards,
            section_identity,
            columns,
            view.card_available_width,
            narrow,
            "",
        );
    }
    let section_identity = card_scroll_identity(view, section_identity, cards);
    let scroll_id = card_row
        .then(|| horizontal_scroll_id("library-card-row-scroll", &section_identity, section_index));
    let snapshot = CardsRenderSnapshot::from_view(view, scroll_id.as_deref());
    cards_with_snapshot(
        &snapshot,
        host,
        cards,
        &section_identity,
        section_index,
        columns,
        preview_limit,
        card_row,
        narrow,
        0,
    )
}

pub(super) fn virtualized_cards(
    view: &LibraryView,
    host: &gpui::Entity<LibraryView>,
    cards: &[Card],
    section_identity: &str,
    columns: u16,
    available_width: f32,
    narrow: bool,
    query: &str,
) -> AnyElement {
    let shared_provider_artist_visual = provider_artist_detail(view);
    let layout = CardGridLayout::new_for_visual(
        columns,
        available_width,
        narrow,
        shared_provider_artist_visual,
    );
    let row_count = virtualization::card_grid_row_count(cards.len(), layout.columns);
    let content_key = card_grid_content_key(view, section_identity, query, cards);
    let card_identities = cards.iter().map(card_content_identity).collect::<Vec<_>>();
    let state = view.card_grid_state_with_rows(
        &content_key,
        row_count,
        layout,
        &card_identities,
        Some(cards.len()),
    );
    let browser_scroll = view.card_grid_browser_scroll(&content_key);
    let fixed_scroll = crate::browser_scroll::FixedListScrollHandle::new(
        state.clone(),
        row_count,
        layout.row_height,
    );
    let cards = Arc::new(cards.to_vec());
    let host = host.clone();
    let snapshot = Rc::new(CardsRenderSnapshot::from_view(view, None));
    let section_identity: Arc<str> = section_identity.to_owned().into();
    let list = gpui::list(state.clone(), move |row_index, _window, _app| {
        let range = virtualization::card_grid_row_range(cards.len(), layout.columns, row_index)
            .expect("card grid list requested an out-of-range row");
        let start = range.start;
        let end = range.end;
        div()
            .w_full()
            .h(layout.row_height)
            .flex_none()
            .pb(px(layout.row_gap))
            .child(cards_with_snapshot(
                &snapshot,
                &host,
                &cards[start..end],
                &section_identity,
                row_index,
                layout.columns as u16,
                None,
                false,
                narrow,
                start,
            ))
            .into_any_element()
    });
    let content = div()
        .w_full()
        .flex_1()
        .min_h_0()
        .relative()
        .child(list.w_full().h_full().min_h_0())
        .child(super::virtualization::library_vertical_scrollbar(
            &fixed_scroll,
            narrow,
        ))
        .into_any_element();
    browser_scroll_surface(
        "library-card-grid-scroll",
        content,
        BrowserScrollTarget::FixedList(fixed_scroll),
        browser_scroll,
    )
}

pub(super) fn card_content_identity(card: &Card) -> String {
    fn field(value: &str) -> String {
        format!("{}:{value}", value.len())
    }

    format!(
        "card:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        card.source.label(),
        card.kind.label(),
        field(&card.id),
        field(&card.title),
        field(&card.subtitle),
        field(&card.artwork),
        field(&card.badge),
        field(&card.service_url),
        match card.is_private {
            Some(true) => "1",
            Some(false) => "0",
            None => "-",
        },
        card.library_service
            .map(|service| service.label())
            .unwrap_or("provider"),
    )
}

fn card_route_identity(view: &LibraryView, section_identity: &str) -> String {
    let route = view.state.route();
    let similar_artist = (section_identity == "similar-artists")
        .then(|| {
            view.similar_artists
                .current
                .as_ref()
                .map(|entry| format!(":{}:{}", entry.artist_id, entry.title))
        })
        .flatten()
        .unwrap_or_default();
    format!(
        "{}:{}:{}:{}:{}{}",
        route.source.label(),
        route.category.label(),
        route.action,
        route.id,
        section_identity,
        similar_artist,
    )
}

pub(super) fn card_scroll_identity(
    view: &LibraryView,
    section_identity: &str,
    cards: &[Card],
) -> String {
    virtualization::content_identity(
        &card_route_identity(view, section_identity),
        "carousel",
        cards.iter().map(card_content_identity),
    )
}

fn card_grid_content_key(
    view: &LibraryView,
    section_identity: &str,
    query: &str,
    cards: &[Card],
) -> String {
    virtualization::content_identity(
        &card_route_identity(view, section_identity),
        query,
        cards.iter().map(card_content_identity),
    )
}

pub(super) fn cards_with_snapshot(
    snapshot: &CardsRenderSnapshot,
    host: &gpui::Entity<LibraryView>,
    cards: &[Card],
    section_identity: &str,
    section_index: usize,
    columns: u16,
    preview_limit: Option<usize>,
    card_row: bool,
    narrow: bool,
    card_index_base: usize,
) -> AnyElement {
    let (card_width, row_gap) = if card_row {
        card_row_metrics(narrow)
    } else if snapshot.shared_provider_artist_visual {
        (0., crate::collection_detail::DETAIL_CARD_GRID_GAP_PX)
    } else {
        (0., virtualization::card_grid_gap(narrow))
    };
    let displayed_count = if card_row {
        preview_limit.map_or(cards.len(), |limit| {
            card_carousel_display_count(cards.len(), limit)
        })
    } else {
        preview_limit.unwrap_or(cards.len()).min(cards.len())
    };
    let cards = cards
        .iter()
        .take(displayed_count)
        .enumerate()
        .map(|(index, card)| {
            let actionable = is_card_actionable(card);
            let card_content = div()
                .id(format!(
                    "library-card-open-{}-{}",
                    card.kind.action(),
                    card.id
                ))
                .when(actionable, |this| {
                    let open_click = card.clone();
                    let open_key = open_click.clone();
                    this.focusable()
                        .tab_stop(true)
                        .role(gpui::Role::Button)
                        .aria_label(if card.kind == Category::Flow {
                            format!("Play {}", card.title)
                        } else {
                            format!("Open {}", card.title)
                        })
                        .cursor_pointer()
                        .focus_visible(|style| {
                            style.border_1().border_color(rgb(crate::theme::PRIMARY))
                        })
                        .on_click({
                            let host = host.clone();
                            move |_, window, app| {
                                activate_card(&host, open_click.clone(), window, app);
                            }
                        })
                        .on_key_down({
                            let host = host.clone();
                            move |event: &KeyDownEvent, window, app| {
                                if crate::tab_keyboard::is_activation_key(
                                    event.keystroke.key.as_str(),
                                ) {
                                    window.prevent_default();
                                    activate_card(&host, open_key.clone(), window, app);
                                }
                            }
                        })
                })
                .child(if snapshot.shared_provider_artist_visual {
                    collection_card_content(
                        &card.title,
                        &card.subtitle,
                        &card.artwork,
                        card_kind(card.kind),
                        card.source,
                        &card.badge,
                        None,
                        &card.id,
                    )
                } else if card.library_service == Some(crate::library::Service::Local) {
                    local_collection_card(
                        &card.title,
                        &card.subtitle,
                        super::local_playlist_artwork::resolve_current(&card.id, &card.artwork),
                        card_kind(card.kind),
                        Some(&card.badge),
                        card.is_private,
                        &card.id,
                    )
                } else {
                    collection_card(
                        &card.title,
                        &card.subtitle,
                        &card.artwork,
                        card_kind(card.kind),
                        None,
                        Some(&card.badge),
                        card.is_private,
                        &card.id,
                    )
                });
            let element = if snapshot.shared_provider_artist_visual {
                collection_card_frame(card_row, narrow, card_content.into_any_element())
                    .id(format!("library-card-{}-{}", card.kind.action(), card.id))
            } else {
                div()
                    .id(format!("library-card-{}-{}", card.kind.action(), card.id))
                    .min_w_0()
                    .when(card_row, |this| this.w(px(card_width)).flex_none())
                    .p(px(8.))
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .child(card_content)
            };
            let element = match card.kind {
                Category::Artists => {
                    if let Some(entity) = context_menu::library_card_entity(card, false) {
                        context_menu::artist_card_menu(
                            element,
                            entity,
                            host.clone(),
                            snapshot.favorite_root(Category::Artists),
                        )
                        .into_any_element()
                    } else {
                        element.into_any_element()
                    }
                }
                Category::Albums => {
                    if let Some(entity) = context_menu::library_card_entity(card, true) {
                        context_menu::library_card_menu(
                            element,
                            entity,
                            card.clone(),
                            host.clone(),
                            snapshot.favorite_root(card.kind),
                        )
                        .into_any_element()
                    } else {
                        element.into_any_element()
                    }
                }
                Category::Playlists
                    if card.library_service == Some(crate::library::Service::Local) =>
                {
                    context_menu::local_playlist_card_menu(element, card.clone(), host.clone())
                        .into_any_element()
                }
                Category::Playlists => {
                    if let Some(entity) = context_menu::library_card_entity(card, true) {
                        context_menu::library_card_menu(
                            element,
                            entity,
                            card.clone(),
                            host.clone(),
                            snapshot.favorite_root(card.kind),
                        )
                        .into_any_element()
                    } else {
                        element.into_any_element()
                    }
                }
                Category::Flow => {
                    // The primary click plays the mix, so the flow page
                    // stays reachable through the context menu Open entry.
                    let play_host = host.clone();
                    let play_card = card.clone();
                    let open_host = host.clone();
                    let open_card = card.clone();
                    context_menu::discover_card_menu_with_actions(
                        element,
                        card.title.clone(),
                        vec![
                            context_menu::DiscoverMenuAction::new(
                                context_menu::DiscoverMenuPrimary::PlayFlow,
                                move |_, _, app| {
                                    play_host.update(app, |this, cx| {
                                        this.start_deezer_flow(play_card.clone(), false, cx)
                                    });
                                },
                            ),
                            context_menu::DiscoverMenuAction::new(
                                context_menu::DiscoverMenuPrimary::Open,
                                move |_, _, app| {
                                    open_host.update(app, |this, cx| {
                                        this.open_card(open_card.clone(), cx)
                                    });
                                },
                            ),
                        ],
                    )
                    .into_any_element()
                }
                Category::Station | Category::Tracks | Category::History | Category::MyTracks => {
                    element.into_any_element()
                }
            };
            if snapshot.grid_visual.animating && !card_row {
                animate_grid_card(
                    element,
                    card_index_base + index,
                    snapshot.grid_visual,
                    row_gap,
                    snapshot.grid_height_extra,
                )
            } else {
                element
            }
        })
        .collect::<Vec<_>>();
    if card_row {
        let controls_available = card_carousel_has_overflow(
            cards.len(),
            card_width,
            row_gap,
            snapshot.available_width,
            0.,
        );
        let scroll_id =
            horizontal_scroll_id("library-card-row-scroll", section_identity, section_index);
        let carousel_state = snapshot
            .carousel_state
            .clone()
            .expect("card-row snapshots must include carousel state");
        horizontal_scroll_boundary(
            format!("{scroll_id}-boundary"),
            card_carousel(
                scroll_id,
                carousel_state,
                card_width,
                row_gap,
                controls_available,
                collection_card_carousel_content(
                    row_gap,
                    snapshot.shared_provider_artist_visual,
                    cards,
                )
                .into_any_element(),
            ),
        )
        .into_any_element()
    } else {
        div()
            .w_full()
            .grid()
            .grid_cols(columns)
            .gap(px(row_gap))
            .children(cards)
            .into_any_element()
    }
}

fn activate_card(host: &gpui::Entity<LibraryView>, card: Card, window: &mut Window, app: &mut App) {
    let _ = window;
    host.update(app, |this, cx| {
        // Deezer behavior: the primary click on a Flow card starts the
        // mix playing; the context menu keeps Open for the flow page.
        match card_activation(card.kind) {
            CardActivation::PlayFlow => this.start_deezer_flow(card, false, cx),
            CardActivation::Open => this.open_card(card, cx),
        }
    });
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CardActivation {
    Open,
    PlayFlow,
}

fn card_activation(kind: Category) -> CardActivation {
    if kind == Category::Flow {
        CardActivation::PlayFlow
    } else {
        CardActivation::Open
    }
}

pub(super) fn provider_artist_detail(view: &LibraryView) -> bool {
    is_provider_artist_route(view.state.service, view.state.route())
}

fn is_provider_artist_route(service: super::model::Service, route: &super::model::Route) -> bool {
    service != super::model::Service::Local
        && !route.is_local_route()
        && route.category == Category::Artists
        && matches!(route.action.as_str(), "artist" | "artistTracks")
}

fn is_card_actionable(card: &Card) -> bool {
    !card.id.is_empty()
        && matches!(
            card.kind,
            Category::Albums
                | Category::Artists
                | Category::Playlists
                | Category::Flow
                | Category::Station
        )
}

fn card_kind(kind: Category) -> CardKind {
    match kind {
        Category::Albums => CardKind::Album,
        Category::Artists => CardKind::Artist,
        Category::Flow => CardKind::Flow,
        Category::Playlists => CardKind::Playlist,
        _ => CardKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CardActivation, card_activation, card_content_identity, card_kind, is_card_actionable,
        is_provider_artist_route,
    };
    use crate::library::model::{Card, Category, Route, Service};
    use crate::music_ui::CardKind;

    #[test]
    fn flow_cards_play_while_other_cards_open() {
        assert_eq!(card_activation(Category::Flow), CardActivation::PlayFlow);
        for category in [
            Category::Albums,
            Category::Artists,
            Category::Playlists,
            Category::Station,
        ] {
            assert_eq!(card_activation(category), CardActivation::Open);
        }
    }

    #[test]
    fn card_actionable_matches_route_categories_and_non_empty_id() {
        assert!(is_card_actionable(&Card {
            kind: Category::Albums,
            id: "123".into(),
            ..Card::default()
        }));
        assert!(!is_card_actionable(&Card {
            kind: Category::Albums,
            id: String::new(),
            ..Card::default()
        }));
        assert!(!is_card_actionable(&Card {
            kind: Category::Tracks,
            id: "123".into(),
            ..Card::default()
        }));
    }

    #[test]
    fn flow_cards_use_the_dedicated_centered_card_kind() {
        assert!(matches!(card_kind(Category::Flow), CardKind::Flow));
        assert!(matches!(card_kind(Category::Albums), CardKind::Album));
    }

    #[test]
    fn only_provider_artist_details_use_the_search_card_visual() {
        let provider_route = Route {
            category: Category::Artists,
            action: "artist".into(),
            id: "42".into(),
            ..Route::root(Service::Deezer, Category::Artists)
        };
        assert!(is_provider_artist_route(Service::Deezer, &provider_route));
        assert!(!is_provider_artist_route(Service::Local, &provider_route));

        let root_route = Route::root(Service::Deezer, Category::Artists);
        assert!(!is_provider_artist_route(Service::Deezer, &root_route));
    }

    #[test]
    fn card_identity_includes_ordered_display_data_and_artwork() {
        let card = Card {
            kind: Category::Artists,
            id: "42".into(),
            title: "Artist".into(),
            subtitle: "Genre".into(),
            artwork: "art-a".into(),
            badge: "12".into(),
            ..Card::default()
        };
        let mut artwork_changed = card.clone();
        artwork_changed.artwork = "art-b".into();
        let mut title_changed = card.clone();
        title_changed.title = "Renamed Artist".into();
        let mut privacy_changed = card.clone();
        privacy_changed.is_private = Some(true);

        assert_eq!(card_content_identity(&card), card_content_identity(&card));
        assert_ne!(
            card_content_identity(&card),
            card_content_identity(&artwork_changed)
        );
        assert_ne!(
            card_content_identity(&card),
            card_content_identity(&title_changed)
        );
        assert_ne!(
            card_content_identity(&card),
            card_content_identity(&privacy_changed)
        );
    }
}
