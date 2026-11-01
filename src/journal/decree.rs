//! One decree: what a focus session is *meant* to produce.
//!
//! An inscription looks backwards — it records what was achieved. A decree
//! looks forwards: before the block starts, one line saying what this block is
//! for. Kept together they turn the archive from a diary into a ledger with a
//! plan column, and a block that produced nothing opposite its own intention is
//! visible at a glance.
//!
//! The rules are the inscription's rules, tighter on length: an intention is a
//! promise, not a plan. If it needs a paragraph it is not one thing yet.
//!
//! **Why the session is not known when the decree is written.** The user writes
//! the decree before the block runs, so there is no end timestamp yet to key it
//! by. Rather than invent an id scheme that would have to be repaired later,
//! the decree is only turned into a record once the block finishes — at which
//! point the end timestamp exists and the decree and its inscription can share
//! one key. A decree is therefore held in memory until a session ends, and a
//! block that is skipped or reset simply never becomes one. That is the honest
//! outcome: an intention for work that never happened is not a record.

use serde::{Deserialize, Serialize};

/// The longest decree kept, in characters (not bytes).
///
/// Shorter than an inscription on purpose. An inscription may describe; a
/// decree must fit on the strip under the clock without wrapping to a second
/// line, or it stops being something you can glance at mid-block.
pub const MAX_CHARS: usize = 120;

/// One stated intention, bound to the session it preceded.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Decree {
    /// Stable identity, derived from (session, text) exactly as an inscription's
    /// is. Two devices that recorded the same intention for the same session
    /// therefore produce the same id and merge as a set rather than as
    /// duplicates.
    pub id: String,
    /// Unix seconds when the session that answered this decree ended. Zero only
    /// for a decree still awaiting its session, which is never stored.
    pub session_at: i64,
    /// The line itself, already normalised and validated.
    pub text: String,
    /// Unix seconds when the user wrote it — before the block, so earlier than
    /// `session_at`. Kept because "I meant this at 09:00 and finished at 09:25"
    /// is more useful than a single ambiguous timestamp.
    pub written_at: i64,
    /// Set when discarded. The row stays so the discard can travel to the other
    /// device; the same reasoning as every other tombstone in this store.
    #[serde(default)]
    pub deleted: bool,
}

/// Why a decree was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecreeError {
    /// Nothing but whitespace.
    Empty,
    /// Longer than [`MAX_CHARS`] characters after trimming.
    TooLong { len: usize },
    /// Contains a newline, tab, or other control character.
    ControlChars,
}

impl std::fmt::Display for DecreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "a decree needs words"),
            Self::TooLong { len } => {
                write!(f, "too long: {len} characters, the limit is {MAX_CHARS}")
            }
            Self::ControlChars => write!(f, "a decree cannot contain line breaks or tabs"),
        }
    }
}

impl std::error::Error for DecreeError {}

/// Collapse runs of whitespace to single spaces and trim the ends.
///
/// The same rule as an inscription, and for the same reason: "  write the
/// report  " and "write the report" must be one decree, or the two devices will
/// disagree about whether the same intention was written twice.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
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

/// Check a raw decree. Returns the normalised text on success.
///
/// Emptiness is decided first and on the normalised text, so a pasted blank
/// line is reported as "a decree needs words" rather than as a complaint about
/// its line breaks — the second is true but not what the user needs to hear.
pub fn validate_text(text: &str) -> Result<String, DecreeError> {
    let clean = normalize(text);
    if clean.is_empty() {
        return Err(DecreeError::Empty);
    }
    if clean.chars().any(|c| c.is_control()) {
        return Err(DecreeError::ControlChars);
    }
    let len = clean.chars().count();
    if len > MAX_CHARS {
        return Err(DecreeError::TooLong { len });
    }
    Ok(clean)
}

/// Build a record from a raw line, once the session it precedes has ended.
///
/// `written_at` is when the user typed it; `session_at` is when the block
/// finished. Both are needed, and passing them separately is what stops the
/// record from claiming the intention was formed at the moment it was answered.
pub fn create(session_at: i64, written_at: i64, raw: &str) -> Result<Decree, DecreeError> {
    let text = validate_text(raw)?;
    Ok(Decree {
        id: id_for(session_at, &text),
        session_at,
        written_at,
        text,
        deleted: false,
    })
}

/// A stable id for (session, text).
///
/// Content-derived rather than random, so the same intention recorded on two
/// devices for the same session is one record and not two.
fn id_for(session_at: i64, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(session_at.to_le_bytes());
    h.update(b"\x00");
    h.update(text.as_bytes());
    let digest = h.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-31 09:00:00 +0330, a fixed instant to reason against.
    const T: i64 = 1_793_431_800;

    #[test]
    fn empty_is_refused() {
        assert_eq!(validate_text(""), Err(DecreeError::Empty));
        assert_eq!(validate_text("   \t \n  "), Err(DecreeError::Empty));
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(
            validate_text("  draft\t the   contract  ").unwrap(),
            "draft the contract"
        );
    }

    #[test]
    fn pasted_line_breaks_become_one_line() {
        assert_eq!(
            validate_text("draft the contract\n\nthen send it").unwrap(),
            "draft the contract then send it"
        );
    }

    #[test]
    fn overlong_is_refused_by_characters_not_bytes() {
        // Persian characters are two bytes each; a byte limit would refuse this
        // at half the intended length.
        let persian = "ن".repeat(MAX_CHARS);
        assert_eq!(persian.len(), MAX_CHARS * 2, "precondition: two bytes each");
        assert!(validate_text(&persian).is_ok(), "the limit is characters");

        let too_long = "ن".repeat(MAX_CHARS + 1);
        assert_eq!(
            validate_text(&too_long),
            Err(DecreeError::TooLong { len: MAX_CHARS + 1 })
        );
    }

    #[test]
    fn a_decree_is_shorter_than_an_inscription() {
        // The strip under the clock must stay one line; that is the whole
        // reason the two limits differ.
        assert!(MAX_CHARS < crate::journal::inscription::MAX_CHARS);
    }

    #[test]
    fn create_derives_a_stable_id() {
        let a = create(T, T - 1500, "write the report").unwrap();
        let b = create(T, T - 1500, "write the report").unwrap();
        assert_eq!(a, b, "same session and text must be the same record");
        assert_eq!(a.session_at, T);
        assert_eq!(a.written_at, T - 1500, "written before the block ended");
        assert!(!a.deleted);
    }

    #[test]
    fn different_sessions_do_not_collide() {
        let a = create(T, T, "write the report").unwrap();
        let b = create(T + 3600, T, "write the report").unwrap();
        assert_ne!(a.id, b.id, "the same intention twice is two decrees");
    }

    #[test]
    fn an_empty_after_normalisation_is_refused() {
        assert_eq!(validate_text(" \u{200c} ").unwrap_or_default(), "\u{200c}");
        // A zero-width non-joiner is real Persian orthography, not whitespace,
        // so it must survive rather than be trimmed to nothing.
        let zwnj = "می\u{200c}روم";
        assert_eq!(validate_text(zwnj).unwrap(), zwnj);
    }

    #[test]
    fn a_decree_survives_a_json_round_trip() {
        let d = create(T, T - 900, "دوباره بنویس").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        let back: Decree = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
