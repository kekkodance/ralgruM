use std::time::Duration;

const MP3_NOMINAL_BITRATES: [u64; 3] = [64, 128, 320];
const MP3_BITRATE_TOLERANCE: u64 = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedQuality {
    pub(crate) format: String,
    pub(crate) source_size: Option<u64>,
    pub(crate) declared_bitrate: Option<u32>,
}

impl ResolvedQuality {
    pub(crate) fn label(&self, duration: Duration, downloaded_size: Option<u64>) -> String {
        format_quality(
            &self.format,
            downloaded_size.or(self.source_size),
            duration,
            self.declared_bitrate,
        )
    }
}

/// Format the provider-resolved quality shown in a download row.
///
/// This mirrors `describeResolvedAudio` in `public/js/core/media-format.js`.
/// A provider-declared bitrate wins over a value measured from the output file.
/// This matters for Deezer's MP3_128/MP3_320 names, where container metadata
/// and whole-second durations make a measured value slightly noisy.
pub(crate) fn format_quality(
    format: &str,
    size: Option<u64>,
    duration: Duration,
    declared_bitrate: Option<u32>,
) -> String {
    let bitrate = declared_bitrate
        .filter(|bitrate| *bitrate > 0)
        .or_else(|| declared_bitrate_from_format(format))
        .or_else(|| measured_bitrate(size, duration));
    let format = format_label(format);
    match bitrate {
        Some(bitrate) => format_bitrate(&format, u64::from(bitrate)),
        None => format!("{format} bitrate unavailable"),
    }
}

/// Format a bitrate label, snapping MP3 estimates close to supported tiers.
pub(crate) fn format_bitrate(format: &str, bitrate: u64) -> String {
    let bitrate = if format.eq_ignore_ascii_case("MP3") {
        MP3_NOMINAL_BITRATES
            .iter()
            .copied()
            .find(|tier| bitrate.abs_diff(*tier) <= MP3_BITRATE_TOLERANCE)
            .unwrap_or(bitrate)
    } else {
        bitrate
    };
    format!("{format} {bitrate}kbps")
}

pub(crate) fn measured_bitrate(size: Option<u64>, duration: Duration) -> Option<u32> {
    let size = size.filter(|size| *size > 0)?;
    let seconds = duration.as_secs_f64();
    if seconds <= 0.0 {
        return None;
    }
    let bitrate = ((size as f64 * 8.0) / (seconds * 1_000.0)).round();
    (bitrate > 0.0 && bitrate <= f64::from(u32::MAX)).then_some(bitrate as u32)
}

/// Match `displayAudioFormat` in `public/js/core/media-format.js`: preserve
/// the resolved provider name, uppercase it, and strip only a trailing
/// underscore-plus-digits suffix.
pub(crate) fn format_label(format: &str) -> String {
    if format.is_empty() {
        "AUDIO".into()
    } else {
        let upper = format.to_ascii_uppercase();
        let displayed = strip_format_suffix(&upper);
        if displayed.is_empty() {
            "AUDIO".into()
        } else {
            displayed.to_owned()
        }
    }
}

/// Read the Deezer-style `_(\d{2,4})$` bitrate suffix used by the original
/// media-format helper. Other digits are not treated as a bitrate guess.
#[cfg(test)]
pub(crate) fn declared_bitrate(format: &str) -> Option<u32> {
    declared_bitrate_from_format(format)
}

fn declared_bitrate_from_format(format: &str) -> Option<u32> {
    let (_, suffix) = format.rsplit_once('_')?;
    if !(2..=4).contains(&suffix.len()) || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    suffix.parse().ok()
}

fn strip_format_suffix(format: &str) -> &str {
    let Some((prefix, suffix)) = format.rsplit_once('_') else {
        return format;
    };
    if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        prefix
    } else {
        format
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deezer_suffix_is_authoritative_over_measured_size() {
        assert_eq!(
            format_quality("MP3_128", Some(1_000_000), Duration::from_secs(10), None),
            "MP3 128kbps"
        );
        assert_eq!(declared_bitrate("MP3_320"), Some(320));
        assert_eq!(declared_bitrate("MP3_10000"), None);
        assert_eq!(declared_bitrate("MP3 320kbps"), None);
    }

    #[test]
    fn measured_bitrate_applies_to_every_resolved_format() {
        assert_eq!(
            format_quality("FLAC", Some(1_000_000), Duration::from_secs(10), Some(800)),
            "FLAC 800kbps"
        );
        assert_eq!(
            format_quality("FLAC", Some(1_000_000), Duration::from_secs(10), None),
            "FLAC 800kbps"
        );
    }

    #[test]
    fn available_soundcloud_bitrate_beats_file_measurement() {
        assert_eq!(
            format_quality("MP3", Some(1_000_000), Duration::from_secs(10), Some(128)),
            "MP3 128kbps"
        );
        assert_eq!(
            format_quality("Opus", Some(80_000), Duration::from_secs(10), None),
            "OPUS 64kbps"
        );
    }

    #[test]
    fn mp3_bitrate_labels_snap_to_nominal_tiers() {
        assert_eq!(format_bitrate("MP3", 129), "MP3 128kbps");

        for bitrate in MP3_NOMINAL_BITRATES {
            assert_eq!(format_bitrate("MP3", bitrate), format!("MP3 {bitrate}kbps"));
        }
    }

    #[test]
    fn mp3_bitrate_labels_leave_out_of_tolerance_values_unchanged() {
        assert_eq!(format_bitrate("MP3", 133), "MP3 133kbps");
        assert_eq!(format_bitrate("MP3", 192), "MP3 192kbps");
    }

    #[test]
    fn non_mp3_bitrate_labels_are_unchanged() {
        assert_eq!(format_bitrate("FLAC", 129), "FLAC 129kbps");
        assert_eq!(format_bitrate("AAC", 129), "AAC 129kbps");
        assert_eq!(format_bitrate("Opus", 129), "Opus 129kbps");
        assert_eq!(format_bitrate("unknown", 129), "unknown 129kbps");
    }

    #[test]
    fn downloads_quality_formatter_uses_shared_bitrate_formatter() {
        assert_eq!(
            format_quality("MP3", Some(161_250), Duration::from_secs(10), None),
            "MP3 128kbps"
        );
    }

    #[test]
    fn unavailable_bitrate_is_not_fabricated() {
        assert_eq!(
            format_quality("WAV", None, Duration::ZERO, None),
            "WAV bitrate unavailable"
        );
        assert_eq!(declared_bitrate("MP3"), None);
    }

    #[test]
    fn format_label_preserves_the_resolved_name() {
        assert_eq!(format_label("MP3_320"), "MP3");
        assert_eq!(format_label("Ogg Vorbis"), "OGG VORBIS");
        assert_eq!(format_label("M4A/MP4 AAC"), "M4A/MP4 AAC");
        assert_eq!(format_label("_128"), "AUDIO");
        assert_eq!(format_label(""), "AUDIO");
    }

    #[test]
    fn measured_bitrate_rounds_like_javascript() {
        assert_eq!(
            measured_bitrate(Some(160_625), Duration::from_secs(10)),
            Some(129)
        );
    }
}
