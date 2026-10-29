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

mod events;
mod calendar;
mod refresh;
mod automation;
mod clock;
mod platform;
mod settings;
mod sound;
mod stats;
mod store;
mod sync;

use clock::Shared;
use settings::Settings;
use store::Store;

mod ui {
    slint::include_modules!();
}
use ui::PanganWindow;

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
}

impl AppState {
    fn load() -> Self {
        let store = Store::load();
        let settings = Settings::load();
        let sound_theme = settings.sound_theme.clone();
        Self {
            store: Mutex::new(store),
            settings: Mutex::new(settings),
            sound_theme: Mutex::new(sound_theme),
            last_sync: Mutex::new("همگام‌سازی ابری آماده است".into()),
            events: Mutex::new(match refresh::load_cached() {
                Some(raw) => events::Events::from_str(&raw),
                None => events::Events::load(),
            }),
            calendar_day: Mutex::new((0, 0, 0)),
        }
    }

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
    // system -> try GTK color-scheme; default dark.
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.contains("dark") || s.contains("prefer-dark") {
            return "dark";
        }
        if s.contains("light") || s.contains("prefer-light") {
            return "light";
        }
    }
    "dark"
}

// ---------------------------------------------------------------------------
// UI glue
// ---------------------------------------------------------------------------

fn apply_snapshot(win: &PanganWindow, shared: &Shared) {
    let snap = shared.snapshot();
    win.set_time_left(SharedString::from(snap.time));
    win.set_session_name(SharedString::from(snap.session));
    win.set_progress(snap.progress);
    win.set_phase(SharedString::from(snap.phase));
    win.set_running(snap.running);
    win.set_work_done(snap.work_done as i32);
}

/// Push the resolved theme and heritage era into the Slint palette global.
fn apply_palette(win: &PanganWindow, resolved: &str, era: &str) {
    let p = ui::Palette::get(win);
    p.set_dark(resolved != "light");
    p.set_era(SharedString::from(era));
}

/// Session dots: total = long-break interval, done = position in the cycle.
fn apply_session_dots(win: &PanganWindow, shared: &Shared, app: &AppState) {
    let interval = app.settings.lock().unwrap().long_break_interval.max(1) as i32;
    let done = (shared.snapshot().work_done as i32) % interval;
    win.set_session_total(interval);
    win.set_session_done(done);
}

fn notify(body: &str) {
    platform::notify::send("Pangan", body);
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
            None
        }
        TrayAction::Quit => {
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

/// Build and push the Persian-calendar card: the Jalali date, the weekday, the
/// Sasanian day-name, the feast (if today is one) and the day's events.
///
/// Rebuilt only when the Jalali day changes, so the 300 ms UI tick does not
/// re-serialise a few hundred event rows twice a second.
fn refresh_calendar(win: &PanganWindow, app: &AppState) {
    let (jy, jm, jd) = calendar::today();
    let lang = app.settings.lock().unwrap().lang.clone();
    let en = lang == "en";
    {
        let mut last = app.calendar_day.lock().unwrap();
        if *last == (jy, jm, jd) {
            return;
        }
        *last = (jy, jm, jd);
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

    // The date line: Persian digits for Persian, plain digits for English.
    let date_line = if en {
        format!("{jd} {month_name} {jy}")
    } else {
        format!(
            "{} {} {}",
            calendar::fa_num(jd as i64),
            month_name,
            calendar::fa_num(jy as i64)
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
    win.set_cal_feast(SharedString::from(day.feast.unwrap_or("")));

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
    let to_ui = |rows: Vec<events::Row>, label: &'static str| -> Vec<ui::CalEvent> {
        rows.into_iter()
            .map(|r| ui::CalEvent {
                section: SharedString::from(label),
                year: SharedString::from(r.year),
                text: SharedString::from(r.text),
            })
            .collect()
    }; let (l_ev, l_birth, l_death, l_occ) = if en {
        ("Events", "Born", "Died", "Occasions")
    } else {
        ("رخداد", "زادروز", "درگذشت", "مناسبت")
    }; let mut iran_rows = to_ui(evs, l_ev); iran_rows.extend(to_ui(occasions, l_occ)); iran_rows.extend(to_ui(births, l_birth)); iran_rows.extend(to_ui(deaths, l_death)); win.set_cal_events(ModelRc::new(VecModel::from(iran_rows))); // ── World stream: keyed by the Gregorian day, shown after Iran's ──
    let (_, gm, gd) = calendar::to_gregorian(jy, jm, jd);
    let world = app.events.lock().unwrap().world(&lang, gm, gd);
    let world_label = if en { "World" } else { "جهان" };
    win.set_cal_world(ModelRc::new(VecModel::from(to_ui(world, world_label))));
}

/// Recompute and set report + task text on the window.
fn refresh_screens(win: &PanganWindow, app: &AppState) {
    let st = app.store.lock().unwrap();
    let t = stats::report(&st.data.sessions, stats::Period::Today);
    let w = stats::report(&st.data.sessions, stats::Period::Week);
    let m = stats::report(&st.data.sessions, stats::Period::Month);
    let en = app.settings.lock().unwrap().lang == "en";
    let mut report = String::from(if en { "── TODAY ──\n" } else { "── امروز ──\n" });
    report.push_str(&stats::narrative(&t, en));
    report.push_str(if en { "\n\n── THIS WEEK ──\n" } else { "\n\n── این هفته ──\n" });
    report.push_str(&stats::narrative(&w, en));
    report.push_str(if en { "\n\n── THIS MONTH ──\n" } else { "\n\n── این ماه ──\n" });
    report.push_str(&stats::narrative(&m, en));
    win.set_report_text(SharedString::from(report));

    let today = chrono::Local::now().date_naive();
    let mut tasks_line = String::new();
    let open = st.data.tasks.iter().filter(|t| !t.done).count();
    let today_open = st
        .data
        .tasks
        .iter()
        .filter(|t| !t.done && t.date == today.format("%Y-%m-%d").to_string())
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
        format!(
            "نشست {} · هدف {} دقیقه",
            st.data.sessions.len() + 1,
            goal
        )
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

    // Weekly bars: Monday..Sunday of the current week, normalised to the peak.
    let (week_start, _) = stats::Period::Week.bounds(today);
    let mut per_day = [0u64; 7];
    for s in &st.data.sessions {
        let d = chrono::DateTime::from_timestamp(s.at, 0)
            .map(|d| d.with_timezone(&chrono::Local).date_naive());
        if let Some(d) = d {
            let idx = (d - week_start).num_days();
            if (0..7).contains(&idx) {
                per_day[idx as usize] += s.minutes;
            }
        }
    }
    let peak = per_day.iter().copied().max().unwrap_or(0).max(1);
    let bars: Vec<f32> = per_day
        .iter()
        .map(|m| (*m as f32 / peak as f32).clamp(0.0, 1.0))
        .collect();
    win.set_week_bars(ModelRc::new(VecModel::from(bars)));

    let labels: Vec<SharedString> = ["M", "T", "W", "T", "F", "S", "S"]
        .iter()
        .map(|s| SharedString::from(*s))
        .collect();
    win.set_week_labels(ModelRc::new(VecModel::from(labels)));

    // Task list for the redesigned panel: open tasks first, then done.
    let mut items: Vec<ui::TaskItem> = st
        .data
        .tasks
        .iter()
        .map(|t| ui::TaskItem {
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
    refresh_calendar(win, app);
}

/// Recompute the automation summary line and push it to the window.
fn update_automation_summary(win: &slint::Weak<PanganWindow>, app: &AppState) {
    let automation = automation::sorted(&app.settings.lock().unwrap());
    let summary = if automation.is_empty() {
        "هنوز آیینی ساخته نشده است.".to_string()
    } else {
        let mut s = String::from(format!(
            "{} آیین: ",
            calendar::fa_num(automation.len() as i64)
        ));
        for a in &automation {
            s.push_str(&format!("[{}] {}, ", a.event, a.command));
        }
        s
    };
    if let Some(w) = win.upgrade() {
        w.set_automation_summary(SharedString::from(summary));
    }
}

fn sync_row(app: &AppState) -> sync::SyncRow {
    let st = app.store.lock().unwrap();
    let t = stats::report(&st.data.sessions, stats::Period::Today);
    let open = st.data.tasks.iter().filter(|t| !t.done).count();
    let row = sync::SyncRow {
        ts: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        period: "Today".into(),
        sessions: t.sessions,
        minutes: t.minutes,
        avg_min: t.avg,
        consistency_pct: (t.consistency * 100.0) as u32,
        streak_days: t.streak,
        score: t.score,
        tasks_open: open as u64,
    };
    row
}

/// Perform a sync (used by the button and the auto-sync timer).
fn do_sync(win: &slint::Weak<PanganWindow>, app: &AppState) {
    let row = sync_row(app);
    let status = sync::try_sync(&row);
    {
        let mut ls = app.last_sync.lock().unwrap();
        *ls = status.clone();
    }
    if let Some(w) = win.upgrade() {
        w.set_sync_status(SharedString::from(status));
    }
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
    s
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    platform::autostart::ensure();

    let settings = Settings::load();
    let cfg = clock::Config::from(&settings);
    let shared = Arc::new(Shared::new(cfg));
    let app = Arc::new(AppState::load());

    // Sounds: warm all themes, then pick the active one.
    let sounds_dir = store::Store::data_dir().join("sounds");
    sound::Sounds::warm_all(&sounds_dir);
    let active_theme = settings.sound_theme.clone();
    let sounds = sound::Sounds::ensure(&sounds_dir, &active_theme);

    let win = PanganWindow::new()?;
    let weak = win.as_weak();

    let (tx, rx) = channel::<TrayAction>();

    let inhibit = Arc::new(Mutex::new(None::<platform::inhibit::InhibitGuard>));

    // Initial theme application.
    {
        let resolved = resolve_theme(&settings.theme);
        win.set_resolved_theme(SharedString::from(resolved));
        win.set_theme_label(SharedString::from(settings.theme_label()));
        win.set_theme_pref(SharedString::from(settings.theme.clone()));
        apply_palette(&win, resolved, &settings.era);
    }

    // Push settings into the UI.
    {
        win.set_work_minutes(settings.work_minutes as i32);
        win.set_short_break_minutes(settings.short_break_minutes as i32);
        win.set_long_break_minutes(settings.long_break_minutes as i32);
        win.set_long_break_interval(settings.long_break_interval as i32);
        win.set_auto_start_next(settings.auto_start_next);
        win.set_auto_start_on_launch(settings.auto_start_on_launch);
        win.set_daily_goal_minutes(settings.daily_goal_minutes as i32);
        win.set_compact_mode(settings.compact_mode);
        win.set_sound_enabled(settings.sound_enabled);
        win.set_sound_theme(SharedString::from(settings.sound_theme.clone()));
        win.set_sound_volume(settings.sound_volume as i32);
        win.set_hotkey_enabled(settings.hotkey_enabled);
        win.set_inhibit_enabled(settings.inhibit_enabled);
        win.set_overlay_enabled(settings.overlay_enabled);
        win.set_era(SharedString::from(settings.era.clone()));
        win.set_era_label(SharedString::from(settings.era_label()));
        win.set_era_blurb(SharedString::from(settings.era_blurb()));
        // Language lives in the Strings global; set it once at startup.
        ui::Strings::get(&win).set_lang(SharedString::from(settings.lang.clone()));
    }

    // UI tick: countdown + inhibit + session persistence + auto-sync.
    let tick_shared = Arc::clone(&shared);
    let tick_weak = weak.clone();
    let tick_inhibit = Arc::clone(&inhibit);
    let tick_app = Arc::clone(&app);
    let tick_sounds_work = sounds.work.clone();
    let tick_sounds_rest = sounds.rest.clone();
    let mut last_auto_sync = chrono::Local::now();

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
            }
            if tick_app.settings.lock().unwrap().sound_enabled {
                sound::play(&tick_sounds_work, volume);
            }
        } else if breaks > 0 {
            if tick_app.settings.lock().unwrap().sound_enabled {
                sound::play(&tick_sounds_rest, volume);
            }
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
                *hold = platform::inhibit::acquire();
            } else if !inhibit_on || !tick_shared.should_inhibit() {
                *hold = None;
            }
        }

        // Auto-sync every 10 minutes if a token is configured.
        if chrono::Local::now() - last_auto_sync > chrono::Duration::minutes(10) {
            last_auto_sync = chrono::Local::now();
            if sync::has_config() {
                do_sync(&tick_weak, &tick_app);
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

    // Today (Persian calendar) open/close. Opening rebuilds the card so the
    // date is right even if the app has been left running across midnight.
    let today_open_weak = weak.clone();
    let today_open_app = Arc::clone(&app);
    win.on_open_today(move || {
        if let Some(w) = today_open_weak.upgrade() {
            *today_open_app.calendar_day.lock().unwrap() = (0, 0, 0);
            refresh_calendar(&w, &today_open_app);
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
            // Rebase the timer on the new durations.
            let s = set_close_app.settings.lock().unwrap().clone();
            let cfg = clock::Config::from(&s);
            // The clock is shared; rebuild its totals from the new config.
            w.set_theme_label(SharedString::from(s.theme_label()));
            let resolved = resolve_theme(&s.theme);
            w.set_resolved_theme(SharedString::from(resolved));
            apply_palette(&w, resolved, &s.era);
            w.set_show_view("main".into());
            let _ = cfg;
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
        s.save();
        let resolved = resolve_theme(&s.theme);
        if let Some(w) = theme_weak.upgrade() {
            w.set_theme_label(SharedString::from(label));
            w.set_theme_pref(SharedString::from(s.theme.clone()));
            w.set_resolved_theme(SharedString::from(resolved));
            apply_palette(&w, resolved, &s.era);
        }
    });

    // Sound toggle (header button).
    let sound_app = Arc::clone(&app);
    win.on_toggle_sound(move || {
        let mut s = sound_app.settings.lock().unwrap();
        s.sound_enabled = !s.sound_enabled;
        s.save();
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
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        {
            let mut st = task_app.store.lock().unwrap();
            st.add_task(txt.trim().to_string(), today, String::new());
        }
        if let Some(w) = task_weak.upgrade() {
            w.set_new_task_text(SharedString::from(""));
            refresh_screens(&w, &task_app);
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
        if let Some(w) = toggle_weak.upgrade() {
            refresh_screens(&w, &toggle_app);
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
            (
                resolve_theme(&s.theme),
                s.era_label(),
                s.era_blurb(),
            )
        };
        if let Some(w) = era_weak.upgrade() {
            w.set_era(SharedString::from(era.clone()));
            w.set_era_label(SharedString::from(label));
            w.set_era_blurb(SharedString::from(blurb));
            apply_palette(&w, resolved, &era);
        }
    });

    // Refresh the day's events from Wikipedia, off the UI thread.
    let ref_weak = weak.clone(); let ref_app = Arc::clone(&app); win.on_refresh_events(move || {
        let Some(w) = ref_weak.upgrade() else { return }; let g = ui::Strings::get(&w); w.set_refresh_status(SharedString::from(g.get_refreshing())); let app2 = Arc::clone(&ref_app); let weak2 = ref_weak.clone(); std::thread::spawn(move || {
            let (jy, jm, jd) = calendar::today(); let (_, gm, gd) = calendar::to_gregorian(jy, jm, jd); // Start from the embedded table so a partial refresh never loses
            // days it did not touch.
            let mut table: serde_json::Value =
                serde_json::from_str(include_str!("../data/day_events.json"))
                    .unwrap_or_else(|_| serde_json::json!({"iran":{},"world_fa":{},"world_en":{}})); let result = refresh::refresh_today(&mut table, jm, jd, gm, gd); let status = match result {
                Ok(r) => {
                    let text = serde_json::to_string(&table).unwrap_or_default(); // Cache it, then swap the live table.
                    let _ = std::fs::write(refresh::cache_path(), &text); *app2.events.lock().unwrap() = events::Events::from_str(&text); *app2.calendar_day.lock().unwrap() = (0, 0, 0); format!("{} / {}", r.iran_events, r.world_events)
                }
                Err(e) => format!("— {e}"),
            }; let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = weak2.upgrade() {
                    w.set_refresh_status(SharedString::from(status.clone())); refresh_calendar(&w, &app2); }
            }); }); }); // Language switch. Every visible string lives in the Strings global, so
    // this only has to flip that and refresh the strings Rust owns.
    let lang_weak = weak.clone();
    let lang_app = Arc::clone(&app);
    win.on_set_lang(move |lang| {
        let lang = lang.to_string();
        let (label, blurb, theme_label) = {
            let mut s = lang_app.settings.lock().unwrap();
            s.lang = lang.clone();
            s.sanitize_mut();
            s.save();
            (s.era_label(), s.era_blurb(), s.theme_label())
        };
        if let Some(w) = lang_weak.upgrade() {
            let g = ui::Strings::get(&w);
            g.set_lang(SharedString::from(lang.clone()));
            w.set_era_label(SharedString::from(label));
            w.set_era_blurb(SharedString::from(blurb));
            w.set_theme_label(SharedString::from(theme_label));
            // The calendar card is language-dependent; force a rebuild.
            *lang_app.calendar_day.lock().unwrap() = (0, 0, 0);
            refresh_calendar(&w, &lang_app);
            refresh_screens(&w, &lang_app);
        }
    });

    // Open an external URL in the user's default browser (About / credits links).
    // Only http(s) is accepted, so a stray string can never launch a local binary.
    win.on_open_url(|url| {
        let url = url.to_string();
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
            .arg(&url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });

    // Frameless window drag: move the window with the pointer.
    let drag_weak = weak.clone();
    win.on_window_drag_start(move || {
        if let Some(w) = drag_weak.upgrade() {
            let x = w.get_drag_origin_x();
            let y = w.get_drag_origin_y();
            let _ = w.window().set_position(slint::WindowPosition::Logical(
                slint::LogicalPosition::new(x, y),
            ));
        }
    }); // Sync now.
    let sync_app = Arc::clone(&app);
    let sync_weak = weak.clone();
    win.on_sync_now(move || {
        do_sync(&sync_weak, &sync_app);
    });

    // Cloud backup (push full settings+data snapshot to the backup sheet).
    let backup_app = Arc::clone(&app);
    let backup_weak = weak.clone();
    win.on_backup_now(move || {
        let Some(cfg) = sync::load_config() else {
            if let Some(w) = backup_weak.upgrade() {
                w.set_sync_status(SharedString::from("پشتیبان‌گیری خاموش است (نشانه‌ای نیست)."));
            }
            return;
        };
        let s = backup_app.settings.lock().unwrap().clone();
        let d = backup_app.store.lock().unwrap().data.clone();
        let payload = sync::BackupPayload {
            app: "pangan".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            ts: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            settings: toml::to_string_pretty(&s).unwrap_or_default(),
            data: serde_json::to_string_pretty(&d).unwrap_or_default(),
        };
        let status = match sync::push_backup(&cfg, &payload) {
            Ok(()) => "پشتیبان در برگه‌های گوگل ذخیره شد ✓".into(),
            Err(e) => format!("خطای پشتیبان‌گیری: {e}"),
        };
        {
            let mut ls = backup_app.last_sync.lock().unwrap();
            *ls = status.clone();
        }
        if let Some(w) = backup_weak.upgrade() {
            w.set_sync_status(SharedString::from(status));
        }
    });

    // Cloud restore (pull latest backup and apply it).
    let restore_app = Arc::clone(&app);
    let restore_weak = weak.clone();
    win.on_restore_now(move || {
        let Some(cfg) = sync::load_config() else {
            if let Some(w) = restore_weak.upgrade() {
                w.set_sync_status(SharedString::from("بازگردانی خاموش است (نشانه‌ای نیست)."));
            }
            return;
        };
        let status = match sync::pull_latest_backup(&cfg) {
            Ok(payload) => {
                // Apply settings.
                if let Ok(s) = toml::from_str::<Settings>(&payload.settings) {
                    let mut s = s;
                    s.sanitize_mut();
                    s.save();
                    *restore_app.settings.lock().unwrap() = s.clone();
                    *restore_app.sound_theme.lock().unwrap() = s.sound_theme.clone();
                    if let Some(w) = restore_weak.upgrade() {
                        w.set_sound_theme(SharedString::from(s.sound_theme.clone()));
                        w.set_sound_volume(s.sound_volume as i32);
                        w.set_compact_mode(s.compact_mode);
                        w.set_theme_pref(SharedString::from(s.theme.clone()));
                        w.set_theme_label(SharedString::from(s.theme_label()));
                        let resolved = resolve_theme(&s.theme);
                        w.set_resolved_theme(SharedString::from(resolved));
                        apply_palette(&w, resolved, &s.era);
                    }
                }
                // Apply data.
                if let Ok(d) = serde_json::from_str::<store::Data>(&payload.data) {
                    let mut st = restore_app.store.lock().unwrap();
                    st.data = d;
                    st.save();
                }
                format!("پشتیبان بازگردانده شد از {}", payload.ts)
            }
            Err(e) => format!("خطای بازگردانی: {e}"),
        };
        {
            let mut ls = restore_app.last_sync.lock().unwrap();
            *ls = status.clone();
        }
        if let Some(w) = restore_weak.upgrade() {
            w.set_sync_status(SharedString::from(status));
            refresh_screens(&w, &restore_app);
        }
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
    platform::tray::spawn(tray_shared, tx.clone());

    let hk_shared = Arc::clone(&shared);
    std::thread::spawn(move || hotkey_thread(hk_shared, tx.clone()));

    apply_snapshot(&win, &shared);
    apply_session_dots(&win, &shared, &app);
    refresh_screens(&win, &app);
    update_automation_summary(&weak, &app);
    win.set_sync_status(SharedString::from("همگام‌سازی ابری آماده است"));
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
