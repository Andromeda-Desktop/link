//! The hub actor: sole owner of the device store, the pairing window, and the set of live sessions.

mod bluetooth;
mod handle;
mod nearby;
mod remote;
mod transfers;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use link_core::discovery::Advertiser;
use link_core::identity::{DeviceId, Spki};
use link_core::link_state::LinkState;
use link_core::net;
use link_core::outbox::{self, Outbox, OutboxActor, Seen, Sent};
use link_core::path::LinkKind;
use link_core::proto::CloseCode;
use link_core::proto::message::{
    CALLS, CAMERA, CAN_CONTROL, CAN_INPUT, CAPABILITIES, CLIP_CLOCK, CLIP_ITEMS, CONTROL, Call, CallState,
    Capabilities, DesktopStatus, FEATURE, FS_STAT, FS_WRITE, Facing, FsRefusal, Hello, Hotspot, HotspotEnd,
    HotspotJoined, LATE_HASH, MAX_SYSTEM_ERROR_LEN, MESSAGES, MediaPlayer, Message, MessagesRefusal,
    NotificationPosted, NotificationPull, PHONE_TOGGLES, PHOTOS_PAGE, PHOTOS_RECENT, PhotoMedia, PhotoRefusal, Ping,
    RemoteInput, Ringer, SHARE_OUTBOX, SYSTEM, SYSTEM_CONTROL, Share, Status, SystemCommand, SystemCommandKind,
    SystemResult, TEXTS_MMS, Text, TransferId, USB,
};
use link_core::proto::pairing::{Secret, Secrets};
use link_core::session::{Route, SessionEvent, SessionHandle};
use link_core::store::{Feature, Peer, Store, StoredCommand, feature_of};
use link_core::transfer::{ClipPeer, Source, Status as TransferStatus, TransferEvent, TransferHandle};
use link_core::transport::Puncher;
use link_core::uri::{PairingUri, QR_SECRET_LEN};
use mdns_sd::ServiceDaemon;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::{mpsc, oneshot, watch};

/// Android shows its pairing code for 30 s; the window covers it and the user's tap.
const BT_PAIRING_WINDOW: Duration = Duration::from_secs(60);
/// A slider drag on the desktop moves the phone's slider live, at most this often.
const DESKTOP_STATUS_EVERY: Duration = Duration::from_millis(150);
/// How long a desktop's command waits for the phone's `system-result`.
const SYSTEM_TIMEOUT: Duration = Duration::from_secs(5);
/// Joining a hotspot takes the desktop off its Wi-Fi, so it waits this long for the user to allow it.
const HOTSPOT_ASK_TIMEOUT: Duration = Duration::from_secs(30);
/// Looking for a spare Wi-Fi interface asks `NetworkManager` and `iw`, which the hub waits for no longer than this.
const SPARE_TIMEOUT: Duration = Duration::from_secs(10);
const HOTSPOT_DECLINED: &str = "the hotspot was not allowed on this computer";
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::browse;
use crate::capabilities;
use crate::desktop_media::{DesktopMediaHandle, Request};
use crate::hotspot;
use crate::localsend::LocalSendHandle;
use crate::media::Media;
use crate::mirror::Capture;
use crate::notifications::Mirror;
use crate::paths::Paths;

const WINDOW: Duration = Duration::from_secs(120);

type SessionReply = oneshot::Sender<Result<SessionHandle, Gate>>;

/// Why a desktop action was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    NotConnected,
    /// The phone's switch for the action's feature is off for this desktop.
    NotShared,
}

pub enum Admission {
    Session,
    Pair(Attempt),
    Reject(CloseCode),
}

/// One pairing attempt: the secrets of the window that admitted it, and that window's id.
pub struct Attempt {
    pub secrets: Arc<Secrets>,
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    /// Unix seconds of the last session, kept in the device store.
    pub last_seen: i64,
    /// wifi, bluetooth, or empty when not connected.
    pub transport: &'static str,
    /// live, reconnecting (the session is open but the phone missed its heartbeat), or empty without a session.
    pub link: &'static str,
    /// `(free, total)` bytes of the phone's shared storage, as last reported; kept in memory past the session.
    pub storage: Option<(u64, u64)>,
    /// Percent, while connected.
    pub battery: Option<u8>,
    /// Connected, plays `OpenSpeaker` streams, and its speaker switch is on.
    pub speaker: bool,
    /// The phone's microphone: ready (connected, has one, its switch on), asking (waiting for the phone's consent),
    /// on, or empty when it cannot give one.
    pub mic: &'static str,
    /// The phone's media sound without its screen: the same values as `mic`, with the screen switch.
    pub sound: &'static str,
    /// The phone's cameras as a webcam: the same values as `mic`, with the webcam switch.
    pub camera: &'static str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "one field per D-Bus property")]
pub struct Snapshot {
    /// `(device_id, name, connected)`, the D-Bus `Devices` property.
    pub devices: Vec<(String, String, bool)>,
    /// One per paired device, the D-Bus `DeviceInfo` property.
    pub info: Vec<DeviceInfo>,
    pub pairing: bool,
    /// Devices whose file offers are accepted without asking, the D-Bus `AutoAccept` property.
    pub auto_accept: Vec<String>,
    /// Devices the user disconnected, the D-Bus `Paused` property.
    pub paused: Vec<String>,
    /// Device id to granted features, the D-Bus `Grants` property.
    pub grants: Vec<(String, Vec<String>)>,
    /// Connected device id to `(battery, charging, network, signal)`, the D-Bus `DeviceStatus` property.
    pub status: Vec<(String, (u32, bool, String, i32))>,
    /// Connected device id to `(media volume, ringer, dnd, locked)`, the D-Bus `DeviceControls` property.
    pub controls: Vec<(String, (i32, String, i32, i32))>,
    /// Connected device id to `(wifi, bluetooth, torch)`, the D-Bus `DeviceToggles` property.
    pub toggles: Vec<(String, (i32, i32, i32))>,
    /// Connected devices whose notification access is off, the D-Bus `NotificationsOff` property.
    pub notifications_off: Vec<String>,
    /// The D-Bus `NotificationCounts` property.
    pub notification_counts: Vec<(String, u32)>,
    /// The D-Bus `NotificationsWhileLocked` property.
    pub notify_locked: bool,
    /// "Visible to LocalSend", the D-Bus `LocalSendVisible` property.
    pub localsend: bool,
    /// Device id to its contacts as `(name, numbers)`, the D-Bus `Contacts` property.
    pub contacts: Vec<(String, crate::messages::Contacts)>,
    /// Device id to its newest texts, the D-Bus `Texts` property.
    pub texts: Vec<(String, crate::messages::Texts)>,
    /// Device id to its newest calls, the D-Bus `Calls` property.
    pub calls: Vec<(String, crate::messages::Calls)>,
    /// Device id to why its phone refused the call log, the D-Bus `CallsRefused` property.
    pub calls_refused: Vec<(String, String)>,
    /// A phone mirrors here or presents: the D-Bus `KeepAwake` property.
    pub keep_awake: bool,
    /// `(name, command)`, the D-Bus `Commands` property.
    pub commands: Vec<(String, String)>,
    /// What `link-session` can do now, the D-Bus `SessionCapabilities` property.
    pub capabilities: Vec<String>,
    /// Device id to its lead player, the D-Bus `PhonePlayers` property.
    pub players: Vec<(String, crate::media::PhonePlayer)>,
    /// The D-Bus `LanState` property.
    pub lan: crate::lan::LanState,
}

#[derive(Debug, Clone)]
pub enum Event {
    PairingFinished {
        id: DeviceId,
        name: String,
    },
    PairingFailed {
        reason: String,
    },
    Received {
        id: DeviceId,
        share: Share,
    },
    /// A transfer in either direction, from any backend.
    Transfer(Signal),
    ClipboardOffered {
        from: DeviceId,
        id: u64,
        mimes: Vec<String>,
        size: u64,
    },
    NotificationPosted {
        id: DeviceId,
        posted: NotificationPosted,
        /// Whether the desktop sounds and pops it up, or only replaces the shown one.
        alert: bool,
    },
    NotificationRemoved {
        id: DeviceId,
        notification: String,
    },
    /// The phone asks this desktop to ring, or to stop.
    RingRequested {
        id: DeviceId,
        on: bool,
    },
    PhoneRinging {
        id: DeviceId,
        on: bool,
    },
    ScreenShape {
        id: DeviceId,
        shape: link_core::proto::message::ScreenShape,
    },
    Call {
        id: DeviceId,
        call: Call,
    },
    TextReceived {
        id: DeviceId,
        text: Text,
    },
    /// A phone offers its hotspot (`ssid`); the desktop joins only once the user allows it. An empty `ssid` withdraws
    /// the question.
    HotspotAsk {
        id: DeviceId,
        ssid: String,
    },
    /// The desktop is joining a phone's hotspot (`ssid`), or left it or failed to join (None).
    Hotspot {
        id: DeviceId,
        ssid: Option<String>,
    },
    /// A phone's command for the media keys player, on a desktop without MPRIS: play-pause, next, or previous.
    MediaKey {
        key: &'static str,
    },
    /// The phone's touchpad or keyboard.
    RemoteInput {
        id: DeviceId,
        input: RemoteInput,
    },
    /// The phone asks for the desktop's screen, or to switch to `source`.
    RemoteRequested {
        id: DeviceId,
        source: Option<u32>,
    },
    /// Remote control ended on the phone, or the phone left.
    RemoteStopped {
        id: DeviceId,
    },
    /// A command for the desktop from a phone with the system grant; `serial` is its message id, which
    /// `system_result` answers, or 0 for one nobody waits on, as from Do Not Disturb sync.
    SystemCommand {
        id: DeviceId,
        serial: u64,
        command: SystemCommand,
    },
    Pinged {
        id: DeviceId,
        message: Option<String>,
    },
}

/// What any transfer backend reports, as the shell sees it: the Link transfer actor and LocalSend alike.
#[derive(Debug, Clone)]
pub enum Signal {
    /// An offer waiting for consent; `device` is a Link device id or `localsend:<fingerprint>`.
    Offered {
        id: TransferId,
        device: String,
        files: Vec<(String, u64)>,
    },
    Progress {
        id: TransferId,
        bytes: u64,
        total: u64,
    },
    Finished {
        id: TransferId,
        status: TransferStatus,
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Link,
    LocalSend,
}

enum Command {
    StartPairing {
        reply: oneshot::Sender<anyhow::Result<(String, String)>>,
    },
    CancelPairing,
    UsbCables(usize),
    Unpair {
        id: DeviceId,
        reply: oneshot::Sender<bool>,
    },
    SetPaused {
        id: DeviceId,
        paused: bool,
        reply: oneshot::Sender<bool>,
    },
    AnswerHotspot {
        id: DeviceId,
        allow: bool,
    },
    ConfirmBtPairing {
        passkey: u32,
        reply: oneshot::Sender<bool>,
    },
    Admit {
        spki: Spki,
        reply: oneshot::Sender<Admission>,
    },
    Paired {
        spki: Spki,
        name: String,
        window: u64,
    },
    PairingFailed {
        reason: String,
        window: u64,
    },
    ReleaseWindow {
        window: u64,
    },
    Connected {
        id: DeviceId,
        hello: Hello,
        session: SessionHandle,
    },
    Disconnected {
        id: DeviceId,
        stable_id: usize,
    },
    Session {
        id: DeviceId,
        needs: Option<Feature>,
        reply: SessionReply,
    },
    SetAutoAccept {
        id: DeviceId,
        enabled: bool,
        reply: oneshot::Sender<bool>,
    },
    SetGrant {
        id: DeviceId,
        feature: String,
        granted: bool,
        reply: oneshot::Sender<bool>,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    SendFiles {
        device: String,
        sources: Vec<Source>,
        reply: oneshot::Sender<Result<TransferId, link_core::Error>>,
    },
    Decide {
        id: TransferId,
        accept: bool,
        reply: oneshot::Sender<bool>,
    },
    CancelTransfer {
        id: TransferId,
        reply: oneshot::Sender<bool>,
    },
    SetLocalSend {
        visible: bool,
    },
    #[cfg_attr(not(windows), expect(dead_code, reason = "only the Windows firewall doctor reports"))]
    SetLan {
        state: crate::lan::LanState,
    },
    SetNotifyLocked {
        on: bool,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    ClipboardPeers {
        reply: oneshot::Sender<Vec<DeviceId>>,
    },
    DesktopPlayer {
        player: MediaPlayer,
    },
    DesktopPlayerGone {
        player: String,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "Windows reads its players through GSMTC, not media keys"))]
    MediaKey {
        key: &'static str,
    },
    Browse {
        id: DeviceId,
        request: browse::Request,
        reply: browse::Reply,
    },
    SendText {
        id: DeviceId,
        to: String,
        body: String,
        reply: crate::messages::SendReply,
    },
    Dial {
        id: DeviceId,
        number: String,
        reply: crate::messages::SendReply,
    },
    TakePhoto {
        id: DeviceId,
        reply: crate::messages::PhotoReply,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    OpenMirror {
        id: DeviceId,
        ask: crate::mirror::Ask,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    ServeRemote {
        id: DeviceId,
        stream: RemoteStream,
        reply: oneshot::Sender<Result<(), String>>,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    OpenCapture {
        id: DeviceId,
        audio: Capture,
        ask: crate::mirror::Ask,
    },
    StopCapture {
        id: DeviceId,
        audio: Capture,
    },
    #[cfg_attr(windows, expect(dead_code, reason = "only the Unix Link1 members that pass a descriptor use it"))]
    OpenSpeaker {
        id: DeviceId,
        reply: oneshot::Sender<Result<crate::local::Handle, crate::mirror::Failure>>,
    },
    StopRemote {
        id: DeviceId,
        reason: String,
    },
    SetCommands {
        commands: Vec<StoredCommand>,
        reply: oneshot::Sender<bool>,
    },
    SetCapabilities {
        owner: String,
        can: Vec<String>,
    },
    CapabilitiesGone {
        owner: String,
    },
    SetDesktopStatus {
        status: DesktopStatus,
    },
    SystemResult {
        id: DeviceId,
        serial: u64,
        error: Option<String>,
    },
    SystemToPhone {
        id: DeviceId,
        command: SystemCommand,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Ping {
        id: DeviceId,
        message: Option<String>,
        reply: oneshot::Sender<Result<(), link_core::Error>>,
    },
    Doctor {
        reply: oneshot::Sender<Vec<Checkup>>,
    },
}

/// One paired device as `link doctor` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkup {
    pub id: DeviceId,
    pub name: String,
    /// "live", "reconnecting", or "offline".
    pub state: &'static str,
    /// "wifi", "nearby" (the phone's Wi-Fi Direct group or hotspot), "bluetooth", or "" while offline.
    pub path: &'static str,
    /// QUIC's smoothed round trip; None while offline.
    pub rtt: Option<Duration>,
    /// Packets QUIC declared lost on this session.
    pub lost: u64,
    /// Shares in the outbox for this device.
    pub outbox: usize,
}

#[derive(Clone)]
pub struct HubHandle {
    commands: mpsc::Sender<Command>,
    session_events: mpsc::Sender<SessionEvent>,
    transfers: TransferHandle,
    outbox: Outbox,
}

struct Window {
    id: u64,
    secrets: Arc<Secrets>,
    deadline: Instant,
    taken: bool,
}

pub struct Hub {
    own: Spki,
    store: Store,
    paths: Paths,
    /// Present only while someone could be looking: a paired device or an open pairing window.
    advertiser: Option<Advertiser>,
    lan: crate::lan::LanState,
    /// The process's one mDNS responder; absent when it failed to start.
    mdns: Option<ServiceDaemon>,
    window: Option<Window>,
    windows_opened: u64,
    sessions: HashMap<DeviceId, SessionHandle>,
    status: HashMap<DeviceId, Status>,
    /// The last storage each phone reported, kept past its session.
    storage: HashMap<DeviceId, link_core::proto::message::Storage>,
    notifications: Mirror,
    media: Media,
    /// Phones with a call ringing or active, during which the desktop's players stay paused.
    in_call: HashSet<DeviceId>,
    /// Its own handle, for the exported MPRIS players to reach sessions.
    me: HubHandle,
    commands: mpsc::Receiver<Command>,
    session_events: mpsc::Receiver<SessionEvent>,
    snapshots: watch::Sender<Snapshot>,
    events: mpsc::Sender<Event>,
    transfers: TransferHandle,
    transfer_events: mpsc::UnboundedReceiver<TransferEvent>,
    outbox: Outbox,
    outbox_actor: Option<OutboxActor>,
    /// Share items already applied, so a resend after a lost ack is applied once.
    seen: Seen,
    localsend: LocalSendHandle,
    localsend_signals: mpsc::UnboundedReceiver<Signal>,
    /// Which backend each open transfer belongs to, for consent and cancelling.
    open: HashMap<TransferId, Backend>,
    /// Each phone's group or hotspot this desktop joins or joined.
    nearby: HashMap<DeviceId, Nearby>,
    /// Spare-interface lookups and joins, off the actor.
    joining: JoinSet<JoinStep>,
    /// The code a phone with a session reported while Android pairs Bluetooth with it, until when it counts.
    bt_pairing: Option<(u32, Instant)>,
    /// This desktop's Bluetooth adapter, named in the QR code, and the deadline that keeps it open to pairing.
    bluetooth: Option<(String, watch::Sender<Option<Instant>>)>,
    /// True while the adapter advertises the pairing window over LE.
    beacon: Option<watch::Sender<bool>>,
    /// Work too large for Bluetooth, waiting for the phone's session to move to its hotspot.
    waiting: Vec<Waiting>,
    /// Phones on Bluetooth already asked for their hotspot by browsing, which does not wait for it.
    hotspot_asked: HashSet<DeviceId>,
    browsing: browse::Browsing,
    /// Phones whose `adb reverse` reaches this desktop's USB listener; a phone that connects meanwhile gets usb-ready.
    usb_cables: usize,
    /// Each connected phone's `feature:` entries from its hello.
    features: HashMap<DeviceId, Vec<String>>,
    /// What each connected phone lets this desktop use, from its last `sharing`; none yet (an older phone): every
    /// action goes out and the phone decides.
    sharing: HashMap<DeviceId, Vec<Feature>>,
    capabilities: capabilities::Reports,
    /// The desktop's state as `link-session` last reported it.
    desktop_status: Option<DesktopStatus>,
    /// When `desktop_status` last went to phones, and when a change held back by [`DESKTOP_STATUS_EVERY`] goes.
    status_sent: Option<Instant>,
    status_due: Option<Instant>,
    /// `system-command`s sent to phones, by phone and message id, waiting for their `system-result`.
    system_waiting: HashMap<(DeviceId, u64), oneshot::Sender<Result<(), String>>>,
    messages: crate::messages::Messages,
    mirrors: crate::mirror::Mirrors,
    /// Phones whose presenter for this desktop is open.
    presenting: HashSet<DeviceId>,
    remotes: crate::remote::Remotes,
    /// Each phone's pump of desktop sound for its speaker.
    speakers: crate::remote::Remotes,
    /// Phones' microphones: requests waiting for the phone, and the pumps to `OpenMic`'s sockets.
    mics: crate::mirror::Mirrors,
    /// Phones' media sound without the screen, the same way, to `OpenPhoneSound`'s sockets.
    sounds: crate::mirror::Mirrors,
    /// Phones' cameras as webcams, the same way, to `OpenCamera`'s sockets.
    cameras: crate::mirror::Mirrors,
    puncher: Puncher,
}

/// `link-session`'s socket of access units for a phone's remote control, and what it shows.
pub struct RemoteStream {
    pub socket: crate::local::Handle,
    pub width: u32,
    pub height: u32,
    pub started: link_core::proto::message::RemoteStarted,
}

/// A phone's Wi-Fi Direct group or hotspot, from the first step of joining it until this desktop leaves it.
enum Nearby {
    /// `asked` holds the user's answer and the network's name while the shell asks.
    Joining {
        kind: NearbyKind,
        asked: Option<(oneshot::Sender<bool>, String)>,
    },
    Joined(hotspot::Active),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NearbyKind {
    Group,
    Hotspot,
}

/// What a task in `joining` reports to the hub.
enum JoinStep {
    /// The spare-interface lookup for `group` ended; without one the user is asked first.
    Spare {
        id: DeviceId,
        group: Hotspot,
        spare: Option<crate::wifi_direct::Spare>,
    },
    Joined {
        id: DeviceId,
        kind: NearbyKind,
        joined: Result<hotspot::Joined, String>,
    },
}

struct Waiting {
    peer: DeviceId,
    work: Parked,
    deadline: Instant,
}

enum Parked {
    Send { sources: Vec<Source>, reply: oneshot::Sender<Result<TransferId, link_core::Error>> },
    Mirror(crate::mirror::Ask),
}

/// The transfer backends the hub consents for: the Link transfer actor, which it also attaches sessions to, and
/// LocalSend.
pub struct Transfers {
    pub handle: TransferHandle,
    pub events: mpsc::UnboundedReceiver<TransferEvent>,
    pub localsend: LocalSendHandle,
    pub localsend_signals: mpsc::UnboundedReceiver<Signal>,
}

impl Hub {
    pub fn new(
        own: Spki,
        store: Store,
        paths: Paths,
        desktop_media: DesktopMediaHandle,
        transfers: Transfers,
        puncher: Puncher,
        mdns: Option<ServiceDaemon>,
    ) -> (Self, HubHandle, watch::Receiver<Snapshot>, mpsc::Receiver<Event>) {
        let (commands_tx, commands) = mpsc::channel(32);
        let (session_events_tx, session_events) = mpsc::channel(16);
        let (outbox, outbox_actor) = outbox::outbox(paths.state.join("outbox.json"));
        let handle = HubHandle {
            commands: commands_tx,
            session_events: session_events_tx,
            transfers: transfers.handle.clone(),
            outbox: outbox.clone(),
        };
        let media = Media::new(desktop_media, paths.art.clone());
        let notifications = Mirror::load(paths.notifications.clone());
        let (snapshots, snapshots_rx) = watch::channel(Snapshot::default());
        let (events, events_rx) = mpsc::channel(16);
        let mut hub = Self {
            own,
            store,
            paths,
            advertiser: None,
            lan: crate::lan::LanState::Unknown,
            mdns,
            window: None,
            windows_opened: 0,
            sessions: HashMap::new(),
            status: HashMap::new(),
            storage: HashMap::new(),
            notifications,
            media,
            in_call: HashSet::new(),
            me: handle.clone(),
            commands,
            session_events,
            snapshots,
            events,
            transfers: transfers.handle.clone(),
            transfer_events: transfers.events,
            localsend: transfers.localsend,
            localsend_signals: transfers.localsend_signals,
            open: HashMap::new(),
            nearby: HashMap::new(),
            joining: JoinSet::new(),
            bt_pairing: None,
            bluetooth: None,
            beacon: None,
            waiting: Vec::new(),
            hotspot_asked: HashSet::new(),
            browsing: browse::Browsing::default(),
            usb_cables: 0,
            features: HashMap::new(),
            sharing: HashMap::new(),
            outbox,
            outbox_actor: Some(outbox_actor),
            seen: Seen::default(),
            capabilities: capabilities::Reports::default(),
            desktop_status: None,
            status_sent: None,
            status_due: None,
            system_waiting: HashMap::new(),
            messages: crate::messages::Messages::default(),
            mirrors: crate::mirror::Mirrors::default(),
            presenting: HashSet::new(),
            remotes: crate::remote::Remotes::default(),
            speakers: crate::remote::Remotes::default(),
            mics: crate::mirror::Mirrors::capture(Capture::Mic),
            sounds: crate::mirror::Mirrors::capture(Capture::Sound),
            cameras: crate::mirror::Mirrors::capture(Capture::Camera(Facing::Back)),
            puncher,
        };
        hub.refresh();
        (hub, handle, snapshots_rx, events_rx)
    }

    /// Runs until `shutdown` ends, then leaves any phone hotspot or group this desktop joined.
    pub async fn run(mut self, shutdown: impl std::future::Future<Output = anyhow::Result<()>>) -> anyhow::Result<()> {
        tokio::pin!(shutdown);
        if let Some(outbox) = self.outbox_actor.take() {
            tokio::spawn(outbox.run());
        }
        if self.store.localsend {
            self.localsend.set_visible(true).await;
        }
        loop {
            let deadline = self.window.as_ref().map(|window| window.deadline);
            let give_up = self.waiting.iter().map(|waiting| waiting.deadline).min();
            let browse_deadline = self.browsing.deadline();
            let contacts_deadline = self.messages.deadline();
            let mirror_deadline = self.mirrors.deadline();
            let mic_deadline = self.mics.deadline();
            let sound_deadline = self.sounds.deadline();
            let camera_deadline = self.cameras.deadline();
            let status_due = self.status_due;
            tokio::select! {
                result = &mut shutdown => {
                    for (_, nearby) in self.nearby.drain() {
                        if let Nearby::Joined(active) = nearby {
                            hotspot::leave(&active).await;
                        }
                    }
                    return result;
                }
                () = sleep_until(status_due) => self.flush_desktop_status().await,
                () = sleep_until(mirror_deadline) => {
                    for id in self.mirrors.expire() {
                        self.tell_mirror_stop(&id, "nobody allowed it on the phone in time").await;
                    }
                }
                () = sleep_until(mic_deadline) => {
                    for id in self.mics.expire() {
                        self.tell_capture_stop(&id, Capture::Mic).await;
                    }
                }
                () = sleep_until(sound_deadline) => {
                    for id in self.sounds.expire() {
                        self.tell_capture_stop(&id, Capture::Sound).await;
                    }
                }
                () = sleep_until(camera_deadline) => {
                    for id in self.cameras.expire() {
                        self.tell_capture_stop(&id, Capture::Camera(Facing::Back)).await;
                    }
                }
                Some(id) = self.cameras.ended() => {
                    log::info!("{id}: the camera stream ended");
                    self.tell_capture_stop(&id, Capture::Camera(Facing::Back)).await;
                }
                Some(id) = self.mics.ended() => {
                    log::info!("{id}: the microphone stream ended");
                    self.tell_capture_stop(&id, Capture::Mic).await;
                }
                Some(id) = self.sounds.ended() => {
                    log::info!("{id}: the phone's sound stream ended");
                    self.tell_capture_stop(&id, Capture::Sound).await;
                }
                Some(id) = self.mirrors.ended() => {
                    self.tell_mirror_stop(&id, "the viewer closed").await;
                    self.publish();
                }
                Some(id) = self.remotes.ended() => log::debug!("{id}: a remote control stream ended"),
                Some(id) = self.speakers.ended() => {
                    log::info!("{id}: the speaker stream ended");
                    self.speakers.stop(&id);
                }
                () = sleep_until(browse_deadline) => self.browsing.expire(),
                () = sleep_until(contacts_deadline) => self.messages.expire(),
                Some(step) = self.joining.join_next() => match step {
                    Ok(JoinStep::Spare { id, group, spare }) => self.join_group(id, group, spare).await,
                    Ok(JoinStep::Joined { id, kind, joined }) => self.hotspot_joined(id, kind, joined).await,
                    Err(join) => log::warn!("hotspot join task: {join}"),
                },
                () = sleep_until(give_up) => self.expire_waiting().await,
                command = self.commands.recv() => match command {
                    Some(command) => self.handle(command).await,
                    None => return Ok(()),
                },
                Some(event) = self.session_events.recv() => self.on_session_event(event).await,
                Some(event) = self.transfer_events.recv() => self.on_transfer(event).await,
                Some(signal) = self.localsend_signals.recv() => self.on_signal(Backend::LocalSend, signal).await,
                () = sleep_until(deadline) => {
                    log::info!("pairing window expired");
                    self.close_window();
                }
            }
            if self.mics.take_changed() | self.sounds.take_changed() | self.cameras.take_changed() {
                self.publish();
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per command")]
    async fn handle(&mut self, command: Command) {
        match command {
            Command::StartPairing { reply } => drop(reply.send(self.start_pairing())),
            Command::CancelPairing => self.close_window(),
            Command::UsbCables(cables) => self.usb_cables(cables).await,
            Command::ConfirmBtPairing { passkey, reply } => {
                let confirmed =
                    self.bt_pairing.take().is_some_and(|(code, until)| code == passkey && Instant::now() < until);
                let _ = reply.send(confirmed);
            }
            Command::AnswerHotspot { id, allow } => {
                let asked = match self.nearby.get_mut(&id) {
                    Some(Nearby::Joining { asked, .. }) => asked.take(),
                    _ => None,
                };
                if let Some((answer, ssid)) = asked {
                    if allow {
                        self.emit(Event::Hotspot { id: id.clone(), ssid: Some(ssid) }).await;
                    }
                    let _ = answer.send(allow);
                }
            }
            Command::Unpair { id, reply } => {
                let unpaired = self.unpair(&id);
                self.media.disconnected(&id).await;
                self.forget_notifications(&id).await;
                let _ = reply.send(unpaired);
            }
            Command::SetPaused { id, paused, reply } => drop(reply.send(self.set_paused(&id, paused))),
            Command::Admit { spki, reply } => drop(reply.send(self.admit(&spki))),
            Command::Paired { spki, name, window } => self.paired(&spki, name, window).await,
            Command::PairingFailed { reason, window } => {
                if self.close_window_if(window) {
                    self.emit(Event::PairingFailed { reason }).await;
                }
            }
            Command::ReleaseWindow { window } => self.release_window(window),
            Command::Connected { id, hello, session } => self.connected(id, hello, session).await,
            Command::Disconnected { id, stable_id } => self.disconnected(id, stable_id).await,
            Command::SendText { id, to, body, reply } => self.send_text(id, to, body, reply).await,
            Command::Dial { id, number, reply } => self.dial(id, number, reply).await,
            Command::TakePhoto { id, reply } => self.take_photo(id, reply).await,
            Command::Browse { id, request, reply } => self.browse(id, request, reply).await,
            Command::OpenMirror { id, ask } => self.open_mirror(id, ask).await,
            Command::ServeRemote { id, stream, reply } => drop(reply.send(self.serve_remote(id, stream).await)),
            Command::OpenCapture { id, audio, ask } => self.open_capture(id, audio, ask).await,
            Command::StopCapture { id, audio } => {
                self.capture(audio).stopped(&id, Some(format!("the desktop stopped the {}", audio.name())));
                self.tell_capture_stop(&id, audio).await;
            }
            Command::OpenSpeaker { id, reply } => drop(reply.send(self.open_speaker(id).await)),
            Command::StopRemote { id, reason } => self.stop_remote(&id, &reason).await,
            Command::SetCommands { commands, reply } => drop(reply.send(self.set_commands(commands).await)),
            Command::SetCapabilities { owner, can } => {
                self.change_capabilities(|reports| reports.report(owner, can)).await;
            }
            Command::SetDesktopStatus { status } => self.set_desktop_status(status).await,
            Command::SystemResult { id, serial, error } => self.answer_system(&id, serial, error).await,
            Command::SystemToPhone { id, command, reply } => self.send_system_command(id, command, reply).await,
            Command::CapabilitiesGone { owner } => {
                if self.change_capabilities(|reports| reports.gone(&owner)).await {
                    // The session that captured the screen is gone.
                    for id in self.remotes.stop_all() {
                        self.tell_remote_stop(&id, "the desktop stopped sharing its screen").await;
                    }
                }
            }
            Command::Ping { id, message, reply } => drop(reply.send(self.ping(&id, message).await)),
            Command::Doctor { reply } => drop(reply.send(self.checkups())),
            Command::DesktopPlayer { player } => self.media.desktop_player(player, self.sessions.values()),
            Command::DesktopPlayerGone { player } => self.media.desktop_gone(player, self.sessions.values()),
            Command::MediaKey { key } => self.emit(Event::MediaKey { key }).await,
            Command::Session { id, needs, reply } => {
                let session = self.sessions.get(&id).filter(|session| session.is_live()).cloned();
                drop(reply.send(match session {
                    None => Err(Gate::NotConnected),
                    Some(_) if needs.is_some_and(|feature| !self.shares(&id, feature)) => Err(Gate::NotShared),
                    Some(session) => Ok(session),
                }));
            }
            Command::SetAutoAccept { id, enabled, reply } => {
                let Some(peer) = self.store.peer_mut(&id) else {
                    let _ = reply.send(false);
                    return;
                };
                peer.auto_accept = enabled;
                self.save();
                self.publish();
                let _ = reply.send(true);
            }
            Command::SetGrant { id, feature, granted, reply } => {
                let _ = reply.send(self.set_grant(&id, &feature, granted).await);
            }
            Command::SendFiles { device, sources, reply } => match self.needs_hotspot(&device, &sources) {
                Some(peer) => self.wait_for_hotspot(peer, Parked::Send { sources, reply }).await,
                None => drop(reply.send(self.send_files(&device, sources).await)),
            },
            Command::Decide { id, accept, reply } => {
                let answered = match self.open.get(&id) {
                    Some(Backend::LocalSend) => self.localsend.decide(id, accept).await,
                    _ => self.transfers.decide(id, accept).await.unwrap_or(false),
                };
                let _ = reply.send(answered);
            }
            Command::CancelTransfer { id, reply } => {
                let cancelled = match self.open.get(&id) {
                    Some(Backend::LocalSend) => self.localsend.cancel(id).await,
                    _ => self.transfers.cancel(id).await.unwrap_or(false),
                };
                let _ = reply.send(cancelled);
            }
            Command::SetLocalSend { visible } => {
                self.store.localsend = visible;
                self.save();
                self.publish();
                self.localsend.set_visible(visible).await;
            }
            Command::SetLan { state } => {
                self.lan = state;
                self.publish();
            }
            Command::SetNotifyLocked { on } => {
                self.store.notify_locked = on;
                self.save();
                self.publish();
            }
            Command::ClipboardPeers { reply } => {
                let granted = |id: &DeviceId| self.store.peer(id).is_some_and(|peer| peer.grants.clipboard);
                let peers = self.sessions.iter().filter(|(id, session)| session.is_live() && granted(id));
                drop(reply.send(peers.map(|(id, _)| id.clone()).collect()));
            }
        }
    }

    /// A live session gets the phone's queued shares.
    fn on_link(&mut self, from: &DeviceId, state: LinkState) {
        if matches!(state, LinkState::Live { .. })
            && let Some(session) = self.sessions.get(from)
        {
            self.outbox.live(from.clone(), session.clone(), self.phone_has(from, SHARE_OUTBOX));
        }
        self.publish();
    }

    async fn on_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Received { from, share } if self.sessions.contains_key(&from) => {
                self.on_share(from, share).await;
            }
            SessionEvent::Status { from, status } if self.sessions.contains_key(&from) => {
                self.on_status(from, status).await;
            }
            SessionEvent::SystemCommand { from, id, command } if self.sessions.contains_key(&from) => {
                self.on_system_command(from, id, command).await;
            }
            SessionEvent::Link { from, state } => self.on_link(&from, state),
            SessionEvent::Path { from, on_bluetooth } => self.moved(&from, on_bluetooth).await,
            SessionEvent::Received { .. } | SessionEvent::Status { .. } | SessionEvent::SystemCommand { .. } => {}
            SessionEvent::Unpaired { from } => self.unpaired(from).await,
            SessionEvent::Message { from, message } if browse::is_answer(&message) => {
                // Answers to this desktop's own requests, which need no grant of the phone's here.
                if let Some(next) = self.browsing.on_answer(&from, message)
                    && let Some(session) = self.sessions.get(&from)
                    && session.send(next).await.is_err()
                {
                    log::info!("{from}: asking for the next page of a listing failed");
                }
            }
            // The phone's own switch governs its contacts and texts; this desktop asked for them.
            SessionEvent::Message { from, message: Message::Contacts(page) } => {
                let changed = self.messages.on_contacts(&from, page);
                self.messages_changed(from, changed).await;
            }
            SessionEvent::Message { from, message: Message::Texts(page) } => {
                let changed = self.messages.on_texts(&from, page);
                self.messages_changed(from, changed).await;
            }
            SessionEvent::Message { from, message: Message::TextSent(sent) } => self.messages.on_sent(&from, &sent),
            SessionEvent::Message { from, message: Message::Calls(page) } => {
                let changed = self.messages.on_calls(&from, page);
                self.messages_changed(from, changed).await;
            }
            SessionEvent::Message { from, message: Message::Dialed(dialed) } => self.messages.on_dialed(&from, &dialed),
            SessionEvent::Message { from, message: Message::PhotoTaken(taken) } => self.messages.on_photo(&from, taken),
            SessionEvent::Message { from, message: Message::CallsChanged } if self.phone_has(&from, CALLS) => {
                let pull = self.messages.pull_calls(from.clone());
                self.send_to(&from, pull).await;
            }
            SessionEvent::Message { from, message: Message::ContactsChanged } if self.phone_has(&from, MESSAGES) => {
                let pull = self.messages.pull_contacts(from.clone());
                self.send_to(&from, pull).await;
            }
            SessionEvent::Message { from, message: Message::TextsChanged } if self.phone_has(&from, MESSAGES) => {
                let pull = self.messages.pull_texts(from.clone());
                self.send_to(&from, pull).await;
            }
            // Answers to this desktop's own mirror request, which needs no grant of the phone's here.
            SessionEvent::Message { from, message: Message::MirrorStarted(started) } => {
                log::info!("{from}: the phone allowed mirroring at {}x{}", started.width, started.height);
            }
            SessionEvent::Message { from, message: Message::CameraStarted(started) } => {
                log::info!("{from}: the phone allowed its camera at {}x{}", started.width, started.height);
            }
            SessionEvent::Message { from, message: Message::MicStop } => self.phone_stopped(&from, Capture::Mic),
            SessionEvent::Message { from, message: Message::SoundStop } => self.phone_stopped(&from, Capture::Sound),
            SessionEvent::Message { from, message: Message::CameraStop(stop) } => {
                self.camera_stopped(&from, stop.reason);
            }
            SessionEvent::Message { from, message: Message::MirrorStop(stop) } => {
                log::info!("{from}: the phone stopped mirroring: {:?}", stop.reason);
                self.mirrors.stopped(&from, stop.reason);
                self.publish();
            }
            SessionEvent::Message { from, message: Message::Punch(punch) } if self.sessions.contains_key(&from) => {
                let addresses: Vec<std::net::SocketAddr> =
                    punch.addresses.iter().filter_map(|text| text.parse().ok()).collect();
                log::info!("{from}: punching toward {addresses:?}");
                self.puncher.punch(&addresses);
            }
            // Connection housekeeping, not a shared feature: any paired phone with a session may pair Bluetooth.
            SessionEvent::Message { from, message: Message::BtPairing(pairing) }
                if self.sessions.contains_key(&from) =>
            {
                log::info!("{from}: pairing Bluetooth");
                let until = Instant::now() + BT_PAIRING_WINDOW;
                self.open_bluetooth_until(until);
                if let Some(passkey) = pairing.passkey {
                    self.bt_pairing = Some((passkey, until));
                }
            }
            SessionEvent::Message { from, message: Message::Sharing(sharing) } => {
                self.set_sharing(from, &sharing.allows);
                self.publish();
            }
            SessionEvent::Message { from, message: Message::Ping(ping) } if self.sessions.contains_key(&from) => {
                self.emit(Event::Pinged { id: from, message: ping.message }).await;
            }
            SessionEvent::Message { from, message } => {
                let granted = feature_of(&message)
                    .is_some_and(|feature| self.store.peer(&from).is_some_and(|peer| peer.grants.allows(feature)));
                if !granted {
                    log::info!("{from}: dropped a {} its grant does not allow", message.kind());
                } else if self.sessions.contains_key(&from) {
                    self.on_message(from, message).await;
                }
            }
        }
    }

    async fn unpaired(&mut self, from: DeviceId) {
        log::info!("{from} unpaired itself");
        self.sessions.remove(&from);
        self.store.remove(&from);
        self.outbox.forget(from.clone());
        self.messages.forget(&from);
        self.save();
        self.refresh();
        self.media.disconnected(&from).await;
        self.forget_notifications(&from).await;
    }

    async fn on_message(&mut self, from: DeviceId, message: Message) {
        match message {
            Message::NotificationPosted(posted) => {
                if let Some(alert) = self.notifications.post(&from, &posted) {
                    for notification in self.notifications.trim(&from) {
                        self.emit(Event::NotificationRemoved { id: from.clone(), notification }).await;
                    }
                    self.emit(Event::NotificationPosted { id: from, posted, alert }).await;
                    self.publish();
                }
            }
            Message::NotificationSnapshot(snapshot) => {
                let (gone, ids) = self.notifications.snapshot(&from, &snapshot.notifications);
                for notification in gone {
                    self.emit(Event::NotificationRemoved { id: from.clone(), notification }).await;
                }
                if !ids.is_empty() {
                    self.send_to(&from, Message::NotificationPull(NotificationPull { ids })).await;
                }
                self.publish();
            }
            Message::NotificationRemoved(removed) => {
                if self.notifications.remove(&from, &removed.id) {
                    self.emit(Event::NotificationRemoved { id: from, notification: removed.id }).await;
                    self.publish();
                }
            }
            media @ (Message::MediaPlayer(_) | Message::MediaGone(_) | Message::MediaCommand(_)) => {
                let name = self.store.peer(&from).map(|peer| peer.name.clone()).unwrap_or_default();
                self.media.on_phone_message(&self.me, &from, &name, media).await;
                self.publish();
            }
            Message::Ring(ring) => self.emit(Event::RingRequested { id: from, on: ring.on }).await,
            Message::Call(call) => {
                self.set_in_call(&from, call.state != CallState::Idle).await;
                self.emit(Event::Call { id: from, call }).await;
            }
            Message::Ringing(ringing) => self.emit(Event::PhoneRinging { id: from, on: ringing.on }).await,
            Message::ScreenShape(shape) => self.emit(Event::ScreenShape { id: from, shape }).await,
            Message::Hotspot(offered) => self.join_hotspot(from, offered).await,
            Message::WifiDirect(group) => self.join_wifi_direct(from, group).await,
            Message::HotspotEnd(end) => {
                log::info!("{from}: the phone ended its hotspot: {:?}", end.reason);
                let reason = end.reason.unwrap_or_else(|| "the phone ended its hotspot".to_owned());
                self.fail_waiting(&from, &reason).await;
                self.leave_hotspot(&from).await;
            }
            Message::RemoteInput(input) => self.emit(Event::RemoteInput { id: from, input }).await,
            Message::Presenting(presenting) => {
                if presenting.on {
                    self.presenting.insert(from);
                } else {
                    self.presenting.remove(&from);
                }
                self.publish();
            }
            Message::RemoteRequest(request) => self.remote_request(from, request.source).await,
            Message::RemoteStop(stop) => {
                log::info!("{from}: the phone ended remote control: {:?}", stop.reason);
                self.remotes.stop(&from);
                self.emit(Event::RemoteStopped { id: from }).await;
            }
            Message::SystemResult(result) => {
                if let Some(waiting) = self.system_waiting.remove(&(from, result.command)) {
                    drop(waiting.send(result.error.map_or(Ok(()), Err)));
                }
            }
            Message::RunCommand(run) => {
                match self.store.commands.iter().find(|stored| crate::control::id(stored) == run.id) {
                    Some(stored) => match crate::control::run(stored) {
                        Ok(()) => log::info!("{from}: ran the command {:?}", stored.name),
                        Err(error) => log::warn!("{from}: running the command {:?}: {error}", stored.name),
                    },
                    None => log::info!("{from}: no command {}", run.id),
                }
            }
            other => log::warn!("{from}: a desktop session delivered {}", other.kind()),
        }
    }

    /// Pauses the desktop's players when a first phone's call starts, and resumes them when the last one ends.
    async fn set_in_call(&mut self, id: &DeviceId, calling: bool) {
        let before = self.in_call.is_empty();
        if calling {
            self.in_call.insert(id.clone());
        } else {
            self.in_call.remove(id);
        }
        match (before, self.in_call.is_empty()) {
            (true, false) => self.media.request(Request::PauseAll).await,
            (false, true) => self.media.request(Request::ResumePaused).await,
            _ => {}
        }
    }

    async fn forget_notifications(&mut self, id: &DeviceId) {
        for notification in self.notifications.forget(id) {
            self.emit(Event::NotificationRemoved { id: id.clone(), notification }).await;
        }
        self.publish();
    }

    /// One session per device: a newer one replaces the older, which a phone that changed networks leaves behind.
    async fn connected(&mut self, id: DeviceId, hello: Hello, session: SessionHandle) {
        let Some(peer) = self.store.peer_mut(&id) else {
            return session.close(CloseCode::NotPaired);
        };
        if peer.paused {
            return session.close(CloseCode::Paused);
        }
        peer.name = hello.name;
        peer.touch();
        self.save();
        self.media.connected(&session);
        let on_ip = !session.on_bluetooth();
        if on_ip {
            log::info!("{id}: connected over IP from {}", session.connection().quic().remote_address());
        } else {
            log::info!("{id}: connected over Bluetooth");
        }
        if let Some(older) = self.sessions.insert(id.clone(), session.clone()) {
            older.close(CloseCode::Done);
        }
        self.hotspot_asked.remove(&id);
        self.publish();
        let features: Vec<String> =
            hello.addresses.iter().filter(|entry| entry.starts_with(FEATURE)).cloned().collect();
        let clip = ClipPeer {
            granted: self.store.peer(&id).is_some_and(|peer| peer.grants.clipboard),
            items: features.iter().any(|feature| feature == CLIP_ITEMS),
            clock: features.iter().any(|feature| feature == CLIP_CLOCK),
        };
        let late_hash = features.iter().any(|feature| feature == LATE_HASH);
        self.transfers.attach(id.clone(), session.clone(), clip, late_hash).await;
        self.features.insert(id.clone(), features);
        self.sharing.remove(&id);
        self.outbox.live(id.clone(), session.clone(), self.phone_has(&id, SHARE_OUTBOX));
        if self.phone_has(&id, MESSAGES) {
            let contacts = self.messages.pull_contacts(id.clone());
            self.send_to(&id, contacts).await;
            let texts = self.messages.pull_texts(id.clone());
            self.send_to(&id, texts).await;
        }
        if self.phone_has(&id, CALLS) {
            let calls = self.messages.pull_calls(id.clone());
            self.send_to(&id, calls).await;
        }
        self.send_commands(&id).await;
        self.send_capabilities(&id).await;
        self.send_desktop_status(&id).await;
        if self.usb_cables > 0 && self.phone_has(&id, USB) {
            self.send_to(&id, Message::UsbReady).await;
        }
        self.moved(&id, !on_ip).await;
    }

    /// A new cable reaches this desktop: each live phone that takes `usb-ready` tries it, since a cable has no phone id.
    async fn usb_cables(&mut self, cables: usize) {
        let added = cables > self.usb_cables;
        self.usb_cables = cables;
        if !added {
            return;
        }
        let phones: Vec<(DeviceId, SessionHandle)> = self
            .sessions
            .iter()
            .filter(|(id, session)| session.is_live() && self.phone_has(id, USB))
            .map(|(id, session)| (id.clone(), session.clone()))
            .collect();
        for (id, session) in phones {
            if let Err(error) = session.send(Message::UsbReady).await {
                log::info!("{id}: usb-ready: {error}");
            }
        }
    }

    /// A session that reached IP takes the sends waiting for it; one back on Bluetooth means the phone's hotspot is
    /// gone, even if its hotspot-end was lost with an old session.
    async fn moved(&mut self, id: &DeviceId, on_bluetooth: bool) {
        if !self.sessions.contains_key(id) {
            return;
        }
        if on_bluetooth {
            log::info!("{id}: the session is on Bluetooth");
        } else {
            let session = &self.sessions[id];
            log::info!(
                "{id}: the session is on {} from {}",
                transport(session),
                session.connection().quic().remote_address()
            );
        }
        if on_bluetooth {
            self.leave_hotspot(id).await;
        } else {
            self.send_waiting(id).await;
        }
        self.publish();
    }

    async fn browse(&mut self, id: DeviceId, request: browse::Request, reply: browse::Reply) {
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            return drop(reply.send(Err(browse::Failure::NotConnected)));
        };
        // The phone answers a text's picture part under Messages (phone_requires), every other request under Browse.
        let needs =
            if matches!(request, browse::Request::TextPart { .. }) { Feature::Messages } else { Feature::Browse };
        if !self.shares(&id, needs) {
            return drop(reply.send(Err(browse::Failure::Refused(FsRefusal::NotAllowed))));
        }
        if request.bulk() && session.on_bluetooth() {
            self.ask_hotspot_once(&id, &session).await;
        }
        if request.writes() {
            let granted = self.store.peer(&id).is_some_and(|peer| peer.grants.files);
            if !granted || !self.phone_has(&id, FS_WRITE) {
                let reason = if granted { FsRefusal::Denied } else { FsRefusal::NotAllowed };
                return drop(reply.send(Err(browse::Failure::Refused(reason))));
            }
        }
        if matches!(request, browse::Request::TextPart { .. }) && !self.phone_has(&id, TEXTS_MMS) {
            return drop(reply.send(Err(browse::Failure::Refused(FsRefusal::NotFound))));
        }
        if matches!(request, browse::Request::Recent { .. }) && !self.phone_has(&id, PHOTOS_RECENT) {
            return drop(reply.send(Err(browse::Failure::Refused(FsRefusal::NotFound))));
        }
        let request = match request {
            // An older phone has only its newest photos: they stand in for the first page of All and Photos.
            browse::Request::Recent { count, page: Some(page) } if !self.phone_has(&id, PHOTOS_PAGE) => {
                if page.offset > 0 || page.media == PhotoMedia::Videos {
                    return drop(reply.send(Ok(browse::Answer::Photos { photos: Vec::new(), total: None })));
                }
                browse::Request::Recent { count, page: None }
            }
            other => other,
        };
        let request = if self.phone_has(&id, FS_STAT) { Ok(request) } else { request.for_older_phone() };
        let request = match request {
            Ok(request) => request,
            Err(failure) => return drop(reply.send(Err(failure))),
        };
        let (req, message) = self.browsing.start(id, request, reply);
        if session.send(message).await.is_err() {
            self.browsing.fail(req, browse::Failure::NotConnected);
        }
    }

    async fn send_text(&mut self, id: DeviceId, to: String, body: String, reply: crate::messages::SendReply) {
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            let _ = reply.send(Err(crate::messages::SendFailure::NotConnected));
            return;
        };
        if !self.shares(&id, Feature::Messages) {
            let _ = reply.send(Err(crate::messages::SendFailure::Refused(MessagesRefusal::NotAllowed)));
            return;
        }
        let (req, message) = self.messages.send_text(id, to, body, reply);
        if session.send(message).await.is_err() {
            self.messages.answer_send(req, Some(crate::messages::SendFailure::NotConnected));
        }
    }

    async fn dial(&mut self, id: DeviceId, number: String, reply: crate::messages::SendReply) {
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            let _ = reply.send(Err(crate::messages::SendFailure::NotConnected));
            return;
        };
        if !self.shares(&id, Feature::Calls) {
            let _ = reply.send(Err(crate::messages::SendFailure::Refused(MessagesRefusal::NotAllowed)));
            return;
        }
        let (req, message) = self.messages.dial(id, number, reply);
        if session.send(message).await.is_err() {
            self.messages.answer_send(req, Some(crate::messages::SendFailure::NotConnected));
        }
    }

    async fn take_photo(&mut self, id: DeviceId, reply: crate::messages::PhotoReply) {
        use crate::messages::PhotoFailure;
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            let _ = reply.send(Err(PhotoFailure::NotConnected));
            return;
        };
        if !self.phone_has(&id, CAMERA) {
            let _ = reply.send(Err(PhotoFailure::Refused(PhotoRefusal::Denied)));
            return;
        }
        if !self.shares(&id, Feature::Browse) {
            let _ = reply.send(Err(PhotoFailure::Refused(PhotoRefusal::NotAllowed)));
            return;
        }
        let (req, message) = self.messages.take_photo(id, reply);
        if session.send(message).await.is_err() {
            self.messages.answer_photo(req, Err(PhotoFailure::NotConnected));
        }
    }

    async fn messages_changed(&mut self, from: DeviceId, changed: crate::messages::Changed) {
        if let Some(next) = changed.next {
            self.send_to(&from, next).await;
        }
        if changed.publish {
            self.publish();
        }
        for text in changed.received {
            self.emit(Event::TextReceived { id: from.clone(), text }).await;
        }
    }

    async fn send_to(&self, id: &DeviceId, message: Message) {
        let kind = message.kind();
        if let Some(session) = self.sessions.get(id)
            && let Err(error) = session.send(message).await
        {
            log::info!("{id}: sending {kind}: {error}");
        }
    }

    /// Keeps what a connected phone shares; names this desktop does not know are skipped.
    fn set_sharing(&mut self, id: DeviceId, allows: &[String]) {
        if self.sessions.contains_key(&id) {
            self.sharing.insert(id, allows.iter().filter_map(|name| Feature::parse(name)).collect());
        }
    }

    /// Whether phone `id` lets this desktop use `feature`; true until its first `sharing` (an older phone decides alone).
    fn shares(&self, id: &DeviceId, feature: Feature) -> bool {
        self.sharing.get(id).is_none_or(|shared| shared.contains(&feature))
    }

    fn phone_has(&self, id: &DeviceId, feature: &str) -> bool {
        self.features.get(id).is_some_and(|features| features.iter().any(|entry| entry == feature))
    }

    async fn set_grant(&mut self, id: &DeviceId, feature: &str, granted: bool) -> bool {
        let (Some(peer), Some(feature)) = (self.store.peer_mut(id), Feature::parse(feature)) else { return false };
        peer.grants.set(feature, granted);
        self.save();
        self.publish();
        match feature {
            Feature::Commands => self.send_commands(id).await,
            // Tells the phone whether it obeys this desktop's commands, or may send it input.
            Feature::System | Feature::Input => self.send_capabilities(id).await,
            _ => {}
        }
        true
    }

    async fn set_commands(&mut self, commands: Vec<StoredCommand>) -> bool {
        if !crate::control::valid(&commands) {
            return false;
        }
        self.store.commands = commands;
        self.save();
        self.publish();
        for id in self.sessions.keys().cloned().collect::<Vec<_>>() {
            self.send_commands(&id).await;
        }
        true
    }

    async fn ping(&self, id: &DeviceId, message: Option<String>) -> Result<(), link_core::Error> {
        match self.sessions.get(id) {
            Some(_) if !self.phone_has(id, CONTROL) => Err(link_core::Error::Unexpected("ping")),
            Some(session) => session.send(Message::Ping(Ping { message })).await,
            None => Err(link_core::Error::NotConnected),
        }
    }

    fn capabilities(&self) -> &[String] {
        self.capabilities.current()
    }

    async fn change_capabilities<T>(&mut self, change: impl FnOnce(&mut capabilities::Reports) -> T) -> T {
        let before = self.capabilities().to_vec();
        let result = change(&mut self.capabilities);
        // A state no session reports any more is not sent to phones that connect later.
        if self.capabilities.is_empty() {
            self.desktop_status = None;
        }
        if before == self.capabilities() {
            return result;
        }
        self.publish();
        for id in self.sessions.keys().cloned().collect::<Vec<_>>() {
            self.send_capabilities(&id).await;
        }
        result
    }

    async fn send_capabilities(&self, id: &DeviceId) {
        if self.phone_has(id, CAPABILITIES) {
            let mut can = self.capabilities().to_vec();
            // "input" means this phone may use it: the session takes input and the phone has the Input grant.
            if !self.store.peer(id).is_some_and(|peer| peer.grants.input) {
                can.retain(|entry| entry != CAN_INPUT);
            }
            if self.phone_has(id, SYSTEM_CONTROL) && self.store.peer(id).is_some_and(|peer| peer.grants.system) {
                can.push(CAN_CONTROL.to_owned());
            }
            self.send_to(id, Message::Capabilities(Capabilities { can })).await;
        }
    }

    async fn send_desktop_status(&self, id: &DeviceId) {
        let Some(status) = self.desktop_status.clone().filter(|_| self.phone_has(id, SYSTEM)) else { return };
        let status = if self.phone_has(id, SYSTEM_CONTROL) { Some(status) } else { status.without_control() };
        if let Some(status) = status {
            self.send_to(id, Message::DesktopStatus(status)).await;
        }
    }

    async fn set_desktop_status(&mut self, status: DesktopStatus) {
        if self.desktop_status.as_ref() == Some(&status) {
            return;
        }
        let dnd_changed = self.desktop_status.as_ref().and_then(|old| old.dnd) != status.dnd;
        self.desktop_status = Some(status);
        let now = Instant::now();
        match self.status_sent.map(|sent| sent + DESKTOP_STATUS_EVERY).filter(|next| *next > now) {
            Some(next) => self.status_due = Some(next),
            None => self.flush_desktop_status().await,
        }
        if dnd_changed {
            self.sync_dnd_to_phones().await;
        }
    }

    /// Records a phone's status; with Do Not Disturb sync, the phone's new DND goes to `link-session`. On connect the
    /// desktop's DND wins, so a DND set here while the phone was away stays.
    async fn on_status(&mut self, from: DeviceId, status: Status) {
        if let Some(storage) = status.storage {
            self.storage.insert(from.clone(), storage);
        }
        let Some(before) = self.status.insert(from.clone(), status).map(|old| old.dnd) else {
            self.publish();
            self.sync_dnd_to_phones().await;
            return;
        };
        self.publish();
        if let Some(dnd) = status.dnd
            && before != Some(dnd)
            && self.syncs_dnd(&from)
            && self.desktop_status.as_ref().and_then(|desktop| desktop.dnd) != Some(dnd)
        {
            let command = SystemCommand { command: SystemCommandKind::SetDnd, value: Some(u8::from(dnd)) };
            self.emit(Event::SystemCommand { id: from, serial: 0, command }).await;
        }
    }

    async fn on_system_command(&self, from: DeviceId, id: u64, command: SystemCommand) {
        if !command.command.to_desktop() {
            log::info!("{from}: dropped a {:?} meant for a phone", command.command);
        } else if self.store.peer(&from).is_some_and(|peer| peer.grants.system) {
            self.emit(Event::SystemCommand { id: from, serial: id, command }).await;
        } else {
            log::info!("{from}: dropped a system-command its grant does not allow");
            let error = "Control system is off for this phone on the desktop".to_owned();
            self.answer_system(&from, id, Some(error)).await;
        }
    }

    async fn flush_desktop_status(&mut self) {
        self.status_due = None;
        self.status_sent = Some(Instant::now());
        for id in self.sessions.keys().cloned().collect::<Vec<_>>() {
            self.send_desktop_status(&id).await;
        }
    }

    fn syncs_dnd(&self, id: &DeviceId) -> bool {
        self.phone_has(id, SYSTEM_CONTROL)
            && self.store.peer(id).is_some_and(|peer| peer.grants.system && peer.grants.dnd_sync)
    }

    /// Sends the desktop's Do Not Disturb to the phones that sync it and differ.
    async fn sync_dnd_to_phones(&self) {
        let Some(dnd) = self.desktop_status.as_ref().and_then(|status| status.dnd) else { return };
        for id in self.sessions.keys() {
            if self.syncs_dnd(id) && self.status.get(id).and_then(|status| status.dnd) != Some(dnd) {
                let command = SystemCommand { command: SystemCommandKind::SetDnd, value: Some(u8::from(dnd)) };
                self.send_to(id, Message::SystemCommand(command)).await;
            }
        }
    }

    /// Answers `system-command` `serial` from `id`; 0 is one nobody waits on.
    async fn answer_system(&self, id: &DeviceId, serial: u64, error: Option<String>) {
        if serial != 0 && self.phone_has(id, SYSTEM_CONTROL) {
            let error = error.map(|error| error.chars().take(MAX_SYSTEM_ERROR_LEN).collect());
            self.send_to(id, Message::SystemResult(SystemResult { command: serial, error })).await;
        }
    }

    async fn send_system_command(
        &mut self,
        id: DeviceId,
        command: SystemCommand,
        reply: oneshot::Sender<Result<(), String>>,
    ) {
        let refused = if !self.sessions.contains_key(&id) {
            Some("the phone is not connected")
        } else if !self.phone_has(&id, SYSTEM_CONTROL)
            || (command.command.toggle() && !self.phone_has(&id, PHONE_TOGGLES))
        {
            Some("the phone's Link app is too old")
        } else if !self.store.peer(&id).is_some_and(|peer| peer.grants.system) {
            Some("Control system is off for this phone")
        } else if !command.command.to_phone() {
            Some("not a command for a phone")
        } else if command.value == Some(0)
            && self.sessions.get(&id).is_some_and(|session| cuts_link(session, command.command))
        {
            Some("Turning that off would end this connection")
        } else {
            None
        };
        if let Some(refused) = refused {
            drop(reply.send(Err(refused.to_owned())));
            return;
        }
        let Some(session) = self.sessions.get(&id) else { return };
        match session.send_numbered(Message::SystemCommand(command)).await {
            Ok(serial) => {
                self.system_waiting.retain(|_, waiting| !waiting.is_closed());
                self.system_waiting.insert((id, serial), reply);
            }
            Err(error) => drop(reply.send(Err(error.to_string()))),
        }
    }

    /// The commands `id` may run: all of them with its grant, none without.
    async fn send_commands(&self, id: &DeviceId) {
        let (Some(session), Some(peer)) = (self.sessions.get(id), self.store.peer(id)) else { return };
        if !self.phone_has(id, CONTROL) {
            return;
        }
        let granted = if peer.grants.commands { self.store.commands.as_slice() } else { &[] };
        if let Err(error) = session.send(Message::Commands(crate::control::listing(granted))).await {
            log::info!("{id}: sending the commands: {error}");
        }
    }

    /// A session ended; what hung on it goes only if no newer session replaced it.
    async fn disconnected(&mut self, id: DeviceId, stable_id: usize) {
        // A session queues its last events, such as a share it acked just before the phone left, before it ends.
        while let Ok(event) = self.session_events.try_recv() {
            self.on_session_event(event).await;
        }
        self.transfers.detach(id.clone(), stable_id).await;
        if self.sessions.get(&id).is_some_and(|live| live.stable_id() == stable_id) {
            self.sessions.remove(&id);
            self.hotspot_asked.remove(&id);
            self.browsing.disconnected(&id);
            self.messages.disconnected(&id);
            self.mirrors.fail(&id, crate::mirror::Failure::NotConnected);
            self.mics.stopped(&id, Some("the phone disconnected".to_owned()));
            self.sounds.stopped(&id, Some("the phone disconnected".to_owned()));
            self.cameras.stopped(&id, Some("the phone disconnected".to_owned()));
            self.speakers.stop(&id);
            if self.remotes.running(&id) {
                self.remotes.stop(&id);
                self.emit(Event::RemoteStopped { id: id.clone() }).await;
            }
            self.leave_hotspot(&id).await;
            self.status.remove(&id);
            self.presenting.remove(&id);
            self.media.disconnected(&id).await;
            self.publish();
            self.set_in_call(&id, false).await;
        }
    }

    fn set_paused(&mut self, id: &DeviceId, paused: bool) -> bool {
        let Some(peer) = self.store.peer_mut(id) else { return false };
        peer.paused = paused;
        if paused && let Some(session) = self.sessions.get(id) {
            session.close(CloseCode::Paused);
        }
        self.save();
        self.publish();
        true
    }

    fn unpair(&mut self, id: &DeviceId) -> bool {
        if self.store.remove(id).is_none() {
            return false;
        }
        if let Some(session) = self.sessions.remove(id) {
            session.close(CloseCode::Unpaired);
        }
        self.store.revoked.push(id.clone());
        self.outbox.forget(id.clone());
        self.messages.forget(id);
        self.save();
        self.refresh();
        true
    }

    /// Publishes the D-Bus snapshot and starts, updates, or stops the mDNS advertisement to match.
    fn refresh(&mut self) {
        self.publish();
        let pairing = self.window.is_some();
        if let Some(beacon) = &self.beacon {
            beacon.send_if_modified(|on| std::mem::replace(on, pairing) != pairing);
        }
        if !pairing && self.store.peers.is_empty() {
            self.advertiser = None;
            return;
        }
        if let Some(advertiser) = &mut self.advertiser {
            return log_mdns(advertiser.set_pairing(pairing));
        }
        let Some(mdns) = &self.mdns else { return };
        match Advertiser::start(mdns, &self.own.device_id(), self.store.port, pairing) {
            Ok(advertiser) => self.advertiser = Some(advertiser),
            Err(error) => log::warn!("mdns: {error}"),
        }
    }

    fn save(&self) {
        if let Err(error) = self.store.save(&self.paths.devices) {
            log::error!("saving the device store: {error}");
        }
    }

    fn link(&self, id: &DeviceId) -> &'static str {
        match self.sessions.get(id).map(SessionHandle::link) {
            Some(LinkState::Live { .. }) => "live",
            Some(LinkState::Degraded { .. }) => "reconnecting",
            Some(LinkState::Closed) | None => "",
        }
    }

    fn checkups(&self) -> Vec<Checkup> {
        self.store
            .peers
            .iter()
            .map(|peer| {
                let session = self.sessions.get(&peer.id).filter(|session| session.is_live());
                let quic = session.map(|session| session.connection().quic());
                Checkup {
                    id: peer.id.clone(),
                    name: peer.name.clone(),
                    state: match self.link(&peer.id) {
                        "" => "offline",
                        state => state,
                    },
                    path: match session {
                        None => "",
                        Some(session) if session.link_kind() == Some(LinkKind::Usb) => "usb",
                        Some(session) if session.on_bluetooth() => "bluetooth",
                        Some(_) if matches!(self.nearby.get(&peer.id), Some(Nearby::Joined(_))) => "nearby",
                        Some(_) => "wifi",
                    },
                    rtt: quic.map(quinn::Connection::rtt),
                    lost: quic.map_or(0, |quic| quic.stats().path.lost_packets),
                    outbox: 0,
                }
            })
            .collect()
    }

    fn phone_stopped(&mut self, from: &DeviceId, audio: Capture) {
        log::info!("{from}: the phone stopped its {}", audio.name());
        self.capture(audio).stopped(from, Some(format!("the phone stopped its {}", audio.name())));
    }

    fn camera_stopped(&mut self, from: &DeviceId, reason: Option<String>) {
        log::info!("{from}: the phone stopped its camera: {reason:?}");
        self.cameras.stopped(from, reason.or_else(|| Some("the phone stopped its camera".to_owned())));
    }

    fn capture(&mut self, audio: Capture) -> &mut crate::mirror::Mirrors {
        match audio {
            Capture::Mic => &mut self.mics,
            Capture::Sound => &mut self.sounds,
            Capture::Camera(_) => &mut self.cameras,
        }
    }

    /// The D-Bus `DeviceInfo` mic and sound fields: see [`DeviceInfo::mic`].
    fn capture_state(&self, id: &DeviceId, audio: Capture) -> &'static str {
        let streams = match audio {
            Capture::Mic => &self.mics,
            Capture::Sound => &self.sounds,
            Capture::Camera(_) => &self.cameras,
        };
        streams.state(id).unwrap_or(
            if self.sessions.contains_key(id) && self.phone_has(id, audio.feature()) && self.shares(id, audio.grant()) {
                "ready"
            } else {
                ""
            },
        )
    }

    fn publish(&self) {
        let devices = self
            .store
            .peers
            .iter()
            .map(|peer| (peer.id.to_string(), peer.name.clone(), self.link(&peer.id) == "live"))
            .collect();
        let info = self
            .store
            .peers
            .iter()
            .map(|peer| {
                let transport = self.sessions.get(&peer.id).map_or("", transport);
                DeviceInfo {
                    id: peer.id.to_string(),
                    last_seen: i64::try_from(peer.last_seen).unwrap_or(0),
                    transport,
                    link: self.link(&peer.id),
                    storage: self.storage.get(&peer.id).map(|storage| (storage.free, storage.total)),
                    battery: self.status.get(&peer.id).map(|status| status.battery),
                    speaker: self.sessions.contains_key(&peer.id)
                        && self.phone_has(&peer.id, link_core::proto::message::SPEAKER)
                        && self.shares(&peer.id, Feature::Speaker),
                    mic: self.capture_state(&peer.id, Capture::Mic),
                    camera: self.capture_state(&peer.id, Capture::Camera(Facing::Back)),
                    sound: self.capture_state(&peer.id, Capture::Sound),
                }
            })
            .collect();
        let auto_accept =
            self.store.peers.iter().filter(|peer| peer.auto_accept).map(|peer| peer.id.to_string()).collect();
        let paused = self.store.peers.iter().filter(|peer| peer.paused).map(|peer| peer.id.to_string()).collect();
        let grants = self.store.peers.iter().map(|peer| (peer.id.to_string(), peer.grants.names())).collect();
        let mut status: Vec<_> = self
            .status
            .iter()
            .map(|(id, status)| {
                let signal = status.signal.map_or(-1, i32::from);
                let value = (u32::from(status.battery), status.charging, status.network.as_str().to_owned(), signal);
                (id.to_string(), value)
            })
            .collect();
        status.sort();
        let flag = |value: Option<bool>| value.map_or(-1, i32::from);
        let mut controls: Vec<_> = self
            .status
            .iter()
            .map(|(id, status)| {
                let ringer = status.ringer.map_or("", Ringer::as_str).to_owned();
                let value = (status.media_volume.map_or(-1, i32::from), ringer, flag(status.dnd), flag(status.locked));
                (id.to_string(), value)
            })
            .collect();
        controls.sort();
        let mut toggles: Vec<_> = self
            .status
            .iter()
            .filter(|(_, status)| status.wifi.is_some() || status.bluetooth.is_some() || status.torch.is_some())
            .map(|(id, status)| (id.to_string(), (flag(status.wifi), flag(status.bluetooth), flag(status.torch))))
            .collect();
        toggles.sort();
        let mut notifications_off: Vec<String> =
            self.status.iter().filter(|(_, status)| status.notifications_off).map(|(id, _)| id.to_string()).collect();
        notifications_off.sort();
        self.snapshots.send_if_modified(|current| {
            let localsend = self.store.localsend;
            let (contacts, texts) = (self.messages.contacts(), self.messages.texts());
            let (calls, calls_refused) = (self.messages.calls(), self.messages.calls_refused());
            let commands =
                self.store.commands.iter().map(|stored| (stored.name.clone(), stored.command.clone())).collect();
            let pairing = self.window.is_some();
            let capabilities = self.capabilities().to_vec();
            let next = Snapshot {
                devices,
                info,
                pairing,
                auto_accept,
                paused,
                grants,
                status,
                controls,
                toggles,
                notifications_off,
                notification_counts: self.notifications.counts(),
                notify_locked: self.store.notify_locked,
                localsend,
                contacts,
                texts,
                calls,
                calls_refused,
                keep_awake: self.mirrors.active() || !self.presenting.is_empty(),
                commands,
                capabilities,
                players: self.media.players(),
                lan: self.lan,
            };
            let changed = *current != next;
            *current = next;
            changed
        });
    }

    async fn emit(&self, event: Event) {
        if self.events.send(event).await.is_err() {
            log::debug!("no D-Bus forwarder for hub events");
        }
    }
}

fn log_mdns(result: Result<(), link_core::Error>) {
    if let Err(error) = result {
        log::warn!("mdns: {error}");
    }
}

fn total_size(sources: &[Source]) -> u64 {
    sources.iter().map(Source::size).sum()
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// A uniform 6-digit code and a 128-bit QR secret.
fn generate_secrets() -> anyhow::Result<(String, [u8; QR_SECRET_LEN])> {
    const CODES: u32 = 1_000_000;
    let rng = SystemRandom::new();
    let fill = |bytes: &mut [u8]| rng.fill(bytes).map_err(|_| anyhow::anyhow!("system randomness unavailable"));
    let code = loop {
        let mut bytes = [0; 4];
        fill(&mut bytes)?;
        let value = u32::from_be_bytes(bytes);
        if value < u32::MAX - u32::MAX % CODES {
            break value % CODES;
        }
    };
    let mut qr = [0; QR_SECRET_LEN];
    fill(&mut qr)?;
    Ok((format!("{code:06}"), qr))
}

/// True when turning `kind` off on the phone drops the radio `session` runs on.
fn cuts_link(session: &SessionHandle, kind: SystemCommandKind) -> bool {
    match kind {
        SystemCommandKind::SetWifi => transport(session) == "wifi",
        SystemCommandKind::SetBluetooth => transport(session) == "bluetooth",
        _ => false,
    }
}

/// What carries `session`, as D-Bus names it: "bluetooth", "usb" or "wifi" (any IP path, and the relay).
fn transport(session: &SessionHandle) -> &'static str {
    match session.link_kind() {
        Some(LinkKind::Usb) => "usb",
        _ if session.on_bluetooth() => "bluetooth",
        _ => "wifi",
    }
}
