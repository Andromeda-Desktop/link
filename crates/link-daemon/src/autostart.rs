//! Windows starts linkd at logon through the user's `Run` value, which linkd writes at start when it is missing or
//! names another executable; systemd units do this on Linux. The uninstaller removes the value.

#![allow(unsafe_code, reason = "FFI: advapi32 registry calls with NUL-terminated wide strings that outlive each call")]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use anyhow::bail;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Andromeda Link";
/// Microsoft's limit for a Run command line, in characters.
const MAX_COMMAND: usize = 260;

pub fn register() {
    if let Err(error) = ensure() {
        log::warn!("andromeda-linkd will not start at logon: {error:#}");
    }
}

fn ensure() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let command: Vec<u16> = OsStr::new("\"")
        .encode_wide()
        .chain(exe.as_os_str().encode_wide())
        .chain(OsStr::new("\"").encode_wide())
        .collect();
    if command.len() > MAX_COMMAND {
        bail!("the path {} is longer than a Run command line may be", exe.display());
    }
    let command: Vec<u16> = command.into_iter().chain([0]).collect();
    let (key, name) = (wide(RUN), wide(NAME));
    if current(&key, &name)?.as_deref() == Some(command.as_slice()) {
        return Ok(());
    }
    let bytes = u32::try_from(command.len() * 2)?;
    // SAFETY: key, name and command are NUL-terminated and live across the call; bytes counts command with its NUL.
    let status = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, command.as_ptr().cast(), bytes)
    };
    if status != ERROR_SUCCESS {
        bail!("RegSetKeyValueW: {}", std::io::Error::from_raw_os_error(status.cast_signed()));
    }
    log::info!("andromeda-linkd starts at logon from {}", exe.display());
    Ok(())
}

/// The value with its NUL, or None when it does not exist.
fn current(key: &[u16], name: &[u16]) -> anyhow::Result<Option<Vec<u16>>> {
    let mut buffer = vec![0u16; MAX_COMMAND + 1];
    let mut bytes = u32::try_from(buffer.len() * 2)?;
    // SAFETY: key and name are NUL-terminated; buffer holds `bytes` bytes and outlives the call.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut bytes,
        )
    };
    match status {
        ERROR_SUCCESS => {
            buffer.truncate(bytes as usize / 2);
            Ok(Some(buffer))
        }
        ERROR_FILE_NOT_FOUND => Ok(None),
        // Longer than a Run command line: not ours, so it is replaced.
        ERROR_MORE_DATA => Ok(Some(Vec::new())),
        _ => bail!("RegGetValueW: {}", std::io::Error::from_raw_os_error(status.cast_signed())),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
