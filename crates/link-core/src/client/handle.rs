use super::{
    Arc, BLUETOOTH_FILE_LIMIT, BLUETOOTH_UPGRADE_ABOVE, Camera, Client, Codec, Command, DesktopLink, DesktopState,
    DeviceId, Error, Feature, LocalClip, MIRROR_STREAMS, Message, Messages, MirrorSender, MirrorStarted, MirrorStop,
    NotificationPosted, PRESENTING, PairTarget, Peer, SCREEN_SHAPE, ScreenShape, Sent, SessionHandle, Share, Source,
    Status, TransferId, UNPAIR_TELL_TIMEOUT, UPGRADE_TIMEOUT, browse, feature_of, oneshot,
};

impl Client {
    /// Pairs and keeps the session open.
    pub async fn pair(&self, target: PairTarget) -> Result<Peer, Error> {
        self.request(|reply| Command::Pair { target, reply }).await?
    }

    /// Opens a session unless one is live.
    pub async fn connect(&self, id: DeviceId) -> Result<(), Error> {
        self.session(id).await.map(drop)
    }

    /// While present, every paired desktop is kept connected with keep-alive and redialled when it drops; otherwise
    /// sessions are opened on demand and idle out.
    pub async fn set_present(&self, present: bool) -> Result<(), Error> {
        self.commands.send(Command::SetPresent { present }).await.map_err(|_| Error::Stopped)
    }

    /// Whether the phone's screen is on: a nearby network nothing uses stays up longer while it is.
    pub async fn set_screen_on(&self, on: bool) -> Result<(), Error> {
        self.commands.send(Command::SetScreenOn { on }).await.map_err(|_| Error::Stopped)
    }

    /// Queues `share` for the desktop, connecting first if needed; returns once the desktop acknowledged it, or when
    /// it waits in the outbox for the desktop's next session.
    pub async fn share(&self, id: DeviceId, share: Share) -> Result<Sent, Error> {
        share.check()?;
        if !self.desktops().await?.iter().any(|desktop| desktop.peer.id == id) {
            return Err(Error::UnknownDevice);
        }
        if let Err(error) = self.session(id.clone()).await {
            log::info!("{id}: not connected for a share: {error}");
        }
        self.outbox.share(id, share).await
    }

    /// Offers files to the desktop, connecting with keep-alive first; the session stays until the transfer ends.
    pub async fn send_files(&self, id: DeviceId, sources: Vec<Source>) -> Result<TransferId, Error> {
        self.commands.send(Command::Keep { id: id.clone(), keep: true }).await.map_err(|_| Error::Stopped)?;
        let total: u64 = sources.iter().map(Source::size).sum();
        let sent = match self.session(id.clone()).await {
            Ok(handle) if handle.on_bluetooth() && total > BLUETOOTH_UPGRADE_ABOVE => {
                match self.upgrade(id.clone()).await {
                    Ok(()) => self.transfers.send(id.clone(), sources).await,
                    Err(error) if total <= BLUETOOTH_FILE_LIMIT => {
                        log::info!("{id}: sending over Bluetooth: {error}");
                        self.transfers.send(id.clone(), sources).await
                    }
                    Err(error) => {
                        log::info!("{id}: no faster path: {error}");
                        Err(Error::NoFasterPath(error.to_string()))
                    }
                }
            }
            Ok(_) => self.transfers.send(id.clone(), sources).await,
            Err(error) => Err(error),
        };
        self.commands.send(Command::Keep { id, keep: false }).await.map_err(|_| Error::Stopped)?;
        sent
    }

    /// Tells the desktop that asked for mirroring the video's size and codec, then opens the stream the access units
    /// go on. The codec is the desktop's best one in `encoders` (what the phone encodes besides H.264), else H.264
    /// (None); the caller encodes in it.
    pub async fn start_mirror(
        &self,
        id: DeviceId,
        width: u32,
        height: u32,
        encoders: &[Codec],
    ) -> Result<(MirrorSender, Option<Codec>), Error> {
        let session = self.session(id.clone()).await?;
        let desktops = self.desktops().await?;
        let desktop = desktops.iter().find(|desktop| desktop.peer.id == id);
        let codec =
            desktop.and_then(|desktop| desktop.mirror_codecs.iter().copied().find(|codec| encoders.contains(codec)));
        let streams = desktop.is_some_and(|desktop| desktop.peer.has(MIRROR_STREAMS));
        session.send(Message::MirrorStarted(MirrorStarted { width, height, codec })).await?;
        let sender = MirrorSender::open(&session, width, height, codec, streams).await?;
        self.commands.send(Command::Mirroring { id, on: true }).await.map_err(|_| Error::Stopped)?;
        Ok((sender, codec))
    }

    /// Opens the stream of this phone's microphone to the desktop that sent `mic-request`; the caller sends Opus units
    /// (48 kHz mono) and finishes it when the microphone stops.
    pub async fn start_mic(&self, id: DeviceId) -> Result<MirrorSender, Error> {
        let session = self.session(id).await?;
        MirrorSender::open_sound(&session, Message::MicData).await
    }

    /// Tells a mirroring desktop the screen's corners and cutouts; a desktop without [`SCREEN_SHAPE`] gets nothing.
    pub async fn send_screen_shape(&self, id: DeviceId, shape: ScreenShape) -> Result<(), Error> {
        let desktops = self.desktops().await?;
        if !desktops.iter().any(|desktop| desktop.peer.id == id && desktop.peer.has(SCREEN_SHAPE)) {
            return Ok(());
        }
        self.session(id).await?.send(Message::ScreenShape(shape)).await
    }

    /// Ends mirroring, or refuses a request, with a reason when it failed.
    pub async fn stop_mirror(&self, id: DeviceId, reason: Option<String>) -> Result<(), Error> {
        let reason = reason.map(|reason| reason.chars().take(link_proto::message::MAX_REASON_LEN).collect());
        self.commands.send(Command::Mirroring { id: id.clone(), on: false }).await.map_err(|_| Error::Stopped)?;
        self.send(id, Message::MirrorStop(MirrorStop { reason })).await
    }

    /// Asks the desktop for its screen (remote control), or to switch to `source`; the video arrives as a
    /// [`TransferEvent::Mirror`] from it.
    pub async fn start_remote(&self, id: DeviceId, source: Option<u32>) -> Result<(), Error> {
        let request = Message::RemoteRequest(link_proto::message::RemoteRequest { source });
        self.send(id.clone(), request).await?;
        self.commands.send(Command::Mirroring { id, on: true }).await.map_err(|_| Error::Stopped)
    }

    pub async fn stop_remote(&self, id: DeviceId) -> Result<(), Error> {
        self.commands.send(Command::Mirroring { id: id.clone(), on: false }).await.map_err(|_| Error::Stopped)?;
        self.send(id, Message::RemoteStop(MirrorStop { reason: None })).await
    }

    /// Tells the desktop that the phone's presenter for it opened or closed, so it stays awake meanwhile; a no-op
    /// for desktops without `presenting`.
    pub async fn presenting(&self, id: DeviceId, on: bool) -> Result<(), Error> {
        let desktops = self.desktops().await?;
        if !desktops.iter().any(|desktop| desktop.peer.id == id && desktop.peer.has(PRESENTING)) {
            return Ok(());
        }
        self.send(id, Message::Presenting(link_proto::message::Presenting { on })).await
    }

    /// Offers the phone's clipboard to every connected desktop whose clipboard switch is on, and keeps it for the
    /// next one that connects. Returns how many got it now.
    pub async fn offer_clip(&self, clip: LocalClip) -> Result<usize, Error> {
        let desktops = self.desktops().await?;
        let connected: Vec<DeviceId> = desktops
            .into_iter()
            .filter(|desktop| desktop.link == DesktopLink::Live && desktop.peer.grants.clipboard)
            .map(|desktop| desktop.peer.id)
            .collect();
        let count = connected.len();
        self.transfers.offer_clip(connected, clip).await.map(|()| count)
    }

    /// Writes `mime` of a desktop's clipboard offer into `sink`.
    pub async fn pull_clip(&self, id: DeviceId, clip: u64, mime: String, sink: std::fs::File) -> Result<u64, Error> {
        self.transfers.pull_clip(id, clip, mime, sink).await
    }

    /// The folders a desktop with the browse switch on sees; empty while the app may not read shared storage.
    pub async fn set_browse_roots(&self, roots: browse::Roots) -> Result<(), Error> {
        self.commands.send(Command::SetBrowseRoots { roots }).await.map_err(|_| Error::Stopped)
    }

    /// The phone's contacts for desktops with the messages switch on; None while the app has no access.
    pub async fn set_messages(&self, messages: Option<Arc<dyn Messages>>) -> Result<(), Error> {
        self.commands.send(Command::SetMessages { messages }).await.map_err(|_| Error::Stopped)
    }

    /// The camera desktops with the browse switch on may ask for a photo; None while the app may not use it.
    pub async fn set_camera(&self, camera: Option<Arc<dyn Camera>>) -> Result<(), Error> {
        self.commands.send(Command::SetCamera { camera }).await.map_err(|_| Error::Stopped)
    }

    /// Tells desktops with the messages switch on to pull the contacts again.
    pub async fn contacts_changed(&self) -> Result<(), Error> {
        self.commands.send(Command::MessagesChanged(Message::ContactsChanged)).await.map_err(|_| Error::Stopped)
    }

    /// Tells desktops with the messages switch on to pull the texts again.
    pub async fn texts_changed(&self) -> Result<(), Error> {
        self.commands.send(Command::MessagesChanged(Message::TextsChanged)).await.map_err(|_| Error::Stopped)
    }

    /// Tells desktops with the calls switch on to pull the call log again.
    pub async fn calls_changed(&self) -> Result<(), Error> {
        self.commands.send(Command::MessagesChanged(Message::CallsChanged)).await.map_err(|_| Error::Stopped)
    }

    /// The phone's battery and network: sent to every desktop on connect and, at most every 10 s, on change.
    pub async fn set_status(&self, status: Status) -> Result<(), Error> {
        self.commands.send(Command::SetStatus { status }).await.map_err(|_| Error::Stopped)
    }

    /// Answers a desktop's offer; false when it no longer waits.
    pub async fn decide(&self, transfer: TransferId, accept: bool) -> Result<bool, Error> {
        self.transfers.decide(transfer, accept).await
    }

    pub async fn cancel_transfer(&self, transfer: TransferId) -> Result<bool, Error> {
        self.transfers.cancel(transfer).await
    }

    /// Tells the desktop, then forgets it. Returns whether the desktop was told; it is forgotten either way.
    pub async fn unpair(&self, id: DeviceId) -> Result<bool, Error> {
        // Telling the desktop is a courtesy, over an open session only: a dial would hold the actor, and the forget
        // queued behind it, for up to a connect and a hello; one that went away without closing holds until QUIC
        // times out.
        let tell = async {
            match self.request(|reply| Command::Live { id: id.clone(), reply }).await? {
                Some(handle) => handle.unpair().await,
                None => Err(Error::NotConnected),
            }
        };
        let told = tokio::time::timeout(UNPAIR_TELL_TIMEOUT, tell).await.unwrap_or(Err(Error::NotConnected));
        if let Err(error) = &told {
            log::info!("unpairing {id} without telling it: {error}");
        }
        self.request(|reply| Command::Forget { id, reply }).await??;
        Ok(told.is_ok())
    }

    pub async fn desktops(&self) -> Result<Vec<DesktopState>, Error> {
        self.request(|reply| Command::Desktops { reply }).await
    }

    pub async fn set_sharing(&self, id: DeviceId, feature: Feature, on: bool) -> Result<(), Error> {
        self.request(|reply| Command::SetSharing { id, feature, on, reply }).await?
    }

    /// Sends an unacknowledged message to every connected desktop whose switch for its feature is on, without
    /// dialling; returns how many it was written to.
    pub async fn broadcast(&self, message: Message) -> Result<usize, Error> {
        message.validate()?;
        if matches!(message, Message::NotificationPosted(_) | Message::NotificationRemoved(_)) {
            return self.request(|reply| Command::Notify { message, reply }).await;
        }
        let feature = feature_of(&message).ok_or(Error::Unexpected(message.kind()))?;
        let kind = message.kind();
        let sessions = self.request(|reply| Command::Sessions { feature, message, reply }).await?;
        let mut sent = 0;
        for (session, message) in sessions {
            match session.send(message).await {
                Ok(()) => sent += 1,
                Err(error) => log::info!("broadcasting a {kind}: {error}"),
            }
        }
        Ok(sent)
    }

    /// Replaces every notification the phone shows: connected desktops pull what changed and drop what is gone.
    pub async fn sync_notifications(&self, notifications: Vec<NotificationPosted>) -> Result<(), Error> {
        for posted in &notifications {
            Message::NotificationPosted(posted.clone()).validate()?;
        }
        self.commands.send(Command::SyncNotifications { notifications }).await.map_err(|_| Error::Stopped)
    }

    /// Sends an unacknowledged message to one desktop, connecting first if needed.
    pub async fn send(&self, id: DeviceId, message: Message) -> Result<(), Error> {
        message.validate()?;
        self.session(id).await?.send(message).await
    }

    /// Waits for `id`'s session to move from Bluetooth to the phone's hotspot.
    async fn upgrade(&self, id: DeviceId) -> Result<(), Error> {
        let upgraded = self.request(|reply| Command::Upgrade { id, reply });
        tokio::time::timeout(UPGRADE_TIMEOUT, upgraded).await.map_err(|_| Error::TooLargeForBluetooth)??
    }

    async fn session(&self, id: DeviceId) -> Result<SessionHandle, Error> {
        self.request(|reply| Command::Session { id, reply }).await?
    }

    async fn request<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Result<T, Error> {
        let (reply, response) = oneshot::channel();
        self.commands.send(make(reply)).await.map_err(|_| Error::Stopped)?;
        response.await.map_err(|_| Error::Stopped)
    }
}
