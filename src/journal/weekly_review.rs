//! Weekly read-back: one Markdown document that makes a whole week face you.
//!
//! The journal, fulfillment, and stats each hold a part of what a week was —
//! decrees and inscriptions, plan fulfillment, best focus window — but they
//! live on separate screens and none of them reads back as a single document
//! you can open on Sunday and face honestly. This module is that document:
//! a pure assembly of the week's blocks into one readable Markdown file.
//!
//! The week is the project's week: **Saturday-first**, matching the Iranian
//! weekday convention used by `stats::report` and every other week-shaped
//! screen in the app (see `stats::report::week_index`). It deliberately does
//! *not* use ISO Monday weeks — that would disagree with every other surface
//! the user sees.
//!
//! Everything here is pure: takes `Data`-shaped slices and an explicit
//! `tz_offset_secs`, no I/O, no clock reads beyond what's passed in. The
//! Markdown string is returned to the caller, which decides where to write it.

use crate::journal::{Decree, Inscription, Verdict, VerdictKind};
use crate::state::store::SessionRecord;
use crate::stats::report::{
    best_focus_window, break_adherence, honest_hour_label, honesty_gap_label,
    hourly_verdict_summary, BreakAdherence, HourWindow, Range,
};
use chrono::{Datelike, NaiveDate};

/// One block's worth of the week, as it reads back.
#[derive(Clone, Debug)]
pub struct WeeklyBlock {
    /// Unix seconds when the session ended — the join key.
    pub session_at: i64,
    /// What the block was for, if a decree was written.
    pub decree: Option<String>,
    /// What it produced, if an inscription was written.
    pub inscription: Option<String>,
    /// The effective verdict (declared or inferred), if any.
    pub verdict: Option<VerdictKind>,
    /// True when the block landed its promise: a Kept/Extended verdict, or
    /// (unjudged) an inscription facing the decree.
    pub fulfilled: bool,
}

/// The week's summary lines.
#[derive(Clone, Debug, Default)]
pub struct WeeklySummary {
    /// Completed focus sessions in the week.
    pub sessions: u32,
    /// Total focus minutes in the week.
    pub focus_minutes: u64,
    /// Intentions written in the week.
    pub planned: usize,
    /// Intentions answered by an inscription.
    pub landed: usize,
    /// Spoken-but-undelivered promises (planned − landed).
    pub spoken_not_delivered: usize,
    /// Break adherence over the week, when there are enough sessions.
    pub break_adherence: Option<BreakAdherence>,
    /// Break adherence measured from *recorded* breaks (started_at/ended_at),
    /// when the week has enough completed breaks. This is the honest number:
    /// it does not guess from session gaps.
    pub recorded_break_adherence: Option<BreakAdherence>,
    /// The best 3-hour focus window, when there are enough sessions.
    pub best_window: Option<HourWindow>,
    /// Words that recur across the week's decrees, most frequent first.
    pub top_decree_tokens: Vec<(String, u32)>,
    /// Promises written repeatedly across the last 30 days and never kept
    /// (3+ occurrences, no Kept verdict).
    pub standing_promises: Vec<crate::journal::standing_promise::StandingPromise>,
    /// The hour the user was most honest this week, when enough sessions were
    /// judged. None until there is a qualifying sample.
    pub honest_hours: Option<crate::stats::report::HourlyVerdictSummary>,
    /// Blocks that ended this week with no verdict — promises dropped without
    /// being faced. Oldest first.
    pub unjudged_blocks: Vec<crate::journal::unjudged::UnjudgedBlock>,
}

/// The whole week, ready to render.
#[derive(Clone, Debug)]
pub struct WeeklyReview {
    /// Inclusive day range of the week.
    pub range: Range,
    /// The week's blocks, chronological.
    pub blocks: Vec<WeeklyBlock>,
    /// The week's summary lines.
    pub summary: WeeklySummary,
    /// Local UTC offset (seconds) used to bucket this week's datetimes.
    pub tz_offset_secs: i32,
}

/// The Saturday-first week containing `today`.
///
/// Mirrors `stats::report::bounds(Period::Week, 0, today, None)` so the
/// weekly review covers exactly the same week as the report screen. The
/// `tz_offset_secs` is threaded for signature symmetry with the rest of the
/// stats layer; the bounds themselves are date-based, not timestamp-based.
pub fn week_bounds(today: NaiveDate) -> Range {
    let base = today - chrono::Duration::days(crate::stats::report::week_index(today));
    Range {
        start: base,
        end: base + chrono::Duration::days(6),
    }
}

/// Assemble the week's read-back from the store's slices.
///
/// `tz_offset_secs` is the local UTC offset (e.g. +12600 for Iran) used to
/// bucket sessions into the week and to compute the best focus window.
/// `planned_break_secs` is the configured short-break length, used by the
/// break-adherence summary.
pub fn build_week(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
) -> WeeklyReview {
    let range = week_bounds(today);

    // Live lookups keyed by session_at.
    let decree_for: std::collections::HashMap<i64, &Decree> = decrees
        .iter()
        .filter(|d| !d.deleted)
        .map(|d| (d.session_at, d))
        .collect();
    let inscription_for: std::collections::HashMap<i64, &Inscription> = inscriptions
        .iter()
        .filter(|i| !i.deleted)
        .map(|i| (i.session_at, i))
        .collect();
    let verdict_for: std::collections::HashMap<i64, &Verdict> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| (v.session_at, v))
        .collect();

    // The week's sessions, chronological, bucketed by local date.
    let mut week_sessions: Vec<&SessionRecord> = Vec::new();
    for s in sessions {
        if let Some(d) = date_of(s.at, tz_offset_secs) {
            if range.contains(d) {
                week_sessions.push(s);
            }
        }
    }
    week_sessions.sort_by_key(|s| s.at);

    let mut blocks = Vec::with_capacity(week_sessions.len());
    let mut focus_minutes = 0u64;
    for s in &week_sessions {
        let dec = decree_for.get(&s.at).map(|d| d.text.clone());
        let ins = inscription_for.get(&s.at).map(|i| i.text.clone());
        let decl = verdict_for.get(&s.at).map(|v| v.kind);
        let effective = crate::journal::verdict::effective(
            decl,
            decree_for.get(&s.at).copied(),
            inscription_for.get(&s.at).copied(),
        );
        let fulfilled = match effective {
            Some(e) => e.kind.is_met(),
            None => dec.is_some() && ins.is_some(),
        };
        focus_minutes += s.minutes;
        blocks.push(WeeklyBlock {
            session_at: s.at,
            decree: dec,
            inscription: ins,
            verdict: decl,
            fulfilled,
        });
    }

    // Reuse the reckoning engine for the summary numbers (spoken/answered/…)
    // and the recurring-word list — it already implements the exact pairing
    // rules this document needs.
    let reckoning = crate::journal::reckoning::build(decrees, inscriptions, &[], range);
    let summary = WeeklySummary {
        sessions: week_sessions.len() as u32,
        focus_minutes,
        planned: reckoning.spoken,
        landed: reckoning.answered,
        spoken_not_delivered: reckoning.unanswered(),
        break_adherence: {
            let all: Vec<SessionRecord> = week_sessions.iter().map(|s| (**s).clone()).collect();
            break_adherence(&all, planned_break_secs, usize::MAX)
        },
        recorded_break_adherence: {
            let week_breaks: Vec<crate::timer::break_record::BreakRecord> = breaks
                .iter()
                .filter(|b| {
                    date_of(b.started_at, tz_offset_secs)
                        .map(|d| range.contains(d))
                        .unwrap_or(false)
                })
                .cloned()
                .collect();
            crate::timer::break_record::adherence_summary(&week_breaks)
        },
        best_window: best_focus_window(
            &week_sessions
                .iter()
                .map(|s| (**s).clone())
                .collect::<Vec<_>>(),
            tz_offset_secs,
        ),
        top_decree_tokens: reckoning
            .recurring
            .into_iter()
            .map(|r| (r.word, r.count))
            .collect(),
        standing_promises: {
            // The week's end (last local day, 23:59) is "now" for the
            // lookback window, so a promise written today is still counted.
            let end_of_week = chrono::NaiveDateTime::new(
                range.end,
                chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
            )
            .and_utc()
            .timestamp();
            let mut ctx = crate::state::store::Data::default();
            ctx.decrees = decrees.to_vec();
            ctx.verdicts = verdicts.to_vec();
            crate::journal::standing_promise::standing_promises(&ctx, end_of_week, 30 * 86_400, 3)
        },
        honest_hours: {
            let all: Vec<SessionRecord> = week_sessions.iter().map(|s| (**s).clone()).collect();
            let sum = hourly_verdict_summary(verdicts, &all, tz_offset_secs, 3);
            (sum.best_kept_hour.is_some()).then_some(sum)
        },
        unjudged_blocks: {
            use chrono::TimeZone;
            let offset = chrono::FixedOffset::east_opt(tz_offset_secs)
                .unwrap_or_else(|| chrono::FixedOffset::east(0));
            let start_utc = offset
                .from_local_datetime(&chrono::NaiveDateTime::new(
                    range.start,
                    chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
                ))
                .single()
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            let end_utc = offset
                .from_local_datetime(&chrono::NaiveDateTime::new(
                    range.end,
                    chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
                ))
                .single()
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            let now_end = end_utc.max(start_utc);
            crate::journal::unjudged::unjudged_sessions(
                sessions,
                verdicts,
                decrees,
                now_end,
                start_utc,
                end_utc,
                tz_offset_secs,
            )
        },
    };

    WeeklyReview {
        range,
        blocks,
        summary,
        tz_offset_secs,
    }
}

/// Render the week as a single Markdown document.
pub fn render_markdown(review: &WeeklyReview, locale_fa: bool) -> String {
    let fa = locale_fa;
    let mut out = String::new();

    let title = if fa {
        format!(
            "# بازخوانی هفتهٔ {}",
            crate::calendar::jalali::fa_num(review.range.start.year() as i64)
        )
    } else {
        format!("# Weekly read-back — week of {}", review.range.start)
    };
    out.push_str(&title);
    out.push_str("\n\n");

    // Summary block.
    out.push_str(&render_summary(review, fa));
    out.push('\n');

    // Blocks, grouped by local day.
    let mut current_day: Option<String> = None;
    for b in &review.blocks {
        let day = crate::journal::inscription::day_key(b.session_at);
        if current_day.as_deref() != Some(day.as_str()) {
            if current_day.is_some() {
                out.push('\n');
            }
            out.push_str(&format!("## {day}\n\n"));
            current_day = Some(day);
        }
        out.push_str(&render_block(b, fa));
    }

    // If there were no blocks at all, say so plainly rather than emitting an
    // empty bullet list.
    if review.blocks.is_empty() {
        out.push_str(if fa {
            "هیچ جلسه‌ای در این هفته ثبت نشده است.\n"
        } else {
            "No sessions were recorded this week.\n"
        });
    }

    out
}

fn render_summary(review: &WeeklyReview, fa: bool) -> String {
    let s = &review.summary;
    let mut out = String::new();
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };

    out.push_str(&format!(
        "{} **{}** · {} **{}**\n",
        if fa { "جلسه‌ها:" } else { "Sessions:" },
        num(s.sessions as i64),
        if fa {
            "دقیقهٔ تمرکز:"
        } else {
            "Focus minutes:"
        },
        num(s.focus_minutes as i64)
    ));
    out.push_str(&format!(
        "{} **{}** · {} **{}**\n",
        if fa { "وعده‌ها:" } else { "Planned:" },
        num(s.planned as i64),
        if fa {
            "انجام‌شده:"
        } else {
            "Landed:"
        },
        num(s.landed as i64)
    ));
    out.push_str(&format!(
        "{} **{}**\n",
        if fa {
            "گفته‌شده ولی انجام‌نشده:"
        } else {
            "Spoken, not delivered:"
        },
        num(s.spoken_not_delivered as i64)
    ));

    if let Some(b) = s.recorded_break_adherence.or(s.break_adherence) {
        out.push_str(&format!(
            "{} **{:.0}%** ({} / {} / {}){}\n",
            if fa {
                "پایبندی به استراحت:"
            } else {
                "Break adherence:"
            },
            b.rate * 100.0,
            b.followed,
            b.skipped,
            b.stretched,
            if s.recorded_break_adherence.is_some() {
                if fa {
                    " (ثبت‌شده)"
                } else {
                    " (recorded)"
                }
            } else {
                ""
            }
        ));
    }
    if !s.unjudged_blocks.is_empty() {
        let label = if fa {
            "بلوک‌های بدون داوری:"
        } else {
            "Unjudged blocks:"
        };
        out.push_str(&format!(
            "{} {}\n",
            label,
            crate::journal::unjudged::backlog_label(&s.unjudged_blocks, fa)
        ));
        // The three oldest unjudged blocks, called out explicitly.
        for b in s.unjudged_blocks.iter().take(3) {
            let date = date_of(b.session_at, review.tz_offset_secs)
                .map(|d| crate::stats::report::jalali_label(d, !fa, true))
                .unwrap_or_default();
            let spoke = if b.has_decree {
                if fa {
                    " (دارای فرمان)"
                } else {
                    " (had a decree)"
                }
            } else {
                ""
            };
            out.push_str(&format!(
                "  - {} · {} دقیقه{}{}\n",
                date,
                if fa {
                    crate::calendar::jalali::fa_num(b.minutes as i64)
                } else {
                    b.minutes.to_string()
                },
                spoke,
                if fa {
                    " — بی‌داوری"
                } else {
                    " — unjudged"
                }
            ));
        }
    }
    if let Some(win) = s.best_window {
        out.push_str(&format!(
            "{} **{:02}:00–{:02}:00** ({} {})\n",
            if fa {
                "بهترین بازهٔ تمرکز:"
            } else {
                "Best focus window:"
            },
            win.start_hour,
            win.end_hour,
            win.sessions,
            if fa { "جلسه" } else { "sessions" }
        ));
    }
    if !s.top_decree_tokens.is_empty() {
        let tokens: Vec<String> = s
            .top_decree_tokens
            .iter()
            .take(5)
            .map(|(w, c)| format!("{} ({})", w, num(*c as i64)))
            .collect();
        out.push_str(&format!(
            "{} {}\n",
            if fa {
                "واژه‌های تکرارشونده:"
            } else {
                "Recurring words:"
            },
            tokens.join(", ")
        ));
    }
    if crate::journal::standing_promise::has_stood_unmet(&s.standing_promises) {
        out.push_str(&format!(
            "{}\n",
            if fa {
                "وعده‌های برقرارمانده:"
            } else {
                "Standing promises:"
            }
        ));
        for p in s.standing_promises.iter().take(5) {
            out.push_str(&format!(
                "  - {} ({}×, {})\n",
                p.original_sample,
                num(p.count as i64),
                if fa {
                    "برقرار مانده"
                } else {
                    "unmet"
                }
            ));
        }
    }
    if let Some(h) = &s.honest_hours {
        let honest = honest_hour_label(h, fa);
        if !honest.is_empty() {
            out.push_str(&format!("{}\n", honest));
        }
        if let Some(gap) = honesty_gap_label(h, fa) {
            out.push_str(&format!("  {gap}\n"));
        }
    }
    out
}

fn render_block(b: &WeeklyBlock, fa: bool) -> String {
    let time = chrono::DateTime::from_timestamp(b.session_at, 0)
        .map(|u| u.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string());

    let mut out = String::new();
    out.push_str(&format!("- `{time}` "));

    if let Some(d) = &b.decree {
        out.push_str(&format!(
            "**{}:** {} ",
            if fa { "فرمان" } else { "decree" },
            d
        ));
    }
    match &b.inscription {
        Some(i) => out.push_str(&format!(" — {i}")),
        None => out.push_str(&format!(
            " — {}",
            if fa {
                "بی‌نوشته ماند"
            } else {
                "nothing written"
            }
        )),
    }
    if let Some(v) = b.verdict {
        let label = match (fa, v) {
            (true, VerdictKind::Kept) => "نگه‌داشته‌شده",
            (true, VerdictKind::Partial) => "بخشی",
            (true, VerdictKind::Broken) => "شکسته",
            (true, VerdictKind::Extended) => "گسترده",
            (true, VerdictKind::Short) => "کوتاه",
            (false, _) => v.as_str(),
        };
        out.push_str(&format!(" [{label}]"));
    } else if !b.fulfilled {
        out.push_str(&format!(
            " [{}]",
            if fa {
                "برآورده‌نشده"
            } else {
                "unfulfilled"
            }
        ));
    }
    out.push('\n');
    out
}

/// Local date of a Unix timestamp for the given offset.
fn date_of(ts: i64, tz_offset_secs: i32) -> Option<NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)?;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&offset).date_naive())
}

/// Write the current week's read-back to a file, once per week.
///
/// The file is named `weekly-<saturday>.md` in `pangan`'s data directory
/// (next to `data.json`), so it survives across launches. It is written only
/// when that week's file does not already exist — the week's review is a
/// fixed document for a fixed week, not something to rewrite on every launch.
/// Returns the path written, or `None` when the week's file already existed
/// (or nothing was written).
#[allow(clippy::too_many_arguments)] // one record's worth of inputs, in one call
pub fn write_if_new(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
    locale_fa: bool,
) -> Option<std::path::PathBuf> {
    let review = build_week(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    let dir = crate::state::store::Store::data_dir().join("weekly");
    let filename = format!("weekly-{}.md", review.range.start);
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let md = render_markdown(&review, locale_fa);
    std::fs::create_dir_all(&dir).ok()?;
    crate::state::atomic_file::atomic_write(&path, md.as_bytes()).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{decree, inscription, verdict, VerdictKind};

    /// 2026-10-31 is a Saturday (the start of an Iranian week).
    const SAT: i64 = 1_793_431_800;

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    fn today() -> NaiveDate {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()
    }

    #[test]
    fn week_bounds_starts_on_saturday() {
        let r = week_bounds(today());
        assert_eq!(r.start, today());
        assert_eq!(r.end, today() + chrono::Duration::days(6));
    }

    #[test]
    fn build_week_empty_returns_zero_summary() {
        let rev = build_week(&[], &[], &[], &[], &[], today(), 12600, 300);
        assert!(rev.blocks.is_empty());
        assert_eq!(rev.summary.sessions, 0);
        assert_eq!(rev.summary.focus_minutes, 0);
        assert_eq!(rev.summary.planned, 0);
        assert_eq!(rev.summary.spoken_not_delivered, 0);
    }

    #[test]
    fn build_week_pairs_decree_and_inscription_by_session_at() {
        let s = vec![sess(SAT, 25)];
        let d = vec![decree::create(SAT, SAT - 1500, "write the report").unwrap()];
        let i = vec![inscription::create(SAT, SAT, "wrote the report").unwrap()];
        let rev = build_week(&s, &d, &i, &[], &[], today(), 12600, 300);
        assert_eq!(rev.blocks.len(), 1);
        let b = &rev.blocks[0];
        assert_eq!(b.decree.as_deref(), Some("write the report"));
        assert_eq!(b.inscription.as_deref(), Some("wrote the report"));
        assert!(b.fulfilled);
        assert_eq!(rev.summary.planned, 1);
        assert_eq!(rev.summary.landed, 1);
        assert_eq!(rev.summary.spoken_not_delivered, 0);
    }

    #[test]
    fn build_week_counts_spoken_not_delivered() {
        let s = vec![sess(SAT, 25)];
        let d = vec![decree::create(SAT, SAT - 1500, "an unkept promise").unwrap()];
        let rev = build_week(&s, &d, &[], &[], &[], today(), 12600, 300);
        assert_eq!(rev.summary.planned, 1);
        assert_eq!(rev.summary.landed, 0);
        assert_eq!(rev.summary.spoken_not_delivered, 1);
        assert!(!rev.blocks[0].fulfilled);
    }

    #[test]
    fn build_week_excludes_sessions_outside_the_week() {
        // Sunday of the *following* week (SAT + 8 days) must be excluded.
        let outside = SAT + 8 * 86_400;
        let s = vec![sess(SAT, 25), sess(outside, 25)];
        let rev = build_week(&s, &[], &[], &[], &[], today(), 12600, 300);
        assert_eq!(rev.blocks.len(), 1);
        assert_eq!(rev.summary.sessions, 1);
    }

    #[test]
    fn render_markdown_persian_digits_when_locale_fa() {
        let s = vec![sess(SAT, 25)];
        let d = vec![decree::create(SAT, SAT - 1500, "نوشتن گزارش").unwrap()];
        let rev = build_week(&s, &d, &[], &[], &[], today(), 12600, 300);
        let md = render_markdown(&rev, true);
        assert!(
            md.contains("جلسه‌ها: **۱**"),
            "Persian digits used, got: {}",
            md
        );
        assert!(md.contains("فرمان"), "decree label in Persian");
        // No ASCII digit in the count line.
        assert!(!md.contains("جلسه‌ها: **1**"));
    }

    #[test]
    fn render_markdown_includes_spoken_not_delivered_count() {
        let s = vec![sess(SAT, 25)];
        let d = vec![decree::create(SAT, SAT - 1500, "an unkept promise").unwrap()];
        let rev = build_week(&s, &d, &[], &[], &[], today(), 12600, 300);
        let md = render_markdown(&rev, false);
        assert!(md.contains("Spoken, not delivered: **1**"));
    }

    #[test]
    fn render_markdown_is_valid_utf8_and_no_crlf() {
        let s = vec![sess(SAT, 25)];
        let rev = build_week(&s, &[], &[], &[], &[], today(), 12600, 300);
        let md = render_markdown(&rev, false);
        assert!(std::str::from_utf8(md.as_bytes()).is_ok());
        assert!(!md.contains("\r\n"), "must use LF line endings");
        assert!(!md.contains('\r'));
    }

    #[test]
    fn render_markdown_includes_honest_hours_when_judged() {
        // Three sessions with a Kept verdict in the same local hour → an
        // honest hour emerges and shows in the markdown.
        let s = vec![sess(SAT, 25), sess(SAT + 60, 25), sess(SAT + 120, 25)];
        let v = vec![
            verdict::declare(SAT, VerdictKind::Kept, SAT).unwrap(),
            verdict::declare(SAT + 60, VerdictKind::Kept, SAT + 60).unwrap(),
            verdict::declare(SAT + 120, VerdictKind::Kept, SAT + 120).unwrap(),
        ];
        let rev = build_week(&s, &[], &[], &v, &[], today(), 12600, 300);
        assert!(rev.summary.honest_hours.is_some());
        let md = render_markdown(&rev, false);
        assert!(
            md.contains("Most honest hour"),
            "markdown should surface honest hours: {md}"
        );
    }

    #[test]
    fn render_markdown_omits_honest_hours_without_verdicts() {
        let s = vec![sess(SAT, 25)];
        let rev = build_week(&s, &[], &[], &[], &[], today(), 12600, 300);
        assert!(rev.summary.honest_hours.is_none());
        let md = render_markdown(&rev, false);
        assert!(!md.contains("Most honest hour"));
    }

    #[test]
    fn build_week_records_break_adherence_from_real_breaks() {
        use crate::timer::break_record::{BreakRecord, BreakRel};
        let mk = |started, ended, planned, kind| BreakRecord {
            started_at: started,
            ended_at: Some(ended),
            planned_secs: planned,
            kind,
        };
        // Three followed breaks within the week: start at SAT, SAT+1d, SAT+2d.
        let s = vec![
            sess(SAT, 25),
            sess(SAT + 86_400, 25),
            sess(SAT + 2 * 86_400, 25),
        ];
        let breaks = vec![
            mk(SAT + 100, SAT + 400, 300, BreakRel::Short),
            mk(SAT + 86_400 + 100, SAT + 86_400 + 400, 300, BreakRel::Short),
            mk(
                SAT + 2 * 86_400 + 100,
                SAT + 2 * 86_400 + 400,
                300,
                BreakRel::Short,
            ),
        ];
        let rev = build_week(&s, &[], &[], &[], &breaks, today(), 12600, 300);
        let rec = rev.summary.recorded_break_adherence.unwrap();
        assert_eq!(rec.followed, 3);
        let md = render_markdown(&rev, false);
        assert!(md.contains("(recorded)"), "recorded adherence is labelled");
    }

    #[test]
    fn build_week_surfaces_unjudged_blocks_with_decree_flag() {
        // Two sessions in the week: one judged, one not (but spoken for).
        let s = vec![sess(SAT, 25), sess(SAT + 60, 25)];
        let verdicts = vec![verdict::Verdict {
            session_at: SAT,
            kind: crate::journal::VerdictKind::Kept,
            declared_at: SAT,
            deleted: false,
        }];
        let decrees = vec![crate::journal::decree::create(SAT + 60, SAT + 50, "plan").unwrap()];
        let rev = build_week(&s, &decrees, &[], &verdicts, &[], today(), 12600, 300);
        assert_eq!(rev.summary.unjudged_blocks.len(), 1);
        assert_eq!(rev.summary.unjudged_blocks[0].session_at, SAT + 60);
        assert!(rev.summary.unjudged_blocks[0].has_decree);
        let md = render_markdown(&rev, false);
        assert!(md.contains("Unjudged blocks:"), "section header appears");
        assert!(md.contains("had a decree"), "spoken-for block is flagged");
    }
}
