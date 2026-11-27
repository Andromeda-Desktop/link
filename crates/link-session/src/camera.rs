//! A phone camera as a webcam: on `CameraRequested`, `OpenCamera` gives the socket of its video, decoded into a
//! `PipeWire` `Video/Source` node "<phone> camera" with `media.role` Camera, which the camera portal lists to
//! browsers, and into each `v4l2loopback` device, which apps without the portal read. The node exists only while
//! the stream runs.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Context as _;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use link_video::{Codec, END, KEYFRAME, quarter_turns, read_unit};
use tokio::task::JoinHandle;

use crate::link::Link;

/// Where `v4l2loopback` puts its devices; real cameras sit under their bus.
const LOOPBACK_DEVICES: &str = "/sys/devices/virtual/video4linux";

pub struct Cameras {
    running: HashMap<String, JoinHandle<()>>,
}

impl Cameras {
    pub fn new() -> anyhow::Result<Self> {
        gst::init()?;
        gst::ElementFactory::find("pipewiresink").context("no GStreamer PipeWire sink (gst-plugin-pipewire)")?;
        Ok(Self { running: HashMap::new() })
    }

    /// Opens phone `id`'s front or back camera unless one runs already.
    pub fn start(&mut self, link: Link, id: String, front: bool) {
        self.running.retain(|_, task| !task.is_finished());
        if self.running.contains_key(&id) {
            return;
        }
        let key = id.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = serve(&link, &id, front).await {
                log::info!("{id}: the phone's camera: {error:#}");
            }
        });
        self.running.insert(key, task);
    }
}

async fn serve(link: &Link, id: &str, front: bool) -> anyhow::Result<()> {
    let name = link.proxy.devices().await?.into_iter().find(|(device, ..)| device == id).map(|(_, name, _)| name);
    let name = name.unwrap_or_else(|| "Phone".to_owned());
    let offered = link_video::offered(false);
    let (fd, width, height, name_of_codec) = link.proxy.open_camera(id, front, &offered).await?;
    let codec = Codec::from_name(&name_of_codec)
        .filter(|codec| *codec == Codec::H264 || offered.contains(&name_of_codec.as_str()))
        .with_context(|| format!("the phone chose codec {name_of_codec:?}, which was not offered"))?;
    let decoder = codec.live_decoder().with_context(|| format!("no decoder for {codec:?}"))?;
    let mut socket = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    // The phone's H.264 carries no frame rate; without one the node offers 0/1, which Chromium's reader refuses.
    let (caps, parser) = codec.caps_and_parser().split_once(" ! ").context("caps without a parser")?;
    let caps = format!("{caps},framerate={}/1 ! {parser}", link_api::CAMERA_FPS);
    let description = format!("{name} camera");
    log::info!("{id}: \"{description}\" is on at {width}x{height} in {codec:?}");
    let mut wait_keyframe = false;
    let mut turns = 0;
    let ended = loop {
        let (pipeline, src) = pipeline(&caps, decoder, (width, height), turns, &description, id, &loopbacks())?;
        pipeline.set_state(gst::State::Playing)?;
        let bus = pipeline.bus().context("no bus")?;
        let turn = pipeline.by_name("turn").context("no turn element")?;
        let (back, now, fed) = tokio::task::spawn_blocking(move || {
            let fed = feed(&mut socket, &src, &bus, &turn, &mut turns, wait_keyframe);
            (socket, turns, fed)
        })
        .await?;
        socket = back;
        turns = now;
        pipeline.set_state(gst::State::Null)?;
        match fed {
            // A reader that cannot agree on a format fails the PipeWire sink: the node comes back for the next one.
            Fed::Broken { pushed: true, error } => {
                log::info!("{id}: \"{description}\" restarts after: {error:#}");
                if let Err(error) = link.proxy.camera_keyframe(id).await {
                    log::info!("{id}: no camera keyframe: {error}");
                }
                wait_keyframe = true;
            }
            Fed::Broken { error, .. } => break Err(error),
            Fed::Ended(ended) => break ended,
        }
    };
    log::info!("{id}: \"{description}\" is off{}", ended.err().map(|error| format!(": {error:#}")).unwrap_or_default());
    Ok(())
}

/// How [`feed`] stopped.
enum Fed {
    /// The stream ended on purpose, or its socket failed.
    Ended(anyhow::Result<()>),
    /// The pipeline failed; `pushed` is true when it took units before.
    Broken { pushed: bool, error: anyhow::Error },
}

/// The decode pipeline: system-memory YUY2 frames into the `PipeWire` node and each loopback device. Leaky queues keep
/// a slow consumer from holding up the others; nothing waits on the clock, the phone already paces the frames.
/// YUY2 because pipewiresink sends one data block per plane and libwebrtc (Chromium, Firefox) reads only the first.
/// The element `turn` turns the picture upright by `turns` quarter turns; borders keep the size, so readers never
/// renegotiate when the phone turns.
fn pipeline(
    caps: &str,
    decoder: &str,
    (width, height): (u32, u32),
    turns: u8,
    description: &str,
    id: &str,
    loopbacks: &[String],
) -> anyhow::Result<(gst::Pipeline, gst_app::AppSrc)> {
    let quote = |text: &str| text.replace(['"', '\\', ','], " ");
    let direction = direction(turns);
    let convert = if decoder.starts_with("va") {
        format!("vapostproc name=turn add-borders=true video-direction={direction}")
    } else {
        format!("videoflip name=turn video-direction={direction} ! videoscale add-borders=true ! videoconvert")
    };
    let mut launch = format!(
        "appsrc name=src is-live=true format=time caps={caps} ! {decoder} ! {convert} \
         ! video/x-raw,format=YUY2,width={width},height={height},pixel-aspect-ratio=1/1 ! tee name=t \
         t. ! queue leaky=downstream max-size-buffers=2 ! pipewiresink mode=provide sync=false \
         client-name=andromeda-link-camera stream-properties=\"props,media.class=Video/Source,media.role=Camera,\
         node.name=andromeda-link-camera-{id},node.description=\\\"{}\\\"\"",
        quote(description)
    );
    for device in loopbacks {
        let _ = write!(launch, " t. ! queue leaky=downstream max-size-buffers=2 ! v4l2sink device={device} sync=false");
    }
    let pipeline =
        gst::parse::launch(&launch)?.downcast::<gst::Pipeline>().map_err(|_| anyhow::anyhow!("not a pipeline"))?;
    let src = pipeline
        .by_name("src")
        .context("no appsrc")?
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| anyhow::anyhow!("not an appsrc"))?;
    Ok((pipeline, src))
}

/// The `video-direction` that turns a picture `turns` quarter turns clockwise.
fn direction(turns: u8) -> &'static str {
    match turns {
        1 => "90r",
        2 => "180",
        3 => "90l",
        _ => "identity",
    }
}

/// The `v4l2loopback` devices, as `/dev/videoN` paths.
fn loopbacks() -> Vec<String> {
    std::fs::read_dir(LOOPBACK_DEVICES)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| format!("/dev/{}", entry.file_name().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default()
}

/// Pushes the phone's units into the decoder, from a keyframe when `wait_keyframe`, until the stream ends on purpose
/// (`END`), the socket closes, or the pipeline posts an error. Each unit's quarter turns set `turn` when they change.
fn feed(
    socket: &mut std::os::unix::net::UnixStream,
    src: &gst_app::AppSrc,
    bus: &gst::Bus,
    turn: &gst::Element,
    turns: &mut u8,
    mut wait_keyframe: bool,
) -> Fed {
    let mut first = None;
    let mut pushed = false;
    loop {
        let unit = match read_unit(socket) {
            Ok(Some(unit)) if unit.flags & END == 0 => unit,
            Ok(_) => return Fed::Ended(Ok(())),
            Err(error) => return Fed::Ended(Err(error.into())),
        };
        if wait_keyframe && unit.flags & KEYFRAME == 0 {
            continue;
        }
        wait_keyframe = false;
        if quarter_turns(unit.flags) != *turns {
            *turns = quarter_turns(unit.flags);
            turn.set_property_from_str("video-direction", direction(*turns));
            log::info!("camera picture turns {} quarter turns clockwise", *turns);
        }
        let base = *first.get_or_insert(unit.pts_us);
        let mut buffer = gst::Buffer::from_mut_slice(unit.data);
        if let Some(buffer) = buffer.get_mut() {
            buffer.set_pts(gst::ClockTime::from_useconds(unit.pts_us.saturating_sub(base)));
            if unit.flags & KEYFRAME == 0 {
                buffer.set_flags(gst::BufferFlags::DELTA_UNIT);
            }
        }
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
            && let gst::MessageView::Error(error) = message.view()
        {
            return Fed::Broken { pushed, error: anyhow::anyhow!("{}", error.error()) };
        }
        if let Err(error) = src.push_buffer(buffer) {
            return Fed::Broken { pushed, error: anyhow::anyhow!("the decoder stopped: {error}") };
        }
        pushed = true;
    }
}
