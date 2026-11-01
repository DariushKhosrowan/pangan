//! User verdicts on the coach's advice: Accept / Rebut / Defer.
//!
//! The prescription ledger records what was advised, and the outcome module
//! measures whether the *metric* moved. Neither hears from the human whether
//! the advice itself was applicable, appropriate, or wanted. A Rebut is the
//! loudest signal a coach can get: it means the advice was wrong or
//! inapplicable, not merely unheard. If the coach never learns *that*, it
//! keeps offering advice the user has already rejected. This module is that
//! signal, stored once per prescription row.
//!
//! Three verdicts cover the three honest answers to an advice:
//!
//! * **Accepted** — the advice fit, and the user committed to it. The coach
//!   may follow up on it next week: did committing move the metric?
//! * **Rebutted** — the advice was wrong or inapplicable. The coach should
//!   back off this metric for a while, not re-issue or escalate it.
//! * **Deferred** — the advice is reasonable but not now. Revisit later, but
//!   do not treat it as accepted.
//!
//! A verdict is keyed by `prescription_id` (the ledger row it answers), not
//! by session — a prescription is a week-shaped statement, and the verdict is
//! about that statement. The `created_at` is the merge stamp, and `deleted`
//! is the tombstone, so a verdict travels across devices like every other
//! mutable row.
//!
//! Everything here is pure: slices and a counter in, owned data out, no I/O,
//! no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// What a user did with a prescription.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrescriptionVerdictKind {
    /// The advice fit; the user committed to it.
    Accepted,
    /// The advice was wrong or inapplicable; the coach should back off.
    Rebutted,
    /// The advice is reasonable but not now; revisit later.
    Deferred,
}

impl PrescriptionVerdictKind {
    /// The stable storage form, so a hand-read `data.json` says `"accepted"`.
    pub fn as_str(self) -> &'static str {
        match self {
            PrescriptionVerdictKind::Accepted => "accepted",
            PrescriptionVerdictKind::Rebutted => "rebutted",
            PrescriptionVerdictKind::Deferred => "deferred",
        }
    }

    /// The human label, localised.
    pub fn label(self, fa: bool) -> &'static str {
        match (fa, self) {
            (true, PrescriptionVerdictKind::Accepted) => "پذیرفته شد",
            (true, PrescriptionVerdictKind::Rebutted) => "رد شد",
            (true, PrescriptionVerdictKind::Deferred) => "به تعویق افتاد",
            (false, PrescriptionVerdictKind::Accepted) => "accepted",
            (false, PrescriptionVerdictKind::Rebutted) => "rebutted",
            (false, PrescriptionVerdictKind::Deferred) => "deferred",
        }
    }
}

/// One user verdict on one prescription row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrescriptionVerdict {
    /// Stable identity from the store's counter.
    pub id: u64,
    /// The ledger row this answers.
    pub prescription_id: u64,
    /// What the user decided.
    pub kind: PrescriptionVerdictKind,
    /// Unix seconds when this row was written — the merge stamp.
    pub created_at: i64,
    /// Optional free text: why the advice was rebutted or deferred.
    #[serde(default)]
    pub note: Option<String>,
    /// Tombstone. Set → this row should be treated as gone everywhere.
    #[serde(default)]
    pub deleted: bool,
}

/// Create a verdict row, drawing its id from the store's counter.
pub fn declare(
    next_id: u64,
    prescription_id: u64,
    kind: PrescriptionVerdictKind,
    created_at: i64,
    note: Option<String>,
) -> (PrescriptionVerdict, u64) {
    (
        PrescriptionVerdict {
            id: next_id + 1,
            prescription_id,
            kind,
            created_at,
            note,
            deleted: false,
        },
        next_id + 1,
    )
}

/// Merge two verdict lists: keyed by id, newest `created_at` wins, a tombstone
/// beats a live copy, ties broken deterministically by id.
pub fn merge(a: &[PrescriptionVerdict], b: &[PrescriptionVerdict]) -> Vec<PrescriptionVerdict> {
    let mut map: std::collections::BTreeMap<u64, PrescriptionVerdict> =
        std::collections::BTreeMap::new();
    for v in a.iter().chain(b.iter()) {
        match map.get(&v.id) {
            None => {
                map.insert(v.id, v.clone());
            }
            Some(existing) => {
                if keep(v, existing) {
                    map.insert(v.id, v.clone());
                }
            }
        }
    }
    let mut out: Vec<PrescriptionVerdict> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.prescription_id
            .cmp(&y.prescription_id)
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Is `candidate` the copy of one verdict row to keep?
fn keep(candidate: &PrescriptionVerdict, existing: &PrescriptionVerdict) -> bool {
    if candidate.created_at != existing.created_at {
        return candidate.created_at > existing.created_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    candidate.id > existing.id
}

/// The latest live verdict for a given prescription row, if any.
pub fn latest_for_prescription<'a>(
    rows: &'a [PrescriptionVerdict],
    prescription_id: u64,
) -> Option<&'a PrescriptionVerdict> {
    rows.iter()
        .filter(|v| !v.deleted && v.prescription_id == prescription_id)
        .max_by_key(|v| v.created_at)
}

/// True when the latest live verdict for `prescription_id` is a Rebut.
pub fn is_rebutted(rows: &[PrescriptionVerdict], prescription_id: u64) -> bool {
    matches!(
        latest_for_prescription(rows, prescription_id),
        Some(v) if v.kind == PrescriptionVerdictKind::Rebutted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(id: u64, pid: u64, kind: PrescriptionVerdictKind, at: i64) -> PrescriptionVerdict {
        PrescriptionVerdict {
            id,
            prescription_id: pid,
            kind,
            created_at: at,
            note: None,
            deleted: false,
        }
    }

    #[test]
    fn declare_draws_counter_forward() {
        let (v, next) = declare(3, 7, PrescriptionVerdictKind::Accepted, 100, None);
        assert_eq!(v.id, 4);
        assert_eq!(v.prescription_id, 7);
        assert_eq!(v.kind, PrescriptionVerdictKind::Accepted);
        assert_eq!(next, 4);
        assert!(!v.deleted);
    }

    #[test]
    fn merge_keeps_newest_created_at() {
        let a = [verdict(1, 1, PrescriptionVerdictKind::Deferred, 100)];
        let b = [verdict(1, 1, PrescriptionVerdictKind::Accepted, 200)];
        let out = merge(&a, &b);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, PrescriptionVerdictKind::Accepted);
    }

    #[test]
    fn merge_tombstone_beats_live() {
        // Same created_at: the deleted flag decides — a tombstone beats a live
        // copy, exactly like the prescription ledger's convention.
        let mut dead = verdict(1, 1, PrescriptionVerdictKind::Accepted, 200);
        dead.deleted = true;
        let mut live = verdict(1, 1, PrescriptionVerdictKind::Accepted, 200);
        live.deleted = false;
        let out = merge(&[live], &[dead]);
        assert_eq!(out.len(), 1);
        assert!(out[0].deleted, "a tombstone always beats a live copy");
    }

    #[test]
    fn latest_for_prescription_prefers_latest_live() {
        let rows = [
            verdict(1, 5, PrescriptionVerdictKind::Deferred, 100),
            verdict(2, 5, PrescriptionVerdictKind::Rebutted, 300),
            verdict(3, 6, PrescriptionVerdictKind::Accepted, 200),
        ];
        let latest = latest_for_prescription(&rows, 5).unwrap();
        assert_eq!(latest.kind, PrescriptionVerdictKind::Rebutted);
        assert!(is_rebutted(&rows, 5));
        assert!(!is_rebutted(&rows, 6));
        assert!(latest_for_prescription(&rows, 99).is_none());
    }

    #[test]
    fn deleted_verdict_is_not_latest() {
        let mut dead = verdict(2, 5, PrescriptionVerdictKind::Rebutted, 300);
        dead.deleted = true;
        let rows = [verdict(1, 5, PrescriptionVerdictKind::Deferred, 100), dead];
        assert_eq!(
            latest_for_prescription(&rows, 5).unwrap().kind,
            PrescriptionVerdictKind::Deferred
        );
        assert!(!is_rebutted(&rows, 5));
    }

    #[test]
    fn labels_cover_all_kinds_in_both_locales() {
        for kind in [
            PrescriptionVerdictKind::Accepted,
            PrescriptionVerdictKind::Rebutted,
            PrescriptionVerdictKind::Deferred,
        ] {
            assert!(!kind.label(false).is_empty());
            assert!(!kind.label(true).is_empty());
        }
    }
}
