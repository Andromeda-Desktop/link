use crate::path::{FdLink, LinkKind, StreamLink};

use super::{
    ClientActor, ClientEvent, CloseCode, DeviceId, Duration, Ended, Error, FIRST_RETRY, Hop, Instant, LinkId,
    MAX_RETRY, NetworkKind, OPEN_TTLS, OwnedFd, PROBE_EVERY, Peer, Reached, Relay, Retry, SessionHandle, Via,
    close_code_for, oneshot, punch, reach, slot,
};
use crate::phone::Opened;

/// How a dial starts.
pub(super) enum Start {
    /// IP, else USB, Bluetooth, then the relay.
    Any,
    /// Bluetooth only: Wi-Fi just went away.
    Bluetooth,
    /// The IP path a probe found for a session on Bluetooth or the relay.
    Reached(Reached),
}

/// The path a dial in flight tries, which decides whether its session replaces a live one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Over {
    /// Every path: a live session stays and the new one closes.
    Any,
    /// Bluetooth after Wi-Fi went away: replaces a live session that is not on Bluetooth.
    Bluetooth,
    /// A probe's IP path: replaces a live session on Bluetooth or the relay.
    Probe,
}

/// A dial in flight to one desktop.
pub(super) struct Dial {
    over: Over,
    task: tokio::task::Id,
    /// The backoff of the redial that started it or joined it; a failure doubles it.
    pub(super) redial: Option<Duration>,
    /// Commands answered with the session, or the error, when it ends.
    pub(super) waiters: Vec<oneshot::Sender<Result<SessionHandle, Error>>>,
}

impl ClientActor {
    /// A Bluetooth standby waits for Wi-Fi to go; a USB link takes the session at once.
    pub(super) async fn standby_opened(
        &mut self,
        id: DeviceId,
        stable_id: usize,
        via: Via,
        opened: Result<OwnedFd, Error>,
    ) {
        let Some(quic) = self
            .sessions
            .get(&id)
            .filter(|handle| handle.stable_id() == stable_id)
            .map(|handle| handle.connection().quic().clone())
        else {
            return;
        };
        let added = opened.and_then(|fd| match via {
            Via::Usb => self.phone.add_standby(StreamLink::new(fd, LinkKind::Usb)?, quic),
            _ => self.phone.add_standby(FdLink::new(fd)?, quic),
        });
        match (added, via) {
            (Ok(link), Via::Usb) => {
                let ended = self.phone.socket().link_closed(link);
                self.usb_ends.spawn({
                    let id = id.clone();
                    async move {
                        ended.await;
                        (id, stable_id)
                    }
                });
                self.move_to_standby(id, (link, via), "the USB cable reaches the desktop").await;
            }
            (Ok(link), _) => {
                log::info!("{id}: standby {via:?} link up");
                self.standby.insert(id, (stable_id, link, via));
            }
            (Err(error), _) => log::info!("{id}: no standby {via:?} link: {error}"),
        }
    }

    /// The USB link under `id`'s session ended and its route went back to UDP: a heartbeat shows the desktop the
    /// path before it gives up on the session.
    pub(super) async fn usb_ended(&mut self, id: DeviceId, stable_id: usize) {
        let Some(handle) = self.sessions.get(&id).filter(|handle| handle.stable_id() == stable_id).cloned() else {
            return;
        };
        let Some(Hop::Udp(addr)) = handle.path().and_then(crate::path::Route::via) else { return };
        log::info!("{id}: the USB cable is gone; the session is back at {addr}");
        handle.wake();
        self.path_changed(&id, Via::LastKnown, Some(addr)).await;
        if let Some(desktop) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() {
            self.emit(ClientEvent::Connected { desktop, addr: Some(addr), via: Via::LastKnown, resumed: false });
        }
    }

    /// The desktop says this phone's USB cable reaches it: a live session not on USB yet moves there once a pinned
    /// handshake over the cable proves it is the same desktop.
    pub(super) fn usb_ready(&mut self, id: DeviceId) {
        let Some(handle) = self.sessions.get(&id).filter(|handle| handle.path().is_some()) else { return };
        if handle.link_kind() == Some(LinkKind::Usb) {
            return;
        }
        let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() else { return };
        let stable_id = handle.stable_id();
        match self.phone.usb_probe(&peer) {
            Ok(probe) => drop(self.standbys.spawn(async move { (id, stable_id, Via::Usb, probe.await) })),
            Err(error) => log::info!("{id}: no USB probe: {error}"),
        }
    }

    /// The standby link `id`'s live IP session can move onto now.
    pub(super) fn standby_for(&self, id: &DeviceId) -> Option<(LinkId, Via)> {
        let &(stable_id, link, via) = self.standby.get(id)?;
        let handle = self.sessions.get(id)?;
        let elsewhere = handle.path().is_some_and(|path| path.via() != Some(Hop::Link(link)));
        (handle.stable_id() == stable_id && elsewhere && self.phone.has_link(link)).then_some((link, via))
    }

    /// Moves `id`'s session onto its standby link, keeping its streams.
    pub(super) async fn move_to_standby(&mut self, id: DeviceId, (link, via): (LinkId, Via), why: &str) {
        let Some(handle) = self.sessions.get(&id).cloned() else { return };
        let Some(path) = handle.path() else { return };
        log::info!("{id}: {why}; moving the session to its standby {via:?} link");
        path.set(Hop::Link(link));
        handle.wake();
        self.path_changed(&id, via, None).await;
        if let Some(desktop) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() {
            self.emit(ClientEvent::Connected { desktop, addr: None, via, resumed: false });
        }
    }

    pub(super) async fn ended(&mut self, ended: Ended) {
        let Ended { id, stable_id, result } = ended;
        self.transfers.detach(id.clone(), stable_id).await;
        if self.standby.get(&id).is_some_and(|&(standby_of, ..)| standby_of == stable_id) {
            self.standby.remove(&id);
        }
        if self.sessions.get(&id).is_none_or(|handle| handle.stable_id() != stable_id) {
            return;
        }
        let Some(handle) = self.sessions.remove(&id) else { return };
        self.relayed.remove(&id);
        self.controlled_by.remove(&id);
        self.status_to.remove(&id);
        self.mirroring.remove(&id);
        self.on_hotspot.remove(&id);
        self.slot_event(slot::Event::SessionEnded { peer: id.clone() }).await;
        let reason = match &result {
            Ok(()) => "done".to_owned(),
            Err(error) => error.to_string(),
        };
        if let Err(error) = &result
            && !matches!(error, Error::Timeout | Error::Closed(_))
        {
            handle.close(close_code_for(error));
        }
        log::info!("{id}: disconnected: {reason}");
        self.emit(ClientEvent::Disconnected { id: id.clone(), reason });
        let unpaired = result.as_ref().is_err_and(|error| self.closed_by_desktop(&id, error));
        if !unpaired && self.wants(&id) {
            self.retries.insert(id, Retry { at: Instant::now() + FIRST_RETRY, delay: FIRST_RETRY });
        }
    }

    pub(super) fn retry_due(&mut self) {
        let now = Instant::now();
        let due: Vec<DeviceId> =
            self.retries.iter().filter(|(_, retry)| retry.at <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            let Some(retry) = self.retries.remove(&id) else { continue };
            if self.sessions.get(&id).is_some_and(SessionHandle::is_live) {
                continue;
            }
            // A dial already running for a command takes the redial's place.
            if let Some(dial) = self.dial(&id, Start::Any) {
                dial.redial = Some(retry.delay);
            }
        }
    }

    /// The dial to `id` in flight, started now when none runs; none for a desktop the phone does not know.
    pub(super) fn dial(&mut self, id: &DeviceId, start: Start) -> Option<&mut Dial> {
        if !self.dialing.contains_key(id) {
            let peer = self.phone.desktops().iter().find(|peer| &peer.id == id).cloned()?;
            self.phone.set_present(self.present || self.is_busy(id));
            let dialing = self.phone.dialing();
            let over = match start {
                Start::Any => Over::Any,
                Start::Bluetooth => Over::Bluetooth,
                Start::Reached(_) => Over::Probe,
            };
            let task_id = id.clone();
            let task = self
                .dials
                .spawn(async move {
                    let opened = match start {
                        Start::Any => dialing.open(&peer).await,
                        Start::Bluetooth => dialing.connect_bluetooth(&peer).await,
                        Start::Reached(reached) => dialing.open_reached(&peer, reached).await,
                    };
                    (task_id, opened)
                })
                .id();
            self.dialing.insert(id.clone(), Dial { over, task, redial: None, waiters: Vec::new() });
        }
        self.dialing.get_mut(id)
    }

    /// A dial task that panicked: its desktop may dial again.
    pub(super) fn dial_failed(&mut self, task: tokio::task::Id) {
        self.dialing.retain(|_, dial| dial.task != task);
    }

    /// Finishes a dial on the actor: a session, or its error for whoever waits and the next redial.
    pub(super) async fn dialed(&mut self, id: DeviceId, opened: Result<Opened, Error>) {
        let Some(Dial { over, redial, waiters, .. }) = self.dialing.remove(&id) else {
            if let Ok(opened) = opened {
                opened.close(CloseCode::Done);
            }
            return;
        };
        let known = self.phone.desktops().iter().any(|peer| peer.id == id);
        let live = self.sessions.get(&id).filter(|handle| handle.is_live()).cloned();
        let result = match (opened, live) {
            (Ok(opened), _) if !known => {
                opened.close(CloseCode::Done);
                Err(Error::UnknownDevice)
            }
            (Ok(opened), Some(handle)) if !self.replaces(over, &id, &handle) => {
                opened.close(CloseCode::Done);
                Ok(handle)
            }
            (Ok(opened), _) => match self.phone.opened(opened) {
                Ok(session) => {
                    if over != Over::Any {
                        log::info!("{id}: moving the session to {:?} ({:?})", session.via, session.addr);
                    }
                    Ok(self.adopt(session).await)
                }
                Err(error) => Err(error),
            },
            (Err(error), _) => Err(error),
        };
        if let Err(error) = &result {
            let unpaired = known && self.closed_by_desktop(&id, error);
            if over != Over::Any && !unpaired && (redial.is_some() || !waiters.is_empty()) {
                log::info!("{id}: {over:?} dial failed: {error}; trying every path for who joined it");
                if let Some(dial) = self.dial(&id, Start::Any) {
                    dial.redial = redial;
                    dial.waiters = waiters;
                }
                return;
            }
            match redial {
                Some(delay) if !unpaired && self.wants(&id) => {
                    let delay = (delay * 2).min(MAX_RETRY);
                    log::info!("{id}: redial failed: {error}; next in {} s", delay.as_secs());
                    let retry = self.retries.entry(id.clone()).or_insert(Retry { at: Instant::now() + delay, delay });
                    retry.delay = retry.delay.max(delay);
                }
                _ => log::info!("{id}: {over:?} dial failed: {error}"),
            }
        }
        answer(waiters, result);
    }

    /// Whether a session the dial `over` opened replaces the live one.
    fn replaces(&self, over: Over, id: &DeviceId, live: &SessionHandle) -> bool {
        match over {
            Over::Any => false,
            Over::Bluetooth => !live.on_bluetooth(),
            Over::Probe => live.on_bluetooth() || self.on_relay(id, live),
        }
    }

    /// Acts on a desktop's close: `unpaired` forgets the desktop, `paused` tells the app. True when forgotten.
    fn closed_by_desktop(&mut self, id: &DeviceId, error: &Error) -> bool {
        match error {
            Error::Closed(CloseCode::Unpaired) => {
                if let Err(error) = self.phone.forget(id) {
                    log::error!("forgetting {id}: {error}");
                }
                self.outbox.forget(id.clone());
                self.retries.remove(id);
                self.emit(ClientEvent::Unpaired { id: id.clone() });
                true
            }
            Error::Closed(CloseCode::Paused) => {
                self.emit(ClientEvent::Paused { id: id.clone() });
                false
            }
            _ => false,
        }
    }

    /// Wi-Fi going away moves every session over it to Bluetooth at once, instead of after the idle timeout and a
    /// round of redials; Wi-Fi coming back looks for an IP path at once, instead of at the next probe.
    pub(super) async fn network_changed(&mut self, before: Option<NetworkKind>, now: NetworkKind) {
        if now != NetworkKind::None {
            // A new network may reach a desktop the last one could not; the backoff learned on the old one only delays.
            let at = Instant::now();
            for retry in self.retries.values_mut() {
                *retry = Retry { at, delay: FIRST_RETRY };
            }
        }
        if before == Some(NetworkKind::Wifi) && now != NetworkKind::Wifi {
            let standby: Vec<(DeviceId, (LinkId, Via))> =
                self.standby.keys().filter_map(|id| Some((id.clone(), self.standby_for(id)?))).collect();
            for (id, link) in standby {
                self.move_to_standby(id, link, "Wi-Fi is gone").await;
            }
            let over_wifi: Vec<Peer> = self
                .phone
                .desktops()
                .iter()
                .filter(|peer| peer.bluetooth.is_some() && !self.on_bluetooth(&peer.id) && self.wants(&peer.id))
                .cloned()
                .collect();
            for peer in over_wifi {
                if self.dialing.contains_key(&peer.id) {
                    continue;
                }
                log::info!("{}: Wi-Fi is gone; moving to Bluetooth", peer.id);
                self.dial(&peer.id, Start::Bluetooth);
            }
        } else if now == NetworkKind::Wifi && self.sessions.values().any(SessionHandle::on_bluetooth) {
            self.next_probe = Some(Instant::now());
        }
    }

    /// Looks for an IP path to every desktop on Bluetooth, off the actor, since reaching one can take seconds.
    pub(super) fn probe(&mut self) {
        let waiting: Vec<(DeviceId, bool)> = self
            .sessions
            .iter()
            .filter_map(|(id, handle)| {
                let relayed = self.on_relay(id, handle);
                (relayed || handle.on_bluetooth()).then(|| (id.clone(), relayed))
            })
            .collect();
        self.next_probe = (!waiting.is_empty()).then(|| Instant::now() + PROBE_EVERY);
        // On mobile data the only IP path to a Bluetooth peer is an overlay such as Tailscale, which would move the
        // session, and its transfers, onto the data plan; a relayed session is on that plan already.
        let cellular = self.status.is_some_and(|status| status.network == NetworkKind::Cellular);
        let ttl = OPEN_TTLS[self.probe_round % OPEN_TTLS.len()];
        self.probe_round = self.probe_round.wrapping_add(1);
        for (id, relayed) in waiting {
            if cellular && !relayed {
                continue;
            }
            let Some(mut peer) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() else { continue };
            let public = peer.public.filter(|_| relayed);
            peer.addresses.splice(0..0, public);
            let dialer = self.phone.dialer().clone();
            let session = self.sessions.get(&id).cloned();
            let observed = self.relayed.get(&id).filter(|_| relayed).and_then(Relay::observed);
            let opener = relayed.then(|| self.phone.socket().clone());
            if let Some(older) = self.probing.remove(&id) {
                older.abort();
            }
            let task_id = id.clone();
            let probe = self.probes.spawn(async move {
                if let Some(socket) = opener {
                    socket.open_toward(public.as_slice(), ttl);
                }
                if let (Some(session), Some(punch)) = (session, punch(&dialer, observed)) {
                    match session.send(punch).await {
                        Err(error) => log::info!("{task_id}: asking for a punch: {error}"),
                        // The desktop's punch must pass its own NAT before this phone's first dial reaches it.
                        Ok(()) if relayed => tokio::time::sleep(session.connection().quic().rtt()).await,
                        Ok(()) => {}
                    }
                }
                let reached = reach::reach(&dialer, &peer).await;
                (task_id, reached)
            });
            self.probing.insert(id, probe);
        }
    }

    /// Moves a Bluetooth session to the IP path a probe found: a session with a route moves it and keeps its streams;
    /// one without gives way to a new session, and the desktop closes the older one.
    pub(super) async fn probed(&mut self, id: DeviceId, reached: Result<Reached, Error>) {
        self.probing.remove(&id);
        let Ok(reached) = reached else { return };
        let Some(handle) =
            self.sessions.get(&id).filter(|handle| handle.on_bluetooth() || self.on_relay(&id, handle)).cloned()
        else {
            return;
        };
        let from = if handle.on_bluetooth() { "Bluetooth" } else { "the relay" };
        let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() else { return };
        if let Some(path) = handle.path() {
            // First: while the probe's connection lives, packets from the address it reached are its own.
            crate::wire::Connection::from(reached.dialed.connection).close(CloseCode::Done);
            if let Some(Hop::Link(link)) = path.via() {
                let via = if handle.on_bluetooth() { Via::Bluetooth } else { Via::Relay };
                self.standby.insert(id.clone(), (handle.stable_id(), link, via));
            }
            log::info!("{id}: moving the session from {from} to {}", reached.addr);
            path.set(Hop::Udp(reached.addr));
            handle.wake();
            self.path_changed(&id, reached.via, Some(reached.addr)).await;
            self.emit(ClientEvent::Connected {
                desktop: peer,
                addr: Some(reached.addr),
                via: reached.via,
                resumed: false,
            });
            return;
        }
        if self.dialing.contains_key(&id) {
            crate::wire::Connection::from(reached.dialed.connection).close(CloseCode::Done);
            return;
        }
        log::info!("{id}: a new session from {from} to {}", reached.addr);
        self.dial(&id, Start::Reached(reached));
    }
}

/// Answers every command that waited on one dial; only the first gets an error that does not copy.
fn answer(waiters: Vec<oneshot::Sender<Result<SessionHandle, Error>>>, result: Result<SessionHandle, Error>) {
    let mut waiters = waiters.into_iter();
    let Some(first) = waiters.next() else { return };
    for waiter in waiters {
        drop(waiter.send(match &result {
            Ok(handle) => Ok(handle.clone()),
            Err(Error::Closed(code)) => Err(Error::Closed(*code)),
            Err(Error::UnknownDevice) => Err(Error::UnknownDevice),
            Err(Error::Stopped) => Err(Error::Stopped),
            Err(Error::Timeout) => Err(Error::Timeout),
            Err(_) => Err(Error::Unreachable),
        }));
    }
    drop(first.send(result));
}
