//! The video of a phone's mirror or camera as linkd hands it over: the socket's framing (a big-endian u32 length and
//! u64 presentation time in microseconds, a flags byte, then the bytes), the codecs, and the `GStreamer` decoder for
//! each. Shared by the mirror window and link-session's webcam node.

use std::io::Read as _;
use std::os::unix::net::UnixStream;

use gstreamer as gst;

pub const UNIT_HEADER: usize = 13;
pub const MAX_UNIT: usize = 4 * 1024 * 1024;
pub const KEYFRAME: u8 = 1;
pub const AUDIO: u8 = 4;
/// The daemon's last unit, empty, when the phone ended mirroring on purpose; a socket closing without it
/// lost the stream.
pub const END: u8 = 8;
/// Camera video only: bits 5-6 hold the quarter turns clockwise that make the picture upright.
const TURNS_SHIFT: u8 = 5;

/// The quarter turns clockwise, 0 to 3, that a camera unit's `flags` ask for.
pub fn quarter_turns(flags: u8) -> u8 {
    (flags >> TURNS_SHIFT) & 3
}

/// The stream's video codec, as `OpenMirrorWith` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "h264" => Some(Self::H264),
            "hevc" => Some(Self::Hevc),
            "av1" => Some(Self::Av1),
            _ => None,
        }
    }

    /// Appsrc caps and parser: Annex B access units for H.264 and HEVC, one temporal unit of low-overhead OBUs for AV1.
    pub fn caps_and_parser(self) -> &'static str {
        match self {
            Self::H264 => "video/x-h264,stream-format=byte-stream,alignment=au ! h264parse",
            Self::Hevc => "video/x-h265,stream-format=byte-stream,alignment=au ! h265parse",
            Self::Av1 => "video/x-av1,stream-format=obu-stream,alignment=tu ! av1parse",
        }
    }

    /// The first of `candidates` whose element is installed.
    pub fn first(candidates: &[&'static str]) -> Option<&'static str> {
        candidates.iter().copied().find(|c| gst::ElementFactory::find(c.split(' ').next().unwrap_or(c)).is_some())
    }

    /// The window's decoder, which the latency test times too: VA-API when the GPU has it. Frame threads would hold
    /// up to one frame per core, so the software decoders split each frame into slices instead.
    pub fn live_decoder(self) -> Option<&'static str> {
        match self {
            Self::H264 => Self::first(&["vah264dec", "avdec_h264 thread-type=slice ! videoconvert"]),
            Self::Hevc => Self::first(&["vah265dec", "avdec_h265 thread-type=slice ! videoconvert"]),
            Self::Av1 => Self::first(&["vaav1dec", "dav1ddec ! videoconvert", "av1dec ! videoconvert"]),
        }
    }

    /// A software decoder for tests, so they run anywhere.
    pub fn soft_decoder(self) -> Option<&'static str> {
        match self {
            Self::H264 => Self::first(&["avdec_h264"]),
            Self::Hevc => Self::first(&["avdec_h265"]),
            Self::Av1 => Self::first(&["dav1ddec", "av1dec"]),
        }
    }
}

/// What this viewer decodes besides H.264, best first, for `OpenMirrorWith`: HEVC only on VA-API (software HEVC costs
/// more CPU than hardware H.264), AV1 on VA-API or dav1d; `soft` for the test decoders. Needs `gst::init`.
pub fn offered(soft: bool) -> Vec<&'static str> {
    let has = |element: &str| gst::ElementFactory::find(element).is_some();
    let mut out = Vec::new();
    if has("h265parse") && (if soft { Codec::Hevc.soft_decoder().is_some() } else { has("vah265dec") }) {
        out.push("hevc");
    }
    if has("av1parse") && (if soft { Codec::Av1.soft_decoder() } else { Codec::Av1.live_decoder() }).is_some() {
        out.push("av1");
    }
    out
}

/// One access unit as the socket frames it.
pub struct Unit {
    pub pts_us: u64,
    pub flags: u8,
    pub data: Vec<u8>,
}

/// The next unit; `None` when the socket closed between units.
pub fn read_unit(socket: &mut UnixStream) -> std::io::Result<Option<Unit>> {
    let mut header = [0; UNIT_HEADER];
    match socket.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let [a, b, c, d, pts @ .., flags] = header;
    let len = u32::from_be_bytes([a, b, c, d]) as usize;
    if len > MAX_UNIT {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "an access unit over 4 MiB"));
    }
    let mut data = vec![0; len];
    socket.read_exact(&mut data)?;
    Ok(Some(Unit { pts_us: u64::from_be_bytes(pts), flags, data }))
}
