//! `ralgrum://` scheme registration for Linux.
//!
//! Windows registers the scheme in HKCU; Linux installs a `.desktop` file
//! with `MimeType=x-scheme-handler/ralgrum` into the user applications
//! directory, exports the app icon into the hicolor theme, and refreshes
//! the desktop database. Everything is best effort: an unwritable home or a
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::diagnostics;

const SCHEME: &str = "ralgrum";
const DESKTOP_FILE_NAME: &str = "ralgruM.desktop";
const APP_ID: &str = "ralgruM";
const ICON_NAME: &str = "ralgruM";
const ICON_SIZE_DIR: &str = "512x512";
/// The icon is installed from the shared asset directory rather than an
/// embedded copy so the shipped icon never drifts from `assets/app-icon.png`.
const ICON_SOURCE: &str = "assets/app-icon.png";

/// The registration content, kept separate from installation so the desktop
/// file text can be unit tested without touching the filesystem.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationPlan {
    exec_line: String,
    icon: String,
}

impl RegistrationPlan {
    fn for_executable(path: &Path) -> Option<Self> {
        let executable = path.to_str()?.to_owned();
        Some(Self {
            // A bare `%u` field code receives the whole URL including the
            // ralgrum:// scheme as one argument. Prefixing a literal scheme
            // would double it (`ralgrum://ralgrum://open?...`) and the
            // parser would reject the activation.
            exec_line: format!("{} %u", quote_desktop_argument(&executable)),
            icon: ICON_NAME.to_owned(),
        })
    }

    /// The desktop entry body in the same order the AppImage build writes
    /// it, so a user comparing the installed file with the packaged one
    /// sees a stable diff.
    fn desktop_file(&self) -> String {
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={APP_ID}\n\
             Comment=ralgruM music player\n\
             Exec={}\n\
             Icon={}\n\
             Terminal=false\n\
             Categories=Audio;AudioVideo;\n\
             MimeType=x-scheme-handler/{SCHEME};\n\
             StartupWMClass={APP_ID}\n",
            self.exec_line, self.icon
        )
    }
}

/// Quotes one argument of a desktop entry `Exec` line. The spec says
/// arguments containing a reserved character must be quoted by enclosing
/// them in double quotes and escaping double quotes, backticks, dollar
/// signs and backslashes with a preceding backslash.
fn quote_desktop_argument(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// Writes the plan under `$XDG_DATA_HOME` (or `~/.local/share` as the spec
/// fallback). Returns the base directory on success so callers can chain
/// the icon export.
fn install_desktop_file(plan: &RegistrationPlan) -> Option<PathBuf> {
    let data_home = data_home()?;
    let directory = data_home.join("applications");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        diagnostics::event(
            "WARN",
            format!(
                "Linux {SCHEME} protocol registration failed: {directory:?} could not be created: {error}"
            ),
        );
        return None;
    }
    let target = directory.join(DESKTOP_FILE_NAME);
    let temporary = directory.join(format!(".{DESKTOP_FILE_NAME}.tmp"));
    // Write through a temporary file and rename so a crash mid-write never
    // leaves a truncated desktop entry behind.
    if let Err(error) = std::fs::write(&temporary, plan.desktop_file()) {
        diagnostics::event(
            "WARN",
            format!(
                "Linux {SCHEME} protocol registration failed: {temporary:?} could not be written: {error}"
            ),
        );
        return None;
    }
    if let Err(error) = std::fs::rename(&temporary, &target) {
        diagnostics::event(
            "WARN",
            format!(
                "Linux {SCHEME} protocol registration failed: {target:?} could not be installed: {error}"
            ),
        );
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    Some(data_home)
}

/// Copies the shipped app icon into the hicolor icon theme so `Icon=ralgruM`
/// resolves on desktops that do not inspect the executable directory.
fn install_icon(data_home: &Path) {
    let source = Path::new(ICON_SOURCE);
    let target = data_home
        .join("icons")
        .join("hicolor")
        .join(ICON_SIZE_DIR)
        .join("apps")
        .join(format!("{ICON_NAME}.png"));
    let exe_directory = match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        Some(directory) => directory,
        None => {
            diagnostics::event(
                "WARN",
                "Linux ralgrum:// icon installation skipped: executable path has no parent directory",
            );
            return;
        }
    };
    let source = exe_directory.join(source);
    if let Err(error) = std::fs::create_dir_all(target.parent().unwrap_or(&target)) {
        diagnostics::event(
            "WARN",
            format!(
                "Linux {SCHEME} icon installation failed: {} could not be created: {error}",
                target.parent().unwrap_or(&target).display()
            ),
        );
        return;
    }
    if let Err(error) = std::fs::copy(&source, &target) {
        diagnostics::event(
            "WARN",
            format!(
                "Linux {SCHEME} icon installation failed: {} could not be copied to {}: {error}",
                source.display(),
                target.display()
            ),
        );
    }
}

/// Refreshes the MIME application database so the new handler is picked up
/// without a re-login. The tool is optional on minimal installs, so a
/// missing binary is fine; the desktop environment rescans on demand too.
fn refresh_desktop_database(data_home: &Path) {
    let applications = data_home.join("applications");
    let status = Command::new("update-desktop-database")
        .arg(&applications)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if let Err(error) = status {
        diagnostics::event(
            "INFO",
            format!("update-desktop-database is not available: {error}"),
        );
    }
}

/// `$XDG_DATA_HOME` per the basedir spec, falling back to `~/.local/share`
/// when the variable is unset. The `dirs` crate already implements the
/// fallback, including its requirement that an unset or relative XDG value
/// is ignored.
fn data_home() -> Option<PathBuf> {
    let home = dirs::data_dir()?;
    if home.as_os_str().is_empty() {
        return None;
    }
    Some(home)
}

/// Installs or refreshes the `ralgrum://` handler.
pub(crate) fn sync_registration() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            diagnostics::event(
                "WARN",
                format!("Linux {SCHEME} protocol registration skipped: {error}"),
            );
            return;
        }
    };
    let Some(plan) = RegistrationPlan::for_executable(&executable) else {
        diagnostics::event(
            "WARN",
            format!("Linux {SCHEME} protocol registration skipped: executable path is not UTF-8"),
        );
        return;
    };
    let Some(data_home) = install_desktop_file(&plan) else {
        return;
    };
    install_icon(&data_home);
    refresh_desktop_database(&data_home);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_quotes_executable_paths_with_spaces() {
        let plan =
            RegistrationPlan::for_executable(Path::new("/opt/ralgruM App/bin/ralgruM")).unwrap();
        assert_eq!(plan.exec_line, r#""/opt/ralgruM App/bin/ralgruM" %u"#);
    }

    #[test]
    fn plan_escapes_reserved_characters() {
        assert_eq!(
            quote_desktop_argument(r#"C:\weird"path`$x"#),
            r#""C:\\weird\"path\`\$x""#
        );
    }

    #[test]
    fn desktop_file_contains_scheme_and_expected_keys() {
        let plan = RegistrationPlan::for_executable(Path::new("/usr/bin/ralgruM")).unwrap();
        let desktop = plan.desktop_file();
        assert!(desktop.contains("[Desktop Entry]\n"));
        assert!(desktop.contains("MimeType=x-scheme-handler/ralgrum;\n"));
        assert!(desktop.contains("StartupWMClass=ralgruM\n"));
        assert!(desktop.contains("Categories=Audio;AudioVideo;\n"));
        assert!(desktop.contains("Terminal=false\n"));
        assert!(desktop.contains("Name=ralgruM\n"));
        assert!(desktop.contains("Comment=ralgruM music player\n"));
        assert!(desktop.contains("Icon=ralgruM\n"));
        assert!(desktop.contains(r#"Exec="/usr/bin/ralgruM" %u"#));
        // The icon key is thematic, not a path; a bare name lets the desktop
        // environment pick the best size from the hicolor theme.
        assert_eq!(plan.icon, "ralgruM");
    }

    #[test]
    fn desktop_file_for_plain_paths_has_no_needless_quotes() {
        let plan = RegistrationPlan::for_executable(Path::new("/usr/bin/ralgruM")).unwrap();
        assert_eq!(plan.exec_line, r#""/usr/bin/ralgruM" %u"#);
    }

    #[test]
    fn non_utf8_executable_paths_are_rejected() {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::OsStr::from_bytes(b"/usr/bin/ralgr\xFFm");
        assert!(RegistrationPlan::for_executable(Path::new(path)).is_none());
    }

    #[test]
    fn scheme_constants_are_stable() {
        assert_eq!(SCHEME, "ralgrum");
        assert_eq!(DESKTOP_FILE_NAME, "ralgruM.desktop");
    }
}
