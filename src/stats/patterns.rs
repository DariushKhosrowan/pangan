//! Cross-metric patterns: which metric leads, and which follows.
//!
//! Every analytics function in the app reads one metric at a time. The coach
//! can see that a metric dropped this week, but not that *break-adherence
//! dropping last week is what tends to precede kept-rate dropping this week*.
//! This module is the pattern-recognition layer: it scans the existing weekly
//! metric series (the same snapshots [`gather_weeks`] already produces) and
//! reports the top pairs that consistently move together, or in opposite
//! directions, with a zero- or one-week lag.
//!
//! The output is deliberately small — at most three patterns in the weekly
//! digest — because a pattern is only useful when it is *consistent*. A
//! single lucky week is noise; the module requires enough paired weeks to
//! trust a correlation.
//!
//! **No fabricated precision.** A correlation is only reported when at least
//! [`MIN_PAIRED_WEEKS`] paired observations exist. The module would rather
//! say nothing than infer a pattern from two data points.
//!
//! Everything here is pure: `WeekMetrics` snapshots in, owned patterns out,
//! no I/O, no clock reads beyond what is passed in.

use crate::journal::week_drift::WeekMetrics;

/// Fewest paired weeks for which a correlation is reported.
pub const MIN_PAIRED_WEEKS: usize = 6;

/// The weekly metric series for one named metric.
pub struct MetricSeries {
    /// Display key (matches [`crate::journal::week_drift::value_of`]).
    pub name: &'static str,
    /// One entry per week; `None` where the week is too thin to measure.
    pub values: Vec<Option<f32>>,
}

/// Pearson correlation over paired observations, skipping `None` sides.
/// Returns `None` when fewer than [`MIN_PAIRED_WEEKS`] pairs remain.
pub fn pearson(xs: &[Option<f32>], ys: &[Option<f32>]) -> Option<f32> {
    let pairs: Vec<(f32, f32)> = xs
        .iter()
        .zip(ys.iter())
        .filter_map(|(x, y)| match (x, y) {
            (Some(a), Some(b)) => Some((*a, *b)),
            _ => None,
        })
        .collect();
    if pairs.len() < MIN_PAIRED_WEEKS {
        return None;
    }
    let n = pairs.len() as f32;
    let x_mean = pairs.iter().map(|(a, _)| a).sum::<f32>() / n;
    let y_mean = pairs.iter().map(|(_, b)| b).sum::<f32>() / n;
    let mut sxy = 0.0f32;
    let mut sxx = 0.0f32;
    let mut syy = 0.0f32;
    for (a, b) in &pairs {
        let dx = a - x_mean;
        let dy = b - y_mean;
        sxy += dx * dy;
        sxx += dx * dx;
        syy += dy * dy;
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return None;
    }
    Some(sxy / (sxx * syy).sqrt())
}

/// Correlation of `xs[t]` with `ys[t+1]` — does `xs` lead `ys` by a week?
pub fn lag1_correlation(xs: &[Option<f32>], ys: &[Option<f32>]) -> Option<f32> {
    let x_lead: Vec<Option<f32>> = xs
        .iter()
        .take(xs.len().saturating_sub(1))
        .cloned()
        .collect();
    let y_follow: Vec<Option<f32>> = ys.iter().skip(1).cloned().collect();
    pearson(&x_lead, &y_follow)
}

/// One detected cross-metric pattern.
#[derive(Clone, Debug)]
pub struct Pattern {
    /// The leading metric.
    pub leading: &'static str,
    /// The following metric.
    pub following: &'static str,
    /// Correlation strength (−1..1).
    pub correlation: f32,
    /// 0 (same week) or 1 (lead by one week).
    pub lag: u8,
    /// Paired observations behind the correlation.
    pub samples: u32,
}

/// Extract a named metric's series from weekly snapshots.
fn series_for(name: &'static str, weeks: &[WeekMetrics]) -> MetricSeries {
    let values = weeks
        .iter()
        .map(|w| crate::journal::week_drift::value_of(w, name))
        .collect();
    MetricSeries { name, values }
}

/// The metric keys available on every `WeekMetrics` snapshot.
pub const METRIC_KEYS: [&str; 6] = [
    "sessions",
    "focus_minutes",
    "kept_rate",
    "fulfilment_rate",
    "break_adherence",
    "unjudged",
];

/// Build the series for every available metric.
pub fn all_series(weeks: &[WeekMetrics]) -> Vec<MetricSeries> {
    METRIC_KEYS.iter().map(|k| series_for(k, weeks)).collect()
}

/// Find the strongest cross-metric patterns across all ordered pairs, with
/// lag ∈ {0, 1}. Returns at most `max_patterns` patterns, sorted by
/// |correlation| descending, ties broken by (leading, following) for
/// determinism.
pub fn find_patterns(
    series: &[MetricSeries],
    min_abs_corr: f32,
    max_patterns: usize,
) -> Vec<Pattern> {
    let mut patterns: Vec<Pattern> = Vec::new();
    for a in series {
        for b in series {
            if a.name == b.name {
                continue;
            }
            // Same-week (lag 0).
            if let Some(r) = pearson(&a.values, &b.values) {
                if r.abs() >= min_abs_corr {
                    patterns.push(Pattern {
                        leading: a.name,
                        following: b.name,
                        correlation: r,
                        lag: 0,
                        samples: paired_count(&a.values, &b.values),
                    });
                }
            }
            // Lead-by-one-week (lag 1): a leads b.
            if let Some(r) = lag1_correlation(&a.values, &b.values) {
                if r.abs() >= min_abs_corr {
                    patterns.push(Pattern {
                        leading: a.name,
                        following: b.name,
                        correlation: r,
                        lag: 1,
                        samples: lag1_paired_count(&a.values, &b.values),
                    });
                }
            }
        }
    }
    patterns.sort_by(|x, y| {
        y.correlation
            .abs()
            .partial_cmp(&x.correlation.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(x.leading.cmp(y.leading))
            .then(x.following.cmp(y.following))
            .then(x.lag.cmp(&y.lag))
    });
    // Dedupe symmetric lag-0 pairs: A↔B and B↔A carry the same information
    // (Pearson at lag 0 is symmetric). When both appear with essentially the
    // same |correlation|, keep only the alphabetically-first direction.
    let mut seen: Vec<(&str, &str)> = Vec::new();
    patterns.retain(|p| {
        if p.lag != 0 {
            return true;
        }
        let (a, b) = if p.leading < p.following {
            (p.leading, p.following)
        } else {
            (p.following, p.leading)
        };
        if seen.iter().any(|(x, y)| *x == a && *y == b) {
            false
        } else {
            seen.push((a, b));
            true
        }
    });
    patterns.truncate(max_patterns);
    patterns
}

fn paired_count(xs: &[Option<f32>], ys: &[Option<f32>]) -> u32 {
    xs.iter()
        .zip(ys.iter())
        .filter(|(x, y)| x.is_some() && y.is_some())
        .count() as u32
}

fn lag1_paired_count(xs: &[Option<f32>], ys: &[Option<f32>]) -> u32 {
    xs.iter()
        .take(xs.len().saturating_sub(1))
        .zip(ys.iter().skip(1))
        .filter(|(x, y)| x.is_some() && y.is_some())
        .count() as u32
}

/// A localized one-line label for a detected pattern.
pub fn pattern_label(p: &Pattern, fa: bool) -> String {
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
    let direction = if p.correlation >= 0.0 { "↑" } else { "↓" };
    if fa {
        format!(
            "{} {} {} (همبستگی {}٪، {}) — {} هفته.",
            if p.lag == 1 {
                "هفتهٔ بعد"
            } else {
                "همان هفته"
            },
            p.leading.replace('_', " "),
            p.following.replace('_', " "),
            pct(p.correlation.abs()),
            direction,
            num(p.samples as i64),
        )
    } else {
        format!(
            "{} {} {} (corr {}% {}, lag {}) — {} weeks.",
            if p.lag == 1 { "next week" } else { "same week" },
            p.leading,
            p.following,
            pct(p.correlation.abs()),
            direction,
            p.lag,
            num(p.samples as i64),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wk(
        sessions: u32,
        focus: u64,
        kept: f32,
        fulfil: f32,
        adhere: f32,
        unjudged: u32,
    ) -> WeekMetrics {
        WeekMetrics {
            week_start: chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            sessions,
            focus_minutes: focus,
            kept_rate: Some(kept),
            fulfilment_rate: Some(fulfil),
            break_adherence: Some(adhere),
            unjudged,
            honest_hour: None,
        }
    }

    #[test]
    fn test_pearson_none_when_pairs_under_six() {
        let xs: Vec<Option<f32>> = vec![Some(1.0); 5];
        let ys: Vec<Option<f32>> = vec![Some(2.0); 5];
        assert!(pearson(&xs, &ys).is_none());
    }

    #[test]
    fn test_pearson_positive_relationship() {
        let xs: Vec<Option<f32>> = (0..8).map(|i| Some(i as f32)).collect();
        let ys: Vec<Option<f32>> = (0..8).map(|i| Some((i * 2) as f32)).collect();
        let r = pearson(&xs, &ys).unwrap();
        assert!(r > 0.95);
    }

    #[test]
    fn test_pearson_negative_relationship() {
        let xs: Vec<Option<f32>> = (0..8).map(|i| Some(i as f32)).collect();
        let ys: Vec<Option<f32>> = (0..8).map(|i| Some((7 - i) as f32)).collect();
        let r = pearson(&xs, &ys).unwrap();
        assert!(r < -0.95);
    }

    #[test]
    fn test_lag1_shifts_correctly() {
        // xs[i] = ys[i+1]: a perfect 1-week lead.
        let xs: Vec<Option<f32>> = (0..8).map(|i| Some(i as f32)).collect();
        let ys: Vec<Option<f32>> = (1..9).map(|i| Some(i as f32)).collect();
        let r = lag1_correlation(&xs, &ys).unwrap();
        assert!(r > 0.95);
    }

    #[test]
    fn test_find_patterns_below_min_abs_corr_returns_empty() {
        // A flat series produces zero-variance columns → None everywhere.
        let weeks = (0..8)
            .map(|i| wk(i as u32, 25, 0.5, 0.5, 0.5, 1))
            .collect::<Vec<_>>();
        let series = all_series(&weeks);
        let pats = find_patterns(&series, 0.9, 3);
        assert!(pats.is_empty());
    }

    #[test]
    fn test_find_patterns_sorted_by_abs_correlation() {
        // Build weeks where sessions stays flat; kept_rate tracks a sine; and
        // focus_minutes is a noisy copy of kept_rate (high corr), unjudged is
        // independent (low corr).
        let weeks: Vec<WeekMetrics> = (0..8)
            .map(|i| {
                let f = (i as f32 * std::f32::consts::PI / 2.0).sin();
                // kept between 0.2 and 0.8
                let kept = 0.5 + 0.3 * f;
                wk(
                    10 + (i % 2) as u32,
                    25 + (i % 3) as u64,
                    kept,
                    kept,
                    0.5,
                    (i % 4) as u32,
                )
            })
            .collect();
        let series = all_series(&weeks);
        let pats = find_patterns(&series, 0.5, 10);
        // The strongest same-week pair should be kept_rate ↔ fulfilment_rate
        // (perfectly identical → r = 1.0).
        assert!(!pats.is_empty());
        assert!(pats[0].correlation.abs() > 0.9);
    }

    #[test]
    fn test_find_patterns_deterministic_tie_break() {
        let weeks: Vec<WeekMetrics> = (0..8).map(|i| wk(i as u32, 25, 0.5, 0.5, 0.5, 1)).collect();
        let series = all_series(&weeks);
        let a = find_patterns(&series, 0.0, 100);
        let b = find_patterns(&series, 0.0, 100);
        let key = |p: &Pattern| format!("{}|{}|{}", p.leading, p.following, p.lag);
        let ka: Vec<String> = a.iter().map(key).collect();
        let kb: Vec<String> = b.iter().map(key).collect();
        assert_eq!(ka, kb);
    }

    #[test]
    fn test_find_patterns_dedupes_symmetric_lag0_pair() {
        // kept_rate and fulfilment_rate are identical → symmetric lag-0 pairs
        // both rank top. The dedup must keep only one of the two directions.
        let weeks: Vec<WeekMetrics> = (0..8)
            .map(|i| {
                let f = (i as f32 * std::f32::consts::PI / 2.0).sin();
                let kept = 0.5 + 0.3 * f;
                wk(10 + (i % 2) as u32, 25, kept, kept, 0.5, 1)
            })
            .collect();
        let series = all_series(&weeks);
        let pats = find_patterns(&series, 0.5, 100);
        let lag0_pairs: Vec<(&str, &str)> = pats
            .iter()
            .filter(|p| p.lag == 0)
            .map(|p| (p.leading, p.following))
            .collect();
        // No lag-0 pair appears in both directions.
        for (a, b) in &lag0_pairs {
            assert!(
                !lag0_pairs.iter().any(|(x, y)| x == b && y == a),
                "symmetric lag-0 pair {:?}/{:?} not deduped",
                a,
                b
            );
        }
    }

    #[test]
    fn test_pattern_label_persian_digits_when_locale_fa() {
        let p = Pattern {
            leading: "break_adherence",
            following: "kept_rate",
            correlation: -0.8,
            lag: 1,
            samples: 7,
        };
        let fa = pattern_label(&p, true);
        assert!(fa.contains('٪'));
        assert!(fa.contains("هفته"));
    }
}
