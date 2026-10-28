//! Idle-inhibit during focus sessions, so the screen does not lock/sleep.
//! Linux: logind `org.freedesktop.login1.Manager.Inhibit` via zbus.
//! Windows: `SetThreadExecutionState`.
//! macOS: `caffeinate -d` subprocess.

/// Guard that keeps an inhibit active until dropped.
pub struct InhibitGuard {
    #[cfg(target_os = "linux")]
    _fd: Option<zbus::zvariant::OwnedFd>,
    #[cfg(target_os = "windows")]
    _dummy: (),
    #[cfg(target_os = "macos")]
    child: Option<std::process::Child>,
}

/// Acquire an idle-inhibit. Returns `None` if unavailable (harmless).
pub fn acquire() -> Option<InhibitGuard> {
    #[cfg(target_os = "linux")]
    {
        use zbus::blocking::Connection;
        let conn = Connection::system().ok()?;
        let reply = conn
            .call_method(
                Some("org.freedesktop.login1"),
                "/org/freedesktop/login1",
                Some("org.freedesktop.login1.Manager"),
                "Inhibit",
                &("sleep", "پنگان", "نشست کار ژرف روان است", "block"),
            )
            .ok()?;
        let fd: zbus::zvariant::OwnedFd = reply.body().deserialize().ok()?;
        Some(InhibitGuard { _fd: Some(fd) })
    }

    #[cfg(target_os = "windows")]
    {
        #[link(name = "powrprof")]
        extern "system" {
            fn SetThreadExecutionState(es_flags: u32) -> u32;
        }
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        }
        Some(InhibitGuard { _dummy: () })
    }

    #[cfg(target_os = "macos")]
    {
        // caffeinate -d prevents display sleep.
        let child = std::process::Command::new("caffeinate")
            .arg("-d")
            .spawn()
            .ok()?;
        Some(InhibitGuard { child: Some(child) })
    }
}

impl Drop for InhibitGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            #[link(name = "powrprof")]
            extern "system" {
                fn SetThreadExecutionState(es_flags: u32) -> u32;
            }
            const ES_CONTINUOUS: u32 = 0x8000_0000;
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
            }
        }
        // Linux: dropping the OwnedFd releases the logind inhibit.
    }
}
