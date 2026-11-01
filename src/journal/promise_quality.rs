//! Promise quality: do quantified decrees land more often?
//!
//! Every analytic in this app treats blocks as numbers — minutes, hours,
//! counts — but none read the *content* of the promise itself. An intention
//! like "write 500 words" is a different kind of promise than "do some work",
//! and it plausibly predicts whether the block is kept. This module tests that
//! with a deterministic, NLP-free heuristic: a decree is *quantified* when its
//! text contains a digit (ASCII or Persian). It then compares the kept-rate of
//! quantified decrees against unquantified ones and reports the **lift** — the
//! headline number — plus the two rates behind it.
//!
//! **What counts as kept?** A decree's session must carry a live (non-deleted)
//! verdict whose kind is `Kept` or `Extended` (`VerdictKind::is_met`). A decree
//! whose session has no verdict, or whose verdict was deleted, says nothing and
//! is excluded rather than guessed.
//!
//! **Why this is honest:** both groups come from the same record and the same
//! verdict set, so the rates are directly comparable. If the sample is too
//! thin to be meaningful, the rates are still reported with their `decrees`
//! counts so the caller can judge; `lift` is simply the difference. An empty
//! record returns `None` — no fabricated precision.
//!
//! Everything here is pure: slices in, owned data out, no I/O, no clock reads.

use crate::journal::{Decree, Verdict, VerdictKind};

/// One group of decrees (quantified or not) and how it fared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PromiseGroup {
    /// How many decrees landed in this group.
    pub decrees: u32,
    /// How many of those were kept (Kept or Extended verdict).
    pub kept: u32,
}

impl PromiseGroup {
    /// The kept-rate as 0.0–1.0, or `None` when the group is empty.
    pub fn kept_rate(&self) -> Option<f32> {
        if self.decrees == 0 {
            None
        } else {
            Some(self.kept as f32 / self.decrees as f32)
        }
    }
}

/// Quantified-promise statistics over the record.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PromiseQuality {
    /// Decrees whose text contains a digit.
    pub quantified: PromiseGroup,
    /// Decrees whose text contains no digit.
    pub unquantified: PromiseGroup,
}

impl PromiseQuality {
    /// The lift: quantified kept-rate minus unquantified kept-rate, as a
    /// signed fraction. Positive means quantified decrees are kept more often.
    /// `None` when either group is empty.
    pub fn lift(&self) -> Option<f32> {
        Some(self.quantified.kept_rate()? - self.unquantified.kept_rate()?)
    }
}

/// True when `text` contains any digit — ASCII (0–9) or Persian (۰–۹).
pub fn contains_digit(text: &str) -> bool {
    text.chars()
        .any(|c| c.is_ascii_digit() || matches!(c, '۰'..='۹'))
}

/// Compute quantified-promise statistics.
///
/// Pure: existing rows in, a value out, no I/O. Decrees are joined to a
/// verdict by `session_at`; a decree with no live verdict is excluded because
/// it cannot be judged.
pub fn promise_quality(decrees: &[Decree], verdicts: &[Verdict]) -> Option<PromiseQuality> {
    // Live verdicts keyed by session completion time.
    let verdict_for: std::collections::HashMap<i64, VerdictKind> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| (v.session_at, v.kind))
        .collect();

    let mut q = PromiseQuality::default();
    for d in decrees {
        if d.deleted {
            continue;
        }
        let Some(kind) = verdict_for.get(&d.session_at).copied() else {
            continue; // an unjudged decree says nothing about kept-rate
        };
        let kept = kind.is_met();
        let g = if contains_digit(&d.text) {
            &mut q.quantified
        } else {
            &mut q.unquantified
        };
        g.decrees += 1;
        if kept {
            g.kept += 1;
        }
    }

    if q.quantified.decrees == 0 && q.unquantified.decrees == 0 {
        return None;
    }
    Some(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decree(session_at: i64, text: &str) -> Decree {
        Decree {
            id: format!("d{session_at}"),
            session_at,
            text: text.to_string(),
            written_at: session_at - 60,
            deleted: false,
        }
    }

    fn verdict(session_at: i64, kind: VerdictKind) -> Verdict {
        Verdict {
            session_at,
            kind,
            declared_at: session_at,
            deleted: false,
        }
    }

    /// An empty record — no decrees — returns None.
    #[test]
    fn test_promise_quality_empty_record_returns_none() {
        assert!(promise_quality(&[], &[]).is_none());
    }

    /// A decree whose session has no verdict is excluded, not guessed.
    #[test]
    fn test_promise_quality_unjudged_decree_is_excluded() {
        let q = promise_quality(&[decree(1, "write 500 words")], &[]);
        assert!(
            q.is_none(),
            "every decree unjudged → None, no fabricated groups"
        );
    }

    /// A deleted decree is ignored even when its session is judged.
    #[test]
    fn test_promise_quality_deleted_decree_is_ignored() {
        let mut d = decree(1, "write 500 words");
        d.deleted = true;
        let v = verdict(1, VerdictKind::Kept);
        let q = promise_quality(&[d], &[v]);
        assert!(q.is_none(), "only a deleted decree remains → None");
    }

    /// Digit detection: ASCII and Persian digits qualify; prose without a
    /// number does not.
    #[test]
    fn test_contains_digit_ascii_and_persian() {
        assert!(contains_digit("write 500 words"));
        assert!(contains_digit("بنویس ۵۰۰ کلمه"));
        assert!(contains_digit("3 papers"));
        assert!(!contains_digit("write some words"));
        assert!(!contains_digit("think harder"));
        assert!(!contains_digit(""));
    }

    /// Quantified decrees kept at 80% vs unquantified at 40% give a lift of
    /// +0.40.
    #[test]
    fn test_promise_quality_quantified_lift() {
        // Quantified: 5 decrees, 4 kept (80%).
        // Unquantified: 5 decrees, 2 kept (40%).
        let mut decrees = Vec::new();
        let mut verdicts = Vec::new();
        for i in 0..5 {
            decrees.push(decree(
                100 + i,
                format!("write {} words", 100 * (i + 1)).as_str(),
            ));
            // copy is fine
            verdicts.push(verdict(
                100 + i,
                if i == 4 {
                    VerdictKind::Broken
                } else {
                    VerdictKind::Kept
                },
            ));
        }
        for i in 0..5 {
            decrees.push(decree(200 + i, "do some thinking"));
            verdicts.push(verdict(
                200 + i,
                if i < 2 {
                    VerdictKind::Kept
                } else {
                    VerdictKind::Broken
                },
            ));
        }

        let q = promise_quality(&decrees, &verdicts).unwrap();
        assert_eq!(
            q.quantified,
            PromiseGroup {
                decrees: 5,
                kept: 4
            }
        );
        assert_eq!(
            q.unquantified,
            PromiseGroup {
                decrees: 5,
                kept: 2
            }
        );
        assert_eq!(q.quantified.kept_rate(), Some(0.8));
        assert_eq!(q.unquantified.kept_rate(), Some(0.4));
        let lift = q.lift().unwrap();
        assert!((lift - 0.40).abs() < 1e-6, "lift = {lift}");
    }

    /// Extended counts as kept (is_met), Short does not.
    #[test]
    fn test_promise_quality_extended_counts_as_kept() {
        let decrees = vec![decree(1, "finish 3 sections"), decree(2, "read")];
        let verdicts = vec![
            verdict(1, VerdictKind::Extended),
            verdict(2, VerdictKind::Short),
        ];
        let q = promise_quality(&decrees, &verdicts).unwrap();
        assert_eq!(
            q.quantified,
            PromiseGroup {
                decrees: 1,
                kept: 1
            }
        );
        assert_eq!(
            q.unquantified,
            PromiseGroup {
                decrees: 1,
                kept: 0
            }
        );
    }

    /// A single empty group makes lift None but still reports the other group.
    #[test]
    fn test_promise_quality_single_group_lift_none() {
        let decrees = vec![decree(1, "write 500 words")];
        let verdicts = vec![verdict(1, VerdictKind::Kept)];
        let q = promise_quality(&decrees, &verdicts).unwrap();
        assert_eq!(q.quantified.kept_rate(), Some(1.0));
        assert_eq!(q.unquantified.kept_rate(), None);
        assert_eq!(q.lift(), None);
    }
}
