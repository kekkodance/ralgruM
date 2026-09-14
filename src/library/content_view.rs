use gpui::{
    AnimationExt as _, AnyElement, Context, FontWeight, KeyDownEvent, div, prelude::*, px, rgb,
};
use gpui_component::{
    Icon, Sizable,
    button::{Button, ButtonVariants},
};
use std::{borrow::Cow, rc::Rc, sync::Arc};

use crate::{
    app_button::{primary_button, secondary_page_action_button_with_disabled},
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollTarget, FixedListScrollHandle, browser_scroll_surface},
    collection_detail::{
        ArtistHeaderSpec, DETAIL_SECTION_GAP_PX, LocalPlaylistHeaderSpec, ProviderHeaderSpec,
        render_artist_header as render_shared_artist_header,
        render_artist_section as render_shared_artist_section, render_collection_empty,
        render_local_playlist_header as render_shared_local_playlist_header,
        render_provider_header as render_shared_provider_header,
    },
    music_ui::{
        playlist_header_action_button, playlist_header_delete_button, playlist_header_more_button,
    },
    search::artist_section_shows_action_for_counts,
    smart_mix_title::{CANONICAL_SMART_MIX_TITLE, specific_smart_mix_title},
    theme::{BORDER, DEEZER, FOREGROUND, MUTED, SOUNDCLOUD, SURFACE, SURFACE_RAISED},
};

use super::{
    client::DEEZER_SESSION_EXPIRED,
    flow_controls,
    model::{
        Card, Category, FLOW_TRACK_DESCRIPTION, Page, Route, Section, SectionLayout, Service,
        is_deezer_flow_detail, is_detail_route, root_copy, section_count_visible,
    },
    state::Status,
    view::LibraryView,
};

pub(super) const PAGE_HEADING_SMALL_MAX_VIEWPORT: f32 = 460.0;
const HEADING_CONTROL_OPTICAL_OFFSET_PX: f32 = 2.;
const PLAYLIST_CREATE_OPTICAL_OFFSET_PX: f32 = 2.;
const HEADER_SECONDARY_LINE_HEIGHT_PX: f32 = 18.125;
const FAVORITE_PINK: u32 = 0xec4899;

struct PageHeaderSnapshot {
    service: Service,
    category: Category,
    route_depth: usize,
    flow_mode_control: bool,
    flow_mode: super::deezer_radio::FlowMode,
    flow_mode_label: &'static str,
    flow_header_prefers_subtitle: bool,
    flow_header_subtitle_skeleton: bool,
    edit_playlist: Option<(crate::search::Provider, String)>,
    delete_playlist: Option<(crate::search::Provider, String)>,
    more: Option<Card>,
    favorite: Option<DetailFavoriteSnapshot>,
    download: Option<DetailDownloadSnapshot>,
}

struct DetailFavoriteSnapshot {
    provider: crate::search::Provider,
    kind: super::favorite_state::FavoriteKind,
    id: String,
    value: Option<bool>,
    pending: bool,
}

struct DetailDownloadSnapshot {
    id: String,
    tracks: Vec<crate::playback::PlaybackTrack>,
    downloads: gpui::Entity<crate::downloads::DownloadModel>,
    account: gpui::Entity<crate::settings::AccountState>,
}

pub(super) fn render_content(
    view: &LibraryView,
    columns: u16,
    available_width: f32,
    available_height: f32,
    narrow: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    match &view.state.status {
        Status::Initial => message(
            LocalIcon::Layers,
            "Your service library",
            "Select a category to load your saved music.",
            None,
        ),
        Status::Loading => super::skeleton::render(
            view.state.route(),
            columns,
            available_width,
            available_height,
            narrow,
            false,
        ),
        Status::AccountRequired => {
            let provider = if view.state.service == super::model::Service::SoundCloud {
                "SoundCloud"
            } else {
                "Deezer"
            };
            let (title, description) = crate::empty_state::account_required_copy(provider);
            crate::empty_state::render(
                LocalIcon::UserLock,
                title,
                description,
                Some(open_account_settings_action(cx)),
            )
        }
        Status::Failed(error) => {
            if error.as_str() == DEEZER_SESSION_EXPIRED {
                crate::empty_state::render(
                    LocalIcon::UserLock,
                    "Your Deezer session has expired",
                    "Log in to Deezer again to refresh your library.",
                    Some(open_account_settings_action(cx)),
                )
            } else {
                crate::empty_state::render(
                    LocalIcon::TriangleExclamation,
                    "Could not load this library",
                    error,
                    Some(
                        secondary_page_action_button_with_disabled(
                            "library-retry",
                            Some(LocalIcon::RotateRight),
                            "Try again",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.reload_root(cx);
                            }),
                        )
                        .into_any_element(),
                    ),
                )
            }
        }
        Status::Empty => view
            .state
            .page
            .as_ref()
            .map(|page| render_empty_page(view, page, cx))
            .unwrap_or_else(|| empty_message(view)),
        Status::Results => {
            let page = filtered_page_for_view(view, view.state.page.as_ref().unwrap(), cx);
            let page = page.as_ref();
            let virtualized = page_uses_virtualized_scroll(page);
            if page.is_empty() {
                let controls = (!promotes_flow_detail_to_main_header(view))
                    .then(|| flow_controls::render_page_controls(view, &cx.entity()))
                    .flatten();
                let empty = if is_provider_detail(view) {
                    render_collection_empty(view.state.route().category == Category::Playlists)
                } else {
                    filtered_empty_message(page)
                };
                with_page_controls(controls, empty)
            } else if page_uses_flattened_sections(page) {
                render_section_page_list(view, page, columns, available_width, narrow, cx)
            } else {
                div()
                    .flex()
                    .flex_col()
                    .when(virtualized, |this| this.flex_1().min_h_0())
                    .gap(px(10.))
                    .child(render_page(
                        view,
                        page,
                        columns,
                        available_width,
                        narrow,
                        cx,
                    ))
                    .into_any_element()
            }
        }
    }
}

fn filtered_page_for_view<'a>(
    view: &LibraryView,
    page: &'a Page,
    cx: &Context<LibraryView>,
) -> Cow<'a, Page> {
    let query = view.query(cx);
    if query.trim().is_empty() {
        Cow::Borrowed(page)
    } else {
        Cow::Owned(super::filter::filtered_page(page, &query))
    }
}

fn render_empty_page(view: &LibraryView, page: &Page, cx: &mut Context<LibraryView>) -> AnyElement {
    let controls = (!promotes_flow_detail_to_main_header(view))
        .then(|| flow_controls::render_page_controls(view, &cx.entity()))
        .flatten();
    let empty = if is_provider_detail(view) {
        render_collection_empty(view.state.route().category == Category::Playlists)
    } else {
        empty_message_for_page(page)
    };
    with_page_controls(controls, empty)
}

fn with_page_controls(controls: Option<AnyElement>, body: AnyElement) -> AnyElement {
    let Some(controls) = controls else {
        return body;
    };
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(controls)
        .child(body)
        .into_any_element()
}

pub(super) fn uses_virtualized_scroll(view: &LibraryView, cx: &Context<LibraryView>) -> bool {
    if !matches!(view.state.status, Status::Results) {
        return false;
    }
    view.state
        .page
        .as_ref()
        .map(|page| page_uses_virtualized_scroll(filtered_page_for_view(view, page, cx).as_ref()))
        .unwrap_or(false)
}

fn page_uses_virtualized_scroll(page: &Page) -> bool {
    (!page.uses_sections() && (!page.tracks.is_empty() || !page.cards.is_empty()))
        || page_uses_flattened_sections(page)
}

fn page_uses_flattened_sections(page: &Page) -> bool {
    page.sections.iter().any(|section| {
        section.preview_limit.is_none()
            && ((!section.tracks.is_empty() && section.layout == SectionLayout::Tracks)
                || (!section.cards.is_empty() && section.layout == SectionLayout::Cards))
    })
}

fn empty_message(view: &LibraryView) -> AnyElement {
    if let Some(page) = &view.state.page
        && !page.empty_title.is_empty()
    {
        return message(
            LocalIcon::CompactDisc,
            &page.empty_title,
            &page.empty_description,
            None,
        );
    }
    let (title, description) = category_empty_copy(view.state.service, view.state.category);
    message(LocalIcon::CompactDisc, title, description, None)
}

fn category_empty_copy(
    service: super::model::Service,
    category: Category,
) -> (&'static str, &'static str) {
    match category {
        Category::MyTracks => (
            "Nothing here yet",
            "Tracks uploaded by your SoundCloud account will appear here.",
        ),
        Category::Tracks if service == super::model::Service::Local => (
            "No local tracks yet",
            "Tracks you save to your local library will appear here. Saving a track keeps a reference, not an audio file.",
        ),
        Category::Tracks | Category::History => (
            "Nothing here yet",
            "This part of your library is currently empty.",
        ),
        Category::Flow => (
            "Flow is unavailable",
            "No Flow mixes were returned for this account.",
        ),
        Category::Albums => (
            if service == super::model::Service::SoundCloud {
                "No saved albums"
            } else {
                "No favorite albums"
            },
            if service == super::model::Service::SoundCloud {
                "Albums you save on SoundCloud will appear here."
            } else {
                "Albums you save on Deezer will appear here."
            },
        ),
        Category::Artists => (
            "No followed artists",
            if service == super::model::Service::SoundCloud {
                "Artists you follow on SoundCloud will appear here."
            } else {
                "Artists you follow on Deezer will appear here."
            },
        ),
        Category::Playlists if service == super::model::Service::Local => (
            "No local playlists yet",
            "Create a local playlist to organize track references. Local playlists do not download audio files.",
        ),
        Category::Playlists => (
            "No playlists",
            if service == super::model::Service::SoundCloud {
                "Playlists you create or save on SoundCloud will appear here."
            } else {
                "Saved and created Deezer playlists will appear here."
            },
        ),
        Category::Station => (
            "No station seeds",
            "Like a SoundCloud track first, then use it to start a station.",
        ),
    }
}

fn empty_message_for_page(page: &Page) -> AnyElement {
    let (title, description) = page_empty_copy(page);
    crate::empty_state::render(LocalIcon::CompactDisc, title, description, None)
}

fn filtered_empty_message(page: &Page) -> AnyElement {
    let (title, description) = page_empty_copy(page);
    div()
        .w_full()
        .min_h(px(300.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .py(px(48.))
        .px(px(24.))
        .text_color(rgb(MUTED))
        .text_center()
        .child(
            div()
                .flex_none()
                .mb(px(13.))
                .child(local_icon(LocalIcon::FolderOpen, MUTED).size(px(31.))),
        )
        .child(
            div()
                .text_color(rgb(FOREGROUND))
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .line_height(px(19.5))
                .child(title.to_owned()),
        )
        .child(
            div()
                .max_w(px(440.))
                .mt(px(5.))
                .text_size(px(12.5))
                .line_height(px(18.75))
                .child(description.to_owned()),
        )
        .into_any_element()
}

fn page_empty_copy(page: &Page) -> (&str, &str) {
    if page.empty_title.is_empty() {
        (
            "Nothing here yet",
            "This part of your library is currently empty.",
        )
    } else {
        (page.empty_title.as_str(), page.empty_description.as_str())
    }
}

fn render_page(
    view: &LibraryView,
    page: &Page,
    columns: u16,
    available_width: f32,
    narrow: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let route = view.state.route();
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
    let body = (!page.uses_sections()).then(|| {
        if page.tracks.is_empty() {
            super::cards_view::virtualized_cards(
                view,
                &cx.entity(),
                &page.cards,
                &view.state.route().action,
                columns,
                available_width,
                narrow,
                &view.query(cx),
            )
        } else {
            super::track_view::tracks(
                view,
                &page.tracks,
                None,
                narrow,
                view.track_favorites_available(),
                view.playing.clone(),
                view.favorites.clone(),
                removal.clone(),
                view.playlists.remove_pending,
                reorder_for_page(view, page, cx),
                reorder_pending_for_page(view),
                view.state.route().source,
                view.external_track_navigation_openers(),
                view.track_favorites_available()
                    && view.state.selected_root_active(Category::Tracks),
                playback_context(view),
                view.playback.clone(),
                view.downloads.clone(),
                view.account.clone(),
                album_navigation_context(view),
                "page",
                cx,
            )
        }
    });
    let artist_detail = is_artist_detail(view);
    let tracks_only = artist_detail
        && soundcloud_tracks_only_page(view.state.route().source, &view.state.route().action, page);
    let expanded_artist_section = (!tracks_only)
        .then_some(view.artist_section_expanded)
        .flatten();
    div()
        .flex()
        .flex_col()
        .when(page_uses_virtualized_scroll(page), |this| {
            this.flex_1().min_h_0()
        })
        .gap(px(if artist_detail {
            DETAIL_SECTION_GAP_PX
        } else {
            14.
        }))
        .when_some(body, |this, body| this.child(body))
        .children(
            page.sections
                .iter()
                .enumerate()
                .filter(|(section_index, section)| {
                    (!artist_detail || !section.tracks.is_empty() || !section.cards.is_empty())
                        && (!artist_detail
                            || expanded_artist_section.is_none()
                            || expanded_artist_section == Some(*section_index))
                })
                .map(|(section_index, section)| {
                    render_section(
                        view,
                        section,
                        section_index,
                        columns,
                        narrow,
                        view.favorites.clone(),
                        removal.clone(),
                        view.playlists.remove_pending,
                        cx,
                    )
                }),
        )
        .into_any_element()
}

fn is_artist_detail(view: &LibraryView) -> bool {
    is_detail_route(view.state.routes.len())
        && view.state.route().category == Category::Artists
        && matches!(
            view.state.route().action.as_str(),
            "artist" | "artistTracks"
        )
}

fn card_row_for_artist_section(card_row: bool, artist_section: bool, expanded: bool) -> bool {
    card_row && !(artist_section && expanded)
}

fn soundcloud_tracks_only_page(
    provider: crate::search::Provider,
    action: &str,
    page: &Page,
) -> bool {
    if provider != crate::search::Provider::SoundCloud || action != "artistTracks" {
        return false;
    }
    let Some(tracks_section) = page
        .sections
        .iter()
        .find(|section| section.title == "Tracks" && !section.tracks.is_empty())
    else {
        return false;
    };
    page.sections.iter().all(|section| {
        std::ptr::eq(section, tracks_section)
            || (section.tracks.is_empty() && section.cards.is_empty())
    })
}

pub(super) fn render_main_header(
    view: &LibraryView,
    small_heading: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let detail = is_detail_route(view.state.routes.len());
    let promotes_flow = promotes_flow_detail_to_main_header(view);
    let flow_header_prefers_subtitle = promotes_flow
        && matches!(
            view.flow_detail_kind(),
            crate::playback::DeezerFlowKind::SmartMix
        );
    let (fallback_title, fallback_description) = root_copy(view.state.service, view.state.category);
    let root_page = if !detail {
        view.state.page.as_ref()
    } else {
        None
    };
    let page = if promotes_flow {
        flow_detail_header_page(
            view.state.route(),
            view.state.page.as_ref(),
            flow_header_prefers_subtitle,
        )
    } else if detail {
        detail_header_page(view.state.route(), view.state.page.as_ref())
    } else {
        root_header_page(
            view.state.category,
            root_page,
            fallback_title,
            fallback_description,
        )
    };
    let host = cx.entity();
    let snapshot = if detail {
        page_header_snapshot(view, cx)
    } else {
        main_page_header_snapshot(view, promotes_flow)
    };
    let show_playlist_create_control = playlist_create_control_visible(
        view.state.service,
        view.state.category,
        view.state.routes.len(),
    );
    if is_provider_detail(view) {
        if is_artist_detail(view) {
            return render_artist_header_snapshot(view, &page, &snapshot, &host);
        }
        return render_provider_header_snapshot(view, &page, &snapshot, &host);
    }
    if is_local_playlist_detail(view) {
        return render_local_playlist_header_snapshot(view, &page, &host);
    }
    render_page_header_snapshot(
        &snapshot,
        &page,
        &host,
        detail && small_heading,
        true,
        show_playlist_create_control,
    )
}

fn is_local_playlist_detail(view: &LibraryView) -> bool {
    is_detail_route(view.state.routes.len())
        && view.state.service == Service::Local
        && view.state.route().is_local_playlist_detail()
}

fn render_local_playlist_header_snapshot(
    view: &LibraryView,
    page: &Page,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    let route = view.state.route();
    let artwork = view
        .local_playlists
        .as_ref()
        .ok()
        .and_then(|store| store.artwork_path(&route.id, &page.artwork));
    let mut actions = vec![render_local_detail_edit(&route.id, host)];
    actions.push(render_local_detail_more(
        &Card {
            kind: Category::Playlists,
            id: route.id.clone(),
            title: page.title.clone(),
            subtitle: page.description.clone(),
            artwork: page.artwork.clone(),
            badge: page.tracks.len().to_string(),
            library_service: Some(Service::Local),
            ..Card::default()
        },
        host,
    ));
    render_shared_local_playlist_header(LocalPlaylistHeaderSpec {
        artwork,
        title: page.title.clone(),
        description: page.description.clone(),
        actions,
    })
}

fn is_provider_detail(view: &LibraryView) -> bool {
    is_detail_route(view.state.routes.len())
        && view.state.service != Service::Local
        && matches!(
            view.state.route().category,
            Category::Albums | Category::Artists | Category::Playlists
        )
}

fn render_provider_header_snapshot(
    view: &LibraryView,
    page: &Page,
    snapshot: &PageHeaderSnapshot,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    let route = view.state.route();
    let mut actions = Vec::new();
    if let Some(favorite) = snapshot.favorite.as_ref() {
        actions.push(render_detail_favorite(favorite, host));
    }
    if let Some((provider, id)) = detail_edit_id(view) {
        actions.push(render_detail_edit(&id, provider, host));
    }
    if let Some(card) = detail_more_card(view) {
        actions.push(render_detail_more(&card, host));
    }
    let kind = match route.category {
        Category::Albums => crate::search::ResultType::Albums,
        Category::Playlists => crate::search::ResultType::Playlists,
        Category::Artists => crate::search::ResultType::Artists,
        _ => crate::search::ResultType::Tracks,
    };
    let detail_route = provider_detail_route(route, page, kind);
    render_shared_provider_header(ProviderHeaderSpec {
        artwork: page.artwork.clone(),
        title: page.title.clone(),
        metadata: crate::search::detail_metadata(&detail_route),
        description: page.description.clone(),
        provider: route.source,
        kind,
        total: page.owns_top_level_count().then_some(page.total),
        actions,
        body_fills: false,
    })
}

fn provider_detail_route(
    route: &Route,
    page: &Page,
    kind: crate::search::ResultType,
) -> crate::search::DetailRoute {
    let release_date = if kind == crate::search::ResultType::Albums {
        page.album_info
            .as_ref()
            .and_then(|info| nonempty_copy(&info.release_date))
            .unwrap_or(route.release_date.as_str())
            .to_owned()
    } else {
        route.release_date.clone()
    };
    crate::search::DetailRoute {
        provider: route.source,
        kind,
        id: route.id.clone(),
        title: route.title.clone(),
        subtitle: nonempty_copy(&page.subtitle)
            .unwrap_or_else(|| nonempty_copy(&route.subtitle).unwrap_or(""))
            .to_owned(),
        artwork: route.artwork.clone(),
        release_date,
        service_url: page.service_url.clone(),
    }
}

fn playlist_create_control_visible(
    service: Service,
    category: Category,
    route_depth: usize,
) -> bool {
    matches!(
        service,
        Service::Local | Service::Deezer | Service::SoundCloud
    ) && category == Category::Playlists
        && !is_detail_route(route_depth)
}

fn root_header_page(
    category: Category,
    loaded_page: Option<&Page>,
    fallback_title: &str,
    fallback_description: &str,
) -> Page {
    let title = if category == Category::Flow {
        fallback_title
    } else {
        loaded_page
            .and_then(|page| nonempty_copy(&page.title))
            .unwrap_or(fallback_title)
    };
    let description = if category == Category::Flow {
        fallback_description
    } else {
        loaded_page
            .and_then(|page| nonempty_copy(&page.description))
            .unwrap_or(fallback_description)
    };
    let (show_count, total, count_noun) = loaded_page
        .map(|page| {
            (
                page.owns_top_level_count(),
                page.total,
                page.count_noun.clone(),
            )
        })
        .unwrap_or_default();
    Page {
        title: title.into(),
        description: description.into(),
        show_count,
        total,
        count_noun,
        raw_loaded_count: loaded_page
            .map(|page| page.raw_loaded_count)
            .unwrap_or_default(),
        normalized_count: loaded_page
            .map(|page| page.normalized_count)
            .unwrap_or_default(),
        authoritative_total: loaded_page.and_then(|page| page.authoritative_total),
        ..Page::default()
    }
}

fn flow_detail_header_page(route: &Route, loaded_page: Option<&Page>, smart_mix: bool) -> Page {
    let (title, description) = flow_detail_header_copy(route, loaded_page, smart_mix);
    let subtitle = if smart_mix {
        {
            loaded_page
                .and_then(|page| nonempty_copy(&page.subtitle))
                .or_else(|| nonempty_copy(&route.subtitle))
                .unwrap_or("")
                .to_owned()
        }
    } else {
        Default::default()
    };
    let (show_count, total, count_noun) = loaded_page
        .map(|page| {
            (
                page.owns_top_level_count(),
                page.total,
                page.count_noun.clone(),
            )
        })
        .unwrap_or_default();
    Page {
        title: title.into(),
        subtitle,
        description: description.into(),
        show_count,
        total,
        count_noun,
        ..Page::default()
    }
}

fn detail_header_page(route: &Route, loaded_page: Option<&Page>) -> Page {
    let title = loaded_page
        .and_then(|page| nonempty_copy(&page.title))
        .or_else(|| nonempty_copy(&route.title))
        .unwrap_or(route.category.label());
    let subtitle = loaded_page
        .and_then(|page| nonempty_copy(&page.subtitle))
        .or_else(|| nonempty_copy(&route.subtitle))
        .unwrap_or("");
    let description = loaded_page
        .and_then(|page| nonempty_copy(&page.description))
        .unwrap_or_else(|| {
            if route.category == Category::Flow {
                FLOW_TRACK_DESCRIPTION
            } else {
                ""
            }
        });
    let artwork = loaded_page
        .and_then(|page| nonempty_copy(&page.artwork))
        .unwrap_or(route.artwork.as_str());
    let (show_count, total, count_noun) = loaded_page
        .map(|page| {
            (
                page.owns_top_level_count(),
                page.total,
                page.count_noun.clone(),
            )
        })
        .unwrap_or_default();
    let (platform, meta_text) = loaded_page
        .map(|page| (page.platform, page.meta_text.clone()))
        .unwrap_or_default();
    Page {
        title: title.into(),
        subtitle: subtitle.into(),
        description: description.into(),
        artwork: artwork.into(),
        platform,
        meta_text,
        show_count,
        total,
        count_noun,
        ..Page::default()
    }
}

fn flow_detail_header_copy<'a>(
    route: &'a Route,
    loaded_page: Option<&'a Page>,
    smart_mix: bool,
) -> (&'a str, &'a str) {
    let title = if smart_mix {
        loaded_page
            .and_then(|page| specific_smart_mix_title(&page.title))
            .or_else(|| specific_smart_mix_title(&route.title))
            .unwrap_or(CANONICAL_SMART_MIX_TITLE)
    } else {
        loaded_page
            .and_then(|page| nonempty_copy(&page.title))
            .or_else(|| nonempty_copy(&route.title))
            .unwrap_or(Category::Flow.label())
    };
    let description = loaded_page
        .and_then(|page| nonempty_copy(&page.description))
        .or_else(|| (!smart_mix).then_some(FLOW_TRACK_DESCRIPTION))
        .unwrap_or("");
    (title, description)
}

fn nonempty_copy(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn flow_header_subtitle_skeleton_visible(
    smart_mix: bool,
    status: &Status,
    page_loaded: bool,
    route_subtitle: &str,
) -> bool {
    smart_mix
        && !page_loaded
        && route_subtitle.trim().is_empty()
        && matches!(status, Status::Loading)
}

fn page_header_secondary_copy<'a>(
    prefer_subtitle: bool,
    _title: &str,
    subtitle: &'a str,
    description: &'a str,
) -> &'a str {
    // Eponymous releases (an album named after its artist) must still show
    // the artist, so only emptiness decides between the two copies.
    if prefer_subtitle && !subtitle.is_empty() {
        subtitle
    } else if !description.is_empty() {
        description
    } else if !subtitle.is_empty() {
        subtitle
    } else {
        ""
    }
}

fn flow_header_subtitle_skeleton() -> impl IntoElement {
    div()
        .mt(px(4.))
        .w(px(220.))
        .h(px(HEADER_SECONDARY_LINE_HEIGHT_PX))
        .flex()
        .items_center()
        .child(
            div()
                .w(px(220.))
                .h(px(10.))
                .rounded(px(4.))
                .bg(rgb(SURFACE_RAISED)),
        )
        .with_animation(
            "library-flow-header-subtitle-skeleton",
            crate::motion::skeleton_loop(),
            |this, delta| this.opacity(crate::motion::lerp(0.72, 0.96, delta)),
        )
}

fn main_page_header_snapshot(view: &LibraryView, flow_mode_control: bool) -> PageHeaderSnapshot {
    PageHeaderSnapshot {
        service: view.state.service,
        category: view.state.category,
        route_depth: view.state.routes.len(),
        flow_mode_control,
        flow_mode: view.flow_mode,
        flow_mode_label: view.flow_mode_context_label(),
        flow_header_prefers_subtitle: false,
        flow_header_subtitle_skeleton: false,
        edit_playlist: None,
        delete_playlist: None,
        more: None,
        favorite: None,
        download: None,
    }
}

fn page_header_snapshot(view: &LibraryView, app: &gpui::App) -> PageHeaderSnapshot {
    let flow_header_prefers_subtitle = promotes_flow_detail_to_main_header(view)
        && matches!(
            view.flow_detail_kind(),
            crate::playback::DeezerFlowKind::SmartMix
        );
    PageHeaderSnapshot {
        service: view.state.service,
        category: view.state.category,
        route_depth: view.state.routes.len(),
        flow_mode_control: flow_controls::flow_control_kind(view)
            == flow_controls::FlowControlKind::Mode,
        flow_mode: view.flow_mode,
        flow_mode_label: view.flow_mode_context_label(),
        flow_header_prefers_subtitle,
        flow_header_subtitle_skeleton: flow_header_subtitle_skeleton_visible(
            flow_header_prefers_subtitle,
            &view.state.status,
            view.state.page.is_some(),
            &view.state.route().subtitle,
        ),
        edit_playlist: detail_edit_id(view),
        delete_playlist: detail_delete_id(view),
        more: detail_more_card(view),
        favorite: detail_favorite_snapshot(view, app),
        download: detail_download_snapshot(view),
    }
}

fn render_artist_header_snapshot(
    view: &LibraryView,
    page: &Page,
    snapshot: &PageHeaderSnapshot,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    let provider = view.state.route().source;
    let subtitle = if page.meta_text.trim().is_empty() {
        page.subtitle.clone()
    } else {
        page.meta_text.clone()
    };
    let mut actions = Vec::new();
    if let Some(favorite) = snapshot
        .favorite
        .as_ref()
        .filter(|favorite| favorite.kind == super::favorite_state::FavoriteKind::Artist)
    {
        actions.push(render_detail_favorite(favorite, host));
    }
    if let Some(card) = snapshot.more.as_ref() {
        actions.push(render_detail_more(card, host));
    }
    render_shared_artist_header(ArtistHeaderSpec {
        artwork: page.artwork.clone(),
        title: page.title.clone(),
        subtitle,
        provider,
        actions,
    })
}

fn render_page_header_snapshot(
    snapshot: &PageHeaderSnapshot,
    page: &Page,
    host: &gpui::Entity<LibraryView>,
    small_heading: bool,
    root_header: bool,
    show_playlist_create_control: bool,
) -> AnyElement {
    let heading_description = page_header_secondary_copy(
        snapshot.flow_header_prefers_subtitle,
        &page.title,
        &page.subtitle,
        &page.description,
    );
    let platform = page
        .platform
        .filter(|_| page_header_shows_platform(snapshot.category, page.platform));
    let show_platform_name = platform.is_some() && detail_heading_has_context(snapshot.route_depth);
    let flow_mode_control = snapshot.flow_mode_control;
    let title_has_intrinsic_width = title_uses_intrinsic_width(
        flow_mode_control,
        platform.is_some(),
        show_playlist_create_control,
    );
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .when(!page.artwork.is_empty(), |this| {
            this.child(
                div()
                    .relative()
                    .size(px(76.))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(rgb(SURFACE_RAISED))
                    .child(
                        crate::artwork_reveal::artwork_reveal(
                            format!("page-header-artwork-{}", page.artwork),
                            page.artwork.clone(),
                        )
                        .size_full()
                        .rounded(px(6.)),
                    ),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(0.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(if small_heading {
                            16.
                        } else if root_header {
                            20.
                        } else {
                            18.
                        }))
                        .font_weight(FontWeight(650.))
                        .line_height(px(if small_heading {
                            20.
                        } else if root_header {
                            25.
                        } else {
                            22.5
                        }))
                        .child(
                            div()
                                .when(title_has_intrinsic_width, |this| this.flex_initial())
                                .when(!title_has_intrinsic_width, |this| this.flex_1())
                                .min_w_0()
                                .when(flow_mode_control || show_playlist_create_control, |this| {
                                    this.whitespace_nowrap()
                                })
                                .when(
                                    !flow_mode_control && !show_playlist_create_control,
                                    |this| this.truncate(),
                                )
                                .child(page.title.clone()),
                        )
                        .when(show_playlist_create_control, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .relative()
                                    .top(px(PLAYLIST_CREATE_OPTICAL_OFFSET_PX))
                                    .child(render_playlist_create_control(host, snapshot.service)),
                            )
                        })
                        .when(flow_mode_control, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .relative()
                                    .top(px(HEADING_CONTROL_OPTICAL_OFFSET_PX))
                                    .child(flow_controls::render_mode_selector(
                                        snapshot.flow_mode,
                                        host,
                                        snapshot.flow_mode_label,
                                    )),
                            )
                        })
                        .when_some(platform, |this, platform| {
                            let (icon, color) = match platform {
                                super::model::Service::Local => {
                                    (LocalIcon::FolderOpen, crate::theme::PRIMARY)
                                }
                                super::model::Service::Deezer => (LocalIcon::Deezer, DEEZER),
                                super::model::Service::SoundCloud => {
                                    (LocalIcon::SoundCloud, SOUNDCLOUD)
                                }
                            };
                            this.child(
                                div()
                                    .flex_none()
                                    .mt(px(HEADING_CONTROL_OPTICAL_OFFSET_PX))
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .child(local_icon(icon, color).size(px(17.)))
                                    .when(show_platform_name, |this| {
                                        this.child(
                                            div()
                                                .text_size(px(11.5))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(color))
                                                .child(platform.label()),
                                        )
                                    }),
                            )
                        }),
                )
                .when(snapshot.flow_header_subtitle_skeleton, |this| {
                    this.child(flow_header_subtitle_skeleton())
                })
                .when(
                    !snapshot.flow_header_subtitle_skeleton && !heading_description.is_empty(),
                    |this| {
                        this.child(
                            div()
                                .mt(px(4.))
                                .text_size(px(12.5))
                                .line_height(px(HEADER_SECONDARY_LINE_HEIGHT_PX))
                                .text_color(rgb(MUTED))
                                .child(heading_description.to_owned()),
                        )
                    },
                )
                .when(
                    !page.artwork.is_empty() && !page.meta_text.is_empty(),
                    |this| {
                        this.child(
                            div()
                                .mt(px(4.))
                                .text_size(px(11.5))
                                .line_height(px(16.1))
                                .text_color(rgb(MUTED))
                                .child(page.meta_text.clone()),
                        )
                    },
                ),
        )
        .when_some(
            snapshot
                .more
                .as_ref()
                .map(|card| render_detail_more(card, host)),
            |this, action| this.child(action),
        )
        .when_some(
            snapshot
                .favorite
                .as_ref()
                .filter(|favorite| {
                    matches!(
                        favorite.kind,
                        super::favorite_state::FavoriteKind::Album
                            | super::favorite_state::FavoriteKind::Artist
                            | super::favorite_state::FavoriteKind::Playlist
                    )
                })
                .map(|favorite| render_detail_favorite(favorite, host)),
            |this, action| this.child(action),
        )
        .when(
            snapshot.edit_playlist.is_some() || snapshot.delete_playlist.is_some(),
            |this| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .when_some(
                            snapshot
                                .edit_playlist
                                .as_ref()
                                .map(|(provider, id)| render_detail_edit(id, *provider, host)),
                            |this, action| this.child(action),
                        )
                        .when_some(
                            snapshot
                                .delete_playlist
                                .as_ref()
                                .map(|(provider, id)| render_detail_delete(id, *provider, host)),
                            |this, action| this.child(action),
                        ),
                )
            },
        )
        .when_some(
            snapshot.download.as_ref().map(render_detail_download),
            |this, action| this.child(action),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .when(page.owns_top_level_count(), |this| {
                    this.text_size(px(12.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(rgb(MUTED))
                        .child(page_count_text(page))
                }),
        )
        .into_any_element()
}

fn playlist_create_button_id(service: Service) -> &'static str {
    match service {
        Service::Local => "create-local-playlist",
        Service::Deezer => "create-deezer-playlist",
        Service::SoundCloud => "create-soundcloud-playlist",
    }
}

fn render_playlist_create_control(
    host: &gpui::Entity<LibraryView>,
    service: Service,
) -> AnyElement {
    let host = host.clone();
    div()
        .id("library-create-playlist-tooltip")
        .flex_none()
        .size(px(22.))
        .rounded(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .app_tooltip("Create playlist")
        .child(
            Button::new(playlist_create_button_id(service))
                .xsmall()
                .with_size(px(14.666666))
                .ghost()
                .size(px(22.))
                .rounded(px(6.))
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .focus_visible(|style| style.border_1().border_color(rgb(crate::theme::PRIMARY)))
                .icon(Icon::default().path(LocalIcon::Plus.path()))
                .on_click(move |_, window, app| {
                    host.update(app, |this, cx| {
                        if this.state.service == Service::Local
                            && this.state.selected_root_active(Category::Playlists)
                        {
                            this.open_local_playlist_create(window, cx);
                        } else if this.state.deezer_root_active(Category::Playlists) {
                            this.open_playlist_create(Vec::new(), window, cx);
                        } else if this.state.service == Service::SoundCloud
                            && this.state.selected_root_active(Category::Playlists)
                        {
                            this.open_soundcloud_playlist_create(window, cx);
                        }
                    });
                }),
        )
        .into_any_element()
}

fn page_count_text(page: &Page) -> String {
    format!(
        "{} {}{}",
        page.total,
        if page.count_noun.is_empty() {
            "item"
        } else {
            &page.count_noun
        },
        if page.total == 1 { "" } else { "s" }
    )
}

fn detail_heading_has_context(route_depth: usize) -> bool {
    is_detail_route(route_depth)
}

fn title_uses_intrinsic_width(
    flow_mode_control: bool,
    has_platform: bool,
    has_inline_heading_control: bool,
) -> bool {
    flow_mode_control || has_platform || has_inline_heading_control
}

fn promotes_flow_detail_to_main_header(view: &LibraryView) -> bool {
    is_deezer_flow_detail(
        view.state.service,
        view.state.route(),
        view.state.routes.len(),
    )
}

fn page_header_shows_platform(category: Category, platform: Option<super::model::Service>) -> bool {
    platform.is_some() && category != Category::Flow
}

fn album_navigation_context(view: &LibraryView) -> Option<crate::entity_navigation::ContextRoute> {
    let route = view.state.route();
    (route.action == "albumTracks").then(|| crate::entity_navigation::ContextRoute {
        provider: route.source,
        action: route.action.clone(),
        id: route.id.clone(),
        title: route.title.clone(),
    })
}

fn render_section_page_list(
    view: &LibraryView,
    page: &Page,
    columns: u16,
    available_width: f32,
    narrow: bool,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let host = cx.entity();
    let route = view.state.route();
    let mut builders = Vec::<super::virtualization::PageItemBuilder>::new();
    let mut page_items = Vec::<super::virtualization::PageItem>::new();
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
                super::track_view::library_track_identity(track)
            )
        }));
        ordered_rows.extend(section.cards.iter().map(|card| {
            format!(
                "card:{section_index}:{}",
                super::cards_view::card_content_identity(card)
            )
        }));
    }
    let filter_identity = view.query(cx);
    let content_key =
        super::virtualization::content_identity(&route_key, &filter_identity, ordered_rows);
    let card_layout = super::virtualization::CardGridLayout::new_for_visual(
        columns,
        available_width,
        narrow,
        super::cards_view::provider_artist_detail(view),
    );
    let has_unbounded_cards = page.sections.iter().any(|section| {
        section.layout == SectionLayout::Cards
            && section.preview_limit.is_none()
            && !section.cards.is_empty()
    });

    for (section_index, section) in page.sections.iter().enumerate() {
        let section_header = Arc::new(section.clone());
        page_items.push(super::virtualization::PageItem::Header(section_index));
        item_identities.push(format!(
            "header:{section_index}:{}:{}:{}:{}:{}:{}:{}",
            section.title,
            section.description,
            section.total,
            section.show_count,
            section.layout == SectionLayout::Cards,
            section.preview_limit.map_or(usize::MAX, |limit| limit),
            section.card_row,
        ));
        builders.push(Rc::new(move |_, _| render_section_header(&section_header)));
        if section.tracks.is_empty() && section.cards.is_empty() {
            let empty_section = Arc::new(section.clone());
            page_items.push(super::virtualization::PageItem::Tail(section_index));
            item_identities.push(format!(
                "empty:{section_index}:{}",
                empty_section.empty_message
            ));
            builders.push(Rc::new(move |_, _| render_section_empty(&empty_section)));
            continue;
        }
        match section.layout {
            SectionLayout::Tracks => {
                let rows = Rc::new(super::track_view::LibraryTrackRows::new(
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
                        .map(super::track_view::library_track_identity)
                        .unwrap_or_else(|| format!("row-{index}"));
                    page_items.push(super::virtualization::PageItem::Track(index));
                    item_identities.push(format!("track:{section_index}:{identity}"));
                    builders.push(Rc::new(move |_, app| {
                        div()
                            .w_full()
                            .h(super::virtualization::row_height())
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
                        super::cards_view::CardsRenderSnapshot::from_view(view, None),
                    );
                    let row_count = super::virtualization::card_grid_row_count(
                        cards.len(),
                        card_layout.columns,
                    );
                    for row_index in 0..row_count {
                        let range = super::virtualization::card_grid_row_range(
                            cards.len(),
                            card_layout.columns,
                            row_index,
                        )
                        .expect("card grid section row must contain cards");
                        let row_identity = format!(
                            "card-row:{section_index}:{row_index}:{}",
                            cards[range.clone()]
                                .iter()
                                .map(super::cards_view::card_content_identity)
                                .collect::<Vec<_>>()
                                .join("|")
                        );
                        let cards = cards.clone();
                        let host = host.clone();
                        let cards_snapshot = cards_snapshot.clone();
                        let section_identity = section_identity.clone();
                        page_items.push(super::virtualization::PageItem::CardRow {
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
                                .child(super::cards_view::cards_with_snapshot(
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
                    let section_identity = super::cards_view::card_scroll_identity(
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
                        Rc::new(super::cards_view::CardsRenderSnapshot::from_view(
                            view,
                            scroll_id.as_deref(),
                        ));
                    page_items.push(super::virtualization::PageItem::Tail(section_index));
                    item_identities.push(format!(
                        "bounded-cards:{section_index}:{}",
                        section
                            .cards
                            .iter()
                            .map(super::cards_view::card_content_identity)
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                    builders.push(Rc::new(move |_, _app| {
                        super::cards_view::cards_with_snapshot(
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
        let layout = super::virtualization::TrackListLayout::new(narrow, false, true);
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
    let page_height = super::virtualization::page_item_uniform_height(&page_items, card_layout);
    let fixed_scroll = FixedListScrollHandle::new(state.clone(), page_items.len(), page_height);
    browser_scroll_surface(
        "library-section-page-list-scroll",
        super::virtualization::page_list(state, fixed_scroll.clone(), Rc::new(builders), narrow),
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

#[cfg_attr(not(test), allow(dead_code))]
fn playback_context_for_route(
    route: &Route,
    flow_mode: super::deezer_radio::FlowMode,
    tuner: Option<super::deezer_radio::FlowTuner>,
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

fn playback_context_for_route_with_kind(
    route: &Route,
    flow_mode: super::deezer_radio::FlowMode,
    tuner: Option<super::deezer_radio::FlowTuner>,
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

fn station_seed_from_page(page: Option<&Page>) -> Option<String> {
    page.and_then(|page| page.tracks.last())
        .map(|track| track.id.trim())
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
}

fn detail_download_snapshot(view: &LibraryView) -> Option<DetailDownloadSnapshot> {
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

fn detail_download_action_eligible(action: &str, route_depth: usize, page: &Page) -> bool {
    is_detail_route(route_depth)
        && action == "albumTracks"
        && detail_download_page_is_complete(page)
}

fn detail_download_page_is_complete(page: &Page) -> bool {
    !page.tracks.is_empty()
        && page.authoritative_total == Some(page.raw_loaded_count)
        && page.normalized_count == page.raw_loaded_count
        && page.tracks.len() == page.raw_loaded_count
}

fn render_detail_download(snapshot: &DetailDownloadSnapshot) -> AnyElement {
    crate::context_menu::collection_download_button(
        snapshot.id.clone(),
        snapshot.tracks.clone(),
        snapshot.downloads.clone(),
        snapshot.account.clone(),
    )
}

fn reorder_for_page(
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
    if !super::playlist_reorder::route_eligible(route.source, &route.action, &route.id, &route.id) {
        return None;
    }
    super::playlist_reorder::move_eligible(
        page,
        view.playlist_catalog(route.source).is_editable(&route.id),
        !view.query(cx).trim().is_empty(),
        view.playlist_catalog(route.source).reorder_pending,
    )
    .then(|| (route.id.clone(), true))
}

fn reorder_pending_for_page(view: &LibraryView) -> bool {
    (!view.state.route().is_local_route())
        && view
            .playlist_catalog(view.state.route().source)
            .reorder_pending
}

fn detail_edit_id(view: &LibraryView) -> Option<(crate::search::Provider, String)> {
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len())
        || !super::playlist_state::matching_playlist_route(
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

fn detail_more_card(view: &LibraryView) -> Option<Card> {
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
            super::release_year(&route.release_date)
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

fn render_detail_more(card: &Card, host: &gpui::Entity<LibraryView>) -> AnyElement {
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

fn render_local_detail_more(card: &Card, host: &gpui::Entity<LibraryView>) -> AnyElement {
    crate::context_menu::local_playlist_card_menu_button(
        playlist_header_more_button(format!("library-detail-more-local-{}", card.id).into()),
        card.clone(),
        host.clone(),
    )
    .into_any_element()
}

fn render_detail_edit(
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

fn render_local_detail_edit(id: &str, host: &gpui::Entity<LibraryView>) -> AnyElement {
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

fn detail_delete_id(view: &LibraryView) -> Option<(crate::search::Provider, String)> {
    let route = view.state.route();
    if !is_detail_route(view.state.routes.len())
        || !super::playlist_state::matching_playlist_route(
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

fn render_detail_delete(
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

fn detail_favorite_snapshot(view: &LibraryView, app: &gpui::App) -> Option<DetailFavoriteSnapshot> {
    use super::favorite_state::{FavoriteKey, FavoriteKind};
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

fn render_detail_favorite(
    snapshot: &DetailFavoriteSnapshot,
    host: &gpui::Entity<LibraryView>,
) -> AnyElement {
    use super::favorite_state::FavoriteKey;
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

fn artist_section_action(
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
fn artist_section_action_visible(section: &Section, expanded: bool) -> bool {
    artist_section_action_visible_for_mode(section, expanded, false)
}

fn artist_section_action_visible_for_mode(
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
fn render_section(
    view: &LibraryView,
    section: &Section,
    section_index: usize,
    columns: u16,
    narrow: bool,
    favorite_state: gpui::Entity<super::favorite_state::FavoriteState>,
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
                    SectionLayout::Tracks => super::track_view::tracks(
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
                    SectionLayout::Cards => super::cards_view::cards(
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

fn open_account_settings_action(cx: &mut Context<LibraryView>) -> AnyElement {
    primary_button(
        "library-open-account-settings",
        Some(LocalIcon::Settings),
        "Open account settings",
        cx.listener(|this, _, window, cx| {
            this.open_settings(window, cx);
        }),
    )
    .into_any_element()
}

fn message(
    icon: LocalIcon,
    title: &str,
    description: &str,
    action: Option<AnyElement>,
) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_color(rgb(MUTED))
        .child(local_icon(icon, MUTED).size(px(40.)))
        .child(
            div()
                .mt(px(12.))
                .mb(px(4.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(title.to_owned()),
        )
        .when(!description.is_empty(), |this| {
            this.child(div().text_size(px(13.)).child(description.to_owned()))
        })
        .when_some(action, |this, action| {
            this.child(div().mt(px(14.)).child(action))
        })
        .into_any_element()
}

#[cfg(test)]
mod render_contract_tests {
    use super::{playlist_create_button_id, playlist_create_control_visible};
    use crate::library::model::{Category, Service};

    #[test]
    fn create_playlist_control_appears_with_the_playlist_root_heading() {
        assert!(playlist_create_control_visible(
            Service::Deezer,
            Category::Playlists,
            1,
        ));
        assert!(playlist_create_control_visible(
            Service::SoundCloud,
            Category::Playlists,
            1,
        ));
        assert!(playlist_create_control_visible(
            Service::Local,
            Category::Playlists,
            1,
        ));

        for (service, category, route_depth) in [
            (Service::Deezer, Category::Artists, 1),
            (Service::Deezer, Category::Playlists, 2),
            (Service::SoundCloud, Category::Playlists, 2),
            (Service::Local, Category::Playlists, 2),
        ] {
            assert!(!playlist_create_control_visible(
                service,
                category,
                route_depth,
            ));
        }

        assert_eq!(
            playlist_create_button_id(Service::Deezer),
            "create-deezer-playlist"
        );
        assert_eq!(
            playlist_create_button_id(Service::SoundCloud),
            "create-soundcloud-playlist"
        );
        assert_eq!(
            playlist_create_button_id(Service::Local),
            "create-local-playlist"
        );
    }
}

#[cfg(test)]
#[path = "content_view_tests.rs"]
mod tests;
