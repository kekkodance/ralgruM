use super::*;

pub(super) fn adjacent_track(enabled: bool, state: &PlaybackState) -> Option<&PlaybackTrack> {
    if !enabled {
        return None;
    }
    state
        .upcoming_indices()
        .first()
        .and_then(|index| state.queue.get(*index))
        .filter(|track| !state.explicit_blocked(track))
}

pub(super) const fn listen_history_changed_on_completion(
    completed_listen: bool,
    request_succeeded: bool,
) -> bool {
    completed_listen && request_succeeded
}

pub(super) fn listen_history_scope_matches(scheduled_scope: &str, current_scope: &str) -> bool {
    scheduled_scope == current_scope
}

pub(super) fn media_play_should_toggle(status: PlaybackStatus) -> bool {
    matches!(
        status,
        PlaybackStatus::Loading | PlaybackStatus::Paused | PlaybackStatus::Ended
    )
}

pub(super) fn quality_label(
    format: &str,
    declared_bitrate: Option<u32>,
    file_size: Option<u64>,
    duration: Option<Duration>,
) -> Option<String> {
    if let Some(bitrate) = declared_bitrate.filter(|bitrate| *bitrate > 0) {
        return Some(format_bitrate(format, u64::from(bitrate)));
    }
    let seconds = duration?.as_secs();
    if seconds == 0 {
        return Some(format.to_owned());
    }
    match file_size {
        Some(bytes) if bytes > 0 => {
            let kbps = (bytes * 8) / (seconds * 1000);
            Some(format_bitrate(format, kbps))
        }
        _ => Some(format.to_owned()),
    }
}

pub(super) fn player_format_label(format: super::super::resolver::AudioFormat) -> &'static str {
    match format {
        super::super::resolver::AudioFormat::M4a => "AAC",
        _ => format.label(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SeekSliderAction {
    Preview(f32),
    Commit(f32),
    Ignore,
}

pub(super) fn seek_slider_action(event: &SliderEvent) -> SeekSliderAction {
    match event {
        SliderEvent::Change(SliderValue::Single(fraction)) => {
            SeekSliderAction::Preview(fraction.clamp(0., 1.))
        }
        SliderEvent::Release(SliderValue::Single(fraction)) => {
            SeekSliderAction::Commit(fraction.clamp(0., 1.))
        }
        SliderEvent::Change(SliderValue::Range(_, _))
        | SliderEvent::Release(SliderValue::Range(_, _)) => SeekSliderAction::Ignore,
    }
}

pub(super) fn seek_slider_accessibility_commit(
    value: SliderValue,
    current_fraction: Option<f32>,
    preview_fraction: Option<f32>,
) -> Option<f32> {
    let SliderValue::Single(value) = value else {
        return None;
    };
    let current_fraction = current_fraction?;
    if preview_fraction.is_some() {
        return None;
    }
    let value = value.clamp(0.0, 1.0);
    (value - current_fraction)
        .abs()
        .gt(&0.0001)
        .then_some(value)
}
