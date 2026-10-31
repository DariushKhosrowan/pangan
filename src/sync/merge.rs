//! Record-level merge for cloud sync.
//!
//! Two devices editing the same data must not overwrite each other. A whole
//! file that "last writer wins" loses whichever device saved second: its
//! edits are replaced by the other one's snapshot, and the user sees work
//! disappear. So the merge is per record, not per file:
//!
//! * **Sessions** are append-only facts (a completed focus block happened).
//!   They are merged as a set keyed by `(at, minutes)`, so a session recorded
//!   on either device survives, and a session present on both is not doubled.
//! * **Tasks** are mutable, so each carries `updated_at` and the newer copy
//!   wins. Deletions are tombstones (`deleted: true`) rather than absences,
//!   because "missing here" cannot be told apart from "never seen here" —
//!   without a tombstone a delete on one device would be resurrected by the
//!   other device's copy on the next sync.
//!
//! * **Inscriptions and decrees** are append-only writings about a session —
//!   what was promised before the block and what was produced after it. They
//!   merge as sets keyed by a content-derived id, with tombstones for discards.
//!   Their order is sorted on the way out, so two devices holding the same
//!   writings produce byte-identical stores and stop exchanging deltas.
//!
//! The merge is commutative and idempotent: merging A with B gives the same
//! result as B with A, and merging a result with itself changes nothing. That
//! is what makes it safe to run on every sync tick without drift.

use crate::journal::{Decree, Inscription, Verdict};
use crate::state::plans::Plan;
use crate::state::rota::RotaEntry;
use crate::state::store::{Data, SessionRecord, Task};
use std::collections::BTreeMap;

/// Merge two stores into one.
///
/// Every collection is a union keyed by a stable id, so:
///
/// * the result contains everything either side had;
/// * merging the result with itself changes nothing — the property that makes
///   it safe to run on every sync tick without drift;
/// * a tombstone (deleted row) always beats a live one, because a deletion that
///   loses to a stale copy is a deletion that silently comes back.
///
/// The ordering of the output is sorted rather than preserved, so two devices
/// that hold the same set produce byte-identical stores and a later sync sees
/// no difference to push.
pub fn merge(local: &Data, remote: &Data) -> Data {
    let mut out = Data {
        schema_version: local.schema_version.max(remote.schema_version),
        sessions: merge_sessions(&local.sessions, &remote.sessions),
        tasks: merge_tasks(&local.tasks, &remote.tasks),
        next_task_id: local.next_task_id.max(remote.next_task_id),
        inscriptions: merge_inscriptions(&local.inscriptions, &remote.inscriptions),
        decrees: merge_decrees(&local.decrees, &remote.decrees),
        verdicts: merge_verdicts(&local.verdicts, &remote.verdicts),
        obstructions: merge_obstructions(&local.obstructions, &remote.obstructions),
        rota: merge_rota(&local.rota, &remote.rota),
        next_rota_id: local.next_rota_id.max(remote.next_rota_id),
        plans: merge_plans(&local.plans, &remote.plans),
        next_plan_id: local.next_plan_id.max(remote.next_plan_id),
        breaks: crate::timer::break_record::merge(&local.breaks, &remote.breaks),
        prescriptions: crate::journal::prescription_ledger::merge(
            &local.prescriptions,
            &remote.prescriptions,
        ),
        next_prescription_id: local.next_prescription_id.max(remote.next_prescription_id),
        prescription_verdicts: crate::journal::prescription_verdict::merge(
            &local.prescription_verdicts,
            &remote.prescription_verdicts,
        ),
        next_prescription_verdict_id: local
            .next_prescription_verdict_id
            .max(remote.next_prescription_verdict_id),
    };
    // Keep the id counters ahead of every id actually in use, so a merge can
    // never hand out an id that already belongs to a row. This matters more for
    // the rota than for tasks: a stale counter would make the next planned block
    // collide with one already on the list, and `mark_opened` would then open
    // the wrong entry.
    let highest_task = out.tasks.iter().map(|t| t.id).max().unwrap_or(0);
    out.next_task_id = out.next_task_id.max(highest_task + 1);
    // The rota counter is "the last id handed out", not "the next id", so the
    // floor is the highest id itself. Using `highest + 1` here made an empty
    // rota's counter climb from 0 to 1 on every merge, which is a change to a
    // store that held nothing — enough to make the sync engine rewrite a device
    // whose data was already the merged result, every tick, forever.
    let highest_rota = out.rota.iter().map(|e| e.id).max().unwrap_or(0);
    out.next_rota_id = out.next_rota_id.max(highest_rota);
    // Plans carry `updated_at` and are mutable, so the counter means the same
    // thing the rota's does: the last id handed out. The same `+1` mistake
    // would make an empty plan list change on every merge.
    let highest_plan = out.plans.iter().map(|p| p.id).max().unwrap_or(0);
    out.next_plan_id = out.next_plan_id.max(highest_plan);
    // Prescriptions are handed out by a counter that means "the last id
    // handed out", exactly like the rota and plans counters. The floor is
    // therefore the highest id itself — using `highest + 1` would make an
    // empty ledger's counter climb on every merge, which is a change to a
    // store that held nothing.
    let highest_prescription = out.prescriptions.iter().map(|p| p.id).max().unwrap_or(0);
    out.next_prescription_id = out.next_prescription_id.max(highest_prescription);
    // Prescription verdicts are handed out by a counter that means "the last id
    // handed out", exactly like the prescriptions counter. Same floor rule: the
    // highest id itself, so an empty list never climbs on merge.
    let highest_verdict = out
        .prescription_verdicts
        .iter()
        .map(|v| v.id)
        .max()
        .unwrap_or(0);
    out.next_prescription_verdict_id = out.next_prescription_verdict_id.max(highest_verdict);
    out
}

/// Union of both session lists, de-duplicated and in chronological order.
fn merge_sessions(a: &[SessionRecord], b: &[SessionRecord]) -> Vec<SessionRecord> {
    // Keyed by (timestamp, minutes): the same completed block recorded on two
    // devices is one session, but two blocks finishing in the same second
    // with different durations are genuinely two.
    let mut map: BTreeMap<(i64, u64), SessionRecord> = BTreeMap::new();
    for s in a.iter().chain(b.iter()) {
        map.insert((s.at, s.minutes), s.clone());
    }
    map.into_values().collect()
}

/// Per-task merge: newest `updated_at` wins, ties broken deterministically.
fn merge_tasks(a: &[Task], b: &[Task]) -> Vec<Task> {
    let mut map: BTreeMap<u64, Task> = BTreeMap::new();
    for t in a.iter().chain(b.iter()) {
        match map.get(&t.id) {
            None => {
                map.insert(t.id, t.clone());
            }
            Some(existing) => {
                if newer(t, existing) {
                    map.insert(t.id, t.clone());
                }
            }
        }
    }
    map.into_values().collect()
}

/// Is `candidate` the copy to keep when both describe the same task?
///
/// `updated_at` decides. When the stamps are equal the two copies are almost
/// always identical; if they are not, a deletion wins over an edit, because
/// resurrecting a task the user deleted is more annoying than losing one edit
/// to a task they were already removing.
fn newer(candidate: &Task, existing: &Task) -> bool {
    if candidate.updated_at != existing.updated_at {
        return candidate.updated_at > existing.updated_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    // Same stamp, same deletion state: fall back to a stable ordering so both
    // devices converge on the same answer instead of keeping their own copy.
    candidate.title > existing.title
}

/// Union of both archives, keyed by id.
///
/// An inscription's id already encodes (session, text), so two devices that
/// recorded the same session produce the same id and the union cannot double
/// it. Deletion is a tombstone: `deleted: true` wins over the live copy,
/// otherwise a discard on the desktop would be undone by the laptop's copy on
/// the next sync — the same reasoning as tasks.
fn merge_inscriptions(a: &[Inscription], b: &[Inscription]) -> Vec<Inscription> {
    let mut map: BTreeMap<String, Inscription> = BTreeMap::new();
    for ins in a.iter().chain(b.iter()) {
        match map.get(&ins.id) {
            None => {
                map.insert(ins.id.clone(), ins.clone());
            }
            Some(existing) => {
                // A tombstone always wins. Between two live copies the text is
                // identical by construction (the id hashes it), so keeping the
                // earlier `written_at` is enough to make the result stable.
                if ins.deleted && !existing.deleted {
                    map.insert(ins.id.clone(), ins.clone());
                } else if !ins.deleted && !existing.deleted && ins.written_at < existing.written_at
                {
                    map.insert(ins.id.clone(), ins.clone());
                }
            }
        }
    }
    let mut out: Vec<Inscription> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.session_at
            .cmp(&y.session_at)
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Union of both decree sets, keyed by id — the same rule as inscriptions.
///
/// Kept as its own function rather than made generic over a "written record"
/// trait: the two types differ in their length limit and in *when* they are
/// written, and a shared abstraction over two fields would cost more in
/// indirection than the twenty lines it would save.
fn merge_decrees(a: &[Decree], b: &[Decree]) -> Vec<Decree> {
    let mut map: BTreeMap<String, Decree> = BTreeMap::new();
    for dec in a.iter().chain(b.iter()) {
        match map.get(&dec.id) {
            None => {
                map.insert(dec.id.clone(), dec.clone());
            }
            Some(existing) => {
                // A tombstone always wins; between two live copies the text is
                // identical by construction, so the earlier `written_at` gives a
                // deterministic answer on both devices.
                if dec.deleted && !existing.deleted {
                    map.insert(dec.id.clone(), dec.clone());
                } else if !dec.deleted && !existing.deleted && dec.written_at < existing.written_at
                {
                    map.insert(dec.id.clone(), dec.clone());
                }
            }
        }
    }
    let mut out: Vec<Decree> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.session_at
            .cmp(&y.session_at)
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Union of both rotas, keyed by id — a set, not a newest-wins merge.
///
/// A rota entry is mutable in one field only (`opened_at`, which only ever goes
/// from 0 to a timestamp), so the rule is the same as tasks': the copy that has
/// been *used* is the one to keep, because a plan started on one device is
/// started everywhere. Ties fall back to the planned day then the id, so two
/// devices holding the same set converge to the same order instead of
/// exchanging a delta on every sync.
fn merge_rota(a: &[RotaEntry], b: &[RotaEntry]) -> Vec<RotaEntry> {
    let mut map: BTreeMap<u64, RotaEntry> = BTreeMap::new();
    for e in a.iter().chain(b.iter()) {
        match map.get(&e.id) {
            None => {
                map.insert(e.id, e.clone());
            }
            Some(existing) => {
                if newer_rota(e, existing) {
                    map.insert(e.id, e.clone());
                }
            }
        }
    }
    let mut out: Vec<RotaEntry> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.day
            .cmp(&y.day)
            .then_with(|| x.created_at.cmp(&y.created_at))
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Per-plan merge: newest `updated_at` wins, a tombstone beats a live copy.
///
/// Same rule as tasks, for the same reason: a plan is a mutable named thing,
/// and a deletion that loses to a stale copy is a deletion that comes back. The
/// tie-break is the plan's name rather than its id, so two devices that edited
/// the same plan in the same second still converge on one answer instead of
/// keeping their own.
fn merge_plans(a: &[Plan], b: &[Plan]) -> Vec<Plan> {
    let mut map: BTreeMap<u64, Plan> = BTreeMap::new();
    for p in a.iter().chain(b.iter()) {
        match map.get(&p.id) {
            None => {
                map.insert(p.id, p.clone());
            }
            Some(existing) => {
                if newer_plan(p, existing) {
                    map.insert(p.id, p.clone());
                }
            }
        }
    }
    let mut out: Vec<Plan> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.updated_at
            .cmp(&y.updated_at)
            .then_with(|| x.name.cmp(&y.name))
            .then_with(|| x.id.cmp(&y.id))
    });
    out
}

/// Is `candidate` the copy of one plan to keep?
fn newer_plan(candidate: &Plan, existing: &Plan) -> bool {
    if candidate.updated_at != existing.updated_at {
        return candidate.updated_at > existing.updated_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    candidate.name > existing.name
}

/// Per-verdict merge: newest `declared_at` wins, a withdrawal beats an old word.
///
/// Keyed by `session_at` rather than by a row id, because one session holds at
/// most one verdict — that is the invariant [`crate::journal::verdict::put`]
/// enforces on the local path, and sync has to enforce the same one or two
/// devices would drift into holding two answers to one question.
///
/// The tie-break is the kind's rank, not its name: two devices that judged the
/// same block in the same second must converge on one answer, and any fixed
/// order does that. Rank is used so the more forgiving judgement survives a
/// true tie — a coin-flip between "I did it" and "I didn't" should not land on
/// the accusation.
fn merge_verdicts(a: &[Verdict], b: &[Verdict]) -> Vec<Verdict> {
    let mut map: BTreeMap<i64, Verdict> = BTreeMap::new();
    for v in a.iter().chain(b.iter()) {
        match map.get(&v.session_at) {
            None => {
                map.insert(v.session_at, v.clone());
            }
            Some(existing) => {
                if newer_verdict(v, existing) {
                    map.insert(v.session_at, v.clone());
                }
            }
        }
    }
    let mut out: Vec<Verdict> = map.into_values().collect();
    out.sort_by(|x, y| {
        x.session_at
            .cmp(&y.session_at)
            .then_with(|| x.declared_at.cmp(&y.declared_at))
    });
    out
}

/// Is `candidate` the copy of one verdict to keep?
fn newer_verdict(candidate: &Verdict, existing: &Verdict) -> bool {
    if candidate.declared_at != existing.declared_at {
        return candidate.declared_at > existing.declared_at;
    }
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    // Equal stamps and equal tombstones: pick deterministically. `Kept` sorts
    // before `Broken` in `rank`, so the gentler reading of an honest tie wins.
    candidate.kind.rank() < existing.kind.rank()
}

/// Merge two obstruction lists. One row per session; the newer `created_at`
/// wins, a tombstone beats a live copy, and an equal stamp picks the
/// lexicographically smaller text for determinism.
fn merge_obstructions(
    a: &[crate::journal::obstruction::Obstruction],
    b: &[crate::journal::obstruction::Obstruction],
) -> Vec<crate::journal::obstruction::Obstruction> {
    use crate::journal::obstruction::Obstruction;
    let mut map: BTreeMap<i64, Obstruction> = BTreeMap::new();
    for o in a.iter().chain(b.iter()) {
        match map.get(&o.session_at) {
            None => {
                map.insert(o.session_at, o.clone());
            }
            Some(existing) => {
                let keep = if o.created_at != existing.created_at {
                    o.created_at > existing.created_at
                } else if o.deleted != existing.deleted {
                    o.deleted
                } else {
                    o.text <= existing.text
                };
                if keep {
                    map.insert(o.session_at, o.clone());
                }
            }
        }
    }
    let mut out: Vec<Obstruction> = map.into_values().collect();
    out.sort_by_key(|o| o.session_at);
    out
}

/// Which of two copies of one rota entry to keep.
///
/// Precedence, in order:
///
/// 1. a tombstone wins — a removal that loses to a stale copy would come back;
/// 2. an entry with an outcome beats one without — the block was actually
///    carried out (or actually abandoned) somewhere, and that fact is newer
///    than "still waiting", which is only ever the state before it;
/// 3. a finished outcome beats an abandoned one. Both are final, but done is
///    the stronger fact: it is the one a session was recorded for, and letting
///    a stale `abandoned` win would erase a block that really ran;
/// 4. an opened entry beats a pending one — the block was actually started
///    somewhere, and putting it back in the queue would ask the user to start
///    work they have already begun;
/// 5. otherwise the later `opened_at`, then the label, so both devices land on
///    the same row rather than each keeping its own.
///
/// Rules 2 and 3 are not an afterthought: a field that is merged by falling
/// through to the label would let a device that never saw a session revert a
/// finished block to pending on every sync.
fn newer_rota(candidate: &RotaEntry, existing: &RotaEntry) -> bool {
    if candidate.deleted != existing.deleted {
        return candidate.deleted;
    }
    let c_out = crate::state::outcome::Outcome::parse(candidate.outcome.as_deref().unwrap_or(""));
    let e_out = crate::state::outcome::Outcome::parse(existing.outcome.as_deref().unwrap_or(""));
    match (c_out, e_out) {
        (Some(_), None) => return true,
        (None, Some(_)) => return false,
        (Some(c), Some(e)) if c != e => return c.rank() < e.rank(),
        _ => {}
    }
    if candidate.opened_at != existing.opened_at {
        return candidate.opened_at > existing.opened_at;
    }
    candidate.label > existing.label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, title: &str, updated_at: i64) -> Task {
        Task {
            id,
            title: title.into(),
            date: String::new(),
            time: String::new(),
            done: false,
            created_at: 0,
            updated_at,
            deleted: false,
        }
    }

    fn sess(at: i64, minutes: u64) -> SessionRecord {
        SessionRecord { at, minutes }
    }

    #[test]
    fn sessions_are_a_union_without_duplicates() {
        let a = Data {
            sessions: vec![sess(100, 25), sess(200, 25)],
            ..Default::default()
        };
        let b = Data {
            sessions: vec![sess(200, 25), sess(300, 25)],
            ..Default::default()
        };
        let m = merge(&a, &b);
        assert_eq!(m.sessions.len(), 3, "shared session must not be doubled");
        let ats: Vec<i64> = m.sessions.iter().map(|s| s.at).collect();
        assert_eq!(ats, vec![100, 200, 300], "must come out chronological");
    }

    #[test]
    fn same_second_different_duration_are_two_sessions() {
        let a = Data {
            sessions: vec![sess(100, 25)],
            ..Default::default()
        };
        let b = Data {
            sessions: vec![sess(100, 50)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).sessions.len(), 2);
    }

    #[test]
    fn newer_task_edit_wins() {
        let a = Data {
            tasks: vec![task(1, "old", 10)],
            ..Default::default()
        };
        let b = Data {
            tasks: vec![task(1, "new", 20)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).tasks[0].title, "new");
        assert_eq!(merge(&b, &a).tasks[0].title, "new", "must be commutative");
    }

    #[test]
    fn deletion_beats_an_older_edit_but_not_a_newer_one() {
        let mut deleted = task(1, "gone", 20);
        deleted.deleted = true;
        let a = Data {
            tasks: vec![task(1, "edited", 10)],
            ..Default::default()
        };
        let b = Data {
            tasks: vec![deleted.clone()],
            ..Default::default()
        };
        assert!(merge(&a, &b).tasks[0].deleted, "delete must travel");

        let mut revived = task(1, "edited later", 30);
        revived.deleted = false;
        let c = Data {
            tasks: vec![revived],
            ..Default::default()
        };
        assert!(
            !merge(&b, &c).tasks[0].deleted,
            "a later edit resurrects the task"
        );
    }

    #[test]
    fn merge_is_idempotent() {
        let a = Data {
            schema_version: 1,
            sessions: vec![sess(1, 25)],
            tasks: vec![task(1, "x", 5)],
            next_task_id: 2,
            rota: vec![],
            next_rota_id: 0,
            plans: vec![],
            next_plan_id: 0,
            breaks: vec![],
            inscriptions: vec![],
            decrees: vec![],
            verdicts: vec![],
            obstructions: vec![],
            prescriptions: vec![],
            next_prescription_id: 0,
            prescription_verdicts: Vec::new(),
            next_prescription_verdict_id: 0,
        };
        let once = merge(&a, &a);
        let twice = merge(&once, &once);
        assert_eq!(once.sessions.len(), twice.sessions.len());
        assert_eq!(once.tasks.len(), twice.tasks.len());
        assert_eq!(once.tasks[0].title, twice.tasks[0].title);
    }

    #[test]
    fn next_task_id_stays_ahead_of_every_id_in_use() {
        let a = Data {
            tasks: vec![task(7, "x", 1)],
            next_task_id: 2, // stale counter, as an older file would have
            ..Default::default()
        };
        let b = Data::default();
        assert!(
            merge(&a, &b).next_task_id > 7,
            "a stale counter must not hand out a live id"
        );
    }

    // ── decrees ──────────────────────────────────────────────────────────

    fn decree(session_at: i64, text: &str) -> crate::journal::Decree {
        crate::journal::decree::create(session_at, session_at - 600, text).unwrap()
    }

    #[test]
    fn decrees_from_both_devices_both_survive() {
        let a = Data {
            decrees: vec![decree(1000, "from desktop")],
            ..Default::default()
        };
        let b = Data {
            decrees: vec![decree(2000, "from laptop")],
            ..Default::default()
        };
        let out = merge(&a, &b);
        let texts: Vec<&str> = out.decrees.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["from desktop", "from laptop"],
            "ordered by session"
        );
    }

    #[test]
    fn the_same_decree_on_both_devices_merges_to_one() {
        let d = decree(1000, "the same intention");
        let a = Data {
            decrees: vec![d.clone()],
            ..Default::default()
        };
        let b = Data {
            decrees: vec![d],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).decrees.len(), 1, "a content id cannot double");
    }

    #[test]
    fn a_discarded_decree_is_not_resurrected_by_the_other_device() {
        let live = decree(1000, "withdraw me");
        let mut dead = live.clone();
        dead.deleted = true;

        // The discard on one side, the stale live copy on the other — in both
        // orders, because the merge has to be commutative.
        let a = Data {
            decrees: vec![dead.clone()],
            ..Default::default()
        };
        let b = Data {
            decrees: vec![live.clone()],
            ..Default::default()
        };
        assert!(merge(&a, &b).decrees[0].deleted, "tombstone wins: a then b");
        assert!(merge(&b, &a).decrees[0].deleted, "tombstone wins: b then a");
    }

    #[test]
    fn merging_decrees_repeatedly_does_not_drift() {
        let a = Data {
            decrees: vec![decree(1000, "one"), decree(3000, "three")],
            ..Default::default()
        };
        let b = Data {
            decrees: vec![decree(2000, "two")],
            ..Default::default()
        };
        let once = merge(&a, &b);
        let twice = merge(&once, &b);
        let again = merge(&twice, &twice);
        assert_eq!(once.decrees.len(), 3);
        assert_eq!(twice.decrees, once.decrees, "merging again changes nothing");
        assert_eq!(again.decrees, once.decrees, "and is idempotent on itself");
    }

    // ── rota ─────────────────────────────────────────────────────────────

    fn planned(id: u64, day: &str, label: &str, created_at: i64) -> RotaEntry {
        crate::state::rota::create(id, day, label, None, "", 25, created_at, &[]).unwrap()
    }

    #[test]
    fn rotas_from_both_devices_both_survive() {
        let a = Data {
            rota: vec![planned(1, "2026-10-31", "from desktop", 10)],
            ..Default::default()
        };
        let b = Data {
            rota: vec![planned(2, "2026-10-31", "from laptop", 20)],
            ..Default::default()
        };
        let out = merge(&a, &b);
        let labels: Vec<&str> = out.rota.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, vec!["from desktop", "from laptop"], "planned order");
    }

    #[test]
    fn a_block_started_on_one_device_is_not_pending_on_the_other() {
        // The one mutable field on a rota entry. Losing this would put work the
        // user has already begun back at the top of the queue.
        let pending = planned(1, "2026-10-31", "write", 10);
        let mut opened = pending.clone();
        opened.opened_at = 500;

        let a = Data {
            rota: vec![opened],
            ..Default::default()
        };
        let b = Data {
            rota: vec![pending],
            ..Default::default()
        };
        assert_eq!(
            merge(&a, &b).rota[0].opened_at,
            500,
            "the started copy wins: a then b"
        );
        assert_eq!(
            merge(&b, &a).rota[0].opened_at,
            500,
            "and in the other direction"
        );
    }

    #[test]
    fn a_removed_rota_entry_is_not_resurrected() {
        let live = planned(1, "2026-10-31", "dropped", 10);
        let mut dead = live.clone();
        dead.deleted = true;
        let a = Data {
            rota: vec![dead],
            ..Default::default()
        };
        let b = Data {
            rota: vec![live],
            ..Default::default()
        };
        assert!(merge(&a, &b).rota[0].deleted, "tombstone wins: a then b");
        assert!(merge(&b, &a).rota[0].deleted, "tombstone wins: b then a");
    }

    #[test]
    fn the_rota_id_counter_stays_ahead_of_every_planned_block() {
        let a = Data {
            rota: vec![planned(9, "2026-10-31", "nine", 10)],
            next_rota_id: 2, // stale, as an older file would carry
            ..Default::default()
        };
        assert!(
            merge(&a, &Data::default()).next_rota_id >= 9,
            "a stale counter must not make a new block collide with a planned one"
        );
    }

    /// Merging must not change a store that holds nothing.
    ///
    /// The counter is the trap here: bumping an empty rota's counter on every
    /// merge rewrites `data.json` on every tick, so the sync engine reports a
    /// local change forever and pushes an unchanged file to Drive twice a second.
    #[test]
    fn merging_two_empty_rotas_is_a_no_op() {
        let empty = Data::default();
        let once = merge(&empty, &empty);
        assert_eq!(once.next_rota_id, empty.next_rota_id);
        let twice = merge(&once, &once);
        assert_eq!(twice.next_rota_id, once.next_rota_id);
    }

    #[test]
    fn merging_rotas_repeatedly_does_not_drift() {
        let a = Data {
            rota: vec![planned(1, "2026-10-31", "one", 10)],
            ..Default::default()
        };
        let b = Data {
            rota: vec![planned(2, "2026-10-31", "two", 20)],
            ..Default::default()
        };
        let once = merge(&a, &b);
        let twice = merge(&once, &b);
        assert_eq!(once.rota.len(), 2);
        assert_eq!(twice.rota, once.rota, "merging again changes nothing");
    }

    // ── plans ────────────────────────────────────────────────────────────

    fn plan(id: u64, name: &str, updated_at: i64) -> Plan {
        Plan {
            id,
            name: name.into(),
            blocks: vec![crate::state::plans::PlanBlock {
                label: "block".into(),
                minutes: 25,
                decree: String::new(),
            }],
            created_at: 0,
            updated_at,
            deleted: false,
        }
    }

    #[test]
    fn plans_from_both_devices_both_survive() {
        let a = Data {
            plans: vec![plan(1, "desktop shape", 10)],
            ..Default::default()
        };
        let b = Data {
            plans: vec![plan(2, "laptop shape", 20)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).plans.len(), 2);
    }

    #[test]
    fn the_newer_plan_edit_wins_and_the_merge_is_commutative() {
        let a = Data {
            plans: vec![plan(1, "old name", 10)],
            ..Default::default()
        };
        let b = Data {
            plans: vec![plan(1, "new name", 20)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).plans[0].name, "new name");
        assert_eq!(merge(&b, &a).plans[0].name, "new name");
    }

    #[test]
    fn a_deleted_plan_is_not_resurrected_by_the_other_device() {
        let mut dead = plan(1, "gone", 20);
        dead.deleted = true;
        let a = Data {
            plans: vec![plan(1, "gone", 10)],
            ..Default::default()
        };
        let b = Data {
            plans: vec![dead],
            ..Default::default()
        };
        assert!(merge(&a, &b).plans[0].deleted, "a delete must travel");
        assert!(merge(&b, &a).plans[0].deleted);
    }

    #[test]
    fn merging_two_empty_plan_lists_is_a_no_op() {
        // The counter rule that broke the sync engine for the rota applies
        // here too: an empty list must not raise its own counter, or a device
        // holding nothing rewrites and re-uploads on every tick.
        let empty = Data::default();
        let out = merge(&empty, &empty);
        assert_eq!(out.next_plan_id, empty.next_plan_id);
        // `Data` has no `PartialEq`, so compare the shape that matters here:
        // the counters and collection lengths both sides must agree on.
        let again = merge(&out, &out);
        assert_eq!(again.next_plan_id, out.next_plan_id);
        assert_eq!(again.plans.len(), out.plans.len());
    }

    #[test]
    fn the_plan_counter_stays_ahead_of_every_id_in_use() {
        let a = Data {
            plans: vec![plan(9, "x", 1)],
            next_plan_id: 2, // stale counter from an older file
            ..Default::default()
        };
        assert!(merge(&a, &Data::default()).next_plan_id >= 9);
    }

    // ── outcomes ─────────────────────────────────────────────────────────

    fn finished(id: u64, day: &str, outcome: &str, session_at: i64) -> RotaEntry {
        let mut e = planned(id, day, "write the thing", 100);
        e.opened_at = 5_000;
        e.outcome = Some(outcome.to_string());
        e.session_at = session_at;
        e
    }

    #[test]
    fn a_finished_block_is_not_reverted_by_a_device_that_never_saw_the_session() {
        // The failure this rules out: the laptop syncs before the session lands,
        // then the desktop merges the laptop's copy back in. If the outcome were
        // merged by falling through to the label, the block would revert to
        // pending and the day would forget work that actually happened.
        let done = finished(1, "2026-10-02", "done", 9_000);
        let stale = {
            let mut e = planned(1, "2026-10-02", "write the thing", 100);
            e.opened_at = 5_000;
            e
        };
        let a = Data {
            rota: vec![done],
            ..Default::default()
        };
        let b = Data {
            rota: vec![stale],
            ..Default::default()
        };
        let merged = merge(&a, &b);
        assert_eq!(merged.rota[0].outcome.as_deref(), Some("done"));
        assert_eq!(merged.rota[0].session_at, 9_000);
        // And the other direction agrees, so the two devices converge.
        let merged = merge(&b, &a);
        assert_eq!(merged.rota[0].outcome.as_deref(), Some("done"));
    }

    #[test]
    fn a_done_block_beats_a_stale_abandoned_one() {
        // A session that landed is the stronger fact. A device that marked the
        // block abandoned before the session reached it must not erase it.
        let done = finished(1, "2026-10-02", "done", 9_000);
        let abandoned = finished(1, "2026-10-02", "abandoned", 0);
        let a = Data {
            rota: vec![done],
            ..Default::default()
        };
        let b = Data {
            rota: vec![abandoned],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).rota[0].outcome.as_deref(), Some("done"));
        assert_eq!(merge(&b, &a).rota[0].outcome.as_deref(), Some("done"));
    }

    #[test]
    fn an_abandoned_block_still_beats_a_pending_one() {
        let abandoned = finished(1, "2026-10-02", "abandoned", 0);
        let pending = planned(1, "2026-10-02", "write the thing", 100);
        let a = Data {
            rota: vec![abandoned],
            ..Default::default()
        };
        let b = Data {
            rota: vec![pending],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).rota[0].outcome.as_deref(), Some("abandoned"));
    }

    #[test]
    fn a_removed_plan_takes_its_outcome_with_it() {
        // A tombstone must still beat a finished outcome, or deleting a plan
        // would be undone by the device that had it done.
        let mut dead = finished(1, "2026-10-02", "done", 9_000);
        dead.deleted = true;
        let a = Data {
            rota: vec![finished(1, "2026-10-02", "done", 9_000)],
            ..Default::default()
        };
        let b = Data {
            rota: vec![dead],
            ..Default::default()
        };
        assert!(merge(&a, &b).rota[0].deleted);
        assert!(merge(&b, &a).rota[0].deleted);
    }

    // ── verdicts ─────────────────────────────────────────────────────────

    fn verdict(session_at: i64, kind: crate::journal::VerdictKind, at: i64) -> Verdict {
        Verdict {
            session_at,
            kind,
            declared_at: at,
            deleted: false,
        }
    }

    #[test]
    fn verdicts_from_both_devices_both_survive() {
        use crate::journal::VerdictKind;
        let a = Data {
            verdicts: vec![verdict(1, VerdictKind::Kept, 10)],
            ..Default::default()
        };
        let b = Data {
            verdicts: vec![verdict(2, VerdictKind::Partial, 20)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).verdicts.len(), 2);
    }

    #[test]
    fn the_later_word_on_one_block_wins_and_the_merge_is_commutative() {
        use crate::journal::VerdictKind;
        let a = Data {
            verdicts: vec![verdict(1, VerdictKind::Broken, 10)],
            ..Default::default()
        };
        let b = Data {
            verdicts: vec![verdict(1, VerdictKind::Kept, 20)],
            ..Default::default()
        };
        assert_eq!(merge(&a, &b).verdicts[0].kind, VerdictKind::Kept);
        assert_eq!(merge(&b, &a).verdicts[0].kind, VerdictKind::Kept);
    }

    #[test]
    fn one_block_never_ends_up_with_two_verdicts() {
        use crate::journal::VerdictKind;
        // Both devices judged the same block; the union must be one row, or the
        // tally would count one block twice.
        let a = Data {
            verdicts: vec![verdict(1, VerdictKind::Kept, 10)],
            ..Default::default()
        };
        let b = Data {
            verdicts: vec![verdict(1, VerdictKind::Short, 20)],
            ..Default::default()
        };
        let out = merge(&a, &b);
        assert_eq!(out.verdicts.len(), 1);
        assert_eq!(out.verdicts[0].kind, VerdictKind::Short, "the newer word");
    }

    #[test]
    fn a_withdrawn_verdict_is_not_resurrected_by_the_other_device() {
        use crate::journal::VerdictKind;
        let mut dead = verdict(1, VerdictKind::Kept, 20);
        dead.deleted = true;
        let a = Data {
            verdicts: vec![verdict(1, VerdictKind::Kept, 10)],
            ..Default::default()
        };
        let b = Data {
            verdicts: vec![dead],
            ..Default::default()
        };
        assert!(
            merge(&a, &b).verdicts[0].deleted,
            "a retraction must travel"
        );
        assert!(merge(&b, &a).verdicts[0].deleted);
    }

    #[test]
    fn an_honest_tie_on_one_block_converges_instead_of_keeping_both() {
        use crate::journal::VerdictKind;
        // Same session, same second, different words. Any fixed order converges;
        // the one chosen should not land on the accusation.
        let a = Data {
            verdicts: vec![verdict(1, VerdictKind::Kept, 50)],
            ..Default::default()
        };
        let b = Data {
            verdicts: vec![verdict(1, VerdictKind::Broken, 50)],
            ..Default::default()
        };
        let ab = merge(&a, &b);
        let ba = merge(&b, &a);
        assert_eq!(ab.verdicts.len(), 1);
        assert_eq!(ab.verdicts, ba.verdicts, "both orders reach the same store");
        assert_eq!(ab.verdicts[0].kind, VerdictKind::Kept);
    }

    #[test]
    fn merging_two_empty_verdict_lists_changes_nothing() {
        let empty = Data::default();
        let out = merge(&empty, &empty);
        assert!(out.verdicts.is_empty());
        assert_eq!(merge(&out, &out).verdicts.len(), 0);
    }
}

#[test]
fn merge_breaks_unions_by_start_and_kind() {
    use crate::timer::break_record::{BreakRecord, BreakRel};
    let mk = |started, kind| BreakRecord {
        started_at: started,
        ended_at: Some(started + 300),
        planned_secs: 300,
        kind,
    };
    let a = Data {
        breaks: vec![mk(1000, BreakRel::Short), mk(5000, BreakRel::Short)],
        ..Default::default()
    };
    let b = Data {
        breaks: vec![mk(1000, BreakRel::Short), mk(7000, BreakRel::Long)],
        ..Default::default()
    };
    let merged = merge(&a, &b);
    assert_eq!(merged.breaks.len(), 3, "dedupe by key; keep distinct");
}

#[test]
fn merge_breaks_complete_beats_open() {
    use crate::timer::break_record::{begin, BreakRel};
    let a = Data {
        breaks: vec![begin(1000, 300, BreakRel::Short)],
        ..Default::default()
    };
    let b = Data {
        breaks: vec![crate::timer::break_record::close(
            begin(1000, 300, BreakRel::Short),
            1300,
        )],
        ..Default::default()
    };
    let merged = merge(&a, &b);
    assert_eq!(merged.breaks.len(), 1);
    assert_eq!(merged.breaks[0].ended_at, Some(1300));
}
