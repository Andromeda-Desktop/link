//! The typed client of `org.andromeda.Link1`, generated from `api/org.andromeda.Link1.xml`: [`Link1Proxy`],
//! [`QuickShareProxy`] and their signal streams. Clients use only this, never names copied from the XML.

#[cfg(unix)]
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub mod pipe;

pub const SERVICE: &str = "org.andromeda.Link1";
pub const PATH: &str = "/org/andromeda/Link1";
/// Names a Unix socket (on Windows, a named pipe) that carries the API in place of the session bus.
pub const SOCKET_ENV: &str = "ANDROMEDA_LINK_SOCKET";
/// The frame rate `OpenCamera` asks the phone for; a camera reader needs a fixed rate to agree on a format.
pub const CAMERA_FPS: u32 = 30;

mod generated {
    #![allow(clippy::pedantic, reason = "zbus_xmlgen output")]
    use zbus::proxy;

    include!(concat!(env!("OUT_DIR"), "/link1.rs"));
}
pub use generated::*;

/// A new empty file only this process can reach, for bytes a Link1 member passes as a descriptor or keeps: a memfd
/// on Linux, else a file in the user's own temporary directory that no path names once this returns (macOS) or that
/// goes away with its last handle (Windows). Those directories belong to the user, so another account cannot plant
/// a guessed name.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn anonymous_file() -> std::io::Result<std::fs::File> {
    Ok(rustix::fs::memfd_create("link-clip", rustix::fs::MemfdFlags::CLOEXEC)?.into())
}

/// See the Linux form.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn anonymous_file() -> std::io::Result<std::fs::File> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!("link-clip-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let path = std::env::temp_dir().join(name);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE};
        options.custom_flags(FILE_FLAG_DELETE_ON_CLOSE).attributes(FILE_ATTRIBUTE_TEMPORARY);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    std::fs::remove_file(&path)?;
    Ok(file)
}

/// The socket `andromeda-linkd` serves the API on: `$ANDROMEDA_LINK_SOCKET`, else on macOS, which has no session bus,
/// `~/Library/Application Support/Andromeda Link/link.sock`. None means the session bus.
#[cfg(unix)]
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SOCKET_ENV) {
        return Some(path.into());
    }
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Andromeda Link/link.sock"));
    #[cfg(not(target_os = "macos"))]
    None
}

/// A connection to `andromeda-linkd`: peer to peer over [`socket_path`] when it names one, else the session bus.
/// Over the socket there is no bus driver: no name owners, no activation; the connection closes when the daemon stops.
#[cfg(unix)]
pub async fn connect() -> zbus::Result<zbus::Connection> {
    match socket_path() {
        Some(path) => connect_socket(&path).await,
        None => zbus::Connection::session().await,
    }
}

/// A connection to `andromeda-linkd` over its named pipe, [`pipe_name`].
#[cfg(windows)]
pub async fn connect() -> zbus::Result<zbus::Connection> {
    connect_pipe(&pipe_name()?).await
}

/// A peer-to-peer connection to the daemon's pipe `name`.
#[cfg(windows)]
pub async fn connect_pipe(name: &str) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::socket(pipe::Pipe(pipe::open(name).await?)).p2p().build().await
}

/// The pipe `andromeda-linkd` serves the API on: `$ANDROMEDA_LINK_SOCKET`, else [`pipe::default_name`].
#[cfg(windows)]
pub fn pipe_name() -> std::io::Result<String> {
    std::env::var(SOCKET_ENV).or_else(|_| pipe::default_name())
}

/// A peer-to-peer connection to the daemon's socket at `path`.
#[cfg(unix)]
pub async fn connect_socket(path: &Path) -> zbus::Result<zbus::Connection> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    zbus::connection::Builder::unix_stream(stream).p2p().build().await
}
