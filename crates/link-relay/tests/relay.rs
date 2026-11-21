//! A phone reaches a desktop through the relay over Link's own transport, and an unknown device is refused.

use std::net::SocketAddr;
use std::time::Duration;

use link_core::identity::Identity;
use link_core::path::{Hop, PathSocket, link_address};
use link_core::relay::{MTU, Relay, tag};
use link_core::tls::ServerPin;
use link_core::transport::{Dialer, PathServer};

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
    tokio::spawn(link_relay::serve(endpoint, move |device| allowed.iter().any(|id| id == device.as_str())));
    Ok((addr, identity))
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
