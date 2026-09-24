use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
use windows_sys::Win32::System::Diagnostics::Debug::Beep;

use crate::config::AlertCfg;

/// Plays the alert without ever blocking the UI thread.
/// `Beep` is synchronous, so the generated fallback runs on a short-lived
/// thread; `playing` keeps overlapping alerts from stacking up.
#[derive(Clone, Default)]
pub struct Player {
    playing: Arc<AtomicBool>,
}

impl Player {
    pub fn new() -> Self {
        Self::default()
    }

    /// Alarm for a dropped connection: descending double beep, or the
    /// user's own .wav when `alert.sound_file` points at one.
    pub fn alarm(&self, cfg: &AlertCfg) {
        if !cfg.sound_file.trim().is_empty() && Path::new(cfg.sound_file.trim()).is_file() {
            play_wav(cfg.sound_file.trim());
            return;
        }
        let freq = cfg.beep_freq_hz;
        let dur = cfg.beep_ms;
        let count = cfg.beep_count;
        self.beeps((0..count).map(|i| (freq.saturating_sub(i * 110).max(200), dur)).collect());
    }

    /// Short rising chime when replies come back.
    pub fn recovered(&self) {
        self.beeps(vec![(660, 70), (990, 90)]);
    }

    fn beeps(&self, pattern: Vec<(u32, u32)>) {
        if self.playing.swap(true, Ordering::SeqCst) {
            return; // an alert is already sounding
        }
        let flag = self.playing.clone();
        std::thread::Builder::new()
            .name("pinglive-beep".into())
            .spawn(move || {
                for (freq, ms) in pattern {
                    unsafe { Beep(freq, ms) };
                }
                flag.store(false, Ordering::SeqCst);
            })
            .map(|_| ())
            .unwrap_or_else(|_| self.playing.store(false, Ordering::SeqCst));
    }
}

fn play_wav(path: &str) {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        PlaySoundW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}
