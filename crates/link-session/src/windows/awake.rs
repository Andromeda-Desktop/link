//! Keeps the PC and its display awake while a phone mirrors here or presents (the daemon's `KeepAwake`), with a power
//! request; Windows clears it too when the user puts the PC to sleep.

#![allow(unsafe_code, reason = "FFI: kernel32 power requests on a handle this struct owns")]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Power::{
    POWER_REQUEST_TYPE, PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
    PowerSetRequest,
};
use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows_sys::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

/// The display alone would not stop the PC sleeping.
const KINDS: [POWER_REQUEST_TYPE; 2] = [PowerRequestDisplayRequired, PowerRequestSystemRequired];

#[derive(Default)]
pub struct Awake {
    /// The power request, while it is set.
    request: Option<HANDLE>,
}

impl Awake {
    pub fn set(&mut self, on: bool) {
        match (on, self.request) {
            (true, None) => match create() {
                Ok(request) => {
                    log::info!("keeping the PC awake");
                    self.request = Some(request);
                }
                Err(error) => log::warn!("keeping the PC awake: {error}"),
            },
            (false, Some(request)) => {
                self.request = None;
                release(request);
                log::info!("the PC may sleep again");
            }
            _ => {}
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        if let Some(request) = self.request.take() {
            release(request);
        }
    }
}

fn create() -> std::io::Result<HANDLE> {
    let mut reason: Vec<u16> = "A phone shows its screen or presents".encode_utf16().chain([0]).collect();
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 { SimpleReasonString: reason.as_mut_ptr() },
    };
    // SAFETY: `context` and its string live for the call.
    let request = unsafe { PowerCreateRequest(&raw const context) };
    if request == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    for kind in KINDS {
        // SAFETY: `request` is the open handle from above.
        if unsafe { PowerSetRequest(request, kind) } == 0 {
            let error = std::io::Error::last_os_error();
            release(request);
            return Err(error);
        }
    }
    Ok(request)
}

fn release(request: HANDLE) {
    for kind in KINDS {
        // SAFETY: `request` is open until CloseHandle below; clearing a kind never set fails harmlessly.
        unsafe { PowerClearRequest(request, kind) };
    }
    // SAFETY: this struct owns `request` and drops it once.
    unsafe { CloseHandle(request) };
}
