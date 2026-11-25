//! A phone's microphone as a `PipeWire` source, "<phone> microphone": on `MicRequested`, `OpenMic` gives the socket of
//! the phone's Opus units, decoded into a virtual source that ranks above built-in microphones, so it is the default
//! while it runs. The socket's end removes the source, and the old default comes back.

use std::collections::HashMap;

use anyhow::Context as _;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use tokio::io::AsyncReadExt as _;
use tokio::task::JoinHandle;

use crate::link::Link;

const AUDIO: u8 = 4;
const END: u8 = 8;
const MAX_UNIT: usize = 64 * 1024;
/// Above the built-in sources' session priority (ALSA cards rank in the low thousands), so this one is the default.
const PRIORITY: i32 = 3000;

#[derive(Default)]
pub struct Mics {
    running: HashMap<String, JoinHandle<()>>,
}

impl Mics {
    pub fn available() -> anyhow::Result<()> {
        gst::init()?;
        gst::ElementFactory::find("pipewiresink").context("no GStreamer PipeWire plugin")?;
        gst::ElementFactory::find("opusdec").context("no GStreamer Opus decoder")?;
        Ok(())
    }

    /// Opens phone `id`'s microphone unless it runs already.
    pub fn start(&mut self, link: Link, id: String) {
        self.running.retain(|_, task| !task.is_finished());
        if self.running.contains_key(&id) {
            return;
        }
        let task = tokio::spawn(run(link, id.clone()));
        self.running.insert(id, task);
    }
}

async fn run(link: Link, id: String) {
    if let Err(error) = serve(&link, &id).await {
        log::info!("{id}: the phone's microphone: {error:#}");
    }
}

async fn serve(link: &Link, id: &str) -> anyhow::Result<()> {
    let name = link.proxy.devices().await?.into_iter().find(|(device, ..)| device == id).map(|(_, name, _)| name);
    let name = name.unwrap_or_else(|| "Phone".to_owned());
    let fd: std::os::fd::OwnedFd = link.proxy.open_mic(id).await?.into();
    let socket = std::os::unix::net::UnixStream::from(fd);
    socket.set_nonblocking(true)?;
    let mut socket = tokio::net::UnixStream::from_std(socket)?;
    let source = Source::start(id, &name)?;
    log::info!("{id}: \"{name} microphone\" is on");
    let mut header = [0; 13];
    let ended = loop {
        if let Err(error) = socket.read_exact(&mut header).await {
            break if error.kind() == std::io::ErrorKind::UnexpectedEof { None } else { Some(error) };
        }
        let [a, b, c, d, .., flags] = header;
        let len = u32::from_be_bytes([a, b, c, d]) as usize;
        anyhow::ensure!(len <= MAX_UNIT, "a microphone unit over 64 KiB");
        let mut data = vec![0; len];
        socket.read_exact(&mut data).await?;
        if flags & END != 0 {
            break None;
        }
        if flags & AUDIO != 0 {
            source.push(data)?;
        }
    };
    log::info!("{id}: \"{name} microphone\" is off{}", ended.map(|error| format!(": {error}")).unwrap_or_default());
    Ok(())
}

/// The decoder and the virtual source; dropping it removes the source.
struct Source {
    pipeline: gst::Pipeline,
    input: gst_app::AppSrc,
}

impl Source {
    fn start(id: &str, name: &str) -> anyhow::Result<Self> {
        let pipeline = gst::parse::launch(
            "appsrc name=input is-live=true format=time do-timestamp=true \
             caps=audio/x-opus,channel-mapping-family=0,channels=1,rate=48000 \
             ! opusdec plc=true ! audioconvert ! audioresample ! audio/x-raw,rate=48000,channels=1 \
             ! pipewiresink name=output mode=provide",
        )?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
        let props = gst::Structure::builder("props")
            .field("media.class", "Audio/Source/Virtual")
            .field("node.name", format!("andromeda-link-mic-{id}"))
            .field("node.description", format!("{name} microphone"))
            .field("priority.session", PRIORITY)
            .build();
        pipeline.by_name("output").context("no output")?.set_property("stream-properties", props);
        let input = pipeline.by_name("input").and_then(|input| input.downcast::<gst_app::AppSrc>().ok());
        let input = input.context("no input")?;
        pipeline.set_state(gst::State::Playing)?;
        Ok(Self { pipeline, input })
    }

    fn push(&self, data: Vec<u8>) -> anyhow::Result<()> {
        self.input.push_buffer(gst::Buffer::from_mut_slice(data)).map_err(|flow| anyhow::anyhow!("{flow:?}"))?;
        Ok(())
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Err(error) = self.pipeline.set_state(gst::State::Null) {
            log::debug!("stopping a microphone source: {error}");
        }
    }
}
