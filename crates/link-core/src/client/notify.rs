use super::{
    CALLS, ClientActor, DeviceId, Error, Feature, FsRefusal, Instant, MAX_SNAPSHOT, MESSAGES, Message, MessagesRefusal,
    NotificationDigest, NotificationPosted, NotificationRemoved, NotificationSnapshot, Peer, PhotoRefusal, SHARING,
    SessionHandle, Sharing, TEXTS_MMS, Takes, browse, camera, for_desktop, messages, shown_digest, slot,
};

impl ClientActor {
    /// Keeps a post or removal, then sends it to each desktop that takes notifications; returns how many.
    pub(super) async fn notify(&mut self, message: Message) -> usize {
        let held = self.notifications.get_or_insert_default();
        match &message {
            Message::NotificationPosted(posted) => {
                held.retain(|shown| shown.id != posted.id);
                held.push(posted.clone());
            }
            Message::NotificationRemoved(removed) => held.retain(|shown| shown.id != removed.id),
            _ => {}
        }
        let desktops: Vec<(SessionHandle, Takes)> = self
            .phone
            .desktops()
            .iter()
            .filter(|peer| peer.grants.allows(Feature::Notifications))
            .filter_map(|peer| {
                let handle = self.sessions.get(&peer.id).filter(|handle| handle.is_live())?;
                Some((handle.clone(), Takes::of(peer)))
            })
            .collect();
        let mut sent = 0;
        for (handle, takes) in desktops {
            match handle.send(for_desktop(message.clone(), takes)).await {
                Ok(()) => sent += 1,
                Err(error) => log::info!("broadcasting a {}: {error}", message.kind()),
            }
        }
        sent
    }

    pub(super) async fn sync_notifications(&mut self, notifications: Vec<NotificationPosted>) {
        let old = self.notifications.replace(notifications.clone()).unwrap_or_default();
        let gone: Vec<String> = old
            .into_iter()
            .map(|posted| posted.id)
            .filter(|id| !notifications.iter().any(|posted| posted.id == *id))
            .collect();
        let live: Vec<(DeviceId, SessionHandle)> = self
            .sessions
            .iter()
            .filter(|(_, handle)| handle.is_live())
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect();
        for (id, handle) in live {
            self.send_notifications(&id, &handle, &gone).await;
        }
    }

    /// A snapshot to a desktop that syncs, so that it pulls only what it lacks; every notification to an older one.
    /// Nothing until the app first reports what it shows; an older desktop also hears the removal of `gone`.
    pub(super) async fn send_notifications(&self, id: &DeviceId, handle: &SessionHandle, gone: &[String]) {
        let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == *id).cloned() else { return };
        let Some(held) = self.notifications.as_ref().filter(|_| peer.grants.allows(Feature::Notifications)) else {
            return;
        };
        let takes = Takes::of(&peer);
        let messages = if takes.sync {
            let notifications = held
                .iter()
                .rev()
                .take(MAX_SNAPSHOT)
                .map(|posted| NotificationDigest { id: posted.id.clone(), hash: shown_digest(posted, takes) })
                .collect();
            vec![Message::NotificationSnapshot(NotificationSnapshot { notifications })]
        } else {
            let removed = gone.iter().map(|id| Message::NotificationRemoved(NotificationRemoved { id: id.clone() }));
            removed
                .chain(held.iter().map(|posted| for_desktop(Message::NotificationPosted(posted.clone()), takes)))
                .collect()
        };
        for message in messages {
            if let Err(error) = handle.send(message).await {
                return log::info!("{id}: sending the notifications: {error}");
            }
        }
    }

    /// Answers a browse request off the actor, when this desktop's browse switch is on.
    pub(super) async fn browse(&mut self, from: &DeviceId, request: Message) {
        let Some(handle) = self.sessions.get(from).cloned() else { return };
        let Some(req) = browse::request_id(&request) else { return };
        let allowed = self.phone.desktops().iter().any(|peer| peer.id == *from && peer.grants.browse);
        let refusal = if !allowed {
            Some(FsRefusal::NotAllowed)
        } else if self.phone.browse_roots().is_empty() {
            Some(FsRefusal::Denied)
        } else if self.browsing.len() >= browse::MAX_IN_FLIGHT {
            Some(FsRefusal::Busy)
        } else {
            None
        };
        if let Some(reason) = refusal {
            if let Err(error) = handle.send(browse::error(req, reason)).await {
                log::info!("{from}: refusing a browse request: {error}");
            }
            return;
        }
        drop(self.slot.apply(slot::Event::Used { peer: from.clone() }, Instant::now(), self.idle()));
        drop(self.browsing.spawn(browse::answer(self.phone.browse_roots().clone(), handle, request)));
    }

    /// Answers a contacts or texts request off the actor, when this desktop's messages switch is on.
    pub(super) async fn messages_request(&mut self, from: &DeviceId, mut request: messages::Request) {
        let Some(handle) = self.sessions.get(from).cloned() else { return };
        let peer = self.phone.desktops().iter().find(|peer| peer.id == *from);
        let allowed = peer.is_some_and(|peer| if request.is_call() { peer.grants.calls } else { peer.grants.messages });
        if let messages::Request::Texts(_, rich) = &mut request {
            *rich = peer.is_some_and(|peer| peer.has(TEXTS_MMS));
        }
        let refusal = match &self.messages {
            _ if !allowed => MessagesRefusal::NotAllowed,
            None => MessagesRefusal::Denied,
            Some(_) if self.browsing.len() >= browse::MAX_IN_FLIGHT => MessagesRefusal::Failed,
            Some(messages) => {
                drop(self.browsing.spawn(messages::answer(messages.clone(), handle, request)));
                return;
            }
        };
        if let Err(error) = handle.send(request.refused(refusal)).await {
            log::info!("{from}: refusing a contacts or texts request: {error}");
        }
    }

    /// Takes a photo for `from` off the actor, when its browse switch is on.
    pub(super) async fn photo(&mut self, from: &DeviceId, req: u64) {
        let Some(handle) = self.sessions.get(from).cloned() else { return };
        let peer = self.phone.desktops().iter().find(|peer| peer.id == *from).cloned();
        let refusal = match (&self.camera, peer) {
            (_, Some(peer)) if !peer.grants.browse => PhotoRefusal::NotAllowed,
            (None, _) | (_, None) => PhotoRefusal::Denied,
            (Some(_), _) if self.browsing.len() >= browse::MAX_IN_FLIGHT => PhotoRefusal::Failed,
            (Some(camera), Some(peer)) => {
                drop(self.browsing.spawn(camera::answer(camera.clone(), handle, peer.name, req)));
                return;
            }
        };
        if let Err(error) = handle.send(camera::refused(req, refusal)).await {
            log::info!("{from}: refusing a photo request: {error}");
        }
    }

    /// Turns one feature on or off for desktop `id` and tells that desktop what changed.
    pub(super) async fn set_sharing(&mut self, id: &DeviceId, feature: Feature, on: bool) -> Result<(), Error> {
        self.phone.set_sharing(id, feature, on)?;
        if let Some(peer) = self.phone.desktops().iter().find(|peer| peer.id == *id)
            && let Some(handle) = self.sessions.get(id).filter(|handle| handle.is_live())
        {
            Self::send_sharing(peer, handle).await;
        }
        if feature == Feature::Messages {
            self.messages_changed(Some(id), &[Message::ContactsChanged, Message::TextsChanged]).await;
        }
        if feature == Feature::Calls {
            self.messages_changed(Some(id), &[Message::CallsChanged]).await;
        }
        Ok(())
    }

    /// Tells `peer` what this phone lets it use, when its hello takes `sharing`.
    pub(super) async fn send_sharing(peer: &Peer, handle: &SessionHandle) {
        if !peer.has(SHARING) {
            return;
        }
        if let Err(error) = handle.send(Message::Sharing(Sharing { allows: peer.grants.sharing() })).await {
            log::info!("{}: sending sharing: {error}", peer.id);
        }
    }

    /// Sends each of `changed` to `only`, else to every live desktop that takes them.
    pub(super) async fn messages_changed(&self, only: Option<&DeviceId>, changed: &[Message]) {
        for peer in self.phone.desktops().iter().filter(|peer| only.is_none_or(|id| *id == peer.id)) {
            let Some(handle) = self.sessions.get(&peer.id).filter(|handle| handle.is_live()) else { continue };
            for message in changed {
                let call = matches!(message, Message::CallsChanged);
                if !peer.has(if call { CALLS } else { MESSAGES }) {
                    continue;
                }
                if let Err(error) = handle.send(message.clone()).await {
                    log::info!("{}: sending {}: {error}", peer.id, message.kind());
                }
            }
        }
    }
}
