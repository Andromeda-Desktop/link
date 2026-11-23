//! Link's relay for peers on different networks. Each device holds one QUIC connection to the relay from its
//! `PathSocket`'s UDP socket; a datagram there is one inner QUIC packet of a Link connection, addressed by the peer's
//! [`Tag`]. The relay replies with the address it sees, which is the NAT mapping a hole punch needs.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::{BufMut, Bytes, BytesMut};
use tokio::sync::{mpsc, watch};

use crate::identity::Spki;
use crate::path::{LinkKind, MessageLink};

/// A device on the relay: the first bytes of its key fingerprint.
pub type Tag = [u8; 8];
/// The source tag of the relay's own datagrams, which carry the observed address as text.
pub const CONTROL: Tag = [0; 8];
/// The relay connection's packet size: a whole inner packet (QUIC's 1200-byte minimum), its tag and the outer
/// headers must fit one datagram.
pub const MTU: u16 = 1300;
/// Keep-alive on a relay connection, so the NAT mapping to the relay outlives quiet periods.
pub const KEEP_ALIVE: std::time::Duration = std::time::Duration::from_secs(15);
/// Inner packets waiting per link before the oldest arrivals drop, as a full socket buffer drops them.
const QUEUE: usize = 1024;

#[must_use]
pub fn tag(spki: &Spki) -> Tag {
    let mut tag = CONTROL;
    tag.copy_from_slice(&spki.fingerprint()[..8]);
    tag
}

/// `tag` then `body`: the destination on the way in, the source on the way out.
#[must_use]
pub fn frame(tag: &Tag, body: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(tag.len() + body.len());
    out.put_slice(tag);
    out.put_slice(body);
    out.freeze()
}

/// The relay address and its key fingerprint in a `host:port#<64 hex>` spec, or None when malformed.
#[must_use]
pub fn parse(spec: &str) -> Option<(String, [u8; 32])> {
    let (host, hex) = spec.split_once('#')?;
    if hex.len() != 64 || !host.contains(':') {
        return None;
    }
    let mut key = [0; 32];
    for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some((host.to_owned(), key))
}

type Links = Arc<Mutex<HashMap<Tag, mpsc::Sender<Bytes>>>>;

/// A device's connection to the relay, read by one task until it ends.
pub struct Relay {
    connection: quinn::Connection,
    links: Links,
    observed: watch::Receiver<Option<SocketAddr>>,
}

impl Relay {
    /// Starts reading `connection`; links that peers open arrive on the receiver.
    pub fn new(connection: quinn::Connection) -> (Self, mpsc::Receiver<RelayLink>) {
        let links = Links::default();
        let (observed_tx, observed) = watch::channel(None);
        let (accepted_tx, accepted) = mpsc::channel(8);
        tokio::spawn(read(connection.clone(), links.clone(), observed_tx, accepted_tx));
        (Self { connection, links, observed }, accepted)
    }

    /// A link to `peer`; its packets from then on reach this link.
    pub fn link(&self, peer: Tag) -> RelayLink {
        let (tx, rx) = mpsc::channel(QUEUE);
        lock(&self.links).insert(peer, tx);
        RelayLink { connection: self.connection.clone(), peer, rx: tokio::sync::Mutex::new(rx) }
    }

    /// This device's address as the relay sees it, once the relay said.
    pub fn observed(&self) -> Option<SocketAddr> {
        *self.observed.borrow()
    }

    /// This device's address as the relay sees it, as it changes.
    #[must_use]
    pub fn observed_changes(&self) -> watch::Receiver<Option<SocketAddr>> {
        self.observed.clone()
    }

    /// Waits until the relay said this device's address.
    pub async fn wait_observed(&mut self) -> Option<SocketAddr> {
        self.observed.wait_for(Option::is_some).await.ok().and_then(|seen| *seen)
    }

    pub fn connection(&self) -> &quinn::Connection {
        &self.connection
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn read(
    connection: quinn::Connection,
    links: Links,
    observed: watch::Sender<Option<SocketAddr>>,
    accepted: mpsc::Sender<RelayLink>,
) {
    while let Ok(datagram) = connection.read_datagram().await {
        let Some((&source, _)) = datagram.split_first_chunk::<8>() else { continue };
        let body = datagram.slice(8..);
        if source == CONTROL {
            if let Ok(addr) = std::str::from_utf8(&body).unwrap_or_default().parse() {
                observed.send_replace(Some(addr));
            }
            continue;
        }
        let sent = lock(&links).get(&source).map(|tx| tx.try_send(body.clone()));
        match sent {
            Some(Ok(()) | Err(mpsc::error::TrySendError::Full(_))) => {}
            // A new peer, or one whose link ended: a fresh link takes this packet.
            None | Some(Err(mpsc::error::TrySendError::Closed(_))) => {
                let (tx, rx) = mpsc::channel(QUEUE);
                drop(tx.try_send(body));
                lock(&links).insert(source, tx);
                let link = RelayLink { connection: connection.clone(), peer: source, rx: tokio::sync::Mutex::new(rx) };
                if accepted.try_send(link).is_err() {
                    lock(&links).remove(&source);
                }
            }
        }
    }
    log::info!("relay connection ended: {:?}", connection.close_reason());
}

/// One peer through the relay: a datagram is one inner QUIC packet.
pub struct RelayLink {
    connection: quinn::Connection,
    peer: Tag,
    rx: tokio::sync::Mutex<mpsc::Receiver<Bytes>>,
}

impl RelayLink {
    #[must_use]
    pub fn peer(&self) -> Tag {
        self.peer
    }
}

impl MessageLink for RelayLink {
    fn kind(&self) -> LinkKind {
        LinkKind::Relay
    }

    fn send(&self, packet: &[u8]) -> impl Future<Output = io::Result<()>> + Send {
        std::future::ready(match self.connection.send_datagram(frame(&self.peer, packet)) {
            // An inner MTU probe larger than the relay path carries is lost, as on any path.
            Ok(()) | Err(quinn::SendDatagramError::TooLarge) => Ok(()),
            Err(error) => Err(io::Error::other(error)),
        })
    }

    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let Some(packet) = self.rx.lock().await.recv().await else { return Ok(0) };
        let len = packet.len().min(buf.len());
        buf[..len].copy_from_slice(&packet[..len]);
        Ok(len)
    }
}
