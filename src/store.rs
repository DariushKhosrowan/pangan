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
            let dup = self.data.sessions.iter().any(|s| s.at == r.at && s.minutes == r.minutes);
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
        let rec = SessionRecord { at: 1234, minutes: 25 };
        let added = store.add_sessions(&[rec.clone()]);
        assert_eq!(added, 1);
        let added2 = store.add_sessions(&[rec.clone()]);
        assert_eq!(added2, 0);
        assert_eq!(store.data.sessions.len(), 1);
    }
}
