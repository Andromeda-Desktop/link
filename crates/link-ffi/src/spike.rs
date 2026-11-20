//! The phone side of the channel spike (link/.agent/TASK-reliability.md step 0): one QUIC connection that moves
//! between Wi-Fi (UDP) and a Bluetooth L2CAP link, with ping round trips idle and under bulk load on each path.
//! Pairs with `cargo run -p link-core --example path_spike` on the desktop. Not used by the app.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use link_core::identity::Identity;
use link_core::path::{FdLink, PathSocket};
use link_core::tls::ServerPin;
use link_core::transport::Dialer;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const PINGS: usize = 50;
const BULK: Duration = Duration::from_secs(8);
const PING_EVERY: Duration = Duration::from_millis(50);

/// Runs the spike against `desktop` (`ip:port`), over `l2cap_fd` too when it is not negative (one end of a
/// `SOCK_SEQPACKET` pair bridged to an L2CAP channel, which this takes over). Each report line is also appended to
/// `report` as it is made, since some vendors drop app lines from logcat. Blocking; returns the whole report.
#[uniffi::export]
#[must_use]
#[expect(clippy::needless_pass_by_value, reason = "UniFFI hands strings over owned")]
pub fn run_path_spike(desktop: String, l2cap_fd: i32, report: String) -> String {
    // SAFETY: the app hands over a descriptor it detached and no longer uses, so nothing else owns or closes it.
    let fd = (l2cap_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(l2cap_fd) });
    let mut out = Report { text: String::new(), file: std::fs::File::create(&report).ok(), start: Instant::now() };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Into::into)
        .and_then(|runtime| runtime.block_on(spike(&desktop, fd, &mut out, &report)));
    if let Err(error) = result {
        let _ = writeln!(out, "failed: {error}");
    }
    out.text
}

/// The report: kept for the return value and appended to a file at once, each line stamped with seconds since start.
struct Report {
    text: String,
    file: Option<std::fs::File>,
    start: Instant,
}

impl std::fmt::Write for Report {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        let stamped = if self.text.is_empty() || self.text.ends_with('\n') {
            format!("{:7.2} {text}", self.start.elapsed().as_secs_f64())
        } else {
            text.to_owned()
        };
        self.text.push_str(&stamped);
        if let Some(file) = &mut self.file {
            let _ = std::io::Write::write_all(file, stamped.as_bytes());
        }
        Ok(())
    }
}

#[expect(clippy::too_many_lines, reason = "one linear measurement script")]
async fn spike(desktop: &str, fd: Option<OwnedFd>, out: &mut Report, report: &str) -> Result {
    // Prefixes: `plain:` dials from quinn's own UDP socket (the baseline for PathSocket); `cubic:` and `newreno:`
    // replace BBR; `win:` raises the stream and connection windows to 8 and 32 MiB; `nogso:` turns off UDP GSO.
    let mut desktop = desktop;
    let mut take = |prefix: &str| desktop.strip_prefix(prefix).map(|rest| desktop = rest).is_some();
    let (plain, cubic, newreno, win) = (take("plain:"), take("cubic:"), take("newreno:"), take("win:"));
    let nogso = take("nogso:");
    let desktop: SocketAddr = desktop.parse()?;
    let socket = PathSocket::new(std::net::UdpSocket::bind("[::]:0")?)?;
    let link = fd.map(|fd| FdLink::new(fd).map(|link| socket.add_link(link, desktop))).transpose()?;
    let link = link.filter(|_| !plain);
    let phone = if plain {
        Dialer::new(&Identity::generate()?)?
    } else {
        Dialer::on_path(&Identity::generate()?, socket.clone())?
    };
    let phone = phone.tuned(|config| {
        config.enable_segmentation_offload(!nogso);
        if cubic {
            config.congestion_controller_factory(Arc::new(quinn::congestion::CubicConfig::default()));
        }
        if newreno {
            config.congestion_controller_factory(Arc::new(quinn::congestion::NewRenoConfig::default()));
        }
        if win {
            config
                .stream_receive_window((8u32 << 20).into())
                .receive_window((32u32 << 20).into())
                .send_window(32 << 20);
        }
    });
    let cc = if cubic {
        "cubic"
    } else if newreno {
        "newreno"
    } else {
        "bbr"
    };
    writeln!(
        out,
        "socket: {}, {cc}, windows {}, gso {}",
        if plain { "quinn udp" } else { "path" },
        if win { "8/32 MiB" } else { "default" },
        !nogso
    )?;

    let start = Instant::now();
    let wifi = tokio::time::timeout(Duration::from_secs(3), phone.dial(desktop, ServerPin::Any)).await;
    let (connection, mut on_link) = match (wifi, link) {
        (Ok(Ok(dialed)), _) => (dialed.connection, false),
        (_, Some(link)) => {
            socket.route(desktop, Some(link));
            let start = Instant::now();
            let dialed = phone.dial(desktop, ServerPin::Any).await?;
            writeln!(out, "wifi dial failed; l2cap dial {:?}", start.elapsed())?;
            (dialed.connection, true)
        }
        (Ok(Err(error)), None) => return Err(error.into()),
        (Err(elapsed), None) => return Err(elapsed.into()),
    };
    writeln!(out, "connected in {:?} on {}", start.elapsed(), if on_link { "l2cap" } else { "wifi" })?;
    sample_stats(connection.clone(), format!("{report}.stats"));
    let (mut ping_send, mut ping_recv) = connection.open_bi().await?;
    ping_send.set_priority(10)?;
    let mut ping = async || -> Result<Duration> {
        let start = Instant::now();
        let mut echo = [0; 8];
        tokio::time::timeout(Duration::from_secs(5), async {
            ping_send.write_all(&[0; 8]).await?;
            ping_recv.read_exact(&mut echo).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await??;
        Ok(start.elapsed())
    };

    for round in 0..if link.is_some() { 3 } else { 1 } {
        if round > 0 {
            on_link = !on_link;
            socket.route(desktop, on_link.then_some(link).flatten());
            writeln!(out, "switch to {}: first ping {:?}", path(on_link), ping().await?)?;
        }
        let mut idle = Vec::with_capacity(PINGS);
        for _ in 0..PINGS {
            idle.push(ping().await?);
            tokio::time::sleep(PING_EVERY / 5).await;
        }
        writeln!(out, "{} idle rtt {}", path(on_link), stats(&mut idle))?;

        writeln!(out, "{} bulk starts", path(on_link))?;
        let mut bulk = connection.open_uni().await?;
        bulk.set_priority(-10)?;
        let writer = tokio::spawn(async move {
            let (chunk, start, mut sent) = (vec![0u8; 1 << 16], Instant::now(), 0u64);
            while start.elapsed() < BULK && bulk.write_all(&chunk).await.is_ok() {
                sent += 1 << 16;
            }
            let _ = bulk.finish();
            let _ = bulk.stopped().await;
            (sent, start.elapsed())
        });
        let mut loaded = Vec::new();
        while !writer.is_finished() {
            match ping().await {
                Ok(rtt) => loaded.push(rtt),
                Err(error) => {
                    let stats = connection.stats();
                    writeln!(out, "ping under load: {error} after {} pings; {}", loaded.len(), stats.path.cwnd)?;
                    writeln!(out, "path {:?}", stats.path)?;
                    writeln!(out, "frames tx {:?}", stats.frame_tx)?;
                    writeln!(out, "udp tx {:?} rx {:?}", stats.udp_tx, stats.udp_rx)?;
                    return Err(error);
                }
            }
            tokio::time::sleep(PING_EVERY).await;
        }
        let (sent, took) = writer.await?;
        let rate = f64::from(u32::try_from(sent >> 10).unwrap_or(u32::MAX)) * 8192.0 / took.as_secs_f64() / 1e6;
        writeln!(out, "{} loaded rtt {}", path(on_link), stats(&mut loaded))?;
        writeln!(out, "{} bulk {} KiB acked in {took:.2?} = {rate:.2} Mbit/s", path(on_link), sent >> 10)?;
    }
    connection.close(0u32.into(), b"done");
    phone.finish().await;
    Ok(())
}

/// Appends the connection stats to `file` every second from an OS thread, so a blocked runtime still reports.
fn sample_stats(connection: quinn::Connection, file: String) {
    std::thread::spawn(move || {
        let Ok(mut file) = std::fs::File::create(file) else { return };
        let start = Instant::now();
        while connection.close_reason().is_none() {
            let s = connection.stats();
            let line = format!(
                "{:7.2} rtt {:?} cwnd {} lost {} sent {} congestion {} | tx dgrams {} bytes {} ios {} | rx dgrams {} bytes {} | tx stream {} ack {} | rx ack {} max_data {} max_stream_data {} blocked {}/{}\n",
                start.elapsed().as_secs_f64(),
                s.path.rtt,
                s.path.cwnd,
                s.path.lost_packets,
                s.path.sent_packets,
                s.path.congestion_events,
                s.udp_tx.datagrams,
                s.udp_tx.bytes,
                s.udp_tx.ios,
                s.udp_rx.datagrams,
                s.udp_rx.bytes,
                s.frame_tx.stream,
                s.frame_tx.acks,
                s.frame_rx.acks,
                s.frame_rx.max_data,
                s.frame_rx.max_stream_data,
                s.frame_tx.data_blocked,
                s.frame_tx.stream_data_blocked,
            );
            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

fn path(on_link: bool) -> &'static str {
    if on_link { "l2cap" } else { "wifi" }
}

fn stats(samples: &mut [Duration]) -> String {
    samples.sort_unstable();
    let at = |q: usize| samples.get((samples.len().saturating_sub(1)) * q / 100).copied().unwrap_or_default();
    format!("p50 {:?} p95 {:?} p99 {:?} max {:?} (n {})", at(50), at(95), at(99), at(100), samples.len())
}
