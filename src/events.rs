// ═══════════════════════════════════════════════════════════════════════════
// events.rs — "on this day" data, embedded from data/day_events.json.
//
// Two independent streams, deliberately kept apart:
//
//   iran   Persian heritage, keyed by Jalali month-day ("8-7"). Read from the
//          fa.wikipedia page for that Jalali day. This leads the screen.
//   world  General world history, keyed by *Gregorian* month-day ("08-07").
//          The caller converts the Jalali date to Gregorian before looking up.
//
// The file is compiled into the binary, so the feature works offline; a
// background refresh can replace it from the network (see tools/fetch_events.py
// and the "refresh" path in main.rs).
// ═══════════════════════════════════════════════════════════════════════════

use serde::Deserialize;

/// The Iranian-heritage entry for one Jalali day.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct IranDay {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub births: Vec<String>,
    #[serde(default)]
    pub deaths: Vec<String>,
    #[serde(default)]
    pub occasions: Vec<String>,
}

/// The whole table.
#[derive(Deserialize, Default)]
pub struct Table {
    #[serde(default)]
    pub iran: std::collections::HashMap<String, IranDay>,
    /// Persian-language world events, keyed by Gregorian month-day.
    #[serde(default)]
    pub world_fa: std::collections::HashMap<String, Vec<String>>,
    /// English-language world events, keyed by Gregorian month-day.
    #[serde(default)]
    pub world_en: std::collections::HashMap<String, Vec<String>>,
}

/// Parsed event table, plus where it came from.
pub struct Events {
    table: Table,
}

/// Which stream a row belongs to, so the UI can group and style it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Iran,
    World,
    Birth,
    Death,
    Occasion,
}

/// One display row.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub year: String,
    pub text: String,
}

impl Events {
    /// Load the embedded table. A malformed table degrades to empty rather than
    /// panicking, so a bad data file can never take the timer down.
    pub fn load() -> Self {
        Self::from_str(include_str!("../data/day_events.json"))
    }

    /// Load from an explicit JSON string (used by the refresh path, so a
    /// downloaded file is validated before it replaces anything).
    pub fn from_str(raw: &str) -> Self {
        match serde_json::from_str::<Table>(raw) {
            Ok(table) => Events { table },
            Err(e) => {
                eprintln!("events: could not parse day_events.json: {e}");
                Events {
                    table: Table::default(),
                }
            }
        }
    }

    /// Iranian-heritage rows for a Jalali month-day.
    pub fn iran(&self, jm: u32, jd: u32) -> (Vec<Row>, Vec<Row>, Vec<Row>, Vec<Row>) {
        let Some(d) = self.table.iran.get(&format!("{jm}-{jd}")) else {
            return (vec![], vec![], vec![], vec![]);
        };
        let pack = |v: &[String], kind: Kind| -> Vec<Row> {
            v.iter()
                .map(|line| {
                    let (year, text) = split_year(line).unwrap_or((String::new(), line.clone()));
                    Row { kind, year, text }
                })
                .collect()
        };
        (
            pack(&d.events, Kind::Iran),
            pack(&d.births, Kind::Birth),
            pack(&d.deaths, Kind::Death),
            pack(&d.occasions, Kind::Occasion),
        )
    }

    /// World-history rows for a Gregorian month-day, in the chosen language.
    /// Falls back to the other language when one has no data for the day.
    pub fn world(&self, lang: &str, gm: u32, gd: u32) -> Vec<Row> {
        let key = format!("{gm:02}-{gd:02}");
        let (primary, secondary) = if lang == "en" {
            (&self.table.world_en, &self.table.world_fa)
        } else {
            (&self.table.world_fa, &self.table.world_en)
        };
        let list = primary.get(&key).or_else(|| secondary.get(&key));
        let Some(list) = list else { return vec![] };
        list.iter()
            .map(|line| {
                let (year, text) = split_year(line).unwrap_or((String::new(), line.clone()));
                Row {
                    kind: Kind::World,
                    year,
                    text,
                }
            })
            .collect()
    }

    /// True when the embedded table has any Iranian data at all.
    pub fn has_iran(&self) -> bool {
        !self.table.iran.is_empty()
    }

    /// (days with Iranian data, days with world data)
    pub fn coverage(&self) -> (usize, usize) {
        let iran = self.table.iran.values().filter(|d| !d.events.is_empty()).count();
        let world = self.table.world_fa.len().max(self.table.world_en.len());
        (iran, world)
    }
}

/// Turn a raw source line into (year, text) when it starts with a year.
///
/// Source lines look like "۱۱۹۲ - انعقاد عهدنامه گلستان، ..." or
/// "۵۳۹ (پیش از میلاد) - ورود کوروش بزرگ ...". Returns None when no leading
/// year is present, so the caller can show the line unchanged.
pub fn split_year(line: &str) -> Option<(String, String)> {
    // Both the Persian dash and the ASCII one appear in the sources.
    let (head, rest) = line
        .split_once(" - ")
        .or_else(|| line.split_once('–'))
        .or_else(|| line.split_once(" ‐ "))?;
    let head = head.trim();
    let rest = rest.trim();
    if head.is_empty() || rest.is_empty() {
        return None;
    }
    // The head must start with digits (Persian or ASCII); after them only an
    // era marker is allowed, in the several forms the source pages use.
    let digits: String = head.chars().take_while(|c| c.is_numeric()).collect();
    if digits.is_empty() {
        return None;
    }
    let tail = head[digits.len()..].trim();
    let era_ok = tail.is_empty()
        || matches!(
            tail,
            "قبل از هجرت"
                | "قبل از میلاد"
                | "پیش از میلاد"
                | "پ.م."
                | "پ. م."
                | "ق.م."
                | "BC"
                | "AD"
        )
        || {
            let inner = tail
                .trim_start_matches(['(', '['])
                .trim_end_matches([')', ']'])
                .trim();
            matches!(
                inner,
                "قبل از هجرت"
                    | "قبل از میلاد"
                    | "پیش از میلاد"
                    | "پ.م."
                    | "پ. م."
                    | "ق.م."
                    | "BC"
                    | "AD"
            )
        };
    if !era_ok {
        return None;
    }
    Some((head.to_string(), rest.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_loads_both_streams() {
        let e = Events::load();
        let (iran, world) = e.coverage();
        assert!(iran > 300, "expected most Jalali days, got {iran}");
        assert!(world > 300, "expected most Gregorian days, got {world}");
    }

    #[test]
    fn iranian_day_has_events() {
        let e = Events::load();
        // 7 Aban carries the Cyrus-in-Babylon entry among others.
        let (evs, _, _, _) = e.iran(8, 7);
        assert!(!evs.is_empty(), "7 Aban should have Iranian events");
        // And the stream as a whole is populated on most days.
        let with_events = (1..=12)
            .flat_map(|m| (1..=30).map(move |d| (m, d)))
            .filter(|&(m, d)| !e.iran(m, d).0.is_empty())
            .count();
        assert!(with_events > 250, "expected most Jalali days, got {with_events}");
    }

    #[test]
    fn world_day_has_events_in_both_languages() {
        let e = Events::load();
        assert!(!e.world("fa", 8, 7).is_empty(), "7 August should have fa events");
        assert!(!e.world("en", 8, 7).is_empty(), "7 August should have en events");
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
        // The en-dash form used by the English pages.
        assert_eq!(
            split_year("153 BC – Roman consuls begin their year"),
            Some(("153 BC".into(), "Roman consuls begin their year".into()))
        );
        // No leading year -> left alone.
        assert_eq!(split_year("بدون سال - متن"), None);
        // A year followed by prose is not a year head.
        assert_eq!(split_year("۱۳۰۰ مردم تهران - متن"), None);
    }
}
