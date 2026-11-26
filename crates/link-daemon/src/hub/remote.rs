use std::os::fd::OwnedFd;

use crate::mirror::Failure;

use link_core::proto::message::{MIRROR_CODECS, MirrorRequest, SPEAKER};

use super::{DeviceId, Event, Feature, Hub, Message, Parked, RemoteStream, net};

impl Hub {
    /// Asks the phone to mirror, only over IP: on Bluetooth, which video does not fit through, the request waits for
    /// the session to move to the phone's hotspot.
    pub(super) async fn open_mirror(&mut self, id: DeviceId, mut ask: crate::mirror::Ask) {
        use crate::mirror::Failure;
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            return drop(ask.reply.send(Err(Failure::NotConnected)));
        };
        if !self.shares(&id, Feature::Screen) {
            return drop(ask.reply.send(Err(Failure::Refused("not-allowed".to_owned()))));
        }
        if session.on_bluetooth() {
            log::info!("{id}: mirroring waits for the phone's hotspot");
            return self.wait_for_hotspot(id, Parked::Mirror(ask)).await;
        }
        if self.mirrors.busy(&id) {
            return drop(ask.reply.send(Err(Failure::Busy)));
        }
        if !self.phone_has(&id, MIRROR_CODECS) {
            ask.codecs.clear();
        }
        log::info!("{id}: asking the phone to mirror its screen");
        let request = MirrorRequest { codecs: ask.codecs.clone() };
        match session.send(Message::MirrorRequest(request)).await {
            Ok(()) => self.mirrors.wait(id, ask),
            Err(error) => drop(ask.reply.send(Err(Failure::Failed(error.to_string())))),
        }
    }

    pub(super) async fn tell_mirror_stop(&self, id: &DeviceId, reason: &str) {
        let stop = Message::MirrorStop(link_core::proto::message::MirrorStop { reason: Some(reason.to_owned()) });
        if let Some(session) = self.sessions.get(id)
            && let Err(error) = session.send(stop).await
        {
            log::info!("{id}: telling the phone to stop mirroring: {error}");
        }
    }

    /// Passes a phone's request for the desktop's screen to `link-session`, only over the same network: a Bluetooth
    /// session is too slow, and an overlay network may be far away, where nobody sees the desktop being controlled.
    pub(super) async fn remote_request(&mut self, id: DeviceId, source: Option<u32>) {
        use link_core::proto::message::CAN_SCREEN;
        let quic = self
            .sessions
            .get(&id)
            .filter(|session| !session.on_bluetooth())
            .map(|session| session.connection().quic().clone());
        let refusal = match quic {
            None => Some("remote control needs the same Wi-Fi as the desktop"),
            Some(quic) if net::is_overlay(quic.remote_address().ip()) => {
                Some("remote control works only on the same Wi-Fi as the desktop")
            }
            Some(_) if !self.capabilities().iter().any(|can| can == CAN_SCREEN) => {
                Some("this desktop cannot share its screen")
            }
            Some(_) => None,
        };
        if let Some(reason) = refusal {
            log::info!("{id}: refusing remote control: {reason}");
            return self.tell_remote_stop(&id, reason).await;
        }
        log::info!("{id}: the phone asks for the screen (source {source:?})");
        self.emit(Event::RemoteRequested { id, source }).await;
    }

    /// Tells the phone what it shows, then opens the video stream and pumps `link-session`'s socket into it.
    /// Asks the phone for its microphone or its sound; `ask` gets the socket of its Opus units once the phone
    /// streams them.
    pub(super) async fn open_audio(&mut self, id: DeviceId, audio: crate::mirror::Audio, ask: crate::mirror::Ask) {
        let Some(session) = self.sessions.get(&id).filter(|session| session.is_live()).cloned() else {
            return drop(ask.reply.send(Err(Failure::NotConnected)));
        };
        if !self.phone_has(&id, audio.feature()) {
            return drop(ask.reply.send(Err(Failure::Refused("unsupported".to_owned()))));
        }
        if !self.shares(&id, audio.grant()) {
            return drop(ask.reply.send(Err(Failure::Refused("not-allowed".to_owned()))));
        }
        if self.audio(audio).busy(&id) {
            return drop(ask.reply.send(Err(Failure::Busy)));
        }
        log::info!("{id}: asking the phone for its {}", audio.name());
        match session.send(audio.request()).await {
            Ok(()) => self.audio(audio).wait(id, ask),
            Err(error) => drop(ask.reply.send(Err(Failure::Failed(error.to_string())))),
        }
    }

    pub(super) async fn tell_audio_stop(&self, id: &DeviceId, audio: crate::mirror::Audio) {
        if let Some(session) = self.sessions.get(id)
            && let Err(error) = session.send(audio.stop()).await
        {
            log::info!("{id}: telling the phone to stop its {}: {error}", audio.name());
        }
    }

    /// Refused when the phone plays no speaker streams or its switch is off; a new stream replaces the phone's last.
    pub(super) async fn open_speaker(&mut self, id: DeviceId) -> Result<OwnedFd, Failure> {
        let session = self.sessions.get(&id).filter(|session| session.is_live()).cloned();
        let session = session.ok_or(Failure::NotConnected)?;
        if !self.phone_has(&id, SPEAKER) {
            return Err(Failure::Refused("unsupported".to_owned()));
        }
        if !self.shares(&id, Feature::Speaker) {
            return Err(Failure::Refused("not-allowed".to_owned()));
        }
        let failed = |error: &dyn std::fmt::Display| Failure::Failed(error.to_string());
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().map_err(|error| failed(&error))?;
        let sender = link_core::mirror::MirrorSender::open_sound(&session, Message::SpeakerData)
            .await
            .map_err(|error| failed(&error))?;
        log::info!("{id}: playing desktop sound on the phone");
        self.speakers.serve(id, ours.into(), sender).map_err(|error| failed(&error))?;
        Ok(theirs.into())
    }

    pub(super) async fn serve_remote(&mut self, id: DeviceId, stream: RemoteStream) -> Result<(), String> {
        let session = self.sessions.get(&id).filter(|session| session.is_live()).cloned();
        let session = session.ok_or_else(|| format!("{id} is not connected"))?;
        session.send(Message::RemoteStarted(stream.started)).await.map_err(|error| error.to_string())?;
        let sender =
            match link_core::mirror::MirrorSender::open(&session, stream.width, stream.height, None, false).await {
                Ok(sender) => sender,
                Err(error) => {
                    // The phone already shows remote control as started.
                    self.tell_remote_stop(&id, "the desktop could not open the screen stream").await;
                    return Err(error.to_string());
                }
            };
        log::info!("{id}: showing the screen at {}x{}", stream.width, stream.height);
        self.remotes.serve(id, stream.socket, sender).map_err(|error| error.to_string())
    }

    pub(super) async fn stop_remote(&mut self, id: &DeviceId, reason: &str) {
        self.remotes.stop(id);
        self.tell_remote_stop(id, reason).await;
    }

    pub(super) async fn tell_remote_stop(&self, id: &DeviceId, reason: &str) {
        let stop = Message::RemoteStop(link_core::proto::message::MirrorStop { reason: Some(reason.to_owned()) });
        if let Some(session) = self.sessions.get(id)
            && let Err(error) = session.send(stop).await
        {
            log::info!("{id}: telling the phone to stop remote control: {error}");
        }
    }
}
