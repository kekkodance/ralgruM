use gpui::{Entity, MouseButton};

use super::ContextMenuExt;

use crate::{
    assets::LocalIcon,
    entity_navigation::TrackMenuHost,
    library::{Card as LibraryCard, FavoriteKey, FavoriteKind, LibraryView},
    search::{Card, SearchView, collection_routable},
    settings::AccountState,
};

use super::{EntityKind, MenuEntity, action_availability, items};

pub(super) fn playlist_info_enabled(kind: &EntityKind) -> bool {
    matches!(kind, EntityKind::Playlist)
}

pub(super) fn album_info_enabled(kind: &EntityKind) -> bool {
    matches!(kind, EntityKind::Album)
}

pub(super) fn playlist_add_to_playlist_enabled(kind: &EntityKind) -> bool {
    !playlist_info_enabled(kind)
}

pub(super) fn album_add_to_playlist_enabled(
    kind: &EntityKind,
    provider: crate::search::Provider,
    id: &str,
    has_tracks: bool,
    deezer_arl: bool,
) -> bool {
    matches!(kind, EntityKind::Album)
        && provider == crate::search::Provider::Deezer
        && super::links::valid_id(id).is_some()
        && has_tracks
        && deezer_arl
}

fn collection_add_to_playlist_item<F>(
    menu: super::PopupMenu,
    enabled: bool,
    on_click: F,
) -> super::PopupMenu
where
    F: Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
{
    if enabled {
        menu.item(items::action_item(
            "Add to playlist",
            Some(LocalIcon::Plus),
            false,
            on_click,
        ))
    } else {
        menu.item(items::disabled_action("Add to playlist", LocalIcon::Plus))
    }
}

pub(crate) fn artist_card_menu<E, N>(
    element: E,
    entity: MenuEntity,
    host: Entity<N>,
    favorite_root_known: bool,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
    N: crate::entity_navigation::TrackMenuHost,
{
    element.context_menu(move |menu, _, cx| {
        let menu = super::style_entity_menu(menu);
        let key = crate::library::FavoriteKey::for_provider(
            entity.provider,
            crate::library::FavoriteKind::Artist,
            entity.id.clone(),
        );
        let shared = host.read(cx).favorites_entity().read(cx).favorite(&key);
        let fallback = favorite_root_known.then_some(true);
        if shared.is_none() && fallback.is_none() {
            host.update(cx, |host, cx| host.resolve_favorite_state(key, cx));
        }
        items::artist_menu_items(menu, host.clone(), &entity, fallback)
    })
}

pub(crate) fn library_card_menu<E>(
    element: E,
    entity: MenuEntity,
    card: LibraryCard,
    host: Entity<LibraryView>,
    favorite_root_known: bool,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    library_card_menu_with_trigger(
        element,
        entity,
        card,
        host,
        favorite_root_known,
        MouseButton::Right,
    )
}

pub(crate) fn library_card_menu_button<E>(
    element: E,
    entity: MenuEntity,
    card: LibraryCard,
    host: Entity<LibraryView>,
    favorite_root_known: bool,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    library_card_menu_with_trigger(
        element,
        entity,
        card,
        host,
        favorite_root_known,
        MouseButton::Left,
    )
}

fn library_card_menu_with_trigger<E>(
    element: E,
    entity: MenuEntity,
    card: LibraryCard,
    host: Entity<LibraryView>,
    favorite_root_known: bool,
    trigger_button: MouseButton,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    let menu = element
        .context_menu(move |menu, window, cx| {
            let menu = super::style_entity_menu(menu);
            if entity.kind == EntityKind::Artist {
                let key = FavoriteKey::for_provider(
                    entity.provider,
                    FavoriteKind::Artist,
                    entity.id.clone(),
                );
                let shared = host.read(cx).favorites_entity().read(cx).favorite(&key);
                let fallback = favorite_root_known.then_some(true);
                if shared.is_none() && fallback.is_none() {
                    host.update(cx, |host, cx| host.resolve_favorite_state(key, cx));
                }
                return items::artist_menu_items(menu, host.clone(), &entity, fallback);
            }
            if !matches!(entity.kind, EntityKind::Album | EntityKind::Playlist) {
                return menu;
            }
            // Start the about info fetch while the menu is open so the Info
            // dialog can open already populated. The dialog still fetches on its
            // own when prefetch has not completed.
            {
                let prefetch_host = host.clone();
                let prefetch_card = card.clone();
                let _ = prefetch_host.update(cx, |view, cx| {
                    view.prefetch_card_info(prefetch_card, cx);
                });
            }
            let enabled = super::links::valid_id(&entity.id).is_some();
            let (deezer_arl, soundcloud_token, murglar_token) = {
                let library = host.read(cx);
                (
                    library.deezer_arl_available(cx),
                    library.soundcloud_token_available(cx),
                    library.murglar_available(cx),
                )
            };
            let is_owned = entity.kind == EntityKind::Playlist
                && host
                    .read(cx)
                    .is_playlist_owned(entity.provider, &entity.id, &card.subtitle, cx);
            if entity.kind == EntityKind::Playlist {
                let ensure_host = host.clone();
                cx.defer(move |cx| {
                    ensure_host.update(cx, |host, cx| match entity.provider {
                        crate::search::Provider::Deezer => host.ensure_playlist_catalog(cx),
                        crate::search::Provider::SoundCloud => {
                            host.ensure_soundcloud_playlist_catalog(cx)
                        }
                    });
                });
            }
            let has_tracks = card.badge.parse::<usize>().map_or(true, |count| count > 0);
            let download_enabled = enabled
                && has_tracks
                && match entity.provider {
                    crate::search::Provider::Deezer => deezer_arl,
                    crate::search::Provider::SoundCloud => soundcloud_token,
                };
            let queue_empty = host.read(cx).playback.read(cx).state.queue.is_empty();
            let next_enabled = enabled && has_tracks;
            let last_enabled = next_enabled && !queue_empty;
            let menu = items::library_collection_queue_items(
                menu,
                card.clone(),
                host.clone(),
                next_enabled,
                last_enabled,
            );
            let menu = if download_enabled {
                items::library_collection_download_submenu(
                    menu.separator(),
                    window,
                    cx,
                    card.clone(),
                    host.clone(),
                    deezer_arl,
                    soundcloud_token,
                    murglar_token,
                )
            } else {
                menu.separator()
                    .item(items::disabled_action("Download", LocalIcon::Download))
            };
            let menu = if playlist_add_to_playlist_enabled(&entity.kind) {
                let add_enabled = album_add_to_playlist_enabled(
                    &entity.kind,
                    entity.provider,
                    &entity.id,
                    has_tracks,
                    deezer_arl,
                );
                let add_host = host.clone();
                let add_card = card.clone();
                collection_add_to_playlist_item(menu, add_enabled, move |_, _, cx| {
                    add_host.update(cx, |library, cx| {
                        library.add_collection_to_playlist(add_card.clone(), cx);
                    });
                })
            } else {
                menu
            };
            let (kind, label) = match entity.kind {
                EntityKind::Album => (FavoriteKind::Album, "Album Name"),
                EntityKind::Playlist => (FavoriteKind::Playlist, "Playlist Name"),
                _ => unreachable!(),
            };
            let favorite_enabled = enabled
                && !is_owned
                && matches!(
                    entity.provider,
                    crate::search::Provider::Deezer | crate::search::Provider::SoundCloud
                );
            let key = FavoriteKey::for_provider(entity.provider, kind, entity.id.clone());
            let shared = host.read(cx).favorites_entity().read(cx).favorite(&key);
            let fallback = favorite_root_known.then_some(true);
            if favorite_enabled && shared.is_none() && fallback.is_none() {
                host.update(cx, |library, cx| {
                    library.resolve_favorite_state(key.clone(), cx)
                });
            }
            let favorites = host.read(cx).favorites_entity();
            let favorite_host = host.clone();
            let menu = if !is_owned {
                items::favorite_item(
                    menu,
                    favorites,
                    key.clone(),
                    fallback,
                    favorite_enabled,
                    move |known, _, cx| {
                        favorite_host.update(cx, |library, cx| {
                            library.toggle_favorite(key.clone(), known, cx)
                        });
                    },
                )
            } else {
                menu
            };
            let menu = if is_owned {
                let edit_host = host.clone();
                let playlist_id = entity.id.clone();
                menu.item(items::action_item(
                    "Edit",
                    Some(LocalIcon::Pen),
                    false,
                    move |_, window, cx| {
                        edit_host.update(cx, |library, cx| {
                            library.open_playlist_editor(
                                entity.provider,
                                playlist_id.clone(),
                                window,
                                cx,
                            );
                        });
                    },
                ))
            } else {
                menu
            };
            let menu = if playlist_info_enabled(&entity.kind) || album_info_enabled(&entity.kind) {
                let info_host = host.clone();
                let info_card = card.clone();
                items::playlist_info_item(menu, move |_, window, cx| {
                    info_host.update(cx, |library, cx| {
                        library.open_card_info(info_card.clone(), window, cx);
                    });
                })
            } else {
                menu
            };
            let menu = items::copy_items(
                menu.separator(),
                entity.title.clone(),
                label,
                entity.service_link(),
            );
            if is_owned {
                let delete_host = host.clone();
                let playlist_id = entity.id.clone();
                menu.separator().item(items::danger_action_item(
                    "Delete playlist",
                    Some(LocalIcon::TrashCan),
                    false,
                    move |_, window, cx| {
                        delete_host.update(cx, |host, cx| {
                            host.open_playlist_delete(
                                entity.provider,
                                playlist_id.clone(),
                                window,
                                cx,
                            );
                        });
                    },
                ))
            } else {
                menu
            }
        })
        .open_on(trigger_button);
    if matches!(trigger_button, MouseButton::Left) {
        menu.place_below()
    } else {
        menu
    }
}

pub(crate) fn local_playlist_card_menu<E>(
    element: E,
    card: LibraryCard,
    host: Entity<LibraryView>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    local_playlist_card_menu_with_trigger(element, card, host, MouseButton::Right)
}

pub(crate) fn local_playlist_card_menu_button<E>(
    element: E,
    card: LibraryCard,
    host: Entity<LibraryView>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    local_playlist_card_menu_with_trigger(element, card, host, MouseButton::Left)
}

fn local_playlist_card_menu_with_trigger<E>(
    element: E,
    card: LibraryCard,
    host: Entity<LibraryView>,
    trigger_button: MouseButton,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    let menu = element
        .context_menu(move |menu, _, cx| {
            let menu = super::style_entity_menu(menu);
            let snapshot = host.read(cx).local_playlist_action_snapshot(&card.id);
            let has_tracks = snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.tracks.is_empty());
            let queue_empty = host.read(cx).playback.read(cx).state.queue.is_empty();
            let download_enabled = snapshot.as_ref().is_some_and(|snapshot| {
                host.read(cx)
                    .local_playlist_download_available(snapshot, cx)
            });
            let next_host = host.clone();
            let next_id = card.id.clone();
            let menu = menu.item(items::action_item(
                "Play next in queue",
                Some(LocalIcon::ListUl),
                !has_tracks,
                move |_, _, cx| {
                    next_host.update(cx, |library, cx| {
                        library.queue_local_playlist(next_id.clone(), false, cx)
                    });
                },
            ));
            let last_host = host.clone();
            let last_id = card.id.clone();
            let menu = menu.item(items::action_item(
                "Play last in queue",
                Some(LocalIcon::ListUl),
                !has_tracks || queue_empty,
                move |_, _, cx| {
                    last_host.update(cx, |library, cx| {
                        library.queue_local_playlist(last_id.clone(), true, cx)
                    });
                },
            ));
            let download_host = host.clone();
            let download_id = card.id.clone();
            let menu = menu.separator().item(items::action_item(
                "Download",
                Some(LocalIcon::Download),
                !download_enabled,
                move |_, _, cx| {
                    download_host.update(cx, |library, cx| {
                        library.download_local_playlist(download_id.clone(), cx)
                    });
                },
            ));
            let edit_host = host.clone();
            let edit_id = card.id.clone();
            let menu = menu.item(items::action_item(
                "Edit",
                Some(LocalIcon::Pen),
                false,
                move |_, window, cx| {
                    edit_host.update(cx, |library, cx| {
                        library.open_local_playlist_editor(edit_id.clone(), window, cx);
                    });
                },
            ));
            let info_host = host.clone();
            let info_id = card.id.clone();
            let menu = items::playlist_info_item(menu, move |_, window, cx| {
                info_host.update(cx, |library, cx| {
                    library.open_local_playlist_info(info_id.clone(), window, cx);
                });
            });
            let copy_title = card.title.clone();
            let menu = menu.separator().item(items::action_item(
                "Copy title",
                Some(LocalIcon::Copy),
                false,
                move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_title.clone()));
                    super::copied_toast("Playlist Name", cx);
                },
            ));
            let delete_host = host.clone();
            let delete_id = card.id.clone();
            let delete_title = card.title.clone();
            menu.separator().item(items::danger_action_item(
                "Delete playlist",
                Some(LocalIcon::TrashCan),
                false,
                move |_, window, cx| {
                    delete_host.update(cx, |library, cx| {
                        library.open_local_playlist_delete(
                            delete_id.clone(),
                            delete_title.clone(),
                            window,
                            cx,
                        );
                    });
                },
            ))
        })
        .open_on(trigger_button);
    if matches!(trigger_button, MouseButton::Left) {
        menu.place_below()
    } else {
        menu
    }
}

pub(crate) fn card_menu<E>(
    element: E,
    entity: MenuEntity,
    card: Card,
    search: Entity<SearchView>,
    account: Entity<AccountState>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    card_menu_with_trigger(element, entity, card, search, account, MouseButton::Right)
}

pub(crate) fn card_menu_button<E>(
    element: E,
    entity: MenuEntity,
    card: Card,
    search: Entity<SearchView>,
    account: Entity<AccountState>,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    card_menu_with_trigger(element, entity, card, search, account, MouseButton::Left)
}

fn card_menu_with_trigger<E>(
    element: E,
    entity: MenuEntity,
    card: Card,
    search: Entity<SearchView>,
    account: Entity<AccountState>,
    trigger_button: MouseButton,
) -> impl gpui::IntoElement
where
    E: gpui::InteractiveElement + gpui::ParentElement + gpui::Styled + gpui::IntoElement + 'static,
{
    let menu = element
        .context_menu(move |menu, window, cx| {
            let menu = super::style_entity_menu(menu);
            let available = action_availability(&entity, false);
            if entity.kind == EntityKind::Artist {
                let key = crate::library::FavoriteKey::for_provider(
                    entity.provider,
                    crate::library::FavoriteKind::Artist,
                    entity.id.clone(),
                );
                if search.read(cx).favorites.read(cx).favorite(&key).is_none() {
                    search.update(cx, |search, cx| search.resolve_favorite_state(key, cx));
                }
                return items::artist_menu_items(menu, search.clone(), &entity, None);
            }
            if matches!(entity.kind, EntityKind::Album | EntityKind::Playlist) {
                // Start the about info fetch while the menu is open so the
                // Info dialog can open already populated. The dialog still
                // fetches on its own when prefetch has not completed.
                let prefetch_search = search.clone();
                let prefetch_card = card.clone();
                let _ = prefetch_search.update(cx, |view, cx| {
                    view.prefetch_card_info(prefetch_card, cx);
                });
            }
            let title = entity.title.clone();
            let link = entity.service_link();
            let copy_label = match entity.kind {
                EntityKind::Album => "Album Name",
                EntityKind::Playlist => "Playlist Name",
                EntityKind::Artist => "Artist Name",
                EntityKind::Track => "title",
            };
            // Album and playlist cards queue and download through the search
            // view, which fetches the collection tracks on demand. The items stay
            // available whenever a provider route for the card exists.
            let is_owned = entity.kind == EntityKind::Playlist
                && search.read(cx).is_playlist_owned(
                    entity.provider,
                    &entity.id,
                    &card.subtitle,
                    cx,
                );
            let has_tracks = card.badge.parse::<usize>().map_or(true, |count| count > 0);
            let routable = collection_routable(&card);
            let queue_empty = search.read(cx).playback.read(cx).state.queue.is_empty();
            let next_enabled = routable && has_tracks;
            let last_enabled = next_enabled && !queue_empty;
            let menu = items::collection_queue_items(
                menu,
                card.clone(),
                search.clone(),
                next_enabled,
                last_enabled,
            );
            let menu = if routable && has_tracks {
                items::collection_download_submenu(
                    menu.separator(),
                    window,
                    cx,
                    card.clone(),
                    search.clone(),
                    account.clone(),
                )
            } else {
                menu.separator()
                    .item(items::disabled_action("Download", LocalIcon::Download))
            };
            let deezer_arl = account.read(cx).deezer_arl().is_some();
            let menu = if playlist_add_to_playlist_enabled(&entity.kind) {
                let add_enabled = album_add_to_playlist_enabled(
                    &entity.kind,
                    entity.provider,
                    &entity.id,
                    has_tracks,
                    deezer_arl,
                );
                let add_search = search.clone();
                let add_card = card.clone();
                collection_add_to_playlist_item(menu, add_enabled, move |_, _, cx| {
                    add_search.update(cx, |search, cx| {
                        search.add_collection_to_playlist(add_card.clone(), cx);
                    });
                })
            } else {
                menu
            };
            let menu = if available.favorite && !is_owned {
                let search = search.clone();
                let card = card.clone();
                let kind = match entity.kind {
                    EntityKind::Album => crate::library::FavoriteKind::Album,
                    EntityKind::Playlist => crate::library::FavoriteKind::Playlist,
                    EntityKind::Artist => crate::library::FavoriteKind::Artist,
                    EntityKind::Track => crate::library::FavoriteKind::Track,
                };
                let key = crate::library::FavoriteKey::for_provider(
                    entity.provider,
                    kind,
                    entity.id.clone(),
                );
                if search.read(cx).favorites.read(cx).favorite(&key).is_none() {
                    search.update(cx, |search, cx| {
                        search.resolve_favorite_state(key.clone(), cx)
                    });
                }
                let favorites = search.read(cx).favorites.clone();
                items::favorite_item(menu, favorites, key, None, true, move |known, _, cx| {
                    search.update(cx, |search, cx| {
                        search.toggle_collection_favorite(card.clone(), known, cx)
                    });
                })
            } else if is_owned {
                menu
            } else {
                menu.item(items::disabled_action("Favorite", LocalIcon::Heart))
            };
            let menu = if is_owned {
                let edit_search = search.clone();
                let playlist_id = entity.id.clone();
                menu.item(items::action_item(
                    "Edit",
                    Some(LocalIcon::Pen),
                    false,
                    move |_, window, cx| {
                        edit_search.update(cx, |search, cx| {
                            search.open_playlist_editor(
                                entity.provider,
                                playlist_id.clone(),
                                window,
                                cx,
                            );
                        });
                    },
                ))
            } else {
                menu
            };
            let menu = if playlist_info_enabled(&entity.kind) || album_info_enabled(&entity.kind) {
                let info_search = search.clone();
                let info_card = card.clone();
                items::playlist_info_item(menu, move |_, window, cx| {
                    info_search.update(cx, |search, cx| {
                        search.open_card_info(info_card.clone(), window, cx);
                    });
                })
            } else {
                menu
            };
            let menu = items::copy_items(menu.separator(), title, copy_label, link);
            if is_owned {
                let delete_search = search.clone();
                let playlist_id = entity.id.clone();
                menu.separator().item(items::danger_action_item(
                    "Delete playlist",
                    Some(LocalIcon::TrashCan),
                    false,
                    move |_, window, cx| {
                        delete_search.update(cx, |search, cx| {
                            search.open_playlist_delete(
                                entity.provider,
                                playlist_id.clone(),
                                window,
                                cx,
                            );
                        });
                    },
                ))
            } else {
                menu
            }
        })
        .open_on(trigger_button);
    if matches!(trigger_button, MouseButton::Left) {
        menu.place_below()
    } else {
        menu
    }
}
