//! "Ring desktop" from a phone: a sound on repeat until the phone stops it or [`RING_FOR`] passes. The Andromeda
//! shell rings on its own desktop, so this runs only elsewhere.

use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::link::Link;

/// Players to try, in order, with the sound each plays.
#[cfg(target_os = "macos")]
const PLAYERS: [(&str, &str); 1] = [("/usr/bin/afplay", "/System/Library/Sounds/Sosumi.aiff")];
#[cfg(target_os = "linux")]
const PLAYERS: [(&str, &str); 2] = [
    ("pw-play", "/usr/share/sounds/freedesktop/stereo/phone-incoming-call.oga"),
    ("paplay", "/usr/share/sounds/freedesktop/stereo/phone-incoming-call.oga"),
];
const RING_FOR: Duration = Duration::from_secs(30);

/// False on the Andromeda desktop, whose shell rings.
#[cfg(target_os = "linux")]
pub fn wanted() -> bool {
    !std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .any(|desktop| desktop.eq_ignore_ascii_case("andromeda"))
}

#[derive(Default)]
pub struct Ring {
    /// The phone that asked, the sound loop, and when it stops on its own.
    now: Option<(String, JoinHandle<()>, Instant)>,
}

impl Ring {
    pub async fn set(&mut self, link: &Link, device: &str, on: bool) {
        self.stop(link).await;
        if !on {
            return;
        }
        let sound = tokio::spawn(play());
        self.now = Some((device.to_owned(), sound, Instant::now() + RING_FOR));
        report(link, device, true).await;
    }

    pub async fn stop(&mut self, link: &Link) {
        if let Some((device, sound, _)) = self.now.take() {
            sound.abort();
            // Dropping the task stops its sound; on Windows that stop is process-wide, so it ends before a new ring.
            if let Err(error) = sound.await
                && error.is_panic()
            {
                log::warn!("ringing: {error}");
            }
            report(link, &device, false).await;
        }
    }

    /// Waits until the ring times out; pending while nothing rings.
    pub async fn timed_out(&self) {
        match &self.now {
            Some((_, _, until)) => tokio::time::sleep_until(*until).await,
            None => std::future::pending().await,
        }
    }
}

/// The sound on repeat until the task is aborted.
#[cfg(unix)]
async fn play() {
    loop {
        // Dropping the child, when the task is aborted, ends the sound at once.
        let child = PLAYERS.iter().find_map(|(player, sound)| {
            tokio::process::Command::new(player).arg(sound).kill_on_drop(true).spawn().ok()
        });
        match child {
            // A player that fails would fail again at once.
            Some(mut child) => match child.wait().await {
                Ok(status) if status.success() => {}
                result => return log::warn!("ringing: the player ended with {result:?}"),
            },
            None => return log::warn!("ringing: none of {PLAYERS:?} runs"),
        }
    }
}

/// The incoming-call sound on repeat until the task is aborted, which stops it.
#[cfg(windows)]
#[allow(unsafe_code, reason = "FFI: winmm PlaySoundW")]
async fn play() {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_LOOP, SND_NODEFAULT};
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            // SAFETY: a null sound stops the one this process plays.
            unsafe { PlaySoundW(std::ptr::null(), std::ptr::null_mut(), 0) };
        }
    }
    // Registry sound events (PlaySound docs); the first is the toast catalog's looping call sound.
    let played = [windows_sys::core::w!("Notification.Looping.Call"), windows_sys::core::w!("SystemAsterisk")]
        .into_iter()
        // SAFETY: static wide strings; SND_ASYNC returns at once, and the sound loops until a null PlaySoundW.
        .any(|alias| unsafe { PlaySoundW(alias, std::ptr::null_mut(), SND_ALIAS | SND_ASYNC | SND_LOOP | SND_NODEFAULT) } != 0);
    if !played {
        return log::warn!("ringing: Windows has no sound for an incoming call");
    }
    let _stop = Stop;
    std::future::pending::<()>().await;
}

async fn report(link: &Link, device: &str, on: bool) {
    if let Err(error) = link.proxy.desktop_ringing(device, on).await {
        log::warn!("reporting the ring to {device}: {error}");
    }
}
