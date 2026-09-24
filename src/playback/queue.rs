use std::time::Duration;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{
        BrowserScrollState, BrowserScrollTarget, FixedListScrollHandle, browser_scroll_surface,
    },
    drag_cursor::{DragCursorOwner, DragCursorState, grabbing_cursor, set_drag_cursor_owned},
    library::{FavoriteKey, FavoriteKind, FavoriteState, LibraryView, flow_controls},
    playback::RightSidebar,
    search::SearchView,
    theme::{BACKGROUND, BORDER, FOREGROUND, MUTED, PRIMARY, SURFACE_RAISED},
};
use gpui::{
    App, Bounds, Context, DragMoveEvent, Entity, FontWeight, IntoElement, ListAlignment, ListState,
    MouseButton, Pixels, Render, Transformation, Window, div, list, point, prelude::*, px, rgb,
};
use gpui_component::scroll::{Scrollbar, ScrollbarShow};

use super::{
    PlaybackContext, PlaybackModel,
    queue_list::{
        QueueListChange, QueueListIdentity, QueueRowSnapshot, list_change, should_extend,
        track_index_for_list_item,
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

fn queue_scrollbar_lane(scroll: &FixedListScrollHandle, narrow: bool) -> impl IntoElement {
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
        .child(Scrollbar::vertical(scroll).scrollbar_show(ScrollbarShow::Hover))
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

    /// Whether this panel renders inside the detached floating window.
    /// Derived from the playback state so a popout window that dies without
    /// a callback can never leave the panel stuck in detached mode.
    fn detached(&self, cx: &App) -> bool {
        self.playback.read_with(cx, |model, _| {
            model.state.right_sidebar_popped() == Some(RightSidebar::Queue)
        })
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
        let scroll = self.fixed_scroll_handle();
        let target_y = (scroll.position() + delta).clamp(0., scroll.maximum());
        scroll.set_position(target_y);
        cx.notify();
        true
    }

    /// Exact scroll math for the queue list. Queue rows are all the same fixed
    /// height, so the fixed-height adapter keeps the scrollbar thumb, wheel
    /// bounds, and drag autoscroll clamps exact even while gpui has discarded
    /// the list's cached heights and size hints after a width change.
    fn fixed_scroll_handle(&self) -> FixedListScrollHandle {
        FixedListScrollHandle::new(
            self.list_state.clone(),
            self.list_identity.item_count(),
            px(QUEUE_ITEM_HEIGHT_PX),
        )
        .with_tail_padding(px(QUEUE_BOTTOM_PADDING_PX))
    }

    /// Whether the queue sits at the very bottom of its scroll range, using the
    /// fixed-height math so the answer stays exact even when gpui has
    /// discarded the list's cached heights and size hints. The one pixel
    /// tolerance mirrors gpui's own scrollbar end-of-drag check.
    fn is_scrolled_to_end(&self) -> bool {
        let scroll = self.fixed_scroll_handle();
        scroll.maximum() > 0. && scroll.position() >= scroll.maximum() - 1.
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

        let narrow = !self.detached(cx)
            && crate::music_ui::narrow_content_viewport(f32::from(window.viewport_size().width));
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
                        blocked: state.content_blocked(track),
                    })
                })
                .collect::<Vec<_>>()
        };
        let has_loading_row = extend_in_flight && count > 0;
        self.sync_list_state(&upcoming, has_loading_row);
        self.install_scroll_handler(count, context_infinite);

        let extension_started = if context_infinite && count == 0 && !extend_in_flight {
            self.request_extension(cx)
        } else if context_infinite && count > 0 && !extend_in_flight && self.is_scrolled_to_end() {
            // Scrollbar drags do not emit wheel events. Consult the scroll
            // state after layout so reaching the exact end still extends the
            // queue.
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
        // The fixed-height adapter gives the scrollbar and the wheel math an
        // exact extent: gpui discards the list's size hints after a width
        // change, which would otherwise leave unmeasured rows at zero height
        // and make the thumb drift while scrolling a large queue.
        let fixed_scroll = self.fixed_scroll_handle();
        let queue_scroll = browser_scroll_surface(
            "queue-browser-scroll",
            queue_scroll.into_any_element(),
            BrowserScrollTarget::FixedList(fixed_scroll.clone()),
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
                            .child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .items_center()
                                    .gap(px(8.))
                                    .min_w_0()
                                    .when(self.detached(cx), |this| {
                                        this.window_control_area(gpui::WindowControlArea::Drag)
                                    })
                                    .child(local_icon(LocalIcon::ListUl, FOREGROUND).size(px(14.)))
                                    .child(
                                        div()
                                            .text_size(px(15.5))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(title),
                                    ),
                            )
                            .when_some(flow_mode_selector, |this, selector| this.child(selector))
                            .when(!self.detached(cx), |this| {
                                this.child(crate::music_ui::ghost_icon_button_with_nudge(
                                    "queue-detach",
                                    LocalIcon::ArrowUpRightFromSquare,
                                    "Pop out",
                                    12.,
                                    1.,
                                    {
                                        let playback = self.playback.clone();
                                        move |_, _, cx| {
                                            playback.update(cx, |playback, cx| {
                                                playback.state.detach_sidebar();
                                                cx.notify();
                                            });
                                        }
                                    },
                                ))
                            })
                            .when(self.detached(cx), |this| {
                                let playback = self.playback.clone();
                                this.child(crate::music_ui::ghost_icon_button_with_nudge(
                                    "queue-always-on-top",
                                    if playback.read(cx).state.popout_always_on_top() {
                                        LocalIcon::Thumbtack
                                    } else {
                                        LocalIcon::ThumbtackSlash
                                    },
                                    if playback.read(cx).state.popout_always_on_top() {
                                        "Disable always on top"
                                    } else {
                                        "Enable always on top"
                                    },
                                    12.,
                                    0.,
                                    {
                                        let playback = playback.clone();
                                        move |_, _, cx| {
                                            playback.update(cx, |playback, cx| {
                                                playback.state.toggle_popout_always_on_top();
                                                cx.notify();
                                            });
                                        }
                                    },
                                ))
                            })
                            .child(crate::music_ui::ghost_close_button_with_icon_size(
                                "queue-close",
                                crate::music_ui::PANEL_CLOSE_ICON_SIZE_PX,
                                {
                                    let playback = self.playback.clone();
                                    move |_, window, cx| {
                                        if playback.read(cx).state.right_sidebar_popped().is_some()
                                        {
                                            // Closing the popout docks the
                                            // view back into the app, then
                                            // removes the window.
                                            playback.update(cx, |playback, cx| {
                                                playback.state.dock_sidebar();
                                                cx.notify();
                                            });
                                            window.remove_window();
                                        } else {
                                            playback.update(cx, |playback, cx| {
                                                playback.state.close_sidebar();
                                                cx.notify();
                                            });
                                        }
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
                    .child(queue_scrollbar_lane(&fixed_scroll, narrow)),
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

    #[gpui::test]
    fn queue_scroll_math_stays_exact_across_gpui_hint_discards(cx: &mut gpui::TestAppContext) {
        use gpui_component::scroll::ScrollbarHandle;

        // Mirrors the queue list: fixed-height rows with a uniform height
        // hint, tail padding inside the scrollable content, and the
        // fixed-height scroll handle the panel uses for scrollbar and wheel
        // math. gpui discards cached heights and size hints on the first
        // layout and on every width change, which leaves unmeasured rows at
        // zero height in the raw ListState; the handle must stay exact.
        struct QueueListProbe(gpui::ListState);
        impl gpui::Render for QueueListProbe {
            fn render(
                &mut self,
                _: &mut gpui::Window,
                _: &mut gpui::Context<Self>,
            ) -> impl gpui::IntoElement {
                gpui::div().size_full().child(
                    gpui::list(self.0.clone(), |_, _, _| {
                        gpui::div()
                            .w_full()
                            .h(px(QUEUE_ITEM_HEIGHT_PX))
                            .flex_none()
                            .into_any_element()
                    })
                    .w_full()
                    .h_full()
                    .pb(px(QUEUE_BOTTOM_PADDING_PX)),
                )
            }
        }

        const ROWS: usize = 120;
        let content_height = ROWS as f32 * QUEUE_ITEM_HEIGHT_PX + QUEUE_BOTTOM_PADDING_PX;
        let state =
            gpui::ListState::new(ROWS, gpui::ListAlignment::Top, px(QUEUE_LIST_OVERDRAW_PX))
                .with_uniform_item_height(px(QUEUE_ITEM_HEIGHT_PX));
        let scroll = FixedListScrollHandle::new(state.clone(), ROWS, px(QUEUE_ITEM_HEIGHT_PX))
            .with_tail_padding(px(QUEUE_BOTTOM_PADDING_PX));
        let window = cx.add_window({
            let state = state.clone();
            |_, _| QueueListProbe(state)
        });
        let mut visual =
            gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        visual.simulate_resize(gpui::size(px(320.), px(600.)));
        visual.run_until_parked();

        // First layout: the raw extent only counts measured rows while the
        // handle reports the exact content size from the first frame.
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(f32::from(scroll.content_size().height), content_height);
        assert_eq!(scroll.maximum(), content_height - 600.);

        // Wheel scrolling deep into the list maps to exact item positions.
        scroll.set_position(3000.);
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(scroll.position(), 3000.);
        assert_eq!(
            state.logical_scroll_top().item_ix,
            (3000. / QUEUE_ITEM_HEIGHT_PX) as usize
        );

        // A width change (sidebar animation, window resize) discards the
        // list's cached heights, but the handle keeps both bounds and the
        // current position exact.
        visual.simulate_resize(gpui::size(px(300.), px(600.)));
        visual.update(|window, cx| {
            _ = window.draw(cx);
        });
        assert_eq!(f32::from(scroll.content_size().height), content_height);
        assert_eq!(scroll.maximum(), content_height - 600.);
        assert_eq!(scroll.position(), 3000.);

        // Dragging the thumb to the very bottom reaches the exact end, which
        // is what re-arms infinite queue extension.
        scroll.set_position(f32::INFINITY);
        assert_eq!(scroll.position(), scroll.maximum());
    }

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
}
