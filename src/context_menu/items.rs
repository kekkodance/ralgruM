use std::collections::HashSet;

use gpui::{
    App, ClickEvent, Entity, FontWeight, SharedString, Window, div, prelude::*, px, rgb, rgba,
};
use gpui_component::{Icon, menu::PopupMenuItem as NativePopupMenuItem};

use crate::{
    assets::LocalIcon,
    downloads::DownloadModel,
    entity_navigation::{MenuRoute, NavigationOpener, TrackMenuHost},
    library::{
        Card as LibraryCard, DeezerFeedbackKind, FavoriteKey, FavoriteKind, FavoriteState,
        LibraryView, TrackInfo,
    },
    playback::{
        DownloadVariant, PlaybackModel, PlaybackProvider, PlaybackTrack,
        deezer_collection_download_choices,
    },
    search::{Card, Provider, SearchView},
    settings::AccountState,
    theme::MUTED,
};

use super::{EntityKind, MenuEntity, PopupMenu, PopupMenuItem, copied_toast};

pub(crate) const ACTION_ROW_HEIGHT: f32 = 34.;
pub(crate) const ACTION_ROW_ICON_COLUMN: f32 = 20.;
pub(crate) const ACTION_ROW_GAP: f32 = 9.;
/// Shared icon and row builder for every available download format menu.
/// Keeping this at the context-menu layer prevents track and collection
/// menus from drifting apart as their data sources evolve independently.
pub(super) const DOWNLOAD_FORMAT_ICON: LocalIcon = LocalIcon::Music;
pub(super) const DOWNLOAD_VARIANT_RADIUS: f32 = 6.;
const ACTION_ROW_INNER_PADDING: f32 = 2.;
const ACTION_ROW_HORIZONTAL_PADDING: f32 = 8.;
const ACTION_ROW_VERTICAL_PADDING: f32 = 6.;
const SUBMENU_ARROW_WIDTH: f32 = 7.;
const NATIVE_MENU_OUTER_PADDING: f32 = 4.;
const NATIVE_MENU_ITEM_PADDING: f32 = 8.;
const NATIVE_MENU_ICON_SIZE: f32 = 12.;
const NATIVE_MENU_GAP: f32 = 4.;
const NATIVE_MENU_ICON_MARGIN: f32 = -16.;
const FAVORITE_PINK: u32 = 0xec4899;
const fn action_row_width(menu_width: f32) -> f32 {
    menu_width - 2. - 12.
}
const fn action_row_label_width(menu_width: f32) -> f32 {
    action_row_width(menu_width)
        - (ACTION_ROW_HORIZONTAL_PADDING * 2.)
        - ACTION_ROW_ICON_COLUMN
        - ACTION_ROW_GAP
}
const DEEZER_FEEDBACK_SUBMENU_LABEL: &str = "Not interested in";

pub(super) fn icon(local: LocalIcon) -> Icon {
    Icon::default().path(local.path())
}

pub(crate) fn action_row(
    label: SharedString,
    local_icon: Option<LocalIcon>,
    menu_width: f32,
) -> gpui::AnyElement {
    action_row_with_icon_color(label, local_icon, menu_width, None)
}

fn action_row_with_icon_color(
    label: SharedString,
    local_icon: Option<LocalIcon>,
    menu_width: f32,
    icon_color: Option<u32>,
) -> gpui::AnyElement {
    let row_width = action_row_width(menu_width);
    let label_width = action_row_label_width(menu_width);
    div()
        .relative()
        .w(px(row_width))
        .max_w_full()
        .min_w_0()
        .min_h(px(ACTION_ROW_HEIGHT))
        .px(px(ACTION_ROW_HORIZONTAL_PADDING))
        .py(px(ACTION_ROW_VERTICAL_PADDING))
        .flex()
        .items_center()
        .gap(px(ACTION_ROW_GAP))
        .child(
            div()
                .w(px(ACTION_ROW_ICON_COLUMN))
                .flex()
                .items_center()
                .justify_center()
                .children(local_icon.map(|local_icon| {
                    if let Some(color) = icon_color {
                        crate::assets::local_icon(local_icon, color)
                            .size(px(13.))
                            .into_any_element()
                    } else {
                        icon(local_icon).size(px(13.)).into_any_element()
                    }
                })),
        )
        .child(
            div()
                .flex_1()
                .w(px(label_width))
                .max_w_full()
                .min_w_0()
                .flex_shrink_1()
                .text_size(px(12.5))
                .font_weight(FontWeight(450.))
                .truncate()
                .overflow_hidden()
                .child(label),
        )
        .into_any_element()
}

pub(super) fn action_item<F>(
    label: impl Into<SharedString>,
    local_icon: Option<LocalIcon>,
    disabled: bool,
    on_click: F,
) -> PopupMenuItem
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let label = label.into();
    PopupMenuItem::element(move |_, _| {
        action_row(label.clone(), local_icon, super::ENTITY_MENU_WIDTH)
    })
    // Keep the compatibility icon setter available to callers. The app-owned
    // renderer does not reserve a component icon slot around this row.
    .icon(Icon::empty())
    .disabled(disabled)
    .on_click(on_click)
}

pub(super) fn playlist_info_item<F>(menu: PopupMenu, on_click: F) -> PopupMenu
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    menu.item(action_item(
        "Info",
        Some(LocalIcon::CircleInfo),
        false,
        on_click,
    ))
}

pub(crate) fn danger_action_row(
    label: SharedString,
    local_icon: Option<LocalIcon>,
    menu_width: f32,
) -> gpui::AnyElement {
    let palette = crate::app_button::DANGER_SECONDARY_PALETTE;
    let row_width = action_row_width(menu_width);
    let label_width = action_row_label_width(menu_width);
    div()
        .relative()
        .w(px(row_width))
        .max_w_full()
        .min_w_0()
        .min_h(px(ACTION_ROW_HEIGHT))
        .px(px(ACTION_ROW_HORIZONTAL_PADDING))
        .py(px(ACTION_ROW_VERTICAL_PADDING))
        .flex()
        .items_center()
        .gap(px(ACTION_ROW_GAP))
        .border_1()
        .border_color(rgba(0x00000000))
        .rounded(px(crate::app_button::DANGER_SECONDARY_RADIUS))
        .bg(rgba(palette.normal_background))
        .text_color(rgb(palette.normal_text))
        .hover(move |style| {
            style
                .border_color(rgba(palette.hover_border))
                .bg(rgba(palette.hover_background))
                .text_color(rgb(palette.hover_text))
        })
        .child(
            div()
                .w(px(ACTION_ROW_ICON_COLUMN))
                .flex()
                .items_center()
                .justify_center()
                .children(local_icon.map(|local_icon| {
                    crate::assets::local_icon(local_icon, palette.normal_text).size(px(13.))
                })),
        )
        .child(
            div()
                .flex_1()
                .w(px(label_width))
                .max_w_full()
                .min_w_0()
                .flex_shrink_1()
                .text_size(px(12.5))
                .font_weight(FontWeight(450.))
                .truncate()
                .overflow_hidden()
                .child(label),
        )
        .into_any_element()
}

pub(super) fn danger_action_item<F>(
    label: impl Into<SharedString>,
    local_icon: Option<LocalIcon>,
    disabled: bool,
    on_click: F,
) -> PopupMenuItem
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let label = label.into();
    PopupMenuItem::element(move |_, _| {
        danger_action_row(label.clone(), local_icon, super::ENTITY_MENU_WIDTH)
    })
    .icon(Icon::empty())
    .disabled(disabled)
    .on_click(on_click)
}

pub(super) fn remove_from_cache_item<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    provider: Provider,
    track_id: String,
) -> PopupMenu {
    let remove_host = host;
    menu.item(danger_action_item(
        "Remove from cache",
        Some(LocalIcon::TrashCan),
        false,
        move |_, _, cx| {
            remove_host.update(cx, |host, cx| {
                host.remove_from_cache(provider, track_id.clone(), cx);
            });
        },
    ))
}

pub(super) fn local_library_item<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    track: PlaybackTrack,
    cx: &App,
) -> PopupMenu {
    let saved = host.read(cx).local_track_saved(&track, cx);
    let action_host = host;
    if saved {
        menu.item(danger_action_item(
            "Remove from Local",
            Some(LocalIcon::TrashCan),
            false,
            move |_, _, cx| {
                action_host.update(cx, |host, cx| {
                    host.set_local_track_saved(track.clone(), false, cx)
                });
            },
        ))
    } else {
        menu.item(action_item(
            "Save to Local",
            Some(LocalIcon::FolderOpen),
            false,
            move |_, _, cx| {
                action_host.update(cx, |host, cx| {
                    host.set_local_track_saved(track.clone(), true, cx)
                });
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlaylistDestination {
    Local,
    Provider(Provider),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlaylistDestinationSpec {
    destination: PlaylistDestination,
    label: &'static str,
    enabled: bool,
}

fn playlist_destination_specs(
    playback_provider: PlaybackProvider,
    provider_enabled: bool,
) -> [PlaylistDestinationSpec; 2] {
    let provider = provider_for_playback(playback_provider);
    [
        PlaylistDestinationSpec {
            destination: PlaylistDestination::Local,
            label: "Local",
            enabled: true,
        },
        PlaylistDestinationSpec {
            destination: PlaylistDestination::Provider(provider),
            label: provider.label(),
            enabled: provider_enabled,
        },
    ]
}

fn provider_for_playback(playback_provider: PlaybackProvider) -> Provider {
    match playback_provider {
        PlaybackProvider::Deezer => Provider::Deezer,
        PlaybackProvider::SoundCloud => Provider::SoundCloud,
    }
}

fn provider_playlist_icon(provider: Provider) -> LocalIcon {
    match provider {
        Provider::Deezer => LocalIcon::Deezer,
        Provider::SoundCloud => LocalIcon::SoundCloud,
    }
}

pub(super) fn add_to_playlist_submenu<N: TrackMenuHost>(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
    host: Entity<N>,
    track: PlaybackTrack,
    provider_enabled: bool,
) -> PopupMenu {
    let [local, provider_spec] = playlist_destination_specs(track.provider, provider_enabled);
    let local_host = host.clone();
    let local_track = track.clone();
    let provider_host = host;
    let provider_track_id = track.id.clone();
    let origin_provider = match provider_spec.destination {
        PlaylistDestination::Provider(provider) => provider,
        PlaylistDestination::Local => unreachable!("the provider destination is not local"),
    };

    super::submenu::styled_submenu_with_icon(
        menu,
        LocalIcon::Plus,
        "Add to playlist",
        window,
        cx,
        move |menu, _, _| {
            let local_host = local_host.clone();
            let local_track = local_track.clone();
            let provider_host = provider_host.clone();
            let provider_track_id = provider_track_id.clone();
            let menu = menu.item(action_item(
                local.label,
                Some(LocalIcon::FolderOpen),
                !local.enabled,
                move |_, window, cx| {
                    let local_host = local_host.clone();
                    let local_track = local_track.clone();
                    window.defer(cx, move |window, cx| {
                        local_host.update(cx, |host, cx| {
                            host.open_local_playlist_picker(local_track, window, cx)
                        });
                    });
                },
            ));
            menu.item(action_item(
                provider_spec.label,
                Some(provider_playlist_icon(origin_provider)),
                !provider_spec.enabled,
                move |_, window, cx| {
                    if provider_spec.enabled {
                        let provider_host = provider_host.clone();
                        let provider_track_id = provider_track_id.clone();
                        window.defer(cx, move |window, cx| {
                            provider_host.update(cx, |host, cx| {
                                host.open_playlist_picker(
                                    provider_track_id,
                                    origin_provider,
                                    window,
                                    cx,
                                )
                            });
                        });
                    }
                },
            ))
        },
    )
}

pub(super) fn favorite_item<F>(
    menu: PopupMenu,
    favorites: Entity<FavoriteState>,
    key: FavoriteKey,
    fallback: Option<bool>,
    enabled: bool,
    on_toggle: F,
) -> PopupMenu
where
    F: Fn(bool, &mut Window, &mut App) + 'static,
{
    let render_favorites = favorites.clone();
    let render_key = key.clone();
    let disabled_favorites = favorites.clone();
    let disabled_key = key.clone();
    menu.item(
        PopupMenuItem::element(move |_, cx| {
            let favorites = render_favorites.read(cx);
            let known = favorites.favorite(&render_key).or(fallback);
            let label = if known == Some(true) {
                "Unfavorite"
            } else if known.is_none() && favorites.resolving(&render_key) {
                "Loading..."
            } else {
                "Favorite"
            };
            action_row_with_icon_color(
                label.into(),
                Some(LocalIcon::Heart),
                super::ENTITY_MENU_WIDTH,
                (known == Some(true)).then_some(FAVORITE_PINK),
            )
        })
        .icon(Icon::empty())
        .disabled(!enabled)
        .disabled_when(move |cx| {
            let favorites = disabled_favorites.read(cx);
            favorites.favorite(&disabled_key).or(fallback).is_none()
                && favorites.resolving(&disabled_key)
        })
        .on_click(move |_, window, cx| {
            if !enabled {
                return;
            }
            let known = favorites.read(cx).favorite(&key).or(fallback);
            if let Some(known) = known {
                on_toggle(known, window, cx);
            }
        }),
    )
}

/// Build a compact checked menu row while retaining the same spacing and
/// typography as the entity context menus. PopupMenu still owns focus,
/// keyboard navigation, checked state, and dismissal for this row.
pub(crate) fn compact_checked_action_item<F>(
    label: impl Into<SharedString>,
    checked: bool,
    on_click: F,
) -> NativePopupMenuItem
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let label = label.into();
    let check_icon = checked.then_some(LocalIcon::Check);
    NativePopupMenuItem::element(move |_, _| {
        native_compact_action_row(label.clone(), check_icon, super::COMPACT_MENU_WIDTH)
    })
    .icon(Icon::empty())
    .checked(checked)
    .on_click(on_click)
}

fn native_compact_action_row(
    label: SharedString,
    local_icon: Option<LocalIcon>,
    menu_width: f32,
) -> gpui::AnyElement {
    let row_width = menu_width
        - (NATIVE_MENU_OUTER_PADDING * 2.)
        - (NATIVE_MENU_ITEM_PADDING * 2.)
        - NATIVE_MENU_ICON_SIZE
        - NATIVE_MENU_GAP
        - NATIVE_MENU_ICON_MARGIN;
    div()
        .relative()
        .flex_1()
        .w(px(row_width))
        .max_w_full()
        .min_w_0()
        .min_h(px(ACTION_ROW_HEIGHT))
        .ml(px(NATIVE_MENU_ICON_MARGIN))
        .px(px(ACTION_ROW_INNER_PADDING))
        .flex()
        .items_center()
        .gap(px(ACTION_ROW_GAP))
        .child(
            div()
                .w(px(ACTION_ROW_ICON_COLUMN))
                .flex()
                .items_center()
                .justify_center()
                .children(local_icon.map(|local_icon| icon(local_icon).size(px(13.))))
                .into_any_element(),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.5))
                .font_weight(FontWeight(450.))
                .truncate()
                .overflow_hidden()
                .child(label),
        )
        .into_any_element()
}

pub(super) fn disabled_action(
    label: impl Into<SharedString>,
    local_icon: LocalIcon,
) -> PopupMenuItem {
    action_item(label, Some(local_icon), true, |_, _, _| {})
}

pub(super) fn submenu_action_row(
    label: SharedString,
    local_icon: Option<Icon>,
    menu_width: f32,
) -> gpui::AnyElement {
    let row_width = action_row_width(menu_width);
    let label_width = action_row_label_width(menu_width) - ACTION_ROW_GAP - SUBMENU_ARROW_WIDTH;
    div()
        .w(px(row_width))
        .max_w_full()
        .min_w_0()
        .min_h(px(ACTION_ROW_HEIGHT))
        .px(px(ACTION_ROW_HORIZONTAL_PADDING))
        .py(px(ACTION_ROW_VERTICAL_PADDING))
        .flex()
        .items_center()
        .gap(px(ACTION_ROW_GAP))
        .child(
            div()
                .w(px(ACTION_ROW_ICON_COLUMN))
                .flex()
                .items_center()
                .justify_center()
                .children(local_icon.map(|icon| icon.size(px(13.))))
                .into_any_element(),
        )
        .child(
            div()
                .flex_1()
                .w(px(label_width))
                .max_w_full()
                .min_w_0()
                .flex_shrink_1()
                .text_size(px(12.5))
                .font_weight(FontWeight(450.))
                .truncate()
                .overflow_hidden()
                .child(label),
        )
        .child(
            crate::assets::widget_icon(LocalIcon::ChevronRight)
                .size(px(SUBMENU_ARROW_WIDTH))
                .text_color(rgb(0xa1a1aa)),
        )
        .into_any_element()
}

fn link_copied(cx: &mut gpui::App) {
    copied_toast("Link", cx);
}

/// "Play next" and "Play last" for a single track, matching the queue items
/// of the original track menu.
pub(super) fn queue_position_items(
    menu: PopupMenu,
    track: PlaybackTrack,
    playback: Entity<PlaybackModel>,
    next_enabled: bool,
    last_enabled: bool,
) -> PopupMenu {
    let next_playback = playback.clone();
    let last_playback = playback;
    let next_track = track.clone();
    let last_track = track;
    menu.item(action_item(
        "Play next in queue",
        Some(LocalIcon::ListUl),
        !next_enabled,
        move |_, _, cx| {
            next_playback.update(cx, |playback, cx| {
                playback.enqueue_track(next_track.clone(), false, cx)
            });
        },
    ))
    .item(action_item(
        "Play last in queue",
        Some(LocalIcon::ListUl),
        !last_enabled,
        move |_, _, cx| {
            last_playback.update(cx, |playback, cx| {
                playback.enqueue_track(last_track.clone(), true, cx)
            });
        },
    ))
}

/// The per-variant "Download format" submenu for one track.
pub(super) fn download_format_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
    track: PlaybackTrack,
    downloads: Entity<DownloadModel>,
    account: Entity<AccountState>,
) -> PopupMenu {
    super::download_menu::download_format_submenu(menu, window, cx, track, downloads, account)
}

pub(super) fn track_favorite_item<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    provider: Provider,
    track_id: String,
    fallback: Option<bool>,
    enabled: bool,
) -> PopupMenu {
    let key = FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id.clone());
    let render_host = host.clone();
    let render_key = key.clone();
    let disabled_host = host.clone();
    let disabled_key = key.clone();
    menu.item(
        PopupMenuItem::element(move |_, cx| {
            let favorites = render_host.read(cx).favorites_entity();
            let favorites = favorites.read(cx);
            let known = favorites.favorite(&render_key).or(fallback);
            let label = if known == Some(true) {
                "Unfavorite"
            } else if known.is_none() && favorites.resolving(&render_key) {
                "Loading..."
            } else {
                "Favorite"
            };
            action_row_with_icon_color(
                label.into(),
                Some(LocalIcon::Heart),
                super::ENTITY_MENU_WIDTH,
                (known == Some(true)).then_some(FAVORITE_PINK),
            )
        })
        .icon(Icon::empty())
        .disabled(!enabled)
        .disabled_when(move |cx| {
            let favorites = disabled_host.read(cx).favorites_entity();
            let favorites = favorites.read(cx);
            favorites.favorite(&disabled_key).or(fallback).is_none()
                && favorites.resolving(&disabled_key)
        })
        .on_click(move |_, _, cx| {
            if !enabled {
                return;
            }
            let favorites = host.read(cx).favorites_entity();
            let known = favorites.read(cx).favorite(&key).or(fallback);
            if let Some(known) = known {
                host.update(cx, |host, cx| {
                    host.toggle_favorite_state(provider, track_id.clone(), known, cx)
                });
            }
        }),
    )
}

pub(super) fn station_item<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    provider: Provider,
    track: PlaybackTrack,
    enabled: bool,
) -> PopupMenu {
    menu.item(action_item(
        "Station",
        Some(LocalIcon::Radio),
        !enabled,
        move |_, _, cx| {
            if enabled {
                host.update(cx, |host, cx| {
                    if provider == Provider::Deezer {
                        host.start_deezer_track_mix(track.id.clone(), cx);
                    } else {
                        host.start_soundcloud_track_station(track.clone(), cx);
                    }
                });
            }
        },
    ))
}

#[derive(Clone)]
struct FeedbackTarget {
    kind: DeezerFeedbackKind,
    id: String,
    label: String,
    icon: LocalIcon,
}

fn deezer_feedback_targets(entity: &MenuEntity) -> Vec<FeedbackTarget> {
    if entity.kind != EntityKind::Track || entity.provider != Provider::Deezer {
        return Vec::new();
    }

    let mut targets = Vec::new();
    if let Some(id) = super::links::valid_id(&entity.id) {
        targets.push(FeedbackTarget {
            kind: DeezerFeedbackKind::Song,
            id: id.to_owned(),
            label: feedback_track_label(entity),
            icon: LocalIcon::Music,
        });
    }

    let mut seen_artist_ids = HashSet::new();
    for artist in &entity.artists {
        let Some(id) = super::links::valid_id(&artist.id) else {
            continue;
        };
        let name = artist.name.trim();
        if name.is_empty() || !seen_artist_ids.insert(id.to_owned()) {
            continue;
        }
        targets.push(FeedbackTarget {
            kind: DeezerFeedbackKind::Artist,
            id: id.to_owned(),
            label: name.to_owned(),
            icon: LocalIcon::User,
        });
    }
    targets
}

fn feedback_track_label(entity: &MenuEntity) -> String {
    let title = entity.title.trim();
    if title.is_empty() {
        "Track".into()
    } else {
        title.to_owned()
    }
}

pub(super) fn deezer_feedback_items<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    entity: &MenuEntity,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
) -> PopupMenu {
    if entity.provider != Provider::Deezer {
        return menu;
    }
    let targets = deezer_feedback_targets(entity);
    if targets.is_empty() {
        return menu.item(disabled_action(
            DEEZER_FEEDBACK_SUBMENU_LABEL,
            LocalIcon::CircleXmark,
        ));
    }

    let feedback_host = host;
    super::submenu::styled_submenu_with_icon(
        menu,
        LocalIcon::CircleXmark,
        DEEZER_FEEDBACK_SUBMENU_LABEL,
        window,
        cx,
        move |menu, _, _| {
            targets.clone().into_iter().fold(menu, |menu, target| {
                let FeedbackTarget {
                    kind,
                    id,
                    label,
                    icon,
                } = target;
                let target_host = feedback_host.clone();
                menu.item(action_item(label, Some(icon), false, move |_, _, cx| {
                    target_host.update(cx, |host, cx| {
                        host.add_negative_feedback(kind, id.clone(), cx);
                    });
                }))
            })
        },
    )
}

pub(super) fn artist_menu_items<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    entity: &MenuEntity,
    fallback: Option<bool>,
) -> PopupMenu {
    let valid_artist = super::links::valid_id(&entity.id).is_some();
    let favorite_enabled =
        valid_artist && matches!(entity.provider, Provider::Deezer | Provider::SoundCloud);
    let valid_deezer_artist = entity.provider == Provider::Deezer && valid_artist;
    let favorite_host = host.clone();
    let render_host = host.clone();
    let favorite_provider = entity.provider;
    let favorite_id = entity.id.clone();
    let favorite_key =
        FavoriteKey::for_provider(favorite_provider, FavoriteKind::Artist, favorite_id.clone());
    let render_key = favorite_key.clone();
    let disabled_host = host.clone();
    let disabled_key = favorite_key.clone();
    let menu = menu.item(
        PopupMenuItem::element(move |_, cx| {
            let favorites = render_host.read(cx).favorites_entity();
            let favorites = favorites.read(cx);
            let known = favorites.favorite(&render_key).or(fallback);
            let label = if known == Some(true) {
                "Unfavorite"
            } else if known.is_none() && favorites.resolving(&render_key) {
                "Loading..."
            } else {
                "Favorite"
            };
            action_row_with_icon_color(
                label.into(),
                Some(LocalIcon::Heart),
                super::ENTITY_MENU_WIDTH,
                (known == Some(true)).then_some(FAVORITE_PINK),
            )
        })
        .icon(Icon::empty())
        .disabled(!favorite_enabled)
        .disabled_when(move |cx| {
            let favorites = disabled_host.read(cx).favorites_entity();
            let favorites = favorites.read(cx);
            favorites.favorite(&disabled_key).or(fallback).is_none()
                && favorites.resolving(&disabled_key)
        })
        .on_click(move |_, _, cx| {
            if !favorite_enabled {
                return;
            }
            let favorites = favorite_host.read(cx).favorites_entity();
            let known = favorites.read(cx).favorite(&favorite_key).or(fallback);
            if let Some(known) = known {
                favorite_host.update(cx, |host, cx| {
                    host.toggle_artist_favorite(favorite_provider, favorite_id.clone(), known, cx);
                });
            }
        }),
    );
    if !valid_deezer_artist {
        if entity.provider == Provider::SoundCloud {
            let station_host = host.clone();
            let station_id = entity.id.clone();
            let menu = menu.separator().item(action_item(
                "Station",
                Some(LocalIcon::Radio),
                !valid_artist,
                move |_, _, cx| {
                    if valid_artist {
                        station_host.update(cx, |host, cx| {
                            host.start_soundcloud_artist_station(station_id.clone(), cx);
                        });
                    }
                },
            ));
            return copy_named_items(
                menu.separator(),
                entity.title.clone(),
                "Copy name",
                "Artist name",
                entity.service_link(),
            );
        }
        return copy_named_items(
            menu.separator(),
            entity.title.clone(),
            "Copy name",
            "Artist name",
            entity.service_link(),
        );
    }

    let similar_host = host.clone();
    let similar_id = entity.id.clone();
    let similar_title = entity.title.clone();
    let menu = menu.separator().item(action_item(
        "Similar artists",
        Some(LocalIcon::UserGroup),
        false,
        move |_, window, cx| {
            similar_host.update(cx, |host, cx| {
                host.open_similar_artists(similar_id.clone(), similar_title.clone(), window, cx);
            });
        },
    ));
    let mix_host = host.clone();
    let mix_id = entity.id.clone();
    let menu = menu.item(action_item(
        "Mix",
        Some(LocalIcon::Radio),
        false,
        move |_, _, cx| {
            mix_host.update(cx, |host, cx| {
                host.start_deezer_artist_mix(mix_id.clone(), cx);
            });
        },
    ));
    let feedback_host = host;
    let feedback_id = entity.id.clone();
    let menu = menu.item(action_item(
        "Not interested in this artist",
        Some(LocalIcon::CircleXmark),
        false,
        move |_, _, cx| {
            feedback_host.update(cx, |host, cx| {
                host.add_negative_feedback(DeezerFeedbackKind::Artist, feedback_id.clone(), cx);
            });
        },
    ));
    copy_named_items(
        menu.separator(),
        entity.title.clone(),
        "Copy name",
        "Artist name",
        entity.service_link(),
    )
}

/// Opens the lyrics panel for any track; playback is untouched when the track
/// is not the playing one.
pub(super) fn lyrics_item(
    menu: PopupMenu,
    playback: Entity<PlaybackModel>,
    track: PlaybackTrack,
) -> PopupMenu {
    menu.item(action_item(
        "Lyrics",
        Some(LocalIcon::QuoteRight),
        false,
        move |_, _, cx| {
            playback.update(cx, |playback, cx| playback.open_lyrics_for(&track, cx));
        },
    ))
}

pub(super) fn track_info_available(provider: Provider, id: &str) -> bool {
    matches!(provider, Provider::Deezer | Provider::SoundCloud)
        && super::links::valid_id(id).is_some()
}

/// Track Info row. Supported provider tracks reuse the same tag dialog;
/// anything else keeps the greyed placeholder in the same position.
pub(super) fn track_info_menu_row<N: TrackMenuHost>(
    menu: PopupMenu,
    host: Entity<N>,
    account: Entity<AccountState>,
    entity: &MenuEntity,
    track: &PlaybackTrack,
) -> PopupMenu {
    if !track_info_available(entity.provider, &entity.id) {
        return menu.item(disabled_action("Info", LocalIcon::CircleInfo));
    }
    let provider = entity.provider;
    let track_id = entity.id.clone();
    let seed = match provider {
        Provider::Deezer => TrackInfo::seed(
            track.title.clone(),
            track.artist.clone(),
            track.album.clone(),
            track.release_date.clone(),
            track.duration.as_secs(),
        ),
        Provider::SoundCloud => TrackInfo::soundcloud_seed(
            track.title.clone(),
            track.artist.clone(),
            track.duration.as_secs(),
        ),
    };
    menu.item(action_item(
        "Info",
        Some(LocalIcon::CircleInfo),
        false,
        move |_, window, cx| {
            let account = account.read(cx);
            let deezer_arl = account.deezer_arl();
            let soundcloud_token = account.soundcloud_mobile_token();
            host.update(cx, |host, cx| {
                host.open_track_info(
                    provider,
                    track_id.clone(),
                    seed.clone(),
                    deezer_arl,
                    soundcloud_token,
                    window,
                    cx,
                );
            });
        },
    ))
}

/// Album and artist navigation entries shared by track menus.
pub(super) fn route_items(
    menu: PopupMenu,
    album_target: Option<crate::entity_navigation::NavigationTarget>,
    artist_routes: &[MenuRoute],
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
) -> PopupMenu {
    let menu = if let Some(crate::entity_navigation::NavigationTarget::Album(album)) = album_target
    {
        let title = album.title.clone();
        let target = crate::entity_navigation::NavigationTarget::Album(album);
        menu.item(action_item(
            title,
            Some(LocalIcon::CompactDisc),
            false,
            move |_, window, cx| {
                open_album(target.clone(), window, cx);
            },
        ))
    } else {
        menu
    };
    let mut menu = menu;
    for route in artist_routes {
        let open_artist = open_artist.clone();
        let route = route.clone();
        menu = menu.item(action_item(
            route.title.clone(),
            Some(LocalIcon::User),
            false,
            move |_, window, cx| {
                if let Some(target) = route.navigation_target() {
                    open_artist(target, window, cx);
                }
            },
        ));
    }
    menu
}

/// Adds a disabled artist row when legacy playback metadata has only the
/// display string.  It keeps the current-track menu faithful to the track
/// label without inventing an invalid navigation target.
pub(super) fn route_items_with_artist_fallback(
    menu: PopupMenu,
    album_target: Option<crate::entity_navigation::NavigationTarget>,
    artist_routes: &[MenuRoute],
    fallback_artist: Option<&str>,
    open_album: NavigationOpener,
    open_artist: NavigationOpener,
) -> PopupMenu {
    let menu = route_items(menu, album_target, artist_routes, open_album, open_artist);
    if let Some(artist) = fallback_artist_label(artist_routes, fallback_artist) {
        menu.item(disabled_action(artist.to_owned(), LocalIcon::User))
    } else {
        menu
    }
}

fn fallback_artist_label<'a>(routes: &[MenuRoute], fallback: Option<&'a str>) -> Option<&'a str> {
    routes
        .is_empty()
        .then_some(fallback)
        .flatten()
        .map(str::trim)
        .filter(|artist| !artist.is_empty())
}

#[cfg(test)]
fn track_context_order(
    deezer_actions: bool,
    lyrics: bool,
    album: bool,
    artists: usize,
) -> Vec<&'static str> {
    let mut order = vec![
        "track-info",
        "separator",
        "play-next",
        "play-last",
        "separator",
    ];
    order.push("download");
    order.push(if deezer_actions {
        "add-to-playlist"
    } else {
        "add-to-playlist-disabled"
    });
    order.push(if deezer_actions {
        "favorite"
    } else {
        "favorite-disabled"
    });
    order.push(if lyrics { "lyrics" } else { "lyrics-disabled" });
    order.push(if deezer_actions {
        "info"
    } else {
        "info-disabled"
    });
    order.push("separator");
    if album {
        order.push("album");
    }
    order.extend((0..artists).map(|_| "artist"));
    order.push(if deezer_actions {
        "station"
    } else {
        "station-disabled"
    });
    order.push(if deezer_actions {
        "feedback"
    } else {
        "feedback-disabled"
    });
    order.push("separator");
    order.extend(["copy-title", "copy-link"]);
    order
}

#[cfg(test)]
fn artist_context_order(valid_deezer_artist: bool) -> Vec<&'static str> {
    if valid_deezer_artist {
        vec![
            "favorite",
            "separator",
            "similar-artists",
            "mix",
            "negative-artist",
            "separator",
            "copy-name",
            "copy-link",
        ]
    } else {
        vec!["favorite-disabled", "separator", "copy-name", "copy-link"]
    }
}

#[cfg(test)]
fn soundcloud_artist_context_order() -> Vec<&'static str> {
    vec![
        "favorite",
        "separator",
        "station",
        "separator",
        "copy-name",
        "copy-link",
    ]
}

pub(crate) fn lyrics_copy_menu(
    menu: super::PopupMenu,
    line: String,
    block: String,
    full: String,
    url: Option<String>,
) -> super::PopupMenu {
    let menu = super::style_entity_menu(menu).item(action_item(
        "Copy line",
        Some(LocalIcon::Copy),
        false,
        move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(line.clone()));
            copied_toast("Lyric line", cx);
        },
    ));
    let menu = if block.is_empty() {
        menu
    } else {
        menu.item(action_item(
            "Copy text block",
            Some(LocalIcon::Copy),
            false,
            move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(block.clone()));
                copied_toast("Lyrics block", cx);
            },
        ))
    };
    menu.item(action_item(
        "Copy entire lyrics",
        Some(LocalIcon::List),
        full.is_empty(),
        move |_, _, cx| {
            if !full.is_empty() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(full.clone()));
                copied_toast("Lyrics", cx);
            }
        },
    ))
    .item(action_item(
        "Copy link",
        Some(LocalIcon::ShareNodes),
        url.is_none(),
        move |_, _, cx| {
            if let Some(link) = url.clone() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
                copied_toast("Link", cx);
            }
        },
    ))
}

/// "Copy title" plus "Copy canonical provider link". The link entry stays
/// visible but disabled when no provider URL exists, matching the original.
pub(super) fn copy_items(
    menu: PopupMenu,
    title: String,
    copy_label: &'static str,
    link: Option<String>,
) -> PopupMenu {
    copy_named_items(menu, title, "Copy title", copy_label, link)
}

pub(super) fn copy_named_items(
    menu: PopupMenu,
    title: String,
    title_label: &'static str,
    copied_label: &'static str,
    link: Option<String>,
) -> PopupMenu {
    menu.item(action_item(
        title_label,
        Some(LocalIcon::Copy),
        false,
        move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(title.clone()));
            copied_toast(copied_label, cx);
        },
    ))
    .item(action_item(
        "Copy link",
        Some(LocalIcon::ShareNodes),
        link.is_none(),
        move |_, _, cx| {
            if let Some(link) = link.clone() {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
                link_copied(cx);
            }
        },
    ))
}

/// Queue actions for album and playlist cards. Tracks are fetched on click
/// through the search view and appended without replacing the queue.
pub(super) fn collection_queue_items(
    menu: PopupMenu,
    card: Card,
    search: Entity<SearchView>,
    next_enabled: bool,
    last_enabled: bool,
) -> PopupMenu {
    let next_search = search.clone();
    let last_search = search;
    let next_card = card.clone();
    let last_card = card;
    menu.item(action_item(
        "Play next in queue",
        Some(LocalIcon::ListUl),
        !next_enabled,
        move |_, _, cx| {
            next_search.update(cx, |search, cx| {
                search.queue_collection(next_card.clone(), false, cx)
            });
        },
    ))
    .item(action_item(
        "Play last in queue",
        Some(LocalIcon::ListUl),
        !last_enabled,
        move |_, _, cx| {
            last_search.update(cx, |search, cx| {
                search.queue_collection(last_card.clone(), true, cx)
            });
        },
    ))
}

pub(super) fn library_collection_queue_items(
    menu: PopupMenu,
    card: LibraryCard,
    host: Entity<LibraryView>,
    next_enabled: bool,
    last_enabled: bool,
) -> PopupMenu {
    let next_host = host.clone();
    let last_host = host;
    let next_card = card.clone();
    let last_card = card;
    menu.item(action_item(
        "Play next in queue",
        Some(LocalIcon::ListUl),
        !next_enabled,
        move |_, _, cx| {
            if next_enabled {
                next_host.update(cx, |library, cx| {
                    library.queue_collection(next_card.clone(), false, cx)
                });
            }
        },
    ))
    .item(action_item(
        "Play last in queue",
        Some(LocalIcon::ListUl),
        !last_enabled,
        move |_, _, cx| {
            if last_enabled {
                last_host.update(cx, |library, cx| {
                    library.queue_collection(last_card.clone(), true, cx)
                });
            }
        },
    ))
}

pub(super) fn library_collection_download_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
    card: LibraryCard,
    host: Entity<LibraryView>,
    deezer_arl: bool,
    soundcloud_token: bool,
    murglar_token: bool,
) -> PopupMenu {
    super::submenu::styled_submenu_with_icon(
        menu,
        LocalIcon::Download,
        "Download",
        window,
        cx,
        move |menu, _, _| {
            collection_variants(card.source, deezer_arl, soundcloud_token, murglar_token)
                .into_iter()
                .fold(menu, |menu, choice| {
                    let host = host.clone();
                    let card = card.clone();
                    menu.item(
                        PopupMenuItem::element(move |_, _| {
                            download_variant_shell(
                                download_variant_row(choice.label, choice.detail),
                                false,
                            )
                        })
                        .icon(Icon::empty())
                        .on_click(move |_, _, cx| {
                            host.update(cx, |library, cx| {
                                library.download_collection(card.clone(), choice.variant, cx)
                            });
                        }),
                    )
                })
        },
    )
}

struct CollectionVariant {
    variant: DownloadVariant,
    label: &'static str,
    detail: &'static str,
}

fn collection_variants(
    provider: Provider,
    deezer_arl: bool,
    soundcloud_token: bool,
    murglar_token: bool,
) -> Vec<CollectionVariant> {
    match provider {
        Provider::Deezer => deezer_collection_download_choices(deezer_arl, murglar_token)
            .iter()
            .map(|choice| CollectionVariant {
                variant: choice.variant,
                label: choice.label,
                detail: choice.detail,
            })
            .collect(),
        Provider::SoundCloud => {
            let mut variants = vec![CollectionVariant {
                variant: DownloadVariant::Best,
                label: "Best available",
                detail: DownloadVariant::Best.download_detail(),
            }];
            if soundcloud_token {
                variants.push(CollectionVariant {
                    variant: DownloadVariant::Original,
                    label: "Original file",
                    detail: soundcloud_download_detail(DownloadVariant::Original),
                });
            }
            if murglar_token {
                variants.push(CollectionVariant {
                    variant: DownloadVariant::Murglar,
                    label: "Lossless / high quality",
                    detail: soundcloud_download_detail(DownloadVariant::Murglar),
                });
            }
            variants.push(CollectionVariant {
                variant: DownloadVariant::Standard,
                label: "MP3 128 kbps",
                detail: soundcloud_download_detail(DownloadVariant::Standard),
            });
            variants
        }
    }
}

/// Download submenu for album and playlist cards. Variant entries mirror
/// downloadVariantsForContext in the original app; each entry fetches the
/// collection tracks on click and batch downloads them in that format.
pub(super) fn collection_download_submenu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
    card: Card,
    search: Entity<SearchView>,
    account: Entity<AccountState>,
) -> PopupMenu {
    let submenu_account = account;
    let submenu_search = search;
    let submenu_card = card;
    super::submenu::styled_submenu_with_icon(
        menu,
        LocalIcon::Download,
        "Download",
        window,
        cx,
        move |menu, _, cx| {
            let (deezer_arl, soundcloud_token, murglar_token) = {
                let account = submenu_account.read(cx);
                (
                    account.deezer_arl().is_some(),
                    account.soundcloud_token().is_some(),
                    account.murglar_media_credentials().is_some(),
                )
            };
            let variants = collection_variants(
                submenu_card.source,
                deezer_arl,
                soundcloud_token,
                murglar_token,
            );
            variants.into_iter().fold(menu, |menu, entry| {
                let search = submenu_search.clone();
                let card = submenu_card.clone();
                menu.item(
                    PopupMenuItem::element(move |_, _| {
                        download_variant_shell(
                            download_variant_row(entry.label, entry.detail),
                            false,
                        )
                    })
                    .icon(Icon::empty())
                    .on_click(move |_, _, cx| {
                        search.update(cx, |search, cx| {
                            search.download_collection(card.clone(), entry.variant, cx)
                        });
                    }),
                )
            })
        },
    )
}

pub(super) fn variant_row(
    local_icon: LocalIcon,
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> gpui::AnyElement {
    let label = label.into();
    let detail = detail.into();
    div()
        .w_full()
        .max_w_full()
        .min_w_0()
        .min_h(px(ACTION_ROW_HEIGHT))
        .px(px(ACTION_ROW_HORIZONTAL_PADDING))
        .py(px(ACTION_ROW_VERTICAL_PADDING))
        .flex()
        .items_center()
        .gap(px(ACTION_ROW_GAP))
        .overflow_hidden()
        .child(
            div()
                .w(px(ACTION_ROW_ICON_COLUMN))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(local_icon).size(px(13.))),
        )
        .child(
            div()
                .flex_1()
                .w_full()
                .max_w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(
                    div()
                        .w_full()
                        .max_w_full()
                        .text_size(px(12.5))
                        .font_weight(FontWeight(450.))
                        .truncate()
                        .overflow_hidden()
                        .child(label),
                )
                .when(!detail.is_empty(), |this| {
                    this.child(
                        div()
                            .w_full()
                            .max_w_full()
                            .text_size(px(10.5))
                            .text_color(rgb(MUTED))
                            .truncate()
                            .overflow_hidden()
                            .child(detail),
                    )
                }),
        )
        .into_any_element()
}

pub(super) fn download_variant_row(
    label: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> gpui::AnyElement {
    variant_row(DOWNLOAD_FORMAT_ICON, label, detail)
}

pub(super) fn download_variant_shell(row: gpui::AnyElement, highlighted: bool) -> gpui::Div {
    div()
        .w_full()
        .cursor_pointer()
        .when(highlighted, |this| {
            this.rounded(px(DOWNLOAD_VARIANT_RADIUS))
        })
        .child(row)
}

pub(super) const fn soundcloud_download_detail(variant: DownloadVariant) -> &'static str {
    variant.download_detail()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_destinations_keep_local_first_and_origin_provider_only() {
        let deezer = playlist_destination_specs(PlaybackProvider::Deezer, true);
        assert_eq!(
            deezer
                .iter()
                .map(|entry| entry.destination)
                .collect::<Vec<_>>(),
            vec![
                PlaylistDestination::Local,
                PlaylistDestination::Provider(Provider::Deezer),
            ]
        );
        assert_eq!(deezer[0].label, "Local");
        assert_eq!(deezer[1].label, "Deezer");
        assert!(!deezer.iter().any(|entry| {
            entry.destination == PlaylistDestination::Provider(Provider::SoundCloud)
        }));

        let soundcloud = playlist_destination_specs(PlaybackProvider::SoundCloud, true);
        assert_eq!(
            soundcloud
                .iter()
                .map(|entry| entry.destination)
                .collect::<Vec<_>>(),
            vec![
                PlaylistDestination::Local,
                PlaylistDestination::Provider(Provider::SoundCloud),
            ]
        );
        assert_eq!(soundcloud[0].label, "Local");
        assert_eq!(soundcloud[1].label, "SoundCloud");
        assert!(
            !soundcloud.iter().any(|entry| {
                entry.destination == PlaylistDestination::Provider(Provider::Deezer)
            })
        );
    }

    #[test]
    fn playlist_destinations_disable_only_the_provider_child() {
        let entries = playlist_destination_specs(PlaybackProvider::Deezer, false);
        assert!(entries[0].enabled);
        assert!(!entries[1].enabled);
        assert_eq!(entries[0].destination, PlaylistDestination::Local);
        assert_eq!(
            entries[1].destination,
            PlaylistDestination::Provider(Provider::Deezer)
        );
    }

    #[test]
    fn track_context_order_matches_original_sections() {
        assert_eq!(
            track_context_order(true, true, true, 2),
            vec![
                "track-info",
                "separator",
                "play-next",
                "play-last",
                "separator",
                "download",
                "add-to-playlist",
                "favorite",
                "lyrics",
                "info",
                "separator",
                "album",
                "artist",
                "artist",
                "station",
                "feedback",
                "separator",
                "copy-title",
                "copy-link",
            ]
        );
    }

    #[test]
    fn track_context_order_keeps_disabled_placeholders_present() {
        let order = track_context_order(false, false, false, 0);
        assert!(order.contains(&"add-to-playlist-disabled"));
        assert!(order.contains(&"favorite-disabled"));
        assert!(order.contains(&"lyrics-disabled"));
        assert!(order.contains(&"info-disabled"));
        assert!(order.contains(&"station-disabled"));
        assert!(order.contains(&"feedback-disabled"));
    }

    #[test]
    fn track_info_is_available_for_numeric_ids_from_both_providers() {
        assert!(track_info_available(Provider::Deezer, "42"));
        assert!(track_info_available(Provider::SoundCloud, "42"));
        assert!(!track_info_available(Provider::SoundCloud, "track"));
    }

    #[test]
    fn fallback_artist_label_is_only_used_without_routeable_artists() {
        assert_eq!(
            fallback_artist_label(&[], Some("  Legacy Artist  ")),
            Some("Legacy Artist")
        );
        assert_eq!(fallback_artist_label(&[], Some("   ")), None);
        let route = MenuRoute {
            kind: crate::entity_navigation::MenuRouteKind::Artist,
            id: "7".into(),
            title: "Routeable Artist".into(),
        };
        assert_eq!(fallback_artist_label(&[route], Some("Legacy Artist")), None);
    }

    #[test]
    fn feedback_submenu_keeps_track_first_and_deduplicates_artist_ids() {
        let entity = MenuEntity {
            kind: EntityKind::Track,
            provider: Provider::Deezer,
            id: " 100 ".into(),
            title: "Track".into(),
            track: None,
            has_tracks: true,
            album_id: String::new(),
            artists: vec![
                crate::search::TrackArtistRef {
                    id: "11".into(),
                    name: "First Artist".into(),
                },
                crate::search::TrackArtistRef {
                    id: " 11 ".into(),
                    name: "Duplicate Artist".into(),
                },
                crate::search::TrackArtistRef {
                    id: "bad/id".into(),
                    name: "Invalid Artist".into(),
                },
                crate::search::TrackArtistRef {
                    id: "22".into(),
                    name: "  Second Artist  ".into(),
                },
                crate::search::TrackArtistRef {
                    id: "33".into(),
                    name: "   ".into(),
                },
            ],
            service_url: String::new(),
        };

        let targets = deezer_feedback_targets(&entity);
        let summary: Vec<_> = targets
            .iter()
            .map(|target| (target.kind, target.id.as_str(), target.label.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (DeezerFeedbackKind::Song, "100", "Track"),
                (DeezerFeedbackKind::Artist, "11", "First Artist"),
                (DeezerFeedbackKind::Artist, "22", "Second Artist"),
            ]
        );
    }

    #[test]
    fn feedback_track_uses_trimmed_title_and_falls_back_when_empty() {
        let mut entity = MenuEntity {
            kind: EntityKind::Track,
            provider: Provider::Deezer,
            id: "100".into(),
            title: "  A very long title that remains truncated by the menu row  ".into(),
            track: None,
            has_tracks: true,
            album_id: String::new(),
            artists: Vec::new(),
            service_url: String::new(),
        };
        assert_eq!(
            feedback_track_label(&entity),
            "A very long title that remains truncated by the menu row"
        );
        entity.title = "   ".into();
        assert_eq!(feedback_track_label(&entity), "Track");
    }

    #[test]
    fn feedback_submenu_has_no_actions_for_non_deezer_or_invalid_tracks() {
        let mut entity = MenuEntity {
            kind: EntityKind::Track,
            provider: Provider::SoundCloud,
            id: "100".into(),
            title: "Track".into(),
            track: None,
            has_tracks: true,
            album_id: String::new(),
            artists: vec![crate::search::TrackArtistRef {
                id: "11".into(),
                name: "Artist".into(),
            }],
            service_url: String::new(),
        };
        assert!(deezer_feedback_targets(&entity).is_empty());

        entity.provider = Provider::Deezer;
        entity.id = "not-a-number".into();
        assert_eq!(deezer_feedback_targets(&entity).len(), 1);
        entity.artists.clear();
        assert!(deezer_feedback_targets(&entity).is_empty());
    }

    #[test]
    fn deezer_artist_menu_order_matches_original_core() {
        assert_eq!(
            artist_context_order(true),
            vec![
                "favorite",
                "separator",
                "similar-artists",
                "mix",
                "negative-artist",
                "separator",
                "copy-name",
                "copy-link",
            ]
        );
    }

    #[test]
    fn non_deezer_artist_menu_has_no_deezer_actions() {
        let order = artist_context_order(false);
        assert!(!order.contains(&"similar-artists"));
        assert!(!order.contains(&"mix"));
        assert!(!order.contains(&"negative-artist"));
    }

    #[test]
    fn soundcloud_artist_menu_keeps_station_between_favorite_and_copy() {
        assert_eq!(
            soundcloud_artist_context_order(),
            vec![
                "favorite",
                "separator",
                "station",
                "separator",
                "copy-name",
                "copy-link",
            ]
        );
    }

    #[test]
    fn collection_variants_mirror_the_original_submenu() {
        let deezer = collection_variants(Provider::Deezer, true, false, true);
        assert_eq!(
            deezer.iter().map(|entry| entry.variant).collect::<Vec<_>>(),
            vec![
                DownloadVariant::DeezerFlac,
                DownloadVariant::DeezerMp3_320,
                DownloadVariant::DeezerMp3_128,
            ]
        );
        assert_eq!(
            deezer.iter().map(|entry| entry.label).collect::<Vec<_>>(),
            vec!["FLAC", "MP3 320 kbps", "MP3 128 kbps"]
        );

        let arl_only = collection_variants(Provider::Deezer, true, false, false);
        assert_eq!(
            arl_only
                .iter()
                .map(|entry| entry.variant)
                .collect::<Vec<_>>(),
            vec![DownloadVariant::DeezerMp3_128]
        );

        let murglar_only = collection_variants(Provider::Deezer, false, false, true);
        assert_eq!(
            murglar_only
                .iter()
                .map(|entry| entry.variant)
                .collect::<Vec<_>>(),
            vec![DownloadVariant::DeezerFlac, DownloadVariant::DeezerMp3_320]
        );
        assert!(collection_variants(Provider::Deezer, false, false, false).is_empty());

        let soundcloud = collection_variants(Provider::SoundCloud, false, true, true);
        let labels: Vec<_> = soundcloud.iter().map(|entry| entry.label).collect();
        assert_eq!(
            labels,
            vec![
                "Best available",
                "Original file",
                "Lossless / high quality",
                "MP3 128 kbps"
            ]
        );

        let unauthenticated = collection_variants(Provider::SoundCloud, false, false, false);
        assert_eq!(unauthenticated.len(), 2);
    }

    #[test]
    fn every_soundcloud_collection_row_uses_the_shared_format_detail() {
        let variants = collection_variants(Provider::SoundCloud, false, true, true);
        assert!(variants.iter().all(|entry| {
            !entry.detail.is_empty() && entry.detail == soundcloud_download_detail(entry.variant)
        }));
        assert_eq!(DOWNLOAD_FORMAT_ICON.path(), LocalIcon::Music.path());
    }

    #[test]
    fn library_deezer_collection_path_uses_the_shared_credential_policy() {
        let choices = deezer_collection_download_choices(true, false);
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.variant)
                .collect::<Vec<_>>(),
            vec![DownloadVariant::DeezerMp3_128]
        );
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.detail)
                .collect::<Vec<_>>(),
            vec!["Standard quality"]
        );
    }
}
