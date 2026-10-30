//! What actually travels between devices: the sync envelope.
//!
//! ## Why an envelope and not just the data file
//!
//! The first version uploaded `Data` — sessions and tasks — as a bare JSON
//! object. That was enough to keep two devices' *history* in step, but it left
//! the user's actual preferences behind: set 50-minute blocks and a Santur
//! chime on the desktop, open the laptop, and you got 25 minutes and silence.
//! "Sync" that does not carry your settings is half a sync, and the half it
//! drops is the half the user notices first.
//!
//! So the file on Drive is now an envelope: a schema version, a revision, who
//! wrote it, the data, and the portable settings. The envelope is what makes
//! the rest possible — a version so an old build cannot silently misread a new
//! file, a revision so two devices can tell whether anything moved, and a
//! device name so the status line can say *where* the last change came from.
//!
//! ## Why settings are merged per field
//!
//! Settings are not one value, they are twenty. Merging the block as a whole
//! ("newest wins") means changing the theme on the laptop silently reverts the
//! focus duration you set on the desktop a minute earlier — the classic
//! settings-sync bug. Each field therefore carries its own stamp, and the
//! merge picks a winner per field. Two devices editing two different settings
//! both keep their change.
//!
//! ## Why some settings are deliberately not synced
//!
//! `PORTABLE` is a whitelist, not a blacklist, because the failure mode of
//! guessing wrong is bad in one direction only. Shell automation, global
//! hotkeys, compact mode and the sync switches describe *this machine*: a
//! `pactl` command or a `<Super>p` accelerator pushed from a Linux desktop to
//! a Mac is at best inert and at worst surprising. A whitelist means a new
//! setting is machine-local until someone decides otherwise, which is the safe
//! default.

use crate::state::settings::Settings;
use crate::state::store::Data;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Envelope schema version. Bumped when the meaning of a field changes, so a
/// reader can refuse a file it would otherwise misread.
pub const SCHEMA: u32 = 2;

/// Settings that describe the person rather than the machine.
///
/// Everything here is a preference that should feel the same wherever you sign
/// in. Everything absent is per-machine on purpose (see the module docs).
pub const PORTABLE: &[&str] = &[
    "work_minutes",
    "short_break_minutes",
    "long_break_minutes",
    "long_break_interval",
    "auto_start_next",
    "auto_start_on_launch",
    "daily_goal_minutes",
    "theme",
    "era",
    "lang",
    "animations",
    "sound_enabled",
    "sound_theme",
    "tick_sound",
    "sound_volume",
];

// ---------------------------------------------------------------------------
// Portable settings
// ---------------------------------------------------------------------------

/// The portable slice of `Settings`, with a per-field stamp.
///
/// `values` holds the setting as TOML rather than a struct with twenty named
/// fields, so adding a setting to `PORTABLE` needs no new merge code and no new
/// field here. `stamps` is the per-field clock the merge reads.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SyncSettings {
    #[serde(default)]
    pub values: BTreeMap<String, toml::Value>,
    /// Unix seconds when this device last changed each field.
    #[serde(default)]
    pub stamps: BTreeMap<String, i64>,
}

impl SyncSettings {
    /// The portable slice of `settings`, with no stamps.
    ///
    /// Used to seed a fresh state file: the values are known, but nothing has
    /// been *changed* yet, so nothing should claim to be newer than the other
    /// device's copy.
    #[cfg(test)]
    pub fn snapshot(settings: &Settings) -> Self {
        Self {
            values: portable_table(settings),
            stamps: BTreeMap::new(),
        }
    }

    /// Fold a local settings value in, stamping only the fields that changed.
    ///
    /// This is what makes the merge honest without hooking every control in
    /// the UI: a field whose value differs from the last one we saw is a field
    /// the user just edited, so it gets the current time. A field that is
    /// unchanged keeps its old stamp and therefore loses to a newer edit made
    /// elsewhere.
    ///
    /// Call this *after* applying a remote merge, never before: applying the
    /// remote values first means they compare equal here and are not
    /// re-stamped as if this device had just changed them.
    pub fn absorb(&mut self, settings: &Settings, now: i64) {
        for (key, value) in portable_table(settings) {
            if self.values.get(&key) == Some(&value) {
                continue;
            }
            self.values.insert(key.clone(), value);
            self.stamps.insert(key, now);
        }
    }

    /// Record the current values as the baseline, claiming nothing about when
    /// they were set.
    ///
    /// This runs once, at startup, before any sync. Without it a device has no
    /// baseline, so the first `absorb` would see every value as changed and
    /// stamp all of them "now" — which would let a brand-new install push its
    /// defaults over the settings the user already chose on their other
    /// machine, and would let any device win every conflict simply by being
    /// the first to sync after a restart.
    ///
    /// An unstamped value always loses to a stamped one, which is exactly the
    /// behaviour a baseline needs.
    pub fn seed(&mut self, settings: &Settings) {
        if !self.values.is_empty() {
            return;
        }
        self.values = portable_table(settings);
    }

    /// Field-wise merge. Commutative and idempotent, like the data merge.
    ///
    /// For each field the higher stamp wins. Equal stamps mean both devices
    /// wrote the same field in the same second, which is rare and unresolvable
    /// by time; the tie is broken by comparing the values, so both devices
    /// still land on the same answer instead of each keeping its own.
    pub fn merge(&self, other: &Self) -> Self {
        let mut out = SyncSettings::default();
        let keys: std::collections::BTreeSet<&String> =
            self.values.keys().chain(other.values.keys()).collect();
        for key in keys {
            let mine = self.values.get(key);
            let theirs = other.values.get(key);
            let picked = match (mine, theirs) {
                (Some(a), Some(b)) => {
                    let sa = self.stamps.get(key).copied().unwrap_or(0);
                    let sb = other.stamps.get(key).copied().unwrap_or(0);
                    if sa != sb {
                        if sa > sb {
                            a
                        } else {
                            b
                        }
                    } else if value_key(a) >= value_key(b) {
                        a
                    } else {
                        b
                    }
                }
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => continue,
            };
            out.values.insert(key.clone(), picked.clone());
            let stamp = self
                .stamps
                .get(key)
                .copied()
                .unwrap_or(0)
                .max(other.stamps.get(key).copied().unwrap_or(0));
            out.stamps.insert(key.clone(), stamp);
        }
        out
    }

    /// Write these values into a live `Settings` struct.
    ///
    /// Only portable keys are touched, so a merge can never reach in and change
    /// this machine's hotkeys or automation. The result is sanitised by the
    /// caller, which is where an out-of-range value from a newer build gets
    /// clamped instead of trusted.
    pub fn apply(&self, settings: &mut Settings) {
        let mut table = match toml::Value::try_from(&*settings) {
            Ok(toml::Value::Table(t)) => t,
            _ => return,
        };
        for (key, value) in &self.values {
            if PORTABLE.contains(&key.as_str()) {
                table.insert(key.clone(), value.clone());
            }
        }
        if let Ok(merged) = toml::Value::Table(table).try_into::<Settings>() {
            *settings = merged;
        }
    }

    /// True when the other device has not sent any portable setting yet.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// The portable keys of `settings`, as TOML.
fn portable_table(settings: &Settings) -> BTreeMap<String, toml::Value> {
    let mut out = BTreeMap::new();
    let Ok(toml::Value::Table(table)) = toml::Value::try_from(settings) else {
        return out;
    };
    for (key, value) in table {
        if PORTABLE.contains(&key.as_str()) {
            out.insert(key, value);
        }
    }
    out
}

/// A stable string for a value, used only to break an equal-stamp tie.
fn value_key(v: &toml::Value) -> String {
    v.to_string()
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

/// One complete sync file: everything a second device needs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    /// Schema version of this file.
    #[serde(default = "schema_version")]
    pub schema: u32,
    /// Increments on every write. Two devices can compare this to tell whether
    /// the file moved since they last looked, without diffing the payload.
    #[serde(default)]
    pub revision: u64,
    /// Unix seconds of the last write.
    #[serde(default)]
    pub updated_at: i64,
    /// Which device wrote it last.
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub data: Data,
    #[serde(default)]
    pub settings: SyncSettings,
}

fn schema_version() -> u32 {
    SCHEMA
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            revision: 0,
            updated_at: 0,
            device_id: String::new(),
            device_name: String::new(),
            data: Data::default(),
            settings: SyncSettings::default(),
        }
    }
}

impl Envelope {
    /// Read a sync file, accepting the pre-envelope format.
    ///
    /// A file written by the first release is a bare `Data` object. It has no
    /// `schema` key, so it can be told apart from an envelope without guessing
    /// — and it is read as "data, no settings", which is exactly what it holds.
    /// Refusing it would strand every existing user's history on Drive.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self::default());
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("sync file is not valid JSON: {e}"))?;
        if value.get("schema").is_none() {
            // Legacy: a bare Data object.
            let data: Data = serde_json::from_value(value)
                .map_err(|e| format!("legacy sync file is not valid data: {e}"))?;
            return Ok(Self {
                data,
                ..Self::default()
            });
        }
        let env: Envelope = serde_json::from_value(value)
            .map_err(|e| format!("sync file is not a valid envelope: {e}"))?;
        if env.schema > SCHEMA {
            // A newer build wrote this. Reading it as if it were ours would
            // drop whatever fields we do not know about on the next push, so
            // refuse and let the user update instead of quietly losing data.
            return Err(format!(
                "this sync file was written by a newer version of Pangan (schema {}); update the app",
                env.schema
            ));
        }
        Ok(env)
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("encode: {e}"))
    }

    /// True when this envelope holds nothing worth uploading.
    #[allow(dead_code)] // test support, not called from the app
    pub fn is_empty(&self) -> bool {
        self.data.sessions.is_empty()
            && self.data.tasks.is_empty()
            && self.settings.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Per-device state
// ---------------------------------------------------------------------------

/// What this device remembers between runs.
///
/// The settings stamps have to survive a restart. If they were rebuilt on every
/// launch, every launch would look like "the user just changed every setting"
/// and this device would clobber the other one's edits on the first sync.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeviceState {
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    /// The last settings this device saw, with their stamps.
    #[serde(default)]
    pub settings: SyncSettings,
    /// The revision this device last wrote or read.
    #[serde(default)]
    pub last_revision: u64,
    /// Whether this device has ever completed a sync that had settings to
    /// learn from.
    ///
    /// A brand-new install has default settings and no history, so it must
    /// adopt the cloud's settings rather than push its defaults over them.
    /// That is a one-time bootstrap, not a merge rule: after it, this device's
    /// settings are a real choice and compete on stamps like any other.
    #[serde(default)]
    pub bootstrapped: bool,
    /// Unix seconds of the last successful exchange, so the settings card can
    /// say when sync last worked even after a restart.
    ///
    /// Without this the card could only report the current process's cycles,
    /// and a user who reopened the app after a day of failures saw a blank
    /// "last sync" and had no way to tell whether anything had ever worked.
    #[serde(default)]
    pub last_ok: i64,
}

fn state_path() -> std::path::PathBuf {
    crate::state::store::Store::data_dir().join("sync-state.json")
}

impl DeviceState {
    pub fn load(settings: &Settings) -> Self {
        let mut state: DeviceState = std::fs::read_to_string(state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        state.ensure_identity();
        // Establish the baseline now, before the first sync. See `seed`.
        state.settings.seed(settings);
        state
    }

    pub fn save(&self) {
        let path = state_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// Give this device an id and a name, once.
    ///
    /// The id is what tells "my own write" apart from "the other machine
    /// wrote"; the name is only ever shown to the user. Neither is a secret,
    /// so they live in a plain file next to the data.
    pub fn ensure_identity(&mut self) {
        if self.device_id.is_empty() {
            self.device_id = generate_id();
        }
        if self.device_name.is_empty() {
            self.device_name = hostname();
        }
    }
}

/// A short, collision-resistant id. Not a security token — just a label.
fn generate_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    // A cheap mix so two devices starting in the same nanosecond (they cannot,
    // but two processes on one machine can) still differ.
    let mut x = nanos ^ (pid << 64);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    format!("{:016x}", (x as u64) ^ (nanos as u64))
}

/// The machine's name, best effort. Falls back to a neutral label rather than
/// failing: a device with no name still syncs, it just says less.
fn hostname() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "Pangan device".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(work: u64, theme: &str) -> Settings {
        let mut s = Settings::default();
        s.work_minutes = work;
        s.theme = theme.to_string();
        s
    }

    /// Two devices that have both synced once, so each has a baseline and is
    /// not claiming every value as freshly edited.
    fn settled(settings: &Settings) -> SyncSettings {
        let mut s = SyncSettings::default();
        s.absorb(settings, 0);
        s
    }

    #[test]
    fn a_legacy_bare_data_file_is_still_readable() {
        // Every existing user has one of these on Drive. Refusing it would
        // strand their history.
        let legacy = r#"{"sessions":[{"at":100,"minutes":25}],"tasks":[],"next_task_id":1}"#;
        let env = Envelope::parse(legacy).unwrap();
        assert_eq!(env.data.sessions.len(), 1);
        assert!(env.settings.is_empty(), "a legacy file carries no settings");
        assert_eq!(env.schema, SCHEMA);
    }

    #[test]
    fn an_empty_file_is_an_empty_envelope() {
        assert!(Envelope::parse("").unwrap().is_empty());
        assert!(Envelope::parse("   ").unwrap().is_empty());
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused_rather_than_misread() {
        let future = format!(r#"{{"schema":{},"data":{{}}}}"#, SCHEMA + 1);
        let err = Envelope::parse(&future).unwrap_err();
        assert!(err.contains("newer version"), "got: {err}");
    }

    #[test]
    fn an_envelope_round_trips_through_json() {
        let mut env = Envelope {
            revision: 7,
            updated_at: 1234,
            device_name: "laptop".into(),
            ..Default::default()
        };
        env.settings = SyncSettings::snapshot(&settings_with(50, "light"));
        let back = Envelope::parse(&env.to_json().unwrap()).unwrap();
        assert_eq!(back.revision, 7);
        assert_eq!(back.device_name, "laptop");
        assert_eq!(
            back.settings.values.get("work_minutes"),
            Some(&toml::Value::Integer(50))
        );
    }

    #[test]
    fn only_portable_keys_are_captured() {
        let mut s = Settings::default();
        s.work_minutes = 50;
        s.inhibit_enabled = false; // machine-local
        s.sync_enabled = true; // machine-local
        let snap = SyncSettings::snapshot(&s);
        assert!(snap.values.contains_key("work_minutes"));
        assert!(
            !snap.values.contains_key("inhibit_enabled"),
            "a per-machine switch must not travel"
        );
        assert!(!snap.values.contains_key("sync_enabled"));
    }

    #[test]
    fn applying_remote_settings_leaves_machine_local_ones_alone() {
        let mut local = Settings::default();
        local.inhibit_enabled = false;
        local.work_minutes = 25;

        let mut remote = Settings::default();
        remote.work_minutes = 50;
        remote.inhibit_enabled = true;
        let snap = SyncSettings::snapshot(&remote);
        snap.apply(&mut local);

        assert_eq!(local.work_minutes, 50, "the portable value must arrive");
        assert!(
            !local.inhibit_enabled,
            "the per-machine switch must not be overwritten"
        );
    }

    #[test]
    fn two_devices_editing_two_different_settings_both_keep_their_change() {
        // The bug a whole-block merge causes: the laptop's theme change would
        // revert the desktop's duration change made a second earlier.
        let baseline = settings_with(25, "dark");
        let mut desktop = settled(&baseline);
        let mut laptop = settled(&baseline);

        // Now each changes a different field.
        desktop.absorb(&settings_with(50, "dark"), 100);
        laptop.absorb(&settings_with(25, "light"), 200);

        let merged = desktop.merge(&laptop);
        assert_eq!(
            merged.values.get("work_minutes"),
            Some(&toml::Value::Integer(50)),
            "the desktop set 50 minutes and the laptop never touched it"
        );
        assert_eq!(
            merged.values.get("theme"),
            Some(&toml::Value::String("light".into())),
            "the laptop's theme change is newer"
        );
    }

    #[test]
    fn a_fresh_device_seeds_without_claiming_its_values_are_new() {
        // A brand-new install must adopt the cloud's settings, not push its
        // own defaults over them. Seeding with no stamp is what makes that
        // work: an unstamped value always loses to a stamped one.
        let mut fresh = SyncSettings::default();
        fresh.seed(&Settings::default());
        assert!(
            fresh.stamps.is_empty(),
            "seeding a baseline must not claim every setting was just edited"
        );

        let mut experienced = settled(&Settings::default());
        experienced.absorb(&settings_with(50, "dark"), 100);

        let merged = fresh.merge(&experienced);
        assert_eq!(
            merged.values.get("work_minutes"),
            Some(&toml::Value::Integer(50)),
            "the fresh device must adopt the cloud's value"
        );
    }

    #[test]
    fn the_newer_edit_of_the_same_field_wins() {
        let baseline = settings_with(25, "dark");
        let mut a = settled(&baseline);
        let mut b = settled(&baseline);
        a.absorb(&settings_with(50, "dark"), 100);
        b.absorb(&settings_with(30, "dark"), 200);
        assert_eq!(
            a.merge(&b).values.get("work_minutes"),
            Some(&toml::Value::Integer(30))
        );
        assert_eq!(
            b.merge(&a).values.get("work_minutes"),
            Some(&toml::Value::Integer(30)),
            "the merge must be commutative"
        );
    }

    #[test]
    fn absorbing_an_unchanged_setting_does_not_restamp_it() {
        // If a sync re-stamped everything it touched, this device would win
        // every conflict forever just by syncing often.
        let mut s = settled(&settings_with(25, "dark"));
        s.absorb(&settings_with(50, "dark"), 100);
        s.absorb(&settings_with(50, "dark"), 999);
        assert_eq!(s.stamps.get("work_minutes"), Some(&100));
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let baseline = settings_with(25, "dark");
        let mut a = settled(&baseline);
        let mut b = settled(&baseline);
        a.absorb(&settings_with(50, "dark"), 100);
        b.absorb(&settings_with(25, "light"), 200);
        let once = a.merge(&b);
        let twice = once.merge(&b);
        assert_eq!(once, twice, "a stable state must not keep moving");
    }

    #[test]
    fn an_equal_stamp_tie_resolves_the_same_way_on_both_devices() {
        let baseline = settings_with(25, "dark");
        let mut a = settled(&baseline);
        let mut b = settled(&baseline);
        a.absorb(&settings_with(50, "dark"), 100);
        b.absorb(&settings_with(30, "dark"), 100);
        assert_eq!(a.merge(&b), b.merge(&a));
    }

    #[test]
    fn a_device_gets_an_identity_once_and_keeps_it() {
        let mut d = DeviceState::default();
        d.ensure_identity();
        let id = d.device_id.clone();
        assert_eq!(id.len(), 16);
        d.ensure_identity();
        assert_eq!(d.device_id, id, "the id must be stable across runs");
        assert!(!d.device_name.is_empty());
    }

    #[test]
    fn two_devices_get_different_ids() {
        let mut a = DeviceState::default();
        let mut b = DeviceState::default();
        a.ensure_identity();
        b.ensure_identity();
        assert_ne!(a.device_id, b.device_id);
    }
}
