//! `andromeda-linkd`: the desktop side of Link. No UI; clients drive it over `org.andromeda.Link1`, on the session bus
//! or, where there is none, on a Unix socket.

mod bluetooth;
mod browse;
mod capabilities;
mod control;
mod dbus;
mod desktop_media;
#[cfg_attr(target_os = "macos", path = "hotspot_mac.rs")]
mod hotspot;
mod hub;
mod ifaddrs;
#[cfg(target_os = "linux")]
mod iwd;
#[cfg(target_os = "linux")]
mod l2cap;
mod listener;
mod localsend;
mod low_delay;
mod media;
mod messages;
mod mirror;
mod mpris;
mod notifications;
mod paths;
mod quickshare;
mod relay;
mod remote;
mod save;
mod socket;
#[cfg_attr(target_os = "macos", path = "wifi_direct_none.rs")]
mod wifi_direct;

use std::net::{Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use link_core::identity::Identity;
use link_core::inbox::Inbox;
use link_core::store::Store;
use link_core::transfer;
use link_core::transport::PathServer;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,zbus=warn,tracing=warn"))
        .format_timestamp(None)
        .init();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    match std::env::args().nth(1).as_deref() {
        None => runtime.block_on(run()),
        Some("doctor") => runtime.block_on(doctor()),
        Some(other) => anyhow::bail!("unknown command {other:?}; the only command is doctor"),
    }
}

/// Prints each paired device's state, path, round trip, losses and outbox depth, read from the running daemon.
async fn doctor() -> anyhow::Result<()> {
    let bus = link_api::connect().await.context("connecting to the session bus")?;
    let link = link_api::Link1Proxy::new(&bus).await?;
    let devices = link.doctor().await.context("asking andromeda-linkd (is it running?)")?;
    if devices.is_empty() {
        println!("no paired devices");
        return Ok(());
    }
    println!("{:<20} {:<12} {:<9} {:>9} {:>6} {:>6}  id", "device", "state", "path", "rtt", "lost", "outbox");
    for (id, name, state, path, rtt, lost, outbox) in devices {
        let rtt = if rtt == 0 { "-".to_owned() } else { format!("{:.1} ms", f64::from(rtt) / 1000.0) };
        let path = if path.is_empty() { "-" } else { &path };
        println!("{name:<20.20} {state:<12} {path:<9} {rtt:>9} {lost:>6} {outbox:>6}  {id}");
    }
    Ok(())
}

async fn run() -> anyhow::Result<()> {
    let paths = paths::Paths::create()?;
    save::sweep(&paths.downloads);
    let identity = Identity::load_or_create(&paths.identity).context("loading the device key")?;
    let mut store = Store::load(&paths.devices).context("loading the device store")?;
    let server = Arc::new(bind(&identity, store.port)?);
    let (endpoint, puncher) = (server.endpoint.clone(), server.puncher.clone());
    let port = endpoint.local_addr()?.port();
    if store.port != port {
        store.port = port;
        store.save(&paths.devices)?;
    }
    let transport = match link_api::socket_path() {
        Some(path) => Transport::Socket(path),
        None => Transport::Bus(zbus::Connection::session().await.context("connecting to the session bus")?),
    };
    let name = dbus::device_name().await;
    log::info!("{} listening on port {port} as {:?}", identity.device_id(), name);

    let state_dir = paths.state.clone();
    let inbox = Inbox::new(paths.downloads.clone(), &paths.state).context("opening the transfer state")?;
    let (transfers, transfer_actor, transfer_events) = transfer::transfers(inbox);
    let (signals, localsend_signals) = tokio::sync::mpsc::unbounded_channel();
    let (localsend_actor, localsend, nearby) =
        localsend::LocalSend::new(&paths.state, paths.downloads.clone(), name.clone(), signals)?;
    let transfers = hub::Transfers { handle: transfers, events: transfer_events, localsend, localsend_signals };
    let (desktop_media, media_requests) = desktop_media::channel();
    wifi_direct::remove_stale().await;
    let mdns = link_core::discovery::responder().inspect_err(|error| log::warn!("mdns: {error}")).ok();
    let (mut hub, handle, snapshots, events) =
        hub::Hub::new(identity.spki().clone(), store, paths, desktop_media, transfers, puncher, mdns.clone());
    let bluetooth = bluetooth::Bluetooth::start().await;
    #[cfg(target_os = "linux")]
    l2cap::start(server.socket.clone(), bluetooth.on_radio()).await;
    let relay = relay::spec(&state_dir.join("relay"));
    let (relay_seen_tx, relay_seen) = tokio::sync::watch::channel(None);
    if let Some(spec) = &relay {
        relay::start(server.clone(), spec.clone(), relay_seen_tx);
    }
    if let Some(address) = &bluetooth.address {
        hub.set_bluetooth(address.clone(), bluetooth.pairing_window(), bluetooth.pairing_beacon());
    }
    if let Transport::Bus(bus) = &transport {
        dbus::serve(bus, handle.clone(), snapshots.clone(), nearby.clone()).await?;
    }
    let (quick_share, qs_handle, qs_watches, qs_events) = quickshare::QuickShare::new(&name, &state_dir, mdns.clone())?;
    if let Transport::Bus(bus) = &transport {
        quickshare::serve(bus, qs_handle.clone(), qs_watches.clone(), name.clone()).await?;
        // Clients query on NameOwnerChanged: every interface is exported before the name is owned.
        bus.request_name(dbus::NAME).await?;
    }
    let api = socket::Api {
        hub: handle.clone(),
        snapshots,
        nearby,
        quick_share: qs_handle,
        quick_share_watches: qs_watches,
        name: name.clone(),
    };
    let listener =
        listener::Listener::new(server, bluetooth, relay, relay_seen, handle.clone(), identity.spki().clone(), name);
    let mut terminate = signal(SignalKind::terminate())?;
    #[cfg(not(target_os = "linux"))]
    let replaced = std::future::pending::<()>;
    #[cfg(not(target_os = "linux"))]
    let follow_addresses = |_| std::future::pending::<()>();
    let stop = async {
        tokio::select! {
            () = replaced() => Err(anyhow::anyhow!("an upgrade replaced andromeda-linkd; exiting to run the new one")),
            _ = terminate.recv() => Ok(()),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    };
    let result = tokio::select! {
        result = hub.run(stop) => result,
        () = follow_addresses(mdns) => Ok(()),
        () = transfer_actor.run() => Ok(()),
        result = listener.run() => result,
        result = serve_api(transport, api, (events, qs_events), media_requests) => result,
        () = localsend_actor.run() => Ok(()),
        result = quick_share.run() => result,
    };
    endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    endpoint.wait_idle().await;
    result
}

enum Transport {
    Bus(zbus::Connection),
    Socket(PathBuf),
}

/// Forwards state and events to the API's clients; desktop media control is MPRIS on the bus, media keys otherwise.
async fn serve_api(
    transport: Transport,
    api: socket::Api,
    (events, qs_events): (mpsc::Receiver<hub::Event>, mpsc::Receiver<quickshare::Event>),
    media_requests: mpsc::Receiver<desktop_media::Request>,
) -> anyhow::Result<()> {
    match transport {
        Transport::Socket(path) => {
            let hub = api.hub.clone();
            tokio::select! {
                result = socket::serve(&path, api, events, qs_events) => result,
                result = desktop_media::now_playing(hub, media_requests) => result,
            }
        }
        Transport::Bus(bus) => tokio::select! {
            result = dbus::forward(&bus, api.snapshots, events, api.nearby) => result,
            result = quickshare::forward(&bus, api.quick_share_watches, qs_events) => result,
            result = desktop_media::run(bus.clone(), api.hub, media_requests) => result,
        },
    }
}

/// Unassigned at IANA; the ufw profile `Andromeda Link` opens it.
const DEFAULT_PORT: u16 = 4717;

/// Binds [`DEFAULT_PORT`], the one the firewall opens, else the port of the last run, where phones dial first, else
/// any.
fn bind(identity: &Identity, last: u16) -> anyhow::Result<PathServer> {
    let addr = |port| SocketAddr::from((Ipv6Addr::UNSPECIFIED, port));
    PathServer::bind(identity, addr(DEFAULT_PORT))
        .or_else(|error| {
            log::warn!("port {DEFAULT_PORT}: {error}; choosing another");
            match last {
                0 | DEFAULT_PORT => Err(error),
                last => PathServer::bind(identity, addr(last)),
            }
        })
        .or_else(|_| PathServer::bind(identity, addr(0)))
        .context("binding the QUIC endpoint")
}

/// Never resolves; on each kernel address change it makes the mDNS responder recheck, so after resume or a network
/// change the advertisement carries the new address at once.
#[cfg(target_os = "linux")]
async fn follow_addresses(mdns: Option<mdns_sd::ServiceDaemon>) {
    use rustix::net::{AddressFamily, SocketFlags, SocketType, netlink};
    const RTMGRP_IPV4_IFADDR: u32 = 0x10;
    const RTMGRP_IPV6_IFADDR: u32 = 0x100;
    let Some(mdns) = mdns else { return std::future::pending().await };
    let watch = || -> std::io::Result<_> {
        let fd = rustix::net::socket_with(
            AddressFamily::NETLINK,
            SocketType::RAW,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )?;
        rustix::net::bind(&fd, &netlink::SocketAddrNetlink::new(0, RTMGRP_IPV4_IFADDR | RTMGRP_IPV6_IFADDR))?;
        // Sound for OwnedFd, whose descriptor never changes; `register` would need unsafe, which the workspace forbids.
        #[allow(deprecated)]
        tokio::io::unix::AsyncFd::new(fd)
    };
    let fd = match watch() {
        Ok(fd) => fd,
        Err(error) => {
            log::warn!("cannot watch interface addresses; mDNS sees changes within a minute: {error}");
            return std::future::pending().await;
        }
    };
    let mut buf = [0u8; 8192];
    loop {
        let Ok(mut ready) = fd.readable().await else {
            return std::future::pending().await;
        };
        // One recheck covers a burst of messages; their content does not matter.
        while rustix::io::read(fd.get_ref(), &mut buf).is_ok() {}
        ready.clear_ready();
        if let Err(error) = link_core::discovery::recheck_interfaces(&mdns) {
            log::warn!("mdns: {error}");
        }
    }
}

/// Resolves when a package upgrade replaced this program's file, so the service manager restarts it into the new one.
#[cfg(target_os = "linux")]
async fn replaced() {
    use rustix::fs::inotify;
    let deleted =
        || std::fs::read_link("/proc/self/exe").is_ok_and(|exe| exe.to_string_lossy().ends_with(" (deleted)"));
    // Replacing or removing the file lowers its link count, which inotify reports as ATTRIB.
    let watch = || -> std::io::Result<_> {
        let fd = inotify::init(inotify::CreateFlags::NONBLOCK | inotify::CreateFlags::CLOEXEC)?;
        inotify::add_watch(&fd, std::fs::read_link("/proc/self/exe")?, inotify::WatchFlags::ATTRIB)?;
        // Sound for OwnedFd, whose descriptor never changes; `register` would need unsafe, which the workspace forbids.
        #[allow(deprecated)]
        tokio::io::unix::AsyncFd::new(fd)
    };
    let fd = match watch() {
        Ok(fd) => fd,
        Err(error) => {
            log::warn!("cannot watch the program file for upgrades: {error}");
            return std::future::pending().await;
        }
    };
    let mut buf = [0u8; 1024];
    loop {
        if deleted() {
            return;
        }
        let Ok(mut ready) = fd.readable().await else {
            return std::future::pending().await;
        };
        while rustix::io::read(fd.get_ref(), &mut buf).is_ok() {}
        ready.clear_ready();
    }
}
