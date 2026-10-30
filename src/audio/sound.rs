//! Iranian sound engine — procedural synthesis (no external assets).
//!
//! Pangan generates its own audio from mathematical models, in the spirit of
//! the traditional instruments of Persia: the **santur** (hammered dulcimer),
//! the **daf** (frame drum with jingles), the **tombak** (goblet drum), the
//! **karn** (ceremonial horn/gong), and a fine **sand** tick that evokes an
//! ancient water clock (پنگان — the pangan).
//!
//! All sounds are synthesized to WAV files at first run (or when the theme
//! changes) so the app ships self-contained with zero audio assets.

use std::io::Write;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 44100;
const TAU: f32 = std::f32::consts::TAU;

/// How loud a completed focus cue plays relative to full scale (0..1).
const MASTER_GAIN: f32 = 0.55;

// ---------------------------------------------------------------------------
// Basic building blocks (additive / modal synthesis)
// ---------------------------------------------------------------------------

/// `[freq_multiplier, amplitude]` partials for a **santur** pluck. A
/// hammered dulcimer has a bright, inharmonic spectrum that decays fast.
const SANTUR_PARTIALS: [(f32, f32); 6] = [
    (1.00, 1.00),
    (2.02, 0.42),
    (3.01, 0.22),
    (4.03, 0.10),
    (5.97, 0.05),
    (7.94, 0.025),
];

/// Rich, long-decay partials for a ceremonial **karn/gong** strike.
const GONG_PARTIALS: [(f32, f32); 5] = [
    (0.56, 0.60), // sub fund
    (1.00, 1.00),
    (2.76, 0.35),
    (5.40, 0.16),
    (8.93, 0.07),
];

/// Generate a plucked-string note wave (santur-like).
fn santur_note(freq: f32, seconds: f32, gain: f32) -> Vec<i16> {
    let n = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(n);
    let decay = 2.6; // fast pluck decay
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-decay * t / seconds).exp().min(1.0);
        let mut s = 0.0f32;
        for (mult, amp) in SANTUR_PARTIALS {
            s += amp * (TAU * freq * mult * t).sin();
        }
        // slight brightness bloom at attack
        let att = (t * 800.0).min(1.0);
        let v = (s * env * att * gain * MASTER_GAIN).clamp(-1.0, 1.0);
        out.push((v * i16::MAX as f32) as i16);
    }
    out
}

/// A resonant gong / karn strike with a long beautiful tail.
fn gong_strike(freq: f32, seconds: f32, gain: f32) -> Vec<i16> {
    let n = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(n);
    let decay = 1.15;
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-decay * t / seconds).exp().min(1.0);
        let mut s = 0.0f32;
        for (mult, amp) in GONG_PARTIALS {
            s += amp * (TAU * freq * mult * t).sin();
        }
        // beat between near partials → shimmer
        s += 0.18 * (TAU * freq * 1.005 * t).sin();
        let v = (s * env * gain * MASTER_GAIN).clamp(-1.0, 1.0);
        out.push((v * i16::MAX as f32) as i16);
    }
    out
}

/// A membrane hit (tombak/daf drum). `body` is the fundamental in Hz,
/// `bright` adds a short slap transient (noise) for the "tak" stroke.
fn drum_hit(body: f32, brightness: f32, seconds: f32, gain: f32) -> Vec<i16> {
    let n = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(n);
    // deterministic pseudo-noise seed
    let mut seed = 0x9E3779B9u32;
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        // pitch drops slightly after strike (drum behavior)
        let freq = body * (1.0 - 0.08 * (t / seconds));
        let env = (-4.0 * t / seconds).exp();
        let mut s = (TAU * freq * t).sin();
        s += 0.35 * (TAU * freq * 1.98 * t).sin();
        // slap transient (bandpassed-ish noise)
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let rnd = ((seed >> 8) as f32 / 16777216.0) * 2.0 - 1.0;
        let slap = rnd * (-55.0 * t).exp() * brightness;
        let v = ((s + slap) * env * gain * MASTER_GAIN).clamp(-1.0, 1.0);
        out.push((v * i16::MAX as f32) as i16);
    }
    out
}

/// A single fine sand/"water-clock" tick: a short, bright click.
fn sand_tick(gain: f32) -> Vec<i16> {
    let n = (SAMPLE_RATE as f32 * 0.012) as usize;
    let mut out = Vec::with_capacity(n);
    let mut seed = 0x7F4A7C15u32;
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-90.0 * t).exp();
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let rnd = ((seed >> 8) as f32 / 16777216.0) * 2.0 - 1.0;
        let v = (rnd * env * gain * MASTER_GAIN * 0.5).clamp(-1.0, 1.0);
        out.push((v * i16::MAX as f32) as i16);
    }
    out
}

fn mix(a: &[i16], b: &[i16], out: &mut Vec<i16>) {
    let len = a.len().max(b.len());
    out.resize(len, 0);
    for i in 0..len {
        let sa = a.get(i).copied().unwrap_or(0) as i32;
        let sb = b.get(i).copied().unwrap_or(0) as i32;
        out[i] = ((sa + sb) >> 1).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

fn append_into(buf: &mut Vec<i16>, samples: &[i16]) {
    buf.extend_from_slice(samples);
}

// ---------------------------------------------------------------------------
// Phrases (melodic contours tuned to Dastgah-e Mahur / Homayoun flavor)
// ---------------------------------------------------------------------------

/// Frequencies of a Mahur-ish phrase around C (step 1/2 in 12-edo).
const MAHUR: [f32; 7] = [261.63, 293.66, 329.63, 369.99, 392.00, 440.00, 493.88]; // C D E F# G A B

/// Place a note into `buf` at `start_sample`, summing overlaps.
fn place_note(buf: &mut Vec<i16>, start_sample: usize, note: &[i16]) {
    if buf.len() < start_sample + note.len() {
        buf.resize(start_sample + note.len(), 0);
    }
    for i in 0..note.len() {
        let idx = start_sample + i;
        let sum = buf[idx] as i32 + note[i] as i32;
        buf[idx] = (sum >> 1).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

/// Play a sequence of frequencies as staggered santur notes.
fn arpeggio(freqs: &[f32], step_secs: f32, note_secs: f32, gain: f32) -> Vec<i16> {
    let mut buf = Vec::new();
    for (k, &f) in freqs.iter().enumerate() {
        let start = (k as f32 * step_secs * SAMPLE_RATE as f32) as usize;
        place_note(&mut buf, start, &santur_note(f, note_secs, gain));
    }
    buf
}

/// Work-complete: a bright ascending cascade on the santur
/// (the "victory" flourish — rising like the sun over Persepolis).
fn work_phrase() -> Vec<i16> {
    // C5 E5 G5 A5 C6 — a glorious ascending Mahur gesture
    let freqs = [523.25, 659.25, 783.99, 880.00, 1046.50];
    arpeggio(&freqs, 0.16, 0.9, 0.7)
}

/// Break-complete: a warm, grounded descending cadence.
fn rest_phrase() -> Vec<i16> {
    // A4 G4 E4 C4 — settling down
    let freqs = [440.00, 392.00, 329.63, 261.63];
    arpeggio(&freqs, 0.20, 1.0, 0.55)
}

/// Daf frame-drum greeting: low thump + shimmering jingles.
fn daf_work() -> Vec<i16> {
    let thump = drum_hit(92.0, 0.30, 0.6, 0.8);
    let mut jingles = Vec::new();
    for k in 0..6 {
        let t = (k as f32 * 0.06f32 * SAMPLE_RATE as f32) as usize;
        let mut slot = vec![0i16; t];
        let j = gong_strike(2400.0, 0.12, 0.16); // metal jingle
        slot.append(&mut j.clone());
        if jingles.len() < slot.len() {
            jingles.resize(slot.len(), 0);
        }
        for i in 0..slot.len() {
            jingles[i] = ((jingles[i] as i32 + slot[i] as i32) >> 1)
                .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }
    }
    let mut out = Vec::new();
    mix(&thump, &jingles, &mut out);
    out
}

fn daf_rest() -> Vec<i16> {
    let thump = drum_hit(74.0, 0.4, 0.8, 0.6);
    // a gentle roll of jingles
    let mut jingles = Vec::new();
    for k in 0..10 {
        let t = (k as f32 * 0.045f32 * SAMPLE_RATE as f32) as usize;
        let mut slot = vec![0i16; t];
        let j = gong_strike(1800.0, 0.10, 0.12);
        slot.append(&mut j.clone());
        if jingles.len() < slot.len() {
            jingles.resize(slot.len(), 0);
        }
        for i in 0..slot.len() {
            jingles[i] = ((jingles[i] as i32 + slot[i] as i32) >> 1)
                .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        }
    }
    let mut out = Vec::new();
    mix(&thump, &jingles, &mut out);
    out
}

/// Tombak goblet drum pattern: "tak–dum" (work) / "dum–tak" (rest).
fn tombak_work() -> Vec<i16> {
    let mut buf = Vec::new();
    let tak = drum_hit(320.0, 0.9, 0.16, 0.6);
    let dum = drum_hit(96.0, 0.18, 0.5, 0.85);
    let mut slot = vec![0i16; (0.18 * SAMPLE_RATE as f32) as usize];
    slot.splice(..tak.len(), tak.iter().copied());
    append_into(&mut buf, &slot);
    append_into(&mut buf, &dum);
    buf
}

fn tombak_rest() -> Vec<i16> {
    let mut buf = Vec::new();
    let tak = drum_hit(300.0, 0.85, 0.16, 0.55);
    let dum = drum_hit(88.0, 0.16, 0.6, 0.8);
    append_into(&mut buf, &dum);
    let mut slot = vec![0i16; (0.16 * SAMPLE_RATE as f32) as usize];
    slot.splice(..tak.len(), tak.iter().copied());
    append_into(&mut buf, &slot);
    buf
}

/// Ceremonial karn/gong.
fn karn_work() -> Vec<i16> {
    gong_strike(196.0, 2.4, 0.8) // G3 — regal, sun-like
}

fn karn_rest() -> Vec<i16> {
    gong_strike(146.83, 2.8, 0.6) // D3 — deeper, settling
}

/// Sand / water-clock ticking: sparse, meditative.
fn sand_work() -> Vec<i16> {
    let mut buf = Vec::new();
    for k in 0..5 {
        let off = (0.12 * k as f32 * SAMPLE_RATE as f32) as usize;
        let mut slot = vec![0i16; off];
        slot.append(&mut sand_tick(1.0));
        append_into(&mut buf, &slot);
    }
    buf
}

fn sand_rest() -> Vec<i16> {
    let mut buf = Vec::new();
    for k in 0..3 {
        let off = (0.18 * k as f32 * SAMPLE_RATE as f32) as usize;
        let mut slot = vec![0i16; off];
        slot.append(&mut sand_tick(0.7));
        append_into(&mut buf, &slot);
    }
    buf
}

// ---------------------------------------------------------------------------
// Cyrus — a regal fanfare theme evoking the trumpeting herald of an ancient
// Persian court. Built from two interleaved karn/gong strikes and a bright
// ascending arpeggio of the *Cyrus cylinder* scale (a pentachord on D).
// ---------------------------------------------------------------------------

/// A fifth-above karn strike for the fanfare opening (the "herald").
fn cyrus_herald() -> Vec<i16> {
    let mut buf = Vec::new();
    let a = gong_strike(196.0, 1.6, 0.7); // G3
    let b = gong_strike(293.66, 1.2, 0.5); // D4
    append_into(&mut buf, &a);
    // Small gap before the second strike.
    let gap = (0.10 * SAMPLE_RATE as f32) as usize;
    if buf.len() < a.len() + gap {
        buf.resize(a.len() + gap, 0);
    }
    append_into(&mut buf, &b);
    buf
}

/// A bright, noble arpeggio (D F G A D) — the "triumph" statement.
fn cyrus_arpeggio() -> Vec<i16> {
    // D4 F4 G4 A4 D5 — a pentachord gesture that feels both ancient and regal.
    let freqs = [293.66, 349.23, 392.00, 440.00, 587.33];
    arpeggio(&freqs, 0.17, 0.9, 0.65)
}

fn cyrus_work() -> Vec<i16> {
    let mut buf = cyrus_herald();
    buf.extend(cyrus_arpeggio());
    buf
}

fn cyrus_rest() -> Vec<i16> {
    // A warm, resolving cadence: D A D.
    let freqs = [293.66, 440.00, 293.66];
    arpeggio(&freqs, 0.28, 1.2, 0.5)
}

// ---------------------------------------------------------------------------
// Theming + file generation
// ---------------------------------------------------------------------------

pub const THEMES: [&str; 6] = ["santur", "daf", "tombak", "karn", "sand", "cyrus"];

pub struct Theme {
    pub work: Vec<i16>,
    pub rest: Vec<i16>,
}

impl Theme {
    fn generate(kind: &str) -> Self {
        let (work, rest) = match kind {
            "daf" => (daf_work(), daf_rest()),
            "tombak" => (tombak_work(), tombak_rest()),
            "karn" => (karn_work(), karn_rest()),
            "sand" => (sand_work(), sand_rest()),
            "cyrus" => (cyrus_work(), cyrus_rest()),
            _ => (work_phrase(), rest_phrase()), // santur default
        };
        Self { work, rest }
    }
}

pub struct Sounds {
    pub work: PathBuf,
    pub rest: PathBuf,
}

fn write_wav(path: &std::path::Path, samples: &[i16]) -> std::io::Result<()> {
    let data_bytes = samples.len() * 2;
    if data_bytes > u32::MAX as usize {
        return Err(std::io::Error::other("wav too large"));
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_bytes as u32).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&(data_bytes as u32).to_le_bytes())?;
    for s in samples.iter().take(SAMPLE_RATE as usize * 20) {
        // cap at 20s to be safe
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

impl Sounds {
    /// Persist the requested theme's pair to disk (only when missing or
    /// theme changed), returning the two file paths.
    pub fn ensure(dir: &PathBuf, theme: &str) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let fname = |suffix: &str| dir.join(format!("{theme}_{suffix}.wav"));
        let work = fname("work");
        let rest = fname("rest");

        if !work.exists() || !rest.exists() {
            let t = Theme::generate(theme);
            let _ = write_wav(&work, &t.work);
            let _ = write_wav(&rest, &t.rest);
        }
        Self { work, rest }
    }

    /// Ensure all themes are cached (warm the cache so switching is instant).
    pub fn warm_all(dir: &PathBuf) {
        for theme in THEMES {
            let _ = Self::ensure(dir, theme);
        }
    }
}

/// Find a playable command for a WAV file at the given volume (0-100).
fn play_command(path: &std::path::Path, volume: u8) -> Option<Vec<String>> {
    let vol = (volume as f32 / 100.0).clamp(0.0, 1.0);
    for player in ["paplay", "pw-play", "aplay", "ffplay"] {
        if which(player) {
            let args: Vec<String> = match player {
                "paplay" => vec![
                    player.into(),
                    "--volume".into(),
                    ((vol * 65535.0) as u32).to_string(),
                    path.display().to_string(),
                ],
                "pw-play" => vec![
                    player.into(),
                    "--volume".into(),
                    format!("{vol:.2}"),
                    path.display().to_string(),
                ],
                "ffplay" => vec![
                    player.into(),
                    "-nodisp".into(),
                    "-autoexit".into(),
                    "-loglevel".into(),
                    "quiet".into(),
                    "-volume".into(),
                    ((vol * 100.0) as u32).to_string(),
                    path.display().to_string(),
                ],
                _ => vec![player.into(), path.display().to_string()],
            };
            return Some(args);
        }
    }
    None
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {cmd} >/dev/null 2>&1"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Play a WAV file at the given volume (0-100), spawned detached so it never
/// blocks the UI.
pub fn play(path: &std::path::Path, volume: u8) {
    if let Some(args) = play_command(path, volume) {
        let _ = std::process::Command::new(&args[0])
            .args(&args[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Play at the default/current volume (kept for backward compatibility in tests).
#[allow(dead_code)]
pub fn play_at_default(path: &std::path::Path) {
    play(path, 80);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_themes_generate_nonempty_waveforms() {
        for theme in THEMES {
            let t = Theme::generate(theme);
            assert!(!t.work.is_empty(), "{theme} work empty");
            assert!(!t.rest.is_empty(), "{theme} rest empty");
        }
    }

    #[test]
    fn waveforms_stay_within_i16_bounds_and_are_not_silent() {
        for theme in THEMES {
            let t = Theme::generate(theme);
            let peak = t
                .work
                .iter()
                .chain(t.rest.iter())
                .map(|s| s.unsigned_abs() as u32)
                .max()
                .unwrap_or(0);
            assert!(peak > 0, "{theme} silent");
            assert!(peak <= i16::MAX as u32);
        }
    }
}
