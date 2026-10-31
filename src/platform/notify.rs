//! Desktop notifications. Linux/macOS use `notify-rust`; Windows uses
//! `winrt-notification` (toast API).
//!
//! `send` returns what actually happened, so the app can tell the user when
//! the OS-level cue did not fire (no notification daemon, no permission, Do
//! Not Disturb) instead of failing silently and letting them miss the end of
//! a block.

/// Why a notification could not be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyError {
    /// Linux: no notification daemon on the session bus (e.g. `notify-osd`,
    /// `dunst`, `mako` not running).
    LinuxDbusUnavailable,
    /// macOS: the app has not been granted notification permission.
    /// Constructed only on the macOS target; a Linux build never builds it.
    #[allow(dead_code)]
    MacDenied,
    /// Windows: Focus Assist / quiet hours suppressed the toast.
    /// Constructed only on the Windows target.
    #[allow(dead_code)]
    WindowsFocusAssist,
    /// Any other failure (the underlying API returned an error).
    Other,
}

impl std::fmt::Display for NotifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifyError::LinuxDbusUnavailable => {
                write!(f, "notification daemon not available")
            }
            NotifyError::MacDenied => write!(f, "notification permission denied"),
            NotifyError::WindowsFocusAssist => {
                write!(f, "suppressed by Focus Assist")
            }
            NotifyError::Other => write!(f, "notification failed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Linux / macOS
// ---------------------------------------------------------------------------
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn send(title: &str, body: &str) -> Result<(), NotifyError> {
    let result = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        // Request urgency normal; not strictly required, but keeps us from
        // being lumped with critical alerts.
        .hint(notify_rust::Hint::Urgency(notify_rust::Urgency::Normal))
        .show();
    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            // A failed `.show()` almost always means no daemon is reachable on
            // the session bus. Distinguish that reason when we can.
            let msg = e.to_string();
            if msg.contains("org.freedesktop.Notifications")
                || msg.contains("dbus")
                || msg.contains("DBus")
            {
                Err(NotifyError::LinuxDbusUnavailable)
            } else {
                Err(NotifyError::Other)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
pub fn send(title: &str, body: &str) -> Result<(), NotifyError> {
    use winrt_notification::{Duration, Sound, Toast};
    let result = Toast::new("pangan.Pangan")
        .title(title)
        .text1(body)
        .duration(Duration::Short)
        .sound(Some(Sound::Default))
        .show();
    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(NotifyError::WindowsFocusAssist),
    }
}
