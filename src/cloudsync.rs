//! Automatic cloud sync: pull, merge, push, on a timer.
//!
//! The engine is deliberately small. Every sync is the same three steps:
//!
//!   1. download the remote snapshot (if any),
//!   2. merge it with the local one record by record (see `merge`),
//!   3. if the merge changed anything, write it back and save locally.
//!
//! Because the merge is commutative and idempotent, running it often is free
//! of consequences: two devices that sync in either order end up with the same
//! data, and a device that syncs twice in a row does not change anything the
//! second time. That is what makes "sync every time something changes, and
//! also every minute" safe rather than a source of drift.
//!
//! Conflict handling is not a dialog. The user is not asked to pick a winner,
//! because the merge already knows: sessions are unioned, tasks are decided by
//! `updated_at`, deletions travel as tombstones. There is nothing left for the
//! user to adjudicate, which is the point — a sync that asks questions is a
//! sync people turn off.

use crate::gdrive::{self, Tokens};
use crate::merge;
use crate::store::Data;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What happened on the last sync, for the status line.
#[derive(Clone, Debug, Default)]
pub struct SyncState {
    /// Human-readable status, already localised by the caller.
    pub status: String,
    /// Unix seconds of the last successful exchange.
    pub last_ok: i64,
    /// True while a sync is in flight.
    pub busy: bool,
    /// Set once the user has signed in and a token is stored.
    pub connected: bool,
    /// The signed-in account, when known.
    pub email: String,
}

/// Shared sync engine, owned by the app and driven from a timer.
pub struct Engine {
    pub state: Mutex<SyncState>,
    /// Set when the user turns automatic sync off.
    pub enabled: AtomicBool,
    /// Set whenever local data changes, so the timer knows to push.
    pub dirty: AtomicBool,
    /// How many cycles in a row have failed. The timer backs off on this so a
    /// dead network does not mean a request every tick.
    pub consecutive_failures: AtomicUsize,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SyncState::default()),
            enabled: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            consecutive_failures: AtomicUsize::new(0),
        }
    }

    /// Mark local data as changed. The next tick will push it.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Clear the dirty flag, returning whether it had been set. Doing this in
    /// one step means two threads cannot both decide they have work to push.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    pub fn set_status(&self, s: impl Into<String>) {
        self.state.lock().unwrap().status = s.into();
    }

    pub fn status(&self) -> String {
        self.state.lock().unwrap().status.clone()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// One full sync cycle. Returns the merged data when it differs from `local`.
///
/// `save` is called with the merged result so the caller owns persistence;
/// this keeps the engine free of file paths and easy to test.
pub fn sync_once(
    tokens: &mut Tokens,
    local: &Data,
    save: &mut dyn FnMut(&Data),
) -> Result<Outcome, String> {
    let token = gdrive::access_token(tokens)?;

    let remote_text = gdrive::download(&token)?;
    let remote: Data = match remote_text {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| format!("remote file is not valid sync data: {e}"))?,
        _ => Data::default(),
    };

    let merged = merge::merge(local, &remote);

    // Push only when the merge produced something the remote does not already
    // have. Comparing the serialised form is the cheap, exact test: if the
    // remote already holds exactly this, uploading again is pure waste.
    let merged_json = serde_json::to_string(&merged).map_err(|e| format!("encode: {e}"))?;
    let remote_json = serde_json::to_string(&remote).unwrap_or_default();
    let pushed = merged_json != remote_json;
    if pushed {
        gdrive::upload(&token, &merged_json)?;
    }

    // Save locally when the merge changed our own copy.
    let local_json = serde_json::to_string(local).unwrap_or_default();
    let changed_locally = merged_json != local_json;
    if changed_locally {
        save(&merged);
    }

    Ok(Outcome {
        pulled: remote_json != local_json,
        pushed,
        changed_locally,
    })
}

/// What a sync cycle did, so the UI can say something true.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outcome {
    pub pulled: bool,
    pub pushed: bool,
    pub changed_locally: bool,
}

impl Outcome {
    /// Nothing moved in either direction.
    pub fn quiet(&self) -> bool {
        !self.pulled && !self.pushed
    }
}

/// Run `sync_once` on a background thread and report through `engine`.
///
/// Network calls must never block the UI thread: a slow Drive request would
/// freeze the timer, which is the one thing this app must not do.
pub fn spawn_sync(
    engine: Arc<Engine>,
    tokens: Arc<Mutex<Tokens>>,
    local: Data,
    save: Arc<Mutex<dyn FnMut(&Data) + Send>>,
    on_done: Arc<dyn Fn(Result<Outcome, String>) + Send + Sync>,
) {
    {
        let mut st = engine.state.lock().unwrap();
        if st.busy {
            return; // a cycle is already running; let it finish
        }
        st.busy = true;
    }
    std::thread::spawn(move || {
        let result = {
            let mut t = tokens.lock().unwrap();
            let mut save_guard = save.lock().unwrap();
            let saver: &mut dyn FnMut(&Data) = &mut *save_guard;
            sync_once(&mut t, &local, saver)
        };
        {
            let mut st = engine.state.lock().unwrap();
            st.busy = false;
            if result.is_ok() {
                st.last_ok = chrono::Utc::now().timestamp();
            }
        }
        on_done(result);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SessionRecord, Task};

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

    /// The engine's contract, without the network: merging twice is stable.
    #[test]
    fn a_second_pass_over_the_same_data_is_quiet() {
        let a = Data {
            sessions: vec![SessionRecord { at: 1, minutes: 25 }],
            tasks: vec![task(1, "write", 10)],
            next_task_id: 2,
        };
        let b = Data {
            sessions: vec![SessionRecord { at: 2, minutes: 25 }],
            tasks: vec![task(2, "read", 20)],
            next_task_id: 3,
        };
        let first = merge::merge(&a, &b);
        let second = merge::merge(&first, &b);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap(),
            "re-merging the same remote must not keep changing the result"
        );
    }

    #[test]
    fn outcome_quiet_only_when_nothing_moved() {
        assert!(Outcome::default().quiet());
        assert!(!Outcome {
            pushed: true,
            ..Default::default()
        }
        .quiet());
        assert!(!Outcome {
            pulled: true,
            ..Default::default()
        }
        .quiet());
    }

    #[test]
    fn engine_starts_disabled_and_clean() {
        let e = Engine::new();
        assert!(!e.enabled.load(Ordering::Relaxed));
        assert!(!e.is_dirty());
        e.mark_dirty();
        assert!(e.is_dirty());
    }

    /// A real round trip through the engine against a fake Drive: device A
    /// pushes, device B pulls and ends up with A's data, and a third pass is
    /// quiet. This is the behaviour the whole feature exists for.
    ///
    /// Note the `save` contract: the engine only calls it when the merge
    /// changed the local copy. A device whose data is already the merged
    /// result is left alone, which is why A's own snapshot stays authoritative
    /// for A after its first push.
    #[test]
    fn two_devices_converge_through_the_engine() {
        let _guard = crate::gdrive::drive_roundtrip::DRIVE_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (base, _store) = crate::gdrive::drive_roundtrip::fake_drive_for_tests();
        std::env::set_var("PANGAN_DRIVE_BASE", &base);

        let mut tokens = Tokens {
            access_token: "tok".into(),
            refresh_token: "refresh".into(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            email: "a@example.com".into(),
        };

        // Device A: one session and one task, nothing remote yet.
        let a = Data {
            sessions: vec![SessionRecord {
                at: 100,
                minutes: 25,
            }],
            tasks: vec![task(1, "write the report", 10)],
            next_task_id: 2,
        };
        let out_a = sync_once(&mut tokens, &a, &mut |_| {}).unwrap();
        assert!(out_a.pushed, "A must push when the remote is empty");
        assert!(
            !out_a.changed_locally,
            "A's own copy is already the merged result, so it must not be rewritten"
        );

        // Device B: starts empty and pulls. It must end up with A's data.
        let b = Data::default();
        let mut b_now = Data::default();
        let out_b = sync_once(&mut tokens, &b, &mut |m| b_now = m.clone()).unwrap();
        assert!(out_b.pulled, "B must pull A's data");
        assert!(out_b.changed_locally, "B's empty copy must be replaced");
        assert_eq!(b_now.sessions.len(), 1);
        assert_eq!(b_now.tasks.len(), 1);
        assert_eq!(b_now.tasks[0].title, "write the report");

        // B adds a task and pushes.
        let b2 = Data {
            tasks: vec![task(1, "write the report", 10), task(2, "review", 20)],
            next_task_id: 3,
            ..b_now.clone()
        };
        sync_once(&mut tokens, &b2, &mut |_| {}).unwrap();

        // A pulls and must see both tasks, with its own session intact.
        let mut a_final = Data::default();
        sync_once(&mut tokens, &a, &mut |m| a_final = m.clone()).unwrap();
        assert_eq!(a_final.tasks.len(), 2, "A must see B's task");
        assert_eq!(a_final.sessions.len(), 1, "A's session must survive");

        // A third pass changes nothing: the engine is quiet on stable data.
        let out_c = sync_once(&mut tokens, &a_final, &mut |_| {}).unwrap();
        assert!(out_c.quiet(), "a stable state must not push or pull");

        std::env::remove_var("PANGAN_DRIVE_BASE");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::store::{SessionRecord, Task};
    fn task(id: u64, title: &str, updated_at: i64) -> Task {
        Task {
            id,
            title: title.into(),
            date: "1405-08-07".into(),
            time: String::new(),
            done: false,
            created_at: 0,
            updated_at,
            deleted: false,
        }
    }

    /// These tests all point `PANGAN_DRIVE_BASE` at their own fake server, so
    /// they must not run at the same time or they would each see the other's
    /// Drive. The guard is released on drop.
    fn fake() -> (std::sync::MutexGuard<'static, ()>, String, Tokens) {
        let guard = crate::gdrive::drive_roundtrip::DRIVE_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (base, _store) = crate::gdrive::drive_roundtrip::fake_drive_for_tests();
        std::env::set_var("PANGAN_DRIVE_BASE", &base);
        let tokens = Tokens {
            access_token: "tok".into(),
            refresh_token: "refresh".into(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
            email: "a@example.com".into(),
        };
        (guard, base, tokens)
    }

    /// The user's requirement, stated plainly: if the local data is wiped, a
    /// sync must put it back from the cloud. This is the whole point of having
    /// a cloud copy, and it is the case that a naive "push local, pull remote"
    /// implementation gets wrong — an empty local file must not be treated as
    /// "the user deleted everything".
    #[test]
    fn a_wiped_local_store_is_restored_from_the_cloud() {
        let (_guard, base, mut tokens) = fake();

        // The good state reaches the cloud.
        let good = Data {
            sessions: vec![SessionRecord {
                at: 100,
                minutes: 25,
            }],
            tasks: vec![task(1, "write the report", 10), task(2, "review", 20)],
            next_task_id: 3,
        };
        sync_once(&mut tokens, &good, &mut |_| {}).unwrap();

        // The local file is wiped — a fresh install, a deleted config dir, a
        // corrupted file that failed to parse.
        let wiped = Data::default();
        let mut restored = Data::default();
        let out = sync_once(&mut tokens, &wiped, &mut |m| restored = m.clone()).unwrap();

        assert!(out.pulled, "an empty local store must pull the cloud copy");
        assert!(out.changed_locally, "the empty store must be rewritten");
        assert_eq!(restored.tasks.len(), 2, "tasks were not restored");
        assert_eq!(restored.sessions.len(), 1, "sessions were not restored");
        assert_eq!(restored.tasks[0].title, "write the report");
        // And the id counter must come back too, or the next new task would
        // collide with a restored one.
        assert!(restored.next_task_id >= 3);

        std::env::remove_var("PANGAN_DRIVE_BASE");
        let _ = base;
    }

    /// Restoring must not be a one-way door: after the pull, the next sync is
    /// quiet, so a wiped-and-restored device does not then push its emptiness
    /// back over the good copy.
    #[test]
    fn restoring_does_not_then_overwrite_the_cloud_with_emptiness() {
        let (_guard, base, mut tokens) = fake();

        let good = Data {
            tasks: vec![task(1, "keep me", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &good, &mut |_| {}).unwrap();

        let mut restored = Data::default();
        sync_once(&mut tokens, &Data::default(), &mut |m| restored = m.clone()).unwrap();
        assert_eq!(restored.tasks.len(), 1);

        // Now sync the restored copy. Nothing should move.
        let out = sync_once(&mut tokens, &restored, &mut |_| {}).unwrap();
        assert!(
            out.quiet(),
            "a restored device must not push again: {out:?}"
        );

        // And the cloud still holds the task.
        let mut check = Data::default();
        sync_once(&mut tokens, &Data::default(), &mut |m| check = m.clone()).unwrap();
        assert_eq!(check.tasks.len(), 1, "the cloud copy was lost");

        std::env::remove_var("PANGAN_DRIVE_BASE");
        let _ = base;
    }

    /// A local edit made while offline must survive a later pull: the merge is
    /// per record, so the cloud copy cannot clobber work the device did.
    #[test]
    fn an_offline_edit_survives_a_later_sync() {
        let (_guard, base, mut tokens) = fake();

        let cloud = Data {
            tasks: vec![task(1, "from cloud", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &cloud, &mut |_| {}).unwrap();

        // The device goes offline and adds its own task.
        let offline = Data {
            tasks: vec![task(1, "from cloud", 10), task(2, "made offline", 30)],
            next_task_id: 3,
            ..Default::default()
        };

        // `save` is only called when the merge changed the local copy, and
        // here the local copy is already the merged result — so the thing to
        // check is the cloud, which must now hold both tasks.
        let out = sync_once(&mut tokens, &offline, &mut |_| {}).unwrap();
        assert!(out.pushed, "the offline task must reach the cloud");

        let mut cloud_after = Data::default();
        sync_once(&mut tokens, &Data::default(), &mut |m| {
            cloud_after = m.clone()
        })
        .unwrap();
        assert_eq!(cloud_after.tasks.len(), 2, "the offline task must survive");
        let titles: Vec<&str> = cloud_after.tasks.iter().map(|t| t.title.as_str()).collect();
        assert!(titles.contains(&"made offline"));
        assert!(titles.contains(&"from cloud"));

        std::env::remove_var("PANGAN_DRIVE_BASE");
        let _ = base;
    }

    /// A deletion made on one device must not come back from the cloud.
    #[test]
    fn a_deletion_is_not_resurrected_by_a_sync() {
        let (_guard, base, mut tokens) = fake();

        let before = Data {
            tasks: vec![task(1, "delete me", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &before, &mut |_| {}).unwrap();

        // Delete it locally: the tombstone carries a newer stamp.
        let mut deleted = before.clone();
        deleted.tasks[0].deleted = true;
        deleted.tasks[0].updated_at = 50;
        sync_once(&mut tokens, &deleted, &mut |_| {}).unwrap();

        // A device that still has the old copy syncs; it must not bring the
        // task back for everyone.
        let mut after = Data::default();
        sync_once(&mut tokens, &before, &mut |m| after = m.clone()).unwrap();
        let live: Vec<&Task> = after.tasks.iter().filter(|t| !t.deleted).collect();
        assert!(
            live.is_empty(),
            "a deleted task was resurrected: {:?}",
            live.iter().map(|t| &t.title).collect::<Vec<_>>()
        );

        std::env::remove_var("PANGAN_DRIVE_BASE");
        let _ = base;
    }
}
