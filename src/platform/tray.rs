//! System tray. Linux uses `ksni` (pure-DBus StatusNotifierItem, no GTK).
//! Windows/macOS use `tray-icon` (native). Both expose a common spawn entry
//! that routes menu actions back into the shared timer state.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::clock::Shared;
use crate::TrayAction;

// ---------------------------------------------------------------------------
// Linux: ksni
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
pub fn spawn(_shared: Arc<Shared>, tx: Sender<TrayAction>) {
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem, Tray};

    struct PanganTray {
        tx: Sender<TrayAction>,
    }

    impl Tray for PanganTray {
        fn id(&self) -> String {
            "pangan".into()
        }
        fn title(&self) -> String {
            "Pangan".into()
        }
        fn icon_pixmap(&self) -> Vec<Icon> {
            let rgba = crate::platform::tomato_icon_rgba(32);
            // Icon.data is ARGB32 network byte order; convert RGBA -> ARGB.
            let mut argb = Vec::with_capacity(rgba.len());
            for px in rgba.as_chunks::<4>().0 {
                argb.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
            }
            vec![Icon {
                width: 32,
                height: 32,
                data: argb,
            }]
        }
        fn menu(&self) -> Vec<MenuItem<Self>> {
            let toggle = StandardItem {
                label: "Start/Pause".into(),
                enabled: true,
                visible: true,
                shortcut: vec![vec!["Super".into(), "P".into()]],
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayAction::Toggle);
                }),
                ..Default::default()
            };
            let skip = StandardItem {
                label: "Skip".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayAction::Skip);
                }),
                ..Default::default()
            };
            let reset = StandardItem {
                label: "Reset".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayAction::Reset);
                }),
                ..Default::default()
            };
            let quit = StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            };
            vec![
                toggle.into(),
                skip.into(),
                reset.into(),
                MenuItem::Separator,
                quit.into(),
            ]
        }
    }

    let tray = PanganTray { tx };

    // Run the tray service on a dedicated thread; blocking API.
    std::thread::spawn(move || {
        match tray.spawn() {
            Ok(handle) => {
                // Keep the handle alive for the process lifetime. The menu
                // callbacks already route actions via `tx`.
                std::mem::forget(handle);
            }
            Err(_) => {
                // The desktop has no StatusNotifierWatcher (e.g. bare WM).
                // The window + hotkey still work; just no tray.
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Windows / macOS: tray-icon
// ---------------------------------------------------------------------------
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn spawn(shared: Arc<Shared>, tx: Sender<TrayAction>) {
    use tray_icon::{
        menu::{Menu, MenuItemBuilder, PredefinedMenuItem},
        Icon, TrayIconBuilder,
    };

    let toggle = MenuItemBuilder::new()
        .id("toggle".into())
        .text("Start/Pause")
        .build();
    let skip = MenuItemBuilder::new()
        .id("skip".into())
        .text("Skip")
        .build();
    let reset = MenuItemBuilder::new()
        .id("reset".into())
        .text("Reset")
        .build();
    let quit = MenuItemBuilder::new()
        .id("quit".into())
        .text("Quit")
        .build();

    let menu = Menu::new();
    let _ = menu.append(&toggle);
    let _ = menu.append(&skip);
    let _ = menu.append(&reset);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&quit);

    let rgba = crate::platform::tomato_icon_rgba(32);
    let icon = Icon::from_rgba(rgba, 32, 32).unwrap();

    if let Ok(_tray) = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("Pangan")
        .build()
    {
        std::mem::forget(_tray);
    }

    // Poll tray menu events on a background thread.
    std::thread::spawn(move || {
        while !shared.quit.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                let action = match event.id().as_ref() {
                    "toggle" => Some(TrayAction::Toggle),
                    "skip" => Some(TrayAction::Skip),
                    "reset" => Some(TrayAction::Reset),
                    "quit" => Some(TrayAction::Quit),
                    _ => None,
                };
                if let Some(action) = action {
                    if tx.send(action).is_err() {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    });
}
