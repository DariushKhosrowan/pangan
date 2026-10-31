//! Prescription ledger: the persistent history of every suggestion.
//!
//! A prescription rendered into a weekly digest is a moment in time. Without
//! a record of it, the app cannot later say "you were told three weeks ago to
//! shorten your blocks" or "you followed that advice and kept-rate moved from
//! 41% to 68%". This module is that record: an append-only ledger of the
//! prescriptions the weekly digest emitted, keyed by the week they were
//! issued for.
//!
//! The ledger follows the same discipline as every other persistent collection
//! in the store:
//!
//! * **Keyed by id** — each row carries a stable `id` from the store's
//!   counter, so two devices cannot both mint the same row for different
//!   content. `next_prescription_id` hands ids out.
//! * **Newest `created_at` wins on merge** — a prescription is a statement
//!   issued at a moment; the later statement about the same row is the one
//!   to keep.
//! * **Tombstone from day one** — `deleted` exists now so a future "remove
//!   this advice" action can travel across devices instead of silently
//!   resurrecting. A tombstone always beats a live copy.
//!
//! Everything here is pure: slices and a counter in, owned data out, no I/O,
//! no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// One ledger row: a prescription issued in a particular week.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prescription {
    /// Stable identity from the store's counter.
    pub id: u64,
    /// The week this was issued for, `YYYY-MM-DD` (the Saturday).
    pub week: String,
    /// Unix seconds when this row was written — the merge stamp.
    pub created_at: i64,
    /// What kind of suggestion this was.
    pub kind: crate::journal::prescription::SuggestionKind,
    /// The metric this advice targets (the row that would move if followed).
    pub target_metric: Option<String>,
    /// The target metric's value at issue time — preserved so an outcome can
    /// be measured against it later instead of recomputed from current data.
    #[serde(default)]
    pub target_value: f32,
    /// The advice text, in the locale it was issued in.
    pub text: String,
    /// The sample size that backed the suggestion at issue time.
    pub sample_n: u32,
    /// Tombstone. Set → this row should be treated as gone everywhere.
    pub deleted: bool,
}

/// Record this week's suggestions into the ledger.
///
/// Only suggestions that have a target metric are recorded — a suggestion
/// without a target cannot be followed up, so ledgering it would just be
/// noise. The target metric's value at issue time is resolved via `value_of`
/// and stored so a later outcome can be measured against it. Returns the new
/// rows (ids drawn from `next_id`).
pub fn record(
    existing: &[Prescription],
    next_id: u64,
    week: &str,
    created_at: i64,
    suggestions: &[crate::journal::prescription::Suggestion],
    value_of: impl Fn(&str) -> Option<f32>,
) -> (Vec<Prescription>, u64) {
    let mut id = next_id;
    let mut rows: Vec<Prescription> = Vec::new();
    for s in suggestions {
        let Some(target) = s.target_metric else {
            continue;
        };
        id += 1;
        rows.push(Prescription {
            id,
            week: week.to_string(),
            created_at,
            kind: s.kind,
            target_metric: Some(target.to_string()),
            target_value: value_of(target).unwrap_or(0.0),
            text: s.text.clone(),
            sample_n: s.sample_n,
            deleted: false,
        });
    }
    let mut out = existing.to_vec();
    out.extend(rows);
    (out, id)
}

/// Merge two ledger lists: keyed by id, newest `created_at` wins, a tombstone
/// beats a live copy, ties broken deterministically by id.
pub fn merge(a: &[Prescription], b: &[Prescription]) -> Vec<Prescription> {
    let mut map: std::collections::BTreeMap<u64, Prescription> = std::collections::BTreeMap::new();
    for p in a.iter().chain(b.iter()) {
        match map.get(&p.id) {
            None => {
                map.insert(p.id, p.clone());
            }
            Some(existing) => {
                if keep(p, existing) {
                    map.insert(p.id, p.clone());
                }
            }
        }
    }
    let mut out: Vec<Prescription> = map.into_values().collect();
    out.sort_by(|x, y| x.week.cmp(&y.week).then_with(|| x.id.cmp(&y.id)));
    out
}

/// Is `candidate` the copy of one ledger row to keep?
fn keep(candidate: &Prescription, existing: &Prescription) -> bool {
    if candidate.created_at != existing.created_at {
        return candidate.created_at > existing.created_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    candidate.id > existing.id
}

/// The latest live prescription for a given metric, if any.
/// Used by follow-up detection (not yet wired into UI).
#[allow(dead_code)] // logic API, not yet wired (UI locked)
pub fn latest_for_metric<'a>(rows: &'a [Prescription], metric: &str) -> Option<&'a Prescription> {
    rows.iter()
        .filter(|p| !p.deleted && p.target_metric.as_deref() == Some(metric))
        .max_by_key(|p| p.created_at)
}

/// Whether the same advice has been issued `n` or more distinct weeks running
/// for a metric without the target having moved (a "stubborn prescription").
/// Used by stubborn-prescription detection (not yet wired into UI).
#[allow(dead_code)] // logic API, not yet wired (UI locked)
pub fn repeated_consecutive_weeks(rows: &[Prescription], metric: &str, n: usize) -> bool {
    let mut weeks: Vec<&str> = rows
        .iter()
        .filter(|p| !p.deleted && p.target_metric.as_deref() == Some(metric))
        .map(|p| p.week.as_str())
        .collect();
    weeks.sort_unstable();
    weeks.dedup();
    // Consecutive distinct weeks.
    let mut run = 0usize;
    let mut prev: Option<&str> = None;
    for w in weeks {
        match prev {
            None => run = 1,
            Some(p) => {
                if consecutive(p, w) {
                    run += 1;
                } else {
                    run = 1;
                }
            }
        }
        prev = Some(w);
        if run >= n {
            return true;
        }
    }
    false
}

/// Whether two `YYYY-MM-DD` week-Saturdays are consecutive (7 days apart).
fn consecutive(a: &str, b: &str) -> bool {
    let pa = chrono::NaiveDate::parse_from_str(a, "%Y-%m-%d");
    let pb = chrono::NaiveDate::parse_from_str(b, "%Y-%m-%d");
    match (pa, pb) {
        (Ok(da), Ok(db)) => (db - da).num_days() == 7,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::prescription::SuggestionKind;

    fn row(id: u64, week: &str, created: i64, metric: &str, deleted: bool) -> Prescription {
        Prescription {
            id,
            week: week.to_string(),
            created_at: created,
            kind: SuggestionKind::Calibration,
            target_metric: Some(metric.to_string()),
            target_value: 0.5,
            text: "use 25-minute blocks".to_string(),
            sample_n: 7,
            deleted,
        }
    }

    fn sugg(metric: Option<&'static str>) -> crate::journal::prescription::Suggestion {
        crate::journal::prescription::Suggestion {
            kind: SuggestionKind::Calibration,
            target_metric: metric,
            text: "use 25-minute blocks".to_string(),
            sample_n: 7,
        }
    }

    #[test]
    fn test_record_prescriptions_empty_suggestions_writes_nothing() {
        // The required acceptance test: a week with zero suggestions produces
        // zero rows and does not increment the id counter.
        let (rows, next) = record(&[], 42, "2026-10-31", 1000, &[], |_| None);
        assert!(rows.is_empty());
        assert_eq!(next, 42, "counter must not move on an empty record");
    }

    #[test]
    fn record_assigns_ids_and_skips_untargeted_suggestions() {
        let suggestions = vec![sugg(Some("kept_rate")), sugg(None), sugg(Some("unjudged"))];
        let (rows, next) = record(&[], 0, "2026-10-31", 1000, &suggestions, |m| {
            if m == "kept_rate" {
                Some(0.41)
            } else {
                None
            }
        });
        assert_eq!(rows.len(), 2, "untargeted suggestion is skipped");
        assert_eq!(rows[0].id, 1);
        assert_eq!(rows[1].id, 2);
        assert_eq!(next, 2);
        assert_eq!(rows[0].target_metric.as_deref(), Some("kept_rate"));
        assert_eq!(rows[1].target_metric.as_deref(), Some("unjudged"));
    }

    #[test]
    fn record_appends_to_existing_with_next_id_offset() {
        let existing = vec![row(1, "2026-10-24", 100, "kept_rate", false)];
        let suggestions = vec![sugg(Some("kept_rate"))];
        let (rows, next) = record(&existing, 1, "2026-10-31", 1000, &suggestions, |_| None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].id, 2, "id continues from the counter");
        assert_eq!(next, 2);
    }

    #[test]
    fn merge_newest_created_at_wins() {
        let a = vec![row(1, "2026-10-31", 100, "kept_rate", false)];
        let b = vec![row(1, "2026-10-31", 200, "kept_rate", false)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].created_at, 200);
    }

    #[test]
    fn merge_tombstone_beats_live_copy() {
        let a = vec![row(1, "2026-10-31", 200, "kept_rate", true)];
        let b = vec![row(1, "2026-10-31", 100, "kept_rate", false)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].deleted, "tombstone wins");
    }

    #[test]
    fn merge_union_of_disjoint_ids() {
        let a = vec![row(1, "2026-10-31", 100, "kept_rate", false)];
        let b = vec![row(2, "2026-11-07", 200, "unjudged", false)];
        let merged = merge(&a, &b);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn latest_for_metric_ignores_tombstones() {
        let rows = vec![
            row(1, "2026-10-24", 100, "kept_rate", false),
            row(2, "2026-10-31", 200, "kept_rate", false),
            row(3, "2026-11-07", 300, "kept_rate", true),
        ];
        let latest = latest_for_metric(&rows, "kept_rate").unwrap();
        assert_eq!(latest.id, 2, "tombstone is not the latest live row");
    }

    #[test]
    fn repeated_consecutive_weeks_detects_stubborn_advice() {
        // Same advice for 3 consecutive weeks → stubborn.
        let rows = vec![
            row(1, "2026-10-17", 100, "kept_rate", false),
            row(2, "2026-10-24", 200, "kept_rate", false),
            row(3, "2026-10-31", 300, "kept_rate", false),
        ];
        assert!(repeated_consecutive_weeks(&rows, "kept_rate", 3));
        assert!(!repeated_consecutive_weeks(&rows, "kept_rate", 4));
    }

    #[test]
    fn repeated_consecutive_weeks_breaks_on_gap_or_other_metric() {
        // Gap between weeks (not consecutive).
        let gapped = vec![
            row(1, "2026-10-17", 100, "kept_rate", false),
            row(2, "2026-11-07", 200, "kept_rate", false),
            row(3, "2026-11-14", 300, "kept_rate", false),
        ];
        assert!(!repeated_consecutive_weeks(&gapped, "kept_rate", 3));

        // Different metric in between.
        let mixed = vec![
            row(1, "2026-10-17", 100, "kept_rate", false),
            row(2, "2026-10-24", 200, "unjudged", false),
            row(3, "2026-10-31", 300, "kept_rate", false),
        ];
        assert!(!repeated_consecutive_weeks(&mixed, "kept_rate", 3));
    }
}
