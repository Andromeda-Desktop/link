//! Link over Bluetooth LE: each L2CAP connection-oriented channel on [`L2CAP_PSM`] becomes a message link of the
//! desktop's QUIC socket, so a phone without IP runs its usual QUIC session over Bluetooth. See "Bluetooth" in
//! `ARCHITECTURE.md`.

use std::io;
use std::path::Path;
use std::sync::Arc;

use bluer::l2cap::{SeqPacket, Socket, SocketAddr as L2capAddr};
use bluer::{Address, AddressType};
use link_core::bluetooth::L2CAP_PSM;
use link_core::path::{FdLink, LinkKind, MessageLink, PathSocket};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// Above any QUIC packet, so one SDU always holds one packet.
const MTU: u16 = 2048;
const BACKLOG: i32 = 4;
const ADAPTER: &str = "/org/bluez/hci0";
const ADVERTISEMENT_PATH: &str = "/org/andromeda/Link1/Advertisement";
/// E2E only: a Unix `SOCK_SEQPACKET` socket stands in for L2CAP, so the link path runs without radios.
const TEST_SOCKET: &str = "ANDROMEDA_LINK_TEST_L2CAP_SOCKET";

struct L2cap(SeqPacket);

impl MessageLink for L2cap {
    const KIND: LinkKind = LinkKind::Bluetooth;

    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        self.0.send(packet).await.map(drop)
    }

    async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.recv(buf).await
    }
}

/// Accepts links on the test socket when one is set, else on the radio when `radio` (`BlueZ` took the RFCOMM
/// profile). Logs and gives up when neither works: RFCOMM still carries Bluetooth.
pub async fn start(socket: Arc<PathSocket>, radio: bool) {
    if let Some(path) = std::env::var_os(TEST_SOCKET) {
        if let Err(error) = test_listen(path.as_ref(), socket) {
            log::error!("the test L2CAP socket: {error}");
        }
        return;
    }
    if !radio {
        return;
    }
    let listener = match listen() {
        Ok(listener) => listener,
        Err(error) => return log::info!("Bluetooth LE unavailable: {error}"),
    };
    // A phone connects only to an adapter that advertises; the bus connection keeps the advertisement registered.
    let bus = match advertise().await {
        Ok(bus) => bus,
        Err(error) => return log::info!("Bluetooth LE advertising unavailable: {error}"),
    };
    log::info!("Bluetooth LE: listening on PSM {L2CAP_PSM:#x}");
    tokio::spawn(async move {
        let _bus = bus;
        while let Ok((link, from)) = listener.accept().await {
            log::info!("Bluetooth LE link from {}", from.addr);
            socket.accept_link(L2cap(link));
        }
        log::warn!("Bluetooth LE: the listener stopped");
    });
}

fn listen() -> io::Result<bluer::l2cap::SeqPacketListener> {
    let l2cap = Socket::<SeqPacket>::new_seq_packet()?;
    l2cap.bind(L2capAddr::new(Address::any(), AddressType::LePublic, L2CAP_PSM))?;
    l2cap.set_recv_mtu(MTU)?;
    l2cap.listen(BACKLOG.unsigned_abs())
}

async fn advertise() -> zbus::Result<zbus::Connection> {
    let bus = zbus::Connection::system().await?;
    bus.object_server().at(ADVERTISEMENT_PATH, Advertisement).await?;
    let manager = zbus::Proxy::new(&bus, "org.bluez", ADAPTER, "org.bluez.LEAdvertisingManager1").await?;
    let path = zbus::ObjectPath::try_from(ADVERTISEMENT_PATH)?;
    let options = std::collections::HashMap::<&str, zbus::Variant<'_>>::new();
    manager.call::<_, _, ()>("RegisterAdvertisement", &(path, options)).await?;
    Ok(bus)
}

/// Connectable and nothing else: the phone dials the adapter's address it already knows, so no name or service
/// tells nearby scanners what this desktop runs.
struct Advertisement;

// zbus needs `&self` on each method, even where the body does not use it.
#[allow(clippy::unused_self)]
#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    fn release(&self) {
        log::info!("BlueZ released the Link advertisement");
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> &'static str {
        "peripheral"
    }
}

fn test_listen(path: &Path, socket: Arc<PathSocket>) -> io::Result<()> {
    drop(std::fs::remove_file(path));
    let flags = SocketFlags::CLOEXEC | SocketFlags::NONBLOCK;
    let listener = rustix::net::socket_with(AddressFamily::UNIX, SocketType::SEQPACKET, flags, None)?;
    rustix::net::bind(&listener, &SocketAddrUnix::new(path)?)?;
    rustix::net::listen(&listener, BACKLOG)?;
    // Sound for OwnedFd, whose descriptor never changes; `register` would need unsafe, which the workspace forbids.
    #[allow(deprecated)]
    let listener = AsyncFd::new(listener)?;
    tokio::spawn(async move {
        loop {
            let accepted = listener
                .async_io(Interest::READABLE, |fd| {
                    rustix::net::accept_with(fd, SocketFlags::CLOEXEC).map_err(io::Error::from)
                })
                .await;
            match accepted.and_then(FdLink::new) {
                Ok(link) => {
                    log::info!("Bluetooth LE link from the test socket");
                    socket.accept_link(link);
                }
                Err(error) => return log::error!("the test L2CAP socket: {error}"),
            }
        }
    });
    Ok(())
}
