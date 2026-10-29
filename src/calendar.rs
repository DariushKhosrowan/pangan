// ═══════════════════════════════════════════════════════════════════════════
// calendar.rs — Jalali (Solar Hijri) calendar arithmetic.
//
// No external crate: the Jalali year length follows the 33-year arithmetic
// cycle, which stays accurate for the years this app will ever display.
// ═══════════════════════════════════════════════════════════════════════════

/// Persian month names, in order.
pub const MONTHS: [&str; 12] = [
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
    let r = ((year % 33) + 33) % 33;
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
    if day >= 1 && day <= 30 {
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
