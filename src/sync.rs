//! Google Cloud sync adapter.
//!
//! Pangan synchronizes its performance snapshot to a Google Sheet via the
//! Sheets API v4. Authentication uses a Bearer access token (scope
//! `https://www.googleapis.com/auth/spreadsheets`) which the user obtains with
//! the gcloud CLI:
//!
//!   gcloud auth print-access-token --scopes=https://www.googleapis.com/auth/spreadsheets
//!
//! and places into the sync config (see `load_config`). When no token is
//! configured the adapter is inert and reports `disconnected`.

use serde::Serialize;

/// Values parsed from `~/.config/pangan/sync.toml`.
#[derive(Clone, Debug)]
pub struct SyncCfg {
    pub spreadsheet_id: String,
    pub sheet: String,
    pub token: String,
}

/// A row pushed to the Google Sheet.
#[derive(Clone, Debug, Serialize)]
pub struct SyncRow {
    pub ts: String,
    pub period: String,
    pub sessions: u64,
    pub minutes: u64,
    pub avg_min: u64,
    pub consistency_pct: u32,
    pub streak_days: u64,
    pub score: u32,
    pub tasks_open: u64,
}

impl Default for SyncRow {
    fn default() -> Self {
        Self {
            ts: String::new(),
            period: "امروز".into(),
            sessions: 0,
            minutes: 0,
            avg_min: 0,
            consistency_pct: 0,
            streak_days: 0,
            score: 0,
            tasks_open: 0,
        }
    }
}

/// Whether a sync config is present (even if not yet validated).
pub fn has_config() -> bool {
    load_config().is_some()
}

/// Load sync config. Returns None when sync is fully disabled.
pub fn load_config() -> Option<SyncCfg> {
    let path = crate::store::Store::data_dir().join("sync.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    let v: toml::Value = text.parse().ok()?;
    let spreadsheet_id = v.get("spreadsheet_id")?.as_str()?.to_string();
    let sheet = v
        .get("sheet")
        .and_then(|s| s.as_str())
        .unwrap_or("Pangan")
        .to_string();
    let token = v.get("token")?.as_str()?.to_string();
    if spreadsheet_id.is_empty() || token.is_empty() {
        return None;
    }
    Some(SyncCfg {
        spreadsheet_id,
        sheet,
        token,
    })
}

/// Push one row to the configured Google Sheet. Returns Ok(()) on success.
pub fn push_row(cfg: &SyncCfg, row: &SyncRow) -> Result<(), String> {
    let range = format!("{}!A1", cfg.sheet);
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}:append?valueInputOption=RAW",
        cfg.spreadsheet_id, range
    );
    // valueInputOption is already injected; body is a Values resource.
    let values: Vec<Vec<String>> = vec![vec![
        row.ts.clone(),
        row.period.clone(),
        row.sessions.to_string(),
        row.minutes.to_string(),
        row.avg_min.to_string(),
        row.consistency_pct.to_string(),
        row.streak_days.to_string(),
        row.score.to_string(),
        row.tasks_open.to_string(),
    ]];

    let body = serde_json::json!({ "values": values });
    let resp = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", cfg.token))
        .send_json(body);

    match resp {
        Ok(r) if r.status() == 200 => Ok(()),
        Ok(r) => {
            let status = r.status();
            let s = r.into_body().read_to_string().unwrap_or_default();
            Err(format!("Sheets API responded HTTP {}: {}", status, s))
        }
        Err(e) => Err(format!("request failed: {e}")),
    }
}

/// Attempt a sync with best-effort error reporting. Returns a status string.
///
/// `en` selects the language of that status. The status is shown in the
/// settings screen, which is otherwise fully translated, so these three
/// strings were the last Persian left in an English window.
pub fn try_sync(row: &SyncRow, en: bool) -> String {
    let Some(cfg) = load_config() else {
        return if en {
            "Google sync is off (no token). See the guide.".into()
        } else {
            "همگام‌سازی گوگل خاموش است (نشانه‌ای نیست). راهنما را ببینید.".into()
        };
    };
    match push_row(&cfg, row) {
        Ok(()) => {
            if en {
                "Synced with Google Sheets ✓".into()
            } else {
                "با برگه‌های گوگل همگام شد ✓".into()
            }
        }
        Err(e) => {
            if en {
                format!("Sync error: {e}")
            } else {
                format!("خطای همگام‌سازی: {e}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Two-way cloud backup: settings + data snapshot as a single JSON cell.
// ---------------------------------------------------------------------------

/// A full app-state backup pushed to the backup sheet tab.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BackupPayload {
    pub app: String,
    pub version: String,
    pub ts: String,
    pub settings: String, // TOML text
    pub data: String,     // JSON text
}

/// Push a full backup payload to a dedicated "PanganBackup" sheet tab.
pub fn push_backup(cfg: &SyncCfg, payload: &BackupPayload) -> Result<(), String> {
    let sheet = "PanganBackup";
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}!A1:append?valueInputOption=RAW",
        cfg.spreadsheet_id, sheet
    );
    let body = serde_json::json!({
        "values": [[
            payload.app,
            payload.version,
            payload.ts,
            payload.settings,
            payload.data,
        ]]
    });
    let resp = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", cfg.token))
        .send_json(body);
    match resp {
        Ok(r) if r.status() == 200 => Ok(()),
        Ok(r) => Err(format!(
            "backup HTTP {}: {}",
            r.status(),
            r.into_body().read_to_string().unwrap_or_default()
        )),
        Err(e) => Err(format!("backup request failed: {e}")),
    }
}

/// Pull the most recent backup row from the "PanganBackup" tab.
pub fn pull_latest_backup(cfg: &SyncCfg) -> Result<BackupPayload, String> {
    let sheet = "PanganBackup";
    let range = format!("{}!A:E", sheet);
    let url = format!(
        "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}?majorDimension=ROWS",
        cfg.spreadsheet_id, range
    );
    let resp = ureq::get(&url)
        .header("Authorization", &format!("Bearer {}", cfg.token))
        .call();
    let text = match resp {
        Ok(r) if r.status() == 200 => r
            .into_body()
            .read_to_string()
            .map_err(|e| format!("read: {e}"))?,
        Ok(r) => {
            return Err(format!(
                "pull HTTP {}: {}",
                r.status(),
                r.into_body().read_to_string().unwrap_or_default()
            ))
        }
        Err(e) => return Err(format!("pull request failed: {e}")),
    };
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("json: {e}"))?;
    let rows = v["values"].as_array().ok_or("no rows")?;
    // Rows are [app, version, ts, settings, data]; take the last non-empty.
    for row in rows.iter().rev() {
        let a = row.as_array().ok_or("bad row")?;
        if a.len() >= 5 {
            let settings = a[3].as_str().unwrap_or("").to_string();
            let data = a[4].as_str().unwrap_or("").to_string();
            if settings.is_empty() && data.is_empty() {
                continue;
            }
            return Ok(BackupPayload {
                app: a
                    .get(0)
                    .and_then(|x| x.as_str())
                    .unwrap_or("pangan")
                    .to_string(),
                version: a.get(1).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                ts: a.get(2).and_then(|x| x.as_str()).unwrap_or("").to_string(),
                settings,
                data,
            });
        }
    }
    Err("no backup found".into())
}
