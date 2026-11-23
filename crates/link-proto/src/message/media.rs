use super::*;

pub const MAX_PLAYER_LEN: usize = 64;
pub const MAX_METADATA_LEN: usize = 512;
pub const MAX_ARTWORK_LEN: usize = 48 * 1024;
pub const MAX_VOLUME: u8 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaCommandKind {
    Play,
    Pause,
    PlayPause,
    Next,
    Previous,
    /// `value` is the absolute position in ms.
    Seek,
    /// `value` is 0 to 100.
    Volume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaPlayer {
    pub player: String,
    pub name: String,
    pub state: PlaybackState,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_ms: Option<u64>,
    /// When sent; the receiver advances it while playing.
    pub position_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<u8>,
    /// PNG or JPEG.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "serde_bytes")]
    pub artwork: Option<Vec<u8>>,
    pub can: Vec<MediaCommandKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaGone {
    pub player: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaCommand {
    pub player: String,
    pub command: MediaCommandKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<u64>,
}

pub(super) fn player_id(player: &str) -> bool {
    sized(player, 1, MAX_PLAYER_LEN)
}

impl MediaPlayer {
    pub(super) fn valid(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        player_id(&self.player)
            && sized(&self.name, 1, MAX_PLAYER_LEN)
            && [&self.title, &self.artist, &self.album].iter().all(|text| sized(text, 0, MAX_METADATA_LEN))
            && self.volume.is_none_or(|volume| volume <= MAX_VOLUME)
            && self.artwork.as_ref().is_none_or(|art| (1..=MAX_ARTWORK_LEN).contains(&art.len()))
            && self.can.iter().all(|command| seen.insert(*command))
    }
}

impl MediaCommand {
    pub(super) fn valid(&self) -> bool {
        let value_ok = match self.command {
            MediaCommandKind::Seek => self.value.is_some(),
            MediaCommandKind::Volume => self.value.is_some_and(|volume| volume <= u64::from(MAX_VOLUME)),
            _ => self.value.is_none(),
        };
        player_id(&self.player) && value_ok
    }
}
