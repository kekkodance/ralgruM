// Port of the original app's playing-row marker (public/js/ui/playing-rows.js
// and the .row-playing-bars styles). The row whose track is current swaps its
// index number for equalizer bars and colors its title with the accent color.
// Bars animate only while playback is active and rest at their base height in
// every other state, which covers both a manual pause and the gap while a
// freshly selected track resolves.

use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Div, ElementId, div, ease_in_out, prelude::*,
    px, rgb,
};

use crate::{
    motion::{CONTENT_DURATION, lerp},
    playback::{PlaybackProvider, PlaybackState, PlaybackStatus, PlaybackTrack},
    theme::{MUTED, PRIMARY},
};

/// What a single rendered row should show for the track that is current.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum RowPlaying {
    #[default]
    Other,
    Current {
        /// False while paused or still resolving, matching the original's
        /// settled bars for every state other than actively playing.
        animating: bool,
    },
}

impl RowPlaying {
    pub(crate) const fn is_current(self) -> bool {
        matches!(self, Self::Current { .. })
    }
}

/// Small view-side digest of playback state. Views keep one and re-render
/// their rows only when it changes, so the 250 ms playback position poll
/// never repaints the lists.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PlayingSnapshot {
    current: Option<(PlaybackProvider, String)>,
    current_index: Option<usize>,
    queue: Vec<(PlaybackProvider, String)>,
    playing: bool,
    skip_explicit: bool,
}

impl PlayingSnapshot {
    pub(crate) fn from_playback(state: &PlaybackState) -> Self {
        Self {
            current: state
                .current()
                .map(|track| (track.provider, track.id.clone())),
            current_index: state.current_index,
            queue: state
                .queue
                .iter()
                .map(|track| (track.provider, track.id.clone()))
                .collect(),
            playing: state.status == PlaybackStatus::Playing,
            skip_explicit: state.skip_explicit,
        }
    }

    /// Resolve queue correspondence once before rendering any rows.
    pub(crate) fn for_queue(&self, queue: &[PlaybackTrack]) -> QueuePlayingSnapshot {
        let queue_matches = queue.len() == self.queue.len()
            && queue
                .iter()
                .zip(&self.queue)
                .all(|(track, (provider, id))| track.provider == *provider && track.id == *id);
        let current_index = if queue_matches {
            self.current_index
        } else {
            queue.iter().position(|track| {
                self.current
                    .as_ref()
                    .is_some_and(|(provider, id)| track.provider == *provider && track.id == *id)
            })
        };
        QueuePlayingSnapshot {
            current_index,
            playing: self.playing,
            skip_explicit: self.skip_explicit,
        }
    }

    pub(crate) fn blocks(&self, track: &PlaybackTrack) -> bool {
        self.skip_explicit && track.explicit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QueuePlayingSnapshot {
    current_index: Option<usize>,
    playing: bool,
    skip_explicit: bool,
}

impl QueuePlayingSnapshot {
    pub(crate) fn row(&self, index: usize) -> RowPlaying {
        if self.current_index == Some(index) {
            RowPlaying::Current {
                animating: self.playing,
            }
        } else {
            RowPlaying::Other
        }
    }

    pub(crate) fn blocks(&self, track: &PlaybackTrack) -> bool {
        self.skip_explicit && track.explicit
    }
}

pub(crate) const INDEX_COLUMN_WIDTH: f32 = 28.;
const INDEX_TEXT_SIZE: f32 = 12.5;
const BAR_WIDTH: f32 = 3.;
const BAR_GAP: f32 = 2.;
const BARS_HEIGHT: f32 = 14.;
const BAR_BASE_HEIGHT: f32 = 4.;
const BOUNCE_PERIOD_MS: f32 = 1500.;
const MIX_VISIBLE: f32 = 0.001;
const MAX_BLEND_ENTRIES: usize = 32;
// Positive delays, as in the original keyframes: a fresh set of bars rests
// flat and each bar only starts rising once its delay has elapsed.
const BAR_DELAYS_MS: [f32; 3] = [0., 320., 620.];
// Keyframe pairs from the original row-playing-bounce animation.
const BOUNCE_KEYFRAMES: [(f32, f32); 5] =
    [(0.00, 4.), (0.30, 14.), (0.55, 7.), (0.78, 12.), (1.00, 4.)];

type BlendKey = (&'static str, usize);

thread_local! {
    static BLENDS: RefCell<HashMap<BlendKey, Blend>> = RefCell::new(HashMap::new());
}

struct Blend {
    from_amp: f32,
    to_amp: f32,
    from_color: u32,
    to_color: u32,
    started_at: Instant,
    last_seen: Instant,
    epoch: u64,
}

impl Blend {
    fn displayed(&self, now: Instant) -> (f32, u32) {
        let t = blend_unit(now.saturating_duration_since(self.started_at));
        (
            lerp(self.from_amp, self.to_amp, t),
            lerp_rgb(self.from_color, self.to_color, t),
        )
    }

    fn in_progress(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started_at) < CONTENT_DURATION
    }
}

fn blend_unit(elapsed: Duration) -> f32 {
    let duration = CONTENT_DURATION.as_secs_f32();
    if duration <= 0. {
        1.
    } else {
        ease_in_out((elapsed.as_secs_f32() / duration).clamp(0., 1.))
    }
}

fn lerp_rgb(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let start = ((from >> shift) & 0xff) as f32;
        let end = ((to >> shift) & 0xff) as f32;
        (lerp(start, end, amount)).round() as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn sync_blend(key: BlendKey, amp: f32, color: u32, reduce_motion: bool) -> (f32, u32, u64, bool) {
    sync_blend_at(key, amp, color, reduce_motion, Instant::now())
}

fn sync_blend_at(
    key: BlendKey,
    amp: f32,
    color: u32,
    reduce_motion: bool,
    now: Instant,
) -> (f32, u32, u64, bool) {
    BLENDS.with(|blends| {
        let mut blends = blends.borrow_mut();
        blends.retain(|_, blend| blend.to_amp > MIX_VISIBLE || blend.in_progress(now));
        if amp <= MIX_VISIBLE && !blends.contains_key(&key) {
            return (amp, color, 0, false);
        }
        let mut remove_after_read = false;
        let result = {
            let blend = blends.entry(key).or_insert_with(|| Blend {
                from_amp: amp,
                to_amp: amp,
                from_color: color,
                to_color: color,
                started_at: now.checked_sub(CONTENT_DURATION).unwrap_or(now),
                last_seen: now,
                epoch: 0,
            });
            blend.last_seen = now;
            if reduce_motion {
                blend.from_amp = amp;
                blend.to_amp = amp;
                blend.from_color = color;
                blend.to_color = color;
                blend.started_at = now.checked_sub(CONTENT_DURATION).unwrap_or(now);
                remove_after_read = amp <= MIX_VISIBLE;
                (amp, color, blend.epoch, false)
            } else {
                if (blend.to_amp - amp).abs() > f32::EPSILON || blend.to_color != color {
                    let (live_amp, live_color) = blend.displayed(now);
                    blend.from_amp = live_amp;
                    blend.from_color = live_color;
                    blend.to_amp = amp;
                    blend.to_color = color;
                    blend.started_at = now;
                    blend.epoch = blend.epoch.wrapping_add(1);
                }
                let (displayed_amp, displayed_color) = blend.displayed(now);
                let in_progress = blend.in_progress(now);
                if amp <= MIX_VISIBLE && !in_progress {
                    remove_after_read = true;
                }
                (displayed_amp, displayed_color, blend.epoch, in_progress)
            }
        };
        if remove_after_read {
            blends.remove(&key);
        }
        while blends.len() > MAX_BLEND_ENTRIES {
            let Some(oldest) = blends
                .iter()
                .filter(|(candidate, _)| **candidate != key)
                .min_by(|(left_key, left), (right_key, right)| {
                    left.last_seen
                        .cmp(&right.last_seen)
                        .then_with(|| left.started_at.cmp(&right.started_at))
                        .then_with(|| left_key.0.cmp(right_key.0))
                        .then_with(|| left_key.1.cmp(&right_key.1))
                })
                .map(|(candidate, _)| *candidate)
            else {
                break;
            };
            blends.remove(&oldest);
        }
        result
    })
}

/// The index column of a track row. The number gives way to the bars without
/// changing the column width, so playback moving between rows never reflows
/// the list.
pub(crate) fn index_slot(index: usize, playing: RowPlaying, cx: &App) -> AnyElement {
    let column = div()
        .w(px(INDEX_COLUMN_WIDTH))
        .flex_none()
        .flex()
        .relative()
        .justify_center()
        .items_center()
        .text_size(px(INDEX_TEXT_SIZE));
    if cx.reduce_motion() {
        return if playing.is_current() {
            column
                .text_color(rgb(PRIMARY))
                .child((index + 1).to_string())
                .into_any_element()
        } else {
            column
                .text_color(rgb(MUTED))
                .child((index + 1).to_string())
                .into_any_element()
        };
    }

    let current = playing.is_current();
    let (mix, _, epoch, in_progress) = sync_blend(
        ("track-index-mix", index),
        if current { 1. } else { 0. },
        0,
        false,
    );
    let animating = matches!(playing, RowPlaying::Current { animating: true });
    let slot = column
        .child(
            div()
                .opacity(1. - mix)
                .text_color(rgb(MUTED))
                .child((index + 1).to_string()),
        )
        .when(mix > MIX_VISIBLE, |this| {
            this.child(div().absolute().opacity(mix).child(playing_bars(
                ("track-index-bars", index),
                animating,
                PRIMARY,
                false,
            )))
        });
    if in_progress {
        slot.with_animation(
            ElementId::named_usize(format!("track-index-mix-{index}"), epoch as usize),
            crate::motion::content(),
            |this, _delta| this,
        )
        .into_any_element()
    } else {
        slot.into_any_element()
    }
}

pub(crate) fn playing_bars(
    key: (&'static str, usize),
    animating: bool,
    color: u32,
    reduce_motion: bool,
) -> AnyElement {
    let target_amp = if animating { 1. } else { 0. };
    let (amp, color, _, in_progress) = sync_blend(key, target_amp, color, reduce_motion);
    let group = div()
        .h(px(BARS_HEIGHT))
        .flex_none()
        .flex()
        .items_end()
        .justify_center()
        .gap(px(BAR_GAP))
        // The three bars and two gaps total an odd number of pixels; the
        // original trims one off the box so the group starts on a whole
        // pixel and all bars land on one.
        .pr(px(1.));
    if amp <= MIX_VISIBLE && !in_progress {
        return group
            .children(BAR_DELAYS_MS.map(|_| bar(BAR_BASE_HEIGHT, color)))
            .into_any_element();
    }
    group
        .with_animation(
            key,
            Animation::new(Duration::from_millis(BOUNCE_PERIOD_MS as u64)).repeat_synced(),
            move |group, delta| {
                group.children(BAR_DELAYS_MS.map(|delay| {
                    let bounce = delayed_height(delta, delay / BOUNCE_PERIOD_MS);
                    bar(lerp(BAR_BASE_HEIGHT, bounce, amp), color)
                }))
            },
        )
        .into_any_element()
}

/// A bar is drawn at its animated height rather than a vertical scale, so the
/// pill cap keeps its roundness at every height, like the original.
fn bar(height: f32, color: u32) -> Div {
    div()
        .w(px(BAR_WIDTH))
        .h(px(height))
        .flex_none()
        .rounded(px(2.))
        .bg(rgb(color))
}

/// Height of one bar at animation progress `delta` (0..1) given its phase
/// offset as a fraction of the period. Wrapping the offset keeps the repeated
/// animation continuous when GPUI resets delta at the end of each cycle.
fn delayed_height(delta: f32, delay_fraction: f32) -> f32 {
    bounce_height((delta - delay_fraction).rem_euclid(1.0))
}

/// Piecewise interpolation of the original keyframes, eased between each pair
/// to stand in for the CSS ease-in-out timing per segment.
fn bounce_height(phase: f32) -> f32 {
    let phase = phase.clamp(0., 1.);
    for pair in BOUNCE_KEYFRAMES.windows(2) {
        let (start, start_height) = pair[0];
        let (end, end_height) = pair[1];
        if phase <= end {
            let span = (end - start).max(f32::EPSILON);
            let local = ((phase - start) / span).clamp(0., 1.);
            let eased = local * local * (3. - 2. * local);
            return start_height + (end_height - start_height) * eased;
        }
    }
    BOUNCE_KEYFRAMES[BOUNCE_KEYFRAMES.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::{
        BAR_BASE_HEIGHT, BLENDS, MAX_BLEND_ENTRIES, PlayingSnapshot, RowPlaying, blend_unit,
        bounce_height, delayed_height, lerp_rgb, sync_blend_at,
    };
    use crate::{
        motion::CONTENT_DURATION,
        playback::{PlaybackProvider, PlaybackState, PlaybackStatus, PlaybackTrack},
        theme::{MUTED, PRIMARY},
    };
    use std::time::{Duration, Instant};

    fn track(provider: PlaybackProvider, id: &str, explicit: bool) -> PlaybackTrack {
        PlaybackTrack {
            provider,
            id: id.into(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::from_secs(10),
            downloadable: false,
            progressive: false,
            explicit,
            service_url: String::new(),
        }
    }

    #[test]
    fn lerp_rgb_hits_endpoints_and_midpoint() {
        assert_eq!(lerp_rgb(MUTED, PRIMARY, 0.), MUTED);
        assert_eq!(lerp_rgb(MUTED, PRIMARY, 1.), PRIMARY);
        let mid = lerp_rgb(0x000000, 0x00ff00, 0.5);
        assert_eq!(mid, 0x008000);
    }

    #[test]
    fn blend_unit_eases_from_start_to_end() {
        assert_eq!(blend_unit(Duration::ZERO), 0.);
        assert_eq!(blend_unit(CONTENT_DURATION), 1.);
        assert_eq!(blend_unit(CONTENT_DURATION + Duration::from_millis(50)), 1.);
        let half = blend_unit(CONTENT_DURATION / 2);
        assert!(half > 0.4 && half < 0.6);
    }

    #[test]
    fn bounce_hits_every_original_keyframe_height() {
        assert_eq!(bounce_height(0.), 4.);
        assert_eq!(bounce_height(0.30), 14.);
        assert_eq!(bounce_height(0.55), 7.);
        assert_eq!(bounce_height(0.78), 12.);
        assert_eq!(bounce_height(1.), 4.);
    }

    #[test]
    fn bounce_interpolates_inside_and_clamps_outside_the_cycle() {
        let rising = bounce_height(0.15);
        assert!(rising > 4. && rising < 14.);
        assert_eq!(bounce_height(-0.5), 4.);
        assert_eq!(bounce_height(1.5), 4.);
    }

    #[test]
    fn delayed_bars_wrap_their_phase_at_the_repeat_boundary() {
        let second_delay = 320. / 1500.;
        assert_eq!(
            delayed_height(0., second_delay),
            bounce_height((0. - second_delay).rem_euclid(1.))
        );
        assert_eq!(
            delayed_height(1., second_delay),
            delayed_height(0., second_delay)
        );
        // The first bar has no phase offset and starts rising immediately.
        assert!(delayed_height(0.01, 0.) > BAR_BASE_HEIGHT);
        // The offset reaches the start of the curve at its phase boundary.
        assert_eq!(
            delayed_height(second_delay, second_delay),
            bounce_height(0.)
        );
    }

    #[test]
    fn inactive_indices_do_not_accumulate_blends() {
        BLENDS.with(|blends| blends.borrow_mut().clear());
        let now = Instant::now();
        for index in 0..5_000 {
            let _ = sync_blend_at(("test-inactive", index), 0., 0, false, now);
        }

        assert_eq!(BLENDS.with(|blends| blends.borrow().len()), 0);
    }

    #[test]
    fn positive_blends_are_bounded_and_current_key_is_protected() {
        BLENDS.with(|blends| blends.borrow_mut().clear());
        let now = Instant::now();
        let protected = ("test-positive", 1_000);
        for index in 0..1_000 {
            let _ = sync_blend_at(
                ("test-positive", index),
                1.,
                0,
                false,
                now + Duration::from_millis(index as u64),
            );
        }

        let _ = sync_blend_at(protected, 1., 0, false, now + Duration::from_secs(2));
        BLENDS.with(|blends| {
            let blends = blends.borrow();
            assert!(blends.len() <= MAX_BLEND_ENTRIES);
            assert!(blends.contains_key(&protected));
        });
    }

    #[test]
    fn current_to_inactive_transition_is_retained_then_pruned() {
        BLENDS.with(|blends| blends.borrow_mut().clear());
        let key = ("test-transition", 7);
        let start = Instant::now();

        let (_, _, _, in_progress) = sync_blend_at(key, 1., 0, false, start);
        assert!(!in_progress);
        assert_eq!(BLENDS.with(|blends| blends.borrow().len()), 1);

        let (mid_amp, _, _, in_progress) =
            sync_blend_at(key, 0., 0, false, start + Duration::from_millis(1));
        assert!(in_progress);
        assert!(mid_amp > 0.);
        assert_eq!(BLENDS.with(|blends| blends.borrow().len()), 1);

        let (settled_amp, _, _, in_progress) = sync_blend_at(
            key,
            0.,
            0,
            false,
            start + CONTENT_DURATION + Duration::from_millis(1),
        );
        assert_eq!(settled_amp, 0.);
        assert!(!in_progress);
        assert_eq!(BLENDS.with(|blends| blends.borrow().len()), 0);
    }

    #[test]
    fn snapshot_marks_rows_by_provider_and_id_and_follows_status() {
        let mut state = PlaybackState::default();
        let deezer = track(PlaybackProvider::Deezer, "42", false);
        let other_provider = track(PlaybackProvider::SoundCloud, "42", false);
        let other_id = track(PlaybackProvider::Deezer, "43", false);
        state.replace(vec![deezer.clone()], 0);
        let snapshot = PlayingSnapshot::from_playback(&state);

        assert_eq!(
            snapshot.for_queue(std::slice::from_ref(&deezer)).row(0),
            RowPlaying::Current { animating: false }
        );
        assert_eq!(
            snapshot
                .for_queue(std::slice::from_ref(&other_provider))
                .row(0),
            RowPlaying::Other
        );
        assert_eq!(
            snapshot.for_queue(std::slice::from_ref(&other_id)).row(0),
            RowPlaying::Other
        );

        state.loaded(state.generation, None);
        let snapshot = PlayingSnapshot::from_playback(&state);
        assert_eq!(
            snapshot.for_queue(std::slice::from_ref(&deezer)).row(0),
            RowPlaying::Current { animating: true }
        );

        state.toggle();
        let snapshot = PlayingSnapshot::from_playback(&state);
        assert_eq!(
            snapshot.for_queue(std::slice::from_ref(&deezer)).row(0),
            RowPlaying::Current { animating: false }
        );
    }

    #[test]
    fn duplicate_tracks_mark_only_the_current_queue_occurrence() {
        let mut state = PlaybackState::default();
        let duplicate = track(PlaybackProvider::Deezer, "42", false);
        let queue = vec![duplicate.clone(), duplicate.clone()];
        state.replace(queue.clone(), 1);
        let snapshot = PlayingSnapshot::from_playback(&state);

        assert_eq!(snapshot.for_queue(&queue).row(0), RowPlaying::Other);
        assert_eq!(
            snapshot.for_queue(&queue).row(1),
            RowPlaying::Current { animating: false }
        );
    }

    #[test]
    fn queue_snapshot_matches_provider_and_marks_only_the_first_external_occurrence() {
        let mut state = PlaybackState::default();
        let current = track(PlaybackProvider::Deezer, "42", false);
        let other = track(PlaybackProvider::SoundCloud, "42", false);
        state.replace(vec![current.clone()], 0);
        let snapshot = PlayingSnapshot::from_playback(&state);
        let rows = snapshot.for_queue(&[other, current.clone(), current]);
        assert_eq!(rows.row(0), RowPlaying::Other);
        assert_eq!(rows.row(1), RowPlaying::Current { animating: false });
        assert_eq!(rows.row(2), RowPlaying::Other);
    }

    #[test]
    fn snapshot_blocking_follows_the_explicit_preference() {
        let explicit = track(PlaybackProvider::Deezer, "42", true);
        let clean = track(PlaybackProvider::Deezer, "43", false);
        let mut state = PlaybackState::default();
        state.replace(vec![explicit.clone(), clean.clone()], 0);
        let snapshot = PlayingSnapshot::from_playback(&state);
        assert!(!snapshot.blocks(&explicit));

        state.skip_explicit = true;
        let snapshot = PlayingSnapshot::from_playback(&state);
        assert!(snapshot.blocks(&explicit));
        assert!(!snapshot.blocks(&clean));
    }

    #[test]
    fn clearing_playback_returns_every_row_to_plain() {
        let mut state = PlaybackState::default();
        let deezer = track(PlaybackProvider::Deezer, "42", false);
        state.replace(vec![deezer.clone()], 0);
        state.clear();
        let snapshot = PlayingSnapshot::from_playback(&state);
        assert_eq!(
            snapshot.for_queue(std::slice::from_ref(&deezer)).row(0),
            RowPlaying::Other
        );
        assert_eq!(state.status, PlaybackStatus::Empty);
    }
}
