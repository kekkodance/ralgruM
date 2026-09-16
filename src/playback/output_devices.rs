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
    sorted_deduped(&names)
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

fn sorted_deduped(names: &[String]) -> Vec<String> {
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
}
