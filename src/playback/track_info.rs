use std::{sync::Arc, time::Duration};

use tokio::{runtime::Runtime, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{MediaCredentials, PlaybackTrack, StreamResolver, resolver::AudioFormat};
use crate::search::{DeezerArl, SoundCloudToken};

/// Everything needed to resolve a track's audio format and size without
/// starting playback. Reuses the same read-only source resolution the player
/// runs before downloading, so no stream bytes are fetched or cached.
pub(crate) struct TrackInfoProbe {
    resolver: StreamResolver,
    runtime: Arc<Runtime>,
    arl: Option<DeezerArl>,
    soundcloud: Option<SoundCloudToken>,
    murglar: Option<MediaCredentials>,
}

impl TrackInfoProbe {
    pub(crate) fn new(
        resolver: StreamResolver,
        runtime: Arc<Runtime>,
        arl: Option<DeezerArl>,
        soundcloud: Option<SoundCloudToken>,
        murglar: Option<MediaCredentials>,
    ) -> Self {
        Self {
            resolver,
            runtime,
            arl,
            soundcloud,
            murglar,
        }
    }

    pub(crate) fn spawn(
        &self,
        track: &PlaybackTrack,
    ) -> JoinHandle<Result<ResolvedTrackInfo, String>> {
        let resolver = self.resolver.clone();
        let arl = self.arl.clone();
        let soundcloud = self.soundcloud.clone();
        let murglar = self.murglar.clone();
        let track = track.clone();
        self.runtime.spawn(async move {
            let cancellation = CancellationToken::new();
            let source = resolver
                .resolve_source(&track, arl, soundcloud, murglar, cancellation.clone(), true)
                .await?;
            let bytes = resolver
                .source_size_for_info(&source, &cancellation)
                .await?;
            Ok(ResolvedTrackInfo {
                format: source.format,
                bytes,
                timeline_size_unknown: source.timeline_size_unknown(),
                declared_bitrate: source.declared_bitrate,
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedTrackInfo {
    pub(crate) format: AudioFormat,
    pub(crate) bytes: u64,
    pub(crate) timeline_size_unknown: bool,
    pub(crate) declared_bitrate: Option<u32>,
}

impl ResolvedTrackInfo {
    pub(crate) fn is_useful(self, duration: Duration) -> bool {
        self.bytes > 0
            || self.declared_bitrate.is_some()
            || bitrate_kbps(self.bytes, duration).is_some()
    }
}

/// Renders the primary context-menu row text: computed size and bitrate.
/// Sizes under 10 MB keep two decimals, matching formatTrackAudioInfo in the
/// original app.
pub(crate) fn describe_track_info(info: ResolvedTrackInfo, duration: Duration) -> String {
    let size = if info.bytes > 0 {
        let megabytes = info.bytes as f64 / (1024.0 * 1024.0);
        if info.bytes < 10 * 1024 * 1024 {
            Some(format!("{megabytes:.2} MB"))
        } else {
            Some(format!("{megabytes:.1} MB"))
        }
    } else if info.timeline_size_unknown {
        None
    } else {
        Some("Size unavailable".to_owned())
    };
    let bitrate = match info
        .declared_bitrate
        .map(u64::from)
        .or_else(|| bitrate_kbps(info.bytes, duration))
    {
        Some(kbps) => format!("{kbps} kbps"),
        None => "Bitrate unavailable".to_owned(),
    };
    match size {
        Some(size) => format!("{size} \u{b7} {bitrate}"),
        None => bitrate,
    }
}

/// Mirrors the measured bitrate of formatTrackAudioInfo in the original app:
/// bytes times eight over the duration, rounded to a whole kbps.
pub(crate) fn bitrate_kbps(bytes: u64, duration: Duration) -> Option<u64> {
    let seconds = duration.as_secs();
    if bytes == 0 || seconds == 0 {
        return None;
    }
    let denominator = u128::from(seconds).checked_mul(1_000)?;
    let numerator = u128::from(bytes).checked_mul(8)?;
    u64::try_from((numerator + denominator / 2) / denominator).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_measures_bytes_over_duration() {
        assert_eq!(bitrate_kbps(1_000_000, Duration::from_secs(10)), Some(800));
        assert_eq!(bitrate_kbps(0, Duration::from_secs(10)), None);
        assert_eq!(bitrate_kbps(1_000_000, Duration::ZERO), None);
    }

    #[test]
    fn descriptions_match_the_original_wording() {
        let info = ResolvedTrackInfo {
            format: AudioFormat::Flac,
            bytes: 1_000_000,
            timeline_size_unknown: false,
            declared_bitrate: None,
        };
        assert_eq!(
            describe_track_info(info, Duration::from_secs(10)),
            "0.95 MB \u{b7} 800 kbps"
        );
        let large = ResolvedTrackInfo {
            format: AudioFormat::Flac,
            bytes: 41_943_040,
            timeline_size_unknown: false,
            declared_bitrate: None,
        };
        assert_eq!(
            describe_track_info(large, Duration::from_secs(600)),
            "40.0 MB \u{b7} 559 kbps"
        );
        let unknown = ResolvedTrackInfo {
            format: AudioFormat::Mp3,
            bytes: 0,
            timeline_size_unknown: false,
            declared_bitrate: None,
        };
        assert_eq!(
            describe_track_info(unknown, Duration::from_secs(10)),
            "Size unavailable \u{b7} Bitrate unavailable"
        );
    }

    #[test]
    fn declared_bitrate_takes_precedence_over_measured_bytes() {
        let info = ResolvedTrackInfo {
            format: AudioFormat::Mp3,
            bytes: 1_000_000,
            timeline_size_unknown: false,
            declared_bitrate: Some(128),
        };
        assert_eq!(
            describe_track_info(info, Duration::from_secs(10)),
            "0.95 MB \u{b7} 128 kbps"
        );
    }

    #[test]
    fn timeline_sources_omit_only_unknown_size() {
        let info = ResolvedTrackInfo {
            format: AudioFormat::Flac,
            bytes: 0,
            timeline_size_unknown: true,
            declared_bitrate: Some(978),
        };
        assert_eq!(
            describe_track_info(info, Duration::from_secs(600)),
            "978 kbps"
        );

        let exact = ResolvedTrackInfo {
            bytes: 1_000_000,
            ..info
        };
        assert_eq!(
            describe_track_info(exact, Duration::from_secs(600)),
            "0.95 MB \u{b7} 978 kbps"
        );
    }
}
