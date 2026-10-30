//! Persistence layer: JSON store for session history, tasks, and app state.
//! Data lives under the config dir (`~/.config/pangan/data.json` on Linux).

use crate::journal::{Decree, Inscription};
use crate::state::rota::{self, RotaEntry, RotaError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A completed focus session, recorded when a Work phase finishes naturally.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Unix timestamp (seconds) when the session completed.
    pub at: i64,
    /// Scheduled focus minutes for this session (config work_minutes).
    pub minutes: u64,
}

/// A task in the scheduling board.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub title: String,
    /// Optional date key `YYYY-MM-DD` (empty = no due date / anytime).
    pub date: String,
    /// Optional time key `HH:MM` (empty = all day).
    pub time: String,
    /// true when the task is completed.
    pub done: bool,
    pub created_at: i64,
    /// Unix seconds of the last local edit. Sync merges per task, so it needs
    /// to know which side of a conflict is newer; without this the only
    /// option was "last writer wins for the whole file", which silently threw
    /// away edits made on the other device.
    #[serde(default)]
    pub updated_at: i64,
    /// Set when the task is deleted. The row stays in the store (invisible to
    /// the UI) so the deletion can travel to other devices; a row that simply
    /// vanishes cannot be told apart from a row the other device never saw.
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Data {
    #[serde(default)]
    pub sessions: Vec<SessionRecord>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub next_task_id: u64,
    /// One written line per finished focus session. Append-only, like sessions:
    /// the record of what was done is never edited away.
    #[serde(default)]
    pub inscriptions: Vec<Inscription>,
    /// One intended line per focus session — what the block was *for*, written
    /// before it ran. Paired with `inscriptions` by `session_at`, so a session
    /// that promised something and produced nothing is visible as a decree
    /// with no inscription facing it.
    #[serde(default)]
    pub decrees: Vec<Decree>,
    /// The ordered queue of blocks a day is meant to hold, plus every entry the
    /// user has taken off it. See `state::rota` for why entries keep the day
    /// they were planned for instead of following the user forward.
    #[serde(default)]
    pub rota: Vec<RotaEntry>,
    /// Hands out rota entry ids. Separate from `next_task_id` because the two
    /// counters live in different collection types and sharing one would tie
    /// two unrelated id spaces together for no gain.
    #[serde(default)]
    pub next_rota_id: u64,
}

impl Data {
    pub fn next_task_id(&mut self) -> u64 {
        self.next_task_id += 1;
        self.next_task_id
    }
}

pub struct Store {
    path: PathBuf,
    pub data: Data,
}

impl Store {
    pub fn data_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("pangan")
    }

    pub fn path() -> PathBuf {
        Self::data_dir().join("data.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { path, data }
    }

    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.data) {
            let _ = std::fs::write(&self.path, json);
        }
    }

    /// Append completed sessions (used by the event loop).
    pub fn add_sessions(&mut self, recs: &[SessionRecord]) -> usize {
        let mut added = 0;
        for r in recs {
            // dedupe by exact timestamp+minutes to avoid double-count on reload
            let dup = self
                .data
                .sessions
                .iter()
                .any(|s| s.at == r.at && s.minutes == r.minutes);
            if !dup {
                self.data.sessions.push(r.clone());
                added += 1;
            }
        }
        if added > 0 {
            self.save();
        }
        added
    }

    pub fn add_task(&mut self, title: String, date: String, time: String) -> u64 {
        let id = self.data.next_task_id();
        let now = chrono::Utc::now().timestamp();
        let t = Task {
            id,
            title,
            date,
            time,
            done: false,
            created_at: now,
            updated_at: now,
            deleted: false,
        };
        self.data.tasks.push(t);
        self.save();
        id
    }

    /// Stamp a task as edited now. Every mutation goes through this, because a
    /// task whose `updated_at` did not move would lose the merge against the
    /// other device's older copy.
    fn touch(t: &mut Task) {
        t.updated_at = chrono::Utc::now().timestamp();
    }

    pub fn set_done(&mut self, id: u64, done: bool) {
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.done = done;
            Self::touch(t);
            self.save();
        }
    }

    /// Delete a task by marking it, not by dropping the row.
    ///
    /// A row that simply disappears cannot be told apart from a row another
    /// device has never seen, so the other device would happily push its copy
    /// back on the next sync and the task would return. The tombstone travels
    /// instead, and the UI filters deleted rows out.
    pub fn remove_task(&mut self, id: u64) {
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.deleted = true;
            Self::touch(t);
            self.save();
        }
    }

    /// Move a task to another day, keeping its time.
    #[cfg(test)]
    pub fn set_task_date(&mut self, id: u64, date: String) {
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.date = date;
            Self::touch(t);
            self.save();
        }
    }

    /// Rename a task. An empty title is refused rather than saved, because a
    /// task with no text is indistinguishable from a deleted one but harder to
    /// get rid of.
    pub fn set_task_title(&mut self, id: u64, title: String) {
        let title = title.trim().to_string();
        if title.is_empty() {
            return;
        }
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.title = title;
            Self::touch(t);
            self.save();
        }
    }

    /// Shift a task by a whole number of days, keeping everything else.
    ///
    /// An undated task cannot be shifted — there is no day to move from — so
    /// it is pinned to today first and then moved, which is what the user
    /// means when they nudge an undated item forward.
    pub fn shift_task_days(&mut self, id: u64, delta: i32, today: &str) -> Option<String> {
        let cur = self.data.tasks.iter().find(|t| t.id == id)?.date.clone();
        let base = if cur.is_empty() { today } else { cur.as_str() };
        let next = crate::calendar::jalali::shift_day_key(base, delta)?;
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.date = next.clone();
            Self::touch(t);
            self.save();
        }
        Some(next)
    }

    /// Tasks due on one date key (`YYYY-MM-DD`), open ones first, then by time.
    ///
    /// A task with no date is not "due never" — it is an undated back-burner
    /// item, and it belongs on the day you are looking at so it stays visible
    /// instead of silently disappearing from every view.
    /// Every task that is not a deletion tombstone.
    ///
    /// Deleted tasks stay in the store so the deletion can sync (see
    /// `remove_task`), so every read path has to filter them out. Going
    /// through this one method means a new screen cannot forget to.
    pub fn live_tasks(&self) -> impl Iterator<Item = &Task> {
        self.data.tasks.iter().filter(|t| !t.deleted)
    }

    pub fn tasks_on(&self, date: &str) -> Vec<Task> {
        let mut out: Vec<Task> = self
            .data
            .tasks
            .iter()
            .filter(|t| !t.deleted)
            .filter(|t| t.date == date || t.date.is_empty())
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            a.done
                .cmp(&b.done)
                .then_with(|| a.time.cmp(&b.time))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// How many open tasks sit on each date, for the month grid's dots.
    ///
    /// Returns `(date key, open count)` for every date that has at least one
    /// open task. Undated tasks are deliberately excluded: they belong to no
    /// particular square, and counting them on every day would be a lie.
    pub fn open_counts_by_date(&self) -> Vec<(String, u32)> {
        let mut counts: std::collections::BTreeMap<String, u32> = Default::default();
        for t in self
            .data
            .tasks
            .iter()
            .filter(|t| !t.done && !t.date.is_empty())
        {
            *counts.entry(t.date.clone()).or_insert(0) += 1;
        }
        counts.into_iter().collect()
    }

    // ── inscriptions ──────────────────────────────────────────────────────
    //
    // The archive is append-only by design. There is no `edit_inscription`,
    // because the point of a record cut in stone is that it says what you
    // wrote on the day, not what you wish you had written later. A wrong entry
    // can be deleted, never rewritten.

    /// Keep an inscription. Returns false when it was already present.
    ///
    /// The id is derived from (session, text), so re-adding the same writing
    /// for the same session — a retry, a sync arriving twice, a UI firing the
    /// callback twice — cannot create a duplicate row.
    pub fn add_inscription(&mut self, ins: Inscription) -> bool {
        if self.data.inscriptions.iter().any(|i| i.id == ins.id) {
            return false;
        }
        self.data.inscriptions.push(ins);
        self.save();
        true
    }

    /// The inscription written for one session, if any.
    ///
    /// A session can only carry one line; asking twice must not open the dialog
    /// twice, and a user who skipped must not be asked again on the next tick.
    pub fn inscription_for_session(&self, session_at: i64) -> Option<&Inscription> {
        self.data
            .inscriptions
            .iter()
            .find(|i| i.session_at == session_at && !i.deleted)
    }

    /// Every live inscription, newest first.
    ///
    /// Discarded entries stay in the store as tombstones so the discard can
    /// sync (see `remove_inscription`), which means every read path has to
    /// filter them. Routing through one method is what stops a new screen from
    /// forgetting to.
    pub fn inscriptions_newest_first(&self) -> Vec<Inscription> {
        let mut out: Vec<Inscription> = self
            .data
            .inscriptions
            .iter()
            .filter(|i| !i.deleted)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.session_at.cmp(&a.session_at));
        out
    }

    /// Discard an inscription by marking it, not by dropping the row.
    ///
    /// Same reasoning as `remove_task`: a row that simply disappears cannot be
    /// told apart from a row the other device never received, so the other
    /// device's copy would put it back on the next sync.
    pub fn remove_inscription(&mut self, id: &str) -> bool {
        match self.data.inscriptions.iter_mut().find(|i| i.id == id) {
            Some(i) if !i.deleted => {
                i.deleted = true;
                self.save();
                true
            }
            _ => false,
        }
    }

    // ── decrees ───────────────────────────────────────────────────────────
    //
    // The other half of the pair. A decree is written before its block runs and
    // only becomes a row once that block ends, so there is no "pending decree"
    // in the store — see `journal::decree` for why.

    /// Keep a decree. Returns false when it was already present.
    ///
    /// `(session, text)` ids make a retry or a re-delivered sync a no-op rather
    /// than a second row for one intention.
    pub fn add_decree(&mut self, dec: Decree) -> bool {
        if self.data.decrees.iter().any(|d| d.id == dec.id) {
            return false;
        }
        self.data.decrees.push(dec);
        self.save();
        true
    }

    /// The decree written for one session, if any.
    ///
    /// This is the pair lookup the archive uses: given the session an
    /// inscription belongs to, find the intention that opened it.
    pub fn decree_for_session(&self, session_at: i64) -> Option<&Decree> {
        self.data
            .decrees
            .iter()
            .find(|d| d.session_at == session_at && !d.deleted)
    }

    /// Every live decree, newest first.
    ///
    /// Tombstones stay in the store so a discard can sync, so every read path
    /// filters them — through this one method, so no screen can forget to.
    pub fn decrees_newest_first(&self) -> Vec<Decree> {
        let mut out: Vec<Decree> = self
            .data
            .decrees
            .iter()
            .filter(|d| !d.deleted)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.session_at.cmp(&a.session_at));
        out
    }

    /// Discard a decree by marking it. A wrong intention is not rewritten.
    pub fn remove_decree(&mut self, id: &str) -> bool {
        match self.data.decrees.iter_mut().find(|d| d.id == id) {
            Some(d) if !d.deleted => {
                d.deleted = true;
                self.save();
                true
            }
            _ => false,
        }
    }

    /// How many finished sessions carried a stated intention.
    ///
    /// The pair is what makes this answerable: an intention on its own proves
    /// nothing, so only decrees whose session actually completed and still has
    /// a live decree are counted. Sessions from before decrees existed are
    /// simply not in the denominator, which keeps the number honest instead of
    /// reporting a low rate the user never had a chance to earn.
    pub fn decreed_session_count(&self) -> usize {
        let completed: std::collections::HashSet<i64> =
            self.data.sessions.iter().map(|s| s.at).collect();
        self.data
            .decrees
            .iter()
            .filter(|d| !d.deleted && completed.contains(&d.session_at))
            .count()
    }

    // ── rota ──────────────────────────────────────────────────────────────
    //
    // The day's queue of planned blocks. Every rule lives in `state::rota`;
    // these methods only own the id counter and the save, so the rules cannot
    // drift between the store and a test.

    /// Plan a block on `day` from raw user input.
    ///
    /// `all` is read from the store rather than passed in because the cap is a
    /// property of the day, and a caller that had to remember to supply the
    /// current list could forget.
    pub fn add_rota_entry(
        &mut self,
        day: &str,
        label: &str,
        task_id: Option<u64>,
        decree: &str,
        minutes: u64,
    ) -> Result<u64, RotaError> {
        let id = self.data.next_rota_id + 1;
        let entry = rota::create(
            id,
            day,
            label,
            task_id,
            decree,
            minutes,
            chrono::Utc::now().timestamp(),
            &self.data.rota,
        )?;
        self.data.next_rota_id = id;
        let new_id = entry.id;
        self.data.rota.push(entry);
        self.save();
        Ok(new_id)
    }

    /// The entries on one day, in the order they were planned.
    pub fn rota_for_day(&self, day: &str) -> Vec<RotaEntry> {
        rota::for_day(&self.data.rota, day)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The block to start next on `day`, if the rota has one left.
    pub fn rota_next(&self, day: &str) -> Option<RotaEntry> {
        rota::peek_next(&self.data.rota, day).cloned()
    }

    /// Mark a planned block as started. Returns false when it was not pending.
    pub fn open_rota_entry(&mut self, id: u64) -> bool {
        if rota::mark_opened(&mut self.data.rota, id, chrono::Utc::now().timestamp()) {
            self.save();
            true
        } else {
            false
        }
    }

    /// Take an entry off the rota by marking it.
    pub fn remove_rota_entry(&mut self, id: u64) -> bool {
        if rota::remove(&mut self.data.rota, id) {
            self.save();
            true
        } else {
            false
        }
    }

    /// Unstarted entries left behind on earlier days, oldest first.
    pub fn rota_carry_over(&self, today: &str) -> Vec<RotaEntry> {
        rota::carry_over(&self.data.rota, today)
    }

    /// Re-plan one stale entry onto `today` and retire the original.
    ///
    /// Both halves happen together so the stale row can never be offered twice:
    /// a replan that tombstoned nothing would return the same leftover on every
    /// launch, and one that only tombstoned would lose the plan.
    pub fn carry_rota_entry_forward(&mut self, id: u64, today: &str) -> Option<u64> {
        let stale = self.data.rota.iter().find(|e| e.id == id)?.clone();
        if !stale.is_pending() {
            return None;
        }
        let new_id = self.data.next_rota_id + 1;
        let fresh = rota::replan(&stale, new_id, today, chrono::Utc::now().timestamp());
        self.data.next_rota_id = new_id;
        rota::remove(&mut self.data.rota, id);
        self.data.rota.push(fresh);
        self.save();
        Some(new_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_task_assigns_increasing_ids() {
        let mut store = Store::load();
        store.data = Data::default();
        store.data.next_task_id = 0;
        let id1 = store.add_task("Write".into(), "2026-10-28".into(), String::new());
        let id2 = store.add_task("Review".into(), "2026-10-29".into(), "10:30".into());
        assert!(id2 > id1);
        assert_eq!(store.data.tasks.len(), 2);
        assert_eq!(store.data.tasks[1].time, "10:30");
    }

    #[test]
    fn set_done_toggles_task() {
        let mut store = Store::load();
        store.data = Data::default();
        store.data.next_task_id = 0;
        let id = store.add_task("A".into(), String::new(), String::new());
        store.set_done(id, true);
        assert!(store.data.tasks[0].done);
        store.remove_task(id);
        // The row stays as a tombstone so the deletion can reach other
        // devices; every read path filters it out.
        assert_eq!(store.data.tasks.len(), 1, "the row is kept as a tombstone");
        assert!(store.data.tasks[0].deleted);
        assert_eq!(store.live_tasks().count(), 0, "but it is not visible");
    }

    #[test]
    fn an_edit_moves_the_updated_stamp() {
        let mut store = Store::load();
        store.data = Data::default();
        store.data.next_task_id = 0;
        let id = store.add_task("A".into(), String::new(), String::new());
        store.data.tasks[0].updated_at = 1; // pretend it was written long ago
        store.set_task_title(id, "B".into());
        assert!(
            store.data.tasks[0].updated_at > 1,
            "a rename must win the merge against the older copy"
        );
    }

    #[test]
    fn add_sessions_dedupes() {
        let mut store = Store::load();
        store.data = Data::default();
        let rec = SessionRecord {
            at: 1234,
            minutes: 25,
        };
        let added = store.add_sessions(std::slice::from_ref(&rec));
        assert_eq!(added, 1);
        let added2 = store.add_sessions(std::slice::from_ref(&rec));
        assert_eq!(added2, 0);
        assert_eq!(store.data.sessions.len(), 1);
    }
}

#[cfg(test)]
mod day_tests {
    use super::*;
    /// An in-memory store pointing at a throwaway path.
    ///
    /// These tests used `Store::load()`, which reads the user's real data file,
    /// and `add_task` saves on every call — so running the suite wrote test
    /// tasks into the live store. A test must never touch the real file.
    fn fresh() -> Store {
        let dir = std::env::temp_dir().join(format!(
            "pangan-test-{}-{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::create_dir_all(&dir);
        Store {
            path: dir.join("data.json"),
            data: Data::default(),
        }
    }

    #[test]
    fn a_task_lands_on_the_day_it_was_added_to() {
        let mut s = fresh();
        s.add_task("خرید نان".into(), "1405-08-20".into(), String::new());
        assert_eq!(s.tasks_on("1405-08-20").len(), 1); // And not on any other day.
        assert_eq!(s.tasks_on("1405-08-21").len(), 0);
    }

    #[test]
    fn a_future_day_keeps_its_own_work() {
        let mut s = fresh();
        s.add_task("today".into(), "1405-08-07".into(), String::new());
        s.add_task("next week".into(), "1405-08-14".into(), String::new());
        let today = s.tasks_on("1405-08-07");
        assert_eq!(today.len(), 1);
        assert_eq!(today[0].title, "today");
        let later = s.tasks_on("1405-08-14");
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].title, "next week");
    }

    #[test]
    fn undated_tasks_stay_visible_on_every_day() {
        let mut s = fresh();
        s.add_task("someday".into(), String::new(), String::new());
        assert_eq!(s.tasks_on("1405-08-07").len(), 1);
        assert_eq!(s.tasks_on("1405-12-01").len(), 1);
    }

    #[test]
    fn open_tasks_come_before_done_ones_and_then_by_time() {
        let mut s = fresh();
        let a = s.add_task("late".into(), "1405-08-07".into(), "18:00".into());
        s.add_task("early".into(), "1405-08-07".into(), "08:00".into());
        s.set_done(a, true);
        let list = s.tasks_on("1405-08-07");
        assert_eq!(list[0].title, "early", "open task should lead");
        assert_eq!(list[1].title, "late", "done task should sink");
    }

    #[test]
    fn dots_count_open_work_only() {
        let mut s = fresh();
        s.add_task("a".into(), "1405-08-07".into(), String::new());
        let b = s.add_task("b".into(), "1405-08-07".into(), String::new());
        s.add_task("c".into(), "1405-08-09".into(), String::new());
        s.add_task("undated".into(), String::new(), String::new());
        let counts: std::collections::HashMap<String, u32> =
            s.open_counts_by_date().into_iter().collect();
        assert_eq!(counts.get("1405-08-07"), Some(&2));
        assert_eq!(counts.get("1405-08-09"), Some(&1)); // Undated work belongs to no square.
        assert_eq!(counts.len(), 2);
        s.set_done(b, true);
        let counts: std::collections::HashMap<String, u32> =
            s.open_counts_by_date().into_iter().collect();
        assert_eq!(
            counts.get("1405-08-07"),
            Some(&1),
            "done work should not dot the day"
        );
    }

    #[test]
    fn moving_a_task_moves_it_between_days() {
        let mut s = fresh();
        let id = s.add_task("move me".into(), "1405-08-07".into(), String::new());
        s.set_task_date(id, "1405-08-10".into());
        assert_eq!(s.tasks_on("1405-08-07").len(), 0);
        assert_eq!(s.tasks_on("1405-08-10").len(), 1);
    }
}

#[cfg(test)]
mod manage_tests {
    use super::*;
    /// An in-memory store pointing at a throwaway path.
    ///
    /// These tests used `Store::load()`, which reads the user's real data file,
    /// and `add_task` saves on every call — so running the suite wrote test
    /// tasks into the live store. A test must never touch the real file.
    fn fresh() -> Store {
        let dir = std::env::temp_dir().join(format!(
            "pangan-test-{}-{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::create_dir_all(&dir);
        Store {
            path: dir.join("data.json"),
            data: Data::default(),
        }
    }

    #[test]
    fn nudging_a_task_moves_it_one_day() {
        let mut s = fresh();
        let id = s.add_task("t".into(), "1405-08-07".into(), String::new());
        let next = s.shift_task_days(id, 1, "1405-08-07").unwrap();
        assert_eq!(next, "1405-08-08");
        assert_eq!(s.tasks_on("1405-08-08").len(), 1);
        assert_eq!(s.tasks_on("1405-08-07").len(), 0);
    }

    #[test]
    fn nudging_back_over_a_month_boundary_works() {
        let mut s = fresh(); // 1 Aban back one day is 30 Mehr (Mehr has 30 days).
        let id = s.add_task("t".into(), "1405-08-01".into(), String::new());
        let prev = s.shift_task_days(id, -1, "1405-08-01").unwrap();
        assert_eq!(prev, "1405-07-30");
    }

    #[test]
    fn nudging_an_undated_task_pins_it_to_today_first() {
        let mut s = fresh();
        let id = s.add_task("someday".into(), String::new(), String::new());
        let next = s.shift_task_days(id, 1, "1405-08-07").unwrap();
        assert_eq!(next, "1405-08-08");
    }

    #[test]
    fn renaming_replaces_the_title() {
        let mut s = fresh();
        let id = s.add_task("old".into(), "1405-08-07".into(), String::new());
        s.set_task_title(id, "  new  ".into());
        assert_eq!(s.tasks_on("1405-08-07")[0].title, "new");
    }

    #[test]
    fn an_empty_rename_is_refused() {
        let mut s = fresh();
        let id = s.add_task("keep me".into(), "1405-08-07".into(), String::new());
        s.set_task_title(id, "   ".into());
        assert_eq!(s.tasks_on("1405-08-07")[0].title, "keep me");
    }

    #[test]
    fn deleting_removes_it_from_the_day() {
        let mut s = fresh();
        let id = s.add_task("bye".into(), "1405-08-07".into(), String::new());
        s.remove_task(id);
        assert_eq!(s.tasks_on("1405-08-07").len(), 0);
    }

    /// The user's requirement: tasks must survive a restart, not reset.
    #[test]
    fn tasks_survive_a_reload_from_disk() {
        // A temporary file, not the real store: the other tests in this module
        // share the live data file, so pointing this one at it made the result
        // depend on test order. It passed alone and failed in the suite, which
        // is a broken test rather than a broken app.
        let dir = std::env::temp_dir().join(format!("pangan-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.json");
        let _ = std::fs::remove_file(&path);

        let mut s = Store {
            path: path.clone(),
            data: Data::default(),
        };
        s.add_task("persisted".into(), "1405-09-03".into(), "09:30".into());
        s.save();

        // Read it back the way a fresh launch would.
        let raw = std::fs::read_to_string(&path).unwrap();
        let reloaded: Data = serde_json::from_str(&raw).unwrap();
        let store = Store {
            path: path.clone(),
            data: reloaded,
        };
        let back = store.tasks_on("1405-09-03");
        assert_eq!(back.len(), 1, "task did not survive the reload");
        assert_eq!(back[0].title, "persisted");
        assert_eq!(back[0].time, "09:30");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── decrees ──────────────────────────────────────────────────────────

    fn temp_store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "pangan-decree-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store {
            path: dir.join("data.json"),
            data: Data::default(),
        };
        (store, dir)
    }

    #[test]
    fn a_decree_is_found_by_the_session_it_opened() {
        let (mut s, dir) = temp_store();
        let d = crate::journal::decree::create(1000, 400, "write the report").unwrap();
        assert!(s.add_decree(d.clone()));
        let found = s.decree_for_session(1000).expect("must pair");
        assert_eq!(found.text, "write the report");
        assert!(s.decree_for_session(2000).is_none(), "no false pairing");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn adding_the_same_decree_twice_keeps_one() {
        let (mut s, dir) = temp_store();
        let d = crate::journal::decree::create(1000, 400, "one intention").unwrap();
        assert!(s.add_decree(d.clone()), "first insert");
        assert!(!s.add_decree(d), "second is a no-op, not an error");
        assert_eq!(s.decrees_newest_first().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_discarded_decree_is_hidden_but_kept_for_sync() {
        let (mut s, dir) = temp_store();
        let d = crate::journal::decree::create(1000, 400, "withdrawn").unwrap();
        let id = d.id.clone();
        s.add_decree(d);
        assert!(s.remove_decree(&id));
        assert!(
            s.decrees_newest_first().is_empty(),
            "a discard must not be shown"
        );
        assert!(
            s.data.decrees.iter().any(|d| d.id == id && d.deleted),
            "the tombstone stays so the discard can reach the other device"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_sessions_that_actually_finished_count_as_decreed() {
        let (mut s, dir) = temp_store();
        // One session finished, one decree issued for it; a second decree
        // exists for a session the timer never completed (the user backed out
        // after the block was reset, so no SessionRecord was ever written).
        s.add_sessions(&[SessionRecord {
            at: 1000,
            minutes: 25,
        }]);
        s.add_decree(crate::journal::decree::create(1000, 400, "counted").unwrap());
        s.add_decree(crate::journal::decree::create(2000, 1400, "not counted").unwrap());
        assert_eq!(
            s.decreed_session_count(),
            1,
            "an intention for work that never completed proves nothing"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
