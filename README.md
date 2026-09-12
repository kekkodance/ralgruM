# ralgruM Desktop

A desktop music player for the Murglar music service.

ralgruM is a third-party client for Murglar. It is not made by, affiliated with, or endorsed by Murglar, Deezer, or SoundCloud. If you have a Murglar account, ralgruM gives you a fast desktop app to search, browse, play, and download your music in one place.

## Who is it for?

ralgruM is primarily intended for:

- **Murglar Pass users**, who get the full experience including lossless playback.
- **People who use Deezer and SoundCloud**, who want to connect those accounts and bring their library, playlists, and recommendations into one player.

You can browse without signing into everything, but signing in unlocks your personal library and the best audio quality.

## What can you do with it?

- Search across Deezer and SoundCloud, and open artists, albums, tracks, and playlists.
- Play music with a full queue, shuffle and repeat, and seamless transitions between tracks.
- View your Deezer library, Flow, and playlists, plus your SoundCloud library.
- Download tracks for offline listening and manage your downloads folder.
- See synced lyrics while you listen.
- Keep playing in the background with system tray and media-key support.
- Optionally show what you are listening to on Discord.

## Getting started

1. Download the latest ralgruM release for Windows and open it.
2. Log in with your Murglar account to unlock the full service.
3. Go to Settings and connect Deezer and SoundCloud if you use them.

That is it. Your logins stay saved on your PC so you do not have to sign in every time.

## Good to know

- ralgruM is an unofficial, community-made app. For Murglar service issues, contact the Murglar developers.
- Some tracks, especially in high quality, require an active Murglar Pass. If you hit a limit, the app will tell you.
- On Windows, ralgruM uses the built-in secure storage for your OS user account to keep your login safe.
- If music stops or a login expires, try signing out and back in from Settings.

---

## For developers

This section is only for people building ralgruM from source. Regular users can stop here and just use the release build.

### Closed-source Murglar backend

The part of the app that talks to the Murglar API is private and cannot be open sourced, at the request of the Murglar developers.

What that means in practice:

- This public repo contains the full player UI, playback, downloads, lyrics, settings, and Deezer/SoundCloud integration.
- The Murglar account, device, and media code lives in a private checkout that is not committed here.
- Without that private checkout, the project still compiles using a public stub, but Murglar login and Murglar playback will report as unavailable. The stub is not a working replacement.

The default private checkout path is `src/murglar_backend/implementation/`. `build.rs` uses its `mod.rs` when present, otherwise it falls back to `src/murglar_backend/stub.rs`. You can point to a different checkout with `RALGRUM_MURGLAR_PRIVATE_DIR`. Relative paths resolve from the repo root, absolute paths are accepted.

### Prerequisites (Windows)

- Rust stable with the MSVC toolchain
- Microsoft C++ Build Tools with the Desktop development with C++ workload
- A Windows SDK installed through Visual Studio Build Tools
- WebView2 Runtime for the embedded login windows
- PowerShell 5.1 or newer

### Build and run

From the repository root:

```powershell
cargo run --locked
```

Or build and launch through the helper script:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run-gpui.ps1
```

Useful flags:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\run-gpui.ps1 -BuildOnly
powershell -ExecutionPolicy Bypass -File scripts\run-gpui.ps1 -Release
```

### Release build

Builds the optimized Windows executable and copies it to `dist/` as `ralgruM.exe`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Bump Patch
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Bump Minor -Prerelease beta.1
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -Version 1.2.3
```

Versions follow strict Semantic Versioning 2.0.0. The new version must be higher than the current one unless `-Force` is passed. `-NoBuild` updates the version files without building.

The Cargo release profile uses opt-level = 3, thin LTO, one codegen unit,
and symbol stripping for a compact optimized executable.

### Checks

```powershell
cargo fmt --package ralgrum-gpui -- --check
cargo metadata --locked --no-deps
cargo check --locked --all-targets
cargo test --locked --all-targets
```

Formatting is scoped to the application package so vendored dependencies keep their upstream formatting.

To check and test the public stub instead of the private backend, point the override at a directory with no `mod.rs`:

```powershell
$env:RALGRUM_MURGLAR_PRIVATE_DIR = 'target/no-private-backend'
cargo check --locked --all-targets
cargo test --locked --all-targets
Remove-Item Env:RALGRUM_MURGLAR_PRIVATE_DIR
```

Removing the override restores the default backend selection.

### Session storage notes

On Windows, the session is stored under `%APPDATA%\ralgruM` in `auth_session.dat` and `auth_session.backup.dat`. Both copies are encrypted with Windows current-user DPAPI in a versioned, size-bounded container (64 KiB session limit, 256 KiB encrypted payload limit). DPAPI protects data in the Windows user context, but it does not protect against code running as that user.
