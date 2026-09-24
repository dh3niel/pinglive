// No console window: this runs as a background overlay, started at logon.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod dashboard;
mod history;
mod hotkeys;
mod ping;
mod service;
mod sound;
mod tray;
mod win;

/// Exact title of the overlay window; `win::make_popup` finds it by this.
pub const OVERLAY_TITLE: &str = "PingLive";

use egui::ViewportBuilder;

use crate::app::PingLive;
use crate::config::Config;

fn main() -> eframe::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("--service") => return Ok(service::run()),
        Some("--install") => return Ok(service::install()),
        Some("--uninstall") => return Ok(service::uninstall()),
        _ => {}
    }
    if already_running() {
        // Started twice (e.g. logon task plus a manual launch) - the first
        // instance keeps the screen to itself.
        std::process::exit(service::EXIT_ALREADY_RUNNING);
    }
    if service::installed() {
        // The service starts us at sign-in now; a leftover per-user Run
        // entry would only race it for the session.
        win::set_autostart(false);
    }

    let mut cfg = Config::load();
    apply_cli_overrides(&mut cfg);
    cfg.save();

    let hist = history::History::open(cfg.alert.high_ping_ms, cfg.retention_days);
    let hk = hotkeys::Hotkeys::register();

    let height = if cfg.window.show_graph {
        cfg.window.height
    } else {
        40.0
    };
    let viewport = ViewportBuilder::default()
        .with_title(OVERLAY_TITLE)
        .with_inner_size([cfg.window.width, height])
        .with_position([cfg.window.x, cfg.window.y])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false);
    // NOTE: mouse passthrough is deliberately NOT set here. Setting it on the
    // builder makes winit re-apply window styles at creation time, which puts
    // WS_CAPTION back and leaves a title bar painted over the readout. The
    // app sends MousePassthrough as a viewport command on its first frame.

    let options = eframe::NativeOptions {
        viewport,
        // The overlay is drawn over a game - no vsync stall, no MSAA cost.
        vsync: false,
        multisampling: 0,
        ..Default::default()
    };

    eframe::run_native(
        "PingLive",
        options,
        Box::new(move |_cc| Ok(Box::new(PingLive::new(cfg, hk, hist)))),
    )
}

/// `--target <host>`, `--interval <ms>`, `--timeout <ms>`, `--show`
/// override the config file for this run and are written back to it.
fn apply_cli_overrides(cfg: &mut Config) {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--target" | "-t" => {
                if let Some(v) = args.next() {
                    cfg.target = v;
                }
            }
            "--interval" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.interval_ms = v;
                }
            }
            "--timeout" => {
                if let Some(v) = args.next().and_then(|v| v.parse().ok()) {
                    cfg.timeout_ms = v;
                }
            }
            "--show" => cfg.window.click_through = false,
            _ => {}
        }
    }
}

/// A named mutex is the cheapest single-instance guard on Windows; the handle
/// is deliberately leaked so it lives exactly as long as the process.
fn already_running() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = "Local\\PingLiveOverlaySingleton"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        GetLastError() == ERROR_ALREADY_EXISTS
    }
}
