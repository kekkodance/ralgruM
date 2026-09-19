use super::*;

pub(super) fn render_section_page_list(
    view: &LibraryView,
    page: &Page,
    columns: u16,
    available_width: f32,
    narrow: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let host = cx.entity();
    let route = view.state.route();
    let mut builders = Vec::<super::super::virtualization::PageItemBuilder>::new();
    let mut page_items = Vec::<super::super::virtualization::PageItem>::new();
    let mut item_identities = Vec::<String>::new();

    let removal = if route.is_local_playlist_detail() && page.playlist_removal_proven() {
        Some((route.id.clone(), true, true))
    } else if route.source == crate::search::Provider::Deezer
        && route.action == "playlistTracks"
        && page.playlist_removal_proven()
    {
        Some((route.id.clone(), true, false))
    } else {
        None
    };
    let playback_context = playback_context(view);
    let route_key = format!(
        "{}:{}:{}:{}:sections",
        route.source.label(),
        route.category.label(),
        route.action,
        route.id
    );
    let mut ordered_rows = Vec::new();
    for (section_index, section) in page.sections.iter().enumerate() {
        ordered_rows.extend(section.tracks.iter().map(|track| {
            format!(
                "track:{section_index}:{}",
                super::super::track_view::library_track_identity(track)
            )
        }));
        ordered_rows.extend(section.cards.iter().map(|card| {
            format!(
                "card:{section_index}:{}",
                super::super::cards_view::card_content_identity(card)
            )
        }));
    }
    let filter_identity = view.query(cx);
    let content_key =
        super::super::virtualization::content_identity(&route_key, &filter_identity, ordered_rows);
    let card_layout = super::super::virtualization::CardGridLayout::new_for_visual(
        columns,
        available_width,
        narrow,
        super::super::cards_view::provider_artist_detail(view),
    );
    let has_unbounded_cards = page.sections.iter().any(|section| {
        section.layout == SectionLayout::Cards
            && section.preview_limit.is_none()
            && !section.cards.is_empty()
    });

    for (section_index, section) in page.sections.iter().enumerate() {
        let section_header = Arc::new(section.clone());
        page_items.push(super::super::virtualization::PageItem::Header(
            section_index,
        ));
        item_identities.push(format!(
            "header:{section_index}:{}:{}:{}:{}:{}:{}:{}",
            section.title,
            section.description,
            section.total,
            section.show_count,
            section.layout == SectionLayout::Cards,
            section.preview_limit.unwrap_or(usize::MAX),
            section.card_row,
        ));
        builders.push(Rc::new(move |_, _| render_section_header(&section_header)));
        if section.tracks.is_empty() && section.cards.is_empty() {
            let empty_section = Arc::new(section.clone());
            page_items.push(super::super::virtualization::PageItem::Tail(section_index));
            item_identities.push(format!(
                "empty:{section_index}:{}",
                empty_section.empty_message
            ));
            builders.push(Rc::new(move |_, _| render_section_empty(&empty_section)));
            continue;
        }
        match section.layout {
            SectionLayout::Tracks => {
                let rows = Rc::new(super::super::track_view::LibraryTrackRows::new(
                    &section.tracks,
                    section.preview_limit,
                    narrow,
                    view.track_favorites_available(),
                    view.playing.clone(),
                    view.favorites.clone(),
                    removal.clone(),
                    view.playlists.remove_pending,
                    None,
                    false,
                    route.source,
                    view.external_track_navigation_openers(),
                    view.track_favorites_available()
                        && view.state.selected_root_active(Category::Tracks),
                    playback_context.clone(),
                    view.playback.clone(),
                    view.downloads.clone(),
                    view.account.clone(),
                    album_navigation_context(view),
                    format!("section-{section_index}"),
                    cx,
                ));
                for index in 0..rows.len() {
                    let rows = rows.clone();
                    let identity = section
                        .tracks
                        .get(index)
                        .map(super::super::track_view::library_track_identity)
                        .unwrap_or_else(|| format!("row-{index}"));
                    page_items.push(super::super::virtualization::PageItem::Track(index));
                    item_identities.push(format!("track:{section_index}:{identity}"));
                    builders.push(Rc::new(move |_, app| {
                        div()
                            .w_full()
                            .h(super::super::virtualization::row_height())
                            .flex_none()
                            .child(rows.render(index, app))
                            .into_any_element()
                    }));
                }
            }
            SectionLayout::Cards => {
                if section.preview_limit.is_none() {
                    let cards = Arc::new(section.cards.clone());
                    let section_identity: Arc<str> = format!("section-{section_index}").into();
                    let cards_snapshot = Rc::new(
                        super::super::cards_view::CardsRenderSnapshot::from_view(view, None),
                    );
                    let row_count = super::super::virtualization::card_grid_row_count(
                        cards.len(),
                        card_layout.columns,
                    );
                    for row_index in 0..row_count {
                        let range = super::super::virtualization::card_grid_row_range(
                            cards.len(),
                            card_layout.columns,
                            row_index,
                        )
                        .expect("card grid section row must contain cards");
                        let row_identity = format!(
                            "card-row:{section_index}:{row_index}:{}",
                            cards[range.clone()]
                                .iter()
                                .map(super::super::cards_view::card_content_identity)
                                .collect::<Vec<_>>()
                                .join("|")
                        );
                        let cards = cards.clone();
                        let host = host.clone();
                        let cards_snapshot = cards_snapshot.clone();
                        let section_identity = section_identity.clone();
                        page_items.push(super::super::virtualization::PageItem::CardRow {
                            section_index,
                            row_index,
                        });
                        item_identities.push(row_identity);
                        builders.push(Rc::new(move |_, _app| {
                            let start = row_index * card_layout.columns;
                            let end = (start + card_layout.columns).min(cards.len());
                            div()
                                .w_full()
                                .when(end < cards.len(), |this| this.pb(px(card_layout.row_gap)))
                                .child(super::super::cards_view::cards_with_snapshot(
                                    &cards_snapshot,
                                    &host,
                                    &cards[start..end],
                                    &section_identity,
                                    row_index,
                                    card_layout.columns as u16,
                                    None,
                                    false,
                                    narrow,
                                    start,
                                ))
                                .into_any_element()
                        }));
                    }
                } else {
                    let section = Arc::new(section.clone());
                    let host = host.clone();
                    let section_identity = super::super::cards_view::card_scroll_identity(
                        view,
                        &format!("section-{section_index}"),
                        &section.cards,
                    );
                    let scroll_id = section.card_row.then(|| {
                        crate::music_ui::horizontal_scroll_id(
                            "library-card-row-scroll",
                            &section_identity,
                            section_index,
                        )
                    });
                    let cards_snapshot =
                        Rc::new(super::super::cards_view::CardsRenderSnapshot::from_view(
                            view,
                            scroll_id.as_deref(),
                        ));
                    page_items.push(super::super::virtualization::PageItem::Tail(section_index));
                    item_identities.push(format!(
                        "bounded-cards:{section_index}:{}",
                        section
                            .cards
                            .iter()
                            .map(super::super::cards_view::card_content_identity)
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                    builders.push(Rc::new(move |_, _app| {
                        super::super::cards_view::cards_with_snapshot(
                            &cards_snapshot,
                            &host,
                            &section.cards,
                            &section_identity,
                            section_index,
                            columns,
                            section.preview_limit,
                            section.card_row,
                            narrow,
                            0,
                        )
                    }));
                }
            }
        }
    }
    debug_assert_eq!(page_items.len(), builders.len());
    let (state, browser_scroll) = if has_unbounded_cards {
        let state = view.mixed_page_state_with_rows(
            &content_key,
            page_items.len(),
            card_layout,
            &page_items,
            &item_identities,
        );
        let browser_scroll = view.card_grid_browser_scroll(&content_key);
        (state, browser_scroll)
    } else {
        let layout = super::super::virtualization::TrackListLayout::new(narrow, false, true);
        let state = view.track_list_state_with_rows(
            &content_key,
            page_items.len(),
            layout,
            &item_identities,
        );
        let browser_scroll = view.track_list_browser_scroll(&content_key);
        (state, browser_scroll)
    };
    // The section page mixes row kinds (headers, track slots, card grid
    // rows), so the scroll math uses the aggregate page height: the average
    // is exact in total because it is derived from the same per-kind
    // heights. The raw ListState cannot feed the scrollbar because gpui
    // discards size hints on width changes, which would leave unmeasured
    // rows at zero height.
    let page_height =
        super::super::virtualization::page_item_uniform_height(&page_items, card_layout);
    let fixed_scroll = FixedListScrollHandle::new(state.clone(), page_items.len(), page_height);
    browser_scroll_surface(
        "library-section-page-list-scroll",
        super::super::virtualization::page_list(
            state,
            fixed_scroll.clone(),
            Rc::new(builders),
            narrow,
        ),
        BrowserScrollTarget::FixedList(fixed_scroll),
        browser_scroll,
    )
}

fn render_section_header(section: &Section) -> AnyElement {
    div()
        .flex()
        .items_end()
        .justify_between()
        .gap(px(16.))
        .mb(px(11.))
        .when(
            !section.title.is_empty() || !section.description.is_empty(),
            |this| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.5))
                                .font_weight(FontWeight(600.))
                                .line_height(px(18.85))
                                .child(section.title.clone()),
                        )
                        .when(!section.description.is_empty(), |this| {
                            this.child(
                                div()
                                    .mt(px(3.))
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .line_height(px(16.1))
                                    .text_color(rgb(MUTED))
                                    .child(section.description.clone()),
                            )
                        }),
                )
            },
        )
        .when(section_count_visible(section), |this| {
            this.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} item{}",
                        section.total,
                        if section.total == 1 { "" } else { "s" }
                    )),
            )
        })
        .into_any_element()
}

fn render_section_empty(section: &Section) -> AnyElement {
    div()
        .text_color(rgb(MUTED))
        .child(if section.empty_message.is_empty() {
            "No items in this section.".to_owned()
        } else {
            section.empty_message.clone()
        })
        .into_any_element()
}

pub(super) fn playback_context(view: &LibraryView) -> crate::playback::PlaybackContext {
    let route = view.state.route();
    let deezer_load_id = view.state.active_tracks_load_id();
    let station_seed = (route.action == "stationTracks")
        .then(|| station_seed_from_page(view.state.page.as_ref()))
        .flatten();
    playback_context_for_route_with_kind(
        route,
        view.flow_mode,
        view.state
            .page
            .as_ref()
            .and_then(|page| page.next_flow_tuner.clone()),
        deezer_load_id,
        station_seed,
        view.flow_detail_kind(),
    )
}

fn render_library_section_header(section: &Section, action: Option<AnyElement>) -> AnyElement {
    div()
        .flex()
        .items_end()
        .justify_between()
        .gap(px(16.))
        .mb(px(11.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.5))
                        .font_weight(FontWeight(600.))
                        .line_height(px(18.85))
                        .child(section.title.clone()),
                )
                .when(!section.description.is_empty(), |this| {
                    this.child(
                        div()
                            .mt(px(3.))
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.5))
                            .line_height(px(16.1))
                            .text_color(rgb(MUTED))
                            .child(section.description.clone()),
                    )
                }),
        )
        .when(section_count_visible(section), |this| {
            this.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "{} item{}",
                        section.total,
                        if section.total == 1 { "" } else { "s" }
                    )),
            )
        })
        .when_some(action, |this, action| this.child(action))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_section(
    view: &LibraryView,
    section: &Section,
    section_index: usize,
    columns: u16,
    narrow: bool,
    favorite_state: gpui::Entity<super::super::favorite_state::FavoriteState>,
    removal: Option<(String, bool, bool)>,
    removal_pending: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let tracks_only = is_artist_detail(view)
        && view.state.page.as_ref().is_some_and(|page| {
            soundcloud_tracks_only_page(view.state.route().source, &view.state.route().action, page)
        });
    let expanded = is_artist_detail(view)
        && !tracks_only
        && view.artist_section_expanded == Some(section_index);
    let artist_section = is_artist_detail(view);
    let preview_limit = if expanded || tracks_only {
        None
    } else {
        section.preview_limit
    };
    // Search expands artist card sections into the full grid. Keep the
    // library surface on the same layout instead of leaving the expanded
    // section in the preview carousel.
    let card_row = card_row_for_artist_section(section.card_row, artist_section, expanded);
    let section_action =
        artist_section_action(view, section, section_index, expanded, tracks_only, cx);
    let track_favorites_available = view.track_favorites_available()
        && (view.state.selected_root_active(Category::Tracks) || is_artist_detail(view));
    let body = div()
        .when(
            section.tracks.is_empty() && section.cards.is_empty(),
            |this| {
                this.child(div().text_color(rgb(MUTED)).child(
                    if section.empty_message.is_empty() {
                        "No items in this section.".to_owned()
                    } else {
                        section.empty_message.clone()
                    },
                ))
            },
        )
        .when(
            !section.tracks.is_empty() || !section.cards.is_empty(),
            |this| {
                this.child(match section.layout {
                    SectionLayout::Tracks => super::super::track_view::tracks(
                        view,
                        &section.tracks,
                        preview_limit,
                        narrow,
                        track_favorites_available,
                        view.playing.clone(),
                        favorite_state,
                        removal,
                        removal_pending,
                        None,
                        false,
                        view.state.route().source,
                        view.external_track_navigation_openers(),
                        track_favorites_available,
                        playback_context(view),
                        view.playback.clone(),
                        view.downloads.clone(),
                        view.account.clone(),
                        album_navigation_context(view),
                        &format!("section-{section_index}"),
                        cx,
                    ),
                    SectionLayout::Cards => super::super::cards_view::cards(
                        view,
                        &cx.entity(),
                        &section.cards,
                        &view.state.route().action,
                        section_index,
                        columns,
                        preview_limit,
                        card_row,
                        narrow,
                        cx,
                    ),
                })
            },
        )
        .into_any_element();
    if artist_section {
        render_shared_artist_section(
            &section.title,
            section.total,
            section.tracks.len() + section.cards.len(),
            expanded,
            tracks_only,
            section_action,
            body,
        )
    } else {
        div()
            .flex()
            .flex_col()
            .when(
                !section.title.is_empty()
                    || !section.description.is_empty()
                    || section_count_visible(section),
                |this| this.child(render_library_section_header(section, section_action)),
            )
            .child(body)
            .into_any_element()
    }
}
