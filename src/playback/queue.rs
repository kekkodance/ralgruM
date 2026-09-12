use std::time::Duration;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    drag_cursor::{DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned},
    library::{FavoriteKey, FavoriteKind, FavoriteState, LibraryView, flow_controls},
    search::SearchView,
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED, PRIMARY, SURFACE_RAISED},
};
use gpui::{
    Bounds, Context, DragMoveEvent, Entity, FontWeight, IntoElement, ListAlignment, ListState,
    MouseButton, Pixels, Render, Transformation, Window, div, list, point, prelude::*, px, rgb,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use super::{
    PlaybackContext, PlaybackModel,
    queue_list::{
        QueueListChange, QueueListIdentity, QueueRowSnapshot, clamp_drag_offset, list_change,
        should_extend, track_index_for_list_item,
    },
    queue_rows::row,
};

const EXTEND_SCROLL_THRESHOLD_PX: f32 = 180.;
const QUEUE_ITEM_HEIGHT_PX: f32 = 59.;
const QUEUE_LIST_OVERDRAW_PX: f32 = 240.;
const DRAG_EDGE_PX: f32 = 42.;
const DRAG_SCROLL_STEP_PX: f32 = 20.;
const DRAG_SCROLL_TICK: Duration = Duration::from_millis(50);
const QUEUE_HEADER_HEIGHT_PX: f32 = 101.;
const QUEUE_SUMMARY_HEIGHT_PX: f32 = 34.;
const QUEUE_SUMMARY_PADDING_PX: f32 = 10.;
const QUEUE_SUMMARY_GAP_PX: f32 = 12.;
const QUEUE_SUMMARY_INNER_GAP_PX: f32 = 7.;
const QUEUE_NARROW_CONTENT_INSET_PX: f32 = 16.;
const QUEUE_BOTTOM_PADDING_PX: f32 = 16.;

fn queue_title(context: &PlaybackContext) -> &'static str {
    match context {
        PlaybackContext::SoundCloudStation { .. } => "Station",
        PlaybackContext::DeezerTrackMix { .. } | PlaybackContext::DeezerArtistMix { .. } => "Mix",
        PlaybackContext::DeezerFlow { kind, .. } => match kind {
            crate::playback::DeezerFlowKind::Flow => "Flow",
            crate::playback::DeezerFlowKind::SmartMix => "Mix",
        },
        PlaybackContext::None
        | PlaybackContext::DeezerLibraryTracks { .. }
        | PlaybackContext::SoundCloudCollection { .. } => "Queue",
    }
}

fn queue_flow_mode(context: &PlaybackContext) -> Option<crate::library::FlowMode> {
    match context {
        PlaybackContext::DeezerFlow {
            mode,
            kind: crate::playback::DeezerFlowKind::Flow,
            ..
        } => Some(*mode),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) struct QueueDrag {
    pub(super) from_ordinal: usize,
}

pub(super) struct QueueDragGhost {
    pub(super) title: String,
}

impl Render for QueueDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(0.))
            .h(px(0.))
            .overflow_hidden()
            .opacity(0.)
            .child(self.title.clone())
    }
}

/// Pointer position of an active queue drag relative to the scroll viewport,
/// driving edge auto-scroll like the original's scheduleQueueAutoScroll.
#[derive(Clone, Copy)]
struct DragAutoScroll {
    pointer_y: Pixels,
    viewport: Bounds<Pixels>,
}

fn request_extension_if_needed(
    playback: &Entity<PlaybackModel>,
    library: &gpui::WeakEntity<LibraryView>,
    cx: &mut gpui::App,
) -> bool {
    let should_extend = {
        let playback = playback.read(cx);
        playback.context().is_infinite() && !playback.extension_in_flight()
    };
    if !should_extend {
        return false;
    }
    let Some(library) = library.upgrade() else {
        return false;
    };
    let playback = playback.clone();
    library.update(cx, |library, cx| {
        library.extend_playback_queue(playback.clone(), cx);
    });
    playback.read(cx).extension_in_flight()
}

fn queue_scrollbar_lane(list_state: &ListState, narrow: bool) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        // The list carries its own bottom padding inside the scrollable
        // content, so the lane stays full height. Padding scrolls with the
        // tail and the thumb still maps to the true max offset.
        .bottom_0()
        .left_0()
        .when(narrow, |this| this.right_0())
        .when(!narrow, |this| this.right(px(-24.)))
        .child(Scrollbar::vertical(list_state).scrollbar_show(ScrollbarShow::Hover))
}

pub(crate) struct QueuePanel {
    playback: Entity<PlaybackModel>,
    library: gpui::WeakEntity<LibraryView>,
    search: gpui::WeakEntity<SearchView>,
    downloads: Entity<crate::downloads::DownloadModel>,
    account: Entity<crate::settings::AccountState>,
    favorites: Entity<FavoriteState>,
    list_state: ListState,
    browser_scroll: BrowserScrollState,
    list_identity: QueueListIdentity,
    drag_pointer: Option<DragAutoScroll>,
    drag_autoscroll_running: bool,
}

impl QueuePanel {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        playback: Entity<PlaybackModel>,
        library: &Entity<LibraryView>,
        search: &Entity<SearchView>,
        downloads: Entity<crate::downloads::DownloadModel>,
        account: Entity<crate::settings::AccountState>,
        favorites: Entity<FavoriteState>,
    ) -> Self {
        Self {
            playback,
            library: library.downgrade(),
            search: search.downgrade(),
            downloads,
            account,
            favorites,
            list_state: ListState::new(0, ListAlignment::Top, px(QUEUE_LIST_OVERDRAW_PX))
                .with_uniform_item_height(px(QUEUE_ITEM_HEIGHT_PX)),
            browser_scroll: BrowserScrollState::new(),
            list_identity: QueueListIdentity::default(),
            drag_pointer: None,
            drag_autoscroll_running: false,
        }
    }

    fn request_extension(&self, cx: &mut Context<Self>) -> bool {
        request_extension_if_needed(&self.playback, &self.library, cx)
    }

    fn sync_list_state(&mut self, upcoming: &[usize], has_loading_row: bool) {
        let next_identity = QueueListIdentity::new(upcoming, has_loading_row);
        match list_change(&self.list_identity, &next_identity) {
            QueueListChange::Unchanged => {}
            QueueListChange::Splice {
                old_range,
                new_count,
            } => {
                self.list_state.splice(old_range, new_count);
                self.list_state
                    .clone()
                    .with_uniform_item_height(px(QUEUE_ITEM_HEIGHT_PX));
            }
            QueueListChange::Reset { item_count } => {
                self.list_state
                    .reset_with_uniform_height(item_count, px(QUEUE_ITEM_HEIGHT_PX));
            }
        }
        self.list_identity = next_identity;
    }

    fn install_scroll_handler(&self, track_count: usize, context_infinite: bool) {
        let playback = self.playback.clone();
        let library = self.library.clone();
        self.list_state.set_scroll_handler(move |event, _, cx| {
            if !should_extend(
                event.visible_range.clone(),
                track_count,
                QUEUE_ITEM_HEIGHT_PX,
                EXTEND_SCROLL_THRESHOLD_PX,
                context_infinite,
                false,
            ) {
                return;
            }
            request_extension_if_needed(&playback, &library, cx);
        });
    }

    /// Scrolls one step toward whichever viewport edge the dragged pointer
    /// hovers near. Returns true while a drag is still active.
    fn step_drag_autoscroll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.drag_pointer else {
            self.drag_autoscroll_running = false;
            return false;
        };
        if !cx.has_active_drag() {
            self.drag_pointer = None;
            self.drag_autoscroll_running = false;
            cx.notify();
            return false;
        }
        let delta = if drag.pointer_y < drag.viewport.top() + px(DRAG_EDGE_PX) {
            -DRAG_SCROLL_STEP_PX
        } else if drag.pointer_y > drag.viewport.bottom() - px(DRAG_EDGE_PX) {
            DRAG_SCROLL_STEP_PX
        } else {
            return true;
        };
        let current_offset = self.list_state.scroll_px_offset_for_scrollbar();
        let max_offset = self.list_state.max_offset_for_scrollbar();
        let target_y =
            clamp_drag_offset(f32::from(current_offset.y), f32::from(max_offset.y), delta);
        self.list_state
            .set_offset_from_scrollbar(point(current_offset.x, px(target_y)));
        cx.notify();
        true
    }

    /// Keeps scrolling while the pointer rests near an edge, not only while it
    /// moves. One loop serves each drag and exits when the drag ends.
    fn run_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.drag_autoscroll_running {
            return;
        }
        self.drag_autoscroll_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(DRAG_SCROLL_TICK).await;
                let alive = this
                    .update(cx, |panel, cx| panel.step_drag_autoscroll(cx))
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }
}

impl crate::entity_navigation::TrackMenuHost for QueuePanel {
    fn favorites_entity(&self) -> Entity<FavoriteState> {
        self.favorites.clone()
    }

    fn local_track_saved(&self, track: &super::PlaybackTrack, cx: &gpui::App) -> bool {
        self.library.upgrade().is_some_and(|library| {
            crate::entity_navigation::TrackMenuHost::local_track_saved(library.read(cx), track, cx)
        })
    }

    fn set_local_track_saved(
        &mut self,
        track: super::PlaybackTrack,
        saved: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                crate::entity_navigation::TrackMenuHost::set_local_track_saved(
                    library, track, saved, cx,
                )
            });
        }
    }

    fn resolve_favorite_state(&mut self, key: FavoriteKey, cx: &mut Context<Self>) {
        let Some(library) = self.library.upgrade() else {
            return;
        };
        library.update(cx, |library, cx| {
            library.resolve_favorite_state(key, cx);
        });
    }

    fn open_playlist_picker(
        &mut self,
        track_id: String,
        provider: crate::search::Provider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(library) = self.library.upgrade() else {
            return;
        };
        library.update(cx, |library, cx| {
            library.open_add_picker(vec![track_id], "queue".into(), provider, window, cx)
        });
    }

    fn open_local_playlist_picker(
        &mut self,
        track: super::PlaybackTrack,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.open_local_playlist_picker(track, window, cx);
            });
        }
    }

    fn preload_playlist_catalog(
        &mut self,
        provider: crate::search::Provider,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| match provider {
                crate::search::Provider::Deezer => library.ensure_playlist_catalog(cx),
                crate::search::Provider::SoundCloud => {
                    library.ensure_soundcloud_playlist_catalog(cx)
                }
            });
        }
    }

    fn toggle_favorite_state(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        known_favorite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(library) = self.library.upgrade() else {
            return;
        };
        library.update(cx, |library, cx| {
            library.toggle_favorite(
                FavoriteKey::for_provider(provider, FavoriteKind::Track, track_id),
                known_favorite,
                cx,
            )
        });
    }

    fn start_deezer_track_mix(&mut self, track_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_deezer_track_mix(track_id, cx);
            });
        }
    }

    fn start_deezer_artist_mix(&mut self, artist_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_deezer_artist_mix(artist_id, cx);
            });
        }
    }

    fn start_soundcloud_artist_station(&mut self, artist_id: String, cx: &mut Context<Self>) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_soundcloud_artist_station(artist_id, cx);
            });
        }
    }

    fn start_soundcloud_track_station(
        &mut self,
        track: super::PlaybackTrack,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.start_soundcloud_track_station(track, cx);
            });
        }
    }

    fn add_negative_feedback(
        &mut self,
        kind: crate::library::DeezerFeedbackKind,
        id: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                library.add_negative_feedback(kind, id, cx);
            });
        }
    }

    fn open_track_info(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        seed: crate::library::TrackInfo,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                crate::entity_navigation::TrackMenuHost::open_track_info(
                    library,
                    provider,
                    track_id,
                    seed,
                    deezer_arl,
                    soundcloud_token,
                    window,
                    cx,
                );
            });
        }
    }

    fn preload_track_info(
        &mut self,
        provider: crate::search::Provider,
        track_id: String,
        deezer_arl: Option<crate::search::DeezerArl>,
        soundcloud_token: Option<crate::search::SoundCloudToken>,
        cx: &mut Context<Self>,
    ) {
        if let Some(library) = self.library.upgrade() {
            library.update(cx, |library, cx| {
                crate::entity_navigation::TrackMenuHost::preload_track_info(
                    library,
                    provider,
                    track_id,
                    deezer_arl,
                    soundcloud_token,
                    cx,
                );
            });
        }
    }
}

impl Render for QueuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let upcoming = self.playback.read(cx).state.upcoming_indices();
        let count = upcoming.len();
        let extend_in_flight = self.playback.read(cx).extension_in_flight();
        let context_infinite = self.playback.read(cx).context().is_infinite();
        let title = queue_title(self.playback.read(cx).context());
        let flow_mode_selector =
            queue_flow_mode(self.playback.read(cx).context()).and_then(|mode| {
                self.library.upgrade().map(|library| {
                    flow_controls::render_queue_mode_selector(
                        mode,
                        &library,
                        flow_controls::flow_mode_context_label(false),
                    )
                })
            });
        let narrow =
            crate::music_ui::narrow_content_viewport(f32::from(window.viewport_size().width));

        let snapshots = {
            let state = &self.playback.read(cx).state;
            upcoming
                .iter()
                .enumerate()
                .filter_map(|(ordinal, index)| {
                    let track = state.queue.get(*index)?;
                    Some(QueueRowSnapshot {
                        ordinal,
                        index: *index,
                        count,
                        track: track.clone(),
                        blocked: state.explicit_blocked(track),
                    })
                })
                .collect::<Vec<_>>()
        };
        let has_loading_row = extend_in_flight && count > 0;
        self.sync_list_state(&upcoming, has_loading_row);
        self.install_scroll_handler(count, context_infinite);

        let extension_started = if context_infinite && count == 0 && !extend_in_flight {
            self.request_extension(cx)
        } else if context_infinite
            && count > 0
            && !extend_in_flight
            && self.list_state.is_scrolled_to_end() == Some(true)
        {
            // Scrollbar drags do not emit wheel events. Consult ListState after
            // layout so reaching the exact end still extends the queue.
            self.request_extension(cx)
        } else {
            false
        };
        if extension_started {
            // Playback extension starts synchronously, but this render already
            // has its element tree on the stack. Refresh after the effect cycle
            // so the loading row is visible without a notify-during-render loop.
            cx.defer_in(window, |_, _, cx| cx.notify());
        }

        let list_playback = self.playback.clone();
        let list_downloads = self.downloads.clone();
        let list_account = self.account.clone();
        let list_host = cx.entity().clone();
        let list_search = self.search.clone();
        let list_snapshots = snapshots;
        let list = list(self.list_state.clone(), move |index, _window, _cx| {
            let Some(index) = track_index_for_list_item(index, list_snapshots.len()) else {
                return if has_loading_row {
                    loading_queue_row()
                } else {
                    div()
                        .w_full()
                        .h(px(QUEUE_ITEM_HEIGHT_PX))
                        .flex_none()
                        .into_any_element()
                };
            };
            let snapshot = &list_snapshots[index];
            div()
                .w_full()
                .h(px(QUEUE_ITEM_HEIGHT_PX))
                .flex_none()
                .items_start()
                .child(row(
                    &list_playback,
                    &list_downloads,
                    &list_account,
                    list_host.clone(),
                    list_search.clone(),
                    snapshot.ordinal,
                    snapshot.index,
                    snapshot.count,
                    &snapshot.track,
                    snapshot.blocked,
                ))
                .into_any_element()
        })
        .flex_1()
        .min_h_0()
        // Breathing room above the player bar. Padding lives inside the
        // scrollable list so it only shows at the tail and the scrollbar
        // track still matches the viewport height.
        .pb(px(QUEUE_BOTTOM_PADDING_PX))
        .with_sizing_behavior(gpui::ListSizingBehavior::Auto);

        let queue_scroll = div()
            .id("queue-scroll")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_up_out(MouseButton::Left, |_, window, cx| {
                if cx.has_active_drag() {
                    set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::queue());
                }
            })
            .on_mouse_up(MouseButton::Left, |_, window, cx| {
                if cx.has_active_drag() {
                    set_drag_cursor_owned(window, DragCursorState::Reset, DragCursorOwner::queue());
                }
            })
            // While a row is dragged over the list, track the pointer and
            // auto-scroll near the viewport edges.
            .on_drag_move::<QueueDrag>(cx.listener(
                |this, event: &DragMoveEvent<QueueDrag>, window, cx| {
                    let first_move = this.drag_pointer.is_none();
                    this.drag_pointer = Some(DragAutoScroll {
                        pointer_y: event.event.position.y,
                        viewport: event.bounds,
                    });
                    this.run_drag_autoscroll(cx);
                    set_drag_cursor_owned(
                        window,
                        DragCursorState::Grabbing,
                        DragCursorOwner::queue(),
                    );
                    cx.set_active_drag_cursor_style(grabbing_cursor(), window);
                    if first_move {
                        cx.notify();
                    }
                },
            ));
        let queue_scroll = if count == 0 {
            // Finite queues ended, infinite ones keep loading.
            queue_scroll.child(empty_queue_state(extend_in_flight))
        } else {
            queue_scroll.child(list)
        };
        let queue_scroll =
            queue_scroll.when(narrow, |this| this.pr(px(QUEUE_NARROW_CONTENT_INSET_PX)));
        let queue_scroll = browser_scroll_surface(
            "queue-browser-scroll",
            queue_scroll.into_any_element(),
            BrowserScrollTarget::List(self.list_state.clone()),
            self.browser_scroll.clone(),
        );

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .child(
                div()
                    .flex_none()
                    .h(px(QUEUE_HEADER_HEIGHT_PX))
                    .min_h(px(QUEUE_HEADER_HEIGHT_PX))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .pb(px(20.))
                    .mb(px(16.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .when(narrow, |this| this.pr(px(QUEUE_NARROW_CONTENT_INSET_PX)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(local_icon(LocalIcon::ListUl, FOREGROUND).size(px(14.)))
                            .child(
                                div()
                                    .text_size(px(15.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .when_some(flow_mode_selector, |this, selector| this.child(selector))
                            .child(div().flex_1())
                            .child(crate::music_ui::ghost_close_button_with_icon_size(
                                "queue-close",
                                crate::music_ui::PANEL_CLOSE_ICON_SIZE_PX,
                                {
                                    let playback = self.playback.clone();
                                    move |_, _, cx| {
                                        playback.update(cx, |playback, cx| {
                                            playback.state.close_sidebar();
                                            cx.notify();
                                        })
                                    }
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(QUEUE_SUMMARY_GAP_PX))
                            .h(px(QUEUE_SUMMARY_HEIGHT_PX))
                            .px(px(QUEUE_SUMMARY_PADDING_PX))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE_RAISED))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(QUEUE_SUMMARY_INNER_GAP_PX))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(
                                        local_icon(LocalIcon::ForwardStep, PRIMARY)
                                            .size(px(11.))
                                            .with_transformation(Transformation::translate(point(
                                                px(0.),
                                                px(-1.),
                                            ))),
                                    )
                                    .child("Up next"),
                            )
                            .child(div().text_color(rgb(MUTED)).child(if count == 1 {
                                "1 track".to_owned()
                            } else {
                                format!("{count} tracks")
                            })),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(queue_scroll)
                    .child(queue_scrollbar_lane(&self.list_state, narrow)),
            )
    }
}

fn loading_queue_row() -> gpui::AnyElement {
    div()
        .w_full()
        .h(px(QUEUE_ITEM_HEIGHT_PX))
        .flex_none()
        .flex()
        .items_center()
        .px(px(10.))
        .text_color(rgb(MUTED))
        .child("Loading more tracks...")
        .into_any_element()
}

fn empty_queue_state(extend_in_flight: bool) -> impl IntoElement {
    div()
        .w_full()
        .min_h(px(220.))
        .py(px(60.))
        .px(px(20.))
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .child(
            div()
                .mb(px(12.))
                .child(local_icon(LocalIcon::Music, MUTED).size(px(40.))),
        )
        .child(
            div()
                .mb(px(4.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(if extend_in_flight {
                    "Loading more tracks"
                } else {
                    "Nothing up next"
                }),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(19.5))
                .text_color(rgb(MUTED))
                .child(if extend_in_flight {
                    "Extending this personalized queue."
                } else {
                    "You've reached the end of the queue."
                }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_title_reflects_playback_context() {
        let cases = [
            (
                PlaybackContext::SoundCloudStation {
                    seed_track_id: "1".into(),
                },
                "Station",
            ),
            (
                PlaybackContext::DeezerTrackMix {
                    seed_track_id: "2".into(),
                },
                "Mix",
            ),
            (
                PlaybackContext::DeezerArtistMix {
                    seed_artist_id: "3".into(),
                },
                "Mix",
            ),
            (
                PlaybackContext::DeezerFlow {
                    config_id: "flow".into(),
                    mode: crate::library::FlowMode::Default,
                    tuner: None,
                    kind: crate::playback::DeezerFlowKind::Flow,
                },
                "Flow",
            ),
            (
                PlaybackContext::DeezerFlow {
                    config_id: "mix".into(),
                    mode: crate::library::FlowMode::Default,
                    tuner: None,
                    kind: crate::playback::DeezerFlowKind::SmartMix,
                },
                "Mix",
            ),
            (PlaybackContext::None, "Queue"),
            (PlaybackContext::DeezerLibraryTracks { load_id: 4 }, "Queue"),
            (
                PlaybackContext::SoundCloudCollection {
                    context_urn: "soundcloud:playlists:5".into(),
                },
                "Queue",
            ),
        ];

        for (context, expected) in cases {
            assert_eq!(queue_title(&context), expected);
        }
    }

    #[test]
    fn queue_mode_selector_is_only_available_for_ordinary_flow() {
        let flow = PlaybackContext::DeezerFlow {
            config_id: "flow".into(),
            mode: crate::library::FlowMode::Discovery,
            tuner: None,
            kind: crate::playback::DeezerFlowKind::Flow,
        };
        let smart_mix = PlaybackContext::DeezerFlow {
            config_id: "smart".into(),
            mode: crate::library::FlowMode::Discovery,
            tuner: None,
            kind: crate::playback::DeezerFlowKind::SmartMix,
        };

        assert_eq!(
            queue_flow_mode(&flow),
            Some(crate::library::FlowMode::Discovery)
        );
        assert_eq!(queue_flow_mode(&smart_mix), None);
    }

    #[test]
    fn queue_scrollbar_matches_the_row_viewport_geometry() {
        let source = include_str!("queue.rs");
        let implementation = source.split_once("#[cfg(test)]").map_or_else(
            || panic!("queue implementation section is missing"),
            |(implementation, _)| implementation,
        );

        let header = implementation
            .find(".h(px(QUEUE_HEADER_HEIGHT_PX))")
            .expect("queue header should be present");
        let viewport_wrapper = implementation
            .find(
                ".child(\n                div()\n                    .relative()\n                    .flex()\n                    .flex_col()\n                    .flex_1()\n                    .min_h_0()\n                    .child(queue_scroll)\n                    .child(queue_scrollbar_lane(&self.list_state, narrow)),\n            )",
            )
            .expect("scrollbar and queue should share a viewport wrapper");
        assert!(header < viewport_wrapper);

        assert!(implementation.contains(".top_0()"));
        // The lane stays full height because bottom padding lives inside the
        // scrollable list and scrolls with the tail.
        assert!(implementation.contains(".bottom_0()"));
        assert!(!implementation.contains(".bottom(px(QUEUE_BOTTOM_PADDING_PX))"));
        assert!(implementation.contains(".pb(px(QUEUE_BOTTOM_PADDING_PX))"));
        assert!(implementation.contains(".right(px(-24.))"));
        assert!(implementation.contains(".right_0()"));
        assert!(implementation.contains("Scrollbar::vertical(list_state)"));
        assert!(implementation.contains("ScrollbarShow::Hover"));

        // Outer scroll container must not carry persistent bottom padding.
        // The padding belongs to the inner list so it only shows at the tail.
        let render = source
            .split_once("impl Render for QueuePanel {")
            .and_then(|(_, code)| code.split_once("\nfn loading_queue_row()"))
            .map_or_else(
                || panic!("QueuePanel render implementation should be present"),
                |(render, _)| render,
            );
        let outer_start = render
            .find("let queue_scroll = div()")
            .expect("queue outer scroll container should be present");
        let outer_end = render[outer_start..]
            .find(".on_mouse_up_out")
            .map(|offset| outer_start + offset)
            .expect("queue outer scroll container end marker is missing");
        let outer = &render[outer_start..outer_end];
        assert!(outer.contains(".overflow_hidden()"));
        assert!(
            !outer.contains(".pb(px(QUEUE_BOTTOM_PADDING_PX))"),
            "outer queue scroll must not keep persistent bottom padding"
        );

        let list_start = render
            .find("let list = list(self.list_state.clone()")
            .expect("queue list should be present");
        let list_end = render[list_start..]
            .find(".with_sizing_behavior(gpui::ListSizingBehavior::Auto)")
            .map(|offset| list_start + offset)
            .expect("queue list end marker is missing");
        let list = &render[list_start..list_end];
        assert!(
            list.contains(".pb(px(QUEUE_BOTTOM_PADDING_PX))"),
            "inner queue list should carry the tail padding"
        );

        let inline_scrollbar = [".vertical_scrollbar", "(&self.list_state)"].concat();
        assert!(!implementation.contains(&inline_scrollbar));

        let old_body_gutter = [".pr(px(", "6.))"].concat();
        assert!(!implementation.contains(&old_body_gutter));
    }

    #[test]
    fn queue_close_button_uses_the_shared_panel_icon_size() {
        assert_eq!(crate::music_ui::PANEL_CLOSE_ICON_SIZE_PX, 10.);
        let source = include_str!("queue.rs");
        assert!(source.contains("ghost_close_button_with_icon_size"));
        assert!(source.contains("PANEL_CLOSE_ICON_SIZE_PX"));
    }

    #[test]
    fn queue_viewport_provides_a_vertical_flex_size_for_browser_scroll() {
        let source = include_str!("queue.rs");
        let render = source
            .split_once("impl Render for QueuePanel {")
            .and_then(|(_, code)| code.split_once("\nfn loading_queue_row()"))
            .map_or_else(
                || panic!("QueuePanel render implementation should be present"),
                |(render, _)| render,
            );
        let viewport_chain = ".child(\n                div()\n                    .relative()\n                    .flex()\n                    .flex_col()\n                    .flex_1()\n                    .min_h_0()";
        let viewport_start = render
            .find(viewport_chain)
            .expect("queue viewport should have a vertical flex chain");
        let browser_scroll_start = render
            .find("let queue_scroll = browser_scroll_surface(")
            .expect("queue content should use the browser scroll surface");
        let queue_scroll_child = render[viewport_start + viewport_chain.len()..]
            .find(".child(queue_scroll)")
            .expect("browser scroll should remain a child of the viewport");

        assert!(browser_scroll_start < viewport_start);
        assert!(queue_scroll_child > 0);
        assert!(source.contains(".min_h(px(220.))"));
    }

    #[test]
    fn queue_browser_scroll_wraps_only_the_list_viewport() {
        let source = include_str!("queue.rs");

        assert!(source.contains("browser_scroll: BrowserScrollState,"));
        assert!(source.contains("browser_scroll: BrowserScrollState::new(),"));

        let render = source
            .split_once("impl Render for QueuePanel {")
            .and_then(|(_, code)| code.split_once("\nfn loading_queue_row()"))
            .map_or_else(
                || panic!("QueuePanel render implementation should be present"),
                |(render, _)| render,
            );

        let wrapper_start = render
            .find("let queue_scroll = browser_scroll_surface(")
            .expect("queue content should use the browser scroll surface");
        let wrapper_end = wrapper_start
            + render[wrapper_start..]
                .find(");")
                .map(|offset| offset + ");".len())
                .expect("browser scroll surface call should be closed");
        let wrapper = &render[wrapper_start..wrapper_end];
        let queue_scroll_child = render[wrapper_end..]
            .find(".child(queue_scroll)")
            .map(|offset| wrapper_end + offset)
            .expect("browser scroll should wrap content before it is rendered");
        let viewport_marker = ".child(\n                div()\n                    .relative()\n                    .flex()\n                    .flex_col()\n                    .flex_1()\n                    .min_h_0()\n                    .child(queue_scroll)\n                    .child(queue_scrollbar_lane(&self.list_state, narrow)),";
        let viewport_start = render
            .find(viewport_marker)
            .expect("browser scroll should be scoped to the list viewport");
        let viewport = &render[viewport_start..viewport_start + viewport_marker.len()];

        assert!(wrapper_end < queue_scroll_child);

        assert!(wrapper.contains("queue_scroll.into_any_element()"));
        assert!(wrapper.contains("BrowserScrollTarget::List(self.list_state.clone())"));
        assert!(wrapper.contains("self.browser_scroll.clone()"));
        assert!(!wrapper.contains("queue_scrollbar_lane"));

        let viewport_queue_scroll = viewport
            .find(".child(queue_scroll)")
            .expect("queue scroll should be a viewport child");
        let viewport_scrollbar_lane = viewport
            .find(".child(queue_scrollbar_lane(&self.list_state, narrow))")
            .expect("scrollbar lane should be a viewport child");
        assert!(viewport_queue_scroll < viewport_scrollbar_lane);
    }

    #[test]
    fn empty_queue_state_matches_empty_lyrics_size_contract_without_expanding_loading_row() {
        let source = include_str!("queue.rs");
        let empty_state = source
            .split_once("fn empty_queue_state(extend_in_flight: bool)")
            .and_then(|(_, code)| code.split_once("\n}\n\n#[cfg(test)]"))
            .map_or_else(
                || panic!("empty queue state function should be present"),
                |(body, _)| body,
            );
        let loading_row = source
            .split_once("fn loading_queue_row()")
            .and_then(|(_, code)| code.split_once("\n}\n\nfn empty_queue_state"))
            .map_or_else(
                || panic!("loading queue row function should be present"),
                |(body, _)| body,
            );

        for measurement in [
            ".w_full()",
            ".min_h(px(220.))",
            ".py(px(60.))",
            ".px(px(20.))",
            ".flex()",
            ".flex_col()",
            ".items_center()",
            ".text_center()",
            ".mb(px(12.))",
            "local_icon(LocalIcon::Music, MUTED).size(px(40.))",
            ".mb(px(4.))",
            ".text_size(px(16.))",
            ".font_weight(FontWeight::SEMIBOLD)",
            ".text_color(rgb(FOREGROUND))",
            ".text_size(px(13.))",
            ".line_height(px(19.5))",
            ".text_color(rgb(MUTED))",
        ] {
            assert!(
                empty_state.contains(measurement),
                "empty queue state should contain {measurement}"
            );
        }
        for removed in [
            ".justify_center()",
            ".gap(px(7.))",
            ".px(px(18.))",
            ".py(px(28.))",
            "0x71717a",
            ".size(px(20.))",
            ".max_w(px(230.))",
            ".text_size(px(11.5)",
        ] {
            assert!(
                !empty_state.contains(removed),
                "empty queue state should not contain {removed}"
            );
        }
        assert!(empty_state.contains("\"Loading more tracks\""));
        assert!(empty_state.contains("\"Nothing up next\""));
        assert!(empty_state.contains("\"Extending this personalized queue.\""));
        assert!(empty_state.contains("\"You've reached the end of the queue.\""));

        assert!(loading_row.contains(".h(px(QUEUE_ITEM_HEIGHT_PX))"));
        assert!(!loading_row.contains(".size(px(40.))"));
    }
}
