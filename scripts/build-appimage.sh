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

# WebKit's network process uses libsoup, which delegates TLS to GIO. GIO
# loads its TLS backend from a module directory compiled into libgio. That
# path is distribution-specific and does not exist on SteamOS, so the GnuTLS
# module is bundled and AppRun points GIO at the bundled directory. The
# module is not a linked dependency of libgio, so linuxdeploy does not see it
# and it has to be copied explicitly. The module links against libgnutls,
# which is passed to linuxdeploy as an additional library.
gio_module_dir=""
if command -v pkg-config >/dev/null 2>&1; then
  gio_module_dir="$(pkg-config --variable=giomoduledir gio-2.0 2>/dev/null || true)"
fi
if [ -z "$gio_module_dir" ] || [ ! -f "$gio_module_dir/libgiognutls.so" ]; then
  if command -v dpkg >/dev/null 2>&1; then
    gio_module_dir="$(dpkg -L glib-networking 2>/dev/null | sed -n 's|/libgiognutls\.so$||p' | head -n1 || true)"
  fi
fi
if [ -z "$gio_module_dir" ] || [ ! -f "$gio_module_dir/libgiognutls.so" ]; then
  for candidate in /usr/lib/*/gio/modules /usr/lib/gio/modules /usr/lib64/gio/modules; do
    if [ -f "$candidate/libgiognutls.so" ]; then
      gio_module_dir="$candidate"
      break
    fi
  done
fi
[ -n "$gio_module_dir" ] && [ -f "$gio_module_dir/libgiognutls.so" ] || {
  echo "Missing libgiognutls.so; install glib-networking before packaging" >&2
  exit 1
}

# GStreamer loads these plugins at runtime, so linuxdeploy's ldd scan does not
# find them. They cover the audio stack DataDome probes during the SoundCloud
# challenge (appsink, autoaudiosink, decodebin, giostreamsrc) plus common
# audio codecs WebKit may advertise through canPlayType.
gst_plugin_dir=""
if command -v pkg-config >/dev/null 2>&1; then
  gst_plugin_dir="$(pkg-config --variable=pluginsdir gstreamer-1.0 2>/dev/null || true)"
fi
if [ -z "$gst_plugin_dir" ] || [ ! -d "$gst_plugin_dir" ]; then
  for candidate in /usr/lib/*/gstreamer-1.0 /usr/lib64/gstreamer-1.0 /usr/lib/gstreamer-1.0; do
    if [ -d "$candidate" ] && [ -f "$candidate/libgstapp.so" ]; then
      gst_plugin_dir="$candidate"
      break
    fi
  done
fi
[ -n "$gst_plugin_dir" ] && [ -d "$gst_plugin_dir" ] || {
  echo "Missing GStreamer plugins; install gstreamer1.0-plugins-base and gstreamer1.0-plugins-good before packaging" >&2
  exit 1
}

gst_scanner=""
for candidate in \
  /usr/libexec/gstreamer-1.0/gst-plugin-scanner \
  /usr/lib/*/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner \
  /usr/lib/*/gstreamer-1.0/gst-plugin-scanner; do
  if [ -x "$candidate" ]; then
    gst_scanner="$candidate"
    break
  fi
done
[ -n "$gst_scanner" ] || {
  echo "Missing gst-plugin-scanner; install gstreamer1.0-plugins-base before packaging" >&2
  exit 1
}

gnutls_library="$(ldd "$gio_module_dir/libgiognutls.so" 2>/dev/null | awk '$1 == "libgnutls.so.30" { print $3; exit }' || true)"
[ -n "$gnutls_library" ] && [ -f "$gnutls_library" ] || {
  echo "Missing libgnutls.so.30; install glib-networking before packaging" >&2
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

# libgio looks for its TLS backend in a module directory compiled into the
# library. Point it at the bundled GnuTLS module instead of the build host's
# distribution-specific path, which does not exist on every target system.
export GIO_MODULE_DIR="$appdir/usr/lib/gio/modules"

# WebKit's media stack loads GStreamer elements dynamically. linuxdeploy
# bundles the linked GStreamer libraries but not the plugins, and a missing
# appsink/autoaudiosink makes WebKit abort its web process while DataDome
# fingerprints the audio stack. Keep GStreamer on the bundled 1.24 plugin set
# and give it a version-specific registry so a host 1.26 cache is not reused.
export GST_PLUGIN_SYSTEM_PATH="$appdir/usr/lib/gstreamer-1.0"
export GST_PLUGIN_SYSTEM_PATH_1_0="$GST_PLUGIN_SYSTEM_PATH"
export GST_PLUGIN_SCANNER="$appdir/usr/libexec/gstreamer-1.0/gst-plugin-scanner"
export GST_REGISTRY="${XDG_CACHE_HOME:-$HOME/.cache}/ralgruM/gstreamer-1.24-registry.bin"
mkdir -p "${GST_REGISTRY%/*}"

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
  --library "$gnutls_library" \
  --custom-apprun "$workdir/AppRun"

# Bundle the GStreamer plugin set and the matching core libraries. Copying all
# libgst* libraries also brings the RTP/riff helpers used by the MP4 plugin,
# which linuxdeploy does not see because they are only linked by plugins.
gst_plugin_target="$appdir/usr/lib/gstreamer-1.0"
mkdir -p "$gst_plugin_target" "$appdir/usr/libexec/gstreamer-1.0"
gst_plugins=(
  libgstapp.so
  libgstapetag.so
  libgstaudioconvert.so
  libgstaudioparsers.so
  libgstaudioresample.so
  libgstaudiotestsrc.so
  libgstautodetect.so
  libgstcoreelements.so
  libgstflac.so
  libgstgio.so
  libgsticydemux.so
  libgstid3demux.so
  libgstisomp4.so
  libgstmpg123.so
  libgstogg.so
  libgstopus.so
  libgstplayback.so
  libgstpulseaudio.so
  libgsttypefindfunctions.so
  libgstvolume.so
  libgstvorbis.so
  libgstwavparse.so
)
for plugin in "${gst_plugins[@]}"; do
  [ -f "$gst_plugin_dir/$plugin" ] || {
    echo "Missing GStreamer plugin $plugin in $gst_plugin_dir" >&2
    exit 1
  }
  install -m 0644 "$gst_plugin_dir/$plugin" "$gst_plugin_target/$plugin"
done
install -m 0755 "$gst_scanner" "$appdir/usr/libexec/gstreamer-1.0/gst-plugin-scanner"

gst_lib_dir="$(dirname "$gst_plugin_dir")"
for library in "$gst_lib_dir"/libgst*.so.*; do
  [ -f "$library" ] || continue
  install -m 0644 "$(readlink -f "$library")" "$appdir/usr/lib/$(basename "$library")"
done

# Plugin-only dependencies that are not linked by WebKit itself. They are
# standard desktop audio/codec libraries; copy them when the build host has
# them so the AppImage does not depend on the target distribution's versions.
for library in \
  libFLAC.so.12 \
  libmpg123.so.0 \
  libogg.so.0 \
  libopus.so.0 \
  libvorbis.so.0 \
  libvorbisenc.so.2 \
  libpulse.so.0 \
  libpulsecommon-16.1.so \
  libsndfile.so.1 \
  libasyncns.so.0 \
  libapparmor.so.1; do
  source_library="$(find /usr/lib /usr/lib64 \( -type f -o -type l \) -name "$library" -print -quit 2>/dev/null || true)"
  [ -n "$source_library" ] || {
    echo "Missing $library; install the matching GStreamer audio plugins before packaging" >&2
    exit 1
  }
  # The build host usually provides the versioned soname as a symlink to the
  # real library; install the target so the AppImage does not carry a dangling
  # link.
  source_library="$(readlink -f "$source_library")"
  install -m 0644 "$source_library" "$appdir/usr/lib/$library"
done

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

# GIO only loads modules from GIO_MODULE_DIR, which AppRun points at this
# directory. libgmp.so.10 stays host-provided because linuxdeploy excludes it
# and SteamOS ships it with the system GnuTLS stack.
gio_module_target="$appdir/usr/lib/gio/modules"
mkdir -p "$gio_module_target"
install -m 0644 "$gio_module_dir/libgiognutls.so" "$gio_module_target/libgiognutls.so"

# linuxdeploy copies copyright files for the libraries it deploys. The GIO
# module is copied manually, so include its copyright file explicitly.
if [ -f /usr/share/doc/glib-networking/copyright ]; then
  mkdir -p "$appdir/usr/share/doc/glib-networking"
  install -m 0644 /usr/share/doc/glib-networking/copyright "$appdir/usr/share/doc/glib-networking/copyright"
fi

for library in \
  libgnutls.so.30 \
  libp11-kit.so.0 \
  libffi.so.8 \
  libnettle.so.8 \
  libhogweed.so.6 \
  libidn2.so.0 \
  libtasn1.so.6 \
  libunistring.so.5; do
  [ -f "$appdir/usr/lib/$library" ] || {
    echo "linuxdeploy did not bundle $library" >&2
    exit 1
  }
done

for library in "$gio_module_target/libgiognutls.so" "$appdir/usr/lib/libgnutls.so.30"; do
  if ! ldd_output="$(LD_LIBRARY_PATH="$appdir/usr/lib" ldd "$library" 2>&1)"; then
    echo "ldd failed for $library:" >&2
    printf '%s\n' "$ldd_output" >&2
    exit 1
  fi
  if printf '%s\n' "$ldd_output" | grep -q 'not found'; then
    echo "Unresolved dependencies for $library:" >&2
    printf '%s\n' "$ldd_output" | grep 'not found' >&2
    exit 1
  fi
done

ldd_output="$(LD_LIBRARY_PATH="$appdir/usr/lib" ldd "$gio_module_target/libgiognutls.so" 2>&1)"
printf '%s\n' "$ldd_output" | grep -F "libgnutls.so.30 => $appdir/usr/lib/libgnutls.so.30" >/dev/null || {
  echo "libgiognutls.so does not load the bundled libgnutls.so.30:" >&2
  printf '%s\n' "$ldd_output" >&2
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

# WebKitGTK 2.52 hard-codes a soundcloud.com User-Agent quirk that replaces the
# application's UA with Chrome/300.0.0.0. DataDome rejects that impossible
# browser and never completes the slider. The WebsitePolicies custom-user-agent
# API that supersedes the quirk only exists in 2.54, while the AppImage ships
# Ubuntu 24.04's 2.52. Neutralize the domain match in the bundled copy instead;
# the replacement has the same length and does not touch the page URL.
if grep -a -F -q 'soundcloud.com' "$webkit_lib"; then
  perl -0777 -pi -e 's/\Qsoundcloud.com\E/soundclouX.com/g' "$webkit_lib"
  if grep -a -F -q 'soundcloud.com' "$webkit_lib"; then
    echo "SoundCloud user-agent quirk patch failed: soundcloud.com is still present" >&2
    exit 1
  fi
  grep -a -F -q 'soundclouX.com' "$webkit_lib" || {
    echo "SoundCloud user-agent quirk patch failed: replacement is missing" >&2
    exit 1
  }
  echo "Neutralized the WebKitGTK SoundCloud user-agent quirk"
else
  echo "WebKitGTK does not contain the SoundCloud user-agent quirk"
fi

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
