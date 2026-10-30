//! Everything date-related: the Jalali calendar, the Sasanian day names,
//! the "on this day" table (embedded, refreshable), and the imperial chronicle.
//!
//! `calendar::calendar` re-exports the date math at module level so callers can
//! keep writing `calendar::to_gregorian` rather than
//! `calendar::calendar::to_gregorian`.

pub mod calendar;
pub mod chronicle;
pub mod events;
pub mod refresh;

pub use calendar::*;
