use super::{
    ClientActor, CloseCode, DeviceId, Effect, Error, GROUP_CHANNEL, Grants, HotspotEnd, Instant, Kind, Message,
    Reached, ServerPin, Via, WIFI_DIRECT_JOIN, punch, reach, slot, text,
};

impl ClientActor {
    /// Starts the group or hotspot for `id`'s Bluetooth session, off the actor; its credentials go out once it is up.
    pub(super) async fn start_hotspot(&mut self, id: DeviceId) {
        if !self.on_bluetooth(&id) {
            self.finish_upgrades(&id, &Ok(()));
            return;
        }
        let needs = |grants: Grants| grants.files || grants.browse || grants.screen;
        if !self.phone.desktops().iter().any(|peer| peer.id == id && needs(peer.grants)) {
            self.tell_hotspot_end(&id, Some("its files, browse, and screen switches are off")).await;
            self.finish_upgrades(&id, &Err(Error::TooLargeForBluetooth));
            return;
        }
        let joins = self.phone.desktops().iter().any(|peer| peer.id == id && peer.has(WIFI_DIRECT_JOIN));
        let group = joins && self.phone.wifi_direct().is_some() && !self.wifi_direct_failed.contains(&id);
        let hotspot = self.phone.hotspot().is_some();
        self.slot_event(slot::Event::Request { peer: id, group, hotspot }).await;
    }

    /// Applies `event` to the slot and carries out its effects for the desktop it concerns.
    pub(super) async fn slot_event(&mut self, event: slot::Event) {
        let Some(peer) = event.peer().or(self.slot.peer()).cloned() else { return };
        let Ok(effects) = self.slot.apply(event, Instant::now(), self.idle()) else { return };
        for effect in effects {
            match effect {
                Effect::Start(kind) => self.start_provider(&peer, kind),
                Effect::Stop(kind) => {
                    if kind == Kind::Group && self.slot.kind() == Some(Kind::Hotspot) {
                        log::info!("{peer}: the Wi-Fi Direct group failed, trying the hotspot");
                        self.wifi_direct_failed.insert(peer.clone());
                    }
                    self.stop_provider(kind);
                    if self.on_hotspot.contains(&peer)
                        && let Some(handle) = self.sessions.get(&peer)
                    {
                        handle.close(CloseCode::Done);
                    }
                }
                Effect::Tell(reason) => self.tell_hotspot_end(&peer, reason.as_deref()).await,
                Effect::Fail(reason) => self.finish_upgrades(&peer, &Err(Error::Hotspot(reason))),
            }
        }
    }

    /// Starts `kind` for `id` off the actor, then sends the desktop its credentials. A group is created by the phone as
    /// its owner, so neither side negotiates and Android asks the user nothing.
    pub(super) fn start_provider(&mut self, id: &DeviceId, kind: Kind) {
        let handle = self.sessions.get(id).cloned();
        let (group, hotspot) = (self.phone.wifi_direct(), self.phone.hotspot());
        let wanted = (kind == Kind::Group).then(|| self.phone.group_for(id).map_err(text));
        let channel = self.phone.desktops().iter().any(|peer| peer.id == *id && peer.has(GROUP_CHANNEL));
        let id = id.clone();
        self.starting.spawn(async move {
            let started = async {
                let handle = handle.ok_or_else(|| "the Bluetooth session ended".to_owned())?;
                let none = || "no provider".to_owned();
                let network = match kind {
                    Kind::Group => {
                        let provider = group.ok_or_else(none)?;
                        let wanted = wanted.ok_or_else(none)??;
                        tokio::task::spawn_blocking(move || provider.start(&wanted)).await
                    }
                    Kind::Hotspot => {
                        let provider = hotspot.ok_or_else(none)?;
                        tokio::task::spawn_blocking(move || provider.start()).await
                    }
                };
                let mut network = network.map_err(text)?.map_err(text)?;
                if !channel {
                    network.mhz = None;
                }
                let message = match kind {
                    Kind::Group => Message::WifiDirect(network),
                    Kind::Hotspot => Message::Hotspot(network),
                };
                handle.send(message).await.map_err(text)
            };
            (id, kind, started.await)
        });
    }

    /// The desktop joined the hotspot; dialling the address it reported moves the session there.
    pub(super) fn hotspot_joined(&mut self, id: DeviceId, joined: &link_proto::message::HotspotJoined) {
        let (Some(peer), Ok(addr)) =
            (self.phone.desktops().iter().find(|peer| peer.id == id).cloned(), joined.address.parse())
        else {
            return;
        };
        let via = match self.slot.kind() {
            Some(Kind::Group) if self.slot.serves(&id) => Via::WifiDirect,
            Some(Kind::Hotspot) if self.slot.serves(&id) => Via::Hotspot,
            _ => return,
        };
        let dialer = self.phone.dialer().clone();
        let session = self.sessions.get(&id).cloned();
        self.probes.spawn(async move {
            // The desktop's firewall takes the dial for a reply once it has sent toward the phone's new address.
            if let (Some(session), Some(punch)) = (session, punch(&dialer, None))
                && let Err(error) = session.send(punch).await
            {
                log::info!("{id}: asking for a punch: {error}");
            }
            let reached = async {
                let pin = ServerPin::Key(peer.spki()?.fingerprint());
                let (connection, addr) = reach::race(&dialer, &[addr], pin).await?;
                Ok(Reached { dialed: connection, addr, via })
            };
            (id, reached.await)
        });
    }

    pub(super) async fn tell_hotspot_end(&self, id: &DeviceId, reason: Option<&str>) {
        let Some(handle) = self.sessions.get(id) else { return };
        let reason = reason.map(|reason| reason.chars().take(link_proto::message::MAX_REASON_LEN).collect());
        if let Err(error) = handle.send(Message::HotspotEnd(HotspotEnd { reason })).await {
            log::info!("{id}: telling the hotspot ended: {error}");
        }
    }

    pub(super) fn stop_provider(&self, kind: Kind) {
        match kind {
            Kind::Group => {
                if let Some(provider) = self.phone.wifi_direct() {
                    drop(tokio::task::spawn_blocking(move || provider.stop()));
                }
            }
            Kind::Hotspot => {
                if let Some(provider) = self.phone.hotspot() {
                    drop(tokio::task::spawn_blocking(move || provider.stop()));
                }
            }
        }
    }

    pub(super) fn finish_upgrades(&mut self, id: &DeviceId, result: &Result<(), Error>) {
        for reply in self.upgrades.remove(id).unwrap_or_default() {
            let result = match result {
                Ok(()) => Ok(()),
                Err(Error::Hotspot(reason)) => Err(Error::Hotspot(reason.clone())),
                Err(_) => Err(Error::TooLargeForBluetooth),
            };
            drop(reply.send(result));
        }
    }
}
