//! Prescriptions: what to do differently next week.
//!
//! Every analytics function in this app describes the past: the weekly digest
//! mirrors the week's shape, and the drift says whether you are moving, but
//! none of them tells you what to *do* next. This module is the coach. It
//! reads the already-computed analytics and turns them into at most
//! [`MAX_SUGGESTIONS`] concrete suggestions for the week ahead, each tagged
//! with the metric that produced it and the sample size that backs it.
//!
//! A suggestion only fires when its supporting sample clears a per-metric
//! threshold, so a fresh install (few sessions, no calibration, nothing to
//! compare) produces zero suggestions rather than fabricated precision. As
//! the record matures, the suggestions become specific: "plan blocks around
//! 25 minutes", "your unjudged backlog is growing", "your focus is slipping".
//!
//! Everything here is pure: a [`PrescriptionInput`] in, owned suggestions
//! out, no I/O, no clock reads.

use crate::journal::week_drift::WeekDrift;
use crate::stats::report::{BreakAdherence, DurationCalibration};

/// Minimum rated sessions before calibration can advise a block length.
pub const MIN_CALIBRATION_SAMPLES: u32 = 5;
/// Minimum judged sessions this week before the honest-hour advice fires.
pub const MIN_HONEST_HOUR_SAMPLES: u32 = 5;
/// Minimum completed breaks before break advice fires.
pub const MIN_BREAK_SAMPLES: u32 = 3;
/// Minimum unjudged blocks before the backlog advice fires.
pub const MIN_UNJUDGED: u32 = 3;
/// Minimum total baseline sessions before drift advice fires.
pub const MIN_DRIFT_SESSIONS: u32 = 10;
/// Most suggestions a prescription may carry.
pub const MAX_SUGGESTIONS: usize = 3;
/// Break-adherence rate below which the "protect your breaks" advice fires.
pub const BREAK_ADHERENCE_WARN_RATE: f32 = 0.7;

/// What kind of evidence a suggestion is built on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    /// Block-length calibration.
    Calibration,
    /// Break adherence.
    Break,
    /// The most honest hour.
    HonestHour,
    /// Unjudged backlog.
    Unjudged,
    /// Week-over-week drift.
    Drift,
}

/// One suggestion for the week ahead.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// What kind of evidence this suggestion is built on.
    pub kind: SuggestionKind,
    /// The advice, already rendered in the requested locale.
    pub text: String,
    /// The sample size backing this suggestion.
    pub sample_n: u32,
}

/// Everything the prescription reads. All fields are pre-computed so this
/// module stays pure and free of any assembly logic.
#[derive(Clone, Debug)]
pub struct PrescriptionInput<'a> {
    /// Duration calibration across the whole record.
    pub calibration: &'a DurationCalibration,
    /// Recorded break adherence (when enough breaks exist).
    pub break_adherence: Option<&'a BreakAdherence>,
    /// The most honest hour this week (when a qualifying hour exists).
    pub honest_hour: Option<u8>,
    /// Judged sessions this week — the sample behind `honest_hour`.
    pub honest_hour_samples: u32,
    /// Unjudged blocks this week.
    pub unjudged: u32,
    /// The week-over-week drift headline.
    pub drift: Option<&'a WeekDrift>,
    /// Total baseline sessions behind `drift`.
    pub drift_baseline_sessions: u32,
}

/// Render `n` in the requested locale (Persian digits when `fa`).
fn num(n: i64, fa: bool) -> String {
    if fa {
        crate::calendar::jalali::fa_num(n)
    } else {
        n.to_string()
    }
}

/// A human noun for a drift metric, in the requested locale.
fn metric_noun(metric: &str, fa: bool) -> String {
    match (fa, metric) {
        (true, "sessions") => "جلسه‌ها".to_string(),
        (true, "focus_minutes") => "دقیقهٔ تمرکز".to_string(),
        (true, "kept_rate") => "نرخ نگه‌داشتن".to_string(),
        (true, "fulfilment_rate") => "نرخ انجام‌شدن".to_string(),
        (true, "break_adherence") => "پایبندی به استراحت".to_string(),
        (true, "unjudged") => "بلوک‌های بدون داوری".to_string(),
        (false, "sessions") => "sessions".to_string(),
        (false, "focus_minutes") => "focus minutes".to_string(),
        (false, "kept_rate") => "kept rate".to_string(),
        (false, "fulfilment_rate") => "fulfilment rate".to_string(),
        (false, "break_adherence") => "break adherence".to_string(),
        (false, "unjudged") => "unjudged blocks".to_string(),
        (_, m) => m.to_string(),
    }
}

/// Build the calibration suggestion, when the sample is big enough.
fn calibration_suggestion(cal: &DurationCalibration, fa: bool) -> Option<Suggestion> {
    if cal.total_rated < MIN_CALIBRATION_SAMPLES {
        return None;
    }
    if let Some(spot) = cal.sweet_spot {
        let text = if fa {
            format!(
                "بلوک‌های خود را حدود {} دقیقه برنامه‌ریزی کنید — در این طول بهترین نگه‌داشتن را دارید.",
                num(spot as i64, true)
            )
        } else {
            format!("Plan blocks around {spot} minutes — that is the length you keep best.")
        };
        return Some(Suggestion {
            kind: SuggestionKind::Calibration,
            text,
            sample_n: cal.total_rated,
        });
    }
    if let Some(ceiling) = cal.overreach_floor {
        let text = if fa {
            format!(
                "بلوک‌های بلندتر از {} دقیقه معمولاً شکست می‌خورند؛ برنامه‌ریزی را زیر این سقف نگه دارید.",
                num(ceiling as i64, true)
            )
        } else {
            format!("Blocks past {ceiling} minutes tend to fail — keep plans under that ceiling.")
        };
        return Some(Suggestion {
            kind: SuggestionKind::Calibration,
            text,
            sample_n: cal.total_rated,
        });
    }
    None
}

/// Build the break suggestion, when enough breaks exist and adherence is low.
fn break_suggestion(adh: &BreakAdherence, fa: bool) -> Option<Suggestion> {
    let total = adh.followed + adh.skipped + adh.stretched;
    if total < MIN_BREAK_SAMPLES {
        return None;
    }
    if adh.rate >= BREAK_ADHERENCE_WARN_RATE {
        return None;
    }
    let text = if fa {
        format!(
            "شما استراحت‌ها را کوتاه یا حذف می‌کنید ({} استراحت ثبت شده). فضای بین بلوک‌ها را محافظت کنید.",
            num(total as i64, true)
        )
    } else {
        format!(
            "You are cutting breaks short or skipping them ({total} recorded). Protect the space between blocks."
        )
    };
    Some(Suggestion {
        kind: SuggestionKind::Break,
        text,
        sample_n: total,
    })
}

/// Build the honest-hour suggestion, when enough sessions were judged.
fn honest_hour_suggestion(hour: u8, samples: u32, fa: bool) -> Option<Suggestion> {
    if samples < MIN_HONEST_HOUR_SAMPLES {
        return None;
    }
    let text = if fa {
        format!(
            "شما در ساعت {} صادق‌ترین هستید — سخت‌ترین بلوک را همان‌جا بگذارید.",
            num(hour as i64, true)
        )
    } else {
        format!("You are most honest around {hour:02}:00 — put your hardest block there.")
    };
    Some(Suggestion {
        kind: SuggestionKind::HonestHour,
        text,
        sample_n: samples,
    })
}

/// Build the unjudged suggestion, when the backlog is real.
fn unjudged_suggestion(count: u32, fa: bool) -> Option<Suggestion> {
    if count < MIN_UNJUDGED {
        return None;
    }
    let text = if fa {
        format!(
            "شما از {} بلوک بدون داوری گذشتید — پیش از پایان هفته با آن‌ها روبه‌رو شوید.",
            num(count as i64, true)
        )
    } else {
        format!(
            "You walked away from {count} blocks without judging them — face them before the week ends."
        )
    };
    Some(Suggestion {
        kind: SuggestionKind::Unjudged,
        text,
        sample_n: count,
    })
}

/// Build the drift suggestion, when the baseline is large enough.
fn drift_suggestion(d: &WeekDrift, baseline_sessions: u32, fa: bool) -> Option<Suggestion> {
    if baseline_sessions < MIN_DRIFT_SESSIONS {
        return None;
    }
    let noun = metric_noun(d.metric, fa);
    let text = if d.better {
        if fa {
            format!("{noun} بزرگ‌ترین بهبود شماست — این عادت را حفظ کنید.")
        } else {
            format!("Your {noun} improved most — protect that habit.")
        }
    } else if fa {
        format!("{noun} بیشتر از همه افت کرده — این هفته به آن بپردازید.")
    } else {
        format!("Your {noun} slipped most — address it this week.")
    };
    Some(Suggestion {
        kind: SuggestionKind::Drift,
        text,
        sample_n: baseline_sessions,
    })
}

/// Produce at most [`MAX_SUGGESTIONS`] suggestions for next week.
///
/// Candidates are gathered in priority order (drift, calibration, unjudged,
/// honest hour, break) and only the first [`MAX_SUGGESTIONS`] that clear
/// their thresholds survive. Returns an empty vector for a fresh or sparse
/// record.
pub fn prescribe(input: &PrescriptionInput<'_>, fa: bool) -> Vec<Suggestion> {
    let mut out = Vec::with_capacity(MAX_SUGGESTIONS);

    let mut push_if = |s: Option<Suggestion>| {
        if let Some(s) = s {
            out.push(s);
        }
    };

    // Drift (most current, highest evidence first).
    if let Some(d) = input.drift {
        push_if(drift_suggestion(d, input.drift_baseline_sessions, fa));
    }
    // Calibration.
    push_if(calibration_suggestion(input.calibration, fa));
    // Unjudged backlog.
    push_if(unjudged_suggestion(input.unjudged, fa));
    // Honest hour.
    if let Some(hour) = input.honest_hour {
        push_if(honest_hour_suggestion(hour, input.honest_hour_samples, fa));
    }
    // Break adherence.
    if let Some(adh) = input.break_adherence {
        push_if(break_suggestion(adh, fa));
    }

    out.truncate(MAX_SUGGESTIONS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(rated: u32, sweet: Option<u64>, ceiling: Option<u64>) -> DurationCalibration {
        DurationCalibration {
            buckets: Vec::new(),
            sweet_spot: sweet,
            overreach_floor: ceiling,
            total_rated: rated,
            unknown: 0,
        }
    }

    fn adh(followed: u32, skipped: u32, stretched: u32) -> BreakAdherence {
        let total = followed + skipped + stretched;
        let rate = if total == 0 {
            0.0
        } else {
            followed as f32 / total as f32
        };
        BreakAdherence {
            followed,
            skipped,
            stretched,
            rate,
        }
    }

    fn drift(better: bool) -> WeekDrift {
        WeekDrift {
            metric: "sessions",
            delta: 3.0,
            baseline: 5.0,
            better,
        }
    }

    fn input<'a>(
        cal: &'a DurationCalibration,
        break_adh: Option<&'a BreakAdherence>,
        driftv: Option<&'a WeekDrift>,
    ) -> PrescriptionInput<'a> {
        PrescriptionInput {
            calibration: cal,
            break_adherence: break_adh,
            honest_hour: None,
            honest_hour_samples: 0,
            unjudged: 0,
            drift: driftv,
            drift_baseline_sessions: 0,
        }
    }

    #[test]
    fn test_prescription_empty_analytics_returns_empty_vec() {
        // A fresh record: no calibration, no breaks, no honest hour, no
        // unjudged, no drift. Must yield zero suggestions, not fabricated
        // precision.
        let c = cal(0, None, None);
        let i = input(&c, None, None);
        let p = prescribe(&i, false);
        assert!(p.is_empty(), "fresh record should advise nothing: {p:?}");
    }

    #[test]
    fn calibration_fires_above_threshold_with_sweet_spot() {
        let c = cal(7, Some(25), None);
        let i = input(&c, None, None);
        let p = prescribe(&i, false);
        assert!(!p.is_empty());
        assert_eq!(p[0].kind, SuggestionKind::Calibration);
        assert!(
            p[0].text.contains("25"),
            "states the sweet spot: {}",
            p[0].text
        );
        assert_eq!(p[0].sample_n, 7);
    }

    #[test]
    fn calibration_silent_below_threshold() {
        let c = cal(3, Some(25), None);
        let i = input(&c, None, None);
        assert!(prescribe(&i, false).is_empty());
    }

    #[test]
    fn break_fires_when_adherence_low() {
        let c = cal(0, None, None);
        let b = adh(1, 4, 1); // rate 0.167, total 6
        let i = input(&c, Some(&b), None);
        let p = prescribe(&i, false);
        assert!(p.iter().any(|s| s.kind == SuggestionKind::Break));
        let bs = p.iter().find(|s| s.kind == SuggestionKind::Break).unwrap();
        assert!(
            bs.text.contains("6"),
            "carries the break count: {}",
            bs.text
        );
    }

    #[test]
    fn break_silent_when_adherence_good() {
        let c = cal(0, None, None);
        let b = adh(5, 1, 0); // rate 0.833, total 6
        let i = input(&c, Some(&b), None);
        assert!(!prescribe(&i, false)
            .iter()
            .any(|s| s.kind == SuggestionKind::Break));
    }

    #[test]
    fn honest_hour_fires_above_threshold() {
        let c = cal(0, None, None);
        let mut i = input(&c, None, None);
        i.honest_hour = Some(9);
        i.honest_hour_samples = 6;
        let p = prescribe(&i, false);
        assert!(p.iter().any(|s| s.kind == SuggestionKind::HonestHour));
        let hs = p
            .iter()
            .find(|s| s.kind == SuggestionKind::HonestHour)
            .unwrap();
        assert!(hs.text.contains("09:00"), "states the hour: {}", hs.text);
    }

    #[test]
    fn unjudged_fires_above_threshold() {
        let c = cal(0, None, None);
        let mut i = input(&c, None, None);
        i.unjudged = 4;
        let p = prescribe(&i, false);
        assert!(p.iter().any(|s| s.kind == SuggestionKind::Unjudged));
    }

    #[test]
    fn drift_fires_only_with_large_baseline() {
        // Large baseline → drift advice appears.
        let c = cal(0, None, None);
        let d = drift(true);
        let mut i = input(&c, None, Some(&d));
        i.drift_baseline_sessions = 40;
        let p = prescribe(&i, false);
        assert!(p.iter().any(|s| s.kind == SuggestionKind::Drift));

        // Small baseline → drift advice is withheld.
        let c2 = cal(0, None, None);
        let d2 = drift(true);
        let mut j = input(&c2, None, Some(&d2));
        j.drift_baseline_sessions = 4;
        assert!(!prescribe(&j, false)
            .iter()
            .any(|s| s.kind == SuggestionKind::Drift));
    }

    #[test]
    fn max_three_suggestions() {
        // Fire every candidate → must truncate to MAX_SUGGESTIONS.
        let c = cal(8, Some(25), None);
        let b = adh(1, 4, 1);
        let d = drift(false);
        let mut i = input(&c, Some(&b), Some(&d));
        i.honest_hour = Some(9);
        i.honest_hour_samples = 6;
        i.unjudged = 4;
        i.drift_baseline_sessions = 40;
        let p = prescribe(&i, false);
        assert!(p.len() <= MAX_SUGGESTIONS);
        assert_eq!(p.len(), MAX_SUGGESTIONS);
        // Priority order: drift first, then calibration, then unjudged.
        assert_eq!(p[0].kind, SuggestionKind::Drift);
        assert_eq!(p[1].kind, SuggestionKind::Calibration);
        assert_eq!(p[2].kind, SuggestionKind::Unjudged);
    }

    #[test]
    fn persian_digits_when_locale_fa() {
        let c = cal(7, Some(25), None);
        let mut i = input(&c, None, None);
        i.honest_hour = Some(9);
        i.honest_hour_samples = 6;
        let p = prescribe(&i, true);
        assert!(!p.is_empty());
        assert!(
            p[0].text.contains("۲۵"),
            "Persian digit for 25: {}",
            p[0].text
        );
    }
}
