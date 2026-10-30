//! Google credentials and the full-state backup.
//!
//! Record-level sync lives in `cloudsync`; this module owns the two things
//! that surround it: where the Google tokens are kept, and the "save a
//! restorable snapshot" button. Both go through the same Drive app folder, so
//! the user signs in once and gets sync and backup together.
//!
//! Tokens are written to `sync.toml` with owner-only permissions. A refresh
//! token is a long-lived credential to the user's Drive; on a shared machine a
//! world-readable config directory would hand it to every other account.

// ---------------------------------------------------------------------------
// Full-state backup
// ---------------------------------------------------------------------------

/// A complete snapshot of settings and data, uploaded as one JSON file.
///
/// This is deliberately separate from the record-level sync: sync keeps two
/// devices in step, a backup is a restorable point in time. They share the
/// same Drive folder and the same tokens, so the user has one connection and
/// two safety nets.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BackupPayload {
    pub app: String,
    pub version: String,
    pub ts: String,
    pub settings: String,
    pub data: String,
}

/// Upload a full backup to `pangan-backup.json` in the Drive app folder.
pub fn push_backup(
    tokens: &mut crate::gdrive::Tokens,
    payload: &BackupPayload,
) -> Result<(), String> {
    let token = crate::gdrive::access_token(tokens)?;
    let body = serde_json::to_string(payload).map_err(|e| format!("encode: {e}"))?;
    crate::gdrive::upload_named(&token, "pangan-backup.json", &body)
}

/// Download the most recent full backup, if one exists.
pub fn pull_backup(tokens: &mut crate::gdrive::Tokens) -> Result<BackupPayload, String> {
    let token = crate::gdrive::access_token(tokens)?;
    let text = crate::gdrive::download_named(&token, "pangan-backup.json")?
        .ok_or_else(|| "no backup has been uploaded yet".to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("backup is not valid: {e}"))
}

// ---------------------------------------------------------------------------
// Credential storage
// ---------------------------------------------------------------------------
//
// Everything sync needs lives in one `sync.toml`: the OAuth client id and the
// Google tokens. Google is the only sync backend — there is no server to
// configure and no token to paste by hand, which is the point: signing in is
// the whole setup.
//
// The file is written with 0600 permissions: a refresh token is a long-lived
// credential to the user's Drive, and on a shared machine a world-readable
// config directory would hand it to every other account.

fn sync_path() -> std::path::PathBuf {
    crate::store::Store::data_dir().join("sync.toml")
}

/// The OAuth client id, if the user pasted one in Settings.
///
/// A device-flow client id is not a secret (it is visible in the URL the user
/// opens), but it is per-project, so it cannot be hard-coded for everyone.
/// Keeping it here means a build can be handed to someone who registers their
/// own Google project without rebuilding the app.
pub fn load_client_id() -> Option<String> {
    let text = std::fs::read_to_string(sync_path()).ok()?;
    let v: toml::Value = text.parse().ok()?;
    v.get("client_id")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The OAuth client secret, if one is configured.
///
/// A **device-flow** client cannot exchange a device code without it: Google
/// answers `Missing required parameter: client_secret`. For that client type the
/// secret is not actually confidential — the flow is designed for TVs and
/// printers that cannot keep one — but Google still insists it be presented, so
/// the app has to hold it.
///
/// It lives next to the token in `sync.toml` (mode 0600) rather than in the
/// binary, so a build handed to someone else carries no credential, and a fork
/// can supply its own without a recompile. A Desktop-app client using the
/// loopback + PKCE flow never needs this: PKCE replaces the secret entirely.
pub fn load_client_secret() -> Option<String> {
    let text = std::fs::read_to_string(sync_path()).ok()?;
    let v: toml::Value = text.parse().ok()?;
    v.get("client_secret")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Store the client secret, keeping every other key in the file.
pub fn save_client_secret(secret: &str) -> Result<(), String> {
    let path = sync_path();
    let mut root: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let secret = secret.trim();
    if secret.is_empty() {
        root.remove("client_secret");
    } else {
        root.insert("client_secret".into(), secret.to_string().into());
    }
    write_private(&path, &root)
}

/// Write the file with owner-only permissions.
///
/// The refresh token and the client secret both live here. A world-readable
/// config directory would hand a long-lived credential to every account on a
/// shared machine.
fn write_private(path: &std::path::Path, root: &toml::Table) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = toml::to_string_pretty(root).map_err(|e| format!("encode: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("write: {e}"))?;
    restrict_permissions(path);
    Ok(())
}

/// Store the client id, keeping the tokens already in the file.
pub fn save_client_id(id: &str) -> Result<(), String> {
    let path = sync_path();
    let mut root: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    let id = id.trim();
    if id.is_empty() {
        root.remove("client_id");
    } else {
        root.insert("client_id".into(), id.to_string().into());
    }
    write_private(&path, &root)
}

/// Load stored Google tokens, if the user has signed in.
pub fn load_tokens() -> Option<crate::gdrive::Tokens> {
    let text = std::fs::read_to_string(sync_path()).ok()?;
    let v: toml::Value = text.parse().ok()?;
    let t = v.get("google")?;
    let tokens = crate::gdrive::Tokens {
        access_token: t
            .get("access_token")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        refresh_token: t
            .get("refresh_token")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        expires_at: t
            .get("expires_at")
            .and_then(|x| x.as_integer())
            .unwrap_or(0),
        email: t
            .get("email")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    };
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

/// Persist Google tokens, keeping any other keys already in the file.
pub fn save_tokens(tokens: &crate::gdrive::Tokens) -> Result<(), String> {
    let path = sync_path();
    let mut root: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();

    let mut google = toml::Table::new();
    google.insert("access_token".into(), tokens.access_token.clone().into());
    google.insert("refresh_token".into(), tokens.refresh_token.clone().into());
    google.insert("expires_at".into(), tokens.expires_at.into());
    google.insert("email".into(), tokens.email.clone().into());
    root.insert("google".into(), toml::Value::Table(google));

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = toml::to_string_pretty(&root).map_err(|e| format!("encode: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write: {e}"))?;
    restrict_permissions(&path);
    Ok(())
}

/// Remove the stored tokens (sign out).
pub fn clear_tokens() -> Result<(), String> {
    let path = sync_path();
    let mut root: toml::Table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default();
    root.remove("google");
    let text = toml::to_string_pretty(&root).map_err(|e| format!("encode: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("write: {e}"))?;
    Ok(())
}

/// Make the credential file owner-only. No-op on Windows, where the file
/// inherits the user profile's ACL, which is already per-user.
fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;
    #[test]
    fn tokens_round_trip_through_the_config_file() {
        let t = crate::gdrive::Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 123,
            email: "me@example.com".into(),
        };
        save_tokens(&t).unwrap();
        let back = load_tokens().expect("tokens must survive a write/read");
        assert_eq!(back.refresh_token, "r");
        assert_eq!(back.email, "me@example.com");
        assert_eq!(back.expires_at, 123);
        clear_tokens().unwrap();
    }

    #[test]
    fn an_empty_token_set_is_not_a_sign_in() {
        let t = crate::gdrive::Tokens::default();
        assert!(t.is_empty());
    }
}
