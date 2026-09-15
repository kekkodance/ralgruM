use super::*;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn playback_context_for_route(
    route: &Route,
    flow_mode: crate::library::deezer_radio::FlowMode,
    tuner: Option<crate::library::deezer_radio::FlowTuner>,
    deezer_load_id: Option<u64>,
    station_seed: Option<String>,
) -> crate::playback::PlaybackContext {
    playback_context_for_route_with_kind(
        route,
        flow_mode,
        tuner,
        deezer_load_id,
        station_seed,
        crate::playback::DeezerFlowKind::Flow,
    )
}

pub(super) fn playback_context_for_route_with_kind(
    route: &Route,
    flow_mode: crate::library::deezer_radio::FlowMode,
    tuner: Option<crate::library::deezer_radio::FlowTuner>,
    deezer_load_id: Option<u64>,
    station_seed: Option<String>,
    flow_kind: crate::playback::DeezerFlowKind,
) -> crate::playback::PlaybackContext {
    if route.source == crate::search::Provider::Deezer
        && route.action == Category::Tracks.action()
        && let Some(load_id) = deezer_load_id
    {
        return crate::playback::PlaybackContext::DeezerLibraryTracks { load_id };
    }
    match route.action.as_str() {
        "flowTracks" => crate::playback::PlaybackContext::DeezerFlow {
            config_id: route.id.clone(),
            mode: flow_mode,
            tuner,
            kind: flow_kind,
        },
        "stationTracks" => crate::playback::PlaybackContext::SoundCloudStation {
            seed_track_id: station_seed.unwrap_or_else(|| route.id.clone()),
        },
        _ => crate::playback::PlaybackContext::soundcloud_collection(
            route.source,
            route.action.as_str(),
            &route.id,
        )
        .unwrap_or(crate::playback::PlaybackContext::None),
    }
}

pub(super) fn station_seed_from_page(page: Option<&Page>) -> Option<String> {
    page.and_then(|page| page.tracks.last())
        .map(|track| track.id.trim())
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
}

pub(super) fn detail_download_snapshot(view: &LibraryView) -> Option<DetailDownloadSnapshot> {
    let route = view.state.route();
    let page = view.state.page.as_ref()?;
    if !detail_download_action_eligible(&route.action, view.state.routes.len(), page) {
        return None;
    }
    let mut seen = std::collections::HashSet::with_capacity(page.tracks.len());
    let mut tracks = Vec::with_capacity(page.tracks.len());
    for track in &page.tracks {
        if track.id.trim().is_empty() || !seen.insert(track.id.as_str()) {
            return None;
        }
        tracks.push(crate::playback::PlaybackTrack::from_library(
            track,
            route.source,
        ));
    }
    Some(DetailDownloadSnapshot {
        id: format!("library-{}-{}", route.source.label(), route.id),
        tracks,
        downloads: view.downloads.clone(),
        account: view.account.clone(),
    })
}

pub(super) fn detail_download_action_eligible(
    action: &str,
    route_depth: usize,
    page: &Page,
) -> bool {
    is_detail_route(route_depth)
        && action == "albumTracks"
        && detail_download_page_is_complete(page)
}

pub(super) fn detail_download_page_is_complete(page: &Page) -> bool {
    !page.tracks.is_empty()
        && page.authoritative_total == Some(page.raw_loaded_count)
        && page.normalized_count == page.raw_loaded_count
        && page.tracks.len() == page.raw_loaded_count
}

pub(super) fn render_detail_download(snapshot: &DetailDownloadSnapshot) -> AnyElement {
    crate::context_menu::collection_download_button(
        snapshot.id.clone(),
        snapshot.tracks.clone(),
        snapshot.downloads.clone(),
        snapshot.account.clone(),
    )
}

pub(super) fn reorder_for_page(
    view: &LibraryView,
    page: &Page,
    cx: &mut Context<LibraryView>,
) -> Option<(String, bool)> {
    let route = view.state.route();
    if route.is_local_route() {
        let identity = if route.id.is_empty() {
            "local".into()
        } else {
            route.id.clone()
        };
        return (page.tracks.len() > 1 && view.query(cx).trim().is_empty())
            .then_some((identity, true));
    }
    if !crate::library::playlist_reorder::route_eligible(
        route.source,
        &route.action,
        &route.id,
        &route.id,
    ) {
        return None;
    }
    crate::library::playlist_reorder::move_eligible(
        page,
        view.playlist_catalog(route.source).is_editable(&route.id),
        !view.query(cx).trim().is_empty(),
        view.playlist_catalog(route.source).reorder_pending,
    )
    .then(|| (route.id.clone(), true))
}

pub(super) fn reorder_pending_for_page(view: &LibraryView) -> bool {
    (!view.state.route().is_local_route())
        && view
            .playlist_catalog(view.state.route().source)
            .reorder_pending
}

pub(super) fn detail_edit_id(view: &LibraryView) -> Option<(crate::search::Provider, String)> {
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len())
        || !crate::library::playlist_state::matching_playlist_route(
            route.source,
            &route.action,
            &route.id,
            &route.id,
        )
        || !view.playlist_editable(route.source, &route.id)
    {
        return None;
    }
    Some((route.source, route.id.clone()))
}

pub(super) fn detail_more_card(view: &LibraryView) -> Option<Card> {
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len())
        || !matches!(
            route.category,
            Category::Albums | Category::Artists | Category::Playlists
        )
        || route.id.trim().is_empty()
        || route.is_local_route()
    {
        return None;
    }
    Some(Card {
        kind: route.category,
        id: route.id.clone(),
        title: route.title.clone(),
        subtitle: route.subtitle.clone(),
        artwork: route.artwork.clone(),
        source: route.source,
        release_date: route.release_date.clone(),
        badge: if route.category == Category::Albums {
            crate::library::release_year(&route.release_date)
        } else {
            String::new()
        },
        // The loaded page carries the canonical provider URL (SoundCloud
        // permalink); Deezer links derive from the numeric id instead.
        service_url: view
            .state
            .page
            .as_ref()
            .map(|page| page.service_url.clone())
            .unwrap_or_default(),
        is_private: None,
        library_service: None,
    })
}

pub(super) fn render_detail_more(card: &Card, host: &gpui::Entity<LibraryView>) -> AnyElement {
    let Some(entity) = crate::context_menu::library_card_entity(card, true) else {
        return div().into_any_element();
    };
    crate::context_menu::library_card_menu_button(
        playlist_header_more_button(
            format!(
                "library-detail-more-{}-{}-{}",
                card.source.label(),
                card.kind.label(),
                card.id
            )
            .into(),
        ),
        entity,
        card.clone(),
        host.clone(),
        false,
    )
    .into_any_element()
}

pub(super) fn render_local_detail_more(
    card: &Card,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    crate::context_menu::local_playlist_card_menu_button(
        playlist_header_more_button(format!("library-detail-more-local-{}", card.id).into()),
        card.clone(),
        host.clone(),
    )
    .into_any_element()
}

pub(super) fn render_detail_edit(
    id: &str,
    provider: crate::search::Provider,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    let id = id.to_owned();
    let host = host.clone();
    playlist_header_action_button(
        format!("library-edit-playlist-{}-{id}", provider.label()).into(),
        LocalIcon::Pen,
        MUTED,
        FOREGROUND,
        false,
        "Edit",
        move |_, window, app| {
            host.update(app, |this, cx| {
                this.open_playlist_editor(provider, id.clone(), window, cx)
            });
        },
    )
}

pub(super) fn render_local_detail_edit(id: &str, host: &gpui::Entity<LibraryView>) -> AnyElement {
    let id = id.to_owned();
    let host = host.clone();
    playlist_header_action_button(
        format!("library-edit-local-playlist-{id}").into(),
        LocalIcon::Pen,
        MUTED,
        FOREGROUND,
        false,
        "Edit",
        move |_, window, app| {
            host.update(app, |this, cx| {
                this.open_local_playlist_editor(id.clone(), window, cx)
            });
        },
    )
}

pub(super) fn detail_delete_id(view: &LibraryView) -> Option<(crate::search::Provider, String)> {
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len())
        || !crate::library::playlist_state::matching_playlist_route(
            route.source,
            &route.action,
            &route.id,
            &route.id,
        )
        || !view.playlist_editable(route.source, &route.id)
    {
        return None;
    }
    Some((route.source, route.id.clone()))
}

pub(super) fn render_detail_delete(
    id: &str,
    provider: crate::search::Provider,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    let id = id.to_owned();
    let host = host.clone();
    playlist_header_delete_button(
        format!("library-delete-playlist-{}-{id}", provider.label()).into(),
        "Delete playlist",
        move |_, window, app| {
            host.update(app, |this, cx| {
                this.open_playlist_delete(provider, id.clone(), window, cx)
            });
        },
    )
}

pub(super) fn detail_favorite_snapshot(
    view: &LibraryView,
    app: &gpui::App,
) -> Option<DetailFavoriteSnapshot> {
    use crate::library::favorite_state::{FavoriteKey, FavoriteKind};
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len()) || route.id.is_empty() || route.is_local_route() {
        return None;
    }
    let kind = match route.category {
        Category::Albums if route.action == "albumTracks" => FavoriteKind::Album,
        Category::Artists if matches!(route.action.as_str(), "artist" | "artistTracks") => {
            FavoriteKind::Artist
        }
        Category::Playlists if route.action == "playlistTracks" => FavoriteKind::Playlist,
        _ => return None,
    };
    if matches!(kind, FavoriteKind::Playlist) && view.playlist_editable(route.source, &route.id) {
        return None;
    }
    let key = FavoriteKey::for_provider(route.source, kind, route.id.clone());
    let value = view.favorites.read(app).favorite(&key);
    let pending = view.favorites.read(app).pending(&key);
    Some(DetailFavoriteSnapshot {
        provider: route.source,
        kind,
        id: route.id.clone(),
        value,
        pending,
    })
}

pub(super) fn render_detail_favorite(
    snapshot: &DetailFavoriteSnapshot,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    use crate::library::favorite_state::FavoriteKey;
    let provider = snapshot.provider;
    let kind = snapshot.kind;
    let id = snapshot.id.clone();
    let value = snapshot.value;
    let pending = snapshot.pending;
    let host = host.clone();
    playlist_header_action_button(
        format!("library-detail-favorite-{id}").into(),
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
                this.toggle_favorite(
                    FavoriteKey::for_provider(provider, kind, id.clone()),
                    value.unwrap_or(false),
                    cx,
                );
            });
        },
    )
}

pub(super) fn artist_section_action(
    view: &LibraryView,
    section: &Section,
    section_index: usize,
    expanded: bool,
    tracks_only: bool,
    cx: &mut Context<LibraryView>,
) -> Option<AnyElement> {
    if !is_artist_detail(view) {
        return None;
    }
    if !artist_section_action_visible_for_mode(section, expanded, tracks_only) {
        return None;
    }
    let host = cx.entity();
    let label = if expanded { "View less" } else { "View all" };
    let click_host = host.clone();
    let key_host = host;
    Some(
        div()
            .id(format!("library-artist-section-{section_index}"))
            .focusable()
            .tab_stop(true)
            .role(gpui::Role::Button)
            .aria_label(label)
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
                    this.toggle_artist_section(section_index, cx);
                });
            })
            .on_key_down(move |event: &KeyDownEvent, window, app| {
                if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                    window.prevent_default();
                    key_host.update(app, |this, cx| {
                        this.toggle_artist_section(section_index, cx);
                    });
                }
            })
            .child(label)
            .into_any_element(),
    )
}

#[cfg(test)]
pub(super) fn artist_section_action_visible(section: &Section, expanded: bool) -> bool {
    artist_section_action_visible_for_mode(section, expanded, false)
}

pub(super) fn artist_section_action_visible_for_mode(
    section: &Section,
    expanded: bool,
    tracks_only: bool,
) -> bool {
    if tracks_only {
        return false;
    }
    let loaded = section.tracks.len() + section.cards.len();
    let previewed = section.preview_limit.unwrap_or(loaded);
    artist_section_shows_action_for_counts(previewed, loaded, expanded)
}
