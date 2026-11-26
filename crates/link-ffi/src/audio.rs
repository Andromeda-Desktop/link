//! Phone sound and microphone as Opus units: libopus encodes each 20 ms frame in the caller's thread, a runtime task
//! sends it, so a slow network never holds the capture back. `MediaCodec`'s Opus encoder releases packets 3-48 ms
//! late, which the desktop must buffer away. `LowDelay` (CELT only) looks ahead 2.5 ms, not 6.5.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use link_core::mirror::{MirrorSender, Unit};
use link_core::proto::message::MIRROR_AUDIO;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::LinkError;
use crate::convert::stopped;

const RATE: u32 = 48_000;
/// Units waiting for the network; above it the oldest goes (the desktop's decoder conceals the gap).
const QUEUE: usize = 8;
/// libopus's largest packet.
const MAX_PACKET: usize = 4000;
/// How long [`AudioUnits::finish`] lets queued units reach the desktop.
const DRAIN: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    /// The sender sends what is queued, then ends.
    Closing,
    /// The desktop is gone.
    Failed,
}

struct Queue {
    units: VecDeque<Unit>,
    state: State,
}

struct Shared {
    queue: Mutex<Queue>,
    ready: Notify,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Encodes 20 ms frames of interleaved 16-bit PCM and sends each as one audio unit; from
/// [`crate::MirrorSession::audio`].
#[derive(uniffi::Object)]
pub struct AudioUnits {
    encoder: Mutex<opus::Encoder>,
    frame: usize,
    shared: Arc<Shared>,
    runtime: tokio::runtime::Handle,
    sending: Mutex<Option<JoinHandle<()>>>,
}

impl AudioUnits {
    pub(crate) fn start(
        runtime: &tokio::runtime::Handle,
        sender: Arc<MirrorSender>,
        channels: u8,
        bitrate: u32,
    ) -> Result<Arc<Self>, LinkError> {
        let layout = match channels {
            1 => opus::Channels::Mono,
            2 => opus::Channels::Stereo,
            _ => return Err(LinkError::Failed { reason: format!("{channels} audio channels") }),
        };
        let failure = |error: opus::Error| LinkError::Failed { reason: error.to_string() };
        let mut encoder = opus::Encoder::new(RATE, layout, opus::Application::LowDelay).map_err(failure)?;
        encoder.set_bitrate(opus::Bitrate::Bits(i32::try_from(bitrate).unwrap_or(i32::MAX))).map_err(failure)?;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue { units: VecDeque::with_capacity(QUEUE), state: State::Open }),
            ready: Notify::new(),
        });
        let sending = runtime.spawn(send(shared.clone(), sender));
        let frame = RATE as usize / 50 * usize::from(channels);
        Ok(Arc::new(Self {
            encoder: Mutex::new(encoder),
            frame,
            shared,
            runtime: runtime.clone(),
            sending: Mutex::new(Some(sending)),
        }))
    }
}

/// Sends queued units in order until the queue closes empty or a send fails.
async fn send(shared: Arc<Shared>, sender: Arc<MirrorSender>) {
    loop {
        let next = {
            let mut queue = shared.lock();
            match (queue.units.pop_front(), queue.state) {
                (Some(unit), _) => Some(unit),
                (None, State::Open) => None,
                (None, State::Closing | State::Failed) => return,
            }
        };
        let Some(unit) = next else {
            shared.ready.notified().await;
            continue;
        };
        if let Err(error) = sender.send(&unit).await {
            log::debug!("audio unit not sent: {error}");
            let mut queue = shared.lock();
            queue.state = State::Failed;
            queue.units.clear();
            return;
        }
    }
}

#[uniffi::export]
impl AudioUnits {
    /// One 20 ms frame (960 samples per channel, little-endian) captured at `pts_us`; returns at once. Fails once
    /// the desktop is gone or after [`AudioUnits::finish`].
    pub fn push(&self, pts_us: u64, pcm: &[u8]) -> Result<(), LinkError> {
        if self.shared.lock().state != State::Open {
            return Err(stopped());
        }
        let samples: Vec<i16> = pcm.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)).collect();
        if samples.len() != self.frame {
            return Err(LinkError::Failed { reason: format!("{} samples, not one 20 ms frame", samples.len()) });
        }
        let mut data = vec![0; MAX_PACKET];
        let len = {
            let mut encoder = self.encoder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            encoder.encode(&samples, &mut data).map_err(|error| LinkError::Failed { reason: error.to_string() })?
        };
        data.truncate(len);
        let mut queue = self.shared.lock();
        if queue.state != State::Open {
            return Err(stopped());
        }
        if queue.units.len() == QUEUE {
            queue.units.pop_front();
            log::debug!("audio unit dropped: the network is behind");
        }
        queue.units.push_back(Unit { pts_us, flags: MIRROR_AUDIO, data });
        drop(queue);
        self.shared.ready.notify_one();
        Ok(())
    }

    /// Stops taking frames and returns once the queued ones are sent, or after 500 ms.
    pub async fn finish(&self) {
        {
            let mut queue = self.shared.lock();
            if queue.state == State::Open {
                queue.state = State::Closing;
            }
        }
        self.shared.ready.notify_one();
        let sending = self.sending.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(sending) = sending {
            let drained = self.runtime.spawn(async move { tokio::time::timeout(DRAIN, sending).await }).await;
            if !matches!(drained, Ok(Ok(_))) {
                log::debug!("audio units still queued at finish");
            }
        }
    }
}
