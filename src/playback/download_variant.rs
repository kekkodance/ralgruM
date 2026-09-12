use super::{PlaybackProvider, PlaybackTrack};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DownloadVariant {
    Best,
    Original,
    Murglar,
    Standard,
    /// Exact Deezer output choices used by track and collection download menus.
    ///
    /// The legacy variants above remain for SoundCloud and fallback paths.
    /// These variants are deliberately separate so a selected Deezer format
    /// can never silently fall back to another bitrate.
    DeezerFlac,
    DeezerMp3_320,
    DeezerMp3_128,
}

impl DownloadVariant {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Best => "Best available",
            Self::Original => "Original quality",
            Self::Murglar => "High quality",
            Self::Standard => "MP3 128 kbps",
            Self::DeezerFlac => "FLAC",
            Self::DeezerMp3_320 => "MP3 320 kbps",
            Self::DeezerMp3_128 => "MP3 128 kbps",
        }
    }

    /// Return the stable explanatory copy shown below a download format.
    /// Keeping this beside the variant makes track and collection menus use
    /// the same quality wording.
    pub(crate) const fn download_detail(self) -> &'static str {
        match self {
            Self::Best => "Best available quality",
            Self::Original => "Original quality",
            Self::Murglar => "High quality",
            Self::Standard => "Standard quality",
            Self::DeezerFlac => "Lossless quality",
            Self::DeezerMp3_320 => "High quality",
            Self::DeezerMp3_128 => "Standard quality",
        }
    }
}

/// A collection-level Deezer format that can be selected without probing
/// every track in the collection first. The resolver still validates the
/// exact format when each track is downloaded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DeezerCollectionDownloadChoice {
    pub(crate) variant: DownloadVariant,
    pub(crate) label: &'static str,
    pub(crate) detail: &'static str,
}

const DEEZER_COLLECTION_ALL: [DeezerCollectionDownloadChoice; 3] = [
    DeezerCollectionDownloadChoice {
        variant: DownloadVariant::DeezerFlac,
        label: "FLAC",
        detail: DownloadVariant::DeezerFlac.download_detail(),
    },
    DeezerCollectionDownloadChoice {
        variant: DownloadVariant::DeezerMp3_320,
        label: "MP3 320 kbps",
        detail: DownloadVariant::DeezerMp3_320.download_detail(),
    },
    DeezerCollectionDownloadChoice {
        variant: DownloadVariant::DeezerMp3_128,
        label: "MP3 128 kbps",
        detail: DownloadVariant::DeezerMp3_128.download_detail(),
    },
];

const DEEZER_COLLECTION_MURGLAR: [DeezerCollectionDownloadChoice; 2] =
    [DEEZER_COLLECTION_ALL[0], DEEZER_COLLECTION_ALL[1]];
const DEEZER_COLLECTION_ARL: [DeezerCollectionDownloadChoice; 1] = [DEEZER_COLLECTION_ALL[2]];

/// Return the exact Deezer choices allowed by the available credentials.
///
/// Murglar credentials authorize FLAC and MP3 320 through Murglar, while a
/// Deezer ARL authorizes direct MP3 128. The two credential sources are
/// independent, so an account with both gets all three choices.
pub(crate) fn deezer_collection_download_choices(
    deezer_arl: bool,
    murglar_token: bool,
) -> &'static [DeezerCollectionDownloadChoice] {
    match (deezer_arl, murglar_token) {
        (true, true) => &DEEZER_COLLECTION_ALL,
        (false, true) => &DEEZER_COLLECTION_MURGLAR,
        (true, false) => &DEEZER_COLLECTION_ARL,
        (false, false) => &[],
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DownloadAvailability {
    pub(crate) best: bool,
    pub(crate) original: bool,
    pub(crate) murglar: bool,
    pub(crate) standard: bool,
}

/// One format proven to be resolvable for one exact track. The context menu
/// uses this instead of provider metadata, which can advertise a format that
/// is unavailable for the current account or track.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadChoice {
    pub(crate) variant: DownloadVariant,
    pub(crate) label: String,
    pub(crate) detail: String,
}

pub(crate) fn download_availability(
    track: &PlaybackTrack,
    deezer_arl: bool,
    soundcloud_token: bool,
    murglar_token: bool,
) -> DownloadAvailability {
    match track.provider {
        PlaybackProvider::Deezer => DownloadAvailability {
            best: murglar_token || deezer_arl,
            original: murglar_token || deezer_arl,
            murglar: false,
            standard: false,
        },
        PlaybackProvider::SoundCloud => DownloadAvailability {
            best: (track.downloadable && soundcloud_token) || murglar_token || track.progressive,
            original: track.downloadable && soundcloud_token,
            murglar: murglar_token,
            standard: track.progressive,
        },
    }
}

pub(crate) fn collection_download_availability(
    tracks: &[PlaybackTrack],
    deezer_arl: bool,
    soundcloud_token: bool,
    murglar_token: bool,
) -> DownloadAvailability {
    tracks
        .iter()
        .map(|track| download_availability(track, deezer_arl, soundcloud_token, murglar_token))
        .reduce(|left, right| DownloadAvailability {
            best: left.best && right.best,
            original: left.original && right.original,
            murglar: left.murglar && right.murglar,
            standard: left.standard && right.standard,
        })
        .unwrap_or(DownloadAvailability {
            best: false,
            original: false,
            murglar: false,
            standard: false,
        })
}

pub(crate) fn selection_order(
    track: &PlaybackTrack,
    variant: DownloadVariant,
    deezer_arl: bool,
    soundcloud_token: bool,
    murglar_token: bool,
) -> &'static [DownloadVariant] {
    const BEST_DEEZER: &[DownloadVariant] = &[DownloadVariant::Murglar, DownloadVariant::Original];
    const ORIGINAL_MURGLAR_STANDARD: &[DownloadVariant] = &[
        DownloadVariant::Original,
        DownloadVariant::Murglar,
        DownloadVariant::Standard,
    ];
    const ORIGINAL_MURGLAR: &[DownloadVariant] =
        &[DownloadVariant::Original, DownloadVariant::Murglar];
    const ORIGINAL_STANDARD: &[DownloadVariant] =
        &[DownloadVariant::Original, DownloadVariant::Standard];
    const MURGLAR_STANDARD: &[DownloadVariant] =
        &[DownloadVariant::Murglar, DownloadVariant::Standard];
    const EMPTY: &[DownloadVariant] = &[];
    match variant {
        DownloadVariant::Best if track.provider == PlaybackProvider::Deezer => {
            if murglar_token || deezer_arl {
                BEST_DEEZER
            } else {
                EMPTY
            }
        }
        DownloadVariant::Best => match (
            track.downloadable && soundcloud_token,
            murglar_token,
            track.progressive,
        ) {
            (true, true, true) => ORIGINAL_MURGLAR_STANDARD,
            (true, true, false) => ORIGINAL_MURGLAR,
            (true, false, true) => ORIGINAL_STANDARD,
            (true, false, false) => &[DownloadVariant::Original],
            (false, true, true) => MURGLAR_STANDARD,
            (false, true, false) => &[DownloadVariant::Murglar],
            (false, false, true) => &[DownloadVariant::Standard],
            (false, false, false) => EMPTY,
        },
        DownloadVariant::Original
            if track.provider == PlaybackProvider::SoundCloud
                && track.downloadable
                && soundcloud_token =>
        {
            &[DownloadVariant::Original]
        }
        DownloadVariant::Murglar if murglar_token => &[DownloadVariant::Murglar],
        DownloadVariant::Standard
            if track.provider == PlaybackProvider::SoundCloud && track.progressive =>
        {
            &[DownloadVariant::Standard]
        }
        DownloadVariant::DeezerFlac
        | DownloadVariant::DeezerMp3_320
        | DownloadVariant::DeezerMp3_128
            if track.provider == PlaybackProvider::Deezer =>
        {
            &[]
        }
        _ => EMPTY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn track(provider: PlaybackProvider, downloadable: bool) -> PlaybackTrack {
        PlaybackTrack {
            provider,
            id: "1".into(),
            title: "t".into(),
            artist: "a".into(),
            album: String::new(),
            album_id: String::new(),
            release_date: String::new(),
            artists: Vec::new(),
            artwork: String::new(),
            duration: Duration::ZERO,
            downloadable,
            progressive: true,
            explicit: false,
            service_url: String::new(),
        }
    }

    #[test]
    fn soundcloud_order_and_strict_choices_match_legacy() {
        let track = track(PlaybackProvider::SoundCloud, true);
        assert_eq!(
            selection_order(&track, DownloadVariant::Best, false, true, true),
            &[
                DownloadVariant::Original,
                DownloadVariant::Murglar,
                DownloadVariant::Standard
            ]
        );
        assert_eq!(
            selection_order(&track, DownloadVariant::Original, false, true, true),
            &[DownloadVariant::Original]
        );
        assert_eq!(
            selection_order(&track, DownloadVariant::Murglar, false, true, true),
            &[DownloadVariant::Murglar]
        );
        assert_eq!(
            selection_order(&track, DownloadVariant::Standard, false, true, true),
            &[DownloadVariant::Standard]
        );
        assert!(selection_order(&track, DownloadVariant::Original, false, false, true).is_empty());
        assert_eq!(
            selection_order(&track, DownloadVariant::Best, false, false, true),
            &[DownloadVariant::Murglar, DownloadVariant::Standard]
        );
    }

    #[test]
    fn availability_does_not_fabricate_soundcloud_credentials() {
        let mut track = track(PlaybackProvider::SoundCloud, false);
        track.progressive = false;
        let value = download_availability(&track, false, false, false);
        assert_eq!(
            value,
            DownloadAvailability {
                best: false,
                original: false,
                murglar: false,
                standard: false
            }
        );
    }

    #[test]
    fn deezer_only_exposes_flac_through_best_or_murglar_credentials() {
        let value =
            download_availability(&track(PlaybackProvider::Deezer, false), false, false, true);
        assert!(value.best && value.original && !value.murglar && !value.standard);

        let without_active_pass =
            download_availability(&track(PlaybackProvider::Deezer, false), false, false, false);
        assert_eq!(
            without_active_pass,
            DownloadAvailability {
                best: false,
                original: false,
                murglar: false,
                standard: false,
            }
        );
    }

    #[test]
    fn exact_deezer_variants_have_strict_display_labels() {
        assert_eq!(DownloadVariant::DeezerFlac.label(), "FLAC");
        assert_eq!(DownloadVariant::DeezerMp3_320.label(), "MP3 320 kbps");
        assert_eq!(DownloadVariant::DeezerMp3_128.label(), "MP3 128 kbps");
        let track = track(PlaybackProvider::SoundCloud, false);
        assert!(
            selection_order(&track, DownloadVariant::DeezerFlac, false, false, false).is_empty()
        );
    }

    #[test]
    fn download_details_match_collection_quality_copy() {
        assert_eq!(
            DownloadVariant::DeezerFlac.download_detail(),
            "Lossless quality"
        );
        assert_eq!(
            DownloadVariant::DeezerMp3_320.download_detail(),
            "High quality"
        );
        assert_eq!(
            DownloadVariant::DeezerMp3_128.download_detail(),
            "Standard quality"
        );
        assert_eq!(DownloadVariant::Murglar.download_detail(), "High quality");
    }

    #[test]
    fn deezer_collection_choices_follow_account_capabilities() {
        assert_eq!(
            deezer_collection_download_choices(false, false)
                .iter()
                .map(|choice| choice.variant)
                .collect::<Vec<_>>(),
            Vec::<DownloadVariant>::new()
        );
        assert_eq!(
            deezer_collection_download_choices(false, true)
                .iter()
                .map(|choice| choice.variant)
                .collect::<Vec<_>>(),
            vec![DownloadVariant::DeezerFlac, DownloadVariant::DeezerMp3_320]
        );
        assert_eq!(
            deezer_collection_download_choices(true, false)
                .iter()
                .map(|choice| choice.variant)
                .collect::<Vec<_>>(),
            vec![DownloadVariant::DeezerMp3_128]
        );
        assert_eq!(
            deezer_collection_download_choices(true, true)
                .iter()
                .map(|choice| choice.variant)
                .collect::<Vec<_>>(),
            vec![
                DownloadVariant::DeezerFlac,
                DownloadVariant::DeezerMp3_320,
                DownloadVariant::DeezerMp3_128,
            ]
        );
    }

    #[test]
    fn deezer_collection_choice_details_describe_user_facing_quality() {
        let choices = deezer_collection_download_choices(true, true);
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.detail)
                .collect::<Vec<_>>(),
            vec!["Lossless quality", "High quality", "Standard quality"]
        );
        assert!(
            choices
                .iter()
                .all(|choice| choice.detail == choice.variant.download_detail())
        );
        assert!(choices.iter().all(|choice| {
            let detail = choice.detail.to_ascii_lowercase();
            !detail.contains("captured") && !detail.contains("source")
        }));
    }

    #[test]
    fn collection_variants_are_the_intersection_across_tracks() {
        let downloadable = track(PlaybackProvider::SoundCloud, true);
        let mut streaming_only = track(PlaybackProvider::SoundCloud, false);
        streaming_only.id = "2".into();
        let value =
            collection_download_availability(&[downloadable, streaming_only], false, true, true);
        assert!(value.best && value.murglar && value.standard);
        assert!(!value.original);
    }
}
