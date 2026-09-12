use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use rodio::Source;

use super::fade::USER_FADE_DURATION;

/// Shared gain state for all sources owned by one playback engine.
///
/// The current gain is advanced by the audio thread through `RampedSource`.
/// Control changes only publish a target, so replacing a source does not
/// restart an active fade at either endpoint.
#[derive(Debug)]
pub(crate) struct RampedGain {
    // The upper half stores current and the lower half stores target. Keeping
    // both values in one atomic word makes every audio-side update compare
    // against the complete control snapshot, including resets.
    state: AtomicU64,
}

impl Default for RampedGain {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl RampedGain {
    pub(crate) fn new(gain: f32) -> Self {
        let gain = normalized_gain(gain);
        Self {
            state: AtomicU64::new(pack_state(gain, gain)),
        }
    }

    #[cfg(test)]
    pub(crate) fn target(&self) -> f32 {
        unpack_state(self.state.load(Ordering::Acquire)).1
    }

    #[cfg(test)]
    pub(crate) fn current(&self) -> f32 {
        unpack_state(self.state.load(Ordering::Acquire)).0
    }

    pub(crate) fn set_target(&self, target: f32) {
        let target = normalized_gain(target);
        let mut snapshot = self.state.load(Ordering::Acquire);
        loop {
            let (current, _) = unpack_state(snapshot);
            let replacement = pack_state(current, target);
            match self.state.compare_exchange_weak(
                snapshot,
                replacement,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => snapshot = observed,
            }
        }
    }

    pub(crate) fn reset(&self, gain: f32) {
        let gain = normalized_gain(gain);
        // A stale audio-side CAS contains the old complete current/target
        // snapshot, so it cannot overwrite this reset state after the store.
        self.state.store(pack_state(gain, gain), Ordering::Release);
    }

    pub(crate) fn is_at_target(&self, target: f32) -> bool {
        let expected = normalized_gain(target);
        let (current, published_target) = unpack_state(self.state.load(Ordering::Acquire));
        current == expected && published_target == expected
    }

    pub(crate) fn wrap<S>(self: &Arc<Self>, source: S) -> RampedSource<S>
    where
        S: Source<Item = f32>,
    {
        RampedSource {
            step: fade_step(source.sample_rate(), source.channels()),
            inner: source,
            gain: Arc::clone(self),
        }
    }

    fn advance(&self, step: f32) -> f32 {
        let mut snapshot = self.state.load(Ordering::Acquire);
        loop {
            let (current, target) = unpack_state(snapshot);
            let next = approach(current, target, step);
            let replacement = pack_state(next, target);
            match self.state.compare_exchange_weak(
                snapshot,
                replacement,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(observed) => snapshot = observed,
            }
        }
    }
}

pub(crate) struct RampedSource<S> {
    inner: S,
    gain: Arc<RampedGain>,
    step: f32,
}

impl<S> Iterator for RampedSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let gain = self.gain.advance(self.step);
        Some(sample * gain)
    }
}

impl<S> Source for RampedSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(position)
    }
}

fn fade_step(sample_rate: rodio::SampleRate, channels: rodio::ChannelCount) -> f32 {
    let sample_count =
        f64::from(sample_rate) * f64::from(channels) * USER_FADE_DURATION.as_secs_f64();
    (1.0 / sample_count.max(1.0)) as f32
}

fn pack_state(current: f32, target: f32) -> u64 {
    (u64::from(normalized_gain(current).to_bits()) << 32)
        | u64::from(normalized_gain(target).to_bits())
}

fn unpack_state(state: u64) -> (f32, f32) {
    let current_bits = (state >> 32) as u32;
    let target_bits = state as u32;
    (
        normalized_gain(f32::from_bits(current_bits)),
        normalized_gain(f32::from_bits(target_bits)),
    )
}

fn approach(current: f32, target: f32, step: f32) -> f32 {
    // A f32 step can accumulate a few ulps over a long ramp. Treat the
    // final step as settled within a small numerical tolerance so the
    // endpoint is exact without extending the audible ramp by one sample.
    let settle_tolerance = step + f32::EPSILON * 8.0;
    if (current - target).abs() <= settle_tolerance {
        target
    } else if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

fn normalized_gain(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use rodio::buffer::SamplesBuffer;

    use super::*;

    fn samples(gain: &Arc<RampedGain>, target: f32, count: usize, channels: u16) -> Vec<f32> {
        gain.set_target(target);
        RampedGain::wrap(gain, SamplesBuffer::new(channels, 1_000, vec![1.0; count])).collect()
    }

    #[test]
    fn fade_out_is_monotonic_and_reaches_exact_zero() {
        let gain = Arc::new(RampedGain::new(1.0));
        let values = samples(&gain, 0.0, 160, 1);

        assert!((values[0] - (1.0 - 1.0 / 150.0)).abs() < 0.000001);
        assert_eq!(values[149], 0.0);
        assert!(values.windows(2).all(|pair| pair[1] <= pair[0]));
    }

    #[test]
    fn fade_in_is_monotonic_and_reaches_exact_one() {
        let gain = Arc::new(RampedGain::new(0.0));
        let values = samples(&gain, 1.0, 160, 1);

        assert!((values[0] - 1.0 / 150.0).abs() < 0.000001);
        assert_eq!(values[149], 1.0);
        assert!(values.windows(2).all(|pair| pair[1] >= pair[0]));
    }

    #[test]
    fn stereo_fade_duration_counts_interleaved_samples() {
        let gain = Arc::new(RampedGain::new(0.0));
        let values = samples(&gain, 1.0, 300, 2);

        assert!((values[0] - 1.0 / 300.0).abs() < 0.000001);
        assert_eq!(values[299], 1.0);
        assert!(values[298] < 1.0);
    }

    #[test]
    fn adjacent_gain_deltas_are_bounded_by_one_sample_step() {
        let gain = Arc::new(RampedGain::new(1.0));
        let values = samples(&gain, 0.0, 160, 1);
        let step = 1.0 / 150.0;

        assert!(
            values
                .windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() <= step + 0.000001)
        );
    }

    #[test]
    fn reversal_starts_from_the_shared_current_gain() {
        let gain = Arc::new(RampedGain::new(1.0));
        let _ = samples(&gain, 0.0, 75, 1);
        let midpoint = gain.current();
        assert!((midpoint - 0.5).abs() < 0.000001);

        let values = samples(&gain, 1.0, 2, 1);
        assert!((values[0] - (midpoint + 1.0 / 150.0)).abs() < 0.000001);
        assert!(values[1] > values[0]);
    }

    #[test]
    fn latest_target_wins() {
        let gain = RampedGain::default();
        gain.set_target(0.0);
        gain.set_target(1.0);

        assert_eq!(gain.target(), 1.0);
        assert!(gain.is_at_target(1.0));
    }

    #[test]
    fn settled_requires_current_and_published_target_to_match() {
        let gain = RampedGain::new(0.0);
        gain.set_target(1.0);

        assert!(!gain.is_at_target(0.0));
        assert!(!gain.is_at_target(1.0));

        gain.reset(1.0);
        gain.set_target(0.0);
        assert!(!gain.is_at_target(1.0));
        assert!(!gain.is_at_target(0.0));
    }

    #[test]
    fn changing_target_makes_the_next_sample_follow_the_latest_target() {
        let gain = Arc::new(RampedGain::new(1.0));
        let mut source = RampedGain::wrap(&gain, SamplesBuffer::new(1, 1_000, vec![1.0; 300]));
        gain.set_target(0.0);
        source.next();
        let before_change = gain.current();

        gain.set_target(1.0);
        let after_change = source.next().unwrap();

        assert_eq!(gain.target(), 1.0);
        assert!(after_change > before_change);
    }

    #[test]
    fn reset_sets_both_halves_and_holds_until_a_new_command() {
        let gain = Arc::new(RampedGain::new(1.0));
        let mut source = RampedGain::wrap(&gain, SamplesBuffer::new(1, 1_000, vec![1.0; 300]));
        gain.set_target(0.0);
        source.next();

        gain.reset(0.25);
        let values = source.by_ref().take(4).collect::<Vec<_>>();

        assert_eq!(gain.current(), 0.25);
        assert_eq!(gain.target(), 0.25);
        assert!(values.iter().all(|value| *value == 0.25));
        assert!(gain.is_at_target(0.25));
    }

    #[test]
    fn a_replacement_wrapper_continues_the_shared_ramp() {
        let gain = Arc::new(RampedGain::new(1.0));
        gain.set_target(0.0);
        let first = RampedGain::wrap(&gain, SamplesBuffer::new(1, 1_000, vec![1.0; 30]))
            .collect::<Vec<_>>();
        let current = gain.current();
        let replacement =
            RampedGain::wrap(&gain, SamplesBuffer::new(1, 1_000, vec![1.0; 2])).collect::<Vec<_>>();

        assert_eq!(first.last().copied(), Some(current));
        assert!(replacement[0] < current);
        assert!(replacement[0] > 0.7);
    }

    #[test]
    fn nonfinite_targets_are_silent_and_never_produce_invalid_gain() {
        let gain = Arc::new(RampedGain::new(f32::INFINITY));
        gain.set_target(f32::NAN);
        let values = samples(&gain, f32::NEG_INFINITY, 8, 1);

        assert!(values.iter().all(|value| value.is_finite()));
        assert!(values.iter().all(|value| (0.0..=1.0).contains(value)));
        assert_eq!(gain.target(), 0.0);
    }
}
