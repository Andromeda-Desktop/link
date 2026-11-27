//! No hotspot join on this desktop yet; a phone offers one only over Bluetooth, which has no backend here either.

use std::net::IpAddr;

pub enum Active {}

pub struct Joined {
    pub active: Active,
    pub address: IpAddr,
}

#[expect(clippy::unused_async, reason = "the Linux module's signature")]
pub async fn join(_ssid: &str, _passphrase: &str) -> Result<Joined, String> {
    Err("no hotspot join on this desktop".to_owned())
}

#[expect(clippy::unused_async, reason = "the Linux module's signature")]
pub async fn leave(active: &Active) {
    match *active {}
}
