//! One inscription: what a focus session actually produced.
//!
//! The rules are deliberately strict, because the value of the archive is that
//! it stays readable years later:
//!
//! * a line must say something — an empty or whitespace-only inscription is
//!   refused rather than stored as a blank row nobody can interpret;
//! * it must be one line — pasted line breaks and tabs collapse to single
//!   spaces, so one entry can never masquerade as several in the export;
//! * no other control characters — a stray escape or bell byte makes the text
//!   unsearchable and, in the export, breaks whatever renders it;
//! * a hard length cap — an inscription is a sentence, not a report. The cap is
//!   Persian-aware: it counts characters, not bytes, so a Persian sentence gets
//!   the same room as an English one.

use serde::{Deserialize, Serialize};

/// The longest inscription kept, in characters (not bytes).
///
/// Persian text is dense: two 140-character lines of Persian carry what four
/// lines of English would. 280 is enough for that, and short enough that the
/// archive stays scannable.
pub const MAX_CHARS: usize = 280;

/// One recorded accomplishment.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Inscription {
    /// Stable identity. Assigned once at creation and never reused, so a
    /// re-import cannot create a second row for the same writing.
    pub id: String,
    /// Unix seconds when the focus session that produced this ended.
    pub session_at: i64,
    /// The line itself, already normalised and validated.
    pub text: String,
    /// Unix seconds when it was written. Usually equal to `session_at`, but a
    /// user who types it a minute later should not have the record lie about
    /// when the work happened.
    pub written_at: i64,
    /// Set when the entry is discarded. The row stays so the discard can reach
    /// the other device; a row that simply vanishes is indistinguishable from
    /// one the other device never received, and the next sync would put it back.
    #[serde(default)]
    pub deleted: bool,
}

/// Why an inscription was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InscriptionError {
    /// Nothing but whitespace.
    Empty,
    /// Longer than [`MAX_CHARS`] characters after trimming.
    TooLong { len: usize },
    /// Contains a newline, tab, or other control character.
    ControlChars,
}

impl std::fmt::Display for InscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "an inscription needs words"),
            Self::TooLong { len } => {
                write!(f, "too long: {len} characters, the limit is {MAX_CHARS}")
            }
            Self::ControlChars => write!(f, "a line cannot contain line breaks or tabs"),
        }
    }
}

impl std::error::Error for InscriptionError {}

/// Collapse runs of whitespace to single spaces and trim the ends.
///
/// Done before validating, so "  wrote the
/// report  " and "wrote the report" are the same inscription and the archive
/// does not fill with near-duplicates that only differ in invisible space.
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

/// Check a raw inscription. Returns the normalised text on success.
///
/// Emptiness is decided first, on the normalised text, so a user who pasted a
/// blank line gets "an inscription needs words" rather than a complaint about
/// line breaks — which is what they would otherwise see for text that says
/// nothing. Length is measured in characters, the only measure fair to Persian.
pub fn validate_text(text: &str) -> Result<String, InscriptionError> {
    let clean = normalize(text);
    if clean.is_empty() {
        return Err(InscriptionError::Empty);
    }
    // A tab or other control byte inside real text makes the stored line
    // unsearchable and breaks the export, so it is refused rather than silently
    // stripped. Whitespace is exempt: it has already been collapsed to spaces by
    // `normalize`, which is how a pasted multi-line note becomes one line.
    if clean.chars().any(|c| c.is_control()) {
        return Err(InscriptionError::ControlChars);
    }
    let len = clean.chars().count();
    if len > MAX_CHARS {
        return Err(InscriptionError::TooLong { len });
    }
    Ok(clean)
}

/// Build a record from a raw line. The id is derived from the session and the
/// text, so the same writing for the same session is the same record.
pub fn create(
    session_at: i64,
    written_at: i64,
    raw: &str,
) -> Result<Inscription, InscriptionError> {
    let text = validate_text(raw)?;
    Ok(Inscription {
        id: id_for(session_at, &text),
        session_at,
        written_at,
        text,
        deleted: false,
    })
}

/// A stable id for (session, text).
///
/// Deliberately not random: an entry written on the desktop and synced to the
/// laptop must not arrive as a second, different inscription. Deriving it from
/// the content means the merge in `sync::merge` can treat it as a set.
fn id_for(session_at: i64, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(session_at.to_le_bytes());
    h.update(b"\x00");
    h.update(text.as_bytes());
    let digest = h.finalize();
    // 16 hex chars is 64 bits: collisions across a lifetime of inscriptions are
    // not a realistic concern, and a short id keeps the JSON readable.
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The calendar day a Unix timestamp falls on, as `YYYY-MM-DD` in local time.
///
/// A focus session that ends at 00:10 belongs to the new day, which is the day
/// the user would look for it on.
pub fn day_key(at: i64) -> String {
    match chrono::DateTime::from_timestamp(at, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string(),
        None => String::new(),
    }
}

/// Case-insensitive substring search over the text.
///
/// Persian has no case, and `to_lowercase` on Persian is a no-op, so the same
/// path works for both languages without a special case. Results come back
/// newest first, which is the order someone scanning their own history wants.
pub fn search<'a>(all: &'a [Inscription], needle: &str) -> Vec<&'a Inscription> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        let mut all_sorted: Vec<&Inscription> = all.iter().collect();
        all_sorted.sort_by(|a, b| b.session_at.cmp(&a.session_at));
        return all_sorted;
    }
    let mut out: Vec<&Inscription> = all
        .iter()
        .filter(|i| i.text.to_lowercase().contains(&needle))
        .collect();
    out.sort_by(|a, b| b.session_at.cmp(&a.session_at));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-31 09:00:00 +0330, a fixed instant to reason against.
    const T: i64 = 1_793_431_800;

    #[test]
    fn empty_is_refused() {
        assert_eq!(validate_text(""), Err(InscriptionError::Empty));
        assert_eq!(validate_text("   \t \n  "), Err(InscriptionError::Empty));
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(normalize("  wrote   the  report "), "wrote the report");
        assert_eq!(normalize("یک   خط\n\nنوشتم"), "یک خط نوشتم");
    }

    #[test]
    fn overlong_is_refused_by_characters_not_bytes() {
        // 281 Persian characters: 562 bytes. A byte cap would refuse this far
        // earlier than it should.
        let long: String = std::iter::repeat('ا').take(MAX_CHARS + 1).collect();
        assert_eq!(
            validate_text(&long),
            Err(InscriptionError::TooLong { len: MAX_CHARS + 1 })
        );
        let at_limit: String = std::iter::repeat('ا').take(MAX_CHARS).collect();
        assert!(
            validate_text(&at_limit).is_ok(),
            "exactly at the limit is fine"
        );
    }

    #[test]
    fn pasted_line_breaks_and_tabs_become_one_line() {
        // A user who pastes two lines is writing one thought; the entry must
        // stay a single line so the export cannot read it as two entries.
        assert_eq!(
            validate_text("wrote the report\nand sent it").unwrap(),
            "wrote the report and sent it"
        );
        assert_eq!(
            validate_text("wrote\tthe report").unwrap(),
            "wrote the report"
        );
    }

    #[test]
    fn other_control_characters_are_refused() {
        // An escape or bell byte is not whitespace and cannot be collapsed; it
        // would make the line unsearchable and break the export.
        assert_eq!(
            validate_text("a\u{1b}b"),
            Err(InscriptionError::ControlChars)
        );
        assert_eq!(
            validate_text("a\u{7}b"),
            Err(InscriptionError::ControlChars)
        );
    }

    #[test]
    fn create_derives_a_stable_id() {
        let a = create(T, T, "wrote the report").unwrap();
        let b = create(T, T + 60, "  wrote   the report  ").unwrap();
        assert_eq!(a.text, b.text);
        assert_eq!(
            a.id, b.id,
            "the same writing for the same session is the same record"
        );
        assert_ne!(a.id, create(T + 1, T + 1, "wrote the report").unwrap().id);
    }

    #[test]
    fn search_is_case_insensitive_substring() {
        let all = vec![
            create(T, T, "Wrote the Report").unwrap(),
            create(T + 100, T + 100, "read papers").unwrap(),
        ];
        let hits = search(&all, "report");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "Wrote the Report");
    }

    #[test]
    fn search_finds_persian_text() {
        let all = vec![
            create(T, T, "گزارش را نوشتم").unwrap(),
            create(T + 100, T + 100, "مقاله خواندم").unwrap(),
        ];
        assert_eq!(search(&all, "گزارش").len(), 1);
        assert_eq!(search(&all, "نوشتم").len(), 1);
        assert_eq!(search(&all, "چیزی که نیست").len(), 0);
    }

    #[test]
    fn an_empty_search_returns_everything_newest_first() {
        let all = vec![
            create(T, T, "first").unwrap(),
            create(T + 100, T + 100, "second").unwrap(),
        ];
        let hits = search(&all, "   ");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].text, "second", "newest first");
    }

    #[test]
    fn an_inscription_survives_a_json_round_trip() {
        let ins = create(T, T, "wrote the report").unwrap();
        let json = serde_json::to_string(&ins).unwrap();
        let back: Inscription = serde_json::from_str(&json).unwrap();
        assert_eq!(ins, back);
    }
}
