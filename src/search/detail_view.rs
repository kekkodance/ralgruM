use gpui::{AnyElement, KeyDownEvent, SharedString, div, prelude::*, px, rgb};
use std::sync::Arc;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    collection_detail::{
        ArtistHeaderSpec, DETAIL_SECTION_GAP_PX, ProviderHeaderSpec,
        render_artist_header as render_shared_artist_header,
        render_artist_section as render_shared_artist_section,
        render_artist_section_header as render_shared_artist_section_header,
        render_provider_detail as render_shared_provider_detail,
    },
    context_menu::{card_entity, card_menu_button},
    music_ui::{playlist_header_action_button, playlist_header_more_button},
    theme::{BORDER, DEEZER, FOREGROUND, MUTED, SOUNDCLOUD, SURFACE},
};

const FAVORITE_PINK: u32 = 0xec4899;
use super::{
    detail::{
        ArtistSection, DetailRoute, DetailState, artist_section_should_render,
        artist_section_shows_action, soundcloud_tracks_only,
    },
    models::{Card, Provider, ResultType, Track},
    view::SearchView,
};

impl SearchView {
    pub(super) fn detail_content(
        &self,
        columns: u16,
        available_width: f32,
        available_height: f32,
        narrow: bool,
        provider_icon_only: bool,
        cx: &mut gpui::Context<SearchView>,
    ) -> AnyElement {
        let route = self.detail.route.as_ref();
        div()
            .w_full()
            .flex()
            .flex_col()
            .relative()
            .when(self.uses_virtualized_scroll(cx), |this| {
                this.flex_1().min_h_0()
            })
            .gap(px(12.))
            .child(match &self.detail.state {
                DetailState::Closed => div().into_any_element(),
                DetailState::Loading => super::skeleton::render(
                    ResultType::All,
                    route,
                    columns,
                    narrow,
                    provider_icon_only,
                    available_width,
                    available_height,
                ),
                DetailState::AccountRequired => {
                    let (color, copy) = match route.map(|route| route.provider) {
                        Some(Provider::SoundCloud) => (
                            SOUNDCLOUD,
                            "Log in to SoundCloud from Settings to open this result.",
                        ),
                        _ => (
                            DEEZER,
                            "Log in to Deezer from Settings to open this result.",
                        ),
                    };
                    detail_message(LocalIcon::UserLock, color, "Account required", copy)
                }
                DetailState::Failed(error) => detail_message(
                    LocalIcon::TriangleExclamation,
                    DEEZER,
                    "Could not open result",
                    error,
                ),
                DetailState::Empty(page) => {
                    let route = &page.route;
                    let editable = route.kind == ResultType::Playlists
                        && self.playlist_editable(route.provider, &route.id, cx);
                    render_page(
                        self,
                        route,
                        &page.tracks,
                        page.total.unwrap_or(page.tracks.len()),
                        narrow,
                        self.favorites.clone(),
                        self.playback.clone(),
                        self.downloads.clone(),
                        self.account.clone(),
                        editable,
                        self.library
                            .read(cx)
                            .playlist_catalog(route.provider)
                            .remove_pending,
                        if route.kind == ResultType::Albums {
                            ""
                        } else {
                            &page.description
                        },
                        self.playing.clone(),
                        cx,
                    )
                }
                DetailState::Results(page) => {
                    if let Some(artist) = &page.artist {
                        render_artist_page(
                            &page.route,
                            artist,
                            self.detail.expanded_artist_section,
                            columns,
                            available_width,
                            narrow,
                            provider_icon_only,
                            self.favorites.clone(),
                            self.playback.clone(),
                            self.downloads.clone(),
                            self.account.clone(),
                            self.playing.clone(),
                            self,
                            cx,
                        )
                    } else {
                        let playlist_editable = page.route.kind == ResultType::Playlists
                            && self.playlist_editable(page.route.provider, &page.route.id, cx);
                        render_page(
                            self,
                            &page.route,
                            &page.tracks,
                            page.total.unwrap_or(page.tracks.len()),
                            narrow,
                            self.favorites.clone(),
                            self.playback.clone(),
                            self.downloads.clone(),
                            self.account.clone(),
                            playlist_editable,
                            self.library
                                .read(cx)
                                .playlist_catalog(page.route.provider)
                                .remove_pending,
                            if page.route.kind == ResultType::Albums {
                                ""
                            } else {
                                &page.description
                            },
                            self.playing.clone(),
                            cx,
                        )
                    }
                }
            })
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_artist_page(
    route: &DetailRoute,
    artist: &super::models::ArtistPage,
    expanded: Option<ArtistSection>,
    columns: u16,
    available_width: f32,
    narrow: bool,
    provider_icon_only: bool,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    playback: gpui::Entity<crate::playback::PlaybackModel>,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    account: gpui::Entity<crate::settings::AccountState>,
    playing: crate::playing_indicator::PlayingSnapshot,
    view: &SearchView,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    let tracks_only = soundcloud_tracks_only(route.provider, artist);
    let expanded = if tracks_only { None } else { expanded };
    let host = cx.entity();
    let mut page = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(DETAIL_SECTION_GAP_PX))
        .when(expanded.is_some(), |this| this.flex_1().min_h_0())
        .child(artist_header(
            route,
            &artist.profile,
            artist.fans,
            favorites.clone(),
            account.clone(),
            &host,
            cx,
        ));
    if expanded.is_some() {
        return page
            .child(render_artist_section_list(
                route,
                artist,
                expanded,
                columns,
                available_width,
                narrow,
                provider_icon_only,
                favorites,
                playback,
                downloads,
                account,
                playing,
                view,
                cx,
            ))
            .into_any_element();
    }
    let popular_identity = format!(
        "artist-popular-tracks:{}:{}:{}:{}",
        route.provider.label(),
        route.id,
        route.title,
        view.detail.view_id()
    );
    if artist_section_should_render(
        ArtistSection::PopularTracks,
        expanded,
        artist.popular_tracks.len(),
    ) {
        page = page.child(artist_section(
            artist_tracks_title(),
            ArtistSection::PopularTracks,
            artist.popular_total,
            artist.popular_tracks.len(),
            expanded,
            tracks_only,
            super::rows_view::render_tracks(
                view,
                &artist.popular_tracks,
                artist_tracks_preview(tracks_only, expanded),
                narrow,
                provider_icon_only,
                false,
                artist_popular_actions(route.provider),
                favorites.clone(),
                None,
                false,
                playback.clone(),
                downloads.clone(),
                account.clone(),
                Some(navigation_context(route)),
                playing.clone(),
                &popular_identity,
                cx,
            ),
            cx,
        ));
    }
    if artist_section_should_render(
        ArtistSection::SimilarArtists,
        expanded,
        artist.similar_artists.len(),
    ) {
        page = page.child(artist_cards_section(
            "Similar Artists",
            0,
            ArtistSection::SimilarArtists,
            artist.similar_total,
            &artist.similar_artists,
            expanded,
            columns,
            available_width,
            narrow,
            favorites.clone(),
            account.clone(),
            view,
            cx,
        ));
    }
    if artist_section_should_render(ArtistSection::Albums, expanded, artist.albums.len()) {
        page = page.child(artist_cards_section(
            "Albums",
            1,
            ArtistSection::Albums,
            artist.albums_total,
            &artist.albums,
            expanded,
            columns,
            available_width,
            narrow,
            favorites.clone(),
            account.clone(),
            view,
            cx,
        ));
    }
    if artist_section_should_render(ArtistSection::Featured, expanded, artist.featured.len()) {
        page = page.child(artist_cards_section(
            "Featured in",
            2,
            ArtistSection::Featured,
            artist.featured_total,
            &artist.featured,
            expanded,
            columns,
            available_width,
            narrow,
            favorites.clone(),
            account.clone(),
            view,
            cx,
        ));
    }
    if artist_section_should_render(ArtistSection::Playlists, expanded, artist.playlists.len()) {
        page = page.child(artist_cards_section(
            "Playlists",
            3,
            ArtistSection::Playlists,
            artist.playlists_total,
            &artist.playlists,
            expanded,
            columns,
            available_width,
            narrow,
            favorites,
            account,
            view,
            cx,
        ));
    }
    page.into_any_element()
}

fn render_artist_section_list(
    route: &DetailRoute,
    artist: &super::models::ArtistPage,
    expanded: Option<ArtistSection>,
    columns: u16,
    available_width: f32,
    narrow: bool,
    provider_icon_only: bool,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    playback: gpui::Entity<crate::playback::PlaybackModel>,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    account: gpui::Entity<crate::settings::AccountState>,
    playing: crate::playing_indicator::PlayingSnapshot,
    view: &SearchView,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    let host = cx.entity();
    let popular_tracks = artist.popular_tracks.clone();
    let mut builders = Vec::<crate::library::virtualization::PageItemBuilder>::new();
    let mut ordered_rows = Vec::new();
    let tracks_only = soundcloud_tracks_only(route.provider, artist);
    let tracks_title = artist_tracks_title();

    if artist_section_should_render(ArtistSection::PopularTracks, expanded, popular_tracks.len()) {
        let popular_total = artist.popular_total;
        let popular_count = popular_tracks.len();
        let section_host = host.clone();
        builders.push(Arc::new(move |_, app| {
            div()
                .w_full()
                .pb(px(9.))
                .child(render_artist_section_heading(
                    tracks_title,
                    ArtistSection::PopularTracks,
                    popular_total,
                    popular_count,
                    expanded,
                    tracks_only,
                    &section_host,
                    app,
                ))
                .into_any_element()
        }));
        ordered_rows.push(format!("section:{tracks_title}"));
        let rows = Arc::new(super::rows_view::SearchTrackRows::new(
            view,
            &popular_tracks,
            false,
            narrow,
            provider_icon_only,
            false,
            artist_popular_actions(route.provider),
            favorites.clone(),
            None,
            false,
            playback.clone(),
            downloads.clone(),
            account.clone(),
            Some(navigation_context(route)),
            playing.clone(),
            cx,
        ));
        for index in 0..rows.len() {
            let rows = rows.clone();
            builders.push(Arc::new(move |_, app| {
                super::rows_view::track_row_slot(rows.render(index, app), true)
            }));
            ordered_rows.push(format!(
                "track:{}",
                super::rows_view::search_track_identity(&popular_tracks[index])
            ));
        }
    }

    let card_sections = [
        (
            "Similar Artists",
            ArtistSection::SimilarArtists,
            artist.similar_total,
            artist.similar_artists.clone(),
        ),
        (
            "Albums",
            ArtistSection::Albums,
            artist.albums_total,
            artist.albums.clone(),
        ),
        (
            "Featured in",
            ArtistSection::Featured,
            artist.featured_total,
            artist.featured.clone(),
        ),
        (
            "Playlists",
            ArtistSection::Playlists,
            artist.playlists_total,
            artist.playlists.clone(),
        ),
    ];
    let card_layout = super::cards_view::card_grid_layout(available_width, columns, narrow);
    let card_row_content_height = px((card_layout.row_height - 12.).max(1.));
    let card_grid_visual = view.card_grid_motion.visual();
    let card_height_extra = (card_layout.row_height - card_layout.card_width - 12.).max(0.);
    for (title, section_kind, total, cards) in card_sections {
        if !artist_section_should_render(section_kind, expanded, cards.len()) {
            continue;
        }
        let card_count = cards.len();
        let row_count = super::cards_view::card_grid_row_count(card_count, columns);
        let section_host = host.clone();
        builders.push(Arc::new(move |_, app| {
            div()
                .w_full()
                .pb(px(9.))
                .child(render_artist_section_heading(
                    title,
                    section_kind,
                    total,
                    card_count,
                    expanded,
                    false,
                    &section_host,
                    app,
                ))
                .into_any_element()
        }));
        ordered_rows.push(format!("section:{title}"));
        let cards = Arc::new(cards);
        for row_index in 0..row_count {
            let row_cards = cards.clone();
            let row_host = host.clone();
            let row_account = view.account.clone();
            builders.push(Arc::new(move |_, _app| {
                let range =
                    super::cards_view::card_grid_row_range(row_index, row_cards.len(), columns)
                        .expect("card row index must be in range");
                div()
                    .w_full()
                    .min_h(card_row_content_height)
                    .when(row_index + 1 < row_count, |this| this.pb(px(12.)))
                    .child(super::cards_view::render_card_grid_row(
                        &row_host,
                        &row_cards[range.clone()],
                        columns,
                        narrow,
                        &row_account,
                        card_grid_visual,
                        range.start,
                        card_height_extra,
                    ))
                    .into_any_element()
            }));
            let range = super::cards_view::card_grid_row_range(row_index, card_count, columns)
                .expect("card row index must be in range");
            ordered_rows.push(format!(
                "card-row:{}:{}",
                format!("{section_kind:?}"),
                cards[range]
                    .iter()
                    .map(super::cards_view::stable_card_identity)
                    .collect::<Vec<_>>()
                    .join("|")
            ));
        }
    }
    let route_key = format!(
        "artist-page:{}:{}:{}:{}",
        route.provider.label(),
        route.id,
        route.title,
        view.detail.view_id()
    );
    let identity = crate::library::virtualization::content_identity(&route_key, "", ordered_rows);
    let layout =
        crate::library::virtualization::TrackListLayout::new(narrow, provider_icon_only, true);
    let state = view.track_list_state(&identity, builders.len(), layout);
    let browser_scroll = view.track_list_browser_scroll(&identity);
    browser_scroll_surface(
        "search-artist-section-list-scroll",
        crate::library::virtualization::page_list(state.clone(), Arc::new(builders), narrow),
        BrowserScrollTarget::List(state),
        browser_scroll,
    )
}

fn render_artist_section_heading(
    title: &str,
    section_kind: ArtistSection,
    total: usize,
    loaded_count: usize,
    expanded: Option<ArtistSection>,
    hide_action: bool,
    host: &gpui::Entity<SearchView>,
    _app: &gpui::App,
) -> AnyElement {
    artist_section_header(
        title,
        section_kind,
        total,
        loaded_count,
        expanded,
        host,
        false,
        hide_action,
    )
}

fn artist_header(
    route: &DetailRoute,
    profile: &Card,
    fans: Option<u64>,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    account: gpui::Entity<crate::settings::AccountState>,
    host: &gpui::Entity<SearchView>,
    app: &gpui::App,
) -> AnyElement {
    let mut actions = Vec::new();
    if let Some(action) = detail_favorite(route, favorites, host, app) {
        actions.push(action);
    }
    if let Some(action) = detail_more_button(profile.clone(), account, host.clone()) {
        actions.push(action);
    }
    let subtitle = fans
        .map(|value| crate::search::format_number(value) + " fans")
        .unwrap_or_else(|| route.subtitle.clone());
    render_shared_artist_header(ArtistHeaderSpec {
        artwork: route.artwork.clone(),
        title: route.title.clone(),
        subtitle,
        provider: route.provider,
        actions,
    })
}

fn artist_section_header(
    title: &str,
    section_kind: ArtistSection,
    total: usize,
    loaded_count: usize,
    expanded: Option<ArtistSection>,
    host: &gpui::Entity<SearchView>,
    bottom_margin: bool,
    hide_action: bool,
) -> AnyElement {
    let mut result = render_shared_artist_section_header(
        title,
        total,
        loaded_count,
        expanded == Some(section_kind),
        hide_action,
        artist_section_action(
            title,
            section_kind,
            loaded_count,
            expanded == Some(section_kind),
            host,
        ),
    );
    if bottom_margin {
        result = div().mb(px(9.)).child(result).into_any_element();
    }
    result
}

fn artist_section_action(
    title: &str,
    section_kind: ArtistSection,
    loaded_count: usize,
    is_expanded: bool,
    host: &gpui::Entity<SearchView>,
) -> Option<AnyElement> {
    if !artist_section_shows_action(section_kind, loaded_count, is_expanded) {
        return None;
    }
    let click_host = host.clone();
    let key_host = host.clone();
    Some(
        div()
            .id(format!("artist-section-{title}"))
            .focusable()
            .tab_stop(true)
            .role(gpui::Role::Button)
            .aria_label(if is_expanded { "View less" } else { "View all" })
            .px(px(7.))
            .py(px(3.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SURFACE))
            .text_size(px(11.))
            .text_color(rgb(FOREGROUND))
            .cursor_pointer()
            .on_click(move |_, _, app| {
                click_host.update(app, |this, cx| {
                    this.toggle_artist_section(section_kind, cx);
                });
            })
            .on_key_down(move |event: &KeyDownEvent, window, app| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    key_host.update(app, |this, cx| {
                        this.toggle_artist_section(section_kind, cx);
                    });
                }
            })
            .child(if is_expanded { "View less" } else { "View all" })
            .into_any_element(),
    )
}

#[allow(clippy::too_many_arguments)]
fn artist_section(
    title: &str,
    section_kind: ArtistSection,
    total: usize,
    loaded_count: usize,
    expanded: Option<ArtistSection>,
    hide_action: bool,
    body: AnyElement,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    section_with_action(
        title,
        section_kind,
        total,
        loaded_count,
        expanded,
        hide_action,
        body,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn artist_cards_section(
    title: &'static str,
    section_index: usize,
    section_kind: ArtistSection,
    total: usize,
    cards: &[Card],
    expanded: Option<ArtistSection>,
    columns: u16,
    available_width: f32,
    narrow: bool,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    account: gpui::Entity<crate::settings::AccountState>,
    view: &SearchView,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    let is_expanded = expanded == Some(section_kind);
    let scroll_id =
        crate::music_ui::horizontal_scroll_id("search-card-preview-scroll", title, section_index);
    let carousel_state = view.card_scroll_handle(&scroll_id);
    section_with_action(
        title,
        section_kind,
        total,
        cards.len(),
        expanded,
        false,
        super::cards_view::render_cards(
            view,
            &cx.entity(),
            cards,
            title,
            section_index,
            !is_expanded,
            columns,
            available_width,
            narrow,
            favorites,
            account,
            carousel_state,
        ),
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn section_with_action(
    title: &str,
    section_kind: ArtistSection,
    total: usize,
    loaded_count: usize,
    expanded: Option<ArtistSection>,
    hide_action: bool,
    body: AnyElement,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    let host = cx.entity();
    render_shared_artist_section(
        title,
        total,
        loaded_count,
        expanded == Some(section_kind),
        hide_action,
        artist_section_action(
            title,
            section_kind,
            loaded_count,
            expanded == Some(section_kind),
            &host,
        ),
        body,
    )
}

#[allow(clippy::too_many_arguments)]
fn render_page(
    view: &SearchView,
    route: &DetailRoute,
    tracks: &[Track],
    total: usize,
    narrow: bool,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    playback: gpui::Entity<crate::playback::PlaybackModel>,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    account: gpui::Entity<crate::settings::AccountState>,
    playlist_editable: bool,
    removal_pending: bool,
    description: &str,
    playing: crate::playing_indicator::PlayingSnapshot,
    cx: &mut gpui::Context<SearchView>,
) -> AnyElement {
    let track_identity = format!(
        "detail-tracks:{}:{}:{}:{}:{}",
        route.provider.label(),
        route.kind.label(),
        route.id,
        route.title,
        view.detail.view_id()
    );
    let meta = super::detail_metadata(route);
    let header_favorite = if route.kind == ResultType::Playlists && playlist_editable {
        None
    } else {
        detail_favorite(route, favorites.clone(), &cx.entity(), cx)
    };
    let more_button = detail_more_button(
        detail_route_card(route, total),
        account.clone(),
        cx.entity(),
    );
    let mut actions = Vec::new();
    if let Some(action) = header_favorite {
        actions.push(action);
    }
    if let Some(action) = detail_edit(route, playlist_editable, cx) {
        actions.push(action);
    }
    if let Some(action) = more_button {
        actions.push(action);
    }
    let body = if tracks.is_empty() {
        crate::collection_detail::render_collection_empty(route.kind == ResultType::Playlists)
    } else {
        super::rows_view::render_tracks(
            view,
            tracks,
            false,
            narrow,
            false,
            false,
            artist_popular_actions(route.provider),
            favorites,
            Some((
                route.id.clone(),
                playlist_editable && route.kind == ResultType::Playlists,
            )),
            removal_pending,
            playback,
            downloads,
            account,
            Some(navigation_context(route)),
            playing,
            &track_identity,
            cx,
        )
    };
    render_shared_provider_detail(
        ProviderHeaderSpec {
            artwork: route.artwork.clone(),
            title: route.title.clone(),
            metadata: meta,
            description: description.to_owned(),
            provider: route.provider,
            kind: route.kind,
            total: Some(total),
            actions,
            body_fills: !tracks.is_empty(),
        },
        body,
    )
}

fn navigation_context(route: &DetailRoute) -> crate::entity_navigation::ContextRoute {
    crate::entity_navigation::ContextRoute {
        provider: route.provider,
        action: match route.kind {
            ResultType::Albums => "albumTracks",
            ResultType::Playlists => "playlistTracks",
            ResultType::Artists => {
                if route.provider == Provider::SoundCloud {
                    "artistTracks"
                } else {
                    "artist"
                }
            }
            ResultType::All | ResultType::Tracks => "",
        }
        .into(),
        id: route.id.clone(),
        title: route.title.clone(),
    }
}

fn detail_edit(
    route: &DetailRoute,
    editable: bool,
    cx: &mut gpui::Context<SearchView>,
) -> Option<AnyElement> {
    if route.kind != ResultType::Playlists || !editable {
        return None;
    }
    let id = route.id.clone();
    let provider = route.provider;
    let host = cx.entity();
    Some(playlist_header_action_button(
        format!("search-edit-playlist-{}-{id}", provider.label()).into(),
        LocalIcon::Pen,
        MUTED,
        FOREGROUND,
        false,
        "Edit",
        move |_, window, app| {
            host.update(app, |this, cx| {
                this.open_playlist_editor(provider, id.clone(), window, cx);
            });
        },
    ))
}

fn detail_favorite(
    route: &DetailRoute,
    favorites: gpui::Entity<crate::library::FavoriteState>,
    host: &gpui::Entity<SearchView>,
    app: &gpui::App,
) -> Option<AnyElement> {
    let kind = super::view::favorite_kind(route.provider, route.kind, &route.id)?;
    let key = crate::library::FavoriteKey::for_provider(route.provider, kind, route.id.clone());
    let value = favorites.read(app).favorite(&key);
    let pending = favorites.read(app).pending(&key);
    let card = super::models::Card {
        kind: route.kind,
        id: route.id.clone(),
        title: route.title.clone(),
        subtitle: route.subtitle.clone(),
        artwork: route.artwork.clone(),
        source: route.provider,
        badge: String::new(),
        release_date: route.release_date.clone(),
        service_url: String::new(),
    };
    let host = host.clone();
    Some(playlist_header_action_button(
        SharedString::from(format!("detail-favorite-{}", route.id)),
        LocalIcon::Heart,
        if value == Some(true) {
            FAVORITE_PINK
        } else {
            MUTED
        },
        if value == Some(true) {
            FAVORITE_PINK
        } else {
            FOREGROUND
        },
        pending,
        "Favorite",
        move |_, _, app| {
            host.update(app, |this, cx| {
                this.toggle_collection_favorite(card.clone(), value.unwrap_or(false), cx);
            });
        },
    ))
}

fn detail_route_card(route: &DetailRoute, total: usize) -> Card {
    Card {
        kind: route.kind,
        id: route.id.clone(),
        title: route.title.clone(),
        subtitle: route.subtitle.clone(),
        artwork: route.artwork.clone(),
        source: route.provider,
        badge: total.to_string(),
        release_date: route.release_date.clone(),
        service_url: String::new(),
    }
}

fn detail_more_button(
    card: Card,
    account: gpui::Entity<crate::settings::AccountState>,
    host: gpui::Entity<SearchView>,
) -> Option<AnyElement> {
    let entity = card_entity(&card, true)?;
    let id = SharedString::from(format!(
        "detail-more-{}-{}-{}",
        card.source.label(),
        card.kind.label(),
        card.id
    ));
    Some(
        card_menu_button(playlist_header_more_button(id), entity, card, host, account)
            .into_any_element(),
    )
}

fn detail_message(icon: LocalIcon, color: u32, title: &str, description: &str) -> AnyElement {
    div()
        .h(px(360.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(local_icon(icon, color).size(px(40.)))
        .child(
            div()
                .mt(px(12.))
                .mb(px(4.))
                .text_size(px(16.))
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .when(!description.is_empty(), |this| {
            this.child(div().text_size(px(13.)).child(description.to_owned()))
        })
        .into_any_element()
}

fn artist_popular_actions(provider: Provider) -> bool {
    matches!(provider, Provider::Deezer | Provider::SoundCloud)
}

fn artist_tracks_title() -> &'static str {
    "Tracks"
}

fn artist_tracks_preview(tracks_only: bool, expanded: Option<ArtistSection>) -> bool {
    !tracks_only && expanded != Some(ArtistSection::PopularTracks)
}

#[cfg(test)]
mod layout_tests {
    use super::{
        ArtistSection, Provider, artist_popular_actions, artist_tracks_preview, artist_tracks_title,
    };
    use crate::collection_detail::DETAIL_CONTEXT_OPTICAL_OFFSET_PX;

    #[test]
    fn detail_heading_uses_the_legacy_18_pixel_line_height_ratio() {
        assert_eq!(crate::collection_detail::DETAIL_HEADER_STACK_GAP_PX, 4.);
    }

    #[test]
    fn detail_context_uses_a_two_pixel_optical_offset() {
        assert_eq!(DETAIL_CONTEXT_OPTICAL_OFFSET_PX, 2.);
    }

    #[test]
    fn collection_headings_do_not_render_a_heading_info_action() {
        let source = include_str!("detail_view.rs");
        for needle in [
            ["album", "_info_button"].concat(),
            ["DETAIL_INFO_OPTICAL", "_OFFSET_PX"].concat(),
            ["About this ", "album"].concat(),
            ["About this ", "playlist"].concat(),
        ] {
            assert!(!source.contains(needle.as_str()), "found {needle}");
        }
    }

    #[test]
    fn detail_track_actions_follow_both_provider_favorite_support() {
        assert!(artist_popular_actions(Provider::Deezer));
        assert!(artist_popular_actions(Provider::SoundCloud));
    }

    #[test]
    fn empty_detail_view_retains_page_heading() {
        let source = include_str!("detail_view.rs");
        assert!(source.contains("DetailState::Empty(page) => {"));
        assert!(source.contains("render_page("));
        assert!(source.contains("detail_message("));
    }

    #[test]
    fn artist_header_groups_favorite_and_more_with_title_and_albums_section() {
        let source = include_str!("detail_view.rs");
        assert!(source.contains("\"Albums\",\n            1,"));
        assert!(source.contains("\"Albums\",\n            ArtistSection::Albums,"));
        let legacy = format!("Albums {} {}Ps", '&', 'E');
        assert!(!source.contains(legacy.as_str()));
    }

    #[test]
    fn soundcloud_tracks_only_heading_is_tracks_without_preview_or_view_all() {
        assert_eq!(artist_tracks_title(), "Tracks");
        assert!(!artist_tracks_preview(true, None));
        assert!(!artist_tracks_preview(
            true,
            Some(ArtistSection::PopularTracks)
        ));
        assert!(artist_tracks_preview(false, None));
        assert!(!artist_tracks_preview(
            false,
            Some(ArtistSection::PopularTracks)
        ));

        let implementation = include_str!("detail_view.rs")
            .split_once("#[cfg(test)]")
            .map_or_else(
                || panic!("detail view implementation section is missing"),
                |(implementation, _)| implementation,
            );
        assert!(implementation.contains("soundcloud_tracks_only(route.provider, artist)"));
        assert!(implementation.contains("artist_tracks_title()"));
        assert!(implementation.contains("artist_tracks_preview(tracks_only, expanded)"));
        assert!(implementation.contains("tracks_only,"));
        assert!(implementation.contains("if tracks_only { None } else { expanded }"));
        assert!(implementation.contains("render_shared_artist_section_header"));
    }

    #[test]
    fn collection_header_groups_actions_with_title_and_keeps_platform_context_separate() {
        let source = include_str!("detail_view.rs")
            .split_once("#[cfg(test)]")
            .expect("detail view tests follow the implementation")
            .0;
        assert!(source.contains("render_shared_provider_detail("));
        assert!(source.contains("ProviderHeaderSpec"));
        assert!(source.contains("render_shared_artist_header"));
        assert!(!source.contains("fn detail_delete("));
        assert!(!source.contains("playlist_header_delete_button"));
    }

    #[test]
    fn playlist_detail_does_not_render_reorder_status_banner() {
        let implementation = include_str!("detail_view.rs")
            .split_once("#[cfg(test)]")
            .expect("detail view tests follow the implementation")
            .0;
        assert!(!implementation.contains("reorder_status("));
        assert!(!implementation.contains("Saving playlist order"));
    }
}
