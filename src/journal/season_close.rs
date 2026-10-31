//! Season close: the definitive closing note on a season.
//!
//! The season summary is a passive read-back; it does not let the user *settle*
//! a season — to look back, say what it was, and mark it done. A season close is
//! that act: one short reflection, written when the season is ending, kept as a
//! permanent record alongside the summary.
//!
//! Two guards keep the record honest:
//!
//! * **Empty body** — a close must say something. A blank prompt is not a
//!   closing reflection; it is an accidental keystroke, and writing a tombstone
//!   for it would silently eat the season's record. [`create`] rejects an empty
//!   body with [`SeasonCloseError::Empty`] and leaves the input untouched.
//! * **Due guard** — a close is meant for the end of a season, not its middle.
//!   [`is_due`] is only true in the season's final 14 days. When the guard is
//!   enabled (the normal path), [`create`] refuses a premature close with
//!   [`SeasonCloseError::NotDueYet`]. A caller that must force a close
//!   mid-season (e.g. archiving a half-finished season) can disable the guard.
//!
//! The record is keyed by a stable `id`, carries `created_at` as the merge
//! stamp, and uses a `deleted` tombstone — a tombstone always beats a live copy
//! and is never physically purged during merge, so a withdrawn close cannot
//! silently resurrect on another device.
//!
//! Everything here is pure: slices and a counter in, owned data out, no I/O,
//! no clock reads beyond what is passed in.

use crate::stats::report::Range;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// One season-close record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeasonClose {
    /// Stable identity from the store's counter.
    pub id: u64,
    /// The Jalali year (e.g. 1405).
    pub jalali_year: i32,
    /// The season (0 = spring … 3 = winter).
    pub season: usize,
    /// The closing reflection, non-empty.
    pub body: String,
    /// Unix seconds when this row was written — the merge stamp.
    pub created_at: i64,
    /// Tombstone. Set → this row should be treated as gone everywhere.
    #[serde(default)]
    pub deleted: bool,
}

/// Why a season close could not be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeasonCloseError {
    /// The body was blank — a blank prompt cannot write a tombstone.
    Empty,
    /// The season is not in its final 14 days and the due guard was enabled.
    NotDueYet,
}

/// Days before the season's end that a close is considered due.
pub const DUE_WINDOW_DAYS: i64 = 14;

/// True when `today` falls in the season's final `DUE_WINDOW_DAYS`.
///
/// The final 14 days are inclusive of the season's last day, so the window is
/// `[range.end - 13, range.end]`.
pub fn is_due(today: NaiveDate, range: Range) -> bool {
    let window_start = range.end - chrono::Duration::days(DUE_WINDOW_DAYS - 1);
    today >= window_start && today <= range.end
}

/// Create a season-close record, drawing its id from the store's counter.
///
/// Returns `Err(SeasonCloseError::Empty)` when `body` is blank — nothing is
/// written, and the input slices are left untouched. Returns
/// `Err(SeasonCloseError::NotDueYet)` when `enforce_due` is set and `is_due`
/// is false.
pub fn create(
    next_id: u64,
    jalali_year: i32,
    season: usize,
    body: &str,
    created_at: i64,
    is_due: bool,
    enforce_due: bool,
) -> Result<(SeasonClose, u64), SeasonCloseError> {
    if body.trim().is_empty() {
        return Err(SeasonCloseError::Empty);
    }
    if enforce_due && !is_due {
        return Err(SeasonCloseError::NotDueYet);
    }
    Ok((
        SeasonClose {
            id: next_id + 1,
            jalali_year,
            season,
            body: body.trim().to_string(),
            created_at,
            deleted: false,
        },
        next_id + 1,
    ))
}

/// Merge two close lists: keyed by id, newest `created_at` wins, a tombstone
/// beats a live copy, ties broken deterministically by id.
pub fn merge(a: &[SeasonClose], b: &[SeasonClose]) -> Vec<SeasonClose> {
    let mut map: std::collections::BTreeMap<u64, SeasonClose> = std::collections::BTreeMap::new();
    for c in a.iter().chain(b.iter()) {
        match map.get(&c.id) {
            None => {
                map.insert(c.id, c.clone());
            }
            Some(existing) => {
                if keep(c, existing) {
                    map.insert(c.id, c.clone());
                }
            }
        }
    }
    let mut out: Vec<SeasonClose> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.jalali_year
            .cmp(&y.jalali_year)
            .then_with(|| x.season.cmp(&y.season))
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Is `candidate` the copy of one close row to keep?
fn keep(candidate: &SeasonClose, existing: &SeasonClose) -> bool {
    if candidate.created_at != existing.created_at {
        return candidate.created_at > existing.created_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    candidate.id > existing.id
}

/// The latest live close for a given season, if any.
pub fn latest_for_season<'a>(
    rows: &'a [SeasonClose],
    jalali_year: i32,
    season: usize,
) -> Option<&'a SeasonClose> {
    rows.iter()
        .filter(|c| !c.deleted && c.jalali_year == jalali_year && c.season == season)
        .max_by_key(|c| c.created_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_end(end: NaiveDate) -> Range {
        Range {
            start: end - chrono::Duration::days(89),
            end,
        }
    }

    #[test]
    fn create_draws_counter_forward() {
        let (c, next) = create(3, 1405, 0, "a season of deep work", 100, true, true).unwrap();
        assert_eq!(c.id, 4);
        assert_eq!(c.jalali_year, 1405);
        assert_eq!(c.season, 0);
        assert_eq!(c.body, "a season of deep work");
        assert_eq!(next, 4);
        assert!(!c.deleted);
    }

    #[test]
    fn create_trims_body() {
        let (c, _) = create(0, 1405, 1, "  settled.  ", 100, true, true).unwrap();
        assert_eq!(c.body, "settled.");
    }

    #[test]
    fn create_rejects_empty_body() {
        let before: Vec<SeasonClose> = vec![];
        let res = create(0, 1405, 0, "", 100, true, true);
        assert_eq!(res, Err(SeasonCloseError::Empty));
        // Input untouched — nothing was appended.
        assert_eq!(before.len(), 0);
    }

    #[test]
    fn create_rejects_whitespace_body() {
        assert_eq!(
            create(0, 1405, 0, "   \n\t ", 100, true, true),
            Err(SeasonCloseError::Empty)
        );
    }

    #[test]
    fn create_rejects_premature_when_due_guard_enabled() {
        // Today is well inside the season, not in the final 14 days.
        let res = create(0, 1405, 0, "too early to close", 100, false, true);
        assert_eq!(res, Err(SeasonCloseError::NotDueYet));
    }

    #[test]
    fn create_accepts_premature_when_guard_disabled() {
        let res = create(0, 1405, 0, "archiving early", 100, false, false);
        assert!(res.is_ok());
    }

    #[test]
    fn is_due_only_last_fourteen_days() {
        let end = NaiveDate::from_ymd_opt(2026, 6, 21).unwrap();
        let range = range_end(end);
        // 13 days before the end → due.
        let due = end - chrono::Duration::days(13);
        assert!(is_due(due, range));
        // The last day → due.
        assert!(is_due(end, range));
        // 20 days before → not due.
        let not_due = end - chrono::Duration::days(20);
        assert!(!is_due(not_due, range));
        // After the season → not due (a late close is out of window).
        let after = end + chrono::Duration::days(1);
        assert!(!is_due(after, range));
    }

    #[test]
    fn merge_keeps_newest_created_at() {
        let a = [SeasonClose {
            id: 1,
            jalali_year: 1405,
            season: 0,
            body: "first".to_string(),
            created_at: 100,
            deleted: false,
        }];
        let b = [SeasonClose {
            id: 1,
            jalali_year: 1405,
            season: 0,
            body: "second".to_string(),
            created_at: 200,
            deleted: false,
        }];
        let out = merge(&a, &b);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].body, "second");
    }

    #[test]
    fn merge_tombstone_beats_live() {
        let dead = SeasonClose {
            id: 1,
            jalali_year: 1405,
            season: 0,
            body: "withdrawn".to_string(),
            created_at: 200,
            deleted: true,
        };
        let live = SeasonClose {
            id: 1,
            jalali_year: 1405,
            season: 0,
            body: "kept".to_string(),
            created_at: 200,
            deleted: false,
        };
        let out = merge(&[live], &[dead]);
        assert_eq!(out.len(), 1);
        assert!(out[0].deleted, "a tombstone always beats a live copy");
    }

    #[test]
    fn latest_for_season_prefers_latest_live() {
        let rows = [
            SeasonClose {
                id: 1,
                jalali_year: 1405,
                season: 0,
                body: "a".to_string(),
                created_at: 100,
                deleted: false,
            },
            SeasonClose {
                id: 2,
                jalali_year: 1405,
                season: 0,
                body: "b".to_string(),
                created_at: 300,
                deleted: false,
            },
            SeasonClose {
                id: 3,
                jalali_year: 1405,
                season: 1,
                body: "c".to_string(),
                created_at: 200,
                deleted: false,
            },
        ];
        let latest = latest_for_season(&rows, 1405, 0).unwrap();
        assert_eq!(latest.body, "b");
        assert!(latest_for_season(&rows, 1405, 2).is_none());
    }

    #[test]
    fn deleted_close_is_not_latest() {
        let mut dead = SeasonClose {
            id: 2,
            jalali_year: 1405,
            season: 0,
            body: "b".to_string(),
            created_at: 300,
            deleted: false,
        };
        dead.deleted = true;
        let rows = [
            SeasonClose {
                id: 1,
                jalali_year: 1405,
                season: 0,
                body: "a".to_string(),
                created_at: 100,
                deleted: false,
            },
            dead,
        ];
        assert_eq!(latest_for_season(&rows, 1405, 0).unwrap().body, "a");
    }
}
