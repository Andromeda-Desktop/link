//! Windows has no user service manager: linkd starts `andromeda-link-session.exe` from its own folder and starts it
//! again when it ends, with backoff. linkd first joins a kill-on-close job, which its children join at creation, so
//! link-session ends with linkd, a crash included.

#![allow(unsafe_code, reason = "FFI: kernel32 job object calls on handles this module owns")]

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

const FIRST_DELAY: Duration = Duration::from_secs(1);
const MAX_DELAY: Duration = Duration::from_secs(60);
/// A run this long counts as healthy: the next restart waits `FIRST_DELAY` again.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// Joins the job, then keeps link-session running; never returns. Without the job it logs why and still supervises.
pub async fn run() {
    if let Err(error) = join_kill_on_close_job() {
        log::warn!("link-session may outlive andromeda-linkd: {error:#}");
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe.with_file_name("andromeda-link-session.exe"),
        Err(error) => {
            log::error!("cannot find link-session beside andromeda-linkd: {error}");
            return std::future::pending().await;
        }
    };
    supervise(exe).await;
}

async fn supervise(exe: PathBuf) -> ! {
    let mut delay = FIRST_DELAY;
    loop {
        let started = Instant::now();
        match tokio::process::Command::new(&exe).kill_on_drop(true).spawn() {
            Ok(mut child) => match child.wait().await {
                Ok(status) => log::warn!("link-session ended ({status})"),
                Err(error) => log::warn!("waiting for link-session: {error}"),
            },
            Err(error) => log::warn!("starting {}: {error}", exe.display()),
        }
        if started.elapsed() >= HEALTHY_RUN {
            delay = FIRST_DELAY;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(MAX_DELAY);
    }
}

/// The job's handle stays open for the life of the process: closing it is what ends the children.
fn join_kill_on_close_job() -> anyhow::Result<()> {
    // SAFETY: both arguments are optional; a null name makes an unnamed job.
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() {
        bail!("CreateJobObjectW: {}", io::Error::last_os_error());
    }
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).context("limit size")?;
    // SAFETY: limits is a JOBOBJECT_EXTENDED_LIMIT_INFORMATION of `size` bytes that outlives the call.
    if unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size) } == 0
    {
        bail!("SetInformationJobObject: {}", io::Error::last_os_error());
    }
    // SAFETY: plain calls; OpenProcess returns null on failure, checked below.
    let me = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, GetCurrentProcessId()) };
    if me.is_null() {
        bail!("OpenProcess: {}", io::Error::last_os_error());
    }
    // SAFETY: job and me are valid handles; me is closed once and not used after.
    let joined = unsafe { AssignProcessToJobObject(job, me) };
    let error = io::Error::last_os_error();
    // SAFETY: as above.
    unsafe { CloseHandle(me) };
    if joined == 0 {
        bail!("AssignProcessToJobObject: {error}");
    }
    Ok(())
}
