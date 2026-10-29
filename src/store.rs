//! Persistence layer: JSON store for session history, tasks, and app state.
//! Data lives under the config dir (`~/.config/pangan/data.json` on Linux).

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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Data {
    #[serde(default)]
    pub sessions: Vec<SessionRecord>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub next_task_id: u64,
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
        let t = Task {
            id,
            title,
            date,
            time,
            done: false,
            created_at: chrono::Utc::now().timestamp(),
        };
        self.data.tasks.push(t);
        self.save();
        id
    }

    pub fn set_done(&mut self, id: u64, done: bool) {
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.done = done;
            self.save();
        }
    }

    pub fn remove_task(&mut self, id: u64) {
        self.data.tasks.retain(|t| t.id != id);
        self.save();
    }

    /// Move a task to another day, keeping its time.
    pub fn set_task_date(&mut self, id: u64, date: String) {
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.date = date;
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
        let next = crate::calendar::shift_day_key(base, delta)?;
        if let Some(t) = self.data.tasks.iter_mut().find(|t| t.id == id) {
            t.date = next.clone();
            self.save();
        }
        Some(next)
    }

    /// Tasks due on one date key (`YYYY-MM-DD`), open ones first, then by time.
    ///
    /// A task with no date is not "due never" — it is an undated back-burner
    /// item, and it belongs on the day you are looking at so it stays visible
    /// instead of silently disappearing from every view.
    pub fn tasks_on(&self, date: &str) -> Vec<Task> {
        let mut out: Vec<Task> = self
            .data
            .tasks
            .iter()
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
        assert!(store.data.tasks.is_empty());
    }

    #[test]
    fn add_sessions_dedupes() {
        let mut store = Store::load();
        store.data = Data::default();
        let rec = SessionRecord {
            at: 1234,
            minutes: 25,
        };
        let added = store.add_sessions(&[rec.clone()]);
        assert_eq!(added, 1);
        let added2 = store.add_sessions(&[rec.clone()]);
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
}
