use super::{
    Admission, Arc, Attempt, CloseCode, Event, Hub, Instant, PairingUri, Peer, Secret, Secrets, Spki, WINDOW, Window,
    generate_secrets, net, watch,
};

impl Hub {
    /// Names the adapter in pairing QR codes and opens it to pairing during each window; before [`Self::run`].
    pub fn set_bluetooth(
        &mut self,
        address: String,
        open: watch::Sender<Option<Instant>>,
        beacon: watch::Sender<bool>,
    ) {
        self.bluetooth = Some((address, open));
        self.beacon = Some(beacon);
    }

    /// Keeps the adapter accepting unpaired devices until at least `until`.
    pub(super) fn open_bluetooth_until(&self, until: Instant) {
        if let Some((_, open)) = &self.bluetooth {
            open.send_modify(|deadline| *deadline = Some(deadline.map_or(until, |deadline| deadline.max(until))));
        }
    }

    pub(super) fn start_pairing(&mut self) -> anyhow::Result<(String, String)> {
        let (code, qr) = generate_secrets()?;
        let uri = PairingUri {
            fingerprint: self.own.fingerprint(),
            secret: qr,
            addresses: net::local_addresses(self.store.port),
            bluetooth: self.bluetooth.as_ref().map(|(address, _)| address.clone()),
        };
        self.open_bluetooth_until(Instant::now() + WINDOW);
        let secrets = Secrets { code: Secret::new(code.clone().into_bytes()), qr: Secret::new(qr.to_vec()) };
        self.windows_opened += 1;
        self.window = Some(Window {
            id: self.windows_opened,
            secrets: Arc::new(secrets),
            deadline: Instant::now() + WINDOW,
            taken: false,
        });
        self.refresh();
        log::info!("pairing window open");
        Ok((code, uri.to_string()))
    }

    pub(super) fn close_window(&mut self) {
        if self.window.take().is_some() {
            self.refresh();
        }
    }

    /// Closes the window only if it is still `id`, so an attempt on a replaced window leaves the new one open.
    pub(super) fn close_window_if(&mut self, id: u64) -> bool {
        if self.window.as_ref().is_none_or(|window| window.id != id) {
            return false;
        }
        self.close_window();
        true
    }

    pub(super) fn release_window(&mut self, id: u64) {
        if let Some(window) = self.window.as_mut().filter(|window| window.id == id) {
            window.taken = false;
        }
    }

    pub(super) fn admit(&mut self, spki: &Spki) -> Admission {
        let id = spki.device_id();
        if let Some(index) = self.store.revoked.iter().position(|revoked| *revoked == id) {
            self.store.revoked.remove(index);
            self.save();
            return Admission::Reject(CloseCode::Unpaired);
        }
        if self.store.peer(&id).is_some_and(|peer| peer.paused) {
            return Admission::Reject(CloseCode::Paused);
        }
        if self.store.peer(&id).is_some() {
            return Admission::Session;
        }
        match &mut self.window {
            Some(window) if window.taken => Admission::Reject(CloseCode::Busy),
            Some(window) => {
                log::info!("{id}: pairing attempt on window {}", window.id);
                window.taken = true;
                Admission::Pair(Attempt { secrets: window.secrets.clone(), window: window.id })
            }
            None => Admission::Reject(CloseCode::NotPaired),
        }
    }

    pub(super) async fn paired(&mut self, spki: &Spki, name: String, window: u64) {
        let mut peer = Peer::new(spki, name.clone());
        peer.touch();
        let id = peer.id.clone();
        self.store.upsert(peer);
        self.save();
        log::info!("paired {id} ({name:?})");
        if self.close_window_if(window) {
            self.emit(Event::PairingFinished { id, name }).await;
        } else {
            self.refresh();
        }
    }
}
