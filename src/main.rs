//! Pangan desktop binary.
//!
//! Thin entry point over the `pangan` library crate; all application logic
//! lives in the library so platform entry points (Android, iOS) reuse the
//! same core.

fn main() {
    pangan::log::init();
    pangan::log::info("pangan starting");
    if let Err(e) = pangan::app::run() {
        pangan::log::error(&format!("fatal: {e}"));
        eprintln!("pangan: fatal: {e}");
        std::process::exit(1);
    }
}
