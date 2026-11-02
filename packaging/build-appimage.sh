#!/usr/bin/env bash
# build-appimage.sh — build a self-contained Pangan AppImage for Linux.
#
# AppImage = a single executable file that runs on most glibc distros
# without installing anything. This script needs a built release binary:
#   cargo build --release
#   ./packaging/build-appimage.sh
#
# Output: dist/Pangan-<version>-<arch>.AppImage
#
# Uses AppImageKit's appimagetool. If it is not installed, the script
# downloads a pinned (by URL hash) copy into /tmp for the current arch.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) AI_ARCH="x86_64" ;;
  aarch64|arm64) AI_ARCH="aarch64" ;;
  *) echo "error: unsupported arch for AppImage: $ARCH" >&2; exit 1 ;;
esac

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
[ -n "$VERSION" ] || VERSION="0.0.0"

BIN="target/release/pangan"
[ -f "$BIN" ] || { echo "error: $BIN not found — run 'cargo build --release' first." >&2; exit 1; }

# --- appimagetool ---------------------------------------------------
AI_TOOL="/tmp/appimagetool-$AI_ARCH"
if [ ! -x "$AI_TOOL" ]; then
  echo "Fetching appimagetool ($AI_ARCH)…"
  case "$AI_ARCH" in
    x86_64) AI_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage" ;;
    aarch64) AI_URL="https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-aarch64.AppImage" ;;
  esac
  curl -fL --retry 3 -o "$AI_TOOL" "$AI_URL"
  chmod +x "$AI_TOOL"
fi

# --- staging dir ----------------------------------------------------
STAGE="build/appimage/$AI_ARCH"
rm -rf "$STAGE"
mkdir -p "$STAGE/usr/bin" "$STAGE/usr/share/applications" \
         "$STAGE/usr/share/icons/hicolor/512x512/apps" \
         "$STAGE/usr/share/icons/hicolor/scalable/apps"

install -m 0755 "$BIN" "$STAGE/usr/bin/pangan"

# AppRun — the AppImage entry point.
cat > "$STAGE/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/pangan" "$@"
EOF
chmod +x "$STAGE/AppRun"

# .desktop + icon at the AppDir root are what appimagetool uses to build the
# AppImage (the usr/share copies are for when the AppImage is "installed").
install -m 0644 packaging/pangan.desktop "$STAGE/pangan.desktop"
install -m 0644 packaging/pangan.desktop "$STAGE/usr/share/applications/pangan.desktop"

# icon: prefer PNG if available, else SVG (hi-res scalable).
if [ -f ui/icons/pangan.png ]; then
  install -m 0644 ui/icons/pangan.png "$STAGE/pangan.png"
  install -m 0644 ui/icons/pangan.png "$STAGE/usr/share/icons/hicolor/512x512/apps/pangan.png"
fi
if [ -f ui/icons/pangan.svg ]; then
  install -m 0644 ui/icons/pangan.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/pangan.svg"
  [ -f "$STAGE/pangan.png" ] || install -m 0644 ui/icons/pangan.svg "$STAGE/pangan.svg"
fi
# appimagetool wants an icon named after the desktop file's Icon= entry.
if [ ! -e "$STAGE/pangan.png" ] && [ -f "$STAGE/pangan.svg" ]; then
  ln -s pangan.svg "$STAGE/pangan.png"
fi

# --- desktop integration hint (file type registration) ---------------
cat > "$STAGE/usr/share/applications/pangan.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Pangan
GenericName=Pomodoro Timer
Comment=A modern Persian Pomodoro timer inspired by Persepolis
Categories=Utility;Office;
Keywords=Pomodoro;timer;focus;persian;
Exec=pangan
Icon=pangan
Terminal=false
X-AppImage-Version=$VERSION
EOF

# --- build the AppImage ----------------------------------------------
mkdir -p dist
OUT="dist/Pangan-$VERSION-$AI_ARCH.AppImage"
echo "Building $OUT …"
(
  cd "$STAGE"
  ARCH="$AI_ARCH" "$AI_TOOL" . "$OLDPWD/$OUT"
)
chmod +x "$OUT"
echo ""
echo "Done: $OUT"
echo "Run it directly:  $OUT"
echo "Install it:       ./packaging/install.sh --appimage \"$OUT\""
