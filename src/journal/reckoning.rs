//! A read-back of the written record: what was promised, what was delivered.
//!
//! The journal is write-only today. A decree goes in before a block and an
//! inscription after it, and both then sit in the archive where the only way to
//! learn anything from them is to scroll. A record nobody reads back is not a
//! record, it is an archive — which is a different and much less useful object.
//!
//! This module answers the three questions the archive can already answer but
//! never asks:
//!
//! * **Did the day hold?** — how many blocks were spoken for in the span, and
//!   how many of those produced words afterwards. The gap is the number that
//!   matters: a week of honest work is mostly answered decrees, and a week where
//!   a third of the intentions were never written up is a week that was busy in
//!   a way the numbers alone cannot show.
//! * **Which intentions did not land?** — the specific decrees with no
//!   inscription facing them. Not to scold: a promise that did not survive
//!   contact with the day is the most informative row in the archive, and it is
//!   only useful if it is put in front of the person who wrote it.
//! * **What was the week actually about?** — the words that recur across the
//!   decrees. The themes of a week are in the writing, not in the minutes, and
//!   counting them is the cheapest way to find out whether the plan in your head
//!   is the plan in your week.
//!
//! Everything here is pure: a span and three slices in, one summary out. No
//! clock reads, no I/O, no UI. That is what lets the counting be tested directly
//! instead of through a fake file.

use crate::journal::{Decree, Inscription};
use crate::stats::report::Range;
use std::collections::BTreeMap;

/// How many unfulfilled decrees to name. Enough to see a pattern, few enough
/// that the list stays a prompt rather than becoming a scolding wall.
pub const MAX_UNDELIVERED: usize = 5;

/// How many recurring words to show.
pub const MAX_WORDS: usize = 5;

/// A word must appear at least this often to count as a theme.
///
/// One appearance is a word used, not a word that recurs — the list is about
/// what a period was *about*, and a single mention says nothing about that.
pub const MIN_WORD_COUNT: u32 = 2;

/// Words too common to carry meaning, in both languages the app speaks.
///
/// Deliberately short. A long stoplist starts deciding what the user was
/// allowed to mean; this only removes the words that would otherwise top every
/// list. Persian compounds joined with ZWNJ are left whole — the joiner is part
/// of the word, not a separator, so `مى‌روم` is one token and not two.
const STOPWORDS: &[&str] = &[
    // English
    "the", "and", "for", "with", "that", "this", "from", "into", "about", "then", "than", "have",
    "has", "had", "was", "were", "will", "just", "some", "them", "they", "there", "their", "what",
    "when", "which", "after", "before", "over", "also", "only", "more", "must", "make", "made",
    "done", "does", "did", // Persian
    "و", "به", "از", "که", "را", "در", "با", "این", "آن", "تا", "هم", "یک", "بر", "برای", "بود",
    "شد", "شده", "می", "نمی", "های", "ها", "ای", "اما", "اگر", "چون", "دیگر", "همه", "روی", "پس",
    "چه", "کی", "نه", "بله",
];

/// One decree that was written and never answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfulfilled {
    /// Unix seconds when the session that answered this decree ended — the key
    /// it shares with an inscription, and the thing that says which block it
    /// was for. Callers format it; this module does not know about calendars.
    pub session_at: i64,
    /// The promise itself.
    pub text: String,
}

/// A word and how often it appeared across the period's decrees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recurring {
    pub word: String,
    pub count: u32,
}

/// What the span's written record adds up to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeeklyReckoning {
    /// Completed sessions in the span.
    pub sessions: usize,
    /// Intentions written in the span.
    pub spoken: usize,
    /// Intentions in the span that were answered by an inscription.
    pub answered: usize,
    /// Words written after a block in the span, with or without a decree.
    pub written: usize,
    /// Decrees with no inscription facing them, newest first.
    pub unfulfilled: Vec<Unfulfilled>,
    /// How many there were in total, of which `unfulfilled` names at most
    /// [`MAX_UNDELIVERED`].
    pub unfulfilled_total: usize,
    /// Words that recur across the span's decrees, most frequent first.
    pub recurring: Vec<Recurring>,
}

impl WeeklyReckoning {
    /// Intentions the span made and did not keep.
    pub fn unanswered(&self) -> usize {
        self.spoken.saturating_sub(self.answered)
    }

    /// Blocks that produced words nobody asked for — working, but not on the
    /// thing that was planned. Worth counting separately: it is the opposite
    /// failure to an unanswered decree, and lumping the two together would
    /// report both as "inconsistent".
    pub fn unspoken(&self) -> usize {
        self.written.saturating_sub(self.answered)
    }

    /// True when there is nothing to say about this span at all.
    pub fn is_empty(&self) -> bool {
        self.spoken == 0 && self.written == 0
    }
}

/// Sum up the written record over an inclusive span of days.
///
/// The span is the report's own `Range`, so the reckoning follows whatever
/// period the user is looking at and cannot drift into disagreeing with the
/// numbers on the same screen about where a week starts.
///
/// Both halves are matched on `session_at`: that is the key a decree and an
/// inscription share, and matching on `written_at` instead would pair a decree
/// with the inscription from a later block that happened to be written nearby.
pub fn build(
    decrees: &[Decree],
    inscriptions: &[Inscription],
    sessions: &[i64],
    span: Range,
) -> WeeklyReckoning {
    let in_span = |at: i64| day_of(at).map(|d| span.contains(d)).unwrap_or(false);

    // Live rows only: a tombstone is a withdrawal, and counting a withdrawn
    // promise as spoken would make the reckoning accuse the user of work they
    // deliberately called off.
    let spoken: Vec<&Decree> = decrees
        .iter()
        .filter(|d| !d.deleted && in_span(d.session_at))
        .collect();
    let written: Vec<&Inscription> = inscriptions
        .iter()
        .filter(|i| !i.deleted && in_span(i.session_at))
        .collect();

    // Keyed by session, because that is what pairs the two halves. A set rather
    // than a count: two inscriptions for one block is one delivered promise.
    let answered_sessions: std::collections::BTreeSet<i64> =
        written.iter().map(|i| i.session_at).collect();

    let answered = spoken
        .iter()
        .filter(|d| answered_sessions.contains(&d.session_at))
        .count();

    // Newest first: the block that did not land an hour ago is more worth
    // seeing than the one from Monday, and the list is capped anyway.
    let mut unfulfilled: Vec<Unfulfilled> = spoken
        .iter()
        .filter(|d| !answered_sessions.contains(&d.session_at))
        .map(|d| Unfulfilled {
            session_at: d.session_at,
            text: d.text.clone(),
        })
        .collect();
    unfulfilled.sort_by(|a, b| b.session_at.cmp(&a.session_at));

    let unfulfilled_total = unfulfilled.len();
    unfulfilled.truncate(MAX_UNDELIVERED);

    WeeklyReckoning {
        sessions: sessions.iter().filter(|at| in_span(**at)).count(),
        spoken: spoken.len(),
        answered,
        written: written.len(),
        unfulfilled,
        unfulfilled_total,
        recurring: recurring_words(&spoken),
    }
}

/// The words that recur across the given decrees, most frequent first.
///
/// Built from the decrees alone, not from inscriptions: a decree is a plan and
/// is written before the work, so the themes it carries are the themes the user
/// *intended*. Reading the themes out of what was actually written afterwards
/// would answer a different and flattering question.
fn recurring_words(decrees: &[&Decree]) -> Vec<Recurring> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for d in decrees {
        // Within one decree, a word counts once. "Report, report, report" is
        // one intention to write a report, not three, and counting repetitions
        // would let a single emphatic line become the week's whole theme.
        let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for w in tokens(&d.text) {
            if seen.insert(w.clone()) {
                *counts.entry(w).or_insert(0) += 1;
            }
        }
    }

    let mut out: Vec<Recurring> = counts
        .into_iter()
        .filter(|(_, n)| *n >= MIN_WORD_COUNT)
        .map(|(word, count)| Recurring { word, count })
        .collect();
    // Count first, then the word: two words with the same count must come back
    // in the same order on both devices, or the surest way to see two people
    // looking at the same week is to watch them disagree about its word list.
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.word.cmp(&b.word)));
    out.truncate(MAX_WORDS);
    out
}

/// The countable words of one line, lowercased and filtered.
fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        // The zero-width non-joiner is part of a Persian word (`می‌روم` is one
        // word), so it must not split one in two. Every other non-letter is a
        // boundary: punctuation, spaces, and the digits that would otherwise
        // turn a date into the week's most repeated word.
        if c.is_alphanumeric() || c == '\u{200c}' {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out.retain(|w| {
        // One letter says nothing about a theme, and a bare number says
        // nothing about anything.
        w.chars().count() >= 2
            && !w.chars().all(|c| c.is_numeric())
            && !STOPWORDS.contains(&w.as_str())
    });
    out
}

/// The local calendar day of a Unix timestamp, or `None` for one out of range.
fn day_of(at: i64) -> Option<chrono::NaiveDate> {
    chrono::DateTime::from_timestamp(at, 0).map(|u| u.with_timezone(&chrono::Local).date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    /// 2026-10-31 09:00:00 +0330 — a Saturday, the first day of an Iranian week.
    const SAT: i64 = 1_793_431_800;
    const DAY: i64 = 86_400;

    fn span() -> Range {
        Range {
            start: NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
            end: NaiveDate::from_ymd_opt(2026, 11, 6).unwrap(),
        }
    }

    fn decree(at: i64, text: &str) -> Decree {
        Decree {
            id: format!("d{at}{text}"),
            session_at: at,
            text: text.to_string(),
            written_at: at - 1500,
            deleted: false,
        }
    }

    fn inscription(at: i64, text: &str) -> Inscription {
        Inscription {
            id: format!("i{at}{text}"),
            session_at: at,
            text: text.to_string(),
            written_at: at,
            deleted: false,
        }
    }

    #[test]
    fn a_promise_answered_by_an_inscription_is_counted_as_kept() {
        let r = build(
            &[decree(SAT, "draft the report")],
            &[inscription(SAT, "the report is drafted")],
            &[SAT],
            span(),
        );
        assert_eq!(r.spoken, 1);
        assert_eq!(r.answered, 1);
        assert_eq!(r.unanswered(), 0);
        assert!(r.unfulfilled.is_empty(), "nothing to name");
    }

    #[test]
    fn a_promise_with_no_words_after_it_is_named_with_its_day() {
        let r = build(
            &[
                decree(SAT, "draft the contract"),
                decree(SAT + DAY, "review the contract"),
            ],
            &[inscription(SAT + DAY, "reviewed it")],
            &[SAT, SAT + DAY],
            span(),
        );
        assert_eq!(r.spoken, 2);
        assert_eq!(r.answered, 1);
        assert_eq!(r.unanswered(), 1);
        assert_eq!(r.unfulfilled.len(), 1);
        assert_eq!(r.unfulfilled[0].text, "draft the contract");
        assert_eq!(
            r.unfulfilled[0].session_at, SAT,
            "the row carries the key its inscription would have shared"
        );
    }

    #[test]
    fn unfulfilled_rows_are_newest_first_and_capped() {
        let decrees: Vec<Decree> = (0..8)
            .map(|i| decree(SAT + i * 600, &format!("promise {i}")))
            .collect();
        let r = build(&decrees, &[], &[], span());
        assert_eq!(r.unfulfilled_total, 8, "the count is the whole truth");
        assert_eq!(
            r.unfulfilled.len(),
            MAX_UNDELIVERED,
            "but only some are named"
        );
        assert_eq!(r.unfulfilled[0].text, "promise 7", "newest first");
    }

    #[test]
    fn words_written_without_a_promise_are_counted_separately() {
        let r = build(&[], &[inscription(SAT, "an unplanned win")], &[SAT], span());
        assert_eq!(r.spoken, 0);
        assert_eq!(r.written, 1);
        assert_eq!(r.unspoken(), 1, "working, but not on what was planned");
        assert_eq!(r.unanswered(), 0, "and not a promise that was broken");
    }

    #[test]
    fn only_the_span_is_read() {
        let r = build(
            &[
                decree(SAT - 7 * DAY, "last week's promise"),
                decree(SAT + 7 * DAY, "next week's promise"),
                decree(SAT, "this week's promise"),
            ],
            &[],
            &[SAT - 7 * DAY, SAT, SAT + 7 * DAY],
            span(),
        );
        assert_eq!(r.spoken, 1);
        assert_eq!(r.sessions, 1);
        assert_eq!(r.unfulfilled[0].text, "this week's promise");
    }

    #[test]
    fn a_withdrawn_promise_is_not_held_against_the_week() {
        let mut d = decree(SAT, "a plan I called off");
        d.deleted = true;
        let r = build(&[d], &[], &[SAT], span());
        assert_eq!(r.spoken, 0, "a withdrawal is not a failure to deliver");
        assert_eq!(r.unanswered(), 0);
    }

    #[test]
    fn a_word_repeated_across_decrees_becomes_a_theme() {
        let r = build(
            &[
                decree(SAT, "draft the report"),
                decree(SAT + 600, "send the report"),
                decree(SAT + 1200, "read a paper"),
            ],
            &[],
            &[],
            span(),
        );
        assert_eq!(r.recurring[0].word, "report");
        assert_eq!(r.recurring[0].count, 2);
        assert!(
            !r.recurring.iter().any(|w| w.word == "paper"),
            "one mention is a word used, not a theme"
        );
    }

    #[test]
    fn a_word_shouted_three_times_in_one_line_counts_once() {
        let r = build(
            &[
                decree(SAT, "report report report"),
                decree(SAT + 600, "report it"),
            ],
            &[],
            &[],
            span(),
        );
        assert_eq!(
            r.recurring[0].count, 2,
            "an emphatic line is one intention, not three"
        );
    }

    #[test]
    fn common_words_and_bare_numbers_are_not_themes() {
        let r = build(
            &[
                decree(SAT, "write the 2026 report for the team"),
                decree(SAT + 600, "write the 2027 report for the team"),
            ],
            &[],
            &[],
            span(),
        );
        let words: Vec<&str> = r.recurring.iter().map(|w| w.word.as_str()).collect();
        assert!(words.contains(&"report") && words.contains(&"team"));
        assert!(!words.contains(&"the"), "a stopword cannot be a theme");
        assert!(!words.contains(&"2026"), "a number is not a theme");
    }

    #[test]
    fn persian_words_survive_the_joiner() {
        let r = build(
            &[
                decree(SAT, "می‌نویسم گزارش را"),
                decree(SAT + 600, "می‌نویسم گزارش دیروز"),
            ],
            &[],
            &[],
            span(),
        );
        let words: Vec<&str> = r.recurring.iter().map(|w| w.word.as_str()).collect();
        assert!(
            words.contains(&"می‌نویسم"),
            "the zero-width joiner is part of the word, not a separator"
        );
        assert!(words.contains(&"گزارش"));
        assert!(!words.contains(&"را"), "Persian stopwords are removed too");
    }

    #[test]
    fn word_order_is_stable_across_equal_counts() {
        let decrees = [decree(SAT, "alpha beta"), decree(SAT + 600, "beta alpha")];
        // Built twice from opposite directions: the same week must read the
        // same way regardless of the order the rows arrived in.
        let a = build(&decrees, &[], &[], span());
        let mut flipped: Vec<Decree> = decrees.to_vec();
        flipped.reverse();
        let b = build(&flipped, &[], &[], span());
        assert_eq!(a.recurring, b.recurring);
        assert_eq!(a.recurring[0].word, "alpha", "ties break alphabetically");
    }

    #[test]
    fn an_empty_record_says_it_has_nothing_to_report() {
        let r = build(&[], &[], &[], span());
        assert!(r.is_empty());
        assert_eq!(r.unanswered(), 0, "no promises is not a failure");
        assert!(r.recurring.is_empty());
    }

    #[test]
    fn a_decree_written_long_after_its_block_still_pairs_by_session() {
        // `written_at` is later than `session_at` here on purpose: pairing must
        // follow the session, or a decree written the next morning would be
        // counted against the wrong block.
        let mut d = decree(SAT, "draft the report");
        d.written_at = SAT + 3 * DAY;
        let r = build(&[d], &[inscription(SAT, "drafted it")], &[SAT], span());
        assert_eq!(r.answered, 1);
    }
}
