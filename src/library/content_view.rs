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

mod detail_actions;
mod sections;
use detail_actions::*;
use sections::{playback_context, render_section, render_section_page_list};

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
        ai_generated: kind == crate::search::ResultType::Albums
            && page.tracks.iter().any(|track| track.ai_generated),
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
