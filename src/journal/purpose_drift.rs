//! Purpose drift: does a block do what its decree promised?
//!
//! The whole app is built around the decree/inscription pair: a decree
//! states what a block is *for*, and the inscription records what it
//! *produced*. Every other coached metric reads the *metadata* of that pair
//! — whether a verdict was kept, how long the block ran, what hour it fell
//! in — but nothing reads the two texts against each other. A user whose
//! blocks keep drifting from their stated purpose (decree "write thesis",
//! inscription "answered emails") gets no signal for it today, even though
//! that drift is often why their kept-rate is stuck.
//!
//! This module is that signal. It compares each block's decree text with its
//! inscription text using a cheap token-overlap heuristic (no NLP dependency,
//! no new crates) and reports how often the block did what the decree
//! promised — per hour of day, per duration bucket, and overall.
//!
//! **No fabricated precision.** A bucket only reports a drift rate when it
//! holds at least [`MIN_SAMPLES`] paired blocks. The module would rather say
//! nothing than invent a rate from two data points — the same discipline as
//! the rest of the journal.
//!
//! Everything here is pure: slices and explicit parameters in, owned data
//! out, no I/O, no clock reads beyond what is passed in.

use crate::journal::decree::Decree;
use crate::journal::inscription::Inscription;

/// Default overlap threshold: a block whose decree and inscription share less
/// than this fraction of their tokens is counted as drifted.
pub const DEFAULT_THRESHOLD: f32 = 0.4;

/// Minimum paired blocks for a bucket to report a drift rate.
pub const MIN_SAMPLES: u32 = 5;

/// One drift bucket: how many paired blocks landed here, how many drifted,
/// and the resulting rate.
#[derive(Clone, Debug)]
pub struct DriftStat {
    /// Bucket id: hour of day (0–23), duration-bucket index, or 0 for the
    /// overall stat.
    pub bucket: u8,
    /// Paired blocks that landed in this bucket.
    pub samples: u32,
    /// Of those, the number whose overlap fell below the threshold.
    pub drifted: u32,
    /// `drifted / samples`. `None` while the bucket has fewer than
    /// [`MIN_SAMPLES`] paired blocks.
    pub drifted_rate: Option<f32>,
}

/// Split a line into the tokens that carry meaning for the overlap check.
///
/// Lowercases ASCII (Persian has no case), splits on ASCII whitespace and the
/// Persian ZWNJ (U+200C) so "می‌خواهم" → "می", "خواهم", drops tokens shorter
/// than 3 *characters* (not bytes), and drops a small embedded Persian+English
/// stopword list.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_ascii_whitespace() || c == '\u{200c}')
        .filter_map(|tok| {
            let lower: String = tok.chars().map(|c| c.to_ascii_lowercase()).collect();
            let n = lower.chars().count();
            if n < 3 || is_stopword(&lower) {
                None
            } else {
                Some(lower)
            }
        })
        .collect()
}

/// The fractional overlap between two token lists: the size of the
/// intersection over the smaller of the two lists. Identical lists score
/// 1.0; disjoint lists score 0.0.
pub fn overlap(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let (small, large) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut hits = 0usize;
    for t in small {
        if large.contains(t) {
            hits += 1;
        }
    }
    hits as f32 / small.len() as f32
}

/// A small embedded Persian + English stopword list (tokens that carry no
/// signal for overlap).
fn is_stopword(tok: &str) -> bool {
    matches!(
        tok,
        "the"
            | "and"
            | "for"
            | "with"
            | "this"
            | "that"
            | "was"
            | "were"
            | "but"
            | "not"
            | "are"
            | "his"
            | "her"
            | "its"
            | "has"
            | "have"
            | "به"
            | "که"
            | "و"
            | "در"
            | "از"
            | "با"
            | "این"
            | "آن"
    )
}

/// Build the list of decree→inscription pairs for blocks that have both.
fn paired_blocks<'a>(
    decrees: &'a [Decree],
    inscriptions: &'a [Inscription],
) -> Vec<(&'a Decree, &'a Inscription)> {
    let ins_by_session: std::collections::HashMap<i64, &Inscription> = inscriptions
        .iter()
        .filter(|i| !i.deleted)
        .map(|i| (i.session_at, i))
        .collect();
    decrees
        .iter()
        .filter(|d| !d.deleted)
        .filter_map(|d| ins_by_session.get(&d.session_at).map(|i| (d, *i)))
        .collect()
}

/// Whether a pair drifted (overlap below threshold).
fn drifted(pair: &(&Decree, &Inscription), threshold: f32) -> bool {
    let dt = tokenize(&pair.0.text);
    let it = tokenize(&pair.1.text);
    overlap(&dt, &it) < threshold
}

/// The local hour of a session's end, under the given tz offset.
fn local_hour(session_at: i64, tz_offset_secs: i32) -> u8 {
    let secs = session_at.wrapping_add(tz_offset_secs as i64);
    let hour = secs.rem_euclid(86_400) / 3_600;
    hour as u8
}

/// The focus-block duration (seconds) inferred from the decree's own timestamps.
fn block_duration_secs(pair: &(&Decree, &Inscription)) -> i64 {
    pair.0.session_at - pair.0.written_at
}

/// Duration-bucket edges in minutes; the *i*-th bucket holds actual lengths
/// from `edges[i]` up to (but not including) `edges[i+1]`, the first holds
/// everything under `edges[0]`, and the last holds `edges[last]` and up.
pub const DRIFT_DURATION_BUCKET_MINUTES: [u64; 4] = [15, 35, 60, 90];

/// The range label for a duration-bucket index.
fn duration_bucket_label(i: usize) -> &'static str {
    match i {
        0 => "<15",
        1 => "15–34",
        2 => "35–59",
        3 => "60–89",
        _ => "90+",
    }
}

/// Which duration bucket a block length (minutes) falls into.
fn duration_bucket_index(minutes: u64) -> usize {
    let mut idx = 0;
    for (i, &edge) in DRIFT_DURATION_BUCKET_MINUTES.iter().enumerate() {
        if minutes >= edge {
            idx = i + 1;
        } else {
            break;
        }
    }
    idx
}

/// Drift bucketed by the local hour the block ended (0–23).
pub fn drift_by_hour(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    tz_offset_secs: i32,
    threshold: f32,
) -> [DriftStat; 24] {
    let pairs = paired_blocks(decrees, inscriptions);
    let mut out: [DriftStat; 24] = std::array::from_fn(|h| DriftStat {
        bucket: h as u8,
        samples: 0,
        drifted: 0,
        drifted_rate: None,
    });
    for p in &pairs {
        let h = local_hour(p.1.session_at, tz_offset_secs) as usize;
        out[h].samples += 1;
        if drifted(p, threshold) {
            out[h].drifted += 1;
        }
    }
    for stat in out.iter_mut() {
        stat.drifted_rate = rate(stat.samples, stat.drifted);
    }
    out
}

/// Drift bucketed by the focus block's duration.
pub fn drift_by_duration(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    threshold: f32,
) -> Vec<DriftStat> {
    let pairs = paired_blocks(decrees, inscriptions);
    let n = DRIFT_DURATION_BUCKET_MINUTES.len() + 1;
    let mut stats: Vec<DriftStat> = (0..n)
        .map(|i| DriftStat {
            bucket: i as u8,
            samples: 0,
            drifted: 0,
            drifted_rate: None,
        })
        .collect();
    for p in &pairs {
        let mins = (block_duration_secs(p) / 60).max(0) as u64;
        let idx = duration_bucket_index(mins);
        stats[idx].samples += 1;
        if drifted(p, threshold) {
            stats[idx].drifted += 1;
        }
    }
    for stat in stats.iter_mut() {
        stat.drifted_rate = rate(stat.samples, stat.drifted);
    }
    stats
}

/// The overall drift across the whole record. `None` until at least
/// `min_samples` paired blocks exist.
pub fn drift_overall(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    threshold: f32,
    min_samples: u32,
) -> Option<DriftStat> {
    let pairs = paired_blocks(decrees, inscriptions);
    let samples = pairs.len() as u32;
    let drifted = pairs.iter().filter(|p| drifted(p, threshold)).count() as u32;
    if samples < min_samples {
        return None;
    }
    Some(DriftStat {
        bucket: 0,
        samples,
        drifted,
        drifted_rate: rate(samples, drifted),
    })
}

/// `drifted / samples`, or `None` when too few samples.
fn rate(samples: u32, drifted: u32) -> Option<f32> {
    if samples < MIN_SAMPLES {
        None
    } else {
        Some(drifted as f32 / samples as f32)
    }
}

/// The local hour with the highest drift rate among hours holding enough
/// paired blocks to judge honestly. Returns the hour (0–23) and its stat.
pub fn worst_hour_drift(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    tz_offset_secs: i32,
    threshold: f32,
) -> Option<(u8, DriftStat)> {
    let by_hour = drift_by_hour(decrees, inscriptions, tz_offset_secs, threshold);
    by_hour
        .iter()
        .filter(|s| s.drifted_rate.is_some())
        .max_by(|a, b| {
            a.drifted_rate
                .unwrap()
                .partial_cmp(&b.drifted_rate.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| (s.bucket, s.clone()))
}

/// The duration bucket with the highest drift rate among buckets holding
/// enough paired blocks. Returns the bucket label and its stat.
pub fn worst_duration_drift(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    threshold: f32,
) -> Option<(&'static str, DriftStat)> {
    let by_duration = drift_by_duration(decrees, inscriptions, threshold);
    by_duration
        .iter()
        .filter(|s| s.drifted_rate.is_some())
        .max_by(|a, b| {
            a.drifted_rate
                .unwrap()
                .partial_cmp(&b.drifted_rate.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| (duration_bucket_label(s.bucket as usize), s.clone()))
}

/// A one-line summary of the overall purpose drift, localized.
///
/// Renders the drift rate plus the paired-block sample that supports it.
/// Digits are Persian when `fa` is true.
pub fn purpose_drift_label(stat: &DriftStat, fa: bool) -> String {
    let num = |n: i64| {
        if fa {
            crate::calendar::jalali::fa_num(n)
        } else {
            n.to_string()
        }
    };
    let pct = |r: f32| {
        let p = (r * 100.0).round() as i64;
        if fa {
            crate::calendar::jalali::fa_num(p)
        } else {
            p.to_string()
        }
    };
    match stat.drifted_rate {
        Some(rate) if fa => format!(
            "{} این که بلوک‌ها همان کاری را کردند که فرمانشان گفته بود ({}٪ از {} بلوک هم‌سو).",
            if rate < 0.5 { "⚠️" } else { "✅" },
            pct(1.0 - rate),
            num(stat.samples as i64),
        ),
        Some(rate) => format!(
            "{} blocks did what their decree promised {}% of the time ({} paired blocks).",
            if rate < 0.5 { "⚠️" } else { "✅" },
            pct(1.0 - rate),
            num(stat.samples as i64),
        ),
        None if fa => format!(
            "هنوز بلوک هم‌سو کافی نیست — تا {} بلوک جفت‌شده، نرخ اعلام نمی‌شود.",
            num(MIN_SAMPLES as i64),
        ),
        None => format!(
            "Not enough paired blocks yet — the rate is withheld below {} paired blocks.",
            num(MIN_SAMPLES as i64),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decree(id: i64, session_at: i64, written_at: i64, text: &str) -> Decree {
        Decree {
            id: format!("d{id}"),
            session_at,
            text: text.to_string(),
            written_at,
            deleted: false,
        }
    }

    fn inscription(session_at: i64, text: &str) -> Inscription {
        Inscription {
            id: format!("i{session_at}"),
            session_at,
            text: text.to_string(),
            written_at: session_at,
            deleted: false,
        }
    }

    #[test]
    fn test_overlap_identical_texts_returns_one() {
        let a = tokenize("write thesis section three");
        assert_eq!(overlap(&a, &a), 1.0);
    }

    #[test]
    fn test_overlap_disjoint_texts_returns_zero() {
        let a = tokenize("write thesis section three");
        let b = tokenize("answered customer emails");
        assert_eq!(overlap(&a, &b), 0.0);
    }

    #[test]
    fn test_tokenize_preserves_zwnj_split() {
        let toks = tokenize("می‌خواهم thesis");
        // ZWNJ split works: the joined form breaks into separate tokens.
        assert!(toks.contains(&"خواهم".to_string()));
        // "می" is only 2 chars and is dropped as a short token.
        assert!(!toks.contains(&"می".to_string()));
        // No token contains a ZWNJ after the split.
        assert!(!toks.iter().any(|t| t.contains('\u{200c}')));
        // "thesis" survives the split unchanged.
        assert!(toks.contains(&"thesis".to_string()));
    }

    #[test]
    fn test_tokenize_drops_short_tokens() {
        let toks = tokenize("a in the on of write");
        assert!(toks.iter().all(|t| t.chars().count() >= 3));
        assert!(toks.contains(&"write".to_string()));
    }

    #[test]
    fn test_tokenize_drops_stopwords() {
        let toks = tokenize("the and for with this was were write");
        assert!(!toks.contains(&"the".to_string()));
        assert!(!toks.contains(&"and".to_string()));
        assert!(toks.contains(&"write".to_string()));
    }

    #[test]
    fn test_drift_overall_none_below_min_samples() {
        let decrees = [decree(1, 100, 0, "write thesis")];
        let inscriptions = [inscription(100, "answered emails")];
        assert!(drift_overall(&decrees, &inscriptions, DEFAULT_THRESHOLD, 5).is_none());
    }

    #[test]
    fn test_drift_overall_counts_drifted() {
        let decrees = [
            decree(1, 100, 0, "write thesis section"),
            decree(2, 200, 100, "write thesis section"),
            decree(3, 300, 200, "write thesis section"),
            decree(4, 400, 300, "write thesis section"),
            decree(5, 500, 400, "write thesis section"),
        ];
        let inscriptions = [
            inscription(100, "wrote thesis section three"),
            inscription(200, "wrote thesis section three"),
            inscription(300, "answered customer emails"),
            inscription(400, "answered customer emails"),
            inscription(500, "answered customer emails"),
        ];
        let overall = drift_overall(&decrees, &inscriptions, DEFAULT_THRESHOLD, 5).unwrap();
        assert_eq!(overall.samples, 5);
        assert_eq!(overall.drifted, 3);
        assert!(overall.drifted_rate.unwrap() > 0.5);
    }

    #[test]
    fn test_drift_by_hour_respects_tz_offset() {
        // session_at = 0 UTC = 03:30 local at +3.5h (12600s).
        let decrees = [decree(1, 0, 0, "write thesis")];
        let inscriptions = [inscription(0, "answered emails")];
        let by = drift_by_hour(&decrees, &inscriptions, 12_600, DEFAULT_THRESHOLD);
        assert_eq!(by[3].samples, 1);
        assert_eq!(by[3].drifted, 1);
        assert_eq!(by[0].samples, 0);
    }

    #[test]
    fn test_drift_by_duration_uses_nearest_bucket() {
        // A 30-minute block (1800s) lands in the 15–34 bucket (index 1).
        let decrees = [decree(1, 1800, 0, "write thesis")];
        let inscriptions = [inscription(1800, "answered emails")];
        let by = drift_by_duration(&decrees, &inscriptions, DEFAULT_THRESHOLD);
        let b = by.iter().find(|s| s.bucket == 1).unwrap();
        assert_eq!(b.samples, 1);
        assert_eq!(duration_bucket_label(1), "15–34");
    }
}
