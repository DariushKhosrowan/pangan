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

use crate::journal::promise_quality::PromiseQuality;
use crate::journal::{Decree, Inscription, Verdict, VerdictKind};
use crate::state::store::SessionRecord;
use crate::stats::bounce::{bounce_stats, BounceStats};
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

/// The week's full digest: the read-back plus every diagnostic that reads the
/// same store.
///
/// The `WeeklyReview` is the week's narrative; the rest are the analytics that
/// have no home of their own yet. The digest is what puts them all on one
/// page, so the user can face the whole week — not just the blocks.
#[derive(Clone, Debug)]
pub struct WeeklyDigest {
    /// The week's read-back (blocks, summary lines).
    pub review: WeeklyReview,
    /// Plan autopsy: what became of every planned block in the week.
    pub autopsy: Vec<crate::state::plan_autopsy::AutopsyRow>,
    /// Duration calibration across the whole record (not just the week — a
    /// week rarely carries enough rated sessions to calibrate).
    pub calibration: crate::stats::report::DurationCalibration,
    /// The most common obstructions in the week, most frequent first.
    pub top_obstructions: Vec<(String, u32)>,
    /// Week-over-week drift headline: which metric moved the most vs the
    /// trailing four-week average, and whether that is an improvement.
    pub drift: Option<crate::journal::week_drift::WeekDrift>,
    /// Suggestions for next week (empty until the record is mature enough).
    pub suggestions: Vec<crate::journal::prescription::Suggestion>,
    /// Feedback on past advice: how each ledged prescription fared against the
    /// metric it targeted. Three buckets — resolved, stubborn, reopened.
    pub outcomes: crate::journal::prescription_outcome::OutcomeBuckets,
    /// Break quality across the whole record: breaks bucketed by actual length,
    /// with the kept-rate of the block that follows each length. Empty until
    /// enough rated observations accumulate.
    pub break_quality: crate::timer::break_quality::BreakQuality,
    /// Bounce rate across the trailing window: how fast the user returns to a
    /// kept block after a bad day. None until a recovery is measurable.
    pub bounce: Option<BounceStats>,
    /// Promise quality: whether quantified decrees are kept at a different
    /// rate than vague ones, and by how much. None until a judgment exists.
    pub promise_quality: Option<PromiseQuality>,
    /// Plan realism ceiling: the largest daily rota size the user reliably
    /// finishes, compared against the size they typically planned this week.
    /// None until the record holds enough history to judge.
    pub realism: Option<crate::journal::realism::RealismInfo>,
    /// Purpose drift: whether blocks did what their decrees promised, across
    /// the whole record. None until enough paired blocks exist to judge.
    pub purpose_drift: Option<crate::journal::purpose_drift::DriftStat>,
    /// The local hour with the worst purpose drift (if any hour qualifies).
    pub purpose_drift_hour: Option<(u8, crate::journal::purpose_drift::DriftStat)>,
    /// The duration bucket with the worst purpose drift (if any qualifies).
    pub purpose_drift_duration: Option<(&'static str, crate::journal::purpose_drift::DriftStat)>,
    /// Which prescription kind actually works for this user, as a localized
    /// one-line summary. None until a kind has enough outcomes to judge.
    pub kind_effectiveness_label: Option<String>,
    /// Cross-metric patterns: the strongest leading/following metric pairs,
    /// as localized one-line labels. Empty until enough weeks exist to trust
    /// a correlation.
    pub patterns: Vec<String>,
}

/// Compose the week's full digest from the store's slices.
///
/// This is the single entry point that calls every pure analytics function in
/// the codebase: the weekly review (`build_week`), the plan autopsy
/// (`autopsy`), duration calibration (`duration_calibration`), recorded break
/// adherence (inside `build_week`), unjudged blocks (inside `build_week`),
/// and the top obstructions (`top_obstructions`). It exists so the diagnostics
/// that were built and never wired reach the user in one artifact.
#[allow(clippy::too_many_arguments)] // one record's worth of inputs, in one call
pub fn compose_weekly(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    obstructions: &[crate::journal::obstruction::Obstruction],
    rota: &[crate::state::rota::RotaEntry],
    prescriptions: &[crate::journal::prescription_ledger::Prescription],
    prescription_verdicts: &[crate::journal::prescription_verdict::PrescriptionVerdict],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
    locale_fa: bool,
) -> WeeklyDigest {
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
    let from = review.range.start.format("%Y-%m-%d").to_string();
    let to = review.range.end.format("%Y-%m-%d").to_string();
    let today_s = today.format("%Y-%m-%d").to_string();
    let autopsy =
        crate::state::plan_autopsy::autopsy(rota, verdicts, obstructions, &from, &to, &today_s);
    let calibration = crate::stats::report::duration_calibration(sessions, verdicts);
    // Obstructions are keyed by timestamp; use the same UTC bucket as the week.
    let (start_utc, end_utc) = week_utc_bounds(review.range, tz_offset_secs);
    let top = crate::journal::obstruction::top_obstructions(obstructions, start_utc, end_utc, 5);
    let drift = crate::journal::week_drift::compute_drift(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    // The prescription reads pre-computed analytics: the calibration, the
    // recorded break adherence, the honest hour (with its sample), the
    // unjudged backlog, and the drift (with its baseline size).
    let (drift_result, baseline_sessions) = crate::journal::week_drift::analyze_weeks(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    let honest_hour = review
        .summary
        .honest_hours
        .map(|h| h.best_kept_hour)
        .flatten();
    let honest_hour_samples = review
        .summary
        .honest_hours
        .map(|h| h.best_kept_total)
        .unwrap_or(0);
    let input = crate::journal::prescription::PrescriptionInput {
        calibration: &calibration,
        break_adherence: review.summary.recorded_break_adherence.as_ref().or(
            match review.summary.break_adherence.as_ref() {
                Some(b) => Some(b),
                None => None,
            },
        ),
        honest_hour,
        honest_hour_samples,
        unjudged: review.summary.unjudged_blocks.len() as u32,
        drift: drift_result.as_ref(),
        drift_baseline_sessions: baseline_sessions,
    };
    let suggestions = crate::journal::prescription::prescribe(&input, locale_fa);
    // Feedback on past advice: measure each stored prescription's target
    // metric at this week's value and classify resolved/stubborn/reopened.
    // Only prescriptions with a target metric and a live value are measured.
    let current_metrics = crate::journal::week_drift::week_metrics(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    let classified = crate::journal::prescription_outcome::classify_all(prescriptions, &|metric| {
        crate::journal::week_drift::value_of(&current_metrics, metric)
    });
    let outcomes = crate::journal::prescription_outcome::bucketed(classified.clone());
    // Prescription kind effectiveness: which levers actually work for this
    // user. Computed from the same classified outcomes, grouped by kind.
    let kind_effectiveness = crate::journal::prescription_effectiveness::kind_effectiveness(
        &classified,
        crate::journal::prescription_effectiveness::MIN_SAMPLES,
    );
    let kind_effectiveness_label = crate::journal::prescription_effectiveness::effectiveness_label(
        &kind_effectiveness,
        locale_fa,
    );
    // Make the coach learn: filter and escalate raw suggestions against the
    // outcome buckets, so resolved advice isn't re-nagged and stubborn advice
    // escalates to a warning instead of silently repeating.
    let now_week = review.range.start.format("%Y-%m-%d").to_string();
    let suggestions = crate::journal::prescription_policy::apply(
        suggestions,
        &outcomes,
        prescriptions,
        prescription_verdicts,
        &now_week,
    );
    let break_quality = crate::timer::break_quality::break_quality(breaks, sessions, verdicts);
    let bounce = bounce_stats(sessions, verdicts, today, tz_offset_secs);
    let promise_quality = crate::journal::promise_quality::promise_quality(decrees, verdicts);
    // Plan realism ceiling: the largest daily plan size the user reliably
    // finishes, compared against the size they typically wrote this week. The
    // week's typical size is the planned blocks over the days that had plans —
    // what they actually put down, not a guessed ideal.
    let historical = crate::journal::realism::historical_day_tallies(rota, &today_s);
    let mut week_days_with_plans: std::collections::BTreeSet<&str> =
        std::collections::BTreeSet::new();
    for r in &autopsy {
        week_days_with_plans.insert(r.day.as_str());
    }
    let week_typical = if week_days_with_plans.is_empty() {
        0
    } else {
        (autopsy.len() as u32) / (week_days_with_plans.len() as u32)
    };
    let realism = crate::journal::realism::realism_ceiling(
        &historical,
        crate::journal::realism::DEFAULT_MIN_WEEKS,
    )
    .map(|ceiling| crate::journal::realism::RealismInfo {
        ceiling,
        current_plan_size: week_typical,
    });
    // Purpose drift: the record's own texts, compared. None until the record
    // holds enough paired blocks for an honest rate.
    let purpose_drift = crate::journal::purpose_drift::drift_overall(
        decrees,
        inscriptions,
        crate::journal::purpose_drift::DEFAULT_THRESHOLD,
        crate::journal::purpose_drift::MIN_SAMPLES,
    );
    let purpose_drift_hour = crate::journal::purpose_drift::worst_hour_drift(
        decrees,
        inscriptions,
        tz_offset_secs,
        crate::journal::purpose_drift::DEFAULT_THRESHOLD,
    );
    let purpose_drift_duration = crate::journal::purpose_drift::worst_duration_drift(
        decrees,
        inscriptions,
        crate::journal::purpose_drift::DEFAULT_THRESHOLD,
    );
    // Cross-metric patterns: the strongest leading/following metric pairs
    // across the trailing weeks. Empty until enough paired weeks exist.
    let weeks = crate::journal::week_drift::gather_weeks(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        today,
        tz_offset_secs,
        planned_break_secs,
    );
    let series = crate::stats::patterns::all_series(&weeks);
    let patterns = crate::stats::patterns::find_patterns(&series, 0.6, 3)
        .into_iter()
        .map(|p| crate::stats::patterns::pattern_label(&p, locale_fa))
        .collect::<Vec<String>>();
    WeeklyDigest {
        review,
        autopsy,
        calibration,
        top_obstructions: top,
        drift,
        suggestions,
        outcomes,
        break_quality,
        bounce,
        promise_quality,
        realism,
        purpose_drift,
        purpose_drift_hour,
        purpose_drift_duration,
        kind_effectiveness_label,
        patterns,
    }
}

/// The week's [start, end] instant in UTC seconds under the given offset.
pub(crate) fn week_utc_bounds(range: Range, tz_offset_secs: i32) -> (i64, i64) {
    use chrono::TimeZone;
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)
        .unwrap_or_else(|| chrono::FixedOffset::east_opt(0).unwrap());
    let start = offset
        .from_local_datetime(&chrono::NaiveDateTime::new(
            range.start,
            chrono::NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
        ))
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or(0);
    let end = offset
        .from_local_datetime(&chrono::NaiveDateTime::new(
            range.end,
            chrono::NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
        ))
        .single()
        .map(|dt| dt.timestamp())
        .unwrap_or(0);
    (start, end.max(start))
}

/// Render the digest as a single Markdown document.
/// Format a metric value for the outcome line: rates as whole percentages,
/// counts as integers (with Persian digits when in Farsi).
fn fmt_outcome_value(metric: &str, value: f32, fa: bool) -> String {
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    let fmt = |n: i64| {
        if fa {
            format!("{}٪", num(n))
        } else {
            format!("{}%", num(n))
        }
    };
    match metric {
        "kept_rate" | "fulfilment_rate" | "break_adherence" => fmt((value * 100.0).round() as i64),
        "sessions" | "focus_minutes" | "unjudged" => num(value as i64),
        _ => num(value as i64),
    }
}

pub fn render_digest(digest: &WeeklyDigest, locale_fa: bool) -> String {
    let fa = locale_fa;
    let mut out = String::new();

    let title = if fa {
        format!(
            "# خلاصهٔ هفتهٔ {}\n\n",
            crate::calendar::jalali::fa_num(digest.review.range.start.year() as i64)
        )
    } else {
        format!(
            "# Weekly digest — week of {}\n\n",
            digest.review.range.start
        )
    };
    out.push_str(&title);

    // 0. The headline finding, if there is one. This is the one line a
    // long-term user actually wants: "am I getting better or worse, and
    // where?"
    if let Some(d) = &digest.drift {
        out.push_str("> ");
        out.push_str(&crate::journal::week_drift::headline_label(d, fa));
        out.push_str("\n\n");
    }

    // 1. The read-back (blocks + summary).
    out.push_str(&render_markdown(&digest.review, fa));
    out.push('\n');

    // 1.5 Purpose drift — whether blocks did what their decrees promised. The
    // record's own texts, compared. Only present once enough paired blocks
    // exist to judge honestly.
    if let Some(pd) = &digest.purpose_drift {
        out.push_str(if fa {
            "## انحراف از قصد\n\n"
        } else {
            "## Purpose drift\n\n"
        });
        out.push_str(&crate::journal::purpose_drift::purpose_drift_label(pd, fa));
        out.push('\n');
        if let Some((hour, _)) = &digest.purpose_drift_hour {
            let h = *hour;
            out.push_str(&format!(
                "  - {}: {}",
                if fa { "ساعت" } else { "hour" },
                if fa {
                    crate::calendar::jalali::fa_num(h as i64)
                } else {
                    h.to_string()
                },
            ));
            out.push('\n');
        }
        if let Some((label, _)) = &digest.purpose_drift_duration {
            out.push_str(&format!(
                "  - {}: {}",
                if fa {
                    "طول بلوک"
                } else {
                    "block length"
                },
                label,
            ));
            out.push('\n');
        }
        out.push('\n');
    }

    // 1.6 Cross-metric patterns — which metric leads, and which follows, as
    // the record thickens. Empty until enough paired weeks exist to trust a
    // correlation.
    if !digest.patterns.is_empty() {
        out.push_str(if fa {
            "## همبستگی‌ها\n\n"
        } else {
            "## Patterns\n\n"
        });
        for p in &digest.patterns {
            out.push_str(&format!("  - {}\n", p));
        }
        out.push('\n');
    }

    // 2. Plan autopsy: what became of what was planned.
    let (landed, abandoned, unreached) = crate::state::plan_autopsy::by_outcome(&digest.autopsy);
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    out.push_str(if fa {
        "## کالبدشکافی برنامه‌ها\n\n"
    } else {
        "## Plan autopsy\n\n"
    });
    out.push_str(&format!(
        "{} **{}** · {} **{}** · {} **{}**\n\n",
        if fa { "فرود" } else { "Landed" },
        num(landed as i64),
        if fa { "رها" } else { "Abandoned" },
        num(abandoned as i64),
        if fa { "نرسیده" } else { "Unreached" },
        num(unreached as i64),
    ));
    // The abandoned and unreached blocks, called out explicitly.
    let missed: Vec<&crate::state::plan_autopsy::AutopsyRow> = digest
        .autopsy
        .iter()
        .filter(|r| r.outcome != crate::state::plan_autopsy::AutopsyOutcome::Landed)
        .collect();
    if missed.is_empty() {
        out.push_str(if fa {
            "هیچ برنامه‌ای ناقص نمانده است.\n\n"
        } else {
            "No planned block went unfulfilled.\n\n"
        });
    } else {
        for r in missed.iter().take(8) {
            let parsed = chrono::NaiveDate::parse_from_str(&r.day, "%Y-%m-%d");
            let date = match parsed {
                Ok(d) => crate::stats::report::jalali_label(d, !fa, true),
                Err(_) => r.day.clone(),
            };
            let label = match r.outcome {
                crate::state::plan_autopsy::AutopsyOutcome::Abandoned => {
                    if fa {
                        "رها شده"
                    } else {
                        "abandoned"
                    }
                }
                _ => {
                    if fa {
                        "نرسیده"
                    } else {
                        "unreached"
                    }
                }
            };
            out.push_str(&format!("  - {} · {} — {}\n", date, r.label, label,));
        }
        out.push('\n');
    }

    // 3. Plan realism ceiling — the largest daily plan size the user reliably
    // finishes, compared against what they typically wrote this week. Only
    // present once the record has enough history to judge honestly.
    if let Some(r) = &digest.realism {
        out.push_str(if fa {
            "## سقف واقع‌بینانهٔ برنامه\n\n"
        } else {
            "## Plan realism ceiling\n\n"
        });
        out.push_str(&crate::journal::realism::realism_label(
            Some(r.ceiling),
            r.current_plan_size,
            fa,
        ));
        out.push_str("\n\n");
    }

    // 4. Duration calibration (lifetime).
    out.push_str(if fa {
        "## کالیبراسیون طول\n\n"
    } else {
        "## Duration calibration\n\n"
    });
    out.push_str(&crate::stats::report::calibration_label(
        &digest.calibration,
        fa,
    ));
    out.push_str("\n\n");

    // 4. Top obstructions.
    if !digest.top_obstructions.is_empty() {
        out.push_str(if fa {
            "## موانع پرتکرار\n\n"
        } else {
            "## Most common obstructions\n\n"
        });
        for (text, count) in digest.top_obstructions.iter().take(5) {
            out.push_str(&format!(
                "  - {} ({}×)\n",
                text,
                crate::calendar::jalali::fa_num(*count as i64),
            ));
        }
        out.push('\n');
    }

    // 5. Suggestions for next week — the coach's line. Only present when the
    // record is mature enough that a suggestion is honest.
    if !digest.suggestions.is_empty() {
        out.push_str(if fa {
            "## پیشنهادها برای هفتهٔ آینده\n\n"
        } else {
            "## Suggestions for next week\n\n"
        });
        for s in &digest.suggestions {
            out.push_str(&format!("  - {}\n", s.text));
        }
        out.push('\n');
    }

    // 6. Prescription outcomes — feedback on past advice. This is the loop
    // that closes: did last week's coaching move the needle? Three buckets:
    // resolved (it worked), stubborn (repeated with no movement — surfaced as
    // a warning), reopened (a formerly-improved metric regressed).
    let o = &digest.outcomes;
    let has = !o.resolved.is_empty() || !o.stubborn.is_empty() || !o.reopened.is_empty();
    if has {
        out.push_str(if fa {
            "## سرانجام پیشنهادها\n\n"
        } else {
            "## Prescription outcomes\n\n"
        });
        for (bucket, heading) in [
            (&o.resolved, if fa { "✅ مؤثر" } else { "✅ Resolved" }),
            (
                &o.stubborn,
                if fa {
                    "⚠️ مقاوم"
                } else {
                    "⚠️ Stubborn"
                },
            ),
            (
                &o.reopened,
                if fa {
                    "🔄 بازگشته"
                } else {
                    "🔄 Reopened"
                },
            ),
        ] {
            if bucket.is_empty() {
                continue;
            }
            out.push_str(&format!("**{}**\n", heading));
            for oc in bucket.iter().take(5) {
                let metric = oc.prescription.target_metric.as_deref().unwrap_or("");
                let from = oc.prescription.target_value;
                let to = oc.current_value;
                out.push_str(&format!(
                    "  - {}: {} → {}\n",
                    metric,
                    fmt_outcome_value(metric, from, fa),
                    fmt_outcome_value(metric, to, fa),
                ));
            }
            out.push('\n');
        }
        // The meta-line: which lever actually works for this user.
        if let Some(kl) = &digest.kind_effectiveness_label {
            out.push_str(kl);
            out.push_str("\n\n");
        }
    }

    // 7. Break quality — the break-side calibration. Are breaks helping or
    // hurting? Each actual break length is bucketed and paired with the
    // kept-rate of the block that follows it. Silent until a bucket clears the
    // sample floor; never fabricates a correlation from thin data.
    let bq = &digest.break_quality;
    let has_rate = bq.buckets.iter().any(|b| b.kept_rate.is_some());
    if has_rate {
        out.push_str(if fa {
            "## کیفیت استراحت\n\n"
        } else {
            "## Break quality\n\n"
        });
        for b in &bq.buckets {
            if let Some(rate) = b.kept_rate {
                let pct = (rate * 100.0).round() as i64;
                out.push_str(&format!(
                    "  - {} — {}: {}٪ ({} استراحت)\n",
                    b.label,
                    if fa {
                        "بلوک بعدی"
                    } else {
                        "next block"
                    },
                    num(pct),
                    num(b.breaks as i64),
                ));
            }
        }
        let obs_line = if fa {
            format!(
                "> بر پایهٔ {} مشاهدهٔ داوری‌شده.\n\n",
                num(bq.total_rated as i64)
            )
        } else {
            format!(
                "> Based on {} rated observations.\n\n",
                num(bq.total_rated as i64)
            )
        };
        out.push_str(&obs_line);
        if let Some(best) = bq.best_break_length {
            let line = if fa {
                format!("> بهترین: استراحت‌های {} دقیقه‌ای بهترن.\n\n", best)
            } else {
                format!(
                    "> Best: breaks in the {} range lead to the most kept blocks.\n\n",
                    best
                )
            };
            out.push_str(&line);
        }
        out.push('\n');
    }

    // 8. Bounce rate — how fast the user returns to a kept block after a bad
    // day. A bad day is a local day with only Broken/Short sessions; bounce is
    // the number of days until the next good day (≥1 Kept). Only present once
    // at least one recovery is measurable — never fabricated from thin data.
    if let Some(b) = &digest.bounce {
        out.push_str(if fa {
            "## نرخ بازگشت\n\n"
        } else {
            "## Bounce rate\n\n"
        });
        if let (Some(m), Some(p)) = (b.median, b.p75) {
            let line = if fa {
                format!(
                    "> بازگشت میانه: {} روز · چارک سوم: {} روز.\n",
                    num(m as i64),
                    num(p as i64)
                )
            } else {
                format!(
                    "> Median bounce: {} days · 75th percentile: {} days.\n",
                    m, p
                )
            };
            out.push_str(&line);
        }
        if let Some(label) = b.band_label(fa) {
            let line = if fa {
                format!("> وضعیت: {}.\n", label)
            } else {
                format!("> Band: {}.\n", label)
            };
            out.push_str(&line);
        }
        let obs = if fa {
            format!(
                "> بر پایهٔ {} روز بد قابل‌اندازه‌گیری.\n\n",
                num(b.bounces.len() as i64)
            )
        } else {
            format!("> Based on {} measurable bad days.\n\n", b.bounces.len())
        };
        out.push_str(&obs);
        out.push('\n');
    }

    // 9. Promise quality — does how you phrase a promise predict whether the
    // block lands? Quantified decrees (one that names a number) compared with
    // vague ones, with lift as the headline. Only present once both groups
    // have at least one judged decree.
    if let Some(pq) = &digest.promise_quality {
        out.push_str(if fa {
            "## کیفیت عهد\n\n"
        } else {
            "## Promise quality\n\n"
        });
        let pct = |r: f32| (r * 100.0).round() as i64;
        let fmt_group = |g: &crate::journal::promise_quality::PromiseGroup| match g.kept_rate() {
            Some(r) => format!(
                "{}٪ ({} از {})",
                num(pct(r)),
                num(g.kept as i64),
                num(g.decrees as i64)
            ),
            None => "—".to_string(),
        };
        let q_line = if fa {
            format!(
                "> کمی: {} · غیرکمی: {}.\n",
                fmt_group(&pq.quantified),
                fmt_group(&pq.unquantified)
            )
        } else {
            format!(
                "> Quantified: {} · Vague: {}.\n",
                fmt_group(&pq.quantified),
                fmt_group(&pq.unquantified)
            )
        };
        out.push_str(&q_line);
        if let Some(lift) = pq.lift() {
            let signed = lift * 100.0;
            let line = if fa {
                if signed >= 0.0 {
                    format!(
                        "> وعده‌های کمی {} درصد بیشتر نگه‌داشته می‌شوند.\n\n",
                        num(signed.round() as i64)
                    )
                } else {
                    format!(
                        "> وعده‌های کمی {} درصد کمتر نگه‌داشته می‌شوند.\n\n",
                        num((-signed).round() as i64)
                    )
                }
            } else if signed >= 0.0 {
                format!(
                    "> Quantified promises are kept {}% more often.\n\n",
                    signed.round() as i64
                )
            } else {
                format!(
                    "> Quantified promises are kept {}% less often.\n\n",
                    (-signed).round() as i64
                )
            };
            out.push_str(&line);
        } else {
            out.push('\n');
        }
    }

    out
}

/// Write the week's digest to a file, once per week.
///
/// Reuses the same convention as [`write_if_new`]: a fixed file per week,
/// written only when it doesn't exist. This is the scheduled hook that finally
/// delivers the diagnostics to the user.
#[allow(clippy::too_many_arguments)] // one record's worth of inputs, in one call
pub fn write_digest_if_new(
    sessions: &[SessionRecord],
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    breaks: &[crate::timer::break_record::BreakRecord],
    obstructions: &[crate::journal::obstruction::Obstruction],
    rota: &[crate::state::rota::RotaEntry],
    prescriptions: &[crate::journal::prescription_ledger::Prescription],
    prescription_verdicts: &[crate::journal::prescription_verdict::PrescriptionVerdict],
    today: NaiveDate,
    tz_offset_secs: i32,
    planned_break_secs: u32,
    locale_fa: bool,
) -> Option<std::path::PathBuf> {
    let digest = compose_weekly(
        sessions,
        decrees,
        inscriptions,
        verdicts,
        breaks,
        obstructions,
        rota,
        prescriptions,
        prescription_verdicts,
        today,
        tz_offset_secs,
        planned_break_secs,
        locale_fa,
    );
    let dir = crate::state::store::Store::data_dir().join("weekly");
    let filename = format!("digest-{}.md", digest.review.range.start);
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let md = render_digest(&digest, locale_fa);
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

    /// The digest renders a Break quality section once enough rated break→next-
    /// block observations accumulate, and stays silent below the sample floor.
    #[test]
    fn digest_renders_break_quality_when_sampled() {
        use crate::journal::VerdictKind;
        use crate::timer::break_record::{BreakRecord, BreakRel};

        // 6 five-minute breaks, each followed by a kept block.
        let mut breaks = Vec::new();
        let mut sessions: Vec<SessionRecord> = Vec::new();
        let mut verdicts = Vec::new();
        let mut t = SAT - 14 * 86_400; // well before this week
        for _ in 0..6 {
            let b_end = t + 300;
            let s_at = b_end + 1200;
            breaks.push(BreakRecord {
                started_at: t,
                ended_at: Some(b_end),
                planned_secs: 300,
                kind: BreakRel::Short,
            });
            sessions.push(sess(s_at, 25));
            verdicts.push(crate::journal::verdict::declare(s_at, VerdictKind::Kept, s_at).unwrap());
            t = s_at + 2000;
        }

        let d = compose_weekly(
            &sessions,
            &[],
            &[],
            &verdicts,
            &breaks,
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        // The whole record only has these six rated break observations, so a
        // bucket clears the floor and the section renders.
        assert!(
            d.break_quality
                .buckets
                .iter()
                .any(|b| b.kept_rate.is_some()),
            "a bucket reports a rate"
        );
        let md = render_digest(&d, false);
        assert!(md.contains("## Break quality"), "the section renders");
        assert!(
            md.contains("Based on 6 rated observations"),
            "observations noted"
        );
        assert!(
            md.contains("> Best: breaks in the 5–9 range"),
            "the best break length is called out"
        );
        // Farsi rendering uses Persian digits but the same structure.
        let md_fa = render_digest(&d, true);
        assert!(md_fa.contains("## کیفیت استراحت"), "Farsi section renders");
        assert!(md_fa.contains("۶"), "Farsi uses Persian digits");

        // Any empty record stays silent — no fabricated correlation.
        let d0 = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(
            !render_digest(&d0, false).contains("## Break quality"),
            "empty record renders no break quality"
        );
    }

    #[test]
    fn digest_renders_bounce_when_recovery_is_measurable() {
        use crate::journal::VerdictKind;
        // A bad day (only a Broken session) followed by a good day the next
        // day (a Kept session) → one measurable bounce of 1 day.
        let mut sessions = Vec::new();
        let mut verdicts = Vec::new();
        let bad = today() - chrono::Duration::days(6); // within the 56-day window
        let good = bad + chrono::Duration::days(1);
        let at_local = |d: NaiveDate| {
            use chrono::TimeZone;
            let off = chrono::FixedOffset::east_opt(12600).unwrap();
            off.from_local_datetime(&chrono::NaiveDateTime::new(
                d,
                chrono::NaiveTime::from_hms_opt(0, 30, 0).unwrap(),
            ))
            .single()
            .unwrap()
            .timestamp()
        };
        let bad_ts = at_local(bad);
        let good_ts = at_local(good);
        sessions.push(sess(bad_ts, 25));
        sessions.push(sess(good_ts, 25));
        verdicts
            .push(crate::journal::verdict::declare(bad_ts, VerdictKind::Broken, bad_ts).unwrap());
        verdicts
            .push(crate::journal::verdict::declare(good_ts, VerdictKind::Kept, good_ts).unwrap());

        let d = compose_weekly(
            &sessions,
            &[],
            &[],
            &verdicts,
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(
            d.bounce.is_some(),
            "a measurable recovery produces bounce stats"
        );
        let md = render_digest(&d, false);
        assert!(md.contains("## Bounce rate"), "the section renders");
        assert!(md.contains("Median bounce: 1 days"), "median reported");
        assert!(md.contains("Band: resilient."), "band labelled");
        assert!(
            md.contains("Based on 1 measurable bad days."),
            "floor noted"
        );

        // Farsi renders the same structure with Persian digit.
        let md_fa = render_digest(&d, true);
        assert!(md_fa.contains("## نرخ بازگشت"), "Farsi section renders");
        assert!(md_fa.contains("۱"), "Farsi uses Persian digits");

        // A record with no recovery stays silent — no fabricated bounce.
        let d0 = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(
            !render_digest(&d0, false).contains("## Bounce rate"),
            "empty record renders no bounce"
        );
    }

    #[test]
    fn digest_renders_promise_quality_when_both_groups_judged() {
        use crate::journal::VerdictKind;
        // Quantified: "write 500 words" kept. Vague: "think" broken.
        let mut sessions = Vec::new();
        let mut decrees = Vec::new();
        let mut verdicts = Vec::new();
        for (i, (text, kind)) in [
            ("write 500 words", VerdictKind::Kept),
            ("think", VerdictKind::Broken),
        ]
        .into_iter()
        .enumerate()
        {
            let at = SAT - i as i64 * 86_400;
            sessions.push(sess(at, 25));
            decrees.push(crate::journal::decree::create(at, at - 60, text).unwrap());
            verdicts.push(crate::journal::verdict::declare(at, kind, at).unwrap());
        }

        let d = compose_weekly(
            &sessions,
            &decrees,
            &[],
            &verdicts,
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(
            d.promise_quality.is_some(),
            "both groups judged → promise quality stats"
        );
        let md = render_digest(&d, false);
        assert!(md.contains("## Promise quality"), "the section renders");
        assert!(
            md.contains("Quantified: 100٪"),
            "quantified rate reported with Persian percent sign"
        );
        assert!(
            md.contains("Quantified promises are kept 100% more often."),
            "lift headline reported"
        );

        // Empty record stays silent.
        let d0 = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(
            !render_digest(&d0, false).contains("## Promise quality"),
            "empty record renders no promise quality"
        );
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

    // --- Weekly dig digest ---

    #[test]
    fn compose_weekly_empty_week_is_safe() {
        let d = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(d.review.blocks.is_empty());
        assert!(d.autopsy.is_empty());
        assert!(d.top_obstructions.is_empty());
        // Empty week still renders without panicking.
        let md = render_digest(&d, false);
        assert!(md.contains("Weekly digest"), "title renders: {md}");
        assert!(md.contains("Plan autopsy"), "autopsy section renders");
        assert!(
            md.contains("Duration calibration"),
            "calibration section renders"
        );
        assert!(md.contains("No planned block went unfulfilled"));
    }

    #[test]
    fn compose_weekly_includes_autopsy_counts() {
        // One planned block in the week that never produced a session → unreached.
        let rota = vec![crate::state::rota::RotaEntry {
            id: 1,
            day: today().format("%Y-%m-%d").to_string(),
            label: "work".to_string(),
            task_id: None,
            decree: String::new(),
            minutes: 25,
            created_at: SAT,
            opened_at: 0,
            session_at: 0,
            outcome: None,
            deleted: false,
        }];
        let d = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &rota,
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        let (landed, abandoned, unreached) = crate::state::plan_autopsy::by_outcome(&d.autopsy);
        assert_eq!((landed, abandoned, unreached), (0, 0, 1));
        let md = render_digest(&d, false);
        assert!(md.contains("Unreached **1**"), "unreached count: {md}");
        assert!(md.contains("work"), "the missed block label appears");
    }

    #[test]
    fn digest_renders_realism_ceiling_when_history_is_mature() {
        // Three distinct past weeks, each with days planned at size 3 and fully
        // kept, so the ceiling is 3 and the section renders.
        let mut rota = Vec::new();
        let mut id = 1u64;
        let mut new = |day: &str| {
            let e = crate::state::rota::RotaEntry {
                id,
                day: day.to_string(),
                label: format!("block {id}"),
                task_id: None,
                decree: String::new(),
                minutes: 25,
                created_at: SAT + id as i64,
                opened_at: 1, // opened, so it can be claimed as done
                session_at: SAT + id as i64 + 60,
                outcome: Some("done".to_string()),
                deleted: false,
            };
            id += 1;
            e
        };
        for d in ["2026-10-03", "2026-10-04", "2026-10-05"] {
            rota.push(new(d));
            rota.push(new(d));
            rota.push(new(d));
        }
        for d in ["2026-10-10", "2026-10-11", "2026-10-12"] {
            rota.push(new(d));
            rota.push(new(d));
            rota.push(new(d));
        }
        for d in ["2026-10-17", "2026-10-18", "2026-10-19"] {
            rota.push(new(d));
            rota.push(new(d));
            rota.push(new(d));
        }
        let d = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &rota,
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert_eq!(
            d.realism.as_ref().map(|r| r.ceiling),
            Some(3),
            "the ceiling should be 3"
        );
        let md = render_digest(&d, false);
        assert!(
            md.contains("## Plan realism ceiling"),
            "section renders: {md}"
        );
        assert!(md.contains("3 blocks a day"), "ceiling named: {md}");
    }

    #[test]
    fn compose_weekly_includes_calibration_and_obstructions() {
        // Enough rated sessions to calibrate (5+ in one bucket).
        let sessions: Vec<SessionRecord> = (0..6).map(|i| sess(SAT + i * 60, 25)).collect();
        let verdicts: Vec<crate::journal::Verdict> = sessions
            .iter()
            .map(|s| verdict::declare(s.at, crate::journal::VerdictKind::Kept, s.at).unwrap())
            .collect();
        let obstructions = vec![crate::journal::obstruction::Obstruction {
            session_at: SAT,
            text: "no time".to_string(),
            created_at: SAT,
            deleted: false,
        }];
        let d = compose_weekly(
            &sessions,
            &[],
            &[],
            &verdicts,
            &[],
            &obstructions,
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(d.calibration.sweet_spot.is_some() || d.calibration.total_rated >= 5);
        assert_eq!(d.top_obstructions[0].0, "no time");
        let md = render_digest(&d, false);
        assert!(
            md.contains("Stable across all lengths")
                || md.contains("honest ceiling")
                || md.contains("sweet")
                || md.contains("One block length")
        );
        assert!(
            md.contains("Most common obstructions"),
            "obstruction section: {md}"
        );
        assert!(md.contains("no time"));
    }

    #[test]
    fn render_digest_persian_digits_when_locale_fa() {
        let d = compose_weekly(
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            true,
        );
        let md = render_digest(&d, true);
        assert!(md.contains("خلاصهٔ هفتهٔ"), "Persian title: {md}");
        assert!(md.contains("کالبدشکافی"), "Persian autopsy header");
        assert!(md.contains("کالیبراسیون"), "Persian calibration header");
        // No ASCII digit in the landed count line (0 → ۰).
        assert!(md.contains("۰"), "Persian glyph for zero appears");
    }

    #[test]
    fn digest_includes_drift_headline_when_data_moves() {
        // Six sessions in the current week vs a zero baseline → a drift
        // headline appears at the top of the digest.
        let sessions: Vec<SessionRecord> = (0..6).map(|i| sess(SAT + i * 60, 25)).collect();
        let verdicts: Vec<crate::journal::Verdict> = sessions
            .iter()
            .map(|s| verdict::declare(s.at, crate::journal::VerdictKind::Kept, s.at).unwrap())
            .collect();
        let d = compose_weekly(
            &sessions,
            &[],
            &[],
            &verdicts,
            &[],
            &[],
            &[],
            &[],
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert!(d.drift.is_some(), "a move from 0 to 6 sessions must drift");
        let md = render_digest(&d, false);
        assert!(
            md.contains("better than your 4-week average"),
            "headline: {md}"
        );
        assert!(md.starts_with("# Weekly digest"), "title first");
    }

    #[test]
    fn digest_renders_prescription_outcome_buckets() {
        // Six sessions, all kept → kept_rate = 1.0. Two prescriptions on the
        // same metric: one issued far below (resolves), one issued near the
        // ceiling (hasn't moved → stubborn after ≥3 weeks).
        let sessions: Vec<SessionRecord> = (0..6).map(|i| sess(SAT + i * 60, 25)).collect();
        let verdicts: Vec<crate::journal::Verdict> = sessions
            .iter()
            .map(|s| verdict::declare(s.at, crate::journal::VerdictKind::Kept, s.at).unwrap())
            .collect();
        use crate::journal::prescription::SuggestionKind;
        let mk =
            |id: u64, target: f32, sample: u32| crate::journal::prescription_ledger::Prescription {
                id,
                week: today().format("%Y-%m-%d").to_string(),
                created_at: SAT,
                kind: SuggestionKind::Calibration,
                target_metric: Some("kept_rate".to_string()),
                target_value: target,
                text: "keep blocks honest".to_string(),
                sample_n: sample,
                deleted: false,
            };
        let prescriptions = vec![mk(1, 0.41, 8), mk(2, 0.99, 8)];
        let d = compose_weekly(
            &sessions,
            &[],
            &[],
            &verdicts,
            &[],
            &[],
            &[],
            &prescriptions,
            &[],
            today(),
            12600,
            300,
            false,
        );
        assert_eq!(d.outcomes.resolved.len(), 1, "low-target advice resolves");
        assert_eq!(
            d.outcomes.stubborn.len(),
            1,
            "near-ceiling advice is stubborn"
        );
        let md = render_digest(&d, false);
        assert!(md.contains("Prescription outcomes"), "section: {md}");
        assert!(md.contains("✅ Resolved"), "resolved bucket heading");
        assert!(md.contains("⚠️ Stubborn"), "stubborn bucket heading");
        assert!(md.contains("41%"), "issue-time value rendered");
        assert!(md.contains("100%"), "current value rendered");
    }
}
