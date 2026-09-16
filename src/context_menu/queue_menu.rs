use gpui::{Entity, IntoElement};

use super::ContextMenuExt;

use crate::{
    assets::LocalIcon,
    downloads::DownloadModel,
    entity_navigation::{
        NavigationOpener, TrackMenuHost, album_target_for_track, artist_routes_for_track,
    },
    playback::{PlaybackModel, PlaybackTrack},
    settings::AccountState,
};

use super::{
    action_availability, items, queue_action_availability, queue_track_entity, track_info,
};

use super::links::playback_link;

pub(crate) fn queue_menu<E, N>(
    element: E,
    track: PlaybackTrack,
    index: usize,
    ordinal: usize,
    count: usize,
    playback: Entity<PlaybackModel>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    host: Entity<N>,
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
    explicit_blocked: bool,
) -> gpui::AnyElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: TrackMenuHost,
{
    element
        .context_menu(move |menu, window, cx| {
            let menu = super::style_entity_menu(menu);
            let entity = queue_track_entity(&track);
            let available = queue_action_availability(&track, ordinal, count, explicit_blocked);
            let link = playback_link(&track);
            let album_target = album_target_for_track(
                entity.provider,
                &track.album_id,
                &track.album,
                &track.artwork,
                &track.artist,
                &track.release_date,
                None,
            );
            let artist_routes = artist_routes_for_track(entity.provider, &track.artists);
            let blocked = playback.read(cx).state.explicit_blocked(&track);
            let entity_actions = action_availability(&entity, blocked);
            // Warm the playlist catalog while the menu is open so the Add to
            // playlist picker opens instantly. The guarded load is a no-op
            // when a fetch is already in flight or the catalog is ready.
            if entity_actions.add_to_playlist {
                host.update(cx, |host, cx| {
                    host.preload_playlist_catalog(entity.provider, cx)
                });
            }
            if entity_actions.favorite {
                let key = crate::library::FavoriteKey::for_provider(
                    entity.provider,
                    crate::library::FavoriteKind::Track,
                    entity.id.clone(),
                );
                if host
                    .read(cx)
                    .favorites_entity()
                    .read(cx)
                    .favorite(&key)
                    .is_none()
                {
                    host.update(cx, |host, cx| host.resolve_favorite_state(key, cx));
                }
            }
            let play = playback.clone();
            let next = playback.clone();
            let last = playback.clone();
            let move_up = playback.clone();
            let move_down = playback.clone();
            let remove = playback.clone();
            let menu = menu
                .item(track_info::track_info_item(&playback, &track, cx))
                .separator();
            let menu = menu
                .item(items::action_item(
                    "Play now",
                    Some(LocalIcon::Play),
                    !available.play_now,
                    move |_, _, cx| {
                        play.update(cx, |playback, cx| playback.select_from_queue(index, cx));
                    },
                ))
                .item(items::action_item(
                    "Play next in queue",
                    Some(LocalIcon::ListUl),
                    !available.play_next,
                    move |_, _, cx| {
                        next.update(cx, |playback, cx| playback.enqueue_next(index, cx));
                    },
                ))
                .item(items::action_item(
                    "Play last in queue",
                    Some(LocalIcon::ListUl),
                    !available.play_last,
                    move |_, _, cx| {
                        last.update(cx, |playback, cx| playback.enqueue_last(index, cx));
                    },
                ))
                .separator()
                .item(items::action_item(
                    "Move up",
                    Some(LocalIcon::ArrowUp),
                    !available.move_up,
                    move |_, _, cx| {
                        if available.move_up {
                            move_up.update(cx, |playback, cx| {
                                playback.reorder(ordinal, ordinal - 1, cx)
                            });
                        }
                    },
                ))
                .item(items::action_item(
                    "Move down",
                    Some(LocalIcon::ArrowDown),
                    !available.move_down,
                    move |_, _, cx| {
                        if available.move_down {
                            move_down.update(cx, |playback, cx| {
                                playback.reorder(ordinal, ordinal + 1, cx)
                            });
                        }
                    },
                ))
                .item(items::action_item(
                    "Remove from queue",
                    Some(LocalIcon::X),
                    false,
                    move |_, _, cx| {
                        remove.update(cx, |playback, cx| playback.remove_from_queue(index, cx));
                    },
                ))
                .separator();
            let menu = items::download_format_submenu(
                menu,
                window,
                cx,
                track.clone(),
                downloads.clone(),
                account.clone(),
            );
            let menu = items::local_library_item(menu, host.clone(), track.clone(), cx);
            let menu = items::add_to_playlist_submenu(
                menu,
                window,
                cx,
                host.clone(),
                track.clone(),
                entity_actions.add_to_playlist,
            );
            let menu = items::track_favorite_item(
                menu,
                host.clone(),
                entity.provider,
                entity.id.clone(),
                None,
                entity_actions.favorite,
            );
            let menu = items::lyrics_item(menu, playback.clone(), track.clone());
            if items::track_info_available(entity.provider, &entity.id) {
                let account = account.read(cx);
                let deezer_arl = account.deezer_arl();
                let soundcloud_token = account.soundcloud_mobile_token();
                host.update(cx, |host, cx| {
                    host.preload_track_info(
                        entity.provider,
                        entity.id.clone(),
                        deezer_arl,
                        soundcloud_token,
                        cx,
                    );
                });
            }
            let menu =
                items::track_info_menu_row(menu, host.clone(), account.clone(), &entity, &track)
                    .separator();
            let menu = items::route_items_with_artist_fallback(
                menu,
                album_target,
                &artist_routes,
                Some(&track.artist),
                open_album.clone(),
                open_artist.clone(),
            );
            let menu = items::station_item(
                menu,
                host.clone(),
                entity.provider,
                track.clone(),
                entity_actions.station,
            );
            let menu =
                items::deezer_feedback_items(menu, host.clone(), &entity, window, cx).separator();
            items::copy_items(menu, track.title.clone(), "Title", link)
        })
        .into_any_element()
}
