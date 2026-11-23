use super::{
    Admission, Checkup, CloseCode, Command, DesktopStatus, DeviceId, Feature, Gate, Hello, HubHandle, MediaPlayer,
    RemoteStream, Route, SYSTEM_TIMEOUT, Sent, SessionHandle, Share, Source, Spki, StoredCommand, SystemCommand,
    TransferHandle, TransferId, browse, oneshot,
};

impl HubHandle {
    async fn request<T>(&self, make: impl FnOnce(oneshot::Sender<T>) -> Command) -> Option<T> {
        let (reply, response) = oneshot::channel();
        self.commands.send(make(reply)).await.ok()?;
        response.await.ok()
    }

    async fn tell(&self, command: Command) {
        if self.commands.send(command).await.is_err() {
            log::debug!("hub stopped");
        }
    }

    pub async fn start_pairing(&self) -> anyhow::Result<(String, String)> {
        self.request(|reply| Command::StartPairing { reply })
            .await
            .unwrap_or_else(|| Err(anyhow::anyhow!("hub stopped")))
    }

    pub async fn cancel_pairing(&self) {
        self.tell(Command::CancelPairing).await;
    }

    /// Whether a phone with a session just reported `passkey` for the Bluetooth pairing under way; it counts once.
    pub async fn confirm_bt_pairing(&self, passkey: u32) -> bool {
        self.request(|reply| Command::ConfirmBtPairing { passkey, reply }).await.unwrap_or(false)
    }

    pub async fn answer_hotspot(&self, id: DeviceId, allow: bool) {
        self.tell(Command::AnswerHotspot { id, allow }).await;
    }

    pub async fn unpair(&self, id: DeviceId) -> bool {
        self.request(|reply| Command::Unpair { id, reply }).await.unwrap_or(false)
    }

    /// Disconnects `id` and keeps it out until `paused` is false; false for an unknown device.
    pub async fn set_paused(&self, id: DeviceId, paused: bool) -> bool {
        self.request(|reply| Command::SetPaused { id, paused, reply }).await.unwrap_or(false)
    }

    pub async fn admit(&self, spki: Spki) -> Admission {
        self.request(|reply| Command::Admit { spki, reply }).await.unwrap_or(Admission::Reject(CloseCode::Busy))
    }

    pub async fn paired(&self, spki: Spki, name: String, window: u64) {
        self.tell(Command::Paired { spki, name, window }).await;
    }

    pub async fn pairing_failed(&self, reason: String, window: u64) {
        self.tell(Command::PairingFailed { reason, window }).await;
    }

    /// Frees window `window` for the next attempt: the last one failed before it could guess the code.
    pub async fn release_window(&self, window: u64) {
        self.tell(Command::ReleaseWindow { window }).await;
    }

    pub async fn connected(&self, id: DeviceId, hello: Hello, session: SessionHandle) {
        self.tell(Command::Connected { id, hello, session }).await;
    }

    /// Asks the phone to mirror its screen; answered once the user allowed it there and the video stream arrived.
    pub async fn open_mirror(
        &self,
        id: DeviceId,
        codecs: Vec<link_core::proto::message::Codec>,
    ) -> Result<crate::mirror::Opened, crate::mirror::Failure> {
        let (reply, answer) = oneshot::channel();
        let ask = crate::mirror::Ask { codecs, reply };
        if self.commands.send(Command::OpenMirror { id, ask }).await.is_err() {
            return Err(crate::mirror::Failure::NotConnected);
        }
        answer.await.unwrap_or(Err(crate::mirror::Failure::NotConnected))
    }

    /// Starts or switches a phone's remote control to `stream`.
    pub async fn serve_remote(&self, id: DeviceId, stream: RemoteStream) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.tell(Command::ServeRemote { id, stream, reply }).await;
        answer.await.unwrap_or_else(|_| Err("the daemon is stopping".to_owned()))
    }

    pub async fn stop_remote(&self, id: DeviceId, reason: String) {
        self.tell(Command::StopRemote { id, reason }).await;
    }

    /// Sends a text through the phone; answered once the phone reports it sent or failed.
    pub async fn send_text(&self, id: DeviceId, to: String, body: String) -> Result<(), crate::messages::SendFailure> {
        let (reply, answer) = oneshot::channel();
        if self.commands.send(Command::SendText { id, to, body, reply }).await.is_err() {
            return Err(crate::messages::SendFailure::NotConnected);
        }
        answer.await.unwrap_or(Err(crate::messages::SendFailure::NotConnected))
    }

    /// Places a call on the phone; answered once Android took it, or with why not.
    pub async fn dial(&self, id: DeviceId, number: String) -> Result<(), crate::messages::SendFailure> {
        let (reply, answer) = oneshot::channel();
        if self.commands.send(Command::Dial { id, number, reply }).await.is_err() {
            return Err(crate::messages::SendFailure::NotConnected);
        }
        answer.await.unwrap_or(Err(crate::messages::SendFailure::NotConnected))
    }

    /// Has the phone take a photo; answered with its browse path once its user took it, or with why not.
    pub async fn take_photo(&self, id: DeviceId) -> Result<String, crate::messages::PhotoFailure> {
        let (reply, answer) = oneshot::channel();
        if self.commands.send(Command::TakePhoto { id, reply }).await.is_err() {
            return Err(crate::messages::PhotoFailure::NotConnected);
        }
        answer.await.unwrap_or(Err(crate::messages::PhotoFailure::NotConnected))
    }

    /// The device's live session, if it has one.
    pub async fn browse(&self, id: DeviceId, request: browse::Request) -> Result<browse::Answer, browse::Failure> {
        let (reply, answer) = oneshot::channel();
        if self.commands.send(Command::Browse { id, request, reply }).await.is_err() {
            return Err(browse::Failure::NotConnected);
        }
        answer.await.unwrap_or(Err(browse::Failure::NotConnected))
    }

    /// Queues `share` for the phone; returns once the phone acknowledged it, or when it waits for the phone's next
    /// session.
    pub async fn share(&self, id: DeviceId, share: Share) -> Result<Sent, link_core::Error> {
        self.outbox.share(id, share).await
    }

    /// The live session to send a message to that needs `needs` shared by the phone.
    pub async fn session(&self, id: DeviceId, needs: Option<Feature>) -> Result<SessionHandle, Gate> {
        self.request(|reply| Command::Session { id, needs, reply }).await.unwrap_or(Err(Gate::NotConnected))
    }

    /// Records what the `link-session` at D-Bus name `owner` can do, and tells connected phones.
    pub async fn set_capabilities(&self, owner: String, can: Vec<String>) {
        self.tell(Command::SetCapabilities { owner, can }).await;
    }

    /// Records the desktop's state and sends it to connected phones when it changed.
    pub async fn set_desktop_status(&self, status: DesktopStatus) {
        self.tell(Command::SetDesktopStatus { status }).await;
    }

    /// Answers phone `id`'s `system-command` `serial`: done, or the error for the user.
    pub async fn system_result(&self, id: DeviceId, serial: u64, error: Option<String>) {
        self.tell(Command::SystemResult { id, serial, error }).await;
    }

    /// Sends phone `id` a command and waits up to [`SYSTEM_TIMEOUT`] for its answer.
    pub async fn send_system_command(&self, id: DeviceId, command: SystemCommand) -> Result<(), String> {
        let answer = self.request(|reply| Command::SystemToPhone { id, command, reply });
        match tokio::time::timeout(SYSTEM_TIMEOUT, answer).await {
            Ok(Some(result)) => result,
            Ok(None) => Err("the phone did not answer".to_owned()),
            Err(_) => Err("the phone did not answer in time".to_owned()),
        }
    }

    /// Forgets the capabilities `owner` reported, when it still holds them.
    pub async fn capabilities_gone(&self, owner: String) {
        self.tell(Command::CapabilitiesGone { owner }).await;
    }

    /// Replaces the commands phones may run; false when the list breaks its limits.
    pub async fn set_commands(&self, commands: Vec<StoredCommand>) -> bool {
        self.request(|reply| Command::SetCommands { commands, reply }).await.unwrap_or(false)
    }

    /// Every paired device's state, path, round trip, losses and outbox depth.
    pub async fn doctor(&self) -> Vec<Checkup> {
        let mut checkups = self.request(|reply| Command::Doctor { reply }).await.unwrap_or_default();
        let depths = self.outbox.depths().await;
        for checkup in &mut checkups {
            checkup.outbox = depths.get(&checkup.id).copied().unwrap_or(0);
        }
        checkups
    }

    pub async fn ping(&self, id: DeviceId, message: Option<String>) -> Result<(), link_core::Error> {
        self.request(|reply| Command::Ping { id, message, reply }).await.unwrap_or(Err(link_core::Error::Stopped))
    }

    pub async fn set_grant(&self, id: DeviceId, feature: String, granted: bool) -> bool {
        self.request(|reply| Command::SetGrant { id, feature, granted, reply }).await.unwrap_or(false)
    }

    /// Connected devices holding the clipboard grant.
    pub async fn clipboard_peers(&self) -> Vec<DeviceId> {
        self.request(|reply| Command::ClipboardPeers { reply }).await.unwrap_or_default()
    }

    /// Offers files to a Link device or a `localsend:<fingerprint>` peer.
    pub async fn send_files(&self, device: String, sources: Vec<Source>) -> Result<TransferId, link_core::Error> {
        self.request(|reply| Command::SendFiles { device, sources, reply })
            .await
            .unwrap_or(Err(link_core::Error::Stopped))
    }

    /// Answers an offer from any backend; false when it no longer waits.
    pub async fn decide(&self, id: TransferId, accept: bool) -> bool {
        self.request(|reply| Command::Decide { id, accept, reply }).await.unwrap_or(false)
    }

    pub async fn cancel_transfer(&self, id: TransferId) -> bool {
        self.request(|reply| Command::CancelTransfer { id, reply }).await.unwrap_or(false)
    }

    pub async fn set_localsend(&self, visible: bool) {
        self.tell(Command::SetLocalSend { visible }).await;
    }

    pub async fn set_notify_locked(&self, on: bool) {
        self.tell(Command::SetNotifyLocked { on }).await;
    }

    pub async fn set_auto_accept(&self, id: DeviceId, enabled: bool) -> bool {
        self.request(|reply| Command::SetAutoAccept { id, enabled, reply }).await.unwrap_or(false)
    }

    pub fn transfers(&self) -> &TransferHandle {
        &self.transfers
    }

    /// Where a session delivers what its peer sends.
    pub fn route(&self, peer: DeviceId) -> Route {
        Route {
            peer,
            events: self.session_events.clone(),
            transfers: self.transfers.clone(),
            heartbeat: false,
            path: None,
        }
    }

    pub async fn desktop_player(&self, player: MediaPlayer) {
        self.tell(Command::DesktopPlayer { player }).await;
    }

    pub async fn desktop_player_gone(&self, player: String) {
        self.tell(Command::DesktopPlayerGone { player }).await;
    }

    pub async fn media_key(&self, key: &'static str) {
        self.tell(Command::MediaKey { key }).await;
    }

    pub async fn disconnected(&self, id: DeviceId, stable_id: usize) {
        self.tell(Command::Disconnected { id, stable_id }).await;
    }
}
