//! Desktop side of the channel spike: one QUIC endpoint on UDP and Bluetooth LE L2CAP. Echoes every bidirectional
//! stream, drains every unidirectional one and prints its rate, and logs each path the connection moves to.
//! Run: `cargo run -p link-core --example path_spike -- [psm] [udp-port]`, then advertise with `bluetoothctl`.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() -> linux::Result {
    linux::main()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("path_spike needs BlueZ");
}
