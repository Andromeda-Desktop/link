//! Shares queued per peer until the peer acknowledges them. The queues are saved to disk; a live session sends the
//! oldest item, an ack removes it, and a failure keeps it for a retry or the next live session. Each item carries a
//! random id that the receiver remembers in [`Seen`], so a resend after a lost ack is applied once.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use link_proto::message::Share;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::Error;
use crate::control::STEP_TIMEOUT;
use crate::identity::DeviceId;
use crate::session::SessionHandle;

/// How a share call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// The peer acknowledged it.
    Delivered,
    /// It waits in the outbox for the peer's next live session.
    Queued,
}

/// After a failed send on a session that is still live, the next try waits this long.
const RETRY: Duration = Duration::from_secs(5);

/// How many item ids a receiver remembers per peer.
const SEEN: usize = 256;

/// The queues as saved: per peer, oldest first, each share with its item id.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Queues(BTreeMap<DeviceId, VecDeque<Share>>);

impl Queues {
    /// A missing file is empty; a file that does not parse is logged and dropped.
    fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                log::warn!("{}: dropping an outbox that does not parse: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                log::warn!("{}: reading the outbox: {error}", path.display());
                Self::default()
            }
        }
    }

    /// Writes atomically: a mode-0600 temporary file renamed over `path`.
    fn save(&self, path: &Path) -> Result<(), Error> {
        let tmp = path.with_extension("tmp");
        let mut file = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    fn push(&mut self, peer: &DeviceId, mut share: Share, item: u64) {
        share.item = Some(item);
        self.0.entry(peer.clone()).or_default().push_back(share);
    }

    fn front(&self, peer: &DeviceId) -> Option<&Share> {
        self.0.get(peer).and_then(VecDeque::front)
    }

    /// Drops `item` from `peer`'s queue; false when it was not queued.
    fn remove(&mut self, peer: &DeviceId, item: u64) -> bool {
        let Some(queue) = self.0.get_mut(peer) else { return false };
        let before = queue.len();
        queue.retain(|share| share.item != Some(item));
        let removed = queue.len() < before;
        if queue.is_empty() {
            self.0.remove(peer);
        }
        removed
    }
}

/// What a receiver remembers to apply each item once. Memory only: after a restart, one share resent after a lost
/// ack can be applied twice.
#[derive(Debug, Default)]
pub struct Seen(HashMap<DeviceId, VecDeque<u64>>);

impl Seen {
    /// Whether `share` from `peer` is new; a share without an item id always is.
    pub fn first(&mut self, peer: &DeviceId, share: &Share) -> bool {
        let Some(item) = share.item else { return true };
        let seen = self.0.entry(peer.clone()).or_default();
        if seen.contains(&item) {
            return false;
        }
        if seen.len() == SEEN {
            seen.pop_front();
        }
        seen.push_back(item);
        true
    }
}

/// A peer's session as the outbox sees it. `items`: the peer announced [`link_proto::message::SHARE_OUTBOX`].
enum Link {
    Down,
    Idle { session: SessionHandle, items: bool },
    Sending { session: SessionHandle, items: bool, item: u64 },
}

enum Command {
    Share { peer: DeviceId, share: Share, reply: oneshot::Sender<Result<Sent, Error>> },
    Live { peer: DeviceId, session: SessionHandle, items: bool },
    Sent { peer: DeviceId, item: u64, result: Result<(), Error> },
    Retry { peer: DeviceId },
    Forget { peer: DeviceId },
}

#[derive(Clone)]
pub struct Outbox(mpsc::UnboundedSender<Command>);

pub struct OutboxActor {
    path: PathBuf,
    queues: Queues,
    links: HashMap<DeviceId, Link>,
    waiting: HashMap<u64, oneshot::Sender<Result<Sent, Error>>>,
    commands: mpsc::UnboundedReceiver<Command>,
    me: mpsc::WeakUnboundedSender<Command>,
}

/// The outbox saved at `path`; run the actor on the runtime.
pub fn outbox(path: PathBuf) -> (Outbox, OutboxActor) {
    let (tx, commands) = mpsc::unbounded_channel();
    let queues = Queues::load(&path);
    let actor =
        OutboxActor { path, queues, links: HashMap::new(), waiting: HashMap::new(), commands, me: tx.downgrade() };
    (Outbox(tx), actor)
}

impl Outbox {
    /// Queues `share` for `peer`, then waits up to [`STEP_TIMEOUT`] for its ack.
    pub async fn share(&self, peer: DeviceId, share: Share) -> Result<Sent, Error> {
        share.check()?;
        let (reply, sent) = oneshot::channel();
        self.0.send(Command::Share { peer, share, reply }).map_err(|_| Error::Unexpected("the outbox stopped"))?;
        match tokio::time::timeout(STEP_TIMEOUT, sent).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) | Err(_) => Ok(Sent::Queued),
        }
    }

    /// Drops `peer`'s queue: it is no longer paired.
    pub fn forget(&self, peer: DeviceId) {
        drop(self.0.send(Command::Forget { peer }));
    }

    /// `session` to `peer` is live: queued shares go out on it.
    pub fn live(&self, peer: DeviceId, session: SessionHandle, items: bool) {
        drop(self.0.send(Command::Live { peer, session, items }));
    }
}

impl OutboxActor {
    pub async fn run(mut self) {
        while let Some(command) = self.commands.recv().await {
            self.on(command);
        }
    }

    fn on(&mut self, command: Command) {
        match command {
            Command::Share { peer, share, reply } => {
                let item = match random_item() {
                    Ok(item) => item,
                    Err(error) => return drop(reply.send(Err(error))),
                };
                self.queues.push(&peer, share, item);
                if let Err(error) = self.queues.save(&self.path) {
                    self.queues.remove(&peer, item);
                    return drop(reply.send(Err(error)));
                }
                if matches!(self.links.get(&peer), None | Some(Link::Down)) {
                    drop(reply.send(Ok(Sent::Queued)));
                } else {
                    self.waiting.insert(item, reply);
                }
                self.pump(&peer);
            }
            Command::Live { peer, session, items } => {
                if let Some(Link::Sending { session: current, .. }) = self.links.get(&peer)
                    && current.stable_id() == session.stable_id()
                {
                    return;
                }
                self.links.insert(peer.clone(), Link::Idle { session, items });
                self.pump(&peer);
            }
            Command::Sent { peer, item, result } => self.on_sent(&peer, item, result),
            Command::Retry { peer } => self.pump(&peer),
            Command::Forget { peer } => {
                self.links.remove(&peer);
                if self.queues.0.remove(&peer).is_some()
                    && let Err(error) = self.queues.save(&self.path)
                {
                    log::warn!("{peer}: saving the outbox: {error}");
                }
            }
        }
    }

    fn on_sent(&mut self, peer: &DeviceId, item: u64, result: Result<(), Error>) {
        let current = match self.links.remove(peer) {
            Some(Link::Sending { session, items, item: sending }) if sending == item => Some((session, items)),
            Some(other) => {
                self.links.insert(peer.clone(), other);
                None
            }
            None => None,
        };
        match result {
            Ok(()) => {
                if self.queues.remove(peer, item)
                    && let Err(error) = self.queues.save(&self.path)
                {
                    log::warn!("{peer}: saving the outbox: {error}");
                }
                if let Some(reply) = self.waiting.remove(&item) {
                    drop(reply.send(Ok(Sent::Delivered)));
                }
                if let Some((session, items)) = current {
                    self.links.insert(peer.clone(), Link::Idle { session, items });
                }
            }
            Err(error) => {
                log::info!("{peer}: share {item:016x} not acknowledged: {error}; kept in the outbox");
                if let Some(reply) = self.waiting.remove(&item) {
                    drop(reply.send(Ok(Sent::Queued)));
                }
                if let Some((session, items)) = current {
                    if session.is_live() {
                        self.links.insert(peer.clone(), Link::Idle { session, items });
                        self.retry_later(peer.clone());
                    } else {
                        self.links.insert(peer.clone(), Link::Down);
                    }
                }
                return;
            }
        }
        self.pump(peer);
    }

    /// Sends `peer`'s oldest share when its session is idle and live.
    fn pump(&mut self, peer: &DeviceId) {
        let Some(Link::Idle { session, items }) = self.links.get(peer) else { return };
        let (session, items) = (session.clone(), *items);
        if !session.is_live() {
            self.links.insert(peer.clone(), Link::Down);
            return;
        }
        let Some(mut share) = self.queues.front(peer).cloned() else { return };
        let Some(item) = share.item else { return };
        if !items {
            share.item = None;
        }
        self.links.insert(peer.clone(), Link::Sending { session: session.clone(), items, item });
        let (me, peer) = (self.me.clone(), peer.clone());
        tokio::spawn(async move {
            let result = session.share(share).await;
            if let Some(me) = me.upgrade() {
                drop(me.send(Command::Sent { peer, item, result }));
            }
        });
    }

    fn retry_later(&self, peer: DeviceId) {
        let me = self.me.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RETRY).await;
            if let Some(me) = me.upgrade() {
                drop(me.send(Command::Retry { peer }));
            }
        });
    }
}

fn random_item() -> Result<u64, Error> {
    let mut bytes = [0; 8];
    SystemRandom::new().fill(&mut bytes).map_err(|_| Error::Unexpected("no randomness for an outbox item"))?;
    Ok(u64::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use link_proto::message::ShareKind;

    use super::*;

    /// Random queue, send, ack-loss and restart sequences, as the actor drives them: once every send succeeds, the
    /// receiver has applied each queued share exactly once, in queue order, and the queue is empty.
    #[test]
    fn random_sends_apply_each_share_once() {
        let mut seed = 0x6a09_e667_f3bc_c908_u64;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let peer = DeviceId::from_fingerprint(&[7; 32]);
        let path = std::env::temp_dir().join(format!("link-outbox-test-{}.json", std::process::id()));
        for _ in 0..500 {
            let mut queues = Queues::default();
            let (mut receiver, mut applied, mut made) = (Seen::default(), Vec::new(), Vec::new());
            let mut item = 0;
            for _ in 0..40 {
                match next(6) {
                    0 | 1 => {
                        item += 1;
                        queues.push(&peer, Share { kind: ShareKind::Text, text: format!("{item}"), item: None }, item);
                        made.push(item);
                    }
                    2 => {
                        queues.save(&path).unwrap_or_else(|error| panic!("{error}"));
                        queues = Queues::load(&path);
                    }
                    outcome => {
                        let Some(share) = queues.front(&peer).cloned() else { continue };
                        // 3: lost on the way; 4: applied but the ack is lost; 5: applied and acked.
                        if outcome >= 4 && receiver.first(&peer, &share) {
                            applied.push(share.item);
                        }
                        if outcome == 5 {
                            queues.remove(&peer, share.item.unwrap_or_default());
                        }
                    }
                }
            }
            while let Some(share) = queues.front(&peer).cloned() {
                if receiver.first(&peer, &share) {
                    applied.push(share.item);
                }
                queues.remove(&peer, share.item.unwrap_or_default());
            }
            assert_eq!(applied, made.into_iter().map(Some).collect::<Vec<_>>());
            assert_eq!(queues, Queues::default());
        }
        drop(fs::remove_file(&path));
    }
}
