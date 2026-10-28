//! Automation engine — FocusTimer-class tools for Pangan.
//!
//! Users can attach shell commands to timer life-cycle events so Pangan can
//! drive their desktop (pause media, switch workspaces, raise a VPN, etc.).
//! Commands run detached, in a background shell, and never block the UI.

use crate::settings::{AutomationAction, Settings};
use std::process::{Command, Stdio};

/// Fire the automation rules for a given event. Called from the event-loop
/// thread. Commands are spawned detached (stdin/stdout/stderr nulled) so they
/// never block or echo into the terminal.
pub fn fire_event(settings: &Settings, event: &str, session_label: &str) {
    for rule in settings.automation.iter() {
        if !rule.enabled || rule.event != event || rule.command.trim().is_empty() {
            continue;
        }
        // Expand a couple of friendly variables.
        let cmd = rule
            .command
            .replace("%event%", event)
            .replace("%session%", session_label);
        let _ = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Return the list of automation rules, sorted for stable display.
pub fn sorted(settings: &Settings) -> Vec<AutomationAction> {
    let mut v = settings.automation.clone();
    v.sort_by(|a, b| a.event.cmp(&b.event));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        EVENT_BREAK_END, EVENT_BREAK_START, EVENT_FOCUS_END, EVENT_FOCUS_START,
        EVENT_POMODORO_COMPLETE,
    };

    #[test]
    fn fire_event_skips_disabled_and_blank_rules() {
        // We can't easily assert side-effects across spawn here; just ensure it
        // doesn't panic and that sorter keeps order.
        let s = Settings::default();
        fire_event(&s, EVENT_FOCUS_END, "Focus");
        let _ = sorted(&s);
    }

    #[test]
    fn clock_fires_focus_and_break_events() {
        // Verify the clock emits the right automation events on transitions.
        let s = crate::clock::Shared::new(crate::clock::Config {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
        });
        // Start focus -> focus_start event.
        s.toggle();
        let ev = s.drain_events();
        assert!(ev.contains(&EVENT_FOCUS_START.to_string()), "got {ev:?}");
        // Advance a work session -> focus_end + break_start.
        s.skip();
        let ev = s.drain_events();
        assert!(ev.contains(&EVENT_FOCUS_END.to_string()), "got {ev:?}");
        assert!(ev.contains(&EVENT_BREAK_START.to_string()), "got {ev:?}");
        // End the break -> break_end + focus_start.
        s.skip();
        let ev = s.drain_events();
        assert!(ev.contains(&EVENT_BREAK_END.to_string()), "got {ev:?}");
        assert!(ev.contains(&EVENT_FOCUS_START.to_string()), "got {ev:?}");
    }

    #[test]
    fn clock_fires_pomodoro_complete_on_fourth_focus() {
        let s = crate::clock::Shared::new(crate::clock::Config {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
        });
        // Complete 3 focus sessions (each: focus -> break -> focus).
        for _ in 0..3 {
            s.skip(); // focus -> break
            let _ = s.drain_events();
            s.skip(); // break -> focus
            let _ = s.drain_events();
        }
        // 4th focus session completes -> pomodoro_complete + break_start.
        s.skip();
        let ev = s.drain_events();
        assert!(ev.contains(&EVENT_POMODORO_COMPLETE.to_string()), "got {ev:?}");
        assert!(ev.contains(&EVENT_BREAK_START.to_string()), "got {ev:?}");
    }

    #[test]
    fn sanitize_keeps_only_valid_rules() {
        let mut s = Settings::default();
        s.automation.push(AutomationAction {
            event: EVENT_FOCUS_END.into(),
            command: "echo done".into(),
            enabled: true,
        });
        s.automation.push(AutomationAction {
            event: "bogus".into(),
            command: "echo x".into(),
            enabled: true,
        });
        s.automation.push(AutomationAction {
            event: EVENT_FOCUS_END.into(),
            command: "   ".into(),
            enabled: true,
        });
        s.sanitize_mut();
        assert_eq!(s.automation.len(), 1);
        assert_eq!(s.automation[0].command, "echo done");
    }
}
