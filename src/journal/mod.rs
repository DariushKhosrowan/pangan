//! The written record: two lines per focus session, one before it and one after.
//!
//! A Pomodoro timer tells you *that* you worked. It never tells you *what* you
//! did, and after twelve blocks in a day nobody remembers. This module keeps
//! the record, in two halves:
//!
//! * [`decree`] — written *before* the block: one line saying what the block is
//!   for. A promise, short enough to sit under the clock while it runs.
//! * [`inscription`] — written *after* the block: one line saying what it
//!   actually produced. Permanent and searchable.
//! * [`reckoning`] — the read-back: over a span, how many promises were kept,
//!   which ones were not, and the words the span was actually about. A record
//!   nobody reads back is an archive, not a record.
//!
//! Kept as a pair on the same session, they answer both halves of the question.
//! A decree with no inscription facing it is the most useful row in the whole
//! archive: it is a block that was spoken for and did not deliver.
//!
//! The name is the point. Persian kings did not trust memory either: what was
//! decreed and what was accomplished went into stone, so both would outlast the
//! day they described. Pangan's version is a clay tablet you can search.
//!
//! Everything here is pure: no file I/O, no UI, no clock reads beyond the ones
//! explicitly passed in. That is what lets the rules be tested directly.

pub mod decree;
pub mod inscription;
pub mod orphans;
pub mod reckoning;
pub mod standing_promise;
pub mod verdict;
pub mod weekly_review;

// The inscription rules are called from the app layer by bare name because they
// are the older half of the pair; the decree's are reached through `decree::`
// so the two sets of `create`/`validate_text` cannot be confused at a call site.
pub use decree::Decree;
pub use inscription::{create, day_key, search, Inscription, MAX_CHARS};
pub use verdict::{Verdict, VerdictKind};

/// Render the whole record — promise and outcome — as Markdown.
///
/// Rows are built from both halves for the same reason the archive screen is:
/// a decree whose session produced no inscription is a fact worth exporting.
/// The two are joined on `session_at`, which is what makes them a pair rather
/// than two unrelated lists.
pub fn export_record(inscriptions: &[Inscription], decrees: &[Decree], lang_fa: bool) -> String {
    use std::collections::BTreeMap;

    let mut by_session: BTreeMap<i64, (Option<&Inscription>, Option<&Decree>)> = BTreeMap::new();
    for ins in inscriptions.iter().filter(|i| !i.deleted) {
        by_session.entry(ins.session_at).or_default().0 = Some(ins);
    }
    for dec in decrees.iter().filter(|d| !d.deleted) {
        by_session.entry(dec.session_at).or_default().1 = Some(dec);
    }

    let title = if lang_fa {
        "# سنگ‌نوشته‌های پنگان"
    } else {
        "# Pangan inscriptions"
    };
    let mut out = String::new();
    out.push_str(title);
    out.push_str("\n\n");

    // Oldest first: an export is a document to read downwards, the way a day
    // actually happened, unlike the archive screen which is newest first.
    let mut current_day = String::new();
    for (session_at, (ins, dec)) in by_session {
        let day = day_key(session_at);
        if day != current_day {
            if !current_day.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("## {day}\n\n"));
            current_day = day;
        }
        let time = chrono::DateTime::from_timestamp(session_at, 0)
            .map(|u| u.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_else(|| "--:--".to_string());
        if let Some(d) = dec {
            out.push_str(&format!(
                "- {time} — **{}:** {}\n",
                if lang_fa { "فرمان" } else { "decree" },
                d.text
            ));
        }
        match ins {
            Some(i) => out.push_str(&format!("  {}\n", i.text)),
            None => out.push_str(&format!(
                "  {}\n",
                if lang_fa {
                    "— بی‌نوشته ماند —"
                } else {
                    "— nothing written —"
                }
            )),
        }
    }
    out
}

#[cfg(test)]
mod record_tests {
    use super::*;

    /// 2026-10-31 09:00:00 +0330.
    const T: i64 = 1_793_431_800;

    fn pair(
        session_at: i64,
        decree: Option<&str>,
        inscription: Option<&str>,
    ) -> (Vec<Inscription>, Vec<Decree>) {
        let mut ins = Vec::new();
        let mut dec = Vec::new();
        if let Some(d) = decree {
            dec.push(decree::create(session_at, session_at - 1500, d).unwrap());
        }
        if let Some(i) = inscription {
            ins.push(inscription::create(session_at, session_at, i).unwrap());
        }
        (ins, dec)
    }

    #[test]
    fn export_puts_the_decree_above_the_inscription_it_answers() {
        let (ins, dec) = pair(T, Some("write the report"), Some("wrote the report"));
        let md = export_record(&ins, &dec, false);
        let d = md.find("write the report").unwrap();
        let i = md.find("wrote the report").unwrap();
        assert!(d < i, "the promise must read before the outcome");
        assert!(md.contains("decree"), "the promise line is labelled");
    }

    #[test]
    fn export_keeps_a_decree_whose_session_produced_nothing() {
        let (ins, dec) = pair(T, Some("draft the contract"), None);
        let md = export_record(&ins, &dec, false);
        assert!(md.contains("draft the contract"), "the intention is kept");
        assert!(
            md.contains("nothing written"),
            "and the absence is stated, not silently dropped"
        );
    }

    #[test]
    fn export_is_chronological_across_both_halves() {
        let (mut ins, mut dec) = pair(T, Some("first intent"), Some("first done"));
        let (i2, d2) = pair(T + 3 * 86_400, Some("third intent"), Some("third day done"));
        let (i3, d3) = pair(T - 7_200, Some("zeroth intent"), Some("zeroth done"));
        ins.extend(i2);
        dec.extend(d2);
        ins.extend(i3);
        dec.extend(d3);

        let md = export_record(&ins, &dec, false);
        let zeroth = md.find("zeroth done").unwrap();
        let first = md.find("first done").unwrap();
        let third = md.find("third day done").unwrap();
        assert!(zeroth < first && first < third, "oldest first");
        // `T - 7200` is two hours earlier on the same day, so there are two
        // headings, not three. The assertion is about grouping, not counting.
        assert_eq!(md.matches("## ").count(), 2, "one heading per distinct day");
    }

    #[test]
    fn a_discarded_half_is_left_out_of_the_export() {
        let (ins, mut dec) = pair(T, Some("an intention I withdrew"), Some("the work"));
        dec[0].deleted = true;
        let md = export_record(&ins, &dec, false);
        assert!(
            !md.contains("an intention I withdrew"),
            "a discarded decree must not be exported"
        );
        assert!(md.contains("the work"), "the surviving half still is");
    }
}
