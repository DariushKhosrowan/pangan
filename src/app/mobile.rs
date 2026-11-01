//! Mobile application host.
//!
//! The desktop [`super::run`] wires desktop-only hooks (tray, hotkeys,
//! autostart, portal theme, gdbus/gsettings, close-to-tray). A mobile host
//! must not call any of that, but it must still:
//!
//! * run the same once-per-day/week journal writers;
//! * recover an interrupted focus block using the lifecycle verdict;
//! * host the *unchanged* Slint interface (`ui/appwindow.slint`);
//! * drive the shared timer and persist completed sessions idempotently.
//!
//! This module is that mobile host. The Android/iOS entry crates call
//! [`run_hosted`] after initializing their native backend and installing the
//! platform adapter, so every mobile host runs the same Rust core.

use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode};

use crate::state::{settings, store};
use crate::timer::clock::{self, Shared};
use crate::timer::lifecycle::{self, RecoveryAction};
use crate::timer::session_snapshot::SessionSnapshot;
use crate::ui::PanganWindow;

/// Run the mobile app. `storage_root` is the application-private directory
/// (set before anything reads the store).
pub fn run_hosted(storage_root: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    // The platform adapter decides where data lives.
    crate::platform::caps::set_storage_root(storage_root);

    crate::log::init();
    crate::log::info("pangan mobile starting");

    let settings = settings::Settings::load();
    let cfg = clock::Config::from(&settings);
    let shared = Arc::new(Shared::new(cfg));
    let mut store = store::Store::load();

    // Same once-per-period journal writers the desktop runs at startup. These
    // are pure and portable; running them keeps mobile and desktop histories
    // identical.
    write_period_documents(&mut store, &settings);

    // Recover an interrupted focus block, if any.
    match recover_focus_block(&shared) {
        RecoveryAction::Resume {
            remaining_secs,
            running,
        } => {
            shared.begin_from_recovery(snap_duration_secs(), remaining_secs, running);
        }
        RecoveryAction::CompleteAndClear => {
            // The block finished while suspended; record it once.
            if let Some(snap) = SessionSnapshot::load() {
                let now = chrono::Utc::now().timestamp();
                let cs = lifecycle::completed_session(&snap, now);
                if !store
                    .data
                    .sessions
                    .iter()
                    .any(|s| s.at == cs.at_unix && s.minutes == cs.minutes)
                {
                    store.data.sessions.push(store::SessionRecord {
                        at: cs.at_unix,
                        minutes: cs.minutes,
                    });
                    store.save();
                }
                SessionSnapshot::clear();
            }
        }
        RecoveryAction::Nothing => {}
    }

    let window = PanganWindow::new()?;
    let weak = window.as_weak();

    // Countdown tick: same 300 ms cadence as desktop, same wall-clock core.
    // Completed sessions are drained from the clock and appended idempotently
    // (the store dedupes by `(at, minutes)`), then the recovery snapshot is
    // retired so a crash later cannot re-record them.
    let tick_shared = Arc::clone(&shared);
    let tick_store = store::Store::load();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(300), move || {
        let recs = tick_shared.drain_sessions();
        let break_records = tick_shared.drain_break_records();
        if recs.is_empty() && break_records.is_empty() {
            return;
        }
        let mut st = store::Store::load();
        st.add_sessions(&recs);
        st.add_breaks(&break_records);
        tick_shared.clear_started_at();
        SessionSnapshot::clear();
    });
    let _ = (timer, tick_store);

    // Run the Slint event loop for the window's lifetime.
    if let Some(win) = weak.upgrade() {
        win.run()?;
    }
    Ok(())
}

fn snap_duration_secs() -> u64 {
    SessionSnapshot::load()
        .map(|s| s.planned_duration_secs)
        .unwrap_or(1500)
}

fn recover_focus_block(shared: &Arc<Shared>) -> RecoveryAction {
    let _ = shared;
    let Some(snap) = SessionSnapshot::load() else {
        return RecoveryAction::Nothing;
    };
    let now = chrono::Utc::now().timestamp();
    let verdict = lifecycle::recovery_verdict(&snap, now);
    if verdict == RecoveryAction::Nothing {
        SessionSnapshot::clear();
    }
    verdict
}

fn write_period_documents(store: &mut store::Store, settings: &settings::Settings) {
    let today = chrono::Local::now().date_naive();
    let tz = crate::stats::report::local_tz_offset_secs();
    let break_secs = (settings.short_break_minutes * 60) as u32;
    let en = settings.lang != crate::state::settings::LANG_FA;
    let _ = crate::journal::weekly_review::write_if_new(
        &store.data.sessions,
        &store.data.decrees,
        &store.data.inscriptions,
        &store.data.verdicts,
        &store.data.breaks,
        today,
        tz,
        break_secs,
        en,
    );
    let _ = crate::journal::daily_briefing::write_if_new(
        &store.data.sessions,
        &store.data.decrees,
        &store.data.inscriptions,
        &store.data.verdicts,
        &store.data.breaks,
        &store.data.rota,
        &store.data.prescriptions,
        today,
        tz,
        break_secs,
        en,
    );
}
