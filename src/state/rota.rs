//! The day's rota: an ordered queue of the blocks this day is meant to hold.
//!
//! A Pomodoro timer has one question at every session start — *what now?* — and
//! answering it costs the first minutes of the block. The decree asks the same
//! question, but one at a time and mid-start, which is the worst possible
//! moment: the user is holding the answer and the keyboard at once, and a line
//! written under that pressure is a line written to get past the dialog.
//!
//! A rota moves the question earlier. Before the day begins, the user writes
//! the day down as a short numbered list — a label, a length, and optionally the
//! decree the block will be opened with. Starting a session then costs one
//! press: the top pending entry opens, its decree is attached, and the clock
//! runs. The intentions are written in a batch, at a desk, thinking, instead of
//! one at a time against a countdown.
//!
//! **Why the entry carries its own day.** A rota is not a backlog. Work planned
//! for Tuesday that did not happen is a fact about Tuesday, and silently
//! dragging it into Wednesday makes the list dishonest and permanently
//! over-full. So an entry keeps the day it was planned for; a pending entry
//! from an earlier day is not deleted but is not on today's rota either — it is
//! offered back explicitly by [`carry_over`], which is the only path by which
//! yesterday's leftovers can reach today.
//!
//! **Why an opened entry stays.** When a block starts, its entry is marked
//! opened rather than removed. The list is a record of what the day was meant
//! to hold, and an entry that vanished on use would leave a rota that shrinks as
//! the day is worked — indistinguishable from one the user edited. Keeping it
//! means the rota can show, at the end of the day, both what was planned and
//! what was actually taken up.
//!
//! Everything here is pure: no clock reads, no I/O, no UI. The day and the
//! current instant are always passed in, which is what lets the rules be tested
//! without a fake system clock.

use serde::{Deserialize, Serialize};

/// The largest number of pending entries one day may hold.
///
/// A rota is a day's shape, not a backlog. Past this it stops being readable at
/// a glance, which is the only property it has over the task list.
pub const MAX_ENTRIES: usize = 20;

/// The longest label kept, in characters (not bytes).
pub const MAX_LABEL_CHARS: usize = 80;

/// Shortest block a rota entry may ask for, in minutes. Below this a "block" is
/// not a focus session and would pollute the history with fragments.
pub const MIN_MINUTES: u64 = 1;

/// Longest block a rota entry may ask for, in minutes.
pub const MAX_MINUTES: u64 = 180;

/// One planned block: what it is, how long, and the line it opens with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RotaEntry {
    /// Stable identity from the store's counter, so two devices that plan the
    /// same line still produce two entries — they are two separate plans, not
    /// one fact recorded twice.
    pub id: u64,
    /// The local day this block was planned for, `YYYY-MM-DD`.
    ///
    /// Set once at creation and never moved, which is what keeps a stale entry
    /// from quietly joining a later day's list.
    pub day: String,
    /// What the block is, in the user's words. Already normalised.
    pub label: String,
    /// The task this block came from, when it came from one. The link is kept
    /// so a later screen can tell "planned from the task list" from "typed
    /// here", but nothing in this module resolves it — a task that has since
    /// been deleted must not make its rota entry unreadable.
    #[serde(default)]
    pub task_id: Option<u64>,
    /// The decree this block opens with, or `""` for none.
    ///
    /// Held on the entry rather than only as a pending decree because the whole
    /// point of the rota is to have the line ready before the block starts.
    #[serde(default)]
    pub decree: String,
    /// How long the block should run, in minutes.
    pub minutes: u64,
    /// Unix seconds when it was planned. Orders the list within a day.
    pub created_at: i64,
    /// Unix seconds when its block was actually started, 0 while still pending.
    ///
    /// An opened entry is neither pending nor gone: it records that the plan was
    /// taken up, which is a different fact from the session itself (a block that
    /// was started and abandoned produced no session at all).
    #[serde(default)]
    pub opened_at: i64,
    /// Unix seconds of the session this block produced, 0 while it has none.
    ///
    /// The link [`super::outcome::claim`] writes when a session lands. Kept as
    /// the session's own key rather than as a copy of its words, so the entry
    /// points at the record instead of duplicating it.
    ///
    /// `#[serde(default)]` on both this and `outcome` is load-bearing: a rota
    /// row written before this feature must still parse, and stop providing the
    /// key older builds do not know about.
    #[serde(default)]
    pub session_at: i64,
    /// What became of the block, as `outcome::Outcome`'s storage string.
    ///
    /// `None` means the app has not seen it settle — the block is still running,
    /// or the row predates the field. Read through
    /// [`super::outcome::effective`], never directly, so the "still pending on a
    /// past day is missed" rule has exactly one home.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Set when the entry is taken off the rota. The row stays so the removal
    /// can travel to the other device; the same reasoning as every other
    /// tombstone in this store.
    #[serde(default)]
    pub deleted: bool,
}

impl RotaEntry {
    /// Still waiting to be started.
    pub fn is_pending(&self) -> bool {
        !self.deleted && self.opened_at == 0
    }

    /// Visible to the user at all: not a tombstone.
    pub fn is_live(&self) -> bool {
        !self.deleted
    }
}

/// Why an entry was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotaError {
    /// Nothing but whitespace in the label.
    Empty,
    /// Longer than [`MAX_LABEL_CHARS`] characters after trimming.
    LabelTooLong { len: usize },
    /// Contains a control character a label cannot carry.
    ControlChars,
    /// A length outside [`MIN_MINUTES`]..=[`MAX_MINUTES`].
    MinutesOutOfRange { minutes: u64 },
    /// The day's rota already holds [`MAX_ENTRIES`] pending entries.
    Full,
}

impl std::fmt::Display for RotaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "a rota entry needs words"),
            Self::LabelTooLong { len } => write!(
                f,
                "too long: {len} characters, the limit is {MAX_LABEL_CHARS}"
            ),
            Self::ControlChars => write!(f, "a line cannot contain line breaks or tabs"),
            Self::MinutesOutOfRange { minutes } => write!(
                f,
                "{minutes} minutes is outside {MIN_MINUTES}–{MAX_MINUTES}"
            ),
            Self::Full => write!(f, "a day's rota holds at most {MAX_ENTRIES} blocks"),
        }
    }
}

impl std::error::Error for RotaError {}

/// Check a raw label. Returns the normalised text on success.
///
/// The rules and the order they are applied in are the inscription's and the
/// decree's, deliberately: the same field shape in three places must not judge
/// the same input differently. Emptiness is decided first, on the normalised
/// text, so a pasted blank line reports "needs words" rather than line breaks.
pub fn validate_label(raw: &str) -> Result<String, RotaError> {
    // One normaliser for the whole app: whitespace runs collapse to a single
    // space and the ends are trimmed, so two ways of typing one label are one
    // label. Reusing the inscription's is the point — a second copy is a second
    // place for the rule to drift.
    let clean = crate::journal::inscription::normalize(raw);
    if clean.is_empty() {
        return Err(RotaError::Empty);
    }
    if clean.chars().any(|c| c.is_control()) {
        return Err(RotaError::ControlChars);
    }
    let len = clean.chars().count();
    if len > MAX_LABEL_CHARS {
        return Err(RotaError::LabelTooLong { len });
    }
    Ok(clean)
}

/// Check a asked-for length.
pub fn validate_minutes(minutes: u64) -> Result<u64, RotaError> {
    if !(MIN_MINUTES..=MAX_MINUTES).contains(&minutes) {
        return Err(RotaError::MinutesOutOfRange { minutes });
    }
    Ok(minutes)
}

/// The decree a rota entry will open its block with, normalised and checked.
///
/// An empty decree is legitimate: the entry can be planned with its length and
/// label alone, and the block will then be offered the usual decree dialog. A
/// non-empty one is judged by the decree's own rules, so a line the rota accepts
/// can never be refused when the block starts.
pub fn validate_decree(raw: &str) -> Result<String, RotaError> {
    let clean = crate::journal::inscription::normalize(raw);
    if clean.is_empty() {
        return Ok(String::new());
    }
    match crate::journal::decree::validate_text(&clean) {
        Ok(text) => Ok(text),
        Err(crate::journal::decree::DecreeError::TooLong { len }) => {
            Err(RotaError::LabelTooLong { len })
        }
        Err(crate::journal::decree::DecreeError::ControlChars) => Err(RotaError::ControlChars),
        // The decree's own "empty" cannot be reached here: emptiness was already
        // turned into "no decree" above.
        Err(crate::journal::decree::DecreeError::Empty) => Ok(String::new()),
    }
}

/// How many entries are still waiting on the given day.
pub fn pending_count(all: &[RotaEntry], day: &str) -> usize {
    all.iter()
        .filter(|e| e.day == day && e.is_pending())
        .count()
}

/// True when the day's rota still has room for another pending entry.
pub fn has_room(all: &[RotaEntry], day: &str) -> bool {
    pending_count(all, day) < MAX_ENTRIES
}

/// Build an entry from raw user input.
///
/// `next_id` comes from the store's counter. The day is passed in rather than
/// read from the clock so that a test, and a rota filled in for tomorrow, both
/// work without touching the system time.
#[allow(clippy::too_many_arguments)] // one struct literal's worth of inputs, in one call
pub fn create(
    next_id: u64,
    day: &str,
    raw_label: &str,
    task_id: Option<u64>,
    raw_decree: &str,
    minutes: u64,
    now: i64,
    all: &[RotaEntry],
) -> Result<RotaEntry, RotaError> {
    let label = validate_label(raw_label)?;
    let minutes = validate_minutes(minutes)?;
    let decree = validate_decree(raw_decree)?;
    if !has_room(all, day) {
        return Err(RotaError::Full);
    }
    Ok(RotaEntry {
        id: next_id,
        day: day.to_string(),
        label,
        task_id,
        decree,
        minutes,
        created_at: now,
        opened_at: 0,
        session_at: 0,
        outcome: None,
        deleted: false,
    })
}

/// The entries on a day, in the order they were planned.
///
/// Creation order rather than any stored position: the list is authored in one
/// sitting, so the order it was typed in *is* the intended order, and an extra
/// field to say so would be a second source of truth to keep in step.
pub fn for_day<'a>(all: &'a [RotaEntry], day: &str) -> Vec<&'a RotaEntry> {
    let mut out: Vec<&RotaEntry> = all.iter().filter(|e| e.day == day && e.is_live()).collect();
    out.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The next block to start on this day: the first that is still pending.
pub fn peek_next<'a>(all: &'a [RotaEntry], day: &str) -> Option<&'a RotaEntry> {
    for_day(all, day).into_iter().find(|e| e.is_pending())
}

/// Mark an entry as taken up. Returns false when it was not a pending entry.
///
/// `opened_at` is set rather than the row being removed, so the day keeps a
/// record of which plans were actually begun.
pub fn mark_opened(all: &mut [RotaEntry], id: u64, now: i64) -> bool {
    match all.iter_mut().find(|e| e.id == id) {
        Some(e) if e.is_pending() => {
            e.opened_at = now;
            true
        }
        _ => false,
    }
}

/// Take an entry off the rota by marking it. A wrong plan is not rewritten.
pub fn remove(all: &mut [RotaEntry], id: u64) -> bool {
    match all.iter_mut().find(|e| e.id == id) {
        Some(e) if !e.deleted => {
            e.deleted = true;
            true
        }
        _ => false,
    }
}

/// Entries planned for an earlier day that were never started.
///
/// Returned oldest-day-first so the caller can offer them in the order they
/// were abandoned. Nothing here moves them: carrying work forward is the user's
/// decision, and a rota that silently refilled itself would be a list you can
/// never finish.
pub fn carry_over(all: &[RotaEntry], today: &str) -> Vec<RotaEntry> {
    let mut out: Vec<RotaEntry> = all
        .iter()
        .filter(|e| e.is_pending() && !e.day.is_empty() && e.day.as_str() < today)
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        a.day
            .cmp(&b.day)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Re-plan a stale entry onto `today`, keeping its label and decree.
///
/// A new row with a new id rather than a moved one: yesterday's plan is a fact
/// that stays true about yesterday, so carrying it forward records a second
/// decision instead of editing the first. The original is tombstoned, which is
/// what stops it being offered again tomorrow.
pub fn replan(entry: &RotaEntry, new_id: u64, today: &str, now: i64) -> RotaEntry {
    RotaEntry {
        id: new_id,
        day: today.to_string(),
        label: entry.label.clone(),
        task_id: entry.task_id,
        decree: entry.decree.clone(),
        minutes: entry.minutes,
        created_at: now,
        opened_at: 0,
        // A replanned entry is a fresh plan for a new day: it inherits the words
        // but none of yesterday's outcome, or a leftover would arrive already
        // marked done and never enter the queue.
        session_at: 0,
        outcome: None,
        deleted: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "2026-10-31";
    const T: i64 = 1_793_431_800;

    fn entry(id: u64, day: &str, label: &str, created_at: i64) -> RotaEntry {
        create(id, day, label, None, "", 25, created_at, &[]).unwrap()
    }

    #[test]
    fn a_blank_label_is_refused_before_anything_else() {
        assert_eq!(validate_label(""), Err(RotaError::Empty));
        assert_eq!(validate_label("   \t  "), Err(RotaError::Empty));
    }

    #[test]
    fn a_label_is_collapsed_to_one_line() {
        assert_eq!(
            validate_label("  write   the report ").unwrap(),
            "write the report"
        );
        assert_eq!(validate_label("draft\nand send").unwrap(), "draft and send");
    }

    #[test]
    fn a_label_cap_counts_characters_not_bytes() {
        let long: String = "ا".repeat(MAX_LABEL_CHARS + 1);
        assert_eq!(
            validate_label(&long),
            Err(RotaError::LabelTooLong {
                len: MAX_LABEL_CHARS + 1
            })
        );
        let fits: String = "ا".repeat(MAX_LABEL_CHARS);
        assert!(
            validate_label(&fits).is_ok(),
            "exactly at the limit is fine"
        );
    }

    #[test]
    fn a_control_character_is_refused() {
        assert_eq!(validate_label("a\u{1b}b"), Err(RotaError::ControlChars));
    }

    #[test]
    fn a_block_length_must_be_within_range() {
        assert_eq!(
            validate_minutes(0),
            Err(RotaError::MinutesOutOfRange { minutes: 0 })
        );
        assert_eq!(
            validate_minutes(MAX_MINUTES + 1),
            Err(RotaError::MinutesOutOfRange {
                minutes: MAX_MINUTES + 1
            })
        );
        assert_eq!(validate_minutes(MIN_MINUTES).unwrap(), MIN_MINUTES);
        assert_eq!(validate_minutes(MAX_MINUTES).unwrap(), MAX_MINUTES);
    }

    #[test]
    fn an_entry_may_carry_no_decree() {
        let e = create(1, D, "read a paper", None, "   ", 30, T, &[]).unwrap();
        assert_eq!(e.decree, "");
        assert_eq!(e.minutes, 30);
    }

    #[test]
    fn an_entry_takes_its_decree_from_the_rota_not_the_dialog() {
        let e = create(
            1,
            D,
            "write the report",
            None,
            "  the whole draft ",
            50,
            T,
            &[],
        )
        .unwrap();
        assert_eq!(e.decree, "the whole draft");
    }

    #[test]
    fn a_decree_longer_than_the_decree_limit_is_refused() {
        let long: String = "ا".repeat(crate::journal::decree::MAX_CHARS + 1);
        assert_eq!(
            validate_decree(&long),
            Err(RotaError::LabelTooLong {
                len: crate::journal::decree::MAX_CHARS + 1
            })
        );
    }

    #[test]
    fn a_day_holds_at_most_the_cap() {
        let mut all: Vec<RotaEntry> = Vec::new();
        for i in 1..=MAX_ENTRIES as u64 {
            all.push(create(i, D, &format!("block {i}"), None, "", 25, T, &all).unwrap());
        }
        assert_eq!(pending_count(&all, D), MAX_ENTRIES);
        assert_eq!(
            create(99, D, "one too many", None, "", 25, T, &all),
            Err(RotaError::Full)
        );
        // A different day is a different list, so it still has room.
        assert!(create(100, "2026-11-01", "tomorrow", None, "", 25, T, &all).is_ok());
    }

    #[test]
    fn the_next_block_is_the_first_one_planned() {
        let all = vec![
            entry(2, D, "second", T + 2),
            entry(1, D, "first", T + 1),
            entry(3, D, "third", T + 3),
        ];
        assert_eq!(peek_next(&all, D).unwrap().label, "first");
    }

    #[test]
    fn a_started_block_is_no_longer_next() {
        let mut all = vec![entry(1, D, "first", T), entry(2, D, "second", T + 1)];
        assert!(mark_opened(&mut all, 1, T + 60));
        assert_eq!(
            peek_next(&all, D).unwrap().label,
            "second",
            "the rota advances on use"
        );
        assert!(
            !mark_opened(&mut all, 1, T + 120),
            "starting it twice must not open it twice"
        );
    }

    #[test]
    fn a_started_entry_stays_on_the_list() {
        let mut all = vec![entry(1, D, "first", T)];
        mark_opened(&mut all, 1, T + 60);
        assert_eq!(
            for_day(&all, D).len(),
            1,
            "the plan is a record, not a queue that erases itself"
        );
        assert_eq!(for_day(&all, D)[0].opened_at, T + 60);
    }

    #[test]
    fn a_removed_entry_is_gone_from_the_day_but_still_in_the_store() {
        let mut all = vec![entry(1, D, "first", T), entry(2, D, "second", T + 1)];
        assert!(remove(&mut all, 1));
        assert_eq!(for_day(&all, D).len(), 1);
        assert_eq!(all.len(), 2, "the tombstone stays so the removal can sync");
        assert!(
            !remove(&mut all, 1),
            "removing twice is not a second removal"
        );
    }

    #[test]
    fn only_yesterdays_unstarted_work_is_offered_back() {
        let mut all = vec![
            entry(1, "2026-10-29", "abandoned", T),
            entry(2, "2026-10-29", "abandoned too", T + 1),
            entry(3, "2026-10-30", "yesterday", T + 2),
            entry(4, D, "today", T + 3),
        ];
        mark_opened(&mut all, 2, T + 10);
        let carried = carry_over(&all, D);
        let labels: Vec<&str> = carried.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["abandoned", "yesterday"],
            "oldest day first, and a block that was started is not offered again"
        );
    }

    #[test]
    fn replanning_records_a_new_decision_and_retires_the_old_one() {
        let stale = entry(7, "2026-10-30", "draft the contract", T);
        let fresh = replan(&stale, 8, D, T + 86_400);
        assert_eq!(fresh.day, D);
        assert_eq!(fresh.label, "draft the contract");
        assert_ne!(fresh.id, stale.id, "a new plan is a new row");
        assert_eq!(fresh.opened_at, 0, "and starts out unopened");
    }

    #[test]
    fn a_rota_entry_survives_a_json_round_trip() {
        let e = create(1, D, "write", Some(42), "the draft", 25, T, &[]).unwrap();
        let json = serde_json::to_string(&e).unwrap();
        let back: RotaEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn an_entry_written_before_a_field_existed_still_parses() {
        // `opened_at` and `task_id` are optional in the format, so an older
        // device's file must load rather than fail the whole store.
        let json = r#"{"id":1,"day":"2026-10-31","label":"write","minutes":25,"created_at":0}"#;
        let e: RotaEntry = serde_json::from_str(json).unwrap();
        assert!(e.is_pending());
        assert_eq!(e.task_id, None);
    }
}
