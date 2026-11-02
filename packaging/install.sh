#!/usr/bin/env bash
# =============================================================================
#  Pangan — one-command installer
# -----------------------------------------------------------------------------
#  The whole app installs with a single command:
#
#      curl -fsSL https://raw.githubusercontent.com/DariushKhosrowan/pangan/main/packaging/install.sh | bash
#
#  It downloads the latest release for YOUR distribution + CPU, installs any
#  missing runtime libraries, and registers the app (binary, desktop entry,
#  icon) — no separate download step, no manual tweaks.
#
#  You can also run it from a checkout / release bundle:
#      sudo ./install.sh                 # system-wide (default)
#      ./install.sh --user               # per-user install into ~/.local (no root)
#      ./install.sh --appimage ./Pangan-<version>-x86_64.AppImage
#      ./install.sh --skip-deps          # never touch the package manager
#      ./install.sh --autostart          # also add a login autostart entry
#      ./install.sh --dry-run            # show what would happen, change nothing
#
#  The script is conservative: it never overwrites a newer install, prints
#  exactly what it did, and modifies nothing without your say-so.
# =============================================================================
set -euo pipefail

# --------------------------------------------------------------- constants ---
REPO="DariushKhosrowan/pangan"
GH_RELEASES="https://github.com/${REPO}/releases"
GH_API="https://api.github.com/repos/${REPO}/releases/latest"
GH_DL="https://github.com/${REPO}/releases/latest/download"

PANGAN_VERSION_RQ="0.3.1"

# ------------------------------------------------------------------ options --
USER_MODE=0
APPIMAGE=""
USE_APPIMAGE=0
SKIP_DEPS=0
AUTOSTART=0
DRY_RUN=0

usage() {
  sed -n '3,22p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --user)          USER_MODE=1 ;;
    --appimage)      USE_APPIMAGE=1; APPIMAGE="${2:-}"; shift ;;
    --skip-deps)     SKIP_DEPS=1 ;;
    --autostart)     AUTOSTART=1 ;;
    --dry-run)       DRY_RUN=1 ;;
    -h|--help)       usage ;;
    *) echo "warning: ignoring unknown option: $1" >&2 ;;
  esac
  shift
done

# --------------------------------------------------------------- helpers ----
info() { printf '\033[1;34m·\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

run() {
  if [ "$DRY_RUN" -eq 1 ]; then echo "  [dry-run] $*"; return 0; fi
  "$@"
}

need_curl() {
  have curl || die "curl not found — install it first (sudo apt install curl / sudo dnf install curl / sudo pacman -S curl)."
}

# ------------------------------------------------------------- detect arch ---
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)      ARCH=x86_64 ;;
  aarch64|arm64)     ARCH=aarch64 ;;
  *) die "unsupported CPU architecture: $ARCH (Pangan ships x86_64 and aarch64)." ;;
esac
info "Detected CPU architecture: $ARCH"

# ---------------------------------------------------------- detect distro ----
distro=unknown
if have apt-get; then distro=debian
elif have dnf;   then distro=fedora
elif have zypper; then distro=suse
elif have pacman; then distro=arch
fi

case "$distro" in
  debian) dep_pkgs="libxkbcommon0 libx11-6 libwayland-client0 libxcb1 libfontconfig1 libgl1"
          dep_cmd="sudo apt-get install -y $dep_pkgs" ;;
  fedora) dep_pkgs="libxkbcommon libX11 libwayland-client libxcb fontconfig mesa-libGL"
          dep_cmd="sudo dnf install -y $dep_pkgs" ;;
  arch)   dep_pkgs="libxkbcommon libx11 wayland libxcb fontconfig mesa"
          dep_cmd="sudo pacman -S --needed $dep_pkgs" ;;
  suse)   dep_pkgs="libxkbcommon0 libX11-6 libwayland-client libxcb1 fontconfig"
          dep_cmd="sudo zypper install $dep_pkgs" ;;
  *)      dep_pkgs="" ; dep_cmd="" ;;
esac

# ----------------------------------------------------- acquire the app ------
# Determine where the script lives. When piped via `curl | bash` there is no
# script file on disk ($0 == "bash"), so we fall back to the current dir;
# the icon/desktop are then fetched from the repo (see below).
case "$0" in
  */*) HERE="$(cd "$(dirname "$0")" && pwd)" ;;
  *)   HERE="$(pwd)" ;;
esac
RAW_BASE="https://raw.githubusercontent.com/${REPO}/main"

# 1) If the user handed us an AppImage path, use it directly.
if [ "$USE_APPIMAGE" -eq 1 ] && [ -z "$APPIMAGE" ] && [ -f "$HERE/"*.AppImage ]; then
  APPIMAGE="$(ls "$HERE/"*.AppImage | head -n1)"
fi
if [ "$USE_APPIMAGE" -eq 1 ] && [ -n "$APPIMAGE" ]; then
  [ -f "$APPIMAGE" ] || die "AppImage not found at: $APPIMAGE"
  APPIMAGE="$(readlink -f "$APPIMAGE")"
  info "Using provided AppImage: $APPIMAGE"
  BIN="$APPIMAGE"
  SOURCE=appimage
else
  # 2) A local binary next to the script (release bundle / checkout)?
  local_bin=""
  for cand in "$HERE/pangan" "$HERE/../target/release/pangan"; do
    [ -f "$cand" ] && local_bin="$cand" && break
  done
  if [ -n "$local_bin" ] && [ "$USE_APPIMAGE" -eq 0 ]; then
    BIN="$(readlink -f "$local_bin")"
    info "Using local binary: $BIN"
    SOURCE=local
  else
    # 3) Bootstrap: download the latest release for this arch.
    need_curl
    info "Downloading the latest Pangan release (Linux $ARCH) from GitHub Releases…"
    ASSET="pangan-linux-${ARCH}"
    URL="$GH_DL/$ASSET"
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    if [ "$DRY_RUN" -eq 1 ]; then
      echo "  [dry-run] curl -fL '$URL' -o '$TMP/$ASSET'"
      BIN="$TMP/$ASSET"
      SOURCE=download
    else
      curl -fL "$URL" -o "$TMP/$ASSET"
      chmod +x "$TMP/$ASSET"
      BIN="$TMP/$ASSET"
      SOURCE=download
      ok "Downloaded $ASSET ($(du -h "$BIN" | cut -f1))"
    fi
  fi
fi

# ------------------------------------------------- handle missing libraries --
if [ "$SOURCE" = download ] || [ "$SOURCE" = local ]; then
  if have ldd; then
    missing="$(ldd "$BIN" 2>/dev/null | awk '/=> not found/{print $1}' || true)"
    if [ -n "$missing" ]; then
      warn "This build needs these libraries, which appear to be missing: $missing"
      if [ "$SKIP_DEPS" -eq 1 ]; then
        warn "Skipping dependency install (--skip-deps). The app may fail to open a window."
      elif [ -n "$dep_cmd" ] && [ "$(id -u)" -eq 0 ]; then
        info "Installing runtime dependencies ($distro)…"
        warn "  $dep_cmd"
        run bash -c "$dep_cmd"
      elif [ -n "$dep_cmd" ]; then
        warn "You are not root. To finish, the runtime libraries are needed:"
        warn "  $dep_cmd"
        [ "$DRY_RUN" -eq 1 ] || warn "Continuing anyway — re-run as root if the app won't start."
      else
        warn "Unknown distro — install the runtime libs manually (see README)."
      fi
    fi
  fi
fi

# ------------------------------------------------------- install location ----
if [ "$USER_MODE" -eq 1 ]; then
  DEST="$HOME/.local"
  BINDIR="$DEST/bin"
  DATADIR="$DEST/share"
else
  if [ "$(id -u)" -ne 0 ]; then
    if [ "$DRY_RUN" -eq 1 ]; then
      DEST="/usr/local"; BINDIR="/usr/local/bin"; DATADIR="/usr/local/share"
    else
      die "system-wide install needs root — use 'sudo ./install.sh' or './install.sh --user'."
    fi
  fi
  DEST="/usr/local"; BINDIR="/usr/local/bin"; DATADIR="/usr/local/share"
fi

run mkdir -p "$BINDIR" "$DATADIR/applications" \
        "$DATADIR/icons/hicolor/512x512/apps" \
        "$DATADIR/icons/hicolor/scalable/apps" \
        "$DATADIR/pixmaps"

# ----------------------------------------------------- I. install binary ----
if [ "$SOURCE" = appimage ]; then
  run install -m 0755 "$BIN" "$BINDIR/pangan-appimage"
  info "Registered AppImage launcher: $BINDIR/pangan-appimage"
  EXEC="$BINDIR/pangan-appimage"
else
  run install -m 0755 "$BIN" "$BINDIR/pangan"
  info "Installed Pangan to $BINDIR/pangan"
  EXEC="pangan"
fi

# ------------------------------------------------- II. desktop entry + icon --
ICON_SRC=""
for cand in "$HERE/pangan.png" "$HERE/pangan.svg" \
            "$HERE/../ui/icons/pangan.svg" "$HERE/../../ui/icons/pangan.svg"; do
  [ -f "$cand" ] && ICON_SRC="$cand" && break
done

# In bootstrap mode (downloaded binary), fetch the icon + desktop template
# from the repo so the installed entry is complete even though nothing is on
# disk next to the script.
if [ -z "$ICON_SRC" ] && [ "$SOURCE" = download ]; then
  need_curl
  ASSET_DIR="${TMP:-$(mktemp -d)}"
  [ "$DRY_RUN" -eq 0 ] && curl -fsSL "$RAW_BASE/ui/icons/pangan.svg" -o "$ASSET_DIR/pangan.svg" \
    && ICON_SRC="$ASSET_DIR/pangan.svg" \
    || warn "Could not fetch icon from repo — installing without a custom icon."
fi

if [ -n "$ICON_SRC" ] && [ -f "$ICON_SRC" ]; then
  case "$ICON_SRC" in
    *.svg)
      run install -m 0644 "$ICON_SRC" "$DATADIR/icons/hicolor/scalable/apps/pangan.svg"
      run install -m 0644 "$ICON_SRC" "$DATADIR/pixmaps/pangan.svg"
      ;;
    *)
      run install -m 0644 "$ICON_SRC" "$DATADIR/icons/hicolor/512x512/apps/pangan.png"
      run install -m 0644 "$ICON_SRC" "$DATADIR/pixmaps/pangan.png"
      ;;
  esac
fi

DESKTOP_SRC="$HERE/pangan.desktop"
if [ ! -f "$DESKTOP_SRC" ] && [ "$SOURCE" = download ]; then
  ASSET_DIR="${TMP:-$(mktemp -d)}"
  if [ "$DRY_RUN" -eq 0 ]; then
    curl -fsSL "$RAW_BASE/packaging/pangan.desktop" -o "$ASSET_DIR/pangan.desktop" \
      && DESKTOP_SRC="$ASSET_DIR/pangan.desktop" || DESKTOP_SRC=""
  fi
fi
if [ -z "$DESKTOP_SRC" ] || [ ! -f "$DESKTOP_SRC" ]; then
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

# In AppImage mode, point the desktop entry at the launcher.
if [ "$SOURCE" = appimage ]; then
  sed -e "s#^Exec=.*#Exec=$EXEC#" "$DESKTOP_SRC" > "${DESKTOP_SRC}.pangan"
  DESKTOP_SRC="${DESKTOP_SRC}.pangan"
fi
run install -m 0644 "$DESKTOP_SRC" "$DATADIR/applications/pangan.desktop"

# --------------------------------------------------- III. autostart (opt) --
if [ "$AUTOSTART" -eq 1 ]; then
  A_ODIR="$HOME/.config/autostart"
  run mkdir -p "$AUTODIR"
  run install -m 0644 "$DESKTOP_SRC" "$AUTODIR/pangan.desktop"
  ok "Autostart registered ($AUTODIR/pangan.desktop)"
fi

# ------------------------------------------------------- IV. refresh caches --
if [ "$DRY_RUN" -eq 0 ]; then
  if have update-desktop-database && [ -d "$DATADIR/applications" ]; then
    update-desktop-database "$DATADIR/applications" 2>/dev/null || true
  fi
  if have gtk-update-icon-cache && [ -d "$DATADIR/icons/hicolor" ]; then
    gtk-update-icon-cache -f "$DATADIR/icons/hicolor" 2>/dev/null || true
  fi
fi

# -------------------------------------------------------------- summary -----
echo ""
ok "Done. Pangan installed."
echo "  Launch from your app menu, or from a terminal: $EXEC"
[ "$SOURCE" = appimage ] && echo "  (AppImage launcher — runs the self-contained AppImage.)"
