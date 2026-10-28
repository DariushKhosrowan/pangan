# Feature Specification: Pangan — cross-platform Pomodoro timer

**Created**: 2026-10-28

**Status**: Active

**Input**: "A perfect Pomodoro timer for Linux that autostarts at login, minimal RAM/CPU, max efficiency/functionality. Then make it multi-OS (Windows/macOS). Publish publicly."

## Summary

Pangan is a minimal, efficient Pomodoro timer. Core: single native binary per OS,
drift-free timestamp-based countdown, system tray with live countdown, desktop
notifications, global hotkey (Super+P), idle-inhibit during focus, autostart at
login, TOML config. Cross-platform: shared timer logic + UI everywhere; OS
facilities behind a per-platform implementation behind `cfg(target_os)`.

## User Stories & Testing

### User Story 1 - Run a focus session (Priority: P1)

The user starts Pangan and runs a work session: countdown displays, on quit the
timer notifies, then enters a short/long break, then cycles back to focus.

**Why P1**: This is the core purpose. Without it nothing else matters.

**Independent Test**: Build the binary, start it, Start a session, observe
countdown reaching zero triggers a notification and the correct break phase, then
auto-advance back to Focus.

**Acceptance Scenarios**:
1. Given the timer is stopped at 25:00, When the user clicks Start, Then the
   countdown begins and the phase label shows "Focus".
2. Given a Focus session is running, When it reaches 00:00, Then a notification
   fires and the timer advances to "Short Break" (or "Long Break" on the 4th
   focus session) and starts counting.
3. Given a Break session, When it reaches 00:00, Then a notification fires and the
   timer returns to "Focus".

### User Story 2 - Autostart at login (Priority: P1)

Pangan launches when the desktop session starts so the user never remembers to
start it.

**Why P1**: The user's #1 stated requirement ("comes up when I start up my linux").

**Independent Test**: Run Pangan once with no existing autostart entry; verify the
entry is created for the platform (Linux `.desktop`, Windows Run key, macOS
LaunchAgent). Reboot/login is not required to pass this test.

**Acceptance Scenarios**:
1. Given Pangan has never run and autostart entry is absent, When Pangan runs once,
   Then an autostart entry is created pointing at the current binary.
2. Given an autostart entry already exists, When Pangan runs, Then the entry is not
   duplicated.

### User Story 3 - System tray control (Priority: P1)

The tray icon shows a live countdown and provides Start/Pause, Skip, Reset, Quit.

**Why P1**: Tray is the primary interaction surface when the window is hidden.

**Independent Test**: Run Pangan; verify a tray icon appears with a menu; Pause
countdown from tray; Skip to next phase; Quit exits.

**Acceptance Scenarios**:
1. Given Pangan is running, When the user opens the tray menu, Then items
   Start/Pause, Skip, Reset, Quit are present.
2. Given the timer is running, When the user selects Pause from the tray, Then the
   countdown halts and the menu/tooltip reflects paused state.
3. Given any state, When the user selects Quit, Then the process exits cleanly.

### User Story 4 - Desktop notification on session end (Priority: P1)

When a Focus or Break session completes, a desktop notification appears.

**Why P1**: Essential feedback loop; the whole point of a timer.

**Independent Test**: Start a session set to a very short work duration (config),
wait for completion, verify a notification appears.

**Acceptance Scenarios**:
1. Given a Focus session reaches 00:00, When the tick fires, Then a notification
   "Focus finished — take a Short Break." appears.
2. Given a Break session reaches 00:00, When the tick fires, Then a notification
   "Break over — back to focus." appears.

### User Story 5 - Global hotkey (Priority: P2)

Super+P toggles start/pause from anywhere.

**Why P2**: Convenience; adds real functionality but not required for a session.

**Independent Test**: Run Pangan, make another window active, press Super+P, verify
the timer toggles.

**Acceptance Scenarios**:
1. Given Pangan is in the background, When the user presses Super+P, Then the timer
   toggles start/pause.
2. Given the timer is counting, When the user presses Super+P, Then the timer pauses.

### User Story 6 - Idle-inhibit during focus (Priority: P2)

During a Focus session the machine stays awake / does not auto-lock.

**Why P2**: Stands between "drops the user's flow" and "nice to have".

**Independent Test**: Start a Focus session, verify an inhibit is active (logind
inhibit on Linux; SetThreadExecutionState on Windows; caffeinate on macOS). Verify
it is released when paused/completed.

**Acceptance Scenarios**:
1. Given a Focus session is running, When the session is active, Then an inhibit is
   registered.
2. Given a Focus session ends or pauses, When the timer stops, Then the inhibit is
   released.

### User Story 7 - Configurable durations (Priority: P2)

Work/break lengths and long-break interval come from config.toml.

**Why P2**: Personalization; default 25/5/15/4 is fine, config is expected.

**Independent Test**: Write a config with work_minutes=1, short=1; verify behavior.

**Acceptance Scenarios**:
1. Given a valid config.toml, When Pangan starts, Then durations take effect.
2. Given a missing/corrupt config, When Pangan starts, Then defaults are used and
   Pangan still runs.

### User Story 8 - Windowed UI (Priority: P3)

A small frameless dark UI shows countdown, progress, session label, and controls.

**Why P3**: The tray is primary, but the small window is the visible confirmation.

**Independent Test**: Run Pangan; verify the window opens, updates, buttons work,
closing hides to tray.

**Acceptance Scenarios**:
1. Given Pangan is running, When the user opens the window, Then countdown, progress,
   session name, phase, and Start/Pause/Skip/Reset render.
2. Given the window is open, When the user closes it, Then the app stays running in
   the tray.

## Edge Cases / Decisions

- **Deadline drift**: Pause stores `remaining`; resume rebuilds `finish_at`; tick
  reads monotonic `Instant`. No `sleep()`-accumulated drift.
- **Zero/negative remaining**: `saturating_duration_since` + `is_zero` boundary;
  exactly one advance per crossing.
- **Long-break cycle**: `work_done % long_break_interval == 0` triggers a long
  break; interval clamped to ≥1.
- **Skip semantics**: skipping a Focus increments work and moves to break; skipping
  a break returns to Focus.
- **Multiple instances**: not supported in v1; a second instance is harmless but
  not coordinated.
- **Notification backend**:
  - Linux: `notify-rust` over zbus (pure Rust).
  - macOS: `notify-rust` (mac-notification-sys).
  - Windows: `winrt-notification` (toast API).
- **Tray backend**:
  - Linux: `ksni` (StatusNotifierItem over zbus, no GTK).
  - Windows/macOS: `tray-icon` (native).
- **Hotkey**: `global-hotkey` 0.6 on all three; Super+P; degrade gracefully if
  registration fails (Wayland without portal support → hotkey unavailable).
- **Inhibit**:
  - Linux: logind `org.freedesktop.login1` Inhibit via zbus (or `power-daemon`).
  - Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`.
  - macOS: `caffeinate` process or IOPMAssertion.
- **Autostart**:
  - Linux: `~/.config/autostart/pangan.desktop`.
  - Windows: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
  - macOS: `~/Library/LaunchAgents/com.pangan.timer.plist`.
- **Config path**:
  - Linux: `~/.config/pangan/config.toml`
  - Windows: `%APPDATA%\pangan\config.toml`
  - macOS: `~/Library/Application Support/pangan/config.toml`
- **Site**: default English; multilingual `data-lang`; GitHub Pages.

## Milestones

1. Spec + constitution + repo scaffold (this work).
2. Timer core + Linux tray/notify/hotkey/inhibit/autostart, verified build.
3. Windows + macOS platform modules (cfg-gated), CI matrix build.
4. Site + Pages publish + release assets + final public README.

## Out of scope (v1)

Cloud sync, accounts, telemetry, mobile, second-instance coordination, standalone
package managers (apt/homebrew) — install is one-run autostart.
