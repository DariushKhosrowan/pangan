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
            period: "Today".into(),
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
pub fn try_sync(row: &SyncRow) -> String {
    let Some(cfg) = load_config() else {
        return "Google sync disabled (no token). See README.".into();
    };
    match push_row(&cfg, row) {
        Ok(()) => "Synced to Google Sheets ✓".into(),
        Err(e) => format!("Sync error: {e}"),
    }
}
