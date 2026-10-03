use std::path::PathBuf;

/// Asset naming contract for GitHub releases. Windows keeps the historical
/// `ralgruM.exe` name; Linux releases publish one AppImage per architecture.
/// Any other platform has no update channel and must never match an asset, so
/// the release parser refuses to offer updates there instead of downloading a
/// binary that cannot run.
pub(super) fn asset_name() -> Option<&'static str> {
    match std::env::consts::OS {
        "windows" => Some("ralgruM.exe"),
        "linux" => match std::env::consts::ARCH {
            "x86_64" => Some("ralgruM-x86_64-linux.AppImage"),
            "aarch64" => Some("ralgruM-aarch64-linux.AppImage"),
            _ => None,
        },
        _ => None,
    }
}

/// Name of the payload staged inside the random update directory. This is an
/// internal updater name rather than the public asset name, and it follows the
/// platform binary suffix the way a cargo build does, which also lets the debug
/// updater test fixture accept the freshly built executable on every platform.
pub(super) fn staged_file_name() -> String {
    format!("ralgruM{}", std::env::consts::EXE_SUFFIX)
}

/// The host file an update replaces. On Linux the AppImage runtime exports
/// `APPIMAGE`, and the mounted squashfs keeps running from its own inode, so
/// the file at that path can be swapped underneath the running process. On
/// every other platform the running executable itself is the install target.
/// Without `$APPIMAGE` there is no stable file to swap, so self-update has to
/// refuse rather than stage an update next to the ephemeral mount.
pub(super) fn install_target() -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .ok_or_else(|| "Automatic updates require the AppImage build".to_owned())
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::current_exe().map_err(|_| "Cannot find the running executable".to_owned())
    }
}
