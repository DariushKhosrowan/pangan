//! What became of a planned block: the plan's own record of its outcome.
//!
//! The [rota](super::rota) already records that a block was *begun* — `opened_at`
//! is set the moment the clock starts. That was the honest limit while the
//! session carried no identity a plan could point at. But it leaves the day
//! unable to answer its own question. A block started and abandoned at minute
//! two, and a block carried through to the bell, both sit on the rota with a
//! non-zero `opened_at` and nothing to tell them apart. The list says the work
//! was taken up; it cannot say it was done.
//!
//! An outcome closes that gap with one fact the app can actually witness: the
//! rota entry whose block was running when a session landed. No guess, no user
//! input, no inference from shape — the plan that was open is the plan the
//! session belonged to.
//!
//! **Why this lives on the entry rather than in its own collection.** A
//! fulfilment row keyed by `(entry_kind, entry_id, day)` would carry three
//! fields that the entry above it already has, and every one of them would be a
//! second copy free to disagree with the first. The entry *is* the plan; the
//! outcome is a fact about it. Keeping it there also means it inherits the
//! entry's tombstone (a removed plan takes its outcome with it) and the entry's
//! merge rule for free, instead of needing a third identity scheme to sync.
//!
//! **Why the state is four words and not a boolean.** "Not done" is not one
//! thing. A block the user deliberately put off and a block that simply never
//! came round are different failures with different fixes — one is a plan the
//! day outgrew, the other is a plan that was never really meant. Collapsing
//! them into `done: false` would lose the only distinction worth reading back.
//!
//! Everything here is pure: a slice of entries in, a slice or a count out. No
//! clock reads, no I/O, no UI.

use super::rota::RotaEntry;

/// What became of one planned block.
///
/// Ordered as the day reads: what was done, what was started, what was never
/// reached, and what the user chose to put off. [`Outcome::rank`] follows this
/// order only to make two devices converge, never to judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A session landed while this block was open. The plan was carried out.
    Done,
    /// The block was started and no session landed. It was abandoned part-way,
    /// which is a fact the clock can see and the user usually cannot.
    Abandoned,
    /// Never started, and the day it was planned for is over.
    Missed,
    /// Never started, and the day it was planned for is still running.
    Pending,
}

impl Outcome {
    /// Every outcome, in reading order.
    pub const ALL: [Outcome; 4] = [
        Outcome::Done,
        Outcome::Abandoned,
        Outcome::Missed,
        Outcome::Pending,
    ];

    /// The stable storage form, so a hand-read `data.json` says `"done"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Done => "done",
            Outcome::Abandoned => "abandoned",
            Outcome::Missed => "missed",
            Outcome::Pending => "pending",
        }
    }

    /// Read a stored form back. Unknown text is `None` rather than a default:
    /// a value this build cannot read is a value from a newer build, and
    /// guessing "pending" would silently un-do a finished block.
    pub fn parse(raw: &str) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|o| o.as_str() == raw)
    }

    /// Tie-break rank. Only used to make merges converge, so it is
    /// `pub(crate)`: the ordering is a sync detail, not something a caller
    /// should read as a judgement about the user's day.
    pub(crate) fn rank(self) -> u8 {
        match self {
            Outcome::Done => 0,
            Outcome::Abandoned => 1,
            Outcome::Missed => 2,
            Outcome::Pending => 3,
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Attach the session that just landed to the entry its block belonged to.
///
/// Returns the id of the entry that was claimed, so the caller can say which
/// row changed. `None` means no entry was open — a block started from the plain
/// timer rather than from a plan, which is not an error and must not invent a
/// plan to attach itself to.
///
/// **Only an entry that is open and unfinished may be claimed.** Claiming a
/// pending entry would mark work as done that the user never started; claiming
/// one already finished would let a second, later session overwrite the record
/// of the first. Both are refused by the same condition.
///
/// When several entries are open — which a hurried user can produce by starting
/// a block, resetting, and starting another — the **most recently opened** one
/// is claimed, because that is the block that was actually running.
pub fn claim(all: &mut [RotaEntry], session_at: i64) -> Option<u64> {
    let id = all
        .iter()
        .filter(|e| e.is_live() && e.opened_at > 0 && e.outcome.is_none())
        .max_by_key(|e| e.opened_at)
        .map(|e| e.id)?;
    let entry = all.iter_mut().find(|e| e.id == id)?;
    entry.session_at = session_at;
    entry.outcome = Some(Outcome::Done.as_str().to_string());
    Some(id)
}

/// Record that a started block was abandoned, because the clock was reset
/// before any session landed.
///
/// Returns the ids that changed. More than one is normal: a user who opens a
/// block, resets, and opens another leaves the first one abandoned, and the
/// moment to say so is when the reset happens rather than at the end of the day
/// when the reason is forgotten.
///
/// An entry already marked done is left alone. A session that landed is a
/// stronger fact than a reset that followed it, and turning done into
/// abandoned would erase a block that really was carried through.
pub fn abandon_open(all: &mut [RotaEntry]) -> Vec<u64> {
    let mut changed = Vec::new();
    for e in all.iter_mut() {
        if e.is_live() && e.opened_at > 0 && e.outcome.is_none() {
            e.outcome = Some(Outcome::Abandoned.as_str().to_string());
            changed.push(e.id);
        }
    }
    changed
}

/// What one entry's outcome is, given the day it was planned for and today.
///
/// The stored outcome wins when there is one. Otherwise the entry is judged by
/// where it stands: open means the block is running now, a pending entry on a
/// past day was missed, and a pending entry today is simply still to come.
///
/// This is the only place "missed" is derived, so a screen cannot decide for
/// itself that a stale entry was missed and start contradicting the count.
pub fn effective(entry: &RotaEntry, today: &str) -> Outcome {
    if let Some(o) = entry.outcome.as_deref().and_then(Outcome::parse) {
        return o;
    }
    if entry.opened_at > 0 {
        // Open with no outcome yet: the block is running right now. A reset
        // moves it to `abandoned` explicitly, so this is never a stale guess.
        Outcome::Pending
    } else if entry.day.as_str() < today {
        Outcome::Missed
    } else {
        Outcome::Pending
    }
}

/// What a day's plans added up to.
///
/// `planned` counts every live entry for the day, so `done + missed + pending`
/// always equals it — a total that cannot be reconciled with its parts is a
/// total the user stops checking.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DayTally {
    pub planned: u32,
    pub done: u32,
    pub abandoned: u32,
    pub missed: u32,
    pub pending: u32,
}

impl DayTally {
    /// True when the day had no plans at all — the UI says nothing rather than
    /// drawing an empty scoreboard.
    pub fn is_empty(&self) -> bool {
        self.planned == 0
    }

    /// Plan fulfilment rate, 0.0–1.0: the share of the day's planned blocks
    /// that became completed focus sessions.
    ///
    /// `None` when the day had no plans — an empty day is not a 0% day, any
    /// more than it is a 100% one. A block the user deliberately put off and a
    /// block that never started are both "not done", so fulfilment counts only
    /// what actually landed; `done / planned` is the whole story.
    ///
    /// Intentionally exposed as a pure, unit-tested metric rather than wired
    /// into a screen: the day card reads back what happened instead of grading
    /// it, so a percentage has no home there yet. Callers (and future UI) use
    /// this; nothing in the binary currently does.
    #[allow(dead_code)]
    pub fn fulfillment(&self) -> Option<f64> {
        if self.planned == 0 {
            None
        } else {
            Some(self.done as f64 / self.planned as f64)
        }
    }
}

/// Count a day's entries by outcome.
///
/// Tombstones are skipped: a plan the user removed is not a plan they missed,
/// and counting it would turn editing the list into failing it.
pub fn tally(all: &[RotaEntry], day: &str, today: &str) -> DayTally {
    let mut t = DayTally::default();
    for e in all.iter().filter(|e| e.is_live() && e.day == day) {
        t.planned += 1;
        match effective(e, today) {
            Outcome::Done => t.done += 1,
            Outcome::Abandoned => t.abandoned += 1,
            Outcome::Missed => t.missed += 1,
            Outcome::Pending => t.pending += 1,
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::rota;

    fn entry(id: u64, day: &str) -> RotaEntry {
        rota::create(id, day, "write the thing", None, "", 25, 1_000, &[]).unwrap()
    }

    fn opened(mut e: RotaEntry, at: i64) -> RotaEntry {
        e.opened_at = at;
        e
    }

    #[test]
    fn an_entry_with_no_outcome_on_a_past_day_is_missed() {
        let e = entry(1, "2026-10-01");
        assert_eq!(effective(&e, "2026-10-02"), Outcome::Missed);
    }

    #[test]
    fn an_entry_with_no_outcome_today_is_still_pending() {
        let e = entry(1, "2026-10-02");
        assert_eq!(effective(&e, "2026-10-02"), Outcome::Pending);
    }

    #[test]
    fn a_stored_outcome_wins_over_the_day_rule() {
        let mut e = entry(1, "2026-10-01");
        e.outcome = Some(Outcome::Done.as_str().to_string());
        assert_eq!(effective(&e, "2026-12-31"), Outcome::Done);
    }

    #[test]
    fn claiming_attaches_the_session_to_the_open_entry() {
        let mut all = vec![opened(entry(1, "2026-10-02"), 5_000)];
        let id = claim(&mut all, 9_000).expect("an open entry to claim");
        assert_eq!(id, 1);
        assert_eq!(all[0].session_at, 9_000);
        assert_eq!(all[0].outcome.as_deref(), Some("done"));
    }

    #[test]
    fn claiming_never_marks_a_pending_entry_done() {
        let mut all = vec![entry(1, "2026-10-02")];
        assert_eq!(claim(&mut all, 9_000), None);
        assert!(all[0].outcome.is_none());
        assert_eq!(all[0].session_at, 0);
    }

    #[test]
    fn claiming_never_overwrites_an_entry_that_already_finished() {
        let mut all = vec![opened(entry(1, "2026-10-02"), 5_000)];
        assert_eq!(claim(&mut all, 9_000), Some(1));
        // A second session must not steal the first one's row.
        assert_eq!(claim(&mut all, 9_600), None);
        assert_eq!(all[0].session_at, 9_000);
    }

    #[test]
    fn claiming_with_two_blocks_open_takes_the_most_recent() {
        // A user started a block, reset, and started another: the block that
        // was actually running when the session landed is the later one.
        let mut all = vec![
            opened(entry(1, "2026-10-02"), 5_000),
            opened(entry(2, "2026-10-02"), 7_000),
        ];
        assert_eq!(claim(&mut all, 9_000), Some(2));
        assert_eq!(all[0].session_at, 0);
        assert_eq!(all[1].session_at, 9_000);
    }

    #[test]
    fn claiming_skips_a_removed_entry() {
        let mut e = opened(entry(1, "2026-10-02"), 5_000);
        e.deleted = true;
        let mut all = vec![e];
        assert_eq!(claim(&mut all, 9_000), None);
    }

    #[test]
    fn abandoning_marks_every_open_entry_and_returns_their_ids() {
        let mut all = vec![
            opened(entry(1, "2026-10-02"), 5_000),
            opened(entry(2, "2026-10-02"), 7_000),
        ];
        assert_eq!(abandon_open(&mut all), vec![1, 2]);
        assert_eq!(all[0].outcome.as_deref(), Some("abandoned"));
        assert_eq!(all[1].outcome.as_deref(), Some("abandoned"));
    }

    #[test]
    fn abandoning_leaves_a_done_entry_alone() {
        let mut all = vec![opened(entry(1, "2026-10-02"), 5_000)];
        claim(&mut all, 9_000);
        assert!(abandon_open(&mut all).is_empty());
        assert_eq!(all[0].outcome.as_deref(), Some("done"));
    }

    #[test]
    fn abandoning_does_nothing_to_a_pending_entry() {
        let mut all = vec![entry(1, "2026-10-02")];
        assert!(abandon_open(&mut all).is_empty());
        assert!(all[0].outcome.is_none());
    }

    #[test]
    fn the_tally_counts_every_state_and_adds_up_to_the_day() {
        let today = "2026-10-02";
        let mut done = opened(entry(1, today), 5_000);
        done.outcome = Some("done".into());
        let mut abandoned = opened(entry(2, today), 5_100);
        abandoned.outcome = Some("abandoned".into());
        let pending = entry(3, today);
        let stale = entry(4, "2026-10-01");
        let all = vec![done, abandoned, pending, stale];

        let t = tally(&all, today, today);
        assert_eq!(t.planned, 3, "a stale entry is not part of today's count");
        assert_eq!(t.done, 1);
        assert_eq!(t.abandoned, 1);
        assert_eq!(t.pending, 1);
        assert_eq!(t.missed, 0);
        assert_eq!(
            t.done + t.abandoned + t.missed + t.pending,
            t.planned,
            "the parts must add up to the whole"
        );
        assert_eq!(t.abandoned + t.missed, 1, "one plan went undone");

        // The same entry counted on the day it was planned for is missed.
        let t = tally(&all, "2026-10-01", today);
        assert_eq!(t.planned, 1);
        assert_eq!(t.missed, 1);
    }

    #[test]
    fn a_tombstoned_plan_is_not_counted_as_missed() {
        let mut e = entry(1, "2026-10-01");
        e.deleted = true;
        let t = tally(&[e], "2026-10-01", "2026-10-02");
        assert!(t.is_empty(), "removing a plan is not missing it");
    }

    #[test]
    fn an_unknown_stored_outcome_reads_as_none_rather_than_a_guess() {
        // A value from a newer build must not be silently reinterpreted.
        assert_eq!(Outcome::parse("deferred"), None);
        assert_eq!(Outcome::parse("done"), Some(Outcome::Done));
    }

    #[test]
    fn outcomes_round_trip_through_their_storage_form() {
        for o in Outcome::ALL {
            assert_eq!(Outcome::parse(o.as_str()), Some(o));
        }
    }

    #[test]
    fn fulfillment_is_none_when_there_were_no_plans() {
        let t = DayTally::default();
        assert_eq!(t.fulfillment(), None, "an empty day is not a 0% day");
    }

    #[test]
    fn fulfillment_is_one_when_every_plan_landed() {
        let t = DayTally {
            planned: 4,
            done: 4,
            ..Default::default()
        };
        assert_eq!(t.fulfillment(), Some(1.0));
    }

    #[test]
    fn fulfillment_counts_only_what_landed() {
        // 5 planned: 3 done, 1 abandoned, 1 missed. Fulfilment is 3/5 — a
        // block the user put off is still a block that did not land.
        let t = DayTally {
            planned: 5,
            done: 3,
            abandoned: 1,
            missed: 1,
            ..Default::default()
        };
        let f = t.fulfillment().unwrap();
        assert!((f - 0.6).abs() < 1e-9);
    }

    #[test]
    fn fulfillment_ignores_still_pending_blocks() {
        // Pending blocks have not had their verdict yet; they are neither done
        // nor failed, so they simply reduce the denominator's *potential* —
        // the rate is still done over planned-for-the-day.
        let t = DayTally {
            planned: 4,
            done: 2,
            pending: 2,
            ..Default::default()
        };
        assert_eq!(t.fulfillment(), Some(0.5));
    }
}
