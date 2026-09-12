use gpui::{Entity, IntoElement, MouseButton};

use super::ContextMenuExt;

use crate::{
    assets::LocalIcon,
    downloads::DownloadModel,
    entity_navigation::{
        ContextRoute, NavigationOpener, ProviderNavigationOpeners, TrackMenuHost,
        album_target_for_track, artist_routes_for_track,
    },
    playback::{PlaybackModel, PlaybackTrack},
    settings::AccountState,
};

use super::{MenuEntity, action_availability, items, track_info};

use super::links::playback_link;

pub(crate) fn track_menu<E, N>(
    element: E,
    entity: MenuEntity,
    playback: Entity<PlaybackModel>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    host: Entity<N>,
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
    favorite_root_active: bool,
    navigation_context: Option<ContextRoute>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: TrackMenuHost,
{
    track_menu_with_trigger(
        element,
        entity,
        playback,
        downloads,
        account,
        host,
        open_album,
        open_artist,
        favorite_root_active,
        navigation_context,
        MouseButton::Right,
    )
}

pub(crate) fn track_menu_button<E, N>(
    element: E,
    entity: MenuEntity,
    playback: Entity<PlaybackModel>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    host: Entity<N>,
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
    favorite_root_active: bool,
    navigation_context: Option<ContextRoute>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: TrackMenuHost,
{
    track_menu_with_trigger(
        element,
        entity,
        playback,
        downloads,
        account,
        host,
        open_album,
        open_artist,
        favorite_root_active,
        navigation_context,
        MouseButton::Left,
    )
}

/// Tracklist and player bar variant. The menu opens above the button with
/// its bottom edge against the button top edge and a chevron pointing down
/// to the button.
pub(crate) fn track_download_button_above<E>(
    element: E,
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    element
        .context_menu(move |menu, window, cx| {
            super::download_menu::download_format_menu_above(
                menu,
                window,
                cx,
                track.clone(),
                downloads.clone(),
                account.clone(),
            )
        })
        .open_on(MouseButton::Left)
        .place_above()
}

#[allow(clippy::too_many_arguments)]
fn track_menu_with_trigger<E, N>(
    element: E,
    entity: MenuEntity,
    playback: Entity<PlaybackModel>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    host: Entity<N>,
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
    favorite_root_active: bool,
    navigation_context: Option<ContextRoute>,
    trigger_button: MouseButton,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: TrackMenuHost,
{
    let menu = element
        .context_menu(move |menu, window, cx| {
            let menu = super::style_entity_menu(menu);
            let Some(track) = entity.track.clone() else {
                return menu;
            };
            let album_target = album_target_for_track(
                entity.provider,
                &entity.album_id,
                &track.album,
                &track.artwork,
                &track.artist,
                &track.release_date,
                navigation_context.as_ref(),
            );
            let artist_routes = artist_routes_for_track(entity.provider, &entity.artists);
            let blocked = playback.read(cx).state.explicit_blocked(&track);
            let available = action_availability(&entity, blocked);
            // Warm the playlist catalog while the menu is open so the Add to
            // playlist picker opens instantly. The guarded load is a no-op
            // when a fetch is already in flight or the catalog is ready.
            if available.add_to_playlist {
                host.update(cx, |host, cx| {
                    host.preload_playlist_catalog(entity.provider, cx)
                });
            }
            let link = entity.service_link();
            let favorite_fallback = favorite_root_active.then_some(true);
            if available.favorite {
                let key = crate::library::FavoriteKey::for_provider(
                    entity.provider,
                    crate::library::FavoriteKind::Track,
                    entity.id.clone(),
                );
                let favorite = host.read(cx).favorites_entity().read(cx).favorite(&key);
                if favorite.is_none() && favorite_fallback.is_none() {
                    host.update(cx, |host, cx| host.resolve_favorite_state(key, cx));
                }
            }
            let menu = menu
                .item(track_info::track_info_item(&playback, &track, cx))
                .separator();
            let queue_empty = playback.read(cx).state.queue.is_empty();
            let menu = items::queue_position_items(
                menu,
                track.clone(),
                playback.clone(),
                available.play_next,
                available.play_last && !queue_empty,
            )
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
            let menu = if entity.track.is_some()
                && host
                    .read(cx)
                    .can_remove_from_cache(entity.provider, &entity.id)
            {
                items::remove_from_cache_item(
                    menu.separator(),
                    host.clone(),
                    entity.provider,
                    entity.id.clone(),
                )
            } else {
                menu
            };
            let menu = items::add_to_playlist_submenu(
                menu,
                window,
                cx,
                host.clone(),
                track.clone(),
                available.add_to_playlist,
            );
            let menu = items::track_favorite_item(
                menu,
                host.clone(),
                entity.provider,
                entity.id.clone(),
                favorite_fallback,
                available.favorite,
            );
            let menu = if available.lyrics {
                items::lyrics_item(menu, playback.clone(), track.clone())
            } else {
                menu.item(items::disabled_action("Lyrics", LocalIcon::QuoteRight))
            };
            // Warm the tag cache while the menu is open so the Info
            // dialog opens populated. The guarded preload is a no-op when
            // a fetch is already in flight or the tags are cached.
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
                available.station,
            );
            let menu =
                items::deezer_feedback_items(menu, host.clone(), &entity, window, cx).separator();
            items::copy_items(menu, entity.title.clone(), "title", link)
        })
        .open_on(trigger_button);
    if matches!(trigger_button, MouseButton::Left) {
        menu.place_below()
    } else {
        menu
    }
}

pub(crate) fn current_menu<E, N>(
    element: E,
    track: PlaybackTrack,
    playback: Entity<PlaybackModel>,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
    host: Entity<N>,
    external_track_navigation: Option<ProviderNavigationOpeners>,
) -> gpui::AnyElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: TrackMenuHost,
{
    element
        .context_menu(move |menu, window, cx| {
            let menu = super::style_entity_menu(menu);
            let title = track.title.clone();
            let link = playback_link(&track);
            let entity = super::queue_track_entity(&track);
            let available = super::action_availability(&entity, false);
            // Warm the playlist catalog while the menu is open so the Add to
            // playlist picker opens instantly. The guarded load is a no-op
            // when a fetch is already in flight or the catalog is ready.
            if available.add_to_playlist {
                host.update(cx, |host, cx| {
                    host.preload_playlist_catalog(entity.provider, cx)
                });
            }
            if available.favorite {
                let key = crate::library::FavoriteKey::for_provider(
                    entity.provider,
                    crate::library::FavoriteKind::Track,
                    track.id.clone(),
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
            let menu = menu
                .item(track_info::track_info_item(&playback, &track, cx))
                .separator();
            let queue_empty = playback.read(cx).state.queue.is_empty();
            let menu = items::queue_position_items(
                menu,
                track.clone(),
                playback.clone(),
                true,
                !queue_empty,
            )
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
                available.add_to_playlist,
            );
            let menu = items::track_favorite_item(
                menu,
                host.clone(),
                entity.provider,
                track.id.clone(),
                None,
                available.favorite,
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
            let menu = if let Some(openers) = external_track_navigation.clone() {
                let provider = match track.provider {
                    crate::playback::PlaybackProvider::Deezer => crate::search::Provider::Deezer,
                    crate::playback::PlaybackProvider::SoundCloud => {
                        crate::search::Provider::SoundCloud
                    }
                };
                let (open_album, open_artist) = openers.for_provider(provider);
                let album_target = album_target_for_track(
                    provider,
                    &track.album_id,
                    &track.album,
                    &track.artwork,
                    &track.artist,
                    &track.release_date,
                    None,
                );
                let artist_routes = artist_routes_for_track(provider, &track.artists);
                items::route_items_with_artist_fallback(
                    menu,
                    album_target,
                    &artist_routes,
                    Some(&track.artist),
                    open_album,
                    open_artist,
                )
            } else {
                menu
            };
            let menu = menu.separator();
            let menu = items::station_item(
                menu,
                host.clone(),
                entity.provider,
                track.clone(),
                available.station,
            );
            let menu =
                items::deezer_feedback_items(menu, host.clone(), &entity, window, cx).separator();
            items::copy_items(menu, title, "title", link)
        })
        .into_any_element()
}
