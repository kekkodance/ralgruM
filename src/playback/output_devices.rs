use rodio::cpal::traits::{DeviceTrait, HostTrait};

use super::engine::AudioOutputTarget;

/// Picker entry that maps the audio stream onto the system default output
/// device.
pub(crate) const SYSTEM_DEFAULT_OUTPUT_LABEL: &str = "System default";

/// Names of every output device attached to the default host, sorted and
/// deduplicated so the list is stable across calls while devices come and
/// go. Enumeration only; no audio stream is opened.
pub(crate) fn list_output_devices() -> Vec<String> {
    let host = rodio::cpal::default_host();
    let names = host
        .output_devices()
        .map(|devices| {
            devices
                .filter_map(|device| device.name().ok())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let names = sorted_deduped(&names);
    #[cfg(target_os = "linux")]
    {
        linux_picker_output_names(&names)
    }
    #[cfg(not(target_os = "linux"))]
    {
        names
    }
}

/// ALSA exposes several plugin names alongside real cards. The picker keeps
/// the card entries and only falls back to the sound-server aliases, or to
/// the raw list, when no card is visible. This keeps entries such as
/// `default`, `jack`, or `null` from masquerading as selectable hardware.
#[cfg(any(target_os = "linux", test))]
fn linux_picker_output_names(names: &[String]) -> Vec<String> {
    const SOUND_SERVER_ALIASES: &[&str] = &["pipewire", "pulse"];

    let mut cards = Vec::new();
    let mut servers = Vec::new();
    for name in names {
        let key = name.trim().to_ascii_lowercase();
        let pseudo = matches!(
            key.as_str(),
            "default" | "jack" | "oss" | "null" | "sysdefault"
        ) || key.starts_with("sysdefault:")
            || key.starts_with("dmix")
            || key.starts_with("dsnoop");
        if pseudo {
            continue;
        }
        if SOUND_SERVER_ALIASES.contains(&key.as_str()) {
            servers.push(name.clone());
        } else {
            cards.push(name.clone());
        }
    }

    let selected = if !cards.is_empty() {
        cards
    } else if !servers.is_empty() {
        servers
    } else {
        names.to_vec()
    };
    sorted_deduped(&selected)
}

/// Finds the output device whose endpoint name is exactly `name`.
pub(crate) fn find_output_device(name: &str) -> Option<rodio::cpal::Device> {
    let host = rodio::cpal::default_host();
    host.output_devices()
        .ok()?
        .find(|device| device.name().ok().as_deref() == Some(name))
}

/// Maps a saved output device setting onto an engine target. A missing name
/// resolves to the system default so an unplugged device never blocks audio.
pub(crate) fn resolve_output_target(
    saved: Option<&str>,
    available: &[String],
) -> AudioOutputTarget {
    match saved {
        Some(name) if available.iter().any(|device| device == name) => {
            AudioOutputTarget::Device(name.to_owned())
        }
        _ => AudioOutputTarget::SystemDefault,
    }
}

/// Picker entries for the output device dropdown: the system default
/// followed by the deduplicated device names.
pub(crate) fn output_device_options(devices: &[String]) -> Vec<String> {
    let mut options = Vec::with_capacity(devices.len() + 1);
    options.push(SYSTEM_DEFAULT_OUTPUT_LABEL.to_owned());
    options.extend(sorted_deduped(devices));
    options
}

/// Index of the saved device in the picker entries, falling back to the
/// system default row when nothing is saved or the device is gone.
pub(crate) fn output_device_selected_index(options: &[String], saved: Option<&str>) -> usize {
    let Some(name) = saved else {
        return 0;
    };
    options
        .iter()
        .position(|option| option == name)
        .unwrap_or(0)
}

pub(crate) fn sorted_deduped(names: &[String]) -> Vec<String> {
    let mut sorted = names.to_vec();
    sorted.sort();
    sorted.dedup();
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_devices_resolve_to_matching_device_targets() {
        let available = vec!["Headphones".to_owned(), "Speakers".to_owned()];
        assert_eq!(
            resolve_output_target(Some("Headphones"), &available),
            AudioOutputTarget::Device("Headphones".to_owned())
        );
        assert_eq!(
            resolve_output_target(None, &available),
            AudioOutputTarget::SystemDefault
        );
    }

    #[test]
    fn unknown_saved_devices_fall_back_to_the_system_default() {
        let available = vec!["Headphones".to_owned()];
        assert_eq!(
            resolve_output_target(Some("Unplugged DAC"), &available),
            AudioOutputTarget::SystemDefault
        );
        assert_eq!(
            resolve_output_target(Some("Headphones"), &[]),
            AudioOutputTarget::SystemDefault
        );
    }

    #[test]
    fn picker_options_lead_with_the_system_default_and_deduplicate() {
        assert_eq!(
            output_device_options(&[
                "Speakers".to_owned(),
                "Headphones".to_owned(),
                "Speakers".to_owned(),
            ]),
            [
                SYSTEM_DEFAULT_OUTPUT_LABEL.to_owned(),
                "Headphones".to_owned(),
                "Speakers".to_owned(),
            ]
        );
        assert_eq!(
            output_device_options(&[]),
            [SYSTEM_DEFAULT_OUTPUT_LABEL.to_owned()]
        );
    }

    #[test]
    fn picker_selection_falls_back_to_the_system_default_row() {
        let options = output_device_options(&["Headphones".to_owned(), "Speakers".to_owned()]);
        assert_eq!(output_device_selected_index(&options, None), 0);
        assert_eq!(
            output_device_selected_index(&options, Some("Headphones")),
            1
        );
        assert_eq!(output_device_selected_index(&options, Some("Speakers")), 2);
        assert_eq!(
            output_device_selected_index(&options, Some("Unplugged DAC")),
            0
        );
    }

    #[test]
    fn output_devices_are_sorted_and_deduplicated_in_one_stable_order() {
        assert_eq!(
            sorted_deduped(&["Speakers".to_owned(), "Headphones".to_owned()]),
            ["Headphones".to_owned(), "Speakers".to_owned()]
        );
        assert_eq!(
            sorted_deduped(&[
                "b".to_owned(),
                "a".to_owned(),
                "b".to_owned(),
                "a".to_owned(),
            ]),
            ["a".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn linux_picker_prefers_real_cards_over_alsa_plugins() {
        assert_eq!(
            linux_picker_output_names(&[
                "default".to_owned(),
                "fifine Microphone".to_owned(),
                "jack".to_owned(),
                "pipewire".to_owned(),
                "pulse".to_owned(),
            ]),
            ["fifine Microphone".to_owned()]
        );
    }

    #[test]
    fn linux_picker_falls_back_to_sound_server_aliases() {
        assert_eq!(
            linux_picker_output_names(&[
                "default".to_owned(),
                "pipewire".to_owned(),
                "pulse".to_owned(),
                "null".to_owned(),
            ]),
            ["pipewire".to_owned(), "pulse".to_owned()]
        );
    }

    #[test]
    fn linux_picker_keeps_the_raw_list_when_everything_is_filtered() {
        assert_eq!(
            linux_picker_output_names(&["default".to_owned(), "jack".to_owned()]),
            ["default".to_owned(), "jack".to_owned()]
        );
        assert!(linux_picker_output_names(&[]).is_empty());
    }
}
