use gpui::{
    AnyElement, App, Context, Entity, KeyDownEvent, MouseButton, SharedString, div, prelude::*, px,
    rgb,
};
use std::{fmt::Write as _, sync::Arc};

use crate::{
    assets::LocalIcon,
    browser_scroll::{BrowserScrollTarget, browser_scroll_surface},
    context_menu,
    drag_cursor::{DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned},
    entity_navigation::{ContextRoute, NavigationOpener, artist_routes_for_track},
    music_ui::{
        TRACK_ACTION_GAP_PX, TrackArtistNavigation, TrackRowDisplay, row_download_button,
        row_more_button, track_row_with_action,
    },
    playback::PlaybackTrack,
    playing_indicator::{PlayingSnapshot, QueuePlayingSnapshot},
    theme::PRIMARY,
};

use super::{
    favorite_state::FavoriteState,
    model::{Page, Track},
    playlist_drag::{PlaylistTrackDrag, PlaylistTrackDragGhost},
    view::LibraryView,
    virtualization::{self, TrackListLayout},
};

#[derive(Clone)]
pub(super) struct LibraryTrackRows {
    host: Entity<LibraryView>,
    tracks: Arc<Vec<Track>>,
    queue: Arc<Vec<PlaybackTrack>>,
    source_indices: Arc<Vec<usize>>,
    narrow: bool,
    removal: Option<(String, bool, bool)>,
    removal_pending: bool,
    reorder: Option<(String, bool)>,
    reorder_pending: bool,
    playing: QueuePlayingSnapshot,
    favorite_root_active: bool,
    playback: Entity<crate::playback::PlaybackModel>,
    context: crate::playback::PlaybackContext,
    downloads: Entity<crate::downloads::DownloadModel>,
    account: Entity<crate::settings::AccountState>,
    external_navigation: Option<crate::entity_navigation::ProviderNavigationOpeners>,
    navigation_context: Option<ContextRoute>,
    row_identity: String,
}

#[allow(clippy::too_many_arguments)]
impl LibraryTrackRows {
    pub(super) fn new(
        items: &[Track],
        preview_limit: Option<usize>,
        narrow: bool,
        favorites: bool,
        playing: PlayingSnapshot,
        favorite_state: Entity<FavoriteState>,
        removal: Option<(String, bool, bool)>,
        removal_pending: bool,
        reorder: Option<(String, bool)>,
        reorder_pending: bool,
        provider: crate::search::Provider,
        external_navigation: Option<crate::entity_navigation::ProviderNavigationOpeners>,
        favorite_root_active: bool,
        context: crate::playback::PlaybackContext,
        playback: Entity<crate::playback::PlaybackModel>,
        downloads: Entity<crate::downloads::DownloadModel>,
        account: Entity<crate::settings::AccountState>,
        navigation_context: Option<ContextRoute>,
        row_identity: String,
        cx: &Context<LibraryView>,
    ) -> Self {
        Self::new_with_source(
            items,
            None,
            None,
            preview_limit,
            narrow,
            favorites,
            playing,
            favorite_state,
            removal,
            removal_pending,
            reorder,
            reorder_pending,
            provider,
            external_navigation,
            favorite_root_active,
            context,
            playback,
            downloads,
            account,
            navigation_context,
            row_identity,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new_with_source(
        items: &[Track],
        source_tracks: Option<&[Track]>,
        source_indices: Option<&[usize]>,
        preview_limit: Option<usize>,
        narrow: bool,
        _favorites: bool,
        playing: PlayingSnapshot,
        _favorite_state: Entity<FavoriteState>,
        removal: Option<(String, bool, bool)>,
        removal_pending: bool,
        reorder: Option<(String, bool)>,
        reorder_pending: bool,
        provider: crate::search::Provider,
        external_navigation: Option<crate::entity_navigation::ProviderNavigationOpeners>,
        favorite_root_active: bool,
        context: crate::playback::PlaybackContext,
        playback: Entity<crate::playback::PlaybackModel>,
        downloads: Entity<crate::downloads::DownloadModel>,
        account: Entity<crate::settings::AccountState>,
        navigation_context: Option<ContextRoute>,
        row_identity: String,
        cx: &Context<LibraryView>,
    ) -> Self {
        let host = cx.entity().clone();
        let virtualized = virtualization::should_virtualize(preview_limit, items.len());
        let displayed_count = virtualization::displayed_track_count(
            items.len(),
            (!virtualized).then_some(preview_limit.unwrap_or(items.len())),
        );
        let tracks = Arc::new(
            items
                .iter()
                .take(displayed_count)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let source_indices = Arc::new(
            source_indices
                .map(|indices| {
                    indices
                        .iter()
                        .copied()
                        .take(displayed_count)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| (0..displayed_count).collect()),
        );
        let source_tracks = source_tracks.unwrap_or(items);
        let queue = Arc::new(playback_queue(source_tracks, provider));
        let playing = playing.for_queue(&queue);
        Self {
            host,
            tracks,
            queue,
            source_indices,
            narrow,
            removal,
            removal_pending,
            reorder,
            reorder_pending,
            playing,
            favorite_root_active,
            playback,
            context,
            downloads,
            account,
            external_navigation,
            navigation_context,
            row_identity,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.tracks.len()
    }

    pub(super) fn render(&self, index: usize, app: &App) -> AnyElement {
        let queue_index = queue_index_for_row(&self.source_indices, index);
        render_track_item(
            &self.host,
            &self.tracks,
            &self.queue,
            index,
            queue_index,
            self.narrow,
            self.removal.as_ref(),
            self.removal_pending,
            self.reorder.as_ref(),
            self.reorder_pending,
            &self.playing,
            self.favorite_root_active,
            &self.playback,
            &self.context,
            &self.downloads,
            &self.account,
            self.external_navigation.as_ref(),
            self.navigation_context.as_ref(),
            &self.row_identity,
            app,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn tracks(
    view: &LibraryView,
    items: &[Track],
    preview_limit: Option<usize>,
    narrow: bool,
    favorites: bool,
    playing: PlayingSnapshot,
    favorite_state: Entity<FavoriteState>,
    removal: Option<(String, bool, bool)>,
    removal_pending: bool,
    reorder: Option<(String, bool)>,
    reorder_pending: bool,
    provider: crate::search::Provider,
    external_navigation: Option<crate::entity_navigation::ProviderNavigationOpeners>,
    favorite_root_active: bool,
    context: crate::playback::PlaybackContext,
    playback: Entity<crate::playback::PlaybackModel>,
    downloads: Entity<crate::downloads::DownloadModel>,
    account: Entity<crate::settings::AccountState>,
    navigation_context: Option<ContextRoute>,
    identity: &str,
    cx: &mut Context<LibraryView>,
) -> AnyElement {
    let query = view.query(cx);
    let (source_tracks, source_indices) = source_projection(view.state.page.as_ref(), &query);
    let rows = LibraryTrackRows::new_with_source(
        items,
        source_tracks,
        source_indices.as_deref(),
        preview_limit,
        narrow,
        favorites,
        playing,
        favorite_state,
        removal,
        removal_pending,
        reorder,
        reorder_pending,
        provider,
        external_navigation,
        favorite_root_active,
        context,
        playback,
        downloads,
        account,
        navigation_context,
        identity.to_owned(),
        cx,
    );
    let virtualized = virtualization::should_virtualize(preview_limit, items.len());
    let route = view.state.route();
    let load_id = view
        .state
        .active_tracks_load_id()
        .map(|load_id| load_id.to_string())
        .unwrap_or_default();
    let route_key = format!(
        "{}:{}:{}:{}:{}:{}",
        route.source.label(),
        route.category.label(),
        route.action,
        route.id,
        load_id,
        identity
    );
    let layout = TrackListLayout::new(narrow, false, false);
    if virtualized {
        let content_key = virtualization::content_identity(&route_key, &query, std::iter::empty());
        let row_identities = items
            .iter()
            .take(rows.len())
            .map(library_track_identity)
            .collect::<Vec<_>>();
        let list_state =
            view.track_list_state_with_rows(&content_key, rows.len(), layout, &row_identities);
        let browser_scroll = view.track_list_browser_scroll(&content_key);
        let fixed_scroll = crate::browser_scroll::FixedListScrollHandle::new(
            list_state.clone(),
            rows.len(),
            virtualization::row_height(),
        );
        let list_rows = rows.clone();
        let list = gpui::list(list_state.clone(), move |index, _window, app| {
            div()
                .w_full()
                .h(virtualization::row_height())
                .flex_none()
                .child(list_rows.render(index, app))
                .into_any_element()
        });
        let content = super::playlist_drag::playlist_drag_surface(
            div()
                .id("library-track-drag-scroll")
                .w_full()
                .flex_1()
                .min_h_0()
                .relative()
                .child(list.w_full().h_full().min_h_0())
                .child(virtualization::library_vertical_scrollbar(
                    &fixed_scroll,
                    narrow,
                )),
            fixed_scroll.clone(),
            cx,
        )
        .into_any_element();
        return browser_scroll_surface(
            "library-track-list-scroll",
            content,
            BrowserScrollTarget::FixedList(fixed_scroll),
            browser_scroll,
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(f32::from(virtualization::row_gap())))
        .children((0..rows.len()).map(|index| rows.render(index, cx)))
        .into_any_element()
}

pub(super) fn library_track_identity(track: &Track) -> String {
    let origin = track.origin.map(|provider| provider.label());
    let id = track.id.trim();
    if !id.is_empty() {
        return origin
            .map(|origin| format!("{origin}:{id}"))
            .unwrap_or_else(|| id.to_owned());
    }

    let mut identity = format!("{}:{}:{}:", origin.unwrap_or(""), track.title, track.artist);
    for (index, artist) in track.artists.iter().enumerate() {
        if index > 0 {
            identity.push(',');
        }
        write!(identity, "{}={}", artist.id, artist.name).expect("writing to a string");
    }
    write!(
        identity,
        ":{}:{}:{}:{}:{}:{}",
        track.album_id,
        track.album,
        track.artwork,
        track.duration,
        track.explicit,
        track.service_url,
    )
    .expect("writing to a string");
    identity
}

fn queue_index_for_row(source_indices: &[usize], row_index: usize) -> usize {
    source_indices.get(row_index).copied().unwrap_or(row_index)
}

fn source_projection<'a>(
    page: Option<&'a Page>,
    query: &str,
) -> (Option<&'a [Track]>, Option<Vec<usize>>) {
    page.filter(|page| !page.uses_sections())
        .map(|page| {
            (
                Some(page.tracks.as_slice()),
                Some(super::filter::matching_track_indices(&page.tracks, query)),
            )
        })
        .unwrap_or((None, None))
}

fn playback_queue(
    source_tracks: &[Track],
    provider: crate::search::Provider,
) -> Vec<PlaybackTrack> {
    source_tracks
        .iter()
        .map(|track| {
            crate::playback::PlaybackTrack::from_local_library(track)
                .unwrap_or_else(|| crate::playback::PlaybackTrack::from_library(track, provider))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn render_track_item(
    host: &Entity<LibraryView>,
    tracks: &Arc<Vec<Track>>,
    queue: &Arc<Vec<PlaybackTrack>>,
    index: usize,
    queue_index: usize,
    narrow: bool,
    removal: Option<&(String, bool, bool)>,
    removal_pending: bool,
    reorder: Option<&(String, bool)>,
    reorder_pending: bool,
    playing: &QueuePlayingSnapshot,
    favorite_root_active: bool,
    playback: &Entity<crate::playback::PlaybackModel>,
    context: &crate::playback::PlaybackContext,
    downloads: &Entity<crate::downloads::DownloadModel>,
    account: &Entity<crate::settings::AccountState>,
    external_navigation: Option<&crate::entity_navigation::ProviderNavigationOpeners>,
    navigation_context: Option<&ContextRoute>,
    row_identity: &str,
    app: &App,
) -> AnyElement {
    let track = &tracks[index];
    let playback_track = &queue[queue_index];
    let row_provider = match playback_track.provider {
        crate::playback::PlaybackProvider::Deezer => crate::search::Provider::Deezer,
        crate::playback::PlaybackProvider::SoundCloud => crate::search::Provider::SoundCloud,
    };
    let (context_open_album, context_open_artist) = external_navigation
        .map(|openers| openers.for_provider(row_provider))
        .unwrap_or_else(|| local_navigation_openers(host, row_provider));
    let secondary_action = removal.and_then(|(playlist, proven, local)| {
        (*proven && (*local || super::playlist_client::valid_id(&track.id).is_ok())).then(|| {
            remove_button(
                playlist,
                track,
                row_provider,
                removal_pending,
                *local,
                host,
                row_identity,
                index,
            )
        })
    });
    let row_playing = playing.row(queue_index);
    let row_blocked = playing.blocks(playback_track);
    let click_playback = playback.clone();
    let key_playback = playback.clone();
    let click_context = context.clone();
    let key_context = context.clone();
    let click_queue = queue.clone();
    let key_queue = queue.clone();
    let artist_navigation = {
        let routes = artist_routes_for_track(row_provider, &track.artists);
        (!routes.is_empty()).then(|| TrackArtistNavigation {
            provider: row_provider,
            routes,
            opener: context_open_artist.clone(),
        })
    };
    let tertiary_action = Some(
        div()
            .flex()
            .gap(px(TRACK_ACTION_GAP_PX))
            .child(download_button(
                SharedString::from(format!(
                    "library-track-download-{row_identity}-{index}-{}",
                    track.id
                )),
                playback_track,
                downloads,
                account,
            ))
            .child(
                context_menu::track_menu_button(
                    row_more_button(SharedString::from(format!(
                        "library-track-more-{row_identity}-{index}-{}",
                        track.id
                    ))),
                    context_menu::queue_track_entity(playback_track),
                    playback.clone(),
                    downloads.clone(),
                    account.clone(),
                    host.clone(),
                    context_open_album.clone(),
                    context_open_artist.clone(),
                    favorite_root_active,
                    navigation_context.cloned(),
                )
                .into_any_element(),
            )
            .into_any_element(),
    );
    let row = track_row_with_action(
        queue_index,
        row_identity,
        &track.title,
        &track.artist,
        artist_navigation,
        &track.artwork,
        track.duration,
        TrackRowDisplay {
            provider: track.origin,
            narrow,
            provider_icon_only: narrow,
        },
        track.explicit,
        row_playing,
        None,
        row_blocked,
        None,
        secondary_action,
        tertiary_action,
        app,
    )
    .id(format!("library-playback-row-{row_identity}-{queue_index}"))
    .relative()
    .focusable()
    .tab_stop(!row_blocked)
    .role(gpui::Role::Button)
    .focus_visible(|style| {
        style
            .border_1()
            .border_color(gpui::rgb(crate::theme::PRIMARY))
    })
    .cursor_pointer()
    .on_click(move |_, _, cx| {
        click_playback.update(cx, |playback, cx| {
            if !playback.state.explicit_blocked(&click_queue[queue_index]) {
                playback.replace_queue((*click_queue).clone(), queue_index, cx);
                playback.set_context(click_context.clone(), cx);
            }
        })
    })
    .on_key_down(move |event: &KeyDownEvent, window, cx| {
        if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
            window.prevent_default();
            key_playback.update(cx, |playback, cx| {
                if !playback.state.explicit_blocked(&key_queue[queue_index]) {
                    playback.replace_queue((*key_queue).clone(), queue_index, cx);
                    playback.set_context(key_context.clone(), cx);
                }
            });
        }
    });

    let reorder_enabled = reorder.is_some() && !reorder_pending;
    let row = if reorder_enabled {
        let drag_title = track.title.clone();
        let drop_host = host.clone();
        let cursor_owner = DragCursorOwner::playlist();
        row.on_drag_move::<PlaylistTrackDrag>(move |_, window, cx| {
            set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
            cx.set_active_drag_cursor_style(grabbing_cursor(), window);
        })
        .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
            }
        })
        .on_drag(
            PlaylistTrackDrag {
                from_index: queue_index,
            },
            move |_, _, window, cx| {
                set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
                cx.set_active_drag_cursor_style(grabbing_cursor(), window);
                cx.new(|_| PlaylistTrackDragGhost {
                    title: drag_title.clone(),
                })
            },
        )
        .on_drop::<PlaylistTrackDrag>(move |drag, window, cx| {
            if drag.from_index != queue_index {
                drop_host.update(cx, |this, cx| {
                    this.move_reorderable_track(drag.from_index, queue_index, cx)
                });
            }
            set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
        })
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(6.))
                .right(px(12.))
                .drag_over::<PlaylistTrackDrag>(move |style, drag, _, _| {
                    if drag.from_index > queue_index {
                        style.border_t_2().border_color(rgb(PRIMARY))
                    } else if drag.from_index < queue_index {
                        style.border_b_2().border_color(rgb(PRIMARY))
                    } else {
                        style.opacity(0.)
                    }
                }),
        )
    } else {
        row
    };

    context_menu::track_menu(
        row,
        context_menu::queue_track_entity(playback_track),
        playback.clone(),
        downloads.clone(),
        account.clone(),
        host.clone(),
        context_open_album.clone(),
        context_open_artist.clone(),
        favorite_root_active,
        navigation_context.cloned(),
    )
    .into_any_element()
}

fn local_navigation_openers(
    host: &Entity<LibraryView>,
    provider: crate::search::Provider,
) -> (NavigationOpener, NavigationOpener) {
    let album_host = host.clone();
    let album: NavigationOpener = Arc::new(move |target, _, cx| {
        album_host.update(cx, |view, cx| {
            view.open_card(target.library_card(provider), cx)
        });
    });
    let artist_host = host.clone();
    let artist: NavigationOpener = Arc::new(move |target, _, cx| {
        artist_host.update(cx, |view, cx| {
            view.open_card(target.library_card(provider), cx)
        });
    });
    (album, artist)
}

fn download_button(
    id: SharedString,
    track: &PlaybackTrack,
    downloads: &Entity<crate::downloads::DownloadModel>,
    account: &Entity<crate::settings::AccountState>,
) -> AnyElement {
    let button = row_download_button(id);
    context_menu::track_download_button_above(
        button,
        track.clone(),
        downloads.clone(),
        account.clone(),
    )
    .into_any_element()
}

fn remove_button(
    playlist: &str,
    track: &Track,
    provider: crate::search::Provider,
    pending: bool,
    local: bool,
    host: &Entity<LibraryView>,
    row_identity: &str,
    index: usize,
) -> AnyElement {
    let playlist = playlist.to_owned();
    let track_id = track.id.clone();
    let host = host.clone();
    crate::music_ui::danger_row_action_button(
        SharedString::from(format!(
            "library-track-remove-{row_identity}-{index}-{track_id}"
        )),
        LocalIcon::X,
        "Remove from playlist",
        pending,
        move |_, _, cx| {
            cx.stop_propagation();
            host.update(cx, |this, cx| {
                if local {
                    this.remove_local_playlist_track(
                        playlist.clone(),
                        provider,
                        track_id.clone(),
                        cx,
                    );
                } else {
                    this.remove_track_from_playlist(playlist.clone(), track_id.clone(), true, cx);
                }
            });
        },
    )
}

#[cfg(test)]
mod favorite_tests {
    use super::{
        Page, Track, library_track_identity, playback_queue, queue_index_for_row,
        source_projection, virtualization,
    };

    #[test]
    fn track_rows_keep_download_before_more_and_playlist_controls() {
        let source = include_str!("track_view.rs");
        let download_helper = ["fn ", "download_button("].concat();
        let favorite_helper = ["fn ", "favorite_button("].concat();
        let old_reorder_helper = ["reorder_", "buttons("].concat();
        assert!(source.contains("track_menu_button("));
        assert!(source.contains("track_download_button_above("));
        assert!(source.contains("remove_button("));
        assert!(source.contains("PlaylistTrackDrag"));
        assert!(source.contains(".drag_over::<PlaylistTrackDrag>"));
        assert!(!source.contains(&old_reorder_helper));
        assert!(source.contains(&download_helper));
        assert!(!source.contains(&favorite_helper));
        let actions = source
            .split("let tertiary_action = Some(")
            .nth(1)
            .and_then(|source| source.split("let row = track_row_with_action(").next())
            .expect("track action source");
        let download = actions.find(".child(download_button(").unwrap();
        let more = actions.find("context_menu::track_menu_button(").unwrap();
        assert!(download < more);
    }

    #[test]
    fn track_rendering_does_not_reborrow_the_library_entity() {
        let source = include_str!("track_view.rs");
        let read_call = ["host", ".read("].concat();
        assert!(!source.contains(&read_call));
    }

    #[test]
    fn library_artist_rows_use_the_context_artist_opener() {
        let source = include_str!("track_view.rs");
        assert!(source.contains("opener: context_open_artist.clone()"));
    }

    #[test]
    fn local_playlist_rows_remove_by_provider_and_track_id_without_numeric_gate() {
        let source = include_str!("track_view.rs");
        let source = &source[..source.find("#[cfg(test)]").unwrap()];
        assert!(source.contains("remove_local_playlist_track"));
        assert!(source.contains("*local || super::playlist_client::valid_id(&track.id).is_ok()"));
        assert!(source.contains("provider: crate::search::Provider"));
    }

    #[test]
    fn filtered_rows_address_their_original_queue_positions() {
        let source_indices = [43, 44];

        assert_eq!(queue_index_for_row(&source_indices, 0), 43);
        assert_eq!(queue_index_for_row(&source_indices, 1), 44);
    }

    #[test]
    fn unfiltered_rows_keep_their_existing_position_fallback() {
        assert_eq!(queue_index_for_row(&[], 3), 3);
    }

    #[test]
    fn filtered_row_keeps_source_position_and_complete_playback_queue() {
        let source_tracks = (0..48)
            .map(|index| Track {
                id: index.to_string(),
                title: if index == 43 {
                    "Find me".into()
                } else {
                    format!("Track {index}")
                },
                ..Track::default()
            })
            .collect::<Vec<_>>();
        let page = Page {
            tracks: source_tracks,
            ..Page::default()
        };

        let (source, indices) = source_projection(Some(&page), "find");
        let source = source.expect("flat library page provides the playback source");
        let indices = indices.expect("query provides filtered source indices");
        let queue = playback_queue(source, crate::search::Provider::Deezer);

        assert_eq!(indices, vec![43]);
        assert_eq!(queue_index_for_row(&indices, 0), 43);
        assert_eq!(queue.len(), 48);
        assert_eq!(queue[43].id, "43");
        assert_eq!(queue[44].id, "44");
    }

    #[test]
    fn local_queue_and_identity_preserve_each_tracks_provider() {
        let tracks = vec![
            Track {
                origin: Some(crate::search::Provider::Deezer),
                id: "42".into(),
                ..Track::default()
            },
            Track {
                origin: Some(crate::search::Provider::SoundCloud),
                id: "42".into(),
                ..Track::default()
            },
        ];

        let queue = playback_queue(&tracks, crate::search::Provider::Deezer);
        assert_eq!(queue[0].provider, crate::playback::PlaybackProvider::Deezer);
        assert_eq!(
            queue[1].provider,
            crate::playback::PlaybackProvider::SoundCloud
        );
        assert_ne!(
            library_track_identity(&tracks[0]),
            library_track_identity(&tracks[1])
        );
    }

    #[test]
    fn metadata_refresh_preserves_list_identity_and_duplicate_positions() {
        let raw = vec![
            Track {
                id: " 42 ".into(),
                title: "Raw title".into(),
                ..Track::default()
            },
            Track {
                id: "42".into(),
                title: "Duplicate raw title".into(),
                ..Track::default()
            },
            Track {
                id: "7".into(),
                ..Track::default()
            },
        ];
        let enriched = vec![
            Track {
                id: "42".into(),
                title: "Enriched title".into(),
                artist: "Artist".into(),
                duration: 180,
                ..Track::default()
            },
            Track {
                id: "42".into(),
                title: "Duplicate enriched title".into(),
                artwork: "cover".into(),
                ..Track::default()
            },
            Track {
                id: "7".into(),
                album: "Album".into(),
                ..Track::default()
            },
        ];
        let identity = |tracks: &[Track]| {
            virtualization::content_identity(
                "deezer-tracks",
                "",
                tracks.iter().map(library_track_identity),
            )
        };

        assert_eq!(identity(&raw), identity(&enriched));
        assert_ne!(identity(&raw), identity(&enriched[1..]));
    }

    #[test]
    fn idless_tracks_fall_back_to_metadata_identity() {
        let first = Track {
            title: "First title".into(),
            artist: "Artist".into(),
            ..Track::default()
        };
        let renamed = Track {
            title: "Renamed title".into(),
            ..first.clone()
        };

        assert_ne!(
            library_track_identity(&first),
            library_track_identity(&renamed)
        );
    }
}
