#!/usr/bin/env bash
# build-windows-portable.sh — build a portable ZIP for Windows.
#
# The Windows release binary is standalone (statically linked), so a ZIP
# containing just the .exe is already fully portable. We include the
# desktop icon and a short README for convenience.
#
# Usage:  ./packaging/build-windows-portable.sh  (after cargo build --release)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')"
[ -n "$VERSION" ] || VERSION="0.0.0"
ARCH="$(uname -m)"

EXE="target/release/pangan.exe"
if [ ! -f "$EXE" ]; then
  # cross-compiled output lives under the target dir
  EXE=""
  for cand in target/x86_64-pc-windows-msvc/release/pangan.exe \
              target/aarch64-pc-windows-msvc/release/pangan.exe; do
    [ -f "$cand" ] && EXE="$cand" && break
  done
fi
if [ -z "$EXE" ]; then
  echo "error: no pangan.exe found — run 'cargo build --release --target <windows-target>' first." >&2
  exit 1
fi
echo "Using: $EXE"

STAGE="build/windows-portable"
rm -rf "$STAGE"
mkdir -p "$STAGE"
install -m 0755 "$EXE" "$STAGE/Pangan.exe"

# copy an icon if we have one
if [ -f ui/icons/pangan.png ]; then
  install -m 0644 ui/icons/pangan.png "$STAGE/Pangan.png"
elif [ -f ui/icons/pangan.svg ]; then
  install -m 0644 ui/icons/pangan.svg "$STAGE/Pangan.svg"
fi

cat > "$STAGE/README.txt" <<EOF
Pangan $VERSION — portable Windows build

Just double-click Pangan.exe. No installation required; the
application stores its data under %APPDATA%\Pangan, so you can
even run it from a USB stick.

To uninstall, simply delete this folder.
EOF

mkdir -p dist
ZIP="dist/Pangan-$VERSION-windows-$ARCH-portable.zip"
rm -f "$ZIP"
( cd "$STAGE" && zip -r -X "$OLDPWD/$ZIP" . >/dev/null )
echo "Done: $ZIP"
echo "Unzip anywhere and run Pangan.exe."
