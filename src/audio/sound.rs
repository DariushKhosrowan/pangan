//! Modern sound engine — procedural synthesis (no external assets).
//!
//! Pangan synthesizes its own audio from mathematical models, so the app
//! ships self-contained with zero audio assets, but the engine is built to
//! *modern* production standards rather than raw sine plinks:
//!
//! * **Soft, rounded attacks** (short linear attack ramps) — no digital clicks.
//! * **Detuned partial pairs + chorus** — a warm, layered, "expensive" tone.
//! * **Inharmonic bell/percussive partials** (FM-style) for chimes & gongs.
//! * **Schroeder reverb** (comb + allpass network) — a spacious stereo tail.
//! * **Stereo width** — left/right reverb delays differ slightly.
//!
//! Traditional Persian flavor is preserved (santur, daf, tombak, karn,
//! sand, cyrus), but each theme is rendered with a warm, polished, modern
//! character suited to a contemporary productivity app.
//!
//! All sounds are synthesized to WAV files at first run (or when the theme
//! changes) so playback is instant thereafter.

use std::io::Write;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 44100;
const TAU: f32 = std::f32::consts::TAU;
/// Overall level relative to full scale (0..1). Kept below 1 so the summed
/// voice + reverb tail has headroom and never clips harshly.
const MASTER_GAIN: f32 = 0.5;
/// Bump this to force regeneration of the cached WAVs when the engine or
/// tuning changes.
const CACHE_VERSION: u32 = 2;

fn sr() -> f32 {
    SAMPLE_RATE as f32
}

fn clamp16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

/// Scale a mono buffer so its peak is 0.9 (nice headroom before reverb).
fn normalize(buf: &mut [f32]) {
    let peak = buf.iter().fold(0.0f32, |a, &v| a.max(v.abs()));
    if peak > 0.9 {
        let s = 0.9 / peak;
        for v in buf.iter_mut() {
            *v *= s;
        }
    }
}

// ---------------------------------------------------------------------------
// Reverb (Schroeder): 4 parallel comb filters → 2 series allpass filters.
// Independent delay sets for L/R give stereo width.
// ---------------------------------------------------------------------------

struct CombFilter {
    buf: Vec<f32>,
    idx: usize,
    fb: f32,
}

impl CombFilter {
    fn new(delay_samples: usize, fb: f32) -> Self {
        Self {
            buf: vec![0.0; delay_samples.max(1)],
            idx: 0,
            fb,
        }
    }
    fn run(&mut self, x: f32) -> f32 {
        let o = self.buf[self.idx];
        self.buf[self.idx] = x + o * self.fb;
        self.idx = (self.idx + 1) % self.buf.len();
        o
    }
}

struct AllpassFilter {
    buf: Vec<f32>,
    idx: usize,
    fb: f32,
}

impl AllpassFilter {
    fn new(delay_samples: usize, fb: f32) -> Self {
        Self {
            buf: vec![0.0; delay_samples.max(1)],
            idx: 0,
            fb,
        }
    }
    fn run(&mut self, x: f32) -> f32 {
        let d = self.buf[self.idx];
        let out = -x + d;
        self.buf[self.idx] = x + d * self.fb;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

/// Produce a stereo reverberated tail from a mono source (L/R interleaved as
/// separate Vecs). The returned signals are the *wet* contribution only.
fn reverberate_tail(input: &[f32]) -> (Vec<f32>, Vec<f32>) {
    // Comb delays in ms — classic Schroeder spread.
    let l_comb = [29.7f32, 37.1, 41.1, 43.7];
    let r_comb = [32.5f32, 39.3, 42.9, 46.0];
    let l_ap = 5.0f32;
    let r_ap = 1.7f32;

    let mut cl: Vec<CombFilter> = l_comb
        .iter()
        .map(|ms| CombFilter::new((ms * 0.001 * sr()) as usize, 0.77))
        .collect();
    let mut cr: Vec<CombFilter> = r_comb
        .iter()
        .map(|ms| CombFilter::new((ms * 0.001 * sr()) as usize, 0.77))
        .collect();
    let mut al = AllpassFilter::new((l_ap * 0.001 * sr()) as usize, 0.7);
    let mut ar = AllpassFilter::new((r_ap * 0.001 * sr()) as usize, 0.7);

    let n = input.len();
    let mut outl = vec![0.0f32; n];
    let mut outr = vec![0.0f32; n];

    for i in 0..n {
        let x = input[i];
        let mut sl = 0.0f32;
        for c in cl.iter_mut() {
            sl += c.run(x);
        }
        let mut srr = 0.0f32;
        for c in cr.iter_mut() {
            srr += c.run(x);
        }
        outl[i] = al.run(sl / 4.0);
        outr[i] = ar.run(srr / 4.0);
    }
    (outl, outr)
}

/// Convert a mono source into interleaved stereo i16, mixing a dry signal
/// with the wet reverb tail.
fn to_stereo(mono: &[f32], wet: f32) -> Vec<i16> {
    let (l, r) = reverberate_tail(mono);
    let dry = (1.0 - wet).max(0.0);
    let n = mono.len();
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let dl = mono[i] * dry + l[i] * wet;
        let dr = mono[i] * dry + r[i] * wet;
        out.push(clamp16(dl * MASTER_GAIN));
        out.push(clamp16(dr * MASTER_GAIN));
    }
    out
}

// ---------------------------------------------------------------------------
// Tone builders (mono f32, in [-1, 1] roughly)
// ---------------------------------------------------------------------------

/// A plucked string (santur): bright inharmonic partials, fast decay.
fn pluck_string(freq: f32, seconds: f32, gain: f32) -> Vec<f32> {
    let partials = [
        (1.0f32, 1.0f32),
        (1.997, 0.40),
        (2.99, 0.18),
        (3.97, 0.10),
        (5.92, 0.05),
    ];
    let n = (sr() * seconds) as usize;
    let mut out = vec![0.0f32; n];
    for (i, smp) in out.iter_mut().enumerate() {
        let t = i as f32 / sr();
        let att = (t / 0.005).min(1.0);
        let env = (-2.9 * t / seconds).exp().min(1.0);
        let mut s = 0.0f32;
        for (m, a) in partials {
            s += a * (TAU * freq * m * t).sin();
        }
        s += 0.30 * (TAU * freq * 1.004 * t).sin(); // chorus shimmer
        *smp = s * env * att * gain;
    }
    out
}

/// A resonant bell / gong strike: inharmonic partials, long shimmering tail.
fn bell_chime(freq: f32, seconds: f32, gain: f32) -> Vec<f32> {
    let partials = [
        (0.56f32, 0.50f32),
        (1.00, 1.00),
        (2.76, 0.30),
        (5.40, 0.12),
        (8.93, 0.05),
    ];
    let n = (sr() * seconds) as usize;
    let mut out = vec![0.0f32; n];
    for (i, smp) in out.iter_mut().enumerate() {
        let t = i as f32 / sr();
        let att = (t / 0.0025).min(1.0);
        let env = (-1.35 * t / seconds).exp().min(1.0);
        let mut s = 0.0f32;
        for (m, a) in partials {
            s += a * (TAU * freq * m * t).sin();
        }
        s += 0.15 * (TAU * freq * 1.007 * t).sin(); // slow beat shimmer
        *smp = s * env * att * gain;
    }
    out
}

/// A membrane hit (tombak/daf): pitched body + band-ish slap transient.
fn drum_membrane(body: f32, brightness: f32, seconds: f32, gain: f32) -> Vec<f32> {
    let n = (sr() * seconds) as usize;
    let mut out = vec![0.0f32; n];
    let mut seed = 0x9E3779B9u32;
    for (i, smp) in out.iter_mut().enumerate() {
        let t = i as f32 / sr();
        let freq = body * (1.0 - 0.10 * (t / seconds));
        let env = (-3.6 * t / seconds).exp();
        let mut s = (TAU * freq * t).sin();
        s += 0.40 * (TAU * freq * 1.98 * t).sin();
        s += 0.18 * (TAU * freq * 0.5 * t).sin();
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let rnd = ((seed >> 8) as f32 / 16777216.0) * 2.0 - 1.0;
        let slap = rnd * (-48.0 * t).exp() * brightness;
        *smp = (s + slap) * env * gain;
    }
    out
}

/// A short metallic jingle (daf ring).
fn jingle(gain: f32) -> Vec<f32> {
    bell_chime(2300.0, 0.14, 0.28 * gain)
}

/// A single fine sand / water-clock tick: short, bright click.
fn sand_tick(gain: f32) -> Vec<f32> {
    let n = (sr() * 0.014) as usize;
    let mut out = vec![0.0f32; n];
    let mut seed = 0x7F4A7C15u32;
    for (i, smp) in out.iter_mut().enumerate() {
        let t = i as f32 / sr();
        let env = (-85.0 * t).exp();
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let rnd = ((seed >> 8) as f32 / 16777216.0) * 2.0 - 1.0;
        *smp = rnd * env * gain * 0.8;
    }
    out
}

/// A soft sustained drone pad (warm low bed under phrases).
fn pad_drone(freqs: &[f32], seconds: f32, gain: f32) -> Vec<f32> {
    let n = (sr() * seconds) as usize;
    let mut out = vec![0.0f32; n];
    for (i, smp) in out.iter_mut().enumerate() {
        let t = i as f32 / sr();
        let att = (t / 0.25).min(1.0);
        let rel = ((seconds - t) / 0.35).clamp(0.0, 1.0);
        let env = att * rel;
        let mut s = 0.0f32;
        for &f in freqs {
            s += (TAU * f * t).sin() + 0.30 * (TAU * (f * 2.0) * t).sin();
        }
        *smp = s * env * gain;
    }
    out
}

// ---------------------------------------------------------------------------
// Phrase assembly
// ---------------------------------------------------------------------------

fn place(mono: &mut Vec<f32>, start_sample: usize, note: &[f32]) {
    if mono.len() < start_sample + note.len() {
        mono.resize(start_sample + note.len(), 0.0);
    }
    for (k, s) in note.iter().enumerate() {
        mono[start_sample + k] += s;
    }
}

fn arpeggio(freqs: &[f32], step: f32, note_secs: f32, gain: f32) -> Vec<f32> {
    let mut buf: Vec<f32> = Vec::new();
    for (k, &f) in freqs.iter().enumerate() {
        let start = (k as f32 * step * sr()) as usize;
        place(&mut buf, start, &pluck_string(f, note_secs, gain));
    }
    buf
}

fn bell_arp(freqs: &[f32], step: f32, note_secs: f32, gain: f32) -> Vec<f32> {
    let mut buf: Vec<f32> = Vec::new();
    for (k, &f) in freqs.iter().enumerate() {
        let start = (k as f32 * step * sr()) as usize;
        place(&mut buf, start, &bell_chime(f, note_secs, gain));
    }
    buf
}

fn blend(a: &[f32], b: &[f32]) -> Vec<f32> {
    let n = a.len().max(b.len());
    let mut out = vec![0.0f32; n];
    for (i, smp) in out.iter_mut().enumerate() {
        *smp = a.get(i).copied().unwrap_or(0.0) + b.get(i).copied().unwrap_or(0.0);
    }
    out
}

// --- santur: bright plucked strings over a warm pad -------------------------
fn work_phrase() -> Vec<f32> {
    let up = arpeggio(&[523.25, 659.25, 783.99, 880.00, 1046.50], 0.16, 1.0, 0.55);
    let pad = pad_drone(&[220.0, 329.63], 1.8, 0.16);
    blend(&up, &pad)
}

fn rest_phrase() -> Vec<f32> {
    let down = arpeggio(&[440.0, 392.0, 329.63, 261.63], 0.20, 1.1, 0.45);
    let pad = pad_drone(&[196.0, 261.63], 1.9, 0.18);
    blend(&down, &pad)
}

// --- daf: frame drum thump + shimmering jingles ------------------------------
fn daf_work() -> Vec<f32> {
    let thump = drum_membrane(88.0, 0.32, 0.7, 0.9);
    let j = jingle(1.0);
    let mut jingles: Vec<f32> = Vec::new();
    for k in 0..6 {
        let off = (k as f32 * 0.06 * sr()) as usize;
        place(&mut jingles, off, &j);
    }
    blend(&thump, &jingles)
}

fn daf_rest() -> Vec<f32> {
    let thump = drum_membrane(74.0, 0.28, 0.85, 0.7);
    let j = jingle(0.75);
    let mut jingles: Vec<f32> = Vec::new();
    for k in 0..8 {
        let off = (k as f32 * 0.05 * sr()) as usize;
        place(&mut jingles, off, &j);
    }
    blend(&thump, &jingles)
}

// --- tombak: goblet drum "tak–dum" / "dum–tak" -----------------------------
fn tombak_work() -> Vec<f32> {
    let tak = drum_membrane(320.0, 0.9, 0.16, 0.65);
    let dum = drum_membrane(96.0, 0.22, 0.55, 0.9);
    let mut buf: Vec<f32> = Vec::new();
    place(&mut buf, 0, &tak);
    place(&mut buf, (0.14 * sr()) as usize, &dum);
    buf
}

fn tombak_rest() -> Vec<f32> {
    let tak = drum_membrane(300.0, 0.85, 0.16, 0.55);
    let dum = drum_membrane(88.0, 0.20, 0.6, 0.85);
    let mut buf: Vec<f32> = Vec::new();
    place(&mut buf, 0, &dum);
    place(&mut buf, (0.16 * sr()) as usize, &tak);
    buf
}

// --- karn: ceremonial bell / gong with shimmering tail ----------------------
fn karn_work() -> Vec<f32> {
    let b = bell_chime(196.0, 2.8, 0.85);
    let shimmer = bell_chime(523.25, 1.0, 0.10);
    let mut buf = b;
    place(&mut buf, (0.10 * sr()) as usize, &shimmer);
    buf
}

fn karn_rest() -> Vec<f32> {
    let b = bell_chime(146.83, 3.2, 0.6);
    let shimmer = bell_chime(392.0, 0.9, 0.09);
    let mut buf = b;
    place(&mut buf, (0.12 * sr()) as usize, &shimmer);
    buf
}

// --- sand: water-clock ticking, meditative ----------------------------------
fn sand_work() -> Vec<f32> {
    let mut buf: Vec<f32> = Vec::new();
    for k in 0..5 {
        let off = (k as f32 * 0.12 * sr()) as usize;
        let t = sand_tick(1.0);
        place(&mut buf, off, &t);
    }
    let pad = pad_drone(&[1200.0, 1800.0], 1.4, 0.06);
    blend(&buf, &pad)
}

fn sand_rest() -> Vec<f32> {
    let mut buf: Vec<f32> = Vec::new();
    for k in 0..3 {
        let off = (k as f32 * 0.18 * sr()) as usize;
        let t = sand_tick(0.85);
        place(&mut buf, off, &t);
    }
    let pad = pad_drone(&[1000.0, 1500.0], 1.6, 0.05);
    blend(&buf, &pad)
}

// --- cyrus: regal fanfare (bell herald + ascending pentachord + drone) ------
fn cyrus_work() -> Vec<f32> {
    let herald = bell_chime(196.0, 2.2, 0.75);
    let herald2 = bell_chime(293.66, 1.6, 0.5);
    let arp = bell_arp(&[293.66, 349.23, 392.0, 440.0, 587.33], 0.17, 1.1, 0.5);
    let drone = pad_drone(&[146.83, 220.0], 2.6, 0.2);
    let mut buf = herald;
    place(&mut buf, (0.14 * sr()) as usize, &herald2);
    place(&mut buf, (0.30 * sr()) as usize, &arp);
    blend(&buf, &drone)
}

fn cyrus_rest() -> Vec<f32> {
    let cadence = bell_arp(&[293.66, 440.0, 293.66], 0.28, 1.4, 0.45);
    let drone = pad_drone(&[146.83, 196.0], 2.8, 0.22);
    blend(&cadence, &drone)
}

// ---------------------------------------------------------------------------
// Theming + file generation
// ---------------------------------------------------------------------------

pub const THEMES: [&str; 6] = ["santur", "daf", "tombak", "karn", "sand", "cyrus"];

pub struct Theme {
    pub work: Vec<i16>,
    pub rest: Vec<i16>,
}

/// Reverb mix (wet amount) per theme index/name.
fn wet_for(kind: &str) -> f32 {
    match kind {
        "karn" | "cyrus" => 0.38,
        _ => 0.32,
    }
}

impl Theme {
    fn generate(kind: &str) -> Self {
        let (mut work, mut rest) = match kind {
            "daf" => (daf_work(), daf_rest()),
            "tombak" => (tombak_work(), tombak_rest()),
            "karn" => (karn_work(), karn_rest()),
            "sand" => (sand_work(), sand_rest()),
            "cyrus" => (cyrus_work(), cyrus_rest()),
            _ => (work_phrase(), rest_phrase()), // santur default
        };
        normalize(&mut work);
        normalize(&mut rest);
        let w = wet_for(kind);
        Self {
            work: to_stereo(&work, w),
            rest: to_stereo(&rest, w),
        }
    }
}

pub struct Sounds {
    pub work: PathBuf,
    pub rest: PathBuf,
}

fn write_wav(path: &std::path::Path, samples: &[i16]) -> std::io::Result<()> {
    // Stereo 16-bit interleaved.
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
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&2u16.to_le_bytes())?; // 2 channels (stereo)
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&(SAMPLE_RATE * 2 * 2).to_le_bytes())?; // byte rate
    f.write_all(&4u16.to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?; // bits per sample
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
        let fname = |suffix: &str| dir.join(format!("{theme}_{suffix}_v{CACHE_VERSION}.wav"));
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
            // Stereo output must have an even number of samples.
            assert_eq!(t.work.len() % 2, 0, "{theme} work not stereo");
            assert_eq!(t.rest.len() % 2, 0, "{theme} rest not stereo");
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

    #[test]
    fn stereo_tail_differs_between_channels() {
        // The two channels should not be byte-identical (width comes from
        // differing L/R reverb delays).
        for theme in THEMES {
            let t = Theme::generate(theme);
            let mut any_diff = false;
            for ch in t.work.chunks(2) {
                if ch.len() == 2 && ch[0] != ch[1] {
                    any_diff = true;
                    break;
                }
            }
            assert!(any_diff, "{theme} has no stereo width");
        }
    }
}
