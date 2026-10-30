//! Pangan — a Persian Pomodoro timer.
//!
//! Module map:
//!
//! * [`app`]       — startup, the UI event loop, shell automation hooks
//! * [`timer`]     — the drift-free countdown core
//! * [`state`]     — settings and the session/task store
//! * [`sync`]      — Google Drive sync, merge, and backup
//! * [`calendar`]  — Jalali calendar, Sasanian day names, chronicle, events
//! * [`stats`]     — reports and streaks
//! * [`audio`]     — procedural Iranian instrument synthesis
//! * [`platform`]  — per-OS tray, notifications, hotkeys, inhibit, autostart

mod app;
mod audio;
mod calendar;
mod journal;
mod platform;
mod state;
mod stats;
mod sync;
mod timer;

pub(crate) mod ui {
    slint::include_modules!();
}

fn main() {
    if let Err(e) = app::run() {
        eprintln!("pangan: fatal: {e}");
        std::process::exit(1);
    }
}
