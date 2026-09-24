//! Windows-specific window surgery.
//!
//! `ViewportBuilder::with_decorations(false)` is ignored by the winit build
//! eframe 0.29 pulls in on Windows 11: the window keeps `WS_CAPTION` and DWM
//! paints a title bar over the top of the client area - exactly where the ping
//! readout is. So we turn it into a real popup overlay ourselves.
//!
//! The important part is the follow-up resize. Changing the frame without one
//! leaves the GL surface configured for the old client area and the window
//! renders black, which is what an earlier attempt at this ran into.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
    SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, HWND_TOPMOST, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_BORDER, WS_CAPTION, WS_DLGFRAME,
    WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
    WS_THICKFRAME,
};

/// Turns the window into a borderless top-most popup.
/// Returns false if the window is not up yet, so the caller can retry.
pub fn make_popup() -> bool {
    unsafe {
        let mut hwnd: HWND = null_mut();
        EnumWindows(Some(find_own_window), &mut hwnd as *mut HWND as LPARAM);
        if hwnd.is_null() {
            return false;
        }

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        let popup = (style
            & !(WS_CAPTION
                | WS_THICKFRAME
                | WS_SYSMENU
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_BORDER
                | WS_DLGFRAME))
            | WS_POPUP;
        if popup != style {
            SetWindowLongPtrW(hwnd, GWL_STYLE, popup as isize);
        }

        // Keep it out of Alt-Tab and off the taskbar.
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            ((ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW) as isize,
        );

        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        true
    }
}

/// Repaints the overlay every `period_ms` by asking Windows directly
/// (`RedrawWindow` -> `WM_PAINT` -> winit `RedrawRequested` -> an egui frame).
///
/// eframe 0.29's own scheduler (`request_repaint_after`) occasionally loses a
/// request; egui then believes a repaint is still pending, never asks again,
/// and the whole UI - overlay, hotkeys, tray, dashboard - freezes for good
/// while the process stays alive. A paint message from the OS always gets
/// through, and every frame resets egui's bookkeeping, so this cannot stall.
pub fn spawn_heartbeat(period_ms: Arc<AtomicU64>) {
    use windows_sys::Win32::Graphics::Gdi::{RedrawWindow, RDW_INTERNALPAINT};
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

    std::thread::Builder::new()
        .name("pinglive-heartbeat".into())
        .spawn(move || {
            // HWND is a raw pointer (not Send); keep it as an integer here.
            let mut hwnd: isize = 0;
            loop {
                let ms = period_ms.load(Ordering::Relaxed).clamp(100, 1000);
                std::thread::sleep(std::time::Duration::from_millis(ms));
                unsafe {
                    if hwnd == 0 || IsWindow(hwnd as HWND) == 0 {
                        let mut h: HWND = null_mut();
                        EnumWindows(Some(find_own_window), &mut h as *mut HWND as LPARAM);
                        hwnd = h as isize;
                        if hwnd == 0 {
                            continue;
                        }
                    }
                    RedrawWindow(hwnd as HWND, std::ptr::null(), null_mut(), RDW_INTERNALPAINT);
                }
            }
        })
        .expect("spawn heartbeat thread");
}

/// `EnumWindows` callback: stores this process's overlay window (matched by
/// its exact title, so the dashboard and the helper windows winit, the tray
/// and the hotkey manager create are never picked) into the `HWND` at `out`.
unsafe extern "system" fn find_own_window(hwnd: HWND, out: LPARAM) -> i32 {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == GetCurrentProcessId() {
        let mut buf = [0u16; 64];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len > 0 && String::from_utf16_lossy(&buf[..len as usize]) == crate::OVERLAY_TITLE {
            *(out as *mut HWND) = hwnd;
            return 0; // stop enumerating
        }
    }
    1
}

// ---- Start with Windows ---------------------------------------------------
//
// A per-user `Run` entry under HKCU: no admin rights, no service (a service
// runs in session 0 and could never draw the overlay on your desktop).

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "PingLive";

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn autostart_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        ) == 0
    }
}

/// Points the Run entry at the currently running exe, or removes it.
pub fn set_autostart(on: bool) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };
    let (key, name) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            let cmd = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                cmd.as_ptr().cast(),
                (cmd.len() * 2) as u32,
            ) == 0
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            r == 0 || r == 2 // ERROR_FILE_NOT_FOUND: already off
        }
    }
}

/// Opens a folder in Explorer.
pub fn open_folder(path: &std::path::Path) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let (verb, target) = (wide("open"), wide(&path.display().to_string()));
    unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}
