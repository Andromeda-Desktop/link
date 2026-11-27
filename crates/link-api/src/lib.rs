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
