//! The daily briefing: the coach's line, moved from weekly to daily.
//!
//! The weekly digest speaks once a week, after the week is over. But the user
//! acts every day, and the gap between "the digest said shorten your blocks"
//! and "today's first block" is where the advice goes cold. This module
//! composes a short page on first launch each day — *before* the day's work —
//! so the record's prescriptions and analytics are present at the moment of
//! planning, not after the fact.
//!
//! It deliberately contains **no new analytics**. Every number here is already
//! computed elsewhere: today's rota (`rota_for_day`), the recent unjudged
//! backlog (`unjudged_sessions`), last week's kept-rate (`build_week` on the
//! previous week), the active prescription (the newest live ledger row), and
//! the calibrated block length (`duration_calibration`). The briefing is a
//! *reader* of the record, not a writer of new claims — a fresh install
//! therefore renders a page that says "no record yet" rather than inventing
//! numbers. That is the discipline the whole journal holds to: no fabricated
//! precision.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use crate::journal::prescription_ledger::Prescription;
use crate::journal::unjudged::UnjudgedBlock;
use crate::journal::{Decree, Inscription, Verdict};
use crate::state::rota::RotaEntry;
use crate::state::store::SessionRecord;
use crate::stats::report::DurationCalibration;
use chrono::NaiveDate;

/// The number of trailing days of unjudged blocks the briefing surfaces.
/// A full week of backlog is enough to see the pattern without burying the
/// page in rows a fresh user cannot act on.
pub const UNJUDGED_LOOKBACK_DAYS: i64 = 7;

/// One day's briefing — everything the app can honestly say about today
/// before today happens.
#[derive(Clone, Debug)]
pub struct DailyBriefing {
    /// The local day this briefing is for.
    pub day: NaiveDate,
    /// The blocks planned for today (the rota's pending rows), in order.
    pub today_rota: Vec<RotaEntry>,
    /// Sessions that ended in the last [`UNJUDGED_LOOKBACK_DAYS`] days with no
    /// verdict — promises dropped without being faced. Oldest first.
    pub recent_unjudged: Vec<UnjudgedBlock>,
    /// Intention rows in *last* week (the week before the one containing
    /// `day`). `last_week_landed` is how many of them were answered.
    pub last_week_planned: usize,
    /// Intentions in last week that were answered by an inscription.
    pub last_week_landed: usize,
    /// The newest live prescription in the ledger — the one piece of coaching
    /// that is most recent. `None` until the ledger has a row.
    pub active_prescription: Option<Prescription>,
    /// Block-length calibration across the whole record, so the briefing can
    /// name the length to start with.
    pub calibration: DurationCalibration,
}

/// The most recent live prescription in the ledger, if any.
pub fn active_prescription(ledger: &[Prescription]) -> Option<Prescription> {
    ledger
        .iter()
        .filter(|p| !p.deleted)
        .max_by_key(|p| p.created_at)
        .cloned()
}

/// Compose a day's briefing from the store's slices.
///
/// Pure: reads only what it is given, computes nothing new beyond the existing
/// analytics, and renders a page that is honest for an empty record too.
#[allow(clippy::too_many_arguments)] // one record's worth of inputs, in one call
pub fn compose_daily(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    rota: &[RotaEntry],
    prescriptions: &[Prescription],
    day: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> DailyBriefing {
    // Today's rota: the pending rows planned for this exact day.
    let day_s = day.format("%Y-%m-%d").to_string();
    let today_rota = rota
        .iter()
        .filter(|r| r.day == day_s)
        .cloned()
        .collect::<Vec<_>>();

    // Recent unjudged blocks: the trailing lookback window ending today. The
    // "now" for the unjudged scan is the end of today, so `days_old` is
    // correct relative to the actual present.
    let recent_unjudged = crate::journal::unjudged::unjudged_sessions(
        sessions,
        verdicts,
        decrees,
        // now = end of today (UTC)
        day_end_utc(day, tz_offset_secs),
        window_start_utc(day, UNJUDGED_LOOKBACK_DAYS, tz_offset_secs),
        day_end_utc(day, tz_offset_secs),
        tz_offset_secs,
    );

    // Last week's kept-rate: build the *previous* calendar week's review.
    // `week_bounds` is Saturday-first, so subtracting seven days returns the
    // prior week's bounds.
    let last_week = crate::journal::weekly_review::build_week(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        day - chrono::Duration::days(7),
        tz_offset_secs,
        planned_break_secs,
    );

    let calibration = crate::stats::report::duration_calibration(sessions, verdicts);

    DailyBriefing {
        day,
        today_rota,
        recent_unjudged,
        last_week_planned: last_week.summary.planned,
        last_week_landed: last_week.summary.landed,
        active_prescription: active_prescription(prescriptions),
        calibration,
    }
}

/// The UTC instant at the end of `day` (23:59:59 local) under `offset`.
fn day_end_utc(day: NaiveDate, tz_offset_secs: i32) -> i64 {
    use chrono::TimeZone;
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
    offset
        .from_local_datetime(&chrono::NaiveDateTime::new(
            day,
            chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        ))
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// The UTC instant at the start of `day - lookback_days` (00:00:00 local).
fn window_start_utc(day: NaiveDate, lookback_days: i64, tz_offset_secs: i32) -> i64 {
    use chrono::TimeZone;
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
    offset
        .from_local_datetime(&chrono::NaiveDateTime::new(
            day - chrono::Duration::days(lookback_days),
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        ))
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// A date label for the locale — Persian date in Farsi, Gregorian otherwise.
fn date_label(d: NaiveDate, fa: bool) -> String {
    crate::stats::report::jalali_label(d, !fa, true)
}

/// Render the briefing as a single Markdown document.
pub fn render_markdown(briefing: &DailyBriefing, locale_fa: bool) -> String {
    let fa = locale_fa;
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    let mut out = String::new();

    out.push_str(&format!(
        "{} **{}**\n\n",
        if fa {
            "# گزارش روزانه — "
        } else {
            "# Daily briefing — "
        },
        date_label(briefing.day, fa),
    ));

    // 1. Today's rota — the plan for the day, the moment it matters.
    out.push_str(if fa {
        "## برنامهٔ امروز\n\n"
    } else {
        "## Today's rota\n\n"
    });
    if briefing.today_rota.is_empty() {
        out.push_str(if fa {
            "امروز هیچ بلوکی برنامه‌ریزی نشده است. قبل از شروع روز، فهرست کوتاهی بنویس.\n\n"
        } else {
            "No blocks are planned for today. Write a short list before the day starts.\n\n"
        });
    } else {
        for (i, e) in briefing.today_rota.iter().enumerate() {
            out.push_str(&format!(
                "  {}. {} — {} دقیقه\n",
                num(i as i64 + 1),
                e.label,
                num(e.minutes as i64),
            ));
        }
        out.push('\n');
    }

    // 2. Recent unjudged blocks — promises dropped without facing them.
    if !briefing.recent_unjudged.is_empty() {
        out.push_str(if fa {
            "## بلوک‌های بی‌داوری اخیر\n\n"
        } else {
            "## Recent unjudged blocks\n\n"
        });
        out.push_str(&format!(
            "{} {}\n\n",
            if fa {
                "این بلوک‌ها بدون داوری به پایان رسیدند:"
            } else {
                "These blocks ended without being faced:"
            },
            crate::journal::unjudged::backlog_label(&briefing.recent_unjudged, fa),
        ));
        for b in briefing.recent_unjudged.iter().take(8) {
            out.push_str(&format!("  - {} دقیقه\n", num(b.minutes as i64)));
        }
        out.push('\n');
    }

    // 3. Last week's kept-rate — the one honest number about how the plan
    // actually went.
    if briefing.last_week_planned > 0 {
        let rate = briefing.last_week_landed as f32 / briefing.last_week_planned as f32;
        out.push_str(if fa {
            "## هفتهٔ گذشته\n\n"
        } else {
            "## Last week\n\n"
        });
        if fa {
            out.push_str(&format!(
                "از **{}** نیت، **{}** به سرانجام رسید ({}٪).\n\n",
                num(briefing.last_week_planned as i64),
                num(briefing.last_week_landed as i64),
                num((rate * 100.0).round() as i64),
            ));
        } else {
            out.push_str(&format!(
                "**{}** of **{}** intentions came through ({}%).\n\n",
                num(briefing.last_week_landed as i64),
                num(briefing.last_week_planned as i64),
                num((rate * 100.0).round() as i64),
            ));
        }
    }

    // 4. The active prescription — the most recent piece of coaching.
    if let Some(p) = &briefing.active_prescription {
        out.push_str(if fa {
            "## نصیحت فعال\n\n"
        } else {
            "## Active prescription\n\n"
        });
        out.push_str(&format!("  - {}\n\n", p.text));
    }

    // 5. The calibrated block length — the length to start with today.
    let cal_text = crate::stats::report::calibration_label(&briefing.calibration, fa);
    out.push_str(if fa {
        "## طول پیشنهادی بلوک\n\n"
    } else {
        "## Suggested block length\n\n"
    });
    out.push_str(&cal_text);
    out.push_str("\n\n");

    out
}

/// Write today's briefing to a file, once per day.
///
/// Follows the same once-per-period convention as the weekly digest: a fixed
/// file per day (`briefing/<local-date>.md`), written only when it does not
/// exist. This is the scheduled hook that delivers the coach's line at the
/// moment of planning.
#[allow(clippy::too_many_arguments)] // one record's worth of inputs, in one call
pub fn write_if_new(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    rota: &[RotaEntry],
    prescriptions: &[Prescription],
    day: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
    locale_fa: bool,
) -> Option<std::path::PathBuf> {
    let briefing = compose_daily(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        rota,
        prescriptions,
        day,
        tz_offset_secs,
        planned_break_secs,
    );
    let dir = crate::state::store::Store::data_dir().join("briefing");
    let filename = format!("{}.md", day.format("%Y-%m-%d"));
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let md = render_markdown(&briefing, locale_fa);
    std::fs::create_dir_all(&dir).ok()?;
    crate::state::atomic_file::atomic_write(&path, md.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::VerdictKind;
    use crate::state::store::SessionRecord;

    const TZ: i32 = 12600; // Iran UTC+3:30
    /// 2026-10-31 09:00:00 +0330.
    const T: i64 = 1_793_431_800;

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    /// An empty record must render a briefing that claims nothing it cannot
    /// back — no fabricated kept-rate, no invented rota, no invented advice.
    #[test]
    fn test_daily_briefing_empty_record_renders_no_fabricated_claims() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let b = compose_daily(&[], &[], &[], &[], &[], &[], &[], today, TZ, 5 * 60);
        let md = render_markdown(&b, false);

        assert_eq!(b.last_week_planned, 0, "empty record: no plans claimed");
        assert_eq!(b.last_week_landed, 0, "empty record: no landings claimed");
        assert!(b.today_rota.is_empty(), "empty record: no rota claimed");
        assert!(
            b.recent_unjudged.is_empty(),
            "empty record: no unjudged claimed"
        );
        assert!(
            b.active_prescription.is_none(),
            "empty record: no advice claimed"
        );
        assert!(
            md.contains("No blocks are planned"),
            "honest empty-rota line"
        );
        assert!(
            md.contains("Not enough data to calibrate"),
            "calibration stays silent when there is no record"
        );
        // Must not fabricate a kept-rate line.
        assert!(!md.contains("Last week\n"), "no last-week rate when empty");
    }

    #[test]
    fn test_daily_briefing_lists_today_rota_pending_only() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let today_s = "2026-10-31".to_string();
        let tomorrow_s = "2026-11-01".to_string();
        let entry =
            |id: u64, day: String, label: &str, minutes: u64| crate::state::rota::RotaEntry {
                id,
                day,
                label: label.to_string(),
                task_id: None,
                decree: String::new(),
                minutes,
                created_at: T,
                opened_at: 0,
                session_at: 0,
                outcome: None,
                deleted: false,
            };
        let rota = vec![
            entry(1, today_s.clone(), "write report", 25),
            entry(2, today_s, "second block", 50),
            entry(3, tomorrow_s, "tomorrow's plan", 25),
        ];
        let b = compose_daily(&[], &[], &[], &[], &[], &rota, &[], today, TZ, 5 * 60);
        assert_eq!(b.today_rota.len(), 2, "only today's rows");
        let md = render_markdown(&b, false);
        assert!(md.contains("write report"), "today's first block listed");
        assert!(md.contains("second block"), "today's second block listed");
        assert!(
            !md.contains("tomorrow's plan"),
            "a row for another day is not today's plan"
        );
    }

    #[test]
    fn test_daily_briefing_surfaces_recent_unjudged() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        // A session two days ago (in the lookback) with no verdict.
        let old_at = T - 2 * 86_400;
        let sessions = vec![sess(old_at, 25)];
        let b = compose_daily(&sessions, &[], &[], &[], &[], &[], &[], today, TZ, 5 * 60);
        assert_eq!(
            b.recent_unjudged.len(),
            1,
            "the unjudged session is surfaced"
        );
        let md = render_markdown(&b, false);
        assert!(
            md.contains("unjudged"),
            "the backlog is named in the briefing"
        );
    }

    #[test]
    fn test_daily_briefing_last_week_kept_rate() {
        use chrono::TimeZone;
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        // The *previous* Saturday-first week, computed with the same helper the
        // app uses, so the test is correct whatever weekday `today` is.
        let last_week_range =
            crate::journal::weekly_review::week_bounds(today - chrono::Duration::days(7));
        // A session mid-last-week.
        let mid = last_week_range.start + chrono::Duration::days(3);
        let mid_local =
            chrono::NaiveDateTime::new(mid, chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap());
        let offset = chrono::FixedOffset::east_opt(TZ).unwrap();
        let last_week_at = offset
            .from_local_datetime(&mid_local)
            .single()
            .unwrap()
            .timestamp();

        let session = sess(last_week_at, 25);
        let decree =
            crate::journal::decree::create(last_week_at, last_week_at - 60, "plan the day")
                .unwrap();
        let inscription =
            crate::journal::inscription::create(last_week_at, last_week_at, "planned the day")
                .unwrap();
        let verdict = crate::journal::Verdict {
            session_at: last_week_at,
            kind: VerdictKind::Kept,
            declared_at: last_week_at,
            deleted: false,
        };

        let sessions = vec![session];
        let b = compose_daily(
            &sessions,
            &[decree],
            &[inscription],
            &[verdict],
            &[],
            &[],
            &[],
            today,
            TZ,
            5 * 60,
        );
        assert_eq!(b.last_week_planned, 1, "the planned promise is counted");
        assert_eq!(b.last_week_landed, 1, "the kept promise is counted");
        let md = render_markdown(&b, false);
        assert!(md.contains("Last week\n"), "the kept-rate section appears");
        assert!(
            last_week_range.contains(mid),
            "test fixture lands in the previous week"
        );
    }

    #[test]
    fn test_daily_briefing_surfaces_active_prescription() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let old = Prescription {
            id: 1,
            week: "2026-10-18".to_string(),
            created_at: T - 86_400,
            kind: crate::journal::prescription::SuggestionKind::Calibration,
            target_metric: Some("kept_rate".to_string()),
            target_value: 0.5,
            text: "older advice".to_string(),
            sample_n: 10,
            deleted: false,
        };
        let new = Prescription {
            id: 2,
            week: "2026-10-25".to_string(),
            created_at: T,
            kind: crate::journal::prescription::SuggestionKind::Calibration,
            target_metric: Some("kept_rate".to_string()),
            target_value: 0.6,
            text: "newest advice".to_string(),
            sample_n: 12,
            deleted: false,
        };
        let b = compose_daily(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[old.clone(), new.clone()],
            today,
            TZ,
            5 * 60,
        );
        let act = b.active_prescription.as_ref().unwrap();
        assert_eq!(act.id, 2, "the newest live prescription is surfaced");
        let md = render_markdown(&b, false);
        assert!(
            md.contains("newest advice"),
            "the newest advice is rendered"
        );
        assert!(!md.contains("older advice"), "older advice is not surfaced");
    }
}
