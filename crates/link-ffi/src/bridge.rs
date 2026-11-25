use super::{
    Arc, BluetoothLink, CallKind, Contact, FromRawFd, HotspotCredentials, LibraryItem, LoggedCall, MessagesRefusal,
    OwnedFd, PathBuf, PhoneCall, PhoneCamera, PhoneHotspot, PhoneLibrary, PhoneMessages, PhoneText, PhoneTextState,
    PhoneWifiDirect, PhotoMedia, PhotoOutcome, RecentPhoto, RecentPhotos, Text, TextOutcome, message,
};

pub(super) struct Opener(pub(super) Arc<dyn BluetoothLink>);

impl link_core::bluetooth::BluetoothOpener for Opener {
    fn open(&self, address: &str, pair: bool) -> std::io::Result<OwnedFd> {
        adopt(self.0.open(address.to_owned(), pair))
    }

    fn open_l2cap(&self, address: &str) -> std::io::Result<OwnedFd> {
        adopt(self.0.open_l2cap(address.to_owned(), link_core::bluetooth::L2CAP_PSM))
    }

    fn scan_pairing(&self) -> std::io::Result<Vec<String>> {
        let millis = u32::try_from(link_core::bluetooth::PAIRING_SCAN.as_millis()).unwrap_or(u32::MAX);
        Ok(self.0.scan_pairing(link_core::bluetooth::SERVICE_UUID.to_owned(), millis))
    }
}

/// Takes the descriptor `BluetoothLink` returned, or the reason it gave none.
fn adopt(fd: i32) -> std::io::Result<OwnedFd> {
    let reason = match fd {
        0.. => None,
        -2 => Some("the Nearby devices permission is off"),
        -3 => Some("Bluetooth is off on the phone"),
        -4 => Some("the Bluetooth pairing was not accepted"),
        -5 => Some("an RFCOMM link to that address is already open"),
        _ => Some("the desktop did not answer"),
    };
    if let Some(reason) = reason {
        return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, reason));
    }
    // SAFETY: the app hands over a descriptor it detached and no longer uses, so nothing else owns or closes it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(super) struct Library(pub(super) Arc<dyn PhoneLibrary>);

impl link_core::browse::Library for Library {
    fn albums(&self) -> Vec<String> {
        self.0.albums()
    }

    fn album(&self, name: &str) -> Vec<link_core::browse::LibraryItem> {
        let item = |item: LibraryItem| link_core::browse::LibraryItem {
            name: item.name,
            path: PathBuf::from(item.path),
            size: item.size,
            mtime: item.mtime,
        };
        self.0.album(name.to_owned()).into_iter().map(item).collect()
    }

    fn thumbnail(&self, path: &std::path::Path, px: u16) -> Option<Vec<u8>> {
        self.0.thumbnail(path.to_str()?.to_owned(), px)
    }

    fn recent(
        &self,
        offset: usize,
        count: usize,
        media: link_core::proto::message::PhotoMedia,
    ) -> Option<link_core::browse::RecentBatch> {
        use link_core::proto::message::PhotoMedia as Wire;
        let media = match media {
            Wire::All => PhotoMedia::All,
            Wire::Photos => PhotoMedia::Photos,
            Wire::Videos => PhotoMedia::Videos,
        };
        let wide = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let RecentPhotos { total, photos } = self.0.recent(wide(offset), wide(count), media)?;
        let items = photos
            .into_iter()
            .map(|RecentPhoto { album, item, video }| link_core::browse::RecentItem {
                album,
                item: link_core::browse::LibraryItem {
                    name: item.name,
                    path: PathBuf::from(item.path),
                    size: item.size,
                    mtime: item.mtime,
                },
                video,
            })
            .collect();
        Some(link_core::browse::RecentBatch { total: total as usize, items })
    }

    fn trash(&self, path: &std::path::Path) -> bool {
        path.to_str().is_some_and(|path| self.0.trash(path.to_owned()))
    }
}

pub(super) struct Messages(pub(super) Arc<dyn PhoneMessages>);

impl link_core::messages::Messages for Messages {
    fn contacts(&self, offset: u32, limit: u32) -> Result<Vec<Contact>, MessagesRefusal> {
        let contacts = self.0.contacts(offset, limit).ok_or(MessagesRefusal::Denied)?;
        Ok(contacts.into_iter().map(|contact| Contact { name: contact.name, numbers: contact.numbers }).collect())
    }

    fn texts(&self, offset: u32, limit: u32) -> Result<Vec<Text>, MessagesRefusal> {
        let texts = self.0.texts(offset, limit).ok_or(MessagesRefusal::Denied)?;
        let text = |text: PhoneText| Text {
            id: text.id,
            thread: text.thread,
            address: text.address,
            body: text.body,
            date: text.date,
            sent: text.sent,
            state: text.state.map(|state| match state {
                PhoneTextState::Sending => message::TextState::Sending,
                PhoneTextState::Delivered => message::TextState::Delivered,
                PhoneTextState::Failed => message::TextState::Failed,
            }),
            parts: text.parts.into_iter().map(|part| message::TextPart { id: part.id, mime: part.mime }).collect(),
        };
        Ok(texts.into_iter().map(text).collect())
    }

    fn text_part(&self, part: u64, px: u16) -> Option<Vec<u8>> {
        self.0.text_part(part, px)
    }

    fn send_text(&self, to: &str, body: &str) -> Result<(), MessagesRefusal> {
        outcome(self.0.send_text(to.to_owned(), body.to_owned()))
    }

    fn calls(&self, offset: u32, limit: u32) -> Result<Vec<LoggedCall>, MessagesRefusal> {
        let calls = self.0.calls(offset, limit).ok_or(MessagesRefusal::Denied)?;
        let call = |call: PhoneCall| LoggedCall {
            number: call.number,
            name: call.name,
            date: call.date,
            duration: call.duration,
            kind: match call.kind {
                CallKind::Incoming => message::CallKind::Incoming,
                CallKind::Outgoing => message::CallKind::Outgoing,
                CallKind::Missed => message::CallKind::Missed,
            },
        };
        Ok(calls.into_iter().map(call).collect())
    }

    fn dial(&self, number: &str) -> Result<(), MessagesRefusal> {
        outcome(self.0.dial(number.to_owned()))
    }
}

fn outcome(outcome: TextOutcome) -> Result<(), MessagesRefusal> {
    match outcome {
        TextOutcome::Sent => Ok(()),
        TextOutcome::Denied => Err(MessagesRefusal::Denied),
        TextOutcome::Failed => Err(MessagesRefusal::Failed),
    }
}

pub(super) struct Camera(pub(super) Arc<dyn PhoneCamera>);

impl link_core::camera::Camera for Camera {
    fn take(&self, desktop: &str) -> Result<(String, String), message::PhotoRefusal> {
        match self.0.take_photo(desktop.to_owned()) {
            PhotoOutcome::Taken { album, name } => Ok((album, name)),
            PhotoOutcome::Cancelled => Err(message::PhotoRefusal::Cancelled),
            PhotoOutcome::Denied => Err(message::PhotoRefusal::Denied),
            PhotoOutcome::Failed => Err(message::PhotoRefusal::Failed),
        }
    }
}

pub(super) struct Hotspots(pub(super) Arc<dyn PhoneHotspot>);

impl link_core::hotspot::HotspotProvider for Hotspots {
    fn start(&self) -> std::io::Result<link_core::proto::message::Hotspot> {
        let started = self.0.start().ok_or_else(|| std::io::Error::other("Android did not start the hotspot"))?;
        Ok(link_core::proto::message::Hotspot { ssid: started.ssid, passphrase: started.passphrase.into(), mhz: None })
    }

    fn stop(&self) {
        self.0.stop();
    }
}

pub(super) struct WifiDirects(pub(super) Arc<dyn PhoneWifiDirect>);

impl link_core::hotspot::WifiDirectProvider for WifiDirects {
    fn start(&self, group: &link_core::proto::message::Hotspot) -> std::io::Result<link_core::proto::message::Hotspot> {
        let wanted =
            HotspotCredentials { ssid: group.ssid.clone(), passphrase: group.passphrase.to_string(), mhz: None };
        let group = self.0.start(wanted).ok_or_else(|| std::io::Error::other("Android did not create the group"))?;
        Ok(link_core::proto::message::Hotspot { ssid: group.ssid, passphrase: group.passphrase.into(), mhz: group.mhz })
    }

    fn stop(&self) {
        self.0.stop();
    }
}
