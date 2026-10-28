# 🍅 Pangan

A minimal, efficient **Pomodoro timer** for Linux, Windows, and macOS. Built with Rust + Slint — idle RAM is ~4–8 MB, startup is instant, and it ships as a single native binary per OS.

> **Pangan (پنگان)** is the Persian word for an ancient water clock used to divide time fairly. A fitting ancestor for a focus timer.

## Features

- 🧠 **Timer core** — drift-free timestamp-based countdown (no `sleep` clock drift)
- 🎨 **Old Parsi themed UI** — cuneiform-laced deep-indigo & gold design, with System/Dark/Light themes
- 🖥️ **System tray icon** — live countdown, Start/Pause, Skip, Reset, Quit
- 🔔 **Desktop notifications** — when a focus/break session ends
- 🔊 **Smart chimes** — distinct synthesized bell tones for work-complete vs break-complete
- ⌨️ **Global hotkey** — `Super+P` to start/pause from anywhere
- 🌙 **Screen-lock inhibit** — keep the desktop awake during focus sessions
- 🚀 **Autostart** — launches at login
- 📊 **Scientific reports** — daily/weekly/monthly focus analytics: sessions, minutes, consistency %, streaks, deep-work score
- ✅ **Tasks & scheduling** — add tasks (with optional date/time), tracked persistently
- ☁️ **Google Cloud sync** — push performance snapshots to a Google Sheet (see `sync.toml.example`)

## Requirements

- Rust 1.75+ (`rustc`, `cargo`)
- Linux: a system tray (X11 or Wayland via `StatusNotifierItem`), plus `libfontconfig1-dev libxkbcommon-dev libwayland-dev libx11-dev libxkbcommon-x11-dev libgtk-3-dev` to build
- Windows 10/11, macOS 11+

## Build

```bash
cargo build --release
# binary: ./target/release/pangan (a .exe on Windows)
```

## Install / autostart

Run it once to install the autostart entry:

```bash
cargo build --release
./target/release/pangan &
```

It writes the appropriate autostart entry, so it starts next time you log in.

## Configuration

Optional `~/.config/pangan/config.toml` (or `%APPDATA%\pangan\config.toml` on Windows, `~/Library/Application Support/pangan/config.toml` on macOS):

```toml
work_minutes = 25
short_break_minutes = 5
long_break_minutes = 15
long_break_interval = 4   # every N focus sessions -> long break
```

### Google Cloud sync

To push reports to a Google Sheet, copy `sync.toml.example` to `~/.config/pangan/sync.toml`
and fill in `spreadsheet_id`, `sheet`, and a `token` (see the example file for the
`gcloud auth print-access-token` steps). Then click the **☁ Sync** button in the app.
The app writes one row per sync: timestamp, period, sessions, focus minutes, average
session, consistency %, current streak, deep-work score, and open-task count.

### Data

History and tasks are stored in `~/.config/pangan/data.json`. The theme and sound
preferences live in `~/.config/pangan/settings.toml`.

## Architecture

- `src/clock.rs` — platform-agnostic timer state machine (unit-tested)
- `src/store.rs` — JSON persistence for sessions and tasks
- `src/stats.rs` — daily/weekly/monthly report engine
- `src/sound.rs` — synthesized chime WAV generation + playback
- `src/sync.rs` — Google Sheets API adapter
- `src/main.rs` — entry, event loop, wiring
- `src/platform/` — per-OS tray / notifications / hotkey / inhibit / autostart behind `cfg(target_os)`
- `ui/appwindow.slint` — Slint UI definition (themed, Old Parsi icon)
- Timer is driven by an `Instant` deadline + a 300 ms UI tick — no drift, no busy-wait.

## License

MIT
