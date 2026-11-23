use super::{
    CLIP_CLOCK, CLIP_ITEMS, ClientActor, ClientEvent, ClipPeer, CloseCode, DeviceId, Ended, Error, HEARTBEAT, Hop,
    Instant, LATE_HASH, PairTarget, Peer, Role, Route, SHARE_OUTBOX, SessionHandle, SocketAddr, Via, phone, session,
    slot,
};

impl ClientActor {
    pub(super) async fn pair(&mut self, target: PairTarget) -> Result<Peer, Error> {
        let session = self.phone.pair(target).await?;
        let peer = session.desktop.clone();
        self.adopt(session).await;
        Ok(peer)
    }

    pub(super) async fn live_session(&mut self, id: &DeviceId) -> Result<SessionHandle, Error> {
        if let Some(handle) = self.sessions.get(id).filter(|handle| handle.is_live()) {
            return Ok(handle.clone());
        }
        self.open(id).await
    }

    pub(super) async fn open(&mut self, id: &DeviceId) -> Result<SessionHandle, Error> {
        self.phone.set_present(self.present || self.is_busy(id));
        match self.phone.connect(id).await {
            Ok(session) => Ok(self.adopt(session).await),
            Err(error) => {
                if matches!(error, Error::Closed(CloseCode::Unpaired)) {
                    self.retries.remove(id);
                    self.emit(ClientEvent::Unpaired { id: id.clone() });
                } else if matches!(error, Error::Closed(CloseCode::Paused)) {
                    self.emit(ClientEvent::Paused { id: id.clone() });
                }
                Err(error)
            }
        }
    }

    pub(super) async fn adopt(&mut self, session: phone::Session) -> SessionHandle {
        let phone::Session { connection, control, desktop, addr, via, resumed, route: path, relay } = session;
        let id = desktop.id.clone();
        match relay {
            Some(relay) => drop(self.relayed.insert(id.clone(), relay)),
            None => drop(self.relayed.remove(&id)),
        }
        let route = Route {
            peer: id.clone(),
            events: self.session_events_tx.clone(),
            transfers: self.transfers.clone(),
            heartbeat: desktop.has(HEARTBEAT),
            path,
        };
        let (handle, actor) = session::session(connection, control, Role::Phone, route);
        let (task_id, stable_id) = (id.clone(), handle.stable_id());
        self.running.spawn(async move { Ended { id: task_id, stable_id, result: actor.run().await } });
        if let Some(old) = self.sessions.insert(id.clone(), handle.clone()) {
            old.close(CloseCode::Done);
        }
        // Before finish_upgrades wakes the sends waiting on this move, so none goes out on the closing session.
        let clip = ClipPeer {
            granted: desktop.grants.clipboard,
            items: desktop.has(CLIP_ITEMS),
            clock: desktop.has(CLIP_CLOCK),
        };
        self.transfers.attach(id.clone(), handle.clone(), clip, desktop.has(LATE_HASH)).await;
        self.outbox.live(id.clone(), handle.clone(), desktop.has(SHARE_OUTBOX));
        self.retries.remove(&id);
        self.path_changed(&id, via, addr).await;
        if !matches!(via, Via::Bluetooth | Via::Relay)
            && handle.path().is_some()
            && let Some(open) = self.phone.l2cap_opener(&desktop)
        {
            let (id, stable_id) = (id.clone(), handle.stable_id());
            self.standbys.spawn(async move {
                let opened =
                    tokio::task::spawn_blocking(open).await.unwrap_or_else(|join| Err(std::io::Error::other(join)));
                (id, stable_id, opened)
            });
        }
        if let Some(status) = self.status {
            let (handle, status) = (handle.clone(), self.status_message(&id, status));
            self.status_to.insert(id.clone(), status.clone());
            if let Err(error) = handle.send(status).await {
                log::info!("{id}: sending the status: {error}");
            }
        }
        self.send_notifications(&id, &handle, &[]).await;
        self.emit(ClientEvent::Connected { desktop, addr, via, resumed });
        handle
    }

    /// What follows a session's move to `via`, whether a new session or the same one on another path.
    pub(super) async fn path_changed(&mut self, id: &DeviceId, via: Via, addr: Option<SocketAddr>) {
        if via == Via::Bluetooth {
            log::info!("{id}: connected over Bluetooth");
            self.on_hotspot.remove(id);
            self.wifi_direct_failed.remove(id);
            self.next_probe = Some(Instant::now());
        } else {
            log::info!("{id}: connected at {}", addr.map_or_else(|| format!("{via:?}"), |addr| addr.to_string()));
            if via == Via::Relay {
                self.next_probe = Some(Instant::now());
            }
            if matches!(via, Via::Hotspot | Via::WifiDirect) {
                self.on_hotspot.insert(id.clone());
            } else {
                self.on_hotspot.remove(id);
            }
            self.finish_upgrades(id, &Ok(()));
            let busy = self.is_busy(id);
            self.slot_event(slot::Event::Adopted { peer: id.clone(), busy }).await;
        }
    }

    /// Whether `id`'s live session still runs through the relay it started on.
    pub(super) fn on_relay(&self, id: &DeviceId, handle: &SessionHandle) -> bool {
        self.relayed.contains_key(id) && handle.path().is_some_and(|path| matches!(path.via(), Some(Hop::Link(_))))
    }

    /// Whether `id`'s live session runs over Bluetooth: QUIC routed over an L2CAP or RFCOMM link.
    pub(super) fn on_bluetooth(&self, id: &DeviceId) -> bool {
        self.sessions.get(id).is_some_and(SessionHandle::on_bluetooth)
    }
}
