pub type SupportedInputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;
pub type SupportedOutputConfigs = std::vec::IntoIter<SupportedStreamConfigRange>;

use super::sys;
use crate::BackendSpecificError;
use crate::DefaultStreamConfigError;
use crate::DeviceNameError;
use crate::DevicesError;
use crate::SampleFormat;
use crate::SampleRate;
use crate::SupportedBufferSize;
use crate::SupportedStreamConfig;
use crate::SupportedStreamConfigRange;
use crate::SupportedStreamConfigsError;
use std::hash::{Hash, Hasher};
use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Mutex};

/// A ASIO Device
#[derive(Clone)]
pub struct Device {
    /// The driver represented by this device.
    pub driver: Arc<sys::Driver>,

    // Input and/or Output stream.
    // A driver can only have one of each.
    // They need to be created at the same time.
    pub asio_streams: Arc<Mutex<sys::AsioStreams>>,
    pub current_buffer_index: Arc<AtomicI32>,
}

use std::collections::HashMap;
use std::sync::OnceLock;

/// Per-driver shared stream slots, keyed by the driver name which is the
/// process-wide identity of an ASIO driver.
fn shared_driver_state() -> &'static Mutex<HashMap<String, SharedDriverState>> {
    static STATE: OnceLock<Mutex<HashMap<String, SharedDriverState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct SharedDriverState {
    /// Identity of the driver instance the buffers belong to. Only identity
    /// is kept, never a strong reference: the map must not extend the
    /// driver's life or the final unload would never run and every later
    /// load would fail with the driver still registered as current. A slot
    /// left over from a previous load of the driver is discarded on lookup:
    /// its buffers died with that instance, and reusing them would skip
    /// ASIOCreateBuffers and start the fresh driver against freed memory.
    driver_instance: DriverInstanceId,
    asio_streams: Arc<Mutex<sys::AsioStreams>>,
    current_buffer_index: Arc<AtomicI32>,
}

fn driver_key(driver: &Arc<sys::Driver>) -> String {
    driver.name().to_owned()
}

fn fresh_shared_driver_state(driver: &Arc<sys::Driver>) -> SharedDriverState {
    SharedDriverState {
        driver_instance: DriverInstanceId(driver.instance_id()),
        asio_streams: Arc::new(Mutex::new(sys::AsioStreams {
            input: None,
            output: None,
        })),
        current_buffer_index: Arc::new(AtomicI32::new(-1)),
    }
}

/// Raw driver-instance identity. Only compared for equality, never
/// dereferenced, so it is safe to move across threads.
#[derive(Clone, Copy, PartialEq, Eq)]
struct DriverInstanceId(*const std::ffi::c_void);

unsafe impl Send for DriverInstanceId {}

/// Returns the shared stream slot for the driver, creating it on first use.
/// Every Device handle for the same driver gets the same slot, so buffer
/// lifetimes stay tied to the process-global driver instead of a Device.
fn shared_asio_streams(driver: &Arc<sys::Driver>) -> Arc<Mutex<sys::AsioStreams>> {
    let mut state = shared_driver_state().lock().unwrap();
    let entry = state
        .entry(driver_key(driver))
        .and_modify(|entry| {
            if entry.driver_instance.0 != driver.instance_id() {
                *entry = fresh_shared_driver_state(driver);
            }
        })
        .or_insert_with(|| fresh_shared_driver_state(driver));
    entry.asio_streams.clone()
}

/// Clears the shared stream slot for the driver. Callers that fully unloaded
/// the driver (teardown across a system sleep) must clear it so the next
/// open re-creates the ASIO buffers instead of reusing dead ones.
pub fn clear_shared_asio_streams(driver_name: &str) {
    let mut state = shared_driver_state().lock().unwrap();
    if let Some(entry) = state.get_mut(driver_name) {
        if let Ok(mut streams) = entry.asio_streams.lock() {
            streams.output = None;
            streams.input = None;
        }
    }
}

/// Returns the shared silence-tracking buffer index for the driver,
/// discarding state left over from a previous load of the driver.
fn shared_current_buffer_index(driver: &Arc<sys::Driver>) -> Arc<AtomicI32> {
    let mut state = shared_driver_state().lock().unwrap();
    let entry = state
        .entry(driver_key(driver))
        .and_modify(|entry| {
            if entry.driver_instance.0 != driver.instance_id() {
                *entry = fresh_shared_driver_state(driver);
            }
        })
        .or_insert_with(|| fresh_shared_driver_state(driver));
    entry.current_buffer_index.clone()
}

/// All available devices.
pub struct Devices {
    asio: Arc<sys::Asio>,
    drivers: std::vec::IntoIter<String>,
}

impl PartialEq for Device {
    fn eq(&self, other: &Self) -> bool {
        self.driver.name() == other.driver.name()
    }
}

impl Eq for Device {}

impl Hash for Device {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.driver.name().hash(state);
    }
}

impl Device {
    pub fn name(&self) -> Result<String, DeviceNameError> {
        Ok(self.driver.name().to_string())
    }

    /// Gets the supported input configs.
    /// TODO currently only supports the default.
    /// Need to find all possible configs.
    pub fn supported_input_configs(
        &self,
    ) -> Result<SupportedInputConfigs, SupportedStreamConfigsError> {
        // Retrieve the default config for the total supported channels and supported sample
        // format.
        let f = match self.default_input_config() {
            Err(_) => return Err(SupportedStreamConfigsError::DeviceNotAvailable),
            Ok(f) => f,
        };

        // Collect a config for every combination of supported sample rate and number of channels.
        let mut supported_configs = vec![];
        for &rate in crate::COMMON_SAMPLE_RATES {
            if !self
                .driver
                .can_sample_rate(rate.0.into())
                .ok()
                .unwrap_or(false)
            {
                continue;
            }
            for channels in 1..f.channels + 1 {
                supported_configs.push(SupportedStreamConfigRange {
                    channels,
                    min_sample_rate: rate,
                    max_sample_rate: rate,
                    buffer_size: f.buffer_size,
                    sample_format: f.sample_format,
                })
            }
        }
        Ok(supported_configs.into_iter())
    }

    /// Gets the supported output configs.
    /// TODO currently only supports the default.
    /// Need to find all possible configs.
    pub fn supported_output_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        // Retrieve the default config for the total supported channels and supported sample
        // format.
        let f = match self.default_output_config() {
            Err(_) => return Err(SupportedStreamConfigsError::DeviceNotAvailable),
            Ok(f) => f,
        };

        // Collect a config for every combination of supported sample rate and number of channels.
        let mut supported_configs = vec![];
        for &rate in crate::COMMON_SAMPLE_RATES {
            if !self
                .driver
                .can_sample_rate(rate.0.into())
                .ok()
                .unwrap_or(false)
            {
                continue;
            }
            for channels in 1..f.channels + 1 {
                supported_configs.push(SupportedStreamConfigRange {
                    channels,
                    min_sample_rate: rate,
                    max_sample_rate: rate,
                    buffer_size: f.buffer_size,
                    sample_format: f.sample_format,
                })
            }
        }
        Ok(supported_configs.into_iter())
    }

    /// Returns the default input config
    pub fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        let channels = self.driver.channels().map_err(default_config_err)?.ins as u16;
        let sample_rate = SampleRate(self.driver.sample_rate().map_err(default_config_err)? as _);
        let (min, max) = self.driver.buffersize_range().map_err(default_config_err)?;
        let buffer_size = SupportedBufferSize::Range {
            min: min as u32,
            max: max as u32,
        };
        // Map th ASIO sample type to a CPAL sample type
        let data_type = self.driver.input_data_type().map_err(default_config_err)?;
        let sample_format = convert_data_type(&data_type)
            .ok_or(DefaultStreamConfigError::StreamTypeNotSupported)?;
        Ok(SupportedStreamConfig {
            channels,
            sample_rate,
            buffer_size,
            sample_format,
        })
    }

    /// Returns the default output config
    pub fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        let channels = self.driver.channels().map_err(default_config_err)?.outs as u16;
        let sample_rate = SampleRate(self.driver.sample_rate().map_err(default_config_err)? as _);
        let (min, max) = self.driver.buffersize_range().map_err(default_config_err)?;
        let buffer_size = SupportedBufferSize::Range {
            min: min as u32,
            max: max as u32,
        };
        let data_type = self.driver.output_data_type().map_err(default_config_err)?;
        let sample_format = convert_data_type(&data_type)
            .ok_or(DefaultStreamConfigError::StreamTypeNotSupported)?;
        Ok(SupportedStreamConfig {
            channels,
            sample_rate,
            buffer_size,
            sample_format,
        })
    }
}

/// Notification that the ASIO driver requested a host action, delivered
/// through the driver's message callback (`kAsioResetRequest`,
/// `kAsioResyncRequest`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AsioDriverMessage {
    /// The driver asks the host to tear it down and re-initialize it, for
    /// example after a system sleep invalidated its session.
    ResetRequest,
    /// The driver lost synchronization; the host should restart streaming.
    ResyncRequest,
}

static DRIVER_MESSAGE_CALLBACKS: Mutex<Vec<Arc<dyn Fn(AsioDriverMessage) + Send + Sync>>> =
    Mutex::new(Vec::new());

/// Subscribes to ASIO driver messages. The callback is invoked from the
/// driver's own thread; keep it cheap and non-blocking.
pub fn on_driver_message(callback: Arc<dyn Fn(AsioDriverMessage) + Send + Sync>) {
    DRIVER_MESSAGE_CALLBACKS.lock().unwrap().push(callback);
}

fn dispatch_driver_message(message: AsioDriverMessage) {
    if let Ok(callbacks) = DRIVER_MESSAGE_CALLBACKS.lock() {
        for callback in callbacks.iter() {
            callback(message);
        }
    }
}

/// Registers the process-global driver message dispatcher so that
/// `kAsioResetRequest` and `kAsioResyncRequest` reach the subscribers. Safe
/// to call repeatedly; the registration is idempotent per driver.
pub(crate) fn register_driver_message_dispatch(driver: &Arc<sys::Driver>) {
    driver.add_message_callback(|selector| match selector {
        sys::AsioMessageSelectors::kAsioResetRequest => {
            dispatch_driver_message(AsioDriverMessage::ResetRequest)
        }
        sys::AsioMessageSelectors::kAsioResyncRequest => {
            dispatch_driver_message(AsioDriverMessage::ResyncRequest)
        }
        _ => {}
    });
}

impl Devices {
    pub fn new(asio: Arc<sys::Asio>) -> Result<Self, DevicesError> {
        let drivers = asio.driver_names().into_iter();
        Ok(Devices { asio, drivers })
    }
}

impl Iterator for Devices {
    type Item = Device;

    /// Load drivers and return device
    fn next(&mut self) -> Option<Device> {
        loop {
            match self.drivers.next() {
                Some(name) => match self.asio.load_driver(&name) {
                    Ok(driver) => {
                        let driver = Arc::new(driver);
                        // Forward driver-requested resets and resyncs to the
                        // subscribers so a host can recover a session that
                        // died across a system sleep.
                        register_driver_message_dispatch(&driver);
                        // The driver is process-global, so its ASIO buffers
                        // are too: every Device handle for the same driver
                        // must share one stream slot. Without this, a
                        // second Device would dispose and recreate the
                        // buffers while callbacks registered by the first
                        // Device still read them, crashing the process
                        // with a use-after-free on the audio thread.
                        let asio_streams = shared_asio_streams(&driver);
                        let current_buffer_index = shared_current_buffer_index(&driver);
                        return Some(Device {
                            driver,
                            asio_streams,
                            current_buffer_index,
                        });
                    }
                    Err(_) => continue,
                },
                None => return None,
            }
        }
    }
}

pub(crate) fn convert_data_type(ty: &sys::AsioSampleType) -> Option<SampleFormat> {
    let fmt = match *ty {
        sys::AsioSampleType::ASIOSTInt16MSB => SampleFormat::I16,
        sys::AsioSampleType::ASIOSTInt16LSB => SampleFormat::I16,
        sys::AsioSampleType::ASIOSTInt24MSB => SampleFormat::I24,
        sys::AsioSampleType::ASIOSTInt24LSB => SampleFormat::I24,
        sys::AsioSampleType::ASIOSTInt32MSB => SampleFormat::I32,
        sys::AsioSampleType::ASIOSTInt32LSB => SampleFormat::I32,
        sys::AsioSampleType::ASIOSTFloat32MSB => SampleFormat::F32,
        sys::AsioSampleType::ASIOSTFloat32LSB => SampleFormat::F32,
        sys::AsioSampleType::ASIOSTFloat64MSB => SampleFormat::F64,
        sys::AsioSampleType::ASIOSTFloat64LSB => SampleFormat::F64,
        _ => return None,
    };
    Some(fmt)
}

fn default_config_err(e: sys::AsioError) -> DefaultStreamConfigError {
    match e {
        sys::AsioError::NoDrivers | sys::AsioError::HardwareMalfunction => {
            DefaultStreamConfigError::DeviceNotAvailable
        }
        sys::AsioError::NoRate => DefaultStreamConfigError::StreamTypeNotSupported,
        err => {
            let description = format!("{}", err);
            BackendSpecificError { description }.into()
        }
    }
}
