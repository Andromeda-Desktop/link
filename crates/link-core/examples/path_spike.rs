//! Desktop side of the channel spike: one QUIC endpoint on UDP and Bluetooth LE L2CAP. Echoes every bidirectional
//! stream, drains every unidirectional one and prints its rate, and logs each path the connection moves to.
//! Run: `cargo run -p link-core --example path_spike -- [psm] [udp-port]`, then advertise with `bluetoothctl`.

use std::net::SocketAddr;
use std::time::Instant;

use bluer::l2cap::{SeqPacket, Socket, SocketAddr as L2capAddr};
use bluer::{Address, AddressType};
use link_core::identity::Identity;
use link_core::path::{MessageLink, PathSocket};
use link_core::transport::path_server_endpoint;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Above any QUIC packet, so a path move never needs a smaller one.
const L2CAP_MTU: u16 = 2048;

struct L2cap(SeqPacket);

impl MessageLink for L2cap {
    async fn send(&self, packet: &[u8]) -> std::io::Result<()> {
        self.0.send(packet).await.map(drop)
    }

    async fn recv(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv(buf).await
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result {
    let mut args = std::env::args().skip(1);
    let psm = args.next().map_or(Ok(0xC5), |psm| u16::from_str_radix(psm.trim_start_matches("0x"), 16))?;
    let port: u16 = args.next().map_or(Ok(4718), |port| port.parse())?;

    let plain = args.next().is_some_and(|mode| mode == "plain");
    let identity = Identity::generate()?;
    let socket =
        PathSocket::new(std::net::UdpSocket::bind(SocketAddr::from(([0; 16], if plain { 0 } else { port })))?)?;
    let endpoint = if plain {
        link_core::transport::server_endpoint(&identity, SocketAddr::from(([0; 16], port)))?
    } else {
        path_server_endpoint(&identity, socket.clone())?
    };
    println!("socket: {}", if plain { "quinn udp" } else { "path" });
    println!("quic on udp {port}, l2cap le psm {psm:#x}; any client key is accepted");

    let l2cap = Socket::<SeqPacket>::new_seq_packet()?;
    l2cap.bind(L2capAddr::new(Address::any(), AddressType::LePublic, psm))?;
    l2cap.set_recv_mtu(L2CAP_MTU)?;
    let listener = l2cap.listen(4)?;
    let links = socket.clone();
    tokio::spawn(async move {
        let mut next = 1u8;
        while let Ok((link, from)) = listener.accept().await {
            let peer = SocketAddr::from(([10, 255, 255, next], 1));
            next = next.wrapping_add(1);
            println!(
                "l2cap from {} as {peer}: send mtu {:?}, recv mtu {:?}",
                from.addr,
                link.send_mtu(),
                link.recv_mtu()
            );
            let id = links.add_link(L2cap(link), peer);
            links.route(peer, Some(id));
        }
    });

    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            if let Err(error) = serve(incoming).await {
                println!("connection: {error}");
            }
        });
    }
    Ok(())
}

async fn serve(incoming: quinn::Incoming) -> Result {
    let connection = incoming.await?;
    println!("connected from {}", connection.remote_address());
    let sample = connection.clone();
    std::thread::spawn(move || {
        let start = Instant::now();
        while sample.close_reason().is_none() {
            let s = sample.stats();
            println!(
                "{:6.2} rtt {:?} cwnd {} lost {} | tx dgrams {} ios {} | rx dgrams {} bytes {} | tx ack {} max_stream_data {} | rx stream {}",
                start.elapsed().as_secs_f64(),
                s.path.rtt,
                s.path.cwnd,
                s.path.lost_packets,
                s.udp_tx.datagrams,
                s.udp_tx.ios,
                s.udp_rx.datagrams,
                s.udp_rx.bytes,
                s.frame_tx.acks,
                s.frame_tx.max_stream_data,
                s.frame_rx.stream,
            );
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
    let watch = connection.clone();
    tokio::spawn(async move {
        let mut path = watch.remote_address();
        while watch.close_reason().is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if watch.remote_address() != path {
                path = watch.remote_address();
                println!("moved to {path}, rtt {:?}", watch.rtt());
            }
        }
    });
    loop {
        tokio::select! {
            bi = connection.accept_bi() => {
                let (mut send, mut recv) = bi?;
                tokio::spawn(async move { tokio::io::copy(&mut recv, &mut send).await });
            }
            uni = connection.accept_uni() => {
                let mut recv = uni?;
                tokio::spawn(async move {
                    let (start, mut bytes) = (Instant::now(), 0u64);
                    while let Ok(Some(chunk)) = recv.read_chunk(1 << 16, false).await {
                        bytes += chunk.bytes.len() as u64;
                    }
                    let secs = start.elapsed().as_secs_f64();
                    println!("bulk: {bytes} bytes in {secs:.2} s = {:.2} Mbit/s", f64::from(u32::try_from(bytes >> 10).unwrap_or(u32::MAX)) * 8192.0 / secs / 1e6);
                });
            }
        }
    }
}
