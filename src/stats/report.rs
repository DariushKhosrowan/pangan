//! Reports: period-scoped aggregations over completed focus sessions.
//!
//! A report answers a question about one *span of days*: today, this week, a
//! past week, a Jalali month, a year, or everything since the first session.
//! The period and the span are separate ideas — the period is the shape of the
//! question, the offset is which one you are asking about — so "last week" is
//! `Period::Week` with offset −1, not a separate enum case.
//!
//! Bounds are Jalali where the app's calendar is Jalali (month, year) and
//! Saturday-first for the week, because that is when an Iranian week starts.
//! The old code used a Monday-first week with a Saturday-first calendar on the
//! next screen, which is how the bar chart ended up labelling the wrong day.

use crate::calendar::jalali as calendar;
use crate::state::store::SessionRecord;
use chrono::{Datelike, Duration, Local, NaiveDate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Year,
    All,
}

impl Period {
    pub fn label(&self) -> &'static str {
        match self {
            Period::Day => "روز",
            Period::Week => "هفته",
            Period::Month => "ماه",
            Period::Year => "سال",
            Period::All => "همه",
        }
    }

    pub fn label_en(&self) -> &'static str {
        match self {
            Period::Day => "Day",
            Period::Week => "Week",
            Period::Month => "Month",
            Period::Year => "Year",
            Period::All => "All",
        }
    }

    /// The full label, used in the report's own header line.
    #[allow(dead_code)] // test support, not called from the app
    pub fn title(&self) -> &'static str {
        match self {
            Period::Day => "امروز",
            Period::Week => "این هفته",
            Period::Month => "این ماه",
            Period::Year => "این سال",
            Period::All => "همهٔ تاریخچه",
        }
    }

    #[allow(dead_code)] // test support, not called from the app
    pub fn title_en(&self) -> &'static str {
        match self {
            Period::Day => "Today",
            Period::Week => "This week",
            Period::Month => "This month",
            Period::Year => "This year",
            Period::All => "All time",
        }
    }

    pub fn index(&self) -> i32 {
        match self {
            Period::Day => 0,
            Period::Week => 1,
            Period::Month => 2,
            Period::Year => 3,
            Period::All => 4,
        }
    }

    pub fn from_index(i: i32) -> Period {
        match i.clamp(0, 4) {
            0 => Period::Day,
            1 => Period::Week,
            2 => Period::Month,
            3 => Period::Year,
            _ => Period::All,
        }
    }
}

/// An inclusive span of days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl Range {
    #[allow(dead_code)] // test support, not called from the app
    pub fn days(&self) -> u64 {
        (self.end - self.start).num_days().max(0) as u64 + 1
    }

    pub fn contains(&self, d: NaiveDate) -> bool {
        d >= self.start && d <= self.end
    }
}

/// Column of the week, 0 = Saturday. The Iranian week starts on Saturday, so
/// every week-shaped thing in the app uses this and not `num_days_from_monday`.
pub fn week_index(d: NaiveDate) -> i64 {
    // chrono only offers Monday- and Sunday-based indices. Saturday is two
    // days before Monday, so shifting by two lands 0 on Saturday.
    ((d.weekday().num_days_from_monday() as i64) + 2) % 7
}

fn greg(jy: i32, jm: u32, jd: u32) -> NaiveDate {
    let (gy, gm, gd) = calendar::to_gregorian(jy, jm, jd);
    NaiveDate::from_ymd_opt(gy, gm, gd).unwrap_or_else(|| Local::now().date_naive())
}

fn jalali_of(d: NaiveDate) -> (i32, u32, u32) {
    calendar::from_gregorian(d.year(), d.month(), d.day())
}

/// The span a period/offset pair names, anchored on `today`.
///
/// `offset` counts whole periods backwards from the current one: 0 is this
/// week, −1 is last week. `earliest` is only used by `Period::All`, which has
/// no offset of its own.
pub fn bounds(period: Period, offset: i32, today: NaiveDate, earliest: Option<NaiveDate>) -> Range {
    match period {
        Period::Day => {
            let d = today + Duration::days(offset as i64);
            Range { start: d, end: d }
        }
        Period::Week => {
            let base = today - Duration::days(week_index(today));
            let start = base + Duration::days(offset as i64 * 7);
            Range {
                start,
                end: start + Duration::days(6),
            }
        }
        Period::Month => {
            let (jy, jm, _) = jalali_of(today);
            let (y, m) = calendar::shift_month(jy, jm, offset);
            let len = calendar::month_len(y, m);
            Range {
                start: greg(y, m, 1),
                end: greg(y, m, len),
            }
        }
        Period::Year => {
            let (jy, _, _) = jalali_of(today);
            let y = jy + offset;
            let len = calendar::month_len(y, 12);
            Range {
                start: greg(y, 1, 1),
                end: greg(y, 12, len),
            }
        }
        Period::All => {
            let start = earliest.unwrap_or(today).min(today);
            Range { start, end: today }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Aggregation
// ───────────────────────────────────────────────────────────────────────────

/// One column of the report's chart.
// Fields below are part of an external format (chart bucket; sessions is drawn by the report screen).
// Nothing in this binary reads them yet; they are kept so the shape
// stays documented and parsing does not silently drift.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct Bucket {
    /// Short label under the column ("ش", "۱۲", "مهر").
    pub label: String,
    /// Longer label for the tooltip / detail row ("شنبه ۱۲ آبان").
    pub detail: String,
    pub minutes: u64,
    pub sessions: u64,
    /// True for the bucket that contains today, so the chart can mark it.
    pub current: bool,
    /// True when the bucket is in the future relative to today; the chart
    /// draws it as an empty slot rather than as a day the user skipped.
    pub future: bool,
}

// Fields below are part of an external format (report payload; labels are read by the report screen).
// Nothing in this binary reads them yet; they are kept so the shape
// stays documented and parsing does not silently drift.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct ReportStats {
    pub period_label: String,
    pub period_label_en: String,
    /// The human title of the span ("این هفته", "آبان ۱۴۰۵", "۱۴۰۴").
    pub title: String,
    pub title_en: String,
    /// The exact span, for the footer line.
    pub span: String,
    pub span_en: String,
    pub offset: i32,

    pub sessions: u64,
    pub minutes: u64,
    pub avg: u64,
    pub active_days: u64,
    pub days_in_period: u64,
    pub consistency: f32,
    pub best_day_minutes: u64,
    pub best_day_label: String,
    pub best_day_label_en: String,
    pub streak: u64,
    pub longest_streak: u64,
    pub score: u32,

    /// Same shape as the current span, shifted one period back.
    pub prev_minutes: u64,
    pub prev_sessions: u64,
    pub prev_score: u32,
    /// Percent change in focus minutes, 0 when there is no previous period.
    pub delta_pct: f32,
    pub has_prev: bool,

    /// The chart: 7 days for a week, days of the Jalali month, 12 months for a
    /// year, hours for a single day.
    pub buckets: Vec<Bucket>,
    /// Focus minutes by hour of day, 0..24. Always 24 entries.
    pub hours: Vec<u64>,
    /// All active days in the span, ascending — used by the narrative and by
    /// the mini heat strip.
    pub daily: Vec<(NaiveDate, u64)>,

    /// Whether the span contains today (so "streak" is the live one).
    pub contains_today: bool,
}

fn date_of(ts: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local).date_naive())
}

fn hour_of(ts: i64) -> Option<u32> {
    use chrono::Timelike;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&Local).hour())
}

/// Consecutive days with a session, ending at `end` (or at the last active day
/// at or before `end` when `end` itself is empty — a streak is not broken by
/// not having started today yet).
fn streak_ending_on(days: &std::collections::HashSet<NaiveDate>, end: NaiveDate) -> u64 {
    let mut cursor = end;
    if !days.contains(&cursor) {
        cursor -= Duration::days(1);
    }
    let mut streak = 0u64;
    while days.contains(&cursor) {
        streak += 1;
        cursor -= Duration::days(1);
        if streak > 4000 {
            break;
        }
    }
    streak
}

/// The longest run of consecutive active days anywhere at or before `end`.
fn longest_streak(days: &std::collections::HashSet<NaiveDate>, end: NaiveDate) -> u64 {
    let mut sorted: Vec<NaiveDate> = days.iter().copied().filter(|d| *d <= end).collect();
    sorted.sort();
    let mut best = 0u64;
    let mut run = 0u64;
    let mut prev: Option<NaiveDate> = None;
    for d in sorted {
        run = match prev {
            Some(p) if d == p + Duration::days(1) => run + 1,
            _ => 1,
        };
        best = best.max(run);
        prev = Some(d);
    }
    best
}

/// Does this range's end sit in the future for the part we should count?
///
/// A period that has not finished yet (this week, this month) is scored on the
/// days that have actually elapsed, not on its full nominal length. Scoring a
/// month on day 2 out of 30 would read as a catastrophic month.
fn counted_days(range: Range, today: NaiveDate) -> u64 {
    let end = range.end.min(today);
    if end < range.start {
        0
    } else {
        (end - range.start).num_days() as u64 + 1
    }
}

fn deep_work_score(total: u64, consistency: f32, best_day: u64, days: u64) -> u32 {
    if days == 0 {
        return 0;
    }
    let volume = (total as f32 / (days as f32 * 50.0)).min(1.0);
    let consistency_w = consistency.clamp(0.0, 1.0);
    let best_w = (best_day as f32 / 120.0).min(1.0);
    ((0.4 * volume + 0.4 * consistency_w + 0.2 * best_w) * 100.0).round() as u32
}

/// Format a Gregorian date as a Jalali one, digits in the right script.
pub fn jalali_label(d: NaiveDate, en: bool, with_year: bool) -> String {
    let (jy, jm, jd) = jalali_of(d);
    let month = if en {
        calendar::MONTHS_EN[(jm as usize).clamp(1, 12) - 1]
    } else {
        calendar::MONTHS[(jm as usize).clamp(1, 12) - 1]
    };
    let num = |n: i64| {
        if en {
            n.to_string()
        } else {
            calendar::fa_num(n)
        }
    };
    if with_year {
        format!("{} {} {}", num(jd as i64), month, num(jy as i64))
    } else {
        format!("{} {}", num(jd as i64), month)
    }
}

fn minutes_label(total: u64, en: bool) -> String {
    // Over an hour, "3h 20m" reads faster than "200 min" and is what a person
    // actually says about a month of work.
    if total >= 60 {
        let h = total / 60;
        let m = total % 60;
        if en {
            if m == 0 {
                format!("{h}h")
            } else {
                format!("{h}h {m}m")
            }
        } else if m == 0 {
            format!("{} ساعت", calendar::fa_num(h as i64))
        } else {
            format!(
                "{} ساعت و {} دقیقه",
                calendar::fa_num(h as i64),
                calendar::fa_num(m as i64)
            )
        }
    } else if en {
        format!("{total}m")
    } else {
        format!("{} دقیقه", calendar::fa_num(total as i64))
    }
}

/// The buckets for a span: hours for a day, days for a week or month, months
/// for a year, and a rolling set of months for all-time.
fn build_buckets(
    period: Period,
    range: Range,
    today: NaiveDate,
    per_day: &std::collections::BTreeMap<NaiveDate, (u64, u64)>,
    per_hour: &[u64; 24],
    per_hour_sessions: &[u64; 24],
    en: bool,
) -> Vec<Bucket> {
    let mut out: Vec<Bucket> = Vec::new();
    let weekday_short: [&str; 7] = if en {
        ["Sa", "Su", "Mo", "Tu", "We", "Th", "Fr"]
    } else {
        ["ش", "ی", "د", "س", "چ", "پ", "ج"]
    };
    let weekday_full: [&str; 7] = if en {
        [
            "Saturday",
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
        ]
    } else {
        [
            "شنبه",
            "یک‌شنبه",
            "دوشنبه",
            "سه‌شنبه",
            "چهارشنبه",
            "پنج‌شنبه",
            "جمعه",
        ]
    };
    match period {
        Period::Day => {
            for h in 0..24 {
                let m = per_hour[h];
                out.push(Bucket {
                    label: if en {
                        if h % 3 == 0 {
                            format!("{h:02}")
                        } else {
                            String::new()
                        }
                    } else if h % 3 == 0 {
                        calendar::fa_num(h as i64)
                    } else {
                        String::new()
                    },
                    detail: if en {
                        format!("{h:02}:00")
                    } else {
                        format!("{}:۰۰", calendar::fa_num(h as i64))
                    },
                    minutes: m,
                    sessions: per_hour_sessions[h],
                    current: false,
                    future: false,
                });
            }
            // Session counts by hour come from the raw list; minutes alone
            // cannot tell "one 50-minute block" from "two 25s".
            out
        }
        Period::Week | Period::Month => {
            let mut d = range.start;
            while d <= range.end {
                let (mins, sess) = per_day.get(&d).copied().unwrap_or((0, 0));
                let wd = week_index(d);
                out.push(Bucket {
                    label: if period == Period::Week {
                        weekday_short[wd as usize].to_string()
                    } else if en {
                        d.day().to_string()
                    } else {
                        calendar::fa_num(d.day() as i64)
                    },
                    detail: format!(
                        "{} · {}",
                        weekday_full[wd as usize],
                        jalali_label(d, en, false)
                    ),
                    minutes: mins,
                    sessions: sess,
                    current: d == today,
                    future: d > today,
                });
                d += Duration::days(1);
            }
            out
        }
        Period::Year => {
            let (jy, _, _) = jalali_of(range.start);
            for m in 1..=12u32 {
                let len = calendar::month_len(jy, m);
                let s = greg(jy, m, 1);
                let e = greg(jy, m, len);
                let mut mins = 0u64;
                let mut sess = 0u64;
                let mut d = s;
                while d <= e {
                    if let Some((mm, ss)) = per_day.get(&d) {
                        mins += mm;
                        sess += ss;
                    }
                    d += Duration::days(1);
                }
                out.push(Bucket {
                    label: if en {
                        calendar::MONTHS_EN[(m as usize) - 1]
                            [..3.min(calendar::MONTHS_EN[(m as usize) - 1].len())]
                            .to_string()
                    } else {
                        calendar::MONTHS[(m as usize) - 1].to_string()
                    },
                    detail: format!(
                        "{} {}",
                        if en {
                            calendar::MONTHS_EN[(m as usize) - 1]
                        } else {
                            calendar::MONTHS[(m as usize) - 1]
                        },
                        if en {
                            jy.to_string()
                        } else {
                            calendar::fa_num(jy as i64)
                        }
                    ),
                    minutes: mins,
                    sessions: sess,
                    current: today >= s && today <= e,
                    future: s > today,
                });
            }
            out
        }
        Period::All => {
            // Month-shaped columns, newest last, capped so the chart stays
            // legible on a 420px window.
            let mut months: Vec<(i32, u32)> = Vec::new();
            let (sy, sm, _) = jalali_of(range.start);
            let (ey, em, _) = jalali_of(range.end.min(today));
            let (mut y, mut m) = (sy, sm);
            while (y, m) <= (ey, em) {
                months.push((y, m));
                let (ny, nm) = calendar::shift_month(y, m, 1);
                y = ny;
                m = nm;
            }
            if months.len() > 18 {
                months = months.split_off(months.len() - 18);
            }
            for (y, m) in months {
                let len = calendar::month_len(y, m);
                let (s, e) = (greg(y, m, 1), greg(y, m, len));
                let mut mins = 0u64;
                let mut sess = 0u64;
                let mut d = s;
                while d <= e {
                    if let Some((mm, ss)) = per_day.get(&d) {
                        mins += mm;
                        sess += ss;
                    }
                    d += Duration::days(1);
                }
                let name = if en {
                    calendar::MONTHS_EN[(m as usize) - 1]
                } else {
                    calendar::MONTHS[(m as usize) - 1]
                };
                out.push(Bucket {
                    label: if en {
                        name[..3.min(name.len())].to_string()
                    } else {
                        name.to_string()
                    },
                    detail: format!(
                        "{} {}",
                        name,
                        if en {
                            y.to_string()
                        } else {
                            calendar::fa_num(y as i64)
                        }
                    ),
                    minutes: mins,
                    sessions: sess,
                    current: today >= s && today <= e,
                    future: false,
                });
            }
            out
        }
    }
}

/// Aggregate one span of sessions.
///
/// `offset` is how many periods back to look: 0 is the current one. The
/// previous period is always computed too, so the report can say whether the
/// user is up or down without a second call.
#[cfg(test)]
pub fn report(sessions: &[SessionRecord], period: Period, offset: i32) -> ReportStats {
    let today = Local::now().date_naive();
    let en = false; // caller re-labels; kept for internal formatting defaults
    let earliest = sessions.iter().filter_map(|s| date_of(s.at)).min();
    report_at(sessions, period, offset, today, earliest, en)
}

/// The full form: explicit anchor date, explicit earliest session, explicit
/// language. `report` is the convenience wrapper the app calls.
pub fn report_at(
    sessions: &[SessionRecord],
    period: Period,
    offset: i32,
    today: NaiveDate,
    earliest: Option<NaiveDate>,
    en: bool,
) -> ReportStats {
    let range = bounds(period, offset, today, earliest);
    // Offsets count *backwards*: 0 is the current span, -1 the one before it.
    // The previous span is therefore offset − 1, not offset + 1 — the latter
    // walks forward into the future and reports an empty comparison.
    // All-time has no previous span; it is compared against nothing.
    //
    // When the current span is still running (this week, this month), the
    // previous span is cut to the same number of elapsed days. Comparing six
    // days of Aban against all thirty of Mehr would report a collapse every
    // time the month turned over, which is a calendar artefact, not a fact
    // about the user's work.
    let prev_range = if period == Period::All {
        None
    } else {
        let full = bounds(period, offset - 1, today, earliest);
        let elapsed_now = counted_days(range, today);
        let elapsed_prev = counted_days(full, today);
        if offset == 0 && elapsed_now > 0 && elapsed_now < elapsed_prev {
            Some(Range {
                start: full.start,
                end: full.start + Duration::days(elapsed_now as i64 - 1),
            })
        } else {
            Some(full)
        }
    };
    use std::collections::{BTreeMap, HashSet};
    let mut per_day: BTreeMap<NaiveDate, (u64, u64)> = BTreeMap::new();
    let mut per_hour = [0u64; 24];
    let mut per_hour_sessions = [0u64; 24];
    let mut prev_minutes = 0u64;
    let mut prev_sessions = 0u64;
    let mut prev_days: HashSet<NaiveDate> = HashSet::new();
    let mut all_days: HashSet<NaiveDate> = HashSet::new();
    for s in sessions {
        let Some(d) = date_of(s.at) else { continue };
        all_days.insert(d);
        if range.contains(d) {
            let e = per_day.entry(d).or_insert((0, 0));
            e.0 += s.minutes;
            e.1 += 1;
            if let Some(h) = hour_of(s.at) {
                per_hour[h as usize] += s.minutes;
                per_hour_sessions[h as usize] += 1;
            }
        }
        if prev_range.map(|r| r.contains(d)).unwrap_or(false) {
            prev_minutes += s.minutes;
            prev_sessions += 1;
            prev_days.insert(d);
        }
    }

    let minutes: u64 = per_day.values().map(|(m, _)| *m).sum();
    let count: u64 = per_day.values().map(|(_, c)| *c).sum();
    let active = per_day.len() as u64;
    let counted = counted_days(range, today);
    let consistency = if counted > 0 {
        (active.min(counted) as f32) / (counted as f32)
    } else {
        0.0
    };
    let (best_day, best_date) = per_day
        .iter()
        .max_by_key(|(_, (m, _))| *m)
        .map(|(d, (m, _))| (*m, Some(*d)))
        .unwrap_or((0, None));
    let score = deep_work_score(minutes, consistency, best_day, counted);
    let prev_counted = prev_range.map(|r| counted_days(r, today)).unwrap_or(0);
    let prev_consistency = if prev_counted > 0 {
        (prev_days.len() as u64).min(prev_counted) as f32 / prev_counted as f32
    } else {
        0.0
    };
    let prev_best = {
        let mut b = 0u64;
        let mut acc: BTreeMap<NaiveDate, u64> = BTreeMap::new();
        for s in sessions {
            if let Some(d) = date_of(s.at) {
                if prev_range.map(|r| r.contains(d)).unwrap_or(false) {
                    *acc.entry(d).or_insert(0) += s.minutes;
                }
            }
        }
        for v in acc.values() {
            b = b.max(*v);
        }
        b
    };
    let prev_score = deep_work_score(prev_minutes, prev_consistency, prev_best, prev_counted);
    let has_prev = prev_sessions > 0 || prev_minutes > 0;
    let delta_pct = if prev_minutes > 0 {
        ((minutes as f32 - prev_minutes as f32) / prev_minutes as f32) * 100.0
    } else if minutes > 0 {
        100.0
    } else {
        0.0
    };
    let buckets = build_buckets(
        period,
        range,
        today,
        &per_day,
        &per_hour,
        &per_hour_sessions,
        en,
    ); // Titles: "این هفته" for offset 0, "هفتهٔ پیش" for −1, and a dated title
       // for anything further back so the user always knows what they are seeing.
    let (title, title_en) = span_title(period, offset, range, en);
    let span = format!(
        "{} – {}",
        jalali_label(range.start, en, true),
        jalali_label(range.end, en, true)
    );
    let span_en = span.clone();
    let contains_today = range.contains(today);
    let streak = streak_ending_on(&all_days, today.min(range.end));
    let longest = longest_streak(&all_days, today);
    ReportStats {
        period_label: period.label().to_string(),
        period_label_en: period.label_en().to_string(),
        title,
        title_en,
        span,
        span_en,
        offset,
        sessions: count,
        minutes,
        avg: if count > 0 {
            (minutes as f64 / count as f64).round() as u64
        } else {
            0
        },
        active_days: active,
        days_in_period: counted,
        consistency,
        best_day_minutes: best_day,
        best_day_label: best_date
            .map(|d| jalali_label(d, en, false))
            .unwrap_or_default(),
        best_day_label_en: best_date
            .map(|d| jalali_label(d, true, false))
            .unwrap_or_default(),
        streak,
        longest_streak: longest,
        score,
        prev_minutes,
        prev_sessions,
        prev_score,
        delta_pct,
        has_prev,
        buckets,
        hours: per_hour.to_vec(),
        daily: per_day.iter().map(|(d, (m, _))| (*d, *m)).collect(),
        contains_today,
    }
}

fn span_title(period: Period, offset: i32, range: Range, en: bool) -> (String, String) {
    let num = |n: i64| {
        if en {
            n.to_string()
        } else {
            calendar::fa_num(n)
        }
    };
    // The day shown in a title is the Jalali day, not the Gregorian one: on
    // 6 October the app must say "۲۸ مهر", and reading `range.start.day()`
    // there gave "۶ مهر" — the wrong day, in the right month.
    let (jy, jm, jd) = jalali_of(range.start);
    let month = |m: u32| -> (&'static str, &'static str) {
        (
            calendar::MONTHS[(m as usize).clamp(1, 12) - 1],
            calendar::MONTHS_EN[(m as usize).clamp(1, 12) - 1],
        )
    };
    match period {
        Period::Day => {
            if offset == 0 {
                ("امروز".into(), "Today".into())
            } else if offset == -1 {
                ("دیروز".into(), "Yesterday".into())
            } else {
                let (m_fa, m_en) = month(jm);
                (
                    format!("{} {}", num(jd as i64), m_fa),
                    format!("{jd} {m_en}"),
                )
            }
        }
        Period::Week => {
            if offset == 0 {
                ("این هفته".into(), "This week".into())
            } else if offset == -1 {
                ("هفتهٔ پیش".into(), "Last week".into())
            } else {
                (
                    format!("هفتهٔ {} {}", num(jd as i64), month(jm).0),
                    format!("Week of {jd} {}", month(jm).1),
                )
            }
        }
        Period::Month => {
            let (m_fa, m_en) = month(jm);
            if offset == 0 {
                ("این ماه".into(), "This month".into())
            } else if offset == -1 {
                ("ماه پیش".into(), "Last month".into())
            } else {
                (format!("{m_fa} {}", num(jy as i64)), format!("{m_en} {jy}"))
            }
        }
        Period::Year => {
            if offset == 0 {
                ("این سال".into(), "This year".into())
            } else if offset == -1 {
                ("سال پیش".into(), "Last year".into())
            } else {
                (num(jy as i64), jy.to_string())
            }
        }
        Period::All => ("همهٔ تاریخچه".into(), "All time".into()),
    }
}

/// A short human-readable summary of a span.
pub fn narrative(s: &ReportStats, en: bool) -> String {
    let mut out = String::new();
    let m = |v: u64| minutes_label(v, en);
    if en {
        out.push_str(&format!("{}  ({})\n", s.title_en, s.span_en));
        out.push_str(&format!(
            "  Focus: {}   Sessions: {}\n",
            m(s.minutes),
            s.sessions
        ));
        out.push_str(&format!(
            "  Avg session: {}   Active days: {}/{}\n",
            m(s.avg),
            s.active_days,
            s.days_in_period
        ));
        out.push_str(&format!(
            "  Consistency: {:.0}%   Best day: {}",
            s.consistency * 100.0,
            m(s.best_day_minutes)
        ));
        if !s.best_day_label_en.is_empty() {
            out.push_str(&format!(" ({})", s.best_day_label_en));
        }
        out.push('\n');
        if s.has_prev {
            let arrow = if s.delta_pct >= 0.0 { "▲" } else { "▼" };
            out.push_str(&format!(
                "  vs previous: {arrow} {:.0}%  ({} → {})\n",
                s.delta_pct.abs(),
                m(s.prev_minutes),
                m(s.minutes)
            ));
        }
        out.push_str(&format!(
            "  Streak: {} day(s)   Longest: {}\n",
            s.streak, s.longest_streak
        ));
        out.push_str(&format!("  Deep-work score: {}/100", s.score));
    } else {
        out.push_str(&format!("{}  ({})\n", s.title, s.span));
        out.push_str(&format!(
            "  کار ژرف: {}   نشست‌ها: {}\n",
            m(s.minutes),
            calendar::fa_num(s.sessions as i64)
        ));
        out.push_str(&format!(
            "  میانگین نشست: {}   روزهای فعال: {}/{}\n",
            m(s.avg),
            calendar::fa_num(s.active_days as i64),
            calendar::fa_num(s.days_in_period as i64)
        ));
        out.push_str(&format!(
            "  پیوستگی: {:.0}٪   بهترین روز: {}",
            s.consistency * 100.0,
            m(s.best_day_minutes)
        ));
        if !s.best_day_label.is_empty() {
            out.push_str(&format!(" ({})", s.best_day_label));
        }
        out.push('\n');
        if s.has_prev {
            let arrow = if s.delta_pct >= 0.0 { "▲" } else { "▼" };
            out.push_str(&format!(
                "  نسبت به دورهٔ پیش: {arrow} {:.0}٪  ({} ← {})\n",
                s.delta_pct.abs(),
                m(s.prev_minutes),
                m(s.minutes)
            ));
        }
        out.push_str(&format!(
            "  زنجیره: {} روز   بلندترین: {} روز\n",
            calendar::fa_num(s.streak as i64),
            calendar::fa_num(s.longest_streak as i64)
        ));
        out.push_str(&format!(
            "  سنجهٔ کار ژرف: {}/۱۰۰",
            calendar::fa_num(s.score as i64)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ts_for(y: i32, m: u32, d: u32, hour: u32) -> i64 {
        let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
        let t = chrono::NaiveTime::from_hms_opt(hour, 0, 0).unwrap();
        chrono::NaiveDateTime::new(date, t).and_utc().timestamp()
    }

    fn rec(y: i32, m: u32, d: u32, minutes: u64) -> SessionRecord {
        SessionRecord {
            at: ts_for(y, m, d, 12),
            minutes,
        }
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn day_period_counts_only_that_day() {
        let today = day(2026, 10, 28);
        let s = vec![rec(2026, 10, 28, 25), rec(2026, 10, 27, 50)];
        let r = report_at(&s, Period::Day, 0, today, None, false);
        assert_eq!(r.sessions, 1);
        assert_eq!(r.minutes, 25);
        assert_eq!(r.buckets.len(), 24);
    }

    #[test]
    fn week_starts_on_saturday() {
        // 2026-10-28 is a Wednesday; the Iranian week began Saturday 2026-10-24.
        let today = day(2026, 10, 28);
        let r = bounds(Period::Week, 0, today, None);
        assert_eq!(r.start, day(2026, 10, 24));
        assert_eq!(r.end, day(2026, 10, 30));
        assert_eq!(r.days(), 7);
    }

    #[test]
    fn previous_week_is_seven_days_earlier() {
        let today = day(2026, 10, 28);
        let r = bounds(Period::Week, -1, today, None);
        assert_eq!(r.start, day(2026, 10, 17));
        assert_eq!(r.end, day(2026, 10, 23));
    }

    #[test]
    fn month_bounds_are_jalali() {
        // 2026-10-28 is 6 Aban 1405.
        let today = day(2026, 10, 28);
        let r = bounds(Period::Month, 0, today, None);
        let (sy, sm, sd) = calendar::from_gregorian(r.start.year(), r.start.month(), r.start.day());
        assert_eq!((sm, sd), (8, 1), "month must start on 1 Aban");
        let (ey, em, ed) = calendar::from_gregorian(r.end.year(), r.end.month(), r.end.day());
        assert_eq!((em, ed), (8, 30), "Aban has 30 days");
        assert_eq!(sy, ey);
    }

    #[test]
    fn month_offset_walks_back_a_jalali_month() {
        let today = day(2026, 10, 28);
        let r = bounds(Period::Month, -1, today, None);
        let (_, m, d) = calendar::from_gregorian(r.start.year(), r.start.month(), r.start.day());
        assert_eq!((m, d), (7, 1), "previous month starts on 1 Mehr");
    }

    #[test]
    fn month_bucket_count_matches_jalali_month_length() {
        let today = day(2026, 10, 28);
        let r = report_at(&[], Period::Month, 0, today, None, false);
        assert_eq!(r.buckets.len(), 30, "Aban is a 30-day month");
    }

    #[test]
    fn year_has_twelve_month_buckets() {
        let today = day(2026, 10, 28);
        let r = report_at(&[], Period::Year, 0, today, None, false);
        assert_eq!(r.buckets.len(), 12);
    }

    #[test]
    fn elapsed_days_do_not_penalise_a_running_month() {
        // On 6 Aban, a month is 6 days old, not 30. Consistency must be
        // measured against the days that have happened.
        let today = day(2026, 10, 28);
        let s = vec![
            rec(2026, 10, 28, 50),
            rec(2026, 10, 27, 50),
            rec(2026, 10, 26, 50),
        ];
        let r = report_at(&s, Period::Month, 0, today, None, false);
        assert_eq!(r.days_in_period, 6);
        assert_eq!(r.active_days, 3);
        assert!((r.consistency - 0.5).abs() < 0.001);
    }

    #[test]
    fn past_period_uses_its_full_length() {
        let today = day(2026, 10, 28);
        let r = report_at(&[], Period::Month, -1, today, None, false);
        assert_eq!(
            r.days_in_period, 30,
            "a finished month is scored on all 30 days"
        );
    }

    #[test]
    fn comparison_against_previous_period() {
        let today = day(2026, 10, 28);
        let s = vec![
            rec(2026, 10, 28, 100), // this week
            rec(2026, 10, 20, 50),  // last week
        ];
        let r = report_at(&s, Period::Week, 0, today, None, false);
        assert_eq!(r.minutes, 100);
        assert_eq!(r.prev_minutes, 50);
        assert!(r.has_prev);
        assert!((r.delta_pct - 100.0).abs() < 0.001);
    }

    #[test]
    fn all_time_spans_from_first_session() {
        let today = day(2026, 10, 28);
        let s = vec![rec(2026, 9, 1, 25), rec(2026, 10, 28, 25)];
        let r = report_at(&s, Period::All, 0, today, Some(day(2026, 9, 1)), false);
        assert_eq!(r.sessions, 2);
        assert_eq!(r.minutes, 50);
    }

    #[test]
    fn empty_history_is_zero_and_does_not_panic() {
        let r = report_at(&[], Period::Week, 0, day(2026, 10, 28), None, false);
        assert_eq!(r.sessions, 0);
        assert_eq!(r.minutes, 0);
        assert_eq!(r.score, 0);
        assert!(!narrative(&r, false).is_empty());
        assert!(!narrative(&r, true).is_empty());
    }

    #[test]
    fn streak_counts_consecutive_days_back_from_today() {
        let today = day(2026, 10, 28);
        let s = vec![
            rec(2026, 10, 28, 25),
            rec(2026, 10, 27, 25),
            rec(2026, 10, 26, 25),
            rec(2026, 10, 20, 25),
        ];
        let r = report_at(&s, Period::Month, 0, today, None, false);
        assert_eq!(r.streak, 3);
        assert_eq!(r.longest_streak, 3);
    }

    #[test]
    fn streak_survives_a_day_that_has_not_started_yet() {
        // Nothing today, but yesterday and the day before: the streak is 2,
        // not 0. Not having begun today is not a broken streak.
        let today = day(2026, 10, 28);
        let s = vec![rec(2026, 10, 27, 25), rec(2026, 10, 26, 25)];
        let r = report_at(&s, Period::Week, 0, today, None, false);
        assert_eq!(r.streak, 2);
    }

    #[test]
    fn hours_are_bucketed_by_local_hour() {
        let today = Local::now().date_naive();
        let s = vec![SessionRecord {
            at: ts_for(today.year(), today.month(), today.day(), 9),
            minutes: 25,
        }];
        let r = report_at(&s, Period::Day, 0, today, None, false);
        assert_eq!(r.buckets.len(), 24);
        assert_eq!(r.buckets.iter().map(|b| b.minutes).sum::<u64>(), 25);
    }

    #[test]
    fn future_days_are_marked_in_a_running_week() {
        let today = day(2026, 10, 28); // Wednesday
        let r = report_at(&[], Period::Week, 0, today, None, false);
        let future: Vec<&Bucket> = r.buckets.iter().filter(|b| b.future).collect();
        assert_eq!(future.len(), 2, "Thursday and Friday are still ahead");
        assert!(r.buckets.iter().any(|b| b.current));
    }

    #[test]
    fn period_index_round_trips() {
        for p in [
            Period::Day,
            Period::Week,
            Period::Month,
            Period::Year,
            Period::All,
        ] {
            assert_eq!(Period::from_index(p.index()), p);
        }
    }
}
