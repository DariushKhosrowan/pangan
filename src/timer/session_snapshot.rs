//! Crash-safe active-session recovery.
//!
//! A focus block is the user's most valuable unit of time. If the app dies
//! mid-block — a crash, a power cut, a forced kill — the 20 minutes already
//! spent are currently lost: the countdown vanishes and the session is never
//! recorded. This module fixes that by persisting a small snapshot of any
//! in-flight focus block to disk, so a later launch can pick the timer back up
//! where it left off.
//!
//! The snapshot is deliberately *not* part of the cloud-synced [`Data`] store:
//! an in-progress block is machine-local and transient. Writing it into
//! `Data` would make a sync carry a half-finished session to every other
//! device, where it would have no meaning — and the merge would have to
//! reason about a running timer it cannot see. So the snapshot lives in its
//! own file next to `data.json`, is written on every block start, and is
//! cleared the moment the block ends, is skipped, or is reset.
//!
//! ## The grace window
//!
//! Not every leftover file is worth resuming. If the app has been closed for
//! hours, the "block" is stale and the user has clearly moved on. A snapshot
//! is only *recoverable* if it is younger than [`RECOVERY_GRACE_SECS`] — the
//! time you would tolerate being away from the desk mid-focus. After that it
//! is silently discarded, not offered.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A focus block is only resumed if we are within this many seconds of its
/// last known activity. Thirty minutes past the last pause, the user has
/// obviously moved on and a stale countdown would be a nuisance.
pub const RECOVERY_GRACE_SECS: i64 = 1800;

/// The phase of the app at the moment the block was snapshotted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

/// A persisted record of an in-flight block.
///
/// Every field is part of the on-disk format, so a change here is a format
/// change; keep `#[serde(default)]` on any new optional field so older
/// snapshots still parse.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Unix seconds when the block started.
    pub started_at: i64,
    /// The block's total planned length, in seconds.
    pub planned_duration_secs: u64,
    /// What kind of block this is.
    pub phase: Phase,
    /// The id of the rota entry this block answers, if any. Preserved so a
    /// recovered block still claims its planned row when it completes.
    pub rota_entry_id: Option<u64>,
    /// The decree text parked for this block, if any. Preserved so the
    /// intention written for the block is not lost in a crash.
    pub decree_text: Option<String>,
    /// When the block was paused, if it was paused at snapshot time. When set,
    /// the block is not running and `remaining_secs` reports the paused
    /// remaining time. When `None`, the block was running and the countdown
    /// continues from `started_at + planned_duration_secs`.
    pub paused_at: Option<i64>,
    /// The remaining time when paused, in seconds. Meaningful only when
    /// `paused_at` is set; otherwise the remaining is derived from the start.
    #[serde(default)]
    pub paused_remaining_secs: u64,
}

impl SessionSnapshot {
    /// The file the snapshot is persisted to, next to the store's `data.json`.
    pub fn path() -> PathBuf {
        crate::state::store::Store::data_dir().join("session_snapshot.json")
    }

    /// The block's total remaining time at `now`, in seconds.
    ///
    /// * If paused, the remaining is the stored `paused_remaining_secs`.
    /// * If running, the remaining is `planned_duration_secs` minus the time
    ///   elapsed since `started_at`, floored at zero.
    pub fn remaining_secs(&self, now: i64) -> u64 {
        if let Some(paused) = self.paused_at {
            let _ = paused; // the paused timestamp is informational
            self.paused_remaining_secs
        } else {
            let elapsed = (now - self.started_at).max(0);
            self.planned_duration_secs.saturating_sub(elapsed as u64)
        }
    }

    /// Whether this snapshot is worth resuming at `now`.
    ///
    /// True only when the block has not already run its full length and the
    /// last known activity is within [`RECOVERY_GRACE_SECS`].
    pub fn is_recoverable(&self, now: i64) -> bool {
        // A block whose time has fully elapsed had nothing left to resume.
        if self.remaining_secs(now) == 0 {
            return false;
        }
        let last_activity = self.paused_at.unwrap_or(self.started_at);
        now.saturating_sub(last_activity) <= RECOVERY_GRACE_SECS
    }

    /// Save the snapshot to disk, creating the directory if needed.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| format!("encode: {e}"))?;
        std::fs::write(&path, json).map_err(|e| format!("write: {e}"))
    }

    /// Load the snapshot, if a valid one exists.
    pub fn load() -> Option<SessionSnapshot> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Remove the snapshot from disk. A missing file is not an error.
    pub fn clear() {
        let _ = std::fs::remove_file(Self::path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(started_at: i64, dur: u64) -> SessionSnapshot {
        SessionSnapshot {
            started_at,
            planned_duration_secs: dur,
            phase: Phase::Focus,
            rota_entry_id: None,
            decree_text: None,
            paused_at: None,
            paused_remaining_secs: 0,
        }
    }

    #[test]
    fn running_remaining_is_duration_minus_elapsed() {
        let s = snap(1000, 1500); // 25 min block starting at t=1000
                                  // t=1060 -> 60s elapsed -> 1440 remain
        assert_eq!(s.remaining_secs(1060), 1440);
    }

    #[test]
    fn running_remaining_never_goes_below_zero() {
        let s = snap(1000, 1500);
        assert_eq!(s.remaining_secs(10000), 0);
    }

    #[test]
    fn paused_remaining_uses_stored_value() {
        let mut s = snap(1000, 1500);
        s.paused_at = Some(1060);
        s.paused_remaining_secs = 700;
        assert_eq!(s.remaining_secs(2000), 700, "paused time is frozen");
    }

    #[test]
    fn recoverable_within_grace_while_running() {
        let s = snap(1000, 1500);
        assert!(s.is_recoverable(1100), "10s in is recoverable");
        assert!(
            s.is_recoverable(1000 + 1500 - 1),
            "just before end is recoverable"
        );
    }

    #[test]
    fn recoverable_when_paused_freshly() {
        let mut s = snap(1000, 1500);
        s.paused_at = Some(1060);
        s.paused_remaining_secs = 700;
        assert!(s.is_recoverable(1100), "a recent pause is recoverable");
    }

    #[test]
    fn not_recoverable_past_grace() {
        let mut s = snap(1000, 1500);
        s.paused_at = Some(1060);
        s.paused_remaining_secs = 700;
        assert!(
            !s.is_recoverable(1060 + RECOVERY_GRACE_SECS + 1),
            "stale pause must not resume"
        );
    }

    #[test]
    fn not_recoverable_when_fully_elapsed() {
        let s = snap(1000, 10);
        assert!(
            !s.is_recoverable(2000),
            "a finished block has nothing to resume"
        );
    }

    #[test]
    fn serde_roundtrip_preserves_all_fields() {
        let s = SessionSnapshot {
            started_at: 1234,
            planned_duration_secs: 1500,
            phase: Phase::LongBreak,
            rota_entry_id: Some(7),
            decree_text: Some("finish the report".into()),
            paused_at: Some(1300),
            paused_remaining_secs: 900,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_default_for_backward_compat() {
        // A snapshot written before `paused_remaining_secs` existed must still
        // parse, with the new field defaulted to zero.
        let old = r#"{"started_at":1000,"planned_duration_secs":1500,"phase":"Focus","rota_entry_id":null,"decree_text":null,"paused_at":null}"#;
        let s: Result<SessionSnapshot, _> = serde_json::from_str(old);
        assert!(s.is_ok(), "old snapshots must parse: {s:?}");
        assert_eq!(s.unwrap().paused_remaining_secs, 0);
    }
}
