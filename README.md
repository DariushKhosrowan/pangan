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
  system / dark / light themes, and a **compact mini-window** mode.
- 🔊 **Iranian sound engine (procedural)** — six chime **themes**, each synthesized
  from scratch: **Santur** (hammered dulcimer), **Daf** (frame drum), **Tombak**
  (goblet drum), **Karn** (ceremonial horn/gong), **Sand** (a fine water-clock
  tick), and **Cyrus** (a regal herald fanfare). Preview before you choose, with a
  volume slider.
- 🤖 **Automation engine** — attach shell commands to timer events (`focus_start`,
  `focus_end`, `break_start`, `break_end`, `pomodoro_complete`) to drive your
  desktop (pause media, switch workspaces, and more).
- ⌨️ **Remappable global hotkeys** — toggle, skip, reset, and show/hide window,
  each configurable.
- 🧠 **Drift-free timer core** — timestamp+`Instant`-based countdown (no clock drift).
- ⚙️ **Rich settings** — focus/short/long durations, long-break interval, daily goal,
  auto-start-next & auto-start-on-launch, theme, compact mode, sound theme & volume
  toggle, global hotkey, idle-inhibit, and fullscreen break-overlay toggles.
- 📊 **Scientific reports** — daily / weekly / monthly analytics: sessions, focus
  minutes, consistency %, streaks, and a deep-work score.
- 📜 **The written record** — a **decree** before each block (what it is *for*)
  and an **inscription** after it (what it produced), paired on the session.
  A promise with no outcome facing it is the most useful row in the archive.
- ⚖️ **Reckoning** — the read-back, over whatever period is on screen: how many
  intentions were kept, which ones were not, and the words the span was
  actually about. A record nobody reads back is an archive, not a record.
- 🗓️ **Day rota** — write the day down as a numbered list before it starts, then
  begin each block with one press. Its length and its decree come with it.
- ✅ **Tasks & scheduling** — add tasks (with optional date/time), tracked persistently.
- 🖥️ **System tray** — live countdown, Start/Pause, Skip, Reset, Quit.
- 🔔 **Desktop notifications** — when a focus/break session ends.
- 🌙 **Screen-lock inhibit** — keep the desktop awake during focus sessions.
- ☁️ **Google Drive sync** — sign in with Google once per device and your focus
  history, tasks, rota **and settings** stay the same everywhere. No account with
  us, no server: the data lives in your own Drive (see `docs/google-sync.md`).

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

## Google sync

Settings → **Cloud** → **Connect Google**. The app shows a short code; open
<https://www.google.com/device>, type it, approve. Do the same on your other
machine with the same account and everything follows.

What travels: focus history, tasks, and your settings (durations, theme, era,
language, sounds). What stays per-machine: hotkeys, automation commands, compact
mode, screen-lock inhibit. The full list and the reasoning are in
[`docs/google-sync.md`](docs/google-sync.md).

The data lives in a hidden `appDataFolder` inside your own Drive. There is no
Pangan server and no account to create. Conflicts are resolved per record — and
per field for settings — so two devices editing different things both keep their
change, with no dialog to answer.

**Backup / Restore** in the same section still pushes and pulls a full snapshot
on demand, for moving to a new machine or recovering after a wipe.

## Data

History and tasks live in `~/.config/pangan/data.json`. Sound and theme
preferences are written into `config.toml`.

## Architecture

- `src/timer/` — platform-agnostic drift-free timer state machine
- `src/state/` — settings, the session/task store, and the day's rota
- `src/journal/` — decrees, inscriptions, and the reckoning that reads them back
- `src/app/` — startup, the UI event loop, shell automation hooks
- `src/stats/` — daily/weekly/monthly report engine
- `src/calendar/` — Jalali calendar, Sasanian day names, chronicle, events
- `src/audio/` — procedural Iranian instrument synthesis + WAV generation
- `src/sync/` — Google Drive sync, three-way merge, and local backup
- `src/platform/` — per-OS tray / notifications / hotkey / inhibit / autostart
- `ui/appwindow.slint` — the Persepolis-themed Slint UI

## License

MIT — built by **Dariush Khosrowan**.
