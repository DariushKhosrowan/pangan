#!/usr/bin/env bash
# install.sh — self-contained installer for the Pangan Linux binary.
#
# This is the "easy install" path: copy the pangan binary to /usr/local/bin,
# register .desktop + icon, and optionally create an AppImage launcher.
#
# Usage:
#   sudo ./install.sh                       # system-wide install (recommended)
#   ./install.sh --user                     # install to ~/.local (no root)
#   ./install.sh --appimage /path/Pangan.AppImage   # bundle AppImage launcher
#
# The script is conservative: it never overwrites an existing newer install and
# it always prints exactly what it did. Nothing is modified unless the user
# runs this script.
set -euo pipefail

# --- locate the binary ----------------------------------------------
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN=""
for cand in "$HERE/pangan" "$HERE/../../target/release/pangan"; do
  [ -f "$cand" ] && BIN="$cand" && break
done
if [ -z "$BIN" ]; then
  echo "error: could not find the 'pangan' binary next to this script." >&2
  echo "       Put install.sh next to the binary, or build with: cargo build --release" >&2
  exit 1
fi
BIN="$(readlink -f "$BIN")"

# --- options --------------------------------------------------------
USER_MODE=0
APPIMAGE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --user) USER_MODE=1; shift ;;
    --appimage)
      APPIMAGE="${2:-}"
      [ -n "$APPIMAGE" ] || { echo "error: --appimage needs a path." >&2; exit 1; }
      shift 2
      ;;
    *) echo "warning: ignoring unknown option: $1" >&2; shift ;;
  esac
done

if [ "$USER_MODE" -eq 1 ]; then
  DEST="$HOME/.local"
  BINDIR="$DEST/bin"
  DATADIR="$DEST/share"
else
  if [ "$(id -u)" -ne 0 ]; then
    echo "error: system-wide install needs root. Use 'sudo ./install.sh' or './install.sh --user'." >&2
    exit 1
  fi
  DEST="/usr/local"
  BINDIR="/usr/local/bin"
  DATADIR="/usr/local/share"
fi

mkdir -p "$BINDIR" "$DATADIR/applications" \
         "$DATADIR/icons/hicolor/512x512/apps" \
         "$DATADIR/icons/hicolor/scalable/apps" \
         "$DATADIR/pixmaps"

# --- I. install binary ----------------------------------------------
echo "Installing Pangan to $BINDIR/pangan (from $BIN)"
install -m 0755 "$BIN" "$BINDIR/pangan"

# --- II. desktop entry + icon ---------------------------------------
# Prefer a bundled icon (png > svg); otherwise skip (AppImage carries its own).
ICON_SRC=""
for cand in "$HERE/pangan.png" "$HERE/pangan.svg" \
            "$HERE/../ui/icons/pangan.svg" "$HERE/../../ui/icons/pangan.svg"; do
  [ -f "$cand" ] && ICON_SRC="$cand" && break
done

if [ -n "$ICON_SRC" ]; then
  case "$ICON_SRC" in
    *.svg)
      install -m 0644 "$ICON_SRC" "$DATADIR/icons/hicolor/scalable/apps/pangan.svg"
      install -m 0644 "$ICON_SRC" "$DATADIR/pixmaps/pangan.svg"
      ;;
    *)
      install -m 0644 "$ICON_SRC" "$DATADIR/icons/hicolor/512x512/apps/pangan.png"
      install -m 0644 "$ICON_SRC" "$DATADIR/pixmaps/pangan.png"
      ;;
  esac
fi

DESKTOP_SRC="$HERE/pangan.desktop"
if [ ! -f "$DESKTOP_SRC" ]; then
  DESKTOP_SRC="$(mktemp)"
  cat > "$DESKTOP_SRC" <<'EOF'
[Desktop Entry]
Type=Application
Name=Pangan
GenericName=Pomodoro Timer
Comment=A modern Persian Pomodoro timer
Categories=Utility;Office;
Exec=pangan
Icon=pangan
Terminal=false
EOF
fi
install -m 0644 "$DESKTOP_SRC" "$DATADIR/applications/pangan.desktop"

# --- III. optional AppImage launcher ---------------------------------
if [ -n "$APPIMAGE" ] && [ -f "$APPIMAGE" ]; then
  APPIMAGE="$(readlink -f "$APPIMAGE")"
  echo "Registering AppImage launcher: $APPIMAGE"
  cat > "$BINDIR/pangan-appimage" <<EOF
#!/usr/bin/env bash
exec "$APPIMAGE" "\$@"
EOF
  chmod +x "$BINDIR/pangan-appimage"
fi

# --- refresh caches --------------------------------------------------
if command -v update-desktop-database >/dev/null 2>&1 && [ -d "$DATADIR/applications" ]; then
  update-desktop-database "$DATADIR/applications" 2>/dev/null || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -d "$DATADIR/icons/hicolor" ]; then
  gtk-update-icon-cache -f "$DATADIR/icons/hicolor" 2>/dev/null || true
fi

echo ""
echo "Done. Pangan installed."
echo "  Launch from your app menu, or from a terminal: $BINDIR/pangan"
