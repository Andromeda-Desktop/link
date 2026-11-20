//! A session's liveness, one per session on both sides. The phone sends heartbeats; each promises the next one, and a
//! side treats the session as live only until that promise plus [`GRACE`] has passed without word from the peer.
//! Everything that shows "connected" reads [`Liveness::view`], which applies the deadline at read time.

use std::time::Duration;

use tokio::time::Instant;

/// How far past a promised heartbeat the session still counts as live.
pub const GRACE: Duration = Duration::from_secs(2);
/// The phone's heartbeat interval while either side is active; with [`GRACE`] it bounds a stale "connected" to 3 s.
pub const ACTIVE_EVERY: Duration = Duration::from_secs(1);
/// The phone's heartbeat interval while idle: no more wakeups than the keep-alive it replaces.
pub const IDLE_EVERY: Duration = Duration::from_secs(10);
/// How long any traffic keeps the phone on [`ACTIVE_EVERY`].
pub const ACTIVE_FOR: Duration = Duration::from_secs(30);

/// What a reader sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Heard from the peer within its promise. `rtt` is the last heartbeat round trip, if one was measured.
    Live {
        rtt: Option<Duration>,
    },
    /// The peer's promise ran out at `since`; the transport may still recover.
    Degraded {
        since: Instant,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    /// A heartbeat from the peer, promising the next within `next`.
    Heard { next: Duration },
    /// The peer answered our heartbeat sent at `sent`; our next goes out within `next`.
    Acked { sent: Instant, next: Duration },
    /// The session ended.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

/// The state of one session. `Copy`, so a handle can hold a snapshot and judge it at read time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// `deadline: None` is a peer without heartbeats: live while the transport is open.
    Live {
        rtt: Option<Duration>,
        deadline: Option<Instant>,
    },
    Closed,
}

impl Liveness {
    /// A session whose hellos just crossed, so it starts live. `heartbeat` is false for a peer without the feature.
    #[must_use]
    pub fn new(now: Instant, heartbeat: bool) -> Self {
        let deadline = heartbeat.then(|| now + IDLE_EVERY + GRACE);
        Self { state: State::Live { rtt: None, deadline } }
    }

    /// The one transition function. A closed session takes no event; a legacy session takes only `Close`.
    pub fn apply(&mut self, event: LinkEvent, now: Instant) -> Result<(), Invalid> {
        self.state = match (self.state, event) {
            (State::Closed, _)
            | (State::Live { deadline: None, .. }, LinkEvent::Heard { .. } | LinkEvent::Acked { .. }) => {
                return Err(Invalid);
            }
            (State::Live { rtt, .. }, LinkEvent::Heard { next }) => {
                State::Live { rtt, deadline: Some(now + next + GRACE) }
            }
            (State::Live { .. }, LinkEvent::Acked { sent, next }) => {
                State::Live { rtt: Some(now.saturating_duration_since(sent)), deadline: Some(now + next + GRACE) }
            }
            (State::Live { .. }, LinkEvent::Close) => State::Closed,
        };
        Ok(())
    }

    #[must_use]
    pub fn view(&self, now: Instant) -> LinkState {
        match self.state {
            State::Live { deadline: Some(deadline), .. } if now >= deadline => LinkState::Degraded { since: deadline },
            State::Live { rtt, .. } => LinkState::Live { rtt },
            State::Closed => LinkState::Closed,
        }
    }

    /// When the view turns from live to degraded unless the peer is heard first.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Live { deadline, .. } => deadline,
            State::Closed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Random event sequences on a fake clock: the view is live only within the last promise plus the grace, a
    /// rejected event changes nothing, closed is final, and a legacy session never degrades.
    #[test]
    fn random_sequences_keep_the_invariants() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move |bound: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % bound
        };
        for _ in 0..2000 {
            let start = Instant::now();
            let heartbeat = next(4) != 0;
            let mut link = Liveness::new(start, heartbeat);
            let (mut now, mut promise, mut closed) = (start, Some(start + IDLE_EVERY + GRACE), false);
            for _ in 0..40 {
                now += Duration::from_millis(next(6000));
                let wait = Duration::from_millis([1000, 10_000][usize::from(next(2) == 0)]);
                let event = match next(10) {
                    0 => LinkEvent::Close,
                    1..=4 => LinkEvent::Heard { next: wait },
                    _ => LinkEvent::Acked {
                        sent: now.checked_sub(Duration::from_millis(next(500))).unwrap_or(now),
                        next: wait,
                    },
                };
                let before = link;
                if link.apply(event, now).is_ok() {
                    assert!(!closed, "a closed session took {event:?}");
                    match event {
                        LinkEvent::Close => closed = true,
                        LinkEvent::Heard { next } | LinkEvent::Acked { next, .. } => promise = Some(now + next + GRACE),
                    }
                } else {
                    assert_eq!(link, before, "a rejected {event:?} changed the state");
                    assert!(closed || !heartbeat, "{event:?} rejected on a live heartbeat session");
                }
                let at = now + Duration::from_millis(next(15_000));
                match link.view(at) {
                    LinkState::Live { .. } => {
                        assert!(!closed);
                        assert!(!heartbeat || promise.is_some_and(|promise| at < promise), "live past the promise");
                    }
                    LinkState::Degraded { since } => {
                        assert!(heartbeat && !closed);
                        assert_eq!(Some(since), promise);
                        assert!(at >= since);
                    }
                    LinkState::Closed => assert!(closed),
                }
            }
        }
    }
}
