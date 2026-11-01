//! Pangan Android entry point.
//!
//! This cdylib is the Android native activity. The Android Gradle project in
//! `..` loads it and hosts the `Slint` window, which is the *unchanged*
//! desktop interface (`ui/appwindow.slint` from the `pangan` library).
//!
//! The `android_activity` crate starts the app at `android_main`. We
//! initialize the Slint Android backend there, then hand off to the shared
//! mobile host, which sets the storage root, recovers any interrupted focus
//! block, and wires the countdown.

use pangan::platform::caps::android::AndroidPlatform;
use pangan::platform::caps::host;

/// The Android native-activity entry point (from `android_activity`).
#[unsafe(no_mangle)]
pub fn android_main(app: slint::android::AndroidApp) {
    // Initialize the Slint Android backend (event loop + rendering).
    if let Err(e) = slint::android::init(app) {
        pangan::log::error(&format!("slint android init failed: {e}"));
        return;
    }

    // Install the platform adapter: storage root is the app-private files dir.
    //
    // NOTE: the true app-private files dir is obtained from the Android
    // `Context` in the Kotlin host and passed across the FFI boundary before
    // `android_main` runs. Here we fall back to the crate's data dir if it
    // was not set, so a build without the Kotlin bridge still starts.
    let mut root = std::path::PathBuf::from(".");
    if let Ok(files) = std::env::var("PANGAN_ANDROID_FILES") {
        root = std::path::PathBuf::from(files);
    }
    let platform = AndroidPlatform::new(root.clone());
    host::install(Box::new(platform));

    let _ = pangan::app::run_mobile(root);
}
