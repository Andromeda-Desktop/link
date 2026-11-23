use std::fmt::{self, Write as _};

use ciborium::Value;
use serde::{Deserialize, Serialize};

use crate::pairing::{MAC_LEN, SPAKE_MSG_LEN};

mod clip;
mod fs;
mod media;
mod mirror;
mod notify;
mod status;
mod system;
mod telephony;
mod transfer;

pub use clip::*;
pub use fs::*;
pub use media::*;
pub use mirror::*;
pub use notify::*;
pub use status::*;
pub use system::*;
pub use telephony::*;
pub use transfer::*;

pub const MAX_NAME_LEN: usize = 64;
/// Leaves room for the envelope, so a share always fits one control frame.
pub const MAX_SHARE_LEN: usize = 60 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<String>,
}

/// A Bluetooth address among a hello's `addresses`: `bt:` then six colon-separated hex octets. Phones from before
/// Bluetooth skip it, since it is not a socket address.
pub const BLUETOOTH_SCHEME: &str = "bt:";

/// The desktop's relay among a hello's `addresses`: `relay:<host:port>#<64 hex key fingerprint>`. The phone dials the
/// desktop through it when no IP or Bluetooth path answers; older phones skip it, since it is not a socket address.
pub const RELAY_SCHEME: &str = "relay:";

/// The desktop's public address as its relay sees it: `public:<ip:port>`. A phone dials it only after the desktop
/// punched toward the phone, since an earlier dial leaves a NAT entry that moves the desktop's punch to another port;
/// hence not a plain address, which every phone dials at once.
pub const PUBLIC_SCHEME: &str = "public:";

/// A desktop's hello carries this among its `addresses` when it joins a phone's Wi-Fi Direct group by the credentials
/// in `wifi-direct`; phones send that message only then, since older desktops expect a different body.
pub const WIFI_DIRECT_JOIN: &str = "feature:wifi-direct-join";
/// A desktop's hello carries this when it takes a group's `mhz` in `wifi-direct`; older desktops refuse the field.
pub const GROUP_CHANNEL: &str = "feature:group-channel";

/// A desktop's hello carries this among its `addresses` when it takes `bt-pairing`; older desktops close a session on
/// the unknown type, and phones skip entries that are not socket addresses.
pub const BT_PAIRING: &str = "feature:bt-pairing";

/// The prefix of a hello's `addresses` entries that name a feature rather than an address.
pub const FEATURE: &str = "feature:";

/// A desktop's hello carries this when its `status` takes `signal`; older desktops close a session on the field.
pub const STATUS_SIGNAL: &str = "feature:status-signal";

/// A phone's hello carries this when it answers `fs-stat` and `fs-thumb`; older phones close a session on the types.
pub const FS_STAT: &str = "feature:fs-stat";

/// A phone's hello carries this when it answers `fs-write` and `fs-delete`; older phones close a session on the types.
pub const FS_WRITE: &str = "feature:fs-write";

/// A phone's hello carries this when it answers `photo-request`; older phones close a session on the type.
pub const CAMERA: &str = "feature:camera";

/// A phone's hello carries this when it answers `photos-recent`; older phones close a session on the type.
pub const PHOTOS_RECENT: &str = "feature:photos-recent";

/// A phone's hello carries this when its `photos-recent` takes `page` (then its `photos-list` gives `total` and each
/// video's `video`); older phones close a session on the field.
pub const PHOTOS_PAGE: &str = "feature:photos-page";

/// A hello carries this when its side takes the contacts and text message types; older peers close a session on them.
pub const MESSAGES: &str = "feature:messages";

/// A hello carries this when its side takes a text's `state` and `parts` and the `text-part` type; older peers close a
/// session on them.
pub const TEXTS_MMS: &str = "feature:texts-mms";

/// A hello carries this when its side takes the call log and dial types; older peers close a session on them.
pub const CALLS: &str = "feature:calls";

/// A desktop's hello carries this when its `call` takes `app`; older desktops close a session on the field.
pub const CALL_APP: &str = "feature:call-app";

/// A desktop's hello carries this when it takes `notification-snapshot`, `alert_once`, and `status`'s
/// `notifications_off`, and answers with `notification-pull`; older desktops close a session on them.
pub const NOTIFICATION_SYNC: &str = "feature:notification-sync";

/// A desktop's hello carries this when its `notification-posted` takes `replies`; older desktops close a session on it.
pub const NOTIFICATION_REPLIES: &str = "feature:notification-replies";

/// Both hellos carry this when the peer takes `clip-offer`'s `sizes`, `origin`, and `at`, and serves each type's own
/// bytes; older peers close a session on those fields.
pub const CLIP_ITEMS: &str = "feature:clip-items";

/// Both hellos carry this when the peer stamps `clip-offer`'s `at` past every `at` it has made or seen, so two such
/// peers keep the offer with the larger (`at`, `origin`) whatever their clocks say.
pub const CLIP_CLOCK: &str = "feature:clip-clock";

/// Both hellos carry this when the peer takes `share`'s `item` and applies each item once; older peers close a
/// session on the field.
pub const SHARE_OUTBOX: &str = "feature:share-outbox";

/// A desktop's hello carries this when it takes a mirror's video as one stream per keyframe (`mirror-data`'s `gop`);
/// phones send older desktops video and sound on one stream.
pub const MIRROR_STREAMS: &str = "feature:mirror-streams";

/// A phone's hello carries this when it takes `mirror-request`'s `codecs`; older phones close a session on the field.
pub const MIRROR_CODECS: &str = "feature:mirror-codecs";

/// The most codecs a `mirror-request` lists.
pub const MAX_MIRROR_CODECS: usize = 4;

/// A desktop's hello carries this when it takes `screen-shape`; phones send older desktops no shape.
pub const SCREEN_SHAPE: &str = "feature:screen-shape";

/// A desktop's hello carries this when its `status` takes `storage`; older desktops close a session on the field.
pub const STATUS_STORAGE: &str = "feature:status-storage";

/// The address in a `bt:` entry, upper-case, or None when `entry` is not one.
pub fn bluetooth_address(entry: &str) -> Option<String> {
    let address = entry.strip_prefix(BLUETOOTH_SCHEME)?;
    let octets: Vec<&str> = address.split(':').collect();
    let valid = octets.len() == 6
        && octets.iter().all(|octet| octet.len() == 2 && octet.bytes().all(|b| b.is_ascii_hexdigit()));
    valid.then(|| address.to_ascii_uppercase())
}

/// The address in a `public:` entry, or None when `entry` is not one.
#[must_use]
pub fn public_address(entry: &str) -> Option<std::net::SocketAddr> {
    entry.strip_prefix(PUBLIC_SCHEME)?.parse().ok()
}

/// The relay spec in a `relay:` entry, or None when `entry` is not one.
#[must_use]
pub fn relay_entry(entry: &str) -> Option<&str> {
    entry.strip_prefix(RELAY_SCHEME)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairMethod {
    Code,
    Qr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairSpake {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<PairMethod>,
    #[serde(with = "serde_bytes")]
    pub msg: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairConfirm {
    #[serde(with = "serde_bytes")]
    pub mac: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    Text,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    pub kind: ShareKind,
    pub text: String,
    /// [`SHARE_OUTBOX`]: the sender's random id for this share, the same on every resend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShareAck {
    /// The envelope id of the share this acknowledges.
    pub of: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShareRejected {
    #[error("the text is empty")]
    Empty,
    #[error("the text is longer than {MAX_SHARE_LEN} bytes")]
    TooLong,
    #[error("a link must be an http or https url")]
    NotWebLink,
}

impl ShareKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Link => "link",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "text" => Some(Self::Text),
            "link" => Some(Self::Link),
            _ => None,
        }
    }
}

impl Share {
    /// The rules of a share, checked by the sender before sending and by the receiver while decoding.
    pub fn check(&self) -> Result<(), ShareRejected> {
        if self.text.is_empty() {
            return Err(ShareRejected::Empty);
        }
        if self.text.len() > MAX_SHARE_LEN {
            return Err(ShareRejected::TooLong);
        }
        if self.kind == ShareKind::Link && !is_web_link(&self.text) {
            return Err(ShareRejected::NotWebLink);
        }
        Ok(())
    }
}

/// An absolute http or https URL: scheme in any case, something after it, and no whitespace or control characters.
fn is_web_link(text: &str) -> bool {
    let rest = ["https://", "http://"].iter().find_map(|scheme| {
        let head = text.get(..scheme.len())?;
        head.eq_ignore_ascii_case(scheme).then(|| &text[scheme.len()..])
    });
    rest.is_some_and(|rest| !rest.is_empty()) && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Byte length within `min..=max`, as CDDL's `.size` counts it.
/// `aa:bb:cc:dd:ee:ff`, lower-case, the form Android prints.
fn hardware_address(address: &str) -> bool {
    address.len() == 17
        && address
            .split(':')
            .all(|octet| octet.len() == 2 && octet.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

fn sized(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.len())
}

fn notification_id(id: &str) -> bool {
    sized(id, 1, MAX_NOTIFICATION_ID_LEN)
}

/// A snapshot's or a pull's ids: at most [`MAX_SNAPSHOT`], each valid.
fn notification_ids<'a>(ids: impl ExactSizeIterator<Item = &'a String>) -> bool {
    ids.len() <= MAX_SNAPSHOT && ids.into_iter().all(|id| notification_id(id))
}

/// Asks the receiver to start or stop ringing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ring {
    pub on: bool,
}

/// The sender's own ringing started or stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ringing {
    pub on: bool,
}

/// The phone's own hotspot, offered over Bluetooth so the desktop can join it for full speed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hotspot {
    pub ssid: String,
    pub passphrase: String,
    /// The group's frequency, so the desktop probes that one channel instead of scanning all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mhz: Option<u32>,
}

/// Where the desktop listens on the phone's hotspot, once it joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotspotJoined {
    pub address: String,
}

/// Either side is done with the hotspot, or could not start or join it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotspotEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Desktop to phone, from older desktops only: it could join a Wi-Fi Direct group the phone negotiated with it. Phones
/// ignore it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WifiDirectReady {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Phone to desktop, around Android pairing Bluetooth with it. Without `passkey`, first: the desktop starts accepting
/// connections from unpaired devices, which it otherwise refuses before pairing can begin. With it, the six-digit
/// code Android shows, which the desktop's pairing agent confirms against its own instead of asking, since only a
/// phone with a Link session can send it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BtPairing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passkey: Option<u32>,
}

/// A Wi-Fi Direct device name, as the P2P device info attribute carries it.
pub const MAX_P2P_NAME_LEN: usize = 32;

/// The phone's own addresses it is about to dial the desktop from, so the desktop can open its firewall to them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Punch {
    pub addresses: Vec<String>,
}

pub const MAX_PUNCH_ADDRESSES: usize = 8;

pub const MAX_SSID_LEN: usize = 32;
pub const MAX_REASON_LEN: usize = 128;

impl Hotspot {
    /// WPA2-PSK rules: an SSID of 1 to 32 bytes, a passphrase of 8 to 63 printable ASCII characters.
    fn valid(&self) -> bool {
        (1..=MAX_SSID_LEN).contains(&self.ssid.len())
            && (8..=63).contains(&self.passphrase.len())
            && self.passphrase.bytes().all(|byte| (0x20..0x7f).contains(&byte))
            && self.mhz.is_none_or(|mhz| (2400..=7125).contains(&mhz))
    }
}

/// Both hellos carry this among their `addresses` when the peer takes `remote-input`, `commands`, `run-command`, and
/// `ping`; older peers close a session on those types.
pub const CONTROL: &str = "feature:control";

/// Both hellos carry this when the peer takes `heartbeat` and `heartbeat-ack`; older peers close a session on those
/// types.
pub const HEARTBEAT: &str = "feature:heartbeat";

/// Both hellos carry this when the peer takes offers with an empty digest and `file-hash`; older peers reject both.
pub const LATE_HASH: &str = "feature:late-hash";

/// A desktop's hello carries this when it takes `presenting`; older desktops close a session on the type.
pub const PRESENTING: &str = "feature:presenting";

/// Both hellos carry this among their `addresses` when the peer takes `capabilities`; older peers close a session on
/// the type.
pub const CAPABILITIES: &str = "feature:capabilities";

/// Desktop to phone, on connect and after each change: what the desktop's session can do now, so the phone hides
/// the rest. Known entries: [`CAN_CLIPBOARD`], [`CAN_INPUT`], [`CAN_NOTIFICATIONS`], [`CAN_VOLUME`]; a phone skips
/// others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub can: Vec<String>,
}

pub const CAN_CLIPBOARD: &str = "clipboard";
pub const CAN_INPUT: &str = "input";
pub const CAN_NOTIFICATIONS: &str = "notifications";
pub const CAN_VOLUME: &str = "volume";
/// The session can capture the screen, for remote control.
pub const CAN_SCREEN: &str = "screen";
/// Per phone: the desktop granted it "Control system", so the phone obeys the desktop's commands.
pub const CAN_CONTROL: &str = "control";
pub const MAX_CAPABILITIES: usize = 16;
pub const MAX_CAPABILITY_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub name: String,
}

/// Desktop to phone: every command the desktop lets the phone run, sent on connect and after each change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Commands {
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCommand {
    pub id: String,
}

/// Either way, only to a peer whose hello carries [`HEARTBEAT`]: proof the session still moves both ways. The receiver
/// answers with [`HeartbeatAck`] at once and may treat the sender as gone once `next_ms` plus a grace have passed
/// without another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub seq: u64,
    /// When the sender's next heartbeat is due, in milliseconds.
    pub next_ms: u32,
}

/// Either way: the answer to the [`Heartbeat`] with this `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatAck {
    pub seq: u64,
}

/// Either way: a ping the user sent, shown as a notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ping {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Phone to desktop: the phone's presenter opened (`on`) or closed; the desktop stays awake while it is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presenting {
    pub on: bool,
}

pub const MAX_COMMANDS: usize = 64;
pub const MAX_COMMAND_ID_LEN: usize = 64;
pub const MAX_PING_LEN: usize = 256;

impl Commands {
    fn valid(&self) -> bool {
        self.commands.len() <= MAX_COMMANDS
            && self
                .commands
                .iter()
                .all(|command| sized(&command.id, 1, MAX_COMMAND_ID_LEN) && sized(&command.name, 1, MAX_TITLE_LEN))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    PairSpake(PairSpake),
    PairConfirm(PairConfirm),
    Unpair,
    Share(Share),
    ShareAck(ShareAck),
    NotificationPosted(NotificationPosted),
    NotificationRemoved(NotificationRemoved),
    NotificationSnapshot(NotificationSnapshot),
    NotificationPull(NotificationPull),
    NotificationAction(NotificationAction),
    NotificationDismiss(NotificationDismiss),
    MediaPlayer(MediaPlayer),
    MediaGone(MediaGone),
    MediaCommand(MediaCommand),
    Ring(Ring),
    Ringing(Ringing),
    Call(Call),
    CallAction(CallAction),
    Offer(Offer),
    OfferReply(OfferReply),
    Resume(TransferRef),
    ResumeAt(ResumeAt),
    Cancel(TransferRef),
    FileDone(FileDone),
    FileHash(FileHash),
    FileData(FileData),
    ClipOffer(ClipOffer),
    ClipPull(ClipPull),
    ClipData(ClipPull),
    Status(Status),
    HotspotRequest,
    Hotspot(Hotspot),
    HotspotJoined(HotspotJoined),
    HotspotEnd(HotspotEnd),
    Punch(Punch),
    WifiDirectReady(WifiDirectReady),
    /// Phone to desktop, instead of the hotspot: the credentials of a Wi-Fi Direct group the phone owns. The desktop
    /// answers like a hotspot: `hotspot-joined` or `hotspot-end`.
    WifiDirect(Hotspot),
    BtPairing(BtPairing),
    MirrorRequest(MirrorRequest),
    MirrorStarted(MirrorStarted),
    ScreenShape(ScreenShape),
    MirrorStop(MirrorStop),
    MirrorInput(MirrorInput),
    MirrorKeyframe,
    MirrorData(MirrorData),
    RemoteRequest(RemoteRequest),
    RemoteStarted(RemoteStarted),
    RemoteStop(MirrorStop),
    FsList(FsList),
    FsEntries(FsEntries),
    FsRead(FsRead),
    FsData(FsData),
    FsError(FsError),
    FsStat(FsStat),
    FsThumb(FsThumb),
    TextPartThumb(TextPartThumb),
    FsWrite(FsWrite),
    FsDelete(FsDelete),
    PhotoRequest(PhotoRequest),
    PhotoTaken(PhotoTaken),
    PhotosRecent(PhotosRecent),
    PhotosList(PhotosList),
    ContactsList(ContactsList),
    Contacts(Contacts),
    /// The phone's contacts changed; a desktop that keeps them asks again.
    ContactsChanged,
    TextsList(TextsList),
    Texts(Texts),
    /// The phone's texts changed; a desktop that keeps them asks again.
    TextsChanged,
    TextSend(TextSend),
    TextSent(TextSent),
    CallsList(CallsList),
    Calls(Calls),
    /// The phone's call log changed; a desktop that keeps it asks again.
    CallsChanged,
    Dial(Dial),
    Dialed(Dialed),
    RemoteInput(RemoteInput),
    Commands(Commands),
    RunCommand(RunCommand),
    Ping(Ping),
    Heartbeat(Heartbeat),
    HeartbeatAck(HeartbeatAck),
    Presenting(Presenting),
    Capabilities(Capabilities),
    DesktopStatus(DesktopStatus),
    SystemCommand(SystemCommand),
    SystemResult(SystemResult),
}

impl Message {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hello(_) => "hello",
            Self::PairSpake(_) => "pair-spake",
            Self::PairConfirm(_) => "pair-confirm",
            Self::Unpair => "unpair",
            Self::Share(_) => "share",
            Self::ShareAck(_) => "share-ack",
            Self::NotificationPosted(_) => "notification-posted",
            Self::NotificationRemoved(_) => "notification-removed",
            Self::NotificationSnapshot(_) => "notification-snapshot",
            Self::NotificationPull(_) => "notification-pull",
            Self::NotificationAction(_) => "notification-action",
            Self::NotificationDismiss(_) => "notification-dismiss",
            Self::MediaPlayer(_) => "media-player",
            Self::MediaGone(_) => "media-gone",
            Self::MediaCommand(_) => "media-command",
            Self::Ring(_) => "ring",
            Self::Ringing(_) => "ringing",
            Self::Call(_) => "call",
            Self::CallAction(_) => "call-action",
            Self::Offer(_) => "offer",
            Self::OfferReply(_) => "offer-reply",
            Self::Resume(_) => "resume",
            Self::ResumeAt(_) => "resume-at",
            Self::Cancel(_) => "cancel",
            Self::FileDone(_) => "file-done",
            Self::FileHash(_) => "file-hash",
            Self::FileData(_) => "file-data",
            Self::ClipOffer(_) => "clip-offer",
            Self::ClipPull(_) => "clip-pull",
            Self::ClipData(_) => "clip-data",
            Self::Status(_) => "status",
            Self::HotspotRequest => "hotspot-request",
            Self::Hotspot(_) => "hotspot",
            Self::HotspotJoined(_) => "hotspot-joined",
            Self::HotspotEnd(_) => "hotspot-end",
            Self::Punch(_) => "punch",
            Self::WifiDirectReady(_) => "wifi-direct-ready",
            Self::WifiDirect(_) => "wifi-direct",
            Self::BtPairing(_) => "bt-pairing",
            Self::MirrorRequest(_) => "mirror-request",
            Self::MirrorStarted(_) => "mirror-started",
            Self::ScreenShape(_) => "screen-shape",
            Self::MirrorStop(_) => "mirror-stop",
            Self::MirrorInput(_) => "mirror-input",
            Self::MirrorKeyframe => "mirror-keyframe",
            Self::MirrorData(_) => "mirror-data",
            Self::RemoteRequest(_) => "remote-request",
            Self::RemoteStarted(_) => "remote-started",
            Self::RemoteStop(_) => "remote-stop",
            Self::FsList(_) => "fs-list",
            Self::FsEntries(_) => "fs-entries",
            Self::FsRead(_) => "fs-read",
            Self::FsData(_) => "fs-data",
            Self::FsError(_) => "fs-error",
            Self::FsStat(_) => "fs-stat",
            Self::FsThumb(_) => "fs-thumb",
            Self::TextPartThumb(_) => "text-part",
            Self::FsWrite(_) => "fs-write",
            Self::FsDelete(_) => "fs-delete",
            Self::PhotoRequest(_) => "photo-request",
            Self::PhotoTaken(_) => "photo-taken",
            Self::PhotosRecent(_) => "photos-recent",
            Self::PhotosList(_) => "photos-list",
            Self::ContactsList(_) => "contacts-list",
            Self::Contacts(_) => "contacts",
            Self::ContactsChanged => "contacts-changed",
            Self::TextsList(_) => "texts-list",
            Self::Texts(_) => "texts",
            Self::TextsChanged => "texts-changed",
            Self::TextSend(_) => "text-send",
            Self::TextSent(_) => "text-sent",
            Self::CallsList(_) => "calls-list",
            Self::Calls(_) => "calls",
            Self::CallsChanged => "calls-changed",
            Self::Dial(_) => "dial",
            Self::Dialed(_) => "dialed",
            Self::RemoteInput(_) => "remote-input",
            Self::Commands(_) => "commands",
            Self::RunCommand(_) => "run-command",
            Self::Ping(_) => "ping",
            Self::Heartbeat(_) => "heartbeat",
            Self::HeartbeatAck(_) => "heartbeat-ack",
            Self::Presenting(_) => "presenting",
            Self::Capabilities(_) => "capabilities",
            Self::DesktopStatus(_) => "desktop-status",
            Self::SystemCommand(_) => "system-command",
            Self::SystemResult(_) => "system-result",
        }
    }

    fn body(&self) -> Result<Value, ciborium::value::Error> {
        match self {
            Self::Hello(body) => Value::serialized(body),
            Self::PairSpake(body) => Value::serialized(body),
            Self::PairConfirm(body) => Value::serialized(body),
            Self::Unpair
            | Self::HotspotRequest
            | Self::MirrorKeyframe
            | Self::ContactsChanged
            | Self::TextsChanged
            | Self::CallsChanged => Value::serialized(&Empty {}),
            Self::MirrorRequest(body) => Value::serialized(body),
            Self::Share(body) => Value::serialized(body),
            Self::ShareAck(body) => Value::serialized(body),
            Self::NotificationPosted(body) => Value::serialized(body),
            Self::NotificationRemoved(body) => Value::serialized(body),
            Self::NotificationSnapshot(body) => Value::serialized(body),
            Self::NotificationPull(body) => Value::serialized(body),
            Self::NotificationAction(body) => Value::serialized(body),
            Self::NotificationDismiss(body) => Value::serialized(body),
            Self::MediaPlayer(body) => Value::serialized(body),
            Self::MediaGone(body) => Value::serialized(body),
            Self::MediaCommand(body) => Value::serialized(body),
            Self::Ring(body) => Value::serialized(body),
            Self::Ringing(body) => Value::serialized(body),
            Self::Call(body) => Value::serialized(body),
            Self::CallAction(body) => Value::serialized(body),
            Self::Offer(body) => Value::serialized(body),
            Self::OfferReply(body) => Value::serialized(body),
            Self::Resume(body) | Self::Cancel(body) => Value::serialized(body),
            Self::ResumeAt(body) => Value::serialized(body),
            Self::FileDone(body) => Value::serialized(body),
            Self::FileHash(body) => Value::serialized(body),
            Self::FileData(body) => Value::serialized(body),
            Self::ClipOffer(body) => Value::serialized(body),
            Self::ClipPull(body) | Self::ClipData(body) => Value::serialized(body),
            Self::Status(body) => Value::serialized(body),
            Self::Hotspot(body) | Self::WifiDirect(body) => Value::serialized(body),
            Self::HotspotJoined(body) => Value::serialized(body),
            Self::HotspotEnd(body) => Value::serialized(body),
            Self::Punch(body) => Value::serialized(body),
            Self::WifiDirectReady(body) => Value::serialized(body),
            Self::BtPairing(body) => Value::serialized(body),
            Self::MirrorStarted(body) => Value::serialized(body),
            Self::ScreenShape(body) => Value::serialized(body),
            Self::MirrorStop(body) | Self::RemoteStop(body) => Value::serialized(body),
            Self::MirrorInput(body) => Value::serialized(body),
            Self::MirrorData(body) => Value::serialized(body),
            Self::RemoteRequest(body) => Value::serialized(body),
            Self::RemoteStarted(body) => Value::serialized(body),
            Self::FsList(body) => Value::serialized(body),
            Self::FsEntries(body) => Value::serialized(body),
            Self::FsRead(body) => Value::serialized(body),
            Self::FsData(body) => Value::serialized(body),
            Self::FsError(body) => Value::serialized(body),
            Self::FsStat(body) => Value::serialized(body),
            Self::FsThumb(body) => Value::serialized(body),
            Self::TextPartThumb(body) => Value::serialized(body),
            Self::FsWrite(body) => Value::serialized(body),
            Self::FsDelete(body) => Value::serialized(body),
            Self::PhotoRequest(body) => Value::serialized(body),
            Self::PhotoTaken(body) => Value::serialized(body),
            Self::PhotosRecent(body) => Value::serialized(body),
            Self::PhotosList(body) => Value::serialized(body),
            Self::ContactsList(body) => Value::serialized(body),
            Self::Contacts(body) => Value::serialized(body),
            Self::TextsList(body) => Value::serialized(body),
            Self::Texts(body) => Value::serialized(body),
            Self::TextSend(body) => Value::serialized(body),
            Self::TextSent(body) => Value::serialized(body),
            Self::CallsList(body) => Value::serialized(body),
            Self::Calls(body) => Value::serialized(body),
            Self::Dial(body) => Value::serialized(body),
            Self::Dialed(body) => Value::serialized(body),
            Self::RemoteInput(body) => Value::serialized(body),
            Self::Commands(body) => Value::serialized(body),
            Self::RunCommand(body) => Value::serialized(body),
            Self::Ping(body) => Value::serialized(body),
            Self::Heartbeat(body) => Value::serialized(body),
            Self::HeartbeatAck(body) => Value::serialized(body),
            Self::Presenting(body) => Value::serialized(body),
            Self::Capabilities(body) => Value::serialized(body),
            Self::DesktopStatus(body) => Value::serialized(body),
            Self::SystemCommand(body) => Value::serialized(body),
            Self::SystemResult(body) => Value::serialized(body),
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per message type")]
    fn from_parts(kind: &str, body: &Value) -> Result<Self, DecodeError> {
        let message = match kind {
            "hello" => Self::Hello(body.deserialized()?),
            "pair-spake" => Self::PairSpake(body.deserialized()?),
            "pair-confirm" => Self::PairConfirm(body.deserialized()?),
            "unpair" => {
                body.deserialized::<Empty>()?;
                Self::Unpair
            }
            "share" => Self::Share(body.deserialized()?),
            "share-ack" => Self::ShareAck(body.deserialized()?),
            "notification-posted" => Self::NotificationPosted(body.deserialized()?),
            "notification-removed" => Self::NotificationRemoved(body.deserialized()?),
            "notification-snapshot" => Self::NotificationSnapshot(body.deserialized()?),
            "notification-pull" => Self::NotificationPull(body.deserialized()?),
            "notification-action" => Self::NotificationAction(body.deserialized()?),
            "notification-dismiss" => Self::NotificationDismiss(body.deserialized()?),
            "media-player" => Self::MediaPlayer(body.deserialized()?),
            "media-gone" => Self::MediaGone(body.deserialized()?),
            "media-command" => Self::MediaCommand(body.deserialized()?),
            "ring" => Self::Ring(body.deserialized()?),
            "ringing" => Self::Ringing(body.deserialized()?),
            "call" => Self::Call(body.deserialized()?),
            "call-action" => Self::CallAction(body.deserialized()?),
            "offer" => Self::Offer(body.deserialized()?),
            "offer-reply" => Self::OfferReply(body.deserialized()?),
            "resume" => Self::Resume(body.deserialized()?),
            "resume-at" => Self::ResumeAt(body.deserialized()?),
            "cancel" => Self::Cancel(body.deserialized()?),
            "file-done" => Self::FileDone(body.deserialized()?),
            "file-hash" => Self::FileHash(body.deserialized()?),
            "file-data" => Self::FileData(body.deserialized()?),
            "clip-offer" => Self::ClipOffer(body.deserialized()?),
            "clip-pull" => Self::ClipPull(body.deserialized()?),
            "clip-data" => Self::ClipData(body.deserialized()?),
            "status" => Self::Status(body.deserialized()?),
            "hotspot-request" => {
                body.deserialized::<Empty>()?;
                Self::HotspotRequest
            }
            "mirror-request" => Self::MirrorRequest(body.deserialized()?),
            "mirror-keyframe" => {
                body.deserialized::<Empty>()?;
                Self::MirrorKeyframe
            }
            "hotspot" => Self::Hotspot(body.deserialized()?),
            "hotspot-joined" => Self::HotspotJoined(body.deserialized()?),
            "hotspot-end" => Self::HotspotEnd(body.deserialized()?),
            "punch" => Self::Punch(body.deserialized()?),
            "wifi-direct-ready" => Self::WifiDirectReady(body.deserialized()?),
            "wifi-direct" => Self::WifiDirect(body.deserialized()?),
            "bt-pairing" => Self::BtPairing(body.deserialized()?),
            "mirror-started" => Self::MirrorStarted(body.deserialized()?),
            "screen-shape" => Self::ScreenShape(body.deserialized()?),
            "mirror-stop" => Self::MirrorStop(body.deserialized()?),
            "mirror-input" => Self::MirrorInput(body.deserialized()?),
            "mirror-data" => Self::MirrorData(body.deserialized()?),
            "remote-request" => Self::RemoteRequest(body.deserialized()?),
            "remote-started" => Self::RemoteStarted(body.deserialized()?),
            "remote-stop" => Self::RemoteStop(body.deserialized()?),
            "fs-list" => Self::FsList(body.deserialized()?),
            "fs-entries" => Self::FsEntries(body.deserialized()?),
            "fs-read" => Self::FsRead(body.deserialized()?),
            "fs-data" => Self::FsData(body.deserialized()?),
            "fs-error" => Self::FsError(body.deserialized()?),
            "fs-stat" => Self::FsStat(body.deserialized()?),
            "fs-thumb" => Self::FsThumb(body.deserialized()?),
            "text-part" => Self::TextPartThumb(body.deserialized()?),
            "fs-write" => Self::FsWrite(body.deserialized()?),
            "fs-delete" => Self::FsDelete(body.deserialized()?),
            "photo-request" => Self::PhotoRequest(body.deserialized()?),
            "photo-taken" => Self::PhotoTaken(body.deserialized()?),
            "photos-recent" => Self::PhotosRecent(body.deserialized()?),
            "photos-list" => Self::PhotosList(body.deserialized()?),
            "contacts-list" => Self::ContactsList(body.deserialized()?),
            "contacts" => Self::Contacts(body.deserialized()?),
            "contacts-changed" => {
                body.deserialized::<Empty>()?;
                Self::ContactsChanged
            }
            "texts-list" => Self::TextsList(body.deserialized()?),
            "texts" => Self::Texts(body.deserialized()?),
            "texts-changed" => {
                body.deserialized::<Empty>()?;
                Self::TextsChanged
            }
            "text-send" => Self::TextSend(body.deserialized()?),
            "text-sent" => Self::TextSent(body.deserialized()?),
            "calls-list" => Self::CallsList(body.deserialized()?),
            "calls" => Self::Calls(body.deserialized()?),
            "calls-changed" => {
                body.deserialized::<Empty>()?;
                Self::CallsChanged
            }
            "dial" => Self::Dial(body.deserialized()?),
            "dialed" => Self::Dialed(body.deserialized()?),
            "remote-input" => Self::RemoteInput(body.deserialized()?),
            "commands" => Self::Commands(body.deserialized()?),
            "run-command" => Self::RunCommand(body.deserialized()?),
            "ping" => Self::Ping(body.deserialized()?),
            "heartbeat" => Self::Heartbeat(body.deserialized()?),
            "heartbeat-ack" => Self::HeartbeatAck(body.deserialized()?),
            "presenting" => Self::Presenting(body.deserialized()?),
            "capabilities" => Self::Capabilities(body.deserialized()?),
            "desktop-status" => Self::DesktopStatus(body.deserialized()?),
            "system-command" => Self::SystemCommand(body.deserialized()?),
            "system-result" => Self::SystemResult(body.deserialized()?),
            other => return Err(DecodeError::UnknownType(other.to_owned())),
        };
        message.validate()?;
        Ok(message)
    }

    /// The schema's rules, checked by the receiver while decoding and by senders before sending.
    #[expect(clippy::too_many_lines, reason = "one arm per message type")]
    pub fn validate(&self) -> Result<(), DecodeError> {
        let valid = match self {
            Self::Hello(hello) => (1..=MAX_NAME_LEN).contains(&hello.name.chars().count()),
            Self::PairSpake(spake) => spake.msg.len() == SPAKE_MSG_LEN,
            Self::PairConfirm(confirm) => confirm.mac.len() == MAC_LEN,
            Self::FileHash(hash) => hash.sha256.len() == SHA256_LEN,
            Self::Unpair
            | Self::HotspotRequest
            | Self::MirrorKeyframe
            | Self::ShareAck(_)
            | Self::Heartbeat(_)
            | Self::HeartbeatAck(_)
            | Self::Ring(_)
            | Self::Ringing(_)
            | Self::Presenting(_)
            | Self::CallAction(_)
            | Self::Resume(_)
            | Self::Cancel(_)
            | Self::FileDone(_)
            | Self::FileData(_)
            | Self::FsError(_)
            | Self::ContactsList(_)
            | Self::ContactsChanged
            | Self::TextsList(_)
            | Self::TextsChanged
            | Self::TextSent(_)
            | Self::CallsList(_)
            | Self::CallsChanged
            | Self::Dialed(_)
            | Self::PhotoRequest(_) => true,
            Self::Calls(page) => {
                page.calls.len() <= MAX_CALLS_PAGE
                    && (page.refused.is_none() || page.calls.is_empty())
                    && page.calls.iter().all(|call| {
                        sized(&call.number, 1, MAX_NUMBER_LEN)
                            && call.name.as_ref().is_none_or(|name| sized(name, 1, MAX_CALLER_LEN))
                    })
            }
            Self::Dial(dial) => dialable(&dial.number),
            Self::Call(call) => {
                call.number.as_ref().is_none_or(|number| sized(number, 1, MAX_NUMBER_LEN))
                    && call.name.as_ref().is_none_or(|name| sized(name, 1, MAX_CALLER_LEN))
                    && call.app.as_ref().is_none_or(|app| sized(app, 1, MAX_CALLER_LEN))
            }
            Self::Share(share) => share.check().is_ok(),
            Self::NotificationPosted(posted) => posted.valid(),
            Self::NotificationRemoved(NotificationRemoved { id })
            | Self::NotificationDismiss(NotificationDismiss { id }) => notification_id(id),
            Self::NotificationSnapshot(snapshot) => notification_ids(snapshot.notifications.iter().map(|d| &d.id)),
            Self::NotificationPull(pull) => notification_ids(pull.ids.iter()),
            Self::NotificationAction(action) => action.valid(),
            Self::MediaPlayer(player) => player.valid(),
            Self::MediaGone(gone) => player_id(&gone.player),
            Self::MediaCommand(command) => command.valid(),
            Self::ClipPull(pull) | Self::ClipData(pull) => (1..=MAX_MIME_LEN).contains(&pull.mime.len()),
            Self::ClipOffer(offer) => offer.is_valid(),
            Self::Status(status) => status.valid(),
            Self::Offer(offer) => offer.is_valid(),
            Self::OfferReply(reply) => reply.accepted == reply.reason.is_none(),
            Self::ResumeAt(at) => unique(at.offsets.iter().map(|offset| offset.file)),
            Self::Hotspot(hotspot) | Self::WifiDirect(hotspot) => hotspot.valid(),
            Self::HotspotJoined(joined) => joined.address.parse::<std::net::SocketAddr>().is_ok(),
            Self::HotspotEnd(end) => end.reason.as_ref().is_none_or(|reason| sized(reason, 1, MAX_REASON_LEN)),
            Self::Punch(punch) => {
                punch.addresses.len() <= MAX_PUNCH_ADDRESSES
                    && punch.addresses.iter().all(|address| address.parse::<std::net::SocketAddr>().is_ok())
            }
            Self::MirrorRequest(request) => request.codecs.len() <= MAX_MIRROR_CODECS,
            Self::MirrorStarted(MirrorStarted { width, height, .. }) => mirror_size_valid(*width, *height),
            Self::ScreenShape(shape) => shape.valid(),
            Self::MirrorData(MirrorData { width, height, gop, .. }) => {
                mirror_size_valid(*width, *height) && *gop != Some(0)
            }
            Self::MirrorStop(stop) | Self::RemoteStop(stop) => {
                stop.reason.as_ref().is_none_or(|reason| sized(reason, 1, MAX_REASON_LEN))
            }
            Self::RemoteStarted(started) => started.valid(),
            Self::RemoteRequest(request) => request
                .source
                .is_none_or(|source| usize::try_from(source).is_ok_and(|source| source < MAX_REMOTE_SOURCES)),
            Self::MirrorInput(input) => input.valid(),
            Self::WifiDirectReady(WifiDirectReady { name, address }) => {
                sized(name, 1, MAX_P2P_NAME_LEN) && address.as_ref().is_none_or(|address| hardware_address(address))
            }
            Self::BtPairing(BtPairing { passkey }) => passkey.is_none_or(|passkey| passkey <= 999_999),
            Self::FsList(list) => fs_path_valid(&list.path),
            Self::FsEntries(page) => {
                page.entries.len() <= MAX_FS_ENTRIES && page.entries.iter().all(|entry| fs_name_valid(&entry.name))
            }
            Self::FsRead(read) => fs_path_valid(&read.path) && (1..=MAX_FS_READ).contains(&read.len),
            Self::FsData(data) => data.data.len() <= MAX_FS_CHUNK,
            Self::FsStat(stat) => fs_path_valid(&stat.path),
            Self::FsThumb(thumb) => fs_path_valid(&thumb.path) && (1..=MAX_FS_THUMB_PX).contains(&thumb.px),
            Self::TextPartThumb(thumb) => (1..=MAX_FS_THUMB_PX).contains(&thumb.px),
            Self::FsWrite(write) => fs_path_valid(&write.path) && write.data.len() <= MAX_FS_CHUNK,
            Self::FsDelete(delete) => fs_path_valid(&delete.path),
            Self::PhotosRecent(recent) => (1..=MAX_RECENT_PHOTOS).contains(&recent.count),
            Self::PhotosList(list) => {
                list.photos.len() <= usize::from(MAX_RECENT_PHOTOS)
                    && list.photos.iter().all(|photo| fs_path_valid(&photo.path) && fs_name_valid(&photo.name))
            }
            Self::PhotoTaken(taken) => {
                taken.path.as_ref().is_some_and(|path| fs_path_valid(path)) != taken.refused.is_some()
            }
            Self::Contacts(page) => {
                page.contacts.len() <= MAX_CONTACTS_PAGE
                    && (page.refused.is_none() || page.contacts.is_empty())
                    && page.contacts.iter().all(|contact| {
                        sized(&contact.name, 1, MAX_CALLER_LEN)
                            && contact.numbers.len() <= MAX_CONTACT_NUMBERS
                            && contact.numbers.iter().all(|number| sized(number, 1, MAX_NUMBER_LEN))
                    })
            }
            Self::Texts(page) => {
                page.texts.len() <= MAX_TEXTS_PAGE
                    && (page.refused.is_none() || page.texts.is_empty())
                    && page.texts.iter().all(|text| {
                        sized(&text.address, 1, MAX_NUMBER_LEN)
                            && text.body.len() <= MAX_TEXT_BODY
                            && (text.sent || text.state.is_none())
                            && text.parts.len() <= MAX_TEXT_PARTS
                            && text.parts.iter().all(|part| sized(&part.mime, 1, MAX_MIME_LEN))
                    })
            }
            Self::TextSend(send) => sized(&send.to, 1, MAX_NUMBER_LEN) && sized(&send.body, 1, MAX_TEXT_BODY),
            Self::RemoteInput(input) => input.valid(),
            Self::Commands(commands) => commands.valid(),
            Self::RunCommand(run) => sized(&run.id, 1, MAX_COMMAND_ID_LEN),
            Self::Ping(ping) => ping.message.as_ref().is_none_or(|message| sized(message, 1, MAX_PING_LEN)),
            Self::Capabilities(capabilities) => {
                capabilities.can.len() <= MAX_CAPABILITIES
                    && capabilities.can.iter().all(|entry| sized(entry, 1, MAX_CAPABILITY_LEN))
            }
            Self::DesktopStatus(status) => status.valid(),
            Self::SystemCommand(command) => command.valid(),
            Self::SystemResult(result) => {
                result.error.as_deref().is_none_or(|error| sized(error, 1, MAX_SYSTEM_ERROR_LEN))
            }
        };
        if valid { Ok(()) } else { Err(DecodeError::Invalid(self.kind())) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub id: u64,
    pub message: Message,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEnvelope {
    #[serde(rename = "type")]
    kind: String,
    id: u64,
    body: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("malformed cbor: {0}")]
    Cbor(String),
    #[error("unknown message type {0:?}")]
    UnknownType(String),
    #[error("{0} message violates the schema")]
    Invalid(&'static str),
}

impl From<ciborium::value::Error> for DecodeError {
    fn from(error: ciborium::value::Error) -> Self {
        Self::Cbor(error.to_string())
    }
}

impl Envelope {
    pub fn new(id: u64, message: Message) -> Self {
        Self { id, message }
    }

    #[expect(clippy::expect_used, reason = "our own message types always serialize into memory")]
    pub fn to_cbor(&self) -> Vec<u8> {
        let raw = RawEnvelope {
            kind: self.message.kind().to_owned(),
            id: self.id,
            body: self.message.body().expect("message body serializes"),
        };
        let mut out = Vec::new();
        ciborium::into_writer(&raw, &mut out).expect("envelope serializes");
        out
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = std::io::Cursor::new(bytes);
        let raw: RawEnvelope =
            ciborium::from_reader(&mut reader).map_err(|error| DecodeError::Cbor(error.to_string()))?;
        if usize::try_from(reader.position()).ok() != Some(bytes.len()) {
            return Err(DecodeError::Cbor("trailing bytes after the envelope".to_owned()));
        }
        Ok(Self { id: raw.id, message: Message::from_parts(&raw.kind, &raw.body)? })
    }
}
