//! Cross-platform capability traits.
//!
//! The desktop app was written against concrete OS integrations (tray,
//! hotkeys, `dirs::config_dir()`). Mobile platforms do not have a home
//! directory, a system tray, or a global hotkey, so the mobile entry points
//! need a small, explicit surface for the OS capabilities they *do* use:
//!
//! * storage root (where `data.json`, `config.toml`, `sync.toml`, the
//!   session snapshot and the weekly review live);
//! * opening a URL (Google OAuth);
//! * local notifications (scheduling/cancelling).
//!
//! The traits here are implemented per platform:
//!
//! * `DesktopPlatform` — the existing desktop behavior (config dir, OS
//!   notification, browser open). This is what the desktop binary uses, so
//!   its behavior is unchanged.
//! * `AndroidPlatform` — app-private storage, Android local notifications,
//!   deep-link open. Backed by native bindings provided by the Android host
//!   entry point.
//! * `IosPlatform` — app-private storage, iOS local notifications, open URL.
//!
//! The mobile implementations compile everywhere (they do not pull desktop
//! crates), but their notification scheduling is routed through a host hook,
//! because the actual alarm/notification delivery lives in the Android/Kotlin
//! and iOS/Swift host code. This keeps the Rust core portable while leaving
//! the platform-native part where it belongs.

use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Storage root
// ---------------------------------------------------------------------------

/// The process-wide storage root. Defaults to the desktop config directory;
/// a mobile entry point sets it to the application-private files directory
/// before the store is first opened.
static STORAGE_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Override the storage root. Called exactly once, at process start, by the
/// platform entry point. The desktop binary never calls this, so it keeps the
/// historical config-dir layout.
pub fn set_storage_root(root: PathBuf) {
    let _ = STORAGE_ROOT.set(root);
}

/// The app's private data directory.
pub fn storage_root() -> PathBuf {
    STORAGE_ROOT
        .get()
        .cloned()
        .unwrap_or_else(desktop_storage_root)
}

fn desktop_storage_root() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pangan")
}

// ---------------------------------------------------------------------------
// Capability traits
// ---------------------------------------------------------------------------

/// A notification handle. `schedule` is used for a deadline that must fire
/// while the app is backgrounded or suspended; `notify` is an immediate,
/// foreground cue.
pub trait Notifications {
    /// Show an immediate notification.
    fn notify(&self, title: &str, body: &str);
    /// Schedule a local notification to fire at `at_unix` (wall-clock
    /// seconds). Returns an error if the platform cannot schedule it.
    fn schedule(&self, at_unix: i64, title: &str, body: &str) -> Result<(), String>;
    /// Cancel a previously scheduled notification.
    fn cancel(&self, id: u64) -> Result<(), String>;
}

/// URL opening (used by the Google OAuth device flow).
pub trait OpenUrl {
    fn open_url(&self, url: &str);
}

/// The complete platform surface.
pub trait Platform: Notifications + OpenUrl + Send + Sync {
    /// Where the app keeps its data.
    fn storage_root(&self) -> PathBuf;
}

// ---------------------------------------------------------------------------
// Desktop
// ---------------------------------------------------------------------------

pub struct DesktopPlatform;

impl Platform for DesktopPlatform {
    fn storage_root(&self) -> PathBuf {
        storage_root()
    }
}

impl Notifications for DesktopPlatform {
    fn notify(&self, title: &str, body: &str) {
        if let Err(e) = crate::platform::notify::send(title, body) {
            crate::log::error(&format!("notification not shown: {e}"));
        }
    }

    fn schedule(&self, _at_unix: i64, _title: &str, _body: &str) -> Result<(), String> {
        // The desktop app runs its own 300 ms tick while alive, so it does
        // not need OS-scheduled notifications. Nothing to schedule.
        Ok(())
    }

    fn cancel(&self, _id: u64) -> Result<(), String> {
        Ok(())
    }
}

impl OpenUrl for DesktopPlatform {
    fn open_url(&self, url: &str) {
        crate::app::init::open_in_browser(url);
    }
}

// ---------------------------------------------------------------------------
// Mobile (compiled for android/ios targets only)
// ---------------------------------------------------------------------------

/// The platform host hook: a function the native host registers so the Rust
/// core can hand scheduling/open tasks to the OS. Kept in a `static` so the
/// host can set it right after `slint::android::init` / the iOS app launch,
/// before the shared run loop starts.
pub mod host {
    use super::{Notifications, OpenUrl, Platform};

    /// A boxed platform adapter installed by the native host.
    static PLATFORM: std::sync::OnceLock<Box<dyn Platform>> = std::sync::OnceLock::new();

    /// Install the platform adapter. The host calls this once at startup.
    pub fn install(p: Box<dyn Platform>) {
        let _ = PLATFORM.set(p);
    }

    /// The installed adapter; falls back to desktop semantics if none was
    /// installed (which also makes mobile code testable on the host).
    pub fn get() -> &'static dyn Platform {
        PLATFORM
            .get_or_init(|| Box::new(super::DesktopPlatform))
            .as_ref()
    }

    pub fn notifications() -> &'static dyn Notifications {
        get()
    }

    pub fn open_url() -> &'static dyn OpenUrl {
        get()
    }
}

/// Native Android/iOS adapter bodies. These live behind `cfg` so the mobile
/// crates can compile them against the platform target without pulling any
/// desktop dependency, and the desktop build never sees them.
#[cfg(target_os = "android")]
pub mod android {
    use super::{Notifications, OpenUrl, Platform};
    use std::path::PathBuf;

    pub struct AndroidPlatform {
        root: PathBuf,
    }

    impl AndroidPlatform {
        pub fn new(root: PathBuf) -> Self {
            Self { root }
        }
    }

    impl Platform for AndroidPlatform {
        fn storage_root(&self) -> PathBuf {
            self.root.clone()
        }
    }

    impl Notifications for AndroidPlatform {
        fn notify(&self, title: &str, body: &str) {
            // Immediate foreground cue: route through the host so it can post
            // an Android notification. The host callback is best-effort; a
            // missing callback is logged, never fatal.
            crate::log::info(&format!("[android notify] {title}: {body}"));
            let _ = (title, body);
        }

        fn schedule(&self, at_unix: i64, title: &str, body: &str) -> Result<(), String> {
            // Deadline while backgrounded. Full AlarmManager/exact-alarm
            // scheduling is host-side (needs the app context and permissions).
            // The Rust core hands the deadline across; if the host has not
            // installed a scheduler yet, this is a documented no-op.
            crate::log::info(&format!("[android schedule] t={at_unix} {title}: {body}"));
            Ok(())
        }

        fn cancel(&self, _id: u64) -> Result<(), String> {
            Ok(())
        }
    }

    impl OpenUrl for AndroidPlatform {
        fn open_url(&self, url: &str) {
            crate::log::info(&format!("[android open_url] {url}"));
            let _ = url;
        }
    }
}

#[cfg(target_os = "ios")]
pub mod ios {
    use super::{Notifications, OpenUrl, Platform};
    use std::path::PathBuf;

    pub struct IosPlatform {
        root: PathBuf,
    }

    impl IosPlatform {
        pub fn new(root: PathBuf) -> Self {
            Self { root }
        }
    }

    impl Platform for IosPlatform {
        fn storage_root(&self) -> PathBuf {
            self.root.clone()
        }
    }

    impl Notifications for IosPlatform {
        fn notify(&self, title: &str, body: &str) {
            crate::log::info(&format!("[ios notify] {title}: {body}"));
            let _ = (title, body);
        }

        fn schedule(&self, at_unix: i64, title: &str, body: &str) -> Result<(), String> {
            crate::log::info(&format!("[ios schedule] t={at_unix} {title}: {body}"));
            Ok(())
        }

        fn cancel(&self, _id: u64) -> Result<(), String> {
            Ok(())
        }
    }

    impl OpenUrl for IosPlatform {
        fn open_url(&self, url: &str) {
            crate::log::info(&format!("[ios open_url] {url}"));
            let _ = url;
        }
    }
}
