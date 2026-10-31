//! Obstructions: why a block was not kept.
//!
//! A verdict says *what* a block was — kept, partial, broken, extended, short —
//! but not *why*. The record can know that a promise was Broken and still miss
//! that the same cause (a meeting, a dead battery, a forgotten errand) recurs
//! every week, because the cause is never written down. An obstruction is that
//! missing line: one short sentence, captured at the moment a non-Kept verdict
//! is declared, stored against the session it describes.
//!
//! **An obstruction belongs to a session, not to a verdict.** It is keyed by
//! `session_at`, the same key the decree, inscription and verdict join on, so
//! it survives edits to the words around it. Tombstoning (empty text) keeps one
//! row per session and lets a sync merge see a deletion.
//!
//! Everything here is pure: slices and explicit parameters in, owned data out,
//! no I/O, no clock reads beyond what is passed in.

use serde::{Deserialize, Serialize};

/// Longest obstruction, in characters after trimming.
pub const OBSTRUCTION_MAX_CHARS: usize = 120;

/// One "what blocked it" line for a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obstruction {
    /// Unix seconds of the session this explains — the join key shared with
    /// the decree, inscription and verdict.
    pub session_at: i64,
    /// The normalized reason text, 1..=OBSTRUCTION_MAX_CHARS characters.
    pub text: String,
    /// Unix seconds when this row was last written. Doubles as the merge
    /// stamp: overwriting a reason bumps it, so sync keeps the newer copy.
    pub created_at: i64,
    /// Set when the user clears the reason. The row stays so the clearing can
    /// reach the other device; a row that simply vanished would resurrect on
    /// the next merge.
    pub deleted: bool,
}

/// Why an obstruction could not be stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObstructionError {
    #[allow(dead_code)] // part of the error surface; empty is handled as tombstone
    /// The text is empty after normalization (this is a tombstone request,
    /// handled by `set_obstruction` — callers that don't mean to delete
    /// should treat it as a no-op).
    Empty,
    /// Longer than [`OBSTRUCTION_MAX_CHARS`] characters after trimming.
    TooLong { len: usize },
}

/// Normalize a reason: trim, collapse runs of whitespace to a single space,
/// drop control characters, and preserve ZWNJ (the Persian joiner stays part
/// of the word).
pub fn normalize_obstruction(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else if c.is_control() {
            // A control byte is dropped, but it still separates words: keep
            // the boundary so "a\0b" reads as "a b", not "ab".
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Store (or clear) the obstruction for one session.
///
/// Non-empty text replaces any existing row for `session_at`. Empty text
/// tombstones the row (marks it deleted) rather than removing it, so a clear
/// propagates through sync. Returns the new obstruction list.
pub fn set_obstruction(
    existing: &[Obstruction],
    session_at: i64,
    text: &str,
    now: i64,
) -> Result<Vec<Obstruction>, ObstructionError> {
    let clean = normalize_obstruction(text);
    if clean.is_empty() {
        // Tombstone: mark any live row deleted.
        let mut out = existing.to_vec();
        for o in out.iter_mut() {
            if o.session_at == session_at && !o.deleted {
                o.deleted = true;
                o.created_at = now;
            }
        }
        return Ok(out);
    }
    let len = clean.chars().count();
    if len > OBSTRUCTION_MAX_CHARS {
        return Err(ObstructionError::TooLong { len });
    }
    let mut out = existing
        .iter()
        .filter(|o| o.session_at != session_at)
        .cloned()
        .collect::<Vec<_>>();
    out.push(Obstruction {
        session_at,
        text: clean,
        created_at: now,
        deleted: false,
    });
    out.sort_by_key(|o| o.session_at);
    Ok(out)
}

/// The live obstruction for a session, if any.
pub fn obstruction_for<'a>(
    existing: &'a [Obstruction],
    session_at: i64,
) -> Option<&'a Obstruction> {
    existing
        .iter()
        .filter(|o| o.session_at == session_at && !o.deleted)
        .max_by_key(|o| o.created_at)
}

/// The most common obstructions in a span, most frequent first.
///
/// Live rows only, keyed by normalized text. Ties are broken by earliest first
/// seen, so the ordering is deterministic.
pub fn top_obstructions(
    existing: &[Obstruction],
    from: i64,
    to: i64,
    n: usize,
) -> Vec<(String, u32)> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, (u32, i64)> = BTreeMap::new();
    for o in existing {
        if o.deleted {
            continue;
        }
        if o.session_at < from || o.session_at > to {
            continue;
        }
        // Key by normalized text so "no  time" and "no time" count together.
        let entry = counts
            .entry(normalize_obstruction(&o.text))
            .or_insert((0, o.session_at));
        entry.0 += 1;
        entry.1 = entry.1.min(o.session_at);
    }
    let mut out: Vec<(String, u32)> = counts
        .into_iter()
        .map(|(text, (count, _))| (text, count))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(n);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_793_431_800;

    #[test]
    fn normalize_collapses_whitespace_and_trims() {
        assert_eq!(
            normalize_obstruction("  not  enough   time "),
            "not enough time"
        );
    }

    #[test]
    fn normalize_drops_control_characters() {
        assert_eq!(normalize_obstruction("a\u{0}b\nc"), "a b c");
    }

    #[test]
    fn normalize_preserves_zwnj() {
        assert_eq!(normalize_obstruction("می\u{200c}روم"), "می\u{200c}روم");
    }

    #[test]
    fn set_adds_for_new_session() {
        let out = set_obstruction(&[], 100, "no time", NOW).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_at, 100);
        assert_eq!(out[0].text, "no time");
        assert!(!out[0].deleted);
    }

    #[test]
    fn set_replaces_existing() {
        let first = set_obstruction(&[], 100, "no time", NOW).unwrap();
        let out = set_obstruction(&first, 100, "phone call", NOW + 10).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "phone call");
        assert_eq!(out[0].created_at, NOW + 10);
    }

    #[test]
    fn set_empty_tombstones() {
        let first = set_obstruction(&[], 100, "no time", NOW).unwrap();
        let out = set_obstruction(&first, 100, "", NOW + 10).unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].deleted);
        assert!(obstruction_for(&out, 100).is_none());
    }

    #[test]
    fn set_rejects_overlong() {
        let long: String = std::iter::repeat('ا')
            .take(OBSTRUCTION_MAX_CHARS + 1)
            .collect();
        let err = set_obstruction(&[], 100, &long, NOW).unwrap_err();
        assert!(matches!(err, ObstructionError::TooLong { .. }));
    }

    #[test]
    fn obstruction_for_none_when_missing() {
        assert!(obstruction_for(&[], 100).is_none());
    }

    #[test]
    fn top_counts_normalized_only() {
        let rows = vec![
            Obstruction {
                session_at: 1,
                text: "no time".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 2,
                text: "no  time".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 3,
                text: "phone call".into(),
                created_at: NOW,
                deleted: false,
            },
        ];
        let top = top_obstructions(&rows, 0, 100, 10);
        assert_eq!(top[0], ("no time".to_string(), 2));
        assert_eq!(top[1], ("phone call".to_string(), 1));
    }

    #[test]
    fn top_respects_window_and_tombstones() {
        let rows = vec![
            Obstruction {
                session_at: 1,
                text: "a".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 2,
                text: "a".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 3,
                text: "a".into(),
                created_at: NOW,
                deleted: true,
            },
            Obstruction {
                session_at: 200,
                text: "a".into(),
                created_at: NOW,
                deleted: false,
            },
        ];
        let top = top_obstructions(&rows, 0, 100, 10);
        assert_eq!(top, vec![("a".to_string(), 2)]);
    }

    #[test]
    fn top_sorted_by_count_then_earliest() {
        let rows = vec![
            Obstruction {
                session_at: 10,
                text: "x".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 20,
                text: "y".into(),
                created_at: NOW,
                deleted: false,
            },
            Obstruction {
                session_at: 30,
                text: "y".into(),
                created_at: NOW,
                deleted: false,
            },
        ];
        let top = top_obstructions(&rows, 0, 100, 10);
        assert_eq!(top[0].0, "y");
        assert_eq!(top[1].0, "x");
    }

    #[test]
    fn top_truncates_to_n() {
        let rows: Vec<Obstruction> = (0..5)
            .map(|i| Obstruction {
                session_at: i,
                text: format!("r{i}"),
                created_at: NOW,
                deleted: false,
            })
            .collect();
        assert_eq!(top_obstructions(&rows, 0, 100, 2).len(), 2);
    }
}
