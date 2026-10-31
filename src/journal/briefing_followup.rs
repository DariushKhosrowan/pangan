//! Briefing follow-through: does the day honor what the morning briefing asked?
//!
//! The daily briefing speaks in the morning; this module checks — once the day
//! is over — whether it was heard. It evaluates **one completed day** against
//! the three promises the briefing makes:
//!
//! * **The first block should match the calibrated length.** The briefing
//!   names a length to start with (`sweet_spot` from duration calibration). If
//!   the day's first session used a different length, a first block was begun
//!   against the coach's own advice.
//! * **The planned rota should be carried through.** A rota row that ended
//!   `Abandoned` (started, no session landed) or `Missed` (never started, day
//!   over) is a plan the day did not honor.
//! * **The active prescription should get some engagement.** The briefing
//!   surfaces the newest live prescription. If the day produced **no sessions
//!   at all**, nothing was done toward any coaching, and the advice went
//!   unaddressed.
//!
//! Everything here is pure, like the rest of the journal: slices and explicit
//! parameters in, owned signals out, no I/O, no clock reads beyond what is
//! passed in. An empty day returns an empty signal list — a day with no plan,
//! no sessions, and no advice cannot be judged and must not manufacture a
//! verdict (the same no-fabricated-precision discipline as the briefing).
//!
//! **Why the prescription check is intentionally coarse.** The ledger snapshots
//! a target metric's value at *weekly* issue time; comparing a single day's
//! value to that weekly baseline is not apples-to-apples and would risk false
//! claims. So this module only asserts the one violation it can witness for
//! certain: a day with no sessions at all did not act on any prescription.
//! Finer per-metric movement stays in the weekly outcome loop, where the
//! baseline is honest.

use crate::journal::prescription_ledger::Prescription;
use crate::journal::Verdict;
use crate::state::rota::RotaEntry;
use crate::state::store::SessionRecord;
use chrono::NaiveDate;

/// One way a day failed to honor its morning briefing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailySignal {
    /// What went wrong.
    pub kind: SignalKind,
}

/// The concrete failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalKind {
    /// The day's first session used a length other than the calibrated sweet
    /// spot, so the coach's suggested starting length was not taken up.
    FirstBlockLengthMismatch {
        /// The calibrated length the briefing advised, in minutes.
        calibrated_minutes: u64,
        /// The length the first block actually ran, in minutes.
        first_block_minutes: u64,
    },
    /// A planned rota row was started but no session landed.
    RotaEntryAbandoned {
        /// The row's label.
        label: String,
    },
    /// A planned rota row never started, and the day is over.
    RotaEntryMissed {
        /// The row's label.
        label: String,
    },
    /// An active prescription existed and the day produced no sessions at all.
    PrescriptionUnaddressed {
        /// The advice that went unaddressed.
        text: String,
    },
}

/// The local date of a timestamp under a UTC offset.
fn local_date(ts: i64, tz_offset_secs: i32) -> Option<NaiveDate> {
    let offset = chrono::FixedOffset::east_opt(tz_offset_secs)?;
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.with_timezone(&offset).date_naive())
}

/// Evaluate one completed day against its morning briefing.
///
/// `day` is the day being judged — callers pass an already-finished day, such
/// as yesterday. `today` is the real current date; it is what lets [`effective`]
/// tell a plan that was *missed* (its day is over) from one still pending. This
/// is the same `(day, today)` pair the day card uses. Pure: the module reads
/// only what it is given and emits only signals it can witness.
pub fn evaluate_day(
    sessions: &[SessionRecord],
    verdicts: &[Verdict],
    rota: &[RotaEntry],
    prescriptions: &[Prescription],
    day: NaiveDate,
    today: NaiveDate,
    tz_offset_secs: i32,
) -> Vec<DailySignal> {
    let day_s = day.format("%Y-%m-%d").to_string();
    let today_s = today.format("%Y-%m-%d").to_string();
    let mut signals: Vec<DailySignal> = Vec::new();

    // ── 1. First block vs calibrated length ───────────────────────────────
    let calibration = crate::stats::report::duration_calibration(sessions, verdicts);
    if let Some(sweet) = calibration.sweet_spot {
        let first = sessions
            .iter()
            .filter(|s| local_date(s.at, tz_offset_secs) == Some(day))
            .min_by_key(|s| s.at);
        if let Some(first) = first {
            if first.minutes != sweet {
                signals.push(DailySignal {
                    kind: SignalKind::FirstBlockLengthMismatch {
                        calibrated_minutes: sweet,
                        first_block_minutes: first.minutes,
                    },
                });
            }
        }
    }

    // ── 2. Rota follow-through ────────────────────────────────────────────
    // Use the same outcome engine the day card uses, so a row's verdict here is
    // identical to the one the UI would show. `today` is the real date, which
    // is what makes a never-started row on the finished `day` read as missed.
    for e in rota.iter().filter(|e| e.is_live() && e.day == day_s) {
        match crate::state::outcome::effective(e, &today_s) {
            crate::state::outcome::Outcome::Abandoned => signals.push(DailySignal {
                kind: SignalKind::RotaEntryAbandoned {
                    label: e.label.clone(),
                },
            }),
            crate::state::outcome::Outcome::Missed => signals.push(DailySignal {
                kind: SignalKind::RotaEntryMissed {
                    label: e.label.clone(),
                },
            }),
            _ => {}
        }
    }

    // ── 3. Prescription engagement ────────────────────────────────────────
    // Only the witnessable violation: an active prescription and no sessions at
    // all. See the module docs for why finer metric movement stays weekly.
    if let Some(p) = crate::journal::daily_briefing::active_prescription(prescriptions) {
        let has_sessions = sessions
            .iter()
            .any(|s| local_date(s.at, tz_offset_secs) == Some(day));
        if !has_sessions {
            signals.push(DailySignal {
                kind: SignalKind::PrescriptionUnaddressed { text: p.text },
            });
        }
    }

    signals
}

/// Render a day's follow-up as a short Markdown document.
pub fn render_markdown(signals: &[DailySignal], day: NaiveDate, locale_fa: bool) -> String {
    let fa = locale_fa;
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    let date = crate::stats::report::jalali_label(day, !fa, true);
    let mut out = String::new();
    out.push_str(&format!(
        "{} **{}**\n\n",
        if fa {
            "# پیگیری روزانه — "
        } else {
            "# Daily follow-up — "
        },
        date,
    ));

    if signals.is_empty() {
        out.push_str(if fa {
            "هیچ سیگنال منفی‌ای برای این روز ثبت نشد.\n\n"
        } else {
            "No unaddressed signals for this day.\n\n"
        });
        return out;
    }

    out.push_str(if fa {
        "## آنچه روز به انجام نرساند\n\n"
    } else {
        "## What the day did not honor\n\n"
    });
    for s in signals {
        let line = match &s.kind {
            SignalKind::FirstBlockLengthMismatch {
                calibrated_minutes,
                first_block_minutes,
            } => {
                if fa {
                    format!(
                        "اولین بلوک {} دقیقه بود، نه {} دقیقهٔ پیشنهادی.",
                        num(*first_block_minutes as i64),
                        num(*calibrated_minutes as i64)
                    )
                } else {
                    format!(
                        "The first block ran {} minutes, not the calibrated {}.",
                        num(*first_block_minutes as i64),
                        num(*calibrated_minutes as i64)
                    )
                }
            }
            SignalKind::RotaEntryAbandoned { label } => {
                if fa {
                    format!("بلوک «{label}» آغاز شد اما به سرانجام نرسید.")
                } else {
                    format!("The planned block “{label}” was started but abandoned.")
                }
            }
            SignalKind::RotaEntryMissed { label } => {
                if fa {
                    format!("بلوک «{label}» هرگز آغاز نشد.")
                } else {
                    format!("The planned block “{label}” was never started.")
                }
            }
            SignalKind::PrescriptionUnaddressed { text } => {
                if fa {
                    format!("نصیحت فعال نادیده گرفته شد: «{text}»")
                } else {
                    format!("The active prescription went unaddressed: “{text}”")
                }
            }
        };
        out.push_str(&format!("  - {line}\n"));
    }
    out.push('\n');
    out
}

/// Write the follow-up for a finished day, once per day.
///
/// Callers pass the *previous* completed day, so this runs at the end of that
/// day whenever the app next launches. Same once-per-period, atomic-write
/// convention as the briefing and the weekly digest.
pub fn write_yesterday_if_new(
    sessions: &[SessionRecord],
    verdicts: &[Verdict],
    rota: &[RotaEntry],
    prescriptions: &[Prescription],
    today: NaiveDate,
    tz_offset_secs: i32,
    locale_fa: bool,
) -> Option<std::path::PathBuf> {
    let yesterday = today - chrono::Duration::days(1);
    let signals = evaluate_day(
        sessions,
        verdicts,
        rota,
        prescriptions,
        yesterday,
        today,
        tz_offset_secs,
    );
    let dir = crate::state::store::Store::data_dir().join("briefing");
    let filename = format!("{}.followup.md", yesterday.format("%Y-%m-%d"));
    let path = dir.join(&filename);
    if path.exists() {
        return None;
    }
    let md = render_markdown(&signals, yesterday, locale_fa);
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

    fn entry(id: u64, day: &str, label: &str, minutes: u64) -> RotaEntry {
        crate::state::rota::RotaEntry {
            id,
            day: day.to_string(),
            label: label.to_string(),
            task_id: None,
            decree: String::new(),
            minutes,
            created_at: T,
            opened_at: 0,
            session_at: 0,
            outcome: None,
            deleted: false,
        }
    }

    /// An empty day — no plan, no sessions, no advice — renders no signals. It
    /// cannot be judged and must not manufacture a verdict.
    #[test]
    fn test_followup_empty_day_returns_no_signals() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let signals = evaluate_day(&[], &[], &[], &[], day, today, TZ);
        assert!(signals.is_empty(), "empty day must not be judged");
        let md = render_markdown(&signals, day, false);
        assert!(md.contains("No unaddressed signals"), "honest empty render");
    }

    /// When calibration names a sweet spot and the day's first block ignores
    /// it, a length-mismatch signal fires.
    #[test]
    fn test_followup_first_block_length_mismatch() {
        // Calibration needs two well-sampled lengths to name a sweet spot:
        // 5× 25-min kept sessions vs 5× 50-min broken ones → sweet spot 25.
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let mut sessions: Vec<SessionRecord> = Vec::new();
        let mut verdicts: Vec<Verdict> = Vec::new();
        for i in 0..5 {
            let at = T - 3 * 86_400 + i as i64 * 3600;
            sessions.push(sess(at, 25));
            verdicts.push(Verdict {
                session_at: at,
                kind: VerdictKind::Kept,
                declared_at: at,
                deleted: false,
            });
            let at2 = T - 3 * 86_400 + 86_400 + i as i64 * 3600;
            sessions.push(sess(at2, 50));
            verdicts.push(Verdict {
                session_at: at2,
                kind: VerdictKind::Broken,
                declared_at: at2,
                deleted: false,
            });
        }
        // The evaluated day's first block runs 50 minutes, not the calibrated 25.
        sessions.push(sess(T - 86_400, 50));
        let signals = evaluate_day(&sessions, &verdicts, &[], &[], day, today, TZ);
        assert!(
            signals.iter().any(|s| matches!(
                s.kind,
                SignalKind::FirstBlockLengthMismatch {
                    calibrated_minutes: 25,
                    first_block_minutes: 50
                }
            )),
            "first block length mismatch is surfaced"
        );
    }

    /// A planned rota row that was abandoned, and one that was missed, are both
    /// called out by label. Abandoned rows carry an explicit outcome (the app's
    /// end-of-day sweep writes it); missed rows never started.
    #[test]
    fn test_followup_rota_abandoned_and_missed() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let day_s = "2026-10-30";
        let abandoned = {
            let mut e = entry(1, day_s, "started not finished", 25);
            e.opened_at = T; // started, no session landed → abandoned (as the sweep writes it)
            e.outcome = Some("abandoned".to_string());
            e
        };
        let missed = entry(2, day_s, "never started", 25); // not opened, day over → missed
        let done = {
            let mut e = entry(3, day_s, "carried through", 25);
            e.session_at = T;
            e.outcome = Some("done".to_string());
            e
        };
        let rota = vec![abandoned, missed, done];
        let signals = evaluate_day(&[], &[], &rota, &[], day, today, TZ);
        assert!(signals.iter().any(|s| matches!(&s.kind, SignalKind::RotaEntryAbandoned { label } if label == "started not finished")));
        assert!(signals.iter().any(
            |s| matches!(&s.kind, SignalKind::RotaEntryMissed { label } if label == "never started")
        ));
        assert!(!signals.iter().any(|s| matches!(&s.kind, SignalKind::RotaEntryAbandoned { label } if label == "carried through")));
    }

    /// An active prescription with a day that produced no sessions fires the
    /// unaddressed signal; a day that worked does not.
    #[test]
    fn test_followup_prescription_unaddressed_only_when_no_sessions() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap();
        let prescription = Prescription {
            id: 1,
            week: "2026-10-25".to_string(),
            created_at: T,
            kind: crate::journal::prescription::SuggestionKind::Calibration,
            target_metric: Some("kept_rate".to_string()),
            target_value: 0.6,
            text: "Plan blocks around 25 minutes.".to_string(),
            sample_n: 12,
            deleted: false,
        };
        // No sessions on the evaluated day → unaddressed.
        let signals = evaluate_day(&[], &[], &[], &[prescription.clone()], day, today, TZ);
        assert!(signals.iter().any(|s| matches!(&s.kind, SignalKind::PrescriptionUnaddressed { text } if text == "Plan blocks around 25 minutes.")));
        // A session on the evaluated day → not unaddressed.
        let sessions = vec![sess(T - 86_400, 25)];
        let signals = evaluate_day(&sessions, &[], &[], &[prescription], day, today, TZ);
        assert!(!signals
            .iter()
            .any(|s| matches!(s.kind, SignalKind::PrescriptionUnaddressed { .. })));
    }
}
