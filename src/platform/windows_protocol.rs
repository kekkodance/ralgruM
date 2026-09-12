use std::path::Path;

use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows_registry::CURRENT_USER;

const SCHEME: &str = "ralgrum";
const ROOT_KEY: &str = r"Software\Classes\ralgrum";
const DEFAULT_ICON_KEY: &str = r"Software\Classes\ralgrum\DefaultIcon";
const OPEN_COMMAND_KEY: &str = r"Software\Classes\ralgrum\shell\open\command";
const URL_PROTOCOL_VALUE: &str = "URL Protocol";
const PROTOCOL_DESCRIPTION: &str = "URL:ralgruM Protocol";

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegistrationPlan {
    default_icon: String,
    open_command: String,
}

impl RegistrationPlan {
    fn for_executable(path: &Path) -> Option<Self> {
        let executable = path.to_str()?.to_owned();
        let quoted_executable = quote_windows_argument(&executable);
        Some(Self {
            default_icon: format!("{quoted_executable},0"),
            open_command: format!(r#"{quoted_executable} --open-url="%1""#),
        })
    }

    fn values(&self) -> [RegistryValue<'_>; 4] {
        [
            RegistryValue {
                key: DEFAULT_ICON_KEY,
                name: "",
                value: &self.default_icon,
            },
            RegistryValue {
                key: OPEN_COMMAND_KEY,
                name: "",
                value: &self.open_command,
            },
            RegistryValue {
                key: ROOT_KEY,
                name: "",
                value: PROTOCOL_DESCRIPTION,
            },
            RegistryValue {
                key: ROOT_KEY,
                name: URL_PROTOCOL_VALUE,
                value: "",
            },
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegistryValue<'a> {
    key: &'static str,
    name: &'static str,
    value: &'a str,
}

fn quote_windows_argument(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            character => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

fn write_registration(plan: &RegistrationPlan) -> windows_registry::Result<()> {
    for value in plan.values() {
        CURRENT_USER
            .create(value.key)?
            .set_string(value.name, value.value)?;
    }
    Ok(())
}

fn notify_shell() {
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
}

pub(crate) fn sync_registration() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            crate::diagnostics::event(
                "WARN",
                format!("Windows {SCHEME} protocol registration skipped: {error}"),
            );
            return;
        }
    };
    let Some(plan) = RegistrationPlan::for_executable(&executable) else {
        crate::diagnostics::event(
            "WARN",
            format!("Windows {SCHEME} protocol registration skipped: executable path is not UTF-8"),
        );
        return;
    };
    if let Err(error) = write_registration(&plan) {
        crate::diagnostics::event(
            "WARN",
            format!("Windows {SCHEME} protocol registration failed: {error}"),
        );
        return;
    }
    notify_shell();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_quotes_executable_paths_with_spaces_and_unicode() {
        let plan = RegistrationPlan::for_executable(Path::new(
            r#"C:\Users\Kekko\Music Apps\ralgruM 日本.exe"#,
        ))
        .unwrap();

        assert_eq!(
            plan.default_icon,
            r#""C:\Users\Kekko\Music Apps\ralgruM 日本.exe",0"#
        );
        assert_eq!(
            plan.open_command,
            r#""C:\Users\Kekko\Music Apps\ralgruM 日本.exe" --open-url="%1""#
        );
    }

    #[test]
    fn plan_escapes_trailing_backslashes_inside_quoted_paths() {
        assert_eq!(quote_windows_argument(r#"C:\folder\"#), r#""C:\folder\\""#);
    }

    #[test]
    fn plan_contains_expected_values_in_safe_publication_order() {
        let plan = RegistrationPlan::for_executable(Path::new(r#"C:\ralgruM.exe"#)).unwrap();
        assert_eq!(
            plan.values(),
            [
                RegistryValue {
                    key: DEFAULT_ICON_KEY,
                    name: "",
                    value: r#""C:\ralgruM.exe",0"#,
                },
                RegistryValue {
                    key: OPEN_COMMAND_KEY,
                    name: "",
                    value: r#""C:\ralgruM.exe" --open-url="%1""#,
                },
                RegistryValue {
                    key: ROOT_KEY,
                    name: "",
                    value: PROTOCOL_DESCRIPTION,
                },
                RegistryValue {
                    key: ROOT_KEY,
                    name: URL_PROTOCOL_VALUE,
                    value: "",
                },
            ]
        );
    }

    #[test]
    fn plan_is_scoped_to_current_user_classes() {
        assert_eq!(ROOT_KEY, r"Software\Classes\ralgrum");
        assert_eq!(SCHEME, "ralgrum");
    }
}
