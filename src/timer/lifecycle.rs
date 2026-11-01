//! Mobile lifecycle correctness — the timer under suspension.
//!
//! On desktop the app is alive for the whole block: its 300 ms tick drives
//! the countdown, and the moment a deadline is crossed the session is
//! recorded. A mobile app cannot rely on that. The OS can suspend or kill it
//! at any moment; no Rust code runs while it is suspended. The timer must
//! therefore be *derivable from durable state and the wall clock*:
//!
//!   * the deadline is a Unix timestamp (already true in `clock.rs`);
//!   * enough state is persisted to reconstruct the block on resume;
//!   * completion is *idempotent* — suspending past the deadline must record
//!     the session exactly once, never twice, and never fabricate extra
//!     sessions for phases that were never actually run;
//!   * the recorded session length is the block's *actual planned length*,
//!     not the configured default focus duration.
//!
//! This module factors those decisions into pure functions over the persisted
//! [`SessionSnapshot`], with an injectable clock so every scenario is tested
//! deterministically. The mobile hosts use it to drive recovery on launch;
//! the desktop keeps its live tick, but the same correctness rules apply.

use crate::timer::session_snapshot::{Phase, SessionSnapshot, RECOVERY_GRACE_SECS};

/// A source of wall-clock time in Unix seconds. The real app uses the actual
/// clock; tests inject a fixed time.
pub trait Clock {
    fn now_unix(&self) -> i64;
}

/// The real wall clock.
pub struct RealClock;

impl Clock for RealClock {
    fn now_unix(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

/// A clock with a controllable current time, for deterministic tests.
pub struct SimClock {
    now: i64,
}

impl SimClock {
    pub fn new(now: i64) -> Self {
        Self { now }
    }
    pub fn set(&mut self, now: i64) {
        self.now = now;
    }
}

impl Clock for SimClock {
    fn now_unix(&self) -> i64 {
        self.now
    }
}

/// What the app should do when it comes back from a suspension / relaunch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Nothing durable to do: no snapshot, or it is stale/finished.
    Nothing,
    /// Resume the in-flight block with the given remaining seconds. `running`
    /// is true if the block was running (not paused) when snapshotted.
    Resume { remaining_secs: u64, running: bool },
    /// The block's deadline passed while we were away: record one completed
    /// session and do not resume anything.
    CompleteAndClear,
}

/// Decide what to do with a persisted snapshot now that the app is awake.
///
/// `now` is the current wall-clock time. The verdict is pure (no I/O), so it
/// is fully deterministic.
///
/// Policy:
///
/// * A *paused* block keeps its frozen remaining time. If the last activity is
///   still within grace, it is resumed paused; otherwise it is abandoned.
/// * A *running* block that has not reached its deadline is resumed running if
///   within grace.
/// * A *running* block whose deadline passed while the app was suspended was a
///   genuine session that ran to completion; if still within grace it is
///   recorded exactly once (CompleteAndClear), never resumed.
/// * Anything beyond grace is stale and treated as abandoned — we never
///   fabricate a session for time the user was simply away.
pub fn recovery_verdict(snap: &SessionSnapshot, now: i64) -> RecoveryAction {
    if snap.started_at == 0 {
        return RecoveryAction::Nothing;
    }

    let remaining = snap.remaining_secs(now);
    let within_grace = now.saturating_sub(snap.last_activity()) <= RECOVERY_GRACE_SECS;

    if remaining == 0 {
        if within_grace {
            return RecoveryAction::CompleteAndClear;
        }
        return RecoveryAction::Nothing;
    }

    if within_grace {
        let running = snap.paused_at.is_none();
        return RecoveryAction::Resume {
            remaining_secs: remaining,
            running,
        };
    }

    RecoveryAction::Nothing
}

/// Build the session record for a just-completed block.
///
/// The recorded length is the block's *actual planned length*
/// (`planned_duration_secs`), not the configured default duration — a rota
/// block planned for fifty minutes must be recorded as fifty minutes.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedSession {
    pub at_unix: i64,
    pub minutes: u64,
    pub phase: Phase,
    pub rota_entry_id: Option<u64>,
    pub decree_text: Option<String>,
}

pub fn completed_session(snap: &SessionSnapshot, now: i64) -> CompletedSession {
    CompletedSession {
        at_unix: now,
        minutes: (snap.planned_duration_secs / 60).max(1),
        phase: snap.phase,
        rota_entry_id: snap.rota_entry_id,
        decree_text: snap.decree_text.clone(),
    }
}

/// Whether a completion was already recorded for this block.
///
/// Idempotency: `Store::add_sessions` dedupes by `(at, minutes)`, but the
/// app must never *attempt* to record the same completion twice in the first
/// place. We can detect a duplicate by checking whether a session already
/// exists for the block's completion instant. This is the guard the host
/// calls before writing a completion.
pub fn completion_already_recorded(
    existing: &[crate::state::store::SessionRecord],
    snap: &SessionSnapshot,
    now: i64,
) -> bool {
    let minutes = (snap.planned_duration_secs / 60).max(1);
    existing.iter().any(|s| s.at == now && s.minutes == minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(started_at: i64, dur_secs: u64) -> SessionSnapshot {
        SessionSnapshot {
            started_at,
            planned_duration_secs: dur_secs,
            phase: Phase::Focus,
            rota_entry_id: Some(7),
            decree_text: Some("write report".into()),
            paused_at: None,
            paused_remaining_secs: 0,
        }
    }

    fn paused(started_at: i64, dur_secs: u64, remaining: u64) -> SessionSnapshot {
        let mut s = snap(started_at, dur_secs);
        s.paused_at = Some(started_at + 60);
        s.paused_remaining_secs = remaining;
        s
    }

    #[test]
    fn no_snapshot_is_nothing() {
        let s = SessionSnapshot {
            started_at: 0,
            planned_duration_secs: 0,
            phase: Phase::Focus,
            rota_entry_id: None,
            decree_text: None,
            paused_at: None,
            paused_remaining_secs: 0,
        };
        assert_eq!(recovery_verdict(&s, 1000), RecoveryAction::Nothing);
    }

    #[test]
    fn resume_before_deadline_running() {
        let s = snap(1000, 1500); // started t=1000, runs 25 min
        assert_eq!(
            recovery_verdict(&s, 1100),
            RecoveryAction::Resume {
                remaining_secs: 1400,
                running: true
            }
        );
    }

    #[test]
    fn resume_paused_frozen() {
        let s = paused(1000, 1500, 700);
        assert_eq!(
            recovery_verdict(&s, 2000),
            RecoveryAction::Resume {
                remaining_secs: 700,
                running: false
            }
        );
    }

    #[test]
    fn resume_after_deadline_is_complete_not_resume() {
        let s = snap(1000, 600); // 10-minute block
                                 // t=1700 > deadline 1600, within grace (30 min) -> complete once.
        assert_eq!(recovery_verdict(&s, 1700), RecoveryAction::CompleteAndClear);
    }

    #[test]
    fn stale_finished_block_is_not_completed() {
        let s = snap(1000, 600);
        // Hours later, way past grace (1800s).
        assert_eq!(
            recovery_verdict(&s, 1000 + 30 * 60 + 3600),
            RecoveryAction::Nothing
        );
    }

    #[test]
    fn stale_paused_not_resumed() {
        let s = paused(1000, 1500, 700);
        // paused_at = 1060; grace is 1800s -> beyond 2860 is stale.
        assert_eq!(recovery_verdict(&s, 3000), RecoveryAction::Nothing);
    }

    #[test]
    fn session_uses_actual_planned_length() {
        // A 50-minute rota block must be recorded as 50, not the default 25.
        let s = snap(1000, 3000);
        let c = completed_session(&s, 4000);
        assert_eq!(c.minutes, 50);
        assert_eq!(c.at_unix, 4000);
        assert_eq!(c.rota_entry_id, Some(7));
        assert_eq!(c.decree_text.as_deref(), Some("write report"));
    }

    #[test]
    fn short_blocks_floor_at_one_minute() {
        let s = snap(1000, 30);
        assert_eq!(completed_session(&s, 1050).minutes, 1);
    }

    #[test]
    fn idempotent_completion_detected() {
        let s = snap(1000, 3000);
        let existing = vec![crate::state::store::SessionRecord {
            at: 4000,
            minutes: 50,
        }];
        assert!(completion_already_recorded(&existing, &s, 4000));
        assert!(!completion_already_recorded(&[], &s, 4000));
    }

    #[test]
    fn idempotent_completion_distinguishes_moments() {
        let s = snap(1000, 3000);
        let existing = vec![crate::state::store::SessionRecord {
            at: 3999,
            minutes: 50,
        }];
        // Different completion instant -> not a duplicate.
        assert!(!completion_already_recorded(&existing, &s, 4000));
    }
}
