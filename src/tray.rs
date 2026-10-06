//! Notification-area (tray) icon: a coloured dot that follows the live ping,
//! a tooltip with the current reading, and a menu to reach everything.
//!
//! Left double-click brings a hidden overlay back; right-click opens the menu.
//! Events are polled from the tray's channels on each UI frame; no callbacks
//! that wake the UI from outside (see ping.rs for why).

use tray_icon::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};
use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    ShowOverlay,
    ToggleOverlay,
    Dashboard,
    Settings,
    ToggleMute,
    /// Alert volume in percent.
    Volume(u8),
    TestSound,
    Exit,
}

/// Levels offered in the tray's Volume submenu, in percent.
const VOLUME_STEPS: [u8; 5] = [100, 75, 50, 25, 10];

/// Colour of the tray dot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Idle,
    Good,
    Warn,
    Bad,
    Down,
}

pub struct Tray {
    icon: TrayIcon,
    overlay: CheckMenuItem,
    mute: CheckMenuItem,
    volumes: Vec<(u8, CheckMenuItem)>,
    ids: Vec<(MenuId, TrayCmd)>,
    level: Level,
    tooltip: String,
}

impl Tray {
    pub fn new(overlay_visible: bool, muted: bool, volume: f32) -> Option<Self> {
        let overlay = CheckMenuItem::new("Show overlay", true, overlay_visible, None);
        let dashboard = MenuItem::new("Dashboard", true, None);
        let settings = MenuItem::new("Settings", true, None);
        let mute = CheckMenuItem::new("Mute alerts", true, muted, None);
        let volumes: Vec<(u8, CheckMenuItem)> = VOLUME_STEPS
            .iter()
            .map(|&p| (p, CheckMenuItem::new(format!("{p}%"), true, p == percent(volume), None)))
            .collect();
        let volume_menu = Submenu::new("Volume", true);
        for (_, item) in &volumes {
            volume_menu.append(item).ok()?;
        }
        let test = MenuItem::new("Test sound", true, None);
        let exit = MenuItem::new("Exit PingLive", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &dashboard,
            &overlay,
            &PredefinedMenuItem::separator(),
            &settings,
            &mute,
            &volume_menu,
            &test,
            &PredefinedMenuItem::separator(),
            &exit,
        ])
        .ok()?;

        let mut ids = vec![
            (overlay.id().clone(), TrayCmd::ToggleOverlay),
            (dashboard.id().clone(), TrayCmd::Dashboard),
            (settings.id().clone(), TrayCmd::Settings),
            (mute.id().clone(), TrayCmd::ToggleMute),
            (test.id().clone(), TrayCmd::TestSound),
            (exit.id().clone(), TrayCmd::Exit),
        ];
        ids.extend(volumes.iter().map(|(p, item)| (item.id().clone(), TrayCmd::Volume(*p))));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("PingLive")
            .with_icon(dot(Level::Idle))
            .build()
            .map_err(|e| eprintln!("pinglive: tray icon unavailable: {e}"))
            .ok()?;

        Some(Self {
            icon,
            overlay,
            mute,
            volumes,
            ids,
            level: Level::Idle,
            tooltip: String::new(),
        })
    }

    pub fn poll(&self) -> Option<TrayCmd> {
        while let Ok(e) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = e
            {
                return Some(TrayCmd::ShowOverlay);
            }
        }
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if let Some((_, cmd)) = self.ids.iter().find(|(id, _)| *id == e.id) {
                return Some(*cmd);
            }
        }
        None
    }

    /// Keeps the check marks in line with state changed from elsewhere
    /// (hotkeys, the dashboard). Clicking a check item flips it on its own.
    pub fn sync_checks(&self, overlay_visible: bool, muted: bool, volume: f32) {
        self.overlay.set_checked(overlay_visible);
        self.mute.set_checked(muted);
        for (p, item) in &self.volumes {
            item.set_checked(*p == percent(volume));
        }
    }

    /// Only touches the shell when something actually changed.
    pub fn show_status(&mut self, level: Level, tooltip: String) {
        if level != self.level {
            self.level = level;
            let _ = self.icon.set_icon(Some(dot(level)));
        }
        if tooltip != self.tooltip {
            let _ = self.icon.set_tooltip(Some(&tooltip));
            self.tooltip = tooltip;
        }
    }
}

fn percent(volume: f32) -> u8 {
    (volume.clamp(0.0, 1.0) * 100.0).round() as u8
}

/// A 32x32 anti-aliased dot with a dark rim, coloured by level.
fn dot(level: Level) -> Icon {
    const N: u32 = 32;
    let (r, g, b) = match level {
        Level::Idle => (150, 156, 168),
        Level::Good => (80, 220, 120),
        Level::Warn => (245, 200, 70),
        Level::Bad => (250, 110, 90),
        Level::Down => (235, 45, 45),
    };
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let alpha = (15.5 - d).clamp(0.0, 1.0);
            // 2 px dark rim so the dot reads on light and dark taskbars.
            let rim = (d - 11.5).clamp(0.0, 1.0);
            let mix = |v: u8| (v as f32 * (1.0 - rim) + 20.0 * rim) as u8;
            px.extend_from_slice(&[mix(r), mix(g), mix(b), (alpha * 255.0) as u8]);
        }
    }
    Icon::from_rgba(px, N, N).expect("valid icon buffer")
}
