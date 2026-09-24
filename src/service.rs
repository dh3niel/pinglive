//! Windows service, installer and uninstaller.
//!
//! A service runs in session 0 and can never draw on a user's desktop, so the
//! service does not show the overlay itself. It is a small supervisor: at
//! every sign-in it starts `pinglive.exe` inside that user's session (as that
//! user, on their desktop), restarts it if it crashes, and closes it cleanly
//! when the service is stopped. Quitting the overlay yourself (Ctrl+Alt+Q or
//! the tray) exits with code 0, and the service leaves it closed until the
//! next sign-in.
//!
//! `pinglive.exe --install`   copy to Program Files, register the service and
//!                            the "Installed apps" entry, start it (asks for admin)
//! `pinglive.exe --uninstall` stop and remove all of that again; your config and
//!                            ping history in %APPDATA%\PingLive are kept
//! `pinglive.exe --service`   what the Service Control Manager runs

use std::collections::HashMap;
use std::ffi::{c_void, OsString};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType, SessionChangeReason,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};

use crate::win::wide;

pub const SERVICE_NAME: &str = "PingLive";
const DISPLAY_NAME: &str = "PingLive Ping Overlay";
const DESCRIPTION: &str =
    "Starts the PingLive ping overlay in your desktop session at sign-in and restarts it if it crashes.";
/// Signalled by the service on stop; every overlay polls it and closes cleanly.
const STOP_EVENT: &str = r"Global\PingLiveServiceStop";
const UNINSTALL_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PingLive";
const EXE_NAME: &str = "pinglive.exe";

/// More crashes than this within `RESTART_WINDOW` and the service gives up on
/// that session until the next sign-in, instead of spinning.
const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(600);
const STILL_ACTIVE: u32 = 259;
/// Exit code of an overlay that found another copy already running in its
/// session (see `main::already_running`). Unlike 0 it is not a user quit, so
/// the service keeps trying (within the restart budget) until it owns the
/// session.
pub const EXIT_ALREADY_RUNNING: i32 = 3;

// ---- service ---------------------------------------------------------------

define_windows_service!(ffi_service_main, service_main);

/// Entry point for `--service`; blocks until the service stops.
pub fn run() {
    let _ = service_dispatcher::start(SERVICE_NAME, ffi_service_main);
}

enum Msg {
    Stop,
    Logon(u32),
    Connect(u32),
    Logoff(u32),
}

/// One overlay per signed-in session.
struct Session {
    process: HANDLE,
    /// The user quit the overlay themselves; leave it closed until next logon.
    quit: bool,
    launches: Vec<Instant>,
}

fn service_main(_args: Vec<OsString>) {
    let (tx, rx) = mpsc::channel();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(Msg::Stop);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(p) => {
            let id = p.notification.session_id;
            let msg = match p.reason {
                SessionChangeReason::SessionLogon => Some(Msg::Logon(id)),
                SessionChangeReason::ConsoleConnect | SessionChangeReason::RemoteConnect => {
                    Some(Msg::Connect(id))
                }
                SessionChangeReason::SessionLogoff => Some(Msg::Logoff(id)),
                _ => None,
            };
            if let Some(m) = msg {
                let _ = tx.send(m);
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status) = service_control_handler::register(SERVICE_NAME, handler) else {
        return;
    };
    let set = |state, accept| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        });
    };
    set(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN | ServiceControlAccept::SESSION_CHANGE,
    );

    let stop_event = create_stop_event();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(EXE_NAME));
    let mut sessions: HashMap<u32, Session> = HashMap::new();

    // Service (re)started while people are already signed in.
    for id in existing_sessions() {
        sessions.insert(id, Session::new());
    }

    loop {
        supervise(&mut sessions, &exe);
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Msg::Logon(id)) => match sessions.get_mut(&id) {
                // Already supervising a live overlay there (the session was
                // seeded at service start): just clear the quit/backoff state.
                Some(s) if s.alive() => {
                    s.quit = false;
                    s.launches.clear();
                }
                _ => {
                    if let Some(old) = sessions.insert(id, Session::new()) {
                        old.close();
                    }
                }
            },
            Ok(Msg::Connect(id)) => {
                sessions.entry(id).or_insert_with(Session::new);
            }
            Ok(Msg::Logoff(id)) => {
                if let Some(old) = sessions.remove(&id) {
                    old.close();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    set(ServiceState::StopPending, ServiceControlAccept::empty());
    stop_overlays(stop_event, sessions);
    set(ServiceState::Stopped, ServiceControlAccept::empty());
}

impl Session {
    fn new() -> Self {
        Session { process: null_mut(), quit: false, launches: Vec::new() }
    }

    fn alive(&self) -> bool {
        use windows_sys::Win32::System::Threading::GetExitCodeProcess;
        let mut code = 0u32;
        !self.process.is_null()
            && unsafe { GetExitCodeProcess(self.process, &mut code) } != 0
            && code == STILL_ACTIVE
    }

    fn close(self) {
        if !self.process.is_null() {
            unsafe { CloseHandle(self.process) };
        }
    }
}

/// Starts overlays that are missing and restarts ones that crashed.
fn supervise(sessions: &mut HashMap<u32, Session>, exe: &Path) {
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    for (&id, s) in sessions.iter_mut() {
        if s.quit {
            continue;
        }
        if !s.process.is_null() {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(s.process, &mut code) };
            if code == STILL_ACTIVE {
                continue;
            }
            unsafe { CloseHandle(s.process) };
            s.process = null_mut();
            if code == 0 {
                // The user quit the overlay.
                s.quit = true;
                continue;
            }
        }
        s.launches.retain(|t| t.elapsed() < RESTART_WINDOW);
        if s.launches.len() >= MAX_RESTARTS {
            continue;
        }
        match launch_in_session(id, exe) {
            Launch::Started(p) => {
                s.process = p;
                s.launches.push(Instant::now());
            }
            // Only a real attempt counts against the budget; a session with
            // nobody signed in yet is simply retried on the next tick.
            Launch::Failed => s.launches.push(Instant::now()),
            Launch::NoUser => {}
        }
    }
}

enum Launch {
    Started(HANDLE),
    NoUser,
    Failed,
}

/// Runs the overlay as the user signed in to `session`, on their desktop.
fn launch_in_session(session: u32, exe: &Path) -> Launch {
    use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows_sys::Win32::System::RemoteDesktop::WTSQueryUserToken;
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let app = wide(&exe.display().to_string());
    let mut cmdline = wide(&format!("\"{}\"", exe.display()));
    let dir = exe.parent().map(|d| wide(&d.display().to_string()));
    let mut desktop = wide(r"winsta0\default");
    unsafe {
        let mut token: HANDLE = null_mut();
        if WTSQueryUserToken(session, &mut token) == 0 {
            return Launch::NoUser; // nobody signed in to that session (yet)
        }
        let mut env: *mut c_void = null_mut();
        let have_env = CreateEnvironmentBlock(&mut env, token, 0) != 0;

        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessAsUserW(
            token,
            app.as_ptr(),
            cmdline.as_mut_ptr(),
            null(),
            null(),
            0,
            if have_env { CREATE_UNICODE_ENVIRONMENT } else { 0 },
            if have_env { env } else { null() },
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            &si,
            &mut pi,
        ) != 0;
        if have_env {
            DestroyEnvironmentBlock(env);
        }
        CloseHandle(token);
        if !ok {
            return Launch::Failed;
        }
        CloseHandle(pi.hThread);
        Launch::Started(pi.hProcess)
    }
}

/// Every session with a user in it, connected or not (session 0 is services).
fn existing_sessions() -> Vec<u32> {
    use windows_sys::Win32::System::RemoteDesktop::{
        WTSActive, WTSDisconnected, WTSEnumerateSessionsW, WTSFreeMemory, WTS_SESSION_INFOW,
    };
    let mut info: *mut WTS_SESSION_INFOW = null_mut();
    let mut count = 0u32;
    let mut ids = Vec::new();
    unsafe {
        if WTSEnumerateSessionsW(null_mut(), 0, 1, &mut info, &mut count) == 0 {
            return ids;
        }
        for s in std::slice::from_raw_parts(info, count as usize) {
            if s.SessionId != 0 && (s.State == WTSActive || s.State == WTSDisconnected) {
                ids.push(s.SessionId);
            }
        }
        WTSFreeMemory(info.cast());
    }
    ids
}

/// Asks every overlay to close (they flush their history on the way out),
/// and terminates any that have not gone after a few seconds.
fn stop_overlays(stop_event: HANDLE, sessions: HashMap<u32, Session>) {
    use windows_sys::Win32::System::Threading::{SetEvent, TerminateProcess, WaitForSingleObject};
    unsafe {
        if !stop_event.is_null() {
            SetEvent(stop_event);
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        for (_, s) in sessions {
            if !s.process.is_null() {
                let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
                if WaitForSingleObject(s.process, left) != WAIT_OBJECT_0 {
                    TerminateProcess(s.process, 1);
                }
            }
            s.close();
        }
    }
    // The event handle is left to process exit, so an overlay polling it in
    // the meantime still sees it signalled.
}

/// Manual-reset event any signed-in user may wait on (SYNCHRONIZE only).
fn create_stop_event() -> HANDLE {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent};

    let sddl = wide("D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x100000;;;AU)");
    let name = wide(STOP_EVENT);
    unsafe {
        let mut sd: *mut c_void = null_mut();
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, null_mut());
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let h = CreateEventW(if sd.is_null() { null() } else { &sa }, 1, 0, name.as_ptr());
        // An overlay can still hold the object from a previous service run,
        // in which case it comes back signalled.
        if !h.is_null() {
            ResetEvent(h);
        }
        h
    }
}

/// Called from the overlay's frame loop: has the service asked us to close?
pub fn stop_requested() -> bool {
    use windows_sys::Win32::System::Threading::{OpenEventW, WaitForSingleObject};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    static EVENT: AtomicUsize = AtomicUsize::new(0);

    let mut h = EVENT.load(Ordering::Relaxed) as HANDLE;
    if h.is_null() {
        let name = wide(STOP_EVENT);
        h = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
        if h.is_null() {
            return false; // no service running
        }
        EVENT.store(h as usize, Ordering::Relaxed);
    }
    unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 }
}

/// True when the PingLive service is registered on this machine. Checked
/// once: installing or uninstalling restarts the overlay anyway.
pub fn installed() -> bool {
    static INSTALLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .and_then(|m| m.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS))
            .is_ok()
    })
}

// ---- install / uninstall -------------------------------------------------

/// The elevated copy may be started from Downloads; never let it load DLLs
/// from next to the exe.
fn harden_dll_search() {
    use windows_sys::Win32::System::LibraryLoader::{
        SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };
}

pub fn install() {
    harden_dll_search();
    if !elevate_if_needed("--install") {
        return;
    }
    match try_install() {
        Ok(dir) => message(
            &format!(
                "PingLive is installed in\n{}\n\nThe PingLive service starts the overlay every time you sign in \
                 and restarts it if it crashes. It is listed under Settings > Apps > Installed apps, \
                 where you can uninstall it.",
                dir.display()
            ),
            false,
        ),
        Err(e) => message(&format!("Install failed:\n{e}"), true),
    }
}

fn try_install() -> Result<PathBuf, String> {
    let dir = install_dir().ok_or("Cannot find the Program Files folder.")?;
    let dest = dir.join(EXE_NAME);
    let src = std::env::current_exe().map_err(|e| e.to_string())?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| format!("Cannot open the Service Control Manager: {e}"))?;

    // Upgrade: stop the old service (and with it the overlays) so the exe
    // is not locked while it is replaced.
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::START
        | ServiceAccess::STOP
        | ServiceAccess::CHANGE_CONFIG;
    let existing = manager.open_service(SERVICE_NAME, access).ok();
    if let Some(svc) = &existing {
        stop_and_wait(svc);
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    if !same_file(&src, &dest) {
        if let Err(e) = copy_with_retry(&src, &dest) {
            // Leave the previous install running rather than stopped.
            if let Some(svc) = &existing {
                let _ = svc.start::<&str>(&[]);
            }
            return Err(e);
        }
    }

    let info = ServiceInfo {
        name: SERVICE_NAME.into(),
        display_name: DISPLAY_NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: dest.clone(),
        launch_arguments: vec!["--service".into()],
        dependencies: vec![],
        account_name: None, // LocalSystem: needed to start processes in user sessions
        account_password: None,
    };
    let svc = match existing {
        Some(svc) => {
            svc.change_config(&info).map_err(|e| format!("Cannot update the service: {e}"))?;
            svc
        }
        None => manager
            .create_service(&info, access)
            .map_err(|e| format!("Cannot create the service: {e}"))?,
    };
    let _ = svc.set_description(DESCRIPTION);
    let restart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) };
    let _ = svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    });

    write_uninstall_entry(&dir, &dest).map_err(|e| format!("Cannot register in Installed apps: {e}"))?;

    // The per-user "Start with Windows" entry would only start a second copy
    // that exits straight away; the service does that job now.
    crate::win::set_autostart(false);

    svc.start::<&str>(&[]).map_err(|e| format!("Installed, but the service did not start: {e}"))?;
    Ok(dir)
}

pub fn uninstall() {
    harden_dll_search();
    if !elevate_if_needed("--uninstall") {
        return;
    }
    match try_uninstall() {
        Ok(leftover) => {
            message(
            "PingLive has been removed.\n\nYour settings and ping history are still in %APPDATA%\\PingLive; \
             delete that folder if you do not want them.",
                false,
            );
            if let Some(dir) = leftover {
                delete_after_exit(&dir);
            }
        }
        Err(e) => message(&format!("Uninstall failed:\n{e}"), true),
    }
}

/// Returns the install folder when it still has to be deleted after this
/// process exits (uninstall started from the installed exe itself, which is
/// what Installed apps does).
fn try_uninstall() -> Result<Option<PathBuf>, String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("Cannot open the Service Control Manager: {e}"))?;
    if let Ok(svc) = manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        stop_and_wait(&svc);
        svc.delete().map_err(|e| format!("Cannot delete the service: {e}"))?;
    }

    let mut leftover = None;
    if let Some(dir) = install_dir().filter(|d| d.exists()) {
        let running_from_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| same_file(p, &dir)))
            .unwrap_or(false);
        if running_from_dir {
            leftover = Some(dir);
        } else {
            // On failure the Installed apps entry stays, so uninstall can be retried.
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("Cannot remove {}: {e}", dir.display()))?;
        }
    }

    delete_uninstall_entry();
    Ok(leftover)
}

/// A running exe cannot delete itself: a hidden cmd keeps trying for about
/// 20 s, which covers this process exiting right after the message box.
fn delete_after_exit(dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let d = dir.display();
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(format!(
            "/D /C for /L %i in (1,1,10) do (ping -n 3 127.0.0.1 >NUL & rd /S /Q \"{d}\" 2>NUL & if not exist \"{d}\" exit)"
        ))
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

fn stop_and_wait(svc: &windows_service::service::Service) {
    let _ = svc.stop();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match svc.query_status() {
            Ok(s) if s.current_state != ServiceState::Stopped => {
                std::thread::sleep(Duration::from_millis(250))
            }
            _ => break,
        }
    }
}

/// `C:\Program Files\PingLive`, from the shell's known-folder API: the
/// `ProgramFiles` environment variables can be overridden per user, which
/// would let a non-admin pick where a LocalSystem service is installed.
fn install_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};
    unsafe {
        let mut p: *mut u16 = null_mut();
        let hr = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, null_mut(), &mut p);
        let dir = (hr == 0 && !p.is_null()).then(|| {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(p, len))).join("PingLive")
        });
        CoTaskMemFree(p.cast());
        dir
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A just-stopped overlay can hold the exe for a moment after it exits.
fn copy_with_retry(src: &Path, dest: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..20 {
        match std::fs::copy(src, dest) {
            Ok(_) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "Cannot copy to {}: {last}\nClose any running PingLive and try again.",
        dest.display()
    ))
}

// ---- "Installed apps" entry ------------------------------------------------

fn write_uninstall_entry(dir: &Path, exe: &Path) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE,
        REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    };
    let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);
    let exe_s = exe.display().to_string();
    let strings = [
        ("DisplayName", "PingLive".to_owned()),
        ("DisplayVersion", env!("CARGO_PKG_VERSION").to_owned()),
        ("Publisher", env!("CARGO_PKG_AUTHORS").to_owned()),
        ("Comments", env!("CARGO_PKG_DESCRIPTION").to_owned()),
        ("InstallLocation", dir.display().to_string()),
        ("DisplayIcon", format!("{exe_s},0")),
        ("UninstallString", format!("\"{exe_s}\" --uninstall")),
        ("InstallDate", chrono::Local::now().format("%Y%m%d").to_string()),
    ];
    let dwords = [("NoModify", 1u32), ("NoRepair", 1), ("EstimatedSize", size_kb)];

    let path = wide(UNINSTALL_KEY);
    unsafe {
        let mut key: HKEY = null_mut();
        let r = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        );
        if r != 0 {
            return Err(format!("error {r}"));
        }
        let mut result = Ok(());
        for (name, value) in &strings {
            let (n, v) = (wide(name), wide(value));
            let r = RegSetValueExW(key, n.as_ptr(), 0, REG_SZ, v.as_ptr().cast(), (v.len() * 2) as u32);
            if r != 0 {
                result = Err(format!("error {r} writing {name}"));
            }
        }
        for (name, value) in &dwords {
            let n = wide(name);
            let r = RegSetValueExW(key, n.as_ptr(), 0, REG_DWORD, (value as *const u32).cast(), 4);
            if r != 0 {
                result = Err(format!("error {r} writing {name}"));
            }
        }
        RegCloseKey(key);
        result
    }
}

fn delete_uninstall_entry() {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_LOCAL_MACHINE};
    let path = wide(UNINSTALL_KEY);
    unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, path.as_ptr()) };
}

// ---- elevation & messages --------------------------------------------------

/// True when already elevated. Otherwise relaunches this exe with `arg`
/// through the UAC prompt and returns false (this copy should just exit).
fn elevate_if_needed(arg: &str) -> bool {
    use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if unsafe { IsUserAnAdmin() } != 0 {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(arg));
    let r = unsafe {
        ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), null(), SW_SHOWNORMAL)
    };
    if (r as isize) <= 32 {
        message("PingLive needs administrator rights for this.", true);
    }
    false
}

fn message(text: &str, error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
    };
    let (t, c) = (wide(text), wide("PingLive"));
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | icon | MB_SETFOREGROUND) };
}
