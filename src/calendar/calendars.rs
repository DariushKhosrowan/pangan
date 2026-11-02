//! Alternative calendar reckonings: Gregorian and Hijri (Islamic tabular).
//!
//! Pangan is born Persian, but time is not one thing. This module lets the
//! month grid and date labels run on the proleptic Gregorian calendar or the
//! Hijri (Islamic) calendar, in addition to the default Jalali one.
//!
//! Everything here converts through the same Julian Day Number (JDN) that
//! `jalali` uses, so switching calendars never changes which physical day is
//! being described — only how it is labelled and laid out.
//!
//! The Hijri reckoning used here is the standard *tabular* Islamic calendar
//! (arithmetic, 30-year cycle with 11 leap years), not the moon-sighting
//! one. It is deterministic and reproducible, which is what a planning tool
//! needs.

/// Hijri month names, transliterated, for the English UI.
pub const HIJRI_MONTHS_EN: [&str; 12] = [
    "Muharram",
    "Safar",
    "Rabi' al-Awwal",
    "Rabi' al-Thani",
    "Jumada al-Ula",
    "Jumada al-Thaniya",
    "Rajab",
    "Sha'ban",
    "Ramadan",
    "Shawwal",
    "Dhu al-Qi'dah",
    "Dhu al-Hijjah",
];

/// Hijri month names in Persian for the Persian UI.
pub const HIJRI_MONTHS_FA: [&str; 12] = [
    "محرم",
    "صفر",
    "ربیع‌الأول",
    "ربیع‌الثانی",
    "جمادی‌الأول",
    "جمادی‌الثانی",
    "رجب",
    "شعبان",
    "رمضان",
    "شوال",
    "ذی‌القعده",
    "ذی‌الحجه",
];

/// Is `year` a leap year in the 30-year Hijri cycle?
pub fn hijri_is_leap(year: i32) -> bool {
    ((year * 11 + 14) % 30) < 11
}

/// Days in a Hijri month.
pub fn hijri_month_len(year: i32, month: u32) -> u32 {
    if month <= 6 {
        30
    } else if month == 12 {
        if hijri_is_leap(year) {
            30
        } else {
            29
        }
    } else {
        29
    }
}

/// Convert a Hijri date to a Julian Day Number.
///
/// Epoch: 1 Muharram 1 AH = JDN 1948439 (Friday, 16 July 622 CE).
pub fn hijri_to_jdn(y: i32, m: u32, d: u32) -> i64 {
    const EPOCH: i64 = 1948439;
    EPOCH
        + (y as i64 - 1) * 354
        + ((3 + 11 * y as i64) / 30)
        + (m as i64 - 1) * 29
        + (m as i64) / 2
        + d as i64
        - 1
}

/// Convert a Julian Day Number to a Hijri date.
pub fn jdn_to_hijri(jdn: i64) -> (i32, u32, u32) {
    const EPOCH: i64 = 1948439;
    let days = jdn - EPOCH;
    let mut year = ((30 * days + 10646) / 10631) as i32;
    while hijri_to_jdn(year, 1, 1) > jdn {
        year -= 1;
    }
    let mut month = 1u32;
    while month < 12 && hijri_to_jdn(year, month + 1, 1) <= jdn {
        month += 1;
    }
    let day = (jdn - hijri_to_jdn(year, month, 1) + 1) as u32;
    (year, month, day)
}

/// Gregorian month names in English.
pub const GREG_MONTHS_EN: [&str; 12] = [
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

/// Gregorian month names in Persian.
pub const GREG_MONTHS_FA: [&str; 12] = [
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

/// Days in a Gregorian month.
pub fn greg_month_len(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Is `year` a Gregorian leap year?
pub fn greg_is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// A single cell in a month grid, calendar-agnostic.
#[derive(Clone, Debug)]
pub struct GridCell {
    /// Day of the month, 0 for leading/trailing blank padding.
    pub day: u32,
    pub year: i32,
    pub month: u32,
    /// `YYYY-MM-DD` key in this calendar's reckoning; empty for blanks.
    pub key: String,
    pub is_today: bool,
}

/// Lay out a Gregorian month as weeks of seven, Saturday first.
pub fn greg_month_grid(y: i32, m: u32, today: (i32, u32, u32)) -> Vec<GridCell> {
    let m = m.clamp(1, 12);
    let len = greg_month_len(y, m);
    let jdn = crate::calendar::jalali::gregorian_to_jdn(y, m, 1);
    let lead = ((jdn + 2) % 7) as u32;
    let mut cells = Vec::with_capacity(((lead + len).div_ceil(7) * 7) as usize);
    for _ in 0..lead {
        cells.push(GridCell {
            day: 0,
            year: y,
            month: m,
            key: String::new(),
            is_today: false,
        });
    }
    for day in 1..=len {
        cells.push(GridCell {
            day,
            year: y,
            month: m,
            key: format!("{y:04}-{m:02}-{day:02}"),
            is_today: today == (y, m, day),
        });
    }
    while cells.len() % 7 != 0 {
        cells.push(GridCell {
            day: 0,
            year: y,
            month: m,
            key: String::new(),
            is_today: false,
        });
    }
    cells
}

/// Lay out a Hijri month as weeks of seven, Saturday first.
pub fn hijri_month_grid(y: i32, m: u32, today: (i32, u32, u32)) -> Vec<GridCell> {
    let m = m.clamp(1, 12);
    let len = hijri_month_len(y, m);
    let jdn = hijri_to_jdn(y, m, 1);
    let lead = ((jdn + 2) % 7) as u32;
    let mut cells = Vec::with_capacity(((lead + len).div_ceil(7) * 7) as usize);
    for _ in 0..lead {
        cells.push(GridCell {
            day: 0,
            year: y,
            month: m,
            key: String::new(),
            is_today: false,
        });
    }
    for day in 1..=len {
        cells.push(GridCell {
            day,
            year: y,
            month: m,
            key: format!("{y:04}-{m:02}-{day:02}"),
            is_today: today == (y, m, day),
        });
    }
    while cells.len() % 7 != 0 {
        cells.push(GridCell {
            day: 0,
            year: y,
            month: m,
            key: String::new(),
            is_today: false,
        });
    }
    cells
}

/// Step a month forward/back across a 12-month year.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let mut y = year;
    let mut m = month as i32 + delta;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gregorian_known_dates() {
        // 20 March 2024 == 1 Farvardin 1403 (Jalali) == JDN 2460390.
        let jdn = crate::calendar::jalali::gregorian_to_jdn(2024, 3, 20);
        assert_eq!(jdn, 2460390);
        assert_eq!(
            crate::calendar::jalali::jdn_to_gregorian(jdn),
            (2024, 3, 20)
        );
    }

    #[test]
    fn hijri_epoch() {
        // 1 Muharram 1 AH == JDN 1948439.
        assert_eq!(hijri_to_jdn(1, 1, 1), 1948439);
    }

    #[test]
    fn hijri_known_round_trip() {
        // 1 Ramadan 1445 AH ≈ 10 March 2024 in the tabular (arithmetic)
        // Hijri calendar. Round-trip through JDN must be lossless.
        let jdn = hijri_to_jdn(1445, 9, 1);
        let (gy, gm, gd) = crate::calendar::jalali::jdn_to_gregorian(jdn);
        let back = jdn_to_hijri(jdn);
        assert_eq!(back, (1445, 9, 1));
        assert_eq!(gy, 2024);
        assert_eq!(gm, 3);
        assert_eq!(gd, 10);
    }

    #[test]
    fn hijri_leap_cycle() {
        // The 30-year cycle has 11 leap years: years 2,5,7,10,13,16,18,21,24,26,29.
        let leaps = [2i32, 5, 7, 10, 13, 16, 18, 21, 24, 26, 29];
        for y in 1..=30 {
            assert_eq!(hijri_is_leap(y), leaps.contains(&y), "year {y}");
        }
    }

    #[test]
    fn hijri_month_lengths() {
        assert_eq!(hijri_month_len(1445, 1), 30); // Muharram
        assert_eq!(hijri_month_len(1445, 6), 30); // Jumada al-Thaniya
        assert_eq!(hijri_month_len(1445, 7), 29); // Rajab
        assert_eq!(hijri_month_len(1445, 8), 29); // Sha'ban
                                                  // 1445 is a leap year (1445 mod 30 = 5 → leap), so Dhu al-Hijjah is 30.
        assert_eq!(hijri_month_len(1445, 12), 30);
        assert_eq!(hijri_month_len(1444, 12), 29); // non-leap
    }

    #[test]
    fn greg_grid_is_7_aligned() {
        let cells = greg_month_grid(2024, 3, (2024, 3, 20));
        assert_eq!(cells.len() % 7, 0);
        assert!(cells.iter().any(|c| c.is_today));
        // 1 March 2024 is a Friday; Saturday-first → lead column 5 (0-based).
        assert_eq!(cells[0].day, 0);
    }

    #[test]
    fn hijri_grid_is_7_aligned() {
        let cells = hijri_month_grid(1445, 9, (1445, 9, 1));
        assert_eq!(cells.len() % 7, 0);
        assert!(cells.iter().any(|c| c.is_today));
    }

    #[test]
    fn shift_month_rolls_year() {
        assert_eq!(shift_month(1445, 12, 1), (1446, 1));
        assert_eq!(shift_month(1445, 1, -1), (1444, 12));
    }
}
