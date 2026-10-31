//! Plan realism ceiling: the largest rota size the user actually finishes.
//!
//! [`DayTally::fulfillment`] tells how a single day went — but it cannot say
//! what *size* of day is sustainable. The realism ceiling answers that, across
//! the record: for each daily plan size (number of blocks planned for a day),
//! how often did that size reach 80% fulfillment? The ceiling is the largest
//! size whose kept-rate clears the bar. It turns "plan less" from a shrug into
//! a number the digest can print.
//!
//! Everything here is pure: slices in, owned data out, no clock reads beyond
//! the `today` string passed in. No new persisted table — this reads the
//! rota entries that already exist and renders into the briefing/digest
//! Markdown only.

use chrono::NaiveDate;

use crate::state::outcome::DayTally;

/// A day counts as *kept* when at least this share of its planned blocks landed.
pub const FULFILLMENT_FLOOR: f64 = 0.80;

/// A plan size is realistic when at least this share of its days were kept.
pub const KEEP_RATE_FLOOR: f32 = 0.50;

/// At least this many days are needed before a plan size is judged.
pub const MIN_DAYS_PER_SIZE: u32 = 3;

/// Default minimum history (in distinct weeks) before any ceiling is offered.
pub const DEFAULT_MIN_WEEKS: u32 = 3;

/// The verdict for one day: which plan size it had and whether it was kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RealismInfo {
    /// The largest daily plan size the user reliably finishes.
    pub ceiling: u32,
    /// The size the user is currently planning (today's rota, or the week's
    /// typical size). Compared against `ceiling` in the label.
    pub current_plan_size: u32,
}

/// Collect the [`DayTally`] of every completed past day that had plans.
///
/// `today` is excluded — a running day has not earned its verdict yet. Days are
/// returned sorted, one per date, with tombstoned plans already counted out by
/// [`tally`](crate::state::outcome::tally).
pub fn historical_day_tallies(
    rota: &[crate::state::rota::RotaEntry],
    today: &str,
) -> Vec<(NaiveDate, DayTally)> {
    let mut days: Vec<String> = rota
        .iter()
        .filter(|e| e.is_live() && e.day.as_str() < today)
        .map(|e| e.day.clone())
        .collect();
    days.sort();
    days.dedup();
    days.into_iter()
        .filter_map(|d| {
            NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok().map(|date| {
                let tally = crate::state::outcome::tally(rota, &d, today);
                (date, tally)
            })
        })
        .collect()
}

/// The largest daily plan size the user actually finishes.
///
/// For each plan size present in the record, days are grouped by that size. A
/// size is *realistic* when at least [`KEEP_RATE_FLOOR`] of its days reached
/// [`FULFILLMENT_FLOOR`] fulfillment, and the size was sampled at least
/// [`MIN_DAYS_PER_SIZE`] times. The ceiling is the largest such size.
///
/// Returns `None` when the record is too shallow (fewer than `min_weeks`
/// distinct weeks with plans) or no size clears the bar.
pub fn realism_ceiling(days: &[(NaiveDate, DayTally)], min_weeks: u32) -> Option<u32> {
    // History floor: require enough distinct weeks of lived-in days.
    let mut weeks: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (d, t) in days {
        if t.planned > 0 {
            weeks.insert(crate::stats::report::week_index(*d));
        }
    }
    if (weeks.len() as u32) < min_weeks {
        return None;
    }

    // Bucket days by their plan size: (size) -> (days, kept).
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for (_, t) in days {
        if t.planned == 0 {
            continue;
        }
        let kept = t
            .fulfillment()
            .map(|f| f >= FULFILLMENT_FLOOR)
            .unwrap_or(false);
        let entry = buckets.entry(t.planned).or_insert((0, 0));
        entry.0 += 1;
        if kept {
            entry.1 += 1;
        }
    }

    let mut ceiling: Option<u32> = None;
    for (size, (n, kept)) in buckets {
        if n < MIN_DAYS_PER_SIZE {
            continue; // too few samples to judge this size
        }
        let rate = kept as f32 / n as f32;
        if rate >= KEEP_RATE_FLOOR {
            ceiling = Some(size);
        }
    }
    ceiling
}

/// A human sentence turning the ceiling into advice.
pub fn realism_label(ceiling: Option<u32>, current_plan_size: u32, locale_fa: bool) -> String {
    match ceiling {
        None => {
            if locale_fa {
                "هنوز تاریخچهٔ کافی برای دانستن ظرفیت واقعی برنامه‌ات نیست.".to_string()
            } else {
                "Not enough history yet to know your realistic plan size.".to_string()
            }
        }
        Some(c) => {
            let head = if locale_fa {
                format!(
                    "برنامهٔ واقع‌بینانه: {} بلوک در روز",
                    crate::calendar::jalali::fa_num(c as i64)
                )
            } else {
                format!("Your realistic plan ceiling: {c} blocks a day.")
            };
            if current_plan_size > c {
                if locale_fa {
                    format!(
                        "{} اما امروز {} بلوک برنامه‌ریزی شده‌ای — بیشتر از آنچه واقعاً تمام می‌کنی. کمتر بچین.",
                        head,
                        crate::calendar::jalali::fa_num(current_plan_size as i64)
                    )
                } else {
                    format!(
                        "{head} You've planned {current_plan_size} today — above it. Trim the list."
                    )
                }
            } else {
                head
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: &str, planned: u32, done: u32) -> (NaiveDate, DayTally) {
        let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        let t = DayTally {
            planned,
            done,
            ..Default::default()
        };
        (date, t)
    }

    #[test]
    fn empty_history_yields_no_ceiling() {
        assert_eq!(realism_ceiling(&[], 3), None);
    }

    #[test]
    fn history_below_min_weeks_yields_none() {
        let days = vec![day("2026-10-03", 3, 3)];
        assert_eq!(realism_ceiling(&days, 3), None);
    }

    #[test]
    fn largest_kept_size_is_the_ceiling() {
        // Three distinct weeks. Sizes 2, 3, 4 all kept ≥ 50% of the time.
        let days = vec![
            day("2026-10-03", 2, 2),
            day("2026-10-04", 2, 2),
            day("2026-10-05", 2, 1), // 2/3 kept
            day("2026-10-10", 3, 3),
            day("2026-10-11", 3, 2),
            day("2026-10-12", 3, 3), // 3/3 kept
            day("2026-10-17", 4, 4),
            day("2026-10-18", 4, 4),
            day("2026-10-19", 4, 4), // 3/3 kept
        ];
        assert_eq!(realism_ceiling(&days, 3), Some(4));
    }

    #[test]
    fn ceiling_stops_at_the_first_unkept_size() {
        // Size 4 is sampled enough but kept only 1/3 days → below the bar.
        let days = vec![
            day("2026-10-03", 3, 3),
            day("2026-10-04", 3, 2),
            day("2026-10-05", 3, 3), // 3/3 kept
            day("2026-10-10", 4, 1),
            day("2026-10-11", 4, 0),
            day("2026-10-12", 4, 2), // kept 1/3 = 0.33 → below 0.50
        ];
        assert_eq!(realism_ceiling(&days, 3), Some(3));
    }

    #[test]
    fn undersampled_sizes_are_not_judged() {
        let days = vec![
            day("2026-10-03", 3, 3),
            day("2026-10-04", 3, 3),
            day("2026-10-05", 3, 3), // size 3 kept 3/3
            day("2026-10-10", 9, 9), // size 9 has a single sample → ignored
        ];
        assert_eq!(realism_ceiling(&days, 3), Some(3));
    }

    #[test]
    fn label_english_plain_when_within_ceiling() {
        assert_eq!(
            realism_label(Some(3), 3, false),
            "Your realistic plan ceiling: 3 blocks a day."
        );
    }

    #[test]
    fn label_warns_when_current_exceeds_ceiling() {
        let s = realism_label(Some(3), 5, false);
        assert!(s.contains("5"), "should mention the over-planned count");
        assert!(s.contains("above it"));
    }

    #[test]
    fn label_persian_warns_when_current_exceeds() {
        let s = realism_label(Some(3), 5, true);
        assert!(s.contains('۵'), "should use Persian digits for the warning");
    }

    #[test]
    fn label_none_message_says_history_is_shallow() {
        assert!(realism_label(None, 3, false).contains("enough history"));
    }
}
