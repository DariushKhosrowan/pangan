//! Platform-specific integrations: system tray, notifications, global hotkey,
//! idle-inhibit, and autostart. Shared, platform-agnostic timer logic lives in
//! `clock.rs`. Each capability here is implemented per-OS behind `cfg`.

pub mod autostart;
pub mod inhibit;
pub mod notify;
pub mod tray;

/// Icon generation shared by tray implementations (RGBA tomato disc).
pub fn tomato_icon_rgba(size: u32) -> Vec<u8> {
    let size = size as usize;
    let mut rgba = vec![0u8; size * size * 4];
    let (cx, cy) = ((size as f32 - 1.0) / 2.0, (size as f32 - 1.0) / 2.0);
    let r = size as f32 * 0.42;
    let r2 = r * r;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let idx = (y * size + x) * 4;
            if dx * dx + dy * dy <= r2 {
                rgba[idx] = 230;
                rgba[idx + 1] = 69;
                rgba[idx + 2] = 60;
                rgba[idx + 3] = 255;
            } else {
                rgba[idx + 3] = 0;
            }
        }
    }
    rgba
}
