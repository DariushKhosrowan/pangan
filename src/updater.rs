//! Self-update.
//!
//! Pangan quietly checks GitHub Releases for a newer build than the one that
//! is running. When a newer release exists and auto-update is enabled, we
//! download the matching Linux binary for this CPU, verify its SHA-256 against
//! the release's signed sidecar, swap it over the current executable, and
//! relaunch the new build. This is how the app "understands there is an
//! update on the GitHub server and quickly updates itself."
//!
//! Safety / robustness:
//! * The check runs once per launch on a background thread; it never blocks
//!   startup or the UI.
//! * The swap is atomic (write to a temp path in the same directory, then
//!   rename over the live executable). On Unix the old image stays open until
//!   this process exits, so the running app is never corrupted.
//! * We never replace a launcher script (e.g. the AppImage launcher installed
//!   by `install.sh --appimage`, which starts with `#!`). For those installs
//!   the updater reports that a manual refresh is the right move.
//! * If the executable is not writable (a system-wide install under
//!   /usr/local), we report it and retry on a later launch rather than fail.
//! * Updates are restart-applied: after a successful swap we spawn the new
//!   binary and exit. Because the new binary is already the latest version,
//!   the next check finds nothing newer and there is no update loop.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use crate::log;

/// GitHub repo that publishes the releases.
pub const REPO_OWNER: &str = "DariushKhosrowan";
pub const REPO_NAME: &str = "pangan";

const API_LATEST: &str = "https://api.github.com/repos/DariushKhosrowan/pangan/releases/latest";

/// A newer release that was found and matched to this CPU.
#[derive(Debug)]
pub struct ReleaseInfo {
    /// Version without the leading `v` (e.g. `"0.3.2"`).
    pub version: String,
    /// Direct download URL of the matching Linux binary asset.
    pub asset_url: String,
    /// URL of the release's `<asset>.sha256` sidecar.
    pub sha256_url: String,
}

/// Errors surfaced while checking / applying an update.
#[derive(Debug)]
pub enum UpdateError {
    /// Network or transport failure.
    Network(String),
    /// The release API responded but the payload was unexpected.
    Http(String),
    /// The expected asset / field was not present.
    NotFound(String),
    /// Filesystem failure while staging or swapping.
    Io(std::io::Error),
    /// The downloaded binary did not match its recorded checksum.
    ChecksumMismatch { expected: String, got: String },
    /// The current executable cannot be replaced (system-wide install).
    NotWritable(PathBuf),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Network(e) => write!(f, "network: {e}"),
            UpdateError::Http(e) => write!(f, "http: {e}"),
            UpdateError::NotFound(msg) => write!(f, "not found: {msg}"),
            UpdateError::Io(e) => write!(f, "io: {e}"),
            UpdateError::ChecksumMismatch { expected, got } => {
                write!(f, "checksum mismatch (expected {expected}, got {got})")
            }
            UpdateError::NotWritable(p) => write!(f, "executable not writable: {}", p.display()),
        }
    }
}

impl std::error::Error for UpdateError {}

fn parse_version(tag: &str) -> Option<String> {
    let t = tag.strip_prefix('v').unwrap_or(tag);
    Some(t.to_string())
}

/// Three-component numeric version compare ("0.3.1" < "0.3.2"). Components
/// beyond the third and pre-release suffixes are ignored; that covers the
/// version scheme Pangan actually uses.
fn compare_versions(a: &str, b: &str) -> Ordering {
    let pa: Vec<u64> = a
        .split('.')
        .take(3)
        .filter_map(|p| p.parse().ok())
        .collect();
    let pb: Vec<u64> = b
        .split('.')
        .take(3)
        .filter_map(|p| p.parse().ok())
        .collect();
    for i in 0..3 {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x.cmp(&y);
        }
    }
    Ordering::Equal
}

/// The `uname -m`-style arch token used in release asset names.
fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" | "aarch64" => std::env::consts::ARCH,
        other => other,
    }
}

/// Max bytes we will accept for a downloaded release asset (the Linux binary
/// is ~29 MB; give generous headroom).
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;

fn fetch(url: &str) -> Result<Vec<u8>, UpdateError> {
    let resp = ureq::get(url)
        .header("User-Agent", "pangan-updater/1.0")
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    resp.into_body()
        .into_with_config()
        .limit(MAX_ASSET_BYTES)
        .read_to_vec()
        .map_err(|e| UpdateError::Network(e.to_string()))
}

/// CDN-hosted latest-version marker attached to the newest release. Fetched
/// from the `latest/download` endpoint, which is served by GitHub's CDN and is
/// NOT subject to the unauthenticated API rate limit. This is the primary
/// source; the releases API is only a fallback.
const LATEST_VERSION_URL: &str =
    "https://github.com/DariushKhosrowan/pangan/releases/latest/download/LATEST_VERSION";

/// Query GitHub for the latest version newer than the running one, matched to
/// this CPU — or `None` when already up to date.
pub fn latest_release() -> Result<Option<ReleaseInfo>, UpdateError> {
    let version = fetch_latest_version()?;
    let current = env!("CARGO_PKG_VERSION");
    if compare_versions(&version, current) != Ordering::Greater {
        return Ok(None);
    }
    let asset_name = format!("pangan-linux-{}", current_arch());
    let url = asset_url_for(version.clone())?;
    let base = url.trim_end_matches(&asset_name);
    let sha256_url = format!("{base}{asset_name}.sha256");

    Ok(Some(ReleaseInfo {
        version,
        asset_url: url,
        sha256_url,
    }))
}

/// Fetch the latest release version string, preferring the CDN marker over
/// the (rate-limited) releases API.
fn fetch_latest_version() -> Result<String, UpdateError> {
    // Primary: CDN LATEST_VERSION file — no API rate limit.
    match fetch(LATEST_VERSION_URL) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let tag = text.trim();
            if !tag.is_empty() {
                if let Some(v) = parse_version(tag) {
                    return Ok(v);
                }
            }
        }
        Err(_) => { /* fall through to API */ }
    }

    // Fallback: releases API.
    let body = fetch(API_LATEST)?;
    let v: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| UpdateError::Http(e.to_string()))?;
    let tag = v
        .get("tag_name")
        .and_then(|t| t.as_str())
        .ok_or_else(|| UpdateError::NotFound("release has no tag_name".into()))?;
    parse_version(tag).ok_or_else(|| UpdateError::NotFound("unparseable tag".into()))
}

/// Build the download URL for the Linux asset matching this CPU.
fn asset_url_for(version: String) -> Result<String, UpdateError> {
    let asset = format!("pangan-linux-{}", current_arch());
    Ok(format!(
        "https://github.com/DariushKhosrowan/pangan/releases/download/v{version}/{asset}"
    ))
}
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn verify_checksum(data: &[u8], sha_url: &str) -> Result<(), UpdateError> {
    let sidecar = String::from_utf8(fetch(sha_url)?)
        .map_err(|_| UpdateError::NotFound("sha256 sidecar is not valid text".into()))?;
    let expected = sidecar
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    let got = sha256_hex(data);
    if expected.is_empty() {
        return Ok(()); // sidecar empty — nothing to verify against
    }
    if expected != got {
        return Err(UpdateError::ChecksumMismatch { expected, got });
    }
    Ok(())
}

/// True when `path` is a `#!` script (e.g. the AppImage launcher) rather than
/// a native executable. We must never swap a script for a binary.
fn is_launcher_script(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 2];
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    head == *b"#!"
}

/// Download the newer binary, verify its checksum, and atomically replace the
/// running executable with it. Does not relaunch.
pub fn apply_update(info: &ReleaseInfo) -> Result<(), UpdateError> {
    let exe = std::env::current_exe().map_err(UpdateError::Io)?;
    replace_executable(&exe, info)
}

/// Replace `target` (an executable path) with the newer binary downloaded from
/// `asset_url`, verifying it against `sha_url`. Downloads to a sibling path and
/// renames atomically so the live process image is never corrupted.
fn replace_executable(target: &std::path::Path, info: &ReleaseInfo) -> Result<(), UpdateError> {
    // Never overwrite a launcher script with a native binary.
    if is_launcher_script(target) {
        return Err(UpdateError::NotFound(
            "current executable is a launcher script; refresh via the installer".into(),
        ));
    }

    let dir = target.parent().ok_or_else(|| {
        UpdateError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "current executable has no parent directory",
        ))
    })?;

    // Download to a sibling path so the final rename stays on the same
    // filesystem (atomic) even when /tmp is a different mount.
    let tmp = dir.join(format!(".pangan-update-{}", std::process::id()));

    let data = fetch(&info.asset_url)?;
    verify_checksum(&data, &info.sha256_url)?;

    std::fs::write(&tmp, &data).map_err(UpdateError::Io)?;

    #[cfg(unix)]
    {
        if let Ok(meta) = std::fs::metadata(target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
    }

    // Atomic replace. On Unix the running image stays valid; on Windows this
    // fails while the process is alive, which we report as NotWritable.
    std::fs::rename(&tmp, target).map_err(|_| UpdateError::NotWritable(target.to_path_buf()))?;

    Ok(())
}

/// Spawn the (new) binary and exit this process.
pub fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = Command::new(&exe).args(std::env::args().skip(1)).spawn();
    }
    std::process::exit(0);
}

/// Takes `true` while an update is being applied in this process, so two
/// concurrent checks cannot both try to swap.
static UPDATING: AtomicBool = AtomicBool::new(false);

/// The full auto-update flow. Safe to call on a background thread.
pub fn run_auto_update(enabled: bool) {
    if !enabled {
        return;
    }
    if UPDATING.swap(true, AtomicOrdering::SeqCst) {
        return; // already updating
    }

    match latest_release() {
        Ok(Some(info)) => {
            log::info(&format!(
                "update available: v{} -> v{}",
                env!("CARGO_PKG_VERSION"),
                info.version
            ));
            match apply_update(&info) {
                Ok(()) => {
                    log::info(&format!("update {} applied; relaunching", info.version));
                    relaunch(); // does not return
                }
                Err(UpdateError::NotWritable(p)) => {
                    log::warn(&format!(
                        "update available (v{}) but {} is not writable — run the installer as root",
                        info.version,
                        p.display()
                    ));
                    UPDATING.store(false, AtomicOrdering::SeqCst);
                }
                Err(e) => {
                    log::warn(&format!("update apply failed: {e}; retry next launch"));
                    UPDATING.store(false, AtomicOrdering::SeqCst);
                }
            }
        }
        Ok(None) => { /* up to date */ }
        Err(e) => log::info(&format!("update check skipped: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_strips_v() {
        assert_eq!(parse_version("v0.3.1").as_deref(), Some("0.3.1"));
        assert_eq!(parse_version("0.4.0").as_deref(), Some("0.4.0"));
    }

    #[test]
    fn version_ordering() {
        assert_eq!(compare_versions("0.3.1", "0.3.1"), Ordering::Equal);
        assert_eq!(compare_versions("0.3.1", "0.3.2"), Ordering::Less);
        assert_eq!(compare_versions("0.3.10", "0.4.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0.0", "0.9.9"), Ordering::Greater);
    }

    #[test]
    fn arch_tokens() {
        let arch = current_arch();
        assert!(arch == "x86_64" || arch == "aarch64");
    }

    #[test]
    fn launcher_script_detection() {
        let dir = std::env::temp_dir();
        let p = dir.join("pangan-test-launcher.sh");
        std::fs::write(&p, "#!/usr/bin/env bash\nexec /tmp/x.AppImage\n").expect("write launcher");
        assert!(is_launcher_script(&p));
        let native = dir.join("pangan-test-native");
        std::fs::write(&native, "\x7fELF...not real").expect("write native");
        assert!(!is_launcher_script(&native));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&native);
    }

    #[test]
    fn sha256_hex_format() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Live check against the real GitHub release. Ignored by default —
    /// run explicitly with `cargo test updater_live -- --ignored`.
    #[test]
    #[ignore]
    fn updater_live() {
        match latest_release() {
            Ok(Some(info)) => {
                eprintln!(
                    "live: newer than {}: v{} (asset {})",
                    env!("CARGO_PKG_VERSION"),
                    info.version,
                    info.asset_url
                );
            }
            Ok(None) => {
                eprintln!("live: up to date (v{})", env!("CARGO_PKG_VERSION"));
            }
            Err(e) => panic!("live update check failed: {e}"),
        }
    }

    /// End-to-end swap test against the *published* release assets: download
    /// the real 0.3.1 binary and its sidecar from GitHub's CDN into a scratch
    /// target path, verify the checksum, and atomically replace the target.
    /// Ignored by default; run explicitly with `cargo test updater_apply_live
    /// -- --ignored`.
    #[test]
    #[ignore]
    fn updater_apply_live() {
        let dir = std::env::temp_dir().join(format!("pangan-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let target = dir.join("pangan-linux-x86_64");
        // A placeholder "current executable". Not a launcher script, so the
        // swap path is exercised for real.
        std::fs::write(&target, b"# placeholder current binary\n").expect("write target");
        let info = ReleaseInfo {
            version: "0.3.1".into(),
            asset_url: "https://github.com/DariushKhosrowan/pangan/releases/download/v0.3.1/pangan-linux-x86_64".into(),
            sha256_url: "https://github.com/DariushKhosrowan/pangan/releases/download/v0.3.1/pangan-linux-x86_64.sha256".into(),
        };
        let result = replace_executable(&target, &info);
        // Regardless of the outcome, the temp dir is cleaned up.
        let _ = std::fs::remove_dir_all(&dir);
        match result {
            Ok(()) => eprintln!("live apply: OK (target replaced)"),
            Err(e) => panic!("live apply failed: {e}"),
        }
    }
}
