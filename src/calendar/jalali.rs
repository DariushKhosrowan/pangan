// ═══════════════════════════════════════════════════════════════════════════
// calendar.rs — Jalali (Solar Hijri) calendar arithmetic.
//
// No external crate: the Jalali year length follows the 33-year arithmetic
// cycle, which stays accurate for the years this app will ever display.
// ═══════════════════════════════════════════════════════════════════════════

/// Persian month names, in order.
///
/// These are the Middle Persian (Pahlavi) forms, not the modern New Persian
/// ones. The app is a heritage calendar, so the month is called امرداد rather
/// than مرداد, and سپندارمذ rather than اسفند — the same names the Zoroastrian
/// and Sasanian calendars used. The New Persian spellings are kept in
/// MONTHS_NEW_PERSIAN for anything that needs to look a page up by the modern
/// name (the Wikipedia fetch does: those pages are titled مرداد, not امرداد).
pub const MONTHS: [&str; 12] = [
    "فروردین",
    "اردیبهشت",
    "خرداد",
    "تیر",
    "امرداد",
    "شهریور",
    "مهر",
    "آبان",
    "آذر",
    "دی",
    "بهمن",
    "سپندارمذ",
];

/// The modern New Persian spellings, used only to address Wikipedia pages,
/// which are titled with the modern forms. Never shown to the user.
pub const MONTHS_NEW_PERSIAN: [&str; 12] = [
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

/// English transliterations of the month names, same order.
pub const MONTHS_EN: [&str; 12] = [
    "Farvardin",
    "Ordibehesht",
    "Khordad",
    "Tir",
    "Mordad",
    "Shahrivar",
    "Mehr",
    "Aban",
    "Azar",
    "Dey",
    "Bahman",
    "Esfand",
];

/// Persian weekday names, indexed by `chrono::Weekday::num_days_from_saturday`.
pub const WEEKDAYS: [&str; 7] = [
    "شنبه",
    "یک‌شنبه",
    "دوشنبه",
    "سه‌شنبه",
    "چهارشنبه",
    "پنج‌شنبه",
    "جمعه",
];

/// English weekday names, same order (Saturday first).
pub const WEEKDAYS_EN: [&str; 7] = [
    "Saturday",
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
];

/// The thirty day-names of the Zoroastrian month, in the order used by the
/// Sasanian calendar. Each month day 1..30 carries one of these names, and when
/// the day-name matches the month-name the day is a feast (e.g. Mehr-ruz in
/// Mehr-mah is Mehregan).
///
/// Source: fa.wikipedia "فهرست نام روزهای ماه" (see data/CREDITS.md).
pub const DAY_NAMES: [&str; 30] = [
    "اورمزد",    // 1
    "بهمن",      // 2  (وهومنه)
    "اردیبهشت",  // 3  (اشه وهیشته)
    "شهریور",    // 4
    "سپندارمذ",  // 5
    "خرداد",     // 6  (هیوروتات)
    "امرداد",    // 7  (امرتات)
    "دی به آذر", // 8
    "آذر",       // 9
    "آبان",      // 10 (آناهیتا)
    "خور",       // 11 (هور خشه‌ئته)
    "ماه",       // 12
    "تیر",       // 13 (تیشتر)
    "گوش",       // 14
    "دی به مهر", // 15
    "مهر",       // 16
    "سروش",      // 17
    "رشن",       // 18
    "فروردین",   // 19
    "بهرام",     // 20 (ورهرام)
    "رام",       // 21
    "باد",       // 22
    "دی به دین", // 23
    "دین",       // 24 (دئنا)
    "ارد",       // 25
    "اشتاد",     // 26
    "آسمان",     // 27
    "زامیاد",    // 28
    "مهراسپند",  // 29 (مانتره سپند)
    "انارام",    // 30
];

/// The five Gatha days (پنجه) that close the year, plus the leap-day name.
pub const GATHA_DAYS: [&str; 5] = ["اهنود", "اشتود", "سپنتمد", "وهوخشتر", "وهشتواش"];
pub const LEAP_DAY_NAME: &str = "اورداد";

/// English transliterations of the thirty day-names, same order.
pub const DAY_NAMES_EN: [&str; 30] = [
    "Ohrmazd",
    "Vohuman",
    "Ardibehesht",
    "Shahrivar",
    "Spandarmad",
    "Khordad",
    "Amordad",
    "Dey-be-Azar",
    "Azar",
    "Aban",
    "Khor",
    "Mah",
    "Tir",
    "Gosh",
    "Dey-be-Mehr",
    "Mehr",
    "Soroush",
    "Rashn",
    "Farvardin",
    "Bahram",
    "Ram",
    "Bad",
    "Dey-be-Din",
    "Din",
    "Ard",
    "Ashtad",
    "Aseman",
    "Zamyad",
    "Mehraspand",
    "Anaram",
];

/// What the day is called in the old calendar.
pub struct DayName {
    /// The day-name itself, e.g. "مهر".
    pub name: &'static str,
    /// Latin transliteration of the day-name, e.g. "Mehr".
    pub name_en: &'static str,
    /// The month-name, for the feast test.
    pub month: &'static str,
    /// True when day-name and month-name coincide — a feast day.
    pub is_feast: bool,
    /// The feast's name, when `is_feast`.
    pub feast: Option<&'static str>,
}

/// Feasts that fall on the day whose name matches the month's name.
/// Indexed by month number (1..12).
const MONTH_FEASTS: [&str; 12] = [
    "فروردینگان",
    "اردیبهشتگان",
    "خردادگان",
    "تیرگان",
    "امردادگان",
    "شهریورگان",
    "مهرگان",
    "آبانگان",
    "آذرگان",
    "خرم روز",
    "بهمنگان",
    "سپندارمذگان",
];

/// Festivals that do not fall on a namesake day.
///
/// The twelve name-day feasts (Tirgan, Mehregan, and the rest) are derived
/// from MONTH_NAME_DAY and need no entry here. What is listed here is the
/// handful of festivals that are fixed to a month-day for other reasons.
///
/// The six Gahambars are deliberately *not* here. They are five-day windows
/// rather than single days, and the Shahenshahi and Kadmi variants of the
/// Zoroastrian calendar shift them by months, so pinning them to one Jalali
/// month-day would be wrong more often than right.
///
/// `(month, day, name, note)`. The note is a one-line gloss, shown under the
/// name; keep it short — it is a caption, not an article.
const FIXED_FEASTS: [(u32, u32, &str, &str); 7] = [
    // ── Nowruz and the days that follow it ──
    (1, 1, "نوروز", "سرآغاز سال نو و جشن بهار"),
    (1, 2, "نوروز", "دومین روز نوروز"),
    (1, 3, "نوروز", "سومین روز نوروز"),
    (1, 4, "نوروز", "چهارمین روز نوروز"),
    (1, 6, "زایش زرتشت", "به روایتی زادروز اشو زرتشت"),
    (1, 13, "سیزده‌به‌در", "روز طبیعت و پایان جشن‌های نوروز"),
    // ── Sadeh: the mid-winter fire festival, fifty days before Nowruz ──
    (11, 10, "سده", "جشن آتش در میانهٔ زمستان"),
];

/// The era year: the count of years since the founding of the Iranian
/// kingdom, which is the reckoning this calendar uses instead of the Hijri
/// Solar year.
///
/// The epoch is 559 BC, the accession of Cyrus the Great — the point at which
/// Iran becomes a kingdom in the record. So 1405 SH (the Jalali year this app
/// computes) is 2585 in the era count: 1405 + 1180 = 2585.
///
/// This is a display conversion only. All the arithmetic stays in Jalali,
/// because that is the calendar the day boundaries and leap years follow; the
/// era year is the label put on the result.
pub const ERA_EPOCH_OFFSET: i32 = 1180;

/// The era year for a Jalali year.
pub fn era_year(jy: i32) -> i32 {
    jy + ERA_EPOCH_OFFSET
}

/// A festival falling on a given month-day, if any.
///
/// Name-day feasts are derived from the day-name table; the fixed festivals
/// are looked up in FIXED_FEASTS. A day can be both, in which case the
/// name-day feast is the more specific answer and wins.
pub fn feast_on(month: u32, day: u32) -> Option<(&'static str, &'static str)> {
    let m = (month as usize).clamp(1, 12) - 1;

    // A fixed festival first: these carry a note, which the name-day feasts
    // do not have.
    if let Some(&(_, _, name, note)) = FIXED_FEASTS
        .iter()
        .find(|&&(fm, fd, _, _)| fm == month && fd == day)
    {
        return Some((name, note));
    }

    // Otherwise the name-day feast, if this is the namesake day.
    if MONTH_NAME_DAY[m] == day {
        return Some((MONTH_FEASTS[m], ""));
    }

    None
}

/// The festival's name in Latin script, for the English UI.
///
/// `feast_on` returns the Persian name because that is what the Persian UI
/// shows. The English UI was still printing the Persian string, which is why
/// the timer screen's day subline stayed Persian in English mode. This maps
/// the same feast to its transliteration; an unknown name falls back to the
/// Persian so nothing is ever blank.
pub fn feast_name_en(month: u32, day: u32) -> Option<&'static str> {
    let m = (month as usize).clamp(1, 12) - 1;

    if let Some(&(_, _, name, _)) = FIXED_FEASTS
        .iter()
        .find(|&&(fm, fd, _, _)| fm == month && fd == day)
    {
        return Some(match name {
            "نوروز" => "Nowruz",
            "زایش زرتشت" => "Birth of Zoroaster",
            "سیزده‌به‌در" => "Sizdah Bedar",
            "سده" => "Sadeh",
            other => other,
        });
    }

    if MONTH_NAME_DAY[m] == day {
        return Some(MONTH_FEASTS_EN[m]);
    }

    None
}

/// Latin transliterations of the name-day feasts, in month order.
const MONTH_FEASTS_EN: [&str; 12] = [
    "Farvardinegan",
    "Ordibeheshtegan",
    "Khordadegan",
    "Tirgan",
    "Amordadegan",
    "Shahrivaregan",
    "Mehregan",
    "Abangan",
    "Azaregan",
    "Khorram Rouz",
    "Bahmanegan",
    "Sepandarmazegan",
];

/// The day of the month on which each month's namesake day falls.
/// (The day-name index that equals the month's own name.)
const MONTH_NAME_DAY: [u32; 12] = [
    19, // فروردین روز
    3,  // اردیبهشت روز
    6,  // خرداد روز
    13, // تیر روز
    7,  // امرداد روز
    4,  // شهریور روز
    16, // مهر روز
    10, // آبان روز
    9,  // آذر روز
    1,  // دی روز (خرم روز)
    2,  // بهمن روز
    5,  // سپندارمذ روز
];

/// Is `year` a leap year in the Jalali calendar?
pub fn is_leap(year: i32) -> bool {
    // 33-year cycle: years where (year % 33) is one of these have 366 days.
    let r = year.rem_euclid(33);
    matches!(r, 1 | 5 | 9 | 13 | 17 | 22 | 26 | 30)
}

/// Number of days in a Jalali month.
pub fn month_len(year: i32, month: u32) -> u32 {
    match month {
        1..=6 => 31,
        7..=11 => 30,
        12 => {
            if is_leap(year) {
                30
            } else {
                29
            }
        }
        _ => 0,
    }
}

/// Days elapsed since 1 Farvardin of year 1, for a given Jalali date.
fn day_number(year: i32, month: u32, day: u32) -> i64 {
    let mut n: i64 = 0;
    // Whole years before this one.
    for y in 1..year {
        n += if is_leap(y) { 366 } else { 365 };
    }
    // Whole months before this one, in this year.
    for m in 1..month {
        n += month_len(year, m) as i64;
    }
    n + (day as i64 - 1)
}

/// Convert a Jalali date to a Gregorian one.
pub fn to_gregorian(jy: i32, jm: u32, jd: u32) -> (i32, u32, u32) {
    // 1 Farvardin 1 AP == 19 March 622 CE (proleptic Gregorian).
    // Work in days-since-epoch to keep the arithmetic exact.
    let jdn = jalali_to_jdn(jy, jm, jd);
    jdn_to_gregorian(jdn)
}

/// Julian Day Number for a Jalali date.
pub fn jalali_to_jdn(jy: i32, jm: u32, jd: u32) -> i64 {
    // Epoch: 1 Farvardin 1 AP = JDN 1948320 (19 March 622 CE).
    const EPOCH: i64 = 1948320;
    EPOCH + day_number(jy, jm, jd)
}

/// Convert a Julian Day Number to a Gregorian date.
pub fn jdn_to_gregorian(jdn: i64) -> (i32, u32, u32) {
    let a = jdn + 32044;
    let b = (4 * a + 3) / 146097;
    let c = a - (146097 * b) / 4;
    let d = (4 * c + 3) / 1461;
    let e = c - (1461 * d) / 4;
    let m = (5 * e + 2) / 153;
    let day = (e - (153 * m + 2) / 5 + 1) as u32;
    let month = (m + 3 - 12 * (m / 10)) as u32;
    let year = (100 * b + d - 4800 + m / 10) as i32;
    (year, month, day)
}

/// Julian Day Number for a Gregorian date.
pub fn gregorian_to_jdn(y: i32, m: u32, d: u32) -> i64 {
    let a = (14 - m as i32) / 12;
    let y2 = y + 4800 - a;
    let m2 = m as i32 + 12 * a - 3;
    d as i64 + (153 * m2 as i64 + 2) / 5 + 365 * y2 as i64 + y2 as i64 / 4 - y2 as i64 / 100
        + y2 as i64 / 400
        - 32045
}

/// Convert a Gregorian date to Jalali.
pub fn from_gregorian(gy: i32, gm: u32, gd: u32) -> (i32, u32, u32) {
    let jdn = gregorian_to_jdn(gy, gm, gd);
    jdn_to_jalali(jdn)
}

/// Convert a Julian Day Number to a Jalali date.
pub fn jdn_to_jalali(jdn: i64) -> (i32, u32, u32) {
    const EPOCH: i64 = 1948320;
    let mut days = jdn - EPOCH;

    // Find the year by walking the 33-year cycle.
    let mut year = 1 + ((days as f64 / 365.2424).floor() as i32 - 1).max(0);
    while day_number(year + 1, 1, 1) <= days {
        year += 1;
    }
    while day_number(year, 1, 1) > days {
        year -= 1;
    }
    days -= day_number(year, 1, 1);

    let mut month = 1u32;
    while month < 12 && days >= month_len(year, month) as i64 {
        days -= month_len(year, month) as i64;
        month += 1;
    }
    (year, month, days as u32 + 1)
}

/// Today's Jalali date, from the system clock.
pub fn today() -> (i32, u32, u32) {
    let now = chrono::Local::now().date_naive();
    use chrono::Datelike;
    from_gregorian(now.year(), now.month(), now.day())
}

/// The name of a month day, plus the feast flag.
pub fn day_name(month: u32, day: u32) -> DayName {
    let month_name = MONTHS[(month as usize).clamp(1, 12) - 1];

    // Days 1..30 carry the thirty names; 31 is the extra day of the first six
    // months, which has no name of its own.
    if (1..=30).contains(&day) {
        let name = DAY_NAMES[(day - 1) as usize];
        let name_en = DAY_NAMES_EN[(day - 1) as usize];
        let is_feast = MONTH_NAME_DAY
            .get((month as usize).clamp(1, 12) - 1)
            .map(|&d| d == day)
            .unwrap_or(false);
        DayName {
            name,
            name_en,
            month: month_name,
            is_feast,
            feast: if is_feast {
                Some(MONTH_FEASTS[(month as usize).clamp(1, 12) - 1])
            } else {
                None
            },
        }
    } else {
        // Day 31 of a 31-day month. The Zoroastrian month has thirty named
        // days, so the extra day carries no name of its own.
        DayName {
            name: "روز افزوده",
            name_en: "Extra day",
            month: month_name,
            is_feast: false,
            feast: None,
        }
    }
}

/// Persian ordinal words for the days of a month, 1..=31.
const ORDINALS: [&str; 31] = [
    "یکم",
    "دوم",
    "سوم",
    "چهارم",
    "پنجم",
    "ششم",
    "هفتم",
    "هشتم",
    "نهم",
    "دهم",
    "یازدهم",
    "دوازدهم",
    "سیزدهم",
    "چهاردهم",
    "پانزدهم",
    "شانزدهم",
    "هفدهم",
    "هجدهم",
    "نوزدهم",
    "بیستم",
    "بیست‌ویکم",
    "بیست‌ودوم",
    "بیست‌وسوم",
    "بیست‌وچهارم",
    "بیست‌وپنجم",
    "بیست‌وششم",
    "بیست‌وهفتم",
    "بیست‌وهشتم",
    "بیست‌ونهم",
    "سی‌ام",
    "سی‌ویکم",
];

/// The Persian ordinal for a month day, e.g. 7 -> "هفتم".
pub fn ordinal_fa(n: u32) -> &'static str {
    ORDINALS
        .get((n as usize).clamp(1, 31) - 1)
        .copied()
        .unwrap_or("")
}

/// Persian digits for a number.
pub fn fa_num(n: i64) -> String {
    let s = n.to_string();
    s.chars()
        .map(|c| match c {
            '0' => '۰',
            '1' => '۱',
            '2' => '۲',
            '3' => '۳',
            '4' => '۴',
            '5' => '۵',
            '6' => '۶',
            '7' => '۷',
            '8' => '۸',
            '9' => '۹',
            _ => c,
        })
        .collect()
}

/// Weekday name for a Jalali date.
pub fn weekday_name(jy: i32, jm: u32, jd: u32) -> &'static str {
    let jdn = jalali_to_jdn(jy, jm, jd);
    // JDN 2451545 (1 Jan 2000) was a Saturday and 2451545 % 7 == 5,
    // so (jdn + 2) % 7 == 0 lands on Saturday.
    let idx = ((jdn + 2) % 7) as usize; // 0 = Saturday
    WEEKDAYS[idx]
}

/// English weekday name for a Jalali date.
pub fn weekday_name_en(jy: i32, jm: u32, jd: u32) -> &'static str {
    let jdn = jalali_to_jdn(jy, jm, jd);
    let idx = ((jdn + 2) % 7) as usize;
    WEEKDAYS_EN[idx]
}

/// One square of a month grid.
#[derive(Clone, Debug, PartialEq)]
pub struct GridCell {
    /// Day of the month, 1..=31. Zero for the leading blanks that pad the
    /// first week out to Saturday.
    pub day: u32,
    /// Jalali year and month this cell belongs to. For a blank cell these are
    /// still the month being displayed, so the caller can skip them by `day`.
    pub year: i32,
    pub month: u32,
    /// `YYYY-MM-DD` for a real day, empty for a blank.
    pub key: String,
    pub is_today: bool,
}

/// The month laid out as weeks of seven, Saturday first.
///
/// The Iranian week starts on Saturday, so the grid is padded with blanks at
/// the front until the 1st lands in the right column. The result is always a
/// whole number of weeks, so the grid never has a ragged last row.
pub fn month_grid(jy: i32, jm: u32, today: (i32, u32, u32)) -> Vec<GridCell> {
    let jm = jm.clamp(1, 12);
    let len = month_len(jy, jm);

    // Column of the 1st: 0 = Saturday .. 6 = Friday.
    let jdn = jalali_to_jdn(jy, jm, 1);
    let lead = ((jdn + 2) % 7) as u32;

    let mut cells = Vec::with_capacity(((lead + len).div_ceil(7) * 7) as usize);
    for _ in 0..lead {
        cells.push(GridCell {
            day: 0,
            year: jy,
            month: jm,
            key: String::new(),
            is_today: false,
        });
    }
    for day in 1..=len {
        cells.push(GridCell {
            day,
            year: jy,
            month: jm,
            key: format!("{jy:04}-{jm:02}-{day:02}"),
            is_today: today == (jy, jm, day),
        });
    }
    while cells.len() % 7 != 0 {
        cells.push(GridCell {
            day: 0,
            year: jy,
            month: jm,
            key: String::new(),
            is_today: false,
        });
    }
    cells
}

/// Step a Jalali month forward or back by one, rolling the year over.
pub fn shift_month(jy: i32, jm: u32, delta: i32) -> (i32, u32) {
    let mut y = jy;
    let mut m = jm as i32 + delta;
    while m < 1 {
        m += 12;
        y -= 1;
    }
    while m > 12 {
        m -= 12;
        y += 1;
    }
    (y, m as u32)
}

/// Move a `YYYY-MM-DD` Jalali key by a whole number of days.
///
/// This goes through the Julian day number rather than trying to add to the
/// day field, so it crosses month and year boundaries correctly and cannot
/// produce a day 31 in a 30-day month.
pub fn shift_day_key(key: &str, delta: i32) -> Option<String> {
    let mut it = key.split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    let jdn = jalali_to_jdn(y, m, d) + delta as i64;
    let (ny, nm, nd) = jdn_to_jalali(jdn);
    Some(format!("{ny:04}-{nm:02}-{nd:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates_round_trip() {
        // 1 Farvardin 1403 == 20 March 2024
        assert_eq!(to_gregorian(1403, 1, 1), (2024, 3, 20));
        // 12 Farvardin 1403 == 31 March 2024
        assert_eq!(to_gregorian(1403, 1, 12), (2024, 3, 31));
        // 1 Mehr 1403 == 22 September 2024
        assert_eq!(to_gregorian(1403, 7, 1), (2024, 9, 22));
        // 1 Farvardin 1404 == 21 March 2025
        assert_eq!(to_gregorian(1404, 1, 1), (2025, 3, 21));
    }

    #[test]
    fn gregorian_round_trip() {
        for &(gy, gm, gd) in &[
            (2024, 3, 20),
            (2024, 10, 28),
            (2025, 1, 1),
            (2026, 10, 28),
            (1999, 12, 31),
        ] {
            let j = from_gregorian(gy, gm, gd);
            assert_eq!(
                to_gregorian(j.0, j.1, j.2),
                (gy, gm, gd),
                "for {gy}-{gm}-{gd}"
            );
        }
    }

    #[test]
    fn leap_years() {
        assert!(is_leap(1403));
        assert!(!is_leap(1404));
        assert_eq!(month_len(1403, 12), 30);
        assert_eq!(month_len(1404, 12), 29);
    }

    #[test]
    fn day_names_and_feasts() {
        // 16 Mehr is Mehr-ruz in Mehr-mah -> Mehregan.
        let d = day_name(7, 16);
        assert_eq!(d.name, "مهر");
        assert!(d.is_feast);
        assert_eq!(d.feast, Some("مهرگان"));

        // 1 Farvardin is Ormazd-ruz; the Farvardin feast is on day 19.
        assert_eq!(day_name(1, 1).name, "اورمزد");
        assert!(!day_name(1, 1).is_feast);
        assert_eq!(day_name(1, 19).feast, Some("فروردینگان"));

        // 10 Aban is Aban-ruz in Aban-mah -> Abangan.
        assert_eq!(day_name(8, 10).feast, Some("آبانگان"));

        // 1 Dey is Dey-ruz -> Khorram-ruz.
        assert_eq!(day_name(10, 1).feast, Some("خرم روز"));
    }

    #[test]
    fn ordinals_read_naturally() {
        assert_eq!(ordinal_fa(1), "یکم");
        assert_eq!(ordinal_fa(7), "هفتم");
        assert_eq!(ordinal_fa(30), "سی‌ام");
        assert_eq!(ordinal_fa(31), "سی‌ویکم");
    }

    #[test]
    fn day_31_has_no_month_name() {
        let d = day_name(1, 31);
        assert_eq!(d.name, "روز افزوده");
        assert!(!d.is_feast);
    }

    #[test]
    fn weekday_is_correct() {
        // 20 March 2024 was a Wednesday -> چهارشنبه
        assert_eq!(weekday_name(1403, 1, 1), "چهارشنبه");
    }
}

#[cfg(test)]
mod era_tests {
    use super::*;
    #[test]
    fn era_year_counts_from_cyrus() {
        // 1405 SH is the year this app runs in; the era count is 2585.
        assert_eq!(era_year(1405), 2585); // The epoch itself: 559 BC is year 1 of the count.
        assert_eq!(era_year(-559), 621);
    }

    #[test]
    fn month_names_use_the_ancient_forms() {
        assert_eq!(MONTHS[4], "امرداد");
        assert_eq!(MONTHS[11], "سپندارمذ"); // The modern spellings are still available for Wikipedia lookups.
        assert_eq!(MONTHS_NEW_PERSIAN[4], "مرداد");
        assert_eq!(MONTHS_NEW_PERSIAN[11], "اسفند");
    }

    #[test]
    fn festivals_are_found() {
        // Nowruz, the first day of the year.
        assert_eq!(feast_on(1, 1).map(|f| f.0), Some("نوروز")); // Tirgan: the name-day feast of Tir, on Tir-ruz in Tir-mah.
        assert_eq!(feast_on(4, 13).map(|f| f.0), Some("تیرگان")); // Mehregan: Mehr-ruz in Mehr-mah.
        assert_eq!(feast_on(7, 16).map(|f| f.0), Some("مهرگان")); // Sadeh, the mid-winter fire festival.
        assert_eq!(feast_on(11, 10).map(|f| f.0), Some("سده")); // An ordinary day is not a festival.
        assert_eq!(feast_on(2, 11), None);
    }

    #[test]
    fn festivals_carry_a_note() {
        let (_, note) = feast_on(1, 1).unwrap();
        assert!(!note.is_empty(), "Nowruz should explain itself");
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    /// What the calendar screen actually puts on the date line, for today.
    #[test]
    fn today_reads_in_the_era_reckoning() {
        let (jy, jm, jd) = today();
        let era = era_year(jy);
        let month = MONTHS[(jm as usize).clamp(1, 12) - 1];
        let line = format!("{} {} {}", fa_num(jd as i64), month, fa_num(era as i64));
        println!("date line: {line}  (jalali {jy}-{jm}-{jd}, era {era})");
        assert_eq!(era, 2585, "expected the era year 2585 in 2026");
        assert!(
            line.contains("۲۵۸۵"),
            "date line should carry the era year: {line}"
        );
    }

    #[test]
    fn every_festival_is_reachable_and_named() {
        let mut found = 0;
        for m in 1..=12u32 {
            for d in 1..=31u32 {
                if let Some((name, _)) = feast_on(m, d) {
                    assert!(!name.is_empty());
                    found += 1;
                }
            }
        }
        println!("festival days in the year: {found}");
        assert!(
            found >= 18,
            "expected at least 18 festival days, got {found}"
        );
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;
    #[test]
    fn grid_is_whole_weeks_and_starts_on_saturday() {
        for m in 1..=12u32 {
            let g = month_grid(1405, m, (1405, 8, 7));
            assert_eq!(g.len() % 7, 0, "month {m} is not whole weeks"); // The first real day must land in the column its weekday says.
            let first = g.iter().position(|c| c.day == 1).unwrap();
            let jdn = jalali_to_jdn(1405, m, 1);
            assert_eq!(first as u32, ((jdn + 2) % 7) as u32);
        }
    }

    #[test]
    fn grid_holds_every_day_exactly_once() {
        let g = month_grid(1405, 8, (1405, 8, 7));
        let days: Vec<u32> = g.iter().filter(|c| c.day > 0).map(|c| c.day).collect();
        assert_eq!(days, (1..=30).collect::<Vec<u32>>());
    }

    #[test]
    fn today_is_marked_once() {
        let g = month_grid(1405, 8, (1405, 8, 7));
        let marked: Vec<u32> = g.iter().filter(|c| c.is_today).map(|c| c.day).collect();
        assert_eq!(marked, vec![7]);
    }

    #[test]
    fn keys_are_zero_padded_and_sortable() {
        let g = month_grid(1405, 8, (1405, 8, 7));
        let k = &g.iter().find(|c| c.day == 7).unwrap().key;
        assert_eq!(k, "1405-08-07");
    }

    #[test]
    fn month_stepping_rolls_the_year() {
        assert_eq!(shift_month(1405, 12, 1), (1406, 1));
        assert_eq!(shift_month(1405, 1, -1), (1404, 12));
        assert_eq!(shift_month(1405, 6, 0), (1405, 6));
        assert_eq!(shift_month(1405, 1, -13), (1403, 12));
    }
}

#[cfg(test)]
mod step_tests {
    use super::*;
    #[test]
    fn stepping_a_day_moves_one_day_each_way() {
        assert_eq!(shift_day_key("1405-08-07", 1).unwrap(), "1405-08-08");
        assert_eq!(shift_day_key("1405-08-07", -1).unwrap(), "1405-08-06");
    }

    #[test]
    fn stepping_crosses_month_and_year_boundaries() {
        // 30 Aban + 1 = 1 Azar.
        assert_eq!(shift_day_key("1405-08-30", 1).unwrap(), "1405-09-01");
        // 1 Farvardin - 1 = the last day of the previous Esfand. 1404 is not a
        // leap year, so that is 29 Esfand, not 30 — the step goes through the
        // day number rather than assuming a fixed month length.
        assert!(!is_leap(1404));
        assert_eq!(shift_day_key("1405-01-01", -1).unwrap(), "1404-12-29");
        // 31 Tir + 1 = 1 Amordad (Tir has 31 days).
        assert_eq!(shift_day_key("1405-04-31", 1).unwrap(), "1405-05-01");
    }

    #[test]
    fn stepping_never_produces_an_invalid_day() {
        // Walk a whole year forward one day at a time and check every step is
        // a real date with a day inside its month's length.
        let mut key = "1405-01-01".to_string();
        for _ in 0..366 {
            let mut it = key.split('-');
            let y: i32 = it.next().unwrap().parse().unwrap();
            let m: u32 = it.next().unwrap().parse().unwrap();
            let d: u32 = it.next().unwrap().parse().unwrap();
            assert!(d >= 1 && d <= month_len(y, m), "invalid day in {key}");
            key = shift_day_key(&key, 1).unwrap();
        }
    }
}
