use std::{
    fs::{File, OpenOptions, create_dir_all, read_dir},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use chrono::Utc;

mod redaction;

use redaction::redact_sensitive_message;

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const LOG_TRIM_TARGET_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CAPTURE_FILES: usize = 40;
const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LAUNCHER_LOG_BYTES: u64 = 1024 * 1024;

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();

pub(crate) fn init() -> PathBuf {
    let (path, file) = open_log_file();
    let _ = LOG_FILE.set(Mutex::new(file));
    eprintln!("ralgruM GPUI log: {}", path.display());
    event("INFO", format!("diagnostics log path={}", path.display()));
    install_panic_hook();
    path
}

pub(crate) fn event(level: &str, message: impl AsRef<str>) {
    let thread = std::thread::current();
    let line = format_line(
        &Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        std::process::id(),
        &format!("{:?}", thread.id()),
        thread.name().unwrap_or("unnamed"),
        level,
        message.as_ref(),
    );
    let Some(file) = LOG_FILE.get() else {
        eprint!("{line}");
        let _ = std::io::stderr().flush();
        return;
    };
    if let Ok(mut file) = file.lock() {
        let _ = file.seek(SeekFrom::End(0));
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
        let _ = trim_log_file(&mut file, MAX_LOG_BYTES, LOG_TRIM_TARGET_BYTES);
    }
    eprint!("{line}");
    let _ = std::io::stderr().flush();
}

fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        let location = info
            .location()
            .map(ToString::to_string)
            .unwrap_or_else(|| "unknown location".to_owned());
        event(
            "PANIC",
            format!(
                "payload={payload}; location={location}; backtrace=\n{}",
                std::backtrace::Backtrace::force_capture()
            ),
        );
        default_hook(info);
    }));
}

fn format_line(
    timestamp: &str,
    pid: u32,
    thread_id: &str,
    thread_name: &str,
    level: &str,
    message: &str,
) -> String {
    let thread_name = thread_name.replace(['\r', '\n', ' '], "_");
    let message = redact_sensitive_message(message).replace(['\r', '\n'], "\\n");
    format!(
        "{timestamp} pid={pid} thread={thread_id} thread_name={thread_name} level={level} message={message}\n"
    )
}

fn selected_path(local_app_data: Option<&Path>, temp: &Path) -> PathBuf {
    local_app_data.map_or_else(
        || temp.join("ralgrum-gpui.log"),
        |path| path.join("ralgruM").join("logs").join("gpui.log"),
    )
}

fn open_log_file() -> (PathBuf, File) {
    let temp = std::env::temp_dir();
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let path = selected_path(Some(&local_app_data), &temp);
        if let Ok(mut file) = create_log_file(&path) {
            report_trim_failure(&mut file);
            retain_auxiliary_logs(&path);
            return (path, file);
        }
    }
    let path = selected_path(None, &temp);
    let mut file = create_log_file(&path).unwrap_or_else(|error| {
        panic!(
            "unable to open GPUI diagnostics log {}: {error}",
            path.display()
        )
    });
    report_trim_failure(&mut file);
    retain_auxiliary_logs(&path);
    (path, file)
}

fn create_log_file(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .and_then(|mut file| {
            file.seek(SeekFrom::End(0))?;
            Ok(file)
        })
}

fn trim_log_file(file: &mut File, max_bytes: u64, target_bytes: u64) -> std::io::Result<bool> {
    let length = file.metadata()?.len();
    if length <= max_bytes {
        return Ok(false);
    }

    let target_bytes = target_bytes.min(max_bytes);
    file.seek(SeekFrom::Start(length - target_bytes))?;
    let mut retained = Vec::new();
    file.read_to_end(&mut retained)?;
    if let Some(newline) = retained.iter().position(|byte| *byte == b'\n') {
        retained.drain(..=newline);
    }

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&retained)?;
    file.flush()?;
    Ok(true)
}

fn report_trim_failure(file: &mut File) {
    if let Err(error) = trim_log_file(file, MAX_LOG_BYTES, LOG_TRIM_TARGET_BYTES) {
        eprintln!("ralgruM GPUI log retention failed: {error}");
    }
}

fn retain_auxiliary_logs(log_path: &Path) {
    let Some(directory) = log_path.parent() else {
        return;
    };
    if let Err(error) = prune_capture_logs(directory, MAX_CAPTURE_FILES, MAX_CAPTURE_BYTES) {
        eprintln!("ralgruM GPUI capture retention failed: {error}");
    }
    let launcher_path = directory.join("gpui-launcher.log");
    let Ok(mut launcher) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(launcher_path)
    else {
        return;
    };
    if let Err(error) = trim_log_file(
        &mut launcher,
        MAX_LAUNCHER_LOG_BYTES,
        LOG_TRIM_TARGET_BYTES.min(MAX_LAUNCHER_LOG_BYTES),
    ) {
        eprintln!("ralgruM GPUI launcher log retention failed: {error}");
    }
}

fn prune_capture_logs(
    directory: &Path,
    max_files: usize,
    max_bytes: u64,
) -> std::io::Result<usize> {
    let mut captures = read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            is_capture_name(&name).then_some((name, entry.path()))
        })
        .collect::<Vec<_>>();
    captures.sort_unstable_by(|left, right| right.0.cmp(&left.0));

    let preserve_count = max_files.min(2);
    let old_file_limit = max_files.saturating_sub(preserve_count);
    let mut retained_old_files = 0;
    let mut retained_old_bytes = 0;
    let mut removed = 0;
    let mut first_error = None;
    for (index, (_, path)) in captures.into_iter().enumerate() {
        if index < preserve_count {
            continue;
        }

        let length = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }
        };
        if retained_old_files < old_file_limit
            && length <= max_bytes.saturating_sub(retained_old_bytes)
        {
            retained_old_files += 1;
            retained_old_bytes += length;
            continue;
        }

        match std::fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(removed)
}

fn is_capture_name(name: &str) -> bool {
    let Some(stamp) = name.strip_prefix("gpui-").and_then(|name| {
        name.strip_suffix(".stdout.log")
            .or_else(|| name.strip_suffix(".stderr.log"))
    }) else {
        return false;
    };
    let bytes = stamp.as_bytes();
    bytes.len() == 19
        && bytes[8] == b'T'
        && bytes[18] == b'Z'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 8 || index == 18 || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{
        create_log_file, format_line, is_capture_name, prune_capture_logs, selected_path,
        trim_log_file,
    };
    use std::{fs::write, io::Write, path::Path};

    #[test]
    fn formats_one_sanitized_event_per_line() {
        assert_eq!(
            format_line(
                "2026-08-20T12:34:56.789Z",
                42,
                "ThreadId(7)",
                "render thread",
                "INFO",
                "opened\nsettings",
            ),
            "2026-08-20T12:34:56.789Z pid=42 thread=ThreadId(7) thread_name=render_thread level=INFO message=opened\\nsettings\n"
        );
    }

    #[test]
    fn selects_stable_path_and_temp_fallback() {
        assert_eq!(
            selected_path(
                Some(Path::new("C:/Users/test/AppData/Local")),
                Path::new("C:/Temp")
            ),
            Path::new("C:/Users/test/AppData/Local/ralgruM/logs/gpui.log")
        );
        assert_eq!(
            selected_path(None, Path::new("C:/Temp")),
            Path::new("C:/Temp/ralgrum-gpui.log")
        );
    }

    #[test]
    fn opens_existing_log_for_append() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("logs").join("gpui.log");
        let mut first = create_log_file(&path).unwrap();
        first.write_all(b"one\n").unwrap();
        drop(first);
        let mut second = create_log_file(&path).unwrap();
        second.write_all(b"two\n").unwrap();
        drop(second);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn trims_oldest_complete_lines_when_the_log_exceeds_its_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpui.log");
        let mut file = create_log_file(&path).unwrap();
        file.write_all(b"old line\nnew line\nlatest line\n")
            .unwrap();

        assert!(trim_log_file(&mut file, 20, 16).unwrap());

        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.len() <= 16);
        assert!(!content.contains("old line"));
        assert!(content.contains("latest line"));
    }

    #[test]
    fn leaves_logs_at_or_below_the_limit_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpui.log");
        let mut file = create_log_file(&path).unwrap();
        file.write_all(b"one\ntwo\n").unwrap();

        assert!(!trim_log_file(&mut file, 8, 6).unwrap());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn hysteresis_prevents_retrimming_small_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("gpui.log");
        let mut file = create_log_file(&path).unwrap();
        file.write_all(b"01234567890123456789\n").unwrap();

        assert!(trim_log_file(&mut file, 20, 16).unwrap());
        file.write_all(b"tiny\n").unwrap();
        assert!(!trim_log_file(&mut file, 20, 16).unwrap());
    }

    #[test]
    fn capture_name_requires_the_exact_launcher_pattern() {
        assert!(is_capture_name("gpui-20260902T123456789Z.stdout.log"));
        assert!(is_capture_name("gpui-20260902T123456789Z.stderr.log"));
        for name in [
            "gpui-20260902T12345678Z.stdout.log",
            "gpui-20260902T123456789Z.txt",
            "gpui-20260902T123456789Z.stdout.log.bak",
            "gpui-20260902T123456789Z.stdout.log.png",
            "gpui-20260902T123456789z.stdout.log",
            "gpui-20260902T123456789Z.stdout.log ",
        ] {
            assert!(!is_capture_name(name), "unexpected match for {name}");
        }
    }

    #[test]
    fn capture_pruning_keeps_newest_exact_matches_and_unrelated_files() {
        let directory = tempfile::tempdir().unwrap();
        for stamp in ["000000001", "000000002", "000000003"] {
            for stream in ["stdout", "stderr"] {
                write(
                    directory
                        .path()
                        .join(format!("gpui-20260902T{stamp}Z.{stream}.log")),
                    b"capture",
                )
                .unwrap();
            }
        }
        write(directory.path().join("gpui-screenshot.png"), b"keep").unwrap();
        write(
            directory
                .path()
                .join("gpui-20260902T000000004Z.stdout.log.bak"),
            b"keep",
        )
        .unwrap();

        assert_eq!(prune_capture_logs(directory.path(), 2, 1024).unwrap(), 4);
        assert!(
            directory
                .path()
                .join("gpui-20260902T000000003Z.stdout.log")
                .exists()
        );
        assert!(
            directory
                .path()
                .join("gpui-20260902T000000003Z.stderr.log")
                .exists()
        );
        assert!(directory.path().join("gpui-screenshot.png").exists());
        assert!(
            directory
                .path()
                .join("gpui-20260902T000000004Z.stdout.log.bak")
                .exists()
        );
    }

    #[test]
    fn capture_pruning_enforces_old_capture_byte_budget_but_keeps_newest_pair() {
        let directory = tempfile::tempdir().unwrap();
        for (stamp, contents) in [
            ("000000001", b"old".as_slice()),
            ("000000002", b"huge-old-capture".as_slice()),
            ("000000003", b"newest-capture".as_slice()),
        ] {
            for stream in ["stdout", "stderr"] {
                write(
                    directory
                        .path()
                        .join(format!("gpui-20260902T{stamp}Z.{stream}.log")),
                    contents,
                )
                .unwrap();
            }
        }

        assert_eq!(prune_capture_logs(directory.path(), 6, 5).unwrap(), 3);
        for stream in ["stdout", "stderr"] {
            assert!(
                directory
                    .path()
                    .join(format!("gpui-20260902T000000003Z.{stream}.log"))
                    .exists()
            );
        }
        for stream in ["stdout", "stderr"] {
            assert!(
                !directory
                    .path()
                    .join(format!("gpui-20260902T000000002Z.{stream}.log"))
                    .exists()
            );
        }
        assert!(
            directory
                .path()
                .join("gpui-20260902T000000001Z.stdout.log")
                .exists()
        );
    }
}
