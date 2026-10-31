use std::sync::{
    atomic::Ordering,
    mpsc::{channel, Sender},
    Arc, Mutex,
};
use std::time::Duration;

use slint::{
    CloseRequestResponse, ComponentHandle, Global, ModelRc, SharedString, Timer, TimerMode,
    VecModel,
};

use crate::app::automation;
use crate::audio::sound;
use crate::calendar::{chronicle, events, jalali as calendar, refresh};
use crate::journal;
use crate::state::{settings, store};
use crate::stats::report as stats;
use crate::sync::{backup as sync, cloudsync, gdrive, oauth, syncfile};
use crate::timer::clock;

use clock::Shared;
use settings::Settings;
use store::Store;

use crate::ui::{InscriptionItem, PanganWindow};

/// Actions that tray menu / hotkey events translate into.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrayAction {
    Toggle,
    Skip,
    Reset,
    Quit,
    ToggleWindow,
}

/// User-facing app state shared with the UI update closures.
struct AppState {
    store: Mutex<Store>,
    settings: Mutex<Settings>,
    /// Active sound theme ("santur", "daf", ...).
    sound_theme: Mutex<String>,
    last_sync: Mutex<String>,
    /// "On this day" table, parsed once at startup (287 KB of JSON).
    events: Mutex<events::Events>,
    /// The Jalali date the calendar card was last built for, so the card is
    /// rebuilt when the day rolls over and not on every 300 ms tick.
    calendar_day: Mutex<(i32, u32, u32)>,
    /// Which day the calendar screen is showing tasks for, as a `YYYY-MM-DD`
    /// key. Empty until the screen is first opened, when it is set to today.
    selected_day: Mutex<String>,
    /// The Jalali month the grid is showing, independent of the selected day:
    /// you can page to next month and look around without moving the selection.
    shown_month: Mutex<(i32, u32)>,
    /// Which report the Stats screen is showing: the period shape and how many
    /// periods back. Kept in app state rather than in the Slint window so the
    /// choice survives the 300 ms tick that rebuilds the screen.
    report_period: Mutex<stats::Period>,
    report_offset: Mutex<i32>,
    /// Automatic cloud sync: the timer reads this, the UI writes it.
    cloud: Arc<cloudsync::Engine>,
    /// Google credentials, refreshed in place when they age out.
    tokens: Arc<Mutex<gdrive::Tokens>>,
    /// This machine's sync identity and its remembered settings stamps.
    ///
    /// The stamps have to survive a restart: rebuilt from scratch on every
    /// launch, every launch would look like "the user just changed everything"
    /// and this device would clobber the other one's edits on the first sync.
    device: Arc<Mutex<syncfile::DeviceState>>,
    /// The timer core. Held here so a sync that delivers new durations can
    /// apply them to the running clock, not just to the settings screen.
    shared: Arc<Shared>,
    /// The search text the archive is filtered by. Kept here rather than in the
    /// window so the 300 ms tick that rebuilds the archive list cannot clear
    /// what the user typed.
    ins_query: Mutex<String>,
    /// A session the user chose not to write about.
    ///
    /// Without this, the tick that notices an unwritten session would reopen
    /// the dialog every 300 ms after a Skip, and the only way out would be to
    /// write something. One remembered timestamp is enough: only the newest
    /// session is ever offered.
    ins_skipped: Mutex<Option<i64>>,
    /// The intention stated for the block that is running right now, waiting
    /// for the block to finish so it can be paired with its session.
    ///
    /// It cannot be written to the store yet: a session is keyed by the instant
    /// it *ended*, and that instant does not exist until the block is over. So
    /// the text and the moment it was typed are held here, and the tick that
    /// drains completed sessions turns the pair into a `Decree`.
    ///
    /// Plain `Option<(String, i64)>` and not a `Decree`: everything in the
    /// journal module is a finished record, and a record with no session yet
    /// would be a lie told in a struct. The conversion happens in one place.
    pending_decree: Mutex<Option<(String, i64)>>,
}

impl AppState {
    fn load(shared: Arc<Shared>) -> Self {
        let store = Store::load();
        let settings = Settings::load();
        let sound_theme = settings.sound_theme.clone();
        let last_sync = if settings.lang == "en" {
            "Cloud sync ready".to_string()
        } else {
            "همگام‌سازی ابری آماده است".to_string()
        };
        let device = syncfile::DeviceState::load(&settings);
        Self {
            store: Mutex::new(store),
            settings: Mutex::new(settings),
            sound_theme: Mutex::new(sound_theme),
            last_sync: Mutex::new(last_sync),
            // A cached table is only used when it is actually better than the
            // copy compiled into the binary; a stale or damaged cache must not
            // be able to show worse data than the app ships with.
            events: Mutex::new(match refresh::load_cached() {
                Some(raw) => events::Events::from_cache_or_embedded(&raw),
                None => events::Events::load(),
            }),
            calendar_day: Mutex::new((0, 0, 0)),
            selected_day: Mutex::new(String::new()),
            shown_month: Mutex::new((0, 0)),
            report_period: Mutex::new(stats::Period::Week),
            report_offset: Mutex::new(0),
            cloud: Arc::new(cloudsync::Engine::new()),
            tokens: Arc::new(Mutex::new(sync::load_tokens().unwrap_or_default())),
            device: Arc::new(Mutex::new(device)),
            shared,
            ins_query: Mutex::new(String::new()),
            ins_skipped: Mutex::new(None),
            pending_decree: Mutex::new(None),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)] // test support, not called from the app
    fn save_settings(&self) {
        let s = self.settings.lock().unwrap().clone();
        s.save();
        // Keep the fast sound-theme mirror in sync.
        *self.sound_theme.lock().unwrap() = s.sound_theme.clone();
    }
}

/// Resolve the effective theme ("dark"|"light") from a preference.
fn resolve_theme(pref: &str) -> &'static str {
    if pref == "light" {
        return "light";
    }
    if pref == "dark" {
        return "dark";
    }
    // system -> ask the desktop, then fall back to light.
    theme_from_portal()
        .or_else(theme_from_gsettings)
        .unwrap_or("light")
}

/// Read the colour preference from the XDG desktop portal.
///
/// Returns Some("dark") or Some("light"), and None when the desktop has no
/// preference. The portal encodes it as 0 (no preference), 1 (dark) or 2
/// (light). Zero must not be mistaken for dark: that is exactly the bug that
/// made the app open dark right after the user had chosen light.
fn theme_from_portal() -> Option<&'static str> {
    let out = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    if s.contains("uint32 2") {
        return Some("light");
    }
    if s.contains("uint32 1") {
        return Some("dark");
    }
    None
}

/// Read the colour preference from GTK settings, for desktops that do not
/// expose the portal's colour-scheme key.
fn theme_from_gsettings() -> Option<&'static str> {
    let out = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    if s.contains("prefer-dark") {
        return Some("dark");
    }
    if s.contains("prefer-light") {
        return Some("light");
    }
    // Older desktops encode the preference in the theme name instead.
    let theme = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
        .ok()?;
    let t = String::from_utf8_lossy(&theme.stdout).to_lowercase();
    if t.contains("dark") {
        return Some("dark");
    }
    if t.contains("light") {
        return Some("light");
    }
    None
}

// ---------------------------------------------------------------------------
// UI glue
// ---------------------------------------------------------------------------

fn apply_snapshot(win: &PanganWindow, shared: &Shared) {
    let snap = shared.snapshot();

    win.set_time_left(SharedString::from(snap.time));
    win.set_time_minutes(snap.minutes as i32);
    win.set_time_seconds(snap.seconds as i32);
    win.set_session_name(SharedString::from(snap.session));
    win.set_progress(snap.progress);
    win.set_phase(SharedString::from(snap.phase));
    win.set_running(snap.running);
    win.set_work_done(snap.work_done as i32);
}

/// Open an http(s) URL in the user's default browser.
///
/// Only http(s) is accepted, so a stray string can never launch a local binary.
/// Used by the About links and by the sign-in flow, which opens Google's device
/// page for the user rather than making them copy a URL by hand.
fn open_in_browser(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    #[cfg(target_os = "linux")]
    let opener = "xdg-open";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";

    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Flush pending changes before the app goes away.
///
/// Automatic sync already pushes a change within a tick of it happening, so
/// quitting is normally a no-op. But the last edit before a quit can still be
/// in flight, and quitting on it used to mean the edit sat unsynced until the
/// next launch — and if the user never reopened the app, it never reached their
/// other machine at all. This runs one synchronous cycle with a hard timeout,
/// so a stalled network cannot trap the process; whatever is still dirty stays
/// on disk and goes out next time.
fn flush_on_quit(app: &Arc<AppState>) {
    if !app.cloud.take_dirty() {
        return;
    }
    if app.tokens.lock().unwrap().is_empty() || !app.settings.lock().unwrap().sync_enabled {
        // Nothing to sync with. Put the flag back so the next launch retries
        // rather than believing this change was already delivered.
        app.cloud.mark_dirty();
        return;
    }

    let local = app.store.lock().unwrap().data.clone();
    let settings_now = app.settings.lock().unwrap().clone();
    let mut t = app.tokens.lock().unwrap().clone();

    // Run the cycle here rather than on a spawned thread: `store` is a plain
    // mutex, not an Arc, so it cannot move into a thread. Blocking the quit for
    // the length of one request is the right trade — the alternative is losing
    // the edit. The HTTP layer has its own timeouts, so this cannot hang
    // indefinitely.
    let result = {
        let mut merged_out: Option<(store::Data, Settings)> = None;
        let mut dev = app.device.lock().unwrap();
        let out = cloudsync::sync_once(
            &mut t,
            &cloudsync::Local {
                data: local,
                settings: settings_now,
            },
            &mut dev,
            &mut |merged: &store::Data, merged_settings: &Settings| {
                merged_out = Some((merged.clone(), merged_settings.clone()));
            },
        );
        if let Some((merged, merged_settings)) = merged_out {
            let mut st = app.store.lock().unwrap();
            st.data = merged;
            st.save();
            drop(st);
            let mut s = app.settings.lock().unwrap();
            *s = merged_settings;
            s.save();
        }
        out
    };

    // Persist a rotated refresh token before the process dies, or the next
    // launch would sign out.
    if !t.is_empty() {
        let _ = sync::save_tokens(&t);
        *app.tokens.lock().unwrap() = t;
    }

    if result.is_err() {
        // Leave the change marked so the next launch pushes it.
        app.cloud.mark_dirty();
    }
}

/// Push the resolved theme and heritage era into the Slint palette global.
fn apply_palette(win: &PanganWindow, resolved: &str, era: &str) {
    let p = crate::ui::Palette::get(win);
    p.set_dark(resolved != "light");
    p.set_era(SharedString::from(era));
    // Elevation is theme-dependent (a grey shadow on a dark page reads as a
    // smudge, a black one on cream reads as dirt), so it follows the same
    // resolved theme the palette does.
    crate::ui::Elev::get(win).set_dark(resolved != "light");
}

/// Session dots: total = long-break interval, done = position in the cycle.
fn apply_session_dots(win: &PanganWindow, shared: &Shared, app: &AppState) {
    let interval = app.settings.lock().unwrap().long_break_interval.max(1) as i32;
    let done = (shared.snapshot().work_done as i32) % interval;
    win.set_session_total(interval);
    win.set_session_done(done);
}

fn notify(body: &str) {
    crate::platform::notify::send("Pangan", body);
}

/// Begin the block a rota entry describes.
///
/// Three things have to happen together, and each of them is easy to forget:
///
/// 1. the entry is marked taken up, so it stops being "next" — this must happen
///    before the clock starts, because the tick that follows reads the rota;
/// 2. the block's own length is applied, via `start_work_block`, which leaves
///    the configured focus duration alone;
/// 3. the entry's decree is parked in `pending_decree`, so when the session ends
///    the intention written for it in the morning is what gets recorded —
///    rather than the dialog asking again at the end of the block, or the
///    decree going missing entirely.
///
/// A refresh follows, so the row's strikethrough and the button's new label
/// appear on the same frame the countdown starts rather than one tick later.
fn start_planned_block(
    entry: &crate::state::rota::RotaEntry,
    app: &Arc<AppState>,
    shared: &Arc<Shared>,
    win: &slint::Weak<PanganWindow>,
) {
    // Refuse on a running clock *before* marking anything: a refused start that
    // has already struck the entry through would leave the rota claiming a
    // block was taken up when it was not.
    if !shared.at_fresh_work_start() {
        return;
    }
    {
        let mut st = app.store.lock().unwrap();
        if !st.open_rota_entry(entry.id) {
            return; // already taken up, or removed while the dialog was open
        }
    }
    // Park the decree. An entry without one clears any stale pending decree
    // rather than leaving a previous block's line waiting to attach itself to
    // this session.
    let written_at = chrono::Utc::now().timestamp();
    *app.pending_decree.lock().unwrap() = if entry.decree.is_empty() {
        None
    } else {
        Some((entry.decree.clone(), written_at))
    };

    shared.start_work_block(entry.minutes);
    app.cloud.mark_dirty();
    if let Some(w) = win.upgrade() {
        apply_snapshot(&w, shared);
        refresh_screens(&w, app);
    }
}

/// Handle an action. `notify_msg` (if any) is shown from the event-loop thread.
fn run_action(
    action: TrayAction,
    shared: &Arc<Shared>,
    app: &Arc<AppState>,
    win: &slint::Weak<PanganWindow>,
) {
    let notify_msg: Option<String> = match action {
        TrayAction::Toggle => {
            shared.toggle();
            None
        }
        TrayAction::Skip => shared.skip(),
        TrayAction::Reset => {
            shared.reset();
            // A reset that follows a started block rewrites the clock but not
            // the plan: without this the rota would keep claiming the block was
            // taken up and never learn it was abandoned, and the day's record
            // would read as if it were still running. Said here rather than at
            // midnight, when the user no longer remembers why.
            //
            // A finish (`Toggle`/`Skip` landing a session) is a stronger fact
            // than the reset that may follow it, so `abandon_open` leaves a
            // claimed entry alone.
            let abandoned = {
                let mut st = app.store.lock().unwrap();
                st.abandon_open_rota_blocks()
            };
            if abandoned > 0 {
                app.cloud.mark_dirty();
            }
            None
        }
        TrayAction::Quit => {
            // Last chance to deliver anything still unsynced.
            flush_on_quit(app);
            shared.quit.store(true, Ordering::Relaxed);
            let _ = slint::quit_event_loop();
            return;
        }
        TrayAction::ToggleWindow => {
            let w = win.upgrade();
            if let Some(w) = w {
                let visible = w.window().is_visible();
                let _ = if visible { w.hide() } else { w.show() };
            }
            None
        }
    };

    let shared = Arc::clone(shared);
    let app = Arc::clone(app);
    let win = win.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(win) = win.upgrade() {
            apply_snapshot(&win, &shared);
            refresh_screens(&win, &app);
        }
        if let Some(text) = notify_msg {
            notify(&text);
        }
    });
}

/// The day the calendar screen is showing: the user's selection, or today
/// before anything has been selected.
///
/// Every part of the screen (the date card, the day-name, the feast and the
/// events) is built from this, so picking a day in the grid moves the whole
/// screen and not just the task list underneath it.
fn selected_day(app: &AppState) -> (i32, u32, u32) {
    let key = app.selected_day.lock().unwrap().clone();
    parse_day_key(&key).unwrap_or_else(calendar::today)
}

/// Build and push the Persian-calendar card: the Jalali date, the weekday, the
/// Sasanian day-name, the feast (if that day is one) and the day's events.
///
/// Rebuilt only when the day being shown changes, so the 300 ms UI tick does
/// not re-serialise a few hundred event rows twice a second.
fn refresh_calendar(win: &PanganWindow, app: &AppState, day: (i32, u32, u32)) {
    let (jy, jm, jd) = day;
    let lang = app.settings.lock().unwrap().lang.clone();
    let en = lang == "en";
    {
        let mut last = app.calendar_day.lock().unwrap();
        if *last == day {
            return;
        }
        *last = day;
    }

    let month_name = if en {
        calendar::MONTHS_EN[(jm as usize).clamp(1, 12) - 1]
    } else {
        calendar::MONTHS[(jm as usize).clamp(1, 12) - 1]
    };
    let weekday = if en {
        calendar::weekday_name_en(jy, jm, jd)
    } else {
        calendar::weekday_name(jy, jm, jd)
    };
    let day = calendar::day_name(jm, jd);

    // The date line. The year is the era count — years since the founding of
    // the Iranian kingdom — not the Hijri Solar year, which is the reckoning
    // this calendar deliberately replaces. The Jalali year is still what drives
    // all the arithmetic; only the label changes.
    let era = calendar::era_year(jy);
    let date_line = if en {
        format!("{jd} {month_name} {era}")
    } else {
        format!(
            "{} {} {}",
            calendar::fa_num(jd as i64),
            month_name,
            calendar::fa_num(era as i64)
        )
    };
    win.set_cal_date(SharedString::from(date_line));
    win.set_cal_weekday(SharedString::from(weekday));

    // The Sasanian day-name. In English we keep the Persian name and gloss it,
    // because the name *is* the heritage; transliterating it would lose it.
    let day_name = if en {
        format!("{} ({})", day.name_en, day.name)
    } else {
        day.name.to_string()
    };
    win.set_cal_day_name(SharedString::from(day_name));

    win.set_cal_day_index(SharedString::from(if en {
        format!("day {} of the month", jd)
    } else {
        format!("روز {} ماه", calendar::ordinal_fa(jd))
    }));

    // The festival for today, with its one-line gloss. This covers the fixed
    // festivals (Nowruz, the six Gahambars, the fire and water feasts) as well
    // as the twelve name-day ones, so a day that is a festival at all says so.
    // The feast card follows the language too. The note is only available in
    // Persian, so in English the name is transliterated and the note is left
    // off rather than printing Persian prose inside an English screen.
    let (feast_name, feast_note) = if en {
        (calendar::feast_name_en(jm, jd).unwrap_or(""), "")
    } else {
        match calendar::feast_on(jm, jd) {
            Some((name, note)) => (name, note),
            None => ("", ""),
        }
    };
    win.set_cal_feast(SharedString::from(feast_name));
    win.set_cal_feast_note(SharedString::from(feast_note));

    let month_len = calendar::month_len(jy, jm);
    win.set_cal_month_progress(SharedString::from(if en {
        format!("day {jd} of {month_len}")
    } else {
        format!(
            "روز {} از {}",
            calendar::fa_num(jd as i64),
            calendar::fa_num(month_len as i64)
        )
    }));

    // ── Iranian heritage stream: leads the screen ──
    let (evs, births, deaths, occasions) = app.events.lock().unwrap().iran(jm, jd);
    let to_ui = |rows: Vec<events::Row>, label: &'static str| -> Vec<crate::ui::CalEvent> {
        rows.into_iter()
            .map(|r| crate::ui::CalEvent {
                section: SharedString::from(label),
                year: SharedString::from(r.year),
                text: SharedString::from(r.text),
            })
            .collect()
    };
    let (l_ev, l_birth, l_death, l_occ) = if en {
        ("Events", "Born", "Died", "Occasions")
    } else {
        ("رخداد", "زادروز", "درگذشت", "مناسبت")
    };
    let mut iran_rows = to_ui(evs, l_ev);
    iran_rows.extend(to_ui(occasions, l_occ));
    iran_rows.extend(to_ui(births, l_birth));
    iran_rows.extend(to_ui(deaths, l_death));
    win.set_cal_events(ModelRc::new(VecModel::from(iran_rows))); // ── World stream: keyed by the Gregorian day, shown after Iran's ──
    let (_, gm, gd) = calendar::to_gregorian(jy, jm, jd);
    let world = app.events.lock().unwrap().world(&lang, gm, gd);
    let world_label = if en { "World" } else { "جهان" };
    win.set_cal_world(ModelRc::new(VecModel::from(to_ui(world, world_label))));
}

/// Parse a `YYYY-MM-DD` Jalali key back into `(year, month, day)`.
fn parse_day_key(key: &str) -> Option<(i32, u32, u32)> {
    let mut it = key.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, m, d))
}

/// Build the month grid and the selected day's task list.
///
/// This is the calendar screen's whole state: which month is on screen, which
/// day is selected, and what work sits on that day. It is rebuilt whenever any
/// of those change, which is cheap — a month is at most 42 cells.
fn refresh_day_view(win: &PanganWindow, app: &AppState) {
    let (ty, tm, td) = calendar::today();
    let en = app.settings.lock().unwrap().lang == "en";
    let today_key = format!("{ty:04}-{tm:02}-{td:02}");

    // Default the selection to today the first time the screen is built.
    let selected_key = {
        let mut sel = app.selected_day.lock().unwrap();
        if sel.is_empty() {
            *sel = today_key.clone();
        }
        sel.clone()
    };
    let (sy, sm, sd) = parse_day_key(&selected_key).unwrap_or((ty, tm, td));

    // The grid follows the selected day unless the user has paged away.
    let (gy, gm) = {
        let shown = *app.shown_month.lock().unwrap();
        if shown == (0, 0) {
            (sy, sm)
        } else {
            shown
        }
    };

    // ── the grid ──
    let counts: std::collections::HashMap<String, u32> = {
        let st = app.store.lock().unwrap();
        st.open_counts_by_date().into_iter().collect()
    };
    let cells: Vec<crate::ui::DayCell> = calendar::month_grid(gy, gm, (ty, tm, td))
        .into_iter()
        .map(|c| {
            let has = counts.get(&c.key).copied().unwrap_or(0);
            crate::ui::DayCell {
                day: c.day as i32,
                label: if c.day == 0 {
                    SharedString::new()
                } else if en {
                    SharedString::from(c.day.to_string())
                } else {
                    SharedString::from(calendar::fa_num(c.day as i64))
                },
                key: SharedString::from(c.key.clone()),
                is_today: c.is_today,
                selected: c.key == selected_key,
                has_tasks: has > 0,
                open_count: if has > 0 {
                    SharedString::from(if en {
                        has.to_string()
                    } else {
                        calendar::fa_num(has as i64)
                    })
                } else {
                    SharedString::new()
                },
            }
        })
        .collect();
    // Slint's GridLayout has no explicit column count, so the month is handed
    // over as weeks of seven rather than one flat list.
    let rows: Vec<crate::ui::DayRow> = cells
        .chunks(7)
        .map(|w| crate::ui::DayRow {
            cells: ModelRc::new(VecModel::from(w.to_vec())),
        })
        .collect();
    win.set_month_rows(ModelRc::new(VecModel::from(rows)));

    // ── the month title ──
    let month_name = if en {
        calendar::MONTHS_EN[(gm as usize).clamp(1, 12) - 1]
    } else {
        calendar::MONTHS[(gm as usize).clamp(1, 12) - 1]
    };
    let era = calendar::era_year(gy);
    win.set_month_title(SharedString::from(if en {
        format!("{month_name} {era}")
    } else {
        format!("{} {}", month_name, calendar::fa_num(era as i64))
    }));

    // ── the selected day's heading ──
    let sel_month = if en {
        calendar::MONTHS_EN[(sm as usize).clamp(1, 12) - 1]
    } else {
        calendar::MONTHS[(sm as usize).clamp(1, 12) - 1]
    };
    let weekday = if en {
        calendar::weekday_name_en(sy, sm, sd)
    } else {
        calendar::weekday_name(sy, sm, sd)
    };
    let is_today = selected_key == today_key;
    win.set_selected_is_today(is_today);
    win.set_cal_selected(SharedString::from(selected_key.clone()));

    win.set_selected_day_title(SharedString::from(if en {
        format!("{weekday}, {sd} {sel_month}")
    } else {
        format!("{}، {} {}", weekday, calendar::fa_num(sd as i64), sel_month)
    }));
    win.set_selected_day_sub(SharedString::from(if en {
        if is_today {
            "today".to_string()
        } else {
            String::new()
        }
    } else if is_today {
        "امروز".to_string()
    } else {
        String::new()
    }));

    // ── the day's tasks ──
    let tasks: Vec<crate::ui::TaskItem> = {
        let st = app.store.lock().unwrap();
        st.tasks_on(&selected_key)
            .into_iter()
            .map(|t| crate::ui::TaskItem {
                id: t.id as i32,
                title: SharedString::from(t.title),
                time: SharedString::from(t.time),
                done: t.done,
            })
            .collect()
    };
    win.set_day_tasks(ModelRc::new(VecModel::from(tasks)));

    // ── the timer screen's day navigator ──
    // The same selection drives both screens, so stepping the day on the timer
    // and picking a day in the month grid always agree.
    let open_here = app
        .store
        .lock()
        .unwrap()
        .tasks_on(&selected_key)
        .iter()
        .filter(|t| !t.done)
        .count();
    // The headline is the full date in the era reckoning — "۷ آبان ۲۵۸۵" —
    // with the weekday kept in the subline. The date itself leads, marked
    // "(امروز)" when it is today, rather than the word "today" standing in
    // for a date.
    let sel_era = calendar::era_year(sy);
    let date_text = if en {
        format!("{sd} {sel_month} {sel_era}")
    } else {
        format!(
            "{} {} {}",
            calendar::fa_num(sd as i64),
            sel_month,
            calendar::fa_num(sel_era as i64)
        )
    };
    win.set_day_headline(SharedString::from(if is_today {
        if en {
            format!("{date_text} (today)")
        } else {
            format!("{date_text} (امروز)")
        }
    } else {
        date_text
    }));

    // Every part of this line has to follow the language. The weekday already
    // did, but the feast and the old-calendar day-name were hardcoded to the
    // Persian tables, so English mode still produced a Persian subline.
    let day_name = calendar::day_name(sm, sd);
    let feast = if en {
        calendar::feast_name_en(sm, sd).unwrap_or("")
    } else {
        calendar::feast_on(sm, sd).map(|f| f.0).unwrap_or("")
    };
    let mut bits: Vec<String> = Vec::new();
    bits.push(weekday.to_string());
    if !feast.is_empty() {
        bits.push(feast.to_string());
    }
    bits.push(if en {
        day_name.name_en.to_string()
    } else {
        day_name.name.to_string()
    });
    bits.push(if en {
        if open_here == 1 {
            "1 open task".to_string()
        } else {
            format!("{open_here} open tasks")
        }
    } else {
        format!("{} کار باز", calendar::fa_num(open_here as i64))
    });
    win.set_day_subline(SharedString::from(bits.join(" · ")));
    win.set_day_is_today(is_today);

    // The card below the grid (date, day-name, feast, events) belongs to the
    // same day as the grid and the task list. Without this the screen said
    // "7 Aban" while the grid had 12 Aban selected.
    refresh_calendar(win, app, (sy, sm, sd));
}

/// Push the decree's static labels and the strip's current text.
///
/// Kept separate from `push_inscriptions` because the two are read at different
/// moments: the archive is only worth rebuilding when the archive screen is
/// open, but the strip under the clock is on the main screen and has to be right
/// on every tick.
///
/// The strip's text comes from `AppState`, not from the store. A decree exists
/// in the store only after its session has ended, so during the block the only
/// copy of the intention is the pending one — and looking it up by session
/// timestamp would find nothing for the precise reason that the session is
/// still running.
fn push_decree_state(win: &PanganWindow, app: &AppState, st: &Store) {
    let en = app.settings.lock().unwrap().lang == "en";

    win.set_decree_prompt(SharedString::from(if en {
        "What is this session for?"
    } else {
        "این نشست برای چه کاری است؟"
    }));
    win.set_decree_placeholder(SharedString::from(if en {
        "One task, in one sentence…"
    } else {
        "یک کار، در یک جمله…"
    }));
    win.set_decree_limit(journal::decree::MAX_CHARS as i32);

    // The strip: the pending intention if a block is running under one, else
    // the most recent decree so the clock does not go blank the instant a
    // session ends and its inscription is being written.
    let pending = app
        .pending_decree
        .lock()
        .unwrap()
        .as_ref()
        .map(|(text, _)| text.clone());
    let text = pending.unwrap_or_else(|| {
        st.decrees_newest_first()
            .first()
            .map(|d| d.text.clone())
            .unwrap_or_default()
    });
    win.set_current_decree(SharedString::from(text));
}

/// Push the day's rota into the window: the rows, the Start Next label, and how
/// much of the plan is still ahead.
///
/// Rebuilt from the store on every tick, like every other list here, for the
/// same reason: the store is the truth and a window-side copy is one more thing
/// that can disagree with it. The list is a handful of rows, so the cost is a
/// few allocations every 300 ms.
///
/// The day pushed is *today*, not the day the calendar screen happens to be
/// showing. A rota is about the hours you are actually in; planning for a day
/// you are browsing is a feature, but beginning a block planned for last Tuesday
/// is not, and conflating the two would make Start Next a way to start the wrong
/// work.
fn push_rota(win: &PanganWindow, app: &AppState, st: &Store) {
    let en = app.settings.lock().unwrap().lang == "en";
    let today = chrono::Local::now().date_naive();
    let today_key = today.format("%Y-%m-%d").to_string();

    let entries = st.rota_for_day(&today_key);
    let total = entries.len();
    let done = entries.iter().filter(|e| !e.is_pending()).count();
    let pending = total - done;

    let mut items: Vec<crate::ui::RotaItem> = Vec::with_capacity(total);
    // The row height is decided here because the Slint side cannot sum it: a
    // row with a decree is 54px and one without is 40px, and a Flickable's
    // content-height must not be derived from its own children.
    let mut list_height = 0.0f32;
    let mut first_pending_seen = false;
    for (i, e) in entries.iter().enumerate() {
        let is_pending = e.is_pending();
        // "next" marks exactly one row: the first still waiting. Marking every
        // pending row would make the head of the queue impossible to find.
        let state = if !is_pending {
            "started"
        } else if !first_pending_seen {
            first_pending_seen = true;
            "next"
        } else {
            "waiting"
        };
        let number = if en {
            (i + 1).to_string()
        } else {
            calendar::fa_num((i + 1) as i64)
        };
        let length = if en {
            format!("{}m", e.minutes)
        } else {
            format!("{}د", calendar::fa_num(e.minutes as i64))
        };
        // What became of this block, as the row draws it. Resolved through
        // `outcome::effective` rather than read off the field, so a stale entry
        // from an earlier day reads as missed here for the same reason it does
        // in the tally — one rule, two readers.
        let outcome = crate::state::outcome::effective(e, &today_key);
        list_height += if e.decree.is_empty() { 40.0 } else { 54.0 };
        items.push(crate::ui::RotaItem {
            id: e.id as i32,
            number: SharedString::from(number),
            label: SharedString::from(e.label.clone()),
            length: SharedString::from(length),
            state: SharedString::from(state),
            decree: SharedString::from(e.decree.clone()),
            outcome: SharedString::from(outcome.as_str()),
        });
    }
    win.set_rota(ModelRc::new(VecModel::from(items)));
    win.set_rota_list_height(list_height);

    let next = entries.iter().find(|e| e.is_pending());
    win.set_rota_next_enabled(next.is_some());
    // The button says what it will start, not "start next": the press should be
    // informed about which block it is about to spend half an hour on.
    win.set_rota_next_label(SharedString::from(match next {
        Some(e) => e.label.clone(),
        None => String::new(),
    }));

    // What the day's plans came to. Said as a sentence with the parts named,
    // not as a score: "۴ انجام · ۱ رهاشده · ۲ مانده" reads back what happened,
    // where "3/7" would only say how much of it did not. Rendered from the same
    // tally the report reads, so the two screens cannot disagree about the day.
    //
    // A day with no plans says nothing at all. An empty scoreboard reads as a
    // failure the user did not commit — they simply had not written the day yet.
    let tally = st.rota_day_tally(&today_key, &today_key);
    win.set_rota_tally_visible(!tally.is_empty());
    win.set_rota_tally_label(SharedString::from(if tally.is_empty() {
        String::new()
    } else {
        let mut parts: Vec<String> = Vec::with_capacity(3);
        if tally.done > 0 {
            parts.push(if en {
                format!("{} done", tally.done)
            } else {
                format!("{} انجام", calendar::fa_num(tally.done as i64))
            });
        }
        if tally.abandoned > 0 {
            parts.push(if en {
                format!("{} abandoned", tally.abandoned)
            } else {
                format!("{} رهاشده", calendar::fa_num(tally.abandoned as i64))
            });
        }
        if tally.pending > 0 {
            parts.push(if en {
                format!("{} to go", tally.pending)
            } else {
                format!("{} مانده", calendar::fa_num(tally.pending as i64))
            });
        }
        if tally.missed > 0 {
            parts.push(if en {
                format!("{} missed", tally.missed)
            } else {
                format!("{} از دست رفته", calendar::fa_num(tally.missed as i64))
            });
        }
        // Only the parts that happened. A zero is not information here, and a
        // row of zeros is what makes a summary stop being read.
        parts.join(" · ")
    }));

    // "2 of 5 left". Built here rather than in Slint because this is where the
    // Persian digits are, and because the app's strings cannot interpolate.
    win.set_rota_remaining(SharedString::from(if total == 0 {
        String::new()
    } else if en {
        format!("{pending} of {total} left")
    } else {
        format!(
            "{} از {} مانده",
            calendar::fa_num(pending as i64),
            calendar::fa_num(total as i64)
        )
    }));

    // Leftovers from earlier days. Counted and offered, never merged in: a rota
    // that silently refilled itself would be a list you can never finish.
    let carry = st.rota_carry_over(&today_key);
    win.set_rota_carry_count(carry.len() as i32);
    win.set_rota_carry_label(SharedString::from(if carry.is_empty() {
        String::new()
    } else if en {
        format!(
            "Bring {} unfinished block{} to today",
            carry.len(),
            if carry.len() == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "{} بلوک ناتمام از روزهای پیش را به امروز بیاور",
            calendar::fa_num(carry.len() as i64)
        )
    }));
}

/// Push the saved plans into the window: the rows, and whether today has
/// anything worth saving as a new one.
///
/// Rebuilt from the store on every tick like every other list here, for the
/// same reason: the store is the truth, and a window-side copy is one more
/// thing that can disagree with it.
///
/// The "save today" affordance is hidden when today's rota holds nothing. An
/// empty plan saves nothing, and a button that is present but always refuses
/// teaches the user that the feature does not work rather than that today is
/// empty.
fn push_plans(win: &PanganWindow, app: &AppState, st: &Store) {
    let en = app.settings.lock().unwrap().lang == "en";
    let plans = st.live_plans();
    let mut items: Vec<crate::ui::PlanItem> = Vec::with_capacity(plans.len());
    let mut list_height = 0.0f32;
    for p in &plans {
        list_height += 42.0;
        // "4 blocks · 135 min". Built here because this is where the Persian
        // digits are, and Slint cannot interpolate a count into a string.
        let summary = if en {
            format!(
                "{} block{} · {} min",
                p.blocks.len(),
                if p.blocks.len() == 1 { "" } else { "s" },
                p.total_minutes()
            )
        } else {
            format!(
                "{} بلوک · {} دقیقه",
                calendar::fa_num(p.blocks.len() as i64),
                calendar::fa_num(p.total_minutes() as i64)
            )
        };
        items.push(crate::ui::PlanItem {
            id: p.id as i32,
            name: SharedString::from(p.name.clone()),
            summary: SharedString::from(summary),
        });
    }
    win.set_plans(ModelRc::new(VecModel::from(items)));
    win.set_plans_list_height(list_height);

    let today = chrono::Local::now().date_naive();
    let today_key = today.format("%Y-%m-%d").to_string();
    let has_today = !st.rota_for_day(&today_key).is_empty();
    win.set_plan_save_visible(has_today);
    if !has_today {
        // Nothing to save, so the name field cannot be half-filled either.
        win.set_plan_can_save(false);
    }
}

/// Push the inscription archive into the window, honouring the search box.
///
/// The archive is rebuilt from the store on every tick rather than being edited
/// in place, for the same reason every other list in this app is: the store is
/// the truth, and a second copy in the window would be one more thing that can
/// disagree with it. The list is small (one line per focus session), so the
/// cost is a sort and a dozen allocations every 300 ms.
///
/// Rows come from *both* halves of the record, which is the point of keeping
/// them: every inscription gets a row, and so does every decree whose session
/// produced no inscription. That second kind is the most useful row in the list
/// — a block that was spoken for and did not deliver — and it would be invisible
/// if the archive were built from inscriptions alone.
/// The user-facing word for a verdict kind.
///
/// Lives here rather than in `strings.slint` because the archive row and the
/// reckoning tally must never disagree about what a seal says: the row draws
/// one word per session, the tally counts five. Two copies of these words in
/// two files is two chances to rename one of them.
fn verdict_word(kind: journal::VerdictKind, en: bool) -> &'static str {
    match (kind, en) {
        (journal::VerdictKind::Kept, false) => "انجام شد",
        (journal::VerdictKind::Kept, true) => "Kept",
        (journal::VerdictKind::Partial, false) => "نیمه",
        (journal::VerdictKind::Partial, true) => "Partial",
        (journal::VerdictKind::Broken, false) => "نشد",
        (journal::VerdictKind::Broken, true) => "Not done",
        (journal::VerdictKind::Extended, false) => "بیشتر",
        (journal::VerdictKind::Extended, true) => "Ran over",
        (journal::VerdictKind::Short, false) => "کوتاه",
        (journal::VerdictKind::Short, true) => "Cut short",
    }
}

/// The storage key for a verdict kind, as the UI sends it back.
///
/// Searched over [`journal::VerdictKind::ALL`] rather than matched arm by arm:
/// a `match` here would compile happily after a sixth kind is added and would
/// silently refuse it at runtime, while an `ALL` scan keeps the key-to-kind
/// mapping and the definition of the vocabulary in one list.
fn verdict_from_key(key: &str) -> Option<journal::VerdictKind> {
    journal::VerdictKind::ALL
        .iter()
        .copied()
        .find(|k| k.as_str() == key)
}

fn push_inscriptions(win: &PanganWindow, app: &AppState, st: &Store) {
    let en = app.settings.lock().unwrap().lang == "en";
    let query = app.ins_query.lock().unwrap().clone();
    let live = st.inscriptions_newest_first();
    let hits: Vec<journal::Inscription> = journal::search(&live, &query)
        .into_iter()
        .cloned()
        .collect();

    // A search filters the written lines only. Decree-only rows have no text to
    // match, so including them in a search would put a row in the results for a
    // reason the user cannot see.
    let decree_only: Vec<journal::Decree> = if query.trim().is_empty() {
        st.decrees_newest_first()
            .into_iter()
            .filter(|d| st.inscription_for_session(d.session_at).is_none())
            .collect()
    } else {
        Vec::new()
    };

    let mut items: Vec<InscriptionItem> = Vec::with_capacity(hits.len() + decree_only.len());
    // One pass over a merged, newest-first key list so a decree-only row lands
    // in its proper place in time rather than being appended at the end.
    let mut keys: Vec<i64> = hits.iter().map(|i| i.session_at).collect();
    keys.extend(decree_only.iter().map(|d| d.session_at));
    keys.sort_unstable_by(|a, b| b.cmp(a));
    keys.dedup();

    let by_session: std::collections::HashMap<i64, &journal::Inscription> =
        hits.iter().map(|i| (i.session_at, i)).collect();
    let decree_by_session: std::collections::HashMap<i64, &journal::Decree> =
        decree_only.iter().map(|d| (d.session_at, d)).collect();

    let mut last_day = String::new();
    for session_at in keys {
        let text = by_session
            .get(&session_at)
            .map(|i| i.text.clone())
            .unwrap_or_default();
        // Both halves of the session's record, held as references so the
        // verdict rule can see the shape it falls back to without cloning it.
        let ins_ref = by_session.get(&session_at).copied();
        let dec_ref = st.decree_for_session(session_at);
        let (id, decree) = match (ins_ref, decree_by_session.get(&session_at)) {
            (Some(ins), _) => (
                ins.id.clone(),
                dec_ref.map(|d| d.text.clone()).unwrap_or_default(),
            ),
            // A decree with no inscription is addressed by its own id, and
            // the row shows nothing in the `text` field: there was no
            // written outcome, and inventing one would be a lie.
            (None, Some(dec)) => (dec.id.clone(), dec.text.clone()),
            (None, None) => continue,
        };

        let day_key = journal::day_key(session_at);
        // The heading only appears on the first row of each day, so the archive
        // reads as day groups instead of repeating the date on every line.
        let day = if day_key == last_day {
            String::new()
        } else {
            last_day = day_key.clone();
            jalali_day_heading(&day_key, en)
        };

        // What this block was, as the user said it — or, if they have not said,
        // as the shape implies. A declared verdict fills the row's seal; an
        // inferred one leaves the five open and says "inferred" beside them.
        // The distinction is carried, not flattened: a guess drawn like a fact
        // is a guess the user will trust.
        let said = st.verdict_for(session_at).map(|v| v.kind);
        let (verdict_label, verdict_declared, verdict_kind) =
            match journal::verdict::effective(said, dec_ref, ins_ref) {
                Some(e) => (
                    verdict_word(e.kind, en).to_string(),
                    e.declared,
                    e.kind.as_str().to_string(),
                ),
                None => (String::new(), false, String::new()),
            };

        items.push(InscriptionItem {
            id: SharedString::from(id),
            text: SharedString::from(text),
            time: SharedString::from(local_time(session_at)),
            day: SharedString::from(day),
            session: session_at as i32,
            decree: SharedString::from(decree),
            verdict: SharedString::from(verdict_label),
            verdict_declared,
            verdict_kind: SharedString::from(verdict_kind),
            // Which planned block this session answered. Empty for a block
            // started from the plain timer, which had no plan to answer — the
            // row then simply omits the line rather than saying "unplanned".
            planned: SharedString::from(
                st.rota_entry_for_session(session_at)
                    .map(|e| e.label.clone())
                    .unwrap_or_default(),
            ),
        });
    }

    win.set_inscriptions(ModelRc::new(VecModel::from(items)));
    win.set_inscription_text(SharedString::from(String::new()));
    win.set_inscription_prompt(SharedString::from(if en {
        "What did you accomplish this session?"
    } else {
        "در این نشست چه کردید؟"
    }));
    win.set_inscription_limit(journal::MAX_CHARS as i32);

    // How many finished blocks were actually spoken for. The one number that
    // makes the pair worth keeping: without an intention, an inscription says
    // what happened but not whether it was what you meant to happen.
    let decreed = st.decreed_session_count();
    win.set_archive_decree_count(SharedString::from(if decreed == 0 {
        String::new()
    } else if en {
        format!("{decreed} of your sessions began with a stated intention")
    } else {
        format!(
            "{} نشست با فرمانی روشن آغاز شد",
            calendar::fa_num(decreed as i64)
        )
    }));

    win.set_archive_empty(SharedString::from(if query.trim().is_empty() {
        if en {
            "Nothing written yet. It begins when your first focus session ends."
        } else {
            "هنوز چیزی نوشته نشده است. با پایان نخستین نشست کار، اینجا آغاز می‌شود."
        }
    } else if en {
        "Nothing matches that."
    } else {
        "چیزی با این واژه پیدا نشد."
    }));
}

/// `HH:MM` for a Unix timestamp, in local time.
fn local_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|u| u.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".to_string())
}

/// A Jalali heading for a `YYYY-MM-DD` local date, e.g. "۱۴ آبان ۱۴۰۵".
///
/// The store keys days the Gregorian way (the same key every other screen uses),
/// but the archive is read by a Persian user, so the heading is Jalali. A
/// malformed key falls back to the raw string rather than an empty heading that
/// would make the day invisible.
fn jalali_day_heading(day_key: &str, en: bool) -> String {
    let parts: Vec<&str> = day_key.split('-').collect();
    if parts.len() != 3 {
        return day_key.to_string();
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        parts[0].parse::<i32>(),
        parts[1].parse::<u32>(),
        parts[2].parse::<u32>(),
    ) else {
        return day_key.to_string();
    };
    let (jy, jm, jd) = calendar::from_gregorian(y, m, d);
    let month = if en {
        calendar::MONTHS_EN
            .get(jm.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("")
    } else {
        calendar::MONTHS
            .get(jm.saturating_sub(1) as usize)
            .copied()
            .unwrap_or("")
    };
    if en {
        format!("{jd} {month} {jy}")
    } else {
        format!(
            "{} {} {}",
            calendar::fa_num(jd as i64),
            month,
            calendar::fa_num(jy as i64)
        )
    }
}

/// Push the read-back of the written record: what the span promised, what it
/// delivered, which promises did not land, and the words it was actually about.
///
/// Reads the same `Range` the report above uses (`stats::bounds`), so the
/// reckoning can never tell a different story about where a week starts than
/// the chart sitting directly over it.
///
/// Both lists are rebuilt from scratch on every tick, like everything else on
/// this screen. The archive is a handful of rows and the cost is a few
/// allocations every 300 ms — far cheaper than the alternative, which is a
/// window-side copy that can disagree with the store it came from.
fn push_reckoning(
    win: &PanganWindow,
    st: &Store,
    period: stats::Period,
    offset: i32,
    today: chrono::NaiveDate,
    earliest: Option<chrono::NaiveDate>,
    en: bool,
) {
    let range = stats::bounds(period, offset, today, earliest);
    let session_times: Vec<i64> = st.data.sessions.iter().map(|s| s.at).collect();
    let r = journal::reckoning::build(
        &st.data.decrees,
        &st.data.inscriptions,
        &session_times,
        range,
    );

    win.set_reck_spoken(r.spoken as i32);
    win.set_reck_answered(r.answered as i32);
    win.set_reck_written(r.written as i32);
    win.set_reck_unanswered(r.unanswered() as i32);
    win.set_reck_unspoken(r.unspoken() as i32);
    win.set_reck_unfulfilled_total(r.unfulfilled_total as i32);
    // The card is hidden entirely when the span contains no writing at all.
    // An empty reckoning is not "zero of zero kept" — it is a period the
    // feature does not apply to, and showing it would be a card of dashes.
    win.set_reck_empty(r.is_empty());

    let mut unfulfilled: Vec<crate::ui::UnfulfilledItem> = Vec::with_capacity(r.unfulfilled.len());
    for u in &r.unfulfilled {
        // Only the day, not the time: the question a missed promise raises is
        // "when was I going to do this", and the hour of a block that is over
        // does not answer it.
        let day = chrono::DateTime::from_timestamp(u.session_at, 0)
            .map(|t| stats::jalali_label(t.with_timezone(&chrono::Local).date_naive(), en, false))
            .unwrap_or_default();
        unfulfilled.push(crate::ui::UnfulfilledItem {
            text: SharedString::from(u.text.clone()),
            day: SharedString::from(day),
        });
    }
    win.set_reck_unfulfilled(ModelRc::new(VecModel::from(unfulfilled)));

    // Bars are relative to the strongest theme, so the list reads as a shape.
    // The peak is taken from the list rather than recomputed, because
    // `recurring` is already sorted and already capped to what is shown.
    let peak = r.recurring.first().map(|w| w.count).unwrap_or(1).max(1);
    // Persian digits for the count column, Latin ones for English — the same
    // rule every other number on this screen follows.
    let num = |n: i64| -> String {
        if en {
            n.to_string()
        } else {
            calendar::fa_num(n)
        }
    };
    let themes: Vec<crate::ui::ThemeItem> = r
        .recurring
        .iter()
        .map(|w| crate::ui::ThemeItem {
            word: SharedString::from(w.word.clone()),
            count: SharedString::from(num(w.count as i64)),
            weight: (w.count as f32 / peak as f32).clamp(0.0, 1.0),
        })
        .collect();
    win.set_reck_themes(ModelRc::new(VecModel::from(themes)));

    // The verdict line. One sentence, and deliberately not a score: the point
    // of the reckoning is to show the shape of the span, not to grade it. A
    // single number here would be read as a grade and would make the honest
    // weeks look worse than the busy ones.
    win.set_reck_summary(SharedString::from(if en {
        summary_en(&r)
    } else {
        summary_fa(&r)
    }));

    // ── how the span's blocks were judged ────────────────────────────────
    //
    // A separate count from the decree/inscription numbers above, not a
    // replacement for them. Those measure whether words exist; this measures
    // what the user says the blocks were worth, and a block can have its words
    // and still have been abandoned. Showing only the first would let a week
    // of write-throughs read as a week of delivery.
    let t = journal::verdict::tally(
        &st.data.decrees,
        &st.data.inscriptions,
        &st.data.verdicts,
        range,
    );
    win.set_verdict_kept(t.kept as i32);
    win.set_verdict_partial(t.partial as i32);
    win.set_verdict_broken(t.broken as i32);
    win.set_verdict_extended(t.extended as i32);
    win.set_verdict_short(t.short as i32);
    win.set_verdict_declared(t.declared as i32);
    win.set_verdict_total(t.total() as i32);
    win.set_verdict_met(t.met() as i32);
    win.set_verdict_empty(t.is_empty());
}

fn summary_en(r: &journal::reckoning::WeeklyReckoning) -> String {
    if r.spoken == 0 {
        return String::new();
    }
    let mut s = format!("{} of {} decrees were answered", r.answered, r.spoken);
    if r.unspoken() > 0 {
        s.push_str(&format!(
            "; {} block{} produced words nobody planned",
            r.unspoken(),
            if r.unspoken() == 1 { "" } else { "s" }
        ));
    }
    s.push('.');
    s
}

fn summary_fa(r: &journal::reckoning::WeeklyReckoning) -> String {
    if r.spoken == 0 {
        return String::new();
    }
    let mut s = format!(
        "از {} فرمان، {} پاسخ گرفت",
        calendar::fa_num(r.spoken as i64),
        calendar::fa_num(r.answered as i64)
    );
    if r.unspoken() > 0 {
        s.push_str(&format!(
            "؛ {} بلوک بی‌فرمان نوشته شد",
            calendar::fa_num(r.unspoken() as i64)
        ));
    }
    s.push('؛');
    s
}

/// Rebuild every screen from the store. The one entry point the tick calls.
///
/// The Stats screen is driven entirely from here: the period and offset live in
/// `AppState`, this builds the report for that span and pushes every number the
/// screen shows. Nothing on that screen computes its own figures, so the chart,
/// the cards and the narrative can never disagree with each other.
fn refresh_screens(win: &PanganWindow, app: &AppState) {
    let st = app.store.lock().unwrap();
    push_inscriptions(win, app, &st);
    push_decree_state(win, app, &st);
    push_rota(win, app, &st);
    push_plans(win, app, &st);
    let en = app.settings.lock().unwrap().lang == "en";
    let period = *app.report_period.lock().unwrap();
    let offset = *app.report_offset.lock().unwrap();

    let today = chrono::Local::now().date_naive();
    let earliest = st
        .data
        .sessions
        .iter()
        .filter_map(|s| {
            chrono::DateTime::from_timestamp(s.at, 0)
                .map(|d| d.with_timezone(&chrono::Local).date_naive())
        })
        .min();
    let r = stats::report_at(&st.data.sessions, period, offset, today, earliest, en);

    win.set_report_period_index(period.index());
    win.set_report_offset(offset);
    win.set_report_title(SharedString::from(if en {
        r.title_en.clone()
    } else {
        r.title.clone()
    }));
    win.set_report_span(SharedString::from(r.span.clone()));
    win.set_report_narrative(SharedString::from(stats::narrative(&r, en)));

    win.set_report_score(r.score as i32);
    win.set_report_prev_score(r.prev_score as i32);
    win.set_report_minutes(r.minutes as i32);
    win.set_report_sessions(r.sessions as i32);
    win.set_report_avg(r.avg as i32);
    win.set_report_active_days(r.active_days as i32);
    win.set_report_days(r.days_in_period as i32);
    win.set_report_consistency((r.consistency * 100.0).round() as i32);
    win.set_report_best(r.best_day_minutes as i32);
    win.set_report_best_label(SharedString::from(if en {
        r.best_day_label_en.clone()
    } else {
        r.best_day_label.clone()
    }));
    win.set_report_streak(r.streak as i32);
    win.set_report_longest(r.longest_streak as i32);
    win.set_report_prev_minutes(r.prev_minutes as i32);
    win.set_report_delta(r.delta_pct.round() as i32);
    win.set_report_has_prev(r.has_prev);
    win.set_report_empty(r.sessions == 0);

    // The chart. Bars are normalised to the peak so a quiet week still reads as
    // a shape rather than as seven invisible stubs; an all-zero span keeps a
    // floor so the baseline is still visible.
    let peak = r
        .buckets
        .iter()
        .map(|b| b.minutes)
        .max()
        .unwrap_or(0)
        .max(1);
    let bars: Vec<f32> = r
        .buckets
        .iter()
        .map(|b| (b.minutes as f32 / peak as f32).clamp(0.0, 1.0))
        .collect();
    let labels: Vec<SharedString> = r
        .buckets
        .iter()
        .map(|b| SharedString::from(b.label.clone()))
        .collect();
    let details: Vec<SharedString> = r
        .buckets
        .iter()
        .map(|b| SharedString::from(b.detail.clone()))
        .collect();
    let current = r
        .buckets
        .iter()
        .position(|b| b.current)
        .map(|i| i as i32)
        .unwrap_or(-1);
    let future_from = r
        .buckets
        .iter()
        .position(|b| b.future)
        .map(|i| i as i32)
        .unwrap_or(r.buckets.len() as i32);

    win.set_chart_bars(ModelRc::new(VecModel::from(bars)));
    win.set_chart_labels(ModelRc::new(VecModel::from(labels)));
    win.set_chart_details(ModelRc::new(VecModel::from(details)));
    win.set_chart_current(current);
    win.set_chart_future_from(future_from);

    // Hour-of-day profile, always 24 slots: this is what tells a user *when*
    // they actually work, which no daily total can.
    let hour_peak = r.hours.iter().copied().max().unwrap_or(0).max(1);
    let hours: Vec<f32> = r
        .hours
        .iter()
        .map(|m| (*m as f32 / hour_peak as f32).clamp(0.0, 1.0))
        .collect();
    win.set_hour_bars(ModelRc::new(VecModel::from(hours)));
    let has_hours = r.hours.iter().any(|m| *m > 0);
    win.set_has_hour_profile(has_hours);

    // ── the reckoning ────────────────────────────────────────────────────
    // The same span the report above is showing, read back through the written
    // record. Done here rather than in Slint because this is where the report's
    // own range lives, and because the digits and day names are here.
    push_reckoning(win, &st, period, offset, today, earliest, en);

    // The old single-blob report text is still fed, because the main screen's
    // compact summary reads it.
    let t = stats::report_at(
        &st.data.sessions,
        stats::Period::Day,
        0,
        today,
        earliest,
        en,
    );
    let w = stats::report_at(
        &st.data.sessions,
        stats::Period::Week,
        0,
        today,
        earliest,
        en,
    );
    let m = stats::report_at(
        &st.data.sessions,
        stats::Period::Month,
        0,
        today,
        earliest,
        en,
    );
    let mut report = String::new();
    report.push_str(&stats::narrative(&t, en));
    report.push_str("\n\n");
    report.push_str(&stats::narrative(&w, en));
    report.push_str("\n\n");
    report.push_str(&stats::narrative(&m, en));
    win.set_report_text(SharedString::from(report));

    let mut tasks_line = String::new();
    let open = st.live_tasks().filter(|t| !t.done).count();
    let today_key = today.format("%Y-%m-%d").to_string();
    let today_open = st
        .live_tasks()
        .filter(|t| !t.done && t.date == today_key)
        .count();
    if en {
        tasks_line.push_str(&format!("Tasks: {} open ({} today) ", open, today_open));
    } else {
        tasks_line.push_str(&format!("کارها: {} باز ({} امروز) ", open, today_open));
    }
    for task in st
        .data
        .tasks
        .iter()
        .filter(|t| !t.done && t.date == today.format("%Y-%m-%d").to_string())
    {
        tasks_line.push_str(&format!("• {}", task.title));
        if !task.time.is_empty() {
            tasks_line.push_str(&format!(" @{}", task.time));
        }
        tasks_line.push(' ');
    }
    win.set_status_line(SharedString::from(tasks_line));

    // Session counter: current session index vs daily goal.
    let goal = app.settings.lock().unwrap().daily_goal_minutes;
    win.set_session_counter(SharedString::from(if en {
        format!("Session {} · goal {} min", st.data.sessions.len() + 1, goal)
    } else {
        format!("نشست {} · هدف {} دقیقه", st.data.sessions.len() + 1, goal)
    }));

    // ---- Rich data for the redesigned UI ----
    win.set_today_sessions(t.sessions as i32);
    win.set_today_minutes(t.minutes as i32);
    win.set_today_score(t.score as i32);
    win.set_today_streak(t.streak as i32);
    win.set_today_consistency((t.consistency * 100.0).round() as i32);
    win.set_today_best(t.best_day_minutes as i32);
    win.set_goal_minutes(goal as i32);
    win.set_goal_done_minutes(t.minutes as i32);

    // The main screen's compact week strip. Saturday-first, taken from the
    // same week report the Stats screen uses, so the two can never disagree
    // about which day a bar belongs to.
    let wb = stats::report_at(
        &st.data.sessions,
        stats::Period::Week,
        0,
        today,
        earliest,
        en,
    );
    let week_peak = wb
        .buckets
        .iter()
        .map(|b| b.minutes)
        .max()
        .unwrap_or(0)
        .max(1);
    let week_bars: Vec<f32> = wb
        .buckets
        .iter()
        .map(|b| (b.minutes as f32 / week_peak as f32).clamp(0.0, 1.0))
        .collect();
    win.set_week_bars(ModelRc::new(VecModel::from(week_bars)));
    let week_labels: Vec<SharedString> = wb
        .buckets
        .iter()
        .map(|b| SharedString::from(b.label.clone()))
        .collect();
    win.set_week_labels(ModelRc::new(VecModel::from(week_labels)));
    win.set_week_current(
        wb.buckets
            .iter()
            .position(|b| b.current)
            .map(|i| i as i32)
            .unwrap_or(-1),
    );

    // Task list for the redesigned panel: open tasks first, then done.
    let mut items: Vec<crate::ui::TaskItem> = st
        .live_tasks()
        .map(|t| crate::ui::TaskItem {
            id: t.id as i32,
            title: SharedString::from(t.title.clone()),
            time: SharedString::from(t.time.clone()),
            done: t.done,
        })
        .collect();
    items.sort_by_key(|t| t.done);
    win.set_tasks(ModelRc::new(VecModel::from(items)));

    drop(st);

    // The calendar card only changes at midnight; it guards itself against
    // rebuilding on every 300 ms tick.
    refresh_calendar(win, app, selected_day(app));
}

/// Recompute the automation summary line and push it to the window.
fn update_automation_summary(win: &slint::Weak<PanganWindow>, app: &AppState) {
    let settings = app.settings.lock().unwrap();
    let en = settings.lang == "en";
    let automation = automation::sorted(&settings);
    drop(settings);
    // This summary was hardcoded to Persian, so it stayed Persian in English
    // mode while every label around it switched.
    let summary = if automation.is_empty() {
        if en {
            "No routines yet.".to_string()
        } else {
            "هنوز آیینی ساخته نشده است.".to_string()
        }
    } else {
        let mut s = if en {
            format!("{} routines: ", automation.len())
        } else {
            format!("{} آیین: ", calendar::fa_num(automation.len() as i64))
        };
        for a in &automation {
            s.push_str(&format!("[{}] {}, ", a.event, a.command));
        }
        s
    };
    if let Some(w) = win.upgrade() {
        w.set_automation_summary(SharedString::from(summary));
    }
}

/// A one-line, human summary of when sync last succeeded.
///
/// Returns `None` when this device has never completed a cycle, so the card
/// shows nothing rather than "never" — a fresh install is not a failure.
fn last_sync_detail(app: &AppState, en: bool) -> Option<String> {
    let last = app.device.lock().unwrap().last_ok;
    describe_last_sync(last, chrono::Utc::now().timestamp(), en)
}

/// The formatting half of `last_sync_detail`, split out so it can be tested
/// without an `AppState`.
fn describe_last_sync(last: i64, now: i64, en: bool) -> Option<String> {
    if last <= 0 {
        return None;
    }
    let ago = (now - last).max(0);
    let when = if ago < 60 {
        if en {
            "just now".to_string()
        } else {
            "همین حالا".to_string()
        }
    } else if ago < 3600 {
        let m = ago / 60;
        if en {
            format!("{m} min ago")
        } else {
            format!("{m} دقیقه پیش")
        }
    } else if ago < 86_400 {
        let h = ago / 3600;
        if en {
            format!("{h} h ago")
        } else {
            format!("{h} ساعت پیش")
        }
    } else {
        let d = ago / 86_400;
        if en {
            format!("{d} d ago")
        } else {
            format!("{d} روز پیش")
        }
    };
    Some(if en {
        format!("Last successful sync: {when}")
    } else {
        format!("آخرین همگام‌سازی موفق: {when}")
    })
}

/// Run one cloud sync on a background thread.
///
/// The network call must not happen on the UI thread: a slow Drive request
/// would freeze the countdown, which is the one thing this app cannot do.
/// The device-code fallback sign-in.
///
/// Used only when the client id cannot do a loopback redirect, which means the
/// user registered a "TVs and Limited Input devices" client. It is the same
/// flow as before: show a code, open the page, poll until approved. The app
/// still needs the user to type eight characters, which is why the loopback
/// flow is preferred — but a working sign-in beats a blocked one.
/// Put text on the system clipboard, returning whether it worked.
///
/// Linux desktops vary wildly here, so this tries the in-process X11 selection
/// first (no external tool needed, which is the common case on a minimal
/// install), then the usual command-line helpers. Returning a bool is the point:
/// the caller must not tell the user it copied when it did not.
fn copy_to_clipboard(text: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        // No external binary required: talk to the X server directly. This is
        // what wl-copy/xclip do, minus the dependency.
        if let Ok(mut cb) = arboard::Clipboard::new() {
            if cb.set_text(text.to_string()).is_ok() {
                return true;
            }
        }
        // Fall back to the command-line tools for Wayland and odd setups.
        for (prog, args) in [
            ("wl-copy", vec![]),
            ("xclip", vec!["-selection", "clipboard"]),
            ("xsel", vec!["--clipboard", "--input"]),
        ] {
            let ok = std::process::Command::new(prog)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .and_then(|mut c| {
                    use std::io::Write;
                    if let Some(stdin) = c.stdin.as_mut() {
                        let _ = stdin.write_all(text.as_bytes());
                    }
                    c.wait()
                })
                .map(|st| st.success())
                .unwrap_or(false);
            if ok {
                return true;
            }
        }
        false
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                if let Some(stdin) = c.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                c.wait()
            })
            .map(|st| st.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        // `clip` is present on every Windows install.
        std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                if let Some(stdin) = c.stdin.as_mut() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                c.wait()
            })
            .map(|st| st.success())
            .unwrap_or(false)
    }
}

/// Append one line to the sync log.
///
/// Sign-in failures used to be invisible: the status line said "Device
/// connected" on the phone while the app quietly gave up, and there was nothing
/// on disk to say why. Every step of the flow now leaves a line in
/// `~/.config/pangan/sync.log`, so a failure can be diagnosed instead of guessed
/// at. Secrets are never written: only outcomes and error strings.
fn sync_log(msg: &str) {
    use std::io::Write;
    let path = crate::state::store::Store::data_dir().join("sync.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn device_flow_sign_in(
    _client_id: &str,
    en: bool,
    weak: &slint::Weak<PanganWindow>,
) -> Result<oauth::SignedIn, String> {
    let started = std::time::Instant::now();
    sync_log("device flow: requesting a device code");
    let code = match gdrive::request_device_code() {
        Ok(c) => {
            sync_log(&format!(
                "device flow: got code {} (expires in {}s, interval {}s)",
                c.user_code, c.expires_in, c.interval
            ));
            c
        }
        Err(e) => {
            sync_log(&format!("device flow: device code request FAILED: {e}"));
            return Err(e);
        }
    };

    // Show the code before polling, so the user can start typing immediately.
    {
        let user_code = code.user_code.clone();
        let url = code.verification_url.clone();
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = w.upgrade() {
                win.set_sync_device_code(SharedString::from(user_code));
                win.set_sync_device_url(SharedString::from(url));
            }
        });
        open_in_browser(&code.verification_url);
    }

    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(code.expires_in.min(1800));
    let interval = gdrive::poll_interval(code.interval);
    loop {
        std::thread::sleep(interval);
        if std::time::Instant::now() > deadline {
            let w = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = w.upgrade() {
                    win.set_sync_device_code(SharedString::new());
                }
            });
            return Err(if en {
                "the code expired".to_string()
            } else {
                "کد منقضی شد".to_string()
            });
        }
        match gdrive::poll_token(&code.device_code) {
            gdrive::PollOutcome::Pending => continue,
            gdrive::PollOutcome::Failed(e) => {
                sync_log(&format!("device flow: poll FAILED: {e}"));
                return Err(e);
            }
            gdrive::PollOutcome::Approved(tokens) => {
                sync_log(&format!(
                    "device flow: approved, access token {} chars, refresh token {} chars",
                    tokens.access_token.len(),
                    tokens.refresh_token.len()
                ));
                return Ok(oauth::SignedIn {
                    tokens,
                    seconds: started.elapsed().as_secs(),
                });
            }
        }
    }
}

fn do_sync(win: &slint::Weak<PanganWindow>, app: &Arc<AppState>) {
    let en = app.settings.lock().unwrap().lang == "en";
    let tokens = app.tokens.lock().unwrap().clone();
    if tokens.is_empty() {
        let status = if en {
            "Not signed in".to_string()
        } else {
            "وارد نشده‌اید".to_string()
        };
        *app.last_sync.lock().unwrap() = status.clone();
        if let Some(w) = win.upgrade() {
            w.set_sync_status(SharedString::from(status));
        }
        return;
    }

    // Snapshot the data now; the merge happens against this copy.
    let local = cloudsync::Local {
        data: app.store.lock().unwrap().data.clone(),
        settings: app.settings.lock().unwrap().clone(),
    };

    let weak = win.clone();
    let app2 = Arc::clone(app);
    let tokens_arc = Arc::clone(&app.tokens);
    let engine = Arc::clone(&app.cloud);
    let device_arc = Arc::clone(&app.device);

    // Claim the engine *before* the caller consumes the dirty flag, and put the
    // flag back if the claim fails. Doing it the other way round lost edits: the
    // timer cleared `dirty`, then found a cycle already running and returned,
    // and the edit — made after that cycle snapshotted the data — was never
    // pushed until the next interval, or until the next launch if the user
    // closed the app first.
    if !engine.try_begin() {
        engine.mark_dirty();
        return; // one cycle at a time; the running one will report
    }

    std::thread::spawn(move || {
        // The credential and device locks must NOT be held across the network
        // call. They used to be, and the UI tick reads both (`is_empty()` on
        // every 300 ms tick, `last_ok` for the settings card), so any sync in
        // flight froze the countdown for as long as Drive took to answer — up
        // to the 20 s request timeout, three attempts, plus the conflict
        // backoff. Adding a task marks the data dirty, which starts a sync
        // immediately, which is exactly when the clock appeared to stop.
        //
        // So: take a copy, release, work on the copy, then write the refreshed
        // credentials back under a brief lock. Nothing is lost by this — the
        // engine already guarantees one cycle at a time, so there is no second
        // writer to race with.
        let mut t = tokens_arc.lock().unwrap().clone();
        let mut dev = device_arc.lock().unwrap().clone();
        let result = cloudsync::sync_once(
            &mut t,
            &local,
            &mut dev,
            &mut |merged: &store::Data, merged_settings: &Settings| {
                let mut st = app2.store.lock().unwrap();
                st.data = merged.clone();
                st.save();
                drop(st);
                let mut s = app2.settings.lock().unwrap();
                *s = merged_settings.clone();
                s.save();
            },
        );
        // Persist refreshed credentials so the next launch does not sign in
        // again, and so a rotated refresh token is not lost.
        {
            if !t.is_empty() {
                *tokens_arc.lock().unwrap() = t.clone();
                let _ = sync::save_tokens(&t);
            }
        }
        // Persist the device stamps: without them the next launch would treat
        // every setting as freshly edited and overwrite the other device.
        // The last-success time is stamped here too, so the settings card can
        // still say when sync last worked after a restart.
        {
            if result.is_ok() {
                dev.last_ok = chrono::Utc::now().timestamp();
            }
            dev.save();
            *device_arc.lock().unwrap() = dev;
        }

        let status = match &result {
            Ok(out) => {
                let stamp = chrono::Local::now().format("%H:%M").to_string();
                // Say what actually moved. "Synced" for every outcome hides
                // the interesting case — that settings arrived from another
                // machine — and the user is left wondering why the theme
                // changed on its own.
                if out.quiet() {
                    if en {
                        format!("Synced {stamp}")
                    } else {
                        format!("همگام شد {stamp}")
                    }
                } else if out.settings_pulled && !out.data_pulled {
                    let from = if out.remote_device.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", out.remote_device)
                    };
                    if en {
                        format!("Settings updated{from} {stamp}")
                    } else {
                        format!("تنظیمات به‌روز شد{from} {stamp}")
                    }
                } else if out.pulled {
                    let from = if out.remote_device.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", out.remote_device)
                    };
                    if en {
                        format!("Updated from cloud{from} {stamp}")
                    } else {
                        format!("از ابر به‌روز شد{from} {stamp}")
                    }
                } else {
                    if en {
                        format!("Uploaded {stamp}")
                    } else {
                        format!("بارگذاری شد {stamp}")
                    }
                }
            }
            Err(e) => {
                if en {
                    format!("Sync error: {e}")
                } else {
                    format!("خطای همگام‌سازی: {e}")
                }
            }
        };

        {
            let mut st = engine.state.lock().unwrap();
            st.busy = false;
            st.status = status.clone();
            if result.is_ok() {
                st.last_ok = chrono::Utc::now().timestamp();
            }
        }

        // Track failure streaks so the timer can back off, and notice a dead
        // credential: a revoked or expired refresh token can never succeed, so
        // the right move is to stop syncing and tell the user to sign in again
        // rather than retry forever behind a status line that says "error".
        let mut signed_out = false;
        match &result {
            Ok(_) => engine
                .consecutive_failures
                .store(0, std::sync::atomic::Ordering::Relaxed),
            Err(e) => {
                // Every failure leaves a line. Without this, a sync that failed
                // for a reason nobody could see was indistinguishable from one
                // that never ran, and a wrong `is_auth_error` verdict signed the
                // user out with no trace of why.
                sync_log(&format!("sync FAILED: {e}"));
                if gdrive::is_auth_error(e) {
                    sync_log("sync: treating this as a dead credential, signing out");
                    signed_out = true;
                    let _ = sync::clear_tokens();
                    *app2.tokens.lock().unwrap() = gdrive::Tokens::default();
                    {
                        let mut st = app2.settings.lock().unwrap();
                        st.sync_enabled = false;
                        st.save();
                    }
                } else {
                    engine
                        .consecutive_failures
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        if result.is_ok() {
            sync_log("sync ok");
        }

        *app2.last_sync.lock().unwrap() = status.clone();

        // Settings that arrived from the cloud have to reach the window, or the
        // user sees the old theme and the old durations until the next launch.
        // This is the half of sync that is invisible when it works and obvious
        // when it does not.
        let settings_arrived = matches!(&result, Ok(out) if out.settings_pulled);
        // Computed before the closure so the event loop does no file locking.
        let detail = last_sync_detail(&app2, en);

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                w.set_sync_status(SharedString::from(status));
                if let Some(d) = detail {
                    w.set_sync_detail(SharedString::from(d));
                }
                if settings_arrived {
                    push_settings_to_ui(&w, &app2);
                    // New durations have to reach the timer, not just the
                    // settings screen. A running countdown keeps its own total
                    // (the user started a 25-minute block and it should finish
                    // as one); the new value applies from the next phase.
                    let cfg = clock::Config::from(&*app2.settings.lock().unwrap());
                    app2.shared.set_config(cfg);
                }
                if signed_out {
                    // Drop the UI back to the signed-out state so the Connect
                    // button returns. The account is genuinely gone; leaving
                    // the card showing "Connected" would be a lie.
                    w.set_sync_signed_in(false);
                    w.set_sync_auto_on(false);
                    w.set_sync_account(SharedString::new());
                }
            }
        });
    });
}

/// Push the model's settings into the window.
///
/// Used at startup and whenever a cloud sync delivers settings from another
/// device. Keeping it in one place is the point: the startup path and the sync
/// path must show the same values, and a control that is wired into one but
/// not the other is the kind of bug that only appears on the second machine.
fn push_settings_to_ui(win: &PanganWindow, app: &AppState) {
    let s = app.settings.lock().unwrap().clone();

    win.set_work_minutes(s.work_minutes as i32);
    win.set_short_break_minutes(s.short_break_minutes as i32);
    win.set_long_break_minutes(s.long_break_minutes as i32);
    win.set_long_break_interval(s.long_break_interval as i32);
    win.set_auto_start_next(s.auto_start_next);
    win.set_auto_start_on_launch(s.auto_start_on_launch);
    win.set_daily_goal_minutes(s.daily_goal_minutes as i32);
    win.set_compact_mode(s.compact_mode);
    win.set_animations(s.animations);
    crate::ui::Motion::get(win).set_enabled(s.animations);
    win.set_sound_enabled(s.sound_enabled);
    win.set_sound_theme(SharedString::from(s.sound_theme.clone()));
    win.set_sound_volume(s.sound_volume as i32);
    win.set_hotkey_enabled(s.hotkey_enabled);
    win.set_inhibit_enabled(s.inhibit_enabled);
    win.set_overlay_enabled(s.overlay_enabled);
    win.set_era(SharedString::from(s.era.clone()));
    win.set_era_label(SharedString::from(s.era_label()));
    win.set_era_blurb(SharedString::from(s.era_blurb()));
    win.set_theme_pref(SharedString::from(s.theme.clone()));
    win.set_theme_label(SharedString::from(s.theme_label()));

    let resolved = resolve_theme(&s.theme);
    win.set_resolved_theme(SharedString::from(resolved));
    apply_palette(win, resolved, &s.era);
    crate::ui::Strings::get(win).set_lang(SharedString::from(s.lang.clone()));

    *app.sound_theme.lock().unwrap() = s.sound_theme.clone();
}

/// Push the current settings values from the window back into the model and
/// persist them.
fn apply_ui_settings(win: &PanganWindow, app: &AppState) -> Settings {
    let mut s = app.settings.lock().unwrap().clone();
    s.work_minutes = win.get_work_minutes().max(1) as u64;
    s.short_break_minutes = win.get_short_break_minutes().max(1) as u64;
    s.long_break_minutes = win.get_long_break_minutes().max(1) as u64;
    s.long_break_interval = win.get_long_break_interval().max(1) as u32;
    s.auto_start_next = win.get_auto_start_next();
    s.auto_start_on_launch = win.get_auto_start_on_launch();
    s.daily_goal_minutes = win.get_daily_goal_minutes().max(15) as u64;
    s.theme = win.get_theme_pref().to_string();
    s.compact_mode = win.get_compact_mode();
    s.animations = win.get_animations();
    s.sound_enabled = win.get_sound_enabled();
    s.sound_theme = win.get_sound_theme().to_string();
    s.sound_volume = win.get_sound_volume().clamp(0, 100) as u8;
    s.hotkey_enabled = win.get_hotkey_enabled();
    s.inhibit_enabled = win.get_inhibit_enabled();
    s.overlay_enabled = win.get_overlay_enabled();
    s.sanitize_mut();
    s.save();
    *app.settings.lock().unwrap() = s.clone();
    *app.sound_theme.lock().unwrap() = s.sound_theme.clone();
    // Settings are portable, so closing the settings screen is a sync event.
    // Without this the other device kept the old durations until the next
    // interval elapsed — the user changes 25 to 50, opens the laptop, and gets
    // 25 minutes. The whole point of syncing settings is that this does not
    // happen.
    app.cloud.mark_dirty();
    s
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    crate::platform::autostart::ensure();

    let settings = Settings::load();
    let cfg = clock::Config::from(&settings);
    let shared = Arc::new(Shared::new(cfg));
    let app = Arc::new(AppState::load(Arc::clone(&shared)));

    // Sounds: warm all themes, then pick the active one.
    let sounds_dir = store::Store::data_dir().join("sounds");
    sound::Sounds::warm_all(&sounds_dir);
    let active_theme = settings.sound_theme.clone();
    let sounds = sound::Sounds::ensure(&sounds_dir, &active_theme);

    let win = PanganWindow::new()?;
    let weak = win.as_weak();

    let (tx, rx) = channel::<TrayAction>();

    let inhibit = Arc::new(Mutex::new(None::<crate::platform::inhibit::InhibitGuard>));

    // Initial state of the Google account card, read from what was stored on
    // the last run, so a signed-in user does not see "Not connected" flash.
    {
        let tokens = app.tokens.lock().unwrap().clone();
        let signed_in = !tokens.is_empty();
        win.set_sync_signed_in(signed_in);
        win.set_sync_auto_on(settings.sync_enabled);
        win.set_sync_available(gdrive::has_client_id());
        if let Some(id) = sync::load_client_id() {
            win.set_sync_client_id(SharedString::from(id));
        }
        if signed_in {
            win.set_sync_account(SharedString::from(tokens.email.clone()));
        }
        // Which machine this is, so the card can say so once there are two.
        let device_name = app.device.lock().unwrap().device_name.clone();
        win.set_sync_device_name(SharedString::from(device_name));
        // When the last successful exchange happened, if it ever did. A card
        // that only says "Connected" cannot tell a user whether sync is
        // actually running or whether it has been failing quietly for a day.
        if let Some(detail) = last_sync_detail(&app, settings.lang == "en") {
            win.set_sync_detail(SharedString::from(detail));
        }
    }

    // Initial theme application.
    {
        let resolved = resolve_theme(&settings.theme);
        win.set_resolved_theme(SharedString::from(resolved));
        win.set_theme_label(SharedString::from(settings.theme_label()));
        win.set_theme_pref(SharedString::from(settings.theme.clone()));
        apply_palette(&win, resolved, &settings.era);
    }

    // Push settings into the UI. The same helper runs when a cloud sync
    // delivers settings from another device, so the two paths cannot drift.
    push_settings_to_ui(&win, &app);

    // The UI deliberately has no width/height binding (that would undo any
    // user resize), so the initial size is applied here, once. After this the
    // window manager owns the geometry.
    {
        let (w0, h0) = if settings.compact_mode {
            (320.0, 300.0)
        } else {
            (420.0, 680.0)
        };
        win.window()
            .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(w0, h0)));
    }

    // UI tick: countdown + inhibit + session persistence + auto-sync.
    let tick_shared = Arc::clone(&shared);
    let tick_weak = weak.clone();
    let tick_inhibit = Arc::clone(&inhibit);
    let tick_app = Arc::clone(&app);
    let tick_sounds_work = sounds.work.clone();
    let tick_sounds_rest = sounds.rest.clone();
    // Start the clock in the past so the first tick syncs at once. Waiting a
    // full interval meant a user who opened the app for a minute saw nothing
    // arrive from their other machine — and then assumed sync was broken.
    let mut last_auto_sync = chrono::Local::now() - chrono::Duration::seconds(3600);

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(300), move || {
        let notify_msg = tick_shared.tick();

        let recs = tick_shared.drain_sessions();
        let breaks = tick_shared.take_breaks();
        let events = tick_shared.drain_events();
        let volume = tick_app.settings.lock().unwrap().sound_volume;
        if !events.is_empty() {
            let settings_snapshot = tick_app.settings.lock().unwrap().clone();
            for ev in &events {
                automation::fire_event(&settings_snapshot, ev, "session");
            }
        }
        if !recs.is_empty() {
            {
                let mut st = tick_app.store.lock().unwrap();
                st.add_sessions(&recs);
                // Offer to write a line about the block that just finished. This
                // is the only moment the app knows both that the work happened
                // and roughly what it was, so it is the only moment worth
                // asking. The newest session is the one being offered; an older
                // unwritten session is not resurrected, because by then the user
                // has lost the thread of what it was.
                if let Some(newest) = recs.iter().map(|r| r.at).max() {
                    // Attach the session to the plan it answered, before anything
                    // else touches the store. This is the only instant the app
                    // knows both that a block finished and which rota entry was
                    // open for it — the rota row keeps `opened_at` precisely so
                    // the pair can be joined here and nowhere else.
                    //
                    // A block started from the plain timer claims nothing: no
                    // entry was open, so no plan changed, and no new plan is
                    // invented to hold a session that never had one.
                    st.claim_rota_outcome(newest);
                    // Pair the intention with the session that just ended. This
                    // is the only instant both timestamps exist: `written_at`
                    // was captured when the user pressed Issue, `newest` is now.
                    // The pending slot is taken rather than read, so one decree
                    // can never be attached to two sessions — a stale one left
                    // behind by a skipped block would otherwise be waiting here
                    // for the next session and would answer for work it never
                    // described.
                    let pending = tick_app.pending_decree.lock().unwrap().take();
                    if let Some((text, written_at)) = pending {
                        if let Ok(dec) = journal::decree::create(newest, written_at, &text) {
                            st.add_decree(dec);
                        }
                    }

                    let already_written = st.inscription_for_session(newest).is_some();
                    let skipped = *tick_app.ins_skipped.lock().unwrap() == Some(newest);
                    if !already_written && !skipped {
                        if let Some(w) = tick_weak.upgrade() {
                            w.set_inscription_session(newest as i32);
                            w.set_inscription_text(SharedString::from(""));
                            w.set_inscription_error(SharedString::from(""));
                            w.set_inscription_open(true);
                        }
                    }
                }
            }
            // A finished focus block is the most important thing to sync: it
            // is the user's actual history.
            tick_app.cloud.mark_dirty();
            if tick_app.settings.lock().unwrap().sound_enabled {
                sound::play(&tick_sounds_work, volume);
            }
        } else if breaks > 0 && tick_app.settings.lock().unwrap().sound_enabled {
            sound::play(&tick_sounds_rest, volume);
        }

        if let Some(win) = tick_weak.upgrade() {
            apply_snapshot(&win, &tick_shared);
            apply_session_dots(&win, &tick_shared, &tick_app);
            refresh_screens(&win, &tick_app);
        }

        {
            let mut hold = tick_inhibit.lock().unwrap();
            let inhibit_on = tick_app.settings.lock().unwrap().inhibit_enabled;
            if inhibit_on && tick_shared.should_inhibit() && hold.is_none() {
                *hold = crate::platform::inhibit::acquire();
            } else if !inhibit_on || !tick_shared.should_inhibit() {
                *hold = None;
            }
        }

        // Automatic sync. Two triggers, both needed:
        //
        //  * a change just happened, so push it now — waiting for the interval
        //    would leave the user staring at a stale phone for minutes;
        //  * the interval elapsed, so pull — this is how a change made
        //    elsewhere arrives when nothing is happening locally.
        //
        // The engine refuses to start a second cycle while one is running, so
        // a burst of edits cannot stack up requests.
        {
            let settings = tick_app.settings.lock().unwrap().clone();
            let signed_in = !tick_app.tokens.lock().unwrap().is_empty();
            if settings.sync_enabled && signed_in {
                // A failed sync backs off instead of retrying every tick: a
                // dead network would otherwise mean a request every 300 ms,
                // which burns battery and hammers Drive for nothing. The
                // delay doubles up to five minutes and resets on success.
                let failures = tick_app
                    .cloud
                    .consecutive_failures
                    .load(std::sync::atomic::Ordering::Relaxed);
                let base = settings.sync_interval_secs.max(30) as i64;
                let interval = if failures == 0 {
                    base
                } else {
                    (base * (1 << failures.min(4))).min(300)
                };
                let due =
                    chrono::Local::now() - last_auto_sync > chrono::Duration::seconds(interval);
                if tick_app.cloud.take_dirty() || due {
                    last_auto_sync = chrono::Local::now();
                    do_sync(&tick_weak, &tick_app);
                }
            }
        }

        if let Some(text) = notify_msg {
            notify(&text);
        }
    });

    // Poll tray/hotkey events.
    let poll_shared = Arc::clone(&shared);
    let poll_app = Arc::clone(&app);
    let poll_weak = weak.clone();
    let poll_timer = Timer::default();
    poll_timer.start(TimerMode::Repeated, Duration::from_millis(80), move || {
        while let Ok(action) = rx.try_recv() {
            run_action(action, &poll_shared, &poll_app, &poll_weak);
            if action == TrayAction::Quit {
                break;
            }
        }
    });

    // Window close -> hide to tray.
    let close_weak = weak.clone();
    win.window().on_close_requested(move || {
        if let Some(win) = close_weak.upgrade() {
            let _ = win.hide();
        }
        CloseRequestResponse::KeepWindowShown
    });

    // UI callbacks.
    let toggle_shared = Arc::clone(&shared);
    let toggle_app = Arc::clone(&app);
    let toggle_weak = weak.clone();
    win.on_start_pause(move || {
        // Starting a focus block from rest is the one moment worth asking what
        // the block is for: the user is at the keyboard, and the answer is still
        // in their head. Resuming a block already in flight is not — it would
        // ask the same question twice and interrupt a running countdown.
        //
        // The tray and hotkey paths go through `run_action` and deliberately do
        // not ask: a global hotkey exists to start work without touching the
        // window, and a dialog appearing over whatever app you were in would
        // defeat that entirely.
        if toggle_shared.at_fresh_work_start() {
            if let Some(w) = toggle_weak.upgrade() {
                w.set_decree_text(SharedString::from(""));
                w.set_decree_error(SharedString::from(""));
                w.set_decree_count(SharedString::from(String::new()));
                w.set_can_issue(false);
                w.set_decree_open(true);
            }
            return;
        }
        run_action(
            TrayAction::Toggle,
            &toggle_shared,
            &toggle_app,
            &toggle_weak,
        );
    });

    let skip_shared = Arc::clone(&shared);
    let skip_app = Arc::clone(&app);
    let skip_weak = weak.clone();
    win.on_skip(move || {
        run_action(TrayAction::Skip, &skip_shared, &skip_app, &skip_weak);
    });

    let reset_shared = Arc::clone(&shared);
    let reset_app = Arc::clone(&app);
    let reset_weak = weak.clone();
    win.on_reset(move || {
        run_action(TrayAction::Reset, &reset_shared, &reset_app, &reset_weak);
    });

    // Report open/close.
    let open_weak = weak.clone();
    win.on_open_report(move || {
        if let Some(w) = open_weak.upgrade() {
            w.set_show_view("report".into());
        }
    });
    let close_weak = weak.clone();
    win.on_close_report(move || {
        if let Some(w) = close_weak.upgrade() {
            w.set_show_view("main".into());
        }
    });

    let insopen_app = Arc::clone(&app);
    let insopen_weak = weak.clone();
    win.on_open_inscriptions(move || {
        if let Some(w) = insopen_weak.upgrade() {
            w.set_show_view("inscriptions".into());
            let st = insopen_app.store.lock().unwrap();
            push_inscriptions(&w, &insopen_app, &st);
        }
    });
    let insclose_weak = weak.clone();
    win.on_close_inscriptions(move || {
        if let Some(w) = insclose_weak.upgrade() {
            w.set_show_view("main".into());
        }
    });

    // ---- Stats navigation ----
    //
    // Three controls drive the whole screen: pick a period shape, step one
    // period back or forward, and jump back to the current one. Every one of
    // them writes to AppState and then re-renders, so the screen is always a
    // pure function of (period, offset) and cannot drift out of sync with the
    // numbers it is showing.
    let period_app = Arc::clone(&app);
    let period_weak = weak.clone();
    win.on_set_report_period(move |index| {
        *period_app.report_period.lock().unwrap() = stats::Period::from_index(index);
        // Switching shape resets to the current span: "last month" makes no
        // sense as a starting point for the Day view.
        *period_app.report_offset.lock().unwrap() = 0;
        if let Some(w) = period_weak.upgrade() {
            refresh_screens(&w, &period_app);
        }
    });

    let step_app = Arc::clone(&app);
    let step_weak = weak.clone();
    win.on_step_report(move |delta| {
        let period = *step_app.report_period.lock().unwrap();
        if period == stats::Period::All {
            return; // all-time has nothing to step through
        }
        let mut off = step_app.report_offset.lock().unwrap();
        let next = *off + delta;
        if next > 0 {
            return; // never step past the current period into the future
        }
        *off = next;
        drop(off);
        if let Some(w) = step_weak.upgrade() {
            refresh_screens(&w, &step_app);
        }
    });

    let now_app = Arc::clone(&app);
    let now_weak = weak.clone();
    win.on_report_today(move || {
        *now_app.report_offset.lock().unwrap() = 0;
        if let Some(w) = now_weak.upgrade() {
            refresh_screens(&w, &now_app);
        }
    });

    // Today (Persian calendar) open/close. Opening rebuilds the card so the
    // date is right even if the app has been left running across midnight.
    let today_open_weak = weak.clone();
    let today_open_app = Arc::clone(&app);
    win.on_open_today(move || {
        if let Some(w) = today_open_weak.upgrade() {
            *today_open_app.calendar_day.lock().unwrap() = (0, 0, 0);
            refresh_calendar(&w, &today_open_app, selected_day(&today_open_app));
            w.set_show_view("today".into());
        }
    });
    let today_close_weak = weak.clone();
    win.on_close_today(move || {
        if let Some(w) = today_close_weak.upgrade() {
            w.set_show_view("main".into());
        }
    });

    // Settings open/close.
    let set_open_weak = weak.clone();
    win.on_open_settings(move || {
        if let Some(w) = set_open_weak.upgrade() {
            w.set_show_view("settings".into());
        }
    });
    let set_close_weak = weak.clone();
    let set_close_app = Arc::clone(&app);
    win.on_close_settings(move || {
        if let Some(w) = set_close_weak.upgrade() {
            let _ = apply_ui_settings(&w, &set_close_app);
            // Rebase the timer on the new durations. This is the whole point of
            // the settings screen: without this call the new numbers were saved
            // to disk and then never reached the clock, so the dial kept the old
            // duration until a restart.
            let s = set_close_app.settings.lock().unwrap().clone();
            let cfg = clock::Config::from(&s);
            set_close_app.shared.set_config(cfg);
            // Push the new snapshot so the dial and the phase label update now.
            apply_snapshot(&w, &set_close_app.shared);
            w.set_theme_label(SharedString::from(s.theme_label()));
            let resolved = resolve_theme(&s.theme);
            w.set_resolved_theme(SharedString::from(resolved));
            apply_palette(&w, resolved, &s.era);
            w.set_show_view("main".into());
        }
    });

    // Theme cycling (header button cycles system -> dark -> light).
    let theme_app = Arc::clone(&app);
    let theme_weak = weak.clone();
    win.on_cycle_theme(move || {
        let mut s = theme_app.settings.lock().unwrap();
        s.theme = match s.theme.as_str() {
            "system" => "dark".into(),
            "dark" => "light".into(),
            _ => "system".into(),
        };
        let label = s.theme_label();
        let pref = s.theme.clone();
        let era = s.era.clone();
        s.save();
        drop(s);
        // The theme is a portable setting: cycle it here and the laptop follows.
        theme_app.cloud.mark_dirty();
        let resolved = resolve_theme(&pref);
        if let Some(w) = theme_weak.upgrade() {
            w.set_theme_label(SharedString::from(label));
            w.set_theme_pref(SharedString::from(pref));
            w.set_resolved_theme(SharedString::from(resolved));
            apply_palette(&w, resolved, &era);
        }
    });

    // Sound toggle (header button).
    let sound_app = Arc::clone(&app);
    win.on_toggle_sound(move || {
        let mut s = sound_app.settings.lock().unwrap();
        s.sound_enabled = !s.sound_enabled;
        s.save();
        drop(s);
        // `sound_enabled` is portable; a silent laptop after turning sound on
        // at the desktop is exactly the surprise sync is supposed to remove.
        sound_app.cloud.mark_dirty();
    });

    // Preview a sound theme (reads the currently-selected theme from the UI).
    let preview_app = Arc::clone(&app);
    let preview_weak = weak.clone();
    win.on_preview_sound(move || {
        let theme = preview_weak
            .upgrade()
            .map(|w| w.get_sound_theme().to_string())
            .unwrap_or_default();
        let volume = preview_app.settings.lock().unwrap().sound_volume;
        let dir = store::Store::data_dir().join("sounds");
        let snd = sound::Sounds::ensure(&dir, &theme);
        sound::play(&snd.work, volume);
    });

    // Add task.
    let task_app = Arc::clone(&app);
    let task_weak = weak.clone();
    win.on_add_task(move || {
        let txt = match task_weak.upgrade() {
            Some(w) => w.get_new_task_text().to_string(),
            None => return,
        };
        if txt.trim().is_empty() {
            return;
        }
        // Stamp the task with the Jalali day, which is the key every other
        // part of the app uses. A Gregorian stamp here meant the task existed
        // but appeared on no calendar day at all.
        let (ty, tm, td) = calendar::today();
        let today = format!("{ty:04}-{tm:02}-{td:02}");
        {
            let mut st = task_app.store.lock().unwrap();
            st.add_task(txt.trim().to_string(), today, String::new());
        }
        task_app.cloud.mark_dirty();
        if let Some(w) = task_weak.upgrade() {
            w.set_new_task_text(SharedString::from(""));
            refresh_screens(&w, &task_app);
            refresh_day_view(&w, &task_app);
        }
    });

    // ── inscriptions: the written record of each session ──
    //
    // The dialog opens from the tick loop when a focus session finishes, not
    // from here. These handlers only record what the user typed.
    let ins_app = Arc::clone(&app);
    let ins_weak = weak.clone();
    win.on_inscribe(move |session_at, text| {
        let session_at = session_at as i64;
        let raw = text.to_string();
        let now = chrono::Utc::now().timestamp();
        match journal::create(session_at, now, &raw) {
            Ok(ins) => {
                {
                    let mut st = ins_app.store.lock().unwrap();
                    st.add_inscription(ins);
                }
                // The record is the user's own writing about their work, so it
                // is worth the same immediate sync a finished session gets.
                ins_app.cloud.mark_dirty();
                if let Some(w) = ins_weak.upgrade() {
                    w.set_inscription_open(false);
                    w.set_inscription_text(SharedString::from(""));
                    w.set_inscription_error(SharedString::from(""));
                    refresh_screens(&w, &ins_app);
                }
            }
            Err(e) => {
                // The refusal is shown next to the field rather than swallowed:
                // a dialog that silently refuses a keystroke reads as broken.
                if let Some(w) = ins_weak.upgrade() {
                    w.set_inscription_error(SharedString::from(format!("{e}")));
                }
            }
        }
    });

    // Skipping has to be remembered, or the same session asks again on the next
    // tick — twelve times a second — and the dialog becomes impossible to close.
    let skip_app = Arc::clone(&app);
    let skip_weak = weak.clone();
    win.on_skip_inscription(move |session_at| {
        let session_at = session_at as i64;
        *skip_app.ins_skipped.lock().unwrap() = Some(session_at);
        if let Some(w) = skip_weak.upgrade() {
            w.set_inscription_open(false);
            w.set_inscription_text(SharedString::from(""));
            w.set_inscription_error(SharedString::from(""));
        }
    });

    // The dialog's own text drives two derived values. Doing it here keeps the
    // rule in one place: the button enables exactly when `journal::create`
    // would accept the line, so the two can never disagree about "blank".
    {
        let sync_app = Arc::clone(&app);
        let sync_weak = weak.clone();
        win.on_inscription_text_changed(move |text| {
            let t = text.to_string();
            let trimmed = t.trim();
            let count = trimmed.chars().count() as i32;
            let over = count > journal::MAX_CHARS as i32;
            if let Some(w) = sync_weak.upgrade() {
                w.set_can_inscribe(!trimmed.is_empty() && !over);
                w.set_inscription_count(SharedString::from(format!(
                    "{}{} / {}",
                    count,
                    if over { "  ⚠" } else { "" },
                    journal::MAX_CHARS
                )));
            }
            let _ = &sync_app;
        });
    }

    let dismiss_weak = weak.clone();
    win.on_dismiss_inscription(move || {
        if let Some(w) = dismiss_weak.upgrade() {
            w.set_inscription_open(false);
            w.set_inscription_error(SharedString::from(""));
        }
    });

    let delins_app = Arc::clone(&app);
    let delins_weak = weak.clone();
    // ── verdicts: what the user says a block actually was ─────────────────
    //
    // Declared from the archive row, one press, and it replaces whatever was
    // said before about the same session rather than adding a second word. The
    // store owns that rule (`put`), so a re-declaration from the UI and one
    // arriving through a sync cannot disagree about it.
    let verdict_app = Arc::clone(&app);
    let verdict_weak = weak.clone();
    win.on_declare_verdict(move |session, key| {
        // An empty key means "take the word back": pressing the filled chip is
        // the only route back to the five seals once a verdict is declared.
        // Handled here rather than as a second callback so the UI has one
        // gesture to wire and one rule to learn.
        if key.is_empty() {
            let retracted = {
                let mut st = verdict_app.store.lock().unwrap();
                st.retract_verdict(session as i64)
            };
            if retracted {
                verdict_app.cloud.mark_dirty();
            }
            if let Some(w) = verdict_weak.upgrade() {
                refresh_screens(&w, &verdict_app);
            }
            return;
        }
        // An unknown key is a UI bug, not a user error: nothing is said and
        // nothing is written, rather than storing a verdict no seal can show.
        let Some(kind) = verdict_from_key(key.as_str()) else {
            return;
        };
        {
            let mut st = verdict_app.store.lock().unwrap();
            if st.declare_verdict(session as i64, kind).is_ok() {
                verdict_app.cloud.mark_dirty();
            }
        }
        if let Some(w) = verdict_weak.upgrade() {
            refresh_screens(&w, &verdict_app);
        }
    });

    win.on_delete_inscription(move |id| {
        let id = id.to_string();
        {
            let mut st = delins_app.store.lock().unwrap();
            // A row can be an inscription, a decree-only entry, or both. Both
            // are tried rather than being told apart by a type field on the row:
            // the ids live in different collections, an id can only be in one,
            // and asking twice is cheaper than carrying a discriminator through
            // the UI for this one purpose.
            if !st.remove_inscription(&id) {
                st.remove_decree(&id);
            }
        }
        delins_app.cloud.mark_dirty();
        if let Some(w) = delins_weak.upgrade() {
            refresh_screens(&w, &delins_app);
        }
    });

    // The search box writes into app state, not the window, so the tick that
    // rebuilds the list cannot clear what was typed.
    let search_app = Arc::clone(&app);
    let search_weak = weak.clone();
    win.on_search_inscriptions(move |q| {
        *search_app.ins_query.lock().unwrap() = q.to_string();
        if let Some(w) = search_weak.upgrade() {
            let st = search_app.store.lock().unwrap();
            push_inscriptions(&w, &search_app, &st);
        }
    });

    // Export writes the Markdown next to the data file, which is where the user
    // already knows to look, and reports the path rather than claiming success.
    let exp_app = Arc::clone(&app);
    let exp_weak = weak.clone();
    win.on_export_inscriptions(move || {
        let en = exp_app.settings.lock().unwrap().lang == "en";
        let md = {
            let st = exp_app.store.lock().unwrap();
            let all = st.inscriptions_newest_first();
            let decs = st.decrees_newest_first();
            journal::export_record(&all, &decs, !en)
        };
        let path = Store::data_dir().join("inscriptions.md");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let ok = std::fs::write(&path, md).is_ok();
        if let Some(w) = exp_weak.upgrade() {
            let msg = if ok {
                if en {
                    format!("Exported to {}", path.display())
                } else {
                    format!("برون‌بری در {}", path.display())
                }
            } else if en {
                "Export failed.".to_string()
            } else {
                "برون‌بری انجام نشد.".to_string()
            };
            w.set_sync_status(SharedString::from(msg));
        }
    });

    let copy_app = Arc::clone(&app);
    let copy_weak = weak.clone();
    win.on_copy_inscriptions(move || {
        let en = copy_app.settings.lock().unwrap().lang == "en";
        let md = {
            let st = copy_app.store.lock().unwrap();
            let all = st.inscriptions_newest_first();
            let decs = st.decrees_newest_first();
            journal::export_record(&all, &decs, !en)
        };
        let ok = copy_to_clipboard(&md);
        if let Some(w) = copy_weak.upgrade() {
            let msg = if ok {
                if en {
                    "Copied".to_string()
                } else {
                    "رونوشت شد".to_string()
                }
            } else if en {
                "Not copied: no clipboard tool found.".to_string()
            } else {
                "رونوشت نشد: ابزار تخته‌گیره‌ای یافت نشد.".to_string()
            };
            w.set_sync_status(SharedString::from(msg));
        }
    });

    // ── decrees: the intention stated before a block ──
    //
    // The dialog is opened by `on_start_pause` and these three handlers decide
    // what happens next. All of them end by starting the clock, except dismiss,
    // which leaves it paused — the user changed their mind about working, and
    // silently starting a countdown they just backed out of is the worse error.
    {
        let dec_text_app = Arc::clone(&app);
        let dec_text_weak = weak.clone();
        win.on_decree_text_changed(move |text| {
            let t = text.to_string();
            let trimmed = t.trim();
            let count = trimmed.chars().count() as i32;
            let over = count > journal::decree::MAX_CHARS as i32;
            if let Some(w) = dec_text_weak.upgrade() {
                w.set_can_issue(!trimmed.is_empty() && !over);
                w.set_decree_count(SharedString::from(format!(
                    "{}{} / {}",
                    count,
                    if over { "  ⚠" } else { "" },
                    journal::decree::MAX_CHARS
                )));
            }
            let _ = &dec_text_app;
        });
    }

    // Issue: hold the text against the block that is about to start, then start
    // it. The record cannot be written yet — see `pending_decree` — so this only
    // parks the intention and lets the tick finish the job when the session ends.
    let issue_app = Arc::clone(&app);
    let issue_shared = Arc::clone(&shared);
    let issue_weak = weak.clone();
    win.on_issue_decree(move |text| {
        let raw = text.to_string();
        // Validate with the same function that will judge it at session end, so
        // a line the dialog accepted cannot be refused later by surprise.
        match journal::decree::validate_text(&raw) {
            Ok(clean) => {
                *issue_app.pending_decree.lock().unwrap() =
                    Some((clean, chrono::Utc::now().timestamp()));
                if let Some(w) = issue_weak.upgrade() {
                    w.set_decree_open(false);
                    w.set_decree_text(SharedString::from(""));
                    w.set_decree_error(SharedString::from(""));
                    w.set_can_issue(false);
                }
                run_action(TrayAction::Toggle, &issue_shared, &issue_app, &issue_weak);
            }
            Err(e) => {
                if let Some(w) = issue_weak.upgrade() {
                    w.set_decree_error(SharedString::from(format!("{e}")));
                }
            }
        }
    });

    // Start without: the block runs unspoken for. Any decree still parked is
    // cleared, so an intention written for an earlier block cannot silently
    // attach itself to this one.
    let dec_skip_app = Arc::clone(&app);
    let dec_skip_shared = Arc::clone(&shared);
    let dec_skip_weak = weak.clone();
    win.on_skip_decree(move || {
        *dec_skip_app.pending_decree.lock().unwrap() = None;
        if let Some(w) = dec_skip_weak.upgrade() {
            w.set_decree_open(false);
            w.set_decree_text(SharedString::from(""));
            w.set_decree_error(SharedString::from(""));
            w.set_can_issue(false);
            w.set_current_decree(SharedString::from(String::new()));
        }
        run_action(
            TrayAction::Toggle,
            &dec_skip_shared,
            &dec_skip_app,
            &dec_skip_weak,
        );
    });

    // Dismiss: back out entirely. The clock stays paused where it was.
    let dec_dismiss_weak = weak.clone();
    win.on_dismiss_decree(move || {
        if let Some(w) = dec_dismiss_weak.upgrade() {
            w.set_decree_open(false);
            w.set_decree_text(SharedString::from(""));
            w.set_decree_error(SharedString::from(""));
            w.set_can_issue(false);
        }
    });

    // ── rota ────────────────────────────────────────────────────────────────
    //
    // Planning a block writes it to today's rota. Nothing starts: the whole
    // point of planning ahead is that writing the day down and beginning it are
    // two separate acts, done at two separate times.
    let rota_add_app = Arc::clone(&app);
    let rota_add_weak = weak.clone();
    win.on_add_rota_entry(move |label, minutes, decree| {
        let day = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let minutes = (minutes.max(1) as u64).min(crate::state::rota::MAX_MINUTES);
        let mut st = rota_add_app.store.lock().unwrap();
        let res = st.add_rota_entry(&day, &label, None, &decree, minutes);
        drop(st);
        // A refused entry (blank label, a full day) leaves the field's text
        // alone rather than clearing it: what the user typed is the thing they
        // would have to type again.
        if res.is_ok() {
            if let Some(w) = rota_add_weak.upgrade() {
                w.set_rota_new_label(SharedString::from(""));
                w.set_rota_can_add(false);
            }
            rota_add_app.cloud.mark_dirty();
        }
        if let Some(w) = rota_add_weak.upgrade() {
            refresh_screens(&w, &rota_add_app);
        }
    });

    // The label field drives the Add button's enabled state. A predicate on the
    // trimmed text, matching what the validator will accept, so the button can
    // never be enabled for something the store then refuses.
    let rota_lbl_weak = weak.clone();
    win.on_rota_label_changed(move |text| {
        if let Some(w) = rota_lbl_weak.upgrade() {
            w.set_rota_can_add(!text.trim().is_empty());
        }
    });

    // Re-plan every leftover from earlier days onto today.
    //
    // One press, because offering them one at a time would mean a dialog per
    // stale entry before the user has had any coffee. Each is moved as a new
    // row and the original tombstoned, so the same leftover cannot be offered
    // again tomorrow either way.
    let carry_app = Arc::clone(&app);
    let carry_weak = weak.clone();
    win.on_carry_rota_entries(move || {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let mut st = carry_app.store.lock().unwrap();
        let ids: Vec<u64> = st.rota_carry_over(&today).iter().map(|e| e.id).collect();
        for id in ids {
            st.carry_rota_entry_forward(id, &today);
        }
        drop(st);
        carry_app.cloud.mark_dirty();
        if let Some(w) = carry_weak.upgrade() {
            refresh_screens(&w, &carry_app);
        }
    });

    // One stale entry, by id — the shape a future carry sheet would use.
    let carry1_app = Arc::clone(&app);
    let carry1_weak = weak.clone();
    win.on_carry_rota_entry(move |id| {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        {
            let mut st = carry1_app.store.lock().unwrap();
            st.carry_rota_entry_forward(id as u64, &today);
        }
        carry1_app.cloud.mark_dirty();
        if let Some(w) = carry1_weak.upgrade() {
            refresh_screens(&w, &carry1_app);
        }
    });

    // ── saved plans ─────────────────────────────────────────────────────────
    //
    // A plan is the rota's memory: one day's shape written down once, applied
    // to whatever day the user is standing in. Saving and applying are the two
    // halves, and both are one press — the whole point of a saved plan is that
    // re-deciding the day's shape costs nothing.

    // Save today's rota under the typed name.
    let plan_save_app = Arc::clone(&app);
    let plan_save_weak = weak.clone();
    win.on_save_plan(move |name| {
        let day = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let res = {
            let mut st = plan_save_app.store.lock().unwrap();
            st.save_plan(&name, &day)
        };
        if let Some(w) = plan_save_weak.upgrade() {
            match res {
                Ok(_) => {
                    w.set_plan_name(SharedString::from(""));
                    w.set_plan_can_save(false);
                    w.set_plan_error(SharedString::from(""));
                    plan_save_app.cloud.mark_dirty();
                }
                // The refusal is shown, not swallowed: the user pressed a
                // button and gets no visible result otherwise.
                Err(e) => w.set_plan_error(SharedString::from(e.to_string())),
            }
            refresh_screens(&w, &plan_save_app);
        }
    });

    // The name field drives the Save button, on the same trimmed-text rule the
    // validator uses, so the button is never enabled for something the store
    // will then refuse.
    let plan_lbl_weak = weak.clone();
    win.on_plan_name_changed(move |text| {
        if let Some(w) = plan_lbl_weak.upgrade() {
            w.set_plan_can_save(!text.trim().is_empty());
        }
    });

    // Write a plan's blocks onto today.
    //
    // Applying to *today* rather than to the day the calendar screen is
    // showing is deliberate: a saved shape is applied in the morning, to the
    // day being started. Applying it to a day being browsed would let one press
    // fill a Tuesday three weeks out with work that was meant for now.
    let plan_apply_app = Arc::clone(&app);
    let plan_apply_weak = weak.clone();
    win.on_apply_plan(move |id| {
        let day = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let res = {
            let mut st = plan_apply_app.store.lock().unwrap();
            st.apply_plan(id as u64, &day)
        };
        if let Some(w) = plan_apply_weak.upgrade() {
            match res {
                Ok(_) => {
                    w.set_plan_error(SharedString::from(""));
                    plan_apply_app.cloud.mark_dirty();
                }
                Err(e) => w.set_plan_error(SharedString::from(e.to_string())),
            }
            refresh_screens(&w, &plan_apply_app);
        }
    });

    // Take a plan off the list. Marked, never dropped: the tombstone is what
    // carries the deletion to the other device.
    let plan_rm_app = Arc::clone(&app);
    let plan_rm_weak = weak.clone();
    win.on_delete_plan(move |id| {
        {
            let mut st = plan_rm_app.store.lock().unwrap();
            st.remove_plan(id as u64);
        }
        plan_rm_app.cloud.mark_dirty();
        if let Some(w) = plan_rm_weak.upgrade() {
            w.set_plan_error(SharedString::from(""));
            refresh_screens(&w, &plan_rm_app);
        }
    });

    // Take a planned block off the rota.
    let rota_rm_app = Arc::clone(&app);
    let rota_rm_weak = weak.clone();
    win.on_remove_rota_entry(move |id| {
        {
            let mut st = rota_rm_app.store.lock().unwrap();
            st.remove_rota_entry(id as u64);
        }
        rota_rm_app.cloud.mark_dirty();
        if let Some(w) = rota_rm_weak.upgrade() {
            refresh_screens(&w, &rota_rm_app);
        }
    });

    // Start the block at the head of the queue.
    //
    // This is the rota's payoff, so it does everything a session start does and
    // three things more: it marks the entry taken up, adopts the entry's length
    // as the block's length, and parks the entry's decree as the pending one so
    // the block is governed by the intention that was written for it — without
    // asking the user the question again.
    //
    // It refuses to act on a running clock. Starting "next" while a block is in
    // flight would silently swap the session out from under the countdown the
    // user is watching.
    let next_app = Arc::clone(&app);
    let next_shared = Arc::clone(&shared);
    let next_weak = weak.clone();
    win.on_start_rota_next(move || {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        {
            let st = next_app.store.lock().unwrap();
            let Some(entry) = st.rota_next(&today) else {
                return;
            };
            start_planned_block(&entry, &next_app, &next_shared, &next_weak);
        }
    });

    // Start one specific entry.
    let one_app = Arc::clone(&app);
    let one_shared = Arc::clone(&shared);
    let one_weak = weak.clone();
    win.on_start_rota_entry(move |id| {
        let draft = {
            let st = one_app.store.lock().unwrap();
            st.data.rota.iter().find(|e| e.id == id as u64).cloned()
        };
        if let Some(entry) = draft {
            start_planned_block(&entry, &one_app, &one_shared, &one_weak);
        }
    });

    // ── calendar screen: pick a day, page the month, jump to today ──
    let pick_app = Arc::clone(&app);
    let pick_weak = weak.clone();
    win.on_pick_day(move |key| {
        let key = key.to_string();
        if parse_day_key(&key).is_none() {
            return;
        }
        *pick_app.selected_day.lock().unwrap() = key.clone();
        // Selecting a day also brings the grid to that day's month, so the
        // selection is never off-screen after a jump.
        if let Some((y, m, _)) = parse_day_key(&key) {
            *pick_app.shown_month.lock().unwrap() = (y, m);
        }
        if let Some(w) = pick_weak.upgrade() {
            refresh_day_view(&w, &pick_app);
        }
    });

    let shift_app = Arc::clone(&app);
    let shift_weak = weak.clone();
    win.on_shift_month(move |delta| {
        let (ty, tm, _) = calendar::today();
        let (cy, cm) = {
            let shown = *shift_app.shown_month.lock().unwrap();
            if shown == (0, 0) {
                (ty, tm)
            } else {
                shown
            }
        };
        let (ny, nm) = calendar::shift_month(cy, cm, delta);
        *shift_app.shown_month.lock().unwrap() = (ny, nm);
        if let Some(w) = shift_weak.upgrade() {
            refresh_day_view(&w, &shift_app);
        }
    });

    let goto_app = Arc::clone(&app);
    let goto_weak = weak.clone();
    win.on_goto_today(move || {
        let (ty, tm, td) = calendar::today();
        *goto_app.selected_day.lock().unwrap() = format!("{ty:04}-{tm:02}-{td:02}");
        *goto_app.shown_month.lock().unwrap() = (ty, tm);
        if let Some(w) = goto_weak.upgrade() {
            refresh_day_view(&w, &goto_app);
        }
    });

    // Add a task onto a specific day, which is the whole point of the calendar:
    // a task typed while looking at next Tuesday belongs to next Tuesday.
    let addon_app = Arc::clone(&app);
    let addon_weak = weak.clone();
    win.on_add_task_on(move |key| {
        let key = key.to_string();
        let Some(w) = addon_weak.upgrade() else {
            return;
        };
        let txt = w.get_new_task_text().to_string();
        if txt.trim().is_empty() {
            return;
        }
        if parse_day_key(&key).is_none() {
            return;
        }
        {
            let mut st = addon_app.store.lock().unwrap();
            st.add_task(txt.trim().to_string(), key, String::new());
        }
        w.set_new_task_text(SharedString::from(""));
        refresh_day_view(&w, &addon_app);
    });

    // ── per-task management: delete, rename, move between days ──
    let del_app = Arc::clone(&app);
    let del_weak = weak.clone();
    win.on_delete_task(move |id| {
        {
            let mut st = del_app.store.lock().unwrap();
            st.remove_task(id as u64);
        }
        del_app.cloud.mark_dirty();
        if let Some(w) = del_weak.upgrade() {
            refresh_screens(&w, &del_app);
            refresh_day_view(&w, &del_app);
        }
    });

    let ren_app = Arc::clone(&app);
    let ren_weak = weak.clone();
    win.on_rename_task(move |id, title| {
        {
            let mut st = ren_app.store.lock().unwrap();
            st.set_task_title(id as u64, title.to_string());
        }
        ren_app.cloud.mark_dirty();
        if let Some(w) = ren_weak.upgrade() {
            w.set_edit_text(SharedString::new());
            refresh_screens(&w, &ren_app);
            refresh_day_view(&w, &ren_app);
        }
    });

    // Nudging a task a day either way. The selection follows the task, so the
    // row does not vanish out from under the cursor when it lands on a day you
    // are not looking at.
    let mv_app = Arc::clone(&app);
    let mv_weak = weak.clone();
    win.on_move_task(move |id, delta| {
        let (ty, tm, td) = calendar::today();
        let today_key = format!("{ty:04}-{tm:02}-{td:02}");
        let landed = {
            let mut st = mv_app.store.lock().unwrap();
            st.shift_task_days(id as u64, delta, &today_key)
        };
        if landed.is_some() {
            mv_app.cloud.mark_dirty();
        }
        if let Some(w) = mv_weak.upgrade() {
            if let Some(next) = landed {
                *mv_app.selected_day.lock().unwrap() = next.clone();
                if let Some((y, m, _)) = parse_day_key(&next) {
                    *mv_app.shown_month.lock().unwrap() = (y, m);
                }
            }
            refresh_screens(&w, &mv_app);
            refresh_day_view(&w, &mv_app);
        }
    });

    // Step the selected day by one, from the timer screen's navigator.
    let step_app = Arc::clone(&app);
    let step_weak = weak.clone();
    win.on_step_day(move |delta| {
        let cur = {
            let sel = step_app.selected_day.lock().unwrap().clone();
            if sel.is_empty() {
                let (ty, tm, td) = calendar::today();
                format!("{ty:04}-{tm:02}-{td:02}")
            } else {
                sel
            }
        };
        let Some(next) = calendar::shift_day_key(&cur, delta) else {
            return;
        };
        *step_app.selected_day.lock().unwrap() = next.clone();
        if let Some((y, m, _)) = parse_day_key(&next) {
            *step_app.shown_month.lock().unwrap() = (y, m);
        }
        if let Some(w) = step_weak.upgrade() {
            refresh_day_view(&w, &step_app);
        }
    });

    // Toggle a task's done state from the new task rows.
    let toggle_app = Arc::clone(&app);
    let toggle_weak = weak.clone();
    win.on_toggle_task(move |id| {
        {
            let mut st = toggle_app.store.lock().unwrap();
            let current = st
                .data
                .tasks
                .iter()
                .find(|t| t.id == id as u64)
                .map(|t| t.done)
                .unwrap_or(false);
            st.set_done(id as u64, !current);
        }
        toggle_app.cloud.mark_dirty();
        if let Some(w) = toggle_weak.upgrade() {
            refresh_screens(&w, &toggle_app);
            // The calendar's dots and the day's list both depend on done state.
            refresh_day_view(&w, &toggle_app);
        }
    }); // Explicit theme selection from the Settings segmented control.
    let settheme_app = Arc::clone(&app);
    let settheme_weak = weak.clone();
    win.on_set_theme(move |pref| {
        let pref = pref.to_string();
        let (resolved, era) = {
            let mut s = settheme_app.settings.lock().unwrap();
            s.theme = pref.clone();
            s.save();
            (resolve_theme(&s.theme), s.era.clone())
        };
        settheme_app.cloud.mark_dirty();
        if let Some(w) = settheme_weak.upgrade() {
            w.set_theme_pref(SharedString::from(pref));
            w.set_resolved_theme(SharedString::from(resolved));
            apply_palette(&w, resolved, &era);
        }
    });

    // Heritage era selection from the Settings era picker.
    let era_app = Arc::clone(&app);
    let era_weak = weak.clone();
    win.on_set_era(move |era| {
        let era = era.to_string();
        let (resolved, label, blurb) = {
            let mut s = era_app.settings.lock().unwrap();
            s.era = era.clone();
            s.sanitize_mut();
            s.save();
            (resolve_theme(&s.theme), s.era_label(), s.era_blurb())
        };
        era_app.cloud.mark_dirty();
        if let Some(w) = era_weak.upgrade() {
            w.set_era(SharedString::from(era.clone()));
            w.set_era_label(SharedString::from(label));
            w.set_era_blurb(SharedString::from(blurb));
            apply_palette(&w, resolved, &era);
        }
    });

    // Refresh the day's events from Wikipedia, off the UI thread.
    let ref_weak = weak.clone();
    let ref_app = Arc::clone(&app);
    win.on_refresh_events(move || {
        let Some(w) = ref_weak.upgrade() else { return };
        let g = crate::ui::Strings::get(&w);
        w.set_refresh_status(g.get_refreshing());
        let app2 = Arc::clone(&ref_app);
        let weak2 = ref_weak.clone();
        std::thread::spawn(move || {
            // Refresh the day the user is looking at, not always today: the
            // button sits on a screen that can be showing any day.
            let (jy, jm, jd) = selected_day(&app2);
            let (_, gm, gd) = calendar::to_gregorian(jy, jm, jd); // Start from the embedded table so a partial refresh never loses
                                                                  // days it did not touch.
            let mut table: serde_json::Value =
                serde_json::from_str(include_str!("../../data/day_events.json"))
                    .unwrap_or_else(|_| serde_json::json!({"iran":{},"world_fa":{},"world_en":{}}));
            let result = refresh::refresh_today(&mut table, jm, jd, gm, gd);
            let status = match result {
                Ok(r) => {
                    let text = serde_json::to_string(&table).unwrap_or_default(); // Cache it, then swap the live table.
                    let _ = std::fs::write(refresh::cache_path(), &text);
                    *app2.events.lock().unwrap() = events::Events::from_str(&text);
                    *app2.calendar_day.lock().unwrap() = (0, 0, 0);
                    format!("{} / {}", r.iran_events, r.world_events)
                }
                Err(e) => format!("— {e}"),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak2.upgrade() {
                    w.set_refresh_status(SharedString::from(status.clone()));
                    refresh_calendar(&w, &app2, selected_day(&app2));
                }
            });
        });
    }); // Language switch. Every visible string lives in the Strings global, so
        // this only has to flip that and refresh the strings Rust owns.
    let lang_weak = weak.clone();
    let lang_app = Arc::clone(&app);
    let lang_clock = Arc::clone(&shared);
    win.on_set_lang(move |lang| {
        let lang = lang.to_string();
        let (label, blurb, theme_label) = {
            let mut s = lang_app.settings.lock().unwrap();
            s.lang = lang.clone();
            s.sanitize_mut();
            s.save();
            (s.era_label(), s.era_blurb(), s.theme_label())
        };
        lang_app.cloud.mark_dirty();
        if let Some(w) = lang_weak.upgrade() {
            let g = crate::ui::Strings::get(&w);
            g.set_lang(SharedString::from(lang.clone()));
            w.set_era_label(SharedString::from(label));
            w.set_era_blurb(SharedString::from(blurb));
            w.set_theme_label(SharedString::from(theme_label));
            // The clock produces the phase and state labels in Rust, so it
            // needs the new language as well. Without this "کار ژرف" and
            // "درنگ" stayed Persian while every label around them switched.
            lang_clock.set_lang(&lang);
            // The sync status line is Rust-owned too.
            w.set_sync_status(SharedString::from(if lang == "en" {
                "Cloud sync ready"
            } else {
                "همگام‌سازی ابری آماده است"
            }));
            // The calendar card is language-dependent; force a rebuild.
            *lang_app.calendar_day.lock().unwrap() = (0, 0, 0);
            refresh_calendar(&w, &lang_app, selected_day(&lang_app));
            refresh_screens(&w, &lang_app);
        }
    });

    // Chronicle view: open, close, and keep its rows in the active language.
    let chron_weak = weak.clone();
    let chron_app = Arc::clone(&app);
    win.on_open_chronicle(move || {
        let en = chron_app.settings.lock().unwrap().lang == "en";
        if let Some(w) = chron_weak.upgrade() {
            w.set_dynasties(ModelRc::new(VecModel::from(chronicle::rows(en))));
            w.set_show_view("chronicle".into());
        }
    });
    let chron_back = weak.clone();
    win.on_close_chronicle(move || {
        if let Some(w) = chron_back.upgrade() {
            w.set_show_view("main".into());
        }
    });

    // Copy the device code to the clipboard, so the user can paste it into
    // Google's page instead of typing eight characters by hand. The label
    // flips to "Copied" for a moment, then back.
    let copy_weak = weak.clone();
    win.on_copy_code(move |code| {
        let code = code.to_string();
        if code.is_empty() {
            return;
        }
        // Only claim success if the clipboard actually took the text. The old
        // version flipped the label to "Copied" before trying, so on a machine
        // with no clipboard tool it said "Copied" and pasted nothing — which is
        // exactly the kind of lie that wastes a user's afternoon.
        let copied = copy_to_clipboard(&code);
        if let Some(w) = copy_weak.upgrade() {
            w.set_code_copied(copied);
        }
    });

    // Open an external URL in the user's default browser (About / credits links).
    // Only http(s) is accepted, so a stray string can never launch a local binary.
    win.on_open_url(|url| {
        open_in_browser(url.as_ref());
    });

    // Frameless window drag: move the window with the pointer.
    let drag_weak = weak.clone();
    win.on_window_drag_start(move || {
        if let Some(w) = drag_weak.upgrade() {
            let x = w.get_drag_origin_x();
            let y = w.get_drag_origin_y();
            w.window()
                .set_position(slint::WindowPosition::Logical(slint::LogicalPosition::new(
                    x, y,
                )));
        }
    });
    // Motion on/off takes effect immediately: the global drives every
    // duration, so flipping it is all that is needed.
    let an_weak = weak.clone();
    win.on_set_animations(move |on| {
        if let Some(w) = an_weak.upgrade() {
            w.set_animations(on);
            crate::ui::Motion::get(&w).set_enabled(on);
        }
    });

    // Compact mode: swap the layout and resize the window to match.
    let cm_weak = weak.clone();
    win.on_set_compact_mode(move |on| {
        if let Some(w) = cm_weak.upgrade() {
            w.set_compact_mode(on);
            let (cw, ch) = if on { (320.0, 300.0) } else { (420.0, 680.0) };
            w.window()
                .set_size(slint::WindowSize::Logical(slint::LogicalSize::new(cw, ch)));
        }
    });

    // Sync now.
    let sync_app = Arc::clone(&app);
    let sync_weak = weak.clone();
    win.on_sync_now(move || {
        do_sync(&sync_weak, &sync_app);
    });

    // ── Google client id ──
    //
    // Storing the id re-evaluates availability immediately: the connect button
    // is disabled while `sync-available` is false, and a user who has just
    // pasted a valid id should not have to restart the app to use it.
    let cid_app = Arc::clone(&app);
    let cid_weak = weak.clone();
    win.on_set_client_id(move |id: SharedString| {
        let en = cid_app.settings.lock().unwrap().lang == "en";
        let id = id.trim().to_string();
        let status = match sync::save_client_id(&id) {
            Ok(()) => {
                let ok = gdrive::has_client_id();
                if let Some(w) = cid_weak.upgrade() {
                    w.set_sync_available(ok);
                }
                if ok {
                    if en {
                        "Client id saved. You can connect now.".to_string()
                    } else {
                        "شناسهٔ کلاینت ذخیره شد. حالا می‌توانید وصل شوید.".to_string()
                    }
                } else if en {
                    "That does not look like a client id.".to_string()
                } else {
                    "این شبیه شناسهٔ کلاینت نیست.".to_string()
                }
            }
            Err(e) => {
                if en {
                    format!("Could not save: {e}")
                } else {
                    format!("ذخیره نشد: {e}")
                }
            }
        };
        if let Some(w) = cid_weak.upgrade() {
            w.set_sync_status(SharedString::from(status));
        }
    });

    // ── Google sign-in (one click) ──
    //
    // Press the button, the browser opens on the account chooser, the user
    // picks their account, and the app is signed in. No codes to copy. The
    // loopback listener and token exchange run on a background thread, so the
    // UI never blocks and the window keeps repainting while the browser is up.
    let conn_app = Arc::clone(&app);
    let conn_weak = weak.clone();
    win.on_sync_connect(move || {
        let en = conn_app.settings.lock().unwrap().lang == "en";
        let weak2 = conn_weak.clone();
        let app2 = Arc::clone(&conn_app);

        if !gdrive::has_client_id() {
            if let Some(w) = weak2.upgrade() {
                w.set_sync_available(false);
                w.set_sync_status(SharedString::from(if en {
                    "This build has no Google client id.".to_string()
                } else {
                    "این نسخه شناسهٔ کلاینت گوگل ندارد.".to_string()
                }));
            }
            return;
        }

        if let Some(w) = conn_weak.upgrade() {
            w.set_sync_waiting(true);
            w.set_sync_status(SharedString::from(if en {
                "Finish signing in in your browser…".to_string()
            } else {
                "ورود را در مرورگر کامل کنید…".to_string()
            }));
        }

        std::thread::spawn(move || {
            let client_id = gdrive::client_id();
            // Two flows, one button. A Desktop-app client gets the one-click
            // loopback sign-in. A "TVs and Limited Input devices" client cannot
            // use a loopback redirect at all, so it falls back to the device
            // code — worse, but it works, and the user still only presses one
            // button. Which one applies is a property of the client id, so it
            // is detected rather than asked.
            let signed = if oauth::supports_loopback(&client_id) {
                oauth::sign_in(&client_id, en, open_in_browser)
            } else {
                device_flow_sign_in(&client_id, en, &weak2)
            };

            let mut tokens = match signed {
                Ok(s) => s.tokens,
                Err(e) => {
                    let msg = if en {
                        format!("Sign-in did not finish: {e}")
                    } else {
                        format!("ورود کامل نشد: {e}")
                    };
                    let w3 = weak2.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w3.upgrade() {
                            w.set_sync_waiting(false);
                            w.set_sync_status(SharedString::from(msg));
                        }
                    });
                    return;
                }
            };

            // Name the account, so the card can say *which* Google account is
            // connected — the only way to tell two of them apart.
            if let Ok(tok) = gdrive::access_token(&mut tokens) {
                tokens.email = gdrive::fetch_email(&tok).unwrap_or_default();
            }
            match sync::save_tokens(&tokens) {
                Ok(()) => sync_log("sign-in: tokens saved to sync.toml"),
                Err(e) => sync_log(&format!("sign-in: SAVING TOKENS FAILED: {e}")),
            }
            *app2.tokens.lock().unwrap() = tokens.clone();

            // Signing in *is* asking for sync. A second switch would be a
            // puzzle, not a choice.
            {
                let mut st = app2.settings.lock().unwrap();
                st.sync_enabled = true;
                st.save();
            }

            let email = tokens.email.clone();
            let w3 = weak2.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w3.upgrade() {
                    w.set_sync_waiting(false);
                    w.set_sync_device_code(SharedString::new());
                    w.set_sync_signed_in(true);
                    w.set_sync_auto_on(true);
                    w.set_sync_account(SharedString::from(email));
                    w.set_sync_status(SharedString::from(if en {
                        "Connected".to_string()
                    } else {
                        "متصل شد".to_string()
                    }));
                }
            });
            // First sync immediately, so something visibly arrives.
            sync_log("sign-in: running the first sync");
            do_sync(&weak2, &app2);
        });
    });

    let disc_app = Arc::clone(&app);
    let disc_weak = weak.clone();
    win.on_sync_disconnect(move || {
        let en = disc_app.settings.lock().unwrap().lang == "en";
        let _ = sync::clear_tokens();
        *disc_app.tokens.lock().unwrap() = gdrive::Tokens::default();
        {
            let mut st = disc_app.settings.lock().unwrap();
            st.sync_enabled = false;
            st.save();
        }
        if let Some(w) = disc_weak.upgrade() {
            w.set_sync_signed_in(false);
            w.set_sync_auto_on(false);
            w.set_sync_account(SharedString::new());
            w.set_sync_device_code(SharedString::new());
            w.set_sync_status(SharedString::from(if en {
                "Disconnected".to_string()
            } else {
                "قطع شد".to_string()
            }));
        }
    });

    let auto_app = Arc::clone(&app);
    win.on_set_sync_auto(move |on| {
        {
            let mut st = auto_app.settings.lock().unwrap();
            st.sync_enabled = on;
            st.save();
        }
        if on {
            auto_app.cloud.mark_dirty();
        }
    });

    // Cloud backup (push full settings+data snapshot to the backup sheet).
    let backup_app = Arc::clone(&app);
    let backup_weak = weak.clone();
    win.on_backup_now(move || {
        let en = backup_app.settings.lock().unwrap().lang == "en";
        let mut tokens = backup_app.tokens.lock().unwrap().clone();
        if tokens.is_empty() {
            if let Some(w) = backup_weak.upgrade() {
                w.set_sync_status(SharedString::from(if en {
                    "Connect Google first."
                } else {
                    "اول به گوگل وصل شوید."
                }));
            }
            return;
        }
        let s = backup_app.settings.lock().unwrap().clone();
        let d = backup_app.store.lock().unwrap().data.clone();
        let payload = sync::BackupPayload {
            app: "pangan".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            settings: toml::to_string_pretty(&s).unwrap_or_default(),
            data: serde_json::to_string_pretty(&d).unwrap_or_default(),
        };
        // The upload runs on a worker thread, not here.
        //
        // This used to block the UI thread for the whole Drive round trip —
        // up to the 20 s request timeout — and every 300 ms tick queued behind
        // it, so the countdown visibly stopped while a backup was in flight.
        // The countdown is the one thing this app cannot afford to freeze.
        let weak2 = backup_weak.clone();
        let tokens_arc = Arc::clone(&backup_app.tokens);
        let app2 = Arc::clone(&backup_app);
        std::thread::spawn(move || {
            let status = match sync::push_backup(&mut tokens, &payload) {
                Ok(()) => {
                    // Persist a rotated access token so the next call does not
                    // refresh again.
                    *tokens_arc.lock().unwrap() = tokens.clone();
                    let _ = sync::save_tokens(&tokens);
                    if en {
                        "Backup saved to Google Drive ✓".to_string()
                    } else {
                        "پشتیبان در گوگل درایو ذخیره شد ✓".to_string()
                    }
                }
                Err(e) => {
                    if en {
                        format!("Backup error: {e}")
                    } else {
                        format!("خطای پشتیبان‌گیری: {e}")
                    }
                }
            };
            *app2.last_sync.lock().unwrap() = status.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak2.upgrade() {
                    w.set_sync_status(SharedString::from(status));
                }
            });
        });
    });

    // Cloud restore (pull latest backup and apply it).
    let restore_app = Arc::clone(&app);
    let restore_weak = weak.clone();
    win.on_restore_now(move || {
        let en = restore_app.settings.lock().unwrap().lang == "en";
        let mut tokens = restore_app.tokens.lock().unwrap().clone();
        if tokens.is_empty() {
            if let Some(w) = restore_weak.upgrade() {
                w.set_sync_status(SharedString::from(if en {
                    "Connect Google first."
                } else {
                    "اول به گوگل وصل شوید."
                }));
            }
            return;
        }
        // The download runs on a worker thread, not on the UI thread, for the
        // same reason the backup upload does: a Drive round trip here froze the
        // countdown, and the restore is not urgent enough to justify that.
        let weak2 = restore_weak.clone();
        let tokens_arc = Arc::clone(&restore_app.tokens);
        let app2 = Arc::clone(&restore_app);
        std::thread::spawn(move || {
            let status = match sync::pull_backup(&mut tokens) {
                Ok(payload) => {
                    *tokens_arc.lock().unwrap() = tokens.clone();
                    let _ = sync::save_tokens(&tokens);
                    // Apply settings and data, then push the result to the UI
                    // from the event loop: Slint properties must not be set
                    // off the UI thread.
                    let mut restored_settings = None;
                    if let Ok(mut s) = toml::from_str::<Settings>(&payload.settings) {
                        s.sanitize_mut();
                        s.save();
                        *app2.settings.lock().unwrap() = s.clone();
                        *app2.sound_theme.lock().unwrap() = s.sound_theme.clone();
                        restored_settings = Some(s);
                    }
                    if let Ok(d) = serde_json::from_str::<store::Data>(&payload.data) {
                        let mut st = app2.store.lock().unwrap();
                        st.data = d;
                        st.save();
                    }
                    let ts = payload.ts.clone();
                    let msg = if en {
                        format!("Backup restored from {ts}")
                    } else {
                        format!("پشتیبان بازگردانده شد از {ts}")
                    };
                    let w3 = weak2.clone();
                    let app3 = Arc::clone(&app2);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = w3.upgrade() {
                            if let Some(s) = restored_settings {
                                w.set_sound_theme(SharedString::from(s.sound_theme.clone()));
                                w.set_sound_volume(s.sound_volume as i32);
                                w.set_compact_mode(s.compact_mode);
                                w.set_animations(s.animations);
                                crate::ui::Motion::get(&w).set_enabled(s.animations);
                                let (cw, ch) = if s.compact_mode {
                                    (320.0, 300.0)
                                } else {
                                    (420.0, 680.0)
                                };
                                w.window().set_size(slint::WindowSize::Logical(
                                    slint::LogicalSize::new(cw, ch),
                                ));
                                w.set_theme_pref(SharedString::from(s.theme.clone()));
                                w.set_theme_label(SharedString::from(s.theme_label()));
                                let resolved = resolve_theme(&s.theme);
                                w.set_resolved_theme(SharedString::from(resolved));
                                apply_palette(&w, resolved, &s.era);
                            }
                            refresh_screens(&w, &app3);
                        }
                    });
                    msg
                }
                Err(e) => {
                    if en {
                        format!("Restore error: {e}")
                    } else {
                        format!("خطای بازگردانی: {e}")
                    }
                }
            };
            *app2.last_sync.lock().unwrap() = status.clone();
            let w4 = weak2.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = w4.upgrade() {
                    w.set_sync_status(SharedString::from(status));
                }
            });
        });
    });

    // Automation: add a rule (event, command).
    let add_auto_app = Arc::clone(&app);
    let add_auto_weak = weak.clone();
    win.on_add_automation(move |event: SharedString, command: SharedString| {
        let cmd = command.trim().to_string();
        if cmd.is_empty() {
            return;
        }
        {
            let mut s = add_auto_app.settings.lock().unwrap();
            s.automation.push(settings::AutomationAction {
                event: event.to_string(),
                command: cmd,
                enabled: true,
            });
            s.save();
        }
        update_automation_summary(&add_auto_weak, &add_auto_app);
    });

    // Tray + hotkey.
    let tray_shared = Arc::clone(&shared);
    crate::platform::tray::spawn(tray_shared, tx.clone());

    let hk_shared = Arc::clone(&shared);
    std::thread::spawn(move || hotkey_thread(hk_shared, tx.clone()));

    apply_snapshot(&win, &shared);
    apply_session_dots(&win, &shared, &app);
    refresh_screens(&win, &app);
    refresh_day_view(&win, &app);
    update_automation_summary(&weak, &app);
    win.set_sync_status(SharedString::from(if settings.lang == "en" {
        "Cloud sync ready"
    } else {
        "همگام‌سازی ابری آماده است"
    }));
    win.show()?;

    slint::run_event_loop()?;
    Ok(())
}

fn hotkey_thread(shared: Arc<Shared>, tx: Sender<TrayAction>) {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};

    let Ok(manager) = GlobalHotKeyManager::new() else {
        return;
    };

    // Map accelerator string -> (modifiers, code, action).
    // Supported modifiers: <Super>, <Control>, <Alt> (case-insensitive).
    // Key is the trailing single key token.
    let accel_to_hotkey = |accel: &str| -> Option<HotKey> {
        let mut mods = Modifiers::empty();
        let mut key: Option<char> = None;
        for part in accel.split('<') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let lower = part.to_lowercase();
            if lower.starts_with("super") {
                mods |= Modifiers::SUPER;
            } else if lower.starts_with("control") || lower.starts_with("ctrl") {
                mods |= Modifiers::CONTROL;
            } else if lower.starts_with("alt") {
                mods |= Modifiers::ALT;
            } else if lower.starts_with("shift") {
                mods |= Modifiers::SHIFT;
            } else if let Some(c) = lower.chars().next() {
                if c.is_ascii_alphanumeric() {
                    key = Some(c);
                }
            }
        }
        let key = key?;
        let code = match key.to_ascii_lowercase() {
            'a' => Code::KeyA,
            'b' => Code::KeyB,
            'c' => Code::KeyC,
            'd' => Code::KeyD,
            'e' => Code::KeyE,
            'f' => Code::KeyF,
            'g' => Code::KeyG,
            'h' => Code::KeyH,
            'i' => Code::KeyI,
            'j' => Code::KeyJ,
            'k' => Code::KeyK,
            'l' => Code::KeyL,
            'm' => Code::KeyM,
            'n' => Code::KeyN,
            'o' => Code::KeyO,
            'p' => Code::KeyP,
            'q' => Code::KeyQ,
            'r' => Code::KeyR,
            's' => Code::KeyS,
            't' => Code::KeyT,
            'u' => Code::KeyU,
            'v' => Code::KeyV,
            'w' => Code::KeyW,
            'x' => Code::KeyX,
            'y' => Code::KeyY,
            'z' => Code::KeyZ,
            '0' => Code::Digit0,
            '1' => Code::Digit1,
            '2' => Code::Digit2,
            '3' => Code::Digit3,
            '4' => Code::Digit4,
            '5' => Code::Digit5,
            '6' => Code::Digit6,
            '7' => Code::Digit7,
            '8' => Code::Digit8,
            '9' => Code::Digit9,
            _ => return None,
        };
        Some(HotKey::new(Some(mods), code))
    };

    let settings = Settings::load();
    let mut registered: Vec<(HotKey, TrayAction)> = Vec::new();
    if settings.hotkey_enabled {
        let action_for = |name: &str| -> Option<TrayAction> {
            match name {
                "toggle" => Some(TrayAction::Toggle),
                "skip" => Some(TrayAction::Skip),
                "reset" => Some(TrayAction::Reset),
                "toggle_window" => Some(TrayAction::ToggleWindow),
                _ => None,
            }
        };
        for (name, accel) in &settings.shortcuts {
            if let (Some(action), Some(hk)) = (action_for(name), accel_to_hotkey(accel)) {
                if manager.register(hk).is_ok() {
                    registered.push((hk, action));
                }
            }
        }
    }

    loop {
        if shared.quit.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            for (hk, action) in &registered {
                if event.id() == hk.id() {
                    let _ = tx.send(*action);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
mod day_key_tests {
    use super::parse_day_key;
    #[test]
    fn a_well_formed_key_parses() {
        assert_eq!(parse_day_key("1405-08-07"), Some((1405, 8, 7)));
    }

    #[test]
    fn a_malformed_key_is_rejected_rather_than_guessed() {
        // Every one of these used to be reachable from the grid or a stored
        // task, and a wrong answer here would move the whole screen to a day
        // that does not exist.
        assert_eq!(parse_day_key(""), None);
        assert_eq!(parse_day_key("1405-08"), None);
        assert_eq!(parse_day_key("1405-08-07-01"), None);
        assert_eq!(parse_day_key("1405-13-01"), None, "month 13");
        assert_eq!(parse_day_key("1405-00-01"), None, "month 0");
        assert_eq!(parse_day_key("1405-08-00"), None, "day 0");
        assert_eq!(parse_day_key("1405-08-32"), None, "day 32");
        assert_eq!(parse_day_key("abcd-08-07"), None);
        assert_eq!(parse_day_key("1405/08/07"), None, "wrong separator");
    }

    #[test]
    fn a_key_round_trips_through_the_format_the_grid_emits() {
        // The grid builds keys with `{y:04}-{m:02}-{d:02}`; anything the grid
        // can produce must parse back to the same day.
        for (y, m, d) in [(1405, 1, 1), (1405, 12, 29), (1399, 7, 15)] {
            let key = format!("{y:04}-{m:02}-{d:02}");
            assert_eq!(parse_day_key(&key), Some((y, m, d)), "key {key}");
        }
    }
}

#[cfg(test)]
mod sync_detail_tests {
    use super::describe_last_sync;

    #[test]
    fn a_device_that_never_synced_says_nothing_rather_than_never() {
        // A fresh install is not a failure. Showing "never" would read as one.
        assert_eq!(describe_last_sync(0, 1_000_000, true), None);
        assert_eq!(describe_last_sync(-5, 1_000_000, false), None);
    }

    #[test]
    fn the_age_picks_the_right_unit() {
        let now = 1_000_000;
        assert!(describe_last_sync(now - 5, now, true)
            .unwrap()
            .contains("just now"));
        assert!(describe_last_sync(now - 300, now, true)
            .unwrap()
            .contains("5 min ago"));
        assert!(describe_last_sync(now - 7_200, now, true)
            .unwrap()
            .contains("2 h ago"));
        assert!(describe_last_sync(now - 172_800, now, true)
            .unwrap()
            .contains("2 d ago"));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_produce_a_negative_age() {
        // NTP corrections and timezone edits happen; "in -3 minutes" is worse
        // than saying "just now".
        let detail = describe_last_sync(1_000_100, 1_000_000, true).unwrap();
        assert!(detail.contains("just now"), "got {detail}");
    }

    #[test]
    fn the_persian_copy_is_persian() {
        let detail = describe_last_sync(1_000_000 - 300, 1_000_000, false).unwrap();
        assert!(detail.contains("دقیقه پیش"), "got {detail}");
        assert!(!detail.contains("min ago"), "got {detail}");
    }
}
