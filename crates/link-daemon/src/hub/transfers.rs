use super::{Backend, DeviceId, Event, Hub, Share, Signal, Source, TransferEvent, TransferId};
use link_core::proto::message::MIRROR_LAG;

impl Hub {
    /// Consent and grants are the hub's: an offer from a device without the files grant is declined, one from an
    /// auto-accept device accepted, any other goes to the shell; a clip from a device without the clipboard grant is
    /// dropped.
    pub(super) async fn on_transfer(&mut self, event: TransferEvent) {
        let grants = |id: &DeviceId| self.store.peer(id).map(|peer| peer.grants).unwrap_or_default();
        match &event {
            TransferEvent::Busy { .. } => return,
            TransferEvent::Offered { id, from, .. } if !grants(from).files => {
                log::info!("{from}: declining transfer {id}: no files grant");
                if let Err(error) = self.transfers.decide(*id, false).await {
                    log::error!("declining {id}: {error}");
                }
                return;
            }
            TransferEvent::ClipOffered { from, .. } if !grants(from).clipboard => {
                log::info!("{from}: dropping a clipboard offer: no clipboard grant");
                return;
            }
            TransferEvent::Offered { id, from, .. } if self.store.peer(from).is_some_and(|peer| peer.auto_accept) => {
                log::info!("{from}: auto-accepting transfer {id}");
                if let Err(error) = self.transfers.decide(*id, true).await {
                    log::error!("accepting {id}: {error}");
                }
                return;
            }
            _ => {}
        }
        let signal = match event {
            TransferEvent::Offered { id, from, files } => {
                let files = files.into_iter().map(|file| (file.name, file.size)).collect();
                Signal::Offered { id, device: from.to_string(), files }
            }
            TransferEvent::Progress { id, bytes, total } => Signal::Progress { id, bytes, total },
            TransferEvent::Finished { id, peer, incoming, status, files } => {
                if incoming {
                    let name = self.store.peer(&peer).map_or("", |known| known.name.as_str());
                    for file in &files {
                        crate::save::mark_origin(&file.path, peer.as_str(), name);
                    }
                }
                Signal::Finished { id, status, paths: files.into_iter().map(|file| file.path).collect() }
            }
            TransferEvent::ClipOffered { from, id, mimes, size, .. } => {
                return self.emit(Event::ClipboardOffered { from, id, mimes, size }).await;
            }
            TransferEvent::Busy { .. } => return,
            TransferEvent::Mic { from, stream } => {
                self.mics.stream(from, &stream, None);
                return;
            }
            TransferEvent::Sound { from, stream } => {
                self.sounds.stream(from, &stream, None);
                return;
            }
            TransferEvent::Camera { from, stream } => {
                log::info!("{from}: refusing a camera stream; this desktop asked for none");
                if let Some(mut receiver) = stream.take() {
                    receiver.stop();
                }
                return;
            }
            TransferEvent::Speaker { from, stream } => {
                log::info!("{from}: refusing a speaker stream; only phones play one");
                if let Some(mut receiver) = stream.take() {
                    receiver.stop();
                }
                return;
            }
            TransferEvent::Mirror { from, stream } => {
                let lag = self.sessions.get(&from).filter(|_| self.phone_has(&from, MIRROR_LAG)).cloned();
                self.mirrors.stream(from, &stream, lag);
                return self.publish();
            }
        };
        self.on_signal(Backend::Link, signal).await;
    }

    /// Tracks which backend owns each open transfer and passes the signal to D-Bus.
    pub(super) async fn on_signal(&mut self, backend: Backend, signal: Signal) {
        match &signal {
            Signal::Offered { id, .. } | Signal::Progress { id, .. } => {
                self.open.entry(*id).or_insert(backend);
            }
            Signal::Finished { id, .. } => {
                self.open.remove(id);
            }
        }
        self.emit(Event::Transfer(signal)).await;
    }

    /// A `localsend:<fingerprint>` device goes to LocalSend, anything else to Link.
    pub(super) async fn send_files(
        &mut self,
        device: &str,
        sources: Vec<Source>,
    ) -> Result<TransferId, link_core::Error> {
        if let Some(fingerprint) = device.strip_prefix(crate::localsend::PREFIX) {
            let id = self
                .localsend
                .send(fingerprint.to_owned(), sources)
                .await
                .map_err(|_| link_core::Error::NotConnected)?;
            self.open.insert(id, Backend::LocalSend);
            return Ok(id);
        }
        let id = self.transfers.send(DeviceId::parse(device)?, sources).await?;
        self.open.insert(id, Backend::Link);
        Ok(id)
    }

    /// Applies a share once: a phone resends one whose ack was lost.
    pub(super) async fn on_share(&mut self, from: DeviceId, share: Share) {
        if self.seen.first(&from, &share) {
            self.emit(Event::Received { id: from, share }).await;
        } else {
            log::info!("{from}: share {:016x} dropped: it came before", share.item.unwrap_or(0));
        }
    }
}
