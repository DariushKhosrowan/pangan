//! A tiny, dependency-free structured file logger.
//!
//! Pangan is a desktop app: there is no console to read when something goes
//! wrong, and the user will not paste a terminal. So this module writes
//! leveled, timestamped lines to a log file next to the app's data, so a
//! failing run leaves a trail that can be opened and read.
//!
//! The log lives at `~/.config/pangan/logs/app.log` (or the platform's
//! config dir), and is rotated at a size cap: the current file is shifted to
//! `app.log.1`, `.1` → `.2`, and so on up to `LOG_KEEP` generations, then a
//! fresh one is started, so it never grows without bound and a recent
//! history survives rotation.
//!
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
use std::path::{Path, PathBuf};
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

/// How many rotated generations to retain (`.1` … `.LOG_KEEP`).
const LOG_KEEP: u32 = 3;

/// The log file's base name (the rotated predecessors get `.1`, `.2`, …).
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

/// Rotate a log file when it has grown past `max_bytes`, shifting generations
/// up to `keep` and deleting the oldest beyond that.
///
/// - If the file at `path` is missing, it's a no-op (`Ok(false)`).
/// - If it is at or below `max_bytes`, it's a no-op (`Ok(false)`).
/// - Otherwise the current file is renamed to `path.1`, `.1` → `.2`, … up to
///   `path.{keep}`, and the oldest generation beyond the chain is removed.
///   Returns `Ok(true)`.
pub fn rotate_if_needed(path: &Path, max_bytes: u64, keep: u32) -> std::io::Result<bool> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(false), // missing file: nothing to rotate
    };
    if meta.len() <= max_bytes {
        return Ok(false);
    }

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app.log".to_string());

    // Drop the oldest generation beyond the retained count.
    if keep > 0 {
        let _ = std::fs::remove_file(dir.join(format!("{base}.{keep}")));
    }
    // Shift `.1` → `.2`, `.2` → `.3`, … down from the top so the newest
    // predecessor ends up at `.1`.
    if keep > 1 {
        for i in (1..keep).rev() {
            let src = dir.join(format!("{base}.{i}"));
            if src.exists() {
                let _ = std::fs::rename(&src, dir.join(format!("{base}.{}", i + 1)));
            }
        }
    }
    // Promote the current file to `.1`.
    std::fs::rename(path, dir.join(format!("{base}.1")))?;
    Ok(true)
}

/// Rotate the singleton logger's active file if it has grown past the cap,
/// reopening a fresh one. Returns whether a rotation happened.
fn rotate_logger(logger: &Logger) -> bool {
    // Drop the writer lock before touching the files so we don't hold a stale
    // handle across the rename chain.
    drop(logger.file.lock().unwrap());
    match rotate_if_needed(&logger.path, MAX_LOG_BYTES, LOG_KEEP) {
        Ok(true) => {
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&logger.path)
            {
                *logger.file.lock().unwrap() = std::io::BufWriter::new(f);
            }
            true
        }
        _ => false,
    }
}

/// Write one line to the log. This is the single funnel every level goes
/// through; the caller passes the source module for the line.
pub fn write(level: Level, module: &str, msg: &str) {
    let Some(logger) = LOGGER.get() else {
        return; // not initialised — nothing to log to
    };
    let line = format!(
        "{} {} {} {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        level.letter(),
        module,
        msg
    );
    {
        let mut w = logger.file.lock().unwrap();
        let _ = w.write_all(line.as_bytes());
        let _ = w.flush();
    }
    // Keep the file bounded; rotation reopens a fresh file.
    let len = std::fs::metadata(&logger.path)
        .map(|m| m.len())
        .unwrap_or(0);
    if len > MAX_LOG_BYTES {
        rotate_logger(logger);
    }
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

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pangan-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn log_dir_sits_under_the_store_dir() {
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
    fn rotate_noop_below_threshold() {
        let dir = tmpdir("noop");
        let path = dir.join("app.log");
        std::fs::write(&path, vec![b'x'; 10]).unwrap();
        assert!(!rotate_if_needed(&path, 1000, 3).unwrap());
        assert!(path.exists());
        assert!(!dir.join("app.log.1").exists());
    }

    #[test]
    fn rotate_ignores_missing_file() {
        let dir = tmpdir("missing");
        let path = dir.join("app.log");
        assert!(!rotate_if_needed(&path, 100, 3).unwrap());
    }

    #[test]
    fn rotate_shifts_and_retains_up_to_keep() {
        let dir = tmpdir("shift");
        let path = dir.join("app.log");
        // Seed three generations.
        std::fs::write(dir.join("app.log.1"), b"one").unwrap();
        std::fs::write(dir.join("app.log.2"), b"two").unwrap();
        std::fs::write(dir.join("app.log.3"), b"three").unwrap();
        std::fs::write(&path, vec![b'x'; 2000]).unwrap();

        assert!(rotate_if_needed(&path, 1000, 3).unwrap());

        assert_eq!(
            std::fs::read(dir.join("app.log.1")).unwrap(),
            vec![b'x'; 2000]
        );
        assert_eq!(std::fs::read(dir.join("app.log.2")).unwrap(), b"one");
        assert_eq!(std::fs::read(dir.join("app.log.3")).unwrap(), b"two");
        // `.4` never existed; nothing past `keep` is left.
        assert!(!dir.join("app.log.4").exists());
        // The active file is gone (renamed to `.1`).
        assert!(!path.exists());
    }

    #[test]
    fn rotate_keeps_oldest_deleted_at_limit() {
        let dir = tmpdir("oldest");
        let path = dir.join("app.log");
        // Pre-seed generations up to the keep limit.
        std::fs::write(dir.join("app.log.1"), b"one").unwrap();
        std::fs::write(dir.join("app.log.2"), b"two").unwrap();
        std::fs::write(dir.join("app.log.3"), b"three").unwrap();
        std::fs::write(&path, vec![b'x'; 2000]).unwrap();

        assert!(rotate_if_needed(&path, 1000, 3).unwrap());

        // Oldest (".3" = "three") is dropped; the rest shift forward.
        assert_eq!(std::fs::read(dir.join("app.log.3")).unwrap(), b"two");
        assert_eq!(std::fs::read(dir.join("app.log.2")).unwrap(), b"one");
        assert_eq!(
            std::fs::read(dir.join("app.log.1")).unwrap(),
            vec![b'x'; 2000]
        );
    }

    #[test]
    fn rotate_creates_fresh_active_file() {
        let dir = tmpdir("fresh");
        let path = dir.join("app.log");
        std::fs::write(&path, vec![b'x'; 2000]).unwrap();
        assert!(rotate_if_needed(&path, 1000, 3).unwrap());
        // A brand-new active file is created after rotation.
        std::fs::write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
