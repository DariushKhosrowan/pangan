// ═══════════════════════════════════════════════════════════════════════════
// events.rs — "on this day" events, embedded from data/day_events.json.
//
// The file is compiled into the binary, so the feature works offline. Its
// contents are extracted from the Persian Wikipedia pages for each Jalali month
// day (see data/CREDITS.md); nothing here is hand-written.
// ═══════════════════════════════════════════════════════════════════════════

use serde::Deserialize;

/// One event on a given month day.
#[derive(Deserialize, Clone, Debug)]
pub struct Event {
    /// Section it appeared under on the source page: سیاسی، هنری، ورزشی، ...
    #[serde(default)]
    pub s: String,
    /// The event text, as written in the source.
    pub t: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DayEntry {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// The whole table, keyed by "<month>-<day>".
#[derive(Deserialize)]
struct Raw {
    #[serde(flatten)]
    days: std::collections::HashMap<String, DayEntry>,
}

/// Parsed event table.
pub struct Events {
    days: std::collections::HashMap<String, DayEntry>,
}

impl Events {
    /// Load the embedded table. A malformed table degrades to empty rather than
    /// panicking, so a bad data file can never take the timer down.
    pub fn load() -> Self {
        let raw = include_str!("../data/day_events.json");
        match serde_json::from_str::<Raw>(raw) {
            Ok(r) => Events { days: r.days },
            Err(e) => {
                eprintln!("events: could not parse day_events.json: {e}");
                Events {
                    days: Default::default(),
                }
            }
        }
    }

    /// Events for a Jalali month/day.
    pub fn for_day(&self, month: u32, day: u32) -> &[Event] {
        self.days
            .get(&format!("{month}-{day}"))
            .map(|d| d.events.as_slice())
            .unwrap_or(&[])
    }
}

/// Turn a raw source line into (year, text) when it starts with a year.
///
/// Source lines look like "۱۱۹۲ - انعقاد عهدنامه گلستان، ..." or
/// "۵۳۹ (پیش از میلاد) - ورود کوروش بزرگ ...". Returns None when no leading
/// year is present, so the caller can show the line unchanged.
pub fn split_year(line: &str) -> Option<(String, String)> {
    let (head, rest) = line.split_once(" - ")?;
    let head = head.trim();
    let rest = rest.trim();
    if head.is_empty() || rest.is_empty() {
        return None;
    }
    // The head must start with Persian digits; after them only an era marker
    // is allowed, in the several forms the source pages use.
    let digits: String = head.chars().take_while(|c| c.is_numeric()).collect();
    if digits.is_empty() {
        return None;
    }
    let tail = head[digits.len()..].trim();
    let era_ok = tail.is_empty()
        || matches!(
            tail,
            "قبل از هجرت" | "قبل از میلاد" | "پیش از میلاد" | "پ.م." | "پ. م." | "ق.م."
        )
        || {
            // Tolerate the same markers wrapped in parentheses.
            let inner = tail.trim_start_matches(['(', '[']).trim_end_matches([')', ']']).trim();
            matches!(
                inner,
                "قبل از هجرت" | "قبل از میلاد" | "پیش از میلاد" | "پ.م." | "پ. م." | "ق.م."
            )
        };
    if !era_ok {
        return None;
    }
    Some((head.to_string(), rest.to_string()))
}

/// Persian digits for a number.
fn fa(n: i64) -> String {
    crate::calendar::fa_num(n)
}

/// A short, human label for a source section.
pub fn section_label(s: &str) -> &str {
    match s {
        "سیاسی" => "سیاسی",
        "هنری" => "هنری",
        "ورزشی" => "ورزشی",
        "علمی" => "علمی",
        "فرهنگی" => "فرهنگی",
        "جنگ" => "جنگ",
        "اقتصادی" => "اقتصادی",
        other => other,
    }
}

/// Build the display rows for a day: the year (if any) and the text.
pub fn rows(events: &[Event]) -> Vec<(String, String, String)> {
    events
        .iter()
        .map(|e| {
            let (year, text) = match split_year(&e.t) {
                Some((y, t)) => (y, t),
                None => (String::new(), e.t.clone()),
            };
            (section_label(&e.s).to_string(), year, text)
        })
        .collect()
}

/// Count of days that actually have at least one event.
pub fn coverage(events: &Events) -> (usize, usize) {
    let with = events.days.values().filter(|d| !d.events.is_empty()).count();
    (with, events.days.len())
}

/// Unused helper kept for symmetry with calendar::fa_num.
#[allow(dead_code)]
fn _fa(n: i64) -> String {
    fa(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_loads_and_has_events() {
        let e = Events::load();
        let (with, total) = coverage(&e);
        assert!(total > 300, "expected most days present, got {total}");
        assert!(with > 300, "expected most days to have events, got {with}");
    }

    #[test]
    fn known_day_has_events() {
        let e = Events::load();
        // 2 Aban has a well-populated page (Treaty of Gulistan among others).
        let evs = e.for_day(8, 2);
        assert!(!evs.is_empty(), "2 Aban should have events");
    }

    #[test]
    fn year_splitting() {
        assert_eq!(
            split_year("۱۱۹۲ - انعقاد عهدنامه گلستان"),
            Some(("۱۱۹۲".into(), "انعقاد عهدنامه گلستان".into()))
        );
        assert_eq!(
            split_year("۱۰۷۰ قبل از هجرت - به رسمیت شناختن حاکمیت"),
            Some((
                "۱۰۷۰ قبل از هجرت".into(),
                "به رسمیت شناختن حاکمیت".into()
            ))
        );
        // BCE years, with and without parentheses.
        assert_eq!(
            split_year("۵۳۹ (پیش از میلاد) - ورود کوروش بزرگ به بابل"),
            Some((
                "۵۳۹ (پیش از میلاد)".into(),
                "ورود کوروش بزرگ به بابل".into()
            ))
        );
        assert_eq!(
            split_year("۳۳۰ پیش از میلاد - سقوط هخامنشیان"),
            Some(("۳۳۰ پیش از میلاد".into(), "سقوط هخامنشیان".into()))
        );
        // No leading year -> left alone.
        assert_eq!(split_year("بدون سال - متن"), None);
        // A year followed by prose is not a year head.
        assert_eq!(split_year("۱۳۰۰ مردم تهران - متن"), None);
    }
}
