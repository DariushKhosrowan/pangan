//! User settings — the full, persistent preference model.
//!
//! Everything the user can tune lives here and is persisted as TOML at
//! `~/.config/pangan/config.toml` (Linux), `%APPDATA%\pangan\config.toml`
//! (Windows) or `~/Library/Application Support/pangan/config.toml` (macOS).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const THEME_SYSTEM: &str = "system";
pub const THEME_DARK: &str = "dark";
pub const THEME_LIGHT: &str = "light";

pub const SOUND_SANTUR: &str = "santur";
pub const SOUND_DAF: &str = "daf";
pub const SOUND_TOMBAK: &str = "tombak";
pub const SOUND_KARN: &str = "karn";
pub const SOUND_SAND: &str = "sand";
pub const SOUND_CYRUS: &str = "cyrus";

pub const SOUND_THEMES: [&str; 6] = [
    SOUND_SANTUR,
    SOUND_DAF,
    SOUND_TOMBAK,
    SOUND_KARN,
    SOUND_SAND,
    SOUND_CYRUS,
];

// --- Automation events (FocusTimer-class tools) ------------------------------
pub const EVENT_FOCUS_START: &str = "focus_start";
pub const EVENT_FOCUS_END: &str = "focus_end";
pub const EVENT_BREAK_START: &str = "break_start";
pub const EVENT_BREAK_END: &str = "break_end";
pub const EVENT_POMODORO_COMPLETE: &str = "pomodoro_complete";
pub const EVENT_ALL: [&str; 5] = [
    EVENT_FOCUS_START,
    EVENT_FOCUS_END,
    EVENT_BREAK_START,
    EVENT_BREAK_END,
    EVENT_POMODORO_COMPLETE,
];

/// A user-defined automation rule: run `command` when `event` fires.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AutomationAction {
    pub event: String,
    pub command: String,
    pub enabled: bool,
}

impl Default for AutomationAction {
    fn default() -> Self {
        Self {
            event: EVENT_FOCUS_START.into(),
            command: String::new(),
            enabled: true,
        }
    }
}

/// Default global-hotkey accelerators (GTK-style strings).
pub const DEFAULT_SHORTCUTS: [(&str, &str); 4] = [
    ("toggle", "<Super>p"),
    ("skip", "<Super>k"),
    ("reset", "<Super>r"),
    ("toggle_window", "<Super>w"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // --- Timer durations (minutes) ---
    pub work_minutes: u64,
    pub short_break_minutes: u64,
    pub long_break_minutes: u64,
    pub long_break_interval: u32,

    // --- Behaviour ---
    pub auto_start_next: bool,
    pub auto_start_on_launch: bool,
    pub daily_goal_minutes: u64,

    // --- Appearance ---
    pub theme: String,
    pub compact_mode: bool,

    // --- Sound ---
    pub sound_enabled: bool,
    pub sound_theme: String,
    pub tick_sound: bool,
    pub sound_volume: u8,

    // --- System integration ---
    pub hotkey_enabled: bool,
    pub inhibit_enabled: bool,
    pub overlay_enabled: bool,

    // --- Automation (FocusTimer-class tools) ---
    pub automation: Vec<AutomationAction>,

    // --- Global hotkeys (action -> accelerator) ---
    pub shortcuts: std::collections::BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        let mut shortcuts = std::collections::BTreeMap::new();
        for (action, accel) in DEFAULT_SHORTCUTS {
            shortcuts.insert(action.to_string(), accel.to_string());
        }
        Self {
            work_minutes: 25,
            short_break_minutes: 5,
            long_break_minutes: 15,
            long_break_interval: 4,
            auto_start_next: true,
            auto_start_on_launch: false,
            daily_goal_minutes: 120,
            theme: THEME_SYSTEM.into(),
            compact_mode: false,
            sound_enabled: true,
            sound_theme: SOUND_SANTUR.into(),
            tick_sound: false,
            sound_volume: 80,
            hotkey_enabled: true,
            inhibit_enabled: true,
            overlay_enabled: true,
            automation: Vec::new(),
            shortcuts,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        crate::store::Store::data_dir().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut s: Settings = toml::from_str(&text).unwrap_or_default();
        s.sanitize_mut();
        s
    }

    pub fn save(&self) {
        if let Some(dir) = Self::path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let header = "# Pangan settings (Dariush Khosrowan)\n";
        // Chrono/TOML won't preserve comments; write a plain serialized doc.
        let mut body = toml::to_string_pretty(self).unwrap_or_default();
        // TOML arrays of strings we don't use; strip empty fields for clarity.
        body = body.replace("sheet = \"\"", "").replace("token = \"\"", "");
        let _ = std::fs::write(Self::path(), format!("{header}{body}"));
    }

    /// Clamp values to sane ranges.
    pub fn sanitize_mut(&mut self) {
        self.work_minutes = self.work_minutes.clamp(1, 180);
        self.short_break_minutes = self.short_break_minutes.clamp(1, 60);
        self.long_break_minutes = self.long_break_minutes.clamp(1, 90);
        self.long_break_interval = self.long_break_interval.clamp(1, 12);
        self.daily_goal_minutes = self.daily_goal_minutes.clamp(15, 600);
        self.sound_volume = self.sound_volume.clamp(0, 100);
        if !SOUND_THEMES.contains(&self.sound_theme.as_str()) {
            self.sound_theme = SOUND_SANTUR.into();
        }
        if self.theme != THEME_DARK && self.theme != THEME_LIGHT && self.theme != THEME_SYSTEM {
            self.theme = THEME_SYSTEM.into();
        }
        // Merge in default shortcuts for any action missing an accelerator.
        for (action, accel) in DEFAULT_SHORTCUTS {
            self.shortcuts
                .entry(action.to_string())
                .or_insert_with(|| accel.to_string());
        }
        // Keep only valid automation rules; drop blank commands.
        self.automation.retain(|a| {
            a.enabled && !a.command.trim().is_empty() && EVENT_ALL.contains(&a.event.as_str())
        });
    }

    pub fn theme_label(&self) -> String {
        match self.theme.as_str() {
            THEME_DARK => "Dark".into(),
            THEME_LIGHT => "Light".into(),
            _ => "System".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.work_minutes, 25);
        assert_eq!(s.long_break_interval, 4);
        assert!(s.auto_start_next);
        assert_eq!(s.sound_theme, SOUND_SANTUR);
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings::default();
        s.work_minutes = 9999;
        s.sound_theme = "bogus".into();
        s.sanitize_mut();
        assert_eq!(s.work_minutes, 180);
        assert_eq!(s.sound_theme, SOUND_SANTUR);
    }
}
