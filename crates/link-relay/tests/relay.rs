//! A phone reaches a desktop through the relay over Link's own transport, and an unknown device is refused.

use std::net::SocketAddr;
use std::time::Duration;

use link_core::control::Control;
use link_core::identity::Identity;
use link_core::path::{Hop, PathSocket, link_address};
use link_core::phone::Phone;
use link_core::proto::message::Hello;
use link_core::proto::{CloseCode, VERSION};
use link_core::reach::Via;
use link_core::relay::{MTU, Relay, tag};
use link_core::store::{Peer, Store};
use link_core::tls::ServerPin;
use link_core::transport::{Dialer, PathServer};
use link_core::wire::Connection;

type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn pin(identity: &Identity) -> ServerPin {
    ServerPin::Key(identity.spki().fingerprint())
}

fn relay(
    allowed: Vec<String>,
) -> std::result::Result<(SocketAddr, Identity), Box<dyn std::error::Error + Send + Sync>> {
    let identity = Identity::generate()?;
    let endpoint = link_relay::endpoint(&identity, "127.0.0.1:0".parse()?)?;
    let addr = endpoint.local_addr()?;
    tokio::spawn(link_relay::serve(endpoint, Duration::from_secs(30), move |device| {
        allowed.iter().any(|id| id == device.as_str())
    }));
    Ok((addr, identity))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_removed_from_the_allow_list_is_closed_while_connected() -> Result {
    let desk_key = Identity::generate()?;
    let allow = std::sync::Arc::new(std::sync::Mutex::new(vec![desk_key.spki().device_id().as_str().to_owned()]));
    let relay_key = Identity::generate()?;
    let endpoint = link_relay::endpoint(&relay_key, "127.0.0.1:0".parse()?)?;
    let relay_addr = endpoint.local_addr()?;
    let list = allow.clone();
    tokio::spawn(link_relay::serve(endpoint, Duration::from_millis(100), move |device| {
        list.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|id| id == device.as_str())
    }));

    let desk = PathServer::bind(&desk_key, "[::]:0".parse()?)?;
    let connection = desk
        .dial(relay_addr, pin(&relay_key), |transport| {
            transport.initial_mtu(MTU);
            transport.keep_alive_interval(Some(Duration::from_millis(50)));
        })
        .await?;
    let (mut desk_relay, _opened) = Relay::new(connection.clone());
    desk_relay.wait_observed().await.ok_or("the relay did not admit the desktop")?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(connection.close_reason().is_none(), "an allowed device was closed");

    allow.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    let closed = tokio::time::timeout(Duration::from_secs(5), connection.closed()).await;
    assert!(
        matches!(&closed, Ok(quinn::ConnectionError::ApplicationClosed(close)) if close.error_code == 1u32.into()),
        "removed device: {closed:?}"
    );
    Ok(())
}

fn phone_dialer(
    identity: &Identity,
) -> std::result::Result<(Dialer, std::sync::Arc<PathSocket>), Box<dyn std::error::Error + Send + Sync>> {
    let socket = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
    Ok((Dialer::on_path(identity, socket.clone())?, socket))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_reaches_its_desktop_through_the_relay() -> Result {
    let (desk_key, phone_key) = (Identity::generate()?, Identity::generate()?);
    let ids = [&desk_key, &phone_key].map(|key| key.spki().device_id().as_str().to_owned());
    let (relay_addr, relay_key) = relay(ids.to_vec())?;

    let desk = PathServer::bind(&desk_key, "[::]:0".parse()?)?;
    let desk_relay = desk
        .dial(relay_addr, pin(&relay_key), |transport| {
            transport.initial_mtu(MTU);
        })
        .await?;
    let (mut desk_relay, mut opened) = Relay::new(desk_relay);
    let seen = desk_relay.wait_observed().await.ok_or("the relay did not say the desktop's address")?;
    assert_eq!(seen.port(), desk.endpoint.local_addr()?.port(), "the relay saw another socket than the desktop's");

    let (dialer, socket) = phone_dialer(&phone_key)?;
    let relayed = dialer
        .tuned(|transport| {
            transport.initial_mtu(MTU);
        })
        .dial(relay_addr, pin(&relay_key))
        .await?;
    let (phone_relay, _) = Relay::new(relayed.connection);
    let link = socket.add_link(phone_relay.link(tag(desk_key.spki())), link_address(1));

    let echo = tokio::spawn(async move {
        let link = opened.recv().await.ok_or("no link opened")?;
        desk.socket.accept_link(link);
        let incoming = desk.endpoint.accept().await.ok_or("no connection")?;
        let (connecting, _route) = desk.accept(incoming)?;
        let connection = connecting.await?;
        let (mut send, mut recv) = connection.accept_bi().await?;
        let got = recv.read_to_end(1 << 20).await?;
        send.write_all(&got).await?;
        send.finish()?;
        connection.closed().await;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let inner = dialer.dial_hop(Hop::Link(link), pin(&desk_key)).await?;
    let (mut send, mut recv) = inner.connection.open_bi().await?;
    let payload = vec![7; 200_000];
    send.write_all(&payload).await?;
    send.finish()?;
    let back = tokio::time::timeout(Duration::from_secs(10), recv.read_to_end(1 << 20)).await??;
    assert_eq!(back, payload, "the stream did not cross the relay whole");
    inner.connection.close(0u8.into(), b"done");
    tokio::time::timeout(Duration::from_secs(5), echo).await???;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_phone_without_a_direct_path_opens_its_session_through_the_relay() -> Result {
    let (desk_key, phone_key) = (Identity::generate()?, Identity::generate()?);
    let ids = [&desk_key, &phone_key].map(|key| key.spki().device_id().as_str().to_owned());
    let (relay_addr, relay_key) = relay(ids.to_vec())?;
    let fingerprint =
        relay_key.spki().fingerprint().iter().fold(String::new(), |hex, byte| hex + &format!("{byte:02x}"));
    let spec = format!("{relay_addr}#{fingerprint}");

    let desk = PathServer::bind(&desk_key, "[::]:0".parse()?)?;
    let desk_relay = desk
        .dial(relay_addr, pin(&relay_key), |transport| {
            transport.initial_mtu(MTU);
        })
        .await?;
    let (_desk_relay, mut opened) = Relay::new(desk_relay);
    let desktop = tokio::spawn(async move {
        let link = opened.recv().await.ok_or("no link opened")?;
        desk.socket.accept_link(link);
        let incoming = desk.endpoint.accept().await.ok_or("no connection")?;
        let (connecting, _route) = desk.accept(incoming)?;
        let connection = Connection::from(connecting.await?);
        let mut control = Control::accept(&connection, None).await?;
        let hello = Hello { version: VERSION, name: "desk".to_owned(), addresses: Vec::new() };
        control.hello_as_server(hello).await?;
        connection.closed().await;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });

    let store_path = std::env::temp_dir().join(format!("link-relay-phone-{}.json", std::process::id()));
    let mut store = Store::default();
    let mut peer = Peer::new(desk_key.spki(), "desk".to_owned());
    peer.relay = Some(spec);
    store.upsert(peer);
    store.save(&store_path)?;
    let mut phone = Phone::new(phone_key, store_path.clone(), "phone".to_owned(), None)?;
    let session = tokio::time::timeout(Duration::from_secs(15), phone.connect(&desk_key.spki().device_id()))
        .await
        .map_err(|_| "the session did not open")?;
    drop(std::fs::remove_file(&store_path));
    let session = session?;
    assert_eq!(session.via, Via::Relay);
    session.connection.close(CloseCode::Done);
    tokio::time::timeout(Duration::from_secs(5), desktop)
        .await
        .map_err(|_| "the desktop did not see the close")???;
    Ok(())
}

#[tokio::test]
async fn an_unknown_device_is_refused() -> Result {
    let (relay_addr, relay_key) = relay(Vec::new())?;
    let (dialer, _socket) = phone_dialer(&Identity::generate()?)?;
    let relayed = dialer
        .tuned(|transport| {
            transport.initial_mtu(MTU);
        })
        .dial(relay_addr, pin(&relay_key))
        .await?;
    let reason = tokio::time::timeout(Duration::from_secs(5), relayed.connection.closed()).await?;
    assert!(matches!(reason, quinn::ConnectionError::ApplicationClosed(_)), "{reason}");
    Ok(())
}
