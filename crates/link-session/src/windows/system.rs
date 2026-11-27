//! The PC's state for phones and the commands they send. A hidden window on its own thread hears lock and unlock
//! (session notifications), user presence, the display and the battery (power setting notifications), so nothing is
//! polled. Windows sends presence and the display only when they change, so both are unknown until then. Brightness,
//! Focus and the power mode stay out for now.

#![allow(unsafe_code, reason = "FFI: user32, wtsapi32, powrprof and advapi32 calls")]
#![allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::unused_self,
    reason = "the Linux backend's interface, whose calls wait"
)]

use std::cell::RefCell;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::{Arc, OnceLock};

use tokio::sync::watch;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, HWND, LPARAM, LRESULT, LUID, WPARAM,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{
    GetSystemPowerStatus, POWERBROADCAST_SETTING, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS,
    SetSuspendState,
};
use windows_sys::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    WTS_SESSIONSTATE_UNLOCK, WTSFreeMemory, WTSINFOEXW, WTSQuerySessionInformationW, WTSRegisterSessionNotification,
    WTSSessionInfoEx,
};
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_SESSION_DISPLAY_STATUS, GUID_SESSION_USER_PRESENCE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
    PBT_APMPOWERSTATUSCHANGE, PBT_POWERSETTINGCHANGE, PostMessageW, RegisterClassExW, SC_MONITORPOWER,
    WM_POWERBROADCAST, WM_SYSCOMMAND, WM_WTSSESSION_CHANGE, WNDCLASSEXW, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows_sys::core::{GUID, PWSTR, w};

/// `USER_ACTIVITY_PRESENCE`'s `PowerUserPresent`; the other values mean away or inactive.
const USER_PRESENT: u32 = 0;
/// `MONITOR_DISPLAY_STATE`'s `PowerMonitorOff`; on and dimmed count as on.
const DISPLAY_OFF: u32 = 0;
/// `SC_MONITORPOWER`'s argument that turns the display off.
const MONITOR_OFF: LPARAM = 2;
/// `SYSTEM_POWER_STATUS.BatteryFlag` bits and values.
const BATTERY_CHARGING: u8 = 8;
const NO_BATTERY: u8 = 128;
const UNKNOWN: u8 = 255;

/// The fields of `desktop-status` after volume and mute; `None` for what this desktop cannot tell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub battery: Option<(u8, bool)>,
    pub brightness: Option<u8>,
    pub locked: Option<bool>,
    pub idle: Option<bool>,
    pub dnd: Option<bool>,
    pub profile: Option<String>,
    pub screen_off: Option<bool>,
}

pub struct System {
    state: watch::Receiver<State>,
    /// The hidden window's address once the watcher thread made it.
    window: Arc<OnceLock<usize>>,
}

thread_local! {
    /// The watcher thread's end of [`System::state`], for its window procedure.
    static SENDER: RefCell<Option<watch::Sender<State>>> = const { RefCell::new(None) };
}

impl System {
    pub fn connect() -> Self {
        let first = State { battery: battery(), locked: locked(), ..State::default() };
        let (sender, state) = watch::channel(first);
        let window = Arc::new(OnceLock::new());
        let made = Arc::clone(&window);
        let spawned = std::thread::Builder::new().name("system-watch".into()).spawn(move || {
            if let Err(error) = watch_session(sender, &made) {
                log::warn!("watching lock, presence and battery: {error}");
            }
        });
        if let Err(error) = spawned {
            log::warn!("watching lock, presence and battery: {error}");
        }
        Self { state, window }
    }

    /// What phones can ask of this desktop, as `SetSessionCapabilities` lists it.
    pub fn capabilities(&self) -> Vec<&'static str> {
        vec!["lock", "suspend", "screen-off"]
    }

    pub async fn read(&self) -> State {
        self.state.borrow().clone()
    }

    /// Waits for a change; pending forever once the watcher has stopped.
    pub async fn changed(&mut self) {
        if self.state.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Carries out a phone's command other than volume and mute; the error is for the user.
    pub async fn run(&mut self, command: &str, _value: u32) -> Result<(), String> {
        match command {
            // SAFETY: no arguments; it fails off the interactive desktop.
            "lock" => match unsafe { LockWorkStation() } {
                0 => Err(format!("Lock failed: {}", io::Error::last_os_error())),
                _ => Ok(()),
            },
            "suspend" => match tokio::task::spawn_blocking(suspend).await {
                Ok(result) => result.map_err(|error| format!("Sleep failed: {error}")),
                Err(error) => Err(format!("Sleep failed: {error}")),
            },
            "screen-off" => {
                let Some(&window) = self.window.get() else {
                    return Err("Screen off failed: the PC's state window did not start".to_owned());
                };
                let window: HWND = std::ptr::with_exposed_provenance_mut(window);
                // SAFETY: the window lives until the process ends; its procedure hands the command to
                // DefWindowProcW, which turns the display off. Posting does not wait for that thread.
                match unsafe { PostMessageW(window, WM_SYSCOMMAND, SC_MONITORPOWER as WPARAM, MONITOR_OFF) } {
                    0 => Err(format!("Screen off failed: {}", io::Error::last_os_error())),
                    _ => Ok(()),
                }
            }
            other => Err(format!("{other} is not a command this PC takes")),
        }
    }
}

/// Runs the hidden window's message loop until the process ends.
fn watch_session(sender: watch::Sender<State>, made: &OnceLock<usize>) -> io::Result<()> {
    SENDER.with(|cell| *cell.borrow_mut() = Some(sender));
    let class = w!("AndromedaLinkSessionWatch");
    // SAFETY: null asks for this program's module.
    let instance = unsafe { GetModuleHandleW(null()) };
    let window_class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(u32::MAX),
        lpfnWndProc: Some(procedure),
        hInstance: instance,
        lpszClassName: class,
        ..WNDCLASSEXW::default()
    };
    // SAFETY: `window_class` is filled in and its class name is a static string.
    if unsafe { RegisterClassExW(&raw const window_class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // A top-level window that is never shown: a message-only window gets no broadcasts, and battery changes are one.
    // SAFETY: the class is registered above; no parent, menu or creation data.
    let window = unsafe { CreateWindowExW(0, class, class, 0, 0, 0, 0, 0, null_mut(), null_mut(), instance, null()) };
    if window.is_null() {
        return Err(io::Error::last_os_error());
    }
    made.get_or_init(|| window.expose_provenance());
    // SAFETY: `window` lives until the process ends, so neither registration needs undoing.
    if unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) } == 0 {
        log::warn!("lock and unlock notifications: {}", io::Error::last_os_error());
    }
    for (name, setting) in [
        ("presence", GUID_SESSION_USER_PRESENCE),
        ("display", GUID_SESSION_DISPLAY_STATUS),
        ("power source", GUID_ACDC_POWER_SOURCE),
        ("battery", GUID_BATTERY_PERCENTAGE_REMAINING),
    ] {
        // SAFETY: as above; `setting` lives for the call.
        let handle =
            unsafe { RegisterPowerSettingNotification(window, &raw const setting, DEVICE_NOTIFY_WINDOW_HANDLE) };
        if handle == 0 {
            log::warn!("{name} notifications: {}", io::Error::last_os_error());
        }
    }
    let mut message = MSG::default();
    // SAFETY: `message` is written by GetMessageW and read by DispatchMessageW on this thread.
    while unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) } > 0 {
        unsafe { DispatchMessageW(&raw const message) };
    }
    Ok(())
}

unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match (message, u32::try_from(wparam).unwrap_or(u32::MAX)) {
        (WM_WTSSESSION_CHANGE, WTS_SESSION_LOCK) => update(|state| state.locked = Some(true)),
        (WM_WTSSESSION_CHANGE, WTS_SESSION_UNLOCK) => update(|state| state.locked = Some(false)),
        (WM_POWERBROADCAST, PBT_APMPOWERSTATUSCHANGE) => update(|state| state.battery = battery()),
        (WM_POWERBROADCAST, PBT_POWERSETTINGCHANGE) => {
            // SAFETY: for this event `lparam` points at the setting, valid while the message is handled.
            let setting = unsafe { &*(lparam as *const POWERBROADCAST_SETTING) };
            setting_changed(setting);
        }
        _ => {}
    }
    // SAFETY: the arguments are the ones Windows passed in.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn setting_changed(setting: &POWERBROADCAST_SETTING) {
    let guid = setting.PowerSetting;
    if same(&guid, &GUID_ACDC_POWER_SOURCE) || same(&guid, &GUID_BATTERY_PERCENTAGE_REMAINING) {
        return update(|state| state.battery = battery());
    }
    if setting.DataLength < 4 {
        return;
    }
    // SAFETY: Windows gives `DataLength` bytes from `Data`, at least the 4 of a DWORD here.
    let value = unsafe { setting.Data.as_ptr().cast::<u32>().read_unaligned() };
    if same(&guid, &GUID_SESSION_USER_PRESENCE) {
        update(|state| state.idle = Some(value != USER_PRESENT));
    } else if same(&guid, &GUID_SESSION_DISPLAY_STATUS) {
        update(|state| state.screen_off = Some(value == DISPLAY_OFF));
    }
}

fn same(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

fn update(change: impl FnOnce(&mut State)) {
    SENDER.with(|cell| {
        if let Some(sender) = cell.borrow().as_ref() {
            sender.send_if_modified(|state| {
                let before = state.clone();
                change(state);
                *state != before
            });
        }
    });
}

/// The battery's (percent, charging or on mains power); none on a PC without one.
fn battery() -> Option<(u8, bool)> {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is written by the call.
    if unsafe { GetSystemPowerStatus(&raw mut status) } == 0
        || status.BatteryFlag & NO_BATTERY != 0
        || status.BatteryFlag == UNKNOWN
        || status.BatteryLifePercent > 100
    {
        return None;
    }
    Some((status.BatteryLifePercent, status.BatteryFlag & BATTERY_CHARGING != 0 || status.ACLineStatus == 1))
}

/// Whether this session is locked, as its session information tells.
fn locked() -> Option<bool> {
    let mut buffer: PWSTR = null_mut();
    let mut bytes = 0;
    // SAFETY: WTS allocates `buffer`, freed below.
    let queried = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &raw mut buffer,
            &raw mut bytes,
        )
    };
    if queried == 0 || buffer.is_null() {
        log::warn!("reading the lock state: {}", io::Error::last_os_error());
        return None;
    }
    // SAFETY: for WTSSessionInfoEx the buffer holds a WTSINFOEXW; level 1 is the only union member.
    let flags = unsafe {
        let info = buffer.cast::<WTSINFOEXW>().read_unaligned();
        (info.Level == 1).then_some(info.Data.WTSInfoExLevel1.SessionFlags)
    };
    // SAFETY: `buffer` came from WTSQuerySessionInformationW and is not used after this.
    unsafe { WTSFreeMemory(buffer.cast()) };
    match flags.and_then(|flags| u32::try_from(flags).ok()) {
        Some(WTS_SESSIONSTATE_LOCK) => Some(true),
        Some(WTS_SESSIONSTATE_UNLOCK) => Some(false),
        _ => None,
    }
}

/// Puts the PC to sleep. It needs the shutdown privilege enabled in the token; client Windows grants it to Users.
fn suspend() -> io::Result<()> {
    enable_shutdown_privilege()?;
    // SAFETY: plain flags: sleep, not hibernate; wake events stay on.
    if unsafe { SetSuspendState(false, false, false) } { Ok(()) } else { Err(io::Error::last_os_error()) }
}

fn enable_shutdown_privilege() -> io::Result<()> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo handle needs no closing; `token` is written on success and closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &raw mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = adjust(token);
    // SAFETY: `token` is open and not used after this.
    unsafe { CloseHandle(token) };
    result
}

fn adjust(token: HANDLE) -> io::Result<()> {
    let mut luid = LUID::default();
    // SAFETY: the name is a static wide string; `luid` is written on success.
    if unsafe { LookupPrivilegeValueW(null(), SE_SHUTDOWN_NAME, &raw mut luid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
    };
    // SAFETY: `privileges` lives for the call; no previous state is asked for.
    if unsafe { AdjustTokenPrivileges(token, 0, &raw const privileges, 0, null_mut(), null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // It succeeds without enabling a privilege the account lacks; only the last error tells.
    // SAFETY: no arguments.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(io::Error::from_raw_os_error(i32::try_from(ERROR_NOT_ALL_ASSIGNED).unwrap_or(i32::MAX)));
    }
    Ok(())
}
