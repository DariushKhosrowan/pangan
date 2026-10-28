use std::sync::{
    atomic::Ordering,
    mpsc::{channel, Sender},
    Arc, Mutex,
};
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, SharedString, Timer, TimerMode};

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
}

/// User-facing app state shared with the UI update closures.
struct AppState {
    store: Mutex<Store>,
    settings: Mutex<Settings>,
    /// Active sound theme ("santur", "daf", ...).
    sound_theme: Mutex<String>,
    last_sync: Mutex<String>,
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
            last_sync: Mutex::new("Cloud sync ready".into()),
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

/// Recompute and set report + task text on the window.
fn refresh_screens(win: &PanganWindow, app: &AppState) {
    let st = app.store.lock().unwrap();
    let t = stats::report(&st.data.sessions, stats::Period::Today);
    let w = stats::report(&st.data.sessions, stats::Period::Week);
    let m = stats::report(&st.data.sessions, stats::Period::Month);
    let mut report = String::from("── TODAY ──\n");
    report.push_str(&stats::narrative(&t));
    report.push_str("\n\n── THIS WEEK ──\n");
    report.push_str(&stats::narrative(&w));
    report.push_str("\n\n── THIS MONTH ──\n");
    report.push_str(&stats::narrative(&m));
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
    tasks_line.push_str(&format!("Tasks: {} open ({} today) ", open, today_open));
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
    win.set_session_counter(SharedString::from(format!(
        "Session {} · Goal {} min",
        st.data.sessions.len() + 1,
        goal
    )));
    drop(st);
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
    s.sound_enabled = win.get_sound_enabled();
    s.sound_theme = win.get_sound_theme().to_string();
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
        win.set_sound_enabled(settings.sound_enabled);
        win.set_sound_theme(SharedString::from(settings.sound_theme.clone()));
        win.set_hotkey_enabled(settings.hotkey_enabled);
        win.set_inhibit_enabled(settings.inhibit_enabled);
        win.set_overlay_enabled(settings.overlay_enabled);
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
        if !recs.is_empty() {
            {
                let mut st = tick_app.store.lock().unwrap();
                st.add_sessions(&recs);
            }
            if tick_app.settings.lock().unwrap().sound_enabled {
                sound::play(&tick_sounds_work);
            }
        } else if breaks > 0 {
            if tick_app.settings.lock().unwrap().sound_enabled {
                sound::play(&tick_sounds_rest);
            }
        }

        if let Some(win) = tick_weak.upgrade() {
            apply_snapshot(&win, &tick_shared);
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
            w.set_resolved_theme(SharedString::from(resolve_theme(&s.theme)));
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
        let dir = store::Store::data_dir().join("sounds");
        let snd = sound::Sounds::ensure(&dir, &theme);
        sound::play(&snd.work);
        let _ = &preview_app;
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

    // Sync now.
    let sync_app = Arc::clone(&app);
    let sync_weak = weak.clone();
    win.on_sync_now(move || {
        do_sync(&sync_weak, &sync_app);
    });

    // Tray + hotkey.
    let tray_shared = Arc::clone(&shared);
    platform::tray::spawn(tray_shared, tx.clone());

    let hk_shared = Arc::clone(&shared);
    std::thread::spawn(move || hotkey_thread(hk_shared, tx.clone()));

    apply_snapshot(&win, &shared);
    refresh_screens(&win, &app);
    win.set_sync_status(SharedString::from("Cloud sync ready"));
    win.show()?;

    slint::run_event_loop()?;
    Ok(())
}

fn hotkey_thread(shared: Arc<Shared>, tx: Sender<TrayAction>) {
    use global_hotkey::{
        hotkey::{Code, HotKey, Modifiers},
        GlobalHotKeyEvent, GlobalHotKeyManager,
    };
    let Ok(manager) = GlobalHotKeyManager::new() else {
        return;
    };
    let hotkey = HotKey::new(Some(Modifiers::SUPER), Code::KeyP);
    let _ = manager.register(hotkey);
    loop {
        if shared.quit.load(Ordering::Relaxed) {
            break;
        }
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id() == hotkey.id() {
                let _ = tx.send(TrayAction::Toggle);
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}
