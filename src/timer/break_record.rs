//! Recorded breaks: the other half of the Pomodoro method.
//!
//! A Pomodoro timer counts focus blocks, but a block's *quality* lives in the
//! spaces between them. A break that ran 5 minutes past its plan and one that
//! was cut short at 1 minute both read as "a break happened" in a tally; only
//! a record of each break's plan against its actual length can tell them
//! apart. This module is that record.
//!
//! Breaks share the shape of [`SessionRecord`](crate::state::store::SessionRecord):
//! they are append-only facts — a completed break is a thing that happened,
//! not a row an editor rewrites. They are therefore keyed by their content
//! (`started_at` + `kind`) and merged as a union, exactly like sessions, with
//! no id counter and no tombstone. A device-local `id` would collide across
//! devices (two machines each start a break at id 1) and silently clobber one
//! side of the merge; the start time and kind identify a break unambiguously.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// A break's place in the work cycle: short (between two focus blocks) or long
/// (after the configured run of blocks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BreakRel {
    /// Between two adjacent focus blocks.
    Short,
    /// After a full run (the long-break interval).
    Long,
}

impl BreakRel {
    /// The stable storage form, so a hand-read `data.json` says `"short"`.
    pub fn as_str(self) -> &'static str {
        match self {
            BreakRel::Short => "short",
            BreakRel::Long => "long",
        }
    }
}

/// One completed break: when it ran and how its actual length compared to the
/// plan. `ended_at` is `None` only while the break is still running; a break
/// that has finished always carries an end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BreakRecord {
    /// Unix seconds when the break began.
    pub started_at: i64,
    /// Unix seconds when the break ended. `None` while the break is open.
    pub ended_at: Option<i64>,
    /// How long the break was planned to be, in seconds (from the configured
    /// short/long break minutes at the time it began).
    pub planned_secs: u32,
    /// Whether this was a short or long break.
    pub kind: BreakRel,
}

/// Open a new break record. `ended_at` is `None`: the break is in flight.
pub fn begin(started_at: i64, planned_secs: u32, kind: BreakRel) -> BreakRecord {
    BreakRecord {
        started_at,
        ended_at: None,
        planned_secs,
        kind,
    }
}

/// Close the break at `ended_at`. Returns the completed record.
pub fn close(mut rec: BreakRecord, ended_at: i64) -> BreakRecord {
    rec.ended_at = Some(ended_at);
    rec
}

/// The live records that overlap [`from`, `to`] (inclusive), oldest first.
pub fn breaks_in_range<'a>(records: &'a [BreakRecord], from: i64, to: i64) -> Vec<&'a BreakRecord> {
    let mut out: Vec<&BreakRecord> = records
        .iter()
        .filter(|r| r.started_at >= from && r.started_at <= to)
        .collect();
    out.sort_by_key(|r| r.started_at);
    out
}

/// Whether a break actually finished (as opposed to still being open).
pub fn is_complete(rec: &BreakRecord) -> bool {
    rec.ended_at.is_some()
}

/// Actual length of a completed break in seconds (0 while still open).
pub fn actual_secs(rec: &BreakRecord) -> i64 {
    match rec.ended_at {
        Some(e) => (e - rec.started_at).max(0),
        None => 0,
    }
}

/// Union of two break lists, de-duplicated and in chronological order.
///
/// Breaks are keyed by their content `(started_at, kind)` — the same fact
/// recorded on two devices is one break, but two breaks that started in the
/// same second with different kinds are genuinely two. When both sides have a
/// record at the same key and one is complete while the other is still open,
/// the complete one wins; otherwise the newer ending wins.
pub fn merge(a: &[BreakRecord], b: &[BreakRecord]) -> Vec<BreakRecord> {
    let mut map: std::collections::BTreeMap<(i64, BreakRel), BreakRecord> =
        std::collections::BTreeMap::new();
    for r in a.iter().chain(b.iter()) {
        match map.get(&(r.started_at, r.kind)) {
            None => {
                map.insert((r.started_at, r.kind), r.clone());
            }
            Some(existing) => {
                let keep = match (existing.ended_at, r.ended_at) {
                    (Some(_), None) => existing.clone(),
                    (None, Some(_)) => r.clone(),
                    (None, None) => existing.clone(),
                    (Some(ee), Some(re)) => {
                        if re >= ee {
                            r.clone()
                        } else {
                            existing.clone()
                        }
                    }
                };
                map.insert((r.started_at, r.kind), keep);
            }
        }
    }
    let mut out: Vec<BreakRecord> = map.into_values().collect();
    out.sort_by_key(|r| r.started_at);
    out
}

/// Classify a recorded break against its plan: followed (within ±30%),
/// cut short (skipped), or stretched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AdherenceKind {
    Followed,
    Skipped,
    Stretched,
}

/// Classify a completed break's actual length against its planned length.
pub fn classify_actual(planned_secs: u32, actual_secs: i64) -> AdherenceKind {
    let planned = planned_secs as i64;
    let tol = (planned as f64 * 0.3) as i64;
    if actual_secs < planned - tol {
        AdherenceKind::Skipped
    } else if actual_secs > planned + tol {
        AdherenceKind::Stretched
    } else {
        AdherenceKind::Followed
    }
}

/// Break adherence over the recorded breaks in `records`. Only completed
/// breaks count; an open break (still running) is not yet measurable.
///
/// Returns `None` when fewer than 3 completed breaks are present — a smaller
/// sample than the gap-based [`report::break_adherence`] (5 sessions) because
/// a week rarely contains many breaks, and 3 completed breaks is enough to
/// say something real.
pub fn adherence_summary(records: &[BreakRecord]) -> Option<crate::stats::report::BreakAdherence> {
    let mut followed = 0u32;
    let mut skipped = 0u32;
    let mut stretched = 0u32;
    for r in records {
        let Some(ended) = r.ended_at else {
            continue;
        };
        let actual = (ended - r.started_at).max(0);
        match classify_actual(r.planned_secs, actual) {
            AdherenceKind::Followed => followed += 1,
            AdherenceKind::Skipped => skipped += 1,
            AdherenceKind::Stretched => stretched += 1,
        }
    }
    let total = followed + skipped + stretched;
    if total < 3 {
        return None;
    }
    let rate = followed as f32 / total as f32;
    Some(crate::stats::report::BreakAdherence {
        followed,
        skipped,
        stretched,
        rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(started: i64, ended: Option<i64>, planned: u32, kind: BreakRel) -> BreakRecord {
        BreakRecord {
            started_at: started,
            ended_at: ended,
            planned_secs: planned,
            kind,
        }
    }

    #[test]
    fn begin_creates_open_record() {
        let r = begin(1000, 300, BreakRel::Short);
        assert_eq!(r.started_at, 1000);
        assert_eq!(r.ended_at, None);
        assert_eq!(r.planned_secs, 300);
        assert_eq!(r.kind, BreakRel::Short);
        assert!(!is_complete(&r));
    }

    #[test]
    fn close_fills_ended_at() {
        let r = close(begin(1000, 300, BreakRel::Short), 1300);
        assert_eq!(r.ended_at, Some(1300));
        assert!(is_complete(&r));
        assert_eq!(actual_secs(&r), 300);
    }

    #[test]
    fn actual_secs_is_zero_while_open() {
        let r = begin(1000, 300, BreakRel::Long);
        assert_eq!(actual_secs(&r), 0);
    }

    #[test]
    fn breaks_in_range_respects_bounds() {
        let records = vec![
            rec(1000, Some(1300), 300, BreakRel::Short),
            rec(2000, Some(2400), 1200, BreakRel::Long),
            rec(3000, Some(3200), 300, BreakRel::Short),
        ];
        let in_range = breaks_in_range(&records, 1500, 2900);
        assert_eq!(in_range.len(), 1);
        assert_eq!(in_range[0].started_at, 2000);
    }

    #[test]
    fn breaks_in_range_includes_bounds_inclusively() {
        let records = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        assert_eq!(breaks_in_range(&records, 1000, 1000).len(), 1);
        assert_eq!(breaks_in_range(&records, 999, 999).len(), 0);
    }

    #[test]
    fn breaks_in_range_sorts_oldest_first() {
        let records = vec![
            rec(2000, Some(2400), 1200, BreakRel::Long),
            rec(1000, Some(1300), 300, BreakRel::Short),
        ];
        let in_range = breaks_in_range(&records, 0, 1_000_000);
        assert_eq!(in_range[0].started_at, 1000);
        assert_eq!(in_range[1].started_at, 2000);
    }

    #[test]
    fn merge_dedupes_by_start_and_kind() {
        let a = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        let b = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn merge_keeps_distinct_kinds_at_same_start() {
        let a = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        let b = vec![rec(1000, Some(2200), 1200, BreakRel::Long)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_complete_beats_open() {
        let a = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        let b = vec![rec(1000, None, 300, BreakRel::Short)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].ended_at, Some(1300));
    }

    #[test]
    fn merge_union_of_disjoint_lists() {
        let a = vec![rec(1000, Some(1300), 300, BreakRel::Short)];
        let b = vec![rec(5000, Some(5400), 300, BreakRel::Short)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn classify_actual_followed_within_tolerance() {
        assert_eq!(classify_actual(300, 300), AdherenceKind::Followed);
        assert_eq!(classify_actual(300, 330), AdherenceKind::Followed);
        assert_eq!(classify_actual(300, 270), AdherenceKind::Followed);
    }

    #[test]
    fn classify_actual_skipped_and_stretched() {
        assert_eq!(classify_actual(300, 100), AdherenceKind::Skipped);
        assert_eq!(classify_actual(300, 400), AdherenceKind::Stretched);
    }

    #[test]
    fn adherence_summary_counts_completed_breaks() {
        let records = vec![
            rec(1000, Some(1300), 300, BreakRel::Short), // followed
            rec(2000, Some(2300), 300, BreakRel::Short), // followed
            rec(3000, Some(3100), 300, BreakRel::Short), // skipped
            rec(4000, Some(4600), 300, BreakRel::Short), // stretched
            rec(5000, Some(5300), 300, BreakRel::Short), // followed
        ];
        let s = adherence_summary(&records).unwrap();
        assert_eq!(s.followed, 3);
        assert_eq!(s.skipped, 1);
        assert_eq!(s.stretched, 1);
        assert_eq!(s.rate, 0.6);
    }

    #[test]
    fn adherence_summary_ignores_open_breaks() {
        let records = vec![
            rec(1000, Some(1300), 300, BreakRel::Short),
            rec(2000, Some(2300), 300, BreakRel::Short),
            rec(3000, Some(3300), 300, BreakRel::Short),
            rec(4000, None, 300, BreakRel::Short), // open
        ];
        assert_eq!(adherence_summary(&records).unwrap().followed, 3);
    }

    #[test]
    fn adherence_summary_requires_at_least_three_completed() {
        let records = vec![
            rec(1000, Some(1300), 300, BreakRel::Short),
            rec(2000, Some(2300), 300, BreakRel::Short),
        ];
        assert!(adherence_summary(&records).is_none());
    }
}
