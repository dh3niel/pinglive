use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_FILENAME, SND_MEMORY, SND_NODEFAULT};

use crate::config::AlertCfg;

/// Plays alerts without ever blocking the UI thread.
///
/// The built-in sounds are synthesised into a small WAV in memory and played
/// with `PlaySound`, so they go through the normal Windows audio mixer (your
/// speakers or headset, the volume mixer) rather than the legacy `Beep()`
/// path, which is silent or barely audible on many Windows 10/11 machines.
/// Each sound plays synchronously on a short-lived thread that owns the
/// buffer; `playing` keeps overlapping alerts from stacking up.
#[derive(Clone, Default)]
pub struct Player {
    playing: Arc<AtomicBool>,
}

const RATE: u32 = 22_050;

impl Player {
    pub fn new() -> Self {
        Self::default()
    }

    /// Request timeout: descending alarm tones, or the user's own .wav when
    /// `alert.sound_file` points at one.
    pub fn alarm(&self, cfg: &AlertCfg) {
        if let Some(file) = wav_file(&cfg.sound_file) {
            return self.play(Sound::File(file));
        }
        let (freq, ms) = (cfg.beep_freq_hz as f32, cfg.beep_ms);
        let notes = (0..cfg.beep_count.max(1))
            .map(|i| Note { freq: (freq - i as f32 * 220.0).max(200.0), ms, gap_ms: 40 })
            .collect();
        self.play(Sound::Tones(notes, cfg.volume));
    }

    /// Ping spike (>= `high_ping_ms`): a quick rising double blip, distinct
    /// from the timeout alarm so you can tell them apart without looking.
    pub fn spike(&self, cfg: &AlertCfg) {
        if let Some(file) = wav_file(&cfg.spike_sound_file) {
            return self.play(Sound::File(file));
        }
        let notes = vec![
            Note { freq: 1175.0, ms: 60, gap_ms: 25 },
            Note { freq: 1568.0, ms: 90, gap_ms: 0 },
        ];
        self.play(Sound::Tones(notes, cfg.volume));
    }

    /// Short rising chime when replies come back.
    pub fn recovered(&self, cfg: &AlertCfg) {
        let notes = vec![
            Note { freq: 660.0, ms: 70, gap_ms: 10 },
            Note { freq: 990.0, ms: 110, gap_ms: 0 },
        ];
        self.play(Sound::Tones(notes, cfg.volume * 0.7));
    }

    fn play(&self, sound: Sound) {
        if self.playing.swap(true, Ordering::SeqCst) {
            return; // an alert is already sounding
        }
        let flag = self.playing.clone();
        let spawned = std::thread::Builder::new()
            .name("pinglive-sound".into())
            .spawn(move || {
                match sound {
                    Sound::File(path) => {
                        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                        unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), SND_FILENAME | SND_NODEFAULT) };
                    }
                    Sound::Tones(notes, volume) => {
                        let wav = synth(&notes, volume);
                        unsafe { PlaySoundW(wav.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_NODEFAULT) };
                    }
                }
                flag.store(false, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.playing.store(false, Ordering::SeqCst);
        }
    }
}

enum Sound {
    File(String),
    Tones(Vec<Note>, f32),
}

struct Note {
    freq: f32,
    ms: u32,
    gap_ms: u32,
}

fn wav_file(path: &str) -> Option<String> {
    let p = path.trim();
    (!p.is_empty() && Path::new(p).is_file()).then(|| p.to_owned())
}

/// 16-bit mono PCM WAV of the notes: a sine with a little third harmonic so
/// it cuts through game audio, with short fades so it does not click.
fn synth(notes: &[Note], volume: f32) -> Vec<u8> {
    let amp = volume.clamp(0.0, 1.0) * 0.95 * i16::MAX as f32;
    let fade = (RATE / 200) as usize; // 5 ms
    let mut pcm: Vec<i16> = Vec::new();
    for n in notes {
        let len = (RATE * n.ms / 1000) as usize;
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let w = std::f32::consts::TAU * n.freq * t;
            let env = (i.min(len - 1 - i) as f32 / fade as f32).min(1.0);
            pcm.push((amp * env * (0.85 * w.sin() + 0.15 * (3.0 * w).sin())) as i16);
        }
        pcm.extend(std::iter::repeat(0).take((RATE * n.gap_ms / 1000) as usize));
    }

    let data_len = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_writes_a_valid_wav() {
        let notes = [Note { freq: 880.0, ms: 100, gap_ms: 50 }, Note { freq: 660.0, ms: 100, gap_ms: 0 }];
        let wav = synth(&notes, 1.0);
        let samples = (RATE * 250 / 1000) as usize;
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize, wav.len() - 8);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize, samples * 2);
        assert_eq!(wav.len(), 44 + samples * 2);
        let peak = wav[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs()).max().unwrap();
        // sine + 15% third harmonic peaks at ~72% of the amplitude
        assert!(peak > 20_000, "too quiet: {peak}");
        // fades in: the first sample is silent
        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), 0);
    }
}
