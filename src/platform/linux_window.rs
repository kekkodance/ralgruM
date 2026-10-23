//! KWin window management for the Linux desktop.
//!
//! GPUI's `set_always_on_top` is a no-op on Linux, and Wayland activation
//! does not reliably clear a minimized main window. Both requests are routed
//! through a short-lived KWin script over D-Bus, which works on KWin X11 and
//! Wayland sessions alike.

use std::{
    env, fs, process,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};

use zbus::blocking::Connection;

use super::{MAIN_WINDOW_TITLE, POPOUT_WINDOW_TITLE};

const KWIN_SERVICE: &str = "org.kde.KWin";
const KWIN_SCRIPTING_PATH: &str = "/Scripting";
const KWIN_SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";
const KWIN_SCRIPT_INTERFACE: &str = "org.kde.kwin.Script";
/// A freshly opened window may not be in the compositor's stacking order on
/// the frame that requests a state change, so let it settle first.
const WINDOW_SETTLE_DELAY: Duration = Duration::from_millis(150);

static REQUESTS: OnceLock<Sender<WindowRequest>> = OnceLock::new();
static UNSUPPORTED_WARNING: AtomicBool = AtomicBool::new(false);
static SCRIPT_COUNTER: AtomicU64 = AtomicU64::new(0);
static SCRIPT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
enum WindowRequest {
    AlwaysOnTop(bool),
    RestoreMain,
}

/// Pins or unpins the detached sidebar window through KWin.
pub(crate) fn set_always_on_top(always_on_top: bool) {
    send(WindowRequest::AlwaysOnTop(always_on_top));
}

/// Clears the minimized state and focuses the main window through KWin.
pub(crate) fn restore_main_window() {
    send(WindowRequest::RestoreMain);
}

fn send(request: WindowRequest) {
    let sender = REQUESTS.get_or_init(|| {
        let (sender, receiver) = channel();
        let spawned = thread::Builder::new()
            .name("kwin-window".to_owned())
            .spawn(move || {
                for request in receiver {
                    handle_request(request);
                }
            });
        if let Err(error) = spawned {
            crate::diagnostics::event(
                "WARN",
                format!("could not start the KWin window worker: {error}"),
            );
        }
        sender
    });
    if sender.send(request).is_err() {
        crate::diagnostics::event("WARN", "KWin window worker stopped unexpectedly");
    }
}

fn handle_request(request: WindowRequest) {
    if !is_kwin() {
        warn_unsupported_compositor();
        return;
    }
    thread::sleep(WINDOW_SETTLE_DELAY);
    let (script, success) = match request {
        WindowRequest::AlwaysOnTop(always_on_top) => (
            always_on_top_script(always_on_top),
            format!("KWin applied always-on-top={always_on_top} to the popout"),
        ),
        WindowRequest::RestoreMain => (
            restore_main_script(),
            "KWin restored and focused the main window".to_owned(),
        ),
    };
    match execute_script(&script) {
        Ok(()) => crate::diagnostics::event("INFO", success),
        Err(error) => {
            crate::diagnostics::event("WARN", format!("KWin window request failed: {error}"))
        }
    }
}

fn always_on_top_script(always_on_top: bool) -> String {
    format!(
        "{}\n\
         for (var index = 0; index < ralgrumClients.length; index++) {{\n\
         \x20   var client = ralgrumClients[index];\n\
         \x20   if (client.caption.indexOf({title}) === 0) {{\n\
         \x20       client.keepAbove = {value};\n\
         \x20   }}\n\
         }}\n",
        clients_script(),
        title = js_string(POPOUT_WINDOW_TITLE),
        value = always_on_top,
    )
}

fn restore_main_script() -> String {
    format!(
        "{}\n\
         for (var index = 0; index < ralgrumClients.length; index++) {{\n\
         \x20   var client = ralgrumClients[index];\n\
         \x20   if (client.caption.indexOf({title}) === 0 && client.caption.indexOf({excluded}) !== 0) {{\n\
         \x20       client.minimized = false;\n\
         \x20       if (typeof workspace.activeWindow !== 'undefined') {{\n\
         \x20           workspace.activeWindow = client;\n\
         \x20       }} else if (typeof workspace.activeClient !== 'undefined') {{\n\
         \x20           workspace.activeClient = client;\n\
         \x20       }}\n\
         \x20       break;\n\
         \x20   }}\n\
         }}\n",
        clients_script(),
        title = js_string(MAIN_WINDOW_TITLE),
        excluded = js_string(POPOUT_WINDOW_TITLE),
    )
}

fn clients_script() -> &'static str {
    "var ralgrumClients = (typeof workspace.stackingOrder !== 'undefined') \
     ? workspace.stackingOrder : workspace.clientList();"
}

fn js_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

fn execute_script(script: &str) -> Result<(), String> {
    let counter = SCRIPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("ralgrum-{}-{counter}", process::id());
    let path = env::temp_dir().join(format!("{name}.js"));
    fs::write(&path, script)
        .map_err(|error| format!("could not write the KWin script: {error}"))?;
    let path = path.to_string_lossy().into_owned();

    let result = execute_script_file(&path, &name);
    let _ = fs::remove_file(&path);
    result
}

fn execute_script_file(path: &str, name: &str) -> Result<(), String> {
    // KWin can process only one script registration at a time; serializing
    // the calls also keeps rapid toggles in the order the user requested.
    let _guard = SCRIPT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let connection = Connection::session()
        .map_err(|error| format!("could not reach the session bus: {error}"))?;
    let reply = connection
        .call_method(
            Some(KWIN_SERVICE),
            KWIN_SCRIPTING_PATH,
            Some(KWIN_SCRIPTING_INTERFACE),
            "loadScript",
            &(path, name),
        )
        .map_err(|error| format!("KWin refused to load the script: {error}"))?;
    let script_id: i32 = reply
        .body()
        .deserialize()
        .map_err(|error| format!("KWin returned an invalid script id: {error}"))?;
    let script_path = format!("/Scripting/Script{script_id}");
    connection
        .call_method(
            Some(KWIN_SERVICE),
            script_path.as_str(),
            Some(KWIN_SCRIPT_INTERFACE),
            "run",
            &(),
        )
        .map_err(|error| format!("KWin refused to run the script: {error}"))?;
    connection
        .call_method(
            Some(KWIN_SERVICE),
            KWIN_SCRIPTING_PATH,
            Some(KWIN_SCRIPTING_INTERFACE),
            "unloadScript",
            &(name,),
        )
        .map_err(|error| format!("KWin refused to unload the script: {error}"))?;
    Ok(())
}

fn is_kwin() -> bool {
    if env::var_os("KDE_FULL_SESSION").is_some_and(|value| !value.is_empty()) {
        return true;
    }
    ["XDG_CURRENT_DESKTOP", "DESKTOP_SESSION"]
        .iter()
        .filter_map(|name| env::var(*name).ok())
        .any(|value| {
            value.split([':', ';']).any(|part| {
                part.eq_ignore_ascii_case("KDE") || part.to_ascii_lowercase().contains("plasma")
            })
        })
}

fn warn_unsupported_compositor() {
    if UNSUPPORTED_WARNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let desktop = env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unknown".to_owned());
    crate::diagnostics::event(
        "WARN",
        format!(
            "always-on-top and tray restore need KWin; current desktop is {desktop}, so the popout stays in the normal stacking order"
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_on_top_script_targets_the_popout_caption() {
        let script = always_on_top_script(true);

        assert!(script.contains("client.caption.indexOf('ralgruM Popout') === 0"));
        assert!(script.contains("client.keepAbove = true;"));
        assert!(script.contains("workspace.stackingOrder"));
        assert!(script.contains("workspace.clientList()"));
    }

    #[test]
    fn restore_script_excludes_the_popout_and_clears_minimized() {
        let script = restore_main_script();

        assert!(script.contains("client.caption.indexOf('ralgruM') === 0"));
        assert!(script.contains("client.caption.indexOf('ralgruM Popout') !== 0"));
        assert!(script.contains("client.minimized = false;"));
        assert!(script.contains("workspace.activeWindow = client;"));
        assert!(script.contains("workspace.activeClient = client;"));
    }

    #[test]
    fn script_strings_escape_quotes() {
        assert_eq!(js_string("ralgruM Popout"), "'ralgruM Popout'");
        assert_eq!(js_string("it's"), "'it\\'s'");
    }
}
