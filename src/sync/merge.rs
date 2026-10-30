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

use crate::journal::{Decree, Inscription};
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
        sessions: merge_sessions(&local.sessions, &remote.sessions),
        tasks: merge_tasks(&local.tasks, &remote.tasks),
        next_task_id: local.next_task_id.max(remote.next_task_id),
        inscriptions: merge_inscriptions(&local.inscriptions, &remote.inscriptions),
        decrees: merge_decrees(&local.decrees, &remote.decrees),
    };
    // Keep the id counter ahead of every id actually in use, so a merge can
    // never hand out an id that already belongs to a task.
    let highest = out.tasks.iter().map(|t| t.id).max().unwrap_or(0);
    out.next_task_id = out.next_task_id.max(highest + 1);
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
            sessions: vec![sess(1, 25)],
            tasks: vec![task(1, "x", 5)],
            next_task_id: 2,
            inscriptions: vec![],
            decrees: vec![],
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
}
