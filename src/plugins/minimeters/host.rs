use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_host::prelude::*;

use super::tap::Tap;

const PLUGIN_ID: &[u8] = b"com.josephlyncheski.MiniMeters-Audio-Server-CLAP";
const BLOCK: usize = 512;

pub(super) fn validate_audio_server(path: &Path) -> Result<(), String> {
    // SAFETY: The user-installed CLAP binary is loaded only for its factory descriptor.
    let entry = unsafe { PluginEntry::load(path) }.map_err(|error| error.to_string())?;
    let factory = entry
        .get_plugin_factory()
        .ok_or("MiniMeters CLAP has no plugin factory")?;
    if !factory
        .plugin_descriptors()
        .any(|descriptor| descriptor.id().is_some_and(|id| id.to_bytes() == PLUGIN_ID))
    {
        return Err("The installed CLAP is not the MiniMeters Audio Server plugin".into());
    }
    Ok(())
}

struct MeterHost;

struct Callbacks {
    callback: Arc<AtomicBool>,
    restart: Arc<AtomicBool>,
}

impl SharedHandler<'_> for Callbacks {
    fn request_restart(&self) {
        self.restart.store(true, Ordering::Release);
    }
    fn request_process(&self) {}
    fn request_callback(&self) {
        self.callback.store(true, Ordering::Release);
    }
}

impl HostHandlers for MeterHost {
    type Shared<'a> = Callbacks;
    type MainThread<'a> = ();
    type AudioProcessor<'a> = ();
}

pub(super) fn run(path: &Path, tap: Arc<Tap>, stop: Arc<AtomicBool>) -> Result<(), String> {
    // SAFETY: The installed CLAP binary is explicitly selected by the user and remains
    // loaded through the PluginEntry while the instance exists.
    let entry = unsafe { PluginEntry::load(path) }.map_err(|error| error.to_string())?;
    let factory = entry
        .get_plugin_factory()
        .ok_or("MiniMeters CLAP has no plugin factory")?;
    let descriptor = factory
        .plugin_descriptors()
        .find(|descriptor| descriptor.id().is_some_and(|id| id.to_bytes() == PLUGIN_ID))
        .ok_or("The installed CLAP is not the MiniMeters Audio Server plugin")?;
    let plugin_id = descriptor.id().ok_or("MiniMeters CLAP has no plugin ID")?;
    let host_info = HostInfo::new(
        "ralgruM",
        "ralgruM",
        "https://github.com/kekkodance/ralgruM",
        env!("CARGO_PKG_VERSION"),
    )
    .map_err(|error| error.to_string())?;
    let callback = Arc::new(AtomicBool::new(false));
    let restart = Arc::new(AtomicBool::new(false));
    let mut instance = PluginInstance::<MeterHost>::new(
        |_| Callbacks {
            callback: Arc::clone(&callback),
            restart: Arc::clone(&restart),
        },
        |_| (),
        &entry,
        plugin_id,
        &host_info,
    )
    .map_err(|error| error.to_string())?;

    let output_channels = {
        let handle = instance.plugin_handle();
        let ports = handle
            .get_extension::<PluginAudioPorts>()
            .ok_or("MiniMeters Audio Server does not declare audio ports")?;
        if ports.count(&handle, true) != 1 {
            return Err("MiniMeters Audio Server must have one audio input port".into());
        }
        let mut input_info = AudioPortInfoBuffer::new();
        let input = ports
            .get(&handle, 0, true, &mut input_info)
            .ok_or("MiniMeters Audio Server input port could not be read")?;
        if input.channel_count != 2 {
            return Err(format!(
                "MiniMeters Audio Server needs stereo input, found {} channels",
                input.channel_count
            ));
        }
        match ports.count(&handle, false) {
            0 => 0,
            1 => {
                let mut output_info = AudioPortInfoBuffer::new();
                let output = ports
                    .get(&handle, 0, false, &mut output_info)
                    .ok_or("MiniMeters Audio Server output port could not be read")?;
                if output.channel_count != 2 {
                    return Err("MiniMeters Audio Server output is not stereo".into());
                }
                2
            }
            _ => return Err("MiniMeters Audio Server has unsupported output ports".into()),
        }
    };

    let mut sample_rate = 48_000;
    while !stop.load(Ordering::Acquire) {
        let processor = instance
            .activate(
                |_, _| (),
                PluginAudioConfiguration {
                    sample_rate: sample_rate as f64,
                    min_frames_count: BLOCK as u32,
                    max_frames_count: BLOCK as u32,
                },
            )
            .map_err(|error| error.to_string())?;
        let finished = AtomicBool::new(false);
        let result = thread::scope(|scope| {
            let audio = scope.spawn(|| {
                let result = process_audio(
                    processor,
                    &tap,
                    &stop,
                    &restart,
                    sample_rate,
                    output_channels,
                );
                finished.store(true, Ordering::Release);
                result
            });
            while !stop.load(Ordering::Acquire) && !finished.load(Ordering::Acquire) {
                if callback.swap(false, Ordering::AcqRel) {
                    instance.call_on_main_thread_callback();
                }
                thread::sleep(Duration::from_millis(10));
            }
            audio.join()
        });
        let (processor, next_rate, process_error) = match result {
            Ok(result) => result,
            Err(_) => {
                let _ = instance.try_deactivate();
                return Err("MiniMeters audio thread panicked".into());
            }
        };
        instance.deactivate(processor);
        if let Some(error) = process_error {
            return Err(error);
        }
        if let Some(next_rate) = next_rate {
            sample_rate = next_rate;
        }
        restart.store(false, Ordering::Release);
    }
    Ok(())
}

fn process_audio(
    processor: StoppedPluginAudioProcessor<MeterHost>,
    tap: &Tap,
    stop: &AtomicBool,
    restart: &AtomicBool,
    sample_rate: u32,
    output_channels: u32,
) -> (
    StoppedPluginAudioProcessor<MeterHost>,
    Option<u32>,
    Option<String>,
) {
    let mut processor = match processor.start_processing() {
        Ok(processor) => processor,
        Err(error) => {
            return (
                error.into_stopped_processor(),
                None,
                Some("MiniMeters could not start processing audio".into()),
            );
        }
    };
    let mut input = [[0.0f32; BLOCK]; 2];
    let mut output = [[0.0f32; BLOCK]; 2];
    let mut input_ports = AudioPorts::with_capacity(2, 1);
    let mut output_ports =
        AudioPorts::with_capacity(output_channels as usize, usize::from(output_channels != 0));
    let mut events = EventBuffer::new();
    let mut next_rate = None;
    let mut process_error = None;
    let block_duration = Duration::from_secs_f64(BLOCK as f64 / sample_rate as f64);
    while !stop.load(Ordering::Acquire) && !restart.load(Ordering::Acquire) {
        let mut filled = 0;
        let deadline = Instant::now() + block_duration;
        while filled < BLOCK {
            let Some(frame) = tap.frames.pop() else {
                if Instant::now() >= deadline
                    || stop.load(Ordering::Acquire)
                    || restart.load(Ordering::Acquire)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
                continue;
            };
            if frame.sample_rate != sample_rate {
                next_rate = Some(frame.sample_rate);
                break;
            }
            input[0][filled] = frame.left;
            input[1][filled] = frame.right;
            filled += 1;
        }
        if next_rate.is_some() {
            break;
        }
        input[0][filled..].fill(0.0);
        input[1][filled..].fill(0.0);
        let audio_input = input_ports.with_input_buffers([AudioPortBuffer {
            latency: 0,
            channels: AudioPortBufferType::f32_input_only(
                input.iter_mut().map(InputChannel::constant),
            ),
        }]);
        let input_events = InputEvents::empty();
        events.clear();
        let mut output_events = OutputEvents::from_buffer(&mut events);
        let result = if output_channels == 0 {
            let mut audio_output = OutputAudioBuffers::empty();
            processor.process(
                &audio_input,
                &mut audio_output,
                &input_events,
                &mut output_events,
                None,
                None,
            )
        } else {
            let mut audio_output = output_ports.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only(
                    output.iter_mut().map(|channel| channel.as_mut_slice()),
                ),
            }]);
            processor.process(
                &audio_input,
                &mut audio_output,
                &input_events,
                &mut output_events,
                None,
                None,
            )
        };
        if let Err(error) = result {
            process_error = Some(error.to_string());
            break;
        }
    }
    (processor.stop_processing(), next_rate, process_error)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Duration,
    };

    #[cfg(windows)]
    #[test]
    #[ignore = "loads and processes with the locally installed proprietary MiniMeters CLAP"]
    fn installed_audio_server_processes() {
        let path = super::super::discovery::audio_server_path().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let timer_stop = Arc::clone(&stop);
        let timer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            timer_stop.store(true, Ordering::Release);
        });
        let result = super::run(&path, super::super::tap::shared(), stop);
        timer.join().unwrap();
        result.unwrap();
    }
}
