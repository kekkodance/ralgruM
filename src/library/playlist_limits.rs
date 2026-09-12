use crate::search::Provider;

pub(crate) const DEEZER_TITLE_MAX_CHARS: usize = 50;
pub(crate) const DEEZER_DESCRIPTION_MAX_CHARS: usize = 200;
pub(crate) const SOUNDCLOUD_TITLE_MAX_CHARS: usize = 100;
pub(crate) const SOUNDCLOUD_DESCRIPTION_MAX_CHARS: usize = 4000;
pub(crate) const LOCAL_TITLE_MAX_CHARS: usize = SOUNDCLOUD_TITLE_MAX_CHARS;
pub(crate) const LOCAL_DESCRIPTION_MAX_CHARS: usize = SOUNDCLOUD_DESCRIPTION_MAX_CHARS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlaylistTextLimits {
    pub(crate) title_max_chars: usize,
    pub(crate) description_max_chars: usize,
}

pub(crate) const fn for_provider(provider: Provider) -> PlaylistTextLimits {
    match provider {
        Provider::Deezer => PlaylistTextLimits {
            title_max_chars: DEEZER_TITLE_MAX_CHARS,
            description_max_chars: DEEZER_DESCRIPTION_MAX_CHARS,
        },
        Provider::SoundCloud => PlaylistTextLimits {
            title_max_chars: SOUNDCLOUD_TITLE_MAX_CHARS,
            description_max_chars: SOUNDCLOUD_DESCRIPTION_MAX_CHARS,
        },
    }
}

pub(crate) const fn for_local() -> PlaylistTextLimits {
    PlaylistTextLimits {
        title_max_chars: LOCAL_TITLE_MAX_CHARS,
        description_max_chars: LOCAL_DESCRIPTION_MAX_CHARS,
    }
}

pub(crate) const fn description_limit_message(provider: Provider) -> &'static str {
    match provider {
        Provider::Deezer => "Playlist descriptions can contain at most 200 characters.",
        Provider::SoundCloud => {
            "SoundCloud playlist descriptions can contain at most 4000 characters."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_limits_keep_deezer_legacy_values_and_add_soundcloud_limits() {
        assert_eq!(
            for_provider(Provider::Deezer),
            PlaylistTextLimits {
                title_max_chars: 50,
                description_max_chars: 200,
            }
        );
        assert_eq!(
            for_provider(Provider::SoundCloud),
            PlaylistTextLimits {
                title_max_chars: 100,
                description_max_chars: 4000,
            }
        );
    }
}
