use gpui::{AnyElement, App, Context, Entity, KeyDownEvent, SharedString, Window, div, prelude::*};
use std::sync::Arc;

use crate::{
    assets::LocalIcon,
    browser_scroll::{BrowserScrollTarget, FixedListScrollHandle, browser_scroll_surface},
    context_menu::{self, track_menu},
    drag_cursor::{DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned},
    entity_navigation::{
        ContextRoute, NavigationOpener, NavigationTarget, artist_routes_for_track,
    },
    music_ui::{
        TRACK_ACTION_GAP_PX, TrackArtistNavigation, TrackRowDisplay, danger_row_action_button,
        row_download_button, row_more_button, track_row_with_action,
    },
    playback::PlaybackTrack,
    playing_indicator::PlayingSnapshot,
    theme::PRIMARY,
};

use crate::library::playlist_drag::{PlaylistTrackDrag, PlaylistTrackDragGhost};

use super::{
    models::{Provider, Track},
    view::SearchView,
};

#[derive(Clone)]
pub(super) struct SearchTrackRows {
    view: Entity<SearchView>,
    tracks: Arc<Vec<Track>>,
    queue: Arc<Vec<PlaybackTrack>>,
    narrow: bool,
    provider_icon_only: bool,
    show_provider_badge: bool,
    removal: Option<(String, bool)>,
    removal_pending: bool,
    reorder_enabled: bool,
    playback: Entity<crate::playback::PlaybackModel>,
    downloads: Entity<crate::downloads::DownloadModel>,
    account: Entity<crate::settings::AccountState>,
    navigation_context: Option<ContextRoute>,
    playing: PlayingSnapshot,
    deezer_openers: (NavigationOpener, NavigationOpener),
    soundcloud_openers: (NavigationOpener, NavigationOpener),
}

#[allow(clippy::too_many_arguments)]
impl SearchTrackRows {
    pub(super) fn new(
        search: &SearchView,
        tracks: &[Track],
        preview: bool,
        narrow: bool,
        provider_icon_only: bool,
        show_provider_badge: bool,
        _actions: bool,
        _favorites: Entity<crate::library::FavoriteState>,
        removal: Option<(String, bool)>,
        removal_pending: bool,
        playback: gpui::Entity<crate::playback::PlaybackModel>,
        downloads: Entity<crate::downloads::DownloadModel>,
        account: Entity<crate::settings::AccountState>,
        navigation_context: Option<ContextRoute>,
        playing: PlayingSnapshot,
        cx: &Context<SearchView>,
    ) -> Self {
        let reorder_enabled =
            playlist_row_reorder_enabled(preview, search.playlist_reorder_detail_eligible(cx));
        let view = cx.entity().clone();
        let virtualized = !preview && !tracks.is_empty();
        let displayed_count = crate::library::virtualization::displayed_track_count(
            tracks.len(),
            (!virtualized).then_some(preview.then_some(5).unwrap_or(tracks.len())),
        );
        let rendered_tracks = Arc::new(
            tracks
                .iter()
                .take(displayed_count)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let queue = Arc::new(
            tracks
                .iter()
                .map(crate::playback::PlaybackTrack::from_search)
                .collect::<Vec<_>>(),
        );
        Self {
            view: view.clone(),
            tracks: rendered_tracks,
            queue,
            narrow,
            provider_icon_only,
            show_provider_badge,
            removal,
            removal_pending,
            reorder_enabled,
            playback,
            downloads,
            account,
            navigation_context,
            playing,
            deezer_openers: openers(&view, Provider::Deezer),
            soundcloud_openers: openers(&view, Provider::SoundCloud),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.tracks.len()
    }

    pub(super) fn render(&self, index: usize, app: &App) -> AnyElement {
        render_track_item(
            &self.view,
            &self.tracks,
            &self.queue,
            index,
            self.narrow,
            self.provider_icon_only,
            self.show_provider_badge,
            self.removal.as_ref(),
            self.removal_pending,
            self.reorder_enabled,
            &self.playback,
            &self.downloads,
            &self.account,
            self.navigation_context.as_ref(),
            &self.playing,
            &self.deezer_openers,
            &self.soundcloud_openers,
            app,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_tracks(
    view: &SearchView,
    tracks: &[Track],
    preview: bool,
    narrow: bool,
    provider_icon_only: bool,
    show_provider_badge: bool,
    actions: bool,
    favorites: Entity<crate::library::FavoriteState>,
    removal: Option<(String, bool)>,
    removal_pending: bool,
    playback: gpui::Entity<crate::playback::PlaybackModel>,
    downloads: Entity<crate::downloads::DownloadModel>,
    account: Entity<crate::settings::AccountState>,
    navigation_context: Option<ContextRoute>,
    playing: PlayingSnapshot,
    identity: &str,
    cx: &mut Context<SearchView>,
) -> AnyElement {
    let rows = SearchTrackRows::new(
        view,
        tracks,
        preview,
        narrow,
        provider_icon_only,
        show_provider_badge,
        actions,
        favorites,
        removal,
        removal_pending,
        playback,
        downloads,
        account,
        navigation_context,
        playing,
        cx,
    );
    let virtualized = !preview && !tracks.is_empty();
    if virtualized {
        let content_key = crate::library::virtualization::content_identity(
            identity,
            "",
            tracks.iter().map(search_track_identity),
        );
        let layout =
            crate::library::virtualization::TrackListLayout::new(narrow, provider_icon_only, false);
        let list_state = view.track_list_state(&content_key, rows.len(), layout);
        let browser_scroll = view.track_list_browser_scroll(&content_key);
        let fixed_scroll = FixedListScrollHandle::new(
            list_state.clone(),
            rows.len(),
            crate::library::virtualization::row_height(),
        );
        let list_rows = rows.clone();
        let list = gpui::list(list_state.clone(), move |index, _window, app| {
            track_row_slot(list_rows.render(index, app), true)
        });
        let content = playlist_drag_surface(
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .relative()
                .child(list.w_full().h_full().min_h_0())
                .child(crate::library::virtualization::library_vertical_scrollbar(
                    &fixed_scroll,
                    narrow,
                )),
            list_state,
            cx,
        )
        .into_any_element();
        return browser_scroll_surface(
            "search-track-list-scroll",
            content,
            BrowserScrollTarget::FixedList(fixed_scroll),
            browser_scroll,
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .children(
            (0..rows.len())
                .map(|index| track_row_slot(rows.render(index, cx), index + 1 < rows.len())),
        )
        .into_any_element()
}

fn playlist_drag_surface(
    surface: gpui::Div,
    list_state: gpui::ListState,
    cx: &mut Context<SearchView>,
) -> gpui::Div {
    surface
        .on_mouse_up_out(gpui::MouseButton::Left, |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::playlist());
            }
        })
        .on_mouse_up(gpui::MouseButton::Left, |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::playlist());
            }
        })
        .on_drag_move::<PlaylistTrackDrag>(cx.listener(move |this, event, window, cx| {
            this.update_playlist_drag_autoscroll(event, list_state.clone(), cx);
            set_drag_cursor_owned(
                window,
                DragCursorState::Grabbing,
                DragCursorOwner::playlist(),
            );
            cx.set_active_drag_cursor_style(grabbing_cursor(), window);
        }))
}

pub(super) fn track_row_slot(row: AnyElement, include_gap_after: bool) -> AnyElement {
    div()
        .w_full()
        .h(track_row_slot_height(include_gap_after))
        .flex_none()
        .child(row)
        .into_any_element()
}

fn track_row_slot_height(include_gap_after: bool) -> gpui::Pixels {
    if include_gap_after {
        crate::library::virtualization::row_height()
    } else {
        crate::library::virtualization::row_content_height()
    }
}

pub(super) fn search_track_identity(track: &Track) -> String {
    let artists = track
        .artists
        .iter()
        .map(|artist| format!("{}={}", artist.id, artist.name))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
        track.id,
        track.title,
        track.artist,
        artists,
        track.album_id,
        track.album,
        track.artwork,
        track.duration,
        track.source.label(),
        track.explicit,
        track.service_url,
        track.downloadable,
        track.progressive,
        track
            .favorite
            .map(|favorite| favorite.to_string())
            .unwrap_or_default()
    )
}

#[allow(clippy::too_many_arguments)]
fn render_track_item(
    view: &Entity<SearchView>,
    tracks: &Arc<Vec<Track>>,
    queue: &Arc<Vec<PlaybackTrack>>,
    index: usize,
    narrow: bool,
    provider_icon_only: bool,
    show_provider_badge: bool,
    removal: Option<&(String, bool)>,
    removal_pending: bool,
    reorder_enabled: bool,
    playback: &Entity<crate::playback::PlaybackModel>,
    downloads: &Entity<crate::downloads::DownloadModel>,
    account: &Entity<crate::settings::AccountState>,
    navigation_context: Option<&ContextRoute>,
    playing: &PlayingSnapshot,
    deezer_openers: &(NavigationOpener, NavigationOpener),
    soundcloud_openers: &(NavigationOpener, NavigationOpener),
    app: &App,
) -> AnyElement {
    let Some(track) = tracks.get(index) else {
        return div().into_any_element();
    };
    let Some(queue_track) = queue.get(index) else {
        return div().into_any_element();
    };
    let action = removal
        .as_ref()
        .filter(|(_, proven)| *proven && super::detail::validate_id(&track.id).is_ok())
        .and_then(|(playlist, _)| {
            (track.source == Provider::Deezer)
                .then(|| remove_button(index, playlist, track, removal_pending, view))
        });
    let row_playing = playing.row_in_queue(queue_track, index, queue);
    let row_blocked = playing.blocks(queue_track);
    let click_playback = playback.clone();
    let key_playback = playback.clone();
    let (open_album, open_artist) = match track.source {
        Provider::Deezer => deezer_openers.clone(),
        Provider::SoundCloud => soundcloud_openers.clone(),
    };
    let artist_navigation = {
        let routes = artist_routes_for_track(track.source, &track.artists);
        (!routes.is_empty()).then(|| TrackArtistNavigation {
            provider: track.source,
            routes,
            opener: open_artist.clone(),
        })
    };
    let more_action = context_menu::track_menu_button(
        row_more_button(SharedString::from(format!(
            "search-track-more-{index}-{}-{}",
            track.source.label(),
            track.id
        ))),
        context_menu::queue_track_entity(queue_track),
        playback.clone(),
        downloads.clone(),
        account.clone(),
        view.clone(),
        open_album.clone(),
        open_artist.clone(),
        false,
        navigation_context.cloned(),
    )
    .into_any_element();
    let trailing_actions = div()
        .flex()
        .gap(gpui::px(TRACK_ACTION_GAP_PX))
        .child(download_button(index, queue_track, downloads, account))
        .child(more_action)
        .into_any_element();
    let click_queue = queue.clone();
    let key_queue = queue.clone();
    let playback_context = navigation_context
        .and_then(|route| {
            crate::playback::PlaybackContext::soundcloud_collection(
                route.provider,
                &route.action,
                &route.id,
            )
        })
        .unwrap_or(crate::playback::PlaybackContext::None);
    let click_context = playback_context.clone();
    let key_context = playback_context;
    let row = track_row_with_action(
        index,
        "search",
        &track.title,
        &track.artist,
        artist_navigation,
        &track.artwork,
        track.duration,
        TrackRowDisplay {
            provider: row_provider(show_provider_badge, track.source),
            narrow,
            provider_icon_only,
        },
        track.explicit,
        row_playing,
        None,
        row_blocked,
        action,
        None,
        Some(trailing_actions),
        app,
    )
    .id(("search-playback-row", index))
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
            let Some(selected) = click_queue.get(index) else {
                return;
            };
            if !playback.state.explicit_blocked(selected) {
                playback.replace_queue((*click_queue).clone(), index, cx);
                playback.set_context(click_context.clone(), cx);
            }
        })
    })
    .on_key_down(move |event: &KeyDownEvent, window, cx| {
        if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
            window.prevent_default();
            key_playback.update(cx, |playback, cx| {
                let Some(selected) = key_queue.get(index) else {
                    return;
                };
                if !playback.state.explicit_blocked(selected) {
                    playback.replace_queue((*key_queue).clone(), index, cx);
                    playback.set_context(key_context.clone(), cx);
                }
            });
        }
    });

    let row = if reorder_enabled {
        let drag_title = track.title.clone();
        let drop_view = view.clone();
        let cursor_owner = DragCursorOwner::playlist();
        row.on_drag_move::<PlaylistTrackDrag>(move |_, window, cx| {
            set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
            cx.set_active_drag_cursor_style(grabbing_cursor(), window);
        })
        .on_mouse_up_out(gpui::MouseButton::Left, move |_, window, cx| {
            if cx.has_active_drag() {
                set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
            }
        })
        .on_drag(
            PlaylistTrackDrag { from_index: index },
            move |_, _, window, cx| {
                set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
                cx.set_active_drag_cursor_style(grabbing_cursor(), window);
                cx.new(|_| PlaylistTrackDragGhost {
                    title: drag_title.clone(),
                })
            },
        )
        .on_drop::<PlaylistTrackDrag>(move |drag, window, cx| {
            if drag.from_index != index {
                drop_view.update(cx, |this, cx| {
                    this.move_playlist_track(drag.from_index, index, cx)
                });
            }
            set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
        })
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(gpui::px(6.))
                .right(gpui::px(12.))
                .drag_over::<PlaylistTrackDrag>(move |style, drag, _, _| {
                    if drag.from_index > index {
                        style.border_t_2().border_color(gpui::rgb(PRIMARY))
                    } else if drag.from_index < index {
                        style.border_b_2().border_color(gpui::rgb(PRIMARY))
                    } else {
                        style.opacity(0.)
                    }
                }),
        )
    } else {
        row
    };

    track_menu(
        row,
        context_menu::queue_track_entity(queue_track),
        playback.clone(),
        downloads.clone(),
        account.clone(),
        view.clone(),
        open_album,
        open_artist,
        false,
        navigation_context.cloned(),
    )
    .into_any_element()
}

fn openers(view: &Entity<SearchView>, source: Provider) -> (NavigationOpener, NavigationOpener) {
    let album_view = view.clone();
    let open_album = Arc::new(
        move |target: NavigationTarget, _: &mut Window, cx: &mut App| {
            album_view.update(cx, |view, cx| view.open_card(target.card(source), cx));
        },
    );
    let artist_view = view.clone();
    let open_artist = Arc::new(
        move |target: NavigationTarget, _: &mut Window, cx: &mut App| {
            artist_view.update(cx, |view, cx| view.open_card(target.card(source), cx));
        },
    );
    (open_album, open_artist)
}

fn download_button(
    index: usize,
    track: &PlaybackTrack,
    downloads: &Entity<crate::downloads::DownloadModel>,
    account: &Entity<crate::settings::AccountState>,
) -> AnyElement {
    let button = row_download_button(SharedString::from(format!(
        "search-track-download-{index}-{}-{}",
        match track.provider {
            crate::playback::PlaybackProvider::Deezer => "Deezer",
            crate::playback::PlaybackProvider::SoundCloud => "SoundCloud",
        },
        track.id
    )));
    context_menu::track_download_button_above(
        button,
        track.clone(),
        downloads.clone(),
        account.clone(),
    )
    .into_any_element()
}

fn remove_button(
    index: usize,
    playlist: &str,
    track: &Track,
    pending: bool,
    view: &Entity<SearchView>,
) -> AnyElement {
    let playlist = playlist.to_owned();
    let track_id = track.id.clone();
    let view = view.clone();
    danger_row_action_button(
        SharedString::from(format!("search-track-remove-{index}-{track_id}")),
        LocalIcon::X,
        "Remove from playlist",
        pending,
        move |_, _, cx| {
            cx.stop_propagation();
            view.update(cx, |this, cx| {
                this.library.update(cx, |library, cx| {
                    library.remove_track_from_playlist(playlist.clone(), track_id.clone(), true, cx)
                })
            });
        },
    )
}

fn row_provider(show_provider_badge: bool, provider: Provider) -> Option<Provider> {
    show_provider_badge.then_some(provider)
}

fn playlist_row_reorder_enabled(preview: bool, eligible: bool) -> bool {
    !preview && eligible
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_badge_visibility_is_explicit_per_row_context() {
        assert_eq!(row_provider(true, Provider::Deezer), Some(Provider::Deezer));
        assert_eq!(row_provider(false, Provider::SoundCloud), None);
    }

    #[test]
    fn track_rows_keep_download_before_more_and_playlist_removal() {
        let source = include_str!("rows_view.rs");
        let download_helper = ["fn ", "download_button("].concat();
        let favorite_helper = ["fn ", "favorite_button("].concat();
        assert!(source.contains("track_menu_button("));
        assert!(source.contains("track_download_button_above("));
        assert!(source.contains("remove_button("));
        assert!(source.contains(&download_helper));
        assert!(!source.contains(&favorite_helper));
        assert!(source.contains(".child(download_button("));
        assert!(source.contains(".child(more_action)"));
    }

    #[test]
    fn preview_and_virtual_track_slots_share_the_same_stride() {
        assert_eq!(
            track_row_slot_height(true),
            crate::library::virtualization::row_height()
        );
        assert_eq!(
            f32::from(track_row_slot_height(false))
                + f32::from(crate::library::virtualization::row_gap()),
            f32::from(crate::library::virtualization::row_height())
        );
    }

    #[test]
    fn playlist_drag_is_only_enabled_for_full_detail_rows() {
        assert!(playlist_row_reorder_enabled(false, true));
        assert!(!playlist_row_reorder_enabled(true, true));
        assert!(!playlist_row_reorder_enabled(false, false));

        let source = include_str!("rows_view.rs");
        assert!(source.contains(".on_drag("));
        assert!(source.contains(".on_drop::<PlaylistTrackDrag>"));
        assert!(source.contains("this.move_playlist_track(drag.from_index, index, cx)"));
    }
}
