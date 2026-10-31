//! Prescription outcome: what happened after the advice was given.
//!
//! The ledger stores what was advised and the target metric's value at issue
//! time. This module is the feedback loop: for each stored prescription it
//! compares that stored value to the metric's current value and classifies the
//! outcome. Coaching cannot compound until it knows whether it worked.
//!
//! Outcomes:
//!
//! * **Resolved** — the target metric moved in the intended direction and now
//!   clears the threshold the advice aimed at (e.g. kept-rate crossed the
//!   honest-ceiling threshold, unjudged dropped below the concern threshold).
//! * **Improved** — the target moved in the intended direction but has not
//!   fully resolved (e.g. kept-rate rose from 41% to 55%).
//! * **Unchanged** — the metric has not moved meaningfully.
//! * **Worsened** — the metric moved *against* the intended direction.
//! * **Stubborn** — the same advice has been issued for three or more
//!   consecutive weeks with no movement.
//!
//! Everything here is pure: the ledger and the current metric values in,
//! owned outcomes out, no I/O.

use crate::journal::prescription_ledger::Prescription;
use serde::{Deserialize, Serialize};

/// A movement bucket for a metric value relative to its issue-time value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    /// The advice worked and the metric crossed the goal.
    Resolved,
    /// The advice helped but the goal isn't reached yet.
    Improved,
    /// No meaningful movement.
    Unchanged,
    /// The metric moved the wrong way.
    Worsened,
    /// Issued repeatedly with no movement for weeks.
    Stubborn,
}

/// How a metric's "better" direction is defined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Higher is better.
    Higher,
    /// Lower is better.
    Lower,
}

/// The "goal" a prescription was steering toward.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// A target rate to reach (higher is better).
    ReachRate(f32),
    /// A target count to stay under (lower is better).
    StayUnder(u32),
    /// A target count to reach (higher is better).
    ReachCount(u32),
}

/// One prescription's measured outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The prescription this describes (its own stored start value).
    pub prescription: Prescription,
    /// The metric's current value.
    pub current_value: f32,
    /// The classification.
    pub kind: OutcomeKind,
    /// Whether the advice was repeated enough to be "stubborn".
    pub stubborn: bool,
}

/// The intended direction of a metric key.
pub fn direction_for(metric: &str) -> Option<Direction> {
    match metric {
        "sessions" | "focus_minutes" | "kept_rate" | "fulfilment_rate" | "break_adherence" => {
            Some(Direction::Higher)
        }
        "unjudged" => Some(Direction::Lower),
        _ => None,
    }
}

/// The goal a metric should reach, given the prescription's advice.
pub fn goal_for(metric: &str, target_value: f32) -> Option<Goal> {
    match metric {
        "kept_rate" | "fulfilment_rate" | "break_adherence" => {
            // Advice was "get better than this"; goal is to clear a
            // meaningful headroom above the issue-time value, so a small
            // nudge counts as Improved, not Resolved. A fixed +0.05 gate is
            // too easy — a kept-rate that rose from 41% to 46% did not
            // finish the job.
            Some(Goal::ReachRate(target_value + 0.20))
        }
        "unjudged" => Some(Goal::StayUnder(
            (target_value.max(1.0) * 0.5).max(0.0) as u32 + 1,
        )),
        "sessions" | "focus_minutes" => Some(Goal::ReachCount((target_value * 1.2) as u32 + 1)),
        _ => None,
    }
}

/// Classify one prescription's outcome.
pub fn classify(p: &Prescription, current: f32) -> Outcome {
    let dir = direction_for(p.target_metric.as_deref().unwrap_or(""));
    let Some(dir) = dir else {
        return Outcome {
            prescription: p.clone(),
            current_value: current,
            kind: OutcomeKind::Unchanged,
            stubborn: false,
        };
    };
    let goal = goal_for(p.target_metric.as_deref().unwrap_or(""), p.target_value);
    let start = p.target_value;
    let delta = current - start;
    let movement = delta.abs();
    // A relative deadband below which movement is "unchanged".
    const BAND: f32 = 0.03;

    let kind = match goal {
        Some(Goal::ReachRate(goal_rate)) => {
            if current >= goal_rate {
                OutcomeKind::Resolved
            } else if movement < BAND {
                OutcomeKind::Unchanged
            } else if current > start {
                OutcomeKind::Improved
            } else {
                OutcomeKind::Worsened
            }
        }
        Some(Goal::StayUnder(limit)) => {
            if current <= limit as f32 {
                OutcomeKind::Resolved
            } else if movement < BAND {
                OutcomeKind::Unchanged
            } else if current < start {
                OutcomeKind::Improved
            } else {
                OutcomeKind::Worsened
            }
        }
        Some(Goal::ReachCount(goal_count)) => {
            if current >= goal_count as f32 {
                OutcomeKind::Resolved
            } else if movement < BAND {
                OutcomeKind::Unchanged
            } else if current > start {
                OutcomeKind::Improved
            } else {
                OutcomeKind::Worsened
            }
        }
        None => {
            if movement < BAND {
                OutcomeKind::Unchanged
            } else {
                match (dir, current > start) {
                    (Direction::Higher, true) | (Direction::Lower, false) => OutcomeKind::Improved,
                    (Direction::Higher, false) | (Direction::Lower, true) => OutcomeKind::Worsened,
                }
            }
        }
    };

    // Stubborn: issued repeatedly with no meaningful movement.
    let stubborn = movement < BAND && p.sample_n >= 3;
    let kind = if stubborn && kind == OutcomeKind::Unchanged {
        OutcomeKind::Stubborn
    } else {
        kind
    };

    Outcome {
        prescription: p.clone(),
        current_value: current,
        kind,
        stubborn: kind == OutcomeKind::Stubborn,
    }
}

/// Classify every live prescription.
pub fn classify_all(
    prescriptions: &[Prescription],
    current: &dyn Fn(&str) -> Option<f32>,
) -> Vec<Outcome> {
    prescriptions
        .iter()
        .filter(|p| !p.deleted)
        .filter_map(|p| {
            let metric = p.target_metric.as_deref()?;
            let cur = current(metric)?;
            Some(classify(p, cur))
        })
        .collect()
}

/// The headlining "three buckets" DeepSeek asked for: resolved, stubborn, and
/// reopened (an improved/resolved prescription whose metric has since regressed).
#[derive(Clone, Debug, PartialEq)]
pub struct OutcomeBuckets {
    /// Resolved (or improved) — the advice worked.
    pub resolved: Vec<Outcome>,
    /// Stubborn — issued repeatedly, no movement.
    pub stubborn: Vec<Outcome>,
    /// Reopened — a previously-improved metric has regressed.
    pub reopened: Vec<Outcome>,
}

/// Split classifier results into the three renderable buckets.
pub fn bucketed(outcomes: Vec<Outcome>) -> OutcomeBuckets {
    let mut out = OutcomeBuckets {
        resolved: Vec::new(),
        stubborn: Vec::new(),
        reopened: Vec::new(),
    };
    for o in outcomes {
        match o.kind {
            OutcomeKind::Resolved | OutcomeKind::Improved => out.resolved.push(o),
            OutcomeKind::Stubborn => out.stubborn.push(o),
            OutcomeKind::Unchanged | OutcomeKind::Worsened => {
                // "Reopened" is best detected by the caller comparing to the
                // previous week's outcome; here we keep it simple and drop
                // unchanged/worsened into a reopened-style bucket only when
                // they were previously resolved. Without history we treat
                // worsened as reopened (regressed since advice).
                if o.kind == OutcomeKind::Worsened {
                    out.reopened.push(o);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::prescription::SuggestionKind;

    fn row(id: u64, metric: &str, target: f32) -> Prescription {
        Prescription {
            id,
            week: "2026-10-31".to_string(),
            created_at: 1000,
            kind: SuggestionKind::Calibration,
            target_metric: Some(metric.to_string()),
            target_value: target,
            text: "advice".to_string(),
            sample_n: 8,
            deleted: false,
        }
    }

    #[test]
    fn test_outcome_no_prescriptions_returns_empty_vec() {
        // Required acceptance: an empty ledger yields zero outcomes.
        let out = classify_all(&[], &|_| None);
        assert!(out.is_empty());
    }

    #[test]
    fn kept_rate_improved_is_improved() {
        let p = row(1, "kept_rate", 0.41);
        let o = classify(&p, 0.55);
        assert_eq!(o.kind, OutcomeKind::Improved);
    }

    #[test]
    fn kept_rate_resolved_past_goal() {
        let p = row(1, "kept_rate", 0.41);
        let o = classify(&p, 0.70);
        assert_eq!(o.kind, OutcomeKind::Resolved);
    }

    #[test]
    fn kept_rate_worsened() {
        let p = row(1, "kept_rate", 0.60);
        let o = classify(&p, 0.40);
        assert_eq!(o.kind, OutcomeKind::Worsened);
    }

    #[test]
    fn unjudged_lower_is_better() {
        let p = row(2, "unjudged", 4.0);
        let o = classify(&p, 1.0);
        assert_eq!(o.kind, OutcomeKind::Resolved, "dropped below the threshold");
    }

    #[test]
    fn unjudged_increase_is_worsened() {
        let p = row(2, "unjudged", 2.0);
        let o = classify(&p, 6.0);
        assert_eq!(o.kind, OutcomeKind::Worsened);
    }

    #[test]
    fn unchanged_small_movement_is_not_stubborn_unless_repeated() {
        // Small movement but sample_n below our stubborn threshold → unchanged.
        let p = Prescription {
            sample_n: 2,
            ..row(3, "kept_rate", 0.50)
        };
        let o = classify(&p, 0.51);
        assert_eq!(o.kind, OutcomeKind::Unchanged);
        assert!(!o.stubborn);
    }

    #[test]
    fn repeated_stubborn_when_no_movement() {
        let p = row(3, "kept_rate", 0.50);
        let o = classify(&p, 0.51);
        assert_eq!(o.kind, OutcomeKind::Stubborn);
        assert!(o.stubborn);
    }

    #[test]
    fn classify_all_skips_deleted_and_untargeted() {
        let mut p1 = row(1, "kept_rate", 0.41);
        p1.deleted = true;
        let p2 = row(2, "kept_rate", 0.41);
        let p3 = row(3, "unknown_metric", 0.41);
        let out = classify_all(&[p1, p2, p3], &|m| {
            if m == "kept_rate" {
                Some(0.60)
            } else {
                None
            }
        });
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].prescription.id, 2);
    }

    #[test]
    fn buckets_split_correctly() {
        let r = row(1, "kept_rate", 0.41);
        let s = row(2, "kept_rate", 0.50);
        let w = row(3, "kept_rate", 0.70);
        let resolved = classify(&r, 0.70);
        let stubborn = classify(&s, 0.51);
        let worsened = classify(&w, 0.40);
        let buckets = bucketed(vec![resolved, stubborn, worsened]);
        assert_eq!(buckets.resolved.len(), 1);
        assert_eq!(buckets.stubborn.len(), 1);
        assert_eq!(buckets.reopened.len(), 1);
    }
}
