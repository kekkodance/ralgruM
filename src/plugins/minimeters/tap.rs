use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use crossbeam_queue::ArrayQueue;
use rodio::Source;

#[derive(Clone, Copy)]
pub(super) struct Frame {
    pub left: f32,
    pub right: f32,
    pub sample_rate: u32,
}

pub(crate) struct Tap {
    pub enabled: AtomicBool,
    volume: AtomicU32,
    pub(super) frames: ArrayQueue<Frame>,
}

static TAP: OnceLock<Arc<Tap>> = OnceLock::new();

pub(crate) fn shared() -> Arc<Tap> {
    Arc::clone(TAP.get_or_init(|| {
        Arc::new(Tap {
            enabled: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            frames: ArrayQueue::new(16_384),
        })
    }))
}

pub(crate) fn set_volume(volume: f32) {
    shared()
        .volume
        .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub(crate) fn clear() {
    let tap = shared();
    while tap.frames.pop().is_some() {}
}

pub(crate) fn wrap<S: Source<Item = f32>>(source: S) -> TapSource<S> {
    let channels = source.channels().max(1) as usize;
    let sample_rate = source.sample_rate();
    TapSource {
        source,
        tap: shared(),
        channels,
        sample_rate,
        index: 0,
        left: 0.0,
        right: 0.0,
    }
}

pub(crate) struct TapSource<S> {
    source: S,
    tap: Arc<Tap>,
    channels: usize,
    sample_rate: u32,
    index: usize,
    left: f32,
    right: f32,
}

impl<S: Source<Item = f32>> Iterator for TapSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.source.next()?;
        if self.tap.enabled.load(Ordering::Relaxed) {
            let sample = sample * f32::from_bits(self.tap.volume.load(Ordering::Relaxed));
            match self.index {
                0 => self.left = sample,
                1 => self.right = sample,
                _ => {}
            }
            if self.index + 1 == self.channels {
                let _ = self.tap.frames.push(Frame {
                    left: self.left,
                    right: if self.channels == 1 {
                        self.left
                    } else {
                        self.right
                    },
                    sample_rate: self.sample_rate,
                });
            }
        }
        self.index = (self.index + 1) % self.channels;
        Some(sample)
    }
}

impl<S: Source<Item = f32>> Source for TapSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.source.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.source.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.source.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.source.total_duration()
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.index = 0;
        self.source.try_seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;

    #[test]
    fn stereo_frames_are_copied_without_changing_playback() {
        let tap = shared();
        while tap.frames.pop().is_some() {}
        set_volume(0.5);
        tap.enabled.store(true, Ordering::Release);
        let input = SamplesBuffer::new(2, 48_000, vec![0.4, -0.6]);
        let output: Vec<_> = wrap(input).collect();
        tap.enabled.store(false, Ordering::Release);
        assert_eq!(output, vec![0.4, -0.6]);
        let frame = tap.frames.pop().unwrap();
        assert!((frame.left - 0.2).abs() < 1e-6);
        assert!((frame.right + 0.3).abs() < 1e-6);
    }
}
