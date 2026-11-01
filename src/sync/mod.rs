//! Cloud sync: what travels between devices, how it merges, and how it talks
//! to Google Drive.
//!
//! * [`syncfile`] — the envelope written to Drive (schema, revision, payload).
//! * [`merge`] — per-record / per-field merge, so two devices do not clobber.
//! * [`cloudsync`] — the engine that drives automatic sync.
//! * [`gdrive`] / [`oauth`] — the Drive REST calls and the device-flow sign-in.
//! * [`backup`] — token storage plus the manual full-snapshot backup.

pub mod backup;
pub mod cloudsync;
pub mod gdrive;
pub mod merge;
pub mod oauth;
pub mod syncfile;
