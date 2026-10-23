use std::{collections::HashSet, process::Command, thread, time::Duration};

/// PCM exposed by the PulseAudio ALSA plugin. Opening it makes the stream a
/// pipewire-pulse client, which is what lets the sink be chosen at runtime
/// without touching the system default.
pub(crate) const PULSE_BRIDGE_PCM: &str = "pulse";
/// How often a freshly opened stream is looked for before giving up. The
/// sink input can take a moment to appear in the pulse registry.
const MOVE_ATTEMPTS: usize = 20;
const MOVE_RETRY_DELAY: Duration = Duration::from_millis(15);

/// A Pulse/PipeWire sink together with the label shown in the picker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LabeledSink {
    pub(crate) name: String,
    pub(crate) label: String,
}

impl LabeledSink {
    fn matches(&self, value: &str) -> bool {
        self.label == value || self.name == value
    }
}

/// Enumerates the sound server's sinks with their human readable
/// descriptions. The node names stay stable across calls; the descriptions
/// are what PipeWire tools such as `wpctl` show.
pub(crate) fn list_sinks() -> Result<Vec<LabeledSink>, String> {
    let listing = run_pactl(&["list", "sinks"])?;
    Ok(label_sinks(parse_pactl_sinks(&listing)))
}

/// Finds the sink whose picker label, or legacy node name, matches `value`.
pub(crate) fn sink_for_label(value: &str) -> Option<LabeledSink> {
    list_sinks()
        .ok()?
        .into_iter()
        .find(|sink| sink.matches(value))
}

/// Moves this process's playback streams onto `sink_name`. The stream is
/// opened through the PulseAudio ALSA bridge first, so the pipewire-pulse
/// server owns it and can move it without changing the system default.
pub(crate) fn move_own_streams_to_sink(sink_name: &str) -> Result<(), String> {
    let mut last_error = None;
    for _ in 0..MOVE_ATTEMPTS {
        match list_own_sink_inputs() {
            Ok(inputs) if !inputs.is_empty() => {
                let mut moved = 0;
                for input in inputs {
                    match run_pactl(&["move-sink-input", &input.to_string(), sink_name]) {
                        Ok(_) => moved += 1,
                        Err(error) => last_error = Some(error),
                    }
                }
                if moved > 0 {
                    return Ok(());
                }
            }
            Ok(_) => {}
            Err(error) => last_error = Some(error),
        }
        thread::sleep(MOVE_RETRY_DELAY);
    }
    Err(last_error.unwrap_or_else(|| {
        "no playback stream of this process was found on the sound server".to_owned()
    }))
}

fn list_own_sink_inputs() -> Result<Vec<u32>, String> {
    let listing = run_pactl(&["list", "sink-inputs"])?;
    Ok(parse_own_sink_inputs(&listing, std::process::id()))
}

fn run_pactl(args: &[&str]) -> Result<String, String> {
    let output = Command::new("pactl")
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("pactl could not be started: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(if stderr.is_empty() {
            format!("pactl exited with {}", output.status)
        } else {
            format!("pactl failed: {stderr}")
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SinkEntry {
    name: String,
    description: String,
}

/// Parses the long `pactl list sinks` format. Short listings only expose the
/// node name, while the long form carries the same description PipeWire
/// shows in its own tools.
fn parse_pactl_sinks(listing: &str) -> Vec<SinkEntry> {
    let mut sinks = Vec::new();
    let mut name = None;
    let mut description = String::new();
    for line in listing.lines() {
        let line = line.trim();
        if line.starts_with("Sink #") {
            flush_sink(&mut sinks, &mut name, &mut description);
            continue;
        }
        if let Some(value) = line.strip_prefix("Name:") {
            name = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("Description:") {
            description = value.trim().to_owned();
        }
    }
    flush_sink(&mut sinks, &mut name, &mut description);
    sinks
}

fn flush_sink(sinks: &mut Vec<SinkEntry>, name: &mut Option<String>, description: &mut String) {
    let Some(name) = name.take() else {
        description.clear();
        return;
    };
    if name.is_empty() {
        description.clear();
        return;
    }
    sinks.push(SinkEntry {
        name,
        description: std::mem::take(description),
    });
}

/// Turns raw sink entries into unique picker labels. Duplicate descriptions
/// get a numeric suffix assigned in node-name order so the same set of sinks
/// always produces the same labels.
fn label_sinks(entries: Vec<SinkEntry>) -> Vec<LabeledSink> {
    let mut entries = entries;
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries.dedup_by(|left, right| left.name == right.name);
    entries.sort_by(|left, right| {
        left.description
            .cmp(&right.description)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut used = HashSet::new();
    let mut labeled = Vec::with_capacity(entries.len());
    for entry in entries {
        let base = if entry.description.is_empty() {
            entry.name.clone()
        } else {
            entry.description
        };
        let mut label = base.clone();
        let mut suffix = 2;
        while !used.insert(label.clone()) {
            label = format!("{base} ({suffix})");
            suffix += 1;
        }
        labeled.push(LabeledSink {
            name: entry.name,
            label,
        });
    }
    labeled.sort_by(|left, right| left.label.cmp(&right.label));
    labeled
}

/// Reads the sink input ids owned by `pid`. Other applications share the
/// listing, so the process id keeps their streams untouched.
fn parse_own_sink_inputs(listing: &str, pid: u32) -> Vec<u32> {
    let mut inputs = Vec::new();
    let mut current = None;
    let mut owned = false;
    for line in listing.lines() {
        let line = line.trim();
        if let Some(id) = line
            .strip_prefix("Sink Input #")
            .and_then(|id| id.parse::<u32>().ok())
        {
            flush_sink_input(&mut inputs, &mut current, &mut owned);
            current = Some(id);
            continue;
        }
        if let Some(value) = line.strip_prefix("application.process.id = ") {
            let value = value.trim().trim_matches('"');
            if value.parse::<u32>().ok() == Some(pid) {
                owned = true;
            }
        }
    }
    flush_sink_input(&mut inputs, &mut current, &mut owned);
    inputs
}

fn flush_sink_input(inputs: &mut Vec<u32>, current: &mut Option<u32>, owned: &mut bool) {
    if let Some(id) = current.take()
        && *owned
    {
        inputs.push(id);
    }
    *owned = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINKS: &str = "\
Sink #72
\tState: RUNNING
\tName: alsa_output.pci-0000_07_00.4.analog-stereo
\tDescription: Computer
\tDriver: PipeWire
\tMonitor Source: alsa_output.pci-0000_07_00.4.analog-stereo.monitor
\tProperties:
\t\tdevice.description = \"Computer\"

Sink #73
\tState: SUSPENDED
\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo
\tDescription: HDMI / DisplayPort
\tDriver: PipeWire
";

    #[test]
    fn parses_names_and_descriptions_from_the_long_listing() {
        assert_eq!(
            parse_pactl_sinks(SINKS),
            [
                SinkEntry {
                    name: "alsa_output.pci-0000_07_00.4.analog-stereo".to_owned(),
                    description: "Computer".to_owned(),
                },
                SinkEntry {
                    name: "alsa_output.pci-0000_01_00.1.hdmi-stereo".to_owned(),
                    description: "HDMI / DisplayPort".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn labels_sinks_by_description_in_a_stable_order() {
        assert_eq!(
            label_sinks(parse_pactl_sinks(SINKS)),
            [
                LabeledSink {
                    name: "alsa_output.pci-0000_07_00.4.analog-stereo".to_owned(),
                    label: "Computer".to_owned(),
                },
                LabeledSink {
                    name: "alsa_output.pci-0000_01_00.1.hdmi-stereo".to_owned(),
                    label: "HDMI / DisplayPort".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn duplicate_descriptions_get_unique_suffixes() {
        let sinks = label_sinks(vec![
            SinkEntry {
                name: "alsa_output.usb-second".to_owned(),
                description: "USB DAC".to_owned(),
            },
            SinkEntry {
                name: "alsa_output.usb-first".to_owned(),
                description: "USB DAC".to_owned(),
            },
        ]);
        assert_eq!(
            sinks,
            [
                LabeledSink {
                    name: "alsa_output.usb-first".to_owned(),
                    label: "USB DAC".to_owned(),
                },
                LabeledSink {
                    name: "alsa_output.usb-second".to_owned(),
                    label: "USB DAC (2)".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn duplicate_labels_keep_advancing_until_they_are_unique() {
        let sinks = label_sinks(vec![
            SinkEntry {
                name: "a".to_owned(),
                description: "Computer".to_owned(),
            },
            SinkEntry {
                name: "b".to_owned(),
                description: "Computer".to_owned(),
            },
            SinkEntry {
                name: "c".to_owned(),
                description: "Computer (2)".to_owned(),
            },
        ]);
        assert_eq!(
            sinks
                .iter()
                .map(|sink| sink.label.as_str())
                .collect::<Vec<_>>(),
            ["Computer", "Computer (2)", "Computer (2) (2)"]
        );
    }

    #[test]
    fn sinks_without_a_description_fall_back_to_the_node_name() {
        let sinks = label_sinks(vec![SinkEntry {
            name: "alsa_output.empty".to_owned(),
            description: String::new(),
        }]);
        assert_eq!(
            sinks,
            [LabeledSink {
                name: "alsa_output.empty".to_owned(),
                label: "alsa_output.empty".to_owned(),
            }]
        );
    }

    #[test]
    fn empty_and_duplicate_names_are_ignored() {
        assert!(parse_pactl_sinks("Sink #1\n\tState: RUNNING\n").is_empty());
        assert_eq!(
            label_sinks(parse_pactl_sinks(
                "Sink #1\n\tName: same\n\tDescription: First\n\
                 Sink #2\n\tName: same\n\tDescription: Second\n"
            )),
            [LabeledSink {
                name: "same".to_owned(),
                label: "First".to_owned(),
            }]
        );
    }

    #[test]
    fn parses_only_sink_inputs_owned_by_the_process() {
        let listing = "\
Sink Input #41
\tSink: 72
\tProperties:
\t\tapplication.name = \"ralgruM\"
\t\tapplication.process.id = \"4242\"

Sink Input #42
\tSink: 72
\tProperties:
\t\tapplication.name = \"Other\"
\t\tapplication.process.id = \"7\"

Sink Input #43
\tSink: 73
\tProperties:
\t\tapplication.process.id = \"4242\"
";
        assert_eq!(parse_own_sink_inputs(listing, 4242), [41, 43]);
        assert!(parse_own_sink_inputs(listing, 99).is_empty());
    }
}
