#!/usr/bin/env bash
# Packages the ralgruM Linux build as ralgruM-<arch>-linux.AppImage.
# Runs on a Linux host (CI) with the release binary already built at
# target/release/ralgruM. Uses linuxdeploy's appimage output plugin, which
# needs the appimage tool for the host architecture (auto-downloaded).
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

binary="$root/target/release/ralgruM"
[ -x "$binary" ] || { echo "Missing release binary at $binary" >&2; exit 1; }

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
appdir="$workdir/AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" "$appdir/usr/share/icons/hicolor/512x512/apps"

install -m 0755 "$binary" "$appdir/usr/bin/ralgruM"
install -m 0644 "$root/assets/app-icon.png" "$appdir/usr/share/icons/hicolor/512x512/apps/ralgruM.png"
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

linuxdeploy="$workdir/linuxdeploy-$arch.AppImage"
curl -fsSL -o "$linuxdeploy" "$LINUXDEPLOY_URL"
echo "${LINUXDEPLOY_SHA256[$arch]}  $linuxdeploy" | sha256sum -c -
chmod +x "$linuxdeploy"

DEPLOY_GTK_VERSION=3 NO_STRIP=true "$linuxdeploy" \
  --appdir "$appdir" \
  --desktop-file "$appdir/usr/share/applications/ralgruM.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/512x512/apps/ralgruM.png" \
  --output appimage \
  --artifact-name "ralgruM-$arch-linux"

out="$root/ralgruM-$arch-linux.AppImage"
mv "$workdir/ralgruM-$arch-linux.AppImage" "$out"
echo "Created $out"
