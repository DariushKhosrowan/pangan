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
    let resp = ureq::post(DEVICE_CODE_URL)
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
    let resp = ureq::post(TOKEN_URL)
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
    let resp = ureq::post(TOKEN_URL)
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
    let resp = ureq::get(USERINFO_URL)
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

/// Find the sync file's id inside `appDataFolder`, if it exists.
fn find_file(token: &str, name: &str) -> Result<Option<String>, String> {
    let url = format!(
        "https://www.googleapis.com/drive/v3/files?spaces=appDataFolder&fields=files(id,name)&q=name%20%3D%20%27{}%27",
        name
    );
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call();
    let text = match resp {
        Ok(r) => r.into_body().read_to_string().unwrap_or_default(),
        Err(e) => return Err(format!("drive list failed: {e}")),
    };
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("drive list json: {e}"))?;
    Ok(v["files"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|f| f["id"].as_str())
        .map(|s| s.to_string()))
}

/// Download the sync file. `Ok(None)` means no device has uploaded yet.
pub fn download(token: &str) -> Result<Option<String>, String> {
    download_named(token, REMOTE_FILE)
}

/// Download any file from `appDataFolder` by name.
pub fn download_named(token: &str, name: &str) -> Result<Option<String>, String> {
    let Some(id) = find_file(token, name)? else {
        return Ok(None);
    };
    let url = format!("https://www.googleapis.com/drive/v3/files/{id}?alt=media");
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {token}"))
        .call();
    match resp {
        Ok(r) => Ok(Some(r.into_body().read_to_string().unwrap_or_default())),
        Err(e) => Err(format!("drive download failed: {e}")),
    }
}

/// Upload the sync file, creating it on first use and updating it after.
pub fn upload(token: &str, body: &str) -> Result<(), String> {
    upload_named(token, REMOTE_FILE, body)
}

/// Upload any file to `appDataFolder` by name, creating or updating it.
pub fn upload_named(token: &str, name: &str, body: &str) -> Result<(), String> {
    match find_file(token, name)? {
        Some(id) => {
            // Content-only update: PATCH the media, leave metadata alone.
            let url =
                format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=media");
            let resp = ureq::patch(&url)
                .header("Authorization", &format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .send(body);
            match resp {
                Ok(r) if r.status() == 200 => Ok(()),
                Ok(r) => Err(format!("drive update HTTP {}", r.status())),
                Err(e) => Err(format!("drive update failed: {e}")),
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
            let url = "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart";
            let resp = ureq::post(url)
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
