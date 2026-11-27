//! The PC's own player for phones on Windows: the current media session (GSMTC) as one player, read again on each
//! session event, never polled. Commands go to that session.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use link_core::proto::message::{
    MAX_ARTWORK_LEN, MAX_METADATA_LEN, MAX_PLAYER_LEN, MediaCommand, MediaCommandKind, MediaPlayer, PlaybackState,
};
use tokio::sync::{Notify, mpsc};
use windows::ApplicationModel::AppInfo;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::core::{HSTRING, RuntimeType};

use crate::desktop_media::{Request, is_image, truncate};
use crate::hub::HubHandle;

const PLAYER: &str = "windows";
/// Ticks (100 ns, the `TimeSpan` unit) per millisecond.
const TICKS_PER_MS: i64 = 10_000;
/// 1601-01-01 to 1970-01-01 in ticks: `DateTime` counts from 1601.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
/// A position this close to the one expected from the last report is no news.
const POSITION_SLACK_MS: u64 = 1000;

/// The session watched now; its handlers end with it.
struct Watched {
    session: Session,
    tokens: [i64; 3],
    app: String,
    track: Option<[String; 3]>,
    artwork: Artwork,
}

/// The current track's thumbnail.
enum Artwork {
    /// None set yet; apps often set it after the title, so it is read again.
    Missing,
    /// Not a PNG or JPEG, or too large to send.
    Unusable,
    Image(Vec<u8>),
}

/// The player phones have and when it was sent, with its playback rate, to skip reports that only move the position
/// as expected.
struct Sent {
    player: MediaPlayer,
    at: Instant,
    rate: f64,
}

pub async fn now_playing(hub: HubHandle, mut requests: mpsc::Receiver<Request>) -> anyhow::Result<()> {
    let (session_changed, state_changed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let manager = async {
        let manager = Manager::RequestAsync()?.await?;
        manager.CurrentSessionChanged(&handler(&session_changed))?;
        Ok::<_, windows::core::Error>(manager)
    };
    let manager = match manager.await {
        Ok(manager) => manager,
        Err(error) => {
            log::warn!("the media sessions are not readable: {error}; no PC player for phones");
            while requests.recv().await.is_some() {}
            return Ok(());
        }
    };
    session_changed.notify_one();
    let mut watched: Option<Watched> = None;
    let mut sent: Option<Sent> = None;
    let mut paused_for_call: Vec<Session> = Vec::new();
    loop {
        tokio::select! {
            () = session_changed.notified() => {
                if let Some(old) = watched.take() {
                    old.unsubscribe();
                }
                watched = manager.GetCurrentSession().ok().and_then(|session| Watched::new(session, &state_changed));
                report(&hub, &mut watched, &mut sent).await;
            }
            () = state_changed.notified() => report(&hub, &mut watched, &mut sent).await,
            request = requests.recv() => match request {
                None => return Ok(()),
                Some(Request::Command(command)) => {
                    if let Some(watched) = &watched {
                        run(&watched.session, &command).await;
                    }
                }
                Some(Request::PauseAll) => paused_for_call.extend(pause_playing(&manager).await),
                Some(Request::ResumePaused) => {
                    for session in paused_for_call.drain(..) {
                        if let Err(error) = async { session.TryPlayAsync()?.await }.await {
                            log::debug!("resuming a player after a call: {error}");
                        }
                    }
                }
            },
        }
    }
}

fn handler<S: RuntimeType + 'static, A: RuntimeType + 'static>(notify: &Arc<Notify>) -> TypedEventHandler<S, A> {
    let notify = Arc::clone(notify);
    TypedEventHandler::new(move |_, _| {
        notify.notify_one();
        Ok(())
    })
}

impl Watched {
    fn new(session: Session, changed: &Arc<Notify>) -> Option<Self> {
        let subscribed = (|| {
            Ok::<_, windows::core::Error>([
                session.MediaPropertiesChanged(&handler(changed))?,
                session.PlaybackInfoChanged(&handler(changed))?,
                session.TimelinePropertiesChanged(&handler(changed))?,
            ])
        })();
        let tokens = subscribed.inspect_err(|error| log::debug!("watching a media session: {error}")).ok()?;
        let id = session.SourceAppUserModelId().map(|id| id.to_string_lossy()).unwrap_or_default();
        Some(Self { app: app_name(&id), session, tokens, track: None, artwork: Artwork::Missing })
    }

    fn unsubscribe(self) {
        let [properties, info, timeline] = self.tokens;
        let removed = (|| {
            self.session.RemoveMediaPropertiesChanged(properties)?;
            self.session.RemovePlaybackInfoChanged(info)?;
            self.session.RemoveTimelinePropertiesChanged(timeline)
        })();
        if let Err(error) = removed {
            log::debug!("leaving a media session: {error}");
        }
    }

    /// The session as the phone's player and its playback rate; None while it has no title.
    async fn read(&mut self) -> windows::core::Result<Option<(MediaPlayer, f64)>> {
        let properties = self.session.TryGetMediaPropertiesAsync()?.await?;
        let text = |value: windows::core::Result<HSTRING>| {
            truncate(&value.map(|text| text.to_string_lossy()).unwrap_or_default(), MAX_METADATA_LEN)
        };
        let track = [text(properties.Title()), text(properties.Artist()), text(properties.AlbumTitle())];
        if track[0].is_empty() {
            return Ok(None);
        }
        if self.track.as_ref() != Some(&track) || matches!(self.artwork, Artwork::Missing) {
            self.artwork = artwork(&properties).await;
            self.track = Some(track.clone());
        }
        let info = self.session.GetPlaybackInfo()?;
        let timeline = self.session.GetTimelineProperties()?;
        let playing = info.PlaybackStatus()? == Status::Playing;
        let rate = info.PlaybackRate().and_then(|rate| rate.Value()).unwrap_or(1.0);
        let controls = info.Controls()?;
        let can = [
            (controls.IsPlayEnabled(), MediaCommandKind::Play),
            (controls.IsPauseEnabled(), MediaCommandKind::Pause),
            (controls.IsPlayPauseToggleEnabled(), MediaCommandKind::PlayPause),
            (controls.IsNextEnabled(), MediaCommandKind::Next),
            (controls.IsPreviousEnabled(), MediaCommandKind::Previous),
            (controls.IsPlaybackPositionEnabled(), MediaCommandKind::Seek),
        ]
        .into_iter()
        .filter_map(|(enabled, command)| enabled.unwrap_or(false).then_some(command))
        .collect();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| {
            i64::try_from(since.as_nanos() / 100).unwrap_or(i64::MAX).saturating_add(UNIX_EPOCH_TICKS)
        });
        let position = position_ms(
            timeline.Position()?.Duration,
            timeline.LastUpdatedTime()?.UniversalTime,
            now,
            playing.then_some(rate),
            timeline.EndTime()?.Duration,
        );
        let [title, artist, album] = track;
        let player = MediaPlayer {
            player: PLAYER.to_owned(),
            name: self.app.clone(),
            state: if playing { PlaybackState::Playing } else { PlaybackState::Paused },
            title,
            artist,
            album,
            length_ms: ms(timeline.EndTime()?.Duration).filter(|ms| *ms > 0),
            position_ms: position,
            volume: None,
            artwork: match &self.artwork {
                Artwork::Image(bytes) => Some(bytes.clone()),
                Artwork::Missing | Artwork::Unusable => None,
            },
            can,
        };
        Ok(Some((player, rate)))
    }
}

/// Reports the watched session to phones when it changed beyond the expected advance of its position.
async fn report(hub: &HubHandle, watched: &mut Option<Watched>, sent: &mut Option<Sent>) {
    let read = match watched {
        Some(watched) => {
            watched.read().await.inspect_err(|error| log::debug!("reading a media session: {error}")).ok().flatten()
        }
        None => None,
    };
    match read {
        Some((player, rate)) => {
            if sent.as_ref().is_some_and(|sent| sent.covers(&player)) {
                return;
            }
            hub.desktop_player(player.clone()).await;
            *sent = Some(Sent { player, at: Instant::now(), rate });
        }
        None => {
            if sent.take().is_some() {
                hub.desktop_player_gone(PLAYER.to_owned()).await;
            }
        }
    }
}

impl Sent {
    /// Whether phones already have `player`: all but the position match, and the position is where the sent one
    /// has moved since.
    fn covers(&self, player: &MediaPlayer) -> bool {
        let rate = if self.player.state == PlaybackState::Playing { self.rate } else { 0.0 };
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a rounded, non-negative ms count")]
        let moved = (self.at.elapsed().as_secs_f64() * 1000.0 * rate).max(0.0).round() as u64;
        let expected = self.player.position_ms.saturating_add(moved);
        let expected = self.player.length_ms.map_or(expected, |length| expected.min(length));
        player.position_ms.abs_diff(expected) < POSITION_SLACK_MS
            && MediaPlayer { position_ms: self.player.position_ms, ..player.clone() } == self.player
    }
}

/// The position now in ms: `position` (ticks) at `updated` (ticks), advanced at `rate` while playing, at most `end`.
fn position_ms(position: i64, updated: i64, now: i64, rate: Option<f64>, end: i64) -> u64 {
    #[expect(clippy::cast_possible_truncation, clippy::cast_precision_loss, reason = "a rounded tick count")]
    let moved = rate.map_or(0, |rate| (now.saturating_sub(updated).max(0) as f64 * rate).round() as i64);
    let position = position.saturating_add(moved);
    ms(if end > 0 { position.min(end) } else { position }).unwrap_or(0)
}

fn ms(ticks: i64) -> Option<u64> {
    u64::try_from(ticks / TICKS_PER_MS).ok()
}

/// The thumbnail, sent as is when it is a PNG or JPEG of at most `MAX_ARTWORK_LEN`.
async fn artwork(properties: &Properties) -> Artwork {
    let Ok(thumbnail) = properties.Thumbnail() else { return Artwork::Missing };
    let image = async {
        let stream = thumbnail.OpenReadAsync().ok()?.await.ok()?;
        let size = stream.Size().ok().filter(|size| *size > 0)?;
        let size =
            u32::try_from(size).ok().filter(|size| usize::try_from(*size).is_ok_and(|n| n <= MAX_ARTWORK_LEN))?;
        let reader = DataReader::CreateDataReader(&stream).ok()?;
        let loaded = reader.LoadAsync(size).ok()?.await.ok()?;
        let mut bytes = vec![0; usize::try_from(loaded).ok()?];
        reader.ReadBytes(&mut bytes).ok()?;
        is_image(&bytes).then_some(bytes)
    };
    image.await.map_or(Artwork::Unusable, Artwork::Image)
}

/// The app's display name for a packaged app, else its program name without `.exe`.
fn app_name(id: &str) -> String {
    let name = AppInfo::GetFromAppUserModelId(&HSTRING::from(id))
        .and_then(|app| app.DisplayInfo()?.DisplayName())
        .map(|name| name.to_string_lossy())
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            let name = id.rsplit('\\').next().unwrap_or(id);
            name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name).to_owned()
        });
    let name = truncate(&name, MAX_PLAYER_LEN);
    if name.is_empty() { "Windows".to_owned() } else { name }
}

async fn run(session: &Session, command: &MediaCommand) {
    let value = i64::try_from(command.value.unwrap_or(0)).unwrap_or(i64::MAX);
    let result = async {
        let operation = match command.command {
            MediaCommandKind::Play => session.TryPlayAsync()?,
            MediaCommandKind::Pause => session.TryPauseAsync()?,
            MediaCommandKind::PlayPause => session.TryTogglePlayPauseAsync()?,
            MediaCommandKind::Next => session.TrySkipNextAsync()?,
            MediaCommandKind::Previous => session.TrySkipPreviousAsync()?,
            MediaCommandKind::Seek => session.TryChangePlaybackPositionAsync(value.saturating_mul(TICKS_PER_MS))?,
            MediaCommandKind::Volume => return Ok(true),
        };
        operation.await
    };
    match result.await {
        Ok(true) => {}
        Ok(false) => log::debug!("the player refused {:?}", command.command),
        Err(error) => log::debug!("{:?} on the player: {error}", command.command),
    }
}

/// Pauses every playing session for a call and returns those it paused.
async fn pause_playing(manager: &Manager) -> Vec<Session> {
    let Ok(sessions) = manager.GetSessions() else { return Vec::new() };
    let mut paused = Vec::new();
    for session in sessions {
        let playing = session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .is_ok_and(|status| status == Status::Playing);
        if playing && async { session.TryPauseAsync()?.await }.await.is_ok_and(|paused| paused) {
            paused.push(session);
        }
    }
    paused
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1000 * TICKS_PER_MS;

    #[test]
    fn the_position_moves_only_while_playing_and_stops_at_the_end() {
        assert_eq!(position_ms(10 * SECOND, 0, 2 * SECOND, Some(1.0), 0), 12_000);
        assert_eq!(position_ms(10 * SECOND, 0, 2 * SECOND, Some(2.0), 60 * SECOND), 14_000);
        assert_eq!(position_ms(10 * SECOND, 0, 2 * SECOND, None, 60 * SECOND), 10_000);
        assert_eq!(position_ms(59 * SECOND, 0, 5 * SECOND, Some(1.0), 60 * SECOND), 60_000);
    }

    #[test]
    #[expect(clippy::expect_used, reason = "a failed step fails the test")]
    fn a_report_that_only_moves_the_position_as_expected_is_skipped() {
        let player = MediaPlayer {
            player: PLAYER.to_owned(),
            name: "Player".to_owned(),
            state: PlaybackState::Playing,
            title: "Song".to_owned(),
            artist: String::new(),
            album: String::new(),
            length_ms: Some(300_000),
            position_ms: 10_000,
            volume: None,
            artwork: None,
            can: vec![MediaCommandKind::PlayPause],
        };
        let at = Instant::now().checked_sub(std::time::Duration::from_secs(5)).expect("an instant 5 s ago");
        let sent = Sent { player: player.clone(), at, rate: 1.0 };
        assert!(sent.covers(&MediaPlayer { position_ms: 15_000, ..player.clone() }));
        assert!(!sent.covers(&MediaPlayer { position_ms: 10_000, ..player.clone() }), "a seek back is news");
        assert!(!sent.covers(&MediaPlayer { position_ms: 15_000, state: PlaybackState::Paused, ..player.clone() }));
        assert!(!sent.covers(&MediaPlayer { position_ms: 15_000, title: "Next".to_owned(), ..player }));
    }
}
