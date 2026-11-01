//! Season coherence: does this week's work actually serve the season?
//!
//! The app now checks intention at every layer of the hierarchy — season →
//! week → day → block — except the top. Season intentions exist
//! (`season_intention`), but nothing ever asks whether this week's decrees
//! are aligned with what the user said the season was *for*. This module is
//! that missing check.
//!
//! It reuses the token-overlap heuristic from [`crate::journal::purpose_drift`]:
//! each live decree in the week is compared against the season's live
//! intention, and a decree is *coherent* when its tokens overlap the
//! intention's by at least a small threshold. Coherence is deliberately
//! loose — a single shared token is signal enough at this abstract level,
//! because the question is "am I working on the season at all", not "did I
//! match the exact wording".
//!
//! **No fabricated precision.** A coherence rate is only reported when enough
//! live decrees exist, and the worst-coherent day is only named when it has
//! at least two samples. The module would rather say nothing than infer a
//! pattern from two data points.
//!
//! Everything here is pure: decrees, the season intention, and the timezone
//! offset in; an owned stat out; no I/O.

use crate::journal::decree::Decree;
use crate::journal::purpose_drift::{overlap, tokenize};
use crate::journal::season_intention::SeasonIntention;

/// A decree's decay is judged coherent when its token overlap with the
/// season intention reaches this bar. A single shared token is enough signal
/// at this abstract level; the user should not have to tune it.
pub const SEASON_COHERENCE_THRESHOLD: f32 = 0.15;

/// Minimum live decrees in the week for a coherence rate to be reported.
pub const MIN_LIVE_DECREES: usize = 3;

/// Minimum samples before a per-day coherence rate is named.
pub const MIN_DAY_SAMPLES: u32 = 2;

/// One decree measured against the season intention.
#[derive(Clone, Debug)]
pub struct CoherenceSample {
    /// The session this decree belongs to.
    pub session_at: i64,
    /// Token count of the decree (after filtering).
    pub decree_tokens: u32,
    /// How many of the intention's tokens matched.
    pub matched: u32,
    /// `overlap >= threshold`.
    pub coherent: bool,
}

/// The season-coherence picture for a week.
#[derive(Clone, Debug)]
pub struct CoherenceStat {
    /// Live decrees measured.
    pub samples: u32,
    /// Of those, the coherent ones.
    pub coherent: u32,
    /// `coherent / samples`. `None` while `samples < MIN_LIVE_DECREES`.
    pub coherent_rate: Option<f32>,
    /// The local-day key (Unix seconds of that day's start) with the lowest
    /// per-day coherence rate, present only when that day has at least
    /// [`MIN_DAY_SAMPLES`] samples.
    pub worst_day_start: Option<i64>,
}

/// Measure the week's decrees against the season's live intention.
///
/// Returns `None` when there is no live intention for the season, or when
/// fewer than [`MIN_LIVE_DECREES`] live decrees are available.
pub fn season_coherence(
    decrees: &[Decree],
    intention: Option<&SeasonIntention>,
    tz_offset_secs: i32,
    threshold: f32,
    min_samples: usize,
) -> Option<CoherenceStat> {
    let intention = intention?;
    let intention_tokens = tokenize(&intention.body);
    if intention_tokens.is_empty() {
        return None;
    }
    let live: Vec<&Decree> = decrees.iter().filter(|d| !d.deleted).collect();
    if live.len() < min_samples {
        return None;
    }
    let samples = live.len() as u32;
    let mut coherent = 0u32;
    let mut per_day: std::collections::HashMap<i64, (u32, u32)> = std::collections::HashMap::new();
    for d in &live {
        let dt = tokenize(&d.text);
        let is_coherent = overlap(&dt, &intention_tokens) >= threshold;
        if is_coherent {
            coherent += 1;
        }
        let day = local_day_start(d.session_at, tz_offset_secs);
        let e = per_day.entry(day).or_insert((0, 0));
        e.0 += 1;
        if is_coherent {
            e.1 += 1;
        }
    }
    let worst_day_start = per_day
        .iter()
        .filter(|(_, (s, _))| *s >= MIN_DAY_SAMPLES)
        .min_by(|(_, (sa, ca)), (_, (sb, cb))| {
            let ra = *ca as f32 / *sa as f32;
            let rb = *cb as f32 / *sb as f32;
            ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(day, _)| *day);
    Some(CoherenceStat {
        samples,
        coherent,
        coherent_rate: if samples < min_samples as u32 {
            None
        } else {
            Some(coherent as f32 / samples as f32)
        },
        worst_day_start,
    })
}

/// The local-day start (Unix seconds) for a session timestamp.
fn local_day_start(session_at: i64, tz_offset_secs: i32) -> i64 {
    let local = session_at.wrapping_add(tz_offset_secs as i64);
    let day_start_local = local.div_euclid(86_400) * 86_400;
    day_start_local.wrapping_sub(tz_offset_secs as i64)
}

/// A one-line summary of the season coherence, localized.
pub fn coherence_label(stat: &CoherenceStat, fa: bool) -> String {
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
    let rate = stat.coherent_rate.unwrap_or(0.0);
    let day = stat.worst_day_start.map(|d| {
        if fa {
            crate::calendar::jalali::fa_num(d)
        } else {
            d.to_string()
        }
    });
    if fa {
        let mut s = format!(
            "این هفته {}٪ قرارها با نیت فصل هم‌سو بود ({} از {}).",
            pct(rate),
            num(stat.coherent as i64),
            num(stat.samples as i64),
        );
        if let Some(d) = day {
            s.push_str(&format!(" ضعیف‌ترین روز: {}.", d));
        }
        s
    } else {
        let mut s = format!(
            "{}% of decrees were coherent with the season intention ({} of {}).",
            pct(rate),
            num(stat.coherent as i64),
            num(stat.samples as i64),
        );
        if let Some(d) = day {
            s.push_str(&format!(" Worst day: {}.", d));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decree(id: i64, session_at: i64, text: &str, deleted: bool) -> Decree {
        Decree {
            id: format!("d{id}"),
            session_at,
            text: text.to_string(),
            written_at: session_at - 1500,
            deleted,
        }
    }

    fn intention(body: &str, deleted: bool) -> SeasonIntention {
        SeasonIntention {
            id: 1,
            jalali_year: 1405,
            season: 0,
            body: body.to_string(),
            created_at: 0,
            deleted,
        }
    }

    #[test]
    fn test_season_coherence_none_when_intention_missing() {
        // Deleted intention → None.
        let decrees = vec![decree(1, 100, "write thesis", false)];
        assert!(season_coherence(
            &decrees,
            Some(&intention("finish the book", true)),
            0,
            SEASON_COHERENCE_THRESHOLD,
            MIN_LIVE_DECREES,
        )
        .is_none());
        // No intention at all → None.
        assert!(season_coherence(
            &decrees,
            None,
            0,
            SEASON_COHERENCE_THRESHOLD,
            MIN_LIVE_DECREES,
        )
        .is_none());
        // Fewer than min_samples live decrees → None.
        assert!(season_coherence(
            &decrees,
            Some(&intention("finish the book", false)),
            0,
            SEASON_COHERENCE_THRESHOLD,
            MIN_LIVE_DECREES,
        )
        .is_none());
    }

    #[test]
    fn test_season_coherence_counts_coherent_decrees() {
        let intention = intention("finish the book", false);
        let decrees = vec![
            decree(1, 100, "write the book", false),
            decree(2, 200, "write the book", false),
            decree(3, 300, "write the book", false),
            decree(4, 400, "answered emails", false),
            decree(5, 500, "answered emails", false),
        ];
        let stat =
            season_coherence(&decrees, Some(&intention), 0, SEASON_COHERENCE_THRESHOLD, 3).unwrap();
        assert_eq!(stat.samples, 5);
        assert_eq!(stat.coherent, 3);
        assert!(stat.coherent_rate.unwrap() > 0.5);
    }

    #[test]
    fn test_season_coherence_skips_deleted_decrees() {
        let intention = intention("finish the book", false);
        let decrees = vec![
            decree(1, 100, "write the book", false),
            decree(2, 200, "write the book", false),
            decree(3, 300, "write the book", false),
            decree(4, 400, "answered emails", true), // deleted, ignored
        ];
        let stat =
            season_coherence(&decrees, Some(&intention), 0, SEASON_COHERENCE_THRESHOLD, 3).unwrap();
        assert_eq!(stat.samples, 3);
        assert_eq!(stat.coherent, 3);
    }

    #[test]
    fn test_season_coherence_worst_day_only_when_enough_samples() {
        let intention = intention("finish the book", false);
        // Sessions on two days; the "answered emails" day has 2 samples and
        // is the worst; the "write the book" day has 1 sample (ignored).
        let decrees = vec![
            decree(1, 100, "answered emails", false),    // day 0
            decree(2, 12_600, "answered emails", false), // day 0 (same day? no)
            decree(3, 100_000, "write the book", false), // day 1
            decree(4, 200_000, "write the book", false), // day 2
            decree(5, 300_000, "write the book", false), // day 3
        ];
        // tz offset 0 → session_at / 86400 gives the day.
        let stat =
            season_coherence(&decrees, Some(&intention), 0, SEASON_COHERENCE_THRESHOLD, 3).unwrap();
        // The day with two "answered emails" (incoherent) samples is the worst
        // and qualifies (>= 2 samples).
        assert_eq!(stat.worst_day_start, Some(0));
    }

    #[test]
    fn test_coherence_label_persian_digits_when_locale_fa() {
        let stat = CoherenceStat {
            samples: 5,
            coherent: 3,
            coherent_rate: Some(0.6),
            worst_day_start: Some(0),
        };
        let fa = coherence_label(&stat, true);
        assert!(fa.contains('٪'));
        assert!(fa.contains("هم‌سو"));
    }
}
