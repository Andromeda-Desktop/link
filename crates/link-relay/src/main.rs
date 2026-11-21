//! andromeda-link-relay [--listen ADDR] [--state DIR]: relays Link connections between networks. DIR holds the relay
//! key (`relay.key`, made on first start) and `allow`: one device id per line, `#` starts a comment; read per connection.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use link_core::identity::{DeviceId, Identity};

const LISTEN: &str = "[::]:4433";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut listen: SocketAddr = LISTEN.parse()?;
    let mut state = PathBuf::from(std::env::var_os("STATE_DIRECTORY").unwrap_or_else(|| ".".into()));
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().context("--listen ADDR")?.parse()?,
            "--state" => state = args.next().context("--state DIR")?.into(),
            _ => anyhow::bail!("usage: andromeda-link-relay [--listen ADDR] [--state DIR]"),
        }
    }
    let identity = Identity::load_or_create(&state.join("relay.key")).map_err(|e| anyhow::anyhow!("relay key: {e}"))?;
    let endpoint = link_relay::endpoint(&identity, listen)?;
    let fingerprint = identity.spki().fingerprint().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    log::info!("relay on {listen}, key {fingerprint}");
    let allow = state.join("allow");
    link_relay::serve(endpoint, move |device| allowed(&allow, device)).await;
    Ok(())
}

fn allowed(file: &std::path::Path, device: &DeviceId) -> bool {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines().map(|line| line.split('#').next().unwrap_or_default().trim()).any(|line| line == device.as_str())
}
