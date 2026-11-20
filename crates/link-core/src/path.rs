//! One QUIC endpoint over UDP and message links (Bluetooth L2CAP, a `SOCK_SEQPACKET` descriptor). A link carries one
//! QUIC packet per message. quinn sees each link as a peer address: the server sees a link's own address, so a phone
//! that moves between UDP and a link migrates the same connection; the phone routes the desktop's address over the
//! link it picks and reports the link's packets as coming from that address.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::os::fd::{AsFd, OwnedFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use quinn::congestion::{Controller, ControllerFactory};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

pub type LinkId = usize;

/// Packets queued per link before a send would block: about 90 KB, a few round trips of a Bluetooth link. Also the
/// most segments one send may carry, so a whole batch always fits an empty queue.
const QUEUE: usize = 64;
/// The congestion window while a connection runs over a link. The Android Bluetooth stack takes ~250 KB of writes at
/// once, and any controller fills it (rtt 2-2.7 s); 12 KiB kept a loaded ping p95 at 123-126 ms on the test phone
/// with bulk unchanged at ~1 Mbit/s.
pub const LINK_WINDOW: u64 = 12 << 10;
/// Inbound packets from all links waiting for the endpoint.
const INBOUND: usize = 256;
/// The largest packet a link may deliver; QUIC packets stay far below it.
const MAX_PACKET: usize = 1 << 16;

/// A connected link that keeps message boundaries: one `send` is one `recv` on the other end.
pub trait MessageLink: Send + Sync + 'static {
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

struct Slot {
    /// One packet per message, counted against `QUEUE`.
    tx: mpsc::Sender<Vec<u8>>,
    /// The endpoint task waiting for room in `tx`.
    writable: Mutex<Option<Waker>>,
}

#[derive(Default)]
struct Shared {
    links: Mutex<Vec<Option<Arc<Slot>>>>,
    /// Destinations sent over a link instead of UDP.
    routes: Mutex<HashMap<SocketAddr, LinkId>>,
}

impl Shared {
    fn slot(&self, id: LinkId) -> Option<Arc<Slot>> {
        lock(&self.links).get(id).cloned().flatten()
    }

    /// Drops a link that closed or failed; its routes fall back to UDP.
    fn drop_link(&self, id: LinkId, why: &io::Error) {
        log::info!("path link {id}: {why}");
        if let Some(slot) = lock(&self.links).get_mut(id) {
            *slot = None;
        }
        lock(&self.routes).retain(|_, link| *link != id);
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
    shared: Arc<Shared>,
    inbound_tx: mpsc::Sender<(SocketAddr, Vec<u8>)>,
    inbound: Mutex<mpsc::Receiver<(SocketAddr, Vec<u8>)>>,
    blocked: Mutex<Blocked>,
}

impl fmt::Debug for PathSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathSocket").finish_non_exhaustive()
    }
}

/// Route key: quinn hands a dual-stack socket IPv4 peers as IPv4-mapped IPv6 addresses.
fn canonical(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PathSocket {
    pub fn new(udp: std::net::UdpSocket) -> io::Result<Arc<Self>> {
        let runtime = quinn::default_runtime().ok_or_else(|| io::Error::other("no async runtime"))?;
        let (inbound_tx, inbound) = mpsc::channel(INBOUND);
        Ok(Arc::new(Self {
            udp: runtime.wrap_udp_socket(udp)?,
            shared: Arc::default(),
            inbound_tx,
            inbound: Mutex::new(inbound),
            blocked: Mutex::new(Blocked::Udp),
        }))
    }

    /// Adds a link whose packets quinn sees as coming from `peer`. Runs a reader and a writer task on the current
    /// runtime until the link ends.
    pub fn add_link(&self, link: impl MessageLink, peer: SocketAddr) -> LinkId {
        // A dual-stack socket reports IPv4 peers as IPv4-mapped, and a quinn client drops packets from any other form.
        let peer = match (peer, self.udp.local_addr()) {
            (SocketAddr::V4(v4), Ok(SocketAddr::V6(_))) => SocketAddr::new(v4.ip().to_ipv6_mapped().into(), v4.port()),
            _ => peer,
        };
        let link = Arc::new(link);
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let slot = Arc::new(Slot { tx, writable: Mutex::default() });
        let id = {
            let mut links = lock(&self.shared.links);
            links.push(Some(slot.clone()));
            links.len() - 1
        };
        let (writer, shared) = (link.clone(), self.shared.clone());
        tokio::spawn(async move {
            while let Some(packet) = rx.recv().await {
                if let Some(waker) = lock(&slot.writable).take() {
                    waker.wake();
                }
                if let Err(error) = writer.send(&packet).await {
                    return shared.drop_link(id, &error);
                }
            }
        });
        let (inbound, shared) = (self.inbound_tx.clone(), self.shared.clone());
        tokio::spawn(async move {
            let mut buf = vec![0; MAX_PACKET];
            loop {
                match link.recv(&mut buf).await {
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

    /// The path to `peer`, one connection's remote address.
    pub fn route_to(self: &Arc<Self>, peer: SocketAddr) -> Route {
        Route { socket: self.clone(), peer: canonical(peer), state: Arc::default() }
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
            // The link ended and its routes went with it; QUIC sends the batch again over UDP.
            Err(mpsc::error::TrySendError::Closed(())) => Ok(()),
        }
    }
}

/// One connection's path: whether its peer is reached over a link or UDP. Changing it here also tells the
/// connection's congestion controller, so the two never disagree.
pub struct Route {
    socket: Arc<PathSocket>,
    peer: SocketAddr,
    state: Arc<RouteState>,
}

#[derive(Default)]
struct RouteState {
    on_link: AtomicBool,
    /// Counts path changes; the controller starts afresh when it sees a new one.
    changes: AtomicU64,
}

impl Route {
    /// Sends to the peer over `via`, or over UDP with `None`.
    pub fn set(&self, via: Option<LinkId>) {
        {
            let mut routes = lock(&self.socket.shared.routes);
            match via {
                Some(link) => routes.insert(self.peer, link),
                None => routes.remove(&self.peer),
            };
        }
        self.state.on_link.store(via.is_some(), Ordering::Relaxed);
        self.state.changes.fetch_add(1, Ordering::Relaxed);
    }

    /// `inner` for this connection, capped at [`LINK_WINDOW`] while it runs over a link and rebuilt after every path
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
        if self.factory.state.on_link.load(Ordering::Relaxed) { window.min(LINK_WINDOW) } else { window }
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

impl AsyncUdpSocket for PathSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        let udp = self.udp.clone().create_io_poller();
        Box::pin(PathPoller { socket: self, udp })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let route = lock(&self.shared.routes).get(&canonical(transmit.destination)).copied();
        if let Some(id) = route
            && let Some(slot) = self.shared.slot(id)
        {
            return self.send_link(id, &slot, transmit);
        }
        let result = self.udp.try_send(transmit);
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
            *first = RecvMeta { addr, len, stride: len, ..RecvMeta::default() };
            return Poll::Ready(Ok(1));
        }
        self.udp.poll_recv(cx, bufs, meta)
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
    use crate::transport::{Dialer, path_server_endpoint};
    use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};

    type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

    const TOTAL: usize = 8 << 20;
    const CHUNK: usize = 64 << 10;

    fn byte(at: usize) -> u8 {
        u8::try_from(at % 251).unwrap_or(0)
    }

    /// One stream keeps flowing while the phone moves the connection from UDP to a link and back.
    #[tokio::test]
    async fn a_live_stream_survives_moves_between_udp_and_a_link() -> Result {
        let server_socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        // Dual-stack like the app's socket: quinn hands it IPv4 peers as IPv4-mapped IPv6 addresses.
        let phone_socket = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
        let server_addr = server_socket.local_addr()?;
        let (desk_end, phone_end) = socketpair(AddressFamily::UNIX, SocketType::SEQPACKET, SocketFlags::CLOEXEC, None)?;
        let link_addr: SocketAddr = "10.255.255.1:1".parse()?;
        let desk_link = server_socket.add_link(FdLink::new(desk_end)?, link_addr);
        server_socket.route_to(link_addr).set(Some(desk_link));
        let phone_link = phone_socket.add_link(FdLink::new(phone_end)?, server_addr);

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

        let path = phone_socket.route_to(server_addr);
        let connection = dialer.dial(server_addr, pin).await?.connection;
        let mut send = connection.open_uni().await?;
        for at in (0..TOTAL).step_by(CHUNK) {
            if at == TOTAL / 8 {
                path.set(Some(phone_link));
            }
            if at == TOTAL / 2 {
                path.set(None);
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

    /// A batch goes into a link's queue whole or not at all; a cut batch would lose its last packets.
    #[tokio::test]
    async fn a_batch_that_does_not_fit_waits_whole() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        let (tx, _rx) = mpsc::channel(QUEUE);
        let slot = Slot { tx, writable: Mutex::default() };
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

    /// The window stays under the link cap while the path is a link, and a path change starts a fresh controller.
    #[tokio::test]
    async fn the_window_follows_the_path() -> Result {
        let socket = PathSocket::new(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
        let path = socket.route_to("10.0.0.1:1".parse()?);
        let mut inner = quinn::congestion::NewRenoConfig::default();
        inner.initial_window(4 * LINK_WINDOW);
        let factory = path.congestion(Arc::new(inner));
        let start = Instant::now();
        let mut controller = factory.build(start, 1200);
        let initial = controller.window();
        let later = |ms| start + std::time::Duration::from_millis(ms);
        controller.on_congestion_event(later(2), later(1), false, 1200);
        assert!(controller.window() < initial, "a loss did not shrink the window");
        path.set(Some(0));
        assert!(controller.window() <= LINK_WINDOW);
        path.set(None);
        controller.on_sent(later(3), 1200, 1);
        assert_eq!(controller.window(), initial, "the window learnt before the moves survived them");
        Ok(())
    }
}
