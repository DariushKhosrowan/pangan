//! Application shell: wiring, event loop, and the glue between the pure
//! modules (`timer`, `state`, `sync`, `calendar`, `stats`, `audio`) and the
//! Slint UI.
//!
//! [`init`] holds the startup path and the UI event loop; [`automation`] runs
//! the user's shell hooks on timer events.

pub mod automation;
pub mod init;
pub mod mobile;

/// Start the application. Returns only when the user quits.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    init::run()
}

/// Start the mobile host. The platform entry points call this after
/// installing their native backend and platform adapter.
pub fn run_mobile(root: std::path::PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    mobile::run_hosted(root)
}
