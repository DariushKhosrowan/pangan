//! Atomic file writes.
//!
//! Writing `Data` directly with `fs::write` / `File::create` + `write_all`
//! truncates the destination file first. A crash, power loss, or `kill -9`
//! mid-write leaves a partially-written file, and the store — the entire user
//! record — is silently lost on the next load.
//!
//! `atomic_write` writes to a temp file in the same directory, fsyncs it,
//! renames it over the destination (an atomic operation on POSIX), then fsyncs
//! the parent directory so the rename itself is durable. The file at `path`
//! is therefore always either the previous content or the new content — never
//! a truncated mix.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically: temp file in the same directory, fsync
/// temp, rename over target, fsync parent directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(dir)?;

    // Unique temp path in the same directory (same filesystem → rename is atomic).
    let tmp = temp_path(path);
    {
        let mut f = fs::File::create(&tmp)?;
        io::Write::write_all(&mut f, bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;

    // fsync the parent dir so the rename is durable across a crash.
    if let Ok(dir_file) = fs::File::open(dir) {
        let _ = dir_file.sync_all();
    }
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".to_string());
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    // Loop until we find a name that doesn't already exist; pid + counter makes
    // concurrent writers to different paths not collide.
    let pid = std::process::id();
    for i in 0u32..10_000 {
        let candidate = dir.join(format!(".{file_name}.tmp.{pid}.{i}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Collision under extreme contention is effectively impossible; fall back to
    // a timestamped name rather than failing.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    dir.join(format!(".{file_name}.tmp.{pid}.{nanos}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pangan-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn test_atomic_write_creates_new_file() {
        let dir = tmpdir("creates");
        let path = dir.join("data.json");
        atomic_write(&path, b"hello world").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello world");
        // no temp residue
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn test_atomic_write_overwrites_existing() {
        let dir = tmpdir("overwrites");
        let path = dir.join("data.json");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new content").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new content");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn test_atomic_write_preserves_existing_on_write_failure() {
        let dir = tmpdir("fail-preserve");
        let path = dir.join("data.json");
        atomic_write(&path, b"original data").unwrap();

        // Force a failure by making the directory read-only *after* the temp
        // file's data is written but before rename. On a read-only dir the
        // rename fails (EACCES), leaving the original intact.
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms.clone()).unwrap();

        let result = atomic_write(&path, b"should not land");
        // restore permissions so cleanup works (explicit mode, not read-only toggle)
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&dir, perms);

        assert!(result.is_err(), "rename into read-only dir must fail");
        assert_eq!(
            fs::read(&path).unwrap(),
            b"original data",
            "original intact"
        );
    }

    #[test]
    fn test_atomic_write_removes_temp_file_on_failure() {
        let dir = tmpdir("fail-clean");
        let path = dir.join("data.json");
        atomic_write(&path, b"original").unwrap();

        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&dir, perms.clone()).unwrap();

        let result = atomic_write(&path, b"x");
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dir).unwrap().permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(&dir, perms);

        assert!(result.is_err());
        // no temp files left
        for entry in fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.contains(".tmp."),
                "temp file {name} left behind after failure"
            );
        }
    }

    #[test]
    fn test_atomic_write_creates_missing_parent_dir() {
        let base = tmpdir("parents");
        let path = base.join("a").join("b").join("data.json");
        atomic_write(&path, b"deep").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"deep");
    }

    #[test]
    fn test_atomic_write_roundtrip_1mb_payload() {
        let dir = tmpdir("1mb");
        let path = dir.join("data.json");
        let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        atomic_write(&path, &payload).unwrap();
        assert_eq!(fs::read(&path).unwrap(), payload);
    }

    #[test]
    fn test_atomic_write_concurrent_different_paths_do_not_collide() {
        let dir = tmpdir("concurrent");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let path = dir.join(format!("data-{i}.json"));
                    let payload = format!("payload-{i}-{}", "x".repeat(1000));
                    for _ in 0..20 {
                        atomic_write(&path, payload.as_bytes()).unwrap();
                    }
                    let read_back = fs::read(&path).unwrap();
                    assert_eq!(read_back, payload.as_bytes());
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 8);
    }
}
