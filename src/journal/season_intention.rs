//! Season intention: what this season is for, said at its start.
//!
//! The season summary looks back; the season close settles a season that ended.
//! Neither asks at the *start* what the season is for. This module is the
//! opening note: a short intention written in the season's first days, the
//! season-shaped sibling of the daily decree.
//!
//! The prompt is due only under two conditions, both enforced here:
//!
//! * **First 7 days** — an intention is an opening act; past the first week
//!   it is late, and the coach should not nag.
//! * **No live intention yet** — [`is_due`] returns false once a live entry
//!   exists for the season. A tombstoned (withdrawn) entry does *not*
//!   suppress the prompt: the user may rewrite their intention.
//!
//! [`create`] rejects a blank body with [`SeasonIntentionError::Empty`],
//! mirroring the season close, so a blank prompt cannot write a record.
//!
//! Everything here is pure: slices and a counter in, owned data out, no I/O,
//! no clock reads beyond what is passed in.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// Days from the season's start during which an intention is due.
pub const DUE_WINDOW_DAYS: i64 = 7;

/// One season-intention record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeasonIntention {
    /// Stable identity from the store's counter.
    pub id: u64,
    /// The Jalali year (e.g. 1405).
    pub jalali_year: i32,
    /// The season (0 = spring … 3 = winter).
    pub season: usize,
    /// The intention, non-empty.
    pub body: String,
    /// Unix seconds when this row was written — the merge stamp.
    pub created_at: i64,
    /// Tombstone. Set → this row should be treated as gone everywhere.
    #[serde(default)]
    pub deleted: bool,
}

/// Why a season intention could not be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeasonIntentionError {
    /// The body was blank — a blank prompt cannot write a record.
    Empty,
    /// The intention is past the first-week window and the due guard was on.
    NotDueYet,
}

/// True when the prompt is due: inside the first `DUE_WINDOW_DAYS` days of the
/// season and no live intention exists for that (year, season).
pub fn is_due(
    existing: &[SeasonIntention],
    jalali_year: i32,
    season: usize,
    days_since_season_start: i64,
) -> bool {
    days_since_season_start <= DUE_WINDOW_DAYS
        && latest_live_for_season(existing, jalali_year, season).is_none()
}

/// The latest live intention for a season, if any.
pub fn latest_live_for_season<'a>(
    rows: &'a [SeasonIntention],
    jalali_year: i32,
    season: usize,
) -> Option<&'a SeasonIntention> {
    rows.iter()
        .filter(|i| !i.deleted && i.jalali_year == jalali_year && i.season == season)
        .max_by_key(|i| i.created_at)
}

/// Create a season-intention record, drawing its id from the store's counter.
///
/// Returns `Err(SeasonIntentionError::Empty)` when `body` is blank — nothing is
/// written. Returns `Err(SeasonIntentionError::NotDueYet)` when
/// `enforce_due` is set and `is_due` is false.
pub fn create(
    existing: &[SeasonIntention],
    next_id: u64,
    jalali_year: i32,
    season: usize,
    body: &str,
    created_at: i64,
    days_since_season_start: i64,
    enforce_due: bool,
) -> Result<(SeasonIntention, u64), SeasonIntentionError> {
    if body.trim().is_empty() {
        return Err(SeasonIntentionError::Empty);
    }
    if enforce_due && !is_due(existing, jalali_year, season, days_since_season_start) {
        return Err(SeasonIntentionError::NotDueYet);
    }
    Ok((
        SeasonIntention {
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

/// Merge two intention lists: keyed by id, newest `created_at` wins, a tombstone
/// beats a live copy, ties broken deterministically by id.
pub fn merge(a: &[SeasonIntention], b: &[SeasonIntention]) -> Vec<SeasonIntention> {
    let mut map: std::collections::BTreeMap<u64, SeasonIntention> =
        std::collections::BTreeMap::new();
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
    let mut out: Vec<SeasonIntention> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.jalali_year
            .cmp(&y.jalali_year)
            .then_with(|| x.season.cmp(&y.season))
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Is `candidate` the copy of one intention row to keep?
fn keep(candidate: &SeasonIntention, existing: &SeasonIntention) -> bool {
    if candidate.created_at != existing.created_at {
        return candidate.created_at > existing.created_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    candidate.id > existing.id
}

/// Write the current season's intention file, once per season, when due.
///
/// Follows the same convention as the season summary: a fixed file per season,
/// written only when it doesn't exist. The prompt is only due in the season's
/// first week and only when no live intention exists. Returns the path written,
/// or `None` when nothing qualified (not due, or already written).
pub fn write_season_intention_if_new(
    existing: &[SeasonIntention],
    today: NaiveDate,
) -> Option<std::path::PathBuf> {
    let season = crate::journal::season_summary::season_of(today);
    let days_since_start = (today - season.range.start).num_days().max(0);
    if !is_due(
        existing,
        season.jalali_year,
        season.season,
        days_since_start,
    ) {
        return None;
    }
    let dir = crate::state::store::Store::data_dir().join("seasons");
    let filename = format!("intention-{}-{}.md", season.jalali_year, season.season + 1);
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let title = format!(
        "# Season intention — year {}, season {}\n\n",
        season.jalali_year,
        season.season + 1
    );
    std::fs::create_dir_all(&dir).ok()?;
    crate::state::atomic_file::atomic_write(&path, title.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intention(id: u64, year: i32, season: usize, at: i64, deleted: bool) -> SeasonIntention {
        SeasonIntention {
            id,
            jalali_year: year,
            season,
            body: "intention".to_string(),
            created_at: at,
            deleted,
        }
    }

    #[test]
    fn is_due_true_in_first_seven_days_without_live_entry() {
        // Empty existing → due at day 5.
        assert!(is_due(&[], 1405, 0, 5));
        // Day 8 → past the window → not due.
        assert!(!is_due(&[], 1405, 0, 8));
        // Day 0 → due.
        assert!(is_due(&[], 1405, 0, 0));
        // Day exactly 7 → due (inclusive).
        assert!(is_due(&[], 1405, 0, 7));
    }

    #[test]
    fn is_due_false_when_live_entry_exists() {
        let rows = [intention(1, 1405, 0, 100, false)];
        assert!(
            !is_due(&rows, 1405, 0, 3),
            "a live entry suppresses the prompt"
        );
    }

    #[test]
    fn is_due_true_when_entry_tombstoned() {
        let rows = [intention(1, 1405, 0, 100, true)];
        assert!(
            is_due(&rows, 1405, 0, 3),
            "a tombstone does not suppress the prompt"
        );
    }

    #[test]
    fn create_draws_counter_forward_and_trims() {
        let (c, next) = create(&[], 3, 1405, 0, "  a season of deep work  ", 100, 2, true).unwrap();
        assert_eq!(c.id, 4);
        assert_eq!(c.body, "a season of deep work");
        assert_eq!(next, 4);
    }

    #[test]
    fn create_rejects_empty_body() {
        assert_eq!(
            create(&[], 0, 1405, 0, "", 100, 2, true),
            Err(SeasonIntentionError::Empty)
        );
        assert_eq!(
            create(&[], 0, 1405, 0, "   \n\t ", 100, 2, true),
            Err(SeasonIntentionError::Empty)
        );
    }

    #[test]
    fn create_rejects_outside_window_when_guard_enabled() {
        assert_eq!(
            create(&[], 0, 1405, 0, "late intention", 100, 9, true),
            Err(SeasonIntentionError::NotDueYet)
        );
    }

    #[test]
    fn create_allows_outside_window_when_guard_disabled() {
        assert!(create(&[], 0, 1405, 0, "forced intention", 100, 9, false).is_ok());
    }

    #[test]
    fn create_rejects_when_live_entry_exists() {
        let rows = [intention(1, 1405, 0, 100, false)];
        assert_eq!(
            create(&rows, 1, 1405, 0, "second intention", 200, 3, true),
            Err(SeasonIntentionError::NotDueYet)
        );
    }

    #[test]
    fn merge_keeps_newest_created_at() {
        let a = [intention(1, 1405, 0, 100, false)];
        let b = [intention(1, 1405, 0, 200, false)];
        let out = merge(&a, &b);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].created_at, 200);
    }

    #[test]
    fn merge_tombstone_beats_live() {
        let dead = intention(1, 1405, 0, 200, true);
        let live = intention(1, 1405, 0, 200, false);
        let out = merge(&[live], &[dead]);
        assert_eq!(out.len(), 1);
        assert!(out[0].deleted, "a tombstone always beats a live copy");
    }

    #[test]
    fn latest_live_prefers_latest_live() {
        let rows = [
            intention(1, 1405, 0, 100, false),
            intention(2, 1405, 0, 300, false),
            intention(3, 1405, 1, 200, false),
        ];
        assert_eq!(latest_live_for_season(&rows, 1405, 0).unwrap().id, 2);
        assert!(latest_live_for_season(&rows, 1405, 2).is_none());
    }

    #[test]
    fn deleted_intention_is_not_latest() {
        let rows = [
            intention(1, 1405, 0, 100, false),
            intention(2, 1405, 0, 300, true),
        ];
        assert_eq!(latest_live_for_season(&rows, 1405, 0).unwrap().id, 1);
    }
}
