//! Prescription kind effectiveness: which coaching levers actually work?
//!
//! The prescription policy suppresses a metric after a Rebut and escalates
//! after three stubborn weeks, but the coach never learns that *some kinds of
//! advice systematically fail for this specific user while others work*. This
//! module closes that meta-loop: it groups the already-classified outcomes by
//! the kind of prescription that produced them and reports each kind's
//! resolution rate — the share of issued advice of that kind that moved its
//! metric to Resolved or Improved.
//!
//! The output feeds two things. The weekly digest prints a short "which
//! levers actually work for you" line, and the policy can demote kinds whose
//! resolution rate sits below a floor — advice that keeps failing for this
//! user should be issued less, not nagged more.
//!
//! **No fabricated precision.** A kind only reports a resolution rate when it
//! holds at least [`MIN_SAMPLES`] outcomes. The module would rather say
//! nothing than infer a pattern from two data points.
//!
//! Everything here is pure: classified outcomes in, owned stats out, no I/O.

use crate::journal::prescription::SuggestionKind;
use crate::journal::prescription_outcome::{Outcome, OutcomeKind};

/// Minimum outcomes for a kind to report a resolution rate.
pub const MIN_SAMPLES: u32 = 5;

/// A stable sort index for `SuggestionKind` (the enum itself has no `Ord`).
fn kind_index(k: SuggestionKind) -> u8 {
    match k {
        SuggestionKind::Calibration => 0,
        SuggestionKind::Break => 1,
        SuggestionKind::HonestHour => 2,
        SuggestionKind::Unjudged => 3,
        SuggestionKind::Drift => 4,
    }
}

/// A stable name for a `SuggestionKind`.
pub fn kind_name(k: SuggestionKind) -> &'static str {
    match k {
        SuggestionKind::Calibration => "calibration",
        SuggestionKind::Break => "break",
        SuggestionKind::HonestHour => "honest hour",
        SuggestionKind::Unjudged => "unjudged",
        SuggestionKind::Drift => "drift",
    }
}

/// One prescription kind's effectiveness across the record.
#[derive(Clone, Debug)]
pub struct KindStat {
    /// The suggestion kind this row describes.
    pub kind: SuggestionKind,
    /// Outcomes issued of this kind.
    pub issued: u32,
    /// `resolved / issued`. `None` while `issued < MIN_SAMPLES`.
    pub resolution_rate: Option<f32>,
}

fn is_resolved(k: OutcomeKind) -> bool {
    matches!(k, OutcomeKind::Resolved | OutcomeKind::Improved)
}

/// Effectiveness per kind, sorted by resolution rate descending (ties broken
/// by larger sample, then kind id).
pub fn kind_effectiveness(outcomes: &[Outcome], min_samples: u32) -> Vec<KindStat> {
    let mut by: std::collections::HashMap<SuggestionKind, (u32, u32)> =
        std::collections::HashMap::new();
    for o in outcomes {
        let e = by.entry(o.prescription.kind).or_insert((0, 0));
        e.0 += 1;
        if is_resolved(o.kind) {
            e.1 += 1;
        }
    }
    let mut stats: Vec<KindStat> = by
        .into_iter()
        .map(|(kind, (issued, resolved))| KindStat {
            kind,
            issued,
            resolution_rate: if issued < min_samples {
                None
            } else {
                Some(resolved as f32 / issued as f32)
            },
        })
        .collect();
    stats.sort_by(|a, b| match (a.resolution_rate, b.resolution_rate) {
        (Some(x), Some(y)) => y
            .partial_cmp(&x)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.issued.cmp(&a.issued))
            .then(kind_index(a.kind).cmp(&kind_index(b.kind))),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => kind_index(a.kind).cmp(&kind_index(b.kind)),
    });
    stats
}

/// The kind with the highest resolution rate among those clearing `min_samples`.
pub fn best_kind(stats: &[KindStat]) -> Option<SuggestionKind> {
    stats
        .iter()
        .filter(|s| s.resolution_rate.is_some())
        .max_by(|a, b| {
            a.resolution_rate
                .unwrap()
                .partial_cmp(&b.resolution_rate.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| s.kind)
}

/// The kind with the lowest resolution rate among those clearing `min_samples`.
pub fn worst_kind(stats: &[KindStat]) -> Option<SuggestionKind> {
    stats
        .iter()
        .filter(|s| s.resolution_rate.is_some())
        .min_by(|a, b| {
            a.resolution_rate
                .unwrap()
                .partial_cmp(&b.resolution_rate.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| s.kind)
}

/// Kinds whose resolution rate sits below `rate_floor` with enough samples to
/// be trusted — candidates the policy should issue less often.
pub fn demotion_candidates(stats: &[KindStat], rate_floor: f32) -> Vec<SuggestionKind> {
    stats
        .iter()
        .filter(|s| s.resolution_rate.is_some() && s.resolution_rate.unwrap() < rate_floor)
        .map(|s| s.kind)
        .collect()
}

/// A short, localized "which levers actually work for you" line from the
/// effectiveness stats. Returns `None` when no kind has enough samples to
/// judge.
pub fn effectiveness_label(stats: &[KindStat], fa: bool) -> Option<String> {
    let best = best_kind(stats)?;
    let worst = worst_kind(stats)?;
    let best_rate = stats
        .iter()
        .find(|s| s.kind == best)
        .and_then(|s| s.resolution_rate)
        .unwrap_or(0.0);
    let worst_rate = stats
        .iter()
        .find(|s| s.kind == worst)
        .and_then(|s| s.resolution_rate)
        .unwrap_or(0.0);
    let pct = |r: f32| {
        let p = (r * 100.0).round() as i64;
        if fa {
            crate::calendar::jalali::fa_num(p)
        } else {
            p.to_string()
        }
    };
    // Which kinds are consistently failing (below a 50% resolution floor)?
    let demoted = demotion_candidates(stats, 0.5);
    let demoted_line = if demoted.is_empty() {
        String::new()
    } else {
        let names: Vec<String> = demoted.iter().map(|k| kind_name(*k).to_string()).collect();
        if fa {
            format!(" کم‌اثرترین‌ها: {}.", names.join("، "))
        } else {
            format!(" Least effective: {}.", names.join(", "))
        }
    };
    if fa {
        Some(format!(
            "برای تو مؤثرترین کار «{}» است ({}٪ حل) و کم‌اثرترین «{}» ({}٪ حل).{}",
            kind_name(best),
            pct(best_rate),
            kind_name(worst),
            pct(worst_rate),
            demoted_line,
        ))
    } else {
        Some(format!(
            "{} advice resolves {}% of the time; {} advice only {}%.{}",
            kind_name(best),
            pct(best_rate),
            kind_name(worst),
            pct(worst_rate),
            demoted_line,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::prescription_ledger::Prescription;

    fn prescription(id: u64, kind: SuggestionKind) -> Prescription {
        Prescription {
            id,
            week: "2026-10-31".to_string(),
            created_at: 0,
            kind,
            target_metric: Some("kept_rate".to_string()),
            target_value: 0.5,
            text: "x".to_string(),
            sample_n: 1,
            deleted: false,
        }
    }

    fn outcome(p: Prescription, kind: OutcomeKind, stubborn: bool) -> Outcome {
        Outcome {
            prescription: p,
            current_value: 0.6,
            kind,
            stubborn,
        }
    }

    #[test]
    fn test_kind_effectiveness_empty_returns_empty_vec() {
        assert!(kind_effectiveness(&[], 5).is_empty());
    }

    #[test]
    fn test_kind_effectiveness_groups_by_kind() {
        let outcomes = vec![
            outcome(
                prescription(1, SuggestionKind::Break),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(2, SuggestionKind::Break),
                OutcomeKind::Improved,
                false,
            ),
            outcome(
                prescription(3, SuggestionKind::Drift),
                OutcomeKind::Worsened,
                false,
            ),
        ];
        let stats = kind_effectiveness(&outcomes, 1);
        assert_eq!(stats.len(), 2);
        let b = stats
            .iter()
            .find(|s| s.kind == SuggestionKind::Break)
            .unwrap();
        assert_eq!(b.issued, 2);
        assert_eq!(b.resolution_rate.unwrap(), 1.0);
    }

    #[test]
    fn test_kind_effectiveness_none_below_min_samples() {
        let outcomes = vec![outcome(
            prescription(1, SuggestionKind::Break),
            OutcomeKind::Resolved,
            false,
        )];
        let stats = kind_effectiveness(&outcomes, 5);
        assert!(stats[0].resolution_rate.is_none());
    }

    #[test]
    fn test_kind_effectiveness_sorted_by_rate_then_sample() {
        let outcomes = vec![
            outcome(
                prescription(1, SuggestionKind::Break),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(2, SuggestionKind::Break),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(3, SuggestionKind::Drift),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(4, SuggestionKind::Drift),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(5, SuggestionKind::Drift),
                OutcomeKind::Resolved,
                false,
            ),
            outcome(
                prescription(6, SuggestionKind::Unjudged),
                OutcomeKind::Unchanged,
                false,
            ),
            outcome(
                prescription(7, SuggestionKind::Unjudged),
                OutcomeKind::Unchanged,
                false,
            ),
        ];
        let stats = kind_effectiveness(&outcomes, 2);
        // All clear min_samples; Drift (3/3=1.0) beats Break (2/2=1.0) on sample.
        assert_eq!(stats[0].kind, SuggestionKind::Drift);
        assert_eq!(stats[1].kind, SuggestionKind::Break);
        assert_eq!(stats[2].kind, SuggestionKind::Unjudged);
    }

    #[test]
    fn test_best_kind_returns_highest_rate() {
        let stats = kind_effectiveness(
            &[
                outcome(
                    prescription(1, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(2, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(3, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(4, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(5, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
            ],
            2,
        );
        assert_eq!(best_kind(&stats), Some(SuggestionKind::Break));
    }

    #[test]
    fn test_worst_kind_returns_lowest_rate() {
        let stats = kind_effectiveness(
            &[
                outcome(
                    prescription(1, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(2, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(3, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(4, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(5, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
            ],
            2,
        );
        assert_eq!(worst_kind(&stats), Some(SuggestionKind::Drift));
    }

    #[test]
    fn test_demotion_candidates_respects_floor() {
        let stats = kind_effectiveness(
            &[
                outcome(
                    prescription(1, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(2, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(3, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(4, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(5, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
            ],
            2,
        );
        let candidates = demotion_candidates(&stats, 0.5);
        assert!(candidates.contains(&SuggestionKind::Drift));
        assert!(!candidates.contains(&SuggestionKind::Break));
    }

    #[test]
    fn test_effectiveness_label_persian_digits_when_locale_fa() {
        let stats = kind_effectiveness(
            &[
                outcome(
                    prescription(1, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(2, SuggestionKind::Break),
                    OutcomeKind::Resolved,
                    false,
                ),
                outcome(
                    prescription(3, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(4, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
                outcome(
                    prescription(5, SuggestionKind::Drift),
                    OutcomeKind::Unchanged,
                    false,
                ),
            ],
            2,
        );
        let fa = effectiveness_label(&stats, true).unwrap();
        assert!(fa.contains('٪'));
        assert!(fa.contains("کم‌اثرترین"));
        // The demotion clause names the failing kind.
        assert!(fa.contains("drift"));
    }

    #[test]
    fn test_effectiveness_label_none_when_no_kind_qualifies() {
        let stats = kind_effectiveness(
            &[outcome(
                prescription(1, SuggestionKind::Break),
                OutcomeKind::Resolved,
                false,
            )],
            5,
        );
        assert!(effectiveness_label(&stats, false).is_none());
    }
}
