//! What phone and desktop share about Bluetooth: how a phone opens a link, and the transfer limits over one.

use std::io;
use std::os::fd::OwnedFd;

/// Files are offered over Bluetooth only up to this total; a larger offer waits for Wi-Fi or the hotspot.
pub const BLUETOOTH_FILE_LIMIT: u64 = 20 * 1024 * 1024;

/// A send past this total asks for the phone's hotspot first, since it takes longer over Bluetooth than joining it; a
/// smaller one, or one within [`BLUETOOTH_FILE_LIMIT`] whose hotspot fails, goes over Bluetooth.
pub const BLUETOOTH_UPGRADE_ABOVE: u64 = 1024 * 1024;

/// The LE L2CAP channel the desktop listens on, in the dynamic range (0x80-0xFF); fixed, since no service record
/// names it.
pub const L2CAP_PSM: u16 = 0xC5;

/// Opens a link to a desktop's Bluetooth adapter (`AA:BB:CC:DD:EE:FF`). Blocking, since the platforms connect
/// synchronously; the phone calls it off the runtime.
pub trait BluetoothOpener: Send + Sync {
    /// Opens RFCOMM as a byte-stream descriptor ([`crate::path::StreamLink`]). With `pair`, the platform first pairs
    /// Bluetooth with the desktop when it is not paired yet, which the user confirms on both screens; only Link
    /// pairing asks for that.
    fn open(&self, address: &str, pair: bool) -> io::Result<OwnedFd>;

    /// Opens an LE L2CAP channel to the desktop's [`L2CAP_PSM`] as a `SOCK_SEQPACKET` descriptor, one SDU per
    /// message. Platforms without LE channels keep the default.
    fn open_l2cap(&self, _address: &str) -> io::Result<OwnedFd> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
