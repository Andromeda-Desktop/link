//! Mirroring a phone's screen for `andromeda-link-mirror`: the request, the wait for the user's consent on the phone,
//! and the pump from the phone's video stream into the viewer's socket, in the stream's own framing.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::time::Duration;

use link_core::identity::DeviceId;
use link_core::mirror::{MirrorReceiver, MirrorStream};
use link_core::proto::message::{
    Codec, MAX_MIRROR_LAG_MS, MIRROR_AUDIO, MIRROR_CONFIG, MIRROR_END, Message, MirrorLag,
};
use link_core::session::SessionHandle;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

/// Android asks the user every time; they may take a while to find the phone.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(60);

/// How often a phone that takes `mirror-lag` hears how late its video arrives.
const LAG_REPORT: Duration = Duration::from_millis(500);

pub struct Opened {
    /// The viewer's end of a socket carrying the phone's access units.
    pub stream: OwnedFd,
    pub width: u32,
    pub height: u32,
    /// H.264 when None.
    pub codec: Option<Codec>,
}

#[derive(Debug)]
pub enum Failure {
    NotConnected,
    /// The session is on Bluetooth, which carries tens of KB/s, and the phone's hotspot did not come up.
    NeedsWifi,
    Busy,
    Refused(String),
    TimedOut,
    Failed(String),
}

pub type Reply = oneshot::Sender<Result<Opened, Failure>>;

/// A viewer's request: `codecs` is what it decodes besides H.264, best first.
pub struct Ask {
    pub codecs: Vec<Codec>,
    pub reply: Reply,
}

#[derive(Default)]
pub struct Mirrors {
    waiting: HashMap<DeviceId, (Ask, Instant)>,
    /// Each running pump's stop, which ends it with [`MIRROR_END`].
    running: HashMap<DeviceId, oneshot::Sender<()>>,
    pumps: JoinSet<DeviceId>,
    wifi: WifiPower,
    /// A microphone's streams: Wi-Fi power save stays with the screen mirrors' instance, which runs its helper.
    sound_only: bool,
}

impl Mirrors {
    pub fn sound_only() -> Self {
        Self { sound_only: true, ..Self::default() }
    }

    fn wifi_awake(&mut self, awake: bool) {
        if !self.sound_only {
            self.wifi.awake(awake);
        }
    }

    /// A phone's screen shows on this desktop.
    pub fn active(&self) -> bool {
        !self.running.is_empty()
    }

    pub fn busy(&self, id: &DeviceId) -> bool {
        self.waiting.contains_key(id) || self.running.contains_key(id)
    }

    pub fn wait(&mut self, id: DeviceId, ask: Ask) {
        self.waiting.insert(id, (ask, Instant::now() + CONSENT_TIMEOUT));
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.waiting.values().map(|(_, deadline)| *deadline).min()
    }

    /// Answers the requests whose consent timed out, and returns their phones, which are told to stop.
    pub fn expire(&mut self) -> Vec<DeviceId> {
        let now = Instant::now();
        let expired: Vec<DeviceId> =
            self.waiting.iter().filter(|(_, (_, deadline))| *deadline <= now).map(|(id, _)| id.clone()).collect();
        for id in &expired {
            self.fail(id, Failure::TimedOut);
        }
        expired
    }

    pub fn fail(&mut self, id: &DeviceId, failure: Failure) {
        if let Some((ask, _)) = self.waiting.remove(id) {
            drop(ask.reply.send(Err(failure)));
        }
    }

    /// The phone stopped: a waiting request is refused with its reason, a running mirror ends.
    pub fn stopped(&mut self, id: &DeviceId, reason: Option<String>) {
        self.fail(id, Failure::Refused(reason.unwrap_or_else(|| "the phone stopped mirroring".to_owned())));
        if let Some(stop) = self.running.remove(id) {
            let _ = stop.send(());
        }
        self.wifi_awake(self.active());
    }

    /// The phone's video stream: handed to the request waiting for it, or refused. `lag` is the session of a phone
    /// that takes `mirror-lag` reports.
    pub fn stream(&mut self, from: DeviceId, stream: &MirrorStream, lag: Option<SessionHandle>) {
        let Some(mut receiver) = stream.take() else { return };
        let Some((Ask { codecs, reply }, _)) = self.waiting.remove(&from) else {
            log::info!("{from}: refusing a mirror stream nobody asked for");
            return receiver.stop();
        };
        let codec = receiver.codec;
        if codec.is_some_and(|codec| !codecs.contains(&codec)) {
            receiver.stop();
            return drop(
                reply.send(Err(Failure::Failed(format!("the phone sent {codec:?} video, which was not asked for")))),
            );
        }
        let (ours, theirs) = match std::os::unix::net::UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) => return drop(reply.send(Err(Failure::Failed(error.to_string())))),
        };
        let (width, height) = (receiver.width, receiver.height);
        let ours = ours.set_nonblocking(true).and_then(|()| tokio::net::UnixStream::from_std(ours));
        let ours = match ours {
            Ok(ours) => ours,
            Err(error) => return drop(reply.send(Err(Failure::Failed(error.to_string())))),
        };
        if reply.send(Ok(Opened { stream: theirs.into(), width, height, codec })).is_err() {
            return receiver.stop();
        }
        if self.sound_only {
            log::info!("{from}: the phone's microphone is on");
        } else {
            log::info!("{from}: mirroring at {width}x{height} in {}", codec.map_or("H.264", codec_name));
        }
        let id = from.clone();
        let (stop, stopped) = oneshot::channel();
        self.pumps.spawn(async move {
            if let Err(error) = pump(&mut receiver, ours, stopped, lag).await {
                log::info!("{id}: mirroring ended: {error}");
            }
            receiver.stop();
            id
        });
        self.running.insert(from, stop);
        self.wifi_awake(true);
    }

    /// The next pump to end, with its phone, which is told to stop in case the viewer closed first.
    pub async fn ended(&mut self) -> Option<DeviceId> {
        let id = match self.pumps.join_next().await? {
            Ok(id) => id,
            Err(join) => {
                if !join.is_cancelled() {
                    log::warn!("mirror pump: {join}");
                }
                return None;
            }
        };
        self.running.remove(&id);
        self.wifi_awake(self.active());
        Some(id)
    }
}

/// Root helper that turns Wi-Fi power save off, or back to what it was; power save holds received packets for up to a
/// beacon interval (about 100 ms).
const WIFI_HELPER: &str = "/usr/lib/andromeda-link/wifi-power-save";

/// Keeps this computer's Wi-Fi out of power save while a phone mirrors. One task runs the helper, so calls apply in order.
#[derive(Default)]
struct WifiPower(Option<watch::Sender<bool>>);

impl WifiPower {
    fn awake(&mut self, awake: bool) {
        let sender = self.0.get_or_insert_with(|| {
            let (sender, mut receiver) = watch::channel(false);
            tokio::spawn(async move {
                let mut applied = false;
                while receiver.changed().await.is_ok() {
                    let awake = *receiver.borrow_and_update();
                    if awake != applied {
                        power_save(!awake).await;
                        applied = awake;
                    }
                }
                if applied {
                    power_save(true).await;
                }
            });
            sender
        });
        sender.send_replace(awake);
    }
}

async fn power_save(on: bool) {
    if !std::path::Path::new(WIFI_HELPER).exists() {
        return;
    }
    let mode = if on { "restore" } else { "off" };
    match tokio::process::Command::new("pkexec").args([WIFI_HELPER, mode]).status().await {
        Ok(status) if status.success() => log::info!("Wi-Fi power save: {mode}"),
        Ok(status) => log::info!("Wi-Fi power save {mode}: the helper {status}"),
        Err(error) => log::info!("Wi-Fi power save {mode}: pkexec: {error}"),
    }
}

/// Copies the phone's units to the viewer. When the phone ends the stream or stops mirroring, a last empty unit
/// flagged [`MIRROR_END`] follows; a viewer whose socket closes without it lost the stream and may open it again.
async fn pump(
    receiver: &mut MirrorReceiver,
    mut viewer: tokio::net::UnixStream,
    mut stopped: oneshot::Receiver<()>,
    session: Option<SessionHandle>,
) -> anyhow::Result<()> {
    let mut report = tokio::time::interval(LAG_REPORT);
    report.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut lag = Lag::new(Instant::now());
    loop {
        let unit = tokio::select! {
            unit = receiver.next() => unit?,
            _ = &mut stopped => None,
            _ = report.tick(), if session.is_some() => {
                if let (Some(session), Some(lag_ms)) = (&session, lag.report(Instant::now()))
                    && let Err(error) = session.send(Message::MirrorLag(MirrorLag { lag_ms })).await
                {
                    log::debug!("mirror lag report: {error}");
                }
                continue;
            }
        };
        let Some(mut unit) = unit else { break };
        if unit.flags & MIRROR_AUDIO == 0 {
            lag.arrived(unit.pts_us, Instant::now());
        }
        if unit.flags & MIRROR_CONFIG != 0 {
            crate::low_delay::rewrite(&mut unit.data);
        }
        viewer.write_all(&header(unit.data.len(), unit.pts_us, unit.flags)?).await?;
        viewer.write_all(&unit.data).await?;
    }
    viewer.write_all(&header(0, 0, MIRROR_END)?).await?;
    Ok(())
}

/// How late video units arrive: arrival minus capture time (the phone's clock), minus the least such value seen, which
/// cancels the clocks' offset. A window's least late unit is its lag, so one large keyframe does not count as loss.
struct Lag {
    start: Instant,
    floor_us: i64,
    window_best_us: Option<i64>,
    last_arrival: Option<Instant>,
}

impl Lag {
    fn new(start: Instant) -> Self {
        Self { start, floor_us: i64::MAX, window_best_us: None, last_arrival: None }
    }

    fn arrived(&mut self, pts_us: u64, now: Instant) {
        let now_us = i64::try_from(now.duration_since(self.start).as_micros()).unwrap_or(i64::MAX);
        let transit = now_us.saturating_sub(i64::try_from(pts_us).unwrap_or(i64::MAX));
        self.floor_us = self.floor_us.min(transit);
        let lag = transit - self.floor_us;
        self.window_best_us = Some(self.window_best_us.map_or(lag, |best| best.min(lag)));
        self.last_arrival = Some(now);
    }

    /// The lag in ms since the last report, or how long ago the last unit came when none came since; None before the
    /// first unit.
    fn report(&mut self, now: Instant) -> Option<u32> {
        let lag_us = match self.window_best_us.take() {
            Some(best) => u64::try_from(best).unwrap_or(0),
            None => u64::try_from(now.duration_since(self.last_arrival?).as_micros()).unwrap_or(u64::MAX),
        };
        Some(u32::try_from(lag_us / 1000).unwrap_or(u32::MAX).min(MAX_MIRROR_LAG_MS))
    }
}

fn header(len: usize, pts_us: u64, flags: u8) -> anyhow::Result<[u8; 13]> {
    let mut header = [0; 13];
    header[..4].copy_from_slice(&u32::try_from(len)?.to_be_bytes());
    header[4..12].copy_from_slice(&pts_us.to_be_bytes());
    header[12] = flags;
    Ok(header)
}

/// The name `OpenMirrorWith` takes and returns.
pub fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Hevc => "hevc",
        Codec::Av1 => "av1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn lag_is_the_least_late_unit_of_each_window_and_silence_counts() {
        let start = Instant::now();
        let mut lag = Lag::new(start);
        assert_eq!(lag.report(start), None);
        // 60 fps, 20 ms in transit; one 200 ms keyframe in the window does not raise its lag.
        for frame in 0..30_u64 {
            let late = if frame == 10 { 200 } else { 20 };
            lag.arrived(frame * 16_000, start + ms(frame * 16 + late));
        }
        assert_eq!(lag.report(start + ms(500)), Some(0));
        // A queue builds: every unit of the next window is 300 ms or more later than the first window's.
        for frame in 30..60_u64 {
            lag.arrived(frame * 16_000, start + ms(frame * 16 + 320 + (frame - 30) * 10));
        }
        assert_eq!(lag.report(start + ms(1500)), Some(300));
        // Nothing arrives: the report is how long ago the last unit came.
        let last = start + ms(59 * 16 + 320 + 29 * 10);
        assert_eq!(lag.report(last + ms(800)), Some(800));
    }
}
