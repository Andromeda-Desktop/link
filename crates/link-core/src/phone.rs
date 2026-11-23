//! The phone role: pairing with desktops, reaching them, and the sessions it opens. Shared by the headless phone and
//! the Android app.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use link_proto::message::{
    BT_PAIRING, CALLS, CAMERA, CAPABILITIES, CLIP_CLOCK, CLIP_ITEMS, CONTROL, FEATURE, FS_STAT, FS_WRITE, HEARTBEAT,
    Hello, Hotspot, LATE_HASH, MESSAGES, MIRROR_CODECS, MIRROR_LAG, Message, PHOTOS_PAGE, PHOTOS_RECENT, PairMethod,
    SHARE_OUTBOX, SYSTEM, SYSTEM_CONTROL, TEXTS_MMS, bluetooth_address, public_address, relay_entry,
};
use link_proto::pairing::Secret;
use link_proto::{CloseCode, VERSION};
use ring::rand::{SecureRandom, SystemRandom};

use crate::bluetooth::BluetoothOpener;
use crate::browse::Roots;
use crate::control::{Control, Tap};
use crate::hotspot::{HotspotProvider, WifiDirectProvider};
use crate::identity::{DeviceId, Identity};
use crate::pairing::pair_as_client;
use crate::path::{FdLink, Hop, LinkId, MessageLink, PathSocket, Route, StreamLink, link_address};
use crate::reach::{self, Reached, Via};
use crate::relay::{self, Relay};
use crate::store::{Feature, Peer, Store};
use crate::tls::{self, ServerPin};
use crate::transport::{Dialed, Dialer};
use crate::uri::PairingUri;
use crate::wire::Connection;
use crate::{Error, discovery};

pub enum PairTarget {
    /// The typed code, and where to find the desktop (empty: the one desktop advertising an open window).
    Code {
        code: String,
        candidates: Vec<SocketAddr>,
    },
    Uri(PairingUri),
}

/// A live connection to a desktop, after both hellos.
pub struct Session {
    pub connection: Connection,
    pub control: Control,
    pub desktop: Peer,
    /// None over Bluetooth or the relay.
    pub addr: Option<SocketAddr>,
    pub via: Via,
    pub resumed: bool,
    /// The connection's path, for a QUIC session.
    pub route: Option<Route>,
    /// The relay connection the session runs through, for [`Via::Relay`].
    pub relay: Option<Relay>,
}

pub struct Phone {
    identity: Identity,
    store: Store,
    store_path: PathBuf,
    name: String,
    dialer: Dialer,
    present_dialer: Dialer,
    /// Under both dialers, so a session can run over a Bluetooth LE link.
    socket: Arc<PathSocket>,
    /// Links added to `socket`, to give each its own address.
    links: u16,
    bluetooth: Option<Arc<dyn BluetoothOpener>>,
    hotspot: Option<Arc<dyn HotspotProvider>>,
    wifi_direct: Option<Arc<dyn WifiDirectProvider>>,
    browse_roots: Roots,
    present: bool,
    tap: Option<Tap>,
}

impl Phone {
    /// Where the store is saved; other phone state lives beside it.
    pub fn store_path(&self) -> &Path {
        &self.store_path
    }

    pub fn new(identity: Identity, store_path: PathBuf, name: String, tap: Option<Tap>) -> Result<Self, Error> {
        let store = Store::load(&store_path)?;
        let udp = std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
            .or_else(|_| std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
        let socket = PathSocket::new(udp)?;
        let dialer = Dialer::on_path(&identity, socket.clone())?;
        let present_dialer = dialer.present();
        Ok(Self {
            identity,
            store,
            store_path,
            name,
            dialer,
            present_dialer,
            socket,
            links: 0,
            bluetooth: None,
            hotspot: None,
            wifi_direct: None,
            browse_roots: Roots::default(),
            present: false,
            tap,
        })
    }

    /// Whether connections opened from now on keep themselves alive.
    pub fn set_present(&mut self, present: bool) {
        self.present = present;
    }

    /// Lets sessions fall back to Bluetooth when no IP path answers.
    pub fn set_bluetooth(&mut self, opener: Arc<dyn BluetoothOpener>) {
        self.bluetooth = Some(opener);
    }

    /// Lets a Bluetooth session move to this phone's hotspot for transfers too large for Bluetooth.
    pub fn set_hotspot(&mut self, provider: Arc<dyn HotspotProvider>) {
        self.hotspot = Some(provider);
    }

    pub fn hotspot(&self) -> Option<Arc<dyn HotspotProvider>> {
        self.hotspot.clone()
    }

    /// Lets a Bluetooth session try a Wi-Fi Direct group before the hotspot, with desktops that can join one.
    pub fn set_wifi_direct(&mut self, provider: Arc<dyn WifiDirectProvider>) {
        self.wifi_direct = Some(provider);
    }

    pub fn wifi_direct(&self) -> Option<Arc<dyn WifiDirectProvider>> {
        self.wifi_direct.clone()
    }

    /// The folders a desktop with the browse switch on sees at `/`.
    pub fn set_browse_roots(&mut self, roots: Roots) {
        self.browse_roots = roots;
    }

    pub fn browse_roots(&self) -> &Roots {
        &self.browse_roots
    }

    /// The dialer sessions opened now use, for a caller that reaches a desktop off the actor.
    pub fn dialer(&self) -> &Dialer {
        if self.present { &self.present_dialer } else { &self.dialer }
    }

    /// The UDP socket under both dialers.
    pub fn socket(&self) -> &Arc<PathSocket> {
        &self.socket
    }

    pub fn desktops(&self) -> &[Peer] {
        &self.store.peers
    }

    /// Pairs over IP when any address answers, else over Bluetooth when the QR code names the desktop's adapter, or,
    /// for a typed code, when one desktop advertises its window over Bluetooth LE.
    pub async fn pair(&mut self, target: PairTarget) -> Result<Session, Error> {
        let (candidates, pin, secret, method, bluetooth) = match target {
            PairTarget::Code { code, candidates } => {
                if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(Error::BadCode);
                }
                let (candidates, bluetooth) = if candidates.is_empty() {
                    self.pairing_desktop().await?
                } else {
                    (candidates, PairBluetooth::None)
                };
                (candidates, ServerPin::Any, Secret::new(code.into_bytes()), PairMethod::Code, bluetooth)
            }
            PairTarget::Uri(uri) => (
                uri.addresses,
                ServerPin::Key(uri.fingerprint),
                Secret::new(uri.secret.to_vec()),
                PairMethod::Qr,
                uri.bluetooth.map_or(PairBluetooth::None, PairBluetooth::Known),
            ),
        };
        let (connection, server, addr, route) = match reach::race(self.dialer(), &candidates, pin).await {
            Ok((dialed, addr)) => {
                let server = tls::peer_spki(dialed.connection.peer_identity())?;
                (Connection::from(dialed.connection), server, Some(addr), dialed.route)
            }
            Err(ip) => {
                let address = match bluetooth {
                    PairBluetooth::None => return Err(ip),
                    PairBluetooth::Known(address) => address,
                    PairBluetooth::Scan(scan) => match scanned(scan).await?.as_slice() {
                        [address] => address.clone(),
                        [] if candidates.is_empty() => return Err(Error::NoPairingDesktop),
                        [] => return Err(ip),
                        _ => return Err(Error::ManyPairingDesktops),
                    },
                };
                let Some(opener) = self.bluetooth.clone() else { return Err(ip) };
                log::info!("pairing: no IP path ({ip}); trying Bluetooth");
                let fd = match tokio::task::spawn_blocking(move || opener.open(&address, true))
                    .await
                    .map_err(|_| Error::Stopped)?
                {
                    Ok(fd) => fd,
                    Err(bluetooth) => {
                        let tried: Vec<String> = candidates.iter().map(ToString::to_string).collect();
                        let ip = if tried.is_empty() { "no address".to_owned() } else { tried.join(", ") };
                        return Err(Error::PairUnreached { ip, bluetooth: bluetooth.to_string() });
                    }
                };
                let dialed = self.dial_link(StreamLink::new(fd)?, pin).await?;
                let server = tls::peer_spki(dialed.connection.peer_identity())?;
                (Connection::from(dialed.connection), server, None, dialed.route)
            }
        };
        let mut control = Control::open(&connection, self.tap.clone()).await?;
        let hello = control.hello_as_client(self.hello()).await?;
        pair_as_client(&connection, &server, &mut control, &secret, method, self.identity.spki()).await?;
        let desktop = self.remember(&server, &hello, addr)?;
        let via = if addr.is_some() { Via::LastKnown } else { Via::Bluetooth };
        Ok(Session { connection, control, desktop, addr, via, resumed: false, route, relay: None })
    }

    /// The one desktop with an open pairing window by mDNS, if any, and an LE scan for desktops that advertise one,
    /// which runs meanwhile and counts only when IP fails. Never two desktops: a wrong code spends the other's window.
    async fn pairing_desktop(&self) -> Result<(Vec<SocketAddr>, PairBluetooth), Error> {
        let opener = self.bluetooth.clone();
        let scan = tokio::task::spawn_blocking(move || opener.map(|opener| opener.scan_pairing()));
        // With no network, mDNS fails: the scan may still find the desktop.
        let browsed = discovery::browse(reach::MDNS_WINDOW, None).await.unwrap_or_else(|error| {
            log::info!("pairing: no mDNS browse: {error}");
            Vec::new()
        });
        let open: Vec<_> = browsed.into_iter().filter(|desktop| desktop.pairing).collect();
        match open.as_slice() {
            [desktop] => Ok((desktop.addresses.clone(), PairBluetooth::Scan(scan))),
            [] => Ok((Vec::new(), PairBluetooth::Scan(scan))),
            _ => Err(Error::ManyPairingDesktops),
        }
    }

    /// Reaches a paired desktop. A desktop that answers `unpaired` is forgotten, as the protocol requires.
    pub async fn connect(&mut self, id: &DeviceId) -> Result<Session, Error> {
        let peer = self.store.peer(id).cloned().ok_or(Error::UnknownDevice)?;
        let result = self.open_session(&peer).await;
        if matches!(result, Err(Error::Closed(CloseCode::Unpaired))) {
            self.forget(id)?;
        }
        result
    }

    /// Tells the desktop, then forgets it. Returns whether the desktop was told; it is forgotten either way.
    pub async fn unpair(&mut self, id: &DeviceId) -> Result<bool, Error> {
        let told = match self.connect(id).await {
            Ok(mut session) => {
                session.control.send(Message::Unpair).await?;
                drop(session.connection.closed().await);
                true
            }
            Err(error) => {
                log::info!("unpairing {id} without telling it: {error}");
                false
            }
        };
        self.forget(id)?;
        Ok(told)
    }

    /// Waits until every connection has sent its close; call before the process exits.
    pub async fn finish(&self) {
        self.dialer.finish().await;
    }

    /// Over IP when any address answers, else over Bluetooth when the desktop has an adapter and the platform can
    /// open one, else through the desktop's relay when it has one.
    async fn open_session(&mut self, peer: &Peer) -> Result<Session, Error> {
        match reach::reach(self.dialer(), peer).await {
            Ok(reached) => self.adopt_reached(peer, reached).await,
            Err(error) => self.connect_indirect(peer, error).await,
        }
    }

    /// With no IP path (`ip` tells why): over Bluetooth when the desktop has an adapter and the platform can open one,
    /// else through the desktop's relay when it has one.
    pub async fn connect_indirect(&mut self, peer: &Peer, ip: Error) -> Result<Session, Error> {
        let mut error = ip;
        if peer.bluetooth.is_some() && self.bluetooth.is_some() {
            log::info!("{}: no IP path ({error}); trying Bluetooth", peer.id);
            match self.connect_bluetooth(peer).await {
                Ok(session) => return Ok(session),
                Err(bluetooth) => error = bluetooth,
            }
        }
        let Some(spec) = peer.relay.clone() else { return Err(error) };
        log::info!("{}: no direct path ({error}); trying the relay {spec}", peer.id);
        self.connect_relay(peer, &spec).await
    }

    /// A session through the relay in `spec`, on a relay connection that ends with it.
    async fn connect_relay(&mut self, peer: &Peer, spec: &str) -> Result<Session, Error> {
        let (host, key) = relay::parse(spec).ok_or(Error::Unreachable)?;
        let desktop = relay::tag(&peer.spki()?);
        let addr = tokio::net::lookup_host(&host).await?.next().ok_or(Error::Unreachable)?;
        let dialer = self.dialer().tuned(|transport| {
            transport.initial_mtu(relay::MTU).keep_alive_interval(Some(relay::KEEP_ALIVE));
        });
        let outer = dialer.dial(addr, ServerPin::Key(key)).await?.connection;
        let (relay, _) = Relay::new(outer.clone());
        let link = relay.link(desktop);
        let session =
            self.connect_link(peer, link, Via::Relay).await.map(|session| Session { relay: Some(relay), ..session });
        let inner = session.as_ref().ok().map(|session| session.connection.quic().clone());
        tokio::spawn(async move {
            if let Some(inner) = inner {
                inner.closed().await;
                // The inner close goes out as a datagram on `outer` once its link sends it; closing `outer` first
                // would drop it.
                tokio::time::sleep(inner.rtt() * 3 + outer.rtt() * 3).await;
            }
            Connection::from(outer).close(CloseCode::Done);
        });
        session
    }

    /// Opens an LE L2CAP channel to `peer` when run, a blocking call for a worker thread.
    pub fn l2cap_opener(&self, peer: &Peer) -> Option<impl FnOnce() -> std::io::Result<OwnedFd> + Send + 'static> {
        let (address, opener) = (peer.bluetooth.clone()?, self.bluetooth.clone()?);
        Some(move || opener.open_l2cap(&address))
    }

    /// Adds `fd` as a standby link that ends with `connection`; the connection's route can move onto it.
    pub fn add_standby(&mut self, fd: OwnedFd, connection: quinn::Connection) -> Result<LinkId, Error> {
        self.links = self.links.wrapping_add(1);
        let link = self.socket.add_link(FdLink::new(fd)?, link_address(self.links));
        self.socket.tie(link, connection);
        Ok(link)
    }

    /// Whether link `id` still runs.
    pub fn has_link(&self, id: LinkId) -> bool {
        self.socket.has_link(id)
    }

    /// A session over Bluetooth now, without trying IP first: the phone just lost Wi-Fi. QUIC over an LE L2CAP
    /// channel where the phone and desktop have one, else over RFCOMM.
    pub async fn connect_bluetooth(&mut self, peer: &Peer) -> Result<Session, Error> {
        let (Some(address), Some(opener)) = (peer.bluetooth.clone(), self.bluetooth.clone()) else {
            return Err(Error::Unreachable);
        };
        let le = (address.clone(), opener.clone());
        let l2cap = tokio::task::spawn_blocking(move || le.1.open_l2cap(&le.0)).await.map_err(|_| Error::Stopped)?;
        let tried = match l2cap {
            Ok(fd) => self.connect_link(peer, FdLink::new(fd)?, Via::Bluetooth).await,
            Err(error) => Err(error.into()),
        };
        match tried {
            Ok(session) => return Ok(session),
            Err(error) => log::info!("{}: no QUIC over Bluetooth LE ({error}); trying RFCOMM", peer.id),
        }
        let fd =
            tokio::task::spawn_blocking(move || opener.open(&address, false)).await.map_err(|_| Error::Stopped)??;
        self.connect_link(peer, StreamLink::new(fd)?, Via::Bluetooth).await
    }

    /// Dials the desktop pinned to `fingerprint` over `link`, which ends with the connection.
    async fn dial_link(&mut self, link: impl MessageLink, pin: ServerPin) -> Result<Dialed, Error> {
        self.links = self.links.wrapping_add(1);
        let link = self.socket.add_link(link, link_address(self.links));
        match self.dialer().dial_hop(Hop::Link(link), pin).await {
            Ok(dialed) => {
                self.socket.tie(link, dialed.connection.clone());
                Ok(dialed)
            }
            Err(error) => {
                self.socket.close_link(link);
                Err(error)
            }
        }
    }

    async fn connect_link(&mut self, peer: &Peer, link: impl MessageLink, via: Via) -> Result<Session, Error> {
        let Dialed { connection, resumed, route } =
            self.dial_link(link, ServerPin::Key(peer.spki()?.fingerprint())).await?;
        let connection = Connection::from(connection);
        let mut control = Control::open(&connection, self.tap.clone()).await?;
        let hello = control.hello_as_client(self.hello()).await?;
        let desktop = self.remember(&peer.spki()?, &hello, None)?;
        Ok(Session { connection, control, desktop, addr: None, via, resumed, route, relay: None })
    }

    /// Finishes a session to a desktop reached over IP, by this phone or by a probe running beside it.
    pub async fn adopt_reached(&mut self, peer: &Peer, reached: Reached) -> Result<Session, Error> {
        let Dialed { connection, resumed, route } = reached.dialed;
        let connection = Connection::from(connection);
        let mut control = Control::open(&connection, self.tap.clone()).await?;
        let hello = control.hello_as_client(self.hello()).await?;
        let desktop = if matches!(reached.via, Via::Hotspot | Via::WifiDirect) {
            // The phone's own hotspot or group is gone once it stops: neither the address it reached nor the ones the desktop
            // announces on it are worth dialling later.
            let addresses = hello.addresses.into_iter().filter(|entry| entry.parse::<SocketAddr>().is_err()).collect();
            self.remember(&peer.spki()?, &Hello { addresses, ..hello }, None)?
        } else {
            self.remember(&peer.spki()?, &hello, Some(reached.addr))?
        };
        Ok(Session {
            connection,
            control,
            desktop,
            addr: Some(reached.addr),
            via: reached.via,
            resumed,
            route,
            relay: None,
        })
    }

    fn hello(&self) -> Hello {
        let features = [
            FS_STAT,
            FS_WRITE,
            CAMERA,
            PHOTOS_RECENT,
            PHOTOS_PAGE,
            MESSAGES,
            TEXTS_MMS,
            CALLS,
            CONTROL,
            CAPABILITIES,
            SYSTEM,
            SYSTEM_CONTROL,
            CLIP_ITEMS,
            CLIP_CLOCK,
            SHARE_OUTBOX,
            HEARTBEAT,
            LATE_HASH,
            MIRROR_CODECS,
            MIRROR_LAG,
        ]
        .map(str::to_owned);
        Hello { version: VERSION, name: self.name.clone(), addresses: features.to_vec() }
    }

    fn remember(
        &mut self,
        desktop: &crate::identity::Spki,
        hello: &Hello,
        addr: Option<SocketAddr>,
    ) -> Result<Peer, Error> {
        let id = desktop.device_id();
        let mut peer = self.store.peer(&id).cloned().unwrap_or_else(|| Peer::new(desktop, hello.name.clone()));
        peer.name.clone_from(&hello.name);
        if let Some(addr) = addr {
            peer.remember(addr);
        }
        peer.learn(hello.addresses.iter().filter_map(|text| text.parse().ok()));
        if let Some(bluetooth) = hello.addresses.iter().find_map(|text| bluetooth_address(text)) {
            peer.bluetooth = Some(bluetooth);
        }
        peer.relay = hello.addresses.iter().find_map(|entry| relay_entry(entry)).map(str::to_owned);
        peer.public = hello.addresses.iter().find_map(|entry| public_address(entry));
        peer.bt_pairing = hello.addresses.iter().any(|entry| entry == BT_PAIRING);
        peer.features = hello.addresses.iter().filter(|entry| entry.starts_with(FEATURE)).cloned().collect();
        peer.touch();
        self.store.upsert(peer.clone());
        self.store.save(&self.store_path)?;
        Ok(peer)
    }

    pub fn set_sharing(&mut self, id: &DeviceId, feature: Feature, on: bool) -> Result<(), Error> {
        let peer = self.store.peer_mut(id).ok_or(Error::UnknownDevice)?;
        peer.grants.set(feature, on);
        self.store.save(&self.store_path)
    }

    /// The group credentials for desktop `id`, made and saved on first use.
    pub fn group_for(&mut self, id: &DeviceId) -> Result<Hotspot, Error> {
        let peer = self.store.peer_mut(id).ok_or(Error::UnknownDevice)?;
        if let Some(group) = &peer.group {
            return Ok(group.clone());
        }
        // Android takes only names that start with "DIRECT-" and two characters.
        let group =
            Hotspot { ssid: format!("DIRECT-{}-Link", random_text(2)?), passphrase: random_text(16)?, mhz: None };
        peer.group = Some(group.clone());
        self.store.save(&self.store_path)?;
        Ok(group)
    }

    pub fn forget(&mut self, id: &DeviceId) -> Result<(), Error> {
        self.store.remove(id);
        self.store.save(&self.store_path)
    }
}

impl Session {
    pub fn close(&self) {
        self.connection.close(CloseCode::Done);
    }
}

/// Where pairing goes over Bluetooth when no IP path answers.
enum PairBluetooth {
    None,
    /// The adapter the QR code names.
    Known(String),
    /// The LE scan for desktops advertising a window: none without a platform opener.
    Scan(tokio::task::JoinHandle<Option<std::io::Result<Vec<String>>>>),
}

/// The distinct addresses the scan found.
async fn scanned(scan: tokio::task::JoinHandle<Option<std::io::Result<Vec<String>>>>) -> Result<Vec<String>, Error> {
    let mut found = match scan.await.map_err(|_| Error::Stopped)? {
        Some(Ok(found)) => found,
        Some(Err(error)) => {
            log::info!("pairing: no Bluetooth scan: {error}");
            Vec::new()
        }
        None => Vec::new(),
    };
    found.sort();
    found.dedup();
    Ok(found)
}

/// `length` characters without look-alikes, which Android accepts in a group name and passphrase.
fn random_text(length: usize) -> Result<String, Error> {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut text = String::with_capacity(length);
    let mut byte = [0u8];
    while text.len() < length {
        SystemRandom::new().fill(&mut byte).map_err(|_| Error::Unexpected("no randomness for a Wi-Fi Direct group"))?;
        // Bytes from 224 = 4 x 56 up are skipped, so every character is equally likely.
        if usize::from(byte[0]) < ALPHABET.len() * 4 {
            text.push(char::from(ALPHABET[usize::from(byte[0]) % ALPHABET.len()]));
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    async fn each_desktop_keeps_its_own_group_after_a_restart() {
        let path = std::env::temp_dir().join(format!("link-phone-group-{}.json", std::process::id()));
        let (first, second) = (Identity::generate().expect("key"), Identity::generate().expect("key"));
        let mut store = Store::default();
        for desktop in [&first, &second] {
            store.upsert(Peer::new(desktop.spki(), "desktop".to_owned()));
        }
        store.save(&path).expect("save");
        let (first, second) = (first.spki().device_id(), second.spki().device_id());
        let open =
            || Phone::new(Identity::generate().expect("key"), path.clone(), "phone".to_owned(), None).expect("phone");
        let mut phone = open();
        let group = phone.group_for(&first).expect("group");
        let other = phone.group_for(&second).expect("group");
        let again = open().group_for(&first).expect("group");
        drop(std::fs::remove_file(&path));
        assert_eq!(group, again);
        assert_ne!(group, other);
        assert!(group.ssid.starts_with("DIRECT-") && group.passphrase.len() == 16, "{group:?}");
    }
}
