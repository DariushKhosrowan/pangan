// ═══════════════════════════════════════════════════════════════════════════
// refresh.rs — update the "on this day" table from Wikipedia at runtime.
//
// The app ships with data/day_events.json compiled in, so it works offline.
// This module lets it pull a fresher copy without a rebuild: it asks the
// Wikipedia API for the current Jalali day's page (Iranian stream) and the
// current Gregorian day's page (world stream), merges those into the cached
// table, and writes the result to the user's config dir.
//
// Deliberate limits: it refreshes *today*, not all 366 days — that is what
// keeps it fast enough to run on demand. A full rebuild is the job of
// tools/fetch_events.py.
// ═══════════════════════════════════════════════════════════════════════════

use std::path::PathBuf;

/// Where the refreshed table is cached between runs.
pub fn cache_path() -> PathBuf {
    crate::state::store::Store::data_dir().join("day_events.json")
}

/// Read the cached table if a previous refresh wrote one.
pub fn load_cached() -> Option<String> {
    std::fs::read_to_string(cache_path()).ok()
}

fn get(url: &str) -> Option<String> {
    let resp = ureq::get(url)
        .header(
            "User-Agent",
            "Pangan/0.4 (https://github.com/DariushKhosrowan/pangan)",
        )
        .call()
        .ok()?;
    resp.into_body().read_to_string().ok()
}

/// Rendered HTML for a page, or None.
///
/// Parsing the *rendered* HTML rather than raw wikitext is what makes this
/// robust: nested links, templates and refs are already resolved by the time
/// we see the text, so there is no markup left to mis-strip.
fn page_html(lang: &str, page: &str) -> Option<String> {
    let encoded = urlencode(page);
    let url = format!(
        "https://{lang}.wikipedia.org/w/api.php?action=parse&format=json\
&prop=text&formatversion=2&redirects=1&page={encoded}"
    );
    let body = get(&url)?;
    let v: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(v.get("parse")?.get("text")?.as_str()?.to_string())
}

/// Percent-encode a page title for the query string.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn strip_between(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn html_sections(html: &str) -> Vec<(String, String)> {
    let mut marks: Vec<(usize, usize, String)> = Vec::new(); // (body_start, heading_start, title)
    let mut search = 0usize;
    while let Some(i) = html[search..].find("<h2") {
        let h_start = search + i;
        let Some(gt) = html[h_start..].find('>') else {
            break;
        };
        let tag_open_end = h_start + gt + 1;
        let Some(close) = html[tag_open_end..].find("</h2>") else {
            break;
        };
        let title_html = &html[tag_open_end..tag_open_end + close];
        let title = html_text(title_html);
        let body_start = tag_open_end + close + "</h2>".len(); // Skip the edit-section chrome that follows the heading.
        marks.push((body_start, h_start, title));
        search = body_start;
    }
    let mut out = Vec::new();
    for (idx, (body_start, _, title)) in marks.iter().enumerate() {
        let end = marks
            .get(idx + 1)
            .map(|(_, h_start, _)| *h_start)
            .unwrap_or(html.len());
        out.push((title.clone(), html[*body_start..end].to_string()));
    }
    out
}

/// The `<li>` items of the first `<ul>` in a section body.
fn html_list_items(body: &str) -> Vec<String> {
    let Some(ul) = body.find("<ul>") else {
        return vec![];
    };
    let Some(ul_end) = body[ul..].find("</ul>") else {
        return vec![];
    };
    let list = &body[ul + 4..ul + ul_end];
    let mut out = Vec::new();
    let mut rest = list;
    while let Some(i) = rest.find("<li>") {
        let after = &rest[i + 4..];
        let Some(j) = after.find("</li>") else { break };
        let item = html_text(&after[..j]);
        if item.chars().count() > 12 {
            out.push(item);
        }
        rest = &after[j + 5..];
    }
    out
}

/// Strip tags and decode entities from an HTML fragment.
fn html_text(frag: &str) -> String {
    // Drop superscripts (citation brackets) and edit-section links outright.
    let mut t = frag.to_string();
    t = strip_between(&t, "<sup", "</sup>");
    t = strip_between(&t, "<style", "</style>");
    t = strip_between(&t, "<script", "</script>"); // Tags -> nothing, but keep a space where a tag separated words.
    let mut out = String::with_capacity(t.len());
    let mut in_tag = false;
    for c in t.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let mut s = decode_entities(&out); // Collapse whitespace.
    let mut clean = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                clean.push(' ');
            }
            prev_space = true;
        } else {
            clean.push(c);
            prev_space = false;
        }
    }
    s = clean.trim().to_string();
    // Tags become spaces, which leaves "word ," before punctuation. Close the
    // gap so the text reads naturally in both scripts.
    for p in [",", ";", ":", ".", "!", "?", ")", "”", "’"] {
        s = s.replace(&format!(" {p}"), p);
    }
    for p in ["(", "“", "‘"] {
        s = s.replace(&format!("{p} "), p);
    }
    s.trim_matches(|c: char| c == ',' || c == ';' || c == ':' || c == '–' || c == '-')
        .trim()
        .to_string()
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let ent = &tail[1..semi];
        let rep = match ent {
            "amp" => Some("&"),
            "lt" => Some("<"),
            "gt" => Some(">"),
            "quot" => Some("\""),
            "apos" => Some("'"),
            "nbsp" => Some(" "),
            "ndash" => Some("–"),
            "mdash" => Some("—"),
            "lsquo" => Some("‘"),
            "rsquo" => Some("’"),
            "ldquo" => Some("“"),
            "rdquo" => Some("”"),
            _ => {
                // Numeric: &#91; / &#x5B;
                if let Some(num) = ent.strip_prefix('#') {
                    let code = if let Some(hex) = num.strip_prefix('x').or(num.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    code.and_then(char::from_u32).map(|c| {
                        // Leak-free: push directly below instead.
                        out.push(c);
                        ""
                    })
                } else {
                    None
                }
            }
        };
        if let Some(r) = rep {
            out.push_str(r);
        } else if !ent.starts_with('#') {
            // Unknown named entity: keep it verbatim.
            out.push_str(&tail[..=semi]);
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

/// Entries the owner of this calendar does not want shown, matched as a
/// substring of the cleaned item text.
///
/// The live refresh replaces a whole day with whatever Wikipedia currently
/// says, so an entry deleted from data/day_events.json by hand reappears the
/// next time the day is refreshed. This list is what makes a removal stick.
///
/// Keep it short and specific: a broad term here silently drops unrelated
/// history with no warning.
const EXCLUDED_ITEMS: [&str; 1] = [
    // 8 Aban: the joint statement naming the Ministry of Intelligence and the
    // IRGC Intelligence Organisation. Removed at the owner's request.
    "وزارت اطلاعات و سازمان اطلاعات سپاه",
];

fn is_excluded(text: &str) -> bool {
    EXCLUDED_ITEMS.iter().any(|term| text.contains(term))
}

/// The list items of the named sections, in document order.
fn section_items(html: &str, headings: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for (title, body) in html_sections(html) {
        if headings.contains(&title.as_str()) {
            out.extend(
                html_list_items(&body)
                    .into_iter()
                    .filter(|s| !is_excluded(s)),
            );
        }
    }
    out
}

const JALALI_MONTHS: [&str; 12] = [
    "فروردین",
    "اردیبهشت",
    "خرداد",
    "تیر",
    "مرداد",
    "شهریور",
    "مهر",
    "آبان",
    "آذر",
    "دی",
    "بهمن",
    "اسفند",
];
const GREG_MONTHS_FA: [&str; 12] = [
    "ژانویه",
    "فوریه",
    "مارس",
    "آوریل",
    "مه",
    "ژوئن",
    "ژوئیه",
    "اوت",
    "سپتامبر",
    "اکتبر",
    "نوامبر",
    "دسامبر",
];
const GREG_MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// What a refresh produced, for the status line.
#[derive(Debug)]
pub struct Refreshed {
    pub iran_events: usize,
    pub world_events: usize,
}

/// Refresh the current day's entries and merge them into `table`.
///
/// `table` is the parsed JSON object from data/day_events.json; it is mutated
/// in place so the caller can persist it.
pub fn refresh_today(
    table: &mut serde_json::Value,
    jm: u32,
    jd: u32,
    gm: u32,
    gd: u32,
) -> Result<Refreshed, String> {
    let mut iran_events = 0;
    let mut world_events = 0;

    // ── Iranian stream: the Jalali day page ──
    let page = format!("{} {}", jd, JALALI_MONTHS[(jm as usize).clamp(1, 12) - 1]);
    if let Some(html) = page_html("fa", &page) {
        let events = section_items(&html, &["رویدادها"]);
        let births = section_items(&html, &["زادروزها"]);
        let deaths = section_items(&html, &["درگذشت‌ها", "درگذشت‌ها و سالگردها"]);
        let occasions = section_items(&html, &["مناسبت‌ها", "تعطیلات و مناسبت‌ها"]);
        iran_events = events.len();
        if let Some(iran) = table.get_mut("iran").and_then(|v| v.as_object_mut()) {
            iran.insert(
                format!("{jm}-{jd}"),
                serde_json::json!({
                    "title": page,
                    "events": events,
                    "births": births,
                    "deaths": deaths,
                    "occasions": occasions,
                }),
            );
        }
    }

    // ── World stream: the Gregorian day page, both languages ──
    let fa_page = format!("{} {}", gd, GREG_MONTHS_FA[(gm as usize).clamp(1, 12) - 1]);
    if let Some(html) = page_html("fa", &fa_page) {
        let events = section_items(&html, &["رویدادها"]);
        world_events = events.len();
        if let Some(w) = table.get_mut("world_fa").and_then(|v| v.as_object_mut()) {
            w.insert(format!("{gm:02}-{gd:02}"), serde_json::json!(events));
        }
    }
    let en_page = format!("{} {}", GREG_MONTHS_EN[(gm as usize).clamp(1, 12) - 1], gd);
    if let Some(html) = page_html("en", &en_page) {
        // The English page nests Events under year-range h3 headings
        // (===Pre-1600===, ===1601–1900===, ...), so collect the whole
        // Events section and every list inside it rather than one <ul>.
        let events = events_section_all_lists(&html);
        if let Some(w) = table.get_mut("world_en").and_then(|v| v.as_object_mut()) {
            w.insert(format!("{gm:02}-{gd:02}"), serde_json::json!(events));
        }
    }

    if iran_events == 0 && world_events == 0 {
        return Err("no data returned from Wikipedia".into());
    }
    Ok(Refreshed {
        iran_events,
        world_events,
    })
}

/// Every list item in the "Events" section, including its h3 subsections.
fn events_section_all_lists(html: &str) -> Vec<String> {
    // Slice from the Events h2 to the next h2, then take all <ul>s inside.
    let Some(h2) = html.find("<h2") else {
        return vec![];
    };
    let mut start = None;
    let mut search = h2;
    while let Some(i) = html[search..].find("<h2") {
        let at = search + i;
        let Some(gt) = html[at..].find('>') else {
            break;
        };
        let open_end = at + gt + 1;
        let Some(close) = html[open_end..].find("</h2>") else {
            break;
        };
        let title = html_text(&html[open_end..open_end + close]);
        if title == "Events" {
            start = Some(open_end + close + 5);
            break;
        }
        search = open_end + close + 5;
    }
    let Some(from) = start else { return vec![] };
    // End at the next h2 after `from`.
    let end = html[from..]
        .find("<h2")
        .map(|i| from + i)
        .unwrap_or(html.len());
    let body = &html[from..end];

    let mut out = Vec::new();
    let mut rest = body;
    while let Some(ul) = rest.find("<ul>") {
        let after = &rest[ul + 4..];
        let Some(ul_end) = after.find("</ul>") else {
            break;
        };
        let list = &after[..ul_end];
        let mut items = list;
        while let Some(li) = items.find("<li>") {
            let a = &items[li + 4..];
            let Some(li_end) = a.find("</li>") else { break };
            let item = html_text(&a[..li_end]);
            if item.chars().count() > 12 {
                out.push(item);
            }
            items = &a[li_end + 5..];
        }
        rest = &after[ul_end + 5..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_items_from_rendered_html() {
        let html = r#"<div class="mw-heading mw-heading2"><h2 id="x">رویدادها</h2></div>
<ul><li><a href="/wiki/a">۵۳۹ (پیش از میلاد)</a> - <a href="/wiki/b">کوروش بزرگ</a> بابل را فتح کرد<sup class="noprint">&#91;۱&#93;</sup></li>
<li>کوتاه</li></ul>
<div class="mw-heading mw-heading2"><h2 id="y">زادروزها</h2></div>
<ul><li>یک زادروز بسیار مهم و طولانی</li></ul>"#;
        let evs = section_items(html, &["رویدادها"]);
        assert_eq!(evs.len(), 1, "short items dropped: {evs:?}");
        assert!(evs[0].starts_with("۵۳۹"), "got {:?}", evs[0]);
        assert!(evs[0].contains("کوروش بزرگ"));
        // The citation bracket must be gone.
        assert!(!evs[0].contains('['), "citation leaked: {:?}", evs[0]);
        assert!(!evs[0].contains('۱'), "citation digit leaked: {:?}", evs[0]);

        let births = section_items(html, &["زادروزها"]);
        assert_eq!(births.len(), 1);
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("a &amp; b &#91;x&#93;"), "a & b [x]");
        assert_eq!(decode_entities("&ndash;"), "–");
    }

    #[test]
    fn collects_all_lists_under_events_with_subsections() {
        let html = r#"<h2 id="e">Events</h2>
<h3 id="p">Pre-1600</h3>
<ul><li>461 – Roman Emperor Majorian is beheaded near the river</li></ul>
<h3 id="q">1601–1900</h3>
<ul><li>1782 – George Washington orders the creation of a badge</li></ul>
<h2 id="b">Births</h2>
<ul><li>1876 – Mata Hari, Dutch dancer and spy</li></ul>"#;
        let evs = events_section_all_lists(html);
        assert_eq!(evs.len(), 2, "got {evs:?}");
        assert!(evs[0].contains("Majorian"));
        assert!(evs[1].contains("Washington"));
        // Births must not leak in.
        assert!(!evs.iter().any(|e| e.contains("Mata Hari")));
    }

    #[test]
    fn url_encoding_handles_persian() {
        let e = urlencode("۸ آبان");
        assert!(e.starts_with('%'), "got {e}");
        assert!(e.contains('+'));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    /// Hits the real Wikipedia API. Ignored by default so the normal test run
    /// stays offline and deterministic; run with:
    ///   cargo test -- --ignored live_refresh
    #[test]
    #[ignore]
    fn live_refresh_returns_data() {
        let mut table: serde_json::Value =
            serde_json::from_str(include_str!("../../data/day_events.json")).unwrap(); // 7 Aban 1405 == 29 October 2026
        let r = refresh_today(&mut table, 8, 7, 10, 29).expect("refresh should succeed");
        assert!(r.iran_events > 0, "expected Iranian events, got {r:?}");
        assert!(r.world_events > 0, "expected world events, got {r:?}");
        let iran = table["iran"]["8-7"]["events"].as_array().unwrap();
        assert!(!iran.is_empty());
        for e in iran {
            let s = e.as_str().unwrap();
            assert!(!s.contains("[["), "wiki link markup leaked: {s}");
            assert!(!s.contains("{{"), "template markup leaked: {s}");
            assert!(!s.contains("<ref"), "ref markup leaked: {s}");
        }
        println!("iran={} world={}", r.iran_events, r.world_events);
        for e in table["iran"]["8-7"]["events"].as_array().unwrap() {
            println!("IRAN: {}", e.as_str().unwrap());
        }
        for e in table["world_fa"]["10-29"].as_array().unwrap() {
            println!("WFA: {}", e.as_str().unwrap());
        }
        for e in table["world_en"]["10-29"]
            .as_array()
            .unwrap()
            .iter()
            .take(3)
        {
            println!("WEN: {}", e.as_str().unwrap());
        }
    }
}
