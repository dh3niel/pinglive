//! Persistent ping history.
//!
//! Every sample is appended to `%APPDATA%\PingLive\history\YYYY-MM-DD.bin`
//! (local date), 6 bytes per record: `u32` LE unix seconds + `u16` LE value
//! (round trip in ms, `0xFFFF` = timeout, `0xFFFE` = name did not resolve).
//! At one ping a second that is about 0.5 MB a day.
//!
//! The dashboard does not read the raw files while it is open: the last
//! `MEMORY_DAYS` are folded into per-minute buckets once at startup and the
//! live samples keep those buckets current, so every chart is a cheap lookup.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate, TimeZone};

use crate::ping::Sample;

const TIMEOUT: u16 = 0xFFFF;
const UNRESOLVED: u16 = 0xFFFE;
const RECORD: usize = 6;
/// Days of history folded into memory for the dashboard (30d view + today).
pub const MEMORY_DAYS: i64 = 31;

/// Aggregate over any span of samples: one minute, an hour, a day...
#[derive(Clone, Copy, Default, Debug)]
pub struct Agg {
    pub n: u32,
    pub ok: u32,
    pub sum: u64,
    /// Only meaningful when `ok > 0`.
    pub min: u32,
    pub max: u32,
    /// Timeouts and unresolved names.
    pub lost: u32,
    /// Replies at or above the high-ping threshold ("lag").
    pub high: u32,
    pub jit_sum: u64,
    pub jit_n: u32,
}

impl Agg {
    fn add(&mut self, s: Sample, high_ms: u32, prev: &mut Option<u32>) {
        self.n += 1;
        match s {
            Sample::Reply(ms) => {
                self.min = if self.ok == 0 { ms } else { self.min.min(ms) };
                self.max = self.max.max(ms);
                self.ok += 1;
                self.sum += ms as u64;
                if ms >= high_ms {
                    self.high += 1;
                }
                if let Some(p) = *prev {
                    self.jit_sum += (ms as i64 - p as i64).unsigned_abs();
                    self.jit_n += 1;
                }
                *prev = Some(ms);
            }
            Sample::Timeout | Sample::Unresolved => {
                self.lost += 1;
                *prev = None;
            }
        }
    }

    pub fn merge(&mut self, o: &Agg) {
        if o.ok > 0 {
            self.min = if self.ok == 0 { o.min } else { self.min.min(o.min) };
        }
        self.n += o.n;
        self.ok += o.ok;
        self.sum += o.sum;
        self.max = self.max.max(o.max);
        self.lost += o.lost;
        self.high += o.high;
        self.jit_sum += o.jit_sum;
        self.jit_n += o.jit_n;
    }

    pub fn avg(&self) -> Option<u32> {
        (self.ok > 0).then(|| (self.sum / self.ok as u64) as u32)
    }

    pub fn min(&self) -> Option<u32> {
        (self.ok > 0).then_some(self.min)
    }

    pub fn max(&self) -> Option<u32> {
        (self.ok > 0).then_some(self.max)
    }

    pub fn jitter(&self) -> Option<u32> {
        (self.jit_n > 0).then(|| (self.jit_sum / self.jit_n as u64) as u32)
    }

    pub fn loss_pct(&self) -> f32 {
        if self.n == 0 {
            0.0
        } else {
            self.lost as f32 * 100.0 / self.n as f32
        }
    }
}

pub struct History {
    dir: PathBuf,
    file: Option<(NaiveDate, BufWriter<File>)>,
    unflushed: u32,
    last_flush: Instant,
    /// Key: unix minute (`secs / 60`).
    minutes: BTreeMap<i64, Agg>,
    prev: Option<u32>,
    high_ms: u32,
}

pub fn history_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("PingLive")
        .join("history")
}

fn local_date(at: i64) -> NaiveDate {
    Local
        .timestamp_opt(at, 0)
        .single()
        .map(|d| d.date_naive())
        .unwrap_or_else(|| Local::now().date_naive())
}

fn encode(s: Sample) -> u16 {
    match s {
        Sample::Reply(ms) => ms.min(0xFFFD) as u16,
        Sample::Timeout => TIMEOUT,
        Sample::Unresolved => UNRESOLVED,
    }
}

fn decode(v: u16) -> Sample {
    match v {
        TIMEOUT => Sample::Timeout,
        UNRESOLVED => Sample::Unresolved,
        ms => Sample::Reply(ms as u32),
    }
}

impl History {
    /// Deletes day files older than `retention_days` (0 = keep forever) and
    /// loads the recent ones into memory.
    pub fn open(high_ms: u32, retention_days: u32) -> Self {
        let dir = history_dir();
        let _ = fs::create_dir_all(&dir);
        let mut h = Self {
            dir,
            file: None,
            unflushed: 0,
            last_flush: Instant::now(),
            minutes: BTreeMap::new(),
            prev: None,
            high_ms,
        };
        h.prune(retention_days);
        h.reload(high_ms);
        h
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn day_path(&self, day: NaiveDate) -> PathBuf {
        self.dir.join(format!("{}.bin", day.format("%Y-%m-%d")))
    }

    fn prune(&self, retention_days: u32) {
        if retention_days == 0 {
            return;
        }
        let cutoff = Local::now().date_naive() - chrono::Duration::days(retention_days as i64);
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".bin")) else {
                continue;
            };
            if let Ok(day) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
                if day < cutoff {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }

    /// Rebuilds the in-memory buckets from disk, e.g. after the lag threshold
    /// changed (the "lag" counts depend on it).
    pub fn reload(&mut self, high_ms: u32) {
        self.flush();
        self.high_ms = high_ms;
        self.minutes.clear();
        let today = Local::now().date_naive();
        for back in (0..MEMORY_DAYS).rev() {
            let day = today - chrono::Duration::days(back);
            let Ok(bytes) = fs::read(self.day_path(day)) else { continue };
            let mut prev = None;
            let mut last_at = 0i64;
            for rec in bytes.chunks_exact(RECORD) {
                let at = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]) as i64;
                let v = u16::from_le_bytes([rec[4], rec[5]]);
                // A gap (app closed, PC asleep) must not count as jitter.
                if at - last_at > 10 {
                    prev = None;
                }
                last_at = at;
                self.minutes
                    .entry(at.div_euclid(60))
                    .or_default()
                    .add(decode(v), high_ms, &mut prev);
            }
        }
    }

    pub fn record(&mut self, s: Sample, at: i64) {
        self.minutes
            .entry(at.div_euclid(60))
            .or_default()
            .add(s, self.high_ms, &mut self.prev);

        let day = local_date(at);
        if self.file.as_ref().map(|(d, _)| *d) != Some(day) {
            self.flush();
            self.file = self.open_day(day).map(|f| (day, BufWriter::new(f)));
            // A new day: drop buckets that fell out of the dashboard window.
            let keep_from = (at - MEMORY_DAYS * 86_400).div_euclid(60);
            self.minutes = self.minutes.split_off(&keep_from);
        }
        if let Some((_, w)) = self.file.as_mut() {
            let mut rec = [0u8; RECORD];
            rec[..4].copy_from_slice(&(at as u32).to_le_bytes());
            rec[4..].copy_from_slice(&encode(s).to_le_bytes());
            let _ = w.write_all(&rec);
            self.unflushed += 1;
        }
        // Batch the disk writes: at most one flush every 30 s.
        if self.last_flush.elapsed() >= Duration::from_secs(30) {
            self.flush();
        }
    }

    fn open_day(&self, day: NaiveDate) -> Option<File> {
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.day_path(day))
            .ok()?;
        // A crash mid-write can leave a partial record; cut it off so the
        // following records stay aligned.
        if let Ok(meta) = f.metadata() {
            let len = meta.len();
            if len % RECORD as u64 != 0 {
                let _ = f.set_len(len - len % RECORD as u64);
            }
        }
        Some(f)
    }

    pub fn flush(&mut self) {
        self.last_flush = Instant::now();
        if self.unflushed == 0 {
            return;
        }
        self.unflushed = 0;
        if let Some((_, w)) = self.file.as_mut() {
            let _ = w.flush();
        }
    }

    pub fn minute(&self, minute: i64) -> Option<&Agg> {
        self.minutes.get(&minute)
    }

    /// Aggregate over unix minutes `[from, to)`.
    pub fn range(&self, from: i64, to: i64) -> Agg {
        let mut a = Agg::default();
        if from < to {
            for b in self.minutes.range(from..to).map(|(_, b)| b) {
                a.merge(b);
            }
        }
        a
    }

    /// Per-minute buckets in `[from, to)`, for "worst minutes" lists.
    pub fn minutes_in(&self, from: i64, to: i64) -> impl Iterator<Item = (&i64, &Agg)> {
        self.minutes.range(from..to.max(from))
    }
}

impl Drop for History {
    fn drop(&mut self) {
        self.flush();
    }
}
