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
use link_video::{Codec, END, KEYFRAME, read_unit};
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
    let socket = std::os::unix::net::UnixStream::from(std::os::fd::OwnedFd::from(fd));
    let (pipeline, src) = pipeline(codec.caps_and_parser(), decoder, &format!("{name} camera"), id, &loopbacks())?;
    pipeline.set_state(gst::State::Playing)?;
    log::info!("{id}: \"{name} camera\" is on at {width}x{height} in {codec:?}");
    let ended = tokio::task::spawn_blocking(move || feed(socket, &src)).await?;
    pipeline.set_state(gst::State::Null)?;
    log::info!("{id}: \"{name} camera\" is off{}", ended.err().map(|error| format!(": {error:#}")).unwrap_or_default());
    Ok(())
}

/// The decode pipeline: system-memory frames into the `PipeWire` node and each loopback device. Leaky queues keep a
/// slow consumer from holding up the others; nothing waits on the clock, the phone already paces the frames.
fn pipeline(
    caps: &str,
    decoder: &str,
    description: &str,
    id: &str,
    loopbacks: &[String],
) -> anyhow::Result<(gst::Pipeline, gst_app::AppSrc)> {
    let quote = |text: &str| text.replace(['"', '\\', ','], " ");
    let mut launch = format!(
        "appsrc name=src is-live=true format=time caps={caps} ! {decoder} ! video/x-raw ! tee name=t \
         t. ! queue leaky=downstream max-size-buffers=2 ! pipewiresink mode=provide sync=false \
         client-name=andromeda-link-camera stream-properties=\"props,media.class=Video/Source,media.role=Camera,\
         node.name=andromeda-link-camera-{id},node.description=\\\"{}\\\"\"",
        quote(description)
    );
    for device in loopbacks {
        let _ = write!(
            launch,
            " t. ! queue leaky=downstream max-size-buffers=2 ! videoconvert ! video/x-raw,format=YUY2 \
             ! v4l2sink device={device} sync=false"
        );
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

/// Pushes the phone's units into the decoder until the stream ends on purpose (`END`) or the socket closes.
fn feed(mut socket: std::os::unix::net::UnixStream, src: &gst_app::AppSrc) -> anyhow::Result<()> {
    let mut first = None;
    while let Some(unit) = read_unit(&mut socket)? {
        if unit.flags & END != 0 {
            break;
        }
        let base = *first.get_or_insert(unit.pts_us);
        let mut buffer = gst::Buffer::from_mut_slice(unit.data);
        if let Some(buffer) = buffer.get_mut() {
            buffer.set_pts(gst::ClockTime::from_useconds(unit.pts_us.saturating_sub(base)));
            if unit.flags & KEYFRAME == 0 {
                buffer.set_flags(gst::BufferFlags::DELTA_UNIT);
            }
        }
        src.push_buffer(buffer).context("the decoder stopped")?;
    }
    Ok(())
}
