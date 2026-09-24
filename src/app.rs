use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Vec2, ViewportBuilder,
    ViewportCommand, ViewportId,
};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};

use crate::config::Config;
use crate::dashboard::{self, Dashboard, Request, Tab};
use crate::history::History;
use crate::hotkeys::{Action, Hotkeys};
use crate::ping::{self, Sample, Stamped};
use crate::sound::Player;
use crate::tray::{Level, Tray, TrayCmd};

pub struct PingLive {
    cfg: Config,
    rx: Receiver<Stamped>,
    sound: Player,
    hotkeys: Option<Hotkeys>,
    hist: History,
    tray: Option<Tray>,
    dash: Dashboard,
    last_tray_update: Option<Instant>,
    /// Frame period for the heartbeat thread (see `win::spawn_heartbeat`).
    beat: Arc<AtomicU64>,

    history: VecDeque<Sample>,
    timeouts: u64,
    streak: u32,
    high_streak: u32,
    last_alert: Option<Instant>,
    was_down: bool,

    muted: bool,
    visible: bool,
    interactive: bool,
    dirty_since: Option<Instant>,
    passthrough_applied: Option<bool>,
    frames: u32,
    /// 0 = untouched, 1 = styles patched, 2 = nudged, 3 = settled.
    fix_stage: u8,
}

impl PingLive {
    pub fn new(cfg: Config, hotkeys: Option<Hotkeys>, hist: History) -> Self {
        let interactive = !cfg.window.click_through;
        // Built here, on the UI thread, because the tray needs its message loop.
        let tray = Tray::new(cfg.window.visible, false);
        let rx = ping::spawn(cfg.target.clone(), cfg.interval_ms, cfg.timeout_ms);
        let beat = Arc::new(AtomicU64::new(cfg.interval_ms));
        crate::win::spawn_heartbeat(beat.clone());
        Self {
            beat,
            visible: cfg.window.visible,
            hist,
            tray,
            dash: Dashboard::new(),
            last_tray_update: None,
            history: VecDeque::with_capacity(cfg.history),
            cfg,
            rx,
            sound: Player::new(),
            hotkeys,
            timeouts: 0,
            streak: 0,
            high_streak: 0,
            last_alert: None,
            was_down: false,
            muted: false,
            interactive,
            dirty_since: None,
            passthrough_applied: None,
            frames: 0,
            fix_stage: 0,
        }
    }

    fn drain_samples(&mut self) {
        while let Ok((sample, at)) = self.rx.try_recv() {
            self.hist.record(sample, at);
            if self.history.len() >= self.cfg.history {
                self.history.pop_front();
            }
            self.history.push_back(sample);

            match sample {
                Sample::Reply(ms) => {
                    self.streak = 0;
                    if self.was_down {
                        self.was_down = false;
                        if self.cfg.alert.enabled && self.cfg.alert.recovery_sound && !self.muted {
                            self.sound.recovered(&self.cfg.alert);
                        }
                    }
                    if ms >= self.cfg.alert.high_ping_ms {
                        self.high_streak += 1;
                        if self.high_streak >= self.cfg.alert.high_ping_streak {
                            self.fire_alert(false);
                        }
                    } else {
                        self.high_streak = 0;
                    }
                }
                Sample::Timeout | Sample::Unresolved => {
                    self.timeouts += 1;
                    self.streak += 1;
                    self.high_streak = 0;
                    if self.streak >= self.cfg.alert.timeout_streak {
                        self.was_down = true;
                        self.fire_alert(true);
                    }
                }
            }
        }
    }

    /// `timeout`: the request timed out; otherwise a ping spike.
    fn fire_alert(&mut self, timeout: bool) {
        if !self.cfg.alert.enabled || self.muted {
            return;
        }
        let cooldown = Duration::from_secs(self.cfg.alert.cooldown_secs);
        if self.last_alert.map_or(false, |t| t.elapsed() < cooldown) {
            return;
        }
        self.last_alert = Some(Instant::now());
        if timeout {
            self.sound.alarm(&self.cfg.alert);
        } else {
            self.sound.spike(&self.cfg.alert);
        }
    }

    fn stats(&self) -> Stats {
        let mut sum = 0u64;
        let mut n = 0u64;
        let mut min = u32::MAX;
        let mut max = 0u32;
        let mut lost = 0u64;
        let mut jitter_sum = 0u64;
        let mut jitter_n = 0u64;
        let mut prev: Option<u32> = None;

        for s in &self.history {
            match s {
                Sample::Reply(ms) => {
                    sum += *ms as u64;
                    n += 1;
                    min = min.min(*ms);
                    max = max.max(*ms);
                    if let Some(p) = prev {
                        jitter_sum += (*ms as i64 - p as i64).unsigned_abs();
                        jitter_n += 1;
                    }
                    prev = Some(*ms);
                }
                _ => {
                    lost += 1;
                    prev = None;
                }
            }
        }
        Stats {
            last: self.history.back().copied(),
            avg: if n > 0 { Some((sum / n) as u32) } else { None },
            min: if n > 0 { Some(min) } else { None },
            max: if n > 0 { Some(max) } else { None },
            jitter: if jitter_n > 0 { Some((jitter_sum / jitter_n) as u32) } else { None },
            loss_pct: if self.history.is_empty() {
                0.0
            } else {
                lost as f32 * 100.0 / self.history.len() as f32
            },
        }
    }

    fn color_for(&self, ms: u32) -> Color32 {
        if ms <= self.cfg.colors.good_ms {
            Color32::from_rgb(80, 220, 120)
        } else if ms <= self.cfg.colors.warn_ms {
            Color32::from_rgb(245, 200, 70)
        } else {
            Color32::from_rgb(250, 110, 90)
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        if self.hotkeys.is_none() {
            return;
        }
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            let action = self.hotkeys.as_ref().and_then(|h| h.action_for(ev.id));
            match action {
                Some(Action::ToggleInteractive) => {
                    self.interactive = !self.interactive;
                    self.cfg.window.click_through = !self.interactive;
                    self.mark_dirty();
                }
                Some(Action::ToggleMute) => self.muted = !self.muted,
                Some(Action::ToggleGraph) => {
                    self.cfg.window.show_graph = !self.cfg.window.show_graph;
                    let h = if self.cfg.window.show_graph {
                        self.cfg.window.height
                    } else {
                        40.0
                    };
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(
                        self.cfg.window.width,
                        h,
                    )));
                    self.mark_dirty();
                }
                Some(Action::ToggleVisible) => self.set_visible(!self.visible),
                Some(Action::Dashboard) => self.toggle_dashboard(),
                Some(Action::ResetStats) => {
                    self.history.clear();
                    self.timeouts = 0;
                    self.streak = 0;
                }
                Some(Action::Quit) => self.quit(ctx),
                None => {}
            }
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = self.tray.as_ref() else { return };
        let mut cmds = Vec::new();
        while let Some(cmd) = tray.poll() {
            cmds.push(cmd);
        }
        for cmd in cmds {
            match cmd {
                TrayCmd::ShowOverlay => self.set_visible(true),
                TrayCmd::ToggleOverlay => self.set_visible(!self.visible),
                TrayCmd::Dashboard => self.dash.show_tab(Tab::Overview),
                TrayCmd::Settings => self.dash.show_tab(Tab::Settings),
                TrayCmd::ToggleMute => self.muted = !self.muted,
                TrayCmd::Exit => self.quit(ctx),
            }
        }
        if let Some(tray) = self.tray.as_ref() {
            tray.sync_checks(self.visible, self.muted);
        }
    }

    fn toggle_dashboard(&mut self) {
        if self.dash.open {
            self.dash.open = false;
        } else {
            self.dash.show_tab(Tab::Overview);
        }
    }

    /// "Hidden" keeps the window alive but draws nothing and lets every click
    /// through: the UI loop keeps running, so the tray and hotkeys can always
    /// bring it back, which a truly hidden eframe window does not guarantee.
    fn set_visible(&mut self, on: bool) {
        if self.visible != on {
            self.visible = on;
            self.cfg.window.visible = on;
            self.mark_dirty();
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.persist();
        self.hist.flush();
        self.dash.open = false;
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
    }

    fn live_line(&self) -> String {
        let now = match self.history.back() {
            Some(Sample::Reply(ms)) => format!("{ms} ms"),
            Some(Sample::Timeout) => "TIMEOUT".to_owned(),
            Some(Sample::Unresolved) => "NO DNS".to_owned(),
            None => "...".to_owned(),
        };
        format!(
            "{}  {now}  TO {}{}",
            self.cfg.display_name(),
            self.timeouts,
            if self.muted { "  muted" } else { "" }
        )
    }

    /// Tray dot colour and tooltip, refreshed about once a second.
    fn update_tray(&mut self) {
        if self.last_tray_update.map_or(false, |t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.last_tray_update = Some(Instant::now());
        let level = match self.history.back() {
            None => Level::Idle,
            Some(Sample::Reply(ms)) if *ms <= self.cfg.colors.good_ms => Level::Good,
            Some(Sample::Reply(ms)) if *ms <= self.cfg.colors.warn_ms => Level::Warn,
            Some(Sample::Reply(_)) => Level::Bad,
            Some(_) => Level::Down,
        };
        let tip = format!("PingLive - {}", self.live_line());
        if let Some(tray) = self.tray.as_mut() {
            tray.show_status(level, tip);
        }
    }

    fn apply_settings(&mut self, ctx: &egui::Context, new: Config) {
        let mut new = new.sanitized();
        // Things the overlay itself owns stay as they are right now.
        new.window.x = self.cfg.window.x;
        new.window.y = self.cfg.window.y;
        new.window.click_through = self.cfg.window.click_through;
        new.window.visible = self.cfg.window.visible;

        let old = std::mem::replace(&mut self.cfg, new);
        let cfg = &self.cfg;
        if cfg.target != old.target
            || cfg.interval_ms != old.interval_ms
            || cfg.timeout_ms != old.timeout_ms
        {
            // Dropping the old receiver ends the old ping thread.
            self.rx = ping::spawn(cfg.target.clone(), cfg.interval_ms, cfg.timeout_ms);
            self.history.clear();
            self.streak = 0;
            self.high_streak = 0;
        }
        self.beat.store(cfg.interval_ms, Ordering::Relaxed);
        if cfg.alert.high_ping_ms != old.alert.high_ping_ms {
            self.hist.reload(cfg.alert.high_ping_ms);
        }
        while self.history.len() > cfg.history {
            self.history.pop_front();
        }
        if cfg.window.width != old.window.width
            || cfg.window.height != old.window.height
            || cfg.window.show_graph != old.window.show_graph
        {
            let size = self.overlay_size();
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::InnerSize(size));
        }
        self.persist();
    }

    fn overlay_size(&self) -> Vec2 {
        let h = if self.cfg.window.show_graph {
            self.cfg.window.height
        } else {
            40.0
        };
        Vec2::new(self.cfg.window.width, h)
    }

    fn show_dashboard(&mut self, ctx: &egui::Context) {
        if !self.dash.open {
            return;
        }
        let live = self.live_line();
        let builder = ViewportBuilder::default()
            .with_title(dashboard::TITLE)
            .with_inner_size([900.0, 780.0])
            .with_min_inner_size([620.0, 420.0]);
        let (dash, hist, cfg, sound) = (&mut self.dash, &self.hist, &self.cfg, &self.sound);
        let req = ctx.show_viewport_immediate(Dashboard::viewport_id(), builder, |dctx, _| {
            dash.ui(dctx, hist, cfg, sound, &live)
        });
        match req {
            Some(Request::Apply(new)) => self.apply_settings(ctx, new),
            Some(Request::OpenHistoryFolder) => crate::win::open_folder(self.hist.dir()),
            None => {}
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    /// Writes the config at most once every couple of seconds, so dragging the
    /// overlay around does not hammer the disk.
    fn persist_if_due(&mut self) {
        if let Some(t) = self.dirty_since {
            if t.elapsed() >= Duration::from_secs(2) {
                self.persist();
            }
        }
    }

    fn persist(&mut self) {
        self.dirty_since = None;
        self.cfg.save();
    }

    fn track_position(&mut self, ctx: &egui::Context) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        if let Some(rect) = outer {
            let (x, y) = (rect.min.x, rect.min.y);
            if (x - self.cfg.window.x).abs() > 1.0 || (y - self.cfg.window.y).abs() > 1.0 {
                self.cfg.window.x = x;
                self.cfg.window.y = y;
                self.mark_dirty();
            }
        }
    }

    fn draw_graph(&self, painter: &egui::Painter, rect: Rect) {
        painter.rect_filled(
            rect,
            Rounding::same(3.0),
            Color32::from_rgba_unmultiplied(0, 0, 0, 60),
        );
        if self.history.is_empty() {
            return;
        }

        // Scale so normal play sits in the lower half but spikes stay on
        // screen: never tighter than the warn threshold, never below the peak.
        let peak = self
            .history
            .iter()
            .filter_map(|s| match s {
                Sample::Reply(ms) => Some(*ms),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let scale = peak.max(self.cfg.colors.warn_ms).max(20) as f32 * 1.15;

        let n = self.cfg.history.max(1);
        let bar_w = rect.width() / n as f32;
        let start = n - self.history.len();

        for (ms, col) in [
            (
                self.cfg.colors.good_ms,
                Color32::from_rgba_unmultiplied(80, 220, 120, 45),
            ),
            (
                self.cfg.colors.warn_ms,
                Color32::from_rgba_unmultiplied(245, 200, 70, 45),
            ),
        ] {
            let y = rect.bottom() - (ms as f32 / scale) * rect.height();
            if y > rect.top() {
                painter.line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(1.0_f32, col),
                );
            }
        }

        for (i, sample) in self.history.iter().enumerate() {
            let x = rect.left() + (start + i) as f32 * bar_w;
            let w = (bar_w - 0.5).max(1.0);
            match sample {
                Sample::Reply(ms) => {
                    let h = ((*ms as f32 / scale) * rect.height()).clamp(1.0, rect.height());
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(x, rect.bottom() - h), Vec2::new(w, h)),
                        Rounding::ZERO,
                        self.color_for(*ms),
                    );
                }
                Sample::Timeout | Sample::Unresolved => {
                    painter.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(x, rect.top()),
                            Vec2::new(w.max(1.5), rect.height()),
                        ),
                        Rounding::ZERO,
                        Color32::from_rgba_unmultiplied(255, 60, 60, 190),
                    );
                }
            }
        }
    }
}

struct Stats {
    last: Option<Sample>,
    avg: Option<u32>,
    min: Option<u32>,
    max: Option<u32>,
    jitter: Option<u32>,
    loss_pct: f32,
}

impl eframe::App for PingLive {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        // Strip the title bar winit leaves on the window, then walk the size
        // by a pixel and back so the GL surface is reconfigured for the new
        // client area - without that nudge the window renders black.
        if self.fix_stage < 3 && self.frames > 2 {
            let Vec2 { x: w, y: h } = self.overlay_size();
            match self.fix_stage {
                0 => {
                    if crate::win::make_popup() {
                        self.fix_stage = 1;
                    }
                }
                1 => {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(w, h + 1.0)));
                    self.fix_stage = 2;
                }
                _ => {
                    ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(w, h)));
                    self.fix_stage = 3;
                }
            }
            ctx.request_repaint();
        }

        if crate::service::stop_requested() {
            // The PingLive service is stopping or being uninstalled.
            self.quit(ctx);
        }
        self.drain_samples();
        self.handle_hotkeys(ctx);
        self.handle_tray(ctx);
        self.update_tray();
        self.track_position(ctx);
        self.persist_if_due();
        self.show_dashboard(ctx);

        // No request_repaint_after here on purpose: frames are driven by the
        // heartbeat thread (one per ping interval, max 1 s), which picks up new
        // samples, hotkeys and tray clicks. eframe's own scheduler can lose a
        // request and freeze the UI for good - see `win::spawn_heartbeat`.

        // Only push the passthrough flag when it changes: sending it every
        // frame makes the window flicker on some drivers.
        let want_passthrough = !self.interactive || !self.visible;
        if self.passthrough_applied != Some(want_passthrough) {
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(want_passthrough));
            self.passthrough_applied = Some(want_passthrough);
        }
        if !self.visible {
            return; // hidden to the tray: a fully transparent, click-through window
        }

        let stats = self.stats();
        let panel = egui::Frame::none()
            .fill(Color32::from_rgba_unmultiplied(
                12,
                14,
                18,
                (self.cfg.window.opacity * 255.0) as u8,
            ))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(8.0, 6.0));

        egui::CentralPanel::default().frame(panel).show(ctx, |ui| {
            let full = ui.available_rect_before_wrap();

            // In interactive mode the whole panel is a drag handle.
            if self.interactive {
                let resp = ui.interact(full, ui.id().with("drag"), Sense::click_and_drag());
                if resp.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if resp.double_clicked() {
                    self.toggle_dashboard();
                }
            }

            let painter = ui.painter();

            let (value, value_col) = match stats.last {
                Some(Sample::Reply(ms)) => (format!("{ms} ms"), self.color_for(ms)),
                Some(Sample::Timeout) => ("TIMEOUT".to_owned(), Color32::from_rgb(255, 80, 80)),
                Some(Sample::Unresolved) => ("NO DNS".to_owned(), Color32::from_rgb(255, 140, 60)),
                None => ("...".to_owned(), Color32::GRAY),
            };

            painter.text(
                full.left_top(),
                Align2::LEFT_TOP,
                self.cfg.display_name(),
                FontId::proportional(12.0),
                Color32::from_rgb(190, 196, 208),
            );
            painter.text(
                full.right_top(),
                Align2::RIGHT_TOP,
                &value,
                FontId::proportional(17.0),
                value_col,
            );

            let line2_y = full.top() + 20.0;
            let avg = stats.avg.map(|v| v.to_string()).unwrap_or_else(|| "-".to_owned());
            let jit = stats.jitter.map(|v| v.to_string()).unwrap_or_else(|| "-".to_owned());
            let lo = stats.min.map(|v| v.to_string()).unwrap_or_else(|| "-".to_owned());
            let hi = stats.max.map(|v| v.to_string()).unwrap_or_else(|| "-".to_owned());
            painter.text(
                Pos2::new(full.left(), line2_y),
                Align2::LEFT_TOP,
                format!("avg {avg}  jit {jit}  {lo}/{hi}"),
                FontId::proportional(10.5),
                Color32::from_rgb(150, 156, 168),
            );

            let to_col = if self.timeouts > 0 {
                Color32::from_rgb(255, 110, 110)
            } else {
                Color32::from_rgb(120, 126, 138)
            };
            painter.text(
                Pos2::new(full.right(), line2_y),
                Align2::RIGHT_TOP,
                format!(
                    "TO {}  loss {:.0}%{}",
                    self.timeouts,
                    stats.loss_pct,
                    if self.muted { "  (muted)" } else { "" }
                ),
                FontId::proportional(10.5),
                to_col,
            );

            if self.cfg.window.show_graph {
                let graph = Rect::from_min_max(
                    Pos2::new(full.left(), full.top() + 36.0),
                    Pos2::new(full.right(), full.bottom()),
                );
                if graph.height() > 8.0 {
                    self.draw_graph(painter, graph);
                }
            }

            // Visible border while the overlay accepts the mouse, so it is
            // obvious when clicks are being stolen from the game.
            if self.interactive {
                painter.rect_stroke(
                    full.expand(3.0),
                    Rounding::same(6.0),
                    Stroke::new(1.0_f32, Color32::from_rgb(90, 170, 255)),
                );
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist();
        self.hist.flush();
    }
}
