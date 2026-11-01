//! Outcome-aware prescription policy: make the coach actually learn.
//!
//! [`crate::journal::prescription::prescribe`] decides *what* to suggest from
//! analytics alone; it never looks at whether last week's advice worked. So
//! once the ledger started classifying outcomes (Resolved / Stubborn /
//! Reopened), the digest could *diagnose* a metric as stubborn and then
//! blithely re-issue the exact same advice that just failed. This module
//! closes that gap: it takes the raw suggestions and the outcome buckets and
//! applies three rules.
//!
//! * **Resolved suppression** — if a metric's advice recently *worked*
//!   (Resolved within two weeks), don't re-suggest it. Nagging what's
//!   already working trains the user to ignore the coach.
//! * **Stubborn suppression** — if a metric's advice has been issued and
//!   gone nowhere (Stubborn within a week), suppress it *unless* the kind of
//!   advice changed since the last issuance. Repeating the same stale
//!   suggestion is noise; a genuinely different lever is still allowed.
//! * **Escalation** — when the *same* metric has been stubborn for three or
//!   more consecutive weeks, promote it to an escalated warning that names
//!   the failed intervention and proposes a different lever, rather than
//!   whispering the same advice again.
//!
//! Everything here is pure: suggestions, outcomes, and the ledger in; a
//! filtered, possibly escalated suggestion list out. No I/O, no clock reads.

use crate::journal::prescription::{Suggestion, SuggestionKind};
use crate::journal::prescription_ledger::Prescription;
use crate::journal::prescription_outcome::{Outcome, OutcomeBuckets};

/// A resolved outcome stops re-suggestion for this many weeks.
pub const RESOLVED_SUPPRESS_WEEKS: i64 = 2;
/// A stubborn outcome stops re-suggestion for this many weeks.
pub const STUBBORN_SUPPRESS_WEEKS: i64 = 1;
/// Consecutive stubborn weeks before the advice escalates.
pub const STUBBORN_ESCALATION_STREAK: usize = 3;

/// Weeks between two `"%Y-%m-%d"` week-start strings.
fn weeks_between(older: &str, newer: &str) -> i64 {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d");
    let (Ok(a), Ok(b)) = (parse(older), parse(newer)) else {
        return i64::MAX; // unparseable → treat as ancient (no suppression)
    };
    (b - a).num_days() / 7
}

/// True when a metric's advice has been rebutted by the user within the last
/// two weeks. A verdict attaches to a ledger row by id; we resolve that row's
/// metric and check how recent the verdict is.
fn metric_recently_rebutted(
    ledger: &[Prescription],
    verdicts: &[crate::journal::prescription_verdict::PrescriptionVerdict],
    metric: &str,
    now_week: &str,
) -> bool {
    use crate::journal::prescription_verdict::PrescriptionVerdictKind;
    for v in verdicts {
        if v.deleted || v.kind != PrescriptionVerdictKind::Rebutted {
            continue;
        }
        let Some(p) = ledger
            .iter()
            .find(|p| !p.deleted && p.id == v.prescription_id)
        else {
            continue;
        };
        if p.target_metric.as_deref() != Some(metric) {
            continue;
        }
        if weeks_between(&p.week, now_week) <= 2 {
            return true;
        }
    }
    false
}

/// Number of stubborn ledger rows for a metric within the last `streak`
/// weeks of `now_week`. A row from exactly `streak` weeks ago counts as the
/// `streak`-th consecutive week.
fn stubborn_streak(metric: &str, ledger: &[Prescription], now_week: &str) -> usize {
    ledger
        .iter()
        .filter(|p| !p.deleted && p.target_metric.as_deref() == Some(metric))
        .filter(|p| weeks_between(&p.week, now_week) <= STUBBORN_ESCALATION_STREAK as i64)
        .count()
}

/// True when an outcome sits in a given bucket and targets `metric`.
fn bucket_has_metric(bucket: &[Outcome], metric: &str) -> bool {
    bucket
        .iter()
        .any(|o| o.prescription.target_metric.as_deref() == Some(metric))
}

/// Apply the rules to a suggestion list.
///
/// `now_week` is the current week's start (`"%Y-%m-%d"`), used to decide how
/// recent an outcome is. `verdicts` are the user's Accept/Rebut/Defer
/// responses to past prescriptions; a Rebut suppresses the metric entirely —
/// the user rejected that advice, so re-suggesting it (even escalated) is
/// worse than silence.
///
/// `demoted` lists the suggestion kinds whose advice has consistently failed
/// for this user (below a resolution floor). Those kinds are dropped from
/// the output unless that would leave the user with no suggestions at all —
/// in which case the best demoted kind is allowed through once so the coach
/// is never completely silent.
///
/// Returns the filtered suggestions and whether any demotion actually
/// suppressed a kind (so the digest can tell the user why advice went quiet).
pub fn apply(
    suggestions: Vec<Suggestion>,
    outcomes: &OutcomeBuckets,
    ledger: &[Prescription],
    verdicts: &[crate::journal::prescription_verdict::PrescriptionVerdict],
    now_week: &str,
    demoted: &[SuggestionKind],
) -> (Vec<Suggestion>, bool) {
    let filtered: Vec<Suggestion> = suggestions
        .into_iter()
        .filter_map(|mut s| {
            let Some(metric) = s.target_metric else {
                // Untargeted suggestions (none today) pass through untouched.
                return Some(s);
            };

            // Rule (0): a user Rebut on any live prescription for this metric
            // silences it entirely. The human already told the coach the advice
            // was wrong; no amount of metric movement overrides that.
            if metric_recently_rebutted(ledger, verdicts, metric, now_week) {
                return None;
            }

            // Rule (c) first: a stubborn streak escalates to a warning. This
            // overrides suppression — a streak is exactly the case the mild
            // "stay quiet" rule must not hide.
            let streak = stubborn_streak(metric, ledger, now_week);
            if streak >= STUBBORN_ESCALATION_STREAK {
                s.text = escalated_text(metric, &s.kind, ledger, now_week);
                return Some(s);
            }

            // Rule (a): recently resolved → silence this metric entirely.
            let recently_resolved = outcomes.resolved.iter().any(|o| {
                o.prescription.target_metric.as_deref() == Some(metric)
                    && weeks_between(&o.prescription.week, now_week) <= RESOLVED_SUPPRESS_WEEKS
            });
            if recently_resolved {
                return None;
            }

            // Rule (b): recently stubborn → suppress unless the lever changed.
            if let Some(o) = outcomes.stubborn.iter().find(|o| {
                o.prescription.target_metric.as_deref() == Some(metric)
                    && weeks_between(&o.prescription.week, now_week) <= STUBBORN_SUPPRESS_WEEKS
            }) {
                // Different kind than the stubborn issuance → a new lever, keep it.
                if s.kind != o.prescription.kind {
                    return Some(s);
                }
                return None;
            }

            Some(s)
        })
        .collect();

    // Rule (d): demote kinds that consistently fail. Drop them, but if that
    // would leave the user with no advice at all, let the best demoted kind
    // through once so the coach never goes silent.
    let mut kept = Vec::with_capacity(filtered.len());
    let mut demoted_seen = Vec::new();
    for s in filtered {
        if demoted.contains(&s.kind) {
            demoted_seen.push(s);
        } else {
            kept.push(s);
        }
    }
    let mut suppressed = false;
    if kept.is_empty() {
        if let Some(best) = demoted.first() {
            if let Some(s) = demoted_seen.into_iter().find(|s| &s.kind == best) {
                kept.push(s);
            }
        }
    } else if !demoted_seen.is_empty() {
        suppressed = true;
    }
    (kept, suppressed)
}

/// Build the escalated warning for a metric that won't budge.
fn escalated_text(
    metric: &str,
    kind: &SuggestionKind,
    ledger: &[Prescription],
    now_week: &str,
) -> String {
    let noun = crate::journal::prescription::metric_noun_for(metric, false);
    let weeks = stubborn_streak(metric, ledger, now_week);
    match kind {
        SuggestionKind::Calibration => format!(
            "⚠️ Your {noun} has not moved after {weeks} weeks of length advice — \
             stop adjusting block length; pick a fixed 25-minute block and audit \
             what interrupts you instead."
        ),
        SuggestionKind::Break => format!(
            "⚠️ Break advice has not moved {noun} in {weeks} weeks — the lever is \
             not the break length. Try scheduling breaks before focus, not after."
        ),
        SuggestionKind::HonestHour => format!(
            "⚠️ Honest-hour advice has not changed {noun} in {weeks} weeks — \
             instead of hunting the ideal hour, protect one fixed hour daily."
        ),
        SuggestionKind::Unjudged => format!(
            "⚠️ Unjudged blocks have persisted for {weeks} weeks — stop adding \
             new plans until you have judged the backlog."
        ),
        SuggestionKind::Drift => format!(
            "⚠️ The {noun} drift has not been corrected in {weeks} weeks — the \
             advice is not landing. Change the environment, not the target."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::prescription::SuggestionKind;
    use crate::journal::prescription_ledger::Prescription;
    use crate::journal::prescription_outcome::{Outcome, OutcomeKind};

    const WEEK: &str = "2026-10-31"; // the "current" week
    const LAST_WEEK: &str = "2026-10-24";

    fn suggestion(kind: SuggestionKind, metric: &'static str) -> Suggestion {
        Suggestion {
            kind,
            target_metric: Some(metric),
            text: "advice".to_string(),
            sample_n: 8,
        }
    }

    fn prescription(week: &str, kind: SuggestionKind, metric: &str) -> Prescription {
        Prescription {
            id: 1,
            week: week.to_string(),
            created_at: 0,
            kind,
            target_metric: Some(metric.to_string()),
            target_value: 0.5,
            text: "advice".to_string(),
            sample_n: 8,
            deleted: false,
        }
    }

    fn outcome(kind: OutcomeKind, week: &str, metric: &str) -> Outcome {
        Outcome {
            prescription: prescription(week, SuggestionKind::Calibration, metric),
            current_value: 0.9,
            kind,
            stubborn: kind == OutcomeKind::Stubborn,
        }
    }

    #[test]
    fn resolved_within_two_weeks_suppresses_metric() {
        // A Resolved outcome recorded one week ago for kept_rate → no kept_rate
        // suggestions this week; unrelated metrics pass through.
        let buckets = OutcomeBuckets {
            resolved: vec![outcome(OutcomeKind::Resolved, LAST_WEEK, "kept_rate")],
            stubborn: vec![],
            reopened: vec![],
        };
        let in_suggestions = vec![
            suggestion(SuggestionKind::Calibration, "kept_rate"),
            suggestion(SuggestionKind::Drift, "sessions"),
        ];
        let (out, _) = apply(in_suggestions, &buckets, &[], &[], WEEK, &[]);
        assert_eq!(out.len(), 1, "kept_rate suppressed, sessions survives");
        assert_eq!(out[0].target_metric, Some("sessions"));
    }

    #[test]
    fn resolved_older_than_two_weeks_does_not_suppress() {
        let buckets = OutcomeBuckets {
            resolved: vec![outcome(OutcomeKind::Resolved, "2026-09-26", "kept_rate")],
            stubborn: vec![],
            reopened: vec![],
        };
        let kept = suggestion(SuggestionKind::Calibration, "kept_rate");
        let (out, _) = apply(vec![kept.clone()], &buckets, &[], &[], WEEK, &[]);
        assert_eq!(
            out.len(),
            1,
            "stale resolved is not a reason to stay silent"
        );
    }

    #[test]
    fn stubborn_same_kind_within_a_week_suppresses() {
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![outcome(OutcomeKind::Stubborn, LAST_WEEK, "kept_rate")],
            reopened: vec![],
        };
        let (out, _) = apply(
            vec![suggestion(SuggestionKind::Calibration, "kept_rate")],
            &buckets,
            &[],
            &[],
            WEEK,
            &[],
        );
        assert!(
            out.is_empty(),
            "repeating the very advice that failed is noise"
        );
    }

    #[test]
    fn stubborn_but_changed_kind_is_allowed() {
        // Stubborn on calibration, but this week's suggestion is a different
        // lever (break) → keep it.
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![outcome(OutcomeKind::Stubborn, LAST_WEEK, "kept_rate")],
            reopened: vec![],
        };
        // The stubborn outcome was a Calibration; this suggestion is Break.
        let s = suggestion(SuggestionKind::Break, "kept_rate");
        let (out, _) = apply(vec![s], &buckets, &[], &[], WEEK, &[]);
        assert_eq!(out.len(), 1, "a new lever should not be blocked");
    }

    #[test]
    fn escalation_after_three_stubborn_weeks() {
        // Advice issued at 3, 2, and 1 week(s) ago, all without effect.
        let ledger = vec![
            prescription("2026-10-10", SuggestionKind::Calibration, "kept_rate"),
            prescription("2026-10-17", SuggestionKind::Calibration, "kept_rate"),
            prescription(LAST_WEEK, SuggestionKind::Calibration, "kept_rate"),
        ];
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![outcome(OutcomeKind::Stubborn, LAST_WEEK, "kept_rate")],
            reopened: vec![],
        };
        let s = suggestion(SuggestionKind::Calibration, "kept_rate");
        let (out, _) = apply(vec![s], &buckets, &ledger, &[], WEEK, &[]);
        assert_eq!(out.len(), 1, "escalated, not dropped");
        assert!(
            out[0].text.contains("⚠️"),
            "escalated text is a warning: {}",
            out[0].text
        );
    }

    #[test]
    fn no_outcomes_passes_through_unchanged() {
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let in_suggestions = vec![suggestion(SuggestionKind::Drift, "sessions")];
        let (out, _) = apply(in_suggestions.clone(), &buckets, &[], &[], WEEK, &[]);
        assert_eq!(out, in_suggestions);
    }

    #[test]
    fn rebutted_metric_is_silenced() {
        // A user rebutted last week's kept_rate advice → no kept_rate
        // suggestions this week, even though the metric has no outcome to
        // speak of. Unrelated metrics pass through.
        use crate::journal::prescription_verdict::PrescriptionVerdictKind;
        let ledger = vec![prescription(
            LAST_WEEK,
            SuggestionKind::Calibration,
            "kept_rate",
        )];
        let verdicts = vec![crate::journal::prescription_verdict::PrescriptionVerdict {
            id: 1,
            prescription_id: ledger[0].id,
            kind: PrescriptionVerdictKind::Rebutted,
            created_at: 0,
            note: Some("that advice doesn't apply to my weeks".to_string()),
            deleted: false,
        }];
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let in_suggestions = vec![
            suggestion(SuggestionKind::Calibration, "kept_rate"),
            suggestion(SuggestionKind::Drift, "sessions"),
        ];
        let (out, _) = apply(in_suggestions, &buckets, &ledger, &verdicts, WEEK, &[]);
        assert_eq!(
            out.len(),
            1,
            "rebutted metric is silenced, sessions survives"
        );
        assert_eq!(out[0].target_metric, Some("sessions"));
    }

    #[test]
    fn rebutted_older_than_two_weeks_is_not_silenced() {
        use crate::journal::prescription_verdict::PrescriptionVerdictKind;
        // The rebut was three weeks ago — outside the two-week window, so the
        // metric may be suggested again.
        let ledger = vec![prescription(
            "2026-10-10",
            SuggestionKind::Calibration,
            "kept_rate",
        )];
        let verdicts = vec![crate::journal::prescription_verdict::PrescriptionVerdict {
            id: 1,
            prescription_id: ledger[0].id,
            kind: PrescriptionVerdictKind::Rebutted,
            created_at: 0,
            note: None,
            deleted: false,
        }];
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let kept = suggestion(SuggestionKind::Calibration, "kept_rate");
        let (out, _) = apply(vec![kept.clone()], &buckets, &ledger, &verdicts, WEEK, &[]);
        assert_eq!(out.len(), 1, "a stale rebut does not silence forever");
    }

    #[test]
    fn accepted_verdict_does_not_silence() {
        use crate::journal::prescription_verdict::PrescriptionVerdictKind;
        // Accepting the advice is not a rejection — it must not suppress.
        let ledger = vec![prescription(
            LAST_WEEK,
            SuggestionKind::Calibration,
            "kept_rate",
        )];
        let verdicts = vec![crate::journal::prescription_verdict::PrescriptionVerdict {
            id: 1,
            prescription_id: ledger[0].id,
            kind: PrescriptionVerdictKind::Accepted,
            created_at: 0,
            note: None,
            deleted: false,
        }];
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let kept = suggestion(SuggestionKind::Calibration, "kept_rate");
        let (out, _) = apply(vec![kept.clone()], &buckets, &ledger, &verdicts, WEEK, &[]);
        assert_eq!(out.len(), 1, "accepted advice is still allowed");
    }

    #[test]
    fn policy_drops_demoted_kind_when_alternatives_exist() {
        // Calibration is demoted (consistently failing); Break is not. With
        // both present, only Calibration is dropped, and the flag is set.
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let in_suggestions = vec![
            suggestion(SuggestionKind::Calibration, "kept_rate"),
            suggestion(SuggestionKind::Break, "break_adherence"),
        ];
        let (out, suppressed) = apply(
            in_suggestions,
            &buckets,
            &[],
            &[],
            WEEK,
            &[SuggestionKind::Calibration],
        );
        assert!(suppressed, "a demoted kind was dropped");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, SuggestionKind::Break);
    }

    #[test]
    fn policy_allows_best_demoted_kind_when_nothing_else_survives() {
        // Only a demoted kind is present → one suggestion is let through so
        // the coach is never silent, and the suppressed flag stays false.
        let buckets = OutcomeBuckets {
            resolved: vec![],
            stubborn: vec![],
            reopened: vec![],
        };
        let in_suggestions = vec![suggestion(SuggestionKind::Calibration, "kept_rate")];
        let (out, suppressed) = apply(
            in_suggestions,
            &buckets,
            &[],
            &[],
            WEEK,
            &[SuggestionKind::Calibration],
        );
        assert_eq!(out.len(), 1, "never go fully silent");
        assert!(!suppressed, "a fallback was granted, nothing was hidden");
    }
}
