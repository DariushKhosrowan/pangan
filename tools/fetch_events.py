#!/usr/bin/env python3
"""Fetch "on this day" data from Wikipedia into data/day_events.json.

Two independent streams, kept apart on purpose:

  iran   Persian heritage — read from the fa.wikipedia page for each Jalali
         month-day ("۸ آبان"). This is the material that leads the screen.
  world  General world history — read from the "<Month> <Day>" page, which is
         keyed by the *Gregorian* day, so the app looks it up after converting
         the Jalali date. Persian text comes from fa.wikipedia ("۷ اوت"),
         English text from en.wikipedia ("August 7").

Usage:
    python3 tools/fetch_events.py            # full refresh
    python3 tools/fetch_events.py --days 3   # smoke test on a few days
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor

UA = "Pangan/0.4 (https://github.com/DariushKhosrowan/pangan; offline calendar data)"

JALALI_MONTHS = [
    "فروردین", "اردیبهشت", "خرداد", "تیر", "مرداد", "شهریور",
    "مهر", "آبان", "آذر", "دی", "بهمن", "اسفند",
]
GREG_MONTHS_EN = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
]
GREG_MONTHS_FA = [
    "ژانویه", "فوریه", "مارس", "آوریل", "مه", "ژوئن",
    "ژوئیه", "اوت", "سپتامبر", "اکتبر", "نوامبر", "دسامبر",
]

# Section headings we care about, per language.
IRAN_SECTIONS = {
    "رویدادها": "event",
    "زادروزها": "birth",
    "درگذشت‌ها": "death",
    "درگذشت‌ها و سالگردها": "death",
    "مناسبت‌ها": "occasion",
    "تعطیلات و مناسبت‌ها": "occasion",
}


def api(lang: str, page: str) -> str | None:
    """Rendered HTML for a page, or None when the page does not exist.

    Parsing rendered HTML rather than raw wikitext is what makes this robust:
    links, templates and refs are already resolved, so there is no markup left
    to mis-strip. Mirrors src/refresh.rs.
    """
    url = (
        f"https://{lang}.wikipedia.org/w/api.php?action=parse&format=json"
        f"&prop=text&formatversion=2&redirects=1&page={urllib.parse.quote(page)}"
    )
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=45) as r:
                data = json.load(r)
            if "error" in data:
                return None
            return data["parse"]["text"]
        except Exception:
            if attempt == 2:
                return None
            time.sleep(1.5 * (attempt + 1))
    return None


# --- HTML cleaning -----------------------------------------------------------

import html as _html

RE_SUP = re.compile(r"<sup.*?</sup>", re.S | re.I)
RE_STYLE = re.compile(r"<(style|script).*?</\1>", re.S | re.I)
RE_TAG = re.compile(r"<[^>]+>")
RE_WS = re.compile(r"\s+")


def clean(fragment: str) -> str:
    """Strip HTML down to readable prose. Mirrors refresh.rs::html_text."""
    t = RE_SUP.sub(" ", fragment)
    t = RE_STYLE.sub(" ", t)
    t = RE_TAG.sub(" ", t)
    t = _html.unescape(t)
    t = RE_WS.sub(" ", t).strip()
    # Tags become spaces, leaving "word ," before punctuation.
    for p in (",", ";", ":", ".", "!", "?", ")", "\u201d", "\u2019"):
        t = t.replace(f" {p}", p)
    for p in ("(", "\u201c", "\u2018"):
        t = t.replace(f"{p} ", p)
    return t.strip(" ,;:\u2013-")


def split_sections(html: str) -> list[tuple[str, str]]:
    """(heading, section-html) pairs, in document order."""
    marks = []
    for m in re.finditer(r"<h2[^>]*>(.*?)</h2>", html, re.S | re.I):
        marks.append((m.start(), m.end(), clean(m.group(1))))
    out = []
    for i, (h_start, body_start, title) in enumerate(marks):
        end = marks[i + 1][0] if i + 1 < len(marks) else len(html)
        out.append((title, html[body_start:end]))
    return out


def list_items(section_html: str) -> list[str]:
    """The <li> items of the first <ul> in a section."""
    m = re.search(r"<ul>(.*?)</ul>", section_html, re.S | re.I)
    if not m:
        return []
    items = []
    for li in re.finditer(r"<li>(.*?)</li>", m.group(1), re.S | re.I):
        text = clean(li.group(1))
        if len(text) > 12:
            items.append(text)
    return items


def all_list_items(section_html: str) -> list[str]:
    """Every <li> in every <ul> of a section (for English year subsections)."""
    items = []
    for ul in re.finditer(r"<ul>(.*?)</ul>", section_html, re.S | re.I):
        for li in re.finditer(r"<li>(.*?)</li>", ul.group(1), re.S | re.I):
            text = clean(li.group(1))
            if len(text) > 12:
                items.append(text)
    return items


# --- Iranian stream ----------------------------------------------------------

def fetch_iran(jm: int, jd: int) -> dict:
    page = f"{jd} {JALALI_MONTHS[jm - 1]}"
    html = api("fa", page)
    if not html:
        return {"title": page, "events": [], "births": [], "deaths": [], "occasions": []}

    buckets = {"event": [], "birth": [], "death": [], "occasion": []}
    for heading, body in split_sections(html):
        kind = IRAN_SECTIONS.get(heading.strip())
        if not kind:
            continue
        buckets[kind].extend(list_items(body))

    return {
        "title": page,
        "events": buckets["event"],
        "births": buckets["birth"],
        "deaths": buckets["death"],
        "occasions": buckets["occasion"],
    }


# --- World stream ------------------------------------------------------------

def fetch_world(lang: str, gm: int, gd: int) -> list[str]:
    if lang == "fa":
        page = f"{gd} {GREG_MONTHS_FA[gm - 1]}"
        wanted = "رویدادها"
    else:
        page = f"{GREG_MONTHS_EN[gm - 1]} {gd}"
        wanted = "Events"

    html = api(lang, page)
    if not html:
        return []

    for heading, body in split_sections(html):
        if heading.strip() == wanted:
            # The English page nests events under year-range h3 headings, so
            # take every list in the section, not just the first.
            return all_list_items(body)
    return []


def month_len_g(gy: int, gm: int) -> int:
    if gm == 2:
        leap = gy % 4 == 0 and (gy % 100 != 0 or gy % 400 == 0)
        return 29 if leap else 28
    return 30 if gm in (4, 6, 9, 11) else 31


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=0, help="only fetch N days (smoke test)")
    ap.add_argument("--out", default="data/day_events.json")
    args = ap.parse_args()

    # Iranian: 12 months x 31 days (day 31 only exists in months 1..6).
    iran_keys = []
    for jm in range(1, 13):
        for jd in range(1, 32):
            if jd == 31 and jm > 6:
                continue
            iran_keys.append((jm, jd))

    # World: every Gregorian month-day, leap year 2024 so 02-29 exists.
    world_keys = []
    for gm in range(1, 13):
        for gd in range(1, month_len_g(2024, gm) + 1):
            world_keys.append((gm, gd))

    if args.days:
        iran_keys = iran_keys[: args.days]
        world_keys = world_keys[: args.days]

    iran: dict[str, dict] = {}
    world_fa: dict[str, list[str]] = {}
    world_en: dict[str, list[str]] = {}

    def do_iran(key):
        jm, jd = key
        return f"{jm}-{jd}", fetch_iran(jm, jd)

    def do_world(key):
        gm, gd = key
        k = f"{gm:02d}-{gd:02d}"
        return k, fetch_world("fa", gm, gd), fetch_world("en", gm, gd)

    with ThreadPoolExecutor(max_workers=6) as ex:
        for i, (k, v) in enumerate(ex.map(do_iran, iran_keys), 1):
            iran[k] = v
            if i % 40 == 0:
                print(f"  iran {i}/{len(iran_keys)}", file=sys.stderr)

        for i, (k, fa, en) in enumerate(ex.map(do_world, world_keys), 1):
            if fa:
                world_fa[k] = fa
            if en:
                world_en[k] = en
            if i % 40 == 0:
                print(f"  world {i}/{len(world_keys)}", file=sys.stderr)

    payload = {
        "version": 2,
        "source": "fa.wikipedia.org / en.wikipedia.org",
        "iran": iran,
        "world_fa": world_fa,
        "world_en": world_en,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False, separators=(",", ":"))

    ni = sum(len(v["events"]) for v in iran.values())
    print(
        f"wrote {args.out}: iran {len(iran)} days / {ni} events; "
        f"world_fa {len(world_fa)} days; world_en {len(world_en)} days",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
