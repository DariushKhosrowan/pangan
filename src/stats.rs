//! Scientific performance reports: daily / weekly / monthly aggregations
//! over completed focus sessions, plus a productivity heuristic.

use crate::store::SessionRecord;
use chrono::{Datelike, Duration, Local, NaiveDate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    Month,
}

impl Period {
    pub fn label(&self) -> &'static str {
        match self {
            Period::Today => "امروز",
            Period::Week => "این هفته",
            Period::Month => "این ماه",
        }
    }

    /// English label, used when the UI language is English.
    pub fn label_en(&self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Week => "This Week",
            Period::Month => "This Month",
        }
    }

    /// (start_date, end_date) inclusive for the period containing `today`.
    pub fn bounds(&self, today: NaiveDate) -> (NaiveDate, NaiveDate) {
        match self {
            Period::Today => (today, today),
            Period::Week => {
                let dow = today.weekday().num_days_from_monday();
                let start = today - Duration::days(dow as i64);
                (start, today)
            }
            Period::Month => {
                let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
                (start, today)
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ReportStats {
    pub period_label: String,
    /// The same label in English, for the English UI.
    pub period_label_en: String,
    /// Number of completed focus sessions in the period.
    pub sessions: u64,
    /// Total focus minutes in the period.
    pub minutes: u64,
    /// Average session length (minutes).
    pub avg: u64,
    /// Distinct days with at least one session.
    pub active_days: u64,
    /// Days in the period (elapsed so far).
    pub days_in_period: u64,
    /// Consistency % = active_days / elapsed days.
    pub consistency: f32,
    /// Best single-day focus minutes.
    pub best_day_minutes: u64,
    /// Current streak: consecutive days (ending today) with >=1 session.
    pub streak: u64,
    /// Heuristic 0..100 deep-work score.
    pub score: u32,
    /// Daily breakdown for the period: (date, minutes) pairs.
    pub daily: Vec<(NaiveDate, u64)>,
}

fn date_of(ts: i64) -> NaiveDate {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.with_timezone(&Local).date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}

/// Count consecutive days <= `today` that have a session (streak).
fn streak_ending_today(
    days_with_session: &std::collections::HashSet<NaiveDate>,
    today: NaiveDate,
) -> u64 {
    let mut streak = 0u64;
    let mut cursor = today;
    loop {
        if days_with_session.contains(&cursor) {
            streak += 1;
            cursor = cursor - Duration::days(1);
        } else if cursor == today {
            // today not active yet; allow streak to be measured from yesterday
            cursor = cursor - Duration::days(1);
            continue;
        } else {
            break;
        }
        if streak > 1000 {
            break;
        }
    }
    streak
}

/// Compute report stats for a period over the given session history.
pub fn report(sessions: &[SessionRecord], period: Period) -> ReportStats {
    let today = Local::now().date_naive();
    let (start, end) = period.bounds(today);
    let elapsed_days = (end - start).num_days().max(0) as u64 + 1;

    use std::collections::HashSet;
    let mut day_minutes: std::collections::BTreeMap<NaiveDate, u64> = Default::default();
    let mut day_set: HashSet<NaiveDate> = HashSet::new();
    let mut total = 0u64;
    let mut count = 0u64;

    for s in sessions {
        let d = date_of(s.at);
        if d >= start && d <= end {
            total += s.minutes;
            count += 1;
            *day_minutes.entry(d).or_insert(0) += s.minutes;
            day_set.insert(d);
        }
    }

    let active = day_set.len() as u64;
    let consistency = if elapsed_days > 0 {
        active as f32 / elapsed_days as f32
    } else {
        0.0
    };
    let best_day = day_minutes.values().copied().max().unwrap_or(0);
    let streak = streak_ending_today(&day_set, today);

    // Deep-work score: blend session volume, consistency, and best-day intensity.
    let mut score = 0u32;
    if elapsed_days > 0 {
        let volume = (total as f32 / (elapsed_days as f32 * 50.0)).min(1.0); // 50min/day ≈ full
        let consistency_w = consistency.clamp(0.0, 1.0);
        let best_w = (best_day as f32 / 120.0).min(1.0); // 2h best day
        score = ((0.4 * volume + 0.4 * consistency_w + 0.2 * best_w) * 100.0).round() as u32;
    }

    ReportStats {
        period_label: period.label().to_string(),
        period_label_en: period.label_en().to_string(),
        sessions: count,
        minutes: total,
        avg: if count > 0 {
            (total as f64 / count as f64).round() as u64
        } else {
            0
        },
        active_days: active,
        days_in_period: elapsed_days,
        consistency,
        best_day_minutes: best_day,
        streak,
        score,
        daily: day_minutes.into_iter().collect(),
    }
}

/// A short human-readable scientific summary of a period.
pub fn narrative(s: &ReportStats, en: bool) -> String {
    let mut out = if en {
        format!(
            "{}\n  Sessions: {}   Focus: {} min\n  Avg session: {} min   Active days: {}/{}\n  Consistency: {:.0}%   Best day: {} min\n  Current streak: {} day(s)\n  Deep-work score: {}/100",
            s.period_label_en,
            s.sessions,
            s.minutes,
            s.avg,
            s.active_days,
            s.days_in_period,
            s.consistency * 100.0,
            s.best_day_minutes,
            s.streak,
            s.score
        )
    } else {
        format!(
            "{}\n  نشست‌ها: {}   کار ژرف: {} دقیقه\n  میانگین نشست: {} دقیقه   روزهای فعال: {}/{}\n  پیوستگی: {:.0}%   بهترین روز: {} دقیقه\n  زنجیرهٔ کنونی: {} روز\n  سنجهٔ کار ژرف: {}/۱۰۰",
            s.period_label,
            s.sessions,
            s.minutes,
            s.avg,
            s.active_days,
            s.days_in_period,
            s.consistency * 100.0,
            s.best_day_minutes,
            s.streak,
            s.score
        )
    };
    // daily rhythm (last up to 7 active days)
    if !s.daily.is_empty() {
        out.push_str(if en { "\n  Daily: " } else { "\n  روزانه: " });
        for (d, m) in s.daily.iter().rev().take(7) {
            out.push_str(&format!(
                "{} {}{} ",
                d.format("%m/%d"),
                m,
                if en { "m" } else { "د" }
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Unix timestamp for a given local date at 12:00 (UTC-equivalent for tests).
    fn ts_for(y: i32, m: u32, d: u32) -> i64 {
        let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let t = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        chrono::NaiveDateTime::new(date, t).and_utc().timestamp()
    }

    #[test]
    fn report_today_filters_to_today() {
        let today = Local::now().date_naive();
        let sess = vec![SessionRecord {
            at: ts_for(today.year(), today.month(), today.day()),
            minutes: 25,
        }];
        let r = report(&sess, Period::Today);
        assert_eq!(r.sessions, 1);
        assert_eq!(r.minutes, 25);
        assert_eq!(r.active_days, 1);
    }

    #[test]
    fn report_empty_has_zero_score_and_no_panic() {
        let r = report(&[], Period::Week);
        assert_eq!(r.sessions, 0);
        assert_eq!(r.minutes, 0);
        assert_eq!(r.score, 0);
        let s = narrative(&r, false);
        assert!(!s.is_empty());
    }

    #[test]
    fn period_bounds_week_starts_monday() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap(); // Wednesday
        let (start, end) = Period::Week.bounds(today);
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()); // Monday
        assert_eq!(end, today);
    }

    #[test]
    fn streak_counts_consecutive_active_days() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();
        let mut days = std::collections::HashSet::new();
        days.insert(today - Duration::days(2));
        days.insert(today - Duration::days(1));
        assert_eq!(streak_ending_today(&days, today), 2);
    }
}
