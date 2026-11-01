# Pangan Constitution

## Core Principles

### I. Efficiency-First Runtime
Pangan is a focus timer. The non-negotiable contract is minimal resource usage:
idle RAM target ≤ ~8 MB, CPU ~0% when idle, instant startup (< 200 ms to
window/tray). Every dependency, thread, and wake must justify its cost. No
Electron or heavyweight runtimes. A timer that bloats the machine defeats its
purpose.

### II. Drift-Free Timer Core
Time is source of truth. The UI is a projection of the timer state, never the
other way around. Countdown MUST be computed from an absolute dead (finish_at)
against the monotonic clock — never accumulated from `sleep()` ticks. Pause
captures remaining time; resume rebuilds the deadline. Skipping, resetting, and
phase transitions (Focus ↔ Break) must be instantaneous and consistent.

### III. Cross-Platform Parity by Default
Pangan is a first-class citizen on Linux, Windows, and macOS. Shared timer
logic, config, and UI run identically everywhere. OS-specific facilities
(system tray, notifications, global hotkey, idle-inhibit, autostart) are
behind a common trait/interface, with per-platform implementations selected by
`cfg(target_os)`. No feature is implemented for only one OS unless the platform
provides no equivalent; such gaps are documented in the spec.

### IV. Single Native Binary + Zero-Fuss Distribution
The product ships as one native binary per OS with no runtime dependencies,
registration, or service installation. Autostart is configured by the app
itself on first run (desktop entry on Linux, Run key on Windows, LaunchAgent on
macOS). Users install by downloading/building and running once — no package
manager gymnastics.

### V. Default-English, Multilingual Companion Site
The application UI defaults to English. The companion marketing/documentation
site ships in English by default and provides multilingual (`data-lang`)
support. The site publishes to GitHub Pages.

### VI. Public, Open-Source Deliverable
This is a public project. Quality gates: clean release build on every target
OS, no dead code, no leaked credentials, a usable README, a working GitHub
Actions CI matrix (linux/windows/macos) that builds and runs checks. Tagged
releases are the public artifact.

## Constraints & Performance Standards

- Rust + Slint. Single binary. Target release profile with `lto`, `opt-level
  3`, `strip`, `panic = "abort"`.
- System library footprint kept minimal. Linux build requires fontconfig +
  xkbcommon dev packages (documented).
- Config: `~/.config/pangan/config.toml` (Linux) with per-OS config path
  resolution (Windows AppData, macOS Library/Application Support).

## Development Workflow

- Spec Kit driven: constitution → spec → plan → tasks → implement → converge.
- Each milestone ends with a verified build and, when complete, a commit.
- The trait interface for OS features stays stable; adding an OS means adding
  one implementation module, not rewriting core.

## Governance

This constitution supersedes ad-hoc decisions. Amendments require updating this
file, bumping the version, and reflecting the change in the spec. All code must
resolve through the platform interface; platform-specific hacks that leak into
core logic are rejected in review.

**Version**: 1.0.0 | **Ratified**: 2026-10-28 | **Last Amended**: 2026-10-28
