mod annotation;
mod client;
mod core;
mod genius_fragment;
mod scroll_motion;
mod scroll_state;

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use self::core::{
    EmptyLyricsReason, GeniusLyrics, LyricLine, LyricsCache, LyricsCacheKey, LyricsCacheStore,
    LyricsProvider, LyricsResponse, LyricsTrack, collapse_blank_lyric_gaps, genius_line_fragments,
    lyric_block_text, lyric_full_text, parse_synced_lyrics, prepare_genius_lyrics,
};
use futures::future::{Either, select};
use gpui::{
    AnimationExt, AnyElement, App, ClickEvent, Context, FontWeight, HighlightStyle, IntoElement,
    Render, ScrollHandle, StyledText, Task, Window, div, point, prelude::*, px, rgb, rgba,
};
use gpui_component::scroll::{Scrollbar, ScrollbarHandle, ScrollbarShow};
use tokio::{runtime::Runtime, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    assets::{LocalIcon, local_icon},
    browser_scroll::{BrowserScrollState, BrowserScrollTarget, browser_scroll_surface},
    context_menu::ContextMenuExt,
    motion::SegmentedSelectorMotion,
    navigation_state::LyricsSource,
    playback::{PlaybackModel, PlaybackStatus, PlaybackTrack},
    playing_indicator::playing_bars,
    theme::{BACKGROUND, BORDER, DANGER, FOREGROUND, MUTED, PRIMARY, SURFACE_RAISED},
};

use self::annotation::{AnnotationController, AnnotationOpen};
use self::scroll_motion::ScrollMotion;
use self::scroll_state::{
    LyricsScrollbarHandle, ScrollOffsetFreeze, ScrollSuppression, active_line_index,
    centered_scroll_offset, is_highlightable_line, should_resume_auto_centering, timed_line_index,
};

// GPUI's stock scrollbar considers itself visible for 3s after scrolling.
const USER_SCROLL_SUPPRESSION: Duration = Duration::from_millis(3000);
const CENTER_RETRY_DELAY: Duration = Duration::from_millis(16);
const CENTER_RETRY_ATTEMPTS: u8 = 4;
const LYRICS_HEADER_BLOCK_HEIGHT: f32 = 117.;
const MIN_LYRICS_VIEWPORT_HEIGHT: f32 = 20.;
const LYRICS_BOTTOM_PADDING_PX: f32 = 16.;
const LYRIC_LINE_CONTENT_HEIGHT: f32 = 22.5;
const LYRIC_LINE_PADDING_Y: f32 = 6.;

fn annotation_panel_max_height(viewport_height: f32) -> f32 {
    let available_height =
        (viewport_height - LYRICS_HEADER_BLOCK_HEIGHT - MIN_LYRICS_VIEWPORT_HEIGHT).max(0.);
    (viewport_height * 0.52)
        .clamp(150., 360.)
        .min(available_height)
}

fn lyrics_scrollbar_outset(viewport_width: f32) -> f32 {
    if crate::music_ui::narrow_content_viewport(viewport_width) {
        0.
    } else {
        24.
    }
}

fn lyrics_vertical_scrollbar(
    scroll: &(impl ScrollbarHandle + Clone),
    viewport_width: f32,
    on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id("lyrics-vertical-scrollbar")
        .absolute()
        .top_0()
        .bottom_0()
        .right(px(-lyrics_scrollbar_outset(viewport_width)))
        // Lane-sized so lyric text hover does not unfreeze or reveal the bar.
        .w(px(16.))
        .on_hover(on_hover)
        .child(Scrollbar::vertical(scroll).scrollbar_show(ScrollbarShow::Hover))
        .into_any_element()
}

pub(crate) struct LyricsTrackInput(LyricsTrack);

impl LyricsTrackInput {
    pub(crate) fn from_playback(track: &PlaybackTrack) -> Self {
        Self(LyricsTrack {
            stable_id: Some(lyrics_track_identity(track)),
            artist: track.artist.clone(),
            title: track.title.clone(),
            album: Some(track.album.clone()),
            duration: Some(track.duration.as_secs()),
        })
    }
}

fn lyrics_track_identity(track: &PlaybackTrack) -> String {
    let provider = match track.provider {
        crate::playback::PlaybackProvider::Deezer => "deezer",
        crate::playback::PlaybackProvider::SoundCloud => "soundcloud",
    };
    format!("{provider}:{}", track.id)
}

pub(crate) struct LyricsPanel {
    playback: gpui::Entity<PlaybackModel>,
    detached: bool,
    runtime: Arc<Runtime>,
    client: client::LyricsClient,
    cache: LyricsCache,
    track: Option<LyricsTrack>,
    preferred_provider: LyricsProvider,
    provider: LyricsProvider,
    provider_motion: SegmentedSelectorMotion,
    manual_provider: Option<LyricsProvider>,
    response: Option<LyricsResponse>,
    status: Status,
    lines: Vec<LyricLine>,
    full_lines: Vec<String>,
    full_text: String,
    prepared: Option<GeniusLyrics>,
    position: f64,
    timed_line: Option<usize>,
    active_line: Option<usize>,
    annotation: AnnotationController,
    generation: u64,
    cancellation: CancellationToken,
    scroll: ScrollHandle,
    browser_scroll: BrowserScrollState,
    last_browser_scroll_generation: u64,
    scroll_motion: ScrollMotion,
    scrollbar_freeze: ScrollOffsetFreeze,
    scroll_suppression: ScrollSuppression,
    scroll_release_task: Option<Task<()>>,
    center_pending: bool,
    center_wait_for_layout: bool,
    center_retry_remaining: u8,
    center_retry_task_id: u64,
    center_retry_task_running: bool,
}

#[derive(Clone, Debug)]
enum Status {
    Idle,
    Loading,
    Empty,
    Error(String),
    Ready,
}

impl LyricsPanel {
    pub(crate) fn new(playback: gpui::Entity<PlaybackModel>, runtime: Arc<Runtime>) -> Self {
        Self {
            playback,
            detached: false,
            runtime,
            client: client::LyricsClient::new(),
            cache: LyricsCache::default(),
            track: None,
            preferred_provider: LyricsProvider::Musixmatch,
            provider: LyricsProvider::Musixmatch,
            provider_motion: SegmentedSelectorMotion::default(),
            manual_provider: None,
            response: None,
            status: Status::Idle,
            lines: Vec::new(),
            full_lines: Vec::new(),
            full_text: String::new(),
            prepared: None,
            position: 0.0,
            timed_line: None,
            active_line: None,
            annotation: AnnotationController::new(),
            generation: 0,
            cancellation: CancellationToken::new(),
            scroll: ScrollHandle::new(),
            browser_scroll: BrowserScrollState::new(),
            last_browser_scroll_generation: 0,
            scroll_motion: ScrollMotion::default(),
            scrollbar_freeze: ScrollOffsetFreeze::default(),
            scroll_suppression: ScrollSuppression::default(),
            scroll_release_task: None,
            center_pending: false,
            center_wait_for_layout: false,
            center_retry_remaining: 0,
            center_retry_task_id: 0,
            center_retry_task_running: false,
        }
    }

    /// Marks the panel as hosted by the detached floating window. The
    /// detached header hides the pop-out button and drags the window.
    pub(crate) fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.detached != detached {
            self.detached = detached;
            cx.notify();
        }
    }
    pub(crate) fn set_default_provider(&mut self, source: LyricsSource, cx: &mut Context<Self>) {
        let provider = match source {
            LyricsSource::Musixmatch => LyricsProvider::Musixmatch,
            LyricsSource::Genius => LyricsProvider::Genius,
        };
        if self.preferred_provider == provider {
            return;
        }
        self.preferred_provider = provider;
        if self.manual_provider.is_none() && self.track.is_some() {
            self.provider = provider;
            self.clear_response();
            self.load(cx);
        }
    }

    pub(crate) fn sync_track(
        &mut self,
        visible: bool,
        track: Option<LyricsTrackInput>,
        position: f64,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        let track = track.map(|input| input.0);
        if !track_changed(self.track.as_ref(), track.as_ref()) {
            if visible {
                let timed_line = timed_line_index(&self.lines, position);
                let active = active_line_index(&self.lines, position);
                if self.timed_line != timed_line || self.active_line != active {
                    if self.timed_line != timed_line {
                        self.scroll_motion.cancel();
                    }
                    self.timed_line = timed_line;
                    self.active_line = active;
                    self.center_pending = timed_line.is_some();
                    self.center_retry_remaining = CENTER_RETRY_ATTEMPTS;
                    cx.notify();
                }
            }
            return;
        }
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.generation = self.generation.wrapping_add(1);
        self.track = track;
        self.manual_provider = None;
        self.provider = self.preferred_provider;
        self.clear_response();
        if self.track.is_some() {
            self.load(cx);
        } else {
            self.status = Status::Idle;
        }
    }

    fn clear_response(&mut self) {
        self.response = None;
        self.lines.clear();
        self.full_lines.clear();
        self.full_text.clear();
        self.prepared = None;
        self.timed_line = None;
        self.active_line = None;
        self.annotation.close();
        self.annotation.clear_cache();
        self.reset_scroll_state();
    }

    fn reset_scroll_state(&mut self) {
        self.scroll_motion.cancel();
        self.browser_scroll.reset();
        self.last_browser_scroll_generation = self.browser_scroll.user_input_generation();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.scrollbar_freeze.reset();
        self.scroll_suppression.reset();
        self.scroll_release_task = None;
        self.center_pending = false;
        self.center_wait_for_layout = false;
        self.center_retry_remaining = 0;
        self.center_retry_task_id = self.center_retry_task_id.wrapping_add(1);
        self.center_retry_task_running = false;
    }

    fn mark_user_scrolling(&mut self, cx: &mut Context<Self>) {
        self.scroll_motion.cancel();
        self.scrollbar_freeze.reveal_for_user_scroll();
        cx.notify();
        let generation = self.scroll_suppression.mark_user_scroll();
        self.scroll_release_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(USER_SCROLL_SUPPRESSION)
                .await;
            this.update(cx, |this, cx| {
                if this.scroll_suppression.generation() != generation {
                    return;
                }
                this.scroll_release_task.take();
                this.scroll_suppression.release_if_current(generation);
                this.center_pending = should_resume_auto_centering(this.timed_line);
                if this.center_pending {
                    this.center_retry_remaining = CENTER_RETRY_ATTEMPTS;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn schedule_center_retry(&mut self, cx: &mut Context<Self>) {
        if self.center_retry_remaining == 0 || self.center_retry_task_running {
            return;
        }
        self.center_retry_remaining -= 1;
        self.center_retry_task_running = true;
        self.center_retry_task_id = self.center_retry_task_id.wrapping_add(1);
        let retry_task_id = self.center_retry_task_id;
        let generation = self.generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(CENTER_RETRY_DELAY).await;
            this.update(cx, |this, cx| {
                if this.center_retry_task_id != retry_task_id {
                    return;
                }
                this.center_retry_task_running = false;
                if this.generation == generation && this.center_pending {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn center_active_line_if_needed(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(index) = self.timed_line else {
            self.center_pending = false;
            return;
        };
        if !self.center_pending || self.scroll_suppression.is_suppressed() {
            return;
        }
        if self.center_wait_for_layout {
            self.center_wait_for_layout = false;
            self.schedule_center_retry(cx);
            return;
        }

        let Some(child_bounds) = self.scroll.bounds_for_item(index) else {
            self.schedule_center_retry(cx);
            return;
        };
        let viewport_bounds = self.scroll.bounds();
        if viewport_bounds.size.height <= px(0.) || child_bounds.size.height <= px(0.) {
            self.schedule_center_retry(cx);
            return;
        }

        let target_offset = centered_scroll_offset(
            f32::from(viewport_bounds.top()),
            f32::from(viewport_bounds.size.height),
            f32::from(child_bounds.top()),
            f32::from(child_bounds.size.height),
            f32::from(self.scroll.max_offset().y),
        );
        let from = f32::from(self.scroll.offset().y);
        let offset_changed = from != target_offset;
        self.scrollbar_freeze.hide_for_changed_auto_scroll(
            offset_changed,
            f32::from(self.scroll.offset().x),
            from,
        );
        if offset_changed {
            self.scroll_motion.animate(
                &self.scroll,
                from,
                target_offset,
                window,
                cx.reduce_motion(),
            );
        }
        self.center_pending = false;
    }

    fn lyric_menu<E>(
        element: E,
        line_text: String,
        block_text: String,
        full_text: Arc<str>,
        url: Option<String>,
    ) -> gpui::AnyElement
    where
        E: gpui::InteractiveElement
            + gpui::ParentElement
            + gpui::Styled
            + gpui::IntoElement
            + 'static,
    {
        element
            .context_menu(move |menu, _, _| {
                crate::context_menu::lyrics_copy_menu(
                    menu,
                    line_text.clone(),
                    block_text.clone(),
                    full_text.to_string(),
                    url.clone(),
                )
            })
            .into_any_element()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.cancellation.cancel();
        self.cancellation = CancellationToken::new();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let Some(track) = self.track.clone() else {
            self.status = Status::Idle;
            return;
        };
        let provider = self.provider;
        let manual = self.manual_provider.is_some();
        let key = LyricsCacheKey::new(provider, &track);
        if let Some(value) = self.cache.get(&key).cloned() {
            if !manual && matches!(value, LyricsResponse::Empty { .. }) {
                self.load_alternate(track, provider, Some(value), generation, cx);
            } else {
                self.apply(provider, value);
                self.preload_alternate(track, provider, generation, cx);
            }
            return;
        }
        if manual {
            self.load_single(track, provider, true, generation, cx);
            return;
        }
        if self
            .cache
            .get(&LyricsCacheKey::new(alternate_provider(provider), &track))
            .is_some()
        {
            self.load_single(track, provider, false, generation, cx);
            return;
        }
        self.load_both_parallel(track, provider, generation, cx);
    }

    fn load_single(
        &mut self,
        track: LyricsTrack,
        provider: LyricsProvider,
        manual: bool,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        self.status = Status::Loading;
        let client = self.client.clone();
        let cancellation = self.cancellation.clone();
        let request_track = track.clone();
        let task = self
            .runtime
            .spawn(async move { client.load(provider, request_track, cancellation).await });
        cx.spawn(async move |this, cx| {
            let result = lyrics_task_result(task).await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(value) => {
                        this.cache
                            .set(LyricsCacheKey::new(provider, &track), value.clone());
                        if !manual && matches!(value, LyricsResponse::Empty { .. }) {
                            this.load_alternate(track, provider, Some(value), generation, cx);
                        } else {
                            this.apply(provider, value);
                            this.preload_alternate(track, provider, generation, cx);
                        }
                    }
                    Err(error) if !manual && error != "Lyrics request cancelled" => {
                        this.load_alternate(track, provider, None, generation, cx)
                    }
                    Err(error) if error != "Lyrics request cancelled" => {
                        this.status = Status::Error(error)
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn preload_alternate(
        &mut self,
        track: LyricsTrack,
        displayed_provider: LyricsProvider,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let provider = alternate_provider(displayed_provider);
        let key = LyricsCacheKey::new(provider, &track);
        if self.cache.get(&key).is_some() {
            return;
        }
        let client = self.client.clone();
        let cancellation = self.cancellation.clone();
        let request_track = track.clone();
        let task = self
            .runtime
            .spawn(async move { client.load(provider, request_track, cancellation).await });
        cx.spawn(async move |this, cx| {
            let result = lyrics_task_result(task).await;
            this.update(cx, |this, _| {
                if generation != this.generation {
                    return;
                }
                if this
                    .cache
                    .get(&LyricsCacheKey::new(provider, &track))
                    .is_some()
                {
                    return;
                }
                if let Ok(value) = result {
                    this.cache.set(LyricsCacheKey::new(provider, &track), value);
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_both_parallel(
        &mut self,
        track: LyricsTrack,
        primary_provider: LyricsProvider,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let alternate = alternate_provider(primary_provider);
        let primary_key = LyricsCacheKey::new(primary_provider, &track);
        let alternate_key = LyricsCacheKey::new(alternate, &track);
        self.status = Status::Loading;
        let client = self.client.clone();
        let cancellation = self.cancellation.clone();
        let primary_task = self.runtime.spawn({
            let client = client.clone();
            let cancellation = cancellation.clone();
            let request_track = track.clone();
            async move {
                client
                    .load(primary_provider, request_track, cancellation)
                    .await
            }
        });
        let alternate_task = self.runtime.spawn({
            let request_track = track.clone();
            async move { client.load(alternate, request_track, cancellation).await }
        });
        cx.spawn(async move |this, cx| {
            let primary_future = Box::pin(lyrics_task_result(primary_task));
            let alternate_future = Box::pin(lyrics_task_result(alternate_task));
            let (first_provider, first_result, second_provider, second_future) =
                match select(primary_future, alternate_future).await {
                    Either::Left((result, remaining)) => {
                        (primary_provider, result, alternate, remaining)
                    }
                    Either::Right((result, remaining)) => {
                        (alternate, result, primary_provider, remaining)
                    }
                };
            let first_has_lyrics = this
                .update(cx, |this, cx| {
                    if generation != this.generation {
                        return None;
                    }
                    let key = LyricsCacheKey::new(first_provider, &track);
                    let has_lyrics = match first_result {
                        Ok(value) if !matches!(value, LyricsResponse::Empty { .. }) => {
                            this.cache.set(key, value.clone());
                            this.apply(first_provider, value);
                            true
                        }
                        Ok(empty) => {
                            this.cache.set(key, empty);
                            this.status = Status::Loading;
                            false
                        }
                        Err(error) if error == "Lyrics request cancelled" => false,
                        Err(_) => {
                            this.status = Status::Loading;
                            false
                        }
                    };
                    cx.notify();
                    Some(has_lyrics)
                })
                .ok()
                .flatten();
            let Some(first_has_lyrics) = first_has_lyrics else {
                return;
            };
            let second_result = second_future.await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                let second_key = LyricsCacheKey::new(second_provider, &track);
                if first_has_lyrics {
                    if let Ok(value) = second_result {
                        if this.cache.get(&second_key).is_none() {
                            this.cache.set(second_key, value.clone());
                        }
                        if let Some((provider, value)) =
                            late_primary_selection(primary_provider, second_provider, value)
                        {
                            this.apply(provider, value);
                            cx.notify();
                        }
                    }
                    return;
                }
                match second_result {
                    Ok(value) => {
                        this.cache.set(second_key, value.clone());
                        if second_provider == primary_provider {
                            this.apply(primary_provider, value);
                        } else {
                            let primary_empty =
                                this.cache.get(&primary_key).cloned().filter(|cached| {
                                    matches!(cached, LyricsResponse::Empty { .. })
                                });
                            let (selected_provider, selected) = automatic_fallback_selection(
                                primary_provider,
                                primary_empty,
                                value,
                            );
                            this.apply(selected_provider, selected);
                        }
                    }
                    Err(error) if error != "Lyrics request cancelled" => {
                        if let Some(primary_empty) = this
                            .cache
                            .get(&primary_key)
                            .cloned()
                            .filter(|cached| matches!(cached, LyricsResponse::Empty { .. }))
                        {
                            this.apply(primary_provider, primary_empty);
                        } else if let Some(alternate_empty) = this
                            .cache
                            .get(&alternate_key)
                            .cloned()
                            .filter(|cached| matches!(cached, LyricsResponse::Empty { .. }))
                        {
                            this.apply(alternate, alternate_empty);
                        } else {
                            this.status = Status::Error(error);
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_alternate(
        &mut self,
        track: LyricsTrack,
        primary_provider: LyricsProvider,
        primary_empty: Option<LyricsResponse>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let provider = alternate_provider(primary_provider);
        let key = LyricsCacheKey::new(provider, &track);
        if let Some(value) = self.cache.get(&key).cloned() {
            let (selected_provider, selected) =
                automatic_fallback_selection(primary_provider, primary_empty, value);
            self.apply(selected_provider, selected);
            return;
        }
        self.status = Status::Loading;
        let client = self.client.clone();
        let cancellation = self.cancellation.clone();
        let request_track = track.clone();
        let task = self
            .runtime
            .spawn(async move { client.load(provider, request_track, cancellation).await });
        cx.spawn(async move |this, cx| {
            let result = lyrics_task_result(task).await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(value) => {
                        this.cache
                            .set(LyricsCacheKey::new(provider, &track), value.clone());
                        let (selected_provider, selected) =
                            automatic_fallback_selection(primary_provider, primary_empty, value);
                        this.apply(selected_provider, selected);
                    }
                    Err(error) if error != "Lyrics request cancelled" => {
                        if let Some(primary_empty) = primary_empty {
                            this.apply(primary_provider, primary_empty);
                        } else {
                            this.status = Status::Error(error);
                        }
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply(&mut self, provider: LyricsProvider, response: LyricsResponse) {
        if self.provider != provider {
            self.reset_scroll_state();
        }
        self.provider = provider;
        self.lines = match &response {
            LyricsResponse::Synced { text, .. } => {
                collapse_blank_lyric_gaps(parse_synced_lyrics(text))
            }
            _ => Vec::new(),
        };
        self.full_lines = match &response {
            LyricsResponse::Synced { .. } => {
                self.lines.iter().map(|line| line.text.clone()).collect()
            }
            LyricsResponse::Plain { text, .. } | LyricsResponse::Genius { text, .. } => {
                text.split('\n').map(str::to_owned).collect()
            }
            _ => Vec::new(),
        };
        self.full_text = lyric_full_text(&self.full_lines);
        self.prepared = match &response {
            LyricsResponse::Genius {
                text,
                annotations,
                url,
            } => Some(prepare_genius_lyrics(text, annotations, url)),
            _ => None,
        };
        self.timed_line = timed_line_index(&self.lines, self.position);
        self.active_line = active_line_index(&self.lines, self.position);
        self.center_pending = self.timed_line.is_some();
        self.center_wait_for_layout = self.center_pending;
        self.center_retry_remaining = CENTER_RETRY_ATTEMPTS;
        self.status = if matches!(response, LyricsResponse::Empty { .. }) {
            Status::Empty
        } else {
            Status::Ready
        };
        self.response = Some(response);
    }

    fn select(&mut self, provider: LyricsProvider, cx: &mut Context<Self>) {
        if self.provider == provider && self.manual_provider == Some(provider) {
            return;
        }
        self.provider = provider;
        self.manual_provider = Some(provider);
        self.clear_response();
        self.load(cx);
    }

    fn seek_to_line(&mut self, time: f64, cx: &mut Context<Self>) {
        let current_id = self
            .playback
            .read(cx)
            .state
            .current()
            .map(lyrics_track_identity);
        let lyrics_id = self
            .track
            .as_ref()
            .and_then(|track| track.stable_id.as_ref());
        if current_id.as_ref() != lyrics_id {
            return;
        }
        self.playback.update(cx, |playback, cx| {
            playback.seek_to(Duration::from_secs_f64(time.max(0.0)), cx)
        });
    }

    fn show_annotation(&mut self, id: String, cx: &mut Context<Self>) {
        match self.annotation.open(&id) {
            AnnotationOpen::Unchanged => {}
            AnnotationOpen::Shown => cx.notify(),
            AnnotationOpen::Fetch(generation, cancellation) => {
                let client = self.client.clone();
                let task = self
                    .runtime
                    .spawn(async move { client.annotation(&id, cancellation).await });
                cx.spawn(async move |this, cx| {
                    let result = lyrics_task_result(task).await;
                    this.update(cx, |this, cx| {
                        if this.annotation.complete(generation, result) {
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
                cx.notify();
            }
        }
    }
}

async fn lyrics_task_result<T>(task: JoinHandle<Result<T, String>>) -> Result<T, String> {
    task.await
        .unwrap_or_else(|_| Err("Lyrics request failed unexpectedly".into()))
}

fn alternate_provider(provider: LyricsProvider) -> LyricsProvider {
    match provider {
        LyricsProvider::Musixmatch => LyricsProvider::Genius,
        LyricsProvider::Genius => LyricsProvider::Musixmatch,
    }
}

fn automatic_fallback_selection(
    primary_provider: LyricsProvider,
    primary_empty: Option<LyricsResponse>,
    alternate: LyricsResponse,
) -> (LyricsProvider, LyricsResponse) {
    match (primary_empty, alternate) {
        (Some(primary @ LyricsResponse::Empty { .. }), LyricsResponse::Empty { .. }) => {
            (primary_provider, primary)
        }
        (_, alternate) => (alternate_provider(primary_provider), alternate),
    }
}

fn late_primary_selection(
    primary_provider: LyricsProvider,
    completed_provider: LyricsProvider,
    response: LyricsResponse,
) -> Option<(LyricsProvider, LyricsResponse)> {
    (completed_provider == primary_provider && !matches!(&response, LyricsResponse::Empty { .. }))
        .then_some((primary_provider, response))
}

fn track_changed(current: Option<&LyricsTrack>, next: Option<&LyricsTrack>) -> bool {
    current != next
}

fn lyric_line_container(id: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w_full()
        .px(px(10.))
        .py(px(LYRIC_LINE_PADDING_Y))
        .rounded(px(6.))
        .text_size(px(15.))
        .line_height(px(LYRIC_LINE_CONTENT_HEIGHT))
        .text_color(rgb(MUTED))
}

fn lyric_gap_visualizer(
    id: String,
    index: usize,
    animating: bool,
    active: bool,
    reduce_motion: bool,
) -> gpui::Stateful<gpui::Div> {
    lyric_line_container(id)
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff0a)))
        .when(active, |this| this.bg(rgba(0xffffff14)))
        .child(
            div()
                .h(px(LYRIC_LINE_CONTENT_HEIGHT))
                .flex()
                .items_center()
                .justify_start()
                .child(playing_bars(
                    ("lyrics-gap", index),
                    animating,
                    if active { PRIMARY } else { MUTED },
                    reduce_motion,
                )),
        )
}

fn centered_lyrics_message(
    icon: LocalIcon,
    heading: &'static str,
    description: StyledText,
) -> AnyElement {
    div()
        .w_full()
        .py(px(60.))
        .px(px(20.))
        .flex()
        .flex_col()
        .items_center()
        .text_center()
        .child(
            div()
                .mb(px(12.))
                .child(local_icon(icon, MUTED).size(px(40.))),
        )
        .child(
            div()
                .mb(px(4.))
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(FOREGROUND))
                .child(heading),
        )
        .child(
            div()
                .text_size(px(13.))
                .line_height(px(19.5))
                .text_color(rgb(MUTED))
                .child(description),
        )
        .into_any_element()
}

fn empty_lyrics_state(provider: LyricsProvider, reason: &EmptyLyricsReason) -> AnyElement {
    let (icon, heading, description, emphasize_genius) = match (provider, reason) {
        (LyricsProvider::Musixmatch, _) => (
            LocalIcon::CompactDisc,
            "No Musixmatch lyrics found",
            "Try switching to Genius above.",
            true,
        ),
        (LyricsProvider::Genius, EmptyLyricsReason::NoMatch) => (
            LocalIcon::MagnifyingGlass,
            "No Genius lyrics found",
            "Try switching to Musixmatch above.",
            false,
        ),
        (LyricsProvider::Genius, EmptyLyricsReason::MatchedWithoutLyrics) => (
            LocalIcon::CompactDisc,
            "No Genius Lyrics Found",
            "The matching Genius song entry does not include plain lyrics.",
            false,
        ),
        (LyricsProvider::Genius, EmptyLyricsReason::NotFound) => (
            LocalIcon::MagnifyingGlass,
            "No Genius Lyrics Found",
            "No Genius lyrics were found for this track.",
            false,
        ),
    };
    let mut description_text = StyledText::new(description);
    if emphasize_genius {
        let start = description
            .find("Genius")
            .expect("Musixmatch empty-state copy contains Genius");
        description_text = description_text.with_highlights([(
            start..start + "Genius".len(),
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..HighlightStyle::default()
            },
        )]);
    }

    centered_lyrics_message(icon, heading, description_text)
}

fn loading_lyrics_state() -> AnyElement {
    centered_lyrics_message(
        LocalIcon::MagnifyingGlass,
        "Finding lyrics",
        StyledText::new("Searching for lyrics for this track..."),
    )
}

fn lyrics_body_animation_key(panel: &LyricsPanel) -> String {
    let response = match panel.response.as_ref() {
        None => "none",
        Some(LyricsResponse::Synced { .. }) => "synced",
        Some(LyricsResponse::Plain { .. }) => "plain",
        Some(LyricsResponse::Genius { .. }) => "genius",
        Some(LyricsResponse::Empty { .. }) => "empty",
    };
    let status = match &panel.status {
        Status::Idle => "idle",
        Status::Loading => "loading",
        Status::Empty => "empty",
        Status::Error(_) => "error",
        Status::Ready => "ready",
    };
    format!(
        "lyrics-body-{}-{:?}-{response}-{status}",
        panel.generation, panel.provider
    )
}

const PROVIDER_ICON_OPTICAL_OFFSET_PX: f32 = 1.;

#[allow(clippy::type_complexity)]
fn provider_chip(
    id: &'static str,
    provider: LyricsProvider,
    active: bool,
    icon: LocalIcon,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label = match provider {
        LyricsProvider::Musixmatch => "Musixmatch",
        LyricsProvider::Genius => "Genius",
    };
    let color = if active { FOREGROUND } else { MUTED };
    div()
        .id(id)
        .flex_1()
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(7.))
        .rounded(px(4.))
        .when(active, |this| this.rounded(px(6.)))
        .border_1()
        .border_color(rgba(0x00000000))
        .bg(rgba(0x00000000))
        .text_color(rgb(color))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.text_color(rgb(FOREGROUND)))
        .child(
            div()
                .relative()
                .top(px(PROVIDER_ICON_OPTICAL_OFFSET_PX))
                .child(local_icon(icon, color).size(px(11.))),
        )
        .child(label)
        .on_click(handler)
        .into_any_element()
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let browser_scroll_generation = self.browser_scroll.user_input_generation();
        if browser_scroll_generation != self.last_browser_scroll_generation {
            self.last_browser_scroll_generation = browser_scroll_generation;
            self.mark_user_scrolling(cx);
        }
        self.center_active_line_if_needed(window, cx);
        let provider = self.provider;
        let provider_index = match provider {
            LyricsProvider::Musixmatch => 0,
            LyricsProvider::Genius => 1,
        };
        let provider_visual =
            self.provider_motion
                .prepare(provider_index, 2, Instant::now(), cx.reduce_motion());
        let full_text: Arc<str> = self.full_text.as_str().into();
        let selected_annotation = self.annotation.selected_index();
        let annotation_panel = self.annotation.state().map(|state| {
            annotation::panel(
                state,
                selected_annotation,
                self.annotation.scroll(),
                self.annotation.browser_scroll(),
                annotation_panel_max_height(f32::from(window.viewport_size().height)),
                cx.listener(|this, _, _, cx| {
                    this.annotation.close();
                    cx.notify();
                }),
                cx.listener(|this, _, _, cx| {
                    if this.annotation.select_prev() {
                        cx.notify();
                    }
                }),
                cx.listener(|this, _, _, cx| {
                    if this.annotation.select_next() {
                        cx.notify();
                    }
                }),
            )
        });
        let playing = self.playback.read(cx).state.status == PlaybackStatus::Playing;
        let reduce_motion = cx.reduce_motion();
        let body: Vec<gpui::AnyElement> = match self.response.as_ref() {
            Some(LyricsResponse::Synced { url, .. }) => self
                .lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let line_text = line.text.trim().to_owned();
                    let time = line.time;
                    let active = self.active_line == Some(index);
                    if !is_highlightable_line(&line_text) {
                        return lyric_gap_visualizer(
                            format!("lyrics-line-{index}"),
                            index,
                            playing && active && !reduce_motion,
                            active,
                            reduce_motion,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.seek_to_line(time, cx)))
                        .into_any_element();
                    }
                    let element = lyric_line_container(format!("lyrics-line-{index}"))
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(FOREGROUND)).bg(rgba(0xffffff0a)))
                        .when(active, |this| {
                            this.font_weight(FontWeight::BOLD)
                                .text_color(rgb(FOREGROUND))
                                .bg(rgba(0xffffff14))
                        })
                        .child(line.text.clone())
                        .on_click(cx.listener(move |this, _, _, cx| this.seek_to_line(time, cx)));
                    Self::lyric_menu(
                        element,
                        line_text,
                        lyric_block_text(&self.full_lines, index),
                        full_text.clone(),
                        url.clone(),
                    )
                })
                .collect(),
            Some(LyricsResponse::Plain { url, .. }) => self
                .full_lines
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let line = line.to_owned();
                    let trimmed = line.trim().to_owned();
                    let interactive = is_highlightable_line(&line);
                    let element = lyric_line_container(format!("plain-lyrics-line-{index}"))
                        .when(interactive, |this| {
                            this.hover(|style| {
                                style.text_color(rgb(FOREGROUND)).bg(rgba(0xffffff0a))
                            })
                        })
                        .child(if line.is_empty() {
                            " ".to_owned()
                        } else {
                            line
                        });
                    if !interactive {
                        element.into_any_element()
                    } else {
                        Self::lyric_menu(
                            element,
                            trimmed,
                            lyric_block_text(&self.full_lines, index),
                            full_text.clone(),
                            url.clone(),
                        )
                    }
                })
                .collect(),
            Some(LyricsResponse::Genius { url, .. }) => {
                if let Some(prepared) = self.prepared.as_ref() {
                    prepared
                        .lyric_lines
                        .iter()
                        .enumerate()
                        .map(|(index, line)| {
                            let fragments = genius_line_fragments(prepared, index);
                            let first_annotation = genius_fragment::first_annotation_id(&fragments);
                            let nonempty = !line.trim().is_empty();
                            let row = lyric_line_container(format!("genius-lyrics-line-{index}"))
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .items_baseline()
                                .when(nonempty, |this| {
                                    this.cursor_pointer().hover(|style| {
                                        style.text_color(rgb(FOREGROUND)).bg(rgba(0xffffff0a))
                                    })
                                })
                                .when_some(first_annotation, |this, id| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.show_annotation(id.clone(), cx)
                                    }))
                                })
                                .children(fragments.into_iter().map(|fragment| {
                                    if let Some(id) = fragment.annotation_id {
                                        genius_fragment::render(
                                            id.clone(),
                                            index,
                                            fragment.text,
                                            cx.listener(move |this, _, _, cx| {
                                                this.show_annotation(id.clone(), cx)
                                            }),
                                        )
                                    } else {
                                        genius_fragment::render_plain(fragment.text)
                                            .into_any_element()
                                    }
                                }));
                            if line.trim().is_empty() {
                                row.child(" ").into_any_element()
                            } else {
                                Self::lyric_menu(
                                    row,
                                    line.trim().to_owned(),
                                    lyric_block_text(&self.full_lines, index),
                                    full_text.clone(),
                                    (!url.is_empty()).then(|| url.clone()),
                                )
                            }
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            Some(LyricsResponse::Empty { provider, reason }) => {
                vec![empty_lyrics_state(*provider, reason)]
            }
            _ => {
                if matches!(&self.status, Status::Loading) {
                    vec![loading_lyrics_state()]
                } else {
                    vec![
                        div()
                            .text_color(rgb(match &self.status {
                                Status::Error(_) => DANGER,
                                _ => MUTED,
                            }))
                            .child(match &self.status {
                                Status::Empty => "No lyrics found for this track.".to_owned(),
                                Status::Error(error) => error.clone(),
                                _ => "Play a track to view lyrics.".to_owned(),
                            })
                            .into_any_element(),
                    ]
                }
            }
        };
        let body_animation = if self.response.is_some() {
            crate::motion::content()
        } else {
            crate::motion::quick_content()
        };
        let body_animation_key = lyrics_body_animation_key(self);
        div()
            .id("lyrics-panel")
            .w_full()
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(BACKGROUND))
            .child(
                div()
                    .flex_none()
                    .h(px(101.))
                    .min_h(px(101.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .pb(px(20.))
                    .mb(px(16.))
                    .border_b_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when(self.detached, |this| {
                                this.window_control_area(gpui::WindowControlArea::Drag)
                            })
                            .child(local_icon(LocalIcon::QuoteRight, FOREGROUND).size(px(14.)))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(15.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Lyrics"),
                            )
                            .when(!self.detached, |this| {
                                this.child(crate::music_ui::ghost_icon_button_with_nudge(
                                    "lyrics-detach",
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
                            .child(crate::music_ui::ghost_close_button("lyrics-close", {
                                let playback = self.playback.clone();
                                move |_, _, cx| {
                                    playback.update(cx, |playback, cx| {
                                        if playback.state.right_sidebar_popped().is_some() {
                                            playback.state.dock_sidebar();
                                        } else {
                                            playback.state.close_sidebar();
                                        }
                                        cx.notify();
                                    });
                                }
                            })),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap(px(3.))
                            .p(px(3.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(SURFACE_RAISED))
                            .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_1()
                                    .gap(px(3.))
                                    .child(crate::motion::segmented_selector_indicator(
                                        provider_visual,
                                    ))
                                    .child(provider_chip(
                                        "lyrics-musixmatch",
                                        LyricsProvider::Musixmatch,
                                        provider == LyricsProvider::Musixmatch,
                                        LocalIcon::Music,
                                        cx.listener(|this, _, _, cx| {
                                            this.select(LyricsProvider::Musixmatch, cx)
                                        }),
                                    ))
                                    .child(provider_chip(
                                        "lyrics-genius",
                                        LyricsProvider::Genius,
                                        provider == LyricsProvider::Genius,
                                        LocalIcon::Brain,
                                        cx.listener(|this, _, _, cx| {
                                            this.select(LyricsProvider::Genius, cx)
                                        }),
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .id("lyrics-content-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(browser_scroll_surface(
                        "lyrics-content-browser-scroll",
                        div()
                            .id("lyrics-content-scroll-viewport")
                            .size_full()
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .pr(px(6.))
                            // Breathing room above the player bar. Padding lives
                            // inside the scrollable viewport so the scrollbar
                            // track still matches the viewport height.
                            .pb(px(LYRICS_BOTTOM_PADDING_PX))
                            .flex()
                            .flex_col()
                            .gap(px(14.))
                            .children(body)
                            .with_animation(body_animation_key, body_animation, |this, delta| {
                                this.opacity(crate::motion::lerp(0., 1., delta))
                            })
                            .into_any_element(),
                        BrowserScrollTarget::Handle(self.scroll.clone()),
                        self.browser_scroll.clone(),
                    ))
                    .child(lyrics_vertical_scrollbar(
                        &LyricsScrollbarHandle::new(
                            self.scroll.clone(),
                            self.scrollbar_freeze.clone(),
                        ),
                        if self.detached {
                            f32::INFINITY
                        } else {
                            f32::from(window.viewport_size().width)
                        },
                        cx.listener(|this, hovered, _, cx| {
                            if *hovered {
                                this.scrollbar_freeze.reveal_for_user_scroll();
                                cx.notify();
                            }
                        }),
                    )),
            )
            .when_some(annotation_panel, |this, panel| {
                this.child(
                    div()
                        .relative()
                        .flex_none()
                        .w_full()
                        .child(panel)
                        .with_animation(
                            "genius-annotation-panel-enter",
                            crate::motion::panel(),
                            |this, delta| {
                                this.opacity(crate::motion::lerp(0., 1., delta))
                                    .top(px(crate::motion::lerp(20., 0., delta)))
                            },
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::StatefulInteractiveElement;
    use gpui_component::scroll::ScrollableElement;

    #[test]
    fn equal_track_ids_from_different_services_have_distinct_lyrics() {
        let deezer = PlaybackTrack {
            provider: crate::playback::PlaybackProvider::Deezer,
            id: "42".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(30),
            downloadable: false,
            progressive: false,
            explicit: false,
            ai_generated: false,
            service_url: String::new(),
        };
        let mut soundcloud = deezer.clone();
        soundcloud.provider = crate::playback::PlaybackProvider::SoundCloud;
        let first = LyricsTrackInput::from_playback(&deezer).0;
        let second = LyricsTrackInput::from_playback(&soundcloud).0;
        assert!(track_changed(Some(&first), Some(&second)));
        assert_ne!(first.stable_id, second.stable_id);
        assert_ne!(
            LyricsCacheKey::new(LyricsProvider::Genius, &first),
            LyricsCacheKey::new(LyricsProvider::Genius, &second),
        );
        assert_eq!(
            first.stable_id.as_deref(),
            Some(lyrics_track_identity(&deezer).as_str())
        );
    }

    struct LyricsScrollProbe {
        scroll: ScrollHandle,
        browser_scroll: BrowserScrollState,
    }

    impl Render for LyricsScrollProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .id("lyrics-scroll-probe-frame")
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(browser_scroll_surface(
                        "lyrics-scroll-probe-browser-scroll",
                        div()
                            .id("lyrics-scroll-probe-viewport")
                            .size_full()
                            .track_scroll(&self.scroll)
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .children((0..30).map(|_| div().h(px(50.)).flex_none()))
                            .into_any_element(),
                        BrowserScrollTarget::Handle(self.scroll.clone()),
                        self.browser_scroll.clone(),
                    ))
                    .vertical_scrollbar(&self.scroll),
            )
        }
    }

    #[test]
    fn alternate_provider_switches_between_sources() {
        assert_eq!(
            alternate_provider(LyricsProvider::Musixmatch),
            LyricsProvider::Genius
        );
        assert_eq!(
            alternate_provider(LyricsProvider::Genius),
            LyricsProvider::Musixmatch
        );
    }

    #[test]
    fn automatic_fallback_preserves_primary_empty_when_both_providers_are_empty() {
        let primary = LyricsResponse::Empty {
            provider: LyricsProvider::Musixmatch,
            reason: EmptyLyricsReason::NotFound,
        };
        let alternate = LyricsResponse::Empty {
            provider: LyricsProvider::Genius,
            reason: EmptyLyricsReason::NoMatch,
        };

        assert_eq!(
            automatic_fallback_selection(
                LyricsProvider::Musixmatch,
                Some(primary.clone()),
                alternate,
            ),
            (LyricsProvider::Musixmatch, primary)
        );
    }

    #[test]
    fn automatic_fallback_switches_only_when_the_alternate_has_lyrics() {
        let alternate = LyricsResponse::Plain {
            text: "lyrics".into(),
            url: None,
        };
        let selected = automatic_fallback_selection(
            LyricsProvider::Musixmatch,
            Some(LyricsResponse::Empty {
                provider: LyricsProvider::Musixmatch,
                reason: EmptyLyricsReason::NotFound,
            }),
            alternate.clone(),
        );
        assert_eq!(selected, (LyricsProvider::Genius, alternate));

        let alternate_empty = LyricsResponse::Empty {
            provider: LyricsProvider::Genius,
            reason: EmptyLyricsReason::NoMatch,
        };
        assert_eq!(
            automatic_fallback_selection(LyricsProvider::Musixmatch, None, alternate_empty.clone(),),
            (LyricsProvider::Genius, alternate_empty)
        );
    }

    #[test]
    fn late_primary_lyrics_replace_a_fast_alternate_but_empty_results_do_not() {
        let primary = LyricsResponse::Plain {
            text: "preferred".into(),
            url: None,
        };
        assert_eq!(
            late_primary_selection(
                LyricsProvider::Musixmatch,
                LyricsProvider::Musixmatch,
                primary.clone(),
            ),
            Some((LyricsProvider::Musixmatch, primary))
        );
        assert_eq!(
            late_primary_selection(
                LyricsProvider::Musixmatch,
                LyricsProvider::Genius,
                LyricsResponse::Plain {
                    text: "alternate".into(),
                    url: None,
                },
            ),
            None
        );
        assert_eq!(
            late_primary_selection(
                LyricsProvider::Musixmatch,
                LyricsProvider::Musixmatch,
                LyricsResponse::Empty {
                    provider: LyricsProvider::Musixmatch,
                    reason: EmptyLyricsReason::NotFound,
                },
            ),
            None
        );
    }

    #[test]
    fn background_track_sync_loads_only_when_the_track_changes() {
        let first = LyricsTrack {
            stable_id: Some("first".into()),
            ..Default::default()
        };
        let second = LyricsTrack {
            stable_id: Some("second".into()),
            ..Default::default()
        };

        assert!(track_changed(None, Some(&first)));
        assert!(!track_changed(Some(&first), Some(&first)));
        assert!(track_changed(Some(&first), Some(&second)));
        assert!(track_changed(Some(&first), None));
    }

    #[test]
    fn tokio_lyrics_task_can_be_spawned_without_entering_runtime_context() {
        let runtime = Runtime::new().unwrap();
        let task = runtime.spawn(async { Err::<(), _>("Lyrics request cancelled".to_owned()) });

        assert_eq!(
            runtime.block_on(lyrics_task_result(task)),
            Err("Lyrics request cancelled".to_owned())
        );
    }

    #[test]
    fn user_scroll_suppression_matches_scrollbar_visibility_cutoff() {
        assert_eq!(USER_SCROLL_SUPPRESSION, Duration::from_millis(3000));
    }

    #[test]
    fn lyrics_scrollbar_moves_into_the_desktop_gutter_only() {
        assert_eq!(lyrics_scrollbar_outset(crate::music_ui::MOBILE_MAX), 0.);
        assert_eq!(
            lyrics_scrollbar_outset(crate::music_ui::MOBILE_MAX + 1.),
            24.
        );
        assert_eq!(PROVIDER_ICON_OPTICAL_OFFSET_PX, 1.);
    }

    #[test]
    fn annotation_height_uses_the_requested_viewport_ratio_and_bounds() {
        assert_eq!(annotation_panel_max_height(100.), 0.);
        assert_eq!(annotation_panel_max_height(200.), 63.);
        assert_eq!(annotation_panel_max_height(500.), 260.);
        assert_eq!(annotation_panel_max_height(1000.), 360.);
        for viewport_height in [140., 200., 500., 1000.] {
            let annotation_height = annotation_panel_max_height(viewport_height);
            assert!(
                LYRICS_HEADER_BLOCK_HEIGHT + annotation_height + MIN_LYRICS_VIEWPORT_HEIGHT
                    <= viewport_height
            );
        }
    }

    #[gpui::test]
    fn lyrics_scroll_handle_reports_overflow_for_direct_line_children(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_component::init(cx);
            crate::theme::configure_component_theme(cx);
        });
        let scroll = ScrollHandle::new();
        let window = cx.add_window({
            let scroll = scroll.clone();
            move |_, _| LyricsScrollProbe {
                scroll,
                browser_scroll: BrowserScrollState::new(),
            }
        });
        let mut cx = gpui::VisualTestContext::from_window(gpui::AnyWindowHandle::from(window), cx);
        cx.run_until_parked();
        cx.update(|window, cx| {
            _ = window.draw(cx);
        });

        assert!(scroll.max_offset().y > px(0.));
        let first = scroll
            .bounds_for_item(0)
            .expect("first line remains a direct scroll child");
        let second = scroll
            .bounds_for_item(1)
            .expect("second line remains a direct scroll child");
        assert_eq!(first.size.height, px(50.));
        assert!(second.top() > first.top());
        let viewport = scroll.bounds();
        let child = scroll
            .bounds_for_item(25)
            .expect("line remains a direct scroll child");
        let centered = centered_scroll_offset(
            f32::from(viewport.top()),
            f32::from(viewport.size.height),
            f32::from(child.top()),
            f32::from(child.size.height),
            f32::from(scroll.max_offset().y),
        );
        assert!(centered < 0.);

        cx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            _ = window.draw(cx);
        });
        assert!(scroll.offset().y < px(0.));
    }
}
