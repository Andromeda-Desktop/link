use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use link_proto::ALPN;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::{AlwaysResolvesClientRawPublicKeys, ClientSessionMemoryCache, Resumption};
use rustls::crypto::ring as provider;
use rustls::server::AlwaysResolvesServerRawPublicKeys;

use crate::Error;
use crate::identity::Identity;
use crate::path::{Hop, PathSocket, Route};
use crate::tls::{self, ServerPin};

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(90);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The desktop's PING interval; `ARCHITECTURE.md` (Session) says why a third of [`IDLE_TIMEOUT`].
pub const KEEP_ALIVE: Duration = Duration::from_secs(30);
/// A present phone's PING interval: a third of the 30 s idle timeout of older desktops.
pub const PHONE_KEEP_ALIVE: Duration = Duration::from_secs(10);
/// Bulk streams a peer may open at once: a transfer streams four files, the rest is room for other transfers.
const MAX_BULK_STREAMS: u8 = 8;
pub(crate) const STREAM_WINDOW: u32 = 8 << 20;
const CONNECTION_WINDOW: u32 = 32 << 20;

/// A connection the phone dialled, handshake complete. `resumed` is set when the desktop accepted the TLS session
/// ticket (0-RTT), skipping the full handshake. No application data rides in 0-RTT, so nothing can be replayed.
pub struct Dialed {
    pub connection: quinn::Connection,
    pub resumed: bool,
    /// Where the connection's packets go, on a dialer with a [`PathSocket`].
    pub route: Option<Route>,
}

type Tune = Arc<dyn Fn(&mut quinn::TransportConfig) + Send + Sync>;

/// The phone side: one UDP socket and one TLS session cache for every desktop it talks to.
#[derive(Clone)]
pub struct Dialer {
    endpoint: quinn::Endpoint,
    config: quinn::ClientConfig,
    path: Option<Arc<PathSocket>>,
    keep_alive: Option<Duration>,
    tune: Option<Tune>,
}

fn transport_config(keep_alive: Option<Duration>) -> Arc<quinn::TransportConfig> {
    Arc::new(transport_config_value(keep_alive))
}

#[expect(clippy::expect_used, reason = "the constant idle timeout is far below QUIC's varint limit")]
fn transport_config_value(keep_alive: Option<Duration>) -> quinn::TransportConfig {
    let mut config = quinn::TransportConfig::default();
    config.max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("idle timeout fits a varint")));
    config.max_concurrent_uni_streams(MAX_BULK_STREAMS.into());
    config.keep_alive_interval(keep_alive);
    // quinn's defaults (1.25 MB per stream) cap one file at about 1.25 MB per RTT, under the Wi-Fi rate at 50 ms.
    config.stream_receive_window(STREAM_WINDOW.into());
    config.receive_window(CONNECTION_WINDOW.into());
    config.send_window(CONNECTION_WINDOW.into());
    // Loss-based control collapses on the random loss of Wi-Fi (about 130 KB/s at 5% and 50 ms); BBR paces by the
    // measured bandwidth instead.
    config.congestion_controller_factory(bbr());
    config
}

pub(crate) fn tls13_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(provider::default_provider())
}

pub fn server_endpoint(identity: &Identity, addr: SocketAddr) -> Result<quinn::Endpoint, Error> {
    let runtime = quinn::default_runtime().ok_or_else(|| std::io::Error::other("no async runtime"))?;
    let socket = std::net::UdpSocket::bind(addr)?;
    Ok(quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server_config(identity)?), socket, runtime)?)
}

fn bbr() -> Arc<dyn quinn::congestion::ControllerFactory + Send + Sync> {
    Arc::new(quinn::congestion::BbrConfig::default())
}

fn server_config(identity: &Identity) -> Result<quinn::ServerConfig, Error> {
    let resolver = AlwaysResolvesServerRawPublicKeys::new(tls::certified_key(identity)?);
    let mut crypto = rustls::ServerConfig::builder_with_provider(tls13_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(tls::server_verifier())
        .with_cert_resolver(Arc::new(resolver));
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    crypto.max_early_data_size = u32::MAX;
    let crypto = QuicServerConfig::try_from(crypto).map_err(|_| Error::BadKey)?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport_config(Some(KEEP_ALIVE)));
    Ok(config)
}

/// A server endpoint on a [`PathSocket`], so phones reach it over UDP and message links alike.
pub fn path_server_endpoint(identity: &Identity, socket: Arc<PathSocket>) -> Result<quinn::Endpoint, Error> {
    let runtime = quinn::default_runtime().ok_or_else(|| std::io::Error::other("no async runtime"))?;
    Ok(quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(server_config(identity)?),
        socket,
        runtime,
    )?)
}

/// The desktop's endpoint on a [`PathSocket`] bound to `addr`, so phones reach it over UDP and message links alike.
pub struct PathServer {
    pub endpoint: quinn::Endpoint,
    pub puncher: Puncher,
    pub socket: Arc<PathSocket>,
    config: quinn::ServerConfig,
}

impl PathServer {
    pub fn bind(identity: &Identity, addr: SocketAddr) -> Result<Self, Error> {
        let config = server_config(identity)?;
        let udp = std::net::UdpSocket::bind(addr)?;
        let puncher = Puncher(Arc::new(udp.try_clone()?));
        let socket = PathSocket::new(udp)?;
        let endpoint = path_server_endpoint(identity, socket.clone())?;
        Ok(Self { endpoint, puncher, socket, config })
    }

    /// Accepts `incoming`, and returns its route, which the session updates (`Route::follow`) when the peer moves.
    pub fn accept(&self, incoming: quinn::Incoming) -> Result<(quinn::Connecting, Route), Error> {
        let remote = incoming.remote_address();
        let route = self.socket.route_to(remote);
        route.follow(remote);
        let mut transport = transport_config_value(Some(KEEP_ALIVE));
        transport.congestion_controller_factory(route.congestion(bbr()));
        let mut config = self.config.clone();
        config.transport_config(Arc::new(transport));
        Ok((incoming.accept_with(Arc::new(config))?, route))
    }
}

/// Sends from the server's own socket, so a stateful firewall in front of it takes a peer's dial from an address it
/// was sent to for a reply.
#[derive(Clone)]
pub struct Puncher(Arc<std::net::UdpSocket>);

impl Puncher {
    /// One byte to each address: not QUIC, so the peer's endpoint drops it.
    pub fn punch(&self, addresses: &[SocketAddr]) {
        let dual_stack = self.0.local_addr().is_ok_and(|local| local.is_ipv6());
        for &addr in addresses {
            let to = match addr {
                SocketAddr::V4(v4) if dual_stack => SocketAddr::new(v4.ip().to_ipv6_mapped().into(), v4.port()),
                addr => addr,
            };
            if let Err(error) = self.0.send_to(&[0], to) {
                log::info!("punching toward {addr}: {error}");
            }
        }
    }
}

impl Dialer {
    pub fn new(identity: &Identity) -> Result<Self, Error> {
        let dual_stack = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0));
        let endpoint =
            quinn::Endpoint::client(dual_stack).or_else(|_| quinn::Endpoint::client(([0, 0, 0, 0], 0).into()))?;
        Ok(Self { endpoint, config: client_config(identity)?, path: None, keep_alive: None, tune: None })
    }

    /// A dialer on a [`PathSocket`], so the phone moves one connection between UDP and message links.
    pub fn on_path(identity: &Identity, socket: Arc<PathSocket>) -> Result<Self, Error> {
        let runtime = quinn::default_runtime().ok_or_else(|| std::io::Error::other("no async runtime"))?;
        let endpoint =
            quinn::Endpoint::new_with_abstract_socket(quinn::EndpointConfig::default(), None, socket.clone(), runtime)?;
        Ok(Self { endpoint, config: client_config(identity)?, path: Some(socket), keep_alive: None, tune: None })
    }

    /// The same socket and TLS session cache with Link's transport settings changed by `tune`, last.
    #[must_use]
    pub fn tuned(&self, tune: impl Fn(&mut quinn::TransportConfig) + Send + Sync + 'static) -> Self {
        Self { tune: Some(Arc::new(tune)), ..self.clone() }
    }

    /// The same socket and TLS session cache, dialling with keep-alive so the connection outlives idle periods.
    #[must_use]
    pub fn present(&self) -> Self {
        Self { keep_alive: Some(PHONE_KEEP_ALIVE), ..self.clone() }
    }

    /// Waits until every connection has sent its close; call before the process exits.
    pub async fn finish(&self) {
        self.endpoint.wait_idle().await;
    }

    /// The UDP port every dial leaves from.
    pub fn port(&self) -> Option<u16> {
        self.endpoint.local_addr().ok().map(|addr| addr.port())
    }

    /// On a [`PathSocket`], dials `addr` at a fresh virtual address routed to it, so the connection can move.
    pub async fn dial(&self, addr: SocketAddr, pin: ServerPin) -> Result<Dialed, Error> {
        self.dial_hop(Hop::Udp(addr), pin).await
    }

    /// Dials over `hop`; a link hop needs a [`PathSocket`].
    pub async fn dial_hop(&self, hop: Hop, pin: ServerPin) -> Result<Dialed, Error> {
        match (&self.path, hop) {
            (Some(socket), _) => self.dial_route(socket.route_new(hop), pin).await,
            (None, Hop::Udp(addr)) => self.connect(addr, None, pin).await,
            (None, Hop::Link(_)) => Err(std::io::Error::other("a link dial needs a path socket").into()),
        }
    }

    /// Dials `route`'s peer, with a congestion controller that follows the route.
    pub async fn dial_route(&self, route: Route, pin: ServerPin) -> Result<Dialed, Error> {
        self.connect(route.peer(), Some(route), pin).await
    }

    async fn connect(&self, addr: SocketAddr, route: Option<Route>, pin: ServerPin) -> Result<Dialed, Error> {
        let mut transport = transport_config_value(self.keep_alive);
        if let Some(route) = &route {
            transport.congestion_controller_factory(route.congestion(bbr()));
        }
        if let Some(tune) = &self.tune {
            tune(&mut transport);
        }
        let mut config = self.config.clone();
        config.transport_config(Arc::new(transport));
        let connecting = self.endpoint.connect_with(config, addr, &pin.server_name())?;
        match connecting.into_0rtt() {
            Ok((connection, accepted)) => {
                let resumed = tokio::time::timeout(CONNECT_TIMEOUT, accepted).await?;
                if let Some(reason) = connection.close_reason() {
                    return Err(reason.into());
                }
                Ok(Dialed { connection, resumed, route })
            }
            Err(connecting) => {
                let connection = tokio::time::timeout(CONNECT_TIMEOUT, connecting).await??;
                Ok(Dialed { connection, resumed: false, route })
            }
        }
    }
}

/// Built once per dialer: rustls reuses a cached session only with the same verifier and key resolver objects.
fn client_config(identity: &Identity) -> Result<quinn::ClientConfig, Error> {
    let resolver = AlwaysResolvesClientRawPublicKeys::new(tls::certified_key(identity)?);
    let mut crypto = rustls::ClientConfig::builder_with_provider(tls13_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(tls::client_verifier())
        .with_client_cert_resolver(Arc::new(resolver));
    crypto.alpn_protocols = vec![ALPN.to_vec()];
    crypto.enable_early_data = true;
    crypto.resumption = Resumption::store(Arc::new(ClientSessionMemoryCache::new(32)));
    let crypto = QuicClientConfig::try_from(crypto).map_err(|_| Error::BadKey)?;
    Ok(quinn::ClientConfig::new(Arc::new(crypto)))
}

/// A phone connection to a desktop over loopback UDP: (phone side, desktop side). The phone's 1 MiB send window
/// makes a write past the desktop's flow-control window wait.
#[cfg(test)]
pub(crate) async fn connected(
    phone: &Identity,
    desktop: &Identity,
) -> Result<(quinn::Connection, quinn::Connection), Error> {
    let server = server_endpoint(desktop, SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let pin = ServerPin::Key(desktop.spki().fingerprint());
    let accept = async { Ok::<_, Error>(server.accept().await.ok_or(Error::NotConnected)?.await?) };
    let dialer = Dialer::new(phone)?.tuned(|t| {
        t.send_window(1 << 20);
    });
    let (phone_side, desktop_side) = tokio::join!(dialer.dial(server.local_addr()?, pin), accept);
    Ok((phone_side?.connection, desktop_side?))
}
