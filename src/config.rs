use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Hostname or IP to ping, e.g. "8.8.8.8" or a Dota 2 server IP.
    pub target: String,
    /// Optional short label drawn in the overlay instead of `target`.
    pub label: String,
    /// Milliseconds between pings.
    pub interval_ms: u64,
    /// Milliseconds before a reply counts as a timeout.
    pub timeout_ms: u32,
    /// How many samples the graph keeps.
    pub history: usize,
    /// Days of raw ping history kept on disk; 0 = keep forever.
    pub retention_days: u32,

    pub window: WindowCfg,
    pub alert: AlertCfg,
    pub colors: ColorCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowCfg {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Background opacity of the overlay panel, 0.0 (invisible) .. 1.0 (solid).
    pub opacity: f32,
    /// Start in click-through mode (mouse events go to the game).
    pub click_through: bool,
    /// Keep the window above fullscreen-windowed games.
    pub always_on_top: bool,
    /// Show the graph; false = compact one-line readout.
    pub show_graph: bool,
    /// false = hidden to the tray (double-click the tray icon to bring it back).
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertCfg {
    pub enabled: bool,
    /// Consecutive timeouts needed before the alert fires.
    pub timeout_streak: u32,
    /// Minimum seconds between two alerts, so a dead link does not machine-gun.
    pub cooldown_secs: u64,
    /// Play a short two-tone "recovered" chime when replies come back.
    pub recovery_sound: bool,
    /// Also alert when ping stays above `high_ping_ms` for `high_ping_streak` samples.
    pub high_ping_ms: u32,
    pub high_ping_streak: u32,
    /// Absolute path to a .wav played on timeout. Empty = built-in generated beeps.
    pub sound_file: String,
    /// Built-in beep shape (used when `sound_file` is empty).
    pub beep_freq_hz: u32,
    pub beep_ms: u32,
    pub beep_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorCfg {
    /// ping <= good_ms is green
    pub good_ms: u32,
    /// ping <= warn_ms is yellow, above is red
    pub warn_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target: "8.8.8.8".into(),
            label: String::new(),
            interval_ms: 1000,
            timeout_ms: 1000,
            history: 120,
            retention_days: 365,
            window: WindowCfg::default(),
            alert: AlertCfg::default(),
            colors: ColorCfg::default(),
        }
    }
}

impl Default for WindowCfg {
    fn default() -> Self {
        Self {
            x: 40.0,
            y: 40.0,
            width: 240.0,
            height: 104.0,
            opacity: 0.45,
            click_through: true,
            always_on_top: true,
            show_graph: true,
            visible: true,
        }
    }
}

impl Default for AlertCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_streak: 1,
            cooldown_secs: 3,
            recovery_sound: false,
            high_ping_ms: 300,
            high_ping_streak: 1,
            sound_file: String::new(),
            beep_freq_hz: 880,
            beep_ms: 120,
            beep_count: 2,
        }
    }
}

impl Default for ColorCfg {
    fn default() -> Self {
        Self { good_ms: 60, warn_ms: 120 }
    }
}

pub fn config_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("PingLive").join("config.toml")
}

impl Config {
    /// Loads the config, writing a commented default file on first run.
    /// A broken file is kept on disk and defaults are used, so a typo never
    /// costs the user their settings.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => cfg.sanitized(),
                Err(e) => {
                    eprintln!("pinglive: {} is invalid ({e}); using defaults", path.display());
                    Config::default()
                }
            },
            Err(_) => {
                let cfg = Config::default();
                cfg.save();
                cfg
            }
        }
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = toml::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    pub fn sanitized(mut self) -> Self {
        self.interval_ms = self.interval_ms.clamp(100, 60_000);
        self.timeout_ms = self.timeout_ms.clamp(100, 10_000);
        self.history = self.history.clamp(20, 2000);
        self.window.opacity = self.window.opacity.clamp(0.0, 1.0);
        self.window.width = self.window.width.clamp(140.0, 1200.0);
        self.window.height = self.window.height.clamp(48.0, 800.0);
        self.alert.timeout_streak = self.alert.timeout_streak.max(1);
        self.alert.high_ping_streak = self.alert.high_ping_streak.max(1);
        self.alert.beep_count = self.alert.beep_count.clamp(1, 10);
        self.alert.beep_ms = self.alert.beep_ms.clamp(20, 2000);
        self.alert.beep_freq_hz = self.alert.beep_freq_hz.clamp(37, 32767);
        self.alert.high_ping_ms = self.alert.high_ping_ms.max(1);
        self.target = self.target.trim().to_owned();
        if self.target.trim().is_empty() {
            self.target = "8.8.8.8".into();
        }
        self
    }

    pub fn display_name(&self) -> &str {
        if self.label.trim().is_empty() { &self.target } else { &self.label }
    }
}
