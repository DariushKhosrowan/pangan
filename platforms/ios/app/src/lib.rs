//! Pangan iOS entry point.
//!
//! This crate builds a `staticlib` that the Xcode project links into the
//! iOS app. The Swift host (`../ios`) drives the UIKit app lifecycle and
//! hands the Rust core the app-private storage directory. The same unchanged
//! Slint interface (`ui/appwindow.slint`) is hosted via the Slint winit
//! backend on the `UIApplicationMain` thread.

use pangan::platform::caps::host;
use pangan::platform::caps::ios::IosPlatform;

/// Configure the platform adapter and start the shared mobile host.
///
/// Called from Swift once the app has a valid files directory. `root` is the
/// application support directory that Pangan should keep its data in.
///
/// This returns only when the UI event loop ends.
#[no_mangle]
pub extern "C" fn pangan_ios_run(root: *const std::os::raw::c_char) -> i32 {
    let root = unsafe {
        if root.is_null() {
            None
        } else {
            std::ffi::CStr::from_ptr(root)
                .to_str()
                .ok()
                .map(String::from)
        }
    };
    let Some(root) = root else {
        return -1;
    };

    let platform = IosPlatform::new(std::path::PathBuf::from(&root));
    host::install(Box::new(platform));

    match pangan::app::run_mobile(std::path::PathBuf::from(root)) {
        Ok(()) => 0,
        Err(e) => {
            pangan::log::error(&format!("ios run error: {e}"));
            1
        }
    }
}

/// Minimal entry point so the .app bundle executable is self-contained: it
/// locates the app's Documents directory and starts the shared host. The
/// real app is expected to pass the storage root via [`pangan_ios_run`];
/// this `main` is only a fallback so the simulator bundle can actually launch.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn main() -> i32 {
    // Best-effort Documents path. Cocoa is the source of truth; this is a
    // safe default for a developer build.
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let root = std::path::PathBuf::from(home)
        .join("Documents")
        .join("pangan");
    let c = std::ffi::CString::new(root.to_str().unwrap_or("/tmp/pangan")).unwrap();
    pangan_ios_run(c.as_ptr())
}
