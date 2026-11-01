//! Saved plans: a day's rota written once and applied again later.
//!
//! The rota already answers *what now?* before the block starts. But most days
//! are not new: the same shape — three blocks of writing, a review, a reading
//! block — comes back every week, and writing it down again each morning is
//! work that produces nothing. A plan is that shape kept, named, and re-applied
//! to whatever day the user is standing in.
//!
//! **A plan is a shape, not a record.** It stores labels, lengths and decrees;
//! it does not store days, ids, `opened_at`, or task links. Two things follow
//! from that, and both are deliberate:
//!
//! * Applying a plan produces *new* rota rows in the target day. The plan
//!   itself is untouched, so the same shape can be applied next Monday without
//!   this Monday's history leaking into it.
//! * A plan never remembers which task an entry came from. A task link that
//!   outlived the task would be a dangling pointer dressed as data; the plan
//!   carries the words, which is what the block was ever about.
//!
//! **Applying is all-or-nothing.** A day holds at most
//! [`rota::MAX_ENTRIES`] pending blocks. Applying a six-block plan to a day
//! that has room for four could be "fill what fits", but a half-applied plan
//! looks exactly like a plan that only had four blocks — the user would not see
//! that two were dropped, and would notice only when the day ran short. So the
//! whole application is refused, with both numbers stated, and nothing is
//! written.
//!
//! Everything here is pure: the day, the instant and the ids to hand out are
//! all passed in, so the rules are tested without a clock or a store.

use crate::state::rota::{self, RotaEntry};
use serde::{Deserialize, Serialize};

/// The most plans one store keeps.
///
/// A plan list is a row of shapes the user recognises, not a library. Past this
/// the list stops being scannable and becomes something to search, which is a
/// different feature with a different cost.
pub const MAX_PLANS: usize = 24;

/// The longest plan name kept, in characters (not bytes).
pub const MAX_NAME_CHARS: usize = 60;

/// The most blocks one plan may hold. A plan is applied to a day, and a day
/// holds [`rota::MAX_ENTRIES`]; a plan larger than that could never be applied.
pub const MAX_BLOCKS_PER_PLAN: usize = rota::MAX_ENTRIES;

/// One block of a saved shape: what it is, how long, and the line it opens with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanBlock {
    /// What the block is, already normalised.
    pub label: String,
    /// How long it runs, in minutes.
    pub minutes: u64,
    /// The decree it opens with, or `""` for none.
    #[serde(default)]
    pub decree: String,
}

/// A named, reusable day shape.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    /// Stable identity from the store's counter.
    pub id: u64,
    /// What the user calls this shape ("a writing morning").
    pub name: String,
    /// The blocks, in the order they will be planned.
    pub blocks: Vec<PlanBlock>,
    /// Unix seconds when it was first saved.
    pub created_at: i64,
    /// Unix seconds of the last edit. Plans are mutable (a name is renamed, a
    /// block is dropped), so sync needs to know which copy of one plan is
    /// newer — the same per-record rule tasks already use.
    #[serde(default)]
    pub updated_at: i64,
    /// Set when the plan is deleted. The row stays so the deletion can travel
    /// to another device; a row that simply vanished is indistinguishable from
    /// one that device never received.
    #[serde(default)]
    pub deleted: bool,
}

impl Plan {
    /// Visible to the user at all: not a tombstone.
    pub fn is_live(&self) -> bool {
        !self.deleted
    }

    /// Minutes the plan adds up to, for the summary line.
    pub fn total_minutes(&self) -> u64 {
        self.blocks.iter().map(|b| b.minutes).sum()
    }
}

/// Why a plan was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Nothing but whitespace in the name.
    Empty,
    /// Name longer than [`MAX_NAME_CHARS`] characters after trimming.
    NameTooLong { len: usize },
    /// A control character a single-line name cannot carry.
    ControlChars,
    /// There is nothing to save: the day has no blocks on it.
    NoBlocks,
    /// More blocks than a day could ever hold.
    TooManyBlocks { len: usize },
    /// The store already holds [`MAX_PLANS`] plans.
    Full,
    /// The target day has room for fewer blocks than the plan holds.
    NoRoom { room: usize, wanted: usize },
    /// No live plan with that id.
    NotFound,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "a plan needs a name"),
            Self::NameTooLong { len } => write!(
                f,
                "too long: {len} characters, the limit is {MAX_NAME_CHARS}"
            ),
            Self::ControlChars => write!(f, "a plan name cannot contain line breaks"),
            Self::NoBlocks => write!(f, "the day has no blocks to save"),
            Self::TooManyBlocks { len } => write!(
                f,
                "{len} blocks is more than the {MAX_BLOCKS_PER_PLAN} a day holds"
            ),
            Self::Full => write!(f, "at most {MAX_PLANS} plans are kept"),
            Self::NoRoom { room, wanted } => write!(
                f,
                "the day has room for {room} block{} but this plan holds {wanted}",
                if *room == 1 { "" } else { "s" }
            ),
            Self::NotFound => write!(f, "that plan is no longer there"),
        }
    }
}

impl std::error::Error for PlanError {}

/// Check a raw plan name, returning the normalised text on success.
///
/// Emptiness is decided first, on the normalised text, so a pasted blank line
/// reports "needs a name" rather than line breaks — the same order every other
/// text field in the app uses, which is what keeps one rule from judging the
/// same input differently in two places.
pub fn validate_name(raw: &str) -> Result<String, PlanError> {
    let clean = crate::journal::inscription::normalize(raw);
    if clean.is_empty() {
        return Err(PlanError::Empty);
    }
    if clean.chars().any(|c| c.is_control()) {
        return Err(PlanError::ControlChars);
    }
    let len = clean.chars().count();
    if len > MAX_NAME_CHARS {
        return Err(PlanError::NameTooLong { len });
    }
    Ok(clean)
}

/// Turn a day's rota into a saved shape.
///
/// Every live entry of the day is captured, started or not: the plan is the
/// shape the day was *meant* to have, and dropping the blocks that were already
/// begun would save a plan that gets smaller the later in the day it is saved.
/// Task links are deliberately not carried — see the module note.
pub fn capture(
    next_id: u64,
    raw_name: &str,
    entries: &[RotaEntry],
    now: i64,
    all: &[Plan],
) -> Result<Plan, PlanError> {
    let name = validate_name(raw_name)?;
    let blocks: Vec<PlanBlock> = entries
        .iter()
        .filter(|e| e.is_live())
        .map(|e| PlanBlock {
            label: e.label.clone(),
            minutes: e.minutes,
            decree: e.decree.clone(),
        })
        .collect();
    if blocks.is_empty() {
        return Err(PlanError::NoBlocks);
    }
    if blocks.len() > MAX_BLOCKS_PER_PLAN {
        return Err(PlanError::TooManyBlocks { len: blocks.len() });
    }
    if live(all).len() >= MAX_PLANS {
        return Err(PlanError::Full);
    }
    Ok(Plan {
        id: next_id,
        name,
        blocks,
        created_at: now,
        updated_at: now,
        deleted: false,
    })
}

/// The rota rows a plan becomes on `day`.
///
/// `first_id` is the first id to hand out; the caller's counter is advanced by
/// `rows.len()` afterwards. `created_at` is staggered by one second per row so
/// the day's list keeps the plan's order — [`rota::for_day`] sorts by creation
/// time, and identical stamps would let the order fall back to id, which is only
/// correct by luck.
///
/// Refused entirely when the day has room for fewer blocks than the plan holds:
/// a plan that half-applied would look like a shorter plan, and the user would
/// find out by running out of day.
pub fn apply(
    plan: &Plan,
    day: &str,
    first_id: u64,
    now: i64,
    existing: &[RotaEntry],
) -> Result<Vec<RotaEntry>, PlanError> {
    if plan.deleted {
        return Err(PlanError::NotFound);
    }
    if plan.blocks.is_empty() {
        return Err(PlanError::NoBlocks);
    }
    let room = rota::MAX_ENTRIES.saturating_sub(rota::pending_count(existing, day));
    if plan.blocks.len() > room {
        return Err(PlanError::NoRoom {
            room,
            wanted: plan.blocks.len(),
        });
    }
    let mut out = Vec::with_capacity(plan.blocks.len());
    for (i, b) in plan.blocks.iter().enumerate() {
        // Built through `rota::create` rather than by struct literal, so an
        // applied block is judged by exactly the rules a typed one is. A plan
        // that somehow holds a label the rota would refuse fails loudly here
        // instead of being written into the store unvalidated.
        let entry = rota::create(
            first_id + i as u64,
            day,
            &b.label,
            None,
            &b.decree,
            b.minutes,
            now + i as i64,
            existing,
        )
        .map_err(|e| match e {
            rota::RotaError::Full => PlanError::NoRoom {
                room: 0,
                wanted: plan.blocks.len(),
            },
            rota::RotaError::MinutesOutOfRange { minutes } => PlanError::TooManyBlocks {
                // Not reachable for a plan built by `capture`, which validates
                // its lengths; kept so a hand-edited data.json cannot sneak an
                // impossible block into a day.
                len: minutes as usize,
            },
            _ => PlanError::NoBlocks,
        })?;
        out.push(entry);
    }
    Ok(out)
}

/// The live plans, most recently edited first.
pub fn live(all: &[Plan]) -> Vec<&Plan> {
    let mut out: Vec<&Plan> = all.iter().filter(|p| p.is_live()).collect();
    out.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    out
}

/// Find one live plan by id.
pub fn find(all: &[Plan], id: u64) -> Option<&Plan> {
    all.iter().find(|p| p.id == id && p.is_live())
}

/// Take a plan off the list by marking it. A wrong shape is not rewritten.
pub fn remove(all: &mut [Plan], id: u64, now: i64) -> bool {
    match all.iter_mut().find(|p| p.id == id) {
        Some(p) if !p.deleted => {
            p.deleted = true;
            p.updated_at = now;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: &str = "2026-10-31";
    const T: i64 = 1_793_431_800;

    fn entry(id: u64, label: &str, minutes: u64, created_at: i64) -> RotaEntry {
        rota::create(id, D, label, None, "", minutes, created_at, &[]).unwrap()
    }

    fn plan(id: u64, name: &str, blocks: Vec<PlanBlock>) -> Plan {
        Plan {
            id,
            name: name.into(),
            blocks,
            created_at: T,
            updated_at: T,
            deleted: false,
        }
    }

    fn block(label: &str, minutes: u64) -> PlanBlock {
        PlanBlock {
            label: label.into(),
            minutes,
            decree: String::new(),
        }
    }

    #[test]
    fn a_blank_name_is_refused_before_anything_else() {
        assert_eq!(validate_name(""), Err(PlanError::Empty));
        assert_eq!(validate_name("  \t "), Err(PlanError::Empty));
    }

    #[test]
    fn a_name_is_collapsed_to_one_line() {
        assert_eq!(
            validate_name("  writing   morning ").unwrap(),
            "writing morning"
        );
        assert_eq!(validate_name("deep\nwork").unwrap(), "deep work");
    }

    #[test]
    fn a_name_cap_counts_characters_not_bytes() {
        let long: String = "ا".repeat(MAX_NAME_CHARS + 1);
        assert_eq!(
            validate_name(&long),
            Err(PlanError::NameTooLong {
                len: MAX_NAME_CHARS + 1
            })
        );
        assert!(validate_name(&"ا".repeat(MAX_NAME_CHARS)).is_ok());
    }

    #[test]
    fn capturing_a_day_keeps_every_block_started_or_not() {
        let mut day = vec![entry(1, "draft", 50, T), entry(2, "review", 25, T + 1)];
        rota::mark_opened(&mut day, 1, T + 60);
        let p = capture(1, "writing morning", &day, T + 120, &[]).unwrap();
        assert_eq!(
            p.blocks.len(),
            2,
            "a started block is still part of the shape"
        );
        assert_eq!(p.blocks[0].label, "draft");
        assert_eq!(p.blocks[1].minutes, 25);
        assert_eq!(p.total_minutes(), 75);
    }

    #[test]
    fn capturing_a_removed_block_leaves_it_out() {
        let mut day = vec![entry(1, "draft", 50, T), entry(2, "review", 25, T + 1)];
        rota::remove(&mut day, 2);
        let p = capture(1, "short", &day, T + 60, &[]).unwrap();
        assert_eq!(p.blocks.len(), 1);
        assert_eq!(p.blocks[0].label, "draft");
    }

    #[test]
    fn an_empty_day_has_no_shape_to_save() {
        assert_eq!(capture(1, "nothing", &[], T, &[]), Err(PlanError::NoBlocks));
    }

    #[test]
    fn a_store_holds_at_most_the_plan_cap() {
        let all: Vec<Plan> = (1..=MAX_PLANS as u64)
            .map(|i| plan(i, &format!("plan {i}"), vec![block("x", 25)]))
            .collect();
        let day = vec![entry(1, "draft", 25, T)];
        assert_eq!(
            capture(999, "one too many", &day, T, &all),
            Err(PlanError::Full)
        );
    }

    #[test]
    fn applying_a_plan_writes_new_rows_in_order() {
        let p = plan(
            1,
            "morning",
            vec![block("draft", 50), block("review", 25), block("read", 30)],
        );
        let rows = apply(&p, D, 10, T, &[]).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, 10);
        assert_eq!(rows[2].id, 12);
        assert_eq!(rows[0].label, "draft");
        assert_eq!(rows[0].minutes, 50);
        // Staggered stamps so the day's own sort keeps the plan's order.
        assert!(rows[0].created_at < rows[1].created_at);
        assert_eq!(rows[0].day, D);
    }

    #[test]
    fn applying_a_plan_does_not_touch_the_plan() {
        let p = plan(1, "morning", vec![block("draft", 50)]);
        let before = p.clone();
        let _ = apply(&p, D, 10, T, &[]).unwrap();
        assert_eq!(p, before, "applying is a read of the plan, never an edit");
    }

    #[test]
    fn a_plan_carries_its_decrees_into_the_new_day() {
        let mut b = block("draft", 50);
        b.decree = "the whole first section".into();
        let p = plan(1, "morning", vec![b]);
        let rows = apply(&p, D, 10, T, &[]).unwrap();
        assert_eq!(rows[0].decree, "the whole first section");
    }

    #[test]
    fn applying_is_refused_whole_when_the_day_is_nearly_full() {
        // A day at 18 of 20 pending blocks has room for 2.
        let mut existing: Vec<RotaEntry> = Vec::new();
        for i in 1..=18u64 {
            existing.push(entry(i, &format!("b{i}"), 25, T + i as i64));
        }
        let p = plan(
            1,
            "three",
            vec![block("a", 25), block("b", 25), block("c", 25)],
        );
        assert_eq!(
            apply(&p, D, 100, T, &existing),
            Err(PlanError::NoRoom { room: 2, wanted: 3 })
        );
    }

    #[test]
    fn a_plan_that_exactly_fills_the_room_is_applied() {
        let mut existing: Vec<RotaEntry> = Vec::new();
        for i in 1..=18u64 {
            existing.push(entry(i, &format!("b{i}"), 25, T + i as i64));
        }
        let p = plan(1, "two", vec![block("a", 25), block("b", 25)]);
        assert_eq!(apply(&p, D, 100, T, &existing).unwrap().len(), 2);
    }

    #[test]
    fn a_deleted_plan_cannot_be_applied() {
        let mut p = plan(1, "gone", vec![block("a", 25)]);
        p.deleted = true;
        assert_eq!(apply(&p, D, 10, T, &[]), Err(PlanError::NotFound));
    }

    #[test]
    fn a_removed_plan_leaves_the_list_but_stays_in_the_store() {
        let mut all = vec![
            plan(1, "one", vec![block("a", 25)]),
            plan(2, "two", vec![block("b", 25)]),
        ];
        assert!(remove(&mut all, 1, T));
        assert_eq!(live(&all).len(), 1);
        assert_eq!(all.len(), 2, "the tombstone stays so the removal can sync");
        assert!(!remove(&mut all, 1, T), "removing twice is not a removal");
    }

    #[test]
    fn the_list_is_most_recently_edited_first() {
        let mut a = plan(1, "older", vec![block("a", 25)]);
        a.updated_at = T;
        let mut b = plan(2, "newer", vec![block("b", 25)]);
        b.updated_at = T + 100;
        let all = vec![a, b];
        assert_eq!(live(&all)[0].name, "newer");
    }
}
