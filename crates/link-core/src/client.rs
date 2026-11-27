//! The phone's actor over [`Phone`]: its live sessions, presence with reconnection, file transfers, and the events an
//! app shows. Shared by the headless phone and the Android bindings.

mod handle;
mod notify;
mod redial;
mod sessions;
mod upgrade;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::time::Duration;

use link_proto::CloseCode;
use link_proto::message::{
    CALL_APP, CALLS, CAN_CONTROL, CLIP_CLOCK, CLIP_ITEMS, Call, CameraRequest, Codec, FsRefusal, GROUP_CHANNEL,
    HEARTBEAT, HotspotEnd, LATE_HASH, MAX_PUNCH_ADDRESSES, MAX_SNAPSHOT, MESSAGES, MIRROR_STREAMS, Message,
    MessagesRefusal, MirrorRequest, MirrorStarted, MirrorStop, NOTIFICATION_REPLIES, NOTIFICATION_SYNC, NetworkKind,
    NotificationDigest, NotificationPosted, NotificationRemoved, NotificationSnapshot, PRESENTING, PhotoRefusal, Punch,
    SCREEN_SHAPE, SHARE_OUTBOX, SHARING, STATUS_SIGNAL, STATUS_STORAGE, STATUS_TOGGLES, SYSTEM_CONTROL, ScreenShape,
    Share, Sharing, Status, SystemCommand, SystemResult, TEXTS_MMS, TransferId, WIFI_DIRECT_JOIN,
};
use link_proto::session::Role;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::bluetooth::{BLUETOOTH_FILE_LIMIT, BLUETOOTH_UPGRADE_ABOVE};
use crate::browse;
use crate::camera::{self, Camera};
use crate::hotspot::{self, UPGRADE_TIMEOUT};
use crate::identity::DeviceId;
use crate::inbox::Inbox;
use crate::link_state::LinkState;
use crate::messages::{self, Messages};
use crate::mirror::MirrorSender;
use crate::net;
use crate::outbox::{self, Outbox, OutboxActor, Seen, Sent};
use crate::path::{Hop, LinkId};
use crate::phone::{self, PairTarget, Phone};
use crate::reach::{self, Reached, Via};
use crate::relay::Relay;
use crate::session::{self, Route, SessionEvent, SessionHandle};
use crate::slot::{self, Effect, Kind, Slot};
use crate::store::{Feature, Grants, Peer, feature_of};
use crate::tls::ServerPin;
use crate::transfer::{self, ClipPeer, LocalClip, Source, TransferActor, TransferEvent, TransferHandle};
use crate::transport::Dialer;
use crate::{Error, close_code_for};

const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// The least time between two status messages that change no control field (battery, signal, storage).
const STATUS_EVERY: Duration = Duration::from_secs(10);
/// A change to a control field (volume, ringer, DND, lock, a toggle) goes out this soon, with any other change since.
const CONTROL_EVERY: Duration = Duration::from_millis(150);
/// How often a session on Bluetooth looks for an IP path to move to.
const PROBE_EVERY: Duration = Duration::from_secs(30);
/// Hop counts that pass the phone's own NAT but end before the desktop's; the path's length is unknown.
const OPEN_TTLS: [u8; 3] = [2, 3, 4];

#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// A desktop asked to mirror while its Screen switch is off; the app asks the user, then allows or refuses.
    MirrorBlocked {
        from: DeviceId,
    },
    Connected {
        desktop: Peer,
        /// None over Bluetooth.
        addr: Option<SocketAddr>,
        via: Via,
        resumed: bool,
    },
    Disconnected {
        id: DeviceId,
        reason: String,
    },
    /// A connected desktop's heartbeats went late (`live` false) or came back.
    Link {
        id: DeviceId,
        live: bool,
    },
    Received {
        from: DeviceId,
        share: Share,
    },
    /// The desktop unpaired this phone, which has forgotten it.
    Unpaired {
        id: DeviceId,
    },
    /// The desktop disconnected this phone and refuses it until the user connects it there.
    Paused {
        id: DeviceId,
    },
    /// Offers, progress, and results of file transfers in both directions.
    Transfer(TransferEvent),
    /// A feature message from a desktop whose switch for that feature is on.
    Message {
        from: DeviceId,
        message: Message,
    },
    /// A command from a desktop that granted this phone "Control system"; answer `id` with a `system-result`.
    SystemCommand {
        from: DeviceId,
        id: u64,
        command: SystemCommand,
    },
}

/// What the phone shows for a paired desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopLink {
    Live,
    /// A session is open but the desktop's heartbeats are late.
    Reconnecting,
    Offline,
}

#[derive(Debug, Clone)]
pub struct DesktopState {
    pub peer: Peer,
    pub link: DesktopLink,
    /// The live session runs over Bluetooth.
    pub bluetooth: bool,
    /// What the desktop's last `mirror-request` said its viewer decodes besides H.264, best first.
    pub mirror_codecs: Vec<Codec>,
}

enum Command {
    Keep {
        id: DeviceId,
        keep: bool,
    },
    Mirroring {
        id: DeviceId,
        on: bool,
    },
    SetStatus {
        status: Status,
    },
    Pair {
        target: PairTarget,
        reply: oneshot::Sender<Result<Peer, Error>>,
    },
    Session {
        id: DeviceId,
        reply: oneshot::Sender<Result<SessionHandle, Error>>,
    },
    /// The session already open to `id`, if any; unlike [`Command::Session`], it never dials.
    Live {
        id: DeviceId,
        reply: oneshot::Sender<Option<SessionHandle>>,
    },
    SetPresent {
        present: bool,
    },
    SetScreenOn {
        on: bool,
    },
    Forget {
        id: DeviceId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Desktops {
        reply: oneshot::Sender<Vec<DesktopState>>,
    },
    SetSharing {
        id: DeviceId,
        feature: Feature,
        on: bool,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    SetBrowseRoots {
        roots: browse::Roots,
    },
    SetMessages {
        messages: Option<Arc<dyn Messages>>,
    },
    SetCamera {
        camera: Option<Arc<dyn Camera>>,
    },
    /// `contacts-changed` or `texts-changed`, to desktops with the messages switch on.
    MessagesChanged(Message),
    /// Moves `id`'s Bluetooth session to this phone's hotspot; answered once it runs over IP, or with why not.
    Upgrade {
        id: DeviceId,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    /// A `notification-posted` or `notification-removed`: kept for the snapshot on connect, then broadcast.
    Notify {
        message: Message,
        reply: oneshot::Sender<usize>,
    },
    /// Every notification the phone shows, replacing what it held; sent to the connected desktops as on connect.
    SyncNotifications {
        notifications: Vec<NotificationPosted>,
    },
    /// The live sessions of desktops whose switch for `feature` is on, each with `message` as that desktop takes it.
    Sessions {
        feature: Feature,
        message: Message,
        reply: oneshot::Sender<Vec<(SessionHandle, Message)>>,
    },
}

/// How long unpairing waits to tell the desktop before it forgets it anyway.
const UNPAIR_TELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The handle apps hold; every method is answered by the [`ClientActor`].
#[derive(Clone)]
pub struct Client {
    commands: mpsc::Sender<Command>,
    transfers: TransferHandle,
    outbox: Outbox,
}

pub struct ClientActor {
    phone: Phone,
    commands: mpsc::Receiver<Command>,
    events: mpsc::UnboundedSender<ClientEvent>,
    sessions: HashMap<DeviceId, SessionHandle>,
    running: JoinSet<Ended>,
    session_events_tx: mpsc::Sender<SessionEvent>,
    session_events: mpsc::Receiver<SessionEvent>,
    transfers: TransferHandle,
    transfer_actor: Option<TransferActor>,
    outbox: Outbox,
    outbox_actor: Option<OutboxActor>,
    /// Share items already applied, so a resend after a lost ack is applied once.
    seen: Seen,
    transfer_events: mpsc::UnboundedReceiver<TransferEvent>,
    present: bool,
    /// False until the app reports it: an unused network then stops after the shorter idle time.
    screen_on: bool,
    /// Desktops with an open transfer, kept connected like a present phone until it ends.
    busy: HashSet<DeviceId>,
    /// Desktops whose last `capabilities` this session said they granted this phone "Control system".
    controlled_by: HashSet<DeviceId>,
    /// The status each desktop last got, as it took it; an equal one is not sent again.
    status_to: HashMap<DeviceId, Message>,
    /// Desktops a transfer is being started with, kept like busy ones until the transfer actor reports them.
    keeping: HashSet<DeviceId>,
    /// Desktops this phone's screen streams to, kept like busy ones, and their hotspot with them.
    mirroring: HashSet<DeviceId>,
    /// By desktop: [`DesktopState::mirror_codecs`].
    mirror_codecs: HashMap<DeviceId, Vec<Codec>>,
    retries: HashMap<DeviceId, Retry>,
    status: Option<Status>,
    /// The status last sent, when, and whether a newer one waits for [`STATUS_EVERY`] to pass.
    status_sent: Option<Instant>,
    /// The status last sent, to tell a change of control fields only.
    status_last: Option<Status>,
    status_pending: bool,
    /// When to probe the desktops whose live session runs over Bluetooth for an IP path, every [`PROBE_EVERY`].
    next_probe: Option<Instant>,
    /// Each IP session's standby link (LE, or the relay it started on), with the session's stable id and the link's
    /// path; losing Wi-Fi or the desktop's heartbeat moves the session onto it.
    standby: HashMap<DeviceId, (usize, LinkId, Via)>,
    /// Standby links being opened: an L2CAP channel (`Via::Bluetooth`) or a proven USB stream (`Via::Usb`).
    standbys: JoinSet<(DeviceId, usize, Via, Result<OwnedFd, Error>)>,
    probes: JoinSet<(DeviceId, Result<Reached, Error>)>,
    /// Ends when a session's USB link ends (the cable came out), with the session's stable id.
    usb_ends: JoinSet<(DeviceId, usize)>,
    /// The probe running for each desktop: a new one stops it, since two dials to one address contend for its packets.
    probing: HashMap<DeviceId, tokio::task::AbortHandle>,
    /// The relay connection under each session that started through the relay; it tells the phone's public address.
    relayed: HashMap<DeviceId, Relay>,
    /// Probe rounds so far; the NAT-opening packets' TTL cycles through [`OPEN_TTLS`] by round.
    probe_round: usize,
    /// Dials off the actor, at most one per desktop: a desktop that does not answer takes seconds to give up on.
    dials: JoinSet<(DeviceId, Result<phone::Opened, Error>)>,
    dialing: HashMap<DeviceId, redial::Dial>,
    /// The phone's group or hotspot, for one desktop at a time.
    slot: Slot,
    /// Desktops whose live session runs over the phone's hotspot.
    on_hotspot: HashSet<DeviceId>,
    /// Provider starts, each ending once its credentials went to the desktop.
    starting: JoinSet<(DeviceId, Kind, Result<(), String>)>,
    upgrades: HashMap<DeviceId, Vec<oneshot::Sender<Result<(), Error>>>>,
    /// Desktops whose Bluetooth session already failed to move to a Wi-Fi Direct group.
    wifi_direct_failed: HashSet<DeviceId>,
    /// Browse and contacts requests being answered, at most [`browse::MAX_IN_FLIGHT`].
    browsing: JoinSet<()>,
    messages: Option<Arc<dyn Messages>>,
    camera: Option<Arc<dyn Camera>>,
    /// The phone's notifications on show, oldest first; None until the app first posts, removes, or syncs.
    notifications: Option<Vec<NotificationPosted>>,
}

struct Ended {
    id: DeviceId,
    stable_id: usize,
    result: Result<(), Error>,
}

struct Retry {
    at: Instant,
    delay: Duration,
}

/// The caller runs the actor in a task it owns and reads the events. Received files go to `inbox`.
pub fn client(phone: Phone, inbox: Inbox) -> (Client, ClientActor, mpsc::UnboundedReceiver<ClientEvent>) {
    let (commands_tx, commands) = mpsc::channel(16);
    // Unbounded: the app may call the actor while it handles an event, so a full queue would block both forever.
    let (events, events_rx) = mpsc::unbounded_channel();
    let (session_events_tx, session_events) = mpsc::channel(16);
    let (transfers, transfer_actor, transfer_events) = transfer::transfers(inbox);
    let (outbox, outbox_actor) = outbox::outbox(phone.store_path().with_file_name("outbox.json"));
    let actor = ClientActor {
        phone,
        commands,
        events,
        sessions: HashMap::new(),
        running: JoinSet::new(),
        session_events_tx,
        session_events,
        transfers: transfers.clone(),
        transfer_actor: Some(transfer_actor),
        outbox: outbox.clone(),
        outbox_actor: Some(outbox_actor),
        seen: Seen::default(),
        transfer_events,
        present: false,
        screen_on: false,
        busy: HashSet::new(),
        controlled_by: HashSet::new(),
        status_to: HashMap::new(),
        keeping: HashSet::new(),
        mirroring: HashSet::new(),
        mirror_codecs: HashMap::new(),
        retries: HashMap::new(),
        status: None,
        status_sent: None,
        status_last: None,
        status_pending: false,
        next_probe: None,
        standby: HashMap::new(),
        standbys: JoinSet::new(),
        usb_ends: JoinSet::new(),
        probes: JoinSet::new(),
        probing: HashMap::new(),
        relayed: HashMap::new(),
        probe_round: 0,
        dials: JoinSet::new(),
        dialing: HashMap::new(),
        slot: Slot::Idle,
        on_hotspot: HashSet::new(),
        starting: JoinSet::new(),
        upgrades: HashMap::new(),
        wifi_direct_failed: HashSet::new(),
        browsing: JoinSet::new(),
        messages: None,
        camera: None,
        notifications: None,
    };
    (Client { commands: commands_tx, transfers, outbox }, actor, events_rx)
}

impl ClientActor {
    /// Runs until every [`Client`] is dropped, then closes the sessions and waits for the closes to be sent.
    pub async fn run(mut self) {
        let (Some(transfer_actor), Some(outbox_actor)) = (self.transfer_actor.take(), self.outbox_actor.take()) else {
            return;
        };
        tokio::select! {
            () = self.serve() => {}
            () = transfer_actor.run() => log::error!("the transfer actor stopped"),
            () = outbox_actor.run() => log::error!("the outbox stopped"),
        }
        for handle in self.sessions.values() {
            handle.close(CloseCode::Done);
        }
        self.phone.finish().await;
    }

    async fn serve(&mut self) {
        loop {
            let next_retry = self.retries.values().map(|retry| retry.at).min();
            let every = self.status_every();
            let next_status = self.status_pending.then(|| self.status_sent.map(|sent| sent + every)).flatten();
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle(command).await,
                    None => return,
                },
                Some(joined) = self.running.join_next() => match joined {
                    Ok(ended) => self.ended(ended).await,
                    Err(join) => log::warn!("session task: {join}"),
                },
                Some(event) = self.session_events.recv() => self.relay(event).await,
                Some(event) = self.transfer_events.recv() => self.on_transfer(event).await,
                () = sleep_until(next_retry) => self.retry_due(),
                () = sleep_until(next_status) => self.send_status().await,
                () = sleep_until(self.next_probe) => self.probe(),
                Some(joined) = self.standbys.join_next() => match joined {
                    Ok((id, stable_id, via, opened)) => self.standby_opened(id, stable_id, via, opened).await,
                    Err(join) => log::warn!("standby link task: {join}"),
                },
                Some(_) = self.browsing.join_next() => {}
                () = sleep_until(self.slot.deadline()) => self.slot_event(slot::Event::Tick).await,
                Some(started) = self.starting.join_next() => match started {
                    Ok((id, kind, Err(reason))) => {
                        log::info!("{id}: no {kind:?}: {reason}");
                        self.slot_event(slot::Event::Failed { peer: id, kind, reason }).await;
                    }
                    // Started after the slot moved on, so the stop it got may have run first.
                    Ok((_, kind, Ok(()))) if self.slot.kind() != Some(kind) => self.stop_provider(kind),
                    Ok(_) => {}
                    Err(join) => log::warn!("network start task: {join}"),
                },
                Some(joined) = self.dials.join_next() => match joined {
                    Ok((id, opened)) => self.dialed(id, opened).await,
                    Err(join) => {
                        log::warn!("dial task: {join}");
                        self.dial_failed(join.id());
                    }
                },
                Some(Ok((id, stable_id))) = self.usb_ends.join_next() => self.usb_ended(id, stable_id).await,
                Some(joined) = self.probes.join_next() => match joined {
                    Ok((id, reached)) => self.probed(id, reached).await,
                    Err(join) if join.is_cancelled() => {}
                    Err(join) => log::warn!("probe task: {join}"),
                },
            }
        }
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Pair { target, reply } => {
                let result = self.pair(target).await;
                drop(reply.send(result));
            }
            Command::Session { id, reply } => self.session_for(&id, reply),
            Command::Live { id, reply } => {
                drop(reply.send(self.sessions.get(&id).filter(|handle| handle.is_live()).cloned()));
            }
            Command::SetPresent { present } => self.set_present(present),
            Command::SetScreenOn { on } => {
                log::info!("screen on: {on}");
                self.screen_on = on;
                self.slot_event(slot::Event::IdleChanged).await;
            }
            Command::Forget { id, reply } => {
                self.outbox.forget(id.clone());
                self.retries.remove(&id);
                self.controlled_by.remove(&id);
                self.status_to.remove(&id);
                self.standby.remove(&id);
                self.relayed.remove(&id);
                if let Some(handle) = self.sessions.remove(&id) {
                    handle.close(CloseCode::Done);
                }
                drop(reply.send(self.phone.forget(&id)));
            }
            Command::Desktops { reply } => drop(reply.send(self.desktops())),
            Command::SetStatus { status } => {
                let before = self.status.map(|status| status.network);
                if before != Some(status.network) {
                    self.network_changed(before, status.network).await;
                }
                if self.status != Some(status) {
                    self.status = Some(status);
                    self.status_pending = true;
                    if self.status_sent.is_none_or(|sent| sent.elapsed() >= self.status_every()) {
                        self.send_status().await;
                    }
                }
            }
            Command::Keep { id, keep } => {
                if keep {
                    self.keeping.insert(id);
                } else {
                    self.keeping.remove(&id);
                }
            }
            Command::Mirroring { id, on } => self.set_mirroring(&id, on),
            Command::SetSharing { id, feature, on, reply } => {
                drop(reply.send(self.set_sharing(&id, feature, on).await));
            }
            Command::SetBrowseRoots { roots } => self.phone.set_browse_roots(roots),
            Command::SetMessages { messages } => {
                self.messages = messages;
                self.messages_changed(None, &[Message::ContactsChanged, Message::TextsChanged, Message::CallsChanged])
                    .await;
            }
            Command::SetCamera { camera } => self.camera = camera,
            Command::MessagesChanged(changed) => self.messages_changed(None, &[changed]).await,
            Command::Upgrade { id, reply } => {
                self.upgrades.entry(id.clone()).or_default().push(reply);
                self.start_hotspot(id).await;
            }
            Command::Sessions { feature, message, reply } => {
                let sessions = self
                    .phone
                    .desktops()
                    .iter()
                    .filter(|peer| peer.grants.allows(feature))
                    .filter_map(|peer| {
                        let handle = self.sessions.get(&peer.id).filter(|handle| handle.is_live())?.clone();
                        let message = match &message {
                            Message::Call(call) if !peer.has(CALL_APP) => {
                                Message::Call(Call { app: None, ..call.clone() })
                            }
                            message => message.clone(),
                        };
                        Some((handle, message))
                    })
                    .collect();
                drop(reply.send(sessions));
            }
            Command::Notify { message, reply } => {
                let _ = reply.send(self.notify(message).await);
            }
            Command::SyncNotifications { notifications } => self.sync_notifications(notifications).await,
        }
    }

    async fn on_transfer(&mut self, event: TransferEvent) {
        if let TransferEvent::Mirror { from, stream } = &event
            && !self.mirroring.contains(from)
        {
            log::info!("{from}: refusing a mirror stream nobody asked for");
            if let Some(mut receiver) = stream.take() {
                receiver.stop();
            }
            return;
        }
        if let TransferEvent::Speaker { from, stream } = &event
            && !self.phone.desktops().iter().any(|peer| peer.id == *from && peer.grants.allows(Feature::Speaker))
        {
            log::info!("{from}: refusing sound for the speaker; its switch is off");
            if let Some(mut receiver) = stream.take() {
                receiver.stop();
            }
            return;
        }
        if let TransferEvent::Mic { from, stream } | TransferEvent::Sound { from, stream } = &event {
            log::info!("{from}: refusing a phone's sound stream; only desktops take one");
            if let Some(mut receiver) = stream.take() {
                receiver.stop();
            }
            return;
        }
        if let TransferEvent::Busy { peer, busy } = &event {
            drop(self.slot.apply(slot::Event::Busy { peer: peer.clone(), busy: *busy }, Instant::now(), self.idle()));
            if *busy {
                self.busy.insert(peer.clone());
            } else {
                self.busy.remove(peer);
                if !self.present
                    && let Some(handle) = self.sessions.get(peer)
                {
                    handle.close(CloseCode::Done);
                }
            }
            return;
        }
        let allows = |id: &DeviceId, feature| {
            self.phone.desktops().iter().any(|peer| peer.id == *id && peer.grants.allows(feature))
        };
        match &event {
            TransferEvent::ClipOffered { from, .. } if !allows(from, Feature::Clipboard) => {
                log::info!("{from}: dropped a clipboard offer its switch does not allow");
                return;
            }
            TransferEvent::Offered { id, from, .. } if !allows(from, Feature::Files) => {
                log::info!("{from}: declining transfer {id}: its files switch is off");
                if let Err(error) = self.transfers.decide(*id, false).await {
                    log::warn!("declining {id}: {error}");
                }
                return;
            }
            _ => {}
        }
        self.emit(ClientEvent::Transfer(event));
    }

    fn idle(&self) -> std::time::Duration {
        if self.screen_on { hotspot::IDLE_SCREEN_ON } else { hotspot::IDLE_SCREEN_OFF }
    }

    fn set_present(&mut self, present: bool) {
        self.present = present;
        if !present {
            let (busy, keeping) = (&self.busy, &self.keeping);
            self.retries.retain(|id, _| busy.contains(id) || keeping.contains(id));
            for (id, handle) in &self.sessions {
                if !self.is_busy(id) {
                    handle.close(CloseCode::Done);
                }
            }
            return;
        }
        for handle in self.sessions.values() {
            handle.wake();
        }
        let now = Instant::now();
        for peer in self.phone.desktops() {
            if !self.sessions.get(&peer.id).is_some_and(SessionHandle::is_live) {
                self.retries.entry(peer.id.clone()).or_insert(Retry { at: now, delay: FIRST_RETRY });
            }
        }
    }

    fn status_every(&self) -> Duration {
        status_every(self.status_last, self.status)
    }

    /// Sends the latest status to every live session.
    async fn send_status(&mut self) {
        self.status_pending = false;
        self.status_sent = Some(Instant::now());
        let Some(status) = self.status else { return };
        self.status_last = Some(status);
        for (id, handle) in self.sessions.iter().filter(|(_, handle)| handle.is_live()) {
            // A desktop without some fields would get the same status again, against its rate limit.
            let message = self.status_message(id, status);
            if self.status_to.get(id) == Some(&message) {
                continue;
            }
            self.status_to.insert(id.clone(), message.clone());
            if let Err(error) = handle.send(message).await {
                log::info!("{id}: sending the status: {error}");
            }
        }
    }

    /// The status as `id` takes it: without the fields of a feature the desktop lacks.
    fn status_message(&self, id: &DeviceId, status: Status) -> Message {
        let has = |feature| self.phone.desktops().iter().any(|peer| peer.id == *id && peer.has(feature));
        let status = if has(SYSTEM_CONTROL) { status } else { status.without_control() };
        Message::Status(Status {
            signal: status.signal.filter(|_| has(STATUS_SIGNAL)),
            storage: status.storage.filter(|_| has(STATUS_STORAGE)),
            wifi: status.wifi.filter(|_| has(STATUS_TOGGLES)),
            bluetooth: status.bluetooth.filter(|_| has(STATUS_TOGGLES)),
            torch: status.torch.filter(|_| has(STATUS_TOGGLES)),
            notifications_off: status.notifications_off && has(NOTIFICATION_SYNC),
            ..status
        })
    }

    fn is_busy(&self, id: &DeviceId) -> bool {
        self.busy.contains(id) || self.keeping.contains(id) || self.mirroring.contains(id)
    }

    /// The hotspot stays up while the screen streams over it, and idles out after. Mirroring lives only as long as
    /// its session: an `on` that arrives after [`Self::ended`] removed the session is dropped.
    fn set_mirroring(&mut self, id: &DeviceId, on: bool) {
        if on {
            if !self.sessions.contains_key(id) {
                return;
            }
            self.mirroring.insert(id.clone());
        } else {
            self.mirroring.remove(id);
        }
        let busy = self.is_busy(id);
        drop(self.slot.apply(slot::Event::Busy { peer: id.clone(), busy }, Instant::now(), self.idle()));
    }

    /// Whether a lost session to `id` should be redialled.
    fn wants(&self, id: &DeviceId) -> bool {
        (self.present || self.is_busy(id)) && self.phone.desktops().iter().any(|peer| peer.id == *id)
    }

    #[expect(clippy::too_many_lines, reason = "one arm per message the client answers itself")]
    async fn relay(&mut self, event: SessionEvent) {
        if let SessionEvent::Message { from, message: Message::MirrorStop(_) | Message::RemoteStop(_) } = &event {
            self.set_mirroring(from, false);
        }
        match event {
            SessionEvent::Message { from, message: Message::NotificationPull(pull) } => {
                let Some(handle) = self.sessions.get(&from).cloned() else { return };
                let Some(takes) = self
                    .phone
                    .desktops()
                    .iter()
                    .find(|peer| peer.id == from && peer.has(NOTIFICATION_SYNC))
                    .map(Takes::of)
                else {
                    return log::info!("{from}: a notification pull without the notifications grant");
                };
                let held = self.notifications.as_ref();
                for posted in
                    pull.ids.iter().filter_map(|id| held.and_then(|held| held.iter().find(|posted| posted.id == *id)))
                {
                    if let Err(error) =
                        handle.send(for_desktop(Message::NotificationPosted(posted.clone()), takes)).await
                    {
                        return log::info!("{from}: answering a notification pull: {error}");
                    }
                }
            }
            SessionEvent::Message { from, message: Message::HotspotRequest } => self.start_hotspot(from).await,
            SessionEvent::Message {
                from,
                message:
                    request @ (Message::FsList(_)
                    | Message::FsStat(_)
                    | Message::FsRead(_)
                    | Message::FsThumb(_)
                    | Message::PhotosRecent(_)
                    | Message::FsWrite(_)
                    | Message::FsDelete(_)),
            } => {
                self.browse(&from, request).await;
            }
            SessionEvent::Message {
                from,
                message:
                    request @ (Message::ContactsList(_)
                    | Message::TextsList(_)
                    | Message::TextPartThumb(_)
                    | Message::TextSend(_)
                    | Message::CallsList(_)
                    | Message::Dial(_)),
            } => {
                if let Some(request) = messages::Request::of(request) {
                    self.messages_request(&from, request).await;
                }
            }
            SessionEvent::Message { from, message: Message::PhotoRequest(request) } => {
                self.photo(&from, request.req).await;
            }
            SessionEvent::Message { from, message: Message::HotspotJoined(joined) } => {
                self.hotspot_joined(from, &joined);
            }
            SessionEvent::Message { from, message: Message::UsbReady } => self.usb_ready(from),
            SessionEvent::Message { from, message: Message::MirrorRequest(_) }
                if !self.phone.desktops().iter().any(|peer| peer.id == from && peer.grants.screen) =>
            {
                log::info!("{from}: refusing to mirror: the screen switch is off");
                if let Some(handle) = self.sessions.get(&from) {
                    let reason = Some("the phone's screen switch for this desktop is off".to_owned());
                    if let Err(error) = handle.send(Message::MirrorStop(MirrorStop { reason })).await {
                        log::info!("{from}: refusing to mirror: {error}");
                    }
                }
            }
            SessionEvent::Message { from, message: Message::HotspotEnd(end) } => {
                let what = match self.slot.kind() {
                    Some(Kind::Group) => "Wi-Fi Direct group",
                    Some(Kind::Hotspot) | None => "hotspot",
                };
                log::info!("{from}: the desktop ended the {what}: {:?}", end.reason);
                let reason = end.reason.unwrap_or_else(|| "the desktop left it".to_owned());
                self.slot_event(slot::Event::DesktopEnded { peer: from, reason }).await;
            }
            SessionEvent::Received { from, share } => {
                if self.seen.first(&from, &share) {
                    self.emit(ClientEvent::Received { from, share });
                } else {
                    log::info!("{from}: dropped a share it sent again");
                }
            }
            SessionEvent::Message {
                from,
                message:
                    message @ (Message::Ping(_)
                    | Message::Capabilities(_)
                    | Message::DesktopStatus(_)
                    | Message::SystemResult(_)
                    | Message::RemoteStarted(_)
                    | Message::RemoteStop(_)),
            } => {
                if let Message::Capabilities(capabilities) = &message {
                    if capabilities.can.iter().any(|can| can == CAN_CONTROL) {
                        self.controlled_by.insert(from.clone());
                    } else {
                        self.controlled_by.remove(&from);
                    }
                }
                self.emit(ClientEvent::Message { from, message });
            }
            SessionEvent::SystemCommand { from, id, command } => self.on_system_command(from, id, command).await,
            SessionEvent::Message { from, message } => {
                // A stop only ends what runs, so no switch holds it back.
                let allowed = is_stop(&message)
                    || feature_of(&message).is_some_and(|feature| {
                        self.phone.desktops().iter().any(|peer| peer.id == from && peer.grants.allows(feature))
                    });
                if allowed {
                    match &message {
                        Message::MirrorRequest(MirrorRequest { codecs })
                        | Message::CameraRequest(CameraRequest { codecs, .. }) => {
                            self.mirror_codecs.insert(from.clone(), codecs.clone());
                        }
                        _ => {}
                    }
                    self.emit(ClientEvent::Message { from, message });
                } else if matches!(message, Message::MirrorRequest(_)) {
                    self.emit(ClientEvent::MirrorBlocked { from });
                } else {
                    log::info!("{from}: dropped a {} its switch does not allow", message.kind());
                }
            }
            // Closed is reported as Disconnected.
            SessionEvent::Link { from, state } if state != LinkState::Closed => {
                let live = matches!(state, LinkState::Live { .. });
                // A Bluetooth session moves up only through a probe, which proves the IP path first.
                if !live && let Some(link) = self.standby_for(&from) {
                    self.move_to_standby(from.clone(), link, "the desktop's heartbeat is overdue").await;
                }
                if live && let Some(handle) = self.sessions.get(&from) {
                    let items = self.phone.desktops().iter().any(|peer| peer.id == from && peer.has(SHARE_OUTBOX));
                    self.outbox.live(from.clone(), handle.clone(), items);
                }
                self.emit(ClientEvent::Link { id: from, live });
            }
            SessionEvent::Unpaired { .. }
            | SessionEvent::Status { .. }
            | SessionEvent::Link { .. }
            | SessionEvent::Path { .. } => {}
        }
    }

    /// Hands a desktop's command to the app, or refuses it when that desktop has not granted "Control system".
    async fn on_system_command(&mut self, from: DeviceId, id: u64, command: SystemCommand) {
        let error = if !command.command.to_phone() {
            Some("not a command for a phone")
        } else if !self.controlled_by.contains(&from) {
            Some("Control system is off for this phone on the desktop")
        } else {
            None
        };
        match error {
            None => self.emit(ClientEvent::SystemCommand { from, id, command }),
            Some(error) => {
                log::info!("{from}: refused a {:?}: {error}", command.command);
                if let Some(session) = self.sessions.get(&from) {
                    let result = SystemResult { command: id, error: Some(error.to_owned()) };
                    drop(session.send(Message::SystemResult(result)).await);
                }
            }
        }
    }

    fn desktops(&self) -> Vec<DesktopState> {
        let link = |id: &DeviceId| match self.sessions.get(id).map(SessionHandle::link) {
            Some(LinkState::Live { .. }) => DesktopLink::Live,
            Some(LinkState::Degraded { .. }) => DesktopLink::Reconnecting,
            Some(LinkState::Closed) | None => DesktopLink::Offline,
        };
        self.phone
            .desktops()
            .iter()
            .map(|peer| DesktopState {
                peer: peer.clone(),
                link: link(&peer.id),
                bluetooth: link(&peer.id) == DesktopLink::Live && self.on_bluetooth(&peer.id),
                mirror_codecs: self.mirror_codecs.get(&peer.id).cloned().unwrap_or_default(),
            })
            .collect()
    }

    fn emit(&self, event: ClientEvent) {
        if self.events.send(event).is_err() {
            log::debug!("nobody reads client events");
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// The phone's addresses its dials leave from, IPv4 first, for the desktop to punch its firewall toward.
/// The `notification-posted` fields a desktop takes; it closes a session on the others.
#[derive(Debug, Clone, Copy)]
struct Takes {
    /// [`NOTIFICATION_SYNC`]: `alert_once`, snapshots and pulls.
    sync: bool,
    /// [`NOTIFICATION_REPLIES`]: `replies`.
    replies: bool,
}

impl Takes {
    fn of(peer: &Peer) -> Self {
        Self { sync: peer.has(NOTIFICATION_SYNC), replies: peer.has(NOTIFICATION_REPLIES) }
    }
}

/// Drops the fields the desktop does not take.
fn for_desktop(message: Message, takes: Takes) -> Message {
    match message {
        Message::NotificationPosted(posted) => Message::NotificationPosted(NotificationPosted {
            alert_once: posted.alert_once && takes.sync,
            replies: if takes.replies { posted.replies } else { Vec::new() },
            ..posted
        }),
        message => message,
    }
}

/// The digest of what [`for_desktop`] sends, as the desktop computes it.
fn shown_digest(posted: &NotificationPosted, takes: Takes) -> u64 {
    if takes.replies || posted.replies.is_empty() {
        posted.digest()
    } else {
        NotificationPosted { replies: Vec::new(), ..posted.clone() }.digest()
    }
}

/// Asks the desktop to punch toward this phone's addresses: `observed`, the relay's view of its NAT mapping, first.
fn punch(dialer: &Dialer, observed: Option<SocketAddr>) -> Option<Message> {
    let mut addresses = net::local_addresses(dialer.port()?);
    addresses.sort_by_key(SocketAddr::is_ipv6);
    addresses.splice(0..0, observed);
    addresses.truncate(MAX_PUNCH_ADDRESSES);
    let addresses = addresses.iter().map(ToString::to_string).collect::<Vec<_>>();
    (!addresses.is_empty()).then_some(Message::Punch(Punch { addresses }))
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// How long after the status `last` sent the status `now` may go out.
fn status_every(last: Option<Status>, now: Option<Status>) -> Duration {
    let control = |status: Status| {
        (status.media_volume, status.ringer, status.dnd, status.locked, status.wifi, status.bluetooth, status.torch)
    };
    match (last, now) {
        (Some(last), Some(now)) if control(last) != control(now) || last.without_control() == now.without_control() => {
            CONTROL_EVERY
        }
        _ => STATUS_EVERY,
    }
}

fn is_stop(message: &Message) -> bool {
    matches!(message, Message::MirrorStop(_) | Message::SoundStop | Message::MicStop | Message::CameraStop(_))
}

/// What a desktop's stop of `feature` looks like: the phone acts out these when that switch turns off.
fn stops_of(feature: Feature) -> Vec<Message> {
    match feature {
        Feature::Screen => vec![Message::MirrorStop(MirrorStop { reason: None }), Message::SoundStop],
        Feature::Mic => vec![Message::MicStop],
        Feature::Webcam => vec![Message::CameraStop(MirrorStop { reason: None })],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use link_proto::message::NotificationButton;

    use super::*;

    /// A torch turned on right after a battery change reaches the desktop at once, not with the next 10 s status.
    #[test]
    fn a_control_change_goes_out_soon_even_after_a_battery_change() {
        let sent = Status {
            battery: 80,
            charging: false,
            network: link_proto::message::NetworkKind::Wifi,
            signal: None,
            notifications_off: false,
            media_volume: Some(50),
            ringer: None,
            dnd: None,
            locked: None,
            storage: None,
            wifi: Some(true),
            bluetooth: Some(false),
            torch: Some(false),
        };
        let battery = Status { battery: 79, ..sent };
        assert_eq!(status_every(Some(sent), Some(battery)), STATUS_EVERY);
        assert_eq!(status_every(Some(sent), Some(Status { torch: Some(true), ..battery })), CONTROL_EVERY);
        assert_eq!(status_every(Some(sent), Some(Status { media_volume: Some(60), ..sent })), CONTROL_EVERY);
    }

    /// A `Mirroring { on }` that reaches the actor after its session ended leaves no mirroring behind, so the slot
    /// is not held busy (SM-156).
    #[tokio::test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    async fn mirroring_without_a_session_is_dropped() {
        let dir = std::env::temp_dir().join(format!("link-client-mirroring-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let phone = Phone::new(
            crate::identity::Identity::generate().expect("key"),
            dir.join("store.json"),
            "phone".to_owned(),
            None,
        )
        .expect("phone");
        let (_client, mut actor, _events) = client(phone, Inbox::new(dir.clone(), &dir).expect("inbox"));
        let id = crate::identity::Identity::generate().expect("key").device_id();
        actor.set_mirroring(&id, true);
        assert!(!actor.is_busy(&id));
        drop(std::fs::remove_dir_all(&dir));
    }

    /// With its switch off, a desktop's mic, sound or mirror stop still reaches the app, its requests do not, and
    /// turning a switch off stops what that switch covers.
    #[tokio::test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    async fn a_switch_turned_off_stops_its_capture_and_lets_stops_through() {
        let dir = std::env::temp_dir().join(format!("link-client-stops-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let desktop = crate::identity::Identity::generate().expect("key");
        let mut store = crate::store::Store::default();
        store.upsert(Peer::new(desktop.spki(), "desk".to_owned()));
        store.save(&dir.join("store.json")).expect("store");
        let phone = Phone::new(
            crate::identity::Identity::generate().expect("key"),
            dir.join("store.json"),
            "phone".to_owned(),
            None,
        )
        .expect("phone");
        let (_client, mut actor, mut events) = client(phone, Inbox::new(dir.clone(), &dir).expect("inbox"));
        let id = desktop.device_id();
        let mut emitted = || {
            let mut out = Vec::new();
            while let Ok(ClientEvent::Message { message, .. }) = events.try_recv() {
                out.push(message);
            }
            out
        };
        for (feature, stops) in [
            (Feature::Mic, vec![Message::MicStop]),
            (Feature::Webcam, vec![Message::CameraStop(MirrorStop { reason: None })]),
            (Feature::Screen, vec![Message::MirrorStop(MirrorStop { reason: None }), Message::SoundStop]),
        ] {
            actor.set_sharing(&id, feature, true).await.expect("on");
            assert!(emitted().is_empty(), "{feature:?} on stopped something");
            actor.set_sharing(&id, feature, false).await.expect("off");
            assert_eq!(emitted(), stops, "{feature:?} off");
            for stop in stops {
                actor.relay(SessionEvent::Message { from: id.clone(), message: stop.clone() }).await;
                assert_eq!(emitted(), vec![stop], "{feature:?} off");
            }
        }
        for request in [Message::MicRequest, Message::SoundRequest] {
            actor.relay(SessionEvent::Message { from: id.clone(), message: request.clone() }).await;
            assert!(emitted().is_empty(), "{request:?} passed a switch that is off");
        }
        drop(std::fs::remove_dir_all(&dir));
    }

    /// Blocks every Bluetooth open until the test releases it, and counts the opens per address.
    struct StuckBluetooth {
        opens: std::sync::Mutex<HashMap<String, usize>>,
        started: mpsc::UnboundedSender<()>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl crate::bluetooth::BluetoothOpener for StuckBluetooth {
        fn open(&self, address: &str, _pair: bool) -> std::io::Result<OwnedFd> {
            self.open_l2cap(address)
        }

        fn open_l2cap(&self, address: &str) -> std::io::Result<OwnedFd> {
            if let Ok(mut opens) = self.opens.lock() {
                *opens.entry(address.to_owned()).or_default() += 1;
            }
            self.started.send(()).unwrap_or_default();
            if let Ok(release) = self.release.lock() {
                // A closed channel releases too.
                release.recv().unwrap_or_default();
            }
            Err(std::io::ErrorKind::TimedOut.into())
        }
    }

    /// While dials to two desktops hang in Bluetooth, commands for a third answer at once; two commands for one
    /// hanging desktop share one dial and both get its result, and the other desktop's dial runs on its own.
    #[tokio::test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    async fn a_hanging_dial_blocks_no_other_command() {
        let dir = std::env::temp_dir().join(format!("link-client-dial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let key = || crate::identity::Identity::generate().expect("key");
        let (a, b, c) = (key(), key(), key());
        let mut store = crate::store::Store::default();
        store.upsert(Peer::new(a.spki(), "a".to_owned()));
        for (far, address) in [(&b, "AA:BB:CC:DD:EE:0B"), (&c, "AA:BB:CC:DD:EE:0C")] {
            let mut peer = Peer::new(far.spki(), "far".to_owned());
            peer.bluetooth = Some(address.to_owned());
            store.upsert(peer);
        }
        store.save(&dir.join("store.json")).expect("store");
        let mut phone = Phone::new(key(), dir.join("store.json"), "phone".to_owned(), None).expect("phone");
        let (started, mut opened) = mpsc::unbounded_channel();
        let (release, stuck) = std::sync::mpsc::channel();
        let bluetooth = Arc::new(StuckBluetooth {
            opens: std::sync::Mutex::default(),
            started,
            release: std::sync::Mutex::new(stuck),
        });
        phone.set_bluetooth(bluetooth.clone());
        let (client, actor, _events) = client(phone, Inbox::new(dir.clone(), &dir).expect("inbox"));
        tokio::spawn(actor.run());
        let dials: Vec<_> = [b.device_id(), b.device_id(), c.device_id()]
            .into_iter()
            .map(|id| {
                let client = client.clone();
                tokio::spawn(async move { client.connect(id).await })
            })
            .collect();
        for _ in 0..2 {
            tokio::time::timeout(Duration::from_secs(30), opened.recv())
                .await
                .expect("the two desktops did not both dial at once")
                .expect("open");
        }
        let quick = Duration::from_secs(1);
        tokio::time::timeout(quick, client.set_sharing(a.device_id(), Feature::Screen, false))
            .await
            .expect("set_sharing waited for another desktop's dial")
            .expect("set_sharing");
        tokio::time::timeout(quick, client.desktops()).await.expect("desktops waited").expect("desktops");
        // One dial per desktop opens LE, then RFCOMM.
        for _ in 0..4 {
            release.send(()).expect("release");
        }
        for dial in dials {
            let result = tokio::time::timeout(Duration::from_secs(30), dial).await.expect("dial ended").expect("join");
            assert!(result.is_err(), "a dial to an unreachable desktop succeeded");
        }
        let opens = bluetooth.opens.lock().expect("opens").clone();
        assert_eq!(opens.get("AA:BB:CC:DD:EE:0B"), Some(&2), "two commands for one desktop: {opens:?}");
        assert_eq!(opens.get("AA:BB:CC:DD:EE:0C"), Some(&2), "the other desktop: {opens:?}");
        drop(std::fs::remove_dir_all(&dir));
    }

    /// For every desktop feature mix, a snapshot's hash matches what the desktop computes over the post it gets, and
    /// the post carries no field the desktop does not take.
    #[test]
    fn notification_view_per_desktop() {
        for (alert_once, replies) in
            [(false, vec![]), (true, vec!["On my way".to_owned(), "Call you later".to_owned()])]
        {
            let posted = NotificationPosted {
                id: "a".to_owned(),
                app: "Messages".to_owned(),
                title: "Ann".to_owned(),
                text: "Dinner?".to_owned(),
                icon: None,
                actions: vec![NotificationButton { id: "0".to_owned(), label: "Reply".to_owned(), reply: true }],
                alert_once,
                replies,
            };
            for (sync, replies) in [(false, false), (false, true), (true, false), (true, true)] {
                let takes = Takes { sync, replies };
                let Message::NotificationPosted(sent) = for_desktop(Message::NotificationPosted(posted.clone()), takes)
                else {
                    unreachable!()
                };
                assert_eq!(shown_digest(&posted, takes), sent.digest(), "{takes:?}");
                assert!(sync || !sent.alert_once, "{takes:?}");
                assert!(replies || sent.replies.is_empty(), "{takes:?}");
                assert_eq!(sent.replies.len(), if replies { posted.replies.len() } else { 0 }, "{takes:?}");
            }
        }
    }
}
