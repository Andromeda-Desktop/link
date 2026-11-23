//! QUIC over a phone's USB cable: a loopback TCP listener, and `adb reverse` from each ready phone's
//! [`USB_PORT`] to it. Without `adb` nothing runs; any local process may connect, and the QUIC handshake still pins
//! both keys.

use std::net::Ipv4Addr;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use link_core::path::{LinkKind, PathSocket, StreamLink, USB_PORT};

use crate::hub::HubHandle;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};

/// The first wait before `adb track-devices` runs again after it ended; it doubles up to [`MAX_RESTART`].
const FIRST_RESTART: Duration = Duration::from_secs(1);
const MAX_RESTART: Duration = Duration::from_secs(60);
/// E2E only: a test linkd must not take the USB path from the user's own.
const DISABLE: &str = "ANDROMEDA_LINK_NO_USB";

/// `adb` that talks only to a server that already runs: adb starts none for a named host. A server linkd started in
/// another network namespace (a sandbox, a test) would take the phone's USB interface from the user's own.
fn adb() -> tokio::process::Command {
    let mut adb = tokio::process::Command::new("adb");
    adb.args(["-H", "127.0.0.1"]);
    adb
}

pub async fn start(socket: Arc<PathSocket>, hub: HubHandle) {
    if std::env::var_os(DISABLE).is_some() {
        return;
    }
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await {
        Ok(listener) => listener,
        Err(error) => return log::info!("USB: no loopback listener: {error}"),
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => return log::info!("USB: {error}"),
    };
    tokio::spawn(accept(listener, socket));
    tokio::spawn(track(port, hub));
}

async fn accept(listener: TcpListener, socket: Arc<PathSocket>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => match link(stream) {
                Ok(link) => log::info!("USB link {}", socket.accept_link(link)),
                Err(error) => log::info!("USB: {error}"),
            },
            Err(error) => return log::info!("USB: the listener failed: {error}"),
        }
    }
}

fn link(stream: TcpStream) -> std::io::Result<StreamLink> {
    // QUIC packets go out whole and at once; Nagle would hold the small ones back.
    stream.set_nodelay(true)?;
    StreamLink::new(OwnedFd::from(stream.into_std()?), LinkKind::Usb)
}

/// Runs `adb track-devices` while linkd runs, points each phone that becomes ready at `port`, and tells the hub.
async fn track(port: u16, hub: HubHandle) {
    let mut wait = FIRST_RESTART;
    loop {
        let started = tokio::time::Instant::now();
        let ended = follow(port, &hub).await;
        hub.usb_cables(0).await;
        match ended {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return log::info!("USB: no adb"),
            Err(error) => log::info!("USB: adb track-devices: {error}"),
            Ok(()) => log::info!("USB: adb track-devices ended"),
        }
        if started.elapsed() > MAX_RESTART {
            wait = FIRST_RESTART;
        }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(MAX_RESTART);
    }
}

/// One `adb track-devices` run. Each report is 4 hex digits of length, then lines of "serial\tstate".
async fn follow(port: u16, hub: &HubHandle) -> std::io::Result<()> {
    let mut child = adb()
        .arg("track-devices")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut out = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
    let mut ready = std::collections::HashSet::new();
    let mut reversed = std::collections::HashSet::new();
    loop {
        let mut len = [0; 4];
        if out.read_exact(&mut len).await.is_err() {
            return Ok(());
        }
        let len = std::str::from_utf8(&len)
            .ok()
            .and_then(|hex| usize::from_str_radix(hex, 16).ok())
            .ok_or_else(|| std::io::Error::other("a malformed report"))?;
        let mut report = vec![0; len];
        out.read_exact(&mut report).await?;
        let now: std::collections::HashSet<String> = String::from_utf8_lossy(&report)
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .filter(|(_, state)| *state == "device")
            .map(|(serial, _)| serial.to_owned())
            .collect();
        let before = reversed.len();
        reversed.retain(|serial| now.contains(serial));
        for serial in now.difference(&ready) {
            if reverse(serial, port).await {
                reversed.insert(serial.clone());
            }
        }
        if reversed.len() != before {
            hub.usb_cables(reversed.len()).await;
        }
        ready = now;
    }
}

async fn reverse(serial: &str, port: u16) -> bool {
    let status = adb()
        .args(["-s", serial, "reverse", &format!("tcp:{USB_PORT}"), &format!("tcp:{port}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .status();
    let failure = match tokio::time::timeout(Duration::from_secs(10), status).await {
        Ok(Ok(status)) if status.success() => {
            log::info!("USB: {serial} reaches this desktop over its cable");
            return true;
        }
        Ok(Ok(status)) => status.to_string(),
        Ok(Err(error)) => error.to_string(),
        Err(_) => "hung".to_owned(),
    };
    log::info!("USB: adb reverse on {serial}: {failure}");
    false
}
