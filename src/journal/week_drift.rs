//! Week-over-week drift: am I getting better or worse, and where?
//!
//! Every analytics function in this app returns a snapshot: the week's
//! sessions, its fulfilment rate, its break adherence. A snapshot tells you
//! where you stand; it does not tell you whether you are moving. This module
//! is the movement — it compares this week's numbers against the trailing
//! four-week mean and reports which single metric moved the most, in which
//! direction, and whether that move is an improvement.
//!
//! The point is a *headline*, not a dashboard. A long-term user does not need
//! seven numbers; they need to know, on Sunday evening, "you kept 12% more
//! of your plans than your usual", or "your unjudged backlog grew again".
//! That is the one thing every other screen fails to say.
//!
//! Everything here is pure: slices and an explicit `today` in, owned data
//! out, no I/O, no clock reads.
//!
//! How the weeks are gathered: the week containing `today` is the *current*
//! week. The weeks before it (up to [`TRAILING_WEEKS`]) are the baseline. A
//! metric only participates when the current week and at least one baseline
//! week both have a measured value for it.

use chrono::NaiveDate;

/// How many weeks before the current one form the baseline mean.
pub const TRAILING_WEEKS: usize = 4;

/// How many weeks (current + trailing) we gather at most.
pub const MAX_WEEKS: usize = TRAILING_WEEKS + 1;

/// One week's worth of the headline metrics, as measured by the review.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeekMetrics {
    /// The Saturday that starts this week.
    pub week_start: NaiveDate,
    /// Completed focus sessions in the week.
    pub sessions: u32,
    /// Total focus minutes in the week.
    pub focus_minutes: u64,
    /// Kept-or-extended share of rated blocks, 0..=1.
    pub kept_rate: Option<f32>,
    /// Fulfilment rate: landed / planned, 0..=1.
    pub fulfilment_rate: Option<f32>,
    /// Recorded break adherence (followed / completed breaks), 0..=1.
    pub break_adherence: Option<f32>,
    /// Unjudged blocks in the week (promises dropped without being faced).
    pub unjudged: u32,
    /// The hour (0–23) the user was most honest this week, if any.
    pub honest_hour: Option<u8>,
}

/// Resolve a metric key to this week's value, the same mapping the ledger
/// uses to snapshot advice at issue time. Shared by the digest's outcome
/// section and the ledger's `record` call so a prescription is always
/// measured against the same number the advice was issued against.
pub fn value_of(m: &WeekMetrics, metric: &str) -> Option<f32> {
    match metric {
        "sessions" => Some(m.sessions as f32),
        "focus_minutes" => Some(m.focus_minutes as f32),
        "kept_rate" => m.kept_rate,
        "fulfilment_rate" => m.fulfilment_rate,
        "break_adherence" => m.break_adherence,
        "unjudged" => Some(m.unjudged as f32),
        _ => None,
    }
}

/// The headline finding: whichever metric drifted the most.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeekDrift {
    /// The metric's display key ("sessions", "kept_rate", …).
    pub metric: &'static str,
    /// `current − baseline_mean` (raw, unnormalised).
    pub delta: f32,
    /// The baseline mean this delta is measured against.
    pub baseline: f32,
    /// Whether the drift was an improvement for that metric.
    pub better: bool,
}

/// Direction of "better" for each metric. `None` = a shift has no inherent
/// better/worse (e.g. which hour you are honest — moving it is not a verdict).
fn better_direction(metric: &str) -> Option<bool> {
    match metric {
        "sessions" | "focus_minutes" | "kept_rate" | "fulfilment_rate" | "break_adherence" => {
            Some(true)
        }
        "unjudged" => Some(false),
        _ => None,
    }
}

/// Measure one week (the one containing `week_start`) from the store slices.
///
/// This is a thin wrapper over the review builder: it extracts the headline
/// metrics from a [`WeeklyReview`](super::weekly_review::WeeklyReview) without
/// duplicating its assembly logic. Weeks with no data still produce a valid
/// `WeekMetrics` (all zeros / `None`) — they are real, empty weeks.
pub fn week_metrics(
    sessions: &[crate::state::store::SessionRecord],
    decrees: &[crate::journal::Decree],
    inscriptions: &[crate::journal::Inscription],
    verdicts: &[crate::journal::Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    week_start: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> WeekMetrics {
    let review = crate::journal::weekly_review::build_week(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        week_start,
        tz_offset_secs,
        planned_break_secs,
    );
    let s = &review.summary;

    // Kept-rate: met verdicts / rated verdicts within the week. The summary
    // does not carry it directly, so compute it from the verdict slice.
    let kept_rate = {
        let (start_utc, end_utc) =
            crate::journal::weekly_review::week_utc_bounds(review.range, tz_offset_secs);
        let mut rated = 0u32;
        let mut met = 0u32;
        for v in verdicts {
            if v.deleted {
                continue;
            }
            if v.session_at < start_utc || v.session_at > end_utc {
                continue;
            }
            rated += 1;
            if v.kind.is_met() {
                met += 1;
            }
        }
        (rated > 0).then(|| met as f32 / rated as f32)
    };

    WeekMetrics {
        week_start,
        sessions: s.sessions,
        focus_minutes: s.focus_minutes,
        kept_rate,
        fulfilment_rate: (s.planned > 0).then(|| s.landed as f32 / s.planned as f32),
        break_adherence: s
            .recorded_break_adherence
            .or(s.break_adherence)
            .map(|b| b.rate),
        unjudged: s.unjudged_blocks.len() as u32,
        honest_hour: s.honest_hours.and_then(|h| h.best_kept_hour),
    }
}

/// Compute a metric's delta for the given current value and baseline mean.
fn delta_for(current: Option<f32>, baseline_mean: Option<f32>) -> Option<f32> {
    match (current, baseline_mean) {
        (Some(c), Some(b)) => Some(c - b),
        _ => None,
    }
}

/// The headline drift across the gathered weeks.
///
/// Returns the single metric whose `|current − baseline_mean|` is largest and
/// that has a measurable direction (both a current value and a baseline).
/// Returns `None` when there are fewer than two weeks (no baseline), or when
/// no metric has both a current value and a baseline, or when every drift is
/// zero (nothing actually moved).
pub fn drift(weeks: &[WeekMetrics]) -> Option<WeekDrift> {
    if weeks.len() < 2 {
        return None;
    }
    let current = &weeks[0];
    let baseline = &weeks[1..];

    let base_mean = |f: fn(&WeekMetrics) -> Option<f32>| -> Option<f32> {
        let vals: Vec<f32> = baseline.iter().filter_map(f).collect();
        if vals.is_empty() {
            None
        } else {
            Some(vals.iter().sum::<f32>() / vals.len() as f32)
        }
    };

    // Metric key → (current value, baseline mean).
    let candidates: [(&str, Option<f32>, Option<f32>); 6] = [
        (
            "sessions",
            Some(current.sessions as f32),
            base_mean(|w| Some(w.sessions as f32)),
        ),
        (
            "focus_minutes",
            Some(current.focus_minutes as f32),
            base_mean(|w| Some(w.focus_minutes as f32)),
        ),
        ("kept_rate", current.kept_rate, base_mean(|w| w.kept_rate)),
        (
            "fulfilment_rate",
            current.fulfilment_rate,
            base_mean(|w| w.fulfilment_rate),
        ),
        (
            "break_adherence",
            current.break_adherence,
            base_mean(|w| w.break_adherence),
        ),
        (
            "unjudged",
            Some(current.unjudged as f32),
            base_mean(|w| Some(w.unjudged as f32)),
        ),
    ];

    let mut best: Option<(f32, &str, f32, f32)> = None;
    for (metric, cur, base) in candidates {
        let Some(delta) = delta_for(cur, base) else {
            continue;
        };
        let mag = delta.abs();
        if mag < 1e-6 {
            continue; // nothing moved — not a finding
        }
        if best.is_none() || mag > best.unwrap().0 {
            best = Some((mag, metric, delta, base.unwrap_or(0.0)));
        }
    }

    let (_, metric, delta, base) = best?;
    Some(WeekDrift {
        metric,
        delta,
        baseline: base,
        better: better_direction(metric).unwrap_or(false) == (delta >= 0.0),
    })
}

/// A report-ready headline label for the drift, in the requested locale.
pub fn headline_label(d: &WeekDrift, locale_fa: bool) -> String {
    let fa = locale_fa;
    let metric_label = |metric: &str| -> String {
        match (fa, metric) {
            (true, "sessions") => "جلسه‌ها".to_string(),
            (true, "focus_minutes") => "دقیقهٔ تمرکز".to_string(),
            (true, "kept_rate") => "نرخ نگه‌داشتن".to_string(),
            (true, "fulfilment_rate") => "نرخ انجام‌شدن".to_string(),
            (true, "break_adherence") => "پایبندی به استراحت".to_string(),
            (true, "unjudged") => "بلوک‌های بدون داوری".to_string(),
            (false, "sessions") => "sessions".to_string(),
            (false, "focus_minutes") => "focus minutes".to_string(),
            (false, "kept_rate") => "kept rate".to_string(),
            (false, "fulfilment_rate") => "fulfilment rate".to_string(),
            (false, "break_adherence") => "break adherence".to_string(),
            (false, "unjudged") => "unjudged blocks".to_string(),
            (_, m) => m.to_string(),
        }
    };
    // Relative change against the baseline, as a whole percentage.
    let pct = if d.baseline.abs() > 1e-6 {
        ((d.delta / d.baseline).abs() * 100.0).round() as i64
    } else {
        (d.delta.abs() * 100.0).round() as i64
    };
    let pct_fa = crate::calendar::jalali::fa_num(pct);
    if d.better {
        if fa {
            format!(
                "{} {}٪ بهتر از میانگین چهار هفته‌ای شما",
                metric_label(d.metric),
                pct_fa
            )
        } else {
            format!(
                "Your {m} are {p}% better than your 4-week average",
                m = metric_label(d.metric),
                p = pct
            )
        }
    } else if fa {
        format!(
            "{} {}٪ بدتر از میانگین چهار هفته‌ای شما",
            metric_label(d.metric),
            pct_fa
        )
    } else {
        format!(
            "Your {m} are {p}% worse than your 4-week average",
            m = metric_label(d.metric),
            p = pct
        )
    }
}

/// Gather up to [`MAX_WEEKS`] weeks ending at the week containing `today`.
pub fn gather_weeks(
    sessions: &[crate::state::store::SessionRecord],
    decrees: &[crate::journal::Decree],
    inscriptions: &[crate::journal::Inscription],
    verdicts: &[crate::journal::Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> Vec<WeekMetrics> {
    let current_start = crate::journal::weekly_review::week_bounds(today).start;
    let mut weeks = Vec::with_capacity(MAX_WEEKS);
    for i in 0..MAX_WEEKS {
        let start = current_start - chrono::Duration::days(7 * i as i64);
        weeks.push(week_metrics(
            sessions,
            decrees,
            inscriptions,
            verdicts,
            breaks,
            start,
            tz_offset_secs,
            planned_break_secs,
        ));
    }
    weeks
}

/// Total completed sessions across the baseline (all but the first) weeks.
pub fn baseline_sessions(weeks: &[WeekMetrics]) -> u32 {
    weeks.iter().skip(1).map(|w| w.sessions).sum()
}

/// Gather weeks, compute the drift headline, and report the baseline session
/// total (the sample size behind the drift).
pub fn analyze_weeks(
    sessions: &[crate::state::store::SessionRecord],
    decrees: &[crate::journal::Decree],
    inscriptions: &[crate::journal::Inscription],
    verdicts: &[crate::journal::Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> (Option<WeekDrift>, u32) {
    let weeks = gather_weeks(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    (drift(&weeks), baseline_sessions(&weeks))
}

/// Gather up to [`MAX_WEEKS`] weeks ending at the week containing `today`,
/// then produce the drift headline.
pub fn compute_drift(
    sessions: &[crate::state::store::SessionRecord],
    decrees: &[crate::journal::Decree],
    inscriptions: &[crate::journal::Inscription],
    verdicts: &[crate::journal::Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> Option<WeekDrift> {
    analyze_weeks(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    )
    .0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{decree, verdict, VerdictKind};
    use crate::state::store::SessionRecord;

    /// 2026-10-31 is a Saturday (the start of an Iranian week).
    const SAT: i64 = 1_793_431_800;

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    fn today() -> NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
    }

    #[test]
    fn test_drift_empty_baseline_returns_none() {
        // Only one week of data (the current one), no baseline.
        let weeks = vec![WeekMetrics {
            week_start: today(),
            sessions: 4,
            focus_minutes: 100,
            kept_rate: Some(0.5),
            fulfilment_rate: Some(0.5),
            break_adherence: Some(0.5),
            unjudged: 0,
            honest_hour: Some(10),
        }];
        assert!(drift(&weeks).is_none());
    }

    #[test]
    fn test_drift_empty_data_all_weeks_returns_none() {
        // No data at all: every metric is 0/None, so nothing can drift.
        let weeks = (0..MAX_WEEKS)
            .map(|i| {
                week_metrics(
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    today() - chrono::Duration::days(7 * i as i64),
                    12600,
                    300,
                )
            })
            .collect::<Vec<_>>();
        assert!(weeks.len() >= 2);
        assert!(drift(&weeks).is_none());
    }

    #[test]
    fn test_drift_detects_improved_sessions() {
        // Baseline: 2 sessions/week, 50 focus min/week for 4 weeks.
        // Current: 6 sessions but the SAME focus minutes (decoupled so
        // sessions is the only mover).
        let mut weeks: Vec<WeekMetrics> = (1..=TRAILING_WEEKS)
            .map(|i| WeekMetrics {
                week_start: today() - chrono::Duration::days(7 * i as i64),
                sessions: 2,
                focus_minutes: 50,
                kept_rate: Some(0.5),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 0,
                honest_hour: Some(10),
            })
            .collect();
        weeks.insert(
            0,
            WeekMetrics {
                week_start: today(),
                sessions: 6,
                focus_minutes: 50,
                kept_rate: Some(0.5),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 0,
                honest_hour: Some(10),
            },
        );
        let d = drift(&weeks).expect("should have a headline");
        assert_eq!(d.metric, "sessions");
        assert!(d.better, "more sessions is better");
        assert!((d.delta - 4.0).abs() < 1e-6, "6 − 2 = 4");
        assert!((d.baseline - 2.0).abs() < 1e-6);
    }

    #[test]
    fn test_drift_picks_largest_absolute_move() {
        // Everything decoupled. Baseline: sessions=2, kept_rate=0.5,
        // focus=50. Current: sessions=3 (delta +1), kept_rate=0.9
        // (delta +0.4), focus=50 (no move). Sessions has the largest delta.
        let mut weeks: Vec<WeekMetrics> = (1..=TRAILING_WEEKS)
            .map(|i| WeekMetrics {
                week_start: today() - chrono::Duration::days(7 * i as i64),
                sessions: 2,
                focus_minutes: 50,
                kept_rate: Some(0.5),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 0,
                honest_hour: Some(10),
            })
            .collect();
        weeks.insert(
            0,
            WeekMetrics {
                week_start: today(),
                sessions: 3,
                focus_minutes: 50,
                kept_rate: Some(0.9),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 0,
                honest_hour: Some(10),
            },
        );
        let d = drift(&weeks).unwrap();
        assert_eq!(d.metric, "sessions");
        assert!((d.delta - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_drift_worse_unjudged_is_not_better() {
        let mut weeks: Vec<WeekMetrics> = (1..=TRAILING_WEEKS)
            .map(|i| WeekMetrics {
                week_start: today() - chrono::Duration::days(7 * i as i64),
                sessions: 4,
                focus_minutes: 100,
                kept_rate: Some(0.5),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 1,
                honest_hour: Some(10),
            })
            .collect();
        weeks.insert(
            0,
            WeekMetrics {
                week_start: today(),
                sessions: 4,
                focus_minutes: 100,
                kept_rate: Some(0.5),
                fulfilment_rate: Some(0.5),
                break_adherence: Some(0.5),
                unjudged: 5,
                honest_hour: Some(10),
            },
        );
        let d = drift(&weeks).unwrap();
        assert_eq!(d.metric, "unjudged");
        assert!(!d.better, "more unjudged blocks is worse");
    }

    #[test]
    fn test_compute_drift_from_store_slices() {
        // With empty slices across all weeks, nothing can drift → None.
        assert!(compute_drift(&[], &[], &[], &[], &[], today(), 12600, 300).is_none());

        // Now give the current week 6 sessions. The prior weeks are still
        // empty (sessions=0), so sessions has current=6, baseline=0 → a real
        // finding. This confirms the helper runs on real slices end to end.
        let sessions: Vec<SessionRecord> = (0..6).map(|i| sess(SAT + i * 60, 25)).collect();
        let verdicts: Vec<crate::journal::Verdict> = sessions
            .iter()
            .map(|s| verdict::declare(s.at, VerdictKind::Kept, s.at).unwrap())
            .collect();
        let d = compute_drift(&sessions, &[], &[], &verdicts, &[], today(), 12600, 300)
            .expect("sessions moved from 0 to 6");
        // The current week has 6 sessions × 25min = 150 focus minutes vs a
        // zero baseline, so either focus_minutes or sessions is the improving
        // headline. Both are legitimate.
        assert!(d.better, "a move from nothing to something is improvement");
        assert!(
            d.metric == "sessions" || d.metric == "focus_minutes",
            "headline should be the volume metric, got: {}",
            d.metric
        );
    }

    #[test]
    fn test_headline_label_english_and_persian() {
        let up = WeekDrift {
            metric: "sessions",
            delta: 4.0,
            baseline: 2.0,
            better: true,
        };
        let down = WeekDrift {
            metric: "unjudged",
            delta: 3.0,
            baseline: 1.0,
            better: false,
        };
        let en = headline_label(&up, false);
        assert!(en.contains("better"), "English headline: {en}");
        // 4/2 = 200% better.
        assert!(en.contains("200%"));
        let fa = headline_label(&up, true);
        assert!(fa.contains("بهتر"), "Persian headline: {fa}");
        assert!(fa.contains("٪"));
        let en_down = headline_label(&down, false);
        assert!(en_down.contains("worse"), "worse headline: {en_down}");
        let fa_down = headline_label(&down, true);
        assert!(
            fa_down.contains("بدتر"),
            "Persian worse headline: {fa_down}"
        );
    }
}
