#!/usr/bin/env bash
# Packages the ralgruM Linux build as ralgruM-<arch>-linux.AppImage.
# Runs on a Linux host (CI) with the release binary already built at
# target/release/ralgruM. Uses linuxdeploy for AppDir dependency bundling
# and appimagetool for the final artifact.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
arch="$(uname -m)"
case "$arch" in
  x86_64|aarch64) ;;
  *) echo "Unsupported build architecture: $arch" >&2; exit 1 ;;
esac

# linuxdeploy's continuous channel is a moving target, so the download is
# pinned by SHA-256. A hash mismatch means the upstream publish moved, and
# the failure points at the pin instead of producing a silently different
# release artifact. Bump both together.
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$arch.AppImage"
declare -A LINUXDEPLOY_SHA256=(
  [x86_64]="8aea8da0f7f7039d2a2cecb14657d752a222a5e1d3825caeef186c82f751cdd1"
  [aarch64]="5c1fddf96066891e829831cac0d84424690f3b22846c7f8f1bb9990a5c6c73f4"
)

# appimagetool is pinned to a tagged release so packaging does not change
# under a moving continuous build. It otherwise downloads its runtime from the
# moving type2-runtime continuous channel, so that download is pinned too.
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage"
declare -A APPIMAGETOOL_SHA256=(
  [x86_64]="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"
  [aarch64]="f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158"
)
TYPE2_RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$arch"
declare -A TYPE2_RUNTIME_SHA256=(
  [x86_64]="617aa77314d57c8b8e4fae9c3f0b35af7b30d5529cb69e98f64f92510ed98a46"
  [aarch64]="9a72a744d221576c9347ec074243acc5c3c7568896b12714e0e82e9f0bf47900"
)

binary="$root/target/release/ralgruM"
[ -x "$binary" ] || { echo "Missing release binary at $binary" >&2; exit 1; }
appindicator_library="$(ldconfig -p | awk '$1 ~ /^lib(ayatana-)?appindicator3\.so\.1$/ { print $NF; exit }' || true)"
[ -n "$appindicator_library" ] || {
  echo "Missing libayatana-appindicator3.so.1; install libayatana-appindicator3-1 before packaging" >&2
  exit 1
}

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
appdir="$workdir/AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/512x512/apps"

install -m 0755 "$binary" "$appdir/usr/bin/ralgruM"
install -m 0644 "$root/assets/app-icon-512.png" "$appdir/usr/share/icons/hicolor/512x512/apps/ralgruM.png"
cat > "$appdir/usr/share/applications/ralgruM.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=ralgruM
Comment=ralgruM music player
Exec=ralgruM
Icon=ralgruM
Terminal=false
Categories=Audio;AudioVideo;
MimeType=x-scheme-handler/ralgrum;
StartupWMClass=ralgruM
EOF

# WebKitGTK runs rendering and networking in separate helper processes.
# linuxdeploy only sees the shared library through ldd, so the helpers are not
# deployed automatically. The library also looks for them at an absolute path
# baked in at build time. Keep the AppDir as the process working directory and
# rewrite that path to an equal-length relative path so the helpers resolve
# inside the mounted AppImage.
cat > "$workdir/AppRun" <<'EOF'
#!/bin/sh
set -eu
if [ -n "${APPDIR:-}" ]; then
  appdir="$APPDIR"
else
  appdir="$(dirname "$(readlink -f "$0")")"
fi

# The AppImage runtime reports the launch directory in OWD. Extracted runs do
# not set it, so fall back to the current directory.
if [ -n "${OWD:-}" ] && [ -d "$OWD" ]; then
  original_cwd="$OWD"
else
  original_cwd="$(pwd -P 2>/dev/null || pwd)"
fi
export RALGRUM_ORIGINAL_CWD="$original_cwd"

# The WebKit helper processes are started from the AppImage mount and must
# load the bundled libraries, not whatever the host distribution provides.
export LD_LIBRARY_PATH="$appdir/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# WebKit's bubblewrap sandbox mounts the host system directories and cannot
# see the AppImage mount where the bundled helpers and libraries live. The
# helpers still run as separate processes, but without the OS-level sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="${WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS:-1}"

export APPDIR="$appdir"
cd "$appdir"
exec "$appdir/usr/bin/ralgruM" "$@"
EOF
chmod 0755 "$workdir/AppRun"

linuxdeploy="$workdir/linuxdeploy-$arch.AppImage"
curl -fsSL -o "$linuxdeploy" "$LINUXDEPLOY_URL"
echo "${LINUXDEPLOY_SHA256[$arch]}  $linuxdeploy" | sha256sum -c -
chmod +x "$linuxdeploy"

# Deploy only. The library patch below must be the last write to the AppDir;
# running linuxdeploy again would rescan and potentially replace it.
DEPLOY_GTK_VERSION=3 NO_STRIP=true "$linuxdeploy" \
  --appdir "$appdir" \
  --desktop-file "$appdir/usr/share/applications/ralgruM.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/512x512/apps/ralgruM.png" \
  --library "$appindicator_library" \
  --custom-apprun "$workdir/AppRun"

webkit_lib="$(find "$appdir/usr/lib" -maxdepth 2 -name 'libwebkit2gtk-4.1.so.0' -print -quit)"
[ -n "$webkit_lib" ] || { echo "linuxdeploy did not bundle libwebkit2gtk-4.1.so.0" >&2; exit 1; }

# The library contains the base helper directory and the same path with
# /injected-bundle/ appended. Matching the base path is enough for both.
baked_webkit_path="$(grep -a -o -E '/usr/(lib|lib64|libexec)/[A-Za-z0-9._/-]+/webkit2gtk-4\.1' "$webkit_lib" | sort -u | head -n1 || true)"
[ -n "$baked_webkit_path" ] || {
  echo "Could not find the WebKit helper path in $webkit_lib" >&2
  exit 1
}
case "$baked_webkit_path" in
  /usr/*) ;;
  *)
    echo "Unsupported WebKit helper path: $baked_webkit_path" >&2
    exit 1
    ;;
esac
relative_webkit_path="usr//${baked_webkit_path#/usr/}"
[ "${#baked_webkit_path}" -eq "${#relative_webkit_path}" ] || {
  echo "WebKit path replacement must keep the same byte length" >&2
  exit 1
}

webkit_helper_dir=""
for candidate in "$baked_webkit_path" "/usr/libexec/webkit2gtk-4.1" "/usr/lib64/webkit2gtk-4.1"; do
  if [ -x "$candidate/WebKitWebProcess" ]; then
    webkit_helper_dir="$candidate"
    break
  fi
done
if [ -z "$webkit_helper_dir" ] && command -v dpkg >/dev/null 2>&1; then
  webkit_helper_dir="$(dpkg -L libwebkit2gtk-4.1-0 2>/dev/null | sed -n 's|/WebKitWebProcess$||p' | head -n1 || true)"
fi
[ -n "$webkit_helper_dir" ] || {
  echo "Could not find WebKitWebProcess; install libwebkit2gtk-4.1-0 before packaging" >&2
  exit 1
}

helper_target="$appdir$baked_webkit_path"
mkdir -p "$helper_target"
cp -a "$webkit_helper_dir/." "$helper_target/"
for helper in WebKitWebProcess WebKitNetworkProcess WebKitGPUProcess; do
  [ -x "$helper_target/$helper" ] || {
    echo "Failed to deploy $helper from $webkit_helper_dir" >&2
    exit 1
  }
done
[ -d "$helper_target/injected-bundle" ] || {
  echo "Failed to deploy the WebKit injected bundle from $webkit_helper_dir" >&2
  exit 1
}

grep -a -F -q "$baked_webkit_path" "$webkit_lib" || {
  echo "Could not find $baked_webkit_path in $webkit_lib" >&2
  exit 1
}
perl -0777 -pi -e "s|\Q$baked_webkit_path\E|$relative_webkit_path|g" "$webkit_lib"
chmod 0644 "$webkit_lib"
if grep -a -F -q "$baked_webkit_path" "$webkit_lib"; then
  echo "WebKit helper path patch failed: $baked_webkit_path is still present" >&2
  exit 1
fi
grep -a -F -q "$relative_webkit_path" "$webkit_lib" || {
  echo "WebKit helper path patch failed: $relative_webkit_path is missing" >&2
  exit 1
}

# appimagetool requires the desktop file and icon at the AppDir root. linuxdeploy
# normally creates them, but keep the fallback explicit so packaging fails for
# the right reason on a different linuxdeploy build.
[ -f "$appdir/ralgruM.desktop" ] || \
  install -m 0644 "$appdir/usr/share/applications/ralgruM.desktop" "$appdir/ralgruM.desktop"
[ -e "$appdir/ralgruM.png" ] || \
  install -m 0644 "$appdir/usr/share/icons/hicolor/512x512/apps/ralgruM.png" "$appdir/ralgruM.png"

appimagetool="$workdir/appimagetool-$arch.AppImage"
curl -fsSL -o "$appimagetool" "$APPIMAGETOOL_URL"
echo "${APPIMAGETOOL_SHA256[$arch]}  $appimagetool" | sha256sum -c -
chmod +x "$appimagetool"

runtime="$workdir/runtime-$arch"
curl -fsSL -o "$runtime" "$TYPE2_RUNTIME_URL"
echo "${TYPE2_RUNTIME_SHA256[$arch]}  $runtime" | sha256sum -c -

cd "$root"
rm -f ./*.AppImage
out="$root/ralgruM-$arch-linux.AppImage"
ARCH="$arch" "$appimagetool" --runtime-file "$runtime" "$appdir" "$out"
[ -f "$out" ] || { echo "appimagetool did not produce $out" >&2; exit 1; }
chmod 0755 "$out"
echo "Created $out"
