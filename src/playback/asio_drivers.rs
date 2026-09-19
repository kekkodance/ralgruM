use rodio::cpal::traits::{DeviceTrait, HostTrait};

use super::engine::AudioOutputTarget;
use super::output_devices::sorted_deduped;

/// Names of every installed ASIO driver, read straight from the registry
/// without loading any driver. The list includes drivers whose hardware is
/// currently absent, matching what other ASIO hosts show, and is sorted
/// and deduplicated so it stays stable across calls.
pub(crate) fn list_registry_asio_drivers() -> Vec<String> {
    sorted_deduped(&registry_asio_driver_names())
}

#[cfg(windows)]
fn registry_asio_driver_names() -> Vec<String> {
    let Ok(asio) = windows_registry::LOCAL_MACHINE.open("SOFTWARE\\ASIO") else {
        return Vec::new();
    };
    let Ok(names) = asio.keys() else {
        return Vec::new();
    };
    names.collect()
}

#[cfg(not(windows))]
fn registry_asio_driver_names() -> Vec<String> {
    Vec::new()
}

/// Finds the ASIO driver whose name is exactly `name`. Enumeration loads
/// and initializes each candidate driver, and ASIO keeps a single driver
/// loaded per process, so the lookup fails while a different ASIO stream
/// is still open.
#[cfg(windows)]
pub(crate) fn find_asio_driver(name: &str) -> Result<rodio::cpal::Device, String> {
    let registered = list_registry_asio_drivers()
        .iter()
        .any(|driver| driver == name);
    if !registered {
        return Err(format!("The ASIO driver \"{name}\" is no longer installed"));
    }
    // CPAL silently skips a registered driver when ASIO cannot load it.
    // This lookup can run on the UI thread, so it must not sleep and retry.
    let host = rodio::cpal::host_from_id(rodio::cpal::HostId::Asio)
        .map_err(|error| format!("The ASIO host could not start: {error}"))?;
    let devices = host
        .output_devices()
        .map_err(|error| format!("ASIO drivers could not be enumerated: {error}"))?;
    devices
        .into_iter()
        .find(|device| device.name().ok().as_deref() == Some(name))
        .ok_or_else(|| format!("The installed ASIO driver \"{name}\" could not be loaded"))
}

#[cfg(not(windows))]
pub(crate) fn find_asio_driver(_name: &str) -> Result<rodio::cpal::Device, String> {
    Err("ASIO is unavailable on this platform".into())
}

/// Maps the saved output selection onto an engine target. ASIO mode plays
/// through the saved driver, falling back to the first installed driver
/// when the saved name is unset or no longer installed; without any
/// installed driver the selection maps like WASAPI mode so playback never
/// blocks. WASAPI mode keeps the plain device mapping.
pub(crate) fn effective_target(
    asio_mode: bool,
    asio_driver: Option<&str>,
    registry_drivers: &[String],
    output_device: Option<&str>,
    wasapi_devices: &[String],
) -> AudioOutputTarget {
    if !asio_mode {
        return super::output_devices::resolve_output_target(output_device, wasapi_devices);
    }
    match asio_driver {
        Some(name) if registry_drivers.iter().any(|driver| driver == name) => {
            AudioOutputTarget::AsioDriver(name.to_owned())
        }
        _ => registry_drivers
            .first()
            .map(|driver| AudioOutputTarget::AsioDriver(driver.clone()))
            .unwrap_or_else(|| {
                super::output_devices::resolve_output_target(output_device, wasapi_devices)
            }),
    }
}

/// Picker entries for the output device dropdown: the registry ASIO driver
/// names in ASIO mode, or the given WASAPI options (system default plus
/// devices) unchanged otherwise.
pub(crate) fn picker_options(
    asio_mode: bool,
    registry_drivers: &[String],
    wasapi_options: Vec<String>,
) -> Vec<String> {
    if asio_mode {
        sorted_deduped(registry_drivers)
    } else {
        wasapi_options
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asio_mode_resolves_the_saved_driver_or_the_first_installed_one() {
        let registry = [
            "Focusrite USB ASIO".to_owned(),
            "MiniFuse ASIO Driver".to_owned(),
        ];
        assert_eq!(
            effective_target(
                true,
                Some("MiniFuse ASIO Driver"),
                &registry,
                Some("Headphones"),
                &["Headphones".to_owned()],
            ),
            AudioOutputTarget::AsioDriver("MiniFuse ASIO Driver".to_owned())
        );
        assert_eq!(
            effective_target(true, None, &registry, None, &[]),
            AudioOutputTarget::AsioDriver("Focusrite USB ASIO".to_owned())
        );
        // A saved driver that is no longer installed falls back to the
        // first installed one, exactly like an unset selection.
        assert_eq!(
            effective_target(
                true,
                Some("Uninstalled ASIO"),
                &registry,
                Some("Headphones"),
                &["Headphones".to_owned()],
            ),
            AudioOutputTarget::AsioDriver("Focusrite USB ASIO".to_owned())
        );
    }

    #[test]
    fn asio_mode_without_installed_drivers_keeps_the_wasapi_mapping() {
        let wasapi = ["Headphones".to_owned()];
        assert_eq!(
            effective_target(
                true,
                Some("MiniFuse ASIO Driver"),
                &[],
                Some("Headphones"),
                &wasapi,
            ),
            AudioOutputTarget::Device("Headphones".to_owned())
        );
        assert_eq!(
            effective_target(true, None, &[], None, &wasapi),
            AudioOutputTarget::SystemDefault
        );
        assert_eq!(
            effective_target(
                true,
                Some("MiniFuse ASIO Driver"),
                &[],
                Some("Unplugged DAC"),
                &wasapi,
            ),
            AudioOutputTarget::SystemDefault
        );
    }

    #[test]
    fn wasapi_mode_maps_the_saved_device_exactly_like_the_plain_helper() {
        let wasapi = ["Headphones".to_owned(), "Speakers".to_owned()];
        let registry = ["MiniFuse ASIO Driver".to_owned()];
        assert_eq!(
            effective_target(
                false,
                Some("MiniFuse ASIO Driver"),
                &registry,
                Some("Headphones"),
                &wasapi,
            ),
            AudioOutputTarget::Device("Headphones".to_owned())
        );
        assert_eq!(
            effective_target(false, None, &registry, Some("Unplugged DAC"), &wasapi),
            AudioOutputTarget::SystemDefault
        );
        assert_eq!(
            effective_target(false, None, &registry, None, &wasapi),
            AudioOutputTarget::SystemDefault
        );
    }

    #[test]
    fn picker_options_follow_the_output_mode() {
        let registry = ["Zeta ASIO".to_owned(), "Alpha ASIO".to_owned()];
        let wasapi = vec![
            "System default".to_owned(),
            "Headphones".to_owned(),
            "Speakers".to_owned(),
        ];
        assert_eq!(
            picker_options(true, &registry, wasapi.clone()),
            ["Alpha ASIO".to_owned(), "Zeta ASIO".to_owned()]
        );
        assert_eq!(picker_options(false, &registry, wasapi.clone()), wasapi);
    }

    #[test]
    fn picker_options_deduplicate_registry_names_in_asio_mode() {
        assert_eq!(
            picker_options(
                true,
                &[
                    "Beta ASIO".to_owned(),
                    "Alpha ASIO".to_owned(),
                    "Beta ASIO".to_owned(),
                ],
                Vec::new(),
            ),
            ["Alpha ASIO".to_owned(), "Beta ASIO".to_owned()]
        );
    }

    #[test]
    fn registry_read_returns_a_stable_list_without_loading_drivers() {
        let drivers = list_registry_asio_drivers();
        // The assertions stay machine-independent: the read must produce a
        // plain, sorted, deduplicated name list without asserting which
        // drivers this machine has installed.
        let mut sorted = drivers.clone();
        sorted.sort();
        assert_eq!(drivers, sorted);
        for driver in &drivers {
            assert!(!driver.is_empty());
        }
    }
}
