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

/// The scopes the **device flow** is allowed to ask for.
///
/// Note what is missing: `drive.appdata`. Google rejects it for this grant type
/// outright (`Invalid device flow scope`), so a device-flow sign-in cannot
/// reach the hidden app folder. It asks for `drive.file` instead, which is
/// narrower than full Drive access in practice: the API then only exposes
/// files this app itself created. The cost is that the file lives where the
/// user can see it in Drive rather than in a hidden folder.
///
/// The loopback flow in `oauth` has no such restriction and uses
/// `drive.appdata`, which is why it is the preferred path.
const SCOPE: &str =
    "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/userinfo.email";

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
/// The shared HTTP agent, exposed so the loopback sign-in flow in `oauth`
/// uses the same timeouts and connection pool rather than building its own.
pub fn http_agent() -> ureq::Agent {
    agent()
}

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
    // Only signals that mean "this credential can never work again".
    //
    // `invalid_grant` is the one Google returns for a revoked or expired refresh
    // token. `invalid_client` means the client id/secret pair is wrong, which no
    // amount of retrying fixes. The rest are our own wording for the same
    // situation.
    //
    // Deliberately NOT here: a bare "401". Matching that substring signed the
    // user out whenever any response body happened to contain those three
    // digits — a Drive error, a quota message, a request id — which looked like
    // the app randomly forgetting the account. A real 401 from Drive already
    // carries `unauthorized` in the text, so nothing is lost by dropping it.
    m.contains("invalid_grant")
        || m.contains("invalid_client")
        || m.contains("unauthorized")
        || m.contains("no refresh token")
        || m.contains("sign in again")
}

/// Client id baked into this build, if any.
///
/// The point of compiling it in is that the user never sees a setup step: they
/// press Connect and it works. `option_env!` keeps the source honest for forks
/// and CI builds that do not supply one — those fall back to the placeholder
/// and the Settings field, exactly as before.
const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("PANGAN_GOOGLE_CLIENT_ID");

/// Client secret baked into this build, if any.
///
/// A device-flow client cannot exchange a code without presenting this, so
/// without it the sign-in is approved on the phone and then silently fails.
/// It is not a real secret for this client type (the flow targets TVs and
/// printers that cannot keep one), but it must be sent.
const BUILT_IN_CLIENT_SECRET: Option<&str> = option_env!("PANGAN_GOOGLE_CLIENT_SECRET");

/// Client id of the Pangan desktop app. Overridable for forks and for local
/// testing without a rebuild.
pub fn client_id() -> String {
    // Precedence: a value the user pasted in Settings, then the build-time
    // value, then the environment, then the placeholder. Settings wins because
    // it is the only one an end user can change without a compiler.
    if let Some(id) = crate::sync::backup::load_client_id() {
        return id;
    }
    if let Some(id) = BUILT_IN_CLIENT_ID.filter(|s| !s.trim().is_empty()) {
        return id.to_string();
    }
    std::env::var("PANGAN_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "PANGAN_DESKTOP_CLIENT_ID.apps.googleusercontent.com".to_string())
}

/// The client secret to present when exchanging a token.
///
/// Same precedence as the id: a value the user configured, then the build-time
/// value, then the environment. Empty means "send nothing", which is correct
/// for a Desktop-app client where PKCE replaces the secret.
pub fn client_secret() -> String {
    if let Some(s) = crate::sync::backup::load_client_secret() {
        return s;
    }
    if let Some(s) = BUILT_IN_CLIENT_SECRET.filter(|s| !s.trim().is_empty()) {
        return s.to_string();
    }
    std::env::var("PANGAN_GOOGLE_CLIENT_SECRET")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_default()
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

/// A short, log-safe excerpt of a response body.
///
/// Token responses can carry a refresh token, so the log gets at most a
/// single line and nothing that looks like a credential is useful here: this
/// exists to make a protocol surprise readable, not to record the payload.
fn preview(body: &str) -> String {
    let one_line: String = body.chars().take(200).collect::<String>().replace('\n', " ");
    one_line.trim().to_string()
}

/// Poll the token endpoint once. Call this every `interval` seconds until it
/// returns `Approved` or `Failed`.
pub fn poll_token(device_code: &str) -> PollOutcome {
    // The device-code grant requires the client secret. Google is explicit
    // about it: without one it answers `Missing required parameter:
    // client_secret` and never issues a token, which is why a sign-in could be
    // approved on the phone and still leave the app disconnected.
    let body = serde_json::json!({
        "client_id": client_id(),
        "client_secret": client_secret(),
        "device_code": device_code,
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
    });
    // The status code must not decide the outcome here.
    //
    // Google answers the device-flow token endpoint with **428 Precondition
    // Required** while the user has not finished on their phone
    // (`error: authorization_pending`), and with 400 for the other error
    // codes. The useful answer is always in the body. Treating a non-2xx
    // status as a transport failure aborted the poll five seconds in, well
    // before anyone could type the code, so a sign-in that Google had accepted
    // never reached the app. Only a genuine transport error (no response at
    // all) is a failure here.
    let resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return PollOutcome::Failed(format!("token request failed: {e}")),
    };
    let v: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return PollOutcome::Failed(format!("token json: {e} (body: {})", preview(&text)))
        }
    };
    if let Some(err) = v.get("error").and_then(|x| x.as_str()) {
        return match err {
            // These two mean "not yet", not "no".
            "authorization_pending" | "slow_down" => PollOutcome::Pending,
            "access_denied" => PollOutcome::Failed("access denied".into()),
            "expired_token" => PollOutcome::Failed("the code expired".into()),
            other => PollOutcome::Failed(format!(
                "{other}: {}",
                v.get("error_description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
            )),
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
        "client_secret": client_secret(),
        "refresh_token": tokens.refresh_token,
        "grant_type": "refresh_token",
    });
    // Read the body whatever the status is, for the same reason `poll_token`
    // does: Google puts the reason in the JSON, and ureq turns a non-2xx into an
    // opaque error that throws that reason away. Without this, a 400 came back
    // as "refresh failed: http status: 400" — no `invalid_grant` for
    // `is_auth_error` to recognise, and nothing useful in the log.
    let resp = agent()
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .config()
        .http_status_as_error(false)
        .build()
        .send_json(body);
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("refresh failed: {e}")),
    };
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("refresh json: {e} (body: {})", preview(&text)))?;
    let access = v["access_token"].as_str().unwrap_or("");
    if access.is_empty() {
        let err = v["error"].as_str().unwrap_or("refresh failed");
        let desc = v
            .get("error_description")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        return Err(if desc.is_empty() {
            err.to_string()
        } else {
            format!("{err}: {desc}")
        });
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
// Fields below are part of an external format (Drive files API metadata (version, space)).
// Nothing in this binary reads them yet; they are kept so the shape
// stays documented and parsing does not silently drift.
#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct RemoteFile {
    pub id: String,
    /// Drive's monotonic version counter, for cheap change detection.
    pub version: i64,
    /// The concurrency token used with `If-Match`.
    pub etag: String,
    /// Which Drive space the file was found in: `appDataFolder` (hidden, needs
    /// the `drive.appdata` scope) or `drive` (visible in the user's Drive,
    /// which is all a `drive.file` token can reach).
    pub space: String,
}

/// The spaces the sync file can live in, in order of preference.
///
/// `appDataFolder` is the hidden per-app folder and is the better home: it is
/// invisible in the Drive UI, so it cannot be renamed, edited, or deleted by
/// hand. But it needs the `drive.appdata` scope, which only the loopback
/// sign-in can obtain. A device-flow token has `drive.file` instead, and for
/// that token `appDataFolder` is not reachable at all — so the file has to go
/// in the user's normal Drive. Listing `spaces=drive` under `drive.file`
/// returns only files this app created, so the search stays narrow.
const SPACES: [&str; 2] = ["appDataFolder", "drive"];

/// Find the sync file and its concurrency metadata.
///
/// Searches both spaces, because a user who signed in with one flow and later
/// with the other must still find their data rather than silently starting a
/// second, empty file.
fn find_file_meta(token: &str, name: &str) -> Result<Option<RemoteFile>, String> {
    for space in SPACES {
        let url = format!(
            "{}/drive/v3/files?spaces={}&fields=files(id,name,version,etag)&q=name%20%3D%20%27{}%27",
            drive_base(),
            space,
            name
        );
        let resp = agent()
            .get(&url)
            .header("Authorization", &format!("Bearer {token}"))
            .config()
            .http_status_as_error(false)
            .build()
            .call();
        let (status, text) = match resp {
            Ok(r) => (r.status(), r.into_body().read_to_string().unwrap_or_default()),
            // A transport failure is a real failure, not "the file is absent".
            Err(e) => return Err(format!("drive list failed: {e}")),
        };
        if status != 200 {
            let reason = google_reason(&text);
            // A space this token cannot see is not fatal; try the next one.
            // But an error that applies to *every* space (the Drive API being
            // disabled, a revoked scope) must be reported, not silently turned
            // into "no file yet" — that made the app try to create a file after
            // a read that had actually failed, and report the create's error
            // instead of the read's.
            if status == 401 || status == 403 {
                let is_space_problem = reason.to_lowercase().contains("appdatafolder");
                if is_space_problem {
                    continue;
                }
                return Err(format!("drive list failed: HTTP {status}: {reason}"));
            }
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(f) = v["files"].as_array().and_then(|a| a.first()) {
            return Ok(Some(RemoteFile {
                id: f["id"].as_str().unwrap_or("").to_string(),
                version: f["version"].as_i64().unwrap_or(0),
                etag: f["etag"].as_str().unwrap_or("").to_string(),
                space: space.to_string(),
            }));
        }
    }
    Ok(None)
}

/// Download any file from `appDataFolder` by name.
pub fn download_named(token: &str, name: &str) -> Result<Option<String>, String> {
    Ok(download_meta(token, name)?.map(|(text, _)| text))
}

/// Read the sync file. `Ok(None)` means no device has uploaded yet.
///
/// The engine itself goes through [`download_meta`], because it needs the
/// concurrency token to write back safely. This is the plain read, kept for
/// the round-trip tests that assert what a device would actually see.
#[cfg(test)]
pub fn download(token: &str) -> Result<Option<String>, String> {
    download_named(token, "pangan-sync.json")
}

/// Write the sync file unconditionally, as a device with no metadata would.
/// Test helper: the engine always goes through [`upload_conditional`].
#[cfg(test)]
pub fn upload(token: &str, body: &str) -> Result<(), String> {
    upload_named(token, "pangan-sync.json", body)
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
            // Multipart: metadata (name + parent) then content. The parent has
            // to match the space this token can actually write to. A
            // `drive.file` token cannot create anything in `appDataFolder`, so
            // it goes in the user's normal Drive; a `drive.appdata` token goes
            // in the hidden folder. Getting this wrong fails the create with a
            // 403 that says nothing useful about which scope was at fault.
            //
            // The scope is a property of *how the user signed in*, not of what
            // the caller passed: on the very first sync there is no remote file
            // yet, so `expected` is `None` and carries no space. Inferring the
            // parent from it therefore always chose `appDataFolder`, which a
            // device-flow (`drive.file`) token can never write to — the first
            // sync after a successful sign-in failed with a 403 every time.
            // So: try the hidden folder, and fall back to the visible root when
            // Google refuses it. One of the two is always allowed.
            let mut last = String::new();
            for parent in ["appDataFolder", "drive"] {
                match create_file(token, name, body, parent) {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        // A refusal here means "this token cannot use that
                        // space", which the next iteration answers.
                        last = e;
                    }
                }
            }
            Err(last)
        }
    }
}

/// Create a file in one specific Drive space.
///
/// Split out so the caller can try both spaces without repeating the multipart
/// body construction, and so a failure names the space it failed in.
fn create_file(token: &str, name: &str, body: &str, parent: &str) -> Result<(), String> {
    let boundary = "pangan-sync-boundary";
    let meta = if parent == "drive" {
        serde_json::json!({ "name": name })
    } else {
        serde_json::json!({ "name": name, "parents": ["appDataFolder"] })
    };
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
    // Read the body whatever the status is. Google puts the actual reason in
    // the JSON — "Google Drive API has not been used in project N or it is
    // disabled", a scope problem, a quota — and a bare "HTTP 403" throws that
    // away, leaving the user with a number and no way to act on it.
    let resp = agent()
        .post(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .header(
            "Content-Type",
            &format!("multipart/related; boundary={boundary}"),
        )
        .config()
        .http_status_as_error(false)
        .build()
        .send(payload);
    match resp {
        Ok(r) if r.status() == 200 => Ok(()),
        Ok(r) => {
            let status = r.status();
            let text = r.into_body().read_to_string().unwrap_or_default();
            Err(format!(
                "drive create in {parent} HTTP {status}: {}",
                google_reason(&text)
            ))
        }
        Err(e) => Err(format!("drive create in {parent} failed: {e}")),
    }
}

/// Pull the human-readable reason out of a Google API error body.
///
/// Google's errors are nested (`error.message`, plus an `errors[0].message`),
/// and the useful sentence is the one the user can act on. When the body is not
/// JSON at all, the raw preview is still better than nothing.
pub fn google_reason(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            // The API-disabled message is long and ends with a console link.
            // Keep it whole: the link is the fix.
            return msg.trim().to_string(); }
        if let Some(msg) = v
            .get("error_description")
            .and_then(|m| m.as_str())
            .or_else( || v.get("error").and_then(|e| e.as_str()))
        {
            return msg.trim().to_string(); }
    }
    preview(body)
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
    fn a_stray_401_in_a_body_does_not_sign_the_user_out() {
        // The regression: `is_auth_error` matched the substring "401" anywhere
        // in the message. A Drive error, a quota notice, or a request id that
        // happened to contain those digits was read as a dead credential, and
        // the app cleared the tokens — which looked exactly like the account
        // being forgotten at random.
        assert!(!is_auth_error("drive list failed: quota exceeded for project 4012"));
        assert!(!is_auth_error("drive update HTTP 403 (request id 9f401c2a)"));
        assert!(!is_auth_error("rate limited, retry after 401 seconds"));
        // A real auth failure still matches, by its own words.
        assert!(is_auth_error("drive list failed: HTTP 401 Unauthorized"));
        assert!(is_auth_error(
            "invalid_grant: Token has been expired or revoked"
        ));
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
    #[allow(dead_code)] // shape of the fake Drive file; body is read by the
    // fake server's own response builder
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
