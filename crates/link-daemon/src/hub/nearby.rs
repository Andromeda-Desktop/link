use super::{
    DeviceId, Event, HOTSPOT_ASK_TIMEOUT, HOTSPOT_DECLINED, Hotspot, HotspotEnd, HotspotJoined, Hub, Instant, JoinStep,
    Message, Nearby, NearbyKind, Parked, SPARE_TIMEOUT, SessionHandle, Source, Waiting, hotspot, oneshot, total_size,
};

impl Hub {
    /// The Link device a send is for, when its session runs over Bluetooth and the files are worth the hotspot.
    pub(super) fn needs_hotspot(&self, device: &str, sources: &[Source]) -> Option<DeviceId> {
        let peer = DeviceId::parse(device).ok()?;
        let on_bluetooth = self.sessions.get(&peer)?.on_bluetooth();
        (on_bluetooth && total_size(sources) > link_core::bluetooth::BLUETOOTH_UPGRADE_ABOVE).then_some(peer)
    }

    /// Asks the phone for its hotspot and parks the work until the session moves there.
    pub(super) async fn wait_for_hotspot(&mut self, peer: DeviceId, work: Parked) {
        let asked = match self.sessions.get(&peer) {
            Some(session) => session.send(Message::HotspotRequest).await,
            None => Err(link_core::Error::NotConnected),
        };
        match (asked, work) {
            (Ok(()), work) => {
                let deadline = Instant::now() + link_core::hotspot::UPGRADE_TIMEOUT;
                self.waiting.push(Waiting { peer, work, deadline });
            }
            (Err(error), Parked::Send { reply, .. }) => drop(reply.send(Err(error))),
            (Err(_), Parked::Mirror(ask)) => drop(ask.reply.send(Err(crate::mirror::Failure::NotConnected))),
        }
    }

    /// Browsing goes on over Bluetooth while the phone starts its hotspot, so it asks once per session.
    pub(super) async fn ask_hotspot_once(&mut self, id: &DeviceId, session: &SessionHandle) {
        if matches!(self.nearby.get(id), Some(Nearby::Joined(_))) || !self.hotspot_asked.insert(id.clone()) {
            return;
        }
        log::info!("{id}: asking for the phone's hotspot to browse");
        if let Err(error) = session.send(Message::HotspotRequest).await {
            log::info!("{id}: asking for the hotspot: {error}");
        }
    }

    pub(super) async fn send_waiting(&mut self, id: &DeviceId) {
        let (ready, rest) = std::mem::take(&mut self.waiting).into_iter().partition(|waiting| waiting.peer == *id);
        self.waiting = rest;
        for Waiting { peer, work, .. } in ready {
            match work {
                Parked::Send { sources, reply } => drop(reply.send(self.send_files(peer.as_str(), sources).await)),
                Parked::Mirror(ask) => self.open_mirror(peer, ask).await,
            }
        }
    }

    pub(super) async fn fail_waiting(&mut self, id: &DeviceId, reason: &str) {
        let (failed, rest) = std::mem::take(&mut self.waiting).into_iter().partition(|waiting| waiting.peer == *id);
        self.waiting = rest;
        for waiting in failed {
            self.without_hotspot(waiting, link_core::Error::Hotspot(reason.to_owned())).await;
        }
    }

    pub(super) async fn expire_waiting(&mut self) {
        let now = Instant::now();
        let (expired, rest) =
            std::mem::take(&mut self.waiting).into_iter().partition(|waiting| waiting.deadline <= now);
        self.waiting = rest;
        for waiting in expired {
            log::info!("{}: the session did not move to the phone's hotspot in time", waiting.peer);
            self.without_hotspot(waiting, link_core::Error::TooLargeForBluetooth).await;
        }
    }

    /// Sends over Bluetooth what fits there, and fails the rest with `error`; mirroring never fits.
    pub(super) async fn without_hotspot(&mut self, waiting: Waiting, error: link_core::Error) {
        let Waiting { peer, work, .. } = waiting;
        let (sources, reply) = match work {
            Parked::Mirror(ask) => return drop(ask.reply.send(Err(crate::mirror::Failure::NeedsWifi))),
            Parked::Send { sources, reply } => (sources, reply),
        };
        if total_size(&sources) > link_core::bluetooth::BLUETOOTH_FILE_LIMIT {
            return drop(reply.send(Err(error)));
        }
        log::info!("{peer}: sending over Bluetooth: {error}");
        let sent = self.send_files(peer.as_str(), sources).await;
        drop(reply.send(sent));
    }

    /// Joins the phone's Wi-Fi Direct group off the actor, like a hotspot. Without a spare Wi-Fi interface the desktop
    /// leaves its own network for it, so the shell asks first, as for the hotspot.
    pub(super) async fn join_wifi_direct(&mut self, id: DeviceId, group: Hotspot) {
        let on_bluetooth = self.sessions.get(&id).is_some_and(SessionHandle::on_bluetooth);
        if !on_bluetooth || self.nearby.contains_key(&id) {
            return log::info!("{id}: ignoring a Wi-Fi Direct group this session does not need");
        }
        // `spare` may remove a stale spare interface, so not while a group of this desktop may be on it.
        let spare_in_use = self.nearby.values().any(|nearby| match nearby {
            Nearby::Joining { kind, .. } => *kind == NearbyKind::Group,
            Nearby::Joined(active) => crate::wifi_direct::on_spare(active),
        });
        self.nearby.insert(id.clone(), Nearby::Joining { kind: NearbyKind::Group, asked: None });
        if spare_in_use {
            return self.join_group(id, group, None).await;
        }
        self.joining.spawn(async move {
            let spare = tokio::time::timeout(SPARE_TIMEOUT, crate::wifi_direct::spare()).await.ok().flatten();
            JoinStep::Spare { id, group, spare }
        });
    }

    /// Joins the group on a spare interface beside this desktop's network, or without one asks to leave that network.
    pub(super) async fn join_group(&mut self, id: DeviceId, group: Hotspot, spare: Option<crate::wifi_direct::Spare>) {
        if !matches!(self.nearby.get(&id), Some(Nearby::Joining { kind: NearbyKind::Group, .. })) {
            if let Some(spare) = spare {
                tokio::spawn(crate::wifi_direct::release(spare));
            }
            return log::info!("{id}: dropping a Wi-Fi Direct group nobody waits for");
        }
        if let Some(spare) = spare {
            log::info!("{id}: joining the phone's Wi-Fi Direct group {:?} beside this desktop's network", group.ssid);
            self.joining.spawn(async move {
                let joined = crate::wifi_direct::join(spare, &group.ssid, &group.passphrase, group.mhz).await;
                JoinStep::Joined { id, kind: NearbyKind::Group, joined }
            });
            return;
        }
        let what = format!("leave this desktop's Wi-Fi for the phone's group {:?}", group.ssid);
        let answered = self.ask_hotspot(&id, &group.ssid, &what).await;
        self.joining.spawn(async move {
            let joined = match tokio::time::timeout(HOTSPOT_ASK_TIMEOUT, answered).await {
                Ok(Ok(true)) => hotspot::join(&group.ssid, &group.passphrase).await,
                _ => Err(HOTSPOT_DECLINED.to_owned()),
            };
            JoinStep::Joined { id, kind: NearbyKind::Group, joined }
        });
    }

    pub(super) async fn join_hotspot(&mut self, id: DeviceId, offered: Hotspot) {
        let on_bluetooth = self.sessions.get(&id).is_some_and(SessionHandle::on_bluetooth);
        if !on_bluetooth {
            return log::info!("{id}: ignoring a hotspot this session does not need");
        }
        // A join in flight answers the phone once; a second ask would orphan the first one's answer.
        if matches!(self.nearby.get(&id), Some(Nearby::Joining { kind: NearbyKind::Hotspot, .. })) {
            return log::info!("{id}: ignoring a hotspot offer while one is being joined");
        }
        // The phone offers its hotspot only after giving up on a group, which this desktop may have joined anyway.
        let abandoned = match self.nearby.remove(&id) {
            Some(Nearby::Joined(active)) => Some(active),
            _ => None,
        };
        self.nearby.insert(id.clone(), Nearby::Joining { kind: NearbyKind::Hotspot, asked: None });
        let what = format!("join the phone's hotspot {:?}", offered.ssid);
        let answered = self.ask_hotspot(&id, &offered.ssid, &what).await;
        self.joining.spawn(async move {
            if let Some(group) = abandoned {
                hotspot::leave(&group).await;
            }
            let joined = match tokio::time::timeout(HOTSPOT_ASK_TIMEOUT, answered).await {
                Ok(Ok(true)) => hotspot::join(&offered.ssid, &offered.passphrase).await,
                _ => Err(HOTSPOT_DECLINED.to_owned()),
            };
            JoinStep::Joined { id, kind: NearbyKind::Hotspot, joined }
        });
    }

    /// Asks the shell whether the desktop may leave its Wi-Fi for `ssid`; the answer comes through `AnswerHotspot`. A
    /// phone with the `nearby` grant gets yes at once.
    pub(super) async fn ask_hotspot(&mut self, id: &DeviceId, ssid: &str, what: &str) -> oneshot::Receiver<bool> {
        let (answer, answered) = oneshot::channel();
        if self.store.peer(id).is_some_and(|peer| peer.grants.nearby) {
            log::info!("{id}: allowed to {what} without asking");
            self.emit(Event::Hotspot { id: id.clone(), ssid: Some(ssid.to_owned()) }).await;
            let _ = answer.send(true);
            return answered;
        }
        log::info!("{id}: asking whether to {what}");
        if let Some(Nearby::Joining { asked, .. }) = self.nearby.get_mut(id) {
            *asked = Some((answer, ssid.to_owned()));
        }
        self.emit(Event::HotspotAsk { id: id.clone(), ssid: ssid.to_owned() }).await;
        answered
    }

    /// Answers the phone with where the desktop listens on its hotspot or group. A failed hotspot fails the sends
    /// waiting for it; a failed group does not, since the phone falls back to its hotspot.
    pub(super) async fn hotspot_joined(
        &mut self,
        id: DeviceId,
        kind: NearbyKind,
        joined: Result<hotspot::Joined, String>,
    ) {
        let asked = match self.nearby.get_mut(&id) {
            Some(Nearby::Joining { kind: joining, asked }) if *joining == kind => Some(asked.take()),
            _ => None,
        };
        let Some(asked) = asked else {
            // The phone gave up on its group and runs a hotspot now, which a HotspotEnd would stop.
            let what = match kind {
                NearbyKind::Group => "Wi-Fi Direct group",
                NearbyKind::Hotspot => "hotspot",
            };
            log::info!("{id}: dropping the {what} the phone gave up on");
            if let Ok(joined) = joined {
                hotspot::leave(&joined.active).await;
            }
            return;
        };
        self.nearby.remove(&id);
        if asked.is_some() {
            self.emit(Event::HotspotAsk { id: id.clone(), ssid: String::new() }).await;
        }
        let Some(session) = self.sessions.get(&id).cloned() else {
            let declined = joined.as_ref().is_err_and(|reason| reason == HOTSPOT_DECLINED);
            if let Ok(joined) = joined {
                hotspot::leave(&joined.active).await;
            }
            if kind == NearbyKind::Hotspot && !declined {
                self.emit(Event::Hotspot { id, ssid: None }).await;
            }
            return;
        };
        let reply = match joined {
            Ok(joined) => {
                let address = std::net::SocketAddr::new(joined.address, self.store.port).to_string();
                log::info!("{id}: joined the hotspot at {address}");
                self.nearby.insert(id.clone(), Nearby::Joined(joined.active));
                Message::HotspotJoined(HotspotJoined { address })
            }
            Err(reason) => {
                log::info!("{id}: joining the hotspot failed: {reason}");
                if kind == NearbyKind::Hotspot {
                    if reason != HOTSPOT_DECLINED {
                        self.emit(Event::Hotspot { id: id.clone(), ssid: None }).await;
                    }
                    self.fail_waiting(&id, &reason).await;
                }
                let reason = reason.chars().take(link_core::proto::message::MAX_REASON_LEN).collect();
                Message::HotspotEnd(HotspotEnd { reason: Some(reason) })
            }
        };
        if let Err(error) = session.send(reply).await {
            log::info!("{id}: answering the hotspot: {error}");
        }
    }

    pub(super) async fn leave_hotspot(&mut self, id: &DeviceId) {
        if !matches!(self.nearby.get(id), Some(Nearby::Joined(_))) {
            return;
        }
        if let Some(Nearby::Joined(active)) = self.nearby.remove(id) {
            hotspot::leave(&active).await;
            self.emit(Event::Hotspot { id: id.clone(), ssid: None }).await;
        }
    }
}
