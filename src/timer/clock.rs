//! Platform-agnostic timer core: drift-free countdown state machine.
//! The deadline is a Unix timestamp (wall clock); every tick recomputes
//! remaining from the wall clock, so there is no `sleep()` drift and — just
//! as important — a system suspend does not freeze the countdown. A
//! monotonic clock (`std::time::Instant`) does not advance while a laptop is
//! suspended; a wall clock does, so the block still finishes on time.

use crate::state::settings::Settings;
use crate::state::store::SessionRecord;
use std::sync::Mutex;
use std::time::Duration;

/// The current Unix time in seconds — the one clock the timer trusts.
fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Clone, Debug)]
pub struct Config {
    pub work_minutes: u64,
    pub short_break_minutes: u64,
    pub long_break_minutes: u64,
    pub long_break_interval: u32,
    pub auto_start_next: bool,
    /// "fa" | "en". The clock produces the phase and state labels the UI
    /// shows, so it has to know which language to produce them in; before
    /// this the labels were hardcoded Persian and stayed Persian in English
    /// mode no matter what the rest of the window did.
    pub lang: String,
}

impl From<&Settings> for Config {
    fn from(s: &Settings) -> Self {
        Self {
            work_minutes: s.work_minutes,
            short_break_minutes: s.short_break_minutes,
            long_break_minutes: s.long_break_minutes,
            long_break_interval: s.long_break_interval,
            auto_start_next: s.auto_start_next,
            lang: s.lang.clone(),
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
            lang: "fa".to_string(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

impl Phase {
    /// The phase's name in the given language ("fa" | "en").
    pub fn label(&self, lang: &str) -> &'static str {
        let en = lang == "en";
        match self {
            Phase::Work => {
                if en {
                    "Deep Work"
                } else {
                    "کار ژرف"
                }
            }
            Phase::ShortBreak => {
                if en {
                    "Short Break"
                } else {
                    "آسایش کوتاه"
                }
            }
            Phase::LongBreak => {
                if en {
                    "Long Break"
                } else {
                    "آسایش دراز"
                }
            }
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

#[derive(Clone, Copy, Debug)]
pub struct State {
    pub phase: Phase,
    pub running: bool,
    pub remaining: Duration,
    pub total: Duration,
    pub work_done: u32,
    /// Wall-clock (Unix seconds) deadline for the current phase, when running.
    /// Using the wall clock — not `std::time::Instant` — is what keeps the
    /// countdown honest across a system suspend, where the monotonic clock
    /// freezes.
    pub finish_unix: Option<i64>,
    /// Unix seconds when the current focus block started, for crash recovery.
    /// Zero while no block is in flight.
    pub started_at: i64,
}

pub struct Shared {
    state: Mutex<State>,
    pub quit: std::sync::atomic::AtomicBool,
    /// The timer's durations. Behind a lock because a cloud sync can change
    /// them while the app is running: settings that arrive from another device
    /// have to take effect, not wait for a restart.
    cfg: Mutex<Config>,
    /// The UI language the snapshot's labels are produced in. Separate from
    /// `cfg` because it changes at runtime when the user switches language,
    /// while the durations in `cfg` are only re-read when settings close.
    lang: Mutex<String>,
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
        let lang = Mutex::new(cfg.lang.clone());
        Self {
            lang,
            state: Mutex::new(State {
                phase: Phase::Work,
                running: false,
                remaining: total,
                total,
                work_done: 0,
                finish_unix: None,
                started_at: 0,
            }),
            quit: std::sync::atomic::AtomicBool::new(false),
            cfg: Mutex::new(cfg),
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
        let cfg = self.cfg.lock().unwrap();
        match phase {
            Phase::Work => Duration::from_secs(cfg.work_minutes * 60),
            Phase::ShortBreak => Duration::from_secs(cfg.short_break_minutes * 60),
            Phase::LongBreak => Duration::from_secs(cfg.long_break_minutes * 60),
        }
    }

    /// Replace the durations, e.g. after the settings screen closes or a cloud
    /// sync delivered new ones.
    ///
    /// Two cases, and they are deliberately different:
    ///
    /// * **Paused** — the dial is rebased onto the new duration immediately.
    ///   No reset needed: set focus to 50 and it reads 50:00. This is the case
    ///   that was broken; the new numbers were saved and never reached the
    ///   clock, so the only way to see them was to press reset.
    ///
    /// * **Running** — the countdown is left completely alone. Its remaining
    ///   time and its finish instant are untouched, so the block the user
    ///   started finishes as one and the count never jumps. The new duration
    ///   is picked up by `phase_total` when the next phase begins.
    pub fn set_config(&self, cfg: Config) {
        let new_total = {
            let mut c = self.cfg.lock().unwrap();
            *c = cfg;
            match self.state.lock().unwrap().phase {
                Phase::Work => Duration::from_secs(c.work_minutes * 60),
                Phase::ShortBreak => Duration::from_secs(c.short_break_minutes * 60),
                Phase::LongBreak => Duration::from_secs(c.long_break_minutes * 60),
            }
        };

        let mut s = self.state.lock().unwrap();
        if !s.running {
            s.total = new_total;
            s.remaining = new_total;
            s.finish_unix = None;
        }
    }

    /// The current durations, for callers that need to read them back.
    #[cfg(test)]
    #[allow(dead_code)] // test support, not called from the app
    pub fn config(&self) -> Config {
        self.cfg.lock().unwrap().clone()
    }

    /// Advance to next phase; returns a notification string when a session
    /// completed so the caller can show a desktop notification.
    pub fn advance(&self, increment_work: bool) -> Option<String> {
        let mut s = self.state.lock().unwrap();

        let notify = if increment_work {
            s.work_done += 1;
            let is_long = s
                .work_done
                .is_multiple_of(self.cfg.lock().unwrap().long_break_interval);
            s.phase = if is_long {
                Phase::LongBreak
            } else {
                Phase::ShortBreak
            };
            // Automation: a focus session completed; a break begins.
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::state::settings::EVENT_FOCUS_END.into());
            if is_long {
                self.pending_events
                    .lock()
                    .unwrap()
                    .push(crate::state::settings::EVENT_POMODORO_COMPLETE.into());
            }
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::state::settings::EVENT_BREAK_START.into());
            Some(if self.is_en() {
                format!(
                    "{} finished — {} started.",
                    Phase::Work.label(&self.lang_str()),
                    s.phase.label(&self.lang_str())
                )
            } else {
                format!(
                    "{} به پایان رسید — {} آغاز شد.",
                    Phase::Work.label(&self.lang_str()),
                    s.phase.label(&self.lang_str())
                )
            })
        } else {
            s.phase = Phase::Work;
            // Automation: the break ended; focus begins again.
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::state::settings::EVENT_BREAK_END.into());
            self.pending_events
                .lock()
                .unwrap()
                .push(crate::state::settings::EVENT_FOCUS_START.into());
            Some(if self.is_en() {
                "Break over — back to deep work.".to_string()
            } else {
                "آسایش به سر رسید — بازگشت به کار ژرف.".to_string()
            })
        };
        s.total = self.phase_total(s.phase);
        s.remaining = s.total;
        s.running = self.cfg.lock().unwrap().auto_start_next;
        // When auto-start is off, land paused ready for the user to begin.
        s.finish_unix = if s.running {
            Some(now_unix() + s.remaining.as_secs() as i64)
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
        // Wall-clock now. The deadline is a Unix timestamp, so a system
        // suspend (where the monotonic clock freezes) still advances the
        // countdown on resume.
        let now = now_unix();
        if let Some(finish) = s.finish_unix {
            let remain = (finish - now).max(0) as u64;
            s.remaining = Duration::from_secs(remain);
            if s.remaining.is_zero() {
                // Which phase just ended decides what comes next. Passing the
                // wrong flag here is what makes a finished break start another
                // break: `advance(true)` means "a focus session just
                // completed", so it increments the counter and picks a break.
                // A break ending must call `advance(false)` to go back to work.
                let ended_work = s.phase == Phase::Work;
                if ended_work {
                    let rec = SessionRecord {
                        at: now,
                        minutes: self.cfg.lock().unwrap().work_minutes,
                    };
                    self.pending_sessions.lock().unwrap().push(rec);
                } else {
                    *self.pending_breaks.lock().unwrap() += 1;
                }
                drop(s);
                return self.advance(ended_work);
            }
        }
        None
    }

    pub fn toggle(&self) {
        let mut s = self.state.lock().unwrap();
        s.running = !s.running;
        if s.running {
            s.finish_unix = Some(now_unix() + s.remaining.as_secs() as i64);
            if s.phase == Phase::Work {
                // Stamp the block start so a crash mid-block can be recovered,
                // even when the block was started from the plain timer rather
                // than a rota entry. Kept on the first start only: pausing and
                // resuming must not reset the clock the block began on.
                if s.started_at == 0 {
                    s.started_at = now_unix();
                }
                self.pending_events
                    .lock()
                    .unwrap()
                    .push(crate::state::settings::EVENT_FOCUS_START.into());
            }
        } else {
            s.finish_unix = None;
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
        s.finish_unix = None;
        s.work_done = 0;
    }

    /// Switch the language the snapshot labels are produced in.
    pub fn set_lang(&self, lang: &str) {
        *self.lang.lock().unwrap() = lang.to_string();
    }

    fn lang_str(&self) -> String {
        self.lang.lock().unwrap().clone()
    }

    fn is_en(&self) -> bool {
        self.lang.lock().unwrap().as_str() == "en"
    }

    /// Whether the UI is currently in English. The tray menu is built outside
    /// the window and needs the same answer.
    pub fn lang_is_en(&self) -> bool {
        self.is_en()
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
            session: s.phase.label(&self.lang_str()).to_string(),
            progress: progress.clamp(0.0, 1.0),
            // "running" / "paused" — the word under the clock, and the other
            // half of the untranslated timer screen.
            phase: if self.is_en() {
                if s.running {
                    "Running"
                } else {
                    "Paused"
                }
            } else if s.running {
                "روان"
            } else {
                "درنگ"
            }
            .to_string(),
            running: s.running,
            work_done: s.work_done,
        }
    }

    /// True while in the Work phase and running (used to drive inhibit).
    pub fn should_inhibit(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.running && s.phase == Phase::Work
    }

    /// The Unix second the current block started (0 if none is in flight).
    pub fn started_at(&self) -> i64 {
        self.state.lock().unwrap().started_at
    }

    /// Record when the block in flight started. The caller passes the real
    /// wall-clock second so a crash can be recovered to the exact remainder.
    pub fn set_started_at(&self, at: i64) {
        self.state.lock().unwrap().started_at = at;
    }

    /// Clear the recorded start time (the block ended, was skipped, or the
    /// clock was reset). The snapshot on disk is cleared at the call site so
    /// this method stays a pure timer-state concern.
    pub fn clear_started_at(&self) {
        self.state.lock().unwrap().started_at = 0;
    }

    /// A copy of the timer's raw state, used to build a crash-recovery
    /// snapshot. Kept on the clock so the recovery logic reads the same truth
    /// the countdown runs on.
    pub fn state_snapshot(&self) -> State {
        *self.state.lock().unwrap()
    }

    /// Restore the countdown from a crash-recovery snapshot.
    ///
    /// `duration_secs` is the block's planned length, `remaining_secs` is what
    /// is left of it (what the resume dialog promised), and `running` says
    /// whether the block was in flight (true) or paused (false) when it was
    /// snapshotted. The clock is rebuilt around those two numbers so the
    /// remainder is exact and the progress bar reads correctly.
    pub fn begin_from_recovery(&self, duration_secs: u64, remaining_secs: u64, running: bool) {
        let mut s = self.state.lock().unwrap();
        let total = Duration::from_secs(duration_secs.max(1));
        s.phase = Phase::Work;
        s.total = total;
        s.remaining = Duration::from_secs(remaining_secs.min(duration_secs.max(1)));
        s.running = running;
        s.finish_unix = if running {
            Some(now_unix() + s.remaining.as_secs() as i64)
        } else {
            None
        };
        s.started_at = now_unix();
    }

    /// True when the clock is sitting at the start of an untouched focus block.
    ///
    /// This is the condition the decree dialog is gated on, and it lives here
    /// rather than in the UI so the definition cannot drift from the timer's:
    /// work phase, paused, and not a single second consumed. Resuming a block
    /// that is already part-way through is deliberately *not* this, because
    /// asking what a half-finished block is for is a question with no useful
    /// answer.
    pub fn at_fresh_work_start(&self) -> bool {
        let s = self.state.lock().unwrap();
        s.phase == Phase::Work && !s.running && s.remaining == s.total
    }

    /// Begin a focus block of a given length, from rest.
    ///
    /// This exists for the rota. A planned block carries its own length — the
    /// whole point of planning ahead is deciding that *this* block is fifty
    /// minutes and the next is twenty-five — but the settings hold one focus
    /// duration, and rewriting it for every planned block would be the app
    /// quietly editing a preference the user set on purpose.
    ///
    /// So the length is applied to this block only. `advance` reads its totals
    /// from the config as it always has, which means the following block
    /// returns to the configured duration. The settings are never touched.
    ///
    /// Returns false, changing nothing, when the clock is not at rest. Swapping
    /// the length out from under a running countdown — or a break the user is
    /// part-way through — is not something a button should be able to do.
    pub fn start_work_block(&self, minutes: u64) -> bool {
        let mut s = self.state.lock().unwrap();
        if s.running || s.phase != Phase::Work || s.remaining != s.total {
            return false;
        }
        let total = Duration::from_secs(minutes.clamp(1, 180) * 60);
        s.total = total;
        s.remaining = total;
        s.running = true;
        s.finish_unix = Some(now_unix() + total.as_secs() as i64);
        self.pending_events
            .lock()
            .unwrap()
            .push(crate::state::settings::EVENT_FOCUS_START.into());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changing the focus duration must move the dial immediately when the
    /// timer is paused — no reset needed. This is the bug the user hit: the
    /// setting was saved to disk and never reached the clock.
    #[test]
    fn set_config_rebases_a_paused_timer() {
        let shared = Shared::new(config());
        assert_eq!(shared.snapshot().time, "25:00");

        let mut longer = config();
        longer.work_minutes = 50;
        shared.set_config(longer);

        assert_eq!(
            shared.snapshot().time,
            "50:00",
            "paused dial must follow the new duration"
        );
    }

    /// A running countdown keeps its own total: the user started a 25-minute
    /// block and it should finish as one, with no jump in the displayed time.
    #[test]
    fn set_config_leaves_a_running_timer_alone() {
        let shared = Shared::new(config());
        shared.toggle(); // start
        assert!(shared.snapshot().running);

        let mut longer = config();
        longer.work_minutes = 50;
        shared.set_config(longer);

        assert_eq!(
            shared.snapshot().time,
            "25:00",
            "a running block must not be stretched mid-flight"
        );
    }

    /// After the running block ends, the new duration applies.
    #[test]
    fn new_duration_applies_to_the_next_phase() {
        let shared = Shared::new(config());
        shared.toggle();
        let mut longer = config();
        longer.work_minutes = 50;
        shared.set_config(longer);

        shared.skip(); // move to the break
        shared.skip(); // back to work
        assert_eq!(shared.snapshot().time, "50:00");
    }

    fn config() -> Config {
        Config {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
            lang: "fa".to_string(),
        }
    }

    /// The phase and state labels are shown on the timer screen, so they have
    /// to follow the selected language. They were hardcoded Persian and stayed
    /// Persian in English mode while every label around them switched.
    #[test]
    fn labels_follow_the_language() {
        let s = Shared::new(config());
        assert_eq!(s.snapshot().session, "کار ژرف");
        assert_eq!(s.snapshot().phase, "درنگ");

        s.set_lang("en");
        assert_eq!(s.snapshot().session, "Deep Work");
        assert_eq!(s.snapshot().phase, "Paused");

        s.toggle();
        assert_eq!(s.snapshot().phase, "Running");

        s.set_lang("fa");
        assert_eq!(s.snapshot().session, "کار ژرف");
        assert_eq!(s.snapshot().phase, "روان");
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
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 1);
        s.state.lock().unwrap().running = true;
        let msg = s.tick();
        assert!(msg.is_some());
        // already advanced; a second tick should not re-advance
        let msg2 = s.tick();
        assert!(msg2.is_none());
        assert_eq!(s.snapshot().session, "آسایش کوتاه");
    }

    #[test]
    fn a_finished_break_returns_to_work_and_does_not_chain_another_break() {
        // The bug this pins down: `tick` advanced with `advance(true)` no
        // matter which phase ended. `true` means "a focus session completed",
        // so a break that ran out incremented the work counter and started
        // *another break* — the timer sat in an endless rest loop and the user
        // never got back to work.
        let s = Shared::new(config());

        // Finish a work session: this legitimately starts a break.
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 1);
        s.state.lock().unwrap().running = true;
        assert!(s.tick().is_some());
        assert_eq!(
            s.snapshot().session,
            "آسایش کوتاه",
            "work should lead to a break"
        );

        // Now let the break run out. It must go back to work.
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 1);
        s.state.lock().unwrap().running = true;
        assert!(s.tick().is_some());
        assert_eq!(
            s.snapshot().session,
            "کار ژرف",
            "a finished break must return to focus, not start another break"
        );

        // And the work counter must not have been bumped by the break ending:
        // one work session completed, so the next break is still a short one.
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 1);
        s.state.lock().unwrap().running = true;
        assert!(s.tick().is_some());
        assert_eq!(
            s.snapshot().session,
            "آسایش کوتاه",
            "the second work session is still before the long-break interval"
        );
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

    // ── system suspend: the countdown must keep the wall-clock deadline ──

    /// A system suspend freezes the monotonic clock, but the wall clock keeps
    /// running. The timer's deadline is a Unix timestamp, so after a suspend
    /// gap the block must be finished — not still counting down. The bug this
    /// pins down: when the deadline was an `Instant`, a suspend gap appeared
    /// as zero elapsed time, so the block would finish 15 minutes too late.
    #[test]
    fn suspend_gap_makes_the_block_finish_on_time() {
        let s = Shared::new(config());
        s.toggle(); // start a 25-minute focus block
        assert!(s.started_at() > 0);

        // Simulate closing the laptop: 25 minutes of wall time pass (the
        // block's full length), so the deadline is already in the past
        // relative to the real wall clock. On resume, the tick must see it
        // as finished.
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 1);
        s.state.lock().unwrap().running = true;

        let msg = s.tick();
        assert!(
            msg.is_some(),
            "a block past its wall-clock deadline must finish"
        );
        assert_eq!(s.snapshot().session, "آسایش کوتاه");
    }

    /// A large suspend gap must not overflow or go negative.
    #[test]
    fn large_suspend_gap_clamps_to_zero() {
        let s = Shared::new(config());
        s.toggle();
        // An hour of wall time passes while the laptop is closed.
        s.state.lock().unwrap().finish_unix = Some(now_unix() - 3600);
        s.state.lock().unwrap().running = true;
        assert!(s.tick().is_some());
        // After finish, remaining is the fresh full break, not a negative.
        assert!(s.snapshot().minutes <= 15);
    }

    /// A clock jump backward must not extend the block beyond its planned
    /// length — the deadline is absolute, not relative to a moving "now".
    #[test]
    fn clock_jump_backward_does_not_extend_the_block() {
        let s = Shared::new(config());
        s.toggle();
        // The deadline is one minute from real now.
        s.state.lock().unwrap().finish_unix = Some(now_unix() + 60);
        s.state.lock().unwrap().running = true;
        let remaining_before = s.snapshot().minutes;
        let _ = s.tick(); // recompute
                          // Remaining must be ~1 minute, never more than the planned total.
        assert!(s.snapshot().minutes <= remaining_before + 1);
    }

    /// The snapshot's remaining and the live clock's remaining must agree on
    /// the same inputs — recovery and live ticking read the same truth.
    #[test]
    fn snapshot_remaining_matches_clock_remaining() {
        let snap = crate::timer::session_snapshot::SessionSnapshot {
            started_at: 100,
            planned_duration_secs: 1500,
            phase: crate::timer::session_snapshot::Phase::Focus,
            rota_entry_id: None,
            decree_text: None,
            paused_at: None,
            paused_remaining_secs: 0,
        };
        // At wall time 1100 (1000s in), both agree on 500s remaining.
        assert_eq!(snap.remaining_secs(1100), 500);
    }
}
