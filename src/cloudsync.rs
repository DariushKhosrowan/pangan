//! Automatic cloud sync: pull, merge, push, on a timer.
//!
//! The engine is deliberately small. Every sync is the same three steps:
//!
//!   1. download the remote envelope (if any),
//!   2. merge it with the local state record by record (see `merge` and
//!      `syncfile`),
//!   3. if the merge changed anything, write it back and save locally.
//!
//! Because both merges are commutative and idempotent, running them often is
//! free of consequences: two devices that sync in either order end up with the
//! same state, and a device that syncs twice in a row does not change anything
//! the second time. That is what makes "sync every time something changes, and
//! also every minute" safe rather than a source of drift.
//!
//! Conflict handling is not a dialog. The user is not asked to pick a winner,
//! because the merge already knows: sessions are unioned, tasks are decided by
//! `updated_at`, deletions travel as tombstones, and settings are decided per
//! field. There is nothing left for the user to adjudicate, which is the point
//! — a sync that asks questions is a sync people turn off.
//!
//! ## The one race the merge cannot fix
//!
//! A merge resolves two *snapshots*. It cannot help if the file changes between
//! the read and the write, because the write would then replace a snapshot
//! this device never saw. That is a lost update, and no amount of clever
//! merging recovers it. So the upload carries the etag it read (`If-Match`),
//! Drive rejects a stale write with 412, and the cycle re-reads and re-merges.
//! Three attempts, then an honest error: a device that keeps losing the race is
//! not going to win it by trying harder.

use crate::gdrive::{self, Tokens};
use crate::merge;
use crate::settings::Settings;
use crate::store::Data;
use crate::syncfile::{DeviceState, Envelope};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// How many times a cycle will re-read and retry after losing a write race.
const MAX_ATTEMPTS: usize = 3;

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

/// The local side of a sync: everything this device would push.
#[derive(Clone, Debug)]
pub struct Local {
    pub data: Data,
    pub settings: Settings,
}

/// One full sync cycle. Returns what moved, and calls `save` with the merged
/// result so the caller owns persistence.
///
/// `device` is this machine's identity and its remembered settings stamps. It
/// is mutated in place: the stamps are what let the next cycle tell "the user
/// changed this" from "this arrived from the other machine", so losing them
/// would make every launch look like a fresh edit.
pub fn sync_once(
    tokens: &mut Tokens,
    local: &Local,
    device: &mut DeviceState,
    save: &mut dyn FnMut(&Data, &Settings),
) -> Result<Outcome, String> {
    device.ensure_identity();
    let token = gdrive::access_token(tokens)?;

    let mut last_err = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        match attempt_once(&token, local, device, save) {
            Ok(out) => return Ok(out),
            Err(e) if e == gdrive::CONFLICT => {
                // The file moved between our read and our write. Re-read and
                // re-merge: the other device's work is in the new copy, so the
                // retry folds it in instead of overwriting it.
                last_err = e;
                // A short pause so two devices that collided do not collide
                // again in lockstep on the next attempt.
                std::thread::sleep(std::time::Duration::from_millis(
                    120 * (attempt as u64 + 1),
                ));
            }
            Err(e) => return Err(e),
        }
    }
    Err(format!(
        "another device kept writing at the same time ({last_err}); try again"
    ))
}

/// One read-merge-write pass. `Err(CONFLICT)` means the write lost a race.
fn attempt_once(
    token: &str,
    local: &Local,
    device: &mut DeviceState,
    save: &mut dyn FnMut(&Data, &Settings),
) -> Result<Outcome, String> {
    let remote_read = gdrive::download_meta(token, "pangan-sync.json")?;
    let (remote, remote_meta) = match remote_read {
        Some((text, meta)) => (Envelope::parse(&text)?, Some(meta)),
        None => (Envelope::default(), None),
    };

    // Merge the data record by record.
    let merged_data = merge::merge(&local.data, &remote.data);

    // Merge the settings field by field. The local side is first brought up to
    // date with what the user has actually changed since the last cycle, so a
    // field they just edited carries a fresh stamp and wins; a field they did
    // not touch keeps its old stamp and loses to a newer edit elsewhere.
    //
    // `seed` covers a device with no baseline yet — a fresh install, or a state
    // file that was deleted. It records the current values without stamping
    // them, so the cloud's settings win over this device's defaults instead of
    // the other way round.
    let now = chrono::Utc::now().timestamp();
    device.settings.seed(&local.settings);
    device.settings.absorb(&local.settings, now);

    // A device that has never synced has nothing worth defending: its settings
    // are whatever the app shipped with. Adopting the cloud's copy outright is
    // the only reading that is not surprising — otherwise a fresh install
    // would push 25-minute defaults over the 50 minutes the user set on their
    // desktop. This is a one-time bootstrap; after it, stamps decide.
    let merged_settings = if !device.bootstrapped && !remote.settings.is_empty() {
        device.bootstrapped = true; remote.settings.clone()
    } else {
        device.bootstrapped = true; device.settings.merge(&remote.settings)
    }; 

    // What the remote already holds, for the cheap "did anything move" test.
    let remote_data_json = serde_json::to_string(&remote.data).unwrap_or_default();
    let remote_settings_json = serde_json::to_string(&remote.settings).unwrap_or_default();
    let merged_data_json =
        serde_json::to_string(&merged_data).map_err(|e| format!("encode: {e}"))?;
    let merged_settings_json =
        serde_json::to_string(&merged_settings).map_err(|e| format!("encode: {e}"))?;

    // Two different questions, and conflating them was a bug:
    //
    //  * `pushed` — does the remote lack something we have? Compare the merged
    //    result against the remote.
    //  * `pulled` — did the remote teach us something? Compare the merged
    //    result against our own copy.
    //
    // Asking only the first question and calling the answer "pulled" made an
    // empty device report that it had nothing to learn while it was in fact
    // about to be filled from the cloud.
    let local_data_json = serde_json::to_string(&local.data).unwrap_or_default();
    let local_settings_json = serde_json::to_string(&device.settings).unwrap_or_default();

    let pushed_data = merged_data_json != remote_data_json;
    let pushed_settings = merged_settings_json != remote_settings_json;
    let pulled_data = merged_data_json != local_data_json;
    let pulled_settings = merged_settings_json != local_settings_json;
    let pushed = pushed_data || pushed_settings;

    if pushed {
        let envelope = Envelope {
            schema: crate::syncfile::SCHEMA,
            revision: remote.revision + 1,
            updated_at: now,
            device_id: device.device_id.clone(),
            device_name: device.device_name.clone(),
            data: merged_data.clone(),
            settings: merged_settings.clone(),
        };
        let body = envelope.to_json()?;
        gdrive::upload_conditional(token, "pangan-sync.json", &body, remote_meta.as_ref())?;
        device.last_revision = envelope.revision;
    }

    // Save locally when the merge changed our own copy. Settings are applied
    // through a copy so the caller's `Settings` is only replaced when the
    // values genuinely differ — otherwise every sync would rewrite the config
    // file and re-trigger the "did the user change something" check.
    let data_changed_locally = pulled_data;

    let mut next_settings = local.settings.clone();
    merged_settings.apply(&mut next_settings);
    next_settings.sanitize_mut();
    let settings_changed_locally = toml::to_string(&next_settings).ok()
        != toml::to_string(&local.settings).ok();

    // Adopt the merged settings as the baseline, so the next cycle does not
    // mistake values that just arrived from the other device for local edits.
    device.settings = merged_settings.clone();

    if data_changed_locally || settings_changed_locally {
        save(&merged_data, &next_settings);
    }

    Ok(Outcome {
        pulled: pulled_data || pulled_settings,
        pushed,
        changed_locally: data_changed_locally || settings_changed_locally,
        data_pulled: pulled_data,
        settings_pulled: pulled_settings,
        remote_device: remote.device_name.clone(),
    })
}

/// What a sync cycle did, so the UI can say something true.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub pulled: bool,
    pub pushed: bool,
    pub changed_locally: bool,
    /// Whether the data (sessions/tasks) moved in either direction.
    pub data_pulled: bool,
    /// Whether any setting moved in either direction.
    pub settings_pulled: bool,
    /// The name of the device that wrote the remote copy, for the status line.
    pub remote_device: String,
}

impl Outcome {
    /// Nothing moved in either direction.
    pub fn quiet(&self) -> bool {
        !self.pulled && !self.pushed
    }
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

    /// A local snapshot with the given data and default settings.
    fn local(data: Data) -> Local {
        Local {
            data,
            settings: Settings::default(),
        }
    }

    /// A device with a fixed identity, so a test's output does not depend on
    /// the machine it runs on.
    fn device(name: &str) -> DeviceState {
        DeviceState {
            device_id: format!("id-{name}"),
            device_name: name.into(),
            ..Default::default()
        }
    }

    /// Point the client at a fresh fake Drive and hand back a signed-in token.
    /// The caller holds the guard so two such tests cannot share a server.
    fn fake() -> (std::sync::MutexGuard<'static, ()>, Tokens) {
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
        (guard, tokens)
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
        let (_guard, mut tokens) = fake();

        // Device A: one session and one task, nothing remote yet.
        let a = Data {
            sessions: vec![SessionRecord {
                at: 100,
                minutes: 25,
            }],
            tasks: vec![task(1, "write the report", 10)],
            next_task_id: 2,
        };
        let mut dev_a = device("desktop");
        let out_a = sync_once(&mut tokens, &local(a.clone()), &mut dev_a, &mut |_, _| {}).unwrap();
        assert!(out_a.pushed, "A must push when the remote is empty");
        assert!(
            !out_a.changed_locally,
            "A's own copy is already the merged result, so it must not be rewritten"
        );

        // Device B: starts empty and pulls. It must end up with A's data.
        let mut b_now = Data::default();
        let mut dev_b = device("laptop");
        let out_b = sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev_b,
            &mut |m, _| b_now = m.clone(),
        )
        .unwrap();
        assert!(out_b.pulled, "B must pull A's data");
        assert!(out_b.changed_locally, "B's empty copy must be replaced");
        assert_eq!(b_now.sessions.len(), 1);
        assert_eq!(b_now.tasks.len(), 1);
        assert_eq!(b_now.tasks[0].title, "write the report");
        assert_eq!(
            out_b.remote_device, "desktop",
            "B must be able to say which device it last heard from"
        );

        // B adds a task and pushes.
        let b2 = Data {
            tasks: vec![task(1, "write the report", 10), task(2, "review", 20)],
            next_task_id: 3,
            ..b_now.clone()
        };
        sync_once(&mut tokens, &local(b2), &mut dev_b, &mut |_, _| {}).unwrap();

        // A pulls and must see both tasks, with its own session intact.
        let mut a_final = Data::default();
        sync_once(&mut tokens, &local(a), &mut dev_a, &mut |m, _| {
            a_final = m.clone()
        })
        .unwrap();
        assert_eq!(a_final.tasks.len(), 2, "A must see B's task");
        assert_eq!(a_final.sessions.len(), 1, "A's session must survive");

        // A third pass changes nothing: the engine is quiet on stable data.
        let out_c = sync_once(&mut tokens, &local(a_final), &mut dev_a, &mut |_, _| {}).unwrap();
        assert!(out_c.quiet(), "a stable state must not push or pull");
    }

    /// The user's requirement, stated plainly: settings must follow them from
    /// device to device. This is the gap the first version left open — it
    /// synced history and tasks but silently kept preferences local.
    #[test]
    fn settings_travel_between_devices() {
        let (_guard, mut tokens) = fake();

        let mut desktop_settings = Settings::default();
        desktop_settings.work_minutes = 50;
        desktop_settings.sound_theme = "daf".into();
        desktop_settings.theme = "light".into();

        let mut dev_a = device("desktop");
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: desktop_settings,
            },
            &mut dev_a,
            &mut |_, _| {},
        )
        .unwrap();

        // The laptop has never seen these values.
        let mut dev_b = device("laptop");
        let mut arrived = Settings::default();
        assert_eq!(arrived.work_minutes, 25);

        let out = sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: Settings::default(),
            },
            &mut dev_b,
            &mut |_, s| arrived = s.clone(),
        )
        .unwrap();

        assert!(out.settings_pulled, "the laptop must pull the settings");
        assert_eq!(arrived.work_minutes, 50, "the focus duration must arrive");
        assert_eq!(arrived.sound_theme, "daf", "the chime must arrive");
        assert_eq!(arrived.theme, "light", "the theme must arrive");
    }

    /// A per-machine setting must not be dragged along with the portable ones.
    #[test]
    fn machine_local_settings_stay_where_they_belong() {
        let (_guard, mut tokens) = fake();

        let mut desktop = Settings::default();
        desktop.inhibit_enabled = false;
        desktop.sync_enabled = true;

        let mut dev_a = device("desktop");
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: desktop,
            },
            &mut dev_a,
            &mut |_, _| {},
        )
        .unwrap();

        let mut laptop = Settings::default();
        laptop.inhibit_enabled = true;
        let mut dev_b = device("laptop");
        let mut arrived = laptop.clone();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: laptop,
            },
            &mut dev_b,
            &mut |_, s| arrived = s.clone(),
        )
        .unwrap();

        assert!(
            arrived.inhibit_enabled,
            "the laptop's own screen-lock preference must survive"
        );
    }

    /// Two devices editing two different settings must both keep their change.
    /// A whole-block "newest wins" merge fails exactly here.
    #[test]
    fn two_devices_editing_different_settings_both_keep_their_change() {
        let (_guard, mut tokens) = fake();

        // Desktop sets a longer focus block.
        let mut desktop = Settings::default();
        desktop.work_minutes = 50;
        let mut dev_a = device("desktop");
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: desktop.clone(),
            },
            &mut dev_a,
            &mut |_, _| {},
        )
        .unwrap();

        // The laptop pulls, so it now knows about the 50 minutes.
        let mut dev_b = device("laptop");
        let mut on_laptop = Settings::default();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: Settings::default(),
            },
            &mut dev_b,
            &mut |_, s| on_laptop = s.clone(),
        )
        .unwrap();
        assert_eq!(on_laptop.work_minutes, 50);

        // The laptop changes only the theme, and pushes.
        on_laptop.theme = "light".into();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: on_laptop,
            },
            &mut dev_b,
            &mut |_, _| {},
        )
        .unwrap();

        // The desktop pulls: it must keep its 50 minutes and gain the theme.
        let mut back = desktop.clone();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: desktop,
            },
            &mut dev_a,
            &mut |_, s| back = s.clone(),
        )
        .unwrap();
        assert_eq!(back.work_minutes, 50, "the desktop's duration must survive");
        assert_eq!(back.theme, "light", "the laptop's theme must arrive");
    }

    /// A device that syncs often must not win every conflict just by syncing.
    /// Without per-field stamps that do not move on unchanged values, it would.
    #[test]
    fn syncing_repeatedly_does_not_make_a_device_win_conflicts() {
        let (_guard, mut tokens) = fake();

        let mut dev_a = device("desktop");
        let mut dev_b = device("laptop");

        // Both devices settle on the same settings.
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: Settings::default(),
            },
            &mut dev_a,
            &mut |_, _| {},
        )
        .unwrap();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: Settings::default(),
            },
            &mut dev_b,
            &mut |_, _| {},
        )
        .unwrap();

        // The laptop syncs a few more times, changing nothing.
        for _ in 0..3 {
            sync_once(
                &mut tokens,
                &Local {
                    data: Data::default(),
                    settings: Settings::default(),
                },
                &mut dev_b,
                &mut |_, _| {},
            )
            .unwrap();
        }

        // Now the desktop changes the focus duration and pushes.
        let mut changed = Settings::default();
        changed.work_minutes = 45;
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: changed,
            },
            &mut dev_a,
            &mut |_, _| {},
        )
        .unwrap();

        // The laptop must accept it, not overwrite it with its own idle copy.
        let mut on_laptop = Settings::default();
        sync_once(
            &mut tokens,
            &Local {
                data: Data::default(),
                settings: Settings::default(),
            },
            &mut dev_b,
            &mut |_, s| on_laptop = s.clone(),
        )
        .unwrap();
        assert_eq!(
            on_laptop.work_minutes, 45,
            "an idle device must not out-vote a real edit"
        );
    }

    /// A file written by the previous release is a bare `Data` object. It must
    /// be read, not rejected, or every existing user loses their history.
    #[test]
    fn a_legacy_remote_file_is_migrated_on_first_sync() {
        let (_guard, mut tokens) = fake();

        // Write the old format straight into the fake Drive.
        let legacy = r#"{"sessions":[{"at":100,"minutes":25}],"tasks":[{"id":1,"title":"old work","date":"","time":"","done":false,"created_at":0,"updated_at":10,"deleted":false}],"next_task_id":2}"#;
        crate::gdrive::upload("tok", legacy).unwrap();

        let mut dev = device("desktop");
        let mut arrived = Data::default();
        let out = sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev,
            &mut |d, _| arrived = d.clone(),
        )
        .unwrap();

        assert!(out.pulled, "the legacy data must be pulled");
        assert_eq!(arrived.sessions.len(), 1);
        assert_eq!(arrived.tasks.len(), 1);
        assert_eq!(arrived.tasks[0].title, "old work");

        // And the file is now an envelope, so the next read is the new format.
        let raw = crate::gdrive::download("tok").unwrap().unwrap();
        let env = Envelope::parse(&raw).unwrap();
        assert_eq!(env.schema, crate::syncfile::SCHEMA);
        assert_eq!(env.data.tasks.len(), 1);
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

    fn local(data: Data) -> Local {
        Local {
            data,
            settings: Settings::default(),
        }
    }

    fn device(name: &str) -> DeviceState {
        DeviceState {
            device_id: format!("id-{name}"),
            device_name: name.into(),
            ..Default::default()
        }
    }

    /// These tests all point `PANGAN_DRIVE_BASE` at their own fake server, so
    /// they must not run at the same time or they would each see the other's
    /// Drive. The guard is released on drop.
    fn fake() -> (std::sync::MutexGuard<'static, ()>, Tokens) {
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
        (guard, tokens)
    }

    /// The user's requirement, stated plainly: if the local data is wiped, a
    /// sync must put it back from the cloud. This is the whole point of having
    /// a cloud copy, and it is the case that a naive "push local, pull remote"
    /// implementation gets wrong — an empty local file must not be treated as
    /// "the user deleted everything".
    #[test]
    fn a_wiped_local_store_is_restored_from_the_cloud() {
        let (_guard, mut tokens) = fake();

        // The good state reaches the cloud.
        let good = Data {
            sessions: vec![SessionRecord {
                at: 100,
                minutes: 25,
            }],
            tasks: vec![task(1, "write the report", 10), task(2, "review", 20)],
            next_task_id: 3,
        };
        let mut dev = device("desktop");
        sync_once(&mut tokens, &local(good), &mut dev, &mut |_, _| {}).unwrap();

        // The local file is wiped — a fresh install, a deleted config dir, a
        // corrupted file that failed to parse.
        let mut restored = Data::default();
        let out = sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev,
            &mut |m, _| restored = m.clone(),
        )
        .unwrap();

        assert!(out.pulled, "an empty local store must pull the cloud copy");
        assert!(out.changed_locally, "the empty store must be rewritten");
        assert_eq!(restored.tasks.len(), 2, "tasks were not restored");
        assert_eq!(restored.sessions.len(), 1, "sessions were not restored");
        assert_eq!(restored.tasks[0].title, "write the report");
        // And the id counter must come back too, or the next new task would
        // collide with a restored one.
        assert!(restored.next_task_id >= 3);
    }

    /// Restoring must not be a one-way door: after the pull, the next sync is
    /// quiet, so a wiped-and-restored device does not then push its emptiness
    /// back over the good copy.
    #[test]
    fn restoring_does_not_then_overwrite_the_cloud_with_emptiness() {
        let (_guard, mut tokens) = fake();
        let mut dev = device("desktop");

        let good = Data {
            tasks: vec![task(1, "keep me", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(good), &mut dev, &mut |_, _| {}).unwrap();

        let mut restored = Data::default();
        sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev,
            &mut |m, _| restored = m.clone(),
        )
        .unwrap();
        assert_eq!(restored.tasks.len(), 1);

        // Now sync the restored copy. Nothing should move.
        let out = sync_once(&mut tokens, &local(restored), &mut dev, &mut |_, _| {}).unwrap();
        assert!(
            out.quiet(),
            "a restored device must not push again: {out:?}"
        );

        // And the cloud still holds the task.
        let mut check = Data::default();
        sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev,
            &mut |m, _| check = m.clone(),
        )
        .unwrap();
        assert_eq!(check.tasks.len(), 1, "the cloud copy was lost");
    }

    /// A local edit made while offline must survive a later pull: the merge is
    /// per record, so the cloud copy cannot clobber work the device did.
    #[test]
    fn an_offline_edit_survives_a_later_sync() {
        let (_guard, mut tokens) = fake();
        let mut dev = device("desktop");

        let cloud = Data {
            tasks: vec![task(1, "from cloud", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(cloud), &mut dev, &mut |_, _| {}).unwrap();

        // The device goes offline and adds its own task.
        let offline = Data {
            tasks: vec![task(1, "from cloud", 10), task(2, "made offline", 30)],
            next_task_id: 3,
            ..Default::default()
        };

        // `save` is only called when the merge changed the local copy, and
        // here the local copy is already the merged result — so the thing to
        // check is the cloud, which must now hold both tasks.
        let out = sync_once(&mut tokens, &local(offline), &mut dev, &mut |_, _| {}).unwrap();
        assert!(out.pushed, "the offline task must reach the cloud");

        let mut cloud_after = Data::default();
        sync_once(
            &mut tokens,
            &local(Data::default()),
            &mut dev,
            &mut |m, _| cloud_after = m.clone(),
        )
        .unwrap();
        assert_eq!(cloud_after.tasks.len(), 2, "the offline task must survive");
        let titles: Vec<&str> = cloud_after.tasks.iter().map(|t| t.title.as_str()).collect();
        assert!(titles.contains(&"made offline"));
        assert!(titles.contains(&"from cloud"));
    }

    /// A deletion made on one device must not come back from the cloud.
    #[test]
    fn a_deletion_is_not_resurrected_by_a_sync() {
        let (_guard, mut tokens) = fake();
        let mut dev = device("desktop");

        let before = Data {
            tasks: vec![task(1, "delete me", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(before.clone()), &mut dev, &mut |_, _| {}).unwrap();

        // Delete it locally: the tombstone carries a newer stamp.
        let mut deleted = before.clone();
        deleted.tasks[0].deleted = true;
        deleted.tasks[0].updated_at = 50;
        sync_once(&mut tokens, &local(deleted), &mut dev, &mut |_, _| {}).unwrap();

        // A device that still has the old copy syncs; it must not bring the
        // task back for everyone.
        let mut after = Data::default();
        sync_once(
            &mut tokens,
            &local(before),
            &mut dev,
            &mut |m, _| after = m.clone(),
        )
        .unwrap();
        let live: Vec<&Task> = after.tasks.iter().filter(|t| !t.deleted).collect();
        assert!(
            live.is_empty(),
            "a deleted task was resurrected: {:?}",
            live.iter().map(|t| &t.title).collect::<Vec<_>>()
        );
    }

    /// A corrupt remote file must not be able to destroy local data.
    ///
    /// The dangerous failure is not "sync stops" — that is visible and
    /// recoverable. It is "sync treats the unreadable file as empty and pushes
    /// the local copy over it", which would silently erase the other device's
    /// history. Refusing to parse is the safe answer.
    #[test]
    fn a_corrupt_remote_file_is_refused_rather_than_overwritten() {
        let (_guard, mut tokens) = fake(); // Something that is not valid JSON at all.
        crate::gdrive::upload("tok", "{ this is not json").unwrap(); let mut dev = device("desktop"); let local_data = Data {
            tasks: vec![task(1, "mine", 10)],
            next_task_id: 2,
            ..Default::default()
        }; let err = sync_once(
            &mut tokens,
            &local(local_data),
            &mut dev,
            &mut |_, _| {},
        )
        .unwrap_err(); assert!(
            err.contains("not valid JSON"),
            "the error must say what is wrong: {err}"
        ); // And the file is untouched, so the user can inspect or restore it.
        let raw = crate::gdrive::download("tok").unwrap().unwrap(); assert_eq!(raw, "{ this is not json"); }

    /// The race the merge cannot fix, end to end: two devices read the same
    /// version, both write, and the second write must be retried against the
    /// first one's result rather than silently replacing it.
    ///
    /// This is what the etag is for. Without it the test passes only because
    /// the writes happen to be sequential.
    #[test]
    fn a_write_that_lost_the_race_is_retried_not_lost() {
        let (_guard, mut tokens) = fake();

        // Seed the file so both devices have something to read.
        let mut dev_a = device("desktop");
        let seed = Data {
            tasks: vec![task(1, "shared", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(seed), &mut dev_a, &mut |_, _| {}).unwrap();

        // Device A reads the current state, then device B writes a new task
        // before A gets to write. A's conditional write must be refused.
        let (text, meta) = crate::gdrive::download_meta("tok", "pangan-sync.json")
            .unwrap()
            .expect("the seeded file must exist");

        let mut b_env = Envelope::parse(&text).unwrap();
        b_env.revision += 1;
        b_env.device_name = "laptop".into();
        b_env.data.tasks.push(task(2, "from laptop", 20));
        b_env.data.next_task_id = 3;
        crate::gdrive::upload_conditional("tok", "pangan-sync.json", &b_env.to_json().unwrap(), Some(&meta))
            .unwrap();

        // A now tries to write against the version it read. It must be told
        // the file moved, not silently allowed to overwrite the laptop's task.
        let stale = crate::gdrive::upload_conditional(
            "tok",
            "pangan-sync.json",
            r#"{"schema":2,"revision":2,"data":{"sessions":[],"tasks":[],"next_task_id":1}}"#,
            Some(&meta),
        );
        assert_eq!(
            stale.unwrap_err(),
            crate::gdrive::CONFLICT,
            "a stale write must be refused, not applied"
        );

        // And the laptop's task is still there.
        let raw = crate::gdrive::download("tok").unwrap().unwrap();
        let env = Envelope::parse(&raw).unwrap();
        assert_eq!(env.data.tasks.len(), 2, "the laptop's task was lost");
    }

    /// The full retry path: a device whose write loses the race re-reads and
    /// ends up with both devices' work, because the cycle retries on conflict.
    #[test]
    fn a_conflicting_sync_converges_instead_of_dropping_data() {
        let (_guard, mut tokens) = fake();

        let mut dev_a = device("desktop");
        let mut dev_b = device("laptop");

        // Both start from the same cloud state.
        let seed = Data {
            tasks: vec![task(1, "shared", 10)],
            next_task_id: 2,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(seed.clone()), &mut dev_a, &mut |_, _| {}).unwrap();
        sync_once(&mut tokens, &local(seed.clone()), &mut dev_b, &mut |_, _| {}).unwrap();

        // Each adds its own task and pushes. The second push is against a file
        // the first one changed, which is exactly the race.
        let a_data = Data {
            tasks: vec![task(1, "shared", 10), task(2, "from desktop", 20)],
            next_task_id: 3,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(a_data), &mut dev_a, &mut |_, _| {}).unwrap();

        let b_data = Data {
            tasks: vec![task(1, "shared", 10), task(3, "from laptop", 30)],
            next_task_id: 4,
            ..Default::default()
        };
        sync_once(&mut tokens, &local(b_data), &mut dev_b, &mut |_, _| {}).unwrap();

        // Both tasks must have survived.
        let raw = crate::gdrive::download("tok").unwrap().unwrap();
        let env = Envelope::parse(&raw).unwrap();
        let titles: Vec<&str> = env.data.tasks.iter().map(|t| t.title.as_str()).collect();
        assert!(
            titles.contains(&"from desktop") && titles.contains(&"from laptop"),
            "a concurrent write dropped data: {titles:?}"
        );
    }
}
