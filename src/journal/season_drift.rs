//! Season drift: which metric moved the most between this season and the last.
//!
//! The season summary faces one season. The *trend* between seasons is the
//! question the summary alone cannot answer: did kept-rate actually rise, or
//! did the season just contain more sessions? This module is the season-shaped
//! equivalent of `week_drift` — it compares the current season against the
//! previous one and names the single metric that moved the most.
//!
//! It reuses the season aggregation (`crate::journal::season_summary::aggregate`)
//! so the numbers match the summary's, and the same metric keys and direction
//! rules as `week_drift` so a "kept_rate" finding means the same thing on both
//! screens.
//!
//! Everything here is pure: slices and a date in, owned data out, no I/O, no
//! clock reads beyond what is passed in.

use crate::journal::season_summary::{aggregate, season_bounds, SeasonAgg};
use crate::journal::{Decree, Inscription, Verdict};
use crate::state::store::SessionRecord;
use chrono::NaiveDate;

/// The headline finding: whichever metric drifted the most between seasons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeasonDrift {
    /// The metric's display key ("sessions", "kept_rate", …).
    pub metric: &'static str,
    /// `current − previous` (raw, unnormalised).
    pub delta: f32,
    /// The previous season's value this delta is measured against.
    pub baseline: f32,
    /// Whether the drift was an improvement for that metric.
    pub better: bool,
}

/// A season's headline metrics, mirroring `WeekMetrics` at season granularity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeasonMetrics {
    /// The season this describes (the first day of its span).
    pub season_start: NaiveDate,
    /// Completed focus sessions in the season.
    pub sessions: u32,
    /// Total focus minutes in the season.
    pub focus_minutes: u64,
    /// Kept-or-extended share of rated blocks, 0..=1.
    pub kept_rate: Option<f32>,
    /// Fulfilment rate: landed / planned, 0..=1.
    pub fulfilment_rate: Option<f32>,
    /// Unjudged blocks in the season.
    pub unjudged: u32,
}

/// Direction of "better" for each metric. `None` = a shift has no inherent
/// better/worse.
fn better_direction(metric: &str) -> Option<bool> {
    match metric {
        "sessions" | "focus_minutes" | "kept_rate" | "fulfilment_rate" => Some(true),
        "unjudged" => Some(false),
        _ => None,
    }
}

/// Convert a season aggregate into headline metrics.
fn to_metrics(agg: &SeasonAgg) -> SeasonMetrics {
    let total = agg.tally.total();
    let kept_rate = if total > 0 {
        Some(agg.tally.met() as f32 / total as f32)
    } else {
        None
    };
    SeasonMetrics {
        season_start: agg.range.start,
        sessions: agg.sessions,
        focus_minutes: agg.focus_minutes,
        kept_rate,
        fulfilment_rate: (agg.planned > 0).then(|| agg.landed as f32 / agg.planned as f32),
        unjudged: agg.unjudged,
    }
}

/// The current season's metrics.
pub fn current_metrics(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    today: NaiveDate,
    tz_offset_secs: i32,
) -> SeasonMetrics {
    let season = crate::journal::season_summary::season_of(today);
    let agg = aggregate(
        season.range,
        sessions,
        decrees,
        inscriptions,
        verdicts,
        tz_offset_secs,
    );
    to_metrics(&agg)
}

/// The previous season's metrics (the season before the one containing `today`).
pub fn previous_metrics(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    today: NaiveDate,
    tz_offset_secs: i32,
) -> SeasonMetrics {
    let season = crate::journal::season_summary::season_of(today);
    let (prev_year, prev_season) = if season.season == 0 {
        (season.jalali_year - 1, 3)
    } else {
        (season.jalali_year, season.season - 1)
    };
    let range = season_bounds(prev_year, prev_season);
    let agg = aggregate(
        range,
        sessions,
        decrees,
        inscriptions,
        verdicts,
        tz_offset_secs,
    );
    to_metrics(&agg)
}

/// Compute the drift between the two seasons.
///
/// Returns the single metric whose `|current − previous|` is largest and that
/// has a measurable direction (both seasons have a value). Returns `None` when
/// no metric has both values, or when every drift is zero.
pub fn drift(current: &SeasonMetrics, previous: &SeasonMetrics) -> Option<SeasonDrift> {
    let val = |m: &SeasonMetrics, f: fn(&SeasonMetrics) -> Option<f32>| f(m);
    let num = |m: &SeasonMetrics, f: fn(&SeasonMetrics) -> f32| Some(f(m));

    let candidates: [(&str, Option<f32>, Option<f32>); 5] = [
        (
            "sessions",
            num(current, |m| m.sessions as f32),
            num(previous, |m| m.sessions as f32),
        ),
        (
            "focus_minutes",
            num(current, |m| m.focus_minutes as f32),
            num(previous, |m| m.focus_minutes as f32),
        ),
        (
            "kept_rate",
            val(current, |m| m.kept_rate),
            val(previous, |m| m.kept_rate),
        ),
        (
            "fulfilment_rate",
            val(current, |m| m.fulfilment_rate),
            val(previous, |m| m.fulfilment_rate),
        ),
        (
            "unjudged",
            num(current, |m| m.unjudged as f32),
            num(previous, |m| m.unjudged as f32),
        ),
    ];

    let mut best: Option<(f32, &str, f32, f32)> = None;
    for (metric, cur, base) in candidates {
        let (Some(c), Some(b)) = (cur, base) else {
            continue;
        };
        let delta = c - b;
        let mag = delta.abs();
        if mag < 1e-6 {
            continue; // nothing moved — not a finding
        }
        if best.is_none() || mag > best.unwrap().0 {
            best = Some((mag, metric, delta, b));
        }
    }

    let (_, metric, delta, base) = best?;
    Some(SeasonDrift {
        metric,
        delta,
        baseline: base,
        better: better_direction(metric).unwrap_or(false) == (delta >= 0.0),
    })
}

/// A report-ready headline label for the drift, in the requested locale.
pub fn headline_label(d: &SeasonDrift, locale_fa: bool) -> String {
    let fa = locale_fa;
    let metric_label = |metric: &str| -> String {
        match (fa, metric) {
            (true, "sessions") => "جلسه‌ها".to_string(),
            (true, "focus_minutes") => "دقیقهٔ تمرکز".to_string(),
            (true, "kept_rate") => "نرخ نگه‌داشتن".to_string(),
            (true, "fulfilment_rate") => "نرخ انجام‌شدن".to_string(),
            (true, "unjudged") => "بلوک‌های بدون داوری".to_string(),
            (false, "sessions") => "sessions".to_string(),
            (false, "focus_minutes") => "focus minutes".to_string(),
            (false, "kept_rate") => "kept rate".to_string(),
            (false, "fulfilment_rate") => "fulfilment rate".to_string(),
            (false, "unjudged") => "unjudged blocks".to_string(),
            _ => metric.to_string(),
        }
    };

    let direction = if d.better { "up" } else { "down" };
    let fa_direction = if d.better { "بهتر" } else { "بدتر" };
    let pct = |n: f32| {
        if fa {
            format!(
                "{}٪",
                crate::calendar::jalali::fa_num((n * 100.0).round() as i64)
            )
        } else {
            format!("{}%", (n * 100.0).round())
        }
    };

    let (metric_name, delta_str) = match d.metric {
        "kept_rate" | "fulfilment_rate" => (metric_label(d.metric), pct(d.delta)),
        _ => (metric_label(d.metric), (d.delta.round() as i64).to_string()),
    };

    if fa {
        format!(
            "{}: {} {} ({})",
            metric_name,
            delta_str,
            fa_direction,
            metric_label(d.metric)
        )
    } else {
        format!(
            "{} {} {} (was {})",
            metric_name,
            direction,
            delta_str,
            d.baseline.round()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::VerdictKind;

    /// 2026-04-10 = 21 Farvardin 1405 (spring). ~90 days earlier = winter.
    const SPRING: i64 = 1_775_779_200; // 2026-04-10 UTC
    const WINTER: i64 = 1_768_003_200; // 2026-01-10 UTC

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 10).unwrap()
    }

    #[test]
    fn previous_season_wraps_year_boundary() {
        // 2026-04-10 is spring (season 0) of 1405 → previous is winter 1404.
        let season = crate::journal::season_summary::season_of(today());
        assert_eq!(season.season, 0);
        assert_eq!(season.jalali_year, 1405);
        let prev = crate::journal::season_summary::season_bounds(1404, 3);
        // Winter 1404 = Dey, Bahman, Esfand → runs into late March 2026.
        assert!(prev.start < season.range.start);
    }

    #[test]
    fn drift_detects_kept_rate_improvement() {
        // Winter: 2 sessions, both broken. Spring: 2 sessions, both kept.
        let winter_sessions = vec![sess(WINTER, 25), sess(WINTER + 3600, 25)];
        let spring_sessions = vec![sess(SPRING, 25), sess(SPRING + 3600, 25)];
        let winter_verdicts: Vec<Verdict> = winter_sessions
            .iter()
            .map(|s| crate::journal::verdict::declare(s.at, VerdictKind::Broken, s.at).unwrap())
            .collect();
        let spring_verdicts: Vec<Verdict> = spring_sessions
            .iter()
            .map(|s| crate::journal::verdict::declare(s.at, VerdictKind::Kept, s.at).unwrap())
            .collect();
        let all_sessions: Vec<SessionRecord> = winter_sessions
            .iter()
            .chain(spring_sessions.iter())
            .cloned()
            .collect();
        let all_verdicts: Vec<Verdict> = winter_verdicts
            .iter()
            .chain(spring_verdicts.iter())
            .cloned()
            .collect();

        let current = current_metrics(&all_sessions, &[], &[], &all_verdicts, today(), 12600);
        let previous = previous_metrics(&all_sessions, &[], &[], &all_verdicts, today(), 12600);

        assert_eq!(previous.kept_rate, Some(0.0));
        assert_eq!(current.kept_rate, Some(1.0));

        let d = drift(&current, &previous).unwrap();
        assert_eq!(d.metric, "kept_rate");
        assert!(d.better, "kept-rate went up → better");
        assert!((d.delta - 1.0).abs() < 1e-6);
    }

    #[test]
    fn drift_is_none_when_no_change() {
        // Two kept sessions in each season, same count, same kept-rate →
        // nothing moved on any axis → no finding.
        let winter_sessions = vec![sess(WINTER, 25), sess(WINTER + 3600, 25)];
        let spring_sessions = vec![sess(SPRING, 25), sess(SPRING + 3600, 25)];
        let winter_verdicts: Vec<Verdict> = winter_sessions
            .iter()
            .map(|s| crate::journal::verdict::declare(s.at, VerdictKind::Kept, s.at).unwrap())
            .collect();
        let spring_verdicts: Vec<Verdict> = spring_sessions
            .iter()
            .map(|s| crate::journal::verdict::declare(s.at, VerdictKind::Kept, s.at).unwrap())
            .collect();
        let all_sessions: Vec<SessionRecord> = winter_sessions
            .iter()
            .chain(spring_sessions.iter())
            .cloned()
            .collect();
        let all_verdicts: Vec<Verdict> = winter_verdicts
            .iter()
            .chain(spring_verdicts.iter())
            .cloned()
            .collect();

        let current = current_metrics(&all_sessions, &[], &[], &all_verdicts, today(), 12600);
        let previous = previous_metrics(&all_sessions, &[], &[], &all_verdicts, today(), 12600);

        let d = drift(&current, &previous);
        assert!(d.is_none(), "identical seasons produce no drift finding");
    }

    #[test]
    fn headline_label_reports_direction() {
        let d = SeasonDrift {
            metric: "kept_rate",
            delta: 0.2,
            baseline: 0.5,
            better: true,
        };
        let en = headline_label(&d, false);
        assert!(en.contains("kept rate"));
        assert!(en.contains("up"));
        let fa = headline_label(&d, true);
        assert!(fa.contains("نرخ"));
    }
}
