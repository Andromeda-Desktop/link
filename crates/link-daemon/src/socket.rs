//! `org.andromeda.Link1` peer to peer, where there is no session bus: a Unix socket on macOS, a named pipe on
//! Windows. Each client gets its own connection with its own copy of the objects, and every signal.

#[cfg(unix)]
use std::fs::DirBuilder;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt as _;
#[cfg(unix)]
use std::path::Path;

use anyhow::Context;
#[cfg(unix)]
use tokio::net::UnixListener;
#[cfg(windows)]
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::watch;
use tokio::task::JoinSet;
use zbus::connection::Builder;

use crate::dbus::{self, PATH};
use crate::hub::{self, HubHandle, Snapshot};
use crate::quickshare;

/// Signals queued for one client; a client this far behind loses the newest.
const CLIENT_QUEUE: usize = 256;
/// The wait before opening a pipe instance again after Windows refused one.
#[cfg(windows)]
const INSTANCE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// What each client's connection serves.
#[derive(Clone)]
pub struct Api {
    pub hub: HubHandle,
    pub snapshots: watch::Receiver<Snapshot>,
    pub nearby: watch::Receiver<Vec<(String, String)>>,
    pub quick_share: quickshare::Handle,
    pub quick_share_watches: quickshare::Watches,
    pub name: String,
}

/// Where the API listens: a socket path, or on Windows a pipe name.
#[cfg(unix)]
pub type Address = std::path::PathBuf;
#[cfg(windows)]
pub type Address = String;

/// Serves `api` at `address` until an error; signals come from `events` and `quick_share_events`.
pub async fn serve(
    address: &Address,
    api: Api,
    mut events: mpsc::Receiver<hub::Event>,
    mut quick_share_events: mpsc::Receiver<quickshare::Event>,
) -> anyhow::Result<()> {
    let mut listener = Listener::bind(address)?;
    let (mut sinks, mut quick_share_sinks) = (Vec::new(), Vec::new());
    let mut clients = JoinSet::new();
    let mut next = 0_u64;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Some(stream) = accepted else { continue };
                next += 1;
                let (sink, client_events) = mpsc::channel(CLIENT_QUEUE);
                let (quick_share_sink, client_quick_share) = mpsc::channel(CLIENT_QUEUE);
                sinks.push(sink);
                quick_share_sinks.push(quick_share_sink);
                clients.spawn(client(stream, format!("socket-{next}"), api.clone(), client_events, client_quick_share));
            }
            Some(event) = events.recv() => broadcast(&mut sinks, &event),
            Some(event) = quick_share_events.recv() => broadcast(&mut quick_share_sinks, &event),
            Some(done) = clients.join_next() => match done {
                Ok(Ok(())) => {}
                Ok(Err(error)) => log::info!("API client: {error:#}"),
                Err(error) => log::warn!("API client task: {error}"),
            },
        }
    }
}

#[cfg(unix)]
struct Listener {
    listener: UnixListener,
    uid: u32,
}

#[cfg(unix)]
impl Listener {
    fn bind(path: &Path) -> anyhow::Result<Self> {
        let listener = bind(path)?;
        log::info!("API on {}", path.display());
        Ok(Self { listener, uid: rustix::process::getuid().as_raw() })
    }

    /// The next client of this user; None for a refused one.
    async fn accept(&mut self) -> Option<Builder<'static>> {
        let (stream, _) =
            self.listener.accept().await.inspect_err(|error| log::warn!("accepting an API client: {error}")).ok()?;
        match stream.peer_cred() {
            Ok(cred) if cred.uid() == self.uid => {}
            Ok(cred) => {
                log::warn!("refused an API client of user {}", cred.uid());
                return None;
            }
            Err(error) => {
                log::warn!("refused an API client without credentials: {error}");
                return None;
            }
        }
        let stream = stream.into_std().inspect_err(|error| log::warn!("accepting an API client: {error}")).ok()?;
        Some(Builder::unix_stream(stream))
    }
}

/// The pipe's DACL admits only this account; each accept opens the instance the next client connects to.
#[cfg(windows)]
struct Listener {
    name: String,
    next: NamedPipeServer,
}

#[cfg(windows)]
impl Listener {
    fn bind(name: &str) -> anyhow::Result<Self> {
        let next = link_api::pipe::listen(name, true)
            .with_context(|| format!("creating {name}; another daemon or account holds it"))?;
        log::info!("API on {name}");
        Ok(Self { name: name.to_owned(), next })
    }

    /// The next client of this user in this session; None for a refused one. Cancel-safe: a client connected
    /// before a cancel stays in `next`, whose connect then returns at once.
    async fn accept(&mut self) -> Option<Builder<'static>> {
        let connected = self.next.connect().await;
        let pipe = loop {
            match link_api::pipe::listen(&self.name, false) {
                Ok(fresh) => break std::mem::replace(&mut self.next, fresh),
                Err(error) => {
                    log::warn!("opening the next instance of {}: {error}", self.name);
                    tokio::time::sleep(INSTANCE_RETRY).await;
                }
            }
        };
        if let Err(error) = connected {
            log::warn!("accepting an API client: {error}");
            return None;
        }
        match link_api::pipe::same_session(&pipe) {
            Ok(true) => {}
            Ok(false) => {
                log::warn!("refused an API client of another session");
                return None;
            }
            Err(error) => {
                log::warn!("checking an API client's session: {error}");
                return None;
            }
        }
        Some(Builder::socket(link_api::pipe::Pipe(pipe)).auth_mechanism(zbus::connection::AuthMechanism::Anonymous))
    }
}

/// Creates the socket's directory owner-only and replaces a socket no daemon answers on.
#[cfg(unix)]
fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    if path.exists() {
        anyhow::ensure!(
            std::os::unix::net::UnixStream::connect(path).is_err(),
            "another andromeda-linkd serves {}",
            path.display()
        );
        std::fs::remove_file(path).with_context(|| format!("removing the stale {}", path.display()))?;
    }
    UnixListener::bind(path).with_context(|| format!("binding {}", path.display()))
}

fn broadcast<E: Clone>(sinks: &mut Vec<mpsc::Sender<E>>, event: &E) {
    sinks.retain(|sink| match sink.try_send(event.clone()) {
        Ok(()) | Err(TrySendError::Full(_)) => true,
        Err(TrySendError::Closed(_)) => false,
    });
}

/// One client's connection, until it closes; then the capabilities it reported and its discovery are gone.
async fn client(
    builder: Builder<'static>,
    peer: String,
    api: Api,
    events: mpsc::Receiver<hub::Event>,
    quick_share_events: mpsc::Receiver<quickshare::Event>,
) -> anyhow::Result<()> {
    let link = dbus::Link::new(api.hub.clone(), api.snapshots.clone(), api.nearby.clone(), Some(peer.clone()));
    let quick_share = quickshare::Interface::new(
        api.quick_share.clone(),
        api.quick_share_watches.clone(),
        api.name,
        Some(peer.clone()),
    );
    let connection = builder
        .server(zbus::Guid::generate())
        .p2p()
        .serve_at(PATH, link)
        .serve_at(PATH, quick_share)
        .build()
        .await
        .context("the API handshake")?;
    let result = tokio::select! {
        result = dbus::forward(&connection, api.snapshots, events, api.nearby) => result,
        result = quickshare::forward(&connection, api.quick_share_watches, quick_share_events) => result,
        () = connection.closed() => Ok(()),
    };
    api.quick_share.discover(peer.clone(), false).await;
    api.hub.capabilities_gone(peer).await;
    result
}
