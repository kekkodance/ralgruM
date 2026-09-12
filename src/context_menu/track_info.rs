use std::{
    collections::HashMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext, Context, Entity, FontWeight, IntoElement, Render, Window, div, prelude::*, px,
    rgb,
};
use gpui_component::{Sizable, Size, spinner::Spinner};
use tokio::sync::Semaphore;

use crate::{
    assets::{LocalIcon, local_icon},
    playback::{
        PlaybackModel, PlaybackProvider, PlaybackTrack, ResolvedTrackInfo, describe_track_info,
    },
    theme::{MUTED, PRIMARY},
};

use super::PopupMenuItem;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TrackInfoCacheKey {
    provider: PlaybackProvider,
    track_id: String,
    credential_generation: u128,
    cache_revision: u64,
}

impl TrackInfoCacheKey {
    fn new(
        provider: PlaybackProvider,
        track_id: &str,
        credential_generation: u128,
        cache_revision: u64,
    ) -> Self {
        Self {
            provider,
            track_id: track_id.to_owned(),
            credential_generation,
            cache_revision,
        }
    }
}

static TRACK_INFO_CACHE: LazyLock<Mutex<HashMap<TrackInfoCacheKey, ResolvedTrackInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static TRACK_INFO_PROBE_EPOCH: AtomicU64 = AtomicU64::new(0);
static TRACK_INFO_PROBE_GATE: Semaphore = Semaphore::const_new(1);
static TRACK_INFO_LAST_PROBE: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));
const TRACK_INFO_PROBE_DEBOUNCE: Duration = Duration::from_millis(180);
const TRACK_INFO_PROBE_INTERVAL: Duration = Duration::from_secs(1);

fn cached_track_info(key: &TrackInfoCacheKey) -> Option<ResolvedTrackInfo> {
    let cache = TRACK_INFO_CACHE.lock().ok()?;
    cache.get(key).copied()
}

fn cache_track_info(key: TrackInfoCacheKey, info: ResolvedTrackInfo) {
    if let Ok(mut cache) = TRACK_INFO_CACHE.lock() {
        cache.insert(key, info);
    }
}
/// Header row every track menu opens with: computed size and bitrate, with a
/// checking state while the values resolve asynchronously. Mirrors
/// createTrackContextInfo and updateTrackContextAudioInfo in the original
/// app. The resolve reuses the playback source resolution read-only, so no
/// stream bytes are downloaded or cached.
pub(crate) struct TrackInfoState {
    status: InfoStatus,
}

enum InfoStatus {
    Checking { secondary: String },
    Resolved { primary: String, secondary: String },
}

const UNAVAILABLE_PRIMARY: &str = "Size unavailable · Bitrate unavailable";
const DEFAULT_SECONDARY: &str = "Audio information";

impl TrackInfoState {
    fn checking() -> Self {
        Self {
            status: InfoStatus::Checking {
                secondary: DEFAULT_SECONDARY.to_owned(),
            },
        }
    }

    fn checking_with_source(source: Option<String>) -> Self {
        Self {
            status: InfoStatus::Checking {
                secondary: source
                    .map(|source| format!("Playing source · {source}"))
                    .unwrap_or_else(|| DEFAULT_SECONDARY.to_owned()),
            },
        }
    }

    fn resolved_now(primary: String, secondary: String) -> Self {
        Self {
            status: InfoStatus::Resolved { primary, secondary },
        }
    }

    fn apply(
        &mut self,
        result: Result<ResolvedTrackInfo, String>,
        duration: Duration,
    ) -> Option<(String, String)> {
        let (primary, secondary) = match result {
            Ok(info) => (
                describe_track_info(info, duration),
                display_audio_format(info.format),
            ),
            Err(_) => (UNAVAILABLE_PRIMARY.to_owned(), DEFAULT_SECONDARY.to_owned()),
        };
        self.status = InfoStatus::Resolved {
            primary: primary.clone(),
            secondary: secondary.clone(),
        };
        Some((primary, secondary))
    }
}

impl Render for TrackInfoState {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (primary, secondary, checking) = match &self.status {
            InfoStatus::Checking { secondary } => (
                "Checking size and bitrate…".to_owned(),
                secondary.clone(),
                true,
            ),
            InfoStatus::Resolved { primary, secondary } => {
                (primary.clone(), secondary.clone(), false)
            }
        };
        let glyph = local_icon(LocalIcon::CircleInfo, PRIMARY).size(px(13.));
        div()
            .id("track-context-info")
            .flex_1()
            .min_w_0()
            .min_h(px(44.))
            .px(px(8.))
            .py(px(6.))
            .flex()
            .items_center()
            .gap(px(9.))
            .child(div().w(px(20.)).flex().justify_center().child(if checking {
                Spinner::new()
                    .icon(crate::assets::widget_icon(LocalIcon::Spinner))
                    .with_size(Size::Size(px(13.)))
                    .color(rgb(MUTED).into())
                    .ease(|delta| delta)
                    .into_any_element()
            } else {
                glyph.into_any_element()
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .text_color(rgb(0xf4f4f5))
                            .child(primary),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(rgb(MUTED))
                            .truncate()
                            .child(secondary),
                    ),
            )
    }
}

/// Builds the inert header item for a track menu. Resolution runs in the
/// background and leaves the checking state visible until the probe finishes.
pub(super) fn track_info_item(
    playback: &Entity<PlaybackModel>,
    track: &PlaybackTrack,
    cx: &mut App,
) -> PopupMenuItem {
    let state = cx.new(|_| TrackInfoState::checking());
    let request_epoch = TRACK_INFO_PROBE_EPOCH
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1);
    let (current_info, playing_quality, has_probe, credential_generation, cache_revision) = {
        let model = playback.read(cx);
        let is_current = model
            .state
            .current()
            .is_some_and(|current| current.provider == track.provider && current.id == track.id);
        let current_info = is_current
            .then(|| model.current_track_info(track))
            .flatten();
        let playing_quality = is_current
            .then(|| model.resolved_quality().map(str::to_owned))
            .flatten();
        let has_probe = model.track_info_probe(cx).is_some();
        let credential_generation = model.track_info_credential_generation(cx);
        let cache_revision = model.track_info_cache_revision();
        (
            current_info,
            playing_quality,
            has_probe,
            credential_generation,
            cache_revision,
        )
    };
    if let Some(info) = current_info {
        state.update(cx, |state, _| {
            *state = TrackInfoState::resolved_now(
                describe_track_info(info, track.duration),
                display_audio_format(info.format),
            );
        });
        return element_item(state);
    }
    if playing_quality.is_some() {
        state.update(cx, |state, _| {
            *state = TrackInfoState::checking_with_source(playing_quality.clone());
        });
    }
    let cache_key = TrackInfoCacheKey::new(
        track.provider,
        &track.id,
        credential_generation,
        cache_revision,
    );
    if let Some(info) = cached_track_info(&cache_key)
        && info.is_useful(track.duration)
    {
        state.update(cx, |state, _| {
            *state = TrackInfoState::resolved_now(
                describe_track_info(info, track.duration),
                display_audio_format(info.format),
            );
        });
        return element_item(state);
    }
    if !has_probe {
        state.update(cx, |state, cx| {
            state.apply(
                Err("The playback source resolver is unavailable".to_owned()),
                track.duration,
            );
            cx.notify();
        });
        return element_item(state);
    }
    let duration = track.duration;
    let request_track = track.clone();
    let request_playback = playback.clone();
    let task_state = state.clone();
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        executor.timer(TRACK_INFO_PROBE_DEBOUNCE).await;
        if TRACK_INFO_PROBE_EPOCH.load(Ordering::Relaxed) != request_epoch {
            return;
        }
        let Ok(_permit) = TRACK_INFO_PROBE_GATE.acquire().await else {
            return;
        };
        if TRACK_INFO_PROBE_EPOCH.load(Ordering::Relaxed) != request_epoch {
            return;
        }
        let wait = TRACK_INFO_LAST_PROBE
            .lock()
            .ok()
            .and_then(|last| *last)
            .and_then(|last| TRACK_INFO_PROBE_INTERVAL.checked_sub(last.elapsed()));
        if let Some(wait) = wait {
            executor.timer(wait).await;
        }
        if TRACK_INFO_PROBE_EPOCH.load(Ordering::Relaxed) != request_epoch {
            return;
        }
        if let Ok(mut last) = TRACK_INFO_LAST_PROBE.lock() {
            *last = Some(Instant::now());
        }
        let result = match request_playback.read_with(cx, |model, app| model.track_info_probe(app))
        {
            Some(probe) => match probe.spawn(&request_track).await {
                Ok(result) => result,
                Err(_) => Err("The playback worker stopped unexpectedly".to_owned()),
            },
            None => Err("The playback source resolver is unavailable".to_owned()),
        };
        task_state.update(cx, |state, cx| {
            let cache_info = result.as_ref().ok().copied();
            let cacheable = cache_info
                .as_ref()
                .is_some_and(|info| info.is_useful(duration));
            state.apply(result, duration);
            if cacheable && let Some(info) = cache_info {
                cache_track_info(cache_key, info);
            }
            cx.notify();
        });
    })
    .detach();
    element_item(state)
}

fn element_item(state: Entity<TrackInfoState>) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| state.clone())
        .disabled(true)
        .disabled_visual(false)
}

fn display_audio_format(format: crate::playback::AudioFormat) -> String {
    format.label().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::AudioFormat;

    #[test]
    fn track_info_cache_key_changes_with_credential_generation() {
        let first = TrackInfoCacheKey::new(PlaybackProvider::SoundCloud, "track", 1, 1);
        let second = TrackInfoCacheKey::new(PlaybackProvider::SoundCloud, "track", 2, 1);

        assert_ne!(first, second);
    }

    #[test]
    fn track_info_cache_key_changes_with_audio_cache_revision() {
        let first = TrackInfoCacheKey::new(PlaybackProvider::SoundCloud, "track", 1, 1);
        let second = TrackInfoCacheKey::new(PlaybackProvider::SoundCloud, "track", 1, 2);

        assert_ne!(first, second);
    }

    #[test]
    fn failed_probe_finishes_with_unavailable_info() {
        let mut state = TrackInfoState::checking_with_source(Some("FLAC".to_owned()));

        assert_eq!(
            state.apply(Err("resolver failed".to_owned()), Duration::from_secs(240)),
            Some((UNAVAILABLE_PRIMARY.to_owned(), DEFAULT_SECONDARY.to_owned()))
        );
        assert!(matches!(state.status, InfoStatus::Resolved { .. }));
    }

    #[test]
    fn zero_sized_probe_finishes_with_known_format() {
        let mut state = TrackInfoState::checking();
        let info = ResolvedTrackInfo {
            format: AudioFormat::Flac,
            bytes: 0,
            timeline_size_unknown: false,
            declared_bitrate: None,
        };

        assert_eq!(
            state.apply(Ok(info), Duration::from_secs(240)),
            Some((UNAVAILABLE_PRIMARY.to_owned(), "FLAC".to_owned()))
        );
    }

    #[test]
    fn checking_state_keeps_the_loading_copy_until_resolution() {
        let state = TrackInfoState::checking();

        assert!(matches!(
            state.status,
            InfoStatus::Checking { ref secondary } if secondary == DEFAULT_SECONDARY
        ));
    }

    #[test]
    fn gpui_probe_task_uses_gpui_executor_timers() {
        let production = include_str!("track_info.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("track-info production source");

        assert!(!production.contains("tokio::time::sleep"));
        assert!(production.contains("let executor = cx.background_executor().clone();"));
        assert!(production.contains("executor.timer(TRACK_INFO_PROBE_DEBOUNCE).await;"));
        assert!(production.contains("executor.timer(wait).await;"));
    }

    #[gpui::test]
    fn track_info_item_is_inert_without_disabled_opacity(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let state = cx.new(|_| TrackInfoState::checking());
            let item = element_item(state);

            match item {
                PopupMenuItem::ElementItem {
                    disabled,
                    render_disabled,
                    ..
                } => {
                    assert!(disabled);
                    assert!(!render_disabled);
                }
                _ => panic!("expected an element menu item"),
            }
        });
    }
}
