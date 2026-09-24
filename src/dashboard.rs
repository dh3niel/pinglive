//! The dashboard window: summaries for this hour / today / 24 h, heatmaps of
//! when timeouts (RTO) and lag happen, and the settings editor.
//!
//! Everything is computed from the per-minute buckets in `History`, so a
//! frame costs a few thousand map lookups - nothing is read from disk here.

use chrono::{Local, NaiveDate, TimeZone, Timelike};
use egui::{
    Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2, ViewportCommand,
    ViewportId,
};

use crate::config::Config;
use crate::history::{Agg, History};
use crate::sound::Player;

pub const TITLE: &str = "PingLive Dashboard";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    Week,
    Month,
}

impl Span {
    fn days(self) -> i64 {
        match self {
            Span::Week => 7,
            Span::Month => 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    Rto,
    Lag,
    Avg,
}

pub struct Dashboard {
    pub open: bool,
    pub tab: Tab,
    focus_pending: bool,
    span: Span,
    metric: Metric,
    draft: Option<Config>,
    autostart: bool,
    note: String,
}

/// What the dashboard asks the app to do after a frame.
pub enum Request {
    Apply(Config),
    OpenHistoryFolder,
}

const NO_DATA: Color32 = Color32::from_gray(40);
const CALM: Color32 = Color32::from_rgb(38, 96, 64);
const MUTED: Color32 = Color32::from_rgb(150, 156, 168);
const RED: Color32 = Color32::from_rgb(250, 95, 85);

impl Dashboard {
    pub fn new() -> Self {
        Self {
            open: false,
            tab: Tab::Overview,
            focus_pending: false,
            span: Span::Week,
            metric: Metric::Rto,
            draft: None,
            autostart: false,
            note: String::new(),
        }
    }

    pub fn viewport_id() -> ViewportId {
        ViewportId::from_hash_of("pinglive-dashboard")
    }

    pub fn show_tab(&mut self, tab: Tab) {
        self.open = true;
        self.tab = tab;
        self.focus_pending = true;
        if tab == Tab::Settings {
            self.draft = None; // start from the live config
        }
    }

    /// Draws the whole window; `live` is the overlay's current one-liner.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        hist: &History,
        cfg: &Config,
        sound: &Player,
        live: &str,
    ) -> Option<Request> {
        if self.focus_pending {
            self.focus_pending = false;
            self.autostart = crate::win::autostart_enabled();
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.open = false;
        }

        let mut req = None;
        egui::TopBottomPanel::top("dash-tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Overview, "Overview");
                if ui
                    .selectable_value(&mut self.tab, Tab::Settings, "Settings")
                    .clicked()
                {
                    self.draft = None;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(live).monospace());
                });
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                Tab::Overview => self.overview(ui, hist, cfg),
                Tab::Settings => req = self.settings(ui, cfg, sound),
            });
        });
        req
    }

    // ---- Overview ---------------------------------------------------------

    fn overview(&mut self, ui: &mut Ui, hist: &History, cfg: &Config) {
        let now = Local::now();
        let now_s = now.timestamp();
        let now_m = now_s.div_euclid(60);
        let hour_start = now_s - (now.minute() * 60 + now.second()) as i64;
        let day_start = local_ts(now.date_naive(), 0);

        ui.columns(3, |cols| {
            card(
                &mut cols[0],
                &format!("This hour ({:02}:00 - now)", now.hour()),
                &hist.range(hour_start / 60, now_m + 1),
                cfg,
            );
            card(&mut cols[1], "Today", &hist.range(day_start / 60, now_m + 1), cfg);
            card(
                &mut cols[2],
                "Last 24 hours",
                &hist.range(now_m - 24 * 60 + 1, now_m + 1),
                cfg,
            );
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Heatmaps show").strong());
            ui.selectable_value(&mut self.metric, Metric::Rto, "Timeouts (RTO)");
            ui.selectable_value(
                &mut self.metric,
                Metric::Lag,
                format!("Lag >= {} ms", cfg.alert.high_ping_ms),
            );
            ui.selectable_value(&mut self.metric, Metric::Avg, "Average ping");
        });
        legend(ui, self.metric, cfg);

        // Last 24 h, one cell per minute: rows are clock hours, oldest on top.
        ui.add_space(6.0);
        ui.label(RichText::new("Last 24 hours - per minute").strong());
        let first_row = hour_start - 23 * 3600;
        let mut cells = Vec::with_capacity(24 * 60);
        for r in 0..24i64 {
            for c in 0..60i64 {
                let m = (first_row + r * 3600) / 60 + c;
                cells.push((m <= now_m).then(|| hist.minute(m).copied().unwrap_or_default()));
            }
        }
        heat_grid(
            ui,
            &cells,
            (24, 60),
            12.0,
            &|r| fmt_local(first_row + r as i64 * 3600, "%H:00"),
            &|c| (c % 10 == 0).then(|| format!(":{c:02}")),
            &|r, c| fmt_local(first_row + r as i64 * 3600 + c as i64 * 60, "%a %d %b  %H:%M"),
            self.metric,
            cfg,
        );

        // Day x hour for the last 7 / 30 days.
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("By day and hour").strong());
            ui.selectable_value(&mut self.span, Span::Week, "7 days");
            ui.selectable_value(&mut self.span, Span::Month, "30 days");
        });
        let days = self.span.days();
        let today = now.date_naive();
        let dates: Vec<NaiveDate> = (0..days)
            .rev()
            .map(|b| today - chrono::Duration::days(b))
            .collect();
        let mut cells = Vec::with_capacity(dates.len() * 24);
        let mut by_hour = [Agg::default(); 24];
        for d in &dates {
            for h in 0..24u32 {
                let start = local_ts(*d, h);
                if start > now_s {
                    cells.push(None);
                    continue;
                }
                let a = hist.range(start / 60, (start + 3600) / 60);
                by_hour[h as usize].merge(&a);
                cells.push(Some(a));
            }
        }
        let row_h = if days <= 7 { 22.0 } else { 13.0 };
        heat_grid(
            ui,
            &cells,
            (dates.len(), 24),
            row_h,
            &|r| dates[r].format("%a %d %b").to_string(),
            &|c| (c % 3 == 0).then(|| format!("{c:02}")),
            &|r, c| format!("{}  {c:02}:00 - {:02}:00", dates[r].format("%a %d %b"), c + 1),
            self.metric,
            cfg,
        );

        // Hour-of-day profile across the span: *when* does it usually happen.
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!(
                "Time of day - last {days} days combined ({})",
                metric_name(self.metric, cfg)
            ))
            .strong(),
        );
        hour_profile(ui, &by_hour, self.metric, cfg);
        worst_hours(ui, &by_hour, self.metric);

        ui.add_space(12.0);
        ui.label(RichText::new(format!("Worst minutes - last {days} days")).strong());
        worst_minutes(ui, hist, (now_s - days * 86_400) / 60, now_m + 1, cfg);
    }

    // ---- Settings ---------------------------------------------------------

    fn settings(&mut self, ui: &mut Ui, cfg: &Config, sound: &Player) -> Option<Request> {
        let draft = self.draft.get_or_insert_with(|| cfg.clone());
        let mut req = None;

        section(ui, "Target");
        egui::Grid::new("set-target").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Host or IP");
            ui.text_edit_singleline(&mut draft.target);
            ui.end_row();
            ui.label("Label (optional)");
            ui.text_edit_singleline(&mut draft.label);
            ui.end_row();
            ui.label("Ping every (ms)");
            ui.add(egui::DragValue::new(&mut draft.interval_ms).range(100..=60_000).speed(10));
            ui.end_row();
            ui.label("Timeout after (ms)");
            ui.add(egui::DragValue::new(&mut draft.timeout_ms).range(100..=10_000).speed(10));
            ui.end_row();
        });

        section(ui, "Overlay");
        egui::Grid::new("set-overlay").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Background opacity");
            ui.add(egui::Slider::new(&mut draft.window.opacity, 0.0..=1.0));
            ui.end_row();
            ui.label("Show graph");
            ui.checkbox(&mut draft.window.show_graph, "");
            ui.end_row();
            ui.label("Size (w x h)");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut draft.window.width).range(140.0..=1200.0));
                ui.add(egui::DragValue::new(&mut draft.window.height).range(48.0..=800.0));
            });
            ui.end_row();
            ui.label("Graph samples");
            ui.add(egui::DragValue::new(&mut draft.history).range(20..=2000));
            ui.end_row();
            ui.label("Green up to (ms)");
            ui.add(egui::DragValue::new(&mut draft.colors.good_ms).range(1..=2000));
            ui.end_row();
            ui.label("Yellow up to (ms)");
            ui.add(egui::DragValue::new(&mut draft.colors.warn_ms).range(1..=5000));
            ui.end_row();
        });

        section(ui, "Alert sound");
        egui::Grid::new("set-alert").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Enabled");
            ui.checkbox(&mut draft.alert.enabled, "");
            ui.end_row();
            ui.label("Beep on timeouts in a row");
            ui.add(egui::DragValue::new(&mut draft.alert.timeout_streak).range(1..=100));
            ui.end_row();
            ui.label("Beep / count as lag at (ms)");
            ui.add(egui::DragValue::new(&mut draft.alert.high_ping_ms).range(20..=5000));
            ui.end_row();
            ui.label("...for samples in a row");
            ui.add(egui::DragValue::new(&mut draft.alert.high_ping_streak).range(1..=100));
            ui.end_row();
            ui.label("Minimum gap (s)");
            ui.add(egui::DragValue::new(&mut draft.alert.cooldown_secs).range(0..=600));
            ui.end_row();
            ui.label("Chime on recovery");
            ui.checkbox(&mut draft.alert.recovery_sound, "");
            ui.end_row();
            ui.label("Custom .wav (empty = beeps)");
            ui.add(egui::TextEdit::singleline(&mut draft.alert.sound_file).desired_width(320.0));
            ui.end_row();
            ui.label("Beep tone (Hz / ms / count)");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut draft.alert.beep_freq_hz).range(37..=20_000));
                ui.add(egui::DragValue::new(&mut draft.alert.beep_ms).range(20..=2000));
                ui.add(egui::DragValue::new(&mut draft.alert.beep_count).range(1..=10));
            });
            ui.end_row();
            ui.label("");
            if ui.button("Test sound").clicked() {
                sound.alarm(&draft.alert);
            }
            ui.end_row();
        });

        section(ui, "History");
        egui::Grid::new("set-history").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
            ui.label("Keep raw history (days, 0 = forever)");
            ui.add(egui::DragValue::new(&mut draft.retention_days).range(0..=3650));
            ui.end_row();
            ui.label("");
            if ui.button("Open history folder").clicked() {
                req = Some(Request::OpenHistoryFolder);
            }
            ui.end_row();
        });

        section(ui, "System");
        if crate::service::installed() {
            ui.label("Started at sign-in by the PingLive Windows service. Uninstall it from Settings > Apps > Installed apps.");
        } else if ui
            .checkbox(&mut self.autostart, "Start PingLive when I sign in to Windows")
            .changed()
        {
            let ok = crate::win::set_autostart(self.autostart);
            self.note = match (ok, self.autostart) {
                (true, true) => "Autostart on.".into(),
                (true, false) => "Autostart off.".into(),
                (false, _) => "Could not change autostart.".into(),
            };
            self.autostart = crate::win::autostart_enabled();
        }
        ui.label(
            RichText::new("Hotkeys: Ctrl+Alt+D dashboard, P move/click-through, H hide, M mute, G graph, R reset, Q quit")
                .color(MUTED),
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Save & apply").strong()).clicked() {
                req = Some(Request::Apply(draft.clone()));
                self.note = "Saved.".into();
            }
            if ui.button("Revert").clicked() {
                *draft = cfg.clone();
                self.note = "Reverted to the saved settings.".into();
            }
            ui.label(RichText::new(&self.note).color(MUTED));
        });
        req
    }
}

// ---- helpers ---------------------------------------------------------------

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.separator();
}

/// Unix seconds of `hour:00` local time on `day`.
fn local_ts(day: NaiveDate, hour: u32) -> i64 {
    day.and_hms_opt(hour, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map_or(0, |t| t.timestamp())
}

fn fmt_local(secs: i64, fmt: &str) -> String {
    Local
        .timestamp_opt(secs, 0)
        .single()
        .map(|t| t.format(fmt).to_string())
        .unwrap_or_default()
}

fn fmt_ms(v: Option<u32>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| format!("{v} ms"))
}

fn ms_color(ms: u32, cfg: &Config) -> Color32 {
    if ms <= cfg.colors.good_ms {
        Color32::from_rgb(80, 200, 110)
    } else if ms <= cfg.colors.warn_ms {
        Color32::from_rgb(235, 195, 70)
    } else if ms < cfg.alert.high_ping_ms {
        Color32::from_rgb(240, 140, 60)
    } else {
        RED
    }
}

/// Yellow for a single event, deepening to red as the share grows.
fn rate_color(k: u32, n: u32) -> Color32 {
    if k == 0 {
        return CALM;
    }
    let t = ((k as f32 / n.max(1) as f32) * 5.0).sqrt().clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
    Color32::from_rgb(lerp(240.0, 225.0), lerp(200.0, 35.0), lerp(60.0, 40.0))
}

fn cell_color(metric: Metric, a: &Agg, cfg: &Config) -> Color32 {
    if a.n == 0 {
        return NO_DATA;
    }
    match metric {
        Metric::Rto => rate_color(a.lost, a.n),
        Metric::Lag => rate_color(a.high, a.n),
        Metric::Avg => a.avg().map_or(RED, |ms| ms_color(ms, cfg)),
    }
}

fn metric_name(metric: Metric, cfg: &Config) -> String {
    match metric {
        Metric::Rto => "share of pings that timed out".into(),
        Metric::Lag => format!("share of pings >= {} ms", cfg.alert.high_ping_ms),
        Metric::Avg => "average ping".into(),
    }
}

fn metric_value(metric: Metric, a: &Agg) -> f32 {
    match metric {
        Metric::Rto => a.lost as f32 * 100.0 / a.n.max(1) as f32,
        Metric::Lag => a.high as f32 * 100.0 / a.n.max(1) as f32,
        Metric::Avg => a.avg().unwrap_or(0) as f32,
    }
}

fn fmt_metric(metric: Metric, a: &Agg) -> String {
    match metric {
        Metric::Rto => format!("{:.2}% RTO ({})", metric_value(metric, a), a.lost),
        Metric::Lag => format!("{:.2}% lag ({})", metric_value(metric, a), a.high),
        Metric::Avg => fmt_ms(a.avg()),
    }
}

fn legend(ui: &mut Ui, metric: Metric, cfg: &Config) {
    let items: Vec<(Color32, String)> = match metric {
        Metric::Rto | Metric::Lag => vec![
            (NO_DATA, "no data".into()),
            (CALM, "none".into()),
            (rate_color(1, 100), "some".into()),
            (rate_color(1, 5), "a lot".into()),
        ],
        Metric::Avg => vec![
            (NO_DATA, "no data".into()),
            (ms_color(0, cfg), format!("<= {}", cfg.colors.good_ms)),
            (ms_color(cfg.colors.warn_ms, cfg), format!("<= {}", cfg.colors.warn_ms)),
            (ms_color(cfg.colors.warn_ms + 1, cfg), format!("< {}", cfg.alert.high_ping_ms)),
            (RED, format!(">= {} ms / down", cfg.alert.high_ping_ms)),
        ],
    };
    ui.horizontal(|ui| {
        for (col, text) in items {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, col);
            ui.label(RichText::new(text).small().color(MUTED));
            ui.add_space(6.0);
        }
    });
}

fn card(ui: &mut Ui, title: &str, a: &Agg, cfg: &Config) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(title).strong());
        egui::Grid::new(title).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
            let avg_col = a.avg().map_or(MUTED, |v| ms_color(v, cfg));
            let rows: [(&str, String, Color32); 6] = [
                ("Average", fmt_ms(a.avg()), avg_col),
                (
                    "Min / max",
                    format!("{} / {}", fmt_ms(a.min()), fmt_ms(a.max())),
                    MUTED,
                ),
                ("Jitter", fmt_ms(a.jitter()), MUTED),
                (
                    "Timeouts (RTO)",
                    format!("{}  ({:.2}%)", a.lost, a.loss_pct()),
                    if a.lost > 0 { RED } else { MUTED },
                ),
                (
                    "Lag spikes",
                    format!("{}  (>= {} ms)", a.high, cfg.alert.high_ping_ms),
                    if a.high > 0 { Color32::from_rgb(240, 140, 60) } else { MUTED },
                ),
                ("Pings", a.n.to_string(), MUTED),
            ];
            for (k, v, col) in rows {
                ui.label(k);
                ui.label(RichText::new(v).color(col));
                ui.end_row();
            }
        });
    });
}

fn agg_tooltip(ui: &mut Ui, a: &Agg, cfg: &Config) {
    if a.n == 0 {
        ui.label("no data");
        return;
    }
    ui.label(format!("pings {}   timeouts {} ({:.2}%)", a.n, a.lost, a.loss_pct()));
    ui.label(format!(
        "avg {}   max {}   lag >= {} ms: {}",
        fmt_ms(a.avg()),
        fmt_ms(a.max()),
        cfg.alert.high_ping_ms,
        a.high
    ));
}

/// A rows x cols heatmap; `None` cells (the future) are left blank.
#[allow(clippy::too_many_arguments)]
fn heat_grid(
    ui: &mut Ui,
    cells: &[Option<Agg>],
    (rows, cols): (usize, usize),
    cell_h: f32,
    row_label: &dyn Fn(usize) -> String,
    col_header: &dyn Fn(usize) -> Option<String>,
    title: &dyn Fn(usize, usize) -> String,
    metric: Metric,
    cfg: &Config,
) {
    const HEADER: f32 = 14.0;
    const LABEL_W: f32 = 78.0;
    let size = Vec2::new(ui.available_width(), HEADER + rows as f32 * cell_h);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let left = rect.left() + LABEL_W;
    let cw = (rect.right() - left) / cols as f32;
    let font = FontId::proportional(10.0);
    let cell_rect = |r: usize, c: usize| {
        Rect::from_min_size(
            Pos2::new(left + c as f32 * cw, rect.top() + HEADER + r as f32 * cell_h),
            Vec2::new(cw, cell_h),
        )
        .shrink(0.5)
    };

    for c in 0..cols {
        if let Some(t) = col_header(c) {
            painter.text(
                Pos2::new(left + c as f32 * cw, rect.top()),
                Align2::LEFT_TOP,
                t,
                font.clone(),
                MUTED,
            );
        }
    }
    for r in 0..rows {
        painter.text(
            Pos2::new(rect.left(), cell_rect(r, 0).center().y),
            Align2::LEFT_CENTER,
            row_label(r),
            font.clone(),
            MUTED,
        );
        for c in 0..cols {
            if let Some(a) = &cells[r * cols + c] {
                painter.rect_filled(cell_rect(r, c), 1.0, cell_color(metric, a, cfg));
            }
        }
    }

    let Some(p) = resp.hover_pos() else { return };
    let (c, r) = ((p.x - left) / cw, (p.y - rect.top() - HEADER) / cell_h);
    if c < 0.0 || r < 0.0 || c as usize >= cols || r as usize >= rows {
        return;
    }
    let (r, c) = (r as usize, c as usize);
    let Some(a) = cells[r * cols + c] else { return };
    painter.rect_stroke(cell_rect(r, c), 1.0, Stroke::new(1.5_f32, Color32::WHITE));
    let t = title(r, c);
    resp.on_hover_ui_at_pointer(|ui| {
        ui.label(RichText::new(t).strong());
        agg_tooltip(ui, &a, cfg);
    });
}

fn hour_profile(ui: &mut Ui, hours: &[Agg; 24], metric: Metric, cfg: &Config) {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = Rect::from_min_max(rect.min + Vec2::new(0.0, 14.0), rect.max - Vec2::new(0.0, 14.0));
    let peak = hours
        .iter()
        .map(|a| metric_value(metric, a))
        .fold(0.0_f32, f32::max);
    let bw = plot.width() / 24.0;
    painter.text(
        rect.left_top(),
        Align2::LEFT_TOP,
        match metric {
            Metric::Avg => format!("peak {peak:.0} ms"),
            _ => format!("peak {peak:.2}%"),
        },
        FontId::proportional(10.0),
        MUTED,
    );
    for (h, a) in hours.iter().enumerate() {
        let x = plot.left() + h as f32 * bw;
        let v = metric_value(metric, a);
        if a.n > 0 && peak > 0.0 {
            let bh = (v / peak * plot.height()).max(if v > 0.0 { 2.0 } else { 0.0 });
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(x + 1.0, plot.bottom() - bh), Vec2::new(bw - 2.0, bh)),
                1.0,
                cell_color(metric, a, cfg),
            );
        }
        if h % 3 == 0 {
            painter.text(
                Pos2::new(x + 1.0, plot.bottom() + 2.0),
                Align2::LEFT_TOP,
                format!("{h:02}"),
                FontId::proportional(10.0),
                MUTED,
            );
        }
    }
    painter.line_segment(
        [plot.left_bottom(), plot.right_bottom()],
        Stroke::new(1.0_f32, Color32::from_gray(70)),
    );

    let Some(p) = resp.hover_pos() else { return };
    let h = ((p.x - plot.left()) / bw) as usize;
    if p.x < plot.left() || h >= 24 {
        return;
    }
    let a = hours[h];
    resp.on_hover_ui_at_pointer(|ui| {
        ui.label(RichText::new(format!("{h:02}:00 - {:02}:00", h + 1)).strong());
        agg_tooltip(ui, &a, cfg);
    });
}

fn worst_hours(ui: &mut Ui, hours: &[Agg; 24], metric: Metric) {
    let mut ranked: Vec<(usize, &Agg)> = hours
        .iter()
        .enumerate()
        .filter(|(_, a)| a.n > 0 && metric_value(metric, a) > 0.0)
        .collect();
    ranked.sort_by(|a, b| metric_value(metric, b.1).total_cmp(&metric_value(metric, a.1)));
    let text = if ranked.is_empty() {
        match metric {
            Metric::Avg => "No data yet.".to_owned(),
            _ => "Nothing recorded in this period.".to_owned(),
        }
    } else {
        let label = if metric == Metric::Avg { "Slowest" } else { "Most often" };
        let top: Vec<String> = ranked
            .iter()
            .take(3)
            .map(|(h, a)| format!("{h:02}:00-{:02}:00  {}", h + 1, fmt_metric(metric, a)))
            .collect();
        format!("{label}: {}", top.join("   |   "))
    };
    ui.label(RichText::new(text).color(if ranked.is_empty() { MUTED } else { RED }));
}

fn worst_minutes(ui: &mut Ui, hist: &History, from: i64, to: i64, cfg: &Config) {
    let mut bad: Vec<(i64, Agg)> = hist
        .minutes_in(from, to)
        .filter(|(_, a)| a.lost > 0 || a.high > 0)
        .map(|(m, a)| (*m, *a))
        .collect();
    if bad.is_empty() {
        ui.label(RichText::new("No timeouts or lag spikes in this period.").color(MUTED));
        return;
    }
    bad.sort_by(|(_, a), (_, b)| {
        (b.lost, b.high, b.max).cmp(&(a.lost, a.high, a.max))
    });
    egui::Grid::new("worst-min")
        .num_columns(5)
        .striped(true)
        .spacing([18.0, 3.0])
        .show(ui, |ui| {
            for h in ["Minute", "Timeouts", "Lag", "Max", "Avg"] {
                ui.label(RichText::new(h).color(MUTED));
            }
            ui.end_row();
            for (m, a) in bad.iter().take(10) {
                ui.label(fmt_local(m * 60, "%a %d %b  %H:%M"));
                ui.label(RichText::new(a.lost.to_string()).color(if a.lost > 0 { RED } else { MUTED }));
                ui.label(a.high.to_string());
                ui.label(fmt_ms(a.max()));
                ui.label(
                    RichText::new(fmt_ms(a.avg())).color(a.avg().map_or(RED, |v| ms_color(v, cfg))),
                );
                ui.end_row();
            }
        });
}
