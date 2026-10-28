//! Sound: synthesizes pleasant chime cues (work/break completion) as WAV
//! files and plays them through the best available system player.

use std::io::Write;
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 44100;

/// A single synthesized note: sine partials with exponential decay (bell-like).
fn note_samples(freq: f32, seconds: f32, gain: f32) -> Vec<i16> {
    let n = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut out = Vec::with_capacity(n);
    // bell partials: fundamental, ~2.7x, ~5.4x (inharmonic → pleasant chime)
    let partials = [(1.0f32, 1.0f32), (2.7, 0.35), (5.4, 0.12)];
    for i in 0..n {
        let t = i as f32 / SAMPLE_RATE as f32;
        let env = (-3.5 * t / seconds).exp();
        let mut s = 0.0f32;
        for (mult, amp) in partials {
            s += amp * (2.0 * std::f32::consts::PI * freq * mult * t).sin();
        }
        let v = (s * env * gain).clamp(-1.0, 1.0);
        out.push((v * i16::MAX as f32) as i16);
    }
    out
}

fn write_wav(path: &std::path::Path, samples: &[i16]) -> std::io::Result<()> {
    let data_bytes = samples.len() * 2;
    let mut f = std::fs::File::create(path)?;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_bytes as u32).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;      // fmt chunk size
    f.write_all(&1u16.to_le_bytes())?;        // PCM
    f.write_all(&1u16.to_le_bytes())?;        // mono
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?; // byte rate
    f.write_all(&2u16.to_le_bytes())?;        // block align
    f.write_all(&16u16.to_le_bytes())?;       // bits per sample
    f.write_all(b"data")?;
    f.write_all(&(data_bytes as u32).to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

pub struct Sounds {
    pub work: PathBuf,
    pub rest: PathBuf,
}

impl Sounds {
    /// Generate chime files (if missing) under the given directory.
    pub fn ensure(dir: &PathBuf) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let work = dir.join("work_done.wav");
        let rest = dir.join("break_done.wav");
        if !work.exists() {
            // work complete: bright ascending pair C5 -> E5
            let mut s = Vec::new();
            s.extend(note_samples(523.25, 0.55, 0.5));
            s.extend(note_samples(659.25, 0.8, 0.45));
            let _ = write_wav(&work, &s);
        }
        if !rest.exists() {
            // break over: warm gentle pair G4 -> E4
            let mut s = Vec::new();
            s.extend(note_samples(392.0, 0.5, 0.45));
            s.extend(note_samples(329.63, 0.7, 0.4));
            let _ = write_wav(&rest, &s);
        }
        Self { work, rest }
    }
}

/// Find a playable command for a WAV file, preferring quality/quiet players.
fn play_command(path: &std::path::Path) -> Option<Vec<String>> {
    // Prefer PulseAudio/pipewire CLI (does not steal the audio sink config).
    for player in ["paplay", "pw-play", "aplay", "ffplay"] {
        if which(player) {
            let args: Vec<String> = match player {
                "ffplay" => vec![
                    player.into(),
                    "-nodisp".into(),
                    "-autoexit".into(),
                    "-loglevel".into(),
                    "quiet".into(),
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

/// Play a WAV file, spawned detached so it never blocks the UI.
pub fn play(path: &std::path::Path) {
    if let Some(args) = play_command(path) {
        let _ = std::process::Command::new(&args[0])
            .args(&args[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
