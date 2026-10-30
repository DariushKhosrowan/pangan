//! The inscription archive: one written line per finished focus session.
//!
//! A Pomodoro timer tells you *that* you worked. It never tells you *what* you
//! did, and after twelve blocks in a day nobody remembers. This module keeps
//! the record — a single sentence, attached to the session that produced it,
//! permanent and searchable.
//!
//! The name is the point. Persian kings did not trust memory either: the record
//! of what was accomplished was cut into stone so it would outlast the day it
//! described. Pangan's version is a clay tablet you can search.
//!
//! Everything here is pure: no file I/O, no UI, no clock reads beyond the one
//! explicitly passed in. That is what lets the rules be tested directly.

pub mod inscription;

pub use inscription::{
    create, day_of, export_markdown, search, Inscription, MAX_CHARS,
};
