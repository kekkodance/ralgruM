use std::sync::Arc;

use crate::{
    app_tooltip::AppTooltipExt,
    assets::{LocalIcon, local_icon},
    context_menu,
    drag_cursor::{DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned},
    entity_navigation::{NavigationOpener, NavigationTarget},
    music_ui::{TRACK_TITLE_ARTIST_GAP_PX, danger_remove_button},
    search::{Provider, SearchView},
    theme::{BORDER, DEEZER, MUTED, PRIMARY, SOUNDCLOUD, SURFACE_RAISED},
};
use gpui::{
    AnyElement, Entity, FontWeight, IntoElement, KeyDownEvent, MouseButton, Window, div,
    prelude::*, px, rgb, rgba,
};

use super::{
    PlaybackModel, PlaybackProvider, PlaybackTrack, QueueDrag, QueueDragGhost, QueuePanel,
};

/// Queue rows follow the original's compact controls (app.js renderQueueSidebar):
/// a provider badge and one remove button. The reorder, play next, and play last
/// actions stay available in the row context menu. Clicks right after a drag are
/// suppressed natively: gpui drops the pending click once the drag threshold is
/// passed, so the row click cannot fire on drop.
#[allow(clippy::too_many_arguments)]
pub(super) fn row(
    playback: &Entity<PlaybackModel>,
    downloads: &Entity<crate::downloads::DownloadModel>,
    account: &Entity<crate::settings::AccountState>,
    host: Entity<QueuePanel>,
    search: gpui::WeakEntity<SearchView>,
    ordinal: usize,
    index: usize,
    count: usize,
    track: &PlaybackTrack,
    blocked: bool,
) -> impl IntoElement {
    let activate = playback.clone();
    let title = track.title.clone();
    let cursor_owner = DragCursorOwner::queue();
    let (open_album, open_artist) = queue_openers(search, track.provider);
    let container = div()
        .id(("queue-track", ordinal))
        .group(format!("queue-row-{index}"))
        .relative()
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(6.))
        .py(px(7.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgba(0x00000000))
        .when(!blocked, |this| {
            this.cursor_pointer()
                .hover(|style| style.bg(rgba(0xffffff0b)).border_color(rgb(BORDER)))
        })
        .aria_label(if blocked {
            format!("{title} (playback disabled, explicit content blocked)")
        } else {
            title.clone()
        })
        .when(blocked, |this| this.opacity(0.38));
    let container = if blocked {
        container
    } else {
        let drag_title = title.clone();
        let drop_playback = playback.clone();
        container
            .focusable()
            .tab_stop(true)
            .role(gpui::Role::Button)
            .focus_visible(|style| style.border_color(rgb(PRIMARY)))
            .on_drag_move::<QueueDrag>(move |_, window, cx| {
                set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
                cx.set_active_drag_cursor_style(grabbing_cursor(), window);
            })
            .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                if cx.has_active_drag() {
                    set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
                }
            })
            .on_drag(
                QueueDrag {
                    from_ordinal: ordinal,
                },
                move |_, _, window, cx| {
                    set_drag_cursor_owned(window, DragCursorState::Grabbing, cursor_owner);
                    cx.set_active_drag_cursor_style(grabbing_cursor(), window);
                    cx.new(|_| QueueDragGhost {
                        title: drag_title.clone(),
                    })
                },
            )
            .on_drop::<QueueDrag>(move |drag, window, cx| {
                if drag.from_ordinal != ordinal {
                    drop_playback.update(cx, |playback, cx| {
                        playback.reorder(drag.from_ordinal, ordinal, cx)
                    });
                }
                set_drag_cursor_owned(window, DragCursorState::Reset, cursor_owner);
            })
            .on_click({
                let activate = activate.clone();
                move |_, _, cx| {
                    activate.update(cx, |playback, cx| playback.select_from_queue(index, cx))
                }
            })
            .on_key_down({
                let activate = activate.clone();
                move |event: &KeyDownEvent, window, cx| {
                    if crate::tab_keyboard::is_activation_key(event.keystroke.key.as_str()) {
                        window.prevent_default();
                        activate.update(cx, |playback, cx| playback.select_from_queue(index, cx));
                    }
                }
            })
    };
    let artwork = track.artwork.clone();
    let has_artwork = !artwork.is_empty();
    let track_artist = track.artist.clone();
    let menu_track = track.clone();
    context_menu::queue_menu(
        container
            .child(
                div()
                    .relative()
                    .size(px(40.))
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE_RAISED))
                    .flex()
                    .items_center()
                    .justify_center()
                    // The music note is the stable placeholder the row shows
                    // while its artwork loads; the reveal element fades the
                    // image in over it.
                    .child(local_icon(LocalIcon::Music, MUTED).size(px(14.)))
                    .when(has_artwork, |this| {
                        this.child(
                            crate::artwork_reveal::artwork_reveal(
                                ("queue-artwork-reveal", ordinal),
                                artwork,
                            )
                            .size_full()
                            .rounded(px(5.)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(TRACK_TITLE_ARTIST_GAP_PX))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(track_artist),
                    ),
            )
            .child(row_meta(playback, index, track.provider))
            .when(!blocked, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(6.))
                        .right(px(12.))
                        .drag_over::<QueueDrag>(move |style, drag, _, _| {
                            if drag.from_ordinal > ordinal {
                                style.border_t_2().border_color(rgb(PRIMARY))
                            } else if drag.from_ordinal < ordinal {
                                style.border_b_2().border_color(rgb(PRIMARY))
                            } else {
                                style.opacity(0.)
                            }
                        }),
                )
            }),
        menu_track,
        index,
        ordinal,
        count,
        playback.clone(),
        downloads.clone(),
        account.clone(),
        host,
        open_album,
        open_artist,
        blocked,
    )
}

/// Provider badge plus the single remove control the original rows carried.
/// The remove button sits at 0.65 opacity and only fully appears while its row
/// is hovered or focused nearby, like the original's queue-remove-btn.
fn row_meta(
    playback: &Entity<PlaybackModel>,
    index: usize,
    provider: PlaybackProvider,
) -> AnyElement {
    let (badge, color, tooltip) = match provider {
        PlaybackProvider::Deezer => (LocalIcon::Deezer, DEEZER, "Deezer"),
        PlaybackProvider::SoundCloud => (LocalIcon::SoundCloud, SOUNDCLOUD, "SoundCloud"),
    };
    let remove = playback.clone();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(7.))
        .child(
            div()
                .id(format!("queue-provider-{index}"))
                .app_tooltip(tooltip)
                .child(local_icon(badge, color).size(px(11.))),
        )
        .child(danger_remove_button(
            format!("queue-remove-{index}").into(),
            "Remove from queue",
            Some(format!("queue-row-{index}").into()),
            move |_, _, cx| {
                cx.stop_propagation();
                remove.update(cx, |playback, cx| playback.remove_from_queue(index, cx));
            },
        ))
        .into_any_element()
}

/// Album and artist navigation from a queue row opens the search detail
/// pages, the same routes the original queue context menu navigated.
fn queue_openers(
    search: gpui::WeakEntity<SearchView>,
    provider: PlaybackProvider,
) -> (NavigationOpener, NavigationOpener) {
    let provider = match provider {
        PlaybackProvider::Deezer => Provider::Deezer,
        PlaybackProvider::SoundCloud => Provider::SoundCloud,
    };
    let album_search = search.clone();
    let open_album: NavigationOpener = Arc::new(
        move |target: NavigationTarget, _: &mut Window, cx: &mut gpui::App| {
            let Some(search) = album_search.upgrade() else {
                return;
            };
            search.update(cx, |view, cx| {
                view.open_card(target.card(provider), cx);
            });
        },
    );
    let open_artist: NavigationOpener = Arc::new(
        move |target: NavigationTarget, _: &mut Window, cx: &mut gpui::App| {
            let Some(search) = search.upgrade() else {
                return;
            };
            search.update(cx, |view, cx| {
                view.open_card(target.card(provider), cx);
            });
        },
    );
    (open_album, open_artist)
}

#[cfg(test)]
mod tests {
    use crate::music_ui::{DANGER_REMOVE_HIT_TARGET_PX, DANGER_REMOVE_ICON_SIZE_PX};

    #[test]
    fn remove_button_keeps_the_original_hit_target_and_smaller_glyph() {
        assert_eq!(DANGER_REMOVE_HIT_TARGET_PX, 26.);
        assert_eq!(DANGER_REMOVE_ICON_SIZE_PX, 10.);
        const { assert!(DANGER_REMOVE_ICON_SIZE_PX < 11.) };
    }
}
