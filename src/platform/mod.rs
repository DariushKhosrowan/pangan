//! Platform-specific integrations: system tray, notifications, global hotkey,
//! idle-inhibit, and autostart. Shared, platform-agnostic timer logic lives in
//! `clock.rs`. Each capability here is implemented per-OS behind `cfg`.

pub mod autostart;
pub mod inhibit;
pub mod notify;
pub mod tray;

/// Icon generation shared by tray implementations (golden Persian sun disc).
pub fn tomato_icon_rgba(size: u32) -> Vec<u8> {
    let size = size as usize;
    let mut rgba = vec![0u8; size * size * 4];
    let (cx, cy) = ((size as f32 - 1.0) / 2.0, (size as f32 - 1.0) / 2.0);
    let r = size as f32 * 0.42;
    let r2 = r * r;
    let inner = size as f32 * 0.17;
    let inner2 = inner * inner;
    let gold = [224u8, 178, 92, 255]; // #E0B25C
    let indigo = [38u8, 43, 78, 255]; // #262B4E
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let idx = (y * size + x) * 4;
            let d2 = dx * dx + dy * dy;
            if d2 <= r2 {
                if d2 >= inner2 {
                    rgba[idx..idx + 4].copy_from_slice(&gold);
                } else {
                    rgba[idx..idx + 4].copy_from_slice(&indigo);
                }
            } else {
                rgba[idx + 3] = 0;
            }
        }
    }
    rgba
}
