use std::sync::{
    atomic::Ordering,
    mpsc::{channel, Sender},
    Arc, Mutex,
};
use std::time::Duration;

use slint::{CloseRequestResponse, ComponentHandle, SharedString, Timer, TimerMode};

mod clock;
mod platform;
mod sound;
mod stats;
mod store;
mod sync;

use clock::Shared;
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

/// User-facing "app state" group shared with the UI update closures.
struct AppState {
    store: Mutex<Store>,
    theme_pref: Mutex<String>,
    sound_enabled: Mutex<bool>,
    last_sync: Mutex<String>,
}

impl AppState {
    fn load() -> Self {
        let store = Store::load();
        let (theme_pref, sound_enabled) = Self::load_settings();
        Self {
            store: Mutex::new(store),
            theme_pref: Mutex::new(theme_pref),
            sound_enabled: Mutex::new(sound_enabled),
            last_sync: Mutex::new("Google sync ready".into()),
        }
    }

    fn settings_path() -> std::path::PathBuf {
        store::Store::data_dir().join("settings.toml")
    }

    fn load_settings() -> (String, bool) {
        let path = Self::settings_path();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(v) = text.parse::<toml::Value>() {
                let theme = v
                    .get("theme")
                    .and_then(|x| x.as_str())
                    .unwrap_or("system")
                    .to_string();
                let sound = v
                    .get("sound")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(true);
                return (theme, sound);
            }
        }
        ("system".into(), true)
    }

    fn save_settings(&self) {
        let theme = self.theme_pref.lock().unwrap().clone();
        let sound = *self.sound_enabled.lock().unwrap();
        let body = format!(
            "# Pangan user settings\ntheme = {:?}\nsound = {}\n",
            theme, sound
        );
        if let Some(dir) = Self::settings_path().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(Self::settings_path(), body);
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
}

fn notify(body: &str) {
    platform::notify::send("Pangan", body);
}

/// Handle an action. `notify_msg` (if any) is shown from the event-loop thread.
fn run_action(action: TrayAction, shared: &Arc<Shared>, win: &slint::Weak<PanganWindow>) {
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
    let win = win.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(win) = win.upgrade() {
            apply_snapshot(&win, &shared);
        }
        if let Some(text) = notify_msg {
            notify(&text);
        }
    });
}

/// Recompute and set report + task text on the window.
fn refresh_screens(win: &PanganWindow, app: &AppState) {
    let st = app.store.lock().unwrap();
    // Report: combine all three periods.
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

    // Daily strip for the current week (Mon→today).
    let today = chrono::Local::now().date_naive();
    let (_ws, _we) = stats::Period::Week.bounds(today);
    let mut tasks_line = String::new();
    let open = st.data.tasks.iter().filter(|t| !t.done).count();
    let today_open = st
        .data
        .tasks
        .iter()
        .filter(|t| !t.done && t.date == today.format("%Y-%m-%d").to_string())
        .count();
    tasks_line.push_str(&format!(
        "Tasks: {} open ({} today) ",
        open, today_open
    ));
    // list today's open tasks
    for task in st.data.tasks.iter().filter(|t| !t.done && t.date == today.format("%Y-%m-%d").to_string()) {
        tasks_line.push_str(&format!("• {}", task.title));
        if !task.time.is_empty() {
            tasks_line.push_str(&format!(" @{}", task.time));
        }
        tasks_line.push(' ');
    }
    win.set_status_line(SharedString::from(tasks_line));
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

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    platform::autostart::ensure();

    let cfg = clock::load_config();
    let shared = Arc::new(Shared::new(cfg));
    let app = Arc::new(AppState::load());

    // Sounds (generate cues if missing).
    let sounds = sound::Sounds::ensure(&store::Store::data_dir().join("sounds"));

    let win = PanganWindow::new()?;
    let weak = win.as_weak();

    let (tx, rx) = channel::<TrayAction>();

    let inhibit = Arc::new(Mutex::new(None::<platform::inhibit::InhibitGuard>));

    // Initial theme application.
    let pref = {
        let p = app.theme_pref.lock().unwrap().clone();
        p
    };
    {
        let label = match pref.as_str() {
            "dark" => "Dark",
            "light" => "Light",
            _ => "System",
        };
        win.set_theme_label(SharedString::from(label));
        win.set_resolved_theme(SharedString::from(resolve_theme(&pref)));
    }

    // UI tick: countdown + inhibit + session persistence.
    let tick_shared = Arc::clone(&shared);
    let tick_weak = weak.clone();
    let tick_inhibit = Arc::clone(&inhibit);
    let tick_app = Arc::clone(&app);
    let tick_sounds_work = sounds.work.clone();
    let tick_sounds_rest = sounds.rest.clone();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_millis(300), move || {
        let notify_msg = tick_shared.tick();

        // Persist completed sessions and play the matching chime.
        let recs = tick_shared.drain_sessions();
        let breaks = tick_shared.take_breaks();
        if !recs.is_empty() {
            let mut st = tick_app.store.lock().unwrap();
            st.add_sessions(&recs);
            drop(st);
            if *tick_app.sound_enabled.lock().unwrap() {
                sound::play(&tick_sounds_work);
            }
        } else if breaks > 0 {
            if *tick_app.sound_enabled.lock().unwrap() {
                sound::play(&tick_sounds_rest);
            }
        }

        if let Some(win) = tick_weak.upgrade() {
            apply_snapshot(&win, &tick_shared);
            refresh_screens(&win, &tick_app);
        }

        let mut hold = tick_inhibit.lock().unwrap();
        if tick_shared.should_inhibit() && hold.is_none() {
            *hold = platform::inhibit::acquire();
        } else if !tick_shared.should_inhibit() {
            *hold = None;
        }
        drop(hold);

        if let Some(text) = notify_msg {
            notify(&text);
        }
    });

    // Poll tray/hotkey events.
    let poll_shared = Arc::clone(&shared);
    let poll_weak = weak.clone();
    let poll_timer = Timer::default();
    poll_timer.start(TimerMode::Repeated, Duration::from_millis(80), move || {
        while let Ok(action) = rx.try_recv() {
            run_action(action, &poll_shared, &poll_weak);
            if action == TrayAction::Quit {
                break;
            }
        }
    });

    // Window close -> hide to tray.
    let close_shared = Arc::clone(&shared);
    let close_weak = weak.clone();
    win.window().on_close_requested(move || {
        if let Some(win) = close_weak.upgrade() {
            let _ = win.hide();
        }
        let _ = close_shared;
        CloseRequestResponse::KeepWindowShown
    });

    // UI callbacks.
    let toggle_shared = Arc::clone(&shared);
    let toggle_weak = weak.clone();
    win.on_start_pause(move || {
        run_action(TrayAction::Toggle, &toggle_shared, &toggle_weak);
    });

    let skip_shared = Arc::clone(&shared);
    let skip_weak = weak.clone();
    win.on_skip(move || {
        run_action(TrayAction::Skip, &skip_shared, &skip_weak);
    });

    let reset_shared = Arc::clone(&shared);
    let reset_weak = weak.clone();
    win.on_reset(move || {
        run_action(TrayAction::Reset, &reset_shared, &reset_weak);
    });

    // Report open/close.
    let open_weak = weak.clone();
    win.on_open_report(move || {
        if let Some(w) = open_weak.upgrade() {
            w.set_show_report(true);
        }
    });
    let close_weak = weak.clone();
    win.on_close_report(move || {
        if let Some(w) = close_weak.upgrade() {
            w.set_show_report(false);
        }
    });

    // Theme switching.
    let theme_app = Arc::clone(&app);
    let theme_weak = weak.clone();
    win.on_cycle_theme(move || {
        {
            let mut p = theme_app.theme_pref.lock().unwrap();
            *p = match p.as_str() {
                "system" => "dark".to_string(),
                "dark" => "light".to_string(),
                _ => "system".to_string(),
            };
        }
        theme_app.save_settings();
        let p = theme_app.theme_pref.lock().unwrap().clone();
        let label = match p.as_str() {
            "dark" => "Dark",
            "light" => "Light",
            _ => "System",
        };
        if let Some(w) = theme_weak.upgrade() {
            w.set_theme_label(SharedString::from(label));
            w.set_resolved_theme(SharedString::from(resolve_theme(&p)));
        }
    });

    // Sound toggle.
    let sound_app = Arc::clone(&app);
    win.on_toggle_sound(move || {
        {
            let mut s = sound_app.sound_enabled.lock().unwrap();
            *s = !*s;
        }
        sound_app.save_settings();
    });

    // Add task (from the quick input).
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
        let row = sync_row(&sync_app);
        let status = sync::try_sync(&row);
        {
            let mut ls = sync_app.last_sync.lock().unwrap();
            *ls = status.clone();
        }
        if let Some(w) = sync_weak.upgrade() {
            w.set_sync_status(SharedString::from(status));
        }
    });

    // Tray + hotkey.
    let tray_shared = Arc::clone(&shared);
    platform::tray::spawn(tray_shared, tx.clone());

    let hk_shared = Arc::clone(&shared);
    std::thread::spawn(move || hotkey_thread(hk_shared, tx.clone()));

    // Build task text once at load.
    {
        let mut st = app.store.lock().unwrap();
        if st.data.next_task_id == 0 {
            st.data.next_task_id = 0;
        }
        drop(st);
    }

    apply_snapshot(&win, &shared);
    refresh_screens(&win, &app);
    win.set_sync_status(SharedString::from("Google sync ready"));
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
