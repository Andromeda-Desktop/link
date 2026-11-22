//! The transfer actor: every file transfer of this process, both directions, across sessions. It owns the open
//! sources, the inbox, and the transfer rules' state; sessions hand it messages and bulk streams, and its owner
//! answers consent. See `ARCHITECTURE.md` (Files).

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use link_proto::CloseCode;
use link_proto::frame::MAX_FRAME;
use link_proto::message::{
    ClipPull, Envelope, FileData, FileDone, FileHash, FileMeta, FileOffset, Message, Offer, OfferReply, RefuseReason,
    ResumeAt, TransferId, TransferRef,
};
use link_proto::transfer::{Budget, Incoming, MAX_IN_FLIGHT, Outgoing, OutgoingPhase, Overrun};
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::Instant;

use crate::Error;
use crate::control::{STEP_TIMEOUT, read_frame, write_frame};
use crate::identity::DeviceId;
use crate::inbox::{self, Inbox, Part, Record, RecordFile};
use crate::session::SessionHandle;
use crate::wire::{RecvStream, SendStream};

mod clip;

use clip::Clips;
pub use clip::{ClipBytes, ClipPeer, LocalClip};

/// How long the receiver's user has to answer an offer.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long an unfinished transfer is kept for a resume.
pub const KEEP: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_OPEN_PER_PEER: usize = 8;
const CHUNK: usize = 256 * 1024;
/// Disk reads and writes go in batches of this size to the blocking pool, off the thread that drives QUIC.
const BATCH: usize = 1 << 20;
const SYNC_EVERY: u64 = 8 << 20;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
const PROTOCOL_ERROR: u32 = CloseCode::ProtocolError as u32;
const CANCELLED: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    Failed,
    Declined,
    NoSpace,
    TooLarge,
    Busy,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferedFile {
    /// Already sanitized.
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// Empty when the peer sends the digest after the bytes (`LATE_HASH`).
    pub sha256: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    /// A peer offers files; answer with [`TransferHandle::decide`] within [`CONSENT_TIMEOUT`].
    Offered { id: TransferId, from: DeviceId, files: Vec<OfferedFile> },
    /// At most every 250 ms per transfer.
    Progress { id: TransferId, bytes: u64, total: u64 },
    /// `files` holds what an incoming transfer published.
    Finished { id: TransferId, peer: DeviceId, incoming: bool, status: Status, files: Vec<ReceivedFile> },
    /// Whether any transfer with `peer` is open; a phone keeps its session alive while one is.
    Busy { peer: DeviceId, busy: bool },
    /// A peer's clipboard changed; pull it with [`TransferHandle::pull_clip`]. `text` is inline, set at once.
    ClipOffered { from: DeviceId, id: u64, mimes: Vec<String>, size: u64, text: Option<String> },
    /// A peer opened its screen's video stream; the handler takes it or drops it.
    Mirror { from: DeviceId, stream: crate::mirror::MirrorStream },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedFile {
    pub path: PathBuf,
    pub sha256: Vec<u8>,
}

/// A file to send: an open regular file and the name the receiver sees.
pub struct Source {
    file: File,
    size: u64,
    name: String,
    mime: String,
}

enum Command {
    Attach { peer: DeviceId, session: SessionHandle, clip: ClipPeer, late_hash: bool },
    Detach { peer: DeviceId, stable_id: usize },
    Control { peer: DeviceId, message: Message },
    Stream { peer: DeviceId, recv: RecvStream },
    Send { peer: DeviceId, sources: Vec<Source>, reply: oneshot::Sender<Result<TransferId, Error>> },
    Decide { id: TransferId, accept: bool, reply: oneshot::Sender<bool> },
    Cancel { id: TransferId, reply: oneshot::Sender<bool> },
    OfferClip { peers: Vec<DeviceId>, clip: LocalClip, reply: oneshot::Sender<Result<(), Error>> },
    PullClip { peer: DeviceId, id: u64, mime: String, sink: File, reply: oneshot::Sender<Result<u64, Error>> },
}

#[derive(Clone)]
pub struct TransferHandle(mpsc::Sender<Command>);

struct Out {
    peer: DeviceId,
    sources: HashMap<u64, (Arc<File>, u64)>,
    offer: Option<Offer>,
    state: Option<Outgoing>,
    deadline: Option<Instant>,
    created: Instant,
    queue: VecDeque<FileOffset>,
    running: HashMap<u64, (AbortHandle, u64)>,
    sent: HashMap<u64, u64>,
    last_progress: Option<Instant>,
    /// The offer went out with empty digests; each file's digest follows its last byte in `file-hash`.
    late_hash: bool,
}

struct In {
    state: Incoming,
    record: Record,
    deadline: Option<Instant>,
    running: HashMap<u64, (AbortHandle, u64)>,
    received: HashMap<u64, u64>,
    last_progress: Option<Instant>,
    /// Files whose bytes all arrived before their `file-hash`, with the part's digest.
    unverified: HashMap<u64, Vec<u8>>,
}

enum TaskEnd {
    Hashed {
        id: TransferId,
        hashes: Result<Vec<Vec<u8>>, Error>,
    },
    Header {
        peer: DeviceId,
        recv: RecvStream,
        header: Result<Envelope, Error>,
    },
    Received {
        id: TransferId,
        file: u64,
        seq: u64,
        end: Received,
    },
    /// `Ok(Some(digest))` when the file was hashed while it was sent.
    Sent {
        id: TransferId,
        file: u64,
        seq: u64,
        result: Result<Option<Vec<u8>>, Error>,
    },
    ClipServed {
        peer: DeviceId,
        result: Result<(), Error>,
    },
    ClipPulled {
        peer: DeviceId,
        header: ClipPull,
        result: Result<u64, Error>,
    },
}

enum Received {
    Complete { digest: Vec<u8> },
    Paused { durable: u64 },
    Overrun(Overrun),
    Failed(Error),
}

struct Tick {
    id: TransferId,
    file: u64,
    seq: u64,
    offset: u64,
    durable: bool,
}

#[derive(Clone)]
struct Ticker {
    tx: mpsc::Sender<Tick>,
    id: TransferId,
    file: u64,
    seq: u64,
}

pub struct TransferActor {
    inbox: Inbox,
    commands: mpsc::Receiver<Command>,
    events: mpsc::UnboundedSender<TransferEvent>,
    sessions: HashMap<DeviceId, SessionHandle>,
    /// Peers whose hello carries `LATE_HASH`.
    late_hash: HashSet<DeviceId>,
    outgoing: HashMap<TransferId, Out>,
    incoming: HashMap<TransferId, In>,
    tasks: JoinSet<TaskEnd>,
    ticks_tx: mpsc::Sender<Tick>,
    ticks: mpsc::Receiver<Tick>,
    next_seq: u64,
    busy: HashSet<DeviceId>,
    clips: Clips,
    /// Each peer's latest mirror, which takes its later video streams.
    mirrors: HashMap<DeviceId, crate::mirror::MirrorParts>,
}

/// The caller runs the actor in a task it owns and reads the events.
pub fn transfers(inbox: Inbox) -> (TransferHandle, TransferActor, mpsc::UnboundedReceiver<TransferEvent>) {
    let (commands_tx, commands) = mpsc::channel(32);
    let (events, events_rx) = mpsc::unbounded_channel();
    let (ticks_tx, ticks) = mpsc::channel(64);
    let mut actor = TransferActor {
        inbox,
        commands,
        events,
        sessions: HashMap::new(),
        late_hash: HashSet::new(),
        outgoing: HashMap::new(),
        incoming: HashMap::new(),
        tasks: JoinSet::new(),
        ticks_tx,
        ticks,
        next_seq: 0,
        busy: HashSet::new(),
        clips: Clips::default(),
        mirrors: HashMap::new(),
    };
    actor.restore();
    (TransferHandle(commands_tx), actor, events_rx)
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Declined => "declined",
            Self::NoSpace => "no-space",
            Self::TooLarge => "too-large",
            Self::Busy => "busy",
            Self::Cancelled => "cancelled",
        }
    }

    fn refused(reason: RefuseReason) -> Self {
        match reason {
            RefuseReason::Declined => Self::Declined,
            RefuseReason::NoSpace => Self::NoSpace,
            RefuseReason::TooLarge => Self::TooLarge,
            RefuseReason::Busy => Self::Busy,
        }
    }
}

impl Source {
    /// Only a regular file can be hashed up front and read again from an offset.
    pub fn new(file: File, name: String, mime: String) -> Result<Self, Error> {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::NotRegularFile);
        }
        Ok(Self { file, size: metadata.len(), name, mime })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// The file, its size, name, and type, for another backend to send.
    pub fn into_parts(self) -> (File, u64, String, String) {
        (self.file, self.size, self.name, self.mime)
    }
}

impl TransferHandle {
    async fn tell(&self, command: Command) {
        if self.0.send(command).await.is_err() {
            log::debug!("transfer actor stopped");
        }
    }

    async fn request<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T, Error> {
        let (reply, response) = oneshot::channel();
        self.0.send(make(reply)).await.map_err(|_| Error::Stopped)?;
        response.await.map_err(|_| Error::Stopped)
    }

    /// `peer` has a new live session; unfinished transfers with it resume there, and it gets the kept clip.
    /// `late_hash`: its hello carries `LATE_HASH`.
    pub async fn attach(&self, peer: DeviceId, session: SessionHandle, clip: ClipPeer, late_hash: bool) {
        self.tell(Command::Attach { peer, session, clip, late_hash }).await;
    }

    pub async fn detach(&self, peer: DeviceId, stable_id: usize) {
        self.tell(Command::Detach { peer, stable_id }).await;
    }

    /// Never waits, since this actor waits on sessions: false when the queue is full, which only a flooding peer
    /// causes.
    pub(crate) fn control(&self, peer: DeviceId, message: Message) -> bool {
        self.0.try_send(Command::Control { peer, message }).is_ok()
    }

    pub(crate) fn stream(&self, peer: DeviceId, recv: RecvStream) -> bool {
        self.0.try_send(Command::Stream { peer, recv }).is_ok()
    }

    /// Hashes the sources and offers them to `peer`, which must have a live session. Returns at once with the id;
    /// events report the rest.
    pub async fn send(&self, peer: DeviceId, sources: Vec<Source>) -> Result<TransferId, Error> {
        self.request(|reply| Command::Send { peer, sources, reply }).await?
    }

    /// Answers an offer; false when it is no longer waiting.
    pub async fn decide(&self, id: TransferId, accept: bool) -> Result<bool, Error> {
        self.request(|reply| Command::Decide { id, accept, reply }).await
    }

    /// Offers `clip` to each of `peers` that has a live session, replacing the previous offer. Nothing is sent when
    /// the clip is the one last applied from a peer, so a clip never echoes back.
    pub async fn offer_clip(&self, peers: Vec<DeviceId>, clip: LocalClip) -> Result<(), Error> {
        self.request(|reply| Command::OfferClip { peers, clip, reply }).await?
    }

    /// Writes `mime` of `peer`'s clipboard offer `id` into `sink` and returns the byte count.
    pub async fn pull_clip(&self, peer: DeviceId, id: u64, mime: String, sink: File) -> Result<u64, Error> {
        self.request(|reply| Command::PullClip { peer, id, mime, sink, reply }).await?
    }

    /// Cancels an open transfer in either direction; false when there is none.
    pub async fn cancel(&self, id: TransferId) -> Result<bool, Error> {
        self.request(|reply| Command::Cancel { id, reply }).await
    }
}

impl TransferActor {
    /// Runs until every handle is dropped.
    pub async fn run(mut self) {
        loop {
            let deadline = self.next_deadline();
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(command) => self.handle(command).await,
                    None => break,
                },
                Some(joined) = self.tasks.join_next() => match joined {
                    Ok(end) => self.task_ended(end).await,
                    Err(join) if join.is_cancelled() => {}
                    Err(join) => log::warn!("transfer task: {join}"),
                },
                Some(tick) = self.ticks.recv() => self.tick(&tick),
                () = sleep_until(deadline) => self.deadlines().await,
            }
        }
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Attach { peer, session, clip, late_hash } => {
                if late_hash {
                    self.late_hash.insert(peer.clone());
                } else {
                    self.late_hash.remove(&peer);
                }
                self.attach(peer.clone(), session).await;
                self.offer_kept_clip(&peer, clip).await;
            }
            Command::Detach { peer, stable_id } => self.detach(&peer, stable_id),
            Command::Control { peer, message } => {
                if let Err(error) = self.on_control(&peer, message).await {
                    log::warn!("{peer}: {error}");
                    if let Some(session) = self.sessions.get(&peer) {
                        session.close(CloseCode::ProtocolError);
                    }
                }
            }
            Command::Stream { peer, recv } => {
                self.tasks.spawn(async move {
                    let mut recv = recv;
                    let header = match tokio::time::timeout(STEP_TIMEOUT, read_frame(&mut recv, None)).await {
                        Ok(header) => header,
                        Err(elapsed) => Err(elapsed.into()),
                    };
                    TaskEnd::Header { peer, recv, header }
                });
            }
            Command::Send { peer, sources, reply } => drop(reply.send(self.start_send(&peer, sources))),
            Command::Decide { id, accept, reply } => drop(reply.send(self.decide(id, accept).await)),
            Command::Cancel { id, reply } => drop(reply.send(self.cancel(id, true).await)),
            Command::OfferClip { peers, clip, reply } => drop(reply.send(self.offer_clip(&peers, clip).await)),
            Command::PullClip { peer, id, mime, sink, reply } => self.pull_clip(peer, id, mime, sink, reply).await,
        }
    }

    fn restore(&mut self) {
        for record in self.inbox.load(KEEP.as_secs()) {
            let Some(id) = record.id() else { continue };
            let offer = Offer {
                transfer: id,
                files: record
                    .files
                    .iter()
                    .map(|file| FileMeta {
                        id: file.id,
                        name: file.name.clone(),
                        size: file.size,
                        mime: "application/octet-stream".to_owned(),
                        sha256: file.sha256.clone(),
                    })
                    .collect(),
            };
            let mut state = Incoming::new(&offer);
            state.accept();
            for file in &record.files {
                state.set_offset(file.id, file.durable);
                if let Some(ok) = file.done {
                    state.finish(file.id, ok);
                }
            }
            log::info!("{}: transfer {id} kept for a resume", record.peer);
            let incoming = In {
                state,
                record,
                deadline: None,
                running: HashMap::new(),
                received: HashMap::new(),
                last_progress: None,
                unverified: HashMap::new(),
            };
            self.incoming.insert(id, incoming);
        }
    }

    async fn attach(&mut self, peer: DeviceId, session: SessionHandle) {
        self.sessions.insert(peer.clone(), session);
        self.expire();
        let resumable: Vec<TransferId> =
            self.outgoing.iter().filter(|(_, out)| out.peer == peer).map(|(id, _)| *id).collect();
        for id in resumable {
            let Some(out) = self.outgoing.get_mut(&id) else { continue };
            for (task, _) in out.running.drain().map(|(_, running)| running) {
                task.abort();
            }
            out.queue.clear();
            if out.state.as_mut().is_some_and(Outgoing::resume) {
                log::info!("{peer}: resuming transfer {id}");
                self.send_to(&peer, Message::Resume(TransferRef { transfer: id })).await;
            }
        }
    }

    /// The session ended. Offers still waiting for an answer are withdrawn on both sides.
    fn detach(&mut self, peer: &DeviceId, stable_id: usize) {
        if self.sessions.get(peer).is_none_or(|session| session.stable_id() != stable_id) {
            return;
        }
        self.sessions.remove(peer);
        self.mirrors.remove(peer);
        let withdrawn: Vec<TransferId> = self
            .outgoing
            .iter()
            .filter(|(_, out)| {
                out.peer == *peer && out.state.as_ref().is_some_and(|s| s.phase() == OutgoingPhase::Offered)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in withdrawn {
            self.finish_outgoing(id, Status::Failed);
        }
        let pending: Vec<TransferId> = self
            .incoming
            .iter()
            .filter(|(_, incoming)| incoming.record.peer == *peer && !incoming.state.is_accepted())
            .map(|(id, _)| *id)
            .collect();
        for id in pending {
            self.finish_incoming(id, Status::Cancelled);
        }
    }

    async fn on_control(&mut self, peer: &DeviceId, message: Message) -> Result<(), Error> {
        match message {
            Message::Offer(offer) => self.on_offer(peer, offer).await,
            Message::OfferReply(reply) => self.on_reply(peer, &reply),
            Message::Resume(resume) => {
                self.on_resume(peer, resume.transfer).await;
                Ok(())
            }
            Message::ResumeAt(at) => self.on_resume_at(peer, &at),
            Message::Cancel(cancel) => {
                if self.owned_by(cancel.transfer, peer) {
                    log::info!("{peer}: cancelled transfer {}", cancel.transfer);
                    self.cancel(cancel.transfer, false).await;
                }
                Ok(())
            }
            Message::FileDone(done) => self.on_file_done(peer, &done),
            Message::FileHash(hash) => self.on_file_hash(peer, hash).await,
            Message::ClipOffer(offer) => {
                self.on_clip_offer(peer, offer);
                Ok(())
            }
            Message::ClipPull(pull) => {
                self.on_clip_pull(peer, &pull);
                Ok(())
            }
            other => Err(Error::Unexpected(other.kind())),
        }
    }

    fn owned_by(&self, id: TransferId, peer: &DeviceId) -> bool {
        self.outgoing.get(&id).is_some_and(|out| out.peer == *peer)
            || self.incoming.get(&id).is_some_and(|incoming| incoming.record.peer == *peer)
    }

    async fn on_offer(&mut self, peer: &DeviceId, offer: Offer) -> Result<(), Error> {
        let id = offer.transfer;
        if self.incoming.contains_key(&id) || self.outgoing.contains_key(&id) {
            return Err(Error::Unexpected("offer reusing a transfer id"));
        }
        self.expire();
        if let Some(reason) = self.refusal(peer, &offer) {
            log::info!("{peer}: refused transfer {id}: {}", reason.as_str());
            let reply = OfferReply { transfer: id, accepted: false, reason: Some(reason) };
            self.send_to(peer, Message::OfferReply(reply)).await;
            return Ok(());
        }
        let offered: Vec<OfferedFile> = offer
            .files
            .iter()
            .map(|file| OfferedFile {
                name: inbox::sanitize(&file.name),
                size: file.size,
                mime: file.mime.clone(),
                sha256: file.sha256.clone(),
            })
            .collect();
        let record = Record {
            transfer: id.to_hex(),
            peer: peer.clone(),
            created: inbox::now(),
            files: offer
                .files
                .iter()
                .zip(&offered)
                .map(|(file, sanitized)| RecordFile {
                    id: file.id,
                    name: sanitized.name.clone(),
                    size: file.size,
                    sha256: file.sha256.clone(),
                    part: PathBuf::new(),
                    durable: 0,
                    done: None,
                    path: None,
                })
                .collect(),
        };
        log::info!("{peer}: offers transfer {id} of {} files", offer.files.len());
        let incoming = In {
            state: Incoming::new(&offer),
            record,
            deadline: Some(Instant::now() + CONSENT_TIMEOUT),
            running: HashMap::new(),
            received: HashMap::new(),
            last_progress: None,
            unverified: HashMap::new(),
        };
        self.incoming.insert(id, incoming);
        self.update_busy(peer);
        self.emit(TransferEvent::Offered { id, from: peer.clone(), files: offered });
        Ok(())
    }

    fn refusal(&self, peer: &DeviceId, offer: &Offer) -> Option<RefuseReason> {
        let open = self.incoming.values().filter(|incoming| incoming.record.peer == *peer).count();
        if open >= MAX_OPEN_PER_PEER {
            return Some(RefuseReason::Busy);
        }
        let space = match self.inbox.space() {
            Ok(space) => space,
            Err(error) => {
                log::warn!("download directory: {error}");
                return Some(RefuseReason::NoSpace);
            }
        };
        let total = offer.total_size();
        if total > space.total {
            return Some(RefuseReason::TooLarge);
        }
        (total > space.free).then_some(RefuseReason::NoSpace)
    }

    async fn decide(&mut self, id: TransferId, accept: bool) -> bool {
        let Some(incoming) = self.incoming.get_mut(&id).filter(|incoming| !incoming.state.is_accepted()) else {
            return false;
        };
        let peer = incoming.record.peer.clone();
        if !accept {
            self.send_to(&peer, Message::OfferReply(declined(id))).await;
            self.finish_incoming(id, Status::Declined);
            return true;
        }
        let created = create_parts(&self.inbox, &mut incoming.record);
        if let Err(error) = created.and_then(|()| self.inbox.save(&incoming.record)) {
            log::error!("accepting transfer {id}: {error}");
            self.send_to(&peer, Message::OfferReply(declined(id))).await;
            self.finish_incoming(id, Status::Failed);
            return true;
        }
        incoming.state.accept();
        incoming.deadline = None;
        log::info!("{peer}: accepted transfer {id}");
        self.send_to(&peer, Message::OfferReply(OfferReply { transfer: id, accepted: true, reason: None })).await;
        true
    }

    async fn on_resume(&mut self, peer: &DeviceId, id: TransferId) {
        let Some(incoming) = self.incoming.get_mut(&id).filter(|incoming| incoming.record.peer == *peer) else {
            log::info!("{peer}: resume of unknown transfer {id}");
            self.send_to(peer, Message::Cancel(TransferRef { transfer: id })).await;
            return;
        };
        if !incoming.state.is_accepted() {
            self.send_to(peer, Message::Cancel(TransferRef { transfer: id })).await;
            self.finish_incoming(id, Status::Cancelled);
            return;
        }
        for (file, (task, _)) in incoming.running.drain() {
            task.abort();
            let durable = incoming.record.file_mut(file).map_or(0, |record| record.durable);
            incoming.state.stream_ended(file, durable);
        }
        let (results, at) = incoming.state.resume();
        log::info!("{peer}: transfer {id} resumes at {:?}", at.offsets);
        for done in results {
            self.send_to(peer, Message::FileDone(done)).await;
        }
        self.send_to(peer, Message::ResumeAt(at)).await;
    }

    fn on_reply(&mut self, peer: &DeviceId, reply: &OfferReply) -> Result<(), Error> {
        let Some(out) = self.outgoing.get_mut(&reply.transfer).filter(|out| out.peer == *peer) else {
            return Err(Error::Unexpected("offer-reply"));
        };
        let state = out.state.as_mut().ok_or(Error::Unexpected("offer-reply"))?;
        if let Some(reason) = state.on_reply(reply)? {
            log::info!("{peer}: refused transfer {}: {}", reply.transfer, reason.as_str());
            self.finish_outgoing(reply.transfer, Status::refused(reason));
            return Ok(());
        }
        out.deadline = None;
        let files = out.offer.iter().flat_map(|offer| &offer.files);
        out.queue = files.map(|file| FileOffset { file: file.id, offset: 0 }).collect();
        self.pump(reply.transfer);
        Ok(())
    }

    fn on_resume_at(&mut self, peer: &DeviceId, at: &ResumeAt) -> Result<(), Error> {
        let Some(out) = self.outgoing.get_mut(&at.transfer).filter(|out| out.peer == *peer) else {
            return Err(Error::Unexpected("resume-at"));
        };
        let state = out.state.as_mut().ok_or(Error::Unexpected("resume-at"))?;
        out.queue = state.on_resume_at(at)?.into();
        for offset in &out.queue {
            out.sent.insert(offset.file, offset.offset);
        }
        self.pump(at.transfer);
        Ok(())
    }

    fn on_file_done(&mut self, peer: &DeviceId, done: &FileDone) -> Result<(), Error> {
        let Some(out) = self.outgoing.get_mut(&done.transfer).filter(|out| out.peer == *peer) else {
            return Err(Error::Unexpected("file-done"));
        };
        let state = out.state.as_mut().ok_or(Error::Unexpected("file-done"))?;
        let outcome = state.on_file_done(done)?;
        if let Some((task, _)) = out.running.remove(&done.file) {
            task.abort();
        }
        out.queue.retain(|queued| queued.file != done.file);
        if let Some(all_ok) = outcome {
            self.finish_outgoing(done.transfer, if all_ok { Status::Done } else { Status::Failed });
        } else {
            self.pump(done.transfer);
        }
        Ok(())
    }

    /// A file's digest, after its last byte; the file is checked now if its bytes all arrived, else when they do.
    async fn on_file_hash(&mut self, peer: &DeviceId, hash: FileHash) -> Result<(), Error> {
        let unexpected = Error::Unexpected("file-hash");
        let Some(incoming) = self.incoming.get_mut(&hash.transfer).filter(|incoming| incoming.record.peer == *peer)
        else {
            return Err(unexpected);
        };
        let record = incoming.record.file_mut(hash.file).ok_or(Error::Unexpected("file-hash"))?;
        if record.done.is_some() || record.sha256 == hash.sha256 {
            return Ok(());
        }
        if !record.sha256.is_empty() {
            return Err(unexpected);
        }
        record.sha256 = hash.sha256;
        let digest = incoming.unverified.remove(&hash.file);
        self.save(hash.transfer);
        if let Some(digest) = digest {
            self.verify(hash.transfer, hash.file, &digest).await;
        }
        Ok(())
    }

    fn start_send(&mut self, peer: &DeviceId, sources: Vec<Source>) -> Result<TransferId, Error> {
        if sources.is_empty() {
            return Err(Error::Unexpected("a transfer without files"));
        }
        if !self.sessions.get(peer).is_some_and(SessionHandle::is_live) {
            return Err(Error::NotConnected);
        }
        let mut id = TransferId([0; 16]);
        if SystemRandom::new().fill(&mut id.0).is_err() {
            log::error!("system randomness unavailable");
        }
        let mut shared = HashMap::new();
        let mut files = Vec::new();
        for (index, source) in (0..).zip(sources) {
            shared.insert(index, (Arc::new(source.file), source.size));
            let name = if source.name.is_empty() { "file".to_owned() } else { source.name };
            files.push(FileMeta { id: index, name, size: source.size, mime: source.mime, sha256: Vec::new() });
        }
        let to_hash: Vec<(Arc<File>, u64)> = (0..).map_while(|index| shared.get(&index).cloned()).collect();
        let late_hash = self.late_hash.contains(peer);
        let out = Out {
            peer: peer.clone(),
            sources: shared,
            offer: Some(Offer { transfer: id, files }),
            state: None,
            deadline: None,
            created: Instant::now(),
            queue: VecDeque::new(),
            running: HashMap::new(),
            sent: HashMap::new(),
            last_progress: None,
            late_hash,
        };
        self.outgoing.insert(id, out);
        if late_hash {
            let hashes = Ok(vec![Vec::new(); to_hash.len()]);
            self.tasks.spawn(async move { TaskEnd::Hashed { id, hashes } });
        } else {
            self.tasks.spawn(async move { TaskEnd::Hashed { id, hashes: blocking(move || hash_all(&to_hash)).await } });
        }
        self.update_busy(peer);
        Ok(id)
    }

    async fn hashed(&mut self, id: TransferId, hashes: Result<Vec<Vec<u8>>, Error>) {
        let Some(out) = self.outgoing.get_mut(&id) else { return };
        let (Ok(hashes), Some(offer)) =
            (hashes.inspect_err(|error| log::warn!("hashing {id}: {error}")), &mut out.offer)
        else {
            return self.finish_outgoing(id, Status::Failed);
        };
        for (file, hash) in offer.files.iter_mut().zip(hashes) {
            file.sha256 = hash;
        }
        let fits = frame_len(&Message::Offer(offer.clone())) <= MAX_FRAME;
        if !offer.is_valid() || !fits {
            log::warn!("transfer {id}: the offer breaks the rules or does not fit one frame");
            return self.finish_outgoing(id, Status::Failed);
        }
        out.state = Some(Outgoing::new(offer));
        out.deadline = Some(Instant::now() + CONSENT_TIMEOUT + STEP_TIMEOUT);
        let (peer, message) = (out.peer.clone(), Message::Offer(offer.clone()));
        if !self.send_to(&peer, message).await {
            self.finish_outgoing(id, Status::Failed);
        }
    }

    /// Starts streams from the queue while fewer than [`MAX_IN_FLIGHT`] run.
    fn pump(&mut self, id: TransferId) {
        let Self { outgoing, sessions, tasks, ticks_tx, next_seq, .. } = self;
        let Some(out) = outgoing.get_mut(&id) else { return };
        let Some(session) = sessions.get(&out.peer).filter(|session| session.is_live()) else { return };
        while out.running.len() < MAX_IN_FLIGHT {
            let Some(next) = out.queue.pop_front() else { break };
            let Some((file, size)) = out.sources.get(&next.file).cloned() else { continue };
            *next_seq += 1;
            let ticker = Ticker { tx: ticks_tx.clone(), id, file: next.file, seq: *next_seq };
            let header = FileData { transfer: id, file: next.file, offset: next.offset };
            let session = session.clone();
            let late_hash = out.late_hash;
            let abort = tasks.spawn(async move {
                let result = send_file(&session, header, &file, size, &ticker, late_hash).await;
                TaskEnd::Sent { id, file: ticker.file, seq: ticker.seq, result }
            });
            out.running.insert(next.file, (abort, *next_seq));
        }
    }

    async fn task_ended(&mut self, end: TaskEnd) {
        match end {
            TaskEnd::Hashed { id, hashes } => self.hashed(id, hashes).await,
            TaskEnd::Header { peer, recv, header } => self.on_header(&peer, recv, header),
            TaskEnd::Received { id, file, seq, end } => self.on_received(id, file, seq, end).await,
            TaskEnd::ClipServed { peer, result } => {
                if let Err(error) = result {
                    log::info!("{peer}: serving the clipboard: {error}");
                }
            }
            TaskEnd::ClipPulled { peer, header, result } => self.clip_pulled(&peer, &header, result),
            TaskEnd::Sent { id, file, seq, result } => {
                let Some(out) = self.outgoing.get_mut(&id) else { return };
                if out.running.get(&file).is_some_and(|(_, running)| *running == seq) {
                    out.running.remove(&file);
                }
                let peer = out.peer.clone();
                match result {
                    Ok(Some(sha256)) => {
                        self.send_to(&peer, Message::FileHash(FileHash { transfer: id, file, sha256 })).await;
                    }
                    Ok(None) => {}
                    // A lost connection resumes the file on reconnect; resume runs only then, so any other
                    // failure would leave the file unfinished on a live session forever.
                    Err(Error::Closed(_) | Error::Timeout | Error::Connection(_)) => {
                        log::info!("{peer}: sending file {file} of {id} stopped with the connection");
                    }
                    Err(error) => {
                        log::info!("{peer}: sending file {file} of {id} failed: {error}");
                        self.send_to(&peer, Message::Cancel(TransferRef { transfer: id })).await;
                        self.finish_outgoing(id, Status::Failed);
                        return;
                    }
                }
                self.pump(id);
            }
        }
    }

    fn on_header(&mut self, peer: &DeviceId, mut recv: RecvStream, header: Result<Envelope, Error>) {
        let header = match header {
            Ok(Envelope { message: Message::FileData(header), .. }) => header,
            Ok(Envelope { message: Message::ClipData(header), .. }) => return self.on_clip_data(peer, recv, header),
            Ok(Envelope { message: Message::MirrorData(header), .. }) => {
                if let Some(gop) = header.gop {
                    match self.mirrors.get(peer).filter(|parts| !parts.is_closed()) {
                        Some(parts) => parts.add(gop, recv),
                        None => recv.stop(0),
                    }
                    return;
                }
                let (receiver, parts) = crate::mirror::MirrorReceiver::new(recv, &header);
                self.mirrors.insert(peer.clone(), parts);
                let stream = crate::mirror::MirrorStream::new(receiver);
                return self.emit(TransferEvent::Mirror { from: peer.clone(), stream });
            }
            // A mirror's video stream may be reset before its header arrives.
            Err(Error::Timeout | Error::StreamEnded) => {
                recv.stop(PROTOCOL_ERROR);
                return;
            }
            Ok(Envelope { message, .. }) => return self.violation(peer, &Error::Unexpected(message.kind())),
            Err(error) => return self.violation(peer, &error),
        };
        let Self { incoming, tasks, ticks_tx, next_seq, .. } = self;
        let admitted = incoming
            .get_mut(&header.transfer)
            .filter(|incoming| incoming.record.peer == *peer)
            .ok_or(link_proto::transfer::StreamRefused::NotAccepted)
            .and_then(|incoming| Ok((incoming.state.on_stream(&header)?, incoming)));
        let (budget, incoming) = match admitted {
            Ok(admitted) => admitted,
            Err(refused) => {
                log::warn!("{peer}: stream for {} file {} refused: {refused}", header.transfer, header.file);
                recv.stop(PROTOCOL_ERROR);
                return;
            }
        };
        let Some(record) = incoming.record.files.iter().find(|file| file.id == header.file) else { return };
        let part = record.part.clone();
        *next_seq += 1;
        let ticker = Ticker { tx: ticks_tx.clone(), id: header.transfer, file: header.file, seq: *next_seq };
        let abort = tasks.spawn(async move {
            let end = receive(recv, &part, budget, &ticker).await;
            TaskEnd::Received { id: ticker.id, file: ticker.file, seq: ticker.seq, end }
        });
        incoming.running.insert(header.file, (abort, *next_seq));
    }

    fn violation(&self, peer: &DeviceId, error: &Error) {
        log::warn!("{peer}: bulk stream: {error}");
        if let Some(session) = self.sessions.get(peer) {
            session.close(CloseCode::ProtocolError);
        }
    }

    async fn on_received(&mut self, id: TransferId, file: u64, seq: u64, end: Received) {
        let Some(incoming) = self.incoming.get_mut(&id) else { return };
        if incoming.running.get(&file).is_none_or(|(_, running)| *running != seq) {
            return;
        }
        incoming.running.remove(&file);
        let peer = incoming.record.peer.clone();
        let Some(record) = incoming.record.file_mut(file) else { return };
        let ok = match end {
            Received::Paused { durable } => {
                record.durable = durable;
                incoming.state.stream_ended(file, durable);
                self.save(id);
                return;
            }
            Received::Complete { digest } if record.sha256.is_empty() => {
                record.durable = record.size;
                incoming.state.stream_ended(file, record.size);
                incoming.unverified.insert(file, digest);
                self.save(id);
                return;
            }
            Received::Complete { digest } => return self.verify(id, file, &digest).await,
            Received::Overrun(overrun) => {
                log::warn!("{peer}: {}: {overrun}", record.name);
                false
            }
            Received::Failed(error) => {
                log::error!("writing {}: {error}", record.name);
                false
            }
        };
        self.file_ended(id, file, ok).await;
    }

    /// The file's bytes all arrived and its expected digest is known: publish it when they match.
    async fn verify(&mut self, id: TransferId, file: u64, digest: &[u8]) {
        let Some(incoming) = self.incoming.get_mut(&id) else { return };
        let peer = &incoming.record.peer;
        let Some(record) = incoming.record.files.iter_mut().find(|record| record.id == file) else { return };
        let ok = if record.sha256 == digest {
            match self.inbox.publish(&record.part, &record.name) {
                Ok(path) => {
                    log::info!("{peer}: received {}", path.display());
                    record.path = Some(path);
                    true
                }
                Err(error) => {
                    log::error!("publishing {}: {error}", record.name);
                    false
                }
            }
        } else {
            log::warn!("{peer}: {} does not match its hash", record.name);
            false
        };
        self.file_ended(id, file, ok).await;
    }

    async fn file_ended(&mut self, id: TransferId, file: u64, ok: bool) {
        let Some(incoming) = self.incoming.get_mut(&id) else { return };
        let peer = incoming.record.peer.clone();
        let Some(record) = incoming.record.file_mut(file) else { return };
        if !ok {
            inbox::remove_quietly(&record.part);
        }
        record.done = Some(ok);
        record.durable = record.size;
        let outcome = incoming.state.finish(file, ok);
        self.save(id);
        self.send_to(&peer, Message::FileDone(FileDone { transfer: id, file, ok })).await;
        if let Some(all_ok) = outcome {
            self.finish_incoming(id, if all_ok { Status::Done } else { Status::Failed });
        } else {
            self.progress(id, true);
        }
    }

    fn tick(&mut self, tick: &Tick) {
        if let Some(incoming) = self.incoming.get_mut(&tick.id) {
            if incoming.running.get(&tick.file).is_none_or(|(_, seq)| *seq != tick.seq) {
                return;
            }
            incoming.received.insert(tick.file, tick.offset);
            if tick.durable
                && let Some(record) = incoming.record.file_mut(tick.file)
            {
                record.durable = tick.offset;
                self.save(tick.id);
            }
        } else if let Some(out) = self.outgoing.get_mut(&tick.id) {
            if out.running.get(&tick.file).is_none_or(|(_, seq)| *seq != tick.seq) {
                return;
            }
            out.sent.insert(tick.file, tick.offset);
        }
        self.progress(tick.id, false);
    }

    /// Emits the transfer's progress unless it did within [`PROGRESS_EVERY`], or `now` is set.
    fn progress(&mut self, id: TransferId, now: bool) {
        let due = |last: &mut Option<Instant>| {
            let due = now || last.is_none_or(|last| last.elapsed() >= PROGRESS_EVERY);
            if due {
                *last = Some(Instant::now());
            }
            due
        };
        let (bytes, total) = if let Some(incoming) = self.incoming.get_mut(&id) {
            if !due(&mut incoming.last_progress) {
                return;
            }
            let files = &incoming.record.files;
            let at = |file: &RecordFile| match file.done {
                Some(_) => file.size,
                None => incoming.received.get(&file.id).copied().unwrap_or(file.durable),
            };
            (files.iter().map(at).sum(), files.iter().map(|file| file.size).sum())
        } else if let Some(out) = self.outgoing.get_mut(&id) {
            if !due(&mut out.last_progress) {
                return;
            }
            let (Some(offer), Some(state)) = (&out.offer, &out.state) else { return };
            let at = |file: &FileMeta| {
                if state.is_done(file.id) { file.size } else { out.sent.get(&file.id).copied().unwrap_or(0) }
            };
            (offer.files.iter().map(at).sum(), offer.total_size())
        } else {
            return;
        };
        self.emit(TransferEvent::Progress { id, bytes, total });
    }

    fn next_deadline(&self) -> Option<Instant> {
        let incoming = self.incoming.values().filter_map(|incoming| incoming.deadline);
        let outgoing = self.outgoing.values().filter_map(|out| out.deadline);
        incoming.chain(outgoing).chain(self.clips.next_deadline()).min()
    }

    /// Unanswered offers: the receiver declines them, the sender gives up.
    async fn deadlines(&mut self) {
        self.clip_deadlines();
        let now = Instant::now();
        let due = |deadline: Option<Instant>| deadline.is_some_and(|deadline| deadline <= now);
        let unanswered_in: Vec<(TransferId, DeviceId)> = self
            .incoming
            .iter()
            .filter(|(_, incoming)| due(incoming.deadline))
            .map(|(id, incoming)| (*id, incoming.record.peer.clone()))
            .collect();
        for (id, peer) in unanswered_in {
            log::info!("{peer}: no answer to transfer {id}");
            self.send_to(&peer, Message::OfferReply(declined(id))).await;
            self.finish_incoming(id, Status::Declined);
        }
        let unanswered: Vec<TransferId> =
            self.outgoing.iter().filter(|(_, out)| due(out.deadline)).map(|(id, _)| *id).collect();
        for id in unanswered {
            self.finish_outgoing(id, Status::Failed);
        }
    }

    /// Drops transfers older than [`KEEP`].
    fn expire(&mut self) {
        let old_out: Vec<TransferId> =
            self.outgoing.iter().filter(|(_, out)| out.created.elapsed() > KEEP).map(|(id, _)| *id).collect();
        for id in old_out {
            self.finish_outgoing(id, Status::Failed);
        }
        let cutoff = inbox::now().saturating_sub(KEEP.as_secs());
        let old_in: Vec<TransferId> =
            self.incoming.iter().filter(|(_, incoming)| incoming.record.created < cutoff).map(|(id, _)| *id).collect();
        for id in old_in {
            self.finish_incoming(id, Status::Failed);
        }
    }

    async fn cancel(&mut self, id: TransferId, tell_peer: bool) -> bool {
        let peer = match (self.outgoing.get(&id), self.incoming.get(&id)) {
            (Some(out), _) => out.peer.clone(),
            (None, Some(incoming)) => incoming.record.peer.clone(),
            (None, None) => return false,
        };
        if tell_peer {
            self.send_to(&peer, Message::Cancel(TransferRef { transfer: id })).await;
        }
        if self.outgoing.contains_key(&id) {
            self.finish_outgoing(id, Status::Cancelled);
        } else {
            self.finish_incoming(id, Status::Cancelled);
        }
        true
    }

    fn finish_outgoing(&mut self, id: TransferId, status: Status) {
        let Some(out) = self.outgoing.remove(&id) else { return };
        for (task, _) in out.running.values() {
            task.abort();
        }
        log::info!("{}: transfer {id} to it: {}", out.peer, status.as_str());
        let event = TransferEvent::Finished { id, peer: out.peer.clone(), incoming: false, status, files: Vec::new() };
        self.emit(event);
        self.update_busy(&out.peer);
    }

    fn finish_incoming(&mut self, id: TransferId, status: Status) {
        let Some(incoming) = self.incoming.remove(&id) else { return };
        for (task, _) in incoming.running.values() {
            task.abort();
        }
        self.inbox.discard(&incoming.record);
        let peer = incoming.record.peer;
        log::info!("{peer}: transfer {id} from it: {}", status.as_str());
        let files = incoming
            .record
            .files
            .into_iter()
            .filter_map(|file| Some(ReceivedFile { path: file.path?, sha256: file.sha256 }))
            .collect();
        self.emit(TransferEvent::Finished { id, peer: peer.clone(), incoming: true, status, files });
        self.update_busy(&peer);
    }

    fn update_busy(&mut self, peer: &DeviceId) {
        let busy = self.outgoing.values().any(|out| out.peer == *peer)
            || self.incoming.values().any(|incoming| incoming.record.peer == *peer);
        let changed = if busy { self.busy.insert(peer.clone()) } else { self.busy.remove(peer) };
        if changed {
            self.emit(TransferEvent::Busy { peer: peer.clone(), busy });
        }
    }

    fn save(&self, id: TransferId) {
        let Some(incoming) = self.incoming.get(&id).filter(|incoming| incoming.state.is_accepted()) else { return };
        if let Err(error) = self.inbox.save(&incoming.record) {
            log::error!("saving transfer {id}: {error}");
        }
    }

    /// Sends on the peer's live session; false when there is none or it failed.
    async fn send_to(&self, peer: &DeviceId, message: Message) -> bool {
        let Some(session) = self.sessions.get(peer).filter(|session| session.is_live()) else {
            log::info!("{peer}: not connected for {}", message.kind());
            return false;
        };
        let kind = message.kind();
        let sent = tokio::time::timeout(STEP_TIMEOUT, session.send(message)).await.map_err(Error::from);
        match sent.and_then(|sent| sent) {
            Ok(()) => true,
            Err(error) => {
                log::info!("{peer}: sending {kind}: {error}");
                false
            }
        }
    }

    fn emit(&self, event: TransferEvent) {
        if self.events.send(event).is_err() {
            log::debug!("nobody receives transfer events");
        }
    }
}

impl Ticker {
    fn progress(&self, offset: u64) {
        drop(self.tx.try_send(self.tick(offset, false)));
    }

    async fn durable(&self, offset: u64) {
        drop(self.tx.send(self.tick(offset, true)).await);
    }

    fn tick(&self, offset: u64, durable: bool) -> Tick {
        Tick { id: self.id, file: self.file, seq: self.seq, offset, durable }
    }
}

/// Resets the stream when dropped unfinished, since a dropped stream finishes and the receiver would take a
/// cancelled file for a short one.
struct Outbound {
    stream: SendStream,
    finished: bool,
}

impl Drop for Outbound {
    fn drop(&mut self) {
        if !self.finished {
            self.stream.reset(CANCELLED);
        }
    }
}

/// Sends the file from the header's offset; the next batch is read while the current one is written to the stream.
/// With `late_hash` it reads from the start and returns the whole file's digest; bytes before the offset are only
/// hashed.
async fn send_file(
    session: &SessionHandle,
    header: FileData,
    file: &Arc<File>,
    size: u64,
    ticker: &Ticker,
    late_hash: bool,
) -> Result<Option<Vec<u8>>, Error> {
    let resume_at = header.offset;
    let mut offset = if late_hash { 0 } else { resume_at };
    let mut hasher = late_hash.then(|| digest::Context::new(&digest::SHA256));
    let stream = session.connection().open_uni().await?;
    let mut out = Outbound { stream, finished: false };
    write_frame(&mut out.stream, Message::FileData(header), session.tap()).await?;
    let mut next = (offset < size).then(|| read_batch(file.clone(), offset, size, Vec::new(), hasher.take()));
    while let Some(reading) = next.take() {
        let (buf, context) = reading.await.map_err(joined)??;
        hasher = context;
        let start = offset;
        offset += buf.len() as u64;
        let spare = if offset < size { Vec::with_capacity(BATCH) } else { Vec::new() };
        next = (offset < size).then(|| read_batch(file.clone(), offset, size, spare, hasher.take()));
        let skip = usize::try_from(resume_at.saturating_sub(start)).map_or(buf.len(), |skip| skip.min(buf.len()));
        if skip < buf.len() {
            out.stream.write_all(&buf[skip..]).await?;
            ticker.progress(offset);
        }
    }
    out.stream.finish()?;
    out.finished = true;
    Ok(hasher.map(|context| context.finish().as_ref().to_vec()))
}

/// A batch's bytes and the digest context that has hashed them, when the file is hashed while sent.
type Batch = (Vec<u8>, Option<digest::Context>);

/// Reads the batch at `offset` into `buf` and adds it to `hasher`, off the runtime thread.
fn read_batch(
    file: Arc<File>,
    offset: u64,
    size: u64,
    mut buf: Vec<u8>,
    mut hasher: Option<digest::Context>,
) -> tokio::task::JoinHandle<Result<Batch, Error>> {
    tokio::task::spawn_blocking(move || {
        let want = usize::try_from(size - offset).map_or(BATCH, |left| left.min(BATCH));
        buf.resize(want, 0);
        let mut filled = 0;
        while filled < want {
            let read = file.read_at(&mut buf[filled..], offset + filled as u64)?;
            if read == 0 {
                return Err(Error::Io(std::io::Error::other("the file shrank while it was sent")));
            }
            filled += read;
        }
        if let Some(hasher) = &mut hasher {
            hasher.update(&buf);
        }
        Ok((buf, hasher))
    })
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    tokio::task::spawn_blocking(work).await.map_err(joined)?
}

fn joined(error: tokio::task::JoinError) -> Error {
    Error::Io(std::io::Error::other(error))
}

/// Writes `batch` to the part off the runtime thread, and syncs when `sync` is set or [`SYNC_EVERY`] bytes passed since
/// `synced`; returns the part, the emptied batch and the durable offset. An aborted receive leaves a running write to
/// finish: it writes the sender's own bytes at their own offsets, so a resumed part gets the same content there.
async fn flush(mut part: Part, mut batch: Vec<u8>, synced: u64, sync: bool) -> Result<(Part, Vec<u8>, u64), Error> {
    blocking(move || {
        part.write(&batch)?;
        batch.clear();
        let synced = if sync || part.offset() - synced >= SYNC_EVERY { part.sync()? } else { synced };
        Ok((part, batch, synced))
    })
    .await
}

async fn receive(mut recv: RecvStream, part: &std::path::Path, mut budget: Budget, ticker: &Ticker) -> Received {
    let (path, offset) = (part.to_path_buf(), budget.offset());
    let mut part = match blocking(move || Part::open(&path, offset)).await {
        Ok(part) => part,
        Err(error) => {
            recv.stop(PROTOCOL_ERROR);
            return Received::Failed(error);
        }
    };
    let mut synced = part.offset();
    let mut batch = Vec::with_capacity(BATCH);
    loop {
        let chunk = match recv.read_chunk(CHUNK).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                let durable = flush(part, batch, synced, true).await.map_or(synced, |(_, _, durable)| durable);
                return Received::Paused { durable };
            }
        };
        if let Err(overrun) = budget.take(chunk.len()) {
            recv.stop(PROTOCOL_ERROR);
            return Received::Overrun(overrun);
        }
        batch.extend_from_slice(&chunk);
        if batch.len() < BATCH {
            continue;
        }
        let before = synced;
        (part, batch, synced) = match flush(part, batch, synced, false).await {
            Ok(flushed) => flushed,
            Err(error) => {
                recv.stop(PROTOCOL_ERROR);
                return Received::Failed(error);
            }
        };
        if synced == before {
            ticker.progress(part.offset());
        } else {
            ticker.durable(synced).await;
        }
    }
    if let Err(overrun) = budget.finish() {
        return Received::Overrun(overrun);
    }
    match flush(part, batch, synced, true).await {
        Ok((part, _, _)) => Received::Complete { digest: part.digest() },
        Err(error) => Received::Failed(error),
    }
}

fn hash_all(files: &[(Arc<File>, u64)]) -> Result<Vec<Vec<u8>>, Error> {
    let mut hashes = Vec::with_capacity(files.len());
    let mut buf = vec![0; BATCH];
    for (file, size) in files {
        let mut context = digest::Context::new(&digest::SHA256);
        let mut offset = 0;
        while offset < *size {
            let want = usize::try_from(size - offset).map_or(BATCH, |left| left.min(BATCH));
            let read = file.read_at(&mut buf[..want], offset)?;
            if read == 0 {
                return Err(Error::Io(std::io::Error::other("the file shrank while it was hashed")));
            }
            context.update(&buf[..read]);
            offset += read as u64;
        }
        hashes.push(context.finish().as_ref().to_vec());
    }
    Ok(hashes)
}

fn frame_len(message: &Message) -> usize {
    Envelope::new(0, message.clone()).to_cbor().len()
}

fn declined(id: TransferId) -> OfferReply {
    OfferReply { transfer: id, accepted: false, reason: Some(RefuseReason::Declined) }
}

fn create_parts(inbox: &Inbox, record: &mut Record) -> Result<(), Error> {
    for file in &mut record.files {
        file.part = inbox.create_part(&file.name)?;
    }
    Ok(())
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file send that ends with the connection keeps the accepted transfer for the reconnect's resume; any other
    /// failure ends the transfer as failed, so it never stays accepted with an unfinished file on a live session.
    #[tokio::test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    async fn a_failed_file_send_ends_the_transfer_unless_the_connection_dropped() {
        let dir = std::env::temp_dir().join(format!("link-transfer-send-failed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let (_handle, mut actor, mut events) = transfers(Inbox::new(dir.clone(), &dir).expect("inbox"));
        let peer = crate::identity::Identity::generate().expect("key").device_id();
        let cases = [
            (Error::Timeout, true),
            (Error::Closed(CloseCode::Done), true),
            (Error::StreamEnded, false),
            (Error::Io(std::io::Error::other("source gone")), false),
        ];
        for (index, (error, kept)) in (0u8..).zip(cases) {
            let id = TransferId([index; 16]);
            let out = Out {
                peer: peer.clone(),
                sources: HashMap::new(),
                offer: None,
                state: None,
                deadline: None,
                created: Instant::now(),
                queue: VecDeque::new(),
                running: HashMap::new(),
                sent: HashMap::new(),
                last_progress: None,
                late_hash: false,
            };
            actor.outgoing.insert(id, out);
            let message = error.to_string();
            actor.task_ended(TaskEnd::Sent { id, file: 0, seq: 1, result: Err(error) }).await;
            assert_eq!(actor.outgoing.contains_key(&id), kept, "{message}");
            let mut failed = false;
            while let Ok(event) = events.try_recv() {
                failed |=
                    matches!(event, TransferEvent::Finished { id: done, status: Status::Failed, .. } if done == id);
            }
            assert_eq!(failed, !kept, "{message}");
        }
        drop(std::fs::remove_dir_all(&dir));
    }
}
