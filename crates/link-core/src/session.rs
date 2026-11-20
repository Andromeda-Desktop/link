//! The session actor: one per connection, on both sides, after both hellos. It owns the control stream, delivers what
//! the peer shares, hands transfer messages and bulk streams to the transfer actor, and sends what its handle asks for.

use std::collections::HashMap;
use std::pin::pin;

use link_proto::CloseCode;
use link_proto::limit::Limits;
use link_proto::message::{
    Envelope, Heartbeat, HeartbeatAck, HotspotEnd, Message, OfferReply, RefuseReason, Share, ShareAck, Status,
    SystemCommand,
};
use link_proto::session::{Inbound, Role, SessionState};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::Error;
use crate::control::{Control, ControlReader, ControlWriter, STEP_TIMEOUT, Tap};
use crate::identity::DeviceId;
use crate::link_state::{ACTIVE_EVERY, ACTIVE_FOR, IDLE_EVERY, LinkEvent, LinkState, Liveness};
use crate::transfer::TransferHandle;
use crate::wire::Connection;

/// What a session hands to its owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Received {
        from: DeviceId,
        share: Share,
    },
    /// The phone unpaired itself; only a desktop's sessions report this.
    Unpaired {
        from: DeviceId,
    },
    /// A feature message that is not acknowledged, already checked for its direction.
    Message {
        from: DeviceId,
        message: Message,
    },
    /// A `system-command`, already checked for its direction; `id` is its message id, which `system-result` names.
    SystemCommand {
        from: DeviceId,
        id: u64,
        command: SystemCommand,
    },
    /// The phone's battery and network; only a desktop's sessions report this.
    Status {
        from: DeviceId,
        status: Status,
    },
    /// The peer moved the connection between Bluetooth and IP.
    Path {
        from: DeviceId,
        on_bluetooth: bool,
    },
    /// The session turned live or degraded; [`SessionHandle::link`] has the state at any time.
    Link {
        from: DeviceId,
        state: LinkState,
    },
}

/// Where a session delivers what its peer sends.
pub struct Route {
    pub peer: DeviceId,
    pub events: mpsc::Sender<SessionEvent>,
    pub transfers: TransferHandle,
    /// The peer's hello carried `HEARTBEAT`.
    pub heartbeat: bool,
    /// The connection's path when it runs on a `PathSocket`.
    pub path: Option<crate::path::Route>,
}

enum Command {
    Share { share: Share, reply: oneshot::Sender<Result<(), Error>> },
    Unpair { reply: oneshot::Sender<Result<(), Error>> },
    Send { message: Message, reply: oneshot::Sender<Result<u64, Error>> },
    Wake,
}

/// Reaches a running session. Cheap to clone; every clone stops working when the session ends.
#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::Sender<Command>,
    connection: Connection,
    tap: Option<Tap>,
    liveness: watch::Receiver<Liveness>,
    path: Option<crate::path::Route>,
}

pub struct SessionActor {
    reader: ControlReader,
    commands: mpsc::Receiver<Command>,
    live: Live,
}

struct Live {
    connection: Connection,
    writer: ControlWriter,
    state: SessionState,
    /// Applied to what a phone sends a desktop.
    limits: Option<Limits>,
    route: Route,
    waiting: HashMap<u64, oneshot::Sender<Result<(), Error>>>,
    beat: Beat,
}

/// The heartbeat side of a session: the phone sends, both sides answer and track the peer's promise.
struct Beat {
    liveness: watch::Sender<Liveness>,
    /// Heartbeats are on: the peer has the feature.
    on: bool,
    /// This side sends heartbeats: the phone.
    sends: bool,
    seq: u64,
    /// Our heartbeats not yet answered, by seq.
    sent: HashMap<u64, Instant>,
    due: Instant,
    active_until: Instant,
    /// Whether the last [`SessionEvent::Link`] said live.
    reported_live: bool,
}

/// Spawns nothing: the caller runs the actor in a task it owns and keeps the handle.
pub fn session(connection: Connection, control: Control, role: Role, route: Route) -> (SessionHandle, SessionActor) {
    let (commands_tx, commands) = mpsc::channel(32);
    let now = Instant::now();
    let (liveness, liveness_rx) = watch::channel(Liveness::new(now, route.heartbeat));
    let handle = SessionHandle {
        commands: commands_tx,
        connection: connection.clone(),
        tap: control.tap(),
        liveness: liveness_rx,
        path: route.path.clone(),
    };
    let (reader, writer) = control.split();
    let limits = (role == Role::Desktop).then(Limits::default);
    let beat = Beat {
        liveness,
        on: route.heartbeat,
        sends: route.heartbeat && role == Role::Phone,
        seq: 0,
        sent: HashMap::new(),
        due: now,
        active_until: now + ACTIVE_FOR,
        reported_live: true,
    };
    let live =
        Live { connection, writer, state: SessionState::new(role), limits, route, waiting: HashMap::new(), beat };
    (handle, SessionActor { reader, commands, live })
}

impl SessionHandle {
    /// Sends `share` and waits up to [`STEP_TIMEOUT`] for the peer's ack.
    pub async fn share(&self, share: Share) -> Result<(), Error> {
        share.check()?;
        let (reply, acked) = oneshot::channel();
        self.commands.send(Command::Share { share, reply }).await.map_err(|_| Error::NotConnected)?;
        tokio::time::timeout(STEP_TIMEOUT, acked).await?.map_err(|_| Error::NotConnected)?
    }

    /// Sends a message that is not acknowledged, and returns once it is written.
    pub async fn send(&self, message: Message) -> Result<(), Error> {
        self.send_numbered(message).await.map(drop)
    }

    /// Sends `message` and returns its message id.
    pub async fn send_numbered(&self, message: Message) -> Result<u64, Error> {
        message.validate()?;
        let (reply, sent) = oneshot::channel();
        self.commands.send(Command::Send { message, reply }).await.map_err(|_| Error::NotConnected)?;
        sent.await.map_err(|_| Error::NotConnected)?
    }

    /// Queues an unacknowledged message without waiting; false when it breaks the schema, the session ended, or its
    /// queue is full.
    pub fn post(&self, message: Message) -> bool {
        let (reply, _) = oneshot::channel();
        message.validate().is_ok() && self.commands.try_send(Command::Send { message, reply }).is_ok()
    }

    /// Tells the desktop this phone unpaired and waits for it to close the connection.
    pub async fn unpair(&self) -> Result<(), Error> {
        let (reply, sent) = oneshot::channel();
        self.commands.send(Command::Unpair { reply }).await.map_err(|_| Error::NotConnected)?;
        sent.await.map_err(|_| Error::NotConnected)??;
        drop(tokio::time::timeout(STEP_TIMEOUT, self.connection.closed()).await?);
        Ok(())
    }

    pub fn close(&self, code: CloseCode) {
        self.connection.close(code);
    }

    pub fn is_live(&self) -> bool {
        self.connection.is_live()
    }

    /// The session's liveness now; what every "connected" display reads.
    pub fn link(&self) -> LinkState {
        if !self.connection.is_live() {
            return LinkState::Closed;
        }
        self.liveness.borrow().view(Instant::now())
    }

    /// Something on this side wants a fresh answer soon (the app came to the front): heartbeats go active at once.
    pub fn wake(&self) {
        drop(self.commands.try_send(Command::Wake));
    }

    pub fn stable_id(&self) -> usize {
        self.connection.stable_id()
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The connection's path, for a QUIC session on a `PathSocket`.
    pub fn path(&self) -> Option<&crate::path::Route> {
        self.path.as_ref()
    }

    pub fn on_bluetooth(&self) -> bool {
        self.path.as_ref().map_or_else(|| self.connection.on_bluetooth(), crate::path::Route::on_link)
    }

    /// The transcript tap of this session's control stream, which bulk stream headers go through too.
    pub fn tap(&self) -> Option<&Tap> {
        self.tap.as_ref()
    }
}

impl SessionActor {
    /// Runs until the connection ends. The error says how; a protocol violation is the caller's to close with.
    pub async fn run(self) -> Result<(), Error> {
        let Self { reader, mut commands, mut live } = self;
        let (inbox_tx, mut inbox) = mpsc::channel(8);
        let mut reading = pin!(reader.pump(inbox_tx));
        let connection = live.connection.clone();
        let result = async {
            loop {
                let timer = live.beat.wake_at().map_or(far_future(), Instant::into_std);
                tokio::select! {
                    ended = &mut reading => return ended.and(Err(Error::StreamEnded)),
                    Some(envelope) = inbox.recv() => {
                        if live.on_envelope(envelope).await? {
                            live.connection.close(CloseCode::Done);
                            return Ok(());
                        }
                    }
                    Some(command) = commands.recv() => live.on_command(command).await?,
                    stream = connection.accept_uni() => {
                        if !live.route.transfers.stream(live.route.peer.clone(), stream?) {
                            return Err(Error::Flooded);
                        }
                    }
                    () = tokio::time::sleep_until(timer.into()) => live.on_timer().await?,
                }
            }
        }
        .await;
        live.beat.apply(LinkEvent::Close, Instant::now());
        result
    }
}

fn far_future() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(86_400)
}

impl Beat {
    fn interval(&self, now: Instant) -> std::time::Duration {
        if now < self.active_until { ACTIVE_EVERY } else { IDLE_EVERY }
    }

    /// The next time the actor must look: a heartbeat to send, or the peer's promise running out.
    fn wake_at(&self) -> Option<Instant> {
        let deadline = self.liveness.borrow().deadline().filter(|_| self.reported_live);
        let beat = self.sends.then_some(self.due);
        deadline.into_iter().chain(beat).min()
    }

    /// Traffic other than heartbeats: stay active, and probe at once when coming back from idle.
    fn touch(&mut self, now: Instant) {
        if self.sends && now >= self.active_until {
            self.due = now;
        }
        self.active_until = now + ACTIVE_FOR;
    }

    fn apply(&mut self, event: LinkEvent, now: Instant) {
        self.liveness.send_modify(|liveness| {
            if liveness.apply(event, now).is_err() {
                log::warn!("liveness: {event:?} rejected");
            }
        });
    }

    /// The live/degraded change since the last report, if any.
    fn changed(&mut self, now: Instant) -> Option<LinkState> {
        let state = self.liveness.borrow().view(now);
        let live = matches!(state, LinkState::Live { .. });
        (live != self.reported_live).then(|| {
            self.reported_live = live;
            state
        })
    }
}

impl Live {
    /// Sends a heartbeat when one is due, and reports a peer whose promise ran out.
    async fn on_timer(&mut self) -> Result<(), Error> {
        let now = Instant::now();
        if self.beat.sends && now >= self.beat.due {
            let next = self.beat.interval(now);
            self.beat.seq += 1;
            let seq = self.beat.seq;
            let next_ms = u32::try_from(next.as_millis()).unwrap_or(u32::MAX);
            self.writer.send(Message::Heartbeat(Heartbeat { seq, next_ms })).await?;
            self.beat.sent.retain(|_, at| now.saturating_duration_since(*at) < IDLE_EVERY * 2);
            self.beat.sent.insert(seq, now);
            self.beat.due = now + next;
        }
        self.report(now).await;
        Ok(())
    }

    async fn report(&mut self, now: Instant) {
        if let Some(state) = self.beat.changed(now) {
            log::info!("{}: link {state:?}", self.route.peer);
            self.emit(SessionEvent::Link { from: self.route.peer.clone(), state }).await;
        }
    }

    /// Handles one message from the peer; true when the session is over.
    async fn on_envelope(&mut self, envelope: Envelope) -> Result<bool, Error> {
        let now = Instant::now();
        if let Some(path) = &self.route.path
            && path.follow_connection(self.connection.quic())
        {
            let on_bluetooth = path.on_link();
            self.emit(SessionEvent::Path { from: self.route.peer.clone(), on_bluetooth }).await;
        }
        match envelope.message {
            Message::Heartbeat(Heartbeat { seq, next_ms }) if self.beat.on => {
                self.writer.send(Message::HeartbeatAck(HeartbeatAck { seq })).await?;
                self.beat.apply(LinkEvent::Heard { next: std::time::Duration::from_millis(next_ms.into()) }, now);
                self.report(now).await;
                return Ok(false);
            }
            Message::HeartbeatAck(HeartbeatAck { seq }) if self.beat.on => {
                if let Some(sent) = self.beat.sent.remove(&seq) {
                    let next = self.beat.due.saturating_duration_since(now);
                    self.beat.apply(LinkEvent::Acked { sent, next }, now);
                    self.report(now).await;
                }
                return Ok(false);
            }
            _ => self.beat.touch(now),
        }
        let peer = &self.route.peer;
        let now = now.into_std();
        if let Some(limits) = &mut self.limits
            && !limits.admit(&envelope.message, now)
        {
            log::warn!("{peer}: dropping a {} over its rate limit", envelope.message.kind());
            if let Message::Offer(offer) = &envelope.message {
                let reply = OfferReply { transfer: offer.transfer, accepted: false, reason: Some(RefuseReason::Busy) };
                self.writer.send(Message::OfferReply(reply)).await?;
            }
            // Ends the attempt on both sides, so neither waits out the upgrade timeout for it.
            if let Message::Hotspot(_) | Message::WifiDirect(_) = &envelope.message {
                let end = HotspotEnd { reason: Some("too many hotspots in a short time".to_owned()) };
                self.writer.send(Message::HotspotEnd(end.clone())).await?;
                let from = peer.clone();
                self.emit(SessionEvent::Message { from, message: Message::HotspotEnd(end) }).await;
            }
            return Ok(false);
        }
        let message_id = envelope.id;
        match self.state.on_message(envelope)? {
            Inbound::Share { id, share } => {
                log::info!("{peer}: received a {} share", share.kind.as_str());
                self.emit(SessionEvent::Received { from: peer.clone(), share }).await;
                self.writer.send(Message::ShareAck(ShareAck { of: id })).await?;
            }
            Inbound::Acked { of } => {
                if let Some(reply) = self.waiting.remove(&of) {
                    drop(reply.send(Ok(())));
                }
            }
            Inbound::Unpair => {
                self.emit(SessionEvent::Unpaired { from: peer.clone() }).await;
                return Ok(true);
            }
            Inbound::Deliver(Message::SystemCommand(command)) => {
                self.emit(SessionEvent::SystemCommand { from: peer.clone(), id: message_id, command }).await;
            }
            Inbound::Deliver(message) => self.emit(SessionEvent::Message { from: peer.clone(), message }).await,
            Inbound::Status(status) => self.emit(SessionEvent::Status { from: peer.clone(), status }).await,
            Inbound::Transfer(message) => {
                if !self.route.transfers.control(peer.clone(), message) {
                    return Err(Error::Flooded);
                }
            }
        }
        Ok(false)
    }

    async fn on_command(&mut self, command: Command) -> Result<(), Error> {
        self.beat.touch(Instant::now());
        match command {
            Command::Wake => {}
            Command::Share { share, reply } => {
                let id = self.writer.send(Message::Share(share)).await?;
                self.state.sent_share(id);
                self.waiting.insert(id, reply);
            }
            Command::Unpair { reply } => {
                self.writer.send(Message::Unpair).await?;
                drop(reply.send(Ok(())));
            }
            Command::Send { message, reply } => {
                let id = self.writer.send(message).await?;
                drop(reply.send(Ok(id)));
            }
        }
        Ok(())
    }

    async fn emit(&self, event: SessionEvent) {
        if self.route.events.send(event).await.is_err() {
            log::debug!("{}: nobody receives session events", self.route.peer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use link_proto::VERSION;
    use link_proto::message::{HEARTBEAT, Hello};

    use super::*;
    use crate::identity::Identity;
    use crate::inbox::Inbox;
    use crate::link_state::GRACE;

    type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn hello(name: &str) -> Hello {
        Hello { version: VERSION, name: name.to_owned(), addresses: vec![HEARTBEAT.to_owned()] }
    }

    /// A phone and a desktop session over QUIC; the desktop's actor stops being polled while `freeze`
    /// is true, as a stuck peer whose transport stays open.
    #[tokio::test]
    async fn a_stuck_peer_turns_the_link_degraded_and_back() -> Result {
        let (phone_key, desk_key) = (Identity::generate()?, Identity::generate()?);
        let (phone_conn, desktop_conn) = crate::transport::connected(&phone_key, &desk_key).await?;
        let (phone_conn, desktop_conn) = (Connection::from(phone_conn), Connection::from(desktop_conn));
        // QUIC announces a stream with its first bytes, so the desktop accepts the control stream only after the hello.
        let (phone_control, desktop_control) = tokio::try_join!(
            async {
                let mut control = Control::open(&phone_conn, None).await?;
                control.hello_as_client(hello("phone")).await?;
                Ok::<_, Error>(control)
            },
            async {
                let mut control = Control::accept(&desktop_conn, None).await?;
                control.hello_as_server(hello("desk")).await?;
                Ok(control)
            }
        )?;
        // After the handshake: the paused clock would run its timeout out while packets cross the real socket.
        tokio::time::pause();

        let dir = std::env::temp_dir().join(format!("link-session-test-{}", std::process::id()));
        let (transfers, _transfer_actor, _transfer_events) = crate::transfer::transfers(Inbox::new(dir.clone(), &dir)?);
        let route = |peer: DeviceId| {
            let (events, rx) = mpsc::channel(64);
            (Route { peer, events, transfers: transfers.clone(), heartbeat: true, path: None }, rx)
        };
        let (phone_route, mut phone_events) = route(desk_key.spki().device_id());
        let (desktop_route, _desktop_events) = route(phone_key.spki().device_id());
        let (phone, phone_actor) = session(phone_conn, phone_control, Role::Phone, phone_route);
        let (desktop, desktop_actor) = session(desktop_conn, desktop_control, Role::Desktop, desktop_route);
        tokio::spawn(phone_actor.run());
        let (freeze, frozen) = watch::channel(false);
        tokio::spawn(async move {
            let mut run = pin!(desktop_actor.run());
            let mut frozen = frozen;
            loop {
                if *frozen.borrow() {
                    drop(frozen.changed().await);
                    continue;
                }
                tokio::select! {
                    _ = &mut run => return,
                    _ = frozen.changed() => {}
                }
            }
        });

        tokio::time::sleep(Duration::from_secs(15)).await;
        assert!(matches!(phone.link(), LinkState::Live { rtt: Some(_) }), "phone: {:?}", phone.link());
        assert!(matches!(desktop.link(), LinkState::Live { .. }), "desktop: {:?}", desktop.link());

        freeze.send(true)?;
        tokio::time::sleep(ACTIVE_EVERY + GRACE + Duration::from_millis(1100)).await;
        assert!(matches!(phone.link(), LinkState::Degraded { .. }), "phone: {:?}", phone.link());
        assert!(matches!(desktop.link(), LinkState::Degraded { .. }), "desktop: {:?}", desktop.link());
        let mut reported = Vec::new();
        while let Ok(event) = phone_events.try_recv() {
            if let SessionEvent::Link { state, .. } = event {
                reported.push(state);
            }
        }
        assert!(matches!(reported.last(), Some(LinkState::Degraded { .. })), "phone reported {reported:?}");

        freeze.send(false)?;
        tokio::time::sleep(ACTIVE_EVERY * 2).await;
        assert!(matches!(phone.link(), LinkState::Live { .. }), "phone after: {:?}", phone.link());
        assert!(matches!(desktop.link(), LinkState::Live { .. }), "desktop after: {:?}", desktop.link());
        drop(std::fs::remove_dir_all(&dir));
        Ok(())
    }
}
