//! Desktop notifications. Linux/macOS use `notify-rust`; Windows uses
//! `winrt-notification` (toast API).

// ---------------------------------------------------------------------------
// Linux / macOS
// ---------------------------------------------------------------------------
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn send(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show();
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
pub fn send(title: &str, body: &str) {
    use winrt_notification::{Duration, Sound, Toast};
    // AUMID registered at build/runtime; winrt-notification requires one.
    let _ = Toast::new("pangan.Pangan")
        .title(title)
        .text1(body)
        .duration(Duration::Short)
        .sound(Some(Sound::Default))
        .show();
}
