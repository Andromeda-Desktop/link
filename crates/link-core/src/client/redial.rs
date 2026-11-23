use crate::path::{FdLink, LinkKind, StreamLink};

use super::{
    ClientActor, ClientEvent, CloseCode, DeviceId, Duration, Ended, Error, FIRST_RETRY, Hop, Instant, LinkId,
    MAX_RETRY, NetworkKind, OPEN_TTLS, OwnedFd, PROBE_EVERY, Peer, Reached, Relay, Retry, SessionHandle, Via,
    close_code_for, punch, reach, slot,
};

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
        if matches!(result, Err(Error::Closed(CloseCode::Unpaired))) {
            if let Err(error) = self.phone.forget(&id) {
                log::error!("forgetting {id}: {error}");
            }
            self.outbox.forget(id.clone());
            self.emit(ClientEvent::Unpaired { id });
        } else {
            if matches!(result, Err(Error::Closed(CloseCode::Paused))) {
                self.emit(ClientEvent::Paused { id: id.clone() });
            }
            if self.wants(&id) {
                self.retries.insert(id, Retry { at: Instant::now() + FIRST_RETRY, delay: FIRST_RETRY });
            }
        }
    }

    pub(super) fn retry_due(&mut self) {
        let now = Instant::now();
        let due: Vec<DeviceId> =
            self.retries.iter().filter(|(_, retry)| retry.at <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            let Some(retry) = self.retries.remove(&id) else { continue };
            if self.sessions.get(&id).is_some_and(SessionHandle::is_live) || self.redialing.contains(&id) {
                continue;
            }
            let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() else { continue };
            self.redialing.insert(id.clone());
            self.phone.set_present(self.present || self.is_busy(&id));
            let dialer = self.phone.dialer().clone();
            self.redials.spawn(async move { (id, retry.delay, reach::reach(&dialer, &peer).await) });
        }
    }

    /// A redial's IP path becomes a session on the actor; without one, Bluetooth and the relay are tried as
    /// [`Self::open`] does.
    pub(super) async fn redialed(&mut self, id: DeviceId, delay: Duration, reached: Result<Reached, Error>) {
        self.redialing.remove(&id);
        if self.sessions.get(&id).is_some_and(SessionHandle::is_live) {
            return;
        }
        let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == id).cloned() else { return };
        let result = match reached {
            Ok(reached) => self.phone.adopt_reached(&peer, reached).await,
            Err(error) => self.phone.connect_indirect(&peer, error).await,
        };
        let error = match result {
            Ok(session) => return drop(self.adopt(session).await),
            Err(Error::Closed(CloseCode::Unpaired)) => {
                if let Err(error) = self.phone.forget(&id) {
                    log::error!("forgetting {id}: {error}");
                }
                self.outbox.forget(id.clone());
                self.retries.remove(&id);
                return self.emit(ClientEvent::Unpaired { id });
            }
            Err(error) => error,
        };
        if matches!(error, Error::Closed(CloseCode::Paused)) {
            self.emit(ClientEvent::Paused { id: id.clone() });
        }
        if self.wants(&id) {
            let delay = (delay * 2).min(MAX_RETRY);
            log::info!("{id}: redial failed: {error}; next in {} s", delay.as_secs());
            let retry = self.retries.entry(id).or_insert(Retry { at: Instant::now() + delay, delay });
            retry.delay = retry.delay.max(delay);
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
                log::info!("{}: Wi-Fi is gone; moving to Bluetooth", peer.id);
                match self.phone.connect_bluetooth(&peer).await {
                    Ok(session) => drop(self.adopt(session).await),
                    Err(error) => log::info!("{}: Bluetooth right after Wi-Fi: {error}", peer.id),
                }
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
        match self.phone.adopt_reached(&peer, reached).await {
            Ok(session) => {
                log::info!("{id}: moving from {from} to {:?}", session.addr);
                self.adopt(session).await;
            }
            Err(error) => log::info!("{id}: the IP path a probe found failed: {error}"),
        }
    }
}
