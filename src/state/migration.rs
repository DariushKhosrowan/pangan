//! Schema versioning and the migration seam.
//!
//! Every breaking change to `Data` is currently a silent data-loss bomb: a
//! field added or renamed deserializes older stores to partial values under
//! `#[serde(default)]`, and the user's journal quietly degrades. This module
//! installs the safety net once: a version tag on every record, a pure
//! predicate for the load path, and a single `migrate` function where future
//! versions add a step.
//!
//! Today `migrate` is the identity function at `CURRENT_SCHEMA_VERSION`. Its
//! value is the *seam*: the one place a future version bump must touch, and
//! the guarantee that a file from a *newer* app is refused rather than
//! silently misread.

use super::store::Data;

/// The schema version the running app writes and understands.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Why a record cannot be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// The file was written by a newer app (`schema_version > CURRENT`).
    FutureVersion(u32),
    /// The file's schema version is absent or inconsistent with its contents.
    #[allow(dead_code)] // part of the seam's contract; unused until a migration can fail
    Corrupt(String),
}

/// True when `data` holds the current schema version and needs no migration.
#[allow(dead_code)] // test support: the load path uses `migrate`
pub fn needs_migration(data: &Data) -> bool {
    data.schema_version != CURRENT_SCHEMA_VERSION
}

/// True when `data` can be loaded as-is (it is at the current version).
#[allow(dead_code)] // test support: the load path uses `migrate`
pub fn schema_is_compatible(data: &Data) -> bool {
    data.schema_version == CURRENT_SCHEMA_VERSION
}

/// Bring `data` up to the current schema version.
///
/// At the current version this is the identity — no migrations exist yet.
/// Future versions add arms, each stepping a record forward by one version.
/// A record from a *newer* app is rejected: the running app cannot know what
/// the newer fields mean, and silently accepting it would corrupt the user's
/// data the first time it is re-saved.
pub fn migrate(mut data: Data) -> Result<Data, MigrationError> {
    match data.schema_version {
        v if v > CURRENT_SCHEMA_VERSION => Err(MigrationError::FutureVersion(v)),
        // At or below the current version: there are no stepwise migrations
        // yet, and every field is backward-compatible via `#[serde(default)]`.
        // Stamp the record at the current version so a future bump knows it
        // has already been normalised.
        _ => {
            data.schema_version = CURRENT_SCHEMA_VERSION;
            Ok(data)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_data_has_current_schema_version() {
        assert_eq!(Data::default().schema_version, CURRENT_SCHEMA_VERSION);
        assert!(schema_is_compatible(&Data::default()));
        assert!(!needs_migration(&Data::default()));
    }

    #[test]
    fn needs_migration_true_for_older() {
        let mut d = Data::default();
        d.schema_version = CURRENT_SCHEMA_VERSION.saturating_sub(1);
        assert!(needs_migration(&d));
        assert!(!schema_is_compatible(&d));
    }

    #[test]
    fn migrate_identity_at_current_version() {
        let d = Data::default();
        let out = migrate(d).unwrap();
        assert_eq!(out.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn migrate_rejects_future_version() {
        let mut d = Data::default();
        d.schema_version = CURRENT_SCHEMA_VERSION + 1;
        match migrate(d) {
            Ok(_) => panic!("future version must be rejected"),
            Err(MigrationError::FutureVersion(v)) => {
                assert_eq!(v, CURRENT_SCHEMA_VERSION + 1);
            }
            Err(e) => panic!("expected FutureVersion, got {e:?}"),
        }
    }

    #[test]
    fn migrate_accepts_older_version_and_stamps_current() {
        let mut d = Data::default();
        d.schema_version = 0;
        let out = migrate(d).unwrap();
        assert_eq!(out.schema_version, CURRENT_SCHEMA_VERSION);
    }
}
