//! The phone's one nearby network for bulk, a Wi-Fi Direct group or its hotspot: which desktop it serves and what it
//! waits for. Pure: the client applies the effects.

use std::time::Duration;

use tokio::time::Instant;

use crate::hotspot::{UPGRADE_TIMEOUT, WIFI_DIRECT_TIMEOUT};
use crate::identity::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Group,
    Hotspot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Slot {
    Idle,
    /// The network starts, or waits for the desktop's session, until `deadline`. `fallback`: a failed group gives way
    /// to the hotspot.
    Forming {
        peer: DeviceId,
        kind: Kind,
        deadline: Instant,
        fallback: bool,
    },
    /// The desktop's session runs over the network; it stops at `idle_at`, never while something runs (None).
    Up {
        peer: DeviceId,
        kind: Kind,
        idle_at: Option<Instant>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `peer`'s Bluetooth session needs bulk: a group when `group`, else the hotspot when `hotspot`.
    Request {
        peer: DeviceId,
        group: bool,
        hotspot: bool,
    },
    /// `kind`'s provider failed, or the desktop could not join it.
    Failed {
        peer: DeviceId,
        kind: Kind,
        reason: String,
    },
    /// `peer`'s session arrived over the network.
    Adopted {
        peer: DeviceId,
        busy: bool,
    },
    /// A transfer or mirror with `peer` started or ended.
    Busy {
        peer: DeviceId,
        busy: bool,
    },
    /// Something ran through the network: an armed idle timer starts again.
    Used {
        peer: DeviceId,
    },
    /// The phone's idle time changed with its screen: an armed idle timer starts again with the new time.
    IdleChanged,
    /// The desktop ended the network.
    DesktopEnded {
        peer: DeviceId,
        reason: String,
    },
    /// `peer`'s session ended.
    SessionEnded {
        peer: DeviceId,
    },
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Start(Kind),
    /// Stop the provider and close a session running over it.
    Stop(Kind),
    /// Send `hotspot-end` to the desktop.
    Tell(Option<String>),
    /// Answer the sends waiting for the network with this failure.
    Fail(String),
}

/// The event does not apply to the slot now, which is normal for a late result; nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stale;

const BUSY_ELSEWHERE: &str = "the hotspot serves another desktop";
const NO_HOTSPOT: &str = "this phone cannot start a hotspot";
const NOT_IN_TIME: &str = "the network did not carry the session in time";

impl Event {
    pub fn peer(&self) -> Option<&DeviceId> {
        match self {
            Self::Request { peer, .. }
            | Self::Failed { peer, .. }
            | Self::Adopted { peer, .. }
            | Self::Busy { peer, .. }
            | Self::Used { peer }
            | Self::DesktopEnded { peer, .. }
            | Self::SessionEnded { peer } => Some(peer),
            Self::IdleChanged | Self::Tick => None,
        }
    }
}

impl Slot {
    pub fn peer(&self) -> Option<&DeviceId> {
        match self {
            Self::Idle => None,
            Self::Forming { peer, .. } | Self::Up { peer, .. } => Some(peer),
        }
    }

    pub fn serves(&self, id: &DeviceId) -> bool {
        self.peer() == Some(id)
    }

    pub fn kind(&self) -> Option<Kind> {
        match self {
            Self::Idle => None,
            Self::Forming { kind, .. } | Self::Up { kind, .. } => Some(*kind),
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Idle => None,
            Self::Forming { deadline, .. } => Some(*deadline),
            Self::Up { idle_at, .. } => *idle_at,
        }
    }

    /// The only place the slot changes. `idle` is how long an unused network stays up now.
    pub fn apply(&mut self, event: Event, now: Instant, idle: Duration) -> Result<Vec<Effect>, Stale> {
        let (next, effects) = match (&*self, event) {
            (Self::Idle, Event::Request { peer, group, hotspot }) => match (group, hotspot) {
                (true, fallback) => (forming(peer, Kind::Group, fallback, now), vec![Effect::Start(Kind::Group)]),
                (false, true) => (forming(peer, Kind::Hotspot, false, now), vec![Effect::Start(Kind::Hotspot)]),
                (false, false) => (Self::Idle, refuse(NO_HOTSPOT)),
            },
            (slot, Event::Request { peer, .. }) if slot.serves(&peer) => return Ok(Vec::new()),
            (slot, Event::Request { .. }) => (slot.clone(), refuse(BUSY_ELSEWHERE)),
            (Self::Forming { peer, kind, fallback, .. }, Event::Failed { peer: from, kind: failed, reason })
                if *peer == from && *kind == failed =>
            {
                fail(peer, *kind, *fallback, reason, now)
            }
            (Self::Forming { peer, kind, fallback, .. }, Event::DesktopEnded { peer: from, reason })
                if *peer == from && *kind == Kind::Group =>
            {
                fail(peer, *kind, *fallback, reason, now)
            }
            (Self::Forming { peer, kind, .. }, Event::Adopted { peer: from, busy }) if *peer == from => {
                (Self::Up { peer: from, kind: *kind, idle_at: idle_at(busy, now, idle) }, Vec::new())
            }
            (Self::Up { peer, kind, .. }, Event::Busy { peer: from, busy }) if *peer == from => {
                (Self::Up { peer: from, kind: *kind, idle_at: idle_at(busy, now, idle) }, Vec::new())
            }
            (Self::Up { peer, kind, idle_at: Some(_) }, Event::Used { peer: from }) if *peer == from => {
                (Self::Up { peer: from, kind: *kind, idle_at: idle_at(false, now, idle) }, Vec::new())
            }
            (Self::Up { peer, kind, idle_at: Some(_) }, Event::IdleChanged) => {
                (Self::Up { peer: peer.clone(), kind: *kind, idle_at: idle_at(false, now, idle) }, Vec::new())
            }
            (slot, Event::DesktopEnded { peer, reason }) if slot.serves(&peer) => {
                (Self::Idle, vec![Effect::Stop(kind_of(slot)), Effect::Fail(reason)])
            }
            (slot, Event::SessionEnded { peer }) if slot.serves(&peer) => {
                (Self::Idle, vec![Effect::Stop(kind_of(slot)), Effect::Fail(NOT_IN_TIME.to_owned())])
            }
            (Self::Forming { peer, kind, deadline, fallback }, Event::Tick) if *deadline <= now => {
                fail(peer, *kind, *fallback, NOT_IN_TIME.to_owned(), now)
            }
            (Self::Up { kind, idle_at: Some(at), .. }, Event::Tick) if *at <= now => {
                (Self::Idle, vec![Effect::Tell(None), Effect::Stop(*kind)])
            }
            _ => return Err(Stale),
        };
        if next != *self {
            log::info!("hotspot slot: {self:?} -> {next:?}");
        }
        *self = next;
        Ok(effects)
    }
}

fn forming(peer: DeviceId, kind: Kind, fallback: bool, now: Instant) -> Slot {
    let wait = if kind == Kind::Group { WIFI_DIRECT_TIMEOUT } else { UPGRADE_TIMEOUT };
    Slot::Forming { peer, kind, deadline: now + wait, fallback }
}

/// A group with a fallback gives way to the hotspot; anything else ends and fails the waiting sends.
fn fail(peer: &DeviceId, kind: Kind, fallback: bool, reason: String, now: Instant) -> (Slot, Vec<Effect>) {
    if kind == Kind::Group && fallback {
        let next = forming(peer.clone(), Kind::Hotspot, false, now);
        return (next, vec![Effect::Stop(Kind::Group), Effect::Start(Kind::Hotspot)]);
    }
    (Slot::Idle, vec![Effect::Stop(kind), Effect::Tell(Some(reason.clone())), Effect::Fail(reason)])
}

fn refuse(reason: &str) -> Vec<Effect> {
    vec![Effect::Tell(Some(reason.to_owned())), Effect::Fail(reason.to_owned())]
}

fn idle_at(busy: bool, now: Instant, idle: Duration) -> Option<Instant> {
    (!busy).then(|| now + idle)
}

fn kind_of(slot: &Slot) -> Kind {
    slot.kind().unwrap_or(Kind::Hotspot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotspot::{IDLE_SCREEN_OFF, IDLE_SCREEN_ON};

    /// Random event sequences on a fake clock, with the idle time switching between screen on and off. Invariants: a
    /// provider runs exactly while the slot holds its kind; a tick leaves no deadline in the past; an idle timer never
    /// runs past the current idle time; a stale event changes nothing; every failure that ends the slot answers the
    /// waiting sends.
    #[test]
    fn random_sequences_keep_the_invariants() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        let peers = [DeviceId::from_fingerprint(&[1; 32]), DeviceId::from_fingerprint(&[2; 32])];
        let kinds = [Kind::Group, Kind::Hotspot];
        for _ in 0..3000 {
            let mut now = Instant::now();
            let mut slot = Slot::Idle;
            let mut running: Vec<Kind> = Vec::new();
            let mut idle = IDLE_SCREEN_OFF;
            for _ in 0..60 {
                now += Duration::from_millis(next(30_000));
                let peer = peers[usize::from(next(2) == 0)].clone();
                let kind = kinds[usize::from(next(2) == 0)];
                let event = match next(10) {
                    0 => Event::Request { peer, group: next(2) == 0, hotspot: next(2) == 0 },
                    1 => Event::Failed { peer, kind, reason: "x".into() },
                    2 => Event::Adopted { peer, busy: next(2) == 0 },
                    3 => Event::Busy { peer, busy: next(2) == 0 },
                    4 => Event::Used { peer },
                    5 => Event::DesktopEnded { peer, reason: "x".into() },
                    6 => Event::SessionEnded { peer },
                    7 => {
                        idle = if idle == IDLE_SCREEN_OFF { IDLE_SCREEN_ON } else { IDLE_SCREEN_OFF };
                        Event::IdleChanged
                    }
                    _ => Event::Tick,
                };
                let before = slot.clone();
                let is_tick = event == Event::Tick;
                match slot.apply(event.clone(), now, idle) {
                    Err(Stale) => assert_eq!(slot, before, "a stale {event:?} changed the slot"),
                    Ok(effects) => {
                        for effect in &effects {
                            match effect {
                                Effect::Start(kind) => {
                                    assert!(!running.contains(kind), "{kind:?} started twice: {before:?} {event:?}");
                                    running.push(*kind);
                                }
                                Effect::Stop(kind) => {
                                    assert!(running.contains(kind), "{kind:?} stopped while not running");
                                    running.retain(|running| running != kind);
                                }
                                Effect::Tell(_) | Effect::Fail(_) => {}
                            }
                        }
                        let ended = slot == Slot::Idle && before != Slot::Idle;
                        let failed = effects.iter().any(|effect| matches!(effect, Effect::Fail(_)));
                        let idled = matches!(before, Slot::Up { .. }) && is_tick;
                        assert!(!ended || failed || idled, "{before:?} ended on {event:?} without answering the sends");
                    }
                }
                assert_eq!(running, slot.kind().into_iter().collect::<Vec<_>>(), "providers vs {slot:?}");
                if let Slot::Up { idle_at: Some(at), .. } = &slot {
                    assert!(*at <= now + idle, "{slot:?} idles later than {idle:?} after {event:?}");
                }
                if is_tick {
                    assert!(slot.deadline().is_none_or(|deadline| deadline > now), "{slot:?} is past due at a tick");
                }
            }
        }
    }
}
