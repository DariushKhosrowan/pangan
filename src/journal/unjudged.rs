//! Unjudged blocks: sessions that ended without a verdict.
//!
//! The app's thesis is that a plan written down and faced honestly is worth
//! more than a timer that merely counts. A block that ends and never receives
//! a verdict is a promise that was neither kept nor broken — just dropped.
//! That is the single biggest gap in the thesis, because it is invisible: a
//! tally of kept/broken only counts the blocks the user *did* face.
//!
//! This module finds those dropped blocks: completed sessions in a span that
//! have no live verdict. It is read-only — it never writes, only reports — so
//! a weekly review can surface "here are the blocks you walked away from
//! without facing."
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// One unjudged block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnjudgedBlock {
    /// Unix seconds when the session ended — the join key.
    pub session_at: i64,
    /// The block's scheduled length, in minutes.
    pub minutes: u32,
    /// Whole days since the block's local day (0 = today).
    pub days_old: i64,
    /// Whether the block had a decree written for it before it ran. A block
    /// that was spoken for and then ignored is a harsher miss than one that
    /// was just a timer run.
    pub has_decree: bool,
}

/// Local date (Y-m-d under the given UTC offset) for a timestamp.
fn local_date(ts: i64, tz_offset_secs: i32) -> Option<chrono::NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)?;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&offset).date_naive())
}

/// The unjudged sessions in [`from`, `to`] (inclusive timestamps), oldest
/// first. A session counts as judged if it has a *live* verdict (not
/// tombstoned); an inferred reading from a decree/inscription pair does **not**
/// count, because this report is about the blocks the user never faced at all.
pub fn unjudged_sessions(
    sessions: &[crate::state::store::SessionRecord],
    verdicts: &[crate::journal::Verdict],
    decrees: &[crate::journal::Decree],
    now: i64,
    from: i64,
    to: i64,
    tz_offset_secs: i32,
) -> Vec<UnjudgedBlock> {
    let judged: std::collections::HashSet<i64> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| v.session_at)
        .collect();
    let has_decree: std::collections::HashSet<i64> = decrees
        .iter()
        .filter(|d| !d.deleted)
        .map(|d| d.session_at)
        .collect();

    let today = local_date(now, tz_offset_secs);
    let mut out: Vec<UnjudgedBlock> = Vec::new();
    for s in sessions {
        if s.at < from || s.at > to {
            continue;
        }
        if judged.contains(&s.at) {
            continue;
        }
        let days_old = match (today, local_date(s.at, tz_offset_secs)) {
            (Some(t), Some(d)) => (t - d).num_days(),
            _ => 0,
        };
        out.push(UnjudgedBlock {
            session_at: s.at,
            minutes: s.minutes as u32,
            days_old,
            has_decree: has_decree.contains(&s.at),
        });
    }
    out.sort_by_key(|b| b.session_at);
    out
}

/// Cheap count of unjudged sessions in a span (no structured rows).
pub fn unjudged_count(
    sessions: &[crate::state::store::SessionRecord],
    verdicts: &[crate::journal::Verdict],
    from: i64,
    to: i64,
) -> u32 {
    let judged: std::collections::HashSet<i64> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| v.session_at)
        .collect();
    sessions
        .iter()
        .filter(|s| s.at >= from && s.at <= to && !judged.contains(&s.at))
        .count() as u32
}

/// A one-line label for the backlog, in the requested locale. Uses Persian
/// digits when `locale_fa` is true.
pub fn backlog_label(blocks: &[UnjudgedBlock], locale_fa: bool) -> String {
    let n = blocks.len();
    if locale_fa {
        format!(
            "{} بلوک بدون داوری",
            crate::calendar::jalali::fa_num(n as i64)
        )
    } else {
        format!("{n} unjudged block{}", if n == 1 { "" } else { "s" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::VerdictKind;
    use crate::state::store::SessionRecord;

    const TZ: i32 = 12600; // Iran UTC+3:30

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    fn verdict(session_at: i64, deleted: bool) -> crate::journal::Verdict {
        crate::journal::Verdict {
            session_at,
            kind: VerdictKind::Kept,
            declared_at: session_at,
            deleted,
        }
    }

    fn decree(session_at: i64, deleted: bool) -> crate::journal::Decree {
        crate::journal::decree::create(session_at, session_at - 100, "plan")
            .map(|mut d| {
                d.deleted = deleted;
                d
            })
            .unwrap()
    }

    #[test]
    fn empty_when_all_sessions_judged() {
        let sessions = vec![sess(1000, 25), sess(2000, 25)];
        let verdicts = vec![verdict(1000, false), verdict(2000, false)];
        let out = unjudged_sessions(&sessions, &verdicts, &[], 2000, 0, 3000, TZ);
        assert!(out.is_empty());
    }

    #[test]
    fn detects_session_without_verdict() {
        let sessions = vec![sess(1000, 25), sess(2000, 25)];
        let verdicts = vec![verdict(1000, false)];
        let out = unjudged_sessions(&sessions, &verdicts, &[], 2000, 0, 3000, TZ);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_at, 2000);
        assert_eq!(out[0].minutes, 25);
    }

    #[test]
    fn excludes_tombstoned_verdict() {
        let sessions = vec![sess(1000, 25)];
        let verdicts = vec![verdict(1000, true)];
        let out = unjudged_sessions(&sessions, &verdicts, &[], 1000, 0, 3000, TZ);
        assert_eq!(out.len(), 1, "a withdrawn verdict does not judge the block");
    }

    #[test]
    fn respects_date_range() {
        let sessions = vec![sess(1000, 25), sess(10_000, 25)];
        let out = unjudged_sessions(&sessions, &[], &[], 20_000, 0, 5000, TZ);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_at, 1000);
    }

    #[test]
    fn days_old_honors_tz_offset() {
        // 24h apart in UTC, but in Iran (UTC+3:30) they can be on the same or
        // adjacent day. Use two clearly-separated days.
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let day_ago = now - 86_400;
        let sessions = vec![sess(day_ago, 25)];
        let out = unjudged_sessions(&sessions, &[], &[], now, 0, now, TZ);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].days_old, 1);
    }

    #[test]
    fn has_decree_flag_set_correctly() {
        let sessions = vec![sess(1000, 25), sess(2000, 25)];
        let decrees = vec![decree(1000, false)];
        let out = unjudged_sessions(&sessions, &[], &decrees, 2000, 0, 3000, TZ);
        assert!(out[0].has_decree);
        assert!(!out[1].has_decree);
    }

    #[test]
    fn unjudged_count_is_cheap_count() {
        let sessions = vec![sess(1000, 25), sess(2000, 25), sess(3000, 25)];
        let verdicts = vec![verdict(1000, false)];
        assert_eq!(unjudged_count(&sessions, &verdicts, 0, 10_000), 2);
    }

    #[test]
    fn backlog_label_persian_digits_when_locale_fa() {
        let blocks = vec![UnjudgedBlock {
            session_at: 1,
            minutes: 25,
            days_old: 0,
            has_decree: false,
        }];
        let fa = backlog_label(&blocks, true);
        assert!(fa.contains("۱"), "Persian digit is used");
        assert!(fa.contains("بلوک"));
        let en = backlog_label(&blocks, false);
        assert!(en.contains("1 unjudged block"));
    }
}
