//! Google sign-in and Drive storage for cloud sync.
//!
//! ## Why this shape
//!
//! The user asked for the easiest free option that syncs by itself. That rules
//! out the two obvious designs:
//!
//! * **A local redirect server** (`http://127.0.0.1:PORT`) needs a browser to
//!   come back to a loopback port. It breaks on the AppImage, on Windows
//!   firewalls, and on any machine where the browser runs elsewhere.
//! * **A client secret shipped in the binary** is not a secret at all, and
//!   Google rejects the "installed app" flow without one unless the app is
//!   registered for device flow.
//!
//! So this uses the **OAuth 2.0 Device Authorization Grant** (RFC 8628), which
//! is what TV apps use. The app shows a short code, the user types it on any
//! device they already have a browser on — phone, tablet, another laptop — and
//! the app picks up the token by polling. No local port, no embedded browser,
//! no copy-pasting a token out of a terminal.
//!
//! ## Why Drive `appDataFolder`
//!
//! `appDataFolder` is a hidden per-application folder in the user's own Drive.
//! It is free (it counts against the 15 GB everyone has), it needs no
//! spreadsheet to be created or shared, and it is invisible in the Drive UI so
//! it cannot be accidentally deleted or edited by hand. One small JSON file
//! lives there and every device reads and writes the same one.
//!
//! ## Setup the user has to do once
//!
//! Device flow needs a client id, which is per-application and must come from
//! Google Cloud Console. There is no way around that for a desktop app that
//! ships to other people. `PANGAN_GOOGLE_CLIENT_ID` overrides the built-in id,
//! so a fork can use its own project without touching the code.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Google's device-flow endpoints.
const DEVICE_CODE_URL: &str = "https://oauth2.googleapis.com/device/code";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// `drive.appdata` is the narrowest scope that can still store the sync file:
/// it grants access to this app's own hidden folder and nothing else. The
/// email scope is only used to show *which* account is connected.
const SCOPE: &str =
    "https://www.googleapis.com/auth/drive.appdata https://www.googleapis.com/auth/userinfo.email";

/// Base URL for the Drive API.
///
/// Overridable so the round-trip can be tested against a local fake server: the
/// multipart upload format and the file-id handling are exactly the kind of
/// thing that looks right and is not, and they can only be checked by actually
/// speaking the protocol. Production always uses the real host.
fn drive_base() -> String {
    std::env::var("PANGAN_DRIVE_BASE")
        .unwrap_or_else(|_| "https://www.googleapis.com".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// A shared HTTP agent with real timeouts.
///
/// Without this, a stalled connection blocks the calling thread indefinitely.
/// Sync runs off the UI thread, but a hung request still means the engine's
/// `busy` flag never clears and every later sync is silently skipped — the app
/// looks like it is syncing and never is. A global budget turns that into a
/// plain error the caller can report and retry.
fn agent() -> ureq::Agent {
    static ONCE: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(20)))
                .timeout_connect(Some(Duration::from_secs(8)))
                .build(),
        )
    })
    .clone()
}

/// True when an error means the credential is dead rather than the network.
///
/// A revoked or expired refresh token can never succeed by retrying, so the app
/// must stop and ask the user to sign in again instead of looping on failures
/// forever. Everything else — a timeout, a 500, no route — is worth retrying.
pub fn is_auth_error(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("invalid_grant")
        || m.contains("invalid_client")
        || m.contains("unauthorized")
        || m.contains("401")
        || m.contains("no refresh token")
        || m.contains("sign in again")
}

/// The file name inside `appDataFolder`.
const REMOTE_FILE: &str = "pangan-sync.json";

/// Client id of the Pangan desktop app. Overridable for forks and for local
/// testing without a rebuild.
pub fn client_id() -> String {
    // Precedence: a value the user pasted in Settings, then the build-time
    // environment variable, then the placeholder. Settings wins because it is
    // the only one an end user can change without a compiler.
    if let Some(id) = crate::sync::load_client_id() {
        return id;
    }
    std::env::var("PANGAN_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "PANGAN_DESKTOP_CLIENT_ID.apps.googleusercontent.com".to_string())
}

/// True when a real client id is configured. Without one, device flow cannot
/// start, and the UI should say so plainly instead of failing at the network.
pub fn has_client_id() -> bool {
    !is_placeholder(&client_id())
}

/// A client id is usable unless it is the shipped placeholder or blank.
///
/// This is a separate function because "is this the placeholder" is the exact
/// condition that decides whether the connect button is enabled, and getting it
/// wrong is silent: the button looks fine and Google rejects the request.
fn is_placeholder(id: &str) -> bool {
    let id = id.trim();
    id.is_empty() || id.starts_with("PANGAN_DESKTOP_CLIENT_ID")
}

// ---------------------------------------------------------------------------
// Device flow
// ---------------------------------------------------------------------------

/// What Google hands back when a device-code request is accepted.
#[derive(Clone, Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Ask Google for a device code. The user then visits `verification_url` and
/// types `user_code`.
pub fn request_device_code() -> Result<DeviceCode, String> {
    let body = serde_json::json!({
        "client_id": client_id(),
        "scope": SCOPE,
    });
    let resp = agent()
        .post(DEVICE_CODE_URL)
        .header("Content-Type", "application/json")
        .send_json(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("device code request failed: {e}")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("device code json: {e}"))?;
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        let desc = v
            .get("error_description")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        return Err(format!("{err}: {desc}"));
    }
    Ok(DeviceCode {
        device_code: v["device_code"].as_str().unwrap_or("").to_string(),
        user_code: v["user_code"].as_str().unwrap_or("").to_string(),
        verification_url: v["verification_url"]
            .as_str()
            .or_else(|| v["verification_uri"].as_str())
            .unwrap_or("https://www.google.com/device")
            .to_string(),
        expires_in: v["expires_in"].as_u64().unwrap_or(1800),
        interval: v["interval"].as_u64().unwrap_or(5),
    })
}

/// The outcome of one poll.
pub enum PollOutcome {
    /// The user has not approved yet; keep polling.
    Pending,
    /// Approved. These are the tokens to store.
    Approved(Tokens),
    /// The code expired or was refused; start over.
    Failed(String),
}

/// Poll the token endpoint once. Call this every `interval` seconds until it
/// returns `Approved` or `Failed`.
pub fn poll_token(device_code: &str) -> PollOutcome {
    let body = serde_json::json!({
        "client_id": client_id(),
        "device_code": device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
    });
    let resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send_json(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return PollOutcome::Failed(format!("token request failed: {e}")),
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return PollOutcome::Failed(format!("token json: {e}")),
    };
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        return match err {
            // These two mean "not yet", not "no".
            "authorization_pending" | "slow_down" => PollOutcome::Pending,
            "access_denied" => PollOutcome::Failed("access denied".into()),
            "expired_token" => PollOutcome::Failed("the code expired".into()),
            other => PollOutcome::Failed(other.to_string()),
        };
    }
    let access = v["access_token"].as_str().unwrap_or("").to_string();
    if access.is_empty() {
        return PollOutcome::Failed("no access token in response".into());
    }
    PollOutcome::Approved(Tokens {
        access_token: access,
        refresh_token: v["refresh_token"].as_str().unwrap_or("").to_string(),
        expires_at: now_secs() + v["expires_in"].as_i64().unwrap_or(3600),
        email: String::new(),
    })
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// A stored credential. Written to `sync.toml`, readable by the user.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// Unix seconds when `access_token` stops working.
    #[serde(default)]
    pub expires_at: i64,
    /// The signed-in account, for display only.
    #[serde(default)]
    pub email: String,
}

impl Tokens {
    pub fn is_empty(&self) -> bool {
        self.access_token.is_empty() && self.refresh_token.is_empty()
    }

    /// A token that is still good for at least another minute.
    fn usable(&self) -> bool {
        !self.access_token.is_empty() && self.expires_at > now_secs() + 60
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Exchange the refresh token for a fresh access token.
///
/// Access tokens last an hour, so without this the user would be asked to sign
/// in again every hour — which is exactly the kind of thing that makes people
/// turn sync off. The refresh token does not expire for a device-flow client.
pub fn refresh(tokens: &Tokens) -> Result<Tokens, String> {
    if tokens.refresh_token.is_empty() {
        return Err("no refresh token; sign in again".into());
    }
    let body = serde_json::json!({
        "client_id": client_id(),
        "refresh_token": tokens.refresh_token,
        "grant_type": "refresh_token",
    });
    let resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send_json(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("refresh failed: {e}")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("refresh json: {e}"))?;
    let access = v["access_token"].as_str().unwrap_or("");
    if access.is_empty() {
        let err = v["error"].as_str().unwrap_or("refresh failed");
        return Err(err.to_string());
    }
    Ok(Tokens {
        access_token: access.to_string(),
        // Google may or may not rotate the refresh token; keep the old one.
        refresh_token: v["refresh_token"]
            .as_str()
            .unwrap_or(&tokens.refresh_token)
            .to_string(),
        expires_at: now_secs() + v["expires_in"].as_i64().unwrap_or(3600),
        email: tokens.email.clone(),
    })
}

/// Return a usable access token, refreshing first if needed.
pub fn access_token(tokens: &mut Tokens) -> Result<String, String> {
    if tokens.usable() {
        return Ok(tokens.access_token.clone());
    }
    let fresh = refresh(tokens)?;
    *tokens = fresh;
    Ok(tokens.access_token.clone())
}

/// Ask Google which account the token belongs to.
pub fn fetch_email(token: &str) -> Option<String> {
    let resp = agent()
        .get(USERINFO_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()?;
    let text = resp.into_body().read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["email"].as_str().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Drive appDataFolder
// ---------------------------------------------------------------------------

/// What Drive knows about the sync file besides its contents.
///
/// `etag` is the concurrency token. Two devices that both read the file and
/// both write it back would otherwise let the slower one silently erase the
/// faster one's work — the write is unconditional, so the last request to
/// arrive simply wins. Sending the etag back with `If-Match` turns that into a
/// 412 the caller can retry, which is the difference between "sync" and "sync
/// except when two machines are awake".
#[derive(Clone, Debug, Default)]
pub struct RemoteFile {
    pub id: String,
    /// Drive's monotonic version counter, for cheap change detection.
    pub version: i64,
    /// The concurrency token used with `If-Match`.
    pub etag: String,
}

/// Find the sync file's id inside `appDataFolder`, if it exists.
fn find_file(token: &str, name: &str) -> Result<Option<String>, String> {
    Ok(find_file_meta(token, name)?.map(|f| f.id))
}

/// Find the sync file and its concurrency metadata.
fn find_file_meta(token: &str, name: &str) -> Result<Option<RemoteFile>, String> {
    let url = format!(
        "{}/drive/v3/files?spaces=appDataFolder&fields=files(id,name,version,etag)&q=name%20%3D%20%27{}%27",
        drive_base(),
        name
    );
    let resp = agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call();
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("drive list failed: {e}")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("drive list json: {e}"))?;
    Ok(v["files"].as_array().and_then(|a| a.first()).map(|f| {
        RemoteFile {
            id: f["id"].as_str().unwrap_or("").to_string(),
            version: f["version"].as_i64().unwrap_or(0),
            etag: f["etag"].as_str().unwrap_or("").to_string(),
        }
    }))
}

/// Download the sync file. `Ok(None)` means no device has uploaded yet.
pub fn download(token: &str) -> Result<Option<String>, String> {
    download_named(token, REMOTE_FILE)
}

/// Download any file from `appDataFolder` by name.
pub fn download_named(token: &str, name: &str) -> Result<Option<String>, String> {
    Ok(download_meta(token, name)?.map(|(text, _)| text))
}

/// Download a file together with the metadata needed to write it back safely.
///
/// Returns `(contents, metadata)`. The metadata is what a later
/// `upload_conditional` needs, so a caller that wants a safe read-modify-write
/// must use this and not `download_named`.
pub fn download_meta(token: &str, name: &str) -> Result<Option<(String, RemoteFile)>, String> {
    let Some(meta) = find_file_meta(token, name)? else {
        return Ok(None);
    };
    let url = format!("{}/drive/v3/files/{}?alt=media", drive_base(), meta.id);
    let resp = agent()
        .get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call();
    match resp {
        Ok(r) => Ok(Some((
            r.into_body().read_to_string().unwrap_or_default(),
            meta,
        ))),
        Err(e) => Err(format!("drive download failed: {e}")),
    }
}

/// Upload the sync file, creating it on first use and updating it after.
pub fn upload(token: &str, body: &str) -> Result<(), String> {
    upload_named(token, REMOTE_FILE, body)
}

/// Upload any file to `appDataFolder` by name, creating or updating it.
pub fn upload_named(token: &str, name: &str, body: &str) -> Result<(), String> {
    upload_conditional(token, name, body, None)
}

/// Upload, refusing to overwrite a file that changed since `expected`.
///
/// `expected` is the metadata from `download_meta`. When it is `Some` and the
/// remote has moved on, Drive answers 412 and this returns
/// [`CONFLICT`], which the caller resolves by re-reading and re-merging. When
/// it is `None` the write is unconditional, which is only correct for a file
/// this device is creating or for a caller that has already established it is
/// the only writer.
pub const CONFLICT: &str = "pangan-sync-conflict";

pub fn upload_conditional(
    token: &str,
    name: &str,
    body: &str,
    expected: Option<&RemoteFile>,
) -> Result<(), String> {
    match find_file_meta(token, name)? {
        Some(meta) => {
            // Content-only update: PATCH the media, leave metadata alone.
            let url = format!(
                "{}/upload/drive/v3/files/{}?uploadType=media",
                drive_base(),
                meta.id
            );
            let mut req = agent()
                .patch(&url)
                .header("Authorization", &format!("Bearer {token}"))
                .header("Content-Type", "application/json");
            // Only guard when the caller actually read the file. A caller that
            // did not read it has nothing to be stale against.
            if let Some(exp) = expected {
                if !exp.etag.is_empty() {
                    req = req.header("If-Match", &exp.etag);
                }
            }
            let resp = req.send(body);
            match resp {
                Ok(r) if r.status() == 200 => Ok(()),
                Ok(r) if r.status() == 412 => Err(CONFLICT.to_string()),
                Ok(r) => Err(format!("drive update HTTP {}", r.status())),
                Err(e) => {
                    // ureq surfaces a non-2xx status as an error; a 412 must
                    // still be recognised as a conflict rather than a failure.
                    let msg = e.to_string();
                    if msg.contains("412") {
                        Err(CONFLICT.to_string())
                    } else {
                        Err(format!("drive update failed: {msg}"))
                    }
                }
            }
        }
        None => {
            // Multipart: metadata (name + parent appDataFolder) then content.
            let boundary = "pangan-sync-boundary";
            let meta = serde_json::json!({
                "name": name,
                "parents": ["appDataFolder"],
            });
            let payload = format!(
                "--{b}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{m}\r\n--{b}\r\nContent-Type: application/json\r\n\r\n{body}\r\n--{b}--",
                b = boundary,
                m = meta,
                body = body
            );
            let url = format!(
                "{}/upload/drive/v3/files?uploadType=multipart",
                drive_base()
            );
            let resp = agent()
                .post(&url)
                .header("Authorization", &format!("Bearer {token}"))
                .header(
                    "Content-Type",
                    &format!("multipart/related; boundary={boundary}"),
                )
                .send(payload);
            match resp {
                Ok(r) if r.status() == 200 => Ok(()),
                Ok(r) => Err(format!("drive create HTTP {}", r.status())),
                Err(e) => Err(format!("drive create failed: {e}")),
            }
        }
    }
}

/// How long to wait between polls, as a `Duration`.
pub fn poll_interval(secs: u64) -> Duration {
    Duration::from_secs(secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_with_no_expiry_is_not_usable() {
        let t = Tokens {
            access_token: "x".into(),
            expires_at: 0,
            ..Default::default()
        };
        assert!(!t.usable(), "an expired token must be refreshed");
    }

    #[test]
    fn a_token_good_for_an_hour_is_usable() {
        let t = Tokens {
            access_token: "x".into(),
            expires_at: now_secs() + 3600,
            ..Default::default()
        };
        assert!(t.usable());
    }

    #[test]
    fn a_token_expiring_within_a_minute_is_refreshed_early() {
        // Refreshing 30s early avoids a request that dies mid-flight because
        // the token expired between the check and the call.
        let t = Tokens {
            access_token: "x".into(),
            expires_at: now_secs() + 30,
            ..Default::default()
        };
        assert!(!t.usable());
    }

    #[test]
    fn a_placeholder_client_id_is_not_usable() {
        // The shipped placeholder must never be treated as a real client, or
        // the connect button would send a request Google rejects.
        assert!(is_placeholder(
            "PANGAN_DESKTOP_CLIENT_ID.apps.googleusercontent.com"
        ));
        assert!(!is_placeholder("1234-abc.apps.googleusercontent.com"));
        assert!(
            is_placeholder(""),
            "a blank id cannot drive the device flow"
        );
        assert!(is_placeholder("   "), "whitespace is not an id either");
    }

    #[test]
    fn empty_tokens_are_empty() {
        assert!(Tokens::default().is_empty());
        let t = Tokens {
            refresh_token: "r".into(),
            ..Default::default()
        };
        assert!(!t.is_empty());
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn a_dead_credential_is_told_apart_from_a_network_failure() {
        // These must stop syncing and ask for a new sign-in: retrying can
        // never succeed, so the app must not loop on them.
        assert!(is_auth_error("refresh failed: invalid_grant"));
        assert!(is_auth_error("invalid_client"));
        assert!(is_auth_error("no refresh token; sign in again"));
        assert!(is_auth_error("drive list failed: HTTP 401 Unauthorized"));

        // These are transient and worth retrying.
        assert!(!is_auth_error("drive download failed: timeout"));
        assert!(!is_auth_error("drive create HTTP 500"));
        assert!(!is_auth_error("dns error"));
    }

    #[test]
    fn the_agent_is_shared_not_rebuilt_per_request() {
        // The agent lives in a OnceLock, so every call hands back the same
        // pooled instance. Building one per request would throw away
        // connection reuse on every sync.
        let a = agent();
        let b = agent();
        assert!(std::ptr::eq(
            a.config() as *const _ as *const u8,
            b.config() as *const _ as *const u8
        ));
    }
}

#[cfg(test)]
pub(crate) mod drive_roundtrip {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A fake Drive: enough of the protocol to prove the client speaks it.
    ///
    /// It keeps one file in memory and answers the calls the client makes:
    /// list, download, multipart create, media update. The point is to exercise
    /// the real request shapes, because a multipart body with the wrong
    /// boundary or a missing `parents` field fails only against a real server,
    /// never in a test that mocks the function out.
    /// Serialises the tests that point the client at a fake server.
    ///
    /// `PANGAN_DRIVE_BASE` is process-wide, so two such tests running in
    /// parallel would each see the other's server and fail confusingly. The
    /// guard makes them run one at a time.
    pub(crate) static DRIVE_ENV_LOCK: Mutex<()> = Mutex::new(());
    pub(crate) fn fake_drive_for_tests() -> (String, Arc<Mutex<Option<String>>>) {
        fake_drive()
    }

    /// The fake server's view of the file: contents plus a version counter that
    /// stands in for Drive's etag.
    ///
    /// Without a version there is no way to test the conditional write, and the
    /// conditional write is the whole defence against two devices clobbering
    /// each other. A fake that always says "yes" would pass while the real
    /// thing lost data.
    struct FakeFile {
        body: String,
        version: u64,
    }

    fn fake_drive() -> (String, Arc<Mutex<Option<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stored: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let store = Arc::clone(&stored);
        let file: Arc<Mutex<Option<FakeFile>>> = Arc::new(Mutex::new(None));
        let file_state = Arc::clone(&file);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());

                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                let mut if_match: Option<String> = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                    if let Some(v) = lower.strip_prefix("if-match:") {
                        // Keep the original case: the value is compared as-is.
                        if_match = line.split_once(':').map(|(_, v)| v.trim().to_string());
                        let _ = v;
                    }
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                let body = String::from_utf8_lossy(&body).to_string();

                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let method = request_line.split_whitespace().next().unwrap_or("GET");

                // Order matters: "/upload/drive/v3/files?..." also contains
                // "/drive/v3/files?", so the upload cases must come first or a
                // create is answered as a list and silently stores nothing.
                let (status, payload) = if path.contains("uploadType=multipart") {
                    // The content is the last part of the multipart body.
                    let after_meta = body.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
                    let rest = &body[after_meta..];
                    let content = rest
                        .find("\r\n\r\n")
                        .map(|i| rest[i + 4..].to_string())
                        .unwrap_or_default();
                    let content = content
                        .rsplit_once("\r\n--")
                        .map(|(c, _)| c.to_string())
                        .unwrap_or(content);
                    *store.lock().unwrap() = Some(content.clone());
                    *file_state.lock().unwrap() = Some(FakeFile {
                        body: content,
                        version: 1,
                    });
                    ("200 OK", r#"{"id":"file-1"}"#.to_string())
                } else if path.contains("uploadType=media") {
                    // A conditional write must be refused when the caller's
                    // etag is stale, exactly as Drive refuses it.
                    let mut guard = file_state.lock().unwrap();
                    if let Some(exp) = &if_match {
                        let current = guard
                            .as_ref()
                            .map(|f| format!("\"v{}\"", f.version))
                            .unwrap_or_default();
                        if *exp != current {
                            ("412 Precondition Failed", String::new())
                        } else {
                            let next = guard.as_ref().map(|f| f.version + 1).unwrap_or(1);
                            *guard = Some(FakeFile {
                                body: body.clone(),
                                version: next,
                            });
                            *store.lock().unwrap() = Some(body);
                            ("200 OK", r#"{"id":"file-1"}"#.to_string())
                        }
                    } else {
                        let next = guard.as_ref().map(|f| f.version + 1).unwrap_or(1);
                        *guard = Some(FakeFile {
                            body: body.clone(),
                            version: next,
                        });
                        *store.lock().unwrap() = Some(body);
                        ("200 OK", r#"{"id":"file-1"}"#.to_string())
                    }
                } else if path.contains("alt=media") {
                    match store.lock().unwrap().clone() {
                        Some(data) => ("200 OK", data),
                        None => ("404 Not Found", String::new()),
                    }
                } else if path.contains("/drive/v3/files?") {
                    let guard = file_state.lock().unwrap();
                    // Built with serde_json rather than a format string: the
                    // etag has to be a JSON string containing quotes, and
                    // hand-escaping that is exactly the kind of thing that
                    // looks right and produces invalid JSON.
                    let payload = match guard.as_ref() {
                        Some(f) => serde_json::json!({
                            "files": [{
                                "id": "file-1",
                                "name": "pangan-sync.json",
                                "version": f.version.to_string(),
                                "etag": format!("\"v{}\"", f.version),
                            }]
                        }),
                        None => serde_json::json!({ "files": [] }),
                    };
                    ("200 OK", payload.to_string())
                } else {
                    let _ = method;
                    ("404 Not Found", String::new())
                };

                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        (format!("http://{addr}"), stored)
    }

    #[test]
    fn upload_then_download_returns_exactly_what_was_sent() {
        let _guard = DRIVE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (base, _store) = fake_drive();
        std::env::set_var("PANGAN_DRIVE_BASE", &base);

        let payload = r#"{"sessions":[],"tasks":[],"next_task_id":1}"#;

        // Nothing there yet.
        assert_eq!(download("tok").unwrap(), None);

        // Create, then read it back byte for byte.
        upload("tok", payload).unwrap();
        assert_eq!(download("tok").unwrap().as_deref(), Some(payload));

        // Update in place: the second upload must not create a second file.
        let payload2 = r#"{"sessions":[],"tasks":[],"next_task_id":9}"#;
        upload("tok", payload2).unwrap();
        assert_eq!(download("tok").unwrap().as_deref(), Some(payload2));

        std::env::remove_var("PANGAN_DRIVE_BASE");
    }
}
