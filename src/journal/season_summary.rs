//! Season summary: the read-back of a whole Jalali season, at a glance.
//!
//! The weekly digest rises a week. But the user works in seasons — a Jalali
//! season is three months, and a year's arc is four of them. A week is too
//! short to see whether a habit is actually forming; a season is long enough
//! to show the trend under the noise. This module is that longer view: a
//! single Markdown document that faces one season the way the weekly digest
//! faces one week.
//!
//! A season is measured in Jalali months (the app's calendar): months 1–3
//! are spring, 4–6 summer, 7–9 autumn, and 10–12 winter. The season
//! containing "today" is computed from the Gregorian date via the existing
//! calendar arithmetic; the span is returned as a [`Range`] so the session
//! bucketing matches every other screen.
//!
//! The summary reuses the already-tested aggregation engines rather than
//! reimplementing them:
//!
//! * [`verdict::tally`] for the verdict shape (kept/partial/broken/extended/
//!   short, declared vs inferred);
//! * [`reckoning::build`] for planned vs landed (the promise ledger);
//! * [`unjudged_count`] for the untouched backlog.
//!
//! Everything here is pure: slices and a date in, owned data out, no I/O,
//! no clock reads beyond what is passed in.

use crate::journal::{Decree, Inscription, Verdict};
use crate::state::store::SessionRecord;
use crate::stats::report::Range;
use chrono::{Datelike, NaiveDate};

/// The four Jalali seasons, in order, English and Persian.
pub const SEASONS_EN: [&str; 4] = ["Spring", "Summer", "Autumn", "Winter"];
pub const SEASONS_FA: [&str; 4] = ["بهار", "تابستان", "پاییز", "زمستان"];

/// Which Jalali month opens each season (months are 1‑indexed).
fn season_start_month(season: usize) -> u32 {
    (season as u32) * 3 + 1
}

/// The season (0..4) a Jalali month falls in.
fn season_of_month(month: u32) -> usize {
    ((month - 1) / 3) as usize
}

/// The season containing `today`, plus the Jalali year it belongs to.
#[derive(Clone, Debug)]
pub struct SeasonRef {
    /// The Gregorian day span of the season.
    pub range: Range,
    /// The Jalali year (e.g. 1405).
    pub jalali_year: i32,
    /// 0 = spring, 1 = summer, 2 = autumn, 3 = winter.
    pub season: usize,
}

/// Compute the season containing `today` using Jalali arithmetic.
///
/// The season's Gregorian boundaries are derived from the Jalali year and
/// season-opening month: start = the 1st of the opening month, end = the last
/// day of the closing month (via `month_len`).
pub fn season_of(today: NaiveDate) -> SeasonRef {
    let jy = crate::calendar::jalali::from_gregorian(today.year(), today.month(), today.day());
    let (year, month, _day) = (jy.0, jy.1, jy.2);
    let season = season_of_month(month);
    let start_month = season_start_month(season);
    let end_month = start_month + 2;
    let (sy, sm, sd) = crate::calendar::jalali::to_gregorian(year, start_month, 1);
    let (ey, em, ed) = crate::calendar::jalali::to_gregorian(
        year,
        end_month,
        crate::calendar::jalali::month_len(year, end_month),
    );
    let start = NaiveDate::from_ymd_opt(sy, sm, sd).unwrap_or(today);
    let end = NaiveDate::from_ymd_opt(ey, em, ed).unwrap_or(today);
    SeasonRef {
        range: Range { start, end },
        jalali_year: year,
        season,
    }
}

/// One season's worth of the record, as it reads back.
#[derive(Clone, Debug)]
pub struct SeasonSummary {
    /// The season this describes.
    pub season: SeasonRef,
    /// Completed focus sessions in the season.
    pub sessions: u32,
    /// Total focus minutes in the season.
    pub focus_minutes: u64,
    /// The verdict tally (kept/partial/…), declared and inferred.
    pub tally: crate::journal::verdict::Tally,
    /// Promises written in the season.
    pub planned: usize,
    /// Promises answered by an inscription.
    pub landed: usize,
    /// Blocks left unjudged (promises dropped without being faced).
    pub unjudged: u32,
}

/// Assemble the season's summary from the store's slices.
///
/// `tz_offset_secs` is the local UTC offset, used to bucket sessions by local
/// date into the season's range. Sessions are counted by their local date
/// falling inside the season span, matching every other screen.
pub fn summarize(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    today: NaiveDate,
    tz_offset_secs: i32,
) -> SeasonSummary {
    let season = season_of(today);
    let range = season.range;

    // Bucket sessions by local date into the season.
    let mut season_sessions: Vec<&SessionRecord> = Vec::new();
    for s in sessions {
        if let Some(d) = date_of(s.at, tz_offset_secs) {
            if range.contains(d) {
                season_sessions.push(s);
            }
        }
    }
    let focus_minutes = season_sessions.iter().map(|s| s.minutes).sum();

    let tally = crate::journal::verdict::tally(decrees, inscriptions, verdicts, range);
    let reckoning = crate::journal::reckoning::build(decrees, inscriptions, &[], range);

    // Unjudged blocks this season: sessions in the season with no live verdict.
    let judged: std::collections::HashSet<i64> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| v.session_at)
        .collect();
    let unjudged = season_sessions
        .iter()
        .filter(|s| !judged.contains(&s.at))
        .count() as u32;

    SeasonSummary {
        season,
        sessions: season_sessions.len() as u32,
        focus_minutes,
        tally,
        planned: reckoning.spoken,
        landed: reckoning.answered,
        unjudged: unjudged.min(season_sessions.len() as u32),
    }
}

/// Local date of a UTC timestamp, or `None` on a bad offset.
fn date_of(ts: i64, tz_offset_secs: i32) -> Option<NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)?;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&offset).date_naive())
}

/// Render the season as a single Markdown document.
pub fn render_season(s: &SeasonSummary, fa: bool) -> String {
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    let pct = |n: i64| {
        if fa {
            format!("{}٪", num(n))
        } else {
            format!("{}%", num(n))
        }
    };

    let season_name = if fa {
        SEASONS_FA[s.season.season]
    } else {
        SEASONS_EN[s.season.season]
    };
    let year = s.season.jalali_year;
    let mut out = String::new();
    out.push_str(&format!(
        "{} {} {} {}\n\n",
        if fa { "# فصل" } else { "# Season" },
        season_name,
        if fa { "سال" } else { "of" },
        num(year as i64),
    ));

    // Headline: the shape of the season, if there were any judged blocks.
    if s.sessions == 0 {
        out.push_str(if fa {
            "در این فصل هیچ جلسه‌ای ثبت نشده است.\n\n"
        } else {
            "No focus sessions were recorded this season.\n\n"
        });
        return out;
    }

    let met = s.tally.met();
    let total = s.tally.total();
    let kept_rate = if total > 0 {
        pct((met as f64 / total as f64 * 100.0).round() as i64)
    } else {
        pct(0)
    };

    out.push_str(&format!(
        "{} **{}** · {} **{}** · {} **{}**\n\n",
        if fa { "جلسه" } else { "Sessions" },
        num(s.sessions as i64),
        if fa {
            "دقیقه تمرکز"
        } else {
            "Focus minutes"
        },
        num(s.focus_minutes as i64),
        if fa { "نگه‌داری" } else { "Kept rate" },
        kept_rate,
    ));

    if s.planned > 0 {
        out.push_str(&format!(
            "{} **{}** / **{}**\n\n",
            if fa { "وعده‌ها" } else { "Promises" },
            num(s.landed as i64),
            num(s.planned as i64),
        ));
    }

    if s.unjudged > 0 {
        out.push_str(&format!(
            "{} **{}**\n\n",
            if fa {
                "بلوک بدون داوری"
            } else {
                "Unjudged blocks"
            },
            num(s.unjudged as i64),
        ));
    }

    out
}

/// Write the current season's summary to a file, once per season.
///
/// Follows the same convention as the weekly digest: a fixed file per season,
/// written only when it doesn't exist. Returns the path written, or `None`
/// when the season's file already existed.
pub fn write_season_if_new(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    today: NaiveDate,
    tz_offset_secs: i32,
    locale_fa: bool,
) -> Option<std::path::PathBuf> {
    let s = summarize(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        today,
        tz_offset_secs,
    );
    let dir = crate::state::store::Store::data_dir().join("seasons");
    let filename = format!("season-{}-{}.md", s.season.jalali_year, s.season.season + 1);
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let md = render_season(&s, locale_fa);
    std::fs::create_dir_all(&dir).ok()?;
    crate::state::atomic_file::atomic_write(&path, md.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-31 is a Saturday in Jalali month 8 (Aban) — summer (months 4–6
    /// are summer, but Aban is month 8 → autumn). Let me pin the dates exactly:
    /// 2026-10-31 falls in Aban 1405 (month 8) → autumn.
    const SAT: i64 = 1_793_431_800;

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    #[test]
    fn season_of_maps_jalali_months() {
        // 2026-03-21 is 1 Farvardin 1405 → spring (season 0).
        let spring = season_of(NaiveDate::from_ymd_opt(2026, 3, 21).unwrap());
        assert_eq!(spring.season, 0);
        assert_eq!(spring.jalali_year, 1405);
        // Farvardin 1 is the season's first day.
        assert_eq!(
            spring.range.start,
            NaiveDate::from_ymd_opt(2026, 3, 21).unwrap()
        );

        // 2026-06-22 is early Tir (month 4) → summer (season 1).
        let summer = season_of(NaiveDate::from_ymd_opt(2026, 6, 22).unwrap());
        assert_eq!(summer.season, 1);

        // 2026-12-22 is Dey (month 10) → winter (season 3).
        let winter = season_of(NaiveDate::from_ymd_opt(2026, 12, 22).unwrap());
        assert_eq!(winter.season, 3);
    }

    #[test]
    fn season_span_is_three_months() {
        let spring = season_of(NaiveDate::from_ymd_opt(2026, 4, 10).unwrap());
        // Spring = Farvardin, Ordibehesht, Khordad → ends 31 Khordad 1405.
        // Khordad 31 1405 = 2026-06-21.
        let end = spring.range.end;
        assert_eq!(end, NaiveDate::from_ymd_opt(2026, 6, 21).unwrap());
        assert_eq!(spring.range.days(), 31 + 31 + 31);
    }

    #[test]
    fn summarize_counts_only_season_sessions() {
        // One session inside spring (2026-04-10), one far outside (last
        // autumn: 2026-10-31).
        let inside = sess(1_775_779_200, 25); // 2026-04-10 UTC
        let outside = sess(SAT, 50); // 2026-10-31 UTC
        let s = summarize(
            &[inside, outside],
            &[],
            &[],
            &[],
            NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            12600,
        );
        assert_eq!(s.sessions, 1);
        assert_eq!(s.focus_minutes, 25);
    }

    #[test]
    fn render_season_english_and_persian() {
        let v = crate::journal::verdict::declare(
            1_775_779_200,
            crate::journal::VerdictKind::Kept,
            1_775_779_200,
        )
        .unwrap();
        let s = summarize(
            &[sess(1_775_779_200, 25)],
            &[],
            &[],
            &[v],
            NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            12600,
        );
        let en = render_season(&s, false);
        assert!(en.contains("# Season"));
        assert!(en.contains("Sessions"));
        assert!(en.contains("Kept rate"));
        let fa = render_season(&s, true);
        assert!(fa.contains("# فصل"));
        assert!(fa.contains("جلسه"));
    }

    #[test]
    fn empty_season_stays_silent() {
        let s = summarize(
            &[],
            &[],
            &[],
            &[],
            NaiveDate::from_ymd_opt(2026, 4, 10).unwrap(),
            12600,
        );
        let en = render_season(&s, false);
        assert!(en.contains("No focus sessions"));
    }
}
