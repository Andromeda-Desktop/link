//! The desktop's connection to its relay, held from the listening socket, so phones on other networks reach it: each
//! phone that opens a link through the relay becomes a message link of that socket. The spec is the file
//! `<state>/relay`: `host:port#<64 hex key fingerprint>`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use link_core::relay::{self, Relay};
use link_core::tls::ServerPin;
use link_core::transport::PathServer;

const RETRY: Duration = Duration::from_secs(10);

/// The relay spec in `path`, or None when the file is missing or malformed.
pub fn spec(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let spec = text.trim();
    if relay::parse(spec).is_none() {
        log::warn!("{}: not host:port#<64 hex fingerprint>; no relay", path.display());
        return None;
    }
    Some(spec.to_owned())
}

/// Holds the relay connection for as long as the daemon runs, dialling again [`RETRY`] after it ends.
pub fn start(server: Arc<PathServer>, spec: String) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = hold(&server, &spec).await {
                log::info!("relay {spec}: {error:#}");
            }
            tokio::time::sleep(RETRY).await;
        }
    });
}

async fn hold(server: &Arc<PathServer>, spec: &str) -> anyhow::Result<()> {
    let (host, key) = relay::parse(spec).context("malformed spec")?;
    let addr = tokio::net::lookup_host(&host).await?.next().context("no address")?;
    let connection = server
        .dial(addr, ServerPin::Key(key), |transport| {
            transport.initial_mtu(relay::MTU).keep_alive_interval(Some(relay::KEEP_ALIVE));
        })
        .await?;
    // Dropping the relay at the end closes every link it opened.
    let (_relay, mut opened) = Relay::new(connection.clone());
    log::info!("relay {spec}: connected at {addr}");
    while let Some(link) = opened.recv().await {
        log::info!("relay {spec}: link {} from a peer", server.socket.accept_link(link));
    }
    anyhow::bail!("ended: {:?}", connection.close_reason())
}
