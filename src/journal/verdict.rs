//! Verdicts: what the user says a block actually was.
//!
//! The [reckoning](super::reckoning) infers keptness from shape — a decree with
//! an inscription facing it counts as kept, a decree alone counts as broken.
//! That inference is honest about what it can see, but it is still a guess, and
//! it is wrong in exactly the cases that matter most: a block written through
//! for the sake of closing it was **not** kept, and a block that ran long past
//! its decree was not *broken* either. Only the person who sat there knows.
//!
//! A verdict is that knowledge, said once: five words, chosen by the user, for
//! the block they just finished. Between them they cover what actually happens
//! to a plan in a day — it was done, half done, not done, it outgrew itself, or
//! it was cut short before it could become anything.
//!
//! **A verdict is a statement about a session, not about an inscription.** The
//! key is `session_at`, the same key the decree and the inscription already join
//! on. An inscription's own `id` is derived from its text, so keying a verdict on
//! it would mean that editing one word of the inscription detached the verdict
//! from the block it described. The session is the thing that happened; the words
//! about it are the writing.
//!
//! **Declared where declared, inferred where not.** [`tally`] counts both, and
//! reports which is which, so the read-back can say "kept 4, of which 2 you
//! judged and 2 were inferred from the shape". A count that silently mixes a
//! guess with a fact is the kind of number that stops being read.
//!
//! Everything here is pure: a span and four slices in, a tally out. No clock
//! reads, no I/O, no UI.

use crate::journal::{Decree, Inscription};
use crate::stats::report::Range;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The five things a block can have been.
///
/// Ordered from best to worst in [`VerdictKind::rank`], which is only used to
/// break ties deterministically when two devices disagree; the order is not a
/// judgement about the user's week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictKind {
    /// Done, as promised.
    Kept,
    /// Some of it. The block happened, the decree is only half answered.
    Partial,
    /// Not done at all.
    Broken,
    /// Done, and it ran past what was promised — the decree was too small.
    Extended,
    /// Stopped before it could become anything.
    Short,
}

impl VerdictKind {
    /// Every kind, in the order the seals are drawn.
    pub const ALL: [VerdictKind; 5] = [
        VerdictKind::Kept,
        VerdictKind::Partial,
        VerdictKind::Broken,
        VerdictKind::Extended,
        VerdictKind::Short,
    ];

    /// Did the block meet what it was for?
    ///
    /// `Extended` counts: a block that ran past its decree did not fail the
    /// promise, it outgrew it. `Short` does not: it stopped early, whatever it
    /// might have become.
    pub fn is_met(self) -> bool {
        matches!(self, VerdictKind::Kept | VerdictKind::Extended)
    }

    /// The stable storage form, so a hand-read `data.json` says `"kept"`.
    pub fn as_str(self) -> &'static str {
        match self {
            VerdictKind::Kept => "kept",
            VerdictKind::Partial => "partial",
            VerdictKind::Broken => "broken",
            VerdictKind::Extended => "extended",
            VerdictKind::Short => "short",
        }
    }

    /// Tie-break rank, best first. Only used to make merges converge, so it is
    /// `pub(crate)` rather than public: the ordering is a sync detail, not
    /// something a caller should read as a judgement about the user's week.
    pub(crate) fn rank(self) -> u8 {
        match self {
            VerdictKind::Kept => 0,
            VerdictKind::Extended => 1,
            VerdictKind::Partial => 2,
            VerdictKind::Short => 3,
            VerdictKind::Broken => 4,
        }
    }
}

impl std::fmt::Display for VerdictKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One declared verdict.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// Unix seconds of the session this judges — the key it shares with the
    /// decree and the inscription, and the reason editing an inscription's
    /// words cannot detach its verdict.
    pub session_at: i64,
    /// What the user said it was.
    pub kind: VerdictKind,
    /// Unix seconds when the verdict was last declared. Doubles as the
    /// merge stamp: re-declaring a block replaces the earlier word, so sync
    /// needs to know which copy is newer.
    pub declared_at: i64,
    /// Set when the user takes the verdict back. The row stays so the
    /// withdrawal can reach the other device; a row that simply vanished is
    /// indistinguishable from one the other device never received.
    #[serde(default)]
    pub deleted: bool,
}

impl Verdict {
    /// Visible to the user at all: not a withdrawn verdict.
    pub fn is_live(&self) -> bool {
        !self.deleted
    }
}

/// Why a verdict was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerdictError {
    /// A session timestamp of zero is not a session.
    NoSession,
}

impl std::fmt::Display for VerdictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSession => write!(f, "a verdict needs a session to judge"),
        }
    }
}

impl std::error::Error for VerdictError {}

/// What one session's record adds up to, and whether the user said it.
///
/// `declared` is the part that matters: the archive shows a filled seal when
/// the user judged the block, and a hollow one when the app inferred it. A
/// guess drawn the same as a fact is a guess the user will trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effective {
    pub kind: VerdictKind,
    pub declared: bool,
}

/// Say what a block was, replacing any earlier word about the same session.
///
/// Returns the new row rather than mutating, so the caller owns the store
/// write — the same shape every other rule in the journal uses.
///
/// A re-declaration **replaces** rather than appends. Two verdicts for one
/// block would be two answers to one question, and the tally would have to
/// pick between them; picking is the wrong job for a counter.
pub fn declare(session_at: i64, kind: VerdictKind, at: i64) -> Result<Verdict, VerdictError> {
    if session_at == 0 {
        return Err(VerdictError::NoSession);
    }
    Ok(Verdict {
        session_at,
        kind,
        declared_at: at,
        deleted: false,
    })
}

/// What the record says a session was: the user's word if given, otherwise the
/// shape's.
///
/// Inference, in order:
///
/// * an inscription exists → `Kept`. Words were written afterwards, so the block
///   produced something. This is the weakest inference in the module: it cannot
///   tell a real inscription from one typed to close the row, which is precisely
///   the gap verdicts exist to fill.
/// * a decree but no inscription → `Broken`. The block was spoken for and left
///   nothing facing it.
/// * neither → `None`. A block that promised nothing and wrote nothing is not a
///   failed intention, it is just a block; judging it would invent a promise on
///   the user's behalf.
pub fn effective(
    declared: Option<VerdictKind>,
    decree: Option<&Decree>,
    inscription: Option<&Inscription>,
) -> Option<Effective> {
    if let Some(kind) = declared {
        return Some(Effective {
            kind,
            declared: true,
        });
    }
    let kind = match (decree, inscription) {
        (Some(_), None) => VerdictKind::Broken,
        (_, Some(_)) => VerdictKind::Kept,
        (None, None) => return None,
    };
    Some(Effective {
        kind,
        declared: false,
    })
}

/// One session's record, resolved for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judged {
    pub session_at: i64,
    pub kind: VerdictKind,
    /// True when the user said it; false when the shape did.
    pub declared: bool,
}

/// What a span's blocks add up to, by verdict.
///
/// Five counts and two totals, because the five are not interchangeable and
/// summing them into one "delivered" figure is the flattening this module
/// exists to undo. `declared` + `inferred` always equals the sum of the five,
/// which is what lets the UI state its own confidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub kept: u32,
    pub partial: u32,
    pub broken: u32,
    pub extended: u32,
    pub short: u32,
    /// Blocks the user judged.
    pub declared: u32,
    /// Blocks the app guessed at.
    pub inferred: u32,
}

impl Tally {
    /// Every judged block in the span, however it was judged.
    pub fn total(&self) -> u32 {
        self.kept + self.partial + self.broken + self.extended + self.short
    }

    /// Blocks that met what they were for: kept, or outgrew the decree.
    ///
    /// Expressed through [`VerdictKind::is_met`] rather than as `kept +
    /// extended`, so the ruling about what counts as met lives in exactly one
    /// place. Two copies of that rule is how a count and a badge end up
    /// disagreeing about the same week.
    pub fn met(&self) -> u32 {
        [
            (VerdictKind::Kept, self.kept),
            (VerdictKind::Extended, self.extended),
            (VerdictKind::Partial, 0),
            (VerdictKind::Broken, 0),
            (VerdictKind::Short, 0),
        ]
        .iter()
        .filter(|(k, _)| k.is_met())
        .map(|(_, n)| n)
        .sum()
    }

    /// Nothing to say about the span.
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    fn count(&mut self, kind: VerdictKind) {
        match kind {
            VerdictKind::Kept => self.kept += 1,
            VerdictKind::Partial => self.partial += 1,
            VerdictKind::Broken => self.broken += 1,
            VerdictKind::Extended => self.extended += 1,
            VerdictKind::Short => self.short += 1,
        }
    }
}

/// The judgement of every block in the span, newest first.
///
/// One row per session that has anything to judge, taken from the union of the
/// three lists: a session can be judged because a verdict names it, because a
/// decree promised something in it, or because words were written about it.
/// Taking only the decrees would silently drop a block the user wrote up
/// without a promise, which is exactly the block they bothered to judge.
pub fn judged(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    span: Range,
) -> Vec<Judged> {
    let in_span = |at: i64| day_of(at).map(|d| span.contains(d)).unwrap_or(false);

    // Later copies win by `declared_at`, so a re-declaration cannot be shadowed
    // by the word it replaced.
    let mut said: BTreeMap<i64, &Verdict> = BTreeMap::new();
    for v in verdicts
        .iter()
        .filter(|v| v.is_live() && in_span(v.session_at))
    {
        // Later declaration wins; equal stamps keep the first seen, which is
        // stable because `merge` has already collapsed duplicates to one row
        // per session before the list reaches here.
        let take = said
            .get(&v.session_at)
            .map(|prev| prev.declared_at <= v.declared_at)
            .unwrap_or(true);
        if take {
            said.insert(v.session_at, v);
        }
    }

    let live_decrees: BTreeMap<i64, &Decree> = decrees
        .iter()
        .filter(|d| !d.deleted && in_span(d.session_at))
        .map(|d| (d.session_at, d))
        .collect();
    let live_inscriptions: BTreeMap<i64, &Inscription> = inscriptions
        .iter()
        .filter(|i| !i.deleted && in_span(i.session_at))
        .map(|i| (i.session_at, i))
        .collect();

    let mut sessions: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    sessions.extend(live_decrees.keys().copied());
    sessions.extend(live_inscriptions.keys().copied());
    sessions.extend(said.keys().copied());

    let mut out: Vec<Judged> = sessions
        .into_iter()
        .filter_map(|session_at| {
            let declared = said.get(&session_at).map(|v| v.kind);
            let eff = effective(
                declared,
                live_decrees.get(&session_at).copied(),
                live_inscriptions.get(&session_at).copied(),
            )?;
            Some(Judged {
                session_at,
                kind: eff.kind,
                declared: eff.declared,
            })
        })
        .collect();
    out.sort_by(|a, b| b.session_at.cmp(&a.session_at));
    out
}

/// Count what a span's blocks add up to, by verdict kind.
pub fn tally(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    verdicts: &[Verdict],
    span: Range,
) -> Tally {
    let mut t = Tally::default();
    for j in judged(decrees, inscriptions, verdicts, span) {
        t.count(j.kind);
        if j.declared {
            t.declared += 1;
        } else {
            t.inferred += 1;
        }
    }
    t
}

/// The live verdict for one session, if the user has judged it.
pub fn find(verdicts: &[Verdict], session_at: i64) -> Option<&Verdict> {
    verdicts
        .iter()
        .filter(|v| v.session_at == session_at && v.is_live())
        .max_by_key(|v| v.declared_at)
}

/// Take a verdict back by marking it. The block returns to being inferred.
pub fn retract(verdicts: &mut [Verdict], session_at: i64, now: i64) -> bool {
    let mut any = false;
    for v in verdicts.iter_mut() {
        if v.session_at == session_at && !v.deleted {
            v.deleted = true;
            v.declared_at = now;
            any = true;
        }
    }
    any
}

/// Fold a newly declared verdict into the list.
///
/// Kept here rather than in the store so the "one word per session" rule has a
/// single home, and so the sync path and the UI path cannot disagree about
/// whether a re-declaration appends or replaces.
pub fn put(verdicts: &mut Vec<Verdict>, v: Verdict) {
    verdicts.retain(|old| old.session_at != v.session_at);
    verdicts.push(v);
}

fn day_of(at: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp(at, 0).map(|u| u.with_timezone(&chrono::Local).date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{decree, inscription};
    use crate::stats::report::Range;

    /// 2026-10-31 09:00:00 +0330 — a Saturday, the first day of an Iranian week.
    const SAT: i64 = 1_793_431_800;
    const DAY: i64 = 86_400;

    fn today() -> Range {
        let d = day_of(SAT).unwrap();
        Range { start: d, end: d }
    }

    fn dec(at: i64, text: &str) -> Decree {
        decree::create(at, at - 1500, text).unwrap()
    }

    fn ins(at: i64, text: &str) -> Inscription {
        inscription::create(at, at, text).unwrap()
    }

    fn v(session_at: i64, kind: VerdictKind, at: i64) -> Verdict {
        Verdict {
            session_at,
            kind,
            declared_at: at,
            deleted: false,
        }
    }

    // ── declaring ────────────────────────────────────────────────────────

    #[test]
    fn a_verdict_needs_a_session_to_judge() {
        assert_eq!(
            declare(0, VerdictKind::Kept, SAT),
            Err(VerdictError::NoSession)
        );
    }

    #[test]
    fn declaring_a_second_time_replaces_the_first_word() {
        let mut all = Vec::new();
        put(&mut all, declare(SAT, VerdictKind::Broken, SAT).unwrap());
        put(&mut all, declare(SAT, VerdictKind::Kept, SAT + 60).unwrap());
        assert_eq!(all.len(), 1, "one block, one answer");
        assert_eq!(find(&all, SAT).unwrap().kind, VerdictKind::Kept);
    }

    #[test]
    fn the_newer_of_two_copies_of_one_verdict_is_the_one_read() {
        // Two devices can both hold a row for the same session; the later
        // declaration is the answer regardless of which arrived first.
        let all = vec![
            v(SAT, VerdictKind::Broken, SAT + 10),
            v(SAT, VerdictKind::Kept, SAT + 90),
        ];
        assert_eq!(find(&all, SAT).unwrap().kind, VerdictKind::Kept);
    }

    #[test]
    fn a_withdrawn_verdict_is_no_longer_found() {
        let mut all = vec![v(SAT, VerdictKind::Kept, SAT)];
        assert!(retract(&mut all, SAT, SAT + 5));
        assert!(
            find(&all, SAT).is_none(),
            "a retracted word stops being said"
        );
        assert_eq!(all.len(), 1, "the row stays so the withdrawal can sync");
        assert!(
            !retract(&mut all, SAT, SAT + 6),
            "retracting twice is not a retraction"
        );
    }

    #[test]
    fn retracting_one_session_leaves_another_alone() {
        let mut all = vec![
            v(SAT, VerdictKind::Kept, SAT),
            v(SAT + DAY, VerdictKind::Broken, SAT),
        ];
        retract(&mut all, SAT, SAT + 5);
        assert!(find(&all, SAT).is_none());
        assert!(find(&all, SAT + DAY).is_some());
    }

    // ── inference ────────────────────────────────────────────────────────

    #[test]
    fn a_declared_verdict_beats_the_shape() {
        // Words were written, so the shape would say Kept — but the user said
        // Broken, and the user sat there.
        let d = dec(SAT, "write the report");
        let i = ins(SAT, "wrote the report");
        let e = effective(Some(VerdictKind::Broken), Some(&d), Some(&i)).unwrap();
        assert_eq!(e.kind, VerdictKind::Broken);
        assert!(e.declared);
    }

    #[test]
    fn a_decree_with_nothing_facing_it_is_inferred_broken() {
        let d = dec(SAT, "write the report");
        let e = effective(None, Some(&d), None).unwrap();
        assert_eq!(e.kind, VerdictKind::Broken);
        assert!(!e.declared, "the app guessed, and says so");
    }

    #[test]
    fn writing_without_a_promise_is_inferred_kept() {
        let i = ins(SAT, "wrote something");
        let e = effective(None, None, Some(&i)).unwrap();
        assert_eq!(e.kind, VerdictKind::Kept);
        assert!(!e.declared);
    }

    #[test]
    fn a_block_that_promised_and_wrote_nothing_is_not_judged() {
        // Neither half exists: there is no intention to have failed.
        assert!(effective(None, None, None).is_none());
    }

    // ── tallying ─────────────────────────────────────────────────────────

    #[test]
    fn a_tally_counts_each_kind_separately() {
        let decrees = vec![dec(SAT, "a"), dec(SAT + 100, "b"), dec(SAT + 200, "c")];
        let verdicts = vec![
            v(SAT, VerdictKind::Kept, SAT),
            v(SAT + 100, VerdictKind::Partial, SAT),
            v(SAT + 200, VerdictKind::Short, SAT),
        ];
        let t = tally(&decrees, &[], &verdicts, today());
        assert_eq!(t.kept, 1);
        assert_eq!(t.partial, 1);
        assert_eq!(t.short, 1);
        assert_eq!(t.total(), 3);
    }

    #[test]
    fn a_tally_says_how_much_of_it_the_user_actually_judged() {
        let decrees = vec![dec(SAT, "a"), dec(SAT + 100, "b")];
        let inscriptions = vec![ins(SAT + 100, "did it")];
        let verdicts = vec![v(SAT, VerdictKind::Broken, SAT)];
        let t = tally(&decrees, &inscriptions, &verdicts, today());
        assert_eq!(t.declared, 1, "one block was judged");
        assert_eq!(t.inferred, 1, "one was inferred");
        assert_eq!(t.declared + t.inferred, t.total());
    }

    #[test]
    fn the_tally_stops_at_the_span_it_was_given() {
        let decrees = vec![dec(SAT, "in range"), dec(SAT + 3 * DAY, "out of range")];
        let verdicts = vec![
            v(SAT, VerdictKind::Kept, SAT),
            v(SAT + 3 * DAY, VerdictKind::Kept, SAT),
        ];
        let t = tally(&decrees, &[], &verdicts, today());
        assert_eq!(t.total(), 1, "only today's block is counted");
    }

    #[test]
    fn extended_blocks_count_as_met_and_short_ones_do_not() {
        let decrees = vec![dec(SAT, "a"), dec(SAT + 100, "b"), dec(SAT + 200, "c")];
        let verdicts = vec![
            v(SAT, VerdictKind::Extended, SAT),
            v(SAT + 100, VerdictKind::Short, SAT),
            v(SAT + 200, VerdictKind::Kept, SAT),
        ];
        let t = tally(&decrees, &[], &verdicts, today());
        assert_eq!(t.met(), 2, "kept and extended met what they were for");
        assert_eq!(t.total(), 3);
    }

    #[test]
    fn a_withdrawn_verdict_falls_back_to_the_shape_it_was_guessed_from() {
        let decrees = vec![dec(SAT, "write the report")];
        let inscriptions = vec![ins(SAT, "wrote it")];
        let verdicts = vec![Verdict {
            session_at: SAT,
            kind: VerdictKind::Broken,
            declared_at: SAT,
            deleted: true,
        }];
        let t = tally(&decrees, &inscriptions, &verdicts, today());
        assert_eq!(
            t.kept, 1,
            "with the word taken back, the shape decides again"
        );
        assert_eq!(t.inferred, 1);
    }

    #[test]
    fn a_block_the_user_judged_with_no_decree_or_inscription_is_still_counted() {
        // The user can judge a block that promised nothing and wrote nothing;
        // their word is the only record of it, and dropping it would punish
        // exactly the user who took the trouble to say.
        let verdicts = vec![v(SAT, VerdictKind::Short, SAT)];
        let t = tally(&[], &[], &verdicts, today());
        assert_eq!(t.short, 1);
        assert_eq!(t.declared, 1);
    }

    #[test]
    fn every_kind_has_its_own_storage_word() {
        // `as_str` is what lands in data.json and what the UI sends back. If two
        // kinds ever shared a word, a declared verdict would come back as the
        // wrong one and nothing would fail loudly.
        let mut words: Vec<&str> = VerdictKind::ALL.iter().map(|k| k.as_str()).collect();
        let before = words.len();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), before, "two kinds share a storage word");
    }

    #[test]
    fn only_a_kept_or_overrun_block_counts_as_having_met_its_promise() {
        // The ranking `met()` depends on, asserted directly so a change to
        // `is_met` cannot quietly reclassify a week.
        assert!(VerdictKind::Kept.is_met());
        assert!(VerdictKind::Extended.is_met());
        assert!(!VerdictKind::Partial.is_met());
        assert!(!VerdictKind::Broken.is_met());
        assert!(!VerdictKind::Short.is_met());
    }

    #[test]
    fn an_empty_span_tallies_to_nothing() {
        let t = tally(&[], &[], &[], today());
        assert!(t.is_empty());
        assert_eq!(t.met(), 0);
    }

    #[test]
    fn judged_rows_come_back_newest_first() {
        let decrees = vec![dec(SAT, "early"), dec(SAT + 600, "late")];
        let rows = judged(&decrees, &[], &[], today());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].session_at, SAT + 600);
    }
}
