//! DPAPI with user scope: only a process with this user's logon credential, usually on this computer, decrypts.

#![allow(unsafe_code, reason = "FFI: crypt32 calls on blobs that outlive each call; LocalFree on their outputs")]

use std::{io, ptr, slice};

use anyhow::bail;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

pub fn protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    let input = blob(plain)?;
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
    // SAFETY: input points at `plain` for cbData bytes; the null arguments are optional. On success output holds a
    // LocalAlloc buffer, which take() frees.
    let ok = unsafe {
        CryptProtectData(
            &raw const input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    };
    if ok == 0 {
        bail!("CryptProtectData: {}", io::Error::last_os_error());
    }
    Ok(take(output, false))
}

pub fn unprotect(sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
    let input = blob(sealed)?;
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: ptr::null_mut() };
    // SAFETY: as in protect(); a null description pointer asks for no description.
    let ok = unsafe {
        CryptUnprotectData(
            &raw const input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
    };
    if ok == 0 {
        bail!("CryptUnprotectData: {}", io::Error::last_os_error());
    }
    Ok(take(output, true))
}

fn blob(data: &[u8]) -> anyhow::Result<CRYPT_INTEGER_BLOB> {
    Ok(CRYPT_INTEGER_BLOB { cbData: u32::try_from(data.len())?, pbData: data.as_ptr().cast_mut() })
}

/// Copies a DPAPI output and frees it; a secret is zeroed before the free.
fn take(output: CRYPT_INTEGER_BLOB, secret: bool) -> Vec<u8> {
    if output.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: a successful call set pbData to a LocalAlloc buffer of cbData bytes that only this function uses.
    let bytes = unsafe { slice::from_raw_parts_mut(output.pbData, output.cbData as usize) };
    let copy = bytes.to_vec();
    if secret {
        for byte in bytes.iter_mut() {
            // SAFETY: byte is a valid, aligned element of the buffer above.
            unsafe { ptr::write_volatile(byte, 0) };
        }
    }
    // SAFETY: the buffer came from LocalAlloc in crypt32 and nothing uses it after this.
    unsafe { LocalFree(output.pbData.cast()) };
    copy
}
