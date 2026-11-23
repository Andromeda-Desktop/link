//! One QUIC endpoint over UDP and message links (Bluetooth L2CAP, a `SOCK_SEQPACKET` descriptor; RFCOMM, a byte
//! stream that frames each packet). A link carries one QUIC packet per message. quinn sees each link as a peer address: the server sees a link's own address, so a phone
//! that moves between UDP and a link migrates the same connection; the phone routes the desktop's address over the
//! link it picks and reports the link's packets as coming from that address.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::os::fd::{AsFd, OwnedFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::task::{Context, Poll, Waker, ready};
use std::time::{Duration, Instant};

use quinn::congestion::{Controller, ControllerFactory};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, watch};

pub type LinkId = usize;

/// Packets queued per link before a send would block: about 90 KB, a few round trips of a Bluetooth link. Also the
/// most segments one send may carry, so a whole batch always fits an empty queue.
const QUEUE: usize = 64;
/// The congestion window while a connection runs over a link. The Android Bluetooth stack takes ~250 KB of writes at
/// once, and any controller fills it (rtt 2-2.7 s); 12 KiB kept a loaded ping p95 at 123-126 ms on the test phone
/// with bulk unchanged at ~1 Mbit/s.
pub const LINK_WINDOW: u64 = 12 << 10;
/// How long an accepted link may carry no connection: a phone dials at once, and a handshake takes at most
/// `CONNECT_TIMEOUT` (5 s).
pub const UNUSED_LINK: Duration = Duration::from_secs(10);
/// Inbound packets from all links waiting for the endpoint.
const INBOUND: usize = 256;
/// The largest packet a link may deliver; QUIC packets stay far below it.
const MAX_PACKET: usize = 1 << 16;

/// What carries a link. Only Bluetooth gets the [`LINK_WINDOW`] cap and counts as "on Bluetooth".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Bluetooth,
    /// Datagrams through a relay server, over the internet.
    Relay,
    /// A TCP stream over the phone's USB cable (`adb reverse`).
    Usb,
}

/// A connected link that keeps message boundaries: one `send` is one `recv` on the other end.
pub trait MessageLink: Send + Sync + 'static {
    fn kind(&self) -> LinkKind;
    fn send(&self, packet: &[u8]) -> impl Future<Output = io::Result<()>> + Send;
    /// One whole message; 0 bytes means the peer closed the link.
    fn recv(&self, buf: &mut [u8]) -> impl Future<Output = io::Result<usize>> + Send;
}

/// A `SOCK_SEQPACKET` descriptor: `BlueZ`'s L2CAP socket, or one end of the Android app's socket pair.
pub struct FdLink(AsyncFd<OwnedFd>);

impl FdLink {
    pub fn new(fd: OwnedFd) -> io::Result<Self> {
        rustix::io::ioctl_fionbio(&fd, true)?;
        // Sound for OwnedFd, whose descriptor never changes; `register` would need unsafe, which the workspace forbids.
        #[allow(deprecated)]
        Ok(Self(AsyncFd::new(fd)?))
    }
}

impl MessageLink for FdLink {
    fn kind(&self) -> LinkKind {
        LinkKind::Bluetooth
    }

    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        self.0
            .async_io(Interest::WRITABLE, |fd| rustix::io::write(fd.as_fd(), packet).map_err(io::Error::from))
            .await?;
        Ok(())
    }

    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.async_io(Interest::READABLE, |fd| rustix::io::read(fd.as_fd(), &mut *buf).map_err(io::Error::from)).await
    }
}

/// A byte-stream descriptor (`BlueZ`'s RFCOMM socket, one end of a platform's stream socket pair): each message is a
/// big-endian u16 length, then that many bytes. One task sends and one receives, so frames never interleave.
pub struct StreamLink(AsyncFd<OwnedFd>, LinkKind);

impl StreamLink {
    /// `kind` is what carries the stream: an RFCOMM socket is Bluetooth, a TCP stream over `adb reverse` is USB.
    pub fn new(fd: OwnedFd, kind: LinkKind) -> io::Result<Self> {
        rustix::io::ioctl_fionbio(&fd, true)?;
        // Sound for OwnedFd, whose descriptor never changes; `register` would need unsafe, which the workspace forbids.
        #[allow(deprecated)]
        Ok(Self(AsyncFd::new(fd)?, kind))
    }

    /// Fills `buf` whole; false when the peer closed before its first byte.
    async fn read_exact(&self, buf: &mut [u8]) -> io::Result<bool> {
        let mut filled = 0;
        while filled < buf.len() {
            let rest = buf.get_mut(filled..).unwrap_or_default();
            let read = self
                .0
                .async_io(Interest::READABLE, |fd| rustix::io::read(fd.as_fd(), &mut *rest).map_err(io::Error::from))
                .await?;
            if read == 0 {
                return if filled == 0 { Ok(false) } else { Err(io::ErrorKind::UnexpectedEof.into()) };
            }
            filled += read;
        }
        Ok(true)
    }
}

impl MessageLink for StreamLink {
    fn kind(&self) -> LinkKind {
        self.1
    }

    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        let len = u16::try_from(packet.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let frame = [&len.to_be_bytes()[..], packet].concat();
        let mut sent = 0;
        while let Some(rest) = frame.get(sent..).filter(|rest| !rest.is_empty()) {
            sent += self
                .0
                .async_io(Interest::WRITABLE, |fd| rustix::io::write(fd.as_fd(), rest).map_err(io::Error::from))
                .await?;
        }
        Ok(())
    }

    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut len = [0; 2];
        if !self.read_exact(&mut len).await? {
            return Ok(0);
        }
        let len = usize::from(u16::from_be_bytes(len));
        let packet = buf.get_mut(..len).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        // A zero-length frame would read as the peer closing.
        if len == 0 || !self.read_exact(packet).await? {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(len)
    }
}

struct Slot {
    /// One packet per message, counted against `QUEUE`.
    tx: mpsc::Sender<Vec<u8>>,
    /// The endpoint task waiting for room in `tx`.
    writable: Mutex<Option<Waker>>,
    /// Never sent; its receivers wake when the link ends and the slot drops.
    ended: watch::Sender<()>,
    /// The address the link's packets arrive from.
    addr: SocketAddr,
    kind: LinkKind,
    /// A connection is tied to the link.
    tied: AtomicBool,
}

/// Where a route sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hop {
    /// Over UDP to this address; for a route to a virtual address, the peer's real one.
    Udp(SocketAddr),
    Link(LinkId),
}

/// Destinations not sent as addressed, both ways round, changed only together.
#[derive(Default)]
struct Routes {
    to: HashMap<SocketAddr, Hop>,
    /// The reverse of each route: packets from the key (a UDP address or a link's own address) reach quinn as from
    /// the last peer in the list. A newer dial to the same address takes it over; its end gives it back.
    from: HashMap<SocketAddr, Vec<SocketAddr>>,
    /// Virtual addresses handed out and not yet released, routed or not.
    virtuals: HashSet<SocketAddr>,
    /// The last UDP address each virtual address was routed to, where it goes back to when its link ends.
    last_udp: HashMap<SocketAddr, SocketAddr>,
}

impl Routes {
    /// `source` is where the peer's packets arrive from over `via`.
    fn set(&mut self, peer: SocketAddr, via: Hop, source: SocketAddr) {
        self.remove(peer);
        if source != peer {
            self.from.entry(source).or_default().push(peer);
        }
        if source != peer || matches!(via, Hop::Link(_)) {
            self.to.insert(peer, via);
        }
        if let Hop::Udp(addr) = via
            && self.virtuals.contains(&peer)
        {
            self.last_udp.insert(peer, addr);
        }
    }

    fn remove(&mut self, peer: SocketAddr) {
        self.to.remove(&peer);
        self.from.retain(|_, owners| {
            owners.retain(|owner| *owner != peer);
            !owners.is_empty()
        });
    }
}

#[derive(Default)]
struct Shared {
    links: Mutex<Vec<Option<Arc<Slot>>>>,
    routes: Mutex<Routes>,
    next_virtual: AtomicU16,
    next_link: AtomicU16,
}

impl Shared {
    fn slot(&self, id: LinkId) -> Option<Arc<Slot>> {
        lock(&self.links).get(id).cloned().flatten()
    }

    /// Drops a link that closed or failed; its routes fall back to their last UDP address, or end without one.
    fn drop_link(&self, id: LinkId, why: &io::Error) {
        log::info!("path link {id}: {why}");
        if let Some(slot) = lock(&self.links).get_mut(id) {
            *slot = None;
        }
        let mut routes = lock(&self.routes);
        let peers: Vec<SocketAddr> =
            routes.to.iter().filter(|&(_, via)| *via == Hop::Link(id)).map(|(&peer, _)| peer).collect();
        for peer in peers {
            match routes.last_udp.get(&peer).copied() {
                Some(udp) => routes.set(peer, Hop::Udp(udp), canonical(udp)),
                None => routes.remove(peer),
            }
        }
    }
}

/// Where the last send that would block went, so the poller waits for room there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blocked {
    Udp,
    /// A batch of `packets` did not fit link `id`'s queue.
    Link {
        id: LinkId,
        packets: usize,
    },
}

pub struct PathSocket {
    udp: Arc<dyn AsyncUdpSocket>,
    /// The same socket, for packets quinn does not send.
    raw: std::net::UdpSocket,
    shared: Arc<Shared>,
    inbound_tx: mpsc::Sender<(SocketAddr, Vec<u8>)>,
    inbound: Mutex<mpsc::Receiver<(SocketAddr, Vec<u8>)>>,
    blocked: Mutex<Blocked>,
    /// The local address each UDP peer last sent to. quinn keeps a server connection's first local address for good
    /// (quinn-proto 0.11), which no longer exists once the desktop's network changes under a migrated phone.
    replies_from: Mutex<HashMap<SocketAddr, IpAddr>>,
}

impl fmt::Debug for PathSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathSocket").finish_non_exhaustive()
    }
}

/// The address quinn sees for the `n`th link a socket adds, in `2001:db8::/32`, which no real peer has.
pub fn link_address(n: u16) -> SocketAddr {
    SocketAddr::from((std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, n), 1))
}

/// The `n`th address a phone dials a desktop at, in `2001:db8:1::/64`; its route says where packets go.
pub fn virtual_address(n: u16) -> SocketAddr {
    SocketAddr::from((std::net::Ipv6Addr::new(0x2001, 0xdb8, 1, 0, 0, 0, 0, n), 1))
}

/// Whether quinn sees `addr` for a link or a virtual peer rather than a UDP peer.
pub fn is_link_address(addr: SocketAddr) -> bool {
    matches!(addr.ip().to_canonical(), std::net::IpAddr::V6(v6) if v6.segments()[..2] == [0x2001, 0xdb8])
}

/// Route key: quinn hands a dual-stack socket IPv4 peers as IPv4-mapped IPv6 addresses.
fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn send_with_ttl(socket: &std::net::UdpSocket, to: SocketAddr, ttl: u8) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    use nix::sys::socket::{ControlMessage, MsgFlags, SockaddrStorage, sendmsg};

    let hops = nix::libc::c_int::from(ttl);
    // A dual-stack socket sends to an IPv4-mapped address as IPv4, which takes IP_TTL.
    let ipv4 = match to {
        SocketAddr::V4(_) => true,
        SocketAddr::V6(v6) => v6.ip().to_ipv4_mapped().is_some(),
    };
    let control = if ipv4 { ControlMessage::Ipv4Ttl(&hops) } else { ControlMessage::Ipv6HopLimit(&hops) };
    let to = SockaddrStorage::from(to);
    sendmsg(socket.as_raw_fd(), &[io::IoSlice::new(&[0])], &[control], MsgFlags::empty(), Some(&to))?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn send_with_ttl(_: &std::net::UdpSocket, _: SocketAddr, _: u8) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PathSocket {
    pub fn new(udp: std::net::UdpSocket) -> io::Result<Arc<Self>> {
        let runtime = quinn::default_runtime().ok_or_else(|| io::Error::other("no async runtime"))?;
        let (inbound_tx, inbound) = mpsc::channel(INBOUND);
        Ok(Arc::new(Self {
            raw: udp.try_clone()?,
            udp: runtime.wrap_udp_socket(udp)?,
            shared: Arc::default(),
            inbound_tx,
            inbound: Mutex::new(inbound),
            blocked: Mutex::new(Blocked::Udp),
            replies_from: Mutex::new(HashMap::new()),
        }))
    }

    /// `addr` as the UDP socket takes it: a dual-stack socket sends to IPv4 peers at IPv4-mapped addresses.
    fn native(&self, addr: SocketAddr) -> SocketAddr {
        match (addr, self.udp.local_addr()) {
            (SocketAddr::V4(v4), Ok(SocketAddr::V6(_))) => SocketAddr::new(v4.ip().to_ipv6_mapped().into(), v4.port()),
            _ => addr,
        }
    }

    /// Sends 3 one-byte packets to each of `addresses` that end after `ttl` hops. They open this socket's NAT mapping
    /// toward a peer without reaching the peer's NAT, which would keep an entry that moves the peer's punch to another
    /// port. Only the packets themselves carry `ttl`, so QUIC and relay packets on the socket go on unchanged.
    pub fn open_toward(&self, addresses: &[SocketAddr], ttl: u8) {
        for &addr in addresses {
            for _ in 0..3 {
                if let Err(error) = send_with_ttl(&self.raw, self.native(addr), ttl) {
                    log::info!("opening toward {addr}: {error}");
                    break;
                }
            }
        }
    }

    /// Adds a link whose packets quinn sees as coming from `peer`. Runs a reader and a writer task on the current
    /// runtime until the link ends.
    pub fn add_link<L: MessageLink>(&self, link: L, peer: SocketAddr) -> LinkId {
        // A dual-stack socket reports IPv4 peers as IPv4-mapped, and a quinn client drops packets from any other form.
        let peer = self.native(peer);
        let link = Arc::new(link);
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let slot = Arc::new(Slot {
            tx,
            writable: Mutex::default(),
            ended: watch::Sender::new(()),
            addr: canonical(peer),
            kind: link.kind(),
            tied: AtomicBool::new(false),
        });
        let ended = slot.ended.subscribe();
        let id = {
            let mut links = lock(&self.shared.links);
            links.push(Some(slot.clone()));
            links.len() - 1
        };
        // Weak, so dropping the slot closes `rx` and ends this task.
        let (writer, shared, slot) = (link.clone(), self.shared.clone(), Arc::downgrade(&slot));
        tokio::spawn(async move {
            while let Some(packet) = rx.recv().await {
                if let Some(waker) = slot.upgrade().and_then(|slot| lock(&slot.writable).take()) {
                    waker.wake();
                }
                if let Err(error) = writer.send(&packet).await {
                    return shared.drop_link(id, &error);
                }
            }
        });
        let (inbound, shared, mut ended) = (self.inbound_tx.clone(), self.shared.clone(), ended);
        tokio::spawn(async move {
            let mut buf = vec![0; MAX_PACKET];
            loop {
                let received = tokio::select! {
                    // Closed elsewhere: dropping `link` here and in the writer closes it.
                    _ = ended.changed() => return,
                    received = link.recv(&mut buf) => received,
                };
                match received {
                    Ok(0) => return shared.drop_link(id, &io::ErrorKind::UnexpectedEof.into()),
                    Ok(len) => {
                        let Some(packet) = buf.get(..len) else { continue };
                        if inbound.send((peer, packet.to_vec())).await.is_err() {
                            return;
                        }
                    }
                    Err(error) => return shared.drop_link(id, &error),
                }
            }
        });
        id
    }

    /// Adds a link a peer opened, at a link address of its own that quinn sees the peer's packets come from. A link
    /// that no connection is tied to within [`UNUSED_LINK`] ends, so a peer cannot hold one open with garbage.
    pub fn accept_link(self: &Arc<Self>, link: impl MessageLink) -> LinkId {
        let peer = link_address(self.shared.next_link.fetch_add(1, Ordering::Relaxed).wrapping_add(1));
        let id = self.add_link(link, peer);
        self.route_to(peer).set(Hop::Link(id));
        let shared = Arc::downgrade(&self.shared);
        tokio::spawn(async move {
            tokio::time::sleep(UNUSED_LINK).await;
            let Some(shared) = shared.upgrade() else { return };
            if shared.slot(id).is_some_and(|slot| !slot.tied.load(Ordering::Relaxed)) {
                shared.drop_link(id, &io::Error::other("no connection came over it"));
            }
        });
        id
    }

    /// The path to `peer`, one connection's remote address, a real or a virtual one; UDP to `peer` until set.
    pub fn route_to(self: &Arc<Self>, peer: SocketAddr) -> Route {
        let state = RouteState { shared: Arc::downgrade(&self.shared), ..Default::default() };
        Route { socket: self.clone(), peer: canonical(peer), state: Arc::new(state) }
    }

    /// A route over `via` to a virtual address no other route uses. The address is freed when the last handle to
    /// the route goes: its clones and the congestion controllers made from it, so it outlives its connection.
    pub fn route_new(self: &Arc<Self>, via: Hop) -> Route {
        let peer = loop {
            let peer = virtual_address(self.shared.next_virtual.fetch_add(1, Ordering::Relaxed));
            if lock(&self.shared.routes).virtuals.insert(peer) {
                break peer;
            }
        };
        let release = Release { shared: Arc::downgrade(&self.shared), peer };
        let route = Route {
            socket: self.clone(),
            peer,
            state: Arc::new(RouteState {
                shared: Arc::downgrade(&self.shared),
                _release: Some(release),
                ..Default::default()
            }),
        };
        route.set(via);
        route
    }

    pub fn has_link(&self, id: LinkId) -> bool {
        self.shared.slot(id).is_some()
    }

    /// The link `peer` is reached over now.
    pub fn link_to(&self, peer: SocketAddr) -> Option<LinkId> {
        match lock(&self.shared.routes).to.get(&canonical(peer)) {
            Some(&Hop::Link(id)) => Some(id),
            _ => None,
        }
    }

    /// Ends link `id`; its routes fall back to UDP.
    pub fn close_link(&self, id: LinkId) {
        self.shared.drop_link(id, &io::Error::other("closed"));
    }

    /// Ties link `id` and `connection`, the one connection it carries: the link ends with the connection, and the
    /// connection with the link while no other route reaches its peer.
    pub fn tie(self: &Arc<Self>, id: LinkId, connection: quinn::Connection) {
        if let Some(slot) = self.shared.slot(id) {
            slot.tied.store(true, Ordering::Relaxed);
        }
        let (socket, link_closed) = (self.clone(), self.link_closed(id));
        tokio::spawn(async move {
            tokio::select! {
                () = link_closed => {
                    let remote = canonical(connection.remote_address());
                    if is_link_address(remote) && !lock(&socket.shared.routes).to.contains_key(&remote) {
                        connection.close(0u32.into(), b"link closed");
                    }
                }
                _ = connection.closed() => {
                    // The link carries the connection's own close packet first.
                    tokio::time::sleep(connection.rtt() * 3).await;
                    socket.close_link(id);
                }
            }
        });
    }

    /// Resolves once link `id` has ended, at once when it already has.
    pub fn link_closed(&self, id: LinkId) -> impl Future<Output = ()> + Send + 'static {
        let ended = self.shared.slot(id).map(|slot| slot.ended.subscribe());
        async move {
            if let Some(mut ended) = ended {
                let _ = ended.changed().await;
            }
        }
    }

    /// Enqueues the whole batch or nothing: a batch cut short would lose its last packets.
    fn send_link(&self, id: LinkId, slot: &Slot, transmit: &Transmit) -> io::Result<()> {
        let size = transmit.segment_size.unwrap_or(transmit.contents.len()).max(1);
        let packets = transmit.contents.len().div_ceil(size).max(1);
        match slot.tx.try_reserve_many(packets) {
            Ok(permits) => {
                for (permit, packet) in permits.zip(transmit.contents.chunks(size)) {
                    permit.send(packet.to_vec());
                }
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(())) => {
                *lock(&self.blocked) = Blocked::Link { id, packets };
                Err(io::ErrorKind::WouldBlock.into())
            }
            // The link ended and its routes went with it; QUIC sends the batch again, over UDP where the peer has a UDP
            // address.
            Err(mpsc::error::TrySendError::Closed(())) => Ok(()),
        }
    }
}

/// One connection's path: whether its peer is reached over a link or UDP. Changing it here also tells the
/// connection's congestion controller, so the two never disagree.
#[derive(Clone)]
pub struct Route {
    socket: Arc<PathSocket>,
    peer: SocketAddr,
    state: Arc<RouteState>,
}

#[derive(Default)]
struct RouteState {
    /// The link the route runs over plus one; 0 over UDP. Its kind comes from the link, so an ended link reads as UDP.
    link: AtomicUsize,
    shared: Weak<Shared>,
    /// Counts path changes; the controller starts afresh when it sees a new one.
    changes: AtomicU64,
    /// The last link a server's connection was tied to, so each move onto a link ties once.
    tied: Mutex<Option<LinkId>>,
    /// Held for its drop.
    _release: Option<Release>,
}

impl RouteState {
    fn link_kind(&self) -> Option<LinkKind> {
        let id = self.link.load(Ordering::Relaxed).checked_sub(1)?;
        self.shared.upgrade()?.slot(id).map(|slot| slot.kind)
    }
}

/// Frees a virtual address's route when its last user goes.
struct Release {
    shared: Weak<Shared>,
    peer: SocketAddr,
}

impl Drop for Release {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            let mut routes = lock(&shared.routes);
            routes.remove(self.peer);
            routes.virtuals.remove(&self.peer);
            routes.last_udp.remove(&self.peer);
        }
    }
}

impl Route {
    /// Sends to the peer over `via`.
    pub fn set(&self, via: Hop) {
        let slot = match via {
            Hop::Udp(_) => None,
            Hop::Link(id) => self.socket.shared.slot(id),
        };
        let source = match via {
            Hop::Udp(addr) => canonical(addr),
            Hop::Link(_) => slot.as_ref().map_or(self.peer, |slot| slot.addr),
        };
        lock(&self.socket.shared.routes).set(self.peer, via, source);
        let link = match via {
            Hop::Link(id) if slot.is_some() => id + 1,
            _ => 0,
        };
        self.state.link.store(link, Ordering::Relaxed);
        self.state.changes.fetch_add(1, Ordering::Relaxed);
    }

    /// A server's view: the peer moved the connection to `remote`, and quinn followed. Tells the controller whether
    /// which link carries it. True when the route moved onto, off or between links.
    pub fn follow(&self, remote: SocketAddr) -> bool {
        let link = self.socket.link_to(remote).map_or(0, |id| id + 1);
        let moved = self.state.link.swap(link, Ordering::Relaxed) != link;
        if moved {
            self.state.changes.fetch_add(1, Ordering::Relaxed);
        }
        moved
    }

    /// [`Route::follow`] for a server's `connection`, which also ends with a link it moved onto, and that link with it.
    pub fn follow_connection(&self, connection: &quinn::Connection) -> bool {
        let remote = connection.remote_address();
        let moved = self.follow(remote);
        if let Some(link) = self.socket.link_to(remote)
            && lock(&self.state.tied).replace(link) != Some(link)
        {
            self.socket.tie(link, connection.clone());
        }
        moved
    }

    /// Whether the connection runs over a Bluetooth link now.
    pub fn on_bluetooth(&self) -> bool {
        self.link_kind() == Some(LinkKind::Bluetooth)
    }

    /// The kind of link the connection runs over now; None over UDP.
    pub fn link_kind(&self) -> Option<LinkKind> {
        self.state.link_kind()
    }

    /// The address quinn reaches this route's peer at.
    #[must_use]
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Where the peer is reached now; None once its link ended.
    pub fn via(&self) -> Option<Hop> {
        let routes = lock(&self.socket.shared.routes);
        match routes.to.get(&self.peer) {
            Some(&via) => Some(via),
            None if is_link_address(self.peer) => None,
            None => Some(Hop::Udp(self.peer)),
        }
    }

    /// `inner` for this connection, capped at [`LINK_WINDOW`] while it runs over Bluetooth and rebuilt after every path
    /// change: a window learnt on one path is wrong on the other.
    pub fn congestion(
        &self,
        inner: Arc<dyn ControllerFactory + Send + Sync>,
    ) -> Arc<dyn ControllerFactory + Send + Sync> {
        Arc::new(PathCongestion { inner, state: self.state.clone() })
    }
}

struct PathCongestion {
    inner: Arc<dyn ControllerFactory + Send + Sync>,
    state: Arc<RouteState>,
}

impl ControllerFactory for PathCongestion {
    fn build(self: Arc<Self>, now: Instant, mtu: u16) -> Box<dyn Controller> {
        let seen = self.state.changes.load(Ordering::Relaxed);
        Box::new(PathController { controller: self.inner.clone().build(now, mtu), factory: self, seen, mtu })
    }
}

struct PathController {
    controller: Box<dyn Controller>,
    factory: Arc<PathCongestion>,
    /// The path change `controller` was built for.
    seen: u64,
    mtu: u16,
}

impl Controller for PathController {
    fn on_sent(&mut self, now: Instant, bytes: u64, last_packet_number: u64) {
        let changes = self.factory.state.changes.load(Ordering::Relaxed);
        if changes != self.seen {
            self.controller = self.factory.inner.clone().build(now, self.mtu);
            self.seen = changes;
        }
        self.controller.on_sent(now, bytes, last_packet_number);
    }

    fn on_ack(&mut self, now: Instant, sent: Instant, bytes: u64, app_limited: bool, rtt: &quinn_proto::RttEstimator) {
        self.controller.on_ack(now, sent, bytes, app_limited, rtt);
    }

    fn on_end_acks(&mut self, now: Instant, in_flight: u64, app_limited: bool, largest_acked: Option<u64>) {
        self.controller.on_end_acks(now, in_flight, app_limited, largest_acked);
    }

    fn on_congestion_event(&mut self, now: Instant, sent: Instant, persistent: bool, lost_bytes: u64) {
        self.controller.on_congestion_event(now, sent, persistent, lost_bytes);
    }

    fn on_mtu_update(&mut self, mtu: u16) {
        self.mtu = mtu;
        self.controller.on_mtu_update(mtu);
    }

    fn window(&self) -> u64 {
        let window = self.controller.window();
        if self.factory.state.link_kind() == Some(LinkKind::Bluetooth) { window.min(LINK_WINDOW) } else { window }
    }

    fn metrics(&self) -> quinn_proto::congestion::ControllerMetrics {
        self.controller.metrics()
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Self {
            controller: self.controller.clone_box(),
            factory: self.factory.clone(),
            seen: self.seen,
            mtu: self.mtu,
        })
    }

    fn initial_window(&self) -> u64 {
        self.controller.initial_window()
    }

    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> {
        self
    }
}

impl PathSocket {
    /// The source for a packet to `peer`: the address `peer` last sent to, where quinn asks for one.
    fn reply_from(&self, peer: SocketAddr, asked: Option<IpAddr>) -> Option<IpAddr> {
        asked.map(|asked| lock(&self.replies_from).get(&canonical(peer)).copied().unwrap_or(asked))
    }
}

impl AsyncUdpSocket for PathSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        let udp = self.udp.clone().create_io_poller();
        Box::pin(PathPoller { socket: self, udp })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let via = lock(&self.shared.routes).to.get(&canonical(transmit.destination)).copied();
        let result = match via {
            Some(Hop::Link(id)) => {
                // A link that ends here drops its routes too, so the next send goes elsewhere.
                return self.shared.slot(id).map_or(Ok(()), |slot| self.send_link(id, &slot, transmit));
            }
            Some(Hop::Udp(addr)) => self.udp.try_send(&Transmit {
                destination: self.native(addr),
                src_ip: self.reply_from(addr, transmit.src_ip),
                ..*transmit
            }),
            // Its link ended; no UDP route leads there.
            None if is_link_address(transmit.destination) => return Ok(()),
            None => self
                .udp
                .try_send(&Transmit { src_ip: self.reply_from(transmit.destination, transmit.src_ip), ..*transmit }),
        };
        if result.as_ref().is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock) {
            *lock(&self.blocked) = Blocked::Udp;
        }
        result
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if let (Some(buf), Some(first)) = (bufs.first_mut(), meta.first_mut())
            && let Poll::Ready(Some((addr, packet))) = lock(&self.inbound).poll_recv(cx)
        {
            let len = packet.len().min(buf.len());
            buf[..len].copy_from_slice(&packet[..len]);
            let addr = lock(&self.shared.routes)
                .from
                .get(&canonical(addr))
                .and_then(|owners| owners.last().copied())
                .unwrap_or(addr);
            *first = RecvMeta { addr, len, stride: len, ..RecvMeta::default() };
            return Poll::Ready(Ok(1));
        }
        let received = ready!(self.udp.poll_recv(cx, bufs, meta))?;
        {
            let mut replies_from = lock(&self.replies_from);
            for meta in meta.iter().take(received) {
                let Some(local) = meta.dst_ip else { continue };
                let peer = canonical(meta.addr);
                if replies_from.get(&peer) != Some(&local) {
                    // Each address a phone sends from adds one entry; old ones are only stale.
                    if replies_from.len() >= 256 {
                        replies_from.clear();
                    }
                    replies_from.insert(peer, local);
                }
            }
        }
        let routes = lock(&self.shared.routes);
        if !routes.from.is_empty() {
            for meta in meta.iter_mut().take(received) {
                if let Some(&peer) = routes.from.get(&canonical(meta.addr)).and_then(|owners| owners.last()) {
                    meta.addr = peer;
                }
            }
        }
        Poll::Ready(Ok(received))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.udp.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.udp.max_transmit_segments().min(QUEUE)
    }

    fn max_receive_segments(&self) -> usize {
        self.udp.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.udp.may_fragment()
    }
}

struct PathPoller {
    socket: Arc<PathSocket>,
    udp: Pin<Box<dyn UdpPoller>>,
}

impl fmt::Debug for PathPoller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathPoller").finish_non_exhaustive()
    }
}

impl UdpPoller for PathPoller {
    fn poll_writable(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let blocked = *lock(&self.socket.blocked);
        if let Blocked::Link { id, packets } = blocked
            && let Some(slot) = self.socket.shared.slot(id)
        {
            // Registered before the check, so a packet the writer takes in between still wakes this task.
            *lock(&slot.writable) = Some(cx.waker().clone());
            return if slot.tx.capacity() >= packets { Poll::Ready(Ok(())) } else { Poll::Pending };
        }
        self.udp.as_mut().poll_writable(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::tls::ServerPin;
    use crate::transport::{Dialed, Dialer, PathServer, path_server_endpoint};
    use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
    use std::time::Duration;

    type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

    const TOTAL: usize = 8 << 20;
    const CHUNK: usize = 64 << 10;

    fn byte(at: usize) -> u8 {
        u8::try_from(at % 251).unwrap_or(0)
    }

    /// One stream keeps flowing while the phone moves the connection from UDP to a link and back: it dials a virtual
    /// address, so quinn never sees the path change.
    #[tokio::test]
    async fn a_live_stream_survives_moves_between_udp_and_a_link() -> Result {
        let server_socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        // Dual-stack like the app's socket: quinn hands it IPv4 peers as IPv4-mapped IPv6 addresses.
        let phone_socket = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let server_addr = server_socket.local_addr()?;
        let (desk_end, phone_end) = socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        let link_addr: SocketAddr = "10.255.255.1:1".parse()?;
        let desk_link = server_socket.add_link(FdLink::new(desk_end)?, link_addr);
        server_socket.route_to(link_addr).set(Hop::Link(desk_link));
        let phone_link = phone_socket.add_link(FdLink::new(phone_end)?, link_address(2));

        let desktop = Identity::generate()?;
        let server = path_server_endpoint(&desktop, server_socket.clone())?;
        // A small send window keeps writes waiting on the wire, so the moves below happen with the stream in flight.
        let dialer = Dialer::on_path(&Identity::generate()?, phone_socket.clone())?.tuned(|t| {
            t.send_window(1 << 20);
        });
        let pin = ServerPin::Key(desktop.spki().fingerprint());

        let receiver = tokio::spawn(async move {
            let connection = server.accept().await.ok_or("no connection")?.await?;
            let mut recv = connection.accept_uni().await?;
            let (mut got, mut seen_link) = (0, false);
            while let Some(chunk) = recv.read_chunk(CHUNK, true).await? {
                assert!(chunk.bytes.iter().enumerate().all(|(i, b)| *b == byte(got + i)), "corrupt at {got}");
                got += chunk.bytes.len();
                seen_link |= connection.remote_address() == link_addr;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((got, seen_link, connection.remote_address()))
        });

        let Dialed { connection, route, .. } = dialer.dial(server_addr, pin).await?;
        let path = route.ok_or("a path dialer gives a route")?;
        let mut send = connection.open_uni().await?;
        for at in (0..TOTAL).step_by(CHUNK) {
            if at == TOTAL / 8 {
                path.set(Hop::Link(phone_link));
            }
            if at == TOTAL / 2 {
                path.set(Hop::Udp(server_addr));
            }
            let chunk: Vec<u8> = (at..at + CHUNK).map(byte).collect();
            send.write_all(&chunk).await?;
        }
        send.finish()?;
        let (got, seen_link, last) = receiver.await??;
        assert_eq!(got, TOTAL);
        assert!(seen_link, "the desktop never saw the connection on the link");
        assert_eq!(last.port(), phone_socket.local_addr()?.port(), "the connection did not move back to UDP");
        Ok(())
    }

    /// A second dial to the same address takes its packets over while it lives, and gives them back when it ends.
    #[tokio::test]
    async fn a_second_dial_to_an_address_gives_it_back_when_it_ends() -> Result {
        let server = PathServer::bind(&Identity::generate()?, "[::1]:0".parse()?)?;
        let server_addr = server.socket.local_addr()?;
        let phone = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let dialer = Dialer::on_path(&Identity::generate()?, phone.clone())?;
        let accepted = tokio::spawn(async move {
            let first = server.endpoint.accept().await.ok_or("no first")?.await?;
            let second = server.endpoint.accept().await.ok_or("no second")?.await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((server, first, second))
        });
        let first = dialer.dial(server_addr, ServerPin::Any).await?;
        let second = dialer.dial(server_addr, ServerPin::Any).await?;
        let (_server, on_desktop, second_on_desktop) = accepted.await??;
        second.connection.close(0u32.into(), b"done");
        drop(second);
        second_on_desktop.closed().await;
        // Its route goes once quinn drops the drained connection.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let mut send = on_desktop.open_uni().await?;
        send.write_all(b"still here").await?;
        send.finish()?;
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            first
                .connection
                .accept_uni()
                .await?
                .read_to_end(64)
                .await
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
        })
        .await??;
        assert_eq!(got, b"still here");
        Ok(())
    }

    /// A desktop connection that moved onto a link ends when that link ends, not at the idle timeout.
    #[tokio::test]
    async fn a_connection_that_moved_onto_a_link_ends_with_it() -> Result {
        let server = PathServer::bind(&Identity::generate()?, "[::1]:0".parse()?)?;
        let desktop = server.socket.clone();
        let server_addr = desktop.local_addr()?;
        let phone = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let (desk_end, phone_end) = socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        let desk_link = desktop.add_link(FdLink::new(desk_end)?, link_address(1));
        desktop.route_to(link_address(1)).set(Hop::Link(desk_link));
        let phone_link = phone.add_link(FdLink::new(phone_end)?, link_address(2));

        let accepted = tokio::spawn(async move {
            let incoming = server.endpoint.accept().await.ok_or("no connection")?;
            let (connecting, route) = server.accept(incoming)?;
            let connection = connecting.await?;
            route.follow_connection(&connection);
            connection.accept_uni().await?.read_to_end(64).await?;
            route.follow_connection(&connection);
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((server, connection))
        });
        let dialed = Dialer::on_path(&Identity::generate()?, phone.clone())?.dial(server_addr, ServerPin::Any).await?;
        dialed.route.as_ref().ok_or("a path dialer gives a route")?.set(Hop::Link(phone_link));
        let mut send = dialed.connection.open_uni().await?;
        send.write_all(b"over the link").await?;
        send.finish()?;
        let (_server, on_desktop) = accepted.await??;
        assert_eq!(on_desktop.remote_address(), link_address(1), "the desktop did not follow the move");

        desktop.close_link(desk_link);
        tokio::time::timeout(Duration::from_secs(2), on_desktop.closed()).await?;
        Ok(())
    }

    /// A connection that runs over a link ends at both ends when one end closes the link, not at the idle timeout.
    #[tokio::test]
    async fn a_connection_ends_with_its_link() -> Result {
        let server = PathServer::bind(&Identity::generate()?, "[::1]:0".parse()?)?;
        let desktop = server.socket.clone();
        let phone = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let (desk_end, phone_end) = socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        let (desk_addr, phone_addr) = (link_address(1), link_address(1));
        desktop.route_to(desk_addr).set(Hop::Link(desktop.add_link(FdLink::new(desk_end)?, desk_addr)));
        let phone_link = phone.add_link(FdLink::new(phone_end)?, phone_addr);

        let accepted = tokio::spawn(async move {
            let incoming = server.endpoint.accept().await.ok_or("no connection")?;
            let link = server.socket.link_to(incoming.remote_address()).ok_or("the server did not see the link")?;
            let (connecting, _route) = server.accept(incoming)?;
            let connection = connecting.await?;
            server.socket.tie(link, connection.clone());
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((server, connection))
        });
        let identity = Identity::generate()?;
        let dialed = Dialer::on_path(&identity, phone.clone())?.dial_hop(Hop::Link(phone_link), ServerPin::Any).await?;
        phone.tie(phone_link, dialed.connection.clone());
        let (_server, on_desktop) = accepted.await??;

        phone.close_link(phone_link);
        tokio::time::timeout(Duration::from_secs(2), dialed.connection.closed()).await?;
        tokio::time::timeout(Duration::from_secs(2), on_desktop.closed()).await?;
        Ok(())
    }

    /// QUIC runs over a byte-stream link (RFCOMM) as over a message one.
    #[tokio::test]
    async fn a_stream_runs_whole_over_a_byte_stream_link() -> Result {
        let server = PathServer::bind(&Identity::generate()?, "[::1]:0".parse()?)?;
        let phone = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let (desk_end, phone_end) = socketpair(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::CLOEXEC, None)?;
        server.socket.accept_link(StreamLink::new(desk_end, LinkKind::Bluetooth)?);
        let phone_link = phone.add_link(StreamLink::new(phone_end, LinkKind::Bluetooth)?, link_address(1));

        let received = tokio::spawn(async move {
            let incoming = server.endpoint.accept().await.ok_or("no connection")?;
            let (connecting, _route) = server.accept(incoming)?;
            let connection = connecting.await?;
            let mut recv = connection.accept_uni().await?;
            let mut got = 0;
            while let Some(chunk) = recv.read_chunk(CHUNK, true).await? {
                assert!(chunk.bytes.iter().enumerate().all(|(i, b)| *b == byte(got + i)), "corrupt at {got}");
                got += chunk.bytes.len();
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(got)
        });
        let identity = Identity::generate()?;
        let dialed = Dialer::on_path(&identity, phone.clone())?.dial_hop(Hop::Link(phone_link), ServerPin::Any).await?;
        let mut send = dialed.connection.open_uni().await?;
        for at in (0..TOTAL / 4).step_by(CHUNK) {
            send.write_all(&(at..at + CHUNK).map(byte).collect::<Vec<u8>>()).await?;
        }
        send.finish()?;
        assert_eq!(tokio::time::timeout(Duration::from_secs(20), received).await???, TOTAL / 4);
        Ok(())
    }

    /// A connection over UDP moves onto a USB link (a TCP stream) and back to UDP when the link ends, losing nothing;
    /// the USB link never gets the Bluetooth cap.
    #[tokio::test]
    async fn a_connection_moves_onto_a_usb_link_and_back_to_udp_when_it_ends() -> Result {
        let server = PathServer::bind(&Identity::generate()?, "127.0.0.1:0".parse()?)?;
        let server_addr = server.socket.local_addr()?;
        let phone = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let (phone_end, desk_end) = tokio::join!(tokio::net::TcpStream::connect(tcp.local_addr()?), tcp.accept());
        let usb = |stream: tokio::net::TcpStream| StreamLink::new(OwnedFd::from(stream.into_std()?), LinkKind::Usb);
        server.socket.accept_link(usb(desk_end?.0)?);
        let link = phone.add_link(usb(phone_end?)?, link_address(1));

        let dialer = Dialer::on_path(&Identity::generate()?, phone.clone())?;
        let accepted = tokio::spawn(async move {
            let desk = server.endpoint.accept().await.ok_or("no connection")?.await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>((desk, server))
        });
        let Dialed { connection, route, .. } = dialer.dial(server_addr, ServerPin::Any).await?;
        let path = route.ok_or("a path dialer gives a route")?;
        let (desk, _server) = accepted.await??;
        phone.tie(link, connection.clone());
        path.set(Hop::Link(link));
        assert_eq!(path.link_kind(), Some(LinkKind::Usb));
        assert!(!path.on_bluetooth(), "a USB link got the Bluetooth cap");

        let round = |label: &'static str| {
            let (phone, desk) = (connection.clone(), desk.clone());
            async move {
                let mut send = phone.open_uni().await?;
                send.write_all(label.as_bytes()).await?;
                send.finish()?;
                let got = desk.accept_uni().await?.read_to_end(64).await?;
                assert_eq!(got, label.as_bytes());
                Ok::<_, Box<dyn std::error::Error + Send + Sync>>(desk.remote_address())
            }
        };
        let from = tokio::time::timeout(Duration::from_secs(5), round("over usb")).await??;
        assert!(is_link_address(from), "the desktop did not see the connection on the USB link");

        phone.close_link(link);
        assert_eq!(path.link_kind(), None, "the route stayed on a link that ended");
        let from = tokio::time::timeout(Duration::from_secs(5), round("back on udp")).await??;
        assert_eq!(from.port(), phone.local_addr()?.port(), "the connection did not move back to UDP");
        assert!(connection.close_reason().is_none(), "the connection ended with its USB link");
        Ok(())
    }

    /// A frame that arrives in pieces, its length too, is still one message.
    #[tokio::test]
    async fn a_byte_stream_link_joins_a_frame_that_arrives_in_pieces() -> Result {
        let (ours, theirs) = socketpair(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::CLOEXEC, None)?;
        let link = StreamLink::new(theirs, LinkKind::Bluetooth)?;
        let packet: Vec<u8> = (0..1000).map(byte).collect();
        let frame = [&1000u16.to_be_bytes()[..], &packet].concat();
        let mut buf = vec![0; MAX_PACKET];
        let write = async {
            for piece in [&frame[..1], &frame[1..500], &frame[500..]] {
                tokio::task::yield_now().await;
                rustix::io::write(&ours, piece)?;
            }
            Ok::<_, io::Error>(())
        };
        let (len, written) = tokio::join!(link.recv(&mut buf), write);
        written?;
        assert_eq!(buf.get(..len?), Some(&packet[..]));
        Ok(())
    }

    /// An accepted link that no connection comes over ends.
    #[tokio::test(start_paused = true)]
    async fn an_accepted_link_without_a_connection_ends() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        let (_peer, ours) = socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        let id = socket.accept_link(FdLink::new(ours)?);
        tokio::time::sleep(UNUSED_LINK / 2).await;
        assert!(socket.has_link(id), "the link ended before its peer had time to dial");
        tokio::time::timeout(UNUSED_LINK, socket.link_closed(id)).await?;
        assert!(!socket.has_link(id));
        Ok(())
    }

    /// A reply leaves from the address the peer last sent to, not the stale one quinn keeps for the connection.
    #[tokio::test]
    async fn a_reply_leaves_from_the_address_the_peer_used() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("0.0.0.0:0")?)?;
        let port = socket.local_addr()?.port();
        let peer = std::net::UdpSocket::bind("127.0.0.1:0")?;
        peer.set_read_timeout(Some(Duration::from_secs(5)))?;
        peer.send_to(b"hi", ("127.0.0.2", port))?;
        let mut buf = [0u8; 16];
        let mut meta = [RecvMeta::default()];
        std::future::poll_fn(|cx| socket.poll_recv(cx, &mut [IoSliceMut::new(&mut buf)], &mut meta)).await?;
        let stale = std::net::IpAddr::from([127, 0, 0, 1]);
        socket.try_send(&Transmit {
            destination: peer.local_addr()?,
            ecn: None,
            contents: b"back",
            segment_size: None,
            src_ip: Some(stale),
        })?;
        let (_, from) = peer.recv_from(&mut buf)?;
        assert_eq!(from.ip(), std::net::IpAddr::from([127, 0, 0, 2]));
        Ok(())
    }

    /// A batch goes into a link's queue whole or not at all; a cut batch would lose its last packets.
    #[tokio::test]
    async fn a_batch_that_does_not_fit_waits_whole() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        let (tx, _rx) = mpsc::channel(QUEUE);
        let slot = Slot {
            tx,
            writable: Mutex::default(),
            ended: watch::Sender::new(()),
            addr: link_address(1),
            kind: LinkKind::Bluetooth,
            tied: AtomicBool::new(false),
        };
        for _ in 0..QUEUE - 2 {
            slot.tx.try_send(vec![0])?;
        }
        let batch = |packets: usize| Transmit {
            destination: "10.0.0.1:1".parse().unwrap_or(SocketAddr::from(([0; 4], 0))),
            ecn: None,
            contents: &[7; 30][..packets * 10],
            segment_size: Some(10),
            src_ip: None,
        };
        let three = socket.send_link(0, &slot, &batch(3));
        assert!(three.is_err_and(|error| error.kind() == io::ErrorKind::WouldBlock));
        assert_eq!(slot.tx.capacity(), 2, "a batch that did not fit left packets behind");
        assert_eq!(*lock(&socket.blocked), Blocked::Link { id: 0, packets: 3 });
        socket.send_link(0, &slot, &batch(2))?;
        assert_eq!(slot.tx.capacity(), 0);
        Ok(())
    }

    /// A link that carries nothing, of either kind.
    struct Silent<const RELAY: bool>;

    impl<const RELAY: bool> MessageLink for Silent<RELAY> {
        fn kind(&self) -> LinkKind {
            if RELAY { LinkKind::Relay } else { LinkKind::Bluetooth }
        }

        fn send(&self, _: &[u8]) -> impl Future<Output = io::Result<()>> + Send {
            std::future::ready(Ok(()))
        }

        async fn recv(&self, _: &mut [u8]) -> io::Result<usize> {
            std::future::pending().await
        }
    }

    /// The window stays under the link cap while the path is Bluetooth, not on a relay, and a path change starts a
    /// fresh controller.
    #[tokio::test]
    async fn the_window_follows_the_path() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        let addr = "10.0.0.1:1".parse()?;
        let path = socket.route_to(addr);
        let mut inner = quinn::congestion::NewRenoConfig::default();
        inner.initial_window(4 * LINK_WINDOW);
        let factory = path.congestion(Arc::new(inner));
        let start = Instant::now();
        let mut controller = factory.build(start, 1200);
        let initial = controller.window();
        let later = |ms| start + std::time::Duration::from_millis(ms);
        controller.on_congestion_event(later(2), later(1), false, 1200);
        assert!(controller.window() < initial, "a loss did not shrink the window");
        let relay = socket.add_link(Silent::<true>, link_address(1));
        path.set(Hop::Link(relay));
        assert!(!path.on_bluetooth() && controller.window() > LINK_WINDOW, "a relay got the Bluetooth cap");
        let bluetooth = socket.add_link(Silent::<false>, link_address(2));
        path.set(Hop::Link(bluetooth));
        assert!(path.on_bluetooth() && controller.window() <= LINK_WINDOW);
        path.set(Hop::Udp(addr));
        controller.on_sent(later(3), 1200, 1);
        assert_eq!(controller.window(), initial, "the window learnt before the moves survived them");
        Ok(())
    }
}
