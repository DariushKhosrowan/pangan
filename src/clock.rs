//! Platform-agnostic timer core: drift-free countdown state machine.
//! The deadline is an `Instant`; every tick recomputes remaining from the
//! monotonic clock, so there is no `sleep()` drift.

use crate::settings::Settings;
use crate::store::SessionRecord;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub work_minutes: u64,
    pub short_break_minutes: u64,
    pub long_break_minutes: u64,
    pub long_break_interval: u32,
    pub auto_start_next: bool,
}

impl From<&Settings> for Config {
    fn from(s: &Settings) -> Self {
        Self {
            work_minutes: s.work_minutes,
            short_break_minutes: s.short_break_minutes,
            long_break_minutes: s.long_break_minutes,
            long_break_interval: s.long_break_interval,
            auto_start_next: s.auto_start_next,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
        }
    }
}

pub fn load_config() -> Config {
    Config::from(&Settings::load())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Work => "کار ژرف",
            Phase::ShortBreak => "آسایش کوتاه",
            Phase::LongBreak => "آسایش دراز",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub time: String,
    /// Remaining time split out, so the UI can render Persian numerals
    /// without having to parse "MM:SS" back apart.
    pub minutes: u32,
    pub seconds: u32,
    pub session: String,
    pub progress: f32,
    pub phase: String,
    pub running: bool,
    pub work_done: u32,
}

pub struct State {
    pub phase: Phase,
    pub running: bool,
    pub remaining: Duration,
    pub total: Duration,
    pub work_done: u32,
    pub finish_at: Option<Instant>,
}

pub struct Shared {
    state: Mutex<State>,
    pub quit: std::sync::atomic::AtomicBool,
    pub cfg: Config,
    /// Completed focus sessions not yet persisted by the event loop.
    pending_sessions: Mutex<Vec<SessionRecord>>,
    /// Count of completed breaks (rest periods) not yet consumed.
    pending_breaks: Mutex<u32>,
    /// Life-cycle automation events not yet fired by the event loop
    /// (e.g. "focus_start", "break_end", ...).
    pending_events: Mutex<Vec<String>>,
}

impl Shared {
    pub fn new(cfg: Config) -> Self {
        let total = Duration::from_secs(cfg.work_minutes * 60);
        Self {
            state: Mutex::new(State {
                phase: Phase::Work,
                running: false,
                remaining: total,
                total,
                work_done: 0,
                finish_at: None,
            }),
            quit: std::sync::atomic::AtomicBool::new(false),
            cfg,
            pending_sessions: Mutex::new(Vec::new()),
            pending_breaks: Mutex::new(0),
            pending_events: Mutex::new(Vec::new()),
        }
    }

    /// Drain automation life-cycle events since the last call.
    pub fn drain_events(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending_events.lock().unwrap())
    }

    /// Drain and return completed sessions since the last call.
    pub fn drain_sessions(&self) -> Vec<SessionRecord> {
        std::mem::take(&mut *self.pending_sessions.lock().unwrap())
    }

    /// Drain the count of completed breaks since the last call.
    pub fn take_breaks(&self) -> u32 {
        std::mem::take(&mut *self.pending_breaks.lock().unwrap())
    }

    fn phase_total(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Work => Duration::from_secs(self.cfg.work_minutes * 60),
            Phase::ShortBreak => Duration::from_secs(self.cfg.short_break_minutes * 60),
            Phase::LongBreak => Duration::from_secs(self.cfg.long_break_minutes * 60),
        }
    }

    /// Advance to next phase; returns a notification string when a session
    /// completed so the caller can show a desktop notification.
    pub fn advance(&self, increment_work: bool) -> Option<String> {
        let mut s = self.state.lock().unwrap();

        let notify = if increment_work {
            s.work_done += 1;
            let is_long = s.work_done.is_multiple_of(self.cfg.long_break_interval);
            s.phase = if is_long {
                Phase::LongBreak
            } else {
                Phase::ShortBreak
            };
            // Automation: a focus session completed; a break begins.
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::settings::EVENT_FOCUS_END.into());
            if is_long {
                self.pending_events
                    .lock()
                    .unwrap()
                    .push(crate::settings::EVENT_POMODORO_COMPLETE.into());
            }
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::settings::EVENT_BREAK_START.into());
            Some(format!(
                "{} به پایان رسید — {} آغاز شد.",
                Phase::Work.label(),
                s.phase.label()
            ))
        } else {
            s.phase = Phase::Work;
            // Automation: the break ended; focus begins again.
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::settings::EVENT_BREAK_END.into());
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::settings::EVENT_FOCUS_START.into());
            Some("آسایش به سر رسید — بازگشت به کار ژرف.".to_string())
        };
        s.total = self.phase_total(s.phase);
        s.remaining = s.total;
        s.running = self.cfg.auto_start_next;
        // When auto-start is off, land paused ready for the user to begin.
        s.finish_at = if s.running {
            Some(Instant::now() + s.remaining)
        } else {
            None
        };
        notify
    }

    /// Called by the UI tick. Returns an optional notification string.
    pub fn tick(&self) -> Option<String> {
        let mut s = self.state.lock().unwrap();
        if !s.running {
            return None;
        }
        let now = Instant::now();
        if let Some(finish) = s.finish_at {
            s.remaining = finish.saturating_duration_since(now);
            if s.remaining.is_zero() {
                if s.phase == Phase::Work {
                    let rec = SessionRecord {
                        at: chrono::Utc::now().timestamp(),
                        minutes: self.cfg.work_minutes,
                    };
                    self.pending_sessions.lock().unwrap().push(rec);
                } else {
                    *self.pending_breaks.lock().unwrap() += 1;
                }
                drop(s);
                return self.advance(true);
            }
        }
        None
    }

    pub fn toggle(&self) {
        let mut s = self.state.lock().unwrap();
        s.running = !s.running;
        if s.running {
            s.finish_at = Some(Instant::now() + s.remaining);
            if s.phase == Phase::Work {
                self.pending_events
                    .lock()
                    .unwrap()
                    .push(crate::settings::EVENT_FOCUS_START.into());
            }
        } else {
            s.finish_at = None;
        }
    }

    pub fn skip(&self) -> Option<String> {
        let phase = self.state.lock().unwrap().phase;
        self.advance(phase == Phase::Work)
    }

    pub fn reset(&self) {
        let mut s = self.state.lock().unwrap();
        s.phase = Phase::Work;
        s.running = false;
        s.remaining = self.phase_total(Phase::Work);
        s.total = s.remaining;
        s.finish_at = None;
        s.work_done = 0;
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = self.state.lock().unwrap();
        let secs = s.remaining.as_secs();
        let time = format!("{:02}:{:02}", secs / 60, secs % 60);
        let progress = if s.total.is_zero() {
            0.0
        } else {
            (s.total.as_secs_f32() - s.remaining.as_secs_f32()) / s.total.as_secs_f32()
        };
        Snapshot {
            time,
            minutes: (secs / 60) as u32,
            seconds: (secs % 60) as u32,
            session: s.phase.label().to_string(),
            progress: progress.clamp(0.0, 1.0),
            phase: if s.running { "روان" } else { "درنگ" }.to_string(),
            running: s.running,
            work_done: s.work_done,
        }
    }

    /// True while in the Work phase and running (used to drive inhibit).
    pub fn should_inhibit(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.running && s.phase == Phase::Work
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
        }
    }

    #[test]
    fn starts_paused_on_work_25min() {
        let s = Shared::new(config());
        let snap = s.snapshot();
        assert_eq!(snap.time, "25:00");
        assert_eq!(snap.session, "کار ژرف");
        assert_eq!(snap.phase, "درنگ");
        assert!(!snap.running);
    }

    #[test]
    fn toggle_starts_and_pauses_without_losing_remaining() {
        let s = Shared::new(config());
        s.toggle();
        assert!(s.snapshot().running);
        // force remaining down artificially
        {
            let mut st = s.state.lock().unwrap();
            st.remaining = Duration::from_secs(60);
        }
        s.toggle();
        assert!(!s.snapshot().running);
        assert_eq!(s.snapshot().time, "01:00");
    }

    #[test]
    fn advance_to_short_break_then_back_to_focus() {
        let s = Shared::new(config());
        let n = s.advance(true).unwrap();
        assert!(n.contains("به پایان رسید"));
        assert_eq!(s.snapshot().session, "آسایش کوتاه");
        let n = s.advance(false).unwrap();
        assert!(n.contains("آسایش به سر رسید"));
        assert_eq!(s.snapshot().session, "کار ژرف");
    }

    #[test]
    fn long_break_every_nth_work_session() {
        let s = Shared::new(config());
        // work sessions 1..3 -> short breaks (loop runs i=1,2,3)
        for i in 1..4 {
            s.advance(true);
            assert_eq!(s.snapshot().session, "آسایش کوتاه", "work session {i}");
            s.advance(false);
        }
        // 4th work session -> long break
        s.advance(true);
        assert_eq!(s.snapshot().session, "آسایش دراز");
    }

    #[test]
    fn skip_preserves_work_counter() {
        let s = Shared::new(config());
        s.skip(); // skip a work session -> advance (increment work)
        assert_eq!(s.snapshot().session, "آسایش کوتاه");
        s.skip(); // skip the break -> back to focus
        assert_eq!(s.snapshot().session, "کار ژرف");
    }

    #[test]
    fn reset_returns_to_initial_state() {
        let s = Shared::new(config());
        s.toggle();
        s.advance(true);
        s.reset();
        let snap = s.snapshot();
        assert_eq!(snap.time, "25:00");
        assert_eq!(snap.session, "کار ژرف");
        assert!(!snap.running);
        assert!(!s.should_inhibit());
    }

    #[test]
    fn tick_fires_exactly_one_advance_at_zero() {
        let s = Shared::new(config());
        s.state.lock().unwrap().finish_at = Some(Instant::now() - Duration::from_secs(1));
        s.state.lock().unwrap().running = true;
        let msg = s.tick();
        assert!(msg.is_some());
        // already advanced; a second tick should not re-advance
        let msg2 = s.tick();
        assert!(msg2.is_none());
        assert_eq!(s.snapshot().session, "آسایش کوتاه");
    }

    #[test]
    fn should_inhibit_only_when_work_running() {
        let s = Shared::new(config());
        assert!(!s.should_inhibit()); // paused
        s.toggle(); // work running
        assert!(s.should_inhibit());
        s.skip(); // -> short break (running)
        assert!(!s.should_inhibit());
    }
}
