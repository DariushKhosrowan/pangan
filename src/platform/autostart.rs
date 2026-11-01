//! Autostart at login via the OS mechanism:
//! Linux: `~/.config/autostart/pangan.desktop`
//! Windows: HKCU\Software\Microsoft\Windows\CurrentVersion\Run "Pangan"
//! macOS: `~/Library/LaunchAgents/com.pangan.timer.plist`

pub fn ensure() {
    #[cfg(target_os = "linux")]
    {
        let Some(exe) = std::env::current_exe().ok() else {
            return;
        };
        let Some(autostart) = dirs::config_dir().map(|d| d.join("autostart")) else {
            return;
        };
        if autostart.join("pangan.desktop").exists() {
            return;
        }
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Pangan\nName[fa]=پنگان\nComment=Pomodoro timer\nComment[fa]=زمان‌سنج کار ژرف\nExec={}\nX-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        if std::fs::create_dir_all(&autostart).is_ok() {
            let _ = std::fs::write(autostart.join("pangan.desktop"), desktop);
        }
    }

    #[cfg(target_os = "windows")]
    {
        let Some(exe) = std::env::current_exe().ok() else {
            return;
        };
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        if let Ok(hkcu) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            winreg::enums::KEY_SET_VALUE,
        ) {
            let _ = hkcu.set_value("Pangan", &exe.to_string_lossy().to_string());
        }
    }

    #[cfg(target_os = "macos")]
    {
        let Some(exe) = std::env::current_exe().ok() else {
            return;
        };
        let plist_dir = dirs::home_dir().map(|h| h.join("Library/LaunchAgents"));
        let Some(plist_dir) = plist_dir else { return };
        let plist_path = plist_dir.join("com.pangan.timer.plist");
        if plist_path.exists() {
            return;
        }
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>com.pangan.timer</string>\n  <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n  </array>\n  <key>RunAtLoad</key>\n  <true/>\n</dict>\n</plist>\n",
            exe.display()
        );
        if std::fs::create_dir_all(&plist_dir).is_ok() {
            let _ = std::fs::write(plist_path, plist);
        }
    }
}
