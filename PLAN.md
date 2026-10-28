# Implementation Plan: Pangan cross-platform Pomodoro timer

**Branch**: main | **Date**: 2026-10-28 | **Spec**: SPEC.md

## Summary

Build Pangan into a finished cross-platform Pomodoro timer. The current
Linux-only code is a solid base: drift-free `Instant`-based timer core, Slint
UI, `tray-icon`/`notify-rust`/`global-hotkey`/`dirs`. To make it cross-platform
and lighter on Linux, I will:

- Swap Linux tray to `ksni` (pure-DBus StatusNotifierItem, no GTK) — meets the
  efficiency mandate and removes the GTK3 system dependency.
- Keep `tray-icon` on Windows/macOS (native).
- Add per-platform notification modules (Linux/macOS: notify-rust; Windows:
  winrt-notification).
- Add per-platform idle-inhibit (Linux: logind via zbus; Windows:
  SetThreadExecutionState; macOS: caffeinate).
- Add per-platform autostart (Linux: .desktop; Windows: Run key; macOS:
  LaunchAgent plist).
- Add per-platform config path resolution via `dirs`.
- Split the monolithic `main.rs` into a `platform` module tree with
  `#[cfg(target_os)]` selection behind a common trait.

## Technical Context

**Language/Version**: Rust 1.99 (edition 2021)

**Primary Dependencies**:
- slint 1.11 (UI)
- ksni 0.3 (Linux tray; pure zbus)
- tray-icon 0.19 (Windows/macOS tray)
- notify-rust 4.11 (Linux/macOS notifications)
- winrt-notification 0.5 (Windows notifications)
- global-hotkey 0.6 (all; Super+P)
- zbus (Linux logind inhibit)
- dirs 6 (config paths)

**Storage**: TOML config at per-OS path.

**Testing**: `cargo build` locals; GitHub Actions build matrix for all three OS.
Unit tests on the timer state machine.

**Target Platform**: Linux (X11/Wayland), Windows 10/11, macOS.

**Project Type**: desktop-app, single native binary per OS.

**Performance Goals**: idle RAM ≤ ~8 MB, CPU ~0% idle, startup < 200 ms.

**Constraints**: single binary; no cloud/telemetry; default English + multilingual
site; publish publicly.

## Constitution Check

Passed (v1.0.0). Efficiency-first, cross-platform parity, single binary, public
open-source.

## Project Structure

```
src/
├── main.rs            # entry: wire everything, event loop
├── cfg.rs             # Config struct + TOML load
├── clock.rs           # shared timer state machine (platform-agnostic)
├── ui.slint -> ui/appwindow.slint
└── platform/
    ├── mod.rs         # trait Platform + dispatch by cfg
    ├── tray.rs        # Tray trait
    ├── notify.rs      # Notifier trait
    ├── hotkey.rs      # Hotkey wrapper
    ├── inhibit.rs     # IdleInhibit trait
    ├── autostart.rs   # Autostart trait
    ├── linux/mod.rs   # ksni tray, notify-rust, zbus inhibit, .desktop
    ├── windows/mod.rs # tray-icon, winrt-notification, ExecuteState, Run key
    └── macos/mod.rs   # tray-icon, notify-rust, caffeinate, LaunchAgent
```

## Milestones

1. Scaffold platform module tree + traits; keep existing Linux behavior.
2. Linux: swap to ksni tray; verify build locally with staged dev pkgs.
3. Windows + macOS modules (cfg-gated), compile-verified via CI.
4. CI matrix (ubuntu/windows/macos) build + test; release artifacts.
5. Site + Pages; public README; tag release.
