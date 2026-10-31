//! Standing promises: the same decree broken on repeat.
//!
//! The most painful pattern in the journal is not a single broken promise — it
//! is the *same* promise broken every week, which the record cannot currently
//! see because every decree lives only in its own row. This module normalises
//! decree text so the same intention written at different times can be joined,
//! then reports the promises that have stood unmet (3+ occurrences, no Kept
//! verdict in the window).
//!
//! Pure: takes slices and explicit parameters, no I/O, no clock reads.

use crate::journal::{Decree, Verdict, VerdictKind};
use crate::state::store::Data;

/// One intention that has been repeated and not kept.
#[derive(Clone, Debug, PartialEq)]
pub struct StandingPromise {
    /// The normalised text shared by every occurrence of this promise.
    pub normalized_text: String,
    /// The most recent original spelling the user wrote (for display).
    pub original_sample: String,
    /// How many times the promise was written in the window.
    pub count: u32,
    /// Unix seconds of the earliest occurrence in the window.
    pub first_seen_unix: i64,
    /// Unix seconds of the latest occurrence in the window.
    pub last_seen_unix: i64,
    /// The verdict kinds seen on the occurrences (only Broken/Partial survive
    /// the standing filter).
    pub verdict_kinds: Vec<VerdictKind>,
}

/// Normalise a decree's text so two phrasings of the same intention compare
/// equal: lowercased, whitespace-collapsed, tokens sorted, joined with single
/// spaces. Persian compounds joined with ZWNJ stay whole (the joiner is part
/// of the word), matching the reckoning tokenizer.
pub fn normalize_decree(text: &str) -> String {
    let mut tokens: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            let mut cur = String::new();
            for c in w.chars() {
                if c.is_alphanumeric() || c == '\u{200c}' {
                    cur.extend(c.to_lowercase());
                }
            }
            cur
        })
        .filter(|w| !w.is_empty())
        .collect();
    tokens.sort();
    tokens.join(" ")
}

/// Find promises that have stood unmet across the window.
///
/// `lookback_secs` bounds how far back to look; `min_count` is the repeat
/// threshold (the architect's default is 3). A promise qualifies only when
/// every verdict seen on it is Broken or Partial — a single Kept verdict
/// (anywhere in the window) means it was ultimately delivered and is not
/// standing unmet. Sorted by count desc, then recency desc.
pub fn standing_promises(
    data: &Data,
    now_unix: i64,
    lookback_secs: i64,
    min_count: u32,
) -> Vec<StandingPromise> {
    let window_start = now_unix - lookback_secs;

    // Map normalized text -> (count, first, last, verdict kinds, sample).
    let mut map: std::collections::BTreeMap<String, StandingPromise> =
        std::collections::BTreeMap::new();

    let verdict_for: std::collections::HashMap<i64, &Verdict> = data
        .verdicts
        .iter()
        .filter(|v| !v.deleted)
        .map(|v| (v.session_at, v))
        .collect();

    for d in data.decrees.iter().filter(|d| !d.deleted) {
        if d.session_at < window_start {
            continue;
        }
        let norm = normalize_decree(&d.text);
        if norm.is_empty() {
            continue;
        }
        let entry = map.entry(norm.clone()).or_insert_with(|| StandingPromise {
            normalized_text: norm,
            original_sample: d.text.clone(),
            count: 0,
            first_seen_unix: d.session_at,
            last_seen_unix: d.session_at,
            verdict_kinds: Vec::new(),
        });
        entry.count += 1;
        entry.first_seen_unix = entry.first_seen_unix.min(d.session_at);
        entry.last_seen_unix = entry.last_seen_unix.max(d.session_at);
        if let Some(v) = verdict_for.get(&d.session_at) {
            if !entry.verdict_kinds.contains(&v.kind) {
                entry.verdict_kinds.push(v.kind);
            }
        }
    }

    let mut out: Vec<StandingPromise> = map
        .into_values()
        .filter(|p| p.count >= min_count)
        .filter(|p| {
            // Every verdict seen must be a miss — Kept/Extended means it was
            // delivered, so it is not standing unmet. An occurrence with no
            // verdict at all is treated as a miss (it produced no Kept).
            p.verdict_kinds
                .iter()
                .all(|k| matches!(k, VerdictKind::Broken | VerdictKind::Partial))
        })
        .collect();

    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(b.last_seen_unix.cmp(&a.last_seen_unix))
    });
    out
}

/// True when there is at least one standing promise.
pub fn has_stood_unmet(promises: &[StandingPromise]) -> bool {
    !promises.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::decree;
    use crate::journal::verdict;

    const NOW: i64 = 1_793_431_800;

    fn decree_at(session_at: i64, text: &str) -> Decree {
        decree::create(session_at, session_at - 60, text).unwrap()
    }

    fn data_with(decreees: Vec<Decree>, verdicts: Vec<Verdict>) -> Data {
        let mut d = Data::default();
        d.decrees = decreees;
        d.verdicts = verdicts;
        d
    }

    #[test]
    fn normalize_decree_preserves_zwnj() {
        assert_eq!(
            normalize_decree("می\u{200c}روم"),
            "می\u{200c}روم",
            "ZWNJ is part of the word"
        );
    }

    #[test]
    fn normalize_decree_collapses_whitespace() {
        assert_eq!(
            normalize_decree("  write   the  report  "),
            "report the write"
        );
    }

    #[test]
    fn normalize_decree_sorts_tokens() {
        assert_eq!(
            normalize_decree("b a c"),
            "a b c",
            "tokens sorted so phrasing order doesn't matter"
        );
        assert_eq!(normalize_decree("c a b"), "a b c");
    }

    #[test]
    fn standing_promises_empty_when_all_kept() {
        let d = decree_at(NOW - 100, "write the report");
        let v = verdict::declare(NOW - 100, VerdictKind::Kept, NOW).unwrap();
        let data = data_with(vec![d], vec![v]);
        let p = standing_promises(&data, NOW, 30 * 86_400, 1);
        assert!(p.is_empty(), "a Kept promise is delivered, not standing");
    }

    #[test]
    fn standing_promises_detects_repeated_miss() {
        // Five occurrences, all Broken → standing.
        let mut ds = Vec::new();
        for i in 0..5 {
            let at = NOW - i * 86_400;
            ds.push(decree_at(at, "write the report"));
        }
        let data = data_with(ds, vec![]);
        let p = standing_promises(&data, NOW, 30 * 86_400, 3);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].normalized_text, "report the write");
        assert_eq!(p[0].count, 5);
    }

    #[test]
    fn standing_promises_excludes_out_of_window() {
        let old = decree_at(NOW - 60 * 86_400, "write the report");
        let recent = decree_at(NOW - 1000, "write the report");
        let data = data_with(vec![old, recent], vec![]);
        let p = standing_promises(&data, NOW, 30 * 86_400, 2);
        assert!(p.is_empty(), "only one occurrence in the 30-day window");
    }

    #[test]
    fn standing_promises_respects_min_count() {
        let data = data_with(
            vec![
                decree_at(NOW - 100, "write the report"),
                decree_at(NOW - 200, "write the report"),
            ],
            vec![],
        );
        assert!(standing_promises(&data, NOW, 30 * 86_400, 3).is_empty());
        assert_eq!(standing_promises(&data, NOW, 30 * 86_400, 2).len(), 1);
    }

    #[test]
    fn standing_promises_sorted_by_count_then_recency() {
        let data = data_with(
            vec![
                decree_at(NOW - 100, "alpha task"),
                decree_at(NOW - 200, "alpha task"),
                decree_at(NOW - 300, "alpha task"),
                decree_at(NOW - 400, "beta task"),
                decree_at(NOW - 500, "beta task"),
            ],
            vec![],
        );
        let p = standing_promises(&data, NOW, 30 * 86_400, 2);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].normalized_text, "alpha task");
        assert_eq!(p[0].count, 3);
        assert_eq!(p[1].normalized_text, "beta task");
    }

    #[test]
    fn has_stood_unmet_false_when_empty() {
        assert!(!has_stood_unmet(&[]));
    }
}
