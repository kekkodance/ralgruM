use super::engine::AudioOutputTarget;
use super::output_devices::sorted_deduped;

/// One installed ASIO driver as recorded in the registry. ASIO addresses a
/// driver by its `Description` value, while the registry key can use a
/// different name; both are kept so saved selections made from either form
/// keep resolving.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistryAsioDriver {
    key_name: String,
    canonical_name: String,
}

impl RegistryAsioDriver {
    fn new(key_name: String, description: Option<String>) -> Self {
        let canonical_name = description
            .filter(|description| !description.trim().is_empty())
            .unwrap_or_else(|| key_name.clone());
        Self {
            key_name,
            canonical_name,
        }
    }
}

/// Names of every installed ASIO driver, read straight from the registry
/// without loading any driver. Each entry is the driver's `Description`,
/// which is the name CPAL and the ASIO loader address drivers by. The list
/// includes drivers whose hardware is currently absent, matching what other
/// ASIO hosts show, and is sorted and deduplicated so it stays stable across
/// calls.
pub(crate) fn list_registry_asio_drivers() -> Vec<String> {
    sorted_deduped(
        &registry_asio_drivers()
            .into_iter()
            .map(|driver| driver.canonical_name)
            .collect::<Vec<_>>(),
    )
}

/// Maps a saved ASIO selection onto the driver's canonical `Description`.
/// Older builds saved the registry key name, which can differ from the
/// description (for example `US-HR Series ASIO Driver` versus
/// `US-HR Series ASIO`), so both forms are accepted.
pub(crate) fn resolve_registry_asio_driver_name(name: &str) -> Option<String> {
    resolve_registered_driver(name, &registry_asio_drivers()).map(str::to_owned)
}

/// Canonicalizes a saved driver name for display and matching. Names that
/// are no longer installed are returned unchanged so callers can report
/// them as missing.
pub(crate) fn canonical_asio_driver_name(name: &str) -> String {
    resolve_registry_asio_driver_name(name).unwrap_or_else(|| name.to_owned())
}

fn resolve_registered_driver<'a>(name: &str, drivers: &'a [RegistryAsioDriver]) -> Option<&'a str> {
    drivers
        .iter()
        .find(|driver| driver.canonical_name == name)
        .or_else(|| drivers.iter().find(|driver| driver.key_name == name))
        .map(|driver| driver.canonical_name.as_str())
}

#[cfg(windows)]
fn registry_asio_drivers() -> Vec<RegistryAsioDriver> {
    let Ok(asio) = windows_registry::LOCAL_MACHINE.open("SOFTWARE\\ASIO") else {
        return Vec::new();
    };
    let Ok(names) = asio.keys() else {
        return Vec::new();
    };
    names
        .map(|key_name| {
            let description = asio
                .open(&key_name)
                .ok()
                .and_then(|key| key.get_string("Description").ok());
            RegistryAsioDriver::new(key_name, description)
        })
        .collect()
}

#[cfg(not(windows))]
fn registry_asio_drivers() -> Vec<RegistryAsioDriver> {
    Vec::new()
}

/// Finds and loads the ASIO driver saved as `name`. The saved name may be
/// the registry key or the canonical description; loading always uses the
/// canonical description because that is the name the ASIO loader matches.
/// Loading the selected driver directly avoids initializing every other
/// installed driver first. ASIO keeps a single driver loaded per process,
/// so the lookup still fails while a different ASIO stream is open.
#[cfg(windows)]
pub(crate) fn find_asio_driver(name: &str) -> Result<rodio::cpal::Device, String> {
    let Some(canonical) = resolve_registry_asio_driver_name(name) else {
        return Err(format!("The ASIO driver \"{name}\" is no longer installed"));
    };
    // This runs on the ASIO owner thread, which owns the driver's COM
    // lifecycle. It must not sleep or retry: the caller is blocked waiting
    // for the open result.
    let host = rodio::cpal::host_from_id(rodio::cpal::HostId::Asio)
        .map_err(|error| format!("The ASIO host could not start: {error}"))?;
    let rodio::cpal::platform::HostInner::Asio(asio_host) = host.as_inner() else {
        return Err("The ASIO host could not start".to_owned());
    };
    asio_host
        .device_from_name(&canonical)
        .map(rodio::cpal::Device::from)
        .map_err(|error| {
            format!("The installed ASIO driver \"{canonical}\" could not be loaded: {error}")
        })
}

/// Maps the saved output selection onto an engine target. `asio_driver` is
/// expected in canonical registry-description form. ASIO mode plays through
/// the saved driver, falling back to the first installed driver when the
/// saved name is unset or no longer installed; without any installed driver
/// the selection maps like WASAPI mode so playback never blocks. WASAPI mode
/// keeps the plain device mapping.
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
    fn registry_resolution_accepts_the_key_name_and_the_description() {
        let drivers = [
            RegistryAsioDriver::new(
                "US-HR Series ASIO Driver".to_owned(),
                Some("US-HR Series ASIO".to_owned()),
            ),
            RegistryAsioDriver::new(
                "MiniFuse ASIO Driver".to_owned(),
                Some("MiniFuse ASIO Driver".to_owned()),
            ),
        ];
        // The TASCAM key and description differ; both must resolve to the
        // description the ASIO loader actually matches.
        assert_eq!(
            resolve_registered_driver("US-HR Series ASIO Driver", &drivers),
            Some("US-HR Series ASIO")
        );
        assert_eq!(
            resolve_registered_driver("US-HR Series ASIO", &drivers),
            Some("US-HR Series ASIO")
        );
        assert_eq!(
            resolve_registered_driver("MiniFuse ASIO Driver", &drivers),
            Some("MiniFuse ASIO Driver")
        );
        assert_eq!(resolve_registered_driver("Unknown ASIO", &drivers), None);
    }

    #[test]
    fn registry_resolution_falls_back_to_the_key_name_without_a_description() {
        let drivers = [
            RegistryAsioDriver::new("Legacy ASIO Driver".to_owned(), None),
            RegistryAsioDriver::new("Blank ASIO Driver".to_owned(), Some("  ".to_owned())),
        ];
        assert_eq!(
            resolve_registered_driver("Legacy ASIO Driver", &drivers),
            Some("Legacy ASIO Driver")
        );
        assert_eq!(
            resolve_registered_driver("Blank ASIO Driver", &drivers),
            Some("Blank ASIO Driver")
        );
    }

    #[test]
    fn registry_resolution_keeps_duplicate_descriptions_addressable_by_key() {
        let drivers = [
            RegistryAsioDriver::new("First Key".to_owned(), Some("Shared ASIO".to_owned())),
            RegistryAsioDriver::new("Second Key".to_owned(), Some("Shared ASIO".to_owned())),
        ];
        assert_eq!(
            resolve_registered_driver("Shared ASIO", &drivers),
            Some("Shared ASIO")
        );
        assert_eq!(
            resolve_registered_driver("Second Key", &drivers),
            Some("Shared ASIO")
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
