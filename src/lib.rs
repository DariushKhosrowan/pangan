//! Pangan — a Persian Pomodoro timer.
//!
//! This library crate exposes the full application core so that platform
//! entry-point crates (desktop binary, Android, iOS) can reuse the same domain
//! logic: timer, state, sync, calendar, stats, journal, audio, platform
//! capabilities, and logging.
//!
//! The desktop binary lives in `src/main.rs` and calls [`app::run`]. Android
//! and iOS entry points (in `platforms/`) host the same Slint interface via
//! the modules declared here.
//!
//! Module map:
//!
//! * [`app`]       — startup, the UI event loop, shell automation hooks
//! * [`timer`]     — the drift-free countdown core; lifecycle recovery
//! * [`state`]     — settings and the session/task store
//! * [`sync`]      — Google Drive sync, merge, and backup
//! * [`calendar`]  — Jalali calendar, Sasanian day names, chronicle, events
//! * [`stats`]     — reports and streaks
//! * [`journal`]   — coaching, prescriptions, weekly/daily review
//! * [`audio`]     — procedural Iranian instrument synthesis
//! * [`platform`]  — per-OS desktop tray/notify/hotkey/inhibit/autostart and
//!                   cross-platform capability traits ([`platform::caps`])
//! * [`log`]       — leveled file logging

pub mod app;
pub mod audio;
pub mod calendar;
pub mod journal;
pub mod log;
pub mod platform;
pub mod state;
pub mod stats;
pub mod sync;
pub mod timer;

pub mod ui {
    slint::include_modules!();
}
