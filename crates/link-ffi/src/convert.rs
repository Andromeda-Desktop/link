use super::*;

pub(super) fn transfer_event(event: TransferEvent) -> Option<LinkEvent> {
    Some(match event {
        TransferEvent::Offered { id, from, files } => LinkEvent::TransferOffered {
            transfer_id: id.to_hex(),
            desktop_id: from.to_string(),
            files: files
                .into_iter()
                .map(|file| OfferedFile { name: file.name, size: file.size, mime: file.mime })
                .collect(),
        },
        TransferEvent::Progress { id, bytes, total } => {
            LinkEvent::TransferProgress { transfer_id: id.to_hex(), bytes, total }
        }
        TransferEvent::Finished { id, peer, incoming, status, files } => LinkEvent::TransferFinished {
            transfer_id: id.to_hex(),
            desktop_id: peer.to_string(),
            incoming,
            status: status.as_str().to_owned(),
            files: files
                .into_iter()
                .map(|file| ReceivedFile { path: file.path.to_string_lossy().into_owned(), sha256: hex(&file.sha256) })
                .collect(),
        },
        TransferEvent::ClipOffered { from, id, mimes, size, text } => {
            LinkEvent::ClipOffered { desktop_id: from.to_string(), clip_id: id, mimes, size, text }
        }
        TransferEvent::Busy { .. } | TransferEvent::Mirror { .. } => return None,
    })
}

/// Takes ownership of the app's descriptor, which the app detached from its `ParcelFileDescriptor`.
pub(super) fn source(file: OutgoingFile) -> Result<Source, LinkError> {
    if file.fd < 0 {
        return Err(LinkError::Rejected { reason: "not a file descriptor".to_owned() });
    }
    // SAFETY: the app hands over a descriptor it detached and no longer uses, so nothing else owns or closes it.
    let owned = unsafe { OwnedFd::from_raw_fd(file.fd) };
    let source = Source::new(File::from(owned), file.name, file.mime)?;
    if source.size() != file.size {
        return Err(LinkError::Rejected { reason: "the file's size is not what the app reported".to_owned() });
    }
    Ok(source)
}

pub(super) fn parse_transfer(id: &str) -> Result<TransferId, LinkError> {
    TransferId::parse_hex(id).ok_or_else(|| LinkError::Rejected { reason: "not a transfer id".to_owned() })
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(2 * bytes.len()), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

impl LinkClient {
    pub(super) async fn broadcast(&self, message: Message) -> Result<u32, LinkError> {
        let client = self.client.clone();
        let sent = self.run(async move { client.broadcast(message).await }).await?;
        Ok(u32::try_from(sent).unwrap_or(u32::MAX))
    }
}

#[expect(clippy::too_many_lines, reason = "one arm per event")]
pub(super) fn translate(event: ClientEvent, runtime: &tokio::runtime::Handle) -> Option<LinkEvent> {
    Some(match event {
        ClientEvent::Connected { desktop, .. } => LinkEvent::Connected { desktop_id: desktop.id.to_string() },
        ClientEvent::Disconnected { id, .. } => LinkEvent::Disconnected { desktop_id: id.to_string() },
        ClientEvent::Link { id, live } => LinkEvent::Link { desktop_id: id.to_string(), live },
        ClientEvent::Received { from, share } => LinkEvent::Received {
            desktop_id: from.to_string(),
            kind: match share.kind {
                message::ShareKind::Text => ShareKind::Text,
                message::ShareKind::Link => ShareKind::Link,
            },
            text: share.text,
        },
        ClientEvent::Unpaired { id } => LinkEvent::Unpaired { desktop_id: id.to_string() },
        ClientEvent::Paused { id } => LinkEvent::Paused { desktop_id: id.to_string() },
        ClientEvent::MirrorBlocked { from } => LinkEvent::MirrorBlocked { desktop_id: from.to_string() },
        ClientEvent::Transfer(TransferEvent::Mirror { from, stream }) => {
            let receiver = stream.take()?;
            LinkEvent::RemoteStream {
                desktop_id: from.to_string(),
                width: receiver.width,
                height: receiver.height,
                stream: Arc::new(RemoteView {
                    runtime: runtime.clone(),
                    receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
                }),
            }
        }
        ClientEvent::Transfer(event) => return transfer_event(event),
        ClientEvent::SystemCommand { from, id, command } => LinkEvent::SystemCommand {
            desktop_id: from.to_string(),
            id,
            command: command.command.as_str().to_owned(),
            value: command.value,
        },
        ClientEvent::Message { from, message } => {
            let desktop_id = from.to_string();
            match message {
                Message::NotificationAction(action) => LinkEvent::NotificationAction {
                    desktop_id,
                    id: action.id,
                    action: action.action,
                    reply_text: action.reply_text,
                },
                Message::NotificationDismiss(dismiss) => {
                    LinkEvent::NotificationDismissed { desktop_id, id: dismiss.id }
                }
                Message::MediaPlayer(player) => LinkEvent::PlayerChanged { desktop_id, player: player.into() },
                Message::MediaGone(gone) => LinkEvent::PlayerGone { desktop_id, player: gone.player },
                Message::Ring(ring) => LinkEvent::RingRequested { desktop_id, on: ring.on },
                Message::CallAction(action) => LinkEvent::CallActionRequested {
                    desktop_id,
                    action: match action.action {
                        message::CallActionKind::Mute => CallAction::Mute,
                        message::CallActionKind::Decline => CallAction::Decline,
                    },
                },
                Message::Ringing(ringing) => LinkEvent::DesktopRinging { desktop_id, on: ringing.on },
                Message::MirrorRequest(_) => LinkEvent::MirrorRequested { desktop_id },
                Message::MirrorInput(input) => LinkEvent::MirrorInput {
                    desktop_id,
                    action: input.action.into(),
                    x: input.x,
                    y: input.y,
                    x2: input.x2,
                    y2: input.y2,
                    ms: input.ms,
                    text: input.text,
                    code: input.code,
                    meta: input.meta,
                },
                Message::MirrorKeyframe => LinkEvent::MirrorKeyframeRequested { desktop_id },
                Message::Commands(list) => LinkEvent::CommandsChanged {
                    desktop_id,
                    commands: list.commands.into_iter().map(|c| DesktopCommand { id: c.id, name: c.name }).collect(),
                },
                Message::Ping(ping) => LinkEvent::Pinged { desktop_id, message: ping.message },
                Message::Capabilities(capabilities) => {
                    LinkEvent::CapabilitiesChanged { desktop_id, can: capabilities.can }
                }
                Message::DesktopStatus(status) => LinkEvent::DesktopStatusChanged {
                    desktop_id,
                    volume: status.volume,
                    muted: status.muted,
                    battery: status.battery,
                    charging: status.charging,
                    brightness: status.brightness,
                    locked: status.locked,
                    idle: status.idle,
                    dnd: status.dnd,
                    profile: status.profile.map(|profile| profile.as_str().to_owned()),
                },
                Message::MirrorStop(stop) => LinkEvent::MirrorStopped { desktop_id, reason: stop.reason },
                Message::RemoteStarted(started) => LinkEvent::RemoteStarted {
                    desktop_id,
                    sources: started
                        .sources
                        .into_iter()
                        .map(|source| RemoteSource {
                            window: source.kind == message::RemoteSourceKind::Window,
                            name: source.name,
                        })
                        .collect(),
                    current: started.current,
                },
                Message::RemoteStop(stop) => LinkEvent::RemoteStopped { desktop_id, reason: stop.reason },
                Message::MediaCommand(command) => LinkEvent::PlayerCommand {
                    desktop_id,
                    player: command.player,
                    command: command.command.into(),
                    value: command.value,
                },
                _ => return None,
            }
        }
    })
}

impl LinkClient {
    pub(super) async fn system_command(
        &self,
        desktop_id: String,
        command: message::SystemCommandKind,
        value: Option<u8>,
    ) -> Result<(), LinkError> {
        let (client, id) = (self.client.clone(), parse_id(&desktop_id)?);
        let message = Message::SystemCommand(message::SystemCommand { command, value });
        self.run(async move { client.send(id, message).await }).await
    }

    /// Runs `work` on the client's runtime, which owns the timers and sockets it needs.
    pub(super) async fn run<T: Send + 'static>(
        &self,
        work: impl Future<Output = Result<T, link_core::Error>> + Send + 'static,
    ) -> Result<T, LinkError> {
        let joined = self.runtime.spawn(work).await.map_err(|_| stopped())?;
        Ok(joined?)
    }
}

impl From<client::DesktopLink> for DesktopLink {
    fn from(link: client::DesktopLink) -> Self {
        match link {
            client::DesktopLink::Live => Self::Live,
            client::DesktopLink::Reconnecting => Self::Reconnecting,
            client::DesktopLink::Offline => Self::Offline,
        }
    }
}

pub(super) fn describe(peer: &Peer, link: DesktopLink, bluetooth: bool) -> Desktop {
    let store::Grants { clipboard, files, notifications, media, ring, calls, browse, screen, messages, .. } =
        peer.grants;
    Desktop {
        id: peer.id.to_string(),
        name: peer.name.clone(),
        addresses: peer.addresses.iter().map(ToString::to_string).collect(),
        last_seen: peer.last_seen,
        link,
        bluetooth,
        sharing: Sharing { clipboard, files, notifications, media, ring, calls, browse, screen, messages },
        bluetooth_address: peer.bluetooth.clone(),
        bt_pairing: peer.bt_pairing,
        control: peer.has(message::CONTROL),
    }
}

pub(super) fn parse_id(id: &str) -> Result<DeviceId, LinkError> {
    Ok(DeviceId::parse(id)?)
}

pub(super) fn stopped() -> LinkError {
    LinkError::Failed { reason: "the link runtime stopped".to_owned() }
}

impl From<MediaCommandKind> for message::MediaCommandKind {
    fn from(kind: MediaCommandKind) -> Self {
        match kind {
            MediaCommandKind::Play => Self::Play,
            MediaCommandKind::Pause => Self::Pause,
            MediaCommandKind::PlayPause => Self::PlayPause,
            MediaCommandKind::Next => Self::Next,
            MediaCommandKind::Previous => Self::Previous,
            MediaCommandKind::Seek => Self::Seek,
            MediaCommandKind::Volume => Self::Volume,
        }
    }
}

impl From<message::MediaCommandKind> for MediaCommandKind {
    fn from(kind: message::MediaCommandKind) -> Self {
        match kind {
            message::MediaCommandKind::Play => Self::Play,
            message::MediaCommandKind::Pause => Self::Pause,
            message::MediaCommandKind::PlayPause => Self::PlayPause,
            message::MediaCommandKind::Next => Self::Next,
            message::MediaCommandKind::Previous => Self::Previous,
            message::MediaCommandKind::Seek => Self::Seek,
            message::MediaCommandKind::Volume => Self::Volume,
        }
    }
}

impl From<MediaPlayer> for message::MediaPlayer {
    fn from(player: MediaPlayer) -> Self {
        let state = match player.state {
            PlaybackState::Playing => message::PlaybackState::Playing,
            PlaybackState::Paused => message::PlaybackState::Paused,
            PlaybackState::Stopped => message::PlaybackState::Stopped,
        };
        Self {
            player: player.player,
            name: player.name,
            state,
            title: player.title,
            artist: player.artist,
            album: player.album,
            length_ms: player.length_ms,
            position_ms: player.position_ms,
            volume: player.volume,
            artwork: player.artwork,
            can: player.can.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<message::MediaPlayer> for MediaPlayer {
    fn from(player: message::MediaPlayer) -> Self {
        let state = match player.state {
            message::PlaybackState::Playing => PlaybackState::Playing,
            message::PlaybackState::Paused => PlaybackState::Paused,
            message::PlaybackState::Stopped => PlaybackState::Stopped,
        };
        Self {
            player: player.player,
            name: player.name,
            state,
            title: player.title,
            artist: player.artist,
            album: player.album,
            length_ms: player.length_ms,
            position_ms: player.position_ms,
            volume: player.volume,
            artwork: player.artwork,
            can: player.can.into_iter().map(Into::into).collect(),
        }
    }
}
