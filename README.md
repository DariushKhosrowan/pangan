# 🍅 Pangan
> **پنگان** — the Persian word for an ancient water clock used to divide time fairly.
> A fitting ancestor for a focus timer.

Pangan is a modern, beautiful Pomodoro timer crafted in the spirit of the great
Achaemenid empire — Persepolis, Cyrus, and Darius. It is a fast, cross-platform
native app (Rust + Slint) with a bespoke "Persian Imperial Night" interface, a
radiating sun-ray timer, and an entirely synthesized **Iranian sound engine** —
no audio files, everything generated mathematically.

## Highlights

- 🎨 **Persepolis-inspired UI** — deep indigo & gilded gold palette, a radiating
  sun-ray progress ring evoking the winged solar disc, cuneiform-textured borders,
  system / dark / light themes.
- 🔊 **Iranian sound engine (procedural)** — five chime **themes**, each synthesized
  from scratch: **Santur** (hammered dulcimer), **Daf** (frame drum), **Tombak**
  (goblet drum), **Karn** (ceremonial horn/gong), and **Sand** (a fine water-clock
  tick). Preview before you choose.
- 🧠 **Drift-free timer core** — timestamp+`Instant`-based countdown (no clock drift).
- ⚙️ **Rich settings** — focus/short/long durations, long-break interval, daily goal,
  auto-start-next & auto-start-on-launch, theme, sound theme & volume toggle, global
  hotkey (`Super+P`), idle-inhibit, and fullscreen break-overlay toggles.
- 📊 **Scientific reports** — daily / weekly / monthly analytics: sessions, focus
  minutes, consistency %, streaks, and a deep-work score.
- ✅ **Tasks & scheduling** — add tasks (with optional date/time), tracked persistently.
- 🖥️ **System tray** — live countdown, Start/Pause, Skip, Reset, Quit.
- 🔔 **Desktop notifications** — when a focus/break session ends.
- 🌙 **Screen-lock inhibit** — keep the desktop awake during focus sessions.
- ☁️ **Cloud sync** — push performance snapshots to a Google Sheet (see `sync.toml.example`).

## Build

```bash
cargo build --release
# binary: ./target/release/pangan (a .exe on Windows)
```

Run it once and it installs an autostart entry:

```bash
cargo build --release
./target/release/pangan &
```

## Requirements

- Rust 1.75+
- Linux: a system tray (X11 or Wayland via `StatusNotifierItem`), plus
  `libfontconfig1-dev libxkbcommon-dev libwayland-dev libx11-dev
  libxkbcommon-x11-dev libgtk-3-dev` (for Slint).
- Windows 10/11, macOS 11+

## Configuration

All preferences live in `~/.config/pangan/config.toml` (Linux),
`%APPDATA%\pangan\config.toml` (Windows), or
`~/Library/Application Support/pangan/config.toml` (macOS) — or you can just use
the in-app Settings screen. Native durations are 25/5/15 minutes with a long
break every 4 focus sessions.

## Cloud sync (Google Sheets)

1. Copy `sync.toml.example` to `~/.config/pangan/sync.toml`.
2. Fill in `spreadsheet_id`, `sheet`, and a `token` obtained with:
   ```bash
   gcloud auth print-access-token --scopes=https://www.googleapis.com/auth/spreadsheets
   ```
3. Click the **☁ Sync** button in the app. Each sync appends one row: timestamp,
   period, sessions, focus minutes, avg session, consistency %, streak, deep-work
   score, and open-task count. Sync also runs automatically every 10 minutes
   while the token is configured.

## Data

History and tasks live in `~/.config/pangan/data.json`. Sound and theme
preferences are written into `config.toml`.

## Architecture

- `src/clock.rs` — platform-agnostic drift-free timer state machine
- `src/settings.rs` — full TOML settings model
- `src/store.rs` — JSON persistence for sessions and tasks
- `src/stats.rs` — daily/weekly/monthly report engine
- `src/sound.rs` — procedural Iranian instrument synthesis + WAV generation
- `src/sync.rs` — Google Sheets API adapter
- `src/platform/` — per-OS tray / notifications / hotkey / inhibit / autostart
- `ui/appwindow.slint` — the Persepolis-themed Slint UI

## License

MIT — built by **Dariush Khosrowan**.
