//! Desktop sound on a phone's speaker: one `PipeWire` sink, "<phone> speaker", for each phone whose `DeviceInfo` has
//! `speaker`. Sound routed to it goes to the phone as Opus units through `OpenSpeaker`; `SILENCE` of silence or of no
//! buffers closes the stream. `PipeWire` suspends an unused sink, so an idle one costs no work.
//!
//! The sinks are `PipeWire` streams on one thread that runs a `PipeWire` loop: `pipewiresrc` with `autoconnect=false`
//! blocks its state change until an app routes sound to it, and cannot be stopped while it waits.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use anyhow::Context as _;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use pipewire as pw;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::link::Link;

const AUDIO: u8 = 4;
const SILENCE: Duration = Duration::from_secs(1);
/// After a refused `OpenSpeaker`, sound waits this long before the next try.
const HOLD_OFF: Duration = Duration::from_secs(10);
/// 10 ms packets: half a second waits while the stream opens; more is dropped.
const QUEUED: usize = 50;
const RATE: u32 = 48_000;
const CHANNELS: u32 = 2;

/// One Opus packet, and whether its PCM held any sound.
struct Packet {
    pts_us: u64,
    data: Vec<u8>,
    sound: bool,
}

/// Work for the `PipeWire` thread.
enum Command {
    Add { id: String, name: String, input: gst_app::AppSrc },
    Remove(String),
    Quit,
}

pub struct Speakers {
    link: Link,
    sinks: HashMap<String, Sink>,
    commands: pw::channel::Sender<Command>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Speakers {
    pub fn new(link: Link) -> anyhow::Result<Self> {
        gst::init()?;
        gst::ElementFactory::find("opusenc").context("no GStreamer Opus encoder")?;
        let (commands, received) = pw::channel::channel();
        let (ready, started) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new().name("speakers".into()).spawn(move || serve(received, &ready))?;
        started.recv().context("the PipeWire thread ended")??;
        Ok(Self { link, sinks: HashMap::new(), commands, thread: Some(thread) })
    }

    /// Makes the sinks match the phones `DeviceInfo` marks `speaker`.
    pub async fn refresh(&mut self) {
        let (info, devices) = match tokio::try_join!(self.link.proxy.device_info(), self.link.proxy.devices()) {
            Ok(both) => both,
            Err(error) => return log::warn!("reading the phones for their speakers: {error}"),
        };
        let wanted: HashMap<String, String> = devices
            .into_iter()
            .filter(|(id, ..)| {
                let speaker = info.get(id).and_then(|fields| fields.get("speaker"));
                speaker.is_some_and(|value| bool::try_from(value).unwrap_or(false))
            })
            .map(|(id, name, _)| (id, name))
            .collect();
        self.sinks.retain(|id, _| wanted.contains_key(id));
        for (id, name) in wanted {
            if self.sinks.contains_key(&id) {
                continue;
            }
            match Sink::start(self.link.clone(), self.commands.clone(), &id, &name) {
                Ok(sink) => drop(self.sinks.insert(id, sink)),
                Err(error) => log::warn!("{id}: no speaker sink: {error:#}"),
            }
        }
    }
}

impl Drop for Speakers {
    fn drop(&mut self) {
        self.sinks.clear();
        if self.commands.send(Command::Quit).is_ok()
            && let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("the speakers' PipeWire thread panicked");
        }
    }
}

/// Runs the `PipeWire` loop that owns every sink's stream until `Command::Quit`.
fn serve(commands: pw::channel::Receiver<Command>, ready: &std::sync::mpsc::Sender<anyhow::Result<()>>) {
    pw::init();
    let connected = (|| {
        let main_loop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&main_loop, None)?;
        let core = context.connect_rc(None)?;
        Ok::<_, pw::Error>((main_loop, core))
    })();
    let (main_loop, core) = match connected {
        Ok(both) => both,
        Err(error) => return drop(ready.send(Err(anyhow::anyhow!("PipeWire: {error}")))),
    };
    let streams = Rc::new(RefCell::new(HashMap::new()));
    let quit = main_loop.downgrade();
    let _attached = commands.attach(main_loop.loop_(), move |command| match command {
        Command::Add { id, name, input } => match stream(&core, &id, &name, input) {
            Ok(stream) => drop(streams.borrow_mut().insert(id, stream)),
            Err(error) => log::warn!("{id}: no speaker sink: {error}"),
        },
        Command::Remove(id) => drop(streams.borrow_mut().remove(&id)),
        Command::Quit => {
            streams.borrow_mut().clear();
            if let Some(main_loop) = quit.upgrade() {
                main_loop.quit();
            }
        }
    });
    drop(ready.send(Ok(())));
    main_loop.run();
}

/// A stream as an `Audio/Sink` node; never autoconnected, so it carries only what an app or the user routes to it.
fn stream(
    core: &pw::core::CoreRc,
    id: &str,
    name: &str,
    input: gst_app::AppSrc,
) -> Result<(pw::stream::StreamRc, pw::stream::StreamListener<gst_app::AppSrc>), pw::Error> {
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CLASS => "Audio/Sink",
        *pw::keys::NODE_NAME => format!("andromeda-link-speaker-{id}"),
        *pw::keys::NODE_DESCRIPTION => format!("{name} speaker"),
    };
    let stream = pw::stream::StreamRc::new(core.clone(), "phone speaker", props)?;
    let listener = stream
        .add_local_listener_with_user_data(input)
        .process(|stream, input| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
            if let Some(bytes) = data.data().and_then(|bytes| bytes.get(offset..offset + size))
                && !bytes.is_empty()
            {
                // A full appsrc drops its oldest buffer: old sound is worth less than none.
                let _ = input.push_buffer(gst::Buffer::from_slice(bytes.to_vec()));
            }
        })
        .register()?;
    let mut info = pw::spa::param::audio::AudioInfoRaw::new();
    info.set_format(pw::spa::param::audio::AudioFormat::S16LE);
    info.set_rate(RATE);
    info.set_channels(CHANNELS);
    let mut position = [0; pw::spa::sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = pw::spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let format = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    let bytes = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(format),
    )
    .map_err(|_| pw::Error::CreationFailed)?
    .0
    .into_inner();
    let pod = pw::spa::pod::Pod::from_bytes(&bytes).ok_or(pw::Error::CreationFailed)?;
    stream.connect(pw::spa::utils::Direction::Input, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut [pod])?;
    Ok((stream, listener))
}

/// A phone's sink; dropping it removes the node and ends the phone's stream.
struct Sink {
    id: String,
    pipeline: gst::Pipeline,
    commands: pw::channel::Sender<Command>,
    task: tokio::task::JoinHandle<()>,
}

impl Sink {
    fn start(link: Link, commands: pw::channel::Sender<Command>, id: &str, name: &str) -> anyhow::Result<Self> {
        let pipeline = gst::parse::launch(&format!(
            "appsrc name=src is-live=true do-timestamp=true format=time block=false max-bytes=96000 leaky-type=downstream \
             caps=audio/x-raw,format=S16LE,layout=interleaved,rate={RATE},channels={CHANNELS} \
             ! opusenc name=enc bitrate=128000 frame-size=10 ! appsink name=sink sync=false"
        ))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
        let input =
            pipeline.by_name("src").and_then(|src| src.downcast::<gst_app::AppSrc>().ok()).context("no source")?;
        let (packets, received) = mpsc::channel(QUEUED);
        let sound = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let heard = std::sync::Arc::clone(&sound);
        let encoder_input = pipeline.by_name("enc").and_then(|enc| enc.static_pad("sink")).context("no encoder")?;
        encoder_input.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer()
                && let Ok(map) = buffer.map_readable()
            {
                heard.fetch_or(map.iter().any(|byte| *byte != 0), std::sync::atomic::Ordering::Relaxed);
            }
            gst::PadProbeReturn::Ok
        });
        let sink =
            pipeline.by_name("sink").and_then(|sink| sink.downcast::<gst_app::AppSink>().ok()).context("no sink")?;
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                    let packet = Packet {
                        pts_us: buffer.pts().map_or(0, gst::ClockTime::useconds),
                        data: map.to_vec(),
                        sound: sound.swap(false, std::sync::atomic::Ordering::Relaxed),
                    };
                    // A full queue means the phone is behind; old sound is worth less than none.
                    drop(packets.try_send(packet));
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        pipeline.set_state(gst::State::Playing)?;
        commands
            .send(Command::Add { id: id.to_owned(), name: name.to_owned(), input })
            .map_err(|_| anyhow::anyhow!("the PipeWire thread ended"))?;
        log::info!("{id}: speaker sink \"{name} speaker\"");
        let task = tokio::spawn(forward(link, id.to_owned(), received));
        Ok(Self { id: id.to_owned(), pipeline, commands, task })
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        drop(self.commands.send(Command::Remove(std::mem::take(&mut self.id))));
        if let Err(error) = self.pipeline.set_state(gst::State::Null) {
            log::debug!("stopping a speaker sink: {error}");
        }
        self.task.abort();
    }
}

enum Out {
    /// No stream; sound before `hold_off` is dropped.
    Idle { hold_off: Option<Instant> },
    /// `quiet_since` is the first packet of the current silence.
    Playing { socket: tokio::net::UnixStream, quiet_since: Option<Instant> },
}

/// Opens the phone's stream on sound and writes the packets to it; closes it after `SILENCE` of silence or nothing.
async fn forward(link: Link, id: String, mut packets: mpsc::Receiver<Packet>) {
    let mut out = Out::Idle { hold_off: None };
    loop {
        let packet = if matches!(out, Out::Playing { .. }) {
            let Ok(packet) = tokio::time::timeout(SILENCE, packets.recv()).await else {
                log::info!("{id}: no sound for the speaker; closing its stream");
                out = Out::Idle { hold_off: None };
                continue;
            };
            packet
        } else {
            packets.recv().await
        };
        let Some(packet) = packet else { return };
        let now = Instant::now();
        out = match out {
            Out::Idle { hold_off } if !packet.sound || hold_off.is_some_and(|until| now < until) => {
                Out::Idle { hold_off }
            }
            Out::Idle { .. } => match open(&link, &id).await {
                Ok(socket) => Out::Playing { socket, quiet_since: None },
                Err(error) => {
                    log::info!("{id}: the phone's speaker: {error:#}");
                    Out::Idle { hold_off: Some(now + HOLD_OFF) }
                }
            },
            Out::Playing { socket, quiet_since } => {
                let quiet_since = if packet.sound { None } else { quiet_since.or(Some(now)) };
                if quiet_since.is_some_and(|since| now - since >= SILENCE) {
                    log::info!("{id}: silence on the speaker; closing its stream");
                    Out::Idle { hold_off: None }
                } else {
                    Out::Playing { socket, quiet_since }
                }
            }
        };
        if let Out::Playing { socket, .. } = &mut out
            && let Err(error) = write_unit(socket, &packet).await
        {
            log::info!("{id}: the speaker stream ended: {error}");
            out = Out::Idle { hold_off: None };
        }
    }
}

async fn open(link: &Link, id: &str) -> anyhow::Result<tokio::net::UnixStream> {
    let fd: std::os::fd::OwnedFd = link.proxy.open_speaker(id).await?.into();
    let socket = std::os::unix::net::UnixStream::from(fd);
    socket.set_nonblocking(true)?;
    log::info!("{id}: playing desktop sound on the phone");
    Ok(tokio::net::UnixStream::from_std(socket)?)
}

async fn write_unit(socket: &mut tokio::net::UnixStream, packet: &Packet) -> std::io::Result<()> {
    let len = u32::try_from(packet.data.len()).map_err(std::io::Error::other)?;
    let mut header = [0; 13];
    header[..4].copy_from_slice(&len.to_be_bytes());
    header[4..12].copy_from_slice(&packet.pts_us.to_be_bytes());
    header[12] = AUDIO;
    socket.write_all(&header).await?;
    socket.write_all(&packet.data).await
}
