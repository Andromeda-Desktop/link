use super::*;

/// A video codec besides H.264, which every peer decodes and which a message without a codec means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    Hevc,
    Av1,
}

/// Desktop to phone: mirror the screen. `codecs` lists what the desktop's viewer decodes besides H.264, best first;
/// sent only to phones whose hello has [`MIRROR_CODECS`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codecs: Vec<Codec>,
}

/// Desktop to phone about every 500 ms while it mirrors, only to phones whose hello has [`MIRROR_LAG`]: how late the
/// least late video unit of the last half second arrived, past the least late unit of the mirror; or, when no unit
/// arrived, how long ago the last one did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorLag {
    pub lag_ms: u32,
}

/// Phone to desktop, once the user allowed capture: the video that follows on a `mirror-data` stream is this size,
/// in `codec`, one of the request's `codecs`, or H.264 when absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorStarted {
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<Codec>,
}

/// Phone to desktop while mirroring, after `mirror-started` and after each rotation: the screen's rounded corners and
/// camera cutouts as they sit in the picture. Lengths are in 1/10000 of the picture's width (x, radii) or height (y).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenShape {
    /// Top-left, top-right, bottom-right, bottom-left.
    pub corners: [u16; 4],
    pub cutouts: Vec<ShapeRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapeRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub const MAX_CUTOUTS: usize = 4;

impl ScreenShape {
    pub(super) fn valid(&self) -> bool {
        let unit = 1..=10_000;
        self.corners.iter().all(|corner| *corner <= 5_000)
            && self.cutouts.len() <= MAX_CUTOUTS
            && self.cutouts.iter().all(|rect| {
                unit.contains(&rect.width)
                    && unit.contains(&rect.height)
                    && u32::from(rect.x) + u32::from(rect.width) <= 10_000
                    && u32::from(rect.y) + u32::from(rect.height) <= 10_000
            })
    }
}

/// Either side ends mirroring, with a reason when it failed or was refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorStop {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MirrorAction {
    Tap,
    LongPress,
    /// From (x, y) to (x2, y2) over `ms`.
    Swipe,
    /// A wheel turn at (x, y): (x2, y2) is the direction and length, like a swipe that starts where the pointer is.
    Scroll,
    Back,
    Home,
    Recents,
    /// `text` into the focused field.
    Text,
    /// One finger at (x, y) that follows the pointer until `touch-up`.
    TouchDown,
    TouchMove,
    TouchUp,
    /// A wheel turn at (x, y): (x2, y2) in 1/[`WHEEL_STEP`] of a step, positive right and down.
    Wheel,
    /// An Android `KeyEvent` key `code` with its `meta` state.
    KeyDown,
    KeyUp,
    /// The phone's own panel off or on; mirroring goes on.
    ScreenOff,
    ScreenOn,
    /// The phone takes a screenshot as its own screenshot key does.
    Screenshot,
}

/// Desktop to phone: input on the mirrored screen. Coordinates are fractions of the video's width and height in
/// units of 1/[`MIRROR_UNIT`]; a scroll's (x2, y2) is a signed delta in the same units.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorInput {
    pub action: MirrorAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x2: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y2: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<u32>,
}

/// The header of the phone's video stream: access units in `codec` (H.264 when absent) follow, each a big-endian u32 length, a big-endian u64
/// presentation time in microseconds, a flags byte ([`MIRROR_KEYFRAME`], [`MIRROR_CONFIG`], [`MIRROR_AUDIO`]), then
/// the Annex B bytes, or an Opus packet for audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorData {
    pub width: u32,
    pub height: u32,
    /// Absent on the mirror's first stream, which carries everything, or only the sound under [`MIRROR_STREAMS`].
    /// From 1 up on each later stream: one keyframe and the video after it, until a stream with a higher number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gop: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<Codec>,
}

/// The whole width or height in [`MirrorInput`] coordinates.
pub const MIRROR_UNIT: i32 = 10_000;
pub const MIRROR_KEYFRAME: u8 = 1;
/// Codec configuration (SPS and PPS), sent before the first frame and again after a size change.
pub const MIRROR_CONFIG: u8 = 2;
/// One Opus packet of the phone's sound, 48 kHz stereo, instead of video.
pub const MIRROR_AUDIO: u8 = 4;
/// The desktop daemon's last, empty unit to a viewer when the phone ended mirroring on purpose; never on the wire.
pub const MIRROR_END: u8 = 8;
/// A 1080p access unit stays well under this.
pub const MAX_MIRROR_UNIT: usize = 4 * 1024 * 1024;
pub const MAX_MIRROR_SIDE: u32 = 4096;
pub const MAX_MIRROR_TEXT_LEN: usize = 4096;
/// A swipe or long press held longer than this is refused.
pub const MAX_GESTURE_MS: u32 = 10_000;
/// One wheel step in a `wheel` input's (x2, y2).
pub const WHEEL_STEP: i32 = 100;
/// Android's `KeyEvent` codes stay under this.
pub const MAX_KEY_CODE: u32 = 400;

impl MirrorInput {
    pub(super) fn valid(&self) -> bool {
        let fraction = |value: Option<i32>| value.is_some_and(|value| (0..=MIRROR_UNIT).contains(&value));
        let point = fraction(self.x) && fraction(self.y);
        let delta = |value: Option<i32>| value.is_some_and(|value| (-MIRROR_UNIT..=MIRROR_UNIT).contains(&value));
        let held = self.ms.is_none_or(|ms| ms <= MAX_GESTURE_MS);
        held && match self.action {
            MirrorAction::Tap
            | MirrorAction::LongPress
            | MirrorAction::TouchDown
            | MirrorAction::TouchMove
            | MirrorAction::TouchUp => point,
            MirrorAction::Swipe => point && fraction(self.x2) && fraction(self.y2),
            MirrorAction::Scroll | MirrorAction::Wheel => point && delta(self.x2) && delta(self.y2),
            MirrorAction::Back
            | MirrorAction::Home
            | MirrorAction::Recents
            | MirrorAction::ScreenOff
            | MirrorAction::ScreenOn
            | MirrorAction::Screenshot => true,
            MirrorAction::Text => self.text.as_ref().is_some_and(|text| sized(text, 1, MAX_MIRROR_TEXT_LEN)),
            MirrorAction::KeyDown | MirrorAction::KeyUp => {
                self.code.is_some_and(|code| (1..=MAX_KEY_CODE).contains(&code))
            }
        }
    }
}

/// Phone to desktop: show the desktop's screen here (remote control), or switch to `source`, an index into the last
/// [`RemoteStarted`]'s list. The video comes on a `mirror-data` stream from the desktop, input as [`RemoteInput`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteSourceKind {
    Monitor,
    /// One window, whose place on the desktop the capture does not tell; the pointer moves only relatively.
    Window,
}

/// A monitor or window the desktop's user shared for remote control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSource {
    pub kind: RemoteSourceKind,
    pub name: String,
}

/// Desktop to phone: the shared sources, and which one the next `mirror-data` stream shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteStarted {
    pub sources: Vec<RemoteSource>,
    pub current: u32,
}

pub const MAX_REMOTE_SOURCES: usize = 16;
pub const MAX_REMOTE_SOURCE_NAME_LEN: usize = 128;

impl RemoteStarted {
    pub(super) fn valid(&self) -> bool {
        (1..=MAX_REMOTE_SOURCES).contains(&self.sources.len())
            && usize::try_from(self.current).is_ok_and(|current| current < self.sources.len())
            && self.sources.iter().all(|source| sized(&source.name, 1, MAX_REMOTE_SOURCE_NAME_LEN))
    }
}

pub(super) fn mirror_size_valid(width: u32, height: u32) -> bool {
    (1..=MAX_MIRROR_SIDE).contains(&width) && (1..=MAX_MIRROR_SIDE).contains(&height)
}
