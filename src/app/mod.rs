//! Application shell: wiring, event loop, and the glue between the pure
//! modules (`timer`, `state`, `sync`, `calendar`, `stats`, `audio`) and the
//! Slint UI.
//!
//! [`init`] holds the startup path and the UI event loop; [`automation`] runs
//! the user's shell hooks on timer events.

pub mod automation;
pub mod init;

/// Start the application. Returns only when the user quits.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    init::run()
}
