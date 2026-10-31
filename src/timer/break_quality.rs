//! Break quality: do my breaks help or hurt the next block?
//!
//! A Pomodoro timer counts blocks and it counts breaks, but it never asks
//! whether the *length* of a break matters. This module does — it is the
//! break-side analog of [`duration_calibration`](crate::stats::report::duration_calibration).
//! Breaks are bucketed by their actual length, and for each bucket we measure
//! the kept-rate of the **next focus block** that follows a break of that
//! length.
//!
//! The question it answers is one no timer asks: *is a 5-minute break better
//! than a 15-minute one for what comes after?* A user who is 72% kept after
//! 5-minute breaks but 48% after 15-minute ones needs to know the break length
//! is the problem — not merely that a break happened.
//!
//! **How a break is paired to its next block.** A session records its
//! completion (`at`) and its length (`minutes`). A completed break ends at
//! `ended_at`; the block that follows it is the first session whose completion
//! comes strictly after the break ends. That session's verdict — when it has a
//! live one — is the outcome of the break. If no session follows the break, or
//! the following session was never judged, that break contributes no rated
//! observation (a break at the end of the day has nothing to predict).
//!
//! **No fabricated precision.** A bucket only reports a kept-rate when it
//! holds at least [`MIN_SAMPLES`] rated observations. A fresh install, or a
//! record with few judged blocks, produces empty buckets — the module would
//! rather say nothing than invent a correlation from two data points. The same
//! discipline as the rest of the journal.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use crate::journal::VerdictKind;
use crate::state::store::SessionRecord;
use crate::timer::break_record::BreakRecord;

/// A break's actual length is bucketed by its minute count.
///
/// The bucket edges define the ranges: the *i*-th bucket holds actual lengths
/// from `edges[i]` up to (but not including) `edges[i+1]`, the first bucket
/// holds everything under `edges[0]`, and the last holds `edges[last]` and up.
pub const BREAK_QUALITY_BUCKET_MINUTES: [u64; 4] = [5, 10, 15, 20];

/// Minimum rated next-blocks for a bucket to report a kept-rate.
pub const MIN_SAMPLES: u32 = 5;

/// One bucketed break length and how the blocks after it fared.
#[derive(Clone, Debug)]
pub struct BreakQualityBucket {
    /// The bucket's label, e.g. `"5–9"` for the 5-minute range.
    pub label: &'static str,
    /// Completed breaks that landed in this bucket.
    pub breaks: u32,
    /// Next blocks (a break's successor) that carry a live verdict.
    pub rated: u32,
    /// `kept / rated` — the share of next blocks that met their promise.
    /// `None` when the bucket has fewer than [`MIN_SAMPLES`] rated blocks.
    pub kept_rate: Option<f32>,
}

/// The whole break-quality picture.
#[derive(Clone, Debug)]
pub struct BreakQuality {
    /// One bucket per range in [`BREAK_QUALITY_BUCKET_MINUTES`].
    pub buckets: Vec<BreakQualityBucket>,
    /// The break length whose next-block kept-rate is highest, among buckets
    /// with at least [`MIN_SAMPLES`] rated blocks. Ties break by larger
    /// sample, then shorter length. `None` until a bucket qualifies.
    pub best_break_length: Option<&'static str>,
    /// Total rated next-block observations across all buckets.
    pub total_rated: u32,
}

/// The range label for a bucket index.
fn bucket_label(i: usize) -> &'static str {
    match i {
        0 => "<5",
        1 => "5–9",
        2 => "10–14",
        3 => "15–19",
        _ => "20+",
    }
}

/// Which bucket an actual break length (minutes) falls into.
fn bucket_index(minutes: u64) -> usize {
    let mut idx = 0;
    for (i, &edge) in BREAK_QUALITY_BUCKET_MINUTES.iter().enumerate() {
        if minutes >= edge {
            idx = i + 1;
        } else {
            break;
        }
    }
    idx.min(BREAK_QUALITY_BUCKET_MINUTES.len())
}

/// Compute break quality across the whole record.
///
/// Pure: existing rows in, a value out, no I/O. Breaks with no successor
/// session, and successor sessions without a live verdict, contribute no rated
/// observation.
pub fn break_quality(
    breaks: &[BreakRecord],
    sessions: &[SessionRecord],
    verdicts: &[crate::journal::Verdict],
) -> BreakQuality {
    // Live verdicts keyed by session completion time.
    let verdict_for: std::collections::HashMap<i64, VerdictKind> = verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| (v.session_at, v.kind))
        .collect();

    // Sessions sorted by completion, for the binary search.
    let mut sorted: Vec<&SessionRecord> = sessions.iter().collect();
    sorted.sort_by_key(|s| s.at);

    let mut breaks_cnt = vec![0u32; BREAK_QUALITY_BUCKET_MINUTES.len() + 1];
    let mut rated_cnt = vec![0u32; BREAK_QUALITY_BUCKET_MINUTES.len() + 1];
    let mut kept_cnt = vec![0u32; BREAK_QUALITY_BUCKET_MINUTES.len() + 1];

    for br in breaks {
        let Some(ended) = br.ended_at else {
            continue; // an open break has nothing to predict
        };
        let actual_minutes = ((ended - br.started_at).max(0) / 60) as u64;
        let idx = bucket_index(actual_minutes);
        breaks_cnt[idx] += 1;

        // The successor block: the first session completed after the break ends.
        let pos = sorted.partition_point(|s| s.at <= ended);
        let Some(next) = sorted.get(pos) else {
            continue; // the break is the last thing; no block follows
        };
        if let Some(&kind) = verdict_for.get(&next.at) {
            rated_cnt[idx] += 1;
            if kind.is_met() {
                kept_cnt[idx] += 1;
            }
        }
    }

    let buckets: Vec<BreakQualityBucket> = (0..breaks_cnt.len())
        .map(|i| {
            let rated = rated_cnt[i];
            BreakQualityBucket {
                label: bucket_label(i),
                breaks: breaks_cnt[i],
                rated,
                kept_rate: (rated >= MIN_SAMPLES).then(|| kept_cnt[i] as f32 / rated as f32),
            }
        })
        .collect();

    let total_rated: u32 = rated_cnt.iter().sum();

    // Best break length: highest kept-rate among qualifying buckets; ties break
    // by larger sample, then by shorter length (lower index).
    let best_break_length = {
        let mut best: Option<(f32, u32, &'static str)> = None;
        for b in &buckets {
            if let Some(rate) = b.kept_rate {
                let better = match best {
                    None => true,
                    Some((br, brated, _)) => {
                        rate > br + 1e-9 || (rate - br).abs() < 1e-9 && b.rated > brated
                    }
                };
                if better {
                    best = Some((rate, b.rated, b.label));
                }
            }
        }
        best.map(|(_, _, label)| label)
    };

    BreakQuality {
        buckets,
        best_break_length,
        total_rated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Verdict;
    use crate::journal::VerdictKind;
    use crate::state::store::SessionRecord;
    use crate::timer::break_record::{BreakRecord, BreakRel};

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    fn break_rec(started_at: i64, actual_minutes: u64) -> BreakRecord {
        BreakRecord {
            started_at,
            ended_at: Some(started_at + (actual_minutes * 60) as i64),
            planned_secs: 300,
            kind: BreakRel::Short,
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

    /// Fewer than MIN_SAMPLES rated observations in every bucket → no kept-rates
    /// and no best length, rather than fabricated precision.
    #[test]
    fn test_break_quality_below_min_samples_returns_none() {
        let breaks = vec![break_rec(1_000, 5), break_rec(3_000, 10)];
        let sessions = vec![sess(1_600, 25), sess(4_000, 25)];
        let verdicts = vec![
            verdict(1_600, VerdictKind::Kept),
            verdict(4_000, VerdictKind::Kept),
        ];

        let q = break_quality(&breaks, &sessions, &verdicts);
        assert!(
            q.best_break_length.is_none(),
            "no best length below min samples"
        );
        assert!(
            q.buckets.iter().all(|b| b.kept_rate.is_none()),
            "no bucket reports a rate below min samples"
        );
        assert_eq!(q.total_rated, 2, "the observations are still counted");
    }

    /// A break with no successor session, and one whose successor is unjudged,
    /// contributes no rated observation.
    #[test]
    fn test_break_quality_unpaired_breaks_contribute_nothing() {
        // First break: a successor session exists but is unjudged (no verdict).
        // Second break: no successor at all.
        let breaks = vec![break_rec(1_000, 5), break_rec(20_000, 10)];
        let sessions = vec![sess(1_600, 25)];
        let q = break_quality(&breaks, &sessions, &[]);
        assert_eq!(q.total_rated, 0, "no rated observations without verdicts");
        assert_eq!(
            q.buckets[1].breaks, 1,
            "the 5-min break lands in the 5–9 bucket"
        );
        assert_eq!(
            q.buckets[2].breaks, 1,
            "the 10-min break lands in the 10–14 bucket"
        );
    }

    /// Distinguishes break lengths: 5-min breaks followed by kept blocks, and
    /// 15-min breaks followed by broken ones, should surface 5-min as the best
    /// length once both buckets clear the sample floor.
    #[test]
    fn test_break_quality_distinguishes_lengths() {
        let mut breaks = Vec::new();
        let mut sessions = Vec::new();
        let mut verdicts = Vec::new();
        let mut t = 10_000i64;
        // 6× 5-min breaks, each followed by a kept block.
        for i in 0..6 {
            let b_start = t;
            let b_end = b_start + 300;
            let s_at = b_end + 1200;
            breaks.push(BreakRecord {
                started_at: b_start,
                ended_at: Some(b_end),
                planned_secs: 300,
                kind: BreakRel::Short,
            });
            sessions.push(sess(s_at, 25));
            verdicts.push(verdict(s_at, VerdictKind::Kept));
            t = s_at + 2000;
        }
        // 6× 15-min breaks, each followed by a broken block.
        for _ in 0..6 {
            let b_start = t;
            let b_end = b_start + 900;
            let s_at = b_end + 1200;
            breaks.push(BreakRecord {
                started_at: b_start,
                ended_at: Some(b_end),
                planned_secs: 900,
                kind: BreakRel::Short,
            });
            sessions.push(sess(s_at, 25));
            verdicts.push(verdict(s_at, VerdictKind::Broken));
            t = s_at + 2000;
        }

        let q = break_quality(&breaks, &sessions, &verdicts);
        assert_eq!(
            q.best_break_length,
            Some("5–9"),
            "5-min breaks lead to the best kept-rate"
        );
        let b5 = q.buckets.iter().find(|b| b.label == "5–9").unwrap();
        let b15 = q.buckets.iter().find(|b| b.label == "15–19").unwrap();
        assert_eq!(b5.kept_rate, Some(1.0), "all 5-min successors kept");
        assert_eq!(b15.kept_rate, Some(0.0), "all 15-min successors broken");
    }
}
