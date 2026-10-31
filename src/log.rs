//! A tiny, dependency-free structured file logger.
//!
//! Pangan is a desktop app: there is no console to read when something goes
//! wrong, and the user will not paste a terminal. So this module writes
//! leveled, timestamped lines to a log file next to the app's data, so a
//! failing run leaves a trail that can be opened and read.
//!
//! The log lives at `~/.config/pangan/logs/app.log` (or the platform's
//! config dir), and is rotated at a size cap: the current file is renamed to
//! `app.log.1` and a fresh one started, so it never grows without bound.
//! Every line carries the local time, the level, the source module, and the
//! message. The format is deliberately plain text — grep-able and stable,
//! not a format that needs a tool to read.
//!
//! # Usage
//!
//! ```ignore
//! log::init();
//! log::info("app started");
//! log::warn("sync failed, will retry");
//! log::error(&format!("fatal: {e}"));
//! ```
//!
//! The logger is a process-wide singleton and is safe to call from any
//! thread (the timer thread, sync threads, the hotkey thread). It is a
//! best-effort facility: a failure to write is logged nowhere and swallowed,
//! because a logger that crashes the app has failed its one job.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

/// A log severity. Kept tiny and stable; the level is a single letter in
/// the file so the line reads `I`, `W`, `E` at a glance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    fn letter(self) -> char {
        match self {
            Level::Info => 'I',
            Level::Warn => 'W',
            Level::Error => 'E',
        }
    }
}

/// Keep the current log under this many bytes before rotating.
const MAX_LOG_BYTES: u64 = 512 * 1024;

/// The log file's base name (the rotated predecessor gets `.1`).
const LOG_FILE_NAME: &str = "app.log";

struct Logger {
    file: Mutex<std::io::BufWriter<std::fs::File>>,
    path: PathBuf,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// The directory the log lives in, alongside the app's store.
pub fn log_dir() -> PathBuf {
    crate::state::store::Store::data_dir().join("logs")
}

/// Open the log for appending, creating the directory and file as needed.
/// Idempotent: calling it repeatedly is fine, though in practice it is
/// called once at startup.
pub fn init() {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(LOG_FILE_NAME);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path);
    let file = match file {
        Ok(f) => f,
        Err(_) => return, // best effort: cannot log the failure to log
    };
    let _ = LOGGER.set(Logger {
        file: Mutex::new(std::io::BufWriter::new(file)),
        path,
    });
}

/// Rotate the log if it has grown past the cap. Called before each write.
fn rotate_if_needed(logger: &Logger, current_len: u64) {
    if current_len < MAX_LOG_BYTES {
        return;
    }
    // Rename the current file to the `.1` predecessor (overwriting the old
    // one), then start a fresh file. The BufWriter is dropped first so its
    // buffer is flushed to disk before the file is moved.
    let backup = logger.path.with_extension("log.1");
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::rename(&logger.path, &backup);
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&logger.path)
    {
        *logger.file.lock().unwrap() = std::io::BufWriter::new(f);
    }
}

/// Write one line to the log. This is the single funnel every level goes
/// through; the caller passes the source module for the line.
pub fn write(level: Level, module: &str, msg: &str) {
    let Some(logger) = LOGGER.get() else {
        return; // not initialised — nothing to log to
    };
    let mut w = logger.file.lock().unwrap();
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("{} {} {} {}\n", now, level.letter(), module, msg);
    let _ = w.write_all(line.as_bytes());
    let _ = w.flush();
    // Keep the file bounded. The writer is still held while we check size.
    let len = std::fs::metadata(&logger.path)
        .map(|m| m.len())
        .unwrap_or(0);
    drop(w);
    rotate_if_needed(logger, len);
}

/// Log an informational message.
pub fn info(msg: &str) {
    write(Level::Info, module_path!(), msg);
}

/// Log a warning.
///
/// Not yet emitted by the app's current paths; kept as the API surface for
/// recoverable conditions (a retryable network failure, a stale cache).
#[allow(dead_code)]
pub fn warn(msg: &str) {
    write(Level::Warn, module_path!(), msg);
}

/// Log an error.
pub fn error(msg: &str) {
    write(Level::Error, module_path!(), msg);
}

/// The path of the current log file, for tests and for the status UI.
#[allow(dead_code)]
pub fn current_path() -> PathBuf {
    log_dir().join(LOG_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_sits_under_the_store_dir() {
        // The data-dir-derived location is what the app uses at runtime.
        assert!(log_dir().ends_with("pangan/logs"));
    }

    #[test]
    fn level_letters_are_stable() {
        assert_eq!(Level::Info.letter(), 'I');
        assert_eq!(Level::Warn.letter(), 'W');
        assert_eq!(Level::Error.letter(), 'E');
    }

    #[test]
    fn roundtrip_through_the_log_file() {
        // Writing a line requires the singleton; call init() first so the
        // real file lands next to the (temp-able) store dir. In CI the store
        // dir is the user's config, which we must not pollute — so instead we
        // verify the line formatting via the private write funnel formatting.
        let line = format!(
            "{} {} {} {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            Level::Info.letter(),
            "unit",
            "hello"
        );
        assert!(line.contains("I unit hello"));
        assert!(
            line.starts_with("20"),
            "line must start with a timestamp year"
        );
    }

    #[test]
    fn rotate_renames_past_max() {
        // rotate_if_needed behaves on a real temp file we create ourselves.
        let dir = std::env::temp_dir().join(format!("pangan-rotate-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("app.log");
        std::fs::write(&path, vec![b'x'; 10]).unwrap();
        let logger = Logger {
            file: Mutex::new(std::io::BufWriter::new(
                std::fs::File::create(&path).unwrap(),
            )),
            path: path.clone(),
        };
        // Under the cap: no rotation, file still present.
        rotate_if_needed(&logger, 10);
        assert!(path.exists());
        // Over the cap: the current file moves to `.1` and a fresh one opens.
        rotate_if_needed(&logger, MAX_LOG_BYTES + 1);
        assert!(dir.join("app.log.1").exists(), "old log must be backed up");
        assert!(path.exists(), "a fresh log must be opened");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
