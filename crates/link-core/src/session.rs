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
    live: Live,
}

struct Live {
    connection: Connection,
    commands: mpsc::Receiver<Command>,
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
    let live = Live {
        connection,
        commands,
        writer,
        state: SessionState::new(role),
        limits,
        route,
        waiting: HashMap::new(),
        beat,
    };
    (handle, SessionActor { reader, live })
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
        self.path.as_ref().map_or_else(|| self.connection.on_bluetooth(), crate::path::Route::on_bluetooth)
    }

    /// The transcript tap of this session's control stream, which bulk stream headers go through too.
    pub fn tap(&self) -> Option<&Tap> {
        self.tap.as_ref()
    }
}

impl SessionActor {
    /// Runs until the connection ends. The error says how; a protocol violation is the caller's to close with.
    pub async fn run(self) -> Result<(), Error> {
        let Self { reader, mut live } = self;
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
                    Some(command) = live.commands.recv() => live.on_command(command).await?,
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
        self.report(now).await?;
        Ok(())
    }

    async fn report(&mut self, now: Instant) -> Result<(), Error> {
        if let Some(state) = self.beat.changed(now) {
            log::info!("{}: link {state:?}", self.route.peer);
            self.emit(SessionEvent::Link { from: self.route.peer.clone(), state }).await?;
        }
        Ok(())
    }

    /// Handles one message from the peer; true when the session is over.
    async fn on_envelope(&mut self, envelope: Envelope) -> Result<bool, Error> {
        let now = Instant::now();
        if let Some(path) = &self.route.path
            && path.follow_connection(self.connection.quic())
        {
            let on_bluetooth = path.on_bluetooth();
            self.emit(SessionEvent::Path { from: self.route.peer.clone(), on_bluetooth }).await?;
        }
        match envelope.message {
            Message::Heartbeat(Heartbeat { seq, next_ms }) if self.beat.on => {
                self.writer.send(Message::HeartbeatAck(HeartbeatAck { seq })).await?;
                self.beat.apply(LinkEvent::Heard { next: std::time::Duration::from_millis(next_ms.into()) }, now);
                self.report(now).await?;
                return Ok(false);
            }
            Message::HeartbeatAck(HeartbeatAck { seq }) if self.beat.on => {
                if let Some(sent) = self.beat.sent.remove(&seq) {
                    let next = self.beat.due.saturating_duration_since(now);
                    self.beat.apply(LinkEvent::Acked { sent, next }, now);
                    self.report(now).await?;
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
                self.emit(SessionEvent::Message { from, message: Message::HotspotEnd(end) }).await?;
            }
            return Ok(false);
        }
        let message_id = envelope.id;
        match self.state.on_message(envelope)? {
            Inbound::Share { id, share } => {
                log::info!("{peer}: share {:016x} received ({})", share.item.unwrap_or(0), share.kind.as_str());
                self.emit(SessionEvent::Received { from: peer.clone(), share }).await?;
                self.writer.send(Message::ShareAck(ShareAck { of: id })).await?;
            }
            Inbound::Acked { of } => {
                if let Some(reply) = self.waiting.remove(&of) {
                    drop(reply.send(Ok(())));
                }
            }
            Inbound::Unpair => {
                self.emit(SessionEvent::Unpaired { from: peer.clone() }).await?;
                return Ok(true);
            }
            Inbound::Deliver(Message::SystemCommand(command)) => {
                self.emit(SessionEvent::SystemCommand { from: peer.clone(), id: message_id, command }).await?;
            }
            Inbound::Deliver(message) => self.emit(SessionEvent::Message { from: peer.clone(), message }).await?,
            Inbound::Status(status) => self.emit(SessionEvent::Status { from: peer.clone(), status }).await?,
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

    /// Waits for room in the owner's event queue while it serves commands: an owner that sends through this
    /// session before it drains events would otherwise wait on this session forever.
    async fn emit(&mut self, event: SessionEvent) -> Result<(), Error> {
        let events = self.route.events.clone();
        let mut room = pin!(events.reserve());
        loop {
            tokio::select! {
                biased;
                permit = &mut room => {
                    match permit {
                        Ok(permit) => permit.send(event),
                        Err(_) => log::debug!("{}: nobody receives session events", self.route.peer),
                    }
                    return Ok(());
                }
                Some(command) = self.commands.recv() => self.on_command(command).await?,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use link_proto::VERSION;
    use link_proto::message::{HEARTBEAT, Hello, ShareKind};

    use super::*;
    use crate::identity::Identity;
    use crate::inbox::Inbox;
    use crate::link_state::GRACE;

    type Result = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn hello(name: &str) -> Hello {
        Hello { version: VERSION, name: name.to_owned(), addresses: vec![HEARTBEAT.to_owned()] }
    }

    const PHONE: usize = 0;
    const DESKTOP: usize = 1;

    /// A phone (index 0) and a desktop (index 1) session over loopback QUIC, on a paused clock. Each actor stops
    /// being polled while its `freeze` is true, as a stuck peer whose transport stays open.
    struct Pair {
        handles: [SessionHandle; 2],
        events: [mpsc::Receiver<SessionEvent>; 2],
        freeze: [watch::Sender<bool>; 2],
        dir: std::path::PathBuf,
        /// With a bottleneck only: the desktop's mirror and file streams, from a running transfer actor.
        transfers: Option<mpsc::UnboundedReceiver<crate::transfer::TransferEvent>>,
    }

    impl Drop for Pair {
        fn drop(&mut self) {
            drop(std::fs::remove_dir_all(&self.dir));
        }
    }

    async fn pair(name: &str) -> std::result::Result<Pair, Box<dyn std::error::Error + Send + Sync>> {
        pair_with_events(name, 1024).await
    }

    async fn pair_with_events(
        name: &str,
        events: usize,
    ) -> std::result::Result<Pair, Box<dyn std::error::Error + Send + Sync>> {
        pair_through(name, events, None).await
    }

    /// [`pair_with_events`]; with `bottleneck`, the phone sends at that many bytes a second on the real clock, and
    /// the desktop's transfer actor runs.
    async fn pair_through(
        name: &str,
        events: usize,
        bottleneck: Option<u32>,
    ) -> std::result::Result<Pair, Box<dyn std::error::Error + Send + Sync>> {
        let (phone_key, desk_key) = (Identity::generate()?, Identity::generate()?);
        let (phone_conn, desktop_conn) = crate::transport::connected_through(&phone_key, &desk_key, bottleneck).await?;
        let (phone_conn, desktop_conn) = (Connection::from(phone_conn), Connection::from(desktop_conn));
        let (phone_control, desktop_control) = handshake(&phone_conn, &desktop_conn).await?;
        // After the handshake: the paused clock would run its timeout out while packets cross the real socket.
        if bottleneck.is_none() {
            tokio::time::pause();
        }

        let dir = std::env::temp_dir().join(format!("link-session-{name}-{}", std::process::id()));
        let (transfers, transfer_actor, transfer_events) = crate::transfer::transfers(Inbox::new(dir.clone(), &dir)?);
        let transfer_events = bottleneck.map(|_| {
            tokio::spawn(transfer_actor.run());
            transfer_events
        });
        let route = |peer: DeviceId| {
            let (events, rx) = mpsc::channel(events);
            (Route { peer, events, transfers: transfers.clone(), heartbeat: true, path: None }, rx)
        };
        let (phone_route, phone_events) = route(desk_key.spki().device_id());
        let (desktop_route, desktop_events) = route(phone_key.spki().device_id());
        let (phone, phone_actor) = session(phone_conn, phone_control, Role::Phone, phone_route);
        let (desktop, desktop_actor) = session(desktop_conn, desktop_control, Role::Desktop, desktop_route);
        Ok(Pair {
            handles: [phone, desktop],
            events: [phone_events, desktop_events],
            freeze: [freezable(phone_actor), freezable(desktop_actor)],
            dir,
            transfers: transfer_events,
        })
    }

    /// The hello on a new connection's control stream: (phone side, desktop side).
    async fn handshake(phone: &Connection, desktop: &Connection) -> std::result::Result<(Control, Control), Error> {
        // QUIC announces a stream with its first bytes, so the desktop accepts the control stream only after the hello.
        tokio::try_join!(
            async {
                let mut control = Control::open(phone, None).await?;
                control.hello_as_client(hello("phone")).await?;
                Ok::<_, Error>(control)
            },
            async {
                let mut control = Control::accept(desktop, None).await?;
                control.hello_as_server(hello("desk")).await?;
                Ok(control)
            }
        )
    }

    fn freezable(actor: SessionActor) -> watch::Sender<bool> {
        let (freeze, mut frozen) = watch::channel(false);
        tokio::spawn(async move {
            let mut run = pin!(actor.run());
            loop {
                if *frozen.borrow() {
                    drop(frozen.changed().await);
                    continue;
                }
                tokio::select! {
                    _ = &mut run => return,
                    changed = frozen.changed() => if changed.is_err() {
                        drop(run.await);
                        return;
                    },
                }
            }
        });
        freeze
    }

    /// The session's owner sends through it while its events wait for room; the owner drains events only after.
    #[tokio::test]
    async fn a_send_completes_while_the_owner_has_not_drained_events() -> Result {
        let Pair { handles: [phone, desktop], events: [_, desktop_events], .. } =
            &mut pair_with_events("full", 1).await?;
        for item in 0..4 {
            let phone = phone.clone();
            let share = Share { kind: ShareKind::Text, text: format!("share {item}"), item: Some(item) };
            tokio::spawn(async move { phone.share(share).await });
        }
        let first = desktop_events.recv().await;
        assert!(matches!(first, Some(SessionEvent::Received { .. })), "first event: {first:?}");
        // Real time for the other shares to cross loopback, so the actor waits in emit with one event queued.
        tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(300))).await?;

        let sent = tokio::time::timeout(Duration::from_secs(30), desktop.send(Message::ContactsChanged)).await;
        assert!(matches!(sent, Ok(Ok(()))), "send while events are full: {sent:?}");
        let mut received = 1;
        while received < 4 {
            match tokio::time::timeout(Duration::from_secs(30), desktop_events.recv()).await? {
                Some(SessionEvent::Received { .. }) => received += 1,
                Some(_) => {}
                None => break,
            }
        }
        assert_eq!(received, 4);
        Ok(())
    }

    #[tokio::test]
    async fn a_stuck_peer_turns_the_link_degraded_and_back() -> Result {
        let Pair { handles: [phone, desktop], events: [phone_events, _], freeze: [_, freeze], .. } =
            &mut pair("stuck").await?;

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
        Ok(())
    }

    /// Mirror video fills a 3 Mbit/s path with a short queue, as on Bluetooth or weak Wi-Fi: the control stream
    /// outranks it, so heartbeats still cross within their grace and neither side turns degraded.
    #[tokio::test]
    async fn heartbeats_pass_while_mirror_video_fills_a_slow_path() -> Result {
        use crate::mirror::{MirrorSender, Unit};
        use crate::transfer::TransferEvent;
        use link_proto::message::MIRROR_KEYFRAME;

        let mut pair = pair_through("priority", 1024, Some(375_000)).await?;
        let [phone, desktop] = pair.handles.clone();
        // Until the first heartbeat is acked, no deadline exists to miss.
        let first = Instant::now() + IDLE_EVERY + GRACE;
        while !matches!(phone.link(), LinkState::Live { rtt: Some(_) }) {
            assert!(Instant::now() < first, "no heartbeat round trip: {:?}", phone.link());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let sender = MirrorSender::open(&phone, 16, 16, None, true).await?;
        let video = tokio::spawn(async move {
            sender.send(&Unit { pts_us: 0, flags: MIRROR_KEYFRAME, data: vec![1; 64 * 1024] }).await?;
            for pts_us in 1.. {
                sender.send(&Unit { pts_us, flags: 0, data: vec![2; 128 * 1024] }).await?;
            }
            Ok::<_, Error>(())
        });
        let transfers = pair.transfers.as_mut().ok_or("no transfer events")?;
        let stream = loop {
            match tokio::time::timeout(Duration::from_secs(10), transfers.recv()).await? {
                Some(TransferEvent::Mirror { stream, .. }) => break stream,
                Some(_) => {}
                None => return Err("the transfer actor ended".into()),
            }
        };
        let mut receiver = stream.take().ok_or("the mirror stream was taken")?;
        let read = tokio::spawn(async move {
            let mut bytes = 0;
            while let Ok(Some(unit)) = receiver.next().await {
                bytes += unit.data.len();
            }
            bytes
        });

        // Long past ACTIVE_EVERY + GRACE, which a starved heartbeat turns degraded in.
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            assert!(matches!(phone.link(), LinkState::Live { .. }), "phone: {:?}", phone.link());
            assert!(matches!(desktop.link(), LinkState::Live { .. }), "desktop: {:?}", desktop.link());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(!video.is_finished(), "the video stopped: {:?}", video.await);
        video.abort();
        drop(pair);
        let bytes = read.await?;
        assert!(bytes > 1_000_000, "only {bytes} bytes of video crossed, so the path was not full");
        Ok(())
    }

    /// Two phones on one desktop endpoint and one desktop transfer actor, as linkd runs them: phone A's mirror video
    /// stalls because the desktop never reads it, and phone B's file still arrives while both links stay live.
    #[tokio::test]
    async fn a_stalled_mirror_from_one_phone_does_not_hold_another_phones_file() -> Result {
        use crate::mirror::{MirrorSender, Unit};
        use crate::transfer::{ClipPeer, Source, Status, TransferEvent, transfers};
        use link_proto::message::MIRROR_KEYFRAME;

        let dir = std::env::temp_dir().join(format!("link-session-two-phones-{}", std::process::id()));
        let inbox = |name: &str| Inbox::new(dir.join(name), &dir.join(name));
        let desk_key = Identity::generate()?;
        let server = crate::transport::server_endpoint(&desk_key, std::net::SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let (desk_transfers, desk_actor, mut desk_events) = transfers(inbox("desk")?);
        tokio::spawn(desk_actor.run());
        let mut phones = Vec::new();
        for name in ["a", "b"] {
            let phone_key = Identity::generate()?;
            let (phone_conn, desk_conn) =
                crate::transport::connect(&server, server.local_addr()?, &phone_key, &desk_key).await?;
            let (phone_conn, desk_conn) = (Connection::from(phone_conn), Connection::from(desk_conn));
            let (phone_control, desk_control) = handshake(&phone_conn, &desk_conn).await?;
            let (phone_transfers, phone_actor, phone_events) = transfers(inbox(name)?);
            tokio::spawn(phone_actor.run());
            let (events, phone_session_events) = mpsc::channel(1024);
            let route = Route {
                peer: desk_key.spki().device_id(),
                events,
                transfers: phone_transfers.clone(),
                heartbeat: true,
                path: None,
            };
            let (phone, actor) = session(phone_conn, phone_control, Role::Phone, route);
            tokio::spawn(actor.run());
            let (events, desk_session_events) = mpsc::channel(1024);
            let peer = phone_key.spki().device_id();
            let route =
                Route { peer: peer.clone(), events, transfers: desk_transfers.clone(), heartbeat: true, path: None };
            let (desk, actor) = session(desk_conn, desk_control, Role::Desktop, route);
            tokio::spawn(actor.run());
            phone_transfers.attach(desk_key.spki().device_id(), phone.clone(), ClipPeer::default(), false).await;
            desk_transfers.attach(peer.clone(), desk.clone(), ClipPeer::default(), false).await;
            phones.push((peer, phone, desk, phone_transfers, phone_events, phone_session_events, desk_session_events));
        }
        let [(a_id, a_phone, a_desk, ..), (_, b_phone, b_desk, b_transfers, ..)] = phones.as_slice() else {
            return Err("two phones".into());
        };

        let sender = MirrorSender::open(a_phone, 16, 16, None, true).await?;
        let video = tokio::spawn(async move {
            sender.send(&Unit { pts_us: 0, flags: MIRROR_KEYFRAME, data: vec![1; 64 * 1024] }).await?;
            for pts_us in 1.. {
                sender.send(&Unit { pts_us, flags: 0, data: vec![2; 128 * 1024] }).await?;
            }
            Ok::<_, Error>(())
        });
        // Held unread, so phone A's video fills its flow-control window and stops.
        let _stalled = loop {
            match tokio::time::timeout(Duration::from_secs(10), desk_events.recv()).await? {
                Some(TransferEvent::Mirror { from, stream }) if from == *a_id => break stream,
                Some(_) => {}
                None => return Err("the desktop transfer actor ended".into()),
            }
        };
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!video.is_finished(), "the video stopped: {:?}", video.await);

        let path = dir.join("payload");
        std::fs::write(&path, vec![3u8; 8 << 20])?;
        let source =
            Source::new(std::fs::File::open(&path)?, "payload".to_owned(), "application/octet-stream".to_owned())?;
        let desk_id = desk_key.spki().device_id();
        b_transfers.send(desk_id, vec![source]).await?;
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            for (side, handle) in [("A phone", a_phone), ("A desk", a_desk), ("B phone", b_phone), ("B desk", b_desk)] {
                assert!(matches!(handle.link(), LinkState::Live { .. }), "{side}: {:?}", handle.link());
            }
            let event =
                tokio::time::timeout_at(end, desk_events.recv()).await.map_err(|_| "phone B's file did not arrive")?;
            match event {
                Some(TransferEvent::Offered { id, .. }) => assert!(desk_transfers.decide(id, true).await?),
                Some(TransferEvent::Finished { incoming: true, status, files, .. }) => {
                    assert_eq!(status, Status::Done);
                    assert_eq!(std::fs::metadata(&files.first().ok_or("no file")?.path)?.len(), 8 << 20);
                    break;
                }
                Some(_) => {}
                None => return Err("the desktop transfer actor ended".into()),
            }
        }
        assert!(!video.is_finished(), "the video stopped: {:?}", video.await);
        video.abort();
        drop(std::fs::remove_dir_all(&dir));
        Ok(())
    }

    struct Pending {
        item: u64,
        to: usize,
        /// Either side was frozen at some time since the send.
        stalled: bool,
        reply: tokio::task::JoinHandle<std::result::Result<(), Error>>,
    }

    /// What the model test knows about one pair, and the invariants it checks against it.
    struct Model {
        pair: Pair,
        sequence: u32,
        frozen: [bool; 2],
        /// The last time each side ran.
        ran: [Instant; 2],
        /// The last time each side came back from a freeze.
        thawed: [Instant; 2],
        sent: [Vec<u64>; 2],
        received: [Vec<u64>; 2],
        reported_live: [bool; 2],
        pending: Vec<Pending>,
    }

    impl Model {
        fn new(pair: Pair, sequence: u32) -> Self {
            let start = Instant::now();
            Self {
                pair,
                sequence,
                frozen: [false; 2],
                ran: [start; 2],
                thawed: [start; 2],
                sent: [Vec::new(), Vec::new()],
                received: [Vec::new(), Vec::new()],
                reported_live: [true; 2],
                pending: Vec::new(),
            }
        }

        fn share(&mut self, from: usize, item: u64) {
            self.sent[from].push(item);
            let share = Share { kind: ShareKind::Text, text: format!("share {item}"), item: Some(item) };
            let handle = self.pair.handles[from].clone();
            let reply = tokio::spawn(async move { handle.share(share).await });
            self.pending.push(Pending { item, to: 1 - from, stalled: self.frozen.contains(&true), reply });
        }

        fn toggle(&mut self, side: usize) -> Result {
            let now = Instant::now();
            if self.frozen[side] {
                self.thawed[side] = now;
            } else {
                for share in &mut self.pending {
                    share.stalled = true;
                }
            }
            self.ran[side] = now;
            self.frozen[side] = !self.frozen[side];
            self.pair.freeze[side].send(self.frozen[side])?;
            Ok(())
        }

        fn drain(&mut self) {
            let sequence = self.sequence;
            for side in [PHONE, DESKTOP] {
                while let Ok(event) = self.pair.events[side].try_recv() {
                    match event {
                        SessionEvent::Received { share, .. } => {
                            let id = share.item.unwrap_or(0);
                            assert!(!self.received[side].contains(&id), "{sequence}: share {id} received twice");
                            self.received[side].push(id);
                        }
                        SessionEvent::Link { state, .. } => {
                            let live = matches!(state, LinkState::Live { .. });
                            assert_ne!(
                                live, self.reported_live[side],
                                "{sequence}: side {side} reported {state:?} twice"
                            );
                            self.reported_live[side] = live;
                        }
                        _ => {}
                    }
                }
            }
        }

        async fn check(&mut self) -> Result {
            let sequence = self.sequence;
            let now = Instant::now();
            for side in [PHONE, DESKTOP].into_iter().filter(|&side| !self.frozen[side]) {
                self.ran[side] = now;
            }
            self.drain();
            for side in [PHONE, DESKTOP] {
                let last_beat = self.ran[1 - side].max(self.thawed[side]);
                if now > last_beat + IDLE_EVERY + GRACE + Duration::from_secs(1) {
                    let link = self.pair.handles[side].link();
                    assert!(!matches!(link, LinkState::Live { .. }), "{sequence}: side {side} shows {link:?}");
                }
            }
            for share in self.pending.iter_mut().filter(|share| share.reply.is_finished()) {
                let reply = (&mut share.reply).await?;
                if reply.is_ok() {
                    let seen = self.received[share.to].contains(&share.item);
                    assert!(seen, "{sequence}: share {} acked unseen", share.item);
                } else {
                    assert!(matches!(reply, Err(Error::Timeout)) && share.stalled, "{sequence}: {reply:?}");
                }
            }
            self.pending.retain(|share| !share.reply.is_finished());
            Ok(())
        }

        /// Both sides run again: both turn Live, every share arrives, and the last report matches `link()`.
        async fn settle(mut self) -> Result {
            let sequence = self.sequence;
            for side in [PHONE, DESKTOP] {
                self.pair.freeze[side].send(false)?;
            }
            self.pair.handles[PHONE].wake();
            tokio::time::sleep(ACTIVE_EVERY * 2).await;
            self.drain();
            for side in [PHONE, DESKTOP] {
                let link = self.pair.handles[side].link();
                assert!(matches!(link, LinkState::Live { .. }), "{sequence}: side {side} after the thaw: {link:?}");
                assert!(self.reported_live[side], "{sequence}: side {side} last reported degraded but shows {link:?}");
                let mut got = self.received[side].clone();
                got.sort_unstable();
                assert_eq!(got, self.sent[1 - side], "{sequence}: side {side} received");
            }
            Ok(())
        }
    }

    /// Random shares both ways, freezes, wakes and sleeps. Invariants: a side never shows Live once every heartbeat
    /// the peer could have sent is past its longest promise plus GRACE; each share is received once, and an acked
    /// one before its ack; shares fail only by timeout while a side stalled; Link reports alternate.
    #[tokio::test]
    async fn random_sequences_keep_the_invariants() -> Result {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let mut item = 0;
        for sequence in 0..25 {
            let mut model = Model::new(pair("model").await?, sequence);
            for _ in 0..40 {
                match next(6) {
                    0 | 1 => {
                        item += 1;
                        model.share(usize::try_from(next(2))?, item);
                    }
                    2 => model.toggle(usize::try_from(next(2))?)?,
                    3 => model.pair.handles[usize::try_from(next(2))?].wake(),
                    _ => tokio::time::sleep(Duration::from_millis(next(4000))).await,
                }
                tokio::task::yield_now().await;
                model.check().await?;
            }
            model.settle().await?;
            // The next pair's handshake needs real time.
            tokio::time::resume();
        }
        Ok(())
    }
}
