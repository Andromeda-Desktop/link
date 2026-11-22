//! The relay server: forwards each device's datagrams to the device whose tag they name, and tells each device the
//! address it sees. Only allowed devices connect.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link_core::identity::{DeviceId, Identity};
use link_core::relay::{CONTROL, MTU, Tag, frame, tag};
use link_core::tls::{certified_key, peer_spki, server_verifier};

/// Closes a device that is not on the allowlist.
const NOT_ALLOWED: u32 = 1;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub fn endpoint(identity: &Identity, listen: SocketAddr) -> anyhow::Result<quinn::Endpoint> {
    let key = certified_key(identity).map_err(|error| anyhow::anyhow!("relay key: {error}"))?;
    let mut crypto = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(server_verifier())
        .with_cert_resolver(Arc::new(rustls::server::AlwaysResolvesServerRawPublicKeys::new(key)));
    crypto.alpn_protocols = vec![link_core::proto::ALPN.to_vec()];
    let mut config =
        quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(crypto)?));
    let mut transport = quinn::TransportConfig::default();
    transport.initial_mtu(MTU);
    transport.max_idle_timeout(Some(IDLE_TIMEOUT.try_into()?));
    transport.max_concurrent_bidi_streams(0u8.into());
    transport.max_concurrent_uni_streams(0u8.into());
    config.transport_config(Arc::new(transport));
    Ok(quinn::Endpoint::server(config, listen)?)
}

type Peers = Arc<Mutex<HashMap<Tag, quinn::Connection>>>;

/// Serves until the endpoint closes; `allowed` is asked at connect and every `recheck`, so a removed device leaves.
pub async fn serve(
    endpoint: quinn::Endpoint,
    recheck: Duration,
    allowed: impl Fn(&DeviceId) -> bool + Send + Sync + 'static,
) {
    let (peers, allowed) = (Peers::default(), Arc::new(allowed));
    while let Some(incoming) = endpoint.accept().await {
        let (peers, allowed) = (peers.clone(), allowed.clone());
        tokio::spawn(async move {
            if let Err(error) = relay(incoming, &peers, recheck, &*allowed).await {
                log::info!("relay: {error}");
            }
        });
    }
}

async fn relay(
    incoming: quinn::Incoming,
    peers: &Peers,
    recheck: Duration,
    allowed: &(dyn Fn(&DeviceId) -> bool + Send + Sync),
) -> anyhow::Result<()> {
    let connection = incoming.await?;
    let spki = peer_spki(connection.peer_identity()).map_err(|error| anyhow::anyhow!("peer key: {error}"))?;
    let (device, source) = (spki.device_id(), tag(&spki));
    if !allowed(&device) {
        connection.close(NOT_ALLOWED.into(), b"not allowed");
        anyhow::bail!("{device} is not allowed");
    }
    let seen = connection.remote_address();
    log::info!("relay: {device} from {seen}");
    connection.send_datagram(frame(&CONTROL, seen.to_string().as_bytes()))?;
    lock(peers).insert(source, connection.clone());
    let mut check = tokio::time::interval_at(tokio::time::Instant::now() + recheck, recheck);
    let ended = loop {
        let datagram = tokio::select! {
            datagram = connection.read_datagram() => match datagram {
                Ok(datagram) => datagram,
                Err(error) => break error.to_string(),
            },
            _ = check.tick() => {
                if allowed(&device) {
                    continue;
                }
                connection.close(NOT_ALLOWED.into(), b"not allowed");
                break "it is no longer allowed".to_owned();
            }
        };
        let Some((destination, body)) = datagram.split_first_chunk::<8>() else { continue };
        let target = lock(peers).get(destination).cloned();
        // A peer that is away or slow loses the packet; QUIC above resends it.
        if let Some(target) = target {
            drop(target.send_datagram(frame(&source, body)));
        }
    };
    let mut peers = lock(peers);
    if peers.get(&source).is_some_and(|current| current.stable_id() == connection.stable_id()) {
        peers.remove(&source);
    }
    anyhow::bail!("{device} left: {ended}")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
