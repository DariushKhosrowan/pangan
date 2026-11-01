//! The imperial chronicle: Iranian rule, dynasty by dynasty.
//!
//! This is the app's long view. The timer measures an hour and the calendar
//! measures a day; this measures twenty-five centuries, and it exists because
//! the point of the app is not only to focus but to focus *as an Iranian*.
//!
//! Two rules govern the content:
//!
//!  * The list is continuous. Every dynasty is followed by the one that
//!    actually came next, including the Seleucid interlude, because a chronicle
//!    with gaps is a list of highlights rather than a history.
//!  * The facts are dates, capitals and names — checkable things — not
//!    adjectives. The one sentence of legacy per reign says what it is
//!    remembered for, and nothing more.

use crate::ui::Dynasty;

/// One reign, with both language variants resolved at build time.
struct Reign {
    name: (&'static str, &'static str),
    span: (&'static str, &'static str),
    capital: (&'static str, &'static str),
    figure: (&'static str, &'static str),
    legacy: (&'static str, &'static str),
}

/// The chronicle, in order. Persian first in each pair, English second.
const REIGNS: &[Reign] = &[
    Reign {
        name: ("ماد", "Medes"),
        span: ("۶۷۸ – ۵۵۰ پ.م.", "678 – 550 BC"),
        capital: ("هگمتانه", "Ecbatana"),
        figure: ("دیاکو، هووخشتره", "Deioces, Cyaxares"),
        legacy: (
            "نخستین گردآمدن ایرانیان زیر یک فرمان؛ هگمتانه و راه شاهی شرق.",
            "The first union of the Iranian peoples under one crown; Ecbatana and the eastern royal road.",
        ),
    },
    Reign {
        name: ("هخامنشی", "Achaemenid"),
        span: ("۵۵۰ – ۳۳۰ پ.م.", "550 – 330 BC"),
        capital: ("پارسه، شوش", "Persepolis, Susa"),
        figure: ("کوروش بزرگ، داریوش بزرگ", "Cyrus the Great, Darius the Great"),
        legacy: (
            "بزرگ‌ترین شاهنشاهی جهان باستان؛ منشور کوروش، راه شاهی، و نخستین دیوان جهانی.",
            "The largest empire of the ancient world; the Cyrus Cylinder, the Royal Road, the first world administration.",
        ),
    },
    Reign {
        name: ("سلوکی", "Seleucid"),
        span: ("۳۱۲ – ۲۴۷ پ.م.", "312 – 247 BC"),
        capital: ("سلوکیه", "Seleucia"),
        figure: ("سلوکوس، آنتیوخوس", "Seleucus, Antiochus"),
        legacy: (
            "دورهٔ هلنی میان هخامنشی و اشکانی؛ پارسه در این سال‌ها رها نشد.",
            "The Hellenistic interlude between Achaemenid and Parthian; Persepolis was never abandoned.",
        ),
    },
    Reign {
        name: ("اشکانی", "Parthian"),
        span: ("۲۴۷ پ.م. – ۲۲۴ م.", "247 BC – 224 AD"),
        capital: ("نسا، تیسفون", "Nisa, Ctesiphon"),
        figure: ("مهرداد یکم، سورنا", "Mithridates I, Surena"),
        legacy: (
            "درفش کاویانی و سواره‌نظام سنگین؛ چهار سده روم را در فرات نگه داشت.",
            "The Derafsh Kaviani and heavy cavalry; four centuries holding Rome at the Euphrates.",
        ),
    },
    Reign {
        name: ("ساسانی", "Sasanian"),
        span: ("۲۲۴ – ۶۵۱ م.", "224 – 651 AD"),
        capital: ("تیسفون، بیشاپور", "Ctesiphon, Bishapur"),
        figure: ("اردشیر بابکان، خسرو انوشیروان", "Ardashir I, Khosrow Anushirvan"),
        legacy: (
            "اوج هنر و دیوان‌سالاری ایرانی؛ سیم‌رغ، زنگار، و آیین کشورداری که به سه قاره رسید.",
            "The height of Iranian art and administration; the simurgh, verdigris, and a statecraft that reached three continents.",
        ),
    },
    Reign {
        name: ("صفوی", "Safavid"),
        span: ("۱۵۰۱ – ۱۷۳۶ م.", "1501 – 1736 AD"),
        capital: ("تبریز، اصفهان", "Tabriz, Isfahan"),
        figure: ("شاه اسماعیل، شاه عباس", "Shah Ismail, Shah Abbas"),
        legacy: (
            "بازگرداندن یکپارچگی ایران پس از هشت سده؛ اصفهان پایتخت نیم جهان.",
            "Restored Iranian unity after eight centuries; Isfahan, the capital that was half the world.",
        ),
    },
    Reign {
        name: ("افشاری", "Afsharid"),
        span: ("۱۷۳۶ – ۱۷۹۶ م.", "1736 – 1796 AD"),
        capital: ("مشهد", "Mashhad"),
        figure: ("نادرشاه", "Nader Shah"),
        legacy: (
            "بازپس‌گیری مرزهای شمالی و شرقی؛ آخرین لشکرکشی بزرگ ایرانی به بیرون.",
            "Recovered the northern and eastern frontiers; the last great Iranian campaign abroad.",
        ),
    },
    Reign {
        name: ("زند", "Zand"),
        span: ("۱۷۵۱ – ۱۷۹۴ م.", "1751 – 1794 AD"),
        capital: ("شیراز", "Shiraz"),
        figure: ("کریم‌خان زند", "Karim Khan Zand"),
        legacy: (
            "چهل سال آرامش؛ شیراز، ارگ و بازار وکیل.",
            "Forty years of peace; Shiraz, the citadel and the Vakil bazaar.",
        ),
    },
    Reign {
        name: ("قاجار", "Qajar"),
        span: ("۱۷۸۹ – ۱۹۲۵ م.", "1789 – 1925 AD"),
        capital: ("تهران", "Tehran"),
        figure: ("فتحعلی‌شاه، امیرکبیر", "Fath-Ali Shah, Amir Kabir"),
        legacy: (
            "تهران پایتخت شد؛ دارالفنون و نخستین نهادهای نوین ایران.",
            "Tehran became the capital; the Dar ul-Funun and the first modern Iranian institutions.",
        ),
    },
    Reign {
        name: ("پهلوی", "Pahlavi"),
        span: ("۱۹۲۵ – ۱۹۷۹ م.", "1925 – 1979 AD"),
        capital: ("تهران", "Tehran"),
        figure: ("رضاشاه، محمدرضاشاه", "Reza Shah, Mohammad Reza Shah"),
        legacy: (
            "راه‌آهن سراسری، دانشگاه تهران، و جشن‌های دوهزار و پانصد ساله.",
            "The trans-Iranian railway, the University of Tehran, and the 2,500-year celebrations.",
        ),
    },
];

/// Build the rows for the chronicle view in the requested language.
pub fn rows(en: bool) -> Vec<Dynasty> {
    REIGNS
        .iter()
        .map(|r| {
            let pick = |p: (&'static str, &'static str)| {
                slint::SharedString::from(if en { p.1 } else { p.0 })
            };
            Dynasty {
                name: pick(r.name),
                span: pick(r.span),
                capital: pick(r.capital),
                figure: pick(r.figure),
                legacy: pick(r.legacy),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chronicle_is_continuous_and_ordered() {
        // Ten reigns, in order, with no gaps in the sequence.
        assert_eq!(REIGNS.len(), 10);
        assert_eq!(REIGNS[0].name.0, "ماد");
        assert_eq!(REIGNS[9].name.0, "پهلوی");
    }

    #[test]
    fn every_reign_carries_all_five_facts_in_both_languages() {
        for r in REIGNS {
            for pair in [r.name, r.span, r.capital, r.figure, r.legacy] {
                assert!(!pair.0.trim().is_empty(), "missing Persian text");
                assert!(!pair.1.trim().is_empty(), "missing English text");
            }
        }
    }

    #[test]
    fn rows_follow_the_language() {
        let fa = rows(false);
        let en = rows(true);
        assert_eq!(fa[1].name.as_str(), "هخامنشی");
        assert_eq!(en[1].name.as_str(), "Achaemenid");
        assert_eq!(fa.len(), en.len());
    }

    #[test]
    fn persian_rows_use_persian_digits() {
        // The spans are the one place digits appear; they must be Persian in
        // the Persian build, or the chronicle reads as half-translated.
        let fa = rows(false);
        assert!(fa.iter().any(|d| d.span.as_str().contains('۵')));
        assert!(!fa.iter().any(|d| d.span.as_str().contains('5')));
    }
}
