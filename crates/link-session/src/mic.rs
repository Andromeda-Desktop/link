//! A phone's microphone as a `PipeWire` source, "<phone> microphone": on `MicRequested`, `OpenMic` gives the socket of
//! the phone's Opus units, decoded into a source (`audio_nodes`) that is the default while it runs. The socket's end
//! removes the source, and the old default comes back. A phone's media sound comes the same way (`PhoneSoundRequested`,
//! `OpenPhoneSound`) and plays to the default sink as "<phone> sound".

use std::collections::HashMap;

use anyhow::Context as _;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use tokio::io::AsyncReadExt as _;
use tokio::task::JoinHandle;

use crate::audio_nodes::{Node, Nodes, Pcm, RATE};
use crate::link::Link;

const AUDIO: u8 = 4;
const END: u8 = 8;
const MAX_UNIT: usize = 64 * 1024;

/// What a phone streams to this desktop on its own.
#[derive(Clone, Copy)]
pub enum Kind {
    Mic,
    Sound,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Mic => "microphone",
            Self::Sound => "sound",
        }
    }

    fn channels(self) -> usize {
        match self {
            Self::Mic => 1,
            Self::Sound => 2,
        }
    }
}

pub struct Mics {
    nodes: Nodes,
    kind: Kind,
    running: HashMap<String, JoinHandle<()>>,
}

impl Mics {
    pub fn new(nodes: Nodes, kind: Kind) -> anyhow::Result<Self> {
        gst::init()?;
        gst::ElementFactory::find("opusdec").context("no GStreamer Opus decoder")?;
        Ok(Self { nodes, kind, running: HashMap::new() })
    }

    /// Opens phone `id`'s microphone or sound unless it runs already.
    pub fn start(&mut self, link: Link, id: String) {
        self.running.retain(|_, task| !task.is_finished());
        if self.running.contains_key(&id) {
            return;
        }
        let task = tokio::spawn(run(link, self.nodes.clone(), self.kind, id.clone()));
        self.running.insert(id, task);
    }
}

async fn run(link: Link, nodes: Nodes, kind: Kind, id: String) {
    if let Err(error) = serve(&link, &nodes, kind, &id).await {
        log::info!("{id}: the phone's {}: {error:#}", kind.name());
    }
}

async fn serve(link: &Link, nodes: &Nodes, kind: Kind, id: &str) -> anyhow::Result<()> {
    let name = link.proxy.devices().await?.into_iter().find(|(device, ..)| device == id).map(|(_, name, _)| name);
    let name = name.unwrap_or_else(|| "Phone".to_owned());
    let fd: std::os::fd::OwnedFd = match kind {
        Kind::Mic => link.proxy.open_mic(id).await?.into(),
        Kind::Sound => link.proxy.open_phone_sound(id).await?.into(),
    };
    let socket = std::os::unix::net::UnixStream::from(fd);
    socket.set_nonblocking(true)?;
    let mut socket = tokio::net::UnixStream::from_std(socket)?;
    let (node, pcm) = match kind {
        Kind::Mic => nodes.source(id, &name)?,
        Kind::Sound => nodes.playback(id, &name)?,
    };
    let decoder = Decoder::start(node, pcm, kind.channels())?;
    let default = match kind {
        Kind::Mic => Default::take(&format!("andromeda-link-mic-{id}")).await,
        Kind::Sound => None,
    };
    log::info!("{id}: \"{name} {}\" is on", kind.name());
    let ended = pump(&mut socket, &decoder).await;
    if let Some(default) = default {
        default.give_back().await;
    }
    drop(decoder);
    log::info!(
        "{id}: \"{name} {}\" is off{}",
        kind.name(),
        ended.err().map(|error| format!(": {error:#}")).unwrap_or_default()
    );
    Ok(())
}

/// Feeds the phone's units into the decoder until the stream ends.
async fn pump(socket: &mut tokio::net::UnixStream, decoder: &Decoder) -> anyhow::Result<()> {
    let mut header = [0; 13];
    loop {
        if let Err(error) = socket.read_exact(&mut header).await {
            return if error.kind() == std::io::ErrorKind::UnexpectedEof { Ok(()) } else { Err(error.into()) };
        }
        let [a, b, c, d, .., flags] = header;
        let len = u32::from_be_bytes([a, b, c, d]) as usize;
        anyhow::ensure!(len <= MAX_UNIT, "a sound unit over 64 KiB");
        let mut data = vec![0; len];
        socket.read_exact(&mut data).await?;
        if flags & END != 0 {
            return Ok(());
        }
        if flags & AUDIO != 0 {
            decoder.push(data)?;
        }
    }
}

/// The user's configured default source (`default.configured.audio.source`, what `wpctl set-default` writes), set to
/// the phone's microphone while it runs: `WirePlumber` ranks sources the user picked before above any
/// `priority.session`, so only the configured key makes it the default.
struct Default {
    ours: String,
    before: Option<String>,
}

impl Default {
    async fn take(ours: &str) -> Option<Self> {
        let before = match configured().await {
            Ok(before) => before,
            Err(error) => {
                log::warn!("reading the default source: {error:#}");
                return None;
            }
        };
        if let Err(error) = set_configured(Some(&format!("{{\"name\":\"{ours}\"}}"))).await {
            log::warn!("making the phone's microphone the default: {error:#}");
            return None;
        }
        Some(Self { ours: ours.to_owned(), before })
    }

    /// Puts the old choice back unless the user picked another source meanwhile.
    async fn give_back(self) {
        let still_ours = configured().await.ok().flatten().is_some_and(|now| name_of(&now) == Some(self.ours.as_str()));
        if still_ours && let Err(error) = set_configured(self.before.as_deref()).await {
            log::warn!("restoring the default source: {error:#}");
        }
    }
}

const CONFIGURED: &str = "default.configured.audio.source";

/// The key's JSON value; None when the user never picked a source.
async fn configured() -> anyhow::Result<Option<String>> {
    let output = tokio::process::Command::new("pw-metadata").args(["-n", "default", "0", CONFIGURED]).output().await?;
    anyhow::ensure!(output.status.success(), "pw-metadata: {}", String::from_utf8_lossy(&output.stderr).trim());
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().find_map(|line| {
        let value = line.split_once(&format!("key:'{CONFIGURED}' value:'"))?.1;
        Some(value.rsplit_once("' type:")?.0.to_owned())
    }))
}

async fn set_configured(value: Option<&str>) -> anyhow::Result<()> {
    let mut command = tokio::process::Command::new("pw-metadata");
    match value {
        Some(value) => command.args(["-n", "default", "0", CONFIGURED, value, "Spa:String:JSON"]),
        None => command.args(["-n", "default", "-d", "0", CONFIGURED]),
    };
    let output = command.output().await?;
    anyhow::ensure!(output.status.success(), "pw-metadata: {}", String::from_utf8_lossy(&output.stderr).trim());
    Ok(())
}

fn name_of(json: &str) -> Option<&str> {
    let rest = json.split_once("\"name\"")?.1.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    Some(rest.split_once('"')?.0)
}

/// The decoder and the node it feeds; dropping it removes the node.
struct Decoder {
    _node: Node,
    pipeline: gst::Pipeline,
    input: gst_app::AppSrc,
}

impl Decoder {
    fn start(node: Node, pcm: Pcm, channels: usize) -> anyhow::Result<Self> {
        let pipeline = gst::parse::launch(&format!(
            "appsrc name=input is-live=true format=time do-timestamp=true \
             caps=audio/x-opus,channel-mapping-family=0,channels={channels},rate={RATE} \
             ! opusdec plc=true ! audioconvert ! audioresample \
             ! audio/x-raw,format=S16LE,layout=interleaved,rate={RATE},channels={channels} ! appsink name=output sync=false"
        ))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
        let input = pipeline.by_name("input").and_then(|input| input.downcast::<gst_app::AppSrc>().ok());
        let input = input.context("no input")?;
        let output = pipeline.by_name("output").and_then(|output| output.downcast::<gst_app::AppSink>().ok());
        let output = output.context("no output")?;
        output.set_callbacks(gst_app::AppSinkCallbacks::builder().new_sample(move |sink| queue(&pcm, sink)).build());
        pipeline.set_state(gst::State::Playing)?;
        Ok(Self { _node: node, pipeline, input })
    }

    fn push(&self, data: Vec<u8>) -> anyhow::Result<()> {
        self.input.push_buffer(gst::Buffer::from_mut_slice(data)).map_err(|flow| anyhow::anyhow!("{flow:?}"))?;
        Ok(())
    }
}

fn queue(pcm: &Pcm, sink: &gst_app::AppSink) -> Result<gst::FlowSuccess, gst::FlowError> {
    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
    let map = sample.buffer().ok_or(gst::FlowError::Error)?.map_readable().map_err(|_| gst::FlowError::Error)?;
    pcm.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(&map);
    Ok(gst::FlowSuccess::Ok)
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if let Err(error) = self.pipeline.set_state(gst::State::Null) {
            log::debug!("stopping a phone's sound decoder: {error}");
        }
    }
}
