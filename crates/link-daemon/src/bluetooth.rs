//! Link over Bluetooth, desktop side: an RFCOMM profile registered with `BlueZ`, whose connections arrive as
//! descriptors the listener adds as links of the QUIC endpoint. See "Bluetooth" in `ARCHITECTURE.md`.

#[cfg(target_os = "linux")]
use std::collections::HashMap;

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
#[cfg(target_os = "linux")]
use zbus::{OwnedObjectPath, OwnedVariant, Variant};

#[cfg(target_os = "macos")]
#[path = "bluetooth_mac.rs"]
mod mac;

#[cfg(unix)]
pub use link_core::bluetooth::SERVICE_UUID;
#[cfg(target_os = "linux")]
const PROFILE_PATH: &str = "/org/andromeda/Link1/Bluetooth";
/// The RFCOMM channel the service record names; fixed, since the record must name the channel it listens on.
#[cfg(unix)]
const CHANNEL: u16 = 22;
#[cfg(target_os = "linux")]
/// The SDP record: `BlueZ`'s own for a custom UUID lacks what Android needs to accept it ("Bad Service Class ID list
/// attribute"), so the phone would never find the channel.
const SERVICE_RECORD: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<record>
  <attribute id="0x0001"><sequence><uuid value="5c3b1e5a-7d2f-4a8e-9b61-3f0c2d4e8a17" /></sequence></attribute>
  <attribute id="0x0004">
    <sequence>
      <sequence><uuid value="0x0100" /></sequence>
      <sequence><uuid value="0x0003" /><uint8 value="0x16" /></sequence>
    </sequence>
  </attribute>
  <attribute id="0x0005"><sequence><uuid value="0x1002" /></sequence></attribute>
  <attribute id="0x0100"><text value="Andromeda Link" /></attribute>
</record>"#;
#[cfg(target_os = "linux")]
const ADAPTER: &str = "/org/bluez/hci0";
#[cfg(target_os = "linux")]
const BEACON_PATH: &str = "/org/andromeda/Link1/Bluetooth/beacon";
/// E2E only: a Unix socket stands in for RFCOMM, so the RFCOMM path runs without radios.
#[cfg(unix)]
const TEST_SOCKET: &str = "ANDROMEDA_LINK_TEST_BLUETOOTH_SOCKET";
#[cfg(unix)]
const TEST_ADDRESS: &str = "00:00:00:00:00:01";
/// Connections waiting for the listener; more are refused.
const BACKLOG: usize = 4;

/// Where Bluetooth connections come from. With no adapter, `address` is None and nothing arrives.
pub struct Bluetooth {
    pub address: Option<String>,
    pub incoming: mpsc::Receiver<Connection>,
    /// What keeps the service registered; None for the test socket.
    #[cfg_attr(
        any(target_os = "macos", windows),
        expect(dead_code, reason = "macOS keeps the service on its run loop thread; Windows registers none")
    )]
    registration: Option<Registration>,
}

/// The profile lives as long as this connection to the system bus.
#[cfg(target_os = "linux")]
type Registration = zbus::Connection;
#[cfg(not(target_os = "linux"))]
type Registration = ();

/// A connected RFCOMM socket. Windows has no Bluetooth backend yet, so no connection exists there.
#[cfg(unix)]
pub type Connection = std::os::fd::OwnedFd;
#[cfg(windows)]
pub enum Connection {}

impl Bluetooth {
    /// Opens the adapter to pairing during each window; the test socket needs no radio.
    #[cfg_attr(not(target_os = "linux"), expect(clippy::unused_self, reason = "only BlueZ needs a pairing window"))]
    pub fn pairing_window(&self) -> watch::Sender<Option<Instant>> {
        #[cfg(target_os = "linux")]
        if self.registration.is_some() {
            return open_for_pairing();
        }
        // macOS asks the user itself when an unpaired device connects.
        watch::channel(None).0
    }

    /// Advertises over Bluetooth LE while it holds true, so a phone that types the pairing code finds this desktop
    /// with no shared network. The advertisement's service data under [`SERVICE_UUID`] is the adapter address.
    #[cfg_attr(not(target_os = "linux"), expect(clippy::unused_self, reason = "only BlueZ advertises"))]
    pub fn pairing_beacon(&self) -> watch::Sender<bool> {
        let (tx, rx) = watch::channel(false);
        #[cfg(target_os = "linux")]
        if let (Some(_), Some(address)) = (&self.registration, &self.address) {
            tokio::spawn(beacon(address.clone(), rx));
        }
        #[cfg(not(target_os = "linux"))]
        drop(rx);
        tx
    }

    /// Whether `BlueZ` serves Link on the adapter, not the test socket.
    #[cfg(target_os = "linux")]
    pub fn on_radio(&self) -> bool {
        self.registration.is_some()
    }

    #[cfg_attr(
        windows,
        expect(clippy::unused_async, clippy::unused_async_trait_impl, reason = "the backends register asynchronously")
    )]
    pub async fn start() -> Self {
        let (tx, incoming) = mpsc::channel(BACKLOG);
        #[cfg(unix)]
        if let Some(path) = std::env::var_os(TEST_SOCKET) {
            return match test_socket(path.as_ref(), tx) {
                Ok(()) => Self { address: Some(TEST_ADDRESS.to_owned()), incoming, registration: None },
                Err(error) => {
                    log::error!("the test Bluetooth socket: {error}");
                    Self { address: None, incoming, registration: None }
                }
            };
        }
        #[cfg(target_os = "linux")]
        let registered = register(tx).await.map_err(|error| error.to_string());
        #[cfg(target_os = "macos")]
        let registered = mac::register(tx).await.map(|address| ((), address));
        #[cfg(windows)]
        let registered: Result<((), String), _> = {
            drop(tx);
            Err("no Bluetooth backend on Windows yet")
        };
        match registered {
            Ok((registration, address)) => {
                log::info!("Bluetooth: listening on {address}");
                Self { address: Some(address), incoming, registration: Some(registration) }
            }
            Err(error) => {
                log::info!("Bluetooth unavailable: {error}");
                Self { address: None, incoming, registration: None }
            }
        }
    }
}

#[cfg(target_os = "linux")]
async fn register(tx: mpsc::Sender<Connection>) -> zbus::Result<(zbus::Connection, String)> {
    let bus = zbus::Connection::system().await?;
    let adapter = zbus::Proxy::new(&bus, "org.bluez", ADAPTER, "org.bluez.Adapter1").await?;
    let address: String = adapter.get_property("Address").await?;
    bus.object_server().at(PROFILE_PATH, Profile { incoming: tx }).await?;
    let manager = zbus::Proxy::new(&bus, "org.bluez", "/org/bluez", "org.bluez.ProfileManager1").await?;
    // Link's TLS authenticates both ends with the paired keys, so BlueZ is not asked to bond or to prompt.
    let options: HashMap<&str, Variant<'_>> = HashMap::from([
        ("Name", Variant::from("Andromeda Link")),
        ("Role", Variant::from("server")),
        ("RequireAuthentication", Variant::from(false)),
        ("RequireAuthorization", Variant::from(false)),
        ("AutoConnect", Variant::from(false)),
        ("Channel", Variant::from(CHANNEL)),
        ("ServiceRecord", Variant::from(SERVICE_RECORD)),
    ]);
    let path = zbus::ObjectPath::try_from(PROFILE_PATH)?;
    manager.call::<_, _, ()>("RegisterProfile", &(path, SERVICE_UUID, options)).await?;
    Ok((bus, address.to_ascii_uppercase()))
}

#[cfg(target_os = "linux")]
struct Profile {
    incoming: mpsc::Sender<Connection>,
}

#[cfg(target_os = "linux")]
// zbus hands methods owned arguments and needs `&self` on each, even where the body uses neither.
#[allow(clippy::unused_self, clippy::needless_pass_by_value)]
#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {
        log::info!("BlueZ released the Link profile");
    }

    fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: zbus::OwnedFd,
        properties: HashMap<String, OwnedVariant>,
    ) -> zbus::fdo::Result<()> {
        log::info!("Bluetooth connection from {}", device.as_str());
        log::debug!("its RFCOMM properties: {properties:?}");
        self.incoming
            .try_send(fd.into())
            .map_err(|_| zbus::fdo::Error::LimitsExceeded("too many Bluetooth connections waiting".to_owned()))
    }

    fn request_disconnection(&self, device: OwnedObjectPath) {
        log::info!("BlueZ asks to disconnect {}", device.as_str());
    }
}

#[cfg(target_os = "linux")]
/// Keeps the adapter accepting connections from devices it is not paired with while a pairing window is open, so a
/// phone with no IP path can pair over Bluetooth; `BlueZ` otherwise accepts only paired devices. Each send is the
/// latest window's deadline. Restores the setting only if it was the one to change it.
pub fn open_for_pairing() -> watch::Sender<Option<Instant>> {
    let (tx, mut rx) = watch::channel(None::<Instant>);
    tokio::spawn(async move {
        let mut opened = false;
        loop {
            let until = *rx.borrow_and_update();
            if let Some(until) = until.filter(|until| Instant::now() < *until) {
                if !opened {
                    opened = connectable(true).await.is_ok_and(|was| !was);
                }
                tokio::select! {
                    () = tokio::time::sleep_until(until) => {}
                    changed = rx.changed() => if changed.is_err() { break },
                }
                continue;
            }
            if opened {
                drop(connectable(false).await);
                opened = false;
            }
            if rx.changed().await.is_err() {
                break;
            }
        }
    });
    tx
}

#[cfg(target_os = "linux")]
/// Sets whether the adapter accepts new connections and returns what it was.
async fn connectable(on: bool) -> zbus::Result<bool> {
    let bus = zbus::Connection::system().await?;
    let adapter = zbus::Proxy::new(&bus, "org.bluez", ADAPTER, "org.bluez.Adapter1").await?;
    let was: bool = adapter.get_property("Connectable").await?;
    if was != on {
        adapter.set_property("Connectable", on).await.map_err(zbus::Error::from)?;
        log::info!(
            "Bluetooth: {} connections from unpaired devices",
            if on { "accepting" } else { "no longer accepting" }
        );
    }
    Ok(was)
}

#[cfg(target_os = "linux")]
async fn beacon(address: String, mut on: watch::Receiver<bool>) {
    let mut advertising: Option<zbus::Connection> = None;
    loop {
        let want = *on.borrow_and_update();
        if want && advertising.is_none() {
            advertising = advertise(&address)
                .await
                .inspect_err(|error| log::info!("Bluetooth: no pairing advertisement: {error}"))
                .ok();
        } else if let Some(bus) = advertising.take_if(|_| !want) {
            // BlueZ drops a client's advertisements when its bus connection closes.
            drop(bus);
            log::info!("Bluetooth: pairing advertisement stopped");
        }
        if on.changed().await.is_err() {
            break;
        }
    }
}

#[cfg(target_os = "linux")]
async fn advertise(address: &str) -> zbus::Result<zbus::Connection> {
    let data = address
        .split(':')
        .map(|byte| u8::from_str_radix(byte, 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| zbus::Error::Failure(format!("adapter address {address}")))?;
    let bus = zbus::Connection::system().await?;
    bus.object_server().at(BEACON_PATH, Beacon { data }).await?;
    let manager = zbus::Proxy::new(&bus, "org.bluez", ADAPTER, "org.bluez.LEAdvertisingManager1").await?;
    let path = zbus::ObjectPath::try_from(BEACON_PATH)?;
    manager.call::<_, _, ()>("RegisterAdvertisement", &(path, HashMap::<&str, Variant<'_>>::new())).await?;
    log::info!("Bluetooth: advertising the pairing window");
    Ok(bus)
}

/// A non-connectable LE advertisement: [`SERVICE_UUID`] with the adapter address as service data. 24 of the 31 bytes.
#[cfg(target_os = "linux")]
struct Beacon {
    data: Vec<u8>,
}

#[cfg(target_os = "linux")]
#[allow(clippy::unused_self)]
#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Beacon {
    fn release(&self) {}

    #[zbus(property, name = "Type")]
    fn kind(&self) -> &'static str {
        "broadcast"
    }

    #[zbus(property)]
    fn service_data(&self) -> HashMap<String, Variant<'static>> {
        HashMap::from([(SERVICE_UUID.to_owned(), Variant::from(self.data.clone()))])
    }
}

#[cfg(unix)]
fn test_socket(path: &std::path::Path, tx: mpsc::Sender<Connection>) -> std::io::Result<()> {
    drop(std::fs::remove_file(path));
    let listener = tokio::net::UnixListener::bind(path)?;
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let Ok(stream) = stream.into_std() else { continue };
            if tx.send(stream.into()).await.is_err() {
                return;
            }
        }
    });
    Ok(())
}
