//! Bounce rate: how fast does the user return after a bad day?
//!
//! Streaks and kept-rate describe how often blocks are kept, but neither tells
//! you how quickly the user *recovers* after a day that went badly. This
//! module measures that recovery — the bounce.
//!
//! **What is a bad day?** A local day with at least one session judged
//! `Broken` or `Short` and no session judged `Kept`. A day that contains a
//! kept block is not a bad day even if a block was broken — it went partly
//! right, and measuring bounce from it would demand a recovery from a day that
//! already recovered.
//!
//! **What is a good day?** A local day with at least one session judged
//! `Kept`.
//!
//! **Bounce** is the number of local days between a bad day and the next good
//! day after it. It is the recovery time: how long it takes to land a kept
//! block again after a day of broken ones.
//!
//! The report is the **median** and **75th-percentile** bounce over the
//! trailing [`WINDOW_DAYS`] days, labelled with a three-band verdict:
//!
//! * **Resilient** — median ≤ 2 days. The user is back by the next day or
//!   two.
//! * **Fragile** — median ≤ 5 days. Recovery takes a few days.
//! * **At risk** — median > 5 days. Recovery is slow; broken streaks are
//!   becoming terminal.
//!
//! A bad day with no later good day in the record cannot have its bounce
//! measured, so it is excluded rather than guessed. An empty record, or one
//! with no measurable bounce, returns `None` — no fabricated precision.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use crate::journal::Verdict;
use crate::journal::VerdictKind;
use crate::state::store::SessionRecord;
use chrono::NaiveDate;

/// The trailing window over which bad days are observed, in local days.
pub const WINDOW_DAYS: i64 = 56;

/// The three recovery-speed bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceBand {
    /// Median bounce ≤ 2 days.
    Resilient,
    /// Median bounce ≤ 5 days.
    Fragile,
    /// Median bounce > 5 days.
    AtRisk,
}

/// Bounce statistics over the trailing window.
#[derive(Clone, Debug)]
pub struct BounceStats {
    /// The measured bounce lengths, in days, one per measured bad day.
    pub bounces: Vec<u32>,
    /// The median bounce in days.
    pub median: Option<u32>,
    /// The 75th-percentile bounce in days.
    pub p75: Option<u32>,
    /// The recovery-speed band, derived from the median.
    pub band: Option<BounceBand>,
}

impl BounceStats {
    /// The band label for a render, in the requested locale.
    pub fn band_label(&self, locale_fa: bool) -> Option<String> {
        self.band.map(|b| match (b, locale_fa) {
            (BounceBand::Resilient, true) => "تاب‌آور".to_string(),
            (BounceBand::Resilient, false) => "resilient".to_string(),
            (BounceBand::Fragile, true) => "شکننده".to_string(),
            (BounceBand::Fragile, false) => "fragile".to_string(),
            (BounceBand::AtRisk, true) => "در معرض خطر".to_string(),
            (BounceBand::AtRisk, false) => "at risk".to_string(),
        })
    }
}

/// The local date of a timestamp under a UTC offset.
fn local_date(ts: i64, tz_offset_secs: i32) -> Option<NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)?;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&offset).date_naive())
}

/// The nearest-rank index (0-based) for the p-th percentile of `n` items.
fn percentile_index(p: u32, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    // nearest-rank: rank = ceil(p/100 * n), 1-indexed
    let rank = ((p as f64 / 100.0) * n as f64).ceil().max(1.0) as usize;
    Some((rank - 1).min(n - 1))
}

/// Compute bounce statistics over the trailing window ending `today`.
///
/// Pure: existing rows in, a value out, no I/O.
pub fn bounce_stats(
    sessions: &[SessionRecord],
    verdicts: &[Verdict],
    today: NaiveDate,
    tz_offset_secs: i32,
) -> Option<BounceStats> {
    // Live verdicts keyed by session completion time.
    let verdict_for: std::collections::HashMap<i64, VerdictKind> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| (v.session_at, v.kind))
        .collect();

    // Bucket every session into its local day, classifying the day.
    // bad = has Broken/Short and no Kept; good = has Kept.
    let mut days: std::collections::BTreeMap<NaiveDate, (bool, bool)> =
        std::collections::BTreeMap::new();
    for s in sessions {
        let Some(kind) = verdict_for.get(&s.at).copied() else {
            continue; // an unjudged session says nothing about good/bad
        };
        let Some(date) = local_date(s.at, tz_offset_secs) else {
            continue;
        };
        let e = days.entry(date).or_default();
        match kind {
            VerdictKind::Kept => e.1 = true,
            VerdictKind::Broken | VerdictKind::Short => e.0 = true,
            _ => {}
        }
    }

    // The window: trailing WINDOW_DAYS days ending today.
    let window_start = today - chrono::Duration::days(WINDOW_DAYS - 1);

    let mut bad_days: Vec<NaiveDate> = days
        .iter()
        .filter(|(date, (bad, good))| *bad && !*good && **date >= window_start && **date <= today)
        .map(|(date, _)| *date)
        .collect();
    let good_days: Vec<NaiveDate> = days
        .iter()
        .filter(|(_, (_, good))| *good)
        .map(|(date, _)| *date)
        .collect();

    bad_days.sort();
    let good_days = good_days;

    let mut bounces: Vec<u32> = Vec::new();
    for bad in bad_days {
        // Next good day strictly after the bad day.
        if let Some(&next_good) = good_days.iter().find(|g| **g > bad) {
            let bounce = (next_good - bad).num_days();
            if bounce >= 0 {
                bounces.push(bounce as u32);
            }
        }
    }

    if bounces.is_empty() {
        return None;
    }

    bounces.sort_unstable();
    let n = bounces.len();
    let median = percentile_index(50, n).map(|i| bounces[i]);
    let p75 = percentile_index(75, n).map(|i| bounces[i]);
    let band = median.map(|m| {
        if m <= 2 {
            BounceBand::Resilient
        } else if m <= 5 {
            BounceBand::Fragile
        } else {
            BounceBand::AtRisk
        }
    });

    Some(BounceStats {
        bounces,
        median,
        p75,
        band,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TZ: i32 = 12600; // Iran UTC+3:30

    fn verdict(session_at: i64, kind: VerdictKind) -> Verdict {
        Verdict {
            session_at,
            kind,
            declared_at: session_at,
            deleted: false,
        }
    }

    fn sess(at: i64) -> SessionRecord {
        SessionRecord { at, minutes: 25 }
    }

    /// A single session on a given local day (00:30 local).
    fn at(day: NaiveDate) -> i64 {
        use chrono::TimeZone;
        let offset = chrono::FixedOffset::east_opt(TZ).unwrap();
        offset
            .from_local_datetime(&chrono::NaiveDateTime::new(
                day,
                chrono::NaiveTime::from_hms_opt(0, 30, 0).unwrap(),
            ))
            .single()
            .unwrap()
            .timestamp()
    }

    /// An empty record — no bad days, no recovery — returns None.
    #[test]
    fn test_bounce_empty_record_returns_none() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        assert!(bounce_stats(&[], &[], today, TZ).is_none());
    }

    /// A bad day followed by a good day the next day bounces in 1 day.
    #[test]
    fn test_bounce_single_bad_day_recovered_next_day() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let bad_day = today - chrono::Duration::days(4);
        let good_day = bad_day + chrono::Duration::days(1);

        let sessions = vec![sess(at(bad_day)), sess(at(good_day))];
        let verdicts = vec![
            verdict(at(bad_day), VerdictKind::Broken),
            verdict(at(good_day), VerdictKind::Kept),
        ];

        let stats = bounce_stats(&sessions, &verdicts, today, TZ).unwrap();
        assert_eq!(stats.bounces, vec![1], "one bounce of one day");
        assert_eq!(stats.median, Some(1));
        assert_eq!(stats.p75, Some(1));
        assert_eq!(stats.band, Some(BounceBand::Resilient));
    }

    /// A bad day with no later good day cannot have its bounce measured and is
    /// excluded rather than guessed.
    #[test]
    fn test_bounce_bad_day_without_recovery_is_excluded() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let bad_day = today - chrono::Duration::days(1);

        let sessions = vec![sess(at(bad_day))];
        let verdicts = vec![verdict(at(bad_day), VerdictKind::Short)];

        assert!(
            bounce_stats(&sessions, &verdicts, today, TZ).is_none(),
            "no recovery to measure → None (no fabricated bounce)"
        );
    }

    /// Median and 75th percentile over several bounces, and the band label.
    #[test]
    fn test_bounce_median_and_p75_over_multiple_bounces() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        // Bounces of 1, 2, 4, 6 days.
        let mut sessions = Vec::new();
        let mut verdicts = Vec::new();
        let mut day = today - chrono::Duration::days(40);
        let bounces = [1u32, 2, 4, 6];
        for &b in &bounces {
            // bad day
            sessions.push(sess(at(day)));
            verdicts.push(verdict(at(day), VerdictKind::Broken));
            // good day b days later
            let good = day + chrono::Duration::days(b as i64);
            sessions.push(sess(at(good)));
            verdicts.push(verdict(at(good), VerdictKind::Kept));
            day = good + chrono::Duration::days(2);
        }

        let stats = bounce_stats(&sessions, &verdicts, today, TZ).unwrap();
        // sorted: 1,2,4,6 → median (50th) = 2, p75 = 4 (nearest rank)
        assert_eq!(stats.median, Some(2));
        assert_eq!(stats.p75, Some(4));
        assert_eq!(stats.band, Some(BounceBand::Resilient));
        assert_eq!(stats.band_label(false).as_deref(), Some("resilient"));
        assert_eq!(stats.band_label(true).as_deref(), Some("تاب‌آور"));
    }

    /// Slow recovery (>5 days median) is at risk.
    #[test]
    fn test_bounce_at_risk_band() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut sessions = Vec::new();
        let mut verdicts = Vec::new();
        let mut day = today - chrono::Duration::days(50);
        let bounces = [6u32, 7, 8, 9];
        for &b in &bounces {
            sessions.push(sess(at(day)));
            verdicts.push(verdict(at(day), VerdictKind::Short));
            let good = day + chrono::Duration::days(b as i64);
            sessions.push(sess(at(good)));
            verdicts.push(verdict(at(good), VerdictKind::Kept));
            day = good + chrono::Duration::days(1);
        }

        let stats = bounce_stats(&sessions, &verdicts, today, TZ).unwrap();
        assert_eq!(stats.median, Some(7));
        assert_eq!(stats.band, Some(BounceBand::AtRisk));
    }
}
